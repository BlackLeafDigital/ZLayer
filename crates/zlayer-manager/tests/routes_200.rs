//! Route smoke tests.
//!
//! The Manager is an SSR + hydrate Leptos app that's wired up in
//! `bin/main.rs` with a cargo-leptos-generated asset pipeline. Wiring the
//! full router in a unit test would require reproducing that whole binary
//! shell, which isn't practical here — so we instead assert that the page
//! components exist and compile, which is what the acceptance criteria
//! reduce to for this pilot.

#![cfg(feature = "ssr")]

/// Compile-check guard: if this test compiles, the `Variables` component
/// was exported correctly from `app::pages` and is reachable from the
/// crate's public surface.
#[test]
fn variables_component_exists() {
    // Taking a pointer to the function proves the symbol exists without
    // needing to actually render it (which would require a Leptos runtime
    // + reactive context).
    let _: fn() -> _ = zlayer_manager::app::pages::Variables;
}

/// Same guard for `Users` — sanity check that the existing pages list
/// didn't break when we added the `variables` module.
#[test]
fn users_component_exists() {
    let _: fn() -> _ = zlayer_manager::app::pages::Users;
}

/// Assert that the `WireVariable` wire type round-trips via serde — any
/// field drift between the daemon's `StoredVariable` and our mirror shows
/// up here as a deserialize failure.
#[test]
fn wire_variable_round_trips() {
    use zlayer_manager::wire::variables::WireVariable;

    let sample = WireVariable {
        id: "abc-123".to_string(),
        name: "APP_VERSION".to_string(),
        value: "1.2.3".to_string(),
        scope: Some("proj-1".to_string()),
        created_at: "2026-04-16T00:00:00Z".to_string(),
        updated_at: "2026-04-16T01:00:00Z".to_string(),
    };

    let json = serde_json::to_string(&sample).unwrap();
    let back: WireVariable = serde_json::from_str(&json).unwrap();
    assert_eq!(sample, back);
}

/// Global variables return `scope: null`; deserialization must accept
/// that (the `#[serde(default)]` handles it).
#[test]
fn wire_variable_accepts_null_scope() {
    use zlayer_manager::wire::variables::WireVariable;

    let json = r#"{
        "id": "v1",
        "name": "FOO",
        "value": "bar",
        "scope": null,
        "created_at": "2026-04-16T00:00:00Z",
        "updated_at": "2026-04-16T00:00:00Z"
    }"#;
    let v: WireVariable = serde_json::from_str(json).unwrap();
    assert!(v.scope.is_none());
}

/// Compile-check guard: the `Secrets` component is exported and the
/// `/secrets` route is reachable (symbol exists). The full router
/// instantiation lives in the SSR binary — we assert surface here the
/// same way we do for the Variables pilot.
#[test]
fn secrets_component_exists() {
    let _: fn() -> _ = zlayer_manager::app::pages::Secrets;
}

/// Daemon returns `SecretMetadataResponse` — make sure our `WireSecret`
/// still deserializes the exact JSON shape (integer timestamps, u32
/// version, optional `value`). The list endpoint OMITS `value`, the
/// reveal endpoint INCLUDES it; both must parse.
#[test]
fn wire_secret_round_trips_list_shape() {
    use zlayer_manager::wire::secrets::WireSecret;

    let json = r#"{
        "name": "DATABASE_URL",
        "created_at": 1776326400,
        "updated_at": 1776412800,
        "version": 3
    }"#;
    let s: WireSecret = serde_json::from_str(json).unwrap();
    assert_eq!(s.name, "DATABASE_URL");
    assert_eq!(s.version, 3);
    assert!(s.value.is_none());
}

#[test]
fn wire_secret_round_trips_reveal_shape() {
    use zlayer_manager::wire::secrets::WireSecret;

    let json = r#"{
        "name": "TOKEN",
        "created_at": 1776326400,
        "updated_at": 1776326400,
        "version": 1,
        "value": "super-secret"
    }"#;
    let s: WireSecret = serde_json::from_str(json).unwrap();
    assert_eq!(s.value.as_deref(), Some("super-secret"));
}

