//! Docker Engine API image endpoints.
//!
//! Bridges the `/images/*` surface of the Docker Engine API (v1.43) to the
//! running zlayer daemon via [`zlayer_client::DaemonClient`]. Tools that
//! speak Docker — `docker images`, `docker pull`, `docker tag`, `docker rmi`
//! and anything that talks to `/var/run/docker.sock` — can drive zlayer
//! through this router.

use std::collections::HashMap;

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use super::SocketState;

// Keep the typed `ImageSummary` schema alive for external consumers (openapi
// generators, sibling handlers). We emit `serde_json::Value` bodies here so
// we can include Docker fields that the strongly-typed wrapper doesn't carry
// yet (`SharedSize`, `Containers`), but the reference definition still lives
// next to the other `/socket/types.rs` response shapes.
#[allow(dead_code)]
type _ImageSummaryKeepalive = super::types::ImageSummary;

/// Docker-style error response. Docker clients look for a `message` field on
/// a JSON body to surface a meaningful error to the user.
struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }

    fn upstream(e: impl std::fmt::Display) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: e.to_string(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({ "message": self.message }))).into_response()
    }
}

/// Image API routes.
pub(crate) fn routes(state: SocketState) -> Router {
    Router::new()
        .route("/images/json", get(list_images))
        .route("/images/create", post(pull_image))
        .route("/build", post(build_image))
        .route("/images/{name}/tag", post(tag_image))
        .route("/images/{name}", delete(remove_image))
        .route("/images/{name}/json", get(inspect_image))
        .route("/images/{name}/push", post(push_image))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// GET /images/json
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ListImagesQuery {
    /// Docker exposes this flag; we return the same set regardless since the
    /// daemon does not distinguish "dangling" images in its cache listing.
    #[allow(dead_code)]
    all: Option<String>,
    /// Raw JSON-encoded filter map. Accepted and logged but not interpreted
    /// (matches what `docker images` sends even when no filters are given).
    #[allow(dead_code)]
    filters: Option<String>,
}

/// `GET /images/json` — List images.
async fn list_images(
    State(state): State<SocketState>,
    Query(_q): Query<ListImagesQuery>,
) -> Result<Json<Vec<Value>>, ApiError> {
    let images = state
        .client
        .list_images()
        .await
        .map_err(ApiError::upstream)?;

    let now = unix_timestamp();

    let summaries: Vec<Value> = images
        .into_iter()
        .map(|img| {
            let id = img
                .digest
                .clone()
                .unwrap_or_else(|| format!("sha256:{}", hash_ref(&img.reference)));
            let size = i64::try_from(img.size_bytes.unwrap_or(0)).unwrap_or(0);
            let repo_tags = if img.reference.is_empty() {
                vec!["<none>:<none>".to_owned()]
            } else {
                vec![img.reference.clone()]
            };
            let repo_digests = match &img.digest {
                Some(d) => vec![format!("{}@{}", strip_tag(&img.reference), d)],
                None => Vec::new(),
            };

            json!({
                "Id": id,
                "ParentId": "",
                "RepoTags": repo_tags,
                "RepoDigests": repo_digests,
                "Created": now,
                "Size": size,
                "SharedSize": -1_i64,
                "VirtualSize": size,
                "Labels": HashMap::<String, String>::new(),
                "Containers": -1_i64,
            })
        })
        .collect();

    Ok(Json(summaries))
}

// ---------------------------------------------------------------------------
// GET /images/{name}/json
// ---------------------------------------------------------------------------

/// `GET /images/{name}/json` — Inspect an image.
///
/// Returns a Docker-shaped inspect payload. We populate the fields we can
/// derive from `DaemonClient::list_images()` (`Id`, `RepoTags`, `Size`, `Created`)
/// and leave the rest as sensible defaults so Docker-aware tools do not
/// choke on missing keys.
async fn inspect_image(
    State(state): State<SocketState>,
    Path(name): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let images = state
        .client
        .list_images()
        .await
        .map_err(ApiError::upstream)?;

    let found = images
        .into_iter()
        .find(|img| img.reference == name || img.digest.as_deref() == Some(name.as_str()))
        .ok_or_else(|| ApiError {
            status: StatusCode::NOT_FOUND,
            message: format!("No such image: {name}"),
        })?;

    let id = found
        .digest
        .clone()
        .unwrap_or_else(|| format!("sha256:{}", hash_ref(&found.reference)));
    let size = i64::try_from(found.size_bytes.unwrap_or(0)).unwrap_or(0);
    let created = rfc3339_now();
    let repo_tags = if found.reference.is_empty() {
        vec!["<none>:<none>".to_owned()]
    } else {
        vec![found.reference.clone()]
    };
    let repo_digests = match &found.digest {
        Some(d) => vec![format!("{}@{}", strip_tag(&found.reference), d)],
        None => Vec::new(),
    };

    Ok(Json(json!({
        "Id": id,
        "RepoTags": repo_tags,
        "RepoDigests": repo_digests,
        "Parent": "",
        "Comment": "",
        "Created": created,
        "Container": "",
        "ContainerConfig": json!({}),
        "DockerVersion": env!("CARGO_PKG_VERSION"),
        "Author": "",
        "Config": json!({
            "Env": Vec::<String>::new(),
            "Cmd": Vec::<String>::new(),
            "Entrypoint": Vec::<String>::new(),
            "Labels": HashMap::<String, String>::new(),
        }),
        "Architecture": std::env::consts::ARCH,
        "Os": std::env::consts::OS,
        "Size": size,
        "VirtualSize": size,
        "GraphDriver": json!({
            "Name": "zlayer",
            "Data": HashMap::<String, String>::new(),
        }),
        "RootFS": json!({
            "Type": "layers",
            "Layers": Vec::<String>::new(),
        }),
    })))
}

// ---------------------------------------------------------------------------
// POST /images/create (pull)
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct PullImageQuery {
    #[serde(rename = "fromImage")]
    from_image: Option<String>,
    tag: Option<String>,
    /// Docker also supports `fromSrc` (import from tarball) — not implemented.
    #[serde(rename = "fromSrc")]
    #[allow(dead_code)]
    from_src: Option<String>,
}

/// `POST /images/create` — Pull an image.
///
/// Docker returns `application/x-ndjson`: one JSON object per line describing
/// pull progress. Because [`DaemonClient::pull_image_from_server`] is a
/// blocking call that returns only once the pull finishes, we emit a single
/// terminal `{"status":"Downloaded newer image for X"}` line and close the
/// stream. That is enough for the `docker pull` client to report success.
///
/// TODO: wire richer progress events once the daemon exposes a streaming
/// pull endpoint (layer-level `progressDetail`, `id`, etc.).
async fn pull_image(
    State(state): State<SocketState>,
    Query(q): Query<PullImageQuery>,
) -> Result<Response, ApiError> {
    let image = q
        .from_image
        .ok_or_else(|| ApiError::bad_request("missing fromImage query parameter"))?;
    let reference = match q.tag.as_deref() {
        Some(tag) if !tag.is_empty() && !image.contains('@') && !image.contains(':') => {
            format!("{image}:{tag}")
        }
        _ => image.clone(),
    };

    let resp = state
        .client
        .pull_image_from_server(&reference, None)
        .await
        .map_err(ApiError::upstream)?;

    // Line-delimited JSON body — Docker's streaming pull contract.
    let mut body = String::new();
    body.push_str(
        &json!({
            "status": format!("Pulling from {}", strip_tag(&resp.reference)),
        })
        .to_string(),
    );
    body.push('\n');
    if let Some(digest) = resp.digest.as_deref() {
        body.push_str(
            &json!({
                "status": format!("Digest: {digest}"),
            })
            .to_string(),
        );
        body.push('\n');
    }
    body.push_str(
        &json!({
            "status": format!("Status: Downloaded newer image for {}", resp.reference),
        })
        .to_string(),
    );
    body.push('\n');

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/x-ndjson")
        .body(Body::from(body))
        .map_err(ApiError::upstream)
}

// ---------------------------------------------------------------------------
// POST /images/{name}/tag
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct TagImageQuery {
    repo: Option<String>,
    tag: Option<String>,
}

/// `POST /images/{name}/tag` — Tag an image.
async fn tag_image(
    State(state): State<SocketState>,
    Path(name): Path<String>,
    Query(q): Query<TagImageQuery>,
) -> Result<StatusCode, ApiError> {
    let repo = q
        .repo
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::bad_request("missing repo query parameter"))?;
    let target = match q.tag.as_deref() {
        Some(tag) if !tag.is_empty() => format!("{repo}:{tag}"),
        _ => repo,
    };

    state
        .client
        .tag_image(&name, &target)
        .await
        .map_err(ApiError::upstream)?;

    Ok(StatusCode::CREATED)
}

