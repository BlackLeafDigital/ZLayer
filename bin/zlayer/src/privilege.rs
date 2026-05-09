//! Unified privilege handling for `zlayer` CLI subcommands that mutate
//! system-level state (the daemon service unit, launchd plist, or Windows
//! service registration).
//!
//! Users shouldn't have to remember `sudo` for `zlayer daemon install`; if
//! we're not already root/Administrator, we transparently re-exec the same
//! invocation under `sudo -E` (Unix) or via the UAC `runas` verb (Windows).
//!
//! The two entry points are:
//!
//! - [`is_privileged`] — cheap predicate, returns true iff EUID is 0 (Unix)
//!   or the current process token is in the Administrators group (Windows).
//! - [`ensure_root_or_reexec`] — short-circuits with `Ok(())` if already
//!   privileged, otherwise prints a one-line reason on stderr and replaces
//!   (Unix) or relaunches (Windows) the process with elevated rights. On
//!   success the call never returns; on failure to invoke the elevation
//!   helper, an error is propagated to the caller.

use anyhow::{Context, Result};

/// Returns `true` if the current process has the privilege the daemon
/// management subcommands need (root on Unix, Administrator on Windows).
#[must_use]
pub fn is_privileged() -> bool {
    zlayer_paths::is_root()
}

/// Ensure the current process has the privilege required to run a daemon
/// management action (install/uninstall/start/stop/restart/reset).
///
/// - When already privileged: returns `Ok(())` immediately.
/// - On Unix when not root: prints `ZLayer needs root to {reason}; re-running
///   under sudo...` on stderr and re-execs `sudo -E <current_exe> <argv...>`
///   via [`std::os::unix::process::CommandExt::exec`]. On success this call
///   never returns (the parent image is replaced). On exec failure, an error
///   is returned.
/// - On Windows when not Administrator: prints the same message, relaunches
///   the current executable via `ShellExecuteW` with the `runas` verb (which
///   triggers the UAC consent prompt), then exits the parent with status 0.
///   On `ShellExecuteW` failure (user declined, missing executable, etc.),
///   an error is returned.
pub fn ensure_root_or_reexec(reason: &str) -> Result<()> {
    if is_privileged() {
        return Ok(());
    }

    eprintln!("ZLayer needs root to {reason}; re-running under sudo...");

    #[cfg(unix)]
    {
        unix::reexec_with_sudo()
    }

    #[cfg(windows)]
    {
        windows_impl::relaunch_as_admin()
    }

    #[cfg(not(any(unix, windows)))]
    {
        anyhow::bail!(
            "privilege elevation is not supported on this platform; \
             please re-run zlayer as a privileged user"
        )
    }
}

#[cfg(unix)]
mod unix {
    use super::{Context, Result};
    use std::os::unix::process::CommandExt;
    use std::process::Command;

    /// Re-exec the current process via `sudo -E` so the user's environment
    /// (notably `ZLAYER_*` variables) flows through to the elevated child.
    ///
    /// `Command::exec` replaces the current process image — on success it
    /// never returns. If it does return, the OS rejected the exec (e.g. sudo
    /// not on PATH) and we surface the error.
    pub(super) fn reexec_with_sudo() -> Result<()> {
        let exe = std::env::current_exe()
            .context("failed to resolve the current zlayer executable for re-exec")?;

        // Skip argv[0] — sudo + the resolved exe path replace it.
        let forwarded_args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();

        let mut cmd = Command::new("sudo");
        cmd.arg("-E").arg(&exe).args(&forwarded_args);

        // `exec` consumes the Command and returns io::Error only on failure;
        // on success the process image has already been replaced.
        let err = cmd.exec();
        Err(anyhow::Error::new(err)
            .context("failed to re-exec zlayer under sudo (is `sudo` installed and on PATH?)"))
    }
}

#[cfg(windows)]
mod windows_impl {
    use super::{Context, Result};

