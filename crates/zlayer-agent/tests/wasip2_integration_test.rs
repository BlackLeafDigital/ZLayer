//! WASIp2 Component Integration Tests
//!
//! These tests verify the WasmRuntime's handling of WASIp2 (WebAssembly Component Model)
//! artifacts. Since creating valid WASIp2 component bytes programmatically requires the
//! component toolchain (wit-component, wasm-tools), these tests focus on:
//!
//! 1. **Component Loading Tests**: Detection and loading of component binaries
//! 2. **WASI Interface Binding Tests**: Verification that WASI interfaces are linked
//! 3. **Error Handling Tests**: Proper error responses for invalid/malformed components
//! 4. **Version Detection Tests**: Correct identification of WASIp2 vs WASIp1 binaries
//!
//! # Test Architecture
//!
//! - Tests use programmatically constructed binary headers to test detection logic
//! - Valid execution tests use WASIp1 modules (via `wat` crate) where component execution
//!   would require pre-built fixtures
//! - Error paths are tested with malformed/invalid binary data
//!
//! # Running
//! ```bash
//! cargo test -p zlayer-agent --features wasm -- wasip2 --nocapture
//! ```

#![cfg(feature = "wasm")]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use zlayer_agent::runtimes::{WasmConfig, WasmRuntime};
use zlayer_agent::{ContainerId, ContainerState, Runtime};
use zlayer_registry::{detect_wasm_version_from_binary, WasiVersion};
use zlayer_spec::{
    CommandSpec, ErrorsSpec, HealthCheck, HealthSpec, ImageSpec, InitSpec, NetworkSpec, NodeMode,
    PullPolicy, ResourceType, ResourcesSpec, ScaleSpec, ServiceSpec,
};

// =============================================================================
// Test Binary Constants and Helpers
// =============================================================================

/// WASM magic bytes: `\0asm`
const WASM_MAGIC: [u8; 4] = [0x00, 0x61, 0x73, 0x6d];

/// Version 1 (little-endian) - used by WASIp1 core modules
const MODULE_VERSION: [u8; 4] = [0x01, 0x00, 0x00, 0x00];

/// Version 13 (0x0d, little-endian) - used by WASIp2 components
const COMPONENT_VERSION: [u8; 4] = [0x0d, 0x00, 0x00, 0x00];

/// Create a minimal valid WASIp1 module header
fn make_module_header() -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&WASM_MAGIC);
    bytes.extend_from_slice(&MODULE_VERSION);
    bytes.push(0x01); // Type section marker
    bytes
}

/// Create a minimal WASIp2 component header (binary header only, not fully valid)
///
/// Note: This creates a binary that will be detected as a component based on its
/// version number (13), but it is not a fully valid component. It's useful for
/// testing detection and error handling paths.
fn make_component_header() -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&WASM_MAGIC);
    bytes.extend_from_slice(&COMPONENT_VERSION);
    bytes.push(0x00); // Component section marker
    bytes
}

/// Create a more complete (but still invalid) component binary
///
/// This includes additional bytes to make it look more like a real component,
/// but without valid component model sections it will fail to instantiate.
fn make_invalid_component() -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&WASM_MAGIC);
    bytes.extend_from_slice(&COMPONENT_VERSION);
    // Add some fake component sections
    bytes.push(0x00); // component section type
    bytes.push(0x08); // size: 8 bytes
    bytes.extend_from_slice(&[0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08]);
    bytes
}

/// Create a temporary cache directory for testing
fn create_test_cache_dir() -> TempDir {
    tempfile::tempdir().expect("Failed to create temp directory")
}

/// Create a WasmConfig for testing with shorter timeouts
fn create_test_config(cache_dir: &TempDir) -> WasmConfig {
    WasmConfig {
        cache_dir: cache_dir.path().to_path_buf(),
        enable_epochs: true,
        epoch_deadline: 100_000,
        max_execution_time: Duration::from_secs(10), // Short timeout for tests
    }
}

/// Generate a unique service name to avoid test collisions
fn unique_service_name(prefix: &str) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64
        % 1_000_000_000;
    format!("test-{}-{}", prefix, timestamp)
}

/// Create a ContainerId with a unique service name
fn unique_container_id(prefix: &str) -> ContainerId {
    ContainerId {
        service: unique_service_name(prefix),
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
            pull_policy: PullPolicy::Never, // Using local cache
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

/// Create a ServiceSpec with environment variables
fn create_wasm_spec_with_env(image: &str, env: HashMap<String, String>) -> ServiceSpec {
    let mut spec = create_wasm_spec(image);
    spec.env = env;
    spec
}

/// Create a ServiceSpec with command arguments
fn create_wasm_spec_with_args(image: &str, args: Vec<String>) -> ServiceSpec {
    let mut spec = create_wasm_spec(image);
    spec.command = CommandSpec {
        entrypoint: None,
        args: Some(args),
        workdir: None,
    };
    spec
}

/// Write bytes to the cache directory and return the "image" name
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
                let _ = runtime.stop_container(&id, Duration::from_secs(1)).await;
                let _ = runtime.remove_container(&id).await;
            });
        }
    }
}

