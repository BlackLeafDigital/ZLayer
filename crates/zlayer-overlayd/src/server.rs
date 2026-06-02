//! The overlayd server engine.
//!
//! [`OverlaydServer`] is a near 1:1 migration of the *mechanics* half of the
//! agent's `OverlayManager`: it owns the single cluster `WireGuard`
//! [`OverlayTransport`], the per-service Linux bridges (Linux) / HCN Internal
//! network + endpoints (Windows), the per-node IP allocator, DNS config, and
//! NAT traversal. The cluster-brain half (Raft, scheduler, service registry)
//! stays in the main daemon, which drives this server over the IPC contract in
//! [`zlayer_types::overlayd`].
//!
//! Every [`OverlaydRequest`] maps to a method here via [`OverlaydServer::handle`].

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
#[cfg(target_os = "linux")]
use std::os::fd::AsFd;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use ipnetwork::IpNetwork;
use zlayer_overlay::{NatConfig, NatTraversal, OverlayConfig, OverlayTransport, PeerInfo};
use zlayer_types::overlayd::{
    AttachHandle, AttachResult, DedicatedServiceStatus, OverlayMode, OverlaydRequest,
    OverlaydResponse, PeerScope, PeerSpec, PeerStatus, ServiceOverlayInfo, StatusSnapshot,
};

use crate::error::OverlaydError;
use crate::network_state::{
    owner_for_service, DedicatedPortAllocator, ManagedNetwork, NetworkState,
};

/// Maximum length for Linux network interface names (IFNAMSIZ - 1 for null terminator).
const MAX_IFNAME_LEN: usize = 15;

/// Generate a Linux-safe interface name guaranteed to be <= 15 chars.
///
/// Joins the `parts` with `-` after a `"zl-"` prefix and appends `-{suffix}` if
/// non-empty. When the result exceeds 15 characters, a deterministic hash of all
/// parts is used instead to keep the name unique and within the kernel limit.
#[must_use]
pub fn make_interface_name(parts: &[&str], suffix: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

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
            let budget = MAX_IFNAME_LEN - 3;
            format!("zl-{}", &hash[..budget.min(hash.len())])
        } else {
            format!("zl-{}-{}", &hash[..hash_budget.min(hash.len())], suffix)
        }
    }
}

/// First usable host address in `subnet`.
///
/// For IPv4 this is `network() + 1` (skipping the network address). For IPv6
/// the same rule applies — the network address is conventionally reserved.
fn first_usable_ip(subnet: ipnet::IpNet) -> IpAddr {
    match subnet {
        ipnet::IpNet::V4(v4) => {
            let net = u32::from(v4.network());
            IpAddr::V4(Ipv4Addr::from(net.wrapping_add(1)))
        }
        ipnet::IpNet::V6(v6) => {
            let net = u128::from(v6.network());
            IpAddr::V6(Ipv6Addr::from(net.wrapping_add(1)))
        }
    }
}

/// Parameters threaded into [`OverlaydServer::attach_to_interface`] when a
/// container is being attached to a per-service Linux bridge.
#[cfg(target_os = "linux")]
#[derive(Debug)]
struct BridgeAttachParams<'a> {
    /// Linux bridge name on the host to enslave the host-side veth into.
    bridge_name: &'a str,
    /// Bridge's L3 gateway IP. The container's default route is set here.
    gateway: IpAddr,
    /// Prefix length of the bridge's subnet.
    subnet_prefix_len: u8,
}

/// Tracking info recorded by [`OverlaydServer::attach_container`] for every
/// container that successfully attaches on Linux. Used by `detach_container`.
#[cfg(target_os = "linux")]
#[derive(Debug, Clone)]
struct AttachInfo {
    /// IP allocated on the per-service overlay (eth0 inside the container).
    service_ip: IpAddr,
    /// Name of the service whose bridge owns `service_ip`.
    service_name: Option<String>,
    /// IP allocated on the global overlay (eth1), if the container joined it.
    global_ip: Option<IpAddr>,
    /// True iff the container also attached to the global overlay (eth1).
    joined_global: bool,
}

/// Per-service Linux bridge state. One bridge per service per node; containers
/// attach to it via veth pairs and cross-node packets ride the single cluster
/// `OverlayTransport` with the service subnet plumbed into its `AllowedIPs`.
#[cfg(target_os = "linux")]
#[derive(Debug)]
struct ServiceBridge {
    /// Linux bridge name, kept under IFNAMSIZ-1 by [`make_interface_name`].
    name: String,
    /// CIDR of the service's subnet on this node.
    subnet: ipnet::IpNet,
    /// Gateway IP within the subnet (first usable address).
    gateway: IpAddr,
    /// Per-service IP allocator covering `subnet`.
    ip_allocator: zlayer_overlay::allocator::IpAllocator,
}

/// A dedicated per-service `WireGuard` transport (`OverlayMode::Dedicated`).
///
/// Unlike Shared mode — where every service subnet is plumbed onto the single
/// cluster [`OverlayTransport`] via multi-CIDR `AllowedIPs` — a Dedicated
/// service owns a *second* real `WireGuard` device with its own crypto context,
/// listen port, overlay IP, and subnet. The device is portable (boringtun
/// userspace `WireGuard` works on Linux/macOS/Windows), so this struct is
/// cross-platform; only the bridge/HCN *attachment* of containers onto it is
/// platform-gated.
struct ServiceTransport {
    /// The live dedicated `WireGuard` device. Dropping it tears down the TUN.
    transport: OverlayTransport,
    /// Actual interface name (kernel-assigned `utunN` on macOS).
    interface: String,
    /// base64 public key of this dedicated device.
    public_key: String,
    /// UDP listen port handed out by [`DedicatedPortAllocator`].
    listen_port: u16,
    /// This node's overlay IP on the dedicated device.
    overlay_ip: std::net::IpAddr,
    /// The service's subnet carried by the dedicated device.
    subnet: ipnet::IpNet,
}

/// The overlay daemon engine.
pub struct OverlaydServer {
    /// Deployment name (used for network naming). Set by `SetupGlobalOverlay`.
    deployment: String,
    /// Per-daemon-process disambiguator included in overlay link names. Set by
    /// `SetupGlobalOverlay`.
    instance_id: String,
    /// Root data directory; HCN markers, IPAM state, etc. live under it.
    data_dir: PathBuf,
    /// Global overlay interface name.
    global_interface: Option<String>,
    /// Global overlay transport (kept alive for the TUN device lifetime). The
    /// SINGLE cluster-wide `WireGuard` transport; every service subnet is
    /// plumbed through its `AllowedIPs`.
    global_transport: Option<OverlayTransport>,
    /// Service-name -> per-service Linux bridge / placeholder name.
    service_interfaces: HashMap<String, String>,
    /// Service-name -> dedicated per-service `WireGuard` transport (Dedicated
    /// mode). Coexists with `global_transport`. Empty for Shared-only nodes.
    service_transports: HashMap<String, ServiceTransport>,
    /// Port allocator for dedicated devices (band above the global WG port).
    dedicated_ports: DedicatedPortAllocator,
    /// Per-service bridge state (Linux only).
    #[cfg(target_os = "linux")]
    service_bridges: HashMap<String, ServiceBridge>,
    /// Local fallback `ServiceSubnetRegistry`. Used by the Linux Shared bridge
    /// path and by the cross-platform Dedicated path (subnets stay globally
    /// unique regardless of mode/OS).
    service_subnet_registry: Option<zlayer_overlay::allocator::ServiceSubnetRegistry>,
    /// Local raft node id used as the partition key for service-subnet assign.
    local_node_id: u64,
    /// Base64 `WireGuard` public key of THIS node's cluster transport, as told
    /// by the main daemon via `SetLocalWgPubkey` (used for service-subnet
    /// `AllowedIPs` plumbing).
    local_wg_pubkey: Option<String>,
    /// Public key generated for the live global transport, recorded at
    /// `setup_global_overlay` time so `Status` can surface it (the transport
    /// itself exposes no public-key accessor).
    transport_public_key: Option<String>,
    /// IP allocator for the node's overlay slice.
    ip_allocator: IpAllocator,
    /// This node's IP on the global overlay network.
    node_ip: Option<IpAddr>,
    /// `WireGuard` listen port for the overlay network.
    overlay_port: u16,
    /// Full cluster CIDR (e.g. `10.200.0.0/16`).
    cluster_cidr: Option<IpNetwork>,
    /// Per-node slice CIDR.
    slice_cidr: Option<IpNetwork>,
    /// Map of HCN namespace GUID -> (`service_name`, `allocated_ip`) for autoclean.
    #[cfg(target_os = "windows")]
    hcn_cleanup: HashMap<windows::core::GUID, (String, std::net::IpAddr)>,
    /// Per-service container-IP allocators for Windows dedicated services. Each
    /// is bounded to that service's subnet (not the node slice) so dedicated
    /// containers draw addresses from their own isolated network. Keyed by
    /// service name; created lazily on the first dedicated attach.
    #[cfg(target_os = "windows")]
    service_ip_allocators: HashMap<String, IpAllocator>,
    /// Per-PID tracking of overlay attachments on Linux.
    #[cfg(target_os = "linux")]
    attached: HashMap<u32, AttachInfo>,
    /// Overlay DNS server listen address, if one was bootstrapped.
    dns_server_addr: Option<SocketAddr>,
    /// DNS domain for overlay service discovery.
    dns_domain: Option<String>,
    /// Overlay DNS A/AAAA records this node owns (name -> ip).
    dns_records: HashMap<String, IpAddr>,
    /// NAT traversal configuration threaded into every `OverlayConfig`.
    nat_config: Option<NatConfig>,
    /// Override for `OverlayConfig::uapi_sock_dir`.
    uapi_sock_dir: Option<PathBuf>,
    /// Live NAT traversal orchestrator.
    nat_traversal: Option<NatTraversal>,
    /// Unix-epoch seconds of the last successful candidate gather / STUN refresh.
    nat_last_refresh: AtomicU64,
    /// Set when a `Shutdown` request has been received.
    shutdown_requested: bool,
}

impl OverlaydServer {
    /// Create a fresh server bound to `data_dir`. The overlay itself is brought
    /// up lazily by `SetupGlobalOverlay` (which carries the deployment, slice,
    /// port, and NAT toggle from the main daemon).
    ///
    /// # Panics
    /// Panics only if the compile-time-constant default CIDR `10.200.0.0/16`
    /// fails to parse (impossible).
    #[must_use]
    pub fn new(data_dir: PathBuf) -> Self {
        // Until SetupGlobalOverlay arrives, the allocator is bounded to the
        // default cluster /16. SetupGlobalOverlay re-binds it to the node slice.
        let default_cidr: IpNetwork = "10.200.0.0/16".parse().expect("compile-time constant CIDR");
        let overlay_port = zlayer_core::DEFAULT_WG_PORT;

        // Rehydrate the dedicated-port allocator from the on-disk marker so a
        // service that already owns a dedicated overlay re-binds the exact UDP
        // port it had before this process started.
        let marker_path = zlayer_paths::ZLayerDirs::new(data_dir.clone()).agent_network_state();
        let recorded_dedicated_ports: Vec<u16> = NetworkState::load(&marker_path)
            .networks
            .iter()
            .filter(|n| n.owner.starts_with("service:"))
            .filter_map(|n| n.wg_port)
            .collect();

        Self {
            deployment: String::new(),
            instance_id: String::new(),
            data_dir,
            global_interface: None,
            global_transport: None,
            service_interfaces: HashMap::new(),
            service_transports: HashMap::new(),
            dedicated_ports: DedicatedPortAllocator::new(overlay_port, recorded_dedicated_ports),
            #[cfg(target_os = "linux")]
            service_bridges: HashMap::new(),
            service_subnet_registry: None,
            local_node_id: 0,
            local_wg_pubkey: None,
            transport_public_key: None,
            ip_allocator: IpAllocator::new(default_cidr),
            node_ip: None,
            overlay_port,
            cluster_cidr: Some(default_cidr),
            slice_cidr: None,
            #[cfg(target_os = "windows")]
            hcn_cleanup: HashMap::new(),
            #[cfg(target_os = "windows")]
            service_ip_allocators: HashMap::new(),
            #[cfg(target_os = "linux")]
            attached: HashMap::new(),
            dns_server_addr: None,
            dns_domain: None,
            dns_records: HashMap::new(),
            nat_config: None,
            uapi_sock_dir: None,
            nat_traversal: None,
            nat_last_refresh: AtomicU64::new(0),
            shutdown_requested: false,
        }
    }

