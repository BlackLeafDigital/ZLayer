//! Service-manifest editor for `zlayer docker install`.
//!
//! Enables or disables `--docker-socket [--docker-socket-path ...]` on
//! the daemon's service launch command, then restarts the service.
//!
//! Minimum-surgery: rewrites only the `ExecStart=` line (systemd),
//! `ProgramArguments` array (launchd), or service binary path (SCM). All
//! other manifest settings (bind address, env, user, etc.) are left
//! untouched.

use std::path::{Path, PathBuf};

use anyhow::Result;

#[cfg(target_os = "macos")]
use anyhow::bail;
#[cfg(any(target_os = "macos", target_os = "windows"))]
use anyhow::Context;

/// Error cases the caller may want to distinguish.
#[derive(Debug, thiserror::Error)]
pub enum ReconfigError {
    #[error("zlayer daemon is not installed as a service; run `zlayer daemon install` first")]
    DaemonNotInstalled,

    #[error("failed to read service manifest at {path}: {source}")]
    ReadManifest {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to write service manifest at {path}: {source}")]
    WriteManifest {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("service manager command failed: {0}")]
    ServiceManager(String),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// Enable `--docker-socket --docker-socket-path <socket_path>` on the
/// daemon's service launch arguments, then reload + restart the service.
///
/// Idempotent: if the flags are already present on the `ExecStart` line,
/// the manifest is not rewritten (but the daemon is still restarted if
/// `restart == true`).
///
/// # Errors
///
/// Returns [`ReconfigError::DaemonNotInstalled`] when no service
/// manifest is found. Other variants on I/O or service-manager
/// failure.
pub async fn enable_docker_socket(socket_path: &Path, restart: bool) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        linux::enable(socket_path, restart).await
    }
    #[cfg(target_os = "macos")]
    {
        macos::enable(socket_path, restart).await
    }
    #[cfg(target_os = "windows")]
    {
        windows::enable(socket_path, restart).await
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        let _ = (socket_path, restart);
        anyhow::bail!("daemon reconfig is not supported on this platform");
    }
}

/// Disable `--docker-socket` on the daemon's service launch arguments,
/// then reload + restart.
///
/// Idempotent: if the flags are not present, no manifest rewrite
/// occurs.
///
/// # Errors
///
/// Same as [`enable_docker_socket`].
pub async fn disable_docker_socket(restart: bool) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        linux::disable(restart).await
    }
    #[cfg(target_os = "macos")]
    {
        macos::disable(restart).await
    }
    #[cfg(target_os = "windows")]
    {
        windows::disable(restart).await
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        let _ = restart;
        anyhow::bail!("daemon reconfig is not supported on this platform");
    }
}

// -- Linux --------------------------------------------------------------------

#[cfg(target_os = "linux")]
mod linux {
    use super::{Path, ReconfigError, Result};
    use anyhow::{bail, Context};
    use std::fs;
    use tokio::process::Command;

    const UNIT_PATH: &str = "/etc/systemd/system/zlayer.service";

    pub async fn enable(socket_path: &Path, restart: bool) -> Result<()> {
        let unit_path = Path::new(UNIT_PATH);
        if !unit_path.exists() {
            return Err(ReconfigError::DaemonNotInstalled.into());
        }
        let original = fs::read_to_string(unit_path).map_err(|e| ReconfigError::ReadManifest {
            path: unit_path.to_path_buf(),
            source: e,
        })?;
        let updated = rewrite_exec_start(&original, Some(socket_path))?;
        if updated != original {
            fs::write(unit_path, updated.as_bytes()).map_err(|e| ReconfigError::WriteManifest {
                path: unit_path.to_path_buf(),
                source: e,
            })?;
            run_systemctl(&["daemon-reload"]).await?;
        }
        if restart {
            run_systemctl(&["restart", "zlayer.service"]).await?;
        }
        Ok(())
    }

    pub async fn disable(restart: bool) -> Result<()> {
        let unit_path = Path::new(UNIT_PATH);
        if !unit_path.exists() {
            return Ok(());
        }
        let original = fs::read_to_string(unit_path).map_err(|e| ReconfigError::ReadManifest {
            path: unit_path.to_path_buf(),
            source: e,
        })?;
        let updated = rewrite_exec_start(&original, None)?;
        if updated != original {
            fs::write(unit_path, updated.as_bytes()).map_err(|e| ReconfigError::WriteManifest {
                path: unit_path.to_path_buf(),
                source: e,
            })?;
            run_systemctl(&["daemon-reload"]).await?;
        }
        if restart {
            run_systemctl(&["restart", "zlayer.service"]).await?;
        }
        Ok(())
    }

