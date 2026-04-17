//! Notifier CRUD and test-notification endpoints.
//!
//! Notifiers are named notification channels with configurable sinks (Slack,
//! Discord, generic webhook, or SMTP). When triggered, they send a message to
//! the configured endpoint.
//!
//! Routes:
//!
//! ```text
//! GET    /api/v1/notifiers             -> list
//! POST   /api/v1/notifiers             -> create (admin)
//! GET    /api/v1/notifiers/{id}        -> get
//! PATCH  /api/v1/notifiers/{id}        -> update (admin)
//! DELETE /api/v1/notifiers/{id}        -> delete (admin)
//! POST   /api/v1/notifiers/{id}/test   -> send test notification (admin)
//! ```
//!
//! Read endpoints accept any authenticated actor; mutating endpoints
//! (create, update, delete, test) require the `admin` role.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use tracing::info;
use utoipa::ToSchema;

use crate::error::{ApiError, Result};
use crate::handlers::users::AuthActor;
use crate::storage::{NotifierConfig, NotifierKind, NotifierStorage, StoredNotifier};

/// State for notifier endpoints.
#[derive(Clone)]
pub struct NotifiersState {
    /// Underlying notifier storage backend.
    pub store: Arc<dyn NotifierStorage>,
    /// HTTP client for sending webhook notifications.
    pub http_client: reqwest::Client,
}

impl NotifiersState {
    /// Build a new state from a notifier store.
    #[must_use]
    pub fn new(store: Arc<dyn NotifierStorage>) -> Self {
        Self {
            store,
            http_client: reqwest::Client::new(),
        }
    }
}

// ---- Request/response types ----

/// Body for `POST /api/v1/notifiers`.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CreateNotifierRequest {
    /// Notifier name.
    pub name: String,
    /// Notification channel type.
    pub kind: NotifierKind,
    /// Channel-specific configuration.
    pub config: NotifierConfig,
}

/// Body for `PATCH /api/v1/notifiers/{id}`.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct UpdateNotifierRequest {
    /// Updated name.
    #[serde(default)]
    pub name: Option<String>,
    /// Updated enabled flag.
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Updated configuration.
    #[serde(default)]
    pub config: Option<NotifierConfig>,
}

/// Response from `POST /api/v1/notifiers/{id}/test`.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct TestNotifierResponse {
    /// Whether the test notification was sent successfully.
    pub success: bool,
    /// Status message.
    pub message: String,
}

// ---- Endpoints ----

/// List notifiers.
///
/// # Errors
///
/// Returns [`ApiError::Internal`] if the notifier store fails.
#[utoipa::path(
    get,
    path = "/api/v1/notifiers",
    responses(
        (status = 200, description = "List of notifiers", body = Vec<StoredNotifier>),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "Notifiers"
)]
pub async fn list_notifiers(
    _actor: AuthActor,
    State(state): State<NotifiersState>,
) -> Result<Json<Vec<StoredNotifier>>> {
    let notifiers = state
        .store
        .list()
        .await
        .map_err(|e| ApiError::Internal(format!("Notifier store: {e}")))?;
    Ok(Json(notifiers))
}

/// Create a new notifier. Admin only.
///
/// # Errors
///
/// Returns [`ApiError::Forbidden`] when the caller is not an admin,
/// [`ApiError::BadRequest`] for an empty name or kind/config mismatch,
/// or [`ApiError::Internal`] when the notifier store fails.
#[utoipa::path(
    post,
    path = "/api/v1/notifiers",
    request_body = CreateNotifierRequest,
    responses(
        (status = 201, description = "Notifier created", body = StoredNotifier),
        (status = 400, description = "Invalid request"),
        (status = 403, description = "Admin role required"),
    ),
    security(("bearer_auth" = [])),
    tag = "Notifiers"
)]
pub async fn create_notifier(
    actor: AuthActor,
    State(state): State<NotifiersState>,
    Json(req): Json<CreateNotifierRequest>,
) -> Result<(StatusCode, Json<StoredNotifier>)> {
    actor.require_admin()?;

    let name = req.name.trim();
    if name.is_empty() {
        return Err(ApiError::BadRequest(
            "Notifier name cannot be empty".to_string(),
        ));
    }
    if name.len() > 256 {
        return Err(ApiError::BadRequest(
            "Notifier name cannot exceed 256 characters".to_string(),
        ));
    }

    validate_kind_config_match(req.kind, &req.config)?;

    let notifier = StoredNotifier::new(name, req.kind, req.config);

    state
        .store
        .store(&notifier)
        .await
        .map_err(|e| ApiError::Internal(format!("Notifier store: {e}")))?;

    info!(
        notifier_id = %notifier.id,
        notifier_name = %notifier.name,
        kind = %notifier.kind,
        "Created notifier"
    );

    Ok((StatusCode::CREATED, Json(notifier)))
}

