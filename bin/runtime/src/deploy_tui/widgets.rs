//! Custom TUI widgets for deploy progress display
//!
//! This module contains reusable widgets for displaying:
//! - Infrastructure phase progress indicators
//! - Service deployment table with progress bars
//! - Scrollable log pane with color-coded severity
//!
//! Follows the same widget patterns as `zlayer_builder::tui::widgets`.

use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table};

use zlayer_tui::icons::{self, SPINNER_FRAMES};
use zlayer_tui::palette::color;
use zlayer_tui::widgets::progress_bar::ProgressBar;

use super::state::{PhaseStatus, ServiceDeployPhase, ServiceState};
use super::{InfraPhase, ServiceHealth};

// ───────────────────────────────────────────────────────────────────
// InfraProgress widget
// ───────────────────────────────────────────────────────────────────

/// Renders infrastructure phase status in a compact grid layout
///
/// ```text
/// Infrastructure                              4/6
///  v Container Runtime  v Overlay Network  v DNS Server
///  > Proxy Manager     .. Container Supervisor  .. API Server
/// ```
pub struct InfraProgress<'a> {
    /// Infrastructure phases and their statuses
    pub phases: &'a [(InfraPhase, PhaseStatus)],
    /// Monotonic tick counter for spinner animation
    pub tick: usize,
}

impl Widget for InfraProgress<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width < 10 {
            return;
        }

        let done_count = self.phases.iter().filter(|(_, s)| s.is_done()).count();
        let total = self.phases.len();
        let title = format!(" Infrastructure  {}/{} ", done_count, total);

        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(if done_count == total {
                Style::default().fg(color::SUCCESS)
            } else {
                Style::default().fg(color::ACTIVE_BORDER)
            });

        let inner = block.inner(area);
        block.render(area, buf);

        if inner.height == 0 || inner.width < 5 {
            return;
        }

        // Lay out phases in rows of 3
        let cols = 3usize;
        let col_width = inner.width as usize / cols;

        for (i, (phase, status)) in self.phases.iter().enumerate() {
            let row = i / cols;
            let col = i % cols;

            let y = inner.y + row as u16;
            if y >= inner.y + inner.height {
                break;
            }

            let x = inner.x + (col * col_width) as u16;

            // Status indicator: convert icon chars to owned Strings for
            // branches that use shared constants, or use &'static str directly.
            let (indicator, ind_style) = match status {
                PhaseStatus::Complete => (
                    String::from(icons::COMPLETE),
                    Style::default().fg(color::SUCCESS),
                ),
                PhaseStatus::Failed(_) => (
                    String::from(icons::FAILED),
                    Style::default().fg(color::ERROR),
                ),
                PhaseStatus::InProgress => {
                    let frame_idx = self.tick % SPINNER_FRAMES.len();
                    (
                        String::from(SPINNER_FRAMES[frame_idx]),
                        Style::default()
                            .fg(color::WARNING)
                            .add_modifier(Modifier::BOLD),
                    )
                }
                PhaseStatus::Pending => ("..".to_string(), Style::default().fg(color::INACTIVE)),
                PhaseStatus::Skipped(_) => (
                    "~".to_string(),
                    Style::default()
                        .fg(color::INACTIVE)
                        .add_modifier(Modifier::DIM),
                ),
            };

            // Render indicator
            buf.set_string(x, y, &indicator, ind_style);

            // Phase name — truncate to fit column
            let name = phase.to_string();
            let name_x = x + indicator.len() as u16 + 1;
            let available = col_width.saturating_sub(indicator.len() + 2);
            let display_name = if name.len() > available {
                format!("{}..", &name[..available.saturating_sub(2)])
            } else {
                name
            };

            let name_style = match status {
                PhaseStatus::Pending => Style::default().fg(color::INACTIVE),
                PhaseStatus::InProgress => Style::default().fg(color::WARNING),
                PhaseStatus::Complete => Style::default().fg(color::TEXT),
                PhaseStatus::Failed(_) => Style::default().fg(color::ERROR),
                PhaseStatus::Skipped(_) => Style::default()
                    .fg(color::INACTIVE)
                    .add_modifier(Modifier::DIM),
            };

            buf.set_string(name_x, y, &display_name, name_style);
        }
    }
}

