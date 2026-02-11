//! ZLayer Runtime CLI
//!
//! The main entry point for the ZLayer container orchestration runtime.
//! Provides commands for deploying, joining, and managing containerized services.
//!
//! # Feature Flags
//!
//! - `full` (default): Enable all runtime capabilities
//! - `docker`: Docker runtime support
//! - `wasm`: WebAssembly runtime support
//! - `s3`: S3 storage backend
//! - `persistent`: Persistent scheduler/registry storage
//! - `observability`: Axum metrics and trace propagation

mod cli;
mod commands;
mod config;
pub mod daemon;
pub mod daemon_client;
#[allow(dead_code)]
mod deploy_tui;
mod util;

use anyhow::{Context, Result};
use clap::Parser;
use std::io::IsTerminal;
#[cfg(not(target_os = "macos"))]
use std::os::unix::io::AsRawFd;

use cli::{Cli, Commands};
use zlayer_observability::{
    init_observability, LogFormat, LogLevel, LoggingConfig, ObservabilityConfig,
};

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Daemonize BEFORE any threads exist (before tokio runtime or observability init).
    // This is critical: daemon() calls fork(), which is unsafe after threads are spawned.
    let should_daemon = match &cli.command {
        Commands::Serve { daemon, .. } => *daemon,
        _ => false,
    };

    if should_daemon {
        #[cfg(target_os = "macos")]
        {
            // macOS: use launchd for proper daemon management.
            // Generates a plist, installs it, and loads via launchctl.
            install_launchd_service(&cli)?;
            println!("zlayer daemon registered with launchd and started.");
            println!("  Logs: /var/log/zlayer/daemon.log");
            println!("  Stop: launchctl bootout system/com.zlayer.daemon");
            std::process::exit(0);
        }

        #[cfg(not(target_os = "macos"))]
        {
            use std::fs::{self, OpenOptions};

            // Create directories (idempotent via create_dir_all)
            fs::create_dir_all("/var/run/zlayer").context("Failed to create /var/run/zlayer")?;
            fs::create_dir_all("/var/log/zlayer").context("Failed to create /var/log/zlayer")?;

            // Open log file BEFORE forking so errors are visible on the caller's terminal
            let log_file = OpenOptions::new()
                .create(true)
                .append(true)
                .open("/var/log/zlayer/daemon.log")
                .context("Failed to open /var/log/zlayer/daemon.log")?;

            let log_fd = log_file.as_raw_fd();

            // Fork + setsid + chdir to /.
            // nochdir=false  -> chdir to /
            // noclose=true   -> keep fds open (we redirect them ourselves below)
            nix::unistd::daemon(false, true).context("Failed to daemonize")?;

            // We are now the daemon child process.
            // Redirect stdout (fd 1) and stderr (fd 2) to the log file,
            // then close stdin (fd 0).
            //
            // Using libc::dup2/close because nix 0.31's dup2() takes AsFd + &mut OwnedFd
            // which doesn't support targeting specific numbered fds (0, 1, 2) cleanly.
            // SAFETY: No threads exist yet. We own these fds and the log_fd is valid.
            unsafe {
                libc::dup2(log_fd, libc::STDOUT_FILENO);
                libc::dup2(log_fd, libc::STDERR_FILENO);
                libc::close(libc::STDIN_FILENO);
            }

            // Drop the original File handle. The underlying file description stays alive
            // because fds 1 and 2 now reference it (dup2 creates independent references).
            drop(log_file);

            // Write PID file with ONLY the numeric PID
            fs::write("/var/run/zlayer/zlayer.pid", std::process::id().to_string())
                .context("Failed to write PID file")?;
        }
    }

    // Configure observability based on verbosity and environment
    let (log_level, filter_directives) = match cli.verbose {
        0 => (
            LogLevel::Warn,
            Some(
                "runtime=info,zlayer_agent=warn,zlayer_overlay=warn,zlayer_proxy=warn,\
                 zlayer_init_actions=warn,zlayer_scheduler=warn,zlayer_api=warn,warn"
                    .to_string(),
            ),
        ),
        1 => (LogLevel::Info, None),  // -v: global info (old default)
        2 => (LogLevel::Debug, None), // -vv: debug
        _ => (LogLevel::Trace, None), // -vvv: trace
    };

    // Use pretty format for terminals, JSON for piped output
    let log_format = if std::io::stdout().is_terminal() {
        LogFormat::Pretty
    } else {
        LogFormat::Json
    };

    let config = ObservabilityConfig {
        logging: LoggingConfig {
            level: log_level,
            format: log_format,
            filter_directives,
            ..Default::default()
        },
        ..Default::default()
    };

    // Initialize observability - hold guards for application lifetime
    let _guards = init_observability(&config).context("Failed to initialize observability")?;

    // Run the async runtime
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("Failed to create tokio runtime")?
        .block_on(run(cli))
}

