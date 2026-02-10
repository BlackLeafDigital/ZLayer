//! OCI Bundle Creation
//!
//! Creates OCI-compliant bundles for container runtimes using libcontainer (youki).
//! A bundle consists of a directory with:
//! - config.json: OCI runtime specification
//! - rootfs/: Container filesystem (symlink or bind mount target)

use crate::error::{AgentError, Result};
use crate::runtime::ContainerId;
use oci_spec::runtime::{
    Capability, LinuxBuilder, LinuxCapabilitiesBuilder, LinuxCpuBuilder, LinuxDeviceBuilder,
    LinuxDeviceCgroupBuilder, LinuxDeviceType, LinuxMemoryBuilder, LinuxNamespaceBuilder,
    LinuxNamespaceType, LinuxResourcesBuilder, Mount, MountBuilder, ProcessBuilder, RootBuilder,
    Spec, SpecBuilder, UserBuilder,
};
use std::collections::{HashMap, HashSet};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use tokio::fs;
use zlayer_secrets::SecretsProvider;
use zlayer_spec::{ServiceSpec, StorageSpec, StorageTier};

/// Default bundle base directory
pub const DEFAULT_BUNDLE_DIR: &str = "/var/lib/zlayer/bundles";

/// All Linux capabilities for privileged mode
const ALL_CAPABILITIES: &[Capability] = &[
    Capability::AuditControl,
    Capability::AuditRead,
    Capability::AuditWrite,
    Capability::BlockSuspend,
    Capability::Bpf,
    Capability::CheckpointRestore,
    Capability::Chown,
    Capability::DacOverride,
    Capability::DacReadSearch,
    Capability::Fowner,
    Capability::Fsetid,
    Capability::IpcLock,
    Capability::IpcOwner,
    Capability::Kill,
    Capability::Lease,
    Capability::LinuxImmutable,
    Capability::MacAdmin,
    Capability::MacOverride,
    Capability::Mknod,
    Capability::NetAdmin,
    Capability::NetBindService,
    Capability::NetBroadcast,
    Capability::NetRaw,
    Capability::Perfmon,
    Capability::Setfcap,
    Capability::Setgid,
    Capability::Setpcap,
    Capability::Setuid,
    Capability::SysAdmin,
    Capability::SysBoot,
    Capability::SysChroot,
    Capability::SysModule,
    Capability::SysNice,
    Capability::SysPacct,
    Capability::SysPtrace,
    Capability::SysRawio,
    Capability::SysResource,
    Capability::SysTime,
    Capability::SysTtyConfig,
    Capability::Syslog,
    Capability::WakeAlarm,
];

/// Parse memory string like "512Mi", "1Gi" to bytes
///
/// Supports both IEC (binary) and SI (decimal) units:
/// - IEC: Ki, Mi, Gi, Ti (powers of 1024)
/// - SI: K/k, M/m, G/g, T/t (powers of 1000)
/// - No suffix: bytes
///
/// # Examples
/// ```ignore
/// assert_eq!(parse_memory_string("512Mi").unwrap(), 512 * 1024 * 1024);
/// assert_eq!(parse_memory_string("1Gi").unwrap(), 1024 * 1024 * 1024);
/// assert_eq!(parse_memory_string("2G").unwrap(), 2 * 1000 * 1000 * 1000);
/// ```
pub fn parse_memory_string(s: &str) -> std::result::Result<u64, String> {
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

/// Get major and minor device numbers from a device path
fn get_device_major_minor(path: &str) -> std::io::Result<(i64, i64)> {
    let metadata = std::fs::metadata(path)?;
    let rdev = metadata.rdev();
    // Major is upper 8 bits (after shifting), minor is lower 8 bits
    let major = ((rdev >> 8) & 0xff) as i64;
    let minor = (rdev & 0xff) as i64;
    Ok((major, minor))
}

/// Detect device type from path
fn get_device_type(path: &str) -> std::io::Result<LinuxDeviceType> {
    use std::os::unix::fs::FileTypeExt;
    let metadata = std::fs::metadata(path)?;
    let file_type = metadata.file_type();
    if file_type.is_char_device() {
        Ok(LinuxDeviceType::C)
    } else if file_type.is_block_device() {
        Ok(LinuxDeviceType::B)
    } else {
        Ok(LinuxDeviceType::U) // Unknown/other
    }
}

/// Builder for OCI container bundles
///
/// Creates the directory structure and config.json required for OCI-compliant
/// container runtimes like runc or youki.
///
/// # Example
/// ```ignore
/// let builder = BundleBuilder::new("/var/lib/zlayer/bundles/mycontainer".into())
///     .with_rootfs("/var/lib/zlayer/rootfs/myimage".into());
///
/// let bundle_path = builder.build(&container_id, &service_spec).await?;
/// ```
#[derive(Clone)]
pub struct BundleBuilder {
    /// Base directory for the bundle
    bundle_dir: PathBuf,
    /// Path to the unpacked rootfs (from image layers)
    rootfs_path: Option<PathBuf>,
    /// Custom hostname (defaults to container ID)
    hostname: Option<String>,
    /// Additional environment variables
    extra_env: Vec<(String, String)>,
    /// Custom working directory
    cwd: Option<String>,
    /// Custom command/args to run (overrides image default)
    args: Option<Vec<String>>,
    /// Pre-resolved volume paths from StorageManager
    volume_paths: HashMap<String, PathBuf>,
    /// Image configuration from the OCI registry (entrypoint, cmd, env, workdir, user)
    image_config: Option<zlayer_registry::ImageConfig>,
    /// Use host networking (skip Network namespace, container shares host network)
    host_network: bool,
    /// Secrets provider for resolving $S: prefixed env vars
    secrets_provider: Option<Arc<dyn SecretsProvider>>,
    /// Deployment scope for secret lookups (e.g., deployment name)
    deployment_scope: Option<String>,
}

impl std::fmt::Debug for BundleBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BundleBuilder")
            .field("bundle_dir", &self.bundle_dir)
            .field("rootfs_path", &self.rootfs_path)
            .field("hostname", &self.hostname)
            .field("extra_env", &self.extra_env)
            .field("cwd", &self.cwd)
            .field("args", &self.args)
            .field("volume_paths", &self.volume_paths)
            .field("image_config", &self.image_config)
            .field("host_network", &self.host_network)
            .field("secrets_provider", &self.secrets_provider.is_some())
            .field("deployment_scope", &self.deployment_scope)
            .finish()
    }
}

impl BundleBuilder {
    /// Create a new BundleBuilder with the specified bundle directory
    ///
    /// The bundle directory will be created if it doesn't exist.
    /// The structure will be:
    /// ```text
    /// {bundle_dir}/
    /// ├── config.json
    /// └── rootfs/  (symlink to actual rootfs or mount point)
    /// ```
    pub fn new(bundle_dir: PathBuf) -> Self {
        Self {
            bundle_dir,
            rootfs_path: None,
            hostname: None,
            extra_env: Vec::new(),
            cwd: None,
            args: None,
            volume_paths: HashMap::new(),
            image_config: None,
            host_network: false,
            secrets_provider: None,
            deployment_scope: None,
        }
    }

    /// Create a BundleBuilder for a container in the default bundle location
    pub fn for_container(container_id: &ContainerId) -> Self {
        let bundle_dir = PathBuf::from(DEFAULT_BUNDLE_DIR).join(container_id.to_string());
        Self::new(bundle_dir)
    }

    /// Set the rootfs path (from unpacked image layers)
    ///
    /// This path will be symlinked into the bundle as `rootfs/`
    pub fn with_rootfs(mut self, rootfs_path: PathBuf) -> Self {
        self.rootfs_path = Some(rootfs_path);
        self
    }

    /// Set a custom hostname for the container
    pub fn with_hostname(mut self, hostname: String) -> Self {
        self.hostname = Some(hostname);
        self
    }

    /// Add extra environment variables
    pub fn with_env(mut self, key: String, value: String) -> Self {
        self.extra_env.push((key, value));
        self
    }

    /// Set the working directory
    pub fn with_cwd(mut self, cwd: String) -> Self {
        self.cwd = Some(cwd);
        self
    }

    /// Set the command/args to run
    pub fn with_args(mut self, args: Vec<String>) -> Self {
        self.args = Some(args);
        self
    }

    /// Set pre-resolved volume paths from StorageManager
    ///
    /// These are used to map named/anonymous/S3 volumes to their host paths
    /// when building storage mounts in the OCI spec.
    pub fn with_volume_paths(mut self, volume_paths: HashMap<String, PathBuf>) -> Self {
        self.volume_paths = volume_paths;
        self
    }

    /// Set the OCI image configuration (entrypoint, cmd, env, workdir, user)
    ///
    /// When set, the image config provides defaults for the container process
    /// that are used when the deployment spec doesn't override them.
    pub fn with_image_config(mut self, config: zlayer_registry::ImageConfig) -> Self {
        self.image_config = Some(config);
        self
    }

