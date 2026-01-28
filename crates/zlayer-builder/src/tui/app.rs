//! Main TUI application
//!
//! This module contains the main `BuildTui` application that manages
//! the terminal UI, event processing, and rendering loop.

use std::io::{self, Stdout};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::prelude::*;
use ratatui::Terminal;

use super::build_view::BuildView;
use super::{BuildEvent, InstructionStatus};

/// Main TUI application for build progress visualization
pub struct BuildTui {
    /// Channel to receive build events
    event_rx: Receiver<BuildEvent>,
    /// Current build state
    state: BuildState,
    /// Whether to keep running
    running: bool,
}

/// Internal build state tracking
#[derive(Debug, Default)]
pub struct BuildState {
    /// All stages in the build
    pub stages: Vec<StageState>,
    /// Current stage index
    pub current_stage: usize,
    /// Current instruction index within the current stage
    pub current_instruction: usize,
    /// Output lines from the build process
    pub output_lines: Vec<OutputLine>,
    /// Current scroll offset for output
    pub scroll_offset: usize,
    /// Whether the build has completed
    pub completed: bool,
    /// Error message if build failed
    pub error: Option<String>,
    /// Final image ID if build succeeded
    pub image_id: Option<String>,
}

/// State of a single build stage
#[derive(Debug, Clone)]
pub struct StageState {
    /// Stage index
    pub index: usize,
    /// Optional stage name
    pub name: Option<String>,
    /// Base image for this stage
    pub base_image: String,
    /// Instructions in this stage
    pub instructions: Vec<InstructionState>,
    /// Whether the stage is complete
    pub complete: bool,
}

/// State of a single instruction
#[derive(Debug, Clone)]
pub struct InstructionState {
    /// Instruction text
    pub text: String,
    /// Current status
    pub status: InstructionStatus,
}

/// A single line of output
#[derive(Debug, Clone)]
pub struct OutputLine {
    /// Line content
    pub text: String,
    /// Whether this is stderr
    pub is_stderr: bool,
}

impl BuildTui {
    /// Create a new TUI with an event receiver
    pub fn new(event_rx: Receiver<BuildEvent>) -> Self {
        Self {
            event_rx,
            state: BuildState::default(),
            running: true,
        }
    }

    /// Run the TUI (blocking)
    ///
    /// This will take over the terminal, display the build progress,
    /// and return when the build completes or the user quits (q key).
    pub fn run(&mut self) -> io::Result<()> {
        // Setup terminal
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

        // Main loop
        let result = self.run_loop(&mut terminal);

        // Restore terminal
        disable_raw_mode()?;
        execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
        terminal.show_cursor()?;

        result
    }

    /// Main event loop
    fn run_loop(&mut self, terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> io::Result<()> {
        while self.running {
            // Process any pending build events
            self.process_events();

            // Render the UI
            terminal.draw(|frame| self.render(frame))?;

            // Handle keyboard input with a short timeout
            if event::poll(Duration::from_millis(50))? {
                if let Event::Key(key) = event::read()? {
                    if key.kind == KeyEventKind::Press {
                        self.handle_input(key.code);
                    }
                }
            }

            // Stop if build completed and user acknowledged
            if self.state.completed && !self.running {
                break;
            }
        }

        Ok(())
    }

    /// Process pending build events from the channel
    fn process_events(&mut self) {
        loop {
            match self.event_rx.try_recv() {
                Ok(event) => self.handle_build_event(event),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    // Build process has finished sending events
                    if !self.state.completed {
                        // If we didn't get a completion event, treat as unexpected end
                        self.state.completed = true;
                        if self.state.error.is_none() && self.state.image_id.is_none() {
                            self.state.error = Some("Build ended unexpectedly".to_string());
                        }
                    }
                    break;
                }
            }
        }
    }

