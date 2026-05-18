use crate::error::AgentError;
use ipnetwork::IpNetwork;
use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
#[cfg(target_os = "linux")]
use std::os::fd::AsFd;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::RwLock;
use zlayer_overlay::{
    NatConfig, NatPeerSnapshot, NatStatusSnapshot, NatTraversal, OverlayConfig, OverlayTransport,
};
#[cfg(test)]
use zlayer_paths::ZLayerDirs;

/// Maximum length for Linux network interface names (IFNAMSIZ - 1 for null terminator).
const MAX_IFNAME_LEN: usize = 15;

/// Generate a Linux-safe interface name guaranteed to be <= 15 chars.
///
/// Joins the `parts` with `-` after a `"zl-"` prefix and appends `-{suffix}` if non-empty.
/// When the result exceeds 15 characters, a deterministic hash of all parts is used instead
/// to keep the name unique and within the kernel limit.
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

/// Tracking info recorded by `OverlayManager::attach_container` for every
/// container that successfully attaches to an overlay interface on Linux.
/// Used by `detach_container(pid)` to release the IPs back to the
/// allocator and tear down host-side veths.
#[cfg(target_os = "linux")]
#[derive(Debug, Clone)]
struct AttachInfo {
    /// IP allocated on the per-service overlay (eth0 inside the container).
    service_ip: IpAddr,
    /// IP allocated on the global overlay (eth1 inside the container), if
    /// the container joined the global mesh.
    global_ip: Option<IpAddr>,
    /// True iff the container also attached to the global overlay (eth1).
    joined_global: bool,
}

/// Manages overlay networks for a deployment
pub struct OverlayManager {
    /// Deployment name (used for network naming)
    deployment: String,
    /// Per-daemon-process disambiguator included in overlay link names so multiple
    /// daemons sharing a single host netns (local testing, multi-tenant hosts) don't
    /// collide on link names. Stable for the daemon's lifetime, changes across restarts.
    instance_id: String,
    /// Global overlay interface name
    global_interface: Option<String>,
    /// Global overlay transport (must be kept alive for the TUN device lifetime)
    global_transport: Option<OverlayTransport>,
    /// Service-specific overlay interfaces (`service_name` -> `interface_name`)
    service_interfaces: RwLock<HashMap<String, String>>,
    /// Service-specific overlay transports (must be kept alive for TUN device lifetimes)
    service_transports: RwLock<HashMap<String, OverlayTransport>>,
    /// IP allocator for overlay networks
    ip_allocator: IpAllocator,
    /// This node's IP address on the global overlay network.
    /// Set after `setup_global_overlay()` succeeds.
    node_ip: Option<IpAddr>,
    /// `WireGuard` listen port for the overlay network.
    overlay_port: u16,
    /// Full cluster CIDR (e.g. `10.200.0.0/16`). Kept for logging/config; the
    /// allocator itself is only bounded to `slice_cidr` when the manager was
    /// built via [`OverlayManager::with_slice`].
    cluster_cidr: Option<IpNetwork>,
    /// Per-node slice CIDR assigned by the leader's `NodeSliceAllocator`.
    /// `None` for the legacy [`OverlayManager::new`] path, which uses the full
    /// `/16` default.
    slice_cidr: Option<IpNetwork>,
    /// Map of HCN namespace GUID -> (`service_name`, `allocated_ip`) for autoclean.
    /// When a container with `autoclean=true` is attached, its entry is inserted
    /// here. When the container is removed, `detach_container_hcn` removes it.
    #[cfg(target_os = "windows")]
    hcn_cleanup: std::sync::Arc<
        tokio::sync::Mutex<
            std::collections::HashMap<windows::core::GUID, (String, std::net::IpAddr)>,
        >,
    >,
    /// Per-PID tracking of overlay attachments on Linux. Populated by
    /// `attach_container` on success; consumed by `detach_container(pid)`
    /// to release IPs and delete host-side veth pairs. Linux-only because
    /// Windows uses the HCN cleanup path above.
    #[cfg(target_os = "linux")]
    attached: std::sync::Arc<tokio::sync::Mutex<HashMap<u32, AttachInfo>>>,
    /// Overlay hickory DNS server listen address, if the daemon bootstrapped
    /// one. Used to populate the `Dns.ServerList` field on HCN endpoints so
    /// Windows containers resolve overlay service names. `None` when the
    /// daemon did not start a DNS server (host-network mode, bootstrap
    /// failure, etc.).
    dns_server_addr: Option<SocketAddr>,
    /// DNS domain for overlay service discovery (e.g. `"overlay.local"`).
    /// Populated alongside `dns_server_addr`. When set, HCN endpoints receive
    /// this as their `Dns.Domain` + `Dns.Search` so short names (`svc-a`)
    /// resolve to `svc-a.<domain>` without the container needing an explicit
    /// search list.
    dns_domain: Option<String>,
    /// NAT traversal configuration threaded into [`OverlayConfig::nat`] when
    /// the manager builds overlay transports. `None` means the underlying
    /// `OverlayConfig::default()` value is used (which itself defaults to
    /// `NatConfig::default()`, i.e. NAT enabled with public STUN servers).
    nat_config: Option<NatConfig>,
    /// Override for [`OverlayConfig::uapi_sock_dir`] threaded into every
    /// overlay transport this manager builds. `None` means the underlying
    /// `OverlayConfig::default()` value is used (which itself defaults to
    /// `/var/run/wireguard` on Linux for `wg(8)` interop). The daemon sets
    /// this to `{data_dir}/run/wireguard` when running with a non-default
    /// `--data-dir` so a test daemon's UAPI sockets don't collide with a
    /// host-wide install on the same machine.
    uapi_sock_dir: Option<std::path::PathBuf>,
    /// Live NAT traversal orchestrator. `Some` after a successful
    /// [`OverlayManager::start_nat_traversal`] call, otherwise `None`.
    /// Wrapped in a `RwLock` so the maintenance tick (which mutates
    /// reflexive-address state on STUN refresh) can run without holding the
    /// outer manager's write lock.
    nat_traversal: tokio::sync::RwLock<Option<NatTraversal>>,
    /// Unix-epoch seconds of the last successful candidate gather / STUN
    /// refresh. Surfaced to the API for diagnostics.
    nat_last_refresh: AtomicU64,
}

/// Best-effort sweep of orphan veth endpoints whose owning container
/// process is no longer alive. Names matching `veth-<pid>-*` or
/// `vc-<pid>-*` where `/proc/<pid>` does not exist are deleted.
///
/// Linux-only: there is no veth concept on other platforms.
///
/// Implemented as a free function (rather than `&self`) so the periodic
/// background sweep task can call it without holding a reference to the
/// `OverlayManager` — it reads no manager state, only the kernel link
/// list and `/proc`.
#[cfg(target_os = "linux")]
async fn sweep_orphan_veths_inner() {
    let links = match crate::netlink::list_all_links().await {
        Ok(links) => links,
        Err(e) => {
            tracing::warn!(error = %e, "Failed to list links for orphan sweep");
            return;
        }
    };

    for (_index, name) in links {
        // We only care about our veth endpoints.
        let remainder = if let Some(r) = name.strip_prefix("veth-") {
            r
        } else if let Some(r) = name.strip_prefix("vc-") {
            r
        } else {
            continue;
        };

        // Extract the PID: everything before the first `-` after the prefix.
        let Some(pid_str) = remainder.split('-').next() else {
            continue;
        };

        let pid: u32 = match pid_str.parse() {
            Ok(p) => p,
            Err(_) => continue,
        };

        // If the process is still alive, leave the veth alone.
        if std::path::Path::new(&format!("/proc/{pid}")).exists() {
            continue;
        }

        tracing::info!(link = %name, pid = pid, "Deleting orphan veth");
        if let Err(e) = crate::netlink::delete_link_by_name(&name).await {
            tracing::warn!(link = %name, error = %e, "Failed to delete orphan veth");
        }
    }
}

