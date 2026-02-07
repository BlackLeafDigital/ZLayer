//! Main TUI application for deploy progress visualization
//!
//! This module contains the `DeployTui` application that manages the terminal
//! UI, event processing, and rendering loop. It follows the same architecture
//! as `BuildTui` in `zlayer-builder::tui::app`.
//!
//! # Architecture
//!
//! ```text
//! +-------------------+     mpsc::Receiver<DeployEvent>     +-------------+
//! | deploy_services() | ----------------------------------> |  DeployTui  |
//! +-------------------+                                     +-------------+
//!          ^                                                       |
//!          |                   Arc<Notify>                         |
//!          +--- shutdown_signal <---------------------------------+
//! ```
//!
//! The TUI runs on the main thread (blocking). It reads `DeployEvent`s from
//! an `mpsc::Receiver`, updates `DeployState`, and renders via ratatui.
//! When the user presses Ctrl+C (captured as a key event in raw mode),
//! the TUI notifies the deploy task via a shared `Arc<Notify>`.

use std::io::{self, Stdout};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::prelude::*;
use ratatui::Terminal;
use tokio::sync::Notify;

use super::deploy_view::DeployView;
use super::state::{DeployPhase, DeployState};
use super::DeployEvent;

/// Main TUI application for deployment progress visualization
///
/// Takes ownership of a `Receiver<DeployEvent>` channel and renders the
/// deployment progress in an alternate terminal screen. The `shutdown_signal`
/// is an `Arc<Notify>` shared with the deploy orchestration task; when the
/// user presses Ctrl+C in the TUI, it calls `notify_one()` to trigger
/// graceful shutdown.
pub struct DeployTui {
    /// Channel to receive deploy events from the orchestration task
    event_rx: Receiver<DeployEvent>,
    /// Current deployment state (fed by events)
    state: DeployState,
    /// Whether the TUI should keep running
    running: bool,
    /// Frame counter for spinner animation (wraps around)
    frame_counter: usize,
    /// Shared shutdown signal - when notified, the deploy task begins shutdown
    shutdown_signal: Arc<Notify>,
}

impl DeployTui {
    /// Create a new deploy TUI
    ///
    /// # Arguments
    ///
    /// * `event_rx` - Channel receiver for `DeployEvent`s from the deploy orchestration
    /// * `shutdown_signal` - Shared `Notify` that the TUI will trigger on Ctrl+C
    pub fn new(event_rx: Receiver<DeployEvent>, shutdown_signal: Arc<Notify>) -> Self {
        Self {
            event_rx,
            state: DeployState::new(),
            running: true,
            frame_counter: 0,
            shutdown_signal,
        }
    }

    /// Run the TUI (blocking)
    ///
    /// This takes over the terminal (raw mode + alternate screen), runs the
    /// event/render loop, and restores the terminal on exit. Returns when:
    /// - The deployment completes (ShutdownComplete received) and user presses q
    /// - The user presses q/Esc during the Running phase
    /// - The event channel disconnects
    pub fn run(&mut self) -> io::Result<()> {
        // Setup terminal
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

        // Main loop
        let result = self.run_loop(&mut terminal);

        // Restore terminal (always, even on error)
        disable_raw_mode()?;
        execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
        terminal.show_cursor()?;

        result
    }

