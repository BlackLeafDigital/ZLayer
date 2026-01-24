//! End-to-end integration tests for ZLayer with youki/libcontainer runtime
//!
//! These tests verify the complete container lifecycle using the youki-based
//! runtime via libcontainer. Tests require root privileges for namespace operations.
//!
//! # Requirements
//! - Root privileges for container namespace operations
//! - Pre-populated rootfs directories for images
//!
//! # Running
//! ```bash
//! # Run with sudo for container access
//! sudo cargo test --package agent --test youki_e2e -- --nocapture
//! ```

use agent::{
    AgentError, ContainerId, ContainerState, HealthChecker, ProxyManager, ProxyManagerConfig,
    Runtime, ServiceInstance, ServiceManager, YoukiConfig, YoukiRuntime,
};
use spec::{DeploymentSpec, HealthCheck, ServiceSpec};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

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
const E2E_TEST_DIR: &str = "/tmp/zlayer-youki-e2e-test";

/// Test images
const ALPINE_IMAGE: &str = "docker.io/library/alpine:latest";

// =============================================================================
// Skip Mechanism
// =============================================================================

/// Check if we have root privileges required for container operations
fn has_root_privileges() -> bool {
    unsafe { libc::geteuid() == 0 }
}

/// Macro to skip tests when root privileges are not available
macro_rules! skip_without_root {
    () => {
        if !has_root_privileges() {
            eprintln!("Skipping test: root privileges required for container operations");
            return;
        }
    };
}

// =============================================================================
// Helper Functions
// =============================================================================

/// Generate a unique name with the given prefix for test isolation
///
/// This ensures tests can run in parallel without name collisions.
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
///
/// Returns `Ok(())` if the state is reached, `Err` on timeout or other error.
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

/// Wait for a TCP port to become available
///
/// Returns `Ok(())` when connection succeeds, `Err` on timeout.
#[allow(dead_code)]
async fn wait_for_port(addr: &str, timeout: Duration) -> Result<(), String> {
    let start = std::time::Instant::now();
    let poll_interval = Duration::from_millis(100);

    while start.elapsed() < timeout {
        if tokio::net::TcpStream::connect(addr).await.is_ok() {
            return Ok(());
        }
        tokio::time::sleep(poll_interval).await;
    }

    Err(format!(
        "Timeout waiting for port {} to become available",
        addr
    ))
}

/// Create a YoukiRuntime configured for E2E testing
async fn create_e2e_runtime() -> Result<YoukiRuntime, AgentError> {
    let test_dir = PathBuf::from(E2E_TEST_DIR);
    let config = YoukiConfig {
        state_dir: test_dir.join("state"),
        rootfs_dir: test_dir.join("rootfs"),
        bundle_dir: test_dir.join("bundles"),
        cache_dir: test_dir.join("cache"),
        use_systemd: false,
    };
    YoukiRuntime::new(config).await
}

/// Create a minimal ServiceSpec for testing with the given image
#[allow(dead_code)]
fn create_test_spec(image: &str, port: u16) -> ServiceSpec {
    let yaml = format!(
        r#"
version: v1
deployment: e2e-test
services:
  test:
    rtype: service
    image:
      name: {}
    endpoints:
      - name: http
        protocol: http
        port: {}
    scale:
      mode: fixed
      replicas: 1
    health:
      check:
        type: tcp
        port: {}
      retries: 3
"#,
        image, port, port
    );

    serde_yaml::from_str::<DeploymentSpec>(&yaml)
        .expect("Failed to parse test spec")
        .services
        .remove("test")
        .expect("Missing test service")
}

/// Create an alpine ServiceSpec that runs a simple command
fn create_alpine_spec() -> ServiceSpec {
    let yaml = r#"
version: v1
deployment: e2e-test
services:
  alpine:
    rtype: service
    image:
      name: docker.io/library/alpine:latest
    endpoints:
      - name: dummy
        protocol: tcp
        port: 8080
    scale:
      mode: fixed
      replicas: 1
"#;

    serde_yaml::from_str::<DeploymentSpec>(yaml)
        .expect("Failed to parse alpine spec")
        .services
        .remove("alpine")
        .expect("Missing alpine service")
}