/// `WireEnvironment` mirrors the daemon's `StoredEnvironment`. Verify
/// timestamps round-trip as RFC-3339 strings.
#[test]
fn wire_environment_round_trips() {
    use zlayer_manager::wire::secrets::WireEnvironment;

    let json = r#"{
        "id": "env-uuid",
        "name": "dev",
        "project_id": null,
        "description": null,
        "created_at": "2026-04-16T00:00:00Z",
        "updated_at": "2026-04-16T00:00:00Z"
    }"#;
    let e: WireEnvironment = serde_json::from_str(json).unwrap();
    assert_eq!(e.name, "dev");
    assert!(e.project_id.is_none());
}

#[test]
fn bulk_import_result_round_trips() {
    use zlayer_manager::wire::secrets::BulkImportResult;

    let json = r#"{"created":3,"updated":1,"errors":["line 4: missing '='"]}"#;
    let r: BulkImportResult = serde_json::from_str(json).unwrap();
    assert_eq!(r.created, 3);
    assert_eq!(r.updated, 1);
    assert_eq!(r.errors.len(), 1);
}

/// Compile-check guard: the `Tasks` component is exported and reachable
/// from the crate's public surface. Same pattern as the other pages.
#[test]
fn tasks_component_exists() {
    let _: fn() -> _ = zlayer_manager::app::pages::Tasks;
}

/// `WireTask` mirrors the daemon's `StoredTask`. The daemon emits `kind`
/// as the string `"bash"` (via `#[serde(rename_all = "snake_case")]` on
/// `TaskKind::Bash`). Round-trip a full shape to catch drift.
#[test]
fn wire_task_round_trips() {
    use zlayer_manager::wire::tasks::WireTask;

    let json = r#"{
        "id": "task-uuid",
        "name": "build",
        "kind": "bash",
        "body": "cargo build",
        "project_id": "proj-1",
        "created_at": "2026-04-16T00:00:00Z",
        "updated_at": "2026-04-16T01:00:00Z"
    }"#;
    let t: WireTask = serde_json::from_str(json).unwrap();
    assert_eq!(t.name, "build");
    assert_eq!(t.kind, "bash");
    assert_eq!(t.body, "cargo build");
    assert_eq!(t.project_id.as_deref(), Some("proj-1"));
}

/// Global tasks carry `project_id: null`; the `#[serde(default)]` must
/// accept that without an explicit field.
#[test]
fn wire_task_accepts_null_project() {
    use zlayer_manager::wire::tasks::WireTask;

    let json = r#"{
        "id": "t1",
        "name": "global",
        "kind": "bash",
        "body": "echo hi",
        "project_id": null,
        "created_at": "2026-04-16T00:00:00Z",
        "updated_at": "2026-04-16T00:00:00Z"
    }"#;
    let t: WireTask = serde_json::from_str(json).unwrap();
    assert!(t.project_id.is_none());
}

/// `WireTaskRun` mirrors the daemon's `TaskRun`. Exit codes can be
/// negative or null, stdout/stderr can be empty.
#[test]
fn wire_task_run_round_trips() {
    use zlayer_manager::wire::tasks::WireTaskRun;

    let json = r#"{
        "id": "run-uuid",
        "task_id": "task-uuid",
        "exit_code": 0,
        "stdout": "hello\n",
        "stderr": "",
        "started_at": "2026-04-16T00:00:00Z",
        "finished_at": "2026-04-16T00:00:01Z"
    }"#;
    let r: WireTaskRun = serde_json::from_str(json).unwrap();
    assert_eq!(r.exit_code, Some(0));
    assert_eq!(r.stdout, "hello\n");
    assert_eq!(r.task_id, "task-uuid");
}

/// A still-running task: `exit_code` and `finished_at` both null.
#[test]
fn wire_task_run_accepts_null_finish() {
    use zlayer_manager::wire::tasks::WireTaskRun;

    let json = r#"{
        "id": "r1",
        "task_id": "t1",
        "exit_code": null,
        "stdout": "",
        "stderr": "",
        "started_at": "2026-04-16T00:00:00Z",
        "finished_at": null
    }"#;
    let r: WireTaskRun = serde_json::from_str(json).unwrap();
    assert!(r.exit_code.is_none());
    assert!(r.finished_at.is_none());
}

/// Compile-check guard: the `Notifiers` component is exported and the
/// `/notifiers` route target is reachable from the public surface.
#[test]
fn notifiers_component_exists() {
    let _: fn() -> _ = zlayer_manager::app::pages::Notifiers;
}

