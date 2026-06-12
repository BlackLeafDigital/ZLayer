//! Docker-based container runtime using bollard
//!
//! Provides cross-platform support for Windows, macOS, and Linux
//! by connecting to the Docker daemon.

use crate::cgroups_stats::ContainerStats;
use crate::error::{AgentError, Result};
use crate::runtime::{
    signal_name_from_exit_code, ArchivePutOptions, ArchiveStream, CommitOptions, CommitOutcome,
    ContainerId, ContainerInspectDetails, ContainerResourceUpdate, ContainerState,
    ContainerUpdateOutcome, ExecEvent, ExecEventStream, ExecHandle, ExecOptions, ExecPtyStream,
    HealthDetail, ImageExportStream, ImageHistoryEntry, ImageInfo, ImageInspectInfo,
    ImageSearchResult, LoadProgress, LoadProgressStream, LogChannel, LogChunk, LogsStream,
    LogsStreamOptions, NetworkAttachmentDetail, PathStat, PruneResult, PullProgress,
    PullProgressStream, Runtime, StatsSample, StatsStream, WaitCondition, WaitOutcome, WaitReason,
};
use bollard::auth::DockerCredentials;
use bollard::errors::Error as BollardError;
use bollard::exec::{CreateExecOptions, ResizeExecOptions, StartExecOptions, StartExecResults};
use bollard::models::{
    ContainerCreateBody, ContainerStatsResponse, ContainerUpdateBody, ContainerWaitResponse,
    CreateImageInfo, DeviceMapping, DeviceRequest, ExecInspectResponse, HostConfig, ImageInspect,
    PortBinding, RestartPolicy, RestartPolicyNameEnum,
};
use bollard::query_parameters::{
    CreateContainerOptions, CreateImageOptions, LogsOptions, RemoveContainerOptions,
    RenameContainerOptionsBuilder, StartContainerOptions, StatsOptions, StopContainerOptions,
    WaitContainerOptions,
};
use bollard::Docker;
use futures_util::{Stream, StreamExt};
use std::collections::HashMap;
use std::pin::Pin;
use std::str::FromStr;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tracing::instrument;
use zlayer_observability::logs::{LogEntry, LogSource, LogStream};
use zlayer_spec::{PullPolicy, RegistryAuth, RegistryAuthType, ServiceSpec};
use zlayer_types::ImageReference;

/// Docker-based container runtime using bollard
///
/// Provides cross-platform support for Windows, macOS, and Linux
/// by connecting to the Docker daemon.
pub struct DockerRuntime {
    docker: Docker,
    /// Auth context for container-to-host API authentication.
    auth_context: Option<crate::runtime::ContainerAuthContext>,
}

impl std::fmt::Debug for DockerRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DockerRuntime").finish_non_exhaustive()
    }
}

impl DockerRuntime {
    /// Create a new Docker runtime connecting to the local Docker daemon
    ///
    /// This will connect to the Docker daemon using platform-specific defaults:
    /// - Linux: Unix socket at `/var/run/docker.sock`
    /// - Windows: Named pipe at `//./pipe/docker_engine`
    /// - macOS: Unix socket at `/var/run/docker.sock`
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The Docker daemon is not running
    /// - The connection to the daemon fails
    /// - The ping to verify connectivity fails
    pub async fn new(auth_context: Option<crate::runtime::ContainerAuthContext>) -> Result<Self> {
        let docker = Docker::connect_with_local_defaults()
            .map_err(|e| AgentError::Internal(format!("Failed to connect to Docker: {e}")))?;

        // Verify connection by pinging the daemon
        docker
            .ping()
            .await
            .map_err(|e| AgentError::Internal(format!("Docker ping failed: {e}")))?;

        tracing::info!("Connected to Docker daemon");
        Ok(Self {
            docker,
            auth_context,
        })
    }

    /// Create a new Docker runtime with custom connection options
    ///
    /// # Arguments
    ///
    /// * `docker` - A pre-configured bollard Docker client
    #[must_use]
    pub fn with_client(docker: Docker) -> Self {
        Self {
            docker,
            auth_context: None,
        }
    }

    /// Perform the actual image pull: create the pull stream and drain its
    /// progress events. Does not consult any pull policy — callers are
    /// responsible for deciding whether a pull should happen.
    ///
    /// When `auth` is `Some`, the inline credentials are forwarded to the
    /// Docker daemon via bollard's `X-Registry-Auth` header. When `None`, the
    /// daemon falls back to its own credential store lookup (the pre-§3.10
    /// behaviour).
    async fn do_pull(&self, image: &str, auth: Option<&RegistryAuth>) -> Result<()> {
        // Parse image into name and tag using the canonical OCI reference parser.
        // On parse failure (e.g. malformed input), fall back to the original
        // helper's no-op shape: `(image, "latest")`.
        //
        // `repository()` strips the registry host, so re-prefix it with
        // `registry()` to preserve the full `registry/repo` form. Otherwise
        // refs like `ghcr.io/org/image:v1` would collapse to `org/image` and
        // the daemon would try to pull from Docker Hub.
        let (name, tag) = match ImageReference::from_str(image) {
            Ok(r) => (
                format!("{}/{}", r.registry(), r.repository()),
                r.tag().unwrap_or("latest").to_string(),
            ),
            Err(_) => (image.to_string(), "latest".to_string()),
        };

        tracing::info!(
            image = %image,
            name = %name,
            tag = %tag,
            inline_auth = auth.is_some(),
            "pulling image"
        );

        let options = CreateImageOptions {
            from_image: Some(name.clone()),
            tag: if tag.is_empty() {
                None
            } else {
                Some(tag.clone())
            },
            ..Default::default()
        };

        // Translate the inline `RegistryAuth` into bollard's
        // `DockerCredentials`. For `Token`-typed auth we put the token in
        // `password` and leave `identitytoken` empty, which bollard/Docker
        // accept as a standard basic-auth-style header — callers using a
        // real OAuth2 identity token can pre-set `username == "<token>"`
        // per Docker CLI convention.
        let credentials = auth.map(docker_credentials_from_registry_auth);

        let mut stream = self.docker.create_image(Some(options), None, credentials);

        while let Some(result) = stream.next().await {
            match result {
                Ok(info) => {
                    if let Some(status) = info.status {
                        tracing::debug!(status = %status, "pull progress");
                    }
                }
                Err(e) => {
                    return Err(AgentError::PullFailed {
                        image: image.to_string(),
                        reason: e.to_string(),
                    });
                }
            }
        }

        tracing::info!(image = %image, "image pulled successfully");
        Ok(())
    }
}

/// Generate a container name from a `ContainerId`
fn container_name(id: &ContainerId) -> String {
    format!("zlayer-{}-{}", id.service, id.replica)
}

/// Validate a container rename target client-side before handing it to Docker.
///
/// Docker rejects empty, whitespace-only, and malformed names with an HTTP 400
/// that bollard surfaces as a generic server error. That is the wrong taxonomy
/// for what is really bad caller input, so we reject those cases up front as
/// [`AgentError::InvalidSpec`].
///
/// The rules mirror Docker's documented constraint
/// (`[a-zA-Z0-9][a-zA-Z0-9_.-]+`): the name must be non-empty after trimming,
/// every character must be in `[a-zA-Z0-9_.-]`, and the first character must be
/// alphanumeric (Docker rejects leading `-`, `.`, etc.).
///
/// Returns the trimmed, validated name on success so callers can use it
/// directly.
fn validate_rename_target(name: &str) -> Result<&str> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(AgentError::InvalidSpec(
            "new container name must not be empty".to_string(),
        ));
    }

    if let Some(bad) = trimmed
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-')))
    {
        return Err(AgentError::InvalidSpec(format!(
            "new container name '{trimmed}' contains invalid character '{bad}'; \
             only characters in [a-zA-Z0-9_.-] are allowed"
        )));
    }

    // SAFETY: non-empty checked above, so `chars().next()` yields a character.
    let first = trimmed.chars().next().unwrap_or_default();
    if !first.is_ascii_alphanumeric() {
        return Err(AgentError::InvalidSpec(format!(
            "new container name '{trimmed}' must start with an alphanumeric \
             character (got '{first}')"
        )));
    }

    Ok(trimmed)
}

/// Split an incoming chunk of exec output into lines and emit one
/// [`ExecEvent`] per complete line through `tx`.
///
/// Any bytes after the final `\n` in `chunk` are stashed in `partial` so they
/// can be prepended to the next chunk — this avoids emitting a half-line as a
/// complete event when Docker happens to split output mid-line.
///
/// Returns `Err(())` if the receiver has been dropped, signalling the pump
/// task should exit.
async fn emit_lines(
    tx: &tokio::sync::mpsc::Sender<ExecEvent>,
    partial: &mut String,
    chunk: &str,
    is_stdout: bool,
) -> std::result::Result<(), ()> {
    // Prepend any previously-buffered partial line.
    let combined: String = if partial.is_empty() {
        chunk.to_string()
    } else {
        let mut s = std::mem::take(partial);
        s.push_str(chunk);
        s
    };

    // Split into lines, keeping track of whether the final piece was
    // terminated by a `\n`. If it wasn't, the last element is a partial line
    // and must be buffered for the next call.
    let ends_with_newline = combined.ends_with('\n');
    let mut parts: Vec<&str> = combined.split('\n').collect();

    // `split` on "a\n" returns ["a", ""], so when the chunk ends with a
    // newline, drop that trailing empty element rather than emitting an empty
    // line and buffering nothing.
    if ends_with_newline {
        parts.pop();
    } else if let Some(last) = parts.pop() {
        // Final element isn't newline-terminated — buffer it.
        partial.push_str(last);
    }

    for line in parts {
        let event = if is_stdout {
            ExecEvent::Stdout(line.to_string())
        } else {
            ExecEvent::Stderr(line.to_string())
        };
        if tx.send(event).await.is_err() {
            return Err(());
        }
    }

    Ok(())
}

/// Translate an inline [`RegistryAuth`] into bollard's [`DockerCredentials`]
/// shape for forwarding to the Docker daemon via the `X-Registry-Auth`
/// header.
///
/// Basic and Token auth both populate `username` + `password`; the token
/// scheme simply carries the token as the password (matching the Docker CLI
/// convention of `docker login -u <token-marker> -p <token>`). `auth_type`
/// is preserved in the returned value via the password field — bollard does
/// not expose a separate "scheme" knob, the daemon infers it from the
/// registry's response to the pull request.
fn docker_credentials_from_registry_auth(auth: &RegistryAuth) -> DockerCredentials {
    // Exhaustively match on `auth_type` so new variants force us to revisit
    // this function. Today Basic and Token both map to the same
    // username+password shape, so there's nothing variant-specific to do.
    match auth.auth_type {
        RegistryAuthType::Basic | RegistryAuthType::Token => {}
    }
    DockerCredentials {
        username: Some(auth.username.clone()),
        password: Some(auth.password.clone()),
        ..Default::default()
    }
}

/// Extract the local image digest for `image` by scanning `repo_digests` for
/// an entry matching `repo_name` (the part before the `@`). Returns the
/// `sha256:...` suffix of the matching entry, or `None` if no matching
/// digest is recorded.
///
/// `ImageInspect.repo_digests` entries look like
/// `"zachhandley/zlayer-manager@sha256:abc..."`. Docker stores one entry per
/// repository that this image has been pulled from, so we match by repo
/// rather than taking the first entry.
fn extract_local_digest(inspect: &ImageInspect, repo_name: &str) -> Option<String> {
    let digests = inspect.repo_digests.as_ref()?;
    digests.iter().find_map(|entry| {
        let (repo, digest) = entry.split_once('@')?;
        if repo == repo_name {
            Some(digest.to_string())
        } else {
            None
        }
    })
}

/// Build the `Config.Labels` map for a container from `spec.labels`.
///
/// Returns `None` when no labels are set so we hand the daemon a missing
/// `Labels` field rather than an empty object — bollard's
/// [`ContainerCreateBody::labels`] is `Option<HashMap<_, _>>` and the daemon
/// treats `None` and an empty map equivalently, but `None` keeps the wire
/// payload minimal.
///
/// The map is forwarded verbatim. Callers that need to stamp reserved-key
/// labels (e.g. `com.zlayer.container_id`) onto the container must inject
/// them into `spec.labels` before invoking [`Runtime::create_container`].
fn build_labels(spec: &ServiceSpec) -> Option<HashMap<String, String>> {
    if spec.labels.is_empty() {
        None
    } else {
        Some(spec.labels.clone())
    }
}

/// Build exposed ports list for Docker container config
///
/// Combines ports declared via `spec.endpoints` (always TCP) and explicit
/// `spec.port_mappings` entries. Duplicate keys are de-duplicated so Docker
/// sees each `"<port>/<proto>"` exactly once.
fn build_exposed_ports(spec: &ServiceSpec) -> Vec<String> {
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut out: Vec<String> = Vec::new();

    for endpoint in &spec.endpoints {
        let key = format!("{}/tcp", endpoint.target_port());
        if seen.insert(key.clone()) {
            out.push(key);
        }
    }

    for mapping in &spec.port_mappings {
        let key = format!("{}/{}", mapping.container_port, mapping.protocol.as_str());
        if seen.insert(key.clone()) {
            out.push(key);
        }
    }

    out
}

/// Build host configuration for Docker container
#[allow(clippy::too_many_lines)]
fn build_host_config(
    spec: &ServiceSpec,
    auth_socket_path: Option<&str>,
    gpu_indices: Option<&[u32]>,
) -> HostConfig {
    let mut port_bindings: HashMap<String, Option<Vec<PortBinding>>> = HashMap::new();

    for endpoint in &spec.endpoints {
        let key = format!("{}/tcp", endpoint.target_port());
        let binding = PortBinding {
            host_ip: Some("0.0.0.0".to_string()),
            host_port: Some(endpoint.port.to_string()),
        };
        port_bindings.insert(key, Some(vec![binding]));
    }

    // Docker-style explicit port publishes from `spec.port_mappings`. Each
    // mapping produces one `PortBinding` entry keyed by
    // `"{container_port}/{protocol}"`. `host_port == None` or `Some(0)` means
    // "assign an ephemeral port" — bollard treats a binding whose `host_port`
    // is `None` as exactly that.
    for mapping in &spec.port_mappings {
        let key = format!("{}/{}", mapping.container_port, mapping.protocol.as_str());
        let host_ip = if mapping.host_ip.is_empty() {
            "0.0.0.0".to_string()
        } else {
            mapping.host_ip.clone()
        };
        let host_port = match mapping.host_port {
            Some(0) | None => None,
            Some(p) => Some(p.to_string()),
        };
        let binding = PortBinding {
            host_ip: Some(host_ip),
            host_port,
        };
        port_bindings
            .entry(key)
            .or_insert_with(|| Some(Vec::new()))
            .as_mut()
            .expect("entry initialised to Some")
            .push(binding);
    }

    // Build memory limit if specified
    let memory = spec.resources.memory.as_ref().and_then(|m| parse_memory(m));

    // Build CPU limit (Docker uses nano-CPUs: 1 CPU = 1e9 nano-CPUs)
    #[allow(clippy::cast_possible_truncation)]
    let nano_cpus = spec.resources.cpu.map(|c| (c * 1_000_000_000.0) as i64);

    // Build device mappings from spec.devices
    let mut devices: Vec<DeviceMapping> = spec
        .devices
        .iter()
        .map(|d| {
            let mut permissions = String::new();
            if d.read {
                permissions.push('r');
            }
            if d.write {
                permissions.push('w');
            }
            if d.mknod {
                permissions.push('m');
            }
            if permissions.is_empty() {
                permissions = "rw".to_string();
            }
            DeviceMapping {
                path_on_host: Some(d.path.clone()),
                path_in_container: Some(d.path.clone()),
                cgroup_permissions: Some(permissions),
            }
        })
        .collect();

    // Build GPU device requests/mappings when spec.resources.gpu is set
    // NVIDIA uses Docker's device_requests (NVIDIA Container Toolkit),
    // AMD/Intel use raw device passthrough since Docker has no native plugin for them.
    let mut device_requests: Option<Vec<DeviceRequest>> = None;
    if let Some(ref gpu) = spec.resources.gpu {
        let indices: Vec<u32> =
            gpu_indices.map_or_else(|| (0..gpu.count).collect(), <[u32]>::to_vec);

        match gpu.vendor.as_str() {
            "nvidia" => {
                // NVIDIA Container Toolkit handles this via device_requests.
                // Use explicit device_ids so the scheduler can pin specific GPUs.
                device_requests = Some(vec![DeviceRequest {
                    driver: Some("nvidia".into()),
                    device_ids: Some(indices.iter().map(ToString::to_string).collect()),
                    count: None, // don't set count when using device_ids
                    capabilities: Some(vec![vec!["gpu".into()]]),
                    ..Default::default()
                }]);
            }
            "amd" => {
                // AMD ROCm - pass through devices directly
                devices.push(DeviceMapping {
                    path_on_host: Some("/dev/kfd".into()),
                    path_in_container: Some("/dev/kfd".into()),
                    cgroup_permissions: Some("rwm".into()),
                });
                for i in &indices {
                    let render_path = format!("/dev/dri/renderD{}", 128 + i);
                    devices.push(DeviceMapping {
                        path_on_host: Some(render_path.clone()),
                        path_in_container: Some(render_path),
                        cgroup_permissions: Some("rwm".into()),
                    });
                    let card_path = format!("/dev/dri/card{i}");
                    devices.push(DeviceMapping {
                        path_on_host: Some(card_path.clone()),
                        path_in_container: Some(card_path),
                        cgroup_permissions: Some("rwm".into()),
                    });
                }
            }
            "intel" => {
                // Intel GPU - pass through DRI devices
                for i in &indices {
                    let render_path = format!("/dev/dri/renderD{}", 128 + i);
                    devices.push(DeviceMapping {
                        path_on_host: Some(render_path.clone()),
                        path_in_container: Some(render_path),
                        cgroup_permissions: Some("rwm".into()),
                    });
                    let card_path = format!("/dev/dri/card{i}");
                    devices.push(DeviceMapping {
                        path_on_host: Some(card_path.clone()),
                        path_in_container: Some(card_path),
                        cgroup_permissions: Some("rwm".into()),
                    });
                }
            }
            other => {
                // Unknown vendor - try DRI render nodes as default
                tracing::warn!(
                    vendor = %other,
                    "Unknown GPU vendor for Docker, attempting DRI device passthrough"
                );
                for i in &indices {
                    let render_path = format!("/dev/dri/renderD{}", 128 + i);
                    devices.push(DeviceMapping {
                        path_on_host: Some(render_path.clone()),
                        path_in_container: Some(render_path),
                        cgroup_permissions: Some("rwm".into()),
                    });
                }
            }
        }
    }

    // Build Linux capabilities to add
    let cap_add = if spec.capabilities.is_empty() {
        None
    } else {
        Some(spec.capabilities.clone())
    };

    // Build bind mounts (e.g. ZLayer auth socket)
    let mut binds: Vec<String> = Vec::new();
    if let Some(socket) = auth_socket_path {
        binds.push(format!(
            "{socket}:{}:ro",
            zlayer_paths::ZLayerDirs::default_socket_path()
        ));
    }

    // --dns / --add-host wiring. When host_network is set the container
    // inherits the host's resolv.conf and /etc/hosts, so we skip both fields.
    //
    // Overlay-resolver injection happens upstream in `ServiceManager`
    // (service.rs): when container DNS is available it pre-populates
    // `spec.dns`, which then flows into `HostConfig.dns` below — no
    // Docker-specific handling is needed here.
    let (dns, extra_hosts) = if spec.host_network {
        (None, None)
    } else {
        let dns = if spec.dns.is_empty() {
            None
        } else {
            Some(spec.dns.clone())
        };
        let extra_hosts = if spec.extra_hosts.is_empty() {
            None
        } else {
            Some(spec.extra_hosts.clone())
        };
        (dns, extra_hosts)
    };

    // Container restart policy → bollard `RestartPolicy`. Delay (if set)
    // is intentionally ignored: bollard's `RestartPolicy` has no per-kind
    // delay — Docker uses its own exponential backoff starting at 100ms.
    let restart_policy = spec.restart_policy.as_ref().map(translate_restart_policy);

    HostConfig {
        port_bindings: Some(port_bindings),
        privileged: Some(spec.privileged),
        memory,
        nano_cpus,
        devices: if devices.is_empty() {
            None
        } else {
            Some(devices)
        },
        device_requests,
        cap_add,
        binds: if binds.is_empty() { None } else { Some(binds) },
        dns,
        extra_hosts,
        restart_policy,
        ..Default::default()
    }
}

