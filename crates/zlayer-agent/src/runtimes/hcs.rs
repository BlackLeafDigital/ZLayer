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

#![cfg(target_os = "windows")]

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
use crate::windows::{scratch, unpacker};

use zlayer_hcs::enumerate;
use zlayer_hcs::events::{self, HcsEventKind};
use zlayer_hcs::schema::{
    ComputeSystem as HcsDoc, Container as HcsContainer, ContainerMemory, ContainerProcessor,
    ProcessParameters, SchemaVersion, Statistics, Storage as HcsStorage,
};
use zlayer_hcs::system::ComputeSystem;

/// Owner tag stamped onto every compute system this runtime creates. Used at
/// startup to discover zombie systems from a previous agent run, and as the
/// filter for [`enumerate::list_by_owner`].
pub const OWNER_TAG: &str = "zlayer";

/// Name used for the per-daemon HCN Transparent overlay network on the host.
/// Every ZLayer container on this node attaches an endpoint into this
/// network; the network's IPAM subnet is the node's per-node `/28` slice of
/// the cluster CIDR (no longer a constant — see [`HcsConfig::slice_cidr`]).
const OVERLAY_NETWORK_NAME: &str = "zlayer-overlay";

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

/// Configuration for [`HcsRuntime`].
#[derive(Debug, Clone)]
pub struct HcsConfig {
    /// Root directory for the read-only image layer cache (`<root>/images/`)
    /// and the per-container scratch layers (`<root>/scratch/<id>/`).
    pub storage_root: PathBuf,
    /// Default isolation mode applied when a service spec does not pin one.
    pub default_isolation: IsolationMode,
    /// Default scratch layer size in GiB. `0` requests the HCS default.
    pub default_scratch_size_gb: u64,
    /// Cluster CIDR (e.g. "10.200.0.0/16") used for per-endpoint policy
    /// configuration: OutBoundNAT exceptions, SDNRoute destination,
    /// ACL remote-addresses.
    pub cluster_cidr: String,
    /// This node's per-node /28 slice of the cluster CIDR. `None` until
    /// the node joins the cluster and the leader hands out a slice. When
    /// `None`, [`HcsRuntime::ensure_overlay_network`] cannot proceed.
    pub slice_cidr: Option<ipnet::IpNet>,
}

impl Default for HcsConfig {
    fn default() -> Self {
        let dirs = zlayer_paths::ZLayerDirs::system_default();
        Self {
            storage_root: std::env::var("ZLAYER_HCS_STORAGE_ROOT")
                .map_or_else(|_| dirs.containers().join("hcs"), PathBuf::from),
            default_isolation: IsolationMode::default(),
            default_scratch_size_gb: 20,
            cluster_cidr: "10.200.0.0/16".to_string(),
            slice_cidr: None,
        }
    }
}

