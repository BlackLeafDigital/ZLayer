# zlayer-manager

A Komodo-like management UI for ZLayer built with Rust Leptos (SSR + WASM hydration).

## Overview

`zlayer-manager` provides a browser-based dashboard for managing ZLayer container orchestration. It connects to the ZLayer REST API and provides a modern, reactive UI for monitoring and managing your cluster.

## Features

- **Dashboard** - System overview with stats, node counts, and uptime
- **Deployments** - List, create, and manage deployments
- **Builds** - View build history, logs, and trigger new builds
- **Nodes** - Monitor cluster nodes and resource usage
- **Overlay Network** - WireGuard mesh status, peer health, IP allocation, DNS discovery
- **Settings** - Secrets management, cluster configuration

## Tech Stack

| Component | Technology |
|-----------|------------|
| Framework | Leptos 0.8 (SSR + WASM hydration) |
| Styling | Tailwind CSS v4 + DaisyUI |
| HTTP Client | reqwest |
| Server | Axum |

## Running

### Development

```bash
# Install cargo-leptos
cargo install cargo-leptos

# Run development server
cd crates/zlayer-manager
cargo leptos watch

# Access at http://localhost:9120
```

### Production

```bash
# Build release
cargo leptos build --release

# Run
./target/release/zlayer-manager --connect http://your-zlayer-api:3669
```

## CLI Options

```bash
# Connect to existing ZLayer API
zlayer-manager --connect http://localhost:3669

# Connect with authentication token
zlayer-manager --connect http://localhost:3669 --token <JWT_TOKEN>

# Custom port
zlayer-manager --port 9120

# Embedded mode (starts own ZLayer instance) - future
zlayer-manager --embedded
```

## Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `ZLAYER_API_URL` | ZLayer API base URL | `http://localhost:3669` |
| `ZLAYER_API_TOKEN` | JWT token for authentication | None |

## API Endpoints

The manager uses the ZLayer REST API:

| Feature | Endpoint |
|---------|----------|
| Health | `GET /health/ready` |
| Deployments | `GET /api/v1/deployments` |
| Services | `GET /api/v1/deployments/{name}/services` |
| Builds | `GET /api/v1/builds`, `GET /api/v1/build/{id}/logs` |
| Nodes | `GET /api/v1/nodes` |
| Overlay | `GET /api/v1/overlay/status`, `/peers`, `/ip-alloc`, `/dns` |
| Secrets | `GET /api/v1/secrets`, `POST /api/v1/secrets` |
| Jobs | `POST /api/v1/jobs/{name}/trigger` |
| Cron | `GET /api/v1/cron`, `PUT /api/v1/cron/{name}/enable` |

## Project Structure

```
src/
├── main.rs           # Axum SSR server
├── lib.rs            # WASM hydrate entry
├── cli.rs            # CLI arguments (clap)
├── api_client.rs     # HTTP client for ZLayer API
└── app/
    ├── mod.rs        # Router with all routes
    ├── server_fns.rs # Leptos server functions
    ├── components/   # Reusable UI components
    │   ├── navbar.rs
    │   ├── sidebar.rs
    │   ├── stats_card.rs
    │   ├── metrics_card.rs
    │   └── charts/
    └── pages/        # Page components
        ├── dashboard.rs
        ├── deployments.rs
        ├── builds.rs
        ├── nodes.rs
        ├── overlay.rs
        ├── settings.rs
        ├── networking.rs
        ├── ssh_tunnels.rs
        └── git.rs
```

## Building

```bash
# Check SSR build
cargo check -p zlayer-manager --features ssr

# Check WASM build
cargo check -p zlayer-manager --features hydrate --target wasm32-unknown-unknown

# Run tests
cargo test -p zlayer-manager --lib --features ssr

# Lint
cargo clippy -p zlayer-manager --features ssr -- -D warnings

# Format
cargo fmt -p zlayer-manager
```

## License

Apache-2.0
