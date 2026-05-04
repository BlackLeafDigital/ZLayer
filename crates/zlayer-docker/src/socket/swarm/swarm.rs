//! `/swarm` endpoints — bridges between Docker's Swarm wire shape and
//! `ZLayer`'s native cluster primitives.
//!
//! `ZLayer` already runs as a cluster-of-one when the daemon is up, so the
//! handlers here treat the cluster as **always active** rather than
//! emitting Docker's "swarm-inactive" 503 envelope. The reads compose
//! [`zlayer_api::ClusterNodeSummary`] (via
//! [`zlayer_client::DaemonClient::cluster_nodes`]) and
//! [`zlayer_types::api::overlay::OverlayStatusResponse`] (via
//! [`zlayer_client::DaemonClient::overlay_status`]) into the
//! [`super::shape::Swarm`] envelope; the writes either bridge to the
//! daemon's `/api/v1/cluster/*` endpoints or no-op for spec changes that
//! have no `ZLayer` analogue. Cluster-leave is intentionally 501 because
//! the daemon does not expose a leave endpoint yet -- the user is pointed
//! at `zlayer cluster leave` in the error envelope.
//!
//! ## Status code map
//!
//! * `GET /swarm`             -> 200 (always); 500 if both daemon calls fail
//! * `POST /swarm/init`       -> 200 with the local node id (single-node bootstrap)
//! * `POST /swarm/join`       -> 200 / 4xx (forwarded from the daemon)
//! * `POST /swarm/leave`      -> 501 (no native leave endpoint yet)
//! * `POST /swarm/update`     -> 200 (no-op; spec has no `ZLayer` equivalent)
//! * `GET /swarm/unlockkey`   -> 200 `{"UnlockKey": ""}` (autolock not supported)
//! * `POST /swarm/unlock`     -> 200 (no-op; nothing is locked)

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use sha2::{Digest, Sha256};

use crate::socket::system::error_response;
use crate::socket::SocketState;

use super::shape::{iso_timestamp, JoinTokens, Swarm, SwarmSpec, Version};

/// Determine whether a [`zlayer_api::ClusterNodeSummary`] represents a
/// "manager" (Docker terminology) -- i.e. a Raft voter or leader. The
/// matcher mirrors [`super::shape::Node::from`] so the role mapping stays
/// consistent across the `/swarm`, `/nodes`, and `/info` surfaces.
fn is_manager_role(n: &zlayer_api::ClusterNodeSummary) -> bool {
    n.is_leader || n.role == "leader" || n.role == "voter"
}

