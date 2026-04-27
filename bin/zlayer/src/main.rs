//! `ZLayer` -- unified CLI for the `ZLayer` container orchestration platform.
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
mod bootstrap_admin;
mod bootstrap_admin_env_grants;
mod cli;
mod commands;
mod config;
pub mod daemon;
mod daemon_service;
#[allow(dead_code)]
mod deploy_tui;
pub mod resources;
pub mod ui;
mod util;
mod views;
mod widgets;

use anyhow::{Context, Result};
use clap::Parser;
use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::ExitCode;

use cli::{Cli, Commands};
use zlayer_observability::{
    init_observability, LogFormat, LogLevel, LoggingConfig, ObservabilityConfig,
};

#[allow(clippy::too_many_lines, unsafe_code)]
fn main() -> ExitCode {
    let cli = Cli::parse();

    // No subcommand or explicit `tui` -> launch the interactive TUI
    match &cli.command {
        None => return run_tui_entry(None),
        Some(Commands::Tui { context }) => return run_tui_entry(context.clone()),
        Some(Commands::Completions { shell }) => match commands::completions::run(*shell) {
            Ok(()) => return ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("Error: {e:#}");
                return ExitCode::FAILURE;
            }
        },
        _ => {}
    }

    // --- Daemon / CLI path below ---

    // `zlayer serve --service` hands the whole process to the Windows SCM
    // dispatcher (I-1). The dispatcher blocks the thread it's called on and
    // spawns its own thread for `ffi_service_main`, which builds a tokio
    // runtime internally — so we must NOT construct a runtime here, and we
    // also skip the normal observability init, daemonization, and CLI
    // dispatch paths.
    //
    // On non-Windows this returns an error ("not supported on this
    // platform") surfaced via the stub in `daemon_service::stub`.
    if let Some(Commands::Serve { service: true, .. }) = &cli.command {
        return match run_service_entry(&cli) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("Error: {e:#}");
                ExitCode::FAILURE
            }
        };
    }

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

        #[cfg(target_os = "windows")]
        {
            // No fork on Windows; the daemon runs in the foreground. Operators
            // who want a background service can use a Windows service wrapper,
            // scheduled task, or `Start-Process -WindowStyle Hidden`.
            //
            // The `wsl` feature only enables a Linux delegate runtime inside
            // the native HCS-backed daemon — it is NOT required to serve.
            eprintln!(
                "Note: zlayer serve runs in the foreground on Windows. \
                 To run in the background, wrap it in a Windows service or scheduled task."
            );
            // Fall through into the normal serve path below.
        }

        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            use std::fs;

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

            // Fork + setsid + chdir to /.
            if let Err(e) = nix::unistd::daemon(false, true).context("Failed to daemonize") {
                eprintln!("Error: {e:#}");
                return ExitCode::FAILURE;
            }

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
                 zlayer_init_actions=warn,zlayer_scheduler=warn,zlayer_api=warn,\
                 netlink_packet_route::link::buffer_tool=error,warn"
                    .to_string(),
            ),
        ),
        1 => (
            LogLevel::Info,
            Some("netlink_packet_route::link::buffer_tool=error,info".to_string()),
        ),
        2 => (
            LogLevel::Debug,
            Some("netlink_packet_route::link::buffer_tool=error,debug".to_string()),
        ),
        _ => (
            LogLevel::Trace,
            Some("netlink_packet_route::link::buffer_tool=error,trace".to_string()),
        ),
    };

    // Use pretty format for terminals, JSON for piped output
    let log_format = if std::io::stdout().is_terminal() {
        LogFormat::Pretty
    } else {
        LogFormat::Json
    };

    // When running as a daemon/serve, log to files via tracing-appender
    let file_logging = if matches!(&cli.command, Some(Commands::Serve { .. })) {
        let log_dir = cli.effective_log_dir();
        // Ensure log directory exists
        if let Err(e) = std::fs::create_dir_all(&log_dir) {
            eprintln!(
                "Warning: failed to create log dir {}: {e}",
                log_dir.display()
            );
        }
        Some(zlayer_observability::config::FileLoggingConfig {
            directory: log_dir,
            prefix: "daemon.log".to_string(),
            rotation: zlayer_observability::config::RotationStrategy::Daily,
            max_files: Some(7),
        })
    } else {
        None
    };

    let obs_config = ObservabilityConfig {
        logging: LoggingConfig {
            level: log_level,
            format: log_format,
            filter_directives,
            file: file_logging,
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
#[allow(clippy::too_many_lines, unsafe_code)]
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

    // Build the ProgramArguments array entries.
    // IMPORTANT: --data-dir is a top-level Cli arg and MUST come before the
    // subcommand, otherwise clap rejects it as an unknown serve flag.
    let effective_data_dir = cli.effective_data_dir();
    let mut args = vec![
        format!("        <string>{}</string>", exe_str),
        "        <string>--data-dir</string>".to_string(),
        format!("        <string>{}</string>", effective_data_dir.display()),
        "        <string>serve</string>".to_string(),
        "        <string>--bind</string>".to_string(),
        format!("        <string>{}</string>", bind),
        "        <string>--socket</string>".to_string(),
        format!(
            "        <string>{}</string>",
            socket.as_deref().unwrap_or(&resolved_socket)
        ),
    ];

    if let Some(ref secret) = jwt_secret {
        args.push("        <string>--jwt-secret</string>".to_string());
        args.push(format!("        <string>{secret}</string>"));
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
            r"    <key>EnvironmentVariables</key>
    <dict>
        <key>HOME</key>
        <string>{home}</string>
    </dict>"
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

    // Create log directory
    fs::create_dir_all(log_dir)
        .with_context(|| format!("Failed to create {}", log_dir.display()))?;

    // Determine plist location based on privilege level
    let is_root = unsafe { libc::geteuid() } == 0;
    let plist_dir = if is_root {
        "/Library/LaunchDaemons"
    } else {
        let home = std::env::var("HOME").context("HOME not set")?;
        let dir = format!("{home}/Library/LaunchAgents");
        fs::create_dir_all(&dir).context("Failed to create ~/Library/LaunchAgents")?;
        // Leak the string so we get a &'static str -- only runs once at startup
        Box::leak(dir.into_boxed_str())
    };

    let plist_path = format!("{plist_dir}/com.zlayer.daemon.plist");

    // Unload any existing service first (ignore errors)
    let _ = Command::new("launchctl")
        .args(["bootout", "system/com.zlayer.daemon"])
        .output();
    let _ = Command::new("launchctl")
        .args(["unload", &plist_path])
        .output();

    // Write and load the plist
    fs::write(&plist_path, &plist)
        .with_context(|| format!("Failed to write plist to {plist_path}"))?;

    let domain = if is_root { "system" } else { "gui" };
    let uid = unsafe { libc::getuid() };
    let target = if is_root {
        domain.to_string()
    } else {
        format!("{domain}/{uid}")
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

/// Entry point for `zlayer serve --service`.
///
/// Handed control before the tokio runtime is built because the Windows SCM
/// dispatcher blocks the calling thread and spawns its own thread for the
/// service main. The dispatched entry point builds its own runtime internally
/// (see `daemon_service::imp::run_service`).
///
/// On non-Windows targets this bails immediately — the `--service` flag is
/// only meaningful when the Windows SCM spawns the binary.
#[allow(unused_variables)]
fn run_service_entry(cli: &Cli) -> Result<()> {
    #[cfg(not(windows))]
    {
        anyhow::bail!("--service is not supported on this platform (Windows only)");
    }

    #[cfg(windows)]
    {
        let Some(Commands::Serve {
            bind,
            jwt_secret,
            no_swagger,
            ..
        }) = cli.command.as_ref()
        else {
            anyhow::bail!("run_service_entry called without Serve command");
        };

        let socket_path = cli.effective_socket_path();
        let data_dir = cli.effective_data_dir();

        // Observability init: the dispatched thread logs extensively but we
        // can't hold the guard across the sync dispatcher boundary the way
        // we do in `main()`'s tokio path. Fall back to file-based rolling
        // logs under the daemon log dir — the same directory the foreground
        // daemon uses via the FileLoggingConfig branch below.
        let log_dir = cli.effective_log_dir();
        if let Err(e) = std::fs::create_dir_all(&log_dir) {
            eprintln!(
                "Warning: failed to create log dir {}: {e}",
                log_dir.display()
            );
        }
        let obs_config = ObservabilityConfig {
            logging: LoggingConfig {
                level: LogLevel::Info,
                format: LogFormat::Json,
                filter_directives: None,
                file: Some(zlayer_observability::config::FileLoggingConfig {
                    directory: log_dir,
                    prefix: "daemon-service.log".to_string(),
                    rotation: zlayer_observability::config::RotationStrategy::Daily,
                    max_files: Some(7),
                }),
                ..Default::default()
            },
            ..Default::default()
        };
        // Guards drop at end of function, which is fine — service_dispatcher
        // blocks for the entire lifetime of the service, so the guards stay
        // alive until SCM stops us.
        let _guards = init_observability(&obs_config)
            .context("Failed to initialize observability for Windows Service")?;

        daemon_service::run_as_windows_service(
            bind.clone(),
            jwt_secret.clone(),
            *no_swagger,
            socket_path,
            cli.host_network,
            data_dir,
        )
    }
}

/// Dispatch CLI commands to their handlers.
#[allow(clippy::too_many_lines)]
async fn run(
    #[cfg_attr(not(feature = "docker-compat"), allow(unused_mut))] mut cli: Cli,
) -> Result<()> {
    // Docker compat: handle before borrowing since it takes ownership of args
    #[cfg(feature = "docker-compat")]
    if matches!(&cli.command, Some(Commands::Docker(_))) {
        let Some(Commands::Docker(docker_cmd)) = cli.command.take() else {
            unreachable!()
        };
        return zlayer_docker::handle_docker_command(*docker_cmd).await;
    }

    // command is guaranteed to be Some at this point (TUI handled earlier)
    let command = cli
        .command
        .as_ref()
        .expect("command should be Some in run()");
    match command {
        // =================================================================
        // Cross-platform commands (build, registry, inspection, etc.)
        // =================================================================
        Commands::Tui { .. } => unreachable!("TUI handled before async runtime"),
        Commands::Completions { .. } => {
            unreachable!("completions handled before async runtime")
        }
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
            pull,
            no_pull,
            push,
            no_tui,
            verbose_build,
            platform,
            update_bottles,
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
                pull.clone(),
                *no_pull,
                *push,
                *no_tui,
                *verbose_build,
                platform.clone(),
                *update_bottles,
            )
            .await
        }
        Commands::Runtimes => commands::build::handle_runtimes(),
        Commands::Validate { spec_path } => {
            let path = util::discover_spec_path(spec_path.as_deref())?;
            commands::lifecycle::validate(&path)
        }
        Commands::Spec(spec_cmd) => commands::spec::handle_spec(spec_cmd),
        Commands::Pipeline {
            file,
            set,
            push,
            fail_fast,
            no_tui,
            only,
            platform,
        } => {
            commands::pipeline::cmd_pipeline(
                file.clone(),
                set.clone(),
                *push,
                *fail_fast,
                *no_tui,
                only.clone(),
                platform.clone(),
            )
            .await
        }
        Commands::Wasm(wasm_cmd) => commands::wasm::handle_wasm(&cli, wasm_cmd).await,
        Commands::Tunnel(tunnel_cmd) => commands::tunnel::handle_tunnel(&cli, tunnel_cmd).await,
        Commands::Manager(manager_cmd) => commands::manager::handle_manager(manager_cmd).await,
        #[cfg(feature = "docker-compat")]
        Commands::Docker(_) => unreachable!("Docker handled before borrow"),
        Commands::Export {
            image,
            output,
            gzip,
        } => commands::registry::handle_export(&cli, image, output, *gzip).await,
        Commands::Import {
            input,
            tag,
            username,
            password,
        } => {
            commands::registry::handle_import(
                &cli,
                input,
                tag.clone(),
                username.as_deref(),
                password.as_deref(),
            )
            .await
        }
        Commands::Pull { image } => {
            commands::registry::handle_pull(image, &cli.effective_data_dir()).await
        }

        // =================================================================
        // Serve -- native HCS daemon on Windows, direct on Unix.
        //
        // Windows previously routed through a daemon running inside WSL2.
        // After Phase F-6, `create_runtime(RuntimeConfig::Hcs(..))` builds a
        // native CompositeRuntime with an optional WSL2 delegate, so the
        // Windows daemon path is now identical to Linux conceptually — it
        // just binds TCP loopback instead of a Unix socket.
        // =================================================================
        Commands::Serve {
            bind,
            jwt_secret,
            no_swagger,
            daemon: _, // Already handled in main() before tokio runtime
            #[cfg(all(target_os = "windows", feature = "wsl"))]
                vhd_gb: _,
            #[cfg(feature = "docker-compat")]
            docker_socket,
            #[cfg(feature = "docker-compat")]
            docker_socket_path,
            ..
        } => {
            // Spawn Docker API socket server if enabled. On Unix this is a
            // Unix domain socket; on Windows it is a named pipe. The transport
            // is selected inside `zlayer_docker::socket::serve`.
            #[cfg(feature = "docker-compat")]
            if *docker_socket {
                let path = docker_socket_path.clone();
                tokio::spawn(async move {
                    if let Err(e) = zlayer_docker::socket::serve(std::path::Path::new(&path)).await
                    {
                        tracing::error!("Docker API socket server failed: {e}");
                    }
                });
            }

            let socket_path = cli.effective_socket_path();
            let data_dir = cli.effective_data_dir();
            Box::pin(commands::serve::serve(
                bind,
                jwt_secret.clone(),
                *no_swagger,
                &socket_path,
                cli.host_network,
                data_dir,
            ))
            .await
        }

        // =================================================================
        // Runtime commands -- Unix only
        // =================================================================
        Commands::Deploy { spec_path, dry_run } => {
            let path = util::discover_spec_path(spec_path.as_deref())?;
            commands::deploy::deploy(&cli, &path, *dry_run).await
        }
        Commands::Join {
            token,
            spec_dir,
            service,
            replicas,
            install_wsl,
        } => {
            // The Windows branch of `commands::join::join` takes an extra
            // `ConsentMode` argument for the WSL2 auto-install consent flow
            // (H-3). Unix ignores the flag entirely, so the field is only
            // threaded through on non-Unix targets.
            #[cfg(not(unix))]
            let consent = install_wsl.mode();
            #[cfg(unix)]
            let _ = install_wsl; // silence unused-field warning on Unix.

            // Box::pin to keep this large match arm off the stack — the
            // future here is sizeable (spawns daemon state) and clippy's
            // `large_futures` lint flags anything above ~16kB.
            Box::pin(commands::join::join(
                &cli,
                token,
                spec_dir.as_deref(),
                service.as_deref(),
                *replicas,
                #[cfg(not(unix))]
                consent,
            ))
            .await
        }
        Commands::Status => commands::lifecycle::status(&cli).await,
        Commands::Logs {
            deployment,
            service,
            lines,
            follow,
            instance,
        } => {
            commands::lifecycle::logs(
                deployment.as_deref(),
                service,
                *lines,
                *follow,
                instance.clone(),
            )
            .await
        }
        Commands::Stop {
            deployment,
            service,
            force,
            timeout,
        } => {
            commands::lifecycle::stop(
                deployment.as_deref(),
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
        Commands::Token(token_cmd) => commands::token::handle_token(token_cmd),
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
        Commands::Daemon(action) => {
            commands::daemon::handle_daemon(action, &cli.effective_data_dir()).await
        }
        #[cfg(all(target_os = "windows", feature = "wsl"))]
        Commands::Windows(cmd) => commands::windows::handle(cmd).await,
        Commands::Image(image_cmd) => commands::image::handle_image(&cli, image_cmd).await,
        Commands::Container(container_cmd) => {
            commands::container::handle_container(&cli, container_cmd).await
        }
        Commands::System(system_cmd) => commands::system::handle_system(&cli, system_cmd).await,
        Commands::Secret(secret_cmd) => commands::secret::handle_secret(&cli, secret_cmd).await,
        Commands::Run {
            env,
            no_global,
            merge,
            project,
            dry_run,
            unmask,
            command,
        } => {
            commands::run::handle_run(
                env,
                *no_global,
                merge,
                project.as_deref(),
                *dry_run,
                *unmask,
                command,
            )
            .await
        }
        Commands::Network(network_cmd) => {
            commands::network::handle_network(&cli, network_cmd).await
        }
        Commands::Job(job_cmd) => commands::job::handle_job(&cli, job_cmd).await,
        Commands::Volume(volume_cmd) => commands::volume::handle_volume(&cli, volume_cmd).await,
        Commands::Auth(auth_cmd) => match auth_cmd {
            cli::AuthCommands::Bootstrap {
                email,
                password,
                display_name,
            } => {
                commands::auth::bootstrap(email.clone(), password.clone(), display_name.clone())
                    .await
            }
            cli::AuthCommands::Login { email, password } => {
                commands::auth::login(email.clone(), password.clone()).await
            }
            cli::AuthCommands::Logout => commands::auth::logout().await,
            cli::AuthCommands::Whoami => commands::auth::whoami().await,
        },
        Commands::Env(env_cmd) => match env_cmd {
            cli::EnvCommands::Ls { project, output } => {
                commands::env::list(project.clone(), output).await
            }
            cli::EnvCommands::Create {
                name,
                project,
                description,
            } => commands::env::create(name.clone(), project.clone(), description.clone()).await,
            cli::EnvCommands::Show { id, output } => commands::env::get(id.clone(), output).await,
            cli::EnvCommands::Update {
                id,
                name,
                description,
            } => commands::env::update(id.clone(), name.clone(), description.clone()).await,
            cli::EnvCommands::Delete { id, yes } => commands::env::delete(id.clone(), *yes).await,
        },
        Commands::Task(task_cmd) => match task_cmd {
            cli::TaskCommands::List { project, output } => {
                commands::task::list(project.clone(), output).await
            }
            cli::TaskCommands::Create {
                name,
                kind,
                body,
                project,
            } => {
                commands::task::create(name.clone(), kind.clone(), body.clone(), project.clone())
                    .await
            }
            cli::TaskCommands::Run { id } => commands::task::run(id.clone()).await,
            cli::TaskCommands::Logs { id } => commands::task::logs(id.clone()).await,
            cli::TaskCommands::Delete { id, yes } => commands::task::delete(id.clone(), *yes).await,
        },
        Commands::Workflow(wf_cmd) => match wf_cmd {
            cli::WorkflowCommands::List { output } => commands::workflow::list(output).await,
            cli::WorkflowCommands::Create {
                name,
                steps,
                project,
            } => commands::workflow::create(name.clone(), steps.clone(), project.clone()).await,
            cli::WorkflowCommands::Run { id } => commands::workflow::run(id.clone()).await,
            cli::WorkflowCommands::Logs { id } => commands::workflow::logs(id.clone()).await,
            cli::WorkflowCommands::Delete { id, yes } => {
                commands::workflow::delete(id.clone(), *yes).await
            }
        },
        Commands::Notifier(n_cmd) => match n_cmd {
            cli::NotifierCommands::List { output } => commands::notifier::list(output).await,
            cli::NotifierCommands::Create {
                name,
                kind,
                webhook_url,
                url,
            } => {
                commands::notifier::create(
                    name.clone(),
                    kind.clone(),
                    webhook_url.clone(),
                    url.clone(),
                )
                .await
            }
            cli::NotifierCommands::Test { id } => commands::notifier::test(id.clone()).await,
            cli::NotifierCommands::Delete { id, yes } => {
                commands::notifier::delete(id.clone(), *yes).await
            }
        },
        Commands::Variable(var_cmd) => match var_cmd {
            cli::VariableCommands::List { scope, output } => {
                commands::variable::list(scope.clone(), output).await
            }
            cli::VariableCommands::Set { name, value, scope } => {
                commands::variable::set(name.clone(), value.clone(), scope.clone()).await
            }
            cli::VariableCommands::Get { name, scope } => {
                commands::variable::get(name.clone(), scope.clone()).await
            }
            cli::VariableCommands::Unset { name, scope } => {
                commands::variable::unset(name.clone(), scope.clone()).await
            }
        },
        Commands::Project(project_cmd) => match project_cmd {
            cli::ProjectCommands::Ls { output } => commands::project::list(output).await,
            cli::ProjectCommands::Create {
                name,
                git_url,
                git_branch,
                build_kind,
                build_path,
                description,
                registry_credential,
                git_credential,
                default_env,
            } => {
                commands::project::create(
                    name.clone(),
                    git_url.clone(),
                    git_branch.clone(),
                    build_kind.map(|k| k.to_string()),
                    build_path.clone(),
                    description.clone(),
                    registry_credential.clone(),
                    git_credential.clone(),
                    default_env.clone(),
                )
                .await
            }
            cli::ProjectCommands::Show { id, output } => {
                commands::project::show(id.clone(), output).await
            }
            cli::ProjectCommands::Update {
                id,
                name,
                description,
                git_url,
                git_branch,
                build_kind,
                build_path,
                registry_credential,
                git_credential,
                default_env,
            } => {
                commands::project::update(
                    id.clone(),
                    name.clone(),
                    description.clone(),
                    git_url.clone(),
                    git_branch.clone(),
                    build_kind.map(|k| k.to_string()),
                    build_path.clone(),
                    registry_credential.clone(),
                    git_credential.clone(),
                    default_env.clone(),
                )
                .await
            }
            cli::ProjectCommands::Delete { id, yes } => {
                commands::project::delete(id.clone(), *yes).await
            }
            cli::ProjectCommands::LinkDeployment { id, deployment } => {
                commands::project::link_deployment(id.clone(), deployment.clone()).await
            }
            cli::ProjectCommands::UnlinkDeployment { id, deployment } => {
                commands::project::unlink_deployment(id.clone(), deployment.clone()).await
            }
            cli::ProjectCommands::ListDeployments { id } => {
                commands::project::list_deployments(id.clone()).await
            }
            cli::ProjectCommands::Pull { id } => commands::project::pull(id.clone()).await,
            cli::ProjectCommands::AutoDeploy { id, enabled } => {
                commands::project::auto_deploy(id.clone(), *enabled).await
            }
            cli::ProjectCommands::PollInterval { id, seconds } => {
                commands::project::poll_interval(id.clone(), *seconds).await
            }
            cli::ProjectCommands::Webhook(wh_cmd) => match wh_cmd {
                cli::WebhookCommands::Show { id } => {
                    commands::project::webhook_show(id.clone()).await
                }
                cli::WebhookCommands::Rotate { id } => {
                    commands::project::webhook_rotate(id.clone()).await
                }
            },
        },
        Commands::Credential(cred_cmd) => match cred_cmd {
            cli::CredentialCommands::Registry(reg_cmd) => match reg_cmd {
                cli::RegistryCredentialCommands::Ls { output } => {
                    commands::credential::registry_list(output).await
                }
                cli::RegistryCredentialCommands::Add {
                    registry,
                    username,
                    password,
                    auth_type,
                } => {
                    commands::credential::registry_add(
                        registry.clone(),
                        username.clone(),
                        password.clone(),
                        auth_type.to_string(),
                    )
                    .await
                }
                cli::RegistryCredentialCommands::Delete { id, yes } => {
                    commands::credential::registry_delete(id.clone(), *yes).await
                }
            },
            cli::CredentialCommands::Git(git_cmd) => match git_cmd {
                cli::GitCredentialCommands::Ls { output } => {
                    commands::credential::git_list(output).await
                }
                cli::GitCredentialCommands::Add { name, value, kind } => {
                    commands::credential::git_add(name.clone(), value.clone(), kind.to_string())
                        .await
                }
                cli::GitCredentialCommands::Delete { id, yes } => {
                    commands::credential::git_delete(id.clone(), *yes).await
                }
            },
        },
        Commands::Sync(sync_cmd) => match sync_cmd {
            cli::SyncCommands::Ls { output } => commands::sync_cmd::list(output).await,
            cli::SyncCommands::Create {
                name,
                project,
                path,
                auto_apply,
            } => {
                commands::sync_cmd::create(name.clone(), project.clone(), path.clone(), *auto_apply)
                    .await
            }
            cli::SyncCommands::Diff { id } => commands::sync_cmd::diff(id.clone()).await,
            cli::SyncCommands::Apply { id } => commands::sync_cmd::apply(id.clone()).await,
            cli::SyncCommands::Delete { id, yes } => {
                commands::sync_cmd::delete(id.clone(), *yes).await
            }
        },
        Commands::User(user_cmd) => match user_cmd {
            cli::UserCommands::Ls { output } => commands::user::list(output).await,
            cli::UserCommands::Create {
                email,
                password,
                role,
                display_name,
            } => {
                commands::user::create(
                    email.clone(),
                    password.clone(),
                    (*role).into(),
                    display_name.clone(),
                )
                .await
            }
            cli::UserCommands::SetRole { id, role } => {
                commands::user::set_role(id.clone(), (*role).into()).await
            }
            cli::UserCommands::SetPassword {
                id,
                email,
                password,
                password_file,
                random,
                no_confirm,
            } => {
                commands::user::set_password(
                    id.clone(),
                    email.clone(),
                    password.clone(),
                    password_file.clone(),
                    *random,
                    *no_confirm,
                )
                .await
            }
            cli::UserCommands::Delete { id, yes } => commands::user::delete(id.clone(), *yes).await,
        },
        Commands::Group(group_cmd) => match group_cmd {
            cli::GroupCommands::List { output } => commands::group::list(output).await,
            cli::GroupCommands::Create { name } => commands::group::create(name.clone()).await,
            cli::GroupCommands::Delete { id, yes } => {
                commands::group::delete(id.clone(), *yes).await
            }
            cli::GroupCommands::Member(member_cmd) => match member_cmd {
                cli::GroupMemberCommands::Add { group, user } => {
                    commands::group::member_add(group.clone(), user.clone()).await
                }
                cli::GroupMemberCommands::Remove { group, user } => {
                    commands::group::member_remove(group.clone(), user.clone()).await
                }
            },
        },
        Commands::Permission(perm_cmd) => match perm_cmd {
            cli::PermissionCommands::List {
                user,
                group,
                output,
            } => commands::permission::list(user.clone(), group.clone(), output).await,
            cli::PermissionCommands::Grant {
                subject_kind,
                subject,
                resource_kind,
                resource,
                level,
            } => {
                commands::permission::grant(
                    (*subject_kind).into(),
                    subject.clone(),
                    resource_kind.clone(),
                    resource.clone(),
                    (*level).into(),
                )
                .await
            }
            cli::PermissionCommands::Revoke { id } => {
                commands::permission::revoke(id.clone()).await
            }
        },
        Commands::Audit(audit_cmd) => match audit_cmd {
            cli::AuditCommands::Tail {
                user,
                resource,
                limit,
                output,
            } => {
                commands::audit_cmd::tail(user.clone(), resource.clone(), Some(*limit), output)
                    .await
            }
        },
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
