//! Deploy progress view layout
//!
//! This module contains the main deploy view widget that composes
//! the full deployment progress display. It is phase-aware and adapts
//! its layout based on the current deployment lifecycle stage.
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
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table};

use super::state::{DeployPhase, DeployState, PhaseStatus, ServiceDeployPhase};
use super::{LogLevel, ServiceHealth};

/// Spinner characters for cycling animation
const SPINNER: &[char] = &['|', '/', '-', '\\'];

/// Full block character for progress bars (U+2588)
const BLOCK_FULL: char = '\u{2588}';
/// Light shade character for empty progress bar segments (U+2591)
const BLOCK_EMPTY: char = '\u{2591}';

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

    /// Render the infrastructure phases as a grid with status icons
    fn render_infra(&self, area: Rect, buf: &mut Buffer) {
        let block = Block::default()
            .title(" Infrastructure ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray));

        let inner = block.inner(area);
        block.render(area, buf);

        if inner.width < 4 || inner.height < 1 {
            return;
        }

        // Arrange infra phases in a 3-column grid
        let phases = &self.state.infra_phases;
        let col_width = inner.width / 3;

        for (i, (phase, status)) in phases.iter().enumerate() {
            let col = (i % 3) as u16;
            let row = (i / 3) as u16;

            if row >= inner.height {
                break;
            }

            let x = inner.x + col * col_width;
            let y = inner.y + row;

            // Status icon
            let (icon, icon_style) = self.infra_status_icon(status);
            buf.set_string(x, y, icon.to_string(), icon_style);

            // Phase name (truncated to fit column)
            let name = phase.to_string();
            let available = col_width.saturating_sub(3) as usize;
            let display_name = if name.len() > available {
                format!("{}.", &name[..available.saturating_sub(1)])
            } else {
                name
            };
            buf.set_string(x + 2, y, &display_name, Style::default().fg(Color::White));
        }
    }

    /// Get the status icon character and style for an infra phase
    fn infra_status_icon(&self, status: &PhaseStatus) -> (char, Style) {
        match status {
            PhaseStatus::Pending => ('.', Style::default().fg(Color::DarkGray)),
            PhaseStatus::InProgress => {
                let spinner_char = SPINNER[self.frame % SPINNER.len()];
                (spinner_char, Style::default().fg(Color::Cyan))
            }
            PhaseStatus::Complete => ('v', Style::default().fg(Color::Green)),
            PhaseStatus::Failed(_) => ('x', Style::default().fg(Color::Red)),
            PhaseStatus::Skipped(_) => ('-', Style::default().fg(Color::DarkGray)),
        }
    }

    // ---- Services ----

    /// Render the services section as a table
    fn render_services(&self, area: Rect, buf: &mut Buffer) {
        let title = match self.state.phase {
            DeployPhase::Running => " Services (Dashboard) ",
            DeployPhase::ShuttingDown => " Services (Stopping) ",
            _ => " Services ",
        };

        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray));

        let inner = block.inner(area);
        block.render(area, buf);

        if self.state.services.is_empty() {
            Paragraph::new("Waiting for service deployment...")
                .style(
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::ITALIC),
                )
                .render(inner, buf);
            return;
        }

        // Build table rows
        let header = Row::new(vec![
            Cell::from("Status"),
            Cell::from("Name"),
            Cell::from("Replicas"),
            Cell::from("Health"),
            Cell::from("Progress"),
        ])
        .style(
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        );

        let rows: Vec<Row> = self
            .state
            .services
            .iter()
            .map(|svc| {
                let (status_icon, status_style) = self.service_status_display(&svc.phase);
                let replicas = format!("{}/{}", svc.current_replicas, svc.target_replicas);
                let (health_text, health_style) = self.health_display(svc.health);
                let progress = self.progress_bar(svc.current_replicas, svc.target_replicas, 10);

                Row::new(vec![
                    Cell::from(status_icon).style(status_style),
                    Cell::from(svc.name.clone()).style(Style::default().fg(Color::White)),
                    Cell::from(replicas).style(Style::default().fg(Color::White)),
                    Cell::from(health_text).style(health_style),
                    Cell::from(progress).style(Style::default().fg(Color::Cyan)),
                ])
            })
            .collect();

        // Column widths: Status(6) | Name(flex) | Replicas(10) | Health(10) | Progress(12)
        let widths = [
            Constraint::Length(6),
            Constraint::Min(8),
            Constraint::Length(10),
            Constraint::Length(10),
            Constraint::Length(12),
        ];

        let table = Table::new(rows, widths).header(header).column_spacing(1);

        Widget::render(table, inner, buf);
    }

    /// Get display icon and style for a service deploy phase
    fn service_status_display(&self, phase: &ServiceDeployPhase) -> (String, Style) {
        match phase {
            ServiceDeployPhase::Pending => (".".to_string(), Style::default().fg(Color::DarkGray)),
            ServiceDeployPhase::Registering => {
                let c = SPINNER[self.frame % SPINNER.len()];
                (c.to_string(), Style::default().fg(Color::Cyan))
            }
            ServiceDeployPhase::Scaling => {
                let c = SPINNER[self.frame % SPINNER.len()];
                (c.to_string(), Style::default().fg(Color::Yellow))
            }
            ServiceDeployPhase::Running => ("v".to_string(), Style::default().fg(Color::Green)),
            ServiceDeployPhase::Failed(_) => ("x".to_string(), Style::default().fg(Color::Red)),
            ServiceDeployPhase::Stopping => {
                let c = SPINNER[self.frame % SPINNER.len()];
                (c.to_string(), Style::default().fg(Color::Yellow))
            }
            ServiceDeployPhase::Stopped => ("-".to_string(), Style::default().fg(Color::DarkGray)),
        }
    }

    /// Get display text and style for a service health status
    fn health_display(&self, health: ServiceHealth) -> (String, Style) {
        match health {
            ServiceHealth::Healthy => ("Healthy".to_string(), Style::default().fg(Color::Green)),
            ServiceHealth::Degraded => ("Degraded".to_string(), Style::default().fg(Color::Yellow)),
            ServiceHealth::Unhealthy => ("Unhealthy".to_string(), Style::default().fg(Color::Red)),
            ServiceHealth::Unknown => (
                "...".to_string(),
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM),
            ),
        }
    }

    /// Build a unicode progress bar string
    fn progress_bar(&self, current: u32, target: u32, width: u16) -> String {
        if target == 0 {
            return std::iter::repeat_n(BLOCK_EMPTY, width as usize).collect();
        }

        let ratio = (current as f64 / target as f64).clamp(0.0, 1.0);
        let filled = (width as f64 * ratio).round() as u16;
        let empty = width.saturating_sub(filled);

        let bar: String = std::iter::repeat_n(BLOCK_FULL, filled as usize)
            .chain(std::iter::repeat_n(BLOCK_EMPTY, empty as usize))
            .take(width as usize)
            .collect();

        bar
    }

    // ---- Logs ----

    /// Render the scrollable log section
    fn render_logs(&self, area: Rect, buf: &mut Buffer) {
        let block = Block::default()
            .title(" Logs ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray));

        let inner = block.inner(area);
        block.render(area, buf);

        if inner.height == 0 {
            return;
        }

        let entries = &self.state.log_entries;

        if entries.is_empty() {
            Paragraph::new("No log output yet...")
                .style(
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::ITALIC),
                )
                .render(inner, buf);
            return;
        }

        let visible_count = inner.height as usize;
        let total = entries.len();

        // Calculate visible range from scroll offset
        // scroll_to_bottom sets offset to entries.len(), so we clamp
        let scroll = self
            .state
            .log_scroll_offset
            .min(total.saturating_sub(visible_count));
        let end = (scroll + visible_count).min(total);

        for (display_idx, idx) in (scroll..end).enumerate() {
            if display_idx >= inner.height as usize {
                break;
            }

            let entry = &entries[idx];
            let y = inner.y + display_idx as u16;

            // Color based on log level
            let style = match entry.level {
                LogLevel::Info => Style::default().fg(Color::White),
                LogLevel::Warn => Style::default().fg(Color::Yellow),
                LogLevel::Error => Style::default().fg(Color::Red),
            };

            // Prefix with level indicator
            let prefix = match entry.level {
                LogLevel::Info => "[INFO] ",
                LogLevel::Warn => "[WARN] ",
                LogLevel::Error => "[ERR]  ",
            };

            let prefix_len = prefix.len() as u16;
            buf.set_string(inner.x, y, prefix, style);

            // Message text (truncated to fit)
            let available = inner.width.saturating_sub(prefix_len) as usize;
            let text = if entry.message.len() > available {
                format!("{}...", &entry.message[..available.saturating_sub(3)])
            } else {
                entry.message.clone()
            };
            buf.set_string(inner.x + prefix_len, y, &text, style);
        }

        // Scroll position indicator if scrollable
        if total > visible_count {
            let percent = if total == 0 {
                100
            } else {
                ((end as f64 / total as f64) * 100.0) as usize
            };
            let indicator = format!(" {}% ", percent);
            let x = inner.x + inner.width.saturating_sub(indicator.len() as u16 + 1);
            let y = inner.y + inner.height.saturating_sub(1);

            if x >= inner.x && y >= inner.y {
                buf.set_string(
                    x,
                    y,
                    &indicator,
                    Style::default().fg(Color::Black).bg(Color::DarkGray),
                );
            }
        }
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
    fn test_infra_status_icons() {
        let state = DeployState::new();
        let view = DeployView::new(&state, 0);

        let (c, _) = view.infra_status_icon(&PhaseStatus::Pending);
        assert_eq!(c, '.');

        let (c, _) = view.infra_status_icon(&PhaseStatus::InProgress);
        assert_eq!(c, '|'); // frame 0 -> first spinner char

        let (c, _) = view.infra_status_icon(&PhaseStatus::Complete);
        assert_eq!(c, 'v');

        let (c, _) = view.infra_status_icon(&PhaseStatus::Failed("err".to_string()));
        assert_eq!(c, 'x');

        let (c, _) = view.infra_status_icon(&PhaseStatus::Skipped("skip".to_string()));
        assert_eq!(c, '-');
    }

    #[test]
    fn test_spinner_cycles() {
        let state = DeployState::new();

        for frame in 0..8 {
            let view = DeployView::new(&state, frame);
            let (c, _) = view.infra_status_icon(&PhaseStatus::InProgress);
            assert_eq!(c, SPINNER[frame % SPINNER.len()]);
        }
    }

    #[test]
    fn test_health_display() {
        let state = DeployState::new();
        let view = DeployView::new(&state, 0);

        let (text, style) = view.health_display(ServiceHealth::Healthy);
        assert_eq!(text, "Healthy");
        assert_eq!(style.fg, Some(Color::Green));

        let (text, style) = view.health_display(ServiceHealth::Degraded);
        assert_eq!(text, "Degraded");
        assert_eq!(style.fg, Some(Color::Yellow));

        let (text, style) = view.health_display(ServiceHealth::Unhealthy);
        assert_eq!(text, "Unhealthy");
        assert_eq!(style.fg, Some(Color::Red));

        let (text, _) = view.health_display(ServiceHealth::Unknown);
        assert_eq!(text, "...");
    }

    #[test]
    fn test_progress_bar() {
        let state = DeployState::new();
        let view = DeployView::new(&state, 0);

        // Full progress
        let bar = view.progress_bar(3, 3, 10);
        assert_eq!(bar.chars().count(), 10);
        assert!(bar.chars().all(|c| c == BLOCK_FULL));

        // Zero progress
        let bar = view.progress_bar(0, 3, 10);
        assert_eq!(bar.chars().count(), 10);
        assert!(bar.chars().all(|c| c == BLOCK_EMPTY));

        // Zero target
        let bar = view.progress_bar(0, 0, 10);
        assert_eq!(bar.chars().count(), 10);
        assert!(bar.chars().all(|c| c == BLOCK_EMPTY));

        // Half progress
        let bar = view.progress_bar(5, 10, 10);
        assert_eq!(bar.chars().count(), 10);
        let filled: usize = bar.chars().filter(|&c| c == BLOCK_FULL).count();
        assert_eq!(filled, 5);
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
    fn test_service_status_display_all_phases() {
        let state = DeployState::new();
        let view = DeployView::new(&state, 0);

        let phases = [
            ServiceDeployPhase::Pending,
            ServiceDeployPhase::Registering,
            ServiceDeployPhase::Scaling,
            ServiceDeployPhase::Running,
            ServiceDeployPhase::Failed("err".to_string()),
            ServiceDeployPhase::Stopping,
            ServiceDeployPhase::Stopped,
        ];

        for phase in &phases {
            let (icon, _style) = view.service_status_display(phase);
            assert!(!icon.is_empty(), "Phase {:?} should have an icon", phase);
        }
    }
}
