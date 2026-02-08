//! Abstract container runtime interface
//!
//! Defines the Runtime trait that can be implemented for different container runtimes
//! (containerd, CRI-O, etc.)

use crate::cgroups_stats::ContainerStats;
use crate::error::{AgentError, Result};
use std::net::IpAddr;
use std::time::Duration;
use tokio::task::JoinHandle;
use zlayer_spec::{PullPolicy, ServiceSpec};

/// Container state
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContainerState {
    /// Container is being pulled/created
    Pending,
    /// Init actions are running
    Initializing,
    /// Container is running
    Running,
    /// Container is stopping
    Stopping,
    /// Container has exited
    Exited { code: i32 },
    /// Container failed
    Failed { reason: String },
}

/// Container identifier
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ContainerId {
    pub service: String,
    pub replica: u32,
}

impl std::fmt::Display for ContainerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}-rep-{}", self.service, self.replica)
    }
}

/// Container handle
pub struct Container {
    pub id: ContainerId,
    pub state: ContainerState,
    pub pid: Option<u32>,
    pub task: Option<JoinHandle<std::io::Result<()>>>,
    /// Overlay network IP address assigned to this container
    pub overlay_ip: Option<IpAddr>,
    /// Health monitor task handle for this container
    pub health_monitor: Option<JoinHandle<()>>,
}

/// Abstract container runtime trait
///
/// This trait abstracts over different container runtimes (containerd, CRI-O, etc.)
#[async_trait::async_trait]
pub trait Runtime: Send + Sync {
    /// Pull an image to local storage
    async fn pull_image(&self, image: &str) -> Result<()>;

    /// Pull an image to local storage with a specific policy
    async fn pull_image_with_policy(&self, image: &str, policy: PullPolicy) -> Result<()>;

    /// Create a container
    async fn create_container(&self, id: &ContainerId, spec: &ServiceSpec) -> Result<()>;

    /// Start a container
    async fn start_container(&self, id: &ContainerId) -> Result<()>;

    /// Stop a container
    async fn stop_container(&self, id: &ContainerId, timeout: Duration) -> Result<()>;

    /// Remove a container
    async fn remove_container(&self, id: &ContainerId) -> Result<()>;

    /// Get container state
    async fn container_state(&self, id: &ContainerId) -> Result<ContainerState>;

    /// Get container logs
    async fn container_logs(&self, id: &ContainerId, tail: usize) -> Result<String>;

    /// Execute command in container
    async fn exec(&self, id: &ContainerId, cmd: &[String]) -> Result<(i32, String, String)>;

    /// Get container resource statistics from cgroups
    ///
    /// Returns CPU and memory statistics for the specified container.
    /// Used for metrics collection and autoscaling decisions.
    async fn get_container_stats(&self, id: &ContainerId) -> Result<ContainerStats>;

    /// Wait for a container to exit and return its exit code
    ///
    /// This method blocks until the container exits or an error occurs.
    /// Used primarily for job execution to implement run-to-completion semantics.
    async fn wait_container(&self, id: &ContainerId) -> Result<i32>;

    /// Get container logs (stdout/stderr combined)
    ///
    /// Returns logs as a vector of log lines.
    /// Used to capture job output after completion.
    async fn get_logs(&self, id: &ContainerId) -> Result<Vec<String>>;

    /// Get the PID of a container's main process
    ///
    /// Returns:
    /// - `Ok(Some(pid))` for runtimes with real processes (Youki, Docker)
    /// - `Ok(None)` for runtimes without separate PIDs (WASM in-process)
    /// - `Err` if the container doesn't exist or there's an error
    ///
    /// Used for overlay network attachment and process management.
    async fn get_container_pid(&self, id: &ContainerId) -> Result<Option<u32>>;

    /// Get the IP address of a container
    ///
    /// Returns:
    /// - `Ok(Some(ip))` if the container has a known IP address
    /// - `Ok(None)` if the container exists but has no IP assigned yet
    /// - `Err` if the container doesn't exist or there's an error
    ///
    /// Used for proxy backend registration when overlay networking is unavailable.
    async fn get_container_ip(&self, id: &ContainerId) -> Result<Option<IpAddr>>;
}

/// In-memory mock runtime for testing and development
pub struct MockRuntime {
    containers: tokio::sync::RwLock<std::collections::HashMap<ContainerId, Container>>,
}

impl MockRuntime {
    pub fn new() -> Self {
        Self {
            containers: tokio::sync::RwLock::new(std::collections::HashMap::new()),
        }
    }
}

impl Default for MockRuntime {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Runtime for MockRuntime {
    async fn pull_image(&self, _image: &str) -> Result<()> {
        self.pull_image_with_policy(_image, PullPolicy::IfNotPresent)
            .await
    }

    async fn pull_image_with_policy(&self, _image: &str, _policy: PullPolicy) -> Result<()> {
        // Mock: always succeeds
        tokio::time::sleep(Duration::from_millis(100)).await;
        Ok(())
    }

