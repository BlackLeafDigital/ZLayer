# GPU Roadmap

Scope: work that is not yet implemented. Existing GPU support (multi-vendor
device injection, CDI, Apple Silicon, GPU metrics, gang scheduling, GPU-aware
placement) is in-tree and covered by its own code comments.

## Topology-aware scheduling

- Read NVLink / NVSwitch topology from `nvidia-smi topo -m` during agent
  startup and store it alongside the existing `GpuInfoSummary` in
  `crates/zlayer-scheduler/src/raft.rs`.
- Extend bin-packing in `crates/zlayer-scheduler/src/placement.rs` to prefer
  co-located / NVLink-connected GPUs for multi-GPU replicas on the same node.
- Expose a simple spec knob: `resources.gpu.topology: "nvlink"` (hard) vs
  `"prefer-nvlink"` (soft). Default keeps today's behaviour.

## Distributed-inference orchestration

Today each service replica is independent. Multi-node GPU workloads are
possible (gang scheduling exists), but the scheduler has no first-class
understanding of how layers or heads map across replicas.

- **Model sharding metadata** in `crates/zlayer-spec/src/types.rs`: describe
  how a model splits across GPUs (tensor-parallel degree, pipeline-parallel
  degree, per-replica rank).
- **Coordinated placement** in the scheduler: place tensor-parallel groups on
  nodes connected by low-latency networking (NVLink intra-node, overlay-local
  inter-node) and avoid splitting a group across slow paths.
- **Overlay fast-path** in `crates/zlayer-overlay/`: expose an "rdma-ish" peer
  hint that inference runtimes (vLLM, SGLang) can use for collective ops.
- **Federated / peer-to-peer inference** (Petals-style): a spec mode where
  replicas announce themselves to a rendezvous and stitch together a model
  without explicit placement — lower coordination cost for dynamic fleets.

## GPU memory sharing policies

- Configurable MPS (CUDA) / time-slicing caps in `crates/zlayer-agent/src/gpu_sharing.rs`:
  per-service memory fraction, compute percentage, and priority hints.
- Surface the caps on `GpuSpec` so deployment authors don't need to reach into
  the agent config.

## Why these are separate from the implemented items

CDI integration, Apple Silicon runtimes, per-GPU metrics, and vendor env-var
injection (`NVIDIA_VISIBLE_DEVICES`, `ROCR_VISIBLE_DEVICES`, `ZE_AFFINITY_MASK`)
are all live. See `crates/zlayer-agent/src/cdi.rs`, the `runtimes/macos_*.rs`
modules, `crates/zlayer-agent/src/gpu_metrics.rs`, and
`crates/zlayer-agent/src/bundle.rs`/`runtimes/docker.rs` for the code.
