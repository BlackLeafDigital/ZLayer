//! Abstract container runtime interface
//!
//! Defines the Runtime trait that can be implemented for different container runtimes
//! (containerd, CRI-O, etc.)

use crate::error::{AgentError, Result};
use spec::ServiceSpec;
use std::time::Duration;
use tokio::task::JoinHandle;

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
}

/// Abstract container runtime trait
///
/// This trait abstracts over different container runtimes (containerd, CRI-O, etc.)
#[async_trait::async_trait]
pub trait Runtime: Send + Sync {
    /// Pull an image to local storage
    async fn pull_image(&self, image: &str) -> Result<()>;

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
        use spec::*;
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