    async fn run_systemctl(args: &[&str]) -> Result<()> {
        let use_sudo = !zlayer_paths::is_root();
        let (program, full_args): (&str, Vec<&str>) = if use_sudo {
            (
                "sudo",
                std::iter::once("systemctl")
                    .chain(args.iter().copied())
                    .collect(),
            )
        } else {
            ("systemctl", args.to_vec())
        };
        let out = Command::new(program)
            .args(&full_args)
            .output()
            .await
            .with_context(|| format!("failed to exec {program} {}", full_args.join(" ")))?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            return Err(ReconfigError::ServiceManager(format!(
                "{program} {} -> exit={:?}: {}",
                full_args.join(" "),
                out.status.code(),
                stderr.trim()
            ))
            .into());
        }
        Ok(())
    }

    /// Rewrite the `ExecStart=` line to enable or disable
    /// `--docker-socket --docker-socket-path <socket_path>`.
    ///
    /// `new_socket_path = Some(p)` enables the flags (idempotent); `None`
    /// removes them (idempotent).
    pub(super) fn rewrite_exec_start(unit: &str, new_socket_path: Option<&Path>) -> Result<String> {
        let mut out = String::with_capacity(unit.len());
        let mut found = false;
        for line in unit.split_inclusive('\n') {
            let trimmed = line.trim_start();
            if !found && trimmed.starts_with("ExecStart=") {
                found = true;
                out.push_str(&rewrite_single_exec(line, new_socket_path));
            } else {
                out.push_str(line);
            }
        }
        if !found {
            bail!("unit file has no ExecStart= line");
        }
        Ok(out)
    }

    fn rewrite_single_exec(line: &str, new_socket_path: Option<&Path>) -> String {
        let had_newline = line.ends_with('\n');
        let stripped = line.trim_end_matches('\n');
        let Some((prefix, value)) = stripped.split_once('=') else {
            return line.to_string();
        };
        let tokens = shellish_split(value);
        let mut kept: Vec<String> = Vec::with_capacity(tokens.len());
        let mut i = 0;
        while i < tokens.len() {
            let tok = &tokens[i];
            if tok == "--docker-socket" {
                i += 1;
                continue;
            }
            if tok == "--docker-socket-path" {
                i += 2; // skip the value too
                continue;
            }
            if tok.starts_with("--docker-socket-path=") {
                i += 1;
                continue;
            }
            kept.push(tok.clone());
            i += 1;
        }
        if let Some(p) = new_socket_path {
            kept.push("--docker-socket".to_string());
            kept.push("--docker-socket-path".to_string());
            kept.push(p.to_string_lossy().into_owned());
        }
        let rebuilt = format!(
            "{prefix}={}{}",
            kept.iter()
                .map(|s| quote_if_needed(s))
                .collect::<Vec<_>>()
                .join(" "),
            if had_newline { "\n" } else { "" }
        );
        rebuilt
    }

    fn quote_if_needed(tok: &str) -> String {
        if tok.contains(' ') && !(tok.starts_with('"') && tok.ends_with('"')) {
            format!("\"{tok}\"")
        } else {
            tok.to_string()
        }
    }

    fn shellish_split(s: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut cur = String::new();
        let mut in_dq = false;
        let mut chars = s.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '"' => in_dq = !in_dq,
                ' ' | '\t' if !in_dq => {
                    if !cur.is_empty() {
                        out.push(std::mem::take(&mut cur));
                    }
                }
                '\\' if in_dq => {
                    if let Some(&next) = chars.peek() {
                        cur.push(next);
                        chars.next();
                    }
                }
                _ => cur.push(c),
            }
        }
        if !cur.is_empty() {
            out.push(cur);
        }
        out
    }
}

