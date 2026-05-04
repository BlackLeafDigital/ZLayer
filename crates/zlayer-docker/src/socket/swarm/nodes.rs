//! `/nodes` endpoints — Swarm node bridge.
//!
//! These handlers project `ZLayer`'s native cluster state onto Docker's
//! Swarm `Node` wire shape so Docker-aware tooling (Docker CLI, Portainer,
//! `docker node ls`) can observe a `ZLayer` cluster:
//!
//! * `GET /nodes`             — list cluster nodes, with `?filters=` support.
//! * `GET /nodes/{id}`        — inspect one node.
//! * `POST /nodes/{id}/update` — set the node's labels (Role / Availability
//!   changes are accepted but no-op'd; `ZLayer`'s scheduler does not honour
//!   Swarm's `drain` semantics).
//! * `DELETE /nodes/{id}`     — answers 501 with a pointer at `zlayer
//!   cluster`; node membership is managed through the native CLI, not the
//!   Docker shim.
//!
//! Conversion lives partly in [`super::shape`] (the [`Node`] struct's
//! `From<&ClusterNodeSummary>` impl, used by the list endpoint) and partly
//! here (the inspect endpoint, which receives the richer
//! [`zlayer_types::api::nodes::NodeDetails`] DTO).

use std::collections::HashMap;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::Value;
use zlayer_api::ClusterNodeSummary;
use zlayer_types::api::nodes::NodeDetails;

use super::shape::{
    iso_timestamp, Node, NodeDescription, NodeManagerStatus, NodeSpec, NodeStatus, Version,
};
use crate::socket::system::error_response;
use crate::socket::SocketState;

/// Message returned by `DELETE /nodes/{id}`. Stock Docker uses this verb to
/// remove a node from the swarm; on `ZLayer`, membership is managed by
/// Raft and the `zlayer cluster` CLI, so the operation is intentionally
/// not exposed through the Docker shim.
const NODES_DELETE_MESSAGE: &str = "Use `zlayer cluster` to manage nodes";

// ---------------------------------------------------------------------------
// Routes
// ---------------------------------------------------------------------------

/// `/nodes` route table.
pub(super) fn routes(state: SocketState) -> Router {
    Router::new()
        .route("/nodes", get(list_nodes))
        .route(
            "/nodes/{id}",
            get(inspect_node).delete(delete_node_not_implemented),
        )
        .route("/nodes/{id}/update", post(update_node))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Query / body shapes
// ---------------------------------------------------------------------------

/// Query parameters for `GET /nodes`. Docker passes filters as a single
/// URL-encoded JSON object whose values are arrays of strings, e.g.
/// `?filters={"role":["manager"]}`.
#[derive(Debug, Deserialize, Default)]
struct ListQuery {
    #[serde(default)]
    filters: Option<String>,
}

/// Body of `POST /nodes/{id}/update`. We deserialise only the fields we
/// honour — `Labels` is mapped onto the daemon's `node_set_labels` call.
/// `Role` / `Availability` are accepted but not enforced (they no-op).
#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
struct UpdateBody {
    #[serde(default)]
    labels: Option<HashMap<String, String>>,
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    availability: Option<String>,
}

// ---------------------------------------------------------------------------
// Filter parsing
// ---------------------------------------------------------------------------

/// Parse Docker's URL-encoded JSON filter blob into a multi-valued map.
///
/// Returns `Ok(map)` (possibly empty) on success, or `Err(message)` if the
/// blob is non-empty and not valid JSON / not an object-of-string-arrays.
/// An absent or empty blob yields an empty map (matching Docker's "no
/// filters" semantics).
fn parse_node_filters(raw: Option<&str>) -> Result<HashMap<String, Vec<String>>, String> {
    let Some(s) = raw else {
        return Ok(HashMap::new());
    };
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Ok(HashMap::new());
    }
    let value: Value =
        serde_json::from_str(trimmed).map_err(|e| format!("invalid filter '{trimmed}': {e}"))?;
    let obj = value
        .as_object()
        .ok_or_else(|| format!("invalid filter '{trimmed}': expected JSON object"))?;

    let mut out: HashMap<String, Vec<String>> = HashMap::new();
    for (k, v) in obj {
        let mut values: Vec<String> = Vec::new();
        if let Some(arr) = v.as_array() {
            for item in arr {
                let Some(s) = item.as_str() else {
                    return Err(format!("invalid filter '{k}': values must be strings"));
                };
                values.push(s.to_owned());
            }
        } else if let Some(map) = v.as_object() {
            // Tolerate the `{key: {value: true}}` form Docker also accepts.
            for (key, flag) in map {
                if flag.as_bool().unwrap_or(false) {
                    values.push(key.clone());
                }
            }
        } else {
            return Err(format!(
                "invalid filter '{k}': value must be array or object"
            ));
        }
        out.insert(k.clone(), values);
    }
    Ok(out)
}