    /// Override the `WireGuard` UAPI socket directory for every overlay
    /// transport built by this server.
    #[must_use]
    pub fn with_uapi_sock_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.uapi_sock_dir = Some(dir.into());
        self
    }

    /// Whether a `Shutdown` request has been received.
    #[must_use]
    pub fn shutdown_requested(&self) -> bool {
        self.shutdown_requested
    }

    /// The root data directory this server was constructed with. Used by the
    /// uninstall path (`purge_managed_networks`) and for HCN marker resolution.
    #[must_use]
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    // -- request dispatch ----------------------------------------------------

    /// Execute one [`OverlaydRequest`], producing the [`OverlaydResponse`] the
    /// server sends back over IPC. Any internal error is folded into
    /// [`OverlaydResponse::Err`].
    pub async fn handle(&mut self, req: OverlaydRequest) -> OverlaydResponse {
        match self.dispatch(req).await {
            Ok(resp) => resp,
            Err(e) => OverlaydResponse::Err {
                message: e.to_string(),
            },
        }
    }

    #[allow(clippy::too_many_lines)]
    async fn dispatch(&mut self, req: OverlaydRequest) -> Result<OverlaydResponse, OverlaydError> {
        match req {
            OverlaydRequest::SetLocalNodeId { node_id } => {
                self.local_node_id = node_id;
                Ok(OverlaydResponse::Ok)
            }
            OverlaydRequest::SetLocalWgPubkey { pubkey } => {
                self.local_wg_pubkey = Some(pubkey);
                Ok(OverlaydResponse::Ok)
            }
            OverlaydRequest::SetupGlobalOverlay {
                deployment,
                instance_id,
                cluster_cidr,
                slice_cidr,
                wg_port,
                nat_enabled,
            } => {
                let name = self
                    .setup_global_overlay(
                        deployment,
                        instance_id,
                        &cluster_cidr,
                        slice_cidr.as_deref(),
                        wg_port,
                        nat_enabled,
                    )
                    .await?;
                Ok(OverlaydResponse::BridgeName { name })
            }
            OverlaydRequest::TeardownGlobalOverlay => {
                self.teardown_global_overlay();
                Ok(OverlaydResponse::Ok)
            }
            OverlaydRequest::SetupServiceOverlay { service, mode } => {
                let info = self.setup_service_overlay(&service, mode).await?;
                Ok(OverlaydResponse::ServiceOverlay(info))
            }
            OverlaydRequest::TeardownServiceOverlay { service } => {
                self.teardown_service_overlay(&service).await;
                Ok(OverlaydResponse::Ok)
            }
            OverlaydRequest::AllocateIp {
                service,
                join_global,
            } => {
                let ip = self.allocate_ip(&service, join_global)?;
                Ok(OverlaydResponse::Ip { ip })
            }
            OverlaydRequest::ReleaseIp { ip } => {
                self.release_ip(ip);
                Ok(OverlaydResponse::Ok)
            }
            OverlaydRequest::AttachContainer {
                handle,
                service,
                join_global,
                dns_server,
                dns_domain,
            } => {
                let result = self
                    .attach_container(handle, &service, join_global, dns_server, dns_domain)
                    .await?;
                Ok(OverlaydResponse::Attached(result))
            }
            OverlaydRequest::DetachContainer { handle } => {
                self.detach_container(handle).await?;
                Ok(OverlaydResponse::Ok)
            }
            // `scope` selects the target device: `Global` (default) = the single
            // cluster transport; `Service { service }` = that service's
            // dedicated per-service transport.
            OverlaydRequest::AddPeer { peer, scope } => {
                let peer = peer_spec_to_info(&peer)?;
                let transport = self.transport_for_scope(&scope)?;
                Self::add_peer_on(transport, &peer).await?;
                Ok(OverlaydResponse::Ok)
            }
            OverlaydRequest::RemovePeer { pubkey, scope } => {
                let transport = self.transport_for_scope(&scope)?;
                Self::remove_peer_on(transport, &pubkey).await?;
                Ok(OverlaydResponse::Ok)
            }
            OverlaydRequest::AddAllowedIp {
                pubkey,
                cidr,
                scope,
            } => {
                let transport = self.transport_for_scope(&scope)?;
                Self::add_allowed_ip_on(transport, &pubkey, &cidr).await?;
                Ok(OverlaydResponse::Ok)
            }
            OverlaydRequest::RemoveAllowedIp {
                pubkey,
                cidr,
                scope,
            } => {
                let transport = self.transport_for_scope(&scope)?;
                Self::remove_allowed_ip_on(transport, &pubkey, &cidr).await?;
                Ok(OverlaydResponse::Ok)
            }
            OverlaydRequest::RegisterDns { name, ip } => {
                self.register_dns(name, ip);
                Ok(OverlaydResponse::Ok)
            }
            OverlaydRequest::UnregisterDns { name } => {
                self.unregister_dns(&name);
                Ok(OverlaydResponse::Ok)
            }
            OverlaydRequest::Status => Ok(OverlaydResponse::Status(self.status_snapshot().await)),
            OverlaydRequest::NatTick => {
                self.nat_maintenance_tick().await?;
                Ok(OverlaydResponse::Ok)
            }
            OverlaydRequest::Shutdown => {
                self.shutdown_requested = true;
                self.teardown_global_overlay();
                Ok(OverlaydResponse::Ok)
            }
        }
    }

    // -- global overlay ------------------------------------------------------

    /// Bring up (or reuse) this node's base/global overlay.
    ///
    /// Idempotent: if a global transport is already live, reuse it (recreating
    /// without this guard could yank the kernel TUN out from under the running
    /// boringtun worker). Re-binds the IP allocator to `slice_cidr` if one is
    /// supplied so container IPs never collide across nodes.
    ///
    /// # Errors
    /// Returns an error if key generation or interface creation fails.
    async fn setup_global_overlay(
        &mut self,
        deployment: String,
        instance_id: String,
        cluster_cidr: &str,
        slice_cidr: Option<&str>,
        wg_port: u16,
        nat_enabled: bool,
    ) -> Result<String, OverlaydError> {
        self.deployment = deployment;
        self.instance_id = instance_id;
        self.overlay_port = wg_port;

        let cluster: IpNetwork = cluster_cidr.parse().map_err(|e| {
            OverlaydError::Other(format!("invalid cluster CIDR {cluster_cidr}: {e}"))
        })?;
        self.cluster_cidr = Some(cluster);
        if let Some(slice) = slice_cidr {
            let slice_net: IpNetwork = slice
                .parse()
                .map_err(|e| OverlaydError::Other(format!("invalid slice CIDR {slice}: {e}")))?;
            self.slice_cidr = Some(slice_net);
            self.ip_allocator = IpAllocator::new(slice_net);
        }
        // NAT defaults to enabled (NatConfig::default()); honor an explicit
        // disable from the main daemon by stamping a disabled config.
        if !nat_enabled {
            self.nat_config = Some(NatConfig {
                enabled: false,
                ..NatConfig::default()
            });
        }

        if let Some(name) = self.global_interface.clone() {
            if self.global_transport.is_some() {
                tracing::debug!(
                    deployment = %self.deployment,
                    "Global overlay already active, reusing existing transport"
                );
                return Ok(name);
            }
        }

        let interface_name = make_interface_name(&[&self.deployment, &self.instance_id], "g");

        let (private_key, public_key) = OverlayTransport::generate_keys()
            .await
            .map_err(|e| OverlaydError::Overlay(format!("Failed to generate keys: {e}")))?;

        let node_ip = self.ip_allocator.allocate()?;
        self.transport_public_key = Some(public_key.clone());
        let config = self.build_config(private_key, public_key, node_ip, 16, self.overlay_port);
        let mut transport = OverlayTransport::new(config, interface_name);

        transport
            .create_interface()
            .await
            .map_err(|e| OverlaydError::Overlay(format!("Failed to create global overlay: {e}")))?;
        transport.configure(&[]).await.map_err(|e| {
            OverlaydError::Overlay(format!("Failed to configure global overlay: {e}"))
        })?;

        // Read back the actual interface name (on macOS, the kernel assigns utunN).
        let actual_name = transport.interface_name().to_string();

        self.node_ip = Some(node_ip);
        self.global_interface = Some(actual_name.clone());
        self.global_transport = Some(transport);
        Ok(actual_name)
    }

    /// Tear down the node's base overlay (e.g. on full uninstall / shutdown).
    fn teardown_global_overlay(&mut self) {
        if let Some(mut transport) = self.global_transport.take() {
            tracing::info!("Shutting down global overlay");
            transport.shutdown();
        }
        self.global_interface = None;
        self.transport_public_key = None;
    }

    // -- service overlay -----------------------------------------------------

    /// Set up the per-service Linux bridge that backs `service` on this node.
    ///
    /// Returns the bridge name on success.
    ///
    /// # Errors
    /// Returns an error if subnet assignment fails (exhaustion), if the bridge
    /// cannot be created, or if the cluster transport rejects the `AllowedIPs`
    /// update.
    #[cfg(target_os = "linux")]
    async fn setup_service_overlay(
        &mut self,
        service: &str,
        mode: OverlayMode,
    ) -> Result<ServiceOverlayInfo, OverlaydError> {
        match mode.resolve() {
            OverlayMode::Shared => self.setup_service_overlay_shared(service).await,
            OverlayMode::Dedicated => self.setup_service_overlay_dedicated(service).await,
            OverlayMode::Auto => unreachable!("OverlayMode::resolve never returns Auto"),
        }
    }

    /// Shared-mode per-service overlay (Linux): the per-service bridge backed by
    /// the single cluster transport. This is the original `setup_service_overlay`
    /// body verbatim, now returning a [`ServiceOverlayInfo`] with the bridge name
    /// and all identity fields `None` (Shared mode shares the cluster device).
    ///
    /// Returns the bridge name on success.
    ///
    /// # Errors
    /// Returns an error if subnet assignment fails (exhaustion), if the bridge
    /// cannot be created, or if the cluster transport rejects the `AllowedIPs`
    /// update.
    #[cfg(target_os = "linux")]
    #[allow(clippy::too_many_lines)]
    async fn setup_service_overlay_shared(
        &mut self,
        service: &str,
    ) -> Result<ServiceOverlayInfo, OverlaydError> {
        use zlayer_overlay::allocator::IpAllocator as OverlayIpAllocator;

        // 1. Idempotency check.
        if let Some(existing) = self.service_bridges.get(service) {
            let name = existing.name.clone();
            tracing::debug!(service = %service, bridge = %name, "Service bridge already active, reusing");
            return Ok(shared_overlay_info(name));
        }

        // 2. Assign subnet via the (currently local) ServiceSubnetRegistry.
        self.ensure_service_subnet_registry()?;
        let subnet: ipnet::IpNet = {
            let registry = self
                .service_subnet_registry
                .as_mut()
                .expect("ensure_service_subnet_registry leaves Some");
            let node_key = self.local_node_id.to_string();
            registry.assign(service, &node_key).map_err(|e| {
                OverlaydError::Overlay(format!(
                    "ServiceSubnetRegistry::assign({service}, {node_key}) failed: {e}"
                ))
            })?
        };

        // 3+4+6. Create the per-service Linux bridge, assign its gateway, bring
        // it up, build the per-service IpAllocator, and record it.
        let bridge_name = self.create_service_bridge(service, subnet).await?;

        // 5. Plumb subnet into the cluster transport's local AllowedIPs so the
        // single cluster device carries this service's cross-node traffic
        // (Shared mode shares one crypto context for every service).
        if let Some(ref cluster) = self.global_transport {
            if let Some(ref pubkey) = self.local_wg_pubkey {
                if let Err(e) = cluster.add_allowed_ip(pubkey, subnet).await {
                    tracing::warn!(
                        service = %service,
                        subnet = %subnet,
                        error = %e,
                        "Failed to add service subnet to cluster transport AllowedIPs (non-fatal)"
                    );
                }
            } else {
                tracing::debug!(service = %service, "local_wg_pubkey not yet set; skipping cluster AllowedIPs update");
            }
        }

        Ok(shared_overlay_info(bridge_name))
    }

    /// Create the per-service Linux bridge for `service` on `subnet`, assign its
    /// gateway, bring it up, build the per-service [`IpAllocator`], and record it
    /// in `service_bridges` + `service_interfaces`. Returns the bridge name.
    ///
    /// Shared and Dedicated mode share this bridge mechanic verbatim — the ONLY
    /// difference between the two modes is which `WireGuard` device the service
    /// subnet/peers are plumbed onto (the single cluster transport for Shared,
    /// the dedicated per-service transport for Dedicated). This helper does NOT
    /// touch any transport's `AllowedIPs`; the caller does that against the
    /// device it owns.
    ///
    /// # Errors
    /// Returns an error if the bridge cannot be created, addressed, or brought
    /// up, or if the per-service `IpAllocator` cannot be built.
    #[cfg(target_os = "linux")]
    async fn create_service_bridge(
        &mut self,
        service: &str,
        subnet: ipnet::IpNet,
    ) -> Result<String, OverlaydError> {
        use zlayer_overlay::allocator::IpAllocator as OverlayIpAllocator;

        let bridge_name = make_interface_name(&[&self.deployment, &self.instance_id, service], "b");

        if let Err(e) = crate::netlink::create_bridge(&bridge_name).await {
            return Err(OverlaydError::Overlay(format!(
                "create_bridge({bridge_name}) failed: {e}"
            )));
        }
        if let Err(e) = crate::netlink::set_bridge_stp(&bridge_name, false) {
            tracing::warn!(bridge = %bridge_name, error = %e, "set_bridge_stp(off) failed (non-fatal)");
        }

        // Gateway = first usable host in the subnet, assigned to the bridge.
        let gateway = first_usable_ip(subnet);
        if let Err(e) =
            crate::netlink::add_address_to_link_by_name(&bridge_name, gateway, subnet.prefix_len())
                .await
        {
            let _ = crate::netlink::delete_bridge(&bridge_name).await;
            return Err(OverlaydError::Overlay(format!(
                "add_address_to_link_by_name({bridge_name}, {gateway}/{}) failed: {e}",
                subnet.prefix_len()
            )));
        }
        if let Err(e) = crate::netlink::set_link_up_by_name(&bridge_name).await {
            let _ = crate::netlink::delete_bridge(&bridge_name).await;
            return Err(OverlaydError::Overlay(format!(
                "set_link_up_by_name({bridge_name}) failed: {e}"
            )));
        }

        // Build per-service IpAllocator, reserve the gateway.
        let mut ip_allocator = OverlayIpAllocator::new(&subnet.to_string()).map_err(|e| {
            OverlaydError::Overlay(format!("IpAllocator::new({subnet}) failed: {e}"))
        })?;
        let _ = ip_allocator.allocate_specific(gateway);

        self.service_bridges.insert(
            service.to_string(),
            ServiceBridge {
                name: bridge_name.clone(),
                subnet,
                gateway,
                ip_allocator,
            },
        );
        self.service_interfaces
            .insert(service.to_string(), bridge_name.clone());

        tracing::info!(service = %service, bridge = %bridge_name, subnet = %subnet, gateway = %gateway, "Service bridge created");
        Ok(bridge_name)
    }

    /// Non-Linux variant of `setup_service_overlay`. On Windows the per-service
    /// segment is the HCN Internal network created lazily at attach time, and on
    /// macOS containers fall through to host networking. Registers the service
    /// in `service_interfaces` with a placeholder name so presence checks work.
    ///
    /// # Errors
    /// Infallible on non-Linux; the `Result` is preserved for ABI parity.
    #[cfg(not(target_os = "linux"))]
    async fn setup_service_overlay(
        &mut self,
        service: &str,
        mode: OverlayMode,
    ) -> Result<ServiceOverlayInfo, OverlaydError> {
        match mode.resolve() {
            OverlayMode::Shared => self.setup_service_overlay_shared(service).await,
            OverlayMode::Dedicated => self.setup_service_overlay_dedicated(service).await,
            OverlayMode::Auto => unreachable!("OverlayMode::resolve never returns Auto"),
        }
    }

    /// Shared-mode per-service overlay (non-Linux): on Windows the per-service
    /// segment is the HCN Internal network created lazily at attach time, and on
    /// macOS containers fall through to host networking. Registers the service
    /// in `service_interfaces` with a placeholder name so presence checks work.
    ///
    /// # Errors
    /// Infallible on non-Linux; the `Result` is preserved for ABI parity.
    #[cfg(not(target_os = "linux"))]
    #[allow(clippy::unused_async)]
    async fn setup_service_overlay_shared(
        &mut self,
        service: &str,
    ) -> Result<ServiceOverlayInfo, OverlaydError> {
        let placeholder = make_interface_name(&[&self.deployment, &self.instance_id, service], "b");
        self.service_interfaces
            .insert(service.to_string(), placeholder.clone());
        tracing::debug!(service = %service, "Service overlay bridge setup is Linux-only; using direct networking placeholder");
        Ok(shared_overlay_info(placeholder))
    }

    /// Dedicated-mode per-service overlay: stand up a *second* real `WireGuard`
    /// device for `service` with its own crypto context, listen port, overlay
    /// IP, and subnet — distinct from the single cluster transport.
    ///
    /// The cross-platform core (identity, subnet assign, transport bring-up,
    /// marker persist, status) runs on every OS; only the *attachment* of
    /// containers onto the device is platform-gated:
    /// - Linux: a per-service bridge (same mechanic as Shared) routed over the
    ///   dedicated device instead of the cluster device.
    /// - Windows: a per-service HCN Internal network (a later task; a clearly
    ///   marked seam returns an error here for now).
    /// - macOS: nothing further — the utun device is the attachment.
    ///
    /// # Errors
    /// Returns an error if port/key/subnet allocation, transport bring-up,
    /// marker persistence, or the platform attachment fails.
    #[allow(clippy::too_many_lines)]
    async fn setup_service_overlay_dedicated(
        &mut self,
        service: &str,
    ) -> Result<ServiceOverlayInfo, OverlaydError> {
        // ----- cross-platform core (runs on every OS) -----

        // 1. Idempotency: an existing dedicated transport returns its identity.
        if let Some(st) = self.service_transports.get(service) {
            return Ok(dedicated_overlay_info(
                st.interface.clone(),
                &st.public_key,
                st.listen_port,
                st.overlay_ip,
                st.subnet,
            ));
        }

        // 2. Identity: reuse a stable identity from the marker if one exists
        //    (so the device re-binds the same key + port across restarts),
        //    otherwise mint a fresh port + keypair + interface name.
        let marker_path =
            zlayer_paths::ZLayerDirs::new(self.data_dir.clone()).agent_network_state();
        let recorded = NetworkState::load(&marker_path)
            .get(&owner_for_service(service))
            .cloned();

        let (private_key, public_key, listen_port, iface_hint) = match recorded.as_ref() {
            Some(entry)
                if entry.wg_private_key.is_some()
                    && entry.wg_public_key.is_some()
                    && entry.wg_port.is_some()
                    && entry.interface.is_some() =>
            {
                let port = entry.wg_port.expect("checked above");
                self.dedicated_ports.reserve(port);
                (
                    entry.wg_private_key.clone().expect("checked above"),
                    entry.wg_public_key.clone().expect("checked above"),
                    port,
                    entry.interface.clone().expect("checked above"),
                )
            }
            _ => {
                let port = self.dedicated_ports.allocate()?;
                let (priv_key, pub_key) = OverlayTransport::generate_keys()
                    .await
                    .map_err(|e| OverlaydError::Overlay(format!("Failed to generate keys: {e}")))?;
                let iface =
                    make_interface_name(&[&self.deployment, &self.instance_id, service], "d");
                (priv_key, pub_key, port, iface)
            }
        };

        // 3. Subnet: assign from the same registry Shared uses, so per-service
        //    subnets stay globally unique regardless of mode.
        self.ensure_service_subnet_registry()?;
        let subnet: ipnet::IpNet = {
            let registry = self
                .service_subnet_registry
                .as_mut()
                .expect("ensure_service_subnet_registry leaves Some");
            let node_key = self.local_node_id.to_string();
            registry.assign(service, &node_key).map_err(|e| {
                OverlaydError::Overlay(format!(
                    "ServiceSubnetRegistry::assign({service}, {node_key}) failed: {e}"
                ))
            })?
        };
        let overlay_ip = first_usable_ip(subnet);

        // 4. Build + bring up the dedicated transport. The device's overlay CIDR
        //    is the service subnet (so boringtun routes that subnet over THIS
        //    device), and its listen port is the dedicated port.
        let config = self.build_config(
            private_key.clone(),
            public_key.clone(),
            overlay_ip,
            subnet.prefix_len(),
            listen_port,
        );
        let mut transport = OverlayTransport::new(config, iface_hint);
        transport.create_interface().await.map_err(|e| {
            OverlaydError::Overlay(format!(
                "Failed to create dedicated overlay for {service}: {e}"
            ))
        })?;
        transport.configure(&[]).await.map_err(|e| {
            OverlaydError::Overlay(format!(
                "Failed to configure dedicated overlay for {service}: {e}"
            ))
        })?;
        let actual_iface = transport.interface_name().to_string();

        // 5. Persist the marker so the identity survives restarts. Match the
        //    base/Shared entry shape (owner/kind/name/id/subnet) plus the
        //    dedicated WG fields.
        let mut marker = NetworkState::load(&marker_path);
        marker.upsert(ManagedNetwork {
            owner: owner_for_service(service),
            kind: "wg-dedicated".to_string(),
            name: actual_iface.clone(),
            id: public_key.clone(),
            subnet: subnet.to_string(),
            wg_port: Some(listen_port),
            wg_private_key: Some(private_key),
            wg_public_key: Some(public_key.clone()),
            interface: Some(actual_iface.clone()),
        });
        if let Err(e) = marker.save(&marker_path) {
            tracing::warn!(service = %service, error = %e, path = %marker_path.display(), "failed to persist dedicated-overlay marker (device still live)");
        }

        // 6. Record the live transport.
        self.service_transports.insert(
            service.to_string(),
            ServiceTransport {
                transport,
                interface: actual_iface.clone(),
                public_key: public_key.clone(),
                listen_port,
                overlay_ip,
                subnet,
            },
        );

        tracing::info!(
            service = %service,
            interface = %actual_iface,
            listen_port,
            subnet = %subnet,
            overlay_ip = %overlay_ip,
            "Dedicated per-service overlay device created"
        );

        // ----- platform-gated attachment -----
        // `name` in the returned info is the container-attach handle: the bridge
        // name on Linux, the dedicated interface elsewhere.
        let name = self
            .attach_dedicated_service(service, subnet, overlay_ip)
            .await?;

        Ok(dedicated_overlay_info(
            name,
            &public_key,
            listen_port,
            overlay_ip,
            subnet,
        ))
    }

    /// Linux attachment for a dedicated per-service overlay: create the same
    /// per-service bridge Shared uses, but route the service subnet over the
    /// DEDICATED device rather than the cluster device.
    ///
    /// Concretely, the dedicated transport's overlay CIDR already covers
    /// `subnet` (set at `build_config` time in the core), so boringtun routes
    /// `subnet` out the dedicated TUN; we additionally plumb `subnet` onto this
    /// node's own `AllowedIPs` entry on the dedicated device so locally
    /// originated packets to the subnet are accepted. Returns the bridge name.
    ///
    /// # Errors
    /// Returns an error if the bridge cannot be created.
    #[cfg(target_os = "linux")]
    async fn attach_dedicated_service(
        &mut self,
        service: &str,
        subnet: ipnet::IpNet,
        overlay_ip: IpAddr,
    ) -> Result<String, OverlaydError> {
        let _ = overlay_ip;
        let bridge_name = self.create_service_bridge(service, subnet).await?;

        // Plumb the service subnet onto the DEDICATED device (not the cluster
        // device). The dedicated transport's overlay CIDR already routes the
        // subnet out its TUN; adding it to our own pubkey's AllowedIPs keeps the
        // local-accept side consistent with the Shared path's cluster plumbing.
        if let Some(st) = self.service_transports.get(service) {
            if let Some(ref pubkey) = self.local_wg_pubkey {
                if let Err(e) = st.transport.add_allowed_ip(pubkey, subnet).await {
                    tracing::warn!(
                        service = %service,
                        subnet = %subnet,
                        error = %e,
                        "Failed to add service subnet to dedicated transport AllowedIPs (non-fatal)"
                    );
                }
            } else {
                tracing::debug!(service = %service, "local_wg_pubkey not yet set; skipping dedicated AllowedIPs update");
            }
        }

        Ok(bridge_name)
    }

    /// Windows attachment for a dedicated per-service overlay.
    ///
    /// The cross-platform core has already stood up the dedicated Wintun
    /// transport (the encrypted node-to-node path for the service subnet). This
    /// adds the *container-facing* side: a per-service HCN **Internal** network
    /// onto which the agent's containers attach (instead of the node's shared
    /// base overlay network), so dedicated-service traffic is isolated at the
    /// vSwitch layer. Returns the per-service network's name, which the caller
    /// records as the [`ServiceOverlayInfo::name`] attach handle.
    ///
    /// # Errors
    /// Propagates any error from [`Self::ensure_service_network`].
    #[cfg(target_os = "windows")]
    async fn attach_dedicated_service(
        &mut self,
        service: &str,
        subnet: ipnet::IpNet,
        _overlay_ip: IpAddr,
    ) -> Result<String, OverlaydError> {
        // Create (or reuse) the per-service Internal HCN network. The returned
        // GUID is recorded in the marker under `owner_for_service(service)`;
        // the `AttachContainer` handler reuses it via the same marker lookup.
        let _net_id = self.ensure_service_network(service, subnet).await?;
        // The attach handle reported back is the per-service network's name.
        let daemon_name = self.deployment_or_default();
        Ok(format!(
            "{}-svc-{service}",
            overlay_network_name(&daemon_name)
        ))
    }

    /// macOS attachment for a dedicated per-service overlay: the cross-platform
    /// core already brought up a utun device; there is no bridge, so the
    /// interface name itself is the attach handle.
    #[cfg(all(not(target_os = "linux"), not(target_os = "windows")))]
    #[allow(clippy::unused_async)]
    async fn attach_dedicated_service(
        &mut self,
        service: &str,
        _subnet: ipnet::IpNet,
        _overlay_ip: IpAddr,
    ) -> Result<String, OverlaydError> {
        let iface = self
            .service_transports
            .get(service)
            .map(|st| st.interface.clone())
            .unwrap_or_default();
        Ok(iface)
    }

    /// Tear down the per-service segment for `service`. Idempotent.
    // Only the Linux body awaits (netlink + cluster AllowedIPs); other targets
    // are synchronous (transport shutdown is sync) but must keep the async
    // signature for the dispatch call.
    #[cfg_attr(not(target_os = "linux"), allow(clippy::unused_async))]
    async fn teardown_service_overlay(&mut self, service: &str) {
        // Shared-mode segment teardown (bridge on Linux, placeholder elsewhere).
        #[cfg(target_os = "linux")]
        {
            let removed = self.service_bridges.remove(service);
            self.service_interfaces.remove(service);
            if let Some(bridge) = removed {
                if let Some(ref cluster) = self.global_transport {
                    if let Some(ref pubkey) = self.local_wg_pubkey {
                        if let Err(e) = cluster.remove_allowed_ip(pubkey, bridge.subnet).await {
                            tracing::warn!(
                                service = %service,
                                subnet = %bridge.subnet,
                                error = %e,
                                "Failed to remove service subnet from cluster AllowedIPs (non-fatal)"
                            );
                        }
                    }
                }

                if let Err(e) = crate::netlink::delete_bridge(&bridge.name).await {
                    tracing::warn!(service = %service, bridge = %bridge.name, error = %e, "delete_bridge failed (non-fatal)");
                }

                if let Some(registry) = self.service_subnet_registry.as_mut() {
                    let node_key = self.local_node_id.to_string();
                    let _ = registry.release(service, &node_key);
                }

                tracing::info!(service = %service, bridge = %bridge.name, "Tore down service bridge");
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            if let Some(iface) = self.service_interfaces.remove(service) {
                tracing::info!(service = %service, interface = %iface, "Removed service overlay interface (placeholder, non-Linux)");
            }
        }

        // Dedicated-mode teardown (cross-platform): tear down the per-service
        // transport, free its port, and drop its marker entry. No-op when the
        // service ran in Shared mode (nothing in `service_transports`).
        if let Some(mut st) = self.service_transports.remove(service) {
            st.transport.shutdown();
            self.dedicated_ports.release(st.listen_port);

            // Release the subnet assignment (Shared releases it inside the
            // Linux block above; the dedicated subnet lives in the same
            // registry, so release it here for the dedicated case on every OS).
            if let Some(registry) = self.service_subnet_registry.as_mut() {
                let node_key = self.local_node_id.to_string();
                let _ = registry.release(service, &node_key);
            }

            let marker_path =
                zlayer_paths::ZLayerDirs::new(self.data_dir.clone()).agent_network_state();
            let mut marker = NetworkState::load(&marker_path);
            let removed_entry = marker.remove(&owner_for_service(service));
            if removed_entry.is_some() {
                if let Err(e) = marker.save(&marker_path) {
                    tracing::warn!(service = %service, error = %e, path = %marker_path.display(), "failed to persist dedicated-overlay marker removal");
                }
            }

            // Windows: delete the per-service HCN Internal network this service
            // owned. The marker entry's `id` is the bare HCN GUID (set by
            // `ensure_service_network`); delete the network so a dedicated
            // service tears down cleanly without waiting for a full uninstall.
            // Also drop the per-service container-IP allocator.
            #[cfg(target_os = "windows")]
            {
                self.service_ip_allocators.remove(service);
                if let Some(entry) = removed_entry.as_ref() {
                    if entry.kind == "hcn-internal" {
                        if let Ok(guid) = windows::core::GUID::try_from(entry.id.as_str()) {
                            match zlayer_hns::network::Network::delete(guid) {
                                Ok(()) => {
                                    tracing::info!(service = %service, id = %entry.id, "deleted per-service HCN network");
                                }
                                Err(e) => {
                                    tracing::warn!(service = %service, id = %entry.id, error = %e, "failed to delete per-service HCN network (may leak until uninstall)");
                                }
                            }
                        } else {
                            tracing::warn!(service = %service, id = %entry.id, "per-service marker has unparseable HCN GUID; skipping network delete");
                        }
                    }
                }
            }
            #[cfg(not(target_os = "windows"))]
            drop(removed_entry);

            tracing::info!(
                service = %service,
                interface = %st.interface,
                listen_port = st.listen_port,
                "Tore down dedicated per-service overlay device"
            );
        }
    }

    /// Initialize the local fallback `ServiceSubnetRegistry` from the configured
    /// cluster CIDR. Called on first `setup_service_overlay` use.
    ///
    /// # Errors
    /// Returns an error when no cluster CIDR is configured or the registry
    /// cannot be built.
    fn ensure_service_subnet_registry(&mut self) -> Result<(), OverlaydError> {
        use zlayer_overlay::allocator::ServiceSubnetRegistry;

        if self.service_subnet_registry.is_some() {
            return Ok(());
        }
        let cluster_cidr = self.cluster_cidr.ok_or_else(|| {
            OverlaydError::Other(
                "service subnet registry needs a cluster CIDR (SetupGlobalOverlay first)"
                    .to_string(),
            )
        })?;
        let cluster_ipnet: ipnet::IpNet = cluster_cidr.to_string().parse().map_err(|e| {
            OverlaydError::Other(format!(
                "failed to convert cluster CIDR {cluster_cidr} to ipnet::IpNet: {e}"
            ))
        })?;
        let slice_prefix: u8 = match cluster_ipnet {
            ipnet::IpNet::V4(_) => 28,
            ipnet::IpNet::V6(_) => 120,
        };
        let registry = ServiceSubnetRegistry::new(cluster_ipnet, slice_prefix).map_err(|e| {
            OverlaydError::Other(format!("failed to build ServiceSubnetRegistry: {e}"))
        })?;
        self.service_subnet_registry = Some(registry);
        Ok(())
    }

    // -- IP allocation -------------------------------------------------------

    /// Allocate an overlay IP from the per-service bridge (Linux) or the node
    /// slice (otherwise). `join_global` reserves a second global-overlay IP too,
    /// matching the eth1 attach behavior.
    ///
    /// # Errors
    /// Returns an error if the relevant pool is exhausted.
    fn allocate_ip(&mut self, service: &str, join_global: bool) -> Result<IpAddr, OverlaydError> {
        // `join_global` does not allocate a second IP here: the companion
        // global-overlay IP (eth1) is reserved at attach time. `AllocateIp`
        // returns only the primary (service / slice) IP the caller asked for.
        let _ = join_global;
        #[cfg(target_os = "linux")]
        {
            if let Some(bridge) = self.service_bridges.get_mut(service) {
                return bridge.ip_allocator.allocate().ok_or_else(|| {
                    OverlaydError::Overlay(format!(
                        "service bridge {} subnet {} exhausted",
                        bridge.name, bridge.subnet
                    ))
                });
            }
        }
        let _ = service;
        self.ip_allocator.allocate()
    }

    /// Return an overlay IP to the allocator (service-bridge pool when known,
    /// otherwise the node slice).
    fn release_ip(&mut self, ip: IpAddr) {
        #[cfg(target_os = "linux")]
        {
            for bridge in self.service_bridges.values_mut() {
                if bridge.subnet.contains(ip) {
                    bridge.ip_allocator.release(ip);
                    return;
                }
            }
        }
        self.ip_allocator.release(ip);
    }

    // -- container attach (Linux) -------------------------------------------

    /// Wire a container into the overlay and return its [`AttachResult`].
    ///
    /// # Errors
    /// Returns an error if the container cannot be attached.
    async fn attach_container(
        &mut self,
        handle: AttachHandle,
        service: &str,
        join_global: bool,
        dns_server: Option<IpAddr>,
        dns_domain: Option<String>,
    ) -> Result<AttachResult, OverlaydError> {
        // Record the overlay DNS resolver/zone the main daemon staged for this
        // node so later attaches (and the Windows HCN endpoint `Dns` schema)
        // can fall back to them when a per-attach value isn't supplied.
        if let Some(server) = dns_server {
            self.dns_server_addr = Some(SocketAddr::new(server, 53));
        }
        if dns_domain.is_some() {
            self.dns_domain.clone_from(&dns_domain);
        }
        match handle {
            AttachHandle::LinuxPid { pid } => {
                let ip = self
                    .attach_container_linux(pid, service, join_global)
                    .await?;
                Ok(AttachResult {
                    ip,
                    namespace_guid: None,
                })
            }
            AttachHandle::WindowsContainer { container_id, ip } => {
                self.attach_container_windows(&container_id, service, ip, dns_server, dns_domain)
                    .await
            }
        }
    }

    /// Tear down a container's overlay attachment and release its IP.
    ///
    /// # Errors
    /// Returns an error only if a netlink delete fails for a reason other than
    /// "link not found".
    async fn detach_container(&mut self, handle: AttachHandle) -> Result<(), OverlaydError> {
        match handle {
            AttachHandle::LinuxPid { pid } => self.detach_container_linux(pid).await,
            AttachHandle::WindowsContainer { container_id, .. } => {
                self.detach_container_windows(&container_id).await
            }
        }
    }

    /// Linux veth/netns attach. On non-Linux this returns the node's overlay IP
    /// (host networking) and is never wired for a `LinuxPid` handle in practice.
    #[cfg(target_os = "linux")]
    async fn attach_container_linux(
        &mut self,
        container_pid: u32,
        service: &str,
        join_global: bool,
    ) -> Result<IpAddr, OverlaydError> {
        // Look up the per-service bridge.
        let (bridge_name, bridge_subnet, bridge_gateway, container_ip) = {
            let bridge = self.service_bridges.get_mut(service).ok_or_else(|| {
                OverlaydError::Other(format!(
                    "no service bridge for service {service}; call setup_service_overlay() first"
                ))
            })?;
            let ip = bridge.ip_allocator.allocate().ok_or_else(|| {
                OverlaydError::Overlay(format!(
                    "service bridge {} subnet {} exhausted",
                    bridge.name, bridge.subnet
                ))
            })?;
            (bridge.name.clone(), bridge.subnet, bridge.gateway, ip)
        };

        let bridge_params = BridgeAttachParams {
            bridge_name: &bridge_name,
            gateway: bridge_gateway,
            subnet_prefix_len: bridge_subnet.prefix_len(),
        };
        if let Err(e) = self
            .attach_to_interface(
                container_pid,
                container_ip,
                "s",
                "eth0",
                Some(&bridge_params),
            )
            .await
        {
            if let Some(bridge) = self.service_bridges.get_mut(service) {
                bridge.ip_allocator.release(container_ip);
            }
            return Err(e);
        }

        let mut global_ip: Option<IpAddr> = None;
        if join_global && self.global_interface.is_some() {
            let g_ip = self.ip_allocator.allocate()?;
            self.attach_to_interface(container_pid, g_ip, "g", "eth1", None)
                .await?;
            global_ip = Some(g_ip);
        }

        self.attached.insert(
            container_pid,
            AttachInfo {
                service_ip: container_ip,
                service_name: Some(service.to_string()),
                global_ip,
                joined_global: global_ip.is_some(),
            },
        );

        Ok(container_ip)
    }

    /// Non-Linux fallback: containers share the host network, so return the
    /// node's overlay IP (or loopback).
    #[cfg(not(target_os = "linux"))]
    #[allow(clippy::unused_async)]
    async fn attach_container_linux(
        &mut self,
        _container_pid: u32,
        service: &str,
        _join_global: bool,
    ) -> Result<IpAddr, OverlaydError> {
        tracing::debug!(service = %service, "LinuxPid attach is a no-op off Linux; using node overlay IP");
        Ok(self.node_ip.unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST)))
    }

    /// Release the overlay resources held by a Linux container PID. Idempotent.
    #[cfg(target_os = "linux")]
    async fn detach_container_linux(&mut self, pid: u32) -> Result<(), OverlaydError> {
        let Some(info) = self.attached.remove(&pid) else {
            return Ok(());
        };

        let veth_s = format!("veth-{pid}-s");
        if let Err(e) = crate::netlink::delete_link_by_name(&veth_s).await {
            tracing::warn!(link = %veth_s, pid, error = %e, "Failed to delete service veth");
        }
        if info.joined_global {
            let veth_g = format!("veth-{pid}-g");
            if let Err(e) = crate::netlink::delete_link_by_name(&veth_g).await {
                tracing::warn!(link = %veth_g, pid, error = %e, "Failed to delete global veth");
            }
        }

        if let Some(svc) = info.service_name.as_deref() {
            if let Some(bridge) = self.service_bridges.get_mut(svc) {
                bridge.ip_allocator.release(info.service_ip);
            } else {
                tracing::debug!(service = %svc, ip = %info.service_ip, "detach: service bridge already torn down; dropping service IP release");
            }
        } else {
            self.ip_allocator.release(info.service_ip);
        }
        if let Some(g) = info.global_ip {
            self.ip_allocator.release(g);
        }
        Ok(())
    }

    /// Non-Linux fallback: nothing to detach (host networking).
    #[cfg(not(target_os = "linux"))]
    #[allow(clippy::unused_async)]
    async fn detach_container_linux(&mut self, _pid: u32) -> Result<(), OverlaydError> {
        Ok(())
    }

    /// Best-effort sweep of orphan veth endpoints whose owning container PID is
    /// no longer alive. Names matching `veth-<pid>-*` / `vc-<pid>-*` where
    /// `/proc/<pid>` does not exist are deleted.
    #[cfg(target_os = "linux")]
    async fn sweep_orphan_veths() {
        let links = match crate::netlink::list_all_links().await {
            Ok(links) => links,
            Err(e) => {
                tracing::warn!(error = %e, "Failed to list links for orphan sweep");
                return;
            }
        };
        for (_index, name) in links {
            let remainder = if let Some(r) = name.strip_prefix("veth-") {
                r
            } else if let Some(r) = name.strip_prefix("vc-") {
                r
            } else {
                continue;
            };
            let Some(pid_str) = remainder.split('-').next() else {
                continue;
            };
            let pid: u32 = match pid_str.parse() {
                Ok(p) => p,
                Err(_) => continue,
            };
            if Path::new(&format!("/proc/{pid}")).exists() {
                continue;
            }
            tracing::info!(link = %name, pid = pid, "Deleting orphan veth");
            if let Err(e) = crate::netlink::delete_link_by_name(&name).await {
                tracing::warn!(link = %name, error = %e, "Failed to delete orphan veth");
            }
        }
    }

    #[cfg(target_os = "linux")]
    #[allow(clippy::too_many_lines)]
    async fn attach_to_interface(
        &self,
        container_pid: u32,
        ip: IpAddr,
        tag: &str,
        container_iface: &str,
        bridge: Option<&BridgeAttachParams<'_>>,
    ) -> Result<(), OverlaydError> {
        // Best-effort cleanup of orphan veths left by a previous daemon crash.
        Self::sweep_orphan_veths().await;

        let is_v6 = ip.is_ipv6();
        let prefix_len: u8 = if let Some(b) = bridge {
            b.subnet_prefix_len
        } else if is_v6 {
            64
        } else {
            24
        };
        let host_prefix: u8 = if is_v6 { 128 } else { 32 };

        let veth_host = format!("veth-{container_pid}-{tag}");
        let veth_pending = format!("vc-{container_pid}-{tag}");
        let veth_container = container_iface.to_string();

        let container_ns_fd = std::os::fd::OwnedFd::from(
            std::fs::File::open(format!("/proc/{container_pid}/ns/net")).map_err(|e| {
                OverlaydError::Overlay(format!("Failed to open /proc/{container_pid}/ns/net: {e}"))
            })?,
        );

        crate::netlink::delete_link_by_name(&veth_host)
            .await
            .map_err(|e| OverlaydError::Overlay(format!("pre-cleanup delete {veth_host}: {e}")))?;
        crate::netlink::delete_link_by_name(&veth_pending)
            .await
            .map_err(|e| {
                OverlaydError::Overlay(format!("pre-cleanup delete {veth_pending}: {e}"))
            })?;

        let bridge_gateway: Option<IpAddr> = bridge.map(|b| b.gateway);
        let bridge_name: Option<String> = bridge.map(|b| b.bridge_name.to_string());
        let node_ip = self.node_ip;

        let result: Result<(), OverlaydError> = async {
            crate::netlink::create_veth_pair(&veth_host, &veth_pending)
                .await
                .map_err(|e| OverlaydError::Overlay(format!("create veth pair: {e}")))?;

            crate::netlink::move_link_into_netns_fd_and_rename(
                &veth_pending,
                AsFd::as_fd(&container_ns_fd),
                &veth_container,
            )
            .map_err(|e| OverlaydError::Overlay(format!("move veth into netns: {e}")))?;

            let vc = veth_container.clone();
            let bridge_gateway_for_netns = bridge_gateway;
            tokio::task::spawn_blocking(move || {
                crate::netlink::with_netns_fd_async(container_ns_fd, move || async move {
                    crate::netlink::add_address_to_link_by_name(&vc, ip, prefix_len).await?;
                    crate::netlink::set_link_up_by_name(&vc).await?;
                    crate::netlink::set_link_up_by_name("lo").await?;
                    if let Some(gw) = bridge_gateway_for_netns {
                        crate::netlink::add_default_route_via_gateway(gw).await?;
                    }
                    Ok(())
                })
            })
            .await
            .map_err(|e| OverlaydError::Overlay(format!("container netns task panicked: {e}")))?
            .map_err(|e| OverlaydError::Overlay(format!("container netns ops: {e}")))?;

            crate::netlink::set_link_up_by_name(&veth_host)
                .await
                .map_err(|e| OverlaydError::Overlay(format!("set {veth_host} up: {e}")))?;

            if let Some(bname) = bridge_name.as_deref() {
                crate::netlink::add_link_to_bridge(&veth_host, bname)
                    .await
                    .map_err(|e| {
                        OverlaydError::Overlay(format!(
                            "enslave {veth_host} to bridge {bname}: {e}"
                        ))
                    })?;
            } else {
                crate::netlink::replace_route_via_dev(ip, host_prefix, &veth_host, node_ip)
                    .await
                    .map_err(|e| {
                        OverlaydError::Overlay(format!("host route for {ip}/{host_prefix}: {e}"))
                    })?;
            }

            let _ = crate::netlink::set_sysctl("net.ipv4.ip_forward", "1");
            let _ = crate::netlink::set_sysctl("net.ipv6.conf.all.forwarding", "1");

            Ok(())
        }
        .await;

        if result.is_err() {
            let _ = crate::netlink::delete_link_by_name(&veth_host).await;
            let _ = crate::netlink::delete_link_by_name(&veth_pending).await;
        }
        result
    }

    // -- container attach (Windows HCN) -------------------------------------

    /// Windows attach: ensure the overlay HCN Internal network exists, allocate
    /// or validate the IP, create the per-container HCN endpoint + namespace,
    /// and return the bare-lowercase namespace GUID for the agent to embed in
    /// the compute-system document.
    ///
    /// # Errors
    /// Returns an error if the network/endpoint cannot be created or the slice
    /// is exhausted.
    #[cfg(target_os = "windows")]
    async fn attach_container_windows(
        &mut self,
        container_id: &str,
        service: &str,
        ip_override: Option<IpAddr>,
        dns_server: Option<IpAddr>,
        dns_domain: Option<String>,
    ) -> Result<AttachResult, OverlaydError> {
        // Resolve whether THIS service has a dedicated per-service overlay. It
        // does iff a live dedicated transport exists OR a `hcn-internal` marker
        // entry is recorded under `owner_for_service(service)` (the network
        // survives daemon restarts even if the transport map is empty mid-init).
        // Dedicated services attach onto their OWN per-service Internal network
        // and draw IPs from the service subnet; everything else uses the node's
        // shared base overlay network and the node slice.
        let dedicated_subnet = self.dedicated_service_subnet(service);

        let (net_id, ip, prefix_length) = if let Some(svc_subnet) = dedicated_subnet {
            // ----- dedicated per-service network path -----
            let net_id = self.ensure_service_network(service, svc_subnet).await?;

            // Allocate (or validate) the IP from the SERVICE subnet, not the
            // node slice. A per-service allocator is created lazily and bounded
            // to the service subnet so addresses stay inside the dedicated
            // network. An `ip_override` inside the service subnet is honored;
            // one outside it is rejected so a slice-allocated IP can't leak onto
            // the dedicated network.
            let svc_ipnetwork: IpNetwork = svc_subnet.to_string().parse().map_err(|e| {
                OverlaydError::Other(format!("failed to parse service subnet {svc_subnet}: {e}"))
            })?;
            let allocator = self
                .service_ip_allocators
                .entry(service.to_string())
                .or_insert_with(|| IpAllocator::new(svc_ipnetwork));
            let ip = match ip_override {
                Some(ip) if svc_subnet.contains(&ip) => ip,
                Some(ip) => {
                    return Err(OverlaydError::Other(format!(
                        "overridden IP {ip} is not inside dedicated service subnet {svc_subnet} for service {service}"
                    )));
                }
                None => allocator.allocate()?,
            };
            (net_id, ip, svc_subnet.prefix_len())
        } else {
            // ----- shared base overlay network path (unchanged) -----
            let slice = self.slice_cidr.ok_or_else(|| {
                OverlaydError::Other(
                    "no node slice assigned yet (SetupGlobalOverlay with slice_cidr first)"
                        .to_string(),
                )
            })?;
            let slice_ipnet: ipnet::IpNet = slice.to_string().parse().map_err(|e| {
                OverlaydError::Other(format!("failed to parse slice CIDR {slice}: {e}"))
            })?;
            let net_id = self.ensure_overlay_network(slice_ipnet).await?;
            let ip = match ip_override {
                Some(ip) => ip,
                None => self.ip_allocator.allocate()?,
            };
            (net_id, ip, slice_ipnet.prefix_len())
        };

        // 3. Create the endpoint + per-container namespace on the network.
        let dns_server_eff = dns_server.or_else(|| self.dns_server_addr.map(|a| a.ip()));
        let dns_domain_for_attach = dns_domain.or_else(|| self.dns_domain.clone());
        let cluster_cidr = self.cluster_cidr.map(|c| c.to_string()).unwrap_or_default();
        let owner_tag = owner_tag(&self.deployment_or_default());
        let cid = container_id.to_string();

        let attachment = tokio::task::spawn_blocking(move || {
            zlayer_hns::attach::EndpointAttachment::create_overlay(
                net_id,
                &owner_tag,
                cid.as_str(),
                ip,
                prefix_length,
                &cluster_cidr,
                dns_server_eff,
                dns_domain_for_attach.as_deref(),
            )
        })
        .await
        .map_err(|e| OverlaydError::Other(format!("spawn_blocking join failed: {e}")))?
        .map_err(|e| OverlaydError::Overlay(format!("HCN overlay endpoint attach failed: {e}")))?;

        let namespace_id = attachment.namespace_id();
        let bare_guid = format_guid_bare(namespace_id);

        // Record for autoclean keyed by namespace GUID.
        self.hcn_cleanup
            .insert(namespace_id, (service.to_string(), ip));

        tracing::info!(
            ns = %bare_guid,
            service = %service,
            ip = %ip,
            "Attached container to HCN overlay"
        );

        Ok(AttachResult {
            ip,
            namespace_guid: Some(bare_guid),
        })
    }

    /// Non-Windows path: a `WindowsContainer` handle has no meaning off Windows.
    #[cfg(not(target_os = "windows"))]
    #[allow(clippy::unused_async)]
    async fn attach_container_windows(
        &mut self,
        _container_id: &str,
        _service: &str,
        _ip_override: Option<IpAddr>,
        _dns_server: Option<IpAddr>,
        _dns_domain: Option<String>,
    ) -> Result<AttachResult, OverlaydError> {
        Err(OverlaydError::Other(
            "WindowsContainer attach is only supported on Windows".to_string(),
        ))
    }

    /// Detach a Windows container by its bare namespace GUID and release its IP.
    /// Idempotent: unknown ids are a no-op.
    #[cfg(target_os = "windows")]
    #[allow(clippy::unused_async)]
    async fn detach_container_windows(
        &mut self,
        namespace_guid: &str,
    ) -> Result<(), OverlaydError> {
        use windows::core::GUID;

        let Ok(guid) = GUID::try_from(namespace_guid) else {
            tracing::warn!(ns = %namespace_guid, "detach: unparseable namespace GUID");
            return Ok(());
        };
        if let Some((service, ip)) = self.hcn_cleanup.remove(&guid) {
            self.ip_allocator.release(ip);
            tracing::info!(ns = %namespace_guid, service = %service, ip = %ip, "Released HCN overlay attachment");
        }
        Ok(())
    }

    /// Non-Windows path.
    #[cfg(not(target_os = "windows"))]
    #[allow(clippy::unused_async)]
    async fn detach_container_windows(
        &mut self,
        _namespace_guid: &str,
    ) -> Result<(), OverlaydError> {
        Ok(())
    }

    /// Ensure the per-daemon HCN overlay (Internal vSwitch, no physical-NIC
    /// binding) exists on the host, reusing one recorded in the
    /// `{data_dir}/agent_network.json` marker or discoverable by name, and
    /// recording it in the marker on create.
    ///
    /// # Errors
    /// Propagates the underlying `zlayer_hns` error on create failure.
    #[cfg(target_os = "windows")]
    #[allow(clippy::too_many_lines)]
    async fn ensure_overlay_network(
        &mut self,
        slice_cidr: ipnet::IpNet,
    ) -> Result<windows::core::GUID, OverlaydError> {
        use windows::core::GUID;

        let daemon_name = self.deployment_or_default();
        let net_name = overlay_network_name(&daemon_name);
        let marker_path =
            zlayer_paths::ZLayerDirs::new(self.data_dir.clone()).agent_network_state();

        // Fast path: marker names a network GUID that still exists; reopen it.
        if let Some(recorded_id) = crate::network_state::NetworkState::load(&marker_path)
            .get(crate::network_state::OWNER_BASE)
            .and_then(|entry| GUID::try_from(entry.id.as_str()).ok())
        {
            let reopened = tokio::task::spawn_blocking(move || {
                zlayer_hns::network::Network::open(recorded_id).ok()
            })
            .await
            .map_err(|e| OverlaydError::Other(format!("spawn_blocking join failed: {e}")))?;
            if reopened.is_some() {
                tracing::info!(name = %net_name, "reusing HCN overlay network from marker");
                return Ok(recorded_id);
            }
        }

        // Idempotency: reuse a host network whose queried name matches ours.
        let target_name = net_name.clone();
        let existing = tokio::task::spawn_blocking(move || -> Option<GUID> {
            let guids = zlayer_hns::network::list("{}").ok()?;
            for guid in guids {
                let Ok(network) = zlayer_hns::network::Network::open(guid) else {
                    continue;
                };
                if matches!(network.query("{}"), Ok(props) if props.name == target_name) {
                    return Some(guid);
                }
            }
            None
        })
        .await
        .map_err(|e| OverlaydError::Other(format!("spawn_blocking join failed: {e}")))?;

        if let Some(existing_id) = existing {
            tracing::info!(name = %net_name, "reusing existing HCN overlay network");
            return Ok(existing_id);
        }

        let net_id = GUID::new()
            .map_err(|e| OverlaydError::Other(format!("GUID::new for overlay network: {e}")))?;
        let subnet_str = slice_cidr.to_string();

        // Default: an HCN Internal network — an internal vSwitch with NO
        // physical-NIC binding — so container traffic never touches the
        // operator's gateway adapter. Setting ZLAYER_HCN_UPLINK_ADAPTER opts
        // into the legacy Transparent model bound to that named uplink.
        let use_transparent = std::env::var(zlayer_hns::adapter::ZLAYER_UPLINK_ENV)
            .ok()
            .is_some_and(|v| !v.trim().is_empty());

        let net_name_for_create = net_name.clone();
        let subnet_for_create = subnet_str.clone();
        if use_transparent {
            let uplink = zlayer_hns::adapter::find_primary_adapter()
                .map_err(|e| OverlaydError::Other(format!("find_primary_adapter: {e}")))?;
            tracing::warn!(uplink = %uplink, "ZLAYER_HCN_UPLINK_ADAPTER set: creating HCN *Transparent* overlay bound to a physical NIC");
            tokio::task::spawn_blocking(move || {
                zlayer_hns::network::Network::create_transparent(
                    net_id,
                    &net_name_for_create,
                    &subnet_for_create,
                    &uplink,
                )
            })
            .await
            .map_err(|e| OverlaydError::Other(format!("spawn_blocking join failed: {e}")))?
            .map_err(|e| {
                OverlaydError::Overlay(format!("HcnCreateNetwork transparent ({net_name}): {e}"))
            })?;
        } else {
            tokio::task::spawn_blocking(move || {
                zlayer_hns::network::Network::create_internal(
                    net_id,
                    &net_name_for_create,
                    &subnet_for_create,
                )
            })
            .await
            .map_err(|e| OverlaydError::Other(format!("spawn_blocking join failed: {e}")))?
            .map_err(|e| {
                OverlaydError::Overlay(format!("HcnCreateNetwork internal ({net_name}): {e}"))
            })?;
        }

        // HCN's Static IPAM needs ~1-2s after network create to settle its
        // address pool; without this the first endpoint frequently fails with
        // HCN_E_ADDR_INVALID_OR_RESERVED.
        tokio::time::sleep(std::time::Duration::from_millis(2000)).await;

        tracing::info!(
            subnet = %subnet_str,
            mode = if use_transparent { "Transparent" } else { "Internal" },
            "created HCN overlay network"
        );

        // Persist the marker so subsequent runs reuse this network by GUID and a
        // full uninstall knows to delete it. Best-effort.
        let mut marker = crate::network_state::NetworkState::load(&marker_path);
        marker.upsert(crate::network_state::ManagedNetwork {
            owner: crate::network_state::OWNER_BASE.to_string(),
            kind: if use_transparent {
                "hcn-transparent"
            } else {
                "hcn-internal"
            }
            .to_string(),
            name: net_name.clone(),
            id: format_guid_bare(net_id),
            subnet: subnet_str.clone(),
            // Base/Shared HCN network: no dedicated WireGuard identity.
            wg_port: None,
            wg_private_key: None,
            wg_public_key: None,
            interface: None,
        });
        if let Err(e) = marker.save(&marker_path) {
            tracing::warn!(error = %e, path = %marker_path.display(), "failed to persist agent network marker (network still reusable by name)");
        }

        Ok(net_id)
    }

    /// Ensure the per-service HCN **Internal** network for `service` exists on
    /// the host, reusing one recorded under the `service:<name>` marker owner
    /// (or discoverable by its derived name) and recording it on create.
    ///
    /// This is the Windows analogue of the Linux per-service bridge: a
    /// dedicated (`OverlayMode::Dedicated`) service gets its OWN isolated HCN
    /// Internal network — an internal vSwitch with NO physical-NIC binding —
    /// distinct from the node's shared base overlay network. Containers attach
    /// to it (rather than the base network) so dedicated-service traffic is
    /// segregated at the vSwitch layer. Modeled on [`Self::ensure_overlay_network`]
    /// but keyed on [`owner_for_service`] and forced to the Internal type (never
    /// Transparent — the on-box test asserts zero external vSwitches for
    /// dedicated services).
    ///
    /// Returns the network GUID.
    ///
    /// # Errors
    /// Propagates the underlying `zlayer_hns` error on create failure.
    #[cfg(target_os = "windows")]
    #[allow(clippy::too_many_lines)]
    async fn ensure_service_network(
        &mut self,
        service: &str,
        subnet: ipnet::IpNet,
    ) -> Result<windows::core::GUID, OverlaydError> {
        use windows::core::GUID;

        let daemon_name = self.deployment_or_default();
        // Per-service network name: `<base overlay name>-svc-<service>` so it is
        // unambiguously distinct from the base network and from other services.
        let net_name = format!("{}-svc-{service}", overlay_network_name(&daemon_name));
        let owner = owner_for_service(service);
        let marker_path =
            zlayer_paths::ZLayerDirs::new(self.data_dir.clone()).agent_network_state();

        // Fast path: marker names a network GUID that still exists; reopen it.
        // Only honor the recorded id when it belongs to an HCN-internal entry —
        // a Dedicated WireGuard marker (`kind == "wg-dedicated"`) stores the
        // transport public key in `id`, NOT an HCN GUID, so it must be ignored
        // for HCN reuse.
        let recorded_hcn_id = crate::network_state::NetworkState::load(&marker_path)
            .get(&owner)
            .filter(|entry| entry.kind == "hcn-internal")
            .and_then(|entry| GUID::try_from(entry.id.as_str()).ok());
        if let Some(recorded_id) = recorded_hcn_id {
            let reopened = tokio::task::spawn_blocking(move || {
                zlayer_hns::network::Network::open(recorded_id).ok()
            })
            .await
            .map_err(|e| OverlaydError::Other(format!("spawn_blocking join failed: {e}")))?;
            if reopened.is_some() {
                tracing::info!(name = %net_name, service = %service, "reusing per-service HCN network from marker");
                return Ok(recorded_id);
            }
        }

        // Idempotency: reuse a host network whose queried name matches ours.
        let target_name = net_name.clone();
        let existing = tokio::task::spawn_blocking(move || -> Option<GUID> {
            let guids = zlayer_hns::network::list("{}").ok()?;
            for guid in guids {
                let Ok(network) = zlayer_hns::network::Network::open(guid) else {
                    continue;
                };
                if matches!(network.query("{}"), Ok(props) if props.name == target_name) {
                    return Some(guid);
                }
            }
            None
        })
        .await
        .map_err(|e| OverlaydError::Other(format!("spawn_blocking join failed: {e}")))?;

        if let Some(existing_id) = existing {
            tracing::info!(name = %net_name, service = %service, "reusing existing per-service HCN network");
            return Ok(existing_id);
        }

        let net_id = GUID::new()
            .map_err(|e| OverlaydError::Other(format!("GUID::new for per-service network: {e}")))?;
        let subnet_str = subnet.to_string();

        // ALWAYS Internal for a dedicated service — never Transparent. The
        // dedicated requirement is isolation; an Internal network binds NO
        // physical NIC (no external vSwitch), which is what the on-box test
        // asserts.
        let net_name_for_create = net_name.clone();
        let subnet_for_create = subnet_str.clone();
        tokio::task::spawn_blocking(move || {
            zlayer_hns::network::Network::create_internal(
                net_id,
                &net_name_for_create,
                &subnet_for_create,
            )
        })
        .await
        .map_err(|e| OverlaydError::Other(format!("spawn_blocking join failed: {e}")))?
        .map_err(|e| {
            OverlaydError::Overlay(format!("HcnCreateNetwork internal ({net_name}): {e}"))
        })?;

        // HCN's Static IPAM needs ~1-2s after network create to settle its
        // address pool; without this the first endpoint frequently fails with
        // HCN_E_ADDR_INVALID_OR_RESERVED (same wait as the base network).
        tokio::time::sleep(std::time::Duration::from_millis(2000)).await;

        tracing::info!(
            service = %service,
            subnet = %subnet_str,
            "created per-service HCN Internal network"
        );

        // Persist the marker (owner = `service:<name>`, kind = `hcn-internal`)
        // so subsequent runs reuse this network by GUID and a full uninstall
        // (`purge_managed_networks`, which sweeps every `kind` starting with
        // `hcn`) deletes it. Best-effort.
        //
        // A dedicated Windows service shares the SAME owner key for two facts:
        // the dedicated WireGuard identity (written by the cross-platform core
        // in `setup_service_overlay_dedicated`, kind `wg-dedicated`) and this
        // HCN network's GUID. The marker is keyed by owner, so carry the WG
        // identity fields over when we rewrite the entry to `hcn-internal` — the
        // single entry then holds both the HCN GUID (in `id`) and the WG
        // identity (in the `wg_*`/`interface` fields), and the WG private key
        // survives restarts. (The core re-asserts the `wg-dedicated` shape on
        // the next setup; this path re-asserts `hcn-internal` again right after
        // — both are self-healing because the network is also reusable by name.)
        let mut marker = crate::network_state::NetworkState::load(&marker_path);
        let carried = marker.get(&owner).cloned();
        marker.upsert(crate::network_state::ManagedNetwork {
            owner,
            kind: "hcn-internal".to_string(),
            name: net_name.clone(),
            id: format_guid_bare(net_id),
            subnet: subnet_str.clone(),
            wg_port: carried.as_ref().and_then(|c| c.wg_port),
            wg_private_key: carried.as_ref().and_then(|c| c.wg_private_key.clone()),
            wg_public_key: carried.as_ref().and_then(|c| c.wg_public_key.clone()),
            interface: carried.as_ref().and_then(|c| c.interface.clone()),
        });
        if let Err(e) = marker.save(&marker_path) {
            tracing::warn!(service = %service, error = %e, path = %marker_path.display(), "failed to persist per-service network marker (network still reusable by name)");
        }

        Ok(net_id)
    }

    /// Resolve the dedicated per-service subnet for `service`, if the service
    /// runs in `OverlayMode::Dedicated` on this node.
    ///
    /// Source of truth, in order:
    /// 1. The live [`ServiceTransport`] in `service_transports` (the normal
    ///    case once `SetupServiceOverlay` has run this process).
    /// 2. A persisted `hcn-internal` marker entry under
    ///    [`owner_for_service`]`(service)` — covers the window where the HCN
    ///    network exists from a prior run but the transport map is still empty.
    ///
    /// Returns `None` for Shared-mode services (attach onto the base network).
    #[cfg(target_os = "windows")]
    fn dedicated_service_subnet(&self, service: &str) -> Option<ipnet::IpNet> {
        if let Some(st) = self.service_transports.get(service) {
            return Some(st.subnet);
        }
        let marker_path =
            zlayer_paths::ZLayerDirs::new(self.data_dir.clone()).agent_network_state();
        crate::network_state::NetworkState::load(&marker_path)
            .get(&owner_for_service(service))
            .filter(|entry| entry.kind == "hcn-internal")
            .and_then(|entry| entry.subnet.parse::<ipnet::IpNet>().ok())
    }

    /// The daemon name used for HCN network/owner naming, defaulting to
    /// `"zlayer"` when no deployment has been set yet.
    #[cfg(target_os = "windows")]
    fn deployment_or_default(&self) -> String {
        if self.deployment.is_empty() {
            "zlayer".to_string()
        } else {
            self.deployment.clone()
        }
    }

    // -- peers ---------------------------------------------------------------

    /// Resolve a [`PeerScope`] to the live [`OverlayTransport`] its ops target.
    ///
    /// `Global` -> the single cluster transport; `Service { service }` -> that
    /// service's dedicated per-service transport (Dedicated mode only).
    ///
    /// # Errors
    /// Returns an error if the global overlay is not up (for `Global`) or no
    /// dedicated overlay exists for the named service (for `Service`).
    fn transport_for_scope(&self, scope: &PeerScope) -> Result<&OverlayTransport, OverlaydError> {
        match scope {
            PeerScope::Global => self
                .global_transport
                .as_ref()
                .ok_or_else(|| OverlaydError::Other("global overlay not set up".into())),
            PeerScope::Service { service } => self
                .service_transports
                .get(service)
                .map(|s| &s.transport)
                .ok_or_else(|| {
                    OverlaydError::Other(format!("no dedicated overlay for service {service}"))
                }),
        }
    }

    /// Add a peer to a resolved transport.
    ///
    /// # Errors
    /// Wraps the underlying transport error.
    async fn add_peer_on(
        transport: &OverlayTransport,
        peer: &PeerInfo,
    ) -> Result<(), OverlaydError> {
        transport
            .add_peer(peer)
            .await
            .map_err(|e| OverlaydError::Overlay(format!("add_peer failed: {e}")))
    }

    /// Remove a peer (by base64 public key) from a resolved transport.
    ///
    /// # Errors
    /// Wraps the underlying transport error.
    async fn remove_peer_on(
        transport: &OverlayTransport,
        pubkey: &str,
    ) -> Result<(), OverlaydError> {
        transport
            .remove_peer(pubkey)
            .await
            .map_err(|e| OverlaydError::Overlay(format!("remove_peer failed: {e}")))
    }

    /// Plumb a CIDR into a peer's `AllowedIPs` on a resolved transport.
    ///
    /// # Errors
    /// Returns an error when the CIDR is invalid or the UAPI write fails.
    async fn add_allowed_ip_on(
        transport: &OverlayTransport,
        pubkey: &str,
        cidr: &str,
    ) -> Result<(), OverlaydError> {
        let net: ipnet::IpNet = cidr
            .parse()
            .map_err(|e| OverlaydError::Other(format!("invalid CIDR {cidr}: {e}")))?;
        transport
            .add_allowed_ip(pubkey, net)
            .await
            .map_err(|e| OverlaydError::Overlay(format!("add_allowed_ip failed: {e}")))
    }

    /// Remove a CIDR from a peer's `AllowedIPs` on a resolved transport.
    ///
    /// # Errors
    /// Returns an error when the CIDR is invalid or the UAPI write fails.
    async fn remove_allowed_ip_on(
        transport: &OverlayTransport,
        pubkey: &str,
        cidr: &str,
    ) -> Result<(), OverlaydError> {
        let net: ipnet::IpNet = cidr
            .parse()
            .map_err(|e| OverlaydError::Other(format!("invalid CIDR {cidr}: {e}")))?;
        transport
            .remove_allowed_ip(pubkey, net)
            .await
            .map_err(|e| OverlaydError::Overlay(format!("remove_allowed_ip failed: {e}")))
    }

    // -- DNS -----------------------------------------------------------------

    /// Register an overlay DNS A/AAAA record.
    fn register_dns(&mut self, name: String, ip: IpAddr) {
        self.dns_records.insert(name, ip);
    }

    /// Remove an overlay DNS record.
    fn unregister_dns(&mut self, name: &str) {
        self.dns_records.remove(name);
    }

    // -- NAT -----------------------------------------------------------------

    /// Periodic NAT traversal maintenance: re-probe STUN, refresh relays.
    /// No-op when NAT traversal has not been started.
    ///
    /// # Errors
    /// Returns an error when the underlying STUN refresh fails.
    async fn nat_maintenance_tick(&mut self) -> Result<(), OverlaydError> {
        // Lazily start NAT traversal on the first tick if a config asks for it.
        if self.nat_traversal.is_none() {
            let config = self.nat_config.clone().unwrap_or_default();
            if config.enabled {
                let mut nat = NatTraversal::new(config, self.overlay_port);
                match nat.gather_candidates().await {
                    Ok(candidates) => {
                        tracing::info!(count = candidates.len(), "Gathered NAT candidates");
                        self.nat_last_refresh.store(now_unix(), Ordering::SeqCst);
                        self.nat_traversal = Some(nat);
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "NAT candidate gathering failed");
                        return Ok(());
                    }
                }
            } else {
                return Ok(());
            }
        }

        let Some(nat) = self.nat_traversal.as_mut() else {
            return Ok(());
        };
        match nat.refresh().await {
            Ok(changed) => {
                if changed {
                    tracing::info!("NAT reflexive address changed during refresh");
                }
                self.nat_last_refresh.store(now_unix(), Ordering::SeqCst);
                Ok(())
            }
            Err(e) => Err(OverlaydError::Overlay(format!(
                "NAT maintenance tick failed: {e}"
            ))),
        }
    }

    // -- status --------------------------------------------------------------

    /// Build a [`StatusSnapshot`] from current overlay state.
    async fn status_snapshot(&self) -> StatusSnapshot {
        let mut peers: Vec<PeerStatus> = Vec::new();
        let public_key = self.transport_public_key.clone();

        if let Some(transport) = self.global_transport.as_ref() {
            // Parse the UAPI dump for per-peer state. Best-effort: a parse
            // failure leaves the peer list empty rather than failing Status.
            if let Ok(dump) = transport.status().await {
                peers = parse_peer_status(&dump);
            }
        }

        let service_count = u32::try_from(self.service_count()).unwrap_or(u32::MAX);
        let peer_count = u32::try_from(peers.len()).unwrap_or(u32::MAX);

        // Per dedicated per-service overlay device: count its peers the same
        // way the global status does (parse the UAPI/status dump).
        let mut dedicated_services: Vec<DedicatedServiceStatus> = Vec::new();
        for (svc, st) in &self.service_transports {
            let peer_count = match st.transport.status().await {
                Ok(dump) => u32::try_from(parse_peer_status(&dump).len()).unwrap_or(u32::MAX),
                Err(_) => 0,
            };
            dedicated_services.push(DedicatedServiceStatus {
                service: svc.clone(),
                interface: st.interface.clone(),
                public_key: st.public_key.clone(),
                listen_port: st.listen_port,
                overlay_ip: st.overlay_ip,
                subnet: st.subnet.to_string(),
                peer_count,
            });
        }

        StatusSnapshot {
            interface: self.global_interface.clone(),
            node_ip: self.node_ip,
            public_key,
            overlay_cidr: self.cluster_cidr.map(|c| c.to_string()),
            slice_cidr: self.slice_cidr.map(|c| c.to_string()),
            peer_count,
            service_count,
            peers,
            dedicated_services,
        }
    }

    /// Number of per-service overlays set up on this node (Shared bridges /
    /// placeholders plus any Dedicated transports not already counted there).
    fn service_count(&self) -> usize {
        let extra_dedicated = self
            .service_transports
            .keys()
            .filter(|svc| !self.service_interfaces.contains_key(*svc))
            .count();
        self.service_interfaces.len() + extra_dedicated
    }

    // -- config helper -------------------------------------------------------

    fn build_config(
        &self,
        private_key: String,
        public_key: String,
        ip: IpAddr,
        mask: u8,
        listen_port: u16,
    ) -> OverlayConfig {
        let local_addr = match ip {
            IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            IpAddr::V6(_) => IpAddr::V6(Ipv6Addr::UNSPECIFIED),
        };
        let mut config = OverlayConfig {
            local_endpoint: SocketAddr::new(local_addr, listen_port),
            private_key,
            public_key,
            overlay_cidr: format!("{ip}/{mask}"),
            ..OverlayConfig::default()
        };
        if let Some(nat) = self.nat_config.clone() {
            config.nat = nat;
        }
        if let Some(dir) = self.uapi_sock_dir.clone() {
            config.uapi_sock_dir = dir;
        }
        config
    }
}

