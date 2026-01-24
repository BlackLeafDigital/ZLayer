# ZLayer: Outstanding TODOs and Implementation Tasks

This document catalogs all outstanding TODOs, stubs, and unimplemented functionality in the ZLayer codebase following the containerd to youki/libcontainer migration.

---

## Critical (Blocking E2E Tests)

### 1. Image Pulling in YoukiRuntime

- **File**: `/home/zach/github/ZLayer/crates/agent/src/youki_runtime.rs`
- **Line**: 258-267
- **Current State**: Placeholder that creates an empty directory and logs a warning. Does not actually pull any image data from registries.
- **Impact**: E2E tests fail because containers have no rootfs content.

**Current Code**:
```rust
// TODO: Implement full OCI image pull using registry crate
// For now, this serves as a placeholder that expects rootfs to be pre-populated
// The registry crate provides ImagePuller and LayerUnpacker for this purpose

tracing::warn!(
    "Image pull not fully implemented - rootfs at {} should be pre-populated",
    rootfs_path.display()
);
```

**Required Implementation**:
1. Parse the image reference string (e.g., `docker.io/library/nginx:latest`)
2. Use `oci_distribution::Reference::parse()` to parse the reference
3. Create/reuse an `oci_distribution::Client` with proper configuration
4. Handle authentication with `RegistryAuth` (anonymous, basic, or token-based)
5. Call `client.pull_image_manifest(&reference, &auth)` to get the manifest and layer descriptors
6. Iterate over `manifest.layers` and pull each layer blob using `client.pull_blob()` or `client.pull_blob_stream()`
7. Cache each layer blob using the existing `BlobCache` from the registry crate
8. Use `LayerUnpacker` to extract layers in order to the rootfs directory

**Dependencies**:
- `registry` crate: `ImagePuller`, `LayerUnpacker`, `BlobCache`
- `oci-distribution` = "0.10" (workspace dependency)

**Research Notes**:
- The project uses `oci-distribution` v0.10, not the newer `oci-client` crate
- Key API methods: `Client::pull_blob()`, `Client::pull()` for full image
- The `registry::client::ImagePuller` already exists with `pull_blob()` method but lacks manifest fetching
- The `registry::unpack::LayerUnpacker` has full layer unpacking support including whiteouts

**Estimated Complexity**: Complex (requires integrating manifest pulling, layer ordering, and unpacking)

---

## High Priority

### 2. Raft Network Layer (Cluster Consensus)

- **File**: `/home/zach/github/ZLayer/crates/scheduler/src/raft.rs`
- **Lines**: 368, 370, 374, 387, 391, 402, 406, 419-424
- **Current State**: All RPC methods (`append_entries`, `install_snapshot`, `vote`, `full_snapshot`) return `Unreachable` errors with "Network layer not yet implemented" message.

**Current Code (example)**:
```rust
async fn append_entries(...) -> ... {
    // TODO: Implement actual HTTP/gRPC call to target node
    debug!(target = %self.target_addr, "append_entries RPC (stub)");
    Err(RPCError::Unreachable(Unreachable::new(
        &std::io::Error::new(
            std::io::ErrorKind::NotConnected,
            "Network layer not yet implemented",
        ),
    )))
}
```

**Required Implementation**:
1. Implement HTTP or gRPC client for Raft RPCs
2. Add corresponding server endpoints (likely in the API crate)
3. Serialize/deserialize Raft messages using serde
4. Handle connection errors and timeouts appropriately
5. Consider using reqwest or hyper for HTTP transport

**Dependencies**:
- `openraft` = "0.9" (workspace)
- `axum` for server endpoints
- `reqwest` or `hyper` for client calls

**Estimated Complexity**: Complex (requires bidirectional RPC implementation)

---

### 3. Containerd Metrics Collection

- **File**: `/home/zach/github/ZLayer/crates/scheduler/src/metrics.rs`
- **Lines**: 356, 363, 366-374
- **Current State**: Stub that logs a message and returns empty `Vec<ServiceMetrics>`.

**Current Code**:
```rust
// For now, provide a stub that can be fleshed out.
debug!(
    service = service_name,
    socket = %self.socket_path,
    namespace = %self.namespace,
    "Collecting containerd metrics (stub)"
);

// TODO: Implement actual containerd metrics collection
// This requires:
// - containerd_client::connect()
// - task service to get metrics
// - parsing cgroup stats

Ok(Vec::new())
```

**Required Implementation**:
1. Connect to containerd gRPC socket
2. Use containerd task service to list containers
3. Fetch cgroup stats for each container
4. Parse memory/CPU metrics from cgroup data
5. Map to `ServiceMetrics` struct

