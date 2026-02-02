# Stage 1: Chef base with cargo-chef installed
FROM lukemathwalker/cargo-chef:latest-rust-1.83 AS chef
WORKDIR /build

# Stage 2: Generate recipe.json (dependency manifest)
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# Stage 3: Build dependencies (cached layer)
FROM chef AS builder

# Install system dependencies
RUN apt-get update && apt-get install -y \
    protobuf-compiler \
    libseccomp-dev \
    pkg-config \
    && rm -rf /var/lib/apt/lists/*

# Install wasm target and cargo-leptos
RUN rustup target add wasm32-unknown-unknown && \
    cargo install cargo-leptos

# Copy recipe and build dependencies only
COPY --from=planner /build/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

# Copy source and build application (only rebuilds changed code)
COPY . .
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
