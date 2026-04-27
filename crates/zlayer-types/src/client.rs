//! Client-facing wire types shared between the ZLayer daemon and SDK
//! clients (CLI, `zlayer-docker`, `zlayer-py`, future language SDKs).
//!
//! These were originally defined in `zlayer-client`; they are pure serde
//! DTOs with no transport or async state, so they live here so that
//! crates which want the wire shapes don't have to pull in the full HTTP
//! client.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Session
// ---------------------------------------------------------------------------

/// Persisted CLI session record (lives at `~/.zlayer/session.json`,
/// mode 0600 on Unix).
///
/// Stores the JWT returned by `POST /auth/token` (or `/auth/login`) so
/// subsequent CLI invocations don't need to re-authenticate. The daemon
/// client attaches `Authorization: Bearer <token>` from this record when
/// present.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    /// JWT access token.
    pub token: String,
    /// Email of the authenticated user (for `whoami` display).
    pub email: String,
    /// Token expiry. Used to warn the user before the token actually expires.
    pub expires_at: DateTime<Utc>,
}

impl Session {
    /// Whether the token is past its expiry.
    #[must_use]
    pub fn is_expired(&self) -> bool {
        Utc::now() >= self.expires_at
    }
}

// ---------------------------------------------------------------------------
// Build DTOs (client-side mirrors of zlayer-api build request/response shapes)
// ---------------------------------------------------------------------------

/// Specification sent to the daemon's `POST /api/v1/build/json` endpoint to
/// start an image build against a server-side context path.
///
/// Mirrors `zlayer_api::handlers::build::BuildRequestWithContext` on the
/// wire; the server-side type is `Deserialize`-only so we carry a
/// `Serialize` mirror here.
#[derive(Debug, Clone, Default, Serialize)]
pub struct BuildSpec {
    /// Path to the build context on the daemon host. Must already exist.
    pub context_path: String,
    /// Runtime template name (e.g. `"node20"`) to use instead of a Dockerfile.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime: Option<String>,
    /// Build arguments (Dockerfile `ARG` values).
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub build_args: HashMap<String, String>,
    /// Target stage for multi-stage Dockerfiles.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// Tags to apply to the resulting image.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Disable the buildah layer cache.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub no_cache: bool,
    /// Push the resulting image to its registry after the build succeeds.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub push: bool,
}

/// Handle returned by the daemon `start_build` endpoint. Mirrors the wire
/// shape of `zlayer_api::handlers::build::TriggerBuildResponse` (which is
/// `Serialize`-only, so we carry a `Deserialize` mirror here).
///
/// The `build_id` can be fed to `GET /api/v1/build/{id}`,
/// `GET /api/v1/build/{id}/stream`, and `GET /api/v1/build/{id}/logs`.
#[derive(Debug, Clone, Deserialize)]
pub struct BuildHandle {
    /// Unique build ID for tracking.
    pub build_id: String,
    /// Human-readable message from the daemon.
    pub message: String,
}
