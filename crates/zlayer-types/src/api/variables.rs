//! DTOs for the variables API.
//!
//! Variables are plaintext key-value pairs for template substitution in
//! deployment specs. Unlike secrets, variable values are NOT encrypted and
//! are fully visible in API responses.
//!
//! Variables can be global (`scope = None`) or project-scoped
//! (`scope = Some(project_id)`).

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Query for `GET /api/v1/variables`.
///
/// `scope` selects the namespace:
///   - omitted -> list global variables only (`scope IS NULL`).
///   - any other value -> list variables belonging to that project/scope id.
#[derive(Debug, Deserialize, Default)]
pub struct ListVariablesQuery {
    #[serde(default)]
    pub scope: Option<String>,
}

/// Body for `POST /api/v1/variables`.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CreateVariableRequest {
    /// Variable name (e.g. `"APP_VERSION"`). Must be unique within the chosen
    /// scope.
    pub name: String,
    /// Plaintext value.
    pub value: String,
    /// Project id scope. `None` = global variable.
    #[serde(default)]
    pub scope: Option<String>,
}

/// Body for `PATCH /api/v1/variables/{id}`. All fields are optional.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct UpdateVariableRequest {
    /// New variable name. Will be re-checked for uniqueness.
    #[serde(default)]
    pub name: Option<String>,
    /// New value.
    #[serde(default)]
    pub value: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_request_deserialize_minimum() {
        let req: CreateVariableRequest =
            serde_json::from_str(r#"{"name":"FOO","value":"bar"}"#).unwrap();
        assert_eq!(req.name, "FOO");
        assert_eq!(req.value, "bar");
        assert!(req.scope.is_none());
    }

    #[test]
    fn test_create_request_deserialize_full() {
        let req: CreateVariableRequest =
            serde_json::from_str(r#"{"name":"FOO","value":"bar","scope":"proj-1"}"#).unwrap();
        assert_eq!(req.name, "FOO");
        assert_eq!(req.value, "bar");
        assert_eq!(req.scope.as_deref(), Some("proj-1"));
    }

    #[test]
    fn test_update_request_deserialize_partial() {
        let req: UpdateVariableRequest = serde_json::from_str("{}").unwrap();
        assert!(req.name.is_none());
        assert!(req.value.is_none());

        let req: UpdateVariableRequest = serde_json::from_str(r#"{"value":"new"}"#).unwrap();
        assert!(req.name.is_none());
        assert_eq!(req.value.as_deref(), Some("new"));
    }

    #[test]
    fn test_list_query_default_is_empty() {
        let q = ListVariablesQuery::default();
        assert!(q.scope.is_none());
    }

    #[test]
    fn test_list_query_construct_with_scope() {
        let q = ListVariablesQuery {
            scope: Some("p1".to_string()),
        };
        assert_eq!(q.scope.as_deref(), Some("p1"));
    }
}
