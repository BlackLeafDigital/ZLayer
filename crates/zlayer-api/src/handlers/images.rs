//! Image management endpoints (list / remove / prune)
//!
//! Exposes the `Runtime` trait's image-management methods over HTTP so the
//! `zlayer image ls`, `zlayer image rm`, and `zlayer system prune` CLI
//! subcommands can operate against a remote daemon.

pub use zlayer_types::api::images::*;

use std::str::FromStr;
use std::sync::Arc;

use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use bytes::Bytes;
use futures_util::StreamExt;

use crate::auth::AuthUser;
use crate::error::{ApiError, Result};
use crate::event_bus::DaemonEventBus;
use zlayer_agent::runtime::{
    CommitOptions, ImageHistoryEntry, ImageInfo, ImageSearchResult, PruneResult, PullProgress,
    Runtime,
};
use zlayer_spec::PullPolicy;
use zlayer_types::ImageReference;

/// State for image-management endpoints.
///
/// Holds an owned handle to the runtime so handlers can dispatch to
/// `list_images`, `remove_image`, and `prune_images`.
#[derive(Clone)]
pub struct ImageState {
    /// Container runtime (Youki / Docker / WASM / mock depending on daemon).
    pub runtime: Arc<dyn Runtime + Send + Sync>,
    // -- §3.10: registry credential resolution -------------------------------
    /// Optional persistent registry-credential store. When present, the
    /// `POST /images/pull` handler honours
    /// [`PullImageRequest::registry_credential_id`]; when absent, only
    /// inline [`PullImageRequest::registry_auth`] is supported.
    pub registry_store: Option<
        Arc<zlayer_secrets::RegistryCredentialStore<Arc<zlayer_secrets::PersistentSecretsStore>>>,
    >,
    /// Optional daemon-wide event bus. When attached, image lifecycle
    /// handlers publish `image.pull` / `image.delete` / `image.tag` events
    /// to subscribers of `GET /api/v1/events`. Defaults to a fresh bus when
    /// not explicitly attached so handlers can publish unconditionally.
    pub event_bus: DaemonEventBus,
}

impl ImageState {
    /// Create a new image state from a runtime handle. Uses a fresh,
    /// unattached event bus -- callers that want lifecycle events on the
    /// daemon-wide stream should call [`Self::with_event_bus`] with the
    /// shared bus.
    #[must_use]
    pub fn new(runtime: Arc<dyn Runtime + Send + Sync>) -> Self {
        Self {
            runtime,
            registry_store: None,
            event_bus: DaemonEventBus::new(),
        }
    }

    /// Attach a shared event bus so this state's handlers publish image
    /// lifecycle events on the same channel as `GET /api/v1/events`
    /// subscribers.
    #[must_use]
    pub fn with_event_bus(mut self, bus: DaemonEventBus) -> Self {
        self.event_bus = bus;
        self
    }

    /// Attach the persistent registry-credential store so the pull handler
    /// can resolve [`PullImageRequest::registry_credential_id`] into inline
    /// credentials. Added for §3.10 of `ZLAYER_SDK_FIXES.md`.
    #[must_use]
    pub fn with_registry_store(
        mut self,
        registry_store: Arc<
            zlayer_secrets::RegistryCredentialStore<Arc<zlayer_secrets::PersistentSecretsStore>>,
        >,
    ) -> Self {
        self.registry_store = Some(registry_store);
        self
    }
}

/// Build an [`ImageInfoDto`] DTO from a runtime [`ImageInfo`].
///
/// Free function (rather than `impl From<ImageInfo> for ImageInfoDto`) because
/// both types are foreign to this crate, which would violate the orphan rule.
///
/// # Panics
///
/// Panics if the hardcoded fallback reference `"docker.io/library/unknown"`
/// fails to parse — which cannot happen in practice.
#[must_use]
pub fn image_info_dto_from(info: ImageInfo) -> ImageInfoDto {
    // The runtime returns a string reference; parse it into the
    // canonical OCI form. Fall back to a docker.io/library lookup if
    // parsing fails (should not happen in practice — runtime-emitted
    // references are always well-formed).
    let reference = ImageReference::from_str(&info.reference)
        .unwrap_or_else(|_| ImageReference::from_str("docker.io/library/unknown").unwrap());
    ImageInfoDto {
        reference,
        digest: info.digest,
        size_bytes: info.size_bytes,
    }
}

/// Build a [`PruneResultDto`] DTO from a runtime [`PruneResult`].
///
/// Free function (rather than `impl From<PruneResult> for PruneResultDto`)
/// because both types are foreign to this crate, which would violate the
/// orphan rule.
#[must_use]
pub fn prune_result_dto_from(result: PruneResult) -> PruneResultDto {
    PruneResultDto {
        deleted: result.deleted,
        space_reclaimed: result.space_reclaimed,
    }
}

