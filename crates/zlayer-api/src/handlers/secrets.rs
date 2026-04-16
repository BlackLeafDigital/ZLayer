//! Secrets management endpoints
//!
//! Provides CRUD operations for secrets management. Secret values are never
//! exposed through the API (except via an explicit admin-only `?reveal=true`
//! request) — only metadata is returned for listing and retrieval.
//!
//! ## Scoping
//!
//! Each secret lives in a *scope* — an opaque string namespace inside the
//! underlying [`SecretsStore`]. Two scope shapes are supported here:
//!
//! - **Legacy / explicit:** `scope` provided in the request body or via
//!   `?scope=` query string. Used for back-compat and API-key-style clients.
//!   When neither query nor body sets a scope, the literal scope `default`
//!   is used.
//! - **Environment-aware:** `?environment={env_id}` resolves the env from
//!   the [`EnvironmentStorage`] backend and builds the scope via
//!   [`env_scope`].
//!
//! These two routes are mutually exclusive — sending both forms in the same
//! request is rejected with `400 Bad Request`.
//!
//! ## Future follow-ups
//!
//! `?reveal=true` plaintext reads are admin-gated only. A future iteration
//! should also require a re-authentication challenge within the last 60
//! seconds (Phase 4 simplification — see CHANGELOG).

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::auth::AuthUser;
use crate::error::{ApiError, Result};
use crate::handlers::users::AuthActor;
use crate::storage::{EnvironmentStorage, StoredEnvironment};
use zlayer_secrets::{Secret, SecretMetadata, SecretsStore};

/// Default scope used when no explicit scope and no environment are provided.
pub const DEFAULT_SCOPE: &str = "default";

/// Build the storage scope string for a secret stored under an environment.
///
/// Format:
/// - `env:{env_id}` for global environments (`project_id == None`).
/// - `project:{project_id}:env:{env_id}` for project-scoped environments.
#[must_use]
pub fn env_scope(env: &StoredEnvironment) -> String {
    match env.project_id.as_deref() {
        Some(pid) => format!("project:{pid}:env:{}", env.id),
        None => format!("env:{}", env.id),
    }
}

/// Request to create or update a secret.
///
/// `scope` is optional and only honored on the legacy code path — when set
/// alongside `?environment=`, the request is rejected.
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateSecretRequest {
    /// The name of the secret.
    pub name: String,
    /// The secret value (will be encrypted at rest).
    pub value: String,
    /// Optional explicit scope (legacy form). Mutually exclusive with the
    /// `?environment=` query parameter.
    #[serde(default)]
    pub scope: Option<String>,
}

/// Response containing secret metadata. Never includes the value unless
/// the caller is on the explicit `?reveal=true` admin path, in which case
/// `value` is populated.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct SecretMetadataResponse {
    /// The name/identifier of the secret.
    pub name: String,
    /// Unix timestamp when the secret was created.
    pub created_at: i64,
    /// Unix timestamp when the secret was last updated.
    pub updated_at: i64,
    /// Version number of the secret (incremented on each update).
    pub version: u32,
    /// Plaintext value — populated only on `?reveal=true` admin reads.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

impl From<SecretMetadata> for SecretMetadataResponse {
    fn from(metadata: SecretMetadata) -> Self {
        Self {
            name: metadata.name,
            created_at: metadata.created_at,
            updated_at: metadata.updated_at,
            version: metadata.version,
            value: None,
        }
    }
}

impl From<&SecretMetadata> for SecretMetadataResponse {
    fn from(metadata: &SecretMetadata) -> Self {
        Self {
            name: metadata.name.clone(),
            created_at: metadata.created_at,
            updated_at: metadata.updated_at,
            version: metadata.version,
            value: None,
        }
    }
}

/// Result body for `POST /api/v1/secrets/bulk-import`.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct BulkImportResponse {
    /// Number of new secrets created.
    pub created: usize,
    /// Number of existing secrets updated.
    pub updated: usize,
    /// Per-line errors. Empty when every line parsed and stored cleanly.
    pub errors: Vec<String>,
}

/// State for secrets endpoints.
#[derive(Clone)]
pub struct SecretsState {
    /// Secrets store for CRUD operations.
    pub store: Arc<dyn SecretsStore + Send + Sync>,
    /// Optional environment store for env-aware routing. Without it, only
    /// the legacy `scope`-based code paths are usable.
    pub env_store: Option<Arc<dyn EnvironmentStorage>>,
}

