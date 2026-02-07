//! Shared TUI widgets, color palette, and terminal utilities for ZLayer
//!
//! This crate provides reusable building blocks for all ZLayer TUI applications:
//! - **`palette`** - Semantic color constants (ratatui styles + ANSI escape codes)
//! - **`icons`** - Unicode status icons and spinner animation frames
//! - **`terminal`** - Terminal setup/teardown and panic hook utilities
//! - **`logger`** - Color detection and ANSI formatting for plain-text loggers
//! - **`widgets`** - Reusable ratatui widgets (progress bar, scrollable pane, status list)

pub mod icons;
pub mod logger;
pub mod palette;
pub mod terminal;
pub mod widgets;
