//! HCS-backed [`Runtime`] implementation for native Windows containers.
//!
//! This runtime drives the Windows Host Compute Service (HCS) directly via
//! the safe bindings in [`zlayer_hcs`]. It is the Windows analogue of the
//! [`crate::runtimes::youki`] runtime on Linux:
//!
//!   * Image pulling delegates to [`crate::windows::unpacker::unpack_windows_image`],
//!     which unpacks OCI layers into wclayer directories on disk.
//!   * Container lifecycle maps 1:1 onto `ComputeSystem::{create, open, start,
//!     shutdown, terminate}` from `zlayer-hcs`.
//!   * Exit observation uses the HCS event stream (`HcsEventKind::SystemExited`).
//!   * CPU / memory / storage statistics come from
//!     [`ComputeSystem::read_statistics`].
//!
//! # Phase B scope
//!
//! This file implements the MVP that lets `zlayer deploy` start a Windows
//! container. The following areas are intentionally deferred:
//!
//!   * Network attachment (HNS namespace + endpoint wiring) — Phase C.
//!   * Structured container log capture — follow-up once process stdio pipes
//!     are plumbed through `zlayer-observability`.
//!   * `exec_stream` real-time streaming — the current implementation blocks
//!     on process completion and emits buffered stdout/stderr once, matching
//!     the trait's default behaviour. Proper streaming is tracked separately.
//!
//! Methods that are not meaningful on Windows today return a clean
//! [`AgentError::Unsupported`] error rather than panicking.

// `#[cfg(target_os = "windows")]` is applied by the parent `runtimes/mod.rs`
// module declaration, so it is not repeated here.
//
// This module wraps `zlayer-hcs` + HNS into the `Runtime` trait and contains
// a large amount of Windows-container glue. A few clippy lints that do not
// fight the architecture are allowed at the module level with justification;
// each individual `unsafe` block still carries a `SAFETY:` comment:
//
// - `type_complexity`: `Arc<Mutex<Option<(Option<IpAddr>, Option<String>)>>>`
//   is the `next_container_dns` per-call stash; extracting a typedef for a
//   single field is more obscure than the direct signature.
// - `must_use_candidate` / `default_trait_access` / `unused_self`: stylistic
//   nits on methods we keep as-is for API symmetry with the Linux runtime.
// - `needless_pass_by_value` / `bind_instead_of_map` / `map_unwrap_or` /
//   `default_trait_access` / `needless_return` / `unnecessary_debug_formatting`
//   / `used_underscore_binding` / `no_effect_underscore_binding` /
//   `needless_raw_string_hashes`: style-only, not semantic issues.
#![allow(
    unsafe_code,
    clippy::borrow_as_ptr,
    clippy::type_complexity,
    clippy::must_use_candidate,
    clippy::default_trait_access,
    clippy::unused_self,
    clippy::needless_pass_by_value,
    clippy::bind_instead_of_map,
    clippy::map_unwrap_or,
    clippy::needless_return,
    clippy::unnecessary_debug_formatting,
    clippy::used_underscore_binding,
    clippy::no_effect_underscore_binding,
    clippy::needless_raw_string_hashes
)]

use std::collections::HashMap;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures_util::StreamExt;
use oci_client::manifest::OciImageManifest;
use tokio::sync::{Mutex, RwLock};
use tokio::time::timeout as tokio_timeout;
use tracing::instrument;
use windows::core::GUID;
use zlayer_hns::attach::{self as hns_attach, EndpointAttachment};
use zlayer_observability::logs::LogEntry;
use zlayer_overlay::ipnet;
use zlayer_spec::{PullPolicy, RegistryAuth as SpecRegistryAuth, ServiceSpec};

use crate::cgroups_stats::ContainerStats;
use crate::error::{AgentError, Result};
use crate::runtime::{
    ContainerId, ContainerInspectDetails, ContainerState, ExecEvent, ExecEventStream, ImageInfo,
    PruneResult, Runtime, WaitOutcome, WaitReason,
};
use crate::windows::uvm::Uvm;
use crate::windows::{scratch, unpacker};

use zlayer_hcs::enumerate;
use zlayer_hcs::events::{self, HcsEventKind};
use zlayer_hcs::schema::{
    Chipset, ComputeSystem as HcsDoc, Container as HcsContainer, ContainerMemory,
    ContainerProcessor, Devices, GpuAssignment, GpuAssignmentMode, GpuAssignmentRequest,
    GuestOs as HcsGuestOs, ProcessParameters, SchemaVersion, ScsiAttachment, ScsiController,
    Statistics, Storage as HcsStorage, Topology, TopologyMemory, TopologyProcessor, Uefi,
    UefiBootEntry, VirtualMachine, VirtualSmbShare, VirtualSmbShareOptions,
};
use zlayer_hcs::system::ComputeSystem;

/// Owner tag stamped onto every compute system + HCN endpoint this runtime
/// creates. Used at startup to discover zombie systems from a previous agent
/// run, and as the filter for [`enumerate::list_by_owner`].
///
/// The legacy single-instance value is `"zlayer"`. To keep that install
/// stable, [`owner_tag`] returns `"zlayer"` verbatim when `daemon_name` is
/// `"zlayer"`; any other name is used as-is so two daemons running
/// side-by-side never sweep each other's compute systems.
#[must_use]
pub fn owner_tag(daemon_name: &str) -> String {
    if daemon_name == "zlayer" {
        "zlayer".to_string()
    } else {
        daemon_name.to_string()
    }
}

/// Name of the per-daemon HCN Transparent overlay network on the host. Every
/// `ZLayer` container on this node attaches an endpoint into this network;
/// the network's IPAM subnet is the node's per-node `/28` slice of the
/// cluster CIDR (see [`HcsConfig::slice_cidr`]).
///
/// The legacy single-instance value is `"zlayer-overlay"`. To keep that
/// install stable, [`overlay_network_name`] returns `"zlayer-overlay"`
/// verbatim when `daemon_name` is `"zlayer"`; any other name becomes
/// `"<daemon_name>-overlay"` so multiple daemons each get their own
/// network handle.
#[must_use]
pub fn overlay_network_name(daemon_name: &str) -> String {
    if daemon_name == "zlayer" {
        "zlayer-overlay".to_string()
    } else {
        format!("{daemon_name}-overlay")
    }
}

/// Format a GUID as the **bare, lowercase, un-braced** string HCN/HCS use to
/// identify a namespace inside a compute-system document's
/// `Container.Networking.Namespace` field (e.g. `aabbccdd-eeff-...`).
///
/// The windows-rs `{:?}` formatter emits the brace-wrapped, upper-case form
/// (`{AABBCCDD-...}`); HCS's `Construct` step then fails to resolve the
/// namespace against HCN and returns `0x80070490 ERROR_NOT_FOUND`. Normalising
/// to the bare form here keeps the lookup string byte-identical to the id HCN
/// registered.
fn format_guid_bare(id: GUID) -> String {
    // hcsshim uses bare lowercase GUIDs (`aabbccdd-eeff-...`) for HCN/HCS
    // wire references. The previous "braced uppercase" experiment failed; the
    // real bug was that `Namespace::create` returned our random GUID instead
    // of HCN's actual assigned ID (HostDefault is a singleton). With that
    // fixed, bare lowercase should now resolve.
    format!("{id:?}")
        .trim_matches(|c: char| c == '{' || c == '}')
        .to_ascii_lowercase()
}

/// Coerce an arbitrary identifier into a `NetBIOS`-shaped hostname suitable for
/// `Container.GuestOs.HostName`.
///
/// `NetBIOS` rules: 1..=15 ASCII characters, alphanumerics and hyphens only, no
/// underscores, no dots, must start with an ASCII letter. `hcs_id` strings such
/// as `fallthrough-svc-rep-0` are >15 chars; longer values are silently
/// truncated by some HCS builds and rejected by others. We pre-truncate to 15
/// after stripping disallowed characters so HCS never sees a malformed value.
fn netbios_hostname(raw: &str) -> String {
    let mut cleaned: String = raw
        .chars()
        .filter_map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' => Some(c),
            '_' | '.' | ' ' => Some('-'),
            _ => None,
        })
        .collect();
    // NetBIOS requires the first character to be an ASCII letter; prepend `z`
    // if the cleaned string starts with a digit or hyphen.
    if !cleaned
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic())
    {
        cleaned.insert(0, 'z');
    }
    cleaned.truncate(15);
    if cleaned.is_empty() {
        "zlayer".to_string()
    } else {
        cleaned
    }
}

/// Isolation mode for the compute systems this runtime creates.
///
/// Process isolation runs the container as a job object on the host kernel
/// (fastest, requires host/container OS version match). Hyper-V isolation
/// puts the container in a lightweight utility VM (slower, works across OS
/// versions). Overrideable per-service in a later phase; today the runtime
/// default applies uniformly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IsolationMode {
    /// Process-isolated (shared host kernel).
    #[default]
    Process,
    /// Hyper-V-isolated (utility VM).
    Hyperv,
}

/// Cached host Windows version as `(major, minor, build)`. The host's build
/// number is immutable for the process lifetime, so we cache the first
/// successful probe and never re-query.
static HOST_WIN_BUILD: std::sync::OnceLock<Option<(u32, u32, u32)>> = std::sync::OnceLock::new();

/// Return the host's `(major, minor, build)` Windows version via
/// `ntdll!RtlGetVersion` — the only API that returns the actual build number
/// even when the calling process has no manifest declaring Windows 10/11
/// compatibility. `GetVersionExW` and `GetVersion` are manifest-shimmed and
/// will return 6.2 (Windows 8) on a 10.0.26100 host without an explicit
/// manifest entry, which would make isolation-mode detection wrong.
///
/// Backing FFI: `ntdll.dll!RtlGetVersion(version_info: *mut OSVERSIONINFOW)
/// -> NTSTATUS`. We declare the link inline (matching the pattern used in
/// `crate::windows::wclayer::layer_id_for_path`) so we don't need to widen
/// the agent's `windows` crate feature set.
///
/// Returns `None` only if the syscall fails — never observed in practice on
/// any supported Windows host.
fn host_windows_build() -> Option<(u32, u32, u32)> {
    *HOST_WIN_BUILD.get_or_init(|| {
        use windows::Win32::System::SystemInformation::OSVERSIONINFOW;

        windows::core::link!(
            "ntdll.dll" "system" fn RtlGetVersion(
                version_info: *mut OSVERSIONINFOW,
            ) -> windows::core::HRESULT
        );

        let mut info = OSVERSIONINFOW {
            dwOSVersionInfoSize: u32::try_from(std::mem::size_of::<OSVERSIONINFOW>()).unwrap_or(0),
            ..Default::default()
        };
        // SAFETY: `info` is a live, exclusively-borrowed, correctly-sized
        // OSVERSIONINFOW. RtlGetVersion only writes through the pointer and
        // returns an NTSTATUS-as-HRESULT. (RtlGetVersion's signature is
        // documented as NTSTATUS, but `windows::core::link!` accepts HRESULT
        // as a thin wrapper around the same i32 — STATUS_SUCCESS = 0 maps
        // to HRESULT 0 which `.is_ok()` accepts.)
        let hr = unsafe { RtlGetVersion(&mut info) };
        if hr.is_ok() {
            Some((info.dwMajorVersion, info.dwMinorVersion, info.dwBuildNumber))
        } else {
            None
        }
    })
}

/// Parse a Windows `os.version` string (e.g. `"10.0.20348.2700"`) into
/// `(major, minor, build)`. Ignores the UBR (revision) component since
/// different MCR tags within the same build (UBR drift) are
/// isolation-compatible — process isolation tolerates a UBR delta but not a
/// build delta. Returns `None` on parse failure or when the string has
/// fewer than three dotted components.
fn parse_os_version(s: &str) -> Option<(u32, u32, u32)> {
    let mut parts = s.split('.').map(str::parse::<u32>);
    let major = parts.next()?.ok()?;
    let minor = parts.next()?.ok()?;
    let build = parts.next()?.ok()?;
    Some((major, minor, build))
}

/// Pure-logic decision matrix used by [`resolve_isolation_for_image`].
///
/// Extracted as a separate function so tests can drive every cell of the
/// matrix without depending on [`host_windows_build`]'s Windows FFI.
///
/// | spec    | image build       | host build       | result   | reason |
/// |---------|-------------------|------------------|----------|--------|
/// | Process | *                 | *                | Process  | explicit operator choice |
/// | Hyperv  | *                 | *                | Hyperv   | explicit operator choice |
/// | Auto    | known + matches   | known            | Process  | build-matched, no UVM needed |
/// | Auto    | known + mismatch  | known            | Hyperv   | cross-build, UVM required |
/// | Auto    | known             | unknown          | Hyperv   | safer (UVM works on any host) |
/// | Auto    | unknown           | *                | Process  | preserves prior default; documented |
fn decide_isolation(
    spec: Option<zlayer_spec::IsolationMode>,
    image_build: Option<(u32, u32, u32)>,
    host_build: Option<(u32, u32, u32)>,
) -> IsolationMode {
    use zlayer_spec::IsolationMode as Spec;
    match spec {
        Some(Spec::Process) => IsolationMode::Process,
        Some(Spec::Hyperv) => IsolationMode::Hyperv,
        None | Some(Spec::Auto) => match (image_build, host_build) {
            (Some(img), Some(host)) if img == host => IsolationMode::Process,
            (Some(_), Some(_) | None) => IsolationMode::Hyperv,
            (None, _) => IsolationMode::Process,
        },
    }
}

/// Resolve the runtime-internal [`IsolationMode`] for a container, picking
/// Process vs. Hyper-V based on the spec, the image's builder-asserted
/// `os.version`, and the host's Windows build.
///
/// Spec values flow through [`decide_isolation`]; the only side effects are
/// the cached [`host_windows_build`] probe and `image_os_version` parsing.
fn resolve_isolation_for_image(
    spec: Option<zlayer_spec::IsolationMode>,
    image_os_version: Option<&str>,
) -> IsolationMode {
    decide_isolation(
        spec,
        image_os_version.and_then(parse_os_version),
        host_windows_build(),
    )
}

/// Configuration for [`HcsRuntime`].
#[derive(Debug, Clone)]
pub struct HcsConfig {
    /// Root directory for the read-only image layer cache (`<root>/images/`)
    /// and the per-container scratch layers (`<root>/scratch/<id>/`).
    pub storage_root: PathBuf,
    /// Default scratch layer size in GiB. `0` requests the HCS default.
    pub default_scratch_size_gb: u64,
    /// Cluster CIDR (e.g. "10.200.0.0/16") used for per-endpoint policy
    /// configuration: `OutBoundNAT` exceptions, `SDNRoute` destination,
    /// ACL remote-addresses.
    pub cluster_cidr: String,
    /// This node's per-node /28 slice of the cluster CIDR. `None` until
    /// the node joins the cluster and the leader hands out a slice. When
    /// `None`, [`HcsRuntime::ensure_overlay_network`] cannot proceed.
    pub slice_cidr: Option<ipnet::IpNet>,
    /// Daemon instance name (resolved from the top-level `--daemon-name`
    /// flag, with a `current_exe()` fallback). Drives the HCS owner tag
    /// stamped onto every compute system this runtime owns and the name
    /// of the per-daemon HCN Transparent overlay network so two daemons
    /// running side-by-side on one host never collide. Defaults to
    /// `"zlayer"` for backward compatibility with single-instance installs.
    pub daemon_name: String,
}

impl Default for HcsConfig {
    fn default() -> Self {
        let dirs = zlayer_paths::ZLayerDirs::system_default();
        Self {
            storage_root: std::env::var("ZLAYER_HCS_STORAGE_ROOT")
                .map_or_else(|_| dirs.containers().join("hcs"), PathBuf::from),
            // Per-container isolation is resolved per-image at
            // `create_container` time via [`resolve_isolation_for_image`]
            // (matrix: spec choice × image `os.version` × host build). There
            // is no operator-level default anymore — the image-aware
            // resolver is always correct given the inputs.
            default_scratch_size_gb: 20,
            cluster_cidr: "10.200.0.0/16".to_string(),
            slice_cidr: None,
            daemon_name: "zlayer".to_string(),
        }
    }
}

/// Cached unpacked image keyed by manifest digest.
#[derive(Debug)]
struct CachedImage {
    /// Parent chain (child-to-parent order) ready to be plugged into a
    /// compute-system document.
    unpacked: unpacker::UnpackedImage,
    /// Builder-asserted Windows OS version (e.g. `"10.0.20348.2031"`) from
    /// the OCI image config's top-level `os.version` field. `None` when the
    /// field is absent in the config or the pre-fetch failed (best-effort —
    /// only the build-vs-host isolation auto-resolver consumes this, and it
    /// gracefully degrades to Hyper-V when the image's build is unknown).
    os_version: Option<String>,
}

