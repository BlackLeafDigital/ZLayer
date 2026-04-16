//! Wire types for the Projects page.
//!
//! Mirrors `zlayer_api::storage::StoredProject` / `BuildKind` and the
//! request/response shapes in `zlayer_api::handlers::projects` and
//! `zlayer_api::handlers::webhooks`, but defined locally so the
//! hydrate/WASM build doesn't need to pull in `zlayer-api` (which is
//! SSR-only).
//!
//! Field shape was cross-checked against:
//! - `crates/zlayer-api/src/storage/mod.rs` — `StoredProject`,
//!   `BuildKind` (four variants: `Dockerfile`, `Compose`, `ZImagefile`,
//!   `Spec`, all `#[serde(rename_all = "snake_case")]`).
//! - `crates/zlayer-api/src/handlers/projects.rs` —
//!   `CreateProjectRequest`, `UpdateProjectRequest`,
//!   `ProjectPullResponse`.
//! - `crates/zlayer-api/src/handlers/webhooks.rs` —
//!   `WebhookInfoResponse` (fields: `url`, `secret`).
//! - `crates/zlayer-api/src/handlers/credentials.rs` —
//!   `RegistryCredentialResponse` and `GitCredentialResponse` for the
//!   attach-credential picker.
//!
//! `DateTime<Utc>` serialises to RFC-3339 on the wire, so we receive it
//! here as `String` round-trip-safe.

use serde::{Deserialize, Serialize};

/// A stored project as returned by `/api/v1/projects` and
/// `/api/v1/projects/{id}`.
///
/// Matches `zlayer_api::storage::StoredProject` exactly.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WireProject {
    /// UUID identifier.
    pub id: String,
    /// Project name (globally unique).
    pub name: String,
    /// Free-form description shown in the UI.
    #[serde(default)]
    pub description: Option<String>,
    /// Git repository URL.
    #[serde(default)]
    pub git_url: Option<String>,
    /// Git branch to build from (default: `"main"`).
    #[serde(default)]
    pub git_branch: Option<String>,
    /// Reference to a `GitCredential` id.
    #[serde(default)]
    pub git_credential_id: Option<String>,
    /// How the project is built (`"dockerfile" | "compose" | "zimagefile" | "spec"`).
    #[serde(default)]
    pub build_kind: Option<WireBuildKind>,
    /// Relative build path within the repo.
    #[serde(default)]
    pub build_path: Option<String>,
    /// Relative path inside the cloned repo to a `DeploymentSpec` YAML
    /// that the workflow `DeployProject` action should apply.
    #[serde(default)]
    pub deploy_spec_path: Option<String>,
    /// Reference to a `RegistryCredential` id.
    #[serde(default)]
    pub registry_credential_id: Option<String>,
    /// Reference to the default environment id.
    #[serde(default)]
    pub default_environment_id: Option<String>,
    /// Reference to the owning user.
    #[serde(default)]
    pub owner_id: Option<String>,
    /// Whether new commits automatically trigger build + deploy.
    #[serde(default)]
    pub auto_deploy: bool,
    /// If set, the daemon polls the remote for new commits every N
    /// seconds. `None` disables polling.
    #[serde(default)]
    pub poll_interval_secs: Option<u64>,
    /// RFC-3339 creation timestamp.
    pub created_at: String,
    /// RFC-3339 last-updated timestamp.
    pub updated_at: String,
}

/// How a project is built.
///
/// Mirrors `zlayer_api::storage::BuildKind` — the daemon emits these as
/// `"dockerfile" | "compose" | "zimagefile" | "spec"` on the wire via
/// `#[serde(rename_all = "snake_case")]`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WireBuildKind {
    /// Standard Dockerfile build.
    Dockerfile,
    /// Docker Compose / Compose file.
    Compose,
    /// `ZLayer`-native `ZImagefile`.
    #[serde(rename = "zimagefile")]
    ZImagefile,
    /// `ZLayer` deployment spec.
    Spec,
}