// -- macOS --------------------------------------------------------------------

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use std::fs;
    use tokio::process::Command;

    pub async fn enable(socket_path: &Path, restart: bool) -> Result<()> {
        let (plist_path, is_system) = find_plist()?;
        let original =
            fs::read_to_string(&plist_path).map_err(|e| ReconfigError::ReadManifest {
                path: plist_path.clone(),
                source: e,
            })?;
        let updated = rewrite_program_arguments(&original, Some(socket_path))?;
        if updated != original {
            fs::write(&plist_path, updated.as_bytes()).map_err(|e| {
                ReconfigError::WriteManifest {
                    path: plist_path.clone(),
                    source: e,
                }
            })?;
        }
        if restart {
            reload_launchd(&plist_path, is_system).await?;
        }
        Ok(())
    }

    pub async fn disable(restart: bool) -> Result<()> {
        let (plist_path, is_system) = match find_plist() {
            Ok(p) => p,
            Err(e) if matches!(e.downcast_ref(), Some(ReconfigError::DaemonNotInstalled)) => {
                return Ok(());
            }
            Err(e) => return Err(e),
        };
        let original =
            fs::read_to_string(&plist_path).map_err(|e| ReconfigError::ReadManifest {
                path: plist_path.clone(),
                source: e,
            })?;
        let updated = rewrite_program_arguments(&original, None)?;
        if updated != original {
            fs::write(&plist_path, updated.as_bytes()).map_err(|e| {
                ReconfigError::WriteManifest {
                    path: plist_path.clone(),
                    source: e,
                }
            })?;
        }
        if restart {
            reload_launchd(&plist_path, is_system).await?;
        }
        Ok(())
    }

    fn find_plist() -> Result<(PathBuf, bool)> {
        let system = PathBuf::from("/Library/LaunchDaemons/com.zlayer.daemon.plist");
        if system.exists() {
            return Ok((system, true));
        }
        if let Some(home) = std::env::var_os("HOME") {
            let user = PathBuf::from(home).join("Library/LaunchAgents/com.zlayer.daemon.plist");
            if user.exists() {
                return Ok((user, false));
            }
        }
        Err(ReconfigError::DaemonNotInstalled.into())
    }

    async fn reload_launchd(plist_path: &Path, is_system: bool) -> Result<()> {
        let domain = if is_system {
            "system".to_string()
        } else {
            format!("gui/{}", unsafe { libc::getuid() })
        };
        // bootout may fail if not currently loaded — tolerate.
        let _ = Command::new("launchctl")
            .args(["bootout", &domain, plist_path.to_string_lossy().as_ref()])
            .output()
            .await;
        let out = Command::new("launchctl")
            .args(["bootstrap", &domain, plist_path.to_string_lossy().as_ref()])
            .output()
            .await
            .with_context(|| "failed to exec launchctl bootstrap")?;
        if !out.status.success() {
            return Err(ReconfigError::ServiceManager(format!(
                "launchctl bootstrap {} {} -> {}",
                domain,
                plist_path.display(),
                String::from_utf8_lossy(&out.stderr).trim()
            ))
            .into());
        }
        Ok(())
    }

    /// Rewrite the `<array>` under `<key>ProgramArguments</key>`.
    ///
    /// Text-mode rewrite (no plist XML parser) — the launchd plists ZLayer
    /// writes are compact enough for this to be safe. If the format is
    /// unexpected we bail rather than mangle it.
    pub(super) fn rewrite_program_arguments(
        plist: &str,
        new_socket_path: Option<&Path>,
    ) -> Result<String> {
        let key_marker = "<key>ProgramArguments</key>";
        let Some(key_idx) = plist.find(key_marker) else {
            bail!("plist has no ProgramArguments key");
        };
        let array_start = plist[key_idx..]
            .find("<array>")
            .map(|off| key_idx + off)
            .ok_or_else(|| anyhow::anyhow!("ProgramArguments is not followed by <array>"))?;
        let array_open_end = array_start + "<array>".len();
        let array_close = plist[array_open_end..]
            .find("</array>")
            .map(|off| array_open_end + off)
            .ok_or_else(|| anyhow::anyhow!("ProgramArguments <array> has no </array>"))?;

        let args_block = &plist[array_open_end..array_close];
        let mut args: Vec<String> = args_block
            .split("<string>")
            .skip(1)
            .filter_map(|chunk| chunk.split("</string>").next().map(|s| s.to_string()))
            .collect();

        let mut i = 0;
        while i < args.len() {
            if args[i] == "--docker-socket" {
                args.remove(i);
                continue;
            }
            if args[i] == "--docker-socket-path" && i + 1 < args.len() {
                args.remove(i);
                args.remove(i);
                continue;
            }
            if args[i].starts_with("--docker-socket-path=") {
                args.remove(i);
                continue;
            }
            i += 1;
        }
        if let Some(p) = new_socket_path {
            args.push("--docker-socket".to_string());
            args.push("--docker-socket-path".to_string());
            args.push(p.to_string_lossy().into_owned());
        }

        // Reconstruct array body. Detect original leading whitespace so
        // the regenerated block matches the indentation style.
        let indent = detect_indent(&plist[array_open_end..array_close]);
        let mut new_body = String::from("\n");
        for a in &args {
            new_body.push_str(&indent);
            new_body.push_str("<string>");
            new_body.push_str(&xml_escape(a));
            new_body.push_str("</string>\n");
        }
        // Match closing-brace indentation from original
        let close_indent = detect_close_indent(&plist[..array_close]);
        new_body.push_str(&close_indent);

        let mut out = String::with_capacity(plist.len());
        out.push_str(&plist[..array_open_end]);
        out.push_str(&new_body);
        out.push_str(&plist[array_close..]);
        Ok(out)
    }

    fn detect_indent(block: &str) -> String {
        for line in block.lines() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("<string>") {
                return line[..line.len() - trimmed.len()].to_string();
            }
        }
        "\t\t".to_string()
    }

    fn detect_close_indent(prefix: &str) -> String {
        // Look backward for the line containing the `<array>` tag and use
        // its indentation for the `</array>` closer.
        for line in prefix.lines().rev() {
            if line.contains("<array>") {
                let indent_len = line.len() - line.trim_start().len();
                return line[..indent_len].to_string();
            }
        }
        "\t".to_string()
    }

    fn xml_escape(s: &str) -> String {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    }
}