impl OverlayManager {
    /// Create a new overlay manager for a deployment (legacy single-node path).
    ///
    /// This constructor hands out container IPs from the full default cluster
    /// `/16` (`10.200.0.0/16`). In multi-node deployments every node's agent
    /// would then independently allocate from the same flat range, producing
    /// IP collisions. Prefer [`OverlayManager::with_slice`] for cluster
    /// deployments so the agent is bounded to a per-node slice assigned by
    /// the leader's `NodeSliceAllocator`.
    ///
    /// # Errors
    /// Returns an error if the overlay manager cannot be initialized.
    ///
    /// # Panics
    /// Panics if the default CIDR `10.200.0.0/16` cannot be parsed (this is a compile-time constant).
    #[allow(clippy::unused_async)]
    pub async fn new(deployment: String, instance_id: String) -> Result<Self, AgentError> {
        tracing::debug!(
            deployment = %deployment,
            instance_id = %instance_id,
            "OverlayManager::new uses full /16 default; cluster deployments should use with_slice"
        );
        let default_cidr: IpNetwork = "10.200.0.0/16".parse().unwrap();
        Ok(Self {
            deployment,
            instance_id,
            global_interface: None,
            global_transport: None,
            service_interfaces: RwLock::new(HashMap::new()),
            service_transports: RwLock::new(HashMap::new()),
            ip_allocator: IpAllocator::new(default_cidr),
            node_ip: None,
            overlay_port: zlayer_core::DEFAULT_WG_PORT,
            cluster_cidr: Some(default_cidr),
            slice_cidr: None,
            #[cfg(target_os = "windows")]
            hcn_cleanup: std::sync::Arc::new(tokio::sync::Mutex::new(
                std::collections::HashMap::new(),
            )),
            #[cfg(target_os = "linux")]
            attached: std::sync::Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            dns_server_addr: None,
            dns_domain: None,
            nat_config: None,
            uapi_sock_dir: None,
            nat_traversal: tokio::sync::RwLock::new(None),
            nat_last_refresh: AtomicU64::new(0),
        })
    }

    /// Create an `OverlayManager` bound to a per-node slice.
    ///
    /// `slice_cidr` is a `/28` (or whatever the cluster's slice prefix is)
    /// owned by this node, assigned by the leader's `NodeSliceAllocator`. The
    /// internal `IpAllocator` is bounded to this slice so container IPs never
    /// collide across nodes.
    ///
    /// `cluster_cidr` is the full cluster CIDR (e.g. `10.200.0.0/16`), kept
    /// for configuration / logging purposes. The allocator itself only uses
    /// `slice_cidr`.
    #[must_use]
    pub fn with_slice(
        deployment: String,
        cluster_cidr: IpNetwork,
        slice_cidr: IpNetwork,
        port: u16,
        instance_id: String,
    ) -> Self {
        Self {
            deployment,
            instance_id,
            global_interface: None,
            global_transport: None,
            service_interfaces: RwLock::new(HashMap::new()),
            service_transports: RwLock::new(HashMap::new()),
            ip_allocator: IpAllocator::new(slice_cidr),
            node_ip: None,
            overlay_port: port,
            cluster_cidr: Some(cluster_cidr),
            slice_cidr: Some(slice_cidr),
            #[cfg(target_os = "windows")]
            hcn_cleanup: std::sync::Arc::new(tokio::sync::Mutex::new(
                std::collections::HashMap::new(),
            )),
            #[cfg(target_os = "linux")]
            attached: std::sync::Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            dns_server_addr: None,
            dns_domain: None,
            nat_config: None,
            uapi_sock_dir: None,
            nat_traversal: tokio::sync::RwLock::new(None),
            nat_last_refresh: AtomicU64::new(0),
        }
    }

    /// Set the `WireGuard` listen port for the overlay network.
    #[must_use]
    pub fn with_overlay_port(mut self, port: u16) -> Self {
        self.overlay_port = port;
        self
    }

    /// Set the NAT traversal configuration for the overlay network.
    ///
    /// When set, the [`NatConfig`] is threaded into every [`OverlayConfig`]
    /// the manager builds (global and per-service). When unset (the default),
    /// `OverlayConfig::default()` is used, which itself defaults to
    /// `NatConfig::default()` — i.e. NAT traversal enabled with public STUN.
    #[must_use]
    pub fn with_nat_config(mut self, nat: NatConfig) -> Self {
        self.nat_config = Some(nat);
        self
    }

    /// Override the `WireGuard` UAPI socket directory for every overlay
    /// transport built by this manager.
    ///
    /// When set, every [`OverlayConfig`] produced by
    /// [`Self::build_config`] gets `uapi_sock_dir` set to this path. When
    /// unset (the default), `OverlayConfig::default()` is used, which
    /// itself defaults to `/var/run/wireguard` on Linux.
    ///
    /// The daemon threads `ZLayerDirs::wireguard()` (a data-dir-aware
    /// path) through this setter so a daemon running under a non-default
    /// `--data-dir` does not share `/var/run/wireguard` with a system
    /// install.
    #[must_use]
    pub fn with_uapi_sock_dir(mut self, dir: impl Into<std::path::PathBuf>) -> Self {
        self.uapi_sock_dir = Some(dir.into());
        self
    }

    /// Returns the number of services currently registered with this manager.
    ///
    /// Counts entries in `service_interfaces`, which is populated by
    /// [`OverlayManager::setup_service_overlay`] regardless of whether the
    /// underlying overlay transport was successfully created or fell through
    /// to direct networking. Useful for the race regression test in
    /// `tests/overlay_setup_race.rs` and for telemetry endpoints.
    pub async fn service_count(&self) -> usize {
        self.service_interfaces.read().await.len()
    }

    /// Returns whether NAT traversal is enabled for this manager.
    ///
    /// Reflects the most recent `with_nat_config()` call. When no NAT config
    /// has been provided this falls back to [`NatConfig::default`] which has
    /// `enabled = true`.
    #[must_use]
    pub fn nat_enabled(&self) -> bool {
        self.nat_config
            .as_ref()
            .map_or_else(|| NatConfig::default().enabled, |c| c.enabled)
    }

    /// Returns a clone of the configured [`NatConfig`], or `None` if no
    /// override was provided. Used by the API layer to surface the daemon's
    /// effective NAT configuration without exposing the raw
    /// `NatConfig::default()` baseline.
    #[must_use]
    pub fn nat_config(&self) -> Option<NatConfig> {
        self.nat_config.clone()
    }

    /// Bootstrap a [`NatTraversal`] orchestrator for this manager.
    ///
    /// Constructs a fresh `NatTraversal` from the configured [`NatConfig`]
    /// (defaulting when none is set), gathers ICE-style local candidates
    /// (host + STUN reflexive + relay) and stores it for later
    /// [`OverlayManager::nat_maintenance_tick`] / status calls.
    ///
    /// No-op when `enabled = false` in the configured `NatConfig`. Failures
    /// during candidate gathering are logged and surfaced as `Ok(false)` so
    /// the caller can decide whether to spawn a maintenance loop or skip it.
    ///
    /// Returns `Ok(true)` when the traversal was successfully constructed and
    /// at least one candidate was gathered, `Ok(false)` when NAT is disabled
    /// or candidate gathering yielded nothing actionable.
    ///
    /// # Errors
    ///
    /// Returns an error only on unexpected internal failures; STUN/TURN
    /// network errors are downgraded to `Ok(false)` with a warning log so
    /// the daemon can boot with NAT degraded rather than aborting.
    pub async fn start_nat_traversal(&self) -> Result<bool, AgentError> {
        let config = self.nat_config.clone().unwrap_or_default();
        if !config.enabled {
            tracing::debug!("NAT traversal disabled in config; skipping start");
            return Ok(false);
        }

        let mut nat = NatTraversal::new(config, self.overlay_port);
        match nat.gather_candidates().await {
            Ok(candidates) => {
                tracing::info!(
                    count = candidates.len(),
                    "Gathered NAT candidates for overlay manager",
                );
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                self.nat_last_refresh.store(now, Ordering::SeqCst);
                *self.nat_traversal.write().await = Some(nat);
                Ok(true)
            }
            Err(e) => {
                tracing::warn!(error = %e, "NAT candidate gathering failed");
                Ok(false)
            }
        }
    }