    /// Enable host networking mode
    ///
    /// When true, the container will NOT get its own network namespace and will
    /// share the host's network stack. This is equivalent to Docker's `--network host`.
    /// Use this when overlay networking is unavailable or not desired.
    pub fn with_host_network(mut self, host_network: bool) -> Self {
        self.host_network = host_network;
        self
    }

    /// Set the secrets provider for resolving `$S:` prefixed environment variables
    ///
    /// When set, environment variables with `$S:secret-name` syntax will be resolved
    /// from this provider at bundle creation time.
    pub fn with_secrets_provider(mut self, provider: Arc<dyn SecretsProvider>) -> Self {
        self.secrets_provider = Some(provider);
        self
    }

    /// Set the deployment scope for secret lookups
    ///
    /// This is typically the deployment name and is used as the scope when
    /// resolving `$S:` prefixed environment variables.
    pub fn with_deployment_scope(mut self, scope: String) -> Self {
        self.deployment_scope = Some(scope);
        self
    }

    /// Get the bundle directory path
    pub fn bundle_dir(&self) -> &Path {
        &self.bundle_dir
    }

    /// Build the OCI bundle from a ServiceSpec
    ///
    /// Creates the bundle directory structure and generates config.json
    /// based on the provided service specification.
    ///
    /// # Returns
    /// The path to the bundle directory on success
    ///
    /// # Errors
    /// - `AgentError::CreateFailed` if directory creation fails
    /// - `AgentError::InvalidSpec` if the OCI spec generation fails
    pub async fn build(&self, container_id: &ContainerId, spec: &ServiceSpec) -> Result<PathBuf> {
        // Create bundle directory
        fs::create_dir_all(&self.bundle_dir)
            .await
            .map_err(|e| AgentError::CreateFailed {
                id: container_id.to_string(),
                reason: format!("failed to create bundle directory: {}", e),
            })?;

        // Set up rootfs (symlink or create empty directory)
        let rootfs_in_bundle = self.bundle_dir.join("rootfs");
        if let Some(ref rootfs_path) = self.rootfs_path {
            // Remove existing rootfs symlink/dir if present
            let _ = fs::remove_file(&rootfs_in_bundle).await;
            let _ = fs::remove_dir(&rootfs_in_bundle).await;

            // Create symlink to actual rootfs
            tokio::fs::symlink(rootfs_path, &rootfs_in_bundle)
                .await
                .map_err(|e| AgentError::CreateFailed {
                    id: container_id.to_string(),
                    reason: format!(
                        "failed to symlink rootfs from {} to {}: {}",
                        rootfs_path.display(),
                        rootfs_in_bundle.display(),
                        e
                    ),
                })?;
        } else {
            // Create empty rootfs directory (for bind mounts)
            fs::create_dir_all(&rootfs_in_bundle)
                .await
                .map_err(|e| AgentError::CreateFailed {
                    id: container_id.to_string(),
                    reason: format!("failed to create rootfs directory: {}", e),
                })?;
        }

        // Generate OCI runtime spec
        let oci_spec = self
            .build_oci_spec(container_id, spec, &self.volume_paths)
            .await?;

        // Write config.json
        let config_path = self.bundle_dir.join("config.json");
        let config_json =
            serde_json::to_string_pretty(&oci_spec).map_err(|e| AgentError::CreateFailed {
                id: container_id.to_string(),
                reason: format!("failed to serialize OCI spec: {}", e),
            })?;

        fs::write(&config_path, config_json)
            .await
            .map_err(|e| AgentError::CreateFailed {
                id: container_id.to_string(),
                reason: format!("failed to write config.json: {}", e),
            })?;

        tracing::debug!(
            "Created OCI bundle at {} for container {}",
            self.bundle_dir.display(),
            container_id
        );

        Ok(self.bundle_dir.clone())
    }

    /// Build the OCI runtime spec from ServiceSpec
    async fn build_oci_spec(
        &self,
        container_id: &ContainerId,
        spec: &ServiceSpec,
        volume_paths: &std::collections::HashMap<String, PathBuf>,
    ) -> Result<Spec> {
        // Build user: image config user > root (spec doesn't currently have user override)
        let user = {
            let (uid, gid) = if let Some(user_str) = self
                .image_config
                .as_ref()
                .and_then(|c| c.user.as_ref())
                .filter(|u| !u.is_empty())
            {
                // Parse "uid:gid" or "uid" format from image config
                let parts: Vec<&str> = user_str.splitn(2, ':').collect();
                let uid = parts[0].parse::<u32>().unwrap_or(0);
                let gid = if parts.len() > 1 {
                    parts[1].parse::<u32>().unwrap_or(0)
                } else {
                    uid
                };
                (uid, gid)
            } else {
                (0u32, 0u32)
            };

            UserBuilder::default()
                .uid(uid)
                .gid(gid)
                .build()
                .map_err(|e| AgentError::InvalidSpec(format!("failed to build user: {}", e)))?
        };

        // Build environment variables
        // Layer: image config env (base) -> defaults -> spec env -> builder extra env
        let mut env: Vec<String> = Vec::new();
        let mut env_keys: HashSet<String> = HashSet::new();

        // Seed with image config env first (lowest priority)
        if let Some(img_env) = self.image_config.as_ref().and_then(|c| c.env.as_ref()) {
            for entry in img_env {
                if let Some(key) = entry.split('=').next() {
                    env_keys.insert(key.to_string());
                }
                env.push(entry.clone());
            }
        }

        // If image config didn't provide PATH, add the default
        if !env_keys.contains("PATH") {
            env.push(
                "PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".to_string(),
            );
            env_keys.insert("PATH".to_string());
        }

        // Add TERM for interactive compatibility (if not already set)
        if !env_keys.contains("TERM") {
            env.push("TERM=xterm".to_string());
            env_keys.insert("TERM".to_string());
        }

        // Add service-specific env vars, resolving $S: and $E: prefixed references
        // These override image config env for same keys
        //
        // When a secrets provider is available, use the full secrets-aware resolver
        // that handles both $S: (secret) and $E: (env) prefixed values.
        // Otherwise fall back to the env-only resolver.
        if let (Some(secrets_provider), Some(scope)) =
            (&self.secrets_provider, &self.deployment_scope)
        {
            let resolved_map =
                crate::env::resolve_env_with_secrets(&spec.env, secrets_provider.as_ref(), scope)
                    .await
                    .map_err(|e| {
                        AgentError::InvalidSpec(format!(
                            "environment variable resolution failed: {}",
                            e
                        ))
                    })?;

            for (key, value) in &resolved_map {
                if env_keys.contains(key.as_str()) {
                    env.retain(|e| e.split('=').next() != Some(key.as_str()));
                }
                env_keys.insert(key.clone());
                env.push(format!("{}={}", key, value));
            }
        } else {
            let resolved = crate::env::resolve_env_vars_with_warnings(&spec.env).map_err(|e| {
                AgentError::InvalidSpec(format!("environment variable resolution failed: {}", e))
            })?;

            // Log any warnings about resolved env vars
            for warning in &resolved.warnings {
                tracing::warn!(container = %container_id, "{}", warning);
            }

            // Merge spec env: spec values take precedence over image config for same keys
            for var in &resolved.vars {
                if let Some(key) = var.split('=').next() {
                    if env_keys.contains(key) {
                        // Remove the old entry from image config
                        env.retain(|e| e.split('=').next() != Some(key));
                    }
                    env_keys.insert(key.to_string());
                }
                env.push(var.clone());
            }
        }

        // Add extra env vars from builder (highest priority)
        for (key, value) in &self.extra_env {
            if env_keys.contains(key.as_str()) {
                env.retain(|e| e.split('=').next() != Some(key.as_str()));
            }
            env_keys.insert(key.clone());
            env.push(format!("{}={}", key, value));
        }

        // Build capabilities
        let capabilities = self.build_capabilities(spec)?;

        // Determine working directory: builder override > spec.command.workdir > image config > "/"
        let cwd = self
            .cwd
            .clone()
            .or_else(|| spec.command.workdir.clone())
            .or_else(|| {
                self.image_config
                    .as_ref()
                    .and_then(|c| c.working_dir.as_ref())
                    .filter(|w| !w.is_empty())
                    .cloned()
            })
            .unwrap_or_else(|| "/".to_string());

        // Resolve process args: builder override > spec command > image config > /bin/sh
        let process_args = if let Some(ref args) = self.args {
            args.clone()
        } else {
            Self::resolve_command_from_spec(spec, self.image_config.as_ref())
        };

        // Build process
        let mut process_builder = ProcessBuilder::default()
            .terminal(false)
            .user(user)
            .env(env)
            .args(process_args)
            .cwd(cwd)
            .no_new_privileges(!spec.privileged && spec.capabilities.is_empty());

        // Set capabilities if we have them
        if let Some(caps) = capabilities {
            process_builder = process_builder.capabilities(caps);
        }

        let process = process_builder
            .build()
            .map_err(|e| AgentError::InvalidSpec(format!("failed to build process: {}", e)))?;

        // Build root filesystem config
        // Note: "rootfs" is relative to the bundle directory per OCI spec
        let root = RootBuilder::default()
            .path("rootfs".to_string())
            .readonly(false)
            .build()
            .map_err(|e| AgentError::InvalidSpec(format!("failed to build root: {}", e)))?;

        // Build default mounts
        let mut mounts = self.build_default_mounts(spec)?;

        // Add storage mounts from spec
        let storage_mounts = self.build_storage_mounts(spec, volume_paths)?;
        mounts.extend(storage_mounts);

        // Build Linux-specific config
        let linux = self.build_linux_config(spec)?;

        // Determine hostname
        let hostname = self
            .hostname
            .clone()
            .unwrap_or_else(|| container_id.to_string());

        // Build the complete spec
        let oci_spec = SpecBuilder::default()
            .version("1.0.2".to_string())
            .root(root)
            .process(process)
            .hostname(hostname)
            .mounts(mounts)
            .linux(linux)
            .build()
            .map_err(|e| AgentError::InvalidSpec(format!("failed to build OCI spec: {}", e)))?;

        Ok(oci_spec)
    }

