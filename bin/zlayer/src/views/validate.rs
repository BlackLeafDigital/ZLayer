//! Validation view
//!
//! Lets the user pick a file and parses it as a Dockerfile or ZImagefile,
//! displaying the parsed structure or any errors.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::app::{ValidateResult, ValidateStageInfo, ValidateState};
use crate::widgets::file_picker;

/// Render the validation view
pub fn render(frame: &mut Frame, state: &ValidateState) {
    let area = frame.area();

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Blue))
        .title(" Validate ")
        .title_alignment(Alignment::Center);

    let inner = outer.inner(area);
    frame.render_widget(outer, area);

    if let Some(ref result) = state.result {
        render_result(inner, frame, result);
    } else if state.picker_active {
        // Show file picker
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2),
                Constraint::Min(5),
                Constraint::Length(1),
            ])
            .split(inner);

        let instructions =
            Paragraph::new("Select a Dockerfile, ZImagefile, or Containerfile to validate.")
                .style(Style::default().fg(Color::White));
        frame.render_widget(instructions, chunks[0]);

        file_picker::render(
            chunks[1],
            frame.buffer_mut(),
            &state.file_picker,
            "Select File to Validate",
        );

        let footer = Paragraph::new("Enter: select | Esc: back to menu")
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center);
        frame.render_widget(footer, chunks[2]);
    } else {
        // Manual path input mode
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2),
                Constraint::Length(3),
                Constraint::Min(1),
            ])
            .split(inner);

        let label = Paragraph::new("Enter path to file:").style(Style::default().fg(Color::White));
        frame.render_widget(label, chunks[0]);

        let input_block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan));
        let input_inner = input_block.inner(chunks[1]);
        frame.render_widget(input_block, chunks[1]);

        let input_text = Paragraph::new(format!("{}_", state.path_input))
            .style(Style::default().fg(Color::White));
        frame.render_widget(input_text, input_inner);
    }
}

