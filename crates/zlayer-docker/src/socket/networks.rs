//! Docker Engine API network endpoints.
//!
//! These handlers proxy requests to the local zlayer daemon via
//! [`DaemonClient`], translating between Docker Engine API v1.43's JSON
//! shapes (`PascalCase`, `Id`/`Name`/`Driver`/`IPAM`/...) and zlayer's
//! native [`zlayer_types::spec::BridgeNetwork`] representation.
//!
//! Supported endpoints:
//!
//! - `GET    /networks`                 — list networks (honours `filters`)
//! - `POST   /networks/create`          — create a network
//! - `GET    /networks/{id}`            — inspect a network
//! - `DELETE /networks/{id}`            — remove a network
//! - `POST   /networks/{id}/connect`    — attach a container
//! - `POST   /networks/{id}/disconnect` — detach a container
//! - `POST   /networks/prune`           — delete unused networks

use std::collections::HashMap;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use zlayer_types::api::container_networks::CreateBridgeNetworkRequest;
use zlayer_types::spec::{BridgeNetwork, BridgeNetworkDriver};

use super::SocketState;

// ---------------------------------------------------------------------------
// Routes
// ---------------------------------------------------------------------------

/// Network API routes.
pub(crate) fn routes(state: SocketState) -> Router {
    Router::new()
        .route("/networks", get(list_networks))
        .route("/networks/create", post(create_network))
        .route("/networks/prune", post(prune_networks))
        .route("/networks/{id}", get(get_network))
        .route("/networks/{id}", delete(remove_network))
        .route("/networks/{id}/connect", post(connect_container))
        .route("/networks/{id}/disconnect", post(disconnect_container))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Build a `{"message": "..."}` JSON error response with the given status.
fn error_response(code: StatusCode, msg: impl Into<String>) -> Response {
    (code, Json(json!({ "message": msg.into() }))).into_response()
}

/// Map a [`DaemonClient`] anyhow error into a Docker-style HTTP response.
///
/// Mirrors the convention used by `containers.rs::map_daemon_error`: the
/// daemon surfaces 404s as error strings prefixed with `"404 Not Found"`,
/// 409s as `"409 Conflict"`, and 400s as `"400 Bad Request"`.
fn map_daemon_error(err: &anyhow::Error) -> Response {
    let msg = format!("{err:#}");
    let lower = msg.to_ascii_lowercase();
    let status = if msg.starts_with("404 ") || lower.contains("404 not found") {
        StatusCode::NOT_FOUND
    } else if msg.starts_with("409 ") || lower.contains("409 conflict") {
        StatusCode::CONFLICT
    } else if msg.starts_with("400 ") || lower.contains("400 bad request") {
        StatusCode::BAD_REQUEST
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    };
    error_response(status, msg)
}

/// Convert a zlayer [`BridgeNetworkDriver`] into Docker's driver string.
fn driver_to_docker(driver: BridgeNetworkDriver) -> &'static str {
    match driver {
        BridgeNetworkDriver::Bridge => "bridge",
        BridgeNetworkDriver::Overlay => "overlay",
    }
}

/// Convert a Docker driver string into a zlayer [`BridgeNetworkDriver`].
///
/// Returns `Err(message)` for drivers we don't model. Empty string is
/// treated as the default (`bridge`).
fn driver_from_docker(driver: Option<&str>) -> Result<BridgeNetworkDriver, String> {
    match driver.map(str::trim) {
        None | Some("" | "bridge") => Ok(BridgeNetworkDriver::Bridge),
        Some("overlay") => Ok(BridgeNetworkDriver::Overlay),
        Some(other) => Err(format!(
            "plugin \"{other}\" not found: zlayer Docker API only supports bridge and overlay drivers"
        )),
    }
}

