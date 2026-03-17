use std::path::Path;

use anyhow::{bail, Context, Result};
use tracing::info;

use crate::cli::DaemonAction;

/// Handle daemon lifecycle commands.
pub(crate) async fn handle_daemon(action: &DaemonAction, data_dir: &Path) -> Result<()> {
    match action {
        DaemonAction::Install {
            no_start,
            bind,
            jwt_secret,
            no_swagger,
        } => {
            install(
                data_dir,
                *no_start,
                bind,
                jwt_secret.as_deref(),
                *no_swagger,
            )
            .await
        }
        DaemonAction::Uninstall => uninstall().await,
        DaemonAction::Start => start().await,
        DaemonAction::Stop => stop().await,
        DaemonAction::Restart => restart().await,
        DaemonAction::Status => status(data_dir).await,
        DaemonAction::Reset { force } => reset(data_dir, *force),
    }
}

// ---------------------------------------------------------------------------
// macOS (launchd)
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
const PLIST_LABEL: &str = "com.zlayer.daemon";

/// Determine plist directory and launchctl target based on privilege level.
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
fn launchd_context() -> Result<(String, String)> {
    let is_root = unsafe { libc::geteuid() } == 0;
    let uid = unsafe { libc::getuid() };

    let plist_dir = if is_root {
        "/Library/LaunchDaemons".to_string()
    } else {
        let home = std::env::var("HOME").context("HOME not set")?;
        let dir = format!("{home}/Library/LaunchAgents");
        std::fs::create_dir_all(&dir).context("Failed to create ~/Library/LaunchAgents")?;
        dir
    };

    let target = if is_root {
        "system".to_string()
    } else {
        format!("gui/{uid}")
    };

    Ok((plist_dir, target))
}

#[cfg(target_os = "macos")]
fn plist_path_for(plist_dir: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(plist_dir).join(format!("{PLIST_LABEL}.plist"))
}

#[cfg(target_os = "macos")]
#[allow(clippy::too_many_lines, unsafe_code)]
async fn install(
    data_dir: &Path,
    no_start: bool,
    bind: &str,
    jwt_secret: Option<&str>,
    no_swagger: bool,
) -> Result<()> {
    use tokio::process::Command;

    let exe = std::env::current_exe().context("Failed to resolve current executable path")?;
    let exe_str = exe.to_string_lossy();

    let log_dir = crate::cli::default_log_dir(data_dir);
    let run_dir = crate::cli::default_run_dir(data_dir);
    let socket_path = run_dir.join("zlayer.sock");

    // Build ProgramArguments
    let mut args = vec![
        format!("        <string>{exe_str}</string>"),
        "        <string>serve</string>".to_string(),
        "        <string>--bind</string>".to_string(),
        format!("        <string>{bind}</string>"),
        "        <string>--socket</string>".to_string(),
        format!("        <string>{}</string>", socket_path.display()),
        "        <string>--data-dir</string>".to_string(),
        format!("        <string>{}</string>", data_dir.display()),
    ];

    if let Some(secret) = jwt_secret {
        args.push("        <string>--jwt-secret</string>".to_string());
        args.push(format!("        <string>{secret}</string>"));
    }
    if no_swagger {
        args.push("        <string>--no-swagger</string>".to_string());
    }

    let args_xml = args.join("\n");

    // Forward HOME for correct data directory resolution
    let env_xml = if let Ok(home) = std::env::var("HOME") {
        format!(
            r"    <key>EnvironmentVariables</key>
    <dict>
        <key>HOME</key>
        <string>{home}</string>
    </dict>"
        )
    } else {
        String::new()
    };

    let log_path = log_dir.join("daemon.log");
    let log_path_str = log_path.to_string_lossy();

    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{PLIST_LABEL}</string>
    <key>ProgramArguments</key>
    <array>
{args_xml}
    </array>
{env_xml}
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>{log_path_str}</string>
    <key>StandardErrorPath</key>
    <string>{log_path_str}</string>
    <key>WorkingDirectory</key>
    <string>/</string>
</dict>
</plist>"#
    );

    // Create directories
    std::fs::create_dir_all(&log_dir)
        .with_context(|| format!("Failed to create {}", log_dir.display()))?;
    std::fs::create_dir_all(&run_dir)
        .with_context(|| format!("Failed to create {}", run_dir.display()))?;

    let (plist_dir, target) = launchd_context()?;
    let path = plist_path_for(&plist_dir);
    let path_str = path.to_string_lossy().to_string();

    // Unload existing service first (ignore errors)
    let _ = Command::new("launchctl")
        .args(["bootout", &format!("{target}/{PLIST_LABEL}")])
        .output()
        .await;
    let _ = Command::new("launchctl")
        .args(["unload", &path_str])
        .output()
        .await;

    // Write plist
    std::fs::write(&path, &plist)
        .with_context(|| format!("Failed to write plist to {}", path.display()))?;
    println!("Installed launchd plist: {}", path.display());

    if !no_start {
        // Use modern launchctl bootstrap, fall back to legacy load
        let out = Command::new("launchctl")
            .args(["bootstrap", &target, &path_str])
            .output()
            .await
            .context("Failed to run launchctl bootstrap")?;

        if !out.status.success() {
            let out = Command::new("launchctl")
                .args(["load", "-w", &path_str])
                .output()
                .await
                .context("Failed to run launchctl load")?;
            if !out.status.success() {
                let stderr = String::from_utf8_lossy(&out.stderr);
                bail!("launchctl failed to load the service: {stderr}");
            }
        }

        println!("Daemon started via launchd");
        println!("  Logs: {log_path_str}");
        println!("  Stop: zlayer daemon stop");
    }

    Ok(())
}

