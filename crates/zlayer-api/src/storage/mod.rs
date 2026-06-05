//! Deployment storage traits and implementations
//!
//! This module provides persistent storage for deployment specifications using `SQLite`.
//!
//! The `Stored*` wire-shape types and their associated enums (`StoredDeployment`,
//! `StoredUser`, `DeploymentStatus`, `UserRole`, `BuildKind`, etc.) live in
//! `zlayer_types::storage` so SDK clients can deserialize them without
//! pulling in axum/sqlx/tokio. They are re-exported below for backward
//! compatibility with existing `use zlayer_api::storage::*` callers.
//!
//! # Example
//!
//! ```no_run
//! use zlayer_api::storage::{DeploymentStorage, SqlxStorage, StoredDeployment, DeploymentStatus};
//! use std::sync::Arc;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let storage = SqlxStorage::open("/tmp/deployments.db").await?;
//! let deployments = storage.list().await?;
//! # Ok(())
//! # }
//! ```

mod audit;
pub mod compose_projects;
mod deployments;
mod environments;
mod groups;
mod notifiers;
mod oidc_identities;
mod permissions;
mod projects;
mod sqlx_json;
pub mod standalone_container;
mod syncs;
mod tasks;
mod users;
mod variables;
mod workflows;

pub use audit::{AuditFilter, AuditStorage, InMemoryAuditStore, SqlxAuditStore};
pub use compose_projects::{
    ComposeProject, ComposeProjectStorage, InMemoryComposeProjectStorage,
    SqliteComposeProjectStorage,
};
pub use deployments::{DeploymentStorage, InMemoryStorage, SqlxStorage, StorageError};
pub use environments::{EnvironmentStorage, InMemoryEnvironmentStore, SqlxEnvironmentStore};
pub use groups::{GroupStorage, InMemoryGroupStore, SqlxGroupStore};
pub use notifiers::{InMemoryNotifierStore, NotifierStorage, SqlxNotifierStore};
pub use oidc_identities::{InMemoryOidcIdentityStore, OidcIdentityStorage, SqlxOidcIdentityStore};
pub use permissions::{InMemoryPermissionStore, PermissionStorage, SqlxPermissionStore};
pub use projects::{InMemoryProjectStore, ProjectStorage, SqlxProjectStore};
pub use sqlx_json::{IndexSpec, JsonTable, SqlxJsonStore};
pub use standalone_container::{
    InMemoryStandaloneContainerStorage, SqliteStandaloneContainerStorage,
    StandaloneContainerStorage,
};
pub use syncs::{InMemorySyncStore, SqlxSyncStore, SyncStorage};
pub use tasks::{InMemoryTaskStore, SqlxTaskStore, TaskStorage};
pub use users::{InMemoryUserStore, SqlxUserStore, UserStorage};
pub use variables::{InMemoryVariableStore, SqlxVariableStore, VariableStorage};
pub use workflows::{InMemoryWorkflowStore, SqlxWorkflowStore, WorkflowStorage};

// `Stored*` wire types now live in `zlayer-types`. Re-exported here so
// existing callers (`use zlayer_api::storage::StoredDeployment`) keep
// compiling. Database-bound traits, sqlx-backed stores, and `AuditEntry`
// (which still carries a `serde_json::Value` field) remain in this crate.
pub use zlayer_types::storage::{
    BuildKind, DeploymentStatus, NotifierConfig, NotifierKind, OidcIdentity, PermissionLevel,
    StepResult, StoredDeployment, StoredEnvironment, StoredNotifier, StoredPermission,
    StoredProject, StoredSync, StoredTask, StoredUser, StoredUserGroup, StoredVariable,
    StoredWorkflow, SubjectKind, TaskKind, TaskRun, UserRole, WorkflowAction, WorkflowRun,
    WorkflowRunStatus, WorkflowStep,
};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

