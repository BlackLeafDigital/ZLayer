//! Docker Compose specification types for serde deserialization.
//!
//! Handles all common docker-compose.yaml features including the many format
//! variants Docker Compose supports (string vs object, list vs map, etc.).

use std::collections::HashMap;
use std::fmt;

use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};

use crate::DockerError;

// ---------------------------------------------------------------------------
// Top-level ComposeFile
// ---------------------------------------------------------------------------

/// Top-level docker-compose.yaml structure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComposeFile {
    /// Compose spec version (optional in modern compose).
    #[serde(default)]
    pub version: Option<String>,

    /// Optional top-level `name` for the project.
    #[serde(default)]
    pub name: Option<String>,

    /// Service definitions.
    #[serde(default)]
    pub services: HashMap<String, ComposeService>,

    /// Top-level named volumes.
    #[serde(default)]
    pub volumes: HashMap<String, Option<ComposeVolumeConfig>>,

    /// Top-level named networks.
    #[serde(default)]
    pub networks: HashMap<String, Option<ComposeNetworkConfig>>,

    /// Secrets.
    #[serde(default)]
    pub secrets: HashMap<String, ComposeSecretConfig>,

    /// Configs.
    #[serde(default)]
    pub configs: HashMap<String, ComposeConfigDef>,

    /// Extension fields (x-*) and any unknown top-level keys.
    #[serde(flatten)]
    pub extensions: HashMap<String, serde_yaml::Value>,
}

// ---------------------------------------------------------------------------
// ComposeService
// ---------------------------------------------------------------------------

/// A service definition in docker-compose.yaml.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(clippy::struct_excessive_bools)]
pub struct ComposeService {
    #[serde(default)]
    pub image: Option<String>,

    #[serde(default)]
    pub build: Option<ComposeBuild>,

    /// Ports: supports both short (`"8080:80"`) and long syntax.
    #[serde(default, deserialize_with = "deserialize_ports")]
    pub ports: Vec<ComposePort>,

    /// Volumes: supports both short (`"./data:/app/data"`) and long syntax.
    #[serde(default, deserialize_with = "deserialize_volumes")]
    pub volumes: Vec<ComposeVolume>,

    /// Environment variables: supports both map and list (`"FOO=bar"`) format.
    #[serde(default, deserialize_with = "deserialize_env")]
    pub environment: HashMap<String, String>,

    #[serde(default)]
    pub env_file: Option<EnvFile>,

    /// `depends_on`: supports both simple list `["db"]` and map with conditions.
    #[serde(default, deserialize_with = "deserialize_depends_on")]
    pub depends_on: HashMap<String, ComposeDependsOn>,

    #[serde(default)]
    pub healthcheck: Option<ComposeHealthcheck>,

    /// `command`: supports both string `"echo hello"` and list `["echo", "hello"]`.
    #[serde(default, deserialize_with = "deserialize_string_or_list")]
    pub command: Vec<String>,

    /// `entrypoint`: supports both string and list.
    #[serde(default, deserialize_with = "deserialize_string_or_list")]
    pub entrypoint: Vec<String>,

    #[serde(default)]
    pub working_dir: Option<String>,

    #[serde(default)]
    pub user: Option<String>,

    #[serde(default)]
    pub hostname: Option<String>,

    #[serde(default)]
    pub domainname: Option<String>,

    #[serde(default)]
    pub restart: Option<String>,

    #[serde(default)]
    pub deploy: Option<ComposeDeploy>,

    /// Labels as a map. Also accepts list format via custom deserializer.
    #[serde(default, deserialize_with = "deserialize_labels")]
    pub labels: HashMap<String, String>,

    #[serde(default)]
    pub networks: Option<ComposeServiceNetworks>,

    #[serde(default)]
    pub cap_add: Vec<String>,

    #[serde(default)]
    pub cap_drop: Vec<String>,

    #[serde(default)]
    pub privileged: bool,

    #[serde(default)]
    pub init: Option<bool>,

    /// Expose ports (container-only, no host mapping).
    #[serde(default)]
    pub expose: Vec<StringOrNumber>,

    #[serde(default)]
    pub extra_hosts: Vec<String>,

    #[serde(default)]
    pub dns: Option<StringOrList>,

    #[serde(default)]
    pub dns_search: Option<StringOrList>,

    #[serde(default)]
    pub tmpfs: Option<StringOrList>,

    #[serde(default, deserialize_with = "deserialize_sysctls")]
    pub sysctls: HashMap<String, String>,

    #[serde(default)]
    pub ulimits: HashMap<String, ComposeUlimit>,

    #[serde(default)]
    pub logging: Option<ComposeLogging>,

    #[serde(default)]
    pub profiles: Vec<String>,

    #[serde(default, deserialize_with = "deserialize_service_secrets")]
    pub secrets: Vec<ComposeServiceSecret>,

    #[serde(default, deserialize_with = "deserialize_service_configs")]
    pub configs: Vec<ComposeServiceConfig>,

    #[serde(default)]
    pub container_name: Option<String>,

    #[serde(default)]
    pub stop_grace_period: Option<String>,

    #[serde(default)]
    pub stop_signal: Option<String>,

    #[serde(default)]
    pub stdin_open: bool,

    #[serde(default)]
    pub tty: bool,

    #[serde(default)]
    pub platform: Option<String>,

    #[serde(default)]
    pub pull_policy: Option<String>,

    #[serde(default)]
    pub read_only: bool,

    #[serde(default)]
    pub shm_size: Option<StringOrNumber>,

    #[serde(default)]
    pub network_mode: Option<String>,

    #[serde(default)]
    pub pid: Option<String>,

    #[serde(default)]
    pub ipc: Option<String>,

    #[serde(default)]
    pub security_opt: Vec<String>,

    #[serde(default)]
    pub devices: Vec<String>,

    #[serde(default)]
    pub cgroup_parent: Option<String>,

    #[serde(default)]
    pub scale: Option<u64>,

    /// Catch-all for any unknown / extension fields.
    #[serde(flatten)]
    pub extensions: HashMap<String, serde_yaml::Value>,
}

// ---------------------------------------------------------------------------
// ComposeBuild  (string OR object)
// ---------------------------------------------------------------------------

/// Build configuration. Accepts either a plain string (context path) or a full
/// build object.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ComposeBuild {
    /// Short form: just a context path string.
    Simple(String),
    /// Long form: full build configuration object.
    Full(Box<ComposeBuildConfig>),
}

/// Full build configuration object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComposeBuildConfig {
    /// Build context path.
    #[serde(default)]
    pub context: Option<String>,

    /// Dockerfile path (relative to context).
    #[serde(default)]
    pub dockerfile: Option<String>,

    /// Inline Dockerfile content.
    #[serde(default)]
    pub dockerfile_inline: Option<String>,

    /// Build arguments.
    #[serde(default, deserialize_with = "deserialize_env")]
    pub args: HashMap<String, String>,

    /// Target build stage.
    #[serde(default)]
    pub target: Option<String>,

    /// Cache-from images.
    #[serde(default)]
    pub cache_from: Vec<String>,

    /// Cache-to destinations.
    #[serde(default)]
    pub cache_to: Vec<String>,

    /// Labels to set on the built image.
    #[serde(default, deserialize_with = "deserialize_labels")]
    pub labels: HashMap<String, String>,

    /// Shared memory size for build containers.
    #[serde(default)]
    pub shm_size: Option<StringOrNumber>,

    /// Network mode during build.
    #[serde(default)]
    pub network: Option<String>,

    /// Extra hosts to add during build.
    #[serde(default)]
    pub extra_hosts: Vec<String>,

    /// SSH agent forwarding.
    #[serde(default)]
    pub ssh: Option<serde_yaml::Value>,

    /// Build secrets.
    #[serde(default)]
    pub secrets: Option<serde_yaml::Value>,

    /// Platform(s) to build for.
    #[serde(default)]
    pub platforms: Vec<String>,

    /// Build-time tags.
    #[serde(default)]
    pub tags: Vec<String>,

    /// Extension fields.
    #[serde(flatten)]
    pub extensions: HashMap<String, serde_yaml::Value>,
}

