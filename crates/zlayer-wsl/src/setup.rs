//! First-time setup wizard for the WSL2 backend.
//!
//! Handles detecting WSL2, optionally running an **elevated** `wsl --install`
//! after user consent (G-6), creating the dedicated distro, and installing
//! the `ZLayer` binary inside it.

use super::daemon::WslBackendConfig;

/// Ensure the WSL2 backend is fully configured and ready.
///
/// Back-compat entry point: hard-refuses any auto-install prompt. Use
/// [`ensure_wsl_backend_ready_with_consent`] for the G-6 consent flow.
///
/// # Errors
///
/// Returns an error if WSL2 is not installed, or setup fails.
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

/// G-6: consent-aware WSL2 bootstrap.
///
/// Drives the full "detect → (optionally auto-install with consent) → finalize"
/// flow. The `consent` closure is invoked **only** when WSL2 is missing or
/// WSL1-only; returning `Ok(false)` surfaces [`crate::errors::WslError::InstallRefused`]
/// so callers can degrade gracefully (e.g. route Linux workloads to peers).
///
/// Exit code `3010` from `wsl --install` (`ERROR_SUCCESS_REBOOT_REQUIRED`) is
/// mapped to [`crate::errors::WslError::RebootRequired`]. Callers typically bail
/// on this variant since the installer finished but the kernel module isn't
/// active yet.
///
/// # Errors
///
/// - [`crate::errors::WslError::InstallRefused`] — consent closure returned `Ok(false)`.
/// - [`crate::errors::WslError::RebootRequired`] — installer finished with 3010.
/// - Any I/O / command failure from downstream setup steps.
///
/// # Panics
///
/// Panics only if the internal consent slot accounting is violated (the
/// `consent_slot` `Option` is drained twice on the same code path — this
/// would be a logic bug in this function, not a user-reachable condition).
#[cfg(target_os = "windows")]
pub async fn ensure_wsl_backend_ready_with_consent<F>(
    consent: F,
) -> anyhow::Result<WslBackendConfig>
where
    F: FnOnce() -> anyhow::Result<bool>,
{
    use super::detect;
    use super::errors::WslError;

    let mut status = detect::detect_wsl().await?;

    // `consent` is `FnOnce`, so we move it out of an `Option<F>` the first
    // time we need it. The second branch ("wsl1 only after install") either
    // already saw consent (because we just ran the install branch — in which
    // case status.wsl2_available is true and we skip) or reaches into the
    // same `Option` and drains it.
    let mut consent_slot: Option<F> = Some(consent);

    if !status.wsl_installed {
        let consent_fn = consent_slot
            .take()
            .expect("consent_slot was just populated");
        let granted = consent_fn().context_consent()?;
        if !granted {
            return Err(WslError::InstallRefused.into());
        }

        // Elevated `wsl --install --no-distribution`.
        run_elevated_wsl_install().await?;

        // After install, make sure the default version is 2. This runs
        // non-elevated on purpose — `--set-default-version` only needs the
        // current user's WSL session, not admin.
        let set_default = tokio::process::Command::new("wsl.exe")
            .args(["--set-default-version", "2"])
            .output()
            .await;
        if let Ok(out) = &set_default {
            if !out.status.success() {
                let stderr = String::from_utf8_lossy(&out.stderr);
                tracing::warn!(
                    "wsl --set-default-version 2 failed after install: {}",
                    stderr.trim()
                );
            }
        }

        status = detect::detect_wsl().await?;
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
        run_elevated_set_default_version_2().await?;

        status = detect::detect_wsl().await?;
        if !status.wsl2_available {
            return Err(WslError::Wsl1Only.into());
        }
    }

    finalize_wsl_backend(&status).await
}