    /// Relaunch the current executable via `ShellExecuteW` with the `runas`
    /// verb, which triggers the UAC elevation prompt. Returns `Ok(())` only
    /// to satisfy the type signature — on success the parent process exits
    /// with status 0 before this returns to the caller.
    pub(super) fn relaunch_as_admin() -> Result<()> {
        use std::os::windows::ffi::OsStrExt;
        use windows::core::PCWSTR;
        use windows::Win32::UI::Shell::ShellExecuteW;
        use windows::Win32::UI::WindowsAndMessaging::SW_NORMAL;

        let exe = std::env::current_exe()
            .context("failed to resolve the current zlayer executable for relaunch")?;

        // Build the parameter string by joining argv[1..] with proper quoting.
        let parameters = build_parameter_string(std::env::args_os().skip(1));

        // UTF-16 + NUL-terminated buffers for the Win32 API.
        let exe_w: Vec<u16> = exe.as_os_str().encode_wide().chain(Some(0)).collect();
        let verb_w: Vec<u16> = "runas".encode_utf16().chain(Some(0)).collect();
        let params_w: Vec<u16> = parameters.encode_utf16().chain(Some(0)).collect();

        // SAFETY: pointers reference NUL-terminated UTF-16 buffers we own for
        // the duration of the call. ShellExecuteW does not retain them.
        let hinstance = unsafe {
            ShellExecuteW(
                None,
                PCWSTR(verb_w.as_ptr()),
                PCWSTR(exe_w.as_ptr()),
                PCWSTR(params_w.as_ptr()),
                PCWSTR::null(),
                SW_NORMAL,
            )
        };

        // Per MSDN, ShellExecuteW returns an HINSTANCE > 32 on success.
        // Lower values encode well-known error codes (5 = ACCESS_DENIED when
        // the user declines UAC, 2 = FILE_NOT_FOUND, etc.).
        let code = hinstance.0 as isize;
        if code <= 32 {
            anyhow::bail!(
                "failed to relaunch zlayer with administrator rights (ShellExecuteW \
                 returned {code}); the elevation prompt may have been declined"
            );
        }

        // Parent exits cleanly — the elevated child is now running detached.
        std::process::exit(0);
    }

    /// Quote and join command-line arguments per the rules ShellExecuteW /
    /// CommandLineToArgvW expect. Arguments containing whitespace or quotes
    /// are wrapped in `"..."` and embedded backslashes/quotes are escaped.
    fn build_parameter_string<I, S>(args: I) -> String
    where
        I: IntoIterator<Item = S>,
        S: Into<std::ffi::OsString>,
    {
        let mut out = String::new();
        for (i, raw) in args.into_iter().enumerate() {
            if i > 0 {
                out.push(' ');
            }
            let arg: std::ffi::OsString = raw.into();
            let s = arg.to_string_lossy();
            if s.is_empty() {
                out.push_str("\"\"");
                continue;
            }
            let needs_quoting = s
                .chars()
                .any(|c| c == ' ' || c == '\t' || c == '"' || c == '\n');
            if !needs_quoting {
                out.push_str(&s);
                continue;
            }
            out.push('"');
            let mut backslashes = 0usize;
            for ch in s.chars() {
                match ch {
                    '\\' => {
                        backslashes += 1;
                    }
                    '"' => {
                        // Each backslash before a `"` must be doubled, plus
                        // one extra backslash to escape the `"` itself.
                        for _ in 0..(backslashes * 2 + 1) {
                            out.push('\\');
                        }
                        out.push('"');
                        backslashes = 0;
                    }
                    other => {
                        for _ in 0..backslashes {
                            out.push('\\');
                        }
                        backslashes = 0;
                        out.push(other);
                    }
                }
            }
            // Trailing backslashes before the closing quote also need doubling.
            for _ in 0..(backslashes * 2) {
                out.push('\\');
            }
            out.push('"');
        }
        out
    }
}
