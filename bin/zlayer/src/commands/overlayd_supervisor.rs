//! Supervisor glue that guarantees the standalone `zlayer-overlayd` daemon is
//! running before the main daemon's `OverlayManager` (a thin overlayd client)
//! tries to talk to it.
//!
//! `zlayer-overlayd` is registered as its OWN OS service by
//! `zlayer daemon install` (see [`crate::commands::daemon`]), so on a normal
//! installed host it is already running and [`ensure_overlayd_running`] simply
//! confirms the socket answers. The interesting paths are:
//!
//!   1. **Connect** — if the overlayd IPC socket already accepts a connection,
//!      we are done.
//!   2. **Start the installed service** — on a host where the operator ran
//!      `zlayer daemon install` but the overlayd service happens to be down
//!      (crash, manual stop), kick it via the platform service manager
//!      (`systemctl start`, `launchctl kickstart`, `sc start`).
//!   3. **Spawn detached** — in development (`cargo run -- serve`) there is no
//!      installed service, so we locate the `zlayer-overlayd` binary next to
//!      the current executable (or the sibling Cargo target dir) and spawn it
//!      as a detached child with `--data-dir <data_dir>`.
//!
//! After any of (2)/(3) we poll [`OverlaydClient::connect_with_backoff`] until
//! the socket is reachable, returning a clear error on timeout so the daemon
//! surfaces a real failure instead of silently degrading the overlay.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tracing::info;
use zlayer_paths::ZLayerDirs;

/// SCM service name for the overlayd instance on Windows. Mirrors
/// `crate::daemon_service::service_name("zlayer-overlayd")` → `ZLayerDaemon-Overlayd`.
#[cfg(windows)]
fn overlayd_service_name() -> String {
    crate::daemon_service::service_name("zlayer-overlayd")
}

/// launchd label for the overlayd daemon on macOS.
#[cfg(target_os = "macos")]
const OVERLAYD_LAUNCHD_LABEL: &str = "com.zlayer.overlayd";

/// systemd unit name for the overlayd daemon on Linux.
#[cfg(target_os = "linux")]
const OVERLAYD_SYSTEMD_UNIT: &str = "zlayer-overlayd.service";

/// Resolve the `zlayer-overlayd` binary path.
///
/// Resolution order:
///   1. A sibling of the current executable (`<exe_dir>/zlayer-overlayd[.exe]`)
///      — this is how an installed host lays the two binaries out (the
///      installer copies overlayd into the same system bin dir as `zlayer`).
///   2. The system bin dir ([`ZLayerDirs::default_binary_dir`]).
///   3. The Cargo sibling target dir during development (the directory holding
///      the running `zlayer` dev binary already contains `zlayer-overlayd`
///      after `cargo build`, so (1) usually wins; this is a belt-and-braces
///      fallback for unusual layouts).
///   4. Bare `zlayer-overlayd`, relying on `PATH`.
fn overlayd_binary_path() -> PathBuf {
    let bin_name = if cfg!(windows) {
        "zlayer-overlayd.exe"
    } else {
        "zlayer-overlayd"
    };

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join(bin_name);
            if candidate.exists() {
                return candidate;
            }
        }
    }

    let system = ZLayerDirs::default_binary_dir().join(bin_name);
    if system.exists() {
        return system;
    }

    PathBuf::from(bin_name)
}

/// Ensure the standalone `zlayer-overlayd` daemon is running and reachable on
/// its IPC socket for the given `data_dir`.
///
/// Call this in `serve` BEFORE the `OverlayManager` is first constructed, and
/// only when the overlay is enabled (i.e. NOT `host_network`).
///
/// # Errors
///
/// Returns an error only when, after attempting to start the service and/or
/// spawn the binary, the overlayd socket still does not accept a connection
/// within the bounded backoff window.
#[cfg_attr(target_os = "macos", allow(unsafe_code))]
pub(crate) async fn ensure_overlayd_running(data_dir: &Path) -> Result<()> {
    let socket = ZLayerDirs::default_overlayd_socket_path_for(data_dir);
    let socket_path = PathBuf::from(&socket);

    // 1. Already up?
    if zlayer_overlayd::OverlaydClient::connect(&socket_path)
        .await
        .is_ok()
    {
        info!(socket = %socket, "overlayd already running");
        return Ok(());
    }

    info!(
        socket = %socket,
        "overlayd not reachable; attempting to start it"
    );

    // 2. Try the installed OS service first; if that doesn't pan out, spawn
    //    the binary detached. `start_installed_service` returns true only when
    //    it actually issued a start command that the service manager accepted.
    let started_service = start_installed_service().await;
    if !started_service {
        // On macOS the overlay utun device can ONLY be created by root, so a
        // user-spawned overlayd would just EPERM on the device and never own the
        // overlay. Don't spawn a doomed child — return cleanly and let the
        // daemon run with cross-node overlay networking disabled until the root
        // overlayd service is registered (`zlayer daemon install`, which
        // elevates exactly once when the overlay actually changes). This is the
        // "don't touch the overlay when we can't" path; the overlay only comes
        // up via the installed root service.
        #[cfg(target_os = "macos")]
        if unsafe { libc::geteuid() } != 0 {
            info!(
                socket = %socket,
                "overlayd system service not installed and not running as root; \
                 cross-node overlay networking disabled (run `zlayer daemon install` \
                 to register the root overlay service)"
            );
            return Ok(());
        }
        spawn_overlayd_detached(data_dir, &socket_path)
            .context("failed to spawn zlayer-overlayd binary")?;
    }

    // 3. Poll until reachable (bounded ~20 attempts, 100ms→1s backoff).
    match zlayer_overlayd::OverlaydClient::connect_with_backoff(&socket_path).await {
        Ok(_) => {
            info!(socket = %socket, "overlayd is up and reachable");
            Ok(())
        }
        Err(e) => Err(anyhow::anyhow!(
            "zlayer-overlayd did not become reachable on {socket} \
             (started_service={started_service}): {e}. \
             The overlay network owner is down — install it with \
             `zlayer daemon install` or check the overlayd service logs."
        )),
    }
}