// -- Windows -----------------------------------------------------------------

#[cfg(target_os = "windows")]
mod windows {
    use super::{Context, Path, PathBuf, ReconfigError, Result};
    use windows_service::{
        service::{ServiceAccess, ServiceInfo},
        service_manager::{ServiceManager, ServiceManagerAccess},
    };

    const SERVICE_NAME: &str = "ZLayerDaemon";

    pub async fn enable(socket_path: &Path, restart: bool) -> Result<()> {
        tokio::task::spawn_blocking({
            let socket_path = socket_path.to_path_buf();
            move || enable_blocking(&socket_path)
        })
        .await
        .context("blocking task panicked")??;

        if restart {
            restart_service().await?;
        }
        Ok(())
    }

    pub async fn disable(restart: bool) -> Result<()> {
        tokio::task::spawn_blocking(disable_blocking)
            .await
            .context("blocking task panicked")??;
        if restart {
            restart_service().await?;
        }
        Ok(())
    }

    fn enable_blocking(socket_path: &Path) -> Result<()> {
        let mgr = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
            .context("ServiceManager::local_computer")?;

        let service = match mgr.open_service(
            SERVICE_NAME,
            ServiceAccess::QUERY_CONFIG | ServiceAccess::CHANGE_CONFIG,
        ) {
            Ok(s) => s,
            Err(e) => {
                // Map "service does not exist" to DaemonNotInstalled
                let msg = format!("{e}");
                if msg.to_lowercase().contains("does not exist") || msg.contains("1060") {
                    return Err(ReconfigError::DaemonNotInstalled.into());
                }
                return Err(e).context("open_service");
            }
        };
        let cfg = service.query_config().context("query_config")?;
        let existing = cfg.executable_path.to_string_lossy().into_owned();
        let (binary_path, mut tail) = split_binary_and_tail(&existing);
        strip_docker_socket_args(&mut tail);
        tail.push("--docker-socket".to_string());
        tail.push("--docker-socket-path".to_string());
        tail.push(socket_path.to_string_lossy().into_owned());
        let new_cmdline = rebuild_cmdline(&binary_path, &tail);

        let new_info = ServiceInfo {
            name: cfg.display_name.clone(),
            display_name: cfg.display_name.clone(),
            service_type: cfg.service_type,
            start_type: cfg.start_type,
            error_control: cfg.error_control,
            executable_path: PathBuf::from(&new_cmdline),
            launch_arguments: Vec::new(),
            dependencies: cfg.dependencies.clone(),
            account_name: cfg.account_name.clone(),
            account_password: None,
        };
        service.change_config(&new_info).context("change_config")?;
        Ok(())
    }

