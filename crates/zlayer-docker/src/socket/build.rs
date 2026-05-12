//! Docker Engine API `/build` endpoint.
//!
//! Implements `POST /build` end-to-end against the in-process
//! [`zlayer_builder::ImageBuilder`]: the request body is a tar archive of
//! the build context (optionally gzip-compressed), the response is an
//! NDJSON stream of progress events that mirrors what the upstream Docker
//! daemon emits, including the terminal `{"aux":{"ID":"sha256:..."}}`
//! line on success and `{"errorDetail":{...},"error":"..."}` on failure.
//!
//! Also provides the two ancillary build endpoints `docker buildx prune`
//! and the legacy build-cancel call rely on:
//!
//! - `POST /build/cancel?id=<build-id>`
//! - `POST /build/prune?keep-storage=<bytes>&all=<bool>&filters=<json>`
//!
//! The cancel endpoint returns 200 unconditionally — there is no
//! daemon-side build registry to cancel against today, but Docker
//! clients still issue the call, so an acknowledged 200 keeps them
//! happy. The prune endpoint reports an empty cache (`BuildKit`-style
//! incremental cache is not implemented yet) so clients see "nothing
//! to prune" rather than a hard error.

use std::collections::HashMap;
use std::convert::Infallible;
use std::path::PathBuf;

