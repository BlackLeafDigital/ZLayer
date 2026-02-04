# zlayer-builder

Dockerfile parsing, ZImagefile support, and buildah-based container image building.

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
zlayer-builder = "0.8"
```

## Overview

`zlayer-builder` provides a high-level `ImageBuilder` API and low-level Dockerfile
parsing / buildah command generation for building container images without a daemon.

### Build Sources

The builder accepts three kinds of build input (checked in this order):

1. **Runtime template** -- specify a runtime like `node22` or `rust` and the
   builder generates a production-optimized Dockerfile automatically.
2. **ZImagefile** -- a YAML-based alternative to Dockerfiles (see below).
3. **Dockerfile** -- standard Dockerfile syntax with full multi-stage support.

### Quick Start

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

## ZImagefile Format

The `zimage` module provides parsing and Dockerfile conversion for
**ZImagefiles**, a YAML-based image definition format. ZImagefiles support
four mutually exclusive build modes:

| Mode | Top-level key | Description |
|------|--------------|-------------|
| Runtime template | `runtime` | Shorthand like `runtime: node22` |
| Single-stage | `base` + `steps` | One base image with ordered build steps |
| Multi-stage | `stages` | Named stages (last stage is the output image) |
| WASM | `wasm` | WebAssembly component builds |

### Auto-detection

When no explicit Dockerfile or ZImagefile path is provided, the builder
automatically looks for a file named `ZImagefile` in the build context
directory and uses it if found. You can also point to a specific file with
the `zimagefile()` builder method:

```rust,no_run
# use zlayer_builder::ImageBuilder;
# async fn example() -> Result<(), zlayer_builder::BuildError> {
let image = ImageBuilder::new("./my-project").await?
    .zimagefile("./my-project/ZImagefile")
    .tag("myapp:latest")
    .build()
    .await?;
# Ok(())
# }
```

### Example ZImagefile (single-stage)

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

### Example ZImagefile (multi-stage)

```yaml
stages:
  builder:
    base: "node:22-alpine"
    workdir: "/src"
    steps:
      - copy: ["package.json", "package-lock.json"]
        to: "./"
      - run: "npm ci"
      - copy: "."
        to: "."
      - run: "npm run build"
  runtime:
    base: "node:22-alpine"
    workdir: "/app"
    steps:
      - copy: "dist"
        from: builder
        to: "/app"
    cmd: ["node", "dist/index.js"]
expose: 3000
```

## Modules

| Module | Description |
|--------|-------------|
| `dockerfile` | Dockerfile parsing, types, and variable expansion |
| `buildah` | Buildah command generation and execution |
| `builder` | High-level `ImageBuilder` API |
| `zimage` | ZImagefile YAML parsing and Dockerfile conversion |
| `templates` | Runtime templates for common dev environments |
| `tui` | Terminal UI for build progress |
| `error` | Error types |

## License

MIT - See [LICENSE](../../LICENSE) for details.
