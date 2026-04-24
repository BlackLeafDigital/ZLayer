//! User-environment writer on Windows.
//!
//! Writes / removes user-level env vars (`HKCU\Environment`) and
//! broadcasts `WM_SETTINGCHANGE` so running Explorer + cmd sessions
//! pick up the change.

use std::path::Path;

use anyhow::{Context, Result};
use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};
use winreg::RegKey;

const ENV_KEY: &str = "Environment";

/// Install a user-level env var via `HKCU\Environment`. Idempotent.
///
/// # Errors
///
/// Returns an error if the registry write fails.
pub fn install_user_env(var: &str, val: &str) -> Result<()> {
    let env = open_env(true)?;
    let existing: Option<String> = env.get_value(var).ok();
    if existing.as_deref() == Some(val) {
        return Ok(());
    }
    env.set_value(var, &val)
        .with_context(|| format!("failed to set HKCU\\{ENV_KEY}\\{var}"))?;
    broadcast_setting_change();
    Ok(())
}

/// Remove a user-level env var. Idempotent.
///
/// # Errors
///
/// Returns an error if the registry delete fails (other than "not
/// present", which is treated as success).
pub fn uninstall_user_env(var: &str) -> Result<()> {
    let env = open_env(true)?;
    match env.delete_value(var) {
        Ok(()) => {
            broadcast_setting_change();
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("failed to delete HKCU\\{ENV_KEY}\\{var}")),
    }
}

/// Ensure `dir` is on the user's `PATH`. Idempotent — does not add a
/// duplicate entry.
///
/// # Errors
///
/// Returns an error if the registry read or write fails.
pub fn ensure_dir_on_user_path(dir: &Path) -> Result<bool> {
    let env = open_env(true)?;
    let current: String = env.get_value("Path").unwrap_or_default();
    let dir_str = dir.to_string_lossy();
    if path_contains(&current, &dir_str) {
        return Ok(false);
    }
    let new_path = if current.is_empty() {
        dir_str.into_owned()
    } else if current.ends_with(';') {
        format!("{current}{dir_str}")
    } else {
        format!("{current};{dir_str}")
    };
    env.set_value("Path", &new_path)
        .with_context(|| "failed to write HKCU\\Environment\\Path")?;
    broadcast_setting_change();
    Ok(true)
}

/// Remove `dir` from the user's `PATH`. Idempotent.
///
/// # Errors
///
/// Returns an error if the registry read or write fails.
pub fn remove_dir_from_user_path(dir: &Path) -> Result<bool> {
    let env = open_env(true)?;
    let current: String = match env.get_value::<String, _>("Path") {
        Ok(v) => v,
        Err(_) => return Ok(false),
    };
    let dir_str = dir.to_string_lossy();
    if !path_contains(&current, &dir_str) {
        return Ok(false);
    }
    let filtered = current
        .split(';')
        .filter(|entry| !entry.eq_ignore_ascii_case(&dir_str))
        .collect::<Vec<_>>()
        .join(";");
    env.set_value("Path", &filtered)
        .with_context(|| "failed to write HKCU\\Environment\\Path")?;
    broadcast_setting_change();
    Ok(true)
}

fn open_env(writable: bool) -> Result<RegKey> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let access = if writable {
        KEY_READ | KEY_WRITE
    } else {
        KEY_READ
    };
    hkcu.open_subkey_with_flags(ENV_KEY, access)
        .context("failed to open HKCU\\Environment")
}

fn path_contains(path_value: &str, entry: &str) -> bool {
    path_value.split(';').any(|p| p.eq_ignore_ascii_case(entry))
}

/// Broadcast `WM_SETTINGCHANGE` so running sessions re-read the
/// environment.
#[allow(unsafe_code)]
fn broadcast_setting_change() {
    use windows::core::w;
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        SendMessageTimeoutW, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE,
    };

    // Safety: SendMessageTimeoutW is safe to call with these well-formed arguments.
    unsafe {
        let _ = SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            WPARAM(0),
            LPARAM(w!("Environment").as_ptr() as isize),
            SMTO_ABORTIFHUNG,
            5000,
            None,
        );
    }
}

// We can't meaningfully unit-test the registry writer in CI without
// making the tests write to the host's `HKCU\Environment`. Integration
// testing is covered by the `zlayer docker install` / `uninstall`
// smoke tests on the Windows CI runner.