/// Apply the supported subset of Docker node filters to a list of nodes.
///
/// Honoured filter keys:
///
/// - `id`          — exact match against `Node.ID`.
/// - `name`        — exact match against `Node.Spec.Name`.
/// - `role`        — `manager` or `worker`, exact match against `Node.Spec.Role`.
/// - `membership`  — only `accepted` is meaningful; everything in `ZLayer`'s
///   cluster is "accepted," so the filter is honoured by always passing
///   when the value is `accepted` and rejecting all nodes otherwise.
/// - `label`       — substring match `key` or `key=value` against the
///   node's `Spec.Labels`.
///
/// Unknown filter keys are ignored (matching Docker's permissive semantics).
fn apply_node_filters(nodes: Vec<Node>, filters: &HashMap<String, Vec<String>>) -> Vec<Node> {
    if filters.is_empty() {
        return nodes;
    }
    nodes
        .into_iter()
        .filter(|n| node_matches_filters(n, filters))
        .collect()
}

fn node_matches_filters(node: &Node, filters: &HashMap<String, Vec<String>>) -> bool {
    for (key, values) in filters {
        if values.is_empty() {
            continue;
        }
        let pass = match key.as_str() {
            "id" => values.iter().any(|v| v == &node.id),
            "name" => values.iter().any(|v| v == &node.spec.name),
            "role" => values.iter().any(|v| v == &node.spec.role),
            // `ZLayer` only tracks accepted members; anything else is a
            // miss, anything matching `accepted` passes through.
            "membership" => values.iter().any(|v| v == "accepted"),
            "label" => values.iter().any(|v| label_matches(&node.spec.labels, v)),
            // Unknown filter keys are ignored (Docker semantics).
            _ => true,
        };
        if !pass {
            return false;
        }
    }
    true
}

/// Match a `key` or `key=value` label spec against the node's `Spec.Labels`
/// JSON object. Substring match against the value, exact-key match against
/// the key.
fn label_matches(labels: &Value, spec: &str) -> bool {
    let Some(obj) = labels.as_object() else {
        return false;
    };
    if let Some((k, v)) = spec.split_once('=') {
        obj.get(k)
            .and_then(Value::as_str)
            .is_some_and(|got| got.contains(v))
    } else {
        obj.contains_key(spec)
    }
}

// ---------------------------------------------------------------------------
// Shape helpers
// ---------------------------------------------------------------------------

