//! Encrypted overlay transport layer
//!
//! Uses boringtun (userspace `WireGuard`) to create TUN-based encrypted tunnels.
//! No kernel `WireGuard` module or `wg` binary required.
//!
//! On Linux, creates TUN interfaces via `/dev/net/tun`.
//! On macOS, creates utun interfaces via the kernel control socket.

use crate::{config::OverlayConfig, PeerInfo};
use boringtun::device::{DeviceConfig, DeviceHandle};
use std::fmt::Write;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::process::Command;

// ---------------------------------------------------------------------------
// UAPI helpers
// ---------------------------------------------------------------------------

/// Convert a base64-encoded `WireGuard` key to hex (UAPI requires hex-encoded keys).
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
    let msg = format!("set=1\n{body}\n");
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
/// Uses boringtun (userspace `WireGuard`) to create TUN-based encrypted tunnels.
/// No kernel `WireGuard` module required.
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
    #[must_use]
    pub fn new(config: OverlayConfig, interface_name: String) -> Self {
        Self {
            config,
            interface_name,
            device: None,
        }
    }

    /// Get the resolved interface name.
    ///
    /// On macOS, this returns the kernel-assigned `utunN` name after
    /// [`create_interface`] has been called. Before that, it returns the
    /// name passed to [`new`].
    #[must_use]
    pub fn interface_name(&self) -> &str {
        &self.interface_name
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
    ///
    /// On Linux, creates a named TUN interface (requires `CAP_NET_ADMIN`).
    /// On macOS, creates a kernel-assigned `utunN` interface (requires `sudo`).
    ///
    /// # Errors
    ///
    /// Returns an error if the TUN device cannot be created or required
    /// privileges are unavailable.
    pub async fn create_interface(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        // On Linux, validate interface name length (IFNAMSIZ = 15).
        // On macOS, the kernel auto-assigns utunN names so validation is skipped.
        #[cfg(not(target_os = "macos"))]
        if self.interface_name.len() > 15 {
            return Err(format!(
                "Interface name '{}' exceeds 15 character limit",
                self.interface_name
            )
            .into());
        }

        // Ensure the UAPI socket directory exists
        tokio::fs::create_dir_all("/var/run/wireguard").await?;

        // On Linux, clean up stale interfaces from a previous crashed deploy.
        // If the process was SIGKILLed, the TUN device persists in the kernel
        // and DeviceHandle::new() will fail on re-create.
        #[cfg(target_os = "linux")]
        {
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
        }
        // macOS utun devices are kernel-managed and auto-destroyed when the
        // owning socket closes. No stale interface cleanup needed.

        // Clean up stale UAPI socket left behind by a crashed process.
        let sock_path = format!("/var/run/wireguard/{}.sock", self.interface_name);
        if tokio::fs::try_exists(&sock_path).await.unwrap_or(false) {
            tracing::warn!(path = %sock_path, "removing stale UAPI socket");
            let _ = tokio::fs::remove_file(&sock_path).await;
        }

        // On macOS, snapshot existing UAPI sockets so we can discover the
        // kernel-assigned utunN name after device creation.
        #[cfg(target_os = "macos")]
        let existing_socks = {
            let mut set = std::collections::HashSet::new();
            if let Ok(mut entries) = tokio::fs::read_dir("/var/run/wireguard").await {
                while let Ok(Some(entry)) = entries.next_entry().await {
                    set.insert(entry.file_name().to_string_lossy().to_string());
                }
            }
            set
        };

        // On macOS, pass "utun" to let the kernel auto-assign a utunN device.
        #[cfg(target_os = "macos")]
        let name = "utun".to_string();
        #[cfg(not(target_os = "macos"))]
        let name = self.interface_name.clone();

        let cfg = DeviceConfig {
            n_threads: 2,
            use_connected_socket: true,
            #[cfg(target_os = "linux")]
            use_multi_queue: false,
            #[cfg(target_os = "linux")]
            uapi_fd: -1,
        };

        let iface_name_for_err = self.interface_name.clone();

        // DeviceHandle::new() blocks (spawns threads), so run on the blocking
        // thread pool.
        let handle = tokio::task::spawn_blocking(move || DeviceHandle::new(&name, cfg))
            .await
            .map_err(|e| format!("spawn_blocking join error: {e}"))?
            .map_err(|e| {
                #[cfg(target_os = "macos")]
                let hint = "Ensure running with sudo.";
                #[cfg(not(target_os = "macos"))]
                let hint = "Ensure CAP_NET_ADMIN capability is available.";
                format!("Failed to create boringtun device '{iface_name_for_err}': {e}. {hint}")
            })?;

        self.device = Some(handle);

        // On macOS, discover the actual utunN interface name by finding the
        // newly created UAPI socket.
        #[cfg(target_os = "macos")]
        {
            // Small delay to let boringtun finish socket setup
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            if let Ok(mut entries) = tokio::fs::read_dir("/var/run/wireguard").await {
                while let Ok(Some(entry)) = entries.next_entry().await {
                    let fname = entry.file_name().to_string_lossy().to_string();
                    if !existing_socks.contains(&fname)
                        && fname.starts_with("utun")
                        && std::path::Path::new(&fname)
                            .extension()
                            .is_some_and(|ext| ext.eq_ignore_ascii_case("sock"))
                    {
                        self.interface_name = fname.trim_end_matches(".sock").to_string();
                        break;
                    }
                }
            }
        }

        tracing::info!(
            interface = %self.interface_name,
            "Created boringtun overlay transport"
        );
        Ok(())
    }

    /// Configure the transport with private key, listen port, and peers.
    ///
    /// After setting the `WireGuard` parameters via UAPI, this also assigns the
    /// overlay IP address and brings the interface up using platform-appropriate
    /// commands (`ifconfig`/`route` on macOS, `ip` on Linux).
    ///
    /// # Errors
    ///
    /// Returns an error if UAPI configuration fails or IP assignment commands fail.
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
            let _ = writeln!(body, "public_key={pub_hex}");
            let _ = writeln!(body, "endpoint={}", peer.endpoint);
            let _ = writeln!(body, "allowed_ip={}", peer.allowed_ips);
            let _ = writeln!(
                body,
                "persistent_keepalive_interval={}",
                peer.persistent_keepalive_interval.as_secs()
            );
        }

        uapi_set(&sock, &body).await?;
        tracing::debug!(interface = %self.interface_name, "Applied UAPI configuration");

        // Assign overlay IP address and bring interface up
        self.configure_interface().await?;

        tracing::info!(interface = %self.interface_name, "Overlay transport configured and up");
        Ok(())
    }

    /// Platform-specific interface IP assignment and bring-up.
    #[cfg(target_os = "macos")]
    async fn configure_interface(&self) -> Result<(), Box<dyn std::error::Error>> {
        let cidr: ipnet::Ipv4Net = self.config.overlay_cidr.parse().map_err(|e| {
            format!(
                "Failed to parse overlay CIDR '{}': {e}",
                self.config.overlay_cidr
            )
        })?;
        let overlay_ip = cidr.addr().to_string();
        let netmask = cidr.netmask().to_string();

        // Configure point-to-point utun interface
        let output = Command::new("ifconfig")
            .args([
                &self.interface_name,
                "inet",
                &overlay_ip,
                &overlay_ip,
                "netmask",
                &netmask,
                "up",
            ])
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("Failed to configure interface: {stderr}").into());
        }

        // Add route for the overlay subnet
        let network_cidr = format!("{}/{}", cidr.network(), cidr.prefix_len());
        let output = Command::new("route")
            .args([
                "-n",
                "add",
                "-net",
                &network_cidr,
                "-interface",
                &self.interface_name,
            ])
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // Ignore "already in table" — idempotent
            if !stderr.contains("already in table") {
                return Err(format!("Failed to add route: {stderr}").into());
            }
        }

        Ok(())
    }

    /// Platform-specific interface IP assignment and bring-up.
    #[cfg(not(target_os = "macos"))]
    async fn configure_interface(&self) -> Result<(), Box<dyn std::error::Error>> {
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
            // Ignore "already exists" — idempotent
            if !stderr.contains("RTNETLINK answers: File exists") {
                return Err(format!("Failed to assign IP: {stderr}").into());
            }
        }

        // Bring interface up
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

        Ok(())
    }

    /// Add a peer dynamically via UAPI.
    ///
    /// # Errors
    ///
    /// Returns an error if the key conversion or UAPI command fails.
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
    ///
    /// # Errors
    ///
    /// Returns an error if the key conversion or UAPI command fails.
    pub async fn remove_peer(&self, public_key: &str) -> Result<(), Box<dyn std::error::Error>> {
        let sock = self.uapi_sock_path();
        let pub_hex = key_to_hex(public_key)?;

        let body = format!("public_key={pub_hex}\nremove=true\n");

        uapi_set(&sock, &body).await?;
        tracing::debug!(
            peer_key = %public_key,
            interface = %self.interface_name,
            "Removed peer via UAPI"
        );
        Ok(())
    }

    /// Query interface status via UAPI.
    ///
    /// # Errors
    ///
    /// Returns an error if the UAPI query fails.
    pub async fn status(&self) -> Result<String, Box<dyn std::error::Error>> {
        let sock = self.uapi_sock_path();
        let response = uapi_get(&sock).await?;
        Ok(response)
    }

    /// Generate an overlay keypair using native Rust crypto (x25519-dalek).
    ///
    /// No external binary is required. Returns `(private_key, public_key)` in
    /// base64 encoding.
    ///
    /// # Errors
    ///
    /// This method currently always succeeds but returns `Result` for API consistency.
    #[allow(clippy::unused_async)]
    pub async fn generate_keys() -> Result<(String, String), Box<dyn std::error::Error>> {
        use base64::{engine::general_purpose::STANDARD, Engine as _};
        use x25519_dalek::{PublicKey, StaticSecret};

        let secret = StaticSecret::random();
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
        use base64::{engine::general_purpose::STANDARD, Engine as _};
        use x25519_dalek::{PublicKey, StaticSecret};

        let (private_key, public_key) = OverlayTransport::generate_keys().await.unwrap();

        assert_eq!(
            private_key.len(),
            44,
            "Private key should be 44 chars base64"
        );
        assert_eq!(public_key.len(), 44, "Public key should be 44 chars base64");

        let priv_bytes = STANDARD.decode(&private_key).unwrap();
        let pub_bytes = STANDARD.decode(&public_key).unwrap();
        assert_eq!(priv_bytes.len(), 32);
        assert_eq!(pub_bytes.len(), 32);

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
    #[ignore = "Requires root/CAP_NET_ADMIN"]
    async fn test_create_interface_boringtun() {
        let config = OverlayConfig {
            overlay_cidr: "10.42.0.1/24".to_string(),
            private_key: "test_key".to_string(),
            public_key: "test_pub".to_string(),
            local_endpoint: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 51820),
            peer_discovery_interval: Duration::from_secs(30),
        };

        // On macOS, boringtun uses "utun" and the kernel assigns utunN.
        // On Linux, we use a custom interface name.
        #[cfg(target_os = "macos")]
        let iface_name = "utun".to_string();
        #[cfg(not(target_os = "macos"))]
        let iface_name = "zl-bt-test0".to_string();

        let mut transport = OverlayTransport::new(config, iface_name);
        let result = transport.create_interface().await;

        match result {
            Ok(()) => {
                #[cfg(target_os = "macos")]
                assert!(
                    transport.interface_name().starts_with("utun"),
                    "macOS interface should be utunN, got: {}",
                    transport.interface_name()
                );
                transport.shutdown();
            }
            Err(e) => {
                let msg = e.to_string();
                assert!(
                    !msg.contains("Attribute failed policy validation"),
                    "create_interface should not produce kernel WireGuard errors. Got: {msg}",
                );
                assert!(
                    msg.contains("boringtun")
                        || msg.contains("CAP_NET_ADMIN")
                        || msg.contains("sudo"),
                    "Error should mention boringtun, CAP_NET_ADMIN, or sudo. Got: {msg}",
                );
            }
        }
    }
}