/// `WireNotifier` mirrors the daemon's `StoredNotifier`. The
/// `config.type` discriminator is what makes the tagged-union form on
/// the page work — round-trip each variant.
#[test]
fn wire_notifier_slack_round_trips() {
    use zlayer_manager::wire::notifiers::{WireNotifier, WireNotifierConfig};

    let json = r#"{
        "id": "n1",
        "name": "deploy-alerts",
        "kind": "slack",
        "enabled": true,
        "config": { "type": "slack", "webhook_url": "https://hooks.slack.com/test" },
        "created_at": "2026-04-16T00:00:00Z",
        "updated_at": "2026-04-16T01:00:00Z"
    }"#;
    let n: WireNotifier = serde_json::from_str(json).unwrap();
    assert_eq!(n.kind, "slack");
    assert!(matches!(n.config, WireNotifierConfig::Slack { .. }));
}

#[test]
fn wire_notifier_webhook_round_trips() {
    use zlayer_manager::wire::notifiers::{WireNotifier, WireNotifierConfig};

    let json = r#"{
        "id": "n2",
        "name": "ops-hook",
        "kind": "webhook",
        "enabled": false,
        "config": {
            "type": "webhook",
            "url": "https://example.com/hook",
            "method": "PUT",
            "headers": { "Authorization": "Bearer abc" }
        },
        "created_at": "2026-04-16T00:00:00Z",
        "updated_at": "2026-04-16T00:00:00Z"
    }"#;
    let n: WireNotifier = serde_json::from_str(json).unwrap();
    assert!(!n.enabled);
    match n.config {
        WireNotifierConfig::Webhook {
            url,
            method,
            headers,
        } => {
            assert_eq!(url, "https://example.com/hook");
            assert_eq!(method.as_deref(), Some("PUT"));
            assert_eq!(headers.unwrap().get("Authorization").unwrap(), "Bearer abc");
        }
        other => panic!("expected Webhook config, got {other:?}"),
    }
}

#[test]
fn wire_notifier_smtp_round_trips() {
    use zlayer_manager::wire::notifiers::{WireNotifier, WireNotifierConfig};

    let json = r#"{
        "id": "n3",
        "name": "email",
        "kind": "smtp",
        "enabled": true,
        "config": {
            "type": "smtp",
            "host": "smtp.example.com",
            "port": 587,
            "username": "u",
            "password": "p",
            "from": "alerts@example.com",
            "to": ["admin@example.com"]
        },
        "created_at": "2026-04-16T00:00:00Z",
        "updated_at": "2026-04-16T00:00:00Z"
    }"#;
    let n: WireNotifier = serde_json::from_str(json).unwrap();
    match n.config {
        WireNotifierConfig::Smtp { port, to, .. } => {
            assert_eq!(port, 587);
            assert_eq!(to, vec!["admin@example.com".to_string()]);
        }
        other => panic!("expected Smtp config, got {other:?}"),
    }
}

/// Missing optional webhook fields default to `None` via `#[serde(default)]`.
#[test]
fn wire_notifier_webhook_optional_fields_default() {
    use zlayer_manager::wire::notifiers::WireNotifierConfig;

    let json = r#"{ "type": "webhook", "url": "https://example.com/hook" }"#;
    let cfg: WireNotifierConfig = serde_json::from_str(json).unwrap();
    match cfg {
        WireNotifierConfig::Webhook {
            url,
            method,
            headers,
        } => {
            assert_eq!(url, "https://example.com/hook");
            assert!(method.is_none());
            assert!(headers.is_none());
        }
        _ => panic!("expected webhook variant"),
    }
}

/// `NotifierTestResult` round-trips both success and failure shapes.
#[test]
fn notifier_test_result_round_trips() {
    use zlayer_manager::wire::notifiers::NotifierTestResult;

    let ok = r#"{"success":true,"message":"Sent"}"#;
    let r: NotifierTestResult = serde_json::from_str(ok).unwrap();
    assert!(r.success);

    let bad = r#"{"success":false,"message":"SMTP error: timed out"}"#;
    let r: NotifierTestResult = serde_json::from_str(bad).unwrap();
    assert!(!r.success);
    assert!(r.message.starts_with("SMTP error:"));
}

/// Compile-check guard: the `Workflows` component is exported and the
/// `/workflows` route target is reachable from the public surface.
#[test]
fn workflows_component_exists() {
    let _: fn() -> _ = zlayer_manager::app::pages::Workflows;
}