impl ComposeBuild {
    /// Get the build context path regardless of variant.
    #[must_use]
    pub fn context(&self) -> Option<&str> {
        match self {
            ComposeBuild::Simple(s) => Some(s.as_str()),
            ComposeBuild::Full(cfg) => cfg.context.as_deref(),
        }
    }

    /// Get the dockerfile path if specified.
    #[must_use]
    pub fn dockerfile(&self) -> Option<&str> {
        match self {
            ComposeBuild::Simple(_) => None,
            ComposeBuild::Full(cfg) => cfg.dockerfile.as_deref(),
        }
    }
}

// ---------------------------------------------------------------------------
// ComposePort
// ---------------------------------------------------------------------------

/// A parsed port mapping.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComposePort {
    /// Host IP to bind (e.g. `127.0.0.1`).
    #[serde(default)]
    pub host_ip: Option<String>,

    /// Host port or range.
    #[serde(default)]
    pub host_port: Option<String>,

    /// Container port or range (required).
    pub container_port: String,

    /// Protocol (`tcp` or `udp`).
    #[serde(default)]
    pub protocol: Option<String>,
}

impl ComposePort {
    /// Parse a short-syntax port string.
    ///
    /// Supported forms:
    /// - `"80"` (container port only)
    /// - `"8080:80"` (host:container)
    /// - `"8080:80/tcp"` (with protocol)
    /// - `"127.0.0.1:8080:80"` (with host IP)
    /// - `"127.0.0.1:8080:80/tcp"` (full)
    /// - `"9090-9091:8080-8081"` (ranges)
    /// - `"127.0.0.1::80"` (host IP, random host port)
    ///
    /// # Errors
    ///
    /// Returns `DockerError::ComposeParse` if the port string is empty,
    /// malformed, or contains an invalid format.
    pub fn parse(s: &str) -> Result<Self, DockerError> {
        let s = s.trim();
        if s.is_empty() {
            return Err(DockerError::ComposeParse(
                "empty port specification".to_string(),
            ));
        }

        // Split off protocol suffix.
        let (main, protocol) = if let Some(idx) = s.rfind('/') {
            let proto = &s[idx + 1..];
            match proto {
                "tcp" | "udp" | "sctp" => (s[..idx].to_string(), Some(proto.to_string())),
                _ => (s.to_string(), None),
            }
        } else {
            (s.to_string(), None)
        };

        // Count colons to decide the format.
        let parts: Vec<&str> = main.split(':').collect();

        let (host_ip, host_port, container_port) = match parts.len() {
            1 => {
                // "80" or "8080-8081"
                (None, None, parts[0].to_string())
            }
            2 => {
                // "8080:80" or "8080-8081:80-81"
                let host = if parts[0].is_empty() {
                    None
                } else {
                    Some(parts[0].to_string())
                };
                (None, host, parts[1].to_string())
            }
            3 => {
                // "127.0.0.1:8080:80" or "127.0.0.1::80"
                let ip = if parts[0].is_empty() {
                    None
                } else {
                    Some(parts[0].to_string())
                };
                let host = if parts[1].is_empty() {
                    None
                } else {
                    Some(parts[1].to_string())
                };
                (ip, host, parts[2].to_string())
            }
            _ => {
                // Could be IPv6: check if the string contains brackets.
                if main.contains('[') {
                    Self::parse_ipv6_port(&main)?;
                    // Fallthrough handled by the helper.
                    return Self::parse_ipv6_port(&main);
                }
                return Err(DockerError::ComposeParse(format!(
                    "invalid port specification: {s}"
                )));
            }
        };

        if container_port.is_empty() {
            return Err(DockerError::ComposeParse(format!(
                "missing container port in: {s}"
            )));
        }

        Ok(ComposePort {
            host_ip,
            host_port,
            container_port,
            protocol,
        })
    }

    /// Parse IPv6 bracket-notation ports like `[::1]:8080:80`.
    fn parse_ipv6_port(s: &str) -> Result<Self, DockerError> {
        // Expected: [ip]:host_port:container_port
        let bracket_end = s
            .find(']')
            .ok_or_else(|| DockerError::ComposeParse(format!("unclosed bracket in: {s}")))?;
        let ip = s[1..bracket_end].to_string();
        let rest = &s[bracket_end + 1..];

        // rest starts with ':'
        let rest = rest
            .strip_prefix(':')
            .ok_or_else(|| DockerError::ComposeParse(format!("invalid IPv6 port: {s}")))?;

        let colon = rest.find(':');
        let (host_port, container_port) = match colon {
            Some(idx) => {
                let hp = &rest[..idx];
                let cp = &rest[idx + 1..];
                (
                    if hp.is_empty() {
                        None
                    } else {
                        Some(hp.to_string())
                    },
                    cp.to_string(),
                )
            }
            None => (None, rest.to_string()),
        };

        Ok(ComposePort {
            host_ip: Some(ip),
            host_port,
            container_port,
            protocol: None,
        })
    }
}

// ---------------------------------------------------------------------------
// ComposeVolume
// ---------------------------------------------------------------------------

/// Volume type.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VolumeType {
    #[default]
    Volume,
    Bind,
    Tmpfs,
    Npipe,
    Cluster,
}

/// A parsed volume mount.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComposeVolume {
    /// Volume type.
    #[serde(default, rename = "type")]
    pub volume_type: Option<VolumeType>,

    /// Source path or volume name.
    #[serde(default)]
    pub source: Option<String>,

    /// Mount target inside the container.
    #[serde(default)]
    pub target: Option<String>,

    /// Read-only mount.
    #[serde(default)]
    pub read_only: bool,

    /// Bind-propagation option.
    #[serde(default)]
    pub bind: Option<ComposeVolumeBind>,

    /// Volume-specific options.
    #[serde(default)]
    pub volume: Option<ComposeVolumeVolume>,

    /// Tmpfs-specific options.
    #[serde(default)]
    pub tmpfs: Option<ComposeVolumeTmpfs>,

    /// Consistency setting (cached, consistent, delegated).
    #[serde(default)]
    pub consistency: Option<String>,
}

/// Bind-mount specific options.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComposeVolumeBind {
    #[serde(default)]
    pub propagation: Option<String>,
    #[serde(default)]
    pub create_host_path: Option<bool>,
    #[serde(default)]
    pub selinux: Option<String>,
}

/// Volume-specific options in long syntax.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComposeVolumeVolume {
    #[serde(default)]
    pub nocopy: Option<bool>,
}

/// Tmpfs-specific options.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComposeVolumeTmpfs {
    #[serde(default)]
    pub size: Option<StringOrNumber>,
    #[serde(default)]
    pub mode: Option<u32>,
}