/// Fetch a single notifier by id.
///
/// # Errors
///
/// Returns [`ApiError::NotFound`] if no notifier with the given id exists,
/// or [`ApiError::Internal`] when the store fails.
#[utoipa::path(
    get,
    path = "/api/v1/notifiers/{id}",
    params(("id" = String, Path, description = "Notifier id")),
    responses(
        (status = 200, description = "Notifier", body = StoredNotifier),
        (status = 404, description = "Not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "Notifiers"
)]
pub async fn get_notifier(
    _actor: AuthActor,
    State(state): State<NotifiersState>,
    Path(id): Path<String>,
) -> Result<Json<StoredNotifier>> {
    let notifier = state
        .store
        .get(&id)
        .await
        .map_err(|e| ApiError::Internal(format!("Notifier store: {e}")))?
        .ok_or_else(|| ApiError::NotFound(format!("Notifier {id} not found")))?;
    Ok(Json(notifier))
}

/// Update a notifier. Admin only.
///
/// Supports partial updates: only fields present in the request body are
/// changed.
///
/// # Errors
///
/// Returns [`ApiError::Forbidden`] for non-admins, [`ApiError::NotFound`]
/// when the notifier does not exist, [`ApiError::BadRequest`] for
/// validation errors, or [`ApiError::Internal`] when the store fails.
#[utoipa::path(
    patch,
    path = "/api/v1/notifiers/{id}",
    params(("id" = String, Path, description = "Notifier id")),
    request_body = UpdateNotifierRequest,
    responses(
        (status = 200, description = "Notifier updated", body = StoredNotifier),
        (status = 400, description = "Invalid request"),
        (status = 403, description = "Admin role required"),
        (status = 404, description = "Not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "Notifiers"
)]
pub async fn update_notifier(
    actor: AuthActor,
    State(state): State<NotifiersState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateNotifierRequest>,
) -> Result<Json<StoredNotifier>> {
    actor.require_admin()?;

    let mut notifier = state
        .store
        .get(&id)
        .await
        .map_err(|e| ApiError::Internal(format!("Notifier store: {e}")))?
        .ok_or_else(|| ApiError::NotFound(format!("Notifier {id} not found")))?;

    if let Some(ref name) = req.name {
        let name = name.trim();
        if name.is_empty() {
            return Err(ApiError::BadRequest(
                "Notifier name cannot be empty".to_string(),
            ));
        }
        if name.len() > 256 {
            return Err(ApiError::BadRequest(
                "Notifier name cannot exceed 256 characters".to_string(),
            ));
        }
        notifier.name = name.to_string();
    }

    if let Some(enabled) = req.enabled {
        notifier.enabled = enabled;
    }

    if let Some(ref config) = req.config {
        validate_kind_config_match(notifier.kind, config)?;
        notifier.config = config.clone();
    }

    notifier.updated_at = Utc::now();

    state
        .store
        .store(&notifier)
        .await
        .map_err(|e| ApiError::Internal(format!("Notifier store: {e}")))?;

    info!(
        notifier_id = %notifier.id,
        notifier_name = %notifier.name,
        "Updated notifier"
    );

    Ok(Json(notifier))
}

/// Delete a notifier. Admin only.
///
/// # Errors
///
/// Returns [`ApiError::Forbidden`] for non-admins, [`ApiError::NotFound`]
/// when the notifier does not exist, or [`ApiError::Internal`] when the
/// store fails.
#[utoipa::path(
    delete,
    path = "/api/v1/notifiers/{id}",
    params(("id" = String, Path, description = "Notifier id")),
    responses(
        (status = 204, description = "Notifier deleted"),
        (status = 403, description = "Admin role required"),
        (status = 404, description = "Not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "Notifiers"
)]
pub async fn delete_notifier(
    actor: AuthActor,
    State(state): State<NotifiersState>,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    actor.require_admin()?;

    let deleted = state
        .store
        .delete(&id)
        .await
        .map_err(|e| ApiError::Internal(format!("Notifier store: {e}")))?;

    if !deleted {
        return Err(ApiError::NotFound(format!("Notifier {id} not found")));
    }

    info!(notifier_id = %id, "Deleted notifier");

    Ok(StatusCode::NO_CONTENT)
}

