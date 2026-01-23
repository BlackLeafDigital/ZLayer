//! Containerd runtime implementation
//!
//! Implements the Runtime trait using containerd-client to provide
//! real container lifecycle management.

use crate::error::{AgentError, Result};
use crate::runtime::{ContainerId, ContainerState, Runtime};
use containerd_client::services::v1::container::Runtime as ContainerRuntime;
use containerd_client::services::v1::containers_client::ContainersClient;
use containerd_client::services::v1::content_client::ContentClient;
use containerd_client::services::v1::images_client::ImagesClient;
use containerd_client::services::v1::snapshots::snapshots_client::SnapshotsClient;
use containerd_client::services::v1::snapshots::{
    MountsRequest, PrepareSnapshotRequest, RemoveSnapshotRequest,
};
use containerd_client::services::v1::tasks_client::TasksClient;
use containerd_client::services::v1::transfer_client::TransferClient;
use containerd_client::services::v1::{
    Container, CreateContainerRequest, CreateTaskRequest, DeleteContainerRequest,
    DeleteTaskRequest, ExecProcessRequest, GetContainerRequest, GetImageRequest,
    GetRequest as GetTaskRequest, KillRequest, ReadContentRequest, StartRequest, TransferRequest,
    WaitRequest,
};
use containerd_client::types::transfer::{ImageStore, OciRegistry, UnpackConfiguration};
use containerd_client::types::{Mount, Platform};
use containerd_client::{connect, to_any, with_namespace};
use oci_spec::runtime::{
    LinuxBuilder, LinuxNamespaceBuilder, LinuxNamespaceType, ProcessBuilder, RootBuilder, Spec,
    SpecBuilder, UserBuilder,
};
use sha2::{Digest, Sha256};
use spec::ServiceSpec;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use tokio::fs;
use tokio::io::AsyncReadExt;
use tokio::sync::RwLock;
use tonic::transport::Channel;
// Required for with_namespace! macro
use tonic::Request;

/// Default containerd socket path
const DEFAULT_SOCKET_PATH: &str = "/run/containerd/containerd.sock";

/// Default namespace for ZLayer containers
const DEFAULT_NAMESPACE: &str = "zlayer";

/// Default snapshotter (overlayfs)
const DEFAULT_SNAPSHOTTER: &str = "overlayfs";

/// Default runtime name
const DEFAULT_RUNTIME: &str = "io.containerd.runc.v2";

/// Configuration for ContainerdRuntime
#[derive(Debug, Clone)]
pub struct ContainerdConfig {
    /// Path to containerd socket
    pub socket_path: PathBuf,
    /// Namespace for containers
    pub namespace: String,
    /// Snapshotter to use
    pub snapshotter: String,
    /// State directory for FIFOs and runtime state
    pub state_dir: PathBuf,
    /// Runtime name
    pub runtime: String,
}

impl Default for ContainerdConfig {
    fn default() -> Self {
        Self {
            socket_path: PathBuf::from(DEFAULT_SOCKET_PATH),
            namespace: DEFAULT_NAMESPACE.to_string(),
            snapshotter: DEFAULT_SNAPSHOTTER.to_string(),
            state_dir: PathBuf::from("/var/lib/zlayer/containers"),
            runtime: DEFAULT_RUNTIME.to_string(),
        }
    }
}

/// Container state tracking for local operations
#[derive(Debug)]
struct ContainerInfo {
    /// Image reference
    #[allow(dead_code)]
    image: String,
    /// Snapshot key
    snapshot_key: String,
    /// FIFO paths for stdout/stderr
    stdout_path: PathBuf,
    stderr_path: PathBuf,
}

/// Containerd-based container runtime
#[derive(Debug)]
pub struct ContainerdRuntime {
    /// gRPC channel to containerd
    channel: Channel,
    /// Configuration
    config: ContainerdConfig,
    /// Local container state tracking
    containers: RwLock<HashMap<String, ContainerInfo>>,
}

impl ContainerdRuntime {
    /// Create a new ContainerdRuntime with the given configuration
    pub async fn new(config: ContainerdConfig) -> Result<Self> {
        // Ensure state directory exists
        fs::create_dir_all(&config.state_dir)
            .await
            .map_err(|e| AgentError::CreateFailed {
                id: "runtime".to_string(),
                reason: format!("failed to create state directory: {}", e),
            })?;

        // Connect to containerd socket
        let channel = connect(&config.socket_path)
            .await
            .map_err(|e| AgentError::CreateFailed {
                id: "runtime".to_string(),
                reason: format!("failed to connect to containerd: {}", e),
            })?;

        Ok(Self {
            channel,
            config,
            containers: RwLock::new(HashMap::new()),
        })
    }

    /// Create a new ContainerdRuntime with default configuration
    pub async fn with_defaults() -> Result<Self> {
        Self::new(ContainerdConfig::default()).await
    }

    /// Get the container ID string for containerd
    fn container_id_str(&self, id: &ContainerId) -> String {
        format!("{}-{}", id.service, id.replica)
    }

    /// Get the snapshot key for a container
    fn snapshot_key(&self, id: &ContainerId) -> String {
        format!("{}-snapshot", self.container_id_str(id))
    }

