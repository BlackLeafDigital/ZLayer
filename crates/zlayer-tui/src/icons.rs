use ratatui::prelude::*;

pub const PENDING: char = '\u{25CB}';
pub const RUNNING: char = '\u{25B6}';
pub const COMPLETE: char = '\u{2713}';
pub const FAILED: char = '\u{2717}';
pub const STOPPING: char = '\u{25BC}';
pub const STOPPED: char = '\u{25A0}';
pub const PROGRESS_FILLED: char = '\u{2588}';
pub const PROGRESS_EMPTY: char = '\u{2591}';
pub const ARROW_UP: char = '\u{2191}';
pub const ARROW_DOWN: char = '\u{2193}';

pub const SPINNER_FRAMES: &[char] = &['|', '/', '-', '\\'];

pub fn status_icon(done: bool, running: bool, failed: bool) -> (char, Style) {
    if failed {
        (FAILED, Style::default().fg(Color::Red))
    } else if done {
        (COMPLETE, Style::default().fg(Color::Green))
    } else if running {
        (
            RUNNING,
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        (PENDING, Style::default().fg(Color::DarkGray))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_characters_are_correct() {
        assert_eq!(PENDING, '○');
        assert_eq!(RUNNING, '▶');
        assert_eq!(COMPLETE, '✓');
        assert_eq!(FAILED, '✗');
        assert_eq!(STOPPING, '▼');
        assert_eq!(STOPPED, '■');
        assert_eq!(PROGRESS_FILLED, '█');
        assert_eq!(PROGRESS_EMPTY, '░');
        assert_eq!(ARROW_UP, '↑');
        assert_eq!(ARROW_DOWN, '↓');
    }

    #[test]
    fn spinner_frames_has_four_entries() {
        assert_eq!(SPINNER_FRAMES.len(), 4);
        assert_eq!(SPINNER_FRAMES[0], '|');
        assert_eq!(SPINNER_FRAMES[3], '\\');
    }

    #[test]
    fn status_icon_failed_takes_priority() {
        let (icon, style) = status_icon(true, true, true);
        assert_eq!(icon, FAILED);
        assert_eq!(style.fg, Some(Color::Red));
    }

    #[test]
    fn status_icon_done() {
        let (icon, style) = status_icon(true, false, false);
        assert_eq!(icon, COMPLETE);
        assert_eq!(style.fg, Some(Color::Green));
    }

    #[test]
    fn status_icon_running() {
        let (icon, style) = status_icon(false, true, false);
        assert_eq!(icon, RUNNING);
        assert_eq!(style.fg, Some(Color::Yellow));
        assert!(style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn status_icon_pending() {
        let (icon, style) = status_icon(false, false, false);
        assert_eq!(icon, PENDING);
        assert_eq!(style.fg, Some(Color::DarkGray));
    }
}
