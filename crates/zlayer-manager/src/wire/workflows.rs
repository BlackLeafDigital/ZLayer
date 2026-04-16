//! Wire types for the Workflows page.
//!
//! Mirror of `zlayer_api::storage::{StoredWorkflow, WorkflowStep,
//! WorkflowAction, WorkflowRun, WorkflowRunStatus, StepResult}`, defined
//! locally so the hydrate/WASM build doesn't need `zlayer-api` (SSR-only).
//!
//! Field shape was cross-checked against
//! `crates/zlayer-api/src/storage/mod.rs` and
//! `crates/zlayer-api/src/handlers/workflows.rs` (`CreateWorkflowRequest`).
//!
//! Action tagging: `WorkflowAction` is serialised with
//! `#[serde(tag = "type", rename_all = "snake_case")]` so the JSON payload
//! looks like `{"type":"run_task","task_id":"…"}`.
//!
//! Run status: `WorkflowRunStatus` is serialised with
//! `#[serde(rename_all = "snake_case")]`, so the wire strings are
//! `"pending"`, `"running"`, `"completed"`, `"failed"`.
//!
//! NOTE: the daemon does NOT expose an `update_workflow` / PATCH endpoint.
//! The page therefore only supports create / delete / run; edits are done
//! by deleting and recreating (same pattern as Tasks).

use serde::{Deserialize, Serialize};

/// A stored workflow returned by `/api/v1/workflows`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WireWorkflow {
    /// UUID identifier.
    pub id: String,
    /// Workflow name.
    pub name: String,
    /// Ordered list of steps.
    pub steps: Vec<WireWorkflowStep>,
    /// Optional project scope.
    #[serde(default)]
    pub project_id: Option<String>,
    /// RFC-3339 creation timestamp.
    pub created_at: String,
    /// RFC-3339 last-updated timestamp.
    pub updated_at: String,
}

/// A single step in a workflow.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WireWorkflowStep {
    /// Step name (display label).
    pub name: String,
    /// The action to perform.
    pub action: WireWorkflowAction,
    /// Optional task id to run if this step fails.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_failure: Option<String>,
}

/// The action a workflow step performs.
///
/// Tagged with `{"type": "<snake_case>"}`; the daemon's Rust side uses
/// `#[serde(tag = "type", rename_all = "snake_case")]`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WireWorkflowAction {
    /// Execute a task by id.
    RunTask {
        /// The task id to run.
        task_id: String,
    },
    /// Build a project by id.
    BuildProject {
        /// The project id to build.
        project_id: String,
    },
    /// Deploy a project by id.
    DeployProject {
        /// The project id to deploy.
        project_id: String,
    },
    /// Apply a sync resource by id.
    ApplySync {
        /// The sync id to apply.
        sync_id: String,
    },
}

impl WireWorkflowAction {
    /// The snake-case discriminator — matches `#[serde(tag = "type")]`.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            WireWorkflowAction::RunTask { .. } => "run_task",
            WireWorkflowAction::BuildProject { .. } => "build_project",
            WireWorkflowAction::DeployProject { .. } => "deploy_project",
            WireWorkflowAction::ApplySync { .. } => "apply_sync",
        }
    }

    /// Human-readable label used in the UI badges.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            WireWorkflowAction::RunTask { .. } => "Run task",
            WireWorkflowAction::BuildProject { .. } => "Build project",
            WireWorkflowAction::DeployProject { .. } => "Deploy project",
            WireWorkflowAction::ApplySync { .. } => "Apply sync",
        }
    }

    /// The id that the action's reference field carries (task id / project
    /// id / sync id). Useful for rendering and debugging.
    #[must_use]
    pub fn target_id(&self) -> &str {
        match self {
            WireWorkflowAction::RunTask { task_id } => task_id.as_str(),
            WireWorkflowAction::BuildProject { project_id }
            | WireWorkflowAction::DeployProject { project_id } => project_id.as_str(),
            WireWorkflowAction::ApplySync { sync_id } => sync_id.as_str(),
        }
    }
}

/// A recorded execution of a workflow.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WireWorkflowRun {
    /// UUID identifier of this run.
    pub id: String,
    /// The workflow that was executed.
    pub workflow_id: String,
    /// Overall run status: `"pending" | "running" | "completed" | "failed"`.
    pub status: String,
    /// Per-step results, in execution order.
    pub step_results: Vec<WireStepResult>,
    /// RFC-3339 start timestamp.
    pub started_at: String,
    /// RFC-3339 finish timestamp; `None` if still running.
    #[serde(default)]
    pub finished_at: Option<String>,
}

/// Result of executing a single step in a workflow run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WireStepResult {
    /// The step name.
    pub step_name: String,
    /// Step outcome: `"ok" | "failed" | "skipped"` (plus the synthetic
    /// `"<name> (on_failure: …)"` entries the daemon emits for
    /// `on_failure` handlers).
    pub status: String,
    /// Optional stdout / error text emitted by the step.
    #[serde(default)]
    pub output: Option<String>,
}

