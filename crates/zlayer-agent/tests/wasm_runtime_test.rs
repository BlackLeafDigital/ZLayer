//! WebAssembly runtime integration tests
//!
//! These tests verify the WasmRuntime implementation using wasmtime.
//! Tests are gated behind the `wasm` feature and use inline WASM modules.
//!
//! # Requirements
//! - The `wasm` feature must be enabled
//! - The `wat` dev-dependency provides text-to-binary WASM compilation
//!
//! # Running
//! ```bash
//! cargo test -p zlayer-agent --features wasm -- --nocapture
//! ```

#![cfg(feature = "wasm")]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use zlayer_agent::runtimes::{WasmConfig, WasmRuntime};
use zlayer_agent::{ContainerId, ContainerState, Runtime};
use zlayer_spec::{
    CommandSpec, ErrorsSpec, HealthCheck, HealthSpec, ImageSpec, InitSpec, NetworkSpec, NodeMode,
    PullPolicy, ResourceType, ResourcesSpec, ScaleSpec, ServiceSpec,
};

// =============================================================================
// Test WASM Modules (WAT - WebAssembly Text Format)
// =============================================================================

/// Minimal WASM module that exports _start and immediately exits
const MINIMAL_WASM: &str = r#"
(module
    ;; Import WASI proc_exit
    (import "wasi_snapshot_preview1" "proc_exit" (func $proc_exit (param i32)))

    ;; Memory required by WASI
    (memory (export "memory") 1)

    ;; _start function - entry point for WASI command
    (func (export "_start")
        ;; Exit with code 0
        i32.const 0
        call $proc_exit
    )
)
"#;

/// WASM module that exits with a specific exit code (42)
const EXIT_CODE_WASM: &str = r#"
(module
    ;; Import WASI proc_exit
    (import "wasi_snapshot_preview1" "proc_exit" (func $proc_exit (param i32)))

    ;; Memory required by WASI
    (memory (export "memory") 1)

    ;; _start function - exit with code 42
    (func (export "_start")
        i32.const 42
        call $proc_exit
    )
)
"#;

/// WASM module that writes to stdout using fd_write
const HELLO_WORLD_WASM: &str = r#"
(module
    ;; Import WASI functions
    (import "wasi_snapshot_preview1" "fd_write" (func $fd_write (param i32 i32 i32 i32) (result i32)))
    (import "wasi_snapshot_preview1" "proc_exit" (func $proc_exit (param i32)))

    ;; Memory layout:
    ;; 0-13: "Hello, WASM!\n" (14 bytes)
    ;; 16-23: iovec struct (8 bytes: ptr u32 + len u32)
    ;; 24-27: nwritten return value
    (memory (export "memory") 1)

    ;; Initialize data - "Hello, WASM!\n"
    (data (i32.const 0) "Hello, WASM!\n")

    (func (export "_start")
        ;; Set up iovec: pointer to string at offset 16
        (i32.store (i32.const 16) (i32.const 0))    ;; iovec.buf = 0 (pointer to string)
        (i32.store (i32.const 20) (i32.const 13))   ;; iovec.buf_len = 13 (length)

        ;; Call fd_write(stdout=1, iovs=16, iovs_len=1, nwritten=24)
        (call $fd_write
            (i32.const 1)    ;; fd = stdout
            (i32.const 16)   ;; iovs pointer
            (i32.const 1)    ;; iovs count
            (i32.const 24)   ;; nwritten pointer
        )
        drop ;; Ignore result

        ;; Exit successfully
        (i32.const 0)
        call $proc_exit
    )
)
"#;

