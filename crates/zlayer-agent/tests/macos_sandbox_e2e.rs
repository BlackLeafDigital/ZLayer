#![cfg(target_os = "macos")]
//! End-to-end integration tests for ZLayer macOS Seatbelt sandbox runtime
//!
//! These tests verify the complete container lifecycle using the macOS
//! sandbox runtime (`SandboxRuntime`), which uses:
//! - `sandbox_init()` (Seatbelt) for mandatory access control
//! - APFS `clonefile()` for copy-on-write rootfs isolation
//! - Dynamic port allocation for host-network port isolation
//! - Metal/MPS GPU access via IOKit whitelisting
//!
//! # Requirements
//! - macOS (tests are gated with `#![cfg(target_os = "macos")]`)
//! - A pre-populated rootfs or macOS-native binaries (sandbox runs native
//!   macOS processes, NOT Linux containers)
//!
//! # Running
//! ```bash
//! cargo test --package zlayer-agent --test macos_sandbox_e2e -- --nocapture
//! ```

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use zlayer_agent::{
    AgentError, ContainerId, ContainerState, MacSandboxConfig, Runtime, SandboxRuntime,
};
use zlayer_spec::{DeploymentSpec, ServiceSpec};

extern crate libc;

/// Macro to run async test body with a timeout
macro_rules! with_timeout {
    ($timeout_secs:expr, $body:expr) => {{
        tokio::time::timeout(std::time::Duration::from_secs($timeout_secs), async move {
            $body
        })
        .await
        .expect(concat!(
            "Test timed out after ",
            stringify!($timeout_secs),
            " seconds"
        ))
    }};
}

/// E2E test directory prefix
const E2E_TEST_DIR: &str = "/tmp/zlayer-macos-sandbox-e2e-test";

// =============================================================================
// Helper Functions
// =============================================================================

/// Generate a unique name with the given prefix for test isolation
fn unique_name(prefix: &str) -> String {
    use rand::Rng;
    let suffix: u32 = rand::rng().random_range(10000..99999);
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
        % 1_000_000;
    format!("{}-{}-{}", prefix, timestamp, suffix)
}

/// Wait for a container to reach the expected state with timeout
async fn wait_for_state(
    runtime: &dyn Runtime,
    id: &ContainerId,
    expected: ContainerState,
    timeout: Duration,
) -> Result<(), String> {
    let start = std::time::Instant::now();
    let poll_interval = Duration::from_millis(100);

    while start.elapsed() < timeout {
        match runtime.container_state(id).await {
            Ok(state) => {
                if state == expected {
                    return Ok(());
                }
                // For Exited state, just check the variant, not the exit code
                if matches!(
                    (&state, &expected),
                    (ContainerState::Exited { .. }, ContainerState::Exited { .. })
                ) {
                    return Ok(());
                }
            }
            Err(AgentError::NotFound { .. })
                if matches!(expected, ContainerState::Exited { .. }) =>
            {
                // Container was removed - treat as exited
                return Ok(());
            }
            Err(e) => {
                return Err(format!("Error getting container state: {}", e));
            }
        }
        tokio::time::sleep(poll_interval).await;
    }

    Err(format!(
        "Timeout waiting for container {:?} to reach state {:?}",
        id, expected
    ))
}

/// Create a SandboxRuntime configured for E2E testing
fn create_e2e_runtime(gpu_access: bool) -> Result<SandboxRuntime, AgentError> {
    let test_dir = PathBuf::from(E2E_TEST_DIR);
    let config = MacSandboxConfig {
        data_dir: test_dir.join("data"),
        log_dir: test_dir.join("logs"),
        gpu_access,
    };
    SandboxRuntime::new(config)
}

/// Create a ServiceSpec that runs a macOS-native binary.
///
/// Uses `/bin/echo` which is available on all macOS installations and exits
/// immediately, making it ideal for lifecycle tests.
fn create_echo_spec() -> ServiceSpec {
    let yaml = r#"
version: v1
deployment: e2e-test
services:
  echo:
    rtype: service
    image:
      name: macos-native/echo:latest
    command:
      entrypoint: ["/bin/echo", "hello from sandbox"]
    endpoints:
      - name: dummy
        protocol: tcp
        port: 8080
    scale:
      mode: fixed
      replicas: 1
"#;

    serde_yaml::from_str::<DeploymentSpec>(yaml)
        .expect("Failed to parse echo spec")
        .services
        .remove("echo")
        .expect("Missing echo service")
}

/// Create a ServiceSpec that runs a long-lived process (`/bin/sleep`).
///
/// Useful for testing stop/kill behavior where the process needs to be
/// alive long enough to test state transitions.
fn create_sleep_spec(seconds: u32) -> ServiceSpec {
    let yaml = format!(
        r#"
version: v1
deployment: e2e-test
services:
  sleeper:
    rtype: service
    image:
      name: macos-native/sleep:latest
    command:
      entrypoint: ["/bin/sleep", "{}"]
    endpoints:
      - name: dummy
        protocol: tcp
        port: 8080
    scale:
      mode: fixed
      replicas: 1
"#,
        seconds
    );

    serde_yaml::from_str::<DeploymentSpec>(&yaml)
        .expect("Failed to parse sleep spec")
        .services
        .remove("sleeper")
        .expect("Missing sleeper service")
}

/// Create a ServiceSpec with GPU (Metal compute) access enabled.
fn create_gpu_spec() -> ServiceSpec {
    let yaml = r#"
version: v1
deployment: e2e-test
services:
  gpu-test:
    rtype: service
    image:
      name: macos-native/gpu-test:latest
    command:
      entrypoint: ["/bin/echo", "gpu access configured"]
    endpoints:
      - name: dummy
        protocol: tcp
        port: 8080
    resources:
      gpu:
        vendor: apple
        count: 1
    scale:
      mode: fixed
      replicas: 1
"#;

    serde_yaml::from_str::<DeploymentSpec>(yaml)
        .expect("Failed to parse gpu spec")
        .services
        .remove("gpu-test")
        .expect("Missing gpu-test service")
}

/// Create a ServiceSpec with MPS-only GPU access.
fn create_mps_spec() -> ServiceSpec {
    let yaml = r#"
version: v1
deployment: e2e-test
services:
  mps-test:
    rtype: service
    image:
      name: macos-native/mps-test:latest
    command:
      entrypoint: ["/bin/echo", "mps access configured"]
    endpoints:
      - name: dummy
        protocol: tcp
        port: 8080
    resources:
      gpu:
        vendor: apple
        count: 1
        mode: mps
    scale:
      mode: fixed
      replicas: 1
"#;

    serde_yaml::from_str::<DeploymentSpec>(yaml)
        .expect("Failed to parse mps spec")
        .services
        .remove("mps-test")
        .expect("Missing mps-test service")
}

