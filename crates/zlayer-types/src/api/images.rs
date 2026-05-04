//! Image management API DTOs.
//!
//! Wire-format types shared between the daemon's `/api/v1/images` and
//! `/api/v1/system/prune` endpoints and SDK clients. Moved out of
//! `zlayer-api` so SDK crates can depend on them without pulling in the
//! full server stack.

use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

/// Serializable wrapper for `zlayer_agent::runtime::ImageInfo` so we can
/// attach `ToSchema` here (the underlying type in `zlayer-agent` can't
/// depend on `utoipa`).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ImageInfoDto {
    /// Canonical image reference (e.g. `zachhandley/zlayer-manager:latest`).
    #[schema(value_type = String)]
    #[serde(with = "crate::image_ref_serde")]
    pub reference: crate::ImageReference,
    /// Content-addressed digest (`sha256:...`) if known.
    pub digest: Option<String>,
    /// Size in bytes if known.
    pub size_bytes: Option<u64>,
}

/// Serializable wrapper for `zlayer_agent::runtime::PruneResult`.
#[derive(Debug, Clone, Serialize, Deserialize, Default, ToSchema)]
pub struct PruneResultDto {
    /// Image references or digests that were removed.
    pub deleted: Vec<String>,
    /// Bytes reclaimed from the cache.
    pub space_reclaimed: u64,
}

/// Request body for the pull-image handler. Blocking pull of an OCI image.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct PullImageRequest {
    /// OCI image reference to pull, e.g. `docker.io/library/nginx:latest`.
    #[schema(value_type = String)]
    #[serde(with = "crate::image_ref_serde")]
    pub reference: crate::ImageReference,
    /// Pull policy override. Accepts `"always"`, `"if_not_present"`, or
    /// `"never"`. Defaults to `"always"` when omitted.
    #[serde(default)]
    pub pull_policy: Option<String>,
    // -- §3.10: registry auth -----------------------------------------------
    /// Id of a persisted registry credential (from
    /// `POST /api/v1/credentials/registry`) to use for this pull. Ignored
    /// when [`Self::registry_auth`] is also supplied (inline auth wins).
    /// Requires the daemon to be configured with a credential store.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry_credential_id: Option<String>,
    /// Inline Docker/OCI registry credentials used for this pull only. Not
    /// persisted, never logged, never echoed back on a response. Takes
    /// precedence over `registry_credential_id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry_auth: Option<crate::spec::RegistryAuth>,
}

/// Response body for the pull-image handler. Reports the pulled reference
/// and, when the backend exposes it via `list_images`, the resolved digest
/// and on-disk size.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct PullImageResponse {
    /// Canonical reference that was pulled.
    #[schema(value_type = String)]
    #[serde(with = "crate::image_ref_serde")]
    pub reference: crate::ImageReference,
    /// Content-addressed digest (`sha256:...`) if the runtime reports one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
    /// On-disk size in bytes if the runtime reports one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
}

/// Query parameters for the remove-image handler.
#[derive(Debug, Deserialize, IntoParams)]
pub struct RemoveImageQuery {
    /// Force removal even if the image is referenced by containers.
    #[serde(default)]
    pub force: bool,
}

/// Query parameters for the pull-image handler.
///
/// When `stream=true`, the handler returns a Newline-Delimited-JSON (NDJSON)
/// stream of [`PullProgressDto`] events instead of a single
/// [`PullImageResponse`] JSON object. Each event is one JSON object on its
/// own line, terminated with `\n`. The stream closes after a single
/// `done` event (or an error).
#[derive(Debug, Default, Deserialize, IntoParams)]
pub struct PullImageQuery {
    /// When `true`, stream NDJSON `PullProgressDto` events instead of a
    /// single JSON response. Defaults to `false` (snapshot pull).
    #[serde(default)]
    pub stream: bool,
}

/// One progress event emitted by the streaming pull endpoint.
///
/// Wire-format mirror of `zlayer_agent::runtime::PullProgress`. Lives in
/// `zlayer-types` so SDK clients can deserialize the streamed events
/// without depending on the server-side `zlayer-agent` crate.
///
/// Tagged on `kind` so the JSON shape is self-describing on the wire:
/// `{"kind":"status",...}` for in-flight progress and
/// `{"kind":"done",...}` for the terminal completion event.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PullProgressDto {
    /// Progress update for an in-flight layer or stage.
    Status {
        /// Layer ID or other backend-specific identifier, when available.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        /// Human-readable status text, e.g. `"Pulling fs layer"`,
        /// `"Downloading"`, `"Extracting"`. Always present; may be empty
        /// when the backend has nothing to report this tick.
        status: String,
        /// Pre-formatted progress bar string, when the backend reports one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        progress: Option<String>,
        /// Bytes transferred so far for this layer, when reported.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        current: Option<u64>,
        /// Expected total bytes for this layer, when reported.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        total: Option<u64>,
    },
    /// Pull completed successfully. Terminal event; the stream closes
    /// after this is emitted.
    Done {
        /// Resolved canonical image reference.
        reference: String,
        /// Content-addressed digest, when the backend reports one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        digest: Option<String>,
    },
}

