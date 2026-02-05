# zlayer-builder

Dockerfile parsing, ZImagefile support, pipeline orchestration, and buildah-based container image building.

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
zlayer-builder = "0.8"
```

## Overview

`zlayer-builder` provides:
- High-level `ImageBuilder` API for single image builds
- `PipelineExecutor` for multi-image builds with dependency ordering
- Low-level Dockerfile/ZImagefile parsing and buildah command generation

### Build Sources

The builder accepts three kinds of build input (checked in this order):

1. **Runtime template** -- specify a runtime like `node22` or `rust` and the
   builder generates a production-optimized Dockerfile automatically.
2. **ZImagefile** -- a YAML-based alternative to Dockerfiles (see below).
3. **Dockerfile** -- standard Dockerfile syntax with full multi-stage support.

## Modules

| Module | Description |
|--------|-------------|
| `builder` | High-level `ImageBuilder` API for single images |
| `pipeline` | Multi-image build orchestration with wave-based execution |
| `dockerfile` | Dockerfile parsing, types, and variable expansion |
| `zimage` | ZImagefile YAML parsing and Dockerfile conversion |
| `buildah` | Buildah command generation and execution |
| `templates` | Runtime templates for common dev environments |
| `tui` | Terminal UI for build progress |
| `error` | Error types |

## ImageBuilder API

Build a single image from Dockerfile, ZImagefile, or runtime template:

```rust,no_run
use zlayer_builder::ImageBuilder;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let image = ImageBuilder::new("./my-app").await?
        .tag("myapp:latest")
        .build()
        .await?;

    println!("Built image: {}", image.image_id);
    Ok(())
}
```

### With ZImagefile

```rust,no_run
use zlayer_builder::ImageBuilder;

async fn example() -> Result<(), zlayer_builder::BuildError> {
    let image = ImageBuilder::new("./my-project").await?
        .zimagefile("./my-project/ZImagefile")
        .tag("myapp:latest")
        .build()
        .await?;
    Ok(())
}
```

### With Runtime Template

```rust,no_run
use zlayer_builder::ImageBuilder;

async fn example() -> Result<(), zlayer_builder::BuildError> {
    let image = ImageBuilder::new("./my-node-app").await?
        .runtime("node22")
        .tag("myapp:latest")
        .build()
        .await?;
    Ok(())
}
```

## Pipeline API

Build multiple images with dependency ordering:

```rust,no_run
use zlayer_builder::pipeline::{PipelineExecutor, parse_pipeline};
use zlayer_builder::BuildahExecutor;
use std::path::PathBuf;

async fn example() -> Result<(), zlayer_builder::BuildError> {
    let yaml = std::fs::read_to_string("ZPipeline.yaml")?;
    let pipeline = parse_pipeline(&yaml)?;

    let executor = BuildahExecutor::new_async().await?;
    let result = PipelineExecutor::new(pipeline, PathBuf::from("."), executor)
        .fail_fast(true)
        .run()
        .await?;

    println!("Built {} images in {}ms", result.succeeded.len(), result.total_time_ms);
    Ok(())
}
```

### Wave-Based Execution

The pipeline executor resolves dependencies into "waves":
- **Wave 0**: Images with no dependencies (run in parallel)
- **Wave 1**: Images depending only on Wave 0 images
- **Wave N**: Images depending only on earlier waves

## ZImagefile Format

YAML-based alternative to Dockerfiles with four build modes:

| Mode | Top-level key | Description |
|------|--------------|-------------|
| Runtime template | `runtime` | Shorthand like `runtime: node22` |
| Single-stage | `base` + `steps` | One base image with ordered build steps |
| Multi-stage | `stages` | Named stages (last stage is output) |
| WASM | `wasm` | WebAssembly component builds |

### Single-Stage Example

```yaml
base: "alpine:3.19"
steps:
  - run: "apk add --no-cache curl"
  - copy: "app.sh"
    to: "/usr/local/bin/app.sh"
    chmod: "755"
  - workdir: "/app"
env:
  NODE_ENV: production
expose: 8080
cmd: ["./app.sh"]
```

### Multi-Stage Example

```yaml
stages:
  builder:
    base: "rust:1.90-bookworm"
    workdir: "/src"
    steps:
      - copy: "."
        to: "/src"
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

### Step Types

| Step | Description |
|------|-------------|
| `run` | Execute a shell command |
| `copy` | Copy files from context or previous stage |
| `add` | Add files with URL/archive support |
| `env` | Set environment variables |
| `workdir` | Change working directory |
| `user` | Set the user |

### Copy/Add Fields

| Field | Description |
|-------|-------------|
| `copy` / `add` | Source file(s) - string or list |
| `to` | Destination path |
| `from` | Source stage name (multi-stage only) |
| `owner` | Owner (user:group) |
| `chmod` | File permissions (e.g., "755") |

### Cache Mounts

```yaml
- run: "npm ci"
  cache:
    - target: /root/.npm
      id: npm-cache
      sharing: shared
      readonly: false
```

## ZPipeline Format

Multi-image build pipeline with dependency ordering:

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
    tags:
      - "${REGISTRY}/base:${VERSION}"

  app:
    file: docker/ZImagefile.app
    depends_on: [base]
    tags:
      - "${REGISTRY}/app:${VERSION}"

push:
  after_all: true
```

### Pipeline Fields

| Field | Description |
|-------|-------------|
| `version` | Pipeline format version ("1") |
| `vars` | Variables for `${VAR}` substitution in tags |
| `defaults` | Default settings for all images |
| `images` | Named images to build (order preserved) |
| `push.after_all` | Push all images after successful builds |

### Image Fields

| Field | Required | Description |
|-------|----------|-------------|
| `file` | Yes | Path to Dockerfile or ZImagefile |
| `context` | No | Build context directory (default: ".") |
| `tags` | No | Image tags with variable substitution |
| `depends_on` | No | Images that must build first |
| `build_args` | No | Build arguments (merged with defaults) |
| `no_cache` | No | Skip cache for this image |
| `format` | No | Output format: "oci" or "docker" |

## License

MIT - See [LICENSE](../../LICENSE) for details.
