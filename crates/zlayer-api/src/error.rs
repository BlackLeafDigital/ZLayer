//! API error types

use axum::{
    http::{HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Response header that carries the cluster leader's HTTP base URL when a
/// follower returns `421 Misdirected Request` on a leader-only endpoint.
///
/// Clients (CLI, manager UI, other followers initiating a rollout) should
/// honour this header by retrying the same request against the advertised
/// address.
pub const LEADER_ADDR_HEADER: &str = "X-Leader-Addr";

/// API error type
#[derive(Debug, Error)]
pub enum ApiError {
    #[error("Unauthorized: {0}")]
    Unauthorized(String),

    #[error("Forbidden: {0}")]
    Forbidden(String),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Bad request: {0}")]
    BadRequest(String),

    #[error("Conflict: {0}")]
    Conflict(String),

    #[error("Rate limited")]
    RateLimited,

    #[error("Internal error: {0}")]
    Internal(String),

    #[error("Service unavailable: {0}")]
    ServiceUnavailable(String),

    #[error("Validation error: {0}")]
    Validation(String),

    #[error("Not implemented: {0}")]
    NotImplemented(String),

    /// This node is not the cluster Raft leader, so the requested
    /// leader-only operation cannot be executed here. The optional
    /// `leader_addr` field, when present, is the HTTP base URL of the
    /// current leader; clients should redirect the request there.
    ///
    /// Maps to `421 Misdirected Request` and also sets an
    /// `X-Leader-Addr` response header (see [`LEADER_ADDR_HEADER`]).
    #[error("Not the cluster leader (leader: {leader_addr:?})")]
    NotLeader { leader_addr: Option<String> },
}

/// JSON error response body
#[derive(Debug, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub error: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        // `NotLeader` is special-cased because:
        //   1. its status code (`421 Misdirected Request`) is not in the
        //      usual table, and
        //   2. it needs a structured `details` block AND a side-channel
        //      `X-Leader-Addr` response header so HTTP clients can perform
        //      a transparent redirect without re-parsing the JSON body.
        if let ApiError::NotLeader { leader_addr } = &self {
            let message =
                format!("Request must be sent to the cluster leader (leader: {leader_addr:?})");
            let body = ErrorResponse {
                error: "not_leader".to_string(),
                message,
                details: Some(serde_json::json!({ "leader_addr": leader_addr })),
            };
            let mut response = (StatusCode::MISDIRECTED_REQUEST, Json(body)).into_response();
            if let Some(addr) = leader_addr {
                if let Ok(hv) = HeaderValue::from_str(addr) {
                    response.headers_mut().insert(LEADER_ADDR_HEADER, hv);
                }
            }
            return response;
        }

        let (status, error_type) = match &self {
            ApiError::Unauthorized(_) => (StatusCode::UNAUTHORIZED, "unauthorized"),
            ApiError::Forbidden(_) => (StatusCode::FORBIDDEN, "forbidden"),
            ApiError::NotFound(_) => (StatusCode::NOT_FOUND, "not_found"),
            ApiError::BadRequest(_) => (StatusCode::BAD_REQUEST, "bad_request"),
            ApiError::Conflict(_) => (StatusCode::CONFLICT, "conflict"),
            ApiError::RateLimited => (StatusCode::TOO_MANY_REQUESTS, "rate_limited"),
            ApiError::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "internal_error"),
            ApiError::ServiceUnavailable(_) => {
                (StatusCode::SERVICE_UNAVAILABLE, "service_unavailable")
            }
            ApiError::Validation(_) => (StatusCode::UNPROCESSABLE_ENTITY, "validation_error"),
            ApiError::NotImplemented(_) => (StatusCode::NOT_IMPLEMENTED, "not_implemented"),
            // Covered above — keep the match exhaustive.
            ApiError::NotLeader { .. } => unreachable!("handled in the early-return branch above"),
        };

        let body = ErrorResponse {
            error: error_type.to_string(),
            message: self.to_string(),
            details: None,
        };

        (status, Json(body)).into_response()
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(err: anyhow::Error) -> Self {
        ApiError::Internal(err.to_string())
    }
}

impl From<serde_json::Error> for ApiError {
    fn from(err: serde_json::Error) -> Self {
        ApiError::BadRequest(format!("JSON error: {err}"))
    }
}

impl From<crate::storage::StorageError> for ApiError {
    fn from(err: crate::storage::StorageError) -> Self {
        use crate::storage::StorageError;
        match err {
            StorageError::NotFound(msg) => ApiError::NotFound(msg),
            StorageError::AlreadyExists(msg) => ApiError::Conflict(msg),
            StorageError::Serialization(msg) => {
                ApiError::Internal(format!("serialization error: {msg}"))
            }
            StorageError::Other(msg) => ApiError::Internal(msg),
            StorageError::Database(msg) => ApiError::Internal(format!("database error: {msg}")),
            StorageError::Io(e) => ApiError::Internal(format!("io error: {e}")),
        }
    }
}

impl From<crate::identity::IdentityError> for ApiError {
    fn from(err: crate::identity::IdentityError) -> Self {
        use crate::identity::IdentityError;
        match err {
            IdentityError::UserStore(e) => ApiError::from(e),
            IdentityError::Credentials(e) => ApiError::Internal(format!("credential store: {e}")),
            IdentityError::NotFound(msg) => ApiError::NotFound(msg),
            IdentityError::EmailExists(msg) => {
                ApiError::Conflict(format!("Email '{msg}' is already registered"))
            }
            IdentityError::Invalid(msg) => ApiError::BadRequest(msg),
        }
    }
}

pub type Result<T> = std::result::Result<T, ApiError>;