    /// Build Linux capabilities configuration
    fn build_capabilities(
        &self,
        spec: &ServiceSpec,
    ) -> Result<Option<oci_spec::runtime::LinuxCapabilities>> {
        if spec.privileged {
            // Privileged mode: all capabilities
            let all_caps: HashSet<Capability> = ALL_CAPABILITIES.iter().copied().collect();
            let empty_caps: HashSet<Capability> = HashSet::new();

            let caps = LinuxCapabilitiesBuilder::default()
                .bounding(all_caps.clone())
                .effective(all_caps.clone())
                .permitted(all_caps)
                .inheritable(empty_caps.clone())
                .ambient(empty_caps)
                .build()
                .map_err(|e| {
                    AgentError::InvalidSpec(format!("failed to build capabilities: {}", e))
                })?;

            Ok(Some(caps))
        } else if !spec.capabilities.is_empty() {
            // Specific capabilities requested
            let caps: HashSet<Capability> = spec
                .capabilities
                .iter()
                .filter_map(|c| {
                    // Normalize capability name (add CAP_ prefix if missing, uppercase)
                    let cap_name = if c.starts_with("CAP_") {
                        c.to_uppercase()
                    } else {
                        format!("CAP_{}", c.to_uppercase())
                    };
                    Capability::from_str(&cap_name).ok()
                })
                .collect();

            let empty_caps: HashSet<Capability> = HashSet::new();

            let built_caps = LinuxCapabilitiesBuilder::default()
                .bounding(caps.clone())
                .effective(caps.clone())
                .permitted(caps)
                .inheritable(empty_caps.clone())
                .ambient(empty_caps)
                .build()
                .map_err(|e| {
                    AgentError::InvalidSpec(format!("failed to build capabilities: {}", e))
                })?;

            Ok(Some(built_caps))
        } else {
            // Default: minimal capabilities for basic container operation
            let default_caps: HashSet<Capability> = [
                Capability::Chown,
                Capability::DacOverride,
                Capability::Fsetid,
                Capability::Fowner,
                Capability::Mknod,
                Capability::NetRaw,
                Capability::Setgid,
                Capability::Setuid,
                Capability::Setfcap,
                Capability::Setpcap,
                Capability::NetBindService,
                Capability::SysChroot,
                Capability::Kill,
                Capability::AuditWrite,
            ]
            .into_iter()
            .collect();

            let empty_caps: HashSet<Capability> = HashSet::new();

            let built_caps = LinuxCapabilitiesBuilder::default()
                .bounding(default_caps.clone())
                .effective(default_caps.clone())
                .permitted(default_caps)
                .inheritable(empty_caps.clone())
                .ambient(empty_caps)
                .build()
                .map_err(|e| {
                    AgentError::InvalidSpec(format!("failed to build capabilities: {}", e))
                })?;

            Ok(Some(built_caps))
        }
    }

    /// Build default filesystem mounts for the container
    fn build_default_mounts(&self, spec: &ServiceSpec) -> Result<Vec<Mount>> {
        let mut mounts = Vec::new();

        // /proc
        mounts.push(
            MountBuilder::default()
                .destination("/proc".to_string())
                .typ("proc".to_string())
                .source("proc".to_string())
                .options(vec![
                    "nosuid".to_string(),
                    "noexec".to_string(),
                    "nodev".to_string(),
                ])
                .build()
                .map_err(|e| {
                    AgentError::InvalidSpec(format!("failed to build /proc mount: {}", e))
                })?,
        );

        // /dev
        mounts.push(
            MountBuilder::default()
                .destination("/dev".to_string())
                .typ("tmpfs".to_string())
                .source("tmpfs".to_string())
                .options(vec![
                    "nosuid".to_string(),
                    "strictatime".to_string(),
                    "mode=755".to_string(),
                    "size=65536k".to_string(),
                ])
                .build()
                .map_err(|e| {
                    AgentError::InvalidSpec(format!("failed to build /dev mount: {}", e))
                })?,
        );

        // /dev/pts
        mounts.push(
            MountBuilder::default()
                .destination("/dev/pts".to_string())
                .typ("devpts".to_string())
                .source("devpts".to_string())
                .options(vec![
                    "nosuid".to_string(),
                    "noexec".to_string(),
                    "newinstance".to_string(),
                    "ptmxmode=0666".to_string(),
                    "mode=0620".to_string(),
                    "gid=5".to_string(),
                ])
                .build()
                .map_err(|e| {
                    AgentError::InvalidSpec(format!("failed to build /dev/pts mount: {}", e))
                })?,
        );

        // /dev/shm
        mounts.push(
            MountBuilder::default()
                .destination("/dev/shm".to_string())
                .typ("tmpfs".to_string())
                .source("shm".to_string())
                .options(vec![
                    "nosuid".to_string(),
                    "noexec".to_string(),
                    "nodev".to_string(),
                    "mode=1777".to_string(),
                    "size=65536k".to_string(),
                ])
                .build()
                .map_err(|e| {
                    AgentError::InvalidSpec(format!("failed to build /dev/shm mount: {}", e))
                })?,
        );

        // /dev/mqueue
        mounts.push(
            MountBuilder::default()
                .destination("/dev/mqueue".to_string())
                .typ("mqueue".to_string())
                .source("mqueue".to_string())
                .options(vec![
                    "nosuid".to_string(),
                    "noexec".to_string(),
                    "nodev".to_string(),
                ])
                .build()
                .map_err(|e| {
                    AgentError::InvalidSpec(format!("failed to build /dev/mqueue mount: {}", e))
                })?,
        );

        // /sys - read-only unless privileged
        let sys_options = if spec.privileged {
            vec![
                "nosuid".to_string(),
                "noexec".to_string(),
                "nodev".to_string(),
            ]
        } else {
            vec![
                "nosuid".to_string(),
                "noexec".to_string(),
                "nodev".to_string(),
                "ro".to_string(),
            ]
        };

        mounts.push(
            MountBuilder::default()
                .destination("/sys".to_string())
                .typ("sysfs".to_string())
                .source("sysfs".to_string())
                .options(sys_options)
                .build()
                .map_err(|e| {
                    AgentError::InvalidSpec(format!("failed to build /sys mount: {}", e))
                })?,
        );

        // /sys/fs/cgroup - for cgroup access
        mounts.push(
            MountBuilder::default()
                .destination("/sys/fs/cgroup".to_string())
                .typ("cgroup2".to_string())
                .source("cgroup".to_string())
                .options(vec![
                    "nosuid".to_string(),
                    "noexec".to_string(),
                    "nodev".to_string(),
                    "relatime".to_string(),
                ])
                .build()
                .map_err(|e| {
                    AgentError::InvalidSpec(format!("failed to build cgroup mount: {}", e))
                })?,
        );

        Ok(mounts)
    }