**Note**: Since the project migrated to youki/libcontainer, this may need to be changed to read metrics directly from cgroups or from libcontainer's API instead of containerd.

**Dependencies**:
- Consider `containerd-client` crate OR direct cgroup reading
- May need to update to use youki/libcontainer metrics instead

**Estimated Complexity**: Medium (if switching to cgroup reading) to Complex (if keeping containerd)

---

### 4. API Deployment Handlers

- **File**: `/home/zach/github/ZLayer/crates/api/src/handlers/deployments.rs`
- **Lines**: 57, 80, 108-109, 131

**TODOs**:
| Line | Function | Current State |
|------|----------|--------------|
| 57 | `list_deployments` | Returns empty vec, TODO: Get from storage |
| 80 | `get_deployment` | Returns NotFound, TODO: Get from storage |
| 108-109 | `create_deployment` | Returns "Not implemented" error |
| 131 | `delete_deployment` | Returns NotFound, TODO: Delete from storage |

**Required Implementation**:
1. Add deployment storage layer (using redb or other backend)
2. Implement CRUD operations for deployments
3. Wire storage to handlers via Axum state injection
4. Add actual deployment orchestration (call scheduler)

**Estimated Complexity**: Medium

---

### 5. API Service Handlers

- **File**: `/home/zach/github/ZLayer/crates/api/src/handlers/services.rs`
- **Lines**: 114, 138, 170, 199

**TODOs**:
| Line | Function | Current State |
|------|----------|--------------|
| 114 | `list_services` | Returns empty vec, TODO: Get from scheduler/storage |
| 138 | `get_service` | Returns NotFound, TODO: Get from scheduler/storage |
| 170 | `scale_service` | Returns NotFound, TODO: Scale via scheduler |
| 199 | `get_service_logs` | Returns NotFound, TODO: Get logs from container runtime |

**Required Implementation**:
1. Add scheduler client/integration
2. Wire service state from agent/scheduler
3. Implement scaling via scheduler commands
4. Connect log retrieval to runtime layer

**Estimated Complexity**: Medium

---

## Medium Priority

### 6. Runtime Binary - Service Startup

- **File**: `/home/zach/github/ZLayer/bin/runtime/src/main.rs`
- **Lines**: 446-448, 471-476

**Current State**:
- Line 446-448: Spec fetching from API not implemented
- Line 471-476: Service startup not implemented

**Current Code**:
```rust
// TODO: Fetch spec from join_info.api_endpoint
println!("[Spec fetching not yet implemented]");

// TODO: Actually start the service
// 1. Create ServiceManager from agent crate
// 2. Scale service to requested replicas
// 3. Register with scheduler for load balancing
println!("\n[Service startup not yet implemented]");
```

**Required Implementation**:
1. Implement HTTP client to fetch spec from API endpoint
2. Integrate with `ServiceManager` from agent crate
3. Call `service_manager.scale(replicas)` to start containers
4. Register service endpoints with scheduler

**Estimated Complexity**: Medium

---

### 7. Runtime Binary - Log Fetching

- **File**: `/home/zach/github/ZLayer/bin/runtime/src/main.rs`
- **Lines**: 616, 624-628

**Current State**: Placeholder that prints helpful message but doesn't fetch logs.

**Required Implementation**:
1. Connect to scheduler to get service instances
2. Use `runtime.container_logs()` to fetch logs from each instance
3. Merge and format log streams
4. Implement `--follow` for streaming

**Estimated Complexity**: Medium

---

### 8. Runtime Binary - Stop Command

- **File**: `/home/zach/github/ZLayer/bin/runtime/src/main.rs`
- **Lines**: 655-659

**Current State**: Prints "[Stop command not yet implemented]"

**Required Implementation**:
1. Send SIGTERM to containers via runtime
2. Wait for graceful shutdown with timeout
3. Send SIGKILL if timeout exceeded
4. Handle `--force` flag

**Estimated Complexity**: Simple (runtime already has stop_container)

---

### 9. DevCtl - Deployment Inspection

- **File**: `/home/zach/github/ZLayer/tools/devctl/src/main.rs`
- **Lines**: 401-406

**Current State**: Prints placeholder message, suggests using curl.

**Required Implementation**:
1. Add HTTP client to fetch deployment info from API
2. Parse and display results in requested format (json/yaml)

**Estimated Complexity**: Simple

---

### 10. DevCtl - Local Runner

- **File**: `/home/zach/github/ZLayer/tools/devctl/src/main.rs`
- **Lines**: 515-516