/// Build the `/info` `Swarm` JSON object from the daemon's
/// `cluster_nodes` + `overlay_status` responses.
///
/// `/info` differs from `/swarm` in two ways:
///
/// * It must always succeed -- the whole `/info` body is meant to be a
///   single, synchronous snapshot, so the helper never returns an error.
///   When a daemon call fails the Swarm block collapses to the
///   "swarm-inactive" shape (`LocalNodeState: "inactive"`, zero counts)
///   plus a non-empty `Error` field summarising the failure, and `/info`
///   keeps going.
/// * The wire shape carries a `RemoteManagers` array, a `Nodes` /
///   `Managers` count pair, and a nested `Cluster` envelope -- none of
///   which appear in `GET /swarm`. The cluster id is derived by the same
///   [`derive_cluster_id`] helper used by `/swarm`, so the two surfaces
///   never disagree on the swarm id.
///
/// The "local node" is identified by matching the [`OverlayStatusResponse::node_ip`]
/// against each [`ClusterNodeSummary::overlay_ip`]. If the match fails
/// (e.g. the overlay reports an empty / unset IP), the leader -- or, as
/// a last resort, the first listed node -- stands in. This mirrors the
/// "best available local id" approach `/swarm/init` uses for its
/// single-node bootstrap reply.
pub(in crate::socket) async fn build_info_swarm_json(state: &SocketState) -> serde_json::Value {
    let nodes_result = state.client.cluster_nodes().await;
    let overlay_result = state.client.overlay_status().await;

    // Both calls failing collapses to the inactive shape with a
    // human-readable Error summary. `/info` must still succeed, so we
    // never propagate the daemon error up.
    let (nodes, overlay_node_ip, overlay_err) = match (nodes_result, overlay_result) {
        (Err(nodes_err), Err(overlay_err)) => {
            return inactive_swarm_json(format!(
                "ZLayer daemon is unreachable: cluster_nodes failed ({nodes_err:#}); \
                 overlay_status failed ({overlay_err:#})"
            ));
        }
        (Ok(nodes), Ok(overlay)) => (nodes, Some(overlay.node_ip), None),
        (Ok(nodes), Err(err)) => (nodes, None, Some(format!("overlay_status failed: {err:#}"))),
        (Err(err), Ok(overlay)) => {
            // No node list at all -- without it we cannot populate
            // node counts / RemoteManagers / timestamps. Fall back to
            // inactive but still record the (partial) overlay-side error
            // text so operators can tell which call failed.
            return inactive_swarm_json(format!(
                "cluster_nodes failed: {err:#}; overlay node_ip={}",
                overlay.node_ip
            ));
        }
    };

    // Pick the local node. Prefer matching the overlay-reported IP; if
    // that fails (or overlay_status itself failed) fall back to leader,
    // then to the first node. The last fallback only kicks in when the
    // daemon has zero registered nodes -- a single-node ZLayer always
    // registers itself, so this should not happen in practice.
    let local_node = overlay_node_ip
        .as_deref()
        .filter(|ip| !ip.is_empty())
        .and_then(|ip| nodes.iter().find(|n| n.overlay_ip == ip))
        .or_else(|| nodes.iter().find(|n| n.is_leader))
        .or_else(|| nodes.first());

    let local_id = local_node.map(|n| n.id.clone()).unwrap_or_default();
    let local_addr = local_node.map(|n| n.address.clone()).unwrap_or_default();
    let control_available = local_node.is_some_and(is_manager_role);

    // Remote managers = every voter/leader EXCEPT the local one. Worker
    // nodes do not appear here; Docker's wire shape reserves
    // `RemoteManagers` for the control-plane peers a manager would dial.
    let remote_managers: Vec<serde_json::Value> = nodes
        .iter()
        .filter(|n| is_manager_role(n) && n.id != local_id)
        .map(|n| {
            serde_json::json!({
                "NodeID": n.id,
                "Addr":   n.address,
            })
        })
        .collect();

    let total_nodes = i64::try_from(nodes.len()).unwrap_or(i64::MAX);
    let manager_count =
        i64::try_from(nodes.iter().filter(|n| is_manager_role(n)).count()).unwrap_or(i64::MAX);

    // CreatedAt = earliest registration; UpdatedAt = "now" (Docker
    // refreshes it on every spec update, and `/info` is the most
    // up-to-date snapshot we have). Both are RFC 3339 per the
    // surrounding shape.
    let earliest_registered_ms = nodes.iter().map(|n| n.registered_at).min().unwrap_or(0);
    let created_at = iso_timestamp(ms_to_secs(earliest_registered_ms));
    let updated_at = iso_timestamp(now_secs());

    let cluster_id = derive_cluster_id(&local_id);

    // `Error` is only set when overlay_status failed but the node list
    // succeeded -- enough to populate the rest of the block but worth
    // surfacing so operators can spot a half-degraded overlay.
    let error_field = overlay_err.unwrap_or_default();

    serde_json::json!({
        "NodeID": local_id,
        "NodeAddr": local_addr,
        "LocalNodeState": "active",
        "ControlAvailable": control_available,
        "Error": error_field,
        "RemoteManagers": remote_managers,
        "Nodes": total_nodes,
        "Managers": manager_count,
        "Cluster": {
            "ID": cluster_id,
            "Version": { "Index": 0_u64 },
            "CreatedAt": created_at,
            "UpdatedAt": updated_at,
            "Spec": {
                "Name": "zlayer",
                "Labels": serde_json::Map::<String, serde_json::Value>::new(),
            },
        },
    })
}

