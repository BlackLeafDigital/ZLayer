//! Multi-node propagation contract test for the post-bridge overlay model
//! (v0.51+).
//!
//! Background: in v0.51 the per-service `OverlayTransport` was removed in
//! favor of a per-service Linux bridge plus a single cluster
//! `OverlayTransport` whose peer `AllowedIPs` lists carry every service
//! subnet that lives on the remote node. The propagation chain is:
//!
//! 1. `setup_service_overlay(svc)` on node A assigns a subnet via the
//!    `ServiceSubnetRegistry` (P7) and calls `add_allowed_ip` on A's own
//!    cluster transport (so A's kernel WG accepts return traffic from the
//!    subnet).
//! 2. The same operation is submitted as a `Request::AssignServiceSubnet`
//!    Raft command (P8). On apply, every other node B/C/... calls
//!    `add_allowed_ip(A_pubkey, subnet)` on its own cluster transport so
//!    that packets destined for the subnet route through A's WG peer.
//!
//! This test asserts that contract at the `OverlayTransport` layer — i.e.
//! that `add_allowed_ip` produces a UAPI state where `status()` reports the
//! subnet on the named peer. The full `OverlayManager`-level
//! cross-node test will land once the "wire `OverlayManager` into
//! `InternalState`" follow-up exposes a public `apply_assigned_subnet`
//! method on `OverlayManager` that the test can call to drive the apply
//! side of the contract.
//!
//! # Reduction vs. the original spec
//!
//! The original spec called for a 3-node `OverlayManager` test. The
//! current `OverlayManager` API doesn't expose a public hook for
//! "simulate the Raft apply" (the apply path runs inside the scheduler
//! state machine in `zlayer-scheduler::raft`, not in the test process),
//! and the cluster `OverlayTransport` field is private — so a 3-node
//! `OverlayManager` test could not assert on the propagated `AllowedIPs`
//! without leaking implementation details. The reduced 2-node
//! `OverlayTransport`-level test captures the propagation contract
//! (the data-plane invariant the apply handler exists to enforce) without
//! that leakage, and gives the scheduler-side test in
//! `crates/zlayer-scheduler/src/raft.rs::tests` a concrete data-plane
//! assertion to point at.
//!
//! Requires `CAP_NET_ADMIN` (real TUN devices via boringtun). Run with:
//!
//! ```sh
//! sudo -E cargo test -p zlayer-agent --test multi_node_service_subnets -- --ignored --nocapture
//! ```

#![cfg(target_os = "linux")]
// This test is structurally a two-node mirror — pairs of bindings like
// `node_a_pub_hex` / `node_b_pub_hex`, `priv_a` / `priv_b`, `tmp_a` /
// `tmp_b` are exactly the natural way to name node-A and node-B locals.
// Renaming for clippy::similar_names would make the test less readable.
#![allow(clippy::similar_names)]

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use ipnet::IpNet;
use zlayer_overlay::config::OverlayConfig;
use zlayer_overlay::transport::OverlayTransport;
use zlayer_overlay::PeerInfo;

/// Build a fresh `OverlayConfig` for a test transport. Uses a unique UAPI
/// socket directory (under a per-test `tempfile::TempDir`-style path) so
/// two transports in the same process do not collide on
/// `/var/run/wireguard/<iface>.sock`.
fn test_config(
    private_key: String,
    public_key: String,
    listen_port: u16,
    overlay_cidr: &str,
    uapi_sock_dir: std::path::PathBuf,
) -> OverlayConfig {
    OverlayConfig {
        local_endpoint: SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), listen_port),
        private_key,
        public_key,
        overlay_cidr: overlay_cidr.to_string(),
        uapi_sock_dir,
        ..OverlayConfig::default()
    }
}

/// Returns `true` if `dump` (a `wg show`-style UAPI response) contains an
/// `allowed_ip=<cidr>` line attributed to the peer block headed by
/// `peer_pub_hex_or_b64`. Tolerates either base64 (Windows path) or hex
/// (Linux/macOS path) peer key encoding in the dump.
fn dump_contains_allowed_ip_for_peer(dump: &str, peer_pub: &str, cidr: &IpNet) -> bool {
    // Walk peer blocks. A peer block starts at `public_key=...` and ends
    // at the next `public_key=...` (or EOF).
    let needle = format!("allowed_ip={cidr}");
    let mut in_target_peer = false;
    for line in dump.lines() {
        if let Some(rest) = line.strip_prefix("public_key=") {
            in_target_peer = rest == peer_pub || rest.eq_ignore_ascii_case(peer_pub);
            continue;
        }
        if let Some(rest) = line.strip_prefix("public_key_b64=") {
            // Windows dump emits both hex and base64; treat either as a match.
            if rest == peer_pub {
                in_target_peer = true;
            }
            continue;
        }
        if in_target_peer && line.trim() == needle {
            return true;
        }
    }
    false
}