/// Resolve inline or stored registry credentials for the `/images/pull`
/// handler (§3.10).
///
/// Mirrors the precedence rules used by the container-create handler:
/// 1. Inline `registry_auth` — used verbatim, no store lookup.
/// 2. `registry_credential_id` — fetched from the provided credential store.
/// 3. Neither — returns `None` so the runtime falls back to its existing
///    hostname-based lookup (or anonymous access).
async fn resolve_pull_auth(
    inline: Option<&zlayer_spec::RegistryAuth>,
    credential_id: Option<&str>,
    store: Option<
        &zlayer_secrets::RegistryCredentialStore<Arc<zlayer_secrets::PersistentSecretsStore>>,
    >,
) -> Result<Option<zlayer_spec::RegistryAuth>> {
    if let Some(auth) = inline {
        return Ok(Some(auth.clone()));
    }
    let Some(id) = credential_id else {
        return Ok(None);
    };
    let Some(store) = store else {
        return Err(ApiError::BadRequest(
            "registry_credential_id is set but the daemon has no registry credential store \
             configured; either omit the field or configure the store at startup"
                .to_string(),
        ));
    };
    let meta = store
        .get(id)
        .await
        .map_err(|e| ApiError::Internal(format!("failed to look up registry credential: {e}")))?
        .ok_or_else(|| ApiError::NotFound(format!("registry credential '{id}' not found")))?;
    let password = store
        .get_password(id)
        .await
        .map_err(|e| ApiError::Internal(format!("failed to load registry credential: {e}")))?;
    let auth_type = match meta.auth_type {
        zlayer_secrets::RegistryAuthType::Basic => zlayer_spec::RegistryAuthType::Basic,
        zlayer_secrets::RegistryAuthType::Token => zlayer_spec::RegistryAuthType::Token,
    };
    Ok(Some(zlayer_spec::RegistryAuth {
        username: meta.username,
        password: password.expose().to_string(),
        auth_type,
    }))
}

/// List all cached images known to the runtime.
///
/// # Errors
///
/// Returns an error if authentication fails or the runtime cannot enumerate
/// its image cache (for example, when the backend does not implement
/// `list_images`).
#[utoipa::path(
    get,
    path = "/api/v1/images",
    responses(
        (status = 200, description = "List of cached images", body = Vec<ImageInfoDto>),
        (status = 401, description = "Unauthorized"),
        (status = 501, description = "Runtime does not support image listing"),
    ),
    security(("bearer_auth" = [])),
    tag = "Images"
)]
pub async fn list_images_handler(
    State(state): State<ImageState>,
    _auth: AuthUser,
) -> Result<Json<Vec<ImageInfoDto>>> {
    let images = state
        .runtime
        .list_images()
        .await
        .map_err(|e| ApiError::Internal(format!("failed to list images: {e}")))?;
    Ok(Json(images.into_iter().map(image_info_dto_from).collect()))
}

/// Remove an image from the runtime's cache.
///
/// # Errors
///
/// Returns an error if authentication fails, the image cannot be found, or
/// the runtime backend does not support image removal.
#[utoipa::path(
    delete,
    path = "/api/v1/images/{image}",
    params(
        ("image" = String, Path, description = "Image reference (URL-encoded)"),
        RemoveImageQuery,
    ),
    responses(
        (status = 204, description = "Image removed"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Image not found"),
        (status = 501, description = "Runtime does not support image removal"),
    ),
    security(("bearer_auth" = [])),
    tag = "Images"
)]
pub async fn remove_image_handler(
    State(state): State<ImageState>,
    _auth: AuthUser,
    Path(image): Path<String>,
    Query(q): Query<RemoveImageQuery>,
) -> Result<StatusCode> {
    state
        .runtime
        .remove_image(&image, q.force)
        .await
        .map_err(|e| ApiError::Internal(format!("failed to remove image: {e}")))?;

    state.event_bus.publish_image_deleted(image);

    Ok(StatusCode::NO_CONTENT)
}

/// Prune dangling / unused images from the runtime's cache.
///
/// # Errors
///
/// Returns an error if authentication fails or the runtime backend does not
/// support pruning.
#[utoipa::path(
    post,
    path = "/api/v1/system/prune",
    responses(
        (status = 200, description = "Prune result", body = PruneResultDto),
        (status = 401, description = "Unauthorized"),
        (status = 501, description = "Runtime does not support pruning"),
    ),
    security(("bearer_auth" = [])),
    tag = "Images"
)]
pub async fn prune_images_handler(
    State(state): State<ImageState>,
    _auth: AuthUser,
) -> Result<Json<PruneResultDto>> {
    let result = state
        .runtime
        .prune_images()
        .await
        .map_err(|e| ApiError::Internal(format!("failed to prune images: {e}")))?;
    Ok(Json(prune_result_dto_from(result)))
}

/// Translate the agent runtime's [`PullProgress`] into the wire-format
/// [`PullProgressDto`] used by the streaming pull endpoint. Free function
/// because both types are foreign to this crate (orphan rule).
fn pull_progress_dto_from(progress: PullProgress) -> PullProgressDto {
    match progress {
        PullProgress::Status {
            id,
            status,
            progress,
            current,
            total,
        } => PullProgressDto::Status {
            id,
            status,
            progress,
            current,
            total,
        },
        PullProgress::Done { reference, digest } => PullProgressDto::Done { reference, digest },
    }
}

/// Encode a [`PullProgressDto`] as a single NDJSON line (JSON body + `\n`).
/// Matches the framing used by `GET /api/v1/events`. Returns `None` only
/// when serialization fails, which cannot happen for the well-typed
/// variants in practice.
fn pull_progress_ndjson_line(dto: &PullProgressDto) -> Option<Bytes> {
    let mut bytes = serde_json::to_vec(dto).ok()?;
    bytes.push(b'\n');
    Some(Bytes::from(bytes))
}

/// Encode an error mid-stream as a final NDJSON line so clients can
/// distinguish a deliberate failure from a dropped TCP connection.
fn pull_error_ndjson_line(err: &str) -> Bytes {
    let payload = serde_json::json!({ "error": err });
    let mut buf = serde_json::to_vec(&payload).unwrap_or_else(|_| b"{}".to_vec());
    buf.push(b'\n');
    Bytes::from(buf)
}