/// Build the "swarm-inactive" `/info` Swarm JSON.
///
/// Used as the fall-through when both daemon calls fail (or when the
/// node-list call fails -- without it the rest of the block has nothing
/// to report). Mirrors stock dockerd's wire shape for a daemon that has
/// not joined a swarm: empty NodeID/NodeAddr, `LocalNodeState: "inactive"`,
/// zero counts, and a non-empty `Error` describing what went wrong.
fn inactive_swarm_json(error: impl Into<String>) -> serde_json::Value {
    serde_json::json!({
        "NodeID": "",
        "NodeAddr": "",
        "LocalNodeState": "inactive",
        "ControlAvailable": false,
        "Error": error.into(),
        "RemoteManagers": serde_json::Value::Array(Vec::new()),
        "Nodes": 0_i64,
        "Managers": 0_i64,
        "Cluster": {
            "ID": "",
            "Version": { "Index": 0_u64 },
            "CreatedAt": iso_timestamp(0),
            "UpdatedAt": iso_timestamp(0),
            "Spec": {
                "Name": "zlayer",
                "Labels": serde_json::Map::<String, serde_json::Value>::new(),
            },
        },
    })
}

/// Current wall-clock time in seconds since the Unix epoch.
///
/// Saturates to 0 if the system clock is set before 1970 (e.g. on a
/// freshly-imaged appliance with no RTC battery) so [`iso_timestamp`]
/// still emits a valid RFC 3339 string instead of "1970-01-01T00:00:00Z"
/// indirectly via panic.
fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// Message returned when `POST /swarm/leave` is hit. `ZLayer` doesn't
/// expose a native leave endpoint yet -- the operator must use the CLI --
/// so we return 501 (Not Implemented) with the wording that points at it.
const SWARM_LEAVE_MESSAGE: &str =
    "Use `zlayer cluster leave` to leave the cluster. The Docker-compat \
     `/swarm/leave` endpoint is not implemented because the ZLayer daemon \
     does not yet expose a programmatic leave API.";

/// Message returned when a join is attempted on a node that is already a
/// cluster member. Mirrors stock dockerd's "this node is already part of a
/// swarm" wording so Docker CLI surfaces a familiar error.
const ALREADY_IN_SWARM_MESSAGE: &str =
    "This node is already part of a swarm. Use `zlayer cluster leave` first \
     to remove it before re-joining.";

