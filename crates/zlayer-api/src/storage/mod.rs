//! Deployment storage traits and implementations
//!
//! This module provides persistent storage for deployment specifications using `SQLite`.
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
mod deployments;
mod environments;
mod groups;
mod notifiers;
mod permissions;
mod projects;
mod sqlx_json;
mod syncs;
mod tasks;
mod users;
mod variables;
mod workflows;

pub use audit::{AuditFilter, AuditStorage, InMemoryAuditStore, SqlxAuditStore};
pub use deployments::{DeploymentStorage, InMemoryStorage, SqlxStorage, StorageError};
pub use environments::{EnvironmentStorage, InMemoryEnvironmentStore, SqlxEnvironmentStore};
pub use groups::{GroupStorage, InMemoryGroupStore, SqlxGroupStore};
pub use notifiers::{InMemoryNotifierStore, NotifierStorage, SqlxNotifierStore};
pub use permissions::{InMemoryPermissionStore, PermissionStorage, SqlxPermissionStore};
pub use projects::{InMemoryProjectStore, ProjectStorage, SqlxProjectStore};
pub use sqlx_json::{IndexSpec, JsonTable, SqlxJsonStore};
pub use syncs::{InMemorySyncStore, SqlxSyncStore, SyncStorage};
pub use tasks::{InMemoryTaskStore, SqlxTaskStore, TaskStorage};
pub use users::{InMemoryUserStore, SqlxUserStore, UserStorage};
pub use variables::{InMemoryVariableStore, SqlxVariableStore, VariableStorage};
pub use workflows::{InMemoryWorkflowStore, SqlxWorkflowStore, WorkflowStorage};

use std::collections::HashMap;

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
    #[must_use]
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
            DeploymentStatus::Failed { message } => write!(f, "failed: {message}"),
            DeploymentStatus::Stopped => write!(f, "stopped"),
        }
    }
}

/// A stored user account.
///
/// The password hash lives in `zlayer-secrets::CredentialStore` keyed by the
/// email address — NOT in this record. This type only carries user metadata.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct StoredUser {
    /// Opaque user ID (`UUIDv4` string).
    pub id: String,

    /// Primary login identifier. Stored lower-cased.
    pub email: String,

    /// Human-readable display name.
    pub display_name: String,

    /// Role — "admin" or "user".
    pub role: UserRole,

    /// Whether the user can log in.
    pub is_active: bool,

    /// When the user was created.
    #[schema(value_type = String, example = "2026-04-15T12:00:00Z")]
    pub created_at: DateTime<Utc>,

    /// When the user was last updated.
    #[schema(value_type = String, example = "2026-04-15T12:00:00Z")]
    pub updated_at: DateTime<Utc>,

    /// When the user last logged in (if ever).
    #[schema(value_type = Option<String>, example = "2026-04-15T12:00:00Z")]
    pub last_login_at: Option<DateTime<Utc>>,
}

impl StoredUser {
    /// Create a new user record with a fresh UUID and `is_active = true`.
    #[must_use]
    pub fn new(email: impl Into<String>, display_name: impl Into<String>, role: UserRole) -> Self {
        let now = Utc::now();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            email: email.into().to_lowercase(),
            display_name: display_name.into(),
            role,
            is_active: true,
            created_at: now,
            updated_at: now,
            last_login_at: None,
        }
    }

    /// Record a successful login, advancing `last_login_at` and `updated_at`.
    pub fn touch_login(&mut self) {
        let now = Utc::now();
        self.last_login_at = Some(now);
        self.updated_at = now;
    }
}

/// User role. Admins can do everything; regular users are constrained by
/// per-resource permissions (added in a later phase).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum UserRole {
    /// Full administrative access.
    Admin,
    /// Standard user constrained by per-resource permissions.
    User,
}

impl UserRole {
    /// Stable string form of this role.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            UserRole::Admin => "admin",
            UserRole::User => "user",
        }
    }
}

impl std::fmt::Display for UserRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A deployment/runtime environment (e.g. "dev", "staging", "prod").
///
/// Each environment is an isolated namespace for secrets and, later,
/// deployments. Optionally belongs to a `Project` (added in Phase 5) — when
/// `project_id` is `None`, the environment is global.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct StoredEnvironment {
    /// UUID identifier.
    pub id: String,

    /// Display name (e.g. "dev"). Unique within a given `project_id`.
    pub name: String,

    /// Project id this environment belongs to. `None` = global.
    pub project_id: Option<String>,

    /// Free-form description shown in the UI.
    pub description: Option<String>,

    /// When the environment was created.
    #[schema(value_type = String, example = "2026-04-15T12:00:00Z")]
    pub created_at: DateTime<Utc>,

    /// When the environment was last updated.
    #[schema(value_type = String, example = "2026-04-15T12:00:00Z")]
    pub updated_at: DateTime<Utc>,
}

