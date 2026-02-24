//! ZLayer -- unified CLI for the ZLayer container orchestration platform.
//!
//! Without a subcommand (or with `tui`) the interactive Ratatui-based TUI is
//! launched.  All other subcommands (deploy, serve, build, etc.) are handled
//! in-process.
//!
//! # Feature Flags
//!
//! - `full` (default): Enable all runtime capabilities
//! - `docker`: Docker runtime support
//! - `wasm`: WebAssembly runtime support
//! - `s3`: S3 storage backend
//! - `persistent`: Persistent scheduler/registry storage
//! - `observability`: Axum metrics and trace propagation

mod app;
mod cli;
mod commands;
mod config;
pub mod daemon;
pub mod daemon_client;
#[allow(dead_code)]
mod deploy_tui;
mod util;
mod views;
mod widgets;

use anyhow::{Context, Result};
use clap::Parser;
use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::ExitCode;

#[cfg(not(target_os = "macos"))]
use std::os::unix::io::AsRawFd;

use cli::{Cli, Commands};
use zlayer_observability::{
    init_observability, LogFormat, LogLevel, LoggingConfig, ObservabilityConfig,
};

fn main() -> ExitCode {
    let cli = Cli::parse();

    // No subcommand or explicit `tui` -> launch the interactive TUI
    match &cli.command {
        None => return run_tui_entry(None),
        Some(Commands::Tui { context }) => return run_tui_entry(context.clone()),
        _ => {}
    }

    // --- Daemon / CLI path below ---

    // Daemonize BEFORE any threads exist (before tokio runtime or observability init).
    // This is critical: daemon() calls fork(), which is unsafe after threads are spawned.
    let should_daemon = matches!(&cli.command, Some(Commands::Serve { daemon: true, .. }));

    if should_daemon {
        #[cfg(target_os = "macos")]
        {
            let log_dir = cli.effective_log_dir();
            match install_launchd_service(&cli, &log_dir) {
                Ok(()) => {
                    let log_path = log_dir.join("daemon.log");
                    let uid = unsafe { libc::getuid() };
                    let domain = if uid == 0 {
                        "system".to_string()
                    } else {
                        format!("gui/{uid}")
                    };
                    println!("zlayer daemon registered with launchd and started.");
                    println!("  Logs: {}", log_path.display());
                    println!("  Stop: launchctl bootout {domain}/com.zlayer.daemon");
                    return ExitCode::SUCCESS;
                }
                Err(e) => {
                    eprintln!("Error: {e:#}");
                    return ExitCode::FAILURE;
                }
            }
        }

        #[cfg(not(target_os = "macos"))]
        {
            use std::fs::{self, OpenOptions};

            let run_dir = cli.effective_run_dir();
            let log_dir = cli.effective_log_dir();

            // Create directories (idempotent via create_dir_all)
            if let Err(e) = fs::create_dir_all(&run_dir)
                .with_context(|| format!("Failed to create {}", run_dir.display()))
            {
                eprintln!("Error: {e:#}");
                return ExitCode::FAILURE;
            }
            if let Err(e) = fs::create_dir_all(&log_dir)
                .with_context(|| format!("Failed to create {}", log_dir.display()))
            {
                eprintln!("Error: {e:#}");
                return ExitCode::FAILURE;
            }

            // Open log file BEFORE forking so errors are visible on the caller's terminal
            let log_path = log_dir.join("daemon.log");
            let log_file = match OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_path)
                .with_context(|| format!("Failed to open {}", log_path.display()))
            {
                Ok(f) => f,
                Err(e) => {
                    eprintln!("Error: {e:#}");
                    return ExitCode::FAILURE;
                }
            };

            let log_fd = log_file.as_raw_fd();

            // Fork + setsid + chdir to /.
            if let Err(e) = nix::unistd::daemon(false, true).context("Failed to daemonize") {
                eprintln!("Error: {e:#}");
                return ExitCode::FAILURE;
            }

            // We are now the daemon child process.
            // Redirect stdout/stderr to the log file, close stdin.
            // SAFETY: No threads exist yet. We own these fds and the log_fd is valid.
            unsafe {
                libc::dup2(log_fd, libc::STDOUT_FILENO);
                libc::dup2(log_fd, libc::STDERR_FILENO);
                libc::close(libc::STDIN_FILENO);
            }

            drop(log_file);

            // Write PID file
            let pid_path = run_dir.join("zlayer.pid");
            if let Err(e) = fs::write(&pid_path, std::process::id().to_string())
                .with_context(|| format!("Failed to write PID file at {}", pid_path.display()))
            {
                eprintln!("Error: {e:#}");
                return ExitCode::FAILURE;
            }
        }
    }

    // Configure observability based on verbosity and environment
    let (log_level, filter_directives) = match cli.verbose {
        0 => (
            LogLevel::Warn,
            Some(
                "zlayer=warn,zlayer_agent=warn,zlayer_overlay=warn,zlayer_proxy=warn,\
                 zlayer_init_actions=warn,zlayer_scheduler=warn,zlayer_api=warn,warn"
                    .to_string(),
            ),
        ),
        1 => (LogLevel::Info, None),  // -v: global info
        2 => (LogLevel::Debug, None), // -vv: debug
        _ => (LogLevel::Trace, None), // -vvv: trace
    };

    // Use pretty format for terminals, JSON for piped output
    let log_format = if std::io::stdout().is_terminal() {
        LogFormat::Pretty
    } else {
        LogFormat::Json
    };

    let obs_config = ObservabilityConfig {
        logging: LoggingConfig {
            level: log_level,
            format: log_format,
            filter_directives,
            ..Default::default()
        },
        ..Default::default()
    };

    // Initialize observability - hold guards for application lifetime
    let _guards =
        match init_observability(&obs_config).context("Failed to initialize observability") {
            Ok(g) => g,
            Err(e) => {
                eprintln!("Error: {e:#}");
                return ExitCode::FAILURE;
            }
        };

    // Run the async runtime
    match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("Failed to create tokio runtime")
    {
        Ok(rt) => match rt.block_on(run(cli)) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("Error: {e:#}");
                ExitCode::FAILURE
            }
        },
        Err(e) => {
            eprintln!("Error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

/// Install and start zlayer as a launchd service on macOS.
#[cfg(target_os = "macos")]
fn install_launchd_service(cli: &Cli, log_dir: &std::path::Path) -> Result<()> {
    use std::fs;
    use std::process::Command;

    let exe = std::env::current_exe().context("Failed to resolve current executable path")?;
    let exe_str = exe.to_string_lossy();

    // Extract serve args from the parsed CLI
    let (bind, jwt_secret, no_swagger, socket) = match &cli.command {
        Some(Commands::Serve {
            bind,
            jwt_secret,
            no_swagger,
            socket,
            ..
        }) => (
            bind.clone(),
            jwt_secret.clone(),
            *no_swagger,
            socket.clone(),
        ),
        _ => unreachable!("install_launchd_service called without Serve command"),
    };

    let resolved_socket = cli.effective_socket_path();

    // Build the ProgramArguments array entries
    let mut args = vec![
        format!("        <string>{}</string>", exe_str),
        "        <string>serve</string>".to_string(),
        "        <string>--bind</string>".to_string(),
        format!("        <string>{}</string>", bind),
        "        <string>--socket</string>".to_string(),
        format!(
            "        <string>{}</string>",
            socket.as_deref().unwrap_or(&resolved_socket)
        ),
    ];

    // Always forward --data-dir with the effective value
    let effective_data_dir = cli.effective_data_dir();
    args.push("        <string>--data-dir</string>".to_string());
    args.push(format!(
        "        <string>{}</string>",
        effective_data_dir.display()
    ));

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

    let log_path = log_dir.join("daemon.log");
    let log_path_str = log_path.to_string_lossy();

    // Build EnvironmentVariables section
    let env_xml = if let Ok(home) = std::env::var("HOME") {
        format!(
            r#"    <key>EnvironmentVariables</key>
    <dict>
        <key>HOME</key>
        <string>{}</string>
    </dict>"#,
            home
        )
    } else {
        String::new()
    };

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
{}
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>{}</string>
    <key>StandardErrorPath</key>
    <string>{}</string>
    <key>WorkingDirectory</key>
    <string>/</string>
</dict>
</plist>"#,
        args_xml, env_xml, log_path_str, log_path_str
    );

    // Create log directory
    fs::create_dir_all(log_dir)
        .with_context(|| format!("Failed to create {}", log_dir.display()))?;

    // Determine plist location based on privilege level
    let is_root = unsafe { libc::geteuid() } == 0;
    let plist_dir = if is_root {
        "/Library/LaunchDaemons"
    } else {
        let home = std::env::var("HOME").context("HOME not set")?;
        let dir = format!("{}/Library/LaunchAgents", home);
        fs::create_dir_all(&dir).context("Failed to create ~/Library/LaunchAgents")?;
        // Leak the string so we get a &'static str -- only runs once at startup
        Box::leak(dir.into_boxed_str())
    };

    let plist_path = format!("{}/com.zlayer.daemon.plist", plist_dir);

    // Unload any existing service first (ignore errors)
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
        // Fall back to legacy `launchctl load`
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

/// Dispatch CLI commands to their handlers.
async fn run(cli: Cli) -> Result<()> {
    // command is guaranteed to be Some at this point (TUI handled earlier)
    let command = cli
        .command
        .as_ref()
        .expect("command should be Some in run()");
    match command {
        Commands::Tui { .. } => unreachable!("TUI handled before async runtime"),
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
            ..
        } => {
            let socket_path = cli.effective_socket_path();
            let data_dir = cli.effective_data_dir();
            commands::serve::serve(
                bind,
                jwt_secret.clone(),
                *no_swagger,
                &socket_path,
                cli.host_network,
                data_dir,
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
        } => {
            commands::lifecycle::stop(
                deployment,
                service.clone(),
                *force,
                *timeout,
                &cli.effective_state_dir(),
            )
            .await
        }
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
        Commands::Pull { image } => {
            commands::registry::handle_pull(image, &cli.effective_data_dir()).await
        }
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
        Commands::Node(node_cmd) => {
            commands::node::handle_node(node_cmd, &cli.effective_data_dir()).await
        }
        Commands::Pipeline {
            file,
            set,
            push,
            fail_fast,
            no_tui,
            only,
        } => {
            commands::pipeline::cmd_pipeline(
                file.clone(),
                set.clone(),
                *push,
                *fail_fast,
                *no_tui,
                only.clone(),
            )
            .await
        }
    }
}

// ---------------------------------------------------------------------------
// TUI entry point
// ---------------------------------------------------------------------------

/// Wrapper that sets up logging, panic hook, then runs the TUI.
fn run_tui_entry(context: Option<PathBuf>) -> ExitCode {
    // Initialize tracing to a file so it doesn't interfere with TUI
    let _guard = init_file_logging();

    // Install a panic hook that restores the terminal before printing the panic
    zlayer_tui::terminal::install_panic_hook();

    match run_tui(context) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("Error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

/// Set up and run the TUI application
fn run_tui(context: Option<PathBuf>) -> anyhow::Result<()> {
    // Setup terminal using shared utilities
    let mut terminal = zlayer_tui::terminal::setup_terminal()?;

    // Enable mouse capture for the interactive TUI
    crossterm::execute!(terminal.backend_mut(), crossterm::event::EnableMouseCapture)?;

    // Create and run the app
    let mut app = app::App::new(context);
    let result = app.run(&mut terminal);

    // Disable mouse capture before restoring
    let _ = crossterm::execute!(
        terminal.backend_mut(),
        crossterm::event::DisableMouseCapture
    );

    // Always restore terminal, even if the app returned an error
    zlayer_tui::terminal::restore_terminal(&mut terminal)?;

    result
}

/// Initialize tracing that writes to a log file instead of stderr
fn init_file_logging() -> Option<tracing_appender::non_blocking::WorkerGuard> {
    // Only enable file logging if RUST_LOG is set
    if std::env::var("RUST_LOG").is_ok() {
        let file_appender = tracing_appender::rolling::never("/tmp", "zlayer.log");
        let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
            )
            .with_writer(non_blocking)
            .with_ansi(false)
            .init();
        Some(guard)
    } else {
        None
    }
}
