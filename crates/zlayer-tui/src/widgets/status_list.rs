//! Generic status list widget with vertical and horizontal orientations.
//!
//! Unifies the builder's `InstructionList` (vertical, with scroll and
//! `[cached]` tags) and the CLI's step indicator (horizontal, with
//! `" -> "` separators) into a single generic component.

use ratatui::prelude::*;

use crate::icons;
use crate::palette::color;

// ---------------------------------------------------------------------------
// StatusItem trait
// ---------------------------------------------------------------------------

/// Trait for items that can be displayed inside a [`StatusList`].
pub trait StatusItem {
    /// The label text for this item.
    fn label(&self) -> &str;

    /// The current status of this item.
    fn status(&self) -> ItemStatus;

    /// An optional styled tag rendered after the label (e.g., `[cached]`).
    fn tag(&self) -> Option<(&str, Style)> {
        None
    }
}

/// Lifecycle status of a single list item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemStatus {
    Pending,
    Active,
    Complete,
    Failed,
}

/// Layout direction for the status list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    Vertical,
    Horizontal,
}

// ---------------------------------------------------------------------------
// StatusList widget
// ---------------------------------------------------------------------------

/// A list of [`StatusItem`]s rendered either vertically (scrollable, with
/// a count indicator) or horizontally (with `" -> "` separators).
pub struct StatusList<'a, T: StatusItem> {
    /// Items to display.
    pub items: &'a [T],
    /// Index of the current / active item (used for centering in vertical
    /// mode, and for styling in both modes).
    pub current: usize,
    /// Layout direction.
    pub orientation: Orientation,
}

impl<'a, T: StatusItem> StatusList<'a, T> {
    /// Create a new vertical status list.
    pub fn new(items: &'a [T], current: usize) -> Self {
        Self {
            items,
            current,
            orientation: Orientation::Vertical,
        }
    }

    /// Create a new horizontal status list.
    pub fn horizontal(items: &'a [T], current: usize) -> Self {
        Self {
            items,
            current,
            orientation: Orientation::Horizontal,
        }
    }
}

impl<T: StatusItem> Widget for StatusList<'_, T> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 || self.items.is_empty() {
            return;
        }

        match self.orientation {
            Orientation::Vertical => render_vertical(self.items, self.current, area, buf),
            Orientation::Horizontal => render_horizontal(self.items, self.current, area, buf),
        }
    }
}

// ---------------------------------------------------------------------------
// Vertical rendering
// ---------------------------------------------------------------------------

fn render_vertical<T: StatusItem>(items: &[T], current: usize, area: Rect, buf: &mut Buffer) {
    let visible_count = area.height as usize;
    let total = items.len();

    // Calculate window centered on `current`
    let (start, end) = if total <= visible_count {
        (0, total)
    } else {
        let half = visible_count / 2;
        let start = if current < half {
            0
        } else if current + half >= total {
            total.saturating_sub(visible_count)
        } else {
            current.saturating_sub(half)
        };
        let end = (start + visible_count).min(total);
        (start, end)
    };

    for (display_idx, idx) in (start..end).enumerate() {
        if display_idx >= visible_count {
            break;
        }

        let item = &items[idx];
        let y = area.y + display_idx as u16;
        let status = item.status();

        // Status icon via crate::icons
        let (icon_ch, icon_style) = status_to_icon(status);
        buf.set_string(area.x, y, icon_ch.to_string(), icon_style);

        // Label text
        let text_style = status_text_style(status);
        let available_width = area.width.saturating_sub(2) as usize; // icon + space
        let label = item.label();
        let display_label = if label.len() > available_width {
            if available_width >= 4 {
                format!("{}...", &label[..available_width.saturating_sub(3)])
            } else {
                label[..available_width].to_string()
            }
        } else {
            label.to_string()
        };

        buf.set_string(area.x + 2, y, &display_label, text_style);

        // Optional tag after the label
        if let Some((tag_text, tag_style)) = item.tag() {
            let label_end = area.x + 2 + display_label.len() as u16;
            let tag_x = label_end.min(area.x + area.width.saturating_sub(tag_text.len() as u16));

            if tag_x + tag_text.len() as u16 <= area.x + area.width {
                buf.set_string(tag_x, y, tag_text, tag_style);
            }
        }
    }

    // Scroll indicator when items overflow
    if total > visible_count {
        let indicator = format!("({}/{})", end.min(total), total);
        let x = area.x + area.width.saturating_sub(indicator.len() as u16);
        if x >= area.x {
            buf.set_string(x, area.y, &indicator, Style::default().fg(color::INACTIVE));
        }
    }
}