/// WASM module that performs a simple computation (sum 1 to 100)
const COMPUTE_WASM: &str = r#"
(module
    ;; Import WASI proc_exit
    (import "wasi_snapshot_preview1" "proc_exit" (func $proc_exit (param i32)))

    ;; Memory required by WASI
    (memory (export "memory") 1)

    ;; Global to store the result
    (global $sum (mut i32) (i32.const 0))
    (global $i (mut i32) (i32.const 0))

    ;; _start function - compute sum of 1 to 100
    (func (export "_start")
        ;; Initialize
        (global.set $sum (i32.const 0))
        (global.set $i (i32.const 1))

        ;; Loop: sum += i; i++; while i <= 100
        (block $break
            (loop $continue
                ;; sum += i
                (global.set $sum
                    (i32.add (global.get $sum) (global.get $i))
                )
                ;; i++
                (global.set $i
                    (i32.add (global.get $i) (i32.const 1))
                )
                ;; Break if i > 100
                (br_if $break
                    (i32.gt_s (global.get $i) (i32.const 100))
                )
                (br $continue)
            )
        )

        ;; Exit with 0 (sum should be 5050)
        (i32.const 0)
        call $proc_exit
    )
)
"#;

/// WASM module with no _start or main function (should fail)
const NO_ENTRY_WASM: &str = r#"
(module
    (memory (export "memory") 1)

    ;; No _start or main function - this should fail to run
    (func $helper (result i32)
        i32.const 42
    )
)
"#;

// =============================================================================
// Helper Functions
// =============================================================================

/// Compile WAT (WebAssembly Text) to WASM binary
fn compile_wat(wat: &str) -> Vec<u8> {
    wat::parse_str(wat).expect("Failed to parse WAT")
}

/// Create a temporary cache directory for testing
fn create_test_cache_dir() -> TempDir {
    tempfile::tempdir().expect("Failed to create temp directory")
}

/// Create a WasmConfig for testing
fn create_test_config(cache_dir: &TempDir) -> WasmConfig {
    WasmConfig {
        cache_dir: cache_dir.path().to_path_buf(),
        enable_epochs: true,
        epoch_deadline: 100_000,
        max_execution_time: Duration::from_secs(30),
        cache_type: None,
    }
}

/// Generate a unique instance name
fn unique_instance_name(prefix: &str) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
        % 1_000_000;
    format!("test-{}-{}", prefix, timestamp)
}

/// Create a ContainerId with a unique service name
fn unique_container_id(prefix: &str) -> ContainerId {
    ContainerId {
        service: unique_instance_name(prefix),
        replica: 1,
    }
}

/// Create a minimal ServiceSpec for WASM testing
fn create_wasm_spec(image: &str) -> ServiceSpec {
    ServiceSpec {
        rtype: ResourceType::Service,
        schedule: None,
        image: ImageSpec {
            name: image.to_string(),
            pull_policy: PullPolicy::Never, // We're using local WASM files
        },
        resources: ResourcesSpec::default(),
        env: HashMap::new(),
        command: CommandSpec::default(),
        network: NetworkSpec::default(),
        endpoints: vec![],
        scale: ScaleSpec::default(),
        depends: vec![],
        health: HealthSpec {
            start_grace: None,
            interval: None,
            timeout: None,
            retries: 3,
            check: HealthCheck::Tcp { port: 0 },
        },
        init: InitSpec::default(),
        errors: ErrorsSpec::default(),
        devices: vec![],
        storage: vec![],
        capabilities: vec![],
        privileged: false,
        node_mode: NodeMode::default(),
        node_selector: None,
        service_type: zlayer_spec::ServiceType::Standard,
        wasm_http: None,
    }
}

/// Write a WASM binary to the cache directory and return the "image" path
fn write_wasm_to_cache(cache_dir: &TempDir, name: &str, wasm_bytes: &[u8]) -> String {
    let cache_key = name.replace(['/', ':', '@'], "_");
    let cache_path = cache_dir.path().join(format!("{}.wasm", cache_key));
    std::fs::write(&cache_path, wasm_bytes).expect("Failed to write WASM to cache");
    name.to_string()
}

/// RAII guard for WASM instance cleanup
struct InstanceGuard {
    runtime: Arc<WasmRuntime>,
    id: ContainerId,
}

impl InstanceGuard {
    fn new(runtime: Arc<WasmRuntime>, id: ContainerId) -> Self {
        Self { runtime, id }
    }
}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        let runtime = self.runtime.clone();
        let id = self.id.clone();

        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                let _ = runtime.stop_container(&id, Duration::from_secs(5)).await;
                let _ = runtime.remove_container(&id).await;
            });
        }
    }
}

