//! Metrics provider implementations for autoscaling
//!
//! This module provides implementations of the scheduler's metrics traits
//! that bridge the agent crate's ServiceManager and Runtime with the
//! scheduler crate's CgroupsMetricsSource.

use crate::cgroups_stats::ContainerStats;
use crate::runtime::{ContainerId, Runtime};
use crate::service::ServiceManager;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use zlayer_scheduler::metrics::{
    ContainerStatsProvider, MetricsContainerId, RawContainerStats, ServiceContainerProvider,
};

/// Provides container IDs for services from ServiceManager
///
/// Implements the scheduler's `ServiceContainerProvider` trait to
/// bridge ServiceManager with CgroupsMetricsSource.
///
/// # Example
///
/// ```ignore
/// use zlayer_agent::metrics_providers::ServiceManagerContainerProvider;
/// use zlayer_agent::ServiceManager;
/// use zlayer_scheduler::metrics::CgroupsMetricsSource;
/// use std::sync::Arc;
///
/// let manager = Arc::new(ServiceManager::new(runtime));
/// let provider = Arc::new(ServiceManagerContainerProvider::new(manager.clone()));
///
/// // Use with CgroupsMetricsSource
/// let stats_provider = Arc::new(RuntimeStatsProvider::new(runtime));
/// let source = CgroupsMetricsSource::new(provider, stats_provider);
/// ```
pub struct ServiceManagerContainerProvider {
    manager: Arc<ServiceManager>,
}