impl StoredEnvironment {
    /// Create a new environment record with a fresh UUID.
    #[must_use]
    pub fn new(name: impl Into<String>, project_id: Option<String>) -> Self {
        let now = Utc::now();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.into(),
            project_id,
            description: None,
            created_at: now,
            updated_at: now,
        }
    }
}

/// A project bundles a git source, build configuration, registry credential
/// reference, linked deployments, and a default environment.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct StoredProject {
    /// UUID identifier.
    pub id: String,

    /// Project name (globally unique).
    pub name: String,

    /// Free-form description shown in the UI.
    pub description: Option<String>,

    /// Git repository URL (e.g. `"https://github.com/user/repo"`).
    pub git_url: Option<String>,

    /// Git branch to build from (default: `"main"`).
    pub git_branch: Option<String>,

    /// Reference to a `GitCredential` (Phase 5.2).
    pub git_credential_id: Option<String>,

    /// How the project is built.
    pub build_kind: Option<BuildKind>,

    /// Relative path within the repo (e.g. `"./Dockerfile"`).
    pub build_path: Option<String>,

    /// Relative path (inside the cloned repo) to a `DeploymentSpec` YAML that
    /// the workflow `DeployProject` action should apply.
    ///
    /// When `None`, the workflow `DeployProject` action fails with a clear
    /// "no deploy spec configured" error rather than silently succeeding —
    /// callers are expected to set this explicitly via `project edit`.
    #[serde(default)]
    pub deploy_spec_path: Option<String>,

    /// Reference to a `RegistryCredential` (Phase 5.2).
    pub registry_credential_id: Option<String>,

    /// Reference to the default environment for this project.
    pub default_environment_id: Option<String>,

    /// Reference to the owning user.
    pub owner_id: Option<String>,

    /// Whether new commits on the tracked branch should automatically
    /// trigger a build + deploy cycle.
    #[serde(default)]
    pub auto_deploy: bool,

    /// If set, the daemon polls the remote for new commits every N seconds.
    /// `None` disables polling (the project is only updated via manual pull
    /// or webhook).
    #[serde(default)]
    pub poll_interval_secs: Option<u64>,

    /// When the project was created.
    #[schema(value_type = String, example = "2026-04-15T12:00:00Z")]
    pub created_at: DateTime<Utc>,

    /// When the project was last updated.
    #[schema(value_type = String, example = "2026-04-15T12:00:00Z")]
    pub updated_at: DateTime<Utc>,
}

impl StoredProject {
    /// Create a new project record with a fresh UUID.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.into(),
            description: None,
            git_url: None,
            git_branch: Some("main".to_string()),
            git_credential_id: None,
            build_kind: None,
            build_path: None,
            deploy_spec_path: None,
            registry_credential_id: None,
            default_environment_id: None,
            owner_id: None,
            auto_deploy: false,
            poll_interval_secs: None,
            created_at: now,
            updated_at: now,
        }
    }
}

/// How a project is built.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum BuildKind {
    /// Standard Dockerfile build.
    Dockerfile,
    /// Docker Compose / Compose file.
    Compose,
    /// `ZLayer`-native `ZImagefile`.
    ZImagefile,
    /// `ZLayer` deployment spec.
    Spec,
}

impl std::fmt::Display for BuildKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BuildKind::Dockerfile => f.write_str("dockerfile"),
            BuildKind::Compose => f.write_str("compose"),
            BuildKind::ZImagefile => f.write_str("zimagefile"),
            BuildKind::Spec => f.write_str("spec"),
        }
    }
}

/// A stored variable — a plaintext key-value pair for template substitution
/// in deployment specs. Variables are NOT encrypted (unlike secrets). They
/// live in their own storage, separate from the encrypted secrets store.
///
/// Variables can be global (`scope = None`) or project-scoped
/// (`scope = Some(project_id)`).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct StoredVariable {
    /// UUID identifier.
    pub id: String,

    /// Variable name (e.g. `"APP_VERSION"`, `"LOG_LEVEL"`). Unique within a
    /// given scope.
    pub name: String,

    /// Plaintext variable value.
    pub value: String,

    /// Scope: project id or `None` for global.
    pub scope: Option<String>,

    /// When the variable was created.
    #[schema(value_type = String, example = "2026-04-15T12:00:00Z")]
    pub created_at: DateTime<Utc>,

    /// When the variable was last updated.
    #[schema(value_type = String, example = "2026-04-15T12:00:00Z")]
    pub updated_at: DateTime<Utc>,
}

