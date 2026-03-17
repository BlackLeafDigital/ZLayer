# ZLayer CI/CD Pipeline

## Workflow Chain

```
ci.yaml ──(tag/version)──> e2e.yml ──(pass)──> build.yml ──(artifacts)──> release.yml
                                                                              │
                                                                   (dispatches GitHub)
                                                                              │
                                                              .github/workflows/release.yml
```

---

## 1. ci.yaml (CI) [Forgejo]

Triggers: push to main/dev, PRs, manual dispatch

```
check ──> test ──> build
  │         │        │
  │         │        └─> Upload: zlayer + zlayer-build binaries
  │         │
  └──> feature-tests
            │
            └──> trigger-e2e (dispatches e2e.yml)
                 cache-cleanup
```

| Job            | What it does                                      |
|----------------|---------------------------------------------------|
| check          | fmt, cargo check --workspace, clippy --workspace  |
| test           | unit, integration, doc tests                      |
| build          | cargo build --release (runtime + zlayer-build)     |
| feature-tests  | Matrix: join, serve, deploy, full feature combos   |
| trigger-e2e    | Dispatches e2e.yml                                 |

---

## 2. e2e.yml (E2E Tests) [Forgejo]

Triggers: dispatched from ci.yaml

```
e2e-tests ──────────────┐
wasm-e2e-tests ─────────┤
docker-e2e-tests ───────┼──> trigger-build (dispatches build.yml)
manager-tests ──────────┘
```

---

## 3. build.yml (Multi-Platform Builds) [Forgejo]

Triggers: dispatched from e2e.yml (with version + sha)

```
build-linux-amd64 ──> build-linux-arm64 ──> build-macos-amd64 ──> build-macos-arm64
     │                                                                    │
     │  (each builds: zlayer + zlayer-build binaries)                     │
     │                                                                    │
     └────────────────────────> verify <──────────────────────────────────┘
                                  │
                    ┌─────────────┼──────────────┐
                    │             │              │
              image-build   upload-temp     (gate)
              (zlayer-build  packages          │
              + buildah                        │
              + ZImagefiles)                   │
                    │             │              │
                    └─────────────┼──────────────┘
                                  │
                           dispatch-release
                           (dispatches release.yml)
```

### Binary Artifacts (per platform)

| Artifact                        | Platforms                          |
|---------------------------------|------------------------------------|
| zlayer-{os}-{arch}.tar.gz       | linux-amd64, linux-arm64, darwin-amd64, darwin-arm64 |
| zlayer-build-{os}-{arch}.tar.gz | linux-amd64, linux-arm64, darwin-amd64, darwin-arm64 |

### Image Build Job

All images built with zlayer-build + buildah (no Docker daemon needed).
Build-only (no push) -- verification that images build successfully.

| Image          | ZImagefile                          | Notes                    |
|----------------|-------------------------------------|--------------------------|
| zlayer-node    | docker/ZImagefile.zlayer-node       | Downloads pre-built zlayer-runtime binary |
| zlayer-web     | docker/ZImagefile.zlayer-web        | Compiles from source in-image    |
| zlayer-manager | docker/ZImagefile.zlayer-manager    | Compiles from source in-image    |

---

## 4. release.yml (Release + Publishing) [Forgejo]

Triggers: dispatched from build.yml (with version + sha + artifact_base_url)

```
detect ──────────────────────────────────────────────────────────┐
   │                                                             │
   ├─(binary=true)──> upload-binaries ──> merge-to-main ────────┤
   │                                          │                  │
   │                                          ├──> publish-all-crates
   │                                          │
   │                                          ├──> publish-rust-sdk
   │                                          ├──> publish-python-sdk
   │                                          ├──> publish-typescript-sdk
   │                                          ├──> publish-go-sdk
   │                                          ├──> publish-c-sdk
   │                                          │
   │                                          └──> docker (zlayer-build + buildah)
   │                                                  │
   │                                                  └──> trigger-github
   │
   ├─(crate_name)──> publish-crate
   │
   └─(sdk_name)──> publish-{lang}-sdk
```

### Docker Job (Forgejo release.yml)

Uses zlayer-build + buildah to build and push OCI images:

