//! Abstract container runtime interface
//!
//! Defines the Runtime trait that can be implemented for different container runtimes
//! (containerd, CRI-O, etc.)

use crate::cgroups_stats::ContainerStats;
use crate::error::{AgentError, Result};
use futures_util::Stream;
use std::net::IpAddr;
use std::pin::Pin;
use std::time::Duration;
use tokio::task::JoinHandle;
use zlayer_observability::logs::{LogEntry, LogSource, LogStream};
use zlayer_spec::{PullPolicy, RegistryAuth, ServiceSpec};

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

/// Reason a container stopped running, as reported by [`Runtime::wait_outcome`].
///
/// Serialized as `snake_case` strings on the wire (`exited`, `signal`,
/// `oom_killed`, `runtime_error`) so the API DTO can emit the reason as-is
/// without a second translation layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WaitReason {
    /// Container exited normally.
    Exited,
    /// Container was killed by a signal (e.g. `SIGKILL`, `SIGTERM`).
    Signal,
    /// Container was killed by the OOM killer.
    OomKilled,
    /// Runtime-side failure (pre-start error, runtime crash, etc.).
    RuntimeError,
}

/// Richer wait result returned by [`Runtime::wait_outcome`].
///
/// Backwards-compatible with [`Runtime::wait_container`] (which returns just
/// `exit_code`). The API handler uses this to populate the extended
/// `ContainerWaitResponse` fields (`reason`, `signal`, `finished_at`) while
/// existing callers that only need the exit code can keep using
/// `wait_container`.
#[derive(Debug, Clone)]
pub struct WaitOutcome {
    /// Process exit code (0 = success). When the container was killed by
    /// signal `N`, this is typically `128 + N`.
    pub exit_code: i32,
    /// Classification of the exit.
    pub reason: WaitReason,
    /// Signal name when `reason == WaitReason::Signal`, e.g. `"SIGKILL"`.
    /// Derived from `exit_code - 128` on a best-effort basis.
    pub signal: Option<String>,
    /// Time the container exited, if the runtime reports it.
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl WaitOutcome {
    /// Build a plain `Exited` outcome with no signal/timestamp metadata — the
    /// default that matches the pre-§3.12 behaviour.
    #[must_use]
    pub fn exited(exit_code: i32) -> Self {
        Self {
            exit_code,
            reason: WaitReason::Exited,
            signal: None,
            finished_at: None,
        }
    }
}

/// Map a signal-style exit code (`128 + N`) to a canonical signal name.
///
/// Recognises common POSIX signals and falls back to `signal_<n>` for
/// unknown numbers so the caller always gets *something* readable.
#[must_use]
pub fn signal_name_from_exit_code(exit_code: i32) -> Option<String> {
    if exit_code <= 128 {
        return None;
    }
    let n = exit_code - 128;
    let name = match n {
        1 => "SIGHUP",
        2 => "SIGINT",
        3 => "SIGQUIT",
        4 => "SIGILL",
        6 => "SIGABRT",
        7 => "SIGBUS",
        8 => "SIGFPE",
        9 => "SIGKILL",
        10 => "SIGUSR1",
        11 => "SIGSEGV",
        12 => "SIGUSR2",
        13 => "SIGPIPE",
        14 => "SIGALRM",
        15 => "SIGTERM",
        17 => "SIGSTOP",
        18 => "SIGCONT",
        _ => return Some(format!("signal_{n}")),
    };
    Some(name.to_string())
}

/// One streaming event emitted by [`Runtime::exec_stream`].
///
/// Runtimes push these events as the exec'd command produces output. The final
/// event for any successful stream is always an [`ExecEvent::Exit`] carrying
/// the process's exit code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecEvent {
    /// A chunk of stdout from the running command. Emitted line-by-line by
    /// Docker; other runtimes may emit the full buffered output in one event.
    Stdout(String),
    /// A chunk of stderr from the running command.
    Stderr(String),
    /// The command has exited with this exit code. Always the final event.
    Exit(i32),
}

/// Boxed async stream of [`ExecEvent`]s returned by [`Runtime::exec_stream`].
pub type ExecEventStream = Pin<Box<dyn Stream<Item = ExecEvent> + Send>>;

/// Per-network attachment reported by [`Runtime::inspect_detailed`].
///
/// Mirrors the subset of bollard's `EndpointSettings` that the API needs to
/// populate `ContainerInfo.networks` for standalone containers. Kept in
/// `zlayer-agent` (rather than `zlayer-spec`) because it's a runtime-level
/// inspect result, not a deployment specification.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NetworkAttachmentDetail {
    /// Network name as reported by the runtime (Docker's key in
    /// `NetworkSettings.Networks`, e.g. `"bridge"` or a user-defined network
    /// name).
    pub network: String,
    /// DNS aliases the container answers to on this network. Empty when the
    /// runtime doesn't surface aliases.
    pub aliases: Vec<String>,
    /// Assigned IPv4 address on this network, if any. Empty strings are
    /// normalised to `None`.
    pub ipv4: Option<String>,
}

