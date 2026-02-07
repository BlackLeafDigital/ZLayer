//! Docker-based container runtime using bollard
//!
//! Provides cross-platform support for Windows, macOS, and Linux
//! by connecting to the Docker daemon.

use crate::cgroups_stats::ContainerStats;
use crate::error::{AgentError, Result};
use crate::runtime::{ContainerId, ContainerState, Runtime};
use bollard::exec::{CreateExecOptions, StartExecResults};
use bollard::models::{ContainerCreateBody, DeviceMapping, DeviceRequest, HostConfig, PortBinding};
use bollard::query_parameters::{
    CreateContainerOptions, CreateImageOptions, LogsOptions, RemoveContainerOptions,
    StartContainerOptions, StatsOptions, StopContainerOptions, WaitContainerOptions,
};
use bollard::Docker;
use futures_util::StreamExt;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tracing::instrument;
use zlayer_spec::{PullPolicy, ServiceSpec};

/// Docker-based container runtime using bollard
///
/// Provides cross-platform support for Windows, macOS, and Linux
/// by connecting to the Docker daemon.
pub struct DockerRuntime {
    docker: Docker,
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
    pub async fn new() -> Result<Self> {
        let docker = Docker::connect_with_local_defaults()
            .map_err(|e| AgentError::Internal(format!("Failed to connect to Docker: {}", e)))?;

        // Verify connection by pinging the daemon
        docker
            .ping()
            .await
            .map_err(|e| AgentError::Internal(format!("Docker ping failed: {}", e)))?;

        tracing::info!("Connected to Docker daemon");
        Ok(Self { docker })
    }

    /// Create a new Docker runtime with custom connection options
    ///
    /// # Arguments
    ///
    /// * `docker` - A pre-configured bollard Docker client
    pub fn with_client(docker: Docker) -> Self {
        Self { docker }
    }
}

