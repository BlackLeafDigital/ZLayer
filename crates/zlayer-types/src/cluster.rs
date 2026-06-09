//! Daemon-level cluster mode selection.
//!
//! Defines how a `ZLayer` daemon participates in (or doesn't) cluster
//! membership. This is the top-level config the daemon reads at startup
//! to decide which `Cluster` trait implementation to construct.
//!
//! For the wire-level join/membership DTOs see [`crate::api::cluster`].

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::Duration;

/// How the daemon participates in (or doesn't) cluster membership.
///
/// The `WorkerTier` variant is intentionally large (~285 bytes) because it
/// carries the full server-role + worker-role config inline. `ClusterMode`
/// is parsed once at daemon startup and lives in an `Arc<DaemonConfig>` for
/// the process lifetime, so the size delta is irrelevant in practice — and
/// boxing it would force every caller of `is_worker_tier_server()` /
/// `adaptive_ttl_config()` through an extra indirection for no win.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(tag = "mode", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ClusterMode {
    /// Single-node daemon. No peers, no consensus. `is_leader()` is always
    /// true. Suitable for development and single-host deployments.
    #[default]
    #[serde(rename = "single-node")]
    SingleNode,

    /// openraft-backed consensus across the configured peers. This is the
    /// existing production mode for multi-node deployments.
    Raft {
        /// This node's id (must be unique within the cluster).
        node_id: u64,
        /// Peer addresses (raft RPC ports). The daemon's own entry must
        /// be present.
        peers: Vec<RaftPeer>,
    },

    /// Static-membership cluster: config-driven peer list, deterministic
    /// leader (lowest healthy node id), HTTP heartbeats for liveness.
    /// No consensus log — concurrent writers may race; the per-service
    /// scale semaphore in `ServiceManager` is the primary mitigation.
    Static {
        /// This node's id (must be unique within the cluster).
        node_id: u64,
        /// All cluster peers including self.
        peers: Vec<StaticPeer>,
        /// Heartbeat probe interval. Default 5s.
        #[serde(default = "default_heartbeat_interval", with = "duration_secs")]
        heartbeat_interval: Duration,
        /// Time after which a peer with no heartbeat is `Unreachable`.
        /// Default 15s (3x interval).
        #[serde(default = "default_failure_threshold", with = "duration_secs")]
        failure_threshold: Duration,
    },

    /// Nomad-style worker tier: 3–7 control-plane nodes run Raft consensus;
    /// up to ~10,000 worker nodes join as gRPC clients with adaptive-TTL
    /// heartbeats and never enter consensus.
    ///
    /// Workers are issued mTLS leaf certs by the cluster's worker CA during
    /// `Register`. Heartbeat cadence scales with cluster size — every
    /// `StatusAck` carries the next TTL computed from
    /// `clamp(N_workers / max_heartbeats_per_second, min_ttl, max_ttl)`.
    #[serde(rename = "worker-tier")]
    WorkerTier {
        /// What role THIS node plays.
        role: WorkerTierRole,

        /// Server-only: this node's id within the raft control plane.
        /// Required when role == Server; ignored on workers (assigned by
        /// leader during Register).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        node_id: Option<u64>,

        /// Server-only: the raft control-plane peer list (3-7 nodes).
        /// Required when role == Server.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        peers: Vec<RaftPeer>,

        /// Server-only: address to bind the worker-facing gRPC server.
        /// Default `0.0.0.0:3670` (the API server uses 3669; gRPC takes 3670).
        #[serde(
            default = "default_worker_grpc_addr",
            skip_serializing_if = "is_default_worker_grpc_addr"
        )]
        worker_grpc_addr: SocketAddr,

        /// Worker-only: control-plane gRPC endpoints to try (round-robin
        /// fallback). Required when role == Worker.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        servers: Vec<String>,

        /// Worker-only: path to the bootstrap token file (single line,
        /// URL-safe-base64 of a `WorkerBootstrapToken`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        token_file: Option<String>,

        /// Worker-only: directory to persist mTLS identity (cert.pem, key.pem,
        /// ca.pem). Defaults to `<data_dir>/worker/identity/` set by
        /// `ZLayerDirs`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        identity_dir: Option<String>,

        /// Server-only: worker CA storage directory. Defaults to
        /// `<data_dir>/cluster/` (same as the existing cluster CA + signer).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        worker_ca_dir: Option<String>,

        /// Shared: minimum heartbeat TTL (default 10s).
        #[serde(default = "default_min_ttl", with = "duration_secs")]
        heartbeat_min_ttl: Duration,

        /// Shared: maximum heartbeat TTL (default 10min).
        #[serde(default = "default_max_ttl", with = "duration_secs")]
        heartbeat_max_ttl: Duration,

        /// Shared: grace period beyond TTL before a worker's lease is
        /// considered expired. Default 10s.
        #[serde(default = "default_grace", with = "duration_secs")]
        heartbeat_grace: Duration,

        /// Server-only: cluster-wide cap on heartbeats/second the leader is
        /// willing to absorb. The leader hands every worker a TTL such that
        /// total HB rate ≤ this. Default 50 (Nomad's default).
        #[serde(default = "default_max_hb")]
        max_heartbeats_per_second: u32,

        /// Server-only: TTL applied immediately after a leader election so
        /// workers don't all expire while the new leader is bootstrapping
        /// its FSM. Default 5min.
        #[serde(default = "default_failover_ttl", with = "duration_secs")]
        failover_heartbeat_ttl: Duration,

        /// Free-form node labels (placement, selector matching, etc.).
        #[serde(default, skip_serializing_if = "HashMap::is_empty")]
        labels: HashMap<String, String>,
    },
}

