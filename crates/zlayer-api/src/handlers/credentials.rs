//! Credential management endpoints for registry and git credentials.
//!
//! Provides REST handlers for creating, listing, and deleting registry
//! (Docker/OCI) and git (PAT/SSH) credentials. Secrets are stored encrypted
//! via the underlying `zlayer-secrets` credential stores; only metadata is
//! returned in list/create responses.
//!
//! All mutating endpoints require the `admin` role. Read-only list endpoints
//! accept any authenticated actor.

use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use base64::Engine;

pub use zlayer_types::api::credentials::*;

use crate::error::{ApiError, Result};
use crate::handlers::users::AuthActor;

/// State for credential endpoints.
#[derive(Clone)]
pub struct CredentialState {
    /// Registry credential store.
    pub registry_store:
        Arc<zlayer_secrets::RegistryCredentialStore<Arc<zlayer_secrets::PersistentSecretsStore>>>,
    /// Git credential store.
    pub git_store:
        Arc<zlayer_secrets::GitCredentialStore<Arc<zlayer_secrets::PersistentSecretsStore>>>,
}

impl CredentialState {
    /// Build a new credential state from both stores.
    #[must_use]
    pub fn new(
        registry_store: Arc<
            zlayer_secrets::RegistryCredentialStore<Arc<zlayer_secrets::PersistentSecretsStore>>,
        >,
        git_store: Arc<
            zlayer_secrets::GitCredentialStore<Arc<zlayer_secrets::PersistentSecretsStore>>,
        >,
    ) -> Self {
        Self {
            registry_store,
            git_store,
        }
    }
}

// ---- Conversions from `zlayer_secrets` records to wire DTOs ----

/// Build a [`RegistryCredentialResponse`] DTO from a stored
/// [`zlayer_secrets::RegistryCredential`].
///
/// Free function (rather than
/// `impl From<zlayer_secrets::RegistryCredential> for RegistryCredentialResponse`)
/// because both types are foreign to this crate, which would violate the
/// orphan rule.
#[must_use]
pub fn registry_credential_response_from(
    c: zlayer_secrets::RegistryCredential,
) -> RegistryCredentialResponse {
    RegistryCredentialResponse {
        id: c.id,
        registry: c.registry,
        username: c.username,
        auth_type: match c.auth_type {
            zlayer_secrets::RegistryAuthType::Basic => RegistryAuthTypeSchema::Basic,
            zlayer_secrets::RegistryAuthType::Token => RegistryAuthTypeSchema::Token,
        },
    }
}

/// Build a [`GitCredentialResponse`] DTO from a stored
/// [`zlayer_secrets::GitCredential`].
///
/// Free function (rather than
/// `impl From<zlayer_secrets::GitCredential> for GitCredentialResponse`) because
/// both types are foreign to this crate, which would violate the orphan rule.
#[must_use]
pub fn git_credential_response_from(c: zlayer_secrets::GitCredential) -> GitCredentialResponse {
    GitCredentialResponse {
        id: c.id,
        name: c.name,
        kind: match c.kind {
            zlayer_secrets::GitCredentialKind::Pat => GitCredentialKindSchema::Pat,
            zlayer_secrets::GitCredentialKind::SshKey => GitCredentialKindSchema::SshKey,
        },
    }
}

// ---- Docker config.json materialization ----
//
// ZLayer's runtimes resolve pull auth via `zlayer_core::AuthResolver`, whose
// default `AuthConfig` reads `~/.docker/config.json` (see
// `zlayer_core::auth::docker_config::DockerConfigAuth`). So after every mutation
// of the registry credential store we (re)write that file — exactly like
// `docker login` does — so image pulls in this daemon process pick the creds up.

/// Concrete registry credential store type used by [`CredentialState`].
type RegStore =
    zlayer_secrets::RegistryCredentialStore<Arc<zlayer_secrets::PersistentSecretsStore>>;

/// Resolve the Docker config.json path the same way `DockerConfigAuth` reads it:
/// `$DOCKER_CONFIG/config.json` if set, else `~/.docker/config.json`.
fn docker_config_path() -> Option<PathBuf> {
    if let Ok(config_dir) = std::env::var("DOCKER_CONFIG") {
        return Some(PathBuf::from(config_dir).join("config.json"));
    }
    dirs::home_dir().map(|h| h.join(".docker").join("config.json"))
}