/// Send a test notification through a notifier. Admin only.
///
/// For Slack and Discord, sends a test message via the configured webhook
/// URL. For generic webhooks, sends a JSON test payload. For SMTP, sends a
/// real email via [`lettre`] using the notifier's configured credentials.
///
/// # Errors
///
/// Returns [`ApiError::Forbidden`] for non-admins, [`ApiError::NotFound`]
/// when the notifier does not exist, or [`ApiError::Internal`] when the
/// notification fails with a truly internal error. Upstream failures
/// (bad credentials, unreachable SMTP server, invalid addresses) are
/// returned with HTTP 200 and `success: false` in the body, matching the
/// convention established by the Slack/Discord/Webhook arms.
#[utoipa::path(
    post,
    path = "/api/v1/notifiers/{id}/test",
    params(("id" = String, Path, description = "Notifier id")),
    responses(
        (status = 200, description = "Test notification sent", body = TestNotifierResponse),
        (status = 403, description = "Admin role required"),
        (status = 404, description = "Not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "Notifiers"
)]
pub async fn test_notifier(
    actor: AuthActor,
    State(state): State<NotifiersState>,
    Path(id): Path<String>,
) -> Result<Json<TestNotifierResponse>> {
    actor.require_admin()?;

    let notifier = state
        .store
        .get(&id)
        .await
        .map_err(|e| ApiError::Internal(format!("Notifier store: {e}")))?
        .ok_or_else(|| ApiError::NotFound(format!("Notifier {id} not found")))?;

    info!(
        notifier_id = %notifier.id,
        notifier_name = %notifier.name,
        kind = %notifier.kind,
        "Sending test notification"
    );

    send_test_notification(&state.http_client, &notifier).await
}

// ---- Internal helpers ----

/// Validate that the `NotifierKind` matches the `NotifierConfig` variant.
fn validate_kind_config_match(kind: NotifierKind, config: &NotifierConfig) -> Result<()> {
    let matches = matches!(
        (kind, config),
        (NotifierKind::Slack, NotifierConfig::Slack { .. })
            | (NotifierKind::Discord, NotifierConfig::Discord { .. })
            | (NotifierKind::Webhook, NotifierConfig::Webhook { .. })
            | (NotifierKind::Smtp, NotifierConfig::Smtp { .. })
    );
    if !matches {
        return Err(ApiError::BadRequest(format!(
            "Notifier kind '{kind}' does not match the provided config variant"
        )));
    }
    Ok(())
}

/// Send a test notification through the configured channel.
///
/// Dispatches to a per-variant helper so each sink's logic lives in its own
/// function (easier to read and test). Upstream failures are returned as
/// HTTP 200 with `success: false`; only truly internal errors bubble up as
/// [`ApiError`].
async fn send_test_notification(
    client: &reqwest::Client,
    notifier: &StoredNotifier,
) -> Result<Json<TestNotifierResponse>> {
    match &notifier.config {
        NotifierConfig::Slack { webhook_url } => send_slack_test(client, webhook_url).await,
        NotifierConfig::Discord { webhook_url } => send_discord_test(client, webhook_url).await,
        NotifierConfig::Webhook {
            url,
            method,
            headers,
        } => send_webhook_test(client, url, method.as_deref(), headers.as_ref()).await,
        NotifierConfig::Smtp {
            host,
            port,
            username,
            password,
            from,
            to,
        } => {
            let params = SmtpParams {
                host,
                port: *port,
                username,
                password,
                from,
                to,
            };
            let result = send_smtp_message(
                &params,
                "ZLayer test notification",
                "This is a test notification from ZLayer.",
            )
            .await;
            Ok(Json(match result {
                Ok(()) => TestNotifierResponse {
                    success: true,
                    message: "SMTP test notification sent successfully".to_string(),
                },
                Err(message) => TestNotifierResponse {
                    success: false,
                    message,
                },
            }))
        }
    }
}