/// What role this node plays in a worker-tier cluster.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerTierRole {
    /// Participates in raft consensus + serves the worker gRPC.
    Server,
    /// gRPC client to the control plane; never enters consensus.
    Worker,
}

impl ClusterMode {
    /// Extract an `AdaptiveTtlConfig` from a `WorkerTier` variant. Returns
    /// `None` for other modes.
    #[must_use]
    pub fn adaptive_ttl_config(&self) -> Option<AdaptiveTtlConfig> {
        if let ClusterMode::WorkerTier {
            heartbeat_min_ttl,
            heartbeat_max_ttl,
            heartbeat_grace,
            max_heartbeats_per_second,
            failover_heartbeat_ttl,
            ..
        } = self
        {
            Some(AdaptiveTtlConfig {
                min_ttl_secs: u32::try_from(heartbeat_min_ttl.as_secs()).unwrap_or(u32::MAX),
                max_ttl_secs: u32::try_from(heartbeat_max_ttl.as_secs()).unwrap_or(u32::MAX),
                grace_secs: u32::try_from(heartbeat_grace.as_secs()).unwrap_or(u32::MAX),
                max_heartbeats_per_second: *max_heartbeats_per_second,
                failover_ttl_secs: u32::try_from(failover_heartbeat_ttl.as_secs())
                    .unwrap_or(u32::MAX),
            })
        } else {
            None
        }
    }

    /// Convenience: is this a worker-tier server-role config?
    #[must_use]
    pub fn is_worker_tier_server(&self) -> bool {
        matches!(
            self,
            ClusterMode::WorkerTier {
                role: WorkerTierRole::Server,
                ..
            }
        )
    }

    /// Convenience: is this a worker-tier worker-role config?
    #[must_use]
    pub fn is_worker_tier_worker(&self) -> bool {
        matches!(
            self,
            ClusterMode::WorkerTier {
                role: WorkerTierRole::Worker,
                ..
            }
        )
    }
}

/// A raft peer's identity and reachability.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RaftPeer {
    pub id: u64,
    /// Raft RPC address (host:port).
    pub raft_addr: SocketAddr,
    /// HTTP API address advertised to other cluster members.
    pub api_addr: SocketAddr,
}