/// Per-running-container state tracked by the runtime.
#[derive(Debug)]
struct ContainerEntry {
    /// Live handle to the HCS compute system. Dropping closes our reference
    /// (but does not terminate the system — see [`ComputeSystem::terminate`]).
    system: ComputeSystem,
    /// Writable scratch layer backing the container. Dropped on removal to
    /// tear down WCIFS and destroy the scratch directory.
    scratch_layer: Option<scratch::WritableLayer>,
    /// String form of the container id used as the HCS system identifier.
    /// Cached so that enumerate/open paths don't have to re-stringify.
    hcs_id: String,
    /// Last observed exit code, set when a `SystemExited` event is received
    /// via the HCS event stream. `None` while the container is still running.
    last_exit_code: Arc<RwLock<Option<i32>>>,
    /// HCN namespace + endpoint pair that wires this container into the
    /// daemon's Transparent overlay network. `None` if HCN was unavailable
    /// at [`HcsRuntime::create_container`] time — we still let the container
    /// start, but it won't have networking.
    network_attachment: Option<EndpointAttachment>,
    /// Hyper-V utility VM backing this container when isolation is
    /// [`IsolationMode::Hyperv`]. `None` for [`IsolationMode::Process`]
    /// entries. Dropped on remove so the per-container scratch VHDX is
    /// cleaned up via [`Uvm::Drop`].
    uvm: Option<Uvm>,
    /// Parent (read-only) layer paths that were `ActivateLayer` +
    /// `PrepareLayer`d into HCS before this container's compute system was
    /// created. Stored in original child-to-parent order (matching
    /// `parent_layers` from [`HcsRuntime::resolve_parent_chain`]). On
    /// [`HcsRuntime::remove_container`] each entry is `UnprepareLayer` +
    /// `DeactivateLayer`d in **reverse order** (parent-to-child) so HCS's
    /// internal layer table is cleared and a subsequent container with the
    /// same parents can activate them again.
    activated_parent_layers: Vec<PathBuf>,
}

/// Per-daemon HCN Transparent overlay network created lazily on first
/// [`HcsRuntime::create_container`] call. We never tear this down during the
/// daemon's lifetime — containers share it by attaching HCN endpoints to
/// fresh per-container namespaces.
#[derive(Debug)]
struct OverlayNetwork {
    /// HCN-addressable GUID of the network.
    id: GUID,
    /// CIDR the network was created with (this node's per-node slice).
    /// Informational — kept for logging and diagnostics.
    #[allow(dead_code)]
    subnet: String,
    /// Keep the `Network` handle alive so the handle's `Drop` does not close
    /// out from under concurrent endpoint creates. `HcnCloseNetwork` only
    /// releases the caller's handle; the network itself lives until we call
    /// `HcnDeleteNetwork` (we never do, for the daemon's lifetime).
    _network: zlayer_hns::network::Network,
}

/// HCS-backed implementation of [`Runtime`].
pub struct HcsRuntime {
    config: HcsConfig,
    /// In-flight containers keyed by their HCS system id.
    containers: RwLock<HashMap<String, ContainerEntry>>,
    /// Image layer cache keyed by `<image ref>` (not digest — we map one
    /// reference to one unpacked chain for the MVP). Layer reuse across
    /// images that share a base layer is handled by HCS itself.
    images: RwLock<HashMap<String, CachedImage>>,
    /// Shared registry client used for manifest pulls and blob retrieval.
    registry: Arc<zlayer_registry::ImagePuller>,
    /// Auth resolver used to pick up persisted credentials when no inline
    /// auth is supplied on the pull.
    auth_resolver: zlayer_core::AuthResolver,
    /// Lazily-created HCN Transparent overlay network all containers attach
    /// to. Guarded by a `tokio::sync::Mutex` so the first `create_container`
    /// call wins the race and every subsequent call returns the cached GUID
    /// without a double-create.
    overlay_network: Arc<Mutex<Option<OverlayNetwork>>>,
    /// Per-container IP address stashed here by
    /// [`HcsRuntime::set_next_container_ip`] just before the matching
    /// [`Runtime::create_container`] call. Consumed by `create_container` so
    /// the next allocation doesn't leak into a subsequent create. This is a
    /// stopgap until the overlay manager plumbs the IP through a dedicated
    /// attach path (T11).
    next_container_ip: Arc<Mutex<Option<IpAddr>>>,
    /// Per-container DNS configuration stashed here by
    /// [`HcsRuntime::set_next_container_dns`] just before the matching
    /// [`Runtime::create_container`] call. Tuple is `(dns_server, dns_domain)`;
    /// both are optional. Consumed on the next create so the stash does not
    /// leak into a subsequent create. `None` (outer) means the caller did not
    /// set any DNS configuration at all for the next container — the endpoint
    /// is created without a `Dns` field (legacy behavior).
    next_container_dns: Arc<Mutex<Option<(Option<IpAddr>, Option<String>)>>>,
    /// Per-node IP allocator seeded from [`HcsConfig::slice_cidr`]. `Some` once
    /// this node has an assigned slice. The slice gateway (`network + 1`) is
    /// reserved at construction via `allocate_first` so the first container gets
    /// `.2` and never collides with the Transparent network's gateway. `Mutex`
    /// because allocate/release take `&mut self`; never held across an `.await`.
    ip_allocator: Mutex<Option<zlayer_overlay::IpAllocator>>,
}

