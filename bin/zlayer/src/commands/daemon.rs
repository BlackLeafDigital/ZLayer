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
            #[cfg(feature = "docker-compat")]
            docker_socket,
            // `install_wsl` is parsed for forward-compatibility (Windows daemon
            // install is a separate task); the Unix install path ignores it.
            ..
        } => {
            install(
                data_dir,
                *no_start,
                bind,
                jwt_secret.as_deref(),
                *no_swagger,
                #[cfg(feature = "docker-compat")]
                *docker_socket,
            )
            .await
        }
        DaemonAction::Uninstall => uninstall().await,
        DaemonAction::Start => start(data_dir).await,
        DaemonAction::Stop => stop().await,
        DaemonAction::Restart => restart(data_dir).await,
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
    #[cfg(feature = "docker-compat")] _docker_socket: bool,
) -> Result<()> {
    use tokio::process::Command;

    let exe = std::env::current_exe().context("Failed to resolve current executable path")?;
    let exe_str = exe.to_string_lossy();

    let log_dir = crate::cli::default_log_dir(data_dir);
    let run_dir = crate::cli::default_run_dir(data_dir);
    let socket_path = run_dir.join("zlayer.sock");

    // Build ProgramArguments
    // --data-dir is a top-level Cli arg, so it must come BEFORE the subcommand.
    let mut args = vec![
        format!("        <string>{exe_str}</string>"),
        "        <string>--data-dir</string>".to_string(),
        format!("        <string>{}</string>", data_dir.display()),
        "        <string>serve</string>".to_string(),
        "        <string>--bind</string>".to_string(),
        format!("        <string>{bind}</string>"),
        "        <string>--socket</string>".to_string(),
        format!("        <string>{}</string>", socket_path.display()),
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
        // Write spawner PID so the new daemon's cleanup_stale_daemon() won't
        // kill this CLI process while we wait for readiness.
        let spawner_pid_path = data_dir.join("spawner.pid");
        std::fs::write(&spawner_pid_path, std::process::id().to_string()).ok();

        // Clear stale logs so failure diagnostics only show this attempt.
        truncate_daemon_logs(&log_dir);

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
                let _ = std::fs::remove_file(&spawner_pid_path);
                bail!("launchctl failed to load the service: {stderr}");
            }
        }

        print!("Daemon starting...");
        match wait_for_daemon_ready(45).await {
            Ok(()) => {
                let _ = std::fs::remove_file(&spawner_pid_path);
                println!(" started");
                println!("  Stop: zlayer daemon stop");
            }
            Err(e) => {
                let _ = std::fs::remove_file(&spawner_pid_path);
                println!(" failed");
                return Err(e);
            }
        }
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
async fn start(data_dir: &Path) -> Result<()> {
    use tokio::process::Command;

    let (plist_dir, target) = launchd_context()?;
    let path = plist_path_for(&plist_dir);
    let path_str = path.to_string_lossy().to_string();

    if !path.exists() {
        bail!("Daemon not installed. Run `zlayer daemon install` first.");
    }

    // Write spawner PID so the new daemon's cleanup_stale_daemon() won't
    // kill this CLI process while we wait for readiness.
    let spawner_pid_path = data_dir.join("spawner.pid");
    std::fs::write(&spawner_pid_path, std::process::id().to_string()).ok();

    // Clear stale logs so failure diagnostics only show this attempt.
    truncate_daemon_logs(&crate::cli::default_log_dir(data_dir));

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
            let _ = std::fs::remove_file(&spawner_pid_path);
            bail!("Failed to start daemon");
        }
    }

    print!("Daemon starting...");
    match wait_for_daemon_ready(45).await {
        Ok(()) => {
            let _ = std::fs::remove_file(&spawner_pid_path);
            println!(" started");
        }
        Err(e) => {
            let _ = std::fs::remove_file(&spawner_pid_path);
            println!(" failed");
            return Err(e);
        }
    }
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
async fn restart(data_dir: &Path) -> Result<()> {
    use tokio::process::Command;

    let (_plist_dir, target) = launchd_context()?;

    let out = Command::new("launchctl")
        .args(["kickstart", "-k", &format!("{target}/{PLIST_LABEL}")])
        .output()
        .await
        .context("Failed to run launchctl kickstart")?;

    if !out.status.success() {
        stop().await.ok();
        return start(data_dir).await;
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
fn unit_path() -> std::path::PathBuf {
    std::path::PathBuf::from("/etc/systemd/system").join(UNIT_NAME)
}

#[cfg(target_os = "linux")]
fn systemctl_args(base_args: &[&str]) -> Vec<String> {
    base_args.iter().copied().map(ToString::to_string).collect()
}

/// Pick a writable system location for the daemon binary.
///
/// Delegates to [`zlayer_paths::ZLayerDirs::default_binary_dir`] which
/// write-probes `/usr/local/bin` first, then falls back to the `ZLayer`
/// data dir (`/var/lib/zlayer/bin`) which is always writable.
#[cfg(target_os = "linux")]
fn pick_system_binary_path() -> std::path::PathBuf {
    zlayer_paths::ZLayerDirs::default_binary_dir().join("zlayer")
}

/// Create the `zlayer` group, add the invoking user to it, and make the
/// shared build-facing data directories group-writable.
///
/// Lets unprivileged users run `zlayer build` against the system-wide data
/// directory without sudo, which otherwise fails silently when the builder's
/// local-registry import hits EACCES on `/var/lib/zlayer/registry/`.
///
/// Scope is deliberately narrow: only `registry`, `cache`, and `bundles` are
/// group-writable. Secrets, raft state, `admin_password`, and live container
/// runtime state (containers/rootfs/volumes) stay root-only.
///
/// Membership in the `zlayer` group is effectively root-equivalent on the
/// host (a group member can publish a manifest that the daemon later runs as
/// root), same trust model as the `docker` group.
///
/// Failures here are non-fatal — the systemd service install must still
/// succeed even when `groupadd` / `usermod` / `chgrp` aren't available, so
/// this only logs warnings and returns `Ok`.
#[cfg(target_os = "linux")]
#[allow(clippy::too_many_lines, unsafe_code)]
async fn ensure_zlayer_group(data_dir: &Path) -> Result<()> {
    use tokio::process::Command;

    // Only provisioning system-wide paths makes sense as root.
    let is_root = unsafe { libc::geteuid() } == 0;
    if !is_root {
        return Ok(());
    }

    // 1. Create the group if it doesn't exist.
    let getent_ok = Command::new("getent")
        .args(["group", "zlayer"])
        .output()
        .await
        .is_ok_and(|out| out.status.success());
    if !getent_ok {
        match Command::new("groupadd")
            .args(["--system", "zlayer"])
            .output()
            .await
        {
            Ok(out) if out.status.success() => {
                println!("Created system group 'zlayer'.");
            }
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                eprintln!("Warning: groupadd zlayer failed: {}", stderr.trim());
                eprintln!("         Unprivileged users will need sudo to run `zlayer build`.");
                return Ok(());
            }
            Err(e) => {
                eprintln!("Warning: could not run groupadd: {e}");
                return Ok(());
            }
        }
    }

    // 2. Add the invoking user to the group (if sudo'd).
    let sudo_user = std::env::var("SUDO_USER")
        .ok()
        .filter(|u| !u.is_empty() && u != "root");

    if let Some(ref user) = sudo_user {
        // Skip if already a member — avoids redundant usermod noise.
        let already_member = Command::new("id")
            .args(["-nG", user])
            .output()
            .await
            .map(|out| {
                out.status.success()
                    && String::from_utf8_lossy(&out.stdout)
                        .split_whitespace()
                        .any(|g| g == "zlayer")
            })
            .unwrap_or(false);

        if !already_member {
            match Command::new("usermod")
                .args(["-aG", "zlayer", user])
                .output()
                .await
            {
                Ok(out) if out.status.success() => {
                    println!("Added user '{user}' to group 'zlayer'.");
                }
                Ok(out) => {
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    eprintln!(
                        "Warning: usermod -aG zlayer {user} failed: {}",
                        stderr.trim()
                    );
                }
                Err(e) => {
                    eprintln!("Warning: could not run usermod: {e}");
                }
            }
        }
    }

    // 3. Give the installing user write access to the build-facing subdirs.
    //    Only registry/cache/bundles — NOT secrets, raft, containers, rootfs, volumes.
    //
    //    Preferred: chown -R $SUDO_USER:zlayer. UID-based access works in the
    //    user's existing shells immediately, sidestepping the "log out and back
    //    in for supplementary-group membership" trap that the group-only
    //    approach hit. Group membership is still provisioned above so
    //    additional users added later can share these dirs (after their own
    //    re-login). Without SUDO_USER we fall back to chgrp-only.
    let shared_dirs = ["registry", "cache", "bundles"];
    let mut provisioned_any = false;
    for sub in shared_dirs {
        let path = data_dir.join(sub);
        if let Err(e) = std::fs::create_dir_all(&path) {
            eprintln!("Warning: could not create {}: {e}", path.display());
            continue;
        }

        let ownership_ok = if let Some(ref user) = sudo_user {
            Command::new("chown")
                .args(["-R", &format!("{user}:zlayer")])
                .arg(&path)
                .output()
                .await
                .is_ok_and(|out| out.status.success())
        } else {
            Command::new("chgrp")
                .args(["-R", "zlayer"])
                .arg(&path)
                .output()
                .await
                .is_ok_and(|out| out.status.success())
        };
        if !ownership_ok {
            eprintln!(
                "Warning: could not set ownership on {}; unprivileged builds may fail",
                path.display()
            );
            continue;
        }

        // u+rwX,g+rwX → owner and group get read/write; capital X only sets +x
        // on dirs or already-exec files so regular files don't flip executable.
        let _ = Command::new("chmod")
            .args(["-R", "u+rwX,g+rwX"])
            .arg(&path)
            .output()
            .await;

        // setgid on every directory so newly-created files inherit the zlayer
        // group. Applied via `find -type d` so we don't flip g+s on regular
        // files (where SGID has a very different, security-sensitive meaning).
        let _ = Command::new("find")
            .arg(&path)
            .args(["-type", "d", "-exec", "chmod", "g+s", "{}", "+"])
            .output()
            .await;

        provisioned_any = true;
    }

    if provisioned_any {
        if let Some(ref user) = sudo_user {
            println!(
                "Configured build data directories for '{user}': {}",
                shared_dirs.join(", ")
            );
        } else {
            println!(
                "Configured group-writable data directories: {}",
                shared_dirs.join(", ")
            );
        }
    }

    Ok(())
}

#[cfg(target_os = "linux")]
#[allow(clippy::too_many_lines, unsafe_code)]
async fn install(
    data_dir: &Path,
    no_start: bool,
    bind: &str,
    jwt_secret: Option<&str>,
    no_swagger: bool,
    #[cfg(feature = "docker-compat")] docker_socket: bool,
) -> Result<()> {
    use tokio::process::Command;

    let exe = std::env::current_exe().context("Cannot determine zlayer binary path")?;

    // When installing a system-wide service (root), the binary must be in a
    // location accessible to root at runtime.  User home directories are
    // typically mode 0700, so /var/home/user/.local/bin/zlayer is unreachable
    // by the systemd service.  Detect this and copy the binary to
    // /usr/local/bin so the ExecStart path works.
    let exe = {
        let is_root = unsafe { libc::geteuid() } == 0;
        if is_root {
            let in_home = std::env::var("SUDO_USER")
                .ok()
                .and_then(|u| {
                    // Check if exe lives under any home-like prefix
                    let home_prefixes = [format!("/home/{u}"), format!("/var/home/{u}")];
                    home_prefixes.iter().find(|p| exe.starts_with(p)).cloned()
                })
                .is_some()
                || exe.to_string_lossy().contains("/.local/bin/");

            if in_home {
                let system_path = pick_system_binary_path();

                // Stop any running service before overwriting (avoids ETXTBSY)
                let stop_args = systemctl_args(&["stop", UNIT_NAME]);
                let _ = Command::new("systemctl").args(&stop_args).output().await;

                // Unlink destination first — succeeds even while the old process
                // runs (inode stays alive until exit), and the new copy gets a
                // fresh inode that won't conflict with the running text segment.
                let _ = std::fs::remove_file(&system_path);

                std::fs::copy(&exe, &system_path).with_context(|| {
                    format!(
                        "Failed to copy {} to {} (binary is in a user home directory, \
                         inaccessible to the systemd service running as root)",
                        exe.display(),
                        system_path.display()
                    )
                })?;
                // Ensure it's executable
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&system_path, std::fs::Permissions::from_mode(0o755))
                        .ok();
                }
                println!(
                    "Copied binary to {} (original in user home is inaccessible to root service)",
                    system_path.display()
                );
                system_path
            } else {
                exe
            }
        } else {
            exe
        }
    };

    // --data-dir is a top-level Cli arg, so it must come BEFORE the subcommand.
    let mut exec_start = format!(
        "{} --data-dir {} serve --bind {bind}",
        exe.display(),
        data_dir.display()
    );
    if no_swagger {
        exec_start.push_str(" --no-swagger");
    }
    #[cfg(feature = "docker-compat")]
    if docker_socket {
        exec_start.push_str(" --docker-socket");
    }

    let env_line = if let Some(secret) = jwt_secret {
        format!("Environment=ZLAYER_JWT_SECRET={secret}\n")
    } else {
        String::new()
    };

    // Pre-create log directory so tracing-appender can write on first start.
    let log_dir = crate::cli::default_log_dir(data_dir);

    let unit = format!(
        r"[Unit]
Description=ZLayer Container Orchestration Daemon
Documentation=https://zlayer.dev
After=network-online.target
Wants=network-online.target

[Service]
Type=notify
ExecStart={exec_start}
ExecReload=/bin/kill -HUP $MAINPID
TimeoutStartSec=60
Restart=always
RestartSec=5
LimitNOFILE=1048576
LimitNPROC=infinity
LimitCORE=infinity
Delegate=yes
KillMode=process
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

    // Provision the `zlayer` group and make build-facing data directories
    // group-writable. Done before systemctl calls so it still takes effect on
    // systemd-less environments (e.g. WSL distros with systemd disabled),
    // where the subsequent daemon-reload/enable calls may fail.
    ensure_zlayer_group(data_dir).await?;

    let reload_args = systemctl_args(&["daemon-reload"]);
    let reload_out = Command::new("systemctl")
        .args(&reload_args)
        .output()
        .await
        .context("Failed to run systemctl daemon-reload")?;
    if !reload_out.status.success() {
        let stderr = String::from_utf8_lossy(&reload_out.stderr);
        bail!("systemctl daemon-reload failed: {stderr}");
    }

    let enable_args = systemctl_args(&["enable", UNIT_NAME]);
    let enable_out = Command::new("systemctl")
        .args(&enable_args)
        .output()
        .await
        .context("Failed to run systemctl enable")?;
    if !enable_out.status.success() {
        let stderr = String::from_utf8_lossy(&enable_out.stderr);
        bail!("systemctl enable failed: {stderr}");
    }

    // Pre-create log directory so tracing-appender can write on first
    // start before init_daemon() runs.
    if let Err(e) = std::fs::create_dir_all(&log_dir) {
        eprintln!("Warning: could not create {}: {e}", log_dir.display());
    }

    // Install Docker CLI shim when docker-compat is enabled
    #[cfg(feature = "docker-compat")]
    if docker_socket {
        let shim_dir = pick_system_binary_path()
            .parent()
            .unwrap_or(std::path::Path::new("/usr/local/bin"))
            .to_path_buf();
        let shim_path = shim_dir.join("docker");
        let shim_content = "#!/bin/sh\nexec zlayer docker \"$@\"\n";
        match std::fs::write(&shim_path, shim_content) {
            Ok(()) => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&shim_path, std::fs::Permissions::from_mode(0o755))
                        .ok();
                }
                println!(
                    "Installed Docker CLI shim: {} -> zlayer docker",
                    shim_path.display()
                );
            }
            Err(e) => {
                eprintln!(
                    "Warning: could not install Docker shim at {}: {e}",
                    shim_path.display()
                );
            }
        }
    }

    if !no_start {
        // Write spawner PID so the new daemon's cleanup_stale_daemon() won't
        // kill this CLI process while we wait for readiness.
        let spawner_pid_path = data_dir.join("spawner.pid");
        std::fs::write(&spawner_pid_path, std::process::id().to_string()).ok();

        // Clear stale logs so failure diagnostics only show this attempt.
        truncate_daemon_logs(&log_dir);

        let start_args = systemctl_args(&["start", UNIT_NAME]);
        let out = Command::new("systemctl")
            .args(&start_args)
            .output()
            .await
            .context("Failed to start service")?;
        if !out.status.success() {
            let _ = std::fs::remove_file(&spawner_pid_path);
            let context = get_daemon_failure_context();
            bail!("Daemon failed to start.\n{context}");
        }

        print!("Daemon starting...");
        match wait_for_daemon_ready(45).await {
            Ok(()) => {
                let _ = std::fs::remove_file(&spawner_pid_path);
                println!(" started via systemd");
                println!("  Stop: zlayer daemon stop");
            }
            Err(e) => {
                let _ = std::fs::remove_file(&spawner_pid_path);
                println!(" failed");
                return Err(e);
            }
        }
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
async fn start(data_dir: &Path) -> Result<()> {
    use tokio::process::Command;

    let path = unit_path();
    if !path.exists() {
        bail!("Daemon not installed. Run `zlayer daemon install` first.");
    }

    // Write spawner PID so the new daemon's cleanup_stale_daemon() won't
    // kill this CLI process while we wait for readiness.
    let spawner_pid_path = data_dir.join("spawner.pid");
    std::fs::write(&spawner_pid_path, std::process::id().to_string()).ok();

    // Clear stale logs so failure diagnostics only show this attempt.
    truncate_daemon_logs(&crate::cli::default_log_dir(data_dir));

    let args = systemctl_args(&["start", UNIT_NAME]);
    let out = Command::new("systemctl")
        .args(&args)
        .output()
        .await
        .context("Failed to start service")?;
    if !out.status.success() {
        let _ = std::fs::remove_file(&spawner_pid_path);
        let context = get_daemon_failure_context();
        bail!("Daemon failed to start.\n{context}");
    }

    print!("Daemon starting...");
    match wait_for_daemon_ready(45).await {
        Ok(()) => {
            let _ = std::fs::remove_file(&spawner_pid_path);
            println!(" started");
            println!("  Stop: zlayer daemon stop");
        }
        Err(e) => {
            let _ = std::fs::remove_file(&spawner_pid_path);
            println!(" failed");
            return Err(e);
        }
    }
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
async fn restart(_data_dir: &Path) -> Result<()> {
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
// Windows (SCM-managed service)
// ---------------------------------------------------------------------------
//
// After I-1 (`zlayer serve --service` registers with the Service Control
// Manager), I-2..I-5 drive the daemon entirely through SCM:
//
//   - `install`   — `ServiceManager::create_service` registers the daemon
//                   under the service name `ZLayerDaemon`, then starts it
//                   via `Service::start` unless `--no-start` is passed.
//   - `uninstall` — best-effort SCM stop, then `Service::delete` to
//                   deregister.  Foreground-running daemons are unaffected.
//   - `start`     — kept around as a foreground-spawn fallback for users who
//                   haven't run `install` yet; operators using the SCM
//                   service should prefer `sc start ZLayerDaemon` or
//                   (preferred) `zlayer daemon install` then start.
//   - `stop`      — `Service::stop` sends SERVICE_CONTROL_STOP and polls
//                   `query_status` until `Stopped` or 30s timeout.  Falls
//                   back to a message for foreground-running daemons.
//   - `restart`   — `stop` then `start`.
//   - `status`    — `Service::query_status` first, falls back to a TCP
//                   probe via `DaemonClient::try_connect` so a foreground
//                   daemon still reports as "running (foreground)".

/// Windows Win32 error code returned when an SCM operation targets a service
/// that isn't registered.  Named to avoid a `0x424` magic number at each use
/// site; the canonical definition lives in `winerror.h`
/// (`ERROR_SERVICE_DOES_NOT_EXIST`).
#[cfg(target_os = "windows")]
const ERROR_SERVICE_DOES_NOT_EXIST: i32 = 1060;

/// Returns `true` when the given [`windows_service::Error`] wraps the Win32
/// error indicating the target service has not been registered with SCM.
///
/// Callers use this to downgrade "service not found" from a hard failure to
/// a recoverable state (fall back to TCP probe for `status`, print a helpful
/// message for `stop`, no-op for `uninstall`).
#[cfg(target_os = "windows")]
fn is_service_not_found(err: &windows_service::Error) -> bool {
    matches!(
        err,
        windows_service::Error::Winapi(io_err)
            if io_err.raw_os_error() == Some(ERROR_SERVICE_DOES_NOT_EXIST)
    )
}

/// Build the SCM launch-argument vector for `zlayer serve --service ...`.
///
/// Split out so it can be unit-tested without touching SCM: the exact flag
/// set matters for I-2 (if `serve` doesn't recognize one of these, SCM will
/// start the process and it'll exit immediately).
///
/// The JWT secret, if any, is propagated via `--jwt-secret <value>` on the
/// SCM command line. This mirrors the Linux install path, which writes the
/// secret into the systemd unit's `Environment=` line — both are readable
/// by any local admin via `sc qc` / `systemctl cat`, so there's no extra
/// exposure beyond what is already accepted.
#[cfg(target_os = "windows")]
fn build_service_launch_arguments(
    data_dir: &Path,
    bind: &str,
    jwt_secret: Option<&str>,
    no_swagger: bool,
    #[cfg(feature = "docker-compat")] docker_socket: bool,
) -> Vec<std::ffi::OsString> {
    use std::ffi::OsString;

    // `--data-dir` is a top-level Cli arg, so it must come BEFORE the
    // `serve` subcommand — mirrors the Linux/macOS install paths.
    let mut args: Vec<OsString> = vec![
        OsString::from("--data-dir"),
        data_dir.as_os_str().to_os_string(),
        OsString::from("serve"),
        OsString::from("--service"),
        OsString::from("--bind"),
        OsString::from(bind),
    ];
    if no_swagger {
        args.push(OsString::from("--no-swagger"));
    }
    if let Some(secret) = jwt_secret {
        args.push(OsString::from("--jwt-secret"));
        args.push(OsString::from(secret));
    }
    #[cfg(feature = "docker-compat")]
    if docker_socket {
        args.push(OsString::from("--docker-socket"));
    }
    args
}

#[cfg(target_os = "windows")]
async fn install(
    data_dir: &Path,
    no_start: bool,
    bind: &str,
    jwt_secret: Option<&str>,
    no_swagger: bool,
    #[cfg(feature = "docker-compat")] docker_socket: bool,
) -> Result<()> {
    use std::ffi::{OsStr, OsString};
    use windows_service::service::{
        ServiceAccess, ServiceErrorControl, ServiceInfo, ServiceStartType, ServiceType,
    };
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

    // Pre-create the log directory so tracing-appender has a destination on
    // first SCM start — parity with the systemd install flow.
    let log_dir = crate::cli::default_log_dir(data_dir);
    if let Err(e) = std::fs::create_dir_all(&log_dir) {
        eprintln!("Warning: could not create {}: {e}", log_dir.display());
    }

    let exe = std::env::current_exe().context("Failed to resolve current executable path")?;

    let launch_arguments = build_service_launch_arguments(
        data_dir,
        bind,
        jwt_secret,
        no_swagger,
        #[cfg(feature = "docker-compat")]
        docker_socket,
    );

    let service_info = ServiceInfo {
        name: OsString::from(crate::daemon_service::SERVICE_NAME),
        display_name: OsString::from("ZLayer Daemon"),
        service_type: ServiceType::OWN_PROCESS,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: exe.clone(),
        launch_arguments,
        dependencies: vec![],
        // None = LocalSystem. Matches the task spec and the Linux default
        // (root). Enterprise installs that need a dedicated service account
        // can `sc config` after install.
        account_name: None,
        account_password: None,
    };

    let manager = ServiceManager::local_computer(None::<&OsStr>, ServiceManagerAccess::ALL_ACCESS)
        .context(
            "Failed to open Service Control Manager. \
             Run this command from an elevated (Administrator) prompt.",
        )?;

    let service = manager
        .create_service(&service_info, ServiceAccess::ALL_ACCESS)
        .with_context(|| {
            format!(
                "Failed to create Windows Service '{}'. \
                 If the service is already registered, run `zlayer daemon uninstall` first.",
                crate::daemon_service::SERVICE_NAME
            )
        })?;

    // NB: `jwt_secret` is baked into the SCM command line via
    // `build_service_launch_arguments` above — SCM does not inherit env
    // from the installing CLI, so env-passing wouldn't work here.

    println!(
        "Registered Windows Service '{}' (display: 'ZLayer Daemon').",
        crate::daemon_service::SERVICE_NAME
    );
    println!("  Binary:   {}", exe.display());
    println!("  Data dir: {}", data_dir.display());
    println!("  Bind:     {bind}");
    println!("  Account:  LocalSystem");
    println!("  Startup:  AutoStart");
    if no_swagger {
        println!("  Swagger:  disabled");
    }

    if !no_start {
        service.start::<&OsStr>(&[]).with_context(|| {
            format!(
                "Failed to start Windows Service '{}'. \
                 Check the Windows Event Log (Application) for startup errors.",
                crate::daemon_service::SERVICE_NAME
            )
        })?;

        print!("Daemon starting...");
        // Poll the API endpoint until it's reachable — same readiness
        // signal as the systemd/launchd paths.
        match wait_for_daemon_ready(45).await {
            Ok(()) => {
                println!(" started");
                println!("  Stop: zlayer daemon stop");
            }
            Err(e) => {
                println!(" failed");
                return Err(e);
            }
        }
    }

    Ok(())
}

#[cfg(target_os = "windows")]
async fn uninstall() -> Result<()> {
    use std::ffi::OsStr;
    use windows_service::service::ServiceAccess;
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

    // Best-effort stop first so we don't leave an orphaned process after
    // delete. Ignore errors — if it's already stopped (or not registered),
    // `delete` below will pick up the slack.
    let _ = stop().await;

    let manager =
        match ServiceManager::local_computer(None::<&OsStr>, ServiceManagerAccess::CONNECT) {
            Ok(m) => m,
            Err(e) => {
                return Err(e).context(
                    "Failed to open Service Control Manager. \
                 Run this command from an elevated (Administrator) prompt.",
                );
            }
        };

    let service = match manager.open_service(
        crate::daemon_service::SERVICE_NAME,
        ServiceAccess::DELETE | ServiceAccess::STOP | ServiceAccess::QUERY_STATUS,
    ) {
        Ok(s) => s,
        Err(e) if is_service_not_found(&e) => {
            println!(
                "Windows Service '{}' is not registered; nothing to uninstall.",
                crate::daemon_service::SERVICE_NAME
            );
            return Ok(());
        }
        Err(e) => {
            return Err(e).with_context(|| {
                format!(
                    "Failed to open Windows Service '{}'",
                    crate::daemon_service::SERVICE_NAME
                )
            });
        }
    };

    service.delete().with_context(|| {
        format!(
            "Failed to delete Windows Service '{}'",
            crate::daemon_service::SERVICE_NAME
        )
    })?;

    println!(
        "Unregistered Windows Service '{}'.",
        crate::daemon_service::SERVICE_NAME
    );
    Ok(())
}

#[cfg(target_os = "windows")]
async fn start(data_dir: &Path) -> Result<()> {
    // Kept as a foreground-spawn fallback for users who haven't registered
    // the SCM service (e.g. during local development). For an installed
    // service, callers should use `sc start ZLayerDaemon` or re-run
    // `zlayer daemon install` — we don't promote `start` to an SCM call
    // here because `install` already starts the service on registration.
    let bind = "127.0.0.1:3669";
    spawn_daemon_windows(data_dir, bind, None, false).await
}

#[cfg(target_os = "windows")]
async fn stop() -> Result<()> {
    use std::ffi::OsStr;
    use std::time::{Duration, Instant};
    use windows_service::service::{ServiceAccess, ServiceState};
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

    let manager = ServiceManager::local_computer(None::<&OsStr>, ServiceManagerAccess::CONNECT)
        .context(
            "Failed to open Service Control Manager. \
             Run this command from an elevated (Administrator) prompt.",
        )?;

    let service = match manager.open_service(
        crate::daemon_service::SERVICE_NAME,
        ServiceAccess::STOP | ServiceAccess::QUERY_STATUS,
    ) {
        Ok(s) => s,
        Err(e) if is_service_not_found(&e) => {
            println!(
                "Windows Service '{}' is not registered. \
                 If a foreground `zlayer serve` is running, press Ctrl+C in its window.",
                crate::daemon_service::SERVICE_NAME
            );
            return Ok(());
        }
        Err(e) => {
            return Err(e).with_context(|| {
                format!(
                    "Failed to open Windows Service '{}'",
                    crate::daemon_service::SERVICE_NAME
                )
            });
        }
    };

    // Send SERVICE_CONTROL_STOP. If the service is already stopped, SCM
    // returns an error we can ignore — the subsequent poll will confirm.
    match service.stop() {
        Ok(_) => {}
        Err(e) => {
            // Already stopped (ERROR_SERVICE_NOT_ACTIVE = 1062) is a no-op;
            // any other SCM error is worth surfacing.
            const ERROR_SERVICE_NOT_ACTIVE: i32 = 1062;
            let already_stopped = matches!(
                &e,
                windows_service::Error::Winapi(io_err)
                    if io_err.raw_os_error() == Some(ERROR_SERVICE_NOT_ACTIVE)
            );
            if !already_stopped {
                return Err(e).context("Failed to send Stop control to Windows Service");
            }
        }
    }

    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let status = service
            .query_status()
            .context("Failed to query Windows Service status during stop")?;
        if status.current_state == ServiceState::Stopped {
            println!("Daemon stopped.");
            return Ok(());
        }
        if Instant::now() >= deadline {
            println!(
                "Daemon did not reach Stopped state within 30s (current: {:?}). \
                 The service may still be shutting down; check `sc query {}`.",
                status.current_state,
                crate::daemon_service::SERVICE_NAME
            );
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

#[cfg(target_os = "windows")]
async fn restart(data_dir: &Path) -> Result<()> {
    stop().await.ok();
    start(data_dir).await
}

#[cfg(target_os = "windows")]
async fn status(_data_dir: &Path) -> Result<()> {
    use std::ffi::OsStr;
    use windows_service::service::{ServiceAccess, ServiceState};
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

    // Try SCM first. If the service is registered, its state is the
    // authoritative answer — a foreground daemon on a different bind could
    // give a false "running" via TCP probe otherwise.
    let manager_result =
        ServiceManager::local_computer(None::<&OsStr>, ServiceManagerAccess::CONNECT);

    if let Ok(manager) = manager_result {
        match manager.open_service(
            crate::daemon_service::SERVICE_NAME,
            ServiceAccess::QUERY_STATUS,
        ) {
            Ok(service) => {
                let status = service
                    .query_status()
                    .context("Failed to query Windows Service status")?;
                let label = match status.current_state {
                    ServiceState::Running => "Running",
                    ServiceState::Stopped => "Stopped",
                    ServiceState::StartPending => "StartPending",
                    ServiceState::StopPending => "StopPending",
                    ServiceState::Paused => "Paused",
                    ServiceState::PausePending => "PausePending",
                    ServiceState::ContinuePending => "ContinuePending",
                };
                println!(
                    "Daemon: {label} (Windows Service '{}')",
                    crate::daemon_service::SERVICE_NAME
                );
                if let Some(pid) = status.process_id {
                    println!("  PID: {pid}");
                }
                return Ok(());
            }
            Err(e) if is_service_not_found(&e) => {
                // Fall through to the TCP-probe fallback — the user may be
                // running a foreground `zlayer serve` rather than an
                // installed service.
            }
            Err(e) => {
                return Err(e).with_context(|| {
                    format!(
                        "Failed to open Windows Service '{}'",
                        crate::daemon_service::SERVICE_NAME
                    )
                });
            }
        }
    }

    // SCM service not registered (or SCM handle unavailable to non-admin
    // callers). Fall back to the old TCP probe, which correctly detects a
    // foreground-running daemon.
    match zlayer_client::DaemonClient::try_connect().await {
        Ok(Some(_)) => {
            println!("Daemon: running (foreground)");
        }
        _ => {
            println!("Daemon: stopped");
        }
    }
    Ok(())
}

#[cfg(target_os = "windows")]
async fn spawn_daemon_windows(
    data_dir: &Path,
    bind: &str,
    jwt_secret: Option<&str>,
    no_swagger: bool,
) -> Result<()> {
    use tokio::process::Command;

    let exe = std::env::current_exe().context("Failed to resolve current executable path")?;

    let log_dir = crate::cli::default_log_dir(data_dir);
    truncate_daemon_logs(&log_dir);

    // Write spawner PID so the new daemon's cleanup_stale_daemon() won't
    // kill this CLI process while we wait for readiness. Mirrors the Unix
    // behavior in `install`/`start`.
    let spawner_pid_path = data_dir.join("spawner.pid");
    std::fs::write(&spawner_pid_path, std::process::id().to_string()).ok();

    // --data-dir is a top-level Cli arg, so it must come BEFORE the
    // subcommand — matches the Linux systemd unit.
    let mut cmd = Command::new(&exe);
    cmd.arg("--data-dir").arg(data_dir);
    cmd.arg("serve").arg("--daemon").arg("--bind").arg(bind);
    if no_swagger {
        cmd.arg("--no-swagger");
    }
    if let Some(secret) = jwt_secret {
        cmd.env("ZLAYER_JWT_SECRET", secret);
    }
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    // Spawn detached — `zlayer serve` on Windows stays in the foreground
    // of its own process (no fork/daemonize), so we must not `.status()`
    // here or we'd block forever waiting for the daemon to exit.
    let child = cmd
        .spawn()
        .with_context(|| format!("Failed to spawn daemon process: {}", exe.display()))?;
    // Dropping the Child handle without awaiting it leaves the daemon
    // running independently of this CLI process.
    drop(child);

    print!("Daemon starting...");
    match wait_for_daemon_ready(45).await {
        Ok(()) => {
            let _ = std::fs::remove_file(&spawner_pid_path);
            println!(" started");
            println!("  Stop: see `zlayer daemon stop`");
        }
        Err(e) => {
            let _ = std::fs::remove_file(&spawner_pid_path);
            println!(" failed");
            return Err(e);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Log helpers (shared across platforms)
// ---------------------------------------------------------------------------

/// Clear daemon log files so failure diagnostics only show the current attempt.
fn truncate_daemon_logs(log_dir: &std::path::Path) {
    if let Ok(entries) = std::fs::read_dir(log_dir) {
        for entry in entries.flatten() {
            if entry
                .file_name()
                .to_string_lossy()
                .starts_with("daemon.log")
            {
                let _ = std::fs::File::create(entry.path());
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Readiness check (shared across platforms)
// ---------------------------------------------------------------------------

/// Poll for daemon readiness by connecting to the API socket.
///
/// The API socket is the unified readiness signal: the daemon binds it only
/// after all infrastructure phases (overlay, raft, storage, API routes)
/// complete.  A successful health-checked connection means the daemon is
/// fully ready.  This works identically on Linux, macOS, and WSL.
///
/// On timeout, [`get_daemon_failure_context`] auto-surfaces the error from
/// the OS service manager (systemctl/journalctl on Linux, the stderr log on
/// macOS) so the user sees why it failed without running a separate command.
async fn wait_for_daemon_ready(timeout_secs: u64) -> Result<()> {
    let poll_interval = std::time::Duration::from_millis(500);
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);

    loop {
        match zlayer_client::DaemonClient::try_connect().await {
            Ok(Some(_)) => return Ok(()),
            _ if tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(poll_interval).await;
            }
            _ => break,
        }
    }

    let error_context = get_daemon_failure_context();
    bail!("Daemon failed to start within {timeout_secs}s.\n{error_context}");
}

/// Auto-surface error context from the OS service manager when the daemon
/// fails to start.  On Linux, pulls from `systemctl status` and journalctl.
/// On macOS, reads the stderr log from the launchd plist's `StandardErrorPath`.
#[allow(unused_variables)]
fn get_daemon_failure_context() -> String {
    let mut context = String::new();

    #[cfg(target_os = "linux")]
    {
        // systemctl status includes exit code and the last few journal lines
        let args = systemctl_args(&["status", UNIT_NAME]);
        if let Ok(out) = std::process::Command::new("systemctl").args(&args).output() {
            let status = String::from_utf8_lossy(&out.stdout);
            if !status.trim().is_empty() {
                context.push_str(&status);
                context.push('\n');
            }
        }
        // If status output is sparse, pull more lines from the journal
        if context.len() < 100 {
            let jctl_args = vec![
                "--no-pager",
                "-n",
                "30",
                "--since=-2min",
                "-u",
                "zlayer",
                "--output",
                "cat",
            ];
            if let Ok(out) = std::process::Command::new("journalctl")
                .args(&jctl_args)
                .output()
            {
                let journal = String::from_utf8_lossy(&out.stdout);
                if !journal.trim().is_empty() {
                    context.push_str(&journal);
                }
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        // Read the stderr log from the plist's StandardErrorPath
        let log_dir =
            crate::cli::default_log_dir(std::path::Path::new(&crate::cli::default_data_dir()));
        let log_path = log_dir.join("daemon.log");
        if let Ok(content) = std::fs::read_to_string(&log_path) {
            let lines: Vec<&str> = content.lines().collect();
            let start = lines.len().saturating_sub(20);
            context = lines[start..].join("\n");
        }
        if context.trim().is_empty() {
            context = format!(
                "No log output found. Check: {}/daemon.log",
                log_dir.display()
            );
        }
    }

    if context.trim().is_empty() {
        context = "No log output found.".to_string();
    }

    context
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(all(test, target_os = "windows"))]
mod windows_tests {
    use super::*;
    use std::ffi::OsStr;
    use std::path::PathBuf;

    /// `build_service_launch_arguments` must emit `--data-dir <path>` BEFORE
    /// the `serve` subcommand — `--data-dir` is a top-level Cli arg and the
    /// service would fail to start if the order slipped.
    #[test]
    fn launch_arguments_puts_data_dir_before_serve() {
        let data_dir = PathBuf::from(r"C:\ProgramData\zlayer");
        let args = build_service_launch_arguments(
            &data_dir,
            "0.0.0.0:3669",
            None,
            false,
            #[cfg(feature = "docker-compat")]
            false,
        );
        let data_dir_pos = args
            .iter()
            .position(|a| a == OsStr::new("--data-dir"))
            .expect("--data-dir flag present");
        let serve_pos = args
            .iter()
            .position(|a| a == OsStr::new("serve"))
            .expect("serve subcommand present");
        assert!(
            data_dir_pos < serve_pos,
            "--data-dir must come before `serve` (got args: {args:?})"
        );
        // `--data-dir` must be immediately followed by its value.
        assert_eq!(
            args[data_dir_pos + 1].as_os_str(),
            data_dir.as_os_str(),
            "--data-dir value mismatch"
        );
    }

    /// `--service` must be present so SCM spawn enters the Windows Service
    /// dispatcher (I-1) rather than the foreground `serve` path.
    #[test]
    fn launch_arguments_includes_service_flag() {
        let args = build_service_launch_arguments(
            Path::new(r"C:\data"),
            "127.0.0.1:3669",
            None,
            false,
            #[cfg(feature = "docker-compat")]
            false,
        );
        assert!(
            args.iter().any(|a| a == OsStr::new("--service")),
            "--service flag must be present (got args: {args:?})"
        );
    }

    /// `--bind <addr>` must be forwarded verbatim.
    #[test]
    fn launch_arguments_forwards_bind() {
        let args = build_service_launch_arguments(
            Path::new(r"C:\data"),
            "10.0.0.5:4242",
            None,
            false,
            #[cfg(feature = "docker-compat")]
            false,
        );
        let bind_pos = args
            .iter()
            .position(|a| a == OsStr::new("--bind"))
            .expect("--bind flag present");
        assert_eq!(args[bind_pos + 1].as_os_str(), OsStr::new("10.0.0.5:4242"));
    }

    /// `--no-swagger` is only passed through when requested.
    #[test]
    fn launch_arguments_no_swagger_toggle() {
        let without = build_service_launch_arguments(
            Path::new(r"C:\data"),
            "127.0.0.1:3669",
            None,
            false,
            #[cfg(feature = "docker-compat")]
            false,
        );
        assert!(!without.iter().any(|a| a == OsStr::new("--no-swagger")));

        let with = build_service_launch_arguments(
            Path::new(r"C:\data"),
            "127.0.0.1:3669",
            None,
            true,
            #[cfg(feature = "docker-compat")]
            false,
        );
        assert!(with.iter().any(|a| a == OsStr::new("--no-swagger")));
    }

    /// `--jwt-secret <value>` is forwarded verbatim when provided, and
    /// absent otherwise. SCM does not inherit env from the CLI, so the
    /// secret has to ride the command line.
    #[test]
    fn launch_arguments_jwt_secret_round_trip() {
        let without = build_service_launch_arguments(
            Path::new(r"C:\data"),
            "127.0.0.1:3669",
            None,
            false,
            #[cfg(feature = "docker-compat")]
            false,
        );
        assert!(!without.iter().any(|a| a == OsStr::new("--jwt-secret")));

        let with = build_service_launch_arguments(
            Path::new(r"C:\data"),
            "127.0.0.1:3669",
            Some("super-secret-token"),
            false,
            #[cfg(feature = "docker-compat")]
            false,
        );
        let pos = with
            .iter()
            .position(|a| a == OsStr::new("--jwt-secret"))
            .expect("--jwt-secret flag present");
        assert_eq!(with[pos + 1].as_os_str(), OsStr::new("super-secret-token"));
    }

    /// `is_service_not_found` must identify `ERROR_SERVICE_DOES_NOT_EXIST`
    /// so the uninstall/stop/status fallbacks fire instead of erroring out.
    #[test]
    fn is_service_not_found_matches_1060() {
        let io_err = std::io::Error::from_raw_os_error(ERROR_SERVICE_DOES_NOT_EXIST);
        let err = windows_service::Error::Winapi(io_err);
        assert!(is_service_not_found(&err));
    }

    /// Any other Win32 error must NOT be misread as "service not found".
    #[test]
    fn is_service_not_found_rejects_other_errors() {
        // ERROR_ACCESS_DENIED (5) is the other common SCM error and must
        // propagate rather than be swallowed as a no-op.
        let io_err = std::io::Error::from_raw_os_error(5);
        let err = windows_service::Error::Winapi(io_err);
        assert!(!is_service_not_found(&err));
    }
}