/// One container's summary for cluster-wide listing/aggregation, tagged with the
/// node it runs on. Wire type shared by the agent (builds the local view), the
/// scheduler's `Cluster` fan-out, and the API
/// (`GET /internal/services/{svc}/state` + `list_containers`), so the leader can
/// report replicas placed on remote nodes (distributed scaling).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClusterContainerSummary {
    /// Raft id of the node this container runs on.
    pub node_id: u64,
    /// Full container id (`service-replica`).
    pub id: String,
    /// Service name.
    pub service: String,
    /// Replica index.
    pub replica: u32,
    /// Image reference.
    pub image: String,
    /// Lowercased lifecycle state (e.g. `"running"`).
    pub state: String,
    /// Process id, when running.
    pub pid: Option<u32>,
    /// Overlay IP, when assigned.
    pub overlay_ip: Option<String>,
}

/// A single node's view of one service: how many replicas it runs **locally**,
/// whether they're healthy there, and their containers. The leader aggregates
/// one of these per node (its own local view + remote views fetched via the
/// `Cluster` fan-out) to compute cluster-wide replica count, health, and the
/// `ps` container listing for distributed services.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeServiceState {
    /// Raft id of the reporting node.
    pub node_id: u64,
    /// Replicas of the service running on this node.
    pub running: u32,
    /// Whether this node's replicas of the service are healthy (trivially true
    /// when the node runs none).
    pub healthy: bool,
    /// This node's containers for the service.
    pub containers: Vec<ClusterContainerSummary>,
}

/// A static-cluster peer's identity, reachability, and labels.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StaticPeer {
    pub id: u64,
    /// HTTP API address (host:port). Heartbeats land on this address
    /// at `/health`; cluster dispatch lands at `/api/v1/internal/scale`.
    pub api_addr: SocketAddr,
    /// Operating system this peer runs (`"linux"` / `"windows"` / `"darwin"`).
    /// Used by placement when filtering nodes for a service's `OsKind`.
    #[serde(default = "default_os")]
    pub os: String,
    /// Free-form labels.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub labels: HashMap<String, String>,
}

fn default_heartbeat_interval() -> Duration {
    Duration::from_secs(5)
}
fn default_failure_threshold() -> Duration {
    Duration::from_secs(15)
}
fn default_os() -> String {
    "linux".to_string()
}

fn default_worker_grpc_addr() -> SocketAddr {
    "0.0.0.0:3670"
        .parse()
        .expect("hardcoded SocketAddr literal")
}

fn is_default_worker_grpc_addr(addr: &SocketAddr) -> bool {
    *addr == default_worker_grpc_addr()
}

fn default_min_ttl() -> Duration {
    Duration::from_secs(10)
}
fn default_max_ttl() -> Duration {
    Duration::from_secs(600)
}
fn default_grace() -> Duration {
    Duration::from_secs(10)
}
fn default_max_hb() -> u32 {
    50
}
fn default_failover_ttl() -> Duration {
    Duration::from_secs(300)
}

/// `serde_with`-style serializer for `Duration` as integer seconds. Inline
/// here to avoid adding a `serde_with` dep just for two fields.
mod duration_secs {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::time::Duration;

    pub fn serialize<S>(dur: &Duration, ser: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        dur.as_secs().serialize(ser)
    }

    pub fn deserialize<'de, D>(de: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        let secs = u64::deserialize(de)?;
        Ok(Duration::from_secs(secs))
    }
}