/// Translate a `ZLayer` [`zlayer_spec::ContainerRestartPolicy`] into the
/// bollard equivalent.
///
/// `max_attempts` is forwarded only for the `on_failure` kind — Docker
/// ignores the field for all other restart policies. `delay` is not
/// representable in bollard's `RestartPolicy`; if the caller specified one
/// we emit a `warn!` and drop it so the rest of the spec still applies.
fn translate_restart_policy(policy: &zlayer_spec::ContainerRestartPolicy) -> RestartPolicy {
    use zlayer_spec::ContainerRestartKind;

    if policy.delay.is_some() {
        tracing::warn!(
            kind = ?policy.kind,
            delay = ?policy.delay,
            "ContainerRestartPolicy.delay is not supported by the Docker backend; \
             Docker applies its own exponential backoff starting at 100ms"
        );
    }
    match policy.kind {
        ContainerRestartKind::No => RestartPolicy {
            name: Some(RestartPolicyNameEnum::NO),
            maximum_retry_count: None,
        },
        ContainerRestartKind::Always => RestartPolicy {
            name: Some(RestartPolicyNameEnum::ALWAYS),
            maximum_retry_count: None,
        },
        ContainerRestartKind::UnlessStopped => RestartPolicy {
            name: Some(RestartPolicyNameEnum::UNLESS_STOPPED),
            maximum_retry_count: None,
        },
        ContainerRestartKind::OnFailure => RestartPolicy {
            name: Some(RestartPolicyNameEnum::ON_FAILURE),
            maximum_retry_count: policy.max_attempts.map(i64::from),
        },
    }
}

/// Parse a memory string (e.g., "512Mi", "1Gi") to bytes
fn parse_memory(memory: &str) -> Option<i64> {
    let memory = memory.trim();

    // Try to find where the number ends and the unit begins
    let mut split_idx = 0;
    for (i, c) in memory.char_indices() {
        if !c.is_ascii_digit() && c != '.' {
            split_idx = i;
            break;
        }
    }

    if split_idx == 0 {
        return memory.parse::<i64>().ok();
    }

    let (num_str, unit) = memory.split_at(split_idx);
    let num: f64 = num_str.parse().ok()?;

    let multiplier: i64 = match unit.to_uppercase().as_str() {
        "B" | "" => 1,
        "K" | "KB" | "KI" | "KIB" => 1024,
        "M" | "MB" | "MI" | "MIB" => 1024 * 1024,
        "G" | "GB" | "GI" | "GIB" => 1024 * 1024 * 1024,
        "T" | "TB" | "TI" | "TIB" => 1024 * 1024 * 1024 * 1024,
        _ => return None,
    };

    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
    Some((num * multiplier as f64) as i64)
}

/// Map a single frame from bollard's `wait_container` stream to the
/// container's exit code.
///
/// bollard (0.20) does NOT surface a non-zero exit as `Ok`. Its
/// `wait_container` adapter rewrites any `ContainerWaitResponse` whose
/// `status_code > 0` into
/// [`BollardError::DockerContainerWaitError { error, code }`], where `code`
/// IS the container's exit code and `error` is frequently empty (the daemon
/// returns a clean `{"StatusCode":N}` with no `Error` field). A zero exit
/// arrives as `Ok(ContainerWaitResponse { status_code: 0, .. })`.
///
/// Therefore both
/// - `Ok(resp)`                    → `resp.status_code`
/// - `DockerContainerWaitError { code, .. }` → `code`
///
/// are SUCCESSFUL waits and must yield `Ok(exit_code)`. Only genuine
/// transport / not-found failures — the stream closing with no frame, or any
/// other [`BollardError`] variant — are mapped to an [`AgentError`].
#[allow(clippy::cast_possible_truncation)]
fn wait_exit_code_from_frame(
    frame: Option<std::result::Result<ContainerWaitResponse, BollardError>>,
    container: &str,
) -> Result<i32> {
    match frame {
        // Clean terminal frame: exit code 0 (or any code bollard left as Ok).
        Some(Ok(resp)) => Ok(resp.status_code as i32),
        // bollard turns a non-zero (`status_code > 0`) wait result into this
        // error variant — `code` is the real exit code, NOT a wait failure.
        Some(Err(BollardError::DockerContainerWaitError { code, .. })) => Ok(code as i32),
        // Any other bollard error is a genuine transport/daemon failure.
        Some(Err(e)) => Err(AgentError::NotFound {
            container: container.to_string(),
            reason: format!("failed to wait for container: {e}"),
        }),
        // Stream closed without yielding a frame at all.
        None => Err(AgentError::NotFound {
            container: container.to_string(),
            reason: "wait stream closed unexpectedly".to_string(),
        }),
    }
}

