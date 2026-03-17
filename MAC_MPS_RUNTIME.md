# Apple Silicon MPS GPU Runtime for ZLayer

## Comprehensive Implementation Guide

---

# Part 1: Executive Summary

## What This Document Covers

This document is the complete engineering specification for adding native Apple Silicon GPU (Metal/MPS) support to ZLayer on macOS. It covers two complementary runtime approaches, their full implementation details against the existing ZLayer codebase, and the integration points that connect them to the scheduler, proxy, overlay, and service management subsystems.

After implementing what is described here, ZLayer will be the only container orchestration platform capable of providing **100% native Metal Performance Shaders (MPS) access** to isolated workloads on macOS. Docker Desktop cannot do this -- it runs a Linux VM via Virtualization.framework, which only exposes Virtio GPU 2D to guests. No Metal, no MPS, no GPU compute.

## Why ZLayer Can Beat Docker on macOS GPU

Docker Desktop's architecture on macOS is fundamentally incapable of GPU passthrough:

```
Docker Desktop on macOS:
  macOS Host
    -> Virtualization.framework Linux VM
      -> Virtio GPU 2D (no Metal, no compute)
        -> Linux container (no GPU access)
```

ZLayer's architecture bypasses the VM entirely:

```
ZLayer Seatbelt Sandbox on macOS:
  macOS Host
    -> fork() + sandbox_init() + exec()
      -> Direct IOKit -> Metal.framework -> Apple GPU hardware
        -> 100% native MPS performance
```

The key insight: Apple GPUs are accessed through IOKit and Metal.framework, not through device files in `/dev/`. The Seatbelt sandbox (macOS Mandatory Access Control) can allow IOKit GPU access while restricting everything else. This gives us process-level isolation with full GPU performance.

## The Two Approaches

### Approach A: Seatbelt Sandbox Runtime (Primary)

- **Process model**: fork + sandbox_init + exec (not containers, not VMs)
- **GPU performance**: 100% native Metal/MPS
- **Isolation**: Seatbelt MAC (filesystem, network, IPC restrictions)
- **Overhead**: Near-zero (kernel-level policy check per syscall)
- **Limitation**: Can only run macOS-native binaries
- **Best for**: PyTorch MPS, MLX, TensorFlow Metal, CoreML, any Metal workload

### Approach B: libkrun VM Runtime (Backup)

- **Process model**: Lightweight microVM via Hypervisor.framework
- **GPU performance**: 25-30% native (Venus-Vulkan) or 95-98% native (API remoting/ggml)
- **Isolation**: VM-level (full hardware boundary)
- **Overhead**: ~100MB RAM per VM, 100-300ms boot
- **Limitation**: One GPU container at a time, Apple Silicon only (no Intel Macs)
- **Best for**: Linux container images that need GPU, full OCI compatibility

---

# Part 2: Architecture Overview

## High-Level Architecture

```
                        +---------------------------+
                        |     ZLayer Deployment      |
                        |       Spec (YAML)          |
                        +---------------------------+
                                    |
                                    v
                        +---------------------------+
                        |     ServiceManager         |
                        |  crates/zlayer-agent/      |
                        |  src/service.rs            |
                        +---------------------------+
                                    |
                      +-------------+-------------+
                      |             |             |
                      v             v             v
              +-----------+  +-----------+  +-----------+
              |  Runtime   |  |  Runtime   |  |  Runtime   |
              |  Trait     |  |  Trait     |  |  Trait     |
              +-----------+  +-----------+  +-----------+
              | YoukiRuntime| |DockerRuntime| | WasmRuntime|
              | (Linux)    | | (cross-plat)| | (cross-plat)|
              +-----------+  +-----------+  +-----------+
                      |
        +-------------+-------------+
        |                           |
        v                           v
+-----------------+        +-----------------+
| SandboxRuntime  |        |  VmRuntime      |
| (macOS primary) |        | (macOS backup)  |
+-----------------+        +-----------------+
| fork()          |        | libkrun API     |
| sandbox_init()  |        | Hypervisor.fw   |
| Seatbelt .sb    |        | Venus/ggml GPU  |
| APFS clonefile  |        | TSI networking  |
| Metal/MPS 100%  |        | Full Linux OCI  |
+-----------------+        +-----------------+
```

## How They Fit Into ZLayer's Existing Runtime Architecture

The Runtime trait at `crates/zlayer-agent/src/runtime.rs:59-124` defines 14 async methods. Both new macOS runtimes implement this trait. The ServiceManager at `crates/zlayer-agent/src/service.rs` interacts purely through the Runtime trait -- it has no Linux namespace assumptions. The overlay manager already handles the "no PID" case by skipping container attachment and falling back to `get_container_ip()`.

### Platform Detection and Automatic Runtime Selection

The `create_auto_runtime()` function at `crates/zlayer-agent/src/lib.rs:226-284` currently has this flow:

```
Linux:  Youki -> Docker -> Error
Others: Docker -> Error
```

After implementation, the macOS flow becomes:

```
macOS + GPU workload + vendor:"apple" + mode:"native":
    SandboxRuntime -> Error

macOS + GPU workload + vendor:"apple" + mode:"vm":
    VmRuntime -> Error

macOS + non-GPU workload:
    SandboxRuntime -> Docker -> Error

macOS (auto):
    SandboxRuntime -> Docker -> Error

Linux (unchanged):
    Youki -> Docker -> Error
```

---

# Part 3: Approach A -- Seatbelt Sandbox Runtime (Primary)

## 3.1 How It Works

### Process Model

The Seatbelt sandbox runtime does NOT use containers or VMs. It uses the macOS process isolation primitive: `fork()` a child process, apply a Seatbelt sandbox profile via `sandbox_init()`, then `exec()` the workload binary. The sandbox profile is a Scheme-based DSL that the XNU kernel enforces at the syscall level.

```
ZLayer Runtime Process
    |
    +-- fork()
         |
         +-- [child process]
              |
              +-- sandbox_init(sbpl_profile)   // Apply Seatbelt restrictions
              +-- chdir(rootfs_path)           // Change to container rootfs
              +-- setrlimit(...)               // Apply resource limits
              +-- redirect stdout/stderr       // Log capture
              +-- exec(workload_binary, args)  // Replace process image
```

The sandbox survives `exec()` and is inherited by all child processes. Once applied, it cannot be removed or loosened.

### Seatbelt Profile Generation Per Service

Each ZLayer service gets a unique Seatbelt profile generated from its `ServiceSpec`. The profile is parameterized with:

- **Rootfs path**: The APFS-cloned container filesystem
- **Writable directories**: Volume mounts from the spec
- **Network rules**: Derived from `spec.endpoints` (which ports to bind/connect)
- **GPU access**: Enabled when `spec.resources.gpu.vendor == "apple"`
- **IPC restrictions**: Minimal by default

### Filesystem Isolation

macOS has no mount namespaces. Instead, ZLayer uses two mechanisms:

1. **APFS `clonefile()`**: Creates a copy-on-write clone of the rootfs directory. This is nearly instantaneous (metadata-only operation) and uses zero additional disk space until files are modified.

2. **Seatbelt `(subpath ...)` rules**: Restricts the process to only read/write within its cloned rootfs and explicitly allowed paths. This is enforced at the kernel level and cannot be bypassed.

### Network Restrictions

macOS has no network namespaces. All sandboxed processes share the host network stack. Seatbelt provides:

- `(allow network-bind (local ip "localhost:PORT"))` -- restrict which ports the process can bind
- `(allow network-outbound (remote ip "HOST:PORT"))` -- restrict outbound connections
- `(deny network-inbound)` -- deny all inbound (ZLayer proxy handles routing)

### Metal/MPS GPU Access

Metal GPU access on macOS requires three categories of permissions:

1. **IOKit user clients**: `IOSurfaceRoot`, `IOAccelDevice2`, `IOAccelContext2`, `IOAccelSharedUserClient2`, `AGXAccelerator` (Apple Silicon GPU driver)
2. **Mach services**: `com.apple.MTLCompilerService` (shader compilation), `com.apple.AGXCompilerService.*`
3. **Filesystem**: Metal.framework, MetalPerformanceShaders.framework, `/Library/GPUBundles`, `/System/Library/Extensions`

MPS specifically is better suited for sandboxed environments because MPS kernels are pre-compiled as part of the OS -- no MTLCompilerService needed at runtime for MPS-only workloads.

### Resource Limits

macOS lacks cgroups. The available resource controls are:

- **`setrlimit(RLIMIT_NOFILE, ...)`**: Limits open file descriptors (enforced)
- **`setrlimit(RLIMIT_CPU, ...)`**: Limits total CPU time, sends SIGXCPU then SIGKILL (enforced)
- **Memory watchdog thread**: Polls `mach_task_basic_info` and sends SIGKILL if RSS exceeds threshold (not kernel-enforced, but effective)
- **`RLIMIT_RSS` / `RLIMIT_AS`**: NOT enforced on macOS

## 3.2 Implementation Guide -- Step by Step

### Step 1: GPU Detection (`gpu_detector.rs` macOS Branch)

**File**: `crates/zlayer-agent/src/gpu_detector.rs`

**Why**: The current `detect_gpus()` function scans `/sys/bus/pci/devices` which only exists on Linux. On macOS, Apple GPUs are accessed through IOKit. We need a `cfg(target_os = "macos")` branch that uses `system_profiler` to detect Apple GPU hardware.

**Code**:

```rust
// At the top of gpu_detector.rs, add this after the existing detect_gpus():

/// Scan the system for GPU devices via sysfs PCI enumeration (Linux)
#[cfg(target_os = "linux")]
pub fn detect_gpus() -> Vec<GpuInfo> {
    // ... existing Linux implementation unchanged ...
}

/// Detect Apple GPU hardware via system_profiler (macOS)
#[cfg(target_os = "macos")]
pub fn detect_gpus() -> Vec<GpuInfo> {
    detect_apple_gpus()
}

/// Fallback for unsupported platforms
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn detect_gpus() -> Vec<GpuInfo> {
    Vec::new()
}

#[cfg(target_os = "macos")]
fn detect_apple_gpus() -> Vec<GpuInfo> {
    // Use system_profiler to get GPU info as JSON
    let output = match std::process::Command::new("system_profiler")
        .args(["SPDisplaysDataType", "-json"])
        .output()
    {
        Ok(out) if out.status.success() => out,
        _ => {
            tracing::warn!("Failed to run system_profiler for GPU detection");
            return Vec::new();
        }
    };

    let json_str = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = match serde_json::from_str(&json_str) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "Failed to parse system_profiler JSON output");
            return Vec::new();
        }
    };

    let mut gpus = Vec::new();

    // system_profiler returns: { "SPDisplaysDataType": [ { ... }, ... ] }
    if let Some(displays) = parsed
        .get("SPDisplaysDataType")
        .and_then(|v| v.as_array())
    {
        for (index, display) in displays.iter().enumerate() {
            let model = display
                .get("sppci_model")
                .and_then(|v| v.as_str())
                .unwrap_or("Apple GPU")
                .to_string();

            let vendor = display
                .get("sppci_vendor")
                .and_then(|v| v.as_str())
                .map(|v| {
                    if v.to_lowercase().contains("apple") {
                        "apple".to_string()
                    } else if v.to_lowercase().contains("amd") {
                        "amd".to_string()
                    } else if v.to_lowercase().contains("intel") {
                        "intel".to_string()
                    } else {
                        "unknown".to_string()
                    }
                })
                .unwrap_or_else(|| "apple".to_string());

            // For Apple Silicon, VRAM = system RAM (unified memory architecture)
            let memory_mb = detect_apple_unified_memory_mb();

            // Apple GPUs have no /dev/ device paths -- access is via IOKit
            let device_path = format!("iokit://AppleGPU/{}", index);

            let chip_type = display
                .get("sppci_device_type")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            // Construct a synthetic PCI bus ID from the index
            let pci_bus_id = format!("apple:{}", index);

            gpus.push(GpuInfo {
                pci_bus_id,
                vendor,
                model: if chip_type.is_empty() {
                    model
                } else {
                    format!("{} ({})", model, chip_type)
                },
                memory_mb,
                device_path,
                render_path: None, // No render nodes on macOS
            });
        }
    }

    gpus
}

/// Detect total system memory on macOS (unified memory = GPU memory on Apple Silicon)
#[cfg(target_os = "macos")]
fn detect_apple_unified_memory_mb() -> u64 {
    let output = std::process::Command::new("sysctl")
        .args(["-n", "hw.memsize"])
        .output();

    match output {
        Ok(out) if out.status.success() => {
            let mem_str = String::from_utf8_lossy(&out.stdout);
            mem_str
                .trim()
                .parse::<u64>()
                .map(|bytes| bytes / (1024 * 1024))
                .unwrap_or(0)
        }
        _ => 0,
    }
}
```

The function reuses the existing `GpuInfo` struct unchanged. Apple GPUs get `vendor: "apple"`, a synthetic `pci_bus_id` (since there is no PCI bus on Apple Silicon), and `device_path` prefixed with `iokit://` to indicate the access mechanism. The `memory_mb` reports total system RAM because Apple Silicon uses unified memory -- the GPU and CPU share the same physical memory pool.

### Step 2: GpuSpec Vendor Extension (`types.rs`)

**File**: `crates/zlayer-spec/src/types.rs` (around line 540-557)

**Why**: The current `GpuSpec` struct supports `count` and `vendor` fields. For macOS GPU support, we need an additional `mode` field that selects between "native" (Seatbelt sandbox, 100% Metal performance) and "vm" (libkrun, Linux container compat).

**Code** -- replace the existing `GpuSpec` struct:

```rust
/// GPU resource specification
///
/// Supported vendors:
/// - `nvidia` - NVIDIA GPUs via NVIDIA Container Toolkit (default)
/// - `amd` - AMD GPUs via ROCm (/dev/kfd + /dev/dri/renderD*)
/// - `intel` - Intel GPUs via VAAPI/i915 (/dev/dri/renderD*)
/// - `apple` - Apple Silicon GPU via Metal/MPS (macOS only)
///
/// Unknown vendors fall back to DRI render node passthrough.
///
/// For Apple GPUs, the `mode` field controls the isolation mechanism:
/// - `native` - Seatbelt sandbox with direct Metal/MPS access (100% performance)
/// - `vm` - libkrun microVM with GPU forwarding (Linux container compat)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Validate)]
#[serde(deny_unknown_fields)]
pub struct GpuSpec {
    /// Number of GPUs to request
    #[serde(default = "default_gpu_count")]
    pub count: u32,
    /// GPU vendor (`nvidia`, `amd`, `intel`, `apple`) - defaults to `nvidia`
    #[serde(default = "default_gpu_vendor")]
    pub vendor: String,
    /// GPU access mode (only relevant for `apple` vendor)
    /// - `native`: Direct Metal/MPS via Seatbelt sandbox (default for apple)
    /// - `vm`: libkrun microVM with GPU forwarding
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
}
```

This is backwards compatible -- existing specs without `mode` continue to work. The `mode` field is only meaningful when `vendor` is `"apple"`.

### Step 3: RuntimeConfig + CLI Variants

#### 3a. RuntimeConfig Variants

**File**: `crates/zlayer-agent/src/lib.rs` (around line 71-97)

**Why**: We need new `RuntimeConfig` variants for the two macOS runtimes so `create_runtime()` can dispatch to them.

**Code** -- add to the `RuntimeConfig` enum:

```rust
#[derive(Debug, Clone)]
pub enum RuntimeConfig {
    Auto,
    Mock,
    #[cfg(target_os = "linux")]
    Youki(YoukiConfig),
    #[cfg(feature = "docker")]
    Docker,
    #[cfg(feature = "wasm")]
    Wasm(WasmConfig),
    /// macOS Seatbelt sandbox runtime for native Metal/MPS GPU access
    #[cfg(target_os = "macos")]
    MacSandbox,
    /// macOS libkrun VM runtime for Linux container compat with GPU forwarding
    #[cfg(target_os = "macos")]
    MacVm,
}
```

Then update `create_runtime()` at line 198:

