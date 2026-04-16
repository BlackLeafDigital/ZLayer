//! Webhook endpoints for git-push-triggered project operations.
//!
//! Two groups of routes live here:
//!
//! 1. **Public webhook receiver** (`POST /webhooks/{provider}/{project_id}`)
//!    -- unauthenticated, HMAC-verified against a per-project webhook secret
//!    stored in [`SecretsStore`].  On success it triggers the same clone /
//!    fast-forward-pull logic used by `POST /api/v1/projects/{id}/pull`.
//!
//! 2. **Webhook management** (`GET /api/v1/projects/{id}/webhook` and
//!    `POST /api/v1/projects/{id}/webhook/rotate`) -- auth-required, returns
//!    or rotates the webhook secret + URL.
//!
//! Supported providers and their signature formats:
//! - **github**:  `X-Hub-Signature-256: sha256=<HMAC-SHA256(secret, body)>`
//! - **gitea**:   `X-Gitea-Signature: <HMAC-SHA256(secret, body)>`
//! - **forgejo**: `X-Forgejo-Signature: <HMAC-SHA256(secret, body)>`
//! - **gitlab**:  `X-Gitlab-Token: <secret>` (constant-time equality check)

use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    body::Bytes,
    extract::{Path, State},
    http::HeaderMap,
    Json,
};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use subtle::ConstantTimeEq;
use utoipa::ToSchema;

use crate::error::{ApiError, Result};
use crate::handlers::users::AuthActor;
use crate::storage::ProjectStorage;
use zlayer_secrets::{Secret, SecretsStore};

// ---------------------------------------------------------------------------
// Scope / key helpers
// ---------------------------------------------------------------------------

/// The secrets-store scope under which all webhook secrets are kept.
const WEBHOOK_SCOPE: &str = "webhook_secrets";

/// Generate a random 32-byte hex string (64 hex chars).
fn generate_webhook_secret() -> String {
    use rand::Rng;
    let bytes: [u8; 32] = rand::rng().random();
    hex::encode(bytes)
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// Shared state for webhook endpoints.
///
/// The `secrets_store` provides read/write access to per-project webhook
/// secrets (scope = `"webhook_secrets"`, key = project id).
#[derive(Clone)]
pub struct WebhookState {
    /// Project storage backend.
    pub project_store: Arc<dyn ProjectStorage>,
    /// Read-write secrets store for webhook secrets.
    pub secrets_store: Arc<dyn SecretsStore + Send + Sync>,
    /// Optional git credential store for resolving `git_credential_id`.
    pub git_creds: Option<
        Arc<zlayer_secrets::GitCredentialStore<Arc<zlayer_secrets::PersistentSecretsStore>>>,
    >,
    /// Root directory for cloned project working copies.
    pub clone_root: PathBuf,
}

// ---------------------------------------------------------------------------
// Request / response types
// ---------------------------------------------------------------------------

/// Response for the public webhook receiver.
#[derive(Debug, Serialize, ToSchema)]
pub struct WebhookResponse {
    /// Operation result -- always `"ok"` on success.
    pub status: String,
    /// HEAD commit SHA after the pull.
    pub sha: String,
}

/// Response for `GET /api/v1/projects/{id}/webhook`.
#[derive(Debug, Serialize, ToSchema)]
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

// ---------------------------------------------------------------------------
// HMAC / signature verification
// ---------------------------------------------------------------------------

/// Verify an HMAC-SHA256 signature.
///
/// `signature_hex` is the raw hex digest (no prefix).
fn verify_hmac_sha256(secret: &[u8], body: &[u8], signature_hex: &str) -> bool {
    let Ok(expected_bytes) = hex::decode(signature_hex) else {
        return false;
    };
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret) else {
        return false;
    };
    mac.update(body);
    let computed = mac.finalize().into_bytes();
    computed.as_slice().ct_eq(&expected_bytes).into()
}

