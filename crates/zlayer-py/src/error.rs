//! Error types for Python bindings
//!
//! This module defines error types that map Rust errors to Python exceptions.

use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use thiserror::Error;

/// Error type for ZLayer Python bindings
#[derive(Error, Debug)]
pub enum ZLayerError {
    #[error("Container error: {0}")]
    Container(String),

    #[error("Runtime error: {0}")]
    Runtime(String),

    #[error("Build error: {0}")]
    Build(String),

    #[error("Spec error: {0}")]
    Spec(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Timeout waiting for condition: {0}")]
    Timeout(String),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Invalid argument: {0}")]
    InvalidArgument(String),
}

impl From<ZLayerError> for PyErr {
    fn from(err: ZLayerError) -> PyErr {
        match err {
            ZLayerError::InvalidArgument(msg) => PyValueError::new_err(msg),
            ZLayerError::NotFound(msg) => PyValueError::new_err(format!("Not found: {}", msg)),
            ZLayerError::Spec(msg) => PyValueError::new_err(format!("Spec error: {}", msg)),
            _ => PyRuntimeError::new_err(err.to_string()),
        }
    }
}

impl From<zlayer_agent::AgentError> for ZLayerError {
    fn from(err: zlayer_agent::AgentError) -> Self {
        ZLayerError::Container(err.to_string())
    }
}

impl From<zlayer_spec::SpecError> for ZLayerError {
    fn from(err: zlayer_spec::SpecError) -> Self {
        ZLayerError::Spec(err.to_string())
    }
}

impl From<zlayer_builder::BuildError> for ZLayerError {
    fn from(err: zlayer_builder::BuildError) -> Self {
        ZLayerError::Build(err.to_string())
    }
}

impl From<serde_yaml::Error> for ZLayerError {
    fn from(err: serde_yaml::Error) -> Self {
        ZLayerError::Spec(err.to_string())
    }
}

impl From<serde_json::Error> for ZLayerError {
    fn from(err: serde_json::Error) -> Self {
        ZLayerError::Spec(err.to_string())
    }
}

/// Result type alias for Python bindings
pub type Result<T> = std::result::Result<T, ZLayerError>;

/// Convert a Result to a PyResult
pub fn to_py_result<T>(result: Result<T>) -> PyResult<T> {
    result.map_err(|e| e.into())
}