```rust
pub async fn create_runtime(config: RuntimeConfig) -> Result<Arc<dyn Runtime + Send + Sync>> {
    match config {
        RuntimeConfig::Auto => create_auto_runtime().await,
        RuntimeConfig::Mock => Ok(Arc::new(MockRuntime::new())),
        #[cfg(target_os = "linux")]
        RuntimeConfig::Youki(youki_config) => {
            let runtime = YoukiRuntime::new(youki_config).await?;
            Ok(Arc::new(runtime))
        }
        #[cfg(feature = "docker")]
        RuntimeConfig::Docker => {
            let runtime = DockerRuntime::new().await?;
            Ok(Arc::new(runtime))
        }
        #[cfg(feature = "wasm")]
        RuntimeConfig::Wasm(wasm_config) => {
            let runtime = WasmRuntime::new(wasm_config).await?;
            Ok(Arc::new(runtime))
        }
        #[cfg(target_os = "macos")]
        RuntimeConfig::MacSandbox => {
            let runtime = runtimes::SandboxRuntime::new().await?;
            Ok(Arc::new(runtime))
        }
        #[cfg(target_os = "macos")]
        RuntimeConfig::MacVm => {
            let runtime = runtimes::VmRuntime::new().await?;
            Ok(Arc::new(runtime))
        }
    }
}
```

And update `create_auto_runtime()` at line 226 to add a macOS branch:

```rust
async fn create_auto_runtime() -> Result<Arc<dyn Runtime + Send + Sync>> {
    tracing::info!("Auto-selecting container runtime");

    // On Linux, use bundled libcontainer runtime
    #[cfg(target_os = "linux")]
    {
        match YoukiRuntime::new(YoukiConfig::default()).await {
            Ok(runtime) => {
                tracing::info!("Using bundled libcontainer runtime (Linux-native, no daemon)");
                return Ok(Arc::new(runtime));
            }
            Err(e) => {
                tracing::warn!(error = %e, "Failed to initialize libcontainer runtime, trying Docker");
            }
        }
    }

    // On macOS, prefer Seatbelt sandbox runtime
    #[cfg(target_os = "macos")]
    {
        match runtimes::SandboxRuntime::new().await {
            Ok(runtime) => {
                tracing::info!("Using macOS Seatbelt sandbox runtime (native Metal/MPS)");
                return Ok(Arc::new(runtime));
            }
            Err(e) => {
                tracing::warn!(error = %e, "Failed to initialize Seatbelt sandbox runtime, trying Docker");
            }
        }
    }

    // Fallback: try Docker
    #[cfg(feature = "docker")]
    {
        if is_docker_available().await {
            tracing::info!("Selected Docker runtime");
            let runtime = DockerRuntime::new().await?;
            return Ok(Arc::new(runtime));
        }
        tracing::debug!("Docker daemon not available");
    }

    // No runtime available -- platform-specific error messages
    #[cfg(all(target_os = "macos", feature = "docker"))]
    {
        Err(AgentError::Configuration(
            "Seatbelt sandbox runtime failed to initialize and Docker daemon is not available."
                .to_string(),
        ))
    }

    #[cfg(all(target_os = "macos", not(feature = "docker")))]
    {
        Err(AgentError::Configuration(
            "Seatbelt sandbox runtime failed to initialize. Enable 'docker' feature for fallback."
                .to_string(),
        ))
    }

    // ... existing Linux and other platform error branches unchanged ...
}
```

#### 3b. CLI RuntimeType

**File**: `bin/runtime/src/cli.rs` (around line 51-61)

**Why**: The CLI needs to expose the new runtime types so operators can explicitly select them.

**Code**:

```rust
#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum RuntimeType {
    /// Auto-detect the best available runtime
    Auto,
    /// Docker runtime (requires running Docker daemon)
    #[cfg(feature = "docker")]
    Docker,
    /// Youki runtime for production deployments (Linux only)
    #[cfg(target_os = "linux")]
    Youki,
    /// macOS Seatbelt sandbox runtime (native Metal/MPS GPU access)
    #[cfg(target_os = "macos")]
    MacSandbox,
    /// macOS libkrun VM runtime (Linux containers with GPU forwarding)
    #[cfg(target_os = "macos")]
    MacVm,
}
```

### Step 4: The SandboxRuntime Struct and Runtime Trait Implementation

**File**: `crates/zlayer-agent/src/runtimes/macos_sandbox.rs` (NEW)

**Why**: This is the core of Approach A. The `SandboxRuntime` struct manages macOS sandboxed processes that implement ZLayer's `Runtime` trait.

#### Data Structures

```rust
//! macOS Seatbelt sandbox runtime
//!
//! Implements the Runtime trait using macOS process isolation:
//! - fork() + sandbox_init() + exec() for process creation
//! - Seatbelt .sb profiles for mandatory access control
//! - APFS clonefile() for copy-on-write filesystem isolation
//! - Direct Metal/MPS GPU access at 100% native performance

use crate::cgroups_stats::ContainerStats;
use crate::error::{AgentError, Result};
use crate::runtime::{ContainerId, ContainerState, Runtime};
use std::collections::HashMap;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use zlayer_spec::ServiceSpec;

/// State directory layout:
///   /var/lib/zlayer/containers/{container_id}/
///     rootfs/          -- APFS-cloned container filesystem
///     config.json      -- Serialized ServiceSpec + sandbox config
///     profile.sb       -- Generated Seatbelt profile
///     stdout.log       -- Captured stdout
///     stderr.log       -- Captured stderr
///     pid              -- PID file
const DEFAULT_STATE_DIR: &str = "/var/lib/zlayer/containers";

/// Metadata for a running sandboxed process
#[derive(Debug)]
struct SandboxedProcess {
    /// Process ID of the sandboxed child
    pid: u32,
    /// Container state
    state: ContainerState,
    /// Path to the container's state directory
    state_dir: PathBuf,
    /// Path to the cloned rootfs
    rootfs_dir: PathBuf,
    /// When the process was started
    started_at: Instant,
    /// Memory limit in bytes (for watchdog)
    memory_limit: Option<u64>,
    /// Handle to memory watchdog task
    watchdog_handle: Option<tokio::task::JoinHandle<()>>,
}

/// macOS Seatbelt sandbox runtime
pub struct SandboxRuntime {
    /// Base state directory for all containers
    state_dir: PathBuf,
    /// Active sandboxed processes
    processes: Arc<RwLock<HashMap<String, SandboxedProcess>>>,
}

impl SandboxRuntime {
    pub async fn new() -> Result<Self> {
        Self::with_state_dir(PathBuf::from(DEFAULT_STATE_DIR)).await
    }

    pub async fn with_state_dir(state_dir: PathBuf) -> Result<Self> {
        // Create state directory if it doesn't exist
        tokio::fs::create_dir_all(&state_dir)
            .await
            .map_err(|e| AgentError::Configuration(format!(
                "Failed to create state directory {}: {}",
                state_dir.display(), e
            )))?;

        Ok(Self {
            state_dir,
            processes: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    /// Generate a container directory name from a ContainerId
    fn container_dir_name(id: &ContainerId) -> String {
        format!("{}-rep-{}", id.service, id.replica)
    }
}
```

#### Runtime Trait Implementation -- All 14 Methods

Each method below explains what it does on macOS and provides the implementation:

**`pull_image`** -- Reuses zlayer-registry to extract a rootfs. On macOS, we pull the OCI image layers and extract them to a base rootfs directory that will be APFS-cloned per container.

```rust
#[async_trait::async_trait]
impl Runtime for SandboxRuntime {
    async fn pull_image(&self, image: &str) -> Result<()> {
        self.pull_image_with_policy(image, zlayer_spec::PullPolicy::IfNotPresent)
            .await
    }

    async fn pull_image_with_policy(
        &self,
        image: &str,
        policy: zlayer_spec::PullPolicy,
    ) -> Result<()> {
        let image_dir = self.state_dir.join("images").join(
            image.replace('/', "_").replace(':', "_")
        );

        match policy {
            zlayer_spec::PullPolicy::Always => { /* always re-pull */ }
            zlayer_spec::PullPolicy::IfNotPresent => {
                if image_dir.join("rootfs").exists() {
                    tracing::debug!(image = %image, "Image already present, skipping pull");
                    return Ok(());
                }
            }
            zlayer_spec::PullPolicy::Never => {
                if !image_dir.join("rootfs").exists() {
                    return Err(AgentError::PullFailed {
                        image: image.to_string(),
                        reason: "Image not present and pull policy is Never".to_string(),
                    });
                }
                return Ok(());
            }
        }

        tracing::info!(image = %image, "Pulling image for macOS sandbox runtime");

        // Use zlayer-registry to pull and extract OCI image layers
        // The registry handles authentication, layer caching, and deduplication
        let cache = zlayer_registry::BlobCache::open(
            self.state_dir.join("blobs.redb").to_str().unwrap_or("/tmp/blobs.redb")
        ).map_err(|e| AgentError::PullFailed {
            image: image.to_string(),
            reason: format!("Failed to open blob cache: {}", e),
        })?;

        let puller = zlayer_registry::ImagePuller::new(cache);
        let auth = zlayer_registry::RegistryAuth::Anonymous;

        let rootfs_dir = image_dir.join("rootfs");
        tokio::fs::create_dir_all(&rootfs_dir).await.map_err(|e| {
            AgentError::PullFailed {
                image: image.to_string(),
                reason: format!("Failed to create rootfs dir: {}", e),
            }
        })?;

        puller
            .pull_and_extract(image, &auth, rootfs_dir.to_str().unwrap())
            .await
            .map_err(|e| AgentError::PullFailed {
                image: image.to_string(),
                reason: format!("Failed to pull and extract image: {}", e),
            })?;

        tracing::info!(image = %image, rootfs = %rootfs_dir.display(), "Image pulled successfully");
        Ok(())
    }
```

**`create_container`** -- APFS-clones the rootfs, generates the Seatbelt profile, writes config.

```rust
    async fn create_container(&self, id: &ContainerId, spec: &ServiceSpec) -> Result<()> {
        let dir_name = Self::container_dir_name(id);
        let container_dir = self.state_dir.join(&dir_name);
        let rootfs_dir = container_dir.join("rootfs");

        // Create container state directory
        tokio::fs::create_dir_all(&container_dir).await.map_err(|e| {
            AgentError::Runtime(format!(
                "Failed to create container dir {}: {}",
                container_dir.display(), e
            ))
        })?;

        // APFS clone the base image rootfs to this container's rootfs
        let image_rootfs = self.state_dir.join("images").join(
            spec.image.name.replace('/', "_").replace(':', "_")
        ).join("rootfs");

        clone_directory_recursive(&image_rootfs, &rootfs_dir).await.map_err(|e| {
            AgentError::Runtime(format!(
                "Failed to clone rootfs from {} to {}: {}",
                image_rootfs.display(), rootfs_dir.display(), e
            ))
        })?;

        // Generate Seatbelt profile based on the service spec
        let gpu_access = if spec.resources.as_ref()
            .and_then(|r| r.gpu.as_ref())
            .map(|g| g.vendor == "apple")
            .unwrap_or(false)
        {
            let mode = spec.resources.as_ref()
                .and_then(|r| r.gpu.as_ref())
                .and_then(|g| g.mode.as_deref())
                .unwrap_or("native");
            if mode == "native" {
                GpuAccess::MetalCompute
            } else {
                GpuAccess::None
            }
        } else {
            GpuAccess::None
        };

        // Determine network access from spec endpoints
        let network_access = build_network_access(spec);

        // Determine writable directories from spec volumes
        let writable_dirs = build_writable_dirs(spec, &container_dir);

        let sandbox_config = SandboxConfig {
            rootfs_dir: rootfs_dir.clone(),
            workspace_dir: container_dir.clone(),
            gpu_access,
            network_access,
            writable_dirs,
            readonly_dirs: vec![],
            max_files: 4096,
            cpu_time_limit: None,
            memory_limit: spec.resources.as_ref()
                .and_then(|r| r.memory.as_ref())
                .and_then(|m| parse_memory_string(m)),
        };

        let profile = generate_sandbox_profile(&sandbox_config);

        // Write profile to disk
        tokio::fs::write(container_dir.join("profile.sb"), &profile)
            .await
            .map_err(|e| AgentError::Runtime(format!("Failed to write profile: {}", e)))?;

        // Write config to disk
        let config_json = serde_json::to_string_pretty(spec)
            .map_err(|e| AgentError::Runtime(format!("Failed to serialize spec: {}", e)))?;
        tokio::fs::write(container_dir.join("config.json"), &config_json)
            .await
            .map_err(|e| AgentError::Runtime(format!("Failed to write config: {}", e)))?;

        // Register as pending
        let process = SandboxedProcess {
            pid: 0,
            state: ContainerState::Pending,
            state_dir: container_dir,
            rootfs_dir,
            started_at: Instant::now(),
            memory_limit: sandbox_config.memory_limit,
            watchdog_handle: None,
        };

        let mut processes = self.processes.write().await;
        processes.insert(dir_name, process);

        Ok(())
    }
```

**`start_container`** -- Forks, applies sandbox, changes root directory, executes workload.

```rust
    async fn start_container(&self, id: &ContainerId) -> Result<()> {
        let dir_name = Self::container_dir_name(id);

        let mut processes = self.processes.write().await;
        let process = processes.get_mut(&dir_name).ok_or_else(|| {
            AgentError::NotFound {
                container: dir_name.clone(),
                reason: "Container not created".to_string(),
            }
        })?;

        // Read the saved spec
        let config_path = process.state_dir.join("config.json");
        let config_str = tokio::fs::read_to_string(&config_path)
            .await
            .map_err(|e| AgentError::Runtime(format!("Failed to read config: {}", e)))?;
        let spec: ServiceSpec = serde_json::from_str(&config_str)
            .map_err(|e| AgentError::Runtime(format!("Failed to parse config: {}", e)))?;

        // Read the generated Seatbelt profile
        let profile_path = process.state_dir.join("profile.sb");
        let profile = tokio::fs::read_to_string(&profile_path)
            .await
            .map_err(|e| AgentError::Runtime(format!("Failed to read profile: {}", e)))?;

        // Determine the command to execute
        let (program, args) = resolve_entrypoint(&spec, &process.rootfs_dir)?;

        // Set up log files
        let stdout_path = process.state_dir.join("stdout.log");
        let stderr_path = process.state_dir.join("stderr.log");

        // Spawn the sandboxed process
        let child_pid = spawn_sandboxed_process(
            &program,
            &args,
            &profile,
            &process.rootfs_dir,
            &stdout_path,
            &stderr_path,
            &spec,
        )?;

        // Write PID file
        tokio::fs::write(
            process.state_dir.join("pid"),
            child_pid.to_string(),
        ).await.map_err(|e| {
            AgentError::Runtime(format!("Failed to write PID file: {}", e))
        })?;

        process.pid = child_pid;
        process.state = ContainerState::Running;
        process.started_at = Instant::now();

        // Start memory watchdog if a memory limit is configured
        if let Some(limit) = process.memory_limit {
            let pid = child_pid;
            let handle = tokio::spawn(async move {
                memory_watchdog(pid, limit).await;
            });
            process.watchdog_handle = Some(handle);
        }

        tracing::info!(
            container = %dir_name,
            pid = child_pid,
            "Sandboxed process started"
        );

        Ok(())
    }
```

**`stop_container`** -- SIGTERM with timeout, then SIGKILL.

```rust
    async fn stop_container(&self, id: &ContainerId, timeout: Duration) -> Result<()> {
        let dir_name = Self::container_dir_name(id);

        let mut processes = self.processes.write().await;
        let process = processes.get_mut(&dir_name).ok_or_else(|| {
            AgentError::NotFound {
                container: dir_name.clone(),
                reason: "Container not found".to_string(),
            }
        })?;

        if process.pid == 0 {
            process.state = ContainerState::Exited { code: 0 };
            return Ok(());
        }

        process.state = ContainerState::Stopping;

        let pid = process.pid;
        drop(processes); // Release lock during I/O

        // Send SIGTERM
        unsafe {
            libc::kill(pid as i32, libc::SIGTERM);
        }

        // Wait for graceful shutdown with timeout
        let deadline = Instant::now() + timeout;
        loop {
            if Instant::now() >= deadline {
                break;
            }

            // Check if process has exited
            let mut status: libc::c_int = 0;
            let result = unsafe {
                libc::waitpid(pid as i32, &mut status, libc::WNOHANG)
            };

            if result > 0 {
                // Process exited
                let exit_code = if libc::WIFEXITED(status) {
                    libc::WEXITSTATUS(status)
                } else {
                    -1
                };

                let mut processes = self.processes.write().await;
                if let Some(p) = processes.get_mut(&dir_name) {
                    p.state = ContainerState::Exited { code: exit_code };
                    if let Some(h) = p.watchdog_handle.take() {
                        h.abort();
                    }
                }
                return Ok(());
            }

            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        // Timeout reached -- send SIGKILL
        tracing::warn!(pid = pid, "SIGTERM timeout, sending SIGKILL");
        unsafe {
            libc::kill(pid as i32, libc::SIGKILL);
        }

        // Wait for SIGKILL to take effect
        let mut status: libc::c_int = 0;
        unsafe {
            libc::waitpid(pid as i32, &mut status, 0);
        }

        let mut processes = self.processes.write().await;
        if let Some(p) = processes.get_mut(&dir_name) {
            p.state = ContainerState::Exited { code: -9 };
            if let Some(h) = p.watchdog_handle.take() {
                h.abort();
            }
        }

        Ok(())
    }
```

