//! Build wizard view -- multi-step interactive build configuration
//!
//! Steps: SelectSource -> Configure -> Review -> Building -> Complete

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::app::{BuildStep, BuildWizardState};
use crate::widgets::file_picker;

/// Render the build wizard based on the current step
pub fn render(frame: &mut Frame, state: &BuildWizardState) {
    let area = frame.area();

    // Outer chrome
    let outer = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Blue))
        .title(" Build Image ")
        .title_alignment(Alignment::Center);

    let inner = outer.inner(area);
    frame.render_widget(outer, area);

    // Step indicator at the top
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // Step indicator
            Constraint::Length(1), // Separator
            Constraint::Min(5),    // Content
            Constraint::Length(1), // Footer
        ])
        .split(inner);

    render_step_indicator(chunks[0], frame.buffer_mut(), state.step);

    // Main content depends on step
    match state.step {
        BuildStep::SelectSource => render_select_source(chunks[2], frame, state),
        BuildStep::Configure => render_configure(chunks[2], frame.buffer_mut(), state),
        BuildStep::Review => render_review(chunks[2], frame.buffer_mut(), state),
        BuildStep::Building => render_building(chunks[2], frame, state),
        BuildStep::Complete => render_complete(chunks[2], frame.buffer_mut(), state),
    }

    // Footer with step-specific hints
    let footer_text = match state.step {
        BuildStep::SelectSource => "Enter: select | Backspace: parent dir | Esc: cancel",
        BuildStep::Configure => "Tab: next field | Enter: confirm | Esc: back",
        BuildStep::Review => "Enter: start build | Esc: back",
        BuildStep::Building => "Building... (q to cancel)",
        BuildStep::Complete => "Enter: new build | Esc/q: main menu",
    };

    let footer = Paragraph::new(footer_text)
        .style(Style::default().fg(Color::DarkGray))
        .alignment(Alignment::Center);
    frame.render_widget(footer, chunks[3]);
}

/// Render the step progress indicator
fn render_step_indicator(area: Rect, buf: &mut Buffer, current: BuildStep) {
    let steps = [
        ("Source", BuildStep::SelectSource),
        ("Configure", BuildStep::Configure),
        ("Review", BuildStep::Review),
        ("Build", BuildStep::Building),
        ("Done", BuildStep::Complete),
    ];

    let step_order = |s: BuildStep| -> usize {
        match s {
            BuildStep::SelectSource => 0,
            BuildStep::Configure => 1,
            BuildStep::Review => 2,
            BuildStep::Building => 3,
            BuildStep::Complete => 4,
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

        let prefix = if i < current_idx {
            "\u{2713} " // check mark
        } else if i == current_idx {
            "\u{25B6} " // arrow
        } else {
            "\u{25CB} " // circle
        };

        let text = format!("{}{}", prefix, label);
        let width = text.len() as u16;

        if x + width < area.x + area.width {
            buf.set_string(x, area.y, &text, style);
            x += width;

            // Arrow separator
            if i < steps.len() - 1 && x + 4 < area.x + area.width {
                buf.set_string(x, area.y, " -> ", Style::default().fg(Color::DarkGray));
                x += 4;
            }
        }
    }
}

/// Step 1: Select source file or runtime template
fn render_select_source(area: Rect, frame: &mut Frame, state: &BuildWizardState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2), // Instructions
            Constraint::Min(5),    // File picker
        ])
        .split(area);

    let instructions = Paragraph::new(
        "Select a Dockerfile, ZImagefile, or directory to build from.\n\
         Build files are highlighted in yellow.",
    )
    .style(Style::default().fg(Color::White));
    frame.render_widget(instructions, chunks[0]);

    // File picker
    file_picker::render(
        chunks[1],
        frame.buffer_mut(),
        &state.file_picker,
        "Select Build File",
    );
}