/// Convert a CPU-cores count (`f64`) into Docker's `NanoCPUs` (`u64`),
/// saturating on overflow / negative values.
///
/// `ClusterNodeSummary.cpu_total` is `f64` (the daemon reports fractional
/// cores when CPU sets are partial), but Docker's wire format is integer
/// nanoseconds-of-CPU-per-second. The straight `as u64` cast is UB for
/// negative / NaN inputs and silently truncates, so we route through a
/// guarded conversion.
fn nano_cpus_from_cores(cores: f64) -> u64 {
    let nanos = cores * 1_000_000_000.0_f64;
    if !nanos.is_finite() || nanos <= 0.0 {
        return 0;
    }
    // `1.84e19` is `u64::MAX` to f64 precision; anything at-or-above that
    // saturates instead of UB-truncating. We compare against the literal
    // (rather than `u64::MAX as f64`) to keep `cast_precision_loss` quiet.
    if nanos >= 1.844_674_407_370_955_2e19_f64 {
        return u64::MAX;
    }
    // Safe: bounded above by the conditional, finite, non-negative.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    {
        nanos as u64
    }
}

/// Build a Docker [`Node`] from the richer
/// [`zlayer_types::api::nodes::NodeDetails`] DTO returned by `node_inspect`.
///
/// The list endpoint uses [`Node::from(&ClusterNodeSummary)`] (defined in
/// `shape.rs`), but `node_inspect` returns a different DTO with hostnames
/// and per-node resources, so the conversion lives here next to the
/// endpoint that consumes it.
fn node_from_details(details: &NodeDetails) -> Node {
    let role = if details.role == "leader" || details.role == "voter" {
        "manager".to_string()
    } else {
        "worker".to_string()
    };
    let state = match details.status.as_str() {
        "ready" => "ready".to_string(),
        "draining" => "down".to_string(),
        other => other.to_string(),
    };
    let labels_json = serde_json::to_value(&details.labels)
        .unwrap_or_else(|_| Value::Object(serde_json::Map::new()));
    let nano_cpus = nano_cpus_from_cores(details.resources.cpu_total);
    let platform = serde_json::json!({
        "Architecture": "amd64",
        "OS": "linux",
    });
    let resources = serde_json::json!({
        "NanoCPUs": nano_cpus,
        "MemoryBytes": details.resources.memory_total,
    });
    let engine = serde_json::json!({
        "EngineVersion": env!("CARGO_PKG_VERSION"),
        "Plugins": Vec::<Value>::new(),
    });
    let manager_status = if role == "manager" {
        Some(NodeManagerStatus {
            leader: details.role == "leader",
            reachability: "reachable".to_string(),
            addr: details.address.clone(),
        })
    } else {
        None
    };
    Node {
        id: details.id.to_string(),
        version: Version { index: 0 },
        created_at: iso_timestamp_from_secs_or_ms(details.registered_at),
        updated_at: iso_timestamp_from_secs_or_ms(details.last_heartbeat),
        spec: NodeSpec {
            name: details.id.to_string(),
            labels: labels_json,
            role: role.clone(),
            availability: "active".to_string(),
        },
        description: NodeDescription {
            hostname: details.id.to_string(),
            platform,
            resources,
            engine,
            tls_info: Value::Object(serde_json::Map::new()),
        },
        status: NodeStatus {
            state,
            message: String::new(),
            addr: details.address.clone(),
        },
        manager_status,
    }
}

/// `NodeDetails.registered_at` / `last_heartbeat` are documented as Unix
/// timestamps; in practice `ClusterNodeSummary` (the shape `From` lives
/// over) treats them as milliseconds. Detect by magnitude and fall through
/// to the safe iso-from-seconds path so both encodings render correctly.
fn iso_timestamp_from_secs_or_ms(value: u64) -> String {
    // Unix seconds will not exceed ~3.2e10 until the year ~3000, while
    // millisecond timestamps from any modern era exceed ~1e12. Anything
    // above ~1e12 must be milliseconds; anything else is seconds.
    let secs = if value > 1_000_000_000_000 {
        i64::try_from(value / 1_000).unwrap_or(0)
    } else {
        i64::try_from(value).unwrap_or(0)
    };
    iso_timestamp(secs)
}