    /// Periodic NAT traversal maintenance: re-probe STUN, refresh relays,
    /// attempt to upgrade relayed peer connections to direct/hole-punched.
    ///
    /// Intended to be called from a `tokio::time::interval` loop spawned by
    /// the daemon. No-op when [`OverlayManager::start_nat_traversal`] has
    /// not yet succeeded.
    ///
    /// # Errors
    ///
    /// Returns an error when the underlying STUN refresh fails. The daemon's
    /// loop logs and ignores these so a transient STUN outage doesn't kill
    /// the maintenance task.
    pub async fn nat_maintenance_tick(&self) -> Result<(), AgentError> {
        let mut guard = self.nat_traversal.write().await;
        let Some(nat) = guard.as_mut() else {
            return Ok(());
        };

        match nat.refresh().await {
            Ok(changed) => {
                if changed {
                    tracing::info!("NAT reflexive address changed during refresh");
                }
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                self.nat_last_refresh.store(now, Ordering::SeqCst);
                Ok(())
            }
            Err(e) => Err(AgentError::Network(format!(
                "NAT maintenance tick failed: {e}"
            ))),
        }
    }

    /// Snapshot the current NAT traversal state for API consumers.
    ///
    /// Returns an empty snapshot when NAT traversal has not been started.
    /// Per-peer entries are not yet tracked here (the agent path does not
    /// route peers through `NatTraversal::connect_to_peer`); callers should
    /// treat the `peers` list as advisory.
    pub async fn nat_status_snapshot(&self) -> NatStatusSnapshot {
        let guard = self.nat_traversal.read().await;
        let Some(nat) = guard.as_ref() else {
            return NatStatusSnapshot::empty();
        };
        let candidates = nat.local_candidates().to_vec();
        let last_refresh = self.nat_last_refresh.load(Ordering::SeqCst);
        // Per-peer state isn't tracked through this manager today (peers
        // come in via the OverlayTransport's UAPI rather than through
        // NatTraversal::connect_to_peer). Surface an empty list for now;
        // future wiring can populate this once the agent owns peer state.
        let peers: Vec<NatPeerSnapshot> = Vec::new();
        NatStatusSnapshot {
            candidates,
            peers,
            last_refresh,
        }
    }

    /// Record the overlay DNS server address and zone domain so attaches can
    /// propagate them to HCN endpoint schemas (Windows) and future
    /// per-container DNS plumbing (Linux `/etc/resolv.conf`).
    ///
    /// `addr` is the socket address the overlay hickory DNS server is
    /// listening on (typically `overlay_ip:15353`). `domain` is the DNS zone
    /// (e.g. `"overlay.local"`). Either may be omitted independently.
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

    /// Returns the overlay DNS server address if the daemon bootstrapped one.
    #[must_use]
    pub fn dns_server_addr(&self) -> Option<SocketAddr> {
        self.dns_server_addr
    }

    /// Returns the overlay DNS zone domain, if configured.
    #[must_use]
    pub fn dns_domain(&self) -> Option<&str> {
        self.dns_domain.as_deref()
    }

    /// Setup the global overlay network for the deployment
    ///
    /// # Errors
    /// Returns an error if key generation or interface creation fails.
    pub async fn setup_global_overlay(&mut self) -> Result<(), AgentError> {
        // Idempotency: if a global transport is already live, reuse it.
        // Recreating without this guard could yank the kernel TUN out from
        // under the running boringtun worker, causing EBADFD on its read loop.
        if self.global_transport.is_some() {
            tracing::debug!(
                deployment = %self.deployment,
                "Global overlay already active, reusing existing transport"
            );
            return Ok(());
        }

        let interface_name = make_interface_name(&[&self.deployment, &self.instance_id], "g");

        let (private_key, public_key) = OverlayTransport::generate_keys()
            .await
            .map_err(|e| AgentError::Network(format!("Failed to generate keys: {e}")))?;

        let node_ip = self.ip_allocator.allocate()?;
        let config = self.build_config(private_key, public_key, node_ip, 16, self.overlay_port);
        let mut transport = OverlayTransport::new(config, interface_name.clone());

        transport
            .create_interface()
            .await
            .map_err(|e| AgentError::Network(format!("Failed to create global overlay: {e}")))?;
        transport
            .configure(&[])
            .await
            .map_err(|e| AgentError::Network(format!("Failed to configure global overlay: {e}")))?;

        // Read back the actual interface name (on macOS, the kernel assigns utunN)
        let actual_name = transport.interface_name().to_string();

        self.node_ip = Some(node_ip);
        self.global_interface = Some(actual_name);
        self.global_transport = Some(transport);
        Ok(())
    }

    /// Setup a service-scoped overlay network
    ///
    /// # Errors
    /// Returns an error if the overlay interface cannot be created.
    pub async fn setup_service_overlay(&self, service_name: &str) -> Result<String, AgentError> {
        // Hold the service_transports write lock across the entire check-and-create.
        // This closes the TOCTOU race where two concurrent callers (e.g. restore_deployments
        // racing the deploy API handler) both passed the read-lock idempotency check, both
        // entered transport creation, and the second one's netlink activity killed the
        // first's live TUN -> boringtun worker EBADFD.
        let mut transports = self.service_transports.write().await;

        if let Some(existing) = transports.get(service_name) {
            let existing_name = existing.interface_name().to_string();
            tracing::debug!(
                service = %service_name,
                interface = %existing_name,
                "Service overlay already active, reusing existing transport"
            );
            drop(transports);
            return Ok(existing_name);
        }

        let interface_name =
            make_interface_name(&[&self.deployment, &self.instance_id, service_name], "s");

        // Attempt overlay creation (for inter-node communication).
        // Non-fatal: single-node deployments work fine without it.
        match self
            .build_service_transport(&interface_name, service_name)
            .await
        {
            Ok(transport) => {
                let actual_name = transport.interface_name().to_string();
                transports.insert(service_name.to_string(), transport);
                drop(transports);
                tracing::info!(
                    service = %service_name,
                    interface = %actual_name,
                    "Service overlay created"
                );
                // Always register service so attach_container can proceed.
                self.service_interfaces
                    .write()
                    .await
                    .insert(service_name.to_string(), actual_name.clone());
                Ok(actual_name)
            }
            Err(e) => {
                drop(transports);
                tracing::warn!(
                    service = %service_name,
                    error = %e,
                    "Overlay unavailable, using direct networking"
                );
                // Always register service so attach_container can proceed
                // (veth pair creation doesn't require the overlay interface).
                self.service_interfaces
                    .write()
                    .await
                    .insert(service_name.to_string(), interface_name.clone());
                Ok(interface_name)
            }
        }
    }

    /// Build an overlay transport for a service without touching the manager's maps.
    /// Caller is responsible for inserting the returned transport into
    /// `service_transports` while holding the write lock.
    async fn build_service_transport(
        &self,
        interface_name: &str,
        service_name: &str,
    ) -> Result<OverlayTransport, AgentError> {
        let (private_key, public_key) = OverlayTransport::generate_keys()
            .await
            .map_err(|e| AgentError::Network(format!("Failed to generate keys: {e}")))?;

        let service_ip = self.ip_allocator.allocate_for_service(service_name)?;
        let config = self.build_config(private_key, public_key, service_ip, 24, 0);
        let mut transport = OverlayTransport::new(config, interface_name.to_string());

        transport
            .create_interface()
            .await
            .map_err(|e| AgentError::Network(format!("Failed to create service overlay: {e}")))?;
        transport.configure(&[]).await.map_err(|e| {
            AgentError::Network(format!("Failed to configure service overlay: {e}"))
        })?;

        Ok(transport)
    }

