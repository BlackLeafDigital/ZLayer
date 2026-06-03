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
use zlayer_observability::logs::LogEntry;
use zlayer_overlay::ipnet;
use zlayer_spec::{PullPolicy, RegistryAuth as SpecRegistryAuth, ServiceSpec};

use zlayer_overlayd::OverlaydClient;
use zlayer_types::overlayd::{AttachHandle, OverlaydRequest, OverlaydResponse};

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
    ContainerProcessor, DebugOptions, Devices, GpuAssignment, GpuAssignmentMode,
    GpuAssignmentRequest, GuestCrashReporting, GuestOs as HcsGuestOs, HvSocket2,
    HvSocketServiceConfig, HvSocketSystemConfig, ProcessParameters, RegistryChanges, RegistryHive,
    RegistryKey, RegistryValue, RegistryValueType, SchemaVersion, ScsiAttachment, ScsiController,
    Statistics, Storage as HcsStorage, Topology, TopologyMemory, TopologyProcessor, Uefi,
    UefiBootEntry, VirtualMachine, VirtualSmb, VirtualSmbShare, VirtualSmbShareOptions,
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

/// Name of the per-daemon HCN overlay network on the host. Every
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
    /// of the per-daemon HCN overlay network so two daemons
    /// running side-by-side on one host never collide. Defaults to
    /// `"zlayer"` for backward compatibility with single-instance installs.
    pub daemon_name: String,
    /// Daemon data directory (`--data-dir`). Used to locate the managed-network
    /// marker file (`{data_dir}/agent_network.json`) so the HCN overlay network
    /// is reused across restarts and torn down only on a full uninstall.
    pub data_dir: PathBuf,
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
            data_dir: dirs.data_dir().to_path_buf(),
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
    /// Overlay attachment created for this container by `zlayer-overlayd`.
    /// `None` if overlayd was unavailable at [`HcsRuntime::create_container`]
    /// time — we still let the container start, but it won't have networking.
    /// overlayd owns the underlying HCN endpoint + namespace lifecycle; the
    /// agent only records the identifiers it needs to embed in the compute
    /// document and to drive detach.
    overlay_attach: Option<WindowsOverlayAttach>,
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
    /// Connected GCS bridge into this container's hosting UVM. `Some` only
    /// for [`IsolationMode::Hyperv`] entries; `None` for Process-isolated
    /// entries (which have no UVM to bridge into). Created during
    /// [`HcsRuntime::create_container`] after the UVM is started; held here
    /// so a future patch can route container lifecycle RPCs (start/shutdown,
    /// exec, hot-attach) through the in-guest GCS rather than through host-
    /// side HCS APIs.
    ///
    /// Currently the entry is recorded but lifecycle routing through the
    /// bridge is NOT YET wired — `start_container` / `stop_container` still
    /// drive the host-side `ComputeSystem` handle. That swap lives in the
    /// next patch (B4.3); see TODO at the bottom of the Hyper-V branch in
    /// `create_container`.
    #[cfg(feature = "hcs-runtime")]
    #[allow(dead_code)] // read by B4.3 lifecycle routing
    gcs: Option<zlayer_gcs::bridge::GcsBridge>,
}

/// Overlay attachment identifiers returned by `zlayer-overlayd` for a Windows
/// container. overlayd created (and owns the lifetime of) the HCN endpoint +
/// per-container namespace on its HCN Internal network; the agent records only
/// what it needs to (a) embed the namespace GUID in the compute-system document
/// and (b) drive `DetachContainer` on removal.
#[derive(Debug, Clone)]
struct WindowsOverlayAttach {
    /// Bare-lowercase HCN namespace GUID overlayd created for this container.
    /// Embedded into `Container.Networking.Namespace`.
    namespace_guid: String,
    /// The overlay IP overlayd assigned to the container's endpoint.
    ip: IpAddr,
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
    /// IPC client to `zlayer-overlayd`, which owns all HCN network/endpoint/
    /// namespace mechanics. `None` when overlayd could not be reached at
    /// construction time — container creation then fails loudly (a Windows
    /// container with no overlay attach has no addressable network surface).
    /// `Mutex` serializes request/response round-trips on the single framed
    /// connection.
    overlayd: Option<Arc<Mutex<OverlaydClient>>>,
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
    /// `.2` and never collides with the overlay network's gateway. `Mutex`
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
        let data_dir = config.data_dir.clone();
        let mut runtime = Self::new_with_registry(config, registry)?;

        // Connect to overlayd, which owns all HCN network/endpoint/namespace
        // mechanics. Best-effort with backoff so the agent can come up while
        // overlayd is still binding its socket; a hard failure here is surfaced
        // later (per-container) as a CreateFailed rather than blocking startup.
        let socket = zlayer_paths::ZLayerDirs::default_overlayd_socket_path_for(&data_dir);
        match OverlaydClient::connect_with_backoff(std::path::Path::new(&socket)).await {
            Ok(client) => {
                runtime.overlayd = Some(Arc::new(Mutex::new(client)));
            }
            Err(e) => {
                tracing::warn!(
                    socket = %socket,
                    error = %e,
                    "could not connect to overlayd; Windows containers will fail to attach overlay networking until it is reachable"
                );
            }
        }

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
        // `.2` and never collide with the overlay network's default-route
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
            overlayd: None,
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

    /// Ask overlayd to attach a Windows container to the overlay: overlayd
    /// ensures the HCN Internal network exists, creates the per-container
    /// endpoint + namespace at `ip`, and returns the bare-lowercase namespace
    /// GUID for the agent to embed in the compute-system document.
    ///
    /// # Errors
    /// Returns an error when overlayd is unreachable or the attach fails.
    async fn overlayd_attach_windows(
        &self,
        container_id: &str,
        service: &str,
        ip: IpAddr,
        dns_server: Option<IpAddr>,
        dns_domain: Option<String>,
    ) -> Result<WindowsOverlayAttach> {
        let client = self.overlayd.as_ref().ok_or_else(|| {
            AgentError::Network("overlayd is not connected; cannot attach overlay".to_string())
        })?;
        let resp = {
            let mut conn = client.lock().await;
            conn.call(OverlaydRequest::AttachContainer {
                handle: AttachHandle::WindowsContainer {
                    container_id: container_id.to_string(),
                    ip: Some(ip),
                },
                // The real service name (the identity the deploy/scheduler path
                // uses), threaded from `ContainerId::service` at the call site.
                // Required for overlayd to (a) place a Dedicated service's
                // container on its own per-service HCN network, and (b) tag the
                // attachment correctly even in Shared mode.
                service: service.to_string(),
                join_global: false,
                dns_server,
                dns_domain,
            })
            .await
            .map_err(|e| AgentError::Network(format!("overlayd AttachContainer failed: {e}")))?
        };
        match resp {
            OverlaydResponse::Attached(result) => Ok(WindowsOverlayAttach {
                namespace_guid: result.namespace_guid.unwrap_or_default(),
                ip: result.ip,
            }),
            other => Err(AgentError::Network(format!(
                "overlayd AttachContainer returned unexpected response: {other:?}"
            ))),
        }
    }