| Image          | ZImagefile                          | Registries              |
|----------------|-------------------------------------|-------------------------|
| zlayer-node    | docker/ZImagefile.zlayer-node       | DockerHub + GHCR        |
| zlayer-web     | docker/ZImagefile.zlayer-web        | DockerHub + GHCR        |
| zlayer-manager | docker/ZImagefile.zlayer-manager    | DockerHub + GHCR        |

Flow:
1. Download zlayer-build from artifact URL
2. Install buildah via apt
3. `buildah login` to DockerHub and GHCR
4. `zlayer-build build --no-tui -f <zimagefile> -t <tags> --format oci .`
5. `buildah push` each tag to each registry

---

## 5. release.yml (GitHub Mirror) [GitHub Actions]

Triggers: repository_dispatch from Forgejo, or manual

```
setup ──> release (create GitHub Release with all artifacts)
              │
              └──> docker (zlayer-build + buildah ──> push to GHCR)
```

### Artifacts in GitHub Release

| Artifact                              | Description              |
|---------------------------------------|--------------------------|
| zlayer-{version}-linux-amd64.tar.gz   | Runtime binary           |
| zlayer-{version}-linux-arm64.tar.gz   | Runtime binary           |
| zlayer-{version}-darwin-amd64.tar.gz  | Runtime binary           |
| zlayer-{version}-darwin-arm64.tar.gz  | Runtime binary           |
| zlayer-build-{version}-linux-amd64.tar.gz  | Builder binary      |
| zlayer-build-{version}-linux-arm64.tar.gz  | Builder binary      |
| zlayer-build-{version}-darwin-amd64.tar.gz | Builder binary      |
| zlayer-build-{version}-darwin-arm64.tar.gz | Builder binary      |

### Docker Job (GitHub release.yml)

Pushes to GHCR only (DockerHub handled by Forgejo release):

| Image          | ZImagefile                          | Registry                |
|----------------|-------------------------------------|-------------------------|
| zlayer-node    | docker/ZImagefile.zlayer-node       | ghcr.io/blackleafdigital |
| zlayer-web     | docker/ZImagefile.zlayer-web        | ghcr.io/blackleafdigital |
| zlayer-manager | docker/ZImagefile.zlayer-manager    | ghcr.io/blackleafdigital |

---

## All Binaries

| Binary         | Crate             | Size  | Dependencies                    | Purpose                        |
|----------------|-------------------|-------|---------------------------------|--------------------------------|
| zlayer-runtime | bin/runtime       | ~50MB | Everything (agent, scheduler, proxy, wasm, overlay) | Full orchestration runtime |
| zlayer-build   | bin/zlayer-build  | ~4MB  | zlayer-builder, zlayer-spec     | Lightweight image builder CLI  |
| zlayer         | bin/zlayer        | ~5MB  | zlayer-builder, zlayer-spec, ratatui | Main CLI (TUI + runtime passthrough) |

---

## All Container Images

| Image          | Contents                     | Ports    | Base              |
|----------------|------------------------------|----------|-------------------|
| zlayer-node    | zlayer-runtime binary + containerd | 80, 443  | ubuntu:22.04      |
| zlayer-web     | zlayer-web Leptos SSR + WASM | 3000     | debian:bookworm-slim |
| zlayer-manager | zlayer-manager Leptos SSR    | 9120     | debian:bookworm-slim |

---

## Build Files

| File                              | Format     | Used By       | Purpose                  |
|-----------------------------------|------------|---------------|--------------------------|
| docker/ZImagefile.zlayer-node     | ZImagefile | zlayer-build  | zlayer-node image        |
| docker/ZImagefile.zlayer-web      | ZImagefile | zlayer-build  | zlayer-web image         |
| docker/ZImagefile.zlayer-manager  | ZImagefile | zlayer-build  | zlayer-manager image     |
| docker/Dockerfile.zlayer-node     | Dockerfile | (legacy)      | zlayer-node (kept for reference) |
| docker/Dockerfile.zlayer-web      | Dockerfile | (legacy)      | zlayer-web (kept for reference)  |
| docker/Dockerfile.zlayer-manager  | Dockerfile | (legacy)      | zlayer-manager (kept for reference) |
| docker/Dockerfile.arm64-builder   | Dockerfile | cross-compile | ARM64 build base         |