impl ServiceManagerContainerProvider {
    /// Create a new provider wrapping a ServiceManager
    pub fn new(manager: Arc<ServiceManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl ServiceContainerProvider for ServiceManagerContainerProvider {
    /// Get all container IDs for a given service
    async fn get_container_ids(&self, service_name: &str) -> Vec<MetricsContainerId> {
        // Get container IDs from ServiceManager
        let container_ids = self.manager.get_service_containers(service_name).await;

        container_ids
            .into_iter()
            .map(|id| MetricsContainerId {
                service: id.service,
                replica: id.replica,
            })
            .collect()
    }

    /// Get all services and their containers
    async fn get_all_services(&self) -> HashMap<String, Vec<MetricsContainerId>> {
        let service_names = self.manager.list_services().await;
        let mut result = HashMap::new();

        for name in service_names {
            let container_ids = self.get_container_ids(&name).await;
            if !container_ids.is_empty() {
                result.insert(name, container_ids);
            }
        }

        result
    }
}

/// Provides container statistics from the Runtime
///
/// Implements the scheduler's `ContainerStatsProvider` trait to
/// bridge the Runtime's `get_container_stats` with CgroupsMetricsSource.
///
/// # Example
///
/// ```ignore
/// use zlayer_agent::metrics_providers::RuntimeStatsProvider;
/// use zlayer_agent::YoukiRuntime;
/// use std::sync::Arc;
///
/// let runtime = Arc::new(YoukiRuntime::with_defaults().await?);
/// let stats_provider = Arc::new(RuntimeStatsProvider::new(runtime));
/// ```
pub struct RuntimeStatsProvider {
    runtime: Arc<dyn Runtime + Send + Sync>,
}

impl RuntimeStatsProvider {
    /// Create a new stats provider wrapping a Runtime
    pub fn new(runtime: Arc<dyn Runtime + Send + Sync>) -> Self {
        Self { runtime }
    }
}

#[async_trait]
impl ContainerStatsProvider for RuntimeStatsProvider {
    /// Get raw container statistics from cgroups
    async fn get_stats(&self, id: &MetricsContainerId) -> Result<RawContainerStats, String> {
        // Convert MetricsContainerId to ContainerId
        let container_id = ContainerId {
            service: id.service.clone(),
            replica: id.replica,
        };

        // Get stats from Runtime
        let stats = self
            .runtime
            .get_container_stats(&container_id)
            .await
            .map_err(|e| e.to_string())?;

        // Convert ContainerStats to RawContainerStats
        Ok(container_stats_to_raw(stats))
    }
}

/// Convert agent's ContainerStats to scheduler's RawContainerStats
///
/// This function bridges the two stats types, allowing the agent crate
/// to not depend on scheduler for its core types while still providing
/// the necessary data for metrics collection.
fn container_stats_to_raw(stats: ContainerStats) -> RawContainerStats {
    RawContainerStats {
        cpu_usage_usec: stats.cpu_usage_usec,
        memory_bytes: stats.memory_bytes,
        memory_limit: stats.memory_limit,
        timestamp: stats.timestamp,
    }
}

#[cfg(test)]
#[allow(deprecated)]
mod tests {
    use super::*;
    use crate::runtime::MockRuntime;
    use std::sync::Arc;

    fn mock_spec() -> zlayer_spec::ServiceSpec {
        serde_yaml::from_str::<zlayer_spec::DeploymentSpec>(
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

    #[tokio::test]
    async fn test_service_manager_container_provider_empty() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let manager = Arc::new(ServiceManager::new(runtime));
        let provider = ServiceManagerContainerProvider::new(manager);

        // No services registered yet
        let containers = provider.get_container_ids("nonexistent").await;
        assert!(containers.is_empty());

        let all = provider.get_all_services().await;
        assert!(all.is_empty());
    }

    #[tokio::test]
    async fn test_service_manager_container_provider_with_service() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let manager = Arc::new(ServiceManager::new(runtime));

        // Add and scale a service
        manager
            .upsert_service("api".to_string(), mock_spec())
            .await
            .unwrap();
        manager.scale_service("api", 3).await.unwrap();

        let provider = ServiceManagerContainerProvider::new(manager);

        // Should have 3 containers
        let containers = provider.get_container_ids("api").await;
        assert_eq!(containers.len(), 3);

        // Verify container IDs
        for c in &containers {
            assert_eq!(c.service, "api");
            // Replicas are 1-indexed
            assert!(c.replica >= 1 && c.replica <= 3);
        }

        // Test get_all_services
        let all = provider.get_all_services().await;
        assert_eq!(all.len(), 1);
        assert!(all.contains_key("api"));
        assert_eq!(all["api"].len(), 3);
    }

    #[tokio::test]
    async fn test_runtime_stats_provider() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let manager = Arc::new(ServiceManager::new(runtime.clone()));

        // Add and scale a service
        manager
            .upsert_service("test".to_string(), mock_spec())
            .await
            .unwrap();
        manager.scale_service("test", 1).await.unwrap();

        let stats_provider = RuntimeStatsProvider::new(runtime);

        let id = MetricsContainerId {
            service: "test".to_string(),
            replica: 1,
        };

        // MockRuntime returns predefined stats
        let stats = stats_provider.get_stats(&id).await.unwrap();
        assert_eq!(stats.cpu_usage_usec, 1_000_000);
        assert_eq!(stats.memory_bytes, 50 * 1024 * 1024);
        assert_eq!(stats.memory_limit, 256 * 1024 * 1024);
    }

    #[tokio::test]
    async fn test_runtime_stats_provider_not_found() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let stats_provider = RuntimeStatsProvider::new(runtime);

        let id = MetricsContainerId {
            service: "nonexistent".to_string(),
            replica: 1,
        };

        // Should return error for non-existent container
        let result = stats_provider.get_stats(&id).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_container_stats_to_raw() {
        use std::time::Instant;

        let stats = ContainerStats {
            cpu_usage_usec: 1_000_000,
            memory_bytes: 100 * 1024 * 1024,
            memory_limit: 256 * 1024 * 1024,
            timestamp: Instant::now(),
        };

        let raw = container_stats_to_raw(stats.clone());

        assert_eq!(raw.cpu_usage_usec, stats.cpu_usage_usec);
        assert_eq!(raw.memory_bytes, stats.memory_bytes);
        assert_eq!(raw.memory_limit, stats.memory_limit);
        // Note: Instant comparison is tricky, but they should be very close
    }
}
