//! API configuration

use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::time::Duration;

/// API server configuration
#[derive(Clone, Serialize, Deserialize)]
pub struct ApiConfig {
    /// Bind address
    #[serde(default = "default_bind")]
    pub bind: SocketAddr,

    /// JWT secret key (should be at least 32 bytes)
    #[serde(skip_serializing)]
    pub jwt_secret: SecretString,

    /// JWT token expiration
    #[serde(default = "default_token_expiry")]
    pub token_expiry: Duration,

    /// Enable Swagger UI
    #[serde(default = "default_true")]
    pub swagger_enabled: bool,

    /// Rate limiting configuration
    #[serde(default)]
    pub rate_limit: RateLimitConfig,

    /// CORS configuration
    #[serde(default)]
    pub cors: CorsConfig,

    /// Credential store for API key authentication (not serialised)
    #[serde(skip)]
    pub credential_store: Option<
        std::sync::Arc<
            zlayer_secrets::CredentialStore<std::sync::Arc<zlayer_secrets::PersistentSecretsStore>>,
        >,
    >,

    /// User store for the `/api/v1/users` endpoints (not serialised).
    ///
    /// When `Some`, the router injects this store into `AuthState.user_store`
    /// so user CRUD handlers can read/write persistent user records. When
    /// `None`, the user endpoints return an internal error.
    #[serde(skip)]
    pub user_store: Option<std::sync::Arc<dyn crate::storage::UserStorage>>,
}

fn default_bind() -> SocketAddr {
    "0.0.0.0:3669".parse().unwrap()
}

fn default_token_expiry() -> Duration {
    Duration::from_secs(3600) // 1 hour
}

fn default_true() -> bool {
    true
}

impl std::fmt::Debug for ApiConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiConfig")
            .field("bind", &self.bind)
            .field("jwt_secret", &"<redacted>")
            .field("token_expiry", &self.token_expiry)
            .field("swagger_enabled", &self.swagger_enabled)
            .field("rate_limit", &self.rate_limit)
            .field("cors", &self.cors)
            .field("credential_store", &self.credential_store.is_some())
            .field("user_store", &self.user_store.is_some())
            .finish()
    }
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            bind: default_bind(),
            jwt_secret: SecretString::from("CHANGE_ME_IN_PRODUCTION".to_string()),
            token_expiry: default_token_expiry(),
            swagger_enabled: true,
            rate_limit: RateLimitConfig::default(),
            cors: CorsConfig::default(),
            credential_store: None,
            user_store: None,
        }
    }
}

/// Rate limiting configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitConfig {
    /// Enable rate limiting
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Requests per second per IP
    #[serde(default = "default_rps")]
    pub requests_per_second: u32,

    /// Burst size
    #[serde(default = "default_burst")]
    pub burst_size: u32,
}

fn default_rps() -> u32 {
    100
}

fn default_burst() -> u32 {
    50
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            requests_per_second: default_rps(),
            burst_size: default_burst(),
        }
    }
}

/// CORS configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorsConfig {
    /// Allowed origins (empty = allow all)
    #[serde(default)]
    pub allowed_origins: Vec<String>,

    /// Allow credentials
    #[serde(default)]
    pub allow_credentials: bool,

    /// Max age for preflight cache (seconds)
    #[serde(default = "default_max_age")]
    pub max_age: u64,
}

fn default_max_age() -> u64 {
    3600
}

impl Default for CorsConfig {
    fn default() -> Self {
        Self {
            allowed_origins: Vec::new(),
            allow_credentials: false,
            max_age: default_max_age(),
        }
    }
}
