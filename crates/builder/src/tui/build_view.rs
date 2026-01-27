//! Build progress view layout
//!
//! This module contains the main build view widget that composes
//! the progress display from smaller widgets.

use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use super::app::BuildState;
use super::widgets::{BuildProgress, InstructionList, OutputLog};

/// Main build progress view widget
pub struct BuildView<'a> {
    state: &'a BuildState,
}

impl<'a> BuildView<'a> {
    /// Create a new build view
    pub fn new(state: &'a BuildState) -> Self {
        Self { state }
    }

    /// Calculate the layout for the view
    fn layout(&self, area: Rect) -> (Rect, Rect, Rect, Rect) {
        // Main layout:
        // - Header: Stage info + progress bar (3 lines)
        // - Instructions: List of instructions (flexible)
        // - Output: Streaming output (flexible)
        // - Footer: Help text (1 line)

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(4), // Header with progress
                Constraint::Min(6),    // Instructions list
                Constraint::Min(8),    // Output log
                Constraint::Length(1), // Footer
            ])
            .split(area);

        (chunks[0], chunks[1], chunks[2], chunks[3])
    }
}

impl Widget for BuildView<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let (header_area, instructions_area, output_area, footer_area) = self.layout(area);

        // Render header with stage info and progress
        self.render_header(header_area, buf);

        // Render instructions list
        self.render_instructions(instructions_area, buf);

        // Render output log
        self.render_output(output_area, buf);

        // Render footer
        self.render_footer(footer_area, buf);
    }
}

