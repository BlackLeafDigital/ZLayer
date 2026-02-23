# zlayer

The unified ZLayer binary -- TUI dashboard, full CLI, daemon runtime, and image builder in a single executable.

## Overview

Running `zlayer` with no arguments launches the interactive TUI dashboard. With subcommands, it provides the complete CLI for container orchestration, image building, and cluster management.

## TUI Dashboard

Launch with no arguments:

```bash
zlayer
```

The dashboard provides an interactive menu with:

- **Dashboard** -- System overview with node status, resource usage, and deployment health
- **Deploy** -- Deploy or manage services from spec files
- **Build Image** -- Build container images from Dockerfile, ZImagefile, or runtime templates
- **Validate** -- Validate deployment specs and build files with inline error display
- **Runtimes** -- Browse available runtime templates with descriptions
- **Quit** -- Exit the TUI

### Keyboard Shortcuts

| Key | Action |
|-----|--------|
| `1`-`5` | Select menu item |
| `Enter` | Confirm selection |
| `Esc` | Go back / cancel |
| `q` | Quit |
| `j` / `Down` | Move selection down |
| `k` / `Up` | Move selection up |
| `Tab` | Next input field |
| `Shift+Tab` | Previous input field |

## CLI Commands (23+ subcommands)

### Deployment

```bash
zlayer deploy                      # Auto-discover *.zlayer.yml and deploy
zlayer deploy app.zlayer.yml       # Deploy a specific spec
zlayer up                          # Deploy and run in foreground (Ctrl+C to stop)
zlayer up -b                       # Deploy and run in background
zlayer down                        # Stop a running deployment
zlayer status                      # View deployment status
zlayer logs -d my-app -s web -f    # Stream service logs
zlayer stop my-app --service web   # Stop a specific service
```

### Image Building

```bash
zlayer build . -t myapp:latest           # Build from Dockerfile or ZImagefile
zlayer build . --runtime node22 -t app   # Build with runtime template
zlayer build -z ZImagefile . -t app      # Build from explicit ZImagefile
zlayer pipeline ./ZPipeline.yaml         # Multi-image pipeline builds
zlayer runtimes                          # List available runtime templates
```

### Server / Daemon

```bash
zlayer serve --bind 0.0.0.0:3669         # Start the REST API server
zlayer serve --daemon                    # Run as a background daemon
```

### Node & Cluster Management

```bash
zlayer node init --advertise-addr 10.0.0.1   # Initialize a new cluster
zlayer node join 10.0.0.1:3669 --token TOK   # Join an existing cluster
zlayer node list                              # List cluster nodes
zlayer node status                            # Show node health
```

### WASM

```bash
zlayer wasm build .                      # Build WASM from source
zlayer wasm push app.wasm reg/app:v1     # Push WASM to registry
zlayer wasm validate handler.wasm        # Validate a WASM binary
zlayer wasm info handler.wasm            # Inspect WASM binary info
```

### Other Commands

```bash
zlayer validate deployment.yaml          # Validate a spec file
zlayer pull nginx:latest                 # Pull an image from a registry
zlayer token create --roles admin        # Create a JWT token
zlayer spec dump deployment.yaml         # Dump parsed spec as JSON/YAML
zlayer manager init                      # Generate manager deployment spec
zlayer tunnel add name --from a --to b   # Create node-to-node tunnel
```

## Building from Source

```bash
cargo build --release -p zlayer
```

The binary is output to `target/release/zlayer`.

## License

Apache-2.0