    /// Build storage mounts from ServiceSpec storage entries
    ///
    /// Converts StorageSpec entries to OCI Mount entries.
    /// Note: Named and Anonymous volumes require StorageManager to prepare paths.
    /// S3 volumes require s3fs FUSE mount (handled separately).
    fn build_storage_mounts(
        &self,
        spec: &ServiceSpec,
        volume_paths: &std::collections::HashMap<String, PathBuf>,
    ) -> Result<Vec<Mount>> {
        let mut mounts = Vec::new();

        for storage in &spec.storage {
            let mount = match storage {
                StorageSpec::Bind {
                    source,
                    target,
                    readonly,
                } => {
                    let mut options = vec!["rbind".to_string()];
                    if *readonly {
                        options.push("ro".to_string());
                    } else {
                        options.push("rw".to_string());
                    }

                    MountBuilder::default()
                        .destination(target.clone())
                        .typ("none".to_string())
                        .source(source.clone())
                        .options(options)
                        .build()
                        .map_err(|e| {
                            AgentError::InvalidSpec(format!(
                                "failed to build bind mount for {}: {}",
                                target, e
                            ))
                        })?
                }

                StorageSpec::Named {
                    name,
                    target,
                    readonly,
                    tier,
                } => {
                    // Get the prepared volume path from StorageManager
                    let source = volume_paths.get(name).ok_or_else(|| {
                        AgentError::InvalidSpec(format!(
                            "volume '{}' not prepared - ensure StorageManager.ensure_volume() was called",
                            name
                        ))
                    })?;

                    // Warn about SQLite safety for non-local tiers
                    if matches!(tier, StorageTier::Network) {
                        tracing::warn!(
                            volume = %name,
                            tier = ?tier,
                            "Network storage tier is NOT SQLite-safe. Avoid using SQLite databases on this volume."
                        );
                    }

                    let mut options = vec!["rbind".to_string()];
                    if *readonly {
                        options.push("ro".to_string());
                    } else {
                        options.push("rw".to_string());
                    }

                    MountBuilder::default()
                        .destination(target.clone())
                        .typ("none".to_string())
                        .source(source.to_string_lossy().to_string())
                        .options(options)
                        .build()
                        .map_err(|e| {
                            AgentError::InvalidSpec(format!(
                                "failed to build named volume mount for {}: {}",
                                target, e
                            ))
                        })?
                }

                StorageSpec::Anonymous { target, tier } => {
                    // Anonymous volumes should have been created by StorageManager
                    // and the path passed in volume_paths with key "_anon_{target}"
                    let key = format!("_anon_{}", target.trim_start_matches('/').replace('/', "_"));
                    let source = volume_paths.get(&key).ok_or_else(|| {
                        AgentError::InvalidSpec(format!(
                            "anonymous volume for '{}' not prepared",
                            target
                        ))
                    })?;

                    if matches!(tier, StorageTier::Network) {
                        tracing::warn!(
                            target = %target,
                            tier = ?tier,
                            "Network storage tier is NOT SQLite-safe."
                        );
                    }

                    let options = vec!["rbind".to_string(), "rw".to_string()];

                    MountBuilder::default()
                        .destination(target.clone())
                        .typ("none".to_string())
                        .source(source.to_string_lossy().to_string())
                        .options(options)
                        .build()
                        .map_err(|e| {
                            AgentError::InvalidSpec(format!(
                                "failed to build anonymous volume mount for {}: {}",
                                target, e
                            ))
                        })?
                }

                StorageSpec::Tmpfs { target, size, mode } => {
                    let mut options = vec!["nosuid".to_string(), "nodev".to_string()];

                    if let Some(size_str) = size {
                        options.push(format!("size={}", size_str));
                    }

                    if let Some(mode_val) = mode {
                        options.push(format!("mode={:o}", mode_val));
                    }

                    MountBuilder::default()
                        .destination(target.clone())
                        .typ("tmpfs".to_string())
                        .source("tmpfs".to_string())
                        .options(options)
                        .build()
                        .map_err(|e| {
                            AgentError::InvalidSpec(format!(
                                "failed to build tmpfs mount for {}: {}",
                                target, e
                            ))
                        })?
                }

                StorageSpec::S3 {
                    bucket,
                    prefix,
                    target,
                    readonly,
                    endpoint: _,
                    credentials: _,
                } => {
                    // S3 mounts are handled via s3fs FUSE
                    // The StorageManager should have mounted the bucket and passed the path
                    let key = format!("_s3_{}_{}", bucket, prefix.as_deref().unwrap_or(""));
                    let source = volume_paths.get(&key).ok_or_else(|| {
                        AgentError::InvalidSpec(format!(
                            "S3 volume for bucket '{}' not mounted - ensure StorageManager.mount_s3() was called",
                            bucket
                        ))
                    })?;

                    tracing::warn!(
                        bucket = %bucket,
                        target = %target,
                        "S3 storage is NOT SQLite-safe. Use for read-heavy workloads only."
                    );

                    let mut options = vec!["rbind".to_string()];
                    if *readonly {
                        options.push("ro".to_string());
                    } else {
                        options.push("rw".to_string());
                    }

                    MountBuilder::default()
                        .destination(target.clone())
                        .typ("none".to_string())
                        .source(source.to_string_lossy().to_string())
                        .options(options)
                        .build()
                        .map_err(|e| {
                            AgentError::InvalidSpec(format!(
                                "failed to build S3 mount for {}: {}",
                                target, e
                            ))
                        })?
                }
            };

            mounts.push(mount);
        }

        Ok(mounts)
    }

    /// Build Linux-specific configuration
    fn build_linux_config(&self, spec: &ServiceSpec) -> Result<oci_spec::runtime::Linux> {
        // Build namespaces
        let mut namespaces = vec![
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
        ];

        // Only add Network namespace when NOT using host networking.
        // In host networking mode, the container shares the host's network stack
        // (like Docker's --network host).
        if !self.host_network {
            namespaces.push(
                LinuxNamespaceBuilder::default()
                    .typ(LinuxNamespaceType::Network)
                    .build()
                    .unwrap(),
            );
        }

        let mut linux_builder = LinuxBuilder::default().namespaces(namespaces);

        // Build resources (CPU, memory, devices)
        let resources = self.build_resources(spec)?;
        if let Some(resources) = resources {
            linux_builder = linux_builder.resources(resources);
        }

        // Build device entries for passthrough
        let devices = self.build_devices(spec)?;
        if !devices.is_empty() {
            linux_builder = linux_builder.devices(devices);
        }

        // Set masked/readonly paths based on privileged mode
        if spec.privileged {
            // Privileged containers get no masked paths (full access)
            linux_builder = linux_builder.masked_paths(vec![]).readonly_paths(vec![]);
        } else {
            // Set masked paths for security (hide sensitive host info)
            let masked_paths = vec![
                "/proc/acpi".to_string(),
                "/proc/asound".to_string(),
                "/proc/kcore".to_string(),
                "/proc/keys".to_string(),
                "/proc/latency_stats".to_string(),
                "/proc/timer_list".to_string(),
                "/proc/timer_stats".to_string(),
                "/proc/sched_debug".to_string(),
                "/proc/scsi".to_string(),
                "/sys/firmware".to_string(),
            ];

            // Set readonly paths for security
            let readonly_paths = vec![
                "/proc/bus".to_string(),
                "/proc/fs".to_string(),
                "/proc/irq".to_string(),
                "/proc/sys".to_string(),
                "/proc/sysrq-trigger".to_string(),
            ];

            linux_builder = linux_builder
                .masked_paths(masked_paths)
                .readonly_paths(readonly_paths);
        }

        linux_builder
            .build()
            .map_err(|e| AgentError::InvalidSpec(format!("failed to build linux config: {}", e)))
    }

    /// Build resource limits (CPU, memory, device cgroups)
    fn build_resources(
        &self,
        spec: &ServiceSpec,
    ) -> Result<Option<oci_spec::runtime::LinuxResources>> {
        let mut resources_builder = LinuxResourcesBuilder::default();
        let mut has_resources = false;

        // CPU limits
        if let Some(cpu_limit) = spec.resources.cpu {
            // Convert CPU cores to microseconds quota
            // 100000 microseconds = 1 core's worth of time per period
            let quota = (cpu_limit * 100_000.0) as i64;
            let cpu = LinuxCpuBuilder::default()
                .quota(quota)
                .period(100_000u64)
                .build()
                .map_err(|e| {
                    AgentError::InvalidSpec(format!("failed to build CPU limits: {}", e))
                })?;

            resources_builder = resources_builder.cpu(cpu);
            has_resources = true;
        }

        // Memory limits
        if let Some(ref memory_str) = spec.resources.memory {
            let bytes = parse_memory_string(memory_str)
                .map_err(|e| AgentError::InvalidSpec(format!("invalid memory limit: {}", e)))?;

            let memory = LinuxMemoryBuilder::default()
                .limit(bytes as i64)
                .build()
                .map_err(|e| {
                    AgentError::InvalidSpec(format!("failed to build memory limits: {}", e))
                })?;

            resources_builder = resources_builder.memory(memory);
            has_resources = true;
        }

        // Device cgroup rules
        let device_rules = self.build_device_cgroup_rules(spec)?;
        if !device_rules.is_empty() {
            resources_builder = resources_builder.devices(device_rules);
            has_resources = true;
        }

        if has_resources {
            let resources = resources_builder.build().map_err(|e| {
                AgentError::InvalidSpec(format!("failed to build resources: {}", e))
            })?;
            Ok(Some(resources))
        } else {
            Ok(None)
        }
    }