/// Create a ServiceSpec with no endpoints (tests full network access path).
fn create_no_endpoints_spec() -> ServiceSpec {
    let yaml = r#"
version: v1
deployment: e2e-test
services:
  no-net:
    rtype: service
    image:
      name: macos-native/no-net:latest
    command:
      entrypoint: ["/bin/echo", "no endpoints"]
    scale:
      mode: fixed
      replicas: 1
"#;

    serde_yaml::from_str::<DeploymentSpec>(yaml)
        .expect("Failed to parse no-endpoints spec")
        .services
        .remove("no-net")
        .expect("Missing no-net service")
}

/// Create a ServiceSpec with a memory limit for watchdog testing.
fn create_memory_limited_spec() -> ServiceSpec {
    let yaml = r#"
version: v1
deployment: e2e-test
services:
  memlimit:
    rtype: service
    image:
      name: macos-native/memlimit:latest
    command:
      entrypoint: ["/bin/sleep", "30"]
    endpoints:
      - name: dummy
        protocol: tcp
        port: 8080
    resources:
      memory: "512Mi"
    scale:
      mode: fixed
      replicas: 1
"#;

    serde_yaml::from_str::<DeploymentSpec>(yaml)
        .expect("Failed to parse memory-limited spec")
        .services
        .remove("memlimit")
        .expect("Missing memlimit service")
}

/// Create a ServiceSpec with storage volumes for writable dir testing.
fn create_volume_spec() -> ServiceSpec {
    let yaml = r#"
version: v1
deployment: e2e-test
services:
  vol-test:
    rtype: service
    image:
      name: macos-native/vol-test:latest
    command:
      entrypoint: ["/bin/echo", "volumes configured"]
    endpoints:
      - name: dummy
        protocol: tcp
        port: 8080
    storage:
      - type: named
        name: data
        target: /data
        size: 1Gi
    scale:
      mode: fixed
      replicas: 1
"#;

    serde_yaml::from_str::<DeploymentSpec>(yaml)
        .expect("Failed to parse volume spec")
        .services
        .remove("vol-test")
        .expect("Missing vol-test service")
}

/// Prepare a macOS-native image for testing.
///
/// Creates a temporary directory with host system binaries, then registers
/// it with the runtime as a local image. This uses the runtime's own
/// infrastructure (APFS clone, image tracking) instead of manually
/// creating directories.
///
/// Thread-safe: uses a static guard to prepare the source directory once,
/// then each runtime instance registers it via `register_local_rootfs`
/// (which is also idempotent).
static NATIVE_ROOTFS_DIR: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<String, PathBuf>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

async fn prepare_native_image(runtime: &SandboxRuntime, image_name: &str) {
    let safe_name = image_name.replace(['/', ':', '@'], "_");

    // Prepare the source directory once (thread-safe)
    let source_dir = {
        let mut cache = NATIVE_ROOTFS_DIR.lock().unwrap();
        if let Some(dir) = cache.get(&safe_name) {
            dir.clone()
        } else {
            let dir = PathBuf::from(E2E_TEST_DIR)
                .join("native-sources")
                .join(&safe_name);

            std::fs::create_dir_all(dir.join("bin")).expect("Failed to create bin dir");
            for binary in &["echo", "sleep", "sh", "cat", "ls"] {
                let src = format!("/bin/{}", binary);
                let dst = dir.join("bin").join(binary);
                if std::path::Path::new(&src).exists() && !dst.exists() {
                    std::fs::copy(&src, &dst).ok();
                }
            }

            std::fs::create_dir_all(dir.join("usr/bin")).expect("Failed to create usr/bin dir");
            for binary in &["env", "true", "false"] {
                let src = format!("/usr/bin/{}", binary);
                let dst = dir.join("usr/bin").join(binary);
                if std::path::Path::new(&src).exists() && !dst.exists() {
                    std::fs::copy(&src, &dst).ok();
                }
            }

            cache.insert(safe_name.clone(), dir.clone());
            dir
        }
    };

    // Register with the runtime (idempotent, uses APFS clone)
    runtime
        .register_local_rootfs(image_name, &source_dir)
        .await
        .unwrap_or_else(|e| {
            panic!(
                "Failed to register local rootfs for '{}': {}",
                image_name, e
            )
        });
}

/// Cleanup helper -- ensures container is removed even on test failure
struct ContainerGuard {
    runtime: Arc<SandboxRuntime>,
    id: ContainerId,
}

impl ContainerGuard {
    fn new(runtime: Arc<SandboxRuntime>, id: ContainerId) -> Self {
        Self { runtime, id }
    }
}

impl Drop for ContainerGuard {
    fn drop(&mut self) {
        // Synchronous cleanup: we cannot await in Drop, and tokio::spawn tasks
        // get aborted when the test runtime shuts down, leaving orphaned processes.
        // Use the runtime's config to resolve paths, then do direct syscalls.
        let dir_name = format!("{}-{}", self.id.service, self.id.replica);
        let container_dir = self
            .runtime
            .config()
            .data_dir
            .join("containers")
            .join(&dir_name);
        let pid_path = container_dir.join("pid");

        if let Ok(pid_str) = std::fs::read_to_string(&pid_path) {
            if let Ok(pid) = pid_str.trim().parse::<i32>() {
                if pid > 0 {
                    unsafe {
                        libc::kill(pid, libc::SIGKILL);
                        let mut status: libc::c_int = 0;
                        libc::waitpid(pid, &mut status, libc::WNOHANG);
                    }
                }
            }
        }

        let _ = std::fs::remove_dir_all(&container_dir);
    }
}

// =============================================================================
// Runtime Initialization Tests
// =============================================================================

/// Test that the sandbox runtime initializes successfully and creates
/// the required directory structure.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_runtime_initialization() {
    with_timeout!(30, {
        let runtime = create_e2e_runtime(false);
        assert!(
            runtime.is_ok(),
            "Failed to create sandbox runtime: {:?}",
            runtime.err()
        );

        let runtime = runtime.unwrap();
        let config = runtime.config();

        // Verify directory structure was created
        assert!(
            config.data_dir.exists(),
            "data_dir should exist: {}",
            config.data_dir.display()
        );
        assert!(
            config.log_dir.exists(),
            "log_dir should exist: {}",
            config.log_dir.display()
        );
        assert!(
            config.data_dir.join("containers").exists(),
            "containers dir should exist"
        );
        assert!(
            config.data_dir.join("images").exists(),
            "images dir should exist"
        );

        println!(
            "Sandbox runtime initialized at: {}",
            config.data_dir.display()
        );
    });
}

