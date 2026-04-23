//! Windows install / uninstall implementation.
//!
//! Flow:
//! 1. Reconfigure the `ZLayerDaemon` Windows service to add
//!    `--docker-socket --docker-socket-path \\.\pipe\zlayer-docker` and
//!    restart.
//! 2. Install `%ProgramData%\ZLayer\bin\docker.cmd` and
//!    `docker-compose.cmd` shims, and ensure that directory is on the
//!    user's `PATH`.
//! 3. Write `DOCKER_HOST=npipe:////./pipe/zlayer-docker` and
//!    `DOCKER_BUILDKIT=0` to `HKCU\Environment`; broadcast
//!    `WM_SETTINGCHANGE` so running Explorer and cmd sessions re-read
//!    the environment.
//! 4. No `/var/run/docker.sock` symlink on Windows: named pipes cannot
//!    be symlinked. Users must rely on `DOCKER_HOST`.

use anyhow::Result;

use super::{InstallArgs, UninstallArgs};

pub async fn handle_install(args: InstallArgs) -> Result<()> {
    use anyhow::Context;

    let pipe_path = resolve_pipe_path(&args);
    println!("ZLayer Docker compatibility install (Windows)");
    println!("  pipe: {}", pipe_path.display());

    if args.no_daemon_restart {
        println!("  [skip] daemon reconfig (--no-daemon-restart)");
    } else {
        match crate::cli::daemon_reconfig::enable_docker_socket(&pipe_path, true).await {
            Ok(()) => println!("  [ok] ZLayerDaemon service reconfigured and restarted"),
            Err(e) => {
                if let Some(crate::cli::daemon_reconfig::ReconfigError::DaemonNotInstalled) =
                    e.downcast_ref()
                {
                    anyhow::bail!(
                        "ZLayerDaemon Windows service is not installed; run `zlayer daemon install` (elevated) first, then re-run `zlayer docker install`"
                    );
                }
                return Err(e);
            }
        }
    }

    if args.no_shim {
        println!("  [skip] docker/docker-compose .cmd shims (--no-shim)");
    } else {
        let shim_dir = zlayer_paths::ZLayerDirs::default_binary_dir();
        if !shim_dir.exists() {
            std::fs::create_dir_all(&shim_dir)
                .with_context(|| format!("failed to create {}", shim_dir.display()))?;
        }
        install_shim_report(&shim_dir, "docker", "zlayer docker")?;
        install_shim_report(&shim_dir, "docker-compose", "zlayer docker compose")?;
        match crate::cli::env_profile_windows::ensure_dir_on_user_path(&shim_dir) {
            Ok(true) => println!(
                "  [ok] added {} to user PATH (re-open terminals to pick up)",
                shim_dir.display()
            ),
            Ok(false) => println!("  [ok] {} is already on user PATH", shim_dir.display()),
            Err(e) => return Err(e),
        }
    }

    if args.no_env {
        println!("  [skip] DOCKER_HOST user env (--no-env)");
    } else {
        let docker_host = format!("npipe:////./pipe/{}", pipe_name(&pipe_path));
        crate::cli::env_profile_windows::install_user_env("DOCKER_HOST", &docker_host)?;
        crate::cli::env_profile_windows::install_user_env("DOCKER_BUILDKIT", "0")?;
        println!("  [ok] set HKCU\\Environment\\DOCKER_HOST = {docker_host}");
        println!("  [ok] set HKCU\\Environment\\DOCKER_BUILDKIT = 0");
    }

    // Windows note on /var/run/docker.sock equivalent:
    println!(
        "  [skip] named-pipe path cannot be symlinked; docker CLI will pick up DOCKER_HOST instead"
    );

    println!();
    println!("Done. Open a new PowerShell or cmd window and run:   docker ps");
    Ok(())
}