// =============================================================================
// Unit Tests for WasmConfig
// =============================================================================

mod config_tests {
    use super::*;

    #[test]
    fn test_wasm_config_default() {
        let config = WasmConfig::default();

        assert!(config.enable_epochs);
        assert_eq!(config.epoch_deadline, 1_000_000);
        assert_eq!(config.max_execution_time, Duration::from_secs(3600));
    }

    #[test]
    fn test_wasm_config_custom() {
        let config = WasmConfig {
            cache_dir: PathBuf::from("/custom/path"),
            enable_epochs: false,
            epoch_deadline: 500_000,
            max_execution_time: Duration::from_secs(60),
            cache_type: None,
        };

        assert_eq!(config.cache_dir, PathBuf::from("/custom/path"));
        assert!(!config.enable_epochs);
        assert_eq!(config.epoch_deadline, 500_000);
        assert_eq!(config.max_execution_time, Duration::from_secs(60));
    }

    #[test]
    fn test_wasm_config_clone() {
        let config = WasmConfig {
            cache_dir: PathBuf::from("/test"),
            enable_epochs: true,
            epoch_deadline: 100_000,
            max_execution_time: Duration::from_secs(120),
            cache_type: None,
        };

        let cloned = config.clone();

        assert_eq!(cloned.cache_dir, config.cache_dir);
        assert_eq!(cloned.enable_epochs, config.enable_epochs);
        assert_eq!(cloned.epoch_deadline, config.epoch_deadline);
        assert_eq!(cloned.max_execution_time, config.max_execution_time);
    }
}

// =============================================================================
// WAT Compilation Tests
// =============================================================================

mod wat_tests {
    use super::*;

    #[test]
    fn test_compile_minimal_wasm() {
        let bytes = compile_wat(MINIMAL_WASM);
        assert!(!bytes.is_empty());
        // WASM magic number: \0asm
        assert_eq!(&bytes[0..4], b"\0asm");
    }

    #[test]
    fn test_compile_exit_code_wasm() {
        let bytes = compile_wat(EXIT_CODE_WASM);
        assert!(!bytes.is_empty());
        assert_eq!(&bytes[0..4], b"\0asm");
    }

    #[test]
    fn test_compile_hello_world_wasm() {
        let bytes = compile_wat(HELLO_WORLD_WASM);
        assert!(!bytes.is_empty());
        assert_eq!(&bytes[0..4], b"\0asm");
    }

    #[test]
    fn test_compile_compute_wasm() {
        let bytes = compile_wat(COMPUTE_WASM);
        assert!(!bytes.is_empty());
        assert_eq!(&bytes[0..4], b"\0asm");
    }

    #[test]
    fn test_compile_no_entry_wasm() {
        let bytes = compile_wat(NO_ENTRY_WASM);
        assert!(!bytes.is_empty());
        assert_eq!(&bytes[0..4], b"\0asm");
    }
}

// =============================================================================
// WasmRuntime Creation Tests
// =============================================================================

mod runtime_creation_tests {
    use super::*;

    #[tokio::test]
    async fn test_wasm_runtime_creation() {
        let cache_dir = create_test_cache_dir();
        let config = create_test_config(&cache_dir);

        let runtime = WasmRuntime::new(config).await;
        assert!(
            runtime.is_ok(),
            "Failed to create WasmRuntime: {:?}",
            runtime
        );

        let runtime = runtime.unwrap();
        println!("WasmRuntime created successfully: {:?}", runtime);
    }

    #[tokio::test]
    async fn test_wasm_runtime_with_defaults() {
        // Note: This may fail if the default cache directory is not writable
        // In CI/test environments, this test may need to be skipped
        let cache_dir = create_test_cache_dir();

        // Override the default by using create_test_config
        let config = WasmConfig {
            cache_dir: cache_dir.path().to_path_buf(),
            ..WasmConfig::default()
        };

        let runtime = WasmRuntime::new(config).await;
        assert!(
            runtime.is_ok(),
            "Failed to create WasmRuntime with modified defaults: {:?}",
            runtime
        );
    }

