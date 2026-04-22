//! WSL2 delegate runtime that executes Linux containers inside the `zlayer`
//! WSL2 distro via shell-out to `youki`.
//!
//! Used by [`super::composite::CompositeRuntime`] on Windows hosts to handle
//! Linux-image services alongside HCS-managed Windows-image services. There is
//! **no in-distro daemon**, **no HTTP server**, and **no new crate**: every
//! [`Runtime`] trait method maps to one or more `wsl.exe -d zlayer -- ...`
//! invocations via [`zlayer_wsl::distro::wsl_exec`]. Buffered stdout/stderr is
//! parsed directly; no streaming.
//!
//! # Scope
//!
//! This phase (F-2) wires the dispatch path: every `Runtime` method either
//! (a) executes a `youki` subcommand inside the distro and parses its output,
//! or (b) returns a clean [`AgentError::Unsupported`] when the equivalent
//! shell-out is not meaningful. Full OCI bundle generation for
//! `create_container` is deferred — see the `TODO(F-9)` note in that method.
//!
//! # Error mapping
//!
//! Every shell-out failure maps to [`AgentError::Network`] with a message of
//! the form `youki <subcommand> failed (status <code>): <stderr>` so the user
//! sees both the command that was run and the distro's stderr. The broader
//! `anyhow` failures from [`zlayer_wsl::distro::wsl_exec`] itself (e.g.
//! `wsl.exe` exec failure) flow through [`wsl_exec_or`] into the same
//! [`AgentError::Network`] surface.

#![cfg(all(target_os = "windows", feature = "wsl"))]

use std::collections::HashMap;
use std::net::IpAddr;
use std::process::Output;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use tokio::sync::RwLock;
use zlayer_observability::logs::{LogEntry, LogSource, LogStream};
use zlayer_spec::{PullPolicy, RegistryAuth, ServiceSpec};

use crate::cgroups_stats::ContainerStats;
use crate::error::{AgentError, Result};
use crate::runtime::{
    validate_signal, ContainerId, ContainerInspectDetails, ContainerState, ExecEventStream,
    ImageInfo, PruneResult, Runtime,
};

/// Default in-distro path under which per-container OCI bundles are rooted.
const DEFAULT_BUNDLE_ROOT: &str = "/var/lib/zlayer/bundles";

/// Maximum time we'll poll [`Runtime::wait_container`] before giving up. Youki
/// exposes no blocking `wait` subcommand today, so we poll `state` and bail
/// after a day's worth of polling — plenty for batch jobs, cheap to rerun.
const WAIT_POLL_CAP: Duration = Duration::from_secs(24 * 60 * 60);

/// Polling interval used by [`Runtime::wait_container`] and
/// [`Runtime::stop_container`] while waiting for a container to stop.
const WAIT_POLL_INTERVAL: Duration = Duration::from_millis(500);

/// `Runtime` implementation that shells out to `youki` inside the `zlayer`
/// WSL2 distro.
///
/// Construct via [`Wsl2DelegateRuntime::try_new`], which returns `Ok(None)`
/// when WSL2 (or the helper distro) is not available — callers should treat
/// that as "no Linux-container support on this node" rather than an error.
pub struct Wsl2DelegateRuntime {
    /// In-distro path where container OCI bundles are written under
    /// `<bundle_root>/<container-id>/`. Default: `/var/lib/zlayer/bundles`.
    /// Translated from a Windows path via
    /// [`zlayer_wsl::paths::windows_to_wsl`] when set from a Windows-native
    /// data dir.
    bundle_root: String,
    /// Per-container cache of the Linux PID youki reports after `start`.
    /// Populated lazily; used by [`Runtime::get_container_pid`].
    pids: Arc<RwLock<HashMap<ContainerId, u32>>>,
    /// Per-container cache of the assigned overlay IP, set by an external
    /// caller (the `OverlayManager` / agent's create flow) via
    /// [`Wsl2DelegateRuntime::record_container_ip`]. Returned by
    /// [`Runtime::get_container_ip`].
    ips: Arc<RwLock<HashMap<ContainerId, IpAddr>>>,
}