**`remove_container`** -- Cleans up state directory and APFS-cloned rootfs.

```rust
    async fn remove_container(&self, id: &ContainerId) -> Result<()> {
        let dir_name = Self::container_dir_name(id);

        // Remove from tracking
        let mut processes = self.processes.write().await;
        if let Some(mut p) = processes.remove(&dir_name) {
            // Abort watchdog if running
            if let Some(h) = p.watchdog_handle.take() {
                h.abort();
            }

            // Kill process if still running
            if p.pid > 0 {
                if let ContainerState::Running | ContainerState::Stopping = p.state {
                    unsafe { libc::kill(p.pid as i32, libc::SIGKILL); }
                    let mut status: libc::c_int = 0;
                    unsafe { libc::waitpid(p.pid as i32, &mut status, 0); }
                }
            }
        }
        drop(processes);

        // Remove container directory (rootfs, logs, config, profile)
        let container_dir = self.state_dir.join(&dir_name);
        if container_dir.exists() {
            tokio::fs::remove_dir_all(&container_dir).await.map_err(|e| {
                AgentError::Runtime(format!(
                    "Failed to remove container dir {}: {}",
                    container_dir.display(), e
                ))
            })?;
        }

        Ok(())
    }
```

**`container_state`** -- Checks process status via kill(pid, 0) and waitpid(WNOHANG).

```rust
    async fn container_state(&self, id: &ContainerId) -> Result<ContainerState> {
        let dir_name = Self::container_dir_name(id);

        let mut processes = self.processes.write().await;
        let process = processes.get_mut(&dir_name).ok_or_else(|| {
            AgentError::NotFound {
                container: dir_name.clone(),
                reason: "Container not found".to_string(),
            }
        })?;

        // If already in a terminal state, return it
        match &process.state {
            ContainerState::Exited { .. } | ContainerState::Failed { .. } => {
                return Ok(process.state.clone());
            }
            ContainerState::Pending => return Ok(ContainerState::Pending),
            _ => {}
        }

        // Check if process is still alive
        if process.pid > 0 {
            let mut status: libc::c_int = 0;
            let result = unsafe {
                libc::waitpid(process.pid as i32, &mut status, libc::WNOHANG)
            };

            if result > 0 {
                // Process has exited
                let exit_code = if libc::WIFEXITED(status) {
                    libc::WEXITSTATUS(status)
                } else if libc::WIFSIGNALED(status) {
                    -(libc::WTERMSIG(status))
                } else {
                    -1
                };
                process.state = ContainerState::Exited { code: exit_code };
            } else if result == 0 {
                // Process still running
                process.state = ContainerState::Running;
            } else {
                // Error (process doesn't exist)
                process.state = ContainerState::Failed {
                    reason: "Process disappeared".to_string(),
                };
            }
        }

        Ok(process.state.clone())
    }
```

**`container_logs`** -- Reads the captured stdout/stderr log files.

```rust
    async fn container_logs(&self, id: &ContainerId, tail: usize) -> Result<String> {
        let dir_name = Self::container_dir_name(id);

        let processes = self.processes.read().await;
        let process = processes.get(&dir_name).ok_or_else(|| {
            AgentError::NotFound {
                container: dir_name.clone(),
                reason: "Container not found".to_string(),
            }
        })?;

        let stdout_path = process.state_dir.join("stdout.log");
        let stderr_path = process.state_dir.join("stderr.log");

        let mut combined = String::new();

        if let Ok(stdout) = tokio::fs::read_to_string(&stdout_path).await {
            combined.push_str(&stdout);
        }
        if let Ok(stderr) = tokio::fs::read_to_string(&stderr_path).await {
            if !combined.is_empty() {
                combined.push('\n');
            }
            combined.push_str(&stderr);
        }

        // Apply tail
        if tail > 0 {
            let lines: Vec<&str> = combined.lines().collect();
            let start = lines.len().saturating_sub(tail);
            Ok(lines[start..].join("\n"))
        } else {
            Ok(combined)
        }
    }
```

**`exec`** -- Forks a new sandboxed process into the existing rootfs with the given command.

```rust
    async fn exec(&self, id: &ContainerId, cmd: &[String]) -> Result<(i32, String, String)> {
        let dir_name = Self::container_dir_name(id);

        let processes = self.processes.read().await;
        let process = processes.get(&dir_name).ok_or_else(|| {
            AgentError::NotFound {
                container: dir_name.clone(),
                reason: "Container not found".to_string(),
            }
        })?;

        // Read the Seatbelt profile (same sandbox as the main process)
        let profile = tokio::fs::read_to_string(process.state_dir.join("profile.sb"))
            .await
            .map_err(|e| AgentError::Runtime(format!("Failed to read profile: {}", e)))?;

        let rootfs = process.rootfs_dir.clone();
        drop(processes);

        if cmd.is_empty() {
            return Err(AgentError::Runtime("Empty command".to_string()));
        }

        // Use sandbox-exec to run the command in the container's rootfs
        let output = std::process::Command::new("/usr/bin/sandbox-exec")
            .arg("-p")
            .arg(&profile)
            .arg("--")
            .arg(&cmd[0])
            .args(&cmd[1..])
            .current_dir(&rootfs)
            .output()
            .map_err(|e| AgentError::Runtime(format!("Failed to exec: {}", e)))?;

        let exit_code = output.status.code().unwrap_or(-1);
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        Ok((exit_code, stdout, stderr))
    }
```

**`get_container_stats`** -- Uses mach_task_basic_info for memory, rusage for CPU.

```rust
    async fn get_container_stats(&self, id: &ContainerId) -> Result<ContainerStats> {
        let dir_name = Self::container_dir_name(id);

        let processes = self.processes.read().await;
        let process = processes.get(&dir_name).ok_or_else(|| {
            AgentError::NotFound {
                container: dir_name.clone(),
                reason: "Container not found".to_string(),
            }
        })?;

        if process.pid == 0 {
            return Err(AgentError::Runtime("Container not started".to_string()));
        }

        // Get process resource usage via proc_pid_rusage or /proc emulation
        // On macOS, we use the pid_rusage Mach call
        let (cpu_usec, memory_bytes) = get_process_stats(process.pid)?;

        Ok(ContainerStats {
            cpu_usage_usec: cpu_usec,
            memory_bytes,
            memory_limit: process.memory_limit.unwrap_or(0),
            timestamp: Instant::now(),
        })
    }
```

**`wait_container`** -- Blocks on waitpid until the process exits.

```rust
    async fn wait_container(&self, id: &ContainerId) -> Result<i32> {
        let dir_name = Self::container_dir_name(id);
        let pid = {
            let processes = self.processes.read().await;
            let process = processes.get(&dir_name).ok_or_else(|| {
                AgentError::NotFound {
                    container: dir_name.clone(),
                    reason: "Container not found".to_string(),
                }
            })?;
            process.pid
        };

        if pid == 0 {
            return Err(AgentError::Runtime("Container not started".to_string()));
        }

        // Use spawn_blocking to avoid blocking the async runtime
        let exit_code = tokio::task::spawn_blocking(move || {
            let mut status: libc::c_int = 0;
            let result = unsafe { libc::waitpid(pid as i32, &mut status, 0) };
            if result < 0 {
                return -1;
            }
            if libc::WIFEXITED(status) {
                libc::WEXITSTATUS(status)
            } else if libc::WIFSIGNALED(status) {
                -(libc::WTERMSIG(status))
            } else {
                -1
            }
        })
        .await
        .map_err(|e| AgentError::Runtime(format!("Wait task failed: {}", e)))?;

        // Update state
        let mut processes = self.processes.write().await;
        if let Some(p) = processes.get_mut(&dir_name) {
            p.state = ContainerState::Exited { code: exit_code };
        }

        Ok(exit_code)
    }
```

**`get_logs`** -- Same as container_logs but returns Vec<String>.

```rust
    async fn get_logs(&self, id: &ContainerId) -> Result<Vec<String>> {
        let log_str = self.container_logs(id, 0).await?;
        Ok(log_str.lines().map(|l| l.to_string()).collect())
    }
```

**`get_container_pid`** -- Returns the stored PID from the fork.

```rust
    async fn get_container_pid(&self, id: &ContainerId) -> Result<Option<u32>> {
        let dir_name = Self::container_dir_name(id);

        let processes = self.processes.read().await;
        let process = processes.get(&dir_name).ok_or_else(|| {
            AgentError::NotFound {
                container: dir_name.clone(),
                reason: "Container not found".to_string(),
            }
        })?;

        Ok(if process.pid > 0 { Some(process.pid) } else { None })
    }
```

**`get_container_ip`** -- Returns localhost because there are no network namespaces on macOS.

```rust
    async fn get_container_ip(&self, id: &ContainerId) -> Result<Option<IpAddr>> {
        let dir_name = Self::container_dir_name(id);

        let processes = self.processes.read().await;
        if !processes.contains_key(&dir_name) {
            return Err(AgentError::NotFound {
                container: dir_name,
                reason: "Container not found".to_string(),
            });
        }

        // On macOS, all sandboxed processes share the host network.
        // Return 127.0.0.1 -- the proxy manager routes traffic by port.
        Ok(Some(IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)))
    }
} // end impl Runtime for SandboxRuntime
```

### Step 5: Seatbelt Profile Generation Module

**File**: `crates/zlayer-agent/src/runtimes/macos_sandbox.rs` (continued, helper module)

**Why**: Each container needs a unique Seatbelt profile generated from its `ServiceSpec`. The profile must be parameterized with the rootfs path, network rules, GPU access, and writable directories. This is the heart of the macOS isolation model.

#### Configuration Types

```rust
/// GPU access level for the sandbox profile
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GpuAccess {
    /// No GPU access -- deny all IOKit and GPU Mach services
    None,
    /// Full Metal compute -- shader compilation + IOKit GPU access
    MetalCompute,
    /// MPS only -- pre-compiled kernels, no MTLCompilerService needed
    MpsOnly,
}

/// Network access level for the sandbox profile
#[derive(Debug, Clone)]
pub enum NetworkAccess {
    /// No network access at all
    None,
    /// Only specific localhost ports (for inter-service communication)
    LocalhostOnly { bind_ports: Vec<u16>, connect_ports: Vec<u16> },
    /// Full network access (outbound + inbound + bind)
    Full,
}

/// Complete sandbox configuration
#[derive(Debug, Clone)]
pub struct SandboxConfig {
    /// Path to the container's cloned rootfs
    pub rootfs_dir: PathBuf,
    /// Path to the container's workspace/state directory
    pub workspace_dir: PathBuf,
    /// GPU access level
    pub gpu_access: GpuAccess,
    /// Network access configuration
    pub network_access: NetworkAccess,
    /// Directories the process can write to (volume mounts)
    pub writable_dirs: Vec<PathBuf>,
    /// Additional read-only directories
    pub readonly_dirs: Vec<PathBuf>,
    /// Maximum open file descriptors
    pub max_files: u64,
    /// CPU time limit in seconds (RLIMIT_CPU)
    pub cpu_time_limit: Option<u64>,
    /// Memory limit in bytes (for watchdog, not kernel-enforced)
    pub memory_limit: Option<u64>,
}
```

#### Profile Generator

```rust
/// Generate a complete Seatbelt profile from a SandboxConfig.
///
/// The profile follows a deny-default whitelist model: everything is denied
/// unless explicitly allowed. The profile is structured in sections:
///
/// 1. Base process rules (always needed for any process to run)
/// 2. System library access (dyld, libSystem, frameworks)
/// 3. Container rootfs access (read + write)
/// 4. Volume mount access (writable dirs)
/// 5. GPU rules (if gpu_access != None)
/// 6. Network rules (based on network_access)
/// 7. Logging and /dev/null access
pub fn generate_sandbox_profile(config: &SandboxConfig) -> String {
    let mut profile = String::with_capacity(4096);

    // Header
    profile.push_str("(version 1)\n");
    profile.push_str("(deny default)\n");
    profile.push_str("\n");

    // Debug logging for development (remove in production)
    // profile.push_str("(debug deny)\n\n");

    // ===== Section 1: Base process rules =====
    profile.push_str("; --- Base process rules ---\n");
    profile.push_str("(allow process-exec)\n");
    profile.push_str("(allow process-fork)\n");
    profile.push_str("(allow signal (target same-sandbox))\n");
    profile.push_str("(allow process-info* (target self))\n");
    profile.push_str("(allow process-info-pidinfo)\n");
    profile.push_str("(allow process-info-rusage)\n");
    profile.push_str("\n");

    // ===== Section 2: System library and framework access =====
    profile.push_str("; --- System libraries (required for any process to run) ---\n");
    profile.push_str("(allow file-read*\n");
    profile.push_str("  (subpath \"/usr/lib\")\n");
    profile.push_str("  (subpath \"/System/Library/Frameworks\")\n");
    profile.push_str("  (subpath \"/System/Library/PrivateFrameworks\")\n");
    profile.push_str("  (subpath \"/System/Library/Extensions\")\n");
    profile.push_str("  (subpath \"/System/Library/ColorSync\")\n");
    profile.push_str("  (literal \"/\")\n");
    profile.push_str("  (literal \"/dev/random\")\n");
    profile.push_str("  (literal \"/dev/urandom\"))\n");
    profile.push_str("\n");
    profile.push_str("; --- Executable mapping (required for dyld) ---\n");
    profile.push_str("(allow file-map-executable\n");
    profile.push_str("  (subpath \"/usr/lib\")\n");
    profile.push_str("  (subpath \"/System/Library/Frameworks\")\n");
    profile.push_str("  (subpath \"/System/Library/PrivateFrameworks\")\n");
    profile.push_str("  (subpath \"/System/Library/Extensions\"))\n");
    profile.push_str("\n");
    profile.push_str("; --- System info (hw detection, etc.) ---\n");
    profile.push_str("(allow sysctl-read)\n");
    profile.push_str("(allow system-info)\n");
    profile.push_str("\n");
    profile.push_str("; --- Mach basics ---\n");
    profile.push_str("(allow mach-lookup\n");
    profile.push_str("  (global-name \"com.apple.system.opendirectoryd.libinfo\"))\n");
    profile.push_str("\n");

    // ===== Section 3: Container rootfs access =====
    profile.push_str("; --- Container rootfs ---\n");
    profile.push_str(&format!(
        "(allow file-read* file-write* (subpath \"{}\"))\n",
        config.rootfs_dir.display()
    ));
    profile.push_str(&format!(
        "(allow file-map-executable (subpath \"{}\"))\n",
        config.rootfs_dir.display()
    ));
    profile.push_str("\n");

    // Workspace directory (logs, config, etc.)
    profile.push_str("; --- Workspace directory ---\n");
    profile.push_str(&format!(
        "(allow file-read* file-write* (subpath \"{}\"))\n",
        config.workspace_dir.display()
    ));
    profile.push_str("\n");

    // ===== Section 4: Volume mounts =====
    if !config.writable_dirs.is_empty() {
        profile.push_str("; --- Volume mounts (writable) ---\n");
        for dir in &config.writable_dirs {
            profile.push_str(&format!(
                "(allow file-read* file-write* (subpath \"{}\"))\n",
                dir.display()
            ));
        }
        profile.push_str("\n");
    }

    if !config.readonly_dirs.is_empty() {
        profile.push_str("; --- Volume mounts (read-only) ---\n");
        for dir in &config.readonly_dirs {
            profile.push_str(&format!(
                "(allow file-read* (subpath \"{}\"))\n",
                dir.display()
            ));
        }
        profile.push_str("\n");
    }

    // ===== Section 5: GPU access =====
    match config.gpu_access {
        GpuAccess::MetalCompute => {
            profile.push_str(METAL_COMPUTE_PROFILE_SECTION);
        }
        GpuAccess::MpsOnly => {
            profile.push_str(MPS_ONLY_PROFILE_SECTION);
        }
        GpuAccess::None => {}
    }

    // ===== Section 6: Network access =====
    match &config.network_access {
        NetworkAccess::None => {
            profile.push_str("; --- Network: DENIED ---\n\n");
        }
        NetworkAccess::LocalhostOnly { bind_ports, connect_ports } => {
            profile.push_str("; --- Network: localhost only ---\n");
            for port in bind_ports {
                profile.push_str(&format!(
                    "(allow network-bind (local ip \"localhost:{}\"))\n", port
                ));
            }
            for port in connect_ports {
                profile.push_str(&format!(
                    "(allow network-outbound (remote ip \"localhost:{}\"))\n", port
                ));
            }
            // Allow inbound on bound ports
            if !bind_ports.is_empty() {
                profile.push_str("(allow network-inbound (local ip \"localhost:*\"))\n");
            }
            profile.push_str("\n");
        }
        NetworkAccess::Full => {
            profile.push_str("; --- Network: full access ---\n");
            profile.push_str("(allow network-outbound)\n");
            profile.push_str("(allow network-inbound)\n");
            profile.push_str("(allow network-bind)\n");
            profile.push_str("(allow system-socket)\n");
            profile.push_str("\n");
        }
    }

    // ===== Section 7: Logging, /dev/null, pseudo-tty =====
    profile.push_str("; --- I/O essentials ---\n");
    profile.push_str("(allow file-write-data\n");
    profile.push_str("  (require-all (literal \"/dev/null\") (vnode-type CHARACTER-DEVICE)))\n");
    profile.push_str("(allow file-read-data\n");
    profile.push_str("  (require-all (literal \"/dev/null\") (vnode-type CHARACTER-DEVICE)))\n");
    profile.push_str("(allow pseudo-tty)\n");
    profile.push_str("(allow file-read* file-write* file-ioctl (literal \"/dev/ptmx\"))\n");
    profile.push_str("\n");

    // IPC basics
    profile.push_str("; --- IPC ---\n");
    profile.push_str("(allow ipc-posix-sem)\n");
    profile.push_str("(allow ipc-posix-shm)\n");
    profile.push_str("\n");

    profile
}
```