#[async_trait::async_trait]
impl Runtime for DockerRuntime {
    /// Pull an image to local storage with default policy (`IfNotPresent`)
    #[instrument(
        skip(self),
        fields(
            otel.name = "image.pull",
            container.image.name = %image,
        )
    )]
    async fn pull_image(&self, image: &str) -> Result<()> {
        self.pull_image_with_policy(
            image,
            PullPolicy::IfNotPresent,
            None,
            zlayer_spec::SourcePolicy::default(),
        )
        .await
    }

    /// Pull an image to local storage with a specific policy, honouring an
    /// optional inline [`RegistryAuth`] (§3.10).
    ///
    /// `_source` is accepted for trait conformance but ignored: the Docker
    /// backend delegates to `dockerd`, which performs its own tier resolution.
    #[instrument(
        skip(self, auth, _source),
        fields(
            otel.name = "image.pull",
            container.image.name = %image,
            pull_policy = ?policy,
            inline_auth = auth.is_some(),
        )
    )]
    async fn pull_image_with_policy(
        &self,
        image: &str,
        policy: PullPolicy,
        auth: Option<&RegistryAuth>,
        _source: zlayer_spec::SourcePolicy,
    ) -> Result<()> {
        // Handle Never policy - don't pull at all
        if matches!(policy, PullPolicy::Never) {
            tracing::debug!(image = %image, "pull policy is Never, skipping pull");
            return Ok(());
        }

        // Handle IfNotPresent - check local presence, and when a tag is
        // involved, compare local vs. upstream digests so we re-pull when the
        // registry has moved forward.
        if matches!(policy, PullPolicy::IfNotPresent) {
            // Step 1: is the image even present locally?
            let local_inspect = match self.docker.inspect_image(image).await {
                Ok(inspect) => inspect,
                Err(BollardError::DockerResponseServerError {
                    status_code: 404, ..
                }) => {
                    tracing::debug!(image = %image, "image not present locally, pulling");
                    return self.do_pull(image, auth).await;
                }
                Err(e) => {
                    return Err(AgentError::Internal(format!(
                        "failed to inspect image '{image}': {e}"
                    )));
                }
            };

            // Step 2: if it's pinned by digest, local presence is sufficient.
            // Parse via the canonical OCI reference parser; on parse failure,
            // fall back to treating the input as the repository name with the
            // default `latest` tag (matches the old helper's infallible shape).
            let parsed = ImageReference::from_str(image).ok();
            // `repository()` alone strips the registry host; re-prefix with
            // `registry()` so the name matches the `registry/repo` form that
            // appears in `repo_digests` and avoids accidentally pulling from
            // Docker Hub for non-default registries.
            let name = parsed.as_ref().map_or_else(
                || image.to_string(),
                |r| format!("{}/{}", r.registry(), r.repository()),
            );
            // The previous helper returned an empty tag for digest-only refs
            // (e.g. `image@sha256:...`). Mirror that by checking whether the
            // parsed reference has no tag — in which case it's pinned by
            // digest and local presence is sufficient.
            let pinned_by_digest = parsed.as_ref().is_some_and(|r| r.tag().is_none());
            if pinned_by_digest {
                tracing::debug!(
                    image = %image,
                    "image pinned by digest and present, skipping pull"
                );
                return Ok(());
            }

            // Step 3: compare local vs remote digests.
            let local_digest = extract_local_digest(&local_inspect, &name);

            match self.docker.inspect_registry_image(image, None).await {
                Ok(dist) => {
                    let remote_digest = dist.descriptor.digest;
                    match (&local_digest, &remote_digest) {
                        (Some(local), Some(remote)) if local == remote => {
                            tracing::debug!(
                                image = %image,
                                digest = %local,
                                "image up-to-date, skipping pull"
                            );
                            return Ok(());
                        }
                        (Some(local), Some(remote)) => {
                            tracing::info!(
                                image = %image,
                                local = %local,
                                remote = %remote,
                                "image digests differ, re-pulling"
                            );
                            return self.do_pull(image, auth).await;
                        }
                        _ => {
                            tracing::info!(
                                image = %image,
                                "local or remote digest missing, re-pulling to be safe"
                            );
                            return self.do_pull(image, auth).await;
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        image = %image,
                        error = %e,
                        "failed to query registry digest, using local image"
                    );
                    return Ok(());
                }
            }
        }

        self.do_pull(image, auth).await
    }

    /// Create a container from the given spec
    #[instrument(
        skip(self, spec),
        fields(
            otel.name = "container.create",
            container.id = %container_name(id),
            service.name = %id.service,
            service.replica = %id.replica,
            container.image.name = %spec.image.name,
        )
    )]
    async fn create_container(&self, id: &ContainerId, spec: &ServiceSpec) -> Result<()> {
        let name = container_name(id);

        // Clean up any stale container with the same name from a previous deploy
        if self.docker.inspect_container(&name, None).await.is_ok() {
            tracing::warn!(
                container = %name,
                "stale container already exists, removing before re-create"
            );
            // Attempt to stop — ignore errors since it may already be stopped
            let _ = self
                .docker
                .stop_container(
                    &name,
                    Some(StopContainerOptions {
                        t: Some(5),
                        ..Default::default()
                    }),
                )
                .await;
            self.docker
                .remove_container(
                    &name,
                    Some(RemoveContainerOptions {
                        force: true,
                        ..Default::default()
                    }),
                )
                .await
                .map_err(|e| {
                    AgentError::Internal(format!("failed to remove stale container {name}: {e}"))
                })?;
            tracing::info!(container = %name, "stale container removed");
        }

        // Build environment variables
        let mut env: Vec<String> = spec.env.iter().map(|(k, v)| format!("{k}={v}")).collect();

        // Inject auth env vars if auth context is configured
        if let Some(ref auth_ctx) = self.auth_context {
            let token = crate::auth::mint_container_token(
                &auth_ctx.jwt_secret,
                &id.service,
                &format!("{}-{}", id.service, id.replica),
                std::time::Duration::from_secs(86400 * 365),
            )
            .map_err(|e| crate::error::AgentError::CreateFailed {
                id: id.to_string(),
                reason: format!("Failed to mint container token: {e}"),
            })?;
            env.push(format!("ZLAYER_API_URL={}", auth_ctx.api_url));
            env.push(format!("ZLAYER_TOKEN={token}"));
            env.push(format!(
                "ZLAYER_SOCKET={}",
                zlayer_paths::ZLayerDirs::default_socket_path()
            ));
        }

        // Inject GPU device visibility environment variables
        if let Some(ref gpu) = spec.resources.gpu {
            let indices: Vec<String> = (0..gpu.count).map(|i| i.to_string()).collect();
            let device_list = indices.join(",");
            match gpu.vendor.as_str() {
                "nvidia" => {
                    env.push(format!("NVIDIA_VISIBLE_DEVICES={device_list}"));
                    env.push(format!("CUDA_VISIBLE_DEVICES={device_list}"));
                }
                "amd" => {
                    env.push(format!("ROCR_VISIBLE_DEVICES={device_list}"));
                    env.push(format!("HIP_VISIBLE_DEVICES={device_list}"));
                }
                "intel" => {
                    env.push(format!("ZE_AFFINITY_MASK={device_list}"));
                }
                _ => {}
            }
        }

        // Inject distributed training coordination env vars
        if let Some(ref gpu) = spec.resources.gpu {
            if let Some(ref dist) = gpu.distributed {
                env.push(format!("MASTER_PORT={}", dist.master_port));
                env.push(format!("MASTER_ADDR={}", id.service));
                env.push("WORLD_SIZE=1".to_string());
                env.push("RANK=0".to_string());
                env.push("LOCAL_RANK=0".to_string());
                match dist.backend.as_str() {
                    "nccl" => env.push("NCCL_SOCKET_IFNAME=eth0".to_string()),
                    "gloo" => env.push("GLOO_SOCKET_IFNAME=eth0".to_string()),
                    _ => {}
                }
            }
        }

        // Build exposed ports
        let exposed_ports = build_exposed_ports(spec);

        // Build host config (pass socket path for bind mount when auth is configured)
        let auth_socket = self
            .auth_context
            .as_ref()
            .map(|ctx| ctx.socket_path.as_str());
        let host_config = build_host_config(spec, auth_socket, None);

        // Build command/entrypoint
        let cmd = spec.command.args.clone();
        let entrypoint = spec.command.entrypoint.clone();
        let working_dir = spec.command.workdir.clone();

        // Docker's hostname field is silently ignored when sharing the host
        // network namespace, so we skip it to avoid confusing log noise.
        let hostname = if spec.host_network {
            None
        } else {
            spec.hostname.clone()
        };

        let config = ContainerCreateBody {
            image: Some(spec.image.name.to_string()),
            hostname,
            env: if env.is_empty() { None } else { Some(env) },
            cmd,
            entrypoint,
            working_dir,
            exposed_ports: if exposed_ports.is_empty() {
                None
            } else {
                Some(exposed_ports)
            },
            labels: build_labels(spec),
            host_config: Some(host_config),
            ..Default::default()
        };

        let options = CreateContainerOptions {
            name: Some(name.clone()),
            platform: String::new(),
        };

        tracing::info!(container = %name, image = %spec.image.name, "creating container");

        self.docker
            .create_container(Some(options), config)
            .await
            .map_err(|e| AgentError::CreateFailed {
                id: name.clone(),
                reason: e.to_string(),
            })?;

        tracing::info!(container = %name, "container created successfully");
        Ok(())
    }

    /// Start a container
    #[instrument(
        skip(self),
        fields(
            otel.name = "container.start",
            container.id = %container_name(id),
            service.name = %id.service,
        )
    )]
    async fn start_container(&self, id: &ContainerId) -> Result<()> {
        let name = container_name(id);

        tracing::info!(container = %name, "starting container");

        self.docker
            .start_container(&name, None::<StartContainerOptions>)
            .await
            .map_err(|e| AgentError::StartFailed {
                id: name.clone(),
                reason: e.to_string(),
            })?;

        tracing::info!(container = %name, "container started successfully");
        Ok(())
    }

    /// Stop a container with a timeout
    #[instrument(
        skip(self),
        fields(
            otel.name = "container.stop",
            container.id = %container_name(id),
            service.name = %id.service,
            timeout_ms = %timeout.as_millis(),
        )
    )]
    async fn stop_container(&self, id: &ContainerId, timeout: Duration) -> Result<()> {
        let name = container_name(id);

        tracing::info!(container = %name, timeout = ?timeout, "stopping container");

        #[allow(clippy::cast_possible_truncation)]
        let options = StopContainerOptions {
            t: Some(timeout.as_secs() as i32),
            signal: None,
        };

        self.docker
            .stop_container(&name, Some(options))
            .await
            .map_err(|e| AgentError::NotFound {
                container: name.clone(),
                reason: format!("failed to stop container: {e}"),
            })?;

        tracing::info!(container = %name, "container stopped successfully");
        Ok(())
    }

    /// Remove a container
    #[instrument(
        skip(self),
        fields(
            otel.name = "container.remove",
            container.id = %container_name(id),
            service.name = %id.service,
        )
    )]
    async fn remove_container(&self, id: &ContainerId) -> Result<()> {
        let name = container_name(id);

        tracing::info!(container = %name, "removing container");

        let options = RemoveContainerOptions {
            force: true,
            ..Default::default()
        };

        self.docker
            .remove_container(&name, Some(options))
            .await
            .map_err(|e| AgentError::NotFound {
                container: name.clone(),
                reason: format!("failed to remove container: {e}"),
            })?;

        tracing::info!(container = %name, "container removed successfully");
        Ok(())
    }

    /// Get container state by inspecting the container
    #[instrument(
        skip(self),
        fields(
            otel.name = "container.state",
            container.id = %container_name(id),
            service.name = %id.service,
        )
    )]
    async fn container_state(&self, id: &ContainerId) -> Result<ContainerState> {
        let name = container_name(id);

        let inspect = self
            .docker
            .inspect_container(&name, None)
            .await
            .map_err(|e| AgentError::NotFound {
                container: name.clone(),
                reason: format!("failed to inspect container: {e}"),
            })?;

        // Extract the state from the inspection result
        let state = inspect.state.ok_or_else(|| {
            AgentError::Internal(format!("Container {name} has no state information"))
        })?;

        // Map Docker state to our ContainerState enum
        let container_state = match state.status {
            Some(bollard::models::ContainerStateStatusEnum::CREATED) => ContainerState::Pending,
            Some(
                bollard::models::ContainerStateStatusEnum::RUNNING
                | bollard::models::ContainerStateStatusEnum::PAUSED,
            ) => ContainerState::Running, // Treat paused as running
            Some(bollard::models::ContainerStateStatusEnum::RESTARTING) => {
                ContainerState::Initializing
            }
            Some(bollard::models::ContainerStateStatusEnum::REMOVING) => ContainerState::Stopping,
            Some(bollard::models::ContainerStateStatusEnum::EXITED) => {
                #[allow(clippy::cast_possible_truncation)]
                let code = state.exit_code.unwrap_or(0) as i32;
                ContainerState::Exited { code }
            }
            Some(bollard::models::ContainerStateStatusEnum::DEAD) => {
                let error = state
                    .error
                    .unwrap_or_else(|| "container is dead".to_string());
                ContainerState::Failed { reason: error }
            }
            None | Some(bollard::models::ContainerStateStatusEnum::EMPTY) => {
                ContainerState::Pending
            }
        };

        tracing::debug!(container = %name, state = ?container_state, "got container state");
        Ok(container_state)
    }

    /// Get container logs with a tail limit
    #[instrument(
        skip(self),
        fields(
            otel.name = "container.logs",
            container.id = %container_name(id),
            service.name = %id.service,
            tail = %tail,
        )
    )]
    async fn container_logs(&self, id: &ContainerId, tail: usize) -> Result<Vec<LogEntry>> {
        let name = container_name(id);

        let options = LogsOptions {
            stdout: true,
            stderr: true,
            tail: tail.to_string(),
            timestamps: false,
            ..Default::default()
        };

        let mut stream = self.docker.logs(&name, Some(options));
        let mut entries = Vec::new();
        let source = LogSource::Container(name.clone());

        while let Some(result) = stream.next().await {
            match result {
                Ok(log_output) => {
                    let (stream_type, text) = match &log_output {
                        bollard::container::LogOutput::StdOut { message } => {
                            (LogStream::Stdout, String::from_utf8_lossy(message))
                        }
                        bollard::container::LogOutput::StdErr { message } => {
                            (LogStream::Stderr, String::from_utf8_lossy(message))
                        }
                        _ => (LogStream::Stdout, log_output.to_string().into()),
                    };
                    for line in text.lines() {
                        entries.push(LogEntry {
                            timestamp: chrono::Utc::now(),
                            stream: stream_type,
                            message: line.to_string(),
                            source: source.clone(),
                            service: Some(id.service.clone()),
                            deployment: None,
                        });
                    }
                }
                Err(e) => {
                    return Err(AgentError::NotFound {
                        container: name.clone(),
                        reason: format!("failed to get logs: {e}"),
                    });
                }
            }
        }

        tracing::debug!(container = %name, entry_count = entries.len(), "got container logs");
        Ok(entries)
    }

    /// Execute a command inside a container
    ///
    /// Returns a tuple of (`exit_code`, stdout, stderr)
    #[instrument(
        skip(self, cmd),
        fields(
            otel.name = "container.exec",
            container.id = %container_name(id),
            service.name = %id.service,
            cmd = ?cmd,
        )
    )]
    async fn exec(&self, id: &ContainerId, cmd: &[String]) -> Result<(i32, String, String)> {
        let name = container_name(id);

        // Create the exec instance
        let exec_options = CreateExecOptions {
            cmd: Some(cmd.to_vec()),
            attach_stdout: Some(true),
            attach_stderr: Some(true),
            ..Default::default()
        };

        let exec_created = self
            .docker
            .create_exec(&name, exec_options)
            .await
            .map_err(|e| AgentError::NotFound {
                container: name.clone(),
                reason: format!("failed to create exec: {e}"),
            })?;

        // Start the exec and collect output
        let start_result = self
            .docker
            .start_exec(&exec_created.id, None)
            .await
            .map_err(|e| AgentError::Internal(format!("failed to start exec: {e}")))?;

        let mut stdout = String::new();
        let mut stderr = String::new();

        match start_result {
            StartExecResults::Attached { mut output, .. } => {
                while let Some(result) = output.next().await {
                    match result {
                        Ok(bollard::container::LogOutput::StdOut { message }) => {
                            stdout.push_str(&String::from_utf8_lossy(&message));
                        }
                        Ok(bollard::container::LogOutput::StdErr { message }) => {
                            stderr.push_str(&String::from_utf8_lossy(&message));
                        }
                        Ok(_) => {} // Ignore stdin and console output
                        Err(e) => {
                            tracing::warn!(error = %e, "error reading exec output");
                        }
                    }
                }
            }
            StartExecResults::Detached => {
                // This shouldn't happen since we didn't request detached mode
                tracing::warn!("exec started in detached mode unexpectedly");
            }
        }

        // Inspect the exec to get the exit code
        let exec_inspect = self
            .docker
            .inspect_exec(&exec_created.id)
            .await
            .map_err(|e| AgentError::Internal(format!("failed to inspect exec: {e}")))?;

        #[allow(clippy::cast_possible_truncation)]
        let exit_code = exec_inspect.exit_code.unwrap_or(0) as i32;

        tracing::debug!(
            container = %name,
            exit_code = exit_code,
            stdout_len = stdout.len(),
            stderr_len = stderr.len(),
            "exec completed"
        );

        Ok((exit_code, stdout, stderr))
    }

    /// Execute a command inside a container and stream output events as
    /// they arrive.
    ///
    /// Creates a Docker exec instance, attaches to it, and pumps
    /// `bollard::container::LogOutput` chunks into an mpsc channel. Output is
    /// split on `\n` and each line (including an empty trailing chunk when the
    /// data ends on a newline) is emitted as a separate [`ExecEvent::Stdout`]
    /// or [`ExecEvent::Stderr`]. Once the bollard stream closes, the exec is
    /// inspected for its exit code and a final [`ExecEvent::Exit`] is sent
    /// before the channel is dropped.
    #[instrument(
        skip(self, cmd),
        fields(
            otel.name = "container.exec_stream",
            container.id = %container_name(id),
            service.name = %id.service,
            cmd = ?cmd,
        )
    )]
    async fn exec_stream(&self, id: &ContainerId, cmd: &[String]) -> Result<ExecEventStream> {
        let name = container_name(id);

        let exec_options = CreateExecOptions {
            cmd: Some(cmd.to_vec()),
            attach_stdout: Some(true),
            attach_stderr: Some(true),
            ..Default::default()
        };

        let exec_created = self
            .docker
            .create_exec(&name, exec_options)
            .await
            .map_err(|e| AgentError::NotFound {
                container: name.clone(),
                reason: format!("failed to create exec: {e}"),
            })?;

        let start_result = self
            .docker
            .start_exec(&exec_created.id, None)
            .await
            .map_err(|e| AgentError::Internal(format!("failed to start exec: {e}")))?;

        // Bounded channel provides backpressure if the client reads slower
        // than Docker produces output.
        let (tx, rx) = tokio::sync::mpsc::channel::<ExecEvent>(256);
        let docker = self.docker.clone();
        let exec_id = exec_created.id.clone();
        let container_name_for_task = name.clone();

        tokio::spawn(async move {
            // `stdout_partial` / `stderr_partial` buffer any trailing bytes
            // that weren't terminated by `\n`, so they can be joined with the
            // next chunk instead of being emitted as a premature line.
            let mut stdout_partial = String::new();
            let mut stderr_partial = String::new();

            if let StartExecResults::Attached { mut output, .. } = start_result {
                while let Some(result) = output.next().await {
                    match result {
                        Ok(bollard::container::LogOutput::StdOut { message }) => {
                            let text = String::from_utf8_lossy(&message).into_owned();
                            if emit_lines(&tx, &mut stdout_partial, &text, true)
                                .await
                                .is_err()
                            {
                                // Receiver dropped -- give up.
                                return;
                            }
                        }
                        Ok(bollard::container::LogOutput::StdErr { message }) => {
                            let text = String::from_utf8_lossy(&message).into_owned();
                            if emit_lines(&tx, &mut stderr_partial, &text, false)
                                .await
                                .is_err()
                            {
                                return;
                            }
                        }
                        Ok(_) => {}
                        Err(e) => {
                            tracing::warn!(
                                container = %container_name_for_task,
                                error = %e,
                                "error reading exec stream output"
                            );
                        }
                    }
                }
            } else {
                tracing::warn!(
                    container = %container_name_for_task,
                    "exec started in detached mode unexpectedly"
                );
            }

            // Flush any trailing partial lines (output that didn't end in
            // `\n`). This preserves the last chunk of output from short
            // commands that print without a newline.
            if !stdout_partial.is_empty()
                && tx
                    .send(ExecEvent::Stdout(std::mem::take(&mut stdout_partial)))
                    .await
                    .is_err()
            {
                return;
            }
            if !stderr_partial.is_empty()
                && tx
                    .send(ExecEvent::Stderr(std::mem::take(&mut stderr_partial)))
                    .await
                    .is_err()
            {
                return;
            }

            // Fetch the exit code now that the stream has closed.
            let exit_code = match docker.inspect_exec(&exec_id).await {
                Ok(inspect) => {
                    #[allow(clippy::cast_possible_truncation)]
                    let code = inspect.exit_code.unwrap_or(0) as i32;
                    code
                }
                Err(e) => {
                    tracing::warn!(
                        container = %container_name_for_task,
                        error = %e,
                        "failed to inspect exec for exit code"
                    );
                    0
                }
            };

            let _ = tx.send(ExecEvent::Exit(exit_code)).await;
        });

        Ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx)))
    }

    /// Start a long-lived interactive exec session against a running
    /// container.
    ///
    /// Mirrors Docker's two-step flow: `POST /containers/{id}/exec` to mint
    /// the exec instance, then `POST /exec/{id}/start` with `Detach: false,
    /// Tty: <opts.tty>` to upgrade the HTTP connection into a hijacked
    /// duplex stream. The returned [`ExecHandle`] bundles three independent
    /// halves the caller drives concurrently:
    ///
    /// - `stream` — bollard's split `output` (`Stream<LogOutput>`) and
    ///   `input` (`AsyncWrite`) stitched together via [`ExecPtyDuplex`] so
    ///   the caller sees a single `AsyncRead + AsyncWrite` byte channel.
    /// - `resize` — an `mpsc::Receiver<(rows, cols)>` drained by a spawned
    ///   task that calls `Docker::resize_exec` for every tick. The bound is
    ///   small (`8`) on purpose: resize events are bursty (each xterm
    ///   resize fires a flurry of `SIGWINCH`-driven sends), and dropping
    ///   the older sizes is fine because only the latest geometry matters.
    /// - `exit` — a `Pin<Box<dyn Future<...>>>` that resolves once the
    ///   bollard output stream EOFs. We then call `inspect_exec` to read
    ///   the actual exit code (Docker only reports it through the inspect
    ///   endpoint; the hijacked stream itself just closes).
    #[instrument(
        skip(self, opts),
        fields(
            otel.name = "container.exec_pty",
            container.id = %container_name(id),
            service.name = %id.service,
            tty = opts.tty,
        )
    )]
    async fn exec_pty(&self, id: &ContainerId, opts: ExecOptions) -> Result<ExecHandle> {
        let name = container_name(id);

        let create_opts = build_create_exec_options(&opts);

        let exec_created = self
            .docker
            .create_exec(&name, create_opts)
            .await
            .map_err(|e| AgentError::NotFound {
                container: name.clone(),
                reason: format!("failed to create exec: {e}"),
            })?;

        // Force `detach: false` — a detached start would return
        // `StartExecResults::Detached` and leave us with no I/O channels,
        // which is incompatible with the `ExecHandle` contract. The `tty`
        // flag has to match what we passed to `create_exec`; bollard
        // doesn't enforce that for us.
        let start_result = self
            .docker
            .start_exec(
                &exec_created.id,
                Some(StartExecOptions {
                    detach: false,
                    tty: opts.tty,
                    output_capacity: None,
                }),
            )
            .await
            .map_err(|e| AgentError::Internal(format!("failed to start exec: {e}")))?;

        let (output, input) = match start_result {
            StartExecResults::Attached { output, input } => (output, input),
            StartExecResults::Detached => {
                return Err(AgentError::Internal(
                    "exec started in detached mode despite detach=false".into(),
                ));
            }
        };

        let duplex = ExecPtyDuplex {
            output,
            input,
            current_chunk: bytes::Bytes::new(),
            output_done: false,
        };
        let stream: ExecPtyStream = Box::new(duplex);

        // Resize task: drain the receiver and forward every (rows, cols)
        // pair to Docker. The channel closes when the caller drops the
        // sender (typically when the session ends), at which point this
        // task naturally exits.
        let (resize_tx, mut resize_rx) = tokio::sync::mpsc::channel::<(u16, u16)>(8);
        let docker_for_resize = self.docker.clone();
        let exec_id_for_resize = exec_created.id.clone();
        let container_for_resize = name.clone();
        tokio::spawn(async move {
            while let Some((rows, cols)) = resize_rx.recv().await {
                let opts = resize_options_for(rows, cols);
                if let Err(e) = docker_for_resize
                    .resize_exec(&exec_id_for_resize, opts)
                    .await
                {
                    // Docker returns 404 if the exec already exited, which
                    // is benign — log at debug so noisy resizes after exit
                    // don't pollute warn-level logs.
                    tracing::debug!(
                        container = %container_for_resize,
                        rows = rows,
                        cols = cols,
                        error = %e,
                        "resize_exec failed (exec may have exited)"
                    );
                }
            }
        });

        // Exit future: spun up here so the boxed future can own its handles
        // independently of the duplex stream. Note we cannot `await` the
        // bollard output stream from inside this future because the stream
        // is already moved into the duplex — instead, the exit future
        // polls a oneshot signalled by the caller's reader hitting EOF.
        //
        // To keep things simple and avoid forcing the caller to manually
        // notify EOF, we instead poll `inspect_exec` periodically until
        // the exec reports `running == Some(false)`. That's exactly how
        // Docker CLI's `docker exec -it` waits for completion, and it
        // tolerates the caller dropping the duplex stream without ever
        // reading to EOF.
        let docker_for_exit = self.docker.clone();
        let exec_id_for_exit = exec_created.id.clone();
        let exit: crate::runtime::ExecExitFuture = Box::pin(async move {
            // Polling cadence: 100ms is fast enough to feel instant for
            // interactive shells (a `exit` typed at the prompt is usually
            // observed within 1–2 ticks) and slow enough that the daemon
            // doesn't burn cycles inspecting a quiescent exec.
            let interval = Duration::from_millis(100);
            loop {
                match docker_for_exit.inspect_exec(&exec_id_for_exit).await {
                    Ok(inspect) => {
                        if inspect.running == Some(false) {
                            return Ok(extract_exec_exit_code(&inspect));
                        }
                    }
                    Err(e) => {
                        return Err(AgentError::Internal(format!(
                            "failed to inspect exec for exit code: {e}"
                        )));
                    }
                }
                tokio::time::sleep(interval).await;
            }
        });

        Ok(ExecHandle {
            stream,
            resize: resize_tx,
            exit,
        })
    }

    /// Get container resource statistics (CPU and memory)
    #[instrument(
        skip(self),
        fields(
            otel.name = "container.stats",
            container.id = %container_name(id),
            service.name = %id.service,
        )
    )]
    async fn get_container_stats(&self, id: &ContainerId) -> Result<ContainerStats> {
        let name = container_name(id);

        // Get a single stats snapshot (stream: false)
        let options = StatsOptions {
            stream: false,
            one_shot: true,
        };

        let mut stream = self.docker.stats(&name, Some(options));

        let stats = stream
            .next()
            .await
            .ok_or_else(|| AgentError::NotFound {
                container: name.clone(),
                reason: "no stats available".to_string(),
            })?
            .map_err(|e| AgentError::NotFound {
                container: name.clone(),
                reason: format!("failed to get stats: {e}"),
            })?;

        // Extract CPU usage from Docker stats
        // Docker provides cumulative CPU usage in nanoseconds
        // In bollard 0.20+, cpu_stats and memory_stats are Option<T>
        let cpu_usage_usec = stats
            .cpu_stats
            .and_then(|s| s.cpu_usage)
            .and_then(|u| u.total_usage)
            .unwrap_or(0)
            / 1000; // Convert nanoseconds to microseconds

        // Extract memory usage
        let memory_bytes = stats
            .memory_stats
            .as_ref()
            .and_then(|s| s.usage)
            .unwrap_or(0);

        // Extract memory limit
        let memory_limit = stats.memory_stats.and_then(|s| s.limit).unwrap_or(u64::MAX);

        let container_stats = ContainerStats {
            cpu_usage_usec,
            memory_bytes,
            memory_limit,
            timestamp: Instant::now(),
        };

        tracing::debug!(
            container = %name,
            cpu_usec = cpu_usage_usec,
            memory_bytes = memory_bytes,
            memory_limit = memory_limit,
            "got container stats"
        );

        Ok(container_stats)
    }

    /// Wait for a container to exit and return its exit code
    #[instrument(
        skip(self),
        fields(
            otel.name = "container.wait",
            container.id = %container_name(id),
            service.name = %id.service,
        )
    )]
    async fn wait_container(&self, id: &ContainerId) -> Result<i32> {
        let name = container_name(id);

        tracing::debug!(container = %name, "waiting for container to exit");

        let options = WaitContainerOptions {
            condition: "not-running".to_string(),
        };

        let mut stream = self.docker.wait_container(&name, Some(options));

        // Get the first (and only) result from the wait stream. bollard maps a
        // non-zero exit into `DockerContainerWaitError { code, .. }`, so the
        // exit-code extraction is centralized in `wait_exit_code_from_frame`.
        let exit_code = wait_exit_code_from_frame(stream.next().await, &name)?;

        tracing::info!(container = %name, exit_code = exit_code, "container exited");

        Ok(exit_code)
    }

    /// Wait for a container to exit and return a richer [`WaitOutcome`].
    ///
    /// Reads `state.oom_killed`, `state.exit_code`, and `state.finished_at`
    /// from `inspect_container` to classify the exit:
    ///
    /// - `OomKilled` when `state.oom_killed == Some(true)`.
    /// - `Signal` when the exit code looks like `128 + N` (Docker's convention
    ///   for signal-caused exits); `signal` is derived from `N` via
    ///   [`signal_name_from_exit_code`].
    /// - `RuntimeError` when `state.error` is non-empty with a zero-ish exit
    ///   code (runtime-side failure before the process really exited).
    /// - `Exited` otherwise.
    #[instrument(
        skip(self),
        fields(
            otel.name = "container.wait_outcome",
            container.id = %container_name(id),
            service.name = %id.service,
        )
    )]
    async fn wait_outcome(&self, id: &ContainerId) -> Result<WaitOutcome> {
        let name = container_name(id);

        // First block on the normal wait stream so we only inspect *after*
        // the container has actually stopped.
        let exit_code_from_wait = self.wait_container(id).await?;

        // Then inspect to pick up OOMKilled / FinishedAt / Error.
        let inspect = self
            .docker
            .inspect_container(&name, None)
            .await
            .map_err(|e| AgentError::NotFound {
                container: name.clone(),
                reason: format!("failed to inspect container after wait: {e}"),
            })?;

        let state = inspect.state.unwrap_or_default();

        // Prefer the inspect exit_code when present (more authoritative);
        // otherwise fall back to the wait-stream value.
        #[allow(clippy::cast_possible_truncation)]
        let exit_code = state.exit_code.map_or(exit_code_from_wait, |c| c as i32);

        let oom = state.oom_killed.unwrap_or(false);
        let error = state.error.unwrap_or_default();
        let error_trimmed = error.trim();

        let (reason, signal) = if oom {
            (WaitReason::OomKilled, None)
        } else if let Some(sig) = signal_name_from_exit_code(exit_code) {
            // Docker convention: exit_code = 128 + signal_number when the
            // process was killed by a signal. We treat anything in that
            // range as a signal death.
            (WaitReason::Signal, Some(sig))
        } else if !error_trimmed.is_empty() && exit_code == 0 {
            // Runtime reported an error but the process never produced a
            // real exit code -- treat as a runtime-side failure.
            (WaitReason::RuntimeError, None)
        } else {
            (WaitReason::Exited, None)
        };

        // `FinishedAt` is an RFC3339 string like "2024-01-02T15:04:05.123456789Z".
        // Docker emits the zero value ("0001-01-01T00:00:00Z") for containers
        // that never ran; treat that as "no timestamp".
        let finished_at = state.finished_at.as_deref().and_then(|s| {
            if s.starts_with("0001-") || s.is_empty() {
                None
            } else {
                chrono::DateTime::parse_from_rfc3339(s)
                    .ok()
                    .map(|dt| dt.with_timezone(&chrono::Utc))
            }
        });

        tracing::info!(
            container = %name,
            exit_code,
            reason = ?reason,
            signal = signal.as_deref().unwrap_or(""),
            oom_killed = oom,
            "container wait_outcome resolved",
        );

        Ok(WaitOutcome {
            exit_code,
            reason,
            signal,
            finished_at,
        })
    }

    /// Wait for a container to reach a specific [`WaitCondition`].
    ///
    /// Forwards the condition to bollard's `wait_container` via
    /// [`WaitContainerOptions::condition`] so Docker's native
    /// `not-running | next-exit | removed` semantics are respected, then
    /// reuses [`Runtime::wait_outcome`]'s post-wait inspect to produce a
    /// classified [`WaitOutcome`].
    #[instrument(
        skip(self),
        fields(
            otel.name = "container.wait_outcome_with_condition",
            container.id = %container_name(id),
            service.name = %id.service,
            wait.condition = %condition.as_wire_str(),
        )
    )]
    async fn wait_outcome_with_condition(
        &self,
        id: &ContainerId,
        condition: WaitCondition,
    ) -> Result<WaitOutcome> {
        let name = container_name(id);

        let options = WaitContainerOptions {
            condition: condition.as_wire_str().to_string(),
        };

        let mut stream = self.docker.wait_container(&name, Some(options));

        // bollard surfaces a non-zero exit as `DockerContainerWaitError`, which
        // carries the real exit code; `wait_exit_code_from_frame` normalizes
        // both that and the clean `Ok` (exit 0) frame into an exit code.
        let exit_code_from_wait = wait_exit_code_from_frame(stream.next().await, &name)?;

        // For `Removed`, the container is gone by definition: we cannot
        // inspect it. Emit a minimal `Exited` outcome carrying just the
        // wait-stream exit code.
        if matches!(condition, WaitCondition::Removed) {
            return Ok(WaitOutcome::exited(exit_code_from_wait));
        }

        // Otherwise reuse the same inspect-and-classify path as
        // `wait_outcome` to populate `reason`/`signal`/`finished_at`.
        let inspect = self
            .docker
            .inspect_container(&name, None)
            .await
            .map_err(|e| AgentError::NotFound {
                container: name.clone(),
                reason: format!("failed to inspect container after wait: {e}"),
            })?;

        let state = inspect.state.unwrap_or_default();

        #[allow(clippy::cast_possible_truncation)]
        let exit_code = state.exit_code.map_or(exit_code_from_wait, |c| c as i32);

        let oom = state.oom_killed.unwrap_or(false);
        let error = state.error.unwrap_or_default();
        let error_trimmed = error.trim();

        let (reason, signal) = if oom {
            (WaitReason::OomKilled, None)
        } else if let Some(sig) = signal_name_from_exit_code(exit_code) {
            (WaitReason::Signal, Some(sig))
        } else if !error_trimmed.is_empty() && exit_code == 0 {
            (WaitReason::RuntimeError, None)
        } else {
            (WaitReason::Exited, None)
        };

        let finished_at = state.finished_at.as_deref().and_then(|s| {
            if s.starts_with("0001-") || s.is_empty() {
                None
            } else {
                chrono::DateTime::parse_from_rfc3339(s)
                    .ok()
                    .map(|dt| dt.with_timezone(&chrono::Utc))
            }
        });

        Ok(WaitOutcome {
            exit_code,
            reason,
            signal,
            finished_at,
        })
    }

    /// Rename a Docker container via bollard's `POST /containers/{id}/rename`
    /// endpoint.
    ///
    /// Looks up the container by its `ContainerId`-derived name and asks
    /// the daemon to assign `new_name`. Docker enforces the name's validity
    /// and uniqueness, so any rejection (already-taken, invalid characters,
    /// container missing) surfaces as a [`BollardError`] that is mapped to
    /// [`AgentError::NotFound`] for "missing" cases and
    /// [`AgentError::Internal`] otherwise.
    #[instrument(
        skip(self),
        fields(
            otel.name = "container.rename",
            container.id = %container_name(id),
            service.name = %id.service,
            new_name = %new_name,
        )
    )]
    async fn rename_container(&self, id: &ContainerId, new_name: &str) -> Result<()> {
        // Reject empty/whitespace-only and malformed names client-side as
        // `InvalidSpec` rather than letting Docker bounce them back as an
        // opaque HTTP 400 (which maps to `AgentError::Internal`).
        let target = validate_rename_target(new_name)?;

        let current = container_name(id);

        let options = RenameContainerOptionsBuilder::default()
            .name(target)
            .build();

        self.docker
            .rename_container(&current, options)
            .await
            .map_err(|e| match &e {
                BollardError::DockerResponseServerError {
                    status_code: 404, ..
                } => AgentError::NotFound {
                    container: current.clone(),
                    reason: format!("rename target not found: {e}"),
                },
                _ => AgentError::Internal(format!("failed to rename container: {e}")),
            })?;

        tracing::info!(
            container = %current,
            new_name = %new_name,
            "renamed Docker container",
        );

        Ok(())
    }

    /// Get all container logs as a vector of lines
    #[instrument(
        skip(self),
        fields(
            otel.name = "container.get_logs",
            container.id = %container_name(id),
            service.name = %id.service,
        )
    )]
    async fn get_logs(&self, id: &ContainerId) -> Result<Vec<LogEntry>> {
        let name = container_name(id);

        let options = LogsOptions {
            stdout: true,
            stderr: true,
            tail: "all".to_string(),
            timestamps: false,
            ..Default::default()
        };

        let mut stream = self.docker.logs(&name, Some(options));
        let mut entries = Vec::new();
        let source = LogSource::Container(name.clone());

        while let Some(result) = stream.next().await {
            match result {
                Ok(log_output) => {
                    let (stream_type, text) = match &log_output {
                        bollard::container::LogOutput::StdOut { message } => {
                            (LogStream::Stdout, String::from_utf8_lossy(message))
                        }
                        bollard::container::LogOutput::StdErr { message } => {
                            (LogStream::Stderr, String::from_utf8_lossy(message))
                        }
                        _ => (LogStream::Stdout, log_output.to_string().into()),
                    };
                    for line in text.lines() {
                        entries.push(LogEntry {
                            timestamp: chrono::Utc::now(),
                            stream: stream_type,
                            message: line.to_string(),
                            source: source.clone(),
                            service: Some(id.service.clone()),
                            deployment: None,
                        });
                    }
                }
                Err(e) => {
                    return Err(AgentError::NotFound {
                        container: name.clone(),
                        reason: format!("failed to get logs: {e}"),
                    });
                }
            }
        }

        tracing::debug!(container = %name, entry_count = entries.len(), "got container logs");
        Ok(entries)
    }

    /// Get the PID of a container's main process
    #[instrument(
        skip(self),
        fields(
            otel.name = "container.get_pid",
            container.id = %container_name(id),
            service.name = %id.service,
        )
    )]
    async fn get_container_pid(&self, id: &ContainerId) -> Result<Option<u32>> {
        let name = container_name(id);

        let inspect = self
            .docker
            .inspect_container(&name, None)
            .await
            .map_err(|e| AgentError::NotFound {
                container: name.clone(),
                reason: format!("failed to inspect container: {e}"),
            })?;

        // Extract the PID from the state - only return it if the container is running
        // A PID of 0 means the container is not running
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let pid =
            inspect
                .state
                .and_then(|s| s.pid)
                .and_then(|p| if p > 0 { Some(p as u32) } else { None });

        tracing::debug!(container = %name, pid = ?pid, "got container PID");
        Ok(pid)
    }

    /// Get the IP address of a running container from Docker's bridge network
    #[instrument(
        skip(self),
        fields(
            otel.name = "container.get_ip",
            container.id = %container_name(id),
            service.name = %id.service,
        )
    )]
    async fn get_container_ip(&self, id: &ContainerId) -> Result<Option<std::net::IpAddr>> {
        let name = container_name(id);

        let inspect = self
            .docker
            .inspect_container(&name, None)
            .await
            .map_err(|e| AgentError::NotFound {
                container: name.clone(),
                reason: format!("failed to inspect container: {e}"),
            })?;

        // Extract bridge IP from network settings
        let ip = inspect
            .network_settings
            .and_then(|ns| ns.networks)
            .and_then(|nets| nets.get("bridge").cloned())
            .and_then(|bridge| bridge.ip_address)
            .filter(|ip| !ip.is_empty())
            .and_then(|ip| ip.parse::<std::net::IpAddr>().ok());

        tracing::debug!(container = %name, ip = ?ip, "got container IP from Docker inspect");
        Ok(ip)
    }

    #[instrument(skip(self), fields(otel.name = "image.list"))]
    async fn list_images(&self) -> Result<Vec<ImageInfo>> {
        use bollard::query_parameters::ListImagesOptionsBuilder;
        let options = ListImagesOptionsBuilder::default().all(true).build();
        let summaries = self
            .docker
            .list_images(Some(options))
            .await
            .map_err(|e| AgentError::Internal(format!("failed to list images: {e}")))?;

        let mut infos = Vec::with_capacity(summaries.len());
        for summary in summaries {
            // Pick the first non-placeholder repo tag as the canonical reference.
            let reference = summary
                .repo_tags
                .iter()
                .find(|t| !t.is_empty() && t.as_str() != "<none>:<none>")
                .cloned()
                .unwrap_or_else(|| summary.id.clone());

            // First repo digest as the content-addressed digest, if any.
            let digest = summary
                .repo_digests
                .iter()
                .find_map(|rd| rd.split_once('@').map(|(_, d)| d.to_string()));

            let size_bytes = u64::try_from(summary.size).ok();

            infos.push(ImageInfo {
                reference,
                digest,
                size_bytes,
            });
        }
        Ok(infos)
    }

    #[instrument(skip(self), fields(otel.name = "image.remove", container.image.name = %image, force))]
    async fn remove_image(&self, image: &str, force: bool) -> Result<()> {
        use bollard::query_parameters::RemoveImageOptionsBuilder;
        let options = RemoveImageOptionsBuilder::default().force(force).build();
        self.docker
            .remove_image(image, Some(options), None)
            .await
            .map_err(|e| AgentError::Internal(format!("failed to remove image '{image}': {e}")))?;
        Ok(())
    }

    #[instrument(skip(self), fields(otel.name = "image.prune"))]
    async fn prune_images(&self) -> Result<PruneResult> {
        let response = self
            .docker
            .prune_images(None::<bollard::query_parameters::PruneImagesOptions>)
            .await
            .map_err(|e| AgentError::Internal(format!("failed to prune images: {e}")))?;

        let deleted: Vec<String> = response
            .images_deleted
            .into_iter()
            .flatten()
            .filter_map(|item| item.deleted.or(item.untagged))
            .collect();

        let space_reclaimed = response
            .space_reclaimed
            .and_then(|v| u64::try_from(v).ok())
            .unwrap_or(0);

        Ok(PruneResult {
            deleted,
            space_reclaimed,
        })
    }

    #[instrument(
        skip(self),
        fields(
            otel.name = "container.kill",
            container.id = %container_name(id),
            service.name = %id.service,
            signal = ?signal,
        )
    )]
    async fn kill_container(&self, id: &ContainerId, signal: Option<&str>) -> Result<()> {
        use bollard::query_parameters::KillContainerOptionsBuilder;
        let canonical = crate::runtime::validate_signal(signal.unwrap_or("SIGKILL"))?;
        let name = container_name(id);

        tracing::info!(container = %name, signal = %canonical, "killing container");

        let options = KillContainerOptionsBuilder::default()
            .signal(&canonical)
            .build();

        self.docker
            .kill_container(&name, Some(options))
            .await
            .map_err(|e| match e {
                BollardError::DockerResponseServerError {
                    status_code: 404, ..
                } => AgentError::NotFound {
                    container: name.clone(),
                    reason: format!("container '{name}' not found"),
                },
                other => AgentError::Internal(format!(
                    "failed to send {canonical} to container '{name}': {other}"
                )),
            })?;

        Ok(())
    }

    #[instrument(
        skip(self),
        fields(
            otel.name = "container.pause",
            container.id = %container_name(id),
            service.name = %id.service,
        )
    )]
    async fn pause_container(&self, id: &ContainerId) -> Result<()> {
        let name = container_name(id);
        self.docker
            .pause_container(&name)
            .await
            .map_err(|e| match e {
                BollardError::DockerResponseServerError {
                    status_code: 404, ..
                } => AgentError::NotFound {
                    container: name.clone(),
                    reason: format!("container '{name}' not found"),
                },
                other => {
                    AgentError::Internal(format!("failed to pause container '{name}': {other}"))
                }
            })?;
        Ok(())
    }

    #[instrument(
        skip(self),
        fields(
            otel.name = "container.unpause",
            container.id = %container_name(id),
            service.name = %id.service,
        )
    )]
    async fn unpause_container(&self, id: &ContainerId) -> Result<()> {
        let name = container_name(id);
        self.docker
            .unpause_container(&name)
            .await
            .map_err(|e| match e {
                BollardError::DockerResponseServerError {
                    status_code: 404, ..
                } => AgentError::NotFound {
                    container: name.clone(),
                    reason: format!("container '{name}' not found"),
                },
                other => {
                    AgentError::Internal(format!("failed to unpause container '{name}': {other}"))
                }
            })?;
        Ok(())
    }

    /// Update a container's runtime resource limits and restart policy.
    ///
    /// Forwards to bollard's `update_container` with a populated
    /// [`ContainerUpdateBody`]. Empty updates short-circuit before the
    /// daemon roundtrip — Docker still accepts them, but skipping the
    /// network call keeps idle paths cheap and avoids spurious 200s
    /// in the access log.
    ///
    /// Errors map the same way as the other lifecycle endpoints: a
    /// 404 response from the Docker daemon turns into
    /// [`AgentError::NotFound`], everything else becomes
    /// [`AgentError::Internal`] with the original message preserved.
    /// Docker doesn't return any warnings on update success, so the
    /// returned [`ContainerUpdateOutcome`] always has an empty
    /// `warnings` vector.
    #[instrument(
        skip(self, update),
        fields(
            otel.name = "container.update",
            container.id = %container_name(id),
            service.name = %id.service,
        )
    )]
    async fn update_container_resources(
        &self,
        id: &ContainerId,
        update: &ContainerResourceUpdate,
    ) -> Result<ContainerUpdateOutcome> {
        let name = container_name(id);
        if update.is_empty() {
            return Ok(ContainerUpdateOutcome::default());
        }

        let restart_policy = update.restart_policy.as_ref().map(|rp| {
            let name_enum = rp.name.as_deref().and_then(|s| match s {
                "" => Some(RestartPolicyNameEnum::EMPTY),
                "no" => Some(RestartPolicyNameEnum::NO),
                "always" => Some(RestartPolicyNameEnum::ALWAYS),
                "unless-stopped" => Some(RestartPolicyNameEnum::UNLESS_STOPPED),
                "on-failure" => Some(RestartPolicyNameEnum::ON_FAILURE),
                _ => None,
            });
            RestartPolicy {
                name: name_enum,
                maximum_retry_count: rp.maximum_retry_count,
            }
        });

        let body = ContainerUpdateBody {
            cpu_shares: update.cpu_shares,
            memory: update.memory,
            cpu_period: update.cpu_period,
            cpu_quota: update.cpu_quota,
            cpu_realtime_period: update.cpu_realtime_period,
            cpu_realtime_runtime: update.cpu_realtime_runtime,
            cpuset_cpus: update.cpuset_cpus.clone(),
            cpuset_mems: update.cpuset_mems.clone(),
            memory_reservation: update.memory_reservation,
            memory_swap: update.memory_swap,
            blkio_weight: update.blkio_weight,
            pids_limit: update.pids_limit,
            restart_policy,
            ..Default::default()
        };

        self.docker
            .update_container(&name, body)
            .await
            .map_err(|e| match e {
                BollardError::DockerResponseServerError {
                    status_code: 404, ..
                } => AgentError::NotFound {
                    container: name.clone(),
                    reason: format!("container '{name}' not found"),
                },
                BollardError::DockerResponseServerError {
                    status_code: 400,
                    message,
                } => AgentError::InvalidSpec(message),
                other => {
                    AgentError::Internal(format!("failed to update container '{name}': {other}"))
                }
            })?;

        // Surface a single advisory warning when the caller passed
        // `kernel_memory` — bollard's `ContainerUpdateBody` doesn't
        // expose a kernel-memory field anymore (Docker deprecated it
        // upstream), so we accept it on the wire but never forward it.
        let mut warnings = Vec::new();
        if update.kernel_memory.is_some() {
            warnings.push(
                "KernelMemory is deprecated upstream and was not forwarded to the daemon".into(),
            );
        }
        Ok(ContainerUpdateOutcome { warnings })
    }

    #[instrument(
        skip(self),
        fields(
            otel.name = "container.top",
            container.id = %container_name(id),
            service.name = %id.service,
        )
    )]
    async fn top_container(
        &self,
        id: &ContainerId,
        ps_args: &[String],
    ) -> Result<crate::runtime::ContainerTopOutput> {
        use bollard::query_parameters::TopOptionsBuilder;
        let name = container_name(id);
        let options = if ps_args.is_empty() {
            None
        } else {
            // Bollard accepts a single `ps_args` string; join the user's argv
            // with spaces (Docker's daemon does the same on the wire).
            Some(TopOptionsBuilder::new().ps_args(&ps_args.join(" ")).build())
        };
        let response = self
            .docker
            .top_processes(&name, options)
            .await
            .map_err(|e| match e {
                BollardError::DockerResponseServerError {
                    status_code: 404, ..
                } => AgentError::NotFound {
                    container: name.clone(),
                    reason: format!("container '{name}' not found"),
                },
                other => AgentError::Internal(format!("failed to top container '{name}': {other}")),
            })?;
        Ok(crate::runtime::ContainerTopOutput {
            titles: response.titles.unwrap_or_default(),
            processes: response.processes.unwrap_or_default(),
        })
    }

    #[instrument(
        skip(self),
        fields(
            otel.name = "container.changes",
            container.id = %container_name(id),
            service.name = %id.service,
        )
    )]
    async fn changes_container(
        &self,
        id: &ContainerId,
    ) -> Result<Vec<crate::runtime::FilesystemChangeEntry>> {
        use crate::runtime::{FilesystemChangeEntry, FilesystemChangeKind};
        use bollard::models::ChangeType;
        let name = container_name(id);
        let changes = self
            .docker
            .container_changes(&name)
            .await
            .map_err(|e| match e {
                BollardError::DockerResponseServerError {
                    status_code: 404, ..
                } => AgentError::NotFound {
                    container: name.clone(),
                    reason: format!("container '{name}' not found"),
                },
                other => AgentError::Internal(format!(
                    "failed to fetch changes for container '{name}': {other}"
                )),
            })?
            .unwrap_or_default();
        Ok(changes
            .into_iter()
            .map(|change| {
                // Bollard exposes `ChangeType` as a `#[repr(i32)]` enum with
                // `_0`, `_1`, `_2` variants matching Docker's wire integers.
                let kind = match change.kind {
                    ChangeType::_0 => FilesystemChangeKind::Modified,
                    ChangeType::_1 => FilesystemChangeKind::Added,
                    ChangeType::_2 => FilesystemChangeKind::Deleted,
                };
                FilesystemChangeEntry {
                    path: change.path,
                    kind,
                }
            })
            .collect())
    }

    #[instrument(
        skip(self),
        fields(
            otel.name = "container.port",
            container.id = %container_name(id),
            service.name = %id.service,
        )
    )]
    async fn port_mappings_container(
        &self,
        id: &ContainerId,
    ) -> Result<Vec<crate::runtime::PortMappingEntry>> {
        use crate::runtime::PortMappingEntry;
        let name = container_name(id);
        let inspect = self
            .docker
            .inspect_container(&name, None)
            .await
            .map_err(|e| match e {
                BollardError::DockerResponseServerError {
                    status_code: 404, ..
                } => AgentError::NotFound {
                    container: name.clone(),
                    reason: format!("container '{name}' not found"),
                },
                other => {
                    AgentError::Internal(format!("failed to inspect container '{name}': {other}"))
                }
            })?;

        let mut out = Vec::new();
        let Some(port_map) = inspect
            .network_settings
            .as_ref()
            .and_then(|ns| ns.ports.as_ref())
        else {
            return Ok(out);
        };
        for (key, maybe_bindings) in port_map {
            let Some((container_port, protocol)) = parse_port_key(key) else {
                continue;
            };
            let proto_str = protocol.as_str().to_string();
            match maybe_bindings {
                Some(bindings) if !bindings.is_empty() => {
                    for binding in bindings {
                        let host_ip = binding.host_ip.as_ref().filter(|s| !s.is_empty()).cloned();
                        let host_port = binding
                            .host_port
                            .as_deref()
                            .and_then(|s| s.parse::<u16>().ok());
                        out.push(PortMappingEntry {
                            container_port,
                            protocol: proto_str.clone(),
                            host_ip,
                            host_port,
                        });
                    }
                }
                _ => {
                    out.push(PortMappingEntry {
                        container_port,
                        protocol: proto_str,
                        host_ip: None,
                        host_port: None,
                    });
                }
            }
        }
        Ok(out)
    }

    #[instrument(skip(self), fields(otel.name = "container.prune"))]
    async fn prune_containers(&self) -> Result<crate::runtime::ContainerPruneResult> {
        let response = self
            .docker
            .prune_containers(None::<bollard::query_parameters::PruneContainersOptions>)
            .await
            .map_err(|e| AgentError::Internal(format!("failed to prune containers: {e}")))?;
        let deleted = response.containers_deleted.unwrap_or_default();
        let space_reclaimed = response
            .space_reclaimed
            .and_then(|v| u64::try_from(v).ok())
            .unwrap_or(0);
        Ok(crate::runtime::ContainerPruneResult {
            deleted,
            space_reclaimed,
        })
    }

    #[instrument(
        skip(self),
        fields(
            otel.name = "image.tag",
            source = %source,
            target = %target,
        )
    )]
    async fn tag_image(&self, source: &str, target: &str) -> Result<()> {
        use bollard::query_parameters::TagImageOptionsBuilder;
        if source.trim().is_empty() || target.trim().is_empty() {
            return Err(AgentError::InvalidSpec(
                "source and target must be non-empty image references".to_string(),
            ));
        }
        // Split target into repo + tag (defaulting tag to "latest"). Docker's
        // `POST /images/{name}/tag` takes them as separate query parameters.
        let (repo, tag) = match target.rsplit_once(':') {
            Some((r, t)) if !r.is_empty() && !t.is_empty() && !t.contains('/') => {
                (r.to_string(), t.to_string())
            }
            _ => (target.to_string(), "latest".to_string()),
        };

        let options = TagImageOptionsBuilder::default()
            .repo(&repo)
            .tag(&tag)
            .build();

        self.docker
            .tag_image(source, Some(options))
            .await
            .map_err(|e| match e {
                BollardError::DockerResponseServerError {
                    status_code: 404, ..
                } => AgentError::NotFound {
                    container: source.to_string(),
                    reason: format!("source image '{source}' not found"),
                },
                other => AgentError::Internal(format!(
                    "failed to tag image '{source}' -> '{target}': {other}"
                )),
            })?;

        tracing::info!(source = %source, target = %target, "tagged image");
        Ok(())
    }

    /// Rich inspect: translates a single bollard `inspect_container` response
    /// into the runtime-level [`ContainerInspectDetails`].
    ///
    /// Reads:
    /// - `state.exit_code` (most recent exit, if any)
    /// - `state.health` (Docker-native healthcheck status)
    /// - `network_settings.ports` (container→host port bindings)
    /// - `network_settings.networks` (per-network aliases + IPv4)
    ///
    /// Pure translation — no fallback between fields. Missing / empty fields
    /// on the bollard side map to `None` / empty Vec on our side, which the
    /// API layer serialises away via `skip_serializing_if`.
    #[instrument(
        skip(self),
        fields(
            otel.name = "container.inspect_detailed",
            container.id = %container_name(id),
            service.name = %id.service,
        )
    )]
    async fn inspect_detailed(&self, id: &ContainerId) -> Result<ContainerInspectDetails> {
        let name = container_name(id);
        let inspect = self
            .docker
            .inspect_container(&name, None)
            .await
            .map_err(|e| AgentError::NotFound {
                container: name.clone(),
                reason: format!("failed to inspect container: {e}"),
            })?;

        Ok(translate_inspect_details(&inspect))
    }

    /// Stream container logs as `LogChunk`s, mirroring `GET /containers/{id}/logs`.
    ///
    /// Translates `LogsStreamOptions` into bollard's `LogsOptions`, opens the
    /// bollard log stream, and pumps each `LogOutput` frame into an mpsc
    /// channel as a `LogChunk` tagged with the appropriate `LogChannel`. The
    /// returned stream is `'static` so callers can hold it past the trait
    /// method's borrow of `self`.
    #[instrument(
        skip(self, opts),
        fields(
            otel.name = "container.logs_stream",
            container.id = %container_name(id),
            service.name = %id.service,
            follow = opts.follow,
            tail = ?opts.tail,
            since = ?opts.since,
            until = ?opts.until,
            timestamps = opts.timestamps,
            stdout = opts.stdout,
            stderr = opts.stderr,
        )
    )]
    async fn logs_stream(&self, id: &ContainerId, opts: LogsStreamOptions) -> Result<LogsStream> {
        let name = container_name(id);
        let bollard_opts = build_logs_options(&opts);
        let want_timestamps = opts.timestamps;

        let (tx, rx) = tokio::sync::mpsc::channel::<Result<LogChunk>>(256);
        let docker = self.docker.clone();
        let container_name_for_task = name.clone();

        tokio::spawn(async move {
            let mut stream = docker.logs(&container_name_for_task, Some(bollard_opts));
            while let Some(result) = stream.next().await {
                let send_result = match result {
                    Ok(log_output) => {
                        let chunk = log_output_to_chunk(log_output, want_timestamps);
                        tx.send(Ok(chunk)).await
                    }
                    Err(e) => {
                        let err = AgentError::NotFound {
                            container: container_name_for_task.clone(),
                            reason: format!("failed to read logs: {e}"),
                        };
                        let _ = tx.send(Err(err)).await;
                        break;
                    }
                };
                if send_result.is_err() {
                    return;
                }
            }
        });

        Ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx)))
    }

    /// Stream periodic `StatsSample` snapshots, mirroring the streaming form of
    /// `GET /containers/{id}/stats`.
    ///
    /// Drives bollard's `stats` stream (`stream: true, one_shot: false`),
    /// translates each `ContainerStatsResponse` into a `StatsSample`, and
    /// forwards it through an mpsc channel.
    #[instrument(
        skip(self),
        fields(
            otel.name = "container.stats_stream",
            container.id = %container_name(id),
            service.name = %id.service,
        )
    )]
    async fn stats_stream(&self, id: &ContainerId) -> Result<StatsStream> {
        let name = container_name(id);
        let options = StatsOptions {
            stream: true,
            one_shot: false,
        };

        let (tx, rx) = tokio::sync::mpsc::channel::<Result<StatsSample>>(64);
        let docker = self.docker.clone();
        let container_name_for_task = name.clone();

        tokio::spawn(async move {
            let mut stream = docker.stats(&container_name_for_task, Some(options));
            while let Some(result) = stream.next().await {
                let send_result = match result {
                    Ok(response) => {
                        let sample = translate_stats_sample(&response);
                        tx.send(Ok(sample)).await
                    }
                    Err(e) => {
                        let err = AgentError::NotFound {
                            container: container_name_for_task.clone(),
                            reason: format!("failed to read stats: {e}"),
                        };
                        let _ = tx.send(Err(err)).await;
                        break;
                    }
                };
                if send_result.is_err() {
                    return;
                }
            }
        });

        Ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx)))
    }

    /// Pull an image with streaming progress events, mirroring `POST /images/create`.
    ///
    /// Translates inline `RegistryAuth` into bollard's `DockerCredentials`,
    /// drives bollard's `create_image` stream, and emits one
    /// `PullProgress::Status` per `CreateImageInfo` frame followed by exactly
    /// one `PullProgress::Done` when the underlying stream closes without
    /// error. Errors mid-pull surface as `Err` items on the stream and
    /// terminate it.
    #[instrument(
        skip(self, auth),
        fields(
            otel.name = "image.pull_stream",
            container.image.name = %image,
            inline_auth = auth.is_some(),
        )
    )]
    async fn pull_image_stream(
        &self,
        image: &str,
        auth: Option<&RegistryAuth>,
    ) -> Result<PullProgressStream> {
        let (name, tag) = match ImageReference::from_str(image) {
            Ok(r) => (
                format!("{}/{}", r.registry(), r.repository()),
                r.tag().unwrap_or("latest").to_string(),
            ),
            Err(_) => (image.to_string(), "latest".to_string()),
        };

        let options = CreateImageOptions {
            from_image: Some(name.clone()),
            tag: if tag.is_empty() { None } else { Some(tag) },
            ..Default::default()
        };

        let credentials = auth.map(docker_credentials_from_registry_auth);

        let (tx, rx) = tokio::sync::mpsc::channel::<Result<PullProgress>>(64);
        let docker = self.docker.clone();
        let image_for_task = image.to_string();

        tokio::spawn(async move {
            let mut stream = docker.create_image(Some(options), None, credentials);
            // Track the most recent digest seen on the wire so the final
            // `Done` event can carry it. Docker emits a `Digest: sha256:...`
            // status line near the end of every successful pull.
            let mut last_digest: Option<String> = None;

            while let Some(result) = stream.next().await {
                match result {
                    Ok(info) => {
                        if let Some(digest) = extract_digest_from_status(info.status.as_deref()) {
                            last_digest = Some(digest);
                        }
                        let progress = translate_pull_progress(info);
                        if tx.send(Ok(progress)).await.is_err() {
                            return;
                        }
                    }
                    Err(e) => {
                        let err = AgentError::PullFailed {
                            image: image_for_task.clone(),
                            reason: e.to_string(),
                        };
                        let _ = tx.send(Err(err)).await;
                        return;
                    }
                }
            }

            // Stream closed without error — emit the final Done event.
            let _ = tx
                .send(Ok(PullProgress::Done {
                    reference: image_for_task.clone(),
                    digest: last_digest,
                }))
                .await;
        });

        Ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx)))
    }

    /// Stream a TAR archive of `path` inside the container.
    ///
    /// Forwards to bollard's `download_from_container`, which returns a
    /// chunked stream of `application/x-tar` bytes. Errors emitted by
    /// bollard are translated into [`AgentError`] and surfaced on the
    /// returned stream so callers see them mid-stream.
    #[instrument(
        skip(self),
        fields(
            otel.name = "container.archive_get",
            container.id = %container_name(id),
            service.name = %id.service,
            archive.path = %path,
        )
    )]
    async fn archive_get(&self, id: &ContainerId, path: &str) -> Result<ArchiveStream> {
        use bollard::query_parameters::DownloadFromContainerOptionsBuilder;
        let name = container_name(id);
        let options = DownloadFromContainerOptionsBuilder::default()
            .path(path)
            .build();
        // Probe with HEAD first so we surface a clean `NotFound` for missing
        // paths instead of letting the error appear mid-stream.
        let _ = self.archive_head(id, path).await?;
        let stream = self.docker.download_from_container(&name, Some(options));
        let mapped = stream.map(|item| {
            item.map_err(|e| AgentError::Internal(format!("archive_get stream error: {e}")))
        });
        Ok(Box::pin(mapped))
    }

    /// Extract a TAR archive into the container at `path`.
    #[instrument(
        skip(self, tar_bytes),
        fields(
            otel.name = "container.archive_put",
            container.id = %container_name(id),
            service.name = %id.service,
            archive.path = %path,
            archive.bytes = tar_bytes.len(),
        )
    )]
    async fn archive_put(
        &self,
        id: &ContainerId,
        path: &str,
        tar_bytes: bytes::Bytes,
        opts: ArchivePutOptions,
    ) -> Result<()> {
        use bollard::query_parameters::UploadToContainerOptionsBuilder;
        let name = container_name(id);
        let options = UploadToContainerOptionsBuilder::default()
            .path(path)
            .no_overwrite_dir_non_dir(if opts.no_overwrite_dir_non_dir {
                "1"
            } else {
                "0"
            })
            .copy_uidgid(if opts.copy_uid_gid { "1" } else { "0" })
            .build();
        self.docker
            .upload_to_container(&name, Some(options), bollard::body_full(tar_bytes))
            .await
            .map_err(|e| match e {
                BollardError::DockerResponseServerError {
                    status_code: 404, ..
                } => AgentError::NotFound {
                    container: name.clone(),
                    reason: format!("container '{name}' or path '{path}' not found"),
                },
                BollardError::DockerResponseServerError {
                    status_code: 400 | 403,
                    message,
                } => AgentError::InvalidSpec(message),
                other => AgentError::Internal(format!(
                    "failed to upload archive to container '{name}': {other}"
                )),
            })?;
        Ok(())
    }

    /// Return path-stat metadata for `path` inside the container.
    #[instrument(
        skip(self),
        fields(
            otel.name = "container.archive_head",
            container.id = %container_name(id),
            service.name = %id.service,
            archive.path = %path,
        )
    )]
    async fn archive_head(&self, id: &ContainerId, path: &str) -> Result<PathStat> {
        use bollard::query_parameters::ContainerArchiveInfoOptionsBuilder;
        let name = container_name(id);
        let options = ContainerArchiveInfoOptionsBuilder::default()
            .path(path)
            .build();
        let stat = self
            .docker
            .get_container_archive_info(&name, Some(options))
            .await
            .map_err(|e| match e {
                BollardError::DockerResponseServerError {
                    status_code: 404, ..
                } => AgentError::NotFound {
                    container: name.clone(),
                    reason: format!("path '{path}' not found in container '{name}'"),
                },
                other => AgentError::Internal(format!(
                    "failed to stat path '{path}' in container '{name}': {other}"
                )),
            })?;
        Ok(PathStat {
            name: stat.name,
            size: stat.size,
            mode: stat.file_mode,
            mtime: stat.modification_time,
            link_target: stat.link_target,
        })
    }

    #[instrument(skip(self), fields(otel.name = "image.inspect", container.image.name = %image))]
    async fn inspect_image_native(&self, image: &str) -> Result<ImageInspectInfo> {
        let inspect = self
            .docker
            .inspect_image(image)
            .await
            .map_err(|e| match e {
                BollardError::DockerResponseServerError {
                    status_code: 404, ..
                } => AgentError::NotFound {
                    container: image.to_string(),
                    reason: format!("image '{image}' not found"),
                },
                other => {
                    AgentError::Internal(format!("failed to inspect image '{image}': {other}"))
                }
            })?;
        Ok(translate_image_inspect(inspect))
    }

    #[instrument(skip(self), fields(otel.name = "image.history", container.image.name = %image))]
    async fn image_history(&self, image: &str) -> Result<Vec<ImageHistoryEntry>> {
        let history = self
            .docker
            .image_history(image)
            .await
            .map_err(|e| match e {
                BollardError::DockerResponseServerError {
                    status_code: 404, ..
                } => AgentError::NotFound {
                    container: image.to_string(),
                    reason: format!("image '{image}' not found"),
                },
                other => AgentError::Internal(format!("failed to fetch image history: {other}")),
            })?;
        Ok(history
            .into_iter()
            .map(|item| ImageHistoryEntry {
                id: item.id,
                created: item.created,
                created_by: item.created_by,
                tags: item.tags,
                size: u64::try_from(item.size).unwrap_or(0),
                comment: item.comment,
            })
            .collect())
    }

    #[instrument(skip(self), fields(otel.name = "image.search", search.term = %term, search.limit = %limit))]
    async fn search_images(&self, term: &str, limit: u32) -> Result<Vec<ImageSearchResult>> {
        use bollard::query_parameters::SearchImagesOptionsBuilder;
        if term.trim().is_empty() {
            return Err(AgentError::InvalidSpec(
                "search term must not be empty".to_string(),
            ));
        }
        let mut builder = SearchImagesOptionsBuilder::default().term(term);
        if limit > 0 {
            builder = builder.limit(i32::try_from(limit).unwrap_or(i32::MAX));
        }
        let options = builder.build();
        let results = self.docker.search_images(options).await.map_err(|e| {
            AgentError::Internal(format!("failed to search images for '{term}': {e}"))
        })?;
        Ok(results
            .into_iter()
            .map(|item| ImageSearchResult {
                name: item.name.unwrap_or_default(),
                description: item.description.unwrap_or_default(),
                star_count: u64::try_from(item.star_count.unwrap_or(0)).unwrap_or(0),
                official: item.is_official.unwrap_or(false),
                automated: item.is_automated.unwrap_or(false),
            })
            .collect())
    }

    #[instrument(skip(self, names), fields(otel.name = "image.save", count = names.len()))]
    async fn save_images(&self, names: &[String]) -> Result<ImageExportStream> {
        if names.is_empty() {
            return Err(AgentError::InvalidSpec(
                "save_images requires at least one image reference".to_string(),
            ));
        }
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        // bollard's `export_images` borrows the slice for the lifetime of the
        // stream, so collect it eagerly here. The stream itself is `'static`
        // — bollard returns chunks via process_into_body which is owned.
        let stream = self.docker.export_images(&refs);
        let mapped = stream.map(|res| {
            res.map_err(|e| AgentError::Internal(format!("image export stream error: {e}")))
        });
        Ok(Box::pin(mapped))
    }

    #[instrument(skip(self, tar_bytes), fields(otel.name = "image.load", quiet = %quiet, bytes = tar_bytes.len()))]
    async fn load_images(
        &self,
        tar_bytes: bytes::Bytes,
        quiet: bool,
    ) -> Result<LoadProgressStream> {
        use bollard::query_parameters::ImportImageOptionsBuilder;
        let options = ImportImageOptionsBuilder::default().quiet(quiet).build();
        let body = bollard::body_full(tar_bytes);
        let stream = self.docker.import_image(options, body, None);
        let mapped = stream.map(|res| {
            res.map_err(|e| AgentError::Internal(format!("image load stream error: {e}")))
                .map(|info| {
                    if let Some(stream_msg) = info.stream {
                        // Docker's load endpoint emits "Loaded image: foo:latest\n"
                        // lines. The terminal `Done` is synthesised by the API
                        // handler from the cumulative loaded references; here
                        // we surface every line as a Status event.
                        let trimmed = stream_msg.trim();
                        LoadProgress::Status {
                            id: None,
                            status: trimmed.to_string(),
                        }
                    } else if let Some(status_msg) = info.status {
                        LoadProgress::Status {
                            id: info.id,
                            status: status_msg,
                        }
                    } else {
                        LoadProgress::Status {
                            id: info.id,
                            status: String::new(),
                        }
                    }
                })
        });
        Ok(Box::pin(mapped))
    }

    #[instrument(skip(self, tar_bytes), fields(otel.name = "image.import", repo = ?repo, tag = ?tag, bytes = tar_bytes.len()))]
    async fn import_image(
        &self,
        tar_bytes: bytes::Bytes,
        repo: Option<&str>,
        tag: Option<&str>,
    ) -> Result<String> {
        // Docker's `POST /images/create?fromSrc=-` import path is exposed
        // by bollard via `create_image` with a body. We use `import_image`
        // for the load-style entrypoint and post-tag the result. The
        // simpler bollard surface is `commit_container`-style: build the
        // image via `import_image` (which actually targets `/images/load`)
        // and let Docker auto-tag. For a dedicated import we instead use
        // bollard's `import_image` and parse the stream for the resulting
        // image id, then optionally tag it.
        use bollard::query_parameters::ImportImageOptionsBuilder;
        let options = ImportImageOptionsBuilder::default().build();
        let body = bollard::body_full(tar_bytes);
        let mut stream = self.docker.import_image(options, body, None);
        let mut last_id: Option<String> = None;
        while let Some(item) = stream.next().await {
            let info =
                item.map_err(|e| AgentError::Internal(format!("image import stream error: {e}")))?;
            if let Some(id) = info.id {
                last_id = Some(id);
            } else if let Some(stream_msg) = info.stream {
                // Older daemons report "Loaded image ID: sha256:..." on
                // stream lines — extract the digest if so.
                if let Some(digest) = extract_loaded_id(&stream_msg) {
                    last_id = Some(digest);
                }
            }
        }
        let id = last_id.ok_or_else(|| {
            AgentError::Internal("import_image stream produced no image id".to_string())
        })?;
        if let Some(repo) = repo.filter(|s| !s.trim().is_empty()) {
            let target = match tag.filter(|s| !s.trim().is_empty()) {
                Some(t) => format!("{repo}:{t}"),
                None => format!("{repo}:latest"),
            };
            self.tag_image(&id, &target).await?;
        }
        Ok(id)
    }

    #[instrument(
        skip(self),
        fields(
            otel.name = "container.export",
            container.id = %container_name(id),
            service.name = %id.service,
        )
    )]
    async fn export_container_fs(&self, id: &ContainerId) -> Result<ImageExportStream> {
        let name = container_name(id);
        let stream = self.docker.export_container(&name);
        let mapped = stream.map(|res| {
            res.map_err(|e| AgentError::Internal(format!("container export stream error: {e}")))
        });
        Ok(Box::pin(mapped))
    }

    #[instrument(
        skip(self, opts),
        fields(
            otel.name = "container.commit",
            container.id = %container_name(id),
            service.name = %id.service,
        )
    )]
    async fn commit_container(
        &self,
        id: &ContainerId,
        opts: &CommitOptions,
    ) -> Result<CommitOutcome> {
        use bollard::query_parameters::CommitContainerOptionsBuilder;
        let name = container_name(id);
        let mut builder = CommitContainerOptionsBuilder::default()
            .container(&name)
            .pause(opts.pause);
        if let Some(repo) = opts.repo.as_deref().filter(|s| !s.is_empty()) {
            builder = builder.repo(repo);
        }
        if let Some(tag) = opts.tag.as_deref().filter(|s| !s.is_empty()) {
            builder = builder.tag(tag);
        }
        if let Some(comment) = opts.comment.as_deref().filter(|s| !s.is_empty()) {
            builder = builder.comment(comment);
        }
        if let Some(author) = opts.author.as_deref().filter(|s| !s.is_empty()) {
            builder = builder.author(author);
        }
        if let Some(changes) = opts.changes.as_deref().filter(|s| !s.is_empty()) {
            builder = builder.changes(changes);
        }
        let options = builder.build();
        let config = bollard::models::ContainerConfig::default();
        let resp = self
            .docker
            .commit_container(options, config)
            .await
            .map_err(|e| match e {
                BollardError::DockerResponseServerError {
                    status_code: 404, ..
                } => AgentError::NotFound {
                    container: name.clone(),
                    reason: format!("container '{name}' not found"),
                },
                other => {
                    AgentError::Internal(format!("failed to commit container '{name}': {other}"))
                }
            })?;
        Ok(CommitOutcome { id: resp.id })
    }
}