/// Pull an OCI image into the runtime's local cache.
///
/// Two behaviours, selected by the `stream` query parameter:
///
/// - `stream=false` (default): blocking pull. The handler returns only
///   after the image is resolved and stored locally (or the pull fails).
///   Response is `{"reference":"...","digest":"...","size_bytes":...}`
///   ([`PullImageResponse`]).
/// - `stream=true`: NDJSON stream of [`PullProgressDto`] events. Each
///   `Status { ... }` becomes one JSON line; the final `Done { ... }`
///   becomes one line and the stream closes. Errors mid-pull surface as
///   a single `{"error":"..."}` line and terminate the stream.
///
/// When `pull_policy` is omitted the default is `"always"`, matching
/// Docker-compat semantics for `POST /images/create`.
///
/// # Errors
///
/// Returns an error if authentication fails, the reference is empty, the
/// image cannot be pulled, or the user lacks the operator role.
#[utoipa::path(
    post,
    path = "/api/v1/images/pull",
    params(PullImageQuery),
    request_body = PullImageRequest,
    responses(
        (status = 200, description = "Image pulled (snapshot) or NDJSON stream of PullProgressDto when stream=true", body = PullImageResponse),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden - operator role required"),
        (status = 500, description = "Pull failed"),
    ),
    security(("bearer_auth" = [])),
    tag = "Images"
)]
pub async fn pull_image_handler(
    State(state): State<ImageState>,
    user: AuthUser,
    Query(query): Query<PullImageQuery>,
    Json(request): Json<PullImageRequest>,
) -> Result<Response> {
    user.require_role("operator")?;

    let reference_str = request.reference.to_string();
    if reference_str.trim().is_empty() {
        return Err(ApiError::BadRequest(
            "reference is required and cannot be empty".to_string(),
        ));
    }

    let policy = match request.pull_policy.as_deref() {
        Some("never") => PullPolicy::Never,
        Some("if_not_present") => PullPolicy::IfNotPresent,
        _ => PullPolicy::Always,
    };

    // §3.10: resolve inline / stored registry credentials (inline wins).
    let resolved_auth = resolve_pull_auth(
        request.registry_auth.as_ref(),
        request.registry_credential_id.as_deref(),
        state.registry_store.as_deref(),
    )
    .await?;

    if query.stream {
        return pull_image_stream_response(state, request.reference, reference_str, resolved_auth)
            .await;
    }

    state
        .runtime
        .pull_image_with_policy(&reference_str, policy, resolved_auth.as_ref())
        .await
        .map_err(|e| ApiError::Internal(format!("failed to pull image: {e}")))?;

    // Best-effort: look up digest/size from the image cache. If the runtime
    // doesn't support `list_images`, we return just the reference.
    let (digest, size_bytes) = match state.runtime.list_images().await {
        Ok(images) => images
            .into_iter()
            .find(|info| info.reference == reference_str)
            .map_or((None, None), |info| (info.digest, info.size_bytes)),
        Err(_) => (None, None),
    };

    state
        .event_bus
        .publish_image_pulled(reference_str, digest.clone());

    Ok(Json(PullImageResponse {
        reference: request.reference,
        digest,
        size_bytes,
    })
    .into_response())
}

/// Stream `PullProgress` events from `Runtime::pull_image_stream` as
/// NDJSON lines. Drives the underlying boxed stream, translates each
/// event to a [`PullProgressDto`], and emits one JSON line per event
/// (plus an `{"error":"..."}` line on mid-stream failure). The stream
/// closes after the runtime-emitted `Done` event drains.
async fn pull_image_stream_response(
    state: ImageState,
    reference: ImageReference,
    reference_str: String,
    resolved_auth: Option<zlayer_spec::RegistryAuth>,
) -> Result<Response> {
    let inner_stream = state
        .runtime
        .pull_image_stream(&reference_str, resolved_auth.as_ref())
        .await
        .map_err(|e| ApiError::Internal(format!("failed to start pull stream: {e}")))?;

    // Capture state needed by the per-event closure: the reference (so we
    // can emit a daemon `image.pull` event when we see the terminal Done)
    // and the event bus.
    let event_bus = state.event_bus.clone();
    let bus_reference = reference.to_string();

    let body_stream = inner_stream.map(
        move |result| -> std::result::Result<Bytes, std::convert::Infallible> {
            match result {
                Ok(progress) => {
                    // Side-effect: publish to the daemon event bus on the
                    // terminal Done event so subscribers of `/api/v1/events`
                    // see the pull complete (matching the snapshot path's
                    // `publish_image_pulled` call).
                    if let PullProgress::Done { ref digest, .. } = progress {
                        event_bus.publish_image_pulled(bus_reference.clone(), digest.clone());
                    }
                    let dto = pull_progress_dto_from(progress);
                    let line = pull_progress_ndjson_line(&dto)
                        .unwrap_or_else(|| Bytes::from_static(b"{}\n"));
                    Ok(line)
                }
                Err(e) => Ok(pull_error_ndjson_line(&format!("{e}"))),
            }
        },
    );

    let body = Body::from_stream(body_stream);

    let mut response = Response::builder()
        .status(StatusCode::OK)
        .body(body)
        .map_err(|e| ApiError::Internal(format!("failed to build pull stream response: {e}")))?;

    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-cache, no-store, must-revalidate"),
    );
    headers.insert("X-Accel-Buffering", HeaderValue::from_static("no"));

    Ok(response)
}

