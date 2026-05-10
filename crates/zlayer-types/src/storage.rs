//! Storage `Stored*` wire types.
//!
//! These are the serde-friendly DTOs persisted by the daemon's `SqlxStorage`
//! backends and surfaced over the REST API. They live here (not in
//! `zlayer-api`) so SDK consumers can deserialize them without pulling in
//! axum/sqlx/tokio.
//!
//! Convenience constructors that allocate fresh UUIDs, plus the
//! database-bound traits and concrete sqlx implementations, remain in
//! `zlayer-api::storage` — that's where the `uuid` dependency lives. This
//! crate only carries the wire shapes (structs, enums, and pure-data
//! `Display` impls).

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::spec::DeploymentSpec;

// =========================================================================
// Deployments
// =========================================================================

/// A stored deployment with metadata.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
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
    /// Create a new stored deployment from a spec.
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

    /// Update the deployment spec and timestamp.
    pub fn update_spec(&mut self, spec: DeploymentSpec) {
        self.spec = spec;
        self.updated_at = Utc::now();
    }

    /// Update the deployment status and timestamp.
    pub fn update_status(&mut self, status: DeploymentStatus) {
        self.status = status;
        self.updated_at = Utc::now();
    }
}

/// Deployment lifecycle status.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, utoipa::ToSchema)]
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

// =========================================================================
// Users
// =========================================================================

/// A stored user account.
///
/// The password hash lives in `zlayer-secrets::CredentialStore` keyed by the
/// email address — NOT in this record. This type only carries user metadata.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
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
            id: Uuid::new_v4().to_string(),
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
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, utoipa::ToSchema)]
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

// =========================================================================
// Environments
// =========================================================================

/// A deployment/runtime environment (e.g. "dev", "staging", "prod").
///
/// Each environment is an isolated namespace for secrets and, later,
/// deployments. Optionally belongs to a `Project` (added in Phase 5) — when
/// `project_id` is `None`, the environment is global.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
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
            id: Uuid::new_v4().to_string(),
            name: name.into(),
            project_id,
            description: None,
            created_at: now,
            updated_at: now,
        }
    }
}

// =========================================================================
// Projects
// =========================================================================

/// A project bundles a git source, build configuration, registry credential
/// reference, linked deployments, and a default environment.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
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
            id: Uuid::new_v4().to_string(),
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
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, utoipa::ToSchema)]
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

// =========================================================================
// Variables
// =========================================================================

/// A stored variable — a plaintext key-value pair for template substitution
/// in deployment specs. Variables are NOT encrypted (unlike secrets). They
/// live in their own storage, separate from the encrypted secrets store.
///
/// Variables can be global (`scope = None`) or project-scoped
/// (`scope = Some(project_id)`).
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
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
            id: Uuid::new_v4().to_string(),
            name: name.into(),
            value: value.into(),
            scope,
            created_at: now,
            updated_at: now,
        }
    }
}

// =========================================================================
// Syncs
// =========================================================================

/// A stored sync resource (persistent record of a git-backed resource set).
///
/// A sync points at a directory within a project's checkout that contains
/// `ZLayer` resource YAMLs. On diff/apply the directory is scanned, compared
/// against current API state, and reconciled.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
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
            id: Uuid::new_v4().to_string(),
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

// =========================================================================
// Tasks
// =========================================================================

/// A stored task — a named runnable script that can be executed on demand.
///
/// Tasks can be global (`project_id = None`) or project-scoped
/// (`project_id = Some(project_id)`).
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
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
            id: Uuid::new_v4().to_string(),
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
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, utoipa::ToSchema)]
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
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
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

// =========================================================================
// Workflows
// =========================================================================

/// A stored workflow — a named sequence of steps forming a DAG that
/// composes tasks, project builds, deploys, and sync applies.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
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
            id: Uuid::new_v4().to_string(),
            name: name.into(),
            steps,
            project_id: None,
            created_at: now,
            updated_at: now,
        }
    }
}