// ---------------------------------------------------------------------------
// DELETE /images/{name}
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RemoveImageQuery {
    force: Option<String>,
    /// Docker's `noprune` flag — accepted for compatibility, ignored.
    #[allow(dead_code)]
    noprune: Option<String>,
}

/// `DELETE /images/{name}` — Remove an image.
async fn remove_image(
    State(state): State<SocketState>,
    Path(name): Path<String>,
    Query(q): Query<RemoveImageQuery>,
) -> Result<Json<Vec<Value>>, ApiError> {
    let force = matches!(q.force.as_deref(), Some("1" | "true" | "True" | "yes"));

    // Look up the image first so we can report the canonical digest Docker
    // expects in the `Deleted` entry. Missing digest is fine — we fall back
    // to a deterministic synthetic id.
    let digest = state
        .client
        .list_images()
        .await
        .ok()
        .and_then(|images| {
            images
                .into_iter()
                .find(|img| img.reference == name || img.digest.as_deref() == Some(name.as_str()))
                .and_then(|img| img.digest)
        })
        .unwrap_or_else(|| format!("sha256:{}", hash_ref(&name)));

    state
        .client
        .remove_image(&name, force)
        .await
        .map_err(ApiError::upstream)?;

    Ok(Json(vec![
        json!({ "Untagged": name }),
        json!({ "Deleted": digest }),
    ]))
}

