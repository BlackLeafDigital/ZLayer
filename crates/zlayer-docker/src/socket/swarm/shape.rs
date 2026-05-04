//! Docker Swarm wire-format types.
//!
//! These structs serialize to the JSON shape that Docker clients (Docker CLI,
//! docker-py, Portainer, etc.) expect from the Swarm endpoints. They are
//! **wire-only**: `ZLayer`'s internal primitives live in `zlayer-api`,
//! `zlayer-types`, `zlayer-secrets`, and `zlayer-scheduler`, and are mapped
//! into these shapes only at the HTTP boundary.
//!
//! ## Field rules
//!
//! Every struct uses `#[serde(rename_all = "PascalCase")]` because Docker's
//! wire format is uniformly `PascalCase`. Optional fields use `#[serde(default)]`
//! so deserializing a partial payload (e.g. a Docker CLI sending only
//! `Name` + `Labels`) succeeds. Fields whose semantics `ZLayer` does not
//! emulate are typed as [`serde_json::Value`] so we can echo the value
//! back unchanged when round-tripping; Docker clients treat unknown extras
//! permissively.
//!
//! ## What's wired up today
//!
//! Only the simplest `From` conversion lives here:
//!
//! * [`Node`] from a [`ClusterNodeSummary`] — direct field mapping.
//!
//! Service / Task / Secret / Config conversions are intentionally absent.
//! They require cross-resource lookups (a Swarm `Service` aggregates
//! `DeploymentSpec` + `ScaleSpec` + per-replica state; a `Task` joins a
//! service with a node placement record; secrets / configs are sourced
//! through a different crate dependency boundary). Endpoint files build
//! those payloads directly so the conversion logic stays close to the
//! data fetch — and so that `zlayer-docker` does not have to depend on
//! `zlayer-secrets` purely for a wire-shape conversion.
//!
//! ## Dead-code allow
//!
//! Most types in this module are skeletons: defined now so the wire shape
//! is captured in one place, populated later as endpoint families learn to
//! return real data. Rust's dead-code lint flags every still-unused type;
//! the module-level `allow` keeps clippy `-D warnings` green during the
//! foundation phase. As each endpoint file starts constructing values of
//! these types, the allow becomes a no-op — the lint simply stops firing.

#![allow(dead_code)]

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use zlayer_api::ClusterNodeSummary;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Format a Unix timestamp (seconds since epoch) as an RFC 3339 string.
///
/// Docker uses RFC 3339 for `CreatedAt` / `UpdatedAt` fields, not Unix
/// timestamps. Falls back to the epoch ("1970-01-01T00:00:00Z") if the
/// input is out of range — a safer default than panicking inside a handler.
#[must_use]
pub(crate) fn iso_timestamp(secs: i64) -> String {
    DateTime::<Utc>::from_timestamp(secs, 0).map_or_else(
        || "1970-01-01T00:00:00Z".to_string(),
        |dt| dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    )
}

/// Format a millisecond Unix timestamp (as used by [`ClusterNodeSummary`])
/// as an RFC 3339 string.
#[must_use]
pub(crate) fn iso_timestamp_ms(millis: u64) -> String {
    let secs = i64::try_from(millis / 1_000).unwrap_or(0);
    iso_timestamp(secs)
}

// ---------------------------------------------------------------------------
// /swarm
// ---------------------------------------------------------------------------

/// `Swarm` cluster snapshot returned by `GET /swarm`.
///
/// In a "swarm-inactive" daemon (which `ZLayer` always reports) this object
/// is never serialised — `GET /swarm` returns 503. The struct is included
/// here so that future bridges can populate it without redefining the shape.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct Swarm {
    #[serde(default, rename = "ID")]
    pub id: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
    #[serde(default)]
    pub spec: SwarmSpec,
    #[serde(default)]
    pub version: Version,
    #[serde(default)]
    pub root_rotation_in_progress: bool,
    #[serde(default)]
    pub join_tokens: JoinTokens,
}

