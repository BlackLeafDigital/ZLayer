# zlayer-tui

Shared TUI widgets, color palette, and terminal utilities for ZLayer.

## Overview

`zlayer-tui` is the internal building-block library that all ZLayer terminal
interfaces share. It is **not** itself a TUI application — it exposes
reusable `ratatui` widgets and `crossterm`-based terminal helpers that the
ZLayer CLI (`bin/zlayer`) and the image builder (`crates/zlayer-builder`)
compose into their own dashboards. The interactive top-level dashboard
launched by running `zlayer` with no subcommand (or `zlayer tui`) is built
on the modules in this crate.

The library is intentionally small: it unifies code that previously diverged
between the builder, the deploy view, and the main CLI (progress bars, log
panes, instruction lists, terminal setup/teardown) into a single source of
truth.

## Modules

- `zlayer_tui::palette` — semantic ratatui `Color` constants
  (`color::SUCCESS`, `ERROR`, `WARNING`, `ACCENT`, `ACTIVE_BORDER`,
  `INACTIVE`, `TEXT`, `SCROLL_BADGE_FG`, `SCROLL_BADGE_BG`) and matching
  ANSI escape codes (`ansi::GREEN`, `RED`, `YELLOW`, `CYAN`, `DIM`, `BOLD`,
  `RESET`) for non-ratatui loggers.
- `zlayer_tui::icons` — Unicode status glyphs (`PENDING`, `RUNNING`,
  `COMPLETE`, `FAILED`, `STOPPING`, `STOPPED`, `PROGRESS_FILLED`,
  `PROGRESS_EMPTY`, arrows) plus the `SPINNER_FRAMES` table and the
  `status_icon(done, running, failed)` helper that returns a styled glyph.
- `zlayer_tui::terminal` — `setup_terminal()`, `restore_terminal()`,
  `install_panic_hook()`, and the `POLL_DURATION` (50 ms / ~20 FPS) used
  for event polling. Handles raw mode and the alternate screen via
  crossterm.
- `zlayer_tui::logger` — `detect_color_support()` (honours `NO_COLOR`,
  `FORCE_COLOR`, `CI`, and `TERM=dumb`) and `colorize(text, code, enabled)`
  for non-ratatui plain-text loggers.
- `zlayer_tui::widgets::progress_bar::ProgressBar` — configurable bar with
  optional trailing label and/or percentage; also has
  `to_string_compact(width)` for embedding in table cells.
- `zlayer_tui::widgets::scrollable_pane::{ScrollablePane, PaneEntry}` —
  bordered, scrollable list with empty-state placeholder and a scroll
  percentage badge. Implement `PaneEntry` on your row type to plug in.
- `zlayer_tui::widgets::status_list::{StatusList, StatusItem, ItemStatus, Orientation}`
  — vertical (scrollable, with optional `[tag]`) or horizontal
  (`" -> "`-separated) list of pipeline-style steps.

## Use from ZLayer

- `bin/zlayer` — the top-level interactive dashboard (`zlayer` /
  `zlayer tui`) and several subcommand views (`deploy`, `pipeline`, etc.)
  use these widgets and call `terminal::setup_terminal` /
  `terminal::install_panic_hook` so a panic mid-render still restores the
  terminal.
- `crates/zlayer-builder` — the build TUI uses the progress bar, status
  list, and scrollable log pane to render multi-step image builds.

This crate is published as part of the ZLayer workspace but is intended as
an internal helper; its surface may change without notice.

## Platform notes

`crossterm` and `ratatui` work uniformly on Linux, macOS, and Windows
(tested under both native consoles and modern terminal emulators).
`detect_color_support` reads environment variables only — it does not call
`isatty`, so colour decisions are explicit and reproducible inside CI
runners.

## When to edit this crate

Add a new widget here only if at least two ZLayer TUIs need it; one-off
screens belong in their owning crate. When changing the public surface,
audit both `bin/zlayer` (the dashboard) and `crates/zlayer-builder` (the
build TUI) — they are the only first-party consumers and both are part of
the same Cargo workspace.

Repository: <https://github.com/BlackLeafDigital/ZLayer>