/// Install and start zlayer as a launchd service on macOS.
///
/// Generates a plist from the current CLI args (minus `--daemon`), writes it
/// to the appropriate LaunchDaemons/LaunchAgents directory, and loads it via
/// `launchctl`. The plist runs the server in foreground — launchd handles
/// lifecycle, restart-on-crash, and log routing.
#[cfg(target_os = "macos")]
fn install_launchd_service(cli: &Cli) -> Result<()> {
    use std::fs;
    use std::process::Command;

    let exe = std::env::current_exe().context("Failed to resolve current executable path")?;
    let exe_str = exe.to_string_lossy();

    // Extract serve args from the parsed CLI
    let (bind, jwt_secret, no_swagger, socket) = match &cli.command {
        Commands::Serve {
            bind,
            jwt_secret,
            no_swagger,
            socket,
            ..
        } => (
            bind.clone(),
            jwt_secret.clone(),
            *no_swagger,
            socket.clone(),
        ),
        _ => unreachable!("install_launchd_service called without Serve command"),
    };

    // Build the ProgramArguments array entries
    let mut args = vec![
        format!("        <string>{}</string>", exe_str),
        "        <string>serve</string>".to_string(),
        format!("        <string>--bind</string>"),
        format!("        <string>{}</string>", bind),
        format!("        <string>--socket</string>"),
        format!("        <string>{}</string>", socket),
    ];

    if let Some(ref secret) = jwt_secret {
        args.push("        <string>--jwt-secret</string>".to_string());
        args.push(format!("        <string>{}</string>", secret));
    }

    if no_swagger {
        args.push("        <string>--no-swagger</string>".to_string());
    }

    if cli.host_network {
        args.push("        <string>--host-network</string>".to_string());
    }

    // Forward verbosity
    for _ in 0..cli.verbose {
        args.push("        <string>-v</string>".to_string());
    }

    let args_xml = args.join("\n");

    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.zlayer.daemon</string>
    <key>ProgramArguments</key>
    <array>
{}
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>/var/log/zlayer/daemon.log</string>
    <key>StandardErrorPath</key>
    <string>/var/log/zlayer/daemon.log</string>
    <key>WorkingDirectory</key>
    <string>/</string>
</dict>
</plist>"#,
        args_xml
    );

    // Create log directory
    fs::create_dir_all("/var/log/zlayer").context("Failed to create /var/log/zlayer")?;

    // Determine plist location based on privilege level
    let is_root = unsafe { libc::geteuid() } == 0;
    let plist_dir = if is_root {
        "/Library/LaunchDaemons"
    } else {
        let home = std::env::var("HOME").context("HOME not set")?;
        let dir = format!("{}/Library/LaunchAgents", home);
        fs::create_dir_all(&dir).context("Failed to create ~/Library/LaunchAgents")?;
        // Leak the string so we get a &'static str — only runs once at startup
        Box::leak(dir.into_boxed_str())
    };

    let plist_path = format!("{}/com.zlayer.daemon.plist", plist_dir);

    // Unload any existing service first (ignore errors — may not be loaded)
    let _ = Command::new("launchctl")
        .args(["bootout", "system/com.zlayer.daemon"])
        .output();
    let _ = Command::new("launchctl")
        .args(["unload", &plist_path])
        .output();

    // Write and load the plist
    fs::write(&plist_path, &plist)
        .with_context(|| format!("Failed to write plist to {}", plist_path))?;

    let domain = if is_root { "system" } else { "gui" };
    let uid = unsafe { libc::getuid() };
    let target = if is_root {
        domain.to_string()
    } else {
        format!("{}/{}", domain, uid)
    };

    let status = Command::new("launchctl")
        .args(["bootstrap", &target, &plist_path])
        .status()
        .context("Failed to run launchctl bootstrap")?;

    if !status.success() {
        // Fall back to legacy `launchctl load` for older macOS
        let status = Command::new("launchctl")
            .args(["load", "-w", &plist_path])
            .status()
            .context("Failed to run launchctl load")?;

        if !status.success() {
            anyhow::bail!(
                "launchctl failed to load the service (exit code: {:?})",
                status.code()
            );
        }
    }

    Ok(())
}