impl std::fmt::Debug for Wsl2DelegateRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Wsl2DelegateRuntime")
            .field("bundle_root", &self.bundle_root)
            .finish_non_exhaustive()
    }
}

impl Wsl2DelegateRuntime {
    /// Create a [`Wsl2DelegateRuntime`] if WSL2 is available on this host.
    ///
    /// Returns `Ok(None)` when WSL2 is not installed or the helper distro
    /// cannot be set up — callers should treat this as "no Linux-container
    /// support on this node" rather than an error. The probe is best-effort;
    /// transient WSL failures log a warning and return `Ok(None)` so daemon
    /// boot proceeds with HCS-only.
    ///
    /// # Errors
    ///
    /// Never bubbles an error up to the caller — the whole point of
    /// `try_new` returning `Option` is that absence-of-WSL2 is a normal
    /// state on Windows hosts. The `Result` return type is kept to leave
    /// room for future fatal-config errors (e.g. malformed environment).
    pub async fn try_new() -> Result<Option<Self>> {
        // 1. Detect WSL2. Any failure of the detect call is treated as
        //    "no WSL" rather than a hard error.
        let status = match zlayer_wsl::detect::detect_wsl().await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "WSL2 detection failed; Linux container support disabled"
                );
                return Ok(None);
            }
        };
        if !status.wsl2_available {
            tracing::info!(
                wsl_installed = status.wsl_installed,
                "WSL2 not available; Linux container support disabled on this node"
            );
            return Ok(None);
        }

        // 2. Bootstrap the helper distro if missing. Any failure here is
        //    also best-effort — we log and return Ok(None).
        if let Err(e) = zlayer_wsl::setup::ensure_wsl_backend_ready().await {
            tracing::warn!(
                error = %e,
                "failed to bootstrap WSL2 zlayer distro; Linux container support disabled"
            );
            return Ok(None);
        }

        // 3. Verify youki is present inside the distro. Without it, the
        //    delegate cannot do anything useful.
        match zlayer_wsl::distro::wsl_exec("which", &["youki"]).await {
            Ok(output) if output.status.success() => {}
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                tracing::warn!(
                    status = ?output.status.code(),
                    stderr = %stderr,
                    "youki not found in WSL2 distro; Linux container support disabled"
                );
                return Ok(None);
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "probing for youki in WSL2 distro failed; Linux container support disabled"
                );
                return Ok(None);
            }
        }

        Ok(Some(Self {
            bundle_root: DEFAULT_BUNDLE_ROOT.to_string(),
            pids: Arc::new(RwLock::new(HashMap::new())),
            ips: Arc::new(RwLock::new(HashMap::new())),
        }))
    }

    /// Record an overlay IP that has been allocated for the given container.
    ///
    /// Called by the agent's overlay-attach flow before `create_container`,
    /// so [`Runtime::get_container_ip`] can later return the right value.
    pub async fn record_container_ip(&self, id: &ContainerId, ip: IpAddr) {
        self.ips.write().await.insert(id.clone(), ip);
    }

    /// In-distro directory for the given container's OCI bundle.
    fn bundle_dir(&self, id: &ContainerId) -> String {
        format!("{}/{}", self.bundle_root, id_slug(id))
    }

    /// Query `youki state` for the given container and parse its JSON.
    async fn query_state(&self, id: &ContainerId) -> Result<YoukiState> {
        let slug = id_slug(id);
        let output = wsl_exec_or("youki", &["state", &slug]).await?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // Youki emits a consistent "not found" message when the
            // container id is unknown; translate to NotFound so callers get
            // the conventional 404 surface from the API layer.
            if stderr.to_ascii_lowercase().contains("does not exist")
                || stderr.to_ascii_lowercase().contains("not found")
            {
                return Err(AgentError::NotFound {
                    container: id.to_string(),
                    reason: format!("youki state reports container unknown: {stderr}"),
                });
            }
            return Err(youki_error("state", &output));
        }
        parse_youki_state(&output)
    }
}

// ---------------------------------------------------------------------------
// Runtime impl
// ---------------------------------------------------------------------------

