//! Encrypted overlay transport layer
//!
//! Uses boringtun (userspace WireGuard) to create TUN-based encrypted tunnels.
//! No kernel WireGuard module or `wg` binary required.

use crate::{config::OverlayConfig, PeerInfo};
use boringtun::device::{DeviceConfig, DeviceHandle};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::process::Command;

// ---------------------------------------------------------------------------
// UAPI helpers
// ---------------------------------------------------------------------------

/// Convert a base64-encoded WireGuard key to hex (UAPI requires hex-encoded keys).
fn key_to_hex(base64_key: &str) -> Result<String, Box<dyn std::error::Error>> {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    let bytes = STANDARD.decode(base64_key)?;
    if bytes.len() != 32 {
        return Err(format!("Invalid key length: expected 32 bytes, got {}", bytes.len()).into());
    }
    Ok(hex::encode(bytes))
}

/// Send a UAPI `set` command to the boringtun device.
///
/// The body should contain newline-delimited `key=value` pairs (without the
/// leading `set=1\n` — that is prepended automatically).
async fn uapi_set(sock_path: &str, body: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut stream = UnixStream::connect(sock_path).await?;
    let msg = format!("set=1\n{}\n", body);
    stream.write_all(msg.as_bytes()).await?;
    stream.shutdown().await?;
    let mut response = String::new();
    stream.read_to_string(&mut response).await?;
    if response.contains("errno=0") {
        Ok(())
    } else {
        Err(format!("UAPI set failed: {}", response.trim()).into())
    }
}

/// Send a UAPI `get` command and return the raw response.
async fn uapi_get(sock_path: &str) -> Result<String, Box<dyn std::error::Error>> {
    let mut stream = UnixStream::connect(sock_path).await?;
    stream.write_all(b"get=1\n\n").await?;
    stream.shutdown().await?;
    let mut response = String::new();
    stream.read_to_string(&mut response).await?;
    Ok(response)
}

// ---------------------------------------------------------------------------
// OverlayTransport
// ---------------------------------------------------------------------------

/// Encrypted overlay transport layer.
///
/// Uses boringtun (userspace WireGuard) to create TUN-based encrypted tunnels.
/// No kernel WireGuard module required.
///
/// **Important:** This struct holds the boringtun [`DeviceHandle`]. The TUN
/// device is destroyed when the handle is dropped. Callers **must** keep this
/// struct alive for the entire overlay network lifetime.
pub struct OverlayTransport {
    config: OverlayConfig,
    interface_name: String,
    device: Option<DeviceHandle>,
}

impl OverlayTransport {
    /// Create a new overlay transport (device is not started yet).
    pub fn new(config: OverlayConfig, interface_name: String) -> Self {
        Self {
            config,
            interface_name,
            device: None,
        }
    }

    /// Path to the UAPI Unix socket for this interface.
    fn uapi_sock_path(&self) -> String {
        format!("/var/run/wireguard/{}.sock", self.interface_name)
    }

