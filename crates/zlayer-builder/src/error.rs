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

    /// `ZImagefile` YAML deserialization failed
    #[error("ZImagefile parse error: {message}")]
    ZImagefileParse {
        /// The underlying YAML parse error message
        message: String,
    },

    /// `ZImagefile` semantic validation failed
    #[error("ZImagefile validation error: {message}")]
    ZImagefileValidation {
        /// Description of what validation rule was violated
        message: String,
    },

    /// Pipeline validation or execution error
    #[error("Pipeline error: {message}")]
    PipelineError {
        /// Description of the pipeline error
        message: String,
    },

    /// WASM build failed
    #[error("WASM build error: {0}")]
    WasmBuild(#[from] crate::wasm_builder::WasmBuildError),

    /// Operation not supported by this backend
    #[error("Operation '{operation}' is not supported by this backend")]
    NotSupported {
        /// The operation that was attempted
        operation: String,
    },

    /// A code path that is reserved for a future phase / task and is
    /// intentionally not yet wired up. Constructed by
    /// [`BuildError::not_yet_implemented`] so call sites can give a precise
    /// reason (typically referencing the follow-up task that delivers it,
    /// e.g. "RUN execution lands in Phase 4 task 4.B").
    ///
    /// This is distinct from [`BuildError::NotSupported`], which signals a
    /// permanent capability gap of the chosen backend; `NotYetImplemented`
    /// signals "tracked work, coming in a later task — do not silently
    /// no-op".
    #[error("not yet implemented: {0}")]
    NotYetImplemented(String),

    /// A specific RUN step in a Dockerfile failed with a non-zero exit
    /// code. Carries the step index (0-based, counted across the active
    /// stage's instruction list), the exit code surfaced by the
    /// guest process, and a stderr tail to anchor diagnostics.
    ///
    /// Distinct from [`BuildError::RunFailed`] in that the latter is
    /// emitted by the buildah/HCS backends working through the
    /// `BuildBackend` trait, whereas this variant is emitted by the
    /// Phase 4 `WindowsBuilder` path which carries a richer per-step
    /// context (the step index and a stderr tail) than the buildah
    /// path can produce.
    #[error("RUN step {step_index} failed with exit code {exit_code}: {stderr_tail}")]
    RunStepFailed {
        /// Zero-based step index within the active stage's instruction
        /// list (the value Phase 4 errors use to anchor a diagnostic).
        step_index: usize,
        /// Exit code reported by the guest process.
        exit_code: i32,
        /// Last fragment of stderr captured during the RUN step (or a
        /// synthesised message when pipe capture has not yet been
        /// wired). Surfaced verbatim in the error display so users get
        /// the failing command in their build log.
        stderr_tail: String,
    },

    /// `HcsExportLayer` / wclayer-side IO failed while capturing the
    /// post-RUN scratch diff. Distinct from [`BuildError::IoError`] so
    /// the WCOW builder can surface a layer-export-specific message
    /// (the underlying failure is almost always either an
    /// `HcsExportLayer` HRESULT or a tar/gzip walk error).
    #[error("layer export failed: {source}")]
    LayerExportFailed {
        /// Underlying IO error (often wraps an HCS HRESULT).
        #[source]
        source: std::io::Error,
    },

    /// Chocolatey resolver could not produce a Windows equivalent for a
    /// Linux package name encountered in a RUN instruction. The package
    /// is named so the user can edit the Dockerfile (or contribute the
    /// mapping to `RepoSources`).
    #[error(
        "no Chocolatey mapping for Linux package '{package}' in source distro '{source_distro}'"
    )]
    ChocoResolutionFailed {
        /// The Linux package name that did not resolve.
        package: String,
        /// The `RepoSources`-style source distro key that was queried
        /// (e.g. `"debian-12"`, `"ubuntu-22.04"`).
        source_distro: String,
    },

    /// COPY/ADD source path contained a `..` component which is forbidden
    /// because it would escape the build context. Surfaced before any
    /// filesystem access so a malicious Dockerfile cannot reach files
    /// outside the build directory even via a TOCTOU window.
    #[error("COPY/ADD source path '{src}' contains '..' which is forbidden")]
    PathTraversal {
        /// The offending source path as it appeared in the Dockerfile.
        src: String,
    },

    /// ADD `<URL> <dest>` failed to download the remote resource. Carries
    /// the URL for diagnostics; the underlying network/protocol failure is
    /// chained as the `source` so users get the precise cause (connection
    /// refused, 404, TLS error, etc.).
    #[error("ADD failed to fetch '{url}': {source}")]
    HttpFetchFailed {
        /// The URL the builder attempted to fetch.
        url: String,
        /// Underlying reqwest failure.
        #[source]
        source: reqwest::Error,
    },

    /// ADD auto-extraction of a tarball failed. Carries the chained IO
    /// error from the `tar` / `flate2` / `bzip2` / `xz2` pipeline so the
    /// user can see which entry tripped (path traversal in the archive,
    /// disk full, etc.).
    #[error("ADD failed to extract tarball: {source}")]
    TarExtractFailed {
        /// Underlying tar/decompressor IO error.
        #[source]
        source: std::io::Error,
    },
}

