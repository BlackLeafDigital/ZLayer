pub mod color {
    use ratatui::style::Color;

    pub const SUCCESS: Color = Color::Green;
    pub const ERROR: Color = Color::Red;
    pub const WARNING: Color = Color::Yellow;
    pub const ACCENT: Color = Color::Cyan;
    pub const ACTIVE_BORDER: Color = Color::Blue;
    pub const INACTIVE: Color = Color::DarkGray;
    pub const TEXT: Color = Color::White;
    pub const SCROLL_BADGE_FG: Color = Color::Black;
    pub const SCROLL_BADGE_BG: Color = Color::DarkGray;
}

pub mod ansi {
    pub const GREEN: &str = "\x1b[32m";
    pub const RED: &str = "\x1b[31m";
    pub const YELLOW: &str = "\x1b[33m";
    pub const CYAN: &str = "\x1b[36m";
    pub const DIM: &str = "\x1b[2m";
    pub const BOLD: &str = "\x1b[1m";
    pub const RESET: &str = "\x1b[0m";
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    #[test]
    fn color_constants_match_expected_values() {
        assert_eq!(color::SUCCESS, Color::Green);
        assert_eq!(color::ERROR, Color::Red);
        assert_eq!(color::WARNING, Color::Yellow);
        assert_eq!(color::ACCENT, Color::Cyan);
        assert_eq!(color::ACTIVE_BORDER, Color::Blue);
        assert_eq!(color::INACTIVE, Color::DarkGray);
        assert_eq!(color::TEXT, Color::White);
        assert_eq!(color::SCROLL_BADGE_FG, Color::Black);
        assert_eq!(color::SCROLL_BADGE_BG, Color::DarkGray);
    }

    #[test]
    fn ansi_codes_contain_escape_sequences() {
        assert!(ansi::GREEN.starts_with("\x1b["));
        assert!(ansi::RED.starts_with("\x1b["));
        assert!(ansi::YELLOW.starts_with("\x1b["));
        assert!(ansi::CYAN.starts_with("\x1b["));
        assert!(ansi::DIM.starts_with("\x1b["));
        assert!(ansi::BOLD.starts_with("\x1b["));
        assert_eq!(ansi::RESET, "\x1b[0m");
    }
}
