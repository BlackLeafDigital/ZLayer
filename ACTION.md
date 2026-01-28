# ZLayer GitHub Action

Install and run ZLayer CLI commands in your GitHub Actions workflows for container orchestration and WASM plugin management.

## Quick Start

```yaml
- uses: BlackLeafDigital/ZLayer@v1
  with:
    command: wasm build .
```

## Inputs

| Input | Description | Required | Default |
|-------|-------------|----------|---------|
| `version` | ZLayer version to install | No | `latest` |
| `command` | ZLayer command to run | **Yes** | - |
| `working-directory` | Working directory for the command | No | `.` |

## Outputs

| Output | Description |
|--------|-------------|
| `version` | The installed ZLayer version |

## Supported Platforms

| OS | Architecture |
|----|--------------|
| Linux | amd64, arm64 |
| macOS | amd64 (Intel), arm64 (Apple Silicon) |

## Examples

### Build and Push WASM Plugin

```yaml
name: Build WASM

on:
  push:
    branches: [main]

jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Build WASM plugin
        uses: BlackLeafDigital/ZLayer@v1
        with:
          command: wasm build --language rust --target wasip2

      - name: Push to GHCR
        uses: BlackLeafDigital/ZLayer@v1
        with:
          command: wasm push ./target/wasm32-wasip2/release/handler.wasm ghcr.io/${{ github.repository }}/handler:${{ github.sha }}
```

### Validate Deployment Spec

```yaml
- name: Validate deployment
  uses: BlackLeafDigital/ZLayer@v1
  with:
    command: validate deployment.yaml
```

### Deploy to ZLayer

```yaml
- name: Deploy
  uses: BlackLeafDigital/ZLayer@v1
  with:
    command: deploy deployment.yaml
```

### Pin to Specific Version

```yaml
- name: Use ZLayer v0.2.0
  uses: BlackLeafDigital/ZLayer@v1
  with:
    version: v0.2.0
    command: --version
```

### Build in Subdirectory

```yaml
- name: Build plugin in services/handler
  uses: BlackLeafDigital/ZLayer@v1
  with:
    working-directory: services/handler
    command: wasm build .
```

### Matrix Build (Multiple Languages)

```yaml
jobs:
  build:
    strategy:
      matrix:
        plugin: [auth-handler, rate-limiter, transformer]
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Build ${{ matrix.plugin }}
        uses: BlackLeafDigital/ZLayer@v1
        with:
          working-directory: plugins/${{ matrix.plugin }}
          command: wasm build .

      - name: Push ${{ matrix.plugin }}
        uses: BlackLeafDigital/ZLayer@v1
        with:
          command: wasm push plugins/${{ matrix.plugin }}/handler.wasm ghcr.io/${{ github.repository }}/${{ matrix.plugin }}:${{ github.sha }}
```

## Available Commands

| Command | Description |
|---------|-------------|
| `wasm build` | Build WASM plugin from source |
| `wasm push` | Push WASM artifact to OCI registry |
| `wasm validate` | Validate WASM binary |
| `wasm info` | Show WASM binary info |
| `validate` | Validate deployment spec |
| `deploy` | Deploy to ZLayer cluster |
| `build` | Build container image |

See [CLI Reference](https://zlayer.dev/docs/cli) for full command documentation.

## License

Apache-2.0