impl SecretsState {
    /// Create a new secrets state with the given store and no environment
    /// support (legacy paths only).
    #[must_use]
    pub fn new(store: Arc<dyn SecretsStore + Send + Sync>) -> Self {
        Self {
            store,
            env_store: None,
        }
    }

    /// Create a new secrets state wired with an environment store, enabling
    /// `?environment=` routing on every endpoint.
    #[must_use]
    pub fn with_environments(
        store: Arc<dyn SecretsStore + Send + Sync>,
        env_store: Arc<dyn EnvironmentStorage>,
    ) -> Self {
        Self {
            store,
            env_store: Some(env_store),
        }
    }

    /// Resolve an env id to a [`StoredEnvironment`], returning a typed error
    /// when the env store is missing or the id is unknown.
    async fn lookup_env(&self, env_id: &str) -> Result<StoredEnvironment> {
        let store = self
            .env_store
            .as_ref()
            .ok_or_else(|| ApiError::Internal("Environment store not configured".to_string()))?;
        store
            .get(env_id)
            .await
            .map_err(|e| ApiError::Internal(format!("Environment store: {e}")))?
            .ok_or_else(|| ApiError::NotFound(format!("Environment {env_id} not found")))
    }
}

// ---- Query types ----

/// Query for create / list / get / delete endpoints.
#[derive(Debug, Default, Deserialize)]
pub struct SecretsScopeQuery {
    /// Environment id whose namespace to operate in. Mutually exclusive
    /// with `scope`.
    #[serde(default)]
    pub environment: Option<String>,
    /// Explicit scope string (legacy). Mutually exclusive with `environment`.
    #[serde(default)]
    pub scope: Option<String>,
}

impl SecretsScopeQuery {
    /// Reject requests that supply both forms. Returns `Ok(())` when at
    /// most one is set.
    fn validate_exclusive(&self) -> Result<()> {
        if self.environment.is_some() && self.scope.is_some() {
            return Err(ApiError::BadRequest(
                "Pass either ?environment= or ?scope=, not both".to_string(),
            ));
        }
        Ok(())
    }
}

/// Query for `GET /api/v1/secrets/{name}` — extends the scope query with a
/// `reveal` flag for admin-only plaintext reads.
#[derive(Debug, Default, Deserialize)]
pub struct GetSecretQuery {
    #[serde(default)]
    pub environment: Option<String>,
    #[serde(default)]
    pub scope: Option<String>,
    /// When true, include the plaintext value. Admin only.
    #[serde(default)]
    pub reveal: bool,
}

impl GetSecretQuery {
    fn validate_exclusive(&self) -> Result<()> {
        if self.environment.is_some() && self.scope.is_some() {
            return Err(ApiError::BadRequest(
                "Pass either ?environment= or ?scope=, not both".to_string(),
            ));
        }
        Ok(())
    }
}

/// Query for `POST /api/v1/secrets/bulk-import` — `environment` is required.
#[derive(Debug, Deserialize)]
pub struct BulkImportQuery {
    pub environment: String,
}

// ---- Helpers ----

/// Resolve a request's effective scope.
///
/// Precedence: `?environment=` (if env store wired) > body `scope` >
/// `?scope=` > [`DEFAULT_SCOPE`]. Mutual-exclusion is enforced before the
/// fallback chain runs.
async fn resolve_scope(
    state: &SecretsState,
    body_scope: Option<&str>,
    query: &SecretsScopeQuery,
) -> Result<String> {
    query.validate_exclusive()?;
    if body_scope.is_some() && query.environment.is_some() {
        return Err(ApiError::BadRequest(
            "Cannot combine body 'scope' with ?environment=; pick one".to_string(),
        ));
    }

    if let Some(env_id) = query.environment.as_deref() {
        let env = state.lookup_env(env_id).await?;
        return Ok(env_scope(&env));
    }
    if let Some(scope) = body_scope {
        return Ok(scope.to_string());
    }
    if let Some(scope) = query.scope.as_deref() {
        return Ok(scope.to_string());
    }
    Ok(DEFAULT_SCOPE.to_string())
}