impl StoredVariable {
    /// Create a new variable record with a fresh UUID.
    #[must_use]
    pub fn new(name: impl Into<String>, value: impl Into<String>, scope: Option<String>) -> Self {
        let now = Utc::now();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.into(),
            value: value.into(),
            scope,
            created_at: now,
            updated_at: now,
        }
    }
}

/// A stored sync resource (persistent record of a git-backed resource set).
///
/// A sync points at a directory within a project's checkout that contains
/// `ZLayer` resource YAMLs. On diff/apply the directory is scanned, compared
/// against current API state, and reconciled.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct StoredSync {
    /// UUID identifier.
    pub id: String,

    /// Display name for this sync.
    pub name: String,

    /// Linked project id (the git checkout to scan).
    pub project_id: Option<String>,

    /// Path within the project's checkout to scan for resource YAMLs.
    pub git_path: String,

    /// Whether the sync should automatically apply on pull.
    #[serde(default)]
    pub auto_apply: bool,

    /// Whether the sync apply should delete resources that are present on the
    /// API but missing from the manifest directory. Defaults to `false` —
    /// the safer behaviour, which skips deletes and only creates/updates.
    #[serde(default)]
    pub delete_missing: bool,

    /// The commit SHA at which this sync was last applied.
    pub last_applied_sha: Option<String>,

    /// When the sync was created.
    #[schema(value_type = String, example = "2026-04-15T12:00:00Z")]
    pub created_at: DateTime<Utc>,

    /// When the sync was last updated.
    #[schema(value_type = String, example = "2026-04-15T12:00:00Z")]
    pub updated_at: DateTime<Utc>,
}

impl StoredSync {
    /// Create a new sync record with a fresh UUID.
    #[must_use]
    pub fn new(name: impl Into<String>, git_path: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.into(),
            project_id: None,
            git_path: git_path.into(),
            auto_apply: false,
            delete_missing: false,
            last_applied_sha: None,
            created_at: now,
            updated_at: now,
        }
    }
}

/// A stored task — a named runnable script that can be executed on demand.
///
/// Tasks can be global (`project_id = None`) or project-scoped
/// (`project_id = Some(project_id)`).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct StoredTask {
    /// UUID identifier.
    pub id: String,

    /// Task name.
    pub name: String,

    /// Script type.
    pub kind: TaskKind,

    /// The script/command body.
    pub body: String,

    /// Project id this task belongs to. `None` = global.
    pub project_id: Option<String>,

    /// When the task was created.
    #[schema(value_type = String, example = "2026-04-15T12:00:00Z")]
    pub created_at: DateTime<Utc>,

    /// When the task was last updated.
    #[schema(value_type = String, example = "2026-04-15T12:00:00Z")]
    pub updated_at: DateTime<Utc>,
}

impl StoredTask {
    /// Create a new task record with a fresh UUID.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        kind: TaskKind,
        body: impl Into<String>,
        project_id: Option<String>,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.into(),
            kind,
            body: body.into(),
            project_id,
            created_at: now,
            updated_at: now,
        }
    }
}

/// Script type for a task.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    /// A bash shell script executed via `sh -c`.
    Bash,
}

impl std::fmt::Display for TaskKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TaskKind::Bash => f.write_str("bash"),
        }
    }
}

/// A recorded execution of a task.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct TaskRun {
    /// UUID identifier of this run.
    pub id: String,

    /// The task that was executed.
    pub task_id: String,

    /// Process exit code (`None` if the task could not be started).
    pub exit_code: Option<i32>,

    /// Captured standard output.
    pub stdout: String,

    /// Captured standard error.
    pub stderr: String,

    /// When the run started.
    #[schema(value_type = String, example = "2026-04-15T12:00:00Z")]
    pub started_at: DateTime<Utc>,

    /// When the run finished (`None` if it has not finished yet).
    #[schema(value_type = Option<String>, example = "2026-04-15T12:00:01Z")]
    pub finished_at: Option<DateTime<Utc>>,
}

/// A stored workflow — a named sequence of steps forming a DAG that
/// composes tasks, project builds, deploys, and sync applies.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct StoredWorkflow {
    /// UUID identifier.
    pub id: String,

    /// Workflow name.
    pub name: String,

    /// Ordered list of steps to execute sequentially.
    pub steps: Vec<WorkflowStep>,

    /// Optional project scope.
    pub project_id: Option<String>,

    /// When the workflow was created.
    #[schema(value_type = String, example = "2026-04-15T12:00:00Z")]
    pub created_at: DateTime<Utc>,

    /// When the workflow was last updated.
    #[schema(value_type = String, example = "2026-04-15T12:00:00Z")]
    pub updated_at: DateTime<Utc>,
}