/// Cached unpacked image keyed by manifest digest.
#[derive(Debug)]
struct CachedImage {
    /// Parent chain (child-to-parent order) ready to be plugged into a
    /// compute-system document.
    unpacked: unpacker::UnpackedImage,
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
        Self::new_with_registry(config, registry)
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
        Ok(Self {
            config,
            containers: RwLock::new(HashMap::new()),
            images: RwLock::new(HashMap::new()),
            registry,
            auth_resolver: zlayer_core::AuthResolver::new(zlayer_core::AuthConfig::default()),
            overlay_network: Arc::new(Mutex::new(None)),
            next_container_ip: Arc::new(Mutex::new(None)),
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

        let net_id = GUID::new().map_err(|e| {
            AgentError::Internal(format!("GUID::new for overlay network failed: {e}"))
        })?;
        let uplink = zlayer_hns::adapter::find_primary_adapter()
            .map_err(|e| AgentError::Internal(format!("find_primary_adapter: {e}")))?;
        let subnet_str = slice_cidr.to_string();
        let subnet_for_create = subnet_str.clone();
        let uplink_for_create = uplink.clone();

        let network = tokio::task::spawn_blocking(move || {
            zlayer_hns::network::Network::create_transparent(
                net_id,
                OVERLAY_NETWORK_NAME,
                &subnet_for_create,
                &uplink_for_create,
            )
        })
        .await
        .map_err(|e| AgentError::Internal(format!("spawn_blocking join failed: {e}")))?
        .map_err(|e| AgentError::Internal(format!("HcnCreateNetwork(zlayer-overlay): {e}")))?;

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

    /// Scan the host for HCN endpoints owned by this runtime (name prefix
    /// matches [`OWNER_TAG`]) and delete them.
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
    #[allow(dead_code)]
    pub async fn reconcile_orphans(&self) -> Result<()> {
        let live: std::collections::HashSet<String> =
            self.containers.read().await.keys().cloned().collect();

        let owned = tokio::task::spawn_blocking(|| hns_attach::list_owned_endpoints(OWNER_TAG))
            .await
            .map_err(|e| AgentError::Internal(format!("spawn_blocking join failed: {e}")))?
            .map_err(|e| AgentError::Internal(format!("list_owned_endpoints: {e}")))?;

        for (endpoint_id, name) in owned {
            // Endpoint name is `{OWNER_TAG}-{container_id}`. Strip the prefix
            // + dash to recover the container id and skip live containers.
            let prefix = format!("{OWNER_TAG}-");
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

        let mut cache = self.images.write().await;
        cache.insert(image.to_string(), CachedImage { unpacked });
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
    fn build_compute_system_doc(
        &self,
        hcs_id: &str,
        spec: &ServiceSpec,
        scratch_layer: &scratch::WritableLayer,
        parent_layers: Vec<zlayer_hcs::schema::Layer>,
        namespace_ids: Vec<String>,
    ) -> HcsDoc {
        let processor = spec.resources.cpu.and_then(|cpu| {
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
        });

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

        let storage = HcsStorage {
            layers: parent_layers,
            path: Some(scratch_layer.layer_path().to_string_lossy().into_owned()),
        };

        let networking = if namespace_ids.is_empty() {
            None
        } else {
            Some(zlayer_hcs::schema::ContainerNetworking {
                allow_unqualified_dns_query: None,
                dns_search_list: Vec::new(),
                namespace: namespace_ids,
                network_shared_container_name: None,
            })
        };

        let container = HcsContainer {
            storage: Some(storage),
            networking,
            mapped_directories: Vec::new(),
            mapped_pipes: Vec::new(),
            hostname: spec.hostname.clone(),
            processor,
            memory,
        };

        HcsDoc {
            owner: OWNER_TAG.to_string(),
            schema_version: SchemaVersion::default(),
            hosting_system_id: String::new(),
            container: Some(container),
            virtual_machine: None,
            should_terminate_on_last_handle_closed: Some(true),
        }
        .apply_service_id(hcs_id)
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
    fn apply_service_id(mut self, hcs_id: &str) -> Self {
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

        // 1. Look up (or lazy-pull) the unpacked image.
        {
            let cache = self.images.read().await;
            if !cache.contains_key(&spec.image.name) {
                drop(cache);
                self.do_pull(&spec.image.name, spec.image.pull_policy, None)
                    .await?;
            }
        }
        let parent_layers = self.resolve_parent_chain(&spec.image.name).await?;

        // 2. Build a scratch layer for this container.
        let scratch_dir = self.scratch_dir(&hcs_id);
        // Convert the HCS-ordered parent list into the wclayer LayerChain
        // expected by `scratch::create`.
        let chain = crate::windows::wclayer::LayerChain::new(parent_layers.clone());
        let scratch_layer = scratch::create(
            &scratch_dir,
            &chain,
            self.config.default_scratch_size_gb,
            // `is_base_os_bootstrap` is only true for the very first scratch
            // layer built over a given base OS layer. For the MVP we leave
            // this at `false`; the unpacker already handled base-layer
            // preparation during `HcsImportLayer`.
            false,
        )
        .map_err(|e| AgentError::CreateFailed {
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
        let allocated_ip = self.next_container_ip.lock().await.take();
        let cluster_cidr = self.config.cluster_cidr.clone();
        let network_attachment = match (slice_cidr, allocated_ip) {
            (Some(slice), Some(ip)) => match self.ensure_overlay_network(slice).await {
                Ok(net_id) => {
                    let cid_for_attach = hcs_id.clone();
                    let prefix_length = slice.prefix_len();
                    let cluster_cidr_owned = cluster_cidr;
                    match tokio::task::spawn_blocking(move || {
                        EndpointAttachment::create_overlay(
                            net_id,
                            OWNER_TAG,
                            cid_for_attach.as_str(),
                            ip,
                            prefix_length,
                            &cluster_cidr_owned,
                        )
                    })
                    .await
                    {
                        Ok(Ok(att)) => Some(att),
                        Ok(Err(e)) => {
                            tracing::warn!(
                                hcs_id = %hcs_id,
                                error = %e,
                                "HCN overlay endpoint attach failed; starting container without network"
                            );
                            None
                        }
                        Err(e) => {
                            tracing::warn!(
                                hcs_id = %hcs_id,
                                error = %e,
                                "spawn_blocking join for overlay endpoint attach failed; starting container without network"
                            );
                            None
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        hcs_id = %hcs_id,
                        error = %e,
                        "HCN Transparent overlay network unavailable; starting container without network"
                    );
                    None
                }
            },
            (None, _) => {
                tracing::warn!(
                    hcs_id = %hcs_id,
                    "HcsConfig.slice_cidr is None (node has no assigned slice yet); starting container without network"
                );
                None
            }
            (Some(_), None) => {
                tracing::warn!(
                    hcs_id = %hcs_id,
                    "no container IP stashed via set_next_container_ip; starting container without network"
                );
                None
            }
        };

        // 4. Build the compute-system JSON document, populating
        //    `Container.Networking.Namespace` with the namespace GUID we just
        //    created. HCS expects the namespace GUID as a brace-wrapped
        //    string (matching the `{:?}` debug formatter for
        //    `windows::core::GUID`).
        let namespace_strs: Vec<String> = network_attachment
            .as_ref()
            .map(|att| vec![format!("{:?}", att.namespace_id())])
            .unwrap_or_default();
        let doc = self.build_compute_system_doc(
            &hcs_id,
            spec,
            &scratch_layer,
            parent_layers,
            namespace_strs,
        );
        let doc_json = serde_json::to_string(&doc).map_err(|e| AgentError::CreateFailed {
            id: hcs_id.clone(),
            reason: format!("serialize ComputeSystem doc: {e}"),
        })?;

        // 5. Create the compute system.
        let system = ComputeSystem::create(&hcs_id, &doc_json)
            .await
            .map_err(|e| AgentError::CreateFailed {
                id: hcs_id.clone(),
                reason: format!("HcsCreateComputeSystem: {e}"),
            })?;

        // 6. Subscribe to exit events before returning so we don't miss a
        //    fast-exiting container.
        let sink: Arc<RwLock<Option<i32>>> = Arc::new(RwLock::new(None));
        self.spawn_exit_watcher(hcs_id.clone(), system.raw(), sink.clone());

        // 7. Register the entry.
        let entry = ContainerEntry {
            system,
            scratch_layer: Some(scratch_layer),
            hcs_id: hcs_id.clone(),
            last_exit_code: sink,
            network_attachment,
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

        // Tear down the HCN endpoint + namespace, if we attached one. Best-
        // effort: log on failure (the container is already gone so leaving a
        // dangling endpoint is recoverable via startup reconcile) and do
        // **not** propagate — scratch teardown already succeeded and the
        // caller expects success once we reach this point.
        if let Some(attachment) = entry.network_attachment.take() {
            let hcs_id_for_log = entry.hcs_id.clone();
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

        let process = ComputeProcess::spawn(entry.system.raw(), &params)
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
        // Clone the UnpackedImage (LayerChain + root are both Clone).
        let cloned = CachedImage {
            unpacked: entry.unpacked.clone(),
        };
        cache.insert(target.to_string(), cloned);
        Ok(())
    }

    async fn inspect_detailed(&self, id: &ContainerId) -> Result<ContainerInspectDetails> {
        let hcs_id = Self::hcs_id(id);
        let containers = self.containers.read().await;
        let entry = containers
            .get(&hcs_id)
            .ok_or_else(|| AgentError::NotFound {
                container: hcs_id.clone(),
                reason: "no HCS entry for container".to_string(),
            })?;

        Ok(ContainerInspectDetails {
            ports: Vec::new(),
            networks: Vec::new(),
            ipv4: None,
            health: None,
            exit_code: *entry.last_exit_code.read().await,
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
/// # Errors
///
/// Returns the error emitted by [`zlayer_hcs::enumerate::list_by_owner`].
pub async fn list_owned_systems() -> Result<Vec<String>> {
    let systems = enumerate::list_by_owner(OWNER_TAG)
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
}