/// Build a Shared-mode [`ServiceOverlayInfo`]: the bridge/placeholder name with
/// every dedicated-device identity field left `None` (Shared mode shares the
/// single cluster device).
fn shared_overlay_info(name: String) -> ServiceOverlayInfo {
    ServiceOverlayInfo {
        name,
        mode: OverlayMode::Shared,
        wg_public_key: None,
        wg_port: None,
        overlay_ip: None,
        subnet: None,
    }
}

/// Build a Dedicated-mode [`ServiceOverlayInfo`] from a dedicated device's
/// identity. `name` is the container-attach handle (bridge name on Linux, the
/// dedicated interface elsewhere).
fn dedicated_overlay_info(
    name: String,
    public_key: &str,
    listen_port: u16,
    overlay_ip: IpAddr,
    subnet: ipnet::IpNet,
) -> ServiceOverlayInfo {
    ServiceOverlayInfo {
        name,
        mode: OverlayMode::Dedicated,
        wg_public_key: Some(public_key.to_string()),
        wg_port: Some(listen_port),
        overlay_ip: Some(overlay_ip),
        subnet: Some(subnet.to_string()),
    }
}

/// Convert a wire [`PeerSpec`] into a `zlayer_overlay::PeerInfo`.
///
/// # Errors
/// Returns an error if `endpoint` cannot be parsed as a `host:port`
/// [`SocketAddr`].
pub fn peer_spec_to_info(spec: &PeerSpec) -> Result<PeerInfo, OverlaydError> {
    let endpoint: SocketAddr = spec.endpoint.parse().map_err(|e| {
        OverlaydError::Other(format!("invalid peer endpoint {}: {e}", spec.endpoint))
    })?;
    Ok(PeerInfo::new(
        spec.public_key.clone(),
        endpoint,
        &spec.allowed_ips,
        std::time::Duration::from_secs(spec.persistent_keepalive_secs),
    ))
}