/// Parse a bollard port key like `"80/tcp"` or `"53/udp"` into
/// `(container_port, protocol)`. Returns `None` when the key doesn't match
/// the expected shape — the API handler skips unparseable entries.
fn parse_port_key(key: &str) -> Option<(u16, zlayer_spec::PortProtocol)> {
    let (port_str, proto_str) = key.split_once('/')?;
    let container_port: u16 = port_str.parse().ok()?;
    let protocol = match proto_str {
        "tcp" => zlayer_spec::PortProtocol::Tcp,
        "udp" => zlayer_spec::PortProtocol::Udp,
        _ => return None,
    };
    Some((container_port, protocol))
}

/// Translate a bollard `ContainerInspectResponse` into our
/// runtime-level [`ContainerInspectDetails`].
///
/// Pulled out as a free function so it can be unit-tested without a live
/// Docker daemon (see `translate_inspect_details_*` tests below).
pub(crate) fn translate_inspect_details(
    inspect: &bollard::models::ContainerInspectResponse,
) -> ContainerInspectDetails {
    // Ports: invert NetworkSettings.Ports (keyed by "80/tcp") into PortMapping.
    let ports = inspect
        .network_settings
        .as_ref()
        .and_then(|ns| ns.ports.as_ref())
        .map(|port_map| {
            let mut out = Vec::with_capacity(port_map.len());
            for (key, maybe_bindings) in port_map {
                let Some((container_port, protocol)) = parse_port_key(key) else {
                    continue;
                };
                // A key with `None` bindings means "exposed but not
                // published" — emit a single no-host-port mapping so the
                // consumer can still see the exposed port.
                let Some(bindings) = maybe_bindings else {
                    out.push(zlayer_spec::PortMapping {
                        host_port: None,
                        container_port,
                        protocol,
                        host_ip: String::new(),
                    });
                    continue;
                };
                for binding in bindings {
                    let host_port = binding.host_port.as_deref().and_then(|s| s.parse().ok());
                    let host_ip = binding.host_ip.clone().unwrap_or_default();
                    out.push(zlayer_spec::PortMapping {
                        host_port,
                        container_port,
                        protocol,
                        host_ip,
                    });
                }
            }
            out
        })
        .unwrap_or_default();

    // Networks: translate NetworkSettings.Networks into NetworkAttachmentDetail.
    let networks = inspect
        .network_settings
        .as_ref()
        .and_then(|ns| ns.networks.as_ref())
        .map(|nets| {
            nets.iter()
                .map(|(name, endpoint)| NetworkAttachmentDetail {
                    network: name.clone(),
                    aliases: endpoint.aliases.clone().unwrap_or_default(),
                    ipv4: endpoint
                        .ip_address
                        .as_ref()
                        .filter(|s| !s.is_empty())
                        .cloned(),
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    // First non-empty IPv4 across attached networks. Prefer `bridge` when
    // present so the result matches `get_container_ip`'s behaviour; fall back
    // to any other network in arbitrary HashMap order.
    let ipv4 = networks
        .iter()
        .find(|n| n.network == "bridge" && n.ipv4.is_some())
        .and_then(|n| n.ipv4.clone())
        .or_else(|| networks.iter().find_map(|n| n.ipv4.clone()));

    // Health: translate bollard's HealthStatusEnum + failing_streak + last log entry.
    let health = inspect
        .state
        .as_ref()
        .and_then(|s| s.health.as_ref())
        .map(|h| {
            use bollard::models::HealthStatusEnum;
            let status = match h.status {
                Some(HealthStatusEnum::STARTING) => "starting",
                Some(HealthStatusEnum::HEALTHY) => "healthy",
                Some(HealthStatusEnum::UNHEALTHY) => "unhealthy",
                Some(HealthStatusEnum::NONE | HealthStatusEnum::EMPTY) | None => "none",
            }
            .to_string();

            let failing_streak = h.failing_streak.and_then(|n| u32::try_from(n).ok());

            let last_output = h
                .log
                .as_ref()
                .and_then(|entries| entries.last())
                .and_then(|entry| entry.output.clone())
                .filter(|s| !s.is_empty());

            HealthDetail {
                status,
                failing_streak,
                last_output,
            }
        });

    // Exit code: only surface a value when the container has actually exited.
    // Docker reports `exit_code: Some(0)` for containers that have never run,
    // so we gate on `status == EXITED || DEAD` to avoid lying about fresh
    // containers.
    let exit_code = inspect.state.as_ref().and_then(|s| {
        use bollard::models::ContainerStateStatusEnum;
        match s.status {
            Some(ContainerStateStatusEnum::EXITED | ContainerStateStatusEnum::DEAD) =>
            {
                #[allow(clippy::cast_possible_truncation)]
                s.exit_code.map(|c| c as i32)
            }
            _ => None,
        }
    });

    ContainerInspectDetails {
        ports,
        networks,
        ipv4,
        health,
        exit_code,
    }
}

/// Translate a runtime-level [`LogsStreamOptions`] into bollard's
/// [`LogsOptions`].
///
/// Mostly a 1:1 field copy. The runtime trait carries `since`/`until` as
/// `i64` Unix seconds (matching the Docker Engine API), but bollard's stub
/// types declare them as `i32` — large absolute timestamps are clamped to
/// the `i32` range so we never silently wrap-around to a negative value.
/// `tail = None` becomes the bollard sentinel `"all"`, mirroring the rest of
/// the Docker logs API.
fn build_logs_options(opts: &LogsStreamOptions) -> LogsOptions {
    LogsOptions {
        follow: opts.follow,
        stdout: opts.stdout,
        stderr: opts.stderr,
        // i64 -> i32 saturating cast: any timestamp outside the i32 range is
        // far enough in the past/future that clamping is harmless.
        since: opts
            .since
            .map_or(0, |v| i32::try_from(v).unwrap_or(i32::MAX)),
        until: opts
            .until
            .map_or(0, |v| i32::try_from(v).unwrap_or(i32::MAX)),
        timestamps: opts.timestamps,
        tail: opts
            .tail
            .map_or_else(|| "all".to_string(), |n| n.to_string()),
    }
}

/// Translate a bollard `LogOutput` into the runtime-level [`LogChunk`] shape.
///
/// The four bollard variants map onto our three [`LogChannel`]s as follows:
/// `StdOut → Stdout`, `StdErr → Stderr`, `StdIn → Stdin`,
/// `Console → Stdout` (Docker emits `Console` only for tty-attached
/// containers; treating it as stdout matches `docker logs`'s own
/// non-multiplexed view).
///
/// `want_timestamps` controls whether [`LogChunk::timestamp`] is populated.
/// The bollard frame itself doesn't carry a timestamp — Docker prepends one
/// to the log line when the request set `timestamps=true`. Parsing that
/// prefix is fragile (timezone, nanosecond precision, occasional
/// non-RFC3339 lines) so we stamp the chunk with `Utc::now()` at receive
/// time when timestamps were requested. This is consistent with the
/// existing buffered `container_logs` / `get_logs` paths.
fn log_output_to_chunk(
    log_output: bollard::container::LogOutput,
    want_timestamps: bool,
) -> LogChunk {
    let stream = match &log_output {
        bollard::container::LogOutput::StdOut { .. }
        | bollard::container::LogOutput::Console { .. } => LogChannel::Stdout,
        bollard::container::LogOutput::StdErr { .. } => LogChannel::Stderr,
        bollard::container::LogOutput::StdIn { .. } => LogChannel::Stdin,
    };
    let bytes = log_output.into_bytes();
    let timestamp = if want_timestamps {
        Some(chrono::Utc::now())
    } else {
        None
    };
    LogChunk {
        stream,
        bytes,
        timestamp,
    }
}

/// Translate a bollard [`ContainerStatsResponse`] into the runtime-level
/// [`StatsSample`] shape.
///
/// Pulled out as a free function so it can be unit-tested without a live
/// Docker daemon. Counters that the daemon doesn't report (e.g. memory
/// limit on cgroups v2 hosts that omit the field, network stats on
/// host-network containers) collapse to `0`, matching the trait's
/// "missing data is signalled separately" contract. Network and block-IO
/// counters sum across all interfaces / devices because `StatsSample`
/// carries a single per-container total.
fn translate_stats_sample(response: &ContainerStatsResponse) -> StatsSample {
    let cpu_total_ns = response
        .cpu_stats
        .as_ref()
        .and_then(|cs| cs.cpu_usage.as_ref())
        .and_then(|u| u.total_usage)
        .unwrap_or(0);

    let cpu_system_ns = response
        .cpu_stats
        .as_ref()
        .and_then(|cs| cs.system_cpu_usage)
        .unwrap_or(0);

    let online_cpus = response
        .cpu_stats
        .as_ref()
        .and_then(|cs| cs.online_cpus)
        .unwrap_or(0);

    let mem_used_bytes = response
        .memory_stats
        .as_ref()
        .and_then(|m| m.usage)
        .unwrap_or(0);

    let mem_limit_bytes = response
        .memory_stats
        .as_ref()
        .and_then(|m| m.limit)
        .unwrap_or(0);

    // Network: sum rx_bytes / tx_bytes across every interface. Distinct
    // long-form names sidestep `clippy::similar_names`'s "too close" check.
    let (net_received_bytes, net_transmitted_bytes) =
        response.networks.as_ref().map_or((0_u64, 0_u64), |nets| {
            nets.values().fold((0_u64, 0_u64), |(rx_acc, tx_acc), n| {
                (
                    rx_acc.saturating_add(n.rx_bytes.unwrap_or(0)),
                    tx_acc.saturating_add(n.tx_bytes.unwrap_or(0)),
                )
            })
        });

    // Block IO: sum bytes across `io_service_bytes_recursive` entries,
    // splitting by op name. Docker reports per-op rows like
    // `{ op: "Read", value: 12345 }` — case-insensitive match keeps us
    // compatible with both cgroups v1 ("Read"/"Write") and any future
    // capitalisation tweaks.
    let (blkio_read_bytes, blkio_write_bytes) = response
        .blkio_stats
        .as_ref()
        .and_then(|b| b.io_service_bytes_recursive.as_ref())
        .map_or((0, 0), |entries| {
            entries.iter().fold((0_u64, 0_u64), |(r, w), entry| {
                let v = entry.value.unwrap_or(0);
                match entry.op.as_deref().map(str::to_ascii_lowercase).as_deref() {
                    Some("read") => (r.saturating_add(v), w),
                    Some("write") => (r, w.saturating_add(v)),
                    _ => (r, w),
                }
            })
        });

    let pids_current = response
        .pids_stats
        .as_ref()
        .and_then(|p| p.current)
        .unwrap_or(0);

    // Docker reports `limit: 0` to mean "no limit"; collapse that to None
    // so the trait's contract ("`None` means unlimited") holds end-to-end.
    let pids_limit = response
        .pids_stats
        .as_ref()
        .and_then(|p| p.limit)
        .filter(|&l| l > 0);

    StatsSample {
        cpu_total_ns,
        cpu_system_ns,
        online_cpus,
        mem_used_bytes,
        mem_limit_bytes,
        net_rx_bytes: net_received_bytes,
        net_tx_bytes: net_transmitted_bytes,
        blkio_read_bytes,
        blkio_write_bytes,
        pids_current,
        pids_limit,
        timestamp: chrono::Utc::now(),
    }
}

/// Translate a bollard [`CreateImageInfo`] frame into a
/// [`PullProgress::Status`] event.
///
/// Bollard surfaces error frames separately by mapping
/// `error_detail.message` into `BollardError::DockerStreamError` before they
/// reach us, so this translator only sees progress / status frames.
/// `progress` is left `None` because bollard's stub doesn't model the
/// preformatted progress-bar string Docker sometimes emits — callers that
/// want a bar can render one from `current`/`total`.
fn translate_pull_progress(info: CreateImageInfo) -> PullProgress {
    let CreateImageInfo {
        id,
        status,
        progress_detail,
        ..
    } = info;
    let (current, total) = progress_detail.map_or((None, None), |d| {
        (
            d.current.and_then(|v| u64::try_from(v).ok()),
            d.total.and_then(|v| u64::try_from(v).ok()),
        )
    });
    PullProgress::Status {
        id,
        status: status.unwrap_or_default(),
        progress: None,
        current,
        total,
    }
}

/// Pluck a `sha256:...` digest out of a Docker pull status line, when
/// present.
///
/// Docker's `POST /images/create` stream includes a status line like
/// `"Digest: sha256:abc..."` once per pull, just before the final
/// `"Status: Downloaded newer image for ..."` line. Capturing it lets the
/// `Done` event we synthesise carry the resolved digest without an extra
/// round-trip to `inspect_image`.
fn extract_digest_from_status(status: Option<&str>) -> Option<String> {
    let s = status?.trim();
    let prefix = "Digest:";
    if !s.starts_with(prefix) {
        return None;
    }
    let candidate = s[prefix.len()..].trim();
    if candidate.starts_with("sha256:") || candidate.starts_with("sha512:") {
        Some(candidate.to_string())
    } else {
        None
    }
}

/// Parse a bollard `BuildInfo.stream` line emitted during `import_image`
/// looking for `"Loaded image ID: sha256:..."`. Returns the digest when
/// present, `None` otherwise. Used by [`DockerRuntime::import_image`] to
/// recover the freshly-imported image id from the pre-engine-22.10 daemons
/// that stream messages instead of populating `BuildInfo.id`.
fn extract_loaded_id(line: &str) -> Option<String> {
    let trimmed = line.trim();
    let prefix = "Loaded image ID:";
    if let Some(after) = trimmed.strip_prefix(prefix) {
        let candidate = after.trim();
        if candidate.starts_with("sha256:") {
            return Some(candidate.to_string());
        }
    }
    let alt = "Loaded image:";
    if let Some(after) = trimmed.strip_prefix(alt) {
        let candidate = after.trim();
        if !candidate.is_empty() {
            return Some(candidate.to_string());
        }
    }
    None
}

/// Convert bollard's [`bollard::models::ImageInspect`] into the runtime-level
/// [`ImageInspectInfo`] DTO. Pure translation: missing optional fields stay
/// `None` / empty so the API/Docker shim can serialise them as Docker's
/// canonical empty defaults.
fn translate_image_inspect(inspect: bollard::models::ImageInspect) -> ImageInspectInfo {
    let bollard::models::ImageInspect {
        id,
        repo_tags,
        repo_digests,
        comment,
        created,
        author,
        config,
        architecture,
        os,
        size,
        root_fs,
        ..
    } = inspect;

    let mut env = Vec::new();
    let mut cmd = Vec::new();
    let mut entrypoint = Vec::new();
    let mut working_dir = None;
    let mut user = None;
    let mut labels = std::collections::BTreeMap::new();
    if let Some(cfg) = config {
        env = cfg.env.unwrap_or_default();
        cmd = cfg.cmd.unwrap_or_default();
        entrypoint = cfg.entrypoint.unwrap_or_default();
        working_dir = cfg.working_dir.filter(|s| !s.is_empty());
        user = cfg.user.filter(|s| !s.is_empty());
        if let Some(ls) = cfg.labels {
            for (k, v) in ls {
                labels.insert(k, v);
            }
        }
    }

    let layers = root_fs
        .map(|fs| fs.layers.unwrap_or_default())
        .unwrap_or_default();

    ImageInspectInfo {
        id,
        repo_tags: repo_tags.unwrap_or_default(),
        repo_digests: repo_digests.unwrap_or_default(),
        parent: None,
        comment,
        created: created.map(|t| t.to_string()),
        container: None,
        docker_version: None,
        author,
        architecture,
        os,
        size: size.and_then(|v| u64::try_from(v).ok()),
        layers,
        env,
        cmd,
        entrypoint,
        working_dir,
        user,
        labels,
    }
}

/// Translate the runtime-facing [`ExecOptions`] into bollard's
/// [`CreateExecOptions`] payload. Field names are 1:1 with the Docker Engine
/// `ExecConfig` schema (which both shapes mirror) so this is a straight
/// re-pack — the only adjustment is wrapping non-empty optional collections /
/// strings in `Some(..)` (bollard treats `None` as "field absent" in the JSON
/// body, which is how Docker tells empty-vs-unset apart).
fn build_create_exec_options(opts: &ExecOptions) -> CreateExecOptions<String> {
    CreateExecOptions {
        cmd: if opts.command.is_empty() {
            None
        } else {
            Some(opts.command.clone())
        },
        env: if opts.env.is_empty() {
            None
        } else {
            Some(opts.env.clone())
        },
        working_dir: opts.working_dir.clone(),
        user: opts.user.clone(),
        privileged: Some(opts.privileged),
        tty: Some(opts.tty),
        attach_stdin: Some(opts.attach_stdin),
        attach_stdout: Some(opts.attach_stdout),
        attach_stderr: Some(opts.attach_stderr),
        detach_keys: None,
    }
}

/// Build a bollard [`ResizeExecOptions`] from a `(rows, cols)` pair as sent
/// over [`ExecHandle::resize`]. Docker's API uses `h` for rows and `w` for
/// cols, mirroring tty ioctls (`TIOCSWINSZ`). Kept as a free fn so it can be
/// unit-tested without a live Docker connection.
fn resize_options_for(rows: u16, cols: u16) -> ResizeExecOptions {
    ResizeExecOptions {
        height: rows,
        width: cols,
    }
}

/// Pull the exit code out of a bollard [`ExecInspectResponse`] and clamp it
/// into `i32` the way `Runtime` callers expect. Docker's stub models
/// `exit_code` as an `Option<i64>`; `None` (the exec is still running, or
/// reporting failed) collapses to `0` to match the existing buffered
/// [`Runtime::exec`] behaviour. Any out-of-range `i64` (effectively
/// impossible for a real Unix exit status, but the Engine wire-type allows
/// it) saturates to the nearest `i32` bound.
#[allow(clippy::cast_possible_truncation)]
fn extract_exec_exit_code(inspect: &ExecInspectResponse) -> i32 {
    match inspect.exit_code {
        None => 0,
        Some(code) if code > i64::from(i32::MAX) => i32::MAX,
        Some(code) if code < i64::from(i32::MIN) => i32::MIN,
        Some(code) => code as i32,
    }
}

/// Newtype that bridges bollard's split exec channels (an output `Stream`
/// and an input `AsyncWrite`) into a single duplex byte stream so the
/// `Runtime::exec_pty` contract — one [`ExecPtyStream`] for both directions —
/// can be honoured without per-caller plumbing.
///
/// On the read side we drain bollard's `LogOutput` stream a chunk at a time;
/// `current_chunk` holds whatever bytes the caller hasn't consumed yet from
/// the most recent frame. With `tty: true` Docker emits everything as
/// `LogOutput::Console`; with `tty: false` it splits stdout/stderr into the
/// `StdOut` / `StdErr` variants. We treat all four payloads identically here
/// and let the caller demultiplex if it cares — the duplex API is byte-level
/// by design (mirrors Docker's hijacked socket).
///
/// On the write side we forward `poll_write` / `poll_flush` / `poll_shutdown`
/// straight to bollard's input `AsyncWrite`, which is the upgraded HTTP
/// connection's write half.
struct ExecPtyDuplex {
    output: Pin<
        Box<
            dyn Stream<Item = std::result::Result<bollard::container::LogOutput, BollardError>>
                + Send,
        >,
    >,
    input: Pin<Box<dyn AsyncWrite + Send>>,
    /// Bytes from the most recent `LogOutput` frame that haven't been
    /// delivered to a `poll_read` caller yet. Drained left-to-right.
    current_chunk: bytes::Bytes,
    /// Once the underlying `output` stream returns `None`, no further reads
    /// will ever produce data. Latched so polling after EOF doesn't keep
    /// driving the (now-fused) stream.
    output_done: bool,
}

impl AsyncRead for ExecPtyDuplex {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        loop {
            // Fast path: hand the caller as many buffered bytes as fit.
            if !self.current_chunk.is_empty() {
                let take = self.current_chunk.len().min(buf.remaining());
                let chunk = self.current_chunk.split_to(take);
                buf.put_slice(&chunk);
                return Poll::Ready(Ok(()));
            }

            if self.output_done {
                // EOF: zero-fill is the AsyncRead contract for "no more data".
                return Poll::Ready(Ok(()));
            }

            // Pull the next frame from bollard.
            match self.output.as_mut().poll_next(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(None) => {
                    self.output_done = true;
                    return Poll::Ready(Ok(()));
                }
                Poll::Ready(Some(Ok(frame))) => {
                    self.current_chunk = frame.into_bytes();
                    // Loop around to deliver the bytes (or, if the frame was
                    // empty, to fetch the next one).
                }
                Poll::Ready(Some(Err(e))) => {
                    return Poll::Ready(Err(std::io::Error::other(e.to_string())));
                }
            }
        }
    }
}

impl AsyncWrite for ExecPtyDuplex {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        self.input.as_mut().poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        self.input.as_mut().poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        self.input.as_mut().poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_container_name() {
        let id = ContainerId::new("myservice".to_string(), 1);
        assert_eq!(container_name(&id), "zlayer-myservice-1");
    }

    #[test]
    fn validate_rename_target_rejects_empty() {
        assert!(matches!(
            validate_rename_target(""),
            Err(AgentError::InvalidSpec(_))
        ));
    }

    #[test]
    fn validate_rename_target_rejects_whitespace_only() {
        assert!(matches!(
            validate_rename_target("   "),
            Err(AgentError::InvalidSpec(_))
        ));
        assert!(matches!(
            validate_rename_target("\t\n "),
            Err(AgentError::InvalidSpec(_))
        ));
    }

    #[test]
    fn validate_rename_target_rejects_invalid_charset() {
        assert!(matches!(
            validate_rename_target("bad name!"),
            Err(AgentError::InvalidSpec(_))
        ));
        assert!(matches!(
            validate_rename_target("name/slash"),
            Err(AgentError::InvalidSpec(_))
        ));
    }

    #[test]
    fn validate_rename_target_rejects_leading_dash() {
        assert!(matches!(
            validate_rename_target("-foo"),
            Err(AgentError::InvalidSpec(_))
        ));
        assert!(matches!(
            validate_rename_target(".hidden"),
            Err(AgentError::InvalidSpec(_))
        ));
    }

    #[test]
    fn validate_rename_target_accepts_valid_names() {
        assert_eq!(validate_rename_target("web-1").unwrap(), "web-1");
        assert_eq!(validate_rename_target("a.b_c-d").unwrap(), "a.b_c-d");
        assert_eq!(validate_rename_target("x").unwrap(), "x");
        assert_eq!(validate_rename_target("7").unwrap(), "7");
        // The patch's happy-path and missing-container names must pass.
        assert_eq!(
            validate_rename_target("zlayer-zql-renamed-123").unwrap(),
            "zlayer-zql-renamed-123"
        );
        assert_eq!(
            validate_rename_target("anything-else").unwrap(),
            "anything-else"
        );
    }

    #[test]
    fn validate_rename_target_trims_surrounding_whitespace() {
        assert_eq!(validate_rename_target("  web-1  ").unwrap(), "web-1");
    }

    #[test]
    fn wait_exit_code_zero_is_ok() {
        // Exit 0 arrives as a clean `Ok(ContainerWaitResponse)`.
        let frame = Some(Ok(ContainerWaitResponse {
            status_code: 0,
            error: None,
        }));
        assert_eq!(wait_exit_code_from_frame(frame, "zlayer-svc-0").unwrap(), 0);
    }

    #[test]
    fn wait_exit_code_nonzero_error_variant_is_ok() {
        // bollard rewrites a non-zero exit into `DockerContainerWaitError`
        // (often with an EMPTY `error` string). This is a successful wait whose
        // exit code is `code`, NOT a failure.
        let frame = Some(Err(BollardError::DockerContainerWaitError {
            error: String::new(),
            code: 42,
        }));
        assert_eq!(
            wait_exit_code_from_frame(frame, "zlayer-svc-0").unwrap(),
            42
        );
    }

    #[test]
    fn wait_exit_code_nonzero_with_message_is_ok() {
        // Even when the daemon includes an `Error.Message`, a non-zero exit is
        // still the exit code, not a transport failure.
        let frame = Some(Err(BollardError::DockerContainerWaitError {
            error: "container exited".to_string(),
            code: 137,
        }));
        assert_eq!(
            wait_exit_code_from_frame(frame, "zlayer-svc-0").unwrap(),
            137
        );
    }

    #[test]
    fn wait_exit_code_closed_stream_errors() {
        // A stream that yields no frame at all is a genuine failure.
        let err = wait_exit_code_from_frame(None, "zlayer-svc-0").unwrap_err();
        assert!(matches!(err, AgentError::NotFound { .. }));
    }

    #[test]
    fn wait_exit_code_other_bollard_error_errors() {
        // A non-wait bollard error is a real transport/daemon failure.
        let frame = Some(Err(BollardError::DockerResponseServerError {
            status_code: 404,
            message: "no such container".to_string(),
        }));
        let err = wait_exit_code_from_frame(frame, "zlayer-svc-0").unwrap_err();
        assert!(matches!(err, AgentError::NotFound { .. }));
    }

    #[test]
    fn test_container_name_with_different_replicas() {
        let id1 = ContainerId::new("api".to_string(), 0);
        let id2 = ContainerId::new("api".to_string(), 42);
        assert_eq!(container_name(&id1), "zlayer-api-0");
        assert_eq!(container_name(&id2), "zlayer-api-42");
    }

    #[test]
    fn extract_local_digest_matches_repo() {
        let inspect = ImageInspect {
            repo_digests: Some(vec!["zachhandley/zlayer-manager@sha256:abc123".to_string()]),
            ..Default::default()
        };
        assert_eq!(
            extract_local_digest(&inspect, "zachhandley/zlayer-manager"),
            Some("sha256:abc123".to_string())
        );
    }

    #[test]
    fn extract_local_digest_returns_none_for_non_matching_repo() {
        let inspect = ImageInspect {
            repo_digests: Some(vec!["otherrepo/image@sha256:def456".to_string()]),
            ..Default::default()
        };
        assert_eq!(
            extract_local_digest(&inspect, "zachhandley/zlayer-manager"),
            None
        );
    }

    #[test]
    fn extract_local_digest_returns_none_when_repo_digests_missing() {
        let inspect = ImageInspect {
            repo_digests: None,
            ..Default::default()
        };
        assert_eq!(
            extract_local_digest(&inspect, "zachhandley/zlayer-manager"),
            None
        );
    }

    #[test]
    fn extract_local_digest_skips_malformed_entries() {
        let inspect = ImageInspect {
            repo_digests: Some(vec![
                "malformed-no-at-sign".to_string(),
                "zachhandley/zlayer-manager@sha256:abc123".to_string(),
            ]),
            ..Default::default()
        };
        assert_eq!(
            extract_local_digest(&inspect, "zachhandley/zlayer-manager"),
            Some("sha256:abc123".to_string())
        );
    }

    #[test]
    fn extract_local_digest_picks_matching_when_multiple() {
        let inspect = ImageInspect {
            repo_digests: Some(vec![
                "otherrepo/image@sha256:111".to_string(),
                "zachhandley/zlayer-manager@sha256:abc123".to_string(),
                "thirdparty/image@sha256:222".to_string(),
            ]),
            ..Default::default()
        };
        assert_eq!(
            extract_local_digest(&inspect, "zachhandley/zlayer-manager"),
            Some("sha256:abc123".to_string())
        );
    }

    #[test]
    fn test_parse_memory_bytes() {
        assert_eq!(parse_memory("1024"), Some(1024));
    }

    #[test]
    fn test_parse_memory_kilobytes() {
        assert_eq!(parse_memory("1Ki"), Some(1024));
        assert_eq!(parse_memory("1K"), Some(1024));
        assert_eq!(parse_memory("1KB"), Some(1024));
    }

    #[test]
    fn test_parse_memory_megabytes() {
        assert_eq!(parse_memory("512Mi"), Some(512 * 1024 * 1024));
        assert_eq!(parse_memory("512M"), Some(512 * 1024 * 1024));
        assert_eq!(parse_memory("512MB"), Some(512 * 1024 * 1024));
    }

    #[test]
    fn test_parse_memory_gigabytes() {
        assert_eq!(parse_memory("1Gi"), Some(1024 * 1024 * 1024));
        assert_eq!(parse_memory("1G"), Some(1024 * 1024 * 1024));
        assert_eq!(parse_memory("2GB"), Some(2 * 1024 * 1024 * 1024));
    }

    #[test]
    fn test_parse_memory_with_decimals() {
        assert_eq!(parse_memory("0.5Gi"), Some(512 * 1024 * 1024));
        assert_eq!(parse_memory("1.5G"), Some(1536 * 1024 * 1024));
    }

    #[test]
    fn test_parse_memory_invalid() {
        assert_eq!(parse_memory("invalid"), None);
        assert_eq!(parse_memory("XYZ"), None);
    }

    #[test]
    fn test_build_exposed_ports_empty() {
        let spec = create_test_spec(vec![]);
        let ports = build_exposed_ports(&spec);
        assert!(ports.is_empty());
    }

    #[test]
    fn test_build_exposed_ports_single() {
        let spec = create_test_spec(vec![8080]);
        let ports = build_exposed_ports(&spec);
        assert!(ports.contains(&"8080/tcp".to_string()));
        assert_eq!(ports.len(), 1);
    }

    #[test]
    fn test_build_exposed_ports_multiple() {
        let spec = create_test_spec(vec![8080, 9090, 3000]);
        let ports = build_exposed_ports(&spec);
        assert!(ports.contains(&"8080/tcp".to_string()));
        assert!(ports.contains(&"9090/tcp".to_string()));
        assert!(ports.contains(&"3000/tcp".to_string()));
        assert_eq!(ports.len(), 3);
    }

    #[test]
    fn test_build_host_config_ports() {
        let spec = create_test_spec(vec![8080]);
        let host_config = build_host_config(&spec, None, None);

        let port_bindings = host_config.port_bindings.unwrap();
        let bindings = port_bindings.get("8080/tcp").unwrap().as_ref().unwrap();
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].host_port.as_ref().unwrap(), "8080");
        assert_eq!(bindings[0].host_ip.as_ref().unwrap(), "0.0.0.0");
    }

    #[test]
    fn test_build_host_config_privileged() {
        let mut spec = create_test_spec(vec![]);
        spec.privileged = true;
        let host_config = build_host_config(&spec, None, None);
        assert_eq!(host_config.privileged, Some(true));
    }

    /// Empty `spec.labels` must produce `None`, not an empty map. Bollard's
    /// `ContainerCreateBody.labels` is `Option<HashMap<_, _>>` and the daemon
    /// treats `None` and `{}` equivalently, but `None` keeps the wire payload
    /// minimal and matches the convention used for other optional fields in
    /// `create_container`.
    #[test]
    fn build_labels_returns_none_for_empty_map() {
        let spec = create_test_spec(vec![]);
        assert!(spec.labels.is_empty(), "test fixture starts with no labels");
        assert!(build_labels(&spec).is_none());
    }

    /// When the API handler injects `com.zlayer.container_id=<hex>` (and
    /// optionally other user labels) into `spec.labels`, `build_labels` must
    /// forward the entire map verbatim into `Config.Labels`. This is the
    /// runtime-side contract that makes task 1.4.5's reconciliation label
    /// actually land on the Docker container.
    #[test]
    fn build_labels_forwards_zlayer_container_id_label() {
        let mut spec = create_test_spec(vec![]);
        let hex = "deadbeef".repeat(8); // 64-char fake hex
        spec.labels
            .insert("com.zlayer.container_id".to_string(), hex.clone());
        spec.labels
            .insert("user-key".to_string(), "user-value".to_string());

        let labels = build_labels(&spec).expect("non-empty labels must be Some");
        assert_eq!(labels.get("com.zlayer.container_id"), Some(&hex));
        assert_eq!(
            labels.get("user-key"),
            Some(&"user-value".to_string()),
            "user-supplied labels must be preserved alongside the reserved key"
        );
        assert_eq!(labels.len(), 2, "no extra labels should appear");
    }

    /// Helper to create a minimal test `ServiceSpec`
    fn create_test_spec(ports: Vec<u16>) -> ServiceSpec {
        use zlayer_spec::*;

        let endpoints: Vec<EndpointSpec> = ports
            .into_iter()
            .enumerate()
            .map(|(i, port)| EndpointSpec {
                name: format!("endpoint{i}"),
                protocol: Protocol::Http,
                port,
                target_port: None,
                path: None,
                host: None,
                expose: ExposeType::Internal,
                stream: None,
                tunnel: None,
                target_role: None,
            })
            .collect();

        ServiceSpec {
            image: ImageSpec {
                name: "test:latest".parse().expect("valid image reference"),
                pull_policy: PullPolicy::IfNotPresent,
                source_policy: None,
            },
            endpoints,
            health: HealthSpec {
                start_grace: None,
                interval: None,
                timeout: None,
                retries: 3,
                check: HealthCheck::Tcp { port: 0 },
            },
            ..ServiceSpec::default()
        }
    }

    #[test]
    fn test_build_host_config_port_mappings_static_and_ephemeral() {
        use zlayer_spec::{PortMapping, PortProtocol};

        let mut spec = create_test_spec(vec![]);
        spec.port_mappings = vec![
            // Static TCP publish on 0.0.0.0:8080 -> container 80/tcp.
            PortMapping {
                host_port: Some(8080),
                container_port: 80,
                protocol: PortProtocol::Tcp,
                host_ip: "0.0.0.0".to_string(),
            },
            // Ephemeral UDP publish (host_port = None) on 127.0.0.1 -> 53/udp.
            PortMapping {
                host_port: None,
                container_port: 53,
                protocol: PortProtocol::Udp,
                host_ip: "127.0.0.1".to_string(),
            },
            // host_port = Some(0) should also translate to ephemeral (None).
            PortMapping {
                host_port: Some(0),
                container_port: 443,
                protocol: PortProtocol::Tcp,
                host_ip: String::new(), // empty -> defaults to "0.0.0.0"
            },
        ];

        let host_config = build_host_config(&spec, None, None);
        let port_bindings = host_config.port_bindings.expect("port_bindings set");

        let tcp80 = port_bindings
            .get("80/tcp")
            .expect("80/tcp key")
            .as_ref()
            .expect("80/tcp bindings");
        assert_eq!(tcp80.len(), 1);
        assert_eq!(tcp80[0].host_port.as_deref(), Some("8080"));
        assert_eq!(tcp80[0].host_ip.as_deref(), Some("0.0.0.0"));

        let udp53 = port_bindings
            .get("53/udp")
            .expect("53/udp key")
            .as_ref()
            .expect("53/udp bindings");
        assert_eq!(udp53.len(), 1);
        assert!(udp53[0].host_port.is_none(), "ephemeral host_port is None");
        assert_eq!(udp53[0].host_ip.as_deref(), Some("127.0.0.1"));

        let tcp443 = port_bindings
            .get("443/tcp")
            .expect("443/tcp key")
            .as_ref()
            .expect("443/tcp bindings");
        assert_eq!(tcp443.len(), 1);
        assert!(
            tcp443[0].host_port.is_none(),
            "Some(0) should become None (ephemeral)"
        );
        // Empty host_ip defaults to 0.0.0.0.
        assert_eq!(tcp443[0].host_ip.as_deref(), Some("0.0.0.0"));

        // Exposed ports should include the mapping-declared container ports.
        let exposed = build_exposed_ports(&spec);
        assert!(exposed.contains(&"80/tcp".to_string()));
        assert!(exposed.contains(&"53/udp".to_string()));
        assert!(exposed.contains(&"443/tcp".to_string()));
    }

    #[test]
    fn translate_restart_policy_covers_all_kinds() {
        use zlayer_spec::{ContainerRestartKind, ContainerRestartPolicy};

        let cases: &[(
            ContainerRestartKind,
            Option<u32>,
            RestartPolicyNameEnum,
            Option<i64>,
        )] = &[
            (
                ContainerRestartKind::No,
                None,
                RestartPolicyNameEnum::NO,
                None,
            ),
            (
                ContainerRestartKind::Always,
                None,
                RestartPolicyNameEnum::ALWAYS,
                None,
            ),
            (
                ContainerRestartKind::UnlessStopped,
                None,
                RestartPolicyNameEnum::UNLESS_STOPPED,
                None,
            ),
            // on_failure carries max_attempts through as maximum_retry_count.
            (
                ContainerRestartKind::OnFailure,
                Some(7),
                RestartPolicyNameEnum::ON_FAILURE,
                Some(7),
            ),
            // on_failure with no max => unlimited retries.
            (
                ContainerRestartKind::OnFailure,
                None,
                RestartPolicyNameEnum::ON_FAILURE,
                None,
            ),
        ];

        for (kind, max_attempts, expected_name, expected_retry) in cases {
            let policy = ContainerRestartPolicy {
                kind: *kind,
                max_attempts: *max_attempts,
                delay: None,
            };
            let translated = translate_restart_policy(&policy);
            assert_eq!(
                translated.name,
                Some(*expected_name),
                "bad name for {kind:?}"
            );
            assert_eq!(
                translated.maximum_retry_count, *expected_retry,
                "bad retry count for {kind:?}"
            );
        }
    }

    #[test]
    fn translate_restart_policy_ignores_delay_but_still_returns_policy() {
        // When `delay` is set, we warn but still emit a valid policy; the
        // `name` must match and `maximum_retry_count` must reflect
        // `max_attempts` (only for on_failure).
        use zlayer_spec::{ContainerRestartKind, ContainerRestartPolicy};

        let translated = translate_restart_policy(&ContainerRestartPolicy {
            kind: ContainerRestartKind::Always,
            max_attempts: None,
            delay: Some("500ms".to_string()),
        });
        assert_eq!(translated.name, Some(RestartPolicyNameEnum::ALWAYS));
        assert!(translated.maximum_retry_count.is_none());
    }

    #[test]
    fn build_host_config_sets_restart_policy_when_specified() {
        let mut spec = create_test_spec(vec![]);
        spec.restart_policy = Some(zlayer_spec::ContainerRestartPolicy {
            kind: zlayer_spec::ContainerRestartKind::OnFailure,
            max_attempts: Some(3),
            delay: None,
        });
        let host_config = build_host_config(&spec, None, None);
        let rp = host_config
            .restart_policy
            .expect("restart_policy should be populated");
        assert_eq!(rp.name, Some(RestartPolicyNameEnum::ON_FAILURE));
        assert_eq!(rp.maximum_retry_count, Some(3));
    }

    #[test]
    fn build_host_config_omits_restart_policy_when_none() {
        let spec = create_test_spec(vec![]);
        let host_config = build_host_config(&spec, None, None);
        assert!(
            host_config.restart_policy.is_none(),
            "no restart_policy on spec should leave the field None"
        );
    }

    // ----------------------------------------------------------------------
    // §3.15 — translate_inspect_details
    // ----------------------------------------------------------------------

    #[test]
    fn parse_port_key_accepts_tcp_and_udp() {
        use zlayer_spec::PortProtocol;
        assert_eq!(parse_port_key("80/tcp"), Some((80, PortProtocol::Tcp)));
        assert_eq!(parse_port_key("53/udp"), Some((53, PortProtocol::Udp)));
        assert_eq!(parse_port_key(""), None);
        assert_eq!(parse_port_key("80"), None);
        assert_eq!(parse_port_key("abc/tcp"), None);
        assert_eq!(parse_port_key("80/sctp"), None);
    }

    #[test]
    fn translate_inspect_details_translates_ports_and_networks() {
        use bollard::models::{
            ContainerInspectResponse, ContainerState, ContainerStateStatusEnum, EndpointSettings,
            Health, HealthStatusEnum, HealthcheckResult, NetworkSettings, PortBinding,
        };
        use std::collections::HashMap;

        // Two port mappings: 80/tcp -> host 0.0.0.0:8080, 53/udp ephemeral
        // (null host_port).
        let mut ports: HashMap<String, Option<Vec<PortBinding>>> = HashMap::new();
        ports.insert(
            "80/tcp".to_string(),
            Some(vec![PortBinding {
                host_ip: Some("0.0.0.0".to_string()),
                host_port: Some("8080".to_string()),
            }]),
        );
        ports.insert(
            "53/udp".to_string(),
            Some(vec![PortBinding {
                host_ip: Some("127.0.0.1".to_string()),
                host_port: None,
            }]),
        );

        let mut networks: HashMap<String, EndpointSettings> = HashMap::new();
        networks.insert(
            "bridge".to_string(),
            EndpointSettings {
                aliases: Some(vec!["myapp".to_string()]),
                ip_address: Some("172.17.0.2".to_string()),
                ..Default::default()
            },
        );

        let inspect = ContainerInspectResponse {
            state: Some(ContainerState {
                status: Some(ContainerStateStatusEnum::RUNNING),
                health: Some(Health {
                    status: Some(HealthStatusEnum::HEALTHY),
                    failing_streak: Some(0),
                    log: Some(vec![HealthcheckResult {
                        output: Some("OK".to_string()),
                        ..Default::default()
                    }]),
                }),
                ..Default::default()
            }),
            network_settings: Some(NetworkSettings {
                ports: Some(ports),
                networks: Some(networks),
                ..Default::default()
            }),
            ..Default::default()
        };

        let details = translate_inspect_details(&inspect);

        // Ports: both should be present.
        assert_eq!(details.ports.len(), 2);
        let tcp80 = details
            .ports
            .iter()
            .find(|p| p.container_port == 80)
            .expect("80/tcp mapping");
        assert_eq!(tcp80.host_port, Some(8080));
        assert_eq!(tcp80.host_ip, "0.0.0.0");
        assert_eq!(tcp80.protocol, zlayer_spec::PortProtocol::Tcp);

        let udp53 = details
            .ports
            .iter()
            .find(|p| p.container_port == 53)
            .expect("53/udp mapping");
        assert_eq!(udp53.host_port, None);
        assert_eq!(udp53.host_ip, "127.0.0.1");
        assert_eq!(udp53.protocol, zlayer_spec::PortProtocol::Udp);

        // Networks: one `bridge` attachment with alias + IP.
        assert_eq!(details.networks.len(), 1);
        assert_eq!(details.networks[0].network, "bridge");
        assert_eq!(details.networks[0].aliases, vec!["myapp".to_string()]);
        assert_eq!(details.networks[0].ipv4.as_deref(), Some("172.17.0.2"));

        // IPv4: first IP (from bridge).
        assert_eq!(details.ipv4.as_deref(), Some("172.17.0.2"));

        // Health: healthy.
        let health = details.health.expect("health translated");
        assert_eq!(health.status, "healthy");
        assert_eq!(health.failing_streak, Some(0));
        assert_eq!(health.last_output.as_deref(), Some("OK"));

        // Exit code: None for a running container.
        assert!(details.exit_code.is_none());
    }

    #[test]
    fn translate_inspect_details_exit_code_only_on_exited() {
        use bollard::models::{ContainerInspectResponse, ContainerState, ContainerStateStatusEnum};

        // Running: exit_code ignored.
        let running = ContainerInspectResponse {
            state: Some(ContainerState {
                status: Some(ContainerStateStatusEnum::RUNNING),
                exit_code: Some(0),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(translate_inspect_details(&running).exit_code.is_none());

        // Exited: exit_code surfaced.
        let exited = ContainerInspectResponse {
            state: Some(ContainerState {
                status: Some(ContainerStateStatusEnum::EXITED),
                exit_code: Some(137),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(translate_inspect_details(&exited).exit_code, Some(137));

        // Dead: exit_code surfaced too.
        let dead = ContainerInspectResponse {
            state: Some(ContainerState {
                status: Some(ContainerStateStatusEnum::DEAD),
                exit_code: Some(255),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(translate_inspect_details(&dead).exit_code, Some(255));
    }

    #[test]
    fn translate_inspect_details_empty_response_yields_default() {
        let inspect = bollard::models::ContainerInspectResponse::default();
        let details = translate_inspect_details(&inspect);
        assert!(details.ports.is_empty());
        assert!(details.networks.is_empty());
        assert!(details.ipv4.is_none());
        assert!(details.health.is_none());
        assert!(details.exit_code.is_none());
    }

    // ----------------------------------------------------------------------
    // logs_stream / stats_stream / pull_image_stream — shape conversion
    // tests that exercise the pure translation helpers without a daemon.
    // ----------------------------------------------------------------------

    #[test]
    fn build_logs_options_maps_every_field() {
        let opts = LogsStreamOptions {
            follow: true,
            tail: Some(50),
            since: Some(1_700_000_000),
            until: Some(1_700_000_500),
            timestamps: true,
            stdout: true,
            stderr: false,
        };
        let bollard = build_logs_options(&opts);
        assert!(bollard.follow);
        assert!(bollard.stdout);
        assert!(!bollard.stderr);
        assert!(bollard.timestamps);
        assert_eq!(bollard.tail, "50");
        assert_eq!(bollard.since, 1_700_000_000_i32);
        assert_eq!(bollard.until, 1_700_000_500_i32);
    }

    #[test]
    fn build_logs_options_default_tail_is_all_and_zero_window() {
        // `tail = None` -> bollard sentinel "all"; `since` / `until` = None
        // -> 0 (Docker's "no bound" sentinel).
        let opts = LogsStreamOptions::default();
        let bollard = build_logs_options(&opts);
        assert_eq!(bollard.tail, "all");
        assert_eq!(bollard.since, 0);
        assert_eq!(bollard.until, 0);
        assert!(!bollard.follow);
        assert!(!bollard.stdout);
        assert!(!bollard.stderr);
        assert!(!bollard.timestamps);
    }

    #[test]
    fn log_output_to_chunk_classifies_each_variant() {
        // StdOut and Console both translate to the Stdout channel.
        let stdout = log_output_to_chunk(
            bollard::container::LogOutput::StdOut {
                message: bytes::Bytes::from_static(b"hello\n"),
            },
            false,
        );
        assert_eq!(stdout.stream, LogChannel::Stdout);
        assert_eq!(stdout.bytes.as_ref(), b"hello\n");
        assert!(
            stdout.timestamp.is_none(),
            "no timestamp when not requested"
        );

        let console = log_output_to_chunk(
            bollard::container::LogOutput::Console {
                message: bytes::Bytes::from_static(b"tty\n"),
            },
            false,
        );
        assert_eq!(console.stream, LogChannel::Stdout);

        // StdErr -> Stderr.
        let stderr = log_output_to_chunk(
            bollard::container::LogOutput::StdErr {
                message: bytes::Bytes::from_static(b"oops\n"),
            },
            true,
        );
        assert_eq!(stderr.stream, LogChannel::Stderr);
        assert_eq!(stderr.bytes.as_ref(), b"oops\n");
        assert!(
            stderr.timestamp.is_some(),
            "timestamp populated when requested"
        );

        // StdIn -> Stdin (rare but supported).
        let stdin = log_output_to_chunk(
            bollard::container::LogOutput::StdIn {
                message: bytes::Bytes::from_static(b"in\n"),
            },
            false,
        );
        assert_eq!(stdin.stream, LogChannel::Stdin);
    }

    #[test]
    fn translate_stats_sample_maps_cpu_memory_and_pids() {
        use bollard::models::{
            ContainerCpuStats, ContainerCpuUsage, ContainerMemoryStats, ContainerPidsStats,
        };

        let response = ContainerStatsResponse {
            cpu_stats: Some(ContainerCpuStats {
                cpu_usage: Some(ContainerCpuUsage {
                    total_usage: Some(123_456_789),
                    ..Default::default()
                }),
                system_cpu_usage: Some(987_654_321),
                online_cpus: Some(8),
                ..Default::default()
            }),
            memory_stats: Some(ContainerMemoryStats {
                usage: Some(50 * 1024 * 1024),
                limit: Some(256 * 1024 * 1024),
                ..Default::default()
            }),
            pids_stats: Some(ContainerPidsStats {
                current: Some(17),
                limit: Some(0), // 0 means "unlimited" -> None on our side
            }),
            ..Default::default()
        };

        let sample = translate_stats_sample(&response);
        assert_eq!(sample.cpu_total_ns, 123_456_789);
        assert_eq!(sample.cpu_system_ns, 987_654_321);
        assert_eq!(sample.online_cpus, 8);
        assert_eq!(sample.mem_used_bytes, 50 * 1024 * 1024);
        assert_eq!(sample.mem_limit_bytes, 256 * 1024 * 1024);
        assert_eq!(sample.pids_current, 17);
        assert_eq!(sample.pids_limit, None, "Docker `limit: 0` -> None");
        // Network & blkio: no inputs -> 0 across the board.
        assert_eq!(sample.net_rx_bytes, 0);
        assert_eq!(sample.net_tx_bytes, 0);
        assert_eq!(sample.blkio_read_bytes, 0);
        assert_eq!(sample.blkio_write_bytes, 0);
    }

    #[test]
    fn translate_stats_sample_sums_networks_and_blkio() {
        use bollard::models::{
            ContainerBlkioStatEntry, ContainerBlkioStats, ContainerNetworkStats, ContainerPidsStats,
        };
        use std::collections::HashMap;

        let mut nets: HashMap<String, ContainerNetworkStats> = HashMap::new();
        nets.insert(
            "eth0".to_string(),
            ContainerNetworkStats {
                rx_bytes: Some(1000),
                tx_bytes: Some(2000),
                ..Default::default()
            },
        );
        nets.insert(
            "eth1".to_string(),
            ContainerNetworkStats {
                rx_bytes: Some(500),
                tx_bytes: Some(750),
                ..Default::default()
            },
        );

        let blkio = ContainerBlkioStats {
            io_service_bytes_recursive: Some(vec![
                ContainerBlkioStatEntry {
                    op: Some("Read".to_string()),
                    value: Some(4096),
                    ..Default::default()
                },
                ContainerBlkioStatEntry {
                    op: Some("Read".to_string()),
                    value: Some(2048),
                    ..Default::default()
                },
                ContainerBlkioStatEntry {
                    op: Some("Write".to_string()),
                    value: Some(8192),
                    ..Default::default()
                },
                // Unknown op (e.g. "Sync") should be ignored.
                ContainerBlkioStatEntry {
                    op: Some("Sync".to_string()),
                    value: Some(99),
                    ..Default::default()
                },
            ]),
            ..Default::default()
        };

        let response = ContainerStatsResponse {
            networks: Some(nets),
            blkio_stats: Some(blkio),
            pids_stats: Some(ContainerPidsStats {
                current: Some(5),
                // A real (non-zero) limit must round-trip to Some.
                limit: Some(4096),
            }),
            ..Default::default()
        };

        let sample = translate_stats_sample(&response);
        assert_eq!(sample.net_rx_bytes, 1500);
        assert_eq!(sample.net_tx_bytes, 2750);
        assert_eq!(sample.blkio_read_bytes, 4096 + 2048);
        assert_eq!(sample.blkio_write_bytes, 8192);
        assert_eq!(sample.pids_current, 5);
        assert_eq!(sample.pids_limit, Some(4096));
    }

    #[test]
    fn translate_pull_progress_maps_status_and_detail() {
        use bollard::models::ProgressDetail;

        let info = CreateImageInfo {
            id: Some("layer-abc".to_string()),
            status: Some("Downloading".to_string()),
            progress_detail: Some(ProgressDetail {
                current: Some(1024),
                total: Some(2048),
            }),
            ..Default::default()
        };
        match translate_pull_progress(info) {
            PullProgress::Status {
                id,
                status,
                progress,
                current,
                total,
            } => {
                assert_eq!(id.as_deref(), Some("layer-abc"));
                assert_eq!(status, "Downloading");
                assert!(progress.is_none(), "preformatted bar not modelled");
                assert_eq!(current, Some(1024));
                assert_eq!(total, Some(2048));
            }
            done @ PullProgress::Done { .. } => panic!("expected Status, got {done:?}"),
        }
    }

    #[test]
    fn translate_pull_progress_handles_missing_status_and_negative_detail() {
        use bollard::models::ProgressDetail;

        // Missing status -> empty string. Negative `current`/`total` are
        // dropped (u64::try_from fails) — they should not panic.
        let info = CreateImageInfo {
            id: None,
            status: None,
            progress_detail: Some(ProgressDetail {
                current: Some(-1),
                total: Some(-2),
            }),
            ..Default::default()
        };
        match translate_pull_progress(info) {
            PullProgress::Status {
                id,
                status,
                current,
                total,
                ..
            } => {
                assert!(id.is_none());
                assert!(status.is_empty());
                assert!(current.is_none());
                assert!(total.is_none());
            }
            done @ PullProgress::Done { .. } => panic!("expected Status, got {done:?}"),
        }
    }

    #[test]
    fn extract_digest_from_status_recognises_sha256() {
        assert_eq!(
            extract_digest_from_status(Some("Digest: sha256:abc123")).as_deref(),
            Some("sha256:abc123")
        );
        // Whitespace tolerance.
        assert_eq!(
            extract_digest_from_status(Some("  Digest:  sha256:def456  ")).as_deref(),
            Some("sha256:def456")
        );
        // Non-digest status lines are ignored.
        assert!(extract_digest_from_status(Some("Pulling fs layer")).is_none());
        assert!(extract_digest_from_status(Some("Digest: not-a-hash")).is_none());
        assert!(extract_digest_from_status(None).is_none());
    }

    #[test]
    fn docker_credentials_from_registry_auth_basic_and_token() {
        // Basic auth: username + password populate the corresponding fields,
        // everything else stays default-empty.
        let basic = RegistryAuth {
            username: "alice".to_string(),
            password: "hunter2".to_string(),
            auth_type: RegistryAuthType::Basic,
        };
        let creds = docker_credentials_from_registry_auth(&basic);
        assert_eq!(creds.username.as_deref(), Some("alice"));
        assert_eq!(creds.password.as_deref(), Some("hunter2"));
        assert!(creds.serveraddress.is_none());
        assert!(creds.identitytoken.is_none());

        // Token auth: same shape — the token rides as the password per the
        // Docker CLI convention noted on `docker_credentials_from_registry_auth`.
        let token = RegistryAuth {
            username: "<token>".to_string(),
            password: "ghp_xxx".to_string(),
            auth_type: RegistryAuthType::Token,
        };
        let creds = docker_credentials_from_registry_auth(&token);
        assert_eq!(creds.username.as_deref(), Some("<token>"));
        assert_eq!(creds.password.as_deref(), Some("ghp_xxx"));
    }

    #[test]
    fn build_create_exec_options_translates_full_payload() {
        // Every field populated: verifies cmd/env round-trip as
        // `Some(non_empty)` and that bool flags are forwarded literally
        // (none of them implicitly default — Docker treats unset as the
        // server-side default, which differs from `false`).
        let opts = ExecOptions {
            command: vec!["sh".into(), "-lc".into(), "echo hi".into()],
            env: vec!["PATH=/usr/bin".into(), "FOO=bar".into()],
            working_dir: Some("/work".into()),
            user: Some("1000:1000".into()),
            privileged: true,
            tty: true,
            attach_stdin: true,
            attach_stdout: true,
            attach_stderr: true,
        };
        let bollard_opts = build_create_exec_options(&opts);
        assert_eq!(
            bollard_opts.cmd.as_deref(),
            Some(&["sh".to_string(), "-lc".to_string(), "echo hi".to_string()][..])
        );
        assert_eq!(
            bollard_opts.env.as_deref(),
            Some(&["PATH=/usr/bin".to_string(), "FOO=bar".to_string()][..])
        );
        assert_eq!(bollard_opts.working_dir.as_deref(), Some("/work"));
        assert_eq!(bollard_opts.user.as_deref(), Some("1000:1000"));
        assert_eq!(bollard_opts.privileged, Some(true));
        assert_eq!(bollard_opts.tty, Some(true));
        assert_eq!(bollard_opts.attach_stdin, Some(true));
        assert_eq!(bollard_opts.attach_stdout, Some(true));
        assert_eq!(bollard_opts.attach_stderr, Some(true));
        assert!(bollard_opts.detach_keys.is_none());
    }

    #[test]
    fn build_create_exec_options_collapses_empty_collections_to_none() {
        // Default `ExecOptions` carries empty `command`/`env` vecs. We
        // translate those to `None` rather than `Some(vec![])` so Docker
        // sees the fields as absent — the daemon distinguishes "unset"
        // (use container default) from "empty array" (literally no
        // command), and the latter would fail with `cmd: required`.
        let opts = ExecOptions::default();
        let bollard_opts = build_create_exec_options(&opts);
        assert!(
            bollard_opts.cmd.is_none(),
            "empty command must translate to None"
        );
        assert!(
            bollard_opts.env.is_none(),
            "empty env must translate to None"
        );
        assert!(bollard_opts.working_dir.is_none());
        assert!(bollard_opts.user.is_none());
        // The bool flags do still get forwarded as `Some(false)` —
        // `false` is a meaningful explicit override (e.g. "definitely no
        // tty" even if the image config requested one).
        assert_eq!(bollard_opts.privileged, Some(false));
        assert_eq!(bollard_opts.tty, Some(false));
        assert_eq!(bollard_opts.attach_stdin, Some(false));
    }

    #[test]
    fn resize_options_for_maps_rows_to_h_and_cols_to_w() {
        // Docker's API uses `h` for rows and `w` for cols (matching
        // `TIOCSWINSZ`). The runtime channel is `(rows, cols)` to match
        // termios convention; verify the swap from the channel ordering
        // to the wire ordering happens correctly.
        let opts = resize_options_for(40, 132);
        assert_eq!(opts.height, 40, "rows -> h (height)");
        assert_eq!(opts.width, 132, "cols -> w (width)");

        // Edge: a (0, 0) resize is a valid termios pattern (means
        // "default"). It must round-trip without saturation.
        let zero = resize_options_for(0, 0);
        assert_eq!(zero.height, 0);
        assert_eq!(zero.width, 0);

        // Edge: u16::MAX in either dimension. Docker's wire type is i32
        // so this is well within range; nothing should clamp.
        let max = resize_options_for(u16::MAX, u16::MAX);
        assert_eq!(max.height, u16::MAX);
        assert_eq!(max.width, u16::MAX);
    }

    #[test]
    fn extract_exec_exit_code_handles_present_missing_and_out_of_range() {
        // Common case: a real Unix exit status fits in i32 cleanly.
        let inspect = ExecInspectResponse {
            exit_code: Some(0),
            ..Default::default()
        };
        assert_eq!(extract_exec_exit_code(&inspect), 0);

        let nonzero = ExecInspectResponse {
            exit_code: Some(137), // SIGKILL convention
            ..Default::default()
        };
        assert_eq!(extract_exec_exit_code(&nonzero), 137);

        // Missing exit code (e.g. inspect raced the exec finishing)
        // collapses to 0 to match the existing buffered `exec` behaviour.
        let missing = ExecInspectResponse {
            exit_code: None,
            ..Default::default()
        };
        assert_eq!(extract_exec_exit_code(&missing), 0);

        // Wire type is i64; values outside i32 range saturate (impossible
        // for real exits, but the type system permits it).
        let too_big = ExecInspectResponse {
            exit_code: Some(i64::from(i32::MAX) + 1),
            ..Default::default()
        };
        assert_eq!(extract_exec_exit_code(&too_big), i32::MAX);

        let too_small = ExecInspectResponse {
            exit_code: Some(i64::from(i32::MIN) - 1),
            ..Default::default()
        };
        assert_eq!(extract_exec_exit_code(&too_small), i32::MIN);
    }

    /// Live end-to-end smoke: connects to the local Docker daemon, spawns
    /// an `alpine:3` container, drives an interactive `sh` exec through
    /// `exec_pty`, sends `exit 7\n`, and asserts the exit future resolves
    /// with `7`. Marked `#[ignore]` so it doesn't run in the normal test
    /// suite — bring it back with `cargo test ... -- --ignored` on a host
    /// that has Docker available.
    #[ignore = "requires a running local Docker daemon"]
    #[tokio::test]
    async fn exec_pty_round_trip_against_real_docker() {
        use crate::runtime::ContainerId;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let runtime = DockerRuntime::new(None)
            .await
            .expect("connect to local docker");

        // Caller is expected to provision the container themselves; for
        // the smoke test we rely on a pre-existing `alpine` container
        // named `zlayer-exec-pty-smoke-0` so this test stays independent
        // of the rest of the runtime API.
        let id = ContainerId::new("exec-pty-smoke".to_string(), 0);

        let opts = ExecOptions {
            command: vec!["sh".into()],
            tty: true,
            attach_stdin: true,
            attach_stdout: true,
            attach_stderr: true,
            ..Default::default()
        };

        let mut handle = runtime.exec_pty(&id, opts).await.expect("exec_pty starts");

        handle
            .stream
            .write_all(b"exit 7\n")
            .await
            .expect("send exit 7");
        handle.stream.flush().await.expect("flush");

        // Drain output to EOF so the daemon-side process actually exits
        // before we ask for the exit code.
        let mut buf = Vec::new();
        let _ = handle.stream.read_to_end(&mut buf).await;

        let code = handle.exit.await.expect("exit future resolves");
        assert_eq!(code, 7, "shell exited with the value we asked for");
    }
}