#### Profile Constants for GPU Access

```rust
/// Full Metal compute profile section.
///
/// Allows IOKit GPU access, Mach shader compilation services,
/// and all filesystem paths needed for Metal.framework.
const METAL_COMPUTE_PROFILE_SECTION: &str = "\
; --- GPU: Full Metal Compute ---

; IOKit user clients for GPU hardware access
(allow iokit-open
  (iokit-user-client-class \"IOSurfaceRoot\")
  (iokit-user-client-class \"IOSurfaceRootUserClient\")
  (iokit-user-client-class \"IOAccelDevice2\")
  (iokit-user-client-class \"IOAccelContext2\")
  (iokit-user-client-class \"IOAccelSharedUserClient2\")
  (iokit-user-client-class \"IOAccelSubmitter2\")
  (iokit-user-client-class \"RootDomainUserClient\")
  ; Apple Silicon GPU
  (iokit-user-client-class \"AGXAccelerator\")
  ; Intel GPU (for Intel Macs)
  (iokit-user-client-class \"IOAccelerator\")
  (iokit-user-client-class \"IntelAccelerator\"))

; GPU IOKit properties
(allow iokit-get-properties
  (iokit-property \"MetalPluginClassName\")
  (iokit-property \"MetalPluginName\")
  (iokit-property \"MetalStatisticsName\")
  (iokit-property \"MetalStatisticsScriptName\")
  (iokit-property \"MetalCoalesce\")
  (iokit-property \"IOGLBundleName\")
  (iokit-property \"IOGLESBundleName\")
  (iokit-property \"IOGLESDefaultUseMetal\")
  (iokit-property \"IOGLESMetalBundleName\")
  (iokit-property \"GPUConfigurationVariable\")
  (iokit-property \"GPUDCCDisplayable\")
  (iokit-property \"GPUDebugNullClientMask\")
  (iokit-property \"GpuDebugPolicy\")
  (iokit-property \"GPURawCounterBundleName\")
  (iokit-property \"CompactVRAM\")
  (iokit-property \"EnableBlitLib\")
  (iokit-property \"IOPCIMatch\")
  (iokit-property \"model\")
  (iokit-property \"vendor-id\")
  (iokit-property \"device-id\")
  (iokit-property \"class-code\")
  (iokit-property \"soc-generation\")
  (iokit-property \"IORegistryEntryPropertyKeys\"))

; Mach services for Metal shader compilation
(allow mach-lookup
  (global-name \"com.apple.MTLCompilerService\")
  (global-name \"com.apple.CARenderServer\")
  (global-name \"com.apple.PowerManagement.control\")
  (global-name \"com.apple.gpu.process\"))

; XPC services for shader compilation (Apple Silicon)
(allow mach-lookup
  (xpc-service-name \"com.apple.MTLCompilerService\")
  (xpc-service-name-prefix \"com.apple.AGXCompilerService\"))

; User preferences for Metal/OpenGL
(allow user-preference-read
  (preference-domain \"com.apple.opengl\")
  (preference-domain \"com.apple.Metal\")
  (preference-domain \"com.nvidia.OpenGL\"))

; GPU driver bundles and libraries
(allow file-read*
  (subpath \"/Library/GPUBundles\")
  (subpath \"/System/Library/Frameworks/Metal.framework\")
  (subpath \"/System/Library/Frameworks/MetalPerformanceShaders.framework\")
  (subpath \"/System/Library/Frameworks/MetalPerformanceShadersGraph.framework\")
  (subpath \"/System/Library/PrivateFrameworks/GPUCompiler.framework\"))

";

/// MPS-only profile section (subset of Metal compute).
///
/// MPS kernels are pre-compiled in the OS -- no MTLCompilerService needed.
/// This gives a smaller attack surface than full Metal compute.
const MPS_ONLY_PROFILE_SECTION: &str = "\
; --- GPU: MPS Only (pre-compiled kernels, no shader compilation) ---

; IOKit user clients for GPU hardware access (minimal set)
(allow iokit-open
  (iokit-user-client-class \"IOSurfaceRoot\")
  (iokit-user-client-class \"IOSurfaceRootUserClient\")
  (iokit-user-client-class \"IOAccelDevice2\")
  (iokit-user-client-class \"IOAccelContext2\")
  (iokit-user-client-class \"IOAccelSharedUserClient2\")
  (iokit-user-client-class \"RootDomainUserClient\")
  ; Apple Silicon GPU
  (iokit-user-client-class \"AGXAccelerator\"))

; GPU IOKit properties (minimal set for MPS)
(allow iokit-get-properties
  (iokit-property \"MetalPluginClassName\")
  (iokit-property \"MetalPluginName\")
  (iokit-property \"IOGLESDefaultUseMetal\")
  (iokit-property \"GPUConfigurationVariable\")
  (iokit-property \"model\")
  (iokit-property \"vendor-id\")
  (iokit-property \"device-id\")
  (iokit-property \"soc-generation\")
  (iokit-property \"IORegistryEntryPropertyKeys\"))

; Mach services -- NO MTLCompilerService needed for MPS
(allow mach-lookup
  (global-name \"com.apple.PowerManagement.control\"))

; User preferences
(allow user-preference-read
  (preference-domain \"com.apple.Metal\"))

; MPS framework access
(allow file-read*
  (subpath \"/Library/GPUBundles\")
  (subpath \"/System/Library/Frameworks/Metal.framework\")
  (subpath \"/System/Library/Frameworks/MetalPerformanceShaders.framework\")
  (subpath \"/System/Library/Frameworks/MetalPerformanceShadersGraph.framework\"))

";
```

#### Helper: Build Network Access from ServiceSpec

```rust
/// Build NetworkAccess from a ServiceSpec's endpoints and network configuration
fn build_network_access(spec: &ServiceSpec) -> NetworkAccess {
    let mut bind_ports = Vec::new();
    let mut connect_ports = Vec::new();

    // Collect ports from endpoints
    for endpoint in &spec.endpoints {
        bind_ports.push(endpoint.port);
    }

    // If no endpoints and no special network config, default to full access
    // (most services need outbound connectivity for dependencies)
    if bind_ports.is_empty() {
        return NetworkAccess::Full;
    }

    // Add common connect ports (DNS, HTTP, HTTPS) for outbound
    connect_ports.extend_from_slice(&[53, 80, 443]);

    // Add all bind ports as connect ports too (for health checks)
    connect_ports.extend_from_slice(&bind_ports);

    NetworkAccess::LocalhostOnly { bind_ports, connect_ports }
}

/// Build writable directory list from ServiceSpec volume mounts
fn build_writable_dirs(spec: &ServiceSpec, container_dir: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![
        container_dir.join("tmp"),  // Always allow tmp
    ];

    if let Some(ref storage) = spec.storage {
        for volume in &storage.volumes {
            dirs.push(PathBuf::from(&volume.mount_path));
        }
    }

    dirs
}

/// Parse a memory string like "512Mi" or "2Gi" into bytes
fn parse_memory_string(s: &str) -> Option<u64> {
    let s = s.trim();
    if s.ends_with("Gi") {
        s[..s.len()-2].parse::<u64>().ok().map(|v| v * 1024 * 1024 * 1024)
    } else if s.ends_with("Mi") {
        s[..s.len()-2].parse::<u64>().ok().map(|v| v * 1024 * 1024)
    } else if s.ends_with("Ki") {
        s[..s.len()-2].parse::<u64>().ok().map(|v| v * 1024)
    } else {
        s.parse::<u64>().ok()
    }
}
```

### Step 6: Filesystem Isolation

**File**: `crates/zlayer-agent/src/runtimes/macos_sandbox.rs` (continued)

**Why**: macOS has no overlayfs or mount namespaces. APFS `clonefile()` provides zero-copy CoW file duplication. We clone the entire base rootfs to a per-container directory, then use Seatbelt `(subpath ...)` rules to restrict the process.

#### APFS clonefile FFI

```rust
#[cfg(target_os = "macos")]
extern "C" {
    /// Clone a file using APFS copy-on-write semantics.
    ///
    /// From <sys/clonefile.h>. Creates a new file at `dst` that shares
    /// storage blocks with `src`. Nearly instantaneous. Both files must
    /// be on the same APFS volume.
    fn clonefile(
        src: *const libc::c_char,
        dst: *const libc::c_char,
        flags: libc::c_int,
    ) -> libc::c_int;
}

/// Clone a single file using APFS CoW.
///
/// Returns Ok(true) if clonefile succeeded, Ok(false) if clonefile
/// is not supported (non-APFS volume) and the caller should fall
/// back to a regular copy.
#[cfg(target_os = "macos")]
fn clone_file_apfs(src: &Path, dst: &Path) -> std::io::Result<bool> {
    use std::ffi::CString;

    let c_src = CString::new(src.to_str().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid src path")
    })?)
    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;

    let c_dst = CString::new(dst.to_str().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid dst path")
    })?)
    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;

    let ret = unsafe { clonefile(c_src.as_ptr(), c_dst.as_ptr(), 0) };

    if ret == 0 {
        Ok(true) // Clone succeeded
    } else {
        let err = std::io::Error::last_os_error();
        // ENOTSUP means the filesystem doesn't support clonefile (not APFS)
        // EXDEV means src and dst are on different volumes
        if err.raw_os_error() == Some(libc::ENOTSUP)
            || err.raw_os_error() == Some(libc::EXDEV)
        {
            Ok(false) // Fallback needed
        } else {
            Err(err)
        }
    }
}

/// Recursively clone a directory tree using APFS clonefile for files.
///
/// clonefile operates at the file level, not directory level, so we must
/// walk the directory tree, create directories in the destination, and
/// clonefile each regular file.
///
/// If APFS clonefile is not available (non-APFS volume), falls back to
/// regular file copy.
#[cfg(target_os = "macos")]
async fn clone_directory_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    // Create destination directory
    tokio::fs::create_dir_all(dst).await?;

    let mut entries = tokio::fs::read_dir(src).await?;
    while let Some(entry) = entries.next_entry().await? {
        let entry_path = entry.path();
        let file_name = entry.file_name();
        let dest_path = dst.join(&file_name);

        let file_type = entry.file_type().await?;

        if file_type.is_dir() {
            // Recurse into subdirectories
            Box::pin(clone_directory_recursive(&entry_path, &dest_path)).await?;
        } else if file_type.is_file() {
            // Try APFS clone first, fall back to copy
            let src_clone = entry_path.clone();
            let dst_clone = dest_path.clone();

            let cloned = tokio::task::spawn_blocking(move || {
                clone_file_apfs(&src_clone, &dst_clone)
            })
            .await
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))??;

            if !cloned {
                // Fallback: regular copy
                tokio::fs::copy(&entry_path, &dest_path).await?;
            }
        } else if file_type.is_symlink() {
            // Recreate symlinks
            let link_target = tokio::fs::read_link(&entry_path).await?;
            tokio::fs::symlink(&link_target, &dest_path).await?;
        }
    }

    // Preserve directory permissions
    let src_meta = tokio::fs::metadata(src).await?;
    tokio::fs::set_permissions(dst, src_meta.permissions()).await?;

    Ok(())
}

/// Fallback for non-macOS platforms (regular recursive copy)
#[cfg(not(target_os = "macos"))]
async fn clone_directory_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    // Simple recursive copy -- no APFS clonefile available
    tokio::fs::create_dir_all(dst).await?;

    let mut entries = tokio::fs::read_dir(src).await?;
    while let Some(entry) = entries.next_entry().await? {
        let dest_path = dst.join(entry.file_name());
        let file_type = entry.file_type().await?;

        if file_type.is_dir() {
            Box::pin(clone_directory_recursive(&entry.path(), &dest_path)).await?;
        } else {
            tokio::fs::copy(&entry.path(), &dest_path).await?;
        }
    }
    Ok(())
}
```

#### Directory Structure

Each container gets this directory layout:

```
/var/lib/zlayer/containers/
  images/
    docker.io_library_python_3.11/    <-- pulled base images
      rootfs/
        bin/
        lib/
        usr/
        ...
  myservice-rep-1/                     <-- per-container state
    rootfs/                            <-- APFS clone of base image rootfs
      bin/                             <-- CoW: zero space until modified
      lib/
      usr/
      ...
    config.json                        <-- serialized ServiceSpec
    profile.sb                         <-- generated Seatbelt profile
    stdout.log                         <-- captured stdout
    stderr.log                         <-- captured stderr
    pid                                <-- PID file
    tmp/                               <-- container temp directory
```

### Step 7: Process Lifecycle Management

**File**: `crates/zlayer-agent/src/runtimes/macos_sandbox.rs` (continued)

#### The fork + sandbox_init + exec Sequence