/// Request body for the tag-image handler. Matches Docker-compat
/// `docker tag` semantics: create a new reference (`target`) pointing at an
/// already-cached image (`source`).
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct TagImageRequest {
    /// Existing image reference to tag (e.g. `myapp:latest`).
    #[schema(value_type = String)]
    #[serde(with = "crate::image_ref_serde")]
    pub source: crate::ImageReference,
    /// New reference to create (e.g. `registry.example.com/myapp:v1`).
    #[schema(value_type = String)]
    #[serde(with = "crate::image_ref_serde")]
    pub target: crate::ImageReference,
}

/// One row of an image's history. Mirror of
/// `zlayer_agent::runtime::ImageHistoryEntry`. Lives here so SDK clients
/// can deserialize without depending on the server-side agent crate.
#[derive(Debug, Clone, Serialize, Deserialize, Default, ToSchema)]
pub struct ImageHistoryEntryDto {
    /// Layer / image id (`sha256:...`). May be `<missing>` for layers that
    /// were dropped during a squash.
    pub id: String,
    /// Unix-seconds timestamp when this layer was created.
    pub created: i64,
    /// Dockerfile-style instruction that produced this layer.
    pub created_by: String,
    /// Tags that point at this specific layer.
    pub tags: Vec<String>,
    /// Layer size in bytes.
    pub size: u64,
    /// Optional comment recorded with the layer.
    pub comment: String,
}

/// One result returned by the search-images endpoint.
#[derive(Debug, Clone, Serialize, Deserialize, Default, ToSchema)]
pub struct ImageSearchResultDto {
    /// Image name (e.g. `library/nginx`).
    pub name: String,
    /// Free-text description.
    pub description: String,
    /// Number of stars on the source registry, when reported.
    pub star_count: u64,
    /// Whether the image is officially curated.
    pub official: bool,
    /// Whether the image was produced by an automated build (deprecated).
    pub automated: bool,
}

/// Query parameters for the search-images handler.
#[derive(Debug, Default, Deserialize, IntoParams)]
pub struct SearchImagesQuery {
    /// Search term — image name or substring.
    pub term: String,
    /// Maximum number of results to return. `0` means "let the registry
    /// decide".
    #[serde(default)]
    pub limit: u32,
}

/// Body for the commit-container endpoint.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
pub struct CommitContainerRequest {
    /// Container id or name to commit.
    pub container: String,
    /// Repository name to apply (e.g. `myapp`). Optional.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// Tag to apply (defaults to `latest` when `repo` is set).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    /// Free-form comment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// Author.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// Whether to pause the container before committing (default `true`).
    #[serde(default = "default_true")]
    pub pause: bool,
    /// Dockerfile-style instructions to apply during commit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changes: Option<String>,
}

const fn default_true() -> bool {
    true
}

/// Response body for the commit-container endpoint.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
pub struct CommitContainerResponse {
    /// Content-addressed image id of the new image.
    pub id: String,
}

/// Body for the import-image endpoint.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
pub struct ImportImageRequest {
    /// Repository name to apply (optional).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// Tag to apply (optional, defaults to `latest` when `repo` is set).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
}

/// Response body for the import-image endpoint.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
pub struct ImportImageResponse {
    /// Content-addressed image id of the imported image.
    pub id: String,
}

/// Query parameters for the load-images endpoint.
#[derive(Debug, Default, Deserialize, IntoParams)]
pub struct LoadImagesQuery {
    /// Suppress per-line progress output.
    #[serde(default)]
    pub quiet: bool,
}

/// Query parameters for the save-images endpoint. Names are repeatable —
/// `?names=alpine&names=nginx:1.21` produces a multi-image archive.
#[derive(Debug, Default, Deserialize, IntoParams)]
pub struct SaveImagesQuery {
    /// Image names to include in the tar archive.
    #[serde(default)]
    pub names: Vec<String>,
}