/// Resolve scope for the GET-with-reveal path.
async fn resolve_scope_get(state: &SecretsState, query: &GetSecretQuery) -> Result<String> {
    query.validate_exclusive()?;
    if let Some(env_id) = query.environment.as_deref() {
        let env = state.lookup_env(env_id).await?;
        return Ok(env_scope(&env));
    }
    if let Some(scope) = query.scope.as_deref() {
        return Ok(scope.to_string());
    }
    Ok(DEFAULT_SCOPE.to_string())
}

// ---- Endpoints ----

/// Create or update a secret.
///
/// Stores a new secret or updates an existing one. The secret value is
/// encrypted at rest and the version number is incremented on updates.
///
/// Scope resolution: see [`resolve_scope`].
///
/// # Errors
///
/// Returns an error if validation fails, storage operations fail, or the
/// caller lacks permission.
#[utoipa::path(
    post,
    path = "/api/v1/secrets",
    request_body = CreateSecretRequest,
    params(
        ("environment" = Option<String>, Query, description = "Environment id (mutually exclusive with body 'scope')"),
        ("scope" = Option<String>, Query, description = "Explicit scope (legacy)"),
    ),
    responses(
        (status = 201, description = "Secret created", body = SecretMetadataResponse),
        (status = 200, description = "Secret updated", body = SecretMetadataResponse),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden"),
        (status = 404, description = "Environment id unknown"),
    ),
    security(("bearer_auth" = [])),
    tag = "Secrets"
)]
pub async fn create_secret(
    actor: AuthActor,
    State(state): State<SecretsState>,
    Query(query): Query<SecretsScopeQuery>,
    Json(request): Json<CreateSecretRequest>,
) -> Result<(StatusCode, Json<SecretMetadataResponse>)> {
    actor.require_admin()?;

    if request.name.is_empty() {
        return Err(ApiError::BadRequest(
            "Secret name cannot be empty".to_string(),
        ));
    }
    if request.name.len() > 256 {
        return Err(ApiError::BadRequest(
            "Secret name cannot exceed 256 characters".to_string(),
        ));
    }

    let scope = resolve_scope(&state, request.scope.as_deref(), &query).await?;

    let exists = state
        .store
        .exists(&scope, &request.name)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to check secret existence: {e}")))?;

    let secret = Secret::new(&request.value);
    state
        .store
        .set_secret(&scope, &request.name, &secret)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to store secret: {e}")))?;

    let metadata_list = state
        .store
        .list_secrets(&scope)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to list secrets: {e}")))?;

    let metadata = metadata_list
        .into_iter()
        .find(|m| m.name == request.name)
        .ok_or_else(|| {
            ApiError::Internal("Secret was stored but metadata not found".to_string())
        })?;

    let status = if exists {
        StatusCode::OK
    } else {
        StatusCode::CREATED
    };

    Ok((status, Json(SecretMetadataResponse::from(metadata))))
}

/// List secrets in a scope.
///
/// Scope resolution: see [`resolve_scope`].
///
/// # Errors
///
/// Returns an error if storage access fails.
#[utoipa::path(
    get,
    path = "/api/v1/secrets",
    params(
        ("environment" = Option<String>, Query, description = "Environment id"),
        ("scope" = Option<String>, Query, description = "Explicit scope"),
    ),
    responses(
        (status = 200, description = "List of secret metadata", body = Vec<SecretMetadataResponse>),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Environment id unknown"),
    ),
    security(("bearer_auth" = [])),
    tag = "Secrets"
)]
pub async fn list_secrets(
    _actor: AuthActor,
    State(state): State<SecretsState>,
    Query(query): Query<SecretsScopeQuery>,
) -> Result<Json<Vec<SecretMetadataResponse>>> {
    let scope = resolve_scope(&state, None, &query).await?;

    let metadata_list = state
        .store
        .list_secrets(&scope)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to list secrets: {e}")))?;

    let response: Vec<SecretMetadataResponse> = metadata_list
        .iter()
        .map(SecretMetadataResponse::from)
        .collect();

    Ok(Json(response))
}