/// Worker / manager join tokens.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct JoinTokens {
    #[serde(default)]
    pub worker: String,
    #[serde(default)]
    pub manager: String,
}

/// Swarm-wide configuration. Most fields are passed through as opaque JSON
/// because `ZLayer` does not interpret them.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct SwarmSpec {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub labels: serde_json::Value,
    #[serde(default)]
    pub orchestration: serde_json::Value,
    #[serde(default)]
    pub raft: serde_json::Value,
    #[serde(default)]
    pub dispatcher: serde_json::Value,
    #[serde(default, rename = "CAConfig")]
    pub ca_config: serde_json::Value,
    #[serde(default)]
    pub task_defaults: serde_json::Value,
    #[serde(default)]
    pub encryption_config: serde_json::Value,
}

/// Raft index counter Docker exposes alongside every cluster object.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct Version {
    #[serde(default)]
    pub index: u64,
}

/// Cluster info embedded in `GET /info` and the body of `GET /swarm`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct ClusterInfo {
    #[serde(default, rename = "ID")]
    pub id: String,
    #[serde(default)]
    pub version: Version,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
    #[serde(default)]
    pub spec: SwarmSpec,
    #[serde(default)]
    pub root_rotation_in_progress: bool,
    #[serde(default)]
    pub join_tokens: JoinTokens,
}

// ---------------------------------------------------------------------------
// /nodes
// ---------------------------------------------------------------------------

