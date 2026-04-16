//! JWT authentication for the `ZLayer` API
//!
//! Provides token creation, verification, and Axum request extractors.

use axum::{
    extract::FromRequestParts,
    http::{header::AUTHORIZATION, request::Parts},
};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::warn;

use crate::error::ApiError;

/// JWT claims
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    /// Subject (user ID or API key ID)
    pub sub: String,
    /// Expiration time (Unix timestamp)
    pub exp: u64,
    /// Issued at (Unix timestamp)
    pub iat: u64,
    /// Issuer
    pub iss: String,
    /// Roles/permissions
    #[serde(default)]
    pub roles: Vec<String>,
    /// Email address of the user (optional; absent on tokens issued before
    /// user accounts existed, hence `#[serde(default)]` for back-compat).
    #[serde(default)]
    pub email: Option<String>,
}

impl Claims {
    /// Create new claims.
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

/// Create a JWT token.
///
/// # Errors
///
/// Returns an error if token encoding fails.
pub fn create_token(
    secret: &str,
    subject: impl Into<String>,
    expiry: Duration,
    roles: Vec<String>,
) -> Result<String, ApiError> {
    create_token_with_email(secret, subject, expiry, roles, None)
}

/// Create a JWT token, optionally embedding the user's email.
///
/// Used by the browser-session login flow so that downstream handlers can
/// identify the user without a second database lookup. API-key tokens pass
/// `None`.
///
/// # Errors
///
/// Returns an error if token encoding fails.
pub fn create_token_with_email(
    secret: &str,
    subject: impl Into<String>,
    expiry: Duration,
    roles: Vec<String>,
    email: Option<String>,
) -> Result<String, ApiError> {
    let claims = Claims::new(subject, expiry, roles, email);

    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .map_err(|e| ApiError::Internal(format!("Failed to create token: {e}")))
}

/// Verify and decode a JWT token.
///
/// # Errors
///
/// Returns an error if token verification or decoding fails.
pub fn verify_token(secret: &str, token: &str) -> Result<Claims, ApiError> {
    let mut validation = Validation::default();
    validation.set_issuer(&["zlayer"]);

    decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &validation,
    )
    .map(|data| data.claims)
    .map_err(|e| {
        warn!(error = %e, "Token verification failed");
        ApiError::Unauthorized(format!("Invalid token: {e}"))
    })
}

/// Authenticated user extracted from request
#[derive(Debug, Clone)]
pub struct AuthUser {
    pub claims: Claims,
}

impl AuthUser {
    /// Get the user/subject ID
    #[must_use]
    pub fn id(&self) -> &str {
        &self.claims.sub
    }

    /// Get the subject (alias for id)
    #[must_use]
    pub fn subject(&self) -> &str {
        &self.claims.sub
    }

    /// Check if user has a role
    #[must_use]
    pub fn has_role(&self, role: &str) -> bool {
        self.claims.has_role(role)
    }

    /// Require a specific role, returning Forbidden if not present.
    ///
    /// # Errors
    ///
    /// Returns `ApiError::Forbidden` if the user does not have the required role.
    pub fn require_role(&self, role: &str) -> Result<(), ApiError> {
        if self.has_role(role) {
            Ok(())
        } else {
            Err(ApiError::Forbidden(format!("Role '{role}' required")))
        }
    }
}

/// State needed for authentication
#[derive(Clone)]
pub struct AuthState {
    pub jwt_secret: SecretString,
    /// Credential store for validating API keys (optional; falls back to rejection
    /// if not set).
    pub credential_store: Option<
        std::sync::Arc<
            zlayer_secrets::CredentialStore<std::sync::Arc<zlayer_secrets::PersistentSecretsStore>>,
        >,
    >,
    /// User account store (for bootstrap / login / me / users CRUD).
    /// Optional so legacy instances without the store still boot; endpoints
    /// that require it return 503 when `None`.
    pub user_store: Option<std::sync::Arc<dyn crate::storage::UserStorage>>,
    /// Whether to set cookies with `Secure=true`. Production: `true`.
    /// Dev/local-http: `false` so the browser doesn't drop the cookie.
    pub cookie_secure: bool,
}