/// Normalize a registry hostname to the key `DockerConfigAuth` reads back.
///
/// Docker Hub's canonical config key is `https://index.docker.io/v1/`; all other
/// registries are stored under their bare hostname.
fn normalize_registry(registry: &str) -> String {
    match registry {
        "docker.io"
        | "registry-1.docker.io"
        | "index.docker.io"
        | "https://index.docker.io/v1/"
        | "https://registry-1.docker.io" => "https://index.docker.io/v1/".to_string(),
        other => other.to_string(),
    }
}

/// Build the `{ "auth": base64("user:pass") }` entry for an auth pair.
fn auth_entry(username: &str, password: &str) -> serde_json::Value {
    let token = base64::engine::general_purpose::STANDARD.encode(format!("{username}:{password}"));
    serde_json::json!({ "auth": token })
}

/// Merge the current registry credential set into a docker-config JSON value,
/// preserving every unrelated top-level key and any `auths` entries for hosts we
/// don't touch (e.g. written by a real `docker login`).
///
/// - `creds` is the current `(registry, username, password)` set from the store;
///   each is normalized and upserted into `auths`.
/// - `remove_hosts` is a set of ALREADY-NORMALIZED hosts to drop from `auths`
///   before re-adding `creds`. This is how a delete removes a host that is no
///   longer present in `creds`: the handler normalizes the deleted registry and
///   passes it here. (On create, `remove_hosts` is empty; the upsert alone is
///   enough since the host is still in `creds`.)
///
/// Foreign `auths` entries (hosts neither in `creds` nor `remove_hosts`) always
/// survive, as do all non-`auths` top-level keys.
fn merge_docker_config(
    mut existing: serde_json::Value,
    creds: &[(String, String, String)],
    remove_hosts: &std::collections::HashSet<String>,
) -> serde_json::Value {
    if !existing.is_object() {
        existing = serde_json::json!({});
    }
    let obj = existing
        .as_object_mut()
        .expect("existing is an object after the guard above");

    // Ensure `auths` exists and is an object.
    let auths_is_object = obj.get("auths").is_some_and(serde_json::Value::is_object);
    if !auths_is_object {
        obj.insert("auths".to_string(), serde_json::json!({}));
    }
    let auths = obj
        .get_mut("auths")
        .and_then(serde_json::Value::as_object_mut)
        .expect("auths is an object after the guard above");

    // Drop explicitly-removed hosts (deleted creds) AND any host we're about to
    // re-add (so the upsert replaces a stale entry cleanly).
    let to_add: std::collections::HashSet<String> = creds
        .iter()
        .map(|(registry, _, _)| normalize_registry(registry))
        .collect();
    auths.retain(|host, _| !remove_hosts.contains(host) && !to_add.contains(host));

    // Re-add the current set.
    for (registry, username, password) in creds {
        auths.insert(normalize_registry(registry), auth_entry(username, password));
    }

    existing
}

/// Re-materialize the registry credential set to `~/.docker/config.json`.
///
/// Reads the full current set from `store` and upserts it; `removed_registry` (a
/// raw, un-normalized hostname) is additionally dropped from `auths` — that's how
/// a delete removes a host that is no longer present in the store. Pass `None` on
/// create.
///
/// Merge-safe: preserves unrelated top-level keys and foreign `auths` entries.
/// Never fails the request — logs a warning if the file cannot be written, since
/// the secrets store write already succeeded and is the source of truth.
async fn sync_docker_config(store: &RegStore, removed_registry: Option<&str>) {
    let Some(path) = docker_config_path() else {
        tracing::warn!("Cannot determine Docker config path; skipping docker-config sync");
        return;
    };

    // Gather (registry, username, password) for every stored credential.
    let creds = match store.list().await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "Failed to list registry credentials for docker-config sync");
            return;
        }
    };
    let mut entries: Vec<(String, String, String)> = Vec::with_capacity(creds.len());
    for cred in creds {
        match store.get_password(&cred.id).await {
            Ok(pw) => entries.push((cred.registry, cred.username, pw.expose().to_string())),
            Err(e) => {
                tracing::warn!(id = %cred.id, error = %e, "Failed to read registry password for docker-config sync");
            }
        }
    }

    let remove_hosts: std::collections::HashSet<String> = removed_registry
        .map(normalize_registry)
        .into_iter()
        .collect();

    if let Err(e) = write_docker_config(&path, &entries, &remove_hosts) {
        tracing::warn!(path = %path.display(), error = %e, "Failed to write docker config; pulls may not see updated credentials");
    }
}