/// A Swarm node. Returned by `GET /nodes` and `GET /nodes/{id}`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct Node {
    #[serde(default, rename = "ID")]
    pub id: String,
    #[serde(default)]
    pub version: Version,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
    #[serde(default)]
    pub spec: NodeSpec,
    #[serde(default)]
    pub description: NodeDescription,
    #[serde(default)]
    pub status: NodeStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manager_status: Option<NodeManagerStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct NodeSpec {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub labels: serde_json::Value,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub availability: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct NodeDescription {
    #[serde(default)]
    pub hostname: String,
    #[serde(default)]
    pub platform: serde_json::Value,
    #[serde(default)]
    pub resources: serde_json::Value,
    #[serde(default)]
    pub engine: serde_json::Value,
    #[serde(default, rename = "TLSInfo")]
    pub tls_info: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct NodeStatus {
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub addr: String,
}

/// Manager-only metadata. Docker calls this `ManagerStatus`; the Rust name
/// is prefixed to avoid colliding with the worker-side `Status`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct NodeManagerStatus {
    #[serde(default)]
    pub leader: bool,
    #[serde(default)]
    pub reachability: String,
    #[serde(default)]
    pub addr: String,
}

impl From<&ClusterNodeSummary> for Node {
    fn from(n: &ClusterNodeSummary) -> Self {
        let role = if n.is_leader || n.role == "leader" || n.role == "voter" {
            "manager".to_string()
        } else {
            "worker".to_string()
        };
        let state = match n.status.as_str() {
            "ready" => "ready".to_string(),
            "draining" => "down".to_string(),
            other => other.to_string(),
        };
        Self {
            id: n.id.clone(),
            version: Version::default(),
            created_at: iso_timestamp_ms(n.registered_at),
            updated_at: iso_timestamp_ms(n.last_heartbeat),
            spec: NodeSpec {
                name: n.id.clone(),
                labels: serde_json::Value::Object(serde_json::Map::new()),
                role: role.clone(),
                availability: "active".to_string(),
            },
            description: NodeDescription {
                hostname: n.id.clone(),
                ..NodeDescription::default()
            },
            status: NodeStatus {
                state,
                message: String::new(),
                addr: n.advertise_addr.clone(),
            },
            manager_status: if role == "manager" {
                Some(NodeManagerStatus {
                    leader: n.is_leader,
                    reachability: "reachable".to_string(),
                    addr: n.address.clone(),
                })
            } else {
                None
            },
        }
    }
}

// ---------------------------------------------------------------------------
// /services
// ---------------------------------------------------------------------------

/// A Swarm service. Returned by `GET /services` and `GET /services/{id}`.
///
/// `From<&zlayer_spec::DeploymentSpec>` is intentionally not implemented:
/// constructing a `Service` requires both the spec and runtime placement
/// data, which the endpoint handlers fetch together. Endpoint files
/// build this struct directly.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct Service {
    #[serde(default, rename = "ID")]
    pub id: String,
    #[serde(default)]
    pub version: Version,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
    #[serde(default)]
    pub spec: ServiceSpec,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_spec: Option<ServiceSpec>,
    #[serde(default)]
    pub endpoint: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update_status: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct ServiceSpec {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub labels: serde_json::Value,
    #[serde(default)]
    pub task_template: TaskTemplate,
    #[serde(default)]
    pub mode: ServiceMode,
    #[serde(default)]
    pub update_config: serde_json::Value,
    #[serde(default)]
    pub rollback_config: serde_json::Value,
    #[serde(default)]
    pub networks: serde_json::Value,
    #[serde(default)]
    pub endpoint_spec: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct TaskTemplate {
    #[serde(default)]
    pub container_spec: ContainerSpec,
    #[serde(default)]
    pub resources: serde_json::Value,
    #[serde(default)]
    pub restart_policy: serde_json::Value,
    #[serde(default)]
    pub placement: serde_json::Value,
    #[serde(default)]
    pub networks: serde_json::Value,
    #[serde(default)]
    pub log_driver: serde_json::Value,
    #[serde(default)]
    pub force_update: u64,
    #[serde(default)]
    pub runtime: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct ContainerSpec {
    #[serde(default)]
    pub image: String,
    #[serde(default)]
    pub labels: serde_json::Value,
    #[serde(default)]
    pub command: Vec<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: Vec<String>,
    #[serde(default)]
    pub mounts: serde_json::Value,
}

/// Replicated vs global service mode. Docker emits exactly one of the two
/// keys, never both.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct ServiceMode {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replicated: Option<Replicated>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub global: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct Replicated {
    #[serde(default)]
    pub replicas: u64,
}

// ---------------------------------------------------------------------------
// /tasks
// ---------------------------------------------------------------------------

/// A Swarm task — one replica of a service running on one node.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct Task {
    #[serde(default, rename = "ID")]
    pub id: String,
    #[serde(default)]
    pub version: Version,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
    #[serde(default)]
    pub spec: TaskTemplate,
    #[serde(default, rename = "ServiceID")]
    pub service_id: String,
    #[serde(default)]
    pub slot: u64,
    #[serde(default, rename = "NodeID")]
    pub node_id: String,
    #[serde(default)]
    pub status: TaskStatus,
    #[serde(default)]
    pub desired_state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct TaskStatus {
    #[serde(default)]
    pub timestamp: String,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub container_status: serde_json::Value,
}

// ---------------------------------------------------------------------------
// /secrets
// ---------------------------------------------------------------------------

/// A Swarm secret. Returned by `GET /secrets` and `GET /secrets/{id}`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct Secret {
    #[serde(default, rename = "ID")]
    pub id: String,
    #[serde(default)]
    pub version: Version,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
    #[serde(default)]
    pub spec: SecretSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct SecretSpec {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub labels: serde_json::Value,
    /// Base64-encoded payload. Always empty on the wire — `ZLayer` does not
    /// expose secret material through Docker-compat endpoints.
    #[serde(default)]
    pub data: String,
    #[serde(default)]
    pub driver: serde_json::Value,
}

// ---------------------------------------------------------------------------
// /configs
// ---------------------------------------------------------------------------

/// A Swarm config. Same shape as [`Secret`] minus the (unused) Data field;
/// Docker keeps them as separate types.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct Config {
    #[serde(default, rename = "ID")]
    pub id: String,
    #[serde(default)]
    pub version: Version,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
    #[serde(default)]
    pub spec: ConfigSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct ConfigSpec {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub labels: serde_json::Value,
    /// Base64-encoded payload.
    #[serde(default)]
    pub data: String,
    #[serde(default)]
    pub templating: serde_json::Value,
}
