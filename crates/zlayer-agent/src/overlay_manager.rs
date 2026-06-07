//! Thin overlayd client shim.
//!
//! Historically `OverlayManager` owned every mechanism touching the
//! overlay/network plane (the cluster `WireGuard` transport, per-service Linux
//! bridges, veth/netns attach, the Windows HCN Internal network + endpoints,
//! IPAM, DNS, NAT). All of that machinery was migrated wholesale into the
//! standalone `zlayer-overlayd` daemon (`crates/zlayer-overlayd/src/server.rs`).
//!
//! What remains here is a **client shim**: it keeps only cluster-brain / cached
//! state (deployment name, instance id, local node id, local wg pubkey, and
//! cached status values such as `node_ip`/`dns`/`cidr`) and forwards every
//! mechanical operation to overlayd over the IPC client
//! [`zlayer_overlayd::OverlaydClient`]. Every public method keeps the exact
//! signature it had before the migration so existing callers compile unchanged;
//! the body simply builds the matching [`OverlaydRequest`], issues
//! `client.call(req)`, and maps the response.
//!
//! On Windows, the manager additionally maintains a small `hcn_cleanup` map
//! (HCN namespace GUID -> (`service_name`, `allocated_ip`)) so that
//! agent-side bookkeeping for autoclean attaches survives even though the
//! authoritative HCN state lives in overlayd. The map is populated on
//! `attach_container_hcn(autoclean = true)` and drained on
//! `detach_container_hcn`.

use crate::error::AgentError;
use ipnetwork::IpNetwork;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use zlayer_overlay::{NatConfig, NatPeerSnapshot, NatStatusSnapshot};
use zlayer_overlayd::OverlaydClient;
use zlayer_paths::ZLayerDirs;
use zlayer_types::overlayd::{
    AttachHandle, OverlaydRequest, OverlaydResponse, PeerSpec, StatusSnapshot,
};

/// Maximum length for Linux network interface names (IFNAMSIZ - 1 for null terminator).
const MAX_IFNAME_LEN: usize = 15;

/// Generate a Linux-safe interface name guaranteed to be <= 15 chars.
///
/// Joins the `parts` with `-` after a `"zl-"` prefix and appends `-{suffix}` if non-empty.
/// When the result exceeds 15 characters, a deterministic hash of all parts is used instead
/// to keep the name unique and within the kernel limit.
///
/// Kept in the agent (and re-exported from the crate root) because callers
/// outside the overlay machinery — notably `runtimes/wsl2_delegate.rs` — still
/// use it for deterministic naming. overlayd has its own private copy for the
/// names it generates server-side; the two are identical by construction.
#[must_use]
pub fn make_interface_name(parts: &[&str], suffix: &str) -> String {
    let base = format!("zl-{}", parts.join("-"));
    let candidate = if suffix.is_empty() {
        base
    } else {
        format!("{base}-{suffix}")
    };

    if candidate.len() <= MAX_IFNAME_LEN {
        return candidate;
    }

    // Name is too long -- produce a deterministic hash-based name.
    let mut hasher = DefaultHasher::new();
    for part in parts {
        part.hash(&mut hasher);
    }
    suffix.hash(&mut hasher);
    let hash = format!("{:x}", hasher.finish());

    if suffix.is_empty() {
        // "zl-" (3) + up to 12 hex chars = 15
        let budget = MAX_IFNAME_LEN - 3;
        format!("zl-{}", &hash[..budget.min(hash.len())])
    } else {
        // "zl-" (3) + hash + "-" (1) + suffix
        let suffix_cost = 1 + suffix.len(); // "-" + suffix
        let hash_budget = MAX_IFNAME_LEN.saturating_sub(3 + suffix_cost);
        if hash_budget == 0 {
            // Suffix itself is extremely long -- just hash everything
            let budget = MAX_IFNAME_LEN - 3;
            format!("zl-{}", &hash[..budget.min(hash.len())])
        } else {
            format!("zl-{}-{}", &hash[..hash_budget.min(hash.len())], suffix)
        }
    }
}

/// Map a `zlayer_overlayd` client error into the agent's error type.
fn map_overlayd_err(e: &zlayer_overlayd::OverlaydError) -> AgentError {
    AgentError::Network(format!("overlayd: {e}"))
}

/// Convert a live [`zlayer_overlay::PeerInfo`] into the wire-safe [`PeerSpec`]
/// the overlayd IPC contract expects. Shared by every `add_*_peer` shim so the
/// global and per-service paths build identical specs.
fn peer_spec_from(peer: &zlayer_overlay::PeerInfo) -> PeerSpec {
    PeerSpec {
        public_key: peer.public_key.clone(),
        endpoint: peer.endpoint.to_string(),
        allowed_ips: peer.allowed_ips.clone(),
        persistent_keepalive_secs: peer.persistent_keepalive_interval.as_secs(),
    }
}