pub async fn handle_uninstall(args: UninstallArgs) -> Result<()> {
    println!("ZLayer Docker compatibility uninstall (Windows)");

    if !args.no_env_preserved() {
        crate::cli::env_profile_windows::uninstall_user_env("DOCKER_HOST")?;
        crate::cli::env_profile_windows::uninstall_user_env("DOCKER_BUILDKIT")?;
        println!("  [ok] removed HKCU\\Environment\\DOCKER_HOST");
        println!("  [ok] removed HKCU\\Environment\\DOCKER_BUILDKIT");
    }

    let shim_dir = zlayer_paths::ZLayerDirs::default_binary_dir();
    uninstall_shim_report(&shim_dir, "docker", "zlayer docker", !args.keep_backups)?;
    uninstall_shim_report(
        &shim_dir,
        "docker-compose",
        "zlayer docker compose",
        !args.keep_backups,
    )?;
    if let Ok(true) = crate::cli::env_profile_windows::remove_dir_from_user_path(&shim_dir) {
        println!("  [ok] removed {} from user PATH", shim_dir.display());
    }

    if args.keep_daemon_socket {
        println!("  [skip] daemon reconfig (--keep-daemon-socket)");
    } else {
        match crate::cli::daemon_reconfig::disable_docker_socket(true).await {
            Ok(()) => println!("  [ok] ZLayerDaemon service reconfigured and restarted"),
            Err(e) => {
                if let Some(crate::cli::daemon_reconfig::ReconfigError::DaemonNotInstalled) =
                    e.downcast_ref()
                {
                    println!("  [skip] ZLayerDaemon is not installed");
                } else if !args.force {
                    return Err(e);
                } else {
                    eprintln!("  [warn] daemon reconfig failed: {e:?} (continuing due to --force)");
                }
            }
        }
    }

    println!();
    println!("Done.");
    Ok(())
}

trait UninstallArgsExt {
    fn no_env_preserved(&self) -> bool;
}

impl UninstallArgsExt for UninstallArgs {
    fn no_env_preserved(&self) -> bool {
        // UninstallArgs has no --no-env flag — env removal is always on
        // unless the user manually backs up their registry first. This
        // hook exists as an extension point; currently returns false so
        // env vars are always removed.
        false
    }
}

fn install_shim_report(
    shim_dir: &std::path::Path,
    name: &str,
    target_invocation: &str,
) -> Result<()> {
    use crate::shim::{install_shim, ShimInstalled};
    match install_shim(shim_dir, name, target_invocation)? {
        ShimInstalled::Fresh(p) => {
            println!("  [ok] installed {} -> {target_invocation}", p.display());
        }
        ShimInstalled::ReplacedExisting { shim, backup } => {
            println!(
                "  [ok] backed up existing {} to {} and installed -> {target_invocation}",
                shim.display(),
                backup.display()
            );
        }
        ShimInstalled::AlreadyOurs(p) => {
            println!("  [ok] {} already installed as our shim", p.display());
        }
    }
    Ok(())
}

fn uninstall_shim_report(
    shim_dir: &std::path::Path,
    name: &str,
    target_invocation: &str,
    restore_backup: bool,
) -> Result<()> {
    use crate::shim::{uninstall_shim, ShimUninstalled};
    match uninstall_shim(shim_dir, name, target_invocation, restore_backup)? {
        ShimUninstalled::NotPresent => {
            println!("  [skip] shim {name}.cmd: not present or not ours");
        }
        ShimUninstalled::Removed(p) => {
            println!("  [ok] removed shim {}", p.display());
        }
        ShimUninstalled::RemovedAndRestored {
            shim,
            restored_from,
        } => {
            println!(
                "  [ok] removed shim {} and restored backup from {}",
                shim.display(),
                restored_from.display()
            );
        }
    }
    Ok(())
}

fn resolve_pipe_path(args: &InstallArgs) -> std::path::PathBuf {
    let p = args
        .socket_path
        .clone()
        .unwrap_or_else(zlayer_paths::ZLayerDirs::default_docker_socket_path);
    std::path::PathBuf::from(p)
}

/// Extract the pipe name ("zlayer-docker") from `\\.\pipe\zlayer-docker`.
fn pipe_name(path: &std::path::Path) -> String {
    path.file_name().map_or_else(
        || "zlayer-docker".to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}
