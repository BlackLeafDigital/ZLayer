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
