//! Sync CRUD + diff/apply endpoints.
//!
//! A sync resource points at a directory within a project's git checkout that
//! contains `ZLayer` resource YAMLs. The diff endpoint scans the directory,
//! compares against current API state, and returns what would change.  The
//! apply endpoint actually reconciles: creating / updating deployments, and
//! (when `delete_missing = true`) deleting deployments that are no longer
//! present in the manifest directory.
//!
//! Jobs and crons declared in sync manifests are currently recorded as
//! `"skip"` with an explanatory message: the API does not yet carry a
//! standalone `JobStorage` / `CronStorage` — job and cron specs live inside
//! `DeploymentSpec` as scheduled services. This is flagged explicitly rather
//! than silently dropped so callers know the resource wasn't applied.
//!
//! Routes:
//!
//! ```text
//! GET    /api/v1/syncs              -> list
//! POST   /api/v1/syncs              -> create
//! GET    /api/v1/syncs/{id}/diff    -> diff (preview)
//! POST   /api/v1/syncs/{id}/apply   -> apply (real reconcile)
//! DELETE /api/v1/syncs/{id}         -> delete
//! ```

use std::path::Path;
use std::sync::Arc;

use axum::{
    extract::{Path as AxumPath, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::error::{ApiError, Result};
use crate::storage::{DeploymentStorage, StoredDeployment, StoredSync, SyncStorage};
use zlayer_git::sync::{compute_diff, scan_resources, SyncDiff, SyncResource};

/// Shared state for sync endpoints.
#[derive(Clone)]
pub struct SyncState {
    /// Underlying sync storage backend.
    pub store: Arc<dyn SyncStorage>,
    /// Deployment storage — the apply endpoint upserts / deletes here so a
    /// sync can actually reconcile `DeploymentSpec` manifests against the API.
    pub deployment_store: Arc<dyn DeploymentStorage>,
    /// Root directory where project checkouts live.
    /// Each project is at `{clone_root}/{project_id}`.
    pub clone_root: std::path::PathBuf,
}

impl SyncState {
    /// Build a new state from a sync store and a deployment store.
    #[must_use]
    pub fn new(store: Arc<dyn SyncStorage>, deployment_store: Arc<dyn DeploymentStorage>) -> Self {
        Self {
            store,
            deployment_store,
            clone_root: std::env::temp_dir().join("zlayer-projects"),
        }
    }

    /// Build a state with a custom clone root.
    #[must_use]
    pub fn with_clone_root(
        store: Arc<dyn SyncStorage>,
        deployment_store: Arc<dyn DeploymentStorage>,
        clone_root: std::path::PathBuf,
    ) -> Self {
        Self {
            store,
            deployment_store,
            clone_root,
        }
    }
}

// ---- Request/response types ----

/// Body for `POST /api/v1/syncs`.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CreateSyncRequest {
    /// Display name for this sync.
    pub name: String,
    /// Linked project id.
    #[serde(default)]
    pub project_id: Option<String>,
    /// Path within the project's checkout to scan for resource YAMLs.
    pub git_path: String,
    /// Whether the sync should automatically apply on pull.
    #[serde(default)]
    pub auto_apply: Option<bool>,
    /// Whether `apply` should delete resources on the API that are missing
    /// from the manifest directory. Defaults to `false` (the safer choice).
    #[serde(default)]
    pub delete_missing: Option<bool>,
}

/// Result of reconciling a single resource during apply.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SyncResourceResult {
    /// The source manifest path (or remote resource name for deletions).
    pub resource: String,
    /// Resource kind: `"deployment"`, `"job"`, `"cron"`, or other.
    pub kind: String,
    /// Action taken: `"create"`, `"update"`, `"delete"`, or `"skip"`.
    pub action: String,
    /// Outcome status: `"ok"` or `"error"`.
    pub status: String,
    /// Optional error message (`status == "error"`) or skip reason
    /// (`action == "skip"`). Omitted on successful `"ok"` results.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl SyncResourceResult {
    fn ok(resource: &str, kind: &str, action: &str) -> Self {
        Self {
            resource: resource.to_string(),
            kind: kind.to_string(),
            action: action.to_string(),
            status: "ok".to_string(),
            error: None,
        }
    }

    fn err(resource: &str, kind: &str, action: &str, message: String) -> Self {
        Self {
            resource: resource.to_string(),
            kind: kind.to_string(),
            action: action.to_string(),
            status: "error".to_string(),
            error: Some(message),
        }
    }

    fn skip(resource: &str, kind: &str, message: String) -> Self {
        Self {
            resource: resource.to_string(),
            kind: kind.to_string(),
            action: "skip".to_string(),
            status: "ok".to_string(),
            error: Some(message),
        }
    }
}