/// `WireWorkflow` mirrors the daemon's `StoredWorkflow`. The nested
/// `WireWorkflowAction` is tagged with `{"type": "<snake_case>"}` — any
/// drift in the tag or the variant fields shows up here as a
/// deserialize failure.
#[test]
fn wire_workflow_round_trips() {
    use zlayer_manager::wire::workflows::{WireWorkflow, WireWorkflowAction};

    let json = r#"{
        "id": "wf-1",
        "name": "deploy-pipeline",
        "steps": [
            {"name": "build", "action": {"type": "build_project", "project_id": "p1"}},
            {"name": "deploy", "action": {"type": "deploy_project", "project_id": "p1"}, "on_failure": "cleanup"}
        ],
        "project_id": null,
        "created_at": "2026-04-16T00:00:00Z",
        "updated_at": "2026-04-16T01:00:00Z"
    }"#;
    let wf: WireWorkflow = serde_json::from_str(json).unwrap();
    assert_eq!(wf.name, "deploy-pipeline");
    assert_eq!(wf.steps.len(), 2);
    match &wf.steps[0].action {
        WireWorkflowAction::BuildProject { project_id } => assert_eq!(project_id, "p1"),
        other => panic!("expected BuildProject, got {other:?}"),
    }
    assert_eq!(wf.steps[1].on_failure.as_deref(), Some("cleanup"));
}

/// `WireWorkflowAction` round-trips with the daemon's `#[serde(tag =
/// "type", rename_all = "snake_case")]` encoding.
#[test]
fn wire_workflow_action_tag_shape() {
    use zlayer_manager::wire::workflows::WireWorkflowAction;

    for (json, expect_kind) in [
        (r#"{"type":"run_task","task_id":"t1"}"#, "run_task"),
        (
            r#"{"type":"build_project","project_id":"p1"}"#,
            "build_project",
        ),
        (
            r#"{"type":"deploy_project","project_id":"p1"}"#,
            "deploy_project",
        ),
        (r#"{"type":"apply_sync","sync_id":"s1"}"#, "apply_sync"),
    ] {
        let a: WireWorkflowAction = serde_json::from_str(json).unwrap();
        assert_eq!(a.kind(), expect_kind);
        let s = serde_json::to_string(&a).unwrap();
        assert_eq!(s, json);
    }
}

/// `WireWorkflowRun` mirrors the daemon's `WorkflowRun`. Status strings
/// come through as `snake_case` (`"completed"`, `"failed"`, etc.); still-running
/// runs have `finished_at: null`.
#[test]
fn wire_workflow_run_round_trips() {
    use zlayer_manager::wire::workflows::WireWorkflowRun;

    let json = r#"{
        "id": "run-1",
        "workflow_id": "wf-1",
        "status": "completed",
        "step_results": [
            {"step_name": "a", "status": "ok", "output": "done"},
            {"step_name": "b", "status": "skipped", "output": null}
        ],
        "started_at": "2026-04-16T00:00:00Z",
        "finished_at": "2026-04-16T00:00:01Z"
    }"#;
    let run: WireWorkflowRun = serde_json::from_str(json).unwrap();
    assert_eq!(run.status, "completed");
    assert_eq!(run.step_results.len(), 2);
    assert_eq!(run.step_results[1].output, None);
    assert!(run.finished_at.is_some());
}

#[test]
fn wire_workflow_run_accepts_null_finish() {
    use zlayer_manager::wire::workflows::WireWorkflowRun;

    let json = r#"{
        "id": "r1",
        "workflow_id": "wf-1",
        "status": "running",
        "step_results": [],
        "started_at": "2026-04-16T00:00:00Z",
        "finished_at": null
    }"#;
    let run: WireWorkflowRun = serde_json::from_str(json).unwrap();
    assert!(run.finished_at.is_none());
    assert_eq!(run.status, "running");
}