    #[tokio::test]
    async fn test_wasm_runtime_creates_cache_directory() {
        let base_dir = create_test_cache_dir();
        let cache_dir = base_dir.path().join("nested").join("cache");

        let config = WasmConfig {
            cache_dir: cache_dir.clone(),
            enable_epochs: true,
            epoch_deadline: 100_000,
            max_execution_time: Duration::from_secs(30),
            cache_type: None,
        };

        let runtime = WasmRuntime::new(config).await;
        assert!(runtime.is_ok());
        assert!(cache_dir.exists(), "Cache directory should be created");
    }
}

// =============================================================================
// Instance Lifecycle Tests
// =============================================================================

mod lifecycle_tests {
    use super::*;

    #[tokio::test]
    async fn test_create_wasm_instance() {
        let cache_dir = create_test_cache_dir();
        let config = create_test_config(&cache_dir);
        let runtime = Arc::new(
            WasmRuntime::new(config)
                .await
                .expect("Failed to create runtime"),
        );

        // Write minimal WASM to cache
        let wasm_bytes = compile_wat(MINIMAL_WASM);
        let image = write_wasm_to_cache(&cache_dir, "test:minimal", &wasm_bytes);

        let id = unique_container_id("create");
        let spec = create_wasm_spec(&image);

        let _guard = InstanceGuard::new(runtime.clone(), id.clone());

        // Create instance
        let result = runtime.create_container(&id, &spec).await;
        assert!(
            result.is_ok(),
            "Failed to create WASM instance: {:?}",
            result
        );

        // Verify state is Pending
        let state = runtime.container_state(&id).await;
        assert!(state.is_ok());
        assert_eq!(state.unwrap(), ContainerState::Pending);

        println!("WASM instance created successfully");
    }

    #[tokio::test]
    async fn test_start_wasm_instance() {
        let cache_dir = create_test_cache_dir();
        let config = create_test_config(&cache_dir);
        let runtime = Arc::new(
            WasmRuntime::new(config)
                .await
                .expect("Failed to create runtime"),
        );

        // Write minimal WASM to cache
        let wasm_bytes = compile_wat(MINIMAL_WASM);
        let image = write_wasm_to_cache(&cache_dir, "test:start", &wasm_bytes);

        let id = unique_container_id("start");
        let spec = create_wasm_spec(&image);

        let _guard = InstanceGuard::new(runtime.clone(), id.clone());

        // Create and start instance
        runtime
            .create_container(&id, &spec)
            .await
            .expect("Failed to create");
        let result = runtime.start_container(&id).await;
        assert!(
            result.is_ok(),
            "Failed to start WASM instance: {:?}",
            result
        );

        println!("WASM instance started successfully");
    }

    #[tokio::test]
    async fn test_stop_wasm_instance() {
        let cache_dir = create_test_cache_dir();
        let config = create_test_config(&cache_dir);
        let runtime = Arc::new(
            WasmRuntime::new(config)
                .await
                .expect("Failed to create runtime"),
        );

        // Write compute WASM to cache (takes a bit longer to run)
        let wasm_bytes = compile_wat(COMPUTE_WASM);
        let image = write_wasm_to_cache(&cache_dir, "test:stop", &wasm_bytes);

        let id = unique_container_id("stop");
        let spec = create_wasm_spec(&image);

        let _guard = InstanceGuard::new(runtime.clone(), id.clone());

        // Create and start instance
        runtime
            .create_container(&id, &spec)
            .await
            .expect("Failed to create");
        runtime.start_container(&id).await.expect("Failed to start");

        // Stop instance (short timeout since minimal WASM exits quickly)
        let result = runtime.stop_container(&id, Duration::from_secs(5)).await;
        assert!(result.is_ok(), "Failed to stop WASM instance: {:?}", result);

        println!("WASM instance stopped successfully");
    }