```rust
/// FFI declarations for macOS Seatbelt sandbox
#[cfg(target_os = "macos")]
mod seatbelt_ffi {
    use std::os::raw::c_char;

    #[link(name = "System", kind = "dylib")]
    extern "C" {
        /// Apply a sandbox profile to the current process.
        ///
        /// profile: SBPL string (Scheme-based sandbox profile)
        /// flags: 0 for raw SBPL string, 0x0001 for named profile
        /// errorbuf: receives error message on failure (free with sandbox_free_error)
        ///
        /// Returns 0 on success, -1 on failure.
        /// WARNING: Once applied, the sandbox CANNOT be removed or loosened.
        pub fn sandbox_init(
            profile: *const c_char,
            flags: u64,
            errorbuf: *mut *mut c_char,
        ) -> i32;

        /// Free an error buffer allocated by sandbox_init
        pub fn sandbox_free_error(errorbuf: *mut c_char);
    }
}

/// Apply a Seatbelt profile to the current process.
///
/// This is called in the child process after fork() and before exec().
/// Once applied, the sandbox cannot be removed.
#[cfg(target_os = "macos")]
fn apply_seatbelt_profile(sbpl: &str) -> std::io::Result<()> {
    use std::ffi::CString;
    use std::ptr;

    let profile_cstr = CString::new(sbpl)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;

    let mut error_buf: *mut std::os::raw::c_char = ptr::null_mut();

    let result = unsafe {
        seatbelt_ffi::sandbox_init(
            profile_cstr.as_ptr(),
            0, // 0 = raw SBPL string
            &mut error_buf,
        )
    };

    if result != 0 {
        let error_msg = if !error_buf.is_null() {
            let msg = unsafe {
                std::ffi::CStr::from_ptr(error_buf)
                    .to_string_lossy()
                    .into_owned()
            };
            unsafe { seatbelt_ffi::sandbox_free_error(error_buf) };
            msg
        } else {
            format!("sandbox_init returned error code {}", result)
        };
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("Failed to initialize sandbox: {}", error_msg),
        ));
    }

    Ok(())
}

/// Spawn a sandboxed process using fork + sandbox_init + exec.
///
/// The sequence:
/// 1. Set up pipes for stdout/stderr capture
/// 2. fork() the current process
/// 3. In the child:
///    a. Redirect stdout/stderr to log files
///    b. Apply the Seatbelt profile via sandbox_init()
///    c. chdir() to the rootfs directory
///    d. Apply resource limits via setrlimit()
///    e. Set environment variables from the ServiceSpec
///    f. exec() the workload binary
/// 4. In the parent: return the child PID
#[cfg(target_os = "macos")]
fn spawn_sandboxed_process(
    program: &str,
    args: &[String],
    sbpl_profile: &str,
    rootfs_dir: &Path,
    stdout_path: &Path,
    stderr_path: &Path,
    spec: &ServiceSpec,
) -> Result<u32> {
    use std::os::unix::process::CommandExt;

    let profile = sbpl_profile.to_string();
    let rootfs = rootfs_dir.to_path_buf();
    let env_vars: Vec<(String, String)> = spec.env.iter()
        .map(|e| (e.name.clone(), e.value.clone().unwrap_or_default()))
        .collect();

    // Open log files for stdout/stderr redirection
    let stdout_file = std::fs::File::create(stdout_path)
        .map_err(|e| AgentError::Runtime(format!("Failed to create stdout log: {}", e)))?;
    let stderr_file = std::fs::File::create(stderr_path)
        .map_err(|e| AgentError::Runtime(format!("Failed to create stderr log: {}", e)))?;

    let child = unsafe {
        std::process::Command::new(program)
            .args(args)
            .current_dir(&rootfs)
            .stdout(stdout_file)
            .stderr(stderr_file)
            .envs(env_vars)
            .pre_exec(move || {
                // Apply Seatbelt sandbox profile
                // This runs after fork() but before exec()
                apply_seatbelt_profile(&profile)?;

                // Apply resource limits
                set_resource_limits(4096, None)?;

                Ok(())
            })
            .spawn()
    }
    .map_err(|e| AgentError::Runtime(format!("Failed to spawn sandboxed process: {}", e)))?;

    Ok(child.id())
}

/// Set resource limits for the sandboxed process
#[cfg(target_os = "macos")]
fn set_resource_limits(max_files: u64, cpu_seconds: Option<u64>) -> std::io::Result<()> {
    // Limit open file descriptors
    let file_limit = libc::rlimit {
        rlim_cur: max_files,
        rlim_max: max_files,
    };
    if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &file_limit) } != 0 {
        return Err(std::io::Error::last_os_error());
    }

    // Limit CPU time if specified
    if let Some(seconds) = cpu_seconds {
        let cpu_limit = libc::rlimit {
            rlim_cur: seconds,
            rlim_max: seconds,
        };
        if unsafe { libc::setrlimit(libc::RLIMIT_CPU, &cpu_limit) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }

    Ok(())
}
```

#### Memory Watchdog Thread

Since macOS has no kernel-enforced memory limits (RLIMIT_RSS is not enforced), we use a polling watchdog that checks RSS and sends SIGKILL if it exceeds the threshold.

```rust
/// Memory watchdog that monitors a process's RSS and kills it if exceeded.
///
/// This is necessary because macOS does NOT enforce RLIMIT_RSS or RLIMIT_AS.
/// The watchdog polls every 2 seconds using proc_pid_rusage (Mach API).
#[cfg(target_os = "macos")]
async fn memory_watchdog(pid: u32, limit_bytes: u64) {
    let check_interval = Duration::from_secs(2);

    loop {
        tokio::time::sleep(check_interval).await;

        // Check if process is still alive
        let alive = unsafe { libc::kill(pid as i32, 0) } == 0;
        if !alive {
            tracing::debug!(pid = pid, "Memory watchdog: process exited");
            return;
        }

        // Get current RSS
        match get_process_rss(pid) {
            Ok(rss_bytes) => {
                if rss_bytes > limit_bytes {
                    tracing::warn!(
                        pid = pid,
                        rss_mb = rss_bytes / (1024 * 1024),
                        limit_mb = limit_bytes / (1024 * 1024),
                        "Memory limit exceeded, sending SIGKILL"
                    );
                    unsafe { libc::kill(pid as i32, libc::SIGKILL); }
                    return;
                }
            }
            Err(e) => {
                tracing::debug!(pid = pid, error = %e, "Failed to read process RSS");
            }
        }
    }
}

/// Get the resident set size (RSS) of a process using Mach task_info.
///
/// On macOS, we cannot directly call task_info on another process's task port
/// without special entitlements. Instead, we use the proc_pid_rusage syscall
/// which works for child processes.
#[cfg(target_os = "macos")]
fn get_process_rss(pid: u32) -> std::io::Result<u64> {
    // Use proc_pidinfo with PROC_PIDTASKINFO
    // This works for child processes without special entitlements
    #[repr(C)]
    #[allow(non_snake_case)]
    struct ProcTaskInfo {
        pti_virtual_size: u64,
        pti_resident_size: u64,
        pti_total_user: u64,
        pti_total_system: u64,
        pti_threads_user: u64,
        pti_threads_system: u64,
        pti_policy: i32,
        pti_faults: i32,
        pti_pageins: i32,
        pti_cow_faults: i32,
        pti_messages_sent: i32,
        pti_messages_received: i32,
        pti_syscalls_mach: i32,
        pti_syscalls_unix: i32,
        pti_csw: i32,
        pti_threadnum: i32,
        pti_numrunning: i32,
        pti_priority: i32,
    }

    extern "C" {
        fn proc_pidinfo(
            pid: libc::c_int,
            flavor: libc::c_int,
            arg: u64,
            buffer: *mut libc::c_void,
            buffersize: libc::c_int,
        ) -> libc::c_int;
    }

    const PROC_PIDTASKINFO: libc::c_int = 4;

    let mut info: ProcTaskInfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<ProcTaskInfo>() as libc::c_int;

    let ret = unsafe {
        proc_pidinfo(
            pid as libc::c_int,
            PROC_PIDTASKINFO,
            0,
            &mut info as *mut _ as *mut libc::c_void,
            size,
        )
    };

    if ret <= 0 {
        return Err(std::io::Error::last_os_error());
    }

    Ok(info.pti_resident_size)
}

/// Get CPU and memory stats for a process (used by get_container_stats).
#[cfg(target_os = "macos")]
fn get_process_stats(pid: u32) -> Result<(u64, u64)> {
    let rss = get_process_rss(pid)
        .map_err(|e| AgentError::Runtime(format!("Failed to get process RSS: {}", e)))?;

    // CPU time in microseconds -- approximate from proc_pidinfo
    // For a more accurate measurement, we would diff successive readings
    // For now, use a placeholder that scales with process lifetime
    let cpu_usec = 0u64; // TODO: implement delta-based CPU measurement

    Ok((cpu_usec, rss))
}

/// Resolve the entrypoint command from a ServiceSpec.
///
/// Looks at spec.command, spec.args, or falls back to trying to parse
/// the OCI image config for CMD/ENTRYPOINT.
fn resolve_entrypoint(spec: &ServiceSpec, rootfs: &Path) -> Result<(String, Vec<String>)> {
    if let Some(ref cmd) = spec.command {
        if cmd.is_empty() {
            return Err(AgentError::Runtime("Empty command in spec".to_string()));
        }

        // If the command is a single string with spaces, split it
        if cmd.len() == 1 && cmd[0].contains(' ') {
            let parts: Vec<String> = cmd[0].split_whitespace()
                .map(|s| s.to_string())
                .collect();
            let program = parts[0].clone();
            let args = parts[1..].to_vec();
            return Ok((program, args));
        }

        let program = cmd[0].clone();
        let mut args = cmd[1..].to_vec();

        // Append spec.args if present
        if let Some(ref extra_args) = spec.args {
            args.extend(extra_args.iter().cloned());
        }

        return Ok((program, args));
    }

    // Fallback: try to find a shell in the rootfs
    for shell in &["/bin/sh", "/bin/bash", "/usr/bin/sh"] {
        if rootfs.join(shell.trim_start_matches('/')).exists() {
            return Ok((shell.to_string(), vec![]));
        }
    }

    Err(AgentError::Runtime(
        "No command specified and no shell found in rootfs".to_string(),
    ))
}
```

### Step 8: Networking Without Namespaces

**Why**: macOS has no network namespaces. All sandboxed processes share the host network stack. This means two services binding the same port will conflict. ZLayer handles this through its proxy layer.

#### How It Works

1. **All services share 127.0.0.1**: The `get_container_ip()` method returns `127.0.0.1` for all macOS sandboxed containers.

2. **Port differentiation**: Each service binds a different port (from `spec.endpoints[].port`). The Seatbelt profile restricts which ports each process can bind via `(allow network-bind (local ip "localhost:PORT"))`.

3. **ZLayer proxy still works**: The proxy manager at `crates/zlayer-agent/src/proxy_manager.rs` binds to the public-facing ports and routes traffic to backend `127.0.0.1:PORT` addresses. This is the same pattern as when overlay networking is unavailable on Linux.

4. **Seatbelt replaces network namespaces**: Instead of isolating network stacks, Seatbelt restricts which network operations each process can perform:
   - A web service can only bind its configured port
   - A worker service might have no inbound network access at all
   - Outbound access can be restricted to specific hosts/ports

5. **Service-to-service communication**: Services communicate through localhost. The DNS server and proxy manager handle service name resolution. This is transparent to the application -- it connects to `servicename:port` and the proxy routes it.

#### Integration with OverlayManager

The `OverlayManager` at `crates/zlayer-agent/src/overlay_manager.rs` needs no changes. It already handles the "no overlay" case:

- When `get_container_pid()` returns a PID but there is no overlay configured, the overlay manager skips container attachment.
- When `get_container_ip()` returns an IP, the service manager uses it directly for proxy backend registration.
- On macOS, the overlay manager simply never creates overlay interfaces (WireGuard/boringtun requires Linux tun devices).

### Step 9: Integration with ServiceManager and Daemon

#### ServiceManager Behavior on macOS

The `ServiceManager` at `crates/zlayer-agent/src/service.rs` interacts purely through the `Runtime` trait. On macOS with the `SandboxRuntime`:

1. **`scale_to()`** calls `pull_image` -> `create_container` -> `start_container` in sequence. All work correctly with the sandbox runtime.

2. **Overlay attachment** (lines ~120-160 of service.rs): The service manager calls `get_container_pid()` after starting. If overlay is configured, it tries to attach. On macOS, overlay attachment is skipped (no tun device support). The fallback path calls `get_container_ip()` which returns `127.0.0.1`.

3. **Health monitoring**: The health checker connects to `127.0.0.1:PORT` for HTTP/TCP health checks. This works identically on macOS.

4. **Proxy registration**: The proxy manager registers backends as `127.0.0.1:PORT`. Traffic routing works unchanged.

#### Daemon Initialization

The runtime binary at `bin/runtime/src/` needs macOS-specific initialization:

```rust
// In bin/runtime/src/commands/node.rs or equivalent daemon startup

#[cfg(target_os = "macos")]
fn macos_daemon_init() -> Result<()> {
    // 1. Check for SIP (System Integrity Protection) status
    // SIP does not affect sandbox-exec, but good to log
    let sip_status = std::process::Command::new("csrutil")
        .arg("status")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_else(|_| "unknown".to_string());
    tracing::info!(sip = %sip_status.trim(), "macOS SIP status");

    // 2. Verify sandbox-exec exists and is executable
    if !Path::new("/usr/bin/sandbox-exec").exists() {
        tracing::warn!("sandbox-exec not found -- Seatbelt sandboxing may not work");
    }

    // 3. Create state directory (may need sudo for /var/lib/zlayer/)
    let state_dir = Path::new("/var/lib/zlayer/containers");
    if !state_dir.exists() {
        std::fs::create_dir_all(state_dir)?;
    }

    // 4. Detect GPU
    let gpus = crate::detect_gpus();
    for gpu in &gpus {
        tracing::info!(
            vendor = %gpu.vendor,
            model = %gpu.model,
            memory_mb = gpu.memory_mb,
            "Detected GPU"
        );
    }

    Ok(())
}
```

---

## 3.3 Seatbelt Profile Reference

### Complete Annotated Profile: Metal/MPS ML Workload

This profile is suitable for running PyTorch with `device="mps"`, Apple MLX, TensorFlow Metal, or any Metal-based compute workload.

