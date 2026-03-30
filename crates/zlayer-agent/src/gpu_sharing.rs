//! GPU sharing support via NVIDIA MPS (Multi-Process Service) and time-slicing.
//!
//! MPS allows multiple containers to share a single GPU with hardware-level
//! isolation of compute resources. Time-slicing provides simpler round-robin
//! GPU sharing without concurrent kernel execution.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::process::Command;
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};

/// Manages NVIDIA MPS daemon instances, one per physical GPU.
///
/// The MPS control daemon must run on the host for each GPU that uses MPS sharing.
/// This manager starts the daemon on first MPS container creation and stops it
/// when the last MPS container on that GPU exits.
#[derive(Debug)]
pub struct MpsDaemonManager {
    /// Per-GPU state: GPU index -> daemon state
    daemons: Arc<Mutex<HashMap<u32, MpsDaemonState>>>,
    /// Base directory for MPS pipe and log files
    base_dir: PathBuf,
}

#[derive(Debug)]
struct MpsDaemonState {
    /// Number of containers currently using MPS on this GPU
    ref_count: u32,
    /// PID of the MPS control daemon process
    pid: Option<u32>,
}

impl MpsDaemonManager {
    /// Create a new MPS daemon manager.
    ///
    /// `base_dir` is the root directory for per-GPU MPS pipe and log directories
    /// (e.g. `/var/run/zlayer/mps`).
    #[must_use]
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            daemons: Arc::new(Mutex::new(HashMap::new())),
            base_dir: base_dir.into(),
        }
    }

    /// Pipe directory for a specific GPU index.
    fn pipe_dir(&self, gpu_index: u32) -> PathBuf {
        self.base_dir.join(format!("gpu{gpu_index}/pipe"))
    }

    /// Log directory for a specific GPU index.
    fn log_dir(&self, gpu_index: u32) -> PathBuf {
        self.base_dir.join(format!("gpu{gpu_index}/log"))
    }

    /// Start or attach to the MPS daemon for a GPU.
    ///
    /// If the daemon is already running for this GPU, increments the reference count.
    /// Otherwise, starts a new `nvidia-cuda-mps-control` daemon process.
    ///
    /// Returns the pipe directory path that should be injected as
    /// `CUDA_MPS_PIPE_DIRECTORY` into the container's environment.
    ///
    /// # Errors
    ///
    /// Returns an error if the MPS control daemon fails to start.
    pub async fn acquire(&self, gpu_index: u32) -> Result<PathBuf, MpsError> {
        let mut daemons = self.daemons.lock().await;

        let state = daemons.entry(gpu_index).or_insert(MpsDaemonState {
            ref_count: 0,
            pid: None,
        });

        if state.ref_count > 0 {
            // Daemon already running, just bump the reference count
            state.ref_count += 1;
            debug!(
                gpu_index,
                ref_count = state.ref_count,
                "MPS daemon ref count incremented"
            );
            return Ok(self.pipe_dir(gpu_index));
        }

        // Start a new MPS daemon for this GPU
        let pipe_dir = self.pipe_dir(gpu_index);
        let log_dir = self.log_dir(gpu_index);

        // Create directories
        tokio::fs::create_dir_all(&pipe_dir)
            .await
            .map_err(|e| MpsError::Setup(format!("Failed to create MPS pipe directory: {e}")))?;
        tokio::fs::create_dir_all(&log_dir)
            .await
            .map_err(|e| MpsError::Setup(format!("Failed to create MPS log directory: {e}")))?;

        info!(gpu_index, pipe_dir = %pipe_dir.display(), "Starting MPS control daemon");

        let child = Command::new("nvidia-cuda-mps-control")
            .arg("-d") // daemon mode
            .env("CUDA_VISIBLE_DEVICES", gpu_index.to_string())
            .env("CUDA_MPS_PIPE_DIRECTORY", &pipe_dir)
            .env("CUDA_MPS_LOG_DIRECTORY", &log_dir)
            .spawn()
            .map_err(|e| MpsError::Start(format!("Failed to spawn MPS daemon: {e}")))?;

        let pid = child.id();
        state.ref_count = 1;
        state.pid = pid;

        info!(gpu_index, ?pid, "MPS control daemon started");

        Ok(pipe_dir)
    }

    /// Release a reference to the MPS daemon for a GPU.
    ///
    /// Decrements the reference count. When it reaches zero, the MPS daemon
    /// is shut down by sending "quit" to the control pipe.
    pub async fn release(&self, gpu_index: u32) {
        let mut daemons = self.daemons.lock().await;

        if let Some(state) = daemons.get_mut(&gpu_index) {
            state.ref_count = state.ref_count.saturating_sub(1);
            debug!(
                gpu_index,
                ref_count = state.ref_count,
                "MPS daemon ref count decremented"
            );

            if state.ref_count == 0 {
                info!(
                    gpu_index,
                    "Stopping MPS control daemon (last container released)"
                );
                if let Err(e) = self.stop_daemon(gpu_index).await {
                    error!(gpu_index, error = %e, "Failed to stop MPS daemon cleanly");
                }
                daemons.remove(&gpu_index);
            }
        }
    }

    /// Stop the MPS daemon by sending "quit" to the control pipe.
    async fn stop_daemon(&self, gpu_index: u32) -> Result<(), MpsError> {
        let pipe_dir = self.pipe_dir(gpu_index);

        let output = Command::new("sh")
            .arg("-c")
            .arg(format!(
                "echo quit | CUDA_MPS_PIPE_DIRECTORY={} nvidia-cuda-mps-control",
                pipe_dir.display()
            ))
            .output()
            .await
            .map_err(|e| MpsError::Stop(format!("Failed to send quit to MPS daemon: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            warn!(gpu_index, stderr = %stderr, "MPS daemon quit command returned non-zero");
        }

        Ok(())
    }

    /// Get the environment variables needed for a container using MPS on a specific GPU.
    #[must_use]
    pub fn env_vars(&self, gpu_index: u32) -> Vec<(String, String)> {
        vec![
            (
                "CUDA_MPS_PIPE_DIRECTORY".to_string(),
                self.pipe_dir(gpu_index).to_string_lossy().to_string(),
            ),
            (
                "CUDA_MPS_LOG_DIRECTORY".to_string(),
                self.log_dir(gpu_index).to_string_lossy().to_string(),
            ),
        ]
    }

    /// Configure time-slicing for a GPU via nvidia-smi.
    ///
    /// This sets the GPU to shared compute mode, allowing multiple processes
    /// to use it with round-robin scheduling.
    ///
    /// # Errors
    ///
    /// Returns an error if `nvidia-smi` fails.
    pub async fn enable_time_slicing(gpu_index: u32) -> Result<(), MpsError> {
        let output = Command::new("nvidia-smi")
            .args(["-i", &gpu_index.to_string()])
            .args(["-c", "DEFAULT"]) // Set compute mode to default (shared)
            .output()
            .await
            .map_err(|e| MpsError::Setup(format!("Failed to set compute mode: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(MpsError::Setup(format!(
                "nvidia-smi set compute mode failed: {stderr}"
            )));
        }

        info!(gpu_index, "Time-slicing enabled (compute mode = DEFAULT)");
        Ok(())
    }
}

/// Errors from MPS daemon operations
#[derive(Debug, thiserror::Error)]
pub enum MpsError {
    /// Failed to set up MPS directories or configuration
    #[error("MPS setup failed: {0}")]
    Setup(String),
    /// Failed to start the MPS daemon
    #[error("MPS daemon start failed: {0}")]
    Start(String),
    /// Failed to stop the MPS daemon
    #[error("MPS daemon stop failed: {0}")]
    Stop(String),
}