impl ComposeVolume {
    /// Parse a short-syntax volume string.
    ///
    /// Supported forms:
    /// - `"myvolume:/app/data"` (named volume)
    /// - `"./data:/app/data"` (bind mount, relative)
    /// - `"/host/path:/container/path"` (bind mount, absolute)
    /// - `"./data:/app/data:ro"` (read-only)
    /// - `"/container/path"` (anonymous volume)
    /// - `"./data:/app/data:ro,z"` (multiple options)
    ///
    /// # Errors
    ///
    /// Returns `DockerError::ComposeParse` if the volume string is empty
    /// or has an invalid format.
    pub fn parse(s: &str) -> Result<Self, DockerError> {
        let s = s.trim();
        if s.is_empty() {
            return Err(DockerError::ComposeParse(
                "empty volume specification".to_string(),
            ));
        }

        // Split on ':' but be careful with Windows paths (C:\...)
        // Docker Compose handles this by looking at the second character.
        let parts = split_volume_spec(s);

        match parts.len() {
            1 => {
                // Anonymous volume: just a container path.
                Ok(ComposeVolume {
                    volume_type: Some(VolumeType::Volume),
                    source: None,
                    target: Some(parts[0].to_string()),
                    read_only: false,
                    bind: None,
                    volume: None,
                    tmpfs: None,
                    consistency: None,
                })
            }
            2 | 3 => {
                let source = parts[0].to_string();
                let target = parts[1].to_string();
                let options = if parts.len() == 3 { parts[2] } else { "" };

                let read_only = options.split(',').any(|o| o == "ro");

                // Determine volume type based on source.
                let vol_type = if source.starts_with('/')
                    || source.starts_with("./")
                    || source.starts_with("../")
                    || source.starts_with('~')
                {
                    VolumeType::Bind
                } else {
                    VolumeType::Volume
                };

                Ok(ComposeVolume {
                    volume_type: Some(vol_type),
                    source: Some(source),
                    target: Some(target),
                    read_only,
                    bind: None,
                    volume: None,
                    tmpfs: None,
                    consistency: None,
                })
            }
            _ => Err(DockerError::ComposeParse(format!(
                "invalid volume specification: {s}"
            ))),
        }
    }
}

/// Split a volume specification on `:`, respecting Windows drive-letter prefixes
/// (e.g. `C:\path`). On Linux/Mac this is simply `split(':')`.
fn split_volume_spec(s: &str) -> Vec<&str> {
    let bytes = s.as_bytes();
    let mut parts = Vec::new();
    let mut start = 0;

    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b':' {
            // Check for Windows drive letter: single alpha char before ':'
            // and next char is '\' or '/'.
            let is_drive = i == 1
                && bytes[0].is_ascii_alphabetic()
                && i + 1 < bytes.len()
                && (bytes[i + 1] == b'\\' || bytes[i + 1] == b'/');

            if is_drive {
                // Skip this colon, it's part of the drive letter.
                i += 1;
                continue;
            }

            parts.push(&s[start..i]);
            start = i + 1;
        }
        i += 1;
    }
    parts.push(&s[start..]);
    parts
}

// ---------------------------------------------------------------------------
// ComposeHealthcheck
// ---------------------------------------------------------------------------

/// Healthcheck configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComposeHealthcheck {
    /// The test command. Accepts string (`"curl -f http://localhost"`) or
    /// list (`["CMD", "curl", "-f", "http://localhost"]`).
    #[serde(default, deserialize_with = "deserialize_healthcheck_test")]
    pub test: Vec<String>,

    /// Time between checks (e.g. `"30s"`).
    #[serde(default)]
    pub interval: Option<String>,

    /// Single-check timeout.
    #[serde(default)]
    pub timeout: Option<String>,

    /// Number of consecutive failures before unhealthy.
    #[serde(default)]
    pub retries: Option<u64>,

    /// Grace period before checks begin.
    #[serde(default)]
    pub start_period: Option<String>,

    /// Interval between checks during start period.
    #[serde(default)]
    pub start_interval: Option<String>,

    /// Set to `true` to disable any inherited healthcheck.
    #[serde(default)]
    pub disable: bool,
}

// ---------------------------------------------------------------------------
// ComposeDependsOn
// ---------------------------------------------------------------------------

/// Dependency condition for `depends_on`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DependsOnCondition {
    #[default]
    ServiceStarted,
    ServiceHealthy,
    ServiceCompletedSuccessfully,
}

/// A dependency entry with an optional condition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComposeDependsOn {
    /// Condition under which the dependency is met.
    #[serde(default)]
    pub condition: DependsOnCondition,

    /// Whether to restart this service when the dependency restarts.
    #[serde(default)]
    pub restart: Option<bool>,
}

// ---------------------------------------------------------------------------
// ComposeDeploy
// ---------------------------------------------------------------------------

/// Deploy configuration (Swarm mode / Compose spec).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComposeDeploy {
    /// Number of replicas.
    #[serde(default)]
    pub replicas: Option<u64>,

    /// Resource constraints.
    #[serde(default)]
    pub resources: Option<ComposeResources>,

    /// Restart policy.
    #[serde(default)]
    pub restart_policy: Option<ComposeRestartPolicy>,

    /// Placement constraints.
    #[serde(default)]
    pub placement: Option<ComposePlacement>,

    /// Endpoint mode (`vip` or `dnsrr`).
    #[serde(default)]
    pub endpoint_mode: Option<String>,

    /// Deployment mode (`replicated` or `global`).
    #[serde(default)]
    pub mode: Option<String>,

    /// Deploy labels.
    #[serde(default, deserialize_with = "deserialize_labels")]
    pub labels: HashMap<String, String>,

    /// Update configuration.
    #[serde(default)]
    pub update_config: Option<ComposeUpdateConfig>,

    /// Rollback configuration.
    #[serde(default)]
    pub rollback_config: Option<ComposeUpdateConfig>,

    /// Extension fields.
    #[serde(flatten)]
    pub extensions: HashMap<String, serde_yaml::Value>,
}

/// Resource limits and reservations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComposeResources {
    #[serde(default)]
    pub limits: Option<ComposeResourceSpec>,
    #[serde(default)]
    pub reservations: Option<ComposeResourceSpec>,
}

/// A single resource specification (used for both limits and reservations).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComposeResourceSpec {
    /// CPU quota (e.g. `"0.5"`, `"2"`).
    #[serde(default)]
    pub cpus: Option<StringOrNumber>,
    /// Memory limit (e.g. `"512M"`, `"1G"`).
    #[serde(default)]
    pub memory: Option<String>,
    /// Number of PIDs.
    #[serde(default)]
    pub pids: Option<u64>,
    /// GPU devices.
    #[serde(default)]
    pub devices: Option<Vec<ComposeDeviceRequest>>,
}

/// GPU / device request in deploy resources.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComposeDeviceRequest {
    #[serde(default)]
    pub capabilities: Vec<Vec<String>>,
    #[serde(default)]
    pub driver: Option<String>,
    #[serde(default)]
    pub count: Option<StringOrNumber>,
    #[serde(default)]
    pub device_ids: Option<Vec<String>>,
    /// Extension fields.
    #[serde(flatten)]
    pub extensions: HashMap<String, serde_yaml::Value>,
}

/// Restart policy for deploy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComposeRestartPolicy {
    #[serde(default)]
    pub condition: Option<String>,
    #[serde(default)]
    pub delay: Option<String>,
    #[serde(default)]
    pub max_attempts: Option<u64>,
    #[serde(default)]
    pub window: Option<String>,
}

/// Placement constraints.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComposePlacement {
    #[serde(default)]
    pub constraints: Vec<String>,
    #[serde(default)]
    pub preferences: Vec<ComposePlacementPreference>,
    #[serde(default)]
    pub max_replicas_per_node: Option<u64>,
}

/// Placement preference.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComposePlacementPreference {
    #[serde(default)]
    pub spread: Option<String>,
}

/// Update / rollback configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComposeUpdateConfig {
    #[serde(default)]
    pub parallelism: Option<u64>,
    #[serde(default)]
    pub delay: Option<String>,
    #[serde(default)]
    pub failure_action: Option<String>,
    #[serde(default)]
    pub monitor: Option<String>,
    #[serde(default)]
    pub max_failure_ratio: Option<f64>,
    #[serde(default)]
    pub order: Option<String>,
}