    /// Get the state directory for a container
    fn container_state_dir(&self, id: &ContainerId) -> PathBuf {
        self.config.state_dir.join(self.container_id_str(id))
    }

    /// Create stdio FIFO paths for a container
    async fn create_fifos(&self, id: &ContainerId) -> Result<(PathBuf, PathBuf)> {
        let state_dir = self.container_state_dir(id);
        fs::create_dir_all(&state_dir)
            .await
            .map_err(|e| AgentError::CreateFailed {
                id: id.to_string(),
                reason: format!("failed to create container state directory: {}", e),
            })?;

        let stdout_path = state_dir.join("stdout");
        let stderr_path = state_dir.join("stderr");

        // Create FIFOs using nix
        for path in [&stdout_path, &stderr_path] {
            // Remove existing FIFO if present
            let _ = fs::remove_file(path).await;

            // Create new FIFO
            nix::unistd::mkfifo(
                path,
                nix::sys::stat::Mode::from_bits(0o644).unwrap_or(nix::sys::stat::Mode::S_IRUSR),
            )
            .map_err(|e| AgentError::CreateFailed {
                id: id.to_string(),
                reason: format!("failed to create FIFO {}: {}", path.display(), e),
            })?;
        }

        Ok((stdout_path, stderr_path))
    }

    /// Build OCI runtime spec from ServiceSpec
    fn build_oci_spec(&self, _id: &ContainerId, spec: &ServiceSpec) -> Result<Spec> {
        // Build user
        let user = UserBuilder::default()
            .uid(0u32)
            .gid(0u32)
            .build()
            .map_err(|e| AgentError::InvalidSpec(format!("failed to build user: {}", e)))?;

        // Build environment variables
        let mut env: Vec<String> =
            vec!["PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".to_string()];
        for (key, value) in &spec.env {
            env.push(format!("{}={}", key, value));
        }

        // Build process
        // Note: We use the image's default command since ServiceSpec doesn't specify one
        // The actual entrypoint comes from the image config
        let process = ProcessBuilder::default()
            .terminal(false)
            .user(user)
            .env(env)
            .cwd("/".to_string())
            .no_new_privileges(true)
            .build()
            .map_err(|e| AgentError::InvalidSpec(format!("failed to build process: {}", e)))?;

        // Build root filesystem
        let root = RootBuilder::default()
            .path("rootfs".to_string())
            .readonly(false)
            .build()
            .map_err(|e| AgentError::InvalidSpec(format!("failed to build root: {}", e)))?;

        // Build Linux namespaces
        let namespaces = vec![
            LinuxNamespaceBuilder::default()
                .typ(LinuxNamespaceType::Pid)
                .build()
                .unwrap(),
            LinuxNamespaceBuilder::default()
                .typ(LinuxNamespaceType::Ipc)
                .build()
                .unwrap(),
            LinuxNamespaceBuilder::default()
                .typ(LinuxNamespaceType::Uts)
                .build()
                .unwrap(),
            LinuxNamespaceBuilder::default()
                .typ(LinuxNamespaceType::Mount)
                .build()
                .unwrap(),
            // Network namespace will be configured by the network manager
        ];

        // Build Linux config with resource limits
        let mut linux_builder = LinuxBuilder::default().namespaces(namespaces);

        // Apply resource limits if specified
        if spec.resources.cpu.is_some() || spec.resources.memory.is_some() {
            use oci_spec::runtime::{LinuxCpuBuilder, LinuxMemoryBuilder, LinuxResourcesBuilder};

            let mut resources_builder = LinuxResourcesBuilder::default();

            if let Some(cpu_limit) = spec.resources.cpu {
                // Convert CPU cores to microseconds quota (100000 = 1 core period)
                let quota = (cpu_limit * 100_000.0) as i64;
                let cpu = LinuxCpuBuilder::default()
                    .quota(quota)
                    .period(100_000u64)
                    .build()
                    .ok();
                if let Some(cpu) = cpu {
                    resources_builder = resources_builder.cpu(cpu);
                }
            }

            if let Some(ref memory_str) = spec.resources.memory {
                if let Ok(bytes) = parse_memory_string(memory_str) {
                    let memory = LinuxMemoryBuilder::default()
                        .limit(bytes as i64)
                        .build()
                        .ok();
                    if let Some(memory) = memory {
                        resources_builder = resources_builder.memory(memory);
                    }
                }
            }

            if let Ok(resources) = resources_builder.build() {
                linux_builder = linux_builder.resources(resources);
            }
        }

        let linux = linux_builder
            .build()
            .map_err(|e| AgentError::InvalidSpec(format!("failed to build linux config: {}", e)))?;

        // Build the complete spec
        let oci_spec = SpecBuilder::default()
            .version("1.0.0".to_string())
            .root(root)
            .process(process)
            .linux(linux)
            .build()
            .map_err(|e| AgentError::InvalidSpec(format!("failed to build OCI spec: {}", e)))?;

        Ok(oci_spec)
    }