// =============================================================================
// WASIp1 Test Modules (WAT - WebAssembly Text Format)
// These are used for comparison and to verify module vs component detection
// =============================================================================

/// Minimal WASIp1 module that exports _start and exits with code 0
const MINIMAL_P1_MODULE: &str = r#"
(module
    (import "wasi_snapshot_preview1" "proc_exit" (func $proc_exit (param i32)))
    (memory (export "memory") 1)
    (func (export "_start")
        i32.const 0
        call $proc_exit
    )
)
"#;

/// WASIp1 module that exits with code 42
const EXIT_42_P1_MODULE: &str = r#"
(module
    (import "wasi_snapshot_preview1" "proc_exit" (func $proc_exit (param i32)))
    (memory (export "memory") 1)
    (func (export "_start")
        i32.const 42
        call $proc_exit
    )
)
"#;

/// WASIp1 module without _start or main (should fail)
const NO_ENTRY_P1_MODULE: &str = r#"
(module
    (memory (export "memory") 1)
    (func $helper (result i32)
        i32.const 42
    )
)
"#;

/// Compile WAT to WASM binary
fn compile_wat(wat: &str) -> Vec<u8> {
    wat::parse_str(wat).expect("Failed to parse WAT")
}

// =============================================================================
// Component Loading Tests
// =============================================================================

mod component_loading_tests {
    use super::*;

    #[test]
    fn test_component_header_detection() {
        // Verify that our test helper creates a binary detected as Preview2
        let component_bytes = make_component_header();
        let version = detect_wasm_version_from_binary(&component_bytes);
        assert_eq!(
            version,
            WasiVersion::Preview2,
            "Component header should be detected as Preview2"
        );
    }

    #[test]
    fn test_module_header_detection() {
        // Verify that module headers are detected as Preview1
        let module_bytes = make_module_header();
        let version = detect_wasm_version_from_binary(&module_bytes);
        assert_eq!(
            version,
            WasiVersion::Preview1,
            "Module header should be detected as Preview1"
        );
    }

    #[test]
    fn test_compiled_module_detection() {
        // Verify that compiled WAT modules are detected correctly
        let wasm_bytes = compile_wat(MINIMAL_P1_MODULE);
        let version = detect_wasm_version_from_binary(&wasm_bytes);
        assert_eq!(
            version,
            WasiVersion::Preview1,
            "Compiled WAT module should be detected as Preview1"
        );
    }

    #[tokio::test]
    async fn test_load_valid_module() {
        let cache_dir = create_test_cache_dir();
        let config = create_test_config(&cache_dir);
        let runtime = Arc::new(
            WasmRuntime::new(config)
                .await
                .expect("Failed to create runtime"),
        );

        let wasm_bytes = compile_wat(MINIMAL_P1_MODULE);
        let image = write_wasm_to_cache(&cache_dir, "test:valid-module", &wasm_bytes);

        let id = unique_container_id("load-module");
        let spec = create_wasm_spec(&image);
        let _guard = InstanceGuard::new(runtime.clone(), id.clone());

        // Creating the container should succeed
        let result = runtime.create_container(&id, &spec).await;
        assert!(
            result.is_ok(),
            "Loading valid module should succeed: {:?}",
            result
        );

        // State should be Pending
        let state = runtime
            .container_state(&id)
            .await
            .expect("Failed to get state");
        assert_eq!(state, ContainerState::Pending);
    }

    #[tokio::test]
    async fn test_load_component_header_fails_at_execution() {
        // A component header without valid component content should fail when executed
        let cache_dir = create_test_cache_dir();
        let config = create_test_config(&cache_dir);
        let runtime = Arc::new(
            WasmRuntime::new(config)
                .await
                .expect("Failed to create runtime"),
        );

        let component_bytes = make_invalid_component();
        let image = write_wasm_to_cache(&cache_dir, "test:invalid-component", &component_bytes);

        let id = unique_container_id("invalid-component");
        let spec = create_wasm_spec(&image);
        let _guard = InstanceGuard::new(runtime.clone(), id.clone());

        // Creating should succeed (just stores the bytes)
        let create_result = runtime.create_container(&id, &spec).await;
        assert!(
            create_result.is_ok(),
            "Create should succeed: {:?}",
            create_result
        );

        // Starting should also succeed (spawns async task)
        let start_result = runtime.start_container(&id).await;
        assert!(
            start_result.is_ok(),
            "Start should succeed: {:?}",
            start_result
        );

        // But waiting should fail because the component is invalid
        let wait_result = runtime.wait_container(&id).await;
        assert!(
            wait_result.is_err()
                || matches!(
                    runtime.container_state(&id).await,
                    Ok(ContainerState::Failed { .. })
                ),
            "Invalid component should fail during execution"
        );
    }

    #[test]
    fn test_invalid_magic_detection() {
        // Invalid magic bytes should return Unknown
        let invalid_bytes = [0x7f, 0x45, 0x4c, 0x46, 0x01, 0x00, 0x00, 0x00, 0x01]; // ELF header
        let version = detect_wasm_version_from_binary(&invalid_bytes);
        assert_eq!(
            version,
            WasiVersion::Unknown,
            "Invalid magic should return Unknown"
        );
    }

