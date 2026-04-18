//! Abstract container runtime interface
//!
//! Defines the Runtime trait that can be implemented for different container runtimes
//! (containerd, CRI-O, etc.)

use crate::cgroups_stats::ContainerStats;
use crate::error::{AgentError, Result};
use std::net::IpAddr;
use std::time::Duration;
use tokio::task::JoinHandle;
use zlayer_observability::logs::{LogEntry, LogSource, LogStream};
use zlayer_spec::{PullPolicy, ServiceSpec};

/// Container state
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContainerState {
    /// Container is being pulled/created
    Pending,
    /// Init actions are running
    Initializing,
    /// Container is running
    Running,
    /// Container is stopping
    Stopping,
    /// Container has exited
    Exited { code: i32 },
    /// Container failed
    Failed { reason: String },
}

/// Container identifier
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ContainerId {
    pub service: String,
    pub replica: u32,
}

impl std::fmt::Display for ContainerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}-rep-{}", self.service, self.replica)
    }
}

/// Container handle
pub struct Container {
    pub id: ContainerId,
    pub state: ContainerState,
    pub pid: Option<u32>,
    pub task: Option<JoinHandle<std::io::Result<()>>>,
    /// Overlay network IP address assigned to this container
    pub overlay_ip: Option<IpAddr>,
    /// Health monitor task handle for this container
    pub health_monitor: Option<JoinHandle<()>>,
    /// Runtime-assigned port override (used by macOS sandbox where all
    /// containers share the host network and need unique ports).
    /// When `Some(port)`, the proxy should use this port instead of the
    /// spec-declared endpoint port for this specific container's backend address.
    pub port_override: Option<u16>,
}

/// Summary information about a cached image on the host runtime.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ImageInfo {
    /// Canonical image reference (e.g. `zachhandley/zlayer-manager:latest`).
    pub reference: String,
    /// Content-addressed digest if known (`sha256:...`). `None` when the
    /// backend only tracks images by tag.
    pub digest: Option<String>,
    /// Total on-disk / in-cache size in bytes, when available.
    pub size_bytes: Option<u64>,
}

/// Result of a prune operation.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct PruneResult {
    /// Image references that were removed.
    pub deleted: Vec<String>,
    /// Bytes reclaimed from the cache. `0` when the backend cannot report.
    pub space_reclaimed: u64,
}

/// Abstract container runtime trait
///
/// This trait abstracts over different container runtimes (containerd, CRI-O, etc.)
#[async_trait::async_trait]
pub trait Runtime: Send + Sync {
    /// Pull an image to local storage
    async fn pull_image(&self, image: &str) -> Result<()>;

    /// Pull an image to local storage with a specific policy
    async fn pull_image_with_policy(&self, image: &str, policy: PullPolicy) -> Result<()>;

    /// Create a container
    async fn create_container(&self, id: &ContainerId, spec: &ServiceSpec) -> Result<()>;

    /// Start a container
    async fn start_container(&self, id: &ContainerId) -> Result<()>;

    /// Stop a container
    async fn stop_container(&self, id: &ContainerId, timeout: Duration) -> Result<()>;

    /// Remove a container
    async fn remove_container(&self, id: &ContainerId) -> Result<()>;

    /// Get container state
    async fn container_state(&self, id: &ContainerId) -> Result<ContainerState>;

    /// Get container logs as structured entries
    async fn container_logs(&self, id: &ContainerId, tail: usize) -> Result<Vec<LogEntry>>;

    /// Execute command in container
    async fn exec(&self, id: &ContainerId, cmd: &[String]) -> Result<(i32, String, String)>;

    /// Get container resource statistics from cgroups
    ///
    /// Returns CPU and memory statistics for the specified container.
    /// Used for metrics collection and autoscaling decisions.
    async fn get_container_stats(&self, id: &ContainerId) -> Result<ContainerStats>;

    /// Wait for a container to exit and return its exit code
    ///
    /// This method blocks until the container exits or an error occurs.
    /// Used primarily for job execution to implement run-to-completion semantics.
    async fn wait_container(&self, id: &ContainerId) -> Result<i32>;

    /// Get container logs (stdout/stderr combined)
    ///
    /// Returns logs as structured entries.
    /// Used to capture job output after completion.
    async fn get_logs(&self, id: &ContainerId) -> Result<Vec<LogEntry>>;

