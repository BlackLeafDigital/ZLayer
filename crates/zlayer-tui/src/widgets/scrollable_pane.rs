//! Generic scrollable pane widget with pluggable entry types.
//!
//! Unifies the builder's `OutputLog` and deploy's `LogPane` into a single
//! reusable component with identical scroll offset math, line truncation,
//! empty state placeholder, and scroll percentage indicator.

use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::palette::color;

// ---------------------------------------------------------------------------
// PaneEntry trait
// ---------------------------------------------------------------------------

/// Trait for items that can be displayed inside a [`ScrollablePane`].
pub trait PaneEntry {
    /// The main text content of this entry.
    fn display_text(&self) -> &str;

    /// An optional styled prefix rendered before the display text.
    ///
    /// Return `Some(("prefix ", style))` to prepend a label such as `[INFO]`.
    fn prefix(&self) -> Option<(&str, Style)> {
        None
    }

    /// Style applied to the display text returned by [`display_text`](PaneEntry::display_text).
    fn text_style(&self) -> Style {
        Style::default().fg(color::TEXT)
    }
}

// ---------------------------------------------------------------------------
// ScrollablePane widget
// ---------------------------------------------------------------------------

/// A bordered, scrollable list of [`PaneEntry`] items with automatic
/// truncation and a scroll percentage badge.
pub struct ScrollablePane<'a, T: PaneEntry> {
    /// Slice of entries to display.
    pub entries: &'a [T],
    /// Line offset from the top (0 = show from the first entry).
    pub scroll_offset: usize,
    /// Optional title rendered in the top border.
    pub title: Option<&'a str>,
    /// Style applied to the block border.
    pub border_style: Style,
    /// Text shown when `entries` is empty.
    pub empty_text: &'a str,
}

impl<'a, T: PaneEntry> ScrollablePane<'a, T> {
    /// Create a new pane with sensible defaults.
    pub fn new(entries: &'a [T], scroll_offset: usize) -> Self {
        Self {
            entries,
            scroll_offset,
            title: None,
            border_style: Style::default().fg(color::INACTIVE),
            empty_text: "No output yet",
        }
    }

    /// Set the block title.
    pub fn with_title(mut self, title: &'a str) -> Self {
        self.title = Some(title);
        self
    }

    /// Set the border style.
    pub fn with_border_style(mut self, style: Style) -> Self {
        self.border_style = style;
        self
    }

    /// Set the placeholder text shown when there are no entries.
    pub fn with_empty_text(mut self, text: &'a str) -> Self {
        self.empty_text = text;
        self
    }
}