    #[test]
    fn test_truncated_binary_detection() {
        // Binary too short to determine version
        let short_bytes = [0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00]; // No section type
        let version = detect_wasm_version_from_binary(&short_bytes);
        assert_eq!(
            version,
            WasiVersion::Unknown,
            "Truncated binary should return Unknown"
        );
    }

    #[test]
    fn test_empty_binary_detection() {
        let empty: &[u8] = &[];
        let version = detect_wasm_version_from_binary(empty);
        assert_eq!(
            version,
            WasiVersion::Unknown,
            "Empty binary should return Unknown"
        );
    }
}

// =============================================================================
// WASI Interface Binding Tests
// =============================================================================

mod wasi_interface_tests {
    use super::*;

    #[tokio::test]
    async fn test_wasi_proc_exit_available() {
        // Test that proc_exit WASI function is available (via successful exit)
        let cache_dir = create_test_cache_dir();
        let config = create_test_config(&cache_dir);
        let runtime = Arc::new(
            WasmRuntime::new(config)
                .await
                .expect("Failed to create runtime"),
        );

        let wasm_bytes = compile_wat(MINIMAL_P1_MODULE);
        let image = write_wasm_to_cache(&cache_dir, "test:proc-exit", &wasm_bytes);

        let id = unique_container_id("proc-exit");
        let spec = create_wasm_spec(&image);
        let _guard = InstanceGuard::new(runtime.clone(), id.clone());

        runtime.create_container(&id, &spec).await.unwrap();
        runtime.start_container(&id).await.unwrap();

        // Should exit with code 0
        let exit_code = runtime
            .wait_container(&id)
            .await
            .expect("Module with proc_exit should complete");
        assert_eq!(exit_code, 0, "Should exit with code 0");
    }

    #[tokio::test]
    async fn test_wasi_exit_code_passed_correctly() {
        // Test that exit codes are correctly propagated
        let cache_dir = create_test_cache_dir();
        let config = create_test_config(&cache_dir);
        let runtime = Arc::new(
            WasmRuntime::new(config)
                .await
                .expect("Failed to create runtime"),
        );

        let wasm_bytes = compile_wat(EXIT_42_P1_MODULE);
        let image = write_wasm_to_cache(&cache_dir, "test:exit-42", &wasm_bytes);

        let id = unique_container_id("exit-42");
        let spec = create_wasm_spec(&image);
        let _guard = InstanceGuard::new(runtime.clone(), id.clone());

        runtime.create_container(&id, &spec).await.unwrap();
        runtime.start_container(&id).await.unwrap();

        let exit_code = runtime.wait_container(&id).await.expect("Should complete");
        assert_eq!(exit_code, 42, "Should exit with code 42");

        // Verify state reflects the exit code
        let state = runtime.container_state(&id).await.unwrap();
        assert_eq!(state, ContainerState::Exited { code: 42 });
    }

    #[tokio::test]
    async fn test_env_vars_available_in_context() {
        // Test that environment variables are set up in the WASI context
        // Note: We can't easily verify from outside, but this tests the code path
        let cache_dir = create_test_cache_dir();
        let config = create_test_config(&cache_dir);
        let runtime = Arc::new(
            WasmRuntime::new(config)
                .await
                .expect("Failed to create runtime"),
        );

        let wasm_bytes = compile_wat(MINIMAL_P1_MODULE);
        let image = write_wasm_to_cache(&cache_dir, "test:with-env", &wasm_bytes);

        let mut env = HashMap::new();
        env.insert("MY_VAR".to_string(), "my_value".to_string());
        env.insert("ANOTHER_VAR".to_string(), "another_value".to_string());

        let id = unique_container_id("with-env");
        let spec = create_wasm_spec_with_env(&image, env);
        let _guard = InstanceGuard::new(runtime.clone(), id.clone());

        runtime.create_container(&id, &spec).await.unwrap();
        runtime.start_container(&id).await.unwrap();

        // Module should still execute successfully with env vars set
        let exit_code = runtime.wait_container(&id).await.expect("Should complete");
        assert_eq!(exit_code, 0);
    }

    #[tokio::test]
    async fn test_args_available_in_context() {
        // Test that command line arguments are set up in the WASI context
        let cache_dir = create_test_cache_dir();
        let config = create_test_config(&cache_dir);
        let runtime = Arc::new(
            WasmRuntime::new(config)
                .await
                .expect("Failed to create runtime"),
        );

        let wasm_bytes = compile_wat(MINIMAL_P1_MODULE);
        let image = write_wasm_to_cache(&cache_dir, "test:with-args", &wasm_bytes);

        let args = vec!["arg1".to_string(), "arg2".to_string(), "--flag".to_string()];

        let id = unique_container_id("with-args");
        let spec = create_wasm_spec_with_args(&image, args);
        let _guard = InstanceGuard::new(runtime.clone(), id.clone());

        runtime.create_container(&id, &spec).await.unwrap();
        runtime.start_container(&id).await.unwrap();

        // Module should execute successfully with args set
        let exit_code = runtime.wait_container(&id).await.expect("Should complete");
        assert_eq!(exit_code, 0);
    }
}