/// Get metadata for a specific secret. With `?reveal=true` (admin only),
/// the response also includes the plaintext `value`.
///
/// Scope resolution: see [`resolve_scope_get`].
///
/// # Errors
///
/// Returns an error if the secret is not found, the caller is unauthorised
/// to reveal, or storage access fails.
#[utoipa::path(
    get,
    path = "/api/v1/secrets/{name}",
    params(
        ("name" = String, Path, description = "Secret name"),
        ("environment" = Option<String>, Query, description = "Environment id"),
        ("scope" = Option<String>, Query, description = "Explicit scope"),
        ("reveal" = Option<bool>, Query, description = "Include plaintext value (admin only)"),
    ),
    responses(
        (status = 200, description = "Secret metadata", body = SecretMetadataResponse),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Reveal requires admin"),
        (status = 404, description = "Secret not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "Secrets"
)]
pub async fn get_secret_metadata(
    actor: AuthActor,
    State(state): State<SecretsState>,
    Path(name): Path<String>,
    Query(query): Query<GetSecretQuery>,
) -> Result<Json<SecretMetadataResponse>> {
    let reveal = query.reveal;
    if reveal {
        actor.require_admin()?;
    }
    let scope = resolve_scope_get(&state, &query).await?;

    let exists = state
        .store
        .exists(&scope, &name)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to check secret existence: {e}")))?;
    if !exists {
        return Err(ApiError::NotFound(format!("Secret '{name}' not found")));
    }

    let metadata_list = state
        .store
        .list_secrets(&scope)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to list secrets: {e}")))?;

    let metadata = metadata_list
        .into_iter()
        .find(|m| m.name == name)
        .ok_or_else(|| ApiError::NotFound(format!("Secret '{name}' not found")))?;

    let mut response = SecretMetadataResponse::from(metadata);

    if reveal {
        let secret = state
            .store
            .get_secret(&scope, &name)
            .await
            .map_err(|e| ApiError::Internal(format!("Failed to read secret: {e}")))?;
        response.value = Some(secret.expose().to_string());
    }

    Ok(Json(response))
}

/// Delete a secret.
///
/// Scope resolution: see [`resolve_scope_get`] (the same query type is
/// reused minus `reveal`).
///
/// # Errors
///
/// Returns an error if the secret is not found, storage access fails, or
/// the caller lacks permission.
#[utoipa::path(
    delete,
    path = "/api/v1/secrets/{name}",
    params(
        ("name" = String, Path, description = "Secret name"),
        ("environment" = Option<String>, Query, description = "Environment id"),
        ("scope" = Option<String>, Query, description = "Explicit scope"),
    ),
    responses(
        (status = 204, description = "Secret deleted"),
        (status = 404, description = "Secret not found"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden"),
    ),
    security(("bearer_auth" = [])),
    tag = "Secrets"
)]
pub async fn delete_secret(
    actor: AuthActor,
    State(state): State<SecretsState>,
    Path(name): Path<String>,
    Query(query): Query<SecretsScopeQuery>,
) -> Result<StatusCode> {
    actor.require_admin()?;

    let scope = resolve_scope(&state, None, &query).await?;

    let exists = state
        .store
        .exists(&scope, &name)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to check secret existence: {e}")))?;
    if !exists {
        return Err(ApiError::NotFound(format!("Secret '{name}' not found")));
    }

    state
        .store
        .delete_secret(&scope, &name)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to delete secret: {e}")))?;

    Ok(StatusCode::NO_CONTENT)
}