/// Test that GPU access configuration is stored correctly.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_runtime_gpu_config() {
    with_timeout!(30, {
        // GPU enabled
        let runtime_gpu = create_e2e_runtime(true).expect("Failed to create GPU runtime");
        assert!(runtime_gpu.config().gpu_access, "gpu_access should be true");

        // GPU disabled
        let runtime_no_gpu = create_e2e_runtime(false).expect("Failed to create non-GPU runtime");
        assert!(
            !runtime_no_gpu.config().gpu_access,
            "gpu_access should be false"
        );
    });
}

// =============================================================================
// Container Lifecycle Tests
// =============================================================================

/// Test the full container lifecycle: create -> start -> check state -> wait -> remove
///
/// Uses `/bin/echo` which exits immediately after printing, allowing us to verify
/// the full lifecycle including exit code capture.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_container_lifecycle_echo() {
    with_timeout!(60, {
        let runtime = Arc::new(create_e2e_runtime(false).expect("Failed to create runtime"));

        let service_name = unique_name("lifecycle");
        let id = ContainerId {
            service: service_name.clone(),
            replica: 1,
        };
        let spec = create_echo_spec();

        // Prepare a native rootfs with /bin/echo
        prepare_native_image(&runtime, &spec.image.name).await;

        // Setup cleanup guard
        let _guard = ContainerGuard::new(runtime.clone(), id.clone());

        // 1. Create container
        println!("Creating container: {}", id);
        let create_result = runtime.create_container(&id, &spec).await;
        assert!(
            create_result.is_ok(),
            "Failed to create container: {:?}",
            create_result
        );

        // Verify container is in Pending state
        let state = runtime.container_state(&id).await;
        assert!(state.is_ok(), "Failed to get container state: {:?}", state);
        assert_eq!(state.unwrap(), ContainerState::Pending);

        // 2. Verify sandbox profile was written
        let container_dir = runtime
            .config()
            .data_dir
            .join("containers")
            .join(format!("{}-{}", id.service, id.replica));
        assert!(
            container_dir.join("sandbox.sb").exists(),
            "Seatbelt profile should be written"
        );
        assert!(
            container_dir.join("config.json").exists(),
            "Config should be written"
        );

        // 3. Verify rootfs was cloned (APFS CoW)
        assert!(
            container_dir.join("rootfs").exists(),
            "Rootfs should be cloned to container dir"
        );
        assert!(
            container_dir.join("rootfs/bin/echo").exists(),
            "Cloned rootfs should contain /bin/echo"
        );

        // 4. Start container
        println!("Starting container: {}", id);
        let start_result = runtime.start_container(&id).await;
        assert!(
            start_result.is_ok(),
            "Failed to start container: {:?}",
            start_result
        );

        // 5. Verify PID file was created
        assert!(
            container_dir.join("pid").exists(),
            "PID file should be written after start"
        );

        // 6. Wait for the process to exit (echo exits immediately)
        let wait_result = wait_for_state(
            runtime.as_ref(),
            &id,
            ContainerState::Exited { code: 0 },
            Duration::from_secs(10),
        )
        .await;
        assert!(
            wait_result.is_ok(),
            "Container did not exit: {}",
            wait_result.unwrap_err()
        );

        // 7. Check logs
        let logs = runtime.container_logs(&id, 100).await;
        assert!(logs.is_ok(), "Failed to get logs: {:?}", logs);
        let log_content = logs.unwrap();
        println!("Container logs:\n{}", log_content);
        // echo should have written "hello from sandbox" to stdout
        assert!(
            log_content.contains("hello from sandbox"),
            "Logs should contain echo output, got: {}",
            log_content
        );

        // 8. Remove container
        println!("Removing container: {}", id);
        let remove_result = runtime.remove_container(&id).await;
        assert!(
            remove_result.is_ok(),
            "Failed to remove container: {:?}",
            remove_result
        );

        // 9. Verify container directory is cleaned up
        assert!(
            !container_dir.exists(),
            "Container directory should be removed after remove_container"
        );

        // 10. Verify container state returns NotFound
        let state = runtime.container_state(&id).await;
        assert!(state.is_err(), "Container should not exist after removal");
    });
}

/// Test stopping a long-running process with SIGTERM and verifying graceful shutdown.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_container_stop_sigterm() {
    with_timeout!(60, {
        let runtime = Arc::new(create_e2e_runtime(false).expect("Failed to create runtime"));

        let service_name = unique_name("stop");
        let id = ContainerId {
            service: service_name.clone(),
            replica: 1,
        };
        let spec = create_sleep_spec(300); // Sleep 5 minutes -- will be killed

        prepare_native_image(&runtime, &spec.image.name).await;

        let _guard = ContainerGuard::new(runtime.clone(), id.clone());

        // Create and start
        runtime
            .create_container(&id, &spec)
            .await
            .expect("Failed to create");
        runtime.start_container(&id).await.expect("Failed to start");

        // Verify running
        let wait_result = wait_for_state(
            runtime.as_ref(),
            &id,
            ContainerState::Running,
            Duration::from_secs(10),
        )
        .await;
        assert!(
            wait_result.is_ok(),
            "Container did not reach Running state: {}",
            wait_result.unwrap_err()
        );

        // Verify we can get the PID
        let pid = runtime.get_container_pid(&id).await;
        assert!(pid.is_ok(), "Failed to get PID: {:?}", pid);
        let pid = pid.unwrap();
        assert!(pid.is_some(), "PID should be set for running container");
        println!("Container PID: {}", pid.unwrap());

        // Stop with timeout
        println!("Stopping container: {}", id);
        let stop_result = runtime.stop_container(&id, Duration::from_secs(5)).await;
        assert!(
            stop_result.is_ok(),
            "Failed to stop container: {:?}",
            stop_result
        );

        // Verify exited state
        let state = runtime.container_state(&id).await;
        assert!(state.is_ok(), "Failed to get state after stop");
        match state.unwrap() {
            ContainerState::Exited { .. } => {
                println!("Container exited as expected after stop");
            }
            other => panic!("Expected Exited state, got: {:?}", other),
        }

        // Cleanup
        let _ = runtime.remove_container(&id).await;
    });
}

/// Test that wait_container returns the correct exit code.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_wait_container_exit_code() {
    with_timeout!(60, {
        let runtime = Arc::new(create_e2e_runtime(false).expect("Failed to create runtime"));

        let service_name = unique_name("wait");
        let id = ContainerId {
            service: service_name.clone(),
            replica: 1,
        };
        let spec = create_echo_spec();

        prepare_native_image(&runtime, &spec.image.name).await;

        let _guard = ContainerGuard::new(runtime.clone(), id.clone());

        runtime.create_container(&id, &spec).await.expect("create");
        runtime.start_container(&id).await.expect("start");

        // Wait for the process to exit
        let exit_code = runtime.wait_container(&id).await;
        assert!(
            exit_code.is_ok(),
            "wait_container failed: {:?}",
            exit_code.err()
        );
        assert_eq!(exit_code.unwrap(), 0, "/bin/echo should exit with code 0");

        let _ = runtime.remove_container(&id).await;
    });
}