// ---------------------------------------------------------------------------
// Horizontal rendering
// ---------------------------------------------------------------------------

fn render_horizontal<T: StatusItem>(items: &[T], current: usize, area: Rect, buf: &mut Buffer) {
    let separator = " -> ";
    let mut x = area.x;

    for (i, item) in items.iter().enumerate() {
        if x >= area.x + area.width {
            break;
        }

        let status = item.status();

        // Icon
        let (icon_ch, icon_style) = horizontal_icon(status, i, current);
        let icon_str = format!("{} ", icon_ch);
        let icon_len = icon_str.len() as u16;

        if x + icon_len > area.x + area.width {
            break;
        }
        buf.set_string(x, area.y, &icon_str, icon_style);
        x += icon_len;

        // Label
        let label = item.label();
        let label_style = horizontal_label_style(status, i, current);
        let remaining = (area.x + area.width).saturating_sub(x) as usize;
        let display_label = if label.len() > remaining {
            if remaining >= 4 {
                format!("{}...", &label[..remaining.saturating_sub(3)])
            } else {
                label[..remaining].to_string()
            }
        } else {
            label.to_string()
        };

        buf.set_string(x, area.y, &display_label, label_style);
        x += display_label.len() as u16;

        // Separator between items (not after the last one)
        if i < items.len() - 1 {
            let sep_len = separator.len() as u16;
            if x + sep_len <= area.x + area.width {
                buf.set_string(x, area.y, separator, Style::default().fg(color::INACTIVE));
                x += sep_len;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Map an [`ItemStatus`] to an icon + style using [`crate::icons::status_icon`].
fn status_to_icon(status: ItemStatus) -> (char, Style) {
    match status {
        ItemStatus::Pending => icons::status_icon(false, false, false),
        ItemStatus::Active => icons::status_icon(false, true, false),
        ItemStatus::Complete => icons::status_icon(true, false, false),
        ItemStatus::Failed => icons::status_icon(false, false, true),
    }
}

/// Text style for a label in vertical mode based on its status.
fn status_text_style(status: ItemStatus) -> Style {
    match status {
        ItemStatus::Pending => Style::default().fg(color::INACTIVE),
        ItemStatus::Active => Style::default()
            .fg(color::WARNING)
            .add_modifier(Modifier::BOLD),
        ItemStatus::Complete => Style::default().fg(color::TEXT),
        ItemStatus::Failed => Style::default().fg(color::ERROR),
    }
}

/// Icon for horizontal orientation, where completed items get green, active
/// gets cyan+bold, and pending gets dark gray.
fn horizontal_icon(status: ItemStatus, idx: usize, current: usize) -> (char, Style) {
    if idx < current || status == ItemStatus::Complete {
        (icons::COMPLETE, Style::default().fg(color::SUCCESS))
    } else if idx == current || status == ItemStatus::Active {
        (
            icons::RUNNING,
            Style::default()
                .fg(color::ACCENT)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        (icons::PENDING, Style::default().fg(color::INACTIVE))
    }
}

/// Label style for horizontal orientation.
fn horizontal_label_style(status: ItemStatus, idx: usize, current: usize) -> Style {
    if idx < current || status == ItemStatus::Complete {
        Style::default().fg(color::SUCCESS)
    } else if idx == current || status == ItemStatus::Active {
        Style::default()
            .fg(color::ACCENT)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(color::INACTIVE)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // A simple test item
    #[derive(Debug)]
    struct Step {
        label: String,
        status: ItemStatus,
        tag: Option<(String, Style)>,
    }

    impl Step {
        fn new(label: &str, status: ItemStatus) -> Self {
            Self {
                label: label.to_string(),
                status,
                tag: None,
            }
        }

        fn with_tag(mut self, tag: &str, style: Style) -> Self {
            self.tag = Some((tag.to_string(), style));
            self
        }
    }

    impl StatusItem for Step {
        fn label(&self) -> &str {
            &self.label
        }

        fn status(&self) -> ItemStatus {
            self.status
        }

        fn tag(&self) -> Option<(&str, Style)> {
            self.tag.as_ref().map(|(s, st)| (s.as_str(), *st))
        }
    }

    fn create_buffer(width: u16, height: u16) -> Buffer {
        Buffer::empty(Rect::new(0, 0, width, height))
    }

    fn buffer_text(buf: &Buffer) -> String {
        buf.content().iter().map(|c| c.symbol()).collect()
    }

    // -- Empty state --

    #[test]
    fn empty_items_does_not_panic() {
        let mut buf = create_buffer(40, 5);
        let area = Rect::new(0, 0, 40, 5);

        let items: Vec<Step> = vec![];
        let list = StatusList::new(&items, 0);
        list.render(area, &mut buf);
    }

    #[test]
    fn empty_horizontal_does_not_panic() {
        let mut buf = create_buffer(80, 1);
        let area = Rect::new(0, 0, 80, 1);

        let items: Vec<Step> = vec![];
        let list = StatusList::horizontal(&items, 0);
        list.render(area, &mut buf);
    }

    // -- Vertical mode --

    #[test]
    fn vertical_renders_labels() {
        let mut buf = create_buffer(60, 5);
        let area = Rect::new(0, 0, 60, 5);

        let items = vec![
            Step::new("WORKDIR /app", ItemStatus::Complete),
            Step::new("RUN npm ci", ItemStatus::Active),
            Step::new("COPY . .", ItemStatus::Pending),
        ];

        let list = StatusList::new(&items, 1);
        list.render(area, &mut buf);

        let text = buffer_text(&buf);
        assert!(text.contains("WORKDIR"));
        assert!(text.contains("RUN npm ci"));
        assert!(text.contains("COPY"));
    }

    #[test]
    fn vertical_renders_icons() {
        let mut buf = create_buffer(60, 5);
        let area = Rect::new(0, 0, 60, 5);

        let items = vec![
            Step::new("done", ItemStatus::Complete),
            Step::new("active", ItemStatus::Active),
            Step::new("waiting", ItemStatus::Pending),
            Step::new("failed", ItemStatus::Failed),
        ];

        let list = StatusList::new(&items, 1);
        list.render(area, &mut buf);

        let text = buffer_text(&buf);
        // Check that icons from crate::icons are present
        assert!(text.contains(icons::COMPLETE));
        assert!(text.contains(icons::RUNNING));
        assert!(text.contains(icons::PENDING));
        assert!(text.contains(icons::FAILED));
    }

    #[test]
    fn vertical_tag_rendered() {
        let mut buf = create_buffer(60, 3);
        let area = Rect::new(0, 0, 60, 3);

        let items = vec![Step::new("COPY package.json ./", ItemStatus::Complete)
            .with_tag(" [cached]", Style::default().fg(Color::Cyan))];

        let list = StatusList::new(&items, 0);
        list.render(area, &mut buf);

        let text = buffer_text(&buf);
        assert!(text.contains("[cached]"));
    }

    #[test]
    fn vertical_scroll_indicator() {
        let mut buf = create_buffer(40, 3);
        let area = Rect::new(0, 0, 40, 3);

        let items: Vec<Step> = (0..10)
            .map(|i| Step::new(&format!("Step {}", i), ItemStatus::Pending))
            .collect();

        let list = StatusList::new(&items, 5);
        list.render(area, &mut buf);

        let text = buffer_text(&buf);
        // Should show something like "(8/10)"
        assert!(text.contains('/'));
        assert!(text.contains('('));
    }

    #[test]
    fn vertical_no_scroll_when_all_visible() {
        let mut buf = create_buffer(60, 10);
        let area = Rect::new(0, 0, 60, 10);

        let items = vec![
            Step::new("one", ItemStatus::Complete),
            Step::new("two", ItemStatus::Active),
        ];

        let list = StatusList::new(&items, 1);
        list.render(area, &mut buf);

        let text = buffer_text(&buf);
        // No scroll indicator because all items fit
        assert!(!text.contains("(/"));
    }

    #[test]
    fn vertical_truncates_long_labels() {
        let mut buf = create_buffer(15, 2);
        let area = Rect::new(0, 0, 15, 2);

        let items = vec![Step::new(
            "A very long instruction label that exceeds width",
            ItemStatus::Active,
        )];

        let list = StatusList::new(&items, 0);
        list.render(area, &mut buf);

        let text = buffer_text(&buf);
        assert!(text.contains("..."));
    }

    #[test]
    fn vertical_centering_at_start() {
        let mut buf = create_buffer(40, 3);
        let area = Rect::new(0, 0, 40, 3);

        let items: Vec<Step> = (0..10)
            .map(|i| Step::new(&format!("Step {}", i), ItemStatus::Pending))
            .collect();

        // Current = 0: window should start at 0
        let list = StatusList::new(&items, 0);
        list.render(area, &mut buf);

        let text = buffer_text(&buf);
        assert!(text.contains("Step 0"));
    }

    #[test]
    fn vertical_centering_at_end() {
        let mut buf = create_buffer(40, 3);
        let area = Rect::new(0, 0, 40, 3);

        let items: Vec<Step> = (0..10)
            .map(|i| Step::new(&format!("Step {}", i), ItemStatus::Pending))
            .collect();

        // Current = 9 (last): window should show the last 3 items
        let list = StatusList::new(&items, 9);
        list.render(area, &mut buf);

        let text = buffer_text(&buf);
        assert!(text.contains("Step 9"));
    }

    // -- Horizontal mode --

    #[test]
    fn horizontal_renders_items_with_separators() {
        let mut buf = create_buffer(80, 1);
        let area = Rect::new(0, 0, 80, 1);

        let items = vec![
            Step::new("Source", ItemStatus::Complete),
            Step::new("Configure", ItemStatus::Active),
            Step::new("Build", ItemStatus::Pending),
        ];

        let list = StatusList::horizontal(&items, 1);
        list.render(area, &mut buf);

        let text = buffer_text(&buf);
        assert!(text.contains("Source"));
        assert!(text.contains("Configure"));
        assert!(text.contains("Build"));
        assert!(text.contains("->"));
    }

    #[test]
    fn horizontal_completed_items_use_checkmark() {
        let mut buf = create_buffer(80, 1);
        let area = Rect::new(0, 0, 80, 1);

        let items = vec![
            Step::new("Done", ItemStatus::Complete),
            Step::new("Active", ItemStatus::Active),
            Step::new("Waiting", ItemStatus::Pending),
        ];

        let list = StatusList::horizontal(&items, 1);
        list.render(area, &mut buf);

        let text = buffer_text(&buf);
        assert!(text.contains(icons::COMPLETE));
        assert!(text.contains(icons::RUNNING));
        assert!(text.contains(icons::PENDING));
    }

    #[test]
    fn horizontal_narrow_area_truncates() {
        let mut buf = create_buffer(20, 1);
        let area = Rect::new(0, 0, 20, 1);

        let items = vec![
            Step::new("VeryLongStepName", ItemStatus::Complete),
            Step::new("AnotherLongOne", ItemStatus::Active),
        ];

        let list = StatusList::horizontal(&items, 1);
        list.render(area, &mut buf);

        // Should not panic, even if text is cut off
    }

    // -- Orientation constructors --

    #[test]
    fn new_creates_vertical() {
        let items = vec![Step::new("a", ItemStatus::Pending)];
        let list = StatusList::new(&items, 0);
        assert_eq!(list.orientation, Orientation::Vertical);
    }

    #[test]
    fn horizontal_constructor() {
        let items = vec![Step::new("a", ItemStatus::Pending)];
        let list = StatusList::horizontal(&items, 0);
        assert_eq!(list.orientation, Orientation::Horizontal);
    }

    // -- Edge cases --

    #[test]
    fn zero_height_does_not_panic() {
        let mut buf = create_buffer(40, 0);
        let area = Rect::new(0, 0, 40, 0);

        let items = vec![Step::new("x", ItemStatus::Active)];
        let list = StatusList::new(&items, 0);
        list.render(area, &mut buf);
    }

    #[test]
    fn zero_width_does_not_panic() {
        let mut buf = create_buffer(0, 5);
        let area = Rect::new(0, 0, 0, 5);

        let items = vec![Step::new("x", ItemStatus::Active)];
        let list = StatusList::new(&items, 0);
        list.render(area, &mut buf);
    }

    #[test]
    fn current_out_of_bounds_does_not_panic() {
        let mut buf = create_buffer(40, 5);
        let area = Rect::new(0, 0, 40, 5);

        let items = vec![Step::new("only", ItemStatus::Active)];
        // current = 99 is way past the end
        let list = StatusList::new(&items, 99);
        list.render(area, &mut buf);

        let text = buffer_text(&buf);
        assert!(text.contains("only"));
    }

    #[test]
    fn item_status_equality() {
        assert_eq!(ItemStatus::Pending, ItemStatus::Pending);
        assert_ne!(ItemStatus::Pending, ItemStatus::Active);
        assert_ne!(ItemStatus::Complete, ItemStatus::Failed);
    }
}