use axum::body::Body;
use axum::extract::{Query, RawQuery, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;

#[cfg(feature = "build")]
use zlayer_builder::{BuildEvent, ImageBuilder, PullBaseMode};

use super::auth::decode_x_registry_config;
use super::SocketState;
use zlayer_paths::ZLayerDirs;

/// Build API routes.
pub(crate) fn routes(state: SocketState) -> Router {
    Router::new()
        .route("/build", post(build_image))
        .route("/build/cancel", post(cancel_build))
        .route("/build/prune", post(prune_build_cache))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Query parsing
// ---------------------------------------------------------------------------

/// Parsed `POST /build` query parameters.
///
/// Mirrors the union of fields the Docker Engine API v1.43 documents for
/// `/build`. Fields the daemon does not yet honour are still parsed (and
/// stored) so callers see consistent validation behaviour across upgrades.
#[derive(Debug, Default, Clone, PartialEq)]
#[allow(clippy::struct_excessive_bools)]
pub(crate) struct BuildQuery {
    /// Path inside the tar context to the Dockerfile (default: `Dockerfile`).
    pub dockerfile: Option<String>,
    /// `t` may repeat — one entry per tag.
    pub tags: Vec<String>,
    /// Extra entries to inject into `/etc/hosts` inside the build container.
    pub extra_hosts: Option<String>,
    /// Remote URL build alternative to a tarball body.
    pub remote: Option<String>,
    /// `q` flag — when true, the daemon suppresses verbose progress.
    pub quiet: bool,
    /// `nocache` flag.
    pub no_cache: bool,
    /// JSON list of cache image references the build can pull layers from.
    pub cache_from: Vec<String>,
    /// Pull base images (any non-empty value enables pulling).
    pub pull: bool,
    /// Remove intermediate containers on success (Docker default: true).
    pub rm: bool,
    /// Always remove intermediate containers, even on failure.
    pub force_rm: bool,
    /// Memory limit in bytes for the build container.
    pub memory: Option<u64>,
    /// Total memory + swap limit.
    pub memswap: Option<i64>,
    /// CPU shares (relative weight).
    pub cpu_shares: Option<u64>,
    /// Comma-separated list of allowed CPUs (cgroup `cpuset.cpus`).
    pub cpu_set_cpus: Option<String>,
    /// CPU CFS period.
    pub cpu_period: Option<u64>,
    /// CPU CFS quota.
    pub cpu_quota: Option<u64>,
    /// Build-time variables (JSON map).
    pub build_args: HashMap<String, String>,
    /// `/dev/shm` size in bytes.
    pub shm_size: Option<u64>,
    /// Squash the resulting image (`BuildKit` experimental flag).
    pub squash: bool,
    /// Image labels (JSON map).
    pub labels: HashMap<String, String>,
    /// Network mode for the build container (`default`, `host`, `none`).
    pub network_mode: Option<String>,
    /// Target platform (e.g. `linux/amd64`).
    pub platform: Option<String>,
    /// Multi-stage target stage name.
    pub target: Option<String>,
    /// `BuildKit`-style outputs descriptor.
    pub outputs: Option<String>,
    /// Builder version (`1` for legacy, `2` for `BuildKit`).
    pub version: Option<String>,
}

/// Parse the raw query string into a [`BuildQuery`].
///
/// Public to the module so unit tests can assert the per-field decoding
/// behaviour without spinning up a router.
///
/// The parser is intentionally lenient: any unknown key is silently
/// dropped, malformed JSON values fall back to a default, and integer
/// fields that fail to parse are treated as absent. This matches the
/// Docker daemon's "ignore what you cannot use" stance and avoids wedging
/// the entire build over a single malformed query parameter.
pub(crate) fn parse_build_query(raw: Option<&str>) -> BuildQuery {
    let mut out = BuildQuery {
        // Docker's default is rm=true (remove intermediate containers on
        // success). The CLI clears it only when the caller passes --rm=false.
        rm: true,
        ..BuildQuery::default()
    };

    let Some(qs) = raw else {
        return out;
    };

    for (key, value) in url::form_urlencoded::parse(qs.as_bytes()) {
        let key = key.as_ref();
        let value = value.into_owned();
        match key {
            "dockerfile" => out.dockerfile = Some(value),
            "t" if !value.is_empty() => out.tags.push(value),
            "extrahosts" => out.extra_hosts = Some(value),
            "remote" => out.remote = Some(value),
            "q" => out.quiet = parse_bool(&value),
            "nocache" => out.no_cache = parse_bool(&value),
            "cachefrom" => {
                // Docker accepts a JSON array of strings here. Treat anything
                // that doesn't parse as JSON as a single entry so older
                // clients that pass a bare image reference still work.
                if let Ok(list) = serde_json::from_str::<Vec<String>>(&value) {
                    out.cache_from = list;
                } else if !value.is_empty() {
                    out.cache_from = vec![value];
                }
            }
            "pull" => out.pull = parse_bool(&value),
            "rm" => out.rm = parse_bool(&value),
            "forcerm" => out.force_rm = parse_bool(&value),
            "memory" => out.memory = value.parse().ok(),
            "memswap" => out.memswap = value.parse().ok(),
            "cpushares" => out.cpu_shares = value.parse().ok(),
            "cpusetcpus" => out.cpu_set_cpus = Some(value),
            "cpuperiod" => out.cpu_period = value.parse().ok(),
            "cpuquota" => out.cpu_quota = value.parse().ok(),
            "buildargs" => {
                if let Ok(map) = serde_json::from_str::<HashMap<String, String>>(&value) {
                    out.build_args = map;
                }
            }
            "shmsize" => out.shm_size = value.parse().ok(),
            "squash" => out.squash = parse_bool(&value),
            "labels" => {
                if let Ok(map) = serde_json::from_str::<HashMap<String, String>>(&value) {
                    out.labels = map;
                }
            }
            "networkmode" => out.network_mode = Some(value),
            "platform" => out.platform = Some(value),
            "target" => out.target = Some(value),
            "outputs" => out.outputs = Some(value),
            "version" => out.version = Some(value),
            _ => {}
        }
    }

    out
}

/// Loose boolean parser matching Docker's "any non-empty truthy string is
/// true" convention. Empty string and `0` / `false` / `no` map to false;
/// everything else is true.
fn parse_bool(value: &str) -> bool {
    !matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "" | "0" | "false" | "no" | "off"
    )
}

// ---------------------------------------------------------------------------
// NDJSON helpers
// ---------------------------------------------------------------------------

/// Serialize a JSON value into an NDJSON line (`<json>\n`) as `Bytes`.
fn ndjson_line(value: &Value) -> Bytes {
    let mut s = value.to_string();
    s.push('\n');
    Bytes::from(s)
}

/// `{"stream":"<line>\n"}` — a single line of build output as Docker
/// expects to see it on the wire.
pub(crate) fn stream_event(line: &str) -> Bytes {
    let mut payload = line.to_owned();
    if !payload.ends_with('\n') {
        payload.push('\n');
    }
    ndjson_line(&json!({ "stream": payload }))
}

/// `{"aux":{"ID":"sha256:<digest>"}}` — terminal success line. Docker
/// clients use this to extract the built image ID for tagging.
pub(crate) fn aux_image_id(image_id: &str) -> Bytes {
    ndjson_line(&json!({
        "aux": { "ID": image_id }
    }))
}

/// `{"errorDetail":{"message":"..."},"error":"..."}` — terminal failure line.
pub(crate) fn error_event(message: &str) -> Bytes {
    ndjson_line(&json!({
        "errorDetail": { "message": message },
        "error": message,
    }))
}

#[cfg(feature = "build")]
fn build_event_to_ndjson(event: &BuildEvent) -> Option<Bytes> {
    match event {
        BuildEvent::StageStarted {
            index,
            name,
            base_image,
        } => Some(stream_event(&format!(
            "Step {idx} : FROM {img}{nm}",
            idx = index + 1,
            img = base_image,
            nm = name
                .as_deref()
                .map(|n| format!(" AS {n}"))
                .unwrap_or_default(),
        ))),
        BuildEvent::InstructionStarted {
            stage,
            index,
            instruction,
        } => Some(stream_event(&format!(
            " ---> Step {stage}.{idx}: {instruction}",
            stage = stage + 1,
            idx = index + 1,
        ))),
        BuildEvent::Output { line, .. } => Some(stream_event(line)),
        BuildEvent::InstructionComplete { cached: true, .. } => {
            Some(stream_event(" ---> Using cache"))
        }
        BuildEvent::StageComplete { index } => {
            Some(stream_event(&format!(" ---> Stage {} complete", index + 1)))
        }
        // BuildStarted, InstructionComplete{cached:false}, BuildComplete, and
        // BuildFailed are intentionally silent: BuildStarted is metadata only,
        // a non-cached InstructionComplete is implied by Output lines, and the
        // terminal aux/errorDetail lines are emitted by the caller.
        BuildEvent::BuildStarted { .. }
        | BuildEvent::InstructionComplete { cached: false, .. }
        | BuildEvent::BuildComplete { .. }
        | BuildEvent::BuildFailed { .. } => None,
    }
}

// ---------------------------------------------------------------------------
// Tar context extraction
// ---------------------------------------------------------------------------

/// Decode the `body` (raw tar or gzip+tar) into `target_dir`.
///
/// Docker clients send the build context as either:
/// - `Content-Type: application/x-tar` (raw POSIX tar), or
/// - the same with `Content-Encoding: gzip` (gzip-wrapped).
///
/// We sniff the gzip magic bytes (`0x1F 0x8B`) instead of trusting the
/// `Content-Encoding` header so a missing/incorrect header does not break
/// the build. The extraction itself runs on `spawn_blocking` because
/// `tar::Archive::unpack` is synchronous.
async fn extract_build_context(body: Bytes, target_dir: PathBuf) -> Result<(), BuildHandlerError> {
    tokio::task::spawn_blocking(move || {
        let cursor = std::io::Cursor::new(body.as_ref().to_vec());
        let is_gzip = body.len() >= 2 && body[0] == 0x1F && body[1] == 0x8B;
        if is_gzip {
            let gz = flate2::read::GzDecoder::new(cursor);
            let mut archive = tar::Archive::new(gz);
            archive
                .unpack(&target_dir)
                .map_err(BuildHandlerError::TarExtract)
        } else {
            let mut archive = tar::Archive::new(cursor);
            archive
                .unpack(&target_dir)
                .map_err(BuildHandlerError::TarExtract)
        }
    })
    .await
    .map_err(|e| BuildHandlerError::Internal(format!("join error during tar extraction: {e}")))?
}

/// Local error type for the `/build` handler. Kept private — callers see
/// these as Docker-shaped error responses.
enum BuildHandlerError {
    BadRequest(String),
    Internal(String),
    TarExtract(std::io::Error),
}

impl IntoResponse for BuildHandlerError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::BadRequest(m) => (StatusCode::BAD_REQUEST, m),
            Self::Internal(m) => (StatusCode::INTERNAL_SERVER_ERROR, m),
            Self::TarExtract(e) => (
                StatusCode::BAD_REQUEST,
                format!("failed to extract build context tar: {e}"),
            ),
        };
        (status, Json(json!({ "message": message }))).into_response()
    }
}