#[cfg(target_os = "macos")]
async fn uninstall() -> Result<()> {
    use tokio::process::Command;

    let (plist_dir, target) = launchd_context()?;
    let path = plist_path_for(&plist_dir);

    if path.exists() {
        let _ = Command::new("launchctl")
            .args(["bootout", &format!("{target}/{PLIST_LABEL}")])
            .output()
            .await;
        let _ = Command::new("launchctl")
            .args(["unload", path.to_string_lossy().as_ref()])
            .output()
            .await;

        std::fs::remove_file(&path)
            .with_context(|| format!("Failed to remove {}", path.display()))?;
        println!("Uninstalled launchd plist: {}", path.display());
    } else {
        println!("No launchd plist found (checked {})", path.display());
    }
    Ok(())
}

#[cfg(target_os = "macos")]
async fn start() -> Result<()> {
    use tokio::process::Command;

    let (plist_dir, target) = launchd_context()?;
    let path = plist_path_for(&plist_dir);
    let path_str = path.to_string_lossy().to_string();

    if !path.exists() {
        bail!("Daemon not installed. Run `zlayer daemon install` first.");
    }

    let out = Command::new("launchctl")
        .args(["bootstrap", &target, &path_str])
        .output()
        .await
        .context("Failed to run launchctl bootstrap")?;

    if !out.status.success() {
        let out = Command::new("launchctl")
            .args(["load", "-w", &path_str])
            .output()
            .await
            .context("Failed to run launchctl load")?;
        if !out.status.success() {
            bail!("Failed to start daemon");
        }
    }

    println!("Daemon started");
    Ok(())
}

#[cfg(target_os = "macos")]
async fn stop() -> Result<()> {
    use tokio::process::Command;

    let (_plist_dir, target) = launchd_context()?;

    let out = Command::new("launchctl")
        .args(["bootout", &format!("{target}/{PLIST_LABEL}")])
        .output()
        .await
        .context("Failed to run launchctl bootout")?;

    if !out.status.success() {
        let (plist_dir, _) = launchd_context()?;
        let path = plist_path_for(&plist_dir);
        let _ = Command::new("launchctl")
            .args(["unload", path.to_string_lossy().as_ref()])
            .output()
            .await;
    }

    println!("Daemon stopped");
    Ok(())
}