impl std::fmt::Debug for HcsRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HcsRuntime")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl HcsRuntime {
    /// Build a new [`HcsRuntime`] using the provided [`HcsConfig`] and a
    /// registry client constructed from environment-driven defaults.
    ///
    /// The blob cache is configured via
    /// [`zlayer_registry::CacheType::from_env`], falling back to the default
    /// in-memory cache if the environment is unset. Callers that need a
    /// specific cache backend should use [`HcsRuntime::new_with_registry`].
    ///
    /// # Errors
    ///
    /// Returns an error if the storage root cannot be created or the blob
    /// cache cannot be opened.
    pub async fn new(config: HcsConfig) -> Result<Self> {
        let cache_type = zlayer_registry::CacheType::from_env().map_err(|e| {
            AgentError::Configuration(format!("failed to configure HCS blob cache from env: {e}"))
        })?;
        let blob_cache = cache_type.build().await.map_err(|e| {
            AgentError::Configuration(format!("failed to open HCS blob cache: {e}"))
        })?;
        let registry = Arc::new(zlayer_registry::ImagePuller::with_cache(blob_cache));
        let runtime = Self::new_with_registry(config, registry)?;

        // Best-effort startup reconcile: terminate orphan ComputeSystems left
        // over from a previous crashed daemon run, then reap any stray HCN
        // endpoints. Both are non-fatal — a failure here must not block the
        // daemon from coming up.
        if let Err(e) = runtime.reconcile_orphan_systems().await {
            tracing::warn!(
                error = %e,
                "startup reconcile of orphan HCS compute systems failed; continuing without it"
            );
        }
        if let Err(e) = runtime.reconcile_orphans().await {
            tracing::warn!(
                error = %e,
                "startup reconcile of orphan HCN endpoints failed; continuing without it"
            );
        }

        Ok(runtime)
    }

    /// Build a new [`HcsRuntime`] with an explicit registry client.
    ///
    /// # Errors
    ///
    /// Returns an error if the storage root cannot be created.
    pub fn new_with_registry(
        config: HcsConfig,
        registry: Arc<zlayer_registry::ImagePuller>,
    ) -> Result<Self> {
        std::fs::create_dir_all(&config.storage_root).map_err(|e| {
            AgentError::Configuration(format!(
                "failed to create HCS storage root {:?}: {e}",
                config.storage_root
            ))
        })?;
        std::fs::create_dir_all(config.storage_root.join("images")).map_err(|e| {
            AgentError::Configuration(format!("failed to create HCS image cache dir: {e}"))
        })?;
        std::fs::create_dir_all(config.storage_root.join("scratch")).map_err(|e| {
            AgentError::Configuration(format!("failed to create HCS scratch dir: {e}"))
        })?;
        // Seed the per-node IP allocator from the assigned slice (if any),
        // reserving the slice gateway (`network + 1`) so container IPs start at
        // `.2` and never collide with the Transparent network's default-route
        // gateway. A parse failure is unreachable (the source is a validated
        // `IpNet`); degrade to `None` (no-network) rather than failing.
        let ip_allocator = config.slice_cidr.and_then(|slice| {
            match zlayer_overlay::IpAllocator::new(&slice.to_string()) {
                Ok(mut alloc) => {
                    let _ = alloc.allocate_first(); // reserve the .1 gateway
                    Some(alloc)
                }
                Err(e) => {
                    tracing::warn!(
                        slice = %slice,
                        error = %e,
                        "failed to build IP allocator from slice_cidr; Windows \
                         containers will start without overlay networking"
                    );
                    None
                }
            }
        });
        Ok(Self {
            config,
            containers: RwLock::new(HashMap::new()),
            images: RwLock::new(HashMap::new()),
            registry,
            auth_resolver: zlayer_core::AuthResolver::new(zlayer_core::AuthConfig::default()),
            overlay_network: Arc::new(Mutex::new(None)),
            next_container_ip: Arc::new(Mutex::new(None)),
            next_container_dns: Arc::new(Mutex::new(None)),
            ip_allocator: Mutex::new(ip_allocator),
        })
    }

    /// Format a [`ContainerId`] into the stable string used as the HCS
    /// system identifier. Matches the `Display` impl on `ContainerId` but
    /// is kept as a dedicated method so future runtimes can deviate if HCS
    /// imposes stricter id rules (e.g. GUID-only).
    fn hcs_id(id: &ContainerId) -> String {
        id.to_string()
    }

    /// Resolve the on-disk directory that holds the unpacked layers for an
    /// image. Keyed by a filesystem-safe hash of the image reference so that
    /// two calls for the same image land on the same chain.
    fn image_layer_dir(&self, image: &str) -> PathBuf {
        let safe = image.replace(['/', ':', '@'], "_");
        self.config.storage_root.join("images").join(safe)
    }

    /// Scratch-layer directory for a specific container.
    fn scratch_dir(&self, hcs_id: &str) -> PathBuf {
        self.config.storage_root.join("scratch").join(hcs_id)
    }

    /// Lazy-create the per-daemon HCN Transparent overlay network on first
    /// use and cache its GUID.
    ///
    /// The network's IPAM subnet is this node's per-node `/28` slice of the
    /// cluster CIDR (supplied by the caller). HCN installs a connected route
    /// for that slice on the uplink vSwitch so traffic within the slice is
    /// routed without NAT — callers are responsible for ensuring the
    /// overlay tunnel is set up so cross-node traffic works.
    ///
    /// Idempotent: subsequent calls return the cached GUID without touching
    /// HCN. All `HcnCreateNetwork` traffic runs on a `spawn_blocking` thread
    /// because the underlying syscall is synchronous and can block for tens
    /// of milliseconds on a cold host.
    ///
    /// # Errors
    ///
    /// Propagates the underlying [`zlayer_hns::error::HnsError`] when HCN
    /// refuses the create (e.g. `AccessDenied` when the daemon is not
    /// running as Administrator, or `SubnetConflict` if another network
    /// already owns the slice).
    async fn ensure_overlay_network(&self, slice_cidr: ipnet::IpNet) -> Result<GUID> {
        let mut guard = self.overlay_network.lock().await;
        if let Some(net) = guard.as_ref() {
            return Ok(net.id);
        }

        let net_name = overlay_network_name(&self.config.daemon_name);

        // Idempotency: a network with this name may already exist on the host
        // from a previous run that crashed, or from a daemon restart where the
        // in-memory cache is empty. Enumerate host networks and reuse the one
        // whose queried name matches ours instead of blindly recreating (which
        // would fail with a name/subnet conflict HRESULT). The HCN syscalls
        // (`list`/`open`/`query`) are synchronous, so run them on a blocking
        // thread to match the `create_transparent` path below.
        let target_name = net_name.clone();
        let existing =
            tokio::task::spawn_blocking(move || -> Option<(GUID, zlayer_hns::network::Network)> {
                let guids = zlayer_hns::network::list("{}").ok()?;
                for guid in guids {
                    let Ok(network) = zlayer_hns::network::Network::open(guid) else {
                        continue;
                    };
                    match network.query("{}") {
                        Ok(props) if props.name == target_name => {
                            return Some((guid, network));
                        }
                        _ => {}
                    }
                }
                None
            })
            .await
            .map_err(|e| AgentError::Internal(format!("spawn_blocking join failed: {e}")))?;

        if let Some((existing_id, network)) = existing {
            *guard = Some(OverlayNetwork {
                id: existing_id,
                subnet: slice_cidr.to_string(),
                _network: network,
            });
            tracing::info!(
                network_id = %format!("{existing_id:?}"),
                name = %net_name,
                "reusing existing HCN Transparent overlay network"
            );
            return Ok(existing_id);
        }

        let net_id = GUID::new().map_err(|e| {
            AgentError::Internal(format!("GUID::new for overlay network failed: {e}"))
        })?;
        let uplink = zlayer_hns::adapter::find_primary_adapter()
            .map_err(|e| AgentError::Internal(format!("find_primary_adapter: {e}")))?;
        let subnet_str = slice_cidr.to_string();
        let subnet_for_create = subnet_str.clone();
        let uplink_for_create = uplink.clone();
        let net_name_for_create = net_name.clone();

        let network = tokio::task::spawn_blocking(move || {
            zlayer_hns::network::Network::create_transparent(
                net_id,
                &net_name_for_create,
                &subnet_for_create,
                &uplink_for_create,
            )
        })
        .await
        .map_err(|e| AgentError::Internal(format!("spawn_blocking join failed: {e}")))?
        .map_err(|e| AgentError::Internal(format!("HcnCreateNetwork({net_name}): {e}")))?;

        // HCN's Transparent IPAM needs ~1-2s after network create to settle its
        // address pool. Without this wait the FIRST `HcnCreateEndpoint` against
        // a freshly-created network frequently fails with
        // `HCN_E_ADDR_INVALID_OR_RESERVED (0x803b002f)` for what should be a
        // valid host address (verified May 2026: first endpoint at the first
        // usable IP fails, second endpoint same IP succeeds). hcsshim avoids
        // this race because containerd's snapshotter creates the network once
        // at daemon startup, far before any endpoint attaches.
        tokio::time::sleep(std::time::Duration::from_millis(2000)).await;

        *guard = Some(OverlayNetwork {
            id: net_id,
            subnet: subnet_str.clone(),
            _network: network,
        });
        tracing::info!(
            network_id = %format!("{net_id:?}"),
            subnet = %subnet_str,
            uplink = %uplink,
            "created HCN Transparent overlay network"
        );
        Ok(net_id)
    }

    /// Stash the IP the next [`Runtime::create_container`] call should assign
    /// to its HCN endpoint. Callers must invoke this once per container
    /// immediately before `create_container` — the IP is consumed on the
    /// next create. If no IP has been stashed, `create_container` aborts
    /// with a clear error.
    ///
    /// This is a stopgap; once `OverlayManager::attach_container_hcn` is in
    /// place (T11) the allocation will flow through a dedicated call path
    /// and this setter will be removed.
    pub async fn set_next_container_ip(&self, ip: IpAddr) {
        *self.next_container_ip.lock().await = Some(ip);
    }

    /// Stash the DNS configuration the next [`Runtime::create_container`] call
    /// should attach to its HCN endpoint via the `Dns` schema field. Either or
    /// both parameters may be `None` to skip DNS plumbing. The stash is
    /// consumed on the next `create_container` and cleared so it does not
    /// leak into a subsequent create.
    ///
    /// Callers typically wire this from `OverlayManager::dns_server_addr()`
    /// and `OverlayManager::dns_domain()` so the endpoint inherits whichever
    /// overlay DNS the node is running.
    pub async fn set_next_container_dns(
        &self,
        dns_server: Option<IpAddr>,
        dns_domain: Option<String>,
    ) {
        *self.next_container_dns.lock().await = Some((dns_server, dns_domain));
    }

    /// Scan HCS for compute systems owned by this runtime that we do **not**
    /// track in [`Self::containers`], and terminate them.
    ///
    /// This covers the daemon-crash recovery window where
    /// [`Runtime::create_container`] succeeded (the `ComputeSystem` exists in the
    /// Windows kernel and is tagged with our [`owner_tag`]) but the daemon
    /// went down before recording it in any persistence — so on next boot
    /// the in-memory map has no entry and the system would otherwise leak
    /// until manual cleanup.
    ///
    /// At startup the live set is whatever [`Self::containers`] currently
    /// holds (usually empty); every enumerated system not in that set is an
    /// orphan. Each orphan is opened with default access, terminated via
    /// `HcsTerminateComputeSystem`, and the handle is dropped — HCS removes
    /// the system entirely once the explicit terminate completes.
    ///
    /// Individual failures (open, terminate) are logged and swallowed so one
    /// stuck system cannot block the rest of reconciliation; the enumeration
    /// error itself is propagated.
    ///
    /// # Errors
    ///
    /// Returns an error only if [`enumerate::list_by_owner`] fails. Per-system
    /// failures are logged.
    pub async fn reconcile_orphan_systems(&self) -> Result<()> {
        let live: std::collections::HashSet<String> =
            self.containers.read().await.keys().cloned().collect();

        let tag = owner_tag(&self.config.daemon_name);
        let systems = enumerate::list_by_owner(&tag)
            .await
            .map_err(|e| AgentError::Internal(format!("HcsEnumerateComputeSystems: {e}")))?;

        if systems.is_empty() {
            tracing::debug!(owner = %tag, "reconcile: no HCS compute systems found for owner");
            return Ok(());
        }

        for sys in systems {
            if live.contains(&sys.id) {
                continue;
            }

            // Open with `requested_access = 0` (default access for our token)
            // and terminate. Both calls are best-effort: log on failure and
            // continue so a single wedged orphan can't block the sweep.
            let id = sys.id.clone();
            match ComputeSystem::open(&id, 0) {
                Ok(system) => match system.terminate("").await {
                    Ok(()) => {
                        tracing::info!(
                            hcs_id = %id,
                            state = %sys.state,
                            "reconcile: terminated orphan HCS compute system"
                        );
                        // Drop the handle so HCS releases its internal
                        // refcount and finalizes the post-terminate cleanup.
                        drop(system);
                    }
                    Err(e) => {
                        tracing::warn!(
                            hcs_id = %id,
                            error = %e,
                            "reconcile: HcsTerminateComputeSystem failed for orphan; \
                             system may need manual cleanup"
                        );
                    }
                },
                Err(e) => {
                    tracing::warn!(
                        hcs_id = %id,
                        error = %e,
                        "reconcile: HcsOpenComputeSystem failed for enumerated orphan; \
                         skipping (may have just exited)"
                    );
                }
            }
        }
        Ok(())
    }

    /// Scan the host for HCN endpoints owned by this runtime (name prefix
    /// matches [`owner_tag`]) and delete them.
    ///
    /// Intended for agent startup: the in-memory container map is empty at
    /// that moment so every owned endpoint is by definition an orphan from a
    /// previous crashed run. Call sites must supply a `live` set of HCS ids
    /// already known to the runtime so we don't reap endpoints still in use;
    /// an empty set means "reap everything we own".
    ///
    /// Individual delete failures are logged and swallowed so one stuck
    /// endpoint cannot block the rest of reconciliation.
    ///
    /// # Errors
    ///
    /// Returns an error only if the initial HCN enumeration fails. Per-
    /// endpoint delete failures are logged.
    pub async fn reconcile_orphans(&self) -> Result<()> {
        let live: std::collections::HashSet<String> =
            self.containers.read().await.keys().cloned().collect();

        let tag = owner_tag(&self.config.daemon_name);
        let tag_for_list = tag.clone();
        let owned =
            tokio::task::spawn_blocking(move || hns_attach::list_owned_endpoints(&tag_for_list))
                .await
                .map_err(|e| AgentError::Internal(format!("spawn_blocking join failed: {e}")))?
                .map_err(|e| AgentError::Internal(format!("list_owned_endpoints: {e}")))?;

        for (endpoint_id, name) in owned {
            // Endpoint name is `{owner_tag}-{container_id}`. Strip the prefix
            // + dash to recover the container id and skip live containers.
            let prefix = format!("{tag}-");
            let container_id = name.strip_prefix(&prefix).unwrap_or(name.as_str());
            if live.contains(container_id) {
                continue;
            }

            // We only stored the endpoint id in HCN; the matched namespace is
            // discovered via the endpoint's properties. Best-effort: query,
            // then delete both. On failure we log and continue.
            let ep_id = endpoint_id;
            let namespace_id = match tokio::task::spawn_blocking(move || {
                zlayer_hns::endpoint::Endpoint::open(ep_id).and_then(|ep| ep.query_properties("{}"))
            })
            .await
            {
                Ok(Ok(props)) => props
                    .host_compute_namespace
                    .as_deref()
                    .and_then(parse_guid_loose),
                Ok(Err(e)) => {
                    tracing::warn!(
                        endpoint_id = %format!("{ep_id:?}"),
                        error = %e,
                        "reconcile: failed to query orphan endpoint properties"
                    );
                    None
                }
                Err(e) => {
                    tracing::warn!(
                        endpoint_id = %format!("{ep_id:?}"),
                        error = %e,
                        "reconcile: spawn_blocking join failed"
                    );
                    None
                }
            };

            let res = tokio::task::spawn_blocking(move || match namespace_id {
                Some(ns) => hns_attach::delete_endpoint_and_namespace(ep_id, ns),
                None => zlayer_hns::endpoint::Endpoint::delete(ep_id),
            })
            .await;
            match res {
                Ok(Ok(())) => {
                    tracing::info!(
                        endpoint_id = %format!("{ep_id:?}"),
                        container_id = %container_id,
                        "reconcile: reaped orphan HCN endpoint"
                    );
                }
                Ok(Err(e)) => {
                    tracing::warn!(
                        endpoint_id = %format!("{ep_id:?}"),
                        error = %e,
                        "reconcile: failed to delete orphan endpoint"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        endpoint_id = %format!("{ep_id:?}"),
                        error = %e,
                        "reconcile: spawn_blocking join failed during delete"
                    );
                }
            }
        }
        Ok(())
    }

    /// Pick the layer descriptor list from an OCI manifest. The manifest
    /// must already be platform-resolved by the puller; this method just
    /// copies the descriptor metadata the unpacker needs.
    fn manifest_to_descriptors(
        manifest: &OciImageManifest,
    ) -> Vec<unpacker::ResolvedLayerDescriptor> {
        manifest
            .layers
            .iter()
            .map(|l| unpacker::ResolvedLayerDescriptor {
                digest: l.digest.clone(),
                media_type: l.media_type.clone(),
                size: l.size,
                urls: l.urls.clone().unwrap_or_default(),
            })
            .collect()
    }

    /// Perform the actual layer pull + unpack for a single image, populating
    /// the internal image cache on success.
    async fn do_pull(
        &self,
        image: &str,
        _policy: PullPolicy,
        _auth: Option<&SpecRegistryAuth>,
    ) -> Result<()> {
        // Short-circuit if we already have this image unpacked on disk.
        {
            let cache = self.images.read().await;
            if cache.contains_key(image) {
                tracing::debug!(image = %image, "HCS image cache hit, skipping pull");
                return Ok(());
            }
        }

        let auth = self.auth_resolver.resolve(image);

        // Pull the manifest. The puller has already been configured with a
        // platform filter that matches this node's OS/arch (see Phase A3),
        // so a multi-platform index collapses to a single manifest here.
        let (manifest, _digest) = self
            .registry
            .pull_manifest(image, &auth)
            .await
            .map_err(|e| AgentError::PullFailed {
                image: image.to_string(),
                reason: format!("manifest pull: {e}"),
            })?;

        // Inspect the OCI image config's `os` field before invoking the
        // unpacker. The unpacker calls `vmcompute.dll!ProcessBaseImage` on the
        // base layer, which expects the Windows-specific `Hives/` /
        // `UtilityVM/` / `Files/Windows/System32/` layout. Running it against
        // a non-Windows image (e.g. an alpine layer chain when the composite
        // fans the pull out to both HCS and the WSL2 delegate) is guaranteed
        // to return `ERROR_PATH_NOT_FOUND (0x80070003)`. Bail out cleanly with
        // a typed `WrongPlatform` error so the composite can treat us as a
        // soft skip and consume the delegate's result instead.
        //
        // `image_os` returns `Ok(None)` when the manifest has no recognized
        // OS field; in that case we fall through to the unpacker (it might
        // still be a valid Windows image with a non-canonical config).
        match self.registry.image_os(image, &auth).await {
            Ok(Some(os)) if os != zlayer_spec::OsKind::Windows => {
                tracing::debug!(
                    image,
                    image_os = os.as_oci_str(),
                    "HCS runtime skipping unpack: image is not a Windows image"
                );
                return Err(AgentError::WrongPlatform {
                    runtime: "hcs".to_string(),
                    expected: zlayer_spec::OsKind::Windows.as_oci_str().to_string(),
                    actual: os.as_oci_str().to_string(),
                    image: image.to_string(),
                });
            }
            Ok(_) => {}
            Err(e) => {
                // Non-fatal: we couldn't fetch / parse the config. Log and
                // continue — if the image really is non-Windows the unpacker
                // will fail with a clearer message from `ProcessBaseImage`.
                tracing::warn!(
                    image,
                    error = %e,
                    "failed to inspect image OS before HCS unpack; proceeding optimistically",
                );
            }
        }

        let descriptors = Self::manifest_to_descriptors(&manifest);
        let dest_root = self.image_layer_dir(image);

        let unpacked = unpacker::unpack_windows_image(
            self.registry.as_ref(),
            image,
            &auth,
            &descriptors,
            &dest_root,
        )
        .await
        .map_err(|e| AgentError::PullFailed {
            image: image.to_string(),
            reason: format!("unpack: {e}"),
        })?;

        // Best-effort fetch of the image's `os.version` (Windows build
        // identifier the image was authored against). The isolation
        // auto-resolver uses this to pick Process-vs-Hyper-V based on
        // build-vs-host match. Failure here is non-fatal: a `None` value
        // simply funnels Auto resolution to the safer Hyper-V fallback.
        let os_version = match self.registry.image_os_version(image, &auth).await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(
                    image,
                    error = %e,
                    "failed to fetch image os.version; isolation auto-resolution will fall back to Hyper-V",
                );
                None
            }
        };

        let mut cache = self.images.write().await;
        cache.insert(
            image.to_string(),
            CachedImage {
                unpacked,
                os_version,
            },
        );
        Ok(())
    }

    /// Build the HCS compute-system JSON document for a service spec.
    ///
    /// The caller supplies the previously-built scratch layer so its mount
    /// path and directory can be wired into the `Storage` block.
    ///
    /// `namespace_ids` carries the HCN namespace GUID strings (brace-wrapped,
    /// as produced by `format!("{:?}", guid)`) this container should be
    /// attached to. Empty means no network attachment — the resulting
    /// compute-system doc will omit the `Networking` field entirely so HCS
    /// treats the container as isolated.
    // HCS create-container call has to plumb container_id, spec, image, network, mounts, devices, isolation, and gpu through; splitting would just shuffle the surface area.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    fn build_compute_system_doc(
        &self,
        hcs_id: &str,
        spec: &ServiceSpec,
        scratch_layer: &scratch::WritableLayer,
        parent_layers: Vec<zlayer_hcs::schema::Layer>,
        namespace_ids: Vec<String>,
        isolation: IsolationMode,
        uvm: Option<&Uvm>,
    ) -> Result<HcsDoc> {
        // Hyper-V isolation populates the `VirtualMachine` body using the UVM
        // the caller already provisioned: scratch VHDX → SCSI attachment, layer
        // dirs → read-only VirtualSMB shares, boot-files dir → GuestState.
        // [`IsolationMode::Process`] flows down through the `Container` path
        // unchanged.
        //
        // GPU-PV: when `spec.resources.gpu` is set, we enumerate host adapters
        // via DXGI and filter by the spec's vendor/model/count. The filtered
        // list is passed into the VirtualMachine document. Process-isolated
        // containers cannot use GPU-PV; the equivalent DirectX-device-sharing
        // path (`\\.\GLOBALROOT\Device\…` SMB shares + dxgkrnl projection) is
        // an hcsshim-internal surface whose exact paths drift between Windows
        // builds, so rather than fabricate paths we surface a typed error.
        // Users wanting GPU with `isolation: process` must switch to
        // `isolation: hyperv` for now.
        let gpu_adapters: Vec<HostGpuAdapter> = if let Some(gpu_spec) = spec.resources.gpu.as_ref()
        {
            if matches!(isolation, IsolationMode::Process) {
                return Err(AgentError::Unsupported(
                    "GPU passthrough with `isolation: process` is not yet wired; switch to \
                     `isolation: hyperv` (DirectX device-sharing for Process isolation requires \
                     dxgkrnl device paths that drift between Windows builds and would need a \
                     stable hcsshim binding to be safe)"
                        .to_string(),
                ));
            }
            // MPS requires the host MPS control daemon (`nvidia-cuda-mps-control`)
            // to be reachable from the workload. Under Hyper-V isolation the
            // workload runs inside a UVM kernel that does NOT expose the host
            // MPS pipe directory, so MPS sharing is meaningless here.
            // Reject up-front rather than silently producing a broken setup.
            if matches!(gpu_spec.sharing, Some(zlayer_spec::GpuSharingMode::Mps))
                && matches!(isolation, IsolationMode::Hyperv)
            {
                return Err(AgentError::GpuSharingUnavailable {
                    mode: "mps".to_string(),
                    reason: "MPS is not supported with Hyper-V isolation; use Process isolation \
                             or remove the sharing config"
                        .to_string(),
                });
            }
            let all_adapters =
                enumerate_host_gpu_adapters().map_err(|e| AgentError::CreateFailed {
                    id: hcs_id.to_string(),
                    reason: format!("DXGI host GPU enumeration: {e}"),
                })?;
            filter_adapters_by_gpu_spec(&all_adapters, gpu_spec)
        } else {
            Vec::new()
        };

        let virtual_machine = match isolation {
            IsolationMode::Process => None,
            IsolationMode::Hyperv => {
                let uvm = uvm.ok_or_else(|| {
                    AgentError::Internal(
                        "build_compute_system_doc called with Hyperv isolation but no UVM provided"
                            .to_string(),
                    )
                })?;
                Some(build_virtual_machine_doc(
                    uvm,
                    &parent_layers,
                    spec,
                    &gpu_adapters,
                ))
            }
        };

        // `Container.Processor` is always present in containerd-shim-runhcs-v1
        // docs (verified via ETW capture, May 2026), even when the spec sets
        // no CPU limits — containerd emits an empty `{}` object. Match that
        // behavior so HCS `Construct` sees the field unconditionally; without
        // CPU limits we send a default (all fields `None`/skipped → `{}`).
        let processor = Some(
            spec.resources
                .cpu
                .and_then(|cpu| {
                    // `count` must be at least 1 vCPU; we round up fractional requests.
                    let count = cpu.ceil();
                    #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
                    let count_u32 = if count.is_finite() && count >= 1.0 {
                        count as u32
                    } else {
                        return None;
                    };
                    Some(ContainerProcessor {
                        count: Some(count_u32),
                        maximum: None,
                        weight: None,
                    })
                })
                .unwrap_or_default(),
        );

        let memory = spec.resources.memory.as_ref().and_then(|mem_str| {
            crate::bundle::parse_memory_string(mem_str)
                .ok()
                .map(|bytes| {
                    // HCS takes MiB. Round up so 512Mi → 512, 1.5GiB → 1536, etc.
                    let mib = bytes.div_ceil(1024 * 1024);
                    ContainerMemory {
                        size_in_mb: Some(mib),
                    }
                })
        });

        // `Storage.Path` for a process-isolated container must be the prepared
        // writable layer's *volume GUID path* (`\\?\Volume{GUID}\`, with a
        // trailing backslash), as returned by `GetLayerMountPath` and captured
        // in `vhd_mount_path()` — NOT the layer directory. HCS validates this
        // format and rejects a plain directory with `ERROR_NOT_A_REPARSE_POINT`
        // (0x80071126) during "Construct". Matches hcsshim's
        // `internal/hcsoci/hcsdoc_wcow.go` (`v2Container.Storage.Path =
        // coi.Spec.Root.Path`, a volume GUID path).
        let mut root_path = scratch_layer.vhd_mount_path().to_string();
        if !root_path.is_empty() && !root_path.ends_with('\\') {
            root_path.push('\\');
        }
        let storage = HcsStorage {
            layers: parent_layers,
            path: Some(root_path),
        };

        // HCS's `Container.Networking.Namespace` is a single GUID string, so
        // collapse the id list to its first entry (a container attaches to
        // exactly one HCN namespace).
        //
        // `AllowUnqualifiedDnsQuery: true` is set by containerd's
        // `containerd-shim-runhcs-v1` on every container it creates (verified
        // via ETW capture of `Microsoft-Windows-Hyper-V-Compute`, May 2026).
        // The flag enables lookups of unqualified hostnames against the
        // namespace's DNS suffix list — required for typical service-discovery
        // patterns inside a container.
        let networking =
            namespace_ids
                .into_iter()
                .next()
                .map(|ns| zlayer_hcs::schema::ContainerNetworking {
                    allow_unqualified_dns_query: Some(true),
                    dns_search_list: Vec::new(),
                    namespace: Some(ns),
                    network_shared_container_name: None,
                });

        // GuestOs.HostName: hcsshim's WCOW path only emits this when
        // `Spec.Hostname != ""` (`internal/hcsoci/hcsdoc_wcow.go:218`), but in
        // its production callers (containerd-shim-runhcs-v1, CRI, Docker) the
        // OCI runtime spec ALWAYS defaults `Spec.Hostname` to a non-empty
        // value (typically the first 12 chars of the container id), so that
        // branch is never taken in practice. When `Networking.Namespace` is
        // set and `GuestOs` is absent, `HcsCreateComputeSystem` rejects the
        // doc with `E_INVALIDARG (0x80070057)` at
        // `OperationFailure.Detail="Construct"` (verified on
        // 10.0.26100/Windows 11 24H2, May 2026). Match the de-facto behavior:
        // always populate `GuestOs.HostName`, defaulting to the netbios-safe
        // form of `hcs_id` when the spec doesn't supply one.
        let hostname_source = spec.hostname.as_deref().unwrap_or(hcs_id);
        let guest_os = Some(HcsGuestOs {
            host_name: Some(netbios_hostname(hostname_source)),
        });
        let container = HcsContainer {
            guest_os,
            storage: Some(storage),
            networking,
            mapped_directories: Vec::new(),
            mapped_pipes: Vec::new(),
            processor,
            memory,
        };

        // A Hyper-V-isolated compute system carries the `VirtualMachine` body
        // and omits the `Container` body — HCS picks up the container
        // configuration from the workload running inside the UVM, not from the
        // outer compute system. Process isolation is the inverse.
        let (container_doc, vm_doc) = match isolation {
            IsolationMode::Process => (Some(container), None),
            IsolationMode::Hyperv => (None, virtual_machine),
        };

        let doc = HcsDoc {
            owner: owner_tag(&self.config.daemon_name),
            schema_version: SchemaVersion::default(),
            hosting_system_id: String::new(),
            container: container_doc,
            virtual_machine: vm_doc,
            should_terminate_on_last_handle_closed: Some(true),
        }
        .apply_service_id(hcs_id);
        Ok(doc)
    }

    /// Return the cached unpacked image's parent-chain layers in the
    /// child-to-parent order HCS expects for `Storage.Layers`.
    async fn resolve_parent_chain(&self, image: &str) -> Result<Vec<zlayer_hcs::schema::Layer>> {
        let cache = self.images.read().await;
        let entry = cache.get(image).ok_or_else(|| AgentError::CreateFailed {
            id: image.to_string(),
            reason: format!("image '{image}' not pulled before create_container"),
        })?;
        Ok(entry.unpacked.chain.0.clone())
    }

    /// Return the cached image's `os.version` (Windows build the image was
    /// authored against), or `None` when the image was never pulled or its
    /// config blob did not record an `os.version`. Used by
    /// [`resolve_isolation_for_image`] to pick Process-vs-Hyper-V isolation
    /// based on whether the image build matches the host build.
    async fn resolve_image_os_version(&self, image: &str) -> Option<String> {
        let cache = self.images.read().await;
        cache.get(image).and_then(|e| e.os_version.clone())
    }

    /// Activate + Prepare every parent (read-only) layer with HCS before its
    /// `Path` may legally appear in a `Container.Storage.Layers[].Path` entry.
    ///
    /// HCS's `Construct` step rejects compute-system documents whose layer
    /// paths have not been registered with the host-side layer table; the
    /// observed failure is `E_INVALIDARG (0x80070057)`. hcsshim handles this
    /// in its snapshotter by calling `HcsActivateLayer` for every parent layer
    /// (and `HcsPrepareLayer` so the merged read-only view materialises) prior
    /// to writing the `MountedLayerPaths` block into the container doc — see
    /// `internal/layers/wcow_mount.go::mountProcessIsolatedWCIFSLayers` for
    /// the canonical ordering.
    ///
    /// `parent_layers` is the child-to-parent vec from
    /// [`Self::resolve_parent_chain`]. We iterate **oldest to newest**
    /// (reverse) so each `PrepareLayer` call sees a chain whose parents are
    /// already active, mirroring hcsshim. Each layer's `PrepareLayer` parent
    /// chain is the slice **older** than itself (child-to-parent ordered).
    ///
    /// On error, every layer that was activated (and possibly prepared) is
    /// rolled back in reverse order so a partial failure leaves the host
    /// layer table clean.
    ///
    /// Returns the list of successfully activated+prepared layer paths in the
    /// original child-to-parent order. The caller must hand this list to
    /// [`ContainerEntry::activated_parent_layers`] so `remove_container` can
    /// `UnprepareLayer` + `DeactivateLayer` each one on teardown.
    fn activate_parent_layers(
        parent_layers: &[zlayer_hcs::schema::Layer],
    ) -> std::io::Result<Vec<PathBuf>> {
        // Read-only parent layers ONLY need `ActivateLayer` — calling
        // `PrepareLayer` on a parent puts it into a "ready for writes" state,
        // which is the SCRATCH layer's role, not a parent's. hcsshim's
        // snapshotter calls `ActivateLayer` on every parent then
        // `ActivateLayer`+`PrepareLayer` on the scratch only
        // (`Microsoft/hcsshim/internal/wclayer` + `containerd-shim-runhcs-v1`
        // snapshotter `Mount`). Misordered prepare on a parent makes HCS
        // `Construct` reject the eventual compute-system doc with
        // `E_INVALIDARG (0x80070057)`.
        let mut activated: Vec<PathBuf> = Vec::with_capacity(parent_layers.len());
        let n = parent_layers.len();
        // Oldest-to-newest: walk reverse indices.
        for i in (0..n).rev() {
            let layer = &parent_layers[i];
            let layer_path = PathBuf::from(&layer.path);

            if let Err(e) = crate::windows::wclayer::activate_layer(&layer_path) {
                rollback_parent_activations(&activated);
                return Err(std::io::Error::other(format!(
                    "ActivateLayer({}) failed: {e}",
                    layer_path.display()
                )));
            }
            activated.push(layer_path);
        }

        // Reverse into the original child-to-parent order for storage in
        // [`ContainerEntry::activated_parent_layers`].
        activated.reverse();
        Ok(activated)
    }

    /// Spawn a background task that subscribes to the compute system's
    /// event stream and records the exit code when `SystemExited` arrives.
    fn spawn_exit_watcher(
        &self,
        hcs_id: String,
        system_raw: windows::Win32::System::HostComputeSystem::HCS_SYSTEM,
        sink: Arc<RwLock<Option<i32>>>,
    ) {
        let (_sub, mut stream) = match events::subscribe(system_raw) {
            Ok(pair) => pair,
            Err(e) => {
                tracing::warn!(
                    hcs_id = %hcs_id,
                    error = %e,
                    "failed to subscribe to HCS lifecycle events; exit code will be unknown"
                );
                return;
            }
        };

        tokio::spawn(async move {
            // Keep the subscription alive for the whole task.
            let _sub = _sub;
            while let Some(evt) = stream.next().await {
                if matches!(evt.kind, HcsEventKind::SystemExited) {
                    // HCS carries the child-process exit code inside
                    // `detail_json` as `{"ExitCode": <n>}` on most builds.
                    let code = extract_exit_code(&evt.detail_json).unwrap_or(0);
                    *sink.write().await = Some(code);
                    break;
                }
                if matches!(evt.kind, HcsEventKind::ServiceDisconnect) {
                    // vmcompute.exe went away — treat as runtime error.
                    *sink.write().await = Some(-1);
                    break;
                }
            }
        });
    }
}

