# macOS Runtime: Seatbelt Sandbox + libkrun VM

ZLayer ships two macOS-native container backends alongside the Docker fallback.
On macOS the agent picks one automatically (`SandboxRuntime` → `VmRuntime` →
Docker) and the scheduler routes Apple-Silicon-labelled workloads to Mac nodes.

- Seatbelt sandbox: `crates/zlayer-agent/src/runtimes/macos_sandbox.rs`
- libkrun VM: `crates/zlayer-agent/src/runtimes/macos_vm.rs`
- Apple GPU detection: `crates/zlayer-agent/src/gpu_detector.rs`
- Runtime selection: `crates/zlayer-agent/src/lib.rs` (auto-priority)

Both runtimes implement the common `Runtime` trait (`crates/zlayer-agent/src/runtime.rs`),
so the rest of the agent (scheduler, service manager, observability, overlay) treats
them like any other backend.

## Seatbelt sandbox runtime (`SandboxRuntime`)

Process-level isolation for macOS-native binaries with direct Metal / MPS
access.

- Process model: `fork()` + `sandbox_init()` + `execve()`. No VM, no Docker,
  no Linux kernel.
- Isolation: a generated Seatbelt `.sb` profile applied via
  `sandbox_init(3)` (deny-default allow-list for filesystem, IOKit, network,
  IPC, Mach services, and framework paths).
- Filesystem: container rootfs is an APFS `clonefile()` copy of the pulled OCI
  image rootfs. Copy-on-write — near-instant, near-zero disk.
- GPU: three levels on `SandboxConfig::gpu_access`
  - `None` — IOKit and GPU Mach services denied.
  - `MetalCompute` — full Metal pipeline (shader compilation + IOKit GPU user
    clients). Required for JIT workloads like PyTorch MPS.
  - `MpsOnly` — pre-compiled MPS kernels, smaller attack surface; suitable for
    inference-only flows that don't need `MTLCompilerService`.
- Network: `NetworkAccess` enum — `None`, `LocalhostOnly { bind_ports,
  connect_ports }`, or `Full`.
- Performance: 100% native Metal / MPS. No virtualisation layer between the
  workload and the GPU.
- Limitation: runs macOS-native binaries only. No Linux ELF support — use
  `VmRuntime` for that.

Directory layout:

```
{data_dir}/
  images/{image}/rootfs/
  containers/{service}-{replica}/
    rootfs/          APFS clone
    config.json      serialized ServiceSpec
    sandbox.sb       generated Seatbelt profile
    stdout.log, stderr.log, pid, tmp/
```

Typical use: PyTorch MPS, MLX, TensorFlow Metal, Core ML, Whisper MLX, local
LLM inference servers.

## libkrun VM runtime (`VmRuntime`)

Full Linux OCI container compatibility via libkrun microVMs on
Hypervisor.framework, with GPU forwarding on Apple Silicon.

- Process model: one libkrun microVM per container. libkrun is loaded
  dynamically via `dlopen` from `libkrun.dylib`, then `/usr/local/lib/libkrun.dylib`
  (Intel Homebrew), then `/opt/homebrew/lib/libkrun.dylib` (Apple Silicon
  Homebrew).
- Kernel: libkrun ships a minimal Linux kernel — no host kernel involvement
  beyond Hypervisor.framework.
- Rootfs: plain copy of the extracted OCI rootfs (VMs don't need APFS clones).
- GPU forwarding (Apple Silicon only):
  - Venus-Vulkan protocol — ~30% of native Metal performance, works with any
    Vulkan workload.
  - API remoting (ggml / CUDA-like) — ~97% of native for inference workloads
    that use a forwarded op path.
- Networking: TSI (Transparent Socket Impersonation) — guest sockets flow
  through the host process's socket stack without a full virtual NIC.
- Boot: 100–300 ms, ~100 MB RAM overhead per VM.

Limitations:
- One GPU-attached VM at a time (Metal context cannot be shared across VMs).
- `exec` into a running VM is not implemented (would need vsock or an agent
  inside the VM).
- `vcpus` must be `≤` host cores or libkrun silently hangs.
- No graceful shutdown API — VM exits when the entrypoint process exits.
- Intel Macs can run Linux VMs but cannot forward the GPU.

Directory layout:

```
{data_dir}/
  images/{image}/rootfs/
  vms/{service}-{replica}/
    rootfs/
    config.json
    console.log
```

Typical use: a Linux container image that needs GPU but whose binary isn't
available natively on macOS.

## Apple Silicon GPU detection

`crates/zlayer-agent/src/gpu_detector.rs` runs `system_profiler
SPDisplaysDataType -json` on macOS, extracts the chip name (M1/M2/M3/M4 +
Pro/Max/Ultra), and reports unified-memory size as VRAM. `pci_bus_id` is
synthesised as `apple:{idx}` since Apple GPUs aren't exposed through `/dev/`
or sysfs. Results flow into the same `GpuInfo` pipeline as Linux GPUs and are
persisted into Raft cluster state via `GpuInfoSummary`.

Deployment authors can target Apple nodes with:

```yaml
resources:
  gpu:
    count: 1
    vendor: "apple"
```

The scheduler's `can_place_on_node` already rejects non-Apple nodes for this
spec.

## Automatic runtime selection

On macOS the agent's default factory returns a `CompositeRuntime`:

1. `SandboxRuntime` — used whenever the workload is marked macOS-native.
2. `VmRuntime` — delegate for Linux images (used by
   `CompositeRuntime::with_delegate` when available).
3. Docker — final fallback if neither of the above is viable.

That order is hardcoded; operators can override by constructing a specific
`RuntimeConfig` variant (`RuntimeConfig::MacSandbox` or `RuntimeConfig::MacVm`).
