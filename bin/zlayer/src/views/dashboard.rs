//! Dashboard view
//!
//! Displays running deployments, services, and daemon status.
//! Auto-refreshes every 2 seconds. Supports drill-down into
//! individual deployments to see per-service details.

use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph, Row, Table, Wrap};

use crate::app::DashboardState;

/// Render the dashboard view
pub fn render(frame: &mut Frame, state: &DashboardState) {
    let area = frame.area();

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Blue))
        .title(" ZLayer Dashboard ")
        .title_alignment(Alignment::Center);

    let inner = outer.inner(area);
    frame.render_widget(outer, area);

    // Top-level layout: header, content, footer
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2), // Daemon status header
            Constraint::Length(1), // Separator
            Constraint::Min(5),    // Content (table or detail)
            Constraint::Length(1), // Footer
        ])
        .split(inner);

    // Header: daemon status
    render_daemon_status(chunks[0], frame.buffer_mut(), state);

    // Content: deployment list or detail
    if let Some(ref detail) = state.detail_view {
        render_deployment_detail(chunks[2], frame, detail, state);
    } else {
        render_deployment_table(chunks[2], frame, state);
    }

    // Footer: keybindings
    let footer_text = if state.detail_view.is_some() {
        "Esc: back to list | r: refresh | q: main menu"
    } else {
        "Enter: drill in | r: refresh | s: start daemon | q: back"
    };

    let footer = Paragraph::new(footer_text)
        .style(Style::default().fg(Color::DarkGray))
        .alignment(Alignment::Center);
    frame.render_widget(footer, chunks[3]);
}

/// Render the daemon status line
fn render_daemon_status(area: Rect, buf: &mut Buffer, state: &DashboardState) {
    let (status_text, status_color) = match &state.daemon_status {
        DaemonStatus::Connected => ("Connected", Color::Green),
        DaemonStatus::Connecting => ("Connecting...", Color::Yellow),
        DaemonStatus::Offline => ("Offline", Color::Red),
        DaemonStatus::Error(msg) => {
            // Render the error on the second line if there's room
            if area.height > 1 {
                let err_style = Style::default().fg(Color::Red).add_modifier(Modifier::DIM);
                let truncated = if msg.len() > area.width as usize - 4 {
                    &msg[..area.width as usize - 7]
                } else {
                    msg
                };
                buf.set_string(area.x + 2, area.y + 1, truncated, err_style);
            }
            ("Error", Color::Red)
        }
    };

    let icon = if matches!(state.daemon_status, DaemonStatus::Connected) {
        zlayer_tui::icons::COMPLETE
    } else if matches!(state.daemon_status, DaemonStatus::Connecting) {
        zlayer_tui::icons::RUNNING
    } else {
        zlayer_tui::icons::FAILED
    };

    buf.set_string(
        area.x + 2,
        area.y,
        format!("{} Daemon: {}", icon, status_text),
        Style::default()
            .fg(status_color)
            .add_modifier(Modifier::BOLD),
    );
}

/// Render the deployment table
fn render_deployment_table(area: Rect, frame: &mut Frame, state: &DashboardState) {
    if !matches!(state.daemon_status, DaemonStatus::Connected) {
        // Show centered message
        let msg = match state.daemon_status {
            DaemonStatus::Offline => "Daemon not running. Press 's' to start.",
            DaemonStatus::Connecting => "Connecting to daemon...",
            DaemonStatus::Error(_) => "Failed to connect. Press 's' to retry.",
            DaemonStatus::Connected => unreachable!(),
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(" Deployments ")
            .title_alignment(Alignment::Center);

        let inner = block.inner(area);
        frame.render_widget(block, area);

        let paragraph = Paragraph::new(msg)
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: false });
        frame.render_widget(paragraph, inner);
        return;
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(format!(" Deployments ({}) ", state.deployments.len()))
        .title_alignment(Alignment::Center);

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if state.deployments.is_empty() {
        let msg = Paragraph::new("No active deployments. Use Deploy to get started.")
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center);
        frame.render_widget(msg, inner);
        return;
    }

    // Header row
    let header = Row::new(vec!["Name", "Status", "Services", "Replicas"])
        .style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .bottom_margin(1);

    // Data rows
    let rows: Vec<Row> = state
        .deployments
        .iter()
        .enumerate()
        .map(|(i, dep)| {
            let style = if i == state.selected {
                Style::default().fg(Color::White).bg(Color::Blue)
            } else {
                Style::default().fg(Color::White)
            };

            Row::new(vec![
                dep.name.clone(),
                dep.status.clone(),
                dep.services_count.to_string(),
                dep.total_replicas.to_string(),
            ])
            .style(style)
        })
        .collect();

    let widths = [
        Constraint::Min(20),
        Constraint::Length(12),
        Constraint::Length(10),
        Constraint::Length(10),
    ];

    let table = Table::new(rows, widths).header(header);
    Widget::render(table, inner, frame.buffer_mut());
}