    /// Get the PID of a container's main process
    ///
    /// Returns:
    /// - `Ok(Some(pid))` for runtimes with real processes (Youki, Docker)
    /// - `Ok(None)` for runtimes without separate PIDs (WASM in-process)
    /// - `Err` if the container doesn't exist or there's an error
    ///
    /// Used for overlay network attachment and process management.
    async fn get_container_pid(&self, id: &ContainerId) -> Result<Option<u32>>;

    /// Get the IP address of a container
    ///
    /// Returns:
    /// - `Ok(Some(ip))` if the container has a known IP address
    /// - `Ok(None)` if the container exists but has no IP assigned yet
    /// - `Err` if the container doesn't exist or there's an error
    ///
    /// Used for proxy backend registration when overlay networking is unavailable.
    async fn get_container_ip(&self, id: &ContainerId) -> Result<Option<IpAddr>>;

    /// Get a runtime-assigned port override for a container.
    ///
    /// Returns:
    /// - `Ok(Some(port))` if the runtime assigned a dynamic port to this container
    /// - `Ok(None)` if the container should use the spec-declared endpoint port
    ///
    /// This exists for runtimes where all containers share the host network stack
    /// (e.g., macOS sandbox). Without network namespaces, multiple replicas of
    /// the same service would conflict on the same port. The runtime assigns
    /// each replica a unique port and passes it via the `PORT` environment variable.
    /// The proxy then routes to `container_ip:override_port` instead of
    /// `container_ip:spec_port`.
    ///
    /// Runtimes with per-container networking (overlay, VMs, Docker) return `None`.
    async fn get_container_port_override(&self, _id: &ContainerId) -> Result<Option<u16>> {
        Ok(None)
    }

    /// Sync all named volumes associated with this container to S3.
    ///
    /// Called after a container is stopped but before it is removed, giving
    /// the runtime a chance to flush persistent volume data to remote storage.
    ///
    /// The default implementation is a no-op. Runtimes that support S3-backed
    /// volume sync (e.g., Youki with the `s3` feature) override this.
    async fn sync_container_volumes(&self, _id: &ContainerId) -> Result<()> {
        Ok(())
    }

    /// List all images managed by this runtime's image storage.
    ///
    /// The default implementation returns `AgentError::Unsupported` — individual
    /// runtimes override this with backend-specific logic (bollard for Docker,
    /// zlayer-registry cache walk for Youki, etc.).
    async fn list_images(&self) -> Result<Vec<ImageInfo>> {
        Err(AgentError::Unsupported(
            "list_images is not supported by this runtime".into(),
        ))
    }

    /// Remove an image by reference from local storage.
    ///
    /// When `force` is true, also removes the image even when other containers
    /// reference it. The default implementation returns `AgentError::Unsupported`.
    async fn remove_image(&self, _image: &str, _force: bool) -> Result<()> {
        Err(AgentError::Unsupported(
            "remove_image is not supported by this runtime".into(),
        ))
    }

    /// Prune dangling / unused images from local storage.
    ///
    /// Returns a [`PruneResult`] describing what was removed. The default
    /// implementation returns `AgentError::Unsupported`.
    async fn prune_images(&self) -> Result<PruneResult> {
        Err(AgentError::Unsupported(
            "prune_images is not supported by this runtime".into(),
        ))
    }

    /// Send a signal to a running container.
    ///
    /// When `signal` is `None`, the runtime sends `SIGKILL` (matching Docker's
    /// `docker kill` default). Backends validate the signal name and reject
    /// anything outside the standard POSIX set (`SIGKILL`, `SIGTERM`, `SIGINT`,
    /// `SIGHUP`, `SIGUSR1`, `SIGUSR2`).
    ///
    /// Used by `POST /api/v1/containers/{id}/kill` and Docker-compat
    /// `docker kill`. The default implementation returns
    /// [`AgentError::Unsupported`].
    async fn kill_container(&self, _id: &ContainerId, _signal: Option<&str>) -> Result<()> {
        Err(AgentError::Unsupported(
            "kill_container is not supported by this runtime".into(),
        ))
    }

    /// Create a new tag pointing at an existing image.
    ///
    /// `source` is the reference to an already-cached image. `target` is the
    /// new reference to create — it must be a full reference (repository + tag).
    ///
    /// Used by `POST /api/v1/images/tag` and Docker-compat `docker tag`. The
    /// default implementation returns [`AgentError::Unsupported`].
    async fn tag_image(&self, _source: &str, _target: &str) -> Result<()> {
        Err(AgentError::Unsupported(
            "tag_image is not supported by this runtime".into(),
        ))
    }
}