/// Axum extractor for authenticated users
impl<S> FromRequestParts<S> for AuthUser
where
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        // Get auth state from extensions
        let auth_state = parts
            .extensions
            .get::<AuthState>()
            .cloned()
            .ok_or_else(|| ApiError::Internal("Auth state not configured".to_string()))?;

        // Extract Authorization header
        let auth_header = parts
            .headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| ApiError::Unauthorized("Missing Authorization header".to_string()))?;

        // Parse Bearer token
        let token = auth_header.strip_prefix("Bearer ").ok_or_else(|| {
            ApiError::Unauthorized("Invalid Authorization header format".to_string())
        })?;

        // Verify token
        let claims = verify_token(auth_state.jwt_secret.expose_secret(), token)?;

        if claims.is_expired() {
            return Err(ApiError::Unauthorized("Token expired".to_string()));
        }

        Ok(AuthUser { claims })
    }
}

/// Optional authentication extractor
/// Returns None if no auth header, error if invalid auth
#[derive(Debug, Clone)]
pub struct OptionalAuthUser(pub Option<AuthUser>);

impl<S> FromRequestParts<S> for OptionalAuthUser
where
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        // Check if Authorization header exists
        if parts.headers.get(AUTHORIZATION).is_none() {
            return Ok(OptionalAuthUser(None));
        }

        // If header exists, it must be valid
        let user = AuthUser::from_request_parts(parts, state).await?;
        Ok(OptionalAuthUser(Some(user)))
    }
}

/// Authenticated user extracted from a session cookie.
///
/// Mirror of `AuthUser` but sources the JWT from the `zlayer_session` cookie
/// (`HttpOnly`, set by `/auth/login`). Browser sessions use this; API clients
/// using `Authorization: Bearer …` keep using `AuthUser`.
#[derive(Debug, Clone)]
pub struct SessionAuthUser {
    pub claims: Claims,
}

impl SessionAuthUser {
    /// Get the user/subject ID.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.claims.sub
    }

    /// Get the email embedded in the session token, if present.
    #[must_use]
    pub fn email(&self) -> Option<&str> {
        self.claims.email.as_deref()
    }

    /// Check if the user has a role.
    #[must_use]
    pub fn has_role(&self, role: &str) -> bool {
        self.claims.has_role(role)
    }

    /// Require a specific role, returning Forbidden if not present.
    ///
    /// # Errors
    ///
    /// Returns `ApiError::Forbidden` if the user does not have the required role.
    pub fn require_role(&self, role: &str) -> Result<(), ApiError> {
        if self.has_role(role) {
            Ok(())
        } else {
            Err(ApiError::Forbidden(format!("Role '{role}' required")))
        }
    }
}

impl<S> FromRequestParts<S> for SessionAuthUser
where
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let auth_state = parts
            .extensions
            .get::<AuthState>()
            .cloned()
            .ok_or_else(|| ApiError::Internal("Auth state not configured".to_string()))?;

        let jar = axum_extra::extract::cookie::CookieJar::from_headers(&parts.headers);
        let token = jar
            .get(crate::middleware::cookies::SESSION_COOKIE)
            .map(|c| c.value().to_string())
            .ok_or_else(|| ApiError::Unauthorized("Session cookie missing".to_string()))?;

        let claims = verify_token(auth_state.jwt_secret.expose_secret(), &token)?;
        if claims.is_expired() {
            return Err(ApiError::Unauthorized("Session expired".to_string()));
        }

        Ok(SessionAuthUser { claims })
    }
}

