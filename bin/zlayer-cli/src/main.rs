//! zlayer-cli -- Interactive TUI for ZLayer container image building
//!
//! A beautiful, cross-platform Ratatui-based interface that wraps ZLayer's
//! builder functionality with an interactive, menu-driven terminal UI.

use std::io;
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
mod views;
mod widgets;

/// Interactive TUI for ZLayer container image building
#[derive(Parser)]
#[command(name = "zlayer-cli", version, about, long_about = None)]
struct Cli {
    /// Build context directory (optional; can also be selected interactively)
    #[arg(short, long)]
    context: Option<std::path::PathBuf>,
}

fn main() -> ExitCode {
    // Parse CLI args (for --help / --version / optional context)
    let cli = Cli::parse();

    // Initialize tracing to a file so it doesn't interfere with TUI
    let _guard = init_file_logging();

    // Install a panic hook that restores the terminal before printing the panic
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = restore_terminal();
        original_hook(panic_info);
    }));

    match run_tui(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            // Terminal is already restored by the drop guard / panic hook
            eprintln!("Error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

/// Set up and run the TUI application
fn run_tui(cli: Cli) -> anyhow::Result<()> {
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create and run the app
    let mut app = app::App::new(cli.context);
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
        let file_appender = tracing_appender::rolling::never("/tmp", "zlayer-cli.log");
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
