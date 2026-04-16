//! Wire types for the Notifiers page.
//!
//! Mirrors `zlayer_api::storage::StoredNotifier` and its nested
//! `NotifierConfig` enum, but defined locally so the hydrate/WASM build
//! does not need to depend on `zlayer-api` (which is SSR-only).
//!
//! # Discriminated-union pattern
//!
//! `WireNotifierConfig` is a `#[serde(tag = "type", rename_all = "snake_case")]`
//! enum, mirroring the daemon's [`NotifierConfig`][api]. Callers must include
//! a `"type"` field when serializing; missing or mismatched variants are
//! surfaced as a serde error on the way in. This is the pattern that
//! downstream Manager pages (workflows, tasks) should follow when they pick
//! up similarly shaped tagged unions.
//!
//! [api]: https://docs.rs/zlayer-api — `NotifierConfig` in `storage/mod.rs`.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A stored notifier as returned by `/api/v1/notifiers`.
///
/// Fields match the daemon's `StoredNotifier` exactly; timestamps are
/// serialised server-side as RFC-3339 strings so receiving them here as
/// `String` round-trips lossless.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WireNotifier {
    /// UUID identifier.
    pub id: String,
    /// Display name (e.g. `"deploy-alerts"`).
    pub name: String,
    /// Notification channel type as a lowercase string: one of
    /// `"slack"`, `"discord"`, `"webhook"`, `"smtp"`. Kept as a plain
    /// string rather than its own enum so the UI can render arbitrary
    /// future kinds without a wire-type bump.
    pub kind: String,
    /// Whether the notifier is active. Disabled notifiers are skipped
    /// by any dispatcher.
    pub enabled: bool,
    /// Channel-specific configuration (webhook URL, SMTP settings, etc.).
    pub config: WireNotifierConfig,
    /// RFC-3339 creation timestamp.
    pub created_at: String,
    /// RFC-3339 last-updated timestamp.
    pub updated_at: String,
}

/// Channel-specific configuration for a notifier.
///
/// Tagged union: the JSON form carries a `"type"` discriminator, e.g.
/// `{"type":"slack","webhook_url":"https://..."}`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WireNotifierConfig {
    /// Slack incoming webhook configuration.
    Slack {
        /// Slack webhook URL.
        webhook_url: String,
    },
    /// Discord webhook configuration.
    Discord {
        /// Discord webhook URL.
        webhook_url: String,
    },
    /// Generic HTTP webhook configuration.
    Webhook {
        /// Target URL.
        url: String,
        /// HTTP method (defaults to `"POST"` server-side).
        #[serde(default)]
        method: Option<String>,
        /// Extra headers to send with the request.
        #[serde(default)]
        headers: Option<HashMap<String, String>>,
    },
    /// SMTP email configuration.
    Smtp {
        /// SMTP server host.
        host: String,
        /// SMTP server port.
        port: u16,
        /// SMTP authentication username.
        username: String,
        /// SMTP authentication password.
        password: String,
        /// RFC 5322 `From` address.
        from: String,
        /// RFC 5322 `To` addresses; must contain at least one entry.
        to: Vec<String>,
    },
}

impl WireNotifierConfig {
    /// Return the snake-case discriminator (`"slack"`, `"discord"`,
    /// `"webhook"`, `"smtp"`) for this variant. Useful for building the
    /// sibling `kind` field in [`WireNotifier`] without re-serialising.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            WireNotifierConfig::Slack { .. } => "slack",
            WireNotifierConfig::Discord { .. } => "discord",
            WireNotifierConfig::Webhook { .. } => "webhook",
            WireNotifierConfig::Smtp { .. } => "smtp",
        }
    }
}

/// Response from `POST /api/v1/notifiers/{id}/test` — mirrors
/// `zlayer_api::handlers::notifiers::TestNotifierResponse`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NotifierTestResult {
    /// Whether the test notification was sent successfully.
    pub success: bool,
    /// Human-readable status message.
    pub message: String,
}