#[async_trait]
impl Runtime for Wsl2DelegateRuntime {
    async fn pull_image(&self, image: &str) -> Result<()> {
        self.pull_image_with_policy(image, PullPolicy::IfNotPresent, None)
            .await
    }

    async fn pull_image_with_policy(
        &self,
        image: &str,
        _policy: PullPolicy,
        _auth: Option<&RegistryAuth>,
    ) -> Result<()> {
        let output = wsl_exec_or("youki", &["pull", image]).await?;
        if output.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(AgentError::PullFailed {
                image: image.to_string(),
                reason: format!(
                    "youki pull failed (status {:?}): {}",
                    output.status.code(),
                    stderr.trim()
                ),
            })
        }
    }

    async fn create_container(&self, id: &ContainerId, _spec: &ServiceSpec) -> Result<()> {
        // Ensure the bundle root exists (idempotent). If this fails we have
        // no hope of creating a container — surface the error.
        let bundle_dir = self.bundle_dir(id);
        let mkdir = wsl_exec_or("mkdir", &["-p", &bundle_dir]).await?;
        if !mkdir.status.success() {
            return Err(AgentError::CreateFailed {
                id: id.to_string(),
                reason: format!(
                    "mkdir -p {bundle_dir} failed (status {:?}): {}",
                    mkdir.status.code(),
                    String::from_utf8_lossy(&mkdir.stderr).trim()
                ),
            });
        }

        // TODO(F-9): full OCI bundle generation — currently expects
        // pre-populated bundle at <bundle_dir>. Once the agent wires the
        // builder side, this method will populate `config.json` + rootfs
        // from `_spec` before invoking `youki create`.
        Err(AgentError::Unsupported(format!(
            "WSL2 delegate create_container: OCI bundle generation is not wired yet \
             (expected pre-populated bundle at {bundle_dir}; F-9 will complete this path)"
        )))
    }

    async fn start_container(&self, id: &ContainerId) -> Result<()> {
        let slug = id_slug(id);
        let output = wsl_exec_or("youki", &["start", &slug]).await?;
        if !output.status.success() {
            return Err(AgentError::StartFailed {
                id: id.to_string(),
                reason: format!(
                    "youki start failed (status {:?}): {}",
                    output.status.code(),
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
            });
        }
        // Best-effort: cache the PID for later get_container_pid calls.
        if let Ok(state) = self.query_state(id).await {
            if let Some(pid) = state.pid {
                self.pids.write().await.insert(id.clone(), pid);
            }
        }
        Ok(())
    }

    async fn stop_container(&self, id: &ContainerId, timeout: Duration) -> Result<()> {
        let slug = id_slug(id);
        // SIGTERM first.
        let term = wsl_exec_or("youki", &["kill", "--all", &slug, "SIGTERM"]).await?;
        if !term.status.success() {
            // If the container is already stopped youki returns nonzero;
            // treat that as success for stop semantics rather than erroring.
            let stderr = String::from_utf8_lossy(&term.stderr).to_ascii_lowercase();
            if !(stderr.contains("stopped") || stderr.contains("not running")) {
                return Err(youki_error("kill", &term));
            }
        }

        // Poll `state` until status == "stopped" or timeout expires.
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            match self.query_state(id).await {
                Ok(state) if state.is_stopped() => return Ok(()),
                Ok(_) => {}
                Err(AgentError::NotFound { .. }) => return Ok(()),
                Err(e) => return Err(e),
            }
            if tokio::time::Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(WAIT_POLL_INTERVAL).await;
        }

        // Escalate to SIGKILL.
        let kill = wsl_exec_or("youki", &["kill", "--all", &slug, "SIGKILL"]).await?;
        if !kill.status.success() {
            let stderr = String::from_utf8_lossy(&kill.stderr).to_ascii_lowercase();
            if !(stderr.contains("stopped") || stderr.contains("not running")) {
                return Err(youki_error("kill", &kill));
            }
        }
        Ok(())
    }

    async fn remove_container(&self, id: &ContainerId) -> Result<()> {
        let slug = id_slug(id);
        let output = wsl_exec_or("youki", &["delete", &slug]).await?;
        // Clear caches regardless of delete outcome — the container entry
        // is gone from our perspective once we've called `delete`.
        self.pids.write().await.remove(id);
        self.ips.write().await.remove(id);
        if output.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
            if stderr.contains("does not exist") || stderr.contains("not found") {
                return Ok(());
            }
            Err(youki_error("delete", &output))
        }
    }

    async fn container_state(&self, id: &ContainerId) -> Result<ContainerState> {
        let state = self.query_state(id).await?;
        Ok(state.as_container_state())
    }

    async fn container_logs(&self, id: &ContainerId, tail: usize) -> Result<Vec<LogEntry>> {
        let slug = id_slug(id);
        let log_path = format!("/var/log/youki/{slug}.stdout.log");
        let tail_str = tail.to_string();
        let output = wsl_exec_or("tail", &["-n", &tail_str, &log_path]).await?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
            // First-call-before-any-log-writes: treat missing file as an
            // empty log rather than an error so startup polling is quiet.
            if stderr.contains("no such file") || stderr.contains("cannot open") {
                return Ok(Vec::new());
            }
            return Err(youki_error("tail", &output));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let now = chrono::Utc::now();
        let source = LogSource::Container(id.to_string());
        let service = Some(id.service.clone());
        let entries = stdout
            .lines()
            .filter(|l| !l.is_empty())
            .map(|line| LogEntry {
                timestamp: now,
                stream: LogStream::Stdout,
                message: line.to_string(),
                source: source.clone(),
                service: service.clone(),
                deployment: None,
            })
            .collect();
        Ok(entries)
    }

    async fn exec(&self, id: &ContainerId, cmd: &[String]) -> Result<(i32, String, String)> {
        if cmd.is_empty() {
            return Err(AgentError::InvalidSpec(
                "exec command must not be empty".to_string(),
            ));
        }
        let slug = id_slug(id);
        let mut args: Vec<&str> = vec!["exec", &slug, "--"];
        args.extend(cmd.iter().map(String::as_str));
        let output = wsl_exec_or("youki", &args).await?;
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        let exit = output.status.code().unwrap_or(-1);
        Ok((exit, stdout, stderr))
    }

    async fn get_container_stats(&self, id: &ContainerId) -> Result<ContainerStats> {
        let slug = id_slug(id);
        let output = wsl_exec_or("youki", &["events", "--stats", &slug]).await;
        let now = std::time::Instant::now();
        match output {
            Ok(out) if out.status.success() => Ok(parse_youki_stats(&out, now)),
            _ => {
                // If events isn't supported or the container isn't running,
                // return a default so autoscaling / metrics don't blow up.
                Ok(ContainerStats {
                    cpu_usage_usec: 0,
                    memory_bytes: 0,
                    memory_limit: u64::MAX,
                    timestamp: now,
                })
            }
        }
    }

    async fn wait_container(&self, id: &ContainerId) -> Result<i32> {
        let start = tokio::time::Instant::now();
        loop {
            match self.query_state(id).await {
                Ok(state) if state.is_stopped() => {
                    return Ok(state.exit_code.unwrap_or(0));
                }
                Ok(_) => {}
                Err(e) => return Err(e),
            }
            if start.elapsed() >= WAIT_POLL_CAP {
                return Err(AgentError::Timeout {
                    timeout: WAIT_POLL_CAP,
                });
            }
            tokio::time::sleep(WAIT_POLL_INTERVAL).await;
        }
    }

    async fn get_logs(&self, id: &ContainerId) -> Result<Vec<LogEntry>> {
        self.container_logs(id, usize::MAX).await
    }

    async fn get_container_pid(&self, id: &ContainerId) -> Result<Option<u32>> {
        if let Some(pid) = self.pids.read().await.get(id).copied() {
            return Ok(Some(pid));
        }
        // Fallback: query youki state and update the cache.
        match self.query_state(id).await {
            Ok(state) => {
                if let Some(pid) = state.pid {
                    self.pids.write().await.insert(id.clone(), pid);
                    Ok(Some(pid))
                } else {
                    Ok(None)
                }
            }
            Err(AgentError::NotFound { .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }

    async fn get_container_ip(&self, id: &ContainerId) -> Result<Option<IpAddr>> {
        Ok(self.ips.read().await.get(id).copied())
    }

    async fn list_images(&self) -> Result<Vec<ImageInfo>> {
        // Youki doesn't (yet) ship an image-list subcommand that we can rely
        // on. Be explicit about the unsupported surface rather than pretending
        // to return a successful empty list.
        Err(AgentError::Unsupported(
            "list_images is not supported by the WSL2 delegate runtime \
             (youki has no image registry; images are managed on the host)"
                .to_string(),
        ))
    }

    async fn remove_image(&self, _image: &str, _force: bool) -> Result<()> {
        Err(AgentError::Unsupported(
            "remove_image is not supported by the WSL2 delegate runtime".to_string(),
        ))
    }

    async fn prune_images(&self) -> Result<PruneResult> {
        Err(AgentError::Unsupported(
            "prune_images is not supported by the WSL2 delegate runtime".to_string(),
        ))
    }

    async fn kill_container(&self, id: &ContainerId, signal: Option<&str>) -> Result<()> {
        let canonical = validate_signal(signal.unwrap_or("SIGKILL"))?;
        let slug = id_slug(id);
        let output = wsl_exec_or("youki", &["kill", &slug, &canonical]).await?;
        if output.status.success() {
            Ok(())
        } else {
            Err(youki_error("kill", &output))
        }
    }

    async fn tag_image(&self, _source: &str, _target: &str) -> Result<()> {
        Err(AgentError::Unsupported(
            "tag_image is not supported by the WSL2 delegate runtime".to_string(),
        ))
    }

    async fn inspect_detailed(&self, _id: &ContainerId) -> Result<ContainerInspectDetails> {
        Ok(ContainerInspectDetails::default())
    }

    // Stream-exec falls back to the trait default (buffered exec + single
    // Stdout/Stderr/Exit trio). True streaming over the `wsl.exe` boundary is
    // out of scope for this phase.
    async fn exec_stream(&self, id: &ContainerId, cmd: &[String]) -> Result<ExecEventStream> {
        // Re-dispatch through the default implementation by manually
        // constructing the same events list — avoids pulling in the parent
        // trait's default path via super-trait tricks.
        use crate::runtime::ExecEvent;
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
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Wrapper around [`zlayer_wsl::distro::wsl_exec`] that converts its
/// `anyhow::Error` into [`AgentError::Network`] and keeps every `Runtime`
/// method a one-liner at the call site.
async fn wsl_exec_or(cmd: &str, args: &[&str]) -> Result<Output> {
    zlayer_wsl::distro::wsl_exec(cmd, args)
        .await
        .map_err(|e| AgentError::Network(format!("wsl.exe -d zlayer -- {cmd}: {e}")))
}

/// Build a conventional `AgentError::Network` describing a nonzero youki exit.
///
/// Format: `youki <subcommand> failed (status <code>): <stderr>`. Never
/// swallows stderr so the user sees both the command and the distro's own
/// diagnostic output.
fn youki_error(subcommand: &str, output: &Output) -> AgentError {
    let status = output.status.code();
    let stderr = String::from_utf8_lossy(&output.stderr);
    AgentError::Network(format!(
        "youki {subcommand} failed (status {status:?}): {}",
        stderr.trim()
    ))
}

/// Produce a youki-compatible slug for a [`ContainerId`].
///
/// `ContainerId::Display` emits `"{service}-rep-{replica}"`, which is already
/// alphanumeric-plus-dashes except for the degenerate case where a service
/// name contains characters youki rejects (whitespace, quotes, slashes). This
/// helper filters those so a malformed service spec can't break shell-out
/// argument quoting.
fn id_slug(id: &ContainerId) -> String {
    let raw = id.to_string();
    raw.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// Subset of `youki state` output we care about. Matches the runtime-spec
/// `State` document, which youki emits verbatim.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct YoukiState {
    /// `creating` | `created` | `running` | `stopped`.
    status: String,
    /// PID of the init process inside the container (absent for `creating`).
    #[serde(default)]
    pid: Option<u32>,
    /// Exit code — youki adds this on top of the spec when status is
    /// `stopped`. `None` while the container is still alive.
    #[serde(default)]
    exit_code: Option<i32>,
}

impl YoukiState {
    fn is_stopped(&self) -> bool {
        self.status.eq_ignore_ascii_case("stopped")
    }

    fn as_container_state(&self) -> ContainerState {
        match self.status.to_ascii_lowercase().as_str() {
            "creating" => ContainerState::Pending,
            "created" => ContainerState::Initializing,
            "running" => ContainerState::Running,
            "stopped" => ContainerState::Exited {
                code: self.exit_code.unwrap_or(0),
            },
            other => ContainerState::Failed {
                reason: format!("unknown youki state: {other}"),
            },
        }
    }
}

/// Parse the JSON payload emitted by `youki state <id>`.
fn parse_youki_state(output: &Output) -> Result<YoukiState> {
    let stdout = std::str::from_utf8(&output.stdout)
        .map_err(|e| AgentError::Internal(format!("youki state: stdout not utf-8: {e}")))?;
    serde_json::from_str::<YoukiState>(stdout.trim()).map_err(|e| {
        AgentError::Internal(format!(
            "youki state: failed to parse JSON: {e} (raw: {:?})",
            stdout.chars().take(256).collect::<String>()
        ))
    })
}

/// Best-effort parser for the JSON `youki events --stats <id>` payload.
///
/// Youki follows the `runc`-compatible shape: `{"cpu":{"usage":{"total":N}},
/// "memory":{"usage":{"usage":N,"limit":M}}}`. We tolerate missing fields by
/// defaulting to zero / `u64::MAX` so callers never see a malformed-payload
/// error for a metrics sample.
fn parse_youki_stats(output: &Output, timestamp: std::time::Instant) -> ContainerStats {
    let raw = String::from_utf8_lossy(&output.stdout);
    let v: serde_json::Value = match serde_json::from_str(raw.trim()) {
        Ok(v) => v,
        Err(_) => {
            return ContainerStats {
                cpu_usage_usec: 0,
                memory_bytes: 0,
                memory_limit: u64::MAX,
                timestamp,
            };
        }
    };
    // CPU total is in nanoseconds for runc-style stats; convert to usec.
    let cpu_ns = v
        .pointer("/cpu/usage/total")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let cpu_usage_usec = cpu_ns / 1_000;
    let memory_bytes = v
        .pointer("/memory/usage/usage")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let memory_limit = v
        .pointer("/memory/usage/limit")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(u64::MAX);
    ContainerStats {
        cpu_usage_usec,
        memory_bytes,
        memory_limit,
        timestamp,
    }
}

// ---------------------------------------------------------------------------
// Tests — pure-logic coverage only. Anything that shells out to wsl.exe is
// tested by the Windows-gated integration suite in Phase F-9.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;
    use std::os::windows::process::ExitStatusExt;
    use std::process::ExitStatus;

    fn cid(service: &str, replica: u32) -> ContainerId {
        ContainerId {
            service: service.to_string(),
            replica,
        }
    }

    fn fake_output(stdout: &str, code: i32) -> Output {
        // `ExitStatus::from_raw` takes the raw Windows process exit code as
        // `u32`; negative i32 values are reinterpreted bit-for-bit, which is
        // what we want for synthesising realistic status payloads in tests.
        #[allow(clippy::cast_sign_loss)]
        let raw = code as u32;
        Output {
            status: ExitStatus::from_raw(raw),
            stdout: stdout.as_bytes().to_vec(),
            stderr: Vec::new(),
        }
    }

    #[test]
    fn id_slug_sanitizes_special_chars() {
        // Normal ContainerId formats produce an already-safe slug.
        assert_eq!(id_slug(&cid("web", 0)), "web-rep-0");

        // Weird service names get their offending characters replaced with
        // dashes so youki never sees a shell metacharacter.
        let weird = cid("my svc/with:quotes", 3);
        let slug = id_slug(&weird);
        assert!(
            slug.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "slug must be alnum/dash/underscore only: {slug}"
        );
        assert!(
            slug.contains("my-svc"),
            "slug should preserve alnum: {slug}"
        );
        assert!(slug.ends_with("-rep-3"), "slug should keep replica: {slug}");
    }

    #[test]
    fn parse_youki_state_running() {
        let raw = r#"{"ociVersion":"1.0.2","id":"web-rep-0","status":"running","pid":12345,"bundle":"/var/lib/zlayer/bundles/web-rep-0"}"#;
        let out = fake_output(raw, 0);
        let state = parse_youki_state(&out).expect("valid state JSON");
        assert_eq!(state.status, "running");
        assert_eq!(state.pid, Some(12345));
        assert_eq!(state.exit_code, None);
        assert!(!state.is_stopped());
        assert_eq!(state.as_container_state(), ContainerState::Running);
    }

    #[test]
    fn parse_youki_state_stopped() {
        let raw = r#"{"ociVersion":"1.0.2","id":"job-rep-0","status":"stopped","exitCode":42}"#;
        let out = fake_output(raw, 0);
        let state = parse_youki_state(&out).expect("valid state JSON");
        assert_eq!(state.status, "stopped");
        assert_eq!(state.exit_code, Some(42));
        assert!(state.is_stopped());
        assert_eq!(
            state.as_container_state(),
            ContainerState::Exited { code: 42 }
        );
    }

    #[test]
    fn parse_youki_state_creating_maps_to_pending() {
        let raw = r#"{"ociVersion":"1.0.2","id":"x","status":"creating"}"#;
        let out = fake_output(raw, 0);
        let state = parse_youki_state(&out).expect("valid state JSON");
        assert_eq!(state.as_container_state(), ContainerState::Pending);
    }

    #[test]
    fn parse_youki_state_unknown_maps_to_failed() {
        let raw = r#"{"ociVersion":"1.0.2","id":"x","status":"paused"}"#;
        let out = fake_output(raw, 0);
        let state = parse_youki_state(&out).expect("valid state JSON");
        assert!(matches!(
            state.as_container_state(),
            ContainerState::Failed { .. }
        ));
    }

    #[test]
    fn parse_youki_stats_handles_full_payload() {
        let raw = r#"{"cpu":{"usage":{"total":2500000000}},"memory":{"usage":{"usage":104857600,"limit":268435456}}}"#;
        let out = fake_output(raw, 0);
        let stats = parse_youki_stats(&out, std::time::Instant::now());
        // 2_500_000_000 ns / 1_000 = 2_500_000 usec.
        assert_eq!(stats.cpu_usage_usec, 2_500_000);
        assert_eq!(stats.memory_bytes, 104_857_600);
        assert_eq!(stats.memory_limit, 268_435_456);
    }

    #[test]
    fn parse_youki_stats_defaults_on_malformed() {
        let out = fake_output("not json", 0);
        let stats = parse_youki_stats(&out, std::time::Instant::now());
        assert_eq!(stats.cpu_usage_usec, 0);
        assert_eq!(stats.memory_bytes, 0);
        assert_eq!(stats.memory_limit, u64::MAX);
    }

    #[tokio::test]
    async fn record_container_ip_then_get_returns_it() {
        let runtime = Wsl2DelegateRuntime {
            bundle_root: DEFAULT_BUNDLE_ROOT.to_string(),
            pids: Arc::new(RwLock::new(HashMap::new())),
            ips: Arc::new(RwLock::new(HashMap::new())),
        };
        let id = cid("web", 2);
        let ip = IpAddr::V4(Ipv4Addr::new(10, 200, 0, 42));

        // Before recording, get returns None.
        assert_eq!(runtime.get_container_ip(&id).await.unwrap(), None);

        runtime.record_container_ip(&id, ip).await;
        assert_eq!(runtime.get_container_ip(&id).await.unwrap(), Some(ip));
    }
}
