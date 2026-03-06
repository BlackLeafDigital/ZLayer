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

/// Create the ZLayer WSL2 distro with a minimal Alpine rootfs.
#[cfg(target_os = "windows")]
async fn setup_distro() -> anyhow::Result<()> {
    use std::path::{Path, PathBuf};

    // Determine install directory
    let install_dir = if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
        PathBuf::from(local_app_data).join("ZLayer").join("wsl")
    } else {
        PathBuf::from(r"C:\ProgramData\ZLayer\wsl")
    };

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