/// Per-container health detail reported by [`Runtime::inspect_detailed`].
///
/// Sourced directly from bollard's `ContainerState.health` (Docker's native
/// healthcheck tracking). Our internal `HealthMonitor` in
/// `crates/zlayer-agent/src/health.rs` drives service-level health events; for
/// standalone containers the API reports the runtime-native status instead so
/// that images with a baked-in `HEALTHCHECK` show up correctly.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HealthDetail {
    /// One of `"none"`, `"starting"`, `"healthy"`, `"unhealthy"` (Docker's
    /// `HealthStatusEnum`). Empty string is normalised to `"none"` upstream.
    pub status: String,
    /// Consecutive failing probe count, if the runtime reports it.
    pub failing_streak: Option<u32>,
    /// Output from the most recent failing probe, when available.
    pub last_output: Option<String>,
}

/// Rich inspect details for a single container, returned by
/// [`Runtime::inspect_detailed`].
///
/// Carries the fields `ContainerInfo` needs on top of the bare
/// [`ContainerState`] reported by [`Runtime::container_state`]:
/// published ports, attached networks, first IPv4, health, and `exit_code`.
///
/// Default is an all-empty record — that's what the default trait method
/// returns for runtimes that don't (yet) implement rich inspect, and the API
/// layer treats all fields as purely additive, so a default record still
/// produces a backwards-compatible `ContainerInfo`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContainerInspectDetails {
    /// Published port mappings (container → host), translated back from the
    /// runtime's internal port-binding map.
    pub ports: Vec<zlayer_spec::PortMapping>,
    /// Networks the container is attached to, plus the aliases + IPv4 for each.
    pub networks: Vec<NetworkAttachmentDetail>,
    /// First non-empty IPv4 address found across the container's networks,
    /// useful as a "primary" IP for simple clients that don't want to iterate
    /// `networks`. `None` when the container isn't on any network with an IP.
    pub ipv4: Option<String>,
    /// Health status when the container has a Docker-native `HEALTHCHECK` or
    /// the runtime otherwise reports a health state.
    pub health: Option<HealthDetail>,
    /// Most recent exit code, when the runtime reports one. `None` for
    /// containers that are still running and have never exited.
    pub exit_code: Option<i32>,
}

/// Abstract container runtime trait
///
/// This trait abstracts over different container runtimes (containerd, CRI-O, etc.)
#[async_trait::async_trait]
pub trait Runtime: Send + Sync {
    /// Pull an image to local storage
    async fn pull_image(&self, image: &str) -> Result<()>;

    /// Pull an image to local storage with a specific policy.
    ///
    /// When `auth` is `Some`, the runtime uses those inline credentials for
    /// the pull (§3.10 of `ZLAYER_SDK_FIXES.md`). When `auth` is `None`, the
    /// runtime falls back to its existing credential-store lookup keyed by
    /// registry hostname (or anonymous access when no match exists).
    ///
    /// Non-Docker runtimes may accept but ignore the `auth` argument — their
    /// OCI puller (`zlayer-registry`) already resolves credentials from the
    /// store by hostname, and inline auth is primarily a Docker-backend
    /// concern. Ignoring it is safe: callers that need inline auth should use
    /// the Docker runtime.
    async fn pull_image_with_policy(
        &self,
        image: &str,
        policy: PullPolicy,
        auth: Option<&RegistryAuth>,
    ) -> Result<()>;

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

    /// Execute a command in a container and stream stdout / stderr / exit
    /// events as they are produced.
    ///
    /// The default implementation calls the buffered [`Runtime::exec`] and
    /// emits everything as a single `Stdout` event, a single `Stderr` event,
    /// and a final `Exit` event. Runtimes that support true streaming (e.g.
    /// Docker via bollard) override this to produce line-by-line events as
    /// the command runs.
    ///
    /// The returned stream always terminates with exactly one
    /// [`ExecEvent::Exit`] as the final item on success. Errors that occur
    /// before the stream is returned (e.g. container not found, failure to
    /// create the exec) are surfaced via the outer `Result`; errors that
    /// occur mid-stream are logged by the runtime and the stream closes.
    async fn exec_stream(&self, id: &ContainerId, cmd: &[String]) -> Result<ExecEventStream> {
        let (exit, stdout, stderr) = self.exec(id, cmd).await?;
        let mut events: Vec<ExecEvent> = Vec::with_capacity(3);
        if !stdout.is_empty() {
            events.push(ExecEvent::Stdout(stdout));
        }
        if !stderr.is_empty() {
            events.push(ExecEvent::Stderr(stderr));
        }
        events.push(ExecEvent::Exit(exit));
        Ok(Box::pin(futures_util::stream::iter(events)))
    }

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

    /// Wait for a container to exit and return a [`WaitOutcome`] with richer
    /// classification (exit code + reason + signal + `finished_at` timestamp).
    ///
    /// The default implementation delegates to [`Runtime::wait_container`] and
    /// synthesizes a [`WaitReason::Exited`] result with no signal/timestamp.
    /// Runtimes that can distinguish OOM kills, signal deaths, or report a
    /// finished-at time (e.g. the Docker runtime, which has
    /// `ContainerInspectResponse.state.oom_killed` / `.finished_at`) should
    /// override this.
    async fn wait_outcome(&self, id: &ContainerId) -> Result<WaitOutcome> {
        let exit_code = self.wait_container(id).await?;
        Ok(WaitOutcome::exited(exit_code))
    }

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

