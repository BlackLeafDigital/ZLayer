//! Deployment storage traits and implementations
//!
//! This module provides persistent storage for deployment specifications.
//!
//! # Example
//!
//! ```no_run
//! use zlayer_api::storage::{DeploymentStorage, RedbStorage, StoredDeployment, DeploymentStatus};
//! use std::sync::Arc;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let storage = RedbStorage::open("/tmp/deployments.redb")?;
//! let deployments = storage.list().await?;
//! # Ok(())
//! # }
//! ```

mod deployments;

pub use deployments::{DeploymentStorage, InMemoryStorage, RedbStorage, StorageError};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use zlayer_spec::DeploymentSpec;

/// A stored deployment with metadata
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct StoredDeployment {
    /// Deployment name (unique identifier)
    pub name: String,

    /// The deployment specification (complex nested structure, see spec docs)
    #[schema(value_type = Object)]
    pub spec: DeploymentSpec,

    /// Current deployment status
    pub status: DeploymentStatus,

    /// When the deployment was created
    #[schema(value_type = String, example = "2025-01-27T12:00:00Z")]
    pub created_at: DateTime<Utc>,

    /// When the deployment was last updated
    #[schema(value_type = String, example = "2025-01-27T12:00:00Z")]
    pub updated_at: DateTime<Utc>,
}

impl StoredDeployment {
    /// Create a new stored deployment from a spec
    pub fn new(spec: DeploymentSpec) -> Self {
        let now = Utc::now();
        Self {
            name: spec.deployment.clone(),
            spec,
            status: DeploymentStatus::Pending,
            created_at: now,
            updated_at: now,
        }
    }

    /// Update the deployment spec and timestamp
    pub fn update_spec(&mut self, spec: DeploymentSpec) {
        self.spec = spec;
        self.updated_at = Utc::now();
    }

    /// Update the deployment status and timestamp
    pub fn update_status(&mut self, status: DeploymentStatus) {
        self.status = status;
        self.updated_at = Utc::now();
    }
}

/// Deployment lifecycle status
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum DeploymentStatus {
    /// Deployment created but not yet started
    Pending,

    /// Deployment is being rolled out
    Deploying,

    /// All services are running
    Running,

    /// Deployment failed with an error message
    Failed {
        /// Error message describing the failure
        message: String,
    },

    /// Deployment has been stopped
    Stopped,
}

impl std::fmt::Display for DeploymentStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeploymentStatus::Pending => write!(f, "pending"),
            DeploymentStatus::Deploying => write!(f, "deploying"),
            DeploymentStatus::Running => write!(f, "running"),
            DeploymentStatus::Failed { message } => write!(f, "failed: {}", message),
            DeploymentStatus::Stopped => write!(f, "stopped"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use zlayer_spec::{DeploymentSpec, ImageSpec, ServiceSpec};

    fn create_test_spec(name: &str) -> DeploymentSpec {
        let mut services = HashMap::new();
        services.insert(
            "test-service".to_string(),
            ServiceSpec {
                rtype: zlayer_spec::ResourceType::Service,
                schedule: None,
                image: ImageSpec {
                    name: "test:latest".to_string(),
                    pull_policy: zlayer_spec::PullPolicy::IfNotPresent,
                },
                resources: Default::default(),
                env: Default::default(),
                command: Default::default(),
                network: Default::default(),
                endpoints: vec![],
                scale: Default::default(),
                depends: vec![],
                health: zlayer_spec::HealthSpec {
                    start_grace: None,
                    interval: None,
                    timeout: None,
                    retries: 3,
                    check: zlayer_spec::HealthCheck::Tcp { port: 8080 },
                },
                init: Default::default(),
                errors: Default::default(),
                devices: vec![],
                storage: vec![],
                capabilities: vec![],
                privileged: false,
                node_mode: Default::default(),
                node_selector: None,
                service_type: Default::default(),
                wasm_http: None,
            },
        );

        DeploymentSpec {
            version: "v1".to_string(),
            deployment: name.to_string(),
            services,
            tunnels: HashMap::new(),
        }
    }

    #[test]
    fn test_stored_deployment_new() {
        let spec = create_test_spec("test-deployment");
        let stored = StoredDeployment::new(spec.clone());

        assert_eq!(stored.name, "test-deployment");
        assert_eq!(stored.spec.deployment, spec.deployment);
        assert_eq!(stored.status, DeploymentStatus::Pending);
    }

    #[test]
    fn test_stored_deployment_update_status() {
        let spec = create_test_spec("test-deployment");
        let mut stored = StoredDeployment::new(spec);
        let original_updated = stored.updated_at;

        // Small delay to ensure timestamp changes
        std::thread::sleep(std::time::Duration::from_millis(10));

        stored.update_status(DeploymentStatus::Running);

        assert_eq!(stored.status, DeploymentStatus::Running);
        assert!(stored.updated_at > original_updated);
    }

    #[test]
    fn test_deployment_status_display() {
        assert_eq!(DeploymentStatus::Pending.to_string(), "pending");
        assert_eq!(DeploymentStatus::Deploying.to_string(), "deploying");
        assert_eq!(DeploymentStatus::Running.to_string(), "running");
        assert_eq!(DeploymentStatus::Stopped.to_string(), "stopped");
        assert_eq!(
            DeploymentStatus::Failed {
                message: "out of memory".to_string()
            }
            .to_string(),
            "failed: out of memory"
        );
    }

    #[test]
    fn test_deployment_status_serialize() {
        let status = DeploymentStatus::Failed {
            message: "connection refused".to_string(),
        };
        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("failed"));
        assert!(json.contains("connection refused"));

        let deserialized: DeploymentStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(status, deserialized);
    }
}