    /// Handle a single build event
    fn handle_build_event(&mut self, event: BuildEvent) {
        match event {
            BuildEvent::StageStarted {
                index,
                name,
                base_image,
            } => {
                // Ensure we have enough stages
                while self.state.stages.len() <= index {
                    self.state.stages.push(StageState {
                        index: self.state.stages.len(),
                        name: None,
                        base_image: String::new(),
                        instructions: Vec::new(),
                        complete: false,
                    });
                }

                // Update the stage
                self.state.stages[index] = StageState {
                    index,
                    name,
                    base_image,
                    instructions: Vec::new(),
                    complete: false,
                };
                self.state.current_stage = index;
                self.state.current_instruction = 0;
            }

            BuildEvent::InstructionStarted {
                stage,
                index,
                instruction,
            } => {
                if let Some(stage_state) = self.state.stages.get_mut(stage) {
                    // Ensure we have enough instructions
                    while stage_state.instructions.len() <= index {
                        stage_state.instructions.push(InstructionState {
                            text: String::new(),
                            status: InstructionStatus::Pending,
                        });
                    }

                    // Update the instruction
                    stage_state.instructions[index] = InstructionState {
                        text: instruction,
                        status: InstructionStatus::Running,
                    };
                    self.state.current_instruction = index;
                }
            }

            BuildEvent::Output { line, is_stderr } => {
                self.state.output_lines.push(OutputLine {
                    text: line,
                    is_stderr,
                });

                // Auto-scroll to bottom if we were already at the bottom
                let visible_lines = 10; // Approximate
                let max_scroll = self.state.output_lines.len().saturating_sub(visible_lines);
                if self.state.scroll_offset >= max_scroll.saturating_sub(1) {
                    self.state.scroll_offset =
                        self.state.output_lines.len().saturating_sub(visible_lines);
                }
            }

            BuildEvent::InstructionComplete {
                stage,
                index,
                cached,
            } => {
                if let Some(stage_state) = self.state.stages.get_mut(stage) {
                    if let Some(inst) = stage_state.instructions.get_mut(index) {
                        inst.status = InstructionStatus::Complete { cached };
                    }
                }
            }

            BuildEvent::StageComplete { index } => {
                if let Some(stage_state) = self.state.stages.get_mut(index) {
                    stage_state.complete = true;
                }
            }

            BuildEvent::BuildComplete { image_id } => {
                self.state.completed = true;
                self.state.image_id = Some(image_id);
            }

            BuildEvent::BuildFailed { error } => {
                self.state.completed = true;
                self.state.error = Some(error);

                // Mark current instruction as failed
                if let Some(stage_state) = self.state.stages.get_mut(self.state.current_stage) {
                    if let Some(inst) = stage_state
                        .instructions
                        .get_mut(self.state.current_instruction)
                    {
                        if inst.status.is_running() {
                            inst.status = InstructionStatus::Failed;
                        }
                    }
                }
            }
        }
    }

    /// Handle keyboard input
    fn handle_input(&mut self, key: KeyCode) {
        match key {
            KeyCode::Char('q') | KeyCode::Esc => {
                self.running = false;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.state.scroll_offset = self.state.scroll_offset.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let max_scroll = self.state.output_lines.len().saturating_sub(10);
                if self.state.scroll_offset < max_scroll {
                    self.state.scroll_offset += 1;
                }
            }
            KeyCode::PageUp => {
                self.state.scroll_offset = self.state.scroll_offset.saturating_sub(10);
            }
            KeyCode::PageDown => {
                let max_scroll = self.state.output_lines.len().saturating_sub(10);
                self.state.scroll_offset = (self.state.scroll_offset + 10).min(max_scroll);
            }
            KeyCode::Home => {
                self.state.scroll_offset = 0;
            }
            KeyCode::End => {
                let max_scroll = self.state.output_lines.len().saturating_sub(10);
                self.state.scroll_offset = max_scroll;
            }
            _ => {}
        }
    }

    /// Render the UI
    fn render(&self, frame: &mut Frame) {
        let view = BuildView::new(&self.state);
        frame.render_widget(view, frame.area());
    }
}

impl BuildState {
    /// Get the total number of instructions across all stages
    pub fn total_instructions(&self) -> usize {
        self.stages.iter().map(|s| s.instructions.len()).sum()
    }

    /// Get the number of completed instructions across all stages
    pub fn completed_instructions(&self) -> usize {
        self.stages
            .iter()
            .flat_map(|s| s.instructions.iter())
            .filter(|i| i.status.is_complete())
            .count()
    }