    /// Get mounts for a snapshot
    async fn get_snapshot_mounts(&self, snapshot_key: &str) -> Result<Vec<Mount>> {
        let mut client = SnapshotsClient::new(self.channel.clone());

        let request = MountsRequest {
            snapshotter: self.config.snapshotter.clone(),
            key: snapshot_key.to_string(),
        };

        let request = with_namespace!(request, self.config.namespace.as_str());

        let response = client
            .mounts(request)
            .await
            .map_err(|e| AgentError::NotFound {
                container: snapshot_key.to_string(),
                reason: format!("failed to get snapshot mounts: {}", e),
            })?;

        Ok(response.into_inner().mounts)
    }

    /// Map task status to ContainerState
    fn map_task_status(&self, status: i32) -> ContainerState {
        // containerd task status values
        // 0 = Unknown, 1 = Created, 2 = Running, 3 = Stopped, 4 = Paused, 5 = Pausing
        match status {
            1 => ContainerState::Pending,            // Created
            2 => ContainerState::Running,            // Running
            3 => ContainerState::Exited { code: 0 }, // Stopped (exit code not available here)
            4 | 5 => ContainerState::Stopping,       // Paused/Pausing
            _ => ContainerState::Failed {
                reason: format!("unknown status: {}", status),
            },
        }
    }

    /// Get the chain ID (rootfs snapshot parent) for an image
    async fn get_image_chain_id(&self, image: &str) -> Result<String> {
        // 1. Get image metadata
        let mut images_client = ImagesClient::new(self.channel.clone());
        let get_req = with_namespace!(
            GetImageRequest {
                name: image.to_string()
            },
            self.config.namespace.as_str()
        );
        let image_resp =
            images_client
                .get(get_req)
                .await
                .map_err(|e| AgentError::CreateFailed {
                    id: image.to_string(),
                    reason: format!("failed to get image: {}", e),
                })?;

        let img = image_resp
            .into_inner()
            .image
            .ok_or_else(|| AgentError::CreateFailed {
                id: image.to_string(),
                reason: "image not found".to_string(),
            })?;

        let manifest_digest = img
            .target
            .ok_or_else(|| AgentError::CreateFailed {
                id: image.to_string(),
                reason: "image has no target".to_string(),
            })?
            .digest;

        // 2. Read manifest to get config digest
        let mut content_client = ContentClient::new(self.channel.clone());
        let manifest_req = with_namespace!(
            ReadContentRequest {
                digest: manifest_digest,
                offset: 0,
                size: 0,
            },
            self.config.namespace.as_str()
        );

        let manifest_stream =
            content_client
                .read(manifest_req)
                .await
                .map_err(|e| AgentError::CreateFailed {
                    id: image.to_string(),
                    reason: format!("failed to read manifest: {}", e),
                })?;

        let mut manifest_bytes = Vec::new();
        let mut stream = manifest_stream.into_inner();
        while let Some(chunk) = stream
            .message()
            .await
            .map_err(|e| AgentError::CreateFailed {
                id: image.to_string(),
                reason: format!("failed to read manifest chunk: {}", e),
            })?
        {
            manifest_bytes.extend_from_slice(&chunk.data);
        }

        let manifest: serde_json::Value =
            serde_json::from_slice(&manifest_bytes).map_err(|e| AgentError::CreateFailed {
                id: image.to_string(),
                reason: format!("failed to parse manifest: {}", e),
            })?;

        let config_digest = manifest["config"]["digest"]
            .as_str()
            .ok_or_else(|| AgentError::CreateFailed {
                id: image.to_string(),
                reason: "manifest has no config digest".to_string(),
            })?
            .to_string();

        // 3. Read config to get diff_ids
        let config_req = with_namespace!(
            ReadContentRequest {
                digest: config_digest,
                offset: 0,
                size: 0,
            },
            self.config.namespace.as_str()
        );

        let config_stream =
            content_client
                .read(config_req)
                .await
                .map_err(|e| AgentError::CreateFailed {
                    id: image.to_string(),
                    reason: format!("failed to read config: {}", e),
                })?;

        let mut config_bytes = Vec::new();
        let mut stream = config_stream.into_inner();
        while let Some(chunk) = stream
            .message()
            .await
            .map_err(|e| AgentError::CreateFailed {
                id: image.to_string(),
                reason: format!("failed to read config chunk: {}", e),
            })?
        {
            config_bytes.extend_from_slice(&chunk.data);
        }

        let config: serde_json::Value =
            serde_json::from_slice(&config_bytes).map_err(|e| AgentError::CreateFailed {
                id: image.to_string(),
                reason: format!("failed to parse config: {}", e),
            })?;

        let diff_ids: Vec<String> = config["rootfs"]["diff_ids"]
            .as_array()
            .ok_or_else(|| AgentError::CreateFailed {
                id: image.to_string(),
                reason: "config has no diff_ids".to_string(),
            })?
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect();

        // 4. Compute chain ID
        Ok(compute_chain_id(&diff_ids))
    }
}

