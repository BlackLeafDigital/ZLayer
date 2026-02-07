//! Deploy progress view layout
//!
//! This module contains the main deploy view widget that composes
//! the full deployment progress display. It is phase-aware and adapts
//! its layout based on the current deployment lifecycle stage.
//!
//! Rendering of infrastructure, services, and logs is delegated to
//! reusable widgets from [`super::widgets`].
//!
//! # Layout
//!
//! ```text
//! +--------------------------------------------------+
//! |  ZLayer Deploy: my-app v1.0 | DEPLOYING          |  <- Header (3 lines)
//! +--------------------------------------------------+
//! |  v Runtime    v DNS       |/-\ Proxy             |  <- Infra (5 lines)
//! |  v Overlay    v Supervisor  . API                 |
//! +--------------------------------------------------+
//! |  Status | Name   | Replicas | Health | Progress   |  <- Services (flexible)
//! |  v      | web    | 3/3      | OK     | ████████   |
//! |  |      | api    | 1/3      | ...    | ███░░░░░   |
//! +--------------------------------------------------+
//! |  [INFO] Starting container runtime...             |  <- Logs (flexible)
//! |  [WARN] Overlay not available, skipping           |
//! +--------------------------------------------------+
//! |  q: quit | up/dn: scroll logs | Ctrl+C: shutdown  |  <- Footer (1 line)
//! +--------------------------------------------------+
//! ```

use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};

use super::state::{DeployPhase, DeployState};
use super::widgets::{InfraProgress, LogPane, ServiceTable};

/// Main deploy progress view widget
///
/// Renders the full deployment TUI by composing header, infrastructure,
/// services, log, and footer sections. The layout adapts based on the
/// current `DeployPhase`.
pub struct DeployView<'a> {
    /// Reference to the current deploy state
    state: &'a DeployState,
    /// Frame counter for spinner animation (incremented each render tick)
    frame: usize,
}

impl<'a> DeployView<'a> {
    /// Create a new deploy view with the given state and frame counter
    pub fn new(state: &'a DeployState, frame: usize) -> Self {
        Self { state, frame }
    }

    /// Calculate the layout regions based on current deploy phase
    fn layout(&self, area: Rect) -> ViewLayout {
        match self.state.phase {
            DeployPhase::Running => {
                // In Running mode, collapse the infra section
                let chunks = Layout::vertical([
                    Constraint::Length(3), // Header
                    Constraint::Min(6),    // Services (dashboard mode)
                    Constraint::Min(4),    // Logs
                    Constraint::Length(1), // Footer
                ])
                .split(area);

                ViewLayout {
                    header: chunks[0],
                    infra: None,
                    services: chunks[1],
                    logs: chunks[2],
                    footer: chunks[3],
                }
            }
            _ => {
                // Full layout with all sections
                let chunks = Layout::vertical([
                    Constraint::Length(3), // Header
                    Constraint::Length(5), // Infrastructure phases
                    Constraint::Min(6),    // Services
                    Constraint::Min(4),    // Logs
                    Constraint::Length(1), // Footer
                ])
                .split(area);

                ViewLayout {
                    header: chunks[0],
                    infra: Some(chunks[1]),
                    services: chunks[2],
                    logs: chunks[3],
                    footer: chunks[4],
                }
            }
        }
    }
}

/// Regions of the view layout
struct ViewLayout {
    header: Rect,
    infra: Option<Rect>,
    services: Rect,
    logs: Rect,
    footer: Rect,
}

impl Widget for DeployView<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let layout = self.layout(area);

        self.render_header(layout.header, buf);

        if let Some(infra_area) = layout.infra {
            self.render_infra(infra_area, buf);
        }

        self.render_services(layout.services, buf);
        self.render_logs(layout.logs, buf);
        self.render_footer(layout.footer, buf);
    }
}