// ---------------------------------------------------------------------------
// Networks (top-level + service-level)
// ---------------------------------------------------------------------------

/// Top-level network definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComposeNetworkConfig {
    #[serde(default)]
    pub driver: Option<String>,

    #[serde(default)]
    pub driver_opts: HashMap<String, StringOrNumber>,

    #[serde(default)]
    pub external: Option<BoolOrExternal>,

    #[serde(default, deserialize_with = "deserialize_labels")]
    pub labels: HashMap<String, String>,

    #[serde(default)]
    pub ipam: Option<ComposeIpam>,

    #[serde(default)]
    pub name: Option<String>,

    #[serde(default)]
    pub internal: bool,

    #[serde(default)]
    pub attachable: bool,

    #[serde(default)]
    pub enable_ipv6: bool,

    /// Extension fields.
    #[serde(flatten)]
    pub extensions: HashMap<String, serde_yaml::Value>,
}

/// IPAM configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComposeIpam {
    #[serde(default)]
    pub driver: Option<String>,
    #[serde(default)]
    pub config: Vec<ComposeIpamConfig>,
    #[serde(default)]
    pub options: HashMap<String, String>,
}

/// A single IPAM subnet configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComposeIpamConfig {
    #[serde(default)]
    pub subnet: Option<String>,
    #[serde(default)]
    pub gateway: Option<String>,
    #[serde(default)]
    pub ip_range: Option<String>,
    #[serde(default)]
    pub aux_addresses: HashMap<String, String>,
}

/// `external` can be `true` or `{ name: "foo" }`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum BoolOrExternal {
    Bool(bool),
    Named { name: String },
}

/// Service-level networks. Either a simple list of network names or a map
/// with per-network configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ComposeServiceNetworks {
    /// Simple list: `networks: [frontend, backend]`.
    List(Vec<String>),
    /// Map with per-network config.
    Map(HashMap<String, Option<ComposeServiceNetworkConfig>>),
}

/// Per-network configuration for a service.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComposeServiceNetworkConfig {
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub ipv4_address: Option<String>,
    #[serde(default)]
    pub ipv6_address: Option<String>,
    #[serde(default)]
    pub link_local_ips: Vec<String>,
    #[serde(default)]
    pub priority: Option<i32>,
}

// ---------------------------------------------------------------------------
// Volumes (top-level)
// ---------------------------------------------------------------------------

/// Top-level volume definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComposeVolumeConfig {
    #[serde(default)]
    pub driver: Option<String>,

    #[serde(default)]
    pub driver_opts: HashMap<String, StringOrNumber>,

    #[serde(default)]
    pub external: Option<BoolOrExternal>,

    #[serde(default, deserialize_with = "deserialize_labels")]
    pub labels: HashMap<String, String>,

    #[serde(default)]
    pub name: Option<String>,

    /// Extension fields.
    #[serde(flatten)]
    pub extensions: HashMap<String, serde_yaml::Value>,
}

// ---------------------------------------------------------------------------
// Secrets and Configs (top-level + service-level)
// ---------------------------------------------------------------------------

/// Top-level secret definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComposeSecretConfig {
    #[serde(default)]
    pub file: Option<String>,

    #[serde(default)]
    pub external: Option<BoolOrExternal>,

    #[serde(default)]
    pub name: Option<String>,

    #[serde(default)]
    pub environment: Option<String>,

    /// Extension fields.
    #[serde(flatten)]
    pub extensions: HashMap<String, serde_yaml::Value>,
}

/// Top-level config definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComposeConfigDef {
    #[serde(default)]
    pub file: Option<String>,

    #[serde(default)]
    pub external: Option<BoolOrExternal>,

    #[serde(default)]
    pub name: Option<String>,

    #[serde(default)]
    pub content: Option<String>,

    #[serde(default)]
    pub environment: Option<String>,

    /// Extension fields.
    #[serde(flatten)]
    pub extensions: HashMap<String, serde_yaml::Value>,
}

/// Service-level secret reference. Either just a string name or a full object.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ComposeServiceSecret {
    /// Short form: just the secret name.
    Simple(String),
    /// Long form: full secret mount configuration.
    Full(ComposeServiceSecretConfig),
}

/// Full service-level secret configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComposeServiceSecretConfig {
    pub source: String,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub uid: Option<String>,
    #[serde(default)]
    pub gid: Option<String>,
    #[serde(default)]
    pub mode: Option<u32>,
}

/// Service-level config reference. Either just a string name or a full object.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ComposeServiceConfig {
    /// Short form: just the config name.
    Simple(String),
    /// Long form: full config mount configuration.
    Full(ComposeServiceConfigObj),
}

/// Full service-level config configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComposeServiceConfigObj {
    pub source: String,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub uid: Option<String>,
    #[serde(default)]
    pub gid: Option<String>,
    #[serde(default)]
    pub mode: Option<u32>,
}

// ---------------------------------------------------------------------------
// Ulimits
// ---------------------------------------------------------------------------

/// Ulimit value. Either a single integer or a `{soft, hard}` pair.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ComposeUlimit {
    /// Single value used for both soft and hard.
    Single(i64),
    /// Separate soft and hard limits.
    SoftHard { soft: i64, hard: i64 },
}

// ---------------------------------------------------------------------------
// Logging
// ---------------------------------------------------------------------------

/// Logging configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComposeLogging {
    #[serde(default)]
    pub driver: Option<String>,
    #[serde(default)]
    pub options: HashMap<String, String>,
}

// ---------------------------------------------------------------------------
// Env File
// ---------------------------------------------------------------------------

/// `env_file` can be a single string, a list of strings, or a list of
/// objects with `path` and `required` fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum EnvFile {
    /// Single file path.
    Single(String),
    /// List of file paths or objects.
    List(Vec<EnvFileEntry>),
}

/// An entry in the `env_file` list. Either a plain string or an object.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum EnvFileEntry {
    /// Just a file path string.
    Path(String),
    /// Object with path and optional `required` flag.
    Config(EnvFileConfig),
}

/// Full `env_file` entry with path and required flag.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvFileConfig {
    pub path: String,
    #[serde(default = "default_true")]
    pub required: bool,
}

fn default_true() -> bool {
    true
}

// ---------------------------------------------------------------------------
// StringOrList / StringOrNumber — common Docker Compose patterns
// ---------------------------------------------------------------------------

/// A value that can be either a single string or a list of strings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StringOrList {
    Single(String),
    List(Vec<String>),
}

impl StringOrList {
    /// Convert to a `Vec<String>` regardless of variant.
    #[must_use]
    pub fn into_list(self) -> Vec<String> {
        match self {
            StringOrList::Single(s) => vec![s],
            StringOrList::List(v) => v,
        }
    }

    /// View as a slice-like iterator regardless of variant.
    #[must_use]
    pub fn as_slice(&self) -> Vec<&str> {
        match self {
            StringOrList::Single(s) => vec![s.as_str()],
            StringOrList::List(v) => v.iter().map(String::as_str).collect(),
        }
    }
}

/// A value that can be either a string or a number. Docker Compose uses this
/// for fields like `cpus`, `shm_size`, `expose`, etc.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StringOrNumber {
    String(String),
    Integer(i64),
    Float(f64),
}

impl fmt::Display for StringOrNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StringOrNumber::String(s) => write!(f, "{s}"),
            StringOrNumber::Integer(n) => write!(f, "{n}"),
            StringOrNumber::Float(n) => write!(f, "{n}"),
        }
    }
}