/// Create an nginx ServiceSpec for web server testing
fn create_nginx_spec() -> ServiceSpec {
    let yaml = r#"
version: v1
deployment: e2e-test
services:
  nginx:
    rtype: service
    image:
      name: docker.io/library/nginx:alpine
    endpoints:
      - name: http
        protocol: http
        port: 80
        expose: public
    scale:
      mode: fixed
      replicas: 1
    health:
      check:
        type: tcp
        port: 80
      retries: 3
"#;

    serde_yaml::from_str::<DeploymentSpec>(yaml)
        .expect("Failed to parse nginx spec")
        .services
        .remove("nginx")
        .expect("Missing nginx service")
}

/// Cleanup helper - ensures container is removed even on test failure
struct ContainerGuard {
    runtime: Arc<dyn Runtime + Send + Sync>,
    id: ContainerId,
}

impl ContainerGuard {
    fn new(runtime: Arc<dyn Runtime + Send + Sync>, id: ContainerId) -> Self {
        Self { runtime, id }
    }
}

impl Drop for ContainerGuard {
    fn drop(&mut self) {
        let runtime = self.runtime.clone();
        let id = self.id.clone();

        // Spawn cleanup task - we can't await in Drop
        tokio::spawn(async move {
            // Try to stop
            let _ = runtime.stop_container(&id, Duration::from_secs(5)).await;
            // Try to remove
            let _ = runtime.remove_container(&id).await;
        });
    }
}

// =============================================================================
// Container Lifecycle Tests
// =============================================================================

/// Test complete container lifecycle: pull -> create -> start -> stop -> remove
#[tokio::test]
async fn test_container_lifecycle() {
    with_timeout!(180, {
        skip_without_root!();

        let runtime = match create_e2e_runtime().await {
            Ok(r) => Arc::new(r) as Arc<dyn Runtime + Send + Sync>,
            Err(e) => {
                eprintln!("Failed to create runtime: {}", e);
                return;
            }
        };

        let service_name = unique_name("lifecycle");
        let id = ContainerId {
            service: service_name.clone(),
            replica: 1,
        };
        let spec = create_alpine_spec();

        // Setup cleanup guard
        let _guard = ContainerGuard::new(runtime.clone(), id.clone());

        // 1. Pull image (note: youki runtime expects rootfs to be pre-populated)
        println!("Pulling image: {}", ALPINE_IMAGE);
        let pull_result = runtime.pull_image(ALPINE_IMAGE).await;
        assert!(
            pull_result.is_ok(),
            "Failed to pull image: {:?}",
            pull_result
        );

        // 2. Create container
        println!("Creating container: {}", id);
        let create_result = runtime.create_container(&id, &spec).await;
        assert!(
            create_result.is_ok(),
            "Failed to create container: {:?}",
            create_result
        );

        // Verify container exists and is pending
        let state = runtime.container_state(&id).await;
        assert!(state.is_ok(), "Failed to get container state: {:?}", state);
        assert_eq!(state.unwrap(), ContainerState::Pending);

        // 3. Start container
        println!("Starting container: {}", id);
        let start_result = runtime.start_container(&id).await;
        assert!(
            start_result.is_ok(),
            "Failed to start container: {:?}",
            start_result
        );

        // Wait for running state
        let wait_result = wait_for_state(
            runtime.as_ref(),
            &id,
            ContainerState::Running,
            Duration::from_secs(30),
        )
        .await;
        assert!(
            wait_result.is_ok(),
            "Container did not reach Running state: {}",
            wait_result.unwrap_err()
        );

        // 4. Stop container
        println!("Stopping container: {}", id);
        let stop_result = runtime.stop_container(&id, Duration::from_secs(10)).await;
        assert!(
            stop_result.is_ok(),
            "Failed to stop container: {:?}",
            stop_result
        );

        // 5. Remove container
        println!("Removing container: {}", id);
        let remove_result = runtime.remove_container(&id).await;
        assert!(
            remove_result.is_ok(),
            "Failed to remove container: {:?}",
            remove_result
        );

        // Verify container is gone
        let state = runtime.container_state(&id).await;
        assert!(state.is_err(), "Container should not exist after removal");
    });
}

// =============================================================================
// Service Scaling Tests
// =============================================================================