/// Validate a signal name for [`Runtime::kill_container`].
///
/// Accepts both the `SIG`-prefixed form (`"SIGKILL"`) and the bare form
/// (`"KILL"`). Returns the canonical uppercase `SIG`-prefixed name on success.
///
/// # Errors
///
/// Returns [`AgentError::InvalidSpec`] when `signal` is not one of the
/// supported signals: `SIGKILL`, `SIGTERM`, `SIGINT`, `SIGHUP`, `SIGUSR1`,
/// `SIGUSR2`.
pub fn validate_signal(signal: &str) -> Result<String> {
    let trimmed = signal.trim();
    if trimmed.is_empty() {
        return Err(AgentError::InvalidSpec(
            "signal must not be empty".to_string(),
        ));
    }
    let upper = trimmed.to_ascii_uppercase();
    let canonical = if upper.starts_with("SIG") {
        upper
    } else {
        format!("SIG{upper}")
    };
    match canonical.as_str() {
        "SIGKILL" | "SIGTERM" | "SIGINT" | "SIGHUP" | "SIGUSR1" | "SIGUSR2" => Ok(canonical),
        other => Err(AgentError::InvalidSpec(format!(
            "unsupported signal '{other}'; allowed: SIGKILL, SIGTERM, SIGINT, SIGHUP, SIGUSR1, SIGUSR2"
        ))),
    }
}

/// Auth context injected into every container so it can talk back to the host
/// API without needing external credentials.
#[derive(Debug, Clone)]
pub struct ContainerAuthContext {
    /// Base URL of the `ZLayer` API, e.g. `"http://127.0.0.1:3669"`.
    pub api_url: String,
    /// JWT signing secret — used to mint per-container tokens at start time.
    pub jwt_secret: String,
    /// Absolute path of the Unix socket on the host (bind-mounted into Linux
    /// containers; added to `writable_dirs` for macOS sandbox).
    pub socket_path: String,
}

/// In-memory mock runtime for testing and development
pub struct MockRuntime {
    containers: tokio::sync::RwLock<std::collections::HashMap<ContainerId, Container>>,
}

impl MockRuntime {
    #[must_use]
    pub fn new() -> Self {
        Self {
            containers: tokio::sync::RwLock::new(std::collections::HashMap::new()),
        }
    }
}

impl Default for MockRuntime {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Runtime for MockRuntime {
    async fn pull_image(&self, _image: &str) -> Result<()> {
        self.pull_image_with_policy(_image, PullPolicy::IfNotPresent)
            .await
    }

    async fn pull_image_with_policy(&self, _image: &str, _policy: PullPolicy) -> Result<()> {
        // Mock: always succeeds
        tokio::time::sleep(Duration::from_millis(100)).await;
        Ok(())
    }

    async fn create_container(&self, id: &ContainerId, _spec: &ServiceSpec) -> Result<()> {
        let mut containers = self.containers.write().await;
        containers.insert(
            id.clone(),
            Container {
                id: id.clone(),
                state: ContainerState::Pending,
                pid: None,
                task: None,
                overlay_ip: None,
                health_monitor: None,
                port_override: None,
            },
        );
        Ok(())
    }

    async fn start_container(&self, id: &ContainerId) -> Result<()> {
        let mut containers = self.containers.write().await;
        if let Some(container) = containers.get_mut(id) {
            container.state = ContainerState::Running;
            container.pid = Some(std::process::id()); // Mock PID
        }
        Ok(())
    }

    async fn stop_container(&self, id: &ContainerId, _timeout: Duration) -> Result<()> {
        let mut containers = self.containers.write().await;
        if let Some(container) = containers.get_mut(id) {
            container.state = ContainerState::Exited { code: 0 };
        }
        Ok(())
    }

    async fn remove_container(&self, id: &ContainerId) -> Result<()> {
        let mut containers = self.containers.write().await;
        containers.remove(id);
        Ok(())
    }

    async fn container_state(&self, id: &ContainerId) -> Result<ContainerState> {
        let containers = self.containers.read().await;
        containers
            .get(id)
            .map(|c| c.state.clone())
            .ok_or_else(|| AgentError::NotFound {
                container: id.to_string(),
                reason: "container not found".to_string(),
            })
    }