impl StoredWorkflow {
    /// Create a new workflow record with a fresh UUID.
    #[must_use]
    pub fn new(name: impl Into<String>, steps: Vec<WorkflowStep>) -> Self {
        let now = Utc::now();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.into(),
            steps,
            project_id: None,
            created_at: now,
            updated_at: now,
        }
    }
}

/// A single step in a workflow.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct WorkflowStep {
    /// Step name (display label).
    pub name: String,

    /// The action to perform.
    pub action: WorkflowAction,

    /// Name of another step (or task id) to run on failure.
    #[serde(default)]
    pub on_failure: Option<String>,
}

/// The action a workflow step performs.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkflowAction {
    /// Execute a task by id.
    RunTask {
        /// The task id to run.
        task_id: String,
    },
    /// Build a project by id.
    BuildProject {
        /// The project id to build.
        project_id: String,
    },
    /// Deploy a project by id.
    DeployProject {
        /// The project id to deploy.
        project_id: String,
    },
    /// Apply a sync resource by id.
    ApplySync {
        /// The sync id to apply.
        sync_id: String,
    },
}

/// A recorded execution of a workflow.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct WorkflowRun {
    /// UUID identifier of this run.
    pub id: String,

    /// The workflow that was executed.
    pub workflow_id: String,

    /// Overall run status.
    pub status: WorkflowRunStatus,

    /// Per-step results.
    pub step_results: Vec<StepResult>,

    /// When the run started.
    #[schema(value_type = String, example = "2026-04-15T12:00:00Z")]
    pub started_at: DateTime<Utc>,

    /// When the run finished (`None` if still running).
    #[schema(value_type = Option<String>, example = "2026-04-15T12:00:01Z")]
    pub finished_at: Option<DateTime<Utc>>,
}

/// Overall status of a workflow run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowRunStatus {
    /// Not yet started.
    Pending,
    /// Currently executing.
    Running,
    /// All steps completed successfully.
    Completed,
    /// A step failed.
    Failed,
}

impl std::fmt::Display for WorkflowRunStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorkflowRunStatus::Pending => f.write_str("pending"),
            WorkflowRunStatus::Running => f.write_str("running"),
            WorkflowRunStatus::Completed => f.write_str("completed"),
            WorkflowRunStatus::Failed => f.write_str("failed"),
        }
    }
}

/// Result of executing a single step in a workflow run.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct StepResult {
    /// The step name.
    pub step_name: String,

    /// Step outcome: `"ok"`, `"failed"`, or `"skipped"`.
    pub status: String,

    /// Optional output or error message.
    pub output: Option<String>,
}

/// A stored notifier — a named notification channel that fires alerts to
/// Slack, Discord, a generic webhook, or SMTP when triggered.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct StoredNotifier {
    /// UUID identifier.
    pub id: String,

    /// Display name (e.g. `"deploy-alerts"`).
    pub name: String,

    /// Notification channel type.
    pub kind: NotifierKind,

    /// Channel-specific configuration (webhook URL, SMTP settings, etc.).
    pub config: NotifierConfig,

    /// Whether this notifier is active. Disabled notifiers are skipped.
    pub enabled: bool,

    /// When the notifier was created.
    #[schema(value_type = String, example = "2026-04-15T12:00:00Z")]
    pub created_at: DateTime<Utc>,

    /// When the notifier was last updated.
    #[schema(value_type = String, example = "2026-04-15T12:00:00Z")]
    pub updated_at: DateTime<Utc>,
}

impl StoredNotifier {
    /// Create a new notifier record with a fresh UUID and `enabled = true`.
    #[must_use]
    pub fn new(name: impl Into<String>, kind: NotifierKind, config: NotifierConfig) -> Self {
        let now = Utc::now();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.into(),
            kind,
            config,
            enabled: true,
            created_at: now,
            updated_at: now,
        }
    }
}

/// Notification channel type.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum NotifierKind {
    /// Slack incoming webhook.
    Slack,
    /// Discord webhook.
    Discord,
    /// Generic HTTP webhook.
    Webhook,
    /// SMTP email.
    Smtp,
}

impl std::fmt::Display for NotifierKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NotifierKind::Slack => f.write_str("slack"),
            NotifierKind::Discord => f.write_str("discord"),
            NotifierKind::Webhook => f.write_str("webhook"),
            NotifierKind::Smtp => f.write_str("smtp"),
        }
    }
}