/// Extract and verify the provider-specific signature from the request
/// headers.
///
/// Returns `Ok(())` on success or an appropriate [`ApiError`] on failure.
fn verify_signature(provider: &str, headers: &HeaderMap, secret: &str, body: &[u8]) -> Result<()> {
    match provider {
        "github" => {
            let sig = headers
                .get("X-Hub-Signature-256")
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| {
                    ApiError::Forbidden("Missing X-Hub-Signature-256 header".to_string())
                })?;
            let hex_part = sig.strip_prefix("sha256=").ok_or_else(|| {
                ApiError::Forbidden("X-Hub-Signature-256 must start with sha256=".to_string())
            })?;
            if !verify_hmac_sha256(secret.as_bytes(), body, hex_part) {
                return Err(ApiError::Forbidden("HMAC signature mismatch".to_string()));
            }
            Ok(())
        }
        "gitea" => {
            let sig = headers
                .get("X-Gitea-Signature")
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| {
                    ApiError::Forbidden("Missing X-Gitea-Signature header".to_string())
                })?;
            if !verify_hmac_sha256(secret.as_bytes(), body, sig) {
                return Err(ApiError::Forbidden("HMAC signature mismatch".to_string()));
            }
            Ok(())
        }
        "forgejo" => {
            let sig = headers
                .get("X-Forgejo-Signature")
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| {
                    ApiError::Forbidden("Missing X-Forgejo-Signature header".to_string())
                })?;
            if !verify_hmac_sha256(secret.as_bytes(), body, sig) {
                return Err(ApiError::Forbidden("HMAC signature mismatch".to_string()));
            }
            Ok(())
        }
        "gitlab" => {
            let token = headers
                .get("X-Gitlab-Token")
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| ApiError::Forbidden("Missing X-Gitlab-Token header".to_string()))?;
            // Constant-time comparison of the token against the stored secret.
            let eq: bool = token.as_bytes().ct_eq(secret.as_bytes()).into();
            if !eq {
                return Err(ApiError::Forbidden("Token mismatch".to_string()));
            }
            Ok(())
        }
        _ => Err(ApiError::BadRequest(format!(
            "Unsupported webhook provider: {provider}"
        ))),
    }
}

// ---------------------------------------------------------------------------
// Internal pull helper (reused by both pull_project and webhook receiver)
// ---------------------------------------------------------------------------

/// Clone or fast-forward pull a project and return `(sha, path)`.
///
/// This is the shared implementation used by both the authenticated
/// `POST /api/v1/projects/{id}/pull` endpoint and the public webhook
/// receiver.
async fn do_pull(state: &WebhookState, project_id: &str) -> Result<(String, String)> {
    let project = state
        .project_store
        .get(project_id)
        .await
        .map_err(|e| ApiError::Internal(format!("Project store: {e}")))?
        .ok_or_else(|| ApiError::NotFound(format!("Project {project_id} not found")))?;

    let git_url = project
        .git_url
        .as_deref()
        .ok_or_else(|| ApiError::BadRequest("Project has no git_url configured".to_string()))?;
    let branch = project.git_branch.as_deref().unwrap_or("main");

    let auth = match (&project.git_credential_id, &state.git_creds) {
        (Some(cred_id), Some(git_store)) => {
            let cred = git_store
                .get(cred_id)
                .await
                .map_err(|e| ApiError::Internal(format!("Credential lookup: {e}")))?
                .ok_or_else(|| {
                    ApiError::BadRequest(format!("Git credential {cred_id} not found"))
                })?;
            let value = git_store
                .get_value(cred_id)
                .await
                .map_err(|e| ApiError::Internal(format!("Credential value: {e}")))?;
            match cred.kind {
                zlayer_secrets::GitCredentialKind::Pat => {
                    zlayer_git::GitAuth::pat(&cred.name, value.expose())
                }
                zlayer_secrets::GitCredentialKind::SshKey => {
                    zlayer_git::GitAuth::ssh_key(value.expose())
                }
            }
        }
        _ => zlayer_git::GitAuth::anonymous(),
    };

    let dest = state.clone_root.join(&project.id);
    let sha = if dest.exists() {
        zlayer_git::pull_ff(&dest, branch, &auth)
            .await
            .map_err(|e| ApiError::Internal(format!("Git pull failed: {e}")))?
    } else {
        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| ApiError::Internal(format!("Failed to create clone root: {e}")))?;
        }
        zlayer_git::clone_repo(git_url, branch, &auth, &dest)
            .await
            .map_err(|e| ApiError::Internal(format!("Git clone failed: {e}")))?
    };

    Ok((sha, dest.to_string_lossy().to_string()))
}

// ---------------------------------------------------------------------------
// Retrieve or generate webhook secret for a project
// ---------------------------------------------------------------------------

/// Get the webhook secret for a project, generating one if it does not
/// exist yet.
async fn get_or_create_secret(
    secrets_store: &(dyn SecretsStore + Send + Sync),
    project_id: &str,
) -> Result<String> {
    // Try to read first.
    if let Ok(existing) = secrets_store.get_secret(WEBHOOK_SCOPE, project_id).await {
        return Ok(existing.expose().to_string());
    }

    // Not found -- generate and persist.
    let new_secret = generate_webhook_secret();
    secrets_store
        .set_secret(WEBHOOK_SCOPE, project_id, &Secret::new(&new_secret))
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to store webhook secret: {e}")))?;
    Ok(new_secret)
}

