//! Simple file/directory browser widget
//!
//! Provides a navigable filesystem view for selecting Dockerfiles,
//! ZImagefiles, or directories.

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};

/// State for the file picker
pub struct FilePickerState {
    /// Current directory being displayed
    pub current_dir: PathBuf,
    /// Entries in the current directory (directories first, then files)
    pub entries: Vec<FileEntry>,
    /// Currently selected index
    pub selected: usize,
    /// Scroll offset for long lists
    pub scroll_offset: usize,
}

/// A single entry in the file picker
#[derive(Clone, Debug)]
pub struct FileEntry {
    /// Display name
    pub name: String,
    /// Full path
    pub path: PathBuf,
    /// Whether this is a directory
    pub is_dir: bool,
}

impl FilePickerState {
    /// Create a new file picker rooted at the given directory
    pub fn new(dir: PathBuf) -> Self {
        let mut state = Self {
            current_dir: dir,
            entries: Vec::new(),
            selected: 0,
            scroll_offset: 0,
        };
        state.refresh();
        state
    }

    /// Refresh the entry list from the filesystem
    pub fn refresh(&mut self) {
        self.entries.clear();
        self.selected = 0;
        self.scroll_offset = 0;

        // Add parent directory entry (unless we are at root)
        if let Some(parent) = self.current_dir.parent() {
            self.entries.push(FileEntry {
                name: "..".to_string(),
                path: parent.to_path_buf(),
                is_dir: true,
            });
        }

        // Read directory contents
        if let Ok(read_dir) = std::fs::read_dir(&self.current_dir) {
            let mut dirs = Vec::new();
            let mut files = Vec::new();

            for entry in read_dir.flatten() {
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().to_string();

                // Skip hidden files/dirs
                if name.starts_with('.') {
                    continue;
                }

                if path.is_dir() {
                    dirs.push(FileEntry {
                        name: format!("{}/", name),
                        path,
                        is_dir: true,
                    });
                } else {
                    files.push(FileEntry {
                        name,
                        path,
                        is_dir: false,
                    });
                }
            }

            // Sort alphabetically
            dirs.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
            files.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

            self.entries.extend(dirs);
            self.entries.extend(files);
        }
    }

    /// Move selection up
    pub fn move_up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
        self.adjust_scroll();
    }

    /// Move selection down
    pub fn move_down(&mut self) {
        if !self.entries.is_empty() && self.selected < self.entries.len() - 1 {
            self.selected += 1;
            self.adjust_scroll();
        }
    }

    /// Enter the selected directory or return the selected file path
    pub fn select(&mut self) -> Option<PathBuf> {
        if let Some(entry) = self.entries.get(self.selected) {
            if entry.is_dir {
                self.current_dir = entry.path.clone();
                self.refresh();
                None
            } else {
                Some(entry.path.clone())
            }
        } else {
            None
        }
    }

    /// Get the currently selected entry
    #[allow(dead_code)]
    pub fn selected_entry(&self) -> Option<&FileEntry> {
        self.entries.get(self.selected)
    }

    /// Adjust scroll offset to keep selection visible
    fn adjust_scroll(&mut self) {
        // Keep a margin of 2 lines if possible
        if self.selected < self.scroll_offset {
            self.scroll_offset = self.selected;
        }
    }

    /// Handle a key event, returning Some(path) if a file was selected
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<PathBuf> {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_up();
                None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_down();
                None
            }
            KeyCode::Enter => self.select(),
            KeyCode::Backspace => {
                // Go to parent directory
                if let Some(parent) = self.current_dir.parent() {
                    self.current_dir = parent.to_path_buf();
                    self.refresh();
                }
                None
            }
            _ => None,
        }
    }
}

/// Render the file picker into the given area
pub fn render(area: Rect, buf: &mut Buffer, state: &FilePickerState, title: &str) {
    let block = Block::default()
        .title(format!(" {} ", title))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Blue));

    let inner = block.inner(area);
    block.render(area, buf);

    if inner.height < 2 {
        return;
    }

    // Show current directory path at the top
    let path_display = state.current_dir.display().to_string();
    let path_line = Paragraph::new(path_display).style(
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::ITALIC),
    );
    let path_area = Rect {
        x: inner.x,
        y: inner.y,
        width: inner.width,
        height: 1,
    };
    path_line.render(path_area, buf);

    // Render entry list below the path
    let list_area = Rect {
        x: inner.x,
        y: inner.y + 1,
        width: inner.width,
        height: inner.height.saturating_sub(1),
    };

    if state.entries.is_empty() {
        Paragraph::new("(empty directory)")
            .style(Style::default().fg(Color::DarkGray))
            .render(list_area, buf);
        return;
    }

    let visible_count = list_area.height as usize;
    let start = state.scroll_offset;
    let end = (start + visible_count).min(state.entries.len());

    let items: Vec<ListItem> = state.entries[start..end]
        .iter()
        .enumerate()
        .map(|(display_idx, entry)| {
            let actual_idx = start + display_idx;
            let style = if actual_idx == state.selected {
                Style::default()
                    .fg(Color::White)
                    .bg(Color::Blue)
                    .add_modifier(Modifier::BOLD)
            } else if entry.is_dir {
                Style::default().fg(Color::Cyan)
            } else if is_buildfile(&entry.name) {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::White)
            };

            let prefix = if entry.is_dir { "[d] " } else { "    " };
            ListItem::new(format!("{}{}", prefix, entry.name)).style(style)
        })
        .collect();

    let list = List::new(items);
    Widget::render(list, list_area, buf);
}

/// Check if a filename looks like a Dockerfile or ZImagefile
fn is_buildfile(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.contains("dockerfile")
        || lower.contains("zimagefile")
        || lower.contains("containerfile")
        || lower.ends_with(".yml")
        || lower.ends_with(".yaml")
}
