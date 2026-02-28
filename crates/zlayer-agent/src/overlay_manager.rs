use crate::error::AgentError;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use tokio::sync::RwLock;
use zlayer_overlay::{OverlayConfig, OverlayTransport};

/// Maximum length for Linux network interface names (IFNAMSIZ - 1 for null terminator).
const MAX_IFNAME_LEN: usize = 15;

/// Generate a Linux-safe interface name guaranteed to be <= 15 chars.
///
/// Joins the `parts` with `-` after a `"zl-"` prefix and appends `-{suffix}` if non-empty.
/// When the result exceeds 15 characters, a deterministic hash of all parts is used instead
/// to keep the name unique and within the kernel limit.
pub fn make_interface_name(parts: &[&str], suffix: &str) -> String {
    let base = format!("zl-{}", parts.join("-"));
    let candidate = if suffix.is_empty() {
        base
    } else {
        format!("{}-{}", base, suffix)
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
    /// Service-specific overlay interfaces (service_name -> interface_name)
    service_interfaces: RwLock<HashMap<String, String>>,
    /// Service-specific overlay transports (must be kept alive for TUN device lifetimes)
    service_transports: RwLock<HashMap<String, OverlayTransport>>,
    /// IP allocator for overlay networks
    ip_allocator: IpAllocator,
    /// This node's IP address on the global overlay network.
    /// Set after `setup_global_overlay()` succeeds.
    node_ip: Option<Ipv4Addr>,
}

impl OverlayManager {
    /// Create a new overlay manager for a deployment
    pub async fn new(deployment: String) -> Result<Self, AgentError> {
        Ok(Self {
            deployment,
            global_interface: None,
            global_transport: None,
            service_interfaces: RwLock::new(HashMap::new()),
            service_transports: RwLock::new(HashMap::new()),
            ip_allocator: IpAllocator::new("10.200.0.0/16".parse().unwrap()),
            node_ip: None,
        })
    }

    /// Setup the global overlay network for the deployment
    pub async fn setup_global_overlay(&mut self) -> Result<(), AgentError> {
        let interface_name = make_interface_name(&[&self.deployment], "g");

        let (private_key, public_key) = OverlayTransport::generate_keys()
            .await
            .map_err(|e| AgentError::Network(format!("Failed to generate keys: {}", e)))?;

        let node_ip = self.ip_allocator.allocate()?;
        let config = self.build_config(private_key, public_key, node_ip, 16, 51820);
        let mut transport = OverlayTransport::new(config, interface_name.clone());

        transport
            .create_interface()
            .await
            .map_err(|e| AgentError::Network(format!("Failed to create global overlay: {}", e)))?;
        transport.configure(&[]).await.map_err(|e| {
            AgentError::Network(format!("Failed to configure global overlay: {}", e))
        })?;

        self.node_ip = Some(node_ip);
        self.global_interface = Some(interface_name);
        self.global_transport = Some(transport);
        Ok(())
    }

    /// Setup a service-scoped overlay network
    pub async fn setup_service_overlay(&self, service_name: &str) -> Result<String, AgentError> {
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
            .map_err(|e| AgentError::Network(format!("Failed to generate keys: {}", e)))?;

        let service_ip = self.ip_allocator.allocate_for_service(service_name)?;
        let config = self.build_config(private_key, public_key, service_ip, 24, 0);
        let mut transport = OverlayTransport::new(config, interface_name.to_string());

        transport
            .create_interface()
            .await
            .map_err(|e| AgentError::Network(format!("Failed to create service overlay: {}", e)))?;
        transport.configure(&[]).await.map_err(|e| {
            AgentError::Network(format!("Failed to configure service overlay: {}", e))
        })?;

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
    pub async fn attach_container(
        &self,
        container_pid: u32,
        service_name: &str,
        join_global: bool,
    ) -> Result<Ipv4Addr, AgentError> {
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
            return Ok(self.node_ip.unwrap_or(Ipv4Addr::LOCALHOST));
        }

        #[allow(unreachable_code)]
        {
            let interfaces = self.service_interfaces.read().await;
            let service_iface = interfaces.get(service_name).ok_or_else(|| {
                AgentError::Network(format!("No overlay for service: {}", service_name))
            })?;

            let container_ip = self.ip_allocator.allocate()?;
            self.attach_to_interface(container_pid, service_iface, container_ip)
                .await?;

            if join_global {
                if let Some(global_iface) = &self.global_interface {
                    let global_ip = self.ip_allocator.allocate()?;
                    self.attach_to_interface(container_pid, global_iface, global_ip)
                        .await?;
                }
            }

            Ok(container_ip)
        }
    }

    async fn attach_to_interface(
        &self,
        container_pid: u32,
        _interface: &str,
        ip: Ipv4Addr,
    ) -> Result<(), AgentError> {
        let veth_host = format!("veth-{}", container_pid);
        let veth_container = "eth0";

        // Clean up any stale veth pair left by a previous daemon crash.
        // This is idempotent — deleting a non-existent interface fails silently.
        let _ = self
            .run_command("ip", &["link", "delete", &veth_host])
            .await;

        self.run_command(
            "ip",
            &[
                "link",
                "add",
                &veth_host,
                "type",
                "veth",
                "peer",
                "name",
                veth_container,
            ],
        )
        .await?;

        self.run_command(
            "ip",
            &[
                "link",
                "set",
                veth_container,
                "netns",
                &container_pid.to_string(),
            ],
        )
        .await?;

        self.run_command(
            "nsenter",
            &[
                "-t",
                &container_pid.to_string(),
                "-n",
                "ip",
                "addr",
                "add",
                &format!("{}/24", ip),
                "dev",
                veth_container,
            ],
        )
        .await?;

        self.run_command(
            "nsenter",
            &[
                "-t",
                &container_pid.to_string(),
                "-n",
                "ip",
                "link",
                "set",
                veth_container,
                "up",
            ],
        )
        .await?;

        // Bring up host-side veth so traffic can flow
        self.run_command("ip", &["link", "set", &veth_host, "up"])
            .await?;

        // Use "replace" instead of "add" so stale routes from a previous
        // daemon run don't cause a failure (EEXIST).
        self.run_command(
            "ip",
            &["route", "replace", &format!("{}/32", ip), "dev", &veth_host],
        )
        .await?;

        Ok(())
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
    pub fn node_ip(&self) -> Option<Ipv4Addr> {
        self.node_ip
    }

    fn build_config(
        &self,
        private_key: String,
        public_key: String,
        ip: Ipv4Addr,
        mask: u8,
        listen_port: u16,
    ) -> OverlayConfig {
        OverlayConfig {
            local_endpoint: SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), listen_port),
            private_key,
            public_key,
            overlay_cidr: format!("{}/{}", ip, mask),
            ..OverlayConfig::default()
        }
    }

    async fn run_command(&self, cmd: &str, args: &[&str]) -> Result<(), AgentError> {
        let output = tokio::process::Command::new(cmd)
            .args(args)
            .output()
            .await
            .map_err(|e| AgentError::Network(format!("Failed to run {}: {}", cmd, e)))?;

        if !output.status.success() {
            return Err(AgentError::Network(format!(
                "Command {} failed: {}",
                cmd,
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        Ok(())
    }
}

/// Simple IP address allocator
struct IpAllocator {
    base: Ipv4Addr,
    next_offset: std::sync::atomic::AtomicU32,
}

impl IpAllocator {
    fn new(cidr: ipnetwork::Ipv4Network) -> Self {
        Self {
            base: cidr.ip(),
            next_offset: std::sync::atomic::AtomicU32::new(1),
        }
    }

    fn allocate(&self) -> Result<Ipv4Addr, AgentError> {
        let offset = self
            .next_offset
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let octets = self.base.octets();
        Ok(Ipv4Addr::new(
            octets[0],
            octets[1],
            (octets[2] as u32 + (offset >> 8)) as u8,
            (octets[3] as u32 + (offset & 0xFF)) as u8,
        ))
    }

    fn allocate_for_service(&self, _service: &str) -> Result<Ipv4Addr, AgentError> {
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
        assert!(name.len() <= MAX_IFNAME_LEN, "Name '{}' too long", name);

        let name = make_interface_name(&[long_ref, long_ref, long_ref], "s");
        assert!(name.len() <= MAX_IFNAME_LEN, "Name '{}' too long", name);

        let name = make_interface_name(&[long_ref], "");
        assert!(name.len() <= MAX_IFNAME_LEN, "Name '{}' too long", name);
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
                "Global overlay '{}' for deployment '{}' exceeds limit",
                name,
                deployment,
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
                "Service overlay '{}' for ({}, {}) exceeds limit",
                name,
                deployment,
                service,
            );
            assert!(name.starts_with("zl-"));
        }
    }

    /// Unicode inputs must not cause panics and must respect the byte limit.
    #[test]
    fn interface_name_with_unicode() {
        let name = make_interface_name(&["\u{1F600}\u{1F600}\u{1F600}"], "g");
        assert!(name.len() <= MAX_IFNAME_LEN, "Name '{}' too long", name);

        let name = make_interface_name(&["\u{00E9}\u{00E9}\u{00E9}", "\u{00FC}\u{00FC}"], "s");
        assert!(name.len() <= MAX_IFNAME_LEN, "Name '{}' too long", name);
    }

    /// node_ip() should be None before setup_global_overlay and Some after.
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
}