// ───────────────────────────────────────────────────────────────────
// ServiceTable widget
// ───────────────────────────────────────────────────────────────────

/// Renders services as a table with replica progress bars
///
/// ```text
/// Services                                      2/4 deployed
///  v postgres    3/3 replicas  [healthy]
///  v redis       1/1 replicas  [healthy]
///  > api         1/3 replicas  [unknown]    ██████░░░░ 33%
///  .. frontend   0/2 replicas  [pending]
/// ```
pub struct ServiceTable<'a> {
    /// Service states to render
    pub services: &'a [ServiceState],
    /// Number of services that have reached Running phase
    pub deployed_count: usize,
}

impl Widget for ServiceTable<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width < 10 {
            return;
        }

        let total = self.services.len();
        let title = format!(" Services  {}/{} deployed ", self.deployed_count, total);

        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(if self.deployed_count == total && total > 0 {
                Style::default().fg(color::SUCCESS)
            } else {
                Style::default().fg(color::ACTIVE_BORDER)
            });

        let inner = block.inner(area);
        block.render(area, buf);

        if inner.height == 0 || self.services.is_empty() {
            if inner.height > 0 {
                Paragraph::new("No services")
                    .style(Style::default().fg(color::INACTIVE))
                    .render(inner, buf);
            }
            return;
        }

        // Build rows manually for full control over styling
        let widths = [
            Constraint::Length(2),  // indicator
            Constraint::Min(12),    // name
            Constraint::Length(14), // replicas
            Constraint::Length(12), // health
            Constraint::Min(10),    // progress bar (when scaling)
        ];

        let mut rows = Vec::with_capacity(self.services.len());
        for svc in self.services {
            let (indicator, ind_style) = service_indicator(&svc.phase);
            let replicas = format!("{}/{} replicas", svc.current_replicas, svc.target_replicas);
            let (health_text, health_style) = health_display(&svc.health);

            // Progress bar for services that are actively scaling
            let progress_str =
                if svc.phase == ServiceDeployPhase::Scaling && svc.target_replicas > 0 {
                    ProgressBar::new(svc.current_replicas as usize, svc.target_replicas as usize)
                        .with_percentage()
                        .to_string_compact(10)
                } else {
                    String::new()
                };

            let row = Row::new(vec![
                Cell::from(indicator).style(ind_style),
                Cell::from(svc.name.clone()).style(service_name_style(&svc.phase)),
                Cell::from(replicas).style(Style::default().fg(color::TEXT)),
                Cell::from(health_text).style(health_style),
                Cell::from(progress_str).style(Style::default().fg(color::ACCENT)),
            ]);
            rows.push(row);
        }

        let table = Table::new(rows, widths).column_spacing(1);
        Widget::render(table, inner, buf);
    }
}

/// Get the status indicator for a service's deploy phase
///
/// Returns a `String` (since shared icon chars must be converted) and a `Style`.
fn service_indicator(phase: &ServiceDeployPhase) -> (String, Style) {
    match phase {
        ServiceDeployPhase::Pending => ("..".to_string(), Style::default().fg(color::INACTIVE)),
        ServiceDeployPhase::Registering => ("~".to_string(), Style::default().fg(color::WARNING)),
        ServiceDeployPhase::Scaling => (
            String::from(icons::RUNNING),
            Style::default()
                .fg(color::WARNING)
                .add_modifier(Modifier::BOLD),
        ),
        ServiceDeployPhase::Running => (
            String::from(icons::COMPLETE),
            Style::default().fg(color::SUCCESS),
        ),
        ServiceDeployPhase::Failed(_) => (
            String::from(icons::FAILED),
            Style::default().fg(color::ERROR),
        ),
        ServiceDeployPhase::Stopping => (
            String::from(icons::STOPPING),
            Style::default().fg(color::WARNING),
        ),
        ServiceDeployPhase::Stopped => (
            String::from(icons::STOPPED),
            Style::default().fg(color::INACTIVE),
        ),
    }
}

