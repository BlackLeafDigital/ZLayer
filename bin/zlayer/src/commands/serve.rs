use std::sync::Arc;

use anyhow::{Context, Result};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::daemon::{init_daemon, restore_deployments, DaemonConfig};
use zlayer_api::handlers::build::build_routes;
use zlayer_api::handlers::cluster::ClusterApiState;
use zlayer_api::handlers::nodes::NodeApiState;
use zlayer_api::handlers::overlay::OverlayApiState;
use zlayer_api::handlers::{reconcile_standalone_containers, start_auto_remove_subscriber};
use zlayer_api::router::{
    build_cluster_routes, build_container_network_routes, build_container_routes,
    build_cron_routes, build_event_routes, build_job_routes, build_network_routes,
    build_node_routes, build_overlay_routes, build_tunnel_routes, build_volume_routes,
};
use zlayer_api::{
    ApiConfig, BridgeNetworkApiState, BuildState, ContainerApiState, CronState, JobState,
    NetworkApiState, VolumeApiState,
};
#[cfg(feature = "nat")]
use zlayer_overlay::nat::{NatConfig, RelayServerConfig, StunServerConfig, TurnServerConfig};
use zlayer_overlay::IpAllocator;

/// Daemon metadata written to `{data_dir}/daemon.json`.
#[derive(Serialize)]
struct DaemonMetadata {
    pid: u32,
    started_at: String,
    api_bind: String,
    socket_path: String,
    host_network: bool,
    overlay_cidr: String,
}

/// Minimal struct for reading back the PID and bind address from a previous daemon's metadata.
#[derive(Deserialize)]
struct StaleDaemonMeta {
    pid: u32,
    /// The API bind address (e.g. "0.0.0.0:3669") so we can verify the port is
    /// free after killing the old process.
    #[serde(default)]
    api_bind: Option<String>,
}

/// Read a `daemon.json` file, returning the PID + bind it advertised.
///
/// Used by both the stale-self cleanup and the foreign-daemon ownership
/// check. Returns `None` if the file doesn't exist or is malformed — both
/// treated as "no claim". Synchronous on purpose: the foreign-daemon probe
/// runs before we enter async land for the boot sweep, and the file is
/// small enough that the blocking read is irrelevant in practice.
///
/// Only compiled on unix (the sole non-test caller, `foreign_daemon_alive`,
/// is unix-only) or under `cfg(test)` so the unit tests still link on
/// Windows. Without this gate, Windows non-test builds flag it as dead.
#[cfg(any(unix, test))]
fn read_daemon_metadata(path: &std::path::Path) -> Option<StaleDaemonMeta> {
    let contents = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&contents).ok()
}

/// Probe the standard daemon data-dir locations OTHER than the current
/// one for a `daemon.json` with a live PID. Returns `true` when a foreign
/// zlayer daemon appears to be running on this host.
///
/// The sweep that follows must skip its destructive deletes whenever this
/// returns `true`: even with a prefix-scoped match (`zl-<name>-`), two
/// daemons could pick the same default `deployment_name` ("zlayer") and
/// end up clobbering each other's overlay state. Hostility is worse than
/// letting a stale link survive — a stale link is repaired on the next
/// clean boot, a corrupted live link tears down running containers.
#[cfg(unix)]
#[allow(unsafe_code, clippy::cast_possible_wrap)]
fn foreign_daemon_alive(self_data_dir: &std::path::Path) -> Option<u32> {
    let mut probe_dirs: Vec<std::path::PathBuf> = vec![std::path::PathBuf::from("/var/lib/zlayer")];
    if let Some(home) = std::env::var_os("HOME") {
        probe_dirs.push(std::path::PathBuf::from(home).join(".zlayer"));
    }
    for probe in probe_dirs {
        if probe == self_data_dir {
            continue;
        }
        let Some(meta) = read_daemon_metadata(&probe.join("daemon.json")) else {
            continue;
        };
        // SAFETY: signal 0 is a null signal used purely for existence checking.
        if unsafe { libc::kill(meta.pid as i32, 0) } == 0 {
            return Some(meta.pid);
        }
    }
    None
}