/// Resolve registry authentication asynchronously, handling
/// `AuthSource::SecretStore` via the provided credential store.
///
/// For all other `AuthSource` variants this delegates to the synchronous
/// [`AuthResolver::resolve_source`] method. The `SecretStore` variant
/// performs an async lookup against the `RegistryCredentialStore`.
///
/// # Arguments
///
/// * `resolver` - The sync auth resolver (owns per-registry config)
/// * `registry` - Registry hostname to resolve credentials for
/// * `cred_store` - Optional credential store; if `None` and the source is
///   `SecretStore`, returns `Anonymous` with a warning
///
/// # Examples
///
/// ```rust,ignore
/// use zlayer_api_zql::auth::resolve_registry_auth_async;
///
/// let auth = resolve_registry_auth_async(&resolver, "ghcr.io", Some(&reg_store)).await;
/// ```
pub async fn resolve_registry_auth_async<S: zlayer_secrets::SecretsStore>(
    resolver: &zlayer_core::auth::AuthResolver,
    registry: &str,
    cred_store: Option<&zlayer_secrets::RegistryCredentialStore<S>>,
) -> oci_client::secrets::RegistryAuth {
    let source = resolver.source_for_registry(registry);

    match source {
        zlayer_core::auth::AuthSource::SecretStore { credential_id } => {
            let Some(store) = cred_store else {
                tracing::warn!(
                    credential_id,
                    "No credential store provided for SecretStore auth"
                );
                return oci_client::secrets::RegistryAuth::Anonymous;
            };
            match store.get(credential_id).await {
                Ok(Some(cred)) => match store.get_password(credential_id).await {
                    Ok(secret) => oci_client::secrets::RegistryAuth::Basic(
                        cred.username.clone(),
                        secret.expose().to_string(),
                    ),
                    Err(e) => {
                        tracing::error!(
                            error = %e,
                            credential_id,
                            "Failed to retrieve registry password"
                        );
                        oci_client::secrets::RegistryAuth::Anonymous
                    }
                },
                Ok(None) => {
                    tracing::warn!(credential_id, "Registry credential not found");
                    oci_client::secrets::RegistryAuth::Anonymous
                }
                Err(e) => {
                    tracing::error!(
                        error = %e,
                        credential_id,
                        "Failed to look up registry credential"
                    );
                    oci_client::secrets::RegistryAuth::Anonymous
                }
            }
        }
        other => resolver.resolve_source(other, registry),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_SECRET: &str = "test-secret-key-for-jwt-testing-purposes";

    #[test]
    fn test_create_and_verify_token() {
        let token = create_token(
            TEST_SECRET,
            "user123",
            Duration::from_secs(3600),
            vec!["reader".to_string()],
        )
        .unwrap();

        let claims = verify_token(TEST_SECRET, &token).unwrap();

        assert_eq!(claims.sub, "user123");
        assert!(!claims.is_expired());
        assert!(claims.has_role("reader"));
        assert!(!claims.has_role("admin"));
    }

    #[test]
    fn test_expired_token() {
        let claims = Claims {
            sub: "user123".to_string(),
            exp: 0, // Expired immediately
            iat: 0,
            iss: "zlayer".to_string(),
            roles: vec![],
            email: None,
        };

        assert!(claims.is_expired());
    }

    #[test]
    fn test_admin_role_overrides() {
        let claims = Claims {
            sub: "admin".to_string(),
            exp: u64::MAX,
            iat: 0,
            iss: "zlayer".to_string(),
            roles: vec!["admin".to_string()],
            email: None,
        };

        // Admin should have access to any role
        assert!(claims.has_role("admin"));
        assert!(claims.has_role("reader"));
        assert!(claims.has_role("writer"));
        assert!(claims.has_role("anything"));
    }

    #[test]
    fn test_invalid_token_fails() {
        let result = verify_token(TEST_SECRET, "invalid.token.here");
        assert!(result.is_err());
    }

    #[test]
    fn test_wrong_secret_fails() {
        let token =
            create_token(TEST_SECRET, "user123", Duration::from_secs(3600), vec![]).unwrap();

        let result = verify_token("wrong-secret", &token);
        assert!(result.is_err());
    }

    #[test]
    fn test_create_and_verify_token_with_email() {
        let token = create_token_with_email(
            TEST_SECRET,
            "user123",
            Duration::from_secs(3600),
            vec!["reader".to_string()],
            Some("foo@bar.com".to_string()),
        )
        .unwrap();

        let claims = verify_token(TEST_SECRET, &token).unwrap();

        assert_eq!(claims.sub, "user123");
        assert_eq!(claims.email.as_deref(), Some("foo@bar.com"));
        assert!(!claims.is_expired());
        assert!(claims.has_role("reader"));
    }

    #[test]
    fn test_create_token_backcompat_has_no_email() {
        let token = create_token(
            TEST_SECRET,
            "user123",
            Duration::from_secs(3600),
            vec!["reader".to_string()],
        )
        .unwrap();

        let claims = verify_token(TEST_SECRET, &token).unwrap();

        assert!(claims.email.is_none());
    }

    #[test]
    fn test_claims_email_missing_in_old_token() {
        // Simulates deserializing a pre-email token: the JSON payload has no
        // `email` key at all. `#[serde(default)]` should fill in `None`.
        let json = r#"{
            "sub": "user123",
            "exp": 9999999999,
            "iat": 0,
            "iss": "zlayer",
            "roles": ["reader"]
        }"#;

        let claims: Claims = serde_json::from_str(json).unwrap();

        assert_eq!(claims.sub, "user123");
        assert!(claims.email.is_none());
        assert!(claims.has_role("reader"));
    }

    #[tokio::test]
    async fn test_resolve_registry_auth_async_with_cred_store() {
        let key = zlayer_secrets::EncryptionKey::generate();
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("test_async_auth.sqlite");
        let store = zlayer_secrets::PersistentSecretsStore::open(&db_path, key)
            .await
            .unwrap();
        let reg_store = zlayer_secrets::RegistryCredentialStore::new(store);

        // Create a credential
        let cred = reg_store
            .create(
                "private.io",
                "ci-bot",
                "s3cret-token",
                zlayer_secrets::RegistryAuthType::Token,
            )
            .await
            .unwrap();

        // Configure the resolver to use SecretStore for private.io
        let config = zlayer_core::auth::AuthConfig {
            registries: vec![zlayer_core::auth::RegistryAuthConfig {
                registry: "private.io".to_string(),
                source: zlayer_core::auth::AuthSource::SecretStore {
                    credential_id: cred.id.clone(),
                },
            }],
            default: zlayer_core::auth::AuthSource::Anonymous,
            ..Default::default()
        };
        let resolver = zlayer_core::auth::AuthResolver::new(config);

        // Async resolution should return the stored credential
        let auth = resolve_registry_auth_async(&resolver, "private.io", Some(&reg_store)).await;
        match auth {
            oci_client::secrets::RegistryAuth::Basic(username, password) => {
                assert_eq!(username, "ci-bot");
                assert_eq!(password, "s3cret-token");
            }
            _ => panic!("Expected Basic auth from credential store"),
        }

        // Non-SecretStore registries should fall through to the sync path
        let auth = resolve_registry_auth_async(&resolver, "docker.io", Some(&reg_store)).await;
        assert!(matches!(auth, oci_client::secrets::RegistryAuth::Anonymous));
    }

    #[tokio::test]
    async fn test_resolve_registry_auth_async_missing_credential() {
        let key = zlayer_secrets::EncryptionKey::generate();
        let temp_dir = tempfile::tempdir().unwrap();
        let db_path = temp_dir.path().join("test_async_auth_missing.sqlite");
        let store = zlayer_secrets::PersistentSecretsStore::open(&db_path, key)
            .await
            .unwrap();
        let reg_store = zlayer_secrets::RegistryCredentialStore::new(store);

        let config = zlayer_core::auth::AuthConfig {
            registries: vec![zlayer_core::auth::RegistryAuthConfig {
                registry: "private.io".to_string(),
                source: zlayer_core::auth::AuthSource::SecretStore {
                    credential_id: "nonexistent-id".to_string(),
                },
            }],
            default: zlayer_core::auth::AuthSource::Anonymous,
            ..Default::default()
        };
        let resolver = zlayer_core::auth::AuthResolver::new(config);

        // Missing credential should return Anonymous
        let auth = resolve_registry_auth_async(&resolver, "private.io", Some(&reg_store)).await;
        assert!(matches!(auth, oci_client::secrets::RegistryAuth::Anonymous));
    }

    #[tokio::test]
    async fn test_resolve_registry_auth_async_no_store_provided() {
        let config = zlayer_core::auth::AuthConfig {
            registries: vec![zlayer_core::auth::RegistryAuthConfig {
                registry: "private.io".to_string(),
                source: zlayer_core::auth::AuthSource::SecretStore {
                    credential_id: "some-id".to_string(),
                },
            }],
            default: zlayer_core::auth::AuthSource::Anonymous,
            ..Default::default()
        };
        let resolver = zlayer_core::auth::AuthResolver::new(config);

        // No credential store provided -- should return Anonymous
        let auth = resolve_registry_auth_async::<zlayer_secrets::PersistentSecretsStore>(
            &resolver,
            "private.io",
            None,
        )
        .await;
        assert!(matches!(auth, oci_client::secrets::RegistryAuth::Anonymous));
    }
}