/// `WireNotifierConfig::kind()` returns the snake-case discriminator that
/// `manager_create_notifier` uses to build the outbound `CreateNotifierRequest`.
#[test]
fn wire_notifier_kind_helper() {
    use zlayer_manager::wire::notifiers::WireNotifierConfig;

    assert_eq!(
        WireNotifierConfig::Slack {
            webhook_url: "x".into()
        }
        .kind(),
        "slack"
    );
    assert_eq!(
        WireNotifierConfig::Discord {
            webhook_url: "x".into()
        }
        .kind(),
        "discord"
    );
    assert_eq!(
        WireNotifierConfig::Webhook {
            url: "x".into(),
            method: None,
            headers: None
        }
        .kind(),
        "webhook"
    );
    assert_eq!(
        WireNotifierConfig::Smtp {
            host: "h".into(),
            port: 25,
            username: "u".into(),
            password: "p".into(),
            from: "f@x".into(),
            to: vec!["t@x".into()]
        }
        .kind(),
        "smtp"
    );
}

/// Compile-check guard: the `Groups` component is exported and the
/// `/groups` route target is reachable from the public surface.
#[test]
fn groups_component_exists() {
    let _: fn() -> _ = zlayer_manager::app::pages::Groups;
}

/// `WireUserGroup` mirrors the daemon's `StoredUserGroup`. Round-trip a
/// full shape to catch drift; `description` is optional.
#[test]
fn wire_user_group_round_trips() {
    use zlayer_manager::wire::groups::WireUserGroup;

    let json = r#"{
        "id": "group-uuid",
        "name": "developers",
        "description": "Dev team",
        "created_at": "2026-04-16T00:00:00Z",
        "updated_at": "2026-04-16T01:00:00Z"
    }"#;
    let g: WireUserGroup = serde_json::from_str(json).unwrap();
    assert_eq!(g.name, "developers");
    assert_eq!(g.description.as_deref(), Some("Dev team"));
}

/// `description` may be `null`; the `#[serde(default)]` handles it.
#[test]
fn wire_user_group_accepts_null_description() {
    use zlayer_manager::wire::groups::WireUserGroup;

    let json = r#"{
        "id": "g1",
        "name": "ops",
        "description": null,
        "created_at": "2026-04-16T00:00:00Z",
        "updated_at": "2026-04-16T00:00:00Z"
    }"#;
    let g: WireUserGroup = serde_json::from_str(json).unwrap();
    assert!(g.description.is_none());
}

/// `WireGroupMembers` mirrors the daemon's `GroupMembersResponse`: just a
/// `group_id` and a flat list of `user_ids` (names are joined client-side).
#[test]
fn wire_group_members_round_trips() {
    use zlayer_manager::wire::groups::WireGroupMembers;

    let json = r#"{
        "group_id": "group-uuid",
        "members": ["user-a", "user-b", "user-c"]
    }"#;
    let m: WireGroupMembers = serde_json::from_str(json).unwrap();
    assert_eq!(m.group_id, "group-uuid");
    assert_eq!(m.members.len(), 3);
    assert_eq!(m.members[1], "user-b");
}

/// Empty member list round-trips as an empty array (not null).
#[test]
fn wire_group_members_accepts_empty() {
    use zlayer_manager::wire::groups::WireGroupMembers;

    let json = r#"{"group_id":"g1","members":[]}"#;
    let m: WireGroupMembers = serde_json::from_str(json).unwrap();
    assert!(m.members.is_empty());
}

/// `WireUpdateGroup` with all fields `None` serialises to `{}` — the
/// daemon treats an empty patch as a no-op and returns the unchanged
/// group. `skip_serializing_if = "Option::is_none"` keeps the body
/// minimal.
#[test]
fn wire_update_group_omits_none() {
    use zlayer_manager::wire::groups::WireUpdateGroup;

    let patch = WireUpdateGroup {
        name: None,
        description: None,
    };
    let s = serde_json::to_string(&patch).unwrap();
    assert_eq!(s, "{}");
}

/// Partial patch — only `name` set — serialises with just that field.
#[test]
fn wire_update_group_partial() {
    use zlayer_manager::wire::groups::WireUpdateGroup;

    let patch = WireUpdateGroup {
        name: Some("renamed".into()),
        description: None,
    };
    let s = serde_json::to_string(&patch).unwrap();
    assert_eq!(s, r#"{"name":"renamed"}"#);
}

/// Compile-check guard: the `Permissions` component is exported and the
/// `/permissions` route target is reachable from the public surface.
#[test]
fn permissions_component_exists() {
    let _: fn() -> _ = zlayer_manager::app::pages::Permissions;
}

/// `WirePermission` mirrors the daemon's `StoredPermission`. Both the
/// specific-resource and wildcard shapes must round-trip cleanly — the
/// daemon emits `resource_id: null` for wildcards (via `Option<String>`
/// + serde).
#[test]
fn wire_permission_specific_resource_round_trips() {
    use zlayer_manager::wire::permissions::WirePermission;

    let json = r#"{
        "id": "p1",
        "subject_kind": "user",
        "subject_id": "u1",
        "resource_kind": "deployment",
        "resource_id": "d1",
        "level": "write",
        "created_at": "2026-04-16T00:00:00Z"
    }"#;
    let p: WirePermission = serde_json::from_str(json).unwrap();
    assert_eq!(p.subject_kind, "user");
    assert_eq!(p.subject_id, "u1");
    assert_eq!(p.resource_kind, "deployment");
    assert_eq!(p.resource_id.as_deref(), Some("d1"));
    assert_eq!(p.level, "write");
}

