//! A unified progress bar widget for ZLayer TUI applications.
//!
//! Replaces the builder's `BuildProgress` and the deploy TUI's
//! `render_progress_bar()` with a single configurable widget that supports
//! both a trailing label (builder style) and a trailing percentage (deploy
//! style).

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::Style,
    widgets::{Paragraph, Widget},
};

use crate::icons::{PROGRESS_EMPTY, PROGRESS_FILLED};
use crate::palette::color::{ACCENT, TEXT};

/// A configurable progress bar widget.
///
/// # Rendering modes
///
/// | Field              | Effect                                         |
/// |--------------------|-------------------------------------------------|
/// | `label`            | Rendered after the bar, right-aligned in a row  |
/// | `show_percentage`  | Appends `" N%"` after the bar (and after label) |
///
/// Both may be enabled simultaneously.
///
/// # Examples
///
/// ```
/// use zlayer_tui::widgets::progress_bar::ProgressBar;
///
/// // Minimal usage -- just a bar
/// let bar = ProgressBar::new(3, 10);
///
/// // Builder-style with label
/// let bar = ProgressBar::new(3, 10).with_label("Step 3/10");
///
/// // Deploy-style with percentage
/// let bar = ProgressBar::new(3, 10).with_percentage();
///
/// // Compact string for embedding in table cells
/// let text = ProgressBar::new(3, 10).with_percentage().to_string_compact(20);
/// ```
pub struct ProgressBar {
    pub current: usize,
    pub total: usize,
    pub label: Option<String>,
    pub show_percentage: bool,
    pub bar_style: Style,
    pub label_style: Style,
}

impl ProgressBar {
    /// Create a new progress bar with sensible defaults.
    ///
    /// Defaults: no label, no percentage, bar styled with [`ACCENT`] foreground,
    /// label styled with [`TEXT`] foreground.
    pub fn new(current: usize, total: usize) -> Self {
        Self {
            current,
            total,
            label: None,
            show_percentage: false,
            bar_style: Style::default().fg(ACCENT),
            label_style: Style::default().fg(TEXT),
        }
    }

    /// Attach a text label that will be rendered after the bar.
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Enable a trailing percentage indicator (e.g. `" 30%"`).
    pub fn with_percentage(mut self) -> Self {
        self.show_percentage = true;
        self
    }

    // ------------------------------------------------------------------
    // Shared helpers
    // ------------------------------------------------------------------

    /// Compute the fill ratio, clamped to `[0.0, 1.0]`.
    fn ratio(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            (self.current as f64 / self.total as f64).clamp(0.0, 1.0)
        }
    }

    /// Build the bar characters for a given `width` (in columns).
    fn bar_string(ratio: f64, width: usize) -> String {
        let filled = (width as f64 * ratio).round() as usize;
        let empty = width.saturating_sub(filled);
        std::iter::repeat_n(PROGRESS_FILLED, filled)
            .chain(std::iter::repeat_n(PROGRESS_EMPTY, empty))
            .take(width)
            .collect()
    }

    /// Produce a compact string representation suitable for embedding inside
    /// table cells or log lines.
    ///
    /// The returned string has the form `"████░░░░ label N%"` depending on
    /// which display options are enabled.  The bar itself occupies exactly
    /// `width` columns; the suffix is appended after a space.
    ///
    /// If `total` is zero the bar is empty but still occupies `width` columns.
    pub fn to_string_compact(&self, width: usize) -> String {
        let ratio = self.ratio();
        let bar = Self::bar_string(ratio, width);

        let mut suffix = String::new();
        if let Some(ref label) = self.label {
            suffix.push(' ');
            suffix.push_str(label);
        }
        if self.show_percentage {
            let percent = (ratio * 100.0) as u32;
            suffix.push(' ');
            suffix.push_str(&format!("{}%", percent));
        }

        format!("{}{}", bar, suffix)
    }
}

impl Widget for ProgressBar {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width < 3 || area.height < 1 {
            return;
        }

        let ratio = self.ratio();

        // Calculate the suffix width so we know how much space the bar gets.
        let mut suffix = String::new();
        if let Some(ref label) = self.label {
            // One space separator + label text
            suffix.push(' ');
            suffix.push_str(label);
        }
        if self.show_percentage {
            let percent = (ratio * 100.0) as u32;
            suffix.push(' ');
            suffix.push_str(&format!("{}%", percent));
        }

        let suffix_width = suffix.len() as u16;
        let bar_width = area.width.saturating_sub(suffix_width);

        // If there is not enough room for a meaningful bar, fall back to
        // rendering just the label/suffix as text.
        if bar_width < 5 {
            let fallback = suffix.trim_start().to_string();
            Paragraph::new(fallback)
                .style(self.label_style)
                .render(area, buf);
            return;
        }

        // Draw the bar characters.
        let bar = Self::bar_string(ratio, bar_width as usize);
        buf.set_string(area.x, area.y, &bar, self.bar_style);

