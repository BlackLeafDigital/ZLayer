//! Docker Engine API image endpoints.
//!
//! Bridges the `/images/*` surface of the Docker Engine API (v1.43) to the
//! running zlayer daemon via [`zlayer_client::DaemonClient`]. Tools that
//! speak Docker — `docker images`, `docker pull`, `docker tag`, `docker rmi`
//! and anything that talks to `/var/run/docker.sock` — can drive zlayer
//! through this router.

use std::collections::HashMap;
use std::convert::Infallible;

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use bytes::Bytes;
use futures_util::stream::{self, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use zlayer_client::PullProgress;
use zlayer_types::spec::{RegistryAuth as SpecRegistryAuth, RegistryAuthType};

use super::auth::{decode_x_registry_auth, RegistryAuth};
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
///
/// Note: `POST /build` lives in [`super::build`] now, alongside the
/// `/build/cancel` and `/build/prune` ancillary endpoints, since the
/// implementation needs the in-process `zlayer-builder` crate.
pub(crate) fn routes(state: SocketState) -> Router {
    Router::new()
        .route("/images/json", get(list_images))
        .route("/images/create", post(pull_image))
        .route("/images/search", get(search_images))
        .route("/images/get", get(save_images))
        .route("/images/load", post(load_images))
        .route("/images/prune", post(prune_images))
        .route("/images/{name}/tag", post(tag_image))
        .route("/images/{name}/history", get(image_history))
        .route("/images/{name}/get", get(save_image_single))
        .route("/images/{name}", delete(remove_image))
        .route("/images/{name}/json", get(inspect_image))
        .route("/images/{name}/push", post(push_image))
        .route("/commit", post(commit_container))
        .route("/containers/{id}/export", get(export_container))
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
            let ref_str = img.reference.to_string();
            let id = img
                .digest
                .clone()
                .unwrap_or_else(|| format!("sha256:{}", hash_ref(&ref_str)));
            let size = i64::try_from(img.size_bytes.unwrap_or(0)).unwrap_or(0);
            // `Reference` is non-empty by construction (parser rejects empty input),
            // so the legacy `is_empty()` guard is dead — always emit the real ref.
            let repo_tags = vec![ref_str.clone()];
            let repo_digests = match &img.digest {
                Some(d) => vec![format!("{}@{}", strip_tag(&ref_str), d)],
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
/// Forwards to `DaemonClient::inspect_image_native`, which returns the
/// already Docker-shaped JSON object. Falls back to the previous
/// `list_images()`-derived synthesis if the inspect endpoint fails (e.g.
/// the underlying runtime is the Mock backend that doesn't implement
/// `inspect_image_native`).
async fn inspect_image(
    State(state): State<SocketState>,
    Path(name): Path<String>,
) -> Result<Json<Value>, ApiError> {
    if let Ok(value) = state.client.inspect_image_native(&name).await {
        return Ok(Json(value));
    }

    let images = state
        .client
        .list_images()
        .await
        .map_err(ApiError::upstream)?;

    let found = images
        .into_iter()
        .find(|img| {
            img.reference.to_string() == name || img.digest.as_deref() == Some(name.as_str())
        })
        .ok_or_else(|| ApiError {
            status: StatusCode::NOT_FOUND,
            message: format!("No such image: {name}"),
        })?;

    let found_ref = found.reference.to_string();
    let id = found
        .digest
        .clone()
        .unwrap_or_else(|| format!("sha256:{}", hash_ref(&found_ref)));
    let size = i64::try_from(found.size_bytes.unwrap_or(0)).unwrap_or(0);
    let created = rfc3339_now();
    // `Reference` is non-empty by construction; legacy `is_empty()` guard is dead.
    let repo_tags = vec![found_ref.clone()];
    let repo_digests = match &found.digest {
        Some(d) => vec![format!("{}@{}", strip_tag(&found_ref), d)],
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
        "Metadata": json!({
            "LastTagTime": "",
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
    /// Docker's legacy `repo` query alias. Some older clients (and a few
    /// libraries) send `repo=<image>` instead of `fromImage=<image>`; we
    /// fall back to it when `fromImage` is absent.
    repo: Option<String>,
    /// Optional platform selector (e.g. `linux/amd64`). Accepted for
    /// compatibility; the daemon currently always pulls the host platform.
    #[allow(dead_code)]
    platform: Option<String>,
    /// Docker also supports `fromSrc` (import from tarball) — not implemented.
    #[serde(rename = "fromSrc")]
    #[allow(dead_code)]
    from_src: Option<String>,
}

/// Resolve `(fromImage, tag, repo)` query parameters into a single image
/// reference string. Public to the module so the unit tests can exercise the
/// parser without spinning up a router.
///
/// Precedence:
/// 1. `fromImage` is preferred when present.
/// 2. `repo` is the legacy alias and used only when `fromImage` is absent.
/// 3. `tag` is appended only when the image string does not already carry a
///    `:tag` or `@digest` suffix (so `fromImage=nginx:1.21&tag=latest` keeps
///    the `1.21` tag baked into `fromImage`).
fn resolve_pull_reference(
    from_image: Option<&str>,
    tag: Option<&str>,
    repo: Option<&str>,
) -> Option<String> {
    let image = from_image
        .filter(|s| !s.is_empty())
        .or_else(|| repo.filter(|s| !s.is_empty()))?;

    let already_qualified = image.contains('@')
        || image
            .rfind(':')
            .is_some_and(|idx| !image[idx + 1..].contains('/'));

    match tag {
        Some(t) if !t.is_empty() && !already_qualified => Some(format!("{image}:{t}")),
        _ => Some(image.to_owned()),
    }
}

/// Convert the decoded Docker `X-Registry-Auth` header into the daemon's
/// inline `RegistryAuth` shape. Returns `None` when no usable credentials
/// are present (header absent, all fields empty, etc.).
///
/// Mapping rules:
/// - When `identity_token` is present, emit a `Token` auth: the token rides
///   in `password`, and `username` falls back to `<token>` (Docker's
///   conventional placeholder) when the client did not supply one.
/// - Otherwise, when both `username` and `password` are present, emit a
///   `Basic` auth with those values verbatim.
/// - The legacy `email` field is dropped — registries do not consume it.
fn to_spec_auth(auth: &RegistryAuth) -> Option<SpecRegistryAuth> {
    if let Some(token) = auth.identity_token.as_deref().filter(|s| !s.is_empty()) {
        let username = auth
            .username
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or("<token>")
            .to_owned();
        return Some(SpecRegistryAuth {
            username,
            password: token.to_owned(),
            auth_type: RegistryAuthType::Token,
        });
    }

    let username = auth.username.as_deref().filter(|s| !s.is_empty())?;
    let password = auth.password.as_deref().filter(|s| !s.is_empty())?;
    Some(SpecRegistryAuth {
        username: username.to_owned(),
        password: password.to_owned(),
        auth_type: RegistryAuthType::Basic,
    })
}

/// Encode one [`PullProgress`] event as one or more Docker NDJSON lines.
///
/// Returns a `Vec<Bytes>` because a single `Done` event expands into two
/// Docker terminal lines (`"Status: Downloaded newer image for ..."` and an
/// optional `"Digest: ..."`). All other `Status` events map to exactly one
/// line. Each returned `Bytes` already carries the trailing `\n`.
fn pull_progress_to_docker_lines(event: &PullProgress) -> Vec<Bytes> {
    match event {
        PullProgress::Status {
            id,
            status,
            progress,
            current,
            total,
        } => {
            let mut obj = serde_json::Map::with_capacity(4);
            obj.insert("status".to_owned(), Value::String(status.clone()));
            if let Some(id) = id {
                obj.insert("id".to_owned(), Value::String(id.clone()));
            } else {
                obj.insert("id".to_owned(), Value::Null);
            }
            if let Some(progress) = progress {
                obj.insert("progress".to_owned(), Value::String(progress.clone()));
            }
            if current.is_some() || total.is_some() {
                let mut detail = serde_json::Map::with_capacity(2);
                detail.insert(
                    "current".to_owned(),
                    Value::Number((*current).unwrap_or(0).into()),
                );
                detail.insert(
                    "total".to_owned(),
                    Value::Number((*total).unwrap_or(0).into()),
                );
                obj.insert("progressDetail".to_owned(), Value::Object(detail));
            }
            vec![ndjson_line(&Value::Object(obj))]
        }
        PullProgress::Done { reference, digest } => {
            let mut lines = Vec::with_capacity(2);
            lines.push(ndjson_line(&json!({
                "status": format!("Status: Downloaded newer image for {reference}"),
                "id": Value::Null,
            })));
            if let Some(digest) = digest.as_deref().filter(|s| !s.is_empty()) {
                lines.push(ndjson_line(&json!({
                    "status": format!("Digest: {digest}"),
                    "id": Value::Null,
                })));
            }
            lines
        }
    }
}

/// Serialize a JSON value into an NDJSON line (`<json>\n`) as `Bytes`.
fn ndjson_line(value: &Value) -> Bytes {
    let mut s = value.to_string();
    s.push('\n');
    Bytes::from(s)
}

/// `POST /images/create` — Pull an image.
///
/// Docker returns `application/json`: one JSON object per `\n`-terminated
/// line describing pull progress. We forward the daemon's
/// [`DaemonClient::stream_image_pull`] stream and reshape each
/// [`PullProgress`] into Docker's wire format:
///
/// - A leading `{"status":"Pulling from <repo>","id":"<tag>"}` line.
/// - One line per `PullProgress::Status`, with `progressDetail.{current,total}`
///   and a `progress` bar string when the daemon reports them.
/// - A trailing `{"status":"Status: Downloaded newer image for <ref>","id":null}`
///   line, plus a `{"status":"Digest: <digest>","id":null}` line when the
///   daemon reports a digest.
async fn pull_image(
    State(state): State<SocketState>,
    Query(q): Query<PullImageQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let reference =
        resolve_pull_reference(q.from_image.as_deref(), q.tag.as_deref(), q.repo.as_deref())
            .ok_or_else(|| ApiError::bad_request("missing fromImage query parameter"))?;

    let auth = decode_x_registry_auth(&headers)
        .map_err(|e| ApiError::bad_request(format!("invalid X-Registry-Auth header: {e}")))?;
    let spec_auth = auth.as_ref().and_then(to_spec_auth);

    let stream = state
        .client
        .stream_image_pull(&reference, spec_auth)
        .await
        .map_err(ApiError::upstream)?;

    // Leading "Pulling from <repo>" line — Docker clients depend on this to
    // print the human-readable banner before per-layer ticks arrive. The id
    // mirrors the requested tag (or "latest" when none was given) so the CLI
    // can render it next to the repo name.
    let header_line = {
        let repo = strip_tag(&reference);
        let tag = extract_tag(&reference).unwrap_or("latest").to_owned();
        ndjson_line(&json!({
            "status": format!("Pulling from {repo}"),
            "id": tag,
        }))
    };

    // Map each PullProgress event into one or more Docker NDJSON lines.
    // Errors from the underlying stream are logged and swallowed so a single
    // transient parse failure doesn't tear the connection down (matches the
    // pattern used by /events).
    let progress_stream = stream.flat_map(|res| match res {
        Ok(event) => stream::iter(pull_progress_to_docker_lines(&event)),
        Err(err) => {
            tracing::warn!(error = %err, "docker /images/create: dropping malformed pull event");
            stream::iter(Vec::new())
        }
    });

    let body_stream = stream::iter(std::iter::once(header_line))
        .chain(progress_stream)
        .map(Ok::<Bytes, Infallible>);

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from_stream(body_stream))
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
                .find(|img| {
                    img.reference.to_string() == name
                        || img.digest.as_deref() == Some(name.as_str())
                })
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
// GET /images/{name}/history
// ---------------------------------------------------------------------------

/// `GET /images/{name}/history` — Return the layer history for an image.
async fn image_history(
    State(state): State<SocketState>,
    Path(name): Path<String>,
) -> Result<Json<Vec<Value>>, ApiError> {
    let history = state
        .client
        .image_history(&name)
        .await
        .map_err(ApiError::upstream)?;

    let body = history
        .into_iter()
        .map(|entry| {
            json!({
                "Id": entry.id,
                "Created": entry.created,
                "CreatedBy": entry.created_by,
                "Tags": entry.tags,
                "Size": entry.size,
                "Comment": entry.comment,
            })
        })
        .collect();
    Ok(Json(body))
}

// ---------------------------------------------------------------------------
// GET /images/search
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct SearchImagesQueryDocker {
    term: Option<String>,
    limit: Option<u32>,
    /// JSON-encoded filters; accepted but ignored by zlayer (which forwards
    /// to the configured registry-side search API verbatim).
    #[allow(dead_code)]
    filters: Option<String>,
}

/// `GET /images/search` — Search the configured registry for images.
async fn search_images(
    State(state): State<SocketState>,
    Query(q): Query<SearchImagesQueryDocker>,
) -> Result<Json<Vec<Value>>, ApiError> {
    let term = q
        .term
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::bad_request("missing required `term` query parameter"))?;
    let limit = q.limit.unwrap_or(0);

    let results = state
        .client
        .search_images(&term, limit)
        .await
        .map_err(ApiError::upstream)?;

    let body = results
        .into_iter()
        .map(|r| {
            json!({
                "name": r.name,
                "description": r.description,
                "star_count": r.star_count,
                "is_official": r.official,
                "is_automated": r.automated,
            })
        })
        .collect();
    Ok(Json(body))
}

// ---------------------------------------------------------------------------
// GET /images/get and GET /images/{name}/get
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct SaveImagesQueryDocker {
    /// `?names=alpine&names=nginx:1` — repeated query parameter for bulk save.
    #[serde(rename = "names")]
    names: Vec<String>,
}

/// `GET /images/get?names=...` — Save one or more images as a tar archive.
async fn save_images(
    State(state): State<SocketState>,
    Query(q): Query<SaveImagesQueryDocker>,
) -> Result<Response, ApiError> {
    if q.names.is_empty() {
        return Err(ApiError::bad_request(
            "at least one `names` query parameter is required",
        ));
    }
    save_images_impl(&state, q.names).await
}

/// `GET /images/{name}/get` — Save a single image as a tar archive.
async fn save_image_single(
    State(state): State<SocketState>,
    Path(name): Path<String>,
) -> Result<Response, ApiError> {
    save_images_impl(&state, vec![name]).await
}

async fn save_images_impl(state: &SocketState, names: Vec<String>) -> Result<Response, ApiError> {
    let stream = state
        .client
        .save_images(&names)
        .await
        .map_err(ApiError::upstream)?;

    let body_stream = stream.map(|res| match res {
        Ok(bytes) => Ok::<Bytes, std::io::Error>(bytes),
        Err(e) => Err(std::io::Error::other(format!("{e}"))),
    });
    let body = Body::from_stream(body_stream);
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/x-tar")
        .body(body)
        .map_err(ApiError::upstream)
}

// ---------------------------------------------------------------------------
// POST /images/load
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct LoadImagesQueryDocker {
    quiet: Option<u8>,
}

/// `POST /images/load` — Load images from a tar archive.
async fn load_images(
    State(state): State<SocketState>,
    Query(q): Query<LoadImagesQueryDocker>,
    body: Bytes,
) -> Result<Response, ApiError> {
    let quiet = matches!(q.quiet, Some(1));
    let resp = state
        .client
        .load_images(body, quiet)
        .await
        .map_err(ApiError::upstream)?;

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/x-ndjson")
        .body(Body::from(resp))
        .map_err(ApiError::upstream)
}

// ---------------------------------------------------------------------------
// POST /images/prune
// ---------------------------------------------------------------------------

/// `POST /images/prune` — Remove dangling images.
async fn prune_images(State(state): State<SocketState>) -> Result<Json<Value>, ApiError> {
    let result = state
        .client
        .prune_images()
        .await
        .map_err(ApiError::upstream)?;

    let images_deleted: Vec<Value> = result
        .deleted
        .into_iter()
        .map(|id| {
            // Docker's prune response distinguishes Untagged from Deleted;
            // the daemon already merges both into `deleted`. Pick a heuristic:
            // if the entry looks like a digest, surface as `Deleted`,
            // otherwise as `Untagged`.
            if id.starts_with("sha256:") {
                json!({ "Deleted": id })
            } else {
                json!({ "Untagged": id })
            }
        })
        .collect();

    Ok(Json(json!({
        "ImagesDeleted": images_deleted,
        "SpaceReclaimed": result.space_reclaimed,
    })))
}

// ---------------------------------------------------------------------------
// POST /commit
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct CommitContainerQueryDocker {
    container: Option<String>,
    repo: Option<String>,
    tag: Option<String>,
    comment: Option<String>,
    author: Option<String>,
    pause: Option<String>,
    changes: Option<String>,
}

/// `POST /commit` — Commit a container to a new image.
async fn commit_container(
    State(state): State<SocketState>,
    Query(q): Query<CommitContainerQueryDocker>,
) -> Result<Json<Value>, ApiError> {
    let container = q
        .container
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ApiError::bad_request("missing `container` query parameter"))?;
    let pause = q
        .pause
        .as_deref()
        .is_none_or(|v| !matches!(v, "0" | "false" | "False" | "no"));

    let req = zlayer_client::CommitContainerRequest {
        container,
        repo: q.repo,
        tag: q.tag,
        comment: q.comment,
        author: q.author,
        pause,
        changes: q.changes,
    };

    let resp = state
        .client
        .commit_container_image(&req)
        .await
        .map_err(ApiError::upstream)?;

    Ok(Json(json!({ "Id": resp.id })))
}

// ---------------------------------------------------------------------------
// GET /containers/{id}/export
// ---------------------------------------------------------------------------

/// `GET /containers/{id}/export` — Stream a container's filesystem as a tar archive.
async fn export_container(
    State(state): State<SocketState>,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let stream = state
        .client
        .export_container(&id)
        .await
        .map_err(ApiError::upstream)?;

    let body_stream = stream.map(|res| match res {
        Ok(bytes) => Ok::<Bytes, std::io::Error>(bytes),
        Err(e) => Err(std::io::Error::other(format!("{e}"))),
    });
    let body = Body::from_stream(body_stream);
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/x-tar")
        .body(body)
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
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0))
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

/// Extract the `:tag` portion of a reference (`nginx:1.21` -> `Some("1.21")`),
/// returning `None` when no tag is present. Mirrors the heuristic in
/// [`strip_tag`] so registry ports (`registry.io:5000/foo`) are not confused
/// for tags. A `@digest` suffix is treated as having no tag.
fn extract_tag(reference: &str) -> Option<&str> {
    if reference.contains('@') {
        return None;
    }
    let idx = reference.rfind(':')?;
    let after = &reference[idx + 1..];
    if after.contains('/') {
        return None;
    }
    Some(after)
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::socket::auth::X_REGISTRY_AUTH_HEADER;
    use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
    use base64::Engine as _;
    use http::{HeaderMap, HeaderValue};

    // ----- query parsing --------------------------------------------------

    #[test]
    fn resolve_pull_reference_combines_from_image_and_tag() {
        // `docker pull nginx:1.21` typically arrives as
        // fromImage=nginx&tag=1.21. The two pieces should be glued back
        // together with a colon.
        let r = resolve_pull_reference(Some("nginx"), Some("1.21"), None);
        assert_eq!(r.as_deref(), Some("nginx:1.21"));
    }

    #[test]
    fn resolve_pull_reference_keeps_existing_tag_in_from_image() {
        // When the client already baked `:tag` into fromImage, we must not
        // double-tag it (the second `:tag` would produce an invalid ref).
        let r = resolve_pull_reference(Some("nginx:1.21"), Some("latest"), None);
        assert_eq!(r.as_deref(), Some("nginx:1.21"));
    }

    #[test]
    fn resolve_pull_reference_falls_back_to_repo_legacy_alias() {
        // Legacy clients send `repo` instead of `fromImage`. The resolver
        // should pick it up when fromImage is absent, then append the tag.
        let r = resolve_pull_reference(None, Some("3.18"), Some("alpine"));
        assert_eq!(r.as_deref(), Some("alpine:3.18"));
    }

    #[test]
    fn resolve_pull_reference_prefers_from_image_over_repo() {
        let r = resolve_pull_reference(Some("nginx"), None, Some("alpine"));
        assert_eq!(r.as_deref(), Some("nginx"));
    }

    #[test]
    fn resolve_pull_reference_preserves_registry_port() {
        // `registry.example.com:5000/app` has a colon but it's a port, not
        // a tag — extract_tag agrees, so we should still append `:1.0`.
        let r = resolve_pull_reference(Some("registry.example.com:5000/app"), Some("1.0"), None);
        assert_eq!(r.as_deref(), Some("registry.example.com:5000/app:1.0"));
    }

    #[test]
    fn resolve_pull_reference_returns_none_when_both_missing() {
        assert_eq!(resolve_pull_reference(None, Some("latest"), None), None);
        assert_eq!(
            resolve_pull_reference(Some(""), Some("latest"), Some("")),
            None
        );
    }

    #[test]
    fn resolve_pull_reference_skips_tag_when_digest_present() {
        let r = resolve_pull_reference(
            Some("nginx@sha256:0000000000000000000000000000000000000000000000000000000000000000"),
            Some("latest"),
            None,
        );
        assert_eq!(
            r.as_deref(),
            Some("nginx@sha256:0000000000000000000000000000000000000000000000000000000000000000")
        );
    }

    // ----- progress mapping ----------------------------------------------

    #[test]
    fn progress_status_with_current_and_total_emits_progress_detail() {
        let event = PullProgress::Status {
            id: Some("abc123".into()),
            status: "Downloading".into(),
            progress: Some("[===>     ] 30%".into()),
            current: Some(300),
            total: Some(1000),
        };

        let lines = pull_progress_to_docker_lines(&event);
        assert_eq!(lines.len(), 1);
        let parsed: Value = serde_json::from_slice(&lines[0]).expect("valid json");

        assert_eq!(parsed["status"], "Downloading");
        assert_eq!(parsed["id"], "abc123");
        assert_eq!(parsed["progress"], "[===>     ] 30%");
        assert_eq!(parsed["progressDetail"]["current"], 300);
        assert_eq!(parsed["progressDetail"]["total"], 1000);
    }

    #[test]
    fn progress_status_without_byte_counts_omits_progress_detail() {
        let event = PullProgress::Status {
            id: None,
            status: "Pulling fs layer".into(),
            progress: None,
            current: None,
            total: None,
        };

        let lines = pull_progress_to_docker_lines(&event);
        assert_eq!(lines.len(), 1);
        let parsed: Value = serde_json::from_slice(&lines[0]).expect("valid json");

        assert_eq!(parsed["status"], "Pulling fs layer");
        // Docker uses `null` for the id when none is reported.
        assert!(parsed["id"].is_null());
        // No byte counts -> no progressDetail key.
        assert!(parsed.get("progressDetail").is_none());
        assert!(parsed.get("progress").is_none());
    }

    #[test]
    fn progress_done_emits_two_terminal_lines_when_digest_present() {
        let event = PullProgress::Done {
            reference: "nginx:1.21".into(),
            digest: Some("sha256:deadbeef".into()),
        };

        let lines = pull_progress_to_docker_lines(&event);
        assert_eq!(lines.len(), 2);

        let first: Value = serde_json::from_slice(&lines[0]).expect("valid json");
        let second: Value = serde_json::from_slice(&lines[1]).expect("valid json");

        assert_eq!(
            first["status"],
            "Status: Downloaded newer image for nginx:1.21"
        );
        assert!(first["id"].is_null());

        assert_eq!(second["status"], "Digest: sha256:deadbeef");
        assert!(second["id"].is_null());
    }

    #[test]
    fn progress_done_without_digest_emits_only_download_line() {
        let event = PullProgress::Done {
            reference: "nginx:1.21".into(),
            digest: None,
        };

        let lines = pull_progress_to_docker_lines(&event);
        assert_eq!(lines.len(), 1);
        let parsed: Value = serde_json::from_slice(&lines[0]).expect("valid json");
        assert_eq!(
            parsed["status"],
            "Status: Downloaded newer image for nginx:1.21"
        );
    }

    // ----- ndjson line shape ---------------------------------------------

    #[test]
    fn ndjson_line_terminates_with_newline() {
        let line = ndjson_line(&json!({"a": 1}));
        assert_eq!(line.last(), Some(&b'\n'));
        // The content before the newline must be valid JSON.
        let trimmed = &line[..line.len() - 1];
        let parsed: Value = serde_json::from_slice(trimmed).expect("valid json");
        assert_eq!(parsed["a"], 1);
    }

    // ----- X-Registry-Auth decode + forward ------------------------------

    fn header_with_auth(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            X_REGISTRY_AUTH_HEADER,
            HeaderValue::from_str(value).unwrap(),
        );
        headers
    }

    #[test]
    fn x_registry_auth_basic_round_trips_to_spec_auth() {
        let json = r#"{
            "username": "alice",
            "password": "hunter2",
            "email": "alice@example.com",
            "serveraddress": "registry-1.docker.io"
        }"#;
        let encoded = BASE64_STANDARD.encode(json.as_bytes());
        let headers = header_with_auth(&encoded);

        // The handler decodes via decode_x_registry_auth then converts via
        // to_spec_auth — exercise both steps end-to-end.
        let decoded = decode_x_registry_auth(&headers)
            .expect("decode ok")
            .unwrap();
        let spec = to_spec_auth(&decoded).expect("auth converted");

        assert_eq!(spec.username, "alice");
        assert_eq!(spec.password, "hunter2");
        assert_eq!(spec.auth_type, RegistryAuthType::Basic);
    }

    #[test]
    fn x_registry_auth_identity_token_maps_to_token_auth() {
        let json = r#"{"identitytoken":"oauth-tok","serveraddress":"ghcr.io"}"#;
        let encoded = BASE64_STANDARD.encode(json.as_bytes());
        let headers = header_with_auth(&encoded);

        let decoded = decode_x_registry_auth(&headers)
            .expect("decode ok")
            .unwrap();
        let spec = to_spec_auth(&decoded).expect("auth converted");

        // No client-supplied username -> Docker convention placeholder.
        assert_eq!(spec.username, "<token>");
        assert_eq!(spec.password, "oauth-tok");
        assert_eq!(spec.auth_type, RegistryAuthType::Token);
    }

    #[test]
    fn x_registry_auth_identity_token_keeps_supplied_username() {
        let json = r#"{"username":"oauth2accesstoken","identitytoken":"tok"}"#;
        let encoded = BASE64_STANDARD.encode(json.as_bytes());
        let headers = header_with_auth(&encoded);

        let decoded = decode_x_registry_auth(&headers)
            .expect("decode ok")
            .unwrap();
        let spec = to_spec_auth(&decoded).expect("auth converted");

        assert_eq!(spec.username, "oauth2accesstoken");
        assert_eq!(spec.password, "tok");
        assert_eq!(spec.auth_type, RegistryAuthType::Token);
    }

    #[test]
    fn x_registry_auth_empty_object_yields_none() {
        // {} decodes successfully but contains no usable credentials — the
        // handler must forward `None` to the daemon, not an empty Basic auth.
        let encoded = BASE64_STANDARD.encode(b"{}");
        let headers = header_with_auth(&encoded);

        let decoded = decode_x_registry_auth(&headers)
            .expect("decode ok")
            .unwrap();
        assert!(to_spec_auth(&decoded).is_none());
    }

    #[test]
    fn missing_header_yields_none_auth() {
        let headers = HeaderMap::new();
        let decoded = decode_x_registry_auth(&headers).expect("decode ok");
        assert!(decoded.is_none());
    }

    // ----- extract_tag ----------------------------------------------------

    #[test]
    fn extract_tag_handles_registry_port() {
        assert_eq!(extract_tag("registry.io:5000/foo"), None);
        assert_eq!(extract_tag("registry.io:5000/foo:v1"), Some("v1"));
        assert_eq!(extract_tag("nginx:1.21"), Some("1.21"));
        assert_eq!(extract_tag("nginx"), None);
        assert_eq!(extract_tag("nginx@sha256:abc"), None);
    }
}