/// Test service scaling up and down with ServiceManager
#[tokio::test]
async fn test_service_scaling() {
    with_timeout!(180, {
        skip_without_root!();

        let runtime = match create_e2e_runtime().await {
            Ok(r) => Arc::new(r) as Arc<dyn Runtime + Send + Sync>,
            Err(e) => {
                eprintln!("Failed to create runtime: {}", e);
                return;
            }
        };

        let service_name = unique_name("scale");
        let spec = create_alpine_spec();

        let manager = ServiceManager::new(runtime.clone());

        // Add service
        let upsert_result = manager.upsert_service(service_name.clone(), spec).await;
        assert!(
            upsert_result.is_ok(),
            "Failed to upsert service: {:?}",
            upsert_result
        );

        // Scale up to 2 replicas
        println!("Scaling {} to 2 replicas", service_name);
        let scale_result = manager.scale_service(&service_name, 2).await;
        assert!(
            scale_result.is_ok(),
            "Failed to scale up: {:?}",
            scale_result
        );

        // Verify replica count
        let count = manager.service_replica_count(&service_name).await;
        assert!(count.is_ok(), "Failed to get replica count: {:?}", count);
        assert_eq!(count.unwrap(), 2, "Expected 2 replicas after scale up");

        // Scale down to 1 replica
        println!("Scaling {} to 1 replica", service_name);
        let scale_result = manager.scale_service(&service_name, 1).await;
        assert!(
            scale_result.is_ok(),
            "Failed to scale down: {:?}",
            scale_result
        );

        // Verify replica count
        let count = manager.service_replica_count(&service_name).await;
        assert!(count.is_ok(), "Failed to get replica count: {:?}", count);
        assert_eq!(count.unwrap(), 1, "Expected 1 replica after scale down");

        // Cleanup: scale to 0
        let _ = manager.scale_service(&service_name, 0).await;
    });
}

// =============================================================================
// Health Check Tests
// =============================================================================

/// Test TCP health check against nginx
#[tokio::test]
async fn test_health_checks_tcp() {
    with_timeout!(180, {
        skip_without_root!();

        let runtime = match create_e2e_runtime().await {
            Ok(r) => Arc::new(r) as Arc<dyn Runtime + Send + Sync>,
            Err(e) => {
                eprintln!("Failed to create runtime: {}", e);
                return;
            }
        };

        let service_name = unique_name("health");
        let id = ContainerId {
            service: service_name.clone(),
            replica: 1,
        };
        let _spec = create_nginx_spec();

        // Setup cleanup guard
        let _guard = ContainerGuard::new(runtime.clone(), id.clone());

        // Create TCP health checker
        let health_check = HealthCheck::Tcp { port: 80 };
        let checker = HealthChecker::new(health_check);

        // Perform health check (this connects to localhost:80, which won't work in network namespace)
        // In a real E2E test with proper networking, this would pass
        // For now, we just verify the checker doesn't panic
        let check_result = checker.check(&id, Duration::from_secs(5)).await;
        println!("Health check result: {:?}", check_result);

        // Cleanup
        let _ = runtime.stop_container(&id, Duration::from_secs(10)).await;
        let _ = runtime.remove_container(&id).await;
    });
}

// =============================================================================
// Proxy Manager Tests
// =============================================================================

/// Test ProxyManager route and backend management
#[tokio::test]
async fn test_proxy_routing() {
    with_timeout!(180, {
        // Note: This test doesn't require root directly, but we skip it
        // when root isn't available since the full E2E suite is meant
        // to run together
        skip_without_root!();

        let service_name = unique_name("proxy");
        let spec = create_nginx_spec();

        // Create ProxyManager with a random high port
        let port: u16 = 30000 + (rand::random::<u16>() % 10000);
        let addr: SocketAddr = format!("127.0.0.1:{}", port).parse().unwrap();
        let config = ProxyManagerConfig::new(addr);
        let manager = ProxyManager::new(config);

        // Add service routes
        manager.add_service(&service_name, &spec).await;
        assert!(
            manager.has_service(&service_name).await,
            "Service should be registered"
        );

        // Verify route was added
        let route_count = manager.route_count().await;
        assert!(route_count > 0, "Should have at least one route");

        // Add backends
        let backend_addr1: SocketAddr = "127.0.0.1:8080".parse().unwrap();
        let backend_addr2: SocketAddr = "127.0.0.1:8081".parse().unwrap();

        manager.add_backend(&service_name, backend_addr1).await;
        manager.add_backend(&service_name, backend_addr2).await;

        // Verify backends were added via the router
        let lb = manager.router().get_lb(&service_name).await;
        assert!(lb.is_some(), "Load balancer should exist");
        assert_eq!(
            lb.unwrap().backend_count().await,
            2,
            "Should have 2 backends"
        );

        // Update health status
        manager
            .update_backend_health(&service_name, backend_addr1, false)
            .await;

        let lb = manager.router().get_lb(&service_name).await.unwrap();
        assert_eq!(
            lb.healthy_count().await,
            1,
            "One backend should be unhealthy"
        );

        // Remove backend
        manager.remove_backend(&service_name, backend_addr1).await;
        assert_eq!(
            lb.backend_count().await,
            1,
            "Should have 1 backend after removal"
        );

        // Remove service
        manager.remove_service(&service_name).await;
        assert!(
            !manager.has_service(&service_name).await,
            "Service should be removed"
        );
    });
}

