//! Custom TUI widgets for build progress display
//!
//! This module contains reusable widgets for displaying:
//! - Build progress bars
//! - Instruction lists with status indicators
//! - Scrollable output logs

use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use super::app::{InstructionState, OutputLine};
use super::InstructionStatus;

/// Progress bar widget for build progress
pub struct BuildProgress {
    /// Current progress value
    pub current: usize,
    /// Total value
    pub total: usize,
    /// Label to display
    pub label: String,
}

impl Widget for BuildProgress {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width < 3 || area.height < 1 {
            return;
        }

        let ratio = if self.total == 0 {
            0.0
        } else {
            (self.current as f64 / self.total as f64).clamp(0.0, 1.0)
        };

        // Calculate bar width (leave room for label)
        let label_width = self.label.len() as u16 + 2; // 2 for spacing
        let bar_width = area.width.saturating_sub(label_width);

        if bar_width < 5 {
            // Not enough room for bar, just show label
            Paragraph::new(self.label)
                .style(Style::default().fg(Color::Cyan))
                .render(area, buf);
            return;
        }

        // Draw progress bar
        let filled = (bar_width as f64 * ratio).round() as u16;
        let empty = bar_width.saturating_sub(filled);

        let bar_area = Rect {
            x: area.x,
            y: area.y,
            width: bar_width,
            height: 1,
        };

        // Build the bar string - take exactly bar_width characters
        let bar_chars: String = std::iter::repeat_n('\u{2588}', filled as usize) // full block
            .chain(std::iter::repeat_n('\u{2591}', empty as usize)) // light shade
            .take(bar_width as usize)
            .collect();

        // Render bar - the string is already the right length in characters
        buf.set_string(
            bar_area.x,
            bar_area.y,
            &bar_chars,
            Style::default().fg(Color::Cyan),
        );

        // Render label after bar
        let label_area = Rect {
            x: area.x + bar_width + 1,
            y: area.y,
            width: label_width.saturating_sub(1),
            height: 1,
        };

        Paragraph::new(self.label)
            .style(Style::default().fg(Color::White))
            .render(label_area, buf);
    }
}

/// List of instructions with status indicators
pub struct InstructionList<'a> {
    /// Instructions to display
    pub instructions: &'a [InstructionState],
    /// Currently executing instruction index
    pub current: usize,
}

impl Widget for InstructionList<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 {
            return;
        }

        // Calculate which instructions to show (try to keep current visible)
        let visible_count = area.height as usize;
        let total = self.instructions.len();

        let (start, end) = if total <= visible_count {
            (0, total)
        } else {
            // Try to center current instruction
            let half = visible_count / 2;
            let start = if self.current < half {
                0
            } else if self.current + half >= total {
                total.saturating_sub(visible_count)
            } else {
                self.current.saturating_sub(half)
            };
            let end = (start + visible_count).min(total);
            (start, end)
        };

        // Render each visible instruction
        for (display_idx, idx) in (start..end).enumerate() {
            if display_idx >= area.height as usize {
                break;
            }

            let inst = &self.instructions[idx];
            let y = area.y + display_idx as u16;

            // Status indicator
            let (indicator, indicator_style) = match inst.status {
                InstructionStatus::Pending => (
                    '\u{25CB}', // ○
                    Style::default().fg(Color::DarkGray),
                ),
                InstructionStatus::Running => (
                    '\u{25B6}', // ▶
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                InstructionStatus::Complete { cached: false } => (
                    '\u{2713}', // ✓
                    Style::default().fg(Color::Green),
                ),
                InstructionStatus::Complete { cached: true } => (
                    '\u{2713}', // ✓
                    Style::default().fg(Color::Green),
                ),
                InstructionStatus::Failed => (
                    '\u{2717}', // ✗
                    Style::default().fg(Color::Red),
                ),
            };

            // Render indicator
            buf.set_string(area.x, y, indicator.to_string(), indicator_style);

            // Instruction text style
            let text_style = match inst.status {
                InstructionStatus::Pending => Style::default().fg(Color::DarkGray),
                InstructionStatus::Running => Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
                InstructionStatus::Complete { .. } => Style::default().fg(Color::White),
                InstructionStatus::Failed => Style::default().fg(Color::Red),
            };

            // Truncate instruction text if needed
            let available_width = area.width.saturating_sub(2) as usize; // 2 for indicator + space
            let text = if inst.text.len() > available_width {
                format!("{}...", &inst.text[..available_width.saturating_sub(3)])
            } else {
                inst.text.clone()
            };

            // Render instruction text
            buf.set_string(area.x + 2, y, &text, text_style);

            // Add [cached] suffix for cached instructions
            if let InstructionStatus::Complete { cached: true } = inst.status {
                let cached_str = " [cached]";
                let text_end = area.x + 2 + text.len() as u16;
                let cached_x =
                    text_end.min(area.x + area.width.saturating_sub(cached_str.len() as u16));

                if cached_x + cached_str.len() as u16 <= area.x + area.width {
                    buf.set_string(
                        cached_x,
                        y,
                        cached_str,
                        Style::default().fg(Color::Cyan).add_modifier(Modifier::DIM),
                    );
                }
            }
        }

        // Show scroll indicator if there are more items
        if total > visible_count {
            let indicator = format!("({}/{})", end.min(total), total);
            let x = area.x + area.width.saturating_sub(indicator.len() as u16);
            if x >= area.x {
                buf.set_string(x, area.y, &indicator, Style::default().fg(Color::DarkGray));
            }
        }
    }
}