/// Clean up a stale daemon process and leftover network state from a previous run.
///
/// This is best-effort: all errors are logged as warnings but never prevent startup.
#[allow(
    unsafe_code,
    clippy::too_many_lines,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss
)]
async fn cleanup_stale_daemon(
    config: &DaemonConfig,
    socket_path: &str,
    api_bind: &str,
    wg_port_override: Option<u16>,
) {
    let metadata_path = config.data_dir.join("daemon.json");
    #[cfg(unix)]
    let my_pid = std::process::id();

    // Track whether the PID-based kill was conclusive (i.e. daemon.json existed
    // and the named PID was either dead or successfully killed).
    // Only mutated inside the Unix branch below; the Windows branch never
    // reaches `libc::kill`, so the `mut` is Unix-only.
    #[cfg(unix)]
    let mut pid_kill_conclusive = false;
    #[cfg(not(unix))]
    let pid_kill_conclusive = false;

    // The port we need to verify is free before returning.  Default to the
    // port extracted from the *current* bind address; override with the port
    // from the old daemon.json if available.
    let mut api_port: u16 = parse_port_from_bind(api_bind).unwrap_or(3669);

    // -----------------------------------------------------------------------
    // 1. Read the old daemon PID and terminate it if still alive
    //
    // Process termination is Unix-only: the stale-daemon kill path uses
    // `libc::kill()` with SIGTERM/SIGKILL. On Windows we still read the old
    // api_bind to pick the right port for the port-free check below, but
    // skip the kill — subsequent bind attempts will surface
    // "address in use" errors if a stale daemon is still running.
    // -----------------------------------------------------------------------
    if let Ok(contents) = tokio::fs::read_to_string(&metadata_path).await {
        match serde_json::from_str::<StaleDaemonMeta>(&contents) {
            Ok(meta) => {
                // If the old daemon wrote an api_bind, use its port for the
                // port-free check so we wait on the right port even when the
                // operator changed the bind address between runs.
                if let Some(ref old_bind) = meta.api_bind {
                    if let Some(port) = parse_port_from_bind(old_bind) {
                        api_port = port;
                    }
                }

                #[cfg(unix)]
                {
                    let old_pid = meta.pid as i32;
                    // Do not kill ourselves (pid file left over from a clean restart).
                    if old_pid as u32 == my_pid {
                        info!(
                            pid = old_pid,
                            "Stale daemon PID matches current process, skipping kill"
                        );
                        pid_kill_conclusive = true;
                    } else if process_alive(old_pid) {
                        warn!(
                            pid = old_pid,
                            "Stale daemon process detected, sending SIGTERM"
                        );
                        // SAFETY: we validated the PID is alive and is not us.
                        unsafe { libc::kill(old_pid, libc::SIGTERM) };

                        // Poll for up to 5 seconds waiting for graceful exit.
                        let mut terminated = false;
                        for _ in 0..50 {
                            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                            if !process_alive(old_pid) {
                                terminated = true;
                                break;
                            }
                        }

                        if terminated {
                            info!(pid = old_pid, "Stale daemon exited after SIGTERM");
                            pid_kill_conclusive = true;
                        } else {
                            warn!(
                                pid = old_pid,
                                "Stale daemon did not exit in time, sending SIGKILL"
                            );
                            unsafe { libc::kill(old_pid, libc::SIGKILL) };
                            // Brief wait for the kernel to reap.
                            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                            if process_alive(old_pid) {
                                warn!(pid = old_pid, "Stale daemon still alive after SIGKILL");
                            } else {
                                info!(pid = old_pid, "Stale daemon killed with SIGKILL");
                                pid_kill_conclusive = true;
                            }
                        }
                    } else {
                        info!(
                            pid = old_pid,
                            "Previous daemon is not running, cleaning up stale files"
                        );
                        pid_kill_conclusive = true;
                    }
                }
                #[cfg(not(unix))]
                {
                    // No process-kill on Windows in this phase. If the old
                    // daemon is still running, the TCP bind below will fail
                    // with a clear "address already in use" error.
                    info!(
                        pid = meta.pid,
                        "Found stale daemon.json on Windows; skipping process kill \
                         (not yet implemented). TCP bind will fail if the old daemon \
                         is still running."
                    );
                }
            }
            Err(e) => {
                warn!(
                    path = %metadata_path.display(),
                    error = %e,
                    "Failed to parse stale daemon.json, ignoring"
                );
            }
        }
    }

    // -----------------------------------------------------------------------
    // 1b. No pgrep fallback — if daemon.json was missing, we simply proceed.
    //     The old daemon is either gone or will be replaced when we bind the
    //     port/socket. Never kill arbitrary zlayer CLI processes.
    // -----------------------------------------------------------------------
    if !pid_kill_conclusive {
        info!("No daemon.json found, proceeding without killing any processes");
    }

    // -----------------------------------------------------------------------
    // 1c. Remove stale unix socket (Unix only; Windows uses TCP loopback).
    // -----------------------------------------------------------------------
    #[cfg(unix)]
    {
        let socket = std::path::Path::new(socket_path);
        if socket.exists() {
            match tokio::fs::remove_file(socket).await {
                Ok(()) => info!(path = %socket_path, "Removed stale unix socket"),
                Err(e) => {
                    warn!(path = %socket_path, error = %e, "Failed to remove stale unix socket");
                }
            }
        }
    }
    #[cfg(not(unix))]
    {
        // `socket_path` is a TCP URI on Windows (e.g. "tcp://127.0.0.1:3669"),
        // not a filesystem path — nothing to unlink.
        let _ = socket_path;
    }

    // -----------------------------------------------------------------------
    // 2. Clean up stale network interfaces
    //
    // Sweep is scoped to THIS daemon's deployment-name prefix
    // (`zl-<deployment_name>-`) so a second daemon never deletes another
    // daemon's overlay links. Even with that scoping, if any foreign
    // daemon (different data_dir, live PID per its daemon.json) is alive,
    // we skip the sweep entirely: two daemons can share the default
    // "zlayer" deployment name, in which case the prefix match would still
    // be hostile. Stale links are preferable to torn-down live links.
    // -----------------------------------------------------------------------
    let zl_prefix = format!("zl-{}-", config.deployment_name);

    #[cfg(unix)]
    let foreign_pid = foreign_daemon_alive(&config.data_dir);
    #[cfg(not(unix))]
    let foreign_pid: Option<u32> = None;

    if let Some(pid) = foreign_pid {
        warn!(
            pid = pid,
            deployment_name = %config.deployment_name,
            "Foreign zlayer daemon detected; skipping link/socket sweep entirely"
        );
    }

    #[cfg(target_os = "linux")]
    if foreign_pid.is_none() {
        // Linux: use `ip` to find and delete stale veth-* and
        // zl-<deployment_name>-* interfaces.
        if let Ok(output) = tokio::process::Command::new("ip")
            .args(["-br", "link"])
            .output()
            .await
        {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                for line in stdout.lines() {
                    // `ip -br link` format: "NAME  STATE  ..."
                    let Some(iface) = line.split_whitespace().next() else {
                        continue;
                    };

                    if iface.starts_with("veth-") || iface.starts_with(&zl_prefix) {
                        warn!(interface = %iface, "Deleting stale network interface");
                        let _ = tokio::process::Command::new("ip")
                            .args(["link", "delete", iface])
                            .output()
                            .await;
                    }
                }
            }
        } else {
            warn!("Failed to list network interfaces for stale cleanup");
        }
    }
    #[cfg(target_os = "macos")]
    if foreign_pid.is_none() {
        // macOS: use `ifconfig` to find stale utun devices that have a matching
        // WireGuard UAPI socket whose stem matches this daemon's deployment
        // prefix, indicating they belong to a previous zlayer run with the
        // same deployment name. The sock dir is data-dir-aware so a daemon
        // booting against `--data-dir /tmp/foo` only sweeps its own sockets.
        let wg_sock_dir = zlayer_paths::ZLayerDirs::new(&config.data_dir).wireguard();
        let wg_sock_dir = wg_sock_dir.as_path();
        if let Ok(output) = tokio::process::Command::new("ifconfig")
            .args(["-l"])
            .output()
            .await
        {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                for iface in stdout.split_whitespace() {
                    // Only iterate utun devices that have an associated sock
                    // file whose stem belongs to this deployment.
                    if !iface.starts_with("utun") {
                        continue;
                    }
                    let sock_path = wg_sock_dir.join(format!("{iface}.sock"));
                    if !sock_path.exists() {
                        continue;
                    }
                    let Some(stem) = sock_path.file_stem().and_then(|s| s.to_str()) else {
                        continue;
                    };
                    if !stem.starts_with(&zl_prefix) {
                        continue;
                    }
                    warn!(interface = %iface, "Destroying stale utun interface");
                    let _ = tokio::process::Command::new("ifconfig")
                        .args([iface, "destroy"])
                        .output()
                        .await;
                    let _ = tokio::fs::remove_file(&sock_path).await;
                }
            }
        } else {
            warn!("Failed to list network interfaces for stale cleanup");
        }
    }

    // -----------------------------------------------------------------------
    // 3. Remove stale WireGuard UAPI sockets
    //
    // Scoped to this daemon's `zl-<deployment_name>-*.sock` and skipped
    // entirely when a foreign daemon is alive (see block 2 above). The
    // sock dir is data-dir-aware (`ZLayerDirs::wireguard()`) so a daemon
    // running with a non-default `--data-dir` only sweeps its own
    // sockets and leaves the host-global `/var/run/wireguard` alone.
    // -----------------------------------------------------------------------
    if foreign_pid.is_none() {
        let wg_sock_dir = zlayer_paths::ZLayerDirs::new(&config.data_dir).wireguard();
        if wg_sock_dir.is_dir() {
            match tokio::fs::read_dir(&wg_sock_dir).await {
                Ok(mut entries) => {
                    while let Ok(Some(entry)) = entries.next_entry().await {
                        let path = entry.path();
                        if path.extension().and_then(|e| e.to_str()) != Some("sock") {
                            continue;
                        }
                        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                            continue;
                        };
                        if !stem.starts_with(&zl_prefix) {
                            continue;
                        }
                        warn!(path = %path.display(), "Removing stale WireGuard socket");
                        let _ = tokio::fs::remove_file(&path).await;
                    }
                }
                Err(e) => {
                    warn!(
                        dir = %wg_sock_dir.display(),
                        error = %e,
                        "Failed to read WireGuard sock dir for stale socket cleanup"
                    );
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // 4. Wait for API port to be free
    // -----------------------------------------------------------------------
    {
        let bind_addr: std::net::SocketAddr = ([0, 0, 0, 0], api_port).into();
        let mut port_free = false;
        for attempt in 1..=10 {
            if let Ok(_listener) = std::net::TcpListener::bind(bind_addr) {
                // Listener is dropped immediately, port is free.
                if attempt > 1 {
                    info!(port = api_port, attempts = attempt, "API port is now free");
                }
                port_free = true;
                break;
            }
            if attempt == 1 {
                warn!(
                    port = api_port,
                    "API port still in use, waiting for it to be released"
                );
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        if !port_free {
            warn!(
                port = api_port,
                "API port still in use after 1s — startup may fail with 'Address already in use'"
            );
        }
    }

    // -----------------------------------------------------------------------
    // 4b. Wait for WireGuard UDP port to be free (DEFAULT_WG_PORT in bootstrap.rs)
    //
    // This block kills stale boringtun holders on Unix via libc::kill.
    // On Windows we only do a passive poll — killing arbitrary processes
    // that happen to hold the WG UDP port is F-7b territory.
    // -----------------------------------------------------------------------
    #[cfg(unix)]
    {
        // Precedence: CLI flag > node_config.json > zlayer_core::DEFAULT_WG_PORT.
        // `load_overlay_port` reads `node_config.json#overlay_port`, falling
        // back to the constant default; the CLI flag wins above both.
        let wg_port = wg_port_override.unwrap_or_else(|| load_overlay_port(&config.data_dir));
        let wg_addr: std::net::SocketAddr = ([0, 0, 0, 0], wg_port).into();

        if std::net::UdpSocket::bind(wg_addr).is_err() {
            // Port is occupied — try to identify and clean up the holder.
            match find_udp_port_holder(wg_port).await {
                Some((pid, ref name))
                    if (name.contains("zlayer") || name.contains("boringtun")) && pid != my_pid =>
                {
                    warn!(
                        port = wg_port,
                        pid = pid,
                        process = %name,
                        "Stale zlayer/boringtun process holding WireGuard port, sending SIGTERM"
                    );
                    // SAFETY: pid is a valid, live process that is not us.
                    unsafe { libc::kill(pid as i32, libc::SIGTERM) };

                    // Wait up to 3s for graceful exit (30 x 100ms).
                    let mut freed = false;
                    for _ in 0..30 {
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                        if std::net::UdpSocket::bind(wg_addr).is_ok() {
                            freed = true;
                            break;
                        }
                    }

                    if freed {
                        info!(
                            port = wg_port,
                            pid = pid,
                            "WireGuard UDP port freed after SIGTERM"
                        );
                    } else {
                        warn!(
                            port = wg_port,
                            pid = pid,
                            "Port still held after SIGTERM, sending SIGKILL"
                        );
                        unsafe { libc::kill(pid as i32, libc::SIGKILL) };
                    }
                }
                Some((pid, ref name)) => {
                    warn!(
                        port = wg_port,
                        pid = pid,
                        process = %name,
                        "WireGuard UDP port held by non-zlayer process, will not kill"
                    );
                }
                None => {
                    warn!(
                        port = wg_port,
                        "WireGuard UDP port in use but holder could not be identified"
                    );
                }
            }

            // Safety-net passive poll: 5 attempts at 100ms (500ms total).
            let mut wg_free = false;
            for attempt in 1..=5 {
                if std::net::UdpSocket::bind(wg_addr).is_ok() {
                    if attempt > 1 {
                        info!(
                            port = wg_port,
                            attempts = attempt,
                            "WireGuard UDP port is now free"
                        );
                    }
                    wg_free = true;
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            if !wg_free {
                warn!(
                    port = wg_port,
                    "WireGuard UDP port still in use after cleanup — overlay may fail"
                );
            }
        }
    }
    #[cfg(not(unix))]
    {
        // Precedence: CLI flag > node_config.json > zlayer_core::DEFAULT_WG_PORT.
        // `load_overlay_port` reads `node_config.json#overlay_port`, falling
        // back to the constant default; the CLI flag wins above both.
        let wg_port = wg_port_override.unwrap_or_else(|| load_overlay_port(&config.data_dir));
        let wg_addr: std::net::SocketAddr = ([0, 0, 0, 0], wg_port).into();
        if std::net::UdpSocket::bind(wg_addr).is_err() {
            // Passive wait — do not kill arbitrary processes on Windows in F-7a.
            let mut wg_free = false;
            for attempt in 1..=5 {
                if std::net::UdpSocket::bind(wg_addr).is_ok() {
                    if attempt > 1 {
                        info!(
                            port = wg_port,
                            attempts = attempt,
                            "WireGuard UDP port is now free"
                        );
                    }
                    wg_free = true;
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            if !wg_free {
                warn!(
                    port = wg_port,
                    "WireGuard UDP port still in use after passive wait — overlay may fail"
                );
            }
        }
    }

    // -----------------------------------------------------------------------
    // 5. Remove stale daemon metadata and PID files
    // -----------------------------------------------------------------------
    if metadata_path.exists() {
        let _ = tokio::fs::remove_file(&metadata_path).await;
        info!(path = %metadata_path.display(), "Removed stale daemon.json");
    }

    let pid_file = config.run_dir.join("zlayer.pid");
    if pid_file.exists() {
        let _ = tokio::fs::remove_file(&pid_file).await;
        info!(path = %pid_file.display(), "Removed stale zlayer.pid");
    }
}

/// Check whether a process with the given PID is still alive.
///
/// Uses `kill(pid, 0)` which checks for existence without sending a signal.
/// Unix-only: Windows process lifecycle management will arrive with the
/// `DaemonClient` work (Phase F-7b).
#[cfg(unix)]
#[allow(unsafe_code)]
fn process_alive(pid: i32) -> bool {
    // SAFETY: signal 0 is a null signal used purely for existence checking.
    unsafe { libc::kill(pid, 0) == 0 }
}

/// Extract the port number from a bind address string like `"0.0.0.0:3669"`.
fn parse_port_from_bind(bind: &str) -> Option<u16> {
    bind.rsplit_once(':')
        .and_then(|(_, port_str)| port_str.parse::<u16>().ok())
}

/// Build the `ZLAYER_API_URL` injected into container env from the API
/// bind address. Bind addresses can be wildcard (`0.0.0.0`, `::`) which are
/// valid for listening but not for dialing — substitute loopback in that case.
fn container_reachable_api_url(addr: &std::net::SocketAddr) -> String {
    use std::net::IpAddr;
    let port = addr.port();
    match addr.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => format!("http://127.0.0.1:{port}"),
        IpAddr::V6(ip) if ip.is_unspecified() => format!("http://[::1]:{port}"),
        IpAddr::V4(ip) => format!("http://{ip}:{port}"),
        IpAddr::V6(ip) => format!("http://[{ip}]:{port}"),
    }
}

/// Read the overlay port from `{data_dir}/node_config.json`, falling back to
/// [`zlayer_core::DEFAULT_WG_PORT`] on any error.
fn load_overlay_port(data_dir: &std::path::Path) -> u16 {
    let config_path = data_dir.join("node_config.json");
    let Ok(contents) = std::fs::read_to_string(&config_path) else {
        return zlayer_core::DEFAULT_WG_PORT;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&contents) else {
        return zlayer_core::DEFAULT_WG_PORT;
    };
    value
        .get("overlay_port")
        .and_then(serde_json::Value::as_u64)
        .and_then(|p| u16::try_from(p).ok())
        .unwrap_or(zlayer_core::DEFAULT_WG_PORT)
}

/// Attempt to find the process holding a UDP port.
///
/// Returns `Some((pid, process_name))` if a holder is found, `None` if the port
/// appears free or the lookup fails.
#[cfg(target_os = "linux")]
async fn find_udp_port_holder(port: u16) -> Option<(u32, String)> {
    // `ss -ulnp sport = :PORT` prints lines like:
    //   UNCONN 0 0 0.0.0.0:51420 ... users:(("zlayer",pid=12345,fd=7))
    let output = tokio::process::Command::new("ss")
        .args(["-ulnp", &format!("sport = :{port}")])
        .output()
        .await
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    // Look for pid=(\d+) in the output.
    let pid_str = stdout
        .split("pid=")
        .nth(1)?
        .split(|c: char| !c.is_ascii_digit())
        .next()?;

    let pid: u32 = pid_str.parse().ok()?;

    // Resolve the process name via `ps`.
    let ps_out = tokio::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "comm="])
        .output()
        .await
        .ok()?;

    let name = String::from_utf8_lossy(&ps_out.stdout).trim().to_string();
    Some((pid, name))
}

/// Attempt to find the process holding a UDP port.
///
/// Returns `Some((pid, process_name))` if a holder is found, `None` if the port
/// appears free or the lookup fails.
#[cfg(target_os = "macos")]
async fn find_udp_port_holder(port: u16) -> Option<(u32, String)> {
    let output = tokio::process::Command::new("lsof")
        .args([&format!("-i UDP:{port}"), "-t"])
        .output()
        .await
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let pid: u32 = stdout.trim().lines().next()?.parse().ok()?;

    let ps_out = tokio::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "comm="])
        .output()
        .await
        .ok()?;

    let name = String::from_utf8_lossy(&ps_out.stdout).trim().to_string();
    Some((pid, name))
}

/// Start the daemon API server with full infrastructure.
///
/// Foreground variant — uses Ctrl+C / SIGTERM for shutdown. For the
/// Windows Service variant that also honours SCM Stop/Shutdown control
/// codes, see [`serve_with_external_shutdown`].
///
/// On builds without the `nat` feature, this is the entry point used by
/// `Commands::Serve`. With the `nat` feature, [`serve_with_nat_overrides`]
/// is used instead (so the CLI flags actually win over env vars), and this
/// shim is only kept for source compatibility.
#[cfg_attr(feature = "nat", allow(dead_code))]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn serve(
    bind: &str,
    jwt_secret: Option<String>,
    no_swagger: bool,
    socket_path: &str,
    host_network: bool,
    data_dir: std::path::PathBuf,
    deployment_name: String,
    wg_port: Option<u16>,
    dns_port: Option<u16>,
    restart_on_exit: bool,
) -> Result<()> {
    // Stash the `--restart-on-exit` flag in a process-global slot so
    // `serve_with_external_shutdown` (whose public signature is locked by the
    // Windows Service host in `daemon_service.rs`) picks it up after teardown.
    set_pending_restart_on_exit(restart_on_exit);
    // Box::pin keeps the top-level future below clippy's `large_futures`
    // threshold — `serve_with_external_shutdown` materialises the whole
    // daemon state on the stack and its future is sizeable.
    Box::pin(serve_with_external_shutdown(
        bind,
        jwt_secret,
        no_swagger,
        socket_path,
        host_network,
        data_dir,
        deployment_name,
        wg_port,
        dns_port,
        None,
    ))
    .await
}

/// Variant of [`serve`] that also accepts CLI-level NAT overrides so callers
/// (the `Commands::Serve` dispatcher in `main.rs`) can thread `--no-nat` /
/// `--stun-server` / etc. into the daemon config without going through env
/// vars.
#[cfg(feature = "nat")]
#[allow(clippy::too_many_arguments, dead_code)]
pub(crate) async fn serve_with_nat_overrides(
    bind: &str,
    jwt_secret: Option<String>,
    no_swagger: bool,
    socket_path: &str,
    host_network: bool,
    data_dir: std::path::PathBuf,
    deployment_name: String,
    wg_port: Option<u16>,
    dns_port: Option<u16>,
    nat_overrides: NatCliOverrides,
    restart_on_exit: bool,
) -> Result<()> {
    // Stash the overrides in a process-global slot so
    // `serve_with_external_shutdown` (which keeps its public signature for
    // back-compat with `daemon_service.rs`) can pick them up alongside the
    // `ZLAYER_NAT_*` env vars.
    set_pending_nat_overrides(nat_overrides);
    set_pending_restart_on_exit(restart_on_exit);
    Box::pin(serve_with_external_shutdown(
        bind,
        jwt_secret,
        no_swagger,
        socket_path,
        host_network,
        data_dir,
        deployment_name,
        wg_port,
        dns_port,
        None,
    ))
    .await
}

/// CLI overrides for the daemon API listener's TLS configuration.
///
/// Resolved inside [`serve_with_external_shutdown`] and translated into a
/// [`zlayer_api::config::ApiTlsConfig`] that is set on [`ApiConfig::tls`]
/// before the API listener starts. The three fields encode the three valid
/// modes:
///
/// - `cert = Some, key = Some, acme = false` → [`ApiTlsConfig::Static`].
/// - `cert = None, key = None, acme = true`  → [`ApiTlsConfig::Managed`],
///   sharing the proxy's `Arc<CertManager>`.
/// - `cert = None, key = None, acme = false` → no TLS (plain HTTP).
///
/// All other combinations are rejected by clap's `requires` /
/// `conflicts_with` constraints on `Commands::Serve` (see `cli.rs`), so the
/// translation logic in [`build_api_tls_config_from_overrides`] uses
/// `unreachable!` for the unreachable arms.
#[derive(Debug, Clone, Default)]
pub(crate) struct ApiTlsCliOverrides {
    /// `--api-tls-cert <PATH>`: PEM-encoded certificate chain to load at
    /// startup. Mutually exclusive with `acme`. Requires `key`.
    pub cert: Option<std::path::PathBuf>,
    /// `--api-tls-key <PATH>`: PEM-encoded private key paired with `cert`.
    pub key: Option<std::path::PathBuf>,
    /// `--api-tls-acme`: delegate cert resolution to the proxy's
    /// `Arc<CertManager>` instead of loading static files.
    pub acme: bool,
}

/// Process-global slot for the most recently parsed [`ApiTlsCliOverrides`].
///
/// Mirrors the [`PENDING_TUNNEL_OVERRIDES`] / [`PENDING_NAT_OVERRIDES`]
/// pattern: `main.rs` calls [`set_pending_api_tls_overrides`] before
/// dispatching to `serve()`, and [`serve_with_external_shutdown`] calls
/// [`take_pending_api_tls_overrides`] to pick the value back up. Keeping
/// the overrides in a process-global slot avoids extending
/// `serve_with_external_shutdown`'s public signature (which the Windows
/// Service host pins for back-compat).
static PENDING_API_TLS_OVERRIDES: std::sync::Mutex<Option<ApiTlsCliOverrides>> =
    std::sync::Mutex::new(None);

#[allow(dead_code)]
pub(crate) fn set_pending_api_tls_overrides(overrides: ApiTlsCliOverrides) {
    if let Ok(mut slot) = PENDING_API_TLS_OVERRIDES.lock() {
        *slot = Some(overrides);
    }
}

fn take_pending_api_tls_overrides() -> ApiTlsCliOverrides {
    PENDING_API_TLS_OVERRIDES
        .lock()
        .ok()
        .and_then(|mut slot| slot.take())
        .unwrap_or_default()
}

/// Translate parsed `--api-tls-*` CLI overrides into an [`ApiTlsConfig`].
///
/// `cert_manager` is the proxy's `Arc<CertManager>` (always constructed by
/// `init_daemon`), shared with the `Managed` variant when `--api-tls-acme`
/// is set so the daemon API serves the same cert pool the reverse proxy
/// uses for L7 routes.
///
/// Clap's `requires` / `conflicts_with` constraints on `Commands::Serve`
/// guarantee that only three combinations of the three fields are
/// reachable; all others trip the `unreachable!` arm and indicate a
/// regression in the CLI definition rather than user input.
fn build_api_tls_config_from_overrides(
    overrides: &ApiTlsCliOverrides,
    cert_manager: &Arc<zlayer_proxy::CertManager>,
) -> Option<zlayer_api::config::ApiTlsConfig> {
    match (
        overrides.cert.clone(),
        overrides.key.clone(),
        overrides.acme,
    ) {
        (Some(cert_path), Some(key_path), false) => {
            Some(zlayer_api::config::ApiTlsConfig::Static {
                cert_path,
                key_path,
            })
        }
        (None, None, true) => Some(zlayer_api::config::ApiTlsConfig::Managed {
            cert_manager: Arc::clone(cert_manager),
        }),
        (None, None, false) => None,
        // Clap's `requires`/`conflicts_with` make all other combinations
        // unreachable. If we ever hit this, the CLI definition has drifted.
        _ => unreachable!(
            "clap should have rejected this combination of --api-tls-* flags; \
             got cert={:?}, key={:?}, acme={}",
            overrides.cert, overrides.key, overrides.acme,
        ),
    }
}

/// CLI overrides for the daemon-side tunnel server.
///
/// Resolved in `serve()` against environment variables (`ZLAYER_TUNNEL_*`) and
/// the [`crate::daemon::TunnelDaemonConfig::default`] baseline; the result is
/// threaded into the daemon via [`crate::daemon::DaemonConfig::tunnel`].
#[derive(Debug, Clone, Default)]
pub(crate) struct TunnelCliOverrides {
    /// `--no-tunnel-server`: disable the tunnel server entirely.
    pub no_tunnel_server: bool,
    /// `--tunnel-bind <host:port>`. When set, the WebSocket accept loop binds
    /// here; otherwise the [`crate::daemon::TunnelDaemonConfig::default`]
    /// address (`0.0.0.0:3679`) is used.
    pub bind: Option<String>,
    /// `--tunnel-tls-cert <path>`.
    pub tls_cert: Option<std::path::PathBuf>,
    /// `--tunnel-tls-key <path>`.
    pub tls_key: Option<std::path::PathBuf>,
}

static PENDING_TUNNEL_OVERRIDES: std::sync::Mutex<Option<TunnelCliOverrides>> =
    std::sync::Mutex::new(None);

#[allow(dead_code)]
pub(crate) fn set_pending_tunnel_overrides(overrides: TunnelCliOverrides) {
    if let Ok(mut slot) = PENDING_TUNNEL_OVERRIDES.lock() {
        *slot = Some(overrides);
    }
}

fn take_pending_tunnel_overrides() -> TunnelCliOverrides {
    PENDING_TUNNEL_OVERRIDES
        .lock()
        .ok()
        .and_then(|mut slot| slot.take())
        .unwrap_or_default()
}

/// Pending `--restart-on-exit` flag.
///
/// `serve()` / `serve_with_nat_overrides()` stash this for
/// `serve_with_external_shutdown()` to pick up — same pattern as the tunnel /
/// NAT overrides above. Keeping it in a process-global slot avoids changing
/// `serve_with_external_shutdown`'s public signature (the Windows Service
/// host in `daemon_service.rs` calls it with the legacy argument list).
static PENDING_RESTART_ON_EXIT: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

#[allow(dead_code)]
pub(crate) fn set_pending_restart_on_exit(restart: bool) {
    PENDING_RESTART_ON_EXIT.store(restart, std::sync::atomic::Ordering::SeqCst);
}

fn take_pending_restart_on_exit() -> bool {
    PENDING_RESTART_ON_EXIT.swap(false, std::sync::atomic::Ordering::SeqCst)
}

/// Pending `--vacuum-secrets` flag (Wave 6, v0.13.0).
///
/// When the daemon is launched with `--vacuum-secrets` (or
/// `ZLAYER_VACUUM_SECRETS=true`), the boot path wipes
/// `{data_dir}/join_secret` before the HS256 derivation reads it, forcing
/// the HMAC key to regenerate. Useful when an operator suspects the
/// symmetric secret has leaked. Ed25519-signed tokens are unaffected.
///
/// Stored in a process-global slot so `serve()`/`serve_with_nat_overrides()`
/// can stash the parsed CLI value before delegating to
/// `serve_with_external_shutdown()` (whose public signature is pinned by the
/// Windows Service host in `daemon_service.rs`).
static PENDING_VACUUM_SECRETS: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

#[allow(dead_code)]
pub(crate) fn set_pending_vacuum_secrets(vacuum: bool) {
    PENDING_VACUUM_SECRETS.store(vacuum, std::sync::atomic::Ordering::SeqCst);
}

fn take_pending_vacuum_secrets() -> bool {
    PENDING_VACUUM_SECRETS.swap(false, std::sync::atomic::Ordering::SeqCst)
}

/// Idempotently delete `{data_dir}/join_secret`.
///
/// Used by three call sites: the `--vacuum-secrets` flag at startup, the
/// boot-time reconcile that consults `SecretsState::join_secret_wiped_at`,
/// and the long-lived watcher task that drains
/// [`zlayer_secrets::NodeSideEffects::wait_wipe_join_secret`] (fired by the
/// Raft apply wrapper). `reason` is included in the log line so operators
/// can correlate the delete with its trigger.
async fn wipe_join_secret_file(data_dir: &std::path::Path, reason: &str) -> anyhow::Result<()> {
    let path = data_dir.join("join_secret");
    match tokio::fs::remove_file(&path).await {
        Ok(()) => {
            tracing::warn!(
                path = %path.display(),
                reason,
                "removed cluster join_secret",
            );
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            tracing::debug!(
                path = %path.display(),
                reason,
                "join_secret already absent; nothing to remove",
            );
            Ok(())
        }
        Err(e) => Err(e).with_context(|| format!("removing {}", path.display())),
    }
}

/// Resolve the tunnel daemon config from CLI overrides + env vars + defaults.
fn build_tunnel_config_from_overrides(
    overrides: &TunnelCliOverrides,
) -> crate::daemon::TunnelDaemonConfig {
    let mut cfg = crate::daemon::TunnelDaemonConfig::default();

    // Defaults (the same env-var fallbacks clap supplies via `env=...`,
    // duplicated here so the foreground shim path also picks them up when
    // CLI overrides are absent).
    if let Ok(raw) = std::env::var("ZLAYER_TUNNEL_BIND") {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            if let Ok(addr) = trimmed.parse() {
                cfg.bind = addr;
            } else {
                tracing::warn!(value = %trimmed, "Ignoring invalid ZLAYER_TUNNEL_BIND");
            }
        }
    }
    if let Ok(raw) = std::env::var("ZLAYER_TUNNEL_TLS_CERT") {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            cfg.tls_cert = Some(std::path::PathBuf::from(trimmed));
        }
    }
    if let Ok(raw) = std::env::var("ZLAYER_TUNNEL_TLS_KEY") {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            cfg.tls_key = Some(std::path::PathBuf::from(trimmed));
        }
    }
    if let Ok(raw) = std::env::var("ZLAYER_DISABLE_TUNNEL_SERVER") {
        cfg.disabled = !matches!(
            raw.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "no" | "off" | ""
        );
    }

    // CLI overrides win over env.
    if let Some(ref bind) = overrides.bind {
        match bind.parse() {
            Ok(addr) => cfg.bind = addr,
            Err(e) => tracing::warn!(value = %bind, error = %e, "Ignoring invalid --tunnel-bind"),
        }
    }
    if overrides.tls_cert.is_some() {
        cfg.tls_cert.clone_from(&overrides.tls_cert);
    }
    if overrides.tls_key.is_some() {
        cfg.tls_key.clone_from(&overrides.tls_key);
    }
    if overrides.no_tunnel_server {
        cfg.disabled = true;
    }

    cfg
}