/// Body for `POST /api/v1/workflows`. Mirrors the daemon's
/// `CreateWorkflowRequest` exactly.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WireWorkflowSpec {
    /// Workflow name.
    pub name: String,
    /// Ordered list of steps.
    pub steps: Vec<WireWorkflowStep>,
    /// Optional project scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_tag_roundtrip_run_task() {
        let a = WireWorkflowAction::RunTask {
            task_id: "t1".to_string(),
        };
        let s = serde_json::to_string(&a).unwrap();
        assert_eq!(s, r#"{"type":"run_task","task_id":"t1"}"#);
        let back: WireWorkflowAction = serde_json::from_str(&s).unwrap();
        assert_eq!(a, back);
    }

    #[test]
    fn action_tag_roundtrip_build_project() {
        let a = WireWorkflowAction::BuildProject {
            project_id: "p1".to_string(),
        };
        let s = serde_json::to_string(&a).unwrap();
        assert_eq!(s, r#"{"type":"build_project","project_id":"p1"}"#);
    }

    #[test]
    fn action_tag_roundtrip_apply_sync() {
        let a = WireWorkflowAction::ApplySync {
            sync_id: "s1".to_string(),
        };
        let s = serde_json::to_string(&a).unwrap();
        assert_eq!(s, r#"{"type":"apply_sync","sync_id":"s1"}"#);
    }

    #[test]
    fn workflow_round_trips_full_shape() {
        let w = WireWorkflow {
            id: "wf-1".to_string(),
            name: "deploy-pipeline".to_string(),
            steps: vec![
                WireWorkflowStep {
                    name: "build".to_string(),
                    action: WireWorkflowAction::BuildProject {
                        project_id: "p1".to_string(),
                    },
                    on_failure: Some("cleanup-task".to_string()),
                },
                WireWorkflowStep {
                    name: "deploy".to_string(),
                    action: WireWorkflowAction::DeployProject {
                        project_id: "p1".to_string(),
                    },
                    on_failure: None,
                },
            ],
            project_id: Some("p1".to_string()),
            created_at: "2026-04-16T00:00:00Z".to_string(),
            updated_at: "2026-04-16T01:00:00Z".to_string(),
        };
        let json = serde_json::to_string(&w).unwrap();
        let back: WireWorkflow = serde_json::from_str(&json).unwrap();
        assert_eq!(w, back);
    }

    #[test]
    fn workflow_accepts_null_project_id() {
        let json = r#"{
            "id": "wf-1",
            "name": "global",
            "steps": [],
            "project_id": null,
            "created_at": "2026-04-16T00:00:00Z",
            "updated_at": "2026-04-16T00:00:00Z"
        }"#;
        let w: WireWorkflow = serde_json::from_str(json).unwrap();
        assert!(w.project_id.is_none());
    }

    #[test]
    fn run_round_trips_finished() {
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
        let r: WireWorkflowRun = serde_json::from_str(json).unwrap();
        assert_eq!(r.status, "completed");
        assert_eq!(r.step_results.len(), 2);
        assert_eq!(r.step_results[1].output, None);
    }

    #[test]
    fn run_accepts_null_finished_at() {
        let json = r#"{
            "id": "r1",
            "workflow_id": "wf-1",
            "status": "running",
            "step_results": [],
            "started_at": "2026-04-16T00:00:00Z",
            "finished_at": null
        }"#;
        let r: WireWorkflowRun = serde_json::from_str(json).unwrap();
        assert!(r.finished_at.is_none());
        assert_eq!(r.status, "running");
    }

    #[test]
    fn step_result_default_output_on_missing_field() {
        // The struct declares #[serde(default)] on `output`, so a missing
        // field (not just `null`) must also deserialize.
        let json = r#"{"step_name": "x", "status": "ok"}"#;
        let s: WireStepResult = serde_json::from_str(json).unwrap();
        assert_eq!(s.output, None);
    }

    #[test]
    fn action_helpers() {
        let run = WireWorkflowAction::RunTask {
            task_id: "t1".into(),
        };
        assert_eq!(run.kind(), "run_task");
        assert_eq!(run.label(), "Run task");
        assert_eq!(run.target_id(), "t1");

        let deploy = WireWorkflowAction::DeployProject {
            project_id: "p1".into(),
        };
        assert_eq!(deploy.kind(), "deploy_project");
        assert_eq!(deploy.target_id(), "p1");
    }

    #[test]
    fn spec_serialise_matches_create_request() {
        // A spec with `project_id: None` and steps that don't reference
        // a project themselves — lets us assert the top-level
        // `project_id` field is omitted without a false match on the
        // per-step `project_id` the BuildProject/DeployProject variants
        // carry.
        let spec = WireWorkflowSpec {
            name: "wf".into(),
            steps: vec![WireWorkflowStep {
                name: "run-tests".into(),
                action: WireWorkflowAction::RunTask {
                    task_id: "t1".into(),
                },
                on_failure: None,
            }],
            project_id: None,
        };
        let s = serde_json::to_string(&spec).unwrap();
        // `project_id: None` on the spec must be omitted (matches the
        // daemon's `#[serde(default)]` CreateWorkflowRequest).
        assert!(!s.contains("project_id"));
        assert!(s.contains(r#""type":"run_task""#));
        assert!(s.contains(r#""task_id":"t1""#));

        // When project scope IS set, the field is present.
        let scoped = WireWorkflowSpec {
            project_id: Some("proj-1".into()),
            ..WireWorkflowSpec::default()
        };
        let s = serde_json::to_string(&scoped).unwrap();
        assert!(s.contains(r#""project_id":"proj-1""#));

        // And a BuildProject-step spec carries `project_id` inside the
        // step.action payload — that's the shape the daemon expects.
        let build = WireWorkflowSpec {
            name: "build-wf".into(),
            steps: vec![WireWorkflowStep {
                name: "build".into(),
                action: WireWorkflowAction::BuildProject {
                    project_id: "p1".into(),
                },
                on_failure: None,
            }],
            project_id: None,
        };
        let s = serde_json::to_string(&build).unwrap();
        assert!(s.contains(r#""type":"build_project""#));
        assert!(s.contains(r#""project_id":"p1""#));
    }
}