// =============================================================================
// Component Execution Tests
// =============================================================================

mod component_execution_tests {
    use super::*;

    #[tokio::test]
    async fn test_module_executes_and_completes() {
        let cache_dir = create_test_cache_dir();
        let config = create_test_config(&cache_dir);
        let runtime = Arc::new(
            WasmRuntime::new(config)
                .await
                .expect("Failed to create runtime"),
        );

        let wasm_bytes = compile_wat(MINIMAL_P1_MODULE);
        let image = write_wasm_to_cache(&cache_dir, "test:execute", &wasm_bytes);

        let id = unique_container_id("execute");
        let spec = create_wasm_spec(&image);
        let _guard = InstanceGuard::new(runtime.clone(), id.clone());

        // Full lifecycle
        runtime.create_container(&id, &spec).await.unwrap();
        assert_eq!(
            runtime.container_state(&id).await.unwrap(),
            ContainerState::Pending
        );

        runtime.start_container(&id).await.unwrap();
        // State could be Running or already Exited due to fast execution

        let exit_code = runtime.wait_container(&id).await.expect("Should complete");
        assert_eq!(exit_code, 0);

        assert_eq!(
            runtime.container_state(&id).await.unwrap(),
            ContainerState::Exited { code: 0 }
        );
    }

    #[tokio::test]
    async fn test_module_with_custom_exit_code() {
        let cache_dir = create_test_cache_dir();
        let config = create_test_config(&cache_dir);
        let runtime = Arc::new(
            WasmRuntime::new(config)
                .await
                .expect("Failed to create runtime"),
        );

        let wasm_bytes = compile_wat(EXIT_42_P1_MODULE);
        let image = write_wasm_to_cache(&cache_dir, "test:exit-code", &wasm_bytes);

        let id = unique_container_id("exit-code");
        let spec = create_wasm_spec(&image);
        let _guard = InstanceGuard::new(runtime.clone(), id.clone());

        runtime.create_container(&id, &spec).await.unwrap();
        runtime.start_container(&id).await.unwrap();

        let exit_code = runtime.wait_container(&id).await.expect("Should complete");
        assert_eq!(exit_code, 42, "Exit code should be 42");
    }

    #[tokio::test]
    async fn test_stop_container_during_execution() {
        let cache_dir = create_test_cache_dir();
        let config = create_test_config(&cache_dir);
        let runtime = Arc::new(
            WasmRuntime::new(config)
                .await
                .expect("Failed to create runtime"),
        );

        // Use a simple module - it might complete before stop, that's OK
        let wasm_bytes = compile_wat(MINIMAL_P1_MODULE);
        let image = write_wasm_to_cache(&cache_dir, "test:stop", &wasm_bytes);

        let id = unique_container_id("stop");
        let spec = create_wasm_spec(&image);
        let _guard = InstanceGuard::new(runtime.clone(), id.clone());

        runtime.create_container(&id, &spec).await.unwrap();
        runtime.start_container(&id).await.unwrap();

        // Stop immediately - might complete naturally or be stopped
        let stop_result = runtime.stop_container(&id, Duration::from_secs(1)).await;
        assert!(stop_result.is_ok(), "Stop should not error");

        // State should be either Completed or Failed (from stop)
        let state = runtime.container_state(&id).await.unwrap();
        assert!(
            matches!(
                state,
                ContainerState::Exited { .. } | ContainerState::Failed { .. }
            ),
            "State should be Exited or Failed after stop, got: {:?}",
            state
        );
    }

    #[tokio::test]
    async fn test_remove_running_instance() {
        let cache_dir = create_test_cache_dir();
        let config = create_test_config(&cache_dir);
        let runtime = Arc::new(
            WasmRuntime::new(config)
                .await
                .expect("Failed to create runtime"),
        );

        let wasm_bytes = compile_wat(MINIMAL_P1_MODULE);
        let image = write_wasm_to_cache(&cache_dir, "test:remove", &wasm_bytes);

        let id = unique_container_id("remove");
        let spec = create_wasm_spec(&image);
        // No guard - we'll remove manually

        runtime.create_container(&id, &spec).await.unwrap();
        runtime.start_container(&id).await.unwrap();

        // Remove immediately
        let remove_result = runtime.remove_container(&id).await;
        assert!(remove_result.is_ok(), "Remove should succeed");

        // Instance should be gone
        let state_result = runtime.container_state(&id).await;
        assert!(
            state_result.is_err(),
            "Instance should not exist after remove"
        );
    }
}

// =============================================================================
// Error Handling Tests
// =============================================================================

mod error_handling_tests {
    use super::*;