// ---------------------------------------------------------------------------
// Handler: POST /build
// ---------------------------------------------------------------------------

/// `POST /build` — build an image from a tar context.
///
/// Returns a streaming `application/json` body containing one NDJSON event
/// per progress update. Docker clients parse this stream and surface the
/// `stream` / `aux` / `errorDetail` lines to the user.
#[allow(unused_variables)]
async fn build_image(
    State(_state): State<SocketState>,
    RawQuery(raw_query): RawQuery,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, BuildHandlerError> {
    let query = parse_build_query(raw_query.as_deref());

    // Decode (but don't yet forward) the per-registry credentials map so we
    // surface a clean 400 to the client rather than a mid-stream failure.
    let _registry_config = decode_x_registry_config(&headers)
        .map_err(|e| BuildHandlerError::BadRequest(format!("invalid X-Registry-Config: {e}")))?;

    if body.is_empty() && query.remote.is_none() {
        return Err(BuildHandlerError::BadRequest(
            "build request body is empty (expected tar archive of build context)".into(),
        ));
    }

    // Materialise the build context to a temp directory. The directory is
    // owned by the build task so it lives until the build finishes; on
    // failure we still tear it down via TempDir's Drop impl.
    let temp_dir = ZLayerDirs::system_default()
        .scratch_dir("build-image-")
        .map_err(|e| BuildHandlerError::Internal(format!("failed to create temp dir: {e}")))?;
    let context_path = temp_dir.path().to_path_buf();

    if !body.is_empty() {
        extract_build_context(body, context_path.clone()).await?;
    }

    #[cfg(feature = "build")]
    {
        Ok(spawn_streaming_build(query, temp_dir).into_response())
    }

    #[cfg(not(feature = "build"))]
    {
        let _ = (query, temp_dir);
        let body = error_event(
            "image build is unavailable: zlayer-docker was compiled without the `build` feature",
        );
        Ok(Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body))
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response()))
    }
}