    async fn create_container(&self, id: &ContainerId, _spec: &ServiceSpec) -> Result<()> {
        let mut containers = self.containers.write().await;
        containers.insert(
            id.clone(),
            Container {
                id: id.clone(),
                state: ContainerState::Pending,
                pid: None,
                task: None,
                overlay_ip: None,
                health_monitor: None,
            },
        );
        Ok(())
    }

    async fn start_container(&self, id: &ContainerId) -> Result<()> {
        let mut containers = self.containers.write().await;
        if let Some(container) = containers.get_mut(id) {
            container.state = ContainerState::Running;
            container.pid = Some(std::process::id()); // Mock PID
        }
        Ok(())
    }

    async fn stop_container(&self, id: &ContainerId, _timeout: Duration) -> Result<()> {
        let mut containers = self.containers.write().await;
        if let Some(container) = containers.get_mut(id) {
            container.state = ContainerState::Exited { code: 0 };
        }
        Ok(())
    }

    async fn remove_container(&self, id: &ContainerId) -> Result<()> {
        let mut containers = self.containers.write().await;
        containers.remove(id);
        Ok(())
    }

    async fn container_state(&self, id: &ContainerId) -> Result<ContainerState> {
        let containers = self.containers.read().await;
        containers
            .get(id)
            .map(|c| c.state.clone())
            .ok_or_else(|| AgentError::NotFound {
                container: id.to_string(),
                reason: "container not found".to_string(),
            })
    }

    async fn container_logs(&self, id: &ContainerId, _tail: usize) -> Result<String> {
        Ok(format!("Mock logs for {}", id))
    }

    async fn exec(&self, _id: &ContainerId, cmd: &[String]) -> Result<(i32, String, String)> {
        Ok((0, cmd.join(" "), "".to_string()))
    }

    async fn get_container_stats(&self, id: &ContainerId) -> Result<ContainerStats> {
        // Mock: return dummy stats
        let containers = self.containers.read().await;
        if containers.contains_key(id) {
            Ok(ContainerStats {
                cpu_usage_usec: 1_000_000,       // 1 second
                memory_bytes: 50 * 1024 * 1024,  // 50 MB
                memory_limit: 256 * 1024 * 1024, // 256 MB
                timestamp: std::time::Instant::now(),
            })
        } else {
            Err(AgentError::NotFound {
                container: id.to_string(),
                reason: "container not found".to_string(),
            })
        }
    }

    async fn wait_container(&self, id: &ContainerId) -> Result<i32> {
        // Mock: simulate waiting for container to exit
        let containers = self.containers.read().await;
        if let Some(container) = containers.get(id) {
            match &container.state {
                ContainerState::Exited { code } => Ok(*code),
                ContainerState::Failed { .. } => Ok(1),
                _ => {
                    // Simulate a brief wait and then return success
                    drop(containers);
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    Ok(0)
                }
            }
        } else {
            Err(AgentError::NotFound {
                container: id.to_string(),
                reason: "container not found".to_string(),
            })
        }
    }

    async fn get_logs(&self, id: &ContainerId) -> Result<Vec<String>> {
        // Mock: return dummy log lines
        let containers = self.containers.read().await;
        if containers.contains_key(id) {
            Ok(vec![
                format!("[{}] Container started", id),
                format!("[{}] Executing command...", id),
                format!("[{}] Command completed successfully", id),
            ])
        } else {
            Err(AgentError::NotFound {
                container: id.to_string(),
                reason: "container not found".to_string(),
            })
        }
    }

    async fn get_container_pid(&self, id: &ContainerId) -> Result<Option<u32>> {
        let containers = self.containers.read().await;
        if let Some(container) = containers.get(id) {
            Ok(container.pid)
        } else {
            Err(AgentError::NotFound {
                container: id.to_string(),
                reason: "container not found".to_string(),
            })
        }
    }

    async fn get_container_ip(&self, id: &ContainerId) -> Result<Option<IpAddr>> {
        let containers = self.containers.read().await;
        if containers.contains_key(id) {
            // Mock: deterministic IP based on replica number (172.17.0.{replica+2})
            let last_octet = id.replica + 2;
            Ok(Some(IpAddr::V4(std::net::Ipv4Addr::new(
                172,
                17,
                0,
                last_octet as u8,
            ))))
        } else {
            Err(AgentError::NotFound {
                container: id.to_string(),
                reason: "container not found".to_string(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mock_runtime() {
        let runtime = MockRuntime::new();
        let id = ContainerId {
            service: "test".to_string(),
            replica: 1,
        };

        runtime.pull_image("test:latest").await.unwrap();
        runtime.create_container(&id, &mock_spec()).await.unwrap();
        runtime.start_container(&id).await.unwrap();

        let state = runtime.container_state(&id).await.unwrap();
        assert_eq!(state, ContainerState::Running);
    }

    fn mock_spec() -> ServiceSpec {
        use zlayer_spec::*;
        serde_yaml::from_str::<DeploymentSpec>(
            r#"
version: v1
deployment: test
services:
  test:
    rtype: service
    image:
      name: test:latest
    endpoints:
      - name: http
        protocol: http
        port: 8080
"#,
        )
        .unwrap()
        .services
        .remove("test")
        .unwrap()
    }
}