/// Decorate a [`Node`] built from a [`ClusterNodeSummary`] with the
/// description / engine / platform fields that the bare `From` impl in
/// `shape.rs` leaves as defaults.
///
/// `shape.rs` keeps the conversion minimal so it does not have to know
/// about Docker's resource / engine sub-shapes; this helper layers the
/// extra detail in here at the endpoint boundary.
fn enrich_node_from_summary(summary: &ClusterNodeSummary) -> Node {
    let mut node: Node = summary.into();
    let nano_cpus = nano_cpus_from_cores(summary.cpu_total);
    node.description.platform = serde_json::json!({
        "Architecture": "amd64",
        "OS": "linux",
    });
    node.description.resources = serde_json::json!({
        "NanoCPUs": nano_cpus,
        "MemoryBytes": summary.memory_total,
    });
    node.description.engine = serde_json::json!({
        "EngineVersion": env!("CARGO_PKG_VERSION"),
        "Plugins": Vec::<Value>::new(),
    });
    node.description.tls_info = Value::Object(serde_json::Map::new());
    // Spec.Labels: ClusterNodeSummary has no labels field today, so leave
    // the default empty-object the `From` impl already set.
    node
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `GET /nodes` — List cluster nodes as Docker swarm nodes.
async fn list_nodes(State(state): State<SocketState>, Query(q): Query<ListQuery>) -> Response {
    let filters = match parse_node_filters(q.filters.as_deref()) {
        Ok(f) => f,
        Err(msg) => return error_response(StatusCode::BAD_REQUEST, msg),
    };

    let summaries = match state.client.cluster_nodes().await {
        Ok(s) => s,
        Err(e) => {
            return error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to list cluster nodes: {e}"),
            );
        }
    };

    let nodes: Vec<Node> = summaries.iter().map(enrich_node_from_summary).collect();
    let filtered = apply_node_filters(nodes, &filters);
    (StatusCode::OK, Json(filtered)).into_response()
}

/// `GET /nodes/{id}` — Inspect a single cluster node.
async fn inspect_node(State(state): State<SocketState>, Path(id): Path<String>) -> Response {
    match state.client.node_inspect(&id).await {
        Ok(details) => (StatusCode::OK, Json(node_from_details(&details))).into_response(),
        Err(_) => error_response(StatusCode::NOT_FOUND, format!("node {id} not found")),
    }
}

/// `POST /nodes/{id}/update` — Update a node's labels.
///
/// Docker's `NodeSpec` has three mutable fields: `Labels`, `Role`, and
/// `Availability`. `ZLayer` only honours `Labels`; `Role` (manager vs.
/// worker) is decided by Raft, and `Availability` ("active" / "pause" /
/// "drain") has no native equivalent. Both are accepted in the body but
/// silently ignored, so Docker tooling that always sends the full spec
/// does not error out.
async fn update_node(
    State(state): State<SocketState>,
    Path(id): Path<String>,
    Json(body): Json<UpdateBody>,
) -> Response {
    let labels = body.labels.unwrap_or_default();
    // `Role` / `Availability` accepted but no-op'd — see fn docs.
    let _ = body.role;
    let _ = body.availability;

    match state.client.node_set_labels(&id, labels).await {
        Ok(()) => (StatusCode::OK, Json(Value::Object(serde_json::Map::new()))).into_response(),
        Err(e) => {
            let msg = e.to_string();
            // The daemon answers 404 for "node not found"; everything else
            // bubbles up as a 500 with the daemon's own message.
            if msg.contains("not found") || msg.contains("404") {
                error_response(StatusCode::NOT_FOUND, format!("node {id} not found"))
            } else {
                error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("failed to update node labels: {msg}"),
                )
            }
        }
    }
}

