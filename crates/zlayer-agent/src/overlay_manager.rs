use crate::error::AgentError;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use tokio::sync::RwLock;
use zlayer_overlay::{OverlayConfig, WireGuardManager};

/// Manages overlay networks for a deployment
pub struct OverlayManager {
    /// Deployment name (used for network naming)
    deployment: String,
    /// Global overlay interface name
    global_interface: Option<String>,
    /// Service-specific overlay interfaces (service_name -> interface_name)
    service_interfaces: RwLock<HashMap<String, String>>,
    /// IP allocator for overlay networks
    ip_allocator: IpAllocator,
}

impl OverlayManager {
    /// Create a new overlay manager for a deployment
    pub async fn new(deployment: String) -> Result<Self, AgentError> {
        Ok(Self {
            deployment,
            global_interface: None,
            service_interfaces: RwLock::new(HashMap::new()),
            ip_allocator: IpAllocator::new("10.200.0.0/16".parse().unwrap()),
        })
    }

    /// Setup the global overlay network for the deployment
    pub async fn setup_global_overlay(&mut self) -> Result<(), AgentError> {
        let prefix = &self.deployment[..8.min(self.deployment.len())];
        let interface_name = format!("zl-{}-global", prefix);

        let (private_key, public_key) = WireGuardManager::generate_keys()
            .await
            .map_err(|e| AgentError::Network(format!("Failed to generate keys: {}", e)))?;

        let node_ip = self.ip_allocator.allocate()?;
        let config = self.build_config(private_key, public_key, node_ip, 16, 51820);
        let manager = WireGuardManager::new(config, interface_name.clone());

        manager
            .create_interface()
            .await
            .map_err(|e| AgentError::Network(format!("Failed to create global overlay: {}", e)))?;
        manager.configure_interface(&[]).await.map_err(|e| {
            AgentError::Network(format!("Failed to configure global overlay: {}", e))
        })?;

        self.global_interface = Some(interface_name);
        Ok(())
    }

    /// Setup a service-scoped overlay network
    pub async fn setup_service_overlay(&self, service_name: &str) -> Result<String, AgentError> {
        let deployment_prefix = &self.deployment[..4.min(self.deployment.len())];
        let service_prefix = &service_name[..8.min(service_name.len())];
        let interface_name = format!("zl-{}-{}", deployment_prefix, service_prefix);

        let (private_key, public_key) = WireGuardManager::generate_keys()
            .await
            .map_err(|e| AgentError::Network(format!("Failed to generate keys: {}", e)))?;

        let service_ip = self.ip_allocator.allocate_for_service(service_name)?;
        let config = self.build_config(private_key, public_key, service_ip, 24, 0);
        let manager = WireGuardManager::new(config, interface_name.clone());

        manager
            .create_interface()
            .await
            .map_err(|e| AgentError::Network(format!("Failed to create service overlay: {}", e)))?;
        manager.configure_interface(&[]).await.map_err(|e| {
            AgentError::Network(format!("Failed to configure service overlay: {}", e))
        })?;

        self.service_interfaces
            .write()
            .await
            .insert(service_name.to_string(), interface_name.clone());
        Ok(interface_name)
    }

    /// Add a container to the appropriate overlay networks
    pub async fn attach_container(
        &self,
        container_pid: u32,
        service_name: &str,
        join_global: bool,
    ) -> Result<Ipv4Addr, AgentError> {
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

    async fn attach_to_interface(
        &self,
        container_pid: u32,
        _interface: &str,
        ip: Ipv4Addr,
    ) -> Result<(), AgentError> {
        let veth_host = format!("veth-{}", container_pid);
        let veth_container = "eth0";

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

        Ok(())
    }

    /// Cleanup all overlay networks
    pub async fn cleanup(&mut self) -> Result<(), AgentError> {
        let interfaces = self.service_interfaces.read().await;
        for iface in interfaces.values() {
            let _ = self.run_command("ip", &["link", "del", iface]).await;
        }

        if let Some(iface) = &self.global_interface {
            let _ = self.run_command("ip", &["link", "del", iface]).await;
        }

        Ok(())
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