// ---- User Groups ----

/// A stored user group for role-based access control.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct StoredUserGroup {
    /// UUID identifier.
    pub id: String,

    /// Group name.
    pub name: String,

    /// Free-form description.
    pub description: Option<String>,

    /// When the group was created.
    #[schema(value_type = String, example = "2026-04-15T12:00:00Z")]
    pub created_at: DateTime<Utc>,

    /// When the group was last updated.
    #[schema(value_type = String, example = "2026-04-15T12:00:00Z")]
    pub updated_at: DateTime<Utc>,
}

impl StoredUserGroup {
    /// Create a new user group with a fresh UUID.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.into(),
            description: None,
            created_at: now,
            updated_at: now,
        }
    }
}

// ---- Permissions ----

/// Whether a permission subject is a user or a group.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SubjectKind {
    /// A single user.
    User,
    /// A user group.
    Group,
}

impl std::fmt::Display for SubjectKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SubjectKind::User => f.write_str("user"),
            SubjectKind::Group => f.write_str("group"),
        }
    }
}

/// Access level for a resource permission, ordered from least to most
/// privilege.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PermissionLevel {
    /// No access.
    None,
    /// Read-only access.
    Read,
    /// Execute (e.g. deploy, run tasks) in addition to read.
    Execute,
    /// Full read/write/execute access.
    Write,
}

impl std::fmt::Display for PermissionLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PermissionLevel::None => f.write_str("none"),
            PermissionLevel::Read => f.write_str("read"),
            PermissionLevel::Execute => f.write_str("execute"),
            PermissionLevel::Write => f.write_str("write"),
        }
    }
}

/// A stored permission grant binding a subject (user or group) to a resource
/// with a specific access level.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct StoredPermission {
    /// UUID identifier of this permission grant.
    pub id: String,

    /// Whether the subject is a user or a group.
    pub subject_kind: SubjectKind,

    /// The user or group id.
    pub subject_id: String,

    /// The kind of resource (e.g. `"deployment"`, `"project"`, `"secret"`).
    pub resource_kind: String,

    /// A specific resource id, or `None` for a wildcard (all resources of
    /// that kind).
    pub resource_id: Option<String>,

    /// The granted access level.
    pub level: PermissionLevel,

    /// When the permission was created.
    #[schema(value_type = String, example = "2026-04-15T12:00:00Z")]
    pub created_at: DateTime<Utc>,
}

impl StoredPermission {
    /// Create a new permission grant with a fresh UUID.
    #[must_use]
    pub fn new(
        subject_kind: SubjectKind,
        subject_id: impl Into<String>,
        resource_kind: impl Into<String>,
        resource_id: Option<String>,
        level: PermissionLevel,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            subject_kind,
            subject_id: subject_id.into(),
            resource_kind: resource_kind.into(),
            resource_id,
            level,
            created_at: Utc::now(),
        }
    }
}

// ---- Audit Log ----

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

/// Channel-specific configuration for a notifier.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NotifierConfig {
    /// Slack incoming webhook configuration.
    Slack {
        /// Slack webhook URL.
        webhook_url: String,
    },
    /// Discord webhook configuration.
    Discord {
        /// Discord webhook URL.
        webhook_url: String,
    },
    /// Generic HTTP webhook configuration.
    Webhook {
        /// Target URL.
        url: String,
        /// HTTP method (defaults to `"POST"`).
        #[serde(default)]
        method: Option<String>,
        /// Extra headers to send with the request.
        #[serde(default)]
        headers: Option<HashMap<String, String>>,
    },
    /// SMTP email configuration.
    Smtp {
        /// SMTP server host.
        host: String,
        /// SMTP server port.
        port: u16,
        /// SMTP username.
        username: String,
        /// SMTP password.
        password: String,
        /// Sender email address.
        from: String,
        /// Recipient email addresses.
        to: Vec<String>,
    },
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
                resources: zlayer_spec::ResourcesSpec::default(),
                env: HashMap::default(),
                command: zlayer_spec::CommandSpec::default(),
                network: zlayer_spec::ServiceNetworkSpec::default(),
                endpoints: vec![],
                scale: zlayer_spec::ScaleSpec::default(),
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
                devices: vec![],
                storage: vec![],
                capabilities: vec![],
                privileged: false,
                node_mode: zlayer_spec::NodeMode::default(),
                node_selector: None,
                service_type: zlayer_spec::ServiceType::default(),
                wasm: None,
                logs: None,
                host_network: false,
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