/// A single step in a workflow.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
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
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
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
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, utoipa::ToSchema)]
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
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct StepResult {
    /// The step name.
    pub step_name: String,

    /// Step outcome: `"ok"`, `"failed"`, or `"skipped"`.
    pub status: String,

    /// Optional output or error message.
    pub output: Option<String>,
}

// =========================================================================
// Notifiers
// =========================================================================

/// A stored notifier — a named notification channel that fires alerts to
/// Slack, Discord, a generic webhook, or SMTP when triggered.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
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
            id: Uuid::new_v4().to_string(),
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
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, utoipa::ToSchema)]
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

/// Channel-specific configuration for a notifier.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
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

// =========================================================================
// User groups
// =========================================================================

/// A stored user group for role-based access control.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
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
            id: Uuid::new_v4().to_string(),
            name: name.into(),
            description: None,
            created_at: now,
            updated_at: now,
        }
    }
}

// =========================================================================
// Permissions
// =========================================================================

/// Whether a permission subject is a user or a group.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, utoipa::ToSchema)]
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
#[derive(
    Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, utoipa::ToSchema,
)]
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
///
/// Canonical `resource_kind` strings (kept here for grep-ability):
/// - `"environment"` — a `StoredEnvironment` row.
/// - `"deployment"` — a `StoredDeployment` row.
/// - `"project"` — a `StoredProject` row.
/// - `"secret"` — a row in the secrets store (`{scope}:{name}` keyed).
/// - `"node"` — a cluster member identified by `NodeIdentity::node_id`.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
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
            id: Uuid::new_v4().to_string(),
            subject_kind,
            subject_id: subject_id.into(),
            resource_kind: resource_kind.into(),
            resource_id,
            level,
            created_at: Utc::now(),
        }
    }
}

// =========================================================================
// OIDC identities
// =========================================================================

/// One OIDC identity link row.
///
/// One row is inserted the first time a user signs in via a given provider;
/// subsequent sign-ins look up the same row and reuse the linked `user_id`.
/// The uniqueness constraint on `(provider, subject)` enforces the
/// one-subject-one-user invariant.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct OidcIdentity {
    /// Surrogate id (uuid).
    pub id: String,
    /// `ZLayer` user account this identity resolves to.
    pub user_id: String,
    /// Provider slug matching `OidcProviderConfig::name`.
    pub provider: String,
    /// The `sub` claim from the provider's ID token. Opaque.
    pub subject: String,
    /// Email returned by the provider at link time (informational only).
    pub email_at_link: Option<String>,
    #[schema(value_type = String, format = DateTime)]
    pub created_at: DateTime<Utc>,
    #[schema(value_type = String, format = DateTime)]
    pub updated_at: DateTime<Utc>,
}

impl OidcIdentity {
    /// Convenience constructor — fills `id`, `created_at`, `updated_at`.
    #[must_use]
    pub fn new(
        user_id: impl Into<String>,
        provider: impl Into<String>,
        subject: impl Into<String>,
        email_at_link: Option<String>,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4().to_string(),
            user_id: user_id.into(),
            provider: provider.into(),
            subject: subject.into(),
            email_at_link,
            created_at: now,
            updated_at: now,
        }
    }
}

// =========================================================================
// Audit log
//
// `AuditEntry` is intentionally NOT moved here: it carries an
// `Option<serde_json::Value>` field, and `zlayer-types` does not depend on
// `serde_json`. Once a sibling migration adds the dep (or restructures the
// `details` field to a string), `AuditEntry` can move alongside the other
// `Stored*` types — it lives in `zlayer-api::storage` for now.
// =========================================================================

// =========================================================================
// Cluster-replicated secrets (Phase 1 — Raft + sealed-box DEK wrap)
// =========================================================================