/// `/swarm` route table.
pub(super) fn routes(state: SocketState) -> Router {
    Router::new()
        .route("/swarm", get(inspect))
        .route("/swarm/init", post(init))
        .route("/swarm/join", post(join))
        .route("/swarm/leave", post(leave))
        .route("/swarm/update", post(update))
        .route("/swarm/unlockkey", get(unlock_key))
        .route("/swarm/unlock", post(unlock))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// GET /swarm
// ---------------------------------------------------------------------------

/// `GET /swarm` -- Inspect the swarm cluster.
///
/// Composes a [`Swarm`] envelope from `cluster_nodes` + `overlay_status`.
/// The cluster id is derived from the leader node's id (or the first node
/// if no leader is reported) via SHA-256 truncation; Docker tooling only
/// requires an opaque stable string here, so the hash gives us one without
/// pulling a new identifier through the API.
///
/// Both daemon calls failing is the only path that returns 500 -- one
/// failure is tolerated because either source on its own carries enough
/// information for a usable envelope (timestamps come from the node list,
/// the cluster id from either source).
async fn inspect(State(state): State<SocketState>) -> Response {
    let nodes_result = state.client.cluster_nodes().await;
    let overlay_result = state.client.overlay_status().await;

    if nodes_result.is_err() && overlay_result.is_err() {
        return error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "ZLayer daemon is unreachable: both /api/v1/cluster/nodes and \
             /api/v1/overlay/status failed",
        );
    }

    // Pick the most-authoritative node id available. The leader's id is
    // preferred; failing that, the first node in the list; failing that,
    // the empty string -- which `derive_cluster_id` collapses into a
    // hard-coded fallback so the wire shape never carries a blank ID.
    let leader_id = nodes_result
        .as_ref()
        .ok()
        .and_then(|nodes| {
            nodes
                .iter()
                .find(|n| n.is_leader)
                .or_else(|| nodes.first())
                .map(|n| n.id.clone())
        })
        .unwrap_or_default();

    // Most recent heartbeat across all known nodes -> "updated_at"; the
    // earliest registration -> "created_at". Falls back to epoch if the
    // node list call failed entirely.
    let (created_at_ms, updated_at_ms) = nodes_result
        .as_ref()
        .ok()
        .and_then(|nodes| {
            let created = nodes.iter().map(|n| n.registered_at).min()?;
            let updated = nodes.iter().map(|n| n.last_heartbeat).max()?;
            Some((created, updated))
        })
        .unwrap_or((0, 0));

    let id = derive_cluster_id(&leader_id);

    let envelope = Swarm {
        id,
        created_at: iso_timestamp(ms_to_secs(created_at_ms)),
        updated_at: iso_timestamp(ms_to_secs(updated_at_ms)),
        spec: SwarmSpec {
            name: "zlayer".to_string(),
            labels: serde_json::Value::Object(serde_json::Map::new()),
            ..SwarmSpec::default()
        },
        version: Version { index: 0 },
        root_rotation_in_progress: false,
        join_tokens: JoinTokens::default(),
    };

    (StatusCode::OK, Json(envelope)).into_response()
}

/// Convert a millisecond timestamp (as carried by [`zlayer_api::ClusterNodeSummary`])
/// into seconds for [`super::shape::iso_timestamp`].
fn ms_to_secs(ms: u64) -> i64 {
    i64::try_from(ms / 1_000).unwrap_or(0)
}

/// Derive a stable Docker `Swarm.ID` from the leader node id.
///
/// Docker tooling only requires an opaque string here, so SHA-256 of the
/// node id (truncated to 25 hex chars to mirror Docker's own swarm id
/// width) is sufficient. The fallback path (`leader_id` empty, e.g. when
/// the `cluster_nodes` call failed but `overlay_status` succeeded)
/// collapses to a deterministic string so the field is never blank.
fn derive_cluster_id(leader_id: &str) -> String {
    if leader_id.is_empty() {
        return "zlayer-cluster-unknown".to_string();
    }
    let mut hasher = Sha256::new();
    hasher.update(b"zlayer-cluster:");
    hasher.update(leader_id.as_bytes());
    let digest = hasher.finalize();
    let hex = format!("{digest:x}");
    // Docker's native swarm id is 25 hex chars. Truncating SHA-256 keeps
    // the wire shape compatible without losing entropy meaningful to
    // Docker tooling.
    hex.chars().take(25).collect()
}

// ---------------------------------------------------------------------------
// POST /swarm/init
// ---------------------------------------------------------------------------

/// `POST /swarm/init` -- Initialise a new swarm cluster.
///
/// `ZLayer` is already a cluster-of-one when the daemon is running, so
/// "init" reduces to "tell the caller our node id." Docker's wire format
/// for this endpoint is a JSON-encoded string body (the node id of the
/// new manager); we honour that rather than wrapping it in an object.
async fn init(State(state): State<SocketState>) -> Response {
    match state.client.cluster_nodes().await {
        Ok(nodes) => {
            let node_id = nodes
                .iter()
                .find(|n| n.is_leader)
                .or_else(|| nodes.first())
                .map_or_else(String::new, |n| n.id.clone());
            // Docker returns the node id as a JSON-encoded string, e.g.
            // `"abcdef123..."`. `Json(String)` produces exactly that wire
            // shape; the StatusCode is implicit 200.
            (StatusCode::OK, Json(node_id)).into_response()
        }
        Err(err) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("failed to inspect ZLayer cluster: {err:#}"),
        ),
    }
}