impl BuildView<'_> {
    /// Render the header section with stage info and progress bar
    fn render_header(&self, area: Rect, buf: &mut Buffer) {
        let block = Block::default()
            .title(" Build Progress ")
            .borders(Borders::ALL)
            .border_style(self.header_border_style());

        let inner = block.inner(area);
        block.render(area, buf);

        // Split inner area for stage info and progress bar
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Length(1)])
            .split(inner);

        // Stage info
        let stage_info = self.state.current_stage_display();
        let stage_style = if self.state.error.is_some() {
            Style::default().fg(Color::Red)
        } else if self.state.completed {
            Style::default().fg(Color::Green)
        } else {
            Style::default().fg(Color::Cyan)
        };

        Paragraph::new(stage_info)
            .style(stage_style)
            .render(chunks[0], buf);

        // Progress bar
        let total = self.state.total_instructions().max(1);
        let completed = self.state.completed_instructions();

        let progress = BuildProgress {
            current: completed,
            total,
            label: format!("{}/{} instructions", completed, total),
        };
        progress.render(chunks[1], buf);
    }

    /// Get the border style for the header based on build status
    fn header_border_style(&self) -> Style {
        if self.state.error.is_some() {
            Style::default().fg(Color::Red)
        } else if self.state.completed {
            Style::default().fg(Color::Green)
        } else {
            Style::default().fg(Color::Blue)
        }
    }

    /// Render the instructions list
    fn render_instructions(&self, area: Rect, buf: &mut Buffer) {
        let title = if let Some(stage) = self.state.stages.get(self.state.current_stage) {
            if let Some(ref name) = stage.name {
                format!(" Stage: {} ", name)
            } else {
                format!(" Stage {} ", self.state.current_stage + 1)
            }
        } else {
            " Instructions ".to_string()
        };

        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray));

        let inner = block.inner(area);
        block.render(area, buf);

        if let Some(stage) = self.state.stages.get(self.state.current_stage) {
            let list = InstructionList {
                instructions: &stage.instructions,
                current: self.state.current_instruction,
            };
            list.render(inner, buf);
        } else {
            Paragraph::new("Waiting for build to start...")
                .style(Style::default().fg(Color::DarkGray))
                .render(inner, buf);
        }
    }

    /// Render the output log
    fn render_output(&self, area: Rect, buf: &mut Buffer) {
        let block = Block::default()
            .title(" Output ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray));

        let inner = block.inner(area);
        block.render(area, buf);

        // Show completion message or error if build is done
        if let Some(ref error) = self.state.error {
            let error_text = format!("Build failed: {}", error);
            Paragraph::new(error_text)
                .style(Style::default().fg(Color::Red))
                .wrap(Wrap { trim: false })
                .render(inner, buf);
        } else if let Some(ref image_id) = self.state.image_id {
            let success_text = format!("Build complete!\n\nImage: {}", image_id);
            Paragraph::new(success_text)
                .style(Style::default().fg(Color::Green))
                .render(inner, buf);
        } else {
            let log = OutputLog {
                lines: &self.state.output_lines,
                scroll: self.state.scroll_offset,
            };
            log.render(inner, buf);
        }
    }

    /// Render the footer with help text
    fn render_footer(&self, area: Rect, buf: &mut Buffer) {
        let help_text = if self.state.completed {
            "Press 'q' to exit"
        } else {
            "q: quit | arrows/jk: scroll | PgUp/PgDn: page"
        };

        Paragraph::new(help_text)
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center)
            .render(area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::{InstructionState, OutputLine, StageState};
    use crate::tui::InstructionStatus;

    fn create_test_state() -> BuildState {
        BuildState {
            stages: vec![StageState {
                index: 0,
                name: Some("builder".to_string()),
                base_image: "node:20-alpine".to_string(),
                instructions: vec![
                    InstructionState {
                        text: "WORKDIR /app".to_string(),
                        status: InstructionStatus::Complete { cached: false },
                    },
                    InstructionState {
                        text: "COPY package*.json ./".to_string(),
                        status: InstructionStatus::Complete { cached: true },
                    },
                    InstructionState {
                        text: "RUN npm ci".to_string(),
                        status: InstructionStatus::Running,
                    },
                    InstructionState {
                        text: "COPY . .".to_string(),
                        status: InstructionStatus::Pending,
                    },
                ],
                complete: false,
            }],
            current_stage: 0,
            current_instruction: 2,
            output_lines: vec![
                OutputLine {
                    text: "npm warn deprecated inflight@1.0.6".to_string(),
                    is_stderr: true,
                },
                OutputLine {
                    text: "added 847 packages in 12s".to_string(),
                    is_stderr: false,
                },
            ],
            scroll_offset: 0,
            completed: false,
            error: None,
            image_id: None,
        }
    }

    #[test]
    fn test_build_view_creation() {
        let state = create_test_state();
        let view = BuildView::new(&state);
        assert_eq!(view.state.current_stage, 0);
    }

    #[test]
    fn test_layout_calculation() {
        let state = create_test_state();
        let view = BuildView::new(&state);
        let area = Rect::new(0, 0, 80, 24);

        let (header, instructions, output, footer) = view.layout(area);

        // Check that all areas are within bounds
        assert!(header.y + header.height <= area.height);
        assert!(instructions.y + instructions.height <= area.height);
        assert!(output.y + output.height <= area.height);
        assert!(footer.y + footer.height <= area.height);

        // Check that areas don't overlap
        assert!(header.y + header.height <= instructions.y);
        assert!(instructions.y + instructions.height <= output.y);
        assert!(output.y + output.height <= footer.y);
    }

    #[test]
    fn test_header_border_style_normal() {
        let state = BuildState::default();
        let view = BuildView::new(&state);
        let style = view.header_border_style();
        assert_eq!(style.fg, Some(Color::Blue));
    }

    #[test]
    fn test_header_border_style_error() {
        let state = BuildState {
            error: Some("test error".to_string()),
            ..Default::default()
        };
        let view = BuildView::new(&state);
        let style = view.header_border_style();
        assert_eq!(style.fg, Some(Color::Red));
    }

    #[test]
    fn test_header_border_style_complete() {
        let state = BuildState {
            completed: true,
            image_id: Some("sha256:abc".to_string()),
            ..Default::default()
        };
        let view = BuildView::new(&state);
        let style = view.header_border_style();
        assert_eq!(style.fg, Some(Color::Green));
    }
}