impl WireBuildKind {
    /// Return the snake-case discriminator used on the wire.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            WireBuildKind::Dockerfile => "dockerfile",
            WireBuildKind::Compose => "compose",
            WireBuildKind::ZImagefile => "zimagefile",
            WireBuildKind::Spec => "spec",
        }
    }

    /// Parse from the wire string. Returns `None` for an unknown value.
    #[must_use]
    pub fn parse_wire(s: &str) -> Option<Self> {
        match s {
            "dockerfile" => Some(WireBuildKind::Dockerfile),
            "compose" => Some(WireBuildKind::Compose),
            "zimagefile" => Some(WireBuildKind::ZImagefile),
            "spec" => Some(WireBuildKind::Spec),
            _ => None,
        }
    }
}

/// Request body for `POST /api/v1/projects` and (field-compatible)
/// `PATCH /api/v1/projects/{id}`.
///
/// Every field is optional — the create endpoint only requires `name`,
/// and the patch endpoint treats every absent field as "leave unchanged"
/// (except `poll_interval_secs`, which is a double-`Option` on the
/// daemon side: `None` = don't touch, `Some(None)` = clear polling,
/// `Some(Some(n))` = set polling). We don't model that distinction here
/// because the Manager UI always sends whatever the form's current
/// state says, which maps cleanly to "explicit value" on both paths.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct WireProjectSpec {
    /// Project name. Required on create; optional on patch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Free-form description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Git repository URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_url: Option<String>,
    /// Git branch to build from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_branch: Option<String>,
    /// Reference to a `GitCredential` id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_credential_id: Option<String>,
    /// How the project is built.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build_kind: Option<WireBuildKind>,
    /// Relative build path within the repo.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build_path: Option<String>,
    /// Relative path to the deploy-spec YAML.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deploy_spec_path: Option<String>,
    /// Reference to a `RegistryCredential` id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub registry_credential_id: Option<String>,
    /// Default environment id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_environment_id: Option<String>,
    /// Enable or disable automatic deploy on new commits.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_deploy: Option<bool>,
    /// Polling interval in seconds. When set to `Some(0)` or left at
    /// `None`, polling is disabled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub poll_interval_secs: Option<u64>,
}

/// Response body for `POST /api/v1/projects/{id}/pull`.
///
/// Mirrors `zlayer_api::handlers::projects::ProjectPullResponse`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WirePullResult {
    /// Project id the pull was performed for.
    pub project_id: String,
    /// Git URL that was cloned / fetched.
    pub git_url: String,
    /// Branch checked out in the working copy.
    pub branch: String,
    /// HEAD commit SHA after the pull.
    pub sha: String,
    /// Absolute path to the working copy on disk.
    pub path: String,
}

/// Response body for `GET /api/v1/projects/{id}/webhook` and
/// `POST /api/v1/projects/{id}/webhook/rotate`.
///
/// Mirrors `zlayer_api::handlers::webhooks::WebhookInfoResponse`. The
/// daemon does NOT currently expose a `last_triggered_at` field — the
/// webhook receiver just drives a pull and doesn't persist the
/// timestamp — so this mirror only has `url` + `secret`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WireWebhookInfo {
    /// Template URL to configure in the git host's webhook settings.
    /// Contains a `{provider}` placeholder the caller replaces with
    /// `github | gitea | forgejo | gitlab`.
    pub url: String,
    /// HMAC secret to paste into the git host.
    pub secret: String,
}

/// A credential (registry or git) that can be attached to a project.
///
/// The daemon has no single "project credentials" endpoint — instead
/// the UI merges the outputs of `GET /api/v1/credentials/registry` and
/// `GET /api/v1/credentials/git` into this shape for the picker. The
/// `id` is the underlying credential id that would be written to
/// `StoredProject::{registry,git}_credential_id`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WireProjectCredential {
    /// Underlying credential id.
    pub id: String,
    /// `"registry"` or `"git"`.
    pub kind: String,
    /// Human-readable label (registry name + username for registry
    /// creds; free-form name for git creds).
    pub name: String,
    /// Sub-kind. For `kind == "registry"` this is the auth type
    /// (`"basic" | "token"`); for `kind == "git"` it's the credential
    /// kind (`"pat" | "ssh_key"`).
    pub sub_kind: String,
}