// ---------------------------------------------------------------------------
// POST /swarm/join
// ---------------------------------------------------------------------------

/// Docker's `/swarm/join` request body. Only the fields we look at are
/// listed; everything else is ignored because `ZLayer`'s join endpoint
/// requires extra metadata (`WireGuard` public key, ports, capacity) that
/// the Docker payload does not carry.
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DockerJoinRequest {
    /// Bound listen address for the local manager. Unused -- `ZLayer`
    /// chooses its own bind address based on daemon configuration.
    #[serde(default)]
    listen_addr: String,
    /// Public address for inbound peer traffic. Forwarded as
    /// [`zlayer_api::ClusterJoinRequest::advertise_addr`] when set.
    #[serde(default)]
    advertise_addr: String,
    /// Data-plane address override. Unused -- `ZLayer` uses
    /// `advertise_addr` for both control and data planes.
    #[serde(default)]
    #[allow(dead_code)]
    data_path_addr: String,
    /// Existing manager addresses to dial during join. We don't establish
    /// outbound connections here -- the `ZLayer` daemon handles peer
    /// dial-out internally -- but the first entry is a useful
    /// advertise-addr fallback when the client only supplied
    /// `RemoteAddrs`.
    #[serde(default)]
    remote_addrs: Vec<String>,
    /// Opaque join token issued by the cluster leader. Forwarded verbatim
    /// as the daemon's `token` field.
    #[serde(default)]
    join_token: String,
}

/// `POST /swarm/join` -- Join an existing swarm cluster.
///
/// Bridges to `POST /api/v1/cluster/join` on the local daemon. The Docker
/// request shape carries strictly less information than `ZLayer` requires
/// (no `WireGuard` public key, no port information, no capacity report),
/// so the bridge is best-effort: the daemon validates the resulting
/// payload and surfaces its own 4xx if mandatory fields are missing.
///
/// If this node is already a cluster member, returns 503 with stock
/// dockerd's "already part of swarm" wording so Docker CLI prints a
/// recognisable error.
async fn join(State(state): State<SocketState>, Json(req): Json<DockerJoinRequest>) -> Response {
    // Bail early if we already belong to a cluster. ZLayer doesn't
    // currently expose a "membership state" boolean, so we approximate it
    // by treating "cluster_nodes returns >=1 entry that includes us" as
    // "already in a swarm." The cheap check is "any nodes at all?" --
    // every running daemon registers itself, so a non-empty list means
    // we're already a member of *some* cluster (single-node or otherwise).
    if let Ok(nodes) = state.client.cluster_nodes().await {
        if !nodes.is_empty() {
            return error_response(StatusCode::SERVICE_UNAVAILABLE, ALREADY_IN_SWARM_MESSAGE);
        }
    }

    // Pull a sane advertise address: prefer the explicit field, fall back
    // to the first remote-addr the client offered.
    let advertise_addr = if req.advertise_addr.is_empty() {
        req.remote_addrs.first().cloned().unwrap_or_default()
    } else {
        req.advertise_addr.clone()
    };
    // `listen_addr` is informational here; we let the daemon ignore the
    // unfamiliar field rather than dropping it.
    let _ = req.listen_addr;

    // Translate to the daemon's join wire shape. Capacity / GPU / WG
    // fields stay at zero / empty -- the daemon's validator will reject
    // the request with a 4xx if those are required, and the caller will
    // see that error envelope unchanged.
    let body = serde_json::json!({
        "token": req.join_token,
        "advertise_addr": advertise_addr,
        "overlay_port": 0,
        "raft_port": 0,
        "wg_public_key": "",
    });

    match state.client.cluster_join(&body).await {
        Ok(_) => StatusCode::OK.into_response(),
        Err(err) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("cluster join failed: {err:#}"),
        ),
    }
}