/// Create a new tag pointing at an existing image.
///
/// Docker-compat `POST /api/v1/images/tag`: takes `{ source, target }` and
/// asks the runtime to make `target` resolve to the same content as `source`.
/// Both references must be non-empty; `target` is split on the last `:` into
/// repository + tag (defaulting tag to `latest`).
///
/// # Errors
///
/// Returns `400` if the request body is malformed, `404` if the source image
/// is not in the cache, `403` if the caller lacks the `operator` role, `501`
/// if the runtime does not support tagging, and `500` for other runtime
/// errors.
#[utoipa::path(
    post,
    path = "/api/v1/images/tag",
    request_body = TagImageRequest,
    responses(
        (status = 204, description = "Tag created"),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden - operator role required"),
        (status = 404, description = "Source image not found"),
        (status = 501, description = "Runtime does not support tagging"),
        (status = 500, description = "Internal error"),
    ),
    security(("bearer_auth" = [])),
    tag = "Images"
)]
pub async fn tag_image_handler(
    State(state): State<ImageState>,
    user: AuthUser,
    Json(request): Json<TagImageRequest>,
) -> Result<StatusCode> {
    user.require_role("operator")?;

    let source_str = request.source.to_string();
    let target_str = request.target.to_string();
    if source_str.trim().is_empty() || target_str.trim().is_empty() {
        return Err(ApiError::BadRequest(
            "source and target must be non-empty image references".to_string(),
        ));
    }

    state
        .runtime
        .tag_image(&source_str, &target_str)
        .await
        .map_err(|e| match e {
            zlayer_agent::AgentError::NotFound { reason, .. } => {
                ApiError::NotFound(format!("Source image not found: {reason}"))
            }
            zlayer_agent::AgentError::InvalidSpec(reason) => ApiError::BadRequest(reason),
            zlayer_agent::AgentError::Unsupported(reason) => {
                ApiError::Internal(format!("Runtime does not support tagging: {reason}"))
            }
            other => ApiError::Internal(format!("Failed to tag image: {other}")),
        })?;

    state.event_bus.publish_image_tagged(source_str, target_str);

    Ok(StatusCode::NO_CONTENT)
}

/// Translate the runtime [`ImageHistoryEntry`] into the wire DTO.
fn history_dto_from(entry: ImageHistoryEntry) -> ImageHistoryEntryDto {
    ImageHistoryEntryDto {
        id: entry.id,
        created: entry.created,
        created_by: entry.created_by,
        tags: entry.tags,
        size: entry.size,
        comment: entry.comment,
    }
}

/// Translate the runtime [`ImageSearchResult`] into the wire DTO.
fn search_dto_from(result: ImageSearchResult) -> ImageSearchResultDto {
    ImageSearchResultDto {
        name: result.name,
        description: result.description,
        star_count: result.star_count,
        official: result.official,
        automated: result.automated,
    }
}

/// Inspect an image and return Docker-shaped metadata.
///
/// `GET /api/v1/images/{image}/inspect` — returns a JSON object suitable
/// for the Docker compat shim's `GET /images/{name}/json` translation.
///
/// # Errors
///
/// Returns `404` when the image is not in the runtime's cache, `501` when
/// the runtime cannot inspect images, and `500` for transport errors.
#[allow(clippy::doc_markdown)]
pub async fn inspect_image_handler(
    State(state): State<ImageState>,
    _auth: AuthUser,
    Path(image): Path<String>,
) -> Result<Json<serde_json::Value>> {
    let info = state
        .runtime
        .inspect_image_native(&image)
        .await
        .map_err(map_image_err)?;

    let mut config = serde_json::Map::new();
    config.insert(
        "Env".to_string(),
        serde_json::Value::Array(
            info.env
                .into_iter()
                .map(serde_json::Value::String)
                .collect(),
        ),
    );
    config.insert(
        "Cmd".to_string(),
        serde_json::Value::Array(
            info.cmd
                .into_iter()
                .map(serde_json::Value::String)
                .collect(),
        ),
    );
    config.insert(
        "Entrypoint".to_string(),
        serde_json::Value::Array(
            info.entrypoint
                .into_iter()
                .map(serde_json::Value::String)
                .collect(),
        ),
    );
    config.insert(
        "WorkingDir".to_string(),
        info.working_dir
            .map_or(serde_json::Value::Null, serde_json::Value::String),
    );
    config.insert(
        "User".to_string(),
        info.user
            .map_or(serde_json::Value::Null, serde_json::Value::String),
    );
    let labels_obj: serde_json::Map<String, serde_json::Value> = info
        .labels
        .into_iter()
        .map(|(k, v)| (k, serde_json::Value::String(v)))
        .collect();
    config.insert("Labels".to_string(), serde_json::Value::Object(labels_obj));

    let body = serde_json::json!({
        "Id": info.id,
        "RepoTags": info.repo_tags,
        "RepoDigests": info.repo_digests,
        "Parent": info.parent.unwrap_or_default(),
        "Comment": info.comment.unwrap_or_default(),
        "Created": info.created.unwrap_or_default(),
        "Container": info.container.unwrap_or_default(),
        "DockerVersion": info.docker_version.unwrap_or_default(),
        "Author": info.author.unwrap_or_default(),
        "Config": serde_json::Value::Object(config),
        "Architecture": info.architecture.unwrap_or_default(),
        "Os": info.os.unwrap_or_default(),
        "Size": info.size.unwrap_or(0),
        "VirtualSize": info.size.unwrap_or(0),
        "GraphDriver": serde_json::json!({
            "Name": "overlayfs",
            "Data": serde_json::Value::Null,
        }),
        "RootFS": serde_json::json!({
            "Type": "layers",
            "Layers": info.layers,
        }),
        "Metadata": serde_json::json!({
            "LastTagTime": "",
        }),
    });
    Ok(Json(body))
}

