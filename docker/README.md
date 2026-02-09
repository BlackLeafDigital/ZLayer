# ZLayer in Docker

Run ZLayer with embedded containerd inside a Docker container (similar to how KIND runs Kubernetes in Docker).

## Quick Start

```bash
# Build the ZLayer binary first
cargo build --release --package runtime

# Build the Docker image
docker build -f docker/Dockerfile.zlayer-node -t zlayer-node .

# Run ZLayer (privileged required for nested containerd)
docker run --privileged -d --name zlayer \
  -p 80:80 -p 443:443 \
  zlayer-node serve

# Check logs
docker logs zlayer

# Deploy a spec
docker exec zlayer zlayer-runtime deploy --spec /path/to/spec.yaml

# Stop
docker rm -f zlayer
```

## Requirements

- **Docker** with support for `--privileged` containers
- **Linux host** (containerd requires Linux kernel features)
- The `--privileged` flag is required for containerd to manage cgroups and namespaces

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `CONTAINERD_SOCKET` | `/run/containerd/containerd.sock` | Path to containerd socket |
| `ZLAYER_NAMESPACE` | `zlayer` | Containerd namespace for containers |
| `ZLAYER_STATE_DIR` | `/var/lib/zlayer/containers` | Directory for container state/FIFOs |
| `ZLAYER_SNAPSHOTTER` | `overlayfs` | Containerd snapshotter to use |
| `ZLAYER_RUNTIME` | `io.containerd.runc.v2` | OCI runtime for containers |

## Volume Mounts

For persistent state and spec files, mount directories into the container:

```bash
docker run --privileged -d --name zlayer \
  -p 80:80 -p 443:443 \
  -v /path/to/specs:/specs:ro \
  -v zlayer-state:/var/lib/zlayer \
  zlayer-node serve
```

## Commands

The container runs ZLayer with any arguments you pass:

```bash
# Run the API server (default)
docker run --privileged zlayer-node serve

# Deploy a spec file
docker run --privileged -v ./myspec.yaml:/spec.yaml:ro \
  zlayer-node deploy --spec /spec.yaml

# Join a cluster
docker run --privileged zlayer-node join --api https://api.example.com
```

## Networking

By default, containers created inside ZLayer-in-Docker use the container's network namespace. For external access:

- Map ports with `-p 80:80 -p 443:443`
- Use host networking with `--network host` (simplest, but shares host network)
- Configure overlay networking for multi-node setups

## Development

### Building the Image

```bash
# Debug build (faster, larger)
cargo build --package runtime
docker build -f docker/Dockerfile.zlayer-node \
  --build-arg BINARY_PATH=target/debug/zlayer-runtime \
  -t zlayer-node:dev .

# Release build (slower, optimized)
cargo build --release --package runtime
docker build -f docker/Dockerfile.zlayer-node -t zlayer-node .
```

### Debugging Inside the Container

```bash
# Shell into running container
docker exec -it zlayer bash

# Check containerd status
ctr version
ctr -n zlayer containers ls
ctr -n zlayer images ls

# View zlayer logs
docker logs -f zlayer
```

## Architecture

```
┌──────────────────────────────────────┐
│           Docker Container           │
│  ┌────────────────────────────────┐  │
│  │          ZLayer Runtime        │  │
│  │  ┌──────────────────────────┐  │  │
│  │  │       containerd         │  │  │
│  │  │  ┌────────┐ ┌────────┐   │  │  │
│  │  │  │ nginx  │ │  api   │   │  │  │
│  │  │  │  :80   │ │ :8080  │   │  │  │
│  │  │  └────────┘ └────────┘   │  │  │
│  │  └──────────────────────────┘  │  │
│  └────────────────────────────────┘  │
└──────────────────────────────────────┘
```

This is conceptually similar to KIND (Kubernetes IN Docker), where the outer container provides isolation while running a full container runtime inside.

## ZImagefiles

ZImagefile equivalents of the Dockerfiles are available for use with `zlayer-build`:

| File | Description |
|------|-------------|
| `ZImagefile.zlayer-web` | ZLayer web frontend (Leptos SSR + hydration) |
| `ZImagefile.zlayer-manager` | ZLayer management UI (Leptos SSR + hydration + Tailwind v4) |

These use declarative YAML syntax with cache mounts instead of the `cargo-chef` pattern used in the corresponding Dockerfiles.

```bash
# Build the web frontend using ZImagefile
zlayer-build build -f docker/ZImagefile.zlayer-web -t zlayer-web .

# Build the manager UI using ZImagefile
zlayer-build build -f docker/ZImagefile.zlayer-manager -t zlayer-manager .
```