    fn disable_blocking() -> Result<()> {
        let mgr = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
            .context("ServiceManager::local_computer")?;
        let Ok(service) = mgr.open_service(
            SERVICE_NAME,
            ServiceAccess::QUERY_CONFIG | ServiceAccess::CHANGE_CONFIG,
        ) else {
            return Ok(()); // nothing to disable
        };
        let cfg = service.query_config().context("query_config")?;
        let existing = cfg.executable_path.to_string_lossy().into_owned();
        let (binary_path, mut tail) = split_binary_and_tail(&existing);
        strip_docker_socket_args(&mut tail);
        let new_cmdline = rebuild_cmdline(&binary_path, &tail);

        let new_info = ServiceInfo {
            name: cfg.display_name.clone(),
            display_name: cfg.display_name.clone(),
            service_type: cfg.service_type,
            start_type: cfg.start_type,
            error_control: cfg.error_control,
            executable_path: PathBuf::from(&new_cmdline),
            launch_arguments: Vec::new(),
            dependencies: cfg.dependencies.clone(),
            account_name: cfg.account_name.clone(),
            account_password: None,
        };
        service.change_config(&new_info).context("change_config")?;
        Ok(())
    }

    async fn restart_service() -> Result<()> {
        use windows_service::service::ServiceState;
        tokio::task::spawn_blocking(|| -> Result<()> {
            let mgr = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)?;
            let service = mgr.open_service(
                SERVICE_NAME,
                ServiceAccess::STOP | ServiceAccess::START | ServiceAccess::QUERY_STATUS,
            )?;
            // Best-effort stop.
            let _ = service.stop();
            for _ in 0..60 {
                if service.query_status()?.current_state == ServiceState::Stopped {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
            service.start::<&str>(&[])?;
            Ok(())
        })
        .await
        .context("restart task panicked")?
    }

    fn split_binary_and_tail(cmdline: &str) -> (String, Vec<String>) {
        // Handle quoted binary path: `"C:\foo\bar.exe" arg1 arg2`
        let trimmed = cmdline.trim_start();
        let (bin, rest) = if let Some(stripped) = trimmed.strip_prefix('"') {
            match stripped.find('"') {
                Some(end) => (&stripped[..end], &stripped[end + 1..]),
                None => return (cmdline.to_string(), Vec::new()),
            }
        } else {
            match trimmed.find(' ') {
                Some(end) => (&trimmed[..end], &trimmed[end..]),
                None => return (cmdline.to_string(), Vec::new()),
            }
        };
        let tail = shellish_split(rest.trim_start());
        (bin.to_string(), tail)
    }

    fn strip_docker_socket_args(tail: &mut Vec<String>) {
        let mut i = 0;
        while i < tail.len() {
            if tail[i] == "--docker-socket" {
                tail.remove(i);
                continue;
            }
            if tail[i] == "--docker-socket-path" && i + 1 < tail.len() {
                tail.remove(i);
                tail.remove(i);
                continue;
            }
            if tail[i].starts_with("--docker-socket-path=") {
                tail.remove(i);
                continue;
            }
            i += 1;
        }
    }

    fn rebuild_cmdline(binary: &str, tail: &[String]) -> String {
        let mut out = format!("\"{binary}\"");
        for a in tail {
            out.push(' ');
            if a.contains(' ') {
                out.push('"');
                out.push_str(a);
                out.push('"');
            } else {
                out.push_str(a);
            }
        }
        out
    }

    fn shellish_split(s: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut cur = String::new();
        let mut in_dq = false;
        let mut chars = s.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '"' => in_dq = !in_dq,
                ' ' | '\t' if !in_dq => {
                    if !cur.is_empty() {
                        out.push(std::mem::take(&mut cur));
                    }
                }
                '\\' if in_dq => {
                    if let Some(&next) = chars.peek() {
                        cur.push(next);
                        chars.next();
                    }
                }
                _ => cur.push(c),
            }
        }
        if !cur.is_empty() {
            out.push(cur);
        }
        out
    }
}

// -- Tests --------------------------------------------------------------------

#[cfg(all(test, target_os = "linux"))]
mod linux_tests {
    use super::linux::rewrite_exec_start;
    use std::path::Path;

    #[test]
    fn enable_adds_flags_when_missing() {
        let unit =
            "[Service]\nExecStart=/usr/local/bin/zlayer serve --daemon --bind 0.0.0.0:3669\n";
        let out = rewrite_exec_start(unit, Some(Path::new("/var/run/zlayer/docker.sock"))).unwrap();
        assert!(out.contains("--docker-socket"));
        assert!(out.contains("--docker-socket-path /var/run/zlayer/docker.sock"));
        assert!(out.contains("--daemon"));
        assert!(out.contains("--bind 0.0.0.0:3669"));
    }