/// Step 2: Configure build options
fn render_configure(area: Rect, buf: &mut Buffer, state: &BuildWizardState) {
    let block = Block::default()
        .title(" Configure Build ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));

    let inner = block.inner(area);
    block.render(area, buf);

    // Fields layout
    let fields = [
        ("Source", source_display(state)),
        (
            "Tags",
            if state.tags.is_empty() {
                "(none - Enter to add)".to_string()
            } else {
                state.tags.join(", ")
            },
        ),
        (
            "Build Args",
            if state.build_args.is_empty() {
                "(none)".to_string()
            } else {
                state
                    .build_args
                    .iter()
                    .map(|(k, v)| format!("{}={}", k, v))
                    .collect::<Vec<_>>()
                    .join(", ")
            },
        ),
        (
            "Target Stage",
            state
                .target
                .clone()
                .unwrap_or_else(|| "(default)".to_string()),
        ),
        ("Format", state.format.clone()),
        ("Context", state.context_dir.display().to_string()),
    ];

    for (i, (label, value)) in fields.iter().enumerate() {
        let y = inner.y + (i as u16) * 2;
        if y + 1 >= inner.y + inner.height {
            break;
        }

        let is_focused = i == state.config_field;

        // Label
        let label_style = if is_focused {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        buf.set_string(inner.x + 1, y, format!("{}:", label), label_style);

        // Value
        let val_style = if is_focused {
            Style::default().fg(Color::White)
        } else {
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::DIM)
        };

        let display_value = if is_focused && !state.input_buf.is_empty() {
            format!("{}_", state.input_buf)
        } else if is_focused {
            format!("{}_", value)
        } else {
            value.clone()
        };

        let label_width = label.len() as u16 + 3;
        buf.set_string(inner.x + 1 + label_width, y, &display_value, val_style);
    }
}

/// Step 3: Review before building
fn render_review(area: Rect, buf: &mut Buffer, state: &BuildWizardState) {
    let block = Block::default()
        .title(" Review Build Configuration ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));

    let inner = block.inner(area);
    block.render(area, buf);

    let mut lines = Vec::new();
    lines.push(Line::from(vec![
        Span::styled("Source:  ", Style::default().fg(Color::DarkGray)),
        Span::styled(source_display(state), Style::default().fg(Color::White)),
    ]));
    lines.push(Line::from(""));

    if !state.tags.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("Tags:    ", Style::default().fg(Color::DarkGray)),
            Span::styled(state.tags.join(", "), Style::default().fg(Color::Cyan)),
        ]));
    } else {
        lines.push(Line::from(vec![
            Span::styled("Tags:    ", Style::default().fg(Color::DarkGray)),
            Span::styled("(none)", Style::default().fg(Color::DarkGray)),
        ]));
    }

    lines.push(Line::from(vec![
        Span::styled("Context: ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            state.context_dir.display().to_string(),
            Style::default().fg(Color::White),
        ),
    ]));

    lines.push(Line::from(vec![
        Span::styled("Format:  ", Style::default().fg(Color::DarkGray)),
        Span::styled(&state.format, Style::default().fg(Color::White)),
    ]));

    if let Some(ref target) = state.target {
        lines.push(Line::from(vec![
            Span::styled("Target:  ", Style::default().fg(Color::DarkGray)),
            Span::styled(target, Style::default().fg(Color::White)),
        ]));
    }

    if !state.build_args.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Build Arguments:",
            Style::default().fg(Color::DarkGray),
        )));
        for (k, v) in &state.build_args {
            lines.push(Line::from(vec![
                Span::styled("  ", Style::default()),
                Span::styled(k, Style::default().fg(Color::Yellow)),
                Span::styled("=", Style::default().fg(Color::DarkGray)),
                Span::styled(v, Style::default().fg(Color::White)),
            ]));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Press Enter to start building",
        Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD),
    )));

    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    paragraph.render(inner, buf);
}