```scheme
; ===========================================================================
; ZLayer Seatbelt Profile: Metal/MPS ML Workload
; Generated by zlayer-agent SandboxRuntime
;
; This profile provides:
;   - Full Metal/MPS GPU access (IOKit + Mach + frameworks)
;   - Read-write access to the container rootfs
;   - Read-write access to configured volume mounts
;   - Localhost-only network access on configured ports
;   - Process fork/exec within the sandbox
;   - Resource usage monitoring (rusage, pidinfo)
;
; Denied by default:
;   - All filesystem access outside rootfs and explicit mounts
;   - All network access outside configured ports
;   - Sending signals to processes outside this sandbox
;   - Raw sockets, kernel extensions, sysctl writes
;   - Mach services not needed for Metal/MPS
; ===========================================================================

(version 1)
(deny default)

; Uncomment for development debugging:
; (debug deny)

; ---------------------------------------------------------------------------
; Section 1: Process Fundamentals
; ---------------------------------------------------------------------------

; Allow the sandboxed process to exec binaries and fork children.
; Children inherit this sandbox -- they cannot escape or loosen it.
(allow process-exec)
(allow process-fork)

; Allow sending signals only to processes in the same sandbox.
; Prevents the workload from signaling other system processes.
(allow signal (target same-sandbox))

; Allow reading own process info (needed for memory monitoring, etc.)
(allow process-info* (target self))
(allow process-info-pidinfo)
(allow process-info-rusage)

; ---------------------------------------------------------------------------
; Section 2: System Libraries and Frameworks
; ---------------------------------------------------------------------------

; The dynamic linker (dyld) needs to read and map system libraries.
; Without these, no binary can execute.
(allow file-read*
  (subpath "/usr/lib")
  (subpath "/System/Library/Frameworks")
  (subpath "/System/Library/PrivateFrameworks")
  (subpath "/System/Library/Extensions")
  (subpath "/System/Library/ColorSync")
  (literal "/")
  (literal "/dev/random")
  (literal "/dev/urandom"))

; dyld needs to memory-map libraries as executable code
(allow file-map-executable
  (subpath "/usr/lib")
  (subpath "/System/Library/Frameworks")
  (subpath "/System/Library/PrivateFrameworks")
  (subpath "/System/Library/Extensions"))

; Hardware detection (CPU count, architecture, memory size)
(allow sysctl-read)
(allow system-info)

; Basic Mach service for DNS/directory lookups
(allow mach-lookup
  (global-name "com.apple.system.opendirectoryd.libinfo"))

; ---------------------------------------------------------------------------
; Section 3: Container Rootfs (APFS clone, CoW)
; ---------------------------------------------------------------------------

; Read-write access to the container's isolated filesystem.
; This is an APFS clone -- modifications only affect this container.
(allow file-read* file-write*
  (subpath "/var/lib/zlayer/containers/myservice-rep-1/rootfs"))

; Allow executing binaries from the rootfs (Python, model server, etc.)
(allow file-map-executable
  (subpath "/var/lib/zlayer/containers/myservice-rep-1/rootfs"))

; Container workspace (logs, config, temp files)
(allow file-read* file-write*
  (subpath "/var/lib/zlayer/containers/myservice-rep-1"))

; ---------------------------------------------------------------------------
; Section 4: Volume Mounts
; ---------------------------------------------------------------------------

; Model weights directory (read-only -- shared across replicas)
; (allow file-read* (subpath "/data/models"))

; Output directory (writable -- for generated results)
; (allow file-read* file-write* (subpath "/data/output"))

; Temp directory
(allow file-read* file-write*
  (subpath "/var/lib/zlayer/containers/myservice-rep-1/tmp"))

; ---------------------------------------------------------------------------
; Section 5: Metal/MPS GPU Access
; ---------------------------------------------------------------------------

; IOKit user clients for GPU hardware.
; These classes correspond to kernel drivers that manage the GPU.
; IOSurfaceRoot: GPU memory surface management
; IOAccelDevice2/Context2: GPU device and command submission
; AGXAccelerator: Apple Silicon GPU driver (M1/M2/M3/M4)
(allow iokit-open
  (iokit-user-client-class "IOSurfaceRoot")
  (iokit-user-client-class "IOSurfaceRootUserClient")
  (iokit-user-client-class "IOAccelDevice2")
  (iokit-user-client-class "IOAccelContext2")
  (iokit-user-client-class "IOAccelSharedUserClient2")
  (iokit-user-client-class "IOAccelSubmitter2")
  (iokit-user-client-class "RootDomainUserClient")
  (iokit-user-client-class "AGXAccelerator")
  ; Intel GPU support (for Intel Macs running ML workloads)
  (iokit-user-client-class "IOAccelerator")
  (iokit-user-client-class "IntelAccelerator"))

; IOKit properties needed for GPU enumeration and Metal initialization
(allow iokit-get-properties
  (iokit-property "MetalPluginClassName")
  (iokit-property "MetalPluginName")
  (iokit-property "MetalStatisticsName")
  (iokit-property "MetalStatisticsScriptName")
  (iokit-property "MetalCoalesce")
  (iokit-property "IOGLBundleName")
  (iokit-property "IOGLESBundleName")
  (iokit-property "IOGLESDefaultUseMetal")
  (iokit-property "IOGLESMetalBundleName")
  (iokit-property "GPUConfigurationVariable")
  (iokit-property "GPUDCCDisplayable")
  (iokit-property "GPUDebugNullClientMask")
  (iokit-property "GpuDebugPolicy")
  (iokit-property "GPURawCounterBundleName")
  (iokit-property "CompactVRAM")
  (iokit-property "EnableBlitLib")
  (iokit-property "IOPCIMatch")
  (iokit-property "model")
  (iokit-property "vendor-id")
  (iokit-property "device-id")
  (iokit-property "class-code")
  (iokit-property "soc-generation")
  (iokit-property "IORegistryEntryPropertyKeys"))

; Mach services for Metal shader compilation.
; MTLCompilerService compiles Metal shaders at runtime.
; AGXCompilerService handles Apple Silicon GPU-specific compilation.
; CARenderServer is for Core Animation (some ML frameworks use it).
(allow mach-lookup
  (global-name "com.apple.MTLCompilerService")
  (global-name "com.apple.CARenderServer")
  (global-name "com.apple.PowerManagement.control")
  (global-name "com.apple.gpu.process"))

(allow mach-lookup
  (xpc-service-name "com.apple.MTLCompilerService")
  (xpc-service-name-prefix "com.apple.AGXCompilerService"))

; User preferences for Metal/OpenGL configuration
(allow user-preference-read
  (preference-domain "com.apple.opengl")
  (preference-domain "com.apple.Metal")
  (preference-domain "com.nvidia.OpenGL"))

; GPU driver bundles and Metal framework files
(allow file-read*
  (subpath "/Library/GPUBundles")
  (subpath "/System/Library/Frameworks/Metal.framework")
  (subpath "/System/Library/Frameworks/MetalPerformanceShaders.framework")
  (subpath "/System/Library/Frameworks/MetalPerformanceShadersGraph.framework")
  (subpath "/System/Library/PrivateFrameworks/GPUCompiler.framework"))

; ---------------------------------------------------------------------------
; Section 6: Network Access
; ---------------------------------------------------------------------------

; Bind to the service's configured port on localhost only
(allow network-bind (local ip "localhost:8080"))

; Allow inbound connections on localhost (from ZLayer proxy)
(allow network-inbound (local ip "localhost:*"))

; Allow outbound to common services and other ZLayer services
(allow network-outbound (remote ip "localhost:*"))

; For services that need external network access, uncomment:
; (allow network-outbound)
; (allow network-bind)
; (allow system-socket)

; ---------------------------------------------------------------------------
; Section 7: I/O Essentials
; ---------------------------------------------------------------------------

; /dev/null access (many programs write here)
(allow file-write-data
  (require-all (literal "/dev/null") (vnode-type CHARACTER-DEVICE)))
(allow file-read-data
  (require-all (literal "/dev/null") (vnode-type CHARACTER-DEVICE)))

; Pseudo-terminal support (for interactive debugging if needed)
(allow pseudo-tty)
(allow file-read* file-write* file-ioctl (literal "/dev/ptmx"))

; IPC primitives (POSIX semaphores and shared memory)
; Required for multi-threaded ML frameworks
(allow ipc-posix-sem)
(allow ipc-posix-shm)
```

### Complete Annotated Profile: Web Service (No GPU)

This is a minimal profile for a web service that does not need GPU access.

```scheme
; ===========================================================================
; ZLayer Seatbelt Profile: Web Service (No GPU)
; ===========================================================================

(version 1)
(deny default)

; --- Process ---
(allow process-exec)
(allow process-fork)
(allow signal (target same-sandbox))
(allow process-info* (target self))
(allow process-info-pidinfo)
(allow process-info-rusage)

; --- System libraries ---
(allow file-read*
  (subpath "/usr/lib")
  (subpath "/System/Library/Frameworks")
  (subpath "/System/Library/PrivateFrameworks")
  (literal "/")
  (literal "/dev/random")
  (literal "/dev/urandom"))

(allow file-map-executable
  (subpath "/usr/lib")
  (subpath "/System/Library/Frameworks")
  (subpath "/System/Library/PrivateFrameworks"))

(allow sysctl-read)
(allow system-info)

(allow mach-lookup
  (global-name "com.apple.system.opendirectoryd.libinfo"))

; --- Container rootfs ---
(allow file-read* file-write*
  (subpath "/var/lib/zlayer/containers/webservice-rep-1/rootfs"))
(allow file-map-executable
  (subpath "/var/lib/zlayer/containers/webservice-rep-1/rootfs"))
(allow file-read* file-write*
  (subpath "/var/lib/zlayer/containers/webservice-rep-1"))

; --- Network: full HTTP/HTTPS service ---
(allow network-bind (local ip "localhost:3000"))
(allow network-inbound (local ip "localhost:*"))
(allow network-outbound)
(allow system-socket)

; --- I/O ---
(allow file-write-data
  (require-all (literal "/dev/null") (vnode-type CHARACTER-DEVICE)))
(allow file-read-data
  (require-all (literal "/dev/null") (vnode-type CHARACTER-DEVICE)))
(allow pseudo-tty)
(allow file-read* file-write* file-ioctl (literal "/dev/ptmx"))
(allow ipc-posix-sem)
(allow ipc-posix-shm)
```

### Adding Custom Permissions from ZLayer Spec

The ZLayer spec can be extended to allow custom Seatbelt permissions. This would be done through a new `sandbox` field in `ServiceSpec`:

```yaml
services:
  my-ml-service:
    image:
      name: my-ml-image:latest
    resources:
      gpu:
        vendor: "apple"
        mode: "native"
    sandbox:
      allow_mach_services:
        - "com.apple.custom.service"
      allow_read_paths:
        - "/opt/shared-models"
      allow_write_paths:
        - "/opt/output"
      allow_network_outbound: true
```

The profile generator would append these as additional `(allow ...)` rules.

---

## 3.4 Limitations and Mitigations

### Weaker Isolation Than Linux Containers

| Aspect | Linux Container (Youki) | macOS Sandbox (Seatbelt) |
|--------|------------------------|--------------------------|
| Filesystem | Mount namespace + overlayfs | APFS clone + subpath rules |
| Network | Network namespace + veth pair | Shared host network + Seatbelt rules |
| PID | PID namespace (isolated view) | Shared PID space (can see other PIDs) |
| Users | User namespace (root mapping) | Runs as invoking user |
| IPC | IPC namespace | Shared (but restricted by Seatbelt) |
| Memory | cgroups v2 hard limit | Watchdog thread (not kernel-enforced) |
| CPU | cgroups v2 quota/period | setrlimit RLIMIT_CPU (total time only) |

**Mitigation**: The Seatbelt sandbox is a Mandatory Access Control system enforced at the kernel level. While weaker than Linux namespaces + cgroups, it provides meaningful isolation for the primary use case: running ML workloads with GPU access. For workloads requiring stronger isolation, use Approach B (libkrun VM).

### No Network Namespace Separation

All sandboxed processes share the host network stack. Port conflicts are possible.

**Mitigation**: ZLayer's proxy manager handles port allocation and traffic routing. Each service binds a unique port. The Seatbelt profile restricts which ports each process can access.

### No cgroup-Equivalent Resource Limits

Memory and CPU limits are not kernel-enforced on macOS.

**Mitigation**: The memory watchdog thread polls RSS every 2 seconds and kills processes that exceed the limit. This is not as precise as cgroups but is effective for preventing runaway memory consumption. CPU limiting uses `RLIMIT_CPU` which enforces total CPU time (not rate).

### Cannot Run Linux Binaries

The Seatbelt sandbox runs macOS-native binaries only. Linux container images will not work.

**Mitigation**: For Linux container images, use Approach B (libkrun VM runtime). The hybrid mode automatically selects the right runtime based on the spec.

### Best Use Cases

- PyTorch with MPS backend (`device="mps"`)
- Apple MLX framework
- TensorFlow Metal
- Core ML inference
- Any Metal compute shader workload
- macOS-native web services and APIs
- Development and testing workloads

---

# Part 4: Approach B -- libkrun VM Runtime (Backup)

## 4.1 How It Works

### microVM Model

libkrun is a Rust-based virtual machine monitor (VMM) that uses Apple's Hypervisor.framework to create lightweight microVMs. Each "container" is actually a minimal Linux VM:

```
macOS Host
  |
  +-- ZLayer VmRuntime
       |
       +-- libkrun microVM
            |
            +-- Linux kernel (built into libkrun)
            |
            +-- Container rootfs (extracted OCI image)
            |
            +-- GPU: Venus-Vulkan (25-30%) or ggml API remoting (95-98%)
            |
            +-- Networking: TSI (Transparent Socket Impersonation)
```

Key characteristics:
- **Boot time**: 100-300ms
- **Memory overhead**: ~100MB per VM
- **CPU overhead**: Near-native (hardware virtualization)
- **GPU**: Two modes -- Venus-Vulkan protocol (~30% native) or ggml API remoting (~97% native)
- **Networking**: TSI maps guest sockets through the host process
- **Compatibility**: Full Linux OCI containers

### GPU Forwarding Pipeline

libkrun offers two GPU paths on Apple Silicon:

1. **Venus-Vulkan** (general purpose):
   ```
   Guest Linux → Vulkan API → virtio-gpu → Venus protocol →
     Host macOS → virglrenderer → MoltenVK → Metal → Apple GPU
   ```
   Performance: 25-30% of native Metal. Acceptable for light GPU work.

2. **ggml API Remoting** (ML inference):
   ```
   Guest Linux → ggml operations → virtio-gpu custom channel →
     Host macOS → ggml Metal backend → Metal → Apple GPU
   ```
   Performance: 95-98% of native Metal. Excellent for inference workloads using llama.cpp, whisper.cpp, or other ggml-based frameworks.

### TSI Networking

Transparent Socket Impersonation (TSI) is libkrun's networking approach. Instead of a virtual NIC, guest socket operations are transparently forwarded to the host:

- Guest `connect()` → Host `connect()` (traffic appears from host process)
- Guest `bind()` + `listen()` → Host `bind()` + `listen()` (port appears on host)
- No virtual NIC, no MAC address, no IP configuration needed
- Works naturally with ZLayer's proxy and overlay managers

## 4.2 Implementation Guide -- Step by Step

### Step 1: libkrun Installation and Dynamic Loading

**File**: `crates/zlayer-agent/src/runtimes/macos_vm.rs` (NEW)

**Why**: libkrun is loaded dynamically via `dlopen` rather than linked at compile time. This avoids making the entire ZLayer binary depend on libkrun being installed.

```rust
//! macOS libkrun VM runtime
//!
//! Implements the Runtime trait using libkrun microVMs for
//! Linux container compatibility with GPU forwarding on macOS.

use crate::cgroups_stats::ContainerStats;
use crate::error::{AgentError, Result};
use crate::runtime::{ContainerId, ContainerState, Runtime};
use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::net::IpAddr;
use std::os::raw::{c_char, c_int, c_uint};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use zlayer_spec::ServiceSpec;

/// libkrun C API function types (loaded via dlopen)
///
/// These correspond to the functions in libkrun.h:
/// https://github.com/containers/libkrun/blob/main/include/libkrun.h
type KrunSetLogLevel = unsafe extern "C" fn(level: c_uint) -> c_int;
type KrunCreateCtx = unsafe extern "C" fn() -> c_int;
type KrunFreeCtx = unsafe extern "C" fn(ctx_id: c_uint) -> c_int;
type KrunSetVmConfig = unsafe extern "C" fn(ctx_id: c_uint, num_vcpus: c_uint, ram_mib: c_uint) -> c_int;
type KrunSetRoot = unsafe extern "C" fn(ctx_id: c_uint, root_path: *const c_char) -> c_int;
type KrunSetWorkdir = unsafe extern "C" fn(ctx_id: c_uint, workdir_path: *const c_char) -> c_int;
type KrunSetExec = unsafe extern "C" fn(ctx_id: c_uint, exec_path: *const c_char, argv: *const *const c_char, envp: *const *const c_char) -> c_int;
type KrunSetGpu = unsafe extern "C" fn(ctx_id: c_uint, virgl_flags: c_uint) -> c_int;
type KrunSetTsi = unsafe extern "C" fn(ctx_id: c_uint) -> c_int;
type KrunStartEnter = unsafe extern "C" fn(ctx_id: c_uint) -> c_int;

/// Dynamically loaded libkrun library
struct LibKrun {
    _lib: libloading::Library,
    set_log_level: KrunSetLogLevel,
    create_ctx: KrunCreateCtx,
    free_ctx: KrunFreeCtx,
    set_vm_config: KrunSetVmConfig,
    set_root: KrunSetRoot,
    set_workdir: KrunSetWorkdir,
    set_exec: KrunSetExec,
    set_gpu: KrunSetGpu,
    set_tsi: KrunSetTsi,
    start_enter: KrunStartEnter,
}

impl LibKrun {
    /// Load libkrun dynamically. Searches standard library paths.
    fn load() -> Result<Self> {
        let lib_paths = [
            "libkrun.dylib",
            "/usr/local/lib/libkrun.dylib",
            "/opt/homebrew/lib/libkrun.dylib",
        ];

        let lib = lib_paths
            .iter()
            .find_map(|path| unsafe { libloading::Library::new(path).ok() })
            .ok_or_else(|| AgentError::Configuration(
                "libkrun not found. Install via: brew install libkrun".to_string(),
            ))?;

        unsafe {
            let set_log_level: KrunSetLogLevel = *lib.get(b"krun_set_log_level\0")
                .map_err(|e| AgentError::Configuration(format!("libkrun symbol error: {}", e)))?;
            let create_ctx: KrunCreateCtx = *lib.get(b"krun_create_ctx\0")
                .map_err(|e| AgentError::Configuration(format!("libkrun symbol error: {}", e)))?;
            let free_ctx: KrunFreeCtx = *lib.get(b"krun_free_ctx\0")
                .map_err(|e| AgentError::Configuration(format!("libkrun symbol error: {}", e)))?;
            let set_vm_config: KrunSetVmConfig = *lib.get(b"krun_set_vm_config\0")
                .map_err(|e| AgentError::Configuration(format!("libkrun symbol error: {}", e)))?;
            let set_root: KrunSetRoot = *lib.get(b"krun_set_root\0")
                .map_err(|e| AgentError::Configuration(format!("libkrun symbol error: {}", e)))?;
            let set_workdir: KrunSetWorkdir = *lib.get(b"krun_set_workdir\0")
                .map_err(|e| AgentError::Configuration(format!("libkrun symbol error: {}", e)))?;
            let set_exec: KrunSetExec = *lib.get(b"krun_set_exec\0")
                .map_err(|e| AgentError::Configuration(format!("libkrun symbol error: {}", e)))?;
            let set_gpu: KrunSetGpu = *lib.get(b"krun_set_gpu\0")
                .map_err(|e| AgentError::Configuration(format!("libkrun symbol error: {}", e)))?;
            let set_tsi: KrunSetTsi = *lib.get(b"krun_set_tsi\0")
                .map_err(|e| AgentError::Configuration(format!("libkrun symbol error: {}", e)))?;
            let start_enter: KrunStartEnter = *lib.get(b"krun_start_enter\0")
                .map_err(|e| AgentError::Configuration(format!("libkrun symbol error: {}", e)))?;

            Ok(Self {
                _lib: lib,
                set_log_level,
                create_ctx,
                free_ctx,
                set_vm_config,
                set_root,
                set_workdir,
                set_exec,
                set_gpu,
                set_tsi,
                start_enter,
            })
        }
    }
}
```