/// Parse memory string like "512Mi", "1Gi" to bytes
fn parse_memory_string(s: &str) -> std::result::Result<u64, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("empty memory string".to_string());
    }

    let (num_str, multiplier) = if let Some(n) = s.strip_suffix("Ki") {
        (n, 1024u64)
    } else if let Some(n) = s.strip_suffix("Mi") {
        (n, 1024u64 * 1024)
    } else if let Some(n) = s.strip_suffix("Gi") {
        (n, 1024u64 * 1024 * 1024)
    } else if let Some(n) = s.strip_suffix("Ti") {
        (n, 1024u64 * 1024 * 1024 * 1024)
    } else if let Some(n) = s.strip_suffix('K').or_else(|| s.strip_suffix('k')) {
        (n, 1000u64)
    } else if let Some(n) = s.strip_suffix('M').or_else(|| s.strip_suffix('m')) {
        (n, 1000u64 * 1000)
    } else if let Some(n) = s.strip_suffix('G').or_else(|| s.strip_suffix('g')) {
        (n, 1000u64 * 1000 * 1000)
    } else if let Some(n) = s.strip_suffix('T').or_else(|| s.strip_suffix('t')) {
        (n, 1000u64 * 1000 * 1000 * 1000)
    } else {
        (s, 1u64)
    };

    let num: u64 = num_str
        .parse()
        .map_err(|e| format!("invalid number: {}", e))?;

    Ok(num * multiplier)
}

/// Compute the chain ID from diff IDs (OCI image spec algorithm)
fn compute_chain_id(diff_ids: &[String]) -> String {
    let mut chain_id = String::new();
    for diff_id in diff_ids {
        if chain_id.is_empty() {
            chain_id = diff_id.clone();
        } else {
            let input = format!("{} {}", chain_id, diff_id);
            let mut hasher = Sha256::new();
            hasher.update(input.as_bytes());
            let result = hasher.finalize();
            chain_id = format!("sha256:{:x}", result);
        }
    }
    chain_id
}

#[async_trait::async_trait]
impl Runtime for ContainerdRuntime {
    /// Pull an image to local storage using TransferClient
    async fn pull_image(&self, image: &str) -> Result<()> {
        let mut client = TransferClient::new(self.channel.clone());

        // Create OCI registry source
        let source = OciRegistry {
            reference: image.to_string(),
            resolver: None,
        };

        // Map Rust architecture names to OCI platform architecture names
        let arch = match std::env::consts::ARCH {
            "x86_64" => "amd64",
            "aarch64" => "arm64",
            arch => arch,
        }
        .to_string();

        // Define target platform for unpacking
        let platform = Platform {
            os: "linux".to_string(),
            architecture: arch,
            variant: String::new(),
            os_version: String::new(),
        };

        // Create image store destination with unpack configuration
        let destination = ImageStore {
            name: image.to_string(),
            labels: HashMap::new(),
            platforms: vec![platform.clone()],
            all_metadata: false,
            manifest_limit: 0,
            extra_references: vec![],
            unpacks: vec![UnpackConfiguration {
                platform: Some(platform),
                snapshotter: self.config.snapshotter.clone(),
            }],
        };

        // Build transfer request
        let request = TransferRequest {
            source: Some(to_any(&source)),
            destination: Some(to_any(&destination)),
            options: None,
        };

        let request = with_namespace!(request, self.config.namespace.as_str());

        client
            .transfer(request)
            .await
            .map_err(|e| AgentError::PullFailed {
                image: image.to_string(),
                reason: e.to_string(),
            })?;

        Ok(())
    }

    /// Create a container
    async fn create_container(&self, id: &ContainerId, spec: &ServiceSpec) -> Result<()> {
        let container_id = self.container_id_str(id);
        let snapshot_key = self.snapshot_key(id);
        let image = &spec.image.name;

        // First, prepare a snapshot from the image
        let mut snapshots_client = SnapshotsClient::new(self.channel.clone());

        // Get the image's chain ID for snapshot parent
        let chain_id = self.get_image_chain_id(image).await?;

        // Prepare snapshot with correct parent
        let prepare_request = PrepareSnapshotRequest {
            snapshotter: self.config.snapshotter.clone(),
            key: snapshot_key.clone(),
            parent: chain_id,
            labels: HashMap::new(),
        };

        let prepare_request = with_namespace!(prepare_request, self.config.namespace.as_str());

        snapshots_client
            .prepare(prepare_request)
            .await
            .map_err(|e| AgentError::CreateFailed {
                id: container_id.clone(),
                reason: format!("failed to prepare snapshot: {}", e),
            })?;

        // Build OCI runtime spec
        let oci_spec = self.build_oci_spec(id, spec)?;
        let spec_json = serde_json::to_vec(&oci_spec).map_err(|e| AgentError::CreateFailed {
            id: container_id.clone(),
            reason: format!("failed to serialize OCI spec: {}", e),
        })?;

        // Pack spec into Any
        let spec_any = prost_types::Any {
            type_url: "types.containerd.io/opencontainers/runtime-spec/1/Spec".to_string(),
            value: spec_json,
        };

        // Create container
        let mut containers_client = ContainersClient::new(self.channel.clone());

        let container = Container {
            id: container_id.clone(),
            labels: HashMap::new(),
            image: image.clone(),
            runtime: Some(ContainerRuntime {
                name: self.config.runtime.clone(),
                options: None,
            }),
            spec: Some(spec_any),
            snapshotter: self.config.snapshotter.clone(),
            snapshot_key: snapshot_key.clone(),
            extensions: HashMap::new(),
            sandbox: String::new(),
            created_at: None,
            updated_at: None,
        };

        let request = CreateContainerRequest {
            container: Some(container),
        };

        let request = with_namespace!(request, self.config.namespace.as_str());

        containers_client
            .create(request)
            .await
            .map_err(|e| AgentError::CreateFailed {
                id: container_id.clone(),
                reason: format!("failed to create container: {}", e),
            })?;

        // Create FIFOs for stdio
        let (stdout_path, stderr_path) = self.create_fifos(id).await?;

        // Store container info locally
        {
            let mut containers = self.containers.write().await;
            containers.insert(
                container_id.clone(),
                ContainerInfo {
                    image: image.clone(),
                    snapshot_key,
                    stdout_path,
                    stderr_path,
                },
            );
        }

        Ok(())
    }

