//! WireGuard kernel module interface
//!
//! Manages WireGuard interfaces and peer configuration.

use crate::{config::OverlayConfig, PeerInfo};
use std::sync::OnceLock;
use tokio::process::Command;

/// Static flag to track whether wireguard-tools have been verified/installed.
static WG_TOOLS_CHECKED: OnceLock<bool> = OnceLock::new();

/// Ensure `wg` binary is available, attempting automatic installation if not found.
///
/// Uses a `OnceLock` to only check/install once per process lifetime.
pub(crate) async fn ensure_wireguard_tools() -> Result<(), Box<dyn std::error::Error>> {
    if let Some(&available) = WG_TOOLS_CHECKED.get() {
        if available {
            return Ok(());
        } else {
            return Err("wireguard-tools not available and auto-install previously failed".into());
        }
    }

    if Command::new("which")
        .arg("wg")
        .output()
        .await?
        .status
        .success()
    {
        let _ = WG_TOOLS_CHECKED.set(true);
        return Ok(());
    }

    tracing::info!("wireguard-tools not found, attempting automatic installation...");

    let result = if Command::new("which")
        .arg("apt-get")
        .output()
        .await?
        .status
        .success()
    {
        Command::new("apt-get")
            .args(["install", "-y", "wireguard-tools"])
            .output()
            .await
    } else if Command::new("which")
        .arg("dnf")
        .output()
        .await?
        .status
        .success()
    {
        Command::new("dnf")
            .args(["install", "-y", "wireguard-tools"])
            .output()
            .await
    } else if Command::new("which")
        .arg("yum")
        .output()
        .await?
        .status
        .success()
    {
        Command::new("yum")
            .args(["install", "-y", "wireguard-tools"])
            .output()
            .await
    } else if Command::new("which")
        .arg("pacman")
        .output()
        .await?
        .status
        .success()
    {
        Command::new("pacman")
            .args(["-S", "--noconfirm", "wireguard-tools"])
            .output()
            .await
    } else if Command::new("which")
        .arg("apk")
        .output()
        .await?
        .status
        .success()
    {
        Command::new("apk")
            .args(["add", "wireguard-tools"])
            .output()
            .await
    } else {
        let _ = WG_TOOLS_CHECKED.set(false);
        return Err(
            "wireguard-tools not found and no supported package manager detected. \
             Install manually: apt install wireguard-tools (Debian/Ubuntu), \
             dnf install wireguard-tools (Fedora), pacman -S wireguard-tools (Arch), \
             apk add wireguard-tools (Alpine)"
                .into(),
        );
    };

    match result {
        Ok(output) if output.status.success() => {
            tracing::info!("Successfully installed wireguard-tools");
            let _ = WG_TOOLS_CHECKED.set(true);
            Ok(())
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let _ = WG_TOOLS_CHECKED.set(false);
            Err(format!(
                "Failed to install wireguard-tools: {}. Install manually or use --host-network flag.",
                stderr.trim()
            )
            .into())
        }
        Err(e) => {
            let _ = WG_TOOLS_CHECKED.set(false);
            Err(format!("Failed to run package manager: {}", e).into())
        }
    }
}

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
            .args([
                "link",
                "add",
                "dev",
                &self.interface_name,
                "type",
                "wireguard",
            ])
            .output()
            .await?;

        if !output.status.success() {
            return Err(format!(
                "Failed to create WireGuard interface: {}",
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }

        Ok(())
    }

    /// Configure WireGuard interface
    pub async fn configure_interface(
        &self,
        peers: &[PeerInfo],
    ) -> Result<(), Box<dyn std::error::Error>> {
        ensure_wireguard_tools().await?;

        // Build wg-compatible config (without Address, which is wg-quick-only)
        let mut config = format!(
            "[Interface]\nPrivateKey = {}\nListenPort = {}\n",
            self.config.private_key,
            self.config.local_endpoint.port(),
        );

        for peer in peers {
            config.push_str(&peer.to_wg_config());
        }

        let config_path = format!("/etc/wireguard/{}.conf", self.interface_name);
        tokio::fs::create_dir_all("/etc/wireguard").await?;
        tokio::fs::write(&config_path, &config).await?;

        // Apply configuration with wg setconf
        let output = Command::new("wg")
            .args(["setconf", &self.interface_name, &config_path])
            .output()
            .await?;

        if !output.status.success() {
            return Err(format!(
                "Failed to configure WireGuard interface: {}",
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }

        // Assign overlay IP address
        let output = Command::new("ip")
            .args([
                "addr",
                "add",
                &self.config.overlay_cidr,
                "dev",
                &self.interface_name,
            ])
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !stderr.contains("RTNETLINK answers: File exists") {
                return Err(format!("Failed to assign IP: {}", stderr).into());
            }
        }

        // Bring interface up
        let output = Command::new("ip")
            .args(["link", "set", "dev", &self.interface_name, "up"])
            .output()
            .await?;

        if !output.status.success() {
            return Err(format!(
                "Failed to bring up WireGuard interface: {}",
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }

        Ok(())
    }

    /// Get interface status
    pub async fn status(&self) -> Result<String, Box<dyn std::error::Error>> {
        ensure_wireguard_tools().await?;

        let output = Command::new("wg")
            .args(["show", &self.interface_name])
            .output()
            .await?;

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    /// Generate WireGuard keys
    ///
    /// Generates a private/public key pair using native Rust crypto (x25519-dalek).
    /// No external `wg` binary is required for key generation.
    pub async fn generate_keys() -> Result<(String, String), Box<dyn std::error::Error>> {
        use base64::{engine::general_purpose::STANDARD, Engine as _};
        use rand::rngs::OsRng;
        use x25519_dalek::{PublicKey, StaticSecret};

        let secret = StaticSecret::random_from_rng(OsRng);
        let public = PublicKey::from(&secret);

        let private_key = STANDARD.encode(secret.to_bytes());
        let public_key = STANDARD.encode(public.as_bytes());

        Ok((private_key, public_key))
    }

    /// Add a peer dynamically
    pub async fn add_peer(&self, peer: &PeerInfo) -> Result<(), Box<dyn std::error::Error>> {
        ensure_wireguard_tools().await?;

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
                &format!("{}", peer.persistent_keepalive_interval.as_secs()),
            ])
            .output()
            .await?;

        if !output.status.success() {
            return Err(format!(
                "Failed to add WireGuard peer: {}",
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }

        Ok(())
    }

    /// Remove a peer
    pub async fn remove_peer(&self, public_key: &str) -> Result<(), Box<dyn std::error::Error>> {
        ensure_wireguard_tools().await?;

        let output = Command::new("wg")
            .args(["set", &self.interface_name, "peer", public_key, "remove"])
            .output()
            .await?;

        if !output.status.success() {
            return Err(format!(
                "Failed to remove WireGuard peer: {}",
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
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

    #[tokio::test]
    async fn test_generate_keys_native() {
        let (private_key, public_key) = WireGuardManager::generate_keys().await.unwrap();

        assert_eq!(
            private_key.len(),
            44,
            "Private key should be 44 chars base64"
        );
        assert_eq!(public_key.len(), 44, "Public key should be 44 chars base64");

        use base64::{engine::general_purpose::STANDARD, Engine as _};
        let priv_bytes = STANDARD.decode(&private_key).unwrap();
        let pub_bytes = STANDARD.decode(&public_key).unwrap();
        assert_eq!(priv_bytes.len(), 32);
        assert_eq!(pub_bytes.len(), 32);

        use x25519_dalek::{PublicKey, StaticSecret};
        let secret = StaticSecret::from(<[u8; 32]>::try_from(priv_bytes.as_slice()).unwrap());
        let expected_public = PublicKey::from(&secret);
        assert_eq!(pub_bytes.as_slice(), expected_public.as_bytes());
    }

    #[tokio::test]
    async fn test_generate_keys_unique() {
        let (key1, _) = WireGuardManager::generate_keys().await.unwrap();
        let (key2, _) = WireGuardManager::generate_keys().await.unwrap();
        assert_ne!(
            key1, key2,
            "Sequential key generation should produce unique keys"
        );
    }
}