    async fn container_logs(&self, _id: &ContainerId, tail: usize) -> Result<Vec<LogEntry>> {
        let entries = vec![
            LogEntry {
                timestamp: chrono::Utc::now(),
                stream: LogStream::Stdout,
                message: "mock log line 1".to_string(),
                source: LogSource::Container("mock".to_string()),
                service: None,
                deployment: None,
            },
            LogEntry {
                timestamp: chrono::Utc::now(),
                stream: LogStream::Stderr,
                message: "mock error line".to_string(),
                source: LogSource::Container("mock".to_string()),
                service: None,
                deployment: None,
            },
        ];
        let skip = entries.len().saturating_sub(tail);
        Ok(entries.into_iter().skip(skip).collect())
    }

    async fn exec(&self, _id: &ContainerId, cmd: &[String]) -> Result<(i32, String, String)> {
        Ok((0, cmd.join(" "), String::new()))
    }

    async fn get_container_stats(&self, id: &ContainerId) -> Result<ContainerStats> {
        // Mock: return dummy stats
        let containers = self.containers.read().await;
        if containers.contains_key(id) {
            Ok(ContainerStats {
                cpu_usage_usec: 1_000_000,       // 1 second
                memory_bytes: 50 * 1024 * 1024,  // 50 MB
                memory_limit: 256 * 1024 * 1024, // 256 MB
                timestamp: std::time::Instant::now(),
            })
        } else {
            Err(AgentError::NotFound {
                container: id.to_string(),
                reason: "container not found".to_string(),
            })
        }
    }

    async fn wait_container(&self, id: &ContainerId) -> Result<i32> {
        // Mock: simulate waiting for container to exit
        let containers = self.containers.read().await;
        if let Some(container) = containers.get(id) {
            match &container.state {
                ContainerState::Exited { code } => Ok(*code),
                ContainerState::Failed { .. } => Ok(1),
                _ => {
                    // Simulate a brief wait and then return success
                    drop(containers);
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    Ok(0)
                }
            }
        } else {
            Err(AgentError::NotFound {
                container: id.to_string(),
                reason: "container not found".to_string(),
            })
        }
    }

    async fn get_logs(&self, id: &ContainerId) -> Result<Vec<LogEntry>> {
        // Mock: return dummy structured log entries
        let containers = self.containers.read().await;
        if containers.contains_key(id) {
            let container_name = id.to_string();
            Ok(vec![
                LogEntry {
                    timestamp: chrono::Utc::now(),
                    stream: LogStream::Stdout,
                    message: format!("[{container_name}] Container started"),
                    source: LogSource::Container(container_name.clone()),
                    service: None,
                    deployment: None,
                },
                LogEntry {
                    timestamp: chrono::Utc::now(),
                    stream: LogStream::Stdout,
                    message: format!("[{container_name}] Executing command..."),
                    source: LogSource::Container(container_name.clone()),
                    service: None,
                    deployment: None,
                },
                LogEntry {
                    timestamp: chrono::Utc::now(),
                    stream: LogStream::Stdout,
                    message: format!("[{container_name}] Command completed successfully"),
                    source: LogSource::Container(container_name),
                    service: None,
                    deployment: None,
                },
            ])
        } else {
            Err(AgentError::NotFound {
                container: id.to_string(),
                reason: "container not found".to_string(),
            })
        }
    }

    async fn get_container_pid(&self, id: &ContainerId) -> Result<Option<u32>> {
        let containers = self.containers.read().await;
        if let Some(container) = containers.get(id) {
            Ok(container.pid)
        } else {
            Err(AgentError::NotFound {
                container: id.to_string(),
                reason: "container not found".to_string(),
            })
        }
    }

    async fn get_container_ip(&self, id: &ContainerId) -> Result<Option<IpAddr>> {
        let containers = self.containers.read().await;
        if containers.contains_key(id) {
            // Mock: deterministic IP based on replica number (172.17.0.{replica+2})
            #[allow(clippy::cast_possible_truncation)]
            let last_octet = (id.replica + 2) as u8;
            Ok(Some(IpAddr::V4(std::net::Ipv4Addr::new(
                172, 17, 0, last_octet,
            ))))
        } else {
            Err(AgentError::NotFound {
                container: id.to_string(),
                reason: "container not found".to_string(),
            })
        }
    }

    async fn list_images(&self) -> Result<Vec<ImageInfo>> {
        Ok(Vec::new())
    }

    async fn remove_image(&self, _image: &str, _force: bool) -> Result<()> {
        Ok(())
    }

    async fn prune_images(&self) -> Result<PruneResult> {
        Ok(PruneResult::default())
    }

