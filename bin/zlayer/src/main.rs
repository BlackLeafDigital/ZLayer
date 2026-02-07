//! zlayer -- ZLayer container orchestration platform CLI
//!
//! Entry point for the `zlayer` command.  Without a subcommand (or with `tui`)
//! it launches the interactive Ratatui-based TUI.  The `build` subcommand is
//! handled in-process via `zlayer-builder`.  All other subcommands are delegated
//! to `zlayer-runtime`.

use std::io;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::prelude::*;
use ratatui::Terminal;

mod app;
mod cli;
mod views;
mod widgets;

use cli::{Cli, Commands};

fn main() -> ExitCode {
    let cli = Cli::parse();

    match cli.command {
        // No subcommand -> TUI
        None => run_tui_entry(cli.context),

        // Explicit TUI subcommand
        Some(Commands::Tui { context }) => run_tui_entry(context),

        // Build -> passthrough to runtime (it has the full build command)
        Some(Commands::Build { args }) => cli::exec_runtime("build", &args),

        // --- Runtime passthrough commands --------------------------------
        Some(Commands::Deploy { args }) => cli::exec_runtime("deploy", &args),
        Some(Commands::Join { args }) => cli::exec_runtime("join", &args),
        Some(Commands::Up { args }) => cli::exec_runtime("up", &args),
        Some(Commands::Down { args }) => cli::exec_runtime("down", &args),
        Some(Commands::Logs { args }) => cli::exec_runtime("logs", &args),
        Some(Commands::Status { args }) => cli::exec_runtime("status", &args),
        Some(Commands::Stop { args }) => cli::exec_runtime("stop", &args),
        Some(Commands::Validate { args }) => cli::exec_runtime("validate", &args),
        Some(Commands::Serve { args }) => cli::exec_runtime("serve", &args),
        Some(Commands::Pull { args }) => cli::exec_runtime("pull", &args),
        Some(Commands::Export { args }) => cli::exec_runtime("export", &args),
        Some(Commands::Import { args }) => cli::exec_runtime("import", &args),
        Some(Commands::Token { args }) => cli::exec_runtime("token", &args),
        Some(Commands::Spec { args }) => cli::exec_runtime("spec", &args),
        Some(Commands::Wasm { args }) => cli::exec_runtime("wasm", &args),
        Some(Commands::Tunnel { args }) => cli::exec_runtime("tunnel", &args),
        Some(Commands::Manager { args }) => cli::exec_runtime("manager", &args),
        Some(Commands::Node { args }) => cli::exec_runtime("node", &args),
        Some(Commands::Runtimes { args }) => cli::exec_runtime("runtimes", &args),
    }
}

/// Wrapper that sets up logging, panic hook, then runs the TUI.
fn run_tui_entry(context: Option<PathBuf>) -> ExitCode {
    // Initialize tracing to a file so it doesn't interfere with TUI
    let _guard = init_file_logging();

    // Install a panic hook that restores the terminal before printing the panic
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = restore_terminal();
        original_hook(panic_info);
    }));

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
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create and run the app
    let mut app = app::App::new(context);
    let result = app.run(&mut terminal);

    // Always restore terminal, even if the app returned an error
    restore_terminal()?;

    result
}

/// Restore the terminal to its original state
fn restore_terminal() -> anyhow::Result<()> {
    disable_raw_mode()?;
    execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture)?;
    Ok(())
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