    #[tokio::test]
    async fn test_remove_wasm_instance() {
        let cache_dir = create_test_cache_dir();
        let config = create_test_config(&cache_dir);
        let runtime = Arc::new(
            WasmRuntime::new(config)
                .await
                .expect("Failed to create runtime"),
        );

        // Write minimal WASM to cache
        let wasm_bytes = compile_wat(MINIMAL_WASM);
        let image = write_wasm_to_cache(&cache_dir, "test:remove", &wasm_bytes);

        let id = unique_container_id("remove");
        let spec = create_wasm_spec(&image);

        // Create instance (no guard - we'll manually remove)
        runtime
            .create_container(&id, &spec)
            .await
            .expect("Failed to create");

        // Remove instance
        let result = runtime.remove_container(&id).await;
        assert!(
            result.is_ok(),
            "Failed to remove WASM instance: {:?}",
            result
        );

        // Verify instance is gone
        let state = runtime.container_state(&id).await;
        assert!(state.is_err(), "Instance should not exist after removal");

        println!("WASM instance removed successfully");
    }

    #[tokio::test]
    async fn test_full_lifecycle() {
        let cache_dir = create_test_cache_dir();
        let config = create_test_config(&cache_dir);
        let runtime = Arc::new(
            WasmRuntime::new(config)
                .await
                .expect("Failed to create runtime"),
        );

        // Write minimal WASM to cache
        let wasm_bytes = compile_wat(MINIMAL_WASM);
        let image = write_wasm_to_cache(&cache_dir, "test:lifecycle", &wasm_bytes);

        let id = unique_container_id("lifecycle");
        let spec = create_wasm_spec(&image);

        // 1. Create
        println!("Creating WASM instance: {}", id.service);
        runtime
            .create_container(&id, &spec)
            .await
            .expect("Failed to create");
        assert_eq!(
            runtime.container_state(&id).await.unwrap(),
            ContainerState::Pending
        );

        // 2. Start
        println!("Starting WASM instance");
        runtime.start_container(&id).await.expect("Failed to start");

        // 3. Wait for completion
        println!("Waiting for WASM instance to complete");
        let exit_code = runtime.wait_container(&id).await.expect("Failed to wait");
        assert_eq!(exit_code, 0, "Expected exit code 0");

        // 4. Verify completed state
        let state = runtime.container_state(&id).await.unwrap();
        match state {
            ContainerState::Exited { code } => assert_eq!(code, 0),
            other => panic!("Expected Exited state, got: {:?}", other),
        }

        // 5. Remove
        println!("Removing WASM instance");
        runtime
            .remove_container(&id)
            .await
            .expect("Failed to remove");
        assert!(runtime.container_state(&id).await.is_err());

        println!("Full lifecycle completed successfully");
    }
}

// =============================================================================
// State Tracking Tests
// =============================================================================

mod state_tests {
    use super::*;

    #[tokio::test]
    async fn test_state_pending() {
        let cache_dir = create_test_cache_dir();
        let config = create_test_config(&cache_dir);
        let runtime = Arc::new(
            WasmRuntime::new(config)
                .await
                .expect("Failed to create runtime"),
        );

        let wasm_bytes = compile_wat(MINIMAL_WASM);
        let image = write_wasm_to_cache(&cache_dir, "test:pending", &wasm_bytes);

        let id = unique_container_id("pending");
        let spec = create_wasm_spec(&image);

        let _guard = InstanceGuard::new(runtime.clone(), id.clone());

        runtime
            .create_container(&id, &spec)
            .await
            .expect("Failed to create");

        let state = runtime
            .container_state(&id)
            .await
            .expect("Failed to get state");
        assert_eq!(state, ContainerState::Pending);
    }

    #[tokio::test]
    async fn test_state_running() {
        let cache_dir = create_test_cache_dir();
        let config = create_test_config(&cache_dir);
        let runtime = Arc::new(
            WasmRuntime::new(config)
                .await
                .expect("Failed to create runtime"),
        );

        // Use compute WASM which takes a tiny bit longer
        let wasm_bytes = compile_wat(COMPUTE_WASM);
        let image = write_wasm_to_cache(&cache_dir, "test:running", &wasm_bytes);

        let id = unique_container_id("running");
        let spec = create_wasm_spec(&image);

        let _guard = InstanceGuard::new(runtime.clone(), id.clone());

        runtime
            .create_container(&id, &spec)
            .await
            .expect("Failed to create");
        runtime.start_container(&id).await.expect("Failed to start");

        // Check state immediately after start - should be Running
        let state = runtime
            .container_state(&id)
            .await
            .expect("Failed to get state");
        // Note: Due to the speed of WASM execution, it might already be Completed
        assert!(
            matches!(
                state,
                ContainerState::Running | ContainerState::Exited { .. }
            ),
            "Expected Running or Exited, got: {:?}",
            state
        );
    }