impl DeployView<'_> {
    // ---- Header ----

    /// Render the header section with deployment name, version, and phase
    fn render_header(&self, area: Rect, buf: &mut Buffer) {
        let (phase_text, phase_style) = self.phase_display();

        let title_text = if self.state.deployment_name.is_empty() {
            format!("  ZLayer Deploy | {}", phase_text)
        } else if self.state.version.is_empty() {
            format!(
                "  ZLayer Deploy: {} | {}",
                self.state.deployment_name, phase_text
            )
        } else {
            format!(
                "  ZLayer Deploy: {} {} | {}",
                self.state.deployment_name, self.state.version, phase_text
            )
        };

        let block = Block::default()
            .title(" Deploy ")
            .borders(Borders::ALL)
            .border_style(self.header_border_style());

        let inner = block.inner(area);
        block.render(area, buf);

        Paragraph::new(title_text)
            .style(phase_style)
            .render(inner, buf);
    }

    /// Get display text and style for the current phase
    fn phase_display(&self) -> (String, Style) {
        match self.state.phase {
            DeployPhase::Initializing => (
                "INITIALIZING".to_string(),
                Style::default().fg(Color::DarkGray),
            ),
            DeployPhase::Deploying => (
                format!(
                    "DEPLOYING ({}/{})",
                    self.state.services_deployed_count(),
                    self.state.services.len()
                ),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            DeployPhase::Stabilizing => (
                "STABILIZING".to_string(),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            DeployPhase::Running => (
                "RUNNING".to_string(),
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            DeployPhase::ShuttingDown => (
                "SHUTTING DOWN".to_string(),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            DeployPhase::Complete => (
                "COMPLETE".to_string(),
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
        }
    }

    /// Border style for the header based on deployment phase
    fn header_border_style(&self) -> Style {
        match self.state.phase {
            DeployPhase::Initializing => Style::default().fg(Color::DarkGray),
            DeployPhase::Deploying | DeployPhase::Stabilizing => Style::default().fg(Color::Blue),
            DeployPhase::Running | DeployPhase::Complete => Style::default().fg(Color::Green),
            DeployPhase::ShuttingDown => Style::default().fg(Color::Yellow),
        }
    }

    // ---- Infrastructure ----

    /// Render the infrastructure phases by delegating to [`InfraProgress`]
    fn render_infra(&self, area: Rect, buf: &mut Buffer) {
        let widget = InfraProgress {
            phases: &self.state.infra_phases,
            tick: self.frame,
        };
        widget.render(area, buf);
    }

    // ---- Services ----

    /// Render the services section by delegating to [`ServiceTable`]
    fn render_services(&self, area: Rect, buf: &mut Buffer) {
        let widget = ServiceTable {
            services: &self.state.services,
            deployed_count: self.state.services_deployed_count(),
        };
        widget.render(area, buf);
    }

    // ---- Logs ----

    /// Render the scrollable log section by delegating to [`LogPane`]
    fn render_logs(&self, area: Rect, buf: &mut Buffer) {
        let widget = LogPane {
            entries: &self.state.log_entries,
            scroll_offset: self.state.log_scroll_offset,
        };
        widget.render(area, buf);
    }

    // ---- Footer ----

    /// Render the footer with keybind help text
    fn render_footer(&self, area: Rect, buf: &mut Buffer) {
        let help_text = match self.state.phase {
            DeployPhase::Complete => " q: quit",
            DeployPhase::ShuttingDown => " Shutting down... | q: force quit",
            _ => " q: quit | \u{2191}\u{2193}: scroll logs | Ctrl+C: shutdown",
        };

        Paragraph::new(help_text)
            .style(Style::default().fg(Color::DarkGray))
            .render(area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deploy_tui::state::DeployState;
    use crate::deploy_tui::ServicePlan;

    fn create_test_state() -> DeployState {
        let mut state = DeployState::new();
        state.apply_event(&DeployEvent::PlanReady {
            deployment_name: "my-app".to_string(),
            version: "v1.0".to_string(),
            services: vec![
                ServicePlan {
                    name: "web".to_string(),
                    image: "nginx:latest".to_string(),
                    scale_mode: "fixed(2)".to_string(),
                    endpoints: vec!["http:80 (public)".to_string()],
                },
                ServicePlan {
                    name: "api".to_string(),
                    image: "api:latest".to_string(),
                    scale_mode: "fixed(3)".to_string(),
                    endpoints: vec![],
                },
            ],
        });
        state
    }

    use crate::deploy_tui::DeployEvent;
    use crate::deploy_tui::LogLevel;

    fn create_buffer(width: u16, height: u16) -> Buffer {
        Buffer::empty(Rect::new(0, 0, width, height))
    }

    #[test]
    fn test_deploy_view_creation() {
        let state = DeployState::new();
        let view = DeployView::new(&state, 0);
        assert_eq!(view.frame, 0);
    }

    #[test]
    fn test_layout_with_infra_section() {
        let state = DeployState::new();
        let view = DeployView::new(&state, 0);
        let area = Rect::new(0, 0, 80, 30);
        let layout = view.layout(area);

        // Non-running state should have infra section
        assert!(layout.infra.is_some());
    }

    #[test]
    fn test_layout_running_hides_infra() {
        let mut state = DeployState::new();
        state.phase = DeployPhase::Running;
        let view = DeployView::new(&state, 0);
        let area = Rect::new(0, 0, 80, 30);
        let layout = view.layout(area);

        // Running state should collapse infra section
        assert!(layout.infra.is_none());
    }

    #[test]
    fn test_phase_display_all_variants() {
        let mut state = DeployState::new();

        let phases = [
            DeployPhase::Initializing,
            DeployPhase::Deploying,
            DeployPhase::Stabilizing,
            DeployPhase::Running,
            DeployPhase::ShuttingDown,
            DeployPhase::Complete,
        ];

        for phase in phases {
            state.phase = phase;
            let view = DeployView::new(&state, 0);
            let (text, _style) = view.phase_display();
            assert!(
                !text.is_empty(),
                "Phase {:?} should have display text",
                phase
            );
        }
    }

    #[test]
    fn test_header_border_style_variants() {
        let mut state = DeployState::new();

        state.phase = DeployPhase::Initializing;
        let view = DeployView::new(&state, 0);
        assert_eq!(view.header_border_style().fg, Some(Color::DarkGray));

        state.phase = DeployPhase::Running;
        let view = DeployView::new(&state, 0);
        assert_eq!(view.header_border_style().fg, Some(Color::Green));

        state.phase = DeployPhase::ShuttingDown;
        let view = DeployView::new(&state, 0);
        assert_eq!(view.header_border_style().fg, Some(Color::Yellow));
    }

    #[test]
    fn test_render_initializing_no_panic() {
        let state = DeployState::new();
        let view = DeployView::new(&state, 0);
        let area = Rect::new(0, 0, 80, 30);
        let mut buf = create_buffer(80, 30);
        view.render(area, &mut buf);
    }

    #[test]
    fn test_render_deploying_no_panic() {
        let state = create_test_state();
        let view = DeployView::new(&state, 3);
        let area = Rect::new(0, 0, 100, 30);
        let mut buf = create_buffer(100, 30);
        view.render(area, &mut buf);
    }

    #[test]
    fn test_render_running_no_panic() {
        let mut state = create_test_state();
        state.phase = DeployPhase::Running;
        let view = DeployView::new(&state, 10);
        let area = Rect::new(0, 0, 100, 30);
        let mut buf = create_buffer(100, 30);
        view.render(area, &mut buf);
    }

    #[test]
    fn test_render_with_logs() {
        let mut state = create_test_state();
        state.apply_event(&DeployEvent::Log {
            level: LogLevel::Info,
            message: "Starting container runtime".to_string(),
        });
        state.apply_event(&DeployEvent::Log {
            level: LogLevel::Warn,
            message: "Overlay not available".to_string(),
        });
        state.apply_event(&DeployEvent::Log {
            level: LogLevel::Error,
            message: "Service failed to start".to_string(),
        });

        let view = DeployView::new(&state, 0);
        let area = Rect::new(0, 0, 80, 30);
        let mut buf = create_buffer(80, 30);
        view.render(area, &mut buf);
    }

    #[test]
    fn test_render_shutting_down_no_panic() {
        let mut state = create_test_state();
        state.phase = DeployPhase::ShuttingDown;
        let view = DeployView::new(&state, 5);
        let area = Rect::new(0, 0, 80, 30);
        let mut buf = create_buffer(80, 30);
        view.render(area, &mut buf);
    }

    #[test]
    fn test_render_complete_no_panic() {
        let mut state = create_test_state();
        state.phase = DeployPhase::Complete;
        let view = DeployView::new(&state, 0);
        let area = Rect::new(0, 0, 80, 30);
        let mut buf = create_buffer(80, 30);
        view.render(area, &mut buf);
    }

    #[test]
    fn test_render_tiny_terminal() {
        let state = create_test_state();
        let view = DeployView::new(&state, 0);
        // Very small terminal - should not panic
        let area = Rect::new(0, 0, 20, 10);
        let mut buf = create_buffer(20, 10);
        view.render(area, &mut buf);
    }

    #[test]
    fn test_render_infra_delegates_to_widget() {
        // Verify that render_infra produces output containing "Infrastructure"
        let state = DeployState::new();
        let view = DeployView::new(&state, 0);
        let area = Rect::new(0, 0, 80, 5);
        let mut buf = create_buffer(80, 5);
        view.render_infra(area, &mut buf);

        let content: String = buf.content().iter().map(|c| c.symbol()).collect();
        assert!(
            content.contains("Infrastructure"),
            "Infra widget should render Infrastructure title"
        );
    }

    #[test]
    fn test_render_services_delegates_to_widget() {
        // Verify that render_services produces output containing "Services"
        let state = create_test_state();
        let view = DeployView::new(&state, 0);
        let area = Rect::new(0, 0, 80, 10);
        let mut buf = create_buffer(80, 10);
        view.render_services(area, &mut buf);

        let content: String = buf.content().iter().map(|c| c.symbol()).collect();
        assert!(
            content.contains("Services"),
            "Service widget should render Services title"
        );
    }

    #[test]
    fn test_render_logs_delegates_to_widget() {
        // Verify that render_logs produces output containing "Logs"
        let state = DeployState::new();
        let view = DeployView::new(&state, 0);
        let area = Rect::new(0, 0, 60, 6);
        let mut buf = create_buffer(60, 6);
        view.render_logs(area, &mut buf);

        let content: String = buf.content().iter().map(|c| c.symbol()).collect();
        assert!(
            content.contains("Logs"),
            "Log widget should render Logs title"
        );
    }
}