#[cfg(target_os = "macos")]
async fn restart() -> Result<()> {
    use tokio::process::Command;

    let (_plist_dir, target) = launchd_context()?;

    let out = Command::new("launchctl")
        .args(["kickstart", "-k", &format!("{target}/{PLIST_LABEL}")])
        .output()
        .await
        .context("Failed to run launchctl kickstart")?;

    if !out.status.success() {
        stop().await.ok();
        return start().await;
    }

    println!("Daemon restarted");
    Ok(())
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code, clippy::cast_possible_truncation)]
async fn status(data_dir: &Path) -> Result<()> {
    use tokio::process::Command;

    let out = Command::new("launchctl")
        .args(["list", PLIST_LABEL])
        .output()
        .await
        .context("Failed to run launchctl list")?;

    if out.status.success() {
        println!("Service: registered with launchd");
    } else {
        println!("Service: not registered");
    }

    // Check daemon.json for process info
    let metadata_path = data_dir.join("daemon.json");
    if metadata_path.exists() {
        if let Ok(contents) = std::fs::read_to_string(&metadata_path) {
            println!("Daemon metadata: {}", metadata_path.display());
            if let Ok(meta) = serde_json::from_str::<serde_json::Value>(&contents) {
                if let Some(pid) = meta.get("pid").and_then(serde_json::Value::as_u64) {
                    let alive = unsafe { libc::kill(pid as i32, 0) } == 0;
                    println!(
                        "  PID: {pid} ({})",
                        if alive { "running" } else { "not running" }
                    );
                }
                if let Some(bind) = meta.get("api_bind").and_then(serde_json::Value::as_str) {
                    println!("  API: {bind}");
                }
                if let Some(sock) = meta.get("socket_path").and_then(serde_json::Value::as_str) {
                    println!("  Socket: {sock}");
                }
            }
        }
    } else {
        println!("Daemon: not running (no daemon.json)");
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Linux (systemd)
// ---------------------------------------------------------------------------

#[cfg(target_os = "linux")]
const UNIT_NAME: &str = "zlayer.service";

#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
fn unit_path() -> std::path::PathBuf {
    let is_root = unsafe { libc::geteuid() } == 0;
    if is_root {
        std::path::PathBuf::from("/etc/systemd/system").join(UNIT_NAME)
    } else {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
        std::path::PathBuf::from(format!("{home}/.config/systemd/user")).join(UNIT_NAME)
    }
}

#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
fn systemctl_args(base_args: &[&str]) -> Vec<String> {
    let is_root = unsafe { libc::geteuid() } == 0;
    if is_root {
        base_args.iter().copied().map(ToString::to_string).collect()
    } else {
        let mut args = vec!["--user".to_string()];
        args.extend(base_args.iter().copied().map(ToString::to_string));
        args
    }
}

#[cfg(target_os = "linux")]
async fn install(
    data_dir: &Path,
    no_start: bool,
    bind: &str,
    jwt_secret: Option<&str>,
    no_swagger: bool,
) -> Result<()> {
    use std::fmt::Write as _;
    use tokio::process::Command;

    let exe = std::env::current_exe().context("Cannot determine zlayer binary path")?;

    let mut exec_start = format!("{} serve --bind {bind}", exe.display());
    write!(exec_start, " --data-dir {}", data_dir.display()).unwrap();
    if no_swagger {
        exec_start.push_str(" --no-swagger");
    }

    let env_line = if let Some(secret) = jwt_secret {
        format!("Environment=ZLAYER_JWT_SECRET={secret}\n")
    } else {
        String::new()
    };

    let unit = format!(
        r"[Unit]
Description=ZLayer Container Orchestration Daemon
Documentation=https://zlayer.dev
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart={exec_start}
ExecReload=/bin/kill -HUP $MAINPID
Restart=always
RestartSec=5
LimitNOFILE=1048576
LimitNPROC=infinity
LimitCORE=infinity
{env_line}
[Install]
WantedBy=multi-user.target
",
    );

    let path = unit_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }

    tokio::fs::write(&path, unit)
        .await
        .with_context(|| format!("Failed to write {}", path.display()))?;
    println!("Installed systemd unit: {}", path.display());

    let reload_args = systemctl_args(&["daemon-reload"]);
    let _ = Command::new("systemctl").args(&reload_args).output().await;
    let enable_args = systemctl_args(&["enable", UNIT_NAME]);
    let _ = Command::new("systemctl").args(&enable_args).output().await;

    if !no_start {
        let start_args = systemctl_args(&["start", UNIT_NAME]);
        let out = Command::new("systemctl")
            .args(&start_args)
            .output()
            .await
            .context("Failed to start service")?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            bail!("systemctl start failed: {stderr}");
        }
        println!("Daemon started via systemd");
    }

    Ok(())
}