/// Build a Docker Engine API `NetworkResource` JSON payload from a
/// zlayer [`BridgeNetwork`].
///
/// We use `network.name` for both `Id` and `Name` because zlayer's
/// network identifiers are user-facing UUIDs but most Docker tooling
/// (compose, the CLI, the VS Code extension) round-trips the friendly
/// name. Keeping `Id == Name` makes both forms work.
fn to_network_resource(network: &BridgeNetwork) -> Value {
    let driver = driver_to_docker(network.driver);
    let scope = match network.driver {
        BridgeNetworkDriver::Bridge => "local",
        BridgeNetworkDriver::Overlay => "swarm",
    };

    let ipam_config = match network.subnet.as_deref() {
        Some(subnet) if !subnet.is_empty() => {
            json!([{
                "Subnet": subnet,
                "IPRange": "",
                "Gateway": "",
                "AuxiliaryAddresses": {},
            }])
        }
        _ => json!([]),
    };

    json!({
        "Name": network.name,
        "Id": network.name,
        "Created": network.created_at.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true),
        "Scope": scope,
        "Driver": driver,
        "EnableIPv6": false,
        "IPAM": {
            "Driver": "default",
            "Config": ipam_config,
            "Options": {},
        },
        "Internal": network.internal,
        "Attachable": true,
        "Ingress": false,
        "ConfigFrom": { "Network": "" },
        "ConfigOnly": false,
        "Containers": {},
        "Options": {},
        "Labels": network.labels,
    })
}

// ---------------------------------------------------------------------------
// Filter parsing
// ---------------------------------------------------------------------------

/// Parsed Docker `filters` query value.
///
/// Docker accepts JSON in either of two shapes:
///
/// - `{"name": ["foo"], "label": ["k=v", "k2"]}` — array form
/// - `{"name": {"foo": true}, "label": {"k=v": true}}` — map form
///
/// Unknown keys are ignored. Malformed JSON returns `None` so the caller
/// can fall back to "no filter" rather than 500ing.
#[derive(Debug, Default, Clone)]
struct NetworkFilters {
    name: Vec<String>,
    id: Vec<String>,
    driver: Vec<String>,
    label: Vec<String>,
}

fn extract_filter_values(v: &Value) -> Vec<String> {
    if let Some(arr) = v.as_array() {
        arr.iter()
            .filter_map(|x| x.as_str().map(str::to_owned))
            .collect()
    } else if let Some(obj) = v.as_object() {
        obj.keys().cloned().collect()
    } else {
        Vec::new()
    }
}

fn parse_filters(raw: Option<&str>) -> NetworkFilters {
    let Some(s) = raw else {
        return NetworkFilters::default();
    };
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return NetworkFilters::default();
    }
    let Ok(parsed) = serde_json::from_str::<Value>(trimmed) else {
        return NetworkFilters::default();
    };
    let Some(obj) = parsed.as_object() else {
        return NetworkFilters::default();
    };

    let mut filters = NetworkFilters::default();
    if let Some(v) = obj.get("name") {
        filters.name = extract_filter_values(v);
    }
    if let Some(v) = obj.get("id") {
        filters.id = extract_filter_values(v);
    }
    if let Some(v) = obj.get("driver") {
        filters.driver = extract_filter_values(v);
    }
    if let Some(v) = obj.get("label") {
        filters.label = extract_filter_values(v);
    }
    filters
}

/// Returns true if `network` matches every active filter.
fn matches_filters(network: &BridgeNetwork, filters: &NetworkFilters) -> bool {
    if !filters.name.is_empty() && !filters.name.iter().any(|n| network.name.contains(n)) {
        return false;
    }
    // We use name as the surface id (see `to_network_resource`), so id
    // filters are matched against the name too.
    if !filters.id.is_empty() && !filters.id.iter().any(|i| network.name.contains(i)) {
        return false;
    }
    let driver_str = driver_to_docker(network.driver);
    if !filters.driver.is_empty() && !filters.driver.iter().any(|d| d == driver_str) {
        return false;
    }
    if !filters.label.is_empty() {
        for entry in &filters.label {
            let (key, value) = match entry.split_once('=') {
                Some((k, v)) => (k, Some(v)),
                None => (entry.as_str(), None),
            };
            let Some(actual) = network.labels.get(key) else {
                return false;
            };
            if let Some(expected) = value {
                if actual != expected {
                    return false;
                }
            }
        }
    }
    true
}

