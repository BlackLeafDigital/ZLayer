# Dockerfile: Multi-stage Rust build with cache mounts
#
# This is the traditional Dockerfile equivalent of the ZImagefile in this
# directory. Both produce the same image; compare them side-by-side to see
# how ZImagefile cache mounts and args map to Dockerfile syntax.
#
# Build with:
#   docker build -t my-rust-app:latest .
#   (or: DOCKER_BUILDKIT=1 docker build -t my-rust-app:latest .)

ARG RUST_VERSION=1.83

# ---------- Stage 1: builder ----------
FROM rust:${RUST_VERSION}-bookworm AS builder
WORKDIR /build

# Copy manifests first for dependency caching
COPY Cargo.toml Cargo.lock ./

# Create a dummy main.rs so cargo can fetch and compile dependencies
RUN mkdir src && echo 'fn main() {}' > src/main.rs
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry,sharing=shared \
    --mount=type=cache,target=/build/target,id=cargo-target,sharing=shared \
    cargo build --release

# Now copy real source and rebuild (only changed crates recompile)
COPY src /build/src

# Touch main.rs to invalidate the dummy binary
RUN --mount=type=cache,target=/usr/local/cargo/registry,id=cargo-registry,sharing=shared \
    --mount=type=cache,target=/build/target,id=cargo-target,sharing=shared \
    touch src/main.rs && cargo build --release

# Copy the binary out of the cache mount so it persists to the layer
RUN --mount=type=cache,target=/build/target,id=cargo-target,sharing=shared,readonly \
    cp target/release/myapp /build/myapp

# ---------- Stage 2: runtime ----------
FROM debian:bookworm-slim AS runtime

RUN --mount=type=cache,target=/var/cache/apt,id=apt-cache,sharing=shared \
    --mount=type=cache,target=/var/lib/apt,id=apt-lib,sharing=shared \
    apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

RUN groupadd -r app && useradd -r -g app app

COPY --from=builder --chmod=755 /build/myapp /usr/local/bin/myapp

WORKDIR /app
USER app
EXPOSE 8080

LABEL org.opencontainers.image.source="https://github.com/example/my-rust-app"

HEALTHCHECK --interval=15s --timeout=5s --start-period=5s --retries=3 \
    CMD curl -f http://localhost:8080/health

CMD ["/usr/local/bin/myapp"]
