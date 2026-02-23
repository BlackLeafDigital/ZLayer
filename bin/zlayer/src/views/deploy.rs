//! Deploy view
//!
//! Multi-step flow for deploying a spec file to the daemon:
//!   1. File picker -- scan CWD for `*.zlayer.yml` files
//!   2. Preview -- parse and show summary
//!   3. Deploy -- submit to daemon and show progress
//!   4. Result -- success/failure with option to view dashboard

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::app::DeployState;

// ---------------------------------------------------------------------------
// Steps
// ---------------------------------------------------------------------------

/// The current step in the deploy flow
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeployStep {
    /// Select a spec file
    SelectFile,
    /// Preview parsed spec
    Preview,
    /// Deploying (submitting to daemon)
    Deploying,
    /// Result (success or failure)
    Result,
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Render the deploy view
pub fn render(frame: &mut Frame, state: &DeployState) {
    let area = frame.area();

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Blue))
        .title(" Deploy ")
        .title_alignment(Alignment::Center);

    let inner = outer.inner(area);
    frame.render_widget(outer, area);

    // Layout: step indicator, content, footer
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // Step indicator
            Constraint::Length(1), // Spacer
            Constraint::Min(5),    // Content
            Constraint::Length(1), // Footer
        ])
        .split(inner);

    render_step_indicator(chunks[0], frame.buffer_mut(), state.step);

    match state.step {
        DeployStep::SelectFile => render_select_file(chunks[2], frame, state),
        DeployStep::Preview => render_preview(chunks[2], frame.buffer_mut(), state),
        DeployStep::Deploying => render_deploying(chunks[2], frame.buffer_mut()),
        DeployStep::Result => render_result(chunks[2], frame.buffer_mut(), state),
    }

    let footer_text = match state.step {
        DeployStep::SelectFile => "j/k: navigate | Enter: select | Esc: back",
        DeployStep::Preview => "Enter: deploy | Esc: back",
        DeployStep::Deploying => "Deploying...",
        DeployStep::Result => "d: dashboard | Enter: deploy another | Esc: menu",
    };

    let footer = Paragraph::new(footer_text)
        .style(Style::default().fg(Color::DarkGray))
        .alignment(Alignment::Center);
    frame.render_widget(footer, chunks[3]);
}

/// Render step progress
fn render_step_indicator(area: Rect, buf: &mut Buffer, current: DeployStep) {
    let steps = [
        ("Select", DeployStep::SelectFile),
        ("Preview", DeployStep::Preview),
        ("Deploy", DeployStep::Deploying),
        ("Done", DeployStep::Result),
    ];

    let step_order = |s: DeployStep| -> usize {
        match s {
            DeployStep::SelectFile => 0,
            DeployStep::Preview => 1,
            DeployStep::Deploying => 2,
            DeployStep::Result => 3,
        }
    };

    let current_idx = step_order(current);

    let mut x = area.x + 2;
    for (i, (label, _)) in steps.iter().enumerate() {
        let style = if i == current_idx {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else if i < current_idx {
            Style::default().fg(Color::Green)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let icon = if i < current_idx {
            zlayer_tui::icons::COMPLETE
        } else if i == current_idx {
            zlayer_tui::icons::RUNNING
        } else {
            zlayer_tui::icons::PENDING
        };

        let text = format!("{} {}", icon, label);
        let width = text.len() as u16;

        if x + width < area.x + area.width {
            buf.set_string(x, area.y, &text, style);
            x += width;

            if i < steps.len() - 1 && x + 4 < area.x + area.width {
                buf.set_string(x, area.y, " -> ", Style::default().fg(Color::DarkGray));
                x += 4;
            }
        }
    }
}

/// Step 1: Select a spec file
fn render_select_file(area: Rect, frame: &mut Frame, state: &DeployState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2), // Instructions
            Constraint::Min(5),    // File list
        ])
        .split(area);

    let instructions = Paragraph::new(
        "Select a deployment spec file (*.zlayer.yml, *.zlayer.yaml).\n\
         Spec files found in current directory are listed below.",
    )
    .style(Style::default().fg(Color::White));
    frame.render_widget(instructions, chunks[0]);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(" Spec Files ")
        .title_alignment(Alignment::Center);

    let inner = block.inner(chunks[1]);
    frame.render_widget(block, chunks[1]);

    if state.spec_files.is_empty() {
        let msg = Paragraph::new("No *.zlayer.yml files found in current directory.")
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center);
        frame.render_widget(msg, inner);
        return;
    }

    let buf = frame.buffer_mut();
    for (i, path) in state.spec_files.iter().enumerate() {
        let y = inner.y + i as u16;
        if y >= inner.y + inner.height {
            break;
        }

        let is_selected = i == state.selected;
        let indicator = if is_selected { " > " } else { "   " };
        let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("?");

        let label_style = if is_selected {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };

        buf.set_string(
            inner.x + 1,
            y,
            format!("{}{}", indicator, file_name),
            label_style,
        );
    }
}