    /// Get the HCN namespace GUID of a Windows container.
    ///
    /// Windows-only. Linux/macOS runtimes have no HCN namespace concept and
    /// return `Ok(None)`. The `HcsRuntime` overrides this to return the
    /// namespace GUID attached during `create_container`; `OverlayManager`
    /// then uses the GUID to register the container's assigned overlay IP
    /// against the right HCN compartment (analogous to how Linux uses PID
    /// to enter the netns via `/proc/{pid}/ns/net`).
    #[cfg(target_os = "windows")]
    async fn get_container_namespace_id(
        &self,
        _id: &ContainerId,
    ) -> Result<Option<windows::core::GUID>> {
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

    /// Return rich inspect details for a container: published ports, attached
    /// networks, first IPv4, health, and most-recent exit code.
    ///
    /// Runtimes implement this by translating the backend's native inspect
    /// response (bollard's `ContainerInspectResponse` for Docker) into the
    /// runtime-level [`ContainerInspectDetails`] struct. The API layer merges
    /// these fields into `ContainerInfo` on `GET /api/v1/containers` and
    /// `GET /api/v1/containers/{id}` (§3.15 of `ZLAYER_SDK_FIXES.md`).
    ///
    /// The default implementation returns [`ContainerInspectDetails::default`]
    /// — an all-empty record, which the API layer treats as "this runtime
    /// doesn't support rich inspect; skip all the extra fields". This keeps
    /// non-Docker runtimes (Youki, WASM, Mock) backwards compatible; they can
    /// override this later if they gain equivalent inspect capability.
    async fn inspect_detailed(&self, _id: &ContainerId) -> Result<ContainerInspectDetails> {
        Ok(ContainerInspectDetails::default())
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
        self.pull_image_with_policy(_image, PullPolicy::IfNotPresent, None)
            .await
    }

    async fn pull_image_with_policy(
        &self,
        _image: &str,
        _policy: PullPolicy,
        _auth: Option<&RegistryAuth>,
    ) -> Result<()> {
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

    #[test]
    fn wait_reason_serializes_as_snake_case() {
        assert_eq!(
            serde_json::to_string(&WaitReason::Exited).unwrap(),
            "\"exited\""
        );
        assert_eq!(
            serde_json::to_string(&WaitReason::Signal).unwrap(),
            "\"signal\""
        );
        assert_eq!(
            serde_json::to_string(&WaitReason::OomKilled).unwrap(),
            "\"oom_killed\""
        );
        assert_eq!(
            serde_json::to_string(&WaitReason::RuntimeError).unwrap(),
            "\"runtime_error\""
        );
    }

    #[test]
    fn wait_reason_deserialize_roundtrip() {
        for variant in [
            WaitReason::Exited,
            WaitReason::Signal,
            WaitReason::OomKilled,
            WaitReason::RuntimeError,
        ] {
            let s = serde_json::to_string(&variant).unwrap();
            let back: WaitReason = serde_json::from_str(&s).unwrap();
            assert_eq!(variant, back, "roundtrip failed for {variant:?}");
        }
    }

    #[test]
    fn signal_name_from_exit_code_known_signals() {
        assert_eq!(signal_name_from_exit_code(137).as_deref(), Some("SIGKILL"));
        assert_eq!(signal_name_from_exit_code(143).as_deref(), Some("SIGTERM"));
        assert_eq!(signal_name_from_exit_code(130).as_deref(), Some("SIGINT"));
        assert_eq!(signal_name_from_exit_code(129).as_deref(), Some("SIGHUP"));
        assert_eq!(signal_name_from_exit_code(139).as_deref(), Some("SIGSEGV"));
    }

    #[test]
    fn signal_name_from_exit_code_handles_unknown_and_normal() {
        // Normal exits (<= 128) return None.
        assert_eq!(signal_name_from_exit_code(0), None);
        assert_eq!(signal_name_from_exit_code(1), None);
        assert_eq!(signal_name_from_exit_code(128), None);

        // Unknown signals produce a stable string form.
        assert_eq!(
            signal_name_from_exit_code(128 + 99).as_deref(),
            Some("signal_99")
        );
    }

    #[tokio::test]
    async fn default_wait_outcome_delegates_to_wait_container() {
        let runtime = MockRuntime::new();
        let id = ContainerId {
            service: "wait-test".to_string(),
            replica: 0,
        };
        runtime.create_container(&id, &mock_spec()).await.unwrap();
        runtime.start_container(&id).await.unwrap();

        let outcome = runtime.wait_outcome(&id).await.unwrap();
        // MockRuntime::wait_container returns 0 for running containers.
        assert_eq!(outcome.exit_code, 0);
        assert_eq!(outcome.reason, WaitReason::Exited);
        assert!(outcome.signal.is_none());
        assert!(outcome.finished_at.is_none());
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
