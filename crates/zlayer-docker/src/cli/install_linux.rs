//! Linux install / uninstall implementation.
//!
//! Flow:
//! 1. Reconfigure `/etc/systemd/system/zlayer.service` to add
//!    `--docker-socket --docker-socket-path <PATH>` and restart.
//! 2. Install `/usr/local/bin/docker` and `/usr/local/bin/docker-compose`
//!    shell shims (fallback to `{data_dir}/bin` when `/usr/local/bin`
//!    isn't writable).
//! 3. Write `DOCKER_HOST` + `DOCKER_BUILDKIT=0` into `~/.zlayer/env.sh`
//!    plus a guarded block in shell rc files.
//! 4. Offer to symlink `/var/run/docker.sock` to the `ZLayer` socket;
//!    non-interactive skips unless `--replace-docker-sock`.

use anyhow::Result;

use super::{InstallArgs, UninstallArgs};

pub async fn handle_install(args: InstallArgs) -> Result<()> {
    use anyhow::Context;
    use std::path::Path;

    let socket_path = resolve_socket_path(&args);
    println!("ZLayer Docker compatibility install");
    println!("  socket path: {}", socket_path.display());

    // 1. Ensure parent dir of the socket exists (systemd will write into it).
    if let Some(parent) = socket_path.parent() {
        if !parent.exists() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
    }

    // 2. Reconfigure daemon service (unless --no-daemon-restart).
    if args.no_daemon_restart {
        println!("  [skip] daemon reconfig (--no-daemon-restart)");
        println!("         you must restart the daemon with `--docker-socket` manually");
    } else {
        match crate::cli::daemon_reconfig::enable_docker_socket(&socket_path, true).await {
            Ok(()) => println!("  [ok] daemon service reconfigured and restarted"),
            Err(e) => {
                // Surface the DaemonNotInstalled message clearly.
                if let Some(crate::cli::daemon_reconfig::ReconfigError::DaemonNotInstalled) =
                    e.downcast_ref()
                {
                    anyhow::bail!(
                        "zlayer daemon is not installed as a service; run `sudo zlayer daemon install` first, then re-run `zlayer docker install`"
                    );
                }
                return Err(e);
            }
        }
    }

    // 3. Install shims (docker + docker-compose).
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

    // 4. Install env snippet in shell profiles.
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

    // 5. Offer /var/run/docker.sock symlink.
    let docker_sock = Path::new("/var/run/docker.sock");
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
            println!(
                "  [skip] {docker_sock}: {reason}",
                docker_sock = docker_sock.display()
            );
        }
    }

    println!();
    println!("Done. Verify with:   docker ps");
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

fn resolve_socket_path(args: &InstallArgs) -> std::path::PathBuf {
    let p = args
        .socket_path
        .clone()
        .unwrap_or_else(zlayer_paths::ZLayerDirs::default_docker_socket_path);
    std::path::PathBuf::from(p)
}

pub async fn handle_uninstall(args: UninstallArgs) -> Result<()> {
    use std::path::Path;

    println!("ZLayer Docker compatibility uninstall");
    let socket_path = zlayer_paths::ZLayerDirs::default_docker_socket_path();
    let socket_path_pb = std::path::PathBuf::from(&socket_path);

    // 1. Remove /var/run/docker.sock symlink (and optionally restore backup).
    let docker_sock = Path::new("/var/run/docker.sock");
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

    // 2. Remove env snippet.
    let touched = crate::cli::env_profile::uninstall_env_snippet()?;
    for p in &touched {
        println!("  [ok] cleaned {}", p.display());
    }

    // 3. Remove shims.
    let shim_dir = zlayer_paths::ZLayerDirs::default_binary_dir();
    uninstall_shim_report(&shim_dir, "docker", "zlayer docker", !args.keep_backups)?;
    uninstall_shim_report(
        &shim_dir,
        "docker-compose",
        "zlayer docker compose",
        !args.keep_backups,
    )?;

    // 4. Disable --docker-socket on the daemon service (unless --keep-daemon-socket).
    if args.keep_daemon_socket {
        println!("  [skip] daemon reconfig (--keep-daemon-socket)");
    } else {
        match crate::cli::daemon_reconfig::disable_docker_socket(true).await {
            Ok(()) => println!(
                "  [ok] daemon service reconfigured (--docker-socket removed) and restarted"
            ),
            Err(e) => {
                // Tolerate DaemonNotInstalled during uninstall.
                if let Some(crate::cli::daemon_reconfig::ReconfigError::DaemonNotInstalled) =
                    e.downcast_ref()
                {
                    println!("  [skip] daemon is not installed as a service");
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
