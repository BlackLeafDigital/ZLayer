# Multi-stage build for Rust
# Produces a minimal static binary using musl
# Uses cache mounts for faster rebuilds

FROM rust:alpine AS builder
WORKDIR /app

# Install musl-dev for static linking
RUN apk add --no-cache musl-dev

# Copy manifests first
COPY Cargo.toml Cargo.lock ./

# Copy source
COPY src ./src

# Build with cached cargo registry and target directory
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/app/target \
    cargo build --release && \
    cp target/release/$(grep -m1 'name' Cargo.toml | cut -d'"' -f2) /app/app 2>/dev/null || \
    cp target/release/* /app/ 2>/dev/null || true

FROM alpine:latest
WORKDIR /app

# Copy the built binary
COPY --from=builder /app/app ./app 2>/dev/null || true
COPY --from=builder /app/* ./ 2>/dev/null || true

# Default command - override with your binary name
CMD ["./app"]
