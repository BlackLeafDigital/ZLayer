//! One-shot migration: grant `Write` on every environment to every admin.
//!
//! Admin role short-circuits the `PermissionStorage::check()` path so this
//! is not strictly required for correctness. It exists so the
//! `GET /api/v1/permissions/by-resource?kind=environment&id=<env>` endpoint
//! returns a complete picture including admin grants, which is useful for
//! the manager UI and audit trails.
//!
//! Idempotency: gated on a marker row stored in the audit store with
//! `resource_kind = "migration"` and `action = "env_admin_grants_migrated"`.
//! Subsequent daemon starts find the marker and skip re-running.
//!
//! The `EnvironmentStorage::list(None)` call only returns global environments
//! (`project_id IS NULL`), so we additionally walk every project via
//! `ProjectStorage::list()` and collect each project's environments to cover
//! the whole catalog.

use std::sync::Arc;

use anyhow::{Context, Result};
use tracing::{info, warn};

use zlayer_api::storage::{
    AuditEntry, AuditFilter, AuditStorage, EnvironmentStorage, PermissionLevel, PermissionStorage,
    ProjectStorage, StoredPermission, SubjectKind, UserRole, UserStorage,
};

const MIGRATION_ACTION: &str = "env_admin_grants_migrated";
const MIGRATION_RESOURCE_KIND: &str = "migration";
const SYSTEM_ACTOR: &str = "system";