/// Parse a `wg`-style UAPI/`status` dump into [`PeerStatus`] entries.
///
/// The dump is a series of `key=value` lines; each `public_key=` line starts a
/// new peer block, and subsequent `endpoint=` / `allowed_ip=` /
/// `latest_handshake=` lines belong to it.
fn parse_peer_status(dump: &str) -> Vec<PeerStatus> {
    let mut peers: Vec<PeerStatus> = Vec::new();
    let mut current: Option<PeerStatus> = None;
    let mut allowed: Vec<String> = Vec::new();

    let flush = |peers: &mut Vec<PeerStatus>,
                 current: &mut Option<PeerStatus>,
                 allowed: &mut Vec<String>| {
        if let Some(mut p) = current.take() {
            p.allowed_ips = allowed.join(",");
            peers.push(p);
        }
        allowed.clear();
    };

    for line in dump.lines() {
        let line = line.trim();
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "public_key" | "peer" => {
                flush(&mut peers, &mut current, &mut allowed);
                current = Some(PeerStatus {
                    public_key: value.trim().to_string(),
                    endpoint: String::new(),
                    allowed_ips: String::new(),
                    last_handshake_unix_secs: 0,
                });
            }
            "endpoint" => {
                if let Some(p) = current.as_mut() {
                    p.endpoint = value.trim().to_string();
                }
            }
            "allowed_ip" | "allowed_ips" => {
                if current.is_some() {
                    allowed.push(value.trim().to_string());
                }
            }
            "latest_handshake" | "last_handshake_time_sec" => {
                if let Some(p) = current.as_mut() {
                    p.last_handshake_unix_secs = value.trim().parse().unwrap_or(0);
                }
            }
            _ => {}
        }
    }
    flush(&mut peers, &mut current, &mut allowed);
    peers
}