// ============================================================================
// Wire-level scale request shared across cluster impls.
//
// `InternalScaleRequest` and `ScaleAssignment` are the wire types fanned out
// by the cluster leader to each node that gets at least one replica of a
// service. They live here (instead of in `zlayer-scheduler::cluster`) so the
// same Rust type can be shared between:
//
// - The HTTP fan-out path (`StaticCluster` / `RaftCluster` in
//   `zlayer-scheduler`).
// - The `/internal/scale` handler in `zlayer-api` (which deserializes the
//   typed struct directly).
// - A future gRPC `WorkerTierCluster` (Phase 3) that reuses the same shape.
//
// `zlayer-scheduler::cluster` re-exports both types so existing call sites
// (`zlayer_scheduler::cluster::InternalScaleRequest::new`) keep compiling.
// ============================================================================

/// Wire-format scale request fanned out by the cluster's leader to each node
/// that gets assigned at least one replica of a service.
///
/// The leader's placement engine produces a `HashMap<NodeId, Vec<(role, index)>>`
/// of assignments; each entry becomes one `InternalScaleRequest` HTTP POST.
///
/// Backward-compatible: a peer may send `{service, replicas}` without
/// `assignments`. Receiving nodes treat that as a single implicit
/// `{role: "default", indices: 0..replicas}` group.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct InternalScaleRequest {
    /// Service name.
    pub service: String,
    /// Total target replica count for this node, when caller didn't supply
    /// explicit per-role assignments (legacy / Phase 1 shape). When
    /// `assignments` is non-empty, this field is informational.
    #[serde(default)]
    pub replicas: u32,
    /// Per-role-group container index lists. Empty in Phase 1; populated by
    /// Phase 2 once `replica_groups` + cross-node identity ship.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assignments: Vec<ScaleAssignment>,
    /// The full service spec, propagated so the receiving node can register
    /// (or update) the service before scaling. This is what lets a fresh
    /// worker run a replica it has never seen, and what makes an image change
    /// on the leader reach worker containers: the receiver `upsert`s this spec,
    /// which detects digest drift and rolls the local replicas. `None` on the
    /// legacy `{service, replicas}` shape (receiver falls back to its cached
    /// spec). Boxed because `ServiceSpec` is large.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<Object>)]
    pub spec: Option<Box<crate::spec::types::ServiceSpec>>,
}

/// One role-group entry within an [`InternalScaleRequest`]. Phase 2 ships
/// this; Phase 1 uses the legacy `replicas` field only.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ScaleAssignment {
    /// Role name (e.g. `"default"`, `"primary"`, `"read"`).
    pub role: String,
    /// Replica indices within this role that the receiving node should ensure
    /// exist locally. Each becomes a container `{service}-{role}-{index}`.
    pub indices: Vec<u32>,
}

impl InternalScaleRequest {
    /// Build a legacy `{service, replicas}` request (Phase 1).
    #[must_use]
    pub fn new(service: impl Into<String>, replicas: u32) -> Self {
        Self {
            service: service.into(),
            replicas,
            assignments: Vec::new(),
            spec: None,
        }
    }

    /// Attach the full service spec so the receiver can register/update the
    /// service before scaling (image-change propagation + first-deploy on a
    /// fresh worker). Chainable onto [`Self::new`] / [`Self::with_assignments`].
    #[must_use]
    pub fn with_spec(mut self, spec: crate::spec::types::ServiceSpec) -> Self {
        self.spec = Some(Box::new(spec));
        self
    }

