//! Task CRUD and execution DTOs.

use serde::{Deserialize, Serialize};

use crate::storage::TaskKind;

/// Query for `GET /api/v1/tasks`.
#[derive(Debug, Deserialize, Default)]
pub struct ListTasksQuery {
    /// Filter by project id. When omitted, all tasks (global + project-scoped)
    /// are returned.
    #[serde(default)]
    pub project_id: Option<String>,
}

/// Body for `POST /api/v1/tasks`.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct CreateTaskRequest {
    /// Task name.
    pub name: String,
    /// Script type.
    pub kind: TaskKind,
    /// The script/command body.
    pub body: String,
    /// Project id scope. `None` = global task.
    #[serde(default)]
    pub project_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_request_deserialize_minimum() {
        let req: CreateTaskRequest =
            serde_json::from_str(r#"{"name":"build","kind":"bash","body":"cargo build"}"#).unwrap();
        assert_eq!(req.name, "build");
        assert_eq!(req.kind, TaskKind::Bash);
        assert_eq!(req.body, "cargo build");
        assert!(req.project_id.is_none());
    }

    #[test]
    fn test_create_request_deserialize_full() {
        let req: CreateTaskRequest = serde_json::from_str(
            r#"{"name":"build","kind":"bash","body":"cargo build","project_id":"proj-1"}"#,
        )
        .unwrap();
        assert_eq!(req.name, "build");
        assert_eq!(req.kind, TaskKind::Bash);
        assert_eq!(req.body, "cargo build");
        assert_eq!(req.project_id.as_deref(), Some("proj-1"));
    }

    #[test]
    fn test_list_query_default_is_empty() {
        let q = ListTasksQuery::default();
        assert!(q.project_id.is_none());
    }
}