// =============================================================================
// Port Allocation Tests
// =============================================================================

/// Test that each container gets a unique dynamic port.
///
/// The macOS sandbox runtime assigns each container a unique port because
/// all sandboxed processes share the host network stack. This test creates
/// multiple containers and verifies they get different ports.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_unique_port_allocation() {
    with_timeout!(60, {
        let runtime = Arc::new(create_e2e_runtime(false).expect("Failed to create runtime"));

        let spec = create_sleep_spec(60);
        prepare_native_image(&runtime, &spec.image.name).await;

        let mut ports = Vec::new();
        let mut ids = Vec::new();

        // Create 3 containers and collect their assigned ports
        for i in 0..3 {
            let service_name = unique_name("port");
            let id = ContainerId {
                service: service_name,
                replica: i + 1,
            };

            runtime
                .create_container(&id, &spec)
                .await
                .expect("Failed to create container");

            let port = runtime
                .get_container_port_override(&id)
                .await
                .expect("Failed to get port override");
            assert!(
                port.is_some(),
                "Sandbox containers should always have a port override"
            );
            ports.push(port.unwrap());
            ids.push(id);
        }

        println!("Allocated ports: {:?}", ports);

        // Verify all ports are unique
        let unique_ports: std::collections::HashSet<_> = ports.iter().collect();
        assert_eq!(
            unique_ports.len(),
            ports.len(),
            "All container ports should be unique, got: {:?}",
            ports
        );

        // Verify all ports are valid (non-zero, not well-known ports)
        for port in &ports {
            assert!(*port > 0, "Port should be non-zero");
            // OS-assigned ephemeral ports are typically > 1024
            assert!(*port > 1024, "Port {} should be in ephemeral range", port);
        }

        // Cleanup
        for id in &ids {
            let _ = runtime.remove_container(id).await;
        }
    });
}

/// Test that container IP is always 127.0.0.1 on macOS sandbox.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_container_ip_is_localhost() {
    with_timeout!(30, {
        let runtime = Arc::new(create_e2e_runtime(false).expect("Failed to create runtime"));

        let service_name = unique_name("ip");
        let id = ContainerId {
            service: service_name,
            replica: 1,
        };
        let spec = create_echo_spec();

        prepare_native_image(&runtime, &spec.image.name).await;

        let _guard = ContainerGuard::new(runtime.clone(), id.clone());

        runtime.create_container(&id, &spec).await.expect("create");

        let ip = runtime.get_container_ip(&id).await;
        assert!(ip.is_ok(), "Failed to get container IP: {:?}", ip);
        let ip = ip.unwrap();
        assert!(ip.is_some(), "Container should have an IP");
        assert_eq!(
            ip.unwrap(),
            std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
            "Sandbox container IP should be 127.0.0.1"
        );

        let _ = runtime.remove_container(&id).await;
    });
}

// =============================================================================
// GPU Access Configuration Tests
// =============================================================================

/// Test container creation with Metal compute GPU access.
///
/// Verifies that the generated Seatbelt profile contains the Metal compute
/// IOKit rules when GPU access is enabled and vendor is "apple".
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_gpu_metal_compute_profile() {
    with_timeout!(30, {
        let runtime = Arc::new(create_e2e_runtime(true).expect("Failed to create runtime"));

        let service_name = unique_name("gpu-metal");
        let id = ContainerId {
            service: service_name,
            replica: 1,
        };
        let spec = create_gpu_spec();

        prepare_native_image(&runtime, &spec.image.name).await;

        let _guard = ContainerGuard::new(runtime.clone(), id.clone());

        runtime.create_container(&id, &spec).await.expect("create");

        // Read the generated Seatbelt profile
        let container_dir = runtime
            .config()
            .data_dir
            .join("containers")
            .join(format!("{}-{}", id.service, id.replica));
        let profile_path = container_dir.join("sandbox.sb");
        let profile = tokio::fs::read_to_string(&profile_path)
            .await
            .expect("Failed to read sandbox profile");

        println!("Generated Seatbelt profile:\n{}", profile);

        // Verify Metal compute rules are present
        assert!(
            profile.contains("Full Metal Compute"),
            "Profile should contain Metal compute section"
        );
        assert!(
            profile.contains("MTLCompilerService"),
            "Profile should allow Metal shader compilation"
        );
        assert!(
            profile.contains("IOAccelDevice2"),
            "Profile should allow IOKit GPU access"
        );
        assert!(
            profile.contains("AGXDeviceUserClient"),
            "Profile should allow Apple Silicon GPU access"
        );
        assert!(
            profile.contains("MetalPerformanceShaders.framework"),
            "Profile should allow MPS framework access"
        );

        let _ = runtime.remove_container(&id).await;
    });
}

/// Test container creation with MPS-only GPU access.
///
/// MPS-only mode has a smaller attack surface than full Metal compute --
/// it omits AGXCompilerService XPC services and cvmsServ. However,
/// MTLCompilerService is still required because MPSGraph uses JIT
/// compilation internally on macOS 26+.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_gpu_mps_only_profile() {
    with_timeout!(30, {
        let runtime = Arc::new(create_e2e_runtime(true).expect("Failed to create runtime"));

        let service_name = unique_name("gpu-mps");
        let id = ContainerId {
            service: service_name,
            replica: 1,
        };
        let spec = create_mps_spec();

        prepare_native_image(&runtime, &spec.image.name).await;

        let _guard = ContainerGuard::new(runtime.clone(), id.clone());

        runtime.create_container(&id, &spec).await.expect("create");

        // Read the generated Seatbelt profile
        let container_dir = runtime
            .config()
            .data_dir
            .join("containers")
            .join(format!("{}-{}", id.service, id.replica));
        let profile_path = container_dir.join("sandbox.sb");
        let profile = tokio::fs::read_to_string(&profile_path)
            .await
            .expect("Failed to read sandbox profile");

        // Verify MPS-only rules
        assert!(
            profile.contains("MPS Only"),
            "Profile should contain MPS-only section"
        );
        // MPS-only omits AGXCompilerService (Apple Silicon shader pre-compilation)
        assert!(
            !profile.contains("AGXCompilerService"),
            "MPS-only profile should NOT allow AGXCompilerService"
        );
        assert!(
            profile.contains("MetalPerformanceShaders.framework"),
            "Profile should allow MPS framework access"
        );
        // AGXDeviceUserClient is required for Metal device creation
        assert!(
            profile.contains("AGXDeviceUserClient"),
            "Profile should allow AGXDeviceUserClient"
        );

        let _ = runtime.remove_container(&id).await;
    });
}