/// Current Unix time in whole seconds.
fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Simple IP address allocator supporting both IPv4 and IPv6, bounded to a
/// specific CIDR (typically a per-node `/28` slice). Allocations past the last
/// usable host return an exhaustion error.
struct IpAllocator {
    /// CIDR the allocator is bounded to.
    cidr: IpNetwork,
    /// Base (network) address of the CIDR.
    base: IpAddr,
    /// Monotonic counter for the next allocation offset relative to `base`.
    next_offset: AtomicU64,
    /// IPs returned by `release(...)`. `allocate()` drains this first before
    /// incrementing `next_offset`.
    released: parking_lot::Mutex<Vec<IpAddr>>,
}

impl IpAllocator {
    fn new(cidr: IpNetwork) -> Self {
        Self {
            base: cidr.network(),
            cidr,
            next_offset: AtomicU64::new(1),
            released: parking_lot::Mutex::new(Vec::new()),
        }
    }

    #[allow(clippy::cast_possible_truncation)]
    fn compute_addr(&self, offset: u64) -> IpAddr {
        match self.base {
            IpAddr::V4(base_v4) => {
                let base_u32 = u32::from_be_bytes(base_v4.octets());
                let addr = base_u32.wrapping_add(offset as u32);
                IpAddr::V4(Ipv4Addr::from(addr.to_be_bytes()))
            }
            IpAddr::V6(base_v6) => {
                let base_u128 = u128::from(base_v6);
                let addr = base_u128.wrapping_add(u128::from(offset));
                IpAddr::V6(Ipv6Addr::from(addr))
            }
        }
    }

