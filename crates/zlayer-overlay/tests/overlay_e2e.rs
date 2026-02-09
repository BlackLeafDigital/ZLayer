//! End-to-end integration tests for ZLayer overlay networking.
//!
//! Tests marked with `#[ignore]` require root or CAP_NET_ADMIN capability.
//! Run them with:
//!
//! ```sh
//! cargo test -p zlayer-overlay --test overlay_e2e -- --ignored
//! ```

use base64::{engine::general_purpose::STANDARD, Engine as _};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;
use tokio::process::Command;
use x25519_dalek::{PublicKey, StaticSecret};
use zlayer_overlay::config::{OverlayConfig, PeerInfo};
use zlayer_overlay::transport::OverlayTransport;

// ---------------------------------------------------------------------------
// 1. Key generation test
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_native_key_generation_produces_valid_keys() {
    let (private_key, public_key) = OverlayTransport::generate_keys()
        .await
        .expect("generate_keys should succeed");

    // Overlay keys are 32 bytes encoded as standard base64 => 44 characters
    assert_eq!(
        private_key.len(),
        44,
        "Private key should be 44 characters (32 bytes base64-encoded), got {}",
        private_key.len()
    );
    assert_eq!(
        public_key.len(),
        44,
        "Public key should be 44 characters (32 bytes base64-encoded), got {}",
        public_key.len()
    );

    // Decode and verify raw byte lengths
    let priv_bytes = STANDARD
        .decode(&private_key)
        .expect("Private key must be valid base64");
    let pub_bytes = STANDARD
        .decode(&public_key)
        .expect("Public key must be valid base64");

    assert_eq!(
        priv_bytes.len(),
        32,
        "Decoded private key must be exactly 32 bytes"
    );
    assert_eq!(
        pub_bytes.len(),
        32,
        "Decoded public key must be exactly 32 bytes"
    );

    // Verify public key is correctly derived from private key
    let secret =
        StaticSecret::from(<[u8; 32]>::try_from(priv_bytes.as_slice()).expect("32-byte slice"));
    let expected_public = PublicKey::from(&secret);
    assert_eq!(
        pub_bytes.as_slice(),
        expected_public.as_bytes(),
        "Public key must be the x25519 derivation of the private key"
    );

    // Verify successive calls produce unique keys
    let (private_key_2, _) = OverlayTransport::generate_keys()
        .await
        .expect("second generate_keys should succeed");
    assert_ne!(
        private_key, private_key_2,
        "Two consecutive key generations must produce distinct private keys"
    );
}

