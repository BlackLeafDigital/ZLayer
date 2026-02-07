//! Custom TUI widgets for build progress display
//!
//! This module contains reusable widgets for displaying:
//! - Build progress bars (delegated to `zlayer_tui::widgets::progress_bar`)
//! - Instruction lists with status indicators (domain-specific, kept here)
//! - Scrollable output logs (delegated to `zlayer_tui::widgets::scrollable_pane`)

use ratatui::prelude::*;

use zlayer_tui::icons;
use zlayer_tui::palette::color;

use super::app::InstructionState;
use super::InstructionStatus;

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
                InstructionStatus::Pending => {
                    (icons::PENDING, Style::default().fg(color::INACTIVE))
                }
                InstructionStatus::Running => (
                    icons::RUNNING,
                    Style::default()
                        .fg(color::WARNING)
                        .add_modifier(Modifier::BOLD),
                ),
                InstructionStatus::Complete { cached: false } => {
                    (icons::COMPLETE, Style::default().fg(color::SUCCESS))
                }
                InstructionStatus::Complete { cached: true } => {
                    (icons::COMPLETE, Style::default().fg(color::SUCCESS))
                }
                InstructionStatus::Failed => (icons::FAILED, Style::default().fg(color::ERROR)),
            };

            // Render indicator
            buf.set_string(area.x, y, indicator.to_string(), indicator_style);

            // Instruction text style
            let text_style = match inst.status {
                InstructionStatus::Pending => Style::default().fg(color::INACTIVE),
                InstructionStatus::Running => Style::default()
                    .fg(color::WARNING)
                    .add_modifier(Modifier::BOLD),
                InstructionStatus::Complete { .. } => Style::default().fg(color::TEXT),
                InstructionStatus::Failed => Style::default().fg(color::ERROR),
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
                        Style::default()
                            .fg(color::ACCENT)
                            .add_modifier(Modifier::DIM),
                    );
                }
            }
        }

        // Show scroll indicator if there are more items
        if total > visible_count {
            let indicator = format!("({}/{})", end.min(total), total);
            let x = area.x + area.width.saturating_sub(indicator.len() as u16);
            if x >= area.x {
                buf.set_string(x, area.y, &indicator, Style::default().fg(color::INACTIVE));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zlayer_tui::widgets::progress_bar::ProgressBar;
    use zlayer_tui::widgets::scrollable_pane::{OutputLine, ScrollablePane};

    fn create_buffer(width: u16, height: u16) -> Buffer {
        Buffer::empty(Rect::new(0, 0, width, height))
    }

    #[test]
    fn test_progress_bar_full() {
        let mut buf = create_buffer(40, 1);
        let area = Rect::new(0, 0, 40, 1);

        let progress = ProgressBar::new(5, 10).with_label("5/10");
        progress.render(area, &mut buf);

        // Check that something was rendered
        let content = buf.content().iter().map(|c| c.symbol()).collect::<String>();
        assert!(!content.trim().is_empty());
    }

    #[test]
    fn test_progress_bar_zero() {
        let mut buf = create_buffer(40, 1);
        let area = Rect::new(0, 0, 40, 1);

        let progress = ProgressBar::new(0, 0).with_label("0/0");
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

        let lines: Vec<OutputLine> = vec![];
        let pane = ScrollablePane::new(&lines, 0).with_empty_text("Waiting for output...");
        pane.render(area, &mut buf);

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

        let pane = ScrollablePane::new(&lines, 0);
        pane.render(area, &mut buf);

        let content = buf.content().iter().map(|c| c.symbol()).collect::<String>();
        assert!(content.contains("stdout") || content.contains("stderr"));
    }

    #[test]
    fn test_output_log_scroll() {
        let mut buf = create_buffer(40, 5);
        let area = Rect::new(0, 0, 40, 5);

        let lines: Vec<OutputLine> = (0..10)
            .map(|i| OutputLine {
                text: format!("Line {}", i),
                is_stderr: false,
            })
            .collect();

        // Scroll to middle
        let pane = ScrollablePane::new(&lines, 5);
        pane.render(area, &mut buf);

        // Check that we're showing lines from the scrolled position
        let content = buf.content().iter().map(|c| c.symbol()).collect::<String>();
        // Should show lines around position 5
        assert!(
            content.contains("Line 5") || content.contains("Line 6") || content.contains("Line 7")
        );
    }
}