// ---------------------------------------------------------------------------
// Endpoints
// ---------------------------------------------------------------------------

/// Receive a webhook push event and trigger a project pull.
///
/// This endpoint is **unauthenticated** -- it relies on HMAC signature
/// verification against the per-project webhook secret.
///
/// Supported providers: `github`, `gitea`, `forgejo`, `gitlab`.
///
/// # Errors
///
/// Returns [`ApiError::NotFound`] when no webhook secret is configured,
/// [`ApiError::Forbidden`] when signature verification fails,
/// [`ApiError::BadRequest`] for unsupported providers or missing git URL,
/// or [`ApiError::Internal`] on git operation failures.
#[utoipa::path(
    post,
    path = "/webhooks/{provider}/{project_id}",
    params(
        ("provider" = String, Path, description = "Git host provider (github, gitea, forgejo, gitlab)"),
        ("project_id" = String, Path, description = "Project id"),
    ),
    request_body(content = String, description = "Raw webhook payload from the git host", content_type = "application/json"),
    responses(
        (status = 200, description = "Pull triggered", body = WebhookResponse),
        (status = 400, description = "Bad request (unsupported provider or project has no git URL)"),
        (status = 403, description = "Signature verification failed"),
        (status = 404, description = "Project not found or no webhook secret configured"),
        (status = 500, description = "Pull failed"),
    ),
    tag = "Webhooks"
)]
pub async fn receive_webhook(
    State(state): State<WebhookState>,
    Path(WebhookPath {
        provider,
        project_id,
    }): Path<WebhookPath>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<WebhookResponse>> {
    // Look up the stored webhook secret. If there is none, the webhook
    // was never configured -- reject with 404.
    let secret = state
        .secrets_store
        .get_secret(WEBHOOK_SCOPE, &project_id)
        .await
        .map_err(|_| {
            ApiError::NotFound(format!(
                "No webhook secret configured for project {project_id}"
            ))
        })?;

    // Verify the signature / token.
    verify_signature(&provider, &headers, secret.expose(), &body)?;

    // Trigger the pull.
    let (sha, _path) = do_pull(&state, &project_id).await?;

    Ok(Json(WebhookResponse {
        status: "ok".to_string(),
        sha,
    }))
}

/// Get webhook configuration for a project.
///
/// Returns the webhook URL pattern and the HMAC secret. Generates the
/// secret on first call if it does not exist.
///
/// Auth required (any authenticated user).
///
/// # Errors
///
/// Returns [`ApiError::NotFound`] when the project does not exist,
/// or [`ApiError::Internal`] on store failures.
#[utoipa::path(
    get,
    path = "/api/v1/projects/{id}/webhook",
    params(("id" = String, Path, description = "Project id")),
    responses(
        (status = 200, description = "Webhook configuration", body = WebhookInfoResponse),
        (status = 404, description = "Project not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "Webhooks"
)]
pub async fn get_webhook_info(
    _actor: AuthActor,
    State(state): State<WebhookState>,
    Path(id): Path<String>,
) -> Result<Json<WebhookInfoResponse>> {
    // Verify the project exists.
    state
        .project_store
        .get(&id)
        .await
        .map_err(|e| ApiError::Internal(format!("Project store: {e}")))?
        .ok_or_else(|| ApiError::NotFound(format!("Project {id} not found")))?;

    let secret = get_or_create_secret(secrets_store_ref(&state), &id).await?;

    // Build a template URL. The caller replaces `{provider}` with their
    // git host of choice.
    let url = format!("/webhooks/{{provider}}/{id}");

    Ok(Json(WebhookInfoResponse { url, secret }))
}

/// Rotate (regenerate) the webhook secret for a project.
///
/// Admin only.
///
/// # Errors
///
/// Returns [`ApiError::Forbidden`] for non-admins,
/// [`ApiError::NotFound`] when the project does not exist,
/// or [`ApiError::Internal`] on store failures.
#[utoipa::path(
    post,
    path = "/api/v1/projects/{id}/webhook/rotate",
    params(("id" = String, Path, description = "Project id")),
    responses(
        (status = 200, description = "New webhook configuration", body = WebhookInfoResponse),
        (status = 403, description = "Admin role required"),
        (status = 404, description = "Project not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "Webhooks"
)]
pub async fn rotate_webhook_secret(
    actor: AuthActor,
    State(state): State<WebhookState>,
    Path(id): Path<String>,
) -> Result<Json<WebhookInfoResponse>> {
    actor.require_admin()?;

    // Verify the project exists.
    state
        .project_store
        .get(&id)
        .await
        .map_err(|e| ApiError::Internal(format!("Project store: {e}")))?
        .ok_or_else(|| ApiError::NotFound(format!("Project {id} not found")))?;

    let new_secret = generate_webhook_secret();
    state
        .secrets_store
        .set_secret(WEBHOOK_SCOPE, &id, &Secret::new(&new_secret))
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to store webhook secret: {e}")))?;

    let url = format!("/webhooks/{{provider}}/{id}");

    Ok(Json(WebhookInfoResponse {
        url,
        secret: new_secret,
    }))
}

/// Helper to get `&dyn SecretsStore` from the state (avoids borrow-checker
/// issues with `Arc` deref inside async).
fn secrets_store_ref(state: &WebhookState) -> &(dyn SecretsStore + Send + Sync) {
    state.secrets_store.as_ref()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_verify_hmac_sha256_valid() {
        let secret = b"test-secret";
        let body = b"hello world";
        // Compute expected HMAC.
        let mut mac = Hmac::<Sha256>::new_from_slice(secret).unwrap();
        mac.update(body);
        let expected = hex::encode(mac.finalize().into_bytes());
        assert!(verify_hmac_sha256(secret, body, &expected));
    }

    #[test]
    fn test_verify_hmac_sha256_invalid() {
        let secret = b"test-secret";
        let body = b"hello world";
        assert!(!verify_hmac_sha256(secret, body, "deadbeef"));
    }

    #[test]
    fn test_verify_hmac_sha256_bad_hex() {
        let secret = b"test-secret";
        let body = b"hello world";
        assert!(!verify_hmac_sha256(secret, body, "not-hex!!!"));
    }

    #[test]
    fn test_verify_signature_github() {
        let secret = "my-webhook-secret";
        let body = b"payload body";
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body);
        let sig_hex = hex::encode(mac.finalize().into_bytes());

        let mut headers = HeaderMap::new();
        headers.insert(
            "X-Hub-Signature-256",
            format!("sha256={sig_hex}").parse().unwrap(),
        );

        assert!(verify_signature("github", &headers, secret, body).is_ok());
    }

    #[test]
    fn test_verify_signature_github_wrong_sig() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "X-Hub-Signature-256",
            "sha256=0000000000000000000000000000000000000000000000000000000000000000"
                .parse()
                .unwrap(),
        );

        let result = verify_signature("github", &headers, "secret", b"body");
        assert!(result.is_err());
    }

    #[test]
    fn test_verify_signature_gitea() {
        let secret = "gitea-secret";
        let body = b"gitea payload";
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body);
        let sig_hex = hex::encode(mac.finalize().into_bytes());

        let mut headers = HeaderMap::new();
        headers.insert("X-Gitea-Signature", sig_hex.parse().unwrap());

        assert!(verify_signature("gitea", &headers, secret, body).is_ok());
    }

    #[test]
    fn test_verify_signature_forgejo() {
        let secret = "forgejo-secret";
        let body = b"forgejo payload";
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body);
        let sig_hex = hex::encode(mac.finalize().into_bytes());

        let mut headers = HeaderMap::new();
        headers.insert("X-Forgejo-Signature", sig_hex.parse().unwrap());

        assert!(verify_signature("forgejo", &headers, secret, body).is_ok());
    }

    #[test]
    fn test_verify_signature_gitlab() {
        let secret = "gitlab-token-value";
        let mut headers = HeaderMap::new();
        headers.insert("X-Gitlab-Token", secret.parse().unwrap());

        assert!(verify_signature("gitlab", &headers, secret, b"anything").is_ok());
    }

    #[test]
    fn test_verify_signature_gitlab_wrong_token() {
        let mut headers = HeaderMap::new();
        headers.insert("X-Gitlab-Token", "wrong-token".parse().unwrap());

        let result = verify_signature("gitlab", &headers, "correct-token", b"anything");
        assert!(result.is_err());
    }

    #[test]
    fn test_verify_signature_unknown_provider() {
        let headers = HeaderMap::new();
        let result = verify_signature("bitbucket", &headers, "secret", b"body");
        assert!(result.is_err());
    }

    #[test]
    fn test_generate_webhook_secret_length() {
        let secret = generate_webhook_secret();
        assert_eq!(secret.len(), 64); // 32 bytes = 64 hex chars
        assert!(secret.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_generate_webhook_secret_unique() {
        let s1 = generate_webhook_secret();
        let s2 = generate_webhook_secret();
        assert_ne!(s1, s2);
    }
}