    /// Build device cgroup rules
    fn build_device_cgroup_rules(
        &self,
        spec: &ServiceSpec,
    ) -> Result<Vec<oci_spec::runtime::LinuxDeviceCgroup>> {
        let mut rules = Vec::new();

        if spec.privileged {
            // Privileged mode: allow all devices
            let rule = LinuxDeviceCgroupBuilder::default()
                .allow(true)
                .access("rwm".to_string())
                .build()
                .map_err(|e| {
                    AgentError::InvalidSpec(format!("failed to build device cgroup rule: {}", e))
                })?;
            rules.push(rule);
        } else {
            // Default: deny all, then allow specific devices
            let deny_all = LinuxDeviceCgroupBuilder::default()
                .allow(false)
                .access("rwm".to_string())
                .build()
                .map_err(|e| {
                    AgentError::InvalidSpec(format!("failed to build deny rule: {}", e))
                })?;
            rules.push(deny_all);

            // Allow standard container devices
            // /dev/null, /dev/zero, /dev/full, /dev/random, /dev/urandom, /dev/tty
            let standard_char_devices = [
                (1, 3, "rwm"),    // /dev/null
                (1, 5, "rwm"),    // /dev/zero
                (1, 7, "rwm"),    // /dev/full
                (1, 8, "rwm"),    // /dev/random
                (1, 9, "rwm"),    // /dev/urandom
                (5, 0, "rwm"),    // /dev/tty
                (5, 1, "rwm"),    // /dev/console
                (5, 2, "rwm"),    // /dev/ptmx
                (136, -1, "rwm"), // /dev/pts/* (wildcard minor)
            ];

            for (major, minor, access) in standard_char_devices {
                let mut builder = LinuxDeviceCgroupBuilder::default()
                    .allow(true)
                    .typ(LinuxDeviceType::C)
                    .major(major as i64)
                    .access(access.to_string());

                if minor >= 0 {
                    builder = builder.minor(minor as i64);
                }

                let rule = builder.build().map_err(|e| {
                    AgentError::InvalidSpec(format!("failed to build char device rule: {}", e))
                })?;
                rules.push(rule);
            }

            // Allow specific devices from spec
            for device in &spec.devices {
                if let Ok((major, minor)) = get_device_major_minor(&device.path) {
                    let dev_type = get_device_type(&device.path).unwrap_or(LinuxDeviceType::C);

                    // Build access string
                    let mut access = String::new();
                    if device.read {
                        access.push('r');
                    }
                    if device.write {
                        access.push('w');
                    }
                    if device.mknod {
                        access.push('m');
                    }
                    if access.is_empty() {
                        access = "rw".to_string();
                    }

                    let rule = LinuxDeviceCgroupBuilder::default()
                        .allow(true)
                        .typ(dev_type)
                        .major(major)
                        .minor(minor)
                        .access(access)
                        .build()
                        .map_err(|e| {
                            AgentError::InvalidSpec(format!(
                                "failed to build device rule for {}: {}",
                                device.path, e
                            ))
                        })?;
                    rules.push(rule);
                } else {
                    tracing::warn!("Failed to get device info for {}, skipping", device.path);
                }
            }

            // Auto-allow GPU devices in cgroup when gpu spec is set
            if let Some(ref gpu) = spec.resources.gpu {
                match gpu.vendor.as_str() {
                    "nvidia" => {
                        // Allow all nvidia devices (major 195 for nvidia GPUs)
                        let rule = LinuxDeviceCgroupBuilder::default()
                            .allow(true)
                            .typ(LinuxDeviceType::C)
                            .major(195i64)
                            .access("rwm".to_string())
                            .build()
                            .map_err(|e| {
                                AgentError::InvalidSpec(format!(
                                    "failed to build GPU cgroup rule: {}",
                                    e
                                ))
                            })?;
                        rules.push(rule);

                        // nvidia-uvm (major 510 or check dynamically)
                        let uvm_rule = LinuxDeviceCgroupBuilder::default()
                            .allow(true)
                            .typ(LinuxDeviceType::C)
                            .major(510i64)
                            .access("rwm".to_string())
                            .build()
                            .map_err(|e| {
                                AgentError::InvalidSpec(format!(
                                    "failed to build GPU UVM cgroup rule: {}",
                                    e
                                ))
                            })?;
                        rules.push(uvm_rule);
                    }
                    "amd" => {
                        // AMD ROCm: /dev/dri/renderD* and /dev/dri/card* (major 226)
                        let dri_rule = LinuxDeviceCgroupBuilder::default()
                            .allow(true)
                            .typ(LinuxDeviceType::C)
                            .major(226i64)
                            .access("rwm".to_string())
                            .build()
                            .map_err(|e| {
                                AgentError::InvalidSpec(format!(
                                    "failed to build AMD DRI cgroup rule: {}",
                                    e
                                ))
                            })?;
                        rules.push(dri_rule);

                        // /dev/kfd - AMD Kernel Fusion Driver for compute (major 234)
                        let kfd_rule = LinuxDeviceCgroupBuilder::default()
                            .allow(true)
                            .typ(LinuxDeviceType::C)
                            .major(234i64)
                            .access("rwm".to_string())
                            .build()
                            .map_err(|e| {
                                AgentError::InvalidSpec(format!(
                                    "failed to build AMD KFD cgroup rule: {}",
                                    e
                                ))
                            })?;
                        rules.push(kfd_rule);
                    }
                    "intel" => {
                        // Intel GPU: /dev/dri/renderD* and /dev/dri/card* (major 226)
                        let dri_rule = LinuxDeviceCgroupBuilder::default()
                            .allow(true)
                            .typ(LinuxDeviceType::C)
                            .major(226i64)
                            .access("rwm".to_string())
                            .build()
                            .map_err(|e| {
                                AgentError::InvalidSpec(format!(
                                    "failed to build Intel DRI cgroup rule: {}",
                                    e
                                ))
                            })?;
                        rules.push(dri_rule);
                    }
                    other => {
                        // Unknown vendor - allow DRI devices as a reasonable default
                        tracing::warn!(
                            vendor = %other,
                            "Unknown GPU vendor, allowing DRI devices (major 226)"
                        );
                        let dri_rule = LinuxDeviceCgroupBuilder::default()
                            .allow(true)
                            .typ(LinuxDeviceType::C)
                            .major(226i64)
                            .access("rwm".to_string())
                            .build()
                            .map_err(|e| {
                                AgentError::InvalidSpec(format!(
                                    "failed to build GPU DRI cgroup rule: {}",
                                    e
                                ))
                            })?;
                        rules.push(dri_rule);
                    }
                }
            }
        }

        Ok(rules)
    }