    #[tokio::test]
    async fn test_state_exited_success() {
        let cache_dir = create_test_cache_dir();
        let config = create_test_config(&cache_dir);
        let runtime = Arc::new(
            WasmRuntime::new(config)
                .await
                .expect("Failed to create runtime"),
        );

        let wasm_bytes = compile_wat(MINIMAL_WASM);
        let image = write_wasm_to_cache(&cache_dir, "test:exited", &wasm_bytes);

        let id = unique_container_id("exited");
        let spec = create_wasm_spec(&image);

        let _guard = InstanceGuard::new(runtime.clone(), id.clone());

        runtime
            .create_container(&id, &spec)
            .await
            .expect("Failed to create");
        runtime.start_container(&id).await.expect("Failed to start");

        // Wait for completion
        let exit_code = runtime.wait_container(&id).await.expect("Failed to wait");
        assert_eq!(exit_code, 0);

        let state = runtime
            .container_state(&id)
            .await
            .expect("Failed to get state");
        assert_eq!(state, ContainerState::Exited { code: 0 });
    }

    #[tokio::test]
    async fn test_state_exited_with_code() {
        let cache_dir = create_test_cache_dir();
        let config = create_test_config(&cache_dir);
        let runtime = Arc::new(
            WasmRuntime::new(config)
                .await
                .expect("Failed to create runtime"),
        );

        let wasm_bytes = compile_wat(EXIT_CODE_WASM);
        let image = write_wasm_to_cache(&cache_dir, "test:exitcode", &wasm_bytes);

        let id = unique_container_id("exitcode");
        let spec = create_wasm_spec(&image);

        let _guard = InstanceGuard::new(runtime.clone(), id.clone());

        runtime
            .create_container(&id, &spec)
            .await
            .expect("Failed to create");
        runtime.start_container(&id).await.expect("Failed to start");

        // Wait for completion
        let exit_code = runtime.wait_container(&id).await.expect("Failed to wait");
        assert_eq!(exit_code, 42);

        let state = runtime
            .container_state(&id)
            .await
            .expect("Failed to get state");
        assert_eq!(state, ContainerState::Exited { code: 42 });
    }

    #[tokio::test]
    async fn test_state_failed_no_entry() {
        let cache_dir = create_test_cache_dir();
        let config = create_test_config(&cache_dir);
        let runtime = Arc::new(
            WasmRuntime::new(config)
                .await
                .expect("Failed to create runtime"),
        );

        let wasm_bytes = compile_wat(NO_ENTRY_WASM);
        let image = write_wasm_to_cache(&cache_dir, "test:noentry", &wasm_bytes);

        let id = unique_container_id("noentry");
        let spec = create_wasm_spec(&image);

        let _guard = InstanceGuard::new(runtime.clone(), id.clone());

        runtime
            .create_container(&id, &spec)
            .await
            .expect("Failed to create");
        runtime.start_container(&id).await.expect("Failed to start");

        // Wait should return an error because there's no entry point
        let result = runtime.wait_container(&id).await;
        assert!(
            result.is_err()
                || matches!(
                    runtime.container_state(&id).await,
                    Ok(ContainerState::Failed { .. })
                )
        );
    }

    #[tokio::test]
    async fn test_state_not_found() {
        let cache_dir = create_test_cache_dir();
        let config = create_test_config(&cache_dir);
        let runtime = WasmRuntime::new(config)
            .await
            .expect("Failed to create runtime");

        let id = ContainerId {
            service: "nonexistent".to_string(),
            replica: 999,
        };

        let state = runtime.container_state(&id).await;
        assert!(state.is_err(), "Should fail for nonexistent instance");
    }
}

// =============================================================================
// Logs and Stats Tests
// =============================================================================

mod logs_stats_tests {
    use super::*;