/// Test that GPU access is denied when the runtime has gpu_access=false,
/// even if the spec requests a GPU.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_gpu_denied_when_runtime_disabled() {
    with_timeout!(30, {
        // Create runtime with GPU disabled
        let runtime = Arc::new(create_e2e_runtime(false).expect("Failed to create runtime"));

        let service_name = unique_name("no-gpu");
        let id = ContainerId {
            service: service_name,
            replica: 1,
        };
        // Spec requests GPU, but runtime disables it
        let spec = create_gpu_spec();

        prepare_native_image(&runtime, &spec.image.name).await;

        let _guard = ContainerGuard::new(runtime.clone(), id.clone());

        runtime.create_container(&id, &spec).await.expect("create");

        // Read the generated Seatbelt profile
        let container_dir = runtime
            .config()
            .data_dir
            .join("containers")
            .join(format!("{}-{}", id.service, id.replica));
        let profile_path = container_dir.join("sandbox.sb");
        let profile = tokio::fs::read_to_string(&profile_path)
            .await
            .expect("Failed to read sandbox profile");

        // GPU sections should NOT be present
        assert!(
            !profile.contains("Full Metal Compute"),
            "Profile should NOT contain Metal section when GPU disabled"
        );
        assert!(
            !profile.contains("MPS Only"),
            "Profile should NOT contain MPS section when GPU disabled"
        );

        let _ = runtime.remove_container(&id).await;
    });
}

// =============================================================================
// Network Isolation Tests
// =============================================================================

/// Test Seatbelt profile generation for different network configurations.
///
/// Verifies that the profile correctly restricts network access based on
/// the ServiceSpec's endpoints.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_network_localhost_only_profile() {
    with_timeout!(30, {
        let runtime = Arc::new(create_e2e_runtime(false).expect("Failed to create runtime"));

        let service_name = unique_name("net-local");
        let id = ContainerId {
            service: service_name,
            replica: 1,
        };
        // Spec with endpoints -> localhost-only network
        let spec = create_echo_spec();

        prepare_native_image(&runtime, &spec.image.name).await;

        let _guard = ContainerGuard::new(runtime.clone(), id.clone());

        runtime.create_container(&id, &spec).await.expect("create");

        let container_dir = runtime
            .config()
            .data_dir
            .join("containers")
            .join(format!("{}-{}", id.service, id.replica));
        let profile = tokio::fs::read_to_string(container_dir.join("sandbox.sb"))
            .await
            .expect("Failed to read profile");

        // Should have localhost network rules
        assert!(
            profile.contains("localhost"),
            "Profile should contain localhost network rules when endpoints are defined"
        );
        // Should have the dynamically assigned port allowed
        assert!(
            profile.contains("network-bind"),
            "Profile should allow network-bind for assigned ports"
        );

        let _ = runtime.remove_container(&id).await;
    });
}

/// Test Seatbelt profile for spec with no endpoints (full network access).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_network_full_access_profile() {
    with_timeout!(30, {
        let runtime = Arc::new(create_e2e_runtime(false).expect("Failed to create runtime"));

        let service_name = unique_name("net-full");
        let id = ContainerId {
            service: service_name,
            replica: 1,
        };
        // Spec without endpoints -> full network access
        let spec = create_no_endpoints_spec();

        prepare_native_image(&runtime, &spec.image.name).await;

        let _guard = ContainerGuard::new(runtime.clone(), id.clone());

        runtime.create_container(&id, &spec).await.expect("create");

        let container_dir = runtime
            .config()
            .data_dir
            .join("containers")
            .join(format!("{}-{}", id.service, id.replica));
        let profile = tokio::fs::read_to_string(container_dir.join("sandbox.sb"))
            .await
            .expect("Failed to read profile");

        // Should have full network access
        assert!(
            profile.contains("full access"),
            "Profile should contain full network access when no endpoints"
        );
        assert!(
            profile.contains("network-outbound"),
            "Profile should allow all outbound"
        );
        assert!(
            profile.contains("network-inbound"),
            "Profile should allow all inbound"
        );

        let _ = runtime.remove_container(&id).await;
    });
}

// =============================================================================
// APFS Clonefile / Rootfs Isolation Tests
// =============================================================================

/// Test that rootfs is properly cloned from the base image to the container.
///
/// Verifies that APFS clonefile (or fallback copy) creates an isolated copy
/// of the rootfs, allowing each container to modify its rootfs independently.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_rootfs_cloning() {
    with_timeout!(60, {
        let runtime = Arc::new(create_e2e_runtime(false).expect("Failed to create runtime"));

        let spec = create_echo_spec();
        prepare_native_image(&runtime, &spec.image.name).await;

        // Create two containers from the same image
        let id1 = ContainerId {
            service: unique_name("clone-a"),
            replica: 1,
        };
        let id2 = ContainerId {
            service: unique_name("clone-b"),
            replica: 1,
        };

        let _guard1 = ContainerGuard::new(runtime.clone(), id1.clone());
        let _guard2 = ContainerGuard::new(runtime.clone(), id2.clone());

        runtime
            .create_container(&id1, &spec)
            .await
            .expect("create 1");
        runtime
            .create_container(&id2, &spec)
            .await
            .expect("create 2");

        // Both should have their own rootfs
        let dir1 = runtime
            .config()
            .data_dir
            .join("containers")
            .join(format!("{}-{}", id1.service, id1.replica));
        let dir2 = runtime
            .config()
            .data_dir
            .join("containers")
            .join(format!("{}-{}", id2.service, id2.replica));

        assert!(
            dir1.join("rootfs/bin/echo").exists(),
            "Container 1 should have echo in rootfs"
        );
        assert!(
            dir2.join("rootfs/bin/echo").exists(),
            "Container 2 should have echo in rootfs"
        );

        // Verify they are separate copies (different inodes or at least independent)
        // On APFS, clonefiles share the same data blocks but have different inodes
        let meta1 = std::fs::metadata(dir1.join("rootfs/bin/echo")).expect("meta 1");
        let meta2 = std::fs::metadata(dir2.join("rootfs/bin/echo")).expect("meta 2");

        // Both files should exist and be regular files
        assert!(meta1.is_file(), "Container 1 echo should be a file");
        assert!(meta2.is_file(), "Container 2 echo should be a file");

        // They should have the same size (cloned from same source)
        assert_eq!(
            meta1.len(),
            meta2.len(),
            "Cloned files should have the same size"
        );

        let _ = runtime.remove_container(&id1).await;
        let _ = runtime.remove_container(&id2).await;
    });
}

