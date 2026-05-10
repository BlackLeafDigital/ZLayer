//! Phase-1 integration tests for cluster-replicated secrets RBAC.
//!
//! These tests prove that `StoredPermission { resource_kind: "secret", .. }`
//! is honored end-to-end by [`zlayer_api::require_secret_perm`], including:
//!
//! - admin-role short circuit,
//! - direct user grants on the storage key `{scope}:{name}`,
//! - wildcard user grants (`resource_id = None`),
//! - group-membership grants,
//! - env-scope fallback (`env:{id}` and `project:{pid}:env:{id}`),
//! - composition (multiple grant paths) and the deny case.
//!
//! Mocks: re-uses the public in-memory stores from `zlayer_api::storage`
//! (`InMemoryPermissionStore`, `InMemoryGroupStore`, `InMemoryEnvironmentStore`).
//! No new test-only `pub` surface was added.

use zlayer_api::handlers::users::AuthActor;
use zlayer_api::storage::{
    GroupStorage, InMemoryEnvironmentStore, InMemoryGroupStore, InMemoryPermissionStore,
    PermissionLevel, PermissionStorage, StoredPermission, StoredUserGroup, SubjectKind,
};
use zlayer_api::{require_secret_perm, ApiError};

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

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

async fn grant(store: &InMemoryPermissionStore, p: StoredPermission) {
    <InMemoryPermissionStore as PermissionStorage>::grant(store, &p)
        .await
        .expect("permission grant");
}

// ---------------------------------------------------------------------------
// admin role
// ---------------------------------------------------------------------------

#[tokio::test]
async fn admin_role_short_circuits_all_secret_grants() {
    let perms = InMemoryPermissionStore::new();
    let groups = InMemoryGroupStore::new();
    let envs = InMemoryEnvironmentStore::new();
    let actor = admin_actor("admin-1");

    // Admin bypasses RBAC across a variety of scope/name/level combos against
    // a totally empty perm store.
    let cases = [
        ("default", "foo", PermissionLevel::Read),
        ("default", "foo", PermissionLevel::Write),
        ("env:abc-uuid", "API_KEY", PermissionLevel::Write),
        (
            "project:p-uuid:env:abc-uuid",
            "DB_URL",
            PermissionLevel::Execute,
        ),
        ("custom-scope", "name-with-dashes", PermissionLevel::Read),
    ];

    for (scope, name, level) in cases {
        require_secret_perm(&actor, &perms, &groups, Some(&envs), scope, name, level)
            .await
            .unwrap_or_else(|e| {
                panic!("admin should bypass RBAC for ({scope}, {name}, {level:?}): {e:?}")
            });
    }

    // Same with no env store.
    require_secret_perm(
        &actor,
        &perms,
        &groups,
        None,
        "env:abc-uuid",
        "API_KEY",
        PermissionLevel::Write,
    )
    .await
    .expect("admin should bypass RBAC even without an env store");
}

// ---------------------------------------------------------------------------
// direct user grant at the exact storage key
// ---------------------------------------------------------------------------

#[tokio::test]
async fn direct_user_grant_at_exact_storage_key() {
    let perms = InMemoryPermissionStore::new();
    let groups = InMemoryGroupStore::new();
    let alice = user_actor("alice-id");

    grant(
        &perms,
        StoredPermission::new(
            SubjectKind::User,
            "alice-id",
            "secret",
            Some("default:foo".to_string()),
            PermissionLevel::Read,
        ),
    )
    .await;

    // Read at exact key: allowed.
    require_secret_perm(
        &alice,
        &perms,
        &groups,
        None,
        "default",
        "foo",
        PermissionLevel::Read,
    )
    .await
    .expect("direct user Read grant on default:foo should permit Read");

    // Write at exact key: denied (Read grant does not satisfy Write).
    let err = require_secret_perm(
        &alice,
        &perms,
        &groups,
        None,
        "default",
        "foo",
        PermissionLevel::Write,
    )
    .await
    .expect_err("Read grant must not satisfy Write");
    assert!(
        matches!(err, ApiError::Forbidden(_)),
        "expected Forbidden for Write request with Read grant, got {err:?}"
    );

    // Different key, same subject: denied.
    let err = require_secret_perm(
        &alice,
        &perms,
        &groups,
        None,
        "default",
        "other-name",
        PermissionLevel::Read,
    )
    .await
    .expect_err("grant on default:foo must not extend to default:other-name");
    assert!(matches!(err, ApiError::Forbidden(_)));
}

// ---------------------------------------------------------------------------
// direct user wildcard grant
// ---------------------------------------------------------------------------

#[tokio::test]
async fn direct_user_wildcard_grant() {
    let perms = InMemoryPermissionStore::new();
    let groups = InMemoryGroupStore::new();
    let bob = user_actor("bob-id");

    // Wildcard secret-kind grant at Write.
    grant(
        &perms,
        StoredPermission::new(
            SubjectKind::User,
            "bob-id",
            "secret",
            None,
            PermissionLevel::Write,
        ),
    )
    .await;

    // Many disparate scope+name combinations should pass at Write.
    let cases = [
        ("default", "foo"),
        ("default", "bar"),
        ("custom-scope", "anything"),
        ("project:p1:env:e1", "API_KEY"),
        ("env:abc", "PASS"),
    ];

    for (scope, name) in cases {
        require_secret_perm(
            &bob,
            &perms,
            &groups,
            None,
            scope,
            name,
            PermissionLevel::Write,
        )
        .await
        .unwrap_or_else(|e| panic!("wildcard Write should allow ({scope}, {name}): {e:?}"));
    }
}