// ---------------------------------------------------------------------------
// POST /swarm/leave
// ---------------------------------------------------------------------------

/// `POST /swarm/leave` -- Leave the current swarm cluster.
///
/// Returns 501 because the `ZLayer` daemon does not expose a leave
/// endpoint yet. The error envelope points at `zlayer cluster leave` so
/// operators have a clear action.
async fn leave() -> Response {
    error_response(StatusCode::NOT_IMPLEMENTED, SWARM_LEAVE_MESSAGE)
}

// ---------------------------------------------------------------------------
// POST /swarm/update
// ---------------------------------------------------------------------------

/// `POST /swarm/update` -- Update swarm spec.
///
/// Accept-and-ignore. Docker silently accepts most spec changes that have
/// no operational impact, and `ZLayer` has no analogue for the Swarm-spec
/// surface (orchestration / dispatcher / CA config), so a 200 with no
/// body is the closest faithful behaviour.
async fn update() -> Response {
    StatusCode::OK.into_response()
}

// ---------------------------------------------------------------------------
// GET /swarm/unlockkey
// ---------------------------------------------------------------------------

/// `GET /swarm/unlockkey` -- Return the unlock key for an autolocked swarm.
///
/// `ZLayer` never autolocks, so we return Docker's "no key" wire shape
/// (`{"UnlockKey": ""}`) with 200.
async fn unlock_key() -> Response {
    (StatusCode::OK, Json(serde_json::json!({ "UnlockKey": "" }))).into_response()
}

// ---------------------------------------------------------------------------
// POST /swarm/unlock
// ---------------------------------------------------------------------------