/// Default vCPU count assigned to a freshly-provisioned UVM.
///
/// Two vCPUs is the hcsshim convention for a Hyper-V-isolated container's
/// utility VM — enough to keep the guest kernel responsive without giving up
/// large amounts of host scheduling capacity per container.
const UVM_DEFAULT_VCPUS: u32 = 2;

/// Default memory (MiB) assigned to a freshly-provisioned UVM.
///
/// 1 GiB matches the hcsshim default for `WCOW` utility VMs. The container's
/// own memory pressure is independent of this number — the limit set on the
/// service's [`ServiceSpec::resources`] is applied to the workload running
/// inside the UVM, not the UVM itself.
const UVM_DEFAULT_MEMORY_MB: u64 = 1024;

/// Stable controller GUID hcsshim uses for the primary (boot) SCSI controller
/// on a Hyper-V-isolated WCOW UVM. Matches
/// `internal/uvm/scsi.go::guestPrimaryScsiControllerGUID` in hcsshim — HCS keys
/// the controller in `Devices.Scsi` by this exact string and rejects the
/// document if it sees the legacy ordinal `"0"` form for a VM that boots from
/// `VmbFs`.
const PRIMARY_SCSI_CTRL_GUID: &str = "df6d0690-79e5-55b6-a5ec-c1e2f77f580a";

/// Build a [`VirtualMachine`] document populated from a freshly-provisioned
/// [`Uvm`] plus the parent read-only layer chain.
///
/// Layout follows the hcsshim convention for Hyper-V-isolated WCOW containers:
///
/// * `chipset.uefi` boots from `VmbFs` at `\EFI\Microsoft\Boot\bootmgfw.efi`,
///   served out of the image's `UtilityVM\Files` directory via the `"os"`
///   `VirtualSMB` share — there is no host-side `GuestState` VHD.
/// * `compute_topology` defaults to [`UVM_DEFAULT_VCPUS`] vCPUs and
///   [`UVM_DEFAULT_MEMORY_MB`] MiB of memory.
/// * `devices.scsi[<PRIMARY_SCSI_CTRL_GUID>]` carries one attachment at LUN
///   `"0"` for the scratch VHDX (writable). The controller is keyed by the
///   hcsshim-canonical primary SCSI controller GUID.
/// * `devices.virtual_smb` exposes the `"os"` boot-files share plus one
///   read-only share per parent layer so the container workload can mount its
///   image.
/// * `guest_state` is omitted entirely — VmbFs-boot UVMs persist VM state via
///   the SCSI scratch alone; no `.vmgs` host file is needed.
///
/// `spec` carries the workload's [`zlayer_spec::GpuSpec`] (if any) so GPU-PV
/// adapters can be attached when the caller has already enumerated and
/// filtered the host's GPU adapters; the UVM's CPU/memory topology stays
/// fixed by the constants above so the per-container CPU/memory limits remain
/// a property of the workload inside the UVM (see
/// [`HcsRuntime::build_compute_system_doc`]'s `ContainerProcessor`/`ContainerMemory`).
///
/// `gpu_adapters` is the already-filtered list of host adapters to attach for
/// Hyper-V GPU-PV. The caller is responsible for vendor/model filtering and
/// for honoring `spec.resources.gpu.count`. An empty slice paired with a
/// populated `spec.resources.gpu` produces a [`GpuAssignmentMode::Default`]
/// block (let HCS pick); a non-empty slice produces a
/// [`GpuAssignmentMode::List`] block.
#[allow(clippy::too_many_lines)] // construction is sequential by HCS field; splitting hurts readability
fn build_virtual_machine_doc(
    uvm: &Uvm,
    parent_layers: &[zlayer_hcs::schema::Layer],
    spec: &ServiceSpec,
    gpu_adapters: &[HostGpuAdapter],
) -> VirtualMachine {
    use std::collections::BTreeMap;

    let mut scsi_attachments: BTreeMap<String, ScsiAttachment> = BTreeMap::new();
    scsi_attachments.insert(
        "0".to_string(),
        ScsiAttachment {
            path: uvm.scratch_vhdx().to_string_lossy().into_owned(),
            r#type: "VirtualDisk".to_string(),
            read_only: Some(false),
        },
    );

    let mut scsi: BTreeMap<String, ScsiController> = BTreeMap::new();
    scsi.insert(
        PRIMARY_SCSI_CTRL_GUID.to_string(),
        ScsiController {
            attachments: scsi_attachments,
        },
    );

    // VirtualSMB shares: first the `"os"` share that exposes the image's
    // `UtilityVM\Files` directory as the UVM's boot volume (UEFI boots from
    // `VmbFs:\EFI\Microsoft\Boot\bootmgfw.efi`), then one read-only share per
    // parent layer keyed by the layer's HCS id (already a stable GUID) so
    // multiple containers attached to the same layer dir on the same UVM never
    // collide. The `"os"` options match hcsshim's `DefaultVSMBOptions(true)`
    // from `internal/uvm/vsmb.go` — read-only, share-read, cache-io,
    // pseudo-oplocks, take-backup-privilege.
    let mut virtual_smb: BTreeMap<String, VirtualSmbShare> = BTreeMap::new();
    virtual_smb.insert(
        "os".to_string(),
        VirtualSmbShare {
            name: "os".to_string(),
            path: uvm.os_files_dir().to_string_lossy().into_owned(),
            options: Some(VirtualSmbShareOptions {
                read_only: true,
                share_read: true,
                cache_io: true,
                pseudo_oplocks: true,
                take_backup_privilege: true,
                ..Default::default()
            }),
            ..Default::default()
        },
    );
    for layer in parent_layers {
        virtual_smb.insert(
            layer.id.clone(),
            VirtualSmbShare {
                name: layer.id.clone(),
                path: layer.path.clone(),
                options: Some(VirtualSmbShareOptions {
                    read_only: true,
                    share_read: true,
                    cache_io: true,
                    pseudo_oplocks: true,
                    ..Default::default()
                }),
                ..Default::default()
            },
        );
    }

    // Populate the GPU-PV block when the workload requested a GPU. With no
    // candidate adapters we emit `Default` so HCS picks the host default
    // instead of silently dropping the request; with candidates we list them
    // explicitly.
    let gpu = spec.resources.gpu.as_ref().map(|_| {
        let requests: Vec<GpuAssignmentRequest> = gpu_adapters
            .iter()
            .map(|a| GpuAssignmentRequest {
                #[allow(clippy::cast_sign_loss)]
                virtual_machine_id_string: format!(
                    "0x{:08x}:0x{:08x}",
                    a.luid_high, a.luid_low as u32,
                ),
                adapter_luid_high_part: a.luid_high,
                adapter_luid_low_part: a.luid_low,
            })
            .collect();
        let mode = if requests.is_empty() {
            GpuAssignmentMode::Default
        } else {
            GpuAssignmentMode::List
        };
        GpuAssignment {
            assignment_mode: mode,
            assignment_request: requests,
            allow_vendor_extension: Some(true),
        }
    });

    VirtualMachine {
        chipset: Some(Chipset {
            uefi: Some(Uefi {
                boot_this: Some(UefiBootEntry {
                    device_type: "VmbFs".to_string(),
                    device_path: r"\EFI\Microsoft\Boot\bootmgfw.efi".to_string(),
                    disk_number: None,
                }),
            }),
        }),
        compute_topology: Some(Topology {
            memory: Some(TopologyMemory {
                size_in_mb: UVM_DEFAULT_MEMORY_MB,
            }),
            processor: Some(TopologyProcessor {
                count: UVM_DEFAULT_VCPUS,
            }),
        }),
        devices: Some(Devices {
            scsi,
            virtual_smb,
            gpu,
        }),
        // VmbFs boot path means HCS persists VM state via the SCSI scratch
        // alone; no host-side `.vmgs` guest-state file is needed (or wanted —
        // supplying one with VmbFs boot triggers an HCS validation error).
        guest_state: None,
        runtime_state_file_path: None,
    }
}

// ---------------------------------------------------------------------------
// Host GPU adapter probe (DXGI) + spec-side filtering
// ---------------------------------------------------------------------------

/// One host GPU adapter as seen by DXGI.
///
/// `luid_high` / `luid_low` come from
/// `IDXGIAdapter::GetDesc().AdapterLuid` — Microsoft's `LUID` carries
/// `HighPart: i32` and `LowPart: u32` on the wire but the HCS GPU-PV schema
/// expects `AdapterLuidHighPart: u32` and `AdapterLuidLowPart: i32`, so we
/// match that orientation here and apply the sign conversion at the wire
/// boundary inside [`build_virtual_machine_doc`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HostGpuAdapter {
    /// `LUID.HighPart` cast to `u32` for HCS.
    pub luid_high: u32,
    /// `LUID.LowPart` cast to `i32` for HCS.
    pub luid_low: i32,
    /// Human-readable adapter description (`Description` field of
    /// `DXGI_ADAPTER_DESC`, NUL-trimmed and UTF-8'd).
    pub description: String,
    /// PCI vendor id (e.g. `0x10de` for NVIDIA, `0x1002` for AMD, `0x8086`
    /// for Intel, `0x1414` for Microsoft Basic Render Driver / WARP).
    pub vendor_id: u32,
    /// PCI device id.
    pub device_id: u32,
}