/// Process-global slot for the most recently parsed `NatCliOverrides`.
///
/// The serve dispatch flow is:
///   1. `main.rs` parses CLI flags into a `NatCliOverrides` and calls
///      [`serve_with_nat_overrides`].
///   2. That helper writes the overrides into this slot, then defers to
///      [`serve_with_external_shutdown`].
///   3. Inside `serve_with_external_shutdown`, the slot is taken back out
///      and combined with the `ZLAYER_NAT_*` env vars + `NatConfig::default()`.
///
/// Older entry points (the Windows Service `daemon_service.rs` flow and the
/// foreground `serve()` shim) keep their existing signatures and end up with
/// an empty slot — they pick up env-var overrides only.
#[cfg(feature = "nat")]
static PENDING_NAT_OVERRIDES: std::sync::Mutex<Option<NatCliOverrides>> =
    std::sync::Mutex::new(None);

#[cfg(feature = "nat")]
fn set_pending_nat_overrides(overrides: NatCliOverrides) {
    if let Ok(mut slot) = PENDING_NAT_OVERRIDES.lock() {
        *slot = Some(overrides);
    }
}

#[cfg(feature = "nat")]
fn take_pending_nat_overrides() -> NatCliOverrides {
    PENDING_NAT_OVERRIDES
        .lock()
        .ok()
        .and_then(|mut slot| slot.take())
        .unwrap_or_default()
}

/// Bag of NAT-related CLI overrides surfaced from `zlayer serve` flags.
///
/// Resolved in `serve()` against environment variables (`ZLAYER_NAT_*`) and
/// the [`NatConfig::default`] baseline; the result is threaded into the
/// daemon's `OverlayManager` via [`crate::daemon::DaemonConfig::nat`].
#[cfg(feature = "nat")]
#[derive(Debug, Clone, Default)]
pub(crate) struct NatCliOverrides {
    /// `--no-nat`: disable NAT traversal entirely.
    pub no_nat: bool,
    /// `--stun-server <host:port>` (repeatable). Overrides the default STUN
    /// server list when non-empty.
    pub stun_servers: Vec<String>,
    /// `--turn-server <host:port>` (repeatable). Overrides the default TURN
    /// server list when non-empty.
    pub turn_servers: Vec<String>,
    /// `--relay-server-bind <host:port>`. When set, the built-in relay server
    /// listens on this address; otherwise no relay server is started locally.
    pub relay_server_bind: Option<String>,
}

/// Parse a comma-separated host:port list from an env var into a `Vec<String>`.
#[cfg(feature = "nat")]
fn parse_csv_env(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// Parse a boolean-ish env value. "0", "false", "no", "off" => false. Everything
/// else => true. Empty / unset is handled by the caller.
#[cfg(feature = "nat")]
fn parse_bool_env(raw: &str) -> bool {
    !matches!(
        raw.trim().to_ascii_lowercase().as_str(),
        "0" | "false" | "no" | "off"
    )
}

/// Resolve NAT settings from CLI overrides (highest precedence), env vars
/// (middle), and `NatConfig::default()` (lowest).
#[cfg(feature = "nat")]
fn build_nat_config_from_overrides(overrides: &NatCliOverrides) -> NatConfig {
    let mut nat = NatConfig::default();

    // 1. ZLAYER_NAT_ENABLED env (defaults to true via NatConfig::default()).
    if let Ok(raw) = std::env::var("ZLAYER_NAT_ENABLED") {
        if !raw.trim().is_empty() {
            nat.enabled = parse_bool_env(&raw);
        }
    }

    // 2. STUN servers from env (only if set & non-empty).
    if let Ok(raw) = std::env::var("ZLAYER_STUN_SERVERS") {
        let parsed = parse_csv_env(&raw);
        if !parsed.is_empty() {
            nat.stun_servers = parsed
                .into_iter()
                .map(|address| StunServerConfig {
                    address,
                    label: None,
                })
                .collect();
        }
    }

    // 3. TURN servers from env. Format: `host:port` (no auth from env; use a
    //    config file for credentialed TURN). Username/credential default to
    //    empty strings — the runtime should treat empty creds as anonymous.
    if let Ok(raw) = std::env::var("ZLAYER_TURN_SERVERS") {
        let parsed = parse_csv_env(&raw);
        if !parsed.is_empty() {
            nat.turn_servers = parsed
                .into_iter()
                .map(|address| TurnServerConfig {
                    address,
                    username: String::new(),
                    credential: String::new(),
                    region: None,
                })
                .collect();
        }
    }

    // 4. Hole-punch / STUN refresh tunables.
    if let Ok(raw) = std::env::var("ZLAYER_HOLE_PUNCH_TIMEOUT_SECS") {
        if let Ok(secs) = raw.trim().parse::<u64>() {
            nat.hole_punch_timeout_secs = secs;
        }
    }
    if let Ok(raw) = std::env::var("ZLAYER_STUN_REFRESH_INTERVAL_SECS") {
        if let Ok(secs) = raw.trim().parse::<u64>() {
            nat.stun_refresh_interval_secs = secs;
        }
    }

    // 5. Built-in relay server bind from env (host:port).
    if let Ok(raw) = std::env::var("ZLAYER_RELAY_SERVER_BIND") {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            nat.relay_server = parse_relay_bind(trimmed);
        }
    }

    // -----------------------------------------------------------------------
    // CLI overrides (highest precedence) — apply *after* env so they win.
    // -----------------------------------------------------------------------
    if overrides.no_nat {
        nat.enabled = false;
    }
    if !overrides.stun_servers.is_empty() {
        nat.stun_servers = overrides
            .stun_servers
            .iter()
            .cloned()
            .map(|address| StunServerConfig {
                address,
                label: None,
            })
            .collect();
    }
    if !overrides.turn_servers.is_empty() {
        nat.turn_servers = overrides
            .turn_servers
            .iter()
            .cloned()
            .map(|address| TurnServerConfig {
                address,
                username: String::new(),
                credential: String::new(),
                region: None,
            })
            .collect();
    }
    if let Some(bind) = overrides.relay_server_bind.as_deref() {
        let trimmed = bind.trim();
        if !trimmed.is_empty() {
            nat.relay_server = parse_relay_bind(trimmed);
        }
    }

    nat
}

