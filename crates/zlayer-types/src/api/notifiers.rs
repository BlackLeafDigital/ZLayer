//! Notifier API DTOs.
//!
//! Wire types for the notifier CRUD and test-notification endpoints. The
//! storage-resident types ([`NotifierKind`], [`NotifierConfig`],
//! [`StoredNotifier`]) live in [`crate::storage`] and are referenced here.

use serde::{Deserialize, Serialize};

use crate::storage::{NotifierConfig, NotifierKind};

/// Body for `POST /api/v1/notifiers`.
#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct CreateNotifierRequest {
    /// Notifier name.
    pub name: String,
    /// Notification channel type.
    pub kind: NotifierKind,
    /// Channel-specific configuration.
    pub config: NotifierConfig,
}

/// Body for `PATCH /api/v1/notifiers/{id}`.
#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
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
#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct TestNotifierResponse {
    /// Whether the test notification was sent successfully.
    pub success: bool,
    /// Status message.
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn test_test_notifier_response_serialize() {
        let resp = TestNotifierResponse {
            success: true,
            message: "ok".to_string(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"success\":true"));
        assert!(json.contains("\"message\":\"ok\""));
    }
}