/// Per-node identity and key material.
///
/// Each node generates an X25519 keypair on first start and publishes the
/// pubkey via `RegisterNode` during cluster join. Lives in Raft state so
/// every node knows the recipient set when wrapping the cluster DEK.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct NodeIdentity {
    /// Cluster-wide UUID for this node (matches Raft's view of the node).
    pub node_id: String,

    /// 32-byte X25519 pubkey used as the sealed-box recipient when the
    /// leader wraps the cluster DEK to this node.
    #[schema(value_type = String, format = "byte")]
    pub secrets_pubkey: [u8; 32],

    /// `WireGuard` pubkey, kept here for reference — overlay/tunnelling
    /// already track this elsewhere; duplicating it in `NodeIdentity` lets
    /// callers correlate sealed-box identity with overlay identity without
    /// a second lookup.
    pub wg_pubkey: String,

    /// When this node was registered with the cluster.
    #[schema(value_type = String, example = "2026-04-15T12:00:00Z")]
    pub joined_at: DateTime<Utc>,

    /// Soft-revocation timestamp. When set, the node no longer receives
    /// new wraps and must be excluded from `RotateDek` after a quorum
    /// confirms it has been physically removed.
    #[schema(value_type = Option<String>, example = "2026-04-16T12:00:00Z")]
    pub revoked_at: Option<DateTime<Utc>>,
}

/// The cluster data-encryption key (DEK), wrapped per-node so each member
/// can decrypt without ever holding a shared cluster-wide private key.
///
/// The DEK itself is never stored anywhere; only the per-node sealed-box
/// wraps live in Raft. A node decrypts its own wrap on startup using its
/// node X25519 private key, and holds the unwrapped DEK in zeroized memory.
///
/// Generation increments on every rotation (e.g. node revocation, scheduled
/// rotation, suspected compromise). Every `ReplicatedSecret` records the
/// `dek_generation` it was encrypted under so re-encrypts can be batched.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct WrappedDek {
    /// Monotonically increasing generation counter.
    pub dek_generation: u64,

    /// Map from `node_id` to that node's sealed-box-wrapped copy of the DEK.
    /// A node missing from this map cannot decrypt any secret encrypted
    /// under this generation and must be re-wrapped via `RegisterNode` (or
    /// through a `RotateDek` that includes it).
    pub wraps: std::collections::HashMap<String, Vec<u8>>,
}

/// A secret replicated through Raft. Every node has the same encrypted
/// blob; only nodes whose `secrets_pubkey` is in the current `WrappedDek`
/// for this generation can decrypt.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ReplicatedSecret {
    /// `"{scope}:{name}"` — same key shape used by `PersistentSecretsStore`.
    pub storage_key: String,

    /// XChaCha20-Poly1305 ciphertext of the plaintext value, encrypted
    /// under the cluster DEK at `dek_generation`. Nonce is prepended.
    pub ciphertext: Vec<u8>,

    /// Which DEK generation produced `ciphertext`. After a rotation, the
    /// state machine batches re-encrypts of every row whose `dek_generation`
    /// is older than current.
    pub dek_generation: u64,

    /// Standard secret metadata (name, version, timestamps).
    #[schema(value_type = Object)]
    pub metadata: crate::secrets::SecretMetadata,

    /// Optional per-secret affinity. `None` = any node may host (the
    /// default). When set, only matching nodes are entitled to a wrap of
    /// this row's DEK material; the API gate also filters reads accordingly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_affinity: Option<NodeAffinity>,
}

/// Constrains which nodes are allowed to host a given secret's
/// decryptable form. Used as the value of `ReplicatedSecret.node_affinity`.
/// `None` on a secret = unconstrained (any node may host); `Some(...)`
/// = only matching nodes receive a wrap of this row's DEK material,
/// and the API gate filters reads accordingly.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NodeAffinity {
    /// Explicit allow-list of `node_id`s.
    Nodes {
        /// The set of `node_id`s entitled to host this secret.
        node_ids: Vec<String>,
    },

    /// Match by node labels — every entry in the map must be present on
    /// the node. Empty map matches every node.
    Labels {
        /// Required label key/value pairs.
        labels: std::collections::HashMap<String, String>,
    },
}
