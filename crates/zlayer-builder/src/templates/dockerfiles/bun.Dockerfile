# Multi-stage build for Bun
# Optimized for production with minimal image size

FROM oven/bun:latest AS builder
WORKDIR /app

# Install dependencies first for better layer caching
COPY package.json bun.lockb* ./
RUN bun install --frozen-lockfile --production

FROM oven/bun:latest
WORKDIR /app

# Copy production dependencies
COPY --from=builder /app/node_modules ./node_modules

# Copy application source
COPY . .

# Run as non-root user
USER bun

# Default command - override with your entry point
CMD ["bun", "run", "index.ts"]
