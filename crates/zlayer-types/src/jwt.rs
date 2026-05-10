//! JWT claims and related auth wire types.
//!
//! Lifted from `zlayer-api` so cross-crate consumers (`zlayer-agent`,
//! the CLI, future federation code) can name the JWT claims without
//! depending on `zlayer-api`. Token signing/verification helpers stay
//! in `zlayer-api::auth`.

use serde::{Deserialize, Serialize};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// JWT claims used by every protected endpoint.
///
/// `node_id` is `Some` only on node JWTs — tokens issued by the leader
/// during cluster join with `roles: ["node"]` so a node can authenticate
/// to its peers' internal endpoints distinct from any user identity.
/// All other JWT issuers leave it `None`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    /// Subject — user id, api-key id, or `"node:{node_id}"` for node JWTs.
    pub sub: String,

    /// Expiration time (Unix seconds).
    pub exp: u64,

    /// Issued at (Unix seconds).
    pub iat: u64,

    /// Issuer (canonically `"zlayer"`; federation will use a cluster-id form).
    pub iss: String,

    /// Role claims, e.g. `["admin"]`, `["operator"]`, `["node"]`.
    ///
    /// `#[serde(default)]` preserves back-compat with tokens minted before
    /// the field existed.
    #[serde(default)]
    pub roles: Vec<String>,

    /// Email — embedded for session JWTs so the manager UI doesn't
    /// have to round-trip to the user store on every request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,

    /// Cluster-wide node UUID. `Some` only for JWTs minted by the
    /// leader at cluster-join time (where `roles` contains `"node"`).
    /// All other token kinds leave this `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
}

impl Claims {
    /// Create new claims.
    ///
    /// `node_id` is left `None`; node JWTs are constructed via struct
    /// literal at the cluster-join site.
    ///
    /// # Panics
    ///
    /// Panics if the system clock is before the Unix epoch.
    pub fn new(
        subject: impl Into<String>,
        expiry: Duration,
        roles: Vec<String>,
        email: Option<String>,
    ) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before Unix epoch")
            .as_secs();

        Self {
            sub: subject.into(),
            exp: now + expiry.as_secs(),
            iat: now,
            iss: "zlayer".to_string(),
            roles,
            email,
            node_id: None,
        }
    }

    /// Check if token is expired.
    ///
    /// # Panics
    ///
    /// Panics if the system clock is before the Unix epoch.
    #[must_use]
    pub fn is_expired(&self) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before Unix epoch")
            .as_secs();
        self.exp < now
    }

    /// Check if the user has a specific role
    #[must_use]
    pub fn has_role(&self, role: &str) -> bool {
        self.roles.iter().any(|r| r == role || r == "admin")
    }
}
