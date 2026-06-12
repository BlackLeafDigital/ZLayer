//! Encrypted overlay transport layer
//!
//! Uses boringtun (userspace `WireGuard`) to create TUN-based encrypted tunnels.
//! No kernel `WireGuard` module or `wg` binary required.
//!
//! On Linux, creates TUN interfaces via `/dev/net/tun`.
//! On macOS, creates utun interfaces via the kernel control socket.
//! On Windows, creates a Wintun adapter (see [`crate::tun::windows`]),
//! configures it via IP Helper (see [`crate::interface::windows`]), and
//! drives the `WireGuard` noise pipeline directly via
//! `boringtun::noise::Tunn` paired with a userspace UDP socket. Three
//! async tasks shuttle packets: `ingress` (UDP → decap → TUN), `egress`
//! (TUN → encap → UDP), and `timers` (per-peer `update_timers` tick to
//! emit keepalives / re-initiate handshakes).

use crate::interface::platform_ops;
use crate::{config::OverlayConfig, PeerInfo};
#[cfg(not(windows))]
use boringtun::device::{DeviceConfig, DeviceHandle};
use std::fmt::Write;
#[cfg(not(windows))]
use tokio::io::{AsyncReadExt, AsyncWriteExt};
#[cfg(not(windows))]
use tokio::net::UnixStream;

#[cfg(windows)]
use crate::tun::WindowsTun;
#[cfg(windows)]
use boringtun::noise::{Tunn, TunnResult};
#[cfg(windows)]
use dashmap::DashMap;
#[cfg(windows)]
use parking_lot::RwLock;
#[cfg(windows)]
use std::net::{IpAddr, SocketAddr};
#[cfg(windows)]
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(windows)]
use std::sync::Arc;
#[cfg(windows)]
use std::time::{Duration, SystemTime, UNIX_EPOCH};
#[cfg(windows)]
use tokio::net::UdpSocket;
#[cfg(windows)]
use tokio::sync::Mutex as AsyncMutex;
#[cfg(windows)]
use tokio::task::JoinHandle;

// ---------------------------------------------------------------------------
// UAPI helpers (Linux/macOS only — Windows drives boringtun::noise::Tunn
// directly without going through a Unix socket)
// ---------------------------------------------------------------------------

/// Convert a base64-encoded `WireGuard` key to hex (UAPI requires hex-encoded keys).
#[cfg(not(windows))]
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
#[cfg(not(windows))]
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
#[cfg(not(windows))]
async fn uapi_get(sock_path: &str) -> Result<String, Box<dyn std::error::Error>> {
    let mut stream = UnixStream::connect(sock_path).await?;
    stream.write_all(b"get=1\n\n").await?;
    stream.shutdown().await?;
    let mut response = String::new();
    stream.read_to_string(&mut response).await?;
    Ok(response)
}

/// Return `true` if the given hex-encoded peer pubkey appears as a
/// `public_key=` line in a UAPI `get` response.
///
/// Used by `add_allowed_ip` / `remove_allowed_ip` on Linux/macOS to
/// distinguish "peer absent" from "peer present with empty `AllowedIPs`".
#[cfg(not(windows))]
fn peer_exists_in_uapi(uapi_get_response: &str, peer_pub_hex: &str) -> bool {
    for line in uapi_get_response.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("public_key=") {
            if rest == peer_pub_hex {
                return true;
            }
        }
    }
    false
}