See the root [README](../README.md#zimagefile) for full ZImagefile format documentation.

## ZImagefile Format

ZImagefiles are YAML-based alternatives to Dockerfiles. They provide:

- **Cleaner syntax** - Familiar YAML instead of Dockerfile DSL
- **Declarative cache mounts** - Simple `cache:` lists instead of `--mount=type=cache,...` strings
- **Multi-stage builds** - Named stages as a YAML map with `from:` references

### Build Modes

| Mode | Top-level key | Description |
|------|--------------|-------------|
| Runtime template | `runtime` | Shorthand like `runtime: node22` |
| Single-stage | `base` + `steps` | One base image with ordered build steps |
| Multi-stage | `stages` | Named stages (last stage is the output image) |
| WASM | `wasm` | WebAssembly component builds |

### Example: Multi-stage Rust build

```yaml
# ZImagefile
version: "1"

stages:
  builder:
    base: "rust:1.90-bookworm"
    workdir: "/build"
    steps:
      - copy: "."
        to: "/build"
      - run: "cargo build --release"
        cache:
          - target: /usr/local/cargo/registry
            id: cargo-registry
            sharing: shared

  runtime:
    base: "debian:bookworm-slim"
    steps:
      - copy: "target/release/myapp"
        from: builder
        to: "/usr/local/bin/myapp"
        chmod: "755"
    cmd: ["/usr/local/bin/myapp"]

expose: 8080
```

### Key Differences from Dockerfile

| Dockerfile | ZImagefile |
|------------|------------|
| `COPY src dest` | `copy: src` + `to: dest` |
| `COPY --from=stage` | `from: stage` on copy step |
| `RUN --mount=type=cache,...` | `cache:` list on run step |
| Multiple `FROM` blocks | `stages:` map |
| `--chown` / `--chmod` flags | `owner:` / `chmod:` fields |

## ZPipeline Format

ZPipeline.yaml coordinates building multiple images with dependency ordering, shared caches, and push operations.

### Schema

```yaml
version: "1"

vars:
  VERSION: "1.0.0"
  REGISTRY: "ghcr.io/myorg"

defaults:
  format: oci
  build_args:
    RUST_VERSION: "1.90"

images:
  base:
    file: docker/Dockerfile.base
    context: "."
    tags:
      - "${REGISTRY}/base:${VERSION}"
      - "${REGISTRY}/base:latest"
    build_args:
      EXTRA_ARG: "value"

  app:
    file: docker/ZImagefile.app
    context: "."
    depends_on: [base]
    tags:
      - "${REGISTRY}/app:${VERSION}"

push:
  after_all: true
```

### Fields

| Field | Description |
|-------|-------------|
| `version` | Pipeline format version (currently "1") |
| `vars` | Global variables for `${VAR}` substitution in tags |
| `defaults` | Default settings inherited by all images |
| `images` | Named images to build (order preserved) |
| `push.after_all` | Push all images after successful builds |

### Image Fields

| Field | Required | Description |
|-------|----------|-------------|
| `file` | Yes | Path to Dockerfile or ZImagefile |
| `context` | No | Build context directory (default: ".") |
| `tags` | No | Image tags (supports variable substitution) |
| `depends_on` | No | Images that must build first |
| `build_args` | No | Build arguments (merged with defaults) |
| `no_cache` | No | Skip cache for this image |
| `format` | No | Output format: "oci" or "docker" |

### Execution Model

Images are built in "waves" based on dependency depth:
- **Wave 0**: Images with no dependencies (run in parallel)
- **Wave 1**: Images depending only on Wave 0 images
- **Wave N**: Images depending only on earlier waves

## Building Images Locally

### Single Image

```bash
# Build from ZImagefile (auto-detected)
zlayer-build build -f docker/ZImagefile.zlayer-web -t zlayer-web .

# Build from Dockerfile
zlayer-build build -f docker/Dockerfile.zlayer-node -t zlayer-node .
```

### Multiple Images with Pipeline

```bash
# Build all images with default VERSION=dev
zlayer-build pipeline -f ZPipeline.yaml

# Build with specific version
zlayer-build pipeline -f ZPipeline.yaml --set VERSION=0.1.0

# Build and push (CI mode)
zlayer-build pipeline -f ZPipeline.yaml --no-tui --set VERSION=$VERSION --push
```

### Interactive TUI

```bash
# Use zlayer for menu-driven builds
zlayer
```

## Limitations

- **Privileged mode required**: Without `--privileged`, containerd cannot manage cgroups properly
- **Linux only**: The nested containerd approach requires Linux-specific features
- **Resource overhead**: Running containerd inside Docker adds memory/CPU overhead
- **Nested networking**: Network configuration is more complex than bare-metal deployment