// ---------------------------------------------------------------------------
// 2. Key compatibility test (optional: requires `wg` binary)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_native_keys_compatible_with_wg_tool() {
    // Skip if `wg` is not available -- the wg binary is no longer required
    // at runtime, so this test is purely for validating key format compatibility.
    let wg_available = Command::new("which")
        .arg("wg")
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !wg_available {
        eprintln!("SKIP: `wg` binary not found; skipping key compatibility test (wg is optional)");
        return;
    }

    // Generate keys via native Rust implementation
    let (native_priv, native_pub) = OverlayTransport::generate_keys()
        .await
        .expect("native generate_keys should succeed");

    // Generate keys via `wg genkey` / `wg pubkey`
    let wg_genkey_output = Command::new("wg")
        .arg("genkey")
        .output()
        .await
        .expect("wg genkey should execute");
    assert!(
        wg_genkey_output.status.success(),
        "wg genkey failed: {}",
        String::from_utf8_lossy(&wg_genkey_output.stderr)
    );
    let wg_priv = String::from_utf8(wg_genkey_output.stdout)
        .expect("wg genkey output should be valid UTF-8")
        .trim()
        .to_string();

    // Use a standard (sync) Command for piped stdin to keep it simple
    let wg_pubkey_output = {
        let mut child = std::process::Command::new("wg")
            .arg("pubkey")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("wg pubkey should spawn");
        {
            use std::io::Write;
            let stdin = child.stdin.as_mut().expect("stdin should be available");
            stdin
                .write_all(wg_priv.as_bytes())
                .expect("writing to stdin should succeed");
        }
        child.wait_with_output().expect("wg pubkey should complete")
    };
    assert!(
        wg_pubkey_output.status.success(),
        "wg pubkey failed: {}",
        String::from_utf8_lossy(&wg_pubkey_output.stderr)
    );
    let wg_pub = String::from_utf8(wg_pubkey_output.stdout)
        .expect("wg pubkey output should be valid UTF-8")
        .trim()
        .to_string();

    // Both native and wg-generated keys must have the same format
    assert_eq!(
        native_priv.len(),
        wg_priv.len(),
        "Native and wg private keys must have the same length"
    );
    assert_eq!(
        native_pub.len(),
        wg_pub.len(),
        "Native and wg public keys must have the same length"
    );

    // Both must decode to 32 bytes
    let native_priv_bytes = STANDARD
        .decode(&native_priv)
        .expect("native private key must be valid base64");
    let wg_priv_bytes = STANDARD
        .decode(&wg_priv)
        .expect("wg private key must be valid base64");
    assert_eq!(native_priv_bytes.len(), 32);
    assert_eq!(wg_priv_bytes.len(), 32);

    let native_pub_bytes = STANDARD
        .decode(&native_pub)
        .expect("native public key must be valid base64");
    let wg_pub_bytes = STANDARD
        .decode(&wg_pub)
        .expect("wg public key must be valid base64");
    assert_eq!(native_pub_bytes.len(), 32);
    assert_eq!(wg_pub_bytes.len(), 32);

    // Verify cross-derivation: feeding the wg-generated private key to our
    // x25519-dalek derivation must produce the same public key `wg pubkey` gave.
    let wg_secret =
        StaticSecret::from(<[u8; 32]>::try_from(wg_priv_bytes.as_slice()).expect("32-byte slice"));
    let derived_pub = PublicKey::from(&wg_secret);
    assert_eq!(
        derived_pub.as_bytes(),
        wg_pub_bytes.as_slice(),
        "x25519-dalek derivation of the wg-generated private key must match wg pubkey output"
    );
}

// ---------------------------------------------------------------------------
// 3. Overlay config construction test
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_overlay_config_and_peer_config_format() {
    let (private_key, public_key) = OverlayTransport::generate_keys()
        .await
        .expect("key generation should succeed");

    // Build an OverlayConfig with the generated keys
    let config = OverlayConfig {
        local_endpoint: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10)), 51820),
        private_key: private_key.clone(),
        public_key: public_key.clone(),
        overlay_cidr: "10.200.0.1/16".to_string(),
        peer_discovery_interval: Duration::from_secs(30),
    };

    assert_eq!(config.local_endpoint.port(), 51820);
    assert_eq!(config.overlay_cidr, "10.200.0.1/16");
    assert_eq!(config.private_key, private_key);
    assert_eq!(config.public_key, public_key);

    // Build a PeerInfo and verify its peer config block format
    let peer = PeerInfo::new(
        public_key.clone(),
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20)), 51820),
        "10.200.0.2/32",
        Duration::from_secs(25),
    );

    let peer_config = peer.to_peer_config();

    assert!(
        peer_config.contains("[Peer]"),
        "Peer config must contain [Peer] section header"
    );
    assert!(
        peer_config.contains(&format!("PublicKey = {}", public_key)),
        "Peer config must contain the correct public key"
    );
    assert!(
        peer_config.contains("Endpoint = 192.168.1.20:51820"),
        "Peer config must contain the correct endpoint"
    );
    assert!(
        peer_config.contains("AllowedIPs = 10.200.0.2/32"),
        "Peer config must contain the correct allowed IPs"
    );
    assert!(
        peer_config.contains("PersistentKeepalive = 25"),
        "Peer config must contain the correct keepalive interval"
    );

    // Verify the full config block format.
    // It concatenates [Interface] + [Peer] blocks.
    let full_config = format!(
        "[Interface]\nPrivateKey = {}\nListenPort = {}\n{}",
        config.private_key,
        config.local_endpoint.port(),
        peer_config,
    );
    assert!(
        full_config.starts_with("[Interface]"),
        "Full config must start with [Interface] section"
    );
    assert!(
        full_config.contains(&format!("PrivateKey = {}", private_key)),
        "Full config must contain the private key"
    );
    assert!(
        full_config.contains("ListenPort = 51820"),
        "Full config must contain the listen port"
    );
    assert!(
        full_config.contains("[Peer]"),
        "Full config must contain [Peer] section"
    );
}