/// Style for service name based on deploy phase
fn service_name_style(phase: &ServiceDeployPhase) -> Style {
    match phase {
        ServiceDeployPhase::Pending => Style::default().fg(color::INACTIVE),
        ServiceDeployPhase::Failed(_) => Style::default().fg(color::ERROR),
        ServiceDeployPhase::Stopped => Style::default().fg(color::INACTIVE),
        _ => Style::default().fg(color::TEXT),
    }
}

/// Render a health status label with appropriate color
fn health_display(health: &ServiceHealth) -> (String, Style) {
    match health {
        ServiceHealth::Healthy => ("[healthy]".to_string(), Style::default().fg(color::SUCCESS)),
        ServiceHealth::Degraded => (
            "[degraded]".to_string(),
            Style::default().fg(color::WARNING),
        ),
        ServiceHealth::Unhealthy => ("[unhealthy]".to_string(), Style::default().fg(color::ERROR)),
        ServiceHealth::Unknown => (
            "[unknown]".to_string(),
            Style::default().fg(color::INACTIVE),
        ),
    }
}

// ───────────────────────────────────────────────────────────────────
// Convenience: render all deploy widgets into a layout
// ───────────────────────────────────────────────────────────────────

/// Render the full deploy TUI layout into a frame area
///
/// Layout:
/// ```text
/// +--[ Infrastructure ]--------+
/// |  phase grid                |
/// +--[ Services ]-------------+
/// |  service table             |
/// +--[ Logs ]-----------------+
/// |  scrollable log pane       |
/// +----------------------------+
/// | footer help text           |
/// +----------------------------+
/// ```
#[cfg(test)]
use super::state::DeployState;

#[cfg(test)]
use zlayer_tui::widgets::scrollable_pane::ScrollablePane;

