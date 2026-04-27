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
pub use zlayer_types::api::secrets::*;

use crate::auth::AuthUser;
use crate::error::{ApiError, Result};
use crate::handlers::users::AuthActor;
use crate::storage::{EnvironmentStorage, PermissionLevel, PermissionStorage, StoredEnvironment};
use zlayer_secrets::{RotationResult, Secret, SecretMetadata, SecretsStore};

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

/// Build a [`SecretMetadataResponse`] DTO from an owned [`SecretMetadata`].
///
/// Free function (rather than `impl From<SecretMetadata> for SecretMetadataResponse`)
/// because both types are foreign to this crate, which would violate the
/// orphan rule.
#[must_use]
pub fn secret_metadata_response_from_owned(metadata: SecretMetadata) -> SecretMetadataResponse {
    SecretMetadataResponse {
        name: metadata.name,
        created_at: metadata.created_at,
        updated_at: metadata.updated_at,
        version: metadata.version,
        value: None,
    }
}

/// Build a [`SecretMetadataResponse`] DTO from a borrowed [`SecretMetadata`].
///
/// Free function (rather than `impl From<&SecretMetadata> for SecretMetadataResponse`)
/// because both types are foreign to this crate, which would violate the
/// orphan rule.
#[must_use]
pub fn secret_metadata_response_from(metadata: &SecretMetadata) -> SecretMetadataResponse {
    SecretMetadataResponse {
        name: metadata.name.clone(),
        created_at: metadata.created_at,
        updated_at: metadata.updated_at,
        version: metadata.version,
        value: None,
    }
}

/// Build a [`RotateSecretResponse`] DTO from a name + [`RotationResult`].
///
/// Free function (rather than `impl From<(String, RotationResult)> for RotateSecretResponse`)
/// because both types are foreign to this crate, which would violate the
/// orphan rule.
#[must_use]
pub fn rotate_secret_response_from(name: String, r: &RotationResult) -> RotateSecretResponse {
    RotateSecretResponse {
        name,
        previous_version: r.previous_version,
        new_version: r.new_version,
    }
}

/// State for secrets endpoints.
#[derive(Clone)]
pub struct SecretsState {
    /// Secrets store for CRUD operations.
    pub store: Arc<dyn SecretsStore + Send + Sync>,
    /// Optional environment store for env-aware routing. Without it, only
    /// the legacy `scope`-based code paths are usable.
    pub env_store: Option<Arc<dyn EnvironmentStorage>>,
    /// Permission store for per-env RBAC checks. When `None`, all endpoints
    /// fall back to blanket admin-only gates (used by tests or legacy setups
    /// without the permission store wired up yet).
    pub perm_store: Option<Arc<dyn PermissionStorage>>,
}

impl SecretsState {
    /// Create a new secrets state with the given store and no environment
    /// support (legacy paths only).
    #[must_use]
    pub fn new(store: Arc<dyn SecretsStore + Send + Sync>) -> Self {
        Self {
            store,
            env_store: None,
            perm_store: None,
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
            perm_store: None,
        }
    }