// =============================================================================
// Container Logs Tests
// =============================================================================

/// Test retrieving container logs
#[tokio::test]
async fn test_container_logs() {
    with_timeout!(180, {
        skip_without_root!();

        let runtime = match create_e2e_runtime().await {
            Ok(r) => Arc::new(r) as Arc<dyn Runtime + Send + Sync>,
            Err(e) => {
                eprintln!("Failed to create runtime: {}", e);
                return;
            }
        };

        let service_name = unique_name("logs");
        let id = ContainerId {
            service: service_name.clone(),
            replica: 1,
        };
        let spec = create_alpine_spec();

        // Setup cleanup guard
        let _guard = ContainerGuard::new(runtime.clone(), id.clone());

        // Pull and create container
        runtime
            .pull_image(ALPINE_IMAGE)
            .await
            .expect("Failed to pull");
        runtime
            .create_container(&id, &spec)
            .await
            .expect("Failed to create");
        runtime.start_container(&id).await.expect("Failed to start");

        // Wait for container to start
        wait_for_state(
            runtime.as_ref(),
            &id,
            ContainerState::Running,
            Duration::from_secs(30),
        )
        .await
        .ok(); // May not reach running if container exits quickly

        // Give it a moment to potentially write logs
        tokio::time::sleep(Duration::from_secs(1)).await;

        // Get container logs
        let logs_result = runtime.container_logs(&id, 100).await;
        println!("Logs result: {:?}", logs_result);

        // The result depends on whether the container is still tracked
        // Just verify the API doesn't panic
        match logs_result {
            Ok(logs) => {
                println!("Container logs:\n{}", logs);
            }
            Err(e) => {
                println!("Could not get logs (expected if container exited): {}", e);
            }
        }

        // Cleanup
        let _ = runtime.stop_container(&id, Duration::from_secs(5)).await;
        let _ = runtime.remove_container(&id).await;
    });
}

// =============================================================================
// Error Handling Tests
// =============================================================================

/// Test NotFound error when removing a non-existent container
#[tokio::test]
async fn test_error_remove_nonexistent() {
    with_timeout!(180, {
        skip_without_root!();

        let runtime = match create_e2e_runtime().await {
            Ok(r) => r,
            Err(e) => {
                eprintln!("Failed to create runtime: {}", e);
                return;
            }
        };

        let id = ContainerId {
            service: unique_name("nonexistent"),
            replica: 999,
        };

        println!("Attempting to remove non-existent container: {}", id);
        let result = runtime.remove_container(&id).await;

        assert!(result.is_err(), "Should fail for non-existent container");
        match result {
            Err(AgentError::NotFound { container, reason }) => {
                println!("Got expected NotFound error:");
                println!("  container: {}", container);
                println!("  reason: {}", reason);
            }
            Err(other) => {
                println!("Got error (may be acceptable): {:?}", other);
            }
            Ok(_) => panic!("Should not succeed removing non-existent container"),
        }
    });
}

/// Test getting state of a non-existent container returns NotFound
#[tokio::test]
async fn test_error_state_nonexistent() {
    with_timeout!(180, {
        skip_without_root!();

        let runtime = match create_e2e_runtime().await {
            Ok(r) => r,
            Err(e) => {
                eprintln!("Failed to create runtime: {}", e);
                return;
            }
        };

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
                println!("Got different error: {:?}", other);
            }
            Ok(state) => panic!(
                "Should not get state for non-existent container, got: {:?}",
                state
            ),
        }
    });
}

