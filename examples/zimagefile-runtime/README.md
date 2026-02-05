# ZImagefile: Runtime Template Shorthand

This example shows the simplest way to build an image with ZLayer -- a
single `runtime:` field that auto-generates the entire build pipeline.

## File

| File       | Description                                          |
|------------|------------------------------------------------------|
| ZImagefile | Uses `runtime: node22` for zero-config Node.js build |

## What It Demonstrates

- **Runtime templates** -- instead of writing build steps, specify a runtime
  name and ZLayer generates the multi-stage Dockerfile internally.
- **Override defaults** -- `cmd`, `env`, `expose`, and `healthcheck` still
  work alongside the `runtime` field to customize the output image.
- **Zero boilerplate** -- no base image selection, no COPY instructions, no
  dependency installation commands. Just point ZLayer at your project.

## Available Runtimes

| Runtime     | Description                           |
|-------------|---------------------------------------|
| `node20`    | Node.js 20 (LTS), Alpine-based       |
| `node22`    | Node.js 22 (Current), Alpine-based   |
| `python312` | Python 3.12                           |
| `python313` | Python 3.13                           |
| `rust`      | Rust (latest stable)                  |
| `go`        | Go (latest stable)                    |
| `deno`      | Deno (latest)                         |
| `bun`       | Bun (latest)                          |

## Build

```bash
# ZLayer auto-detects the ZImagefile and runtime template
zlayer build -t my-node-app:latest .
```

## When to Use Runtime Templates

Runtime templates are ideal when:

- Your project follows standard conventions (e.g. `package.json` with a
  `build` script, `Cargo.toml` with a binary target)
- You want fast iteration without writing a Dockerfile or full ZImagefile
- You want ZLayer's optimized build pipeline with cache mounts, multi-stage
  separation, and non-root users out of the box

When your build needs custom system packages, non-standard directory layouts,
or multiple services, use the full `stages:` or `base:` + `steps:` mode
instead. See the `zimagefile-node` and `zimagefile-rust` examples.
