//! Agent-specific errors

use std::time::Duration;
use thiserror::Error;

/// Agent runtime errors
#[derive(Debug, Error)]
pub enum AgentError {
    /// Container not found
    #[error("Container '{container}' not found: {reason}")]
    NotFound { container: String, reason: String },

    /// Failed to pull image
    #[error("Failed to pull image '{image}': {reason}")]
    PullFailed { image: String, reason: String },

    /// Failed to create container
    #[error("Failed to create container '{id}': {reason}")]
    CreateFailed { id: String, reason: String },

    /// Failed to start container
    #[error("Failed to start container '{id}': {reason}")]
    StartFailed { id: String, reason: String },

    /// Container exited unexpectedly
    #[error("Container '{id}' exited unexpectedly with code {code}")]
    UnexpectedExit { id: String, code: i32 },

    /// Health check failed
    #[error("Health check failed for '{id}': {reason}")]
    HealthCheckFailed { id: String, reason: String },

    /// Init action failed
    #[error("Init action failed for '{id}': {reason}")]
    InitActionFailed { id: String, reason: String },

    /// Timeout
    #[error("Timeout after {timeout:?}")]
    Timeout { timeout: Duration },

    /// Dependency timeout - service waiting for dependency condition
    #[error("Dependency timeout: '{service}' waiting for '{dependency}' ({condition}) after {timeout:?}")]
    DependencyTimeout {
        service: String,
        dependency: String,
        condition: String,
        timeout: Duration,
    },

    /// Invalid spec
    #[error("Invalid spec: {0}")]
    InvalidSpec(String),

    /// Network setup or operation failed
    #[error("Network error: {0}")]
    Network(String),

    /// Configuration error (missing or invalid configuration)
    #[error("Configuration error: {0}")]
    Configuration(String),

    /// Internal runtime error
    #[error("Internal error: {0}")]
    Internal(String),

    /// Operation is not supported by this runtime
    #[error("Operation not supported by this runtime: {0}")]
    Unsupported(String),

    /// GPU was requested by the service spec, but the underlying WSL2 host
    /// cannot deliver GPU access (typically because `/dev/dxg` is not exposed
    /// by the running WSL2 kernel, or the `WSLg` driver shim mount is missing).
    ///
    /// Returned by the WSL2 delegate when wiring `/dev/dxg` and the `WSLg` lib
    /// mounts into the youki bundle. Silent CPU fallback would be surprising
    /// for users who explicitly asked for a GPU, so this is a hard error;
    /// callers must either downgrade the spec to drop `resources.gpu` or
    /// re-place the workload on a node whose WSL2 distro exposes the
    /// `DirectX` kernel interface.
    #[error("GPU requested but WSL2 GPU support not available on this host: {reason}")]
    WslGpuUnavailable { reason: String },

    /// GPU sharing was requested (MPS or time-slicing) but the host or
    /// runtime cannot satisfy the requested mode.
    ///
    /// Typical causes:
    /// * `mode = "mps"` but the host MPS pipe / log directory does not exist
    ///   (the `nvidia-cuda-mps-control` daemon is not running).
    /// * `mode = "mps"` combined with `isolation: hyperv` on Windows — MPS is
    ///   not exposed inside the UVM kernel.
    ///
    /// Silent fallback to exclusive-mode access would be surprising for users
    /// who explicitly opted in to sharing (they may be relying on sharing for
    /// capacity planning), so this is a hard error. Callers must either fix
    /// the host (start the MPS daemon, switch isolation) or drop the
    /// `sharing` field from the spec.
    #[error("GPU sharing mode '{mode}' is unavailable: {reason}")]
    GpuSharingUnavailable {
        /// Sharing mode that could not be satisfied (`"mps"`, `"time-slice"`).
        mode: String,
        /// Human-readable explanation (e.g. "/tmp/nvidia-mps does not exist; \
        /// ensure nvidia-cuda-mps-control is running").
        reason: String,
    },

    /// The workload cannot run on this node and must be re-placed on a peer
    /// that can satisfy `required_os`.
    ///
    /// Returned by [`crate::runtimes::composite::CompositeRuntime::select_for`]
    /// when a foreign-OS workload (today: Linux on a Windows node) lands on a
    /// node that has no suitable local runtime (e.g. no WSL2 delegate
    /// configured). The scheduler is expected to catch this and re-dispatch
    /// to a cluster peer whose `NodeState.os` matches `required_os`. When no
    /// capable peer exists the scheduler marks the service failed with an
    /// actionable message naming both remediations (enable the local WSL2
    /// delegate, or add a Linux peer to the cluster).
    ///
    /// This variant is *not* a container failure: the service manager must
    /// surface it to the scheduler and must not roll up `CreateFailed` on top
    /// of it, otherwise the rescheduling signal is lost.
    #[error(
        "route-to-peer: service '{service}' requires OS '{required_os}' on another node: {reason}"
    )]
    RouteToPeer {
        /// Service name that needs to be re-placed.
        service: String,
        /// OS the workload requires (OCI-canonical: `linux` / `windows` / `darwin`).
        required_os: String,
        /// Human-readable explanation (e.g. "no WSL2 delegate configured on this Windows node").
        reason: String,
    },
}

pub type Result<T, E = AgentError> = std::result::Result<T, E>;