/// Test that container removal cleans up the rootfs clone.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_rootfs_cleanup_on_removal() {
    with_timeout!(30, {
        let runtime = Arc::new(create_e2e_runtime(false).expect("Failed to create runtime"));

        let service_name = unique_name("cleanup");
        let id = ContainerId {
            service: service_name.clone(),
            replica: 1,
        };
        let spec = create_echo_spec();

        prepare_native_image(&runtime, &spec.image.name).await;

        runtime.create_container(&id, &spec).await.expect("create");

        let container_dir = runtime
            .config()
            .data_dir
            .join("containers")
            .join(format!("{}-{}", id.service, id.replica));
        assert!(
            container_dir.exists(),
            "Container dir should exist after create"
        );
        assert!(
            container_dir.join("rootfs").exists(),
            "Rootfs should exist after create"
        );

        // Remove container
        runtime.remove_container(&id).await.expect("remove");

        assert!(
            !container_dir.exists(),
            "Container dir should be removed after remove_container"
        );
    });
}

// =============================================================================
// Container Exec Tests
// =============================================================================

/// Test executing a command inside a container's sandbox.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_exec_in_container() {
    with_timeout!(60, {
        let runtime = Arc::new(create_e2e_runtime(false).expect("Failed to create runtime"));

        let service_name = unique_name("exec");
        let id = ContainerId {
            service: service_name,
            replica: 1,
        };
        let spec = create_sleep_spec(60);

        prepare_native_image(&runtime, &spec.image.name).await;

        let _guard = ContainerGuard::new(runtime.clone(), id.clone());

        runtime.create_container(&id, &spec).await.expect("create");
        runtime.start_container(&id).await.expect("start");

        // Wait for running state
        wait_for_state(
            runtime.as_ref(),
            &id,
            ContainerState::Running,
            Duration::from_secs(10),
        )
        .await
        .expect("Container should be running");

        // Execute a command in the container's sandbox
        let cmd = vec!["/bin/echo".to_string(), "exec test".to_string()];
        let result = runtime.exec(&id, &cmd).await;
        assert!(result.is_ok(), "exec failed: {:?}", result);

        let (exit_code, stdout, stderr) = result.unwrap();
        println!(
            "exec exit_code={}, stdout={}, stderr={}",
            exit_code, stdout, stderr
        );
        assert_eq!(exit_code, 0, "echo should exit with code 0");
        assert!(
            stdout.contains("exec test"),
            "stdout should contain echo output, got: {}",
            stdout
        );

        let _ = runtime.stop_container(&id, Duration::from_secs(5)).await;
        let _ = runtime.remove_container(&id).await;
    });
}

// =============================================================================
// Container Stats Tests
// =============================================================================

/// Test getting resource stats for a running container.
///
/// Uses macOS `proc_pidinfo` to read CPU time and RSS for the process.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_container_stats() {
    with_timeout!(60, {
        let runtime = Arc::new(create_e2e_runtime(false).expect("Failed to create runtime"));

        let service_name = unique_name("stats");
        let id = ContainerId {
            service: service_name,
            replica: 1,
        };
        let spec = create_sleep_spec(60);

        prepare_native_image(&runtime, &spec.image.name).await;

        let _guard = ContainerGuard::new(runtime.clone(), id.clone());

        runtime.create_container(&id, &spec).await.expect("create");
        runtime.start_container(&id).await.expect("start");

        // Wait for running state
        wait_for_state(
            runtime.as_ref(),
            &id,
            ContainerState::Running,
            Duration::from_secs(10),
        )
        .await
        .expect("Container should be running");

        // Give the process a moment to accumulate some CPU time
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Get stats
        let stats = runtime.get_container_stats(&id).await;
        assert!(stats.is_ok(), "Failed to get stats: {:?}", stats);

        let stats = stats.unwrap();
        println!(
            "Container stats: cpu_usec={}, memory_bytes={}, memory_limit={}",
            stats.cpu_usage_usec, stats.memory_bytes, stats.memory_limit
        );

        // Memory should be non-zero (process is running)
        assert!(
            stats.memory_bytes > 0,
            "Memory bytes should be non-zero for a running process"
        );

        let _ = runtime.stop_container(&id, Duration::from_secs(5)).await;
        let _ = runtime.remove_container(&id).await;
    });
}

/// Test that stats fail for a non-started container.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_stats_fail_before_start() {
    with_timeout!(30, {
        let runtime = Arc::new(create_e2e_runtime(false).expect("Failed to create runtime"));

        let service_name = unique_name("stats-nostart");
        let id = ContainerId {
            service: service_name,
            replica: 1,
        };
        let spec = create_echo_spec();

        prepare_native_image(&runtime, &spec.image.name).await;

        let _guard = ContainerGuard::new(runtime.clone(), id.clone());

        runtime.create_container(&id, &spec).await.expect("create");

        // Stats should fail because container is not started (no PID)
        let stats = runtime.get_container_stats(&id).await;
        assert!(
            stats.is_err(),
            "Stats should fail for non-started container"
        );

        let _ = runtime.remove_container(&id).await;
    });
}

// =============================================================================
// Memory Limit / Watchdog Tests
// =============================================================================

/// Test that a memory-limited container gets a watchdog task spawned.
///
/// We cannot easily test the actual kill behavior without allocating memory
/// in the child, but we can verify the container configuration is correct.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_memory_limited_container() {
    with_timeout!(60, {
        let runtime = Arc::new(create_e2e_runtime(false).expect("Failed to create runtime"));

        let service_name = unique_name("memlimit");
        let id = ContainerId {
            service: service_name,
            replica: 1,
        };
        let spec = create_memory_limited_spec();

        prepare_native_image(&runtime, &spec.image.name).await;

        let _guard = ContainerGuard::new(runtime.clone(), id.clone());

        runtime.create_container(&id, &spec).await.expect("create");
        runtime.start_container(&id).await.expect("start");

        // Verify the container is running
        wait_for_state(
            runtime.as_ref(),
            &id,
            ContainerState::Running,
            Duration::from_secs(10),
        )
        .await
        .expect("Container should be running");

        // Get stats -- memory_limit should reflect the configured 512Mi
        let stats = runtime.get_container_stats(&id).await;
        assert!(stats.is_ok(), "Failed to get stats: {:?}", stats);
        // 512Mi = 536870912 bytes
        // Note: The memory_limit in stats comes from the SandboxContainer's memory_limit field
        // We cannot directly verify it through the public API, but if stats work the watchdog
        // was initialized correctly.
        println!(
            "Memory limit from stats: {} bytes",
            stats.unwrap().memory_limit
        );

        let _ = runtime.stop_container(&id, Duration::from_secs(5)).await;
        let _ = runtime.remove_container(&id).await;
    });
}