async fn run(cli: Cli) -> Result<()> {
    match &cli.command {
        Commands::Deploy { spec_path, dry_run } => {
            let path = util::discover_spec_path(spec_path.as_deref())?;
            commands::deploy::deploy(&cli, &path, *dry_run).await
        }
        Commands::Join {
            token,
            spec_dir,
            service,
            replicas,
        } => {
            commands::join::join(
                &cli,
                token,
                spec_dir.as_deref(),
                service.as_deref(),
                *replicas,
            )
            .await
        }
        Commands::Serve {
            bind,
            jwt_secret,
            no_swagger,
            daemon: _, // Already handled in main() before tokio runtime
            socket,
        } => {
            commands::serve::serve(
                bind,
                jwt_secret.clone(),
                *no_swagger,
                socket,
                cli.host_network,
            )
            .await
        }
        Commands::Status => commands::lifecycle::status(&cli).await,
        Commands::Validate { spec_path } => {
            let path = util::discover_spec_path(spec_path.as_deref())?;
            commands::lifecycle::validate(&path).await
        }
        Commands::Logs {
            deployment,
            service,
            lines,
            follow,
            instance,
        } => {
            commands::lifecycle::logs(deployment, service, *lines, *follow, instance.clone()).await
        }
        Commands::Stop {
            deployment,
            service,
            force,
            timeout,
        } => commands::lifecycle::stop(deployment, service.clone(), *force, *timeout).await,
        Commands::Up { spec_path } => {
            let path = util::discover_spec_path(spec_path.as_deref())?;
            commands::deploy::up(&cli, &path).await
        }
        Commands::Down { deployment } => commands::deploy::down(deployment.clone()).await,
        Commands::Build {
            context,
            file,
            zimagefile,
            tags,
            runtime,
            runtime_auto,
            build_args,
            target,
            no_cache,
            push,
            no_tui,
            verbose_build,
        } => {
            commands::build::handle_build(
                context.clone(),
                file.clone(),
                zimagefile.clone(),
                tags.clone(),
                runtime.clone(),
                *runtime_auto,
                build_args.clone(),
                target.clone(),
                *no_cache,
                *push,
                *no_tui,
                *verbose_build,
            )
            .await
        }
        Commands::Runtimes => commands::build::handle_runtimes().await,
        Commands::Token(token_cmd) => commands::token::handle_token(token_cmd),
        Commands::Spec(spec_cmd) => commands::spec::handle_spec(spec_cmd).await,
        Commands::Export {
            image,
            output,
            gzip,
        } => commands::registry::handle_export(&cli, image, output, *gzip).await,
        Commands::Import { input, tag } => {
            commands::registry::handle_import(&cli, input, tag.clone()).await
        }
        Commands::Wasm(wasm_cmd) => commands::wasm::handle_wasm(&cli, wasm_cmd).await,
        Commands::Tunnel(tunnel_cmd) => commands::tunnel::handle_tunnel(&cli, tunnel_cmd).await,
        Commands::Manager(manager_cmd) => commands::manager::handle_manager(manager_cmd).await,
        Commands::Pull { image } => commands::registry::handle_pull(image).await,
        Commands::Exec {
            service,
            deployment,
            replica,
            cmd,
        } => commands::exec::exec(deployment.clone(), service, *replica, cmd).await,
        Commands::Ps {
            deployment,
            containers,
            format,
        } => commands::ps::ps(deployment.clone(), *containers, format).await,
        Commands::Node(node_cmd) => commands::node::handle_node(node_cmd).await,
    }
}
