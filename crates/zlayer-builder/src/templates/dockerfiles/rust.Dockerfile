# Multi-stage build for Rust
# Produces a minimal static binary using musl

FROM rust:alpine AS builder
WORKDIR /app

# Install musl-dev for static linking
RUN apk add --no-cache musl-dev

# Copy manifests first for dependency caching
COPY Cargo.toml Cargo.lock ./

# Create dummy src to build dependencies
RUN mkdir src && echo "fn main() {}" > src/main.rs
RUN cargo build --release && rm -rf src

# Copy actual source and rebuild
COPY src ./src
RUN touch src/main.rs && cargo build --release

FROM alpine:latest
WORKDIR /app

# Copy the built binary
COPY --from=builder /app/target/release/* ./

# Default command - override with your binary name
CMD ["./app"]