    #[tokio::test]
    async fn test_get_logs_empty() {
        let cache_dir = create_test_cache_dir();
        let config = create_test_config(&cache_dir);
        let runtime = Arc::new(
            WasmRuntime::new(config)
                .await
                .expect("Failed to create runtime"),
        );

        let wasm_bytes = compile_wat(MINIMAL_WASM);
        let image = write_wasm_to_cache(&cache_dir, "test:logs-empty", &wasm_bytes);

        let id = unique_container_id("logs-empty");
        let spec = create_wasm_spec(&image);

        let _guard = InstanceGuard::new(runtime.clone(), id.clone());

        runtime
            .create_container(&id, &spec)
            .await
            .expect("Failed to create");

        // Logs should be accessible even before start
        let logs = runtime.container_logs(&id, 100).await;
        assert!(logs.is_ok());
    }

    #[tokio::test]
    async fn test_get_log_lines() {
        let cache_dir = create_test_cache_dir();
        let config = create_test_config(&cache_dir);
        let runtime = Arc::new(
            WasmRuntime::new(config)
                .await
                .expect("Failed to create runtime"),
        );

        let wasm_bytes = compile_wat(MINIMAL_WASM);
        let image = write_wasm_to_cache(&cache_dir, "test:loglines", &wasm_bytes);

        let id = unique_container_id("loglines");
        let spec = create_wasm_spec(&image);

        let _guard = InstanceGuard::new(runtime.clone(), id.clone());

        runtime
            .create_container(&id, &spec)
            .await
            .expect("Failed to create");

        let lines = runtime.get_logs(&id).await;
        assert!(lines.is_ok());
    }

    #[tokio::test]
    async fn test_get_container_stats() {
        let cache_dir = create_test_cache_dir();
        let config = create_test_config(&cache_dir);
        let runtime = Arc::new(
            WasmRuntime::new(config)
                .await
                .expect("Failed to create runtime"),
        );

        let wasm_bytes = compile_wat(MINIMAL_WASM);
        let image = write_wasm_to_cache(&cache_dir, "test:stats", &wasm_bytes);

        let id = unique_container_id("stats");
        let spec = create_wasm_spec(&image);

        let _guard = InstanceGuard::new(runtime.clone(), id.clone());

        runtime
            .create_container(&id, &spec)
            .await
            .expect("Failed to create");

        // WASM runtime returns stub stats
        let stats = runtime.get_container_stats(&id).await;
        assert!(stats.is_ok());

        let stats = stats.unwrap();
        // WASM doesn't have cgroups, so CPU/memory are stub values
        assert_eq!(stats.cpu_usage_usec, 0);
        assert_eq!(stats.memory_bytes, 0);
        assert_eq!(stats.memory_limit, u64::MAX);
    }

    #[tokio::test]
    async fn test_stats_not_found() {
        let cache_dir = create_test_cache_dir();
        let config = create_test_config(&cache_dir);
        let runtime = WasmRuntime::new(config)
            .await
            .expect("Failed to create runtime");

        let id = ContainerId {
            service: "ghost".to_string(),
            replica: 1,
        };

        let stats = runtime.get_container_stats(&id).await;
        assert!(stats.is_err(), "Should fail for nonexistent instance");
    }
}

// =============================================================================
// Exec Tests
// =============================================================================

mod exec_tests {
    use super::*;

    #[tokio::test]
    async fn test_exec_not_supported() {
        let cache_dir = create_test_cache_dir();
        let config = create_test_config(&cache_dir);
        let runtime = Arc::new(
            WasmRuntime::new(config)
                .await
                .expect("Failed to create runtime"),
        );

        let wasm_bytes = compile_wat(COMPUTE_WASM);
        let image = write_wasm_to_cache(&cache_dir, "test:exec", &wasm_bytes);

        let id = unique_container_id("exec");
        let spec = create_wasm_spec(&image);

        let _guard = InstanceGuard::new(runtime.clone(), id.clone());

        runtime
            .create_container(&id, &spec)
            .await
            .expect("Failed to create");
        runtime.start_container(&id).await.expect("Failed to start");

        // Exec should fail for WASM
        let cmd = vec!["echo".to_string(), "test".to_string()];
        let result = runtime.exec(&id, &cmd).await;

        assert!(result.is_err(), "Exec should not be supported for WASM");
        let err_msg = format!("{:?}", result.unwrap_err());
        assert!(
            err_msg.contains("not supported") || err_msg.contains("WASM"),
            "Error should mention exec not being supported for WASM"
        );
    }
}