### Step 2: VmRuntime Struct and Runtime Trait Implementation

```rust
/// Metadata for a running libkrun VM
struct VmProcess {
    ctx_id: u32,
    state: ContainerState,
    state_dir: PathBuf,
    rootfs_dir: PathBuf,
    started_at: Instant,
    /// Thread handle for the VM (krun_start_enter blocks)
    vm_thread: Option<std::thread::JoinHandle<i32>>,
}

/// macOS libkrun VM runtime
pub struct VmRuntime {
    state_dir: PathBuf,
    libkrun: Arc<LibKrun>,
    processes: Arc<RwLock<HashMap<String, VmProcess>>>,
}

impl VmRuntime {
    pub async fn new() -> Result<Self> {
        let state_dir = PathBuf::from("/var/lib/zlayer/vms");
        tokio::fs::create_dir_all(&state_dir).await.map_err(|e| {
            AgentError::Configuration(format!("Failed to create VM state dir: {}", e))
        })?;

        let libkrun = LibKrun::load()?;

        // Set log level (0=off, 1=error, 2=warn, 3=info, 4=debug)
        unsafe { (libkrun.set_log_level)(2) };

        Ok(Self {
            state_dir,
            libkrun: Arc::new(libkrun),
            processes: Arc::new(RwLock::new(HashMap::new())),
        })
    }
}
```

The Runtime trait implementation for VmRuntime follows the same pattern as SandboxRuntime but uses libkrun API calls instead of fork+sandbox_init:

- **`pull_image`**: Same as SandboxRuntime -- uses zlayer-registry to extract OCI layers
- **`create_container`**: Extracts rootfs, creates libkrun context (`krun_create_ctx`), configures VM (`krun_set_vm_config`), sets rootfs (`krun_set_root`), configures GPU (`krun_set_gpu`), enables TSI networking (`krun_set_tsi`)
- **`start_container`**: Calls `krun_start_enter` on a background thread (it blocks until the VM exits)
- **`stop_container`**: Calls `krun_free_ctx` to destroy the VM context
- **`container_state`**: Checks if the VM thread is still running
- **`exec`**: Not directly supported by libkrun -- would require vsock communication with an agent inside the VM
- **`get_container_stats`**: Reads from the host process's resource usage (the VM runs within a host thread)
- **`get_container_ip`**: Returns `127.0.0.1` (TSI maps guest networking through host)

### Step 3: GPU Configuration

```rust
/// Configure GPU forwarding for a libkrun VM.
///
/// libkrun GPU flags:
/// - 0: No GPU
/// - VIRGL_RENDERER_USE_EGL (1 << 0): Use EGL (not relevant on macOS)
/// - VIRGL_RENDERER_THREAD_SYNC (1 << 1): Thread sync
/// - VIRGL_RENDERER_USE_GLES (1 << 2): Use GLES
///
/// For Apple Silicon with Metal, the flags are typically 0 (libkrun
/// auto-detects and uses its Metal/Venus backend).
fn configure_gpu(libkrun: &LibKrun, ctx_id: u32) -> Result<()> {
    let ret = unsafe { (libkrun.set_gpu)(ctx_id, 0) };
    if ret != 0 {
        return Err(AgentError::Runtime(format!(
            "krun_set_gpu failed with code {}", ret
        )));
    }
    Ok(())
}
```

### Step 4: Networking via TSI + Overlay

TSI networking maps guest sockets through the host process. From the host's perspective, network traffic appears to originate from the ZLayer process itself. This means:

1. The proxy manager works unchanged -- it routes traffic to `127.0.0.1:PORT`
2. Guest services bind ports that appear on the host
3. No virtual NIC configuration needed
4. Works with ZLayer's overlay network (traffic from the VM appears from the host's overlay IP)

```rust
fn configure_networking(libkrun: &LibKrun, ctx_id: u32) -> Result<()> {
    let ret = unsafe { (libkrun.set_tsi)(ctx_id) };
    if ret != 0 {
        return Err(AgentError::Runtime(format!(
            "krun_set_tsi failed with code {}", ret
        )));
    }
    Ok(())
}
```

### Step 5: OCI Image to Rootfs Extraction

libkrun expects a prepared rootfs directory via `krun_set_root()`. ZLayer's existing registry crate (`zlayer-registry`) handles OCI image pulling and layer extraction. The flow:

1. `pull_image()` downloads OCI layers via `zlayer-registry::ImagePuller`
2. Layers are extracted to `{state_dir}/images/{image_name}/rootfs/`
3. `create_container()` points libkrun at this rootfs via `krun_set_root()`

No APFS clonefile is needed for VMs -- each VM has its own filesystem view through the virtualization layer.

## 4.3 Limitations

### One GPU Container at a Time

libkrun currently supports only one GPU-enabled VM at a time per host. The Metal GPU context cannot be shared across multiple VMs. This is a hardware/driver limitation.

**Mitigation**: For multi-tenant GPU workloads, use Approach A (Seatbelt sandbox) which allows multiple processes to share the GPU through Metal's built-in multi-process support.

### Intel Macs Not Supported

libkrun with Hypervisor.framework requires Apple Silicon (M1 or later). Intel Macs are not supported.

**Mitigation**: On Intel Macs, fall back to Docker Desktop (no GPU access) or Approach A (Seatbelt sandbox) for macOS-native workloads.

### 100MB Memory Overhead Per VM

Each libkrun VM reserves approximately 100MB of RAM for the Linux kernel and VM infrastructure, in addition to the memory allocated to the guest workload.

**Mitigation**: Use Approach A for lightweight workloads. Reserve VMs for workloads that genuinely need Linux container compatibility.

### Network Throughput

TSI networking achieves approximately 55-60% of native network throughput due to the socket forwarding overhead.

**Mitigation**: For network-intensive workloads, prefer Approach A. For GPU-bound ML inference where network is not the bottleneck, this is acceptable.

### vCPUs Must Not Exceed Host Cores

Configuring more vCPUs than the host has physical cores causes a silent hang. libkrun does not validate this.

**Mitigation**: ZLayer's VmRuntime should detect the host core count and clamp vCPU allocation:

```rust
fn safe_vcpu_count(requested: u32) -> u32 {
    let host_cores = num_cpus::get() as u32;
    requested.min(host_cores)
}
```

---

# Part 5: Hybrid Mode

## Automatic Selection

When a deployment spec specifies `resources.gpu.vendor: "apple"`, ZLayer automatically selects the runtime based on the `mode` field:

| `vendor` | `mode` | Runtime | Use Case |
|----------|--------|---------|----------|
| `apple` | `native` (default) | SandboxRuntime | macOS-native binaries + MPS |
| `apple` | `vm` | VmRuntime | Linux containers + GPU |
| `nvidia` | (any) | YoukiRuntime/Docker | Linux + NVIDIA GPU |
| `amd` | (any) | YoukiRuntime/Docker | Linux + AMD GPU |
| (none) | (none) | Auto-detected | No GPU |

The selection logic lives in the `ServiceManager` or a new runtime selector:

```rust
/// Select the appropriate runtime for a service based on its spec.
///
/// This function examines the GPU configuration in the ServiceSpec
/// and returns the correct RuntimeConfig for the platform.
pub fn select_runtime_for_spec(spec: &ServiceSpec) -> RuntimeConfig {
    let gpu = spec.resources.as_ref().and_then(|r| r.gpu.as_ref());

    match gpu {
        Some(gpu_spec) if gpu_spec.vendor == "apple" => {
            let mode = gpu_spec.mode.as_deref().unwrap_or("native");
            match mode {
                "native" => {
                    #[cfg(target_os = "macos")]
                    { RuntimeConfig::MacSandbox }
                    #[cfg(not(target_os = "macos"))]
                    {
                        tracing::warn!("Apple GPU requested but not running on macOS");
                        RuntimeConfig::Auto
                    }
                }
                "vm" => {
                    #[cfg(target_os = "macos")]
                    { RuntimeConfig::MacVm }
                    #[cfg(not(target_os = "macos"))]
                    {
                        tracing::warn!("Apple VM GPU requested but not running on macOS");
                        RuntimeConfig::Auto
                    }
                }
                other => {
                    tracing::warn!(mode = %other, "Unknown GPU mode, falling back to auto");
                    RuntimeConfig::Auto
                }
            }
        }
        _ => RuntimeConfig::Auto,
    }
}
```

## Spec Syntax

### macOS-native MPS workload (100% GPU performance)

```yaml
version: v1
deployment: ml-inference
services:
  inference-server:
    rtype: service
    image:
      name: my-registry/mps-model-server:latest
    command: ["python", "-m", "model_server", "--device", "mps"]
    endpoints:
      - name: grpc
        protocol: http
        port: 8080
    resources:
      cpu: 4.0
      memory: "16Gi"
      gpu:
        count: 1
        vendor: "apple"
        mode: "native"    # Direct Metal/MPS via Seatbelt sandbox
    scaling:
      mode: fixed
      replicas: 1
```

### Linux container with GPU forwarding

```yaml
version: v1
deployment: llama-inference
services:
  llama-server:
    rtype: service
    image:
      name: ghcr.io/ggerganov/llama.cpp:server
    command: ["/server", "-m", "/models/llama-7b.gguf", "--port", "8080"]
    endpoints:
      - name: http
        protocol: http
        port: 8080
    resources:
      cpu: 8.0
      memory: "32Gi"
      gpu:
        count: 1
        vendor: "apple"
        mode: "vm"        # libkrun VM with ggml API remoting (~97% native)
    storage:
      volumes:
        - name: models
          mount_path: /models
          host_path: /data/models
    scaling:
      mode: fixed
      replicas: 1
```

### Mixed cluster deployment (macOS + Linux)

```yaml
version: v1
deployment: ml-pipeline
services:
  # Runs on macOS nodes with Apple GPU
  mps-inference:
    rtype: service
    image:
      name: my-registry/mps-inference:latest
    resources:
      gpu:
        vendor: "apple"
        mode: "native"
    endpoints:
      - name: http
        protocol: http
        port: 8080

  # Runs on Linux nodes with NVIDIA GPU
  cuda-training:
    rtype: service
    image:
      name: my-registry/cuda-training:latest
    resources:
      gpu:
        count: 4
        vendor: "nvidia"
    endpoints:
      - name: http
        protocol: http
        port: 9090
```

## Scheduler Integration for Mixed Clusters

The scheduler at `crates/zlayer-scheduler/src/placement.rs` already supports GPU-aware placement. The `can_place_on_node()` function rejects nodes with GPU vendor mismatch. For mixed clusters:

1. **Node registration**: Each node reports its GPU inventory via `detect_gpus()`. macOS nodes report `vendor: "apple"`. Linux nodes report `vendor: "nvidia"` / `vendor: "amd"`.

2. **Placement**: The scheduler's `can_place_on_node()` checks `gpu_spec.vendor` against `node.gpu_vendor`. A service with `vendor: "apple"` will only be placed on macOS nodes.

3. **Unified memory awareness**: Apple Silicon uses unified memory. The scheduler should account for the fact that GPU memory allocation on Apple Silicon reduces available system memory. The `gpu_memory_mb` for Apple nodes equals `system_ram_mb`. When placing a GPU workload, the scheduler should subtract the estimated GPU memory usage from available system memory.

```rust
// In crates/zlayer-scheduler/src/placement.rs

/// Check if a node can host a service, accounting for unified memory on Apple Silicon
fn check_memory_with_unified_gpu(
    node: &NodeResources,
    spec: &ServiceSpec,
) -> bool {
    let base_memory_needed = parse_memory_spec(&spec.resources);

    // On Apple Silicon, GPU memory comes from system RAM
    let gpu_memory_overhead = if node.gpu_vendor == "apple" {
        spec.resources.as_ref()
            .and_then(|r| r.gpu.as_ref())
            .map(|_| {
                // Estimate: GPU workloads typically use 2-8GB of unified memory
                // for MPS operations. Use a conservative estimate.
                2 * 1024 * 1024 * 1024u64 // 2GB default GPU overhead
            })
            .unwrap_or(0)
    } else {
        0 // Discrete GPU has its own VRAM
    };

    let total_memory_needed = base_memory_needed + gpu_memory_overhead;
    node.available_memory() >= total_memory_needed
}
```

---

# Part 6: Testing Strategy

## Unit Tests with Mock GPU Detection

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_sandbox_profile_no_gpu() {
        let config = SandboxConfig {
            rootfs_dir: PathBuf::from("/var/lib/zlayer/containers/test/rootfs"),
            workspace_dir: PathBuf::from("/var/lib/zlayer/containers/test"),
            gpu_access: GpuAccess::None,
            network_access: NetworkAccess::Full,
            writable_dirs: vec![],
            readonly_dirs: vec![],
            max_files: 4096,
            cpu_time_limit: None,
            memory_limit: None,
        };

        let profile = generate_sandbox_profile(&config);

        assert!(profile.contains("(version 1)"));
        assert!(profile.contains("(deny default)"));
        assert!(profile.contains("(allow process-exec)"));
        assert!(!profile.contains("IOSurfaceRoot")); // No GPU
        assert!(!profile.contains("MTLCompilerService")); // No GPU
        assert!(profile.contains("(allow network-outbound)")); // Full network
    }

    #[test]
    fn test_generate_sandbox_profile_metal_compute() {
        let config = SandboxConfig {
            rootfs_dir: PathBuf::from("/var/lib/zlayer/containers/ml/rootfs"),
            workspace_dir: PathBuf::from("/var/lib/zlayer/containers/ml"),
            gpu_access: GpuAccess::MetalCompute,
            network_access: NetworkAccess::LocalhostOnly {
                bind_ports: vec![8080],
                connect_ports: vec![53, 80, 443],
            },
            writable_dirs: vec![PathBuf::from("/data/output")],
            readonly_dirs: vec![PathBuf::from("/data/models")],
            max_files: 4096,
            cpu_time_limit: None,
            memory_limit: Some(16 * 1024 * 1024 * 1024),
        };

        let profile = generate_sandbox_profile(&config);

        // GPU access present
        assert!(profile.contains("IOSurfaceRoot"));
        assert!(profile.contains("AGXAccelerator"));
        assert!(profile.contains("MTLCompilerService"));
        assert!(profile.contains("/Library/GPUBundles"));

        // Network restricted to localhost
        assert!(profile.contains("localhost:8080"));
        assert!(!profile.contains("(allow network-outbound)\n")); // Not full

        // Volume mounts
        assert!(profile.contains("/data/output"));
        assert!(profile.contains("/data/models"));
    }

    #[test]
    fn test_generate_sandbox_profile_mps_only() {
        let config = SandboxConfig {
            rootfs_dir: PathBuf::from("/tmp/test/rootfs"),
            workspace_dir: PathBuf::from("/tmp/test"),
            gpu_access: GpuAccess::MpsOnly,
            network_access: NetworkAccess::None,
            writable_dirs: vec![],
            readonly_dirs: vec![],
            max_files: 1024,
            cpu_time_limit: Some(3600),
            memory_limit: None,
        };

        let profile = generate_sandbox_profile(&config);

        // MPS access (subset of Metal)
        assert!(profile.contains("IOSurfaceRoot"));
        assert!(profile.contains("AGXAccelerator"));
        assert!(profile.contains("MetalPerformanceShaders.framework"));

        // NO shader compilation
        assert!(!profile.contains("MTLCompilerService"));

        // No network
        assert!(profile.contains("Network: DENIED"));
    }

    #[test]
    fn test_parse_memory_string() {
        assert_eq!(parse_memory_string("512Mi"), Some(512 * 1024 * 1024));
        assert_eq!(parse_memory_string("2Gi"), Some(2 * 1024 * 1024 * 1024));
        assert_eq!(parse_memory_string("1024Ki"), Some(1024 * 1024));
        assert_eq!(parse_memory_string("1073741824"), Some(1073741824));
        assert_eq!(parse_memory_string("invalid"), None);
    }

    #[test]
    fn test_container_dir_name() {
        let id = ContainerId {
            service: "myservice".to_string(),
            replica: 3,
        };
        assert_eq!(SandboxRuntime::container_dir_name(&id), "myservice-rep-3");
    }

    #[test]
    fn test_select_runtime_for_spec_apple_native() {
        let spec = make_spec_with_gpu("apple", Some("native"));
        let config = select_runtime_for_spec(&spec);
        #[cfg(target_os = "macos")]
        assert!(matches!(config, RuntimeConfig::MacSandbox));
    }

    #[test]
    fn test_select_runtime_for_spec_apple_vm() {
        let spec = make_spec_with_gpu("apple", Some("vm"));
        let config = select_runtime_for_spec(&spec);
        #[cfg(target_os = "macos")]
        assert!(matches!(config, RuntimeConfig::MacVm));
    }

    #[test]
    fn test_select_runtime_for_spec_nvidia() {
        let spec = make_spec_with_gpu("nvidia", None);
        let config = select_runtime_for_spec(&spec);
        assert!(matches!(config, RuntimeConfig::Auto));
    }

    fn make_spec_with_gpu(vendor: &str, mode: Option<&str>) -> ServiceSpec {
        // Helper to create a ServiceSpec with GPU config
        // (uses serde_yaml parsing for convenience)
        let yaml = format!(
            r#"
rtype: service
image:
  name: test:latest
endpoints:
  - name: http
    protocol: http
    port: 8080
resources:
  gpu:
    count: 1
    vendor: "{}"
    {}
"#,
            vendor,
            mode.map(|m| format!("mode: \"{}\"", m)).unwrap_or_default(),
        );
        serde_yaml::from_str(&yaml).unwrap()
    }
}
```

## Integration Tests on macOS CI

These tests require a macOS runner with Apple Silicon:

```rust
#[cfg(all(test, target_os = "macos"))]
mod integration_tests {
    use super::*;

