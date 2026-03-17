# GPU Roadmap

## Implemented

### Spec & Device Passthrough

- **`GpuSpec`** in `crates/zlayer-spec/src/types.rs` -- `resources.gpu` field with `count` (default 1) and `vendor` (default `"nvidia"`)
- **Multi-vendor device injection in libcontainer** (`crates/zlayer-agent/src/bundle.rs` -- `build_devices` and `build_device_cgroup_rules`):
  - `nvidia`: injects `/dev/nvidiactl`, `/dev/nvidia-uvm`, `/dev/nvidia-uvm-tools`, plus per-GPU `/dev/nvidia{N}` up to `gpu.count`; cgroup rules for majors 195 and 510
  - `amd`: injects `/dev/kfd`, `/dev/dri/renderD{128+N}`, `/dev/dri/card{N}`; cgroup rules for majors 226 (DRI) and 234 (KFD)
  - `intel`: injects `/dev/dri/renderD{128+N}`, `/dev/dri/card{N}`; cgroup rule for major 226
  - Unknown vendors: falls back to `/dev/dri/renderD*` passthrough with a warning log
- **Multi-vendor device injection in Docker runtime** (`crates/zlayer-agent/src/runtimes/docker.rs` -- `build_host_config`):
  - `nvidia`: uses NVIDIA Container Toolkit via Docker `device_requests` with `capabilities: [["gpu"]]`
  - `amd`/`intel`/unknown: raw `DeviceMapping` passthrough of DRI and vendor-specific device nodes
- Basic `spec.devices` passthrough still works for arbitrary `/dev/*` paths

### GPU Auto-Detection

- **`gpu_detector` module** (`crates/zlayer-agent/src/gpu_detector.rs`):
  - Scans `/sys/bus/pci/devices/*/class` for VGA controllers (`0x0300xx`) and 3D controllers (`0x0302xx`)
  - Identifies vendor from PCI vendor ID: `0x10de` = NVIDIA, `0x1002` = AMD, `0x8086` = Intel
  - Reads model name from DRM product name sysfs, falls back to `nvidia-smi --query-gpu=name` for NVIDIA
  - Reads VRAM from AMD `mem_info_vram_total` sysfs, `nvidia-smi --query-gpu=memory.total` for NVIDIA, or PCI BAR regions as fallback
  - Resolves device paths (`/dev/nvidia{N}` for NVIDIA, `/dev/dri/card{N}` + `/dev/dri/renderD{128+N}` for AMD/Intel)
  - Returns `Vec<GpuInfo>` with `pci_bus_id`, `vendor`, `model`, `memory_mb`, `device_path`, `render_path`
- **Node startup detection** (`bin/runtime/src/commands/node.rs`):
  - Both `node init` and `node join` call `zlayer_agent::detect_gpus()` and print detected GPU inventory
  - GPU info logged with tracing (PCI bus ID, vendor, model, VRAM, device path)

### Scheduler GPU Support

- **`NodeResources`** in `crates/zlayer-scheduler/src/placement.rs` includes:
  - `gpu_total: u32`, `gpu_used: u32`, `gpu_models: Vec<String>`, `gpu_memory_mb: u64`, `gpu_vendor: String`
  - `gpu_available()` helper method
- **`GpuInfoSummary`** in `crates/zlayer-scheduler/src/raft.rs` -- lightweight struct (`vendor`, `model`, `memory_mb`) stored in Raft cluster state
- **`NodeInfo`** in `crates/zlayer-scheduler/src/raft.rs` has `gpus: Vec<GpuInfoSummary>` field, persisted via Raft so all nodes see fleet GPU inventory
- **GPU-aware placement** in `crates/zlayer-scheduler/src/placement.rs`:
  - `can_place_on_node()` rejects nodes with insufficient `gpu_available()` for the requested `resources.gpu.count`
  - `can_place_on_node()` rejects nodes with GPU vendor mismatch
  - `select_for_bin_packing()` uses 70/30 weighted scoring (CPU/memory utilization + GPU availability ratio) for GPU workloads
  - `place_service_replicas()` increments `gpu_used` on nodes as containers are placed, preventing over-allocation within a batch
  - Non-GPU workloads are completely unaffected

## Future Work

### Scheduling Enhancements (incremental)