    /// Main event loop
    ///
    /// Runs at ~20fps (50ms poll timeout). Each iteration:
    /// 1. Drains pending deploy events from the channel
    /// 2. Renders the current state
    /// 3. Polls for keyboard input
    fn run_loop(&mut self, terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> io::Result<()> {
        while self.running {
            // Process any pending deploy events
            self.process_events();

            // Render the UI
            terminal.draw(|frame| self.render(frame))?;

            // Handle keyboard input with a short timeout (~20fps)
            if event::poll(Duration::from_millis(50))? {
                if let Event::Key(key) = event::read()? {
                    if key.kind == KeyEventKind::Press {
                        self.handle_input(key.code, key.modifiers);
                    }
                }
            }

            // Advance frame counter for spinner animation
            self.frame_counter = self.frame_counter.wrapping_add(1);
        }

        Ok(())
    }

    /// Process all pending deploy events from the channel
    ///
    /// Drains the channel without blocking. If the channel disconnects,
    /// behavior depends on the current phase:
    /// - **Running** or **Complete**: The deploy task finished normally
    ///   (background/detach mode or shutdown completed). Auto-exit the TUI.
    /// - **ShuttingDown**: Shutdown is in progress. Mark complete and auto-exit.
    /// - **Deploying** / **Stabilizing** / **Initializing**: Unexpected disconnect.
    ///   Show an error message and wait for the user to press q.
    fn process_events(&mut self) {
        loop {
            match self.event_rx.try_recv() {
                Ok(event) => {
                    self.state.apply_event(&event);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    // Channel closed - deploy task has finished
                    match self.state.phase {
                        DeployPhase::Running | DeployPhase::Complete => {
                            // Normal completion: background/detach mode returned after
                            // DeploymentRunning, or foreground shutdown completed.
                            // Auto-exit the TUI.
                            if self.state.phase != DeployPhase::Complete {
                                self.state
                                    .apply_event(&DeployEvent::ShutdownComplete);
                            }
                            self.running = false;
                        }
                        DeployPhase::ShuttingDown => {
                            // Shutdown was in progress; the deploy task finished cleanup.
                            // Mark complete and auto-exit.
                            self.state
                                .apply_event(&DeployEvent::ShutdownComplete);
                            self.running = false;
                        }
                        _ => {
                            // Unexpected disconnect during Initializing/Deploying/Stabilizing.
                            // Show error and let the user review logs before quitting.
                            self.state.apply_event(&DeployEvent::Log {
                                level: super::LogLevel::Error,
                                message: "Deploy process ended unexpectedly".to_string(),
                            });
                            self.state
                                .apply_event(&DeployEvent::ShutdownComplete);
                            // Do NOT set self.running = false; wait for user to press q.
                        }
                    }
                    break;
                }
            }
        }
    }

    /// Handle a keyboard input event
    ///
    /// Key bindings:
    /// - `q` / `Esc`: Quit (only after ShutdownComplete or during Running)
    /// - `Ctrl+C`: Signal shutdown to the deploy task, then wait for completion
    /// - `Up` / `k`: Scroll log up by 1
    /// - `Down` / `j`: Scroll log down by 1
    /// - `PageUp`: Scroll log up by 10
    /// - `PageDown`: Scroll log down by 10
    /// - `Home`: Scroll log to top
    /// - `End`: Scroll log to bottom
    fn handle_input(&mut self, key: KeyCode, modifiers: KeyModifiers) {
        // Ctrl+C handling: in raw mode this is a key event, not a signal
        if key == KeyCode::Char('c') && modifiers.contains(KeyModifiers::CONTROL) {
            if self.state.phase == DeployPhase::Complete {
                // Already done, just quit
                self.running = false;
            } else if self.state.phase != DeployPhase::ShuttingDown {
                // Signal the deploy task to begin graceful shutdown
                self.shutdown_signal.notify_one();
                self.state.apply_event(&DeployEvent::Log {
                    level: super::LogLevel::Warn,
                    message: "Shutdown requested via Ctrl+C...".to_string(),
                });
            }
            // If already shutting down, do nothing (wait for completion)
            return;
        }

        match key {
            KeyCode::Char('q') | KeyCode::Esc => {
                // Allow quit when deployment is complete or running
                match self.state.phase {
                    DeployPhase::Complete | DeployPhase::Running => {
                        self.running = false;
                    }
                    DeployPhase::ShuttingDown => {
                        // During shutdown, 'q' forces quit
                        self.running = false;
                    }
                    _ => {
                        // During deploy/stabilize, don't allow quit without Ctrl+C
                    }
                }
            }

            // Log scrolling
            KeyCode::Up | KeyCode::Char('k') => {
                self.state.scroll_up(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.state.scroll_down(1);
            }
            KeyCode::PageUp => {
                self.state.scroll_up(10);
            }
            KeyCode::PageDown => {
                self.state.scroll_down(10);
            }
            KeyCode::Home => {
                self.state.log_scroll_offset = 0;
            }
            KeyCode::End => {
                self.state.scroll_to_bottom();
            }

            _ => {}
        }
    }

    /// Render the UI by creating a DeployView widget and rendering it
    fn render(&self, frame: &mut Frame) {
        let view = DeployView::new(&self.state, self.frame_counter);
        frame.render_widget(view, frame.area());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deploy_tui::{InfraPhase, LogLevel, ServiceHealth, ServicePlan, ServiceStatus};
    use std::sync::mpsc;

    fn create_tui() -> (mpsc::Sender<DeployEvent>, DeployTui) {
        let (tx, rx) = mpsc::channel();
        let shutdown = Arc::new(Notify::new());
        let tui = DeployTui::new(rx, shutdown);
        (tx, tui)
    }

    #[test]
    fn test_new_creates_default_state() {
        let (_tx, tui) = create_tui();
        assert!(tui.running);
        assert_eq!(tui.frame_counter, 0);
        assert_eq!(tui.state.phase, DeployPhase::Initializing);
    }

    #[test]
    fn test_process_events_applies_to_state() {
        let (tx, mut tui) = create_tui();

        tx.send(DeployEvent::PlanReady {
            deployment_name: "test-app".to_string(),
            version: "v1.0".to_string(),
            services: vec![ServicePlan {
                name: "web".to_string(),
                image: "nginx:latest".to_string(),
                scale_mode: "fixed(2)".to_string(),
                endpoints: vec![],
            }],
        })
        .unwrap();

        tx.send(DeployEvent::InfraPhaseStarted {
            phase: InfraPhase::Runtime,
        })
        .unwrap();

        drop(tx); // Close channel
        tui.process_events();

        assert_eq!(tui.state.deployment_name, "test-app");
        // PlanReady sets phase to Deploying, then disconnect during Deploying
        // is unexpected - synthesizes error + ShutdownComplete but keeps running
        assert_eq!(tui.state.phase, DeployPhase::Complete);
        // TUI should still be running because disconnect was unexpected (during Deploying)
        assert!(tui.running);
    }

    #[test]
    fn test_process_events_synthesizes_completion_on_unexpected_disconnect() {
        let (tx, mut tui) = create_tui();

        tx.send(DeployEvent::InfraPhaseStarted {
            phase: InfraPhase::Runtime,
        })
        .unwrap();

        drop(tx);
        tui.process_events();

        // Should have synthesized ShutdownComplete (unexpected disconnect during Initializing)
        assert_eq!(tui.state.phase, DeployPhase::Complete);
        // But should still be running - user needs to press q to acknowledge the error
        assert!(tui.running);
        // Should have an error log entry
        assert!(tui.state.log_entries.iter().any(|e| e.message.contains("unexpectedly")));
    }

    #[test]
    fn test_process_events_auto_exits_on_running_disconnect() {
        let (tx, mut tui) = create_tui();

        // Simulate a successful deployment that transitions to Running
        tx.send(DeployEvent::DeploymentRunning {
            services: vec![("web".to_string(), 2)],
        })
        .unwrap();

        drop(tx);
        tui.process_events();

        // Channel closed while in Running phase - this is normal (background/detach)
        assert_eq!(tui.state.phase, DeployPhase::Complete);
        // Should auto-exit
        assert!(!tui.running);
    }

    #[test]
    fn test_process_events_auto_exits_on_shutting_down_disconnect() {
        let (tx, mut tui) = create_tui();

        tx.send(DeployEvent::ShutdownStarted).unwrap();

        drop(tx);
        tui.process_events();

        // Channel closed during ShuttingDown - deploy task finished cleanup
        assert_eq!(tui.state.phase, DeployPhase::Complete);
        assert!(!tui.running);
    }

    #[test]
    fn test_process_events_no_double_complete() {
        let (tx, mut tui) = create_tui();

        tx.send(DeployEvent::ShutdownComplete).unwrap();
        drop(tx);
        tui.process_events();

        // Phase should be Complete from the event, not double-applied
        assert_eq!(tui.state.phase, DeployPhase::Complete);
        // Auto-exit because phase was already Complete when channel disconnected
        assert!(!tui.running);
    }

    #[test]
    fn test_handle_input_quit_when_complete() {
        let (_tx, mut tui) = create_tui();
        tui.state.phase = DeployPhase::Complete;

        tui.handle_input(KeyCode::Char('q'), KeyModifiers::NONE);
        assert!(!tui.running);
    }

    #[test]
    fn test_handle_input_quit_when_running() {
        let (_tx, mut tui) = create_tui();
        tui.state.phase = DeployPhase::Running;

        tui.handle_input(KeyCode::Char('q'), KeyModifiers::NONE);
        assert!(!tui.running);
    }

    #[test]
    fn test_handle_input_esc_when_complete() {
        let (_tx, mut tui) = create_tui();
        tui.state.phase = DeployPhase::Complete;

        tui.handle_input(KeyCode::Esc, KeyModifiers::NONE);
        assert!(!tui.running);
    }

    #[test]
    fn test_handle_input_no_quit_while_deploying() {
        let (_tx, mut tui) = create_tui();
        tui.state.phase = DeployPhase::Deploying;

        tui.handle_input(KeyCode::Char('q'), KeyModifiers::NONE);
        assert!(tui.running); // Should still be running
    }

    #[test]
    fn test_handle_input_ctrl_c_triggers_shutdown() {
        let (tx, mut tui) = create_tui();
        tui.state.phase = DeployPhase::Running;

        // Keep tx alive so channel doesn't disconnect
        let _tx = tx;

        tui.handle_input(KeyCode::Char('c'), KeyModifiers::CONTROL);

        // Should have added a log entry about shutdown
        assert!(!tui.state.log_entries.is_empty());
        assert!(tui.state.log_entries.last().unwrap().message.contains("Shutdown"));
        // Should still be running (waiting for completion)
        assert!(tui.running);
    }

    #[test]
    fn test_handle_input_ctrl_c_when_complete_quits() {
        let (_tx, mut tui) = create_tui();
        tui.state.phase = DeployPhase::Complete;

        tui.handle_input(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(!tui.running);
    }

    #[test]
    fn test_handle_input_ctrl_c_during_shutdown_is_noop() {
        let (_tx, mut tui) = create_tui();
        tui.state.phase = DeployPhase::ShuttingDown;
        let initial_log_count = tui.state.log_entries.len();

        tui.handle_input(KeyCode::Char('c'), KeyModifiers::CONTROL);

        // Should NOT add another log entry
        assert_eq!(tui.state.log_entries.len(), initial_log_count);
        // Should still be running
        assert!(tui.running);
    }

    #[test]
    fn test_scroll_up_down() {
        let (_tx, mut tui) = create_tui();

        // Add log entries
        for i in 0..20 {
            tui.state.apply_event(&DeployEvent::Log {
                level: LogLevel::Info,
                message: format!("line {}", i),
            });
        }

        let max_offset = tui.state.log_scroll_offset;
        assert_eq!(max_offset, 20);

        // Scroll up
        tui.handle_input(KeyCode::Up, KeyModifiers::NONE);
        assert_eq!(tui.state.log_scroll_offset, 19);

        // Scroll up with k
        tui.handle_input(KeyCode::Char('k'), KeyModifiers::NONE);
        assert_eq!(tui.state.log_scroll_offset, 18);

        // Scroll down with j
        tui.handle_input(KeyCode::Char('j'), KeyModifiers::NONE);
        assert_eq!(tui.state.log_scroll_offset, 19);

        // Scroll down
        tui.handle_input(KeyCode::Down, KeyModifiers::NONE);
        assert_eq!(tui.state.log_scroll_offset, 20);
    }

    #[test]
    fn test_page_up_down() {
        let (_tx, mut tui) = create_tui();

        for i in 0..30 {
            tui.state.apply_event(&DeployEvent::Log {
                level: LogLevel::Info,
                message: format!("line {}", i),
            });
        }

        // Scroll up by page
        tui.handle_input(KeyCode::PageUp, KeyModifiers::NONE);
        assert_eq!(tui.state.log_scroll_offset, 20);

        // Page up again
        tui.handle_input(KeyCode::PageUp, KeyModifiers::NONE);
        assert_eq!(tui.state.log_scroll_offset, 10);

        // Page down
        tui.handle_input(KeyCode::PageDown, KeyModifiers::NONE);
        assert_eq!(tui.state.log_scroll_offset, 20);
    }

    #[test]
    fn test_home_end() {
        let (_tx, mut tui) = create_tui();

        for i in 0..20 {
            tui.state.apply_event(&DeployEvent::Log {
                level: LogLevel::Info,
                message: format!("line {}", i),
            });
        }

        // Home
        tui.handle_input(KeyCode::Home, KeyModifiers::NONE);
        assert_eq!(tui.state.log_scroll_offset, 0);

        // End
        tui.handle_input(KeyCode::End, KeyModifiers::NONE);
        assert_eq!(tui.state.log_scroll_offset, 20);
    }

    #[test]
    fn test_full_event_sequence() {
        let (tx, mut tui) = create_tui();

        // Simulate a full deployment lifecycle
        let events = vec![
            DeployEvent::PlanReady {
                deployment_name: "prod".to_string(),
                version: "v2.0".to_string(),
                services: vec![ServicePlan {
                    name: "web".to_string(),
                    image: "web:v2.0".to_string(),
                    scale_mode: "fixed(3)".to_string(),
                    endpoints: vec!["http:80".to_string()],
                }],
            },
            DeployEvent::InfraPhaseStarted {
                phase: InfraPhase::Runtime,
            },
            DeployEvent::InfraPhaseComplete {
                phase: InfraPhase::Runtime,
                success: true,
                message: None,
            },
            DeployEvent::ServiceDeployStarted {
                name: "web".to_string(),
            },
            DeployEvent::ServiceRegistered {
                name: "web".to_string(),
            },
            DeployEvent::ServiceScaling {
                name: "web".to_string(),
                target_replicas: 3,
            },
            DeployEvent::ServiceReplicaUpdate {
                name: "web".to_string(),
                current: 2,
                target: 3,
            },
            DeployEvent::ServiceDeployComplete {
                name: "web".to_string(),
                replicas: 3,
            },
            DeployEvent::DeploymentRunning {
                services: vec![("web".to_string(), 3)],
            },
            DeployEvent::Log {
                level: LogLevel::Info,
                message: "All services running".to_string(),
            },
            DeployEvent::StatusTick {
                services: vec![ServiceStatus {
                    name: "web".to_string(),
                    replicas_running: 3,
                    replicas_target: 3,
                    health: ServiceHealth::Healthy,
                }],
            },
            DeployEvent::ShutdownStarted,
            DeployEvent::ServiceStopping {
                name: "web".to_string(),
            },
            DeployEvent::ServiceStopped {
                name: "web".to_string(),
            },
            DeployEvent::ShutdownComplete,
        ];

        for event in events {
            tx.send(event).unwrap();
        }
        drop(tx);

        tui.process_events();

        assert_eq!(tui.state.phase, DeployPhase::Complete);
        assert_eq!(tui.state.deployment_name, "prod");
        assert_eq!(tui.state.services.len(), 1);
        assert!(!tui.state.log_entries.is_empty());
        // Full lifecycle ends at Complete, channel disconnect auto-exits
        assert!(!tui.running);
    }

    #[test]
    fn test_unrecognized_key_is_noop() {
        let (_tx, mut tui) = create_tui();
        let before = tui.state.log_scroll_offset;

        tui.handle_input(KeyCode::Char('x'), KeyModifiers::NONE);

        assert_eq!(tui.state.log_scroll_offset, before);
        assert!(tui.running);
    }

    #[test]
    fn test_quit_during_shutdown_forces_exit() {
        let (_tx, mut tui) = create_tui();
        tui.state.phase = DeployPhase::ShuttingDown;

        tui.handle_input(KeyCode::Char('q'), KeyModifiers::NONE);
        assert!(!tui.running);
    }
}