    /// Allocate the next IP in the slice, reusing released IPs first.
    ///
    /// # Errors
    /// Returns [`OverlaydError::Overlay`] when the CIDR is exhausted.
    fn allocate(&self) -> Result<IpAddr, OverlaydError> {
        if let Some(ip) = self.released.lock().pop() {
            return Ok(ip);
        }
        let offset = self.next_offset.fetch_add(1, Ordering::SeqCst);
        let addr = self.compute_addr(offset);

        let in_cidr = self.cidr.contains(addr);
        let is_v4_broadcast = matches!(
            (&self.cidr, &addr),
            (IpNetwork::V4(v4), IpAddr::V4(a)) if *a == v4.broadcast()
        );
        if !in_cidr || is_v4_broadcast {
            return Err(OverlaydError::Overlay(format!(
                "IP allocator exhausted: next address {addr} is outside slice {}",
                self.cidr
            )));
        }
        Ok(addr)
    }

    /// Return an IP to the free pool. Idempotent.
    fn release(&self, ip: IpAddr) {
        let mut released = self.released.lock();
        if !released.contains(&ip) {
            released.push(ip);
        }
    }
}

// -- Windows HCN helpers (ported from the agent's hcs runtime) --------------

/// Owner tag stamped onto every HCN endpoint this server creates. The legacy
/// single-instance value is `"zlayer"`; any other name is used verbatim so two
/// daemons running side-by-side never sweep each other's endpoints.
#[cfg(target_os = "windows")]
fn owner_tag(daemon_name: &str) -> String {
    if daemon_name == "zlayer" {
        "zlayer".to_string()
    } else {
        daemon_name.to_string()
    }
}