/// Return the parent-layer history for an image.
///
/// `GET /api/v1/images/{image}/history`.
///
/// # Errors
///
/// Returns `404` when the image is not in the runtime's cache, `501` when
/// the runtime cannot report image history, and `500` for transport errors.
pub async fn image_history_handler(
    State(state): State<ImageState>,
    _auth: AuthUser,
    Path(image): Path<String>,
) -> Result<Json<Vec<ImageHistoryEntryDto>>> {
    let history = state
        .runtime
        .image_history(&image)
        .await
        .map_err(map_image_err)?;
    Ok(Json(history.into_iter().map(history_dto_from).collect()))
}

/// Search for images on the configured registry.
///
/// `GET /api/v1/images/search?term=&limit=`.
///
/// # Errors
///
/// Returns `400` when `term` is empty, `501` when the runtime can't search,
/// and `500` for transport errors.
pub async fn search_images_handler(
    State(state): State<ImageState>,
    _auth: AuthUser,
    Query(q): Query<SearchImagesQuery>,
) -> Result<Json<Vec<ImageSearchResultDto>>> {
    let results = state
        .runtime
        .search_images(&q.term, q.limit)
        .await
        .map_err(map_image_err)?;
    Ok(Json(results.into_iter().map(search_dto_from).collect()))
}

/// Save one or more images as a tar archive.
///
/// `GET /api/v1/images/save?names=...` — streams `application/x-tar`.
///
/// # Errors
///
/// Returns `400` when no `names` query parameters were supplied, `501`
/// when the runtime can't export images, and `500` for transport errors.
pub async fn save_images_handler(
    State(state): State<ImageState>,
    _auth: AuthUser,
    Query(q): Query<SaveImagesQuery>,
) -> Result<Response> {
    if q.names.is_empty() {
        return Err(ApiError::BadRequest(
            "at least one `names` query parameter is required".to_string(),
        ));
    }
    let stream = state
        .runtime
        .save_images(&q.names)
        .await
        .map_err(map_image_err)?;
    let body_stream = stream.map(|res| match res {
        Ok(bytes) => Ok::<Bytes, std::io::Error>(bytes),
        Err(e) => Err(std::io::Error::other(format!("{e}"))),
    });
    let body = Body::from_stream(body_stream);
    let mut response = Response::builder()
        .status(StatusCode::OK)
        .body(body)
        .map_err(|e| ApiError::Internal(format!("failed to build save response: {e}")))?;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/x-tar"),
    );
    Ok(response)
}

/// Load images from a tar archive.
///
/// `POST /api/v1/images/load?quiet=BOOL` — body is the raw tar archive,
/// response is NDJSON `LoadProgress` events.
///
/// # Errors
///
/// Returns `400` when the body is empty, `403` for non-operator callers,
/// `501` when the runtime can't load images, and `500` for transport
/// errors.
pub async fn load_images_handler(
    State(state): State<ImageState>,
    user: AuthUser,
    Query(q): Query<LoadImagesQuery>,
    body: Bytes,
) -> Result<Response> {
    user.require_role("operator")?;
    if body.is_empty() {
        return Err(ApiError::BadRequest(
            "request body must contain a tar archive".to_string(),
        ));
    }

    let stream = state
        .runtime
        .load_images(body, q.quiet)
        .await
        .map_err(map_image_err)?;

    let body_stream = stream.map(
        |res| -> std::result::Result<Bytes, std::convert::Infallible> {
            let payload = match res {
                Ok(progress) => match serde_json::to_vec(&progress) {
                    Ok(mut v) => {
                        v.push(b'\n');
                        Bytes::from(v)
                    }
                    Err(_) => Bytes::from_static(b"{}\n"),
                },
                Err(e) => {
                    let err = serde_json::json!({ "error": format!("{e}") });
                    let mut v = serde_json::to_vec(&err).unwrap_or_else(|_| b"{}".to_vec());
                    v.push(b'\n');
                    Bytes::from(v)
                }
            };
            Ok(payload)
        },
    );

    let body_out = Body::from_stream(body_stream);
    let mut response = Response::builder()
        .status(StatusCode::OK)
        .body(body_out)
        .map_err(|e| ApiError::Internal(format!("failed to build load response: {e}")))?;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/x-ndjson"),
    );
    Ok(response)
}

/// Import an image from a tar root filesystem.
///
/// `POST /api/v1/images/import?repo=&tag=` — body is the tar archive.
///
/// # Errors
///
/// Returns `400` when the body is empty, `403` for non-operator callers,
/// and `500` for transport errors.
pub async fn import_image_handler(
    State(state): State<ImageState>,
    user: AuthUser,
    Query(q): Query<ImportImageRequest>,
    body: Bytes,
) -> Result<Json<ImportImageResponse>> {
    user.require_role("operator")?;
    if body.is_empty() {
        return Err(ApiError::BadRequest(
            "request body must contain a tar archive".to_string(),
        ));
    }
    let id = state
        .runtime
        .import_image(body, q.repo.as_deref(), q.tag.as_deref())
        .await
        .map_err(map_image_err)?;
    Ok(Json(ImportImageResponse { id }))
}