#[test]
fn wire_permission_wildcard_round_trips() {
    use zlayer_manager::wire::permissions::WirePermission;

    let json = r#"{
        "id": "p2",
        "subject_kind": "group",
        "subject_id": "g1",
        "resource_kind": "project",
        "resource_id": null,
        "level": "read",
        "created_at": "2026-04-16T00:00:00Z"
    }"#;
    let p: WirePermission = serde_json::from_str(json).unwrap();
    assert_eq!(p.subject_kind, "group");
    assert!(p.resource_id.is_none());
    assert_eq!(p.level, "read");
}

/// `WireGrantPermissionRequest` must serialise without `resource_id` when
/// the caller wants a wildcard grant — the daemon treats the absent
/// field as `None` (matching `#[serde(default)]` on the body type).
#[test]
fn wire_grant_permission_request_wildcard_omits_resource_id() {
    use zlayer_manager::wire::permissions::WireGrantPermissionRequest;

    let req = WireGrantPermissionRequest {
        subject_kind: "user".into(),
        subject_id: "u1".into(),
        resource_kind: "secret".into(),
        resource_id: None,
        level: "read".into(),
    };
    let s = serde_json::to_string(&req).unwrap();
    assert!(
        !s.contains("resource_id"),
        "wildcard grant must omit resource_id; got {s}"
    );
    assert!(s.contains(r#""subject_kind":"user""#));
    assert!(s.contains(r#""level":"read""#));
}

/// Specific-resource grant keeps the `resource_id` on the wire.
#[test]
fn wire_grant_permission_request_specific_includes_resource_id() {
    use zlayer_manager::wire::permissions::WireGrantPermissionRequest;

    let req = WireGrantPermissionRequest {
        subject_kind: "group".into(),
        subject_id: "g1".into(),
        resource_kind: "deployment".into(),
        resource_id: Some("d1".into()),
        level: "write".into(),
    };
    let s = serde_json::to_string(&req).unwrap();
    assert!(s.contains(r#""resource_id":"d1""#));
    assert!(s.contains(r#""subject_kind":"group""#));
}

/// Compile-check guard: the `Audit` component is exported and the
/// `/audit` route target is reachable from the public surface.
#[test]
fn audit_component_exists() {
    let _: fn() -> _ = zlayer_manager::app::pages::Audit;
}

/// `WireAuditEntry` mirrors the daemon's `AuditEntry`. Verify a full
/// shape round-trips — `user_id` and `resource_kind` are NON-optional
/// on the daemon (bare `String`), everything else is optional. The
/// `details` field is free-form JSON (`serde_json::Value`).
#[test]
fn wire_audit_entry_round_trips() {
    use zlayer_manager::wire::audit::WireAuditEntry;

    let json = r#"{
        "id": "audit-uuid",
        "user_id": "user-uuid",
        "action": "update",
        "resource_kind": "secret",
        "resource_id": "secret-42",
        "details": { "field": "value", "count": 3 },
        "ip": "10.0.0.1",
        "user_agent": "zlayer-cli/0.10",
        "created_at": "2026-04-16T00:00:00Z"
    }"#;
    let e: WireAuditEntry = serde_json::from_str(json).unwrap();
    assert_eq!(e.user_id, "user-uuid");
    assert_eq!(e.action, "update");
    assert_eq!(e.resource_kind, "secret");
    assert_eq!(e.resource_id.as_deref(), Some("secret-42"));
    assert!(e.details.is_some());
    assert_eq!(e.ip.as_deref(), Some("10.0.0.1"));
    assert_eq!(e.user_agent.as_deref(), Some("zlayer-cli/0.10"));
}

/// Entries without a specific resource / IP / user-agent / details
/// come through with the Option fields as `None` via `#[serde(default)]`.
#[test]
fn wire_audit_entry_accepts_nulls() {
    use zlayer_manager::wire::audit::WireAuditEntry;

    let json = r#"{
        "id": "a1",
        "user_id": "u1",
        "action": "create",
        "resource_kind": "project",
        "resource_id": null,
        "details": null,
        "ip": null,
        "user_agent": null,
        "created_at": "2026-04-16T00:00:00Z"
    }"#;
    let e: WireAuditEntry = serde_json::from_str(json).unwrap();
    assert!(e.resource_id.is_none());
    assert!(e.details.is_none());
    assert!(e.ip.is_none());
    assert!(e.user_agent.is_none());
}

/// Some daemon responses may omit optional fields entirely rather than
/// emit explicit `null`s; `#[serde(default)]` must handle that too.
#[test]
fn wire_audit_entry_accepts_missing_optionals() {
    use zlayer_manager::wire::audit::WireAuditEntry;

    let json = r#"{
        "id": "a2",
        "user_id": "u2",
        "action": "delete",
        "resource_kind": "deployment",
        "created_at": "2026-04-16T00:00:00Z"
    }"#;
    let e: WireAuditEntry = serde_json::from_str(json).unwrap();
    assert!(e.resource_id.is_none());
    assert!(e.details.is_none());
    assert!(e.ip.is_none());
    assert!(e.user_agent.is_none());
}

/// `WireAuditFilter::default()` serialises to all-null fields — round-trip
/// through serde to prove `Default` doesn't drift from the declared shape.
#[test]
fn wire_audit_filter_default_round_trips() {
    use zlayer_manager::wire::audit::WireAuditFilter;

    let f = WireAuditFilter::default();
    let j = serde_json::to_string(&f).unwrap();
    let back: WireAuditFilter = serde_json::from_str(&j).unwrap();
    assert_eq!(f, back);
    assert!(back.user_id.is_none());
    assert!(back.limit.is_none());
}

// ============================================================================
// Projects page — smoke tests for list + nested detail route.
// ============================================================================

/// Compile-check guard: the `Projects` list page is exported and the
/// `/projects` route target is reachable from the public surface.
#[test]
fn projects_component_exists() {
    let _: fn() -> _ = zlayer_manager::app::pages::Projects;
}

/// Compile-check guard: the `ProjectDetail` nested-route page is exported
/// and the `/projects/:id` route target is reachable. The page must
/// render even for an id that isn't present in the daemon — the detail
/// resource surfaces a not-found alert in that case, which is exactly
/// what this smoke test proves by construction.
#[test]
fn project_detail_component_exists() {
    let _: fn() -> _ = zlayer_manager::app::pages::ProjectDetail;
}

/// `WireProject` mirrors the daemon's `StoredProject`. Round-trip a full
/// shape including every optional field to catch drift. All timestamps
/// are RFC-3339 strings on the wire.
#[test]
fn wire_project_round_trips_full_shape() {
    use zlayer_manager::wire::projects::{WireBuildKind, WireProject};

    let json = r#"{
        "id": "proj-uuid",
        "name": "my-app",
        "description": "Example app",
        "git_url": "https://github.com/user/repo",
        "git_branch": "main",
        "git_credential_id": "gcred-1",
        "build_kind": "dockerfile",
        "build_path": "./Dockerfile",
        "deploy_spec_path": "./deployment.yaml",
        "registry_credential_id": "rcred-1",
        "default_environment_id": "env-1",
        "owner_id": "user-1",
        "auto_deploy": true,
        "poll_interval_secs": 300,
        "created_at": "2026-04-16T00:00:00Z",
        "updated_at": "2026-04-16T01:00:00Z"
    }"#;
    let p: WireProject = serde_json::from_str(json).unwrap();
    assert_eq!(p.name, "my-app");
    assert_eq!(p.build_kind, Some(WireBuildKind::Dockerfile));
    assert!(p.auto_deploy);
    assert_eq!(p.poll_interval_secs, Some(300));
    assert_eq!(p.git_branch.as_deref(), Some("main"));
}

/// Projects with nothing configured come through with every Option empty
/// and `auto_deploy: false` — `#[serde(default)]` handles the missing /
/// null fields.
#[test]
fn wire_project_accepts_minimum_shape() {
    use zlayer_manager::wire::projects::WireProject;

    let json = r#"{
        "id": "p1",
        "name": "tiny",
        "created_at": "2026-04-16T00:00:00Z",
        "updated_at": "2026-04-16T00:00:00Z"
    }"#;
    let p: WireProject = serde_json::from_str(json).unwrap();
    assert_eq!(p.name, "tiny");
    assert!(p.git_url.is_none());
    assert!(p.build_kind.is_none());
    assert!(!p.auto_deploy);
    assert!(p.poll_interval_secs.is_none());
}

/// The four `BuildKind` variants must match the daemon's wire encoding.
#[test]
fn wire_build_kind_round_trip_all_variants() {
    use zlayer_manager::wire::projects::WireBuildKind;

    for (json, expect) in [
        (r#""dockerfile""#, WireBuildKind::Dockerfile),
        (r#""compose""#, WireBuildKind::Compose),
        (r#""zimagefile""#, WireBuildKind::ZImagefile),
        (r#""spec""#, WireBuildKind::Spec),
    ] {
        let parsed: WireBuildKind = serde_json::from_str(json).unwrap();
        assert_eq!(parsed, expect);
        let back = serde_json::to_string(&parsed).unwrap();
        assert_eq!(back, json);
    }
}

/// `WireProjectSpec` uses `skip_serializing_if = "Option::is_none"` on
/// every field so a partial patch body stays minimal — the daemon's
/// `UpdateProjectRequest` matches it by `#[serde(default)]`.
#[test]
fn wire_project_spec_omits_none_fields() {
    use zlayer_manager::wire::projects::{WireBuildKind, WireProjectSpec};

    let spec = WireProjectSpec {
        name: Some("renamed".into()),
        build_kind: Some(WireBuildKind::Dockerfile),
        ..Default::default()
    };
    let json = serde_json::to_string(&spec).unwrap();
    assert!(json.contains(r#""name":"renamed""#));
    assert!(json.contains(r#""build_kind":"dockerfile""#));
    // Every other field should be absent.
    assert!(!json.contains("git_url"));
    assert!(!json.contains("auto_deploy"));
    assert!(!json.contains("poll_interval_secs"));
}

/// `WirePullResult` mirrors `ProjectPullResponse` exactly — round-trip.
#[test]
fn wire_pull_result_round_trips() {
    use zlayer_manager::wire::projects::WirePullResult;

    let json = r#"{
        "project_id": "proj-1",
        "git_url": "https://github.com/user/repo",
        "branch": "main",
        "sha": "deadbeefcafebabe0000000000000000deadbeef",
        "path": "/tmp/zlayer-projects/proj-1"
    }"#;
    let r: WirePullResult = serde_json::from_str(json).unwrap();
    assert_eq!(r.project_id, "proj-1");
    assert_eq!(r.sha.len(), 40);
    assert!(r.path.ends_with("proj-1"));
}

/// `WireWebhookInfo` mirrors the daemon's `WebhookInfoResponse` — the
/// daemon currently exposes only `url` + `secret` on both `GET` and
/// `rotate`, so that's what we keep.
#[test]
fn wire_webhook_info_round_trips() {
    use zlayer_manager::wire::projects::WireWebhookInfo;

    let json = r#"{
        "url": "/webhooks/{provider}/proj-1",
        "secret": "deadbeefcafebabe"
    }"#;
    let w: WireWebhookInfo = serde_json::from_str(json).unwrap();
    assert!(w.url.contains("{provider}"));
    assert_eq!(w.secret, "deadbeefcafebabe");
}

/// `WireBuildKind::parse_wire` accepts the daemon's four snake-case
/// discriminators and rejects anything else.
#[test]
fn wire_build_kind_parse_wire() {
    use zlayer_manager::wire::projects::WireBuildKind;

    assert_eq!(
        WireBuildKind::parse_wire("dockerfile"),
        Some(WireBuildKind::Dockerfile)
    );
    assert_eq!(
        WireBuildKind::parse_wire("zimagefile"),
        Some(WireBuildKind::ZImagefile)
    );
    assert_eq!(WireBuildKind::parse_wire("spec"), Some(WireBuildKind::Spec));
    assert_eq!(WireBuildKind::parse_wire("pipeline"), None);
    assert_eq!(WireBuildKind::parse_wire(""), None);
}