// =============================================================================
// Wait Tests
// =============================================================================

mod wait_tests {
    use super::*;

    #[tokio::test]
    async fn test_wait_success() {
        let cache_dir = create_test_cache_dir();
        let config = create_test_config(&cache_dir);
        let runtime = Arc::new(
            WasmRuntime::new(config)
                .await
                .expect("Failed to create runtime"),
        );

        let wasm_bytes = compile_wat(MINIMAL_WASM);
        let image = write_wasm_to_cache(&cache_dir, "test:wait-success", &wasm_bytes);

        let id = unique_container_id("wait-success");
        let spec = create_wasm_spec(&image);

        let _guard = InstanceGuard::new(runtime.clone(), id.clone());

        runtime
            .create_container(&id, &spec)
            .await
            .expect("Failed to create");
        runtime.start_container(&id).await.expect("Failed to start");

        let exit_code = runtime.wait_container(&id).await.expect("Failed to wait");
        assert_eq!(exit_code, 0);
    }

    #[tokio::test]
    async fn test_wait_with_exit_code() {
        let cache_dir = create_test_cache_dir();
        let config = create_test_config(&cache_dir);
        let runtime = Arc::new(
            WasmRuntime::new(config)
                .await
                .expect("Failed to create runtime"),
        );

        let wasm_bytes = compile_wat(EXIT_CODE_WASM);
        let image = write_wasm_to_cache(&cache_dir, "test:wait-42", &wasm_bytes);

        let id = unique_container_id("wait-42");
        let spec = create_wasm_spec(&image);

        let _guard = InstanceGuard::new(runtime.clone(), id.clone());

        runtime
            .create_container(&id, &spec)
            .await
            .expect("Failed to create");
        runtime.start_container(&id).await.expect("Failed to start");

        let exit_code = runtime.wait_container(&id).await.expect("Failed to wait");
        assert_eq!(exit_code, 42);
    }

    #[tokio::test]
    async fn test_wait_compute() {
        let cache_dir = create_test_cache_dir();
        let config = create_test_config(&cache_dir);
        let runtime = Arc::new(
            WasmRuntime::new(config)
                .await
                .expect("Failed to create runtime"),
        );

        let wasm_bytes = compile_wat(COMPUTE_WASM);
        let image = write_wasm_to_cache(&cache_dir, "test:compute", &wasm_bytes);

        let id = unique_container_id("compute");
        let spec = create_wasm_spec(&image);

        let _guard = InstanceGuard::new(runtime.clone(), id.clone());

        runtime
            .create_container(&id, &spec)
            .await
            .expect("Failed to create");
        runtime.start_container(&id).await.expect("Failed to start");

        let exit_code = runtime.wait_container(&id).await.expect("Failed to wait");
        assert_eq!(exit_code, 0, "Compute module should exit successfully");
    }
}

// =============================================================================
// Instance ID Generation Tests
// =============================================================================

mod instance_id_tests {
    use super::*;

    #[test]
    fn test_container_id_display() {
        let id = ContainerId {
            service: "myservice".to_string(),
            replica: 1,
        };

        let display = format!("{}", id);
        assert_eq!(display, "myservice-rep-1");
    }

    #[test]
    fn test_cache_key_sanitization() {
        let test_cases = vec![
            ("ghcr.io/org/module:v1.0", "ghcr.io_org_module_v1.0"),
            (
                "registry.example.com/wasm@sha256:abc",
                "registry.example.com_wasm_sha256_abc",
            ),
            ("simple-name:latest", "simple-name_latest"),
            ("a/b/c/d:tag", "a_b_c_d_tag"),
        ];

        for (input, expected) in test_cases {
            let sanitized = input.replace(['/', ':', '@'], "_");
            assert_eq!(sanitized, expected, "Failed for input: {}", input);
        }
    }
}