    /// Build Linux device entries for passthrough
    fn build_devices(&self, spec: &ServiceSpec) -> Result<Vec<oci_spec::runtime::LinuxDevice>> {
        let mut devices = Vec::new();

        for device in &spec.devices {
            if let Ok((major, minor)) = get_device_major_minor(&device.path) {
                let dev_type = get_device_type(&device.path).unwrap_or(LinuxDeviceType::C);

                let linux_device = LinuxDeviceBuilder::default()
                    .path(device.path.clone())
                    .typ(dev_type)
                    .major(major)
                    .minor(minor)
                    .file_mode(0o666u32)
                    .uid(0u32)
                    .gid(0u32)
                    .build()
                    .map_err(|e| {
                        AgentError::InvalidSpec(format!(
                            "failed to build device {}: {}",
                            device.path, e
                        ))
                    })?;

                devices.push(linux_device);
            }
        }

        // Auto-inject GPU devices when gpu spec is set
        if let Some(ref gpu) = spec.resources.gpu {
            match gpu.vendor.as_str() {
                "nvidia" => {
                    // Always needed: nvidiactl, nvidia-uvm, nvidia-uvm-tools
                    let always_devices =
                        ["/dev/nvidiactl", "/dev/nvidia-uvm", "/dev/nvidia-uvm-tools"];
                    for dev_path in &always_devices {
                        if let Ok((major, minor)) = get_device_major_minor(dev_path) {
                            let dev_type = get_device_type(dev_path).unwrap_or(LinuxDeviceType::C);
                            let linux_device = LinuxDeviceBuilder::default()
                                .path(dev_path.to_string())
                                .typ(dev_type)
                                .major(major)
                                .minor(minor)
                                .file_mode(0o666u32)
                                .uid(0u32)
                                .gid(0u32)
                                .build()
                                .map_err(|e| {
                                    AgentError::InvalidSpec(format!(
                                        "failed to build GPU device {}: {}",
                                        dev_path, e
                                    ))
                                })?;
                            devices.push(linux_device);
                        } else {
                            tracing::warn!("GPU device {} not found on host, skipping", dev_path);
                        }
                    }

                    // Per-GPU devices: /dev/nvidia0, /dev/nvidia1, etc.
                    for i in 0..gpu.count {
                        let dev_path = format!("/dev/nvidia{}", i);
                        if let Ok((major, minor)) = get_device_major_minor(&dev_path) {
                            let dev_type = get_device_type(&dev_path).unwrap_or(LinuxDeviceType::C);
                            let linux_device = LinuxDeviceBuilder::default()
                                .path(dev_path.clone())
                                .typ(dev_type)
                                .major(major)
                                .minor(minor)
                                .file_mode(0o666u32)
                                .uid(0u32)
                                .gid(0u32)
                                .build()
                                .map_err(|e| {
                                    AgentError::InvalidSpec(format!(
                                        "failed to build GPU device {}: {}",
                                        dev_path, e
                                    ))
                                })?;
                            devices.push(linux_device);
                        } else {
                            tracing::warn!("GPU device {} not found on host, skipping", dev_path);
                        }
                    }
                }
                "amd" => {
                    // AMD ROCm: /dev/kfd is always required for compute
                    let amd_always_devices = ["/dev/kfd"];
                    for dev_path in &amd_always_devices {
                        if let Ok((major, minor)) = get_device_major_minor(dev_path) {
                            let dev_type = get_device_type(dev_path).unwrap_or(LinuxDeviceType::C);
                            let linux_device = LinuxDeviceBuilder::default()
                                .path(dev_path.to_string())
                                .typ(dev_type)
                                .major(major)
                                .minor(minor)
                                .file_mode(0o666u32)
                                .uid(0u32)
                                .gid(0u32)
                                .build()
                                .map_err(|e| {
                                    AgentError::InvalidSpec(format!(
                                        "failed to build GPU device {}: {}",
                                        dev_path, e
                                    ))
                                })?;
                            devices.push(linux_device);
                        } else {
                            tracing::warn!("GPU device {} not found on host, skipping", dev_path);
                        }
                    }

                    // DRI render nodes: /dev/dri/renderD128, renderD129, etc.
                    for i in 0..gpu.count {
                        let dev_path = format!("/dev/dri/renderD{}", 128 + i);
                        if let Ok((major, minor)) = get_device_major_minor(&dev_path) {
                            let dev_type = get_device_type(&dev_path).unwrap_or(LinuxDeviceType::C);
                            let linux_device = LinuxDeviceBuilder::default()
                                .path(dev_path.clone())
                                .typ(dev_type)
                                .major(major)
                                .minor(minor)
                                .file_mode(0o666u32)
                                .uid(0u32)
                                .gid(0u32)
                                .build()
                                .map_err(|e| {
                                    AgentError::InvalidSpec(format!(
                                        "failed to build GPU device {}: {}",
                                        dev_path, e
                                    ))
                                })?;
                            devices.push(linux_device);
                        } else {
                            tracing::warn!("GPU device {} not found on host, skipping", dev_path);
                        }
                    }

                    // DRI card nodes: /dev/dri/card0, card1, etc.
                    for i in 0..gpu.count {
                        let dev_path = format!("/dev/dri/card{}", i);
                        if let Ok((major, minor)) = get_device_major_minor(&dev_path) {
                            let dev_type = get_device_type(&dev_path).unwrap_or(LinuxDeviceType::C);
                            let linux_device = LinuxDeviceBuilder::default()
                                .path(dev_path.clone())
                                .typ(dev_type)
                                .major(major)
                                .minor(minor)
                                .file_mode(0o666u32)
                                .uid(0u32)
                                .gid(0u32)
                                .build()
                                .map_err(|e| {
                                    AgentError::InvalidSpec(format!(
                                        "failed to build GPU device {}: {}",
                                        dev_path, e
                                    ))
                                })?;
                            devices.push(linux_device);
                        } else {
                            tracing::warn!("GPU device {} not found on host, skipping", dev_path);
                        }
                    }
                }
                "intel" => {
                    // Intel GPU: DRI render nodes /dev/dri/renderD128, etc.
                    for i in 0..gpu.count {
                        let dev_path = format!("/dev/dri/renderD{}", 128 + i);
                        if let Ok((major, minor)) = get_device_major_minor(&dev_path) {
                            let dev_type = get_device_type(&dev_path).unwrap_or(LinuxDeviceType::C);
                            let linux_device = LinuxDeviceBuilder::default()
                                .path(dev_path.clone())
                                .typ(dev_type)
                                .major(major)
                                .minor(minor)
                                .file_mode(0o666u32)
                                .uid(0u32)
                                .gid(0u32)
                                .build()
                                .map_err(|e| {
                                    AgentError::InvalidSpec(format!(
                                        "failed to build GPU device {}: {}",
                                        dev_path, e
                                    ))
                                })?;
                            devices.push(linux_device);
                        } else {
                            tracing::warn!("GPU device {} not found on host, skipping", dev_path);
                        }
                    }

                    // Intel DRI card nodes: /dev/dri/card0, card1, etc.
                    for i in 0..gpu.count {
                        let dev_path = format!("/dev/dri/card{}", i);
                        if let Ok((major, minor)) = get_device_major_minor(&dev_path) {
                            let dev_type = get_device_type(&dev_path).unwrap_or(LinuxDeviceType::C);
                            let linux_device = LinuxDeviceBuilder::default()
                                .path(dev_path.clone())
                                .typ(dev_type)
                                .major(major)
                                .minor(minor)
                                .file_mode(0o666u32)
                                .uid(0u32)
                                .gid(0u32)
                                .build()
                                .map_err(|e| {
                                    AgentError::InvalidSpec(format!(
                                        "failed to build GPU device {}: {}",
                                        dev_path, e
                                    ))
                                })?;
                            devices.push(linux_device);
                        } else {
                            tracing::warn!("GPU device {} not found on host, skipping", dev_path);
                        }
                    }
                }
                other => {
                    // Unknown vendor - try DRI render nodes as default
                    tracing::warn!(
                        vendor = %other,
                        "Unknown GPU vendor, attempting DRI device passthrough"
                    );
                    for i in 0..gpu.count {
                        let dev_path = format!("/dev/dri/renderD{}", 128 + i);
                        if let Ok((major, minor)) = get_device_major_minor(&dev_path) {
                            let dev_type = get_device_type(&dev_path).unwrap_or(LinuxDeviceType::C);
                            let linux_device = LinuxDeviceBuilder::default()
                                .path(dev_path.clone())
                                .typ(dev_type)
                                .major(major)
                                .minor(minor)
                                .file_mode(0o666u32)
                                .uid(0u32)
                                .gid(0u32)
                                .build()
                                .map_err(|e| {
                                    AgentError::InvalidSpec(format!(
                                        "failed to build GPU device {}: {}",
                                        dev_path, e
                                    ))
                                })?;
                            devices.push(linux_device);
                        } else {
                            tracing::warn!("GPU device {} not found on host, skipping", dev_path);
                        }
                    }
                }
            }
        }

        Ok(devices)
    }

    /// Generate the OCI spec and write config.json to the bundle directory
    ///
    /// Unlike `build()`, this does NOT create the bundle directory or set up rootfs.
    /// Use this when the bundle directory and rootfs already exist (e.g., rootfs was
    /// extracted directly by LayerUnpacker).
    ///
    /// # Returns
    /// The path to the bundle directory on success
    pub async fn write_config(
        &self,
        container_id: &ContainerId,
        spec: &ServiceSpec,
    ) -> Result<PathBuf> {
        // Generate OCI runtime spec
        let oci_spec = self
            .build_oci_spec(container_id, spec, &self.volume_paths)
            .await?;

        // Write config.json
        let config_path = self.bundle_dir.join("config.json");
        let config_json =
            serde_json::to_string_pretty(&oci_spec).map_err(|e| AgentError::CreateFailed {
                id: container_id.to_string(),
                reason: format!("failed to serialize OCI spec: {}", e),
            })?;

        fs::write(&config_path, config_json)
            .await
            .map_err(|e| AgentError::CreateFailed {
                id: container_id.to_string(),
                reason: format!("failed to write config.json: {}", e),
            })?;

        tracing::debug!(
            "Wrote OCI config.json at {} for container {}",
            config_path.display(),
            container_id
        );

        Ok(self.bundle_dir.clone())
    }

