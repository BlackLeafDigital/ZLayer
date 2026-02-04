# zlayer-cli

Interactive terminal UI for building container images. Provides the same capabilities as `zlayer-build` (Dockerfile, ZImagefile, runtime template builds) but through a menu-driven TUI interface.

## Features

- **Build wizard** -- Step through context selection, file detection, tag naming, and build arguments interactively
- **Runtime browser** -- Browse available runtime templates with descriptions and auto-detect file hints
- **Validation view** -- Validate Dockerfiles and ZImagefiles with inline error display
- **Build progress** -- Real-time TUI progress with stage tracking, layer status, and timing
- **Keyboard-driven** -- Full keyboard navigation with vi-style bindings

## Screenshot

```
 ┌─────────────────────────────────────────────────┐
 │              zlayer-cli  v0.9.0                  │
 │                                                  │
 │   [1] Build an image                             │
 │   [2] Browse runtime templates                   │
 │   [3] Validate a Dockerfile / ZImagefile         │
 │   [q] Quit                                       │
 │                                                  │
 └─────────────────────────────────────────────────┘
```

## Usage

```bash
# Launch the interactive TUI
zlayer-cli

# The TUI guides you through:
#   1. Selecting a build context directory
#   2. Choosing a build file (Dockerfile, ZImagefile, or runtime template)
#   3. Setting tags and build arguments
#   4. Watching the build progress in real time
```

## Keyboard Shortcuts

| Key | Action |
|-----|--------|
| `1`-`3` | Select menu item |
| `Enter` | Confirm selection |
| `Esc` | Go back / cancel |
| `q` | Quit |
| `j` / `Down` | Move selection down |
| `k` / `Up` | Move selection up |
| `Tab` | Next input field |
| `Shift+Tab` | Previous input field |

## Dependencies

Like `zlayer-build`, this binary depends only on `zlayer-builder` and `zlayer-spec`, plus `ratatui` for the terminal UI. It does not require the full ZLayer runtime.

## Building from Source

```bash
cargo build --release -p zlayer-cli
```

The binary is output to `target/release/zlayer-cli`.

## License

Apache-2.0