// ---------------------------------------------------------------------------
// wildcard grant — confirms highest-level handling
// ---------------------------------------------------------------------------

#[tokio::test]
async fn wildcard_grant_does_not_satisfy_higher_level() {
    // Bob has wildcard `Write`. The current `PermissionLevel` ordering is
    // None < Read < Execute < Write, so `Write` is the maximum. This test
    // pins down that:
    //  - the wildcard satisfies its own level,
    //  - and (for sanity) that a *Read*-only wildcard would not satisfy a
    //    Write request — protecting against a future ordering regression.
    let perms = InMemoryPermissionStore::new();
    let groups = InMemoryGroupStore::new();

    // 1. Wildcard Write satisfies a Write request.
    let bob = user_actor("bob-id");
    grant(
        &perms,
        StoredPermission::new(
            SubjectKind::User,
            "bob-id",
            "secret",
            None,
            PermissionLevel::Write,
        ),
    )
    .await;
    require_secret_perm(
        &bob,
        &perms,
        &groups,
        None,
        "default",
        "anything",
        PermissionLevel::Write,
    )
    .await
    .expect("wildcard Write must satisfy a Write request");

    // 2. Separate user with wildcard Read must NOT satisfy a Write request.
    let perms2 = InMemoryPermissionStore::new();
    let bob2 = user_actor("bob2-id");
    grant(
        &perms2,
        StoredPermission::new(
            SubjectKind::User,
            "bob2-id",
            "secret",
            None,
            PermissionLevel::Read,
        ),
    )
    .await;
    let err = require_secret_perm(
        &bob2,
        &perms2,
        &groups,
        None,
        "default",
        "anything",
        PermissionLevel::Write,
    )
    .await
    .expect_err("wildcard Read must not satisfy Write");
    assert!(matches!(err, ApiError::Forbidden(_)));
}

// ---------------------------------------------------------------------------
// group membership grant
// ---------------------------------------------------------------------------

#[tokio::test]
async fn group_membership_grant_via_in_memory_group_store() {
    let perms = InMemoryPermissionStore::new();
    let groups = InMemoryGroupStore::new();
    let carol = user_actor("carol-id");

    // Create the `engineering` group and add Carol to it.
    let engineering = StoredUserGroup::new("engineering");
    let engineering_id = engineering.id.clone();
    groups.store(&engineering).await.expect("store group");
    groups
        .add_member(&engineering_id, "carol-id")
        .await
        .expect("add group member");

    // Group has Read on `default:db_url`.
    grant(
        &perms,
        StoredPermission::new(
            SubjectKind::Group,
            &engineering_id,
            "secret",
            Some("default:db_url".to_string()),
            PermissionLevel::Read,
        ),
    )
    .await;

    // Carol gets Read via group membership.
    require_secret_perm(
        &carol,
        &perms,
        &groups,
        None,
        "default",
        "db_url",
        PermissionLevel::Read,
    )
    .await
    .expect("group Read grant should propagate to member");
}