// ---------------------------------------------------------------------------
// GET /networks
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
struct ListNetworksQuery {
    #[serde(default)]
    filters: Option<String>,
}

async fn list_networks(
    State(state): State<SocketState>,
    Query(query): Query<ListNetworksQuery>,
) -> Response {
    let networks = match state.client.list_bridge_networks().await {
        Ok(list) => list,
        Err(e) => return map_daemon_error(&e),
    };

    let filters = parse_filters(query.filters.as_deref());
    let resources: Vec<Value> = networks
        .iter()
        .filter(|n| matches_filters(n, &filters))
        .map(to_network_resource)
        .collect();

    Json(resources).into_response()
}

// ---------------------------------------------------------------------------
// POST /networks/create
// ---------------------------------------------------------------------------

/// Subset of Docker's `IPAM` body shape we honour on create.
#[derive(Debug, Default, Deserialize)]
struct DockerIpamBody {
    #[serde(default, rename = "Config")]
    config: Vec<DockerIpamPoolBody>,
    // `Driver` and `Options` are accepted but unused — zlayer always
    // uses the default IPAM driver internally.
    #[serde(default, rename = "Driver")]
    #[allow(dead_code)]
    driver: Option<String>,
    #[serde(default, rename = "Options")]
    #[allow(dead_code)]
    options: HashMap<String, String>,
}

#[derive(Debug, Default, Deserialize)]
struct DockerIpamPoolBody {
    #[serde(default, rename = "Subnet")]
    subnet: Option<String>,
    #[serde(default, rename = "IPRange")]
    #[allow(dead_code)]
    ip_range: Option<String>,
    #[serde(default, rename = "Gateway")]
    #[allow(dead_code)]
    gateway: Option<String>,
    #[serde(default, rename = "AuxiliaryAddresses")]
    #[allow(dead_code)]
    aux_addresses: HashMap<String, String>,
}

/// Body for `POST /networks/create`.
///
/// Mirrors Docker's wire shape directly, which is why several boolean
/// fields appear here — they're not controlled by us and a state machine
/// abstraction would only obscure the JSON contract. `clippy::struct_excessive_bools`
/// is therefore silenced for this passive deserialization target.
#[derive(Debug, Deserialize)]
#[allow(clippy::struct_excessive_bools)]
struct NetworkCreateBody {
    #[serde(rename = "Name")]
    name: String,
    #[serde(default, rename = "Driver")]
    driver: Option<String>,
    #[serde(default, rename = "Internal")]
    internal: bool,
    // The rest are accepted but largely unused — they mirror Docker's
    // shape so clients don't choke on them.
    #[serde(default, rename = "CheckDuplicate")]
    #[allow(dead_code)]
    check_duplicate: bool,
    #[serde(default, rename = "Attachable")]
    #[allow(dead_code)]
    attachable: bool,
    #[serde(default, rename = "Ingress")]
    #[allow(dead_code)]
    ingress: bool,
    #[serde(default, rename = "EnableIPv6")]
    #[allow(dead_code)]
    enable_ipv6: bool,
    #[serde(default, rename = "IPAM")]
    ipam: Option<DockerIpamBody>,
    #[serde(default, rename = "Options")]
    #[allow(dead_code)]
    options: HashMap<String, String>,
    #[serde(default, rename = "Labels")]
    labels: HashMap<String, String>,
}

/// Translate a Docker `NetworkCreateBody` into a zlayer
/// [`CreateBridgeNetworkRequest`].
///
/// Pure helper so it can be unit-tested without spinning up a daemon.
fn translate_create_body(
    body: NetworkCreateBody,
) -> Result<CreateBridgeNetworkRequest, (StatusCode, String)> {
    if body.name.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "network name is required".to_owned(),
        ));
    }

    let driver =
        driver_from_docker(body.driver.as_deref()).map_err(|msg| (StatusCode::BAD_REQUEST, msg))?;

    // First IPAM pool with a `Subnet` wins. Docker tooling typically only
    // sends one entry; if more are sent we silently take the first.
    let subnet = body.ipam.and_then(|ipam| {
        ipam.config
            .into_iter()
            .find_map(|p| p.subnet.filter(|s| !s.trim().is_empty()))
    });

    Ok(CreateBridgeNetworkRequest {
        name: body.name,
        driver: Some(driver),
        subnet,
        labels: body.labels,
        internal: body.internal,
    })
}