    /// Get a display string for the current stage
    pub fn current_stage_display(&self) -> String {
        if let Some(stage) = self.stages.get(self.current_stage) {
            let name_part = stage
                .name
                .as_ref()
                .map(|n| format!("{} ", n))
                .unwrap_or_default();
            format!(
                "Stage {}/{}: {}({})",
                self.current_stage + 1,
                self.stages.len().max(1),
                name_part,
                stage.base_image
            )
        } else {
            "Initializing...".to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn test_build_state_default() {
        let state = BuildState::default();
        assert!(state.stages.is_empty());
        assert!(!state.completed);
        assert!(state.error.is_none());
        assert!(state.image_id.is_none());
    }

    #[test]
    fn test_build_state_instruction_counts() {
        let mut state = BuildState::default();
        state.stages.push(StageState {
            index: 0,
            name: None,
            base_image: "alpine".to_string(),
            instructions: vec![
                InstructionState {
                    text: "RUN echo 1".to_string(),
                    status: InstructionStatus::Complete { cached: false },
                },
                InstructionState {
                    text: "RUN echo 2".to_string(),
                    status: InstructionStatus::Running,
                },
            ],
            complete: false,
        });

        assert_eq!(state.total_instructions(), 2);
        assert_eq!(state.completed_instructions(), 1);
    }

    #[test]
    fn test_handle_stage_started() {
        let (tx, rx) = mpsc::channel();
        let mut tui = BuildTui::new(rx);

        tx.send(BuildEvent::StageStarted {
            index: 0,
            name: Some("builder".to_string()),
            base_image: "node:20".to_string(),
        })
        .unwrap();

        drop(tx);
        tui.process_events();

        assert_eq!(tui.state.stages.len(), 1);
        assert_eq!(tui.state.stages[0].name, Some("builder".to_string()));
        assert_eq!(tui.state.stages[0].base_image, "node:20");
    }

    #[test]
    fn test_handle_instruction_lifecycle() {
        let (tx, rx) = mpsc::channel();
        let mut tui = BuildTui::new(rx);

        // Start stage
        tx.send(BuildEvent::StageStarted {
            index: 0,
            name: None,
            base_image: "alpine".to_string(),
        })
        .unwrap();

        // Start instruction
        tx.send(BuildEvent::InstructionStarted {
            stage: 0,
            index: 0,
            instruction: "RUN echo hello".to_string(),
        })
        .unwrap();

        // Complete instruction
        tx.send(BuildEvent::InstructionComplete {
            stage: 0,
            index: 0,
            cached: true,
        })
        .unwrap();

        drop(tx);
        tui.process_events();

        let inst = &tui.state.stages[0].instructions[0];
        assert_eq!(inst.text, "RUN echo hello");
        assert!(matches!(
            inst.status,
            InstructionStatus::Complete { cached: true }
        ));
    }

    #[test]
    fn test_handle_build_complete() {
        let (tx, rx) = mpsc::channel();
        let mut tui = BuildTui::new(rx);

        tx.send(BuildEvent::BuildComplete {
            image_id: "sha256:abc123".to_string(),
        })
        .unwrap();

        drop(tx);
        tui.process_events();

        assert!(tui.state.completed);
        assert_eq!(tui.state.image_id, Some("sha256:abc123".to_string()));
        assert!(tui.state.error.is_none());
    }

    #[test]
    fn test_handle_build_failed() {
        let (tx, rx) = mpsc::channel();
        let mut tui = BuildTui::new(rx);

        // Set up a running instruction first
        tx.send(BuildEvent::StageStarted {
            index: 0,
            name: None,
            base_image: "alpine".to_string(),
        })
        .unwrap();

        tx.send(BuildEvent::InstructionStarted {
            stage: 0,
            index: 0,
            instruction: "RUN exit 1".to_string(),
        })
        .unwrap();

        tx.send(BuildEvent::BuildFailed {
            error: "Command failed with exit code 1".to_string(),
        })
        .unwrap();

        drop(tx);
        tui.process_events();

        assert!(tui.state.completed);
        assert!(tui.state.error.is_some());
        assert!(tui.state.stages[0].instructions[0].status.is_failed());
    }
}