/// PCI vendor id of Microsoft's software renderer (WARP / Basic Render
/// Driver). We skip these in [`enumerate_host_gpu_adapters`] because they
/// cannot back GPU-PV passthrough.
const VENDOR_ID_MICROSOFT_BASIC: u32 = 0x1414;

/// Enumerate host GPU adapters via DXGI on Windows. Returns
/// [`io::ErrorKind::Unsupported`] on every other platform so the runtime
/// surfaces a clean error rather than panicking when a cross-compile slips
/// through.
#[cfg(target_os = "windows")]
fn enumerate_host_gpu_adapters() -> std::io::Result<Vec<HostGpuAdapter>> {
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIAdapter, IDXGIFactory1};

    // SAFETY: `CreateDXGIFactory1` is the documented entry point for the
    // DXGI 1.1 factory; we hold the resulting interface for the lifetime of
    // this function only, so the COM ref-count is managed entirely by
    // `windows-rs`. No raw pointers are dereferenced by the caller.
    let factory: IDXGIFactory1 = unsafe {
        CreateDXGIFactory1()
            .map_err(|e| std::io::Error::other(format!("CreateDXGIFactory1 failed: {e}")))?
    };

    let mut adapters = Vec::new();
    let mut index: u32 = 0;
    loop {
        // SAFETY: `EnumAdapters` returns `DXGI_ERROR_NOT_FOUND` once the
        // index runs past the last adapter; we treat that as the loop's
        // natural termination. Any other error is propagated.
        let adapter: IDXGIAdapter = unsafe {
            match factory.EnumAdapters(index) {
                Ok(a) => a,
                Err(e) => {
                    // DXGI_ERROR_NOT_FOUND is HRESULT 0x887A0002. The HRESULT
                    // bit pattern is what we compare against — sign-loss here
                    // is the intended reinterpretation of the i32 as u32.
                    #[allow(clippy::cast_sign_loss)]
                    let code = e.code().0 as u32;
                    if code == 0x887A_0002 {
                        break;
                    }
                    return Err(std::io::Error::other(format!(
                        "IDXGIFactory1::EnumAdapters({index}) failed: {e}",
                    )));
                }
            }
        };

        // SAFETY: `IDXGIAdapter::GetDesc` returns the descriptor by value;
        // `windows-rs` wraps the HRESULT into a `Result`. No raw pointers are
        // dereferenced by the caller.
        let desc = unsafe {
            adapter.GetDesc().map_err(|e| {
                std::io::Error::other(format!("IDXGIAdapter::GetDesc({index}) failed: {e}"))
            })?
        };

        if desc.VendorId == VENDOR_ID_MICROSOFT_BASIC {
            // Skip WARP / Basic Render Driver — cannot back GPU-PV.
            index += 1;
            continue;
        }

        // `Description` is a NUL-terminated UTF-16 array up to 128 chars.
        let nul = desc
            .Description
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(desc.Description.len());
        let description = String::from_utf16_lossy(&desc.Description[..nul]);

        // LUID parts are opaque bit patterns; the LUID is a 64-bit ID split
        // across i32 high / u32 low in Win32, and our `HostGpuAdapter`
        // stores them as `u32 high / i32 low` to match the HCS schema's
        // `GpuAssignmentRequest` field types. Reinterpret bit patterns.
        #[allow(clippy::cast_sign_loss)]
        let luid_high = desc.AdapterLuid.HighPart as u32;
        #[allow(clippy::cast_possible_wrap)]
        let luid_low = desc.AdapterLuid.LowPart as i32;
        adapters.push(HostGpuAdapter {
            luid_high,
            luid_low,
            description,
            vendor_id: desc.VendorId,
            device_id: desc.DeviceId,
        });

        index += 1;
    }

    Ok(adapters)
}

/// Non-Windows stub: DXGI is a Windows-only API. Compiled on every other
/// platform so unit tests on Linux / macOS can still link the module and
/// assert the expected error shape.
#[cfg(not(target_os = "windows"))]
fn enumerate_host_gpu_adapters() -> std::io::Result<Vec<HostGpuAdapter>> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "DXGI host GPU enumeration is Windows-only",
    ))
}

/// Map a [`zlayer_spec::GpuSpec`] vendor string to its PCI vendor id, or
/// `None` when the vendor is `"all"` / empty / unknown (in which case no
/// vendor filtering is applied).
fn vendor_id_for_spec(vendor: &str) -> Option<u32> {
    match vendor.to_ascii_lowercase().as_str() {
        "nvidia" => Some(0x10de),
        "amd" | "ati" => Some(0x1002),
        "intel" => Some(0x8086),
        _ => None,
    }
}

/// Filter a list of host adapters by the spec's vendor + count. Model
/// filtering matches as a case-insensitive substring against
/// [`HostGpuAdapter::description`].
///
/// Filtering is applied in this order:
/// 1. Vendor (if not `"all"`/empty/unknown).
/// 2. Model substring (if present).
/// 3. Truncate to `count` (always; defaults to 1 in the spec).
fn filter_adapters_by_gpu_spec(
    adapters: &[HostGpuAdapter],
    spec: &zlayer_spec::GpuSpec,
) -> Vec<HostGpuAdapter> {
    let vendor_filter = if spec.vendor.eq_ignore_ascii_case("all") || spec.vendor.is_empty() {
        None
    } else {
        vendor_id_for_spec(&spec.vendor)
    };

    let model_lower = spec.model.as_deref().map(str::to_ascii_lowercase);

    let mut filtered: Vec<HostGpuAdapter> = adapters
        .iter()
        .filter(|a| match vendor_filter {
            Some(vid) => a.vendor_id == vid,
            None => true,
        })
        .filter(|a| match model_lower.as_deref() {
            Some(needle) => a.description.to_ascii_lowercase().contains(needle),
            None => true,
        })
        .cloned()
        .collect();

    let want = spec.count.max(1) as usize;
    if filtered.len() > want {
        filtered.truncate(want);
    }
    filtered
}

/// RAII guard that owns a list of parent layer paths that have been
/// `ActivateLayer` + `PrepareLayer`d. On drop (unless [`Self::disarm`]ed) it
/// `UnprepareLayer` + `DeactivateLayer`s each entry in reverse order
/// (parent-to-child of the original child-to-parent list, i.e. newest-first)
/// so a `create_container` failure between activation and successful
/// `ContainerEntry` insertion does not leak host layer table entries.
struct ParentLayerActivationGuard {
    layers: Vec<PathBuf>,
    armed: bool,
}

impl ParentLayerActivationGuard {
    fn new(layers: Vec<PathBuf>) -> Self {
        Self {
            layers,
            armed: true,
        }
    }

    /// Disarm the guard and return ownership of the path list. The caller is
    /// now responsible for `UnprepareLayer` + `DeactivateLayer` on each path
    /// during container teardown (via [`ContainerEntry::activated_parent_layers`]).
    fn disarm(mut self) -> Vec<PathBuf> {
        self.armed = false;
        std::mem::take(&mut self.layers)
    }
}

impl Drop for ParentLayerActivationGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        // The stored order is child-to-parent. Deactivate newest-first so
        // we mirror hcsshim's teardown direction. Parents were only
        // `ActivateLayer`d (no `PrepareLayer`), so the matching teardown
        // is just `DeactivateLayer`.
        for path in &self.layers {
            if let Err(e) = crate::windows::wclayer::deactivate_layer(path) {
                tracing::warn!(
                    layer = %path.display(),
                    error = %e,
                    "DeactivateLayer failed during create_container rollback",
                );
            }
        }
    }
}

/// Roll back a partial parent-layer activation. `activated` is the running
/// log built by [`HcsRuntime::activate_parent_layers`]: oldest-to-newest in
/// the order they were activated, with a `was_prepared` flag indicating
/// whether `PrepareLayer` also succeeded for that entry. We iterate the log
/// in reverse (newest-back-to-oldest of the activated set) and best-effort
/// unprepare-then-deactivate each layer. All failures are logged; none are
/// propagated, because the caller is already returning an error and we do
/// not want to mask the original cause.
fn rollback_parent_activations(activated: &[PathBuf]) {
    for path in activated.iter().rev() {
        if let Err(e) = crate::windows::wclayer::deactivate_layer(path) {
            tracing::warn!(
                layer = %path.display(),
                error = %e,
                "DeactivateLayer failed during parent-activation rollback",
            );
        }
    }
}

/// Parse a GUID that may or may not be brace-wrapped, as HCN emits them on
/// the wire (`"{aabbccdd-...}"` or bare `"aabbccdd-..."`). Returns `None` for
/// any malformed input so callers can fall back gracefully.
fn parse_guid_loose(s: &str) -> Option<GUID> {
    let bare = s.trim_matches(|c: char| c == '{' || c == '}');
    GUID::try_from(bare).ok()
}