/// `DELETE /nodes/{id}` — Not supported; node membership lives behind
/// `zlayer cluster`. Stock Docker uses 501 for "this daemon does not
/// implement this endpoint," which is the right shape here.
async fn delete_node_not_implemented(Path(_id): Path<String>) -> Response {
    error_response(StatusCode::NOT_IMPLEMENTED, NODES_DELETE_MESSAGE)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_filters_handles_well_formed_json() {
        let out = parse_node_filters(Some(r#"{"role":["manager"]}"#)).unwrap();
        assert_eq!(
            out.get("role").map(Vec::as_slice),
            Some(&["manager".to_string()][..])
        );
    }

    #[test]
    fn parse_filters_empty_input_yields_empty_map() {
        assert!(parse_node_filters(None).unwrap().is_empty());
        assert!(parse_node_filters(Some("")).unwrap().is_empty());
        assert!(parse_node_filters(Some("   ")).unwrap().is_empty());
    }

    #[test]
    fn parse_filters_rejects_garbage() {
        assert!(parse_node_filters(Some("not-json")).is_err());
        assert!(parse_node_filters(Some(r#"["not","object"]"#)).is_err());
    }

    #[test]
    fn parse_filters_accepts_map_form() {
        // Docker also accepts `{role: {manager: true}}`.
        let out = parse_node_filters(Some(r#"{"role":{"manager":true,"worker":false}}"#)).unwrap();
        let v = out.get("role").unwrap();
        assert!(v.contains(&"manager".to_string()));
        assert!(!v.contains(&"worker".to_string()));
    }

    fn fixture_node(id: &str, role: &str) -> Node {
        Node {
            id: id.to_string(),
            spec: NodeSpec {
                name: id.to_string(),
                role: role.to_string(),
                availability: "active".to_string(),
                labels: serde_json::json!({"zone": "us-east-1"}),
            },
            ..Node::default()
        }
    }

    #[test]
    fn apply_filters_id_match() {
        let nodes = vec![fixture_node("a", "manager"), fixture_node("b", "worker")];
        let mut filters = HashMap::new();
        filters.insert("id".to_string(), vec!["a".to_string()]);
        let out = apply_node_filters(nodes, &filters);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, "a");
    }

    #[test]
    fn apply_filters_role_match() {
        let nodes = vec![fixture_node("a", "manager"), fixture_node("b", "worker")];
        let mut filters = HashMap::new();
        filters.insert("role".to_string(), vec!["manager".to_string()]);
        let out = apply_node_filters(nodes, &filters);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].spec.role, "manager");
    }

    #[test]
    fn apply_filters_membership_accepted_passes() {
        let nodes = vec![fixture_node("a", "manager")];
        let mut filters = HashMap::new();
        filters.insert("membership".to_string(), vec!["accepted".to_string()]);
        let out = apply_node_filters(nodes.clone(), &filters);
        assert_eq!(out.len(), 1);

        // Non-accepted value should drop everything.
        filters.insert("membership".to_string(), vec!["pending".to_string()]);
        let out = apply_node_filters(nodes, &filters);
        assert!(out.is_empty());
    }

    #[test]
    fn apply_filters_label_key_and_kv() {
        let nodes = vec![fixture_node("a", "manager")];
        let mut filters = HashMap::new();
        filters.insert("label".to_string(), vec!["zone".to_string()]);
        assert_eq!(apply_node_filters(nodes.clone(), &filters).len(), 1);

        filters.insert("label".to_string(), vec!["zone=us-east".to_string()]);
        assert_eq!(apply_node_filters(nodes.clone(), &filters).len(), 1);

        filters.insert("label".to_string(), vec!["zone=eu-west".to_string()]);
        assert!(apply_node_filters(nodes, &filters).is_empty());
    }

    #[test]
    fn label_matches_handles_missing_keys() {
        let labels = serde_json::json!({"zone": "us-east-1"});
        assert!(label_matches(&labels, "zone"));
        assert!(label_matches(&labels, "zone=us-east"));
        assert!(!label_matches(&labels, "missing"));
        assert!(!label_matches(&labels, "missing=value"));
    }

    #[test]
    fn iso_timestamp_helper_handles_secs_and_ms() {
        // Seconds (well below 1e12).
        let s = iso_timestamp_from_secs_or_ms(1_700_000_000);
        assert!(s.contains("2023"));
        // Milliseconds (well above 1e12).
        let s = iso_timestamp_from_secs_or_ms(1_700_000_000_000);
        assert!(s.contains("2023"));
        // Zero falls back to epoch.
        let s = iso_timestamp_from_secs_or_ms(0);
        assert!(s.starts_with("1970"));
    }

    #[test]
    fn node_from_details_maps_fields() {
        let details = NodeDetails {
            id: 7,
            address: "192.168.1.5:9090".to_string(),
            status: "ready".to_string(),
            role: "leader".to_string(),
            labels: HashMap::from([("zone".to_string(), "us-east-1".to_string())]),
            last_seen: 0,
            resources: zlayer_types::api::nodes::NodeResourceInfo {
                cpu_total: 4.0,
                cpu_used: 1.0,
                cpu_percent: 25.0,
                memory_total: 8_589_934_592,
                memory_used: 0,
                memory_percent: 0.0,
            },
            services: Vec::new(),
            registered_at: 1_700_000_000,
            last_heartbeat: 1_700_000_000,
        };
        let node = node_from_details(&details);
        assert_eq!(node.id, "7");
        assert_eq!(node.spec.role, "manager");
        assert_eq!(node.status.state, "ready");
        assert_eq!(node.status.addr, "192.168.1.5:9090");
        assert_eq!(node.spec.availability, "active");
        let nano = node.description.resources["NanoCPUs"].as_u64();
        assert_eq!(nano, Some(4_000_000_000));
        let mem = node.description.resources["MemoryBytes"].as_u64();
        assert_eq!(mem, Some(8_589_934_592));
        let manager = node.manager_status.as_ref().expect("manager status");
        assert!(manager.leader);
        assert_eq!(manager.reachability, "reachable");
    }

    #[test]
    fn node_from_details_worker_omits_manager_status() {
        let details = NodeDetails {
            id: 9,
            address: "10.0.0.9:9090".to_string(),
            status: "ready".to_string(),
            role: "worker".to_string(),
            labels: HashMap::new(),
            last_seen: 0,
            resources: zlayer_types::api::nodes::NodeResourceInfo {
                cpu_total: 0.0,
                cpu_used: 0.0,
                cpu_percent: 0.0,
                memory_total: 0,
                memory_used: 0,
                memory_percent: 0.0,
            },
            services: Vec::new(),
            registered_at: 0,
            last_heartbeat: 0,
        };
        let node = node_from_details(&details);
        assert_eq!(node.spec.role, "worker");
        assert!(node.manager_status.is_none());
    }

    #[test]
    fn enrich_node_from_summary_fills_description() {
        let summary = ClusterNodeSummary {
            id: "1".to_string(),
            address: "10.0.0.1:9090".to_string(),
            advertise_addr: "10.0.0.1".to_string(),
            status: "ready".to_string(),
            role: "leader".to_string(),
            mode: "full".to_string(),
            is_leader: true,
            overlay_ip: "10.200.0.1".to_string(),
            cpu_total: 8.0,
            cpu_used: 2.0,
            memory_total: 17_179_869_184,
            memory_used: 0,
            registered_at: 1_700_000_000_000,
            last_heartbeat: 1_700_000_000_000,
        };
        let node = enrich_node_from_summary(&summary);
        assert_eq!(node.id, "1");
        assert_eq!(node.spec.role, "manager");
        assert_eq!(
            node.description.resources["NanoCPUs"].as_u64(),
            Some(8_000_000_000)
        );
        assert_eq!(
            node.description.engine["EngineVersion"].as_str(),
            Some(env!("CARGO_PKG_VERSION"))
        );
    }
}