        // Draw the suffix (label and/or percentage) to the right of the bar.
        if !suffix.is_empty() {
            let suffix_area = Rect {
                x: area.x + bar_width,
                y: area.y,
                width: suffix_width,
                height: 1,
            };
            Paragraph::new(suffix)
                .style(self.label_style)
                .render(suffix_area, buf);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    // -----------------------------------------------------------------
    // Helper: render the widget into a fresh buffer and return it.
    // -----------------------------------------------------------------

    fn render_to_buffer(bar: ProgressBar, width: u16, height: u16) -> Buffer {
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        bar.render(area, &mut buf);
        buf
    }

    // -----------------------------------------------------------------
    // Construction / defaults
    // -----------------------------------------------------------------

    #[test]
    fn new_sets_defaults() {
        let bar = ProgressBar::new(5, 10);
        assert_eq!(bar.current, 5);
        assert_eq!(bar.total, 10);
        assert!(bar.label.is_none());
        assert!(!bar.show_percentage);
        assert_eq!(bar.bar_style, Style::default().fg(Color::Cyan));
        assert_eq!(bar.label_style, Style::default().fg(Color::White));
    }

    #[test]
    fn builder_methods_set_fields() {
        let bar = ProgressBar::new(1, 2).with_label("hello").with_percentage();
        assert_eq!(bar.label.as_deref(), Some("hello"));
        assert!(bar.show_percentage);
    }

    // -----------------------------------------------------------------
    // Full bar rendering (Widget impl)
    // -----------------------------------------------------------------

    #[test]
    fn full_bar_renders_all_filled() {
        let bar = ProgressBar::new(10, 10);
        let buf = render_to_buffer(bar, 20, 1);

        let line: String = (0..20)
            .map(|x| {
                buf.cell((x, 0))
                    .unwrap()
                    .symbol()
                    .chars()
                    .next()
                    .unwrap_or(' ')
            })
            .collect();
        // Every cell should be the filled character.
        assert!(
            line.chars().all(|c| c == PROGRESS_FILLED),
            "Expected all filled, got: {:?}",
            line,
        );
    }

    #[test]
    fn empty_bar_renders_all_empty() {
        let bar = ProgressBar::new(0, 10);
        let buf = render_to_buffer(bar, 20, 1);

        let line: String = (0..20)
            .map(|x| {
                buf.cell((x, 0))
                    .unwrap()
                    .symbol()
                    .chars()
                    .next()
                    .unwrap_or(' ')
            })
            .collect();
        assert!(
            line.chars().all(|c| c == PROGRESS_EMPTY),
            "Expected all empty, got: {:?}",
            line,
        );
    }

    #[test]
    fn half_bar_renders_mixed() {
        let bar = ProgressBar::new(5, 10);
        let buf = render_to_buffer(bar, 20, 1);

        let filled_count = (0..20)
            .filter(|&x| {
                buf.cell((x, 0))
                    .unwrap()
                    .symbol()
                    .chars()
                    .next()
                    .unwrap_or(' ')
                    == PROGRESS_FILLED
            })
            .count();
        // 50% of 20 = 10 filled
        assert_eq!(filled_count, 10);
    }

    #[test]
    fn renders_with_label() {
        let bar = ProgressBar::new(5, 10).with_label("OK");
        let buf = render_to_buffer(bar, 30, 1);

        // Collect the full rendered line.
        let line: String = (0..30)
            .map(|x| {
                buf.cell((x, 0))
                    .unwrap()
                    .symbol()
                    .chars()
                    .next()
                    .unwrap_or(' ')
            })
            .collect();
        assert!(line.contains("OK"), "Label not found in: {:?}", line);
    }

    #[test]
    fn renders_with_percentage() {
        let bar = ProgressBar::new(3, 10).with_percentage();
        let buf = render_to_buffer(bar, 30, 1);

        let line: String = (0..30)
            .map(|x| {
                buf.cell((x, 0))
                    .unwrap()
                    .symbol()
                    .chars()
                    .next()
                    .unwrap_or(' ')
            })
            .collect();
        assert!(line.contains("30%"), "Percentage not found in: {:?}", line);
    }

    // -----------------------------------------------------------------
    // Zero-total edge case
    // -----------------------------------------------------------------

    #[test]
    fn zero_total_renders_empty_bar() {
        let bar = ProgressBar::new(5, 0);
        let buf = render_to_buffer(bar, 20, 1);

        let line: String = (0..20)
            .map(|x| {
                buf.cell((x, 0))
                    .unwrap()
                    .symbol()
                    .chars()
                    .next()
                    .unwrap_or(' ')
            })
            .collect();
        assert!(
            line.chars().all(|c| c == PROGRESS_EMPTY),
            "Expected all empty for zero total, got: {:?}",
            line,
        );
    }

    #[test]
    fn zero_total_with_percentage_shows_zero() {
        let compact = ProgressBar::new(0, 0)
            .with_percentage()
            .to_string_compact(10);
        assert!(compact.contains("0%"), "Expected 0%% in: {:?}", compact);
    }

    // -----------------------------------------------------------------
    // to_string_compact
    // -----------------------------------------------------------------

    #[test]
    fn compact_bar_only() {
        let s = ProgressBar::new(10, 10).to_string_compact(10);
        assert_eq!(s.chars().count(), 10);
        assert!(s.chars().all(|c| c == PROGRESS_FILLED));
    }

    #[test]
    fn compact_with_label() {
        let s = ProgressBar::new(5, 10)
            .with_label("building")
            .to_string_compact(10);
        assert!(s.contains("building"), "Label not in compact: {:?}", s);
        // Bar should be exactly 10 chars, then " building"
        assert!(s.starts_with(&std::iter::repeat_n(PROGRESS_FILLED, 5).collect::<String>()));
    }

    #[test]
    fn compact_with_percentage() {
        let s = ProgressBar::new(3, 10)
            .with_percentage()
            .to_string_compact(20);
        assert!(s.contains("30%"), "Percentage not in compact: {:?}", s);
        // Bar portion should be exactly 20 characters wide.
        let bar_part: String = s.chars().take(20).collect();
        assert_eq!(bar_part.chars().count(), 20);
    }

    #[test]
    fn compact_with_label_and_percentage() {
        let s = ProgressBar::new(10, 10)
            .with_label("done")
            .with_percentage()
            .to_string_compact(10);
        assert!(s.contains("done"), "Label not found: {:?}", s);
        assert!(s.contains("100%"), "Percentage not found: {:?}", s);
    }

    #[test]
    fn compact_zero_width_produces_suffix_only() {
        let s = ProgressBar::new(5, 10)
            .with_label("hi")
            .with_percentage()
            .to_string_compact(0);
        // Zero-width bar means the string is just the suffix.
        assert!(s.contains("hi"));
        assert!(s.contains("50%"));
    }

    // -----------------------------------------------------------------
    // Label-only fallback for tiny areas
    // -----------------------------------------------------------------

    #[test]
    fn tiny_area_falls_back_to_label_text() {
        // With a label that occupies most of the width, the bar portion
        // will be < 5 columns, triggering the fallback.
        let bar = ProgressBar::new(5, 10).with_label("Building image step 3 of 10");
        // Area only 10 wide -- the suffix is 28 chars, so bar_width = 0 < 5.
        let buf = render_to_buffer(bar, 10, 1);

        let line: String = (0..10)
            .map(|x| {
                buf.cell((x, 0))
                    .unwrap()
                    .symbol()
                    .chars()
                    .next()
                    .unwrap_or(' ')
            })
            .collect();
        // Should contain the beginning of the label, not bar characters.
        assert!(
            line.starts_with("Building"),
            "Expected label fallback, got: {:?}",
            line,
        );
    }

    #[test]
    fn percentage_fallback_on_tiny_area() {
        let bar = ProgressBar::new(5, 10).with_percentage();
        // Width 6: suffix " 50%" is 4 chars, bar_width = 2 which is < 5.
        let buf = render_to_buffer(bar, 6, 1);

        let line: String = (0..6)
            .map(|x| {
                buf.cell((x, 0))
                    .unwrap()
                    .symbol()
                    .chars()
                    .next()
                    .unwrap_or(' ')
            })
            .collect();
        assert!(
            line.contains("50%"),
            "Expected percentage fallback, got: {:?}",
            line,
        );
    }

    #[test]
    fn zero_height_renders_nothing() {
        let bar = ProgressBar::new(5, 10);
        // Zero height -- render should bail immediately.
        let area = Rect::new(0, 0, 20, 0);
        let mut buf = Buffer::empty(area);
        bar.render(area, &mut buf);
        // No panic is the success criterion; buffer is empty.
    }

    #[test]
    fn very_narrow_area_renders_nothing() {
        let bar = ProgressBar::new(5, 10);
        // Width 2 with no label/percentage -- bar_width < 5 and no suffix to
        // fall back to, so it renders an empty fallback string.
        let buf = render_to_buffer(bar, 2, 1);

        let line: String = (0..2)
            .map(|x| {
                buf.cell((x, 0))
                    .unwrap()
                    .symbol()
                    .chars()
                    .next()
                    .unwrap_or(' ')
            })
            .collect();
        // Should not contain bar characters.
        assert!(
            !line.contains(PROGRESS_FILLED),
            "Did not expect bar chars in narrow area: {:?}",
            line,
        );
    }

    // -----------------------------------------------------------------
    // Ratio clamping
    // -----------------------------------------------------------------

    #[test]
    fn current_exceeding_total_clamps_to_full() {
        let bar = ProgressBar::new(999, 10);
        let buf = render_to_buffer(bar, 20, 1);

        let line: String = (0..20)
            .map(|x| {
                buf.cell((x, 0))
                    .unwrap()
                    .symbol()
                    .chars()
                    .next()
                    .unwrap_or(' ')
            })
            .collect();
        assert!(
            line.chars().all(|c| c == PROGRESS_FILLED),
            "Expected fully filled when current > total, got: {:?}",
            line,
        );
    }

    #[test]
    fn compact_current_exceeding_total_shows_100_percent() {
        let s = ProgressBar::new(999, 10)
            .with_percentage()
            .to_string_compact(10);
        assert!(s.contains("100%"), "Expected 100%% in: {:?}", s);
    }
}