    #[tokio::test]
    async fn test_error_missing_start_export() {
        let cache_dir = create_test_cache_dir();
        let config = create_test_config(&cache_dir);
        let runtime = Arc::new(
            WasmRuntime::new(config)
                .await
                .expect("Failed to create runtime"),
        );

        let wasm_bytes = compile_wat(NO_ENTRY_P1_MODULE);
        let image = write_wasm_to_cache(&cache_dir, "test:no-entry", &wasm_bytes);

        let id = unique_container_id("no-entry");
        let spec = create_wasm_spec(&image);
        let _guard = InstanceGuard::new(runtime.clone(), id.clone());

        runtime.create_container(&id, &spec).await.unwrap();
        runtime.start_container(&id).await.unwrap();

        // Wait should fail because there's no _start function
        let wait_result = runtime.wait_container(&id).await;
        assert!(
            wait_result.is_err()
                || matches!(
                    runtime.container_state(&id).await,
                    Ok(ContainerState::Failed { .. })
                ),
            "Module without _start should fail"
        );

        // If there's an error in state, verify the reason mentions the missing function
        if let Ok(ContainerState::Failed { reason }) = runtime.container_state(&id).await {
            assert!(
                reason.contains("_start")
                    || reason.contains("main")
                    || reason.contains("not found"),
                "Error should mention missing entry point, got: {}",
                reason
            );
        }
    }

    #[tokio::test]
    async fn test_error_invalid_wasm_binary() {
        let cache_dir = create_test_cache_dir();
        let config = create_test_config(&cache_dir);
        let runtime = Arc::new(
            WasmRuntime::new(config)
                .await
                .expect("Failed to create runtime"),
        );

        // Random garbage bytes
        let invalid_bytes = [0xde, 0xad, 0xbe, 0xef, 0x00, 0x00, 0x00, 0x00, 0x00];
        let image = write_wasm_to_cache(&cache_dir, "test:garbage", &invalid_bytes);

        let id = unique_container_id("garbage");
        let spec = create_wasm_spec(&image);
        let _guard = InstanceGuard::new(runtime.clone(), id.clone());

        runtime.create_container(&id, &spec).await.unwrap();
        runtime.start_container(&id).await.unwrap();

        // Execution should fail due to invalid binary
        let wait_result = runtime.wait_container(&id).await;
        assert!(
            wait_result.is_err()
                || matches!(
                    runtime.container_state(&id).await,
                    Ok(ContainerState::Failed { .. })
                ),
            "Invalid binary should fail"
        );
    }

    #[tokio::test]
    async fn test_error_truncated_wasm_binary() {
        let cache_dir = create_test_cache_dir();
        let config = create_test_config(&cache_dir);
        let runtime = Arc::new(
            WasmRuntime::new(config)
                .await
                .expect("Failed to create runtime"),
        );

        // Valid magic but truncated
        let truncated_bytes = [0x00, 0x61, 0x73, 0x6d, 0x01, 0x00];
        let image = write_wasm_to_cache(&cache_dir, "test:truncated", &truncated_bytes);

        let id = unique_container_id("truncated");
        let spec = create_wasm_spec(&image);
        let _guard = InstanceGuard::new(runtime.clone(), id.clone());

        runtime.create_container(&id, &spec).await.unwrap();
        runtime.start_container(&id).await.unwrap();

        // Should fail during compilation
        let wait_result = runtime.wait_container(&id).await;
        assert!(
            wait_result.is_err()
                || matches!(
                    runtime.container_state(&id).await,
                    Ok(ContainerState::Failed { .. })
                ),
            "Truncated binary should fail"
        );
    }

    #[tokio::test]
    async fn test_error_component_invalid_structure() {
        // Test that a component header with invalid content fails
        let cache_dir = create_test_cache_dir();
        let config = create_test_config(&cache_dir);
        let runtime = Arc::new(
            WasmRuntime::new(config)
                .await
                .expect("Failed to create runtime"),
        );

        let invalid_component = make_invalid_component();
        let image = write_wasm_to_cache(&cache_dir, "test:bad-component", &invalid_component);

        let id = unique_container_id("bad-component");
        let spec = create_wasm_spec(&image);
        let _guard = InstanceGuard::new(runtime.clone(), id.clone());

        runtime.create_container(&id, &spec).await.unwrap();
        runtime.start_container(&id).await.unwrap();

        // Should fail during component compilation/instantiation
        let wait_result = runtime.wait_container(&id).await;
        assert!(
            wait_result.is_err()
                || matches!(
                    runtime.container_state(&id).await,
                    Ok(ContainerState::Failed { .. })
                ),
            "Invalid component should fail"
        );
    }

    #[tokio::test]
    async fn test_error_nonexistent_container() {
        let cache_dir = create_test_cache_dir();
        let config = create_test_config(&cache_dir);
        let runtime = WasmRuntime::new(config)
            .await
            .expect("Failed to create runtime");

        let id = ContainerId {
            service: "nonexistent-service".to_string(),
            replica: 999,
        };

        // All operations on nonexistent container should fail
        let state_result = runtime.container_state(&id).await;
        assert!(
            state_result.is_err(),
            "State of nonexistent container should error"
        );

        let start_result = runtime.start_container(&id).await;
        assert!(
            start_result.is_err(),
            "Starting nonexistent container should error"
        );

        let wait_result = runtime.wait_container(&id).await;
        assert!(
            wait_result.is_err(),
            "Waiting for nonexistent container should error"
        );

        let stats_result = runtime.get_container_stats(&id).await;
        assert!(
            stats_result.is_err(),
            "Getting stats of nonexistent container should error"
        );
    }