impl StringOrNumber {
    /// Get the value as a string.
    #[must_use]
    pub fn as_string(&self) -> String {
        self.to_string()
    }
}

// =========================================================================
// Custom Deserializers
// =========================================================================

// ---------------------------------------------------------------------------
// deserialize_ports: Vec<string | ComposePort long-syntax object>
// ---------------------------------------------------------------------------

/// Docker Compose long-syntax port mapping. Uses Docker Compose's native field
/// names (`target`, `published`) which differ from our internal `ComposePort`.
#[derive(Deserialize)]
struct PortLongSyntax {
    target: StringOrNumber,
    #[serde(default)]
    published: Option<StringOrNumber>,
    #[serde(default)]
    protocol: Option<String>,
    #[serde(default)]
    host_ip: Option<String>,
}

/// Intermediate enum for port deserialization. Each element in the `ports`
/// list can be either a string (short syntax) or an object (long syntax).
#[derive(Deserialize)]
#[serde(untagged)]
enum PortEntry {
    Short(StringOrNumber),
    Long(PortLongSyntax),
}

/// Deserialize the `ports` field which can contain a mix of short-syntax
/// strings and long-syntax objects.
fn deserialize_ports<'de, D>(deserializer: D) -> Result<Vec<ComposePort>, D::Error>
where
    D: Deserializer<'de>,
{
    let entries: Vec<PortEntry> = Vec::deserialize(deserializer)?;
    let mut result = Vec::with_capacity(entries.len());

    for entry in entries {
        match entry {
            PortEntry::Short(s) => {
                let port = ComposePort::parse(&s.as_string()).map_err(de::Error::custom)?;
                result.push(port);
            }
            PortEntry::Long(long) => {
                result.push(ComposePort {
                    host_ip: long.host_ip,
                    host_port: long.published.map(|p| p.as_string()),
                    container_port: long.target.as_string(),
                    protocol: long.protocol,
                });
            }
        }
    }

    Ok(result)
}

// ---------------------------------------------------------------------------
// deserialize_volumes: Vec<string | ComposeVolume long-syntax object>
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(untagged)]
enum VolumeEntry {
    Short(String),
    Long(ComposeVolume),
}

/// Deserialize the `volumes` field which can contain a mix of short-syntax
/// strings and long-syntax objects.
fn deserialize_volumes<'de, D>(deserializer: D) -> Result<Vec<ComposeVolume>, D::Error>
where
    D: Deserializer<'de>,
{
    let entries: Vec<VolumeEntry> = Vec::deserialize(deserializer)?;
    let mut result = Vec::with_capacity(entries.len());

    for entry in entries {
        match entry {
            VolumeEntry::Short(s) => {
                let vol = ComposeVolume::parse(&s).map_err(de::Error::custom)?;
                result.push(vol);
            }
            VolumeEntry::Long(vol) => {
                result.push(vol);
            }
        }
    }

    Ok(result)
}

// ---------------------------------------------------------------------------
// deserialize_env: HashMap from either map or list of "KEY=VALUE"
// ---------------------------------------------------------------------------

/// Deserialize `environment` which can be either:
/// - a map: `{ FOO: bar, BAZ: qux }`
/// - a list: `["FOO=bar", "BAZ=qux"]`
///
/// Values in the map can also be numbers, booleans, or null.
fn deserialize_env<'de, D>(deserializer: D) -> Result<HashMap<String, String>, D::Error>
where
    D: Deserializer<'de>,
{
    struct EnvVisitor;

    impl<'de> Visitor<'de> for EnvVisitor {
        type Value = HashMap<String, String>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a map or a list of KEY=VALUE strings")
        }

        fn visit_seq<S>(self, mut seq: S) -> Result<Self::Value, S::Error>
        where
            S: SeqAccess<'de>,
        {
            let mut map = HashMap::new();
            while let Some(item) = seq.next_element::<String>()? {
                if let Some(eq) = item.find('=') {
                    let key = item[..eq].to_string();
                    let value = item[eq + 1..].to_string();
                    map.insert(key, value);
                } else {
                    // Key with no value (e.g. just "DEBUG") — store empty string.
                    map.insert(item, String::new());
                }
            }
            Ok(map)
        }

        fn visit_map<M>(self, mut access: M) -> Result<Self::Value, M::Error>
        where
            M: MapAccess<'de>,
        {
            let mut map = HashMap::new();
            while let Some((key, value)) = access.next_entry::<String, serde_yaml::Value>()? {
                let string_value = yaml_value_to_string(&value);
                map.insert(key, string_value);
            }
            Ok(map)
        }
    }

    deserializer.deserialize_any(EnvVisitor)
}

/// Convert a `serde_yaml::Value` to a string representation suitable for
/// environment variables.
fn yaml_value_to_string(v: &serde_yaml::Value) -> String {
    match v {
        serde_yaml::Value::Null => String::new(),
        serde_yaml::Value::Bool(b) => b.to_string(),
        serde_yaml::Value::Number(n) => n.to_string(),
        serde_yaml::Value::String(s) => s.clone(),
        other => format!("{other:?}"),
    }
}

// ---------------------------------------------------------------------------
// deserialize_labels: HashMap from either map or list of "key=value"
// ---------------------------------------------------------------------------

/// Deserialize `labels` which, like `environment`, can be either a map or
/// a list of `"key=value"` strings.
fn deserialize_labels<'de, D>(deserializer: D) -> Result<HashMap<String, String>, D::Error>
where
    D: Deserializer<'de>,
{
    // Reuse the same logic as environment.
    deserialize_env(deserializer)
}

// ---------------------------------------------------------------------------
// deserialize_sysctls: HashMap from either map or list of "key=value"
// ---------------------------------------------------------------------------

/// Deserialize `sysctls` which can be either a map or a list. Values may
/// also be numbers.
fn deserialize_sysctls<'de, D>(deserializer: D) -> Result<HashMap<String, String>, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_env(deserializer)
}

// ---------------------------------------------------------------------------
// deserialize_depends_on: HashMap from either list or map with conditions
// ---------------------------------------------------------------------------

/// Deserialize `depends_on` which can be:
/// - a list: `["db", "redis"]` (all default to `service_started`)
/// - a map: `{ db: { condition: service_healthy }, redis: { condition: service_started } }`
fn deserialize_depends_on<'de, D>(
    deserializer: D,
) -> Result<HashMap<String, ComposeDependsOn>, D::Error>
where
    D: Deserializer<'de>,
{
    struct DependsOnVisitor;

    impl<'de> Visitor<'de> for DependsOnVisitor {
        type Value = HashMap<String, ComposeDependsOn>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a list of service names or a map with conditions")
        }

        fn visit_seq<S>(self, mut seq: S) -> Result<Self::Value, S::Error>
        where
            S: SeqAccess<'de>,
        {
            let mut map = HashMap::new();
            while let Some(name) = seq.next_element::<String>()? {
                map.insert(
                    name,
                    ComposeDependsOn {
                        condition: DependsOnCondition::ServiceStarted,
                        restart: None,
                    },
                );
            }
            Ok(map)
        }

        fn visit_map<M>(self, mut access: M) -> Result<Self::Value, M::Error>
        where
            M: MapAccess<'de>,
        {
            let mut map = HashMap::new();
            while let Some((key, value)) = access.next_entry::<String, ComposeDependsOn>()? {
                map.insert(key, value);
            }
            Ok(map)
        }
    }

    deserializer.deserialize_any(DependsOnVisitor)
}

// ---------------------------------------------------------------------------
// deserialize_string_or_list: Vec<String> from either string or list
// ---------------------------------------------------------------------------