/// Read-merge-write the docker config at `path` (pure I/O over [`merge_docker_config`]).
fn write_docker_config(
    path: &std::path::Path,
    creds: &[(String, String, String)],
    remove_hosts: &std::collections::HashSet<String>,
) -> std::io::Result<()> {
    let existing: serde_json::Value = if path.exists() {
        let contents = std::fs::read_to_string(path)?;
        serde_json::from_str(&contents).unwrap_or_else(|_| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    let merged = merge_docker_config(existing, creds, remove_hosts);

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let serialized = serde_json::to_string_pretty(&merged)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(path, serialized)
}

// ---- Endpoints: Registry credentials ----

/// List all registry credentials (metadata only, no passwords).
///
/// # Errors
///
/// Returns [`ApiError::Internal`] if the credential store fails.
#[utoipa::path(
    get,
    path = "/api/v1/credentials/registry",
    responses(
        (status = 200, description = "List of registry credentials", body = Vec<RegistryCredentialResponse>),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "Credentials"
)]
pub async fn list_registry_credentials(
    _actor: AuthActor,
    State(state): State<CredentialState>,
) -> Result<Json<Vec<RegistryCredentialResponse>>> {
    let creds = state
        .registry_store
        .list()
        .await
        .map_err(|e| ApiError::Internal(format!("Registry credential store: {e}")))?;
    Ok(Json(
        creds
            .into_iter()
            .map(registry_credential_response_from)
            .collect(),
    ))
}

/// Create a new registry credential. Admin only.
///
/// # Errors
///
/// Returns [`ApiError::Forbidden`] for non-admins,
/// [`ApiError::BadRequest`] for missing fields, or
/// [`ApiError::Internal`] when the store fails.
#[utoipa::path(
    post,
    path = "/api/v1/credentials/registry",
    request_body = CreateRegistryCredentialRequest,
    responses(
        (status = 201, description = "Registry credential created", body = RegistryCredentialResponse),
        (status = 400, description = "Invalid request"),
        (status = 403, description = "Admin role required"),
    ),
    security(("bearer_auth" = [])),
    tag = "Credentials"
)]
pub async fn create_registry_credential(
    actor: AuthActor,
    State(state): State<CredentialState>,
    Json(req): Json<CreateRegistryCredentialRequest>,
) -> Result<(StatusCode, Json<RegistryCredentialResponse>)> {
    actor.require_admin()?;

    if req.registry.trim().is_empty() {
        return Err(ApiError::BadRequest(
            "Registry hostname is required".to_string(),
        ));
    }
    if req.username.trim().is_empty() {
        return Err(ApiError::BadRequest("Username is required".to_string()));
    }
    if req.password.is_empty() {
        return Err(ApiError::BadRequest("Password is required".to_string()));
    }

    let auth_type = match req.auth_type {
        RegistryAuthTypeSchema::Basic => zlayer_secrets::RegistryAuthType::Basic,
        RegistryAuthTypeSchema::Token => zlayer_secrets::RegistryAuthType::Token,
    };

    let cred = state
        .registry_store
        .create(&req.registry, &req.username, &req.password, auth_type)
        .await
        .map_err(|e| ApiError::Internal(format!("Registry credential store: {e}")))?;

    // Re-materialize ~/.docker/config.json so image pulls consume the new cred.
    sync_docker_config(&state.registry_store, None).await;

    Ok((
        StatusCode::CREATED,
        Json(registry_credential_response_from(cred)),
    ))
}