/// Send a test notification to a Slack Incoming Webhook URL.
async fn send_slack_test(
    client: &reqwest::Client,
    webhook_url: &str,
) -> Result<Json<TestNotifierResponse>> {
    let payload = serde_json::json!({ "text": "ZLayer test notification" });
    let resp = client
        .post(webhook_url)
        .json(&payload)
        .send()
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to send Slack notification: {e}")))?;

    if resp.status().is_success() {
        Ok(Json(TestNotifierResponse {
            success: true,
            message: "Slack test notification sent successfully".to_string(),
        }))
    } else {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        Ok(Json(TestNotifierResponse {
            success: false,
            message: format!("Slack webhook returned {status}: {body}"),
        }))
    }
}

/// Send a test notification to a Discord Webhook URL.
async fn send_discord_test(
    client: &reqwest::Client,
    webhook_url: &str,
) -> Result<Json<TestNotifierResponse>> {
    let payload = serde_json::json!({ "content": "ZLayer test notification" });
    let resp = client
        .post(webhook_url)
        .json(&payload)
        .send()
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to send Discord notification: {e}")))?;

    if resp.status().is_success() {
        Ok(Json(TestNotifierResponse {
            success: true,
            message: "Discord test notification sent successfully".to_string(),
        }))
    } else {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        Ok(Json(TestNotifierResponse {
            success: false,
            message: format!("Discord webhook returned {status}: {body}"),
        }))
    }
}

/// Send a test notification to a generic HTTP webhook.
async fn send_webhook_test(
    client: &reqwest::Client,
    url: &str,
    method: Option<&str>,
    headers: Option<&std::collections::HashMap<String, String>>,
) -> Result<Json<TestNotifierResponse>> {
    let http_method = method.unwrap_or("POST");
    let payload = serde_json::json!({
        "event": "test",
        "source": "zlayer",
        "message": "ZLayer test notification"
    });

    let mut request = match http_method.to_uppercase().as_str() {
        "GET" => client.get(url),
        "PUT" => client.put(url).json(&payload),
        "PATCH" => client.patch(url).json(&payload),
        _ => client.post(url).json(&payload),
    };

    if let Some(extra_headers) = headers {
        for (key, value) in extra_headers {
            request = request.header(key.as_str(), value.as_str());
        }
    }

    let resp = request
        .send()
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to send webhook notification: {e}")))?;

    if resp.status().is_success() {
        Ok(Json(TestNotifierResponse {
            success: true,
            message: "Webhook test notification sent successfully".to_string(),
        }))
    } else {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        Ok(Json(TestNotifierResponse {
            success: false,
            message: format!("Webhook returned {status}: {body}"),
        }))
    }
}

/// Borrowed view of an SMTP configuration, used by [`send_smtp_message`].
///
/// This bundles the connection/identity parameters so the helper's
/// signature stays small. All fields are borrowed so callers can pass slices
/// straight out of `NotifierConfig::Smtp { .. }` without cloning.
pub(crate) struct SmtpParams<'a> {
    /// SMTP server hostname.
    pub host: &'a str,
    /// SMTP server port (465 for implicit TLS, 587 for STARTTLS, etc.).
    pub port: u16,
    /// SMTP authentication username.
    pub username: &'a str,
    /// SMTP authentication password.
    pub password: &'a str,
    /// RFC 5322 `From` address (must parse as [`lettre::message::Mailbox`]).
    pub from: &'a str,
    /// RFC 5322 `To` addresses; must contain at least one recipient.
    pub to: &'a [String],
}