**Current State**: Prints "[Local runner not yet fully implemented]" and suggests docker commands.

**Required Implementation**:
1. Create containers using runtime (youki or docker)
2. Apply port mappings and environment variables
3. Manage container lifecycle

**Estimated Complexity**: Medium

---

## Low Priority / Nice to Have

### 11. Health Monitor Handle Storage

- **File**: `/home/zach/github/ZLayer/crates/agent/src/service.rs`
- **Line**: 87-88

**Current State**: Creates health monitor but doesn't store the handle.

**Current Code**:
```rust
// TODO: store monitor handle
let _monitor = monitor;
```

**Required Implementation**:
- Store monitor handle in Container struct or separate map
- Allow cancellation when container stops
- Enable health status queries

**Estimated Complexity**: Simple

---

### 12. Unhealthy State Notification

- **File**: `/home/zach/github/ZLayer/crates/agent/src/health.rs`
- **Line**: 177

**Current State**: Breaks loop when max retries exceeded but doesn't notify.

**Current Code**:
```rust
if failures >= self.retries {
    // TODO: notify of unhealthy state
    break;
}
```

**Required Implementation**:
- Add callback or channel for health state changes
- Notify service manager of unhealthy containers
- Trigger replacement/restart policy

**Estimated Complexity**: Simple

---

### 13. Non-Unix Symlink Handling

- **File**: `/home/zach/github/ZLayer/crates/registry/src/unpack.rs`
- **Lines**: 405-411

**Current State**: Logs warning on non-Unix platforms when encountering symlinks.

**Note**: This is platform-specific and may be acceptable as-is for Linux-only deployment.

**Estimated Complexity**: Low priority (Linux-only project)

---

## Implementation Order Recommendation

1. **Image Pulling** (#1) - Critical for any container functionality
2. **API Deployment/Service Handlers** (#4, #5) - Required for management
3. **Runtime Service Startup** (#6) - Required for actual operation
4. **Raft Network Layer** (#2) - Required for multi-node clusters
5. **Metrics Collection** (#3) - Required for autoscaling
6. **Remaining CLI commands** (#7, #8, #9, #10)
7. **Health monitoring improvements** (#11, #12)

---

## Registry Crate Analysis

The `registry` crate already provides useful components:

### `client.rs` - ImagePuller
```rust
pub struct ImagePuller {
    client: oci_distribution::Client,
    cache: Arc<BlobCache>,
    concurrency_limit: Arc<Semaphore>,
}

// Already has:
// - pull_blob() - pulls and caches individual blobs
// - BlobCache integration

// Missing:
// - pull_manifest() - needs to fetch manifest from registry
// - pull_image() - orchestrate full image pull (manifest + layers)
```

### `unpack.rs` - LayerUnpacker
```rust
pub struct LayerUnpacker {
    rootfs_dir: PathBuf,
    deleted_paths: HashSet<PathBuf>,
}

// Already has:
// - unpack_layer() - supports gzip, zstd, plain tar
// - unpack_layers() - unpacks multiple layers in order
// - Whiteout handling (.wh. files)
// - Path traversal security
```

### Integration Plan for Image Pull

```rust
// In youki_runtime.rs, pull_image should:

async fn pull_image(&self, image: &str) -> Result<()> {
    // 1. Parse image reference
    let reference: oci_distribution::Reference = image.parse()?;

    // 2. Create client with auth
    let client = oci_distribution::Client::new(ClientConfig::default());
    let auth = RegistryAuth::Anonymous; // or from config

    // 3. Pull manifest
    let (manifest, _digest) = client.pull_image_manifest(&reference, &auth).await?;

    // 4. Pull layers using ImagePuller
    let puller = ImagePuller::new(Arc::new(blob_cache));
    let mut layers = Vec::new();

    for layer in manifest.layers() {
        let digest = layer.digest().to_string();
        let media_type = layer.media_type().to_string();
        let data = puller.pull_blob(image, &digest, &auth).await?;
        layers.push((data, media_type));
    }

    // 5. Unpack layers to rootfs
    let mut unpacker = LayerUnpacker::new(self.rootfs_path(image));
    unpacker.unpack_layers(&layers).await?;

    Ok(())
}
```

---

## Dependencies to Consider Updating

| Crate | Current | Consider |
|-------|---------|----------|
| oci-distribution | 0.10 | 0.11+ or migrate to oci-client 0.15 |
| oci-spec | 0.6 | 0.8+ for latest OCI spec features |

The `oci-distribution` crate has been renamed to `oci-client` (maintained by ORAS project). Consider migrating for continued support.

---

*Last Updated: 2026-01-23*
