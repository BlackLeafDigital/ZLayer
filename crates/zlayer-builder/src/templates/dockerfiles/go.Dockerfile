# Multi-stage build for Go
# Produces a minimal static binary
# Uses cache mounts for faster rebuilds

FROM golang:alpine AS builder
WORKDIR /app

# Install git for private dependencies if needed
RUN apk add --no-cache git

# Copy go mod files first
COPY go.mod go.sum ./

# Download dependencies with cache mounts
RUN --mount=type=cache,target=/go/pkg/mod \
    --mount=type=cache,target=/root/.cache/go-build \
    go mod download

# Copy source and build
COPY . .
RUN --mount=type=cache,target=/go/pkg/mod \
    --mount=type=cache,target=/root/.cache/go-build \
    CGO_ENABLED=0 GOOS=linux go build -ldflags="-w -s" -o app .

FROM alpine:latest
WORKDIR /app

# Add ca-certificates for HTTPS
RUN apk add --no-cache ca-certificates

# Copy the built binary
COPY --from=builder /app/app ./

# Default command
CMD ["./app"]
