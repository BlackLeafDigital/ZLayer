# Build stage
FROM rust:1.83-bookworm AS builder

# Install system dependencies
RUN apt-get update && apt-get install -y \
    protobuf-compiler \
    libseccomp-dev \
    pkg-config \
    && rm -rf /var/lib/apt/lists/*

# Install wasm target and cargo-leptos
RUN rustup target add wasm32-unknown-unknown && \
    cargo install cargo-leptos

WORKDIR /build

# Copy the entire workspace (needed for workspace dependencies)
COPY . .

# Build the zlayer-web crate
RUN cd crates/zlayer-web && cargo leptos build --release

# Runtime stage
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy the binary
COPY --from=builder /build/target/release/zlayer-web /app/zlayer-web

# Copy the site assets (WASM, CSS, etc.)
COPY --from=builder /build/target/site /app/target/site

# Set environment variables
ENV ZLAYER_WEB_ADDR=0.0.0.0:3000
ENV LEPTOS_SITE_ROOT=/app/target/site
ENV RUST_LOG=info

EXPOSE 3000

CMD ["/app/zlayer-web"]