/// Delete a registry credential. Admin only.
///
/// # Errors
///
/// Returns [`ApiError::Forbidden`] for non-admins,
/// [`ApiError::NotFound`] when the credential does not exist, or
/// [`ApiError::Internal`] when the store fails.
#[utoipa::path(
    delete,
    path = "/api/v1/credentials/registry/{id}",
    params(("id" = String, Path, description = "Registry credential id")),
    responses(
        (status = 204, description = "Registry credential deleted"),
        (status = 403, description = "Admin role required"),
        (status = 404, description = "Not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "Credentials"
)]
pub async fn delete_registry_credential(
    actor: AuthActor,
    State(state): State<CredentialState>,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    actor.require_admin()?;

    // Capture the registry hostname BEFORE deletion so we can drop its docker-config
    // entry afterwards (the host won't be in the store's list once deleted).
    let removed_registry = state
        .registry_store
        .get(&id)
        .await
        .ok()
        .flatten()
        .map(|c| c.registry);

    state.registry_store.delete(&id).await.map_err(|e| {
        if e.to_string().contains("not found") || e.to_string().contains("NotFound") {
            ApiError::NotFound(format!("Registry credential {id} not found"))
        } else {
            ApiError::Internal(format!("Registry credential store: {e}"))
        }
    })?;

    // Rebuild ~/.docker/config.json from the current store, additionally dropping
    // the just-deleted host (foreign / non-`auths` keys are preserved).
    sync_docker_config(&state.registry_store, removed_registry.as_deref()).await;

    Ok(StatusCode::NO_CONTENT)
}

// ---- Endpoints: Git credentials ----

/// List all git credentials (metadata only, no secret values).
///
/// # Errors
///
/// Returns [`ApiError::Internal`] if the credential store fails.
#[utoipa::path(
    get,
    path = "/api/v1/credentials/git",
    responses(
        (status = 200, description = "List of git credentials", body = Vec<GitCredentialResponse>),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = [])),
    tag = "Credentials"
)]
pub async fn list_git_credentials(
    _actor: AuthActor,
    State(state): State<CredentialState>,
) -> Result<Json<Vec<GitCredentialResponse>>> {
    let creds = state
        .git_store
        .list()
        .await
        .map_err(|e| ApiError::Internal(format!("Git credential store: {e}")))?;
    Ok(Json(
        creds
            .into_iter()
            .map(git_credential_response_from)
            .collect(),
    ))
}

/// Create a new git credential. Admin only.
///
/// # Errors
///
/// Returns [`ApiError::Forbidden`] for non-admins,
/// [`ApiError::BadRequest`] for missing fields, or
/// [`ApiError::Internal`] when the store fails.
#[utoipa::path(
    post,
    path = "/api/v1/credentials/git",
    request_body = CreateGitCredentialRequest,
    responses(
        (status = 201, description = "Git credential created", body = GitCredentialResponse),
        (status = 400, description = "Invalid request"),
        (status = 403, description = "Admin role required"),
    ),
    security(("bearer_auth" = [])),
    tag = "Credentials"
)]
pub async fn create_git_credential(
    actor: AuthActor,
    State(state): State<CredentialState>,
    Json(req): Json<CreateGitCredentialRequest>,
) -> Result<(StatusCode, Json<GitCredentialResponse>)> {
    actor.require_admin()?;

    if req.name.trim().is_empty() {
        return Err(ApiError::BadRequest(
            "Credential name is required".to_string(),
        ));
    }
    if req.value.is_empty() {
        return Err(ApiError::BadRequest(
            "Credential value is required".to_string(),
        ));
    }

    let kind = match req.kind {
        GitCredentialKindSchema::Pat => zlayer_secrets::GitCredentialKind::Pat,
        GitCredentialKindSchema::SshKey => zlayer_secrets::GitCredentialKind::SshKey,
    };

    let cred = state
        .git_store
        .create(&req.name, &req.value, kind)
        .await
        .map_err(|e| ApiError::Internal(format!("Git credential store: {e}")))?;

    Ok((
        StatusCode::CREATED,
        Json(git_credential_response_from(cred)),
    ))
}

