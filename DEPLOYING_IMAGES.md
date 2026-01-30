# Deploying Images to ZLayer

A comprehensive guide to building, packaging, and deploying both container images and WebAssembly modules to ZLayer.

---

## Table of Contents

1. [Overview](#1-overview)
2. [Building Container Images](#2-building-container-images)
3. [Building WASM Images](#3-building-wasm-images)
4. [Packaging WASM as OCI Artifacts](#4-packaging-wasm-as-oci-artifacts)
5. [Deploying to ZLayer](#5-deploying-to-zlayer)
6. [Best Practices](#6-best-practices)
7. [Troubleshooting](#7-troubleshooting)
8. [Quick Reference](#8-quick-reference)

---

## 1. Overview

ZLayer is a unified deployment platform that supports two runtime types:

| Runtime | Technology | Best For |
|---------|------------|----------|
| **Containers** | OCI containers via libcontainer | Full applications, databases, existing Docker images |
| **WebAssembly** | Wasmtime with WASIp1/WASIp2 | Lightweight functions, plugins, edge computing |

### How ZLayer Auto-Detects Image Types

ZLayer automatically determines whether an image is a container or WASM artifact using multiple signals:

1. **OCI 1.1+ `artifactType` field** (most authoritative)
   - `application/vnd.wasm.component.v1+wasm` -> WASIp2 component
   - `application/vnd.wasm.content.layer.v1+wasm` -> WASIp1 module

2. **Config `mediaType` field**
   - `application/vnd.wasm.config.v0+json` -> WASM artifact

3. **Layer `mediaType` field** (fallback)
   - `application/wasm` -> WASM binary

No configuration is required - just reference the image and ZLayer handles the rest:

```yaml
services:
  my-service:
    image:
      name: ghcr.io/myorg/my-app:v1  # ZLayer auto-detects the type
```

### When to Use Containers vs WASM

| Aspect | Container | WASM |
|--------|-----------|------|
| **Cold Start** | ~300ms | ~1-5ms |
| **Image Size** | 10MB - 1GB+ | 100KB - 10MB |
| **Isolation** | Linux namespaces/cgroups | WASM sandbox |
| **Syscalls** | Full Linux | WASI subset |
| **exec() support** | Yes | No (single process model) |
| **Portability** | Arch-specific (x86/ARM) | Universal bytecode |
| **CPU/Memory Stats** | Via cgroups | Limited |
| **Languages** | Any (native binary) | Must compile to WASM |

**Choose Containers when:**
- Running existing Docker images
- Need full Linux syscall support
- Running databases or stateful services
- Complex multi-process applications
- Architecture-specific optimizations

**Choose WASM when:**
- Sub-millisecond cold starts are critical
- Building lightweight functions or plugins
- Edge computing scenarios
- Language-agnostic plugin systems
- Maximum portability is required

---

## 2. Building Container Images

### 2.1 Using ZLayer Builder

ZLayer includes a built-in image builder with Dockerfile support and runtime templates.

#### Basic Build

```bash
# Build from Dockerfile
zlayer build /path/to/app -t myapp:latest

# Build with runtime template (auto-detected from project files)
zlayer build /path/to/app -t myapp:latest --runtime-auto

# Build with specific runtime template
zlayer build /path/to/app -t myapp:latest --runtime node22

# List available runtime templates
zlayer runtimes
```

#### Available Runtime Templates

| Runtime | Base Image | Use Case |
|---------|------------|----------|
| `node20`, `node22` | Alpine | Node.js applications |
| `python312`, `python313` | Debian slim | Python applications |
| `rust` | musl-based | Static Rust binaries |
| `go` | scratch | Static Go binaries |
| `deno` | Debian | Deno runtime |
| `bun` | Debian | Bun runtime |

#### Build via API

```bash
# Build from Dockerfile
curl -X POST http://localhost:8080/api/v1/build/json \
  -H "Content-Type: application/json" \
  -d '{
    "context_path": "/path/to/app",
    "tags": ["myapp:latest"]
  }'

# Build with runtime template
curl -X POST http://localhost:8080/api/v1/build/json \
  -H "Content-Type: application/json" \
  -d '{
    "context_path": "/path/to/app",
    "runtime": "node22",
    "tags": ["myapp:v1.0.0"]
  }'

# Stream build progress
curl http://localhost:8080/api/v1/build/{build_id}/stream
```

#### Dockerfile Examples

**Basic Node.js Application:**

```dockerfile
FROM node:22-alpine AS builder
WORKDIR /app
COPY package*.json ./
RUN npm ci --production

FROM node:22-alpine
WORKDIR /app
COPY --from=builder /app/node_modules ./node_modules
COPY . .
EXPOSE 3000
CMD ["node", "server.js"]
```

**Multi-stage Rust Build:**

```dockerfile
FROM rust:1.85-alpine AS builder
RUN apk add --no-cache musl-dev
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --target x86_64-unknown-linux-musl

FROM scratch
COPY --from=builder /app/target/x86_64-unknown-linux-musl/release/myapp /myapp
ENTRYPOINT ["/myapp"]
```

**Python with uv:**

```dockerfile
FROM python:3.13-slim AS builder
WORKDIR /app
RUN pip install uv
COPY pyproject.toml uv.lock ./
RUN uv sync --frozen --no-dev

FROM python:3.13-slim
WORKDIR /app
COPY --from=builder /app/.venv /app/.venv
COPY . .
ENV PATH="/app/.venv/bin:$PATH"
CMD ["python", "-m", "myapp"]
```

#### Build Caching

ZLayer leverages buildah's layer caching. For optimal cache performance:

```bash
# Build with cache mount for package managers
zlayer build . -t myapp:latest --cache-from myapp:latest

# Build with specific cache directory
zlayer build . -t myapp:latest --cache-dir /var/cache/zlayer/buildcache
```

### 2.2 Using External Tools

#### Docker / Podman

```bash
# Build with Docker
docker build -t myapp:latest .
docker push ghcr.io/myorg/myapp:latest

# Build with Podman (daemonless)
podman build -t myapp:latest .
podman push ghcr.io/myorg/myapp:latest
```

#### Buildah (Daemonless, OCI-native)

```bash
# Build from Dockerfile
buildah bud -t myapp:latest .

# Push to registry
buildah push myapp:latest docker://ghcr.io/myorg/myapp:latest
```

#### Kaniko (CI/CD Environments)

```bash
# Build in Kubernetes or containerized CI
/kaniko/executor \
  --context=git://github.com/myorg/myapp \
  --destination=ghcr.io/myorg/myapp:latest
```

### 2.3 Pushing to Registries

#### OCI-Compliant Registries

ZLayer supports any OCI-compliant registry:

- GitHub Container Registry (ghcr.io)
- Docker Hub (docker.io)
- Amazon ECR
- Google Artifact Registry
- Azure Container Registry
- Self-hosted (Harbor, GitLab Registry, Forgejo/Gitea)

#### Authentication

```bash
# Docker Hub
docker login docker.io

# GitHub Container Registry
echo $GITHUB_TOKEN | docker login ghcr.io -u USERNAME --password-stdin

# AWS ECR
aws ecr get-login-password --region us-east-1 | \
  docker login --username AWS --password-stdin 123456789.dkr.ecr.us-east-1.amazonaws.com

# Generic (works with zlayer CLI)
zlayer registry login ghcr.io --username $USER --password-stdin
```

#### Image Tagging Conventions

```bash
# Semantic versioning
myapp:1.0.0
myapp:1.0
myapp:1

# Git-based tags
myapp:main
myapp:sha-abc1234
myapp:pr-123

# Environment tags
myapp:staging
myapp:production

# Combined
myapp:1.0.0-sha-abc1234
```

---

## 3. Building WASM Images

### Supported Languages

| Tier | Languages | Compilation |
|------|-----------|-------------|
| **Tier 1** | Rust, Go, C/C++, Zig, AssemblyScript | Direct to WASM |
| **Tier 2** | C#/.NET, Kotlin, Swift | Experimental |
| **Tier 3** | Python, JavaScript, Ruby, PHP, Lua | Interpreter-based |

### 3.1 Rust WASM

Rust has the best WASM support with two approaches:

#### Prerequisites

```bash
# Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Add WASI targets
rustup target add wasm32-wasip1    # WASI Preview 1 (stable)
rustup target add wasm32-wasip2    # WASI Preview 2 (component model)

# Install cargo-component for WASIp2 components
cargo install cargo-component
```

#### Standard Rust (WASIp1)

```bash
# Build for WASI Preview 1
cargo build --target wasm32-wasip1 --release

# Output: target/wasm32-wasip1/release/myapp.wasm
```

**Cargo.toml:**

```toml
[package]
name = "myapp"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[profile.release]
opt-level = "s"      # Optimize for size
lto = true           # Link-time optimization
strip = true         # Strip debug symbols
```

#### Rust Components (WASIp2)

```bash
# Build WASI Preview 2 component
cargo component build --release

# Output: target/wasm32-wasip2/release/myapp.wasm
```

**Cargo.toml for cargo-component:**

```toml
[package]
name = "rust-hello-wasm"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[package.metadata.component]
package = "example:rust-hello"

[profile.release]
opt-level = "s"
lto = true
strip = true
codegen-units = 1
```

#### Complete Rust Example

**src/lib.rs:**

```rust
#![no_std]
extern crate alloc;

use alloc::string::String;

pub struct Greeter {
    name: String,
}

impl Greeter {
    pub fn new(name: &str) -> Self {
        Self { name: String::from(name) }
    }

    pub fn greet(&self) -> String {
        alloc::format!("Hello from Rust WASM, {}!", self.name)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn _start() {
    let greeter = Greeter::new("ZLayer");
    let _message = greeter.greet();
}
```

### 3.2 Go WASM (TinyGo)

Standard Go produces large WASM binaries. Use TinyGo for smaller output.

#### Prerequisites

```bash
# Install TinyGo (Linux)
wget https://github.com/tinygo-org/tinygo/releases/download/v0.34.0/tinygo_0.34.0_amd64.deb
sudo dpkg -i tinygo_0.34.0_amd64.deb

# macOS
brew install tinygo

# Verify installation
tinygo version
```

#### Build Commands

```bash
# Build for WASI Preview 1
tinygo build -o main.wasm -target=wasip1 .

# Build for WASI Preview 2
tinygo build -o main.wasm -target=wasip2 .

# Optimized build
tinygo build -o main.wasm -target=wasip2 -opt=2 .
```

#### Complete Go Example

**main.go:**

```go
//go:build tinygo

package main

import "fmt"

type Greeter struct {
    name string
}

func NewGreeter(name string) *Greeter {
    return &Greeter{name: name}
}

func (g *Greeter) Greet() string {
    return fmt.Sprintf("Hello from Go WASM, %s!", g.name)
}

func main() {
    greeter := NewGreeter("ZLayer")
    fmt.Println(greeter.Greet())
}

//export version
func version() {
    fmt.Println("go-hello-wasm v0.1.0")
}
```

**go.mod:**

```go
module example.com/go-hello

go 1.22
```

#### Limitations

- No goroutines (TinyGo has limited concurrency support)
- Reduced standard library
- No reflection
- Some packages unavailable

### 3.3 Python WASM

Python runs via componentize-py which wraps the Python interpreter.

#### Prerequisites

```bash
# Install componentize-py
pip install componentize-py

# Verify installation
componentize-py --version
```

#### Build Commands

```bash
# Create a WIT world definition
mkdir wit
cat > wit/world.wit << 'EOF'
package example:python;

world python-hello {
    export greet: func(name: string) -> string;
}
EOF

# Build component
componentize-py -d wit -w python-hello componentize app -o app.wasm
```

#### Complete Python Example

**app.py:**

```python
def greet(name: str) -> str:
    return f"Hello from Python WASM, {name}!"
```

**wit/world.wit:**

```wit
package example:python;

world python-hello {
    export greet: func(name: string) -> string;
}
```

#### Limitations

- Large binary size (~20MB+ due to embedded interpreter)
- Slower startup than compiled languages
- Limited to WASI-compatible operations

### 3.4 TypeScript WASM

TypeScript compiles to WASM via jco (JavaScript Component Tools).

#### Prerequisites

```bash
# Install jco
npm install -g @bytecodealliance/jco
npm install -g @bytecodealliance/componentize-js

# Verify installation
jco --version
```

#### Build Commands

```bash
# Compile TypeScript to JavaScript first
npx tsc

# Componentize JavaScript to WASM
jco componentize src/index.js --wit wit -o dist/component.wasm
```

#### Complete TypeScript Example

**package.json:**

```json
{
  "name": "ts-hello-wasm",
  "type": "module",
  "scripts": {
    "build": "tsc && jco componentize dist/index.js --wit wit -o dist/component.wasm"
  },
  "devDependencies": {
    "typescript": "^5.0.0",
    "@bytecodealliance/jco": "^1.0.0",
    "@bytecodealliance/componentize-js": "^0.10.0"
  }
}
```

**src/index.ts:**

```typescript
export function greet(name: string): string {
    return `Hello from TypeScript WASM, ${name}!`;
}
```

**wit/world.wit:**

```wit
package example:typescript;

world ts-hello {
    export greet: func(name: string) -> string;
}
```

### 3.5 C/C++ WASM

C/C++ compiles via the WASI SDK which includes a modified Clang.

#### Prerequisites

```bash
# Download WASI SDK
export WASI_VERSION=24
export WASI_VERSION_FULL=${WASI_VERSION}.0
wget https://github.com/WebAssembly/wasi-sdk/releases/download/wasi-sdk-${WASI_VERSION}/wasi-sdk-${WASI_VERSION_FULL}-x86_64-linux.tar.gz
tar xvf wasi-sdk-${WASI_VERSION_FULL}-x86_64-linux.tar.gz
export WASI_SDK_PATH=$(pwd)/wasi-sdk-${WASI_VERSION_FULL}-x86_64-linux

# Add to PATH
export PATH="$WASI_SDK_PATH/bin:$PATH"
```

#### Build Commands

```bash
# Compile C to WASM
$WASI_SDK_PATH/bin/clang --target=wasm32-wasi -o main.wasm main.c

# With optimization
$WASI_SDK_PATH/bin/clang --target=wasm32-wasi -O2 -o main.wasm main.c

# C++ compilation
$WASI_SDK_PATH/bin/clang++ --target=wasm32-wasi -o main.wasm main.cpp
```

#### Complete C Example

**main.c:**

```c
#include <stdio.h>
#include <string.h>

void greet(const char* name) {
    printf("Hello from C WASM, %s!\n", name);
}

int main(int argc, char** argv) {
    const char* name = argc > 1 ? argv[1] : "ZLayer";
    greet(name);
    return 0;
}
```

**Makefile:**

```makefile
WASI_SDK_PATH ?= /opt/wasi-sdk
CC = $(WASI_SDK_PATH)/bin/clang
CFLAGS = --target=wasm32-wasi -O2

main.wasm: main.c
	$(CC) $(CFLAGS) -o $@ $<

clean:
	rm -f main.wasm
```

### 3.6 Other Languages

#### Zig

```bash
# Build with Zig (native WASI support)
zig build -Dtarget=wasm32-wasi -Doptimize=ReleaseFast

# Direct compilation
zig build-exe -target wasm32-wasi main.zig -o main.wasm
```

**build.zig:**

```zig
const std = @import("std");

pub fn build(b: *std.Build) void {
    const target = b.standardTargetOptions(.{
        .default_target = .{
            .cpu_arch = .wasm32,
            .os_tag = .wasi,
        },
    });
    const optimize = b.standardOptimizeOption(.{});

    const exe = b.addExecutable(.{
        .name = "main",
        .root_source_file = b.path("src/main.zig"),
        .target = target,
        .optimize = optimize,
    });

    b.installArtifact(exe);
}
```

#### AssemblyScript

```bash
# Install AssemblyScript
npm install -g assemblyscript

# Compile
asc assembly/index.ts --target release -o build/release.wasm --optimize
```

**assembly/index.ts:**

```typescript
export function greet(name: string): string {
    return "Hello from AssemblyScript WASM, " + name + "!";
}
```

---

## 4. Packaging WASM as OCI Artifacts

WASM modules must be packaged as OCI artifacts to be stored in container registries and deployed by ZLayer.

### 4.1 Manual Packaging

#### Using wkg (WASM Package Tools)

```bash
# Install wkg
cargo install wkg

# Push WASM to OCI registry
wkg oci push target/wasm32-wasip1/release/myapp.wasm ghcr.io/myorg/myapp:v1

# Push with annotations
wkg oci push myapp.wasm ghcr.io/myorg/myapp:v1 \
  --annotation org.opencontainers.image.description="My WASM application"
```

#### Using oras

```bash
# Install oras
curl -LO https://github.com/oras-project/oras/releases/download/v1.2.0/oras_1.2.0_linux_amd64.tar.gz
tar -xzf oras_1.2.0_linux_amd64.tar.gz

# Push WASM artifact
oras push ghcr.io/myorg/myapp:v1 \
  --artifact-type application/vnd.wasm.component.v1+wasm \
  myapp.wasm:application/wasm
```

#### OCI Manifest Structure

WASM artifacts use this manifest structure:

```json
{
  "schemaVersion": 2,
  "mediaType": "application/vnd.oci.image.manifest.v1+json",
  "artifactType": "application/vnd.wasm.component.v1+wasm",
  "config": {
    "mediaType": "application/vnd.wasm.config.v0+json",
    "digest": "sha256:44136fa...",
    "size": 2
  },
  "layers": [
    {
      "mediaType": "application/wasm",
      "digest": "sha256:abc123...",
      "size": 12345
    }
  ]
}
```

### 4.2 Using ZLayer CLI

```bash
# Build and package WASM in one step
zlayer wasm build . -t ghcr.io/myorg/myapp:v1

# Package existing WASM file
zlayer wasm export ./myapp.wasm -t ghcr.io/myorg/myapp:v1

# Push to registry
zlayer wasm push ghcr.io/myorg/myapp:v1

# Validate a WASM artifact
zlayer wasm validate ./myapp.wasm

# Inspect WASM artifact info from registry
zlayer wasm info ghcr.io/myorg/myapp:v1
```

---

## 5. Deploying to ZLayer

### 5.1 Deployment Specification

ZLayer uses a declarative YAML format for deployments:

```yaml
version: v1
deployment: my-application

services:
  <service-name>:
    rtype: service | job | cron
    image:
      name: <image-reference>
      pull_policy: always | if_not_present | never
    resources:
      cpu: <cores>
      memory: <size>
    env:
      KEY: value
      FROM_ENV: $E:ENV_VAR
    endpoints:
      - name: <endpoint-name>
        protocol: http | https | tcp
        port: <port>
        expose: public | internal
    scale:
      mode: fixed | adaptive | manual
      replicas: <count>
    health:
      check:
        type: http | tcp | exec
        # type-specific options
```

### 5.2 Container Deployments

#### Basic Container Deployment

```yaml
version: v1
deployment: nginx-static

services:
  web:
    rtype: service
    image:
      name: nginx:1.25-alpine
      pull_policy: if_not_present

    resources:
      cpu: 0.25
      memory: 64Mi

    endpoints:
      - name: http
        protocol: http
        port: 80
        expose: public

    scale:
      mode: fixed
      replicas: 2

    health:
      start_grace: 5s
      interval: 10s
      timeout: 3s
      retries: 3
      check:
        type: http
        url: http://localhost:80/
        expect_status: 200
```

#### Full Application Stack

```yaml
version: v1
deployment: multi-service-app

services:
  # Frontend
  web:
    rtype: service
    image:
      name: nginx:1.25-alpine
    resources:
      cpu: 0.25
      memory: 64Mi
    endpoints:
      - name: https
        protocol: https
        port: 443
        expose: public
    depends:
      - service: api
        condition: healthy
        timeout: 120s
    scale:
      mode: adaptive
      min: 2
      max: 10
      targets:
        cpu: 70
        rps: 1000

  # Backend API
  api:
    rtype: service
    image:
      name: ghcr.io/myorg/api:latest
    resources:
      cpu: 1.0
      memory: 512Mi
    env:
      DATABASE_URL: postgresql://user:$E:DB_PASS@database.service:5432/app
      REDIS_URL: redis://cache.service:6379
    endpoints:
      - name: http
        port: 8080
        expose: internal
    depends:
      - service: database
        condition: healthy
      - service: cache
        condition: healthy
    init:
      steps:
        - id: wait-db
          uses: init.wait_tcp
          with:
            host: database.service
            port: 5432
        - id: migrate
          uses: init.exec
          with:
            command: ["./migrate", "up"]
    scale:
      mode: adaptive
      min: 2
      max: 20

  # Database
  database:
    rtype: service
    image:
      name: postgres:16-alpine
    resources:
      cpu: 1.0
      memory: 1Gi
    env:
      POSTGRES_DB: app
      POSTGRES_USER: user
      POSTGRES_PASSWORD: $E:DB_PASS
    endpoints:
      - name: postgres
        protocol: tcp
        port: 5432
        expose: internal
    scale:
      mode: fixed
      replicas: 1
    health:
      check:
        type: exec
        command: ["pg_isready", "-U", "user"]

  # Cache
  cache:
    rtype: service
    image:
      name: redis:7-alpine
    resources:
      cpu: 0.5
      memory: 256Mi
    endpoints:
      - name: redis
        protocol: tcp
        port: 6379
        expose: internal
    scale:
      mode: fixed
      replicas: 1
```

### 5.3 WASM Deployments

#### Basic WASM Deployment

```yaml
version: v1
deployment: wasm-function

services:
  handler:
    rtype: service
    image:
      # ZLayer auto-detects WASM artifacts
      name: ghcr.io/myorg/handler:v1
      pull_policy: if_not_present

    resources:
      cpu: 0.5
      memory: 64Mi

    env:
      LOG_LEVEL: info
      GREETING: "Hello from ZLayer"

    # Optional: explicit WASM configuration
    runtime:
      type: wasm
      capabilities:
        - wasi:cli/stdout
        - wasi:cli/stderr
        - wasi:cli/environment
      max_memory: 64Mi
      max_fuel: 0  # 0 = unlimited

    endpoints:
      - name: http
        protocol: http
        port: 8080
        expose: internal

    scale:
      mode: fixed
      replicas: 2
```

#### WASM with Multiple Languages

```yaml
version: v1
deployment: wasm-polyglot

services:
  # Rust WASM handler
  rust-handler:
    rtype: service
    image:
      name: ghcr.io/myorg/rust-handler:v1
    resources:
      cpu: 0.5
      memory: 64Mi
    runtime:
      type: wasm
      max_memory: 64Mi
    endpoints:
      - name: http
        port: 8080
        expose: internal

  # Go WASM handler
  go-handler:
    rtype: service
    image:
      name: ghcr.io/myorg/go-handler:v1
    resources:
      cpu: 0.5
      memory: 128Mi
    runtime:
      type: wasm
      max_memory: 128Mi
    endpoints:
      - name: http
        port: 8080
        expose: internal

  # Gateway routing to WASM backends
  gateway:
    rtype: service
    image:
      name: ghcr.io/myorg/gateway:latest
    env:
      RUST_BACKEND: rust-handler.service:8080
      GO_BACKEND: go-handler.service:8080
    endpoints:
      - name: https
        port: 443
        expose: public
    depends:
      - service: rust-handler
        condition: healthy
      - service: go-handler
        condition: healthy
```

### 5.4 Mixed Deployments

Containers and WASM can coexist in the same deployment:

```yaml
version: v1
deployment: hybrid-app

services:
  # WASM for lightweight request processing
  processor:
    rtype: service
    image:
      name: ghcr.io/myorg/wasm-processor:v1
    runtime:
      type: wasm
    endpoints:
      - name: http
        port: 8080
        expose: internal
    scale:
      mode: adaptive
      min: 2
      max: 100  # WASM scales easily due to fast cold starts

  # Container for stateful operations
  api:
    rtype: service
    image:
      name: ghcr.io/myorg/api:latest
    endpoints:
      - name: http
        port: 3000
        expose: internal
    depends:
      - service: processor
        condition: healthy
      - service: database
        condition: healthy

  # Container database
  database:
    rtype: service
    image:
      name: postgres:16-alpine
    endpoints:
      - name: postgres
        port: 5432
        expose: internal
    scale:
      mode: fixed
      replicas: 1
```

#### Service Discovery

Services communicate using internal DNS:

```
<service-name>.<deployment-name>.service:<port>
```

Simplified within the same deployment:
```
<service-name>.service:<port>
```

---

## 6. Best Practices

### 6.1 Container Best Practices

#### Image Optimization

```dockerfile
# Use multi-stage builds
FROM node:22-alpine AS builder
WORKDIR /app
COPY package*.json ./
RUN npm ci --production

FROM node:22-alpine
WORKDIR /app
# Copy only production dependencies
COPY --from=builder /app/node_modules ./node_modules
COPY . .
# Run as non-root user
USER node
CMD ["node", "server.js"]
```

**Size Reduction Tips:**
- Use Alpine or distroless base images
- Multi-stage builds to exclude build tools
- Remove package manager caches (`rm -rf /var/cache/apk/*`)
- Use `.dockerignore` to exclude unnecessary files

#### Security Scanning

```bash
# Scan with Trivy
trivy image myapp:latest

# Scan during CI/CD
docker scan myapp:latest

# Enable in ZLayer
zlayer build . -t myapp:latest --scan
```

#### Layer Caching

Order Dockerfile instructions from least to most frequently changed:

```dockerfile
FROM node:22-alpine

# System dependencies (rarely change)
RUN apk add --no-cache tini

# Dependencies (change occasionally)
WORKDIR /app
COPY package*.json ./
RUN npm ci --production

# Application code (changes frequently)
COPY . .

ENTRYPOINT ["/sbin/tini", "--"]
CMD ["node", "server.js"]
```

### 6.2 WASM Best Practices

#### Binary Size Optimization

**Rust:**
```toml
[profile.release]
opt-level = "s"       # Optimize for size ("z" for even smaller)
lto = true            # Link-time optimization
strip = true          # Strip symbols
codegen-units = 1     # Better optimization
panic = "abort"       # Smaller panic handling
```

**Additional optimization with wasm-opt:**
```bash
# Install binaryen
cargo install wasm-opt

# Optimize WASM binary
wasm-opt -Oz -o optimized.wasm input.wasm
```

#### Memory Management

```yaml
runtime:
  type: wasm
  max_memory: 64Mi     # Set appropriate limit
  max_fuel: 1000000    # Limit instruction count for CPU-bound protection
```

#### Performance Tuning

1. **Pre-compilation**: ZLayer caches compiled WASM modules for faster subsequent starts
2. **Module pooling**: For high-throughput scenarios, ZLayer maintains warm instances
3. **Appropriate resources**: Match memory limits to actual needs

```yaml
# Development
resources:
  memory: 128Mi

# Production (after profiling)
resources:
  memory: 32Mi
runtime:
  type: wasm
  max_memory: 32Mi
```

---

## 7. Troubleshooting

### 7.1 Common Container Issues

#### Image Pull Failures

```bash
# Check registry authentication
zlayer registry login ghcr.io

# Verify image exists
crane manifest ghcr.io/myorg/myapp:v1

# Check pull policy
# If using if_not_present, the old cached image may be used
image:
  name: ghcr.io/myorg/myapp:v1
  pull_policy: always  # Force pull
```

#### Container Won't Start

```bash
# Check logs
zlayer logs -d my-deployment -s my-service

# Verify entrypoint/command
zlayer spec inspect my-deployment

# Common issues:
# - Missing environment variables
# - Incorrect working directory
# - File permissions
# - Port already in use
```

#### Health Check Failures

```bash
# Increase start grace period for slow-starting containers
health:
  start_grace: 60s  # Wait before first health check
  interval: 10s
  timeout: 5s
  retries: 5
  check:
    type: http
    url: http://localhost:8080/health
```

### 7.2 Common WASM Issues

#### Module Won't Load

```bash
# Validate WASM module
zlayer wasm validate ./myapp.wasm

# Check WASI compatibility
wasm-tools validate --features all myapp.wasm

# Common issues:
# - Missing _start export
# - Incompatible WASI version
# - Memory import issues
```

#### Runtime Errors

```bash
# Check WASM-specific logs
zlayer logs -d my-deployment -s my-service --runtime

# Common issues:
# - Insufficient memory limit
# - Missing WASI capabilities
# - Fuel exhaustion (if limited)
```

#### Performance Issues

```yaml
# Increase memory if hitting limits
runtime:
  type: wasm
  max_memory: 128Mi  # Increase from default

# Remove fuel limit for CPU-intensive tasks
runtime:
  type: wasm
  max_fuel: 0  # Unlimited
```

### 7.3 Registry Issues

#### Authentication Errors

```bash
# Re-authenticate
zlayer registry logout ghcr.io
zlayer registry login ghcr.io

# Check token expiration
# GitHub tokens expire - generate a new one if needed

# Verify permissions
# Ensure your token has read:packages and write:packages scopes
```

#### Push Failures

```bash
# Check image tag format
# Valid: ghcr.io/owner/repo:tag
# Invalid: ghcr.io/owner/repo (missing tag)

# Check size limits
# Some registries have layer size limits

# Verify network connectivity
curl -I https://ghcr.io/v2/
```

#### Pull Timeouts

```yaml
# Increase pull timeout in deployment
image:
  name: ghcr.io/myorg/large-image:v1
  pull_policy: if_not_present  # Cache locally after first pull
```

---

## 8. Quick Reference

### Command Cheat Sheet

```bash
# ============ CONTAINER BUILDS ============
# Build from Dockerfile
zlayer build . -t myapp:latest

# Build with runtime template
zlayer build . -t myapp:latest --runtime node22

# Build via Docker/Podman
docker build -t myapp:latest .
podman build -t myapp:latest .

# Push to registry
docker push ghcr.io/myorg/myapp:latest
zlayer registry push ghcr.io/myorg/myapp:latest

# ============ WASM BUILDS ============
# Rust (WASIp1)
cargo build --target wasm32-wasip1 --release

# Rust (WASIp2 component)
cargo component build --release

# Go (TinyGo)
tinygo build -o main.wasm -target=wasip2 .

# Python
componentize-py -d wit -w world componentize app -o app.wasm

# TypeScript
jco componentize src/index.js --wit wit -o dist/component.wasm

# C
clang --target=wasm32-wasi -O2 -o main.wasm main.c

# Push WASM to registry
wkg oci push myapp.wasm ghcr.io/myorg/myapp:v1

# ============ DEPLOYMENT ============
# Deploy
zlayer deploy deployment.yaml

# Validate spec
zlayer validate deployment.yaml

# Check status
zlayer status my-deployment

# View logs
zlayer logs -d my-deployment -s my-service --follow

# Stop deployment
zlayer stop my-deployment

# ============ WASM MANAGEMENT ============
# Validate WASM
zlayer wasm validate ./myapp.wasm

# Inspect WASM info
zlayer wasm info ghcr.io/myorg/myapp:v1

# Run WASM locally
zlayer run ghcr.io/myorg/myapp:v1
```

### Decision Matrix

| Scenario | Recommendation |
|----------|----------------|
| Existing Docker images | **Container** |
| Database (PostgreSQL, MySQL, etc.) | **Container** |
| Redis, memcached | **Container** |
| Full application with filesystem | **Container** |
| Lightweight API handlers | **WASM** |
| Edge computing | **WASM** |
| Plugin systems | **WASM** |
| Need <10ms cold starts | **WASM** |
| Multi-process applications | **Container** |
| GPU workloads | **Container** |
| Legacy applications | **Container** |
| Polyglot microservices | **Both** (mixed deployment) |

### WASM Target Selection

| WASI Version | Stability | Use When |
|--------------|-----------|----------|
| **Preview 1** (`wasm32-wasip1`) | Stable | Production, broad compatibility |
| **Preview 2** (`wasm32-wasip2`) | Stable | Component model, composability needed |

### Resource Guidelines

| Workload Type | CPU | Memory | Notes |
|---------------|-----|--------|-------|
| Static web server | 0.25 | 64Mi | Nginx, Caddy |
| API service | 0.5-1.0 | 256Mi-512Mi | Typical backend |
| Database | 1.0-4.0 | 1Gi-8Gi | PostgreSQL, MySQL |
| Cache | 0.5 | 256Mi-1Gi | Redis |
| WASM function | 0.25-0.5 | 32Mi-128Mi | Lightweight handlers |
| WASM (compute) | 1.0 | 256Mi | CPU-intensive |

---

## Additional Resources

- [ZLayer Documentation](https://zlayer.dev/docs)
- [ZLayer V1 Specification](./V1_SPEC.md)
- [WASM Implementation Details](./WASM_DONE.md)
- [Example Deployments](./examples/)
- [WASM OCI Artifact Spec](https://tag-runtime.cncf.io/wgs/wasm/deliverables/wasm-oci-artifact/)
- [Wasmtime Documentation](https://docs.wasmtime.dev/)
- [WASI Preview 1 Reference](https://github.com/WebAssembly/WASI/blob/main/legacy/preview1/docs.md)
- [WASI Preview 2 (Component Model)](https://github.com/WebAssembly/component-model)