    /// Create the TUN interface via boringtun.
    ///
    /// This spawns boringtun worker threads that manage the TUN device.  The
    /// device is torn down when this struct is dropped (or [`shutdown`] is
    /// called).
    pub async fn create_interface(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        // Validate interface name (kernel limit is 15 chars for IFNAMSIZ)
        if self.interface_name.len() > 15 {
            return Err(format!(
                "Interface name '{}' exceeds 15 character limit",
                self.interface_name
            )
            .into());
        }

        // Ensure the UAPI socket directory exists
        tokio::fs::create_dir_all("/var/run/wireguard").await?;

        // Clean up any stale interface from a previous crashed deploy.
        // If the process was SIGKILLed, the TUN device persists in the kernel
        // and DeviceHandle::new() will fail on re-create. Best-effort cleanup:
        // if `ip link delete` fails, we still attempt to create the device.
        let check_output = Command::new("ip")
            .args(["link", "show", &self.interface_name])
            .output()
            .await;
        if let Ok(output) = check_output {
            if output.status.success() {
                tracing::warn!(
                    interface = %self.interface_name,
                    "stale network interface found, cleaning up before re-create"
                );
                let _ = Command::new("ip")
                    .args(["link", "delete", &self.interface_name])
                    .output()
                    .await;
            }
        }

        // Clean up stale UAPI socket left behind by a crashed process.
        let sock_path = format!("/var/run/wireguard/{}.sock", self.interface_name);
        if tokio::fs::try_exists(&sock_path).await.unwrap_or(false) {
            tracing::warn!(path = %sock_path, "removing stale UAPI socket");
            let _ = tokio::fs::remove_file(&sock_path).await;
        }

        let name = self.interface_name.clone();
        let cfg = DeviceConfig {
            n_threads: 2,
            use_connected_socket: true,
            #[cfg(target_os = "linux")]
            use_multi_queue: false,
            #[cfg(target_os = "linux")]
            uapi_fd: -1,
        };

        // DeviceHandle::new() blocks (spawns threads), so run on the blocking
        // thread pool.
        let handle = tokio::task::spawn_blocking(move || DeviceHandle::new(&name, cfg))
            .await
            .map_err(|e| format!("spawn_blocking join error: {e}"))?
            .map_err(|e| {
                format!(
                    "Failed to create boringtun device '{}': {e}. \
                     Ensure CAP_NET_ADMIN capability is available.",
                    self.interface_name
                )
            })?;

        self.device = Some(handle);
        tracing::info!(
            interface = %self.interface_name,
            "Created boringtun overlay transport"
        );
        Ok(())
    }

    /// Configure the transport with private key, listen port, and peers.
    ///
    /// After setting the WireGuard parameters via UAPI, this also assigns the
    /// overlay IP address and brings the interface up using standard `ip`
    /// commands.
    pub async fn configure(&self, peers: &[PeerInfo]) -> Result<(), Box<dyn std::error::Error>> {
        let sock = self.uapi_sock_path();

        // Build the UAPI set body
        let private_key_hex = key_to_hex(&self.config.private_key)?;
        let mut body = format!(
            "private_key={}\nlisten_port={}\n",
            private_key_hex,
            self.config.local_endpoint.port(),
        );

        for peer in peers {
            let pub_hex = key_to_hex(&peer.public_key)?;
            body.push_str(&format!("public_key={}\n", pub_hex));
            body.push_str(&format!("endpoint={}\n", peer.endpoint));
            body.push_str(&format!("allowed_ip={}\n", peer.allowed_ips));
            body.push_str(&format!(
                "persistent_keepalive_interval={}\n",
                peer.persistent_keepalive_interval.as_secs()
            ));
        }

        uapi_set(&sock, &body).await?;
        tracing::debug!(interface = %self.interface_name, "Applied UAPI configuration");

        // Assign overlay IP address (generic networking command)
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
            // Ignore "already exists" — idempotent
            if !stderr.contains("RTNETLINK answers: File exists") {
                return Err(format!("Failed to assign IP: {}", stderr).into());
            }
        }

        // Bring interface up (generic networking command)
        let output = Command::new("ip")
            .args(["link", "set", "dev", &self.interface_name, "up"])
            .output()
            .await?;