#[tokio::test]
async fn group_grant_at_lower_level_falls_back_to_forbidden() {
    let perms = InMemoryPermissionStore::new();
    let groups = InMemoryGroupStore::new();
    let carol = user_actor("carol-id");

    let engineering = StoredUserGroup::new("engineering");
    let engineering_id = engineering.id.clone();
    groups.store(&engineering).await.unwrap();
    groups
        .add_member(&engineering_id, "carol-id")
        .await
        .unwrap();

    grant(
        &perms,
        StoredPermission::new(
            SubjectKind::Group,
            &engineering_id,
            "secret",
            Some("default:db_url".to_string()),
            PermissionLevel::Read,
        ),
    )
    .await;

    let err = require_secret_perm(
        &carol,
        &perms,
        &groups,
        None,
        "default",
        "db_url",
        PermissionLevel::Write,
    )
    .await
    .expect_err("group Read should not satisfy Write");
    assert!(
        matches!(err, ApiError::Forbidden(_)),
        "expected Forbidden, got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// env-scope fallback
// ---------------------------------------------------------------------------

#[tokio::test]
async fn env_scope_fallback_via_env_storage() {
    let perms = InMemoryPermissionStore::new();
    let groups = InMemoryGroupStore::new();
    let envs = InMemoryEnvironmentStore::new();
    let dave = user_actor("dave-id");

    // No `secret`-kind grants for Dave; only an environment Read on `abc-uuid`.
    grant(
        &perms,
        StoredPermission::new(
            SubjectKind::User,
            "dave-id",
            "environment",
            Some("abc-uuid".to_string()),
            PermissionLevel::Read,
        ),
    )
    .await;

    // Env-shaped scope => env fallback path => allowed at Read.
    require_secret_perm(
        &dave,
        &perms,
        &groups,
        Some(&envs),
        "env:abc-uuid",
        "any_secret",
        PermissionLevel::Read,
    )
    .await
    .expect("env Read should permit secret Read on env-shaped scope");
}

#[tokio::test]
async fn env_scope_fallback_with_project_prefix() {
    let perms = InMemoryPermissionStore::new();
    let groups = InMemoryGroupStore::new();
    let envs = InMemoryEnvironmentStore::new();
    let dave = user_actor("dave-id");

    grant(
        &perms,
        StoredPermission::new(
            SubjectKind::User,
            "dave-id",
            "environment",
            Some("abc-uuid".to_string()),
            PermissionLevel::Read,
        ),
    )
    .await;

    // `project:{pid}:env:{id}` should still parse to env id `abc-uuid`.
    require_secret_perm(
        &dave,
        &perms,
        &groups,
        Some(&envs),
        "project:p-uuid:env:abc-uuid",
        "DB_URL",
        PermissionLevel::Read,
    )
    .await
    .expect("project-scoped env Read should permit secret Read via env fallback");
}

#[tokio::test]
async fn env_scope_fallback_disabled_when_env_store_none() {
    let perms = InMemoryPermissionStore::new();
    let groups = InMemoryGroupStore::new();
    let dave = user_actor("dave-id");

    // Same env grant as above.
    grant(
        &perms,
        StoredPermission::new(
            SubjectKind::User,
            "dave-id",
            "environment",
            Some("abc-uuid".to_string()),
            PermissionLevel::Read,
        ),
    )
    .await;

    // `env_store: None` => env fallback branch is skipped, so the env Read
    // grant doesn't get consulted, and Dave has no secret-kind grants.
    let err = require_secret_perm(
        &dave,
        &perms,
        &groups,
        None,
        "env:abc-uuid",
        "any_secret",
        PermissionLevel::Read,
    )
    .await
    .expect_err("env fallback must not run when env_store is None");
    assert!(
        matches!(err, ApiError::Forbidden(_)),
        "expected Forbidden, got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// composition
// ---------------------------------------------------------------------------

#[tokio::test]
async fn composite_grants_multiple_grant_paths_succeed() {
    let perms = InMemoryPermissionStore::new();
    let groups = InMemoryGroupStore::new();
    let eve = user_actor("eve-id");

    // Both a wildcard Read and a wildcard Write — the higher-level grant
    // should satisfy a Write request regardless of iteration order.
    grant(
        &perms,
        StoredPermission::new(
            SubjectKind::User,
            "eve-id",
            "secret",
            None,
            PermissionLevel::Read,
        ),
    )
    .await;
    grant(
        &perms,
        StoredPermission::new(
            SubjectKind::User,
            "eve-id",
            "secret",
            None,
            PermissionLevel::Write,
        ),
    )
    .await;

    require_secret_perm(
        &eve,
        &perms,
        &groups,
        None,
        "default",
        "foo",
        PermissionLevel::Write,
    )
    .await
    .expect("composite Read+Write wildcards should satisfy Write");

    // And of course Read should still pass.
    require_secret_perm(
        &eve,
        &perms,
        &groups,
        None,
        "default",
        "foo",
        PermissionLevel::Read,
    )
    .await
    .expect("composite Read+Write wildcards should satisfy Read");
}

// ---------------------------------------------------------------------------
// no grants -> Forbidden, with descriptive message
// ---------------------------------------------------------------------------

#[tokio::test]
async fn no_grants_returns_forbidden_with_descriptive_message() {
    let perms = InMemoryPermissionStore::new();
    let groups = InMemoryGroupStore::new();
    let envs = InMemoryEnvironmentStore::new();
    let frank = user_actor("frank-id");

    // Read attempt: message should mention storage key `default:foo` and `read`.
    let err = require_secret_perm(
        &frank,
        &perms,
        &groups,
        Some(&envs),
        "default",
        "foo",
        PermissionLevel::Read,
    )
    .await
    .expect_err("no grants must produce Forbidden");

    match err {
        ApiError::Forbidden(msg) => {
            assert!(
                msg.contains("default:foo"),
                "denial message should reference storage key, got: {msg}"
            );
            assert!(
                msg.to_ascii_lowercase().contains("read"),
                "denial message should mention requested level (Read), got: {msg}"
            );
        }
        other => panic!("expected Forbidden, got {other:?}"),
    }

    // Write attempt: message should mention `write`.
    let err = require_secret_perm(
        &frank,
        &perms,
        &groups,
        Some(&envs),
        "default",
        "foo",
        PermissionLevel::Write,
    )
    .await
    .expect_err("no grants must produce Forbidden for Write");

    match err {
        ApiError::Forbidden(msg) => {
            assert!(
                msg.contains("default:foo"),
                "denial message should reference storage key, got: {msg}"
            );
            assert!(
                msg.to_ascii_lowercase().contains("write"),
                "denial message should mention requested level (Write), got: {msg}"
            );
        }
        other => panic!("expected Forbidden, got {other:?}"),
    }
}