/// Non-Windows stub that forwards to [`ensure_wsl_backend_ready`] for API
/// symmetry. Consumers on Linux/macOS still get a clear "WSL is Windows-only"
/// error instead of a compile break.
///
/// # Errors
///
/// Always returns an error on non-Windows platforms (WSL2 is Windows-only).
#[cfg(not(target_os = "windows"))]
#[allow(clippy::unused_async)]
pub async fn ensure_wsl_backend_ready_with_consent<F>(
    _consent: F,
) -> anyhow::Result<WslBackendConfig>
where
    F: FnOnce() -> anyhow::Result<bool>,
{
    ensure_wsl_backend_ready().await
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
///
/// Pre-condition: `status.wsl_installed && status.wsl2_available` holds. On
/// violation returns the corresponding [`WslError`] instead of bailing.
#[cfg(target_os = "windows")]
async fn finalize_wsl_backend(
    status: &super::detect::WslStatus,
) -> anyhow::Result<WslBackendConfig> {
    use super::distro;
    use super::errors::WslError;

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

    if !status.zlayer_distro_exists {
        tracing::info!("ZLayer WSL2 distro not found, creating...");
        setup_distro().await?;
    }

    // Verify binary exists inside distro.
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

/// Invoke `wsl.exe --install --no-distribution` elevated via UAC.
///
/// Uses `ShellExecuteExW` with `lpVerb = "runas"`, waits up to 30 minutes for
/// the installer to finish, then inspects the exit code. `3010`
/// (`ERROR_SUCCESS_REBOOT_REQUIRED`) is mapped to [`WslError::RebootRequired`].
#[cfg(target_os = "windows")]
async fn run_elevated_wsl_install() -> anyhow::Result<()> {
    tracing::info!("Launching elevated `wsl.exe --install --no-distribution` (UAC prompt)…");
    run_elevated_and_wait("wsl.exe", "--install --no-distribution").await
}

/// Invoke `wsl.exe --set-default-version 2` elevated via UAC.
#[cfg(target_os = "windows")]
async fn run_elevated_set_default_version_2() -> anyhow::Result<()> {
    tracing::info!("Launching elevated `wsl.exe --set-default-version 2` (UAC prompt)…");
    run_elevated_and_wait("wsl.exe", "--set-default-version 2").await
}

/// Invoke `file params` with the UAC `runas` verb and synchronously wait for
/// the process to exit (up to 30 minutes), then translate the exit code.
///
/// The heavy FFI / handle work runs on `spawn_blocking` — `ShellExecuteExW`
/// pumps the COM/UI thread and `WaitForSingleObject` would block the tokio
/// worker indefinitely otherwise.
#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
async fn run_elevated_and_wait(file: &'static str, params: &'static str) -> anyhow::Result<()> {
    use super::errors::WslError;

    let result = tokio::task::spawn_blocking(move || -> anyhow::Result<u32> {
        use std::mem::size_of;
        use windows::core::{w, PCWSTR};
        use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_FAILED, WAIT_OBJECT_0};
        use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
        use windows::Win32::UI::Shell::{
            ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
        };
        use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;

        // 30-minute ceiling. `wsl --install` downloads the kernel MSIX and
        // enables optional Windows features, which on slow links / fresh VMs
        // can comfortably take 10+ minutes.
        const WAIT_TIMEOUT_MS: u32 = 30 * 60 * 1000;

        // Convert the lightweight &'static str into NUL-terminated UTF-16
        // buffers that outlive the ShellExecuteExW call. We keep the Vec<u16>
        // alive for the full duration of the call by binding it locally.
        let file_w: Vec<u16> = file.encode_utf16().chain(std::iter::once(0)).collect();
        let params_w: Vec<u16> = params.encode_utf16().chain(std::iter::once(0)).collect();
        let verb_w = w!("runas");

        // SAFETY: The lifetimes of `file_w` / `params_w` span the entire
        // block; `SHELLEXECUTEINFOW` is zeroed except for the fields we set
        // explicitly. `ShellExecuteExW` reads the struct synchronously and
        // returns a process HANDLE in `sei.hProcess` because we set
        // `SEE_MASK_NOCLOSEPROCESS`.
        unsafe {
            let mut sei = SHELLEXECUTEINFOW {
                cbSize: u32::try_from(size_of::<SHELLEXECUTEINFOW>())
                    .expect("SHELLEXECUTEINFOW size fits in u32"),
                fMask: SEE_MASK_NOCLOSEPROCESS,
                lpVerb: verb_w,
                lpFile: PCWSTR(file_w.as_ptr()),
                lpParameters: PCWSTR(params_w.as_ptr()),
                nShow: SW_HIDE.0,
                ..Default::default()
            };

            ShellExecuteExW(&raw mut sei).map_err(|e| {
                anyhow::anyhow!("ShellExecuteExW(runas, {file} {params}) failed: {e}")
            })?;

            let h_proc: HANDLE = sei.hProcess;
            if h_proc.is_invalid() {
                anyhow::bail!(
                    "ShellExecuteExW returned success but no process handle — UAC likely \
                     cancelled or no elevation was performed"
                );
            }

            let wait_result = WaitForSingleObject(h_proc, WAIT_TIMEOUT_MS);
            if wait_result == WAIT_FAILED {
                let _ = CloseHandle(h_proc);
                anyhow::bail!("WaitForSingleObject failed waiting for {file} {params}");
            }
            if wait_result != WAIT_OBJECT_0 {
                let _ = CloseHandle(h_proc);
                anyhow::bail!(
                    "Elevated `{file} {params}` did not finish within {} minutes",
                    WAIT_TIMEOUT_MS / 60_000
                );
            }

            let mut code: u32 = 0;
            let exit_result = GetExitCodeProcess(h_proc, &raw mut code);
            // Always close the handle; propagate any GetExitCodeProcess error
            // after cleanup.
            let _ = CloseHandle(h_proc);
            exit_result.map_err(|e| anyhow::anyhow!("GetExitCodeProcess failed: {e}"))?;

            Ok(code)
        }
    })
    .await
    .map_err(|e| anyhow::anyhow!("elevated install join error: {e}"))??;

    // 0 = success, 3010 = ERROR_SUCCESS_REBOOT_REQUIRED, anything else is fatal.
    match result {
        0 => Ok(()),
        3010 => Err(WslError::RebootRequired.into()),
        other => {
            anyhow::bail!(
                "Elevated `{file} {params}` exited with code {other}. \
                 Open an elevated PowerShell and run it manually to see the full error."
            )
        }
    }
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