/// Response for a real apply. Reports per-resource outcomes, the current
/// commit SHA the sync was applied at (when known), and a short human-readable
/// summary for CLI display.
///
/// NOTE: This is a breaking change from the previous dry-run-only
/// `{ diff, message }` shape. Clients inspecting the apply response directly
/// must be updated — callers that only consumed the HTTP status stay working.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SyncApplyResponse {
    /// Per-resource reconcile results.
    pub results: Vec<SyncResourceResult>,
    /// Commit SHA the sync was applied against (when resolvable).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub applied_sha: Option<String>,
    /// Aggregate summary suitable for CLI output, e.g.
    /// `"3 created, 2 updated, 1 deleted, 0 skipped"`.
    pub summary: String,
}

/// JSON-friendly wrapper around [`SyncDiff`].
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct SyncDiffResponse {
    /// Resources to create.
    pub to_create: Vec<SyncResourceResponse>,
    /// Resources to update.
    pub to_update: Vec<SyncResourceResponse>,
    /// Resource names to delete.
    pub to_delete: Vec<String>,
}

/// A single resource in the diff output.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct SyncResourceResponse {
    /// Source file name.
    pub file_path: String,
    /// Resource kind.
    pub kind: String,
    /// Resource name.
    pub name: String,
}

impl From<SyncDiff> for SyncDiffResponse {
    fn from(diff: SyncDiff) -> Self {
        Self {
            to_create: diff
                .to_create
                .into_iter()
                .map(|r| SyncResourceResponse {
                    file_path: r.file_path,
                    kind: r.kind,
                    name: r.name,
                })
                .collect(),
            to_update: diff
                .to_update
                .into_iter()
                .map(|r| SyncResourceResponse {
                    file_path: r.file_path,
                    kind: r.kind,
                    name: r.name,
                })
                .collect(),
            to_delete: diff.to_delete,
        }
    }
}

// ---- Endpoints ----

/// List all syncs.
///
/// # Errors
///
/// Returns [`ApiError::Internal`] if the sync store fails.
#[utoipa::path(
    get,
    path = "/api/v1/syncs",
    responses(
        (status = 200, description = "List of syncs", body = Vec<StoredSync>),
    ),
    security(("bearer_auth" = [])),
    tag = "Syncs"
)]
pub async fn list_syncs(State(state): State<SyncState>) -> Result<Json<Vec<StoredSync>>> {
    let syncs = state.store.list().await?;
    Ok(Json(syncs))
}