    /// Build a Phase-2 request with explicit per-role assignments.
    #[must_use]
    pub fn with_assignments(service: impl Into<String>, assignments: Vec<ScaleAssignment>) -> Self {
        // The `replicas` field is informational when assignments are present:
        // it's still set to the total count for legacy receivers (who would
        // ignore `assignments`) but the authoritative count is the sum of
        // `assignments[i].indices.len()`.
        let replicas: u32 = assignments
            .iter()
            .map(|a| u32::try_from(a.indices.len()).unwrap_or(u32::MAX))
            .sum();
        Self {
            service: service.into(),
            replicas,
            assignments,
            spec: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_single_node() {
        let cfg = ClusterMode::default();
        assert_eq!(cfg, ClusterMode::SingleNode);
    }

    #[test]
    fn scale_request_legacy_shape_has_no_spec() {
        // A legacy `{service, replicas}` body (no `spec`) must still
        // deserialize, with `spec` defaulting to None.
        let req: InternalScaleRequest =
            serde_json::from_str(r#"{"service":"web","replicas":3}"#).unwrap();
        assert_eq!(req.service, "web");
        assert_eq!(req.replicas, 3);
        assert!(req.spec.is_none());
        assert!(req.assignments.is_empty());
    }

    #[test]
    fn scale_request_with_spec_roundtrips() {
        let spec = crate::spec::types::ServiceSpec::default();
        let req = InternalScaleRequest::new("web", 3).with_spec(spec);
        assert!(req.spec.is_some());
        let json = serde_json::to_string(&req).unwrap();
        let back: InternalScaleRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back.service, "web");
        assert_eq!(back.replicas, 3);
        assert!(back.spec.is_some(), "spec must survive the round-trip");
    }

    #[test]
    fn yaml_static_roundtrip() {
        let yaml = r"
mode: static
node_id: 2
peers:
  - id: 1
    api_addr: 10.0.0.10:3669
  - id: 2
    api_addr: 10.0.0.11:3669
heartbeat_interval: 5
failure_threshold: 15
";
        let parsed: ClusterMode = serde_yaml::from_str(yaml).unwrap();
        match parsed {
            ClusterMode::Static {
                node_id,
                peers,
                heartbeat_interval,
                failure_threshold,
            } => {
                assert_eq!(node_id, 2);
                assert_eq!(peers.len(), 2);
                assert_eq!(heartbeat_interval, Duration::from_secs(5));
                assert_eq!(failure_threshold, Duration::from_secs(15));
            }
            _ => panic!("expected Static variant"),
        }
    }

    #[test]
    fn yaml_single_node_roundtrip() {
        let yaml = "mode: single-node";
        let parsed: ClusterMode = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(parsed, ClusterMode::SingleNode);
    }

    // ------------------------------------------------------------------------
    // InternalScaleRequest serde roundtrips.
    //
    // The wire shape is backward-compatible: legacy callers send
    // `{service, replicas}` (no `assignments`); Phase-2 callers add
    // `{service, replicas, assignments}`. Receiving nodes must parse both
    // and the assignments-less form must still produce a useful struct.
    // ------------------------------------------------------------------------

    #[test]
    fn internal_scale_request_legacy_shape() {
        // Legacy `{service, replicas}` without `assignments`.
        let json = r#"{"service":"web","replicas":3}"#;
        let req: InternalScaleRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.service, "web");
        assert_eq!(req.replicas, 3);
        assert!(req.assignments.is_empty());

        // Re-serialize: `assignments` is omitted when empty.
        let out = serde_json::to_string(&req).unwrap();
        assert!(!out.contains("assignments"), "got: {out}");
        assert!(out.contains(r#""service":"web""#));
        assert!(out.contains(r#""replicas":3"#));
    }

    #[test]
    fn internal_scale_request_with_assignments_roundtrip() {
        let req = InternalScaleRequest::with_assignments(
            "db",
            vec![
                ScaleAssignment {
                    role: "primary".to_string(),
                    indices: vec![0],
                },
                ScaleAssignment {
                    role: "read".to_string(),
                    indices: vec![1, 2],
                },
            ],
        );
        assert_eq!(req.replicas, 3); // sum of indices lengths

        let json = serde_json::to_string(&req).unwrap();
        let parsed: InternalScaleRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.service, "db");
        assert_eq!(parsed.replicas, 3);
        assert_eq!(parsed.assignments.len(), 2);
        assert_eq!(parsed.assignments[0].role, "primary");
        assert_eq!(parsed.assignments[0].indices, vec![0]);
        assert_eq!(parsed.assignments[1].role, "read");
        assert_eq!(parsed.assignments[1].indices, vec![1, 2]);
    }

    #[test]
    fn internal_scale_request_new_constructs_legacy_shape() {
        let req = InternalScaleRequest::new("api", 5);
        assert_eq!(req.service, "api");
        assert_eq!(req.replicas, 5);
        assert!(req.assignments.is_empty());
    }

    #[test]
    fn worker_tier_server_yaml_round_trips() {
        let yaml = r"
mode: worker-tier
role: server
node_id: 1
peers:
  - id: 1
    raft_addr: 10.0.0.1:9001
    api_addr: 10.0.0.1:3669
  - id: 2
    raft_addr: 10.0.0.2:9001
    api_addr: 10.0.0.2:3669
  - id: 3
    raft_addr: 10.0.0.3:9001
    api_addr: 10.0.0.3:3669
worker_grpc_addr: 0.0.0.0:3670
worker_ca_dir: /var/lib/zlayer/cluster
heartbeat_min_ttl: 15
heartbeat_max_ttl: 600
heartbeat_grace: 10
max_heartbeats_per_second: 100
failover_heartbeat_ttl: 300
";
        let parsed: ClusterMode = serde_yaml::from_str(yaml).unwrap();
        assert!(parsed.is_worker_tier_server());
        assert!(!parsed.is_worker_tier_worker());
        let ttl = parsed.adaptive_ttl_config().expect("ttl");
        assert_eq!(ttl.max_heartbeats_per_second, 100);
    }

    #[test]
    fn worker_tier_worker_yaml_round_trips() {
        let yaml = r"
mode: worker-tier
role: worker
servers:
  - http://10.0.0.1:3670
  - http://10.0.0.2:3670
token_file: /etc/zlayer/worker.token
identity_dir: /var/lib/zlayer/worker
";
        let parsed: ClusterMode = serde_yaml::from_str(yaml).unwrap();
        assert!(parsed.is_worker_tier_worker());
        assert!(!parsed.is_worker_tier_server());
        // Tunable defaults applied:
        let ttl = parsed.adaptive_ttl_config().expect("ttl");
        assert_eq!(ttl.min_ttl_secs, 10);
        assert_eq!(ttl.max_heartbeats_per_second, 50);
    }
}

// ============================================================================
// Worker tier (Phase 3) — Nomad-style worker protocol over HTTP
// ============================================================================

/// Bootstrap join request from a new worker node.
///
/// Carries a signed bootstrap token (issued by `zlayer node generate-worker-token`)
/// along with the worker's profile so the leader can decide acceptance + assign
/// an id. The leader rejects expired tokens, reused single-use tokens, or
/// tokens from a different cluster.
///
/// Phase 3 MVP: token is a simple bearer string. Future Phase 3.1 will add CSR
/// for mTLS identity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerRegisterRequest {
    /// Bootstrap token issued by the control plane.
    pub token: String,
    /// Optional desired `node_id`. The leader may override (e.g. on conflict).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub desired_node_id: Option<u64>,
    /// This worker's profile (OS, arch, labels, resource caps).
    pub profile: WorkerProfile,
}

/// Worker profile — published to the cluster directory on registration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerProfile {
    /// Worker's externally-reachable HTTP API address (host:port).
    pub api_addr: SocketAddr,
    /// Operating system (`linux` / `windows` / `darwin`).
    pub os: String,
    /// CPU architecture (`x86_64` / `aarch64`).
    pub arch: String,
    /// Free-form labels (region, tier, hardware class, etc.).
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub labels: std::collections::HashMap<String, String>,
    /// Total CPU cores available.
    #[serde(default)]
    pub cpu_total: u32,
    /// Total memory in bytes.
    #[serde(default)]
    pub memory_total_bytes: u64,
}

/// Successful registration response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerRegisterResponse {
    /// Assigned `node_id` (may differ from `desired_node_id`).
    pub node_id: u64,
    /// Cluster identifier — workers reject mismatched-cluster responses.
    pub cluster_id: String,
    /// Initial heartbeat TTL in seconds. Worker schedules its first
    /// `ReportStatus` tick within this window.
    pub heartbeat_ttl_secs: u32,
    /// Grace period (seconds) after `heartbeat_ttl_secs` before the
    /// leader marks the worker as `Unreachable`.
    pub heartbeat_grace_secs: u32,
    /// Internal token to present on subsequent worker requests
    /// (`X-ZLayer-Internal-Token` header).
    pub internal_token: String,
}

/// Long-poll request for new assignments. Server waits up to ~30s for
/// a revision newer than `last_revision`, then returns the current
/// assignment set (which may be empty).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerPollRequest {
    pub node_id: u64,
    /// The highest revision the worker has applied. Server returns
    /// any events with revision > this (or empty after timeout).
    #[serde(default)]
    pub last_revision: u64,
    /// Maximum seconds to wait before returning even with no new
    /// events. Defaults to 30; capped server-side at 60.
    #[serde(default = "default_poll_wait_secs")]
    pub max_wait_secs: u32,
}

