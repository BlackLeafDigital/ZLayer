//! First-time setup wizard for the WSL2 backend.
//!
//! Handles detecting WSL2, creating the dedicated distro,
//! and installing the `ZLayer` binary inside it.
//!
//! On Windows, when WSL2 is missing this module can auto-install it via an
//! elevated `wsl.exe --install --no-distribution` (UAC prompt) — but only
//! after the caller-supplied consent closure returns `Ok(true)`. Callers in
//! the CLI pipe this through to `crate::ui::consent::wsl2_install_consent`
//! (see `bin/zlayer/src/ui/consent.rs`).

use super::daemon::WslBackendConfig;
#[cfg(target_os = "windows")]
use super::errors::WslError;

/// Ensure the WSL2 backend is fully configured and ready.
///
/// Backward-compatible wrapper around
/// [`ensure_wsl_backend_ready_with_consent`] that hard-refuses any
/// auto-install. Existing call sites that did not ask for consent should
/// keep using this entry point — the behaviour is identical to the
/// pre-G-6 bail-out when WSL2 is missing.
///
/// # Errors
///
/// Returns an error if WSL2 is not available or setup fails. See
/// [`WslError`] for the refusal / reboot variants.
#[cfg(target_os = "windows")]
pub async fn ensure_wsl_backend_ready() -> anyhow::Result<WslBackendConfig> {
    ensure_wsl_backend_ready_with_consent(|| Ok(false)).await
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

/// Ensure the WSL2 backend is ready, asking the caller-supplied closure for
/// consent before attempting an elevated auto-install.
///
/// The `consent` closure is invoked **only** when WSL2 is not installed (or
/// only WSL1 is available). It must return:
/// - `Ok(true)` to approve the elevated `wsl --install --no-distribution`.
/// - `Ok(false)` to decline — this function then returns
///   [`WslError::InstallRefused`].
/// - `Err(_)` to bubble the error straight up (e.g. a TTY-detection
///   failure from the consent helper).
///
/// On a successful install the function re-probes `detect_wsl()` and falls
/// through into the normal VHD + distro + binary setup. If `wsl.exe --install`
/// returns `ERROR_SUCCESS_REBOOT_REQUIRED` (3010) the error is surfaced as
/// [`WslError::RebootRequired`] so the caller can show an actionable message.
///
/// # Errors
///
/// - [`WslError::InstallRefused`] if `consent()` returns `Ok(false)`.
/// - [`WslError::RebootRequired`] if the install asked Windows for a reboot.
/// - [`WslError::Wsl1Only`] if WSL is present but WSL2 is disabled and the
///   user declined consent for the version toggle.
/// - Any lower-level error from [`super::detect`], [`super::distro`],
///   [`super::wslconfig`], or a failed `ShellExecuteExW` call.
///
/// # Panics
///
/// Panics if the internal `consent_slot: Option<F>` is `None` immediately
/// after the WSL-not-installed branch populates it — in practice unreachable
/// because the slot is initialized to `Some(consent)` directly above and the
/// `.take()` call is the first consumer. The `.expect("consent_slot was just
/// populated")` documents the invariant rather than guarding a real failure
/// mode.
#[cfg(target_os = "windows")]
pub async fn ensure_wsl_backend_ready_with_consent<F>(
    consent: F,
) -> anyhow::Result<WslBackendConfig>
where
    F: FnOnce() -> anyhow::Result<bool>,
{
    use super::detect;

    let mut status = detect::detect_wsl().await?;

    // `consent` is `FnOnce`, so we move it out of an `Option<F>` the first
    // time we need it. The WSL1-only branch either already saw consent (because
    // we just ran the install branch — in which case status.wsl2_available is
    // true and we skip) or reaches into the same `Option` and drains it.
    let mut consent_slot: Option<F> = Some(consent);

    if !status.wsl_installed {
        tracing::info!("WSL2 is not installed; asking for auto-install consent");
        let consent_fn = consent_slot
            .take()
            .expect("consent_slot was just populated");
        let granted = consent_fn().context_consent()?;
        if !granted {
            return Err(WslError::InstallRefused.into());
        }

        tracing::info!("Consent granted — launching elevated `wsl --install --no-distribution`");
        run_elevated_wsl_install()?;

        // Re-probe WSL status now that the installer reported success.
        let post_install = detect::detect_wsl().await?;
        if !post_install.wsl_installed {
            anyhow::bail!(
                "WSL2 install reported success but `wsl.exe --status` still \
                 fails — a reboot may be required. Please reboot and re-run \
                 the command."
            );
        }
        status = post_install;
    }

    if status.wsl_installed && !status.wsl2_available {
        // Only prompt if we didn't already consume consent above. If we did,
        // the user has already said "yes" and we apply that answer here too.
        let granted = if let Some(consent_fn) = consent_slot.take() {
            consent_fn().context_consent()?
        } else {
            true
        };
        if !granted {
            return Err(WslError::InstallRefused.into());
        }

        // Elevate just for the version toggle — some policies gate it behind
        // admin even though the command itself doesn't strictly require it.
        run_elevated_set_default_version_2()?;

        status = detect::detect_wsl().await?;
        if !status.wsl2_available {
            return Err(WslError::Wsl1Only.into());
        }
    }

    finalize_wsl_backend(&status).await
}

/// Non-Windows stub for [`ensure_wsl_backend_ready_with_consent`].
///
/// # Errors
///
/// Always returns an error on non-Windows platforms.
#[cfg(not(target_os = "windows"))]
#[allow(clippy::unused_async)]
pub async fn ensure_wsl_backend_ready_with_consent<F>(
    _consent: F,
) -> anyhow::Result<WslBackendConfig>
where
    F: FnOnce() -> anyhow::Result<bool>,
{
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

/// Finalize the WSL2 backend once WSL2 itself is confirmed available.
///
/// - Writes/updates `.wslconfig` with a generous VHD cap (idempotent).
/// - Creates the `zlayer` distro if missing.
/// - Installs the `zlayer` Linux binary inside the distro if missing.
/// - Sanity-checks that the installed binary exposes the `runtime`
///   subcommand surface the WSL2 delegate relies on.
///
/// Pre-condition: `status.wsl_installed && status.wsl2_available` holds. On
/// violation returns the corresponding [`WslError`] instead of bailing.
#[cfg(target_os = "windows")]
async fn finalize_wsl_backend(
    status: &super::detect::WslStatus,
) -> anyhow::Result<WslBackendConfig> {
    use super::detect;
    use super::distro;

    if !status.wsl_installed {
        return Err(WslError::WslNotInstalled.into());
    }
    if !status.wsl2_available {
        return Err(WslError::Wsl1Only.into());
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

    // Re-probe to pick up any `zlayer` distro that may already exist.
    let final_status = detect::detect_wsl().await?;
    if !final_status.zlayer_distro_exists {
        tracing::info!("ZLayer WSL2 distro not found, creating...");
        setup_distro().await?;
    }

    // Verify binary exists inside distro
    let check = distro::wsl_exec("test", &["-x", "/usr/local/bin/zlayer"]).await?;
    if !check.status.success() {
        tracing::info!("ZLayer binary not found in WSL2 distro, installing...");
        install_binary().await?;
    }

    // Sanity-check that the installed binary actually exposes the
    // `runtime <verb>` subcommand surface the WSL2 delegate relies on. A
    // present-but-stale binary (wrong arch, built without the
    // `youki-runtime` feature) would otherwise only blow up on first
    // container dispatch with a confusing argv-parse error. Bail clearly
    // here instead.
    let runtime_check = distro::wsl_exec("/usr/local/bin/zlayer", &["runtime", "--help"]).await?;
    if !runtime_check.status.success() {
        return Err(anyhow::anyhow!(
            "/usr/local/bin/zlayer in the WSL2 distro does not expose the `runtime` \
             subcommand (status {:?}): {} — the installed binary is likely the wrong \
             architecture or was built without the `youki-runtime` feature; \
             rebuild and reinstall the Linux `zlayer` binary",
            runtime_check.status.code(),
            String::from_utf8_lossy(&runtime_check.stderr).trim(),
        ));
    }

    Ok(WslBackendConfig::default())
}

/// Launch `wsl.exe --install --no-distribution` elevated via `ShellExecuteExW`
/// (UAC prompt) and wait for the spawned process to exit.
///
/// Returns:
/// - `Ok(())` on clean exit code 0.
/// - `Err(WslError::RebootRequired)` on exit code
///   `ERROR_SUCCESS_REBOOT_REQUIRED` (3010).
/// - `Err(_)` for every other failure mode (UAC cancel, non-zero exit, etc.).
#[cfg(target_os = "windows")]
fn run_elevated_wsl_install() -> anyhow::Result<()> {
    tracing::info!("Launching elevated `wsl.exe --install --no-distribution` (UAC prompt)…");
    run_elevated_and_wait("wsl.exe", "--install --no-distribution")
}

/// Invoke `wsl.exe --set-default-version 2` elevated via UAC.
#[cfg(target_os = "windows")]
fn run_elevated_set_default_version_2() -> anyhow::Result<()> {
    tracing::info!("Launching elevated `wsl.exe --set-default-version 2` (UAC prompt)…");
    run_elevated_and_wait("wsl.exe", "--set-default-version 2")
}

/// Invoke `file params` with the UAC `runas` verb and synchronously wait for
/// the process to exit (up to 30 minutes), then translate the exit code.
///
/// `ERROR_SUCCESS_REBOOT_REQUIRED` (3010) is mapped to
/// [`WslError::RebootRequired`]; every other non-zero exit is propagated as
/// an anyhow error.
#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
fn run_elevated_and_wait(file: &str, params: &str) -> anyhow::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use std::time::Duration;

    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{CloseHandle, ERROR_SUCCESS_REBOOT_REQUIRED, WAIT_OBJECT_0};
    use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject, INFINITE};
    use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOW;

    /// Convert a Rust `&str` into a NUL-terminated UTF-16 buffer.
    fn to_wide(s: &str) -> Vec<u16> {
        std::ffi::OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    let verb = to_wide("runas");
    let file_w = to_wide(file);
    let parameters = to_wide(params);

    let mut info = SHELLEXECUTEINFOW {
        cbSize: u32::try_from(std::mem::size_of::<SHELLEXECUTEINFOW>())
            .expect("SHELLEXECUTEINFOW size fits in u32"),
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(file_w.as_ptr()),
        lpParameters: PCWSTR(parameters.as_ptr()),
        nShow: SW_SHOW.0,
        ..Default::default()
    };

    // SAFETY: `info` is a valid, fully-initialized SHELLEXECUTEINFOW. The
    // UTF-16 buffers it points into outlive the call (they're stack-local in
    // this function and we don't return until after `CloseHandle`).
    let info_ptr: *mut SHELLEXECUTEINFOW = &raw mut info;
    unsafe {
        ShellExecuteExW(info_ptr)
            .map_err(|e| anyhow::anyhow!("ShellExecuteExW(runas, {file} {params}) failed: {e}"))?;
    }

    if info.hProcess.is_invalid() {
        anyhow::bail!(
            "ShellExecuteExW returned without a process handle — UAC may have been cancelled"
        );
    }

    // Wait up to 30 minutes (typical WSL install is ~2 min; give Windows
    // Update plenty of headroom without blocking forever).
    let wait_ms = u32::try_from(Duration::from_secs(30 * 60).as_millis()).unwrap_or(INFINITE);
    // SAFETY: valid process handle returned by ShellExecuteExW.
    let wait_result = unsafe { WaitForSingleObject(info.hProcess, wait_ms) };
    if wait_result != WAIT_OBJECT_0 {
        // SAFETY: close even on wait failure to avoid leaking the handle.
        let _ = unsafe { CloseHandle(info.hProcess) };
        anyhow::bail!(
            "Timed out (or failed) waiting for `{file} {params}` (WaitForSingleObject = 0x{:x})",
            wait_result.0
        );
    }

    let mut exit_code: u32 = 0;
    let exit_code_ptr: *mut u32 = &raw mut exit_code;
    // SAFETY: process has exited (WAIT_OBJECT_0), handle is valid.
    let got_code = unsafe { GetExitCodeProcess(info.hProcess, exit_code_ptr) };
    // SAFETY: handle is still valid until CloseHandle.
    let _ = unsafe { CloseHandle(info.hProcess) };

    if let Err(e) = got_code {
        anyhow::bail!("GetExitCodeProcess failed after `{file} {params}`: {e}");
    }

    if exit_code == ERROR_SUCCESS_REBOOT_REQUIRED.0 {
        return Err(WslError::RebootRequired.into());
    }

    if exit_code != 0 {
        anyhow::bail!("`{file} {params}` exited with code {exit_code} (0x{exit_code:x})");
    }

    tracing::info!("`{file} {params}` completed (exit 0)");
    Ok(())
}

/// Create the `ZLayer` WSL2 distro with a minimal Alpine rootfs.
#[cfg(target_os = "windows")]
async fn setup_distro() -> anyhow::Result<()> {
    use std::path::{Path, PathBuf};

    let install_dir = super::paths::install_dir();

    // Look for bundled rootfs alongside the executable
    let exe_dir = std::env::current_exe()?
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);

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

/// Install the `ZLayer` Linux binary into the WSL2 distro.
#[cfg(target_os = "windows")]
async fn install_binary() -> anyhow::Result<()> {
    use std::path::{Path, PathBuf};

    let exe_dir = std::env::current_exe()?
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);

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

/// Tiny helper trait to annotate a consent-closure error with extra context
/// without pulling `anyhow::Context` into scope at the top of the module
/// (where it would collide with `super::Context` for non-Windows callers).
///
/// Only referenced from the Windows body of [`ensure_wsl_backend_ready_with_consent`].
#[cfg(target_os = "windows")]
trait ContextConsentExt<T> {
    fn context_consent(self) -> anyhow::Result<T>;
}

#[cfg(target_os = "windows")]
impl<T> ContextConsentExt<T> for anyhow::Result<T> {
    fn context_consent(self) -> anyhow::Result<T> {
        use anyhow::Context;
        self.context("WSL2 install-consent callback failed")
    }
}

// ---------------------------------------------------------------------------
// Tests (cross-platform — the elevated install itself can't be unit-tested)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::errors::WslError;

    /// The refuse path runs through the closure synchronously — it doesn't
    /// touch `detect::detect_wsl()` on non-Windows platforms because the
    /// function is stubbed there. To cover the refusal variant in a
    /// platform-agnostic way we call the error constructor directly and
    /// verify it surfaces as `InstallRefused`.
    #[test]
    fn install_refused_error_variant_is_wired() {
        let err = WslError::InstallRefused;
        let message = format!("{err}");
        assert!(
            message.contains("WSL2 auto-install was declined"),
            "unexpected InstallRefused text: {message}"
        );
        assert!(
            message.contains("--install-wsl yes"),
            "error should point users at the consent flag: {message}"
        );
    }

    #[test]
    fn reboot_required_error_variant_is_wired() {
        let err = WslError::RebootRequired;
        let message = format!("{err}");
        assert!(
            message.contains("reboot"),
            "reboot-required error should mention reboot: {message}"
        );
    }

    /// On non-Windows, `ensure_wsl_backend_ready_with_consent` is the
    /// platform stub; we just exercise it to guarantee the new signature
    /// compiles and errors out cleanly without calling the consent closure.
    #[cfg(not(target_os = "windows"))]
    #[tokio::test]
    async fn non_windows_stub_rejects_cleanly() {
        let called = std::sync::atomic::AtomicBool::new(false);
        let result = ensure_wsl_backend_ready_with_consent(|| {
            called.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(true)
        })
        .await;
        assert!(result.is_err(), "non-Windows stub must error");
        assert!(
            !called.load(std::sync::atomic::Ordering::SeqCst),
            "non-Windows stub must not invoke the consent closure"
        );
    }

    /// On Windows, calling the function against a host that already has WSL2
    /// is impossible to stub cleanly here — but we can verify that refusing
    /// consent on a WSL2-less host surfaces `InstallRefused`. We guard the
    /// test behind a real WSL check so it stays meaningful both on
    /// developer machines with WSL installed and on the Windows CI runner
    /// (which does not have WSL).
    #[cfg(target_os = "windows")]
    #[tokio::test]
    async fn refused_consent_yields_install_refused_when_wsl_missing() {
        let Ok(status) = crate::detect::detect_wsl().await else {
            return; // cannot probe WSL → test is inapplicable.
        };
        if status.wsl_installed {
            // Host already has WSL; the refusal branch is unreachable. Skip.
            return;
        }

        let called = std::sync::atomic::AtomicBool::new(false);
        let err = ensure_wsl_backend_ready_with_consent(|| {
            called.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(false)
        })
        .await
        .expect_err("refused consent must error");

        assert!(
            called.load(std::sync::atomic::Ordering::SeqCst),
            "consent closure must run when WSL2 is missing"
        );

        let downcast = err
            .downcast_ref::<WslError>()
            .expect("error should be a WslError");
        assert!(
            matches!(downcast, WslError::InstallRefused),
            "expected InstallRefused, got: {downcast:?}"
        );
    }
}
