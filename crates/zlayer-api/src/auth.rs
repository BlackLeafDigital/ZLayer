//! JWT authentication for the `ZLayer` API
//!
//! Provides token creation, verification, and Axum request extractors.

use axum::{
    extract::FromRequestParts,
    http::{header::AUTHORIZATION, request::Parts},
};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use secrecy::{ExposeSecret, SecretString};
use std::time::Duration;
use tracing::warn;

use crate::error::ApiError;
use crate::handlers::users::AuthActor;
use crate::storage::{
    EnvironmentStorage, GroupStorage, PermissionLevel, PermissionStorage, SubjectKind,
};

/// JWT claims.
///
/// Re-exported from `zlayer-types` so cross-crate consumers can name the
/// type without depending on `zlayer-api`. Token signing/verification
/// helpers (below) are still owned by this crate.
pub use zlayer_types::jwt::Claims;

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
        use jsonwebtoken::errors::ErrorKind;
        let variant = match e.kind() {
            ErrorKind::ExpiredSignature => "Expired",
            ErrorKind::InvalidSignature => "InvalidSignature",
            ErrorKind::InvalidToken => "InvalidToken",
            ErrorKind::InvalidIssuer => "InvalidIssuer",
            ErrorKind::InvalidAudience => "InvalidAudience",
            ErrorKind::Base64(_) => "Base64",
            ErrorKind::Json(_) => "Json",
            ErrorKind::Utf8(_) => "Utf8",
            ErrorKind::Crypto(_) => "Crypto",
            _ => "Other",
        };
        warn!(
            event = "jwt_verify_failed",
            variant = variant,
            error = %e,
            "Token verification failed",
        );
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
    /// Identity facade for combined user-store + credential-store writes.
    /// Handlers touching both stores MUST go through this; `None` only in
    /// legacy tests that build a router without persistent stores.
    pub identity: Option<std::sync::Arc<crate::identity::IdentityManager>>,
    /// Configured OIDC providers. Empty when SSO is disabled.
    pub oidc_clients: std::collections::HashMap<String, crate::oidc::OidcClient>,
    /// In-flight OIDC CSRF / PKCE state.
    pub oidc_state: std::sync::Arc<crate::oidc::StateTokenStore>,
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
        let auth_state_opt = parts.extensions.get::<AuthState>().cloned();
        tracing::warn!(
            auth_state_present = auth_state_opt.is_some(),
            "SessionAuthUser: extension lookup",
        );
        let auth_state = auth_state_opt
            .ok_or_else(|| ApiError::Internal("Auth state not configured".to_string()))?;

        let jar = axum_extra::extract::cookie::CookieJar::from_headers(&parts.headers);
        let token_opt = jar
            .get(crate::middleware::cookies::SESSION_COOKIE)
            .map(|c| c.value().to_string());
        tracing::warn!(
            session_cookie_present = token_opt.is_some(),
            cookie_len = token_opt.as_ref().map_or(0, String::len),
            "SessionAuthUser: session cookie lookup",
        );
        let token = token_opt
            .ok_or_else(|| ApiError::Unauthorized("Session cookie missing".to_string()))?;

        let verify_res = verify_token(auth_state.jwt_secret.expose_secret(), &token);
        match &verify_res {
            Ok(c) => tracing::warn!(
                token_verify_result = "ok",
                sub_prefix = %c.sub.chars().take(8).collect::<String>(),
                "SessionAuthUser: token verified",
            ),
            Err(e) => tracing::warn!(
                token_verify_result = "err",
                error = %e,
                "SessionAuthUser: token verification failed",
            ),
        }
        let claims = verify_res?;
        if claims.is_expired() {
            tracing::warn!(
                sub_prefix = %claims.sub.chars().take(8).collect::<String>(),
                exp = claims.exp,
                "SessionAuthUser: claims is_expired() == true",
            );
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
/// use zlayer_api::auth::resolve_registry_auth_async;
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

// =========================================================================
// Cluster-replicated secrets RBAC
// =========================================================================

/// Parse `"env:{id}"` or `"project:{pid}:env:{id}"` into the env id.
///
/// Returns `None` when `scope` does not match either env-shaped form. Used by
/// [`require_secret_perm`] to decide whether per-env RBAC should be consulted
/// as a fallback for secret access.
#[must_use]
pub fn parse_env_id_from_scope(scope: &str) -> Option<String> {
    if let Some(rest) = scope.strip_prefix("env:") {
        if rest.is_empty() {
            return None;
        }
        return Some(rest.to_string());
    }
    if let Some(rest) = scope.strip_prefix("project:") {
        if let Some((_pid, env_part)) = rest.split_once(":env:") {
            if env_part.is_empty() {
                return None;
            }
            return Some(env_part.to_string());
        }
    }
    None
}

/// Build the cluster-wide storage key used for a per-secret permission grant.
///
/// Mirrors the `{scope}:{name}` shape used by `PersistentSecretsStore` and the
/// Raft secrets state machine, so a `StoredPermission` whose `resource_id`
/// equals this string targets exactly one secret row.
#[must_use]
pub fn secret_storage_key(scope: &str, name: &str) -> String {
    format!("{scope}:{name}")
}

/// Returns `true` when `perms` includes a grant on `("secret", storage_key)`
/// (or a `("secret", *)` wildcard) at >= `level`.
fn secret_grants_satisfy(
    perms: &[crate::storage::StoredPermission],
    storage_key: &str,
    level: PermissionLevel,
) -> bool {
    perms.iter().any(|p| {
        p.resource_kind == "secret"
            && (p.resource_id.is_none() || p.resource_id.as_deref() == Some(storage_key))
            && p.level >= level
    })
}

/// Map a requested per-secret access level to the env-fallback level.
///
/// `Read` requires env `Read`; `Execute`/`Write` require env `Write`; `None`
/// requires nothing. Used inside [`require_secret_perm`] when the secret's
/// scope is env-shaped (`"env:{id}"` or `"project:{pid}:env:{id}"`).
fn env_fallback_level(level: PermissionLevel) -> Option<PermissionLevel> {
    match level {
        PermissionLevel::None => None,
        PermissionLevel::Read => Some(PermissionLevel::Read),
        PermissionLevel::Execute | PermissionLevel::Write => Some(PermissionLevel::Write),
    }
}

/// Check whether `actor` is allowed to perform an operation at `level`
/// against the secret named `name` in `scope`.
///
/// Resolution order:
///
/// 1. **Admin role** — short-circuits to `Ok(())` regardless of grants.
/// 2. **Direct or wildcard user grant** on
///    `(resource_kind: "secret", resource_id: Some("{scope}:{name}"))` or
///    `(resource_kind: "secret", resource_id: None)` at >= `level`.
/// 3. **Env-scope fallback** — when `scope` matches the env-scope shape
///    (`"env:{env_id}"` or `"project:{pid}:env:{env_id}"`) and `env_store` is
///    `Some`, fall back to per-env RBAC via
///    [`AuthActor::require_env_access`]. `Read` maps to env `Read`;
///    `Execute`/`Write` map to env `Write`. The env id is **not** verified
///    against `env_store` here — `require_env_access` only consults the
///    permission store; the env-store argument is reserved for future
///    expansion (e.g. project-scope inheritance) and currently unused.
/// 4. **Group secret grants** — for every group `actor` belongs to, repeat
///    the secret-kind direct/wildcard check at the group subject.
/// 5. **Otherwise** — `Forbidden`.
///
/// # Arguments
///
/// * `actor` — the authenticated caller.
/// * `perm_store` — backing permission store; queried for direct, wildcard,
///   and group grants.
/// * `group_store` — group store used to expand the actor's group membership
///   for step 4.
/// * `env_store` — environment store, only consulted when `scope` is
///   env-shaped (currently reserved; the env-fallback path uses
///   `perm_store` directly via [`AuthActor::require_env_access`]). Pass
///   `None` to disable the env-fallback branch entirely.
/// * `scope` — the secret's scope namespace.
/// * `name` — the secret's name within the scope.
/// * `level` — minimum [`PermissionLevel`] required.
///
/// # Errors
///
/// - [`ApiError::Forbidden`] when no rule grants access.
/// - [`ApiError::Internal`] when the permission or group store call fails.
pub async fn require_secret_perm(
    actor: &AuthActor,
    perm_store: &(dyn PermissionStorage + 'static),
    group_store: &(dyn GroupStorage + 'static),
    env_store: Option<&(dyn EnvironmentStorage + 'static)>,
    scope: &str,
    name: &str,
    level: PermissionLevel,
) -> Result<(), ApiError> {
    // 1. Admin shortcut.
    if actor.has_role("admin") {
        return Ok(());
    }

    let storage_key = secret_storage_key(scope, name);

    // 2. Direct + wildcard secret-kind grants for the user.
    let user_perms = perm_store
        .list_for_subject(SubjectKind::User, &actor.user_id)
        .await
        .map_err(|e| ApiError::Internal(format!("Permission store: {e}")))?;
    if secret_grants_satisfy(&user_perms, &storage_key, level) {
        return Ok(());
    }

    // 3. Env-scope fallback.
    if env_store.is_some() {
        if let Some(env_id) = parse_env_id_from_scope(scope) {
            return match env_fallback_level(level) {
                None => Ok(()),
                Some(env_level) => actor
                    .require_env_access(perm_store, &env_id, env_level)
                    .await
                    .map_err(|e| match e {
                        ApiError::Forbidden(_) => ApiError::Forbidden(format!(
                            "user lacks {level} on secret {storage_key}"
                        )),
                        other => other,
                    }),
            };
        }
    }

    // 4. Group-membership secret grants.
    let group_ids = group_store
        .list_groups_for_user(&actor.user_id)
        .await
        .map_err(|e| ApiError::Internal(format!("Group store: {e}")))?;
    for group_id in group_ids {
        let group_perms = perm_store
            .list_for_subject(SubjectKind::Group, &group_id)
            .await
            .map_err(|e| ApiError::Internal(format!("Permission store: {e}")))?;
        if secret_grants_satisfy(&group_perms, &storage_key, level) {
            return Ok(());
        }
    }

    // 5. Denied.
    Err(ApiError::Forbidden(format!(
        "user lacks {level} on secret {storage_key}"
    )))
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
            node_id: None,
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
            node_id: None,
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

    // =====================================================================
    // require_secret_perm
    // =====================================================================

    use crate::storage::{
        InMemoryEnvironmentStore, InMemoryGroupStore, InMemoryPermissionStore, StoredPermission,
        StoredUserGroup,
    };

    fn user_actor(id: &str) -> AuthActor {
        AuthActor {
            user_id: id.to_string(),
            roles: vec!["user".to_string()],
            email: None,
        }
    }

    fn admin_actor(id: &str) -> AuthActor {
        AuthActor {
            user_id: id.to_string(),
            roles: vec!["admin".to_string()],
            email: None,
        }
    }

    #[test]
    fn test_parse_env_id_from_scope_shapes() {
        assert_eq!(parse_env_id_from_scope("env:abc"), Some("abc".to_string()));
        assert_eq!(
            parse_env_id_from_scope("project:p1:env:xyz"),
            Some("xyz".to_string())
        );
        assert_eq!(parse_env_id_from_scope("default"), None);
        assert_eq!(parse_env_id_from_scope("env:"), None);
        assert_eq!(parse_env_id_from_scope("project:p1:env:"), None);
    }

    #[test]
    fn test_secret_storage_key_shape() {
        assert_eq!(secret_storage_key("default", "foo"), "default:foo");
        assert_eq!(
            secret_storage_key("project:p1:env:xyz", "API_KEY"),
            "project:p1:env:xyz:API_KEY"
        );
    }

    #[tokio::test]
    async fn admin_short_circuits() {
        let perms = InMemoryPermissionStore::new();
        let groups = InMemoryGroupStore::new();
        let envs = InMemoryEnvironmentStore::new();
        let actor = admin_actor("admin-1");

        // Admin succeeds even with no grants and a non-env scope.
        require_secret_perm(
            &actor,
            &perms,
            &groups,
            Some(&envs),
            "default",
            "anything",
            PermissionLevel::Write,
        )
        .await
        .expect("admin should always be allowed");

        // Same with env_store=None.
        require_secret_perm(
            &actor,
            &perms,
            &groups,
            None,
            "env:abc",
            "API_KEY",
            PermissionLevel::Write,
        )
        .await
        .expect("admin should always be allowed (no env store)");
    }

    #[tokio::test]
    async fn direct_grant_user_read() {
        let perms = InMemoryPermissionStore::new();
        let groups = InMemoryGroupStore::new();
        let actor = user_actor("u1");

        let grant = StoredPermission::new(
            SubjectKind::User,
            "u1",
            "secret",
            Some("default:foo".to_string()),
            PermissionLevel::Read,
        );
        <InMemoryPermissionStore as PermissionStorage>::grant(&perms, &grant)
            .await
            .unwrap();

        require_secret_perm(
            &actor,
            &perms,
            &groups,
            None,
            "default",
            "foo",
            PermissionLevel::Read,
        )
        .await
        .expect("direct user Read grant should allow Read");
    }

    #[tokio::test]
    async fn direct_grant_user_insufficient_level() {
        let perms = InMemoryPermissionStore::new();
        let groups = InMemoryGroupStore::new();
        let actor = user_actor("u1");

        let grant = StoredPermission::new(
            SubjectKind::User,
            "u1",
            "secret",
            Some("default:foo".to_string()),
            PermissionLevel::Read,
        );
        <InMemoryPermissionStore as PermissionStorage>::grant(&perms, &grant)
            .await
            .unwrap();

        let err = require_secret_perm(
            &actor,
            &perms,
            &groups,
            None,
            "default",
            "foo",
            PermissionLevel::Write,
        )
        .await
        .unwrap_err();

        assert!(
            matches!(err, ApiError::Forbidden(_)),
            "Read grant should not satisfy Write request, got {err:?}"
        );
    }

    #[tokio::test]
    async fn wildcard_grant_user_write() {
        let perms = InMemoryPermissionStore::new();
        let groups = InMemoryGroupStore::new();
        let actor = user_actor("u1");

        let grant = StoredPermission::new(
            SubjectKind::User,
            "u1",
            "secret",
            None,
            PermissionLevel::Write,
        );
        <InMemoryPermissionStore as PermissionStorage>::grant(&perms, &grant)
            .await
            .unwrap();

        // Any scope, any name should pass at Write or below.
        require_secret_perm(
            &actor,
            &perms,
            &groups,
            None,
            "default",
            "foo",
            PermissionLevel::Write,
        )
        .await
        .expect("wildcard Write should allow Write on default:foo");

        require_secret_perm(
            &actor,
            &perms,
            &groups,
            None,
            "anything-else",
            "bar",
            PermissionLevel::Read,
        )
        .await
        .expect("wildcard Write should allow Read on any other scope");
    }

    #[tokio::test]
    async fn group_membership_grant() {
        let perms = InMemoryPermissionStore::new();
        let groups = InMemoryGroupStore::new();
        let actor = user_actor("u1");

        // Create group g1 and add u1 to it.
        let g1 = StoredUserGroup::new("g1");
        let g1_id = g1.id.clone();
        groups.store(&g1).await.unwrap();
        groups.add_member(&g1_id, "u1").await.unwrap();

        // Grant the GROUP a Read on default:foo.
        let grant = StoredPermission::new(
            SubjectKind::Group,
            &g1_id,
            "secret",
            Some("default:foo".to_string()),
            PermissionLevel::Read,
        );
        <InMemoryPermissionStore as PermissionStorage>::grant(&perms, &grant)
            .await
            .unwrap();

        // u1 should be allowed via group membership; no env-fallback because
        // scope `default` is not env-shaped.
        require_secret_perm(
            &actor,
            &perms,
            &groups,
            None,
            "default",
            "foo",
            PermissionLevel::Read,
        )
        .await
        .expect("group membership Read grant should allow Read");

        // But Write should still be denied (group only has Read).
        let err = require_secret_perm(
            &actor,
            &perms,
            &groups,
            None,
            "default",
            "foo",
            PermissionLevel::Write,
        )
        .await
        .unwrap_err();
        assert!(
            matches!(err, ApiError::Forbidden(_)),
            "group Read grant should not cover Write, got {err:?}"
        );
    }

    #[tokio::test]
    async fn env_scope_fallback_read() {
        let perms = InMemoryPermissionStore::new();
        let groups = InMemoryGroupStore::new();
        let envs = InMemoryEnvironmentStore::new();
        let actor = user_actor("u1");

        // Grant u1 environment Read on env "abc"; NO secret-kind grants.
        let env_grant = StoredPermission::new(
            SubjectKind::User,
            "u1",
            "environment",
            Some("abc".to_string()),
            PermissionLevel::Read,
        );
        <InMemoryPermissionStore as PermissionStorage>::grant(&perms, &env_grant)
            .await
            .unwrap();

        // env-shaped scope should fall back to per-env RBAC.
        require_secret_perm(
            &actor,
            &perms,
            &groups,
            Some(&envs),
            "env:abc",
            "API_KEY",
            PermissionLevel::Read,
        )
        .await
        .expect("env Read should permit secret Read on env-shaped scope");

        // Without env_store, the fallback is disabled — should now deny.
        let err = require_secret_perm(
            &actor,
            &perms,
            &groups,
            None,
            "env:abc",
            "API_KEY",
            PermissionLevel::Read,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, ApiError::Forbidden(_)));
    }

    #[tokio::test]
    async fn env_scope_fallback_project() {
        let perms = InMemoryPermissionStore::new();
        let groups = InMemoryGroupStore::new();
        let envs = InMemoryEnvironmentStore::new();
        let actor = user_actor("u1");

        let env_grant = StoredPermission::new(
            SubjectKind::User,
            "u1",
            "environment",
            Some("abc".to_string()),
            PermissionLevel::Read,
        );
        <InMemoryPermissionStore as PermissionStorage>::grant(&perms, &env_grant)
            .await
            .unwrap();

        // project:p1:env:abc should be parsed to env id "abc".
        require_secret_perm(
            &actor,
            &perms,
            &groups,
            Some(&envs),
            "project:p1:env:abc",
            "DB_URL",
            PermissionLevel::Read,
        )
        .await
        .expect("project-scoped env Read should permit secret Read");

        // Read-level env grant should NOT satisfy a Write request — env
        // fallback maps Write to env Write.
        let err = require_secret_perm(
            &actor,
            &perms,
            &groups,
            Some(&envs),
            "project:p1:env:abc",
            "DB_URL",
            PermissionLevel::Write,
        )
        .await
        .unwrap_err();
        assert!(
            matches!(err, ApiError::Forbidden(_)),
            "env Read should not satisfy secret Write, got {err:?}"
        );
    }

    #[tokio::test]
    async fn no_grants_denied() {
        let perms = InMemoryPermissionStore::new();
        let groups = InMemoryGroupStore::new();
        let envs = InMemoryEnvironmentStore::new();
        let actor = user_actor("nobody");

        let err = require_secret_perm(
            &actor,
            &perms,
            &groups,
            Some(&envs),
            "default",
            "foo",
            PermissionLevel::Read,
        )
        .await
        .unwrap_err();

        match err {
            ApiError::Forbidden(msg) => {
                assert!(
                    msg.contains("default:foo"),
                    "denial message should reference storage key, got: {msg}"
                );
            }
            other => panic!("expected Forbidden, got {other:?}"),
        }
    }
}