/// Core build pipeline: spawn a tokio task that runs `ImageBuilder`,
/// forwards builder events as NDJSON, and emits the terminal aux/error
/// line when the build resolves.
#[cfg(feature = "build")]
fn spawn_streaming_build(query: BuildQuery, temp_dir: zlayer_types::Scratch) -> Response {
    let (tx, rx) = mpsc::unbounded_channel::<Result<Bytes, Infallible>>();

    tokio::spawn(async move {
        let context_path = temp_dir.path().to_path_buf();
        let tx_for_run = tx.clone();
        let result = run_image_build(query, context_path, tx_for_run).await;

        // Emit the terminal line — Docker clients treat the stream as
        // open-ended until they see either an `aux` (success) or `error`
        // (failure) line.
        let terminal = match result {
            Ok(image_id) => aux_image_id(&image_id),
            Err(err) => error_event(&err),
        };
        let _ = tx.send(Ok(terminal));

        // Hold the temp_dir until the build finishes so buildah still has
        // the context on disk; Drop here cleans it up.
        drop(temp_dir);
    });

    let stream = UnboundedReceiverStream::new(rx);
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from_stream(stream))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// Drive a single [`ImageBuilder`] run, forwarding builder events to
/// `tx` as NDJSON `stream` lines. Returns the final image ID on success
/// or a human-readable error string on failure.
#[cfg(feature = "build")]
async fn run_image_build(
    query: BuildQuery,
    context_path: PathBuf,
    tx: mpsc::UnboundedSender<Result<Bytes, Infallible>>,
) -> Result<String, String> {
    // Bridge std::sync::mpsc (what ImageBuilder::with_events expects) to the
    // tokio mpsc that feeds the response stream.
    let (builder_tx, builder_rx) = std::sync::mpsc::channel::<BuildEvent>();
    let tx_for_forwarder = tx.clone();
    let forwarder = tokio::task::spawn_blocking(move || {
        while let Ok(event) = builder_rx.recv() {
            if let Some(line) = build_event_to_ndjson(&event) {
                if tx_for_forwarder.send(Ok(line)).is_err() {
                    // Client hung up — stop draining; the receiver will be
                    // dropped along with the channel when the build ends.
                    break;
                }
            }
        }
    });

    let mut builder = ImageBuilder::new(&context_path)
        .await
        .map_err(|e| e.to_string())?;

    if let Some(df) = &query.dockerfile {
        // Dockerfile path is relative to the context directory.
        builder = builder.dockerfile(context_path.join(df));
    }
    for tag in &query.tags {
        builder = builder.tag(tag.clone());
    }
    if !query.build_args.is_empty() {
        builder = builder.build_args(query.build_args.clone());
    }
    if let Some(target) = &query.target {
        builder = builder.target(target.clone());
    }
    if query.no_cache {
        builder = builder.no_cache();
    }
    for cf in &query.cache_from {
        builder = builder.cache_from(cf.clone());
    }
    if let Some(platform) = &query.platform {
        builder = builder.platform(platform.clone());
    }
    if query.pull {
        builder = builder.pull(PullBaseMode::Always);
    }

    builder = builder.with_events(builder_tx);

    let result = builder.build().await;

    // Drain the forwarder before we report the terminal line so we don't
    // race the success/error event ahead of the builder's last `Output`.
    let _ = forwarder.await;

    match result {
        Ok(built) => Ok(built.image_id),
        Err(e) => Err(e.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Handler: POST /build/cancel
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct CancelQuery {
    /// Build ID Docker assigns when calling `/build` with `BuildKit`. We
    /// accept it for compatibility but do not track build IDs yet.
    #[allow(dead_code)]
    id: Option<String>,
}

/// `POST /build/cancel` — request a build cancellation.
///
/// Returns 200 unconditionally. Real cancellation requires a daemon-side
/// build registry mapping `id -> JoinHandle`; the daemon does not maintain
/// that today, but Docker clients still issue this call as part of normal
/// shutdown, so we acknowledge rather than 501.
async fn cancel_build(
    State(_state): State<SocketState>,
    Query(_q): Query<CancelQuery>,
) -> StatusCode {
    StatusCode::OK
}

// ---------------------------------------------------------------------------
// Handler: POST /build/prune
// ---------------------------------------------------------------------------

/// `POST /build/prune` response shape per Docker Engine API v1.43.
#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct PruneResponse {
    /// IDs of the cache entries that were deleted (empty: no cache).
    pub(crate) caches_deleted: Vec<String>,
    /// Total bytes reclaimed by the prune (`0` when nothing was deleted).
    pub(crate) space_reclaimed: u64,
}

/// `POST /build/prune` — prune `BuildKit`'s cache.
///
/// Returns the documented `{"CachesDeleted":[],"SpaceReclaimed":0}`
/// response. We have no incremental layer cache to prune today, so this
/// is always a no-op. Docker's CLI treats an empty `CachesDeleted` array
/// as "nothing to prune" and reports it as success.
async fn prune_build_cache(
    State(_state): State<SocketState>,
    RawQuery(_raw): RawQuery,
) -> Json<PruneResponse> {
    Json(PruneResponse {
        caches_deleted: Vec::new(),
        space_reclaimed: 0,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::socket::auth::X_REGISTRY_CONFIG_HEADER;
    use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
    use base64::Engine as _;
    use http::HeaderValue;

    // ----- query parsing --------------------------------------------------

    #[test]
    fn parse_build_query_extracts_repeated_tags() {
        // Docker passes one `t=` per tag. The parser must accumulate them
        // rather than overwrite, and the order must match the wire order
        // because the first tag is treated as the "primary" by some clients.
        let q = parse_build_query(Some("t=app:latest&t=app:v1&t=app:v2"));
        assert_eq!(q.tags, vec!["app:latest", "app:v1", "app:v2"]);
    }

    #[test]
    fn parse_build_query_decodes_full_parameter_set() {
        let qs = "dockerfile=Dockerfile.prod&t=foo:bar&nocache=1&pull=true&\
                  rm=false&forcerm=1&q=1&memory=536870912&memswap=-1&\
                  cpushares=512&cpusetcpus=0-3&cpuperiod=100000&cpuquota=50000&\
                  shmsize=67108864&squash=1&networkmode=host&platform=linux%2Famd64&\
                  target=runtime&outputs=type%3Dimage&version=2&\
                  buildargs=%7B%22A%22%3A%221%22%2C%22B%22%3A%222%22%7D&\
                  labels=%7B%22env%22%3A%22prod%22%7D&\
                  cachefrom=%5B%22registry%2Fapp%3Acache%22%5D";
        let q = parse_build_query(Some(qs));

        assert_eq!(q.dockerfile.as_deref(), Some("Dockerfile.prod"));
        assert_eq!(q.tags, vec!["foo:bar"]);
        assert!(q.no_cache);
        assert!(q.pull);
        assert!(!q.rm); // explicitly cleared via rm=false
        assert!(q.force_rm);
        assert!(q.quiet);
        assert_eq!(q.memory, Some(536_870_912));
        assert_eq!(q.memswap, Some(-1));
        assert_eq!(q.cpu_shares, Some(512));
        assert_eq!(q.cpu_set_cpus.as_deref(), Some("0-3"));
        assert_eq!(q.cpu_period, Some(100_000));
        assert_eq!(q.cpu_quota, Some(50_000));
        assert_eq!(q.shm_size, Some(67_108_864));
        assert!(q.squash);
        assert_eq!(q.network_mode.as_deref(), Some("host"));
        assert_eq!(q.platform.as_deref(), Some("linux/amd64"));
        assert_eq!(q.target.as_deref(), Some("runtime"));
        assert_eq!(q.outputs.as_deref(), Some("type=image"));
        assert_eq!(q.version.as_deref(), Some("2"));
        assert_eq!(q.build_args.get("A").map(String::as_str), Some("1"));
        assert_eq!(q.build_args.get("B").map(String::as_str), Some("2"));
        assert_eq!(q.labels.get("env").map(String::as_str), Some("prod"));
        assert_eq!(q.cache_from, vec!["registry/app:cache"]);
    }

    #[test]
    fn parse_build_query_defaults_rm_true_when_omitted() {
        // Docker's default behaviour is rm=true; our parser must reflect
        // that so a bare `POST /build` does not unintentionally keep
        // intermediate containers around.
        let q = parse_build_query(None);
        assert!(q.rm);
        assert!(!q.no_cache);
        assert!(!q.pull);
    }

    #[test]
    fn parse_build_query_falls_back_to_single_cachefrom_when_not_json() {
        // Older Docker clients send `cachefrom=image:tag` (a bare ref)
        // instead of the JSON-list format. Treat that as a one-element list.
        let q = parse_build_query(Some("cachefrom=registry%2Fapp%3Acache"));
        assert_eq!(q.cache_from, vec!["registry/app:cache"]);
    }

    #[test]
    fn parse_build_query_skips_empty_tag_values() {
        // `t=&t=foo:1` should yield only the non-empty tag — empty tag
        // entries would otherwise propagate into ImageBuilder and trigger
        // a build error mid-stream.
        let q = parse_build_query(Some("t=&t=foo%3A1"));
        assert_eq!(q.tags, vec!["foo:1"]);
    }

    #[test]
    fn parse_build_query_ignores_unknown_keys() {
        // The Docker API is a moving target — newer clients can send keys
        // we have not yet wired up. Confirm those don't poison the parse.
        let q = parse_build_query(Some("t=foo&futureflag=1&ulimits=%5B%5D"));
        assert_eq!(q.tags, vec!["foo"]);
    }

    #[test]
    fn parse_bool_matches_docker_truthiness_table() {
        assert!(!parse_bool(""));
        assert!(!parse_bool("0"));
        assert!(!parse_bool("false"));
        assert!(!parse_bool("FALSE"));
        assert!(!parse_bool("no"));
        assert!(!parse_bool("off"));
        assert!(parse_bool("1"));
        assert!(parse_bool("true"));
        assert!(parse_bool("yes"));
        assert!(parse_bool("anything-else"));
    }

    // ----- X-Registry-Config decoder --------------------------------------

    #[test]
    fn x_registry_config_happy_path_decodes_inline() {
        // Confirms the build handler can decode the multi-registry header
        // shape it receives — we exercise the `auth::decode_x_registry_config`
        // public API the same way build_image does.
        let json = r#"{
            "registry-1.docker.io": {
                "username": "alice",
                "password": "hunter2",
                "serveraddress": "registry-1.docker.io"
            }
        }"#;
        let encoded = BASE64_STANDARD.encode(json.as_bytes());
        let mut headers = HeaderMap::new();
        headers.insert(
            X_REGISTRY_CONFIG_HEADER,
            HeaderValue::from_str(&encoded).unwrap(),
        );

        let map = decode_x_registry_config(&headers).expect("decode ok");
        assert_eq!(map.len(), 1);
        let entry = map.get("registry-1.docker.io").expect("entry");
        assert_eq!(entry.username.as_deref(), Some("alice"));
        assert_eq!(entry.password.as_deref(), Some("hunter2"));
    }

    #[test]
    fn x_registry_config_missing_decodes_to_empty_map() {
        // Most builds go without per-registry creds; a missing header must
        // yield an empty map (not an error) so the build proceeds anonymously.
        let headers = HeaderMap::new();
        let map = decode_x_registry_config(&headers).expect("decode ok");
        assert!(map.is_empty());
    }

    // ----- NDJSON shape ---------------------------------------------------

    #[test]
    fn stream_event_emits_stream_object_with_trailing_newline() {
        let line = stream_event("Step 1/3 : FROM alpine:3.18");
        let raw = std::str::from_utf8(&line).expect("utf8");
        assert!(raw.ends_with('\n'));
        let parsed: Value = serde_json::from_str(raw.trim_end()).expect("valid json");
        assert_eq!(parsed["stream"], "Step 1/3 : FROM alpine:3.18\n");
    }

    #[test]
    fn aux_event_carries_image_id_in_pascal_case_id_field() {
        // Docker clients pull the ID off `aux.ID` (capitalised, per Docker's
        // wire conventions). Anything else would silently break tag lookups
        // on the client side.
        let line = aux_image_id("sha256:deadbeef");
        let parsed: Value = serde_json::from_slice(&line[..line.len() - 1]).expect("valid json");
        assert_eq!(parsed["aux"]["ID"], "sha256:deadbeef");
    }

    #[test]
    fn error_event_emits_both_error_and_error_detail() {
        // Docker clients prefer `errorDetail.message` for structured
        // surfaces but fall back to the flat `error` string. Emit both so
        // every client path renders the failure correctly.
        let line = error_event("dockerfile parse failed");
        let parsed: Value = serde_json::from_slice(&line[..line.len() - 1]).expect("valid json");
        assert_eq!(parsed["error"], "dockerfile parse failed");
        assert_eq!(parsed["errorDetail"]["message"], "dockerfile parse failed");
    }

    // ----- prune / cancel response shapes ---------------------------------

    #[test]
    fn prune_response_serializes_with_pascal_case_keys() {
        // Docker spec mandates `CachesDeleted` and `SpaceReclaimed` (Pascal-
        // case). A camelCase regression here breaks `docker buildx prune`
        // output formatting silently.
        let resp = PruneResponse {
            caches_deleted: Vec::new(),
            space_reclaimed: 0,
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert!(json.get("CachesDeleted").is_some());
        assert!(json.get("SpaceReclaimed").is_some());
        assert_eq!(json["CachesDeleted"], json!([]));
        assert_eq!(json["SpaceReclaimed"], 0);
    }

    #[test]
    fn cancel_query_round_trips_via_serde_json() {
        // The handler returns 200 regardless of whether the client supplies
        // an id (no daemon-side build registry exists today). We exercise
        // the deserializer's Default contract via a synthetic JSON object
        // because the State-bound handler can't be unit-tested without a
        // real DaemonClient and we still want to lock in the query shape.
        let parsed: CancelQuery =
            serde_json::from_value(json!({"id": "abc"})).expect("deserializes");
        assert_eq!(parsed.id.as_deref(), Some("abc"));

        let empty: CancelQuery = serde_json::from_value(json!({})).expect("default ok");
        assert!(empty.id.is_none());
    }
}
