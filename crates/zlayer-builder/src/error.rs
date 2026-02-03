//! Builder error types
//!
//! This module defines all error types for the Dockerfile builder subsystem,
//! covering parsing, context handling, build execution, and caching operations.

use std::path::PathBuf;
use thiserror::Error;

/// Build-specific errors
#[derive(Debug, Error)]
pub enum BuildError {
    /// Dockerfile parsing failed
    #[error("Dockerfile parse error at line {line}: {message}")]
    DockerfileParse {
        /// The underlying parsing error message
        message: String,
        /// Line number where the error occurred (1-indexed)
        line: usize,
    },

    /// Failed to read build context
    #[error("Failed to read build context at '{path}': {source}")]
    ContextRead {
        /// Path that could not be read
        path: PathBuf,
        /// Underlying IO error
        source: std::io::Error,
    },

    /// Path escape attempt detected (security violation)
    #[error("Path escape attempt: '{path}' escapes build context")]
    PathEscape {
        /// The offending path
        path: PathBuf,
    },

    /// File was ignored by .dockerignore
    #[error("File '{path}' is ignored by .dockerignore")]
    FileIgnored {
        /// The ignored file path
        path: PathBuf,
    },

    /// Referenced stage not found
    #[error("Stage '{name}' not found in Dockerfile")]
    StageNotFound {
        /// The stage name or index that was referenced
        name: String,
    },

    /// RUN instruction failed
    #[error("RUN command failed with exit code {exit_code}: {command}")]
    RunFailed {
        /// The command that failed
        command: String,
        /// Exit code returned by the command
        exit_code: i32,
    },

    /// Failed to create layer
    #[error("Failed to create layer: {message}")]
    LayerCreate {
        /// Underlying error description
        message: String,
    },

    /// Cache operation failed
    #[error("Cache error: {message}")]
    CacheError {
        /// Underlying cache error
        message: String,
    },

    /// Registry operation failed
    #[error("Registry error: {message}")]
    RegistryError {
        /// Underlying registry error
        message: String,
    },

    /// IO error
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    /// Variable expansion failed
    #[error("Variable expansion failed: {0}")]
    VariableExpansion(String),

    /// Invalid instruction
    #[error("Invalid instruction '{instruction}': {reason}")]
    InvalidInstruction {
        /// The instruction that was invalid
        instruction: String,
        /// Reason why it was invalid
        reason: String,
    },

    /// Buildah command execution failed
    #[error("Buildah execution failed: {command} (exit code {exit_code}): {stderr}")]
    BuildahExecution {
        /// The buildah command that failed
        command: String,
        /// Exit code from buildah
        exit_code: i32,
        /// Standard error output
        stderr: String,
    },

    /// Build context too large
    #[error("Build context too large: {size} bytes (max: {max} bytes)")]
    ContextTooLarge {
        /// Actual size in bytes
        size: u64,
        /// Maximum allowed size
        max: u64,
    },

    /// Base image not found
    #[error("Base image not found: {image}")]
    BaseImageNotFound {
        /// The image reference that was not found
        image: String,
    },

    /// Circular dependency in multi-stage build
    #[error("Circular dependency detected in multi-stage build: {stages:?}")]
    CircularDependency {
        /// The stages involved in the cycle
        stages: Vec<String>,
    },

    /// Buildah binary not found or installation failed
    #[error("Buildah not found: {message}")]
    BuildahNotFound {
        /// Details about the failure
        message: String,
    },

    /// ZImagefile YAML deserialization failed
    #[error("ZImagefile parse error: {message}")]
    ZImagefileParse {
        /// The underlying YAML parse error message
        message: String,
    },

    /// ZImagefile semantic validation failed
    #[error("ZImagefile validation error: {message}")]
    ZImagefileValidation {
        /// Description of what validation rule was violated
        message: String,
    },
}

impl BuildError {
    /// Create a DockerfileParse error from a message and line number
    pub fn parse_error(msg: impl Into<String>, line: usize) -> Self {
        Self::DockerfileParse {
            message: msg.into(),
            line,
        }
    }

    /// Create a ContextRead error from a path and IO error
    pub fn context_read(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::ContextRead {
            path: path.into(),
            source,
        }
    }

    /// Create a PathEscape error
    pub fn path_escape(path: impl Into<PathBuf>) -> Self {
        Self::PathEscape { path: path.into() }
    }

    /// Create a StageNotFound error
    pub fn stage_not_found(name: impl Into<String>) -> Self {
        Self::StageNotFound { name: name.into() }
    }

    /// Create a RunFailed error
    pub fn run_failed(command: impl Into<String>, exit_code: i32) -> Self {
        Self::RunFailed {
            command: command.into(),
            exit_code,
        }
    }

    /// Create a LayerCreate error
    pub fn layer_create(msg: impl Into<String>) -> Self {
        Self::LayerCreate {
            message: msg.into(),
        }
    }

    /// Create a CacheError
    pub fn cache_error(msg: impl Into<String>) -> Self {
        Self::CacheError {
            message: msg.into(),
        }
    }

    /// Create a RegistryError
    pub fn registry_error(msg: impl Into<String>) -> Self {
        Self::RegistryError {
            message: msg.into(),
        }
    }

    /// Create an InvalidInstruction error
    pub fn invalid_instruction(instruction: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::InvalidInstruction {
            instruction: instruction.into(),
            reason: reason.into(),
        }
    }

    /// Create a BuildahExecution error
    pub fn buildah_execution(
        command: impl Into<String>,
        exit_code: i32,
        stderr: impl Into<String>,
    ) -> Self {
        Self::BuildahExecution {
            command: command.into(),
            exit_code,
            stderr: stderr.into(),
        }
    }

    /// Create a BuildahNotFound error
    pub fn buildah_not_found(message: impl Into<String>) -> Self {
        Self::BuildahNotFound {
            message: message.into(),
        }
    }

    /// Create a ZImagefileParse error
    pub fn zimagefile_parse(message: impl Into<String>) -> Self {
        Self::ZImagefileParse {
            message: message.into(),
        }
    }

    /// Create a ZImagefileValidation error
    pub fn zimagefile_validation(message: impl Into<String>) -> Self {
        Self::ZImagefileValidation {
            message: message.into(),
        }
    }
}

/// Result type alias for build operations
pub type Result<T, E = BuildError> = std::result::Result<T, E>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let err = BuildError::parse_error("unexpected token", 42);
        assert!(err.to_string().contains("line 42"));
        assert!(err.to_string().contains("unexpected token"));
    }

    #[test]
    fn test_path_escape_error() {
        let err = BuildError::path_escape("/etc/passwd");
        assert!(err.to_string().contains("/etc/passwd"));
        assert!(err.to_string().contains("escape"));
    }

    #[test]
    fn test_run_failed_error() {
        let err = BuildError::run_failed("apt-get install foo", 127);
        assert!(err.to_string().contains("exit code 127"));
        assert!(err.to_string().contains("apt-get install foo"));
    }
}
