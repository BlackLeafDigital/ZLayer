//! VHDX compaction for the `zlayer` WSL2 distro to reclaim freed space to the Windows host.
//!
//! WSL2's `ext4.vhdx` grows sparsely as the guest allocates blocks but never
//! shrinks on its own when files are deleted inside the distro. Over time
//! users end up with a multi-gigabyte vhdx backing a much smaller live
//! footprint. [`compact_distro`] gracefully stops the daemon, shuts the WSL
//! utility VM down, and invokes either Hyper-V's `Optimize-VHD` cmdlet (when
//! available) or `diskpart`'s `compact vdisk` command to shrink the file.

// The script builders are exercised by cross-platform tests; the Windows-only runtime helpers
// (stat_len, has_optimize_vhd, run_*) are cfg-gated and therefore dead on Linux lib builds.
#![allow(dead_code)]

use std::path::PathBuf;
#[cfg(target_os = "windows")]
use std::time::Duration;

/// Outcome of a [`compact_distro`] run, suitable for printing a human-readable
/// summary from the CLI.
#[derive(Debug, Clone)]
pub struct CompactReport {
    /// Absolute path to the vhdx that was compacted.
    pub vhdx_path: PathBuf,
    /// Size of the vhdx on disk before compaction, if stat succeeded.
    pub size_before_bytes: Option<u64>,
    /// Size of the vhdx on disk after compaction, if stat succeeded.
    pub size_after_bytes: Option<u64>,
    /// Which compaction backend actually ran.
    pub method: CompactMethod,
    /// Whether the daemon was observed running before we shut things down.
    pub daemon_was_running: bool,
}

/// Which backend actually performed the compaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactMethod {
    /// `diskpart compact vdisk` — primary backend, available on Win 11 Home.
    Diskpart,
    /// `Hyper-V`'s `Optimize-VHD` `PowerShell` cmdlet — preferred when present.
    OptimizeVhd,
}

/// Compact the ZLayer WSL2 distro's `ext4.vhdx`, reclaiming freed space to the host.
///
/// Flow:
///
/// 1. Probe the daemon's health; if it's up, send SIGTERM via
///    [`crate::daemon::stop_daemon`] and poll for up to 10 seconds.
/// 2. Run `wsl.exe --shutdown` to tear down the whole WSL utility VM so the
///    host filesystem can take the vhdx lock.
/// 3. Sleep ~2 seconds while `vmcompute.exe` releases the file handle.
/// 4. Stat the vhdx for `size_before_bytes` (best-effort).
/// 5. If Hyper-V's `Optimize-VHD` cmdlet is present, run it. Otherwise fall
///    back to a scripted `diskpart compact vdisk`.
/// 6. Stat again for `size_after_bytes`.
///
/// # Errors
///
/// Returns an error if the vhdx doesn't exist (no WSL2 distro has been set up
/// yet), if the chosen compaction tool exits non-zero, or if the host shell
/// helpers fail to spawn. Errors include the tool's stderr when relevant.
///
/// On non-Windows hosts this function always returns an error — VHDX
/// compaction is a Windows-only operation.
#[cfg(target_os = "windows")]
pub async fn compact_distro() -> anyhow::Result<CompactReport> {
    let vhdx = crate::paths::vhdx_path();

    if !vhdx.exists() {
        anyhow::bail!(
            "no WSL2 distro found to compact at {}; run `zlayer serve` first to create it",
            vhdx.display()
        );
    }

    // 1. Graceful daemon stop.
    let config = crate::daemon::WslBackendConfig::default();
    let daemon_was_running = crate::daemon::check_daemon_health(&config)
        .await
        .unwrap_or(false);

    if daemon_was_running {
        tracing::info!("Stopping daemon before compacting vhdx");
        if let Err(err) = crate::daemon::stop_daemon().await {
            tracing::warn!("stop_daemon failed ({err}); wsl --shutdown will force-kill it");
        }

        let poll_start = std::time::Instant::now();
        let poll_timeout = Duration::from_secs(10);
        loop {
            let healthy = crate::daemon::check_daemon_health(&config)
                .await
                .unwrap_or(false);
            if !healthy {
                tracing::info!("Daemon stopped gracefully");
                break;
            }
            if poll_start.elapsed() >= poll_timeout {
                tracing::warn!(
                    "Daemon still responsive after {poll_timeout:?}; continuing with wsl --shutdown"
                );
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    } else {
        tracing::info!("Daemon not running; skipping graceful stop");
    }

    // 2. Shut the WSL utility VM down so we get the vhdx lock.
    tracing::info!("Shutting down WSL");
    let shutdown = crate::shell::wsl_control(["--shutdown"]).await?;
    if !shutdown.status.success() {
        let stderr = String::from_utf8_lossy(&shutdown.stderr);
        anyhow::bail!("wsl.exe --shutdown failed: {stderr}");
    }

    // 3. Let vmcompute.exe release the handle.
    tokio::time::sleep(Duration::from_secs(2)).await;

    // 4. Stat before.
    let size_before_bytes = stat_len(&vhdx).await;

    // 5. Probe for Optimize-VHD and pick a backend.
    let vhdx_str = vhdx.to_string_lossy().into_owned();
    let method = if has_optimize_vhd().await {
        tracing::info!("Compacting via Optimize-VHD (Hyper-V)");
        run_optimize_vhd(&vhdx_str).await?;
        CompactMethod::OptimizeVhd
    } else {
        tracing::info!("Compacting via Diskpart (Optimize-VHD unavailable)");
        run_diskpart(&vhdx_str).await?;
        CompactMethod::Diskpart
    };

    // 6. Stat after.
    let size_after_bytes = stat_len(&vhdx).await;

    if let (Some(before), Some(after)) = (size_before_bytes, size_after_bytes) {
        let freed = before.saturating_sub(after);
        let freed_mib = freed / (1024 * 1024);
        tracing::info!("Compact complete: freed {freed_mib} MiB");
    } else {
        tracing::info!("Compact complete");
    }

    Ok(CompactReport {
        vhdx_path: vhdx,
        size_before_bytes,
        size_after_bytes,
        method,
        daemon_was_running,
    })
}

/// Compact the `ZLayer` WSL2 distro's `ext4.vhdx` (non-Windows stub).
///
/// # Errors
///
/// Always returns an error — VHDX compaction is only available on Windows.
#[cfg(not(target_os = "windows"))]
#[allow(clippy::unused_async)]
pub async fn compact_distro() -> anyhow::Result<CompactReport> {
    anyhow::bail!("VHDX compaction is only available on Windows")
}

/// Best-effort `metadata().len()`; returns `None` and logs a warning on error.
#[cfg(target_os = "windows")]
async fn stat_len(path: &std::path::Path) -> Option<u64> {
    match tokio::fs::metadata(path).await {
        Ok(meta) => Some(meta.len()),
        Err(err) => {
            tracing::warn!("stat {} failed: {err}", path.display());
            None
        }
    }
}

/// Returns `true` if PowerShell has the `Optimize-VHD` cmdlet registered
/// (i.e. the Hyper-V module is installed — typically Pro/Enterprise).
#[cfg(target_os = "windows")]
async fn has_optimize_vhd() -> bool {
    let script =
        "if (Get-Command Optimize-VHD -ErrorAction SilentlyContinue) { exit 0 } else { exit 1 }";
    match crate::shell::powershell(script).await {
        Ok(output) => output.status.success(),
        Err(err) => {
            tracing::warn!("Optimize-VHD probe failed ({err}); falling back to diskpart");
            false
        }
    }
}

/// Run `Optimize-VHD -Mode Full` against the given vhdx path.
#[cfg(target_os = "windows")]
async fn run_optimize_vhd(vhdx_path: &str) -> anyhow::Result<()> {
    let script = build_optimize_vhd_script(vhdx_path);
    let output = crate::shell::powershell(&script).await?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Optimize-VHD failed: {stderr}");
    }
    Ok(())
}

/// Run the scripted `diskpart` compaction sequence against the given vhdx path.
#[cfg(target_os = "windows")]
async fn run_diskpart(vhdx_path: &str) -> anyhow::Result<()> {
    let script = build_diskpart_script(vhdx_path);
    let output = crate::shell::diskpart_script(&script).await?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        anyhow::bail!("diskpart compact failed: stderr={stderr} stdout={stdout}");
    }
    Ok(())
}