/// Render per-service details for a deployment
fn render_deployment_detail(
    area: Rect,
    frame: &mut Frame,
    detail: &DeploymentDetail,
    state: &DashboardState,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(format!(" {} - Services ", detail.deployment_name))
        .title_alignment(Alignment::Center);

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if detail.services.is_empty() {
        let msg = Paragraph::new("No services found in this deployment.")
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center);
        frame.render_widget(msg, inner);
        return;
    }

    // Header row
    let header = Row::new(vec!["Service", "Status", "Replicas", "Health", "Type"])
        .style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .bottom_margin(1);

    // Data rows
    let rows: Vec<Row> = detail
        .services
        .iter()
        .enumerate()
        .map(|(i, svc)| {
            let style = if i == state.detail_selected {
                Style::default().fg(Color::White).bg(Color::Blue)
            } else {
                Style::default().fg(Color::White)
            };

            Row::new(vec![
                svc.name.clone(),
                svc.status.clone(),
                svc.replicas.clone(),
                svc.health.clone(),
                svc.service_type.clone(),
            ])
            .style(style)
        })
        .collect();

    let widths = [
        Constraint::Min(16),
        Constraint::Length(12),
        Constraint::Length(10),
        Constraint::Length(10),
        Constraint::Length(10),
    ];

    let table = Table::new(rows, widths).header(header);
    Widget::render(table, inner, frame.buffer_mut());
}

// ---------------------------------------------------------------------------
// State types
// ---------------------------------------------------------------------------

/// Daemon connection status
#[derive(Debug, Clone)]
pub enum DaemonStatus {
    /// Successfully connected and healthy
    Connected,
    /// Attempting to connect
    Connecting,
    /// Daemon is offline / socket not found
    Offline,
    /// Connection error with message
    Error(String),
}

/// Summary of a deployment (extracted from daemon JSON)
#[derive(Debug, Clone)]
pub struct DeploymentInfo {
    pub name: String,
    pub status: String,
    pub services_count: usize,
    pub total_replicas: u32,
}

/// Detail view for a single deployment
#[derive(Debug, Clone)]
pub struct DeploymentDetail {
    pub deployment_name: String,
    pub services: Vec<ServiceInfo>,
}

/// Summary of a service within a deployment
#[derive(Debug, Clone)]
pub struct ServiceInfo {
    pub name: String,
    pub status: String,
    pub replicas: String,
    pub health: String,
    pub service_type: String,
}

// ---------------------------------------------------------------------------
// Data fetching (blocking, uses tokio runtime)
// ---------------------------------------------------------------------------

/// Attempt to refresh dashboard data from the daemon.
/// Call this from the event loop with the tokio runtime.
pub fn refresh_data(rt: &tokio::runtime::Runtime, state: &mut DashboardState) {
    state.last_refresh = Some(Instant::now());
    state.daemon_status = DaemonStatus::Connecting;

    // Try to connect and fetch data with a timeout
    let result = rt.block_on(async {
        let client = match tokio::time::timeout(
            std::time::Duration::from_secs(2),
            crate::daemon_client::DaemonClient::connect(),
        )
        .await
        {
            Ok(Ok(c)) => c,
            Ok(Err(e)) => return Err(format!("{:#}", e)),
            Err(_) => return Err("Connection timed out".to_string()),
        };

        // Check health
        match client.health_check().await {
            Ok(true) => {}
            Ok(false) => return Err("Daemon unhealthy".to_string()),
            Err(e) => return Err(format!("Health check failed: {:#}", e)),
        }

        // List deployments
        let deployments = match client.list_deployments().await {
            Ok(d) => d,
            Err(e) => return Err(format!("Failed to list deployments: {:#}", e)),
        };

        let mut infos = Vec::new();
        for dep in &deployments {
            let name = dep
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let status = dep
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let services_count = dep
                .get("services")
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .or_else(|| {
                    dep.get("service_count")
                        .and_then(|v| v.as_u64())
                        .map(|n| n as usize)
                })
                .unwrap_or(0);
            let total_replicas = dep
                .get("total_replicas")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;

            infos.push(DeploymentInfo {
                name,
                status,
                services_count,
                total_replicas,
            });
        }

        Ok(infos)
    });

    match result {
        Ok(deployments) => {
            state.daemon_status = DaemonStatus::Connected;
            state.deployments = deployments;
        }
        Err(msg) => {
            // Check if it's a connection issue vs. other error
            if msg.contains("timed out")
                || msg.contains("Connection refused")
                || msg.contains("No such file")
                || msg.contains("not running")
            {
                state.daemon_status = DaemonStatus::Offline;
            } else {
                state.daemon_status = DaemonStatus::Error(msg);
            }
            state.deployments.clear();
        }
    }
}

