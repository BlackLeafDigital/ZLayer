//! WSL2 detection and capability checking.

/// Decode `wsl.exe` stdout, which is UTF-16 LE on Windows 10/11.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn decode_wsl_stdout(bytes: &[u8]) -> String {
    // Detect UTF-16 LE: even length with at least one NUL in the odd-indexed bytes
    // (ASCII characters encoded as UCS-2 LE alternate <ascii><0x00>).
    let decoded = if bytes.len() >= 2
        && bytes.len() % 2 == 0
        && bytes.iter().skip(1).step_by(2).any(|&b| b == 0)
    {
        let u16s: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|p| u16::from_le_bytes([p[0], p[1]]))
            .collect();
        String::from_utf16_lossy(&u16s)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    };
    // Strip a leading UTF-16 BOM so substring checks line up with the first character.
    decoded.trim_start_matches('\u{feff}').to_string()
}

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

/// Detect the current WSL2 status on this system.
///
/// # Errors
///
/// Returns an error if `wsl.exe --list --verbose` fails unexpectedly (e.g.
/// the WSL feature is installed but the service is broken). An absent
/// `wsl.exe` is not an error — it surfaces as `wsl_installed: false`.
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
            let stdout = decode_wsl_stdout(&o.stdout);
            stdout.contains("WSL 2") || stdout.contains("Default Version: 2")
        })
        .unwrap_or(false);

    // List distros to find default and check for zlayer
    let list_output = Command::new("wsl.exe")
        .args(["--list", "--verbose"])
        .output()
        .await?;

    let list_str = decode_wsl_stdout(&list_output.stdout);
    let mut default_distro = None;
    let mut zlayer_distro_exists = false;

    for line in list_str.lines().skip(1) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let is_default = line.starts_with('*');
        let parts: Vec<&str> = line.trim_start_matches('*').split_whitespace().collect();
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

#[cfg(test)]
mod tests {
    use super::decode_wsl_stdout;

    #[test]
    fn decode_wsl_stdout_passes_utf8_through_unchanged() {
        let input = b"Default Version: 2\n";
        assert_eq!(decode_wsl_stdout(input), "Default Version: 2\n");
    }

    #[test]
    fn decode_wsl_stdout_decodes_utf16_le() {
        // BOM + "Default Version: 2" encoded as UTF-16 LE.
        let mut bytes = vec![0xFF, 0xFE];
        for c in "Default Version: 2".encode_utf16() {
            bytes.extend_from_slice(&c.to_le_bytes());
        }
        let decoded = decode_wsl_stdout(&bytes);
        assert!(
            decoded.contains("Default Version: 2"),
            "expected substring missing in {decoded:?}"
        );
        assert!(
            !decoded.starts_with('\u{feff}'),
            "BOM should have been stripped"
        );
    }
}