impl BuildError {
    /// Create a `DockerfileParse` error from a message and line number
    pub fn parse_error(msg: impl Into<String>, line: usize) -> Self {
        Self::DockerfileParse {
            message: msg.into(),
            line,
        }
    }

    /// Create a `ContextRead` error from a path and IO error
    pub fn context_read(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::ContextRead {
            path: path.into(),
            source,
        }
    }

    /// Create a `PathEscape` error
    pub fn path_escape(path: impl Into<PathBuf>) -> Self {
        Self::PathEscape { path: path.into() }
    }

    /// Create a `StageNotFound` error
    pub fn stage_not_found(name: impl Into<String>) -> Self {
        Self::StageNotFound { name: name.into() }
    }

    /// Create a `RunFailed` error
    pub fn run_failed(command: impl Into<String>, exit_code: i32) -> Self {
        Self::RunFailed {
            command: command.into(),
            exit_code,
        }
    }

    /// Create a `LayerCreate` error
    pub fn layer_create(msg: impl Into<String>) -> Self {
        Self::LayerCreate {
            message: msg.into(),
        }
    }

    /// Create a `CacheError`
    pub fn cache_error(msg: impl Into<String>) -> Self {
        Self::CacheError {
            message: msg.into(),
        }
    }

    /// Create a `RegistryError`
    pub fn registry_error(msg: impl Into<String>) -> Self {
        Self::RegistryError {
            message: msg.into(),
        }
    }

    /// Create an `InvalidInstruction` error
    pub fn invalid_instruction(instruction: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::InvalidInstruction {
            instruction: instruction.into(),
            reason: reason.into(),
        }
    }

    /// Create a `BuildahExecution` error
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

    /// Create a `BuildahNotFound` error
    pub fn buildah_not_found(message: impl Into<String>) -> Self {
        Self::BuildahNotFound {
            message: message.into(),
        }
    }

    /// Create a `ZImagefileParse` error
    pub fn zimagefile_parse(message: impl Into<String>) -> Self {
        Self::ZImagefileParse {
            message: message.into(),
        }
    }

    /// Create a `ZImagefileValidation` error
    pub fn zimagefile_validation(message: impl Into<String>) -> Self {
        Self::ZImagefileValidation {
            message: message.into(),
        }
    }

    /// Create a `PipelineError`
    pub fn pipeline_error(message: impl Into<String>) -> Self {
        Self::PipelineError {
            message: message.into(),
        }
    }

    /// Create a `NotYetImplemented` error. The `msg` should name the
    /// follow-up task or phase that delivers the missing behavior
    /// (e.g. `"RUN execution lands in Phase 4 task 4.B"`).
    pub fn not_yet_implemented(msg: impl Into<String>) -> Self {
        Self::NotYetImplemented(msg.into())
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