/// Refresh the detail view for a specific deployment
pub fn refresh_detail(
    rt: &tokio::runtime::Runtime,
    state: &mut DashboardState,
    deployment_name: &str,
) {
    let name = deployment_name.to_string();
    let result = rt.block_on(async {
        let client = match tokio::time::timeout(
            std::time::Duration::from_secs(2),
            crate::daemon_client::DaemonClient::connect(),
        )
        .await
        {
            Ok(Ok(c)) => c,
            Ok(Err(e)) => return Err(format!("{:#}", e)),
            Err(_) => return Err("Connection timed out".to_string()),
        };

        let services = match client.list_services(&name).await {
            Ok(s) => s,
            Err(e) => return Err(format!("Failed to list services: {:#}", e)),
        };

        let mut infos = Vec::new();
        for svc in &services {
            let svc_name = svc
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let status = svc
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let replicas = svc
                .get("replicas")
                .map(|v| v.to_string())
                .unwrap_or_else(|| "?".to_string());
            let health = svc
                .get("health")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let service_type = svc
                .get("rtype")
                .or_else(|| svc.get("type"))
                .and_then(|v| v.as_str())
                .unwrap_or("service")
                .to_string();

            infos.push(ServiceInfo {
                name: svc_name,
                status,
                replicas,
                health,
                service_type,
            });
        }

        Ok(infos)
    });

    match result {
        Ok(services) => {
            state.detail_view = Some(DeploymentDetail {
                deployment_name: name,
                services,
            });
            state.detail_selected = 0;
        }
        Err(msg) => {
            state.daemon_status = DaemonStatus::Error(msg);
        }
    }
}

/// Attempt to start the daemon
pub fn start_daemon(rt: &tokio::runtime::Runtime) {
    // Use DaemonClient::connect which auto-starts the daemon
    rt.block_on(async {
        let _ = crate::daemon_client::DaemonClient::connect().await;
    });
}

// ---------------------------------------------------------------------------
// Input handling
// ---------------------------------------------------------------------------

/// Handle key events for the dashboard view. Returns Some(true) to go back
/// to main menu, None to stay in dashboard.
pub fn handle_key(key: KeyEvent, state: &mut DashboardState, rt: &tokio::runtime::Runtime) -> bool {
    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => {
            if state.detail_view.is_some() {
                // Go back to deployment list
                state.detail_view = None;
                state.detail_selected = 0;
                false
            } else {
                // Go back to main menu
                true
            }
        }
        KeyCode::Char('r') => {
            if let Some(ref detail) = state.detail_view.clone() {
                refresh_detail(rt, state, &detail.deployment_name);
            } else {
                refresh_data(rt, state);
            }
            false
        }
        KeyCode::Char('s') => {
            start_daemon(rt);
            refresh_data(rt, state);
            false
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if state.detail_view.is_some() {
                state.detail_selected = state.detail_selected.saturating_sub(1);
            } else {
                state.selected = state.selected.saturating_sub(1);
            }
            false
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if let Some(ref detail) = state.detail_view {
                if !detail.services.is_empty() && state.detail_selected < detail.services.len() - 1
                {
                    state.detail_selected += 1;
                }
            } else if !state.deployments.is_empty() && state.selected < state.deployments.len() - 1
            {
                state.selected += 1;
            }
            false
        }
        KeyCode::Enter => {
            if state.detail_view.is_none() {
                // Drill into selected deployment
                if let Some(dep) = state.deployments.get(state.selected) {
                    let name = dep.name.clone();
                    refresh_detail(rt, state, &name);
                }
            }
            false
        }
        _ => false,
    }
}