// ---- Audit Log ----
//
// `AuditEntry` is intentionally NOT moved to `zlayer-types` yet: its
// `details: Option<serde_json::Value>` field requires `serde_json`, which
// `zlayer-types` does not depend on. Once `serde_json` is added there (or
// this field is restructured), `AuditEntry` will join the other
// `Stored*`-shaped DTOs.

/// A recorded audit log entry capturing who did what, when, and to which
/// resource.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct AuditEntry {
    /// UUID identifier of this audit entry.
    pub id: String,

    /// The user who performed the action.
    pub user_id: String,

    /// The action performed (e.g. `"create"`, `"update"`, `"delete"`,
    /// `"execute"`).
    pub action: String,

    /// The kind of resource acted upon (e.g. `"deployment"`, `"user"`,
    /// `"project"`).
    pub resource_kind: String,

    /// The specific resource id, if applicable.
    pub resource_id: Option<String>,

    /// Additional context as free-form JSON.
    #[schema(value_type = Option<Object>)]
    pub details: Option<serde_json::Value>,

    /// Client IP address, if known.
    pub ip: Option<String>,

    /// Client user-agent string, if known.
    pub user_agent: Option<String>,

    /// When the action occurred.
    #[schema(value_type = String, example = "2026-04-15T12:00:00Z")]
    pub created_at: DateTime<Utc>,
}

impl AuditEntry {
    /// Create a new audit entry with a fresh UUID and `created_at = now`.
    #[must_use]
    pub fn new(
        user_id: impl Into<String>,
        action: impl Into<String>,
        resource_kind: impl Into<String>,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            user_id: user_id.into(),
            action: action.into(),
            resource_kind: resource_kind.into(),
            resource_id: None,
            details: None,
            ip: None,
            user_agent: None,
            created_at: Utc::now(),
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
                    name: "test:latest".parse().expect("valid image reference"),
                    pull_policy: zlayer_spec::PullPolicy::IfNotPresent,
                },
                resources: zlayer_spec::ResourcesSpec::default(),
                env: HashMap::default(),
                command: zlayer_spec::CommandSpec::default(),
                network: zlayer_spec::ServiceNetworkSpec::default(),
                endpoints: vec![],
                scale: zlayer_spec::ScaleSpec::default(),
                replica_groups: None,
                depends: vec![],
                health: zlayer_spec::HealthSpec {
                    start_grace: None,
                    interval: None,
                    timeout: None,
                    retries: 3,
                    check: zlayer_spec::HealthCheck::Tcp { port: 8080 },
                },
                init: zlayer_spec::InitSpec::default(),
                errors: zlayer_spec::ErrorsSpec::default(),
                lifecycle: zlayer_spec::LifecycleSpec::default(),
                devices: vec![],
                storage: vec![],
                port_mappings: vec![],
                capabilities: vec![],
                cap_drop: vec![],
                privileged: false,
                node_mode: zlayer_spec::NodeMode::default(),
                node_selector: None,
                affinity: None,
                platform: None,
                service_type: zlayer_spec::ServiceType::default(),
                wasm: None,
                logs: None,
                host_network: false,
                hostname: None,
                dns: Vec::new(),
                extra_hosts: Vec::new(),
                restart_policy: None,
                labels: std::collections::HashMap::new(),
                user: None,
                stop_signal: None,
                stop_grace_period: None,
                sysctls: std::collections::HashMap::new(),
                ulimits: std::collections::HashMap::new(),
                security_opt: Vec::new(),
                pid_mode: None,
                ipc_mode: None,
                network_mode: zlayer_spec::NetworkMode::default(),
                extra_groups: Vec::new(),
                read_only_root_fs: false,
                init_container: None,
                tty: false,
                stdin_open: false,
                userns_mode: None,
                cgroup_parent: None,
                expose: Vec::new(),
                isolation: None,
                overlay: None,
                localhost_reachability: zlayer_spec::LocalhostReachability::default(),
            },
        );

        DeploymentSpec {
            version: "v1".to_string(),
            deployment: name.to_string(),
            services,
            externals: HashMap::new(),
            tunnels: HashMap::new(),
            api: zlayer_spec::ApiSpec::default(),
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