fn default_poll_wait_secs() -> u32 {
    30
}

/// Response to a long-poll: zero or more assignment events newer than
/// the worker's `last_revision`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerPollResponse {
    /// Current cluster revision (worker should record this).
    pub revision: u64,
    /// Assignment events ordered by revision ASC. Empty when nothing
    /// new since `last_revision` (timeout).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<WorkerAssignmentEvent>,
}

/// One change to a worker's assignment set.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkerAssignmentEvent {
    /// Assign or update a service on this worker.
    Set {
        service: String,
        /// Per-role replica indices the worker should own.
        assignments: Vec<ScaleAssignment>,
        revision: u64,
    },
    /// Remove a service entirely from this worker.
    Delete { service: String, revision: u64 },
    /// Drain command — worker should stop accepting new work and
    /// shut down once existing containers exit.
    Drain { revision: u64 },
}

/// Periodic status report from worker → control plane.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerStatusReport {
    pub node_id: u64,
    /// Unix epoch nanoseconds when this snapshot was taken.
    pub ts_ns: u64,
    /// Currently-running container summaries.
    #[serde(default)]
    pub containers: Vec<WorkerContainerStatus>,
    /// Resource utilization snapshot.
    pub resources: WorkerResourceUsage,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerContainerStatus {
    pub service: String,
    pub role: String,
    pub replica: u32,
    /// `running` / `exited` / `failed` etc. — same convention as
    /// the agent's `ContainerState` string form.
    pub state: String,
    /// Container's overlay IP, if attached.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overlay_ip: Option<std::net::IpAddr>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerResourceUsage {
    pub cpu_used: f64,
    pub memory_used_bytes: u64,
    pub gpu_used: u32,
}