/// Delete a git credential. Admin only.
///
/// # Errors
///
/// Returns [`ApiError::Forbidden`] for non-admins,
/// [`ApiError::NotFound`] when the credential does not exist, or
/// [`ApiError::Internal`] when the store fails.
#[utoipa::path(
    delete,
    path = "/api/v1/credentials/git/{id}",
    params(("id" = String, Path, description = "Git credential id")),
    responses(
        (status = 204, description = "Git credential deleted"),
        (status = 403, description = "Admin role required"),
        (status = 404, description = "Not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "Credentials"
)]
pub async fn delete_git_credential(
    actor: AuthActor,
    State(state): State<CredentialState>,
    Path(id): Path<String>,
) -> Result<StatusCode> {
    actor.require_admin()?;

    state.git_store.delete(&id).await.map_err(|e| {
        if e.to_string().contains("not found") || e.to_string().contains("NotFound") {
            ApiError::NotFound(format!("Git credential {id} not found"))
        } else {
            ApiError::Internal(format!("Git credential store: {e}"))
        }
    })?;

    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_registry_credential_request_deserialize() {
        let req: CreateRegistryCredentialRequest = serde_json::from_str(
            r#"{"registry":"docker.io","username":"user","password":"pw","auth_type":"basic"}"#,
        )
        .unwrap();
        assert_eq!(req.registry, "docker.io");
        assert_eq!(req.username, "user");
        assert_eq!(req.password, "pw");
        assert_eq!(req.auth_type, RegistryAuthTypeSchema::Basic);
    }

    #[test]
    fn test_create_registry_credential_request_token_type() {
        let req: CreateRegistryCredentialRequest = serde_json::from_str(
            r#"{"registry":"ghcr.io","username":"bot","password":"ghp_xxx","auth_type":"token"}"#,
        )
        .unwrap();
        assert_eq!(req.auth_type, RegistryAuthTypeSchema::Token);
    }

    #[test]
    fn test_create_git_credential_request_pat() {
        let req: CreateGitCredentialRequest =
            serde_json::from_str(r#"{"name":"GitHub PAT","value":"ghp_xxxx","kind":"pat"}"#)
                .unwrap();
        assert_eq!(req.name, "GitHub PAT");
        assert_eq!(req.value, "ghp_xxxx");
        assert_eq!(req.kind, GitCredentialKindSchema::Pat);
    }

    #[test]
    fn test_create_git_credential_request_ssh() {
        let req: CreateGitCredentialRequest = serde_json::from_str(
            r#"{"name":"Deploy key","value":"ssh-rsa AAAA...","kind":"ssh_key"}"#,
        )
        .unwrap();
        assert_eq!(req.kind, GitCredentialKindSchema::SshKey);
    }

    #[test]
    fn test_registry_auth_type_schema_roundtrip() {
        let basic = serde_json::to_string(&RegistryAuthTypeSchema::Basic).unwrap();
        assert_eq!(basic, "\"basic\"");
        let token = serde_json::to_string(&RegistryAuthTypeSchema::Token).unwrap();
        assert_eq!(token, "\"token\"");

        let parsed: RegistryAuthTypeSchema = serde_json::from_str(&basic).unwrap();
        assert_eq!(parsed, RegistryAuthTypeSchema::Basic);
    }

    fn decode_auth(token: &str) -> (String, String) {
        let raw = base64::engine::general_purpose::STANDARD
            .decode(token)
            .unwrap();
        let s = String::from_utf8(raw).unwrap();
        let (u, p) = s.split_once(':').unwrap();
        (u.to_string(), p.to_string())
    }

    #[test]
    fn test_normalize_registry_docker_hub() {
        assert_eq!(
            normalize_registry("docker.io"),
            "https://index.docker.io/v1/"
        );
        assert_eq!(
            normalize_registry("registry-1.docker.io"),
            "https://index.docker.io/v1/"
        );
        assert_eq!(normalize_registry("ghcr.io"), "ghcr.io");
        assert_eq!(
            normalize_registry("registry.example.com:5000"),
            "registry.example.com:5000"
        );
    }

    #[test]
    fn test_auth_entry_base64_roundtrips() {
        let entry = auth_entry("alice", "s3cr:et");
        let token = entry.get("auth").unwrap().as_str().unwrap();
        let (u, p) = decode_auth(token);
        assert_eq!(u, "alice");
        // Password containing ':' must survive (splitn-style decode).
        assert_eq!(p, "s3cr:et");
    }

    #[test]
    fn test_merge_preserves_unrelated_keys_and_foreign_auths() {
        let existing = serde_json::json!({
            "credsStore": "osxkeychain",
            "experimental": "enabled",
            "auths": {
                "registry.foreign.com": { "auth": "Zm9vOmJhcg==" }
            }
        });

        let creds = vec![(
            "ghcr.io".to_string(),
            "bot".to_string(),
            "ghp_x".to_string(),
        )];

        let merged = merge_docker_config(existing, &creds, &std::collections::HashSet::new());

        // Unrelated top-level keys survive.
        assert_eq!(merged.get("credsStore").unwrap(), "osxkeychain");
        assert_eq!(merged.get("experimental").unwrap(), "enabled");

        let auths = merged.get("auths").unwrap().as_object().unwrap();
        // Foreign auth entry (not in our store) is preserved.
        assert!(auths.contains_key("registry.foreign.com"));
        // Our managed cred is added.
        assert!(auths.contains_key("ghcr.io"));
        let (u, p) = decode_auth(auths["ghcr.io"]["auth"].as_str().unwrap());
        assert_eq!(u, "bot");
        assert_eq!(p, "ghp_x");
    }

    #[test]
    fn test_merge_docker_io_normalizes_to_index() {
        let creds = vec![("docker.io".to_string(), "me".to_string(), "pw".to_string())];
        let merged = merge_docker_config(
            serde_json::json!({}),
            &creds,
            &std::collections::HashSet::new(),
        );
        let auths = merged.get("auths").unwrap().as_object().unwrap();
        assert!(auths.contains_key("https://index.docker.io/v1/"));
        assert!(!auths.contains_key("docker.io"));
    }

    #[test]
    fn test_merge_delete_removes_managed_host() {
        // Existing config already has our ghcr.io cred plus a foreign one.
        let existing = serde_json::json!({
            "auths": {
                "ghcr.io": { "auth": "b2xkOm9sZA==" },
                "registry.foreign.com": { "auth": "Zm9vOmJhcg==" }
            }
        });
        // Current store no longer contains ghcr.io (it was deleted) — the handler
        // passes the just-deleted host (normalized) in `remove_hosts`.
        let creds: Vec<(String, String, String)> = vec![];
        let remove: std::collections::HashSet<String> =
            ["ghcr.io".to_string()].into_iter().collect();
        let merged = merge_docker_config(existing, &creds, &remove);
        let auths = merged.get("auths").unwrap().as_object().unwrap();
        // Managed host removed...
        assert!(!auths.contains_key("ghcr.io"));
        // ...foreign host preserved.
        assert!(auths.contains_key("registry.foreign.com"));
    }

    #[test]
    fn test_write_docker_config_creates_and_merges() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("config.json");

        let empty = std::collections::HashSet::new();
        let creds = vec![("docker.io".to_string(), "me".to_string(), "pw".to_string())];
        write_docker_config(&path, &creds, &empty).unwrap();

        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let auths = written.get("auths").unwrap().as_object().unwrap();
        assert!(auths.contains_key("https://index.docker.io/v1/"));
        let (u, p) = decode_auth(
            auths["https://index.docker.io/v1/"]["auth"]
                .as_str()
                .unwrap(),
        );
        assert_eq!(u, "me");
        assert_eq!(p, "pw");

        // Second write with an empty set + a pre-existing foreign key preserved.
        let mut current: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        current["auths"]["registry.foreign.com"] = serde_json::json!({ "auth": "Zm9vOmJhcg==" });
        std::fs::write(&path, serde_json::to_string(&current).unwrap()).unwrap();

        let remove_docker: std::collections::HashSet<String> =
            [normalize_registry("docker.io")].into_iter().collect();
        write_docker_config(&path, &[], &remove_docker).unwrap();
        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let auths = written.get("auths").unwrap().as_object().unwrap();
        // Our managed docker.io entry removed, foreign preserved.
        assert!(!auths.contains_key("https://index.docker.io/v1/"));
        assert!(auths.contains_key("registry.foreign.com"));
    }

    #[test]
    fn test_git_credential_kind_schema_roundtrip() {
        let pat = serde_json::to_string(&GitCredentialKindSchema::Pat).unwrap();
        assert_eq!(pat, "\"pat\"");
        let ssh = serde_json::to_string(&GitCredentialKindSchema::SshKey).unwrap();
        assert_eq!(ssh, "\"ssh_key\"");

        let parsed: GitCredentialKindSchema = serde_json::from_str(&pat).unwrap();
        assert_eq!(parsed, GitCredentialKindSchema::Pat);
    }
}