/// Scrollable output log widget
pub struct OutputLog<'a> {
    /// Output lines to display
    pub lines: &'a [OutputLine],
    /// Current scroll offset
    pub scroll: usize,
}

impl Widget for OutputLog<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || self.lines.is_empty() {
            // Show placeholder when no output
            if area.height > 0 {
                Paragraph::new("Waiting for output...")
                    .style(
                        Style::default()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::ITALIC),
                    )
                    .render(area, buf);
            }
            return;
        }

        let visible_count = area.height as usize;
        let total = self.lines.len();

        // Calculate visible range
        let start = self.scroll.min(total.saturating_sub(visible_count));
        let end = (start + visible_count).min(total);

        // Render each visible line
        for (display_idx, idx) in (start..end).enumerate() {
            if display_idx >= area.height as usize {
                break;
            }

            let line = &self.lines[idx];
            let y = area.y + display_idx as u16;

            // Style based on stderr/stdout
            let style = if line.is_stderr {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::White)
            };

            // Truncate line if needed
            let available_width = area.width as usize;
            let text = if line.text.len() > available_width {
                format!("{}...", &line.text[..available_width.saturating_sub(3)])
            } else {
                line.text.clone()
            };

            buf.set_string(area.x, y, &text, style);
        }

        // Show scroll position indicator if scrollable
        if total > visible_count {
            let percent = if total == 0 {
                100
            } else {
                ((end as f64 / total as f64) * 100.0) as usize
            };
            let indicator = format!(" {}% ", percent);
            let x = area.x + area.width.saturating_sub(indicator.len() as u16 + 1);
            let y = area.y + area.height.saturating_sub(1);

            if x >= area.x && y >= area.y {
                buf.set_string(
                    x,
                    y,
                    &indicator,
                    Style::default().fg(Color::Black).bg(Color::DarkGray),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_buffer(width: u16, height: u16) -> Buffer {
        Buffer::empty(Rect::new(0, 0, width, height))
    }

    #[test]
    fn test_build_progress_full() {
        let mut buf = create_buffer(40, 1);
        let area = Rect::new(0, 0, 40, 1);

        let progress = BuildProgress {
            current: 5,
            total: 10,
            label: "5/10".to_string(),
        };

        progress.render(area, &mut buf);

        // Check that something was rendered
        let content = buf.content().iter().map(|c| c.symbol()).collect::<String>();
        assert!(!content.trim().is_empty());
    }

    #[test]
    fn test_build_progress_zero() {
        let mut buf = create_buffer(40, 1);
        let area = Rect::new(0, 0, 40, 1);

        let progress = BuildProgress {
            current: 0,
            total: 0,
            label: "0/0".to_string(),
        };

        progress.render(area, &mut buf);

        // Should not panic with zero total
    }

    #[test]
    fn test_instruction_list_empty() {
        let mut buf = create_buffer(40, 5);
        let area = Rect::new(0, 0, 40, 5);

        let list = InstructionList {
            instructions: &[],
            current: 0,
        };

        list.render(area, &mut buf);

        // Should not panic with empty list
    }

    #[test]
    fn test_instruction_list_with_items() {
        let mut buf = create_buffer(60, 5);
        let area = Rect::new(0, 0, 60, 5);

        let instructions = vec![
            InstructionState {
                text: "WORKDIR /app".to_string(),
                status: InstructionStatus::Complete { cached: false },
            },
            InstructionState {
                text: "RUN npm ci".to_string(),
                status: InstructionStatus::Running,
            },
            InstructionState {
                text: "COPY . .".to_string(),
                status: InstructionStatus::Pending,
            },
        ];

        let list = InstructionList {
            instructions: &instructions,
            current: 1,
        };

        list.render(area, &mut buf);

        // Check that content was rendered
        let content = buf.content().iter().map(|c| c.symbol()).collect::<String>();
        assert!(content.contains("WORKDIR") || content.contains("RUN") || content.contains("COPY"));
    }

    #[test]
    fn test_instruction_list_cached_tag() {
        let mut buf = create_buffer(60, 2);
        let area = Rect::new(0, 0, 60, 2);

        let instructions = vec![InstructionState {
            text: "COPY package.json ./".to_string(),
            status: InstructionStatus::Complete { cached: true },
        }];

        let list = InstructionList {
            instructions: &instructions,
            current: 0,
        };

        list.render(area, &mut buf);

        let content = buf.content().iter().map(|c| c.symbol()).collect::<String>();
        assert!(content.contains("[cached]"));
    }

    #[test]
    fn test_output_log_empty() {
        let mut buf = create_buffer(40, 5);
        let area = Rect::new(0, 0, 40, 5);

        let log = OutputLog {
            lines: &[],
            scroll: 0,
        };

        log.render(area, &mut buf);

        let content = buf.content().iter().map(|c| c.symbol()).collect::<String>();
        assert!(content.contains("Waiting"));
    }

    #[test]
    fn test_output_log_with_lines() {
        let mut buf = create_buffer(60, 5);
        let area = Rect::new(0, 0, 60, 5);

        let lines = vec![
            OutputLine {
                text: "stdout line 1".to_string(),
                is_stderr: false,
            },
            OutputLine {
                text: "stderr line 1".to_string(),
                is_stderr: true,
            },
        ];

        let log = OutputLog {
            lines: &lines,
            scroll: 0,
        };

        log.render(area, &mut buf);

        let content = buf.content().iter().map(|c| c.symbol()).collect::<String>();
        assert!(content.contains("stdout") || content.contains("stderr"));
    }

    #[test]
    fn test_output_log_scroll() {
        let mut buf = create_buffer(40, 2);
        let area = Rect::new(0, 0, 40, 2);

        let lines: Vec<OutputLine> = (0..10)
            .map(|i| OutputLine {
                text: format!("Line {}", i),
                is_stderr: false,
            })
            .collect();

        // Scroll to middle
        let log = OutputLog {
            lines: &lines,
            scroll: 5,
        };

        log.render(area, &mut buf);

        // Check that we're showing lines from the scrolled position
        let content = buf.content().iter().map(|c| c.symbol()).collect::<String>();
        // Should show lines around position 5
        assert!(
            content.contains("Line 5") || content.contains("Line 6") || content.contains("Line 7")
        );
    }
}