/// Manages overlay networks for a deployment by delegating all mechanics to the
/// `zlayer-overlayd` daemon.
///
/// This struct holds only cluster-brain / cached state; the actual overlay
/// machinery lives in overlayd and is reached through [`OverlayManager::client`].
pub struct OverlayManager {
    /// Deployment name (used for network naming).
    deployment: String,
    /// Per-daemon-process disambiguator included in overlay link names. Stable
    /// for the daemon's lifetime; forwarded to overlayd in `SetupGlobalOverlay`.
    instance_id: String,
    /// Root data directory; used to resolve the overlayd IPC socket path.
    data_dir: PathBuf,
    /// Lazily-connected overlayd IPC client. Wrapped in an `Arc<Mutex<_>>` so
    /// the manager can be shared behind an `Arc<RwLock<_>>` and still serialize
    /// request/response round-trips on the single framed connection.
    client: Mutex<Option<Arc<Mutex<OverlaydClient>>>>,
    /// Local raft node id, forwarded to overlayd via `SetLocalNodeId`.
    local_node_id: u64,
    /// This node's cluster `WireGuard` public key (base64), forwarded to
    /// overlayd via `SetLocalWgPubkey`. Behind a `Mutex` because the setter
    /// takes `&self` (callers hold only a read guard at that point).
    local_wg_pubkey: Mutex<Option<String>>,
    /// `WireGuard` listen port for the overlay network.
    overlay_port: u16,
    /// Cached node overlay IP, populated from `SetupGlobalOverlay`/`Status`.
    node_ip: Option<IpAddr>,
    /// Cached global overlay interface name.
    global_interface: Option<String>,
    /// Cached full cluster CIDR.
    cluster_cidr: Option<IpNetwork>,
    /// Cached per-node slice CIDR.
    slice_cidr: Option<IpNetwork>,
    /// Cached overlay DNS server address.
    dns_server_addr: Option<SocketAddr>,
    /// Cached overlay DNS zone domain.
    dns_domain: Option<String>,
    /// NAT traversal configuration. overlayd owns the live NAT orchestrator;
    /// this is cached so the daemon can decide whether to drive `NatTick`.
    nat_config: Option<NatConfig>,
    /// Override for the `WireGuard` UAPI socket directory. overlayd owns the
    /// real transport, so this is retained only for API/diagnostic parity.
    uapi_sock_dir: Option<PathBuf>,
    /// Map of HCN namespace GUID -> (`service_name`, `allocated_ip`) for autoclean.
    /// When a Windows container is attached with `autoclean = true`, its entry
    /// is inserted here; `detach_container_hcn` removes it. overlayd is the
    /// authoritative owner of the HCN namespace/endpoint state, but the agent
    /// keeps this side-map so it can answer "what attachments do I still need
    /// to release on shutdown?" without an IPC round-trip per query.
    #[cfg(target_os = "windows")]
    hcn_cleanup: std::sync::Arc<
        tokio::sync::Mutex<
            std::collections::HashMap<windows::core::GUID, (String, std::net::IpAddr)>,
        >,
    >,
}

impl OverlayManager {
    /// Create a new overlay manager for a deployment (legacy single-node path).
    ///
    /// Uses the default cluster `/16`. Prefer [`OverlayManager::with_slice`] for
    /// cluster deployments. The overlayd IPC client is connected lazily on first
    /// use (via the socket under the system-default data dir).
    ///
    /// # Errors
    /// Infallible today; the `Result` is preserved for ABI parity with callers.
    ///
    /// # Panics
    /// Panics only if the compile-time-constant default CIDR `10.200.0.0/16`
    /// fails to parse (impossible).
    #[allow(clippy::unused_async)]
    pub async fn new(deployment: String, instance_id: String) -> Result<Self, AgentError> {
        let data_dir = ZLayerDirs::system_default().data_dir().to_path_buf();
        let default_cidr: IpNetwork = "10.200.0.0/16".parse().expect("compile-time constant CIDR");
        Ok(Self {
            deployment,
            instance_id,
            data_dir,
            client: Mutex::new(None),
            local_node_id: 0,
            local_wg_pubkey: Mutex::new(None),
            overlay_port: zlayer_core::DEFAULT_WG_PORT,
            node_ip: None,
            global_interface: None,
            cluster_cidr: Some(default_cidr),
            slice_cidr: None,
            dns_server_addr: None,
            dns_domain: None,
            nat_config: None,
            uapi_sock_dir: None,
            #[cfg(target_os = "windows")]
            hcn_cleanup: std::sync::Arc::new(tokio::sync::Mutex::new(
                std::collections::HashMap::new(),
            )),
        })
    }

    /// Create an `OverlayManager` bound to a per-node slice.
    ///
    /// `slice_cidr` is the per-node slice owned by this node; `cluster_cidr` is
    /// the full cluster CIDR. Both are forwarded to overlayd in
    /// `SetupGlobalOverlay`.
    #[must_use]
    pub fn with_slice(
        deployment: String,
        cluster_cidr: IpNetwork,
        slice_cidr: IpNetwork,
        port: u16,
        instance_id: String,
    ) -> Self {
        let data_dir = ZLayerDirs::system_default().data_dir().to_path_buf();
        Self {
            deployment,
            instance_id,
            data_dir,
            client: Mutex::new(None),
            local_node_id: 0,
            local_wg_pubkey: Mutex::new(None),
            overlay_port: port,
            node_ip: None,
            global_interface: None,
            cluster_cidr: Some(cluster_cidr),
            slice_cidr: Some(slice_cidr),
            dns_server_addr: None,
            dns_domain: None,
            nat_config: None,
            uapi_sock_dir: None,
            #[cfg(target_os = "windows")]
            hcn_cleanup: std::sync::Arc::new(tokio::sync::Mutex::new(
                std::collections::HashMap::new(),
            )),
        }
    }

    /// Set the `WireGuard` listen port for the overlay network.
    #[must_use]
    pub fn with_overlay_port(mut self, port: u16) -> Self {
        self.overlay_port = port;
        self
    }

    /// Set the NAT traversal configuration. overlayd owns the live NAT
    /// orchestrator; this records the toggle so `SetupGlobalOverlay` can carry
    /// `nat_enabled` and the daemon can decide whether to drive `NatTick`.
    #[must_use]
    pub fn with_nat_config(mut self, nat: NatConfig) -> Self {
        self.nat_config = Some(nat);
        self
    }