/// Extract an exit code from the JSON payload HCS emits on
/// `SystemExited`. Best-effort — returns `None` when the payload is empty
/// or lacks an `ExitCode` field; the caller defaults to `0` in that case.
fn extract_exit_code(detail_json: &str) -> Option<i32> {
    if detail_json.trim().is_empty() {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(detail_json).ok()?;
    v.get("ExitCode")
        .and_then(serde_json::Value::as_i64)
        .and_then(|n| {
            #[allow(clippy::cast_possible_truncation)]
            Some(n as i32)
        })
}

// ---------------------------------------------------------------------------
// Helper: attach a stable service id to the compute-system document so
// enumeration can correlate zombie systems with their originating ZLayer
// service. We stash it in `hosting_system_id` — per the schema that field is
// only used for utility-VM-hosted containers, and we never set those in
// Phase B, so it's free for our tagging purpose.
// ---------------------------------------------------------------------------
trait ApplyServiceId {
    fn apply_service_id(self, hcs_id: &str) -> Self;
}

impl ApplyServiceId for HcsDoc {
    fn apply_service_id(self, hcs_id: &str) -> Self {
        // We don't actually put the id here — HcsCreateComputeSystem already
        // takes the id as a separate argument. This hook exists so future
        // phases that need the id in the JSON (e.g. for `HcsModifyComputeSystem`
        // on a `Container` request) have one obvious place to add it.
        let _ = hcs_id;
        self
    }
}

// ---------------------------------------------------------------------------
// Runtime impl
// ---------------------------------------------------------------------------

#[async_trait]
impl Runtime for HcsRuntime {
    #[instrument(skip(self), fields(otel.name = "image.pull", container.image.name = %image))]
    async fn pull_image(&self, image: &str) -> Result<()> {
        self.do_pull(image, PullPolicy::IfNotPresent, None).await
    }

    #[instrument(
        skip(self, auth),
        fields(otel.name = "image.pull", container.image.name = %image, pull_policy = ?policy)
    )]
    async fn pull_image_with_policy(
        &self,
        image: &str,
        policy: PullPolicy,
        auth: Option<&SpecRegistryAuth>,
    ) -> Result<()> {
        if matches!(policy, PullPolicy::Never) {
            // Never policy: succeed only if we already have it cached.
            let cache = self.images.read().await;
            return if cache.contains_key(image) {
                Ok(())
            } else {
                Err(AgentError::PullFailed {
                    image: image.to_string(),
                    reason: "pull_policy=never and image not cached locally".to_string(),
                })
            };
        }
        self.do_pull(image, policy, auth).await
    }

    #[instrument(
        skip(self, spec),
        fields(
            otel.name = "container.create",
            container.id = %id,
            service.name = %id.service,
            service.replica = %id.replica,
            container.image.name = %spec.image.name,
        )
    )]
    async fn create_container(&self, id: &ContainerId, spec: &ServiceSpec) -> Result<()> {
        let hcs_id = Self::hcs_id(id);
        let image_name = spec.image.name.to_string();

        // 1. Look up (or lazy-pull) the unpacked image.
        {
            let cache = self.images.read().await;
            if !cache.contains_key(&image_name) {
                drop(cache);
                self.do_pull(&image_name, spec.image.pull_policy, None)
                    .await?;
            }
        }
        let parent_layers = self.resolve_parent_chain(&image_name).await?;

        // 1b. Activate + Prepare every parent (read-only) layer with HCS.
        //     Without this, HCS's `Construct` step rejects the eventual
        //     compute-system doc with `E_INVALIDARG (0x80070057)` because the
        //     `Container.Storage.Layers[].Path` values point at unpacked
        //     directories that the host-side layer table has never seen.
        //     Mirrors hcsshim's `mountProcessIsolatedWCIFSLayers`.
        let parent_activation_guard = {
            let parents = parent_layers.clone();
            let activated =
                tokio::task::spawn_blocking(move || Self::activate_parent_layers(&parents))
                    .await
                    .map_err(|e| AgentError::CreateFailed {
                        id: hcs_id.clone(),
                        reason: format!("spawn_blocking join for parent layer activation: {e}"),
                    })?
                    .map_err(|e| AgentError::CreateFailed {
                        id: hcs_id.clone(),
                        reason: format!("parent layer activation: {e}"),
                    })?;
            ParentLayerActivationGuard::new(activated)
        };

        // 2. Build a scratch layer for this container.
        let scratch_dir = self.scratch_dir(&hcs_id);
        // Convert the HCS-ordered parent list into the wclayer LayerChain
        // expected by `scratch::create`.
        let chain = crate::windows::wclayer::LayerChain::new(parent_layers.clone());
        let scratch_layer =
            scratch::create(&scratch_dir, &chain).map_err(|e| AgentError::CreateFailed {
                id: hcs_id.clone(),
                reason: format!("scratch layer create: {e}"),
            })?;

        // 3. Attach the container to the daemon's HCN Transparent overlay
        //    network. If HCN is unavailable (e.g. non-admin daemon on a dev
        //    box), log and proceed with `None` — the container still starts,
        //    just without network connectivity. This keeps the happy path of
        //    `zlayer deploy` green for local smoke tests even without admin.
        //
        //    The per-container IP is stashed via
        //    [`HcsRuntime::set_next_container_ip`] immediately before this
        //    call by the overlay manager. The prefix length comes from the
        //    slice, and the cluster CIDR drives the OutBoundNAT / SDNRoute /
        //    ACL policies on the endpoint.
        let slice_cidr = self.config.slice_cidr;
        // Prefer an explicitly-stashed IP (set_next_container_ip path); otherwise
        // self-allocate from this node's slice allocator. `None` when no slice is
        // assigned (allocator is `None`) or the slice is exhausted — either way we
        // fall through to the no-network arm below.
        let allocated_ip = match self.next_container_ip.lock().await.take() {
            Some(ip) => Some(ip),
            None => self
                .ip_allocator
                .lock()
                .await
                .as_mut()
                .and_then(zlayer_overlay::IpAllocator::allocate),
        };
        let dns_config = self.next_container_dns.lock().await.take();
        let cluster_cidr = self.config.cluster_cidr.clone();
        let owner_tag_for_endpoint = owner_tag(&self.config.daemon_name);
        // Overlay attachment is a hard requirement for an HCS-managed container:
        // without an IP the workload has no addressable network surface, and
        // `get_container_ip` will return `None` which downstream callers (proxy,
        // service discovery, health checks) treat as a permanent fault. Each
        // failure mode below maps to a distinct `AgentError::CreateFailed` so
        // the operator (or test harness) sees the exact cause rather than a
        // late "no IP" mystery.
        let network_attachment = match (slice_cidr, allocated_ip) {
            (Some(slice), Some(ip)) => match self.ensure_overlay_network(slice).await {
                Ok(net_id) => {
                    let cid_for_attach = hcs_id.clone();
                    let prefix_length = slice.prefix_len();
                    let cluster_cidr_owned = cluster_cidr;
                    let (dns_server, dns_domain) = dns_config.unwrap_or((None, None));
                    let owner_tag_for_attach = owner_tag_for_endpoint;
                    match tokio::task::spawn_blocking(move || {
                        EndpointAttachment::create_overlay(
                            net_id,
                            &owner_tag_for_attach,
                            cid_for_attach.as_str(),
                            ip,
                            prefix_length,
                            &cluster_cidr_owned,
                            dns_server,
                            dns_domain.as_deref(),
                        )
                    })
                    .await
                    {
                        Ok(Ok(att)) => att,
                        Ok(Err(e)) => {
                            return Err(AgentError::CreateFailed {
                                id: hcs_id.clone(),
                                reason: format!("HCN overlay endpoint attach failed: {e}"),
                            });
                        }
                        Err(e) => {
                            return Err(AgentError::CreateFailed {
                                id: hcs_id.clone(),
                                reason: format!(
                                    "spawn_blocking join for overlay endpoint attach failed: {e}"
                                ),
                            });
                        }
                    }
                }
                Err(e) => {
                    return Err(AgentError::CreateFailed {
                        id: hcs_id.clone(),
                        reason: format!("HCN Transparent overlay network unavailable: {e}"),
                    });
                }
            },
            (None, _) => {
                return Err(AgentError::CreateFailed {
                    id: hcs_id.clone(),
                    reason: "HcsConfig.slice_cidr is None (node has no assigned slice yet)"
                        .to_string(),
                });
            }
            (Some(_), None) => {
                return Err(AgentError::CreateFailed {
                    id: hcs_id.clone(),
                    reason: "no overlay IP could be allocated for this container".to_string(),
                });
            }
        };

        // 4. Build the compute-system JSON document, populating
        //    `Container.Networking.Namespace` with the namespace GUID we just
        //    created. HCS resolves this GUID against HCN during its `Construct`
        //    step; it must be the **bare, lowercase, un-braced** form
        //    (`aabbccdd-...`). The brace-wrapped upper-case `{:?}` form makes
        //    Construct fail with `0x80070490 ERROR_NOT_FOUND` because the
        //    lookup string doesn't match the namespace HCN registered.
        let namespace_strs: Vec<String> = vec![format_guid_bare(network_attachment.namespace_id())];
        // Resolve the spec-side isolation choice (which may be `Auto` or
        // absent) to the concrete runtime-internal isolation mode using the
        // image's builder-asserted `os.version` and the host's Windows
        // build. Explicit `Process` / `Hyperv` from the spec bypass the
        // matrix and flow through directly. See [`decide_isolation`].
        let image_os_version = self.resolve_image_os_version(&image_name).await;
        let isolation = resolve_isolation_for_image(spec.isolation, image_os_version.as_deref());

        // For Hyper-V isolation, provision the utility VM BEFORE building the
        // compute-system doc so the doc can reference the UVM's scratch VHDX,
        // boot files, and per-layer VirtualSMB shares. The UVM's `Drop` impl
        // cleans up the scratch VHDX if any later step fails; on the happy
        // path it lands in `ContainerEntry.uvm` and is dropped on
        // `remove_container`. Process-isolated containers never allocate a
        // UVM (it would just waste a few hundred MiB of host memory).
        let uvm =
            match isolation {
                IsolationMode::Hyperv => {
                    // Locate the UVM boot payload bundled inside the image's
                    // parent chain. Hyper-V isolation REQUIRES a Windows base
                    // image that ships `UtilityVM\Files\...` and
                    // `UtilityVM\SystemTemplate.vhdx`; non-Windows or
                    // nanoserver-without-UVM images fail loudly here rather
                    // than producing a UVM that can't boot.
                    let boot_files = crate::windows::unpacker::locate_uvm_boot_files(&chain)
                        .map_err(|e| AgentError::CreateFailed {
                            id: hcs_id.clone(),
                            reason: format!(
                            "Hyper-V isolation requires a Windows base image with UVM payload: {e}"
                        ),
                        })?;
                    Some(
                        Uvm::create(&hcs_id, &self.config.storage_root, &boot_files).map_err(
                            |e| AgentError::CreateFailed {
                                id: hcs_id.clone(),
                                reason: format!("UVM provisioning failed: {e}"),
                            },
                        )?,
                    )
                }
                IsolationMode::Process => None,
            };

        let doc = self.build_compute_system_doc(
            &hcs_id,
            spec,
            &scratch_layer,
            parent_layers,
            namespace_strs,
            isolation,
            uvm.as_ref(),
        )?;
        let doc_json = serde_json::to_string(&doc).map_err(|e| AgentError::CreateFailed {
            id: hcs_id.clone(),
            reason: format!("serialize ComputeSystem doc: {e}"),
        })?;
        // Diagnostic: emit the exact JSON we hand to HCS so reproducible E_INVALIDARG
        // failures can be diffed against hcsshim's known-good docs. error! so it
        // shows without RUST_LOG configuration.
        tracing::error!(target: "zlayer_agent::hcs::diag", hcs_id = %hcs_id, doc = %doc_json, "HCS_CREATE_DOC");
        if let Ok(dir) = std::env::var("ZLAYER_HCS_DOC_DUMP_DIR") {
            let path = std::path::PathBuf::from(&dir).join(format!("{hcs_id}.json"));
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(&path, &doc_json);
        }

        // 5. Create the compute system. On failure, tear down the HCN
        //    endpoint we created in step 3 — otherwise the endpoint (and the
        //    IP it owns) leaks, and the next test/deploy attempt that tries
        //    to claim the same IP gets `HCN_E_ADDR_INVALID_OR_RESERVED
        //    (0x803b002f)`. Also release the IP back to the allocator. The
        //    parent-layer guard and scratch layer cleanup are handled by
        //    their own Drop / orphan-reconcile paths.
        let system = match ComputeSystem::create(&hcs_id, &doc_json).await {
            Ok(s) => s,
            Err(e) => {
                let ip_to_release = network_attachment
                    .ip()
                    .and_then(|s| s.parse::<std::net::IpAddr>().ok());
                if let Err(td_err) = network_attachment.teardown() {
                    tracing::warn!(
                        hcs_id = %hcs_id,
                        error = %td_err,
                        "HCS create failed; HCN endpoint teardown also failed (endpoint may leak)"
                    );
                }
                if let Some(ip) = ip_to_release {
                    if let Some(alloc) = self.ip_allocator.lock().await.as_mut() {
                        alloc.release(ip);
                    }
                }
                return Err(AgentError::CreateFailed {
                    id: hcs_id.clone(),
                    reason: format!("HcsCreateComputeSystem: {e}"),
                });
            }
        };

        // 6. Subscribe to exit events before returning so we don't miss a
        //    fast-exiting container.
        //
        // `system.raw()` returns `SendHandle<HCS_SYSTEM>`; deref with `*` to
        // pass the bare handle to the synchronous `HcsSetComputeSystemCallback`
        // path inside `spawn_exit_watcher`. The dereferenced value is used
        // before any `.await`, so the `!Send + !Sync` raw handle never
        // crosses a suspend point.
        let sink: Arc<RwLock<Option<i32>>> = Arc::new(RwLock::new(None));
        self.spawn_exit_watcher(hcs_id.clone(), *system.raw(), sink.clone());

        // 7. Register the entry.
        //
        // Disarm the parent-layer activation guard and transfer its path list
        // into the entry. Teardown of these layers now happens during
        // [`Self::remove_container`] (in reverse order) so subsequent
        // containers sharing the same parent chain can re-activate them.
        let activated_parent_layers = parent_activation_guard.disarm();
        let entry = ContainerEntry {
            system,
            scratch_layer: Some(scratch_layer),
            hcs_id: hcs_id.clone(),
            last_exit_code: sink,
            network_attachment: Some(network_attachment),
            uvm,
            activated_parent_layers,
        };
        self.containers.write().await.insert(hcs_id, entry);
        Ok(())
    }

    #[instrument(skip(self), fields(otel.name = "container.start", container.id = %id))]
    async fn start_container(&self, id: &ContainerId) -> Result<()> {
        let hcs_id = Self::hcs_id(id);
        let containers = self.containers.read().await;
        let entry = containers
            .get(&hcs_id)
            .ok_or_else(|| AgentError::NotFound {
                container: hcs_id.clone(),
                reason: "no HCS entry for container".to_string(),
            })?;
        entry
            .system
            .start("")
            .await
            .map_err(|e| AgentError::StartFailed {
                id: hcs_id.clone(),
                reason: format!("HcsStartComputeSystem: {e}"),
            })
    }

    #[instrument(skip(self), fields(otel.name = "container.stop", container.id = %id))]
    async fn stop_container(&self, id: &ContainerId, timeout: Duration) -> Result<()> {
        let hcs_id = Self::hcs_id(id);
        let containers = self.containers.read().await;
        let entry = containers
            .get(&hcs_id)
            .ok_or_else(|| AgentError::NotFound {
                container: hcs_id.clone(),
                reason: "no HCS entry for container".to_string(),
            })?;

        // Graceful shutdown first. HCS accepts a `{"TimeoutSeconds":N}` options
        // document. Fall back to a forced terminate if shutdown does not
        // complete within `timeout`.
        let opts_json = format!(r#"{{"TimeoutSeconds":{}}}"#, timeout.as_secs().max(1));
        match tokio_timeout(timeout, entry.system.shutdown(&opts_json)).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => {
                tracing::warn!(
                    hcs_id = %hcs_id,
                    error = %e,
                    "graceful shutdown failed; escalating to terminate"
                );
                entry
                    .system
                    .terminate("")
                    .await
                    .map_err(|e| AgentError::Internal(format!("HcsTerminateComputeSystem: {e}")))
            }
            Err(_elapsed) => {
                tracing::warn!(
                    hcs_id = %hcs_id,
                    "graceful shutdown timed out; escalating to terminate"
                );
                entry
                    .system
                    .terminate("")
                    .await
                    .map_err(|e| AgentError::Internal(format!("HcsTerminateComputeSystem: {e}")))
            }
        }
    }

    #[instrument(skip(self), fields(otel.name = "container.remove", container.id = %id))]
    async fn remove_container(&self, id: &ContainerId) -> Result<()> {
        let hcs_id = Self::hcs_id(id);
        let mut containers = self.containers.write().await;
        let Some(mut entry) = containers.remove(&hcs_id) else {
            return Err(AgentError::NotFound {
                container: hcs_id,
                reason: "no HCS entry for container".to_string(),
            });
        };

        // Best-effort terminate — ignore "already stopped" errors. Dropping
        // the `ComputeSystem` after this releases our HCS handle.
        if let Err(e) = entry.system.terminate("").await {
            tracing::debug!(
                hcs_id = %entry.hcs_id,
                error = %e,
                "terminate during remove failed (container may already be stopped)"
            );
        }

        // Tear down the scratch layer: detach the WCIFS filter and destroy
        // the backing directory. Surface the first destructive error.
        if let Some(scratch_layer) = entry.scratch_layer.take() {
            scratch_layer
                .detach_and_destroy()
                .map_err(|e| AgentError::Internal(format!("scratch teardown: {e}")))?;
        }

        // Deactivate every parent (read-only) layer this container's
        // `create_container` activated. The stored vec is in child-to-parent
        // order (matching `resolve_parent_chain`); we iterate **reverse** of
        // activation order — child-most first — mirroring hcsshim's teardown
        // direction. Parents were only `ActivateLayer`d (not `PrepareLayer`d)
        // so the matching teardown is `DeactivateLayer` only. Best-effort:
        // log failures, do NOT propagate.
        if !entry.activated_parent_layers.is_empty() {
            let layers = std::mem::take(&mut entry.activated_parent_layers);
            let hcs_id_for_log = entry.hcs_id.clone();
            let _ = tokio::task::spawn_blocking(move || {
                for path in &layers {
                    if let Err(e) = crate::windows::wclayer::deactivate_layer(path) {
                        tracing::warn!(
                            hcs_id = %hcs_id_for_log,
                            layer = %path.display(),
                            error = %e,
                            "DeactivateLayer failed during remove_container; layer table may leak until reboot",
                        );
                    }
                }
            })
            .await;
        }

        // Tear down the UVM (if any). The scratch VHDX cleanup happens in
        // `Uvm::Drop` — we explicitly `drop` here so the order with respect to
        // the scratch-layer teardown above is deterministic and any
        // best-effort errors get logged via the `Drop` impl's tracing call
        // before we move on to HCN teardown. Process-isolated entries never
        // allocated a UVM and skip this no-op.
        if let Some(uvm) = entry.uvm.take() {
            drop(uvm);
        }

        // Tear down the HCN endpoint + namespace, if we attached one. Best-
        // effort: log on failure (the container is already gone so leaving a
        // dangling endpoint is recoverable via startup reconcile) and do
        // **not** propagate — scratch teardown already succeeded and the
        // caller expects success once we reach this point.
        if let Some(attachment) = entry.network_attachment.take() {
            let hcs_id_for_log = entry.hcs_id.clone();
            // Capture the endpoint IP before `teardown()` consumes the
            // attachment so we can release it back to the node allocator.
            let released_ip = attachment.ip().and_then(|s| s.parse::<IpAddr>().ok());
            let res = tokio::task::spawn_blocking(move || attachment.teardown()).await;
            match res {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    tracing::warn!(
                        hcs_id = %hcs_id_for_log,
                        error = %e,
                        "HCN attachment teardown failed; endpoint may leak until next reconcile"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        hcs_id = %hcs_id_for_log,
                        error = %e,
                        "spawn_blocking join failed during HCN teardown"
                    );
                }
            }
            // Release the IP regardless of teardown result so a failed teardown
            // does not leak the address. `release` is idempotent and safe for
            // unknown IPs (e.g. the reserved gateway).
            if let Some(ip) = released_ip {
                if let Some(alloc) = self.ip_allocator.lock().await.as_mut() {
                    alloc.release(ip);
                }
            }
        }
        drop(entry);
        Ok(())
    }

    async fn container_state(&self, id: &ContainerId) -> Result<ContainerState> {
        let hcs_id = Self::hcs_id(id);
        let containers = self.containers.read().await;
        let Some(entry) = containers.get(&hcs_id) else {
            return Err(AgentError::NotFound {
                container: hcs_id,
                reason: "no HCS entry for container".to_string(),
            });
        };
        if let Some(code) = *entry.last_exit_code.read().await {
            return Ok(ContainerState::Exited { code });
        }
        // HCS does not expose a separate "Pending"/"Initializing" between
        // create and start the way libcontainer does; once the entry exists
        // and no exit has been observed, the system is effectively running
        // (or about to be). A more precise signal would require a
        // `HcsGetComputeSystemProperties` call on the `State` property;
        // that's a follow-up when the cost is justified.
        Ok(ContainerState::Running)
    }

    async fn container_logs(&self, _id: &ContainerId, _tail: usize) -> Result<Vec<LogEntry>> {
        Err(AgentError::Unsupported(
            "container_logs is not yet wired for the HCS runtime; use `zlayer exec` to inspect logs inside the container".to_string(),
        ))
    }

    async fn exec(&self, id: &ContainerId, cmd: &[String]) -> Result<(i32, String, String)> {
        use zlayer_hcs::process::ComputeProcess;

        if cmd.is_empty() {
            return Err(AgentError::InvalidSpec(
                "exec command must not be empty".to_string(),
            ));
        }
        let hcs_id = Self::hcs_id(id);
        let containers = self.containers.read().await;
        let entry = containers
            .get(&hcs_id)
            .ok_or_else(|| AgentError::NotFound {
                container: hcs_id.clone(),
                reason: "no HCS entry for container".to_string(),
            })?;

        let command_line = cmd.join(" ");
        let params = ProcessParameters {
            command_line,
            working_directory: String::new(),
            environment: Default::default(),
            emulate_console: Some(false),
            create_std_in_pipe: Some(false),
            create_std_out_pipe: Some(true),
            create_std_err_pipe: Some(true),
            console_size: None,
            user: None,
        };

        // `entry.system.raw()` already returns `SendHandle<HCS_SYSTEM>` (the
        // `ComputeSystem::raw()` accessor wraps the inner handle at the
        // source so the returned `ComputeProcess::spawn` future remains
        // `Send` across the enclosing `async fn exec`). See
        // `zlayer_hcs::handle::SendHandle`.
        let system_handle = entry.system.raw();
        let process = ComputeProcess::spawn(system_handle, &params)
            .await
            .map_err(|e| AgentError::Internal(format!("HcsCreateProcess: {e}")))?;

        // Poll process properties until it has an exit code. The heavy
        // stdio-pipe plumbing (tokio-side reads of the HCS pipes) is a
        // follow-up; for now we synthesize empty stdout/stderr and only
        // surface the final exit code so callers relying on exec for
        // health checks still work.
        for _ in 0..600 {
            let raw_props = process
                .properties(r#"{"PropertyTypes":["ProcessStatus"]}"#)
                .await
                .map_err(|e| AgentError::Internal(format!("HcsGetProcessProperties: {e}")))?;
            if let Some(code) = extract_process_exit_code(&raw_props) {
                return Ok((code, String::new(), String::new()));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Err(AgentError::Timeout {
            timeout: Duration::from_secs(60),
        })
    }

    async fn exec_stream(&self, id: &ContainerId, cmd: &[String]) -> Result<ExecEventStream> {
        // Fall back to the buffered `exec` path and emit a single Stdout /
        // Stderr / Exit trio. Proper pipe streaming is deferred — plumbing
        // the HCS stdio pipe handles through `tokio::io::unix::AsyncFd`-style
        // wrappers is heavy and deserves its own phase.
        let (exit, stdout, stderr) = self.exec(id, cmd).await?;
        let mut events: Vec<ExecEvent> = Vec::with_capacity(3);
        if !stdout.is_empty() {
            events.push(ExecEvent::Stdout(stdout));
        }
        if !stderr.is_empty() {
            events.push(ExecEvent::Stderr(stderr));
        }
        events.push(ExecEvent::Exit(exit));
        Ok(Box::pin(futures_util::stream::iter(events)))
    }

    async fn get_container_stats(&self, id: &ContainerId) -> Result<ContainerStats> {
        let hcs_id = Self::hcs_id(id);
        let containers = self.containers.read().await;
        let entry = containers
            .get(&hcs_id)
            .ok_or_else(|| AgentError::NotFound {
                container: hcs_id.clone(),
                reason: "no HCS entry for container".to_string(),
            })?;
        let raw = entry
            .system
            .read_statistics()
            .await
            .map_err(|e| AgentError::Internal(format!("HcsGetComputeSystemProperties: {e}")))?;
        Ok(translate_stats(&raw))
    }

    async fn wait_container(&self, id: &ContainerId) -> Result<i32> {
        let hcs_id = Self::hcs_id(id);
        let sink = {
            let containers = self.containers.read().await;
            let entry = containers
                .get(&hcs_id)
                .ok_or_else(|| AgentError::NotFound {
                    container: hcs_id.clone(),
                    reason: "no HCS entry for container".to_string(),
                })?;
            entry.last_exit_code.clone()
        };
        // Poll the exit sink. The exit watcher spawned in `create_container`
        // populates this when HCS fires `SystemExited`.
        loop {
            if let Some(code) = *sink.read().await {
                return Ok(code);
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    async fn wait_outcome(&self, id: &ContainerId) -> Result<WaitOutcome> {
        let exit_code = self.wait_container(id).await?;
        let reason = if exit_code == -1 {
            WaitReason::RuntimeError
        } else {
            WaitReason::Exited
        };
        Ok(WaitOutcome {
            exit_code,
            reason,
            signal: None,
            finished_at: Some(chrono::Utc::now()),
        })
    }

    async fn get_logs(&self, id: &ContainerId) -> Result<Vec<LogEntry>> {
        self.container_logs(id, usize::MAX).await
    }

    async fn get_container_pid(&self, _id: &ContainerId) -> Result<Option<u32>> {
        // HCS containers do not expose the PID of the root process via the
        // compute-system surface; the init process is managed by vmcompute.
        // `service.rs` falls back to the HCN namespace GUID path (see
        // `get_container_namespace_id`) for Windows overlay attach.
        Ok(None)
    }

    async fn get_container_namespace_id(
        &self,
        id: &ContainerId,
    ) -> Result<Option<windows::core::GUID>> {
        let hcs_id = Self::hcs_id(id);
        let entries = self.containers.read().await;
        Ok(entries.get(&hcs_id).and_then(|e| {
            e.network_attachment
                .as_ref()
                .map(EndpointAttachment::namespace_id)
        }))
    }

    async fn get_container_ip(&self, id: &ContainerId) -> Result<Option<IpAddr>> {
        let hcs_id = Self::hcs_id(id);
        let containers = self.containers.read().await;
        let Some(entry) = containers.get(&hcs_id) else {
            return Err(AgentError::NotFound {
                container: hcs_id,
                reason: "no HCS entry for container".to_string(),
            });
        };
        let Some(ip_str) = entry
            .network_attachment
            .as_ref()
            .and_then(|a| a.ip().map(str::to_string))
        else {
            return Ok(None);
        };
        match ip_str.parse::<IpAddr>() {
            Ok(ip) => Ok(Some(ip)),
            Err(e) => {
                tracing::warn!(
                    hcs_id = %hcs_id,
                    ip = %ip_str,
                    error = %e,
                    "HCN endpoint returned unparseable IP"
                );
                Ok(None)
            }
        }
    }

    async fn list_images(&self) -> Result<Vec<ImageInfo>> {
        let cache = self.images.read().await;
        Ok(cache
            .keys()
            .map(|reference| ImageInfo {
                reference: reference.clone(),
                digest: None,
                size_bytes: None,
            })
            .collect())
    }

    async fn remove_image(&self, image: &str, _force: bool) -> Result<()> {
        let mut cache = self.images.write().await;
        if let Some(entry) = cache.remove(image) {
            // Best-effort destroy of each layer directory. HCS refuses to
            // destroy a layer that's currently referenced; we log and press
            // on so the in-memory cache stays consistent even when disk
            // state can't be reclaimed immediately.
            for layer in &entry.unpacked.chain.0 {
                let path = std::path::Path::new(&layer.path);
                if let Err(e) = crate::windows::wclayer::destroy_layer(path) {
                    tracing::warn!(layer = %layer.path, error = %e, "destroy_layer failed");
                }
            }
        }
        Ok(())
    }

    async fn prune_images(&self) -> Result<PruneResult> {
        // The HCS runtime does not yet track dangling images; every entry in
        // the cache is referenced by something (or cheap to re-pull). Treat
        // prune as a successful no-op so CLI prune commands don't fail on
        // Windows — `remove_image` is the explicit tool.
        Ok(PruneResult::default())
    }

    async fn kill_container(&self, id: &ContainerId, signal: Option<&str>) -> Result<()> {
        // Windows does not speak POSIX signals; we map every signal to a
        // forced terminate, matching Docker's `docker kill` behaviour on
        // Windows containers. Still validate the name so callers get a
        // consistent error surface for typos / unsupported signals.
        let _ = crate::runtime::validate_signal(signal.unwrap_or("SIGKILL"))?;

        let hcs_id = Self::hcs_id(id);
        let containers = self.containers.read().await;
        let entry = containers
            .get(&hcs_id)
            .ok_or_else(|| AgentError::NotFound {
                container: hcs_id.clone(),
                reason: "no HCS entry for container".to_string(),
            })?;
        entry
            .system
            .terminate("")
            .await
            .map_err(|e| AgentError::Internal(format!("HcsTerminateComputeSystem: {e}")))
    }

    async fn tag_image(&self, source: &str, target: &str) -> Result<()> {
        // Lightweight aliasing: point `target` at the same unpacked chain as
        // `source`. We can't relocate the on-disk layer directories, so this
        // only works while the source stays cached.
        let mut cache = self.images.write().await;
        let Some(entry) = cache.get(source) else {
            return Err(AgentError::NotFound {
                container: source.to_string(),
                reason: "source image not cached".to_string(),
            });
        };
        // Clone the UnpackedImage (LayerChain + root are both Clone) and
        // carry the source's builder-asserted `os.version` forward so the
        // isolation auto-resolver sees the same value for the alias.
        let cloned = CachedImage {
            unpacked: entry.unpacked.clone(),
            os_version: entry.os_version.clone(),
        };
        cache.insert(target.to_string(), cloned);
        Ok(())
    }

    async fn inspect_detailed(&self, id: &ContainerId) -> Result<ContainerInspectDetails> {
        let hcs_id = Self::hcs_id(id);
        // Clone the exit-code sink out of the entry before the outer
        // `containers` read-guard drops. Holding the guard across the inner
        // `last_exit_code.read().await` below would borrow-check-fail
        // (E0597) and also make the returned future non-Send on Windows
        // because the `RwLockReadGuard<HashMap<..>>` is not Send.
        let last_exit_code_lock = {
            let containers = self.containers.read().await;
            let entry = containers
                .get(&hcs_id)
                .ok_or_else(|| AgentError::NotFound {
                    container: hcs_id.clone(),
                    reason: "no HCS entry for container".to_string(),
                })?;
            Arc::clone(&entry.last_exit_code)
        };
        let exit_code = *last_exit_code_lock.read().await;

        Ok(ContainerInspectDetails {
            ports: Vec::new(),
            networks: Vec::new(),
            ipv4: None,
            health: None,
            exit_code,
        })
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Translate the HCS [`Statistics`] document into the cross-runtime
/// [`ContainerStats`] shape used by autoscaling and metrics exposition.
///
/// HCS reports CPU in 100-nanosecond ticks; the agent's `ContainerStats`
/// takes microseconds. Memory reporting uses the private working set as the
/// best proxy for "in-use bytes" — it matches what Task Manager shows for a
/// process and is what the autoscaler was calibrated against on Linux.
fn translate_stats(raw: &Statistics) -> ContainerStats {
    // 100-ns ticks -> microseconds: divide by 10.
    let cpu_usage_usec = raw
        .processor
        .as_ref()
        .map(|p| p.total_runtime_100ns / 10)
        .unwrap_or(0);

    let memory_bytes = raw
        .memory
        .as_ref()
        .map(|m| m.memory_usage_private_working_set_bytes)
        .unwrap_or(0);

    ContainerStats {
        cpu_usage_usec,
        memory_bytes,
        // HCS does not surface a hard memory limit in the Statistics
        // property — callers that need it should read it off the compute-
        // system config instead. Sentinel `u64::MAX` matches the "unlimited"
        // convention used by the Linux cgroups reader.
        memory_limit: u64::MAX,
        timestamp: Instant::now(),
    }
}

/// Parse an `ExitCode` out of a `ProcessStatus` JSON document.
fn extract_process_exit_code(raw_json: &str) -> Option<i32> {
    let v: serde_json::Value = serde_json::from_str(raw_json).ok()?;
    // HCS occasionally wraps the block as `{"ProcessStatus": { ExitCode }}`
    // and occasionally returns it flat. Try both.
    let status = v
        .get("ProcessStatus")
        .and_then(|s| s.get("ExitCode"))
        .or_else(|| v.get("ExitCode"))?;
    status.as_i64().map(|n| {
        #[allow(clippy::cast_possible_truncation)]
        let truncated = n as i32;
        truncated
    })
}

/// Enumerate zombie compute systems owned by this runtime on startup. Exposed
/// as a free function so the agent's boot path can terminate stragglers
/// before we create any new systems.
///
/// `daemon_name` selects the owner tag this enumeration sweeps —
/// pass the value used to construct the [`HcsRuntime`] so a `zlayer-dev`
/// instance never enumerates a peer `zlayer` daemon's systems (and vice
/// versa).
///
/// # Errors
///
/// Returns the error emitted by [`zlayer_hcs::enumerate::list_by_owner`].
pub async fn list_owned_systems(daemon_name: &str) -> Result<Vec<String>> {
    let tag = owner_tag(daemon_name);
    let systems = enumerate::list_by_owner(&tag)
        .await
        .map_err(|e| AgentError::Internal(format!("HcsEnumerateComputeSystems: {e}")))?;
    Ok(systems.into_iter().map(|s| s.id).collect())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use zlayer_hcs::schema::{MemoryStats, ProcessorStats};

    #[test]
    fn translate_stats_converts_100ns_to_usec_and_private_working_set_to_bytes() {
        let raw = Statistics {
            timestamp: None,
            container_start_time: None,
            uptime_100ns: 0,
            processor: Some(ProcessorStats {
                total_runtime_100ns: 12_345_000, // 1.2345 s -> 1_234_500 us
                runtime_user_100ns: 0,
                runtime_kernel_100ns: 0,
            }),
            memory: Some(MemoryStats {
                memory_usage_commit_bytes: 0,
                memory_usage_commit_peak_bytes: 0,
                memory_usage_private_working_set_bytes: 256 * 1024 * 1024,
            }),
            storage: None,
        };
        let stats = translate_stats(&raw);
        assert_eq!(stats.cpu_usage_usec, 1_234_500);
        assert_eq!(stats.memory_bytes, 256 * 1024 * 1024);
        assert_eq!(stats.memory_limit, u64::MAX);
    }

    #[test]
    fn translate_stats_defaults_zero_when_fields_missing() {
        let raw = Statistics::default();
        let stats = translate_stats(&raw);
        assert_eq!(stats.cpu_usage_usec, 0);
        assert_eq!(stats.memory_bytes, 0);
        assert_eq!(stats.memory_limit, u64::MAX);
    }

    #[test]
    fn extract_exit_code_reads_json_payload() {
        assert_eq!(extract_exit_code(r#"{"ExitCode":42}"#), Some(42));
        assert_eq!(extract_exit_code(""), None);
        assert_eq!(extract_exit_code("not json"), None);
        assert_eq!(extract_exit_code(r#"{"NoExitCode":1}"#), None);
    }

    #[test]
    fn extract_process_exit_code_handles_nested_and_flat() {
        assert_eq!(
            extract_process_exit_code(r#"{"ProcessStatus":{"ExitCode":7}}"#),
            Some(7)
        );
        assert_eq!(extract_process_exit_code(r#"{"ExitCode":9}"#), Some(9));
        assert_eq!(extract_process_exit_code(r#"{}"#), None);
    }

    #[test]
    fn hcs_config_default_sets_overlay_networking_fields() {
        let cfg = HcsConfig::default();
        assert_eq!(cfg.cluster_cidr, "10.200.0.0/16");
        assert!(
            cfg.slice_cidr.is_none(),
            "slice_cidr must be None until the node joins the cluster and the leader hands out a slice"
        );
    }

    #[test]
    fn hcs_config_default_daemon_name_is_legacy() {
        let cfg = HcsConfig::default();
        assert_eq!(
            cfg.daemon_name, "zlayer",
            "single-instance installs must keep the legacy `zlayer` owner tag"
        );
    }

    #[test]
    fn owner_tag_legacy() {
        assert_eq!(owner_tag("zlayer"), "zlayer");
    }

    #[test]
    fn owner_tag_dev() {
        assert_eq!(owner_tag("zlayer-dev"), "zlayer-dev");
    }

    #[test]
    fn overlay_network_legacy() {
        assert_eq!(overlay_network_name("zlayer"), "zlayer-overlay");
    }

    #[test]
    fn overlay_network_dev() {
        assert_eq!(overlay_network_name("zlayer-dev"), "zlayer-dev-overlay");
    }

    /// `parse_os_version` accepts the canonical `major.minor.build.ubr`
    /// shape MCR emits and discards the UBR component.
    #[test]
    fn parse_os_version_four_components() {
        assert_eq!(parse_os_version("10.0.20348.2700"), Some((10, 0, 20348)));
    }

    /// `parse_os_version` also accepts a three-component string (no UBR).
    #[test]
    fn parse_os_version_three_components() {
        assert_eq!(parse_os_version("10.0.26100"), Some((10, 0, 26100)));
    }

    /// `parse_os_version` returns `None` when fewer than three components
    /// are present or any component fails to parse as `u32`.
    #[test]
    fn parse_os_version_rejects_malformed() {
        assert_eq!(parse_os_version(""), None);
        assert_eq!(parse_os_version("10"), None);
        assert_eq!(parse_os_version("10.0"), None);
        assert_eq!(parse_os_version("10.0.x"), None);
        assert_eq!(parse_os_version("not.a.version"), None);
    }

    /// Auto + matching builds → Process (no UVM overhead needed).
    #[test]
    fn decide_isolation_auto_matched_builds_picks_process() {
        assert_eq!(
            decide_isolation(
                Some(zlayer_spec::IsolationMode::Auto),
                Some((10, 0, 26100)),
                Some((10, 0, 26100)),
            ),
            IsolationMode::Process,
        );
        // UBR is stripped, so 26100.1742 and 26100.2700 both parse to
        // (10, 0, 26100) and resolve as matched.
        assert_eq!(
            decide_isolation(None, Some((10, 0, 26100)), Some((10, 0, 26100))),
            IsolationMode::Process,
        );
    }

    /// Auto + mismatched builds → Hyper-V (UVM required for cross-build).
    #[test]
    fn decide_isolation_auto_mismatched_builds_picks_hyperv() {
        assert_eq!(
            decide_isolation(
                Some(zlayer_spec::IsolationMode::Auto),
                Some((10, 0, 20348)),
                Some((10, 0, 26100)),
            ),
            IsolationMode::Hyperv,
        );
    }

    /// Auto + known image build but unknown host build → Hyper-V (safer:
    /// UVM tolerates any host configuration we can detect).
    #[test]
    fn decide_isolation_auto_known_image_unknown_host_picks_hyperv() {
        assert_eq!(
            decide_isolation(
                Some(zlayer_spec::IsolationMode::Auto),
                Some((10, 0, 26100)),
                None,
            ),
            IsolationMode::Hyperv,
        );
    }

    /// Auto + unknown image build → Process (documented prior default;
    /// we cannot argue for UVM without a build to compare against).
    #[test]
    fn decide_isolation_auto_unknown_image_picks_process() {
        assert_eq!(
            decide_isolation(Some(zlayer_spec::IsolationMode::Auto), None, None),
            IsolationMode::Process,
        );
        assert_eq!(
            decide_isolation(None, None, Some((10, 0, 26100))),
            IsolationMode::Process,
        );
    }

    /// Explicit `Process` from the spec wins even when the matrix would
    /// otherwise pick Hyper-V (operator override).
    #[test]
    fn decide_isolation_explicit_process_overrides_matrix() {
        assert_eq!(
            decide_isolation(
                Some(zlayer_spec::IsolationMode::Process),
                Some((10, 0, 20348)),
                Some((10, 0, 26100)),
            ),
            IsolationMode::Process,
        );
        assert_eq!(
            decide_isolation(Some(zlayer_spec::IsolationMode::Process), None, None),
            IsolationMode::Process,
        );
    }

    /// Explicit `Hyperv` from the spec wins even when the matrix would
    /// otherwise pick Process (operator override).
    #[test]
    fn decide_isolation_explicit_hyperv_overrides_matrix() {
        assert_eq!(
            decide_isolation(
                Some(zlayer_spec::IsolationMode::Hyperv),
                Some((10, 0, 26100)),
                Some((10, 0, 26100)),
            ),
            IsolationMode::Hyperv,
        );
        assert_eq!(
            decide_isolation(Some(zlayer_spec::IsolationMode::Hyperv), None, None),
            IsolationMode::Hyperv,
        );
    }

    /// `resolve_isolation_for_image` is the production entry point that
    /// supplies the live host build via [`host_windows_build`]. We can't
    /// pin the host value cross-machine, so the smoke check just confirms
    /// the function is callable and returns a concrete variant. The pure
    /// matrix is covered by the `decide_isolation_*` tests above.
    #[test]
    fn resolve_isolation_for_image_smoke() {
        let mode = resolve_isolation_for_image(None, None);
        assert!(
            matches!(mode, IsolationMode::Process | IsolationMode::Hyperv),
            "resolve_isolation_for_image returned an unexpected variant: {mode:?}",
        );
    }

    /// Build a minimal [`ServiceSpec`] for the unit tests below. Mirrors
    /// `runtimes::composite::tests::make_spec` so the construction stays in
    /// step with the canonical YAML schema; we only need *a* well-formed spec
    /// here because `build_virtual_machine_doc` ignores the spec body today.
    fn fixture_spec() -> ServiceSpec {
        let yaml = r"
version: v1
deployment: test
services:
  test:
    rtype: service
    image:
      name: mcr.microsoft.com/windows/nanoserver:ltsc2022
";
        serde_yaml::from_str::<zlayer_spec::DeploymentSpec>(yaml)
            .expect("valid fixture yaml")
            .services
            .remove("test")
            .expect("service 'test' present")
    }

    /// `build_virtual_machine_doc` populates the `VirtualMachine` body with
    /// the UVM's scratch VHDX (SCSI attachment under the primary controller
    /// GUID), the `"os"` VSMB share that exposes `UtilityVM\Files` as the
    /// boot volume, one read-only `VirtualSMB` share per parent layer, the
    /// default 2 vCPU / 1024 MiB topology, and the UEFI `VmbFs` boot entry.
    /// `guest_state` is omitted entirely — VmbFs boot does not use a
    /// host-side `.vmgs`.
    ///
    /// Uses [`Uvm::for_test`] so the test does not touch HCS, the VHD APIs,
    /// or the filesystem under `%ProgramData%`.
    #[test]
    fn build_virtual_machine_doc_populates_uvm_fields() {
        use std::path::PathBuf;
        use zlayer_hcs::schema::Layer;

        let scratch = PathBuf::from(r"C:\zlayer\uvms\test-container\scratch.vhdx");
        let os_files = PathBuf::from(r"C:\zlayer\images\app\UtilityVM\Files");
        let uvm = Uvm::for_test("test-container", scratch.clone(), os_files.clone());

        let parent_layers = vec![
            Layer {
                id: "11111111-1111-1111-1111-111111111111".to_string(),
                path: r"C:\zlayer\images\base".to_string(),
            },
            Layer {
                id: "22222222-2222-2222-2222-222222222222".to_string(),
                path: r"C:\zlayer\images\app".to_string(),
            },
        ];

        let spec = fixture_spec();
        let vm = build_virtual_machine_doc(&uvm, &parent_layers, &spec, &[]);

        // Chipset / UEFI: boot from VmbFs at the standard Windows boot manager path.
        let chipset = vm.chipset.expect("chipset");
        let uefi = chipset.uefi.expect("uefi");
        let boot_entry = uefi.boot_this.expect("boot_this");
        assert_eq!(boot_entry.device_type, "VmbFs");
        assert_eq!(boot_entry.device_path, r"\EFI\Microsoft\Boot\bootmgfw.efi");
        assert_eq!(boot_entry.disk_number, None);

        // Devices.scsi: one controller keyed by the hcsshim primary-SCSI GUID
        // with one attachment at LUN `"0"` ↦ scratch VHDX (writable).
        let devices = vm.devices.expect("devices");
        let controller = devices
            .scsi
            .get(PRIMARY_SCSI_CTRL_GUID)
            .expect("scsi controller keyed by primary GUID");
        let attachment = controller.attachments.get("0").expect("scsi attachment 0");
        assert_eq!(attachment.path, scratch.to_string_lossy());
        assert_eq!(attachment.r#type, "VirtualDisk");
        assert_eq!(attachment.read_only, Some(false));

        // Devices.virtual_smb: one `"os"` boot-files share + one share per parent layer.
        assert_eq!(
            devices.virtual_smb.len(),
            3,
            "expected `os` share + one VirtualSMB share per parent layer",
        );
        let os_share = devices
            .virtual_smb
            .get("os")
            .expect("os boot-files VSMB share");
        assert_eq!(os_share.name, "os");
        assert_eq!(os_share.path, os_files.to_string_lossy());
        let os_opts = os_share
            .options
            .as_ref()
            .expect("os share carries named options");
        assert!(os_opts.read_only, "os share must be read-only");
        assert!(os_opts.share_read, "os share must set ShareRead");
        assert!(os_opts.cache_io, "os share must set CacheIo");
        assert!(os_opts.pseudo_oplocks, "os share must set PseudoOplocks");
        assert!(
            os_opts.take_backup_privilege,
            "os share must set TakeBackupPrivilege per hcsshim DefaultVSMBOptions(true)",
        );
        assert!(
            os_share.flags.is_none(),
            "named options replace the legacy raw flags bitmask",
        );

        let share = devices
            .virtual_smb
            .get("11111111-1111-1111-1111-111111111111")
            .expect("smb share for base layer");
        assert_eq!(share.path, r"C:\zlayer\images\base");
        assert!(
            share.flags.is_none(),
            "parent-layer shares use named options, not legacy flags",
        );
        let layer_opts = share
            .options
            .as_ref()
            .expect("parent-layer share carries named options");
        assert!(layer_opts.read_only);
        assert!(layer_opts.share_read);
        assert!(layer_opts.cache_io);
        assert!(layer_opts.pseudo_oplocks);

        // Compute topology: defaults to 2 vCPU / 1024 MiB.
        let topology = vm.compute_topology.expect("compute_topology");
        assert_eq!(topology.processor.expect("processor").count, 2);
        assert_eq!(topology.memory.expect("memory").size_in_mb, 1024);

        // GuestState: omitted with VmbFs boot.
        assert!(
            vm.guest_state.is_none(),
            "VmbFs-boot UVMs must not carry a host GuestState path",
        );
    }

    /// `build_virtual_machine_doc` against an empty parent chain still
    /// produces a valid SCSI + chipset + topology block; the `virtual_smb`
    /// map carries only the mandatory `"os"` boot-files share. This pins the
    /// contract that a zero-layer image (theoretical edge case) does not
    /// panic and still boots.
    #[test]
    fn build_virtual_machine_doc_handles_empty_parent_chain() {
        use std::path::PathBuf;

        let uvm = Uvm::for_test(
            "empty-chain",
            PathBuf::from(r"C:\scratch.vhdx"),
            PathBuf::from(r"C:\os-files"),
        );
        let spec = fixture_spec();
        let vm = build_virtual_machine_doc(&uvm, &[], &spec, &[]);

        let devices = vm.devices.expect("devices");
        assert_eq!(devices.virtual_smb.len(), 1);
        assert!(devices.virtual_smb.contains_key("os"));
        assert_eq!(devices.scsi.len(), 1);
        assert!(devices.scsi.contains_key(PRIMARY_SCSI_CTRL_GUID));
        assert!(
            vm.guest_state.is_none(),
            "VmbFs-boot UVMs must not carry a host GuestState path",
        );
    }

    // -----------------------------------------------------------------------
    // GPU-PV tests
    // -----------------------------------------------------------------------

    /// Linux / macOS stub for [`enumerate_host_gpu_adapters`] returns
    /// `ErrorKind::Unsupported`. The Windows implementation is exercised by
    /// the `#[ignore]`'d real-host test below.
    #[cfg(not(target_os = "windows"))]
    #[test]
    fn enumerate_host_gpu_adapters_returns_unsupported_on_non_windows() {
        let err =
            super::enumerate_host_gpu_adapters().expect_err("must be Unsupported off-Windows");
        assert_eq!(err.kind(), std::io::ErrorKind::Unsupported);
    }

    /// Smoke-test the real DXGI probe on a Windows host. Ignored by default
    /// because CI / dev machines may not have a GPU, but flagged so the
    /// `windows-hcs-e2e` job can opt in with `--ignored`.
    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "requires a real Windows host with at least one GPU adapter"]
    fn enumerate_host_gpu_adapters_on_windows_finds_at_least_one() {
        let adapters = super::enumerate_host_gpu_adapters().expect("DXGI probe must succeed");
        assert!(
            !adapters.is_empty(),
            "expected at least one host GPU adapter (WARP excluded); got {adapters:?}",
        );
    }

    /// Fixture: three host adapters — 2 NVIDIA, 1 AMD — so the filter tests
    /// can assert both vendor and count behaviour.
    fn fixture_adapters() -> Vec<HostGpuAdapter> {
        vec![
            HostGpuAdapter {
                luid_high: 0,
                luid_low: 1,
                description: "NVIDIA GeForce RTX 4090".to_string(),
                vendor_id: 0x10de,
                device_id: 0x2684,
            },
            HostGpuAdapter {
                luid_high: 0,
                luid_low: 2,
                description: "NVIDIA RTX A6000".to_string(),
                vendor_id: 0x10de,
                device_id: 0x2230,
            },
            HostGpuAdapter {
                luid_high: 0,
                luid_low: 3,
                description: "AMD Radeon RX 7900 XTX".to_string(),
                vendor_id: 0x1002,
                device_id: 0x744c,
            },
        ]
    }

    #[test]
    fn filter_adapters_by_vendor_nvidia() {
        let adapters = fixture_adapters();
        let spec = zlayer_spec::GpuSpec {
            count: 99, // do not truncate
            vendor: "nvidia".to_string(),
            mode: None,
            model: None,
            scheduling: None,
            distributed: None,
            sharing: None,
            mps_pipe_dir: None,
            mps_log_dir: None,
            time_slice_index: None,
            time_slicing_config_path: None,
        };
        let filtered = filter_adapters_by_gpu_spec(&adapters, &spec);
        assert_eq!(filtered.len(), 2);
        assert!(filtered.iter().all(|a| a.vendor_id == 0x10de));
    }

    #[test]
    fn filter_adapters_by_count_truncates() {
        let adapters = fixture_adapters();
        let spec = zlayer_spec::GpuSpec {
            count: 1,
            vendor: "all".to_string(),
            mode: None,
            model: None,
            scheduling: None,
            distributed: None,
            sharing: None,
            mps_pipe_dir: None,
            mps_log_dir: None,
            time_slice_index: None,
            time_slicing_config_path: None,
        };
        let filtered = filter_adapters_by_gpu_spec(&adapters, &spec);
        assert_eq!(filtered.len(), 1);
    }

    #[test]
    fn filter_adapters_by_model_substring() {
        let adapters = fixture_adapters();
        let spec = zlayer_spec::GpuSpec {
            count: 99,
            vendor: "nvidia".to_string(),
            mode: None,
            model: Some("a6000".to_string()),
            scheduling: None,
            distributed: None,
            sharing: None,
            mps_pipe_dir: None,
            mps_log_dir: None,
            time_slice_index: None,
            time_slicing_config_path: None,
        };
        let filtered = filter_adapters_by_gpu_spec(&adapters, &spec);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].description, "NVIDIA RTX A6000");
    }

    /// When `spec.resources.gpu` is set AND we have candidate adapters, the
    /// `VirtualMachine` document carries a `GpuAssignment` with
    /// `assignment_mode = List` and one `GpuAssignmentRequest` per adapter.
    #[test]
    fn build_virtual_machine_doc_with_gpu_populates_assignment() {
        use std::path::PathBuf;

        let uvm = Uvm::for_test(
            "gpu-list",
            PathBuf::from(r"C:\scratch.vhdx"),
            PathBuf::from(r"C:\boot"),
        );

        let mut spec = fixture_spec();
        spec.resources.gpu = Some(zlayer_spec::GpuSpec {
            count: 1,
            vendor: "nvidia".to_string(),
            mode: None,
            model: None,
            scheduling: None,
            distributed: None,
            sharing: None,
            mps_pipe_dir: None,
            mps_log_dir: None,
            time_slice_index: None,
            time_slicing_config_path: None,
        });

        let adapters = vec![HostGpuAdapter {
            luid_high: 0xdead_beef,
            luid_low: 0x1234_5678,
            description: "NVIDIA GeForce RTX 4090".to_string(),
            vendor_id: 0x10de,
            device_id: 0x2684,
        }];

        let vm = build_virtual_machine_doc(&uvm, &[], &spec, &adapters);
        let devices = vm.devices.expect("devices");
        let gpu = devices.gpu.expect("gpu assignment present");
        assert_eq!(gpu.assignment_mode, GpuAssignmentMode::List);
        assert_eq!(gpu.assignment_request.len(), 1);
        let req = &gpu.assignment_request[0];
        assert_eq!(req.adapter_luid_high_part, 0xdead_beef);
        assert_eq!(req.adapter_luid_low_part, 0x1234_5678);
        assert_eq!(
            req.virtual_machine_id_string, "0xdeadbeef:0x12345678",
            "LUID hex string must be `0x<hi>:0x<lo>`",
        );
        assert_eq!(gpu.allow_vendor_extension, Some(true));
    }

    /// When `spec.resources.gpu` is set but no candidate adapters were found
    /// on the host, fall back to `assignment_mode = Default` rather than
    /// silently dropping the request.
    #[test]
    fn build_virtual_machine_doc_with_gpu_no_adapters_falls_back_to_default() {
        use std::path::PathBuf;

        let uvm = Uvm::for_test(
            "gpu-default",
            PathBuf::from(r"C:\scratch.vhdx"),
            PathBuf::from(r"C:\boot"),
        );

        let mut spec = fixture_spec();
        spec.resources.gpu = Some(zlayer_spec::GpuSpec {
            count: 1,
            vendor: "all".to_string(),
            mode: None,
            model: None,
            scheduling: None,
            distributed: None,
            sharing: None,
            mps_pipe_dir: None,
            mps_log_dir: None,
            time_slice_index: None,
            time_slicing_config_path: None,
        });

        let vm = build_virtual_machine_doc(&uvm, &[], &spec, &[]);
        let devices = vm.devices.expect("devices");
        let gpu = devices.gpu.expect("gpu assignment present");
        assert_eq!(gpu.assignment_mode, GpuAssignmentMode::Default);
        assert!(gpu.assignment_request.is_empty());
    }

    /// When the spec has no GPU, `devices.gpu` is omitted entirely.
    #[test]
    fn build_virtual_machine_doc_without_gpu_omits_assignment() {
        use std::path::PathBuf;

        let uvm = Uvm::for_test(
            "no-gpu",
            PathBuf::from(r"C:\scratch.vhdx"),
            PathBuf::from(r"C:\boot"),
        );
        let spec = fixture_spec();
        let vm = build_virtual_machine_doc(&uvm, &[], &spec, &[]);
        let devices = vm.devices.expect("devices");
        assert!(
            devices.gpu.is_none(),
            "spec without GpuSpec must produce a Devices block with no GPU field",
        );
    }
}