/// Parse a relay-server bind expression `host:port` into a [`RelayServerConfig`].
///
/// Returns `None` if the expression doesn't contain a valid `:port`. The
/// `external_addr` is set to the same string the operator passed; downstream
/// callers can override it if they need to advertise a different reachable
/// address.
#[cfg(feature = "nat")]
fn parse_relay_bind(bind: &str) -> Option<RelayServerConfig> {
    let port_str = bind.rsplit(':').next()?;
    let port: u16 = port_str.parse().ok()?;
    Some(RelayServerConfig {
        listen_port: port,
        external_addr: bind.to_string(),
        max_sessions: 100,
    })
}

/// Redirect process stderr (fd 2) into the tracing subscriber, line by line.
///
/// Background: `boringtun` (and any other dep that uses `eprintln!`) writes
/// directly to fd 2 from worker threads it owns. Those bytes bypass the
/// `tracing` pipeline entirely, so under `systemd`/`launchd` they land in the
/// journal as raw, unstructured text alongside our structured JSON log lines.
///
/// We solve that by replacing fd 2 with the write end of a pipe, then spawning
/// a tokio task that reads the pipe line by line and re-emits each line as a
/// structured `tracing::error!` event with `target = "stderr"`.
///
/// This runs only when stderr is **not** a TTY — interactive
/// `zlayer serve` sessions still see normal stderr output. It is also
/// `#[cfg(unix)]`-only; the Windows daemon uses the SCM event log path.
///
/// On any setup failure (`pipe(2)` / `dup2(2)` returning an error) we log a
/// warning via tracing (which at this point is on stdout / a log file, not
/// stderr) and return without aborting daemon startup — running without the
/// redirect is a strictly better failure mode than refusing to come up.
#[cfg(unix)]
fn install_stderr_redirect_to_tracing() {
    use std::io::IsTerminal;
    use std::os::fd::{AsFd, FromRawFd, IntoRawFd};

    if std::io::stderr().is_terminal() {
        // Foreground from a shell — preserve normal stderr UX.
        return;
    }

    let (read_fd, write_fd) = match nix::unistd::pipe() {
        Ok(pair) => pair,
        Err(e) => {
            tracing::warn!(error = %e, "failed to create stderr-redirect pipe; continuing without redirect");
            return;
        }
    };

    if let Err(e) = nix::unistd::dup2_stderr(write_fd.as_fd()) {
        tracing::warn!(error = %e, "failed to dup2 stderr to pipe; continuing without redirect");
        return;
    }
    // After dup2, fd 2 is now a duplicate of the pipe write end. We can drop
    // our extra OwnedFd handle; the kernel keeps the pipe alive via fd 2 for
    // the lifetime of the process.
    drop(write_fd);

    // Hand the pipe read end to a background task that re-emits each line as
    // a structured tracing event. The task lives until EOF on the pipe, which
    // only happens when every writer (including fd 2) is closed — i.e. at
    // process shutdown.
    let read_fd_raw = read_fd.into_raw_fd();
    tokio::spawn(async move {
        use tokio::io::AsyncBufReadExt;

        // SAFETY: `read_fd_raw` came from an `OwnedFd` we just forfeited via
        // `into_raw_fd`, so we are the unique owner. `File::from_raw_fd` takes
        // ownership and will close the fd when the resulting `File` drops.
        #[allow(unsafe_code)]
        let std_file = unsafe { std::fs::File::from_raw_fd(read_fd_raw) };
        let async_file = tokio::fs::File::from_std(std_file);
        let mut lines = tokio::io::BufReader::new(async_file).lines();
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    tracing::error!(target: "stderr", "{line}");
                }
                Ok(None) => break, // EOF: all writers (including fd 2) closed.
                Err(e) => {
                    tracing::warn!(error = %e, "stderr-redirect reader I/O error; ending task");
                    break;
                }
            }
        }
    });
}

#[cfg(not(unix))]
fn install_stderr_redirect_to_tracing() {
    // Windows daemon path uses the SCM event log; nothing to redirect here.
}

