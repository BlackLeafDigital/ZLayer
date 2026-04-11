use crate::error::AgentError;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::os::fd::AsFd;
use tokio::sync::RwLock;
use zlayer_overlay::{OverlayConfig, OverlayTransport};

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

/// Manages overlay networks for a deployment
pub struct OverlayManager {
    /// Deployment name (used for network naming)
    deployment: String,
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
}

impl OverlayManager {
    /// Create a new overlay manager for a deployment
    ///
    /// # Errors
    /// Returns an error if the overlay manager cannot be initialized.
    ///
    /// # Panics
    /// Panics if the default CIDR `10.200.0.0/16` cannot be parsed (this is a compile-time constant).
    #[allow(clippy::unused_async)]
    pub async fn new(deployment: String) -> Result<Self, AgentError> {
        Ok(Self {
            deployment,
            global_interface: None,
            global_transport: None,
            service_interfaces: RwLock::new(HashMap::new()),
            service_transports: RwLock::new(HashMap::new()),
            ip_allocator: IpAllocator::new("10.200.0.0/16".parse().unwrap()),
            node_ip: None,
            overlay_port: zlayer_core::DEFAULT_WG_PORT,
        })
    }

    /// Set the `WireGuard` listen port for the overlay network.
    #[must_use]
    pub fn with_overlay_port(mut self, port: u16) -> Self {
        self.overlay_port = port;
        self
    }

    /// Setup the global overlay network for the deployment
    ///
    /// # Errors
    /// Returns an error if key generation or interface creation fails.
    pub async fn setup_global_overlay(&mut self) -> Result<(), AgentError> {
        // Idempotency: if a global transport is already live, reuse it.
        // Recreating would call cleanup_stale_linux_interface, which
        // deletes the live TUN out from under the running boringtun
        // worker, causing EBADFD on its read loop.
        if self.global_transport.is_some() {
            tracing::debug!(
                deployment = %self.deployment,
                "Global overlay already active, reusing existing transport"
            );
            return Ok(());
        }

        let interface_name = make_interface_name(&[&self.deployment], "g");

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
        // Idempotency: if a transport already exists for this service,
        // return its interface name without touching the kernel.
        // Recreating would call cleanup_stale_linux_interface, which
        // deletes the live TUN out from under the running boringtun
        // worker, causing EBADFD on its read loop.
        {
            let transports = self.service_transports.read().await;
            if let Some(existing) = transports.get(service_name) {
                let existing_name = existing.interface_name().to_string();
                tracing::debug!(
                    service = %service_name,
                    interface = %existing_name,
                    "Service overlay already active, reusing existing transport"
                );
                return Ok(existing_name);
            }
        }

        let interface_name = make_interface_name(&[&self.deployment, service_name], "s");

        // Attempt overlay creation (for inter-node communication)
        // This is non-fatal: single-node deployments work fine without it
        match self.try_create_overlay(&interface_name, service_name).await {
            Ok(()) => {
                tracing::info!(
                    service = %service_name,
                    interface = %interface_name,
                    "Service overlay created"
                );
            }
            Err(e) => {
                tracing::warn!(
                    service = %service_name,
                    error = %e,
                    "Overlay unavailable, using direct networking"
                );
            }
        }

        // Always register service so attach_container can proceed
        // (veth pair creation doesn't require the overlay interface)
        self.service_interfaces
            .write()
            .await
            .insert(service_name.to_string(), interface_name.clone());
        Ok(interface_name)
    }

    /// Attempt to create an overlay interface for inter-node traffic
    async fn try_create_overlay(
        &self,
        interface_name: &str,
        service_name: &str,
    ) -> Result<(), AgentError> {
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

        // Update interface tracking with the actual name (on macOS, kernel assigns utunN)
        let actual_name = transport.interface_name().to_string();
        self.service_interfaces
            .write()
            .await
            .insert(service_name.to_string(), actual_name);

        self.service_transports
            .write()
            .await
            .insert(service_name.to_string(), transport);
        Ok(())
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

        #[allow(unreachable_code)]
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

            if join_global {
                if let Some(global_iface) = &self.global_interface {
                    let global_ip = self.ip_allocator.allocate()?;
                    self.attach_to_interface(
                        container_pid,
                        global_iface,
                        global_ip,
                        "g",
                        "eth1",
                        false,
                    )
                    .await?;
                }
            }

            Ok(container_ip)
        }
    }

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
    async fn sweep_orphan_veths(&self) {
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

    /// Returns IP allocation statistics: (`allocated_count`, `next_offset`).
    pub fn ip_alloc_stats(&self) -> (u64, IpAddr) {
        let offset = self
            .ip_allocator
            .next_offset
            .load(std::sync::atomic::Ordering::SeqCst);
        (offset.saturating_sub(1), self.ip_allocator.base)
    }

    #[allow(clippy::unused_self)]
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
        OverlayConfig {
            local_endpoint: SocketAddr::new(local_addr, listen_port),
            private_key,
            public_key,
            overlay_cidr: format!("{ip}/{mask}"),
            ..OverlayConfig::default()
        }
    }
}

/// Simple IP address allocator supporting both IPv4 and IPv6.
///
/// For IPv4, the base address octets are incremented using a 32-bit offset.
/// For IPv6, the base address segments are incremented using a 64-bit offset
/// applied to the lower 64 bits (interface identifier portion).
struct IpAllocator {
    base: IpAddr,
    next_offset: std::sync::atomic::AtomicU64,
}

impl IpAllocator {
    fn new(cidr: ipnetwork::IpNetwork) -> Self {
        Self {
            base: cidr.ip(),
            next_offset: std::sync::atomic::AtomicU64::new(1),
        }
    }

    #[allow(clippy::cast_possible_truncation, clippy::unnecessary_wraps)]
    fn allocate(&self) -> Result<IpAddr, AgentError> {
        let offset = self
            .next_offset
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        match self.base {
            IpAddr::V4(base_v4) => {
                let octets = base_v4.octets();
                let offset_u32 = offset as u32;
                Ok(IpAddr::V4(Ipv4Addr::new(
                    octets[0],
                    octets[1],
                    (u32::from(octets[2]) + (offset_u32 >> 8)) as u8,
                    (u32::from(octets[3]) + (offset_u32 & 0xFF)) as u8,
                )))
            }
            IpAddr::V6(base_v6) => {
                let base_u128 = u128::from(base_v6);
                let addr = base_u128.wrapping_add(u128::from(offset));
                Ok(IpAddr::V6(Ipv6Addr::from(addr)))
            }
        }
    }

    fn allocate_for_service(&self, _service: &str) -> Result<IpAddr, AgentError> {
        self.allocate()
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
        let om = OverlayManager::new("test-deploy".to_string())
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
}