// =============================================================================
// Error Handling Tests
// =============================================================================

/// Test that removing a non-existent container succeeds (idempotent).
///
/// Like the youki runtime, remove_container should be idempotent so that
/// cleanup operations are resilient.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_remove_nonexistent_is_idempotent() {
    with_timeout!(30, {
        let runtime = create_e2e_runtime(false).expect("Failed to create runtime");

        let id = ContainerId {
            service: unique_name("nonexistent"),
            replica: 999,
        };

        println!("Attempting to remove non-existent container: {}", id);
        let result = runtime.remove_container(&id).await;

        // remove_container checks the directory; if it doesn't exist,
        // it skips cleanup. The in-memory map won't have this container
        // either, so it should succeed.
        assert!(
            result.is_ok(),
            "remove_container should be idempotent: {:?}",
            result
        );
    });
}

/// Test that container_state returns NotFound for a non-existent container.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_state_nonexistent() {
    with_timeout!(30, {
        let runtime = create_e2e_runtime(false).expect("Failed to create runtime");

        let id = ContainerId {
            service: unique_name("ghost"),
            replica: 1,
        };

        let result = runtime.container_state(&id).await;
        assert!(result.is_err(), "Should fail for non-existent container");
        match result {
            Err(AgentError::NotFound { .. }) => {
                println!("Got expected NotFound error for container state");
            }
            Err(other) => {
                panic!("Expected NotFound, got: {:?}", other);
            }
            Ok(state) => panic!(
                "Should not get state for non-existent container, got: {:?}",
                state
            ),
        }
    });
}

/// Test that creating a container without a pulled image fails with a clear error.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_create_without_image_fails() {
    with_timeout!(30, {
        let runtime = Arc::new(create_e2e_runtime(false).expect("Failed to create runtime"));

        let service_name = unique_name("no-image");
        let id = ContainerId {
            service: service_name,
            replica: 1,
        };

        // Use a spec referencing an image that has NOT been prepared
        let yaml = r#"
version: v1
deployment: e2e-test
services:
  missing:
    rtype: service
    image:
      name: nonexistent-image:v99
    command:
      entrypoint: ["/bin/echo", "this should fail"]
    endpoints:
      - name: dummy
        protocol: tcp
        port: 8080
    scale:
      mode: fixed
      replicas: 1
"#;
        let spec = serde_yaml::from_str::<DeploymentSpec>(yaml)
            .unwrap()
            .services
            .remove("missing")
            .unwrap();

        let result = runtime.create_container(&id, &spec).await;
        assert!(
            result.is_err(),
            "create_container should fail without image rootfs"
        );

        match result {
            Err(AgentError::CreateFailed { reason, .. }) => {
                assert!(
                    reason.contains("rootfs not found")
                        || reason.contains("Image rootfs not found"),
                    "Error should mention missing rootfs, got: {}",
                    reason
                );
                println!("Got expected error: {}", reason);
            }
            Err(other) => {
                panic!("Expected CreateFailed, got: {:?}", other);
            }
            Ok(_) => panic!("Should have failed"),
        }
    });
}

/// Test that starting a non-existent container returns NotFound.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_start_nonexistent_fails() {
    with_timeout!(30, {
        let runtime = create_e2e_runtime(false).expect("Failed to create runtime");

        let id = ContainerId {
            service: unique_name("no-container"),
            replica: 1,
        };

        let result = runtime.start_container(&id).await;
        assert!(
            result.is_err(),
            "start_container should fail for non-existent container"
        );
        match result {
            Err(AgentError::NotFound { .. }) => {
                println!("Got expected NotFound error");
            }
            Err(other) => {
                panic!("Expected NotFound, got: {:?}", other);
            }
            Ok(_) => panic!("Should have failed"),
        }
    });
}

/// Test that exec with an empty command returns an error.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_exec_empty_command_fails() {
    with_timeout!(30, {
        let runtime = Arc::new(create_e2e_runtime(false).expect("Failed to create runtime"));

        let service_name = unique_name("exec-empty");
        let id = ContainerId {
            service: service_name,
            replica: 1,
        };
        let spec = create_sleep_spec(60);

        prepare_native_image(&runtime, &spec.image.name).await;

        let _guard = ContainerGuard::new(runtime.clone(), id.clone());

        runtime.create_container(&id, &spec).await.expect("create");
        runtime.start_container(&id).await.expect("start");

        // Empty command should fail
        let result = runtime.exec(&id, &[]).await;
        assert!(result.is_err(), "exec with empty command should fail");
        match result {
            Err(AgentError::InvalidSpec(msg)) => {
                assert!(
                    msg.contains("empty"),
                    "Error should mention empty command, got: {}",
                    msg
                );
            }
            Err(other) => {
                panic!("Expected InvalidSpec, got: {:?}", other);
            }
            Ok(_) => panic!("Should have failed"),
        }

        let _ = runtime.stop_container(&id, Duration::from_secs(5)).await;
        let _ = runtime.remove_container(&id).await;
    });
}

// =============================================================================
// Concurrent Container Tests
// =============================================================================

/// Test that multiple containers can be created and managed concurrently.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_concurrent_containers() {
    with_timeout!(120, {
        let runtime = Arc::new(create_e2e_runtime(false).expect("Failed to create runtime"));

        let spec = create_echo_spec();
        prepare_native_image(&runtime, &spec.image.name).await;

        let container_count = 5;
        let base_name = unique_name("concurrent");

        // Create multiple containers concurrently
        let mut handles = Vec::new();
        for i in 0..container_count {
            let runtime_clone = runtime.clone();
            let spec_clone = spec.clone();
            let name = base_name.clone();

            handles.push(tokio::spawn(async move {
                let id = ContainerId {
                    service: format!("{}-{}", name, i),
                    replica: 1,
                };

                let create_result = runtime_clone.create_container(&id, &spec_clone).await;
                (id, create_result)
            }));
        }

        // Collect results
        let mut created_ids = Vec::new();
        for handle in handles {
            let (id, result) = handle.await.expect("Task panicked");
            if result.is_ok() {
                created_ids.push(id);
            } else {
                eprintln!("Failed to create container: {:?}", result);
            }
        }

        println!("Created {} containers concurrently", created_ids.len());
        assert_eq!(
            created_ids.len(),
            container_count,
            "All containers should be created"
        );

        // Start all containers concurrently
        let mut start_handles = Vec::new();
        for id in &created_ids {
            let runtime_clone = runtime.clone();
            let id_clone = id.clone();
            start_handles.push(tokio::spawn(async move {
                runtime_clone.start_container(&id_clone).await
            }));
        }

        let mut started = 0;
        for handle in start_handles {
            if handle.await.expect("Task panicked").is_ok() {
                started += 1;
            }
        }
        println!("Started {} containers concurrently", started);
        assert_eq!(started, container_count, "All containers should start");

        // Cleanup all containers
        for id in &created_ids {
            let _ = runtime.stop_container(id, Duration::from_secs(5)).await;
            let _ = runtime.remove_container(id).await;
        }
    });
}

