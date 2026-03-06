//! WSL2 detection and capability checking.

/// WSL2 system status.
#[derive(Debug, Clone)]
pub struct WslStatus {
    /// Whether WSL is installed at all.
    pub wsl_installed: bool,
    /// Whether WSL2 (not just WSL1) is available.
    pub wsl2_available: bool,
    /// The default WSL distribution name, if any.
    pub default_distro: Option<String>,
    /// Whether the dedicated `zlayer` distro already exists.
    pub zlayer_distro_exists: bool,
}

/// Detect the current WSL2 status on this system.
///
/// On non-Windows platforms, this always returns "not installed".
///
/// # Errors
///
/// Returns an error if WSL commands fail unexpectedly.
#[cfg(not(target_os = "windows"))]
#[allow(clippy::unused_async)]
pub async fn detect_wsl() -> anyhow::Result<WslStatus> {
    Ok(WslStatus {
        wsl_installed: false,
        wsl2_available: false,
        default_distro: None,
        zlayer_distro_exists: false,
    })
}

#[cfg(target_os = "windows")]
pub async fn detect_wsl() -> anyhow::Result<WslStatus> {
    use tokio::process::Command;

    // Check if wsl.exe exists
    let wsl_check = Command::new("wsl.exe").arg("--status").output().await;

    let wsl_installed = match &wsl_check {
        Ok(output) => output.status.success(),
        Err(_) => false,
    };

    if !wsl_installed {
        return Ok(WslStatus {
            wsl_installed: false,
            wsl2_available: false,
            default_distro: None,
            zlayer_distro_exists: false,
        });
    }

    // Check WSL version from status output
    let wsl2_available = wsl_check
        .as_ref()
        .map(|o| {
            let stdout = String::from_utf8_lossy(&o.stdout);
            stdout.contains("WSL 2") || stdout.contains("Default Version: 2")
        })
        .unwrap_or(false);

    // List distros to find default and check for zlayer
    let list_output = Command::new("wsl.exe")
        .args(["--list", "--verbose"])
        .output()
        .await?;

    let list_str = String::from_utf8_lossy(&list_output.stdout);
    let mut default_distro = None;
    let mut zlayer_distro_exists = false;

    for line in list_str.lines().skip(1) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let is_default = line.starts_with('*');
        let parts: Vec<&str> = line
            .trim_start_matches('*')
            .trim()
            .split_whitespace()
            .collect();
        if let Some(name) = parts.first() {
            if is_default {
                default_distro = Some((*name).to_string());
            }
            if *name == "zlayer" {
                zlayer_distro_exists = true;
            }
        }
    }

    Ok(WslStatus {
        wsl_installed,
        wsl2_available,
        default_distro,
        zlayer_distro_exists,
    })
}