    /// Ask overlayd to detach (and reap the HCN endpoint + namespace for) a
    /// Windows container previously attached via [`Self::overlayd_attach_windows`].
    /// Best-effort: logs on failure rather than propagating.
    async fn overlayd_detach_windows(&self, namespace_guid: &str) {
        let Some(client) = self.overlayd.as_ref() else {
            return;
        };
        let mut conn = client.lock().await;
        if let Err(e) = conn
            .call(OverlaydRequest::DetachContainer {
                handle: AttachHandle::WindowsContainer {
                    container_id: namespace_guid.to_string(),
                    ip: None,
                },
            })
            .await
        {
            tracing::warn!(ns = %namespace_guid, error = %e, "overlayd DetachContainer failed");
        }
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

    /// Reconcile orphan HCN endpoints.
    ///
    /// As of the overlayd migration, HCN endpoint + namespace lifecycle is owned
    /// entirely by `zlayer-overlayd`: it creates them on `AttachContainer` and
    /// reaps them on `DetachContainer` / its own periodic orphan sweep. The
    /// agent must NOT also enumerate-and-delete HCN endpoints, or it would race
    /// overlayd and tear down endpoints for live containers it doesn't track.
    /// This is therefore a no-op retained for ABI parity with the startup
    /// reconcile call in [`HcsRuntime::new`].
    ///
    /// # Errors
    /// Infallible.
    #[allow(clippy::unused_async)]
    pub async fn reconcile_orphans(&self) -> Result<()> {
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

    /// Orchestrate the Hyper-V-isolated boot of a container via the GCS
    /// bridge. Returns the host-side UVM `ComputeSystem` handle (so the
    /// caller can subscribe to UVM exit events / drive lifecycle) plus the
    /// connected GCS bridge wrapped in `HyperVGcsState` (so it can be
    /// stashed on the container's `ContainerEntry` for later RPCs).
    ///
    /// The flow mirrors hcsshim's `internal/hcsoci/create.go`:
    ///
    /// 1. Build the UVM-only compute-system doc (via
    ///    [`build_uvm_only_doc`]) and `HcsCreateComputeSystem` it. This
    ///    cold-creates the utility VM with its sandbox VHDX on SCSI LUN 0
    ///    and the image's `UtilityVM\Files` over the `"os"` VSMB share.
    ///
    /// 2. `HcsStartComputeSystem` to power the UVM on. After this the
    ///    in-guest GCS listener is reachable over hvsock at the UVM's
    ///    pre-injected `RuntimeId` GUID.
    ///
    /// 3. Connect the GCS bridge over hvsock and negotiate the protocol
    ///    version. `GcsBridge::listen` binds the host listener before start;
    ///    `PendingGcsBridge::accept` accepts the guest's dial-out and
    ///    negotiates after start.
    ///
    /// 4. Hot-attach each parent layer as a read-only VSMB share via
    ///    `system.add_vsmb` (host-side `HcsModifyComputeSystem` with
    ///    resource-path `VirtualMachine/Devices/VirtualSmb/Shares/N`).
    ///
    /// 5. Hot-attach the container's writable scratch VHDX as a SCSI
    ///    attachment on LUN 1 (LUN 0 is already taken by the UVM's
    ///    sandbox).
    ///
    /// 6. Issue `RpcModifySettings` over the bridge to drive
    ///    `CombineLayersWCOW` inside the guest — combining the just-added
    ///    VSMB read-only layers and the SCSI scratch into a single WCIFS
    ///    root that the hosted container can use as its `Storage.Path`.
    ///
    /// 7. Build the hosted-container body (via
    ///    [`build_hosted_container_doc`]) and `RpcCreate` it over the
    ///    bridge with the container's HCS id as the RPC container id.
    ///
    /// 8. `RpcStart` the hosted container to actually launch the workload.
    ///
    /// All in-guest paths used in step 6 and step 7 are placeholders today
    /// — hcsshim derives them from the actual guest VSMB/SCSI mount paths
    /// returned by the in-guest `MountVSMB` / `AttachSCSI` responses,
    /// which we do not yet round-trip through the bridge. The placeholders
    /// produce a well-formed JSON document so HCS accepts the RPCs; real
    /// mount-path discovery is tracked for a follow-up patch (see TODO in
    /// the body for the exact spots that need replacement).
    #[cfg(all(target_os = "windows", feature = "hcs-runtime"))]
    #[allow(clippy::too_many_lines, clippy::too_many_arguments)]
    async fn hyperv_create_via_gcs(
        &self,
        hcs_id: &str,
        spec: &ServiceSpec,
        scratch_layer: &scratch::WritableLayer,
        parent_layers: &[zlayer_hcs::schema::Layer],
        namespace_strs: &[String],
        uvm: &Uvm,
        network_attachment: Option<&WindowsOverlayAttach>,
    ) -> Result<(ComputeSystem, HyperVGcsState)> {
        use uuid::Uuid;
        use zlayer_gcs::bridge::GcsBridge;
        use zlayer_gcs::diagnostics::ts_us;
        use zlayer_gcs::frame::RpcMessageType;
        use zlayer_gcs::protocol::{
            CreateRequest, CreateResponse, ModifySettingsRequest, ModifySettingsResponse,
            RequestBase, StartRequest, StartResponse,
        };

        // Mirror every Hyper-V step transition to stderr alongside
        // `gcs-bridge-send` / `gcs-bridge-reader` so the test stdout has a
        // single monotonically-timestamped timeline of host-side activity
        // (the cargo test harness doesn't init a tracing subscriber, so
        // `tracing::info!` alone is invisible). Sharing
        // [`zlayer_gcs::diagnostics::ts_us`]'s epoch keeps these aligned
        // with the bridge log microsecond-for-microsecond.
        macro_rules! step_log {
            ($($arg:tt)*) => {{
                eprintln!(
                    "[t=+{}us] hcs_id={hcs_id} Hyper-V step: {}",
                    ts_us(),
                    format_args!($($arg)*),
                );
            }};
        }

        // GPU adapter probe — Hyper-V isolation supports GPU-PV when the
        // workload requests it. Mirrors the gating in
        // `build_compute_system_doc` so the UVM-only doc carries the
        // same GpuAssignment block.
        let gpu_adapters: Vec<HostGpuAdapter> = if let Some(gpu_spec) = spec.resources.gpu.as_ref()
        {
            if matches!(gpu_spec.sharing, Some(zlayer_spec::GpuSharingMode::Mps)) {
                return Err(AgentError::GpuSharingUnavailable {
                    mode: "mps".to_string(),
                    reason: "MPS is not supported with Hyper-V isolation; use Process \
                                 isolation or remove the sharing config"
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

        // ----------------------------------------------------------------
        // Step 1: build the UVM-only compute-system doc.
        // ----------------------------------------------------------------
        let owner = owner_tag(&self.config.daemon_name);
        let uvm_doc = build_uvm_only_doc(&owner, hcs_id, uvm, parent_layers, spec, &gpu_adapters);
        let uvm_doc_json =
            serde_json::to_string(&uvm_doc).map_err(|e| AgentError::CreateFailed {
                id: hcs_id.to_string(),
                reason: format!("Hyper-V step 1: serialize UVM doc: {e}"),
            })?;
        tracing::error!(
            target: "zlayer_agent::hcs::diag",
            hcs_id = %hcs_id,
            doc = %uvm_doc_json,
            "HCS_CREATE_UVM_DOC"
        );
        if let Ok(dir) = std::env::var("ZLAYER_HCS_DOC_DUMP_DIR") {
            let path = std::path::PathBuf::from(&dir).join(format!("{hcs_id}.uvm.json"));
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(&path, &uvm_doc_json);
        }

        // ----------------------------------------------------------------
        // Step 1b: HcsGrantVmAccess on every parent layer dir.
        //
        // Grant the UVM's per-VM virtual SID read access to every parent
        // layer dir surfaced via VSMB into the guest. Same root cause as
        // the sandbox VHDX grant in `Uvm::create`: without these the VM's
        // effective identity has no ACL on host-owned files and the UVM
        // fails to start. hcsshim does this in `internal/uvm/vsmb.go::Add`
        // when each VSMB share is added; we batch here since we know the
        // full parent set up front.
        // ----------------------------------------------------------------
        tracing::info!(
            hcs_id = %hcs_id,
            parents = parent_layers.len(),
            "Hyper-V step 1b: HcsGrantVmAccess on parent layer dirs",
        );
        step_log!(
            "1b: HcsGrantVmAccess on parent layer dirs ({} parents)",
            parent_layers.len()
        );
        for layer in parent_layers {
            crate::windows::wclayer::grant_vm_access(
                uvm.runtime_id(),
                std::path::Path::new(&layer.path),
            )
            .map_err(|e| AgentError::CreateFailed {
                id: hcs_id.to_string(),
                reason: format!(
                    "Hyper-V step 1b: HcsGrantVmAccess(parent layer {}): {e}",
                    layer.path
                ),
            })?;
        }

        // ----------------------------------------------------------------
        // Step 2: HcsCreateComputeSystem for the UVM.
        //
        // The UVM's compute-system ID handed to HCS must be the runtime GUID
        // (bare lowercase form). HCS uses this ID as the hvsock VM ID for
        // routing GCS connections — mirrors hcsshim's `internal/uvm/create_wcow.go`
        // where `uvm.id` (the runtime GUID) is passed as the first arg to
        // `HcsCreateComputeSystem`. The runtime GUID is NEVER serialized into
        // the compute-system JSON document itself.
        //
        // `hcs_id` (the human slug like `pair-a-…-rep-1`) stays as the
        // in-process key for `ContainerEntry` indexing — it is unrelated to
        // the string handed to HCS for the UVM.
        // ----------------------------------------------------------------
        let uvm_system_id = format_guid_bare(uvm.runtime_id());
        tracing::info!(
            hcs_id = %hcs_id,
            uvm_system_id = %uvm_system_id,
            "Hyper-V step 2: HcsCreateComputeSystem (UVM)"
        );
        step_log!("2: HcsCreateComputeSystem (UVM) uvm_system_id={uvm_system_id}");
        // windows-debug diagnostics that require offline edits to the UVM's BCD
        // (the `{default}` Windows Boot Loader). The BCD lives on the read-only
        // "os" VSMB source dir but is host-writable before HcsCreate.
        //   * ZLAYER_GCS_BOOTLOG=1 → `bootlog Yes`: kernel writes
        //     `\Windows\ntbtlog.txt` (driver-load log) onto the scratch; read it
        //     offline after a never-dial run.
        //   * ZLAYER_GCS_KD=1 → `debug Yes`: enable the kernel debugger over the
        //     COM1 transport the BCD `{dbgsettings}` already configures (Serial,
        //     debugport 1, 115200). A `kd` client then attaches to the UVM's COM1
        //     named pipe to observe the user-mode service-start failure
        //     (`mpssvc`/`netsetupsvc`) that bootlog can't see.
        // Both best-effort; failures are logged, not fatal.
        #[cfg(feature = "windows-debug")]
        {
            let bcd = uvm.os_files_dir().join(r"EFI\Microsoft\Boot\BCD");
            let set_bcd = |opt: &str, val: &str| match std::process::Command::new("bcdedit")
                .args([
                    "/store",
                    bcd.to_string_lossy().as_ref(),
                    "/set",
                    "{default}",
                    opt,
                    val,
                ])
                .output()
            {
                Ok(o) => step_log!(
                    "2-bcd (windows-debug): {opt} {val} on {} status={} out={} err={}",
                    bcd.display(),
                    o.status,
                    String::from_utf8_lossy(&o.stdout).trim(),
                    String::from_utf8_lossy(&o.stderr).trim()
                ),
                Err(e) => step_log!("2-bcd (windows-debug): bcdedit {opt} spawn failed: {e}"),
            };
            if std::env::var("ZLAYER_GCS_BOOTLOG").as_deref() == Ok("1") {
                set_bcd("bootlog", "Yes");
            }
            if std::env::var("ZLAYER_GCS_KD").as_deref() == Ok("1") {
                set_bcd("debug", "Yes");
            }
        }
        let uvm_system = ComputeSystem::create(&uvm_system_id, &uvm_doc_json)
            .await
            .map_err(|e| AgentError::CreateFailed {
                id: hcs_id.to_string(),
                reason: format!("Hyper-V step 2: HcsCreateComputeSystem (UVM): {e}"),
            })?;

        // ----------------------------------------------------------------
        // Step 3a: bind the host GCS hvsock listener BEFORE starting the UVM.
        // Windows' model (hcsshim create_wcow.go / start.go): the HOST listens
        // on (runtimeID, GCS service GUID) and the in-guest GCS dials OUT once
        // it boots. The listener must already exist when the UVM powers on, so
        // we bind here — between create and start — and accept after start.
        // ----------------------------------------------------------------
        tracing::info!(
            hcs_id = %hcs_id,
            runtime_id = %format_guid_bare(uvm.runtime_id()),
            "Hyper-V step 3a: GcsBridge::listen (bind host hvsock listener)"
        );
        step_log!(
            "3a: GcsBridge::listen (bind host hvsock listener) runtime_id={}",
            format_guid_bare(uvm.runtime_id())
        );
        // Bind on (uvm.runtime_id, GCS service GUID) — matches hcsshim
        // create_wcow.go:126-142 (`winio.ListenHvsock{VMID: uvm.runtimeID,
        // ServiceID: gcsServiceID}`). A wildcard-VMID variant was tested and
        // produced the same timeout, ruling out a runtime_id≠partition-GUID
        // routing mismatch — the in-guest GCS service is not dialing at all.
        let pending_bridge =
            GcsBridge::listen(uvm.runtime_id())
                .await
                .map_err(|e| AgentError::CreateFailed {
                    id: hcs_id.to_string(),
                    reason: format!("Hyper-V step 3a: GcsBridge::listen: {e}"),
                })?;

        // ----------------------------------------------------------------
        // Step 3a' (windows-debug only): bind the host log-forward hvsock
        // listener BEFORE starting the UVM. hcsshim's create_wcow.go binds
        // `uvm.outputListener = winio.ListenHvsock{VMID: uvm.RuntimeID(),
        // ServiceID: WindowsLoggingHvsockServiceID}` here (between create and
        // start) so the host is listening when the in-guest GCS log-forward
        // service dials out. The guest only STARTS forwarding after we issue
        // the `StartLogForwarding` GCS RPC (done inside the bridge's accept(),
        // post-negotiate, windows-debug gated), but the listener must already
        // exist or the guest's connect fails. The receive loop is spawned just
        // after HcsStart (below); records land in `gcs-forward.log` in the
        // UVM debug dir, which `read_uvm_debug_dump` collects.
        #[cfg(feature = "windows-debug")]
        let log_fwd = {
            tracing::info!(
                hcs_id = %hcs_id,
                runtime_id = %format_guid_bare(uvm.runtime_id()),
                "Hyper-V step 3a' (windows-debug): bind host log-forward hvsock listener"
            );
            step_log!("3a' (windows-debug): bind host log-forward hvsock listener");
            zlayer_gcs::log_forward::LogForwardListener::bind(uvm.runtime_id(), uvm.debug_dir())
                .await
                .map_err(|e| AgentError::CreateFailed {
                    id: hcs_id.to_string(),
                    reason: format!("Hyper-V step 3a': LogForwardListener::bind: {e}"),
                })?
        };

        // ----------------------------------------------------------------
        // Step 3: HcsStartComputeSystem — power the UVM on.
        // ----------------------------------------------------------------
        tracing::info!(hcs_id = %hcs_id, "Hyper-V step 3: HcsStartComputeSystem (UVM)");
        step_log!("3: HcsStartComputeSystem (UVM)");
        uvm_system
            .start("")
            .await
            .map_err(|e| AgentError::CreateFailed {
                id: hcs_id.to_string(),
                reason: format!("Hyper-V step 3: HcsStartComputeSystem (UVM): {e}"),
            })?;

        // ----------------------------------------------------------------
        // Step 3b' (windows-debug only): spawn the host log-forward receive
        // loop now that the UVM is Running. The guest's GCS log-forward
        // service may dial as soon as it boots; the listener is already bound
        // (step 3a'). Records are appended to `gcs-forward.log` in the UVM
        // debug dir and echoed to stderr.
        // ----------------------------------------------------------------
        #[cfg(feature = "windows-debug")]
        log_fwd.spawn();

        // NOTE: there is intentionally NO writable "zlayer-debug" VSMB share.
        // A guest-writable VSMB share is rejected by HCS at hot-attach with
        // PowerOnCold `0x80070057` (E_INVALIDARG) regardless of `options` —
        // confirmed on the box (a pre-accept `add_vsmb(zlayer-debug)` fails
        // create outright). Guest-side diagnostics are exfiltrated instead via
        // GCS log forwarding (steps 3a'/3b'), which streams the guest GCS's own
        // logrus records to a HOST file over the logging hvsock — no
        // guest-writable share required.

        // Diagnostic: the CreateFailed reason string is the only channel the
        // e2e test surfaces, so query the HCS-assigned base properties (carries
        // "RuntimeId") and the GuestConnection property (populated iff HCS is
        // managing an INTERNAL GCS connection) once at start. Folded into the
        // step-4 error below if accept times out. This localizes whether our
        // external listener is bound on the wrong VMID or whether HCS grabbed
        // the GCS internally (the latter would mean GuestConnection is set).
        let diag_base_props = uvm_system
            .properties("{}")
            .await
            .unwrap_or_else(|e| format!("<base-props query failed: {e}>"));
        let diag_guest_conn = uvm_system
            .properties(r#"{"PropertyTypes":["GuestConnection"]}"#)
            .await
            .unwrap_or_else(|e| format!("<guest-conn query failed: {e}>"));

        // ----------------------------------------------------------------
        // Step 4: accept the in-guest GCS's inbound hvsock connection (it
        // dials out after boot) and negotiate the protocol. 120s mirrors
        // hcsshim's GCSConnectionTimeout — the guest GCS can take a while to
        // come up on a cold UVM.
        //
        // While we wait, poll the UVM's HCS state every 5s and log it. If the
        // guest bugchecks / fails to boot, the VM leaves "Running" (e.g. to
        // "Stopped" / "SystemCrashed") — a decisive signal that the GCS never
        // dialed because the guest never came up, vs. a guest that is up but
        // not dialing. The poll borrows `&uvm_system` concurrently with the
        // accept future via `select!`.
        // ----------------------------------------------------------------
        tracing::info!(
            hcs_id = %hcs_id,
            runtime_id = %format_guid_bare(uvm.runtime_id()),
            "Hyper-V step 4: PendingGcsBridge::accept (await guest GCS dial-out)"
        );
        step_log!("4: PendingGcsBridge::accept (await guest GCS dial-out, 120s timeout)");
        // Query the host's real timezone and render it as hcsschema's
        // `TimeZoneInformation` JSON to attach to the cold-start Create's
        // UvmConfig — mirroring hcsshim's default (real-host-TZ) path rather
        // than the all-zero-transition-date UTC constant the bridge falls back
        // to. `None` (a failed Win32 query) makes the bridge use that UTC
        // constant fallback.
        let host_tz = crate::windows::timezone::host_timezone_information();
        if host_tz.is_none() {
            tracing::warn!(
                hcs_id = %hcs_id,
                "Hyper-V step 4: host timezone query returned TIME_ZONE_ID_INVALID; \
                 cold-start Create will fall back to the UTC TimeZoneInformation constant"
            );
        }
        let accept_fut = pending_bridge.accept(std::time::Duration::from_secs(120), host_tz);
        tokio::pin!(accept_fut);
        let mut state_poll = tokio::time::interval(std::time::Duration::from_secs(5));
        let bridge = loop {
            tokio::select! {
                res = &mut accept_fut => {
                    match res {
                        Ok(bridge) => break bridge,
                        Err(e) => {
                            // Read back any files the in-guest `zlayer-dump`
                            // diagnostic service wrote into the writable
                            // VSMB exfil share. Folded into the error so
                            // the e2e test (which only surfaces the
                            // CreateFailed.reason string) carries the
                            // actual root-cause signal.
                            let dump = read_uvm_debug_dump(uvm.debug_dir());
                            // ZLAYER_KEEP_UVM_ON_FAILURE=1 → TERMINATE the
                            // UVM here (don't leave it running). A running
                            // UVM holds the scratch VHDX locked, so it can't
                            // be mounted offline — and since GCS is broken,
                            // a live UVM is useless. Terminating releases the
                            // scratch VHDX lock; the `Uvm` Rust wrapper's
                            // KEEP-aware Drop then PRESERVES the (now-unlocked)
                            // scratch VHDX + temp dir when this function
                            // returns Err below. The operator mounts the
                            // preserved VHDX offline and reads the guest
                            // event logs + any WER crash dump from it.
                            if std::env::var("ZLAYER_KEEP_UVM_ON_FAILURE").as_deref()
                                == Ok("1")
                            {
                                // Best-effort terminate to unlock the scratch
                                // VHDX. This is the SAME compute system the
                                // normal-path cleanup would terminate; the
                                // error return below skips that path, so this
                                // is the only terminate of `uvm_system` on this
                                // branch — no double-terminate. The `Uvm`
                                // wrapper (`uvm`) is NOT consumed here, so its
                                // Drop still runs at scope exit and performs
                                // the KEEP-aware preservation.
                                if let Err(te) = uvm_system.terminate("").await {
                                    tracing::warn!(
                                        hcs_id = %hcs_id,
                                        error = %te,
                                        "ZLAYER_KEEP_UVM_ON_FAILURE=1 — best-effort UVM terminate failed; scratch VHDX may remain locked until the VM is stopped manually",
                                    );
                                }
                                let vhdx = uvm.scratch_vhdx().display().to_string();
                                tracing::warn!(
                                    hcs_id = %hcs_id,
                                    runtime_id = %format_guid_bare(uvm.runtime_id()),
                                    scratch_vhdx = %vhdx,
                                    "ZLAYER_KEEP_UVM_ON_FAILURE=1 — UVM TERMINATED and scratch VHDX PRESERVED for offline inspection",
                                );

                                // The test harness surfaces ONLY eprintln!/
                                // step_log! (stdout.log), not `tracing` — and on
                                // panic it deletes the test's parent temp dir,
                                // nuking the in-place VHDX (why preserve-in-place
                                // failed). So ALSO emit via step_log! AND copy the
                                // scratch VHDX to a stable host dir OUTSIDE the
                                // temp tree before it is reaped. Best-effort: log
                                // on failure, never panic. Copy at most ONE VHDX
                                // per run (skip if the debug dir already has one)
                                // to bound disk cost when several UVMs fail.
                                let debug_root =
                                    std::path::Path::new(r"C:\zlayer-uvm-debug");
                                let already_have_vhdx = std::fs::read_dir(debug_root)
                                    .map(|rd| {
                                        rd.filter_map(Result::ok).any(|e| {
                                            e.path()
                                                .extension()
                                                .is_some_and(|x| x.eq_ignore_ascii_case("vhdx"))
                                        })
                                    })
                                    .unwrap_or(false);
                                if already_have_vhdx {
                                    step_log!(
                                        "ZLAYER_KEEP_UVM_ON_FAILURE=1 — {} already contains a captured *.vhdx; skipping copy of {vhdx}",
                                        debug_root.display(),
                                    );
                                } else if let Err(ce) = std::fs::create_dir_all(debug_root) {
                                    eprintln!(
                                        "ZLAYER_KEEP_UVM_ON_FAILURE=1 — failed to create {}: {ce}; scratch VHDX NOT copied out (in-place copy at {vhdx} will be reaped on panic)",
                                        debug_root.display(),
                                    );
                                } else {
                                    let dest = debug_root
                                        .join(format!("{hcs_id}-scratch.vhdx"));
                                    let dest_str = dest.display().to_string();
                                    match std::fs::copy(uvm.scratch_vhdx(), &dest) {
                                        Ok(_) => step_log!(
                                            "ZLAYER_KEEP_UVM_ON_FAILURE=1 — scratch VHDX COPIED to {dest_str} (survives temp-dir reap). \
                                             Read the guest event log + any WER dump OFFLINE from the COPY:\n\
                                             \x20 Mount-VHD -Path '{dest_str}' -ReadOnly   # note the drive letter, e.g. X:\n\
                                             \x20 Get-WinEvent -Path 'X:\\Windows\\System32\\winevt\\Logs\\Application.evtx' | Select-Object -First 100 | Format-List\n\
                                             \x20 Get-WinEvent -Path 'X:\\Windows\\System32\\winevt\\Logs\\System.evtx'      | Select-Object -First 100 | Format-List\n\
                                             \x20 Get-ChildItem 'X:\\zlayer-dbg\\*.dmp'\n\
                                             \x20 Dismount-VHD -Path '{dest_str}'",
                                        ),
                                        Err(ce) => eprintln!(
                                            "ZLAYER_KEEP_UVM_ON_FAILURE=1 — failed to copy scratch VHDX {vhdx} -> {dest_str}: {ce}; \
                                             fall back to the in-place copy at {vhdx} if it survives",
                                        ),
                                    }
                                }
                            }
                            return Err(AgentError::CreateFailed {
                                id: hcs_id.to_string(),
                                reason: format!(
                                    "Hyper-V step 4: GcsBridge accept: {e} || create_id={uvm_system_id} \
                                     || base_props={diag_base_props} || guest_conn={diag_guest_conn} \
                                     || guest_debug_dump={dump}"
                                ),
                            });
                        }
                    }
                }
                _ = state_poll.tick() => {
                    match uvm_system.properties("{}").await {
                        Ok(props) => tracing::info!(
                            hcs_id = %hcs_id,
                            uvm_props = %props,
                            "Hyper-V step 4: UVM state poll (awaiting guest GCS dial-out)"
                        ),
                        Err(e) => tracing::warn!(
                            hcs_id = %hcs_id,
                            error = %e,
                            "Hyper-V step 4: UVM state poll failed"
                        ),
                    }
                }
            }
        };

        // ----------------------------------------------------------------
        // Step 4b: external-GCS HvSocket setup (hcsshim's
        // configureHvSocketForGCS, start.go:36-59). Sent immediately after the
        // GCS connection is established and BEFORE any container resource ops.
        // It makes the guest GCS set up registry keys required to run
        // containers in the UVM. HCS does this automatically for an internal
        // GCS connection; an external GCS connection (our model) must issue it
        // by hand. It is a guest-routed Update of ResourceType "HvSocket" whose
        // Settings is an HvSocketAddress {LocalAddress: <UVM runtime GUID>,
        // ParentAddress: WindowsGcsHvHostID}.
        // ----------------------------------------------------------------
        tracing::info!(hcs_id = %hcs_id, "Hyper-V step 4b: configureHvSocketForGCS");
        step_log!("4b: configureHvSocketForGCS");
        let hvsocket_setup_req = ModifySettingsRequest {
            base: RequestBase {
                activity_id: Uuid::new_v4(),
                container_id: NULL_GUID_STR.to_string(),
            },
            request: serde_json::json!({
                "GuestRequest": {
                    "ResourceType": "HvSocket",
                    "RequestType": "Update",
                    "Settings": {
                        "LocalAddress": format_guid_bare(uvm.runtime_id()),
                        "ParentAddress": format_guid_bare(
                            zlayer_gcs::transport::WINDOWS_GCS_HV_HOST_ID
                        ),
                    }
                }
            }),
        };
        let hvsocket_setup_resp: ModifySettingsResponse = bridge
            .send_rpc_json(RpcMessageType::ModifySettings, &hvsocket_setup_req)
            .await
            .map_err(|e| AgentError::CreateFailed {
                id: hcs_id.to_string(),
                reason: format!("Hyper-V step 4b: GCS HvSocket setup: {e}"),
            })?;
        if hvsocket_setup_resp.result != 0 {
            let hresult_u32 = u32::from_ne_bytes(hvsocket_setup_resp.result.to_ne_bytes());
            return Err(AgentError::CreateFailed {
                id: hcs_id.to_string(),
                reason: format!(
                    "Hyper-V step 4b: GCS HvSocket setup returned HRESULT 0x{hresult_u32:08x}: {}",
                    hvsocket_setup_resp.error_message,
                ),
            });
        }

        // ----------------------------------------------------------------
        // Step 5: hot-attach each parent layer as a read-only VSMB share
        //         on the UVM. Indices are sequential; guest-side VSMB paths
        //         are `\\?\VMSMB\VSMB-{<vsmb-guid>}\sN`. Parent layers start at
        //         index 0 (there is no longer a writable debug share occupying
        //         index 0 — guest diagnostics use GCS log forwarding instead).
        // ----------------------------------------------------------------
        let vsmb_base_idx = 0usize;
        tracing::info!(
            hcs_id = %hcs_id,
            layer_count = parent_layers.len(),
            vsmb_base_idx,
            "Hyper-V step 5: hot-attach per-layer VSMB shares"
        );
        step_log!(
            "5: hot-attach per-layer VSMB shares ({} layers, base_idx={vsmb_base_idx})",
            parent_layers.len()
        );
        for (offset, layer) in parent_layers.iter().enumerate() {
            let idx = vsmb_base_idx + offset;
            let share = VirtualSmbShare {
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
            };
            uvm_system
                .add_vsmb(idx, &share)
                .await
                .map_err(|e| AgentError::CreateFailed {
                    id: hcs_id.to_string(),
                    reason: format!("Hyper-V step 5: add_vsmb({idx}, {}): {e}", layer.id,),
                })?;
        }

        // ----------------------------------------------------------------
        // Step 6: hot-attach the container's writable scratch VHDX as a
        //         SCSI attachment on LUN 1 (LUN 0 holds the UVM sandbox).
        // ----------------------------------------------------------------
        tracing::info!(
            hcs_id = %hcs_id,
            "Hyper-V step 6: hot-attach scratch VHDX on SCSI LUN 1"
        );
        step_log!("6: hot-attach scratch VHDX on SCSI LUN 1");
        let scratch_vhd = scratch_layer.vhd_mount_path().to_string();
        let scratch_attachment = ScsiAttachment {
            path: scratch_vhd.clone(),
            r#type: "VirtualDisk".to_string(),
            read_only: Some(false),
        };
        uvm_system
            .add_scsi(0, 1, &scratch_attachment)
            .await
            .map_err(|e| AgentError::CreateFailed {
                id: hcs_id.to_string(),
                reason: format!("Hyper-V step 6: add_scsi(0, 1, {scratch_vhd}): {e}"),
            })?;

        // ----------------------------------------------------------------
        // Step 7: drive CombineLayersWCOW inside the guest over the GCS
        //         bridge. The exact `ResourcePath` string hcsshim uses for
        //         this varies between builds — what we send below is the
        //         starting shape derived from hcsshim's Combine handler;
        //         live ETW capture from a Windows host will let us pin the
        //         canonical form. See `crates/zlayer-gcs/src/protocol.rs`
        //         `ModifySettingsRequest` for the wire envelope.
        //
        //         TODO(B4.3): replace the placeholder guest paths below
        //         with the real per-VSMB / per-SCSI guest paths returned
        //         by the in-guest `MountVSMB` / `AttachSCSI` responses.
        //         hcsshim's convention for the placeholders we emit:
        //           - VSMB share index `N` mounts at
        //             `\\?\VMSMB\VSMB-{<vsmb-guid>}\sN`
        //           - SCSI LUN `N` on controller `M` mounts at a guest-
        //             chosen `\\?\Volume{<guid>}\`
        //         The guest will reply with the actual chosen path in a
        //         future protocol round-trip we do not yet implement.
        // ----------------------------------------------------------------
        // hcsshim's well-known VSMB controller GUID — same across all
        // hcsshim builds; lives in `internal/uvm/vsmb.go`.
        let vsmb_controller_guid = "{dcc079ae-60ba-4d07-847c-3493609c0870}";
        // Guest-side VSMB path index `sN` is the host-side VSMB share index
        // used at attach time, so it must include `vsmb_base_idx` (1 under
        // windows-debug, where index 0 is the writable zlayer-debug share; 0
        // otherwise) to stay aligned with the Step 5 hot-attach indices.
        let guest_vsmb_paths: Vec<String> = (0..parent_layers.len())
            .map(|n| {
                let idx = vsmb_base_idx + n;
                format!(r"\\?\VMSMB\VSMB-{vsmb_controller_guid}\s{idx}")
            })
            .collect();
        // Placeholder guest scratch mount + WCIFS root — freshly-allocated
        // GUID per create. Real values come from the in-guest mount
        // responses (see TODO above); a UUID v5 derivation from `hcs_id`
        // would be nicer for byte-stability across restarts but the `uuid`
        // workspace dep is configured `features = ["v4", "serde"]` only
        // and we are not permitted to hand-edit `Cargo.toml`. Random per
        // create yields a well-formed document just the same and is
        // harmless under the placeholder regime — the value gets
        // overwritten by the real mount path once B4.3 wires the round
        // trip with the in-guest `MountVSMB` / `AttachSCSI` responses.
        let placeholder_volume_guid = Uuid::new_v4();
        let guest_scratch_path = format!(r"\\?\Volume{{{placeholder_volume_guid}}}\");
        let guest_root_path = guest_scratch_path.clone();

        tracing::info!(
            hcs_id = %hcs_id,
            guest_root = %guest_root_path,
            "Hyper-V step 7: CombineLayersWCOW via GCS"
        );
        step_log!("7: CombineLayersWCOW via GCS guest_root={guest_root_path}");
        let combine_req = ModifySettingsRequest {
            base: RequestBase {
                activity_id: Uuid::new_v4(),
                // CombineLayersWCOW targets the UVM-scoped setting, not a
                // specific hosted container — the null GUID is hcsshim's
                // convention for "this UVM".
                container_id: NULL_GUID_STR.to_string(),
            },
            request: serde_json::json!({
                "ResourcePath": "Container/WCOWLayerPaths",
                "RequestType": "Add",
                "Settings": {
                    "ContainerRootPath": guest_root_path,
                    "Layers": guest_vsmb_paths,
                    "ScratchPath": guest_scratch_path,
                },
            }),
        };
        let _combine_resp: ModifySettingsResponse = bridge
            .send_rpc_json(RpcMessageType::ModifySettings, &combine_req)
            .await
            .map_err(|e| AgentError::CreateFailed {
                id: hcs_id.to_string(),
                reason: format!("Hyper-V step 7: GCS ModifySettings (CombineLayersWCOW): {e}"),
            })?;

        // ----------------------------------------------------------------
        // Step 7.5: surface the host-side HCN endpoint inside the UVM's
        //           network compartment so the workload can actually use
        //           it. overlayd already created the endpoint on the host
        //           and added it to the per-container HCN namespace, BUT in
        //           Hyper-V isolation the namespace's compartment lives
        //           INSIDE the UVM — the in-guest GCS has to be told
        //           explicitly to attach the endpoint to that compartment.
        //           Mirrors hcsshim's
        //           `internal/hcsoci/network.go::addEndpointsToNS` which
        //           issues `ModifySettings({ResourcePath:"Container/Networks",
        //           RequestType:"Add",Settings:{NamespaceId, EndpointId,
        //           AllocatedIPAddress}})` after CombineLayersWCOW and
        //           before RpcCreate of the hosted container.
        //
        //           Because overlayd owns the endpoint, the agent does not
        //           hold an `endpoint_id` directly: resolve it by opening the
        //           overlayd-created namespace and listing its endpoints
        //           (there is exactly one per overlay attach). The namespace
        //           GUID is the bare-lowercase form overlayd returned.
        //
        // Skipped entirely when there is no overlay attachment (degraded
        // mode — overlayd unreachable, or no slice/IP): the container runs
        // in the UVM with no overlay NIC. The GCS bridge + RpcCreate
        // (steps 8/9) still proceed, so the dial path stays exercisable
        // without a fully-configured overlayd.
        // ----------------------------------------------------------------
        if let Some(network_attachment) = network_attachment {
            let namespace_guid = network_attachment.namespace_guid.clone();
            let ns_id =
                GUID::try_from(namespace_guid.as_str()).map_err(|e| AgentError::CreateFailed {
                    id: hcs_id.to_string(),
                    reason: format!(
                    "Hyper-V step 7.5: overlayd namespace GUID {namespace_guid} unparseable: {e}"
                ),
                })?;
            let endpoint_guid = {
                let ns_for_lookup = ns_id;
                let endpoints = tokio::task::spawn_blocking(move || {
                    zlayer_hns::namespace::Namespace::open(ns_for_lookup)
                        .and_then(|ns| ns.list_endpoints())
                })
                .await
                .map_err(|e| AgentError::CreateFailed {
                    id: hcs_id.to_string(),
                    reason: format!("Hyper-V step 7.5: spawn_blocking join failed: {e}"),
                })?
                .map_err(|e| AgentError::CreateFailed {
                    id: hcs_id.to_string(),
                    reason: format!(
                    "Hyper-V step 7.5: failed to list endpoints for namespace {namespace_guid}: {e}"
                ),
                })?;
                endpoints
                    .into_iter()
                    .next()
                    .ok_or_else(|| AgentError::CreateFailed {
                        id: hcs_id.to_string(),
                        reason: format!(
                        "Hyper-V step 7.5: overlayd namespace {namespace_guid} has no endpoints"
                    ),
                    })?
            };
            // `list_endpoints` returns brace/case-normalized GUID strings; reduce
            // to the bare-lowercase form HCS expects in the compartment-attach doc.
            let endpoint_id_bare = endpoint_guid
                .trim_matches(|c: char| c == '{' || c == '}')
                .to_ascii_lowercase();
            tracing::info!(
                hcs_id = %hcs_id,
                endpoint_id = %endpoint_id_bare,
                namespace_id = %namespace_guid,
                "Hyper-V step 7.5: wiring HCN endpoint into UVM network compartment"
            );
            step_log!(
            "7.5: wiring HCN endpoint into UVM network compartment endpoint_id={} namespace_id={}",
            endpoint_id_bare,
            namespace_guid,
        );
            let add_endpoint_req = ModifySettingsRequest {
                base: RequestBase {
                    activity_id: Uuid::new_v4(),
                    container_id: hcs_id.to_string(),
                },
                request: serde_json::json!({
                    "ResourcePath": "Container/Networks",
                    "RequestType": "Add",
                    "Settings": {
                        "NamespaceId": namespace_guid,
                        "EndpointId": endpoint_id_bare,
                        "AllocatedIPAddress": network_attachment.ip.to_string(),
                    },
                }),
            };
            let add_endpoint_resp: ModifySettingsResponse = bridge
                .send_rpc_json(RpcMessageType::ModifySettings, &add_endpoint_req)
                .await
                .map_err(|e| AgentError::CreateFailed {
                    id: hcs_id.to_string(),
                    reason: format!("Hyper-V step 7.5 (network compartment attach): {e}"),
                })?;
            if add_endpoint_resp.result != 0 {
                // Reinterpret the HRESULT bit pattern as u32 for `{:08x}` printing
                // (HRESULT severity-bit set ⇒ i32 < 0, but the canonical Windows
                // textual form is the 8-hex-digit unsigned value, e.g. 0x80070057).
                let hresult_u32 = u32::from_ne_bytes(add_endpoint_resp.result.to_ne_bytes());
                return Err(AgentError::CreateFailed {
                    id: hcs_id.to_string(),
                    reason: format!(
                        "Hyper-V step 7.5 (network compartment attach): guest GCS returned \
                     HRESULT 0x{:08x}: {}",
                        hresult_u32, add_endpoint_resp.error_message,
                    ),
                });
            }
        } else {
            step_log!(
                "7.5: skipped network compartment attach — no overlay attachment \
                 (degraded; container has no overlay NIC)"
            );
        }

        // ----------------------------------------------------------------
        // Step 8: build the hosted-container body using guest paths.
        // ----------------------------------------------------------------
        let guest_layers: Vec<zlayer_hcs::schema::Layer> = parent_layers
            .iter()
            .zip(guest_vsmb_paths.iter())
            .map(|(orig, guest_path)| zlayer_hcs::schema::Layer {
                id: orig.id.clone(),
                path: guest_path.clone(),
            })
            .collect();
        let namespace_id = namespace_strs.first().cloned();
        let hosted_doc =
            build_hosted_container_doc(spec, guest_layers, guest_root_path, namespace_id, hcs_id);

        // ----------------------------------------------------------------
        // Step 9: RpcCreate the hosted container over the bridge.
        // ----------------------------------------------------------------
        tracing::info!(hcs_id = %hcs_id, "Hyper-V step 9: GCS RpcCreate (hosted container)");
        step_log!("9: GCS RpcCreate (hosted container)");
        let create_settings =
            serde_json::to_value(&hosted_doc).map_err(|e| AgentError::CreateFailed {
                id: hcs_id.to_string(),
                reason: format!("Hyper-V step 9: serialize hosted-container body: {e}"),
            })?;
        let create_req = CreateRequest {
            base: RequestBase {
                activity_id: Uuid::new_v4(),
                container_id: hcs_id.to_string(),
            },
            container_config: zlayer_gcs::protocol::AnyInString::new(create_settings),
        };
        let _create_resp: CreateResponse = bridge
            .send_rpc_json(RpcMessageType::Create, &create_req)
            .await
            .map_err(|e| AgentError::CreateFailed {
                id: hcs_id.to_string(),
                reason: format!("Hyper-V step 9: GCS RpcCreate: {e}"),
            })?;

        // ----------------------------------------------------------------
        // Step 10: RpcStart the hosted container.
        // ----------------------------------------------------------------
        tracing::info!(hcs_id = %hcs_id, "Hyper-V step 10: GCS RpcStart (hosted container)");
        step_log!("10: GCS RpcStart (hosted container)");
        let start_req = StartRequest {
            base: RequestBase {
                activity_id: Uuid::new_v4(),
                container_id: hcs_id.to_string(),
            },
        };
        let _start_resp: StartResponse = bridge
            .send_rpc_json(RpcMessageType::Start, &start_req)
            .await
            .map_err(|e| AgentError::CreateFailed {
                id: hcs_id.to_string(),
                reason: format!("Hyper-V step 10: GCS RpcStart: {e}"),
            })?;

        // ----------------------------------------------------------------
        // Step 11: hand back the UVM ComputeSystem + the live bridge.
        //          The caller stashes the bridge on the ContainerEntry so
        //          B4.3 can route lifecycle through it; for now lifecycle
        //          (start/stop) still drives the host-side ComputeSystem
        //          handle. That is harmless on the create path because
        //          we have just RpcStart'd the workload — subsequent
        //          `start_container` calls (idempotent re-arms after a
        //          warm restart) are routed via the host handle, which is
        //          a no-op on an already-started system.
        // ----------------------------------------------------------------
        Ok((
            uvm_system,
            HyperVGcsState {
                bridge: Some(bridge),
            },
        ))
    }
}

/// Stub-out the Hyper-V-via-GCS path when the `hcs-runtime` feature is
/// disabled — the call site in `create_container` is feature-agnostic, so
/// we surface a typed error rather than gating the whole match arm. This
/// keeps the Process-isolation path compiling and testable on Windows
/// without pulling in the `zlayer-gcs` dependency.
#[cfg(all(target_os = "windows", not(feature = "hcs-runtime")))]
impl HcsRuntime {
    // Signature mirrors the real `hcs-runtime` implementation so callers
    // compile identically with or without the feature; the stub body neither
    // awaits nor uses its args.
    #[allow(clippy::too_many_arguments, clippy::unused_async)]
    async fn hyperv_create_via_gcs(
        &self,
        hcs_id: &str,
        _spec: &ServiceSpec,
        _scratch_layer: &scratch::WritableLayer,
        _parent_layers: &[zlayer_hcs::schema::Layer],
        _namespace_strs: &[String],
        _uvm: &Uvm,
        _network_attachment: Option<&WindowsOverlayAttach>,
    ) -> Result<(ComputeSystem, HyperVGcsState)> {
        Err(AgentError::Unsupported(format!(
            "Hyper-V isolation requires the `hcs-runtime` cargo feature \
             (zlayer-gcs); rebuild zlayer-agent with --features hcs-runtime \
             or set `isolation: process` for {hcs_id}",
        )))
    }
}

/// HCS-conventional "null" GUID used as the `ContainerId` field on GCS
/// RPCs that target the UVM itself rather than a hosted container (e.g.
/// `CombineLayersWCOW`).
#[cfg(all(target_os = "windows", feature = "hcs-runtime"))]
const NULL_GUID_STR: &str = "00000000-0000-0000-0000-000000000000";

/// Per-Hyper-V-create state returned by [`HcsRuntime::hyperv_create_via_gcs`]
/// alongside the UVM's [`ComputeSystem`] handle. Process-isolated creates
/// produce an empty default value via [`HyperVGcsState::default`].
///
/// The struct exists so [`HcsRuntime::create_container`] can use one
/// expression to bind both halves of the create result without a tuple of
/// `Option`s in the Process path. The `bridge` field is only populated for
/// successful Hyper-V creates.
#[derive(Default)]
struct HyperVGcsState {
    /// Live GCS bridge into the freshly-booted UVM. Populated by the
    /// Hyper-V path; always `None` for the Process path.
    #[cfg(feature = "hcs-runtime")]
    bridge: Option<zlayer_gcs::bridge::GcsBridge>,
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

/// `prot.WindowsLoggingHvsockServiceID` from hcsshim
/// (`internal/gcs/prot/protocol.go`). Re-added after ground-truth ETW capture of
/// containerd-shim-runhcs-v1's actual `HcsCreateComputeSystem` wire bytes
/// confirmed runhcs.exe DOES emit this entry (with `AllowWildcardBinds=true` +
/// BA/SY bind+connect SDs). Prior revert was based on misreading
/// `create_wcow.go` (the doc entry is guarded by `LogForwardingEnabled` BUT
/// the resulting wire bytes still include it — runhcs runs with log
/// forwarding on by default).
const WINDOWS_LOGGING_HVSOCK_SERVICE_ID: &str = "172dad59-976d-45f2-8b6c-6d1b13f2ac4d";

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
/// Encode a `REG_MULTI_SZ` payload (UTF-16LE, null-terminated strings, final
/// extra null terminator) as base64 — the wire format HCS expects in a
/// `RegistryValue.BinaryValue` field when `Type=MultiString`. Used to
/// override `gcs.DependOnService` so SCM does not block waiting for network
/// services that never come up in a NIC-less UVM.
///
/// Used by `build_uvm_registry_changes` for the `gcs.DependOnService`
/// override.
fn encode_multi_sz_utf16le(strs: &[&str]) -> String {
    use base64::Engine;
    let mut bytes = Vec::<u8>::new();
    for s in strs {
        for c in s.encode_utf16() {
            bytes.extend_from_slice(&c.to_le_bytes());
        }
        // Null-terminate this string.
        bytes.extend_from_slice(&[0, 0]);
    }
    // Final extra null terminator marks end-of-list.
    bytes.extend_from_slice(&[0, 0]);
    base64::engine::general_purpose::STANDARD.encode(&bytes)
}

/// Read the writable `zlayer-debug` VSMB share's host-side backing directory
/// after a step-4 accept timeout and produce a compact one-line summary of
/// what the in-guest `zlayer-dump` diagnostic service wrote there.
///
/// Each file is truncated to the first ~2KB so the resulting string stays
/// embeddable in `AgentError::CreateFailed { reason }` (which is what the
/// integration test surfaces — there is no separate log channel from the
/// runtime to the test harness). UTF-8 invalid bytes are replaced (the
/// guest writes ASCII output from `sc.exe` / `wevtutil` / `dir` / `tasklist`,
/// but defensive against encoding surprises).
///
/// Returns `(empty)` when the dir is absent (UVM never started) or empty
/// (guest never reached the SCM auto-start phase). Returns `(no-files)`
/// when the dir exists but contains zero files — also a signal: SCM didn't
/// kick off our injected service even though the guest reached user-mode.
#[cfg(feature = "hcs-runtime")]
fn read_uvm_debug_dump(debug_dir: &std::path::Path) -> String {
    /// Per-file truncation cap, in bytes. 2KB × ~7 expected files = ~14KB
    /// total dump fold-in, well below any reasonable error-message ceiling.
    const PER_FILE_CAP: usize = 2 * 1024;

    let Ok(entries) = std::fs::read_dir(debug_dir) else {
        return "(empty)".to_string();
    };
    let mut files: Vec<std::path::PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .collect();
    if files.is_empty() {
        return "(no-files)".to_string();
    }
    files.sort(); // deterministic order across runs

    let mut out = String::new();
    for path in &files {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "?".to_string());
        // Crash artifacts (saved-state / kernel dump) are large binaries — do
        // NOT slurp them into the error. Note their size; they're fetched
        // separately and analyzed with cdb on the host.
        let is_binary_artifact = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("vmrs") || e.eq_ignore_ascii_case("dmp"));
        let body = if is_binary_artifact {
            match std::fs::metadata(path) {
                Ok(m) => format!("<binary crash artifact, {} bytes>", m.len()),
                Err(e) => format!("<binary crash artifact, stat error: {e}>"),
            }
        } else {
            // Bounded read — never pull more than PER_FILE_CAP+1 bytes into
            // memory regardless of on-disk size.
            match read_prefix(path, PER_FILE_CAP + 1) {
                Ok(bytes) => {
                    let truncated = bytes.len() > PER_FILE_CAP;
                    let cap = bytes.len().min(PER_FILE_CAP);
                    let body = String::from_utf8_lossy(&bytes[..cap])
                        .replace(['\n', '\r'], " ")
                        .trim()
                        .to_string();
                    if truncated {
                        format!("{body}…<truncated>")
                    } else {
                        body
                    }
                }
                Err(e) => format!("<read error: {e}>"),
            }
        };
        if !out.is_empty() {
            out.push_str(" || ");
        }
        out.push_str(&name);
        out.push('=');
        out.push_str(&body);
    }
    out
}

/// Read at most `max` bytes from the start of `path` without loading the whole
/// file. Used by [`read_uvm_debug_dump`] so a multi-gigabyte crash artifact
/// that happens to land in the debug dir can never OOM the read-back path.
#[cfg(feature = "hcs-runtime")]
fn read_prefix(path: &std::path::Path, max: usize) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut buf = vec![0u8; max];
    let mut filled = 0usize;
    while filled < max {
        let n = f.read(&mut buf[filled..])?;
        if n == 0 {
            break;
        }
        filled += n;
    }
    buf.truncate(filled);
    Ok(buf)
}

/// Build the offline `RegistryChanges` HCS applies to the guest hives before
/// first boot.
///
/// Two purposes:
///
/// 1. **hcsshim parity** — the `gns` `EnableCompartmentNamespace=1` networking
///    workaround hcsshim sets on every WCOW UVM (`prepareCommonConfigDoc` in
///    `create_wcow.go`).
///
/// 2. **gcs `DependOnService` override** — strip `mpssvc`/`netsetupsvc` from
///    `gcs`'s SCM dependency chain so the GCS service starts in a UVM that
///    has no virtual NIC. See the comment block on that entry for rationale.
///
/// (B-4 dropped the prior `zlayer-dump` SCM service and WER `LocalDumps`
/// registry entries because both wrote to the now-removed writable
/// `zlayer-debug` VSMB share. The host-side HCS ETW capture from
/// `scripts/windows/launch_e2e.ps1` is the replacement diagnostic channel
/// while we bisect the 0xEF cause.)
fn build_uvm_registry_changes() -> Vec<RegistryValue> {
    let mut values = vec![
        // hcsshim parity: gns EnableCompartmentNamespace=1.
        RegistryValue {
            key: Some(RegistryKey {
                hive: Some(RegistryHive::System),
                name: r"CurrentControlSet\Services\gns".to_string(),
            }),
            name: "EnableCompartmentNamespace".to_string(),
            r#type: Some(RegistryValueType::DWord),
            d_word_value: Some(1),
            ..Default::default()
        },
    ];

    // gcs `DependOnService` override (handoff 2026-05-31 §4). Strip
    // `mpssvc`/`netsetupsvc` from gcs's SCM dependency chain so the GCS service
    // starts in a UVM that has NO virtual NIC. This is the ONE thing we do that
    // hcsshim never does (hcsshim ships the unmodified inbox UVM). It fixed the
    // never-dial bug but the cold-start `RpcCreate` handler then faults — so the
    // override may have MASKED the real never-dial cause rather than fixing it.
    // A/B toggle: set `ZLAYER_GCS_STOCK_DEPS=1` to OMIT the override and use the
    // inbox gcs's stock dependency chain (hcsshim parity) — used to test whether
    // the strip itself is the Create-fault culprit. Encoded as REG_MULTI_SZ
    // (UTF-16LE, base64).
    let stock_deps = std::env::var("ZLAYER_GCS_STOCK_DEPS").as_deref() == Ok("1");
    if stock_deps {
        tracing::warn!(
            "ZLAYER_GCS_STOCK_DEPS=1 — omitting the gcs DependOnService override; \
             using the inbox gcs stock dependency chain (hcsshim parity)"
        );
    } else {
        values.push(RegistryValue {
            key: Some(RegistryKey {
                hive: Some(RegistryHive::System),
                name: r"CurrentControlSet\Services\gcs".to_string(),
            }),
            name: "DependOnService".to_string(),
            r#type: Some(RegistryValueType::MultiString),
            binary_value: encode_multi_sz_utf16le(&["condrv", "hvsocketcontrol"]),
            ..Default::default()
        });
    }

    // windows-debug: append WER LocalDumps for vmcomputeagent.exe, pointing
    // its full-crash `DumpFolder` at the guest-local scratch path
    // `C:\zlayer-dbg`. WER auto-creates the folder; the dump persists on the
    // preserved scratch VHDX for offline inspection. Compiled out entirely in
    // normal builds.
    #[cfg(feature = "windows-debug")]
    values.extend(build_uvm_debug_registry_changes());

    values
}

/// (`windows-debug` only) Offline registry changes that arm Windows Error
/// Reporting so that, if `vmcomputeagent.exe` (the in-guest GCS host process)
/// fully crashes during the cold-start `RpcCreate`/HvSocket handshake, a full
/// crash dump lands on the guest's writable scratch volume (`C:\`) where the
/// operator can read it offline after the failed UVM is terminated and the
/// scratch VHDX is mounted host-side.
///
/// **WER `LocalDumps` for `vmcomputeagent.exe`.** If gcs.exe actually crashes
/// during cold-start `Create`, Windows Error Reporting writes a full
/// (`DumpType=2`) minidump into the configured `DumpFolder`. WER auto-creates
/// the folder, so we point it at a guest-local path on the scratch volume
/// (`C:\zlayer-dbg`) rather than any VSMB share — there is no guest-writable
/// VSMB share (HCS rejects one at hot-attach), and the scratch VHDX persists
/// after a failure under `ZLAYER_KEEP_UVM_ON_FAILURE=1`, so the dump survives
/// for offline inspection. Lives in the `Software` hive at
/// `Microsoft\Windows\Windows Error Reporting\LocalDumps\vmcomputeagent.exe`.
///
/// (The prior `zlayer-dbg` SCM service was removed: it wrote `sc`/`wevtutil`
/// output to a now-dead VSMB share. The replacement diagnostic path reads the
/// guest's raw `.evtx` event logs offline from the preserved scratch VHDX
/// (`C:\Windows\System32\winevt\Logs\{Application,System}.evtx`), which needs
/// no in-guest service.)
#[cfg(feature = "windows-debug")]
fn build_uvm_debug_registry_changes() -> Vec<RegistryValue> {
    // Guest-local path on the writable scratch volume (guest `C:\`). WER
    // auto-creates this folder before writing the dump. The scratch VHDX is
    // preserved + unlocked after a cold-start failure (KEEP mode terminates
    // the UVM, releasing the disk lock), so the operator can mount it offline
    // and read `C:\zlayer-dbg\*.dmp`.
    let dump_folder = r"C:\zlayer-dbg".to_string();

    vec![
        // --- WER LocalDumps for vmcomputeagent.exe ---
        RegistryValue {
            key: Some(RegistryKey {
                hive: Some(RegistryHive::Software),
                name: r"Microsoft\Windows\Windows Error Reporting\LocalDumps\vmcomputeagent.exe"
                    .to_string(),
            }),
            name: "DumpFolder".to_string(),
            r#type: Some(RegistryValueType::ExpandedString),
            string_value: dump_folder,
            ..Default::default()
        },
        RegistryValue {
            key: Some(RegistryKey {
                hive: Some(RegistryHive::Software),
                name: r"Microsoft\Windows\Windows Error Reporting\LocalDumps\vmcomputeagent.exe"
                    .to_string(),
            }),
            name: "DumpType".to_string(),
            r#type: Some(RegistryValueType::DWord),
            // 2 = full dump.
            d_word_value: Some(2),
            ..Default::default()
        },
        RegistryValue {
            key: Some(RegistryKey {
                hive: Some(RegistryHive::Software),
                name: r"Microsoft\Windows\Windows Error Reporting\LocalDumps\vmcomputeagent.exe"
                    .to_string(),
            }),
            name: "DumpCount".to_string(),
            r#type: Some(RegistryValueType::DWord),
            d_word_value: Some(5),
            ..Default::default()
        },
    ]
}

#[allow(clippy::too_many_lines)] // construction is sequential by HCS field; splitting hurts readability
fn build_virtual_machine_doc(
    uvm: &Uvm,
    // Retained in the signature so callers + tests stay stable, but no
    // longer materialised into the UVM-create-time `VirtualSmb.Shares`
    // array — see the share-construction comment below.
    _parent_layers: &[zlayer_hcs::schema::Layer],
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

    // VirtualSMB shares at UVM-create time: ONLY the `"os"` share that
    // exposes the image's `UtilityVM\Files` directory as the UVM's boot
    // volume (UEFI boots from `VmbFs:\EFI\Microsoft\Boot\bootmgfw.efi`).
    //
    // We deliberately do NOT include parent-layer shares here, nor the
    // writable `"zlayer-debug"` diagnostic exfil share. Hcsshim's
    // `internal/uvm/create_wcow.go` only writes the `os` share at
    // create-time; parent-layer shares are hot-attached AFTER the UVM
    // boots, per hosted container, via `uvm.Add` (`internal/uvm/vsmb.go`).
    // Our `hyperv_create_via_gcs` does those hot-attaches at Step 5 (parent
    // layers) and, under `windows-debug`, the writable `zlayer-debug` share
    // at Step 3c' (post-HcsStart, pre-accept); injecting any of them into the
    // create-time doc poisons gcs.exe's host-compartment init during
    // cold-start `RpcCreate` and is the working hypothesis for the observed
    // 0xEF `CRITICAL_PROCESS_DIED` bugcheck ~0.8 s after Negotiate. See:
    //
    //   `.claude/plans/okay-todo-delme-md-this-is-abstract-kernighan.md`
    //   `BlackLeafDocs/zlayer/windowshcs/gcs-bridge-and-0xEF.md`
    //
    // The `os` options match hcsshim's `DefaultVSMBOptions(true)` from
    // `internal/uvm/vsmb.go` — read-only, share-read, cache-io,
    // pseudo-oplocks, take-backup-privilege.
    // `"os"` share goes FIRST per hcsshim convention (it is the OS root).
    let shares: Vec<VirtualSmbShare> = vec![VirtualSmbShare {
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
    }];
    // B-4: writable `zlayer-debug` VSMB share REMOVED from the
    // create-time doc to test whether it is what kills the in-guest GCS
    // during cold-start RpcCreate. Hcsshim's create-time doc has ONLY the
    // `os` (read-only) share; every parent layer + every additional share
    // is hot-attached AFTER the UVM boots. Our prior `zlayer-debug` share
    // was a session-evidence-only diagnostic — losing it costs us in-guest
    // observability (start.txt, env.txt, dir.txt, sc-*.txt, sys.txt,
    // app.txt) but the host-side HCS ETW + Hyper-V-Worker channels + the
    // bridge wire trace are still live.
    //
    // Original comment for reference (kept while the bisect is in flight):
    //   Mounted in the guest at `\\?\VMSMB\VSMB-{dcc079ae-…}\zlayer-debug`
    //   per hcsshim's `internal/uvm/vsmb.go`. No `options` because
    //   `share_read`/`cache_io`/`take_backup_privilege` on a writable
    //   share triggers HCS `PowerOnCold` HRESULT 0x80070057.
    let virtual_smb = Some(VirtualSmb {
        shares,
        // hcsshim sets DirectFileMappingInMB=1024 on every WCOW UVM VSMB block
        // (`create_wcow.go` prepareCommonConfigDoc). A "sensible default" per
        // their comment; we mirror it for parity.
        direct_file_mapping_in_mb: Some(1024),
    });

    // DebugOptions + GuestCrashReporting removed (2026-05-30 round 2 — second
    // attempt at this revert). Ground-truth ETW capture of
    // containerd-shim-runhcs-v1's actual `HcsCreateComputeSystem` wire bytes
    // shows runhcs emits NEITHER block. Our first attempt to drop them (B-5)
    // produced a never-dial regression, but that test was confounded by
    // COM-pipe additions also being active; the present state has those
    // reverted via Phase H, so re-testing.
    let debug_options: Option<DebugOptions> = None;
    let guest_crash_reporting: Option<GuestCrashReporting> = None;

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
        // hcsshim sets StopOnReset=true on every utility VM so a guest-side
        // reset/bugcheck during boot surfaces as a clean Stop (observable in
        // HCS state) instead of an endless reboot loop.
        stop_on_reset: true,
        // Mirror hcsshim's WCOW `prepareCommonConfigDoc` registry edits. The
        // `gns` `EnableCompartmentNamespace` key is the networking workaround
        // hcsshim applies by default (`!opts.DisableCompartmentNamespace`); it
        // gates a GNS.dll change for SMB-share access inside the UVM. We skip
        // the WER/dump keys (hcsshim only sets those when a dump location is
        // configured, which we don't).
        registry_changes: Some(RegistryChanges {
            add_values: build_uvm_registry_changes(),
        }),
        debug_options,
        chipset: Some(Chipset {
            uefi: Some(Uefi {
                boot_this: Some(UefiBootEntry {
                    device_type: "VmbFs".to_string(),
                    device_path: r"\EFI\Microsoft\Boot\bootmgfw.efi".to_string(),
                    disk_number: None,
                }),
                // NOTE: hcsshim's `internal/uvm/create_wcow.go:313-319` populates
                // `Devices.ComPorts["0"].NamedPipe` for the host-side debug
                // serial console but does NOT set `Uefi.Console = "ComPort1"`.
                // Setting that redirect breaks the WCOW UVM boot in our
                // testing — the UVM enters Running state without ever
                // executing UEFI/bootmgr. Leave Console unset (Default).
                ..Default::default()
            }),
        }),
        compute_topology: Some(Topology {
            memory: Some(TopologyMemory {
                size_in_mb: UVM_DEFAULT_MEMORY_MB,
                // Mirror hcsshim `prepareCommonConfigDoc`: both fields enabled by
                // default on every WCOW UVM. Phase F diff confirmed gcs.exe's
                // reference doc has them; ZLayer was omitting both.
                allow_overcommit: Some(true),
                enable_hot_hint: Some(true),
            }),
            processor: Some(TopologyProcessor {
                count: UVM_DEFAULT_VCPUS,
            }),
        }),
        devices: Some(Devices {
            // COM1 → host named pipe so we can capture UEFI / bootmgr /
            // winload / SMSS / wininit early-boot output that has no other
            // channel before the in-guest GCS comes up and starts logging
            // over hvsock. Mirrors hcsshim `internal/uvm/create_wcow.go`
            // (`doc.VirtualMachine.Devices.ComPorts["0"].NamedPipe`).
            // Pipe name is keyed off the UVM's RuntimeId GUID so concurrent
            // UVMs do not collide on a single pipe. The host-side reader is
            // spawned by `scripts/windows/launch_e2e.ps1` which watches for
            // `\\.\pipe\zlayer-uvm-*-com1` and tees each pipe into
            // `$RunDir\com-zlayer-uvm-<id>-com1.log`.
            com_ports: {
                let mut m = BTreeMap::new();
                m.insert(
                    "0".to_string(),
                    zlayer_hcs::schema::ComPort {
                        named_pipe: format!(
                            r"\\.\pipe\zlayer-uvm-{}-com1",
                            format_guid_bare(uvm.runtime_id())
                        ),
                        optimize_for_debugger: false,
                    },
                );
                m
            },
            scsi,
            virtual_smb,
            gpu,
            // The in-guest GCS only accepts the host's hvsock connection when
            // the UVM authorizes SYSTEM/admin binds. "D:P(A;;FA;;;SY)(A;;FA;;;BA)"
            // = DACL granting Full Access to NT AUTHORITY\SYSTEM (SY) and
            // BUILTIN\Administrators (BA). Mirrors hcsshim create_wcow.go:270-274,
            // which sets ONLY DefaultBindSecurityDescriptor for WCOW (the guest
            // GCS dials OUT, authorized guest-side; DefaultConnectSecurityDescriptor
            // is LCOW-only, where the host dials the guest — verified against the
            // hcsshim tree, and an e2e with the connect SD set still hit the 120s
            // accept timeout with bridge_started=0, confirming it is not the gap).
            hv_socket: Some(HvSocket2 {
                hv_socket_config: Some(HvSocketSystemConfig {
                    default_bind_security_descriptor: "D:P(A;;FA;;;SY)(A;;FA;;;BA)".to_string(),
                    service_table: {
                        let mut table = BTreeMap::new();
                        table.insert(
                            WINDOWS_LOGGING_HVSOCK_SERVICE_ID.to_string(),
                            HvSocketServiceConfig {
                                bind_security_descriptor: "D:P(A;;FA;;;SY)(A;;FA;;;BA)".to_string(),
                                connect_security_descriptor: "D:P(A;;FA;;;SY)(A;;FA;;;BA)"
                                    .to_string(),
                                allow_wildcard_binds: true,
                                ..Default::default()
                            },
                        );
                        table
                    },
                    ..Default::default()
                }),
            }),
            guest_crash_reporting,
        }),
        // VmbFs boot path means HCS persists VM state via the SCSI scratch
        // alone; no host-side `.vmgs` guest-state file is needed (or wanted —
        // supplying one with VmbFs boot triggers an HCS validation error).
        guest_state: None,
        runtime_state_file_path: None,
    }
}

// ---------------------------------------------------------------------------
// Hyper-V two-document builders (B4.1)
//
// The Hyper-V isolation flow needs TWO separate payloads:
//
//   1. A UVM-only `ComputeSystem` doc (`virtual_machine: Some(_)`,
//      `container: None`) handed to `HcsCreateComputeSystem` to boot the
//      utility VM.
//   2. A hosted-container `Container` body (NOT a full ComputeSystem) handed
//      to the GCS bridge's `RpcCreate.settings` field after the UVM is up.
//
// `build_compute_system_doc` above still produces the legacy single-doc form;
// the new builders below are not yet wired into the create-container flow.
// B4.2 will swap callers over to use both helpers in sequence.
// ---------------------------------------------------------------------------

/// Shared `Container.Processor` constructor used by both the legacy single-doc
/// path and the new hosted-container builder. Matches containerd-shim-runhcs-v1
/// behavior: always `Some(_)`, defaulting to `ContainerProcessor::default()`
/// when the spec sets no CPU limits.
//
// `Option<_>` is intentional even though we always return `Some(_)` — the
// HCS schema's `Container.Processor` field is `Option<ContainerProcessor>`
// and callers assign the return value directly into it; collapsing to a bare
// `ContainerProcessor` would force every caller to re-wrap.
#[allow(clippy::unnecessary_wraps)]
fn build_container_processor(spec: &ServiceSpec) -> Option<ContainerProcessor> {
    Some(
        spec.resources
            .cpu
            .and_then(|cpu| {
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
    )
}

/// Shared `Container.Memory` constructor. Returns `None` when the spec sets no
/// memory limit; HCS treats absence as "no memory constraint."
fn build_container_memory(spec: &ServiceSpec) -> Option<ContainerMemory> {
    spec.resources.memory.as_ref().and_then(|mem_str| {
        crate::bundle::parse_memory_string(mem_str)
            .ok()
            .map(|bytes| {
                // HCS takes MiB. Round up so 512Mi → 512, 1.5GiB → 1536, etc.
                let mib = bytes.div_ceil(1024 * 1024);
                ContainerMemory {
                    size_in_mb: Some(mib),
                }
            })
    })
}

/// Build the UVM-only compute-system document for a Hyper-V-isolated
/// container. This is the FIRST `HcsCreateComputeSystem` call in the Hyper-V
/// flow — it boots the UVM. The container itself is created INSIDE the UVM via
/// a subsequent GCS `RpcCreate` (see [`build_hosted_container_doc`]).
///
/// `parent_layers` populates the per-layer `VirtualSMB` shares in the UVM doc.
/// `uvm` provides the scratch VHDX + image's `UtilityVM\Files` os-share path.
/// `spec` provides resource constraints (gpu) consulted by
/// [`build_virtual_machine_doc`]; the UVM's own CPU/memory topology stays
/// fixed at [`UVM_DEFAULT_VCPUS`] / [`UVM_DEFAULT_MEMORY_MB`] — per-container
/// limits live on the hosted-container body instead.
#[allow(dead_code)] // B4.2 will wire this into create_container.
fn build_uvm_only_doc(
    owner_tag: &str,
    uvm_id: &str,
    uvm: &Uvm,
    parent_layers: &[zlayer_hcs::schema::Layer],
    spec: &ServiceSpec,
    gpu_adapters: &[HostGpuAdapter],
) -> HcsDoc {
    let vm_doc = build_virtual_machine_doc(uvm, parent_layers, spec, gpu_adapters);
    HcsDoc {
        owner: owner_tag.to_string(),
        schema_version: SchemaVersion::default(),
        hosting_system_id: String::new(),
        container: None,
        virtual_machine: Some(vm_doc),
        should_terminate_on_last_handle_closed: Some(true),
    }
    .apply_service_id(uvm_id)
}

/// Build the hosted-container body for a Hyper-V-isolated container. This is
/// the SECOND payload — it's a [`HcsContainer`] body, fed to the GCS bridge's
/// `RpcCreate` `settings` field (NOT to `HcsCreateComputeSystem`).
///
/// IMPORTANT: the layer paths inside this Container body are GUEST paths
/// (in-UVM `\\?\VMSMB\VSMB-{dcc079ae-...}\sN`), NOT host paths. The caller
/// (B4.2) provides them after `CombineLayersWCOW` via GCS resolves the per-VSMB
/// guest path. This builder accepts a `Vec<Layer>` of guest layer paths
/// verbatim and a `guest_root_volume` (the guest-side prepared-volume path)
/// for `Storage.Path`.
///
/// `namespace_id` attaches the container to a single HCN namespace inside the
/// guest; `None` means no network — `Networking` is omitted entirely, matching
/// the legacy single-doc path.
#[allow(dead_code)] // B4.2 will wire this into create_container.
fn build_hosted_container_doc(
    spec: &ServiceSpec,
    guest_layers: Vec<zlayer_hcs::schema::Layer>,
    guest_root_volume: String,
    namespace_id: Option<String>,
    hcs_id: &str,
) -> HcsContainer {
    let processor = build_container_processor(spec);
    let memory = build_container_memory(spec);
    let storage = HcsStorage {
        layers: guest_layers,
        path: Some(guest_root_volume),
    };
    let networking = namespace_id.map(|ns| zlayer_hcs::schema::ContainerNetworking {
        allow_unqualified_dns_query: Some(true),
        dns_search_list: Vec::new(),
        namespace: Some(ns),
        network_shared_container_name: None,
    });
    let hostname_source = spec.hostname.as_deref().unwrap_or(hcs_id);
    let guest_os = Some(HcsGuestOs {
        host_name: Some(netbios_hostname(hostname_source)),
    });
    HcsContainer {
        guest_os,
        storage: Some(storage),
        networking,
        mapped_directories: Vec::new(),
        mapped_pipes: Vec::new(),
        processor,
        memory,
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

    #[allow(clippy::too_many_lines)]
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
        // Overlay attachment is a hard requirement for an HCS-managed container:
        // without an IP the workload has no addressable network surface, and
        // `get_container_ip` will return `None` which downstream callers (proxy,
        // service discovery, health checks) treat as a permanent fault. The HCN
        // network/endpoint/namespace mechanics now live in overlayd: the agent
        // allocates (or stashes) the IP and asks overlayd to create the endpoint
        // + per-container namespace, receiving back the namespace GUID to embed
        // in the compute-system document. Each failure mode below maps to a
        // distinct `AgentError::CreateFailed`.
        // Overlay attachment is best-effort, restoring the documented intent in
        // the step-3 comment above. The HCN endpoint/namespace now lives in
        // overlayd, so when overlayd is unreachable (standalone runtime, dev
        // box, e2e harness) — or no slice/IP is available — we log and proceed
        // with NO overlay networking rather than failing the create. The
        // container still starts; it simply has no addressable overlay surface
        // until networking is (re)attached. A genuine attach error while
        // overlayd IS connected stays fatal (a real fault, not a missing daemon).
        // This also keeps the Hyper-V GCS-dial path reachable in tests that do
        // not stand up a full overlayd.
        let network_attachment: Option<WindowsOverlayAttach> = match (slice_cidr, allocated_ip) {
            (Some(_slice), Some(ip)) if self.overlayd.is_some() => {
                let (dns_server, dns_domain) = dns_config.unwrap_or((None, None));
                match self
                    .overlayd_attach_windows(&hcs_id, &id.service, ip, dns_server, dns_domain)
                    .await
                {
                    Ok(att) => Some(att),
                    Err(e) => {
                        return Err(AgentError::CreateFailed {
                            id: hcs_id.clone(),
                            reason: format!("overlayd overlay attach failed: {e}"),
                        });
                    }
                }
            }
            (Some(_), Some(_)) => {
                tracing::warn!(
                    hcs_id = %hcs_id,
                    "overlayd not connected; starting container without overlay networking"
                );
                None
            }
            (None, _) => {
                tracing::warn!(
                    hcs_id = %hcs_id,
                    "node has no assigned slice yet; starting container without overlay networking"
                );
                None
            }
            (Some(_), None) => {
                tracing::warn!(
                    hcs_id = %hcs_id,
                    "no overlay IP could be allocated; starting container without overlay networking"
                );
                None
            }
        };

        // 4. Build the compute-system JSON document, populating
        //    `Container.Networking.Namespace` with the namespace GUID overlayd
        //    created. HCS resolves this GUID against HCN during its `Construct`
        //    step; it must be the **bare, lowercase, un-braced** form
        //    (`aabbccdd-...`), which overlayd already returns.
        let namespace_strs: Vec<String> = network_attachment
            .as_ref()
            .map(|a| vec![a.namespace_guid.clone()])
            .unwrap_or_default();
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

        // 5. Create the compute system. The shape of this differs by
        //    isolation mode:
        //
        //    - Process: build the single legacy doc (Container body only) and
        //      hand it to `HcsCreateComputeSystem`. One call, one system.
        //
        //    - Hyper-V: orchestrate the multi-step GCS-bridge boot sequence
        //      (build UVM-only doc → create + start UVM → connect GCS over
        //      hvsock → hot-attach per-layer VSMB + per-container SCSI scratch
        //      → CombineLayersWCOW inside the guest → RpcCreate the hosted
        //      container → RpcStart it). See `hyperv_create_via_gcs` for the
        //      step-by-step.
        //
        //    On failure, tear down the HCN endpoint we created in step 3 —
        //    otherwise the endpoint (and the IP it owns) leaks, and the next
        //    test/deploy attempt that tries to claim the same IP gets
        //    `HCN_E_ADDR_INVALID_OR_RESERVED (0x803b002f)`. Also release the
        //    IP back to the allocator. The parent-layer guard and scratch
        //    layer cleanup are handled by their own Drop / orphan-reconcile
        //    paths.
        let create_result: Result<(ComputeSystem, HyperVGcsState)> = match isolation {
            IsolationMode::Process => {
                let doc = self.build_compute_system_doc(
                    &hcs_id,
                    spec,
                    &scratch_layer,
                    parent_layers.clone(),
                    namespace_strs.clone(),
                    isolation,
                    uvm.as_ref(),
                )?;
                let doc_json =
                    serde_json::to_string(&doc).map_err(|e| AgentError::CreateFailed {
                        id: hcs_id.clone(),
                        reason: format!("serialize ComputeSystem doc: {e}"),
                    })?;
                // Diagnostic: emit the exact JSON we hand to HCS so reproducible
                // E_INVALIDARG failures can be diffed against hcsshim's known-good
                // docs. error! so it shows without RUST_LOG configuration.
                tracing::error!(
                    target: "zlayer_agent::hcs::diag",
                    hcs_id = %hcs_id,
                    doc = %doc_json,
                    "HCS_CREATE_DOC"
                );
                if let Ok(dir) = std::env::var("ZLAYER_HCS_DOC_DUMP_DIR") {
                    let path = std::path::PathBuf::from(&dir).join(format!("{hcs_id}.json"));
                    let _ = std::fs::create_dir_all(&dir);
                    let _ = std::fs::write(&path, &doc_json);
                }
                ComputeSystem::create(&hcs_id, &doc_json)
                    .await
                    .map(|s| (s, HyperVGcsState::default()))
                    .map_err(|e| AgentError::CreateFailed {
                        id: hcs_id.clone(),
                        reason: format!("HcsCreateComputeSystem: {e}"),
                    })
            }
            IsolationMode::Hyperv => {
                let uvm_ref = uvm.as_ref().ok_or_else(|| AgentError::Internal(
                    "Hyper-V isolation selected but no Uvm was provisioned earlier in create_container".to_string(),
                ))?;
                self.hyperv_create_via_gcs(
                    &hcs_id,
                    spec,
                    &scratch_layer,
                    &parent_layers,
                    &namespace_strs,
                    uvm_ref,
                    network_attachment.as_ref(),
                )
                .await
            }
        };

        let (system, hyperv_state) = match create_result {
            Ok(v) => v,
            Err(e) => {
                // Ask overlayd to tear down the HCN endpoint + namespace it
                // created so neither the endpoint nor its IP leaks, then release
                // the IP back to the node allocator.
                if let Some(att) = &network_attachment {
                    self.overlayd_detach_windows(&att.namespace_guid).await;
                    if let Some(alloc) = self.ip_allocator.lock().await.as_mut() {
                        alloc.release(att.ip);
                    }
                }
                return Err(e);
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
            overlay_attach: network_attachment,
            uvm,
            activated_parent_layers,
            // Carry the GCS bridge through to the entry. Hyper-V path
            // populates it via `hyperv_create_via_gcs`; Process path leaves
            // it as `None`. See `ContainerEntry::gcs` for the eventual
            // lifecycle-routing migration (B4.3).
            #[cfg(feature = "hcs-runtime")]
            gcs: hyperv_state.bridge,
        };
        // Suppress "unused" when the `hcs-runtime` feature is off — the
        // state is only consumed via the gated entry field above. The
        // Process branch always produces an empty state, so dropping it
        // here is safe.
        #[cfg(not(feature = "hcs-runtime"))]
        let _ = hyperv_state;
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

        // Tear down the overlay attachment via overlayd, which owns the HCN
        // endpoint + namespace lifecycle. Best-effort: overlayd logs on failure
        // and its own orphan sweep recovers a leaked endpoint. We still release
        // the IP back to the node allocator so a transient overlayd error does
        // not leak the address (release is idempotent / safe for unknown IPs).
        if let Some(attachment) = entry.overlay_attach.take() {
            self.overlayd_detach_windows(&attachment.namespace_guid)
                .await;
            if let Some(alloc) = self.ip_allocator.lock().await.as_mut() {
                alloc.release(attachment.ip);
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
        // overlayd created the namespace and returned its bare-lowercase GUID;
        // parse it back into a `windows::core::GUID` for trait callers.
        Ok(entries.get(&hcs_id).and_then(|e| {
            e.overlay_attach
                .as_ref()
                .and_then(|a| GUID::try_from(a.namespace_guid.as_str()).ok())
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
        Ok(entry.overlay_attach.as_ref().map(|a| a.ip))
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
    /// `guest_state` is omitted entirely — `VmbFs` boot does not use a
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
        let vsmb = devices
            .virtual_smb
            .as_ref()
            .expect("VirtualSmb block populated");
        // hcsshim parity: the UVM-create-time doc carries ONLY the `os`
        // boot-files share. Parent-layer shares are hot-attached AFTER
        // the UVM boots via `hyperv_create_via_gcs` Step 5
        // (`uvm_system.add_vsmb`) — they do NOT appear in the create-time
        // `VirtualSmb.Shares` array. The `zlayer-debug` writable exfil
        // share was also dropped (B-4) — it was triggering the in-guest
        // GCS to critical-process-die during cold-start RpcCreate (the
        // 0xEF bugcheck symptom).
        assert_eq!(
            vsmb.shares.len(),
            1,
            "create-time shares: `os` only; parent layers hot-attached at Step 5; zlayer-debug dropped per B-4",
        );
        assert_eq!(
            vsmb.shares[0].name, "os",
            "`os` boot-files share must be first per hcsshim convention",
        );
        let os_share = &vsmb.shares[0];
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

        // No parent-layer share and no zlayer-debug share should appear in
        // the create-time doc.
        assert!(
            vsmb.shares.iter().all(|s| s.name == "os"),
            "create-time VSMB.Shares must be `os` only; got {:?}",
            vsmb.shares
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>(),
        );

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

    /// Pins the hcsshim `prepareCommonConfigDoc` parity fields the UVM doc must
    /// carry: `StopOnReset`, the `gns` `EnableCompartmentNamespace` registry
    /// edit, and `VirtualSmb.DirectFileMappingInMB`.
    #[test]
    fn build_virtual_machine_doc_sets_hcsshim_parity_fields() {
        use std::path::PathBuf;

        let uvm = Uvm::for_test(
            "parity-container",
            PathBuf::from(r"C:\zlayer\uvms\parity\scratch.vhdx"),
            PathBuf::from(r"C:\zlayer\images\app\UtilityVM\Files"),
        );
        let spec = fixture_spec();
        let vm = build_virtual_machine_doc(&uvm, &[], &spec, &[]);

        assert!(vm.stop_on_reset, "UVM must set StopOnReset like hcsshim");

        let vsmb = vm
            .devices
            .as_ref()
            .and_then(|d| d.virtual_smb.as_ref())
            .expect("VirtualSmb block populated");
        assert_eq!(
            vsmb.direct_file_mapping_in_mb,
            Some(1024),
            "VSMB DirectFileMappingInMB must mirror hcsshim's WCOW default",
        );

        let reg = vm
            .registry_changes
            .as_ref()
            .expect("UVM must carry RegistryChanges for hcsshim parity");
        let gns = reg
            .add_values
            .iter()
            .find(|v| v.name == "EnableCompartmentNamespace")
            .expect("gns EnableCompartmentNamespace key present");
        assert_eq!(gns.d_word_value, Some(1));
        assert_eq!(gns.r#type, Some(RegistryValueType::DWord));
        let key = gns.key.as_ref().expect("registry value carries a key");
        assert_eq!(key.hive, Some(RegistryHive::System));
        assert_eq!(key.name, r"CurrentControlSet\Services\gns");
    }

    /// `build_virtual_machine_doc` against an empty parent chain still
    /// produces a valid SCSI + chipset + topology block; the `virtual_smb`
    /// map carries the `"os"` boot-files share + the writable `"zlayer-debug"`
    /// exfil share. This pins the contract that a zero-layer image
    /// (theoretical edge case) does not panic and still boots.
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
        let vsmb = devices
            .virtual_smb
            .as_ref()
            .expect("VirtualSmb block populated");
        // `os` only (parent layers are hot-attached at Step 5, not at
        // create time; zlayer-debug dropped per B-4). See
        // `build_virtual_machine_doc` comment for rationale.
        assert_eq!(vsmb.shares.len(), 1);
        assert_eq!(vsmb.shares[0].name, "os");
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