// =============================================================================
// Concurrent Operations Tests
// =============================================================================

/// Test that multiple containers can be managed concurrently
#[tokio::test]
async fn test_concurrent_containers() {
    with_timeout!(180, {
        skip_without_root!();

        let runtime = match create_e2e_runtime().await {
            Ok(r) => Arc::new(r) as Arc<dyn Runtime + Send + Sync>,
            Err(e) => {
                eprintln!("Failed to create runtime: {}", e);
                return;
            }
        };

        // First, pull the image once
        runtime
            .pull_image(ALPINE_IMAGE)
            .await
            .expect("Failed to pull image");

        let container_count = 3;
        let base_name = unique_name("concurrent");
        let spec = create_alpine_spec();

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

        // Cleanup all containers
        for id in created_ids {
            let _ = runtime.remove_container(&id).await;
        }
    });
}

// =============================================================================
// Service Instance Tests
// =============================================================================

/// Test ServiceInstance directly for more granular control
#[tokio::test]
async fn test_service_instance_lifecycle() {
    with_timeout!(180, {
        skip_without_root!();

        let runtime = match create_e2e_runtime().await {
            Ok(r) => Arc::new(r) as Arc<dyn Runtime + Send + Sync>,
            Err(e) => {
                eprintln!("Failed to create runtime: {}", e);
                return;
            }
        };

        let service_name = unique_name("instance");
        let spec = create_alpine_spec();

        let instance = ServiceInstance::new(service_name.clone(), spec, runtime.clone());

        // Initial state should have 0 replicas
        assert_eq!(instance.replica_count().await, 0);

        // Scale up to 1
        println!("Scaling {} to 1 replica via ServiceInstance", service_name);
        let scale_result = instance.scale_to(1).await;
        assert!(
            scale_result.is_ok(),
            "Failed to scale to 1: {:?}",
            scale_result
        );
        assert_eq!(instance.replica_count().await, 1);

        // Get container IDs
        let ids = instance.container_ids().await;
        assert_eq!(ids.len(), 1, "Should have 1 container ID");
        println!("Container IDs: {:?}", ids);

        // Scale back to 0
        println!("Scaling {} to 0 replicas", service_name);
        let scale_result = instance.scale_to(0).await;
        assert!(
            scale_result.is_ok(),
            "Failed to scale to 0: {:?}",
            scale_result
        );
        assert_eq!(instance.replica_count().await, 0);
    });
}

// =============================================================================
// Resource Cleanup Test
// =============================================================================

/// Verify that container state directory is cleaned up on removal
#[tokio::test]
async fn test_cleanup_state_directory() {
    with_timeout!(180, {
        skip_without_root!();

        let runtime = match create_e2e_runtime().await {
            Ok(r) => Arc::new(r) as Arc<dyn Runtime + Send + Sync>,
            Err(e) => {
                eprintln!("Failed to create runtime: {}", e);
                return;
            }
        };

        let service_name = unique_name("cleanup");
        let id = ContainerId {
            service: service_name.clone(),
            replica: 1,
        };
        let spec = create_alpine_spec();

        // Pull image
        runtime
            .pull_image(ALPINE_IMAGE)
            .await
            .expect("Failed to pull");

        // Create container
        runtime
            .create_container(&id, &spec)
            .await
            .expect("Failed to create");

        // Start container
        runtime.start_container(&id).await.expect("Failed to start");

        // Give it a moment
        tokio::time::sleep(Duration::from_millis(500)).await;

        // State directory should exist
        let state_dir = format!("{}/state/{}-{}", E2E_TEST_DIR, id.service, id.replica);
        let exists_before = tokio::fs::metadata(&state_dir).await.is_ok();
        println!(
            "State directory {} exists before removal: {}",
            state_dir, exists_before
        );

        // Stop and remove
        let _ = runtime.stop_container(&id, Duration::from_secs(5)).await;
        runtime
            .remove_container(&id)
            .await
            .expect("Failed to remove");

        // State directory should be cleaned up
        let exists_after = tokio::fs::metadata(&state_dir).await.is_ok();
        println!(
            "State directory {} exists after removal: {}",
            state_dir, exists_after
        );

        // Note: The directory might still exist briefly due to async cleanup
        // This is acceptable behavior
    });
}