// ---------------------------------------------------------------------------
// POST /build (stubbed — build uses a separate daemon endpoint)
// ---------------------------------------------------------------------------

/// `POST /build` — Build an image.
///
/// Docker's build endpoint ships a tar of the build context and streams NDJSON
/// progress events. Wiring that through to `DaemonClient::start_build` is a
/// larger task tracked separately; for now this remains a stub so the router
/// wiring continues to compile.
async fn build_image() -> impl IntoResponse {
    tracing::warn!("docker API: POST /build — not implemented");
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(json!({
            "message": "image build not yet implemented"
        })),
    )
}

// ---------------------------------------------------------------------------
// POST /images/{name}/push
// ---------------------------------------------------------------------------

/// `POST /images/{name}/push` — Push an image to a registry.
///
/// Daemon-side push requires registry credentials (X-Registry-Auth header);
/// we accept the request and kick off a server push but return a minimal
/// NDJSON success line. Rich per-layer progress remains a TODO.
async fn push_image(
    State(state): State<SocketState>,
    Path(name): Path<String>,
) -> Result<Response, ApiError> {
    state
        .client
        .push_image(&name, None, None)
        .await
        .map_err(ApiError::upstream)?;

    let body = format!("{}\n", json!({ "status": format!("Pushed {name}") }));
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/x-ndjson")
        .body(Body::from(body))
        .map_err(ApiError::upstream)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Seconds since the Unix epoch. Docker's `/images/json` wants an integer
/// `Created` field.
fn unix_timestamp() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_secs()).unwrap_or(0))
        .unwrap_or(0)
}

/// RFC 3339 timestamp. Docker's inspect response uses a string rather than
/// the integer used in the list response. Formatted manually to avoid pulling
/// in `chrono` as a new dependency.
fn rfc3339_now() -> String {
    // Seconds since 1970-01-01T00:00:00Z.
    let secs = unix_timestamp();
    // Split into y/m/d/h/m/s using the standard civil-date algorithm.
    let (y, mo, d, h, mi, s) = civil_from_epoch(secs);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

/// Convert Unix epoch seconds to a civil (year, month, day, hour, minute,
/// second) tuple using Howard Hinnant's algorithm. Accurate for all dates
/// in the proleptic Gregorian calendar.
///
/// Variable names are kept terse to match Hinnant's published algorithm
/// (`z`, `doe`, `yoe`, `doy`, `mp`, `era`).
#[allow(clippy::many_single_char_names)]
fn civil_from_epoch(secs: i64) -> (i32, u32, u32, u32, u32, u32) {
    let days = secs.div_euclid(86_400);
    let seconds_of_day = secs.rem_euclid(86_400);
    let h = u32::try_from(seconds_of_day / 3600).unwrap_or(0);
    let mi = u32::try_from((seconds_of_day % 3600) / 60).unwrap_or(0);
    let s = u32::try_from(seconds_of_day % 60).unwrap_or(0);

    // Shift epoch from 1970-01-01 to 0000-03-01 (Hinnant's reference date).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = u32::try_from(z.rem_euclid(146_097)).unwrap_or(0);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = i64::from(yoe) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = i32::try_from(if m <= 2 { y + 1 } else { y }).unwrap_or(1970);
    (y, m, d, h, mi, s)
}

/// Docker wants a `sha256:` id even when we have nothing real to hand over.
/// Fall back to a deterministic (non-cryptographic) hash of the reference so
/// the same image reports a stable synthetic id across calls.
fn hash_ref(reference: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    reference.hash(&mut hasher);
    let h = hasher.finish();
    // Pad to a 64-char hex string so it looks the part.
    format!("{h:016x}{h:016x}{h:016x}{h:016x}")
}

/// Strip the `:tag` (but not `@digest`) off an image reference so we can
/// re-combine it with a digest in `RepoDigests`.
fn strip_tag(reference: &str) -> String {
    if let Some((name, _)) = reference.rsplit_once('@') {
        return name.to_owned();
    }
    // Only strip a trailing `:tag` if there is no `/` after it (otherwise
    // we would eat a registry port like `registry.io:5000/foo`).
    if let Some(idx) = reference.rfind(':') {
        let after = &reference[idx + 1..];
        if !after.contains('/') {
            return reference[..idx].to_owned();
        }
    }
    reference.to_owned()
}