    async fn kill_container(&self, id: &ContainerId, signal: Option<&str>) -> Result<()> {
        // Validate signal even in the mock so callers exercise the same error
        // path. Default to SIGKILL when omitted.
        let _canonical = validate_signal(signal.unwrap_or("SIGKILL"))?;
        let mut containers = self.containers.write().await;
        let container = containers.get_mut(id).ok_or_else(|| AgentError::NotFound {
            container: id.to_string(),
            reason: "container not found".to_string(),
        })?;
        container.state = ContainerState::Exited { code: 137 };
        Ok(())
    }

    async fn tag_image(&self, _source: &str, _target: &str) -> Result<()> {
        // The in-memory mock doesn't store images; treat tag as a no-op success.
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mock_runtime() {
        let runtime = MockRuntime::new();
        let id = ContainerId {
            service: "test".to_string(),
            replica: 1,
        };

        runtime.pull_image("test:latest").await.unwrap();
        runtime.create_container(&id, &mock_spec()).await.unwrap();
        runtime.start_container(&id).await.unwrap();

        let state = runtime.container_state(&id).await.unwrap();
        assert_eq!(state, ContainerState::Running);
    }

    #[test]
    fn validate_signal_accepts_known_signals() {
        // SIG-prefixed form
        assert_eq!(validate_signal("SIGKILL").unwrap(), "SIGKILL");
        assert_eq!(validate_signal("SIGTERM").unwrap(), "SIGTERM");
        assert_eq!(validate_signal("SIGINT").unwrap(), "SIGINT");
        assert_eq!(validate_signal("SIGHUP").unwrap(), "SIGHUP");
        assert_eq!(validate_signal("SIGUSR1").unwrap(), "SIGUSR1");
        assert_eq!(validate_signal("SIGUSR2").unwrap(), "SIGUSR2");

        // Bare form (no "SIG" prefix) should be canonicalised.
        assert_eq!(validate_signal("KILL").unwrap(), "SIGKILL");
        assert_eq!(validate_signal("term").unwrap(), "SIGTERM");
        // Whitespace around the name is tolerated.
        assert_eq!(validate_signal(" INT ").unwrap(), "SIGINT");
    }

    #[test]
    fn validate_signal_rejects_unknown_or_empty() {
        assert!(matches!(
            validate_signal(""),
            Err(AgentError::InvalidSpec(_))
        ));
        assert!(matches!(
            validate_signal("   "),
            Err(AgentError::InvalidSpec(_))
        ));
        assert!(matches!(
            validate_signal("SIGSEGV"),
            Err(AgentError::InvalidSpec(_))
        ));
        assert!(matches!(
            validate_signal("NOPE"),
            Err(AgentError::InvalidSpec(_))
        ));
        // Signals outside the POSIX allowlist are rejected even if real.
        assert!(matches!(
            validate_signal("SIGPIPE"),
            Err(AgentError::InvalidSpec(_))
        ));
    }

    #[tokio::test]
    async fn mock_kill_container_defaults_to_sigkill() {
        let runtime = MockRuntime::new();
        let id = ContainerId {
            service: "kill-me".to_string(),
            replica: 0,
        };
        runtime.create_container(&id, &mock_spec()).await.unwrap();
        runtime.start_container(&id).await.unwrap();

        // `None` -> defaults to SIGKILL; returns Ok and marks the container
        // as exited.
        runtime.kill_container(&id, None).await.unwrap();
        let state = runtime.container_state(&id).await.unwrap();
        assert!(
            matches!(state, ContainerState::Exited { code: 137 }),
            "expected Exited(137), got {state:?}"
        );
    }

    #[tokio::test]
    async fn mock_kill_container_rejects_bogus_signal() {
        let runtime = MockRuntime::new();
        let id = ContainerId {
            service: "kill-me".to_string(),
            replica: 0,
        };
        runtime.create_container(&id, &mock_spec()).await.unwrap();
        runtime.start_container(&id).await.unwrap();

        let err = runtime
            .kill_container(&id, Some("SIGFOO"))
            .await
            .unwrap_err();
        assert!(
            matches!(err, AgentError::InvalidSpec(_)),
            "expected InvalidSpec, got {err:?}"
        );
    }

    fn mock_spec() -> ServiceSpec {
        use zlayer_spec::*;
        serde_yaml::from_str::<DeploymentSpec>(
            r"
version: v1
deployment: test
services:
  test:
    rtype: service
    image:
      name: test:latest
    endpoints:
      - name: http
        protocol: http
        port: 8080
",
        )
        .unwrap()
        .services
        .remove("test")
        .unwrap()
    }
}