async fn create_network(
    State(state): State<SocketState>,
    Json(body): Json<NetworkCreateBody>,
) -> Response {
    let request = match translate_create_body(body) {
        Ok(req) => req,
        Err((code, msg)) => return error_response(code, msg),
    };

    let name = request.name.clone();
    match state.client.create_bridge_network(request).await {
        Ok(network) => (
            StatusCode::CREATED,
            Json(json!({
                "Id": network.name,
                "Warning": "",
            })),
        )
            .into_response(),
        Err(e) => {
            // Detect "already exists" and surface it as a 409 with Docker's
            // canonical message even when the daemon string didn't already
            // contain a status prefix.
            let lower = format!("{e:#}").to_ascii_lowercase();
            if lower.contains("already exists") || lower.contains("409 conflict") {
                return error_response(
                    StatusCode::CONFLICT,
                    format!("network with name {name} already exists"),
                );
            }
            map_daemon_error(&e)
        }
    }
}

// ---------------------------------------------------------------------------
// GET /networks/{id}
// ---------------------------------------------------------------------------

async fn get_network(State(state): State<SocketState>, Path(id): Path<String>) -> Response {
    match state.client.get_bridge_network(&id).await {
        Ok(Some(network)) => Json(to_network_resource(&network)).into_response(),
        Ok(None) => error_response(StatusCode::NOT_FOUND, format!("network {id} not found")),
        Err(e) => map_daemon_error(&e),
    }
}

// ---------------------------------------------------------------------------
// DELETE /networks/{id}
// ---------------------------------------------------------------------------

async fn remove_network(State(state): State<SocketState>, Path(id): Path<String>) -> Response {
    match state.client.delete_bridge_network(&id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => error_response(StatusCode::NOT_FOUND, format!("network {id} not found")),
        Err(e) => map_daemon_error(&e),
    }
}

// ---------------------------------------------------------------------------
// POST /networks/{id}/connect, /disconnect
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
struct EndpointConfigBody {
    #[serde(default, rename = "Aliases")]
    aliases: Vec<String>,
    // The rest are accepted but unused — zlayer's connect helper only
    // takes container + aliases.
    #[serde(default, rename = "IPAMConfig")]
    #[allow(dead_code)]
    ipam_config: Option<Value>,
    #[serde(default, rename = "Links")]
    #[allow(dead_code)]
    links: Vec<String>,
    #[serde(default, rename = "MacAddress")]
    #[allow(dead_code)]
    mac_address: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ConnectBody {
    #[serde(rename = "Container")]
    container: String,
    #[serde(default, rename = "EndpointConfig")]
    endpoint_config: Option<EndpointConfigBody>,
}

#[derive(Debug, Deserialize)]
struct DisconnectBody {
    #[serde(rename = "Container")]
    container: String,
    #[serde(default, rename = "Force")]
    force: bool,
}

async fn connect_container(
    State(state): State<SocketState>,
    Path(id): Path<String>,
    Json(body): Json<ConnectBody>,
) -> Response {
    if body.container.trim().is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "container is required");
    }
    let aliases = body
        .endpoint_config
        .map(|ec| ec.aliases)
        .unwrap_or_default();
    match state
        .client
        .connect_container_to_bridge_network(&id, &body.container, aliases)
        .await
    {
        Ok(()) => StatusCode::OK.into_response(),
        Err(e) => map_daemon_error(&e),
    }
}

async fn disconnect_container(
    State(state): State<SocketState>,
    Path(id): Path<String>,
    Json(body): Json<DisconnectBody>,
) -> Response {
    if body.container.trim().is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "container is required");
    }
    match state
        .client
        .disconnect_container_from_bridge_network(&id, &body.container, body.force)
        .await
    {
        Ok(()) => StatusCode::OK.into_response(),
        Err(e) => map_daemon_error(&e),
    }
}