/// Entry point equivalent to [`serve`] but with an additional external
/// shutdown trigger. Used by the Windows Service event loop (I-1) so the SCM
/// control handler can signal the daemon to shut down gracefully.
///
/// When `external_shutdown` is `Some(rx)`, the server shuts down on *any* of:
///
///   * `Ctrl+C`
///   * `SIGTERM` (Unix only)
///   * `rx` transitioning to `true`
///
/// When `None`, behaves exactly like [`serve`] — foreground signal handling
/// only.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(crate) async fn serve_with_external_shutdown(
    bind: &str,
    jwt_secret: Option<String>,
    no_swagger: bool,
    socket_path: &str,
    host_network: bool,
    data_dir: std::path::PathBuf,
    deployment_name: String,
    wg_port: Option<u16>,
    dns_port: Option<u16>,
    external_shutdown: Option<tokio::sync::watch::Receiver<bool>>,
) -> Result<()> {
    // NOTE: the stderr-to-tracing redirect is deliberately installed AFTER
    // `init_daemon` returns (see below). Installing it here would swallow any
    // early-boot error from `init_daemon` -- including the pre-0.11.20 data
    // dir migration -- into the tracing file appender instead of letting it
    // reach journald via the original fd 2. The `eprintln!` in `main.rs`
    // that prints the final error from `run(cli)` needs the unredirected
    // stderr to be visible in `journalctl`.

    // Pick up any CLI overrides stashed by `serve_with_nat_overrides`, plus
    // `ZLAYER_NAT_*` env vars, plus `NatConfig::default()`. Empty when called
    // through the foreground `serve()` shim or the Windows Service flow.
    #[cfg(feature = "nat")]
    let nat_overrides = take_pending_nat_overrides();

    // Pick up tunnel-server CLI overrides + ZLAYER_TUNNEL_* env vars.
    let tunnel_overrides = take_pending_tunnel_overrides();
    let tunnel_config = build_tunnel_config_from_overrides(&tunnel_overrides);

    // Pick up daemon-API TLS CLI overrides. Translated below into an
    // `ApiTlsConfig` once `cert_manager` is in scope (`init_daemon` always
    // constructs one, so the `Managed` ACME path never lacks a backing
    // resolver and we don't need a no-proxy guard here).
    let api_tls_overrides = take_pending_api_tls_overrides();
    // Resolve the JWT signing secret. Priority is:
    //   1. Explicit `--jwt-secret` flag / `ZLAYER_JWT_SECRET` env var
    //      (clap reads both into the `jwt_secret` parameter).
    //   2. A persisted file under `data_dir` managed by
    //      `zlayer_secrets::JwtSecretManager`.
    //   3. Auto-generate 64 random bytes and persist to that file.
    //
    // Without persistence, every restart minted a fresh secret and silently
    // invalidated every previously issued session cookie.
    let jwt_secret = if let Some(provided) = jwt_secret {
        if provided.is_empty() {
            anyhow::bail!("--jwt-secret was provided but is empty");
        }
        info!("Using JWT secret from --jwt-secret flag / ZLAYER_JWT_SECRET");
        SecretString::from(provided)
    } else {
        zlayer_secrets::JwtSecretManager::with_base_dir(&data_dir)
            .get_or_create("zlayer")
            .context("Failed to resolve JWT signing secret")?
    };
    let jwt_secret_raw = jwt_secret.expose_secret().to_string();

    let bind_addr: std::net::SocketAddr = bind
        .parse()
        .context(format!("Invalid bind address: {bind}"))?;

    // -----------------------------------------------------------------------
    // 1. Create DaemonConfig
    // -----------------------------------------------------------------------
    let log_dir = crate::cli::default_log_dir(&data_dir);
    let run_dir = crate::cli::default_run_dir(&data_dir);
    let s3_storage = std::env::var("ZLAYER_S3_BUCKET").ok().map(|bucket| {
        let mut config = zlayer_storage::LayerStorageConfig::new(&bucket);
        if let Ok(region) = std::env::var("ZLAYER_S3_REGION") {
            config = config.with_region(&region);
        }
        if let Ok(endpoint) = std::env::var("ZLAYER_S3_ENDPOINT") {
            config = config.with_endpoint_url(&endpoint);
        }
        config = config
            .with_staging_dir(data_dir.join("layer-staging"))
            .with_state_db_path(data_dir.join("layer-sync-state.sqlite"));
        config
    });

    // -----------------------------------------------------------------------
    // NAT traversal: resolve CLI flags > env vars > NatConfig::default().
    //
    // CLI flags always win when set. Env vars (`ZLAYER_NAT_*`) act as the
    // fallback before the built-in defaults kick in. The resulting NatConfig
    // is threaded into DaemonConfig and through to the OverlayManager so the
    // overlay transports the agent builds carry the right settings.
    // -----------------------------------------------------------------------
    #[cfg(feature = "nat")]
    let nat_config = build_nat_config_from_overrides(&nat_overrides);

    // Shared network policy list — the proxy and API both hold a reference.
    let network_policies = std::sync::Arc::new(tokio::sync::RwLock::new(Vec::<
        zlayer_spec::NetworkPolicySpec,
    >::new()));

    // DNS port precedence: CLI flag > zlayer_overlay::DEFAULT_DNS_PORT.
    // Threaded into DaemonConfig.dns_port (default 15353); the overlay
    // bootstrap consumes the resolved value via `config.dns_port`.
    let resolved_dns_port = dns_port.unwrap_or(zlayer_overlay::DEFAULT_DNS_PORT);

    let config = DaemonConfig {
        host_network,
        deployment_name,
        dns_port: resolved_dns_port,
        data_dir,
        log_dir,
        run_dir,
        s3_storage,
        auth_context: Some(zlayer_agent::ContainerAuthContext {
            api_url: container_reachable_api_url(&bind_addr),
            jwt_secret: jwt_secret_raw.clone(),
            socket_path: socket_path.to_string(),
        }),
        network_policies: Some(std::sync::Arc::clone(&network_policies)),
        #[cfg(feature = "nat")]
        nat: nat_config,
        tunnel: tunnel_config,
        ..Default::default()
    };

    // -----------------------------------------------------------------------
    // 1b. Clean up any stale daemon from a previous run
    // -----------------------------------------------------------------------
    cleanup_stale_daemon(&config, socket_path, bind, wg_port).await;

    // -----------------------------------------------------------------------
    // 1c. Bind TCP + Unix listeners early so the socket file exists on disk
    //     before restore_deployments tries to bind-mount it into containers.
    // -----------------------------------------------------------------------
    let bound_listeners =
        zlayer_api::bind_dual_with_local_auth(bind_addr, socket_path, &jwt_secret_raw).await?;

    // -----------------------------------------------------------------------
    // 2. Initialise all daemon infrastructure
    // -----------------------------------------------------------------------
    info!("Initialising daemon infrastructure");
    let state = init_daemon(&config).await?;

    // Capture stderr (fd 2) into the tracing pipeline so direct `eprintln!`
    // writes from dependencies (notably boringtun's tun-read worker threads)
    // land as structured `tracing::error!` events instead of raw lines in the
    // systemd/launchd journal. No-op when stderr is a TTY (interactive
    // `zlayer serve`) or on Windows. Deliberately installed AFTER `init_daemon`
    // so any early-boot error (e.g. on-disk migration failure) still reaches
    // the original stderr -> journald path before the redirect is in place.
    install_stderr_redirect_to_tracing();

    // -----------------------------------------------------------------------
    // 2b. Restore previously-persisted deployments
    // -----------------------------------------------------------------------
    if let Err(e) = Box::pin(restore_deployments(&state)).await {
        warn!(error = %e, "Deployment restoration encountered errors (non-fatal)");
    }

    // Destructure the state so we can rewrap the ServiceManager for the router
    // while keeping shutdown-relevant handles separate.
    #[allow(clippy::used_underscore_binding)]
    let crate::daemon::DaemonState {
        runtime,
        overlay,
        dns,
        dns_handle,
        proxy,
        supervisor,
        supervisor_handle,
        manager,
        storage,
        secrets,
        credential_store,
        stream_registry,
        cert_manager,
        log_rotator_handle,
        keystore_pruner_handle,
        health_checker_handle,
        nat_maintenance_handle,
        node_config,
        raft: _raft,
        raft_server_handle,
        heartbeat_handle,
        dead_node_detection_handle,
        scheduler: _scheduler,
        internal_token: daemon_internal_token,
        node_effects,
        replicator,
        job_executor,
        cron_scheduler,
        tunnel: tunnel_handles,
        tunnel_api_state,
    } = state;

    // -----------------------------------------------------------------------
    // 3. Write daemon metadata JSON
    // -----------------------------------------------------------------------
    let metadata = DaemonMetadata {
        pid: std::process::id(),
        started_at: chrono::Utc::now().to_rfc3339(),
        api_bind: bind.to_string(),
        socket_path: socket_path.to_string(),
        host_network: config.host_network,
        overlay_cidr: "10.200.0.0/16".to_string(),
    };

    let metadata_path = config.data_dir.join("daemon.json");
    let metadata_json =
        serde_json::to_string_pretty(&metadata).context("Failed to serialise daemon metadata")?;
    tokio::fs::write(&metadata_path, &metadata_json)
        .await
        .with_context(|| {
            format!(
                "Failed to write daemon metadata to {}",
                metadata_path.display()
            )
        })?;
    info!(path = %metadata_path.display(), "Daemon metadata written");

    // -----------------------------------------------------------------------
    // 3a'. Wave 6 (v0.13.0): `--vacuum-secrets` wipes the cluster
    // `join_secret` BEFORE the loader runs so the HS256 HMAC key
    // regenerates on the very next step. Ed25519 signing material (under
    // `cluster_signing.key`) is intentionally untouched. The flag is
    // consumed exactly once (the static slot is swapped to false) so a
    // supervisor-driven respawn after this run does not vacuum a fresh
    // secret on every restart.
    // -----------------------------------------------------------------------
    if take_pending_vacuum_secrets() {
        wipe_join_secret_file(&config.data_dir, "--vacuum-secrets flag").await?;
    }

    // -----------------------------------------------------------------------
    // 3a''. Boot reconcile: if a prior cluster-wide `WipeJoinSecret` Raft op
    // already applied (state machine has `join_secret_wiped_at = Some(_)`)
    // and this node still has a local copy (e.g. it restored from a snapshot
    // where the wipe happened while this node was offline), delete it now
    // before the HS256 HMAC loader runs. Idempotent; the helper is a no-op
    // if the file is already absent.
    //
    // `_raft` may be `None` if Raft failed to initialize in `init_daemon`;
    // in that case there is no cluster consensus to consult and we skip
    // the reconcile entirely.
    // -----------------------------------------------------------------------
    if let Some(raft) = _raft.as_ref() {
        let secrets_state = raft.secrets_state().await;
        if secrets_state.join_secret_wiped_at.is_some() {
            wipe_join_secret_file(&config.data_dir, "WipeJoinSecret applied in prior boot").await?;
        }
    }

    // -----------------------------------------------------------------------
    // 3a'''. Watcher: drain `NodeSideEffects::wait_wipe_join_secret` and
    // delete `{data_dir}/join_secret` on each fire. The Raft apply wrapper
    // in `zlayer_scheduler::raft::ClusterState` calls `fire_wipe_join_secret`
    // whenever `SecretsRaftOp::WipeJoinSecret` applies successfully, so
    // this is the live (sub-second) path. The task lives for the daemon's
    // lifetime; we keep the JoinHandle for shutdown.
    // -----------------------------------------------------------------------
    // The watcher is fire-and-forget for the daemon's lifetime; the
    // `JoinHandle` is dropped immediately and the task runs detached.
    // Graceful shutdown reaps it via the process exit.
    {
        let data_dir = config.data_dir.clone();
        let effects = std::sync::Arc::clone(&node_effects);
        std::mem::drop(tokio::spawn(async move {
            loop {
                effects.wait_wipe_join_secret().await;
                if let Err(e) = wipe_join_secret_file(&data_dir, "WipeJoinSecret notify").await {
                    tracing::error!(error = %e, "join_secret watcher: wipe failed");
                }
            }
        }));
    }

    // -----------------------------------------------------------------------
    // 3b. Generate or load the cluster join secret
    // -----------------------------------------------------------------------
    let join_secret = {
        let join_secret_path = config.data_dir.join("join_secret");
        if join_secret_path.exists() {
            match tokio::fs::read_to_string(&join_secret_path).await {
                Ok(secret) => {
                    let secret = secret.trim().to_string();
                    info!(
                        path = %join_secret_path.display(),
                        "Loaded existing cluster join secret"
                    );
                    Some(secret)
                }
                Err(e) => {
                    warn!(
                        path = %join_secret_path.display(),
                        error = %e,
                        "Failed to load join secret, generating new one"
                    );
                    None
                }
            }
        } else {
            None
        }
    };

    let join_secret = if let Some(s) = join_secret {
        s
    } else {
        // Generate a random 32-byte secret and hex-encode it
        use rand::Rng;
        let secret_bytes: [u8; 32] = rand::rng().random();
        let secret = hex::encode(secret_bytes);

        // Persist it so it survives restarts
        let join_secret_path = config.data_dir.join("join_secret");
        if let Err(e) = tokio::fs::write(&join_secret_path, &secret).await {
            warn!(
                path = %join_secret_path.display(),
                error = %e,
                "Failed to persist join secret (token validation will not work across restarts)"
            );
        } else {
            info!(
                path = %join_secret_path.display(),
                "Generated and persisted new cluster join secret"
            );
        }

        secret
    };

    // Wave 1: Ed25519 signer for signed cluster join tokens.
    //
    // We persist the signing seed at {data_dir}/cluster_signing.key (mode 0600).
    // The public key is exposed via GET /api/v1/cluster/signing-pubkey (Agent 1.3).
    // HS256 (using join_secret above) and plaintext tokens remain accepted; the
    // signer is plumbed but no token-format changes happen until Wave 3.
    let cluster_signer = {
        let path = config.data_dir.join("cluster_signing.key");
        let signer = zlayer_secrets::ClusterSigner::load_or_generate(&path)
            .await
            .with_context(|| {
                format!(
                    "Failed to load or generate cluster signing keypair at {}",
                    path.display()
                )
            })?;
        info!(
            kid = %signer.key_id(),
            public_key_b64 = %signer.public_key_b64(),
            path = %path.display(),
            "Loaded cluster signing keypair"
        );
        Arc::new(signer)
    };

    // Wave 9: long-lived cluster CA. Generated once per cluster at
    // first daemon start; NEVER rotated. Used to issue CaCerts
    // embedded in v=2 signed join tokens so foreign clusters that
    // imported this cluster's TrustBundle can validate them.
    let cluster_ca = {
        let path = config.data_dir.join("cluster_ca.key");
        let ca = zlayer_secrets::ClusterCa::load_or_generate(&path)
            .await
            .context("loading or generating cluster CA")?;
        info!(ca_kid = %ca.ca_kid(), path = %path.display(), "loaded cluster CA");
        Some(std::sync::Arc::new(ca))
    };

    // Wave 11D: if a prior operator command set the cluster JWT
    // algorithm to `Eddsa` but `{data_dir}/join_secret` is still
    // present, surface a warning suggesting the cleanup. This is
    // BEST-EFFORT — Raft state isn't directly readable here, so we
    // only check the local file existence and rely on the next
    // operator interaction to surface the policy.
    if config.data_dir.join("join_secret").exists() {
        tracing::info!(
            "{{data_dir}}/join_secret is present; if you've migrated to EdDSA, run \
             `zlayer cluster decommission-hs256 --vacuum-secret` to remove it"
        );
    }

    // -----------------------------------------------------------------------
    // 3b'. Load (or generate + persist) the daemon UUID. This stamps every
    // hex container ID minted via `ContainerIdMap::compute_hex` so the same
    // container hashes to the same id across restarts.
    // -----------------------------------------------------------------------
    let daemon_uuid = {
        let daemon_uuid_path = config.data_dir.join("daemon_uuid");
        // The `Ok` arm is intentionally larger than the `Err` arm (it has to
        // distinguish empty-file from valid-content recovery), so clippy's
        // `single_match_else` lint pushes us toward `if let Ok(...) else`.
        // That refactor would force duplicating the whole regenerate-and-write
        // block; the `match` is materially clearer here.
        #[allow(clippy::single_match_else)]
        match tokio::fs::read_to_string(&daemon_uuid_path).await {
            Ok(contents) => {
                let trimmed = contents.trim().to_string();
                if trimmed.is_empty() {
                    let fresh = uuid::Uuid::new_v4().as_simple().to_string();
                    if let Err(e) = tokio::fs::write(&daemon_uuid_path, &fresh).await {
                        warn!(
                            path = %daemon_uuid_path.display(),
                            error = %e,
                            "Failed to persist regenerated daemon_uuid (hex container IDs will not be stable across restarts)"
                        );
                    } else {
                        info!(
                            path = %daemon_uuid_path.display(),
                            "daemon_uuid was empty; regenerated and persisted"
                        );
                    }
                    fresh
                } else {
                    info!(
                        path = %daemon_uuid_path.display(),
                        "Loaded existing daemon UUID"
                    );
                    trimmed
                }
            }
            Err(_) => {
                let fresh = uuid::Uuid::new_v4().as_simple().to_string();
                if let Err(e) = tokio::fs::write(&daemon_uuid_path, &fresh).await {
                    warn!(
                        path = %daemon_uuid_path.display(),
                        error = %e,
                        "Failed to persist daemon_uuid (hex container IDs will not be stable across restarts)"
                    );
                } else {
                    info!(
                        path = %daemon_uuid_path.display(),
                        "Generated and persisted new daemon UUID"
                    );
                }
                fresh
            }
        }
    };

    // -----------------------------------------------------------------------
    // 3c. Open the persistent API store bundle (11 databases under data_dir).
    //     Opened AFTER init_daemon so the data dir exists; kept separate from
    //     the deployment SqlxStorage in daemon.rs which is tied to the S3
    //     SqliteReplicator path.
    // -----------------------------------------------------------------------
    let bundle = StorageBundle::open(Some(config.data_dir.as_path())).await?;

    let identity = Arc::new(
        zlayer_api::IdentityManager::new(bundle.users.clone(), credential_store.clone())
            .with_oidc(bundle.oidc_identities.clone()),
    );

    // Non-interactive admin bootstrap: if the users table is empty and
    // ZLAYER_BOOTSTRAP_EMAIL + password source envs are set, seed the
    // initial admin before the listener accepts. Idempotent.
    crate::bootstrap_admin::maybe_bootstrap_admin(&identity).await?;

    // One-shot migration: give every existing admin an explicit
    // `environment:write` grant for every existing environment. Admins
    // short-circuit permission checks so this isn't required for correctness,
    // but the `GET /permissions/by-resource?kind=environment` endpoint and the
    // manager UI's permission listings want the grants to be visible rather
    // than implicit. Idempotent — gated on an audit-log marker row.
    crate::bootstrap_admin_env_grants::run(
        bundle.users.clone(),
        bundle.environments.clone(),
        bundle.projects.clone(),
        bundle.permissions.clone(),
        bundle.audit.clone(),
    )
    .await
    .context("Failed to run admin env-grants migration")?;

    // Load OIDC providers from env (ZLAYER_OIDC_<NAME>_*). Empty map when
    // SSO is disabled — callers get 404 on /auth/oidc/* endpoints.
    let oidc_providers =
        zlayer_api::oidc::OidcProviderConfig::load_all().context("loading OIDC provider config")?;
    let oidc_clients: std::collections::HashMap<String, zlayer_api::oidc::OidcClient> =
        oidc_providers
            .into_iter()
            .map(|cfg| (cfg.name.clone(), zlayer_api::oidc::OidcClient::new(cfg)))
            .collect();
    if !oidc_clients.is_empty() {
        info!(
            providers = ?oidc_clients.keys().collect::<Vec<_>>(),
            "OIDC / SSO enabled",
        );
    }
    let oidc_state = Arc::new(zlayer_api::oidc::StateTokenStore::new());

    // -----------------------------------------------------------------------
    // 4. Build the full API router
    // -----------------------------------------------------------------------
    // Translate the parsed `--api-tls-*` CLI overrides into an
    // `ApiTlsConfig`. `cert_manager` is the proxy's `Arc<CertManager>`
    // destructured from `DaemonState` above; sharing it with the daemon
    // API listener means ACME-managed certs hot-reload uniformly across the
    // proxy and the API. `init_daemon` unconditionally builds a
    // `CertManager`, so the ACME path always has a backing resolver here
    // and no "no-proxy" guard is required.
    let api_tls = build_api_tls_config_from_overrides(&api_tls_overrides, &cert_manager);
    match &api_tls {
        Some(zlayer_api::config::ApiTlsConfig::Static {
            cert_path,
            key_path,
        }) => {
            info!(
                cert = %cert_path.display(),
                key = %key_path.display(),
                "Daemon API listener will use static TLS",
            );
        }
        Some(zlayer_api::config::ApiTlsConfig::Managed { .. }) => {
            info!("Daemon API listener will use ACME/CertManager TLS");
        }
        None => {
            info!("Daemon API listener will use plain HTTP (no TLS configured)");
        }
    }

    let api_config = ApiConfig {
        bind: bind_addr,
        tls: api_tls,
        jwt_secret,
        swagger_enabled: !no_swagger,
        credential_store: Some(credential_store),
        user_store: Some(bundle.users.clone()),
        identity: Some(identity),
        oidc_clients,
        oidc_state,
        ..Default::default()
    };

    // The router builder functions expect Arc<RwLock<ServiceManager>>.
    // DaemonState provides Arc<ServiceManager>. Unwrap the Arc (we are the
    // sole owner at this point) and re-wrap in RwLock + Arc.
    let service_manager = Arc::try_unwrap(manager)
        .map(|sm| Arc::new(RwLock::new(sm)))
        .map_err(|_| {
            anyhow::anyhow!(
                "BUG: ServiceManager Arc has multiple owners at serve() entry; \
                 this should never happen"
            )
        })?;

    // Build deployment state with orchestration wiring so create_deployment
    // actually registers services, sets up overlays, and scales containers.
    // Clone dns_handle so the deployment state keeps one for API handlers while
    // we retain the original for explicit shutdown cleanup.
    let deployment_state = zlayer_api::DeploymentState::with_orchestration(
        storage.clone() as Arc<dyn zlayer_api::DeploymentStorage + Send + Sync>,
        Arc::clone(&service_manager),
        overlay.clone(),
        Arc::clone(&proxy),
        dns_handle.clone(),
    );

    // Use the internal token generated during daemon init so the scheduler
    // (which already has a copy) and the API InternalState share the same secret.
    let internal_token = daemon_internal_token;

    // Build the core router using the orchestration-wired deployment state.
    // This ensures create_deployment actually orchestrates containers.
    let base_router = zlayer_api::build_router_with_deployment_state(
        &api_config,
        deployment_state,
        service_manager.clone(),
        storage.clone() as Arc<dyn zlayer_api::DeploymentStorage + Send + Sync>,
    );

    // Add internal routes for scheduler-to-agent communication.
    // Include the overlay interface name so the add-peer endpoint can manage
    // WireGuard peers on this node.
    let overlay_interface = if config.host_network {
        None
    } else {
        Some(zlayer_overlay::DEFAULT_INTERFACE_NAME.to_string())
    };
    let internal_state = zlayer_api::InternalState::with_overlay(
        service_manager,
        internal_token.clone(),
        overlay_interface,
    );
    // Attach the Raft handle when clustered so the pre-self-upgrade
    // `trigger-elect` endpoint can nudge this node into campaigning when
    // the leader steps down for a binary swap.
    let internal_state = if let Some(raft) = _raft.as_ref() {
        internal_state.with_raft(raft.clone())
    } else {
        internal_state
    };
    let internal_routes = zlayer_api::build_internal_routes(internal_state);
    let base_router = base_router.nest("/api/v1/internal", internal_routes);

    // Add secrets routes — env-aware so secrets handlers can resolve the
    // environment scope from the bundle's persistent environments store.
    // Wired with the permission store + group store so secrets endpoints
    // can enforce per-env RBAC, per-secret grants, wildcard grants, and
    // group-membership grants in one shot via `require_secret_perm`.
    //
    // The daemon currently always uses the standalone
    // `PersistentSecretsStore`; Task #18 will swap to `RaftSecretsStore`
    // when `--cluster` is on. The `Arc<dyn SecretsStore>` field on
    // `SecretsState` makes that swap cheap.
    let secrets_state = zlayer_api::SecretsState::with_full_rbac(
        secrets,
        bundle.environments.clone(),
        bundle.permissions.clone(),
        bundle.groups.clone(),
    );
    let environments_state = zlayer_api::EnvironmentsState::new(bundle.environments.clone());
    let secrets_routes = zlayer_api::build_secrets_routes(secrets_state.clone());
    let mut router = base_router.nest("/api/v1/secrets", secrets_routes);

    // Add environment routes (CRUD; cascade-safety check against secrets state)
    let environment_routes =
        zlayer_api::build_environment_routes(environments_state, secrets_state);
    router = router.nest("/api/v1/environments", environment_routes);

    // Shared on-disk root for project working copies. `ProjectState`,
    // `SyncState`, and `WorkflowsState` (constructed further below) all
    // resolve `{clone_root}/{project_id}` so every surface that pulls a
    // project's git repo lands in the same place.
    let projects_clone_root = config.data_dir.join("projects");

    // Add project routes (CRUD + deployment linking + pull)
    let mut project_state = zlayer_api::ProjectState::new(bundle.projects.clone());
    project_state.clone_root.clone_from(&projects_clone_root);
    let project_routes = zlayer_api::build_project_routes(project_state);
    router = router.nest("/api/v1/projects", project_routes);

    // Add sync routes (git-backed resource reconciliation).
    // Sync apply upserts / deletes `DeploymentSpec` manifests, so it needs a
    // handle to the persistent deployment store (the same one wired into the
    // deployment routes above).
    let sync_state = zlayer_api::SyncState::with_clone_root(
        bundle.syncs.clone(),
        storage.clone() as Arc<dyn zlayer_api::DeploymentStorage + Send + Sync>,
        projects_clone_root.clone(),
    );
    let sync_routes = zlayer_api::build_sync_routes(sync_state);
    router = router.nest("/api/v1/syncs", sync_routes);

    // Add variable routes (plaintext key-value config) — persistent store
    let variable_state = zlayer_api::VariableState::new(bundle.variables.clone());
    let variable_routes = zlayer_api::build_variable_routes(variable_state);
    router = router.nest("/api/v1/variables", variable_routes);

    // Add task routes (named runnable scripts) — persistent store
    let tasks_state = zlayer_api::TasksState::new(bundle.tasks.clone());
    let task_routes = zlayer_api::build_task_routes(tasks_state);
    router = router.nest("/api/v1/tasks", task_routes);

    // Build the BuildState up front so it can be shared by both the build
    // routes (mounted further below) and the workflow state (which needs
    // the BuildManager for `BuildProject` actions).
    let build_dir = config.data_dir.join("builds");
    let build_state = BuildState::new(build_dir);

    // Add workflow routes (DAGs of steps composing tasks, builds, deploys, syncs).
    // WorkflowsState carries every handle the action arms need so they execute
    // for real (not a placeholder "would execute" string).
    let workflows_state = zlayer_api::WorkflowsState::new(
        bundle.workflows.clone(),
        bundle.tasks.clone(),
        bundle.projects.clone(),
        storage.clone() as Arc<dyn zlayer_api::DeploymentStorage + Send + Sync>,
        bundle.syncs.clone(),
        build_state.manager.clone(),
        projects_clone_root.clone(),
    );
    let workflow_routes = zlayer_api::build_workflow_routes(workflows_state);
    router = router.nest("/api/v1/workflows", workflow_routes);

    // Add notifier routes (Slack, Discord, webhook, SMTP) — persistent store
    let notifiers_state = zlayer_api::NotifiersState::new(bundle.notifiers.clone());
    let notifier_routes = zlayer_api::build_notifier_routes(notifiers_state);
    router = router.nest("/api/v1/notifiers", notifier_routes);

    // Add group routes (user group CRUD and membership) — persistent store
    let groups_state = zlayer_api::GroupsState::new(bundle.groups.clone());
    let group_routes = zlayer_api::build_group_routes(groups_state);
    router = router.nest("/api/v1/groups", group_routes);

    // Add permission routes (grant/revoke/list) — persistent stores
    let permissions_state =
        zlayer_api::PermissionsState::new(bundle.permissions.clone(), bundle.groups.clone());
    let permission_routes = zlayer_api::build_permission_routes(permissions_state);
    router = router.nest("/api/v1/permissions", permission_routes);

    // Add audit routes (query audit log, admin only) — persistent store
    let audit_state = zlayer_api::AuditState::new(bundle.audit.clone());
    let audit_routes = zlayer_api::build_audit_routes(audit_state);
    router = router.nest("/api/v1/audit", audit_routes);

    // Layer audit middleware so mutating requests are recorded
    router = router.layer(zlayer_api::Extension(zlayer_api::AuditStoreExtension(
        bundle.audit.clone(),
    )));

    // Merge network access-control routes (shares the same policy list as the proxy)
    let network_state = NetworkApiState {
        networks: std::sync::Arc::clone(&network_policies),
    };
    let network_routes = build_network_routes(network_state);
    router = router.nest("/api/v1/networks", network_routes);

    // Merge node management routes
    let node_state = NodeApiState::new();
    let node_routes = build_node_routes(node_state);
    router = router.nest("/api/v1/nodes", node_routes);

    // Merge overlay network routes with real overlay manager and DNS references
    let overlay_state = match (&overlay, &dns) {
        (Some(om), Some(d)) => OverlayApiState::with_overlay_and_dns(Arc::clone(om), Arc::clone(d)),
        (Some(om), None) => OverlayApiState::with_overlay(Arc::clone(om)),
        _ => OverlayApiState::new(),
    };
    let overlay_routes = build_overlay_routes(overlay_state);
    router = router.nest("/api/v1/overlay", overlay_routes);

    // Merge tunnel routes -- reuse the state built in init_daemon so the
    // daemon-side tunnel server's TokenValidator and the API handler share
    // the same token map and access manager.
    let tunnel_routes = build_tunnel_routes(tunnel_api_state);
    router = router.nest("/api/v1/tunnels", tunnel_routes);

    // Merge proxy status routes
    let proxy_state = zlayer_api::ProxyApiState {
        registry: Some(proxy.registry()),
        load_balancer: Some(proxy.load_balancer()),
        cert_manager: Some(Arc::clone(&cert_manager)),
        stream_registry: Some(Arc::clone(&stream_registry)),
    };
    let proxy_routes = zlayer_api::build_proxy_routes(proxy_state);
    router = router.nest("/api/v1/proxy", proxy_routes);

    // Merge storage replication status routes
    let storage_api_state = match replicator.as_ref() {
        Some(r) => zlayer_api::StorageState::with_replicator(Arc::clone(r)),
        None => zlayer_api::StorageState::new(),
    };
    let storage_routes = zlayer_api::build_storage_routes(storage_api_state);
    router = router.nest("/api/v1/storage", storage_routes);

    // Construct the daemon-wide event bus early so all resource states
    // (image, container, network, volume) publish lifecycle events on the
    // same broadcast channel. Subscribers of `GET /api/v1/events` then see
    // every transition regardless of which handler emitted it.
    let event_bus = zlayer_api::DaemonEventBus::new();

    // Merge image management routes (list / rm / system prune)
    let image_state =
        zlayer_api::ImageState::new(runtime.clone()).with_event_bus(event_bus.clone());
    let image_routes = zlayer_api::build_image_routes_with_state(image_state);
    router = router.nest("/api/v1", image_routes);

    // Merge cluster routes (join, node listing)
    // Initialize CIDR-aware IP allocator for overlay address assignment
    let ip_allocator_path = config.data_dir.join("ip_allocator.json");
    let ip_allocator = {
        // Try to load persisted state first
        if ip_allocator_path.exists() {
            match IpAllocator::load(&ip_allocator_path).await {
                Ok(alloc) => {
                    info!(
                        path = %ip_allocator_path.display(),
                        allocated = alloc.allocated_count(),
                        "Loaded persisted IP allocator state"
                    );
                    alloc
                }
                Err(e) => {
                    warn!(
                        path = %ip_allocator_path.display(),
                        error = %e,
                        "Failed to load IP allocator state, creating fresh"
                    );
                    IpAllocator::new(&node_config.overlay_cidr)
                        .context("Invalid overlay CIDR in node config")?
                }
            }
        } else {
            IpAllocator::new(&node_config.overlay_cidr)
                .context("Invalid overlay CIDR in node config")?
        }
    };

    // For the leader node, reserve the first overlay IP if not already allocated
    let mut ip_allocator = ip_allocator;
    if node_config.is_leader {
        if let Ok(first_ip) = ip_allocator.allocate_first() {
            info!(overlay_ip = %first_ip, "Reserved first overlay IP for leader node");
        }
        // allocate_first() errors if already allocated -- that's fine on restart
    }

    // Persist the initial state
    if let Err(e) = ip_allocator.save(&ip_allocator_path).await {
        warn!("Failed to persist initial IP allocator state: {e}");
    }

    let ip_allocator = Arc::new(RwLock::new(ip_allocator));

    // Initialize leader-side slice allocator. Carves the cluster CIDR into
    // `/28` slices assigned to each joining node. Persists snapshots to
    // `slice_allocator.json` next to the IP allocator state.
    let slice_allocator_path = config.data_dir.join("slice_allocator.json");
    let slice_allocator = {
        let cluster_cidr: zlayer_overlay::ipnet::IpNet = node_config
            .overlay_cidr
            .parse()
            .context("Invalid overlay CIDR in node config (slice allocator)")?;
        if slice_allocator_path.exists() {
            match tokio::fs::read_to_string(&slice_allocator_path).await {
                Ok(contents) => match serde_json::from_str::<
                    zlayer_overlay::NodeSliceAllocatorSnapshot,
                >(&contents)
                {
                    Ok(snapshot) => match zlayer_overlay::NodeSliceAllocator::restore(snapshot) {
                        Ok(alloc) => {
                            info!(
                                path = %slice_allocator_path.display(),
                                "Loaded persisted slice allocator state"
                            );
                            alloc
                        }
                        Err(e) => {
                            warn!(
                                path = %slice_allocator_path.display(),
                                error = %e,
                                "Failed to restore slice allocator state, creating fresh"
                            );
                            zlayer_overlay::NodeSliceAllocator::new(
                                cluster_cidr,
                                zlayer_overlay::DEFAULT_SLICE_PREFIX,
                            )
                            .context("Failed to create slice allocator")?
                        }
                    },
                    Err(e) => {
                        warn!(
                            path = %slice_allocator_path.display(),
                            error = %e,
                            "Failed to parse slice allocator snapshot, creating fresh"
                        );
                        zlayer_overlay::NodeSliceAllocator::new(
                            cluster_cidr,
                            zlayer_overlay::DEFAULT_SLICE_PREFIX,
                        )
                        .context("Failed to create slice allocator")?
                    }
                },
                Err(e) => {
                    warn!(
                        path = %slice_allocator_path.display(),
                        error = %e,
                        "Failed to read slice allocator state, creating fresh"
                    );
                    zlayer_overlay::NodeSliceAllocator::new(
                        cluster_cidr,
                        zlayer_overlay::DEFAULT_SLICE_PREFIX,
                    )
                    .context("Failed to create slice allocator")?
                }
            }
        } else {
            zlayer_overlay::NodeSliceAllocator::new(
                cluster_cidr,
                zlayer_overlay::DEFAULT_SLICE_PREFIX,
            )
            .context("Failed to create slice allocator")?
        }
    };
    let slice_allocator = Arc::new(RwLock::new(slice_allocator));

    let mut cluster_state = ClusterApiState::with_internal_token(
        _raft.clone(),
        Some(join_secret),
        ip_allocator,
        Some(ip_allocator_path),
        slice_allocator,
        Some(slice_allocator_path),
        internal_token,
        Some(config.data_dir.clone()),
        Some(cluster_signer.clone()),
    );
    // Wire the on-disk keystore path so `validate_join_token_ed25519`
    // (Wave 5A.3) and `cluster_rotate_signing_key` (Wave 5B.4) can look
    // up signers by `kid` and rotate the keystore on demand.
    cluster_state.cluster_signing_key_path = Some(config.data_dir.join("cluster_signing.key"));
    cluster_state.cluster_ca.clone_from(&cluster_ca);
    cluster_state.cluster_domain = Some(node_config.node_id.clone());
    let cluster_routes = build_cluster_routes(cluster_state);
    router = router.nest("/api/v1/cluster", cluster_routes);

    // Merge build routes (build_state was created earlier so WorkflowsState
    // can share the same BuildManager).
    let build_api_routes = build_routes().with_state(build_state);
    router = router.nest("/api/v1", build_api_routes);

    // Build the bridge-network (Docker-style `docker network create`) state.
    // When the `docker` feature is enabled AND the daemon can connect to a
    // Docker socket, we wire a `DockerBridgeNetworkRuntime` into the state so
    // `POST /api/v1/container-networks` actually creates real Docker
    // networks; otherwise the state stays in metadata-only mode and a
    // warning is logged the first time a handler would need the runtime.
    let bridge_network_state = build_bridge_network_state()
        .await
        .with_event_bus(event_bus.clone());
    let container_network_routes = build_container_network_routes(bridge_network_state.clone());
    router = router.nest("/api/v1/container-networks", container_network_routes);

    // Merge container lifecycle routes. The same `container_state` is used
    // for the daemon-wide event stream at `/api/v1/events` so lifecycle
    // handlers and event subscribers share the broadcast bus. The
    // bridge-network state is threaded in so `CreateContainerRequest.networks`
    // can attach freshly-created containers to user-defined networks.
    let container_state = ContainerApiState::with_daemon_uuid(runtime, daemon_uuid.clone())
        .with_shared_event_bus(event_bus.clone())
        .with_bridge_networks(bridge_network_state)
        .with_standalone_storage(bundle.standalone_containers.clone())
        .with_compose_storage(bundle.compose_projects.clone());

    // Repopulate the in-memory standalone-container cache from disk so
    // create / list / inspect / delete handlers see records that survived a
    // daemon restart. A failure here is fatal — running with a half-empty
    // cache would let a follow-up `delete` succeed at the runtime layer
    // while a stale row sat in the database.
    container_state
        .repopulate_cache_from_storage()
        .await
        .context("Failed to repopulate standalone-container cache from storage")?;

    // Reconcile the standalone-container storage against the runtime listing
    // exactly once at boot. This re-registers ContainerIdMap rows for entries
    // that survived restart and prunes rows whose runtime bundles have been
    // removed out-of-band. Failures here are non-fatal — log and continue —
    // because a transient runtime hiccup at boot must not stop the daemon
    // from serving (the next reconcile or REST call will re-surface issues).
    match reconcile_standalone_containers(&container_state).await {
        Ok(report) => {
            info!(
                matched = report.matched,
                pruned = report.pruned,
                orphans_seen = report.orphans_seen,
                "standalone-container reconcile complete",
            );
        }
        Err(e) => {
            warn!(error = %e, "standalone-container reconcile failed; continuing boot");
        }
    }

    // Spawn the daemon-side `--rm` (auto-remove) subscriber. The returned
    // JoinHandle is held for the lifetime of the daemon — dropping it would
    // cancel the task and leak `--rm` containers on exit. The handle is
    // aborted explicitly during the post-shutdown cleanup phase below.
    let auto_remove_handle = start_auto_remove_subscriber(container_state.clone());

    let container_routes = build_container_routes(container_state.clone());
    router = router.nest("/api/v1/containers", container_routes);

    let event_routes = build_event_routes(container_state);
    router = router.nest("/api/v1/events", event_routes);

    // Merge volume management routes
    let volume_dir = config.data_dir.join("volumes");
    let volume_state = VolumeApiState::new(volume_dir).with_event_bus(event_bus.clone());
    let volume_routes = build_volume_routes(volume_state);
    router = router.nest("/api/v1/volumes", volume_routes);

    let job_state = JobState {
        executor: job_executor,
    };
    router = router.nest("/api/v1/jobs", build_job_routes(job_state));

    let cron_state = CronState {
        scheduler: cron_scheduler,
    };
    router = router.nest("/api/v1/cron", build_cron_routes(cron_state));

    // Re-apply the auth extension layer AFTER all .nest() calls so that
    // routes added after the base router (containers, jobs, cron, build, etc.)
    // also receive the AuthState extension. In Axum, layers applied before
    // .nest() do not propagate to routes nested afterwards.
    let auth_state = zlayer_api::AuthState {
        jwt_secret: api_config.jwt_secret.clone(),
        credential_store: api_config.credential_store.clone(),
        user_store: api_config.user_store.clone(),
        identity: api_config.identity.clone(),
        oidc_clients: api_config.oidc_clients.clone(),
        oidc_state: api_config.oidc_state.clone(),
        cookie_secure: false,
    };
    router = router.layer(zlayer_api::Extension(auth_state));

    info!(
        bind = %bind_addr,
        socket = %socket_path,
        swagger = api_config.swagger_enabled,
        "Starting ZLayer daemon API server"
    );

    // -----------------------------------------------------------------------
    // 5. Setup graceful shutdown signal handler
    //
    // Awaits any of: Ctrl+C, SIGTERM (Unix), or the optional
    // `external_shutdown` watch receiver transitioning to `true` (set by the
    // Windows Service SCM handler — see `daemon_service::my_service_main`).
    // -----------------------------------------------------------------------
    let shutdown = async move {
        let ctrl_c = async {
            tokio::signal::ctrl_c()
                .await
                .expect("Failed to install Ctrl+C handler");
        };

        #[cfg(unix)]
        let terminate = async {
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("Failed to install SIGTERM handler")
                .recv()
                .await;
        };

        #[cfg(not(unix))]
        let terminate = std::future::pending::<()>();

        let external = async move {
            match external_shutdown {
                Some(mut rx) => {
                    // Returns once the watched value flips to `true`. If the
                    // sender is dropped, the receiver yields Err(_) which we
                    // treat as a shutdown (the supervisor is gone).
                    let _ = rx.wait_for(|&v| v).await;
                }
                None => std::future::pending::<()>().await,
            }
        };

        tokio::select! {
            () = ctrl_c => info!("Shutdown: Ctrl+C received"),
            () = terminate => info!("Shutdown: SIGTERM received"),
            () = external => info!("Shutdown: external trigger (SCM)"),
        }

        info!("Shutdown signal received, starting graceful shutdown");
    };

    // -----------------------------------------------------------------------
    // 6. Signal readiness to systemd (Type=notify) and run API server
    // -----------------------------------------------------------------------
    // sd_notify fires right before the server binds its sockets.  With
    // Type=notify, `systemctl start` blocks until this point — all init
    // (overlay, raft, storage, etc.) is complete.  The socket bind itself
    // takes microseconds, so the CLI's 500 ms socket-retry poll always
    // finds it on the next tick.
    #[cfg(target_os = "linux")]
    {
        let _ = sd_notify::notify(false, &[sd_notify::NotifyState::Ready]);
    }

    zlayer_api::serve_bound(bound_listeners, router, shutdown).await?;

    // -----------------------------------------------------------------------
    // 7. Post-shutdown cleanup: tear down infrastructure in reverse order
    // -----------------------------------------------------------------------
    info!("API server stopped, shutting down daemon infrastructure");

    // Flush SQLite replicator before tearing down infrastructure
    if let Some(ref replicator) = replicator {
        if let Err(e) = replicator.flush().await {
            warn!("Failed to flush SQLite replicator: {e}");
        }
    }

    // Stop heartbeat / dead-node detection background tasks
    if let Some(handle) = heartbeat_handle {
        handle.abort();
        let _ = handle.await;
    }
    if let Some(handle) = dead_node_detection_handle {
        handle.abort();
        let _ = handle.await;
    }
    info!("Heartbeat / dead-node detection stopped");

    // Stop the standalone `--rm` auto-remove subscriber. It would otherwise
    // exit on its own when the event bus is dropped, but aborting + awaiting
    // here ties its lifetime cleanly to the shutdown path so we don't leave a
    // task running while later cleanup steps tear down shared state.
    auto_remove_handle.abort();
    let _ = auto_remove_handle.await;
    info!("Auto-remove subscriber stopped");

    // Stop Raft RPC server
    if let Some(handle) = raft_server_handle {
        handle.abort();
        let _ = handle.await;
    }
    // Shut down Raft coordinator
    if let Some(raft) = _raft {
        if let Err(e) = raft.shutdown().await {
            warn!("Raft shutdown error: {e}");
        }
    }
    info!("Raft coordinator stopped");

    // Stop health checker
    if let Some(handle) = health_checker_handle {
        handle.abort();
        let _ = handle.await;
    }
    info!("Stream health checker stopped");

    // Stop NAT maintenance task (STUN refresh / relay upgrade loop)
    if let Some(handle) = nat_maintenance_handle {
        handle.abort();
        let _ = handle.await;
    }
    info!("NAT maintenance task stopped");

    // Stop tunnel server (WebSocket accept loop + active sessions)
    if let Some(handles) = tunnel_handles {
        handles.accept_handle.abort();
        let _ = handles.accept_handle.await;
        handles.access_manager.shutdown();
        handles.node_manager.shutdown();
        info!("Tunnel server stopped");
    }

    // Stop log rotator
    if let Some(handle) = log_rotator_handle {
        handle.abort();
        let _ = handle.await;
    }
    info!("Log rotator stopped");

    // Stop cluster signing keystore pruner (Wave 5A.5)
    if let Some(handle) = keystore_pruner_handle {
        handle.abort();
        let _ = handle.await;
    }
    info!("Cluster signing keystore pruner stopped");

    // Stop container supervisor
    supervisor.shutdown();
    if let Some(handle) = supervisor_handle {
        let _ = handle.await;
    }
    info!("Container supervisor stopped");

    // Stop proxy
    proxy.stop().await;
    info!("Proxy manager stopped");

    // Stop DNS server: dropping the DnsHandle and DnsServer Arc stops the
    // background UDP listener task (it is cancelled when the last Arc is dropped).
    drop(dns_handle);
    drop(dns);
    info!("DNS server stopped");

    // Cleanup overlay networks
    if let Some(om) = overlay {
        let mut om_guard = om.write().await;
        if let Err(e) = om_guard.cleanup().await {
            warn!("Failed to cleanup overlay networks: {e}");
        } else {
            info!("Overlay networks cleaned up");
        }
    }

    // Clean up daemon metadata file
    let _ = tokio::fs::remove_file(&metadata_path).await;
    info!(path = %metadata_path.display(), "Daemon metadata cleaned up");

    // Clean up PID file
    let pid_file = config.run_dir.join("zlayer.pid");
    let _ = tokio::fs::remove_file(&pid_file).await;

    info!("Daemon shutdown complete");

    if take_pending_restart_on_exit() {
        // Per the CLAUDE.md note: there is no in-tree supervisor.
        // Exit code 75 (EX_TEMPFAIL) signals "transient failure, please retry"
        // so systemd `Restart=on-failure` (the conventional config) respawns us.
        // Without a supervisor, this becomes a clean exit that the operator
        // can re-wrap.
        tracing::info!("--restart-on-exit set; exiting with code 75 for supervisor respawn");
        std::process::exit(75);
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Persistent API storage bundle
// ---------------------------------------------------------------------------

/// Aggregate handle for every persistent API store the daemon needs beyond
/// the deployment database.
///
/// [`DaemonState::storage`] (the deployment `SqlxStorage`) is still opened
/// inside `init_daemon` because the S3 `SqliteReplicator` is bound to its
/// exact `{data_dir}/deployments.db` path and the restore path depends on
/// the concrete `Arc<SqlxStorage>` type. Every *other* persistent store flows
/// through this bundle so serve.rs has a single source of truth.
///
/// When a `data_dir` is provided, each field is backed by a `SQLite` file at
/// `{data_dir}/<name>.db`, matching the one-file-per-store layout used for
/// `deployments.db`. When `data_dir` is `None`, every store falls back to an
/// in-memory `SQLite` database — used by tests and any hypothetical ephemeral
/// mode. No bare `InMemory*::new()` is used in the production path: the
/// fallback still exercises the sqlx code path (just with `:memory:`), which
/// keeps schema and query coverage identical.
pub(crate) struct StorageBundle {
    pub users: std::sync::Arc<dyn zlayer_api::UserStorage>,
    pub environments: std::sync::Arc<dyn zlayer_api::EnvironmentStorage>,
    pub projects: std::sync::Arc<dyn zlayer_api::ProjectStorage>,
    pub variables: std::sync::Arc<dyn zlayer_api::VariableStorage>,
    pub tasks: std::sync::Arc<dyn zlayer_api::TaskStorage>,
    pub workflows: std::sync::Arc<dyn zlayer_api::WorkflowStorage>,
    pub notifiers: std::sync::Arc<dyn zlayer_api::NotifierStorage>,
    pub groups: std::sync::Arc<dyn zlayer_api::GroupStorage>,
    pub permissions: std::sync::Arc<dyn zlayer_api::PermissionStorage>,
    pub audit: std::sync::Arc<dyn zlayer_api::AuditStorage>,
    pub syncs: std::sync::Arc<dyn zlayer_api::SyncStorage>,
    pub oidc_identities: std::sync::Arc<dyn zlayer_api::OidcIdentityStorage>,
    pub standalone_containers: std::sync::Arc<dyn zlayer_api::StandaloneContainerStorage>,
    pub compose_projects: std::sync::Arc<dyn zlayer_api::ComposeProjectStorage>,
}

impl StorageBundle {
    /// Open every persistent API store rooted at `data_dir`, or fall back to
    /// per-store `:memory:` `SQLite` databases when `data_dir` is `None`.
    ///
    /// # Errors
    ///
    /// Returns an error if any individual store fails to open. Failures are
    /// short-circuited (we'd rather fail fast than bring up a half-persistent
    /// daemon).
    #[allow(clippy::too_many_lines)]
    pub(crate) async fn open(data_dir: Option<&std::path::Path>) -> Result<Self> {
        if let Some(dir) = data_dir {
            tokio::fs::create_dir_all(dir)
                .await
                .with_context(|| format!("Failed to create storage data dir: {}", dir.display()))?;

            let users = zlayer_api::SqlxUserStore::open(dir.join("users.db"))
                .await
                .context("Failed to open users.db")?;
            let environments = zlayer_api::SqlxEnvironmentStore::open(dir.join("environments.db"))
                .await
                .context("Failed to open environments.db")?;
            let projects = zlayer_api::SqlxProjectStore::open(dir.join("projects.db"))
                .await
                .context("Failed to open projects.db")?;
            let variables = zlayer_api::SqlxVariableStore::open(dir.join("variables.db"))
                .await
                .context("Failed to open variables.db")?;
            let tasks = zlayer_api::SqlxTaskStore::open(dir.join("tasks.db"))
                .await
                .context("Failed to open tasks.db")?;
            let workflows = zlayer_api::SqlxWorkflowStore::open(dir.join("workflows.db"))
                .await
                .context("Failed to open workflows.db")?;
            let notifiers = zlayer_api::SqlxNotifierStore::open(dir.join("notifiers.db"))
                .await
                .context("Failed to open notifiers.db")?;
            let groups = zlayer_api::SqlxGroupStore::open(dir.join("groups.db"))
                .await
                .context("Failed to open groups.db")?;
            let permissions = zlayer_api::SqlxPermissionStore::open(dir.join("permissions.db"))
                .await
                .context("Failed to open permissions.db")?;
            let audit = zlayer_api::SqlxAuditStore::open(dir.join("audit.db"))
                .await
                .context("Failed to open audit.db")?;
            let syncs = zlayer_api::SqlxSyncStore::open(dir.join("syncs.db"))
                .await
                .context("Failed to open syncs.db")?;
            let oidc_identities =
                zlayer_api::SqlxOidcIdentityStore::open(dir.join("oidc_identities.db"))
                    .await
                    .context("Failed to open oidc_identities.db")?;
            let standalone_containers = zlayer_api::SqliteStandaloneContainerStorage::open(
                dir.join("standalone_containers.sqlite"),
            )
            .await
            .context("Failed to open standalone_containers.sqlite")?;
            let compose_projects =
                zlayer_api::SqliteComposeProjectStorage::open(dir.join("compose_projects.sqlite"))
                    .await
                    .context("Failed to open compose_projects.sqlite")?;

            info!(
                dir = %dir.display(),
                "Opened persistent API stores (14 databases)"
            );

            Ok(Self {
                users: std::sync::Arc::new(users),
                environments: std::sync::Arc::new(environments),
                projects: std::sync::Arc::new(projects),
                variables: std::sync::Arc::new(variables),
                tasks: std::sync::Arc::new(tasks),
                workflows: std::sync::Arc::new(workflows),
                notifiers: std::sync::Arc::new(notifiers),
                groups: std::sync::Arc::new(groups),
                permissions: std::sync::Arc::new(permissions),
                audit: std::sync::Arc::new(audit),
                syncs: std::sync::Arc::new(syncs),
                oidc_identities: std::sync::Arc::new(oidc_identities),
                standalone_containers: std::sync::Arc::new(standalone_containers),
                compose_projects: std::sync::Arc::new(compose_projects),
            })
        } else {
            let users = zlayer_api::SqlxUserStore::in_memory()
                .await
                .context("Failed to create in-memory users store")?;
            let environments = zlayer_api::SqlxEnvironmentStore::in_memory()
                .await
                .context("Failed to create in-memory environments store")?;
            let projects = zlayer_api::SqlxProjectStore::in_memory()
                .await
                .context("Failed to create in-memory projects store")?;
            let variables = zlayer_api::SqlxVariableStore::in_memory()
                .await
                .context("Failed to create in-memory variables store")?;
            let tasks = zlayer_api::SqlxTaskStore::in_memory()
                .await
                .context("Failed to create in-memory tasks store")?;
            let workflows = zlayer_api::SqlxWorkflowStore::in_memory()
                .await
                .context("Failed to create in-memory workflows store")?;
            let notifiers = zlayer_api::SqlxNotifierStore::in_memory()
                .await
                .context("Failed to create in-memory notifiers store")?;
            let groups = zlayer_api::SqlxGroupStore::in_memory()
                .await
                .context("Failed to create in-memory groups store")?;
            let permissions = zlayer_api::SqlxPermissionStore::in_memory()
                .await
                .context("Failed to create in-memory permissions store")?;
            let audit = zlayer_api::SqlxAuditStore::in_memory()
                .await
                .context("Failed to create in-memory audit store")?;
            let syncs = zlayer_api::SqlxSyncStore::in_memory()
                .await
                .context("Failed to create in-memory syncs store")?;
            let oidc_identities = zlayer_api::SqlxOidcIdentityStore::in_memory()
                .await
                .context("Failed to create in-memory oidc_identities store")?;
            let standalone_containers = zlayer_api::SqliteStandaloneContainerStorage::in_memory()
                .await
                .context("Failed to create in-memory standalone_containers store")?;
            let compose_projects = zlayer_api::SqliteComposeProjectStorage::in_memory()
                .await
                .context("Failed to create in-memory compose_projects store")?;

            info!("Opened in-memory API stores (ephemeral / test mode)");

            Ok(Self {
                users: std::sync::Arc::new(users),
                environments: std::sync::Arc::new(environments),
                projects: std::sync::Arc::new(projects),
                variables: std::sync::Arc::new(variables),
                tasks: std::sync::Arc::new(tasks),
                workflows: std::sync::Arc::new(workflows),
                notifiers: std::sync::Arc::new(notifiers),
                groups: std::sync::Arc::new(groups),
                permissions: std::sync::Arc::new(permissions),
                audit: std::sync::Arc::new(audit),
                syncs: std::sync::Arc::new(syncs),
                oidc_identities: std::sync::Arc::new(oidc_identities),
                standalone_containers: std::sync::Arc::new(standalone_containers),
                compose_projects: std::sync::Arc::new(compose_projects),
            })
        }
    }
}

/// Build the bridge-network ([`BridgeNetworkApiState`]) with a Docker-backed
/// runtime attached when possible.
///
/// When the `docker` feature is enabled and `bollard` can reach the local
/// Docker daemon, we wrap a [`DockerBridgeNetworkRuntime`] into the state so
/// `POST /api/v1/container-networks` and the `networks` field on
/// `CreateContainerRequest` actually create / attach real Docker networks.
///
/// When the feature is disabled, or Docker is unavailable at startup, the
/// state falls back to metadata-only mode (the in-memory registry still
/// tracks network specs and attachments, but nothing happens on the
/// container-runtime side). The `BridgeNetworkApiState` logs a single
/// warning on the first handler call in that mode.
#[cfg_attr(not(feature = "docker"), allow(clippy::unused_async))]
async fn build_bridge_network_state() -> BridgeNetworkApiState {
    let state = BridgeNetworkApiState::new();

    #[cfg(feature = "docker")]
    {
        match zlayer_api::DockerBridgeNetworkRuntime::connect_local().await {
            Ok(rt) => {
                info!("Docker bridge-network runtime attached");
                return state.with_runtime(std::sync::Arc::new(rt));
            }
            Err(e) => {
                warn!(
                    error = %e,
                    "Docker not reachable; /api/v1/container-networks runs in \
                     metadata-only mode"
                );
            }
        }
    }

    state
}

#[cfg(test)]
mod tests {
    use super::{container_reachable_api_url, read_daemon_metadata};
    use zlayer_paths::ZLayerDirs;

    #[test]
    fn read_daemon_metadata_missing_file_is_none() {
        let tmp = ZLayerDirs::system_default()
            .scratch_dir("serve-test-")
            .expect("tempdir");
        let path = tmp.path().join("does-not-exist.json");
        assert!(read_daemon_metadata(&path).is_none());
    }

    #[test]
    fn read_daemon_metadata_malformed_is_none() {
        let tmp = ZLayerDirs::system_default()
            .scratch_dir("serve-test-")
            .expect("tempdir");
        let path = tmp.path().join("daemon.json");
        std::fs::write(&path, "this is not json {{}").expect("write");
        assert!(read_daemon_metadata(&path).is_none());
    }

    #[test]
    fn read_daemon_metadata_valid_returns_pid() {
        let tmp = ZLayerDirs::system_default()
            .scratch_dir("serve-test-")
            .expect("tempdir");
        let path = tmp.path().join("daemon.json");
        std::fs::write(
            &path,
            r#"{"pid":4242,"started_at":"x","api_bind":"0.0.0.0:3669","socket_path":"","host_network":false,"overlay_cidr":"10.0.0.0/16"}"#,
        )
        .expect("write");
        let meta = read_daemon_metadata(&path).expect("parsed");
        assert_eq!(meta.pid, 4242);
        assert_eq!(meta.api_bind.as_deref(), Some("0.0.0.0:3669"));
    }

    #[test]
    fn read_daemon_metadata_tolerates_missing_api_bind() {
        // The full DaemonMetadata writer always writes `api_bind`, but
        // StaleDaemonMeta marks it `#[serde(default)]` to tolerate older
        // formats. Verify that path explicitly so a future tightening of the
        // schema doesn't silently break stale-daemon detection.
        let tmp = ZLayerDirs::system_default()
            .scratch_dir("serve-test-")
            .expect("tempdir");
        let path = tmp.path().join("daemon.json");
        std::fs::write(&path, r#"{"pid":9}"#).expect("write");
        let meta = read_daemon_metadata(&path).expect("parsed");
        assert_eq!(meta.pid, 9);
        assert!(meta.api_bind.is_none());
    }

    #[test]
    fn wildcard_v4_bind_becomes_loopback() {
        let addr = "0.0.0.0:3669".parse().unwrap();
        assert_eq!(container_reachable_api_url(&addr), "http://127.0.0.1:3669");
    }

    #[test]
    fn wildcard_v6_bind_becomes_loopback() {
        let addr = "[::]:3669".parse().unwrap();
        assert_eq!(container_reachable_api_url(&addr), "http://[::1]:3669");
    }

    #[test]
    fn specific_v4_bind_is_preserved() {
        let addr = "10.0.0.5:3669".parse().unwrap();
        assert_eq!(container_reachable_api_url(&addr), "http://10.0.0.5:3669");
    }

    #[test]
    fn specific_v6_bind_is_bracketed() {
        let addr = "[2001:db8::1]:3669".parse().unwrap();
        assert_eq!(
            container_reachable_api_url(&addr),
            "http://[2001:db8::1]:3669"
        );
    }

    // -------------------------------------------------------------------
    // StorageBundle — restart persistence
    // -------------------------------------------------------------------

    /// Round-trip every bundle field through a fresh `TempDir`: write one
    /// record, drop the bundle, reopen at the same path, and confirm each
    /// record survived. This is the high-signal proof that `StorageBundle`
    /// actually hit the disk for every store — an in-memory fallback or an
    /// accidentally-rebuilt `InMemory*` would fail this test.
    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn storage_bundle_survives_restart() {
        use zlayer_api::{
            NotifierConfig, NotifierKind, PermissionLevel, StoredEnvironment, StoredNotifier,
            StoredPermission, StoredProject, StoredSync, StoredTask, StoredUser, StoredUserGroup,
            StoredVariable, StoredWorkflow, SubjectKind, TaskKind, UserRole, WorkflowAction,
            WorkflowStep,
        };

        let tmp = ZLayerDirs::system_default()
            .scratch_dir("serve-test-")
            .expect("tempdir");
        let dir = tmp.path().to_path_buf();

        // ---- first run: open bundle, write one record per store ----
        let user = StoredUser::new("alice@example.com", "Alice", UserRole::Admin);
        let env = StoredEnvironment::new("dev", None);
        let project = StoredProject::new("demo");
        let variable = StoredVariable::new("APP_VERSION", "1.2.3", None);
        let task = StoredTask::new("smoke", TaskKind::Bash, "echo hi", None);
        let workflow = StoredWorkflow::new(
            "release",
            vec![WorkflowStep {
                name: "run-smoke".into(),
                action: WorkflowAction::RunTask {
                    task_id: task.id.clone(),
                },
                on_failure: None,
            }],
        );
        let notifier = StoredNotifier::new(
            "alerts",
            NotifierKind::Webhook,
            NotifierConfig::Webhook {
                url: "https://hooks.example.com/x".into(),
                method: None,
                headers: None,
            },
        );
        let group = StoredUserGroup::new("ops");
        let permission = StoredPermission::new(
            SubjectKind::User,
            user.id.clone(),
            "deployment",
            Some("prod".into()),
            PermissionLevel::Write,
        );
        let sync = StoredSync::new("infra", "manifests/");

        {
            let bundle = super::StorageBundle::open(Some(dir.as_path()))
                .await
                .expect("open bundle 1");

            bundle.users.store(&user).await.expect("write user");
            bundle
                .environments
                .store(&env)
                .await
                .expect("write environment");
            bundle
                .projects
                .store(&project)
                .await
                .expect("write project");
            bundle
                .variables
                .store(&variable)
                .await
                .expect("write variable");
            bundle.tasks.store(&task).await.expect("write task");
            bundle
                .workflows
                .store(&workflow)
                .await
                .expect("write workflow");
            bundle
                .notifiers
                .store(&notifier)
                .await
                .expect("write notifier");
            bundle.groups.store(&group).await.expect("write group");
            bundle
                .permissions
                .grant(&permission)
                .await
                .expect("grant permission");
            bundle
                .syncs
                .create(sync.clone())
                .await
                .expect("create sync");
        }

        // ---- second run: reopen at the same directory, assert all reads ----
        let bundle = super::StorageBundle::open(Some(dir.as_path()))
            .await
            .expect("open bundle 2");

        assert_eq!(
            bundle
                .users
                .get(&user.id)
                .await
                .expect("read user")
                .map(|u| u.id),
            Some(user.id.clone()),
            "user record did not survive restart"
        );
        assert_eq!(
            bundle
                .environments
                .get(&env.id)
                .await
                .expect("read environment")
                .map(|e| e.id),
            Some(env.id.clone()),
            "environment record did not survive restart"
        );
        assert_eq!(
            bundle
                .projects
                .get(&project.id)
                .await
                .expect("read project")
                .map(|p| p.id),
            Some(project.id.clone()),
            "project record did not survive restart"
        );
        assert_eq!(
            bundle
                .variables
                .get(&variable.id)
                .await
                .expect("read variable")
                .map(|v| v.id),
            Some(variable.id.clone()),
            "variable record did not survive restart"
        );
        assert_eq!(
            bundle
                .tasks
                .get(&task.id)
                .await
                .expect("read task")
                .map(|t| t.id),
            Some(task.id.clone()),
            "task record did not survive restart"
        );
        assert_eq!(
            bundle
                .workflows
                .get(&workflow.id)
                .await
                .expect("read workflow")
                .map(|w| w.id),
            Some(workflow.id.clone()),
            "workflow record did not survive restart"
        );
        assert_eq!(
            bundle
                .notifiers
                .get(&notifier.id)
                .await
                .expect("read notifier")
                .map(|n| n.id),
            Some(notifier.id.clone()),
            "notifier record did not survive restart"
        );
        assert_eq!(
            bundle
                .groups
                .get(&group.id)
                .await
                .expect("read group")
                .map(|g| g.id),
            Some(group.id.clone()),
            "group record did not survive restart"
        );
        let perms = bundle
            .permissions
            .list_for_subject(SubjectKind::User, &user.id)
            .await
            .expect("list permissions");
        assert!(
            perms.iter().any(|p| p.id == permission.id),
            "permission record did not survive restart"
        );
        let syncs = bundle.syncs.list().await.expect("list syncs");
        assert!(
            syncs.iter().any(|s| s.id == sync.id),
            "sync record did not survive restart"
        );

        // audit is append-only and has a separate trait — query via list_filtered
        let audit_entry = zlayer_api::AuditEntry::new(&user.id, "create", "deployment");
        bundle
            .audit
            .record(&audit_entry)
            .await
            .expect("record audit");
        drop(bundle);

        let bundle2 = super::StorageBundle::open(Some(dir.as_path()))
            .await
            .expect("open bundle 3");
        let entries = bundle2
            .audit
            .list(zlayer_api::AuditFilter::default())
            .await
            .expect("list audit");
        assert!(
            entries.iter().any(|e| e.id == audit_entry.id),
            "audit entry did not survive restart"
        );
    }

    /// A lighter smoke test that just exercises the in-memory fallback,
    /// guaranteeing `StorageBundle::open(None)` stays working for
    /// test harnesses.
    #[tokio::test]
    async fn storage_bundle_in_memory_fallback_opens() {
        let bundle = super::StorageBundle::open(None)
            .await
            .expect("open in-memory bundle");
        assert_eq!(bundle.users.list().await.unwrap().len(), 0);
        assert_eq!(bundle.variables.list(None).await.unwrap().len(), 0);
    }
}
