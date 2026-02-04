//! Runtime template browser view
//!
//! Displays a scrollable list of available runtime templates with details.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph, Row, Table, Wrap};

use crate::app::RuntimeBrowserState;

/// Render the runtime browser
pub fn render(frame: &mut Frame, state: &RuntimeBrowserState) {
    let area = frame.area();

    let outer = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Blue))
        .title(" Runtime Templates ")
        .title_alignment(Alignment::Center);

    let inner = outer.inner(area);
    frame.render_widget(outer, area);

    let templates = zlayer_builder::list_templates();

    if state.show_detail {
        // Split: table on left, detail on right
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(inner);

        render_template_table(chunks[0], frame.buffer_mut(), &templates, state.selected);
        render_template_detail(chunks[1], frame.buffer_mut(), &templates, state.selected);
    } else {
        render_template_table(inner, frame.buffer_mut(), &templates, state.selected);
    }

    // Footer
    let footer_area = Rect {
        x: area.x + 1,
        y: area.y + area.height.saturating_sub(1),
        width: area.width.saturating_sub(2),
        height: 1,
    };

    let footer = Paragraph::new("j/k: navigate | Enter: toggle detail | Esc: back")
        .style(Style::default().fg(Color::DarkGray))
        .alignment(Alignment::Center);
    frame.render_widget(footer, footer_area);
}

/// Render the template list as a table
fn render_template_table(
    area: Rect,
    buf: &mut Buffer,
    templates: &[&zlayer_builder::RuntimeInfo],
    selected: usize,
) {
    let block = Block::default()
        .title(" Available Runtimes ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));

    let inner = block.inner(area);
    block.render(area, buf);

    if templates.is_empty() {
        Paragraph::new("No runtime templates available")
            .style(Style::default().fg(Color::DarkGray))
            .render(inner, buf);
        return;
    }

    // Header row
    let header = Row::new(vec!["Runtime", "Detect Files", "Description"])
        .style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .bottom_margin(1);

    // Data rows
    let rows: Vec<Row> = templates
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let style = if i == selected {
                Style::default().fg(Color::White).bg(Color::Blue)
            } else {
                Style::default().fg(Color::White)
            };

            Row::new(vec![
                t.name.to_string(),
                t.detect_files.join(", "),
                t.description.to_string(),
            ])
            .style(style)
        })
        .collect();

    let widths = [
        Constraint::Length(14),
        Constraint::Length(20),
        Constraint::Min(20),
    ];

    let table = Table::new(rows, widths).header(header);
    Widget::render(table, inner, buf);
}

/// Render detail pane for the selected runtime
fn render_template_detail(
    area: Rect,
    buf: &mut Buffer,
    templates: &[&zlayer_builder::RuntimeInfo],
    selected: usize,
) {
    let block = Block::default()
        .title(" Detail ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));

    let inner = block.inner(area);
    block.render(area, buf);

    if let Some(template) = templates.get(selected) {
        let mut lines = vec![
            Line::from(Span::styled(
                template.name,
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(vec![
                Span::styled("Description: ", Style::default().fg(Color::DarkGray)),
                Span::styled(template.description, Style::default().fg(Color::White)),
            ]),
            Line::from(""),
            Line::from(Span::styled(
                "Detection Files:",
                Style::default().fg(Color::DarkGray),
            )),
        ];
        for file in template.detect_files {
            lines.push(Line::from(vec![
                Span::styled("  - ", Style::default().fg(Color::DarkGray)),
                Span::styled(*file, Style::default().fg(Color::Yellow)),
            ]));
        }

        lines.push(Line::from(""));

        // Show the generated Dockerfile template
        let template_content = zlayer_builder::get_template(template.runtime);
        lines.push(Line::from(Span::styled(
            "Generated Dockerfile:",
            Style::default().fg(Color::DarkGray),
        )));
        lines.push(Line::from(""));
        for tpl_line in template_content.lines() {
            let style = if tpl_line.starts_with("FROM") || tpl_line.starts_with("RUN") {
                Style::default().fg(Color::Cyan)
            } else if tpl_line.starts_with('#') {
                Style::default().fg(Color::DarkGray)
            } else {
                Style::default().fg(Color::White)
            };
            lines.push(Line::from(Span::styled(tpl_line, style)));
        }

        let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
        paragraph.render(inner, buf);
    } else {
        Paragraph::new("No template selected")
            .style(Style::default().fg(Color::DarkGray))
            .render(inner, buf);
    }
}

/// Handle key events for the runtime browser
pub fn handle_key(key: KeyEvent, state: &mut RuntimeBrowserState) {
    let template_count = zlayer_builder::list_templates().len();

    match key.code {
        KeyCode::Up | KeyCode::Char('k') => {
            state.selected = state.selected.saturating_sub(1);
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if template_count > 0 && state.selected < template_count - 1 {
                state.selected += 1;
            }
        }
        KeyCode::Enter => {
            state.show_detail = !state.show_detail;
        }
        _ => {}
    }
}