/// Step 4: Building -- show progress using zlayer-builder's BuildView
fn render_building(area: Rect, frame: &mut Frame, state: &BuildWizardState) {
    if let Some(ref build_state) = state.build_state {
        // Reuse the existing BuildView widget from zlayer-builder
        let view = zlayer_builder::tui::BuildView::new(build_state);
        frame.render_widget(view, area);
    } else {
        let block = Block::default()
            .title(" Building ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow));

        let inner = block.inner(area);
        frame.render_widget(block, area);

        let msg = Paragraph::new("Initializing build...")
            .style(Style::default().fg(Color::Yellow))
            .alignment(Alignment::Center);
        frame.render_widget(msg, inner);
    }
}

/// Step 5: Build complete -- show result
fn render_complete(area: Rect, buf: &mut Buffer, state: &BuildWizardState) {
    let (title, border_color) = if state.result_error.is_some() {
        (" Build Failed ", Color::Red)
    } else {
        (" Build Complete ", Color::Green)
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    let inner = block.inner(area);
    block.render(area, buf);

    let mut lines = Vec::new();

    if let Some(ref error) = state.result_error {
        lines.push(Line::from(Span::styled(
            "Build failed!",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            error.as_str(),
            Style::default().fg(Color::Red),
        )));
    } else if let Some(ref image_id) = state.result_image_id {
        lines.push(Line::from(Span::styled(
            "Build succeeded!",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("Image ID: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                image_id.as_str(),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));

        if !state.tags.is_empty() {
            lines.push(Line::from(vec![
                Span::styled("Tags:     ", Style::default().fg(Color::DarkGray)),
                Span::styled(state.tags.join(", "), Style::default().fg(Color::Cyan)),
            ]));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Press Enter for a new build, or Esc to return to the main menu.",
        Style::default().fg(Color::DarkGray),
    )));

    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    paragraph.render(inner, buf);
}

// ---------------------------------------------------------------------------
// Input handling
// ---------------------------------------------------------------------------

/// Handle key events for the build wizard
pub fn handle_key(key: KeyEvent, state: &mut BuildWizardState) {
    match state.step {
        BuildStep::SelectSource => handle_select_source_key(key, state),
        BuildStep::Configure => handle_configure_key(key, state),
        BuildStep::Review => handle_review_key(key, state),
        BuildStep::Building => handle_building_key(key, state),
        BuildStep::Complete => handle_complete_key(key, state),
    }
}

fn handle_select_source_key(key: KeyEvent, state: &mut BuildWizardState) {
    if let Some(selected_path) = state.file_picker.handle_key(key) {
        state.source_path = Some(selected_path);
        state.step = BuildStep::Configure;
    }
}

fn handle_configure_key(key: KeyEvent, state: &mut BuildWizardState) {
    const FIELD_COUNT: usize = 6;

    match key.code {
        KeyCode::Tab | KeyCode::Down => {
            // Commit current input before moving
            commit_config_field(state);
            state.config_field = (state.config_field + 1) % FIELD_COUNT;
            state.input_buf.clear();
        }
        KeyCode::BackTab | KeyCode::Up => {
            commit_config_field(state);
            state.config_field = if state.config_field == 0 {
                FIELD_COUNT - 1
            } else {
                state.config_field - 1
            };
            state.input_buf.clear();
        }
        KeyCode::Enter => {
            commit_config_field(state);
            state.step = BuildStep::Review;
        }
        KeyCode::Esc => {
            state.step = BuildStep::SelectSource;
            state.input_buf.clear();
        }
        KeyCode::Char(c) => {
            state.input_buf.push(c);
        }
        KeyCode::Backspace => {
            state.input_buf.pop();
        }
        _ => {}
    }
}

fn commit_config_field(state: &mut BuildWizardState) {
    if state.input_buf.is_empty() {
        return;
    }

    let value = state.input_buf.clone();
    match state.config_field {
        1 => {
            // Tags: add the typed value as a new tag
            state.tags.push(value);
        }
        2 => {
            // Build args: expect KEY=VALUE
            if let Some((k, v)) = value.split_once('=') {
                state.build_args.insert(k.to_string(), v.to_string());
            }
        }
        3 => {
            // Target stage
            state.target = Some(value);
        }
        4 => {
            // Format
            state.format = value;
        }
        _ => {
            // Source and Context are not directly editable via text
        }
    }
    state.input_buf.clear();
}

fn handle_review_key(key: KeyEvent, state: &mut BuildWizardState) {
    match key.code {
        KeyCode::Enter => {
            // Start the build (transition to Building step)
            // The actual async build is kicked off here.
            // We create a channel and spawn the build on tokio.
            start_build(state);
        }
        KeyCode::Esc => {
            state.step = BuildStep::Configure;
        }
        _ => {}
    }
}

fn handle_building_key(key: KeyEvent, state: &mut BuildWizardState) {
    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => {
            // Cancel: drop the receiver (build will notice and fail)
            state.build_rx = None;
            state.result_error = Some("Build cancelled by user".to_string());
            state.step = BuildStep::Complete;
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if let Some(ref mut bs) = state.build_state {
                bs.scroll_offset = bs.scroll_offset.saturating_sub(1);
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if let Some(ref mut bs) = state.build_state {
                let max = bs.output_lines.len().saturating_sub(10);
                if bs.scroll_offset < max {
                    bs.scroll_offset += 1;
                }
            }
        }
        _ => {}
    }
}

fn handle_complete_key(key: KeyEvent, state: &mut BuildWizardState) {
    match key.code {
        KeyCode::Enter => {
            // Reset for a new build
            let ctx = state.context_dir.clone();
            *state = BuildWizardState::new(ctx);
        }
        KeyCode::Esc | KeyCode::Char('q') => {
            // Signal to app.rs to go back to main menu
            // We set step back to SelectSource so the Esc check in app.rs triggers
            state.step = BuildStep::SelectSource;
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Build execution
// ---------------------------------------------------------------------------

/// Kick off the async build using tokio::spawn
fn start_build(state: &mut BuildWizardState) {
    let (tx, rx) = std::sync::mpsc::channel();
    state.build_rx = Some(rx);
    state.build_state = Some(zlayer_builder::tui::BuildState::default());
    state.step = BuildStep::Building;

    let context_dir = state.context_dir.clone();
    let source_path = state.source_path.clone();
    let runtime_name = state.runtime.clone();
    let tags = state.tags.clone();
    let build_args = state.build_args.clone();
    let target = state.target.clone();
    let format = state.format.clone();

    // Spawn the build on the tokio runtime
    tokio::spawn(async move {
        let result = async {
            let mut builder = zlayer_builder::ImageBuilder::new(&context_dir).await?;

            if let Some(ref path) = source_path {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if name.contains("ZImagefile")
                    || matches!(
                        path.extension().and_then(|e| e.to_str()),
                        Some("yml" | "yaml")
                    )
                {
                    builder = builder.zimagefile(path);
                } else {
                    builder = builder.dockerfile(path);
                }
            }

            if let Some(ref rt_name) = runtime_name {
                if let Some(rt) = zlayer_builder::Runtime::from_name(rt_name) {
                    builder = builder.runtime(rt);
                }
            }

            for tag in &tags {
                builder = builder.tag(tag);
            }

            builder = builder.build_args(build_args);

            if let Some(ref target) = target {
                builder = builder.target(target);
            }

            builder = builder.format(&format);
            builder = builder.with_events(tx);

            builder.build().await
        }
        .await;

        if let Err(e) = result {
            tracing::error!("Build failed: {:#}", e);
        }
    });
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn source_display(state: &BuildWizardState) -> String {
    if let Some(ref path) = state.source_path {
        path.display().to_string()
    } else if let Some(ref rt) = state.runtime {
        format!("Runtime: {}", rt)
    } else {
        "(not selected)".to_string()
    }
}