/// Build the exact diskpart script to compact a vhdx at `path`.
///
/// `attach vdisk readonly` is required before `compact vdisk`; detaching
/// afterwards releases the file so subsequent WSL starts can claim it.
#[must_use]
fn build_diskpart_script(path: &str) -> String {
    let escaped = path.replace('"', "\"\"");
    format!(
        "select vdisk file=\"{escaped}\"\nattach vdisk readonly\ncompact vdisk\ndetach vdisk\nexit\n"
    )
}

/// Build the `PowerShell` script that runs `Optimize-VHD -Mode Full` against `path`.
#[must_use]
fn build_optimize_vhd_script(path: &str) -> String {
    let escaped = path.replace('"', "`\"");
    format!(
        "$ErrorActionPreference = \"Stop\"\n\
         if (-not (Get-Command Optimize-VHD -ErrorAction SilentlyContinue)) {{\n\
         \x20\x20Write-Error \"Optimize-VHD cmdlet not available\"\n\
         \x20\x20exit 1\n\
         }}\n\
         Optimize-VHD -Path \"{escaped}\" -Mode Full\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diskpart_script_contains_required_commands() {
        let script = build_diskpart_script(r"C:\Users\zach\AppData\Local\ZLayer\wsl\ext4.vhdx");
        assert!(script.contains(
            "select vdisk file=\"C:\\Users\\zach\\AppData\\Local\\ZLayer\\wsl\\ext4.vhdx\""
        ));
        assert!(script.contains("attach vdisk readonly"));
        assert!(script.contains("compact vdisk"));
        assert!(script.contains("detach vdisk"));
        assert!(script.trim_end().ends_with("exit"));
    }

    #[test]
    fn diskpart_script_handles_spaces_in_path() {
        let script = build_diskpart_script(r"C:\Users\Jane Doe\AppData\Local\ZLayer\wsl\ext4.vhdx");
        assert!(script.contains("\"C:\\Users\\Jane Doe\\AppData\\Local\\ZLayer\\wsl\\ext4.vhdx\""));
    }

    #[test]
    fn diskpart_script_escapes_embedded_quotes() {
        let script = build_diskpart_script(r#"C:\weird"path\ext4.vhdx"#);
        assert!(script.contains(r#""C:\weird""path\ext4.vhdx""#));
    }

    #[test]
    fn optimize_vhd_script_references_path_and_mode() {
        let script = build_optimize_vhd_script(r"C:\Users\zach\AppData\Local\ZLayer\wsl\ext4.vhdx");
        assert!(script.contains("Optimize-VHD"));
        assert!(script.contains("-Mode Full"));
        assert!(script.contains(r"C:\Users\zach\AppData\Local\ZLayer\wsl\ext4.vhdx"));
    }
}