    /// Resolve command from ServiceSpec and optional image config following Docker/OCI semantics
    ///
    /// Resolution order:
    /// 1. spec entrypoint + args -> use those
    /// 2. spec entrypoint only -> use entrypoint
    /// 3. spec args only -> use args
    /// 4. image_config entrypoint/cmd -> use image_config.full_command()
    /// 5. fallback to /bin/sh
    fn resolve_command_from_spec(
        spec: &ServiceSpec,
        image_config: Option<&zlayer_registry::ImageConfig>,
    ) -> Vec<String> {
        let mut args = Vec::new();

        match (&spec.command.entrypoint, &spec.command.args) {
            (Some(entrypoint), Some(cmd_args)) => {
                args.extend_from_slice(entrypoint);
                args.extend_from_slice(cmd_args);
            }
            (Some(entrypoint), None) => {
                args.extend_from_slice(entrypoint);
            }
            (None, Some(cmd_args)) if !cmd_args.is_empty() => {
                args.extend_from_slice(cmd_args);
            }
            _ => {
                // No spec command - try image config
                if let Some(img_cmd) = image_config.and_then(|c| c.full_command()) {
                    if !img_cmd.is_empty() {
                        args.extend(img_cmd);
                    } else {
                        args.push("/bin/sh".to_string());
                    }
                } else {
                    args.push("/bin/sh".to_string());
                }
            }
        }

        args
    }

    /// Clean up a bundle directory
    ///
    /// Removes the bundle directory and all its contents
    pub async fn cleanup(&self) -> Result<()> {
        if self.bundle_dir.exists() {
            fs::remove_dir_all(&self.bundle_dir)
                .await
                .map_err(|e| AgentError::CreateFailed {
                    id: "cleanup".to_string(),
                    reason: format!(
                        "failed to remove bundle directory {}: {}",
                        self.bundle_dir.display(),
                        e
                    ),
                })?;
        }
        Ok(())
    }
}

/// Create a bundle for a container
///
/// Convenience function that creates a bundle in the default location
pub async fn create_bundle(
    container_id: &ContainerId,
    spec: &ServiceSpec,
    rootfs_path: Option<PathBuf>,
) -> Result<PathBuf> {
    let mut builder =
        BundleBuilder::for_container(container_id).with_host_network(spec.host_network);

    if let Some(rootfs) = rootfs_path {
        builder = builder.with_rootfs(rootfs);
    }

    builder.build(container_id, spec).await
}