    /// Start a container
    async fn start_container(&self, id: &ContainerId) -> Result<()> {
        let container_id = self.container_id_str(id);

        // Get container info
        let (stdout_path, stderr_path, snapshot_key) = {
            let containers = self.containers.read().await;
            let info = containers
                .get(&container_id)
                .ok_or_else(|| AgentError::NotFound {
                    container: container_id.clone(),
                    reason: "container not found in local state".to_string(),
                })?;
            (
                info.stdout_path.clone(),
                info.stderr_path.clone(),
                info.snapshot_key.clone(),
            )
        };

        // Get snapshot mounts
        let mounts = self.get_snapshot_mounts(&snapshot_key).await?;

        // Create task
        let mut tasks_client = TasksClient::new(self.channel.clone());

        let create_request = CreateTaskRequest {
            container_id: container_id.clone(),
            rootfs: mounts,
            stdin: String::new(),
            stdout: stdout_path.to_string_lossy().to_string(),
            stderr: stderr_path.to_string_lossy().to_string(),
            terminal: false,
            checkpoint: None,
            options: None,
            runtime_path: String::new(),
        };

        let create_request = with_namespace!(create_request, self.config.namespace.as_str());

        tasks_client
            .create(create_request)
            .await
            .map_err(|e| AgentError::StartFailed {
                id: container_id.clone(),
                reason: format!("failed to create task: {}", e),
            })?;

        // Start task
        let start_request = StartRequest {
            container_id: container_id.clone(),
            exec_id: String::new(),
        };

        let start_request = with_namespace!(start_request, self.config.namespace.as_str());

        tasks_client
            .start(start_request)
            .await
            .map_err(|e| AgentError::StartFailed {
                id: container_id.clone(),
                reason: format!("failed to start task: {}", e),
            })?;

        Ok(())
    }

    /// Stop a container
    async fn stop_container(&self, id: &ContainerId, timeout: Duration) -> Result<()> {
        let container_id = self.container_id_str(id);
        let mut tasks_client = TasksClient::new(self.channel.clone());

        // Send SIGTERM first
        let kill_request = KillRequest {
            container_id: container_id.clone(),
            exec_id: String::new(),
            signal: 15, // SIGTERM
            all: true,
        };

        let kill_request = with_namespace!(kill_request, self.config.namespace.as_str());

        if let Err(e) = tasks_client.kill(kill_request).await {
            // Task might already be stopped
            tracing::debug!("SIGTERM failed (task may be stopped): {}", e);
        }

        // Wait for task to exit with timeout
        let wait_result = tokio::time::timeout(timeout, async {
            let wait_request = WaitRequest {
                container_id: container_id.clone(),
                exec_id: String::new(),
            };

            let wait_request = with_namespace!(wait_request, self.config.namespace.as_str());

            tasks_client.wait(wait_request).await
        })
        .await;

        match wait_result {
            Ok(Ok(_)) => {
                // Task exited cleanly
            }
            Ok(Err(e)) => {
                tracing::debug!("Wait failed (task may be stopped): {}", e);
            }
            Err(_) => {
                // Timeout - send SIGKILL
                tracing::debug!("Timeout waiting for container to stop, sending SIGKILL");

                let kill_request = KillRequest {
                    container_id: container_id.clone(),
                    exec_id: String::new(),
                    signal: 9, // SIGKILL
                    all: true,
                };

                let kill_request = with_namespace!(kill_request, self.config.namespace.as_str());

                if let Err(e) = tasks_client.kill(kill_request).await {
                    tracing::warn!("SIGKILL failed: {}", e);
                }
            }
        }

        // Delete the task
        let delete_request = DeleteTaskRequest {
            container_id: container_id.clone(),
        };

        let delete_request = with_namespace!(delete_request, self.config.namespace.as_str());

        if let Err(e) = tasks_client.delete(delete_request).await {
            tracing::warn!("Failed to delete task: {}", e);
        }

        Ok(())
    }

