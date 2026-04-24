//! macOS install / uninstall implementation.
//!
//! Flow:
//! 1. Reconfigure the launchd plist
//!    (`~/Library/LaunchAgents/com.zlayer.daemon.plist` for user; or
//!    `/Library/LaunchDaemons/com.zlayer.daemon.plist` for system) to
//!    add `--docker-socket --docker-socket-path <PATH>` and reload.
//! 2. Install `/usr/local/bin/docker` + `/usr/local/bin/docker-compose`
//!    shell shims (fallback to `{data_dir}/bin`).
//! 3. Write `DOCKER_HOST` + `DOCKER_BUILDKIT=0` into `~/.zlayer/env.sh`
//!    plus a guarded block in shell rc files.
//! 4. Offer to symlink `/var/run/docker.sock` to the `ZLayer` socket, but
//!    only when running as root; otherwise print guidance to re-run
//!    with `sudo` or set `DOCKER_HOST` manually.

use anyhow::Result;

use super::{InstallArgs, UninstallArgs};

pub async fn handle_install(args: InstallArgs) -> Result<()> {
    use anyhow::Context;
    use std::path::Path;

    let socket_path = resolve_socket_path(&args);
    println!("ZLayer Docker compatibility install (macOS)");
    println!("  socket path: {}", socket_path.display());

    if let Some(parent) = socket_path.parent() {
        if !parent.exists() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
    }

    if args.no_daemon_restart {
        println!("  [skip] daemon reconfig (--no-daemon-restart)");
    } else {
        match crate::cli::daemon_reconfig::enable_docker_socket(&socket_path, true).await {
            Ok(()) => println!("  [ok] daemon service (launchd) reconfigured and reloaded"),
            Err(e) => {
                if let Some(crate::cli::daemon_reconfig::ReconfigError::DaemonNotInstalled) =
                    e.downcast_ref()
                {
                    anyhow::bail!(
                        "zlayer daemon is not installed as a launchd service; run `zlayer daemon install` first, then re-run `zlayer docker install`"
                    );
                }
                return Err(e);
            }
        }
    }

    if args.no_shim {
        println!("  [skip] docker/docker-compose shims (--no-shim)");
    } else {
        let shim_dir = zlayer_paths::ZLayerDirs::default_binary_dir();
        if !shim_dir.exists() {
            std::fs::create_dir_all(&shim_dir)
                .with_context(|| format!("failed to create {}", shim_dir.display()))?;
        }
        install_shim_report(&shim_dir, "docker", "zlayer docker")?;
        install_shim_report(&shim_dir, "docker-compose", "zlayer docker compose")?;
    }

    if args.no_env {
        println!("  [skip] shell env snippet (--no-env)");
    } else {
        let socket_uri = format!("unix://{}", socket_path.display());
        let written = crate::cli::env_profile::install_env_snippet(&socket_uri)?;
        for p in &written {
            println!("  [ok] wrote/appended {}", p.display());
        }
        println!("       run `source ~/.zlayer/env.sh` or open a new shell to pick up DOCKER_HOST");
    }

    let docker_sock = Path::new("/var/run/docker.sock");
    if zlayer_paths::is_root() {
        match crate::cli::sock_symlink::offer_symlink(
            &socket_path,
            docker_sock,
            args.skip_symlink_prompt,
            args.replace_docker_sock,
        )? {
            crate::cli::sock_symlink::SymlinkAction::Created { at, target } => {
                println!("  [ok] symlinked {} -> {}", at.display(), target.display());
            }
            crate::cli::sock_symlink::SymlinkAction::ReplacedWithBackup { at, target, backup } => {
                println!(
                    "  [ok] backed up existing {} to {} and symlinked -> {}",
                    at.display(),
                    backup.display(),
                    target.display()
                );
            }
            crate::cli::sock_symlink::SymlinkAction::AlreadyOurs(p) => {
                println!("  [ok] {} already symlinked to us", p.display());
            }
            crate::cli::sock_symlink::SymlinkAction::Skipped(reason) => {
                println!("  [skip] {}: {reason}", docker_sock.display());
            }
        }
    } else {
        println!(
            "  [skip] /var/run/docker.sock: non-root install; re-run with `sudo` to symlink, or rely on DOCKER_HOST"
        );
    }

    println!();
    println!("Done. Verify with:   docker ps");
    Ok(())
}

pub async fn handle_uninstall(args: UninstallArgs) -> Result<()> {
    use std::path::{Path, PathBuf};

    println!("ZLayer Docker compatibility uninstall (macOS)");
    let socket_path = zlayer_paths::ZLayerDirs::default_docker_socket_path();
    let socket_path_pb = PathBuf::from(&socket_path);

    let docker_sock = Path::new("/var/run/docker.sock");
    if zlayer_paths::is_root() {
        match crate::cli::sock_symlink::remove_symlink(
            &socket_path_pb,
            docker_sock,
            !args.keep_backups,
        )? {
            crate::cli::sock_symlink::RemoveAction::Removed(p) => {
                println!("  [ok] removed symlink {}", p.display());
            }
            crate::cli::sock_symlink::RemoveAction::RemovedAndRestored { at, restored_from } => {
                println!(
                    "  [ok] removed symlink {} and restored backup from {}",
                    at.display(),
                    restored_from.display()
                );
            }
            crate::cli::sock_symlink::RemoveAction::Skipped => {
                println!(
                    "  [skip] {} is not a zlayer-managed symlink",
                    docker_sock.display()
                );
            }
        }
    } else {
        println!("  [skip] /var/run/docker.sock: non-root; nothing for user scope");
    }

    let touched = crate::cli::env_profile::uninstall_env_snippet()?;
    for p in &touched {
        println!("  [ok] cleaned {}", p.display());
    }

    let shim_dir = zlayer_paths::ZLayerDirs::default_binary_dir();
    uninstall_shim_report(&shim_dir, "docker", "zlayer docker", !args.keep_backups)?;
    uninstall_shim_report(
        &shim_dir,
        "docker-compose",
        "zlayer docker compose",
        !args.keep_backups,
    )?;

    if args.keep_daemon_socket {
        println!("  [skip] daemon reconfig (--keep-daemon-socket)");
    } else {
        match crate::cli::daemon_reconfig::disable_docker_socket(true).await {
            Ok(()) => println!("  [ok] daemon service (launchd) reconfigured and reloaded"),
            Err(e) => {
                if let Some(crate::cli::daemon_reconfig::ReconfigError::DaemonNotInstalled) =
                    e.downcast_ref()
                {
                    println!("  [skip] daemon is not installed as a launchd service");
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

fn install_shim_report(
    shim_dir: &std::path::Path,
    name: &str,
    target_invocation: &str,
) -> Result<()> {
    use crate::shim::{install_shim, ShimInstalled};
    match install_shim(shim_dir, name, target_invocation)? {
        ShimInstalled::Fresh(p) => {
            println!(
                "  [ok] installed shim {} -> {target_invocation}",
                p.display()
            );
        }
        ShimInstalled::ReplacedExisting { shim, backup } => {
            println!(
                "  [ok] backed up existing {} to {} and installed shim -> {target_invocation}",
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
            println!("  [skip] shim {name}: not present or not ours");
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

fn resolve_socket_path(args: &InstallArgs) -> std::path::PathBuf {
    let p = args
        .socket_path
        .clone()
        .unwrap_or_else(zlayer_paths::ZLayerDirs::default_docker_socket_path);
    std::path::PathBuf::from(p)
}
