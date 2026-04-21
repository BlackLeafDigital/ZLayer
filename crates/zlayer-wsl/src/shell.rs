//! Shell helpers for invoking Windows host tools.
//!
//! Unlike [`crate::distro::wsl_exec`] (which always runs commands *inside*
//! the `zlayer` distro), these helpers drive host-side tools directly:
//!
//! - [`wsl_control`] invokes `wsl.exe` without `-d zlayer --`, for control-plane
//!   commands like `wsl.exe --shutdown` or `wsl.exe --list`.
//! - [`powershell`] runs a `PowerShell` one-liner.
//! - [`diskpart_script`] pipes a script to `diskpart.exe` via stdin.
//!
//! On non-Windows builds every function returns an error — the crate still
//! compiles on Linux/macOS for unit tests and workspace-wide clippy, but the
//! actual host-tool calls only work under Windows.

use std::process::Output;

/// Invoke `wsl.exe` with the given args (no `-d zlayer --` prefix).
///
/// # Errors
///
/// Returns an error if the process fails to spawn. A non-zero exit is
/// reflected in the returned [`Output`] and is not itself an error.
#[cfg(target_os = "windows")]
pub async fn wsl_control<I, S>(args: I) -> anyhow::Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let output = tokio::process::Command::new("wsl.exe")
        .args(args)
        .output()
        .await?;
    Ok(output)
}

/// Invoke `wsl.exe` (non-Windows stub).
///
/// # Errors
///
/// Always returns an error on non-Windows platforms.
#[cfg(not(target_os = "windows"))]
#[allow(clippy::unused_async)]
pub async fn wsl_control<I, S>(_args: I) -> anyhow::Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    anyhow::bail!("wsl.exe is only available on Windows")
}

/// Run a PowerShell one-liner (`-NoProfile -NonInteractive -Command <script>`).
///
/// # Errors
///
/// Returns an error if the process fails to spawn. Non-zero exit is captured
/// in the returned [`Output`].
#[cfg(target_os = "windows")]
pub async fn powershell(script: &str) -> anyhow::Result<Output> {
    let output = tokio::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .output()
        .await?;
    Ok(output)
}

/// Run a `PowerShell` one-liner (non-Windows stub).
///
/// # Errors
///
/// Always returns an error on non-Windows platforms.
#[cfg(not(target_os = "windows"))]
#[allow(clippy::unused_async)]
pub async fn powershell(_script: &str) -> anyhow::Result<Output> {
    anyhow::bail!("powershell.exe is only available on Windows")
}

/// Pipe a diskpart script to `diskpart.exe` via stdin.
///
/// `diskpart` usually takes a script file via `/s <path>`; piping via stdin
/// sidesteps the need for a tempfile and works the same way.
///
/// # Errors
///
/// Returns an error if the process fails to spawn or stdin can't be written.
/// Non-zero exit is captured in the returned [`Output`].
#[cfg(target_os = "windows")]
pub async fn diskpart_script(script: &str) -> anyhow::Result<Output> {
    use std::process::Stdio;
    use tokio::io::AsyncWriteExt;

    let mut child = tokio::process::Command::new("diskpart.exe")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(script.as_bytes()).await?;
        stdin.shutdown().await?;
    }

    let output = child.wait_with_output().await?;
    Ok(output)
}

/// Pipe a diskpart script to `diskpart.exe` via stdin (non-Windows stub).
///
/// # Errors
///
/// Always returns an error on non-Windows platforms.
#[cfg(not(target_os = "windows"))]
#[allow(clippy::unused_async)]
pub async fn diskpart_script(_script: &str) -> anyhow::Result<Output> {
    anyhow::bail!("diskpart.exe is only available on Windows")
}