    /// Remove a container
    async fn remove_container(&self, id: &ContainerId) -> Result<()> {
        let container_id = self.container_id_str(id);

        // Get snapshot key from local state
        let snapshot_key = {
            let containers = self.containers.read().await;
            containers
                .get(&container_id)
                .map(|info| info.snapshot_key.clone())
        };

        // Delete container
        let mut containers_client = ContainersClient::new(self.channel.clone());

        let delete_request = DeleteContainerRequest {
            id: container_id.clone(),
        };

        let delete_request = with_namespace!(delete_request, self.config.namespace.as_str());

        containers_client
            .delete(delete_request)
            .await
            .map_err(|e| AgentError::NotFound {
                container: container_id.clone(),
                reason: format!("failed to delete container: {}", e),
            })?;

        // Remove snapshot if we have the key
        if let Some(snapshot_key) = snapshot_key {
            let mut snapshots_client = SnapshotsClient::new(self.channel.clone());

            let remove_request = RemoveSnapshotRequest {
                snapshotter: self.config.snapshotter.clone(),
                key: snapshot_key,
            };

            let remove_request = with_namespace!(remove_request, self.config.namespace.as_str());

            if let Err(e) = snapshots_client.remove(remove_request).await {
                tracing::warn!("Failed to remove snapshot: {}", e);
            }
        }

        // Remove local state
        {
            let mut containers = self.containers.write().await;
            containers.remove(&container_id);
        }

        // Clean up state directory
        let state_dir = self.container_state_dir(id);
        let _ = fs::remove_dir_all(&state_dir).await;

        Ok(())
    }

    /// Get container state
    async fn container_state(&self, id: &ContainerId) -> Result<ContainerState> {
        let container_id = self.container_id_str(id);
        let mut tasks_client = TasksClient::new(self.channel.clone());

        let request = GetTaskRequest {
            container_id: container_id.clone(),
            exec_id: String::new(),
        };

        let request = with_namespace!(request, self.config.namespace.as_str());

        match tasks_client.get(request).await {
            Ok(response) => {
                let process = response.into_inner().process;
                if let Some(process) = process {
                    let state = self.map_task_status(process.status);

                    // If task is stopped, get exit code
                    if process.status == 3 {
                        // Stopped
                        return Ok(ContainerState::Exited {
                            code: process.exit_status as i32,
                        });
                    }

                    return Ok(state);
                }

                // No process info - check if container exists
                let mut containers_client = ContainersClient::new(self.channel.clone());
                let get_request = GetContainerRequest {
                    id: container_id.clone(),
                };

                let get_request = with_namespace!(get_request, self.config.namespace.as_str());

                match containers_client.get(get_request).await {
                    Ok(_) => Ok(ContainerState::Pending), // Container exists but no task
                    Err(_) => Err(AgentError::NotFound {
                        container: container_id,
                        reason: "container not found".to_string(),
                    }),
                }
            }
            Err(e) => {
                // Task not found - check if container exists
                let mut containers_client = ContainersClient::new(self.channel.clone());
                let get_request = GetContainerRequest {
                    id: container_id.clone(),
                };

                let get_request = with_namespace!(get_request, self.config.namespace.as_str());

                match containers_client.get(get_request).await {
                    Ok(_) => Ok(ContainerState::Pending), // Container exists but no task
                    Err(_) => Err(AgentError::NotFound {
                        container: container_id,
                        reason: format!("container not found: {}", e),
                    }),
                }
            }
        }
    }

    /// Get container logs
    async fn container_logs(&self, id: &ContainerId, tail: usize) -> Result<String> {
        let container_id = self.container_id_str(id);

        // Get paths from local state
        let (stdout_path, stderr_path) = {
            let containers = self.containers.read().await;
            let info = containers
                .get(&container_id)
                .ok_or_else(|| AgentError::NotFound {
                    container: container_id.clone(),
                    reason: "container not found in local state".to_string(),
                })?;
            (info.stdout_path.clone(), info.stderr_path.clone())
        };

        let mut logs = String::new();

        // Read stdout (non-blocking)
        if let Ok(mut file) = tokio::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&stdout_path)
            .await
        {
            let mut content = String::new();
            let _ = file.read_to_string(&mut content).await;
            if !content.is_empty() {
                logs.push_str("[stdout]\n");
                logs.push_str(&content);
            }
        }