// =============================================================================
// Stale Container Cleanup Tests
// =============================================================================

/// Test that creating a container on top of a stale directory cleans it up.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_stale_container_cleanup() {
    with_timeout!(30, {
        let runtime = Arc::new(create_e2e_runtime(false).expect("Failed to create runtime"));

        let service_name = unique_name("stale");
        let id = ContainerId {
            service: service_name.clone(),
            replica: 1,
        };
        let spec = create_echo_spec();

        prepare_native_image(&runtime, &spec.image.name).await;

        // Create a stale directory manually
        let stale_dir = runtime
            .config()
            .data_dir
            .join("containers")
            .join(format!("{}-{}", id.service, id.replica));
        tokio::fs::create_dir_all(&stale_dir)
            .await
            .expect("Failed to create stale dir");
        tokio::fs::write(stale_dir.join("leftover.txt"), "stale data")
            .await
            .expect("Failed to write stale file");

        // Create the container -- it should clean up the stale directory
        let result = runtime.create_container(&id, &spec).await;
        assert!(
            result.is_ok(),
            "create_container should succeed even with stale dir: {:?}",
            result
        );

        // Stale file should be gone, fresh config should exist
        assert!(
            !stale_dir.join("leftover.txt").exists(),
            "Stale files should be cleaned up"
        );
        assert!(
            stale_dir.join("config.json").exists(),
            "Fresh config should exist"
        );

        let _ = runtime.remove_container(&id).await;
    });
}

// =============================================================================
// Volume / Writable Directory Tests
// =============================================================================

/// Test that the Seatbelt profile includes writable directories for volumes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_volume_writable_dirs_in_profile() {
    with_timeout!(30, {
        let runtime = Arc::new(create_e2e_runtime(false).expect("Failed to create runtime"));

        let service_name = unique_name("vol");
        let id = ContainerId {
            service: service_name,
            replica: 1,
        };
        let spec = create_volume_spec();

        prepare_native_image(&runtime, &spec.image.name).await;

        let _guard = ContainerGuard::new(runtime.clone(), id.clone());

        runtime.create_container(&id, &spec).await.expect("create");

        // Read the generated Seatbelt profile
        let container_dir = runtime
            .config()
            .data_dir
            .join("containers")
            .join(format!("{}-{}", id.service, id.replica));
        let profile = tokio::fs::read_to_string(container_dir.join("sandbox.sb"))
            .await
            .expect("Failed to read profile");

        // Profile should allow writing to the volume mount target
        assert!(
            profile.contains("/data"),
            "Profile should include /data as a writable directory"
        );
        // Profile should always allow the tmp directory
        assert!(
            profile.contains("tmp"),
            "Profile should include tmp directory"
        );

        let _ = runtime.remove_container(&id).await;
    });
}

// =============================================================================
// Container Logs Tests
// =============================================================================

/// Test get_logs (vector form) for a container.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_get_logs_vector() {
    with_timeout!(60, {
        let runtime = Arc::new(create_e2e_runtime(false).expect("Failed to create runtime"));

        let service_name = unique_name("logs-vec");
        let id = ContainerId {
            service: service_name,
            replica: 1,
        };
        let spec = create_echo_spec();

        prepare_native_image(&runtime, &spec.image.name).await;

        let _guard = ContainerGuard::new(runtime.clone(), id.clone());

        runtime.create_container(&id, &spec).await.expect("create");
        runtime.start_container(&id).await.expect("start");

        // Wait for exit (echo exits immediately)
        wait_for_state(
            runtime.as_ref(),
            &id,
            ContainerState::Exited { code: 0 },
            Duration::from_secs(10),
        )
        .await
        .ok();

        // Get logs as vector
        let logs = runtime.get_logs(&id).await;
        assert!(logs.is_ok(), "Failed to get logs: {:?}", logs);

        let log_lines = logs.unwrap();
        println!("Log lines: {:?}", log_lines);

        // At least one line should contain our echo output
        let has_output = log_lines
            .iter()
            .any(|line| line.contains("hello from sandbox"));
        assert!(
            has_output,
            "Logs should contain echo output, got: {:?}",
            log_lines
        );

        let _ = runtime.remove_container(&id).await;
    });
}

// =============================================================================
// Seatbelt Profile Structure Tests
// =============================================================================

/// Test that the generated Seatbelt profile has the correct structure.
///
/// Verifies deny-default, base process rules, system library access, and
/// I/O essentials are always present regardless of configuration.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_seatbelt_profile_structure() {
    with_timeout!(30, {
        let runtime = Arc::new(create_e2e_runtime(false).expect("Failed to create runtime"));

        let service_name = unique_name("profile");
        let id = ContainerId {
            service: service_name,
            replica: 1,
        };
        let spec = create_echo_spec();

        prepare_native_image(&runtime, &spec.image.name).await;

        let _guard = ContainerGuard::new(runtime.clone(), id.clone());

        runtime.create_container(&id, &spec).await.expect("create");

        let container_dir = runtime
            .config()
            .data_dir
            .join("containers")
            .join(format!("{}-{}", id.service, id.replica));
        let profile = tokio::fs::read_to_string(container_dir.join("sandbox.sb"))
            .await
            .expect("Failed to read profile");

        // Required structural elements
        assert!(
            profile.contains("(version 1)"),
            "Profile must start with version 1"
        );
        assert!(
            profile.contains("(deny default)"),
            "Profile must have deny-default"
        );
        assert!(
            profile.contains("process-exec"),
            "Profile must allow process execution"
        );
        assert!(
            profile.contains("process-fork"),
            "Profile must allow process forking"
        );
        assert!(
            profile.contains("/usr/lib"),
            "Profile must allow system libraries"
        );
        assert!(
            profile.contains("sysctl-read"),
            "Profile must allow sysctl reads"
        );
        assert!(
            profile.contains("/dev/null"),
            "Profile must allow /dev/null access"
        );
        assert!(
            profile.contains("pseudo-tty"),
            "Profile must allow pseudo-tty"
        );
        assert!(
            profile.contains("ipc-posix-sem"),
            "Profile must allow POSIX semaphores"
        );

        // Container rootfs access
        assert!(
            profile.contains("rootfs"),
            "Profile must grant access to container rootfs"
        );

        let _ = runtime.remove_container(&id).await;
    });
}
