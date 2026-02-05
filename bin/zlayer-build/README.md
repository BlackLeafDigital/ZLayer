# zlayer-build

Lightweight container image builder CLI (~10MB). Builds images from Dockerfiles, ZImagefiles, or runtime templates using buildah -- without pulling in the full ZLayer orchestration runtime.

## Dependencies

`zlayer-build` depends on only two internal crates:

| Crate | Purpose |
|-------|---------|
| `zlayer-builder` | Dockerfile/ZImagefile parsing, buildah execution, runtime templates, TUI progress |
| `zlayer-spec` | Shared deployment specification types |

It does **not** depend on the agent, scheduler, overlay, proxy, or any other runtime crate. This keeps the binary small and fast to compile.

## Requirements

- **buildah** -- `zlayer-build` shells out to buildah for the actual image assembly. If buildah is not found, the CLI will attempt to install it automatically.
- **Linux** -- buildah requires a Linux host (or a Linux container).

## Usage

### build

Build an image from a Dockerfile, ZImagefile, or runtime template.

```bash
# Build from Dockerfile (auto-detected in context directory)
zlayer-build build .

# Build from an explicit file
zlayer-build build -f path/to/Dockerfile .
zlayer-build build -f path/to/ZImagefile .

# Tag the image
zlayer-build build -t myapp:latest .

# Use a runtime template instead of a file
zlayer-build build --runtime node22 -t myapp:latest .

# Pass build arguments
zlayer-build build --build-arg VERSION=1.0 -t myapp:1.0.0 .

# Multi-stage: build only up to a specific stage
zlayer-build build --target builder .

# Push after building
zlayer-build build --push -t registry.example.com/myapp:latest .

# Disable layer caching
zlayer-build build --no-cache .

# Force TUI progress display (or disable it)
zlayer-build build --tui .
zlayer-build build --no-tui .
```

File detection order when `-f` is omitted:
1. Runtime template (if `--runtime` is specified)
2. `ZImagefile` in the context directory
3. `Dockerfile` in the context directory

When `-f` is provided, the CLI detects whether the file is a ZImagefile (filename contains "ZImagefile", or has a `.yml`/`.yaml` extension) or a Dockerfile, and parses accordingly.

### runtimes

List available runtime templates.

```bash
# Table output
zlayer-build runtimes

# JSON output (for scripting)
zlayer-build runtimes --json
```

### validate

Parse and validate a Dockerfile or ZImagefile without building anything.

```bash
# Validate a Dockerfile
zlayer-build validate Dockerfile

# Validate a ZImagefile
zlayer-build validate ZImagefile

# Validate any file (auto-detected)
zlayer-build validate path/to/build-file
```

The command prints a summary of stages, steps, and build mode, then exits with code 0 on success or 1 on parse errors.

## Comparison with `zlayer`

| | `zlayer` (full runtime) | `zlayer-build` |
|---|---|---|
| Image building | Yes | Yes |
| Container orchestration | Yes | No |
| Overlay networking | Yes | No |
| Scheduling / scaling | Yes | No |
| REST API server | Yes | No |
| Binary size | ~50MB+ | ~10MB |
| Build dependencies | Full workspace | 2 crates |

Use `zlayer-build` when you only need to build images -- for example in CI pipelines, development machines, or air-gapped build servers where the full runtime is unnecessary.

## Building from Source

```bash
cargo build --release -p zlayer-build
```

The binary is output to `target/release/zlayer-build`.

## License

Apache-2.0