/// Send an SMTP message via [`lettre`] using an async rustls transport.
///
/// Parses the `from` and `to` addresses, connects to `host:port` using
/// implicit TLS (submission ports) or STARTTLS (as auto-negotiated by
/// `lettre`'s `relay()` constructor), authenticates with
/// `username`/`password`, and sends the message with the given `subject`
/// and plain-text `body`.
///
/// Returns `Ok(())` on successful delivery handoff to the SMTP server.
/// Returns `Err(String)` with a caller-friendly message for any failure
/// (invalid addresses, empty recipient list, TLS/auth failure, upstream
/// 4xx/5xx, etc.). Callers are expected to translate this into a
/// `TestNotifierResponse { success: false, message }` in the test-endpoint
/// path, matching the convention established by the Slack/Discord/Webhook
/// arms (HTTP 200 + `success: false` for upstream errors, not 5xx).
pub(crate) async fn send_smtp_message(
    params: &SmtpParams<'_>,
    subject: &str,
    body: &str,
) -> std::result::Result<(), String> {
    use lettre::message::Mailbox;
    use lettre::transport::smtp::authentication::Credentials;
    use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

    if params.to.is_empty() {
        return Err("SMTP notifier has no recipients".to_string());
    }

    let from_mailbox: Mailbox = params
        .from
        .parse()
        .map_err(|e| format!("Invalid from address: {e}"))?;

    let mut builder = Message::builder().from(from_mailbox).subject(subject);

    for addr in params.to {
        let mailbox: Mailbox = addr
            .parse()
            .map_err(|e| format!("Invalid recipient {addr}: {e}"))?;
        builder = builder.to(mailbox);
    }

    let message = builder
        .body(body.to_string())
        .map_err(|e| format!("Failed to build SMTP message: {e}"))?;

    let transport: AsyncSmtpTransport<Tokio1Executor> =
        AsyncSmtpTransport::<Tokio1Executor>::relay(params.host)
            .map_err(|e| format!("Invalid SMTP host {}: {e}", params.host))?
            .port(params.port)
            .credentials(Credentials::new(
                params.username.to_string(),
                params.password.to_string(),
            ))
            .build();

    transport
        .send(message)
        .await
        .map_err(|e| format!("SMTP error: {e}"))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::storage::NotifierKind;

    #[test]
    fn test_create_request_deserialize_slack() {
        let req: CreateNotifierRequest = serde_json::from_str(
            r#"{"name":"slack-alerts","kind":"slack","config":{"type":"slack","webhook_url":"https://hooks.slack.com/test"}}"#,
        )
        .unwrap();
        assert_eq!(req.name, "slack-alerts");
        assert_eq!(req.kind, NotifierKind::Slack);
        match &req.config {
            NotifierConfig::Slack { webhook_url } => {
                assert_eq!(webhook_url, "https://hooks.slack.com/test");
            }
            other => panic!("Expected Slack config, got {other:?}"),
        }
    }

    #[test]
    fn test_create_request_deserialize_discord() {
        let req: CreateNotifierRequest = serde_json::from_str(
            r#"{"name":"discord-alerts","kind":"discord","config":{"type":"discord","webhook_url":"https://discord.com/api/webhooks/test"}}"#,
        )
        .unwrap();
        assert_eq!(req.kind, NotifierKind::Discord);
    }

    #[test]
    fn test_create_request_deserialize_webhook() {
        let req: CreateNotifierRequest = serde_json::from_str(
            r#"{
                "name": "generic-hook",
                "kind": "webhook",
                "config": {
                    "type": "webhook",
                    "url": "https://example.com/hook",
                    "method": "PUT",
                    "headers": {"Authorization": "Bearer token123"}
                }
            }"#,
        )
        .unwrap();
        assert_eq!(req.kind, NotifierKind::Webhook);
        match &req.config {
            NotifierConfig::Webhook {
                url,
                method,
                headers,
            } => {
                assert_eq!(url, "https://example.com/hook");
                assert_eq!(method.as_deref(), Some("PUT"));
                let h = headers.as_ref().unwrap();
                assert_eq!(h.get("Authorization").unwrap(), "Bearer token123");
            }
            other => panic!("Expected Webhook config, got {other:?}"),
        }
    }

    #[test]
    fn test_create_request_deserialize_smtp() {
        let req: CreateNotifierRequest = serde_json::from_str(
            r#"{
                "name": "email-alerts",
                "kind": "smtp",
                "config": {
                    "type": "smtp",
                    "host": "smtp.example.com",
                    "port": 587,
                    "username": "user",
                    "password": "pass",
                    "from": "noreply@example.com",
                    "to": ["admin@example.com"]
                }
            }"#,
        )
        .unwrap();
        assert_eq!(req.kind, NotifierKind::Smtp);
    }

    #[test]
    fn test_update_request_partial() {
        let req: UpdateNotifierRequest = serde_json::from_str(r#"{"enabled": false}"#).unwrap();
        assert!(req.name.is_none());
        assert_eq!(req.enabled, Some(false));
        assert!(req.config.is_none());
    }

    #[test]
    fn test_kind_config_match_valid() {
        assert!(validate_kind_config_match(
            NotifierKind::Slack,
            &NotifierConfig::Slack {
                webhook_url: "https://test".to_string()
            }
        )
        .is_ok());
    }

    #[test]
    fn test_kind_config_match_invalid() {
        assert!(validate_kind_config_match(
            NotifierKind::Slack,
            &NotifierConfig::Discord {
                webhook_url: "https://test".to_string()
            }
        )
        .is_err());
    }

    #[test]
    fn test_notifier_kind_display() {
        assert_eq!(NotifierKind::Slack.to_string(), "slack");
        assert_eq!(NotifierKind::Discord.to_string(), "discord");
        assert_eq!(NotifierKind::Webhook.to_string(), "webhook");
        assert_eq!(NotifierKind::Smtp.to_string(), "smtp");
    }

    #[test]
    fn test_notifier_config_serde_roundtrip() {
        let configs = vec![
            NotifierConfig::Slack {
                webhook_url: "https://hooks.slack.com/test".to_string(),
            },
            NotifierConfig::Discord {
                webhook_url: "https://discord.com/api/webhooks/test".to_string(),
            },
            NotifierConfig::Webhook {
                url: "https://example.com/hook".to_string(),
                method: Some("PUT".to_string()),
                headers: Some(HashMap::from([(
                    "X-Custom".to_string(),
                    "value".to_string(),
                )])),
            },
            NotifierConfig::Smtp {
                host: "smtp.example.com".to_string(),
                port: 587,
                username: "user".to_string(),
                password: "pass".to_string(),
                from: "noreply@example.com".to_string(),
                to: vec!["admin@example.com".to_string()],
            },
        ];
        for config in configs {
            let json = serde_json::to_string(&config).unwrap();
            let parsed: NotifierConfig = serde_json::from_str(&json).unwrap();
            assert_eq!(
                serde_json::to_value(&config).unwrap(),
                serde_json::to_value(&parsed).unwrap()
            );
        }
    }

    #[test]
    fn test_test_notifier_response_serialize() {
        let resp = TestNotifierResponse {
            success: true,
            message: "ok".to_string(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"success\":true"));
        assert!(json.contains("\"message\":\"ok\""));
    }

    #[tokio::test]
    async fn test_send_smtp_empty_recipients() {
        // Empty recipient list is rejected before any network activity.
        let to: Vec<String> = Vec::new();
        let params = SmtpParams {
            host: "smtp.example.com",
            port: 465,
            username: "user",
            password: "pass",
            from: "noreply@example.com",
            to: &to,
        };
        let err = send_smtp_message(&params, "subject", "body")
            .await
            .expect_err("empty recipients should fail");
        assert!(err.contains("no recipients"), "got: {err}");
    }

    #[tokio::test]
    async fn test_send_smtp_invalid_from() {
        // Malformed `from` address is rejected before any network activity.
        let to = vec!["admin@example.com".to_string()];
        let params = SmtpParams {
            host: "smtp.example.com",
            port: 465,
            username: "user",
            password: "pass",
            from: "not-an-email-address",
            to: &to,
        };
        let err = send_smtp_message(&params, "subject", "body")
            .await
            .expect_err("invalid from should fail");
        assert!(err.contains("Invalid from address"), "got: {err}");
    }

    #[tokio::test]
    async fn test_send_smtp_unreachable_host() {
        // Pointing at 127.0.0.1:1 is guaranteed unreachable (privileged port,
        // no listener) so `lettre` returns a connection error. The helper
        // must surface it as a `String` error, not panic.
        let to = vec!["admin@example.com".to_string()];
        let params = SmtpParams {
            host: "127.0.0.1",
            port: 1,
            username: "user",
            password: "pass",
            from: "noreply@example.com",
            to: &to,
        };
        let err = send_smtp_message(&params, "subject", "body")
            .await
            .expect_err("unreachable host should fail");
        assert!(err.starts_with("SMTP error:"), "got: {err}");
    }

    #[tokio::test]
    async fn test_send_test_notification_smtp_unreachable() {
        // End-to-end: drive `send_test_notification` with an SMTP notifier
        // pointing at an unreachable host and assert we get HTTP 200 with
        // `success: false`, matching the Slack/Discord/Webhook convention.
        let notifier = StoredNotifier::new(
            "smtp-test",
            NotifierKind::Smtp,
            NotifierConfig::Smtp {
                host: "127.0.0.1".to_string(),
                port: 1,
                username: "user".to_string(),
                password: "pass".to_string(),
                from: "noreply@example.com".to_string(),
                to: vec!["admin@example.com".to_string()],
            },
        );

        let client = reqwest::Client::new();
        let resp = send_test_notification(&client, &notifier)
            .await
            .expect("send_test_notification must not return ApiError for upstream SMTP failure");
        let body = resp.0;
        assert!(!body.success, "expected success=false, got: {body:?}");
        assert!(
            body.message.starts_with("SMTP error:"),
            "unexpected message: {}",
            body.message
        );
    }
}