/// Create a new sync.
///
/// # Errors
///
/// Returns [`ApiError::Validation`] if required fields are missing, or
/// [`ApiError::Conflict`] if a sync with the same name already exists.
#[utoipa::path(
    post,
    path = "/api/v1/syncs",
    request_body = CreateSyncRequest,
    responses(
        (status = 201, description = "Sync created", body = StoredSync),
        (status = 400, description = "Validation error"),
        (status = 409, description = "Sync name already exists"),
    ),
    security(("bearer_auth" = [])),
    tag = "Syncs"
)]
pub async fn create_sync(
    State(state): State<SyncState>,
    Json(body): Json<CreateSyncRequest>,
) -> Result<(StatusCode, Json<StoredSync>)> {
    if body.name.is_empty() {
        return Err(ApiError::Validation("name is required".into()));
    }
    if body.git_path.is_empty() {
        return Err(ApiError::Validation("git_path is required".into()));
    }

    let mut sync = StoredSync::new(&body.name, &body.git_path);
    sync.project_id = body.project_id;
    sync.auto_apply = body.auto_apply.unwrap_or(false);
    sync.delete_missing = body.delete_missing.unwrap_or(false);

    let created = state.store.create(sync).await?;

    Ok((StatusCode::CREATED, Json(created)))
}