/// Deserialize a field that can be either a single string (split on
/// whitespace for `command`/`entrypoint`) or a list of strings.
fn deserialize_string_or_list<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    struct StringOrListVisitor;

    impl<'de> Visitor<'de> for StringOrListVisitor {
        type Value = Vec<String>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a string or a list of strings")
        }

        fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            // For command/entrypoint string form, Docker passes the whole
            // string to the shell. We preserve it as a single element so the
            // caller can decide how to handle it (shell -c vs split).
            Ok(vec![v.to_string()])
        }

        fn visit_seq<S>(self, mut seq: S) -> Result<Self::Value, S::Error>
        where
            S: SeqAccess<'de>,
        {
            let mut items = Vec::new();
            while let Some(item) = seq.next_element::<String>()? {
                items.push(item);
            }
            Ok(items)
        }

        fn visit_none<E>(self) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(Vec::new())
        }

        fn visit_unit<E>(self) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(Vec::new())
        }
    }

    deserializer.deserialize_any(StringOrListVisitor)
}

// ---------------------------------------------------------------------------
// deserialize_healthcheck_test: Vec<String> from string or list
// ---------------------------------------------------------------------------

/// Deserialize healthcheck `test` which can be:
/// - a string: `"curl -f http://localhost"` (wrapped as `["CMD-SHELL", ...]`)
/// - a list: `["CMD", "curl", "-f", "http://localhost"]`
fn deserialize_healthcheck_test<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    struct HealthTestVisitor;

    impl<'de> Visitor<'de> for HealthTestVisitor {
        type Value = Vec<String>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a string or a list of strings for healthcheck test")
        }

        fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(vec!["CMD-SHELL".to_string(), v.to_string()])
        }

        fn visit_seq<S>(self, mut seq: S) -> Result<Self::Value, S::Error>
        where
            S: SeqAccess<'de>,
        {
            let mut items = Vec::new();
            while let Some(item) = seq.next_element::<String>()? {
                items.push(item);
            }
            Ok(items)
        }

        fn visit_none<E>(self) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(Vec::new())
        }

        fn visit_unit<E>(self) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(Vec::new())
        }
    }

    deserializer.deserialize_any(HealthTestVisitor)
}

// ---------------------------------------------------------------------------
// deserialize_service_secrets / deserialize_service_configs
// ---------------------------------------------------------------------------

/// Deserialize service-level secrets list that can contain strings or objects.
fn deserialize_service_secrets<'de, D>(
    deserializer: D,
) -> Result<Vec<ComposeServiceSecret>, D::Error>
where
    D: Deserializer<'de>,
{
    Vec::<ComposeServiceSecret>::deserialize(deserializer)
}

/// Deserialize service-level configs list that can contain strings or objects.
fn deserialize_service_configs<'de, D>(
    deserializer: D,
) -> Result<Vec<ComposeServiceConfig>, D::Error>
where
    D: Deserializer<'de>,
{
    Vec::<ComposeServiceConfig>::deserialize(deserializer)
}