// ---------------------------------------------------------------------------
// POST /networks/prune
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
struct PruneQuery {
    #[serde(default)]
    filters: Option<String>,
}

#[derive(Debug, Default, Clone)]
struct PruneFilters {
    until: Option<String>,
    label: Vec<String>,
}

fn parse_prune_filters(raw: Option<&str>) -> PruneFilters {
    let Some(s) = raw else {
        return PruneFilters::default();
    };
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return PruneFilters::default();
    }
    let Ok(parsed) = serde_json::from_str::<Value>(trimmed) else {
        return PruneFilters::default();
    };
    let Some(obj) = parsed.as_object() else {
        return PruneFilters::default();
    };

    let mut filters = PruneFilters::default();
    if let Some(v) = obj.get("until") {
        let values = extract_filter_values(v);
        filters.until = values.into_iter().next();
    }
    if let Some(v) = obj.get("label") {
        filters.label = extract_filter_values(v);
    }
    filters
}

/// Test whether a network passes prune-time label filters.
fn prune_label_match(network: &BridgeNetwork, filters: &PruneFilters) -> bool {
    if filters.label.is_empty() {
        return true;
    }
    for entry in &filters.label {
        let (key, value) = match entry.split_once('=') {
            Some((k, v)) => (k, Some(v)),
            None => (entry.as_str(), None),
        };
        let Some(actual) = network.labels.get(key) else {
            return false;
        };
        if let Some(expected) = value {
            if actual != expected {
                return false;
            }
        }
    }
    true
}

/// Test whether a network passes the `until` filter.
///
/// Docker accepts both unix timestamps and RFC3339 strings here. We
/// support unix seconds (numeric string) and RFC3339; anything else is
/// treated as "no filter" (network passes).
fn prune_until_match(network: &BridgeNetwork, filters: &PruneFilters) -> bool {
    let Some(raw) = filters.until.as_deref() else {
        return true;
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return true;
    }
    if let Ok(secs) = trimmed.parse::<i64>() {
        let cutoff = chrono::DateTime::<chrono::Utc>::from_timestamp(secs, 0);
        return cutoff.is_none_or(|c| network.created_at < c);
    }
    if let Ok(cutoff) = chrono::DateTime::parse_from_rfc3339(trimmed) {
        return network.created_at < cutoff.with_timezone(&chrono::Utc);
    }
    true
}

/// Response body for `POST /networks/prune`.
#[derive(Debug, Default, Serialize)]
struct PruneResponse {
    #[serde(rename = "NetworksDeleted")]
    networks_deleted: Vec<String>,
}