    #[tokio::test]
    async fn test_exec_not_supported() {
        let cache_dir = create_test_cache_dir();
        let config = create_test_config(&cache_dir);
        let runtime = Arc::new(
            WasmRuntime::new(config)
                .await
                .expect("Failed to create runtime"),
        );

        let wasm_bytes = compile_wat(MINIMAL_P1_MODULE);
        let image = write_wasm_to_cache(&cache_dir, "test:exec", &wasm_bytes);

        let id = unique_container_id("exec");
        let spec = create_wasm_spec(&image);
        let _guard = InstanceGuard::new(runtime.clone(), id.clone());

        runtime.create_container(&id, &spec).await.unwrap();
        runtime.start_container(&id).await.unwrap();

        // Exec should fail with "not supported" error
        let cmd = vec!["echo".to_string(), "test".to_string()];
        let exec_result = runtime.exec(&id, &cmd).await;

        assert!(exec_result.is_err(), "Exec should fail for WASM");
        let err_msg = format!("{:?}", exec_result.unwrap_err());
        assert!(
            err_msg.contains("not supported") || err_msg.contains("WASM"),
            "Error should mention WASM or not supported: {}",
            err_msg
        );
    }
}

// =============================================================================
// Version Detection Edge Cases
// =============================================================================

mod version_detection_tests {
    use super::*;

    #[test]
    fn test_version_13_is_component() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&WASM_MAGIC);
        bytes.extend_from_slice(&[0x0d, 0x00, 0x00, 0x00]); // Version 13
        bytes.push(0x00); // Section marker

        let version = detect_wasm_version_from_binary(&bytes);
        assert_eq!(version, WasiVersion::Preview2);
    }

    #[test]
    fn test_version_14_is_component() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&WASM_MAGIC);
        bytes.extend_from_slice(&[0x0e, 0x00, 0x00, 0x00]); // Version 14
        bytes.push(0x00);

        let version = detect_wasm_version_from_binary(&bytes);
        assert_eq!(version, WasiVersion::Preview2);
    }

    #[test]
    fn test_version_1_with_type_section_is_module() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&WASM_MAGIC);
        bytes.extend_from_slice(&MODULE_VERSION);
        bytes.push(0x01); // Type section

        let version = detect_wasm_version_from_binary(&bytes);
        assert_eq!(version, WasiVersion::Preview1);
    }

    #[test]
    fn test_version_1_with_import_section_is_module() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&WASM_MAGIC);
        bytes.extend_from_slice(&MODULE_VERSION);
        bytes.push(0x02); // Import section

        let version = detect_wasm_version_from_binary(&bytes);
        assert_eq!(version, WasiVersion::Preview1);
    }

    #[test]
    fn test_version_1_with_custom_section_is_module() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&WASM_MAGIC);
        bytes.extend_from_slice(&MODULE_VERSION);
        bytes.push(0x00); // Custom section (valid in modules too)

        let version = detect_wasm_version_from_binary(&bytes);
        // Version 1 with custom section (0x00) should still be Preview1
        assert_eq!(version, WasiVersion::Preview1);
    }

    #[test]
    fn test_version_1_with_invalid_section_is_unknown() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&WASM_MAGIC);
        bytes.extend_from_slice(&MODULE_VERSION);
        bytes.push(0x0f); // Invalid section type for core module

        let version = detect_wasm_version_from_binary(&bytes);
        assert_eq!(version, WasiVersion::Unknown);
    }

    #[test]
    fn test_version_2_is_unknown() {
        // Version 2 is not a known valid WASM version
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&WASM_MAGIC);
        bytes.extend_from_slice(&[0x02, 0x00, 0x00, 0x00]);
        bytes.push(0x01);

        let version = detect_wasm_version_from_binary(&bytes);
        assert_eq!(version, WasiVersion::Unknown);
    }

    #[test]
    fn test_all_valid_module_sections() {
        // Test all valid core module section types (0x00 through 0x0c)
        for section_type in 0x00..=0x0cu8 {
            let mut bytes = Vec::new();
            bytes.extend_from_slice(&WASM_MAGIC);
            bytes.extend_from_slice(&MODULE_VERSION);
            bytes.push(section_type);

            let version = detect_wasm_version_from_binary(&bytes);
            assert_eq!(
                version,
                WasiVersion::Preview1,
                "Section type {:02x} should be detected as Preview1",
                section_type
            );
        }
    }

    #[test]
    fn test_high_version_numbers_are_components() {
        // Test various high version numbers that should be detected as components
        for ver in [0x0d, 0x0e, 0x0f, 0x10, 0x20, 0x40, 0x80, 0xff] {
            let mut bytes = Vec::new();
            bytes.extend_from_slice(&WASM_MAGIC);
            bytes.extend_from_slice(&[ver, 0x00, 0x00, 0x00]);
            bytes.push(0x00);

            let version = detect_wasm_version_from_binary(&bytes);
            assert_eq!(
                version,
                WasiVersion::Preview2,
                "Version {:02x} should be detected as Preview2",
                ver
            );
        }
    }
}