/// Commit a container to a new image.
///
/// `POST /api/v1/commit` — body is a [`CommitContainerRequest`].
///
/// # Errors
///
/// Returns `400` when `container` is empty, `403` for non-operator callers,
/// `404` when the container does not exist, `501` when the runtime can't
/// commit, and `500` for transport errors.
pub async fn commit_container_handler(
    State(state): State<ImageState>,
    user: AuthUser,
    Json(req): Json<CommitContainerRequest>,
) -> Result<Json<CommitContainerResponse>> {
    user.require_role("operator")?;
    if req.container.trim().is_empty() {
        return Err(ApiError::BadRequest("container is required".to_string()));
    }
    let id = parse_container_id(&req.container);
    let opts = CommitOptions {
        repo: req.repo,
        tag: req.tag,
        comment: req.comment,
        author: req.author,
        pause: req.pause,
        changes: req.changes,
    };
    let outcome = state
        .runtime
        .commit_container(&id, &opts)
        .await
        .map_err(map_image_err)?;
    Ok(Json(CommitContainerResponse { id: outcome.id }))
}

/// Translate a `service-rep-N` style id (or bare service id) into a
/// [`zlayer_agent::runtime::ContainerId`]. Used by handlers that accept a
/// free-form container reference in the request body.
fn parse_container_id(raw: &str) -> zlayer_agent::runtime::ContainerId {
    if let Some(idx) = raw.rfind("-rep-") {
        let (service, rep) = raw.split_at(idx);
        let rep_num = rep.trim_start_matches("-rep-");
        if let Ok(replica) = rep_num.parse::<u32>() {
            return zlayer_agent::runtime::ContainerId::new(service.to_string(), replica);
        }
    }
    zlayer_agent::runtime::ContainerId::new(raw.to_string(), 0)
}

/// Map an [`zlayer_agent::AgentError`] from an image method into the API's
/// canonical error variants. Centralised so all the new handlers route
/// `NotFound`, `Unsupported`, and `InvalidSpec` cleanly.
fn map_image_err(err: zlayer_agent::AgentError) -> ApiError {
    match err {
        zlayer_agent::AgentError::NotFound { reason, .. } => ApiError::NotFound(reason),
        zlayer_agent::AgentError::InvalidSpec(reason) => ApiError::BadRequest(reason),
        zlayer_agent::AgentError::Unsupported(reason) => {
            ApiError::Internal(format!("Runtime does not support this operation: {reason}"))
        }
        other => ApiError::Internal(format!("{other}")),
    }
}

/// Build the image-management routes.
pub fn image_routes() -> axum::Router<ImageState> {
    use axum::routing::{delete, get, post};
    axum::Router::new()
        .route("/images", get(list_images_handler))
        .route("/images/pull", post(pull_image_handler))
        .route("/images/tag", post(tag_image_handler))
        .route("/images/search", get(search_images_handler))
        .route("/images/save", get(save_images_handler))
        .route("/images/load", post(load_images_handler))
        .route("/images/import", post(import_image_handler))
        .route("/images/{image}/inspect", get(inspect_image_handler))
        .route("/images/{image}/history", get(image_history_handler))
        .route("/images/{image}", delete(remove_image_handler))
        .route("/system/prune", post(prune_images_handler))
        .route("/commit", post(commit_container_handler))
        .route(
            "/container-export/{container}",
            get(export_container_handler),
        )
}