/// Name of the per-daemon HCN overlay network on the host. Legacy
/// single-instance value is `"zlayer-overlay"`; any other name becomes
/// `"<daemon_name>-overlay"`.
#[cfg(target_os = "windows")]
fn overlay_network_name(daemon_name: &str) -> String {
    if daemon_name == "zlayer" {
        "zlayer-overlay".to_string()
    } else {
        format!("{daemon_name}-overlay")
    }
}

/// Format a GUID as the bare, lowercase, un-braced string HCN/HCS use to
/// identify a namespace inside a compute-system document's
/// `Container.Networking.Namespace` field (e.g. `aabbccdd-eeff-...`).
#[cfg(target_os = "windows")]
fn format_guid_bare(id: windows::core::GUID) -> String {
    format!("{id:?}")
        .trim_matches(|c: char| c == '{' || c == '}')
        .to_ascii_lowercase()
}

/// Delete every host-level HCN network this server created for `daemon_name` and
/// clear the persistent marker. Called on a full uninstall — never on a routine
/// stop/restart. Best-effort throughout. Synchronous (HCN calls are blocking).
#[cfg(target_os = "windows")]
pub fn purge_managed_networks(data_dir: &Path, daemon_name: &str) {
    use windows::core::GUID;

    let marker_path = zlayer_paths::ZLayerDirs::new(data_dir.to_path_buf()).agent_network_state();
    let state = crate::network_state::NetworkState::load(&marker_path);

    // Pass 1: delete recorded HCN networks by GUID.
    for entry in &state.networks {
        if !entry.kind.starts_with("hcn") {
            continue;
        }
        match GUID::try_from(entry.id.as_str()) {
            Ok(guid) => match zlayer_hns::network::Network::delete(guid) {
                Ok(()) => {
                    tracing::info!(name = %entry.name, id = %entry.id, "deleted managed HCN network");
                }
                Err(e) => {
                    tracing::warn!(name = %entry.name, id = %entry.id, error = %e, "failed to delete managed HCN network");
                }
            },
            Err(e) => {
                tracing::warn!(id = %entry.id, error = %e, "managed network marker has unparseable GUID");
            }
        }
    }

    // Pass 2: name-sweep fallback for an overlay network whose marker entry was
    // lost (crash between create and marker write).
    let overlay_name = overlay_network_name(daemon_name);
    if let Ok(guids) = zlayer_hns::network::list("{}") {
        for guid in guids {
            let Ok(network) = zlayer_hns::network::Network::open(guid) else {
                continue;
            };
            let is_ours = matches!(network.query("{}"), Ok(props) if props.name == overlay_name);
            drop(network);
            if is_ours {
                match zlayer_hns::network::Network::delete(guid) {
                    Ok(()) => {
                        tracing::info!(name = %overlay_name, "deleted overlay HCN network (name sweep)");
                    }
                    Err(e) => {
                        tracing::warn!(name = %overlay_name, error = %e, "failed to delete overlay network (name sweep)");
                    }
                }
            }
        }
    }

    if marker_path.exists() {
        if let Err(e) = std::fs::remove_file(&marker_path) {
            tracing::warn!(error = %e, path = %marker_path.display(), "failed to remove agent network marker");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_spec_to_info_parses_endpoint_and_keepalive() {
        let spec = PeerSpec {
            public_key: "base64key".to_string(),
            endpoint: "1.2.3.4:51820".to_string(),
            allowed_ips: "10.200.0.5/32,10.200.1.0/24".to_string(),
            persistent_keepalive_secs: 25,
        };
        let info = peer_spec_to_info(&spec).expect("valid spec");
        assert_eq!(info.public_key, "base64key");
        assert_eq!(info.endpoint, "1.2.3.4:51820".parse().unwrap());
        assert_eq!(info.allowed_ips, "10.200.0.5/32,10.200.1.0/24");
        assert_eq!(
            info.persistent_keepalive_interval,
            std::time::Duration::from_secs(25)
        );
    }

    #[test]
    fn peer_spec_to_info_rejects_bad_endpoint() {
        let spec = PeerSpec {
            public_key: "k".to_string(),
            endpoint: "not-a-socket-addr".to_string(),
            allowed_ips: String::new(),
            persistent_keepalive_secs: 0,
        };
        assert!(peer_spec_to_info(&spec).is_err());
    }

    #[test]
    fn interface_name_never_exceeds_limit() {
        let cases: Vec<(&[&str], &str)> = vec![
            (&["a"], "g"),
            (&["zlayer-manager"], "g"),
            (&["my-very-long-deployment-name-that-goes-on-and-on"], "g"),
            (&["zlayer", "manager"], "s"),
            (
                &["abcdefghijklmnopqrstuvwxyz", "abcdefghijklmnopqrstuvwxyz"],
                "s",
            ),
            (&["x"], ""),
        ];
        for (parts, suffix) in &cases {
            let name = make_interface_name(parts, suffix);
            assert!(name.len() <= MAX_IFNAME_LEN, "Name '{name}' too long");
            assert!(name.starts_with("zl-"));
        }
    }

    #[test]
    fn interface_name_is_deterministic() {
        assert_eq!(
            make_interface_name(&["zlayer-manager"], "g"),
            make_interface_name(&["zlayer-manager"], "g")
        );
    }

    #[test]
    fn parse_peer_status_splits_blocks() {
        let dump = "\
public_key=AAA
endpoint=1.2.3.4:51820
allowed_ip=10.200.0.2/32
allowed_ip=10.200.1.0/24
latest_handshake=1700000000
public_key=BBB
endpoint=5.6.7.8:51820
allowed_ip=10.200.0.3/32
latest_handshake=0
";
        let peers = parse_peer_status(dump);
        assert_eq!(peers.len(), 2);
        assert_eq!(peers[0].public_key, "AAA");
        assert_eq!(peers[0].endpoint, "1.2.3.4:51820");
        assert_eq!(peers[0].allowed_ips, "10.200.0.2/32,10.200.1.0/24");
        assert_eq!(peers[0].last_handshake_unix_secs, 1_700_000_000);
        assert_eq!(peers[1].public_key, "BBB");
        assert_eq!(peers[1].last_handshake_unix_secs, 0);
    }

    #[tokio::test]
    async fn status_snapshot_before_setup_is_empty() {
        let server = OverlaydServer::new(std::path::PathBuf::from("/tmp/zlayer-overlayd-test"));
        let snap = server.status_snapshot().await;
        assert!(snap.interface.is_none());
        assert!(snap.node_ip.is_none());
        assert!(snap.public_key.is_none());
        assert_eq!(snap.peer_count, 0);
        assert_eq!(snap.service_count, 0);
        assert!(snap.peers.is_empty());
    }

    #[tokio::test]
    async fn allocate_and_release_ip_round_trip() {
        let mut server = OverlaydServer::new(std::path::PathBuf::from("/tmp/zlayer-overlayd-test"));
        let a = server.allocate_ip("svc", false).expect("alloc a");
        let b = server.allocate_ip("svc", false).expect("alloc b");
        assert_ne!(a, b);
        server.release_ip(a);
        // Released IP is handed back before the monotonic counter advances.
        let c = server.allocate_ip("svc", false).expect("alloc c");
        assert_eq!(c, a);
    }

    /// Build a throwaway server bound to a unique temp data dir so the marker
    /// file (rehydrated in `new`) never collides between tests.
    fn test_server() -> OverlaydServer {
        let dir = std::env::temp_dir().join(format!(
            "zlayer-overlayd-scope-{}-{}",
            std::process::id(),
            now_unix()
        ));
        OverlaydServer::new(dir)
    }

    #[tokio::test]
    async fn transport_for_scope_global_requires_setup() {
        let server = test_server();
        // No global overlay set up yet -> Global scope errors. (Can't use
        // `expect_err` because `&OverlayTransport` is not `Debug`.)
        match server.transport_for_scope(&PeerScope::Global) {
            Ok(_) => panic!("global overlay should not be set up"),
            Err(OverlaydError::Other(m)) => {
                assert!(m.contains("global overlay not set up"), "got: {m}");
            }
            Err(other) => panic!("unexpected error: {other:?}"),
        }
    }

    #[tokio::test]
    async fn transport_for_scope_unset_service_errors() {
        let server = test_server();
        match server.transport_for_scope(&PeerScope::Service {
            service: "x".to_string(),
        }) {
            Ok(_) => panic!("no dedicated overlay should exist for x"),
            Err(OverlaydError::Other(m)) => {
                assert_eq!(m, "no dedicated overlay for service x");
            }
            Err(other) => panic!("unexpected error: {other:?}"),
        }
    }

    #[tokio::test]
    async fn add_peer_service_scope_before_setup_errors_via_dispatch() {
        let mut server = test_server();
        let resp = server
            .handle(OverlaydRequest::AddPeer {
                peer: PeerSpec {
                    public_key: "k".to_string(),
                    endpoint: "1.2.3.4:51820".to_string(),
                    allowed_ips: "10.200.0.2/32".to_string(),
                    persistent_keepalive_secs: 0,
                },
                scope: PeerScope::Service {
                    service: "x".to_string(),
                },
            })
            .await;
        match resp {
            OverlaydResponse::Err { message } => {
                assert_eq!(message, "no dedicated overlay for service x");
            }
            other => panic!("expected Err response, got {other:?}"),
        }
    }

    /// End-to-end Dedicated setup. Needs a real TUN device, so it is ignored by
    /// default and only runs on a privileged Linux host (mirrors the crate's
    /// other privileged overlay e2e tests).
    #[cfg(target_os = "linux")]
    #[tokio::test]
    #[ignore = "needs CAP_NET_ADMIN; run on a privileged Linux host"]
    async fn dedicated_setup_creates_distinct_device_and_routes_service_peer() {
        let mut server = test_server();
        // Bring up the global overlay first so the cluster CIDR + global device
        // exist (the dedicated device must get a distinct port and key).
        let global_name = server
            .setup_global_overlay(
                "dep".to_string(),
                "i0".to_string(),
                "10.200.0.0/16",
                Some("10.200.0.0/28"),
                zlayer_core::DEFAULT_WG_PORT,
                false,
            )
            .await
            .expect("global overlay up");
        assert!(!global_name.is_empty());

        // Dedicated service setup.
        let info = server
            .setup_service_overlay("web", OverlayMode::Dedicated)
            .await
            .expect("dedicated service overlay up");
        assert_eq!(info.mode, OverlayMode::Dedicated);
        let port = info.wg_port.expect("dedicated port");
        assert_ne!(
            port, server.overlay_port,
            "dedicated device must not share the global port"
        );

        let st = server
            .service_transports
            .get("web")
            .expect("service transport recorded");
        assert_eq!(st.listen_port, port);
        assert_ne!(
            st.interface, global_name,
            "dedicated interface must differ from global"
        );
        assert_eq!(
            Some(st.public_key.clone()),
            info.wg_public_key,
            "info pubkey matches recorded transport"
        );
        assert_ne!(
            Some(st.public_key.clone()),
            server.transport_public_key,
            "dedicated key must differ from global key"
        );

        // A Service-scoped AddPeer must land on the dedicated device (succeeds),
        // proving scope routing targets the per-service transport.
        let resp = server
            .handle(OverlaydRequest::AddPeer {
                peer: PeerSpec {
                    public_key: {
                        let (_priv, pubk) = OverlayTransport::generate_keys().await.unwrap();
                        pubk
                    },
                    endpoint: "5.6.7.8:51999".to_string(),
                    allowed_ips: "10.201.0.2/32".to_string(),
                    persistent_keepalive_secs: 25,
                },
                scope: PeerScope::Service {
                    service: "web".to_string(),
                },
            })
            .await;
        assert!(
            matches!(resp, OverlaydResponse::Ok),
            "service-scoped add_peer should land on the dedicated device, got {resp:?}"
        );
    }
}