// =========================================================================
// Tests
// =========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_port_container_only() {
        let p = ComposePort::parse("80").unwrap();
        assert_eq!(p.container_port, "80");
        assert!(p.host_port.is_none());
        assert!(p.host_ip.is_none());
        assert!(p.protocol.is_none());
    }

    #[test]
    fn test_parse_port_host_container() {
        let p = ComposePort::parse("8080:80").unwrap();
        assert_eq!(p.host_port.as_deref(), Some("8080"));
        assert_eq!(p.container_port, "80");
        assert!(p.host_ip.is_none());
    }

    #[test]
    fn test_parse_port_full() {
        let p = ComposePort::parse("127.0.0.1:8080:80/tcp").unwrap();
        assert_eq!(p.host_ip.as_deref(), Some("127.0.0.1"));
        assert_eq!(p.host_port.as_deref(), Some("8080"));
        assert_eq!(p.container_port, "80");
        assert_eq!(p.protocol.as_deref(), Some("tcp"));
    }

    #[test]
    fn test_parse_port_ip_no_host_port() {
        let p = ComposePort::parse("127.0.0.1::80").unwrap();
        assert_eq!(p.host_ip.as_deref(), Some("127.0.0.1"));
        assert!(p.host_port.is_none());
        assert_eq!(p.container_port, "80");
    }

    #[test]
    fn test_parse_port_range() {
        let p = ComposePort::parse("9090-9091:8080-8081").unwrap();
        assert_eq!(p.host_port.as_deref(), Some("9090-9091"));
        assert_eq!(p.container_port, "8080-8081");
    }

    #[test]
    fn test_parse_port_udp() {
        let p = ComposePort::parse("5000:5000/udp").unwrap();
        assert_eq!(p.protocol.as_deref(), Some("udp"));
    }

    #[test]
    fn test_parse_volume_anonymous() {
        let v = ComposeVolume::parse("/app/data").unwrap();
        assert!(v.source.is_none());
        assert_eq!(v.target.as_deref(), Some("/app/data"));
        assert!(!v.read_only);
    }

    #[test]
    fn test_parse_volume_named() {
        let v = ComposeVolume::parse("mydata:/app/data").unwrap();
        assert_eq!(v.source.as_deref(), Some("mydata"));
        assert_eq!(v.target.as_deref(), Some("/app/data"));
        assert_eq!(v.volume_type, Some(VolumeType::Volume));
    }

    #[test]
    fn test_parse_volume_bind() {
        let v = ComposeVolume::parse("./data:/app/data").unwrap();
        assert_eq!(v.source.as_deref(), Some("./data"));
        assert_eq!(v.target.as_deref(), Some("/app/data"));
        assert_eq!(v.volume_type, Some(VolumeType::Bind));
    }

    #[test]
    fn test_parse_volume_bind_readonly() {
        let v = ComposeVolume::parse("/host/data:/container/data:ro").unwrap();
        assert!(v.read_only);
        assert_eq!(v.volume_type, Some(VolumeType::Bind));
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn test_deserialize_full_compose_file() {
        let yaml = r#"
version: "3.8"
services:
  web:
    image: nginx:latest
    ports:
      - "8080:80"
      - "443:443/tcp"
    volumes:
      - ./html:/usr/share/nginx/html:ro
      - nginx_data:/etc/nginx
    environment:
      - NODE_ENV=production
      - DEBUG=false
    depends_on:
      - api
    restart: always
    deploy:
      replicas: 3
      resources:
        limits:
          cpus: "0.5"
          memory: 256M

  api:
    build:
      context: ./api
      dockerfile: Dockerfile.prod
      args:
        BUILD_ENV: production
    ports:
      - "3000:3000"
    environment:
      DATABASE_URL: postgres://db:5432/myapp
      REDIS_URL: redis://redis:6379
    depends_on:
      db:
        condition: service_healthy
      redis:
        condition: service_started
    healthcheck:
      test: ["CMD", "curl", "-f", "http://localhost:3000/health"]
      interval: 30s
      timeout: 10s
      retries: 3
      start_period: 10s

  db:
    image: postgres:15
    volumes:
      - db_data:/var/lib/postgresql/data
    environment:
      POSTGRES_DB: myapp
      POSTGRES_USER: user
      POSTGRES_PASSWORD: secret
    healthcheck:
      test: "pg_isready -U user"
      interval: 10s
      timeout: 5s
      retries: 5

  redis:
    image: redis:7-alpine
    command: redis-server --appendonly yes
    volumes:
      - redis_data:/data

volumes:
  nginx_data:
  db_data:
    driver: local
  redis_data:

networks:
  frontend:
    driver: bridge
  backend:
    internal: true
"#;

        let compose: ComposeFile = serde_yaml::from_str(yaml).unwrap();

        // Top level
        assert_eq!(compose.version.as_deref(), Some("3.8"));
        assert_eq!(compose.services.len(), 4);
        assert_eq!(compose.volumes.len(), 3);
        assert_eq!(compose.networks.len(), 2);

        // Web service
        let web = &compose.services["web"];
        assert_eq!(web.image.as_deref(), Some("nginx:latest"));
        assert_eq!(web.ports.len(), 2);
        assert_eq!(web.ports[0].host_port.as_deref(), Some("8080"));
        assert_eq!(web.ports[0].container_port, "80");
        assert_eq!(web.ports[1].protocol.as_deref(), Some("tcp"));
        assert_eq!(web.volumes.len(), 2);
        assert_eq!(
            web.environment
                .get("NODE_ENV")
                .map(std::string::String::as_str),
            Some("production")
        );
        assert_eq!(web.depends_on.len(), 1);
        assert!(web.depends_on.contains_key("api"));
        assert_eq!(web.restart.as_deref(), Some("always"));

        // Deploy
        let deploy = web.deploy.as_ref().unwrap();
        assert_eq!(deploy.replicas, Some(3));

        // API service
        let api = &compose.services["api"];
        assert!(api.build.is_some());
        if let Some(ComposeBuild::Full(cfg)) = &api.build {
            assert_eq!(cfg.context.as_deref(), Some("./api"));
            assert_eq!(cfg.dockerfile.as_deref(), Some("Dockerfile.prod"));
            assert_eq!(
                cfg.args.get("BUILD_ENV").map(std::string::String::as_str),
                Some("production")
            );
        } else {
            panic!("expected full build config");
        }

        // Depends on with conditions
        let db_dep = &api.depends_on["db"];
        assert_eq!(db_dep.condition, DependsOnCondition::ServiceHealthy);
        let redis_dep = &api.depends_on["redis"];
        assert_eq!(redis_dep.condition, DependsOnCondition::ServiceStarted);

        // Healthcheck with list test
        let hc = api.healthcheck.as_ref().unwrap();
        assert_eq!(
            hc.test,
            vec!["CMD", "curl", "-f", "http://localhost:3000/health"]
        );
        assert_eq!(hc.retries, Some(3));

        // DB healthcheck with string test
        let db = &compose.services["db"];
        let db_hc = db.healthcheck.as_ref().unwrap();
        assert_eq!(db_hc.test[0], "CMD-SHELL");
        assert_eq!(db_hc.test[1], "pg_isready -U user");

        // Redis command (string form)
        let redis = &compose.services["redis"];
        assert_eq!(redis.command.len(), 1);
        assert_eq!(redis.command[0], "redis-server --appendonly yes");

        // Networks
        let frontend = compose.networks["frontend"].as_ref().unwrap();
        assert_eq!(frontend.driver.as_deref(), Some("bridge"));
        let backend = compose.networks["backend"].as_ref().unwrap();
        assert!(backend.internal);
    }

    #[test]
    fn test_deserialize_env_map_format() {
        let yaml = r"
services:
  app:
    image: myapp
    environment:
      FOO: bar
      NUM: 42
      BOOL: true
";
        let compose: ComposeFile = serde_yaml::from_str(yaml).unwrap();
        let app = &compose.services["app"];
        assert_eq!(
            app.environment.get("FOO").map(std::string::String::as_str),
            Some("bar")
        );
        assert_eq!(
            app.environment.get("NUM").map(std::string::String::as_str),
            Some("42")
        );
        assert_eq!(
            app.environment.get("BOOL").map(std::string::String::as_str),
            Some("true")
        );
    }

    #[test]
    fn test_deserialize_env_list_format() {
        let yaml = r"
services:
  app:
    image: myapp
    environment:
      - FOO=bar
      - DEBUG
";
        let compose: ComposeFile = serde_yaml::from_str(yaml).unwrap();
        let app = &compose.services["app"];
        assert_eq!(
            app.environment.get("FOO").map(std::string::String::as_str),
            Some("bar")
        );
        assert_eq!(
            app.environment
                .get("DEBUG")
                .map(std::string::String::as_str),
            Some("")
        );
    }

    #[test]
    fn test_build_simple_string() {
        let yaml = r"
services:
  app:
    build: ./myapp
";
        let compose: ComposeFile = serde_yaml::from_str(yaml).unwrap();
        let app = &compose.services["app"];
        match &app.build {
            Some(ComposeBuild::Simple(ctx)) => assert_eq!(ctx, "./myapp"),
            other => panic!("expected simple build, got: {other:?}"),
        }
    }

    #[test]
    fn test_service_networks_list() {
        let yaml = r"
services:
  app:
    image: myapp
    networks:
      - frontend
      - backend
";
        let compose: ComposeFile = serde_yaml::from_str(yaml).unwrap();
        let app = &compose.services["app"];
        match &app.networks {
            Some(ComposeServiceNetworks::List(nets)) => {
                assert_eq!(nets, &["frontend", "backend"]);
            }
            other => panic!("expected list, got: {other:?}"),
        }
    }

    #[test]
    fn test_service_networks_map() {
        let yaml = r"
services:
  app:
    image: myapp
    networks:
      frontend:
        aliases:
          - web
        ipv4_address: 172.16.0.10
      backend:
";
        let compose: ComposeFile = serde_yaml::from_str(yaml).unwrap();
        let app = &compose.services["app"];
        match &app.networks {
            Some(ComposeServiceNetworks::Map(nets)) => {
                let frontend = nets["frontend"].as_ref().unwrap();
                assert_eq!(frontend.aliases, vec!["web"]);
                assert_eq!(frontend.ipv4_address.as_deref(), Some("172.16.0.10"));
                // backend has null value
                assert!(nets.contains_key("backend"));
            }
            other => panic!("expected map, got: {other:?}"),
        }
    }

    #[test]
    fn test_ulimit_single() {
        let yaml = r"
services:
  app:
    image: myapp
    ulimits:
      nofile: 65536
";
        let compose: ComposeFile = serde_yaml::from_str(yaml).unwrap();
        let app = &compose.services["app"];
        match &app.ulimits["nofile"] {
            ComposeUlimit::Single(n) => assert_eq!(*n, 65536),
            other @ ComposeUlimit::SoftHard { .. } => panic!("expected single, got: {other:?}"),
        }
    }

    #[test]
    fn test_ulimit_soft_hard() {
        let yaml = r"
services:
  app:
    image: myapp
    ulimits:
      nofile:
        soft: 1024
        hard: 65536
";
        let compose: ComposeFile = serde_yaml::from_str(yaml).unwrap();
        let app = &compose.services["app"];
        match &app.ulimits["nofile"] {
            ComposeUlimit::SoftHard { soft, hard } => {
                assert_eq!(*soft, 1024);
                assert_eq!(*hard, 65536);
            }
            other @ ComposeUlimit::Single(_) => panic!("expected soft/hard, got: {other:?}"),
        }
    }

    #[test]
    fn test_command_list_form() {
        let yaml = r#"
services:
  app:
    image: myapp
    command: ["python", "-m", "app"]
"#;
        let compose: ComposeFile = serde_yaml::from_str(yaml).unwrap();
        let app = &compose.services["app"];
        assert_eq!(app.command, vec!["python", "-m", "app"]);
    }

    #[test]
    fn test_secrets_and_configs() {
        let yaml = r"
services:
  app:
    image: myapp
    secrets:
      - my_secret
      - source: other_secret
        target: /run/secrets/other
        mode: 440
    configs:
      - my_config
      - source: other_config
        target: /etc/other.conf

secrets:
  my_secret:
    file: ./secret.txt
  other_secret:
    external: true

configs:
  my_config:
    file: ./config.txt
  other_config:
    external: true
";
        let compose: ComposeFile = serde_yaml::from_str(yaml).unwrap();
        let app = &compose.services["app"];

        assert_eq!(app.secrets.len(), 2);
        match &app.secrets[0] {
            ComposeServiceSecret::Simple(name) => assert_eq!(name, "my_secret"),
            other @ ComposeServiceSecret::Full(_) => {
                panic!("expected simple secret, got: {other:?}")
            }
        }
        match &app.secrets[1] {
            ComposeServiceSecret::Full(cfg) => {
                assert_eq!(cfg.source, "other_secret");
                assert_eq!(cfg.target.as_deref(), Some("/run/secrets/other"));
            }
            other @ ComposeServiceSecret::Simple(_) => {
                panic!("expected full secret, got: {other:?}")
            }
        }

        assert_eq!(compose.secrets.len(), 2);
        assert_eq!(
            compose.secrets["my_secret"].file.as_deref(),
            Some("./secret.txt")
        );
    }

    #[test]
    fn test_env_file_variants() {
        // Single string
        let yaml = r"
services:
  app:
    image: myapp
    env_file: .env
";
        let compose: ComposeFile = serde_yaml::from_str(yaml).unwrap();
        match &compose.services["app"].env_file {
            Some(EnvFile::Single(s)) => assert_eq!(s, ".env"),
            other => panic!("expected single, got: {other:?}"),
        }

        // List form
        let yaml = r"
services:
  app:
    image: myapp
    env_file:
      - .env
      - .env.local
";
        let compose: ComposeFile = serde_yaml::from_str(yaml).unwrap();
        match &compose.services["app"].env_file {
            Some(EnvFile::List(entries)) => {
                assert_eq!(entries.len(), 2);
            }
            other => panic!("expected list, got: {other:?}"),
        }
    }

    #[test]
    fn test_external_network() {
        let yaml = r"
networks:
  existing:
    external: true
  named:
    external:
      name: my-existing-network
";
        let compose: ComposeFile = serde_yaml::from_str(yaml).unwrap();
        let existing = compose.networks["existing"].as_ref().unwrap();
        match &existing.external {
            Some(BoolOrExternal::Bool(true)) => {}
            other => panic!("expected true, got: {other:?}"),
        }
        let named = compose.networks["named"].as_ref().unwrap();
        match &named.external {
            Some(BoolOrExternal::Named { name }) => {
                assert_eq!(name, "my-existing-network");
            }
            other => panic!("expected named, got: {other:?}"),
        }
    }

    #[test]
    fn test_deploy_resources() {
        let yaml = r#"
services:
  app:
    image: myapp
    deploy:
      resources:
        limits:
          cpus: "1.5"
          memory: 512M
        reservations:
          cpus: "0.25"
          memory: 128M
"#;
        let compose: ComposeFile = serde_yaml::from_str(yaml).unwrap();
        let deploy = compose.services["app"].deploy.as_ref().unwrap();
        let resources = deploy.resources.as_ref().unwrap();
        let limits = resources.limits.as_ref().unwrap();
        assert_eq!(limits.memory.as_deref(), Some("512M"));
        let reservations = resources.reservations.as_ref().unwrap();
        assert_eq!(reservations.memory.as_deref(), Some("128M"));
    }

    #[test]
    fn test_logging_config() {
        let yaml = r#"
services:
  app:
    image: myapp
    logging:
      driver: json-file
      options:
        max-size: "10m"
        max-file: "3"
"#;
        let compose: ComposeFile = serde_yaml::from_str(yaml).unwrap();
        let logging = compose.services["app"].logging.as_ref().unwrap();
        assert_eq!(logging.driver.as_deref(), Some("json-file"));
        assert_eq!(
            logging
                .options
                .get("max-size")
                .map(std::string::String::as_str),
            Some("10m")
        );
    }

    #[test]
    fn test_string_or_list() {
        let sol = StringOrList::Single("hello".to_string());
        assert_eq!(sol.into_list(), vec!["hello"]);

        let sol = StringOrList::List(vec!["a".to_string(), "b".to_string()]);
        assert_eq!(sol.as_slice(), vec!["a", "b"]);
    }

    #[test]
    fn test_ipv6_port() {
        let p = ComposePort::parse("[::1]:8080:80").unwrap();
        assert_eq!(p.host_ip.as_deref(), Some("::1"));
        assert_eq!(p.host_port.as_deref(), Some("8080"));
        assert_eq!(p.container_port, "80");
    }

    #[test]
    fn test_labels_list_format() {
        let yaml = r"
services:
  app:
    image: myapp
    labels:
      - com.example.env=production
      - com.example.team=backend
";
        let compose: ComposeFile = serde_yaml::from_str(yaml).unwrap();
        let app = &compose.services["app"];
        assert_eq!(
            app.labels
                .get("com.example.env")
                .map(std::string::String::as_str),
            Some("production")
        );
    }

    #[test]
    fn test_minimal_compose() {
        // Minimal valid compose file
        let yaml = r"
services:
  app:
    image: alpine
";
        let compose: ComposeFile = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(compose.services.len(), 1);
        assert_eq!(compose.services["app"].image.as_deref(), Some("alpine"));
    }

    #[test]
    fn test_extension_fields() {
        // Extension fields (x-*) are captured in the extensions map.
        // Note: YAML merge keys (<<: *anchor) are a YAML 1.1 feature not
        // supported by serde_yaml 0.9, so we test extensions directly.
        let yaml = r"
x-custom-data:
  foo: bar

services:
  app:
    image: myapp
    restart: always
    x-custom-label: my-value
";
        let compose: ComposeFile = serde_yaml::from_str(yaml).unwrap();

        // Top-level extension field captured
        assert!(compose.extensions.contains_key("x-custom-data"));

        // Service-level extension field captured
        let app = &compose.services["app"];
        assert_eq!(app.restart.as_deref(), Some("always"));
        assert!(app.extensions.contains_key("x-custom-label"));
    }

    #[test]
    fn test_depends_on_simple_list() {
        let yaml = r"
services:
  web:
    image: nginx
    depends_on:
      - api
      - db
  api:
    image: myapi
  db:
    image: postgres
";
        let compose: ComposeFile = serde_yaml::from_str(yaml).unwrap();
        let web = &compose.services["web"];
        assert_eq!(web.depends_on.len(), 2);
        assert_eq!(
            web.depends_on["api"].condition,
            DependsOnCondition::ServiceStarted
        );
        assert_eq!(
            web.depends_on["db"].condition,
            DependsOnCondition::ServiceStarted
        );
    }

    #[test]
    fn test_port_long_syntax() {
        let yaml = r#"
services:
  app:
    image: myapp
    ports:
      - target: 80
        published: "8080"
        protocol: tcp
        host_ip: "127.0.0.1"
"#;
        let compose: ComposeFile = serde_yaml::from_str(yaml).unwrap();
        let app = &compose.services["app"];
        assert_eq!(app.ports.len(), 1);
        // Long syntax gets deserialized into ComposePort fields.
        // The target becomes container_port, published becomes host_port.
        assert_eq!(app.ports[0].container_port, "80");
    }

    #[test]
    fn test_volume_long_syntax() {
        let yaml = r"
services:
  app:
    image: myapp
    volumes:
      - type: bind
        source: ./data
        target: /app/data
        read_only: true
";
        let compose: ComposeFile = serde_yaml::from_str(yaml).unwrap();
        let app = &compose.services["app"];
        assert_eq!(app.volumes.len(), 1);
        assert_eq!(app.volumes[0].volume_type, Some(VolumeType::Bind));
        assert_eq!(app.volumes[0].source.as_deref(), Some("./data"));
        assert_eq!(app.volumes[0].target.as_deref(), Some("/app/data"));
        assert!(app.volumes[0].read_only);
    }
}