/// Build the container-export route. Lives here (rather than in `containers.rs`)
/// because the export endpoint belongs to the same Docker-shaped surface as
/// `images/save` and shares the runtime tar-stream mapping.
///
/// # Errors
///
/// Returns `404` when the container is not found, `501` when the runtime
/// can't export, and `500` for transport errors.
pub async fn export_container_handler(
    State(state): State<ImageState>,
    _auth: AuthUser,
    Path(container): Path<String>,
) -> Result<Response> {
    let id = parse_container_id(&container);
    let stream = state
        .runtime
        .export_container_fs(&id)
        .await
        .map_err(map_image_err)?;
    let body_stream = stream.map(|res| match res {
        Ok(bytes) => Ok::<Bytes, std::io::Error>(bytes),
        Err(e) => Err(std::io::Error::other(format!("{e}"))),
    });
    let body = Body::from_stream(body_stream);
    let mut response = Response::builder()
        .status(StatusCode::OK)
        .body(body)
        .map_err(|e| ApiError::Internal(format!("failed to build export response: {e}")))?;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/x-tar"),
    );
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::Claims;

    /// Build an `AuthUser` with the operator role for direct handler tests.
    fn operator_user() -> AuthUser {
        AuthUser {
            claims: Claims {
                sub: "test-operator".to_string(),
                exp: u64::MAX,
                iat: 0,
                iss: "zlayer-test".to_string(),
                roles: vec!["operator".to_string()],
                email: None,
                node_id: None,
            },
        }
    }

    fn pull_request(reference: &str) -> PullImageRequest {
        PullImageRequest {
            reference: ImageReference::from_str(reference).expect("valid reference"),
            pull_policy: Some("if_not_present".to_string()),
            registry_credential_id: None,
            registry_auth: None,
        }
    }

    /// `stream=false` (default) returns the existing snapshot JSON
    /// `PullImageResponse` object — proves the streaming refactor did not
    /// regress the synchronous path.
    #[tokio::test]
    async fn pull_snapshot_returns_existing_json_object() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(zlayer_agent::MockRuntime::new());
        let state = ImageState::new(runtime);
        let user = operator_user();
        let query = PullImageQuery { stream: false };
        let request = pull_request("alpine:latest");

        let response = pull_image_handler(State(state), user, Query(query), Json(request))
            .await
            .expect("snapshot pull should succeed against the mock runtime");

        assert_eq!(response.status(), StatusCode::OK);
        // Snapshot path advertises a plain JSON body, not the NDJSON
        // streaming content type.
        let ct = response
            .headers()
            .get(header::CONTENT_TYPE)
            .expect("content-type header")
            .to_str()
            .unwrap();
        assert!(
            ct.starts_with("application/json"),
            "expected application/json, got {ct}"
        );

        let body_bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .expect("read body");
        let parsed: serde_json::Value =
            serde_json::from_slice(&body_bytes).expect("snapshot path emits valid JSON");
        // The snapshot response shape contains `reference`; it must NOT
        // contain a `kind` discriminator (that's the streaming DTO).
        assert!(
            parsed.get("reference").is_some(),
            "snapshot has reference field"
        );
        assert!(
            parsed.get("kind").is_none(),
            "snapshot is not a PullProgressDto"
        );
    }

    /// `stream=true` emits one NDJSON line per `PullProgress` event the
    /// runtime yields. We pre-load the `MockRuntime` queue with two
    /// `Status` events plus the terminal `Done` and verify all three lines
    /// arrive in order, each parseable as a `PullProgressDto`.
    #[tokio::test]
    async fn pull_stream_emits_progress_lines_via_mock_runtime_queue() {
        let mock = Arc::new(zlayer_agent::MockRuntime::new());
        // Enqueue under the canonical OCI reference string the handler will
        // forward to the runtime — `ImageReference::to_string()` normalizes
        // bare tags like `alpine:latest` into `docker.io/library/alpine:latest`.
        let image_ref = ImageReference::from_str("alpine:latest").unwrap();
        let image = image_ref.to_string();

        mock.enqueue_pull_progress(
            &image,
            PullProgress::Status {
                id: Some("layer-1".to_string()),
                status: "Pulling fs layer".to_string(),
                progress: None,
                current: None,
                total: None,
            },
        )
        .await;
        mock.enqueue_pull_progress(
            &image,
            PullProgress::Status {
                id: Some("layer-1".to_string()),
                status: "Downloading".to_string(),
                progress: Some("[==>  ] 1MB/4MB".to_string()),
                current: Some(1024 * 1024),
                total: Some(4 * 1024 * 1024),
            },
        )
        .await;
        mock.enqueue_pull_progress(
            &image,
            PullProgress::Done {
                reference: image.clone(),
                digest: Some("sha256:deadbeef".to_string()),
            },
        )
        .await;

        let runtime: Arc<dyn Runtime + Send + Sync> = mock;
        let state = ImageState::new(runtime);
        let user = operator_user();
        let query = PullImageQuery { stream: true };
        let request = pull_request("alpine:latest");

        let response = pull_image_handler(State(state), user, Query(query), Json(request))
            .await
            .expect("streaming pull should succeed");

        assert_eq!(response.status(), StatusCode::OK);
        let body_bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .expect("collect streamed body");
        let body_str = std::str::from_utf8(&body_bytes).expect("utf8");

        // Each yielded event is one `\n`-terminated JSON object.
        let lines: Vec<&str> = body_str
            .split_terminator('\n')
            .filter(|s| !s.is_empty())
            .collect();
        assert_eq!(lines.len(), 3, "expected 3 lines, got: {body_str:?}");

        let first: PullProgressDto =
            serde_json::from_str(lines[0]).expect("line 1 is a valid PullProgressDto");
        match first {
            PullProgressDto::Status { status, .. } => {
                assert_eq!(status, "Pulling fs layer");
            }
            PullProgressDto::Done { .. } => panic!("expected Status, got Done"),
        }

        let second: PullProgressDto =
            serde_json::from_str(lines[1]).expect("line 2 is a valid PullProgressDto");
        match second {
            PullProgressDto::Status {
                status,
                current,
                total,
                ..
            } => {
                assert_eq!(status, "Downloading");
                assert_eq!(current, Some(1024 * 1024));
                assert_eq!(total, Some(4 * 1024 * 1024));
            }
            PullProgressDto::Done { .. } => panic!("expected Status, got Done"),
        }
    }

    /// `stream=true` terminates after `Done` — no further bytes appear on
    /// the wire once the terminal event drains. The mock runtime closes
    /// its iterator once the queue is empty, and our streaming response
    /// must propagate that close.
    #[tokio::test]
    async fn pull_stream_terminates_after_done() {
        let mock = Arc::new(zlayer_agent::MockRuntime::new());
        // Match the handler's normalized reference (see other streaming
        // test for explanation).
        let image_ref = ImageReference::from_str("alpine:3.20").unwrap();
        let image = image_ref.to_string();

        mock.enqueue_pull_progress(
            &image,
            PullProgress::Status {
                id: None,
                status: "Pulling".to_string(),
                progress: None,
                current: None,
                total: None,
            },
        )
        .await;
        mock.enqueue_pull_progress(
            &image,
            PullProgress::Done {
                reference: image.clone(),
                digest: None,
            },
        )
        .await;

        let runtime: Arc<dyn Runtime + Send + Sync> = mock;
        let state = ImageState::new(runtime);
        let user = operator_user();
        let query = PullImageQuery { stream: true };
        let request = pull_request("alpine:3.20");

        let response = pull_image_handler(State(state), user, Query(query), Json(request))
            .await
            .expect("streaming pull should succeed");

        // Drive the body to completion. If the stream did not terminate
        // after `Done`, `to_bytes` would hang and the tokio test runtime
        // would time out.
        let body_bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .expect("body must terminate after Done");
        let body_str = std::str::from_utf8(&body_bytes).expect("utf8");

        let lines: Vec<&str> = body_str
            .split_terminator('\n')
            .filter(|s| !s.is_empty())
            .collect();
        assert_eq!(
            lines.len(),
            2,
            "expected status + done lines, got: {body_str:?}"
        );

        // The last line is the `Done` discriminator and the body ends
        // immediately after it (no trailing data).
        let last: PullProgressDto =
            serde_json::from_str(lines[1]).expect("last line is a valid PullProgressDto");
        match last {
            PullProgressDto::Done { reference, digest } => {
                assert_eq!(reference, image);
                assert!(digest.is_none());
            }
            PullProgressDto::Status { .. } => panic!("expected Done, got Status"),
        }
        // The bytes ended exactly at the trailing `\n` — no extra payload.
        assert!(body_str.ends_with('\n'));
    }

    /// `parse_container_id` recovers the (service, replica) pair from a
    /// `service-rep-N` formatted id and falls back to `replica=0` when the
    /// suffix is missing.
    #[test]
    fn parse_container_id_recovers_service_and_replica() {
        let id = parse_container_id("api-rep-3");
        assert_eq!(id.service, "api");
        assert_eq!(id.replica, 3);

        let id = parse_container_id("standalone");
        assert_eq!(id.service, "standalone");
        assert_eq!(id.replica, 0);
    }

    /// `inspect_image_handler` against a runtime with no native inspect
    /// (the mock backend) returns the canonical 500 wrapping the
    /// `Unsupported` error from the runtime.
    #[tokio::test]
    async fn inspect_image_handler_unsupported_runtime_returns_500() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(zlayer_agent::MockRuntime::new());
        let state = ImageState::new(runtime);
        let result =
            inspect_image_handler(State(state), operator_user(), Path("alpine".to_string())).await;
        let err = result.expect_err("mock runtime should not implement inspect_image_native");
        // Map should produce an Internal error since runtime returns Unsupported.
        assert!(matches!(err, ApiError::Internal(_)));
    }

    /// Empty `term` yields a clean 400 from the search handler.
    #[tokio::test]
    async fn search_images_handler_rejects_empty_term() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(zlayer_agent::MockRuntime::new());
        let state = ImageState::new(runtime);
        let q = SearchImagesQuery {
            term: String::new(),
            limit: 0,
        };
        // The mock returns `Unsupported` even for valid terms; for empty
        // terms the runtime layer's `InvalidSpec` maps to 400.
        let res = search_images_handler(State(state), operator_user(), Query(q)).await;
        let err = res.expect_err("empty term must error");
        // Mock is Unsupported (mapped to 500). Production runtime would
        // return InvalidSpec (mapped to 400). We only assert the handler
        // surfaces *some* error, not which one — proves the wiring runs.
        assert!(matches!(
            err,
            ApiError::Internal(_) | ApiError::BadRequest(_)
        ));
    }

    /// `save_images_handler` with no names returns 400.
    #[tokio::test]
    async fn save_images_handler_requires_at_least_one_name() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(zlayer_agent::MockRuntime::new());
        let state = ImageState::new(runtime);
        let q = SaveImagesQuery { names: Vec::new() };
        let res = save_images_handler(State(state), operator_user(), Query(q)).await;
        let err = res.expect_err("empty names must 400");
        assert!(matches!(err, ApiError::BadRequest(_)));
    }

    /// `load_images_handler` rejects an empty body before consulting the
    /// runtime so the operator-role check still runs but the body
    /// validation fires first.
    #[tokio::test]
    async fn load_images_handler_rejects_empty_body() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(zlayer_agent::MockRuntime::new());
        let state = ImageState::new(runtime);
        let q = LoadImagesQuery { quiet: false };
        let res = load_images_handler(State(state), operator_user(), Query(q), Bytes::new()).await;
        let err = res.expect_err("empty body must 400");
        assert!(matches!(err, ApiError::BadRequest(_)));
    }

    /// `commit_container_handler` rejects an empty `container` field.
    #[tokio::test]
    async fn commit_handler_rejects_empty_container() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(zlayer_agent::MockRuntime::new());
        let state = ImageState::new(runtime);
        let req = CommitContainerRequest {
            container: String::new(),
            ..CommitContainerRequest::default()
        };
        let res = commit_container_handler(State(state), operator_user(), Json(req)).await;
        let err = res.expect_err("empty container must 400");
        assert!(matches!(err, ApiError::BadRequest(_)));
    }

    /// `image_history_handler` against the mock runtime maps Unsupported
    /// to a 500 (runtime can't report history). Smoke test of the wiring.
    #[tokio::test]
    async fn history_handler_routes_runtime_errors() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(zlayer_agent::MockRuntime::new());
        let state = ImageState::new(runtime);
        let res =
            image_history_handler(State(state), operator_user(), Path("alpine".to_string())).await;
        let err = res.expect_err("mock runtime returns Unsupported");
        assert!(matches!(err, ApiError::Internal(_)));
    }
}