/// Compute a diff for a sync (scan git path vs. remote resources).
///
/// # Errors
///
/// Returns [`ApiError::NotFound`] if the sync id does not exist, or
/// [`ApiError::Internal`] if scanning fails.
#[utoipa::path(
    get,
    path = "/api/v1/syncs/{id}/diff",
    params(
        ("id" = String, Path, description = "Sync id"),
    ),
    responses(
        (status = 200, description = "Computed diff", body = SyncDiffResponse),
        (status = 404, description = "Sync not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "Syncs"
)]
pub async fn diff_sync(
    State(state): State<SyncState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<SyncDiffResponse>> {
    let sync = state
        .store
        .get(&id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Sync '{id}' not found")))?;

    let scan_dir = resolve_scan_dir(&state.clone_root, &sync);

    // If the directory doesn't exist yet (project not cloned), return empty diff.
    if !scan_dir.exists() {
        return Ok(Json(SyncDiffResponse {
            to_create: Vec::new(),
            to_update: Vec::new(),
            to_delete: Vec::new(),
        }));
    }

    let local = scan_resources(&scan_dir).map_err(|e| ApiError::Internal(e.to_string()))?;

    let remote = state
        .deployment_store
        .list()
        .await
        .map_err(|e| ApiError::Internal(format!("deployment store: {e}")))?;
    let remote_names: Vec<String> = remote.into_iter().map(|d| d.name).collect();

    let diff = compute_diff(&local, &remote_names);
    Ok(Json(diff.into()))
}

/// Apply a sync — real reconcile against the API.
///
/// For each resource in the manifest directory:
/// - `deployment` YAMLs are parsed, validated, and upserted into the
///   deployment store.
/// - `job` / `cron` kinds are recorded as skipped with an explanatory message
///   (standalone job/cron storage does not yet exist; schedule services via
///   `DeploymentSpec` instead).
/// - Unknown kinds are skipped with `"unknown kind: {kind}"`.
///
/// If `sync.delete_missing` is `true`, deployments present in the store but
/// absent from the manifest directory are deleted. Otherwise deletions are
/// recorded as skipped with reason `"delete_missing=false"`.
///
/// On success the sync's `last_applied_sha` is refreshed from the current
/// checkout and persisted.
///
/// # Errors
///
/// Returns [`ApiError::NotFound`] if the sync id does not exist, or
/// [`ApiError::Internal`] if scanning fails.
#[utoipa::path(
    post,
    path = "/api/v1/syncs/{id}/apply",
    params(
        ("id" = String, Path, description = "Sync id"),
    ),
    responses(
        (status = 200, description = "Apply result", body = SyncApplyResponse),
        (status = 404, description = "Sync not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "Syncs"
)]
pub async fn apply_sync(
    State(state): State<SyncState>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<SyncApplyResponse>> {
    let sync = state
        .store
        .get(&id)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Sync '{id}' not found")))?;

    let response = apply_sync_inner(
        &sync,
        &state.store,
        &state.deployment_store,
        &state.clone_root,
        ApplyOptions {
            delete_missing: sync.delete_missing,
        },
    )
    .await?;

    Ok(Json(response))
}

/// Delete a sync.
///
/// # Errors
///
/// Returns [`ApiError::NotFound`] if the sync id does not exist, or
/// [`ApiError::Internal`] if the store operation fails.
#[utoipa::path(
    delete,
    path = "/api/v1/syncs/{id}",
    params(
        ("id" = String, Path, description = "Sync id"),
    ),
    responses(
        (status = 204, description = "Sync deleted"),
        (status = 404, description = "Sync not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "Syncs"
)]
pub async fn delete_sync(
    State(state): State<SyncState>,
    AxumPath(id): AxumPath<String>,
) -> Result<StatusCode> {
    let deleted = state.store.delete(&id).await?;

    if deleted {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound(format!("Sync '{id}' not found")))
    }
}

// ---------------------------------------------------------------------------
// Apply internals (shared with workflow `ApplySync` action, Fix 3).
// ---------------------------------------------------------------------------

/// Options controlling how [`apply_sync_inner`] reconciles.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ApplyOptions {
    /// Whether to actually delete deployments absent from the manifest dir.
    pub delete_missing: bool,
}

/// Shared apply implementation.
///
/// Extracted as `pub(crate)` so the workflow `ApplySync` action (Fix 3) can
/// invoke the same reconcile logic without going back out through HTTP.
///
/// # Errors
///
/// Returns [`ApiError::Internal`] when scanning the manifest directory or
/// listing deployments fails. Per-resource errors do **not** abort the apply —
/// they are recorded on the returned [`SyncApplyResponse::results`] with
/// `status = "error"` so the caller sees the full picture.
pub(crate) async fn apply_sync_inner(
    sync: &StoredSync,
    sync_store: &Arc<dyn SyncStorage>,
    deployment_store: &Arc<dyn DeploymentStorage>,
    clone_root: &Path,
    opts: ApplyOptions,
) -> Result<SyncApplyResponse> {
    let scan_dir = resolve_scan_dir(clone_root, sync);

    // If the clone directory doesn't exist yet, return an empty successful
    // response rather than erroring — the caller hasn't pulled yet.
    if !scan_dir.exists() {
        return Ok(SyncApplyResponse {
            results: Vec::new(),
            applied_sha: None,
            summary: "project not cloned (scan directory missing)".to_string(),
        });
    }

    let local = scan_resources(&scan_dir).map_err(|e| ApiError::Internal(e.to_string()))?;

    let remote = deployment_store
        .list()
        .await
        .map_err(|e| ApiError::Internal(format!("deployment store: {e}")))?;
    let remote_names: Vec<String> = remote.into_iter().map(|d| d.name).collect();

    let diff = compute_diff(&local, &remote_names);

    let mut results: Vec<SyncResourceResult> = Vec::new();

    // ---- Creates ----
    for resource in &diff.to_create {
        results.push(reconcile_resource(deployment_store, resource, "create").await);
    }

    // ---- Updates ----
    for resource in &diff.to_update {
        results.push(reconcile_resource(deployment_store, resource, "update").await);
    }

    // ---- Deletes ----
    for name in &diff.to_delete {
        if opts.delete_missing {
            match deployment_store.delete(name).await {
                Ok(true) => {
                    results.push(SyncResourceResult::ok(name, "deployment", "delete"));
                }
                Ok(false) => {
                    // Raced with another actor; surface as a skip rather than
                    // an error so the overall reconcile stays monotonic.
                    results.push(SyncResourceResult::skip(
                        name,
                        "deployment",
                        "deployment already absent at delete time".to_string(),
                    ));
                }
                Err(e) => {
                    results.push(SyncResourceResult::err(
                        name,
                        "deployment",
                        "delete",
                        format!("deployment store: {e}"),
                    ));
                }
            }
        } else {
            results.push(SyncResourceResult {
                resource: name.clone(),
                kind: "deployment".to_string(),
                action: "skip".to_string(),
                status: "ok".to_string(),
                error: Some("delete_missing=false".to_string()),
            });
        }
    }

    // ---- Determine applied SHA from current checkout (best-effort) ----
    let applied_sha = zlayer_git::current_sha(&scan_dir).await.ok();

    // ---- Summary counts ----
    let created = results
        .iter()
        .filter(|r| r.action == "create" && r.status == "ok")
        .count();
    let updated = results
        .iter()
        .filter(|r| r.action == "update" && r.status == "ok")
        .count();
    let deleted = results
        .iter()
        .filter(|r| r.action == "delete" && r.status == "ok")
        .count();
    let skipped = results.iter().filter(|r| r.action == "skip").count();
    let errored = results.iter().filter(|r| r.status == "error").count();

    let summary = format!(
        "{created} created, {updated} updated, {deleted} deleted, {skipped} skipped, {errored} error(s)"
    );

    // ---- Persist last_applied_sha when we resolved one ----
    if let Some(ref sha) = applied_sha {
        let mut updated_sync = sync.clone();
        updated_sync.last_applied_sha = Some(sha.clone());
        updated_sync.updated_at = chrono::Utc::now();
        // Non-fatal: if the sync row was deleted concurrently we just skip
        // the metadata update and keep the reconcile results.
        if let Err(e) = sync_store.update(updated_sync).await {
            tracing::warn!(sync_id = %sync.id, error = %e, "failed to persist last_applied_sha");
        }
    }

    Ok(SyncApplyResponse {
        results,
        applied_sha,
        summary,
    })
}

/// Parse a single manifest resource and upsert it into the deployment store.
/// Jobs, crons and unknown kinds are returned as skip results.
async fn reconcile_resource(
    deployment_store: &Arc<dyn DeploymentStorage>,
    resource: &SyncResource,
    action: &str,
) -> SyncResourceResult {
    match resource.kind.as_str() {
        "deployment" => {
            let spec = match zlayer_spec::from_yaml_str(&resource.content) {
                Ok(s) => s,
                Err(e) => {
                    return SyncResourceResult::err(
                        &resource.file_path,
                        &resource.kind,
                        action,
                        format!("invalid deployment spec: {e}"),
                    );
                }
            };

            let stored = StoredDeployment::new(spec);
            match deployment_store.store(&stored).await {
                Ok(()) => SyncResourceResult::ok(&resource.file_path, &resource.kind, action),
                Err(e) => SyncResourceResult::err(
                    &resource.file_path,
                    &resource.kind,
                    action,
                    format!("deployment store: {e}"),
                ),
            }
        }
        "job" | "cron" => SyncResourceResult::skip(
            &resource.file_path,
            &resource.kind,
            format!(
                "{} kind not yet supported by sync apply (schedule services via DeploymentSpec)",
                resource.kind
            ),
        ),
        other => SyncResourceResult::skip(
            &resource.file_path,
            &resource.kind,
            format!("unknown kind: {other}"),
        ),
    }
}

/// Resolve the filesystem path to scan for a given sync.
fn resolve_scan_dir(clone_root: &Path, sync: &StoredSync) -> std::path::PathBuf {
    let base = if let Some(ref project_id) = sync.project_id {
        clone_root.join(project_id)
    } else {
        clone_root.to_path_buf()
    };
    base.join(&sync.git_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{InMemoryStorage, InMemorySyncStore};
    use std::sync::Arc;
    use tempfile::TempDir;

    fn write_deployment_yaml(dir: &Path, name: &str, deployment_name: &str) {
        // Minimal valid DeploymentSpec: version + deployment + empty services.
        let yaml = format!("version: v1\ndeployment: {deployment_name}\nservices: {{}}\n",);
        std::fs::write(dir.join(format!("{name}.yaml")), yaml).unwrap();
    }

    fn make_sync(tmp_scan: &Path) -> StoredSync {
        // When project_id is None the scan dir is `clone_root/git_path`.
        // We use a relative `git_path` of "." so scan_dir == clone_root.
        let mut s = StoredSync::new("s1", ".");
        s.project_id = None;
        // Align id with the deterministic test path so resolve_scan_dir
        // resolves to `tmp_scan/.` = `tmp_scan`.
        // tmp_scan is passed directly as clone_root in the tests, so nothing
        // more to configure here.
        let _ = tmp_scan;
        s
    }

    #[tokio::test]
    async fn test_apply_creates_deployment() {
        let tmp = TempDir::new().unwrap();
        write_deployment_yaml(tmp.path(), "app-a", "app-a");

        let sync_store: Arc<dyn SyncStorage> = Arc::new(InMemorySyncStore::new());
        let deploy_store: Arc<dyn DeploymentStorage> = Arc::new(InMemoryStorage::new());

        let sync = sync_store
            .create(make_sync(tmp.path()))
            .await
            .expect("create sync");

        let resp = apply_sync_inner(
            &sync,
            &sync_store,
            &deploy_store,
            tmp.path(),
            ApplyOptions {
                delete_missing: false,
            },
        )
        .await
        .expect("apply");

        assert_eq!(resp.results.len(), 1);
        assert_eq!(resp.results[0].kind, "deployment");
        assert_eq!(resp.results[0].action, "create");
        assert_eq!(resp.results[0].status, "ok");

        let got = deploy_store.get("app-a").await.unwrap();
        assert!(got.is_some(), "deployment must have been written");
    }

    #[tokio::test]
    async fn test_apply_updates_existing_deployment() {
        let tmp = TempDir::new().unwrap();
        write_deployment_yaml(tmp.path(), "app-a", "app-a");

        let sync_store: Arc<dyn SyncStorage> = Arc::new(InMemorySyncStore::new());
        let deploy_store: Arc<dyn DeploymentStorage> = Arc::new(InMemoryStorage::new());

        // Pre-seed a deployment with the same name so the diff lists it under to_update.
        let seed_spec =
            zlayer_spec::from_yaml_str("version: v1\ndeployment: app-a\nservices: {}\n").unwrap();
        deploy_store
            .store(&StoredDeployment::new(seed_spec))
            .await
            .unwrap();

        let sync = sync_store
            .create(make_sync(tmp.path()))
            .await
            .expect("create sync");

        let resp = apply_sync_inner(
            &sync,
            &sync_store,
            &deploy_store,
            tmp.path(),
            ApplyOptions {
                delete_missing: false,
            },
        )
        .await
        .expect("apply");

        assert_eq!(resp.results.len(), 1);
        assert_eq!(resp.results[0].action, "update");
        assert_eq!(resp.results[0].status, "ok");

        // Still exactly one row (update-in-place, not duplicated).
        let list = deploy_store.list().await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "app-a");
    }

    #[tokio::test]
    async fn test_apply_delete_missing_skipped_when_false() {
        let tmp = TempDir::new().unwrap();
        // No manifest in the scan dir — the existing deployment is "missing"
        // from the local side.

        let sync_store: Arc<dyn SyncStorage> = Arc::new(InMemorySyncStore::new());
        let deploy_store: Arc<dyn DeploymentStorage> = Arc::new(InMemoryStorage::new());

        let seed_spec =
            zlayer_spec::from_yaml_str("version: v1\ndeployment: stale\nservices: {}\n").unwrap();
        deploy_store
            .store(&StoredDeployment::new(seed_spec))
            .await
            .unwrap();

        let sync = sync_store
            .create(make_sync(tmp.path()))
            .await
            .expect("create sync");

        let resp = apply_sync_inner(
            &sync,
            &sync_store,
            &deploy_store,
            tmp.path(),
            ApplyOptions {
                delete_missing: false,
            },
        )
        .await
        .expect("apply");

        assert_eq!(resp.results.len(), 1);
        assert_eq!(resp.results[0].action, "skip");
        assert_eq!(resp.results[0].status, "ok");
        assert_eq!(
            resp.results[0].error.as_deref(),
            Some("delete_missing=false")
        );

        // The deployment must NOT have been deleted.
        assert!(deploy_store.get("stale").await.unwrap().is_some());
    }

    #[tokio::test]
    async fn test_apply_delete_missing_honored_when_true() {
        let tmp = TempDir::new().unwrap();
        // Empty scan dir again; delete_missing=true this time.

        let sync_store: Arc<dyn SyncStorage> = Arc::new(InMemorySyncStore::new());
        let deploy_store: Arc<dyn DeploymentStorage> = Arc::new(InMemoryStorage::new());

        let seed_spec =
            zlayer_spec::from_yaml_str("version: v1\ndeployment: stale\nservices: {}\n").unwrap();
        deploy_store
            .store(&StoredDeployment::new(seed_spec))
            .await
            .unwrap();

        let mut s = make_sync(tmp.path());
        s.delete_missing = true;
        let sync = sync_store.create(s).await.expect("create sync");

        let resp = apply_sync_inner(
            &sync,
            &sync_store,
            &deploy_store,
            tmp.path(),
            ApplyOptions {
                delete_missing: true,
            },
        )
        .await
        .expect("apply");

        assert_eq!(resp.results.len(), 1);
        assert_eq!(resp.results[0].action, "delete");
        assert_eq!(resp.results[0].status, "ok");

        assert!(deploy_store.get("stale").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_apply_unknown_kind_skipped() {
        let tmp = TempDir::new().unwrap();
        // A manifest with an explicit `kind: custom` that isn't deployment/job/cron.
        std::fs::write(
            tmp.path().join("odd.yaml"),
            "kind: custom\nname: my-thing\n",
        )
        .unwrap();

        let sync_store: Arc<dyn SyncStorage> = Arc::new(InMemorySyncStore::new());
        let deploy_store: Arc<dyn DeploymentStorage> = Arc::new(InMemoryStorage::new());

        let sync = sync_store
            .create(make_sync(tmp.path()))
            .await
            .expect("create sync");

        let resp = apply_sync_inner(
            &sync,
            &sync_store,
            &deploy_store,
            tmp.path(),
            ApplyOptions {
                delete_missing: false,
            },
        )
        .await
        .expect("apply");

        assert_eq!(resp.results.len(), 1);
        assert_eq!(resp.results[0].action, "skip");
        assert_eq!(resp.results[0].kind, "custom");
        assert_eq!(resp.results[0].status, "ok");
        assert!(
            resp.results[0]
                .error
                .as_deref()
                .unwrap_or_default()
                .contains("unknown kind"),
            "expected skip reason to mention unknown kind, got: {:?}",
            resp.results[0].error
        );
    }

    #[tokio::test]
    async fn test_apply_job_kind_skipped_with_message() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("nightly.yaml"), "job: nightly-backup\n").unwrap();

        let sync_store: Arc<dyn SyncStorage> = Arc::new(InMemorySyncStore::new());
        let deploy_store: Arc<dyn DeploymentStorage> = Arc::new(InMemoryStorage::new());

        let sync = sync_store
            .create(make_sync(tmp.path()))
            .await
            .expect("create sync");

        let resp = apply_sync_inner(
            &sync,
            &sync_store,
            &deploy_store,
            tmp.path(),
            ApplyOptions {
                delete_missing: false,
            },
        )
        .await
        .expect("apply");

        assert_eq!(resp.results.len(), 1);
        assert_eq!(resp.results[0].action, "skip");
        assert_eq!(resp.results[0].kind, "job");
        assert_eq!(resp.results[0].status, "ok");
        assert!(resp.results[0]
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("not yet supported"));
    }
}
