//! Terminal setup, teardown, and panic hook utilities.
//!
//! Unifies the identical terminal initialization and restoration code
//! used across all ZLayer TUI applications (builder, deploy, main CLI).

use std::io::{self, Stdout};
use std::time::Duration;

use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::prelude::*;
use ratatui::Terminal;

/// Standard event poll duration (~20 FPS).
pub const POLL_DURATION: Duration = Duration::from_millis(50);

/// Setup terminal for TUI (raw mode + alternate screen).
///
/// Enables raw mode, switches to the alternate screen buffer, and returns
/// a ready-to-use [`Terminal`] backed by crossterm.
pub fn setup_terminal() -> io::Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    Terminal::new(backend)
}

/// Restore terminal to normal mode.
///
/// Disables raw mode, leaves the alternate screen, and re-enables the cursor.
/// Call this before exiting the TUI or when an error requires early termination.
pub fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> io::Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

/// Install a panic hook that restores the terminal before panicking.
///
/// This ensures the terminal is returned to a usable state even if the
/// application panics. The previous panic hook is preserved and called
/// after terminal restoration.
pub fn install_panic_hook() {
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        // Best-effort terminal restoration -- ignore errors since we are
        // already in a panic and cannot do much about a failed cleanup.
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        original_hook(panic_info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poll_duration_is_50ms() {
        assert_eq!(POLL_DURATION, Duration::from_millis(50));
    }
}