- GPU model affinity: pin a service to nodes with a specific GPU model (e.g. `gpu_model: "A100"`)
- GPU anti-affinity: spread GPU workloads across nodes to avoid single-node bottlenecks
- Automatic runtime environment variables: `NVIDIA_VISIBLE_DEVICES`, `ROCR_VISIBLE_DEVICES`, `ZE_AFFINITY_MASK` based on vendor and allocated device indices
- Periodic GPU inventory refresh to catch hot-plugged devices (currently detection is at startup only)
- NVML-based GPU utilization metrics fed into adaptive scaling decisions

### Phase 1: CDI Integration

- Adopt the [Container Device Interface (CDI)](https://github.com/cncf-tags/container-device-interface) spec for standardized device injection
- Generate CDI specs from detected NVIDIA devices using `nvidia-ctk cdi generate` or equivalent logic in `crates/zlayer-agent/`
- Discover CDI spec files from `/etc/cdi/` and `/var/run/cdi/` at container creation time
- Apply CDI device annotations to the OCI runtime spec in `crates/zlayer-agent/` before handing off to libcontainer
- Vendor-agnostic: any CDI-compliant device plugin works without ZLayer-specific code

### Phase 2: Distributed GPU Workloads

- First-class orchestration patterns for multi-node GPU jobs:
  - Distributed inference: coordinate vLLM tensor-parallel across nodes via overlay network in `crates/zlayer-overlay/`
  - Federated inference: Petals-style model sharding with automatic peer discovery
- GPU topology awareness: read NVLink/NVSwitch topology from `nvidia-smi topo -m`, prefer co-located GPUs for intra-node parallelism
- Multi-node GPU job scheduling: gang scheduling in `crates/zlayer-scheduler/` -- all-or-nothing placement for jobs that span multiple nodes
- GPU memory overcommit policies: allow best-effort GPU sharing with configurable limits (MPS or time-slicing for NVIDIA)
- Model sharding metadata in `crates/zlayer-spec/`: define how model layers map to GPU devices across the cluster

### Phase 3: Observability

- Per-GPU metrics via NVML (or vendor equivalent): utilization %, memory used/total, temperature, power draw, ECC error counts
- Expose metrics through `crates/zlayer-observability/` OpenTelemetry pipeline with GPU-specific metric names (`zlayer.gpu.utilization`, `zlayer.gpu.memory.used`, etc.)
- GPU health monitoring: detect fallen-off GPUs (Xid errors), thermal throttling, ECC uncorrectable errors -- emit alerts via existing observability channels
- GPU usage attribution: tag GPU metrics with service name and container ID so operators can see which workloads consume GPU resources
- Dashboard-ready: export Prometheus-compatible GPU metrics for Grafana integration

### Phase 4: Apple Silicon / MPS Support

Docker on macOS **cannot** access the Apple GPU at all — it runs a Linux VM via Virtualization.framework, which only exposes Virtio GPU 2D to Linux guests. No Metal, no MPS, no GPU compute. ZLayer can do significantly better because it controls the runtime layer.

**Why libcontainer/youki won't work on macOS:** youki depends on Linux-only kernel primitives (namespaces via `clone()`, cgroups, seccomp). macOS's XNU kernel implements none of these. There is no port, no compat layer, and no path to making youki run natively on macOS.

**How MPS works at the system level:** Unlike Linux GPUs (`/dev/nvidia*`, `/dev/dri/*`), Apple GPUs are accessed entirely through IOKit and Metal.framework — no device files in `/dev/`. The chain is `Metal.framework → IOKit → IOServiceOpen() → kernel GPU driver → hardware`. MPS *can* work from sandboxed processes if the sandbox profile allows IOKit GPU access.

#### Phase 4a: macOS Platform Detection

- Extend `gpu_detector.rs` with a macOS branch using `system_profiler SPDisplaysDataType -json` or IOKit (`IOServiceMatching("IOAccelerator")`) to detect Apple GPU
- Add `vendor: "apple"` to `GpuSpec` with model detection (M1, M2, M3, M4, M1 Pro/Max/Ultra, etc.)
- Report unified memory size (CPU and GPU share address space on Apple Silicon — VRAM = system RAM)
- Detect Neural Engine availability for ANE-compatible workloads
- Relevant: `core-foundation` and `io-kit-sys` Rust crates for IOKit access

#### Phase 4b: macOS Seatbelt Sandbox Isolation (Native MPS — 100% GPU Performance)

Instead of Linux containers, implement macOS-native process isolation using Seatbelt (sandbox-exec):

- Generate custom `.sb` Seatbelt profiles per service that:
  - Allow IOKit GPU access (Metal/MPS) and Mach service communication
  - Restrict filesystem access to the workload directory only
  - Restrict network access per the ZLayer spec
  - Deny process creation, signal sending, and other capabilities not needed
- Run workload processes under `sandbox-exec -f <profile>` with chroot-style filesystem isolation
- **Result:** 100% native Metal/MPS performance with meaningful process isolation
- This is the killer feature: something Docker literally cannot provide on macOS
- Relevant Rust crates: `sandbox-runtime` (cross-platform Seatbelt sandboxing from Rust)
- Spec integration: `resources.gpu.vendor: "apple"` + `resources.gpu.mode: "native"` selects this path
- Supports PyTorch MPS (`device="mps"`), TensorFlow Metal, Apple MLX, Core ML, and any Metal-based workload

#### Phase 4c: libkrun VM Backend (Linux Containers with GPU — 75-98% Performance)

For workloads that require actual Linux containers on macOS, integrate **libkrun** (Rust-based VMM, Apache 2.0):

- libkrun uses Hypervisor.framework directly for lightweight per-container VMs
- GPU acceleration via virtio-gpu + Venus protocol + virglrenderer + MoltenVK → Metal pipeline
- Achieves 75-80% of native Metal performance for general GPU workloads
- Newer API remoting approach (ggml operations forwarded directly to Metal) achieves 95-98% for inference
- Full OCI image compatibility (runs real Linux containers)
- Transparent Socket Impersonation (TSI) for networking without full virtual NIC — maps cleanly to ZLayer overlay
- Implement `MacOsVmRuntime` in `crates/zlayer-agent/` alongside existing `LibcontainerRuntime` and `DockerRuntime`
- Relevant: `libkrun` (github.com/containers/libkrun), `ahv` / `applevisor` crates for Hypervisor.framework bindings

#### Phase 4d: Hybrid Mode and Scheduler Integration

- **Automatic mode selection:** GPU workloads with `vendor: "apple"` default to native sandbox mode (4b) for maximum performance; workloads requiring Linux binaries use libkrun VM mode (4c)
- **Spec syntax:**
  ```yaml
  resources:
    gpu:
      count: 1
      vendor: "apple"
      mode: "native"  # or "vm" for Linux container compat
  ```
- **Cross-platform scheduling:** extend `place_service_replicas()` to route `vendor: "apple"` workloads to Mac nodes and `vendor: "nvidia"` / `vendor: "amd"` to Linux nodes in mixed clusters
- **Unified memory awareness:** Apple Silicon GPU VRAM is system RAM — scheduler should account for this when placing workloads (a 32GB M2 Max has 32GB shared between CPU and GPU, not separate pools)

#### Key Rust Crates for macOS GPU Support

| Crate | Purpose | License |
|-------|---------|---------|
| `sandbox-runtime` | macOS Seatbelt sandboxing from Rust | - |
| `libkrun` | Rust VMM with GPU virtualization via Hypervisor.framework | Apache 2.0 |
| `ahv` | Hypervisor.framework bindings (most popular) | - |
| `applevisor` | Hypervisor.framework bindings (Apple Silicon) | - |
| `virtualization-rs` | Virtualization.framework bindings | - |
| `core-foundation` | Core Foundation type bindings (for IOKit) | MIT/Apache 2.0 |
| `io-kit-sys` | Raw IOKit FFI bindings | MIT/Apache 2.0 |

#### What This Means Competitively

| Platform | Apple GPU Access | Performance | Isolation |
|----------|-----------------|-------------|-----------|
| Docker Desktop | None (VM blocks GPU) | N/A | Linux namespaces in VM |
| Podman + libkrun | Vulkan forwarding | 75-98% native | VM-level |
| ZLayer native sandbox | Direct Metal/MPS | **100% native** | Seatbelt MAC |
| ZLayer + libkrun | Vulkan forwarding | 75-98% native | VM-level |

ZLayer would be the **only container platform offering 100% native Metal/MPS performance** via the Seatbelt sandbox approach.