impl<T: PaneEntry> Widget for ScrollablePane<'_, T> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }

        // Block with optional title
        let mut block = Block::default()
            .borders(Borders::ALL)
            .border_style(self.border_style);

        if let Some(title) = self.title {
            block = block.title(format!(" {} ", title));
        }

        let inner = block.inner(area);
        block.render(area, buf);

        if inner.height == 0 || inner.width == 0 {
            return;
        }

        // Empty state
        if self.entries.is_empty() {
            Paragraph::new(self.empty_text)
                .style(
                    Style::default()
                        .fg(color::INACTIVE)
                        .add_modifier(Modifier::ITALIC),
                )
                .render(inner, buf);
            return;
        }

        let visible_count = inner.height as usize;
        let total = self.entries.len();

        // Scroll math: clamp so we never start past the last screenful
        let start = self.scroll_offset.min(total.saturating_sub(visible_count));
        let end = (start + visible_count).min(total);

        // Render visible entries
        for (display_idx, idx) in (start..end).enumerate() {
            if display_idx >= visible_count {
                break;
            }

            let entry = &self.entries[idx];
            let y = inner.y + display_idx as u16;
            let mut x = inner.x;
            let mut remaining_width = inner.width as usize;

            // Optional prefix
            if let Some((prefix_text, prefix_style)) = entry.prefix() {
                let pw = prefix_text.len().min(remaining_width);
                buf.set_string(x, y, &prefix_text[..pw], prefix_style);
                x += pw as u16;
                remaining_width = remaining_width.saturating_sub(pw);
            }

            // Display text with truncation
            let text = entry.display_text();
            if remaining_width == 0 {
                continue;
            }

            let display = if text.len() > remaining_width {
                // Need at least 4 chars for "X..." to make sense
                if remaining_width >= 4 {
                    format!("{}...", &text[..remaining_width.saturating_sub(3)])
                } else {
                    text[..remaining_width].to_string()
                }
            } else {
                text.to_string()
            };

            buf.set_string(x, y, &display, entry.text_style());
        }

        // Scroll percentage badge (bottom-right of inner area)
        if total > visible_count {
            let percent = if total == 0 {
                100
            } else {
                ((end as f64 / total as f64) * 100.0) as usize
            };
            let indicator = format!(" {}% ", percent);
            let ind_len = indicator.len() as u16;

            let badge_x = inner.x + inner.width.saturating_sub(ind_len + 1);
            let badge_y = inner.y + inner.height.saturating_sub(1);

            if badge_x >= inner.x && badge_y >= inner.y {
                buf.set_string(
                    badge_x,
                    badge_y,
                    &indicator,
                    Style::default()
                        .fg(color::SCROLL_BADGE_FG)
                        .bg(color::SCROLL_BADGE_BG),
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Concrete PaneEntry: OutputLine (builder stdout/stderr)
// ---------------------------------------------------------------------------

/// A single line of build output, tagged as stdout or stderr.
#[derive(Debug, Clone)]
pub struct OutputLine {
    /// The text content.
    pub text: String,
    /// Whether this line came from stderr.
    pub is_stderr: bool,
}

impl PaneEntry for OutputLine {
    fn display_text(&self) -> &str {
        &self.text
    }

    fn text_style(&self) -> Style {
        if self.is_stderr {
            Style::default().fg(color::WARNING)
        } else {
            Style::default().fg(color::TEXT)
        }
    }
}

// ---------------------------------------------------------------------------
// Concrete PaneEntry: LogEntry (deploy log pane)
// ---------------------------------------------------------------------------

/// Severity level for a log entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

/// A single log message with a severity level.
#[derive(Debug, Clone)]
pub struct LogEntry {
    /// Severity level.
    pub level: LogLevel,
    /// The log message text.
    pub message: String,
}

impl PaneEntry for LogEntry {
    fn display_text(&self) -> &str {
        &self.message
    }

    fn prefix(&self) -> Option<(&str, Style)> {
        match self.level {
            LogLevel::Info => Some((
                "[INFO] ",
                Style::default()
                    .fg(color::INACTIVE)
                    .add_modifier(Modifier::BOLD),
            )),
            LogLevel::Warn => Some((
                "[WARN] ",
                Style::default()
                    .fg(color::WARNING)
                    .add_modifier(Modifier::BOLD),
            )),
            LogLevel::Error => Some((
                "[ERROR] ",
                Style::default()
                    .fg(color::ERROR)
                    .add_modifier(Modifier::BOLD),
            )),
        }
    }

    fn text_style(&self) -> Style {
        match self.level {
            LogLevel::Info => Style::default().fg(color::INACTIVE),
            LogLevel::Warn => Style::default().fg(color::WARNING),
            LogLevel::Error => Style::default().fg(color::ERROR),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn create_buffer(width: u16, height: u16) -> Buffer {
        Buffer::empty(Rect::new(0, 0, width, height))
    }

    fn buffer_text(buf: &Buffer) -> String {
        buf.content().iter().map(|c| c.symbol()).collect()
    }

    // -- ScrollablePane with OutputLine --

    #[test]
    fn empty_entries_shows_placeholder() {
        let mut buf = create_buffer(40, 5);
        let area = Rect::new(0, 0, 40, 5);

        let entries: Vec<OutputLine> = vec![];
        let pane = ScrollablePane::new(&entries, 0);
        pane.render(area, &mut buf);

        let text = buffer_text(&buf);
        assert!(text.contains("No output yet"));
    }

    #[test]
    fn custom_empty_text() {
        let mut buf = create_buffer(40, 5);
        let area = Rect::new(0, 0, 40, 5);

        let entries: Vec<OutputLine> = vec![];
        let pane = ScrollablePane::new(&entries, 0).with_empty_text("Waiting for logs...");
        pane.render(area, &mut buf);

        let text = buffer_text(&buf);
        assert!(text.contains("Waiting for logs..."));
    }

    #[test]
    fn renders_output_lines() {
        let mut buf = create_buffer(60, 6);
        let area = Rect::new(0, 0, 60, 6);

        let entries = vec![
            OutputLine {
                text: "stdout line one".to_string(),
                is_stderr: false,
            },
            OutputLine {
                text: "stderr warning".to_string(),
                is_stderr: true,
            },
        ];

        let pane = ScrollablePane::new(&entries, 0).with_title("Output");
        pane.render(area, &mut buf);

        let text = buffer_text(&buf);
        assert!(text.contains("stdout line one"));
        assert!(text.contains("stderr warning"));
        assert!(text.contains("Output"));
    }

    #[test]
    fn truncates_long_lines() {
        // Inner width = 20 - 2 (borders) = 18
        let mut buf = create_buffer(20, 4);
        let area = Rect::new(0, 0, 20, 4);

        let entries = vec![OutputLine {
            text: "A very long line that should be truncated with ellipsis".to_string(),
            is_stderr: false,
        }];

        let pane = ScrollablePane::new(&entries, 0);
        pane.render(area, &mut buf);

        let text = buffer_text(&buf);
        assert!(text.contains("..."));
    }

    #[test]
    fn scroll_offset_clamps_past_end() {
        let mut buf = create_buffer(40, 5);
        let area = Rect::new(0, 0, 40, 5);

        let entries: Vec<OutputLine> = (0..3)
            .map(|i| OutputLine {
                text: format!("Line {}", i),
                is_stderr: false,
            })
            .collect();

        // Scroll offset way past the end -- should clamp gracefully
        let pane = ScrollablePane::new(&entries, 100);
        pane.render(area, &mut buf);

        let text = buffer_text(&buf);
        // Should still render something (the last visible entries)
        assert!(text.contains("Line"));
    }

    #[test]
    fn scroll_percentage_shown_when_scrollable() {
        let mut buf = create_buffer(40, 5);
        let area = Rect::new(0, 0, 40, 5);

        // Inner height = 5 - 2 (borders) = 3 lines visible, 10 entries total
        let entries: Vec<OutputLine> = (0..10)
            .map(|i| OutputLine {
                text: format!("Line {}", i),
                is_stderr: false,
            })
            .collect();

        let pane = ScrollablePane::new(&entries, 0);
        pane.render(area, &mut buf);

        let text = buffer_text(&buf);
        assert!(text.contains('%'));
    }

    #[test]
    fn no_scroll_badge_when_all_visible() {
        let mut buf = create_buffer(40, 10);
        let area = Rect::new(0, 0, 40, 10);

        // Inner height = 10 - 2 = 8 visible, only 2 entries
        let entries = vec![
            OutputLine {
                text: "one".to_string(),
                is_stderr: false,
            },
            OutputLine {
                text: "two".to_string(),
                is_stderr: false,
            },
        ];

        let pane = ScrollablePane::new(&entries, 0);
        pane.render(area, &mut buf);

        let text = buffer_text(&buf);
        assert!(!text.contains('%'));
    }

    // -- ScrollablePane with LogEntry --

    #[test]
    fn log_entry_prefix_rendered() {
        let mut buf = create_buffer(60, 6);
        let area = Rect::new(0, 0, 60, 6);

        let entries = vec![
            LogEntry {
                level: LogLevel::Info,
                message: "Startup complete".to_string(),
            },
            LogEntry {
                level: LogLevel::Warn,
                message: "Overlay unavailable".to_string(),
            },
            LogEntry {
                level: LogLevel::Error,
                message: "Container crashed".to_string(),
            },
        ];

        let pane = ScrollablePane::new(&entries, 0).with_title("Logs");
        pane.render(area, &mut buf);

        let text = buffer_text(&buf);
        assert!(text.contains("[INFO]"));
        assert!(text.contains("[WARN]"));
        assert!(text.contains("[ERROR]"));
        assert!(text.contains("Startup complete"));
    }

    #[test]
    fn log_entry_scroll_shows_percentage() {
        let mut buf = create_buffer(60, 5);
        let area = Rect::new(0, 0, 60, 5);

        let entries: Vec<LogEntry> = (0..20)
            .map(|i| LogEntry {
                level: LogLevel::Info,
                message: format!("Log line {}", i),
            })
            .collect();

        let pane = ScrollablePane::new(&entries, 5).with_title("Logs");
        pane.render(area, &mut buf);

        let text = buffer_text(&buf);
        assert!(text.contains('%'));
    }

    #[test]
    fn zero_height_does_not_panic() {
        let mut buf = create_buffer(40, 0);
        let area = Rect::new(0, 0, 40, 0);

        let entries = vec![OutputLine {
            text: "hello".to_string(),
            is_stderr: false,
        }];

        let pane = ScrollablePane::new(&entries, 0);
        pane.render(area, &mut buf);
    }

    #[test]
    fn zero_width_does_not_panic() {
        let mut buf = create_buffer(0, 5);
        let area = Rect::new(0, 0, 0, 5);

        let entries = vec![OutputLine {
            text: "hello".to_string(),
            is_stderr: false,
        }];

        let pane = ScrollablePane::new(&entries, 0);
        pane.render(area, &mut buf);
    }

    #[test]
    fn builder_methods_chain() {
        let entries: Vec<OutputLine> = vec![];
        let pane = ScrollablePane::new(&entries, 0)
            .with_title("Test")
            .with_border_style(Style::default().fg(Color::Blue))
            .with_empty_text("Nothing here");

        assert_eq!(pane.title, Some("Test"));
        assert_eq!(pane.empty_text, "Nothing here");
    }

    #[test]
    fn output_line_stderr_vs_stdout_styles() {
        let stdout = OutputLine {
            text: "ok".to_string(),
            is_stderr: false,
        };
        let stderr = OutputLine {
            text: "err".to_string(),
            is_stderr: true,
        };

        assert_eq!(stdout.text_style().fg, Some(color::TEXT));
        assert_eq!(stderr.text_style().fg, Some(color::WARNING));
    }

    #[test]
    fn log_level_styles_differ() {
        let info = LogEntry {
            level: LogLevel::Info,
            message: String::new(),
        };
        let warn = LogEntry {
            level: LogLevel::Warn,
            message: String::new(),
        };
        let error = LogEntry {
            level: LogLevel::Error,
            message: String::new(),
        };

        assert_eq!(info.text_style().fg, Some(color::INACTIVE));
        assert_eq!(warn.text_style().fg, Some(color::WARNING));
        assert_eq!(error.text_style().fg, Some(color::ERROR));
    }
}