/// Step 2: Preview parsed spec
fn render_preview(area: Rect, buf: &mut Buffer, state: &DeployState) {
    let (title, border_color) = if state.parse_error.is_some() {
        (" Parse Error ", Color::Red)
    } else {
        (" Spec Preview ", Color::Yellow)
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    let inner = block.inner(area);
    block.render(area, buf);

    if let Some(ref error) = state.parse_error {
        let lines = vec![
            Line::from(Span::styled(
                "Failed to parse spec file:",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(
                error.as_str(),
                Style::default().fg(Color::Red),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "Press Esc to go back and select a different file.",
                Style::default().fg(Color::DarkGray),
            )),
        ];
        let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
        paragraph.render(inner, buf);
        return;
    }

    if let Some(ref spec) = state.parsed_spec {
        let mut lines = Vec::new();

        // File info
        if let Some(ref path) = state.selected_file {
            lines.push(Line::from(vec![
                Span::styled("File:       ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    path.display().to_string(),
                    Style::default().fg(Color::White),
                ),
            ]));
        }

        lines.push(Line::from(vec![
            Span::styled("Deployment: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                &spec.deployment,
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Version:    ", Style::default().fg(Color::DarkGray)),
            Span::styled(&spec.version, Style::default().fg(Color::White)),
        ]));
        lines.push(Line::from(""));

        // Services summary
        lines.push(Line::from(Span::styled(
            format!("Services ({})", spec.services.len()),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(""));

        for (name, svc) in &spec.services {
            let replicas = match &svc.scale {
                zlayer_spec::ScaleSpec::Fixed { replicas } => format!("{} replicas", replicas),
                zlayer_spec::ScaleSpec::Adaptive { min, max, .. } => {
                    format!("{}-{} replicas (adaptive)", min, max)
                }
                zlayer_spec::ScaleSpec::Manual => "manual".to_string(),
            };

            let endpoints_str = if svc.endpoints.is_empty() {
                String::new()
            } else {
                let ep_names: Vec<&str> = svc.endpoints.iter().map(|e| e.name.as_str()).collect();
                format!(" | endpoints: {}", ep_names.join(", "))
            };

            lines.push(Line::from(vec![
                Span::styled(
                    format!("  {} ", zlayer_tui::icons::PENDING),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(
                    name.as_str(),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
            ]));
            lines.push(Line::from(vec![
                Span::styled("    ", Style::default()),
                Span::styled(&svc.image.name, Style::default().fg(Color::Yellow)),
                Span::styled(
                    format!(" | {}{}", replicas, endpoints_str),
                    Style::default().fg(Color::DarkGray),
                ),
            ]));
        }

        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Press Enter to deploy, Esc to go back.",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        )));

        let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
        paragraph.render(inner, buf);
    }
}

/// Step 3: Deploying
fn render_deploying(area: Rect, buf: &mut Buffer) {
    let block = Block::default()
        .title(" Deploying ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));

    let inner = block.inner(area);
    block.render(area, buf);

    let msg = Paragraph::new("Submitting deployment to daemon...")
        .style(Style::default().fg(Color::Yellow))
        .alignment(Alignment::Center);
    msg.render(inner, buf);
}

/// Step 4: Result
fn render_result(area: Rect, buf: &mut Buffer, state: &DeployState) {
    let (title, border_color) = if state.deploy_error.is_some() {
        (" Deploy Failed ", Color::Red)
    } else {
        (" Deploy Complete ", Color::Green)
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    let inner = block.inner(area);
    block.render(area, buf);

    let mut lines = Vec::new();

    if let Some(ref error) = state.deploy_error {
        lines.push(Line::from(Span::styled(
            "Deployment failed!",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            error.as_str(),
            Style::default().fg(Color::Red),
        )));
    } else {
        lines.push(Line::from(Span::styled(
            "Deployment submitted successfully!",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(""));
        if let Some(ref spec) = state.parsed_spec {
            lines.push(Line::from(vec![
                Span::styled("Deployment: ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    &spec.deployment,
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
            ]));
            lines.push(Line::from(vec![
                Span::styled("Services:   ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    spec.services.len().to_string(),
                    Style::default().fg(Color::White),
                ),
            ]));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Press 'd' for dashboard, Enter to deploy another, Esc for main menu.",
        Style::default().fg(Color::DarkGray),
    )));

    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    paragraph.render(inner, buf);
}

// ---------------------------------------------------------------------------
// Data helpers
// ---------------------------------------------------------------------------

/// Scan the current working directory for spec files
pub fn scan_spec_files() -> Vec<PathBuf> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut files = Vec::new();

    if let Ok(entries) = std::fs::read_dir(&cwd) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if name.ends_with(".zlayer.yml") || name.ends_with(".zlayer.yaml") {
                        files.push(path);
                    }
                }
            }
        }
    }

    files.sort();
    files
}

/// Parse a spec file and update state
pub fn parse_spec_file(state: &mut DeployState) {
    let path = match &state.selected_file {
        Some(p) => p.clone(),
        None => return,
    };

    match std::fs::read_to_string(&path) {
        Ok(content) => match zlayer_spec::from_yaml_str(&content) {
            Ok(spec) => {
                state.spec_yaml = Some(content);
                state.parsed_spec = Some(spec);
                state.parse_error = None;
            }
            Err(e) => {
                state.parse_error = Some(format!("{}", e));
                state.parsed_spec = None;
                state.spec_yaml = None;
            }
        },
        Err(e) => {
            state.parse_error = Some(format!("Failed to read file: {}", e));
            state.parsed_spec = None;
            state.spec_yaml = None;
        }
    }
}

/// Submit the deployment to the daemon
pub fn submit_deployment(rt: &tokio::runtime::Runtime, state: &mut DeployState) {
    let yaml = match &state.spec_yaml {
        Some(y) => y.clone(),
        None => {
            state.deploy_error = Some("No spec YAML available".to_string());
            state.step = DeployStep::Result;
            return;
        }
    };

    state.step = DeployStep::Deploying;

    let result = rt.block_on(async {
        let client = match tokio::time::timeout(
            std::time::Duration::from_secs(5),
            crate::daemon_client::DaemonClient::connect(),
        )
        .await
        {
            Ok(Ok(c)) => c,
            Ok(Err(e)) => return Err(format!("Failed to connect to daemon: {:#}", e)),
            Err(_) => return Err("Connection to daemon timed out".to_string()),
        };

        match client.create_deployment(&yaml).await {
            Ok(_) => Ok(()),
            Err(e) => Err(format!("{:#}", e)),
        }
    });

    match result {
        Ok(()) => {
            state.deploy_error = None;
            state.step = DeployStep::Result;
        }
        Err(msg) => {
            state.deploy_error = Some(msg);
            state.step = DeployStep::Result;
        }
    }
}

// ---------------------------------------------------------------------------
// Input handling
// ---------------------------------------------------------------------------

/// Handle key events for the deploy view. Returns a transition signal.
pub enum DeployAction {
    /// Stay in deploy view
    None,
    /// Go back to main menu
    MainMenu,
    /// Switch to dashboard
    Dashboard,
}

pub fn handle_key(
    key: KeyEvent,
    state: &mut DeployState,
    rt: &tokio::runtime::Runtime,
) -> DeployAction {
    match state.step {
        DeployStep::SelectFile => handle_select_file_key(key, state),
        DeployStep::Preview => handle_preview_key(key, state, rt),
        DeployStep::Deploying => DeployAction::None,
        DeployStep::Result => handle_result_key(key, state),
    }
}

fn handle_select_file_key(key: KeyEvent, state: &mut DeployState) -> DeployAction {
    match key.code {
        KeyCode::Up | KeyCode::Char('k') => {
            state.selected = state.selected.saturating_sub(1);
            DeployAction::None
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if !state.spec_files.is_empty() && state.selected < state.spec_files.len() - 1 {
                state.selected += 1;
            }
            DeployAction::None
        }
        KeyCode::Enter => {
            if let Some(path) = state.spec_files.get(state.selected).cloned() {
                state.selected_file = Some(path);
                parse_spec_file(state);
                state.step = DeployStep::Preview;
            }
            DeployAction::None
        }
        KeyCode::Esc | KeyCode::Char('q') => DeployAction::MainMenu,
        _ => DeployAction::None,
    }
}

fn handle_preview_key(
    key: KeyEvent,
    state: &mut DeployState,
    rt: &tokio::runtime::Runtime,
) -> DeployAction {
    match key.code {
        KeyCode::Enter => {
            if state.parse_error.is_none() && state.parsed_spec.is_some() {
                submit_deployment(rt, state);
            }
            DeployAction::None
        }
        KeyCode::Esc => {
            // Go back to file selection
            state.step = DeployStep::SelectFile;
            state.parsed_spec = None;
            state.parse_error = None;
            state.spec_yaml = None;
            state.selected_file = None;
            DeployAction::None
        }
        _ => DeployAction::None,
    }
}

fn handle_result_key(key: KeyEvent, state: &mut DeployState) -> DeployAction {
    match key.code {
        KeyCode::Char('d') => DeployAction::Dashboard,
        KeyCode::Enter => {
            // Reset for another deployment
            *state = DeployState::new();
            DeployAction::None
        }
        KeyCode::Esc | KeyCode::Char('q') => DeployAction::MainMenu,
        _ => DeployAction::None,
    }
}
