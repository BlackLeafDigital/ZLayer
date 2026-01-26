//! Plain logger for CI/non-interactive environments
//!
//! This module provides a simple logging output mode that works in
//! non-interactive environments like CI pipelines, where a full TUI
//! would not be appropriate.

use super::BuildEvent;

/// Simple logging output for CI/non-interactive mode
///
/// This provides a line-by-line output of build progress suitable for
/// log files and CI systems that don't support interactive terminals.
///
/// # Example
///
/// ```
/// use builder::tui::{PlainLogger, BuildEvent};
///
/// let logger = PlainLogger::new(false); // quiet mode
///
/// logger.handle_event(&BuildEvent::StageStarted {
///     index: 0,
///     name: Some("builder".to_string()),
///     base_image: "node:20-alpine".to_string(),
/// });
/// // Output: ==> Stage: builder (node:20-alpine)
///
/// logger.handle_event(&BuildEvent::InstructionStarted {
///     stage: 0,
///     index: 0,
///     instruction: "RUN npm ci".to_string(),
/// });
/// // Output:   -> RUN npm ci
/// ```
#[derive(Debug, Clone)]
pub struct PlainLogger {
    /// Whether to show verbose output (including all stdout/stderr lines)
    verbose: bool,
    /// Whether to use colors in output
    color: bool,
}

impl Default for PlainLogger {
    fn default() -> Self {
        Self::new(false)
    }
}

impl PlainLogger {
    /// Create a new plain logger
    ///
    /// # Arguments
    ///
    /// * `verbose` - If true, shows all output lines. If false, only shows
    ///   stage and instruction transitions.
    pub fn new(verbose: bool) -> Self {
        Self {
            verbose,
            color: Self::detect_color_support(),
        }
    }

    /// Create a new plain logger with explicit color setting
    pub fn with_color(verbose: bool, color: bool) -> Self {
        Self { verbose, color }
    }

    /// Detect if the terminal supports color output
    fn detect_color_support() -> bool {
        // Check common environment variables
        if std::env::var("NO_COLOR").is_ok() {
            return false;
        }

        if std::env::var("FORCE_COLOR").is_ok() {
            return true;
        }

        // Check if we're in a known CI environment that supports color
        if std::env::var("CI").is_ok() {
            // Most CI systems support ANSI colors
            return true;
        }

        // Check TERM variable
        if let Ok(term) = std::env::var("TERM") {
            return term != "dumb";
        }

        // Default to no color if unsure
        false
    }

    /// Apply ANSI color codes if color is enabled
    fn colorize(&self, text: &str, color: &str) -> String {
        if self.color {
            format!("{}{}{}", color, text, "\x1b[0m")
        } else {
            text.to_string()
        }
    }

    /// ANSI color codes
    const GREEN: &'static str = "\x1b[32m";
    const YELLOW: &'static str = "\x1b[33m";
    const RED: &'static str = "\x1b[31m";
    const CYAN: &'static str = "\x1b[36m";
    const DIM: &'static str = "\x1b[2m";

    /// Handle a build event and print appropriate output
    pub fn handle_event(&self, event: &BuildEvent) {
        match event {
            BuildEvent::StageStarted {
                index,
                name,
                base_image,
            } => {
                let stage_name = name.as_deref().unwrap_or("unnamed");
                let header = format!("==> Stage {}: {} ({})", index + 1, stage_name, base_image);
                println!("{}", self.colorize(&header, Self::CYAN));
            }

            BuildEvent::InstructionStarted { instruction, .. } => {
                let line = format!("  -> {}", instruction);
                println!("{}", self.colorize(&line, Self::YELLOW));
            }

            BuildEvent::Output { line, is_stderr } if self.verbose => {
                if *is_stderr {
                    eprintln!("     {}", self.colorize(line, Self::DIM));
                } else {
                    println!("     {}", line);
                }
            }

            BuildEvent::Output { .. } => {
                // In non-verbose mode, we skip individual output lines
            }

            BuildEvent::InstructionComplete { cached, .. } => {
                if *cached && self.verbose {
                    println!("     {}", self.colorize("[cached]", Self::CYAN));
                }
            }

            BuildEvent::StageComplete { index } => {
                if self.verbose {
                    let line = format!("  Stage {} complete", index + 1);
                    println!("{}", self.colorize(&line, Self::GREEN));
                }
            }

            BuildEvent::BuildComplete { image_id } => {
                println!();
                let success = format!("Build complete: {}", image_id);
                println!("{}", self.colorize(&success, Self::GREEN));
            }

            BuildEvent::BuildFailed { error } => {
                println!();
                let failure = format!("Build failed: {}", error);
                eprintln!("{}", self.colorize(&failure, Self::RED));
            }
        }
    }

    /// Process a stream of events, printing each one
    ///
    /// This is useful for processing events from a channel in a loop.
    pub fn process_events<I>(&self, events: I)
    where
        I: IntoIterator<Item = BuildEvent>,
    {
        for event in events {
            self.handle_event(&event);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plain_logger_creation() {
        let logger = PlainLogger::new(false);
        assert!(!logger.verbose);

        let verbose_logger = PlainLogger::new(true);
        assert!(verbose_logger.verbose);
    }

    #[test]
    fn test_with_color() {
        let logger = PlainLogger::with_color(false, true);
        assert!(logger.color);

        let no_color_logger = PlainLogger::with_color(false, false);
        assert!(!no_color_logger.color);
    }

    #[test]
    fn test_colorize_enabled() {
        let logger = PlainLogger::with_color(false, true);
        let result = logger.colorize("test", PlainLogger::GREEN);
        assert!(result.contains("\x1b[32m"));
        assert!(result.contains("\x1b[0m"));
        assert!(result.contains("test"));
    }

    #[test]
    fn test_colorize_disabled() {
        let logger = PlainLogger::with_color(false, false);
        let result = logger.colorize("test", PlainLogger::GREEN);
        assert_eq!(result, "test");
        assert!(!result.contains("\x1b["));
    }

    #[test]
    fn test_handle_event_does_not_panic() {
        // This test just ensures that handling various events doesn't panic
        let logger = PlainLogger::with_color(true, false);

        // All event types should be handled without panic
        logger.handle_event(&BuildEvent::StageStarted {
            index: 0,
            name: Some("builder".to_string()),
            base_image: "alpine".to_string(),
        });

        logger.handle_event(&BuildEvent::InstructionStarted {
            stage: 0,
            index: 0,
            instruction: "RUN echo hello".to_string(),
        });

        logger.handle_event(&BuildEvent::Output {
            line: "hello".to_string(),
            is_stderr: false,
        });

        logger.handle_event(&BuildEvent::Output {
            line: "warning".to_string(),
            is_stderr: true,
        });

        logger.handle_event(&BuildEvent::InstructionComplete {
            stage: 0,
            index: 0,
            cached: true,
        });

        logger.handle_event(&BuildEvent::StageComplete { index: 0 });

        logger.handle_event(&BuildEvent::BuildComplete {
            image_id: "sha256:abc".to_string(),
        });

        logger.handle_event(&BuildEvent::BuildFailed {
            error: "test error".to_string(),
        });
    }

    #[test]
    fn test_default() {
        let logger = PlainLogger::default();
        assert!(!logger.verbose);
    }
}
