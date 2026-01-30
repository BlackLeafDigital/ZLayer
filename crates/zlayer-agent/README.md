# zlayer-agent

Container runtime agent for ZLayer, providing lifecycle management for containers, WebAssembly modules, and jobs.

## Features

The agent supports multiple container runtimes through feature flags:

- **youki** (default): Linux-native container runtime using libcontainer. Preferred on Linux for performance (no daemon overhead).
- **docker**: Cross-platform runtime using Docker daemon. Works on Linux, Windows, and macOS.
- **wasm**: WebAssembly runtime using wasmtime for executing WASM workloads with WASI support.

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
zlayer-agent = { path = "../zlayer-agent" }

# Or with specific features
zlayer-agent = { path = "../zlayer-agent", features = ["docker", "wasm"] }
```

## Runtime Selection

### Automatic Selection

Use `RuntimeConfig::Auto` to automatically select the best available runtime:

```rust
use zlayer_agent::{RuntimeConfig, create_runtime};

let runtime = create_runtime(RuntimeConfig::Auto).await?;
```

Selection logic:
- **Linux**: Prefers youki if available, falls back to Docker
- **Windows/macOS**: Uses Docker directly (youki is Linux-only)
- **WASM artifacts**: Use `RuntimeConfig::Wasm` explicitly

### Explicit Selection

```rust
use zlayer_agent::{RuntimeConfig, YoukiConfig, create_runtime};

// Use youki runtime
let youki = create_runtime(RuntimeConfig::Youki(YoukiConfig::default())).await?;

// Use Docker runtime
#[cfg(feature = "docker")]
let docker = create_runtime(RuntimeConfig::Docker).await?;

// Use WASM runtime
#[cfg(feature = "wasm")]
{
    use zlayer_agent::WasmConfig;
    let wasm = create_runtime(RuntimeConfig::Wasm(WasmConfig::default())).await?;
}
```

## WebAssembly Runtime Support

The `wasm` feature enables running WebAssembly modules with WASI support as an alternative to traditional containers.

### Enabling WASM Support

```toml
[dependencies]
zlayer-agent = { path = "../zlayer-agent", features = ["wasm"] }
```

### WASM Runtime Configuration

```rust
use zlayer_agent::{WasmConfig, WasmRuntime};
use std::path::PathBuf;
use std::time::Duration;

let config = WasmConfig {
    // Directory for caching WASM modules
    cache_dir: PathBuf::from("/var/lib/zlayer/wasm"),

    // Enable epoch-based interruption for timeouts
    enable_epochs: true,

    // Instructions before yield check (for cooperative scheduling)
    epoch_deadline: 1_000_000,

    // Maximum execution time for WASM modules
    max_execution_time: Duration::from_secs(3600),
};

let runtime = WasmRuntime::new(config).await?;
```

### WASI Support

The WASM runtime supports WASI Preview 1 (wasip1) with the following capabilities:

- Environment variables
- Command-line arguments
- Stdout/stderr output
- Exit codes via `proc_exit`

### WASM vs Container Runtime Comparison

| Feature | Container (youki/Docker) | WASM |
|---------|-------------------------|------|
| Isolation | Process/cgroup | In-process sandbox |
| Startup time | 100ms+ | <10ms |
| Memory overhead | Higher (full rootfs) | Lower (just module) |
| `exec` support | Yes | No |
| Cgroup stats | Yes | No (stub values) |
| Filesystem mounts | Yes | Limited |
| Network | Full | Limited |
| Platform | Linux/Windows/macOS | Cross-platform |

### WASM Limitations

- **No `exec` support**: WASM modules are single-process and don't support executing additional commands
- **No cgroup stats**: WASM runs in-process without kernel isolation, so resource stats return stub values
- **Limited filesystem**: Currently only environment variables are passed; filesystem mounts are not yet supported
- **Single entry point**: WASM modules must export `_start` or `main` function

### Example: Running a WASM Workload

```rust
use zlayer_agent::{ContainerId, Runtime, WasmConfig, WasmRuntime};
use zlayer_spec::ServiceSpec;
use std::time::Duration;

async fn run_wasm_module(spec: &ServiceSpec) -> Result<i32, Box<dyn std::error::Error>> {
    // Create WASM runtime
    let runtime = WasmRuntime::new(WasmConfig::default()).await?;

    let id = ContainerId {
        service: "my-wasm-service".to_string(),
        replica: 1,
    };

    // Pull WASM artifact from registry
    runtime.pull_image(&spec.image.name).await?;

    // Create instance
    runtime.create_container(&id, spec).await?;

    // Start execution
    runtime.start_container(&id).await?;

    // Wait for completion
    let exit_code = runtime.wait_container(&id).await?;

    // Cleanup
    runtime.remove_container(&id).await?;

    Ok(exit_code)
}
```

## Service Manager

The `ServiceManager` provides higher-level orchestration for services:

```rust
use zlayer_agent::{ServiceManager, RuntimeConfig};
use zlayer_spec::DeploymentSpec;

// Load deployment spec
let spec: DeploymentSpec = serde_yaml::from_str(yaml)?;

// Create service manager with auto-selected runtime
let manager = ServiceManager::new(RuntimeConfig::Auto).await?;

// Deploy services
for (name, service_spec) in &spec.services {
    manager.deploy_service(name, service_spec).await?;
}
```

## Testing

### Running All Tests

```bash
# Unit tests
cargo test -p zlayer-agent

# With Docker runtime tests
cargo test -p zlayer-agent --features docker

# With WASM runtime tests
cargo test -p zlayer-agent --features wasm

# All features
cargo test -p zlayer-agent --all-features
```

### WASM-Specific Tests

```bash
cargo test -p zlayer-agent --features wasm -- --nocapture
```

The WASM tests use inline WAT (WebAssembly Text Format) modules compiled at test time, so no external WASM binaries are required.

## Module Structure

- `runtime.rs`: Abstract `Runtime` trait and `ContainerState` types
- `runtimes/`: Runtime implementations
  - `youki.rs`: Linux-native runtime using libcontainer
  - `docker.rs`: Docker daemon runtime (feature: `docker`)
  - `wasm.rs`: WebAssembly runtime using wasmtime (feature: `wasm`)
- `service.rs`: `ServiceManager` for higher-level orchestration
- `container_supervisor.rs`: Container health monitoring and restart logic
- `job.rs`: Job execution support for run-to-completion workloads
- `cron_scheduler.rs`: Cron-based job scheduling
- `health.rs`: Health check implementations
- `init.rs`: Init action orchestration
- `env.rs`: Environment variable resolution
- `bundle.rs`: OCI bundle creation for youki

## Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `ZLAYER_WASM_CACHE_DIR` | WASM module cache directory | `/var/lib/zlayer/wasm` |
| `ZLAYER_RUNTIME_DIR` | Runtime state directory | `/var/run/zlayer` |
| `ZLAYER_ROOT_DIR` | Root directory for bundles | `/var/lib/zlayer` |

## License

Apache-2.0