// =============================================================================
// Runtime Configuration Tests
// =============================================================================

mod runtime_config_tests {
    use super::*;

    #[tokio::test]
    async fn test_runtime_creates_cache_directory() {
        let base_dir = create_test_cache_dir();
        let nested_cache = base_dir.path().join("deep").join("nested").join("cache");

        let config = WasmConfig {
            cache_dir: nested_cache.clone(),
            enable_epochs: true,
            epoch_deadline: 100_000,
            max_execution_time: Duration::from_secs(10),
        };

        let runtime = WasmRuntime::new(config).await;
        assert!(runtime.is_ok(), "Runtime creation should succeed");
        assert!(nested_cache.exists(), "Cache directory should be created");
    }

    #[tokio::test]
    async fn test_runtime_with_epochs_disabled() {
        let cache_dir = create_test_cache_dir();
        let config = WasmConfig {
            cache_dir: cache_dir.path().to_path_buf(),
            enable_epochs: false, // Disabled
            epoch_deadline: 100_000,
            max_execution_time: Duration::from_secs(10),
        };

        let runtime = Arc::new(
            WasmRuntime::new(config)
                .await
                .expect("Failed to create runtime"),
        );

        // Should still work normally
        let wasm_bytes = compile_wat(MINIMAL_P1_MODULE);
        let image = write_wasm_to_cache(&cache_dir, "test:no-epochs", &wasm_bytes);

        let id = unique_container_id("no-epochs");
        let spec = create_wasm_spec(&image);
        let _guard = InstanceGuard::new(runtime.clone(), id.clone());

        runtime.create_container(&id, &spec).await.unwrap();
        runtime.start_container(&id).await.unwrap();

        let exit_code = runtime.wait_container(&id).await.expect("Should complete");
        assert_eq!(exit_code, 0);
    }

    #[test]
    fn test_wasm_config_default_values() {
        let config = WasmConfig::default();

        assert!(config.enable_epochs);
        assert_eq!(config.epoch_deadline, 1_000_000);
        assert_eq!(config.max_execution_time, Duration::from_secs(3600));
        // Cache dir depends on environment
    }

    #[test]
    fn test_wasm_config_clone() {
        let config = WasmConfig {
            cache_dir: PathBuf::from("/custom/path"),
            enable_epochs: false,
            epoch_deadline: 500_000,
            max_execution_time: Duration::from_secs(120),
        };

        let cloned = config.clone();
        assert_eq!(cloned.cache_dir, config.cache_dir);
        assert_eq!(cloned.enable_epochs, config.enable_epochs);
        assert_eq!(cloned.epoch_deadline, config.epoch_deadline);
        assert_eq!(cloned.max_execution_time, config.max_execution_time);
    }

    #[test]
    fn test_wasm_config_debug() {
        let config = WasmConfig::default();
        let debug_str = format!("{:?}", config);
        assert!(debug_str.contains("WasmConfig"));
        assert!(debug_str.contains("cache_dir"));
        assert!(debug_str.contains("enable_epochs"));
    }
}

// =============================================================================
// Container Stats Tests (WASM-specific behavior)
// =============================================================================

mod container_stats_tests {
    use super::*;

    #[tokio::test]
    async fn test_wasm_stats_returns_stub_values() {
        let cache_dir = create_test_cache_dir();
        let config = create_test_config(&cache_dir);
        let runtime = Arc::new(
            WasmRuntime::new(config)
                .await
                .expect("Failed to create runtime"),
        );

        let wasm_bytes = compile_wat(MINIMAL_P1_MODULE);
        let image = write_wasm_to_cache(&cache_dir, "test:stats", &wasm_bytes);

        let id = unique_container_id("stats");
        let spec = create_wasm_spec(&image);
        let _guard = InstanceGuard::new(runtime.clone(), id.clone());

        runtime.create_container(&id, &spec).await.unwrap();

        // Get stats before running
        let stats = runtime
            .get_container_stats(&id)
            .await
            .expect("Should get stats");

        // WASM doesn't have cgroups, so stats are stub values
        assert_eq!(stats.cpu_usage_usec, 0);
        assert_eq!(stats.memory_bytes, 0);
        assert_eq!(stats.memory_limit, u64::MAX);
    }

    #[tokio::test]
    async fn test_wasm_stats_after_completion() {
        let cache_dir = create_test_cache_dir();
        let config = create_test_config(&cache_dir);
        let runtime = Arc::new(
            WasmRuntime::new(config)
                .await
                .expect("Failed to create runtime"),
        );

        let wasm_bytes = compile_wat(MINIMAL_P1_MODULE);
        let image = write_wasm_to_cache(&cache_dir, "test:stats-after", &wasm_bytes);

        let id = unique_container_id("stats-after");
        let spec = create_wasm_spec(&image);
        let _guard = InstanceGuard::new(runtime.clone(), id.clone());

        runtime.create_container(&id, &spec).await.unwrap();
        runtime.start_container(&id).await.unwrap();
        runtime.wait_container(&id).await.unwrap();

        // Stats should still work after completion
        let stats = runtime
            .get_container_stats(&id)
            .await
            .expect("Should get stats after completion");

        assert_eq!(stats.cpu_usage_usec, 0);
        assert_eq!(stats.memory_bytes, 0);
    }
}

