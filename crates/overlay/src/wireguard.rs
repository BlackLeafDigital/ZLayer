//! WireGuard kernel module interface
//!
//! Manages WireGuard interfaces and peer configuration.

use crate::{config::OverlayConfig, PeerInfo};
use std::process::Stdio;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

/// WireGuard interface manager
pub struct WireGuardManager {
    config: OverlayConfig,
    interface_name: String,
}

impl WireGuardManager {
    /// Create a new WireGuard manager
    pub fn new(config: OverlayConfig, interface_name: String) -> Self {
        Self {
            config,
            interface_name,
        }
    }

    /// Create WireGuard interface
    pub async fn create_interface(&self) -> Result<(), Box<dyn std::error::Error>> {
        let output = Command::new("ip")
            .args(["link", "add", "dev", &self.interface_name, "type", "wireguard"])
            .output()
            .await?;

        if !output.status.success() {
            return Err(format!("Failed to create WireGuard interface: {}", String::from_utf8_lossy(&output.stderr)).into());
        }

        Ok(())
    }

    /// Configure WireGuard interface
    pub async fn configure_interface(&self, peers: &[PeerInfo]) -> Result<(), Box<dyn std::error::Error>> {
        let mut config = format!(
            "[Interface]\n\
                        PrivateKey = {}\n\
                        ListenPort = {}\n\
                        Address = {}\n",
            self.config.private_key,
            self.config.local_endpoint.port(),
            self.config.overlay_cidr
        );

        for peer in peers {
            config.push_str(&peer.to_wg_config());
        }

        // Write config to file (requires root)
        let config_path = format!("/etc/wireguard/{}.conf", self.interface_name);
        tokio::fs::write(&config_path, config).await?;

        // Bring up interface
        let output = Command::new("wg")
            .args(["up", &self.interface_name])
            .output()
            .await?;

        if !output.status.success() {
            return Err(format!("Failed to bring up WireGuard interface: {}", String::from_utf8_lossy(&output.stderr)).into());
        }

        Ok(())
    }

    /// Get interface status
    pub async fn status(&self) -> Result<String, Box<dyn std::error::Error>> {
        let output = Command::new("wg")
            .args(["show", &self.interface_name])
            .output()
            .await?;

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    /// Generate WireGuard keys
    ///
    /// Generates a private/public key pair using the `wg` command-line tool.
    /// The `wg genkey` command outputs the private key to stdout (no arguments).
    /// The private key is then piped to `wg pubkey` to derive the public key.
    pub async fn generate_keys() -> Result<(String, String), Box<dyn std::error::Error>> {
        // Generate private key: `wg genkey` outputs to stdout
        let genkey_output = Command::new("wg")
            .arg("genkey")
            .output()
            .await?;

        if !genkey_output.status.success() {
            return Err(format!(
                "Failed to generate WireGuard private key: {}",
                String::from_utf8_lossy(&genkey_output.stderr)
            )
            .into());
        }

        let private_key = String::from_utf8_lossy(&genkey_output.stdout)
            .trim()
            .to_string();

        if private_key.is_empty() {
            return Err("wg genkey produced empty output".into());
        }

        // Generate public key: pipe private key to `wg pubkey`
        let mut pubkey_child = Command::new("wg")
            .arg("pubkey")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        // Write private key to stdin
        if let Some(mut stdin) = pubkey_child.stdin.take() {
            stdin.write_all(private_key.as_bytes()).await?;
            // stdin is dropped here, closing the pipe
        }

        let pubkey_output = pubkey_child.wait_with_output().await?;

        if !pubkey_output.status.success() {
            return Err(format!(
                "Failed to generate WireGuard public key: {}",
                String::from_utf8_lossy(&pubkey_output.stderr)
            )
            .into());
        }

        let public_key = String::from_utf8_lossy(&pubkey_output.stdout)
            .trim()
            .to_string();

        if public_key.is_empty() {
            return Err("wg pubkey produced empty output".into());
        }

        Ok((private_key, public_key))
    }

    /// Add a peer dynamically
    pub async fn add_peer(&self, peer: &PeerInfo) -> Result<(), Box<dyn std::error::Error>> {
        let output = Command::new("wg")
            .args([
                "set",
                &self.interface_name,
                "peer",
                &peer.public_key,
                "endpoint",
                &peer.endpoint.to_string(),
                "allowed-ips",
                &peer.allowed_ips,
                "persistent-keepalive",
                &format!("{}s", peer.persistent_keepalive_interval.as_secs()),
            ])
            .output()
            .await?;

        if !output.status.success() {
            return Err(format!("Failed to add WireGuard peer: {}", String::from_utf8_lossy(&output.stderr)).into());
        }

        Ok(())
    }

    /// Remove a peer
    pub async fn remove_peer(&self, public_key: &str) -> Result<(), Box<dyn std::error::Error>> {
        let output = Command::new("wg")
            .args(["set", &self.interface_name, "peer", public_key, "remove"])
            .output()
            .await?;

        if !output.status.success() {
            return Err(format!("Failed to remove WireGuard peer: {}", String::from_utf8_lossy(&output.stderr)).into());
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::time::Duration;

    #[tokio::test]
    async fn test_peer_info_to_wg_config() {
        let peer = PeerInfo::new(
            "test_public_key".to_string(),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), 51820),
            "10.0.0.2/32",
            Duration::from_secs(25),
        );

        let config = peer.to_wg_config();
        assert!(config.contains("PublicKey = test_public_key"));
        assert!(config.contains("Endpoint = 10.0.0.1:51820"));
    }
}