/// Attempt to start the installed overlayd OS service.
///
/// Returns `true` when the platform service manager accepted a start request
/// (the service exists and was kicked), `false` when no installed service was
/// found (so the caller should fall back to spawning the binary directly).
#[cfg(target_os = "linux")]
async fn start_installed_service() -> bool {
    use tokio::process::Command;

    // `systemctl start` returns non-zero when the unit doesn't exist; treat
    // that as "not installed" and let the caller spawn the binary instead.
    match Command::new("systemctl")
        .args(["start", OVERLAYD_SYSTEMD_UNIT])
        .output()
        .await
    {
        Ok(out) if out.status.success() => {
            info!(unit = OVERLAYD_SYSTEMD_UNIT, "started overlayd via systemd");
            true
        }
        Ok(_) | Err(_) => false,
    }
}

/// macOS: kickstart / bootstrap the `com.zlayer.overlayd` launchd job.
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
async fn start_installed_service() -> bool {
    use tokio::process::Command;

    let is_root = unsafe { libc::geteuid() } == 0;
    let uid = unsafe { libc::getuid() };
    let target = if is_root {
        "system".to_string()
    } else {
        format!("gui/{uid}")
    };

    // `launchctl kickstart` only works on an already-loaded job. A successful
    // exit means the job existed and was (re)started.
    match Command::new("launchctl")
        .args([
            "kickstart",
            "-k",
            &format!("{target}/{OVERLAYD_LAUNCHD_LABEL}"),
        ])
        .output()
        .await
    {
        Ok(out) if out.status.success() => {
            info!(
                label = OVERLAYD_LAUNCHD_LABEL,
                "started overlayd via launchd"
            );
            true
        }
        Ok(_) | Err(_) => false,
    }
}

/// Windows: `sc start ZLayerDaemon-Overlayd`.
#[cfg(windows)]
async fn start_installed_service() -> bool {
    use tokio::process::Command;

    let svc = overlayd_service_name();
    match Command::new("sc.exe").args(["start", &svc]).output().await {
        Ok(out) => {
            // `sc start` returns success when it kicks the service, and a
            // specific error (1056 already running / 1060 not installed)
            // otherwise. Already-running is fine; not-installed → spawn.
            if out.status.success() {
                info!(service = %svc, "started overlayd via SCM");
                true
            } else {
                let stdout = String::from_utf8_lossy(&out.stdout);
                // 1056 = ERROR_SERVICE_ALREADY_RUNNING.
                if stdout.contains("1056") {
                    info!(service = %svc, "overlayd service already running");
                    true
                } else {
                    false
                }
            }
        }
        Err(_) => false,
    }
}

/// Spawn `zlayer-overlayd --data-dir <data_dir>` as a detached child so it
/// outlives the spawning daemon process.
fn spawn_overlayd_detached(data_dir: &Path, socket_path: &Path) -> Result<()> {
    let bin = overlayd_binary_path();
    info!(
        binary = %bin.display(),
        data_dir = %data_dir.display(),
        socket = %socket_path.display(),
        "spawning detached zlayer-overlayd"
    );

    let mut cmd = std::process::Command::new(&bin);
    cmd.arg("--data-dir").arg(data_dir);
    // Pass the resolved socket explicitly so a custom data dir's socket is used
    // even if the overlayd binary's defaulting logic ever diverges.
    cmd.arg("--socket").arg(socket_path);

    // Detach: drop a controlling stdio so the child isn't tied to our TTY and
    // keeps running across our exit.
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::null());
    cmd.stderr(std::process::Stdio::null());

    // On Unix, start a new session so the child survives the parent and any
    // session-wide signals (e.g. terminal hangup).
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: setsid() in the pre-exec hook detaches the child into its own
        // session; it touches no parent state and is async-signal-safe.
        #[allow(unsafe_code)]
        unsafe {
            cmd.pre_exec(|| {
                // Best-effort: if setsid fails (already a session leader), the
                // child still runs — don't abort the exec.
                let _ = libc::setsid();
                Ok(())
            });
        }
    }

    // On Windows, DETACHED_PROCESS so the child has no console and isn't
    // killed when the parent's console closes.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        cmd.creation_flags(DETACHED_PROCESS);
    }

    let child = cmd
        .spawn()
        .with_context(|| format!("failed to spawn {}", bin.display()))?;

    // We deliberately do not wait on the child; it is a long-lived detached
    // daemon. Leak the handle (drop without wait) — on Unix this does not
    // reap, but overlayd is expected to outlive us, and the installed-service
    // path is the production norm.
    let pid = child.id();
    info!(pid, "spawned detached zlayer-overlayd");
    drop(child);

    Ok(())
}

/// Non-Linux/macOS/Windows fallback (none in practice — all three are covered
/// above). Kept so the cfg matrix is total and the function always exists.
#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
async fn start_installed_service() -> bool {
    false
}
