# ZLayer in Docker

Run ZLayer with embedded containerd inside a Docker container (similar to how KIND runs Kubernetes in Docker).

## Quick Start

```bash
# Build the ZLayer binary first
cargo build --release --package runtime

# Build the Docker image
docker build -f docker/Dockerfile.zlayer-node -t zlayer-node .

# Run ZLayer (privileged required for nested containerd)
docker run --privileged -d --name zlayer \
  -p 80:80 -p 443:443 \
  zlayer-node serve

# Check logs
docker logs zlayer

# Deploy a spec
docker exec zlayer zlayer deploy --spec /path/to/spec.yaml

# Stop
docker rm -f zlayer
```

## Requirements

- **Docker** with support for `--privileged` containers
- **Linux host** (containerd requires Linux kernel features)
- The `--privileged` flag is required for containerd to manage cgroups and namespaces

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `CONTAINERD_SOCKET` | `/run/containerd/containerd.sock` | Path to containerd socket |
| `ZLAYER_NAMESPACE` | `zlayer` | Containerd namespace for containers |
| `ZLAYER_STATE_DIR` | `/var/lib/zlayer/containers` | Directory for container state/FIFOs |
| `ZLAYER_SNAPSHOTTER` | `overlayfs` | Containerd snapshotter to use |
| `ZLAYER_RUNTIME` | `io.containerd.runc.v2` | OCI runtime for containers |

## Volume Mounts

For persistent state and spec files, mount directories into the container:

```bash
docker run --privileged -d --name zlayer \
  -p 80:80 -p 443:443 \
  -v /path/to/specs:/specs:ro \
  -v zlayer-state:/var/lib/zlayer \
  zlayer-node serve
```

## Commands

The container runs ZLayer with any arguments you pass:

```bash
# Run the API server (default)
docker run --privileged zlayer-node serve

# Deploy a spec file
docker run --privileged -v ./myspec.yaml:/spec.yaml:ro \
  zlayer-node deploy --spec /spec.yaml

# Join a cluster
docker run --privileged zlayer-node join --api https://api.example.com
```

## Networking

By default, containers created inside ZLayer-in-Docker use the container's network namespace. For external access:

- Map ports with `-p 80:80 -p 443:443`
- Use host networking with `--network host` (simplest, but shares host network)
- Configure WireGuard overlay for multi-node setups

## Development

### Building the Image

```bash
# Debug build (faster, larger)
cargo build --package runtime
docker build -f docker/Dockerfile.zlayer-node \
  --build-arg BINARY_PATH=target/debug/zlayer \
  -t zlayer-node:dev .

# Release build (slower, optimized)
cargo build --release --package runtime
docker build -f docker/Dockerfile.zlayer-node -t zlayer-node .
```

### Debugging Inside the Container

```bash
# Shell into running container
docker exec -it zlayer bash

# Check containerd status
ctr version
ctr -n zlayer containers ls
ctr -n zlayer images ls

# View zlayer logs
docker logs -f zlayer
```

## Architecture

```
┌──────────────────────────────────────┐
│           Docker Container           │
│  ┌────────────────────────────────┐  │
│  │          ZLayer Runtime        │  │
│  │  ┌──────────────────────────┐  │  │
│  │  │       containerd         │  │  │
│  │  │  ┌────────┐ ┌────────┐   │  │  │
│  │  │  │ nginx  │ │  api   │   │  │  │
│  │  │  │  :80   │ │ :8080  │   │  │  │
│  │  │  └────────┘ └────────┘   │  │  │
│  │  └──────────────────────────┘  │  │
│  └────────────────────────────────┘  │
└──────────────────────────────────────┘
```

This is conceptually similar to KIND (Kubernetes IN Docker), where the outer container provides isolation while running a full container runtime inside.

## Limitations

- **Privileged mode required**: Without `--privileged`, containerd cannot manage cgroups properly
- **Linux only**: The nested containerd approach requires Linux-specific features
- **Resource overhead**: Running containerd inside Docker adds memory/CPU overhead
- **Nested networking**: Network configuration is more complex than bare-metal deployment