/// Ack for a `WorkerStatusReport`. Carries the next-TTL so the
/// worker can adapt its heartbeat cadence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerStatusAck {
    /// Seconds until the worker's next required heartbeat. The leader
    /// computes this adaptively from cluster size:
    /// `clamp(N_workers / max_hb_per_sec, min, max)`.
    pub next_ttl_secs: u32,
}

/// A worker lease in the leader's directory — drives liveness.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerLease {
    pub node_id: u64,
    pub profile: WorkerProfile,
    /// Unix epoch seconds when the lease was granted.
    pub acquired_unix_secs: i64,
    /// Unix epoch seconds of last successful heartbeat renewal.
    pub renewed_unix_secs: i64,
    /// Current TTL applied to this worker (adaptive).
    pub ttl_secs: u32,
    /// Grace period (seconds) applied on top of `ttl_secs` before the
    /// leader marks this worker as `Unreachable`. Stored on the lease
    /// so the value used at lease-grant time is durable even if the
    /// leader's `AdaptiveTtlConfig` later changes.
    pub grace_secs: u32,
}

impl WorkerLease {
    /// True if `now_unix_secs - renewed_unix_secs > ttl_secs + grace`.
    ///
    /// The `grace_secs` argument lets callers override the lease's
    /// stored grace value (e.g. to apply a temporarily-extended grace
    /// during a known leadership transition).
    #[must_use]
    pub fn is_expired(&self, now_unix_secs: i64, grace_secs: u32) -> bool {
        let elapsed = now_unix_secs.saturating_sub(self.renewed_unix_secs).max(0);
        let elapsed_secs = u64::try_from(elapsed).unwrap_or(0);
        elapsed_secs > u64::from(self.ttl_secs).saturating_add(u64::from(grace_secs))
    }
}

