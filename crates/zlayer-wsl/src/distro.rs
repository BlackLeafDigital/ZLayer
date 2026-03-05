//! `ZLayer` WSL2 distro management.
//!
//! Creates and manages a dedicated `zlayer` WSL2 distribution
//! for running the `ZLayer` daemon, isolated from the user's
//! default distro (similar to Docker Desktop's approach).

use std::path::Path;

/// Name of the dedicated WSL2 distribution.
pub const DISTRO_NAME: &str = "zlayer";

/// Create the dedicated ZLayer WSL2 distro from a rootfs tarball.
///
/// Runs: `wsl.exe --import zlayer <install_dir> <rootfs_path> --version 2`
///
/// # Errors
///
/// Returns an error if the WSL import command fails.
#[cfg(target_os = "windows")]
pub async fn create_distro(rootfs_path: &Path, install_dir: &Path) -> anyhow::Result<()> {
    use tokio::process::Command;

    tokio::fs::create_dir_all(install_dir).await?;

    let output = Command::new("wsl.exe")
        .args([
            "--import",
            DISTRO_NAME,
            &install_dir.display().to_string(),
            &rootfs_path.display().to_string(),
            "--version",
            "2",
        ])
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Failed to create WSL2 distro '{DISTRO_NAME}': {stderr}");
    }

    tracing::info!("Created WSL2 distro '{DISTRO_NAME}'");
    Ok(())
}

/// Create the dedicated `ZLayer` WSL2 distro (non-Windows stub).
///
/// # Errors
///
/// Always returns an error on non-Windows platforms.
#[cfg(not(target_os = "windows"))]
#[allow(clippy::unused_async)]
pub async fn create_distro(_rootfs_path: &Path, _install_dir: &Path) -> anyhow::Result<()> {
    anyhow::bail!("WSL2 distro management is only available on Windows")
}

/// Execute a command inside the ZLayer WSL2 distro.
///
/// # Errors
///
/// Returns an error if the command fails to execute or returns non-zero.
#[cfg(target_os = "windows")]
pub async fn wsl_exec(cmd: &str, args: &[&str]) -> anyhow::Result<std::process::Output> {
    use tokio::process::Command;

    let mut wsl_args = vec!["-d", DISTRO_NAME, "--"];
    wsl_args.push(cmd);
    wsl_args.extend_from_slice(args);

    let output = Command::new("wsl.exe").args(&wsl_args).output().await?;

    Ok(output)
}

/// Execute a command inside the `ZLayer` WSL2 distro (non-Windows stub).
///
/// # Errors
///
/// Always returns an error on non-Windows platforms.
#[cfg(not(target_os = "windows"))]
#[allow(clippy::unused_async)]
pub async fn wsl_exec(_cmd: &str, _args: &[&str]) -> anyhow::Result<std::process::Output> {
    anyhow::bail!("WSL2 commands are only available on Windows")
}

/// Check if the zlayer distro exists.
#[cfg(target_os = "windows")]
pub async fn distro_exists() -> bool {
    use tokio::process::Command;

    let output = Command::new("wsl.exe")
        .args(["--list", "--quiet"])
        .output()
        .await;

    match output {
        Ok(o) => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            stdout.lines().any(|l| l.trim() == DISTRO_NAME)
        }
        Err(_) => false,
    }
}

/// Check if the `zlayer` distro exists (non-Windows stub).
#[cfg(not(target_os = "windows"))]
#[allow(clippy::unused_async)]
pub async fn distro_exists() -> bool {
    false
}

/// Unregister (delete) the ZLayer WSL2 distro.
///
/// # Errors
///
/// Returns an error if the unregister command fails.
#[cfg(target_os = "windows")]
pub async fn remove_distro() -> anyhow::Result<()> {
    use tokio::process::Command;

    let output = Command::new("wsl.exe")
        .args(["--unregister", DISTRO_NAME])
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Failed to unregister distro '{DISTRO_NAME}': {stderr}");
    }

    tracing::info!("Removed WSL2 distro '{DISTRO_NAME}'");
    Ok(())
}

/// Unregister (delete) the `ZLayer` WSL2 distro (non-Windows stub).
///
/// # Errors
///
/// Always returns an error on non-Windows platforms.
#[cfg(not(target_os = "windows"))]
#[allow(clippy::unused_async)]
pub async fn remove_distro() -> anyhow::Result<()> {
    anyhow::bail!("WSL2 distro management is only available on Windows")
}