/// Parse the `allowed_ip=` entries belonging to a specific peer from a
/// UAPI `get` response.
///
/// The UAPI response groups per-peer fields under successive
/// `public_key=...` headers. Each subsequent key=value line (including
/// `allowed_ip=`) belongs to the most recent peer until the next
/// `public_key=` line, the trailing `errno=` marker, or EOF. Malformed
/// CIDRs are silently skipped (boringtun should never emit invalid
/// strings, but we don't want a corrupted UAPI dump to brick a service
/// teardown).
#[cfg(not(windows))]
fn parse_allowed_ips_for_peer(uapi_get_response: &str, peer_pub_hex: &str) -> Vec<ipnet::IpNet> {
    let mut out = Vec::new();
    let mut in_target = false;
    for line in uapi_get_response.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("public_key=") {
            in_target = rest == peer_pub_hex;
            continue;
        }
        if line.starts_with("errno=") {
            // End of payload — any further data is not per-peer.
            break;
        }
        if in_target {
            if let Some(cidr_str) = line.strip_prefix("allowed_ip=") {
                if let Ok(net) = cidr_str.parse::<ipnet::IpNet>() {
                    out.push(net);
                }
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Windows-only helpers (packet parsing, key decoding, Tunn construction)
// ---------------------------------------------------------------------------

/// Per-peer state held by the Windows packet loop.
///
/// `tunn` is behind an async Mutex because ingress / egress / timers
/// tasks all need `&mut Tunn` for encapsulate / decapsulate /
/// `update_timers`. `endpoint` uses `parking_lot::RwLock` since reads
/// dominate (egress path + timers) and writes are rare (NAT endpoint
/// switch). `last_handshake_sec` is a monotonic-ish unix-seconds
/// counter updated from the ingress path; `allowed_ips` is immutable
/// after `add_peer`.
#[cfg(windows)]
#[derive(Clone)]
struct WindowsPeerState {
    tunn: Arc<AsyncMutex<Tunn>>,
    endpoint: Arc<RwLock<Option<SocketAddr>>>,
    last_handshake_sec: Arc<AtomicU64>,
    allowed_ips: Arc<Vec<ipnet::IpNet>>,
    persistent_keepalive: Option<u16>,
}

/// Decode a base64-encoded `WireGuard` key into a 32-byte array.
///
/// Used on Windows where we drive `Tunn` directly and therefore need
/// raw key bytes rather than the hex encoding UAPI uses.
#[cfg(windows)]
fn decode_key_b64(b64: &str) -> Result<[u8; 32], Box<dyn std::error::Error>> {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    let bytes = STANDARD.decode(b64)?;
    if bytes.len() != 32 {
        return Err(format!(
            "invalid WireGuard key length: expected 32 bytes, got {}",
            bytes.len()
        )
        .into());
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(out)
}

/// Extract the destination IP from a raw IPv4 or IPv6 packet.
///
/// IPv4: version in the top 4 bits of byte 0, dst in bytes 16..20.
/// IPv6: version in the top 4 bits of byte 0, dst in bytes 24..40.
/// Returns `None` for non-IP (version mismatch) or truncated packets.
#[cfg(windows)]
fn parse_dst_ip(packet: &[u8]) -> Option<IpAddr> {
    if packet.is_empty() {
        return None;
    }
    match packet[0] >> 4 {
        4 if packet.len() >= 20 => {
            let b: [u8; 4] = packet[16..20].try_into().ok()?;
            Some(IpAddr::from(b))
        }
        6 if packet.len() >= 40 => {
            let b: [u8; 16] = packet[24..40].try_into().ok()?;
            Some(IpAddr::from(b))
        }
        _ => None,
    }
}

/// Build a new `Tunn` from raw key material.
///
/// Any boringtun construction error is surfaced as
/// `OverlayError::NetworkConfig` via the caller's error mapping.
/// `index=0` lets boringtun assign its own internal session indices;
/// `rate_limiter=None` uses the per-tunnel default.
#[cfg(windows)]
fn build_tunn(
    our_priv: &[u8; 32],
    peer_pub: &[u8; 32],
    preshared: Option<[u8; 32]>,
    persistent_keepalive: Option<u16>,
) -> Tunn {
    let priv_secret = boringtun::x25519::StaticSecret::from(*our_priv);
    let peer_pub_key = boringtun::x25519::PublicKey::from(*peer_pub);
    // `Tunn::new` returns `Self` directly in boringtun 0.7 — no Result.
    Tunn::new(
        priv_secret,
        peer_pub_key,
        preshared,
        persistent_keepalive,
        0,
        None,
    )
}

// ---------------------------------------------------------------------------
// OverlayTransport
// ---------------------------------------------------------------------------

/// Encrypted overlay transport layer.
///
/// Uses boringtun (userspace `WireGuard`) to create TUN-based encrypted tunnels.
/// No kernel `WireGuard` module required.
///
/// **Important:** This struct holds the boringtun [`DeviceHandle`] (or, on
/// Windows, the Wintun adapter + spawned loop tasks). Dropping the struct
/// tears down the TUN device. Callers **must** keep this struct alive for
/// the entire overlay network lifetime.
pub struct OverlayTransport {
    config: OverlayConfig,
    interface_name: String,
    /// boringtun-managed TUN device handle (Linux/macOS).
    /// On Windows we own the adapter via `wintun` instead — see `wintun_dev`.
    #[cfg(not(windows))]
    device: Option<DeviceHandle>,
    /// Wintun adapter + session handle (Windows only).
    ///
    /// Wrapped in `Arc` so ingress / egress tasks can hold references
    /// while the transport itself remains owned by the caller. Dropping
    /// the last Arc tears down the Wintun session and removes the
    /// adapter, mirroring the `DeviceHandle::drop` semantics on Unix.
    #[cfg(windows)]
    wintun_dev: Option<Arc<WindowsTun>>,
    /// UDP socket servicing the `WireGuard` protocol for this overlay
    /// (Windows only). Bound to `OverlayConfig::local_endpoint` in
    /// `configure`; shared across ingress / egress / timers tasks.
    #[cfg(windows)]
    udp: Option<Arc<UdpSocket>>,
    /// Per-peer Noise state + metadata, keyed by raw 32-byte public key.
    /// `DashMap` lets the ingress task (write to endpoint / handshake
    /// timestamp), the egress task (read endpoint), and the timers task
    /// (encapsulate keepalives) all mutate independent shards without
    /// contending on a global lock.
    #[cfg(windows)]
    peers: Arc<DashMap<[u8; 32], WindowsPeerState>>,
    /// Ingress task: UDP → `Tunn::decapsulate` → Wintun send.
    #[cfg(windows)]
    ingress_task: Option<JoinHandle<()>>,
    /// Egress task: Wintun recv → `Tunn::encapsulate` → UDP send.
    #[cfg(windows)]
    egress_task: Option<JoinHandle<()>>,
    /// Timers task: ~250 ms tick driving `Tunn::update_timers` per peer
    /// to fire keepalives and handshake re-initiations.
    #[cfg(windows)]
    timers_task: Option<JoinHandle<()>>,
}

impl OverlayTransport {
    /// Create a new overlay transport (device is not started yet).
    #[must_use]
    pub fn new(config: OverlayConfig, interface_name: String) -> Self {
        Self {
            config,
            interface_name,
            #[cfg(not(windows))]
            device: None,
            #[cfg(windows)]
            wintun_dev: None,
            #[cfg(windows)]
            udp: None,
            #[cfg(windows)]
            peers: Arc::new(DashMap::new()),
            #[cfg(windows)]
            ingress_task: None,
            #[cfg(windows)]
            egress_task: None,
            #[cfg(windows)]
            timers_task: None,
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
    ///
    /// Linux/macOS only — Windows does not use UAPI.
    ///
    /// Derived from [`OverlayConfig::uapi_sock_dir`] so callers running
    /// with a non-default `--data-dir` can scope their UAPI sockets to
    /// their own data directory (avoids collisions with a system-wide
    /// zlayer install that owns `/var/run/wireguard`).
    #[cfg(not(windows))]
    fn uapi_sock_path(&self) -> String {
        self.effective_uapi_sock_dir()
            .join(format!("{}.sock", self.interface_name))
            .to_string_lossy()
            .into_owned()
    }

    /// The directory the boringtun `device` UAPI control socket actually lives
    /// in.
    ///
    /// On Linux/Windows this is the configured [`OverlayConfig::uapi_sock_dir`]
    /// (data-dir-scoped to avoid cross-instance collisions). On **macOS**,
    /// boringtun's `device` feature HARDCODES its socket directory to
    /// `/var/run/wireguard/` (`boringtun`'s `SOCK_DIR`) and ignores any
    /// configured path, so we MUST look there — otherwise the post-create
    /// discovery scan and the UAPI `set` connect to a directory boringtun never
    /// writes to and fail with `ENOENT` ("Failed to configure global overlay:
    /// No such file or directory"). Kernel-assigned `utunN` names keep the
    /// per-socket filenames unique, so a shared `/var/run/wireguard` does not
    /// collide across instances on macOS.
    #[cfg(not(windows))]
    #[cfg_attr(target_os = "macos", allow(clippy::unused_self))]
    fn effective_uapi_sock_dir(&self) -> &std::path::Path {
        #[cfg(target_os = "macos")]
        {
            std::path::Path::new("/var/run/wireguard")
        }
        #[cfg(not(target_os = "macos"))]
        {
            self.config.uapi_sock_dir.as_path()
        }
    }

    /// Create the TUN interface.
    ///
    /// On Linux/macOS this spawns boringtun worker threads that manage
    /// the TUN device. On Windows it instantiates a Wintun adapter (no
    /// boringtun threads — the Windows packet loop is driven directly
    /// via `boringtun::noise::Tunn` in a follow-up task; today this
    /// only stands the adapter up so IP configuration can run).
    ///
    /// The device is torn down when this struct is dropped (or
    /// [`shutdown`] is called).
    ///
    /// On Linux, creates a named TUN interface (requires `CAP_NET_ADMIN`).
    /// On macOS, creates a kernel-assigned `utunN` interface (requires `sudo`).
    /// On Windows, creates a Wintun adapter (requires Administrator and
    /// `wintun.dll` on disk — see [`crate::tun::windows`]).
    ///
    /// # Errors
    ///
    /// Returns an error if the TUN device cannot be created or required
    /// privileges are unavailable.
    pub async fn create_interface(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        #[cfg(windows)]
        {
            self.create_interface_windows().await
        }
        #[cfg(not(windows))]
        {
            self.create_interface_unix().await
        }
    }

    /// Linux / macOS implementation of [`Self::create_interface`].
    #[cfg(not(windows))]
    #[allow(clippy::too_many_lines)]
    async fn create_interface_unix(&mut self) -> Result<(), Box<dyn std::error::Error>> {
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

        // Ensure the UAPI socket directory exists. On macOS this is boringtun's
        // hardcoded `/var/run/wireguard`; on Linux it is the data-dir-aware
        // `OverlayConfig::uapi_sock_dir`. See `effective_uapi_sock_dir`.
        let uapi_dir = self.effective_uapi_sock_dir().to_path_buf();
        tokio::fs::create_dir_all(&uapi_dir).await?;

        // On Linux, refuse to silently delete an existing kernel link. Stale
        // interfaces from a previous crashed daemon are swept by the
        // boot-time cleanup in `bin/zlayer/src/commands/serve.rs::cleanup_stale_daemon`.
        // If a link with this name still exists when we get here, that's a
        // real duplicate-name bug (or a foreign owner) and silently deleting
        // it would yank the TUN out from under another live boringtun
        // worker, producing the upstream "Fatal read error on tun interface:
        // Os { code: 77 }" (EBADFD) symptom.
        //
        // macOS utun devices are kernel-managed and auto-destroyed when the
        // owning socket closes, so this check is Linux-only.
        #[cfg(target_os = "linux")]
        {
            let iface_ops = platform_ops();
            match iface_ops.link_exists(&self.interface_name).await {
                Ok(true) => {
                    return Err(format!(
                        "Kernel link '{}' already exists; refusing to delete it. \
                         If this is a stale interface from a previous crash, restart \
                         the daemon (its boot-time sweep clears stale zl-* / veth-* \
                         links). If this fires during normal operation, there is a \
                         duplicate-name bug somewhere in the overlay setup path.",
                        self.interface_name
                    )
                    .into());
                }
                Ok(false) => {}
                Err(e) => {
                    tracing::warn!(
                        interface = %self.interface_name,
                        error = %e,
                        "failed to probe for existing overlay interface; proceeding"
                    );
                }
            }
        }

        // Clean up stale UAPI socket left behind by a crashed process.
        let sock_path = self
            .effective_uapi_sock_dir()
            .join(format!("{}.sock", self.interface_name));
        if tokio::fs::try_exists(&sock_path).await.unwrap_or(false) {
            tracing::warn!(path = %sock_path.display(), "removing stale UAPI socket");
            let _ = tokio::fs::remove_file(&sock_path).await;
        }

        // On macOS, snapshot existing UAPI sockets so we can discover the
        // kernel-assigned utunN name after device creation.
        #[cfg(target_os = "macos")]
        let existing_socks = {
            let mut set = std::collections::HashSet::new();
            if let Ok(mut entries) = tokio::fs::read_dir(&uapi_dir).await {
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

        // boringtun 0.7's `DeviceConfig` exposes no hook to inject the
        // WireGuard *data* socket: `uapi_fd` is the UAPI *control* socket
        // only (the `wg`-style configuration channel), not the UDP transport
        // socket. There is therefore no supported way to pin the data socket
        // with SO_BINDTODEVICE to a specific egress NIC. Source-IP selection
        // is instead achieved via `local_endpoint` (the bind address) plus a
        // correctly advertised peer endpoint — that is the full extent of the
        // egress control surface boringtun 0.7 gives us.
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
                let hint = "Requires root. Run with sudo or install as a system service (zlayer daemon install).";
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
            if let Ok(mut entries) = tokio::fs::read_dir(&uapi_dir).await {
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

    /// Windows implementation of [`Self::create_interface`].
    ///
    /// Stands up a Wintun adapter named `self.interface_name`. The
    /// packet-forwarding loop (UDP ↔ Tunn ↔ Wintun) is started later
    /// from [`Self::configure`] once the peer set + listen port are
    /// known.
    #[cfg(windows)]
    async fn create_interface_windows(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        // Wintun has no IFNAMSIZ-style limit, but very long names are
        // a sign of a misconfigured caller and look terrible in the UI.
        if self.interface_name.len() > 64 {
            return Err(format!(
                "Wintun adapter name '{}' exceeds 64 character limit",
                self.interface_name
            )
            .into());
        }

        let iface_name = self.interface_name.clone();
        // MTU is driven by OverlayConfig (default 1420). Wintun fixes the MTU
        // at adapter-create time and exposes no per-adapter setter afterward,
        // so this is the only place it can be applied on Windows.
        let mtu = self.config.mtu;

        // WindowsTun::new is synchronous — wrap in spawn_blocking so we
        // don't stall the runtime if Wintun's load_from_path / adapter
        // creation is slow on a busy host.
        let dev = tokio::task::spawn_blocking(move || WindowsTun::new(&iface_name, mtu))
            .await
            .map_err(|e| format!("spawn_blocking join error: {e}"))??;

        tracing::info!(
            interface = %self.interface_name,
            luid = dev.luid_value(),
            "Created Wintun overlay adapter"
        );

        self.wintun_dev = Some(Arc::new(dev));
        Ok(())
    }

    /// Configure the transport with private key, listen port, and peers.
    ///
    /// On Linux/macOS this writes the `WireGuard` configuration via UAPI
    /// to boringtun's per-interface socket, then assigns the overlay IP
    /// and brings the interface up via [`InterfaceOps`].
    ///
    /// On Windows this binds the UDP listener, creates a
    /// `boringtun::noise::Tunn` instance for each configured peer,
    /// spawns the ingress / egress / timers tasks that drive the noise
    /// protocol against the Wintun adapter, and configures the IP layer.
    ///
    /// # Errors
    ///
    /// On Linux/macOS, returns an error if UAPI configuration or IP
    /// assignment fails. On Windows, returns an error if the Wintun
    /// adapter is missing, the UDP socket cannot be bound, key decoding
    /// fails, or the IP layer cannot be configured.
    pub async fn configure(
        &mut self,
        peers: &[PeerInfo],
    ) -> Result<(), Box<dyn std::error::Error>> {
        #[cfg(not(windows))]
        {
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

        #[cfg(windows)]
        {
            self.configure_windows(peers).await
        }
    }

    /// Windows implementation of [`Self::configure`].
    ///
    /// Responsibilities (in order):
    /// 1. Configure the IP/route layer so host state is consistent.
    /// 2. Bind the `WireGuard` UDP socket on `local_endpoint`.
    /// 3. Build a `Tunn` per peer and seed `self.peers`.
    /// 4. Spawn the three loop tasks (ingress, egress, timers).
    #[cfg(windows)]
    async fn configure_windows(
        &mut self,
        peers: &[PeerInfo],
    ) -> Result<(), Box<dyn std::error::Error>> {
        // IP layer first.
        self.configure_interface().await?;

        // Install a catch-all host route for the full cluster CIDR via the
        // Wintun adapter. HCN auto-installs the more specific per-node /28 →
        // vSwitch route, so longest-prefix-match sends local-slice traffic to
        // the vSwitch and remote-slice traffic to Wintun (where boringtun's
        // egress loop picks it up, encapsulates, and forwards to the owning
        // peer). Without this route, packets destined for remote container
        // IPs leak out of the default gateway instead of the overlay.
        //
        // Failure is logged as a warning but not fatal — the route may
        // already exist from a previous run, and we want adapter bringup to
        // remain idempotent.
        if let Some(ref cluster_cidr_str) = self.config.cluster_cidr {
            match cluster_cidr_str.parse::<ipnet::IpNet>() {
                Ok(net) => {
                    use crate::interface::windows::WindowsIpHelperOps;
                    use crate::interface::InterfaceOps;
                    let ops = WindowsIpHelperOps::new();
                    let adapter_name = self.interface_name.clone();
                    match ops
                        .add_route_via_dev(net.network(), net.prefix_len(), &adapter_name)
                        .await
                    {
                        Ok(()) => {
                            tracing::info!(
                                cidr = %net,
                                adapter = %adapter_name,
                                "Installed cluster-CIDR host route via Wintun adapter"
                            );
                        }
                        Err(e) => {
                            tracing::warn!(
                                error = %e,
                                cidr = %net,
                                adapter = %adapter_name,
                                "Failed to install cluster-CIDR host route via Wintun (overlay traffic may not route across nodes); route may already exist"
                            );
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        cidr = %cluster_cidr_str,
                        "cluster_cidr unparseable; skipping Wintun route install"
                    );
                }
            }
        } else {
            tracing::warn!(
                "cluster_cidr not set in OverlayConfig; skipping Wintun route install (cross-node overlay traffic may not route)"
            );
        }

        // Wintun adapter must already be up via `create_interface`.
        let tun = self
            .wintun_dev
            .as_ref()
            .ok_or("Wintun adapter not initialized — call create_interface first")?
            .clone();

        // Bind the WireGuard UDP socket. Use the configured local
        // endpoint so IPv4 / IPv6 family matches the overlay.
        let listen = self.config.local_endpoint;
        let udp = Arc::new(
            UdpSocket::bind(listen)
                .await
                .map_err(|e| format!("failed to bind WireGuard UDP socket on {listen}: {e}"))?,
        );
        self.udp = Some(udp.clone());

        // Seed peers from the initial config.
        let priv_bytes = decode_key_b64(&self.config.private_key)?;
        for peer in peers {
            self.add_peer_windows(&priv_bytes, peer)?;
        }

        // Spawn the three driver tasks. They hold Arc clones of the
        // shared state so they outlive individual peer inserts /
        // removes; aborted during `shutdown`.
        let peers_ingress = self.peers.clone();
        let udp_ingress = udp.clone();
        let tun_ingress = tun.clone();
        self.ingress_task = Some(tokio::spawn(async move {
            Self::ingress_loop(udp_ingress, tun_ingress, peers_ingress).await;
        }));

        let peers_egress = self.peers.clone();
        let udp_egress = udp.clone();
        let tun_egress = tun.clone();
        self.egress_task = Some(tokio::spawn(async move {
            Self::egress_loop(tun_egress, udp_egress, peers_egress).await;
        }));

        let peers_timers = self.peers.clone();
        let udp_timers = udp.clone();
        self.timers_task = Some(tokio::spawn(async move {
            Self::timers_loop(udp_timers, peers_timers).await;
        }));

        tracing::info!(
            interface = %self.interface_name,
            peer_count = peers.len(),
            listen = %listen,
            "Windows overlay transport configured (Tunn pipeline online)"
        );
        Ok(())
    }

    /// Insert (or replace) a peer in the Windows peer map.
    ///
    /// Used both by `configure_windows` at bootstrap time and by the
    /// public `add_peer` entry point. Assumes `self.config.private_key`
    /// has already been decoded (the caller passes the raw bytes to
    /// avoid re-decoding per peer during bulk seeding).
    #[cfg(windows)]
    fn add_peer_windows(
        &self,
        our_priv: &[u8; 32],
        peer: &PeerInfo,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let peer_pub = decode_key_b64(&peer.public_key)?;
        let allowed: ipnet::IpNet = peer
            .allowed_ips
            .parse()
            .map_err(|e| format!("invalid allowed_ips '{}': {e}", peer.allowed_ips))?;
        // WireGuard persistent_keepalive_interval of 0 means "disabled",
        // which boringtun models as `None`.
        let keepalive = {
            let secs = peer.persistent_keepalive_interval.as_secs();
            if secs == 0 {
                None
            } else {
                u16::try_from(secs).ok()
            }
        };

        let tunn = build_tunn(our_priv, &peer_pub, None, keepalive);
        let state = WindowsPeerState {
            tunn: Arc::new(AsyncMutex::new(tunn)),
            endpoint: Arc::new(RwLock::new(Some(peer.endpoint))),
            last_handshake_sec: Arc::new(AtomicU64::new(0)),
            allowed_ips: Arc::new(vec![allowed]),
            persistent_keepalive: keepalive,
        };
        self.peers.insert(peer_pub, state);
        tracing::debug!(
            peer_key = %peer.public_key,
            endpoint = %peer.endpoint,
            allowed = %peer.allowed_ips,
            "Added peer to Windows overlay peer map"
        );
        Ok(())
    }

    /// UDP → decapsulate → Wintun loop.
    ///
    /// For each inbound datagram we linear-scan the peer map and hand
    /// the packet to each `Tunn::decapsulate` until one returns a
    /// non-error result. This is O(N peers) per packet; for the
    /// small-cluster overlays `ZLayer` targets it is fine. A future
    /// optimization can cache `src_addr → pubkey` once sessions are
    /// established.
    ///
    /// `decapsulate` returns:
    /// - `WriteToTunnelV4` / `WriteToTunnelV6` — cleartext IP packet to
    ///   inject into Wintun.
    /// - `WriteToNetwork` — an auto-generated WG reply (cookie / 2nd
    ///   handshake message) to echo back to the remote.
    /// - `Done` / `Err` — not our peer, keep scanning.
    ///
    /// After a successful decap, we loop on an empty-datagram call
    /// to drain any queued packets boringtun buffered during the
    /// handshake (documented behavior of `decapsulate`).
    #[cfg(windows)]
    async fn ingress_loop(
        udp: Arc<UdpSocket>,
        tun: Arc<WindowsTun>,
        peers: Arc<DashMap<[u8; 32], WindowsPeerState>>,
    ) {
        // 65536 covers the largest possible IPv4/IPv6 datagram.
        let mut inbuf = vec![0u8; 65536];
        loop {
            let (n, src) = match udp.recv_from(&mut inbuf).await {
                Ok(p) => p,
                Err(e) => {
                    tracing::error!(error = %e, "UDP recv failed; ingress loop exiting");
                    break;
                }
            };

            // Snapshot (pubkey, state) pairs so we release the DashMap
            // shard lock before awaiting on the async per-peer Mutex.
            let snapshot: Vec<([u8; 32], WindowsPeerState)> = peers
                .iter()
                .map(|e| (*e.key(), e.value().clone()))
                .collect();

            for (pk, state) in snapshot {
                let mut out = vec![0u8; 65536];
                let mut handled = false;
                {
                    let mut tunn = state.tunn.lock().await;
                    match tunn.decapsulate(Some(src.ip()), &inbuf[..n], &mut out) {
                        TunnResult::WriteToTunnelV4(pkt, _)
                        | TunnResult::WriteToTunnelV6(pkt, _) => {
                            let pkt_owned = pkt.to_vec();
                            drop(tunn);
                            if let Err(e) = tun.send(&pkt_owned).await {
                                tracing::warn!(error = %e, "Wintun send failed");
                            }
                            *state.endpoint.write() = Some(src);
                            state.last_handshake_sec.store(
                                SystemTime::now()
                                    .duration_since(UNIX_EPOCH)
                                    .unwrap_or_default()
                                    .as_secs(),
                                Ordering::Relaxed,
                            );
                            handled = true;
                        }
                        TunnResult::WriteToNetwork(resp) => {
                            let resp_owned = resp.to_vec();
                            drop(tunn);
                            if let Err(e) = udp.send_to(&resp_owned, src).await {
                                tracing::warn!(error = %e, "UDP reply send failed");
                            }
                            *state.endpoint.write() = Some(src);
                            handled = true;
                        }
                        TunnResult::Done | TunnResult::Err(_) => {
                            // Not this peer — try the next.
                        }
                    }
                }
                if handled {
                    // Drain queued packets: boringtun buffers data
                    // packets that arrived before the handshake
                    // completed; passing an empty datagram releases
                    // them one at a time until `Done`.
                    loop {
                        let mut drain = vec![0u8; 65536];
                        let mut tunn = state.tunn.lock().await;
                        match tunn.decapsulate(None, &[], &mut drain) {
                            TunnResult::WriteToNetwork(resp) => {
                                let resp_owned = resp.to_vec();
                                drop(tunn);
                                if let Err(e) = udp.send_to(&resp_owned, src).await {
                                    tracing::warn!(error = %e, "UDP drain send failed");
                                }
                            }
                            TunnResult::WriteToTunnelV4(pkt, _)
                            | TunnResult::WriteToTunnelV6(pkt, _) => {
                                let pkt_owned = pkt.to_vec();
                                drop(tunn);
                                if let Err(e) = tun.send(&pkt_owned).await {
                                    tracing::warn!(error = %e, "Wintun drain send failed");
                                }
                            }
                            TunnResult::Done | TunnResult::Err(_) => break,
                        }
                    }
                    let _ = pk; // peer matched; stop scanning.
                    break;
                }
            }
        }
    }

    /// Wintun → encapsulate → UDP loop.
    ///
    /// Parses the destination IP from the outbound clear packet,
    /// matches it against each peer's `allowed_ips`, encapsulates with
    /// that peer's `Tunn`, and writes the ciphertext to UDP. If no
    /// endpoint is known yet the packet is dropped silently — callers
    /// typically retry at a higher layer, and `update_timers` will be
    /// firing handshake initiations independently.
    #[cfg(windows)]
    async fn egress_loop(
        tun: Arc<WindowsTun>,
        udp: Arc<UdpSocket>,
        peers: Arc<DashMap<[u8; 32], WindowsPeerState>>,
    ) {
        let mut buf = vec![0u8; 65536];
        loop {
            let n = match tun.recv(&mut buf).await {
                Ok(n) => n,
                Err(e) => {
                    tracing::error!(error = %e, "Wintun recv failed; egress loop exiting");
                    break;
                }
            };

            let Some(dst_ip) = parse_dst_ip(&buf[..n]) else {
                continue;
            };

            // Find the first peer whose allowed_ips contains dst_ip.
            let state = peers.iter().find_map(|entry| {
                if entry
                    .value()
                    .allowed_ips
                    .iter()
                    .any(|net| net.contains(&dst_ip))
                {
                    Some(entry.value().clone())
                } else {
                    None
                }
            });
            let Some(state) = state else {
                tracing::trace!(%dst_ip, "no matching overlay peer");
                continue;
            };

            let endpoint = *state.endpoint.read();
            let Some(endpoint) = endpoint else {
                tracing::trace!(%dst_ip, "peer has no endpoint yet; dropping");
                continue;
            };

            // `encapsulate` requires dst ≥ src.len() + 32 and ≥ 148.
            // We size to 64 KiB + 32 to cover any legal IP packet plus
            // the WG overhead.
            let mut out = vec![0u8; 65536 + 32];
            let mut tunn = state.tunn.lock().await;
            match tunn.encapsulate(&buf[..n], &mut out) {
                TunnResult::WriteToNetwork(pkt) => {
                    let pkt_owned = pkt.to_vec();
                    drop(tunn);
                    if let Err(e) = udp.send_to(&pkt_owned, endpoint).await {
                        tracing::warn!(error = %e, "UDP send failed");
                    }
                }
                TunnResult::Done
                | TunnResult::WriteToTunnelV4(_, _)
                | TunnResult::WriteToTunnelV6(_, _) => {
                    // `Done`: packet queued inside boringtun pending
                    // handshake; nothing to emit right now.
                    // `WriteToTunnel*`: encapsulate never produces
                    // TUN-ward results, but we treat them as no-ops for
                    // exhaustiveness.
                }
                TunnResult::Err(e) => {
                    tracing::warn!(?e, "encapsulate error");
                }
            }
        }
    }

    /// Per-peer periodic `update_timers` tick.
    ///
    /// Fires every 250 ms (the cadence boringtun's reference
    /// implementation uses) to emit keepalives and re-initiate stale
    /// handshakes. `update_timers` writes at most 148 bytes (max WG
    /// handshake init length), so the scratch buffer is sized
    /// accordingly.
    #[cfg(windows)]
    async fn timers_loop(udp: Arc<UdpSocket>, peers: Arc<DashMap<[u8; 32], WindowsPeerState>>) {
        let mut interval = tokio::time::interval(Duration::from_millis(250));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            let snapshot: Vec<WindowsPeerState> = peers.iter().map(|e| e.value().clone()).collect();
            for state in snapshot {
                let endpoint = *state.endpoint.read();
                let mut out = vec![0u8; 148];
                let mut tunn = state.tunn.lock().await;
                match tunn.update_timers(&mut out) {
                    TunnResult::WriteToNetwork(pkt) => {
                        let pkt_owned = pkt.to_vec();
                        drop(tunn);
                        if let Some(ep) = endpoint {
                            if let Err(e) = udp.send_to(&pkt_owned, ep).await {
                                tracing::debug!(error = %e, "timers UDP send failed");
                            }
                        }
                    }
                    TunnResult::Done
                    | TunnResult::WriteToTunnelV4(_, _)
                    | TunnResult::WriteToTunnelV6(_, _) => {}
                    TunnResult::Err(e) => {
                        tracing::debug!(?e, "update_timers error");
                    }
                }
            }
        }
    }

    /// Platform-agnostic interface IP assignment and bring-up.
    ///
    /// Supports both IPv4 and IPv6 overlay CIDRs. The per-OS details
    /// (RTNETLINK on Linux, `ifconfig` / `route` shell-outs on macOS)
    /// are hidden behind the [`InterfaceOps`] trait.
    ///
    /// Idempotency behavior is preserved byte-for-byte:
    /// - `add_address` failures whose message contains `"File exists"`
    ///   or `"EEXIST"` (Linux RTNETLINK EEXIST on re-add) are swallowed.
    /// - `add_route_via_dev` failures whose message contains
    ///   `"File exists"` / `"EEXIST"` (Linux) or `"already in table"`
    ///   (BSD route) are swallowed inside the macOS implementation; the
    ///   Linux implementation returns the error, and we swallow it here
    ///   via the same text-match gate used by the original code.
    async fn configure_interface(&self) -> Result<(), Box<dyn std::error::Error>> {
        let cidr: ipnet::IpNet = self.config.overlay_cidr.parse().map_err(|e| {
            format!(
                "Failed to parse overlay CIDR '{}': {e}",
                self.config.overlay_cidr
            )
        })?;
        let overlay_addr = cidr.addr();
        let prefix_len = cidr.prefix_len();
        let net_addr = cidr.network();

        let iface_ops = platform_ops();

        // Assign overlay IP address — handles both IPv4 and IPv6.
        // Preserve original idempotency: swallow EEXIST / "File exists"
        // since a previous run may have left the address configured on
        // a still-live TUN device.
        if let Err(e) = iface_ops
            .add_address(&self.interface_name, overlay_addr, prefix_len)
            .await
        {
            let msg = e.to_string();
            if !msg.contains("File exists") && !msg.contains("EEXIST") {
                return Err(format!("Failed to assign IP: {msg}").into());
            }
        }

        // Set the MTU before bringing the interface up. The overlay TUN
        // carries WireGuard-in-WireGuard over a mesh, so the effective
        // payload budget is below the physical link MTU; an un-tuned MTU
        // silently blackholes oversized packets. `self.interface_name` is
        // the kernel-assigned utunN name on macOS (discovered during
        // `create_interface` before this runs). A failure here is logged
        // and tolerated — a degraded (un-tuned) MTU is strictly better
        // than aborting overlay bring-up entirely.
        if let Err(e) = iface_ops
            .set_mtu(&self.interface_name, self.config.mtu)
            .await
        {
            tracing::warn!(
                interface = %self.interface_name,
                mtu = self.config.mtu,
                error = %e,
                "Failed to set overlay interface MTU; continuing with the kernel default (oversized packets may blackhole)"
            );
        }

        // Bring interface up. On macOS this is redundant with the
        // `up` token passed into ifconfig during `add_address`, but
        // harmless.
        iface_ops
            .set_link_up(&self.interface_name)
            .await
            .map_err(|e| format!("Failed to bring up interface: {e}"))?;

        // Add explicit route for the overlay subnet. Preserve original
        // idempotency: swallow EEXIST since the kernel may auto-install
        // a connected route when the address is assigned.
        if let Err(e) = iface_ops
            .add_route_via_dev(net_addr, prefix_len, &self.interface_name)
            .await
        {
            let msg = e.to_string();
            if !msg.contains("File exists")
                && !msg.contains("EEXIST")
                && !msg.contains("already in table")
            {
                return Err(format!("Failed to add route: {msg}").into());
            }
        }

        Ok(())
    }

    /// Add a peer dynamically.
    ///
    /// On Linux/macOS this writes to boringtun's UAPI socket. On
    /// Windows it returns an error until the per-peer
    /// `boringtun::noise::Tunn` map is wired (Phase D3.x).
    ///
    /// # Errors
    ///
    /// Returns an error if the key conversion or UAPI command fails on
    /// Linux/macOS, or always on Windows (until the packet loop lands).
    #[cfg_attr(windows, allow(clippy::unused_async))]
    pub async fn add_peer(&self, peer: &PeerInfo) -> Result<(), Box<dyn std::error::Error>> {
        #[cfg(not(windows))]
        {
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
        #[cfg(windows)]
        {
            let priv_bytes = decode_key_b64(&self.config.private_key)?;
            self.add_peer_windows(&priv_bytes, peer)?;
            Ok(())
        }
    }

    /// Remove a peer.
    ///
    /// On Linux/macOS this writes to boringtun's UAPI socket. On
    /// Windows it returns an error until the per-peer
    /// `boringtun::noise::Tunn` map is wired (Phase D3.x).
    ///
    /// # Errors
    ///
    /// Returns an error if the key conversion or UAPI command fails on
    /// Linux/macOS, or always on Windows (until the packet loop lands).
    #[cfg_attr(windows, allow(clippy::unused_async))]
    pub async fn remove_peer(&self, public_key: &str) -> Result<(), Box<dyn std::error::Error>> {
        #[cfg(not(windows))]
        {
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
        #[cfg(windows)]
        {
            let pk = decode_key_b64(public_key)?;
            self.peers.remove(&pk);
            tracing::debug!(
                peer_key = %public_key,
                interface = %self.interface_name,
                "Removed peer from Windows overlay"
            );
            Ok(())
        }
    }

    /// Add a CIDR to an existing peer's `AllowedIPs` list.
    ///
    /// The peer must already be configured via [`Self::add_peer`].
    /// Returns an error if the peer is not found.
    ///
    /// On Linux/macOS this issues a UAPI `set` request with
    /// `replace_allowed_ips=true` and the new full set (old set ∪ {cidr}).
    /// On Windows this mutates the in-memory peer entry's `allowed_ips`
    /// (which the egress loop consults on every packet) by replacing the
    /// `Arc<Vec<IpNet>>` wholesale; snapshot clones taken by in-flight
    /// loops continue to see the old set until they take their next
    /// snapshot.
    ///
    /// Idempotent: if `cidr` is already present in the peer's
    /// `AllowedIPs`, the set is left unchanged (but a UAPI replay still
    /// happens on Linux/macOS to keep the kernel/user view authoritative).
    ///
    /// Used by `OverlayManager` to add a per-service subnet to the
    /// cluster transport's `AllowedIPs` when a new service overlay is set
    /// up.
    ///
    /// # Errors
    ///
    /// Returns an error if the peer public key is invalid, the peer is
    /// not found in the wireguard configuration, or the UAPI / Wintun
    /// state update fails.
    #[cfg_attr(windows, allow(clippy::unused_async))]
    pub async fn add_allowed_ip(
        &self,
        public_key: &str,
        cidr: ipnet::IpNet,
    ) -> Result<(), Box<dyn std::error::Error>> {
        #[cfg(not(windows))]
        {
            let sock = self.uapi_sock_path();
            let pub_hex = key_to_hex(public_key)?;

            // Read current allowed_ips for this peer from UAPI so we can
            // re-issue the full set (boringtun's UAPI is replace-or-append:
            // without `replace_allowed_ips=true`, lone `allowed_ip=` lines
            // append; with it, they fully replace).
            let current = uapi_get(&sock).await?;
            let mut current_set = parse_allowed_ips_for_peer(&current, &pub_hex);

            if !peer_exists_in_uapi(&current, &pub_hex) {
                return Err(
                    format!("cannot add allowed_ip {cidr} to unknown peer {public_key}").into(),
                );
            }

            // Idempotent insert.
            if !current_set.contains(&cidr) {
                current_set.push(cidr);
            }

            let mut body = format!("public_key={pub_hex}\nreplace_allowed_ips=true\n");
            for net in &current_set {
                let _ = writeln!(body, "allowed_ip={net}");
            }

            uapi_set(&sock, &body).await?;
            tracing::debug!(
                peer_key = %public_key,
                cidr = %cidr,
                count = current_set.len(),
                "Added allowed_ip via UAPI"
            );
            Ok(())
        }
        #[cfg(windows)]
        {
            let pk = decode_key_b64(public_key)?;
            let mut entry = self.peers.get_mut(&pk).ok_or_else(|| {
                format!("cannot add allowed_ip {cidr} to unknown peer {public_key}")
            })?;
            let mut new_set: Vec<ipnet::IpNet> = entry.value().allowed_ips.as_ref().clone();
            if !new_set.contains(&cidr) {
                new_set.push(cidr);
            }
            entry.value_mut().allowed_ips = Arc::new(new_set);
            tracing::debug!(
                peer_key = %public_key,
                cidr = %cidr,
                "Added allowed_ip to Windows overlay peer map"
            );
            Ok(())
        }
    }

    /// Remove a CIDR from an existing peer's `AllowedIPs` list.
    ///
    /// Idempotent in two ways:
    /// - If the CIDR is not present in the peer's `AllowedIPs`, returns
    ///   `Ok(())` without error.
    /// - If the peer itself is not found, returns `Ok(())` without
    ///   error (mirrors the "missing == already-removed" semantics
    ///   `OverlayManager` needs during teardown races).
    ///
    /// On Linux/macOS this issues a UAPI `set` request with
    /// `replace_allowed_ips=true` and the new full set (old set ∖ {cidr}).
    /// On Windows this mutates the in-memory peer entry's `allowed_ips`
    /// by replacing the `Arc<Vec<IpNet>>` wholesale.
    ///
    /// # Errors
    ///
    /// Returns an error only if the peer public key is malformed or the
    /// UAPI / Wintun state update fails.
    #[cfg_attr(windows, allow(clippy::unused_async))]
    pub async fn remove_allowed_ip(
        &self,
        public_key: &str,
        cidr: ipnet::IpNet,
    ) -> Result<(), Box<dyn std::error::Error>> {
        #[cfg(not(windows))]
        {
            let sock = self.uapi_sock_path();
            let pub_hex = key_to_hex(public_key)?;

            let current = uapi_get(&sock).await?;
            if !peer_exists_in_uapi(&current, &pub_hex) {
                // Peer not configured — nothing to remove. Idempotent.
                tracing::debug!(
                    peer_key = %public_key,
                    cidr = %cidr,
                    "remove_allowed_ip: peer not found in UAPI; treating as no-op"
                );
                return Ok(());
            }
            let current_set = parse_allowed_ips_for_peer(&current, &pub_hex);
            let new_set: Vec<ipnet::IpNet> =
                current_set.into_iter().filter(|c| *c != cidr).collect();

            let mut body = format!("public_key={pub_hex}\nreplace_allowed_ips=true\n");
            for net in &new_set {
                let _ = writeln!(body, "allowed_ip={net}");
            }

            uapi_set(&sock, &body).await?;
            tracing::debug!(
                peer_key = %public_key,
                cidr = %cidr,
                count = new_set.len(),
                "Removed allowed_ip via UAPI"
            );
            Ok(())
        }
        #[cfg(windows)]
        {
            let pk = decode_key_b64(public_key)?;
            let Some(mut entry) = self.peers.get_mut(&pk) else {
                tracing::debug!(
                    peer_key = %public_key,
                    cidr = %cidr,
                    "remove_allowed_ip: peer not found in Windows peer map; no-op"
                );
                return Ok(());
            };
            let new_set: Vec<ipnet::IpNet> = entry
                .value()
                .allowed_ips
                .iter()
                .copied()
                .filter(|c| *c != cidr)
                .collect();
            entry.value_mut().allowed_ips = Arc::new(new_set);
            tracing::debug!(
                peer_key = %public_key,
                cidr = %cidr,
                "Removed allowed_ip from Windows overlay peer map"
            );
            Ok(())
        }
    }

    /// Query interface status.
    ///
    /// On Linux/macOS this reads boringtun's UAPI socket. On Windows it
    /// returns an error until the packet loop lands (since there is no
    /// running `WireGuard` stack to query yet).
    ///
    /// # Errors
    ///
    /// Returns an error if the UAPI query fails on Linux/macOS, or
    /// always on Windows.
    #[cfg_attr(windows, allow(clippy::unused_async))]
    pub async fn status(&self) -> Result<String, Box<dyn std::error::Error>> {
        #[cfg(not(windows))]
        {
            let sock = self.uapi_sock_path();
            let response = uapi_get(&sock).await?;
            Ok(response)
        }
        #[cfg(windows)]
        {
            // Mimic the `wg show`-style key=value newline-delimited
            // dump that the Linux/macOS UAPI surface produces.
            use base64::{engine::general_purpose::STANDARD, Engine as _};
            let mut out = String::new();
            let priv_bytes = decode_key_b64(&self.config.private_key).unwrap_or([0u8; 32]);
            let _ = writeln!(out, "private_key={}", hex::encode(priv_bytes));
            let _ = writeln!(out, "listen_port={}", self.config.local_endpoint.port());
            for entry in self.peers.iter() {
                let pk_b64 = STANDARD.encode(entry.key());
                let _ = writeln!(out, "public_key={}", hex::encode(entry.key()));
                let _ = writeln!(out, "public_key_b64={pk_b64}");
                if let Some(ep) = *entry.value().endpoint.read() {
                    let _ = writeln!(out, "endpoint={ep}");
                }
                for net in entry.value().allowed_ips.iter() {
                    let _ = writeln!(out, "allowed_ip={net}");
                }
                if let Some(k) = entry.value().persistent_keepalive {
                    let _ = writeln!(out, "persistent_keepalive_interval={k}");
                }
                let last = entry.value().last_handshake_sec.load(Ordering::Relaxed);
                let _ = writeln!(out, "last_handshake_time_sec={last}");
            }
            let _ = writeln!(out, "errno=0");
            Ok(out)
        }
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

    /// Update a peer's endpoint address via UAPI.
    ///
    /// Used by NAT traversal to switch endpoints after discovery (e.g. from a
    /// relay to a direct reflexive address after hole punching succeeds).
    ///
    /// # Errors
    ///
    /// Returns an error if key conversion or UAPI command fails.
    #[cfg(feature = "nat")]
    #[cfg_attr(windows, allow(clippy::unused_async))]
    pub async fn update_peer_endpoint(
        &self,
        public_key: &str,
        new_endpoint: std::net::SocketAddr,
    ) -> Result<(), Box<dyn std::error::Error>> {
        #[cfg(not(windows))]
        {
            let sock = self.uapi_sock_path();
            let pub_hex = key_to_hex(public_key)?;
            let body = format!("public_key={pub_hex}\nendpoint={new_endpoint}\n");
            uapi_set(&sock, &body).await?;
            tracing::debug!(
                peer_key = %public_key,
                endpoint = %new_endpoint,
                "Updated peer endpoint"
            );
            Ok(())
        }
        #[cfg(windows)]
        {
            let pk = decode_key_b64(public_key)?;
            let entry = self
                .peers
                .get(&pk)
                .ok_or_else(|| format!("peer not found: {public_key}"))?;
            *entry.value().endpoint.write() = Some(new_endpoint);
            tracing::debug!(
                peer_key = %public_key,
                endpoint = %new_endpoint,
                "Updated peer endpoint (Windows)"
            );
            Ok(())
        }
    }

    /// Check if a peer has completed a `WireGuard` handshake since a given timestamp.
    ///
    /// Returns `true` if `last_handshake_time_sec >= since` (and is non-zero).
    /// Used by NAT traversal to verify connectivity after switching endpoints.
    ///
    /// # Errors
    ///
    /// Returns an error if the UAPI query fails.
    #[cfg(feature = "nat")]
    #[cfg_attr(windows, allow(clippy::unused_async))]
    pub async fn check_peer_handshake(
        &self,
        public_key: &str,
        since: u64,
    ) -> Result<bool, Box<dyn std::error::Error>> {
        #[cfg(not(windows))]
        {
            let sock = self.uapi_sock_path();
            let response = uapi_get(&sock).await?;
            let target_hex = key_to_hex(public_key)?;

            let mut in_target = false;
            for line in response.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with("errno=") {
                    continue;
                }
                let Some((key, value)) = line.split_once('=') else {
                    continue;
                };
                match key {
                    "public_key" => {
                        in_target = value == target_hex;
                    }
                    "last_handshake_time_sec" if in_target => {
                        if let Ok(t) = value.parse::<u64>() {
                            return Ok(t > 0 && t >= since);
                        }
                    }
                    _ => {}
                }
            }
            Ok(false)
        }
        #[cfg(windows)]
        {
            let pk = decode_key_b64(public_key)?;
            let entry = self
                .peers
                .get(&pk)
                .ok_or_else(|| format!("peer not found: {public_key}"))?;
            let last = entry.value().last_handshake_sec.load(Ordering::Relaxed);
            Ok(last > 0 && last >= since)
        }
    }

    /// Shut down the overlay transport, destroying the TUN device.
    ///
    /// On Linux/macOS this takes the boringtun [`DeviceHandle`] and
    /// drops it, which triggers boringtun's cleanup logic (signal exit +
    /// join worker threads + remove socket).
    ///
    /// On Windows it takes the Wintun adapter handle and drops it, which
    /// ends the session and removes the adapter from the Windows device
    /// tree.
    pub fn shutdown(&mut self) {
        #[cfg(not(windows))]
        if let Some(device) = self.device.take() {
            tracing::info!(
                interface = %self.interface_name,
                "Shutting down overlay transport"
            );
            // DeviceHandle::drop triggers exit + cleanup
            drop(device);
        }
        #[cfg(windows)]
        {
            if let Some(h) = self.ingress_task.take() {
                h.abort();
            }
            if let Some(h) = self.egress_task.take() {
                h.abort();
            }
            if let Some(h) = self.timers_task.take() {
                h.abort();
            }
            // Drop the UDP socket — any in-flight recv on the ingress
            // task will already have been aborted above, but the Arc
            // count must drop to zero for the kernel handle to close.
            self.udp.take();
            self.peers.clear();
            if let Some(dev) = self.wintun_dev.take() {
                tracing::info!(
                    interface = %self.interface_name,
                    "Shutting down Wintun overlay transport"
                );
                drop(dev);
            }
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
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
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

    // -----------------------------------------------------------------
    // Windows-only helper tests
    // -----------------------------------------------------------------

    #[cfg(windows)]
    #[test]
    fn test_parse_dst_ip_v4() {
        // Minimal IPv4 header: version=4 (top nibble), header length=5,
        // dst IP = 10.0.0.7 in bytes 16..20.
        let mut pkt = vec![0u8; 20];
        pkt[0] = 0x45;
        pkt[16..20].copy_from_slice(&[10, 0, 0, 7]);
        assert_eq!(
            super::parse_dst_ip(&pkt),
            Some(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 7)))
        );
    }

    #[cfg(windows)]
    #[test]
    fn test_parse_dst_ip_v6() {
        // IPv6: version=6 (top nibble), dst IP = fd00::1 in bytes 24..40.
        let mut pkt = vec![0u8; 40];
        pkt[0] = 0x60;
        pkt[24] = 0xfd;
        pkt[25] = 0x00;
        pkt[39] = 0x01;
        let expected = IpAddr::V6(Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 1));
        assert_eq!(super::parse_dst_ip(&pkt), Some(expected));
    }

    #[cfg(windows)]
    #[test]
    fn test_parse_dst_ip_truncated_returns_none() {
        let pkt = vec![0x45u8; 10];
        assert_eq!(super::parse_dst_ip(&pkt), None);
        assert_eq!(super::parse_dst_ip(&[]), None);
    }

    #[cfg(windows)]
    #[test]
    fn test_parse_dst_ip_unknown_version_returns_none() {
        let pkt = vec![0x70u8; 64];
        assert_eq!(super::parse_dst_ip(&pkt), None);
    }

    #[cfg(windows)]
    #[test]
    fn test_decode_key_b64_roundtrip() {
        use base64::{engine::general_purpose::STANDARD, Engine as _};
        let raw = [0x42u8; 32];
        let b64 = STANDARD.encode(raw);
        let decoded = super::decode_key_b64(&b64).expect("decode");
        assert_eq!(decoded, raw);
    }

    #[cfg(windows)]
    #[test]
    fn test_decode_key_b64_wrong_length_errors() {
        use base64::{engine::general_purpose::STANDARD, Engine as _};
        let short = STANDARD.encode([0u8; 16]);
        assert!(super::decode_key_b64(&short).is_err());
    }

    #[test]
    fn test_peer_info_ipv6_to_config() {
        let peer = PeerInfo::new(
            "test_public_key_v6".to_string(),
            SocketAddr::new(
                IpAddr::V6(Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 1)),
                51820,
            ),
            "fd00::2/128",
            Duration::from_secs(25),
        );

        let config = peer.to_peer_config();
        assert!(config.contains("PublicKey = test_public_key_v6"));
        // SocketAddr for IPv6 uses bracket notation: [fd00::1]:51820
        assert!(
            config.contains("Endpoint = [fd00::1]:51820"),
            "IPv6 endpoint should use bracket notation, got: {config}"
        );
        assert!(config.contains("AllowedIPs = fd00::2/128"));
    }

    #[test]
    fn test_overlay_cidr_parses_ipv4() {
        let cidr: ipnet::IpNet = "10.200.0.1/24".parse().unwrap();
        assert!(cidr.addr().is_ipv4());
        assert_eq!(cidr.prefix_len(), 24);
        assert_eq!(cidr.network().to_string(), "10.200.0.0");
    }

    #[test]
    fn test_overlay_cidr_parses_ipv6() {
        let cidr: ipnet::IpNet = "fd00::1/48".parse().unwrap();
        assert!(cidr.addr().is_ipv6());
        assert_eq!(cidr.prefix_len(), 48);
        assert_eq!(cidr.network().to_string(), "fd00::");
    }

    #[test]
    fn test_overlay_cidr_ipv6_host_address() {
        // Verify /128 single-host prefix works (used in allowed_ips)
        let cidr: ipnet::IpNet = "fd00::5/128".parse().unwrap();
        assert!(cidr.addr().is_ipv6());
        assert_eq!(cidr.prefix_len(), 128);
        assert_eq!(cidr.addr().to_string(), "fd00::5");
    }

    #[test]
    fn test_peer_info_ipv6_allowed_ips_format() {
        // PeerInfo.allowed_ips is a String — verify both formats are valid
        let peer_v4 = PeerInfo::new(
            "key_v4".to_string(),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 51820),
            "10.200.0.5/32",
            Duration::from_secs(25),
        );
        assert_eq!(peer_v4.allowed_ips, "10.200.0.5/32");

        let peer_v6 = PeerInfo::new(
            "key_v6".to_string(),
            SocketAddr::new(
                IpAddr::V6(Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 5)),
                51820,
            ),
            "fd00::5/128",
            Duration::from_secs(25),
        );
        assert_eq!(peer_v6.allowed_ips, "fd00::5/128");
    }

    #[test]
    fn test_uapi_body_format_ipv6_peer() {
        // Verify that formatting an IPv6 SocketAddr for UAPI produces correct output.
        // WireGuard UAPI expects [ipv6]:port format for endpoints.
        let endpoint = SocketAddr::new(
            IpAddr::V6(Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 1)),
            51820,
        );
        let formatted = format!("endpoint={endpoint}");
        assert_eq!(formatted, "endpoint=[fd00::1]:51820");
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

    #[cfg(not(windows))]
    #[test]
    fn test_parse_allowed_ips_for_peer_basic() {
        let resp = "\
private_key=aaaa\n\
listen_port=51820\n\
public_key=1111\n\
endpoint=127.0.0.1:51821\n\
allowed_ip=10.0.0.0/24\n\
allowed_ip=10.1.0.0/24\n\
public_key=2222\n\
allowed_ip=10.2.0.0/24\n\
errno=0\n";
        let out = super::parse_allowed_ips_for_peer(resp, "1111");
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].to_string(), "10.0.0.0/24");
        assert_eq!(out[1].to_string(), "10.1.0.0/24");
        let out2 = super::parse_allowed_ips_for_peer(resp, "2222");
        assert_eq!(out2.len(), 1);
        assert_eq!(out2[0].to_string(), "10.2.0.0/24");
    }

    #[cfg(not(windows))]
    #[test]
    fn test_parse_allowed_ips_for_peer_unknown_peer_returns_empty() {
        let resp = "\
public_key=1111\n\
allowed_ip=10.0.0.0/24\n\
errno=0\n";
        assert!(super::parse_allowed_ips_for_peer(resp, "ffff").is_empty());
    }

    #[cfg(not(windows))]
    #[test]
    fn test_peer_exists_in_uapi() {
        let resp = "\
public_key=1111\n\
allowed_ip=10.0.0.0/24\n\
public_key=2222\n\
errno=0\n";
        assert!(super::peer_exists_in_uapi(resp, "1111"));
        assert!(super::peer_exists_in_uapi(resp, "2222"));
        assert!(!super::peer_exists_in_uapi(resp, "3333"));
    }

    #[cfg(not(windows))]
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

    #[cfg(not(windows))]
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
            cluster_cidr: None,
            private_key: "test_key".to_string(),
            public_key: "test_pub".to_string(),
            local_endpoint: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 51820),
            peer_discovery_interval: Duration::from_secs(30),
            #[cfg(feature = "nat")]
            nat: crate::nat::NatConfig::default(),
            uapi_sock_dir: std::path::PathBuf::from("/var/run/wireguard"),
            mtu: 1420,
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

    #[tokio::test]
    #[ignore = "Requires root/CAP_NET_ADMIN"]
    async fn test_create_interface_boringtun_ipv6() {
        let config = OverlayConfig {
            overlay_cidr: "fd00::1/48".to_string(),
            cluster_cidr: None,
            private_key: "test_key".to_string(),
            public_key: "test_pub".to_string(),
            local_endpoint: SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 51820),
            peer_discovery_interval: Duration::from_secs(30),
            #[cfg(feature = "nat")]
            nat: crate::nat::NatConfig::default(),
            uapi_sock_dir: std::path::PathBuf::from("/var/run/wireguard"),
            mtu: 1420,
        };

        #[cfg(target_os = "macos")]
        let iface_name = "utun".to_string();
        #[cfg(not(target_os = "macos"))]
        let iface_name = "zl-bt6-test0".to_string();

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
