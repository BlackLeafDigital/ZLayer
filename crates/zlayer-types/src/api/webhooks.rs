//! Webhook receiver / management API DTOs.
//!
//! Wire types for the public webhook receiver
//! (`POST /webhooks/{provider}/{project_id}`) and the auth-required webhook
//! management endpoints (`GET /api/v1/projects/{id}/webhook` and
//! `POST /api/v1/projects/{id}/webhook/rotate`).

use serde::{Deserialize, Serialize};

/// Response for the public webhook receiver.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct WebhookResponse {
    /// Operation result -- always `"ok"` on success.
    pub status: String,
    /// HEAD commit SHA after the pull.
    pub sha: String,
}

/// Response for `GET /api/v1/projects/{id}/webhook`.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct WebhookInfoResponse {
    /// Full URL to configure in the git host's webhook settings.
    pub url: String,
    /// The HMAC secret to paste into the git host.
    pub secret: String,
}

/// Path parameters for the public webhook receiver.
#[derive(Debug, Deserialize)]
pub struct WebhookPath {
    pub provider: String,
    pub project_id: String,
}