/// `POST /swarm/unlock` -- Unlock an autolocked swarm.
///
/// No-op. Nothing is ever locked.
async fn unlock() -> Response {
    StatusCode::OK.into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `derive_cluster_id` is deterministic per-input and stable across
    /// calls -- Docker tooling caches the swarm id, so flapping it would
    /// break clients.
    #[test]
    fn cluster_id_is_deterministic() {
        let a = derive_cluster_id("node-1");
        let b = derive_cluster_id("node-1");
        assert_eq!(a, b);
        assert_eq!(a.len(), 25, "Docker's swarm id is 25 hex chars");
    }

    /// An empty leader id yields the named fallback rather than a hashed
    /// empty string; this keeps the wire shape readable when the daemon's
    /// `cluster_nodes` call has failed but `overlay_status` has not.
    #[test]
    fn cluster_id_falls_back_when_leader_unknown() {
        assert_eq!(derive_cluster_id(""), "zlayer-cluster-unknown");
    }

    /// Different inputs produce different ids (sanity check on the hash --
    /// not a cryptographic assertion).
    #[test]
    fn cluster_id_distinguishes_inputs() {
        assert_ne!(derive_cluster_id("a"), derive_cluster_id("b"));
    }

    /// Millisecond -> seconds conversion divides by 1000 and saturates
    /// to 0 only when the resulting value would overflow `i64`.
    #[test]
    fn ms_to_secs_basic_conversion() {
        assert_eq!(ms_to_secs(0), 0);
        assert_eq!(ms_to_secs(1_500), 1);
        // `u64::MAX / 1000` is still well within `i64`'s positive range
        // (~1.8e16 << ~9.2e18), so the conversion succeeds losslessly. The
        // saturation path only activates if a future caller hands us a
        // value already in seconds that exceeds `i64::MAX`.
        assert_eq!(ms_to_secs(u64::MAX), 18_446_744_073_709_551_i64);
    }

    /// `/swarm/leave` returns 501 with a Docker-shape error envelope that
    /// points at the CLI replacement.
    #[tokio::test]
    async fn leave_returns_501() {
        let resp = leave().await;
        assert_eq!(resp.status(), StatusCode::NOT_IMPLEMENTED);
        let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let msg = v["message"].as_str().expect("message field");
        assert!(
            msg.contains("zlayer cluster leave"),
            "leave message must point at the CLI replacement, got: {msg}"
        );
    }

    /// `/swarm/update` is a no-op 200.
    #[tokio::test]
    async fn update_returns_200() {
        let resp = update().await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    /// `/swarm/unlockkey` returns the documented `{"UnlockKey": ""}` shape.
    #[tokio::test]
    async fn unlock_key_returns_empty_key() {
        let resp = unlock_key().await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["UnlockKey"].as_str(), Some(""));
    }

    /// `/swarm/unlock` is a no-op 200.
    #[tokio::test]
    async fn unlock_returns_200() {
        let resp = unlock().await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    /// Build a fully-populated [`zlayer_api::ClusterNodeSummary`] for use
    /// inside the manager-role / `inactive_swarm_json` unit tests. Pulled
    /// out as a helper because every numeric / string field is mandatory
    /// even when irrelevant to the assertion under test.
    fn fake_node(
        id: &str,
        role: &str,
        is_leader: bool,
        overlay_ip: &str,
    ) -> zlayer_api::ClusterNodeSummary {
        zlayer_api::ClusterNodeSummary {
            id: id.to_string(),
            address: format!("{id}:7000"),
            advertise_addr: format!("{id}:7001"),
            status: "ready".to_string(),
            role: role.to_string(),
            mode: "full".to_string(),
            is_leader,
            overlay_ip: overlay_ip.to_string(),
            cpu_total: 4.0,
            cpu_used: 0.5,
            memory_total: 8 * 1024 * 1024 * 1024,
            memory_used: 1024 * 1024 * 1024,
            registered_at: 1_700_000_000_000,
            last_heartbeat: 1_700_000_100_000,
        }
    }

    /// Manager role recognition matches `Node::from`'s mapping: leader
    /// flag wins, then the literal `"leader"` / `"voter"` strings; every
    /// other value (including the daemon's `"learner"`) is a worker.
    #[test]
    fn is_manager_role_matches_node_from() {
        assert!(is_manager_role(&fake_node("a", "voter", true, "10.0.0.1")));
        assert!(is_manager_role(&fake_node("b", "voter", false, "10.0.0.2")));
        assert!(is_manager_role(&fake_node(
            "c", "leader", false, "10.0.0.3"
        )));
        assert!(!is_manager_role(&fake_node(
            "d", "learner", false, "10.0.0.4"
        )));
        assert!(!is_manager_role(&fake_node(
            "e", "worker", false, "10.0.0.5"
        )));
    }

    /// `inactive_swarm_json` mirrors stock dockerd's swarm-inactive shape:
    /// empty NodeID/NodeAddr, `LocalNodeState: "inactive"`, zero counts,
    /// non-empty `Error`, and an empty (not null) `RemoteManagers` array.
    /// Docker CLI rejects a null array here with "cannot iterate over
    /// null", so the empty-array fallback is load-bearing.
    #[test]
    fn inactive_swarm_json_has_docker_shape() {
        let v = inactive_swarm_json("daemon unreachable");
        assert_eq!(v["NodeID"].as_str(), Some(""));
        assert_eq!(v["NodeAddr"].as_str(), Some(""));
        assert_eq!(v["LocalNodeState"].as_str(), Some("inactive"));
        assert_eq!(v["ControlAvailable"].as_bool(), Some(false));
        assert_eq!(v["Error"].as_str(), Some("daemon unreachable"));
        assert!(v["RemoteManagers"].is_array());
        assert_eq!(v["RemoteManagers"].as_array().unwrap().len(), 0);
        assert_eq!(v["Nodes"].as_i64(), Some(0));
        assert_eq!(v["Managers"].as_i64(), Some(0));
        assert!(v["Cluster"].is_object());
        assert_eq!(v["Cluster"]["Spec"]["Name"].as_str(), Some("zlayer"));
    }
}