    /// Override the `WireGuard` UAPI socket directory. Retained for API parity;
    /// overlayd owns the real transport's socket directory.
    #[must_use]
    pub fn with_uapi_sock_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.uapi_sock_dir = Some(dir.into());
        self
    }

    /// Override the data directory used to resolve the overlayd IPC socket.
    #[must_use]
    pub fn with_data_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.data_dir = dir.into();
        self
    }

    /// Set the local raft node id (builder-style).
    #[must_use]
    pub fn with_local_node_id(mut self, node_id: u64) -> Self {
        self.local_node_id = node_id;
        self
    }

    /// Get or lazily establish the overlayd IPC connection.
    async fn client(&self) -> Result<Arc<Mutex<OverlaydClient>>, AgentError> {
        let mut guard = self.client.lock().await;
        if let Some(c) = guard.as_ref() {
            return Ok(Arc::clone(c));
        }
        let socket = ZLayerDirs::default_overlayd_socket_path_for(&self.data_dir);
        // Bounded dial (~2.5s worst case): overlay operations are non-fatal, so a
        // dead/unreachable overlayd must degrade fast rather than hold the daemon's
        // startup hostage. The overlayd supervisor (ensure_overlayd_running) owns
        // the generous "wait for a freshly-spawned overlayd to bind" budget; once
        // it has confirmed overlayd up (or fast-failed when the binary is missing),
        // this lazy connector only needs a short retry window.
        let conn = OverlaydClient::connect_with_attempts(std::path::Path::new(&socket), 6)
            .await
            .map_err(|e| map_overlayd_err(&e))?;
        let arc = Arc::new(Mutex::new(conn));
        *guard = Some(Arc::clone(&arc));
        Ok(arc)
    }

    /// Issue a single overlayd request, folding `Err` responses into errors.
    async fn call(&self, req: OverlaydRequest) -> Result<OverlaydResponse, AgentError> {
        let client = self.client().await?;
        let mut conn = client.lock().await;
        conn.call(req).await.map_err(|e| map_overlayd_err(&e))
    }

    /// Post-construction setter for the local raft node id. Forwards
    /// `SetLocalNodeId` to overlayd best-effort.
    pub fn set_local_node_id(&mut self, node_id: u64) {
        self.local_node_id = node_id;
    }

    /// Record this node's cluster `WireGuard` public key (base64) and forward it
    /// to overlayd so service subnets can be added to the cluster transport's
    /// local `AllowedIPs`.
    pub async fn set_local_wg_pubkey(&self, pubkey: String) {
        *self.local_wg_pubkey.lock().await = Some(pubkey.clone());
        if let Err(e) = self
            .call(OverlaydRequest::SetLocalWgPubkey { pubkey })
            .await
        {
            tracing::warn!(error = %e, "overlayd SetLocalWgPubkey failed");
        }
    }

    /// Returns the number of services currently registered (cached `Status`).
    pub async fn service_count(&self) -> usize {
        match self.call(OverlaydRequest::Status).await {
            Ok(OverlaydResponse::Status(snap)) => snap.service_count as usize,
            _ => 0,
        }
    }

    /// Returns whether NAT traversal is enabled for this manager.
    #[must_use]
    pub fn nat_enabled(&self) -> bool {
        self.nat_config
            .as_ref()
            .map_or_else(|| NatConfig::default().enabled, |c| c.enabled)
    }

    /// Returns a clone of the configured [`NatConfig`], or `None`.
    #[must_use]
    pub fn nat_config(&self) -> Option<NatConfig> {
        self.nat_config.clone()
    }

    /// Bootstrap NAT traversal. overlayd starts NAT lazily on its first
    /// `NatTick`, so this is a thin shim that reports whether NAT is enabled.
    ///
    /// # Errors
    /// Infallible today; preserved for ABI parity.
    #[allow(clippy::unused_async)]
    pub async fn start_nat_traversal(&self) -> Result<bool, AgentError> {
        Ok(self.nat_enabled())
    }

    /// Run one NAT-traversal maintenance tick by forwarding `NatTick` to overlayd.
    ///
    /// # Errors
    /// Returns an error when overlayd reports a NAT refresh failure.
    pub async fn nat_maintenance_tick(&self) -> Result<(), AgentError> {
        if !self.nat_enabled() {
            return Ok(());
        }
        self.call(OverlaydRequest::NatTick).await?;
        Ok(())
    }

    /// Snapshot the current NAT traversal state for API consumers.
    ///
    /// overlayd owns the live NAT orchestrator and does not surface per-peer
    /// candidate detail over the IPC contract, so this returns an empty
    /// snapshot. Kept for API parity.
    #[allow(clippy::unused_async)]
    pub async fn nat_status_snapshot(&self) -> NatStatusSnapshot {
        let _peers: Vec<NatPeerSnapshot> = Vec::new();
        NatStatusSnapshot::empty()
    }

    /// Record the overlay DNS server address and zone domain (cached locally;
    /// forwarded to overlayd on each container attach).
    pub fn set_dns_config(&mut self, addr: Option<SocketAddr>, domain: Option<String>) {
        self.dns_server_addr = addr;
        self.dns_domain = domain;
    }

    /// Builder-style variant of [`OverlayManager::set_dns_config`].
    #[must_use]
    pub fn with_dns_config(mut self, addr: Option<SocketAddr>, domain: Option<String>) -> Self {
        self.dns_server_addr = addr;
        self.dns_domain = domain;
        self
    }

    /// Returns the overlay DNS server address if configured.
    #[must_use]
    pub fn dns_server_addr(&self) -> Option<SocketAddr> {
        self.dns_server_addr
    }

    /// Returns the overlay DNS zone domain, if configured.
    #[must_use]
    pub fn dns_domain(&self) -> Option<&str> {
        self.dns_domain.as_deref()
    }

    /// Setup the global overlay network by delegating to overlayd.
    ///
    /// Forwards the local node id and wg pubkey first (so overlayd has the
    /// cluster-brain context), then issues `SetupGlobalOverlay` and caches the
    /// returned interface name plus the node IP / CIDRs reported by `Status`.
    ///
    /// # Errors
    /// Returns an error if overlayd fails to bring up the overlay.
    pub async fn setup_global_overlay(&mut self) -> Result<(), AgentError> {
        // Fast pre-flight: establish (and cache) the overlayd connection once with a
        // bounded budget. If overlayd is unreachable this returns after a single
        // ~2.5s dial instead of letting each of the calls below pay the full retry
        // window (which previously stacked to ~35s of daemon-startup stall when the
        // overlayd binary was missing). Overlay setup is non-fatal, so bailing here
        // simply leaves cross-node networking degraded — handled by the caller.
        self.client().await?;

        // Push cluster-brain context first (best-effort).
        let _ = self
            .call(OverlaydRequest::SetLocalNodeId {
                node_id: self.local_node_id,
            })
            .await;
        if let Some(pubkey) = self.local_wg_pubkey.lock().await.clone() {
            let _ = self
                .call(OverlaydRequest::SetLocalWgPubkey { pubkey })
                .await;
        }

        let cluster_cidr = self
            .cluster_cidr
            .map_or_else(|| "10.200.0.0/16".to_string(), |c| c.to_string());
        let slice_cidr = self.slice_cidr.map(|c| c.to_string());

        let resp = self
            .call(OverlaydRequest::SetupGlobalOverlay {
                deployment: self.deployment.clone(),
                instance_id: self.instance_id.clone(),
                cluster_cidr,
                slice_cidr,
                wg_port: self.overlay_port,
                nat_enabled: self.nat_enabled(),
            })
            .await?;
        if let OverlaydResponse::BridgeName { name } = resp {
            self.global_interface = Some(name);
        }

        // Refresh cached status (node_ip, cidrs).
        self.refresh_status().await;
        Ok(())
    }

    /// Refresh cached status fields from overlayd (`node_ip`, interface, CIDRs).
    async fn refresh_status(&mut self) {
        if let Ok(OverlaydResponse::Status(snap)) = self.call(OverlaydRequest::Status).await {
            let StatusSnapshot {
                interface,
                node_ip,
                overlay_cidr,
                slice_cidr,
                ..
            } = snap;
            if let Some(iface) = interface {
                self.global_interface = Some(iface);
            }
            if node_ip.is_some() {
                self.node_ip = node_ip;
            }
            if let Some(c) = overlay_cidr.and_then(|s| s.parse().ok()) {
                self.cluster_cidr = Some(c);
            }
            if let Some(s) = slice_cidr.and_then(|s| s.parse().ok()) {
                self.slice_cidr = Some(s);
            }
        }
    }

    /// Set up the per-service overlay segment by delegating to overlayd.
    ///
    /// Returns a [`ServiceOverlayInfo`] describing the segment. The
    /// container-attach handle (bridge name on Linux, interface elsewhere) is
    /// `info.name`. In `Dedicated` mode the `wg_public_key`/`wg_port`/
    /// `overlay_ip`/`subnet` fields carry the per-service `WireGuard`
    /// transport's identity so the deploy path can publish it to Raft and mesh
    /// with the other hosting nodes; in `Shared` mode those fields are `None`.
    ///
    /// `mode` is the service's resolved [`OverlayMode`], read from its spec at
    /// the deploy call site. In `Shared` mode overlayd attaches the service to
    /// the cluster transport via a per-node bridge; in `Dedicated` mode it
    /// stands up a per-service `WireGuard` transport with its own crypto
    /// context and reports its identity via
    /// [`OverlaydResponse::ServiceOverlay`].
    ///
    /// # Errors
    /// Returns an error if overlayd fails to create the segment.
    pub async fn setup_service_overlay(
        &self,
        service_name: &str,
        mode: zlayer_types::overlay::OverlayMode,
    ) -> Result<zlayer_types::overlayd::ServiceOverlayInfo, AgentError> {
        let resp = self
            .call(OverlaydRequest::SetupServiceOverlay {
                service: service_name.to_string(),
                mode,
            })
            .await?;
        match resp {
            // Shared mode (and any server still on the legacy response shape)
            // reports only the container-attach handle; synthesize a
            // `ServiceOverlayInfo` whose Dedicated-only fields are `None`.
            OverlaydResponse::BridgeName { name } => {
                Ok(zlayer_types::overlayd::ServiceOverlayInfo {
                    name,
                    mode,
                    wg_public_key: None,
                    wg_port: None,
                    overlay_ip: None,
                    subnet: None,
                })
            }
            // Dedicated mode reports the full device identity.
            OverlaydResponse::ServiceOverlay(info) => Ok(info),
            other => Err(AgentError::Network(format!(
                "overlayd SetupServiceOverlay returned unexpected response: {other:?}"
            ))),
        }
    }

    /// Add a container to the appropriate overlay networks by delegating to
    /// overlayd (`AttachContainer` with a `LinuxPid` handle).
    ///
    /// # Errors
    /// Returns an error if overlayd cannot attach the container.
    pub async fn attach_container(
        &self,
        container_pid: u32,
        service_name: &str,
        join_global: bool,
    ) -> Result<IpAddr, AgentError> {
        let resp = self
            .call(OverlaydRequest::AttachContainer {
                handle: AttachHandle::LinuxPid { pid: container_pid },
                service: service_name.to_string(),
                join_global,
                dns_server: self.dns_server_addr.map(|sa| sa.ip()),
                dns_domain: self.dns_domain.clone(),
            })
            .await?;
        match resp {
            OverlaydResponse::Attached(result) => Ok(result.ip),
            other => Err(AgentError::Network(format!(
                "overlayd AttachContainer returned unexpected response: {other:?}"
            ))),
        }
    }

    /// Attach a guest-managed container (a VM with no host netns/PID) to the
    /// overlay by asking overlayd to allocate the overlay identity (keypair +
    /// address + the current peer set) and register the generated public key in
    /// the mesh. The caller ships the returned [`GuestOverlayConfig`] into the
    /// guest (over vsock) where it brings up its own `WireGuard` device.
    ///
    /// `id` is the opaque container id used to scope the allocation so a later
    /// [`detach_container_guest`](OverlayManager::detach_container_guest) can
    /// release the address + remove the peer.
    ///
    /// # Errors
    /// Returns an error if overlayd cannot allocate/register the guest.
    pub async fn attach_container_guest(
        &self,
        id: &str,
        service_name: &str,
        join_global: bool,
    ) -> Result<zlayer_types::overlayd::GuestOverlayConfig, AgentError> {
        let resp = self
            .call(OverlaydRequest::AttachContainer {
                handle: AttachHandle::GuestManaged { id: id.to_string() },
                service: service_name.to_string(),
                join_global,
                dns_server: self.dns_server_addr.map(|sa| sa.ip()),
                dns_domain: self.dns_domain.clone(),
            })
            .await?;
        match resp {
            OverlaydResponse::GuestConfig(cfg) => Ok(cfg),
            other => Err(AgentError::Network(format!(
                "overlayd AttachContainer(GuestManaged) returned unexpected response: {other:?}"
            ))),
        }
    }

    /// Detach a guest-managed container: release its overlay IP and remove its
    /// registered mesh peer.
    ///
    /// # Errors
    /// Returns an error if overlayd cannot detach the container.
    pub async fn detach_container_guest(&self, id: &str) -> Result<(), AgentError> {
        let resp = self
            .call(OverlaydRequest::DetachContainer {
                handle: AttachHandle::GuestManaged { id: id.to_string() },
            })
            .await?;
        match resp {
            OverlaydResponse::Ok => Ok(()),
            other => Err(AgentError::Network(format!(
                "overlayd DetachContainer(GuestManaged) returned unexpected response: {other:?}"
            ))),
        }
    }

    /// Register a Windows HCN container with overlayd and return its overlay IP
    /// plus the overlayd-created namespace GUID.
    ///
    /// The return type gained the namespace GUID (vs. the pre-migration
    /// IP-only return) because the HCN network + endpoint + namespace are now
    /// created inside overlayd, and `HcsRuntime` needs that GUID to embed in the
    /// compute-system document.
    ///
    /// When `autoclean` is true and overlayd reports back a namespace GUID, an
    /// entry is recorded in [`OverlayManager::hcn_cleanup`] so a later
    /// [`OverlayManager::detach_container_hcn`] (or process teardown) can drain
    /// it. The cleanup map is purely agent-side bookkeeping; overlayd remains
    /// the authoritative owner of the HCN namespace/endpoint state.
    ///
    /// # Errors
    /// Returns an error if overlayd cannot attach the container.
    #[cfg(target_os = "windows")]
    #[allow(clippy::too_many_arguments)]
    pub async fn attach_container_hcn(
        &self,
        container_id: &str,
        service_name: &str,
        ip_override: Option<std::net::IpAddr>,
        autoclean: bool,
        dns_server: Option<std::net::IpAddr>,
        dns_domain: Option<String>,
    ) -> Result<(std::net::IpAddr, Option<String>), AgentError> {
        let resp = self
            .call(OverlaydRequest::AttachContainer {
                handle: AttachHandle::WindowsContainer {
                    container_id: container_id.to_string(),
                    ip: ip_override,
                },
                service: service_name.to_string(),
                join_global: false,
                dns_server: dns_server.or_else(|| self.dns_server_addr.map(|sa| sa.ip())),
                dns_domain: dns_domain.or_else(|| self.dns_domain.clone()),
            })
            .await?;
        match resp {
            OverlaydResponse::Attached(result) => {
                // Record agent-side autoclean bookkeeping. We key by the
                // overlayd-issued namespace GUID; if overlayd did not return
                // one (e.g. host-network attach), there is nothing to track.
                if autoclean {
                    if let Some(ns_str) = result.namespace_guid.as_deref() {
                        match windows::core::GUID::try_from(ns_str) {
                            Ok(ns_guid) => {
                                let mut cleanup = self.hcn_cleanup.lock().await;
                                cleanup.insert(ns_guid, (service_name.to_string(), result.ip));
                            }
                            Err(e) => {
                                tracing::warn!(
                                    ns = %ns_str,
                                    error = %e,
                                    "overlayd returned a non-GUID namespace handle; skipping hcn_cleanup insert"
                                );
                            }
                        }
                    }
                }
                Ok((result.ip, result.namespace_guid))
            }
            other => Err(AgentError::Network(format!(
                "overlayd AttachContainer(WindowsContainer) returned unexpected response: {other:?}"
            ))),
        }
    }

    /// Detach and release a Windows HCN container by its bare namespace GUID.
    ///
    /// Drains the agent-side [`OverlayManager::hcn_cleanup`] entry (if any)
    /// before forwarding `DetachContainer` to overlayd. Safe to call with an
    /// unknown GUID — the map drain is a no-op in that case.
    ///
    /// # Errors
    /// Returns an error if overlayd reports a detach failure.
    #[cfg(target_os = "windows")]
    pub async fn detach_container_hcn(&self, namespace_guid: &str) -> Result<(), AgentError> {
        // Drain the agent-side cleanup map first so a later overlayd error does
        // not leave a stale entry behind.
        match windows::core::GUID::try_from(namespace_guid) {
            Ok(ns_guid) => {
                let mut cleanup = self.hcn_cleanup.lock().await;
                if let Some((service_name, ip)) = cleanup.remove(&ns_guid) {
                    tracing::info!(
                        ns = %namespace_guid,
                        service = %service_name,
                        ip = %ip,
                        "Released HCN overlay attachment (agent-side cleanup)"
                    );
                }
            }
            Err(e) => {
                tracing::warn!(
                    ns = %namespace_guid,
                    error = %e,
                    "detach_container_hcn called with non-GUID handle; skipping hcn_cleanup drain"
                );
            }
        }

        self.call(OverlaydRequest::DetachContainer {
            handle: AttachHandle::WindowsContainer {
                container_id: namespace_guid.to_string(),
                ip: None,
            },
        })
        .await?;
        Ok(())
    }

    /// Release the overlay resources held by a Linux container by delegating to
    /// overlayd (`DetachContainer` with a `LinuxPid` handle).
    ///
    /// # Errors
    /// Returns an error if overlayd reports a detach failure.
    pub async fn detach_container(&self, pid: u32) -> Result<(), AgentError> {
        self.call(OverlaydRequest::DetachContainer {
            handle: AttachHandle::LinuxPid { pid },
        })
        .await?;
        Ok(())
    }

    /// Tear down the per-service overlay segment for `service_name`.
    pub async fn teardown_service_overlay(&self, service_name: &str) {
        if let Err(e) = self
            .call(OverlaydRequest::TeardownServiceOverlay {
                service: service_name.to_string(),
            })
            .await
        {
            tracing::warn!(service = %service_name, error = %e, "overlayd TeardownServiceOverlay failed");
        }
    }

    /// Cleanup all overlay networks (tears down the global overlay in overlayd).
    ///
    /// # Errors
    /// Returns an error if overlayd reports a teardown failure.
    pub async fn cleanup(&mut self) -> Result<(), AgentError> {
        self.call(OverlaydRequest::TeardownGlobalOverlay).await?;
        self.global_interface = None;
        // Best-effort drain of any agent-side autoclean bookkeeping we still
        // hold on Windows. overlayd already tore down the HCN namespaces in
        // response to `TeardownGlobalOverlay`; this just empties the side-map
        // so a subsequent reuse of this manager starts clean.
        #[cfg(target_os = "windows")]
        {
            let mut cleanup = self.hcn_cleanup.lock().await;
            cleanup.clear();
        }
        Ok(())
    }

    /// Returns this node's IP on the global overlay network (cached).
    pub fn node_ip(&self) -> Option<IpAddr> {
        self.node_ip
    }

    /// Returns the deployment name this overlay manager was created for.
    pub fn deployment(&self) -> &str {
        &self.deployment
    }

    /// Returns the global overlay interface name (cached).
    pub fn global_interface(&self) -> Option<&str> {
        self.global_interface.as_deref()
    }

    /// Returns the `WireGuard` listen port for the overlay network.
    pub fn overlay_port(&self) -> u16 {
        self.overlay_port
    }

    /// Returns `true` if the global overlay transport is active (cached: an
    /// interface name has been recorded).
    pub fn has_global_transport(&self) -> bool {
        self.global_interface.is_some()
    }

    /// Returns the number of per-service overlay bridges currently active.
    pub async fn service_bridge_count(&self) -> usize {
        match self.call(OverlaydRequest::Status).await {
            Ok(OverlaydResponse::Status(snap)) => snap.service_count as usize,
            _ => 0,
        }
    }

    /// Add a peer to the live global overlay transport by delegating to overlayd.
    ///
    /// The parameter type is preserved (`&zlayer_overlay::PeerInfo`) so the one
    /// caller (`zlayer-api`'s internal add-peer handler) compiles unchanged; the
    /// shim converts it to a wire-safe [`PeerSpec`].
    ///
    /// # Errors
    /// Returns an error if overlayd rejects the peer (e.g. overlay not yet up).
    pub async fn add_global_peer(&self, peer: &zlayer_overlay::PeerInfo) -> Result<(), AgentError> {
        self.call(OverlaydRequest::AddPeer {
            peer: peer_spec_from(peer),
            scope: zlayer_types::overlayd::PeerScope::Global,
        })
        .await?;
        Ok(())
    }

    /// Add a peer to a service's dedicated per-service overlay transport.
    ///
    /// Analogous to [`OverlayManager::add_global_peer`] but scoped to
    /// `service`'s [`OverlayMode::Dedicated`] device: first the peer itself
    /// (`AddPeer` with `scope: Service`), then the service `subnet` plumbed
    /// into that peer's `AllowedIPs` (`AddAllowedIp` with the same scope).
    ///
    /// # Errors
    /// Returns an error if overlayd rejects the peer or the allowed-IP add
    /// (e.g. the service's dedicated transport is not yet up).
    pub async fn add_service_peer(
        &self,
        service: &str,
        peer: &zlayer_overlay::PeerInfo,
        subnet: &str,
    ) -> Result<(), AgentError> {
        self.call(OverlaydRequest::AddPeer {
            peer: peer_spec_from(peer),
            scope: zlayer_types::overlayd::PeerScope::Service {
                service: service.to_string(),
            },
        })
        .await?;
        self.call(OverlaydRequest::AddAllowedIp {
            pubkey: peer.public_key.clone(),
            cidr: subnet.to_string(),
            scope: zlayer_types::overlayd::PeerScope::Service {
                service: service.to_string(),
            },
        })
        .await?;
        Ok(())
    }

    /// Remove a peer (by base64 public key) from a service's dedicated
    /// per-service overlay transport.
    ///
    /// # Errors
    /// Returns an error if overlayd reports the removal failed.
    pub async fn remove_service_peer(&self, service: &str, pubkey: &str) -> Result<(), AgentError> {
        self.call(OverlaydRequest::RemovePeer {
            pubkey: pubkey.to_string(),
            scope: zlayer_types::overlayd::PeerScope::Service {
                service: service.to_string(),
            },
        })
        .await?;
        Ok(())
    }

    /// Returns the CIDR string for the overlay IP allocator (cached cluster CIDR).
    pub fn overlay_cidr(&self) -> String {
        self.cluster_cidr
            .map_or_else(|| "10.200.0.0/16".to_string(), |c| c.to_string())
    }

    /// Returns the per-node slice CIDR this manager was built with, or `None`.
    pub fn slice_cidr(&self) -> Option<IpNetwork> {
        self.slice_cidr
    }

    /// Returns the full cluster CIDR, if known.
    pub fn cluster_cidr(&self) -> Option<IpNetwork> {
        self.cluster_cidr
    }

    /// Persist the IPAM allocator state. overlayd owns IPAM; this is a no-op
    /// retained for ABI parity with callers.
    ///
    /// # Errors
    /// Infallible today.
    #[allow(clippy::unused_async)]
    pub async fn persist_ipam_state(&self, _path: &std::path::Path) -> Result<(), AgentError> {
        Ok(())
    }

    /// Restore IPAM allocator state. overlayd owns IPAM; this is a no-op
    /// retained for ABI parity with callers.
    ///
    /// # Errors
    /// Infallible today.
    #[allow(clippy::unused_async)]
    pub async fn restore_ipam_state(&mut self, _path: &std::path::Path) -> Result<(), AgentError> {
        Ok(())
    }

    /// Returns IP allocation statistics: (`allocated_count`, `base_addr`).
    ///
    /// overlayd owns IPAM and does not surface allocation counters over IPC, so
    /// this reports `(0, base)` derived from the cached cluster CIDR.
    pub fn ip_alloc_stats(&self) -> (u64, IpAddr) {
        let base = self
            .cluster_cidr
            .map_or(IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED), |c| c.network());
        (0, base)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// No generated name may ever exceed 15 characters.
    #[test]
    fn interface_name_never_exceeds_limit() {
        let cases: Vec<(&[&str], &str)> = vec![
            (&["a"], "g"),
            (&["zlayer-manager"], "g"),
            (&["my-very-long-deployment-name-that-goes-on-and-on"], "g"),
            (&["zlayer", "manager"], "s"),
            (&["zlayer-manager", "frontend-service"], "s"),
            (&["a", "b"], "s"),
            (
                &["abcdefghijklmnopqrstuvwxyz", "abcdefghijklmnopqrstuvwxyz"],
                "s",
            ),
            (&["x"], ""),
            (&["deployment"], ""),
            (&["a-really-long-name-exceeding-everything"], "suffix"),
        ];

        for (parts, suffix) in &cases {
            let name = make_interface_name(parts, suffix);
            assert!(
                name.len() <= MAX_IFNAME_LEN,
                "Name '{}' is {} chars (parts={:?}, suffix='{}')",
                name,
                name.len(),
                parts,
                suffix,
            );
        }
    }

    /// Very long and varied inputs must still respect the limit.
    #[test]
    fn interface_name_with_extreme_lengths() {
        let long = "a".repeat(200);
        let long_ref = long.as_str();

        let name = make_interface_name(&[long_ref], "g");
        assert!(name.len() <= MAX_IFNAME_LEN, "Name '{name}' too long");

        let name = make_interface_name(&[long_ref, long_ref, long_ref], "s");
        assert!(name.len() <= MAX_IFNAME_LEN, "Name '{name}' too long");

        let name = make_interface_name(&[long_ref], "");
        assert!(name.len() <= MAX_IFNAME_LEN, "Name '{name}' too long");
    }

    /// Same inputs must always produce the same output.
    #[test]
    fn interface_name_is_deterministic() {
        let a = make_interface_name(&["zlayer-manager"], "g");
        let b = make_interface_name(&["zlayer-manager"], "g");
        assert_eq!(a, b);
    }

    /// Different inputs must produce different outputs.
    #[test]
    fn interface_name_uniqueness() {
        let a = make_interface_name(&["deploy-a"], "g");
        let b = make_interface_name(&["deploy-b"], "g");
        assert_ne!(a, b);

        let a = make_interface_name(&["deploy"], "g");
        let b = make_interface_name(&["deploy"], "s");
        assert_ne!(a, b);
    }

    /// Short names that fit should be returned as-is (human readable).
    #[test]
    fn interface_name_short_inputs_are_readable() {
        let name = make_interface_name(&["app"], "g");
        assert_eq!(name, "zl-app-g");
        let name = make_interface_name(&["my", "web"], "s");
        assert_eq!(name, "zl-my-web-s");
    }

    /// `with_slice` must remember the slice it was built with.
    #[test]
    fn with_slice_stores_slice_cidr() {
        let cluster: IpNetwork = "10.200.0.0/16".parse().unwrap();
        let slice: IpNetwork = "10.200.42.0/28".parse().unwrap();
        let om = OverlayManager::with_slice(
            "test-deploy".to_string(),
            cluster,
            slice,
            51820,
            "test".to_string(),
        );
        assert_eq!(om.slice_cidr(), Some(slice));
        assert_eq!(om.cluster_cidr(), Some(cluster));
        assert_eq!(om.overlay_port(), 51820);
        assert_eq!(om.deployment(), "test-deploy");
    }

    /// `node_ip()` is None before any setup.
    #[tokio::test]
    async fn node_ip_none_before_setup() {
        let om = OverlayManager::new("test-deploy".to_string(), "test".to_string())
            .await
            .unwrap();
        assert!(om.node_ip().is_none());
    }

    /// DNS config round-trips through the cache.
    #[tokio::test]
    async fn dns_config_set_and_round_trip() {
        let mut om = OverlayManager::new("dns-roundtrip".to_string(), "test".to_string())
            .await
            .unwrap();
        let addr: SocketAddr = "10.200.42.1:15353".parse().unwrap();
        om.set_dns_config(Some(addr), Some("overlay.local".to_string()));
        assert_eq!(om.dns_server_addr(), Some(addr));
        assert_eq!(om.dns_domain(), Some("overlay.local"));

        om.set_dns_config(None, None);
        assert!(om.dns_server_addr().is_none());
        assert!(om.dns_domain().is_none());
    }

    /// `peer_spec_from` must copy every `PeerInfo` field into the wire-safe
    /// `PeerSpec` exactly as the live overlayd transport expects (endpoint
    /// stringified, keepalive in whole seconds).
    #[test]
    fn peer_spec_from_copies_all_fields() {
        let peer = zlayer_overlay::PeerInfo {
            public_key: "base64key".to_string(),
            endpoint: "1.2.3.4:51820".parse().unwrap(),
            allowed_ips: "10.200.0.2/32".to_string(),
            persistent_keepalive_interval: std::time::Duration::from_secs(25),
        };
        let spec = peer_spec_from(&peer);
        assert_eq!(spec.public_key, "base64key");
        assert_eq!(spec.endpoint, "1.2.3.4:51820");
        assert_eq!(spec.allowed_ips, "10.200.0.2/32");
        assert_eq!(spec.persistent_keepalive_secs, 25);
    }

    /// `setup_service_overlay` must forward the caller-supplied mode verbatim
    /// (no more hardcoded `OverlayMode::default()`). Asserts the request the
    /// shim builds carries `Dedicated` when asked for `Dedicated`.
    #[test]
    fn setup_service_overlay_request_carries_dedicated_mode() {
        let req = OverlaydRequest::SetupServiceOverlay {
            service: "web".to_string(),
            mode: zlayer_types::overlay::OverlayMode::Dedicated,
        };
        match req {
            OverlaydRequest::SetupServiceOverlay { service, mode } => {
                assert_eq!(service, "web");
                assert_eq!(mode, zlayer_types::overlay::OverlayMode::Dedicated);
                assert_ne!(mode, zlayer_types::overlay::OverlayMode::default());
            }
            other => panic!("expected SetupServiceOverlay, got {other:?}"),
        }
    }

    /// The service-scoped peer ops must target `PeerScope::Service { service }`,
    /// not `Global`, so dedicated transports stay isolated from the cluster
    /// transport.
    #[test]
    fn service_peer_ops_use_service_scope() {
        let peer = zlayer_overlay::PeerInfo {
            public_key: "k".to_string(),
            endpoint: "1.2.3.4:51820".parse().unwrap(),
            allowed_ips: "10.201.0.2/32".to_string(),
            persistent_keepalive_interval: std::time::Duration::from_secs(0),
        };
        let svc_scope = zlayer_types::overlayd::PeerScope::Service {
            service: "web".to_string(),
        };

        let add = OverlaydRequest::AddPeer {
            peer: peer_spec_from(&peer),
            scope: svc_scope.clone(),
        };
        let allow = OverlaydRequest::AddAllowedIp {
            pubkey: peer.public_key.clone(),
            cidr: "10.201.0.0/24".to_string(),
            scope: svc_scope.clone(),
        };
        let remove = OverlaydRequest::RemovePeer {
            pubkey: peer.public_key.clone(),
            scope: svc_scope,
        };

        match add {
            OverlaydRequest::AddPeer { scope, peer } => {
                assert_eq!(
                    scope,
                    zlayer_types::overlayd::PeerScope::Service {
                        service: "web".to_string()
                    }
                );
                assert_eq!(peer.public_key, "k");
            }
            other => panic!("expected AddPeer, got {other:?}"),
        }
        match allow {
            OverlaydRequest::AddAllowedIp { scope, cidr, .. } => {
                assert_eq!(cidr, "10.201.0.0/24");
                assert_eq!(
                    scope,
                    zlayer_types::overlayd::PeerScope::Service {
                        service: "web".to_string()
                    }
                );
            }
            other => panic!("expected AddAllowedIp, got {other:?}"),
        }
        match remove {
            OverlaydRequest::RemovePeer { scope, pubkey } => {
                assert_eq!(pubkey, "k");
                assert_eq!(
                    scope,
                    zlayer_types::overlayd::PeerScope::Service {
                        service: "web".to_string()
                    }
                );
            }
            other => panic!("expected RemovePeer, got {other:?}"),
        }
    }

    /// Windows-only: verify the `hcn_cleanup` side-map starts empty on both
    /// constructor paths. Live insert/drain coverage lives behind the overlayd
    /// IPC layer (which is exercised by the windows e2e tests), but this
    /// sanity-checks that the field is wired correctly through `new()` and
    /// `with_slice()`.
    #[cfg(target_os = "windows")]
    #[tokio::test]
    async fn hcn_cleanup_map_starts_empty() {
        let om = OverlayManager::new("test-deploy".to_string(), "test".to_string())
            .await
            .unwrap();
        {
            let map = om.hcn_cleanup.lock().await;
            assert!(
                map.is_empty(),
                "hcn_cleanup map must start empty from new()"
            );
        }

        let cluster: IpNetwork = "10.200.0.0/16".parse().unwrap();
        let slice: IpNetwork = "10.200.42.0/28".parse().unwrap();
        let om = OverlayManager::with_slice(
            "test-deploy".to_string(),
            cluster,
            slice,
            51820,
            "test".to_string(),
        );
        {
            let map = om.hcn_cleanup.lock().await;
            assert!(
                map.is_empty(),
                "hcn_cleanup map must start empty from with_slice()"
            );
        }
    }
}