    /// Full wiring: secrets + env store + permission store. Enables per-env RBAC.
    #[must_use]
    pub fn with_rbac(
        store: Arc<dyn SecretsStore + Send + Sync>,
        env_store: Arc<dyn EnvironmentStorage>,
        perm_store: Arc<dyn PermissionStorage>,
    ) -> Self {
        Self {
            store,
            env_store: Some(env_store),
            perm_store: Some(perm_store),
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

// ---- Query helpers ----

/// Reject scope queries that supply both `environment` and `scope`.
fn validate_scope_exclusive(q: &SecretsScopeQuery) -> Result<()> {
    if q.environment.is_some() && q.scope.is_some() {
        return Err(ApiError::BadRequest(
            "Pass either ?environment= or ?scope=, not both".to_string(),
        ));
    }
    Ok(())
}

/// Reject get queries that supply both `environment` and `scope`.
fn validate_get_exclusive(q: &GetSecretQuery) -> Result<()> {
    if q.environment.is_some() && q.scope.is_some() {
        return Err(ApiError::BadRequest(
            "Pass either ?environment= or ?scope=, not both".to_string(),
        ));
    }
    Ok(())
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
    validate_scope_exclusive(query)?;
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
    validate_get_exclusive(query)?;
    if let Some(env_id) = query.environment.as_deref() {
        let env = state.lookup_env(env_id).await?;
        return Ok(env_scope(&env));
    }
    if let Some(scope) = query.scope.as_deref() {
        return Ok(scope.to_string());
    }
    Ok(DEFAULT_SCOPE.to_string())
}

// ---- RBAC helpers ----

/// Gate a mutating env-scoped operation: require `Write` on the env.
/// Admin short-circuits via [`AuthActor::require_env_access`].
///
/// Falls back to [`AuthActor::require_admin`] when `perm_store` is not wired
/// (legacy setups).
async fn require_env_write(state: &SecretsState, actor: &AuthActor, env_id: &str) -> Result<()> {
    match state.perm_store.as_ref() {
        Some(ps) => {
            actor
                .require_env_access(ps.as_ref(), env_id, PermissionLevel::Write)
                .await
        }
        None => actor.require_admin(),
    }
}

/// Gate a read-only env-scoped operation: require `Read` on the env.
/// Admin short-circuits via [`AuthActor::require_env_access`].
///
/// Falls back to [`AuthActor::require_admin`] when `perm_store` is not wired.
async fn require_env_read(state: &SecretsState, actor: &AuthActor, env_id: &str) -> Result<()> {
    match state.perm_store.as_ref() {
        Some(ps) => {
            actor
                .require_env_access(ps.as_ref(), env_id, PermissionLevel::Read)
                .await
        }
        None => actor.require_admin(),
    }
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
    if let Some(env_id) = &query.environment {
        require_env_write(&state, &actor, env_id).await?;
    } else {
        // Legacy scope path stays admin-only.
        actor.require_admin()?;
    }

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

    Ok((status, Json(secret_metadata_response_from_owned(metadata))))
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
    actor: AuthActor,
    State(state): State<SecretsState>,
    Query(query): Query<SecretsScopeQuery>,
) -> Result<Json<Vec<SecretMetadataResponse>>> {
    if let Some(env_id) = &query.environment {
        require_env_read(&state, &actor, env_id).await?;
    }
    // else: authenticated actor can list legacy scope (unchanged).

    let scope = resolve_scope(&state, None, &query).await?;

    let metadata_list = state
        .store
        .list_secrets(&scope)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to list secrets: {e}")))?;

    let response: Vec<SecretMetadataResponse> = metadata_list
        .iter()
        .map(secret_metadata_response_from)
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
    if let Some(env_id) = &query.environment {
        if reveal {
            // Design choice: reveal on an env-scoped secret requires Write —
            // plaintext exfil is a more sensitive op than simply listing/
            // reading metadata.
            require_env_write(&state, &actor, env_id).await?;
        } else {
            require_env_read(&state, &actor, env_id).await?;
        }
    } else if reveal {
        // Legacy scope reveal stays admin-only.
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

    let mut response = secret_metadata_response_from_owned(metadata);

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
    if let Some(env_id) = &query.environment {
        require_env_write(&state, &actor, env_id).await?;
    } else {
        actor.require_admin()?;
    }

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

/// Rotate a secret — overwrite with a new value and return the version before+after.
///
/// Admin-only in v1. Mutually exclusive scope query like the other endpoints.
///
/// # Errors
///
/// Returns `ApiError::BadRequest` for empty names or conflicting scope params,
/// `ApiError::Forbidden` for non-admin callers, `ApiError::NotFound` when the
/// secret or environment is unknown, and `ApiError::Internal` for storage
/// failures.
#[utoipa::path(
    post,
    path = "/api/v1/secrets/{name}/rotate",
    params(
        ("name" = String, Path, description = "Secret name"),
        ("environment" = Option<String>, Query, description = "Environment id"),
        ("scope" = Option<String>, Query, description = "Explicit scope (legacy)"),
    ),
    request_body = RotateSecretRequest,
    responses(
        (status = 200, description = "Secret rotated", body = RotateSecretResponse),
        (status = 400, description = "Invalid request"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden"),
        (status = 404, description = "Secret or environment not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "Secrets"
)]
pub async fn rotate_secret(
    actor: AuthActor,
    State(state): State<SecretsState>,
    Path(name): Path<String>,
    Query(query): Query<SecretsScopeQuery>,
    Json(request): Json<RotateSecretRequest>,
) -> Result<Json<RotateSecretResponse>> {
    if let Some(env_id) = &query.environment {
        require_env_write(&state, &actor, env_id).await?;
    } else {
        actor.require_admin()?;
    }

    if name.is_empty() {
        return Err(ApiError::BadRequest(
            "Secret name cannot be empty".to_string(),
        ));
    }

    let scope = resolve_scope(&state, None, &query).await?;
    let new_secret = Secret::new(&request.value);

    let result = state
        .store
        .rotate_secret(&scope, &name, &new_secret)
        .await
        .map_err(|e| match e {
            zlayer_secrets::SecretsError::NotFound { .. } => {
                ApiError::NotFound(format!("Secret '{name}' not found"))
            }
            other => ApiError::Internal(format!("Failed to rotate secret: {other}")),
        })?;

    Ok(Json(rotate_secret_response_from(name, &result)))
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
    // `environment` is required on bulk-import, so always route through
    // per-env RBAC. Admin short-circuits inside `require_env_write`.
    require_env_write(&state, &actor, &query.environment).await?;

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

/// Reveal every secret in an environment at once (admin only).
///
/// Used by `zlayer run` to build the child-process env in a single round-trip.
///
/// # Errors
///
/// Returns `ApiError::Forbidden` if the caller is not admin, `ApiError::NotFound`
/// if the environment is unknown, and `ApiError::Internal` for storage failures.
#[utoipa::path(
    get,
    path = "/api/v1/secrets/reveal-all",
    params(
        ("environment" = String, Query, description = "Environment id (required)"),
    ),
    responses(
        (status = 200, description = "Every secret revealed", body = RevealAllSecretsResponse),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Admin required"),
        (status = 404, description = "Environment not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "Secrets"
)]
pub async fn reveal_all_secrets(
    actor: AuthActor,
    State(state): State<SecretsState>,
    Query(query): Query<SecretsScopeQuery>,
) -> Result<Json<RevealAllSecretsResponse>> {
    let env_id = query.environment.clone().ok_or_else(|| {
        ApiError::BadRequest("`?environment=` is required for reveal-all".to_string())
    })?;
    require_env_read(&state, &actor, &env_id).await?;

    // Resolve env -> scope string
    let scope = resolve_scope(&state, None, &query).await?;

    let metadata_list = state
        .store
        .list_secrets(&scope)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to list secrets: {e}")))?;

    let mut secrets = std::collections::HashMap::new();
    for m in metadata_list {
        let v =
            state.store.get_secret(&scope, &m.name).await.map_err(|e| {
                ApiError::Internal(format!("Failed to read secret {}: {e}", m.name))
            })?;
        secrets.insert(m.name, v.expose().to_string());
    }

    Ok(Json(RevealAllSecretsResponse {
        environment: env_id,
        secrets,
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

        let response = secret_metadata_response_from_owned(metadata);

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
        let err = validate_scope_exclusive(&q).unwrap_err();
        assert!(matches!(err, ApiError::BadRequest(_)));
    }

    #[test]
    fn test_scope_query_allows_either() {
        validate_scope_exclusive(&SecretsScopeQuery {
            environment: Some("e".to_string()),
            scope: None,
        })
        .unwrap();
        validate_scope_exclusive(&SecretsScopeQuery {
            environment: None,
            scope: Some("s".to_string()),
        })
        .unwrap();
        validate_scope_exclusive(&SecretsScopeQuery::default()).unwrap();
    }

    #[test]
    fn test_get_query_rejects_both() {
        let q = GetSecretQuery {
            environment: Some("e".to_string()),
            scope: Some("s".to_string()),
            reveal: false,
        };
        assert!(matches!(
            validate_get_exclusive(&q).unwrap_err(),
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

    // ---- per-env RBAC tests ----

    #[tokio::test]
    async fn test_list_secrets_env_read_grant_succeeds() {
        use crate::storage::{
            InMemoryPermissionStore, PermissionLevel, PermissionStorage, StoredPermission,
            SubjectKind,
        };
        use axum::extract::{Query, State};

        // Build state: secrets + env + perm stores.
        let secrets_store: Arc<dyn SecretsStore + Send + Sync> = Arc::new(MockSecretsStore::new());
        let env_store = Arc::new(InMemoryEnvironmentStore::new());
        let env = StoredEnvironment::new("dev", None);
        let env_id = env.id.clone();
        env_store.store(&env).await.unwrap();

        let perm_store = Arc::new(InMemoryPermissionStore::new());

        // Grant user u1 Read on env dev.
        let grant = StoredPermission::new(
            SubjectKind::User,
            "u1",
            "environment",
            Some(env_id.clone()),
            PermissionLevel::Read,
        );
        <InMemoryPermissionStore as PermissionStorage>::grant(perm_store.as_ref(), &grant)
            .await
            .unwrap();

        let state = SecretsState::with_rbac(secrets_store, env_store, perm_store);

        let actor = AuthActor {
            user_id: "u1".into(),
            roles: vec!["user".into()],
            email: None,
        };
        let query = SecretsScopeQuery {
            environment: Some(env_id),
            scope: None,
        };

        let result = list_secrets(actor, State(state), Query(query)).await;
        assert!(
            result.is_ok(),
            "list_secrets with Read grant should succeed, got {result:?}"
        );
        let Json(list) = result.unwrap();
        assert!(list.is_empty(), "fresh env should have no secrets");
    }

    #[tokio::test]
    async fn test_create_secret_env_without_write_grant_is_forbidden() {
        use crate::storage::{
            InMemoryPermissionStore, PermissionLevel, PermissionStorage, StoredPermission,
            SubjectKind,
        };
        use axum::extract::{Query, State};

        let secrets_store: Arc<dyn SecretsStore + Send + Sync> = Arc::new(MockSecretsStore::new());
        let env_store = Arc::new(InMemoryEnvironmentStore::new());
        let env = StoredEnvironment::new("dev", None);
        let env_id = env.id.clone();
        env_store.store(&env).await.unwrap();

        let perm_store = Arc::new(InMemoryPermissionStore::new());

        // Grant Read only — not Write.
        let grant = StoredPermission::new(
            SubjectKind::User,
            "u1",
            "environment",
            Some(env_id.clone()),
            PermissionLevel::Read,
        );
        <InMemoryPermissionStore as PermissionStorage>::grant(perm_store.as_ref(), &grant)
            .await
            .unwrap();

        let state = SecretsState::with_rbac(secrets_store, env_store, perm_store);

        let actor = AuthActor {
            user_id: "u1".into(),
            roles: vec!["user".into()],
            email: None,
        };
        let query = SecretsScopeQuery {
            environment: Some(env_id),
            scope: None,
        };
        let body = CreateSecretRequest {
            name: "my-secret".into(),
            value: "hunter2".into(),
            scope: None,
        };

        let err = create_secret(actor, State(state), Query(query), Json(body))
            .await
            .expect_err("create_secret without Write grant should return Forbidden");
        assert!(
            matches!(err, ApiError::Forbidden(_)),
            "expected Forbidden, got {err:?}"
        );
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