/// Run the migration idempotently.
///
/// Errors from the individual `permissions.grant` calls are logged but not
/// propagated — a stale/corrupt row shouldn't block daemon startup. Errors
/// from the gating audit-log read or the user/env/project listings ARE
/// propagated, because those represent real corruption we want to surface
/// rather than silently skip the migration.
///
/// # Errors
///
/// Returns an error if the audit log, user store, environment store, or
/// project store cannot be queried.
pub async fn run(
    users: Arc<dyn UserStorage>,
    envs: Arc<dyn EnvironmentStorage>,
    projects: Arc<dyn ProjectStorage>,
    permissions: Arc<dyn PermissionStorage>,
    audit: Arc<dyn AuditStorage>,
) -> Result<()> {
    // Gate on the marker row. AuditFilter has no `action` column, so we
    // narrow by resource_kind and then scan in memory.
    let marker_filter = AuditFilter {
        resource_kind: Some(MIGRATION_RESOURCE_KIND.to_string()),
        limit: 1_000,
        ..AuditFilter::default()
    };
    let prior = audit
        .list(marker_filter)
        .await
        .context("Failed to query audit log for migration marker")?;
    if prior.iter().any(|e| e.action == MIGRATION_ACTION) {
        return Ok(());
    }

    // Find every admin user.
    let all_users = users
        .list()
        .await
        .context("Failed to list users for admin-env-grants migration")?;
    let admins: Vec<_> = all_users
        .into_iter()
        .filter(|u| u.role == UserRole::Admin)
        .collect();
    if admins.is_empty() {
        // Nothing to grant; still mark done so we don't rescan every boot.
        let marker = AuditEntry::new(SYSTEM_ACTOR, MIGRATION_ACTION, MIGRATION_RESOURCE_KIND);
        let _ = audit.record(&marker).await;
        return Ok(());
    }

    // Global environments (project_id IS NULL).
    let mut all_envs = envs
        .list(None)
        .await
        .context("Failed to list global environments for admin-env-grants migration")?;

    // Plus every project's environments.
    let all_projects = projects
        .list()
        .await
        .context("Failed to list projects for admin-env-grants migration")?;
    for project in &all_projects {
        match envs.list(Some(&project.id)).await {
            Ok(mut project_envs) => all_envs.append(&mut project_envs),
            Err(e) => warn!(
                project = %project.id,
                error = %e,
                "skipping project envs while walking admin-env-grants migration",
            ),
        }
    }

    let mut granted = 0_usize;
    for admin in &admins {
        for env in &all_envs {
            let perm = StoredPermission::new(
                SubjectKind::User,
                admin.id.clone(),
                "environment".to_string(),
                Some(env.id.clone()),
                PermissionLevel::Write,
            );
            match permissions.grant(&perm).await {
                Ok(()) => granted += 1,
                Err(e) => warn!(
                    admin = %admin.id,
                    env = %env.id,
                    error = %e,
                    "admin-env-grants migration: grant failed",
                ),
            }
        }
    }

    // Record completion marker so subsequent starts skip the migration.
    // Include a short summary in `details` for audit-log readers.
    let mut marker = AuditEntry::new(SYSTEM_ACTOR, MIGRATION_ACTION, MIGRATION_RESOURCE_KIND);
    marker.details = Some(serde_json::json!({
        "granted": granted,
        "admins": admins.len(),
        "envs": all_envs.len(),
    }));
    let _ = audit.record(&marker).await;

    info!(
        granted,
        admins = admins.len(),
        envs = all_envs.len(),
        "admin env-grants migration complete",
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use zlayer_api::storage::{
        InMemoryAuditStore, InMemoryEnvironmentStore, InMemoryPermissionStore,
        InMemoryProjectStore, InMemoryUserStore, StoredEnvironment, StoredProject, StoredUser,
    };

    struct Stores {
        users: Arc<dyn UserStorage>,
        envs: Arc<dyn EnvironmentStorage>,
        projects: Arc<dyn ProjectStorage>,
        permissions: Arc<dyn PermissionStorage>,
        audit: Arc<dyn AuditStorage>,
    }

    fn empty_stores() -> Stores {
        Stores {
            users: Arc::new(InMemoryUserStore::new()),
            envs: Arc::new(InMemoryEnvironmentStore::new()),
            projects: Arc::new(InMemoryProjectStore::new()),
            permissions: Arc::new(InMemoryPermissionStore::new()),
            audit: Arc::new(InMemoryAuditStore::new()),
        }
    }

    async fn run_once(s: &Stores) {
        run(
            s.users.clone(),
            s.envs.clone(),
            s.projects.clone(),
            s.permissions.clone(),
            s.audit.clone(),
        )
        .await
        .expect("migration run");
    }

    #[tokio::test]
    async fn noop_when_no_admins() {
        let s = empty_stores();
        // Regular user only.
        let u = StoredUser::new("u@example.com", "u", UserRole::User);
        s.users.store(&u).await.unwrap();
        // One env.
        let e = StoredEnvironment::new("prod", None);
        s.envs.store(&e).await.unwrap();

        run_once(&s).await;

        // No permissions written.
        let perms = s
            .permissions
            .list_for_subject(SubjectKind::User, &u.id)
            .await
            .unwrap();
        assert!(perms.is_empty(), "should grant nothing without admins");

        // Marker still written so we don't rescan.
        let marker = s
            .audit
            .list(AuditFilter {
                resource_kind: Some(MIGRATION_RESOURCE_KIND.to_string()),
                ..AuditFilter::default()
            })
            .await
            .unwrap();
        assert_eq!(marker.len(), 1);
        assert_eq!(marker[0].action, MIGRATION_ACTION);
    }

    #[tokio::test]
    async fn grants_admin_write_on_every_env() {
        let s = empty_stores();

        // Two admins, one regular user.
        let admin1 = StoredUser::new("a1@example.com", "a1", UserRole::Admin);
        let admin2 = StoredUser::new("a2@example.com", "a2", UserRole::Admin);
        let user = StoredUser::new("u@example.com", "u", UserRole::User);
        s.users.store(&admin1).await.unwrap();
        s.users.store(&admin2).await.unwrap();
        s.users.store(&user).await.unwrap();

        // One global env, one project with two envs.
        let global_env = StoredEnvironment::new("global-prod", None);
        s.envs.store(&global_env).await.unwrap();

        let project = StoredProject::new("proj");
        s.projects.store(&project).await.unwrap();

        let proj_dev = StoredEnvironment::new("dev", Some(project.id.clone()));
        let proj_prod = StoredEnvironment::new("prod", Some(project.id.clone()));
        s.envs.store(&proj_dev).await.unwrap();
        s.envs.store(&proj_prod).await.unwrap();

        run_once(&s).await;

        // Each admin must have Write on each of the 3 envs.
        for admin in [&admin1, &admin2] {
            for env in [&global_env, &proj_dev, &proj_prod] {
                let ok = s
                    .permissions
                    .check(
                        SubjectKind::User,
                        &admin.id,
                        "environment",
                        Some(&env.id),
                        PermissionLevel::Write,
                    )
                    .await
                    .unwrap();
                assert!(ok, "admin {} should have Write on env {}", admin.id, env.id,);
            }
        }

        // Regular user must have no env grants.
        let user_perms = s
            .permissions
            .list_for_subject(SubjectKind::User, &user.id)
            .await
            .unwrap();
        assert!(user_perms.is_empty());
    }

    #[tokio::test]
    async fn idempotent_second_run_adds_no_new_grants() {
        let s = empty_stores();

        let admin = StoredUser::new("a@example.com", "a", UserRole::Admin);
        s.users.store(&admin).await.unwrap();
        let env = StoredEnvironment::new("prod", None);
        s.envs.store(&env).await.unwrap();

        run_once(&s).await;

        let after_first = s
            .permissions
            .list_for_subject(SubjectKind::User, &admin.id)
            .await
            .unwrap();
        assert_eq!(after_first.len(), 1);

        // Second run should be a no-op (marker exists).
        run_once(&s).await;

        let after_second = s
            .permissions
            .list_for_subject(SubjectKind::User, &admin.id)
            .await
            .unwrap();
        assert_eq!(
            after_second.len(),
            1,
            "second run must not add duplicate grants",
        );

        // Exactly one marker row.
        let markers = s
            .audit
            .list(AuditFilter {
                resource_kind: Some(MIGRATION_RESOURCE_KIND.to_string()),
                ..AuditFilter::default()
            })
            .await
            .unwrap();
        let migrate_markers: Vec<_> = markers
            .into_iter()
            .filter(|e| e.action == MIGRATION_ACTION)
            .collect();
        assert_eq!(migrate_markers.len(), 1);
    }

    #[tokio::test]
    async fn noop_when_admins_but_no_envs() {
        let s = empty_stores();
        let admin = StoredUser::new("a@example.com", "a", UserRole::Admin);
        s.users.store(&admin).await.unwrap();

        run_once(&s).await;

        let perms = s
            .permissions
            .list_for_subject(SubjectKind::User, &admin.id)
            .await
            .unwrap();
        assert!(perms.is_empty(), "no envs -> no grants");

        // Marker still written.
        let markers = s
            .audit
            .list(AuditFilter {
                resource_kind: Some(MIGRATION_RESOURCE_KIND.to_string()),
                ..AuditFilter::default()
            })
            .await
            .unwrap();
        assert_eq!(markers.len(), 1);
        assert_eq!(markers[0].action, MIGRATION_ACTION);
    }
}