    /// Test that we can detect the Apple GPU
    #[test]
    fn test_apple_gpu_detection() {
        let gpus = detect_gpus();
        // On Apple Silicon Macs, should find at least one Apple GPU
        // On Intel Macs or CI VMs, may find AMD/Intel or none
        for gpu in &gpus {
            println!("Detected GPU: {:?}", gpu);
            assert!(!gpu.vendor.is_empty());
            assert!(!gpu.model.is_empty());
        }
    }

    /// Test that sandbox_init works
    #[test]
    fn test_seatbelt_basic() {
        // Apply a minimal sandbox that allows basic operations
        let profile = "(version 1)(allow default)";
        let result = apply_seatbelt_profile(profile);
        // Note: We can only test this in a forked child to avoid
        // sandboxing the test process itself
        // assert!(result.is_ok());
    }

    /// Test APFS clonefile
    #[tokio::test]
    async fn test_apfs_clonefile() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("source.txt");
        let dst = tmp.path().join("cloned.txt");

        std::fs::write(&src, "test content").unwrap();

        let result = tokio::task::spawn_blocking(move || {
            clone_file_apfs(&src, &dst)
        }).await.unwrap();

        match result {
            Ok(true) => {
                // APFS clone succeeded
                let content = std::fs::read_to_string(tmp.path().join("cloned.txt")).unwrap();
                assert_eq!(content, "test content");
            }
            Ok(false) => {
                // Not APFS -- expected on some CI environments
                println!("APFS clonefile not available (non-APFS volume)");
            }
            Err(e) => panic!("clonefile error: {}", e),
        }
    }

    /// Test full sandbox runtime lifecycle (requires macOS)
    #[tokio::test]
    async fn test_sandbox_runtime_lifecycle() {
        let tmp = tempfile::tempdir().unwrap();
        let runtime = SandboxRuntime::with_state_dir(tmp.path().to_path_buf())
            .await
            .unwrap();

        // Create a minimal rootfs with /bin/echo
        let image_dir = tmp.path().join("images").join("test_latest").join("rootfs");
        tokio::fs::create_dir_all(image_dir.join("bin")).await.unwrap();
        // Symlink to system echo
        std::os::unix::fs::symlink("/bin/echo", image_dir.join("bin/echo")).unwrap();

        let id = ContainerId {
            service: "test".to_string(),
            replica: 1,
        };

        let spec: ServiceSpec = serde_yaml::from_str(r#"
rtype: service
image:
  name: test:latest
command: ["/bin/echo", "hello from sandbox"]
endpoints:
  - name: http
    protocol: http
    port: 8080
"#).unwrap();

        // Lifecycle
        runtime.create_container(&id, &spec).await.unwrap();
        runtime.start_container(&id).await.unwrap();

        let exit_code = runtime.wait_container(&id).await.unwrap();
        assert_eq!(exit_code, 0);

        let logs = runtime.container_logs(&id, 0).await.unwrap();
        assert!(logs.contains("hello from sandbox"));

        runtime.remove_container(&id).await.unwrap();
    }
}
```

## GPU Capability Tests

```rust
#[cfg(all(test, target_os = "macos"))]
mod gpu_tests {
    use super::*;

    /// Test that a Metal compute workload runs in a sandbox
    /// Requires: macOS + Apple Silicon + Xcode Command Line Tools
    #[tokio::test]
    #[ignore] // Run manually: cargo test gpu_tests -- --ignored
    async fn test_metal_compute_in_sandbox() {
        // This test creates a minimal Metal compute program,
        // sandboxes it with GPU access, and verifies it completes
        // without sandbox violations.

        let profile = generate_sandbox_profile(&SandboxConfig {
            rootfs_dir: PathBuf::from("/tmp/zlayer-gpu-test"),
            workspace_dir: PathBuf::from("/tmp/zlayer-gpu-test"),
            gpu_access: GpuAccess::MetalCompute,
            network_access: NetworkAccess::None,
            writable_dirs: vec![PathBuf::from("/tmp/zlayer-gpu-test")],
            readonly_dirs: vec![],
            max_files: 4096,
            cpu_time_limit: None,
            memory_limit: None,
        });

        // Use swift to create a quick Metal test
        let swift_code = r#"
import Metal
guard let device = MTLCreateSystemDefaultDevice() else {
    print("FAIL: No Metal device")
    exit(1)
}
print("GPU: \(device.name)")
print("Unified memory: \(device.hasUnifiedMemory)")
print("Max threads per threadgroup: \(device.maxThreadsPerThreadgroup)")
print("PASS: Metal device initialized successfully")
"#;

        std::fs::create_dir_all("/tmp/zlayer-gpu-test").unwrap();
        std::fs::write("/tmp/zlayer-gpu-test/test.swift", swift_code).unwrap();

        let output = std::process::Command::new("/usr/bin/sandbox-exec")
            .arg("-p")
            .arg(&profile)
            .arg("--")
            .arg("swift")
            .arg("/tmp/zlayer-gpu-test/test.swift")
            .output()
            .unwrap();

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        println!("stdout: {}", stdout);
        println!("stderr: {}", stderr);
        println!("exit code: {:?}", output.status.code());

        assert!(output.status.success(), "Metal compute failed in sandbox");
        assert!(stdout.contains("PASS"), "Metal device init failed");
    }
}
```

## Performance Benchmarks

```rust
#[cfg(all(test, target_os = "macos"))]
mod benchmarks {
    use super::*;
    use std::time::Instant;

    /// Benchmark APFS clonefile performance
    #[tokio::test]
    #[ignore]
    async fn bench_apfs_clonefile() {
        let tmp = tempfile::tempdir().unwrap();

        // Create a realistic rootfs (~500MB)
        let src_dir = tmp.path().join("src");
        create_test_rootfs(&src_dir, 500).await; // 500MB

        let start = Instant::now();
        let dst_dir = tmp.path().join("dst");
        clone_directory_recursive(&src_dir, &dst_dir).await.unwrap();
        let elapsed = start.elapsed();

        println!("APFS clone 500MB rootfs: {:?}", elapsed);
        // Expected: < 100ms (metadata-only operation)
    }

    /// Benchmark sandbox_init overhead
    #[test]
    #[ignore]
    fn bench_sandbox_init_overhead() {
        // Measure the time to fork + sandbox_init + exec a simple command
        // compared to just fork + exec without sandboxing

        let iterations = 100;

        // Without sandbox
        let start = Instant::now();
        for _ in 0..iterations {
            let _ = std::process::Command::new("/bin/true").output().unwrap();
        }
        let unsandboxed = start.elapsed();

        // With sandbox
        let profile = "(version 1)(allow default)";
        let start = Instant::now();
        for _ in 0..iterations {
            let _ = std::process::Command::new("/usr/bin/sandbox-exec")
                .arg("-p")
                .arg(profile)
                .arg("--")
                .arg("/bin/true")
                .output()
                .unwrap();
        }
        let sandboxed = start.elapsed();

        println!(
            "Without sandbox: {:?} ({:?}/iter)",
            unsandboxed,
            unsandboxed / iterations
        );
        println!(
            "With sandbox: {:?} ({:?}/iter)",
            sandboxed,
            sandboxed / iterations
        );
        println!(
            "Overhead: {:?}/iter",
            (sandboxed - unsandboxed) / iterations
        );
        // Expected overhead: < 1ms per invocation
    }

    async fn create_test_rootfs(path: &Path, size_mb: u64) {
        tokio::fs::create_dir_all(path.join("bin")).await.unwrap();
        tokio::fs::create_dir_all(path.join("lib")).await.unwrap();
        tokio::fs::create_dir_all(path.join("usr/lib")).await.unwrap();

        // Create files totaling approximately size_mb
        let file_size = 1024 * 1024; // 1MB per file
        let num_files = size_mb;
        let data = vec![0u8; file_size as usize];

        for i in 0..num_files {
            let file_path = path.join("lib").join(format!("file_{}.dat", i));
            tokio::fs::write(&file_path, &data).await.unwrap();
        }
    }
}
```

---

# Part 7: File Manifest

## Files to Create

| File | Purpose |
|------|---------|
| `crates/zlayer-agent/src/runtimes/macos_sandbox.rs` | Seatbelt sandbox runtime (primary macOS runtime) |
| `crates/zlayer-agent/src/runtimes/macos_vm.rs` | libkrun VM runtime (backup macOS runtime) |

## Files to Modify

| File | Lines | Change |
|------|-------|--------|
| `crates/zlayer-agent/src/runtimes/mod.rs` | After line 107 | Add `#[cfg(target_os = "macos")] mod macos_sandbox;` and `#[cfg(target_os = "macos")] mod macos_vm;` modules and re-exports |
| `crates/zlayer-agent/src/lib.rs` | Lines 71-97 | Add `MacSandbox` and `MacVm` variants to `RuntimeConfig` enum |
| `crates/zlayer-agent/src/lib.rs` | Lines 198-218 | Add match arms for new RuntimeConfig variants in `create_runtime()` |
| `crates/zlayer-agent/src/lib.rs` | Lines 226-284 | Add macOS branch to `create_auto_runtime()` |
| `crates/zlayer-agent/src/lib.rs` | Lines 52-66 | Add `#[cfg(target_os = "macos")]` re-exports for `SandboxRuntime` and `VmRuntime` |
| `crates/zlayer-agent/src/gpu_detector.rs` | Entire file | Wrap existing `detect_gpus()` in `#[cfg(target_os = "linux")]`, add `#[cfg(target_os = "macos")]` version |
| `crates/zlayer-spec/src/types.rs` | Lines 540-557 | Add `mode: Option<String>` field to `GpuSpec`, update doc comments to include "apple" vendor |
| `bin/runtime/src/cli.rs` | Lines 51-61 | Add `MacSandbox` and `MacVm` variants to `RuntimeType` enum |
| `crates/zlayer-agent/Cargo.toml` | After line 85 | Add macOS-specific dependencies section |
| `crates/zlayer-scheduler/src/placement.rs` | `can_place_on_node()` | Add unified memory awareness for Apple Silicon |

## Dependencies to Add

### `crates/zlayer-agent/Cargo.toml`

```toml
# macOS-specific dependencies for Seatbelt sandbox and libkrun VM runtimes
[target.'cfg(target_os = "macos")'.dependencies]
# Dynamic library loading for libkrun
libloading = "0.8"
```

No additional crate dependencies are needed for the Seatbelt sandbox runtime -- it uses direct FFI to `libSystem.dylib` which is always available on macOS. The `libc` crate (already a workspace dependency) provides the POSIX types.

## Feature Flags

No new feature flags are required. The macOS runtimes are gated by `#[cfg(target_os = "macos")]` which is a compile-time platform check, not a cargo feature. This is the correct approach because:

1. The Seatbelt sandbox APIs only exist on macOS -- there is nothing to "opt into"
2. The `libloading` dependency for libkrun is lightweight and harmless on macOS
3. The code is dead-code-eliminated on non-macOS platforms

If desired, an optional feature flag could be added later:

```toml
[features]
macos-vm = ["dep:libloading"]  # Only needed if libkrun VM support is wanted
```

## Cargo.toml Module Registration

In `crates/zlayer-agent/src/runtimes/mod.rs`, add:

```rust
// macOS Seatbelt sandbox runtime for native Metal/MPS GPU access
#[cfg(target_os = "macos")]
mod macos_sandbox;

// macOS libkrun VM runtime for Linux container compat with GPU forwarding
#[cfg(target_os = "macos")]
mod macos_vm;

#[cfg(target_os = "macos")]
pub use macos_sandbox::SandboxRuntime;

#[cfg(target_os = "macos")]
pub use macos_vm::VmRuntime;
```

---

# Appendix A: Quick Reference -- Seatbelt Operations

| Operation | What It Controls |
|-----------|-----------------|
| `file-read*` | All file read operations |
| `file-write*` | All file write operations |
| `file-map-executable` | Memory-mapping files as executable (dyld) |
| `process-exec` | Executing new binaries |
| `process-fork` | Forking child processes |
| `signal` | Sending signals to processes |
| `network-outbound` | Outbound network connections |
| `network-inbound` | Inbound network connections |
| `network-bind` | Binding to local ports |
| `mach-lookup` | Looking up Mach service ports |
| `iokit-open` | Opening IOKit user clients (GPU drivers) |
| `iokit-get-properties` | Reading IOKit registry properties |
| `sysctl-read` | Reading kernel sysctl values |
| `ipc-posix-shm` | POSIX shared memory |
| `ipc-posix-sem` | POSIX semaphores |

# Appendix B: IOKit User Client Classes for Apple GPU

| Class | Purpose | Required For |
|-------|---------|-------------|
| `IOSurfaceRoot` | GPU memory surface management | Always |
| `IOSurfaceRootUserClient` | GPU memory surface client | Always |
| `IOAccelDevice2` | GPU device access | Metal |
| `IOAccelContext2` | GPU context/command submission | Metal |
| `IOAccelSharedUserClient2` | GPU shared resources | Metal |
| `IOAccelSubmitter2` | GPU command submission | Metal |
| `AGXAccelerator` | Apple Silicon GPU driver | Apple Silicon |
| `IOAccelerator` | Generic GPU accelerator | Intel Macs |
| `IntelAccelerator` | Intel-specific GPU | Intel Macs |
| `RootDomainUserClient` | Power management | Common |

# Appendix C: Mach Services for Metal/MPS

| Service Name | Purpose | Required For |
|-------------|---------|-------------|
| `com.apple.MTLCompilerService` | Runtime shader compilation | Metal compute |
| `com.apple.AGXCompilerService.*` | Apple Silicon shader compilation | Metal on Apple Silicon |
| `com.apple.gpu.process` | GPU process management | Modern macOS |
| `com.apple.CARenderServer` | Core Animation rendering | Display output |
| `com.apple.PowerManagement.control` | Power management | GPU power states |
| `com.apple.system.opendirectoryd.libinfo` | Directory/DNS lookups | Any process |

# Appendix D: Performance Comparison Matrix

| Metric | Seatbelt Sandbox | libkrun VM | Docker Desktop | Bare Metal |
|--------|-----------------|------------|---------------|------------|
| GPU Performance | 100% | 25-98%* | 0% (no GPU) | 100% |
| Boot Time | < 10ms | 100-300ms | 5-15s | N/A |
| Memory Overhead | ~0 | ~100MB | ~2GB | 0 |
| Network Throughput | 100% | 55-60% | 70-80% | 100% |
| Filesystem Isolation | Seatbelt MAC | VM boundary | VM + namespace | None |
| Network Isolation | Seatbelt rules | VM boundary | VM + namespace | None |
| Linux Compat | No (macOS only) | Yes | Yes | N/A |
| Multi-tenant GPU | Yes | No (1 VM) | No (no GPU) | Yes |

*libkrun GPU: 25-30% via Venus-Vulkan, 95-98% via ggml API remoting