// =============================================================================
// Logs Tests
// =============================================================================

mod logs_tests {
    use super::*;

    #[tokio::test]
    async fn test_get_logs_before_start() {
        let cache_dir = create_test_cache_dir();
        let config = create_test_config(&cache_dir);
        let runtime = Arc::new(
            WasmRuntime::new(config)
                .await
                .expect("Failed to create runtime"),
        );

        let wasm_bytes = compile_wat(MINIMAL_P1_MODULE);
        let image = write_wasm_to_cache(&cache_dir, "test:logs-before", &wasm_bytes);

        let id = unique_container_id("logs-before");
        let spec = create_wasm_spec(&image);
        let _guard = InstanceGuard::new(runtime.clone(), id.clone());

        runtime.create_container(&id, &spec).await.unwrap();

        // Logs should be accessible even before start
        let logs = runtime
            .container_logs(&id, 100)
            .await
            .expect("Should get logs");
        // Logs will be empty initially
        assert!(logs.is_empty() || logs.trim().is_empty());
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

        let wasm_bytes = compile_wat(MINIMAL_P1_MODULE);
        let image = write_wasm_to_cache(&cache_dir, "test:log-lines", &wasm_bytes);

        let id = unique_container_id("log-lines");
        let spec = create_wasm_spec(&image);
        let _guard = InstanceGuard::new(runtime.clone(), id.clone());

        runtime.create_container(&id, &spec).await.unwrap();

        let lines = runtime.get_logs(&id).await.expect("Should get log lines");
        assert!(
            lines.is_empty()
                || lines
                    .iter()
                    .all(|l| l.starts_with("[stdout]") || l.starts_with("[stderr]"))
        );
    }

    #[tokio::test]
    async fn test_logs_not_found() {
        let cache_dir = create_test_cache_dir();
        let config = create_test_config(&cache_dir);
        let runtime = WasmRuntime::new(config)
            .await
            .expect("Failed to create runtime");

        let id = ContainerId {
            service: "ghost".to_string(),
            replica: 1,
        };

        let logs_result = runtime.container_logs(&id, 100).await;
        assert!(
            logs_result.is_err(),
            "Logs for nonexistent container should fail"
        );
    }
}

// =============================================================================
// Concurrent Execution Tests
// =============================================================================

mod concurrent_tests {
    use super::*;

    #[tokio::test]
    async fn test_multiple_instances_concurrent() {
        let cache_dir = create_test_cache_dir();
        let config = create_test_config(&cache_dir);
        let runtime = Arc::new(
            WasmRuntime::new(config)
                .await
                .expect("Failed to create runtime"),
        );

        let wasm_bytes = compile_wat(MINIMAL_P1_MODULE);
        let image = write_wasm_to_cache(&cache_dir, "test:concurrent", &wasm_bytes);

        // Create multiple instances
        let mut handles = Vec::new();
        let mut guards = Vec::new();

        for i in 0..5 {
            let runtime = runtime.clone();
            let image = image.clone();

            let id = ContainerId {
                service: format!("concurrent-{}-{}", i, std::process::id()),
                replica: 1,
            };
            guards.push(InstanceGuard::new(runtime.clone(), id.clone()));

            handles.push(tokio::spawn(async move {
                let spec = create_wasm_spec(&image);

                runtime.create_container(&id, &spec).await?;
                runtime.start_container(&id).await?;
                let exit_code = runtime.wait_container(&id).await?;
                runtime.remove_container(&id).await?;

                Ok::<i32, zlayer_agent::error::AgentError>(exit_code)
            }));
        }

        // Wait for all to complete
        for handle in handles {
            let result = handle.await.unwrap();
            assert!(result.is_ok(), "Concurrent instance failed: {:?}", result);
            assert_eq!(result.unwrap(), 0);
        }

        // Guards will clean up in drop
        drop(guards);
    }

    #[tokio::test]
    async fn test_sequential_reuse_of_service_name() {
        let cache_dir = create_test_cache_dir();
        let config = create_test_config(&cache_dir);
        let runtime = Arc::new(
            WasmRuntime::new(config)
                .await
                .expect("Failed to create runtime"),
        );

        let wasm_bytes = compile_wat(MINIMAL_P1_MODULE);
        let image = write_wasm_to_cache(&cache_dir, "test:sequential", &wasm_bytes);

        // Use the same service name multiple times sequentially
        let service_name = format!("sequential-{}", std::process::id());

        for i in 0..3 {
            let id = ContainerId {
                service: service_name.clone(),
                replica: i,
            };
            let spec = create_wasm_spec(&image);

            runtime.create_container(&id, &spec).await.unwrap();
            runtime.start_container(&id).await.unwrap();
            let exit_code = runtime.wait_container(&id).await.unwrap();
            assert_eq!(exit_code, 0, "Iteration {} failed", i);
            runtime.remove_container(&id).await.unwrap();
        }
    }
}
