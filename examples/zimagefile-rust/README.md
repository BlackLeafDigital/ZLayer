# ZImagefile vs Dockerfile: Rust (Multi-Stage with Cache Mounts)

This example shows a production Rust build with aggressive caching, expressed
in both ZImagefile (YAML) and Dockerfile syntax.

## Files

| File        | Format     | Description                            |
|-------------|------------|----------------------------------------|
| ZImagefile  | YAML       | ZLayer's native image build format     |
| Dockerfile  | Dockerfile | Traditional Docker build format        |

## What It Demonstrates

- **Build arguments** -- `args` in ZImagefile maps to `ARG` in Dockerfile.
  The Rust toolchain version is parameterized.
- **Cache mounts** -- the cargo registry and target directory are cached
  across builds, dramatically reducing incremental compile times. Compare
  the YAML `cache:` list to the `--mount=type=cache` flags.
- **Dependency pre-build trick** -- a dummy `main.rs` is compiled first so
  that dependency artifacts are cached before the real source is copied in.
- **Minimal runtime image** -- the final stage uses `debian:bookworm-slim`
  with only `ca-certificates`, a non-root user, and the compiled binary.
- **Labels** -- OCI image metadata via `labels:` / `LABEL`.

## Build

```bash
# Using ZImagefile (auto-detected when present)
zlayer build -t my-rust-app:latest .

# Override the Rust version
zlayer build --build-arg RUST_VERSION=1.82 -t my-rust-app:latest .

# Using Docker directly (BuildKit required for cache mounts)
DOCKER_BUILDKIT=1 docker build -t my-rust-app:latest .
```