// ---------------------------------------------------------------------------
// 4. Interface lifecycle test (requires root or CAP_NET_ADMIN)
// ---------------------------------------------------------------------------
// Run with: cargo test -p zlayer-overlay --test overlay_e2e -- --ignored

#[tokio::test]
#[ignore = "requires root or CAP_NET_ADMIN"]
async fn test_overlay_interface_lifecycle() {
    let iface_name = "wg-test-life";

    // Generate keys for the interface
    let (private_key, public_key) = OverlayTransport::generate_keys()
        .await
        .expect("key generation should succeed");

    let config = OverlayConfig {
        local_endpoint: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 51830),
        private_key,
        public_key,
        overlay_cidr: "10.250.0.1/24".to_string(),
        peer_discovery_interval: Duration::from_secs(30),
    };

    let mut manager = OverlayTransport::new(config, iface_name.to_string());

    // Cleanup any leftover interface from a previous failed run
    let _ = Command::new("ip")
        .args(["link", "del", "dev", iface_name])
        .output()
        .await;

    // Create the overlay interface
    manager
        .create_interface()
        .await
        .expect("create_interface should succeed");

    // Verify the interface exists via `ip link show`
    let link_output = Command::new("ip")
        .args(["link", "show", "dev", iface_name])
        .output()
        .await
        .expect("ip link show should execute");
    assert!(
        link_output.status.success(),
        "Interface {} should exist after create_interface, stderr: {}",
        iface_name,
        String::from_utf8_lossy(&link_output.stderr)
    );
    let link_stdout = String::from_utf8_lossy(&link_output.stdout);
    assert!(
        link_stdout.contains(iface_name),
        "ip link show output should contain the interface name"
    );

    // Configure the interface (assign IP, bring up)
    manager
        .configure(&[])
        .await
        .expect("configure_interface should succeed with no peers");

    // Verify the interface is UP
    let link_output = Command::new("ip")
        .args(["link", "show", "dev", iface_name])
        .output()
        .await
        .expect("ip link show should execute");
    let link_stdout = String::from_utf8_lossy(&link_output.stdout);
    assert!(
        link_stdout.contains("UP") || link_stdout.contains("up"),
        "Interface should be UP after configure_interface, got: {}",
        link_stdout
    );

    // Verify the overlay IP is assigned
    let addr_output = Command::new("ip")
        .args(["addr", "show", "dev", iface_name])
        .output()
        .await
        .expect("ip addr show should execute");
    let addr_stdout = String::from_utf8_lossy(&addr_output.stdout);
    assert!(
        addr_stdout.contains("10.250.0.1"),
        "Interface should have overlay IP 10.250.0.1 assigned, got: {}",
        addr_stdout
    );

    // Tear down the interface via transport shutdown
    manager.shutdown();

    // Allow a brief moment for cleanup to complete
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Verify the interface is gone
    let link_output = Command::new("ip")
        .args(["link", "show", "dev", iface_name])
        .output()
        .await
        .expect("ip link show should execute");
    assert!(
        !link_output.status.success(),
        "Interface {} should no longer exist after shutdown",
        iface_name
    );
}

// ---------------------------------------------------------------------------
// 5. Dual-interface connectivity test (requires root or CAP_NET_ADMIN)
// ---------------------------------------------------------------------------
// Run with: cargo test -p zlayer-overlay --test overlay_e2e -- --ignored