    #[test]
    fn enable_is_idempotent() {
        let unit = "ExecStart=/usr/local/bin/zlayer serve --daemon --docker-socket --docker-socket-path /foo/docker.sock\n";
        let once = rewrite_exec_start(unit, Some(Path::new("/foo/docker.sock"))).unwrap();
        let twice = rewrite_exec_start(&once, Some(Path::new("/foo/docker.sock"))).unwrap();
        assert_eq!(once, twice);
    }

    #[test]
    fn enable_updates_path_if_changed() {
        let unit = "ExecStart=/usr/local/bin/zlayer serve --docker-socket --docker-socket-path /old/path.sock\n";
        let out = rewrite_exec_start(unit, Some(Path::new("/new/path.sock"))).unwrap();
        assert!(out.contains("--docker-socket-path /new/path.sock"));
        assert!(!out.contains("/old/path.sock"));
    }

    #[test]
    fn disable_removes_both_flags() {
        let unit = "ExecStart=/usr/local/bin/zlayer serve --daemon --docker-socket --docker-socket-path /var/run/docker.sock --bind 0.0.0.0:3669\n";
        let out = rewrite_exec_start(unit, None).unwrap();
        assert!(!out.contains("--docker-socket"));
        assert!(!out.contains("--docker-socket-path"));
        assert!(out.contains("--bind 0.0.0.0:3669"));
        assert!(out.contains("--daemon"));
    }

    #[test]
    fn disable_is_noop_when_absent() {
        let unit = "ExecStart=/usr/local/bin/zlayer serve --daemon\n";
        let out = rewrite_exec_start(unit, None).unwrap();
        assert_eq!(out, unit);
    }

    #[test]
    fn disable_handles_equals_form() {
        let unit = "ExecStart=/usr/local/bin/zlayer serve --daemon --docker-socket-path=/foo/docker.sock --docker-socket\n";
        let out = rewrite_exec_start(unit, None).unwrap();
        assert!(!out.contains("--docker-socket"));
        assert!(!out.contains("/foo/docker.sock"));
    }

    #[test]
    fn nonexistent_exec_start_errors() {
        let unit = "[Service]\n# no ExecStart\n";
        let err = rewrite_exec_start(unit, None).unwrap_err();
        assert!(err.to_string().contains("no ExecStart"));
    }
}

#[cfg(all(test, target_os = "macos"))]
mod macos_tests {
    use super::macos::rewrite_program_arguments;
    use std::path::Path;

    const SAMPLE: &str = r#"<?xml version="1.0"?>
<plist version="1.0">
<dict>
  <key>Label</key><string>com.zlayer.daemon</string>
  <key>ProgramArguments</key>
  <array>
    <string>/usr/local/bin/zlayer</string>
    <string>serve</string>
    <string>--daemon</string>
  </array>
</dict>
</plist>
"#;

    #[test]
    fn enable_adds_flags() {
        let out = rewrite_program_arguments(SAMPLE, Some(Path::new("/tmp/z.sock"))).unwrap();
        assert!(out.contains("<string>--docker-socket</string>"));
        assert!(out.contains("<string>--docker-socket-path</string>"));
        assert!(out.contains("<string>/tmp/z.sock</string>"));
    }

    #[test]
    fn enable_is_idempotent() {
        let once = rewrite_program_arguments(SAMPLE, Some(Path::new("/tmp/z.sock"))).unwrap();
        let twice = rewrite_program_arguments(&once, Some(Path::new("/tmp/z.sock"))).unwrap();
        assert_eq!(once, twice);
    }

    #[test]
    fn disable_is_noop_when_absent() {
        let out = rewrite_program_arguments(SAMPLE, None).unwrap();
        assert_eq!(out, SAMPLE);
    }

    #[test]
    fn disable_removes_both_flags() {
        let enabled = rewrite_program_arguments(SAMPLE, Some(Path::new("/tmp/z.sock"))).unwrap();
        let out = rewrite_program_arguments(&enabled, None).unwrap();
        assert!(!out.contains("--docker-socket"));
        assert!(!out.contains("/tmp/z.sock"));
        assert!(out.contains("<string>/usr/local/bin/zlayer</string>"));
        assert!(out.contains("<string>serve</string>"));
    }
}
