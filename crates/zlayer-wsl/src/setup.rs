//! First-time setup wizard for the WSL2 backend.
//!
//! Handles detecting WSL2, creating the dedicated distro,
//! and installing the `ZLayer` binary inside it.

use super::daemon::WslBackendConfig;

/// Ensure the WSL2 backend is fully configured and ready.
///
/// This is the main entry point called on Windows when the daemon
/// needs to be started. It handles:
/// 1. Checking WSL2 is installed
/// 2. Creating the `zlayer` distro if missing
/// 3. Verifying the ZLayer binary is installed inside the distro
///
/// # Errors
///
/// Returns an error if WSL2 is not available or setup fails.
#[cfg(target_os = "windows")]
pub async fn ensure_wsl_backend_ready() -> anyhow::Result<WslBackendConfig> {
    use super::{detect, distro};

    let status = detect::detect_wsl().await?;

    if !status.wsl_installed {
        anyhow::bail!(
            "WSL2 is not installed.\n\
             Install it by running (as Administrator):\n\
             \n  wsl.exe --install --no-distribution\n\
             \nThen restart your computer and run zlayer again."
        );
    }

    if !status.wsl2_available {
        anyhow::bail!(
            "WSL is installed but WSL2 is not available.\n\
             Enable WSL2:\n\
             \n  wsl.exe --set-default-version 2\n"
        );
    }

    // Ensure .wslconfig has a generous vhdSize cap + sparseVhd=true so the backing
    // ext4.vhdx can grow and reclaim space. Idempotent — no-ops if already correct.
    let install_dir = super::paths::install_dir();
    let min_gb = super::wslconfig::compute_default_gb(&install_dir);
    match super::wslconfig::ensure_wslconfig(min_gb).await {
        Ok(outcome) => {
            if outcome.changed {
                tracing::info!(
                    path = %outcome.path.display(),
                    previous_cap_gb = ?outcome.previous_cap_gb,
                    new_cap_gb = outcome.new_cap_gb,
                    "wrote .wslconfig with vhdSize cap"
                );
            }
        }
        Err(err) => {
            tracing::warn!(
                "failed to ensure .wslconfig; WSL2 will use its default vhdSize: {err:#}"
            );
        }
    }

    if !status.zlayer_distro_exists {
        tracing::info!("ZLayer WSL2 distro not found, creating...");
        setup_distro().await?;
    }

    // Verify binary exists inside distro
    let check = distro::wsl_exec("test", &["-x", "/usr/local/bin/zlayer"]).await?;
    if !check.status.success() {
        tracing::info!("ZLayer binary not found in WSL2 distro, installing...");
        install_binary().await?;
    }

    Ok(WslBackendConfig::default())
}

/// Ensure the WSL2 backend is ready (non-Windows stub).
///
/// # Errors
///
/// Always returns an error on non-Windows platforms.
#[cfg(not(target_os = "windows"))]
#[allow(clippy::unused_async)]
pub async fn ensure_wsl_backend_ready() -> anyhow::Result<WslBackendConfig> {
    anyhow::bail!("WSL2 setup is only available on Windows")
}

/// Same as [`ensure_wsl_backend_ready`] but lets the caller override the VHD cap.
///
/// Passing `Some(gb)` forces that cap (still floored at 64 GiB inside
/// `ensure_wslconfig`). Passing `None` uses [`crate::wslconfig::compute_default_gb`].
///
/// # Errors
///
/// Same failure modes as [`ensure_wsl_backend_ready`].
#[cfg(target_os = "windows")]
pub async fn ensure_wsl_backend_ready_with_vhd_gb(
    override_gb: Option<u64>,
) -> anyhow::Result<WslBackendConfig> {
    // Temporary env override so compute_default_gb picks it up without
    // restructuring the existing function.
    if let Some(gb) = override_gb {
        // SAFETY: env mutation is single-threaded during startup init.
        std::env::set_var("ZLAYER_WSL_VHD_GB", gb.to_string());
    }
    ensure_wsl_backend_ready().await
}

/// Non-Windows stub for [`ensure_wsl_backend_ready_with_vhd_gb`].
///
/// # Errors
///
/// Always returns an error on non-Windows platforms.
#[cfg(not(target_os = "windows"))]
#[allow(clippy::unused_async)]
pub async fn ensure_wsl_backend_ready_with_vhd_gb(
    _override_gb: Option<u64>,
) -> anyhow::Result<WslBackendConfig> {
    anyhow::bail!("WSL2 setup is only available on Windows")
}

/// Create the ZLayer WSL2 distro with a minimal Alpine rootfs.
#[cfg(target_os = "windows")]
async fn setup_distro() -> anyhow::Result<()> {
    use std::path::{Path, PathBuf};

    let install_dir = super::paths::install_dir();

    // Look for bundled rootfs alongside the executable
    let exe_dir = std::env::current_exe()?
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    let rootfs_path = exe_dir.join("zlayer-rootfs.tar.gz");
    if !rootfs_path.exists() {
        anyhow::bail!(
            "ZLayer rootfs not found at {}.\n\
             Place zlayer-rootfs.tar.gz alongside zlayer.exe or download it from releases.",
            rootfs_path.display()
        );
    }

    super::distro::create_distro(&rootfs_path, &install_dir).await?;
    install_binary().await?;

    Ok(())
}

/// Install the ZLayer Linux binary into the WSL2 distro.
#[cfg(target_os = "windows")]
async fn install_binary() -> anyhow::Result<()> {
    use std::path::{Path, PathBuf};

    let exe_dir = std::env::current_exe()?
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    let linux_binary = exe_dir.join("zlayer-linux");
    if !linux_binary.exists() {
        anyhow::bail!(
            "ZLayer Linux binary not found at {}.\n\
             Place zlayer-linux alongside zlayer.exe or download it from releases.",
            linux_binary.display()
        );
    }

    // Copy binary into WSL2 distro via Windows path translation
    let wsl_source = super::paths::windows_to_wsl(&linux_binary)
        .ok_or_else(|| anyhow::anyhow!("Failed to translate path: {}", linux_binary.display()))?;

    let install_cmd =
        format!("cp '{wsl_source}' /usr/local/bin/zlayer && chmod +x /usr/local/bin/zlayer");

    let output = super::distro::wsl_exec("sh", &["-c", &install_cmd]).await?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Failed to install ZLayer binary in WSL2: {stderr}");
    }

    tracing::info!("Installed ZLayer binary into WSL2 distro");
    Ok(())
}