/// Bulk-import secrets from a dotenv-style payload (`KEY=value\n…`).
///
/// Each non-empty, non-comment line is parsed into a (name, value) pair and
/// written to the env's scope. Lines that fail to parse are returned in
/// `errors` and do not abort the import. Each successful write is counted
/// as either `created` or `updated`.
///
/// # Errors
///
/// Returns [`ApiError::Forbidden`] for non-admins, [`ApiError::NotFound`]
/// when the environment id is unknown, or [`ApiError::Internal`] when the
/// secrets store fails.
#[utoipa::path(
    post,
    path = "/api/v1/secrets/bulk-import",
    request_body = String,
    params(
        ("environment" = String, Query, description = "Environment id to import into"),
    ),
    responses(
        (status = 200, description = "Import summary", body = BulkImportResponse),
        (status = 400, description = "Invalid request"),
        (status = 403, description = "Admin role required"),
        (status = 404, description = "Environment id unknown"),
    ),
    security(("bearer_auth" = [])),
    tag = "Secrets"
)]
pub async fn bulk_import_secrets(
    actor: AuthActor,
    State(state): State<SecretsState>,
    Query(query): Query<BulkImportQuery>,
    body: String,
) -> Result<Json<BulkImportResponse>> {
    actor.require_admin()?;

    let env = state.lookup_env(&query.environment).await?;
    let scope = env_scope(&env);

    let mut created = 0usize;
    let mut updated = 0usize;
    let mut errors: Vec<String> = Vec::new();

    for (lineno, raw) in body.lines().enumerate() {
        let line_no = lineno + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((name_raw, value_raw)) = line.split_once('=') else {
            errors.push(format!("line {line_no}: missing '='"));
            continue;
        };
        let name = name_raw.trim();
        if name.is_empty() {
            errors.push(format!("line {line_no}: empty key"));
            continue;
        }
        if name.len() > 256 {
            errors.push(format!("line {line_no}: key '{name}' exceeds 256 chars"));
            continue;
        }
        // Strip optional surrounding single or double quotes; otherwise pass
        // the value through verbatim (no escape processing — secrets are
        // opaque blobs).
        let value = strip_dotenv_quotes(value_raw);

        let exists = match state.store.exists(&scope, name).await {
            Ok(b) => b,
            Err(e) => {
                errors.push(format!("line {line_no}: existence check failed: {e}"));
                continue;
            }
        };
        let secret = Secret::new(value);
        if let Err(e) = state.store.set_secret(&scope, name, &secret).await {
            errors.push(format!("line {line_no}: store failed: {e}"));
            continue;
        }
        if exists {
            updated += 1;
        } else {
            created += 1;
        }
    }

    Ok(Json(BulkImportResponse {
        created,
        updated,
        errors,
    }))
}

/// Strip a single pair of matching surrounding single or double quotes from
/// a dotenv value. Whitespace outside the quotes is trimmed first; whitespace
/// inside quotes is preserved.
fn strip_dotenv_quotes(value: &str) -> &str {
    let trimmed = value.trim();
    let bytes = trimmed.as_bytes();
    if bytes.len() >= 2 {
        let first = bytes[0];
        let last = bytes[bytes.len() - 1];
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return &trimmed[1..trimmed.len() - 1];
        }
    }
    trimmed
}

// ---- Legacy compatibility ----
//
// Older tests still reach into the handler module to call `create_secret`
// directly with an `AuthUser` extractor instead of the new `AuthActor`.
// The signature change above makes those tests fail to compile until they
// migrate. We expose a thin compatibility helper here so external callers
// have a stable migration path.