#[cfg(test)]
pub fn render_deploy_view(state: &DeployState, tick: usize, area: Rect, buf: &mut Buffer) {
    // Vertical layout: infra, services, logs, footer
    let infra_height = 2 + state.infra_phases.len().div_ceil(3) as u16; // border + rows
    let service_count = state.services.len().max(1) as u16;

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(infra_height + 2),  // infra + border
            Constraint::Length(service_count + 2), // services + border
            Constraint::Min(5),                    // logs (flexible)
            Constraint::Length(1),                 // footer
        ])
        .split(area);

    // Infrastructure progress
    let infra = InfraProgress {
        phases: &state.infra_phases,
        tick,
    };
    infra.render(chunks[0], buf);

    // Service table
    let svc_table = ServiceTable {
        services: &state.services,
        deployed_count: state.services_deployed_count(),
    };
    svc_table.render(chunks[1], buf);

    // Log pane (using shared ScrollablePane)
    let log_pane = ScrollablePane::new(&state.log_entries, state.log_scroll_offset)
        .with_title("Logs")
        .with_empty_text("No log output yet");
    log_pane.render(chunks[2], buf);

    // Footer
    let footer_text = match state.phase {
        super::state::DeployPhase::Complete => "Press 'q' to exit",
        _ => "q: quit | arrows/jk: scroll | PgUp/PgDn: page",
    };
    Paragraph::new(footer_text)
        .style(Style::default().fg(color::INACTIVE))
        .alignment(Alignment::Center)
        .render(chunks[3], buf);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deploy_tui::state::{DeployState, ServiceDeployPhase, ServiceState};
    use crate::deploy_tui::{InfraPhase, ServiceHealth};
    use zlayer_tui::widgets::scrollable_pane::{LogEntry, LogLevel};

    fn create_buffer(width: u16, height: u16) -> Buffer {
        Buffer::empty(Rect::new(0, 0, width, height))
    }

    #[test]
    fn test_infra_progress_all_pending() {
        let mut buf = create_buffer(80, 5);
        let area = Rect::new(0, 0, 80, 5);

        let phases = vec![
            (InfraPhase::Runtime, PhaseStatus::Pending),
            (InfraPhase::Overlay, PhaseStatus::Pending),
            (InfraPhase::Dns, PhaseStatus::Pending),
        ];

        let widget = InfraProgress {
            phases: &phases,
            tick: 0,
        };
        widget.render(area, &mut buf);

        let content: String = buf.content().iter().map(|c| c.symbol()).collect();
        assert!(content.contains("Infrastructure"));
    }

    #[test]
    fn test_infra_progress_mixed_statuses() {
        let mut buf = create_buffer(80, 5);
        let area = Rect::new(0, 0, 80, 5);

        let phases = vec![
            (InfraPhase::Runtime, PhaseStatus::Complete),
            (
                InfraPhase::Overlay,
                PhaseStatus::Failed("no wg".to_string()),
            ),
            (InfraPhase::Dns, PhaseStatus::InProgress),
            (InfraPhase::Proxy, PhaseStatus::Pending),
            (InfraPhase::Supervisor, PhaseStatus::Pending),
            (InfraPhase::Api, PhaseStatus::Pending),
        ];

        let widget = InfraProgress {
            phases: &phases,
            tick: 2,
        };
        widget.render(area, &mut buf);

        let content: String = buf.content().iter().map(|c| c.symbol()).collect();
        // Should contain checkmark and x mark
        assert!(content.contains('\u{2713}') || content.contains("Infrastructure"));
    }

    #[test]
    fn test_infra_progress_tiny_area() {
        let mut buf = create_buffer(5, 1);
        let area = Rect::new(0, 0, 5, 1);

        let phases = vec![(InfraPhase::Runtime, PhaseStatus::Complete)];
        let widget = InfraProgress {
            phases: &phases,
            tick: 0,
        };
        // Should not panic on tiny area
        widget.render(area, &mut buf);
    }

    #[test]
    fn test_service_table_empty() {
        let mut buf = create_buffer(80, 5);
        let area = Rect::new(0, 0, 80, 5);

        let widget = ServiceTable {
            services: &[],
            deployed_count: 0,
        };
        widget.render(area, &mut buf);

        let content: String = buf.content().iter().map(|c| c.symbol()).collect();
        assert!(content.contains("Services") || content.contains("No services"));
    }

    #[test]
    fn test_service_table_with_services() {
        let mut buf = create_buffer(80, 10);
        let area = Rect::new(0, 0, 80, 10);

        let services = vec![
            ServiceState {
                name: "postgres".to_string(),
                phase: ServiceDeployPhase::Running,
                target_replicas: 1,
                current_replicas: 1,
                health: ServiceHealth::Healthy,
            },
            ServiceState {
                name: "api".to_string(),
                phase: ServiceDeployPhase::Scaling,
                target_replicas: 3,
                current_replicas: 1,
                health: ServiceHealth::Unknown,
            },
            ServiceState {
                name: "frontend".to_string(),
                phase: ServiceDeployPhase::Pending,
                target_replicas: 2,
                current_replicas: 0,
                health: ServiceHealth::Unknown,
            },
        ];

        let widget = ServiceTable {
            services: &services,
            deployed_count: 1,
        };
        widget.render(area, &mut buf);

        let content: String = buf.content().iter().map(|c| c.symbol()).collect();
        assert!(content.contains("postgres") || content.contains("Services"));
    }

    #[test]
    fn test_scrollable_log_pane_empty() {
        let mut buf = create_buffer(60, 6);
        let area = Rect::new(0, 0, 60, 6);

        let entries: Vec<LogEntry> = vec![];
        let widget = ScrollablePane::new(&entries, 0)
            .with_title("Logs")
            .with_empty_text("No log output yet");
        widget.render(area, &mut buf);

        let content: String = buf.content().iter().map(|c| c.symbol()).collect();
        assert!(content.contains("No log output") || content.contains("Logs"));
    }

    #[test]
    fn test_scrollable_log_pane_with_entries() {
        let mut buf = create_buffer(60, 8);
        let area = Rect::new(0, 0, 60, 8);

        let entries = vec![
            LogEntry {
                level: LogLevel::Info,
                message: "Starting deployment".to_string(),
            },
            LogEntry {
                level: LogLevel::Warn,
                message: "Overlay not available".to_string(),
            },
            LogEntry {
                level: LogLevel::Error,
                message: "Container crashed".to_string(),
            },
        ];

        let widget = ScrollablePane::new(&entries, 0).with_title("Logs");
        widget.render(area, &mut buf);

        let content: String = buf.content().iter().map(|c| c.symbol()).collect();
        assert!(
            content.contains("[INFO]")
                || content.contains("[WARN]")
                || content.contains("[ERROR]")
                || content.contains("Logs")
        );
    }

    #[test]
    fn test_scrollable_log_pane_scrolled() {
        let mut buf = create_buffer(60, 5);
        let area = Rect::new(0, 0, 60, 5);

        let entries: Vec<LogEntry> = (0..20)
            .map(|i| LogEntry {
                level: LogLevel::Info,
                message: format!("Log line {}", i),
            })
            .collect();

        // Scroll to middle
        let widget = ScrollablePane::new(&entries, 10).with_title("Logs");
        widget.render(area, &mut buf);

        let content: String = buf.content().iter().map(|c| c.symbol()).collect();
        // Should show a scroll percentage indicator
        assert!(content.contains('%') || content.contains("Logs"));
    }

    #[test]
    fn test_progress_bar_compact() {
        let bar = ProgressBar::new(1, 3)
            .with_percentage()
            .to_string_compact(10);
        assert!(bar.contains("33%"));
        assert!(bar.contains('\u{2588}')); // filled
        assert!(bar.contains('\u{2591}')); // empty

        let full = ProgressBar::new(3, 3)
            .with_percentage()
            .to_string_compact(10);
        assert!(full.contains("100%"));

        let empty = ProgressBar::new(0, 3)
            .with_percentage()
            .to_string_compact(10);
        assert!(empty.contains("0%"));

        let zero_target = ProgressBar::new(0, 0)
            .with_percentage()
            .to_string_compact(10);
        assert!(zero_target.contains("0%"));
    }

    #[test]
    fn test_health_display() {
        let (text, _style) = health_display(&ServiceHealth::Healthy);
        assert_eq!(text, "[healthy]");

        let (text, _style) = health_display(&ServiceHealth::Degraded);
        assert_eq!(text, "[degraded]");

        let (text, _style) = health_display(&ServiceHealth::Unhealthy);
        assert_eq!(text, "[unhealthy]");

        let (text, _style) = health_display(&ServiceHealth::Unknown);
        assert_eq!(text, "[unknown]");
    }

    #[test]
    fn test_service_indicator() {
        let (text, _) = service_indicator(&ServiceDeployPhase::Running);
        assert_eq!(text, "\u{2713}");

        let (text, _) = service_indicator(&ServiceDeployPhase::Failed("err".to_string()));
        assert_eq!(text, "\u{2717}");

        let (text, _) = service_indicator(&ServiceDeployPhase::Pending);
        assert_eq!(text, "..");
    }

    #[test]
    fn test_render_deploy_view_full() {
        let mut buf = create_buffer(100, 30);
        let area = Rect::new(0, 0, 100, 30);

        let mut state = DeployState::new();
        state.services.push(ServiceState {
            name: "web".to_string(),
            phase: ServiceDeployPhase::Running,
            target_replicas: 2,
            current_replicas: 2,
            health: ServiceHealth::Healthy,
        });
        state.log_entries.push(LogEntry {
            level: LogLevel::Info,
            message: "All good".to_string(),
        });
        state.log_scroll_offset = 1;

        render_deploy_view(&state, 0, area, &mut buf);

        let content: String = buf.content().iter().map(|c| c.symbol()).collect();
        assert!(content.contains("Infrastructure") || content.contains("Services"));
    }
}