#[cfg(target_os = "linux")]
async fn uninstall() -> Result<()> {
    use tokio::process::Command;

    let stop_args = systemctl_args(&["stop", UNIT_NAME]);
    let _ = Command::new("systemctl").args(&stop_args).output().await;
    let disable_args = systemctl_args(&["disable", UNIT_NAME]);
    let _ = Command::new("systemctl").args(&disable_args).output().await;

    let path = unit_path();
    if path.exists() {
        tokio::fs::remove_file(&path)
            .await
            .with_context(|| format!("Failed to remove {}", path.display()))?;
        let reload_args = systemctl_args(&["daemon-reload"]);
        let _ = Command::new("systemctl").args(&reload_args).output().await;
        println!("Uninstalled systemd unit: {}", path.display());
    } else {
        println!("No systemd unit found at {}", path.display());
    }
    Ok(())
}

#[cfg(target_os = "linux")]
async fn start() -> Result<()> {
    use tokio::process::Command;

    let path = unit_path();
    if !path.exists() {
        bail!("Daemon not installed. Run `zlayer daemon install` first.");
    }

    let args = systemctl_args(&["start", UNIT_NAME]);
    let out = Command::new("systemctl")
        .args(&args)
        .output()
        .await
        .context("Failed to start service")?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        bail!("systemctl start failed: {stderr}");
    }
    println!("Daemon started");
    Ok(())
}

#[cfg(target_os = "linux")]
async fn stop() -> Result<()> {
    use tokio::process::Command;

    let args = systemctl_args(&["stop", UNIT_NAME]);
    let out = Command::new("systemctl")
        .args(&args)
        .output()
        .await
        .context("Failed to stop service")?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        bail!("systemctl stop failed: {stderr}");
    }
    println!("Daemon stopped");
    Ok(())
}

#[cfg(target_os = "linux")]
async fn restart() -> Result<()> {
    use tokio::process::Command;

    let args = systemctl_args(&["restart", UNIT_NAME]);
    let out = Command::new("systemctl")
        .args(&args)
        .output()
        .await
        .context("Failed to restart service")?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        bail!("systemctl restart failed: {stderr}");
    }
    println!("Daemon restarted");
    Ok(())
}

#[cfg(target_os = "linux")]
async fn status(data_dir: &Path) -> Result<()> {
    use tokio::process::Command;

    let args = systemctl_args(&["status", UNIT_NAME]);
    let out = Command::new("systemctl")
        .args(&args)
        .output()
        .await
        .context("Failed to query service status")?;

    let stdout = String::from_utf8_lossy(&out.stdout);
    if stdout.is_empty() {
        println!("Service: not installed");
    } else {
        println!("{stdout}");
    }

    // Check daemon.json for extra info
    let metadata_path = data_dir.join("daemon.json");
    if metadata_path.exists() {
        if let Ok(contents) = std::fs::read_to_string(&metadata_path) {
            if let Ok(meta) = serde_json::from_str::<serde_json::Value>(&contents) {
                if let Some(bind) = meta.get("api_bind").and_then(|v| v.as_str()) {
                    println!("  API: {bind}");
                }
                if let Some(sock) = meta.get("socket_path").and_then(|v| v.as_str()) {
                    println!("  Socket: {sock}");
                }
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Reset (shared across platforms)
// ---------------------------------------------------------------------------

fn reset(data_dir: &Path, force: bool) -> Result<()> {
    if !force {
        eprintln!(
            "This will wipe Raft storage and node identity in {}.",
            data_dir.display()
        );
        eprintln!("The daemon will reinitialise on next start.");
        eprintln!("Run with --force to skip this prompt.");
        bail!("Aborted. Pass --force to confirm.");
    }

    let dirs_to_remove = ["raft", "raft-log", "raft-sm"];
    for name in &dirs_to_remove {
        let p = data_dir.join(name);
        if p.exists() {
            std::fs::remove_dir_all(&p)
                .with_context(|| format!("Failed to remove {}", p.display()))?;
            info!("Removed {}", p.display());
        }
    }

    let files_to_remove = ["raft.db", "node_config.json"];
    for name in &files_to_remove {
        let p = data_dir.join(name);
        if p.exists() {
            std::fs::remove_file(&p)
                .with_context(|| format!("Failed to remove {}", p.display()))?;
            info!("Removed {}", p.display());
        }
    }

    println!("Daemon state reset. Start the daemon to reinitialize.");
    Ok(())
}