        if !output.status.success() {
            return Err(format!(
                "Failed to bring up interface: {}",
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }

        tracing::info!(interface = %self.interface_name, "Overlay transport configured and up");
        Ok(())
    }

    /// Add a peer dynamically via UAPI.
    pub async fn add_peer(&self, peer: &PeerInfo) -> Result<(), Box<dyn std::error::Error>> {
        let sock = self.uapi_sock_path();
        let pub_hex = key_to_hex(&peer.public_key)?;

        let body = format!(
            "public_key={}\nendpoint={}\nallowed_ip={}\npersistent_keepalive_interval={}\n",
            pub_hex,
            peer.endpoint,
            peer.allowed_ips,
            peer.persistent_keepalive_interval.as_secs(),
        );

        uapi_set(&sock, &body).await?;
        tracing::debug!(
            peer_key = %peer.public_key,
            interface = %self.interface_name,
            "Added peer via UAPI"
        );
        Ok(())
    }

    /// Remove a peer via UAPI.
    pub async fn remove_peer(&self, public_key: &str) -> Result<(), Box<dyn std::error::Error>> {
        let sock = self.uapi_sock_path();
        let pub_hex = key_to_hex(public_key)?;

        let body = format!("public_key={}\nremove=true\n", pub_hex);

        uapi_set(&sock, &body).await?;
        tracing::debug!(
            peer_key = %public_key,
            interface = %self.interface_name,
            "Removed peer via UAPI"
        );
        Ok(())
    }

    /// Query interface status via UAPI.
    pub async fn status(&self) -> Result<String, Box<dyn std::error::Error>> {
        let sock = self.uapi_sock_path();
        let response = uapi_get(&sock).await?;
        Ok(response)
    }

    /// Generate an overlay keypair using native Rust crypto (x25519-dalek).
    ///
    /// No external binary is required. Returns `(private_key, public_key)` in
    /// base64 encoding.
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

    /// Shut down the overlay transport, destroying the TUN device.
    ///
    /// This takes the [`DeviceHandle`] and drops it, which triggers boringtun's
    /// cleanup logic (signal exit + join worker threads + remove socket).
    pub fn shutdown(&mut self) {
        if let Some(device) = self.device.take() {
            tracing::info!(
                interface = %self.interface_name,
                "Shutting down overlay transport"
            );
            // DeviceHandle::drop triggers exit + cleanup
            drop(device);
        }
    }
}

impl Drop for OverlayTransport {
    fn drop(&mut self) {
        self.shutdown();
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::time::Duration;

    #[test]
    fn test_peer_info_to_config() {
        let peer = PeerInfo::new(
            "test_public_key".to_string(),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), 51820),
            "10.0.0.2/32",
            Duration::from_secs(25),
        );

        let config = peer.to_peer_config();
        assert!(config.contains("PublicKey = test_public_key"));
        assert!(config.contains("Endpoint = 10.0.0.1:51820"));
    }

    #[tokio::test]
    async fn test_generate_keys_native() {
        let (private_key, public_key) = OverlayTransport::generate_keys().await.unwrap();

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
        let (key1, _) = OverlayTransport::generate_keys().await.unwrap();
        let (key2, _) = OverlayTransport::generate_keys().await.unwrap();
        assert_ne!(
            key1, key2,
            "Sequential key generation should produce unique keys"
        );
    }

    #[test]
    fn test_key_to_hex() {
        use base64::{engine::general_purpose::STANDARD, Engine as _};

        // Create a known 32-byte key and encode it as base64
        let key_bytes = [0xABu8; 32];
        let base64_key = STANDARD.encode(key_bytes);
        let hex_key = key_to_hex(&base64_key).unwrap();

        assert_eq!(hex_key, "ab".repeat(32));
        assert_eq!(hex_key.len(), 64, "Hex key should be 64 chars");
    }

    #[test]
    fn test_key_to_hex_invalid_length() {
        use base64::{engine::general_purpose::STANDARD, Engine as _};

        let short_bytes = [0xABu8; 16];
        let base64_key = STANDARD.encode(short_bytes);
        let result = key_to_hex(&base64_key);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Invalid key length"));
    }

    #[tokio::test]
    #[ignore = "Requires CAP_NET_ADMIN capability"]
    async fn test_create_interface_boringtun() {
        let config = OverlayConfig {
            overlay_cidr: "10.42.0.1/24".to_string(),
            private_key: "test_key".to_string(),
            public_key: "test_pub".to_string(),
            local_endpoint: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 51820),
            peer_discovery_interval: Duration::from_secs(30),
        };

        let mut transport = OverlayTransport::new(config, "zl-bt-test0".to_string());
        let result = transport.create_interface().await;

        match result {
            Ok(()) => {
                // Success — device was created. Shut it down cleanly.
                transport.shutdown();
            }
            Err(e) => {
                let msg = e.to_string();
                // Must mention boringtun or CAP_NET_ADMIN, never the cryptic
                // kernel-wireguard "Attribute failed policy validation" message.
                assert!(
                    !msg.contains("Attribute failed policy validation"),
                    "create_interface should not produce kernel WireGuard errors. Got: {msg}",
                );
                assert!(
                    msg.contains("boringtun") || msg.contains("CAP_NET_ADMIN"),
                    "Error should mention boringtun or CAP_NET_ADMIN. Got: {msg}",
                );
            }
        }
    }
}
