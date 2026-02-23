//! Main menu view
//!
//! Shows the ZLayer logo, subtitle, and a navigable list of actions.

use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::app::MainMenuState;
use crate::widgets::logo;

/// Menu items displayed to the user
const MENU_ITEMS: &[(&str, &str)] = &[
    ("Dashboard", "View running deployments and services"),
    ("Deploy", "Deploy from a spec file"),
    ("Build Image", "Interactive image build wizard"),
    ("Validate", "Validate a Dockerfile or ZImagefile"),
    ("Runtimes", "Browse available runtime templates"),
    ("Quit", "Exit zlayer"),
];

/// Render the main menu
pub fn render(frame: &mut Frame, state: &MainMenuState) {
    let area = frame.area();

    // Background block
    let outer = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(" zlayer ")
        .title_alignment(Alignment::Center);

    let inner = outer.inner(area);
    frame.render_widget(outer, area);

    // Vertical layout: logo, subtitle, spacer, menu, spacer, footer
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(logo::LOGO_HEIGHT),            // Logo
            Constraint::Length(1),                            // Subtitle
            Constraint::Length(2),                            // Spacer
            Constraint::Min(MENU_ITEMS.len() as u16 * 2 + 2), // Menu
            Constraint::Length(1),                            // Spacer
            Constraint::Length(1),                            // Footer
        ])
        .split(inner);

    // Logo
    logo::render_logo(chunks[0], frame.buffer_mut());

    // Subtitle
    logo::render_subtitle(chunks[1], frame.buffer_mut());

    // Menu block
    let menu_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(" Menu ")
        .title_alignment(Alignment::Center);

    let menu_inner = menu_block.inner(chunks[3]);
    frame.render_widget(menu_block, chunks[3]);

    // Render each menu item
    render_menu_items(menu_inner, frame.buffer_mut(), state);

    // Footer with help hint
    let footer = Paragraph::new("Press ? for help | q to quit")
        .style(Style::default().fg(Color::DarkGray))
        .alignment(Alignment::Center);
    frame.render_widget(footer, chunks[5]);
}

/// Render the menu items with selection highlight
fn render_menu_items(area: Rect, buf: &mut Buffer, state: &MainMenuState) {
    if area.height == 0 {
        return;
    }

    for (i, (label, description)) in MENU_ITEMS.iter().enumerate() {
        let y = area.y + (i as u16) * 2;
        if y >= area.y + area.height {
            break;
        }

        let is_selected = i == state.selected;

        // Selection indicator and label
        let indicator = if is_selected { " > " } else { "   " };
        let label_style = if is_selected {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };

        buf.set_string(
            area.x + 2,
            y,
            format!("{}{}", indicator, label),
            label_style,
        );

        // Description on the next line (if room)
        if y + 1 < area.y + area.height {
            let desc_style = if is_selected {
                Style::default().fg(Color::DarkGray)
            } else {
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM)
            };
            buf.set_string(area.x + 5, y + 1, *description, desc_style);
        }
    }
}