#[tokio::test]
#[ignore = "requires root or CAP_NET_ADMIN"]
async fn test_dual_overlay_connectivity() {
    let iface_a = "wg-test-a";
    let iface_b = "wg-test-b";
    let port_a: u16 = 51840;
    let port_b: u16 = 51841;
    let ip_a = "10.251.0.1";
    let ip_b = "10.251.0.2";
    let subnet = "/24";

    // Generate keys for both interfaces
    let (priv_a, pub_a) = OverlayTransport::generate_keys()
        .await
        .expect("key generation for A should succeed");
    let (priv_b, pub_b) = OverlayTransport::generate_keys()
        .await
        .expect("key generation for B should succeed");

    let config_a = OverlayConfig {
        local_endpoint: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), port_a),
        private_key: priv_a,
        public_key: pub_a.clone(),
        overlay_cidr: format!("{}{}", ip_a, subnet),
        peer_discovery_interval: Duration::from_secs(30),
    };

    let config_b = OverlayConfig {
        local_endpoint: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), port_b),
        private_key: priv_b,
        public_key: pub_b.clone(),
        overlay_cidr: format!("{}{}", ip_b, subnet),
        peer_discovery_interval: Duration::from_secs(30),
    };

    let mut manager_a = OverlayTransport::new(config_a, iface_a.to_string());
    let mut manager_b = OverlayTransport::new(config_b, iface_b.to_string());

    // Cleanup any leftover interfaces
    let _ = Command::new("ip")
        .args(["link", "del", "dev", iface_a])
        .output()
        .await;
    let _ = Command::new("ip")
        .args(["link", "del", "dev", iface_b])
        .output()
        .await;

    // Create both interfaces
    manager_a
        .create_interface()
        .await
        .expect("create_interface A should succeed");
    manager_b
        .create_interface()
        .await
        .expect("create_interface B should succeed");

    // Peer B is a peer of A: endpoint is 127.0.0.1:port_b, allowed IP is B's overlay IP
    let peer_b_for_a = PeerInfo::new(
        pub_b.clone(),
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), port_b),
        &format!("{}/32", ip_b),
        Duration::from_secs(25),
    );

    // Peer A is a peer of B: endpoint is 127.0.0.1:port_a, allowed IP is A's overlay IP
    let peer_a_for_b = PeerInfo::new(
        pub_a.clone(),
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), port_a),
        &format!("{}/32", ip_a),
        Duration::from_secs(25),
    );

    // Configure both interfaces with their respective peers
    manager_a
        .configure(&[peer_b_for_a])
        .await
        .expect("configure_interface A with peer B should succeed");
    manager_b
        .configure(&[peer_a_for_b])
        .await
        .expect("configure_interface B with peer A should succeed");

    // Allow a brief moment for the tunnel to establish
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Ping from A's overlay IP to B's overlay IP through the overlay tunnel
    let ping_output = Command::new("ping")
        .args([
            "-c", "3", // 3 packets
            "-W", "5", // 5 second timeout
            "-I", iface_a, // source interface
            ip_b,    // destination
        ])
        .output()
        .await
        .expect("ping command should execute");

    let ping_stdout = String::from_utf8_lossy(&ping_output.stdout);
    let ping_stderr = String::from_utf8_lossy(&ping_output.stderr);

    assert!(
        ping_output.status.success(),
        "Ping from {} ({}) to {} ({}) should succeed.\nstdout: {}\nstderr: {}",
        iface_a,
        ip_a,
        iface_b,
        ip_b,
        ping_stdout,
        ping_stderr,
    );

    // Verify we received replies (not 100% packet loss)
    assert!(
        !ping_stdout.contains("100% packet loss"),
        "Ping should not have 100% packet loss.\nstdout: {}",
        ping_stdout
    );

    // Also ping in the reverse direction
    let ping_reverse = Command::new("ping")
        .args(["-c", "3", "-W", "5", "-I", iface_b, ip_a])
        .output()
        .await
        .expect("reverse ping should execute");

    assert!(
        ping_reverse.status.success(),
        "Reverse ping from {} ({}) to {} ({}) should succeed.\nstdout: {}\nstderr: {}",
        iface_b,
        ip_b,
        iface_a,
        ip_a,
        String::from_utf8_lossy(&ping_reverse.stdout),
        String::from_utf8_lossy(&ping_reverse.stderr),
    );

    // Cleanup: shut down both transports
    manager_a.shutdown();
    manager_b.shutdown();

    // Allow a brief moment for cleanup to complete
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Verify both are gone
    let check_a = Command::new("ip")
        .args(["link", "show", "dev", iface_a])
        .output()
        .await
        .expect("ip link show should execute");
    assert!(
        !check_a.status.success(),
        "Interface {} should be removed after shutdown",
        iface_a
    );

    let check_b = Command::new("ip")
        .args(["link", "show", "dev", iface_b])
        .output()
        .await
        .expect("ip link show should execute");
    assert!(
        !check_b.status.success(),
        "Interface {} should be removed after shutdown",
        iface_b
    );
}