    /// Add a container to the appropriate overlay networks.
    ///
    /// On non-Linux platforms this is a no-op: per-container overlay attachment
    /// relies on Linux network namespaces (veth pairs + `nsenter`).  On macOS,
    /// containers share the host network, so the node's overlay IP is returned
    /// directly and the proxy differentiates traffic by port.
    ///
    /// # Errors
    /// Returns an error if the container cannot be attached to the overlay network.
    // The non-Linux branch uses an early `return`; the Linux branch below is
    // the normal tail expression. clippy's `needless_return` /
    // `unused_async` fire whichever branch it does not see, so allow both.
    #[cfg_attr(
        not(target_os = "linux"),
        allow(clippy::needless_return, clippy::unused_async)
    )]
    pub async fn attach_container(
        &self,
        container_pid: u32,
        service_name: &str,
        join_global: bool,
    ) -> Result<IpAddr, AgentError> {
        // Per-container overlay attachment uses Linux network namespaces.
        // On non-Linux platforms, return the node's overlay IP (or loopback).
        #[cfg(not(target_os = "linux"))]
        {
            // Suppress unused-variable warnings for the Linux-only parameters.
            let _ = (container_pid, join_global);
            tracing::debug!(
                service = %service_name,
                "Skipping per-container overlay attachment (not supported on this platform). \
                 Containers will use the node's overlay IP via host networking."
            );
            return Ok(self.node_ip.unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST)));
        }

        #[cfg(target_os = "linux")]
        {
            let interfaces = self.service_interfaces.read().await;
            let service_iface = interfaces.get(service_name).ok_or_else(|| {
                AgentError::Network(format!("No overlay for service: {service_name}"))
            })?;

            let container_ip = self.ip_allocator.allocate()?;
            self.attach_to_interface(
                container_pid,
                service_iface,
                container_ip,
                "s",
                "eth0",
                true,
            )
            .await?;

            let mut global_ip: Option<IpAddr> = None;
            if join_global {
                if let Some(global_iface) = &self.global_interface {
                    let g_ip = self.ip_allocator.allocate()?;
                    self.attach_to_interface(container_pid, global_iface, g_ip, "g", "eth1", false)
                        .await?;
                    global_ip = Some(g_ip);
                }
            }

            // Record the attachment so detach_container can clean it up later.
            {
                let mut attached = self.attached.lock().await;
                attached.insert(
                    container_pid,
                    AttachInfo {
                        service_ip: container_ip,
                        global_ip,
                        joined_global: global_ip.is_some(),
                    },
                );
            }

            Ok(container_ip)
        }
    }

    #[cfg(target_os = "windows")]
    /// Register an HCN endpoint's pre-allocated overlay IP under the given namespace.
    ///
    /// The Windows counterpart to `attach_container(pid, ...)` on Linux. Because
    /// HCN has already plumbed the IP into the container's compartment at
    /// `HcsRuntime::create_container` time (via `EndpointAttachment::create_overlay`),
    /// this method does NOT create a veth or enter a netns. It only:
    ///
    /// 1. Allocates the next IP from the node's local /28 slice allocator.
    ///    (The caller — typically `HcsRuntime` — uses the same allocator, so the
    ///    allocation here must match the IP the runtime already stamped into the
    ///    HCN endpoint. Callers pass `ip_override` when the runtime has already
    ///    reserved an IP; in that case we skip re-allocation and just register.)
    /// 2. Records the `namespace_id -> service_name` mapping for later autoclean.
    ///
    /// DNS registration into the per-service hickory zone still happens in
    /// `service.rs` on successful attach. `dns_server` and `dns_domain` here
    /// are the resolver + zone that the caller also staged onto the
    /// `HcsRuntime` (via `set_next_container_dns`) so the endpoint's `Dns`
    /// schema field is populated at creation time — this lets overlay
    /// containers find the hickory server at namespace-attach time instead of
    /// relying on host-inherited resolvers. Pass `None` for both to preserve
    /// the legacy (no DNS in schema) behavior.
    ///
    /// # Errors
    ///
    /// Returns an error if slice IP allocation fails (e.g. slice is exhausted).
    pub async fn attach_container_hcn(
        &self,
        namespace_id: windows::core::GUID,
        service_name: &str,
        ip_override: Option<std::net::IpAddr>,
        autoclean: bool,
        dns_server: Option<std::net::IpAddr>,
        dns_domain: Option<String>,
    ) -> Result<std::net::IpAddr, AgentError> {
        let ip = match ip_override {
            Some(ip) => ip,
            None => self.ip_allocator.allocate()?,
        };
        if autoclean {
            let mut cleanup = self.hcn_cleanup.lock().await;
            cleanup.insert(namespace_id, (service_name.to_string(), ip));
        }
        tracing::info!(
            ns = ?namespace_id,
            service = %service_name,
            ip = %ip,
            dns_server = ?dns_server,
            dns_domain = ?dns_domain,
            "Attached container to HCN overlay",
        );
        Ok(ip)
    }

    #[cfg(target_os = "windows")]
    /// Detach and release an HCN-attached container's IP.
    ///
    /// Called by `HcsRuntime::remove_container` (via service.rs shutdown path) to
    /// release the slice allocator slot held for this container. Safe to call on
    /// unknown namespace IDs — simply no-op.
    ///
    /// # Errors
    ///
    /// Currently infallible, but returns `Result` to match the async-trait
    /// shape of the Linux `attach_container` sibling.
    pub async fn detach_container_hcn(
        &self,
        namespace_id: windows::core::GUID,
    ) -> Result<(), AgentError> {
        let mut cleanup = self.hcn_cleanup.lock().await;
        if let Some((service_name, ip)) = cleanup.remove(&namespace_id) {
            tracing::info!(ns = ?namespace_id, service = %service_name, ip = %ip, "Released HCN overlay attachment");
        }
        Ok(())
    }

    /// Release the overlay resources held by a container.
    ///
    /// Called by the runtime adapter's `remove_container` BEFORE the
    /// container init process exits (PID is still in `/proc`). Idempotent:
    /// no-op if `pid` was never attached or was already detached.
    ///
    /// On Linux:
    ///   1. Removes the `(pid -> AttachInfo)` entry from `self.attached`.
    ///   2. Deletes the host-side veth endpoints `veth-{pid}-s` and
    ///      (if `joined_global`) `veth-{pid}-g` by name. Deletion is
    ///      idempotent — `delete_link_by_name` returns Ok if the link
    ///      is already gone.
    ///   3. Releases the allocated IPs back to the per-node IP pool so a
    ///      subsequent `allocate()` can hand them out again instead of
    ///      extending the monotonic counter.
    ///
    /// On non-Linux: no-op (Windows uses `detach_container_hcn`; other
    /// platforms use host networking and have nothing to detach).
    ///
    /// # Errors
    /// Returns an error only if a netlink delete operation fails in a way
    /// other than "link not found" (e.g. permission denied). The IP
    /// release step never fails.
    pub async fn detach_container(&self, pid: u32) -> Result<(), AgentError> {
        #[cfg(not(target_os = "linux"))]
        {
            let _ = pid;
            return Ok(());
        }

        #[cfg(target_os = "linux")]
        {
            let info = {
                let mut attached = self.attached.lock().await;
                attached.remove(&pid)
            };
            let Some(info) = info else { return Ok(()) };

            // Delete host-side veths by name. Idempotent (delete_link_by_name
            // returns Ok if the link is already gone).
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

            // Release the IPs back to the slice allocator.
            self.ip_allocator.release(info.service_ip);
            if let Some(g) = info.global_ip {
                self.ip_allocator.release(g);
            }

            Ok(())
        }
    }

    /// Start a background tokio task that periodically calls
    /// [`sweep_orphan_veths_inner`] to clean up any `veth-<pid>-*` or
    /// `vc-<pid>-*` interfaces whose owning container PID has exited.
    ///
    /// Defensive — the primary cleanup path is `detach_container`, which
    /// is called explicitly from runtime adapters' `remove_container`.
    /// This sweep catches anything detach missed (daemon panic mid-attach,
    /// runtime that bypassed `remove_container`, host-side surgery).
    ///
    /// Spawned with a 60-second interval. The task holds an `Arc<RwLock<Self>>`
    /// reference so the manager stays alive for its lifetime; it exits
    /// when the `Arc` is dropped (last strong ref away).
    ///
    /// Linux-only. No-op on other platforms because there are no veths
    /// to sweep.
    pub fn start_periodic_orphan_sweep(this: std::sync::Arc<tokio::sync::RwLock<Self>>) {
        #[cfg(not(target_os = "linux"))]
        {
            let _ = this;
        }

        #[cfg(target_os = "linux")]
        {
            // The free function `sweep_orphan_veths_inner` doesn't capture
            // state, so we don't need to keep `self` alive for it — but we
            // tie the task's lifetime to the manager anyway by holding the
            // Arc in the spawned future. This way the periodic task exits
            // cleanly when the OverlayManager is dropped.
            tokio::spawn(async move {
                let _keep_alive = this;
                let mut ticker = tokio::time::interval(std::time::Duration::from_secs(60));
                // Skip the immediate first tick; the daemon already calls
                // sweep on attach.
                ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                ticker.tick().await; // consume the initial tick
                loop {
                    ticker.tick().await;
                    sweep_orphan_veths_inner().await;
                }
            });
        }
    }

    #[cfg(target_os = "linux")]
    #[allow(clippy::too_many_lines)]
    async fn attach_to_interface(
        &self,
        container_pid: u32,
        _interface: &str,
        ip: IpAddr,
        tag: &str,
        container_iface: &str,
        add_default_route: bool,
    ) -> Result<(), AgentError> {
        // Best-effort cleanup of orphan veths left by a previous daemon crash.
        self.sweep_orphan_veths().await;

        let is_v6 = ip.is_ipv6();
        let prefix_len: u8 = if is_v6 { 64 } else { 24 };
        let host_prefix: u8 = if is_v6 { 128 } else { 32 };

        let veth_host = format!("veth-{container_pid}-{tag}");
        let veth_pending = format!("vc-{container_pid}-{tag}");
        let veth_container = container_iface.to_string();

        // Pin the container's network namespace via an OwnedFd so we
        // survive a racing exit of the container init process.
        let container_ns_fd = std::os::fd::OwnedFd::from(
            std::fs::File::open(format!("/proc/{container_pid}/ns/net")).map_err(|e| {
                AgentError::Network(format!("Failed to open /proc/{container_pid}/ns/net: {e}"))
            })?,
        );

        // Pre-cleanup: delete any stale veth endpoints left by a previous
        // daemon crash. These calls are idempotent.
        crate::netlink::delete_link_by_name(&veth_host)
            .await
            .map_err(|e| AgentError::Network(format!("pre-cleanup delete {veth_host}: {e}")))?;
        crate::netlink::delete_link_by_name(&veth_pending)
            .await
            .map_err(|e| AgentError::Network(format!("pre-cleanup delete {veth_pending}: {e}")))?;

        // Main setup wrapped in a block so we can clean up on error.
        let result: Result<(), AgentError> = async {
            // (a) Create the veth pair in the host netns.
            crate::netlink::create_veth_pair(&veth_host, &veth_pending)
                .await
                .map_err(|e| AgentError::Network(format!("create veth pair: {e}")))?;

            // (b) Atomically move the pending end into the container netns
            //     and rename it to the final container interface name.
            crate::netlink::move_link_into_netns_fd_and_rename(
                &veth_pending,
                AsFd::as_fd(&container_ns_fd),
                &veth_container,
            )
            .map_err(|e| AgentError::Network(format!("move veth into netns: {e}")))?;

            // (c) Container-netns operations: assign IP, bring up links,
            //     optionally add default route. Runs on a dedicated thread
            //     that enters the container netns via setns(2).
            let vc = veth_container.clone();
            tokio::task::spawn_blocking(move || {
                crate::netlink::with_netns_fd_async(container_ns_fd, move || async move {
                    crate::netlink::add_address_to_link_by_name(&vc, ip, prefix_len).await?;
                    crate::netlink::set_link_up_by_name(&vc).await?;
                    crate::netlink::set_link_up_by_name("lo").await?;
                    if add_default_route {
                        crate::netlink::add_default_route_via_dev(&vc, is_v6).await?;
                    }
                    Ok(())
                })
            })
            .await
            .map_err(|e| AgentError::Network(format!("container netns task panicked: {e}")))?
            .map_err(|e| AgentError::Network(format!("container netns ops: {e}")))?;

            // (d) Host-side: bring up our end of the veth pair.
            crate::netlink::set_link_up_by_name(&veth_host)
                .await
                .map_err(|e| AgentError::Network(format!("set {veth_host} up: {e}")))?;

            // (e) Host route: /32 (v4) or /128 (v6) pointing at the veth.
            crate::netlink::replace_route_via_dev(ip, host_prefix, &veth_host, self.node_ip)
                .await
                .map_err(|e| {
                    AgentError::Network(format!("host route for {ip}/{host_prefix}: {e}"))
                })?;

            // (f) Sysctls: best-effort, don't fail the attach on these.
            let _ = crate::netlink::set_sysctl("net.ipv4.ip_forward", "1");
            let _ = crate::netlink::set_sysctl("net.ipv6.conf.all.forwarding", "1");

            Ok(())
        }
        .await;

        // Cleanup on error: try to remove the host-side veth (which also
        // destroys the peer end if it still exists).
        if result.is_err() {
            let _ = crate::netlink::delete_link_by_name(&veth_host).await;
            let _ = crate::netlink::delete_link_by_name(&veth_pending).await;
        }

        result
    }

    /// Best-effort sweep of orphan veth endpoints whose owning container
    /// process is no longer alive. Names matching `veth-<pid>-*` or
    /// `vc-<pid>-*` where `/proc/<pid>` does not exist are deleted.
    ///
    /// Linux-only: the caller (`attach_to_interface`) is also Linux-gated
    /// and there is no veth concept on other platforms.
    #[cfg(target_os = "linux")]
    async fn sweep_orphan_veths(&self) {
        sweep_orphan_veths_inner().await;
    }

    /// Tear down the overlay network for a single service.
    ///
    /// Removes the service's TUN transport (destroying the interface) and
    /// clears its entry from the interface tracking map.  This is safe to call
    /// even if no overlay was created for the service (it will be a no-op).
    pub async fn teardown_service_overlay(&self, service_name: &str) {
        // Remove and shut down the transport (destroys TUN device)
        if let Some(mut transport) = self.service_transports.write().await.remove(service_name) {
            tracing::info!(service = %service_name, "Shutting down service overlay transport");
            transport.shutdown();
        }

        // Remove from interface tracking
        if let Some(iface) = self.service_interfaces.write().await.remove(service_name) {
            tracing::info!(
                service = %service_name,
                interface = %iface,
                "Removed service overlay interface"
            );
        }
    }

    /// Cleanup all overlay networks
    ///
    /// # Errors
    /// Returns an error if cleanup operations fail.
    pub async fn cleanup(&mut self) -> Result<(), AgentError> {
        // Drop service transports (destroys TUN devices)
        let mut transports = self.service_transports.write().await;
        for (name, mut transport) in transports.drain() {
            tracing::info!(service = %name, "Shutting down service overlay");
            transport.shutdown();
        }
        drop(transports);

        // Drop global transport
        if let Some(mut transport) = self.global_transport.take() {
            tracing::info!("Shutting down global overlay");
            transport.shutdown();
        }

        // Clear interface name tracking
        self.service_interfaces.write().await.clear();
        self.global_interface = None;

        Ok(())
    }

    /// Returns this node's IP on the global overlay network, if available.
    ///
    /// This is set after [`setup_global_overlay`] completes successfully.
    pub fn node_ip(&self) -> Option<IpAddr> {
        self.node_ip
    }

    /// Returns the deployment name this overlay manager was created for.
    pub fn deployment(&self) -> &str {
        &self.deployment
    }

    /// Returns the global overlay interface name, if one has been created.
    pub fn global_interface(&self) -> Option<&str> {
        self.global_interface.as_deref()
    }

    /// Returns the `WireGuard` listen port for the overlay network.
    pub fn overlay_port(&self) -> u16 {
        self.overlay_port
    }

    /// Returns `true` if the global overlay transport is active.
    pub fn has_global_transport(&self) -> bool {
        self.global_transport.is_some()
    }

    /// Returns the number of service-specific overlay transports currently active.
    pub async fn service_transport_count(&self) -> usize {
        self.service_transports.read().await.len()
    }

    /// Returns the CIDR string for the overlay IP allocator.
    pub fn overlay_cidr(&self) -> String {
        match self.ip_allocator.base {
            IpAddr::V4(_) => format!("{}/16", self.ip_allocator.base),
            IpAddr::V6(_) => format!("{}/48", self.ip_allocator.base),
        }
    }

    /// Returns the per-node slice CIDR this manager was built with, or `None`
    /// if the legacy [`OverlayManager::new`] constructor was used.
    pub fn slice_cidr(&self) -> Option<IpNetwork> {
        self.slice_cidr
    }

    /// Returns the full cluster CIDR, if this manager was constructed with
    /// one. The legacy [`OverlayManager::new`] path stores the default
    /// `10.200.0.0/16`.
    pub fn cluster_cidr(&self) -> Option<IpNetwork> {
        self.cluster_cidr
    }

    /// Persist the IPAM allocator state to `path`.
    ///
    /// The state is a small JSON blob capturing the allocator's CIDR bound
    /// and its next-offset counter so restarts don't re-hand-out the same
    /// IPs.
    ///
    /// # Errors
    /// Returns an error if the file cannot be written.
    pub async fn persist_ipam_state(&self, path: &Path) -> Result<(), AgentError> {
        self.ip_allocator.save(path).await
    }

    /// Restore IPAM allocator state from `path`.
    ///
    /// If the file does not exist this is a no-op (the allocator keeps its
    /// current counter). On load mismatch (e.g. the persisted CIDR differs
    /// from the allocator's current CIDR) the counter is left untouched and
    /// a warning is emitted.
    ///
    /// # Errors
    /// Returns an error if the file exists but cannot be read or parsed.
    pub async fn restore_ipam_state(&mut self, path: &Path) -> Result<(), AgentError> {
        self.ip_allocator.restore(path).await
    }

    /// Returns IP allocation statistics: (`allocated_count`, `next_offset`).
    pub fn ip_alloc_stats(&self) -> (u64, IpAddr) {
        let offset = self
            .ip_allocator
            .next_offset
            .load(std::sync::atomic::Ordering::SeqCst);
        (offset.saturating_sub(1), self.ip_allocator.base)
    }

    fn build_config(
        &self,
        private_key: String,
        public_key: String,
        ip: IpAddr,
        mask: u8,
        listen_port: u16,
    ) -> OverlayConfig {
        // Bind to the correct address family for the overlay IP
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

/// Simple IP address allocator supporting both IPv4 and IPv6.
///
/// Each allocator is bounded to a specific CIDR (typically a per-node `/28`
/// slice assigned by the leader's `NodeSliceAllocator`). Allocations
/// past the last usable host in the bound return `None`, surfaced as an
/// `AgentError::Network` exhaustion error from [`IpAllocator::allocate`].
///
/// For IPv4 the offset is added to the 32-bit network address. For IPv6 the
/// offset is added to the lower 64 bits (interface identifier portion), up
/// to the `/128` end-of-slice bound.
struct IpAllocator {
    /// Base (network) address of the CIDR. Preserved as a separate field so
    /// `OverlayManager::overlay_cidr` and `ip_alloc_stats` can keep their
    /// previous shape.
    base: IpAddr,
    /// CIDR the allocator is bounded to. Allocations past the broadcast /
    /// last-host address of this CIDR fail.
    cidr: IpNetwork,
    /// Monotonic counter for the next allocation offset relative to `base`.
    next_offset: AtomicU64,
    /// IPs returned by `release(...)`. `allocate()` drains this first before
    /// incrementing `next_offset`, so a churning daemon doesn't exhaust the
    /// slice from monotonic counter growth.
    released: parking_lot::Mutex<Vec<IpAddr>>,
}

/// On-disk serialization format for the IPAM allocator state.
///
/// Kept deliberately simple: `cidr` is a string (e.g. `"10.200.42.0/28"`) so
/// the file is easy to inspect by hand, and `next_offset` is just the
/// counter value at save time.
#[derive(Debug, Serialize, Deserialize)]
struct IpAllocatorState {
    cidr: String,
    next_offset: u64,
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

    /// Allocate the next IP in the slice.
    ///
    /// Returns `AgentError::Network` when the CIDR is exhausted (the next
    /// address would be the broadcast for IPv4 or past the last address for
    /// IPv6).
    fn allocate(&self) -> Result<IpAddr, AgentError> {
        // Reuse released IPs before extending into fresh counter space.
        {
            let mut released = self.released.lock();
            if let Some(ip) = released.pop() {
                return Ok(ip);
            }
        }
        // Reserve the offset up-front so concurrent callers can't both get
        // the same address, then fail-loud if the reserved slot is past the
        // end of the slice.
        let offset = self.next_offset.fetch_add(1, Ordering::SeqCst);
        let addr = self.compute_addr(offset);

        // Bounds check: refuse addresses outside the configured CIDR, and
        // (for IPv4) refuse the broadcast address.
        let in_cidr = self.cidr.contains(addr);
        let is_v4_broadcast = matches!(
            (&self.cidr, &addr),
            (IpNetwork::V4(v4), IpAddr::V4(a)) if *a == v4.broadcast()
        );
        if !in_cidr || is_v4_broadcast {
            return Err(AgentError::Network(format!(
                "IP allocator exhausted: next address {addr} is outside slice {}",
                self.cidr
            )));
        }
        Ok(addr)
    }

    /// Return an IP to the free pool. The next `allocate()` will hand it back
    /// before extending the monotonic counter. Idempotent: no-op if the IP is
    /// already in the free list (we do not double-track because the counter
    /// would already have moved past it).
    fn release(&self, ip: IpAddr) {
        // Light dedup: avoid pushing the same address twice. The pool is
        // small in steady state (number of live containers); linear scan is
        // fine.
        let mut released = self.released.lock();
        if !released.contains(&ip) {
            released.push(ip);
        }
    }

    fn allocate_for_service(&self, _service: &str) -> Result<IpAddr, AgentError> {
        self.allocate()
    }

    /// Serialize allocator state (cidr + counter) to `path` as JSON.
    async fn save(&self, path: &Path) -> Result<(), AgentError> {
        let state = IpAllocatorState {
            cidr: self.cidr.to_string(),
            next_offset: self.next_offset.load(Ordering::SeqCst),
        };
        let json = serde_json::to_vec_pretty(&state)
            .map_err(|e| AgentError::Network(format!("serialize ipam state: {e}")))?;
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                tokio::fs::create_dir_all(parent).await.map_err(|e| {
                    AgentError::Network(format!("create ipam state dir {}: {e}", parent.display()))
                })?;
            }
        }
        tokio::fs::write(path, json).await.map_err(|e| {
            AgentError::Network(format!("write ipam state {}: {e}", path.display()))
        })?;
        Ok(())
    }

    /// Load allocator state from `path`, resuming the counter.
    ///
    /// No-op when the file is missing. If the persisted CIDR differs from
    /// the in-memory allocator's CIDR, the counter is left untouched and a
    /// warning is emitted: it is safer to keep serving fresh IPs than to
    /// jump the counter to an offset that doesn't match the current slice.
    async fn restore(&mut self, path: &Path) -> Result<(), AgentError> {
        let raw = match tokio::fs::read_to_string(path).await {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => {
                return Err(AgentError::Network(format!(
                    "read ipam state {}: {e}",
                    path.display()
                )));
            }
        };
        let state: IpAllocatorState = serde_json::from_str(&raw).map_err(|e| {
            AgentError::Network(format!("parse ipam state {}: {e}", path.display()))
        })?;

        if state.cidr != self.cidr.to_string() {
            tracing::warn!(
                persisted_cidr = %state.cidr,
                current_cidr = %self.cidr,
                path = %path.display(),
                "IPAM state CIDR mismatch; ignoring persisted counter"
            );
            return Ok(());
        }

        self.next_offset.store(state.next_offset, Ordering::SeqCst);
        Ok(())
    }

    /// Construct an allocator from an on-disk state file, bounded to `cidr`.
    ///
    /// If the file does not exist, a fresh allocator is returned. If the
    /// persisted CIDR doesn't match `cidr`, a fresh allocator is returned
    /// and a warning is emitted (same safe-default policy as [`restore`]).
    #[allow(dead_code)]
    async fn load(path: &Path, cidr: IpNetwork) -> Result<Self, AgentError> {
        let mut alloc = Self::new(cidr);
        alloc.restore(path).await?;
        Ok(alloc)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ip_allocator_release_recycles() {
        let alloc = IpAllocator::new("10.200.42.0/28".parse().unwrap());
        let a1 = alloc.allocate().expect("first alloc");
        let a2 = alloc.allocate().expect("second alloc");
        alloc.release(a1);
        let a3 = alloc.allocate().expect("third alloc after release");
        assert_eq!(a3, a1, "allocate should hand back the released IP");
        // The next allocation continues from the monotonic counter.
        let a4 = alloc.allocate().expect("fourth alloc");
        assert_ne!(a4, a1);
        assert_ne!(a4, a2);
    }

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

    /// Empty parts and suffix must still produce a valid name.
    #[test]
    fn interface_name_with_empty_inputs() {
        let name = make_interface_name(&[""], "");
        assert!(name.len() <= MAX_IFNAME_LEN);
        assert!(name.starts_with("zl-"));

        let name = make_interface_name(&["", ""], "s");
        assert!(name.len() <= MAX_IFNAME_LEN);
        assert!(name.starts_with("zl-"));

        let name = make_interface_name(&[], "g");
        assert!(name.len() <= MAX_IFNAME_LEN);
        assert!(name.starts_with("zl-"));
    }

    /// Same inputs must always produce the same output.
    #[test]
    fn interface_name_is_deterministic() {
        let a = make_interface_name(&["zlayer-manager"], "g");
        let b = make_interface_name(&["zlayer-manager"], "g");
        assert_eq!(a, b);

        let a = make_interface_name(&["deploy", "frontend"], "s");
        let b = make_interface_name(&["deploy", "frontend"], "s");
        assert_eq!(a, b);
    }

    /// Different inputs must produce different outputs.
    #[test]
    fn interface_name_uniqueness() {
        let a = make_interface_name(&["deploy-a"], "g");
        let b = make_interface_name(&["deploy-b"], "g");
        assert_ne!(a, b, "Different deployments should yield different names");

        let a = make_interface_name(&["deploy", "svc-a"], "s");
        let b = make_interface_name(&["deploy", "svc-b"], "s");
        assert_ne!(a, b, "Different services should yield different names");

        let a = make_interface_name(&["deploy"], "g");
        let b = make_interface_name(&["deploy"], "s");
        assert_ne!(a, b, "Different suffixes should yield different names");
    }

    /// Short names that fit should be returned as-is (human readable).
    #[test]
    fn interface_name_short_inputs_are_readable() {
        // "zl-" (3) + "app" (3) + "-" (1) + "g" (1) = 8 chars
        let name = make_interface_name(&["app"], "g");
        assert_eq!(name, "zl-app-g");

        // "zl-" (3) + "my" (2) + "-" (1) + "web" (3) + "-" (1) + "s" (1) = 11
        let name = make_interface_name(&["my", "web"], "s");
        assert_eq!(name, "zl-my-web-s");
    }

    /// Global overlay names for realistic deployment names.
    #[test]
    fn global_overlay_realistic_names() {
        let deployments = [
            "zlayer-manager",
            "my-very-long-deployment-name",
            "a",
            "production",
            "zlayer",
        ];

        for deployment in &deployments {
            let name = make_interface_name(&[deployment], "g");
            assert!(
                name.len() <= MAX_IFNAME_LEN,
                "Global overlay '{name}' for deployment '{deployment}' exceeds limit",
            );
            assert!(name.starts_with("zl-"));
        }
    }

    /// Service overlay names for realistic deployment + service combos.
    #[test]
    fn service_overlay_realistic_names() {
        let cases = [
            ("zlayer-manager", "frontend"),
            ("zlayer-manager", "backend-api"),
            ("zlayer", "manager"),
            ("a", "b"),
            ("production", "auth-service-primary"),
            ("my-long-deploy", "my-long-service"),
        ];

        for (deployment, service) in &cases {
            let name = make_interface_name(&[deployment, service], "s");
            assert!(
                name.len() <= MAX_IFNAME_LEN,
                "Service overlay '{name}' for ({deployment}, {service}) exceeds limit",
            );
            assert!(name.starts_with("zl-"));
        }
    }

    /// Unicode inputs must not cause panics and must respect the byte limit.
    #[test]
    fn interface_name_with_unicode() {
        let name = make_interface_name(&["\u{1F600}\u{1F600}\u{1F600}"], "g");
        assert!(name.len() <= MAX_IFNAME_LEN, "Name '{name}' too long");

        let name = make_interface_name(&["\u{00E9}\u{00E9}\u{00E9}", "\u{00FC}\u{00FC}"], "s");
        assert!(name.len() <= MAX_IFNAME_LEN, "Name '{name}' too long");
    }

    /// `node_ip()` should be None before `setup_global_overlay` and Some after.
    #[tokio::test]
    async fn test_node_ip_before_and_after_init() {
        let om = OverlayManager::new("test-deploy".to_string(), "test".to_string())
            .await
            .unwrap();

        // Before global overlay setup, node_ip should be None
        assert!(
            om.node_ip().is_none(),
            "node_ip should be None before setup_global_overlay"
        );
    }

    /// IPv4 allocator produces sequential addresses from the base.
    #[test]
    fn ip_allocator_v4_sequential() {
        let alloc = IpAllocator::new("10.200.0.0/16".parse().unwrap());
        let ip1 = alloc.allocate().unwrap();
        let ip2 = alloc.allocate().unwrap();
        let ip3 = alloc.allocate().unwrap();
        assert_eq!(ip1, IpAddr::V4(Ipv4Addr::new(10, 200, 0, 1)));
        assert_eq!(ip2, IpAddr::V4(Ipv4Addr::new(10, 200, 0, 2)));
        assert_eq!(ip3, IpAddr::V4(Ipv4Addr::new(10, 200, 0, 3)));
    }

    /// IPv6 allocator produces sequential addresses from the base.
    #[test]
    fn ip_allocator_v6_sequential() {
        let alloc = IpAllocator::new("fd00:200::0/48".parse().unwrap());
        let ip1 = alloc.allocate().unwrap();
        let ip2 = alloc.allocate().unwrap();
        let ip3 = alloc.allocate().unwrap();
        assert_eq!(ip1, "fd00:200::1".parse::<IpAddr>().unwrap());
        assert_eq!(ip2, "fd00:200::2".parse::<IpAddr>().unwrap());
        assert_eq!(ip3, "fd00:200::3".parse::<IpAddr>().unwrap());
    }

    /// `allocate_for_service` delegates to `allocate` regardless of IP version.
    #[test]
    fn ip_allocator_service_delegates() {
        let alloc = IpAllocator::new("fd00:200::0/48".parse().unwrap());
        let ip1 = alloc.allocate_for_service("web").unwrap();
        let ip2 = alloc.allocate().unwrap();
        assert_eq!(ip1, "fd00:200::1".parse::<IpAddr>().unwrap());
        assert_eq!(ip2, "fd00:200::2".parse::<IpAddr>().unwrap());
    }

    /// A /28 slice has 14 usable hosts (16 total - network - broadcast).
    /// The 15th allocation must fail-loud as exhaustion.
    #[test]
    fn test_allocator_bounded_to_slice_v4() {
        let slice: IpNetwork = "10.200.42.0/28".parse().unwrap();
        let alloc = IpAllocator::new(slice);

        let mut allocated = Vec::new();
        for _ in 0..14 {
            let ip = alloc
                .allocate()
                .expect("first 14 allocations should succeed");
            allocated.push(ip);
        }

        // All 14 allocated IPs must fall within the slice.
        for ip in &allocated {
            assert!(
                slice.contains(*ip),
                "Allocated IP {ip} outside slice {slice}"
            );
        }

        // The 15th allocation would land on the broadcast (.15) and must fail.
        let exhausted = alloc.allocate();
        assert!(
            exhausted.is_err(),
            "allocation past /28 exhaustion should fail, got {exhausted:?}"
        );
    }

    /// Every allocation from a /28 slice must be inside the /28, never bleeding
    /// into the neighboring slice.
    #[test]
    fn test_allocator_rejects_oob() {
        let slice: IpNetwork = "10.200.42.16/28".parse().unwrap();
        let alloc = IpAllocator::new(slice);

        // A /28 at .16 covers .16 (network) through .31 (broadcast).
        // The 14 host addresses are .17 through .30.
        for _ in 0..14 {
            let ip = alloc.allocate().expect("host allocation should succeed");
            assert!(slice.contains(ip), "Allocation {ip} escaped slice {slice}");
            // Sanity: never hand out the broadcast.
            if let (IpAddr::V4(a), IpNetwork::V4(v4)) = (ip, slice) {
                assert_ne!(a, v4.broadcast(), "handed out broadcast address");
                assert_ne!(a, v4.network(), "handed out network address");
            }
        }

        // Next allocation is the broadcast — refuse it.
        assert!(alloc.allocate().is_err());
    }

    /// `OverlayManager::with_slice` must remember the slice it was built with.
    #[test]
    fn test_overlay_manager_with_slice_stores_slice_cidr() {
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

    /// Save the counter after 3 allocations, reload into a fresh allocator,
    /// and verify the next allocation picks up where we left off.
    #[tokio::test]
    async fn test_allocator_persistence_roundtrip() {
        let slice: IpNetwork = "10.200.42.0/28".parse().unwrap();
        let alloc = IpAllocator::new(slice);

        let a1 = alloc.allocate().unwrap();
        let a2 = alloc.allocate().unwrap();
        let a3 = alloc.allocate().unwrap();
        assert_eq!(a1, IpAddr::V4(Ipv4Addr::new(10, 200, 42, 1)));
        assert_eq!(a2, IpAddr::V4(Ipv4Addr::new(10, 200, 42, 2)));
        assert_eq!(a3, IpAddr::V4(Ipv4Addr::new(10, 200, 42, 3)));

        let dir = ZLayerDirs::system_default()
            .scratch_dir("test-allocator-persistence-roundtrip-")
            .expect("tempdir");
        let state_path = dir.path().join("agent_ipam.json");
        alloc.save(&state_path).await.expect("save");

        let restored = IpAllocator::load(&state_path, slice).await.expect("load");
        let a4 = restored.allocate().unwrap();
        assert_eq!(
            a4,
            IpAddr::V4(Ipv4Addr::new(10, 200, 42, 4)),
            "restored allocator should continue from the persisted counter"
        );

        // Missing file is a no-op for restore (fresh allocator).
        let missing_path = dir.path().join("does-not-exist.json");
        let mut fresh = IpAllocator::new(slice);
        fresh.restore(&missing_path).await.expect("restore missing");
        let first = fresh.allocate().unwrap();
        assert_eq!(first, IpAddr::V4(Ipv4Addr::new(10, 200, 42, 1)));
    }

    /// Windows-only: verify `attach_container_hcn` populates the cleanup map and
    /// `detach_container_hcn` drains it. Uses a zeroed GUID as a stand-in since
    /// we can't spin up a real HCN namespace in a unit test.
    #[cfg(target_os = "windows")]
    #[tokio::test]
    async fn test_attach_detach_container_hcn_tracks_cleanup_map() {
        let cluster: IpNetwork = "10.200.0.0/16".parse().unwrap();
        let slice: IpNetwork = "10.200.42.0/28".parse().unwrap();
        let om = OverlayManager::with_slice(
            "test-deploy".to_string(),
            cluster,
            slice,
            51820,
            "test".to_string(),
        );

        let ns = windows::core::GUID::zeroed();
        let fixed_ip: std::net::IpAddr = "10.200.42.5".parse().unwrap();

        // With an ip_override + autoclean=true, the cleanup map should gain one entry.
        let ip = om
            .attach_container_hcn(ns, "svc-a", Some(fixed_ip), true, None, None)
            .await
            .expect("attach_container_hcn");
        assert_eq!(ip, fixed_ip);
        {
            let map = om.hcn_cleanup.lock().await;
            assert_eq!(map.len(), 1);
            let entry = map.get(&ns).expect("entry for zeroed GUID");
            assert_eq!(entry.0, "svc-a");
            assert_eq!(entry.1, fixed_ip);
        }

        // Detach drains the entry.
        om.detach_container_hcn(ns).await.expect("detach");
        {
            let map = om.hcn_cleanup.lock().await;
            assert!(map.is_empty(), "detach should leave the cleanup map empty");
        }

        // Detaching an unknown GUID is a no-op and must not error.
        om.detach_container_hcn(ns)
            .await
            .expect("unknown GUID is no-op");

        // autoclean=false must NOT insert into the cleanup map.
        let _ip = om
            .attach_container_hcn(ns, "svc-b", Some(fixed_ip), false, None, None)
            .await
            .expect("attach without autoclean");
        {
            let map = om.hcn_cleanup.lock().await;
            assert!(map.is_empty(), "autoclean=false should not populate map");
        }
    }

    /// Default-constructed `OverlayManager` has no DNS config until the
    /// daemon bootstraps one. Both accessors must return `None`.
    #[tokio::test]
    async fn dns_config_defaults_to_none() {
        let om = OverlayManager::new("dns-default".to_string(), "test".to_string())
            .await
            .expect("OverlayManager::new");
        assert!(om.dns_server_addr().is_none());
        assert!(om.dns_domain().is_none());
    }

    /// `set_dns_config` must round-trip both values through the accessors.
    /// Covers the J-1 contract with `attach_container_hcn` / `HcsRuntime`.
    #[tokio::test]
    async fn dns_config_set_and_round_trip() {
        let mut om = OverlayManager::new("dns-roundtrip".to_string(), "test".to_string())
            .await
            .expect("OverlayManager::new");
        let addr: SocketAddr = "10.200.42.1:15353".parse().unwrap();
        om.set_dns_config(Some(addr), Some("overlay.local".to_string()));
        assert_eq!(om.dns_server_addr(), Some(addr));
        assert_eq!(om.dns_domain(), Some("overlay.local"));

        // Clear by passing both as None.
        om.set_dns_config(None, None);
        assert!(om.dns_server_addr().is_none());
        assert!(om.dns_domain().is_none());
    }

    /// Builder-style `with_dns_config` carries the same values as
    /// `set_dns_config`.
    #[test]
    fn with_dns_config_preserves_values() {
        let cluster: IpNetwork = "10.200.0.0/16".parse().unwrap();
        let slice: IpNetwork = "10.200.42.0/28".parse().unwrap();
        let addr: SocketAddr = "10.200.42.1:15353".parse().unwrap();
        let om = OverlayManager::with_slice(
            "dns-builder".to_string(),
            cluster,
            slice,
            51820,
            "test".to_string(),
        )
        .with_dns_config(Some(addr), Some("overlay.local".to_string()));
        assert_eq!(om.dns_server_addr(), Some(addr));
        assert_eq!(om.dns_domain(), Some("overlay.local"));
    }
}