async fn prune_networks(
    State(state): State<SocketState>,
    Query(query): Query<PruneQuery>,
) -> Response {
    let filters = parse_prune_filters(query.filters.as_deref());

    let networks = match state.client.list_bridge_networks().await {
        Ok(list) => list,
        Err(e) => return map_daemon_error(&e),
    };

    // "Unused" = no attached containers. The daemon refuses to delete
    // networks that still have attachments (returning either an error or
    // `Ok(false)` from `delete_bridge_network`), so we let the daemon
    // enforce that invariant and collect names of networks that actually
    // got removed. This avoids needing a separate inspect call per
    // network and matches Docker's prune semantics: only successfully
    // removed networks appear in `NetworksDeleted`.
    let mut deleted = Vec::new();
    for network in networks {
        if !prune_label_match(&network, &filters) {
            continue;
        }
        if !prune_until_match(&network, &filters) {
            continue;
        }

        // 404 / in-use refusal silently skips — matches Docker's
        // behaviour of only listing successfully removed networks.
        if let Ok(true) = state.client.delete_bridge_network(&network.name).await {
            deleted.push(network.name);
        }
    }

    Json(PruneResponse {
        networks_deleted: deleted,
    })
    .into_response()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn sample_network(name: &str) -> BridgeNetwork {
        BridgeNetwork {
            id: format!("id-{name}"),
            name: name.to_owned(),
            driver: BridgeNetworkDriver::Bridge,
            subnet: Some("10.240.0.0/24".to_owned()),
            labels: HashMap::from([("env".to_owned(), "prod".to_owned())]),
            internal: false,
            created_at: chrono::Utc.with_ymd_and_hms(2024, 1, 2, 3, 4, 5).unwrap(),
        }
    }

    // ----- list_networks_maps_daemon_to_docker_shape ----------------------
    #[test]
    fn list_networks_maps_daemon_to_docker_shape() {
        let net = sample_network("frontend");
        let resource = to_network_resource(&net);

        // Required PascalCase keys present.
        assert_eq!(resource["Name"], "frontend");
        assert_eq!(resource["Id"], "frontend");
        assert_eq!(resource["Driver"], "bridge");
        assert_eq!(resource["Scope"], "local");
        assert_eq!(resource["Internal"], false);
        assert_eq!(resource["EnableIPv6"], false);
        assert_eq!(resource["Attachable"], true);
        assert_eq!(resource["IPAM"]["Driver"], "default");

        // IPAM config carries the subnet.
        let pool = &resource["IPAM"]["Config"][0];
        assert_eq!(pool["Subnet"], "10.240.0.0/24");
        assert!(pool["Gateway"].is_string());
        assert!(pool["IPRange"].is_string());

        // Labels round-trip.
        assert_eq!(resource["Labels"]["env"], "prod");

        // Created timestamp is RFC3339.
        let created = resource["Created"].as_str().unwrap();
        assert!(created.starts_with("2024-01-02T03:04:05"));
        assert!(created.ends_with('Z'));
    }

    #[test]
    fn list_networks_overlay_scope_is_swarm() {
        let mut net = sample_network("overlay-net");
        net.driver = BridgeNetworkDriver::Overlay;
        net.subnet = None;
        let resource = to_network_resource(&net);
        assert_eq!(resource["Driver"], "overlay");
        assert_eq!(resource["Scope"], "swarm");
        // No subnet -> empty IPAM Config array (not null).
        assert!(resource["IPAM"]["Config"].as_array().unwrap().is_empty());
    }

    // ----- create_network_translates_body_to_request ----------------------
    #[test]
    fn create_network_translates_body_to_request() {
        let body: NetworkCreateBody = serde_json::from_value(json!({
            "Name": "mynet",
            "Driver": "bridge",
            "Internal": true,
            "EnableIPv6": false,
            "Attachable": true,
            "IPAM": {
                "Driver": "default",
                "Config": [
                    { "Subnet": "172.20.0.0/16", "Gateway": "172.20.0.1" }
                ],
                "Options": {}
            },
            "Labels": { "team": "platform" },
            "Options": {}
        }))
        .unwrap();

        let req = translate_create_body(body).expect("translation should succeed");
        assert_eq!(req.name, "mynet");
        assert_eq!(req.driver, Some(BridgeNetworkDriver::Bridge));
        assert_eq!(req.subnet.as_deref(), Some("172.20.0.0/16"));
        assert!(req.internal);
        assert_eq!(req.labels.get("team").map(String::as_str), Some("platform"));
    }

    #[test]
    fn create_network_defaults_driver_to_bridge() {
        let body: NetworkCreateBody = serde_json::from_value(json!({ "Name": "n" })).unwrap();
        let req = translate_create_body(body).unwrap();
        assert_eq!(req.driver, Some(BridgeNetworkDriver::Bridge));
        assert!(req.subnet.is_none());
        assert!(!req.internal);
    }

    #[test]
    fn create_network_overlay_supported() {
        let body: NetworkCreateBody = serde_json::from_value(json!({
            "Name": "overlaynet",
            "Driver": "overlay",
        }))
        .unwrap();
        let req = translate_create_body(body).unwrap();
        assert_eq!(req.driver, Some(BridgeNetworkDriver::Overlay));
    }

    #[test]
    fn create_network_rejects_unsupported_driver() {
        let body: NetworkCreateBody = serde_json::from_value(json!({
            "Name": "n",
            "Driver": "macvlan"
        }))
        .unwrap();
        let err = translate_create_body(body).unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
        assert!(err.1.contains("macvlan"));
    }

    #[test]
    fn create_network_rejects_empty_name() {
        let body: NetworkCreateBody = serde_json::from_value(json!({ "Name": "  " })).unwrap();
        let err = translate_create_body(body).unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn create_network_takes_first_pool_subnet() {
        let body: NetworkCreateBody = serde_json::from_value(json!({
            "Name": "n",
            "IPAM": {
                "Config": [
                    { "Subnet": "10.0.0.0/8" },
                    { "Subnet": "192.168.0.0/16" }
                ]
            }
        }))
        .unwrap();
        let req = translate_create_body(body).unwrap();
        assert_eq!(req.subnet.as_deref(), Some("10.0.0.0/8"));
    }

    // ----- delete_network_404_on_missing ---------------------------------
    //
    // We exercise the daemon-error mapper and the explicit `Ok(false)`
    // branch via the same helpers the handler uses, so missing-network
    // responses are 404 in both shapes.
    #[test]
    fn delete_network_404_on_missing() {
        let resp = error_response(StatusCode::NOT_FOUND, "network foo not found");
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);

        let err = anyhow::anyhow!("404 Not Found: network foo");
        let mapped = map_daemon_error(&err);
        assert_eq!(mapped.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn map_daemon_error_translates_status_codes() {
        let cases = [
            ("404 Not Found", StatusCode::NOT_FOUND),
            ("409 Conflict: dup", StatusCode::CONFLICT),
            ("400 Bad Request: bad", StatusCode::BAD_REQUEST),
            ("connection refused", StatusCode::INTERNAL_SERVER_ERROR),
        ];
        for (msg, expected) in cases {
            let mapped = map_daemon_error(&anyhow::anyhow!("{msg}"));
            assert_eq!(mapped.status(), expected, "for {msg}");
        }
    }

    // ----- prune_networks_returns_docker_shape ---------------------------
    #[test]
    fn prune_networks_returns_docker_shape() {
        let resp = PruneResponse {
            networks_deleted: vec!["a".to_owned(), "b".to_owned()],
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json, json!({ "NetworksDeleted": ["a", "b"] }));

        let empty = PruneResponse::default();
        assert_eq!(
            serde_json::to_value(&empty).unwrap(),
            json!({ "NetworksDeleted": [] })
        );
    }

    #[test]
    fn prune_label_filter_excludes_non_matching() {
        let net = sample_network("frontend");
        let filters = PruneFilters {
            until: None,
            label: vec!["env=prod".to_owned()],
        };
        assert!(prune_label_match(&net, &filters));

        let filters = PruneFilters {
            until: None,
            label: vec!["env=staging".to_owned()],
        };
        assert!(!prune_label_match(&net, &filters));

        // Bare key matches when present regardless of value.
        let filters = PruneFilters {
            until: None,
            label: vec!["env".to_owned()],
        };
        assert!(prune_label_match(&net, &filters));

        // Missing key fails.
        let filters = PruneFilters {
            until: None,
            label: vec!["missing".to_owned()],
        };
        assert!(!prune_label_match(&net, &filters));
    }

    #[test]
    fn prune_until_filter_compares_created_at() {
        let net = sample_network("frontend"); // 2024-01-02T03:04:05Z
        let before = PruneFilters {
            until: Some("2024-01-01T00:00:00Z".to_owned()),
            label: Vec::new(),
        };
        // Network was created AFTER the cutoff -> should NOT prune.
        assert!(!prune_until_match(&net, &before));

        let after = PruneFilters {
            until: Some("2025-01-01T00:00:00Z".to_owned()),
            label: Vec::new(),
        };
        assert!(prune_until_match(&net, &after));

        // Unparseable -> permissive.
        let bad = PruneFilters {
            until: Some("not-a-date".to_owned()),
            label: Vec::new(),
        };
        assert!(prune_until_match(&net, &bad));
    }

    // ----- connect_disconnect_round_trip ---------------------------------
    //
    // We can't spin up a real daemon in this test crate, so we exercise
    // the body parsing + alias extraction the handler uses end-to-end.
    #[test]
    fn connect_disconnect_round_trip() {
        let connect: ConnectBody = serde_json::from_value(json!({
            "Container": "abc123",
            "EndpointConfig": {
                "Aliases": ["api", "api-v1"],
                "IPAMConfig": { "IPv4Address": "10.0.0.5" },
                "Links": [],
                "MacAddress": "02:42:ac:11:00:02"
            }
        }))
        .unwrap();
        assert_eq!(connect.container, "abc123");
        let aliases = connect
            .endpoint_config
            .map(|ec| ec.aliases)
            .unwrap_or_default();
        assert_eq!(aliases, vec!["api".to_owned(), "api-v1".to_owned()]);

        // Connect body without EndpointConfig.
        let connect: ConnectBody =
            serde_json::from_value(json!({ "Container": "abc123" })).unwrap();
        assert!(connect.endpoint_config.is_none());

        // Disconnect with explicit Force.
        let disconnect: DisconnectBody = serde_json::from_value(json!({
            "Container": "abc123",
            "Force": true,
        }))
        .unwrap();
        assert_eq!(disconnect.container, "abc123");
        assert!(disconnect.force);

        // Force defaults to false.
        let disconnect: DisconnectBody =
            serde_json::from_value(json!({ "Container": "abc123" })).unwrap();
        assert!(!disconnect.force);
    }

    // ----- filter parsing -------------------------------------------------
    #[test]
    fn parse_filters_array_form() {
        let raw = r#"{"name":["foo","bar"],"label":["env=prod"]}"#;
        let f = parse_filters(Some(raw));
        assert_eq!(f.name, vec!["foo".to_owned(), "bar".to_owned()]);
        assert_eq!(f.label, vec!["env=prod".to_owned()]);
    }

    #[test]
    fn parse_filters_map_form() {
        let raw = r#"{"name":{"foo":true},"driver":{"bridge":true}}"#;
        let f = parse_filters(Some(raw));
        assert_eq!(f.name, vec!["foo".to_owned()]);
        assert_eq!(f.driver, vec!["bridge".to_owned()]);
    }

    #[test]
    fn parse_filters_malformed_returns_default() {
        let f = parse_filters(Some("not json"));
        assert!(f.name.is_empty());
        assert!(f.label.is_empty());
    }

    #[test]
    fn matches_filters_combines_all_clauses() {
        let net = sample_network("frontend");

        let f = NetworkFilters {
            name: vec!["frontend".to_owned()],
            id: Vec::new(),
            driver: vec!["bridge".to_owned()],
            label: vec!["env=prod".to_owned()],
        };
        assert!(matches_filters(&net, &f));

        let f = NetworkFilters {
            name: Vec::new(),
            id: Vec::new(),
            driver: vec!["overlay".to_owned()],
            label: Vec::new(),
        };
        assert!(!matches_filters(&net, &f));
    }

    // ----- driver translation --------------------------------------------
    #[test]
    fn driver_round_trip() {
        assert_eq!(
            driver_from_docker(Some("bridge")).unwrap(),
            BridgeNetworkDriver::Bridge
        );
        assert_eq!(
            driver_from_docker(Some("overlay")).unwrap(),
            BridgeNetworkDriver::Overlay
        );
        assert_eq!(
            driver_from_docker(None).unwrap(),
            BridgeNetworkDriver::Bridge
        );
        assert_eq!(
            driver_from_docker(Some("")).unwrap(),
            BridgeNetworkDriver::Bridge
        );

        assert!(driver_from_docker(Some("host")).is_err());
        assert!(driver_from_docker(Some("null")).is_err());

        assert_eq!(driver_to_docker(BridgeNetworkDriver::Bridge), "bridge");
        assert_eq!(driver_to_docker(BridgeNetworkDriver::Overlay), "overlay");
    }
}