/// Result of validating an `AuthUser`'s ability to act on a scope. The
/// historical handlers used the user's id as the scope; new code should
/// pass a real scope explicitly.
#[doc(hidden)]
#[must_use]
pub fn legacy_user_scope(user: &AuthUser) -> String {
    user.id().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{InMemoryEnvironmentStore, StoredEnvironment};

    #[test]
    fn test_secret_metadata_response_from() {
        let metadata = SecretMetadata {
            name: "test-secret".to_string(),
            created_at: 1_234_567_890,
            updated_at: 1_234_567_900,
            version: 3,
        };

        let response = SecretMetadataResponse::from(metadata);

        assert_eq!(response.name, "test-secret");
        assert_eq!(response.created_at, 1_234_567_890);
        assert_eq!(response.updated_at, 1_234_567_900);
        assert_eq!(response.version, 3);
        assert!(response.value.is_none());
    }

    #[test]
    fn test_create_secret_request_deserialize() {
        let json = r#"{"name": "api-key", "value": "secret-value-123"}"#;
        let request: CreateSecretRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.name, "api-key");
        assert_eq!(request.value, "secret-value-123");
        assert!(request.scope.is_none());
    }

    #[test]
    fn test_create_secret_request_with_scope() {
        let json = r#"{"name":"k","value":"v","scope":"proj:foo"}"#;
        let request: CreateSecretRequest = serde_json::from_str(json).unwrap();
        assert_eq!(request.scope.as_deref(), Some("proj:foo"));
    }

    #[test]
    fn test_secret_metadata_response_serialize_omits_value_when_none() {
        let response = SecretMetadataResponse {
            name: "db-password".to_string(),
            created_at: 1000,
            updated_at: 2000,
            version: 5,
            value: None,
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("db-password"));
        assert!(json.contains("1000"));
        assert!(json.contains("2000"));
        assert!(json.contains('5'));
        // Ensure the value key is NOT in the response when missing
        assert!(!json.contains("\"value\""));
    }

    #[test]
    fn test_secret_metadata_response_serialize_includes_value_when_some() {
        let response = SecretMetadataResponse {
            name: "k".to_string(),
            created_at: 0,
            updated_at: 0,
            version: 1,
            value: Some("plaintext".to_string()),
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"value\":\"plaintext\""));
    }

    #[test]
    fn test_env_scope_global() {
        let env = StoredEnvironment::new("dev", None);
        let scope = env_scope(&env);
        assert_eq!(scope, format!("env:{}", env.id));
    }

    #[test]
    fn test_env_scope_project() {
        let env = StoredEnvironment::new("staging", Some("proj-1".to_string()));
        let scope = env_scope(&env);
        assert_eq!(scope, format!("project:proj-1:env:{}", env.id));
    }

    #[test]
    fn test_scope_query_rejects_both() {
        let q = SecretsScopeQuery {
            environment: Some("e".to_string()),
            scope: Some("s".to_string()),
        };
        let err = q.validate_exclusive().unwrap_err();
        assert!(matches!(err, ApiError::BadRequest(_)));
    }

    #[test]
    fn test_scope_query_allows_either() {
        SecretsScopeQuery {
            environment: Some("e".to_string()),
            scope: None,
        }
        .validate_exclusive()
        .unwrap();
        SecretsScopeQuery {
            environment: None,
            scope: Some("s".to_string()),
        }
        .validate_exclusive()
        .unwrap();
        SecretsScopeQuery::default().validate_exclusive().unwrap();
    }

    #[test]
    fn test_get_query_rejects_both() {
        let q = GetSecretQuery {
            environment: Some("e".to_string()),
            scope: Some("s".to_string()),
            reveal: false,
        };
        assert!(matches!(
            q.validate_exclusive().unwrap_err(),
            ApiError::BadRequest(_)
        ));
    }

    #[tokio::test]
    async fn test_resolve_scope_default_when_nothing() {
        let secrets_store: Arc<dyn SecretsStore + Send + Sync> = Arc::new(MockSecretsStore::new());
        let state = SecretsState::new(secrets_store);
        let scope = resolve_scope(&state, None, &SecretsScopeQuery::default())
            .await
            .unwrap();
        assert_eq!(scope, DEFAULT_SCOPE);
    }

    #[tokio::test]
    async fn test_resolve_scope_body_wins_over_query_scope() {
        let secrets_store: Arc<dyn SecretsStore + Send + Sync> = Arc::new(MockSecretsStore::new());
        let state = SecretsState::new(secrets_store);
        let q = SecretsScopeQuery {
            environment: None,
            scope: Some("from-query".to_string()),
        };
        let scope = resolve_scope(&state, Some("from-body"), &q).await.unwrap();
        assert_eq!(scope, "from-body");
    }

    #[tokio::test]
    async fn test_resolve_scope_environment_resolves_to_env_scope() {
        let secrets_store: Arc<dyn SecretsStore + Send + Sync> = Arc::new(MockSecretsStore::new());
        let env_store = Arc::new(InMemoryEnvironmentStore::new());
        let env = StoredEnvironment::new("dev", None);
        let env_id = env.id.clone();
        env_store.store(&env).await.unwrap();

        let state = SecretsState::with_environments(secrets_store, env_store);
        let q = SecretsScopeQuery {
            environment: Some(env_id.clone()),
            scope: None,
        };
        let scope = resolve_scope(&state, None, &q).await.unwrap();
        assert_eq!(scope, format!("env:{env_id}"));
    }

    #[tokio::test]
    async fn test_resolve_scope_unknown_env_is_404() {
        let secrets_store: Arc<dyn SecretsStore + Send + Sync> = Arc::new(MockSecretsStore::new());
        let env_store = Arc::new(InMemoryEnvironmentStore::new());
        let state = SecretsState::with_environments(secrets_store, env_store);
        let q = SecretsScopeQuery {
            environment: Some("nonexistent".to_string()),
            scope: None,
        };
        let err = resolve_scope(&state, None, &q).await.unwrap_err();
        assert!(matches!(err, ApiError::NotFound(_)));
    }

    #[tokio::test]
    async fn test_resolve_scope_environment_without_env_store_is_internal() {
        let secrets_store: Arc<dyn SecretsStore + Send + Sync> = Arc::new(MockSecretsStore::new());
        let state = SecretsState::new(secrets_store);
        let q = SecretsScopeQuery {
            environment: Some("e".to_string()),
            scope: None,
        };
        let err = resolve_scope(&state, None, &q).await.unwrap_err();
        assert!(matches!(err, ApiError::Internal(_)));
    }

    #[tokio::test]
    async fn test_resolve_scope_rejects_body_plus_environment() {
        let secrets_store: Arc<dyn SecretsStore + Send + Sync> = Arc::new(MockSecretsStore::new());
        let env_store = Arc::new(InMemoryEnvironmentStore::new());
        let state = SecretsState::with_environments(secrets_store, env_store);
        let q = SecretsScopeQuery {
            environment: Some("e".to_string()),
            scope: None,
        };
        let err = resolve_scope(&state, Some("from-body"), &q)
            .await
            .unwrap_err();
        assert!(matches!(err, ApiError::BadRequest(_)));
    }

    #[test]
    fn test_strip_dotenv_quotes_double() {
        assert_eq!(strip_dotenv_quotes("\"hello\""), "hello");
    }

    #[test]
    fn test_strip_dotenv_quotes_single() {
        assert_eq!(strip_dotenv_quotes("'hello world'"), "hello world");
    }

    #[test]
    fn test_strip_dotenv_quotes_unquoted() {
        assert_eq!(strip_dotenv_quotes("plain"), "plain");
    }

    #[test]
    fn test_strip_dotenv_quotes_mismatched() {
        assert_eq!(strip_dotenv_quotes("\"hello'"), "\"hello'");
    }

    #[test]
    fn test_strip_dotenv_quotes_trims_outside() {
        assert_eq!(strip_dotenv_quotes("   value   "), "value");
        assert_eq!(strip_dotenv_quotes("  \"v\"  "), "v");
    }

    // ---- minimal mock secrets store for unit tests above ----

    use std::collections::HashMap;
    use tokio::sync::Mutex;
    use zlayer_secrets::{Result as SecretsResult, SecretsError, SecretsProvider};

    struct MockSecretsStore {
        inner: Mutex<HashMap<(String, String), Secret>>,
    }

    impl MockSecretsStore {
        fn new() -> Self {
            Self {
                inner: Mutex::new(HashMap::new()),
            }
        }
    }

    #[async_trait::async_trait]
    impl SecretsProvider for MockSecretsStore {
        async fn get_secret(&self, scope: &str, name: &str) -> SecretsResult<Secret> {
            self.inner
                .lock()
                .await
                .get(&(scope.to_string(), name.to_string()))
                .cloned()
                .ok_or_else(|| SecretsError::NotFound {
                    name: format!("{scope}/{name}"),
                })
        }

        async fn get_secrets(
            &self,
            scope: &str,
            names: &[&str],
        ) -> SecretsResult<HashMap<String, Secret>> {
            let inner = self.inner.lock().await;
            let mut out = HashMap::new();
            for name in names {
                if let Some(s) = inner.get(&(scope.to_string(), (*name).to_string())) {
                    out.insert((*name).to_string(), s.clone());
                }
            }
            Ok(out)
        }

        async fn list_secrets(&self, scope: &str) -> SecretsResult<Vec<SecretMetadata>> {
            Ok(self
                .inner
                .lock()
                .await
                .keys()
                .filter(|(s, _)| s == scope)
                .map(|(_, n)| SecretMetadata {
                    name: n.clone(),
                    created_at: 0,
                    updated_at: 0,
                    version: 1,
                })
                .collect())
        }

        async fn exists(&self, scope: &str, name: &str) -> SecretsResult<bool> {
            Ok(self
                .inner
                .lock()
                .await
                .contains_key(&(scope.to_string(), name.to_string())))
        }
    }

    #[async_trait::async_trait]
    impl SecretsStore for MockSecretsStore {
        async fn set_secret(&self, scope: &str, name: &str, value: &Secret) -> SecretsResult<()> {
            self.inner
                .lock()
                .await
                .insert((scope.to_string(), name.to_string()), value.clone());
            Ok(())
        }

        async fn delete_secret(&self, scope: &str, name: &str) -> SecretsResult<()> {
            self.inner
                .lock()
                .await
                .remove(&(scope.to_string(), name.to_string()));
            Ok(())
        }
    }
}
