//! Docker-based container runtime using bollard
//!
//! Provides cross-platform support for Windows, macOS, and Linux
//! by connecting to the Docker daemon.

use crate::cgroups_stats::ContainerStats;
use crate::error::{AgentError, Result};
use crate::runtime::{
    signal_name_from_exit_code, ContainerId, ContainerInspectDetails, ContainerState, ExecEvent,
    ExecEventStream, HealthDetail, ImageInfo, NetworkAttachmentDetail, PruneResult, Runtime,
    WaitOutcome, WaitReason,
};
use bollard::auth::DockerCredentials;
use bollard::errors::Error as BollardError;
use bollard::exec::{CreateExecOptions, StartExecResults};
use bollard::models::{
    ContainerCreateBody, DeviceMapping, DeviceRequest, HostConfig, ImageInspect, PortBinding,
    RestartPolicy, RestartPolicyNameEnum,
};
use bollard::query_parameters::{
    CreateContainerOptions, CreateImageOptions, LogsOptions, RemoveContainerOptions,
    StartContainerOptions, StatsOptions, StopContainerOptions, WaitContainerOptions,
};
use bollard::Docker;
use futures_util::StreamExt;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tracing::instrument;
use zlayer_observability::logs::{LogEntry, LogSource, LogStream};
use zlayer_spec::{PullPolicy, RegistryAuth, RegistryAuthType, ServiceSpec};

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
        // Parse image into name and tag
        let (name, tag) = parse_image_ref(image);

        tracing::info!(
            image = %image,
            name = %name,
            tag = %tag,
            inline_auth = auth.is_some(),
            "pulling image"
        );

        let options = CreateImageOptions {
            from_image: Some(name.to_string()),
            tag: if tag.is_empty() {
                None
            } else {
                Some(tag.to_string())
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

/// Parse an image reference into name and tag
fn parse_image_ref(image: &str) -> (&str, &str) {
    // Handle digests (image@sha256:...)
    if image.contains('@') {
        // For digest references, return the whole thing as the name
        return (image, "");
    }

    // Handle tag (image:tag)
    if let Some((name, tag)) = image.rsplit_once(':') {
        // Make sure this isn't a port number in the registry (e.g., localhost:5000/image)
        // If there's a '/' after the ':', it's a registry with a port
        if !tag.contains('/') {
            return (name, tag);
        }
    }

    (image, "latest")
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
        self.pull_image_with_policy(image, PullPolicy::IfNotPresent, None)
            .await
    }

    /// Pull an image to local storage with a specific policy, honouring an
    /// optional inline [`RegistryAuth`] (§3.10).
    #[instrument(
        skip(self, auth),
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
            let (name, tag) = parse_image_ref(image);
            if tag.is_empty() {
                tracing::debug!(
                    image = %image,
                    "image pinned by digest and present, skipping pull"
                );
                return Ok(());
            }

            // Step 3: compare local vs remote digests.
            let local_digest = extract_local_digest(&local_inspect, name);

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
            image: Some(spec.image.name.clone()),
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

        // Get the first (and only) result from the wait stream
        let wait_response = stream
            .next()
            .await
            .ok_or_else(|| AgentError::NotFound {
                container: name.clone(),
                reason: "wait stream closed unexpectedly".to_string(),
            })?
            .map_err(|e| AgentError::NotFound {
                container: name.clone(),
                reason: format!("failed to wait for container: {e}"),
            })?;

        #[allow(clippy::cast_possible_truncation)]
        let exit_code = wait_response.status_code as i32;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_container_name() {
        let id = ContainerId {
            service: "myservice".to_string(),
            replica: 1,
        };
        assert_eq!(container_name(&id), "zlayer-myservice-1");
    }

    #[test]
    fn test_container_name_with_different_replicas() {
        let id1 = ContainerId {
            service: "api".to_string(),
            replica: 0,
        };
        let id2 = ContainerId {
            service: "api".to_string(),
            replica: 42,
        };
        assert_eq!(container_name(&id1), "zlayer-api-0");
        assert_eq!(container_name(&id2), "zlayer-api-42");
    }

    #[test]
    fn test_parse_image_ref_with_tag() {
        let (name, tag) = parse_image_ref("nginx:1.25");
        assert_eq!(name, "nginx");
        assert_eq!(tag, "1.25");
    }

    #[test]
    fn test_parse_image_ref_without_tag() {
        let (name, tag) = parse_image_ref("nginx");
        assert_eq!(name, "nginx");
        assert_eq!(tag, "latest");
    }

    #[test]
    fn test_parse_image_ref_with_registry_and_tag() {
        let (name, tag) = parse_image_ref("ghcr.io/org/image:v1.0.0");
        assert_eq!(name, "ghcr.io/org/image");
        assert_eq!(tag, "v1.0.0");
    }

    #[test]
    fn test_parse_image_ref_with_registry_port_and_tag() {
        let (name, tag) = parse_image_ref("localhost:5000/myimage:latest");
        assert_eq!(name, "localhost:5000/myimage");
        assert_eq!(tag, "latest");
    }

    #[test]
    fn test_parse_image_ref_with_digest() {
        let image = "nginx@sha256:abc123def456";
        let (name, tag) = parse_image_ref(image);
        assert_eq!(name, image);
        assert_eq!(tag, "");
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
            })
            .collect();

        ServiceSpec {
            rtype: ResourceType::Service,
            schedule: None,
            image: ImageSpec {
                name: "test:latest".to_string(),
                pull_policy: PullPolicy::IfNotPresent,
            },
            resources: ResourcesSpec::default(),
            env: HashMap::new(),
            command: CommandSpec::default(),
            network: ServiceNetworkSpec::default(),
            endpoints,
            scale: ScaleSpec::default(),
            depends: vec![],
            health: HealthSpec {
                start_grace: None,
                interval: None,
                timeout: None,
                retries: 3,
                check: HealthCheck::Tcp { port: 0 },
            },
            init: InitSpec::default(),
            errors: ErrorsSpec::default(),
            devices: vec![],
            storage: vec![],
            port_mappings: vec![],
            capabilities: vec![],
            privileged: false,
            node_mode: NodeMode::default(),
            node_selector: None,
            service_type: ServiceType::default(),
            wasm: None,
            logs: None,
            host_network: false,
            hostname: None,
            dns: Vec::new(),
            extra_hosts: Vec::new(),
            restart_policy: None,
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
}