        // Read stderr (non-blocking)
        if let Ok(mut file) = tokio::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&stderr_path)
            .await
        {
            let mut content = String::new();
            let _ = file.read_to_string(&mut content).await;
            if !content.is_empty() {
                logs.push_str("\n[stderr]\n");
                logs.push_str(&content);
            }
        }

        // Apply tail limit
        if tail > 0 {
            let lines: Vec<&str> = logs.lines().collect();
            if lines.len() > tail {
                logs = lines[lines.len() - tail..].join("\n");
            }
        }

        Ok(logs)
    }

    /// Execute command in container
    async fn exec(&self, id: &ContainerId, cmd: &[String]) -> Result<(i32, String, String)> {
        let container_id = self.container_id_str(id);
        let exec_id = format!("exec-{}", uuid::Uuid::new_v4());

        // Create temporary FIFOs for exec output
        let state_dir = self.container_state_dir(id);
        let exec_stdout = state_dir.join(format!("{}-stdout", exec_id));
        let exec_stderr = state_dir.join(format!("{}-stderr", exec_id));

        // Create FIFOs
        for path in [&exec_stdout, &exec_stderr] {
            let _ = fs::remove_file(path).await;
            nix::unistd::mkfifo(
                path,
                nix::sys::stat::Mode::from_bits(0o644).unwrap_or(nix::sys::stat::Mode::S_IRUSR),
            )
            .map_err(|e| AgentError::CreateFailed {
                id: exec_id.clone(),
                reason: format!("failed to create exec FIFO: {}", e),
            })?;
        }

        let mut tasks_client = TasksClient::new(self.channel.clone());

        // Build exec process spec
        let user = UserBuilder::default()
            .uid(0u32)
            .gid(0u32)
            .build()
            .map_err(|e| AgentError::InvalidSpec(format!("failed to build user: {}", e)))?;

        let process = ProcessBuilder::default()
            .terminal(false)
            .user(user)
            .args(cmd.to_vec())
            .env(vec![
                "PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".to_string(),
            ])
            .cwd("/".to_string())
            .build()
            .map_err(|e| AgentError::InvalidSpec(format!("failed to build exec process: {}", e)))?;

        let process_json =
            serde_json::to_vec(&process).map_err(|e| AgentError::InvalidSpec(e.to_string()))?;

        let spec_any = prost_types::Any {
            type_url: "types.containerd.io/opencontainers/runtime-spec/1/Process".to_string(),
            value: process_json,
        };

        // Create exec process
        let exec_request = ExecProcessRequest {
            container_id: container_id.clone(),
            stdin: String::new(),
            stdout: exec_stdout.to_string_lossy().to_string(),
            stderr: exec_stderr.to_string_lossy().to_string(),
            terminal: false,
            spec: Some(spec_any),
            exec_id: exec_id.clone(),
        };

        let exec_request = with_namespace!(exec_request, self.config.namespace.as_str());

        tasks_client
            .exec(exec_request)
            .await
            .map_err(|e| AgentError::CreateFailed {
                id: exec_id.clone(),
                reason: format!("failed to create exec: {}", e),
            })?;

        // Start exec process
        let start_request = StartRequest {
            container_id: container_id.clone(),
            exec_id: exec_id.clone(),
        };

        let start_request = with_namespace!(start_request, self.config.namespace.as_str());

        tasks_client
            .start(start_request)
            .await
            .map_err(|e| AgentError::StartFailed {
                id: exec_id.clone(),
                reason: format!("failed to start exec: {}", e),
            })?;

        // Wait for exec to complete
        let wait_request = WaitRequest {
            container_id: container_id.clone(),
            exec_id: exec_id.clone(),
        };

        let wait_request = with_namespace!(wait_request, self.config.namespace.as_str());

        let wait_response =
            tasks_client
                .wait(wait_request)
                .await
                .map_err(|e| AgentError::StartFailed {
                    id: exec_id.clone(),
                    reason: format!("failed to wait for exec: {}", e),
                })?;

        let exit_status = wait_response.into_inner().exit_status as i32;

        // Read output (non-blocking)
        let mut stdout_content = String::new();
        let mut stderr_content = String::new();

        if let Ok(mut file) = tokio::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&exec_stdout)
            .await
        {
            let _ = file.read_to_string(&mut stdout_content).await;
        }

        if let Ok(mut file) = tokio::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&exec_stderr)
            .await
        {
            let _ = file.read_to_string(&mut stderr_content).await;
        }

        // Delete exec process
        let delete_request = containerd_client::services::v1::DeleteProcessRequest {
            container_id: container_id.clone(),
            exec_id: exec_id.clone(),
        };

        let delete_request = with_namespace!(delete_request, self.config.namespace.as_str());

        let _ = tasks_client.delete_process(delete_request).await;

        // Clean up FIFOs
        let _ = fs::remove_file(&exec_stdout).await;
        let _ = fs::remove_file(&exec_stderr).await;

        Ok((exit_status, stdout_content, stderr_content))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_memory_string() {
        assert_eq!(parse_memory_string("512Mi").unwrap(), 512 * 1024 * 1024);
        assert_eq!(parse_memory_string("1Gi").unwrap(), 1024 * 1024 * 1024);
        assert_eq!(parse_memory_string("2G").unwrap(), 2 * 1000 * 1000 * 1000);
        assert_eq!(parse_memory_string("1024").unwrap(), 1024);
        assert_eq!(parse_memory_string("512Ki").unwrap(), 512 * 1024);
    }

    #[test]
    fn test_parse_memory_string_errors() {
        assert!(parse_memory_string("").is_err());
        assert!(parse_memory_string("abc").is_err());
        assert!(parse_memory_string("12.5Mi").is_err());
    }

    #[test]
    fn test_containerd_config_default() {
        let config = ContainerdConfig::default();

        assert_eq!(
            config.socket_path,
            PathBuf::from("/run/containerd/containerd.sock")
        );
        assert_eq!(config.namespace, "zlayer");
        assert_eq!(config.snapshotter, "overlayfs");
        assert_eq!(
            config.state_dir,
            PathBuf::from("/var/lib/zlayer/containers")
        );
        assert_eq!(config.runtime, "io.containerd.runc.v2");
    }

    #[test]
    fn test_container_id_str() {
        // Can't test without runtime, but we can test the string generation
        let id = ContainerId {
            service: "myservice".to_string(),
            replica: 1,
        };

        let expected = "myservice-1";
        assert_eq!(format!("{}-{}", id.service, id.replica), expected);
    }

    #[test]
    fn test_snapshot_key_format() {
        // Test the snapshot key format without a runtime instance
        let id = ContainerId {
            service: "testservice".to_string(),
            replica: 2,
        };

        let container_id = format!("{}-{}", id.service, id.replica);
        let snapshot_key = format!("{}-snapshot", container_id);

        assert_eq!(snapshot_key, "testservice-2-snapshot");
    }

    #[test]
    fn test_container_state_dir_format() {
        // Test the container state directory format
        let config = ContainerdConfig::default();
        let id = ContainerId {
            service: "myapp".to_string(),
            replica: 3,
        };

        let container_id = format!("{}-{}", id.service, id.replica);
        let state_dir = config.state_dir.join(&container_id);

        assert_eq!(
            state_dir,
            PathBuf::from("/var/lib/zlayer/containers/myapp-3")
        );
    }

    /// Test that ContainerdRuntime::new() returns an error when containerd is unavailable
    /// This test works without a running containerd daemon
    #[tokio::test]
    async fn test_containerd_runtime_new_without_daemon() {
        // Use a non-existent socket path to simulate containerd being unavailable
        let config = ContainerdConfig {
            socket_path: PathBuf::from("/tmp/nonexistent-containerd.sock"),
            ..Default::default()
        };

        let result = ContainerdRuntime::new(config).await;

        // Should fail because containerd is not available at that socket
        assert!(
            result.is_err(),
            "Expected error when containerd unavailable"
        );

        match result {
            Err(AgentError::CreateFailed { id, reason }) => {
                assert_eq!(id, "runtime");
                // The error can be about state directory creation or connection failure
                assert!(
                    reason.contains("failed to connect to containerd")
                        || reason.contains("failed to create state directory"),
                    "Unexpected error reason: {}",
                    reason
                );
            }
            Err(other) => {
                panic!("Expected CreateFailed error, got: {:?}", other);
            }
            Ok(_) => {
                panic!("Expected error, but got Ok");
            }
        }
    }

    /// Test get_image_snapshot_key helper function logic
    /// This verifies the snapshot key generation pattern
    #[test]
    fn test_get_image_snapshot_key() {
        // The snapshot key pattern is: {service}-{replica}-snapshot
        let test_cases = vec![
            (("web", 1), "web-1-snapshot"),
            (("api", 5), "api-5-snapshot"),
            (("my-service", 10), "my-service-10-snapshot"),
        ];

        for ((service, replica), expected) in test_cases {
            let id = ContainerId {
                service: service.to_string(),
                replica,
            };
            let container_id_str = format!("{}-{}", id.service, id.replica);
            let snapshot_key = format!("{}-snapshot", container_id_str);
            assert_eq!(snapshot_key, expected);
        }
    }

    /// Test memory parsing with all supported suffixes
    #[test]
    fn test_parse_memory_string_all_suffixes() {
        // Binary units (IEC)
        assert_eq!(parse_memory_string("1Ki").unwrap(), 1024);
        assert_eq!(parse_memory_string("1Mi").unwrap(), 1024 * 1024);
        assert_eq!(parse_memory_string("1Gi").unwrap(), 1024 * 1024 * 1024);
        assert_eq!(
            parse_memory_string("1Ti").unwrap(),
            1024u64 * 1024 * 1024 * 1024
        );

        // Decimal units (SI)
        assert_eq!(parse_memory_string("1K").unwrap(), 1000);
        assert_eq!(parse_memory_string("1k").unwrap(), 1000);
        assert_eq!(parse_memory_string("1M").unwrap(), 1000 * 1000);
        assert_eq!(parse_memory_string("1m").unwrap(), 1000 * 1000);
        assert_eq!(parse_memory_string("1G").unwrap(), 1000 * 1000 * 1000);
        assert_eq!(parse_memory_string("1g").unwrap(), 1000 * 1000 * 1000);
        assert_eq!(
            parse_memory_string("1T").unwrap(),
            1000u64 * 1000 * 1000 * 1000
        );
        assert_eq!(
            parse_memory_string("1t").unwrap(),
            1000u64 * 1000 * 1000 * 1000
        );

        // No suffix (bytes)
        assert_eq!(parse_memory_string("1234567890").unwrap(), 1234567890);
    }

    /// Test ContainerdConfig cloning works correctly
    #[test]
    fn test_containerd_config_clone() {
        let config = ContainerdConfig {
            socket_path: PathBuf::from("/custom/socket.sock"),
            namespace: "custom-namespace".to_string(),
            snapshotter: "native".to_string(),
            state_dir: PathBuf::from("/custom/state"),
            runtime: "io.containerd.runsc.v1".to_string(),
        };

        let cloned = config.clone();

        assert_eq!(cloned.socket_path, config.socket_path);
        assert_eq!(cloned.namespace, config.namespace);
        assert_eq!(cloned.snapshotter, config.snapshotter);
        assert_eq!(cloned.state_dir, config.state_dir);
        assert_eq!(cloned.runtime, config.runtime);
    }
}