/// Render the validation result
fn render_result(area: Rect, frame: &mut Frame, result: &ValidateResult) {
    match result {
        ValidateResult::Dockerfile { path, stages } => {
            let block = Block::default()
                .title(" Valid Dockerfile ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Green));

            let inner = block.inner(area);
            frame.render_widget(block, area);

            let mut lines = Vec::new();
            lines.push(Line::from(vec![
                Span::styled("File: ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    path.as_str(),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
            ]));
            lines.push(Line::from(""));
            lines.push(Line::from(vec![Span::styled(
                format!("{} stage(s)", stages.len()),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )]));
            lines.push(Line::from(""));

            for stage in stages {
                let name_display = if stage.name.is_empty() || stage.name == "(unnamed)" {
                    format!("Stage {}", stage.index)
                } else {
                    format!("Stage {}: {}", stage.index, stage.name)
                };

                lines.push(Line::from(vec![
                    Span::styled(
                        format!("  {} ", zlayer_tui::icons::COMPLETE),
                        Style::default().fg(Color::Green),
                    ),
                    Span::styled(
                        name_display,
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]));
                lines.push(Line::from(vec![
                    Span::styled("    FROM ", Style::default().fg(Color::DarkGray)),
                    Span::styled(&stage.base_image, Style::default().fg(Color::Cyan)),
                    Span::styled(
                        format!("  ({} instructions)", stage.instruction_count),
                        Style::default().fg(Color::DarkGray),
                    ),
                ]));
            }

            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Press Esc to go back.",
                Style::default().fg(Color::DarkGray),
            )));

            let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
            frame.render_widget(paragraph, inner);
        }
        ValidateResult::ZImagefile { path, summary } => {
            let block = Block::default()
                .title(" Valid ZImagefile ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Green));

            let inner = block.inner(area);
            frame.render_widget(block, area);

            let lines = vec![
                Line::from(vec![
                    Span::styled("File: ", Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        path.as_str(),
                        Style::default()
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]),
                Line::from(""),
                Line::from(Span::styled(
                    summary.as_str(),
                    Style::default().fg(Color::Cyan),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    "Press Esc to go back.",
                    Style::default().fg(Color::DarkGray),
                )),
            ];

            let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
            frame.render_widget(paragraph, inner);
        }
        ValidateResult::Error { path, message } => {
            let block = Block::default()
                .title(" Validation Failed ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Red));

            let inner = block.inner(area);
            frame.render_widget(block, area);

            let lines = vec![
                Line::from(vec![
                    Span::styled("File: ", Style::default().fg(Color::DarkGray)),
                    Span::styled(path.as_str(), Style::default().fg(Color::White)),
                ]),
                Line::from(""),
                Line::from(Span::styled(
                    "Error:",
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                )),
                Line::from(Span::styled(
                    message.as_str(),
                    Style::default().fg(Color::Red),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    "Press Esc to go back.",
                    Style::default().fg(Color::DarkGray),
                )),
            ];

            let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
            frame.render_widget(paragraph, inner);
        }
    }
}

/// Handle key events for the validate view
pub fn handle_key(key: KeyEvent, state: &mut ValidateState) {
    // If we are showing a result, Esc clears it
    if state.result.is_some() {
        if matches!(key.code, KeyCode::Esc | KeyCode::Char('q')) {
            state.result = None;
        }
        return;
    }

    if state.picker_active {
        if let Some(selected_path) = state.file_picker.handle_key(key) {
            // Validate the selected file
            run_validation(state, &selected_path);
        }
        // Esc from picker returns to main menu (handled in app.rs)
        return;
    }

    // Manual path input mode
    match key.code {
        KeyCode::Enter => {
            if !state.path_input.is_empty() {
                let path = std::path::PathBuf::from(&state.path_input);
                run_validation(state, &path);
            }
        }
        KeyCode::Char(c) => {
            state.path_input.push(c);
        }
        KeyCode::Backspace => {
            state.path_input.pop();
        }
        KeyCode::Tab => {
            // Switch to file picker mode
            state.picker_active = true;
        }
        _ => {}
    }
}

/// Run validation on a file and store the result
fn run_validation(state: &mut ValidateState, path: &std::path::Path) {
    let path_str = path.display().to_string();

    if !path.exists() {
        state.result = Some(ValidateResult::Error {
            path: path_str,
            message: "File not found".to_string(),
        });
        return;
    }

    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            state.result = Some(ValidateResult::Error {
                path: path_str,
                message: format!("Failed to read file: {}", e),
            });
            return;
        }
    };

    // Detect format: ZImagefile (YAML) or Dockerfile
    let is_yaml = is_likely_zimagefile(path, &content);

    if is_yaml {
        match zlayer_builder::zimage::parse_zimagefile(&content) {
            Ok(zimage) => {
                let summary = summarize_zimagefile(&zimage);
                state.result = Some(ValidateResult::ZImagefile {
                    path: path_str,
                    summary,
                });
            }
            Err(e) => {
                state.result = Some(ValidateResult::Error {
                    path: path_str,
                    message: e.to_string(),
                });
            }
        }
    } else {
        match zlayer_builder::Dockerfile::parse(&content) {
            Ok(dockerfile) => {
                let stages = dockerfile
                    .stages
                    .iter()
                    .map(|s| ValidateStageInfo {
                        index: s.index,
                        name: s.name.clone().unwrap_or_else(|| "(unnamed)".to_string()),
                        base_image: s.base_image.to_string_ref(),
                        instruction_count: s.instructions.len(),
                    })
                    .collect();

                state.result = Some(ValidateResult::Dockerfile {
                    path: path_str,
                    stages,
                });
            }
            Err(e) => {
                state.result = Some(ValidateResult::Error {
                    path: path_str,
                    message: e.to_string(),
                });
            }
        }
    }
}

/// Detect whether a file is a ZImagefile (YAML)
fn is_likely_zimagefile(path: &std::path::Path, content: &str) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if name.contains("ZImagefile") {
        return true;
    }
    if matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("yml" | "yaml")
    ) {
        return true;
    }

    // Content heuristic
    let trimmed = content.trim_start();
    trimmed.starts_with("version:")
        || trimmed.starts_with("runtime:")
        || trimmed.starts_with("base:")
        || trimmed.starts_with("stages:")
        || trimmed.starts_with("wasm:")
}

/// Produce a text summary of a ZImagefile
fn summarize_zimagefile(zimage: &zlayer_builder::zimage::ZImage) -> String {
    if let Some(ref runtime) = zimage.runtime {
        format!("Mode: runtime template ({})", runtime)
    } else if let Some(ref stages) = zimage.stages {
        let stage_list: Vec<String> = stages
            .iter()
            .map(|(name, stage)| {
                format!(
                    "  {} (FROM {}, {} steps)",
                    name,
                    stage.base.as_deref().unwrap_or("scratch"),
                    stage.steps.len()
                )
            })
            .collect();
        format!(
            "Mode: multi-stage ({} stages)\n{}",
            stages.len(),
            stage_list.join("\n")
        )
    } else if let Some(ref base) = zimage.base {
        format!(
            "Mode: single-stage (FROM {})\nSteps: {}",
            base,
            zimage.steps.len()
        )
    } else if zimage.wasm.is_some() {
        "Mode: WASM".to_string()
    } else {
        "Mode: empty (no build instructions)".to_string()
    }
}