/// Clean up a container's bundle
///
/// Convenience function to remove a bundle from the default location
pub async fn cleanup_bundle(container_id: &ContainerId) -> Result<()> {
    let builder = BundleBuilder::for_container(container_id);
    builder.cleanup().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use zlayer_spec::*;

    fn mock_spec() -> ServiceSpec {
        serde_yaml::from_str::<DeploymentSpec>(
            r#"
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
"#,
        )
        .unwrap()
        .services
        .remove("test")
        .unwrap()
    }

    fn mock_spec_with_resources() -> ServiceSpec {
        serde_yaml::from_str::<DeploymentSpec>(
            r#"
version: v1
deployment: test
services:
  test:
    rtype: service
    image:
      name: test:latest
    resources:
      cpu: 0.5
      memory: 512Mi
    env:
      MY_VAR: my_value
      ANOTHER: value2
    endpoints:
      - name: http
        protocol: http
        port: 8080
"#,
        )
        .unwrap()
        .services
        .remove("test")
        .unwrap()
    }

    fn mock_privileged_spec() -> ServiceSpec {
        serde_yaml::from_str::<DeploymentSpec>(
            r#"
version: v1
deployment: test
services:
  test:
    rtype: service
    image:
      name: test:latest
    privileged: true
    endpoints:
      - name: http
        protocol: http
        port: 8080
"#,
        )
        .unwrap()
        .services
        .remove("test")
        .unwrap()
    }

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
    fn test_bundle_builder_new() {
        let builder = BundleBuilder::new("/tmp/test-bundle".into());
        assert_eq!(builder.bundle_dir(), Path::new("/tmp/test-bundle"));
        assert!(builder.rootfs_path.is_none());
    }

    #[test]
    fn test_bundle_builder_for_container() {
        let id = ContainerId {
            service: "myservice".to_string(),
            replica: 1,
        };
        let builder = BundleBuilder::for_container(&id);
        assert_eq!(
            builder.bundle_dir(),
            Path::new("/var/lib/zlayer/bundles/myservice-rep-1")
        );
    }

    #[test]
    fn test_bundle_builder_with_rootfs() {
        let builder = BundleBuilder::new("/tmp/test-bundle".into())
            .with_rootfs("/var/lib/zlayer/rootfs/myimage".into());
        assert_eq!(
            builder.rootfs_path,
            Some(PathBuf::from("/var/lib/zlayer/rootfs/myimage"))
        );
    }

    #[tokio::test]
    async fn test_build_oci_spec_basic() {
        let id = ContainerId {
            service: "test".to_string(),
            replica: 1,
        };
        let spec = mock_spec();
        let builder = BundleBuilder::new("/tmp/test-bundle".into());

        let oci_spec = builder
            .build_oci_spec(&id, &spec, &std::collections::HashMap::new())
            .await
            .unwrap();

        assert_eq!(oci_spec.version(), "1.0.2");
        assert!(oci_spec.root().is_some());
        assert_eq!(
            oci_spec.root().as_ref().unwrap().path(),
            std::path::Path::new("rootfs")
        );
        assert!(oci_spec.process().is_some());
        assert!(oci_spec.linux().is_some());
    }

    #[tokio::test]
    async fn test_build_oci_spec_with_resources() {
        let id = ContainerId {
            service: "test".to_string(),
            replica: 1,
        };
        let spec = mock_spec_with_resources();
        let builder = BundleBuilder::new("/tmp/test-bundle".into());

        let oci_spec = builder
            .build_oci_spec(&id, &spec, &std::collections::HashMap::new())
            .await
            .unwrap();

        // Check that resources are set
        let linux = oci_spec.linux().as_ref().unwrap();
        let resources = linux.resources().as_ref().unwrap();

        // Check CPU
        let cpu = resources.cpu().as_ref().unwrap();
        assert_eq!(cpu.quota(), Some(50_000)); // 0.5 cores * 100000
        assert_eq!(cpu.period(), Some(100_000));

        // Check memory
        let memory = resources.memory().as_ref().unwrap();
        assert_eq!(memory.limit(), Some(512 * 1024 * 1024)); // 512Mi
    }

    #[tokio::test]
    async fn test_build_oci_spec_privileged() {
        let id = ContainerId {
            service: "test".to_string(),
            replica: 1,
        };
        let spec = mock_privileged_spec();
        let builder = BundleBuilder::new("/tmp/test-bundle".into());

        let oci_spec = builder
            .build_oci_spec(&id, &spec, &std::collections::HashMap::new())
            .await
            .unwrap();

        // Check that all capabilities are set
        let process = oci_spec.process().as_ref().unwrap();
        let caps = process.capabilities().as_ref().unwrap();
        let bounding = caps.bounding().as_ref().unwrap();

        // Should have all capabilities
        assert!(bounding.contains(&Capability::SysAdmin));
        assert!(bounding.contains(&Capability::NetAdmin));

        // Check that masked paths are NOT set for privileged
        let linux = oci_spec.linux().as_ref().unwrap();
        assert!(
            linux.masked_paths().is_none() || linux.masked_paths().as_ref().unwrap().is_empty()
        );
    }

    #[tokio::test]
    async fn test_build_oci_spec_environment() {
        let id = ContainerId {
            service: "test".to_string(),
            replica: 1,
        };
        let spec = mock_spec_with_resources();
        let builder = BundleBuilder::new("/tmp/test-bundle".into())
            .with_env("EXTRA_VAR".to_string(), "extra_value".to_string());

        let oci_spec = builder
            .build_oci_spec(&id, &spec, &std::collections::HashMap::new())
            .await
            .unwrap();

        let process = oci_spec.process().as_ref().unwrap();
        let env = process.env().as_ref().unwrap();

        // Check service env vars are present
        assert!(env.iter().any(|e| e == "MY_VAR=my_value"));
        assert!(env.iter().any(|e| e == "ANOTHER=value2"));
        // Check extra env var is present
        assert!(env.iter().any(|e| e == "EXTRA_VAR=extra_value"));
        // Check PATH is present
        assert!(env.iter().any(|e| e.starts_with("PATH=")));
    }

    #[tokio::test]
    async fn test_build_namespaces() {
        let id = ContainerId {
            service: "test".to_string(),
            replica: 1,
        };
        let spec = mock_spec();
        let builder = BundleBuilder::new("/tmp/test-bundle".into());

        let oci_spec = builder
            .build_oci_spec(&id, &spec, &std::collections::HashMap::new())
            .await
            .unwrap();
        let linux = oci_spec.linux().as_ref().unwrap();
        let namespaces = linux.namespaces().as_ref().unwrap();

        // Check we have the expected namespaces
        let namespace_types: Vec<_> = namespaces.iter().map(|ns| ns.typ()).collect();
        assert!(namespace_types.contains(&LinuxNamespaceType::Pid));
        assert!(namespace_types.contains(&LinuxNamespaceType::Ipc));
        assert!(namespace_types.contains(&LinuxNamespaceType::Uts));
        assert!(namespace_types.contains(&LinuxNamespaceType::Mount));
        assert!(namespace_types.contains(&LinuxNamespaceType::Network));
    }

    #[tokio::test]
    async fn test_build_namespaces_host_network() {
        let id = ContainerId {
            service: "test".to_string(),
            replica: 1,
        };
        let spec = mock_spec();
        let builder = BundleBuilder::new("/tmp/test-bundle".into()).with_host_network(true);

        let oci_spec = builder
            .build_oci_spec(&id, &spec, &std::collections::HashMap::new())
            .await
            .unwrap();
        let linux = oci_spec.linux().as_ref().unwrap();
        let namespaces = linux.namespaces().as_ref().unwrap();

        // Check we have the expected namespaces (NO Network namespace)
        let namespace_types: Vec<_> = namespaces.iter().map(|ns| ns.typ()).collect();
        assert!(namespace_types.contains(&LinuxNamespaceType::Pid));
        assert!(namespace_types.contains(&LinuxNamespaceType::Ipc));
        assert!(namespace_types.contains(&LinuxNamespaceType::Uts));
        assert!(namespace_types.contains(&LinuxNamespaceType::Mount));
        assert!(
            !namespace_types.contains(&LinuxNamespaceType::Network),
            "Network namespace should NOT be present in host_network mode"
        );
    }

    #[test]
    fn test_build_default_mounts() {
        let spec = mock_spec();
        let builder = BundleBuilder::new("/tmp/test-bundle".into());

        let mounts = builder.build_default_mounts(&spec).unwrap();

        // Check we have the expected mounts
        let mount_destinations: Vec<_> = mounts
            .iter()
            .map(|m| m.destination().to_string_lossy().to_string())
            .collect();
        assert!(mount_destinations.contains(&"/proc".to_string()));
        assert!(mount_destinations.contains(&"/dev".to_string()));
        assert!(mount_destinations.contains(&"/dev/pts".to_string()));
        assert!(mount_destinations.contains(&"/dev/shm".to_string()));
        assert!(mount_destinations.contains(&"/sys".to_string()));
    }

    #[test]
    fn test_build_storage_mounts_bind() {
        let spec = serde_yaml::from_str::<zlayer_spec::DeploymentSpec>(
            r#"
version: v1
deployment: test
services:
  test:
    image:
      name: test:latest
    storage:
      - type: bind
        source: /host/data
        target: /app/data
        readonly: true
"#,
        )
        .unwrap()
        .services
        .remove("test")
        .unwrap();

        let builder = BundleBuilder::new("/tmp/test-bundle".into());
        let volume_paths = std::collections::HashMap::new();

        let mounts = builder.build_storage_mounts(&spec, &volume_paths).unwrap();

        assert_eq!(mounts.len(), 1);
        assert_eq!(mounts[0].destination().to_string_lossy(), "/app/data");
        assert_eq!(
            mounts[0]
                .source()
                .as_ref()
                .map(|s| s.to_string_lossy().to_string()),
            Some("/host/data".to_string())
        );
        let options = mounts[0].options().as_ref().unwrap();
        assert!(options.contains(&"rbind".to_string()));
        assert!(options.contains(&"ro".to_string()));
    }

    #[test]
    fn test_build_storage_mounts_named() {
        let spec = serde_yaml::from_str::<zlayer_spec::DeploymentSpec>(
            r#"
version: v1
deployment: test
services:
  test:
    image:
      name: test:latest
    storage:
      - type: named
        name: my-volume
        target: /app/data
"#,
        )
        .unwrap()
        .services
        .remove("test")
        .unwrap();

        let builder = BundleBuilder::new("/tmp/test-bundle".into());
        let mut volume_paths = std::collections::HashMap::new();
        volume_paths.insert(
            "my-volume".to_string(),
            PathBuf::from("/var/lib/zlayer/volumes/my-volume"),
        );

        let mounts = builder.build_storage_mounts(&spec, &volume_paths).unwrap();

        assert_eq!(mounts.len(), 1);
        assert_eq!(mounts[0].destination().to_string_lossy(), "/app/data");
        assert_eq!(
            mounts[0]
                .source()
                .as_ref()
                .map(|s| s.to_string_lossy().to_string()),
            Some("/var/lib/zlayer/volumes/my-volume".to_string())
        );
    }

    #[test]
    fn test_build_storage_mounts_tmpfs() {
        let spec = serde_yaml::from_str::<zlayer_spec::DeploymentSpec>(
            r#"
version: v1
deployment: test
services:
  test:
    image:
      name: test:latest
    storage:
      - type: tmpfs
        target: /app/tmp
        size: 256Mi
        mode: 1777
"#,
        )
        .unwrap()
        .services
        .remove("test")
        .unwrap();

        let builder = BundleBuilder::new("/tmp/test-bundle".into());
        let volume_paths = std::collections::HashMap::new();

        let mounts = builder.build_storage_mounts(&spec, &volume_paths).unwrap();

        assert_eq!(mounts.len(), 1);
        assert_eq!(mounts[0].destination().to_string_lossy(), "/app/tmp");
        assert_eq!(mounts[0].typ().as_ref().map(|s| s.as_str()), Some("tmpfs"));
        let options = mounts[0].options().as_ref().unwrap();
        assert!(options.iter().any(|o| o.starts_with("size=")));
        assert!(options.iter().any(|o| o.starts_with("mode=")));
    }

    #[test]
    fn test_build_storage_mounts_multiple() {
        let spec = serde_yaml::from_str::<zlayer_spec::DeploymentSpec>(
            r#"
version: v1
deployment: test
services:
  test:
    image:
      name: test:latest
    storage:
      - type: bind
        source: /etc/config
        target: /app/config
        readonly: true
      - type: named
        name: app-data
        target: /app/data
      - type: tmpfs
        target: /app/tmp
"#,
        )
        .unwrap()
        .services
        .remove("test")
        .unwrap();

        let builder = BundleBuilder::new("/tmp/test-bundle".into());
        let mut volume_paths = std::collections::HashMap::new();
        volume_paths.insert(
            "app-data".to_string(),
            PathBuf::from("/var/lib/zlayer/volumes/app-data"),
        );

        let mounts = builder.build_storage_mounts(&spec, &volume_paths).unwrap();

        assert_eq!(mounts.len(), 3);

        // Verify each mount is correct type
        let destinations: Vec<String> = mounts
            .iter()
            .map(|m| m.destination().to_string_lossy().to_string())
            .collect();
        assert!(destinations.contains(&"/app/config".to_string()));
        assert!(destinations.contains(&"/app/data".to_string()));
        assert!(destinations.contains(&"/app/tmp".to_string()));
    }

    #[test]
    fn test_build_storage_mounts_anonymous_missing_path() {
        let spec = serde_yaml::from_str::<zlayer_spec::DeploymentSpec>(
            r#"
version: v1
deployment: test
services:
  test:
    image:
      name: test:latest
    storage:
      - type: anonymous
        target: /app/cache
"#,
        )
        .unwrap()
        .services
        .remove("test")
        .unwrap();

        let builder = BundleBuilder::new("/tmp/test-bundle".into());
        let volume_paths = std::collections::HashMap::new(); // No path provided

        let result = builder.build_storage_mounts(&spec, &volume_paths);

        // Should fail because anonymous volume path not prepared
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_oci_spec_includes_storage_mounts() {
        let id = ContainerId {
            service: "test".to_string(),
            replica: 1,
        };
        let spec = serde_yaml::from_str::<zlayer_spec::DeploymentSpec>(
            r#"
version: v1
deployment: test
services:
  test:
    image:
      name: test:latest
    storage:
      - type: bind
        source: /host/data
        target: /app/data
      - type: tmpfs
        target: /app/tmp
"#,
        )
        .unwrap()
        .services
        .remove("test")
        .unwrap();

        let builder = BundleBuilder::new("/tmp/test-bundle".into());
        let volume_paths = std::collections::HashMap::new();

        let oci_spec = builder
            .build_oci_spec(&id, &spec, &volume_paths)
            .await
            .unwrap();

        // Verify the OCI spec includes storage mounts
        let mounts = oci_spec.mounts().as_ref().unwrap();
        let destinations: Vec<String> = mounts
            .iter()
            .map(|m| m.destination().to_string_lossy().to_string())
            .collect();

        // Should include both default mounts and storage mounts
        assert!(destinations.contains(&"/proc".to_string())); // default
        assert!(destinations.contains(&"/dev".to_string())); // default
        assert!(destinations.contains(&"/app/data".to_string())); // storage bind
        assert!(destinations.contains(&"/app/tmp".to_string())); // storage tmpfs
    }
}
