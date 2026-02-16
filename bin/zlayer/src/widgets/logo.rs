//! ZLayer ASCII art logo widget

use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

/// The ZLayer ASCII banner
const ZLAYER_LOGO: &str = r#"
 ________  _
|__  /  / | |    __ _ _   _  ___ _ __
  / / /   | |   / _` | | | |/ _ \ '__|
 / / /    | |__| (_| | |_| |  __/ |
/___/_____|_____\__,_|\__, |\___|_|
                      |___/
"#;

/// Logo height in terminal lines (including blank line above)
pub const LOGO_HEIGHT: u16 = 7;

/// Render the ZLayer logo into the given area
pub fn render_logo(area: Rect, buf: &mut Buffer) {
    let logo = Paragraph::new(ZLAYER_LOGO.trim_start_matches('\n'))
        .style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .alignment(Alignment::Center);

    logo.render(area, buf);
}

/// Render a subtitle line below the logo
pub fn render_subtitle(area: Rect, buf: &mut Buffer) {
    let subtitle = Paragraph::new("Container Orchestration Platform")
        .style(Style::default().fg(Color::DarkGray))
        .alignment(Alignment::Center);

    subtitle.render(area, buf);
}
