//! Terminal UI for build progress visualization
//!
//! This module provides a Ratatui-based TUI for displaying build progress,
//! as well as a plain logger for CI/non-interactive environments.
//!
//! # Architecture
//!
//! The TUI is event-driven and communicates with the build process via channels:
//!
//! ```text
//! ┌─────────────────┐     mpsc::Sender<BuildEvent>     ┌─────────────────┐
//! │  Build Process  │ ──────────────────────────────▶ │    BuildTui     │
//! └─────────────────┘                                  └─────────────────┘
//! ```
//!
//! # Example
//!
//! ```no_run
//! use zlayer_builder::tui::{BuildTui, BuildEvent};
//! use std::sync::mpsc;
//!
//! # fn main() -> std::io::Result<()> {
//! // Create channel for build events
//! let (tx, rx) = mpsc::channel();
//!
//! // Spawn build process that sends events
//! std::thread::spawn(move || {
//!     tx.send(BuildEvent::StageStarted {
//!         index: 0,
//!         name: Some("builder".to_string()),
//!         base_image: "node:20-alpine".to_string(),
//!     }).unwrap();
//!     // ... more events ...
//! });
//!
//! // Run TUI (blocks until build completes or user quits)
//! let mut tui = BuildTui::new(rx);
//! tui.run()?;
//! # Ok(())
//! # }
//! ```

mod app;
mod build_view;
mod logger;
mod widgets;

pub use app::BuildTui;
pub use build_view::BuildView;
pub use logger::PlainLogger;

/// Build event for TUI updates
///
/// These events are sent from the build process to update the TUI state.
/// The TUI processes these events asynchronously and updates the display.
#[derive(Debug, Clone)]
pub enum BuildEvent {
    /// Starting a new stage
    StageStarted {
        /// Stage index (0-based)
        index: usize,
        /// Optional stage name (from `AS name`)
        name: Option<String>,
        /// Base image for this stage
        base_image: String,
    },

    /// Starting an instruction within a stage
    InstructionStarted {
        /// Stage index
        stage: usize,
        /// Instruction index within the stage
        index: usize,
        /// Instruction text (e.g., "RUN npm ci")
        instruction: String,
    },

    /// Instruction output (streaming)
    Output {
        /// Output line content
        line: String,
        /// Whether this is stderr (true) or stdout (false)
        is_stderr: bool,
    },

    /// Instruction completed
    InstructionComplete {
        /// Stage index
        stage: usize,
        /// Instruction index
        index: usize,
        /// Whether this instruction was served from cache
        cached: bool,
    },

    /// Stage completed
    StageComplete {
        /// Stage index
        index: usize,
    },

    /// Build complete
    BuildComplete {
        /// Final image ID
        image_id: String,
    },

    /// Build failed
    BuildFailed {
        /// Error message
        error: String,
    },
}

/// Status of an instruction during build
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InstructionStatus {
    /// Instruction has not started yet
    #[default]
    Pending,
    /// Instruction is currently running
    Running,
    /// Instruction completed successfully
    Complete {
        /// Whether the result was served from cache
        cached: bool,
    },
    /// Instruction failed
    Failed,
}

impl InstructionStatus {
    /// Returns true if the instruction is complete (successfully or from cache)
    pub fn is_complete(&self) -> bool {
        matches!(self, Self::Complete { .. })
    }

    /// Returns true if the instruction is currently running
    pub fn is_running(&self) -> bool {
        matches!(self, Self::Running)
    }

    /// Returns true if the instruction failed
    pub fn is_failed(&self) -> bool {
        matches!(self, Self::Failed)
    }

    /// Returns the status indicator character
    pub fn indicator(&self) -> char {
        match self {
            Self::Pending => '\u{25CB}',         // ○
            Self::Running => '\u{25B6}',         // ▶
            Self::Complete { .. } => '\u{2713}', // ✓
            Self::Failed => '\u{2717}',          // ✗
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_instruction_status_indicator() {
        assert_eq!(InstructionStatus::Pending.indicator(), '\u{25CB}');
        assert_eq!(InstructionStatus::Running.indicator(), '\u{25B6}');
        assert_eq!(
            InstructionStatus::Complete { cached: false }.indicator(),
            '\u{2713}'
        );
        assert_eq!(InstructionStatus::Failed.indicator(), '\u{2717}');
    }

    #[test]
    fn test_instruction_status_states() {
        assert!(!InstructionStatus::Pending.is_complete());
        assert!(!InstructionStatus::Pending.is_running());
        assert!(!InstructionStatus::Pending.is_failed());

        assert!(!InstructionStatus::Running.is_complete());
        assert!(InstructionStatus::Running.is_running());
        assert!(!InstructionStatus::Running.is_failed());

        assert!(InstructionStatus::Complete { cached: false }.is_complete());
        assert!(!InstructionStatus::Complete { cached: true }.is_running());
        assert!(!InstructionStatus::Complete { cached: false }.is_failed());

        assert!(!InstructionStatus::Failed.is_complete());
        assert!(!InstructionStatus::Failed.is_running());
        assert!(InstructionStatus::Failed.is_failed());
    }
}
