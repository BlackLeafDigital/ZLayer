//! `zlayer docker install` / `zlayer docker uninstall` — one-shot setup to
//! make the `docker` and `docker compose` CLIs transparently route through
//! `ZLayer`.
//!
//! On each supported platform the install flow:
//!   1. Reconfigures the zlayer daemon service (systemd / launchd / SCM)
//!      to expose the Docker Engine API socket.
//!   2. Drops `docker` and `docker-compose` shims on `PATH` that exec
//!      `zlayer docker ...`.
//!   3. Writes `DOCKER_HOST` + `DOCKER_BUILDKIT=0` to the user's shell
//!      profile (Unix) or user environment (Windows).
//!   4. Optionally offers to symlink `/var/run/docker.sock` to the `ZLayer`
//!      Docker socket (Unix only — named pipes on Windows cannot be
//!      symlinked).
//!
//! Uninstall reverses each step and restores any backed-up files.

use anyhow::Result;
use clap::Args;

#[cfg(target_os = "linux")]
#[path = "install_linux.rs"]
mod platform_impl;

#[cfg(target_os = "macos")]
#[path = "install_macos.rs"]
mod platform_impl;

#[cfg(target_os = "windows")]
#[path = "install_windows.rs"]
mod platform_impl;

/// Arguments for `zlayer docker install`.
#[derive(Debug, Args)]
#[allow(clippy::struct_excessive_bools)]
pub struct InstallArgs {
    /// Override the path (Unix) or named-pipe name (Windows) at which the
    /// Docker-compatible API socket will be exposed. Defaults to the
    /// platform standard (see `zlayer_paths::ZLayerDirs::default_docker_socket_path`).
    #[arg(long, value_name = "PATH")]
    pub socket_path: Option<String>,

    /// Skip installing the `docker` and `docker-compose` shims.
    #[arg(long)]
    pub no_shim: bool,

    /// Skip writing `DOCKER_HOST` and `DOCKER_BUILDKIT` to the user's
    /// shell profile (Unix) or user environment (Windows).
    #[arg(long)]
    pub no_env: bool,

    /// Skip regenerating the daemon service manifest and restarting the
    /// daemon. If you use this, you must manually enable the Docker
    /// socket (e.g. run `zlayer serve --docker-socket` or reinstall the
    /// daemon with `zlayer daemon install --docker-socket`).
    #[arg(long)]
    pub no_daemon_restart: bool,

    /// Replace an existing `/var/run/docker.sock` without prompting.
    /// The existing socket (or file at that path) is backed up to
    /// `/var/run/docker.sock.zlayer-backup-<unix-ts>`.
    /// Requires root on Linux / macOS. Has no effect on Windows.
    #[arg(long)]
    pub replace_docker_sock: bool,

    /// Skip the interactive prompt about symlinking
    /// `/var/run/docker.sock`. Implies "do not symlink" unless
    /// `--replace-docker-sock` is also set.
    #[arg(long)]
    pub skip_symlink_prompt: bool,

    /// Proceed even if precheck warnings would normally abort (e.g. real
    /// Docker installation detected, existing shims with unknown
    /// content).
    #[arg(long)]
    pub force: bool,
}

/// Arguments for `zlayer docker uninstall`.
#[derive(Debug, Args)]
pub struct UninstallArgs {
    /// Leave the daemon service's `--docker-socket` flag in place. By
    /// default, uninstall regenerates the service manifest without the
    /// flag and restarts the daemon.
    #[arg(long)]
    pub keep_daemon_socket: bool,

    /// Do not restore `.zlayer-backup` files. By default, any backup
    /// made during install (e.g. a user-supplied `/usr/local/bin/docker`
    /// that was displaced) is restored in place.
    #[arg(long)]
    pub keep_backups: bool,

    /// Proceed even if parts of the install appear already removed.
    #[arg(long)]
    pub force: bool,
}

/// Handle `zlayer docker install`.
///
/// # Errors
///
/// Returns an error when any of the install steps fail. Steps after a
/// failure are not executed; the caller is expected to surface the error
/// and exit non-zero.
pub async fn handle_install(args: InstallArgs) -> Result<()> {
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    {
        platform_impl::handle_install(args).await
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        let _ = args;
        anyhow::bail!("`zlayer docker install` is not supported on this platform");
    }
}

/// Handle `zlayer docker uninstall`.
///
/// # Errors
///
/// Returns an error when any of the uninstall steps fail. Unlike install,
/// the uninstall flow is best-effort: a failure in one step does not
/// prevent later steps from running (implemented per platform).
pub async fn handle_uninstall(args: UninstallArgs) -> Result<()> {
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    {
        platform_impl::handle_uninstall(args).await
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        let _ = args;
        anyhow::bail!("`zlayer docker uninstall` is not supported on this platform");
    }
}