/// Two-node propagation contract:
///
/// 1. Stand up node A's and node B's cluster `OverlayTransport`s with
///    distinct UAPI socket dirs (so they coexist in one process) and
///    distinct UDP listen ports.
/// 2. Add each as a peer of the other (peer block created with no
///    `AllowedIPs` — the propagation step is what installs them).
/// 3. Simulate node A's `setup_service_overlay("svc-1")`: A assigns
///    `10.220.1.0/28` and adds it to its OWN cluster transport for its own
///    peer entry on B (i.e. B knows "A owns 10.220.1.0/28").
/// 4. Simulate node B's `setup_service_overlay("svc-2")`: B assigns
///    `10.220.2.0/28` and adds it to its OWN cluster transport for its own
///    peer entry on A.
/// 5. Assert the contract: A's UAPI dump shows B's pubkey with svc-2 in
///    `AllowedIPs`; B's UAPI dump shows A's pubkey with svc-1 in
///    `AllowedIPs`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CAP_NET_ADMIN to create real TUN devices and bind UDP sockets"]
async fn two_node_service_subnets_propagate_via_add_allowed_ip() {
    // Per-test scratch directories for UAPI sockets so nodes A and B don't
    // clobber each other in /var/run/wireguard.
    let tmp_a = tempfile::tempdir().expect("tempdir A");
    let tmp_b = tempfile::tempdir().expect("tempdir B");

    // Generate keys for each node.
    let (priv_a, pub_a) = OverlayTransport::generate_keys()
        .await
        .expect("generate_keys A");
    let (priv_b, pub_b) = OverlayTransport::generate_keys()
        .await
        .expect("generate_keys B");

    // Pick two distinct ephemeral-range ports. (Static here for repeatable
    // assertions; if a host actually has these bound the test will fail
    // fast with a clear "address in use" error.)
    let port_a: u16 = 53691;
    let port_b: u16 = 53692;

    let iface_a = "zl-ta-g".to_string();
    let iface_b = "zl-tb-g".to_string();

    let cfg_a = test_config(
        priv_a,
        pub_a.clone(),
        port_a,
        "10.220.0.1/32",
        tmp_a.path().to_path_buf(),
    );
    let cfg_b = test_config(
        priv_b,
        pub_b.clone(),
        port_b,
        "10.220.0.2/32",
        tmp_b.path().to_path_buf(),
    );

    let mut node_a = OverlayTransport::new(cfg_a, iface_a.clone());
    let mut node_b = OverlayTransport::new(cfg_b, iface_b.clone());

    node_a.create_interface().await.expect("create_interface A");
    node_b.create_interface().await.expect("create_interface B");

    // Cross-register peers with empty AllowedIPs initially. The
    // PeerInfo::new constructor requires a non-empty allowed_ips string,
    // so seed each side with the peer's /32 overlay address — that's the
    // baseline a real node would have before any service subnets are
    // assigned.
    let peer_a_on_b = PeerInfo::new(
        pub_a.clone(),
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port_a),
        "10.220.0.1/32",
        Duration::from_secs(25),
    );
    let peer_b_on_a = PeerInfo::new(
        pub_b.clone(),
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port_b),
        "10.220.0.2/32",
        Duration::from_secs(25),
    );

    node_a
        .configure(std::slice::from_ref(&peer_b_on_a))
        .await
        .expect("configure A peers");
    node_b
        .configure(std::slice::from_ref(&peer_a_on_b))
        .await
        .expect("configure B peers");

    // ---- Propagation step ----------------------------------------------
    // Subnet owned by node A. In production this comes from the
    // ServiceSubnetRegistry (P7) and is broadcast via the
    // Request::AssignServiceSubnet apply handler (P8). Here we simulate
    // node B's apply-side handler by calling add_allowed_ip directly on
    // node B's transport, for A's peer entry.
    let subnet_a: IpNet = "10.220.1.0/28".parse().expect("parse subnet_a");
    let subnet_b: IpNet = "10.220.2.0/28".parse().expect("parse subnet_b");

    node_b
        .add_allowed_ip(&pub_a, subnet_a)
        .await
        .expect("B apply AssignServiceSubnet(svc-1, A)");
    node_a
        .add_allowed_ip(&pub_b, subnet_b)
        .await
        .expect("A apply AssignServiceSubnet(svc-2, B)");

    // ---- Assertions ----------------------------------------------------
    let dump_a = node_a.status().await.expect("A status");
    let dump_b = node_b.status().await.expect("B status");

    // The peer pubkeys in the UAPI dump come out as hex (Linux) — convert.
    // (We accept either encoding in the helper so we don't have to import
    // the private `key_to_hex` from the transport module.)
    let node_a_pub_hex = base64_to_hex(&pub_a).expect("decode pub_a");
    let node_b_pub_hex = base64_to_hex(&pub_b).expect("decode pub_b");

    assert!(
        dump_contains_allowed_ip_for_peer(&dump_a, &node_b_pub_hex, &subnet_b),
        "node A's cluster transport should have B's pubkey with svc-2 subnet \
         ({subnet_b}) in AllowedIPs after the propagated AssignServiceSubnet;\n\
         A's UAPI dump:\n{dump_a}"
    );
    assert!(
        dump_contains_allowed_ip_for_peer(&dump_b, &node_a_pub_hex, &subnet_a),
        "node B's cluster transport should have A's pubkey with svc-1 subnet \
         ({subnet_a}) in AllowedIPs after the propagated AssignServiceSubnet;\n\
         B's UAPI dump:\n{dump_b}"
    );

    // Tear down.
    node_a.shutdown();
    node_b.shutdown();
}

/// Decode a base64 `WireGuard` public key (32 bytes) into lowercase hex.
/// The UAPI dump emits peer keys in hex form on Linux/macOS.
fn base64_to_hex(b64: &str) -> Result<String, String> {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    let raw = STANDARD
        .decode(b64.trim())
        .map_err(|e| format!("base64 decode: {e}"))?;
    if raw.len() != 32 {
        return Err(format!("expected 32-byte key, got {}", raw.len()));
    }
    Ok(hex::encode(raw))
}