/// Generate a container name from a ContainerId
fn container_name(id: &ContainerId) -> String {
    format!("zlayer-{}-{}", id.service, id.replica)
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

/// Build exposed ports list for Docker container config
fn build_exposed_ports(spec: &ServiceSpec) -> Vec<String> {
    spec.endpoints
        .iter()
        .map(|endpoint| format!("{}/tcp", endpoint.port))
        .collect()
}

/// Build host configuration for Docker container
fn build_host_config(spec: &ServiceSpec) -> HostConfig {
    let mut port_bindings: HashMap<String, Option<Vec<PortBinding>>> = HashMap::new();

    for endpoint in &spec.endpoints {
        let key = format!("{}/tcp", endpoint.port);
        let binding = PortBinding {
            host_ip: Some("0.0.0.0".to_string()),
            host_port: Some(endpoint.port.to_string()),
        };
        port_bindings.insert(key, Some(vec![binding]));
    }

    // Build memory limit if specified
    let memory = spec.resources.memory.as_ref().and_then(|m| parse_memory(m));

    // Build CPU limit (Docker uses nano-CPUs: 1 CPU = 1e9 nano-CPUs)
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
        match gpu.vendor.as_str() {
            "nvidia" => {
                // NVIDIA Container Toolkit handles this via device_requests
                device_requests = Some(vec![DeviceRequest {
                    driver: Some("nvidia".into()),
                    count: Some(gpu.count as i64),
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
                for i in 0..gpu.count {
                    let render_path = format!("/dev/dri/renderD{}", 128 + i);
                    devices.push(DeviceMapping {
                        path_on_host: Some(render_path.clone()),
                        path_in_container: Some(render_path),
                        cgroup_permissions: Some("rwm".into()),
                    });
                    let card_path = format!("/dev/dri/card{}", i);
                    devices.push(DeviceMapping {
                        path_on_host: Some(card_path.clone()),
                        path_in_container: Some(card_path),
                        cgroup_permissions: Some("rwm".into()),
                    });
                }
            }
            "intel" => {
                // Intel GPU - pass through DRI devices
                for i in 0..gpu.count {
                    let render_path = format!("/dev/dri/renderD{}", 128 + i);
                    devices.push(DeviceMapping {
                        path_on_host: Some(render_path.clone()),
                        path_in_container: Some(render_path),
                        cgroup_permissions: Some("rwm".into()),
                    });
                    let card_path = format!("/dev/dri/card{}", i);
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
                for i in 0..gpu.count {
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
        ..Default::default()
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

    Some((num * multiplier as f64) as i64)
}

#[async_trait::async_trait]
impl Runtime for DockerRuntime {
    /// Pull an image to local storage with default policy (IfNotPresent)
    #[instrument(
        skip(self),
        fields(
            otel.name = "image.pull",
            container.image.name = %image,
        )
    )]
    async fn pull_image(&self, image: &str) -> Result<()> {
        self.pull_image_with_policy(image, PullPolicy::IfNotPresent)
            .await
    }

    /// Pull an image to local storage with a specific policy
    #[instrument(
        skip(self),
        fields(
            otel.name = "image.pull",
            container.image.name = %image,
            pull_policy = ?policy,
        )
    )]
    async fn pull_image_with_policy(&self, image: &str, policy: PullPolicy) -> Result<()> {
        // Handle Never policy - don't pull at all
        if matches!(policy, PullPolicy::Never) {
            tracing::debug!(image = %image, "pull policy is Never, skipping pull");
            return Ok(());
        }

        // Handle IfNotPresent - check if image exists
        if matches!(policy, PullPolicy::IfNotPresent)
            && self.docker.inspect_image(image).await.is_ok()
        {
            tracing::debug!(image = %image, "image already present, skipping pull");
            return Ok(());
        }

        // Parse image into name and tag
        let (name, tag) = parse_image_ref(image);

        tracing::info!(image = %image, name = %name, tag = %tag, "pulling image");

        let options = CreateImageOptions {
            from_image: Some(name.to_string()),
            tag: if tag.is_empty() {
                None
            } else {
                Some(tag.to_string())
            },
            ..Default::default()
        };

        let mut stream = self.docker.create_image(Some(options), None, None);

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

        // Build environment variables
        let env: Vec<String> = spec
            .env
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect();

        // Build exposed ports
        let exposed_ports = build_exposed_ports(spec);

        // Build host config
        let host_config = build_host_config(spec);

        // Build command/entrypoint
        let cmd = spec.command.args.clone();
        let entrypoint = spec.command.entrypoint.clone();
        let working_dir = spec.command.workdir.clone();

        let config = ContainerCreateBody {
            image: Some(spec.image.name.clone()),
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

        let options = StopContainerOptions {
            t: Some(timeout.as_secs() as i32),
            signal: None,
        };

        self.docker
            .stop_container(&name, Some(options))
            .await
            .map_err(|e| AgentError::NotFound {
                container: name.clone(),
                reason: format!("failed to stop container: {}", e),
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
                reason: format!("failed to remove container: {}", e),
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
                reason: format!("failed to inspect container: {}", e),
            })?;

        // Extract the state from the inspection result
        let state = inspect.state.ok_or_else(|| {
            AgentError::Internal(format!("Container {} has no state information", name))
        })?;

        // Map Docker state to our ContainerState enum
        let container_state = match state.status {
            Some(bollard::models::ContainerStateStatusEnum::CREATED) => ContainerState::Pending,
            Some(bollard::models::ContainerStateStatusEnum::RUNNING) => ContainerState::Running,
            Some(bollard::models::ContainerStateStatusEnum::PAUSED) => ContainerState::Running, // Treat paused as running
            Some(bollard::models::ContainerStateStatusEnum::RESTARTING) => {
                ContainerState::Initializing
            }
            Some(bollard::models::ContainerStateStatusEnum::REMOVING) => ContainerState::Stopping,
            Some(bollard::models::ContainerStateStatusEnum::EXITED) => {
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
    async fn container_logs(&self, id: &ContainerId, tail: usize) -> Result<String> {
        let name = container_name(id);

        let options = LogsOptions {
            stdout: true,
            stderr: true,
            tail: tail.to_string(),
            timestamps: false,
            ..Default::default()
        };

        let mut stream = self.docker.logs(&name, Some(options));
        let mut output = String::new();

        while let Some(result) = stream.next().await {
            match result {
                Ok(log_output) => {
                    output.push_str(&log_output.to_string());
                }
                Err(e) => {
                    return Err(AgentError::NotFound {
                        container: name.clone(),
                        reason: format!("failed to get logs: {}", e),
                    });
                }
            }
        }

        tracing::debug!(container = %name, bytes = output.len(), "got container logs");
        Ok(output)
    }

    /// Execute a command inside a container
    ///
    /// Returns a tuple of (exit_code, stdout, stderr)
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
                reason: format!("failed to create exec: {}", e),
            })?;

        // Start the exec and collect output
        let start_result = self
            .docker
            .start_exec(&exec_created.id, None)
            .await
            .map_err(|e| AgentError::Internal(format!("failed to start exec: {}", e)))?;

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
            .map_err(|e| AgentError::Internal(format!("failed to inspect exec: {}", e)))?;

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
                reason: format!("failed to get stats: {}", e),
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
                reason: format!("failed to wait for container: {}", e),
            })?;

        let exit_code = wait_response.status_code as i32;

        tracing::info!(container = %name, exit_code = exit_code, "container exited");

        Ok(exit_code)
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
    async fn get_logs(&self, id: &ContainerId) -> Result<Vec<String>> {
        let name = container_name(id);

        let options = LogsOptions {
            stdout: true,
            stderr: true,
            tail: "all".to_string(),
            timestamps: false,
            ..Default::default()
        };

        let mut stream = self.docker.logs(&name, Some(options));
        let mut lines = Vec::new();

        while let Some(result) = stream.next().await {
            match result {
                Ok(log_output) => {
                    // Each log line might contain newlines, so split them
                    let text = log_output.to_string();
                    for line in text.lines() {
                        lines.push(line.to_string());
                    }
                }
                Err(e) => {
                    return Err(AgentError::NotFound {
                        container: name.clone(),
                        reason: format!("failed to get logs: {}", e),
                    });
                }
            }
        }

        tracing::debug!(container = %name, line_count = lines.len(), "got container logs");
        Ok(lines)
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
                reason: format!("failed to inspect container: {}", e),
            })?;

        // Extract the PID from the state - only return it if the container is running
        // A PID of 0 means the container is not running
        let pid =
            inspect
                .state
                .and_then(|s| s.pid)
                .and_then(|p| if p > 0 { Some(p as u32) } else { None });

        tracing::debug!(container = %name, pid = ?pid, "got container PID");
        Ok(pid)
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
        let host_config = build_host_config(&spec);

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
        let host_config = build_host_config(&spec);
        assert_eq!(host_config.privileged, Some(true));
    }

    /// Helper to create a minimal test ServiceSpec
    fn create_test_spec(ports: Vec<u16>) -> ServiceSpec {
        use zlayer_spec::*;

        let endpoints: Vec<EndpointSpec> = ports
            .into_iter()
            .enumerate()
            .map(|(i, port)| EndpointSpec {
                name: format!("endpoint{}", i),
                protocol: Protocol::Http,
                port,
                path: None,
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
            network: NetworkSpec::default(),
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
            capabilities: vec![],
            privileged: false,
            node_mode: NodeMode::default(),
            node_selector: None,
            service_type: ServiceType::default(),
            wasm_http: None,
        }
    }
}