/// Adaptive-TTL heartbeat configuration. Mirrors Nomad's design:
/// the leader caps cluster-wide heartbeat rate to a constant by
/// stretching individual workers' TTLs as the cluster grows.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdaptiveTtlConfig {
    pub min_ttl_secs: u32,
    pub max_ttl_secs: u32,
    pub grace_secs: u32,
    pub max_heartbeats_per_second: u32,
    pub failover_ttl_secs: u32,
}

impl Default for AdaptiveTtlConfig {
    fn default() -> Self {
        Self {
            min_ttl_secs: 10,
            max_ttl_secs: 600,
            grace_secs: 10,
            max_heartbeats_per_second: 50,
            failover_ttl_secs: 300,
        }
    }
}

impl AdaptiveTtlConfig {
    /// Compute the TTL to hand a worker, given the current cluster
    /// size. Formula matches Nomad: `clamp(N / max_hb_per_sec, min, max)`.
    #[must_use]
    pub fn compute_ttl(&self, n_workers: u32) -> u32 {
        if self.max_heartbeats_per_second == 0 {
            return self.max_ttl_secs;
        }
        let raw = n_workers.saturating_add(self.max_heartbeats_per_second - 1)
            / self.max_heartbeats_per_second;
        raw.clamp(self.min_ttl_secs, self.max_ttl_secs)
    }
}

#[cfg(test)]
mod worker_tier_tests {
    use super::*;

    #[test]
    fn adaptive_ttl_scales_with_cluster() {
        let cfg = AdaptiveTtlConfig::default();
        // 10 workers, 50hb/s target = ttl ~0.2s, clamped up to min=10s.
        assert_eq!(cfg.compute_ttl(10), 10);
        // 100 workers, 50hb/s target = ttl 2s, clamped up to 10s.
        assert_eq!(cfg.compute_ttl(100), 10);
        // 500 workers = 10s exactly.
        assert_eq!(cfg.compute_ttl(500), 10);
        // 1000 workers, 50hb/s = ttl 20s.
        assert_eq!(cfg.compute_ttl(1000), 20);
        // 10000 workers, 50hb/s = 200s (well within 600 cap).
        assert_eq!(cfg.compute_ttl(10000), 200);
        // 100000 workers = 2000s, clamped to 600s max.
        assert_eq!(cfg.compute_ttl(100_000), 600);
    }

    #[test]
    fn worker_lease_expiration() {
        let lease = WorkerLease {
            node_id: 1,
            profile: WorkerProfile {
                api_addr: "127.0.0.1:3669".parse().unwrap(),
                os: "linux".to_string(),
                arch: "x86_64".to_string(),
                labels: HashMap::default(),
                cpu_total: 4,
                memory_total_bytes: 8_000_000_000,
            },
            acquired_unix_secs: 1000,
            renewed_unix_secs: 1000,
            ttl_secs: 30,
            grace_secs: 10,
        };
        // 25s elapsed: not expired
        assert!(!lease.is_expired(1025, 10));
        // 40s elapsed: still within ttl+grace = 40
        assert!(!lease.is_expired(1040, 10));
        // 41s elapsed: expired
        assert!(lease.is_expired(1041, 10));
    }
}
