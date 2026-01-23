//! Service-level container lifecycle management

use crate::error::{AgentError, Result};
use crate::health::{HealthChecker, HealthMonitor};
use crate::init::InitOrchestrator;
use crate::runtime::{Container, ContainerId, ContainerState, Runtime};
use spec::ServiceSpec;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;

/// Service instance manages a single service's containers
pub struct ServiceInstance {
    pub service_name: String,
    pub spec: ServiceSpec,
    runtime: Arc<dyn Runtime + Send + Sync>,
    containers: tokio::sync::RwLock<std::collections::HashMap<ContainerId, Container>>,
}

impl ServiceInstance {
    /// Create a new service instance
    pub fn new(service_name: String, spec: ServiceSpec, runtime: Arc<dyn Runtime + Send + Sync>) -> Self {
        Self {
            service_name,
            spec,
            runtime,
            containers: tokio::sync::RwLock::new(std::collections::HashMap::new()),
        }
    }

    /// Scale to the desired number of replicas
    pub async fn scale_to(&self, replicas: u32) -> Result<()> {
        let mut containers = self.containers.write().await;
        let current_replicas = containers.len() as u32;

        // Scale up
        if replicas > current_replicas {
            for i in current_replicas..replicas {
                let id = ContainerId {
                    service: self.service_name.clone(),
                    replica: i + 1,
                };

                // Pull image
                self.runtime
                    .pull_image(&self.spec.image.name)
                    .await
                    .map_err(|e| AgentError::PullFailed {
                        image: self.spec.image.name.clone(),
                        reason: e.to_string(),
                    })?;

                // Create container
                self.runtime.create_container(&id, &self.spec).await.map_err(|e| {
                    AgentError::CreateFailed {
                        id: id.to_string(),
                        reason: e.to_string(),
                    }
                })?;

                // Run init actions
                let init_orchestrator = InitOrchestrator::new(id.clone(), self.spec.init.clone());
                init_orchestrator.run().await?;

                // Start container
                self.runtime.start_container(&id).await.map_err(|e| AgentError::StartFailed {
                    id: id.to_string(),
                    reason: e.to_string(),
                })?;

                // Start health monitoring
                {
                    let check = self.spec.health.check.clone();
                    let interval = self.spec.health.interval.unwrap_or(Duration::from_secs(10));
                    let retries = self.spec.health.retries;

                    let checker = HealthChecker::new(check);
                    let monitor = HealthMonitor::new(id.clone(), checker, interval, retries);
                    // TODO: store monitor handle
                    let _monitor = monitor;
                }

                containers.insert(id.clone(), Container {
                    id: id.clone(),
                    state: ContainerState::Running,
                    pid: None,
                    task: None,
                });
            }
        }

        // Scale down
        if replicas < current_replicas {
            for i in replicas..current_replicas {
                let id = ContainerId {
                    service: self.service_name.clone(),
                    replica: i + 1,
                };

                if let Some(container) = containers.remove(&id) {
                    // Stop container
                    self.runtime
                        .stop_container(&id, Duration::from_secs(30))
                        .await?;

                    // Remove container
                    self.runtime.remove_container(&id).await?;
                }
            }
        }

        Ok(())
    }

    /// Get current number of replicas
    pub async fn replica_count(&self) -> usize {
        self.containers.read().await.len()
    }

    /// Get all container IDs
    pub async fn container_ids(&self) -> Vec<ContainerId> {
        self.containers.read().await.keys().cloned().collect()
    }
}

/// Service manager for multiple services
pub struct ServiceManager {
    runtime: Arc<dyn Runtime + Send + Sync>,
    services: tokio::sync::RwLock<std::collections::HashMap<String, ServiceInstance>>,
    scale_semaphore: Arc<Semaphore>,
}

impl ServiceManager {
    /// Create a new service manager
    pub fn new(runtime: Arc<dyn Runtime + Send + Sync>) -> Self {
        Self {
            runtime,
            services: tokio::sync::RwLock::new(std::collections::HashMap::new()),
            scale_semaphore: Arc::new(Semaphore::new(10)), // Max 10 concurrent scaling operations
        }
    }

    /// Add or update a service
    pub async fn upsert_service(&self, name: String, spec: ServiceSpec) -> Result<()> {
        let mut services = self.services.write().await;

        if let Some(instance) = services.get_mut(&name) {
            // Update existing service
            instance.spec = spec;
        } else {
            // Create new service
            let instance = ServiceInstance::new(name.clone(), spec, self.runtime.clone());
            services.insert(name, instance);
        }

        Ok(())
    }

    /// Scale a service to desired replica count
    pub async fn scale_service(&self, name: &str, replicas: u32) -> Result<()> {
        let _permit = self.scale_semaphore.acquire().await;

        let services = self.services.read().await;
        let instance = services
            .get(name)
            .ok_or_else(|| AgentError::NotFound {
                container: name.to_string(),
                reason: "service not found".to_string(),
            })?;

        instance.scale_to(replicas).await
    }

    /// Get service replica count
    pub async fn service_replica_count(&self, name: &str) -> Result<usize> {
        let services = self.services.read().await;
        let instance = services
            .get(name)
            .ok_or_else(|| AgentError::NotFound {
                container: name.to_string(),
                reason: "service not found".to_string(),
            })?;

        Ok(instance.replica_count().await)
    }

    /// Remove a service
    pub async fn remove_service(&self, name: &str) -> Result<()> {
        let mut services = self.services.write().await;
        services.remove(name).ok_or_else(|| AgentError::NotFound {
            container: name.to_string(),
            reason: "service not found".to_string(),
        })?;
        Ok(())
    }

    /// List all services
    pub async fn list_services(&self) -> Vec<String> {
        self.services.read().await.keys().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::MockRuntime;

    #[tokio::test]
    async fn test_service_manager() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let manager = ServiceManager::new(runtime);

        // Add service
        let spec = mock_spec();
        manager.upsert_service("test".to_string(), spec).await.unwrap();

        // Scale up
        manager.scale_service("test", 3).await.unwrap();

        // Check count
        let count = manager.service_replica_count("test").await.unwrap();
        assert_eq!(count, 3);

        // List services
        let services = manager.list_services().await;
        assert_eq!(services, vec!["test".to_string()]);
    }

    fn mock_spec() -> ServiceSpec {
        use spec::*;
        serde_yaml::from_str::<spec::DeploymentSpec>(
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
    scale:
      mode: fixed
      replicas: 1
"#,
        )
        .unwrap()
        .services
        .remove("test")
        .unwrap()
    }
}
