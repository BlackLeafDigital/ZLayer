//! `ZLayer` V1 Service Specification Types
//!
//! This module defines all types for parsing and validating `ZLayer` deployment specs.

mod duration {
    use humantime::format_duration;
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::Duration;

    #[allow(clippy::ref_option)]
    pub fn serialize<S>(duration: &Option<Duration>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match duration {
            Some(d) => serializer.serialize_str(&format_duration(*d).to_string()),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<Duration>, D::Error>
    where
        D: Deserializer<'de>,
    {
        use serde::de::Error;
        let s: Option<String> = Option::deserialize(deserializer)?;
        match s {
            Some(s) => humantime::parse_duration(&s)
                .map(Some)
                .map_err(|e| D::Error::custom(format!("invalid duration: {e}"))),
            None => Ok(None),
        }
    }

    pub mod option {
        pub use super::*;
    }

    /// Serde module for required (non-Option) Duration fields
    pub mod required {
        use humantime::format_duration;
        use serde::{Deserialize, Deserializer, Serializer};
        use std::time::Duration;

        pub fn serialize<S>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            serializer.serialize_str(&format_duration(*duration).to_string())
        }

        pub fn deserialize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
        where
            D: Deserializer<'de>,
        {
            use serde::de::Error;
            let s: String = String::deserialize(deserializer)?;
            humantime::parse_duration(&s)
                .map_err(|e| D::Error::custom(format!("invalid duration: {e}")))
        }
    }
}

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use validator::Validate;

/// How service replicas are allocated to nodes
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeMode {
    /// Containers placed on any node with capacity (default, bin-packing)
    #[default]
    Shared,
    /// Each replica gets its own dedicated node (1:1 mapping)
    Dedicated,
    /// Service is the ONLY thing on its nodes (no other services)
    Exclusive,
}

/// Service type - determines runtime behavior and scaling model
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceType {
    /// Standard long-running container service
    #[default]
    Standard,
    /// WASM-based HTTP service (wasi:http/incoming-handler)
    WasmHttp,
    /// WASM-based general plugin (zlayer:plugin handler - full host access)
    WasmPlugin,
    /// WASM-based stateless request/response transformer
    WasmTransformer,
    /// WASM-based authenticator plugin (secrets + KV + HTTP)
    WasmAuthenticator,
    /// WASM-based rate limiter (KV + metrics)
    WasmRateLimiter,
    /// WASM-based request/response middleware
    WasmMiddleware,
    /// WASM-based custom router
    WasmRouter,
    /// Run-to-completion job
    Job,
}

/// Storage performance tier
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum StorageTier {
    /// Direct local filesystem (SSD/NVMe) - SQLite-safe, fast fsync
    #[default]
    Local,
    /// bcache-backed tiered storage (SSD cache + slower backend)
    Cached,
    /// NFS/network storage - NOT SQLite-safe (will warn)
    Network,
}

/// Node selection constraints for service placement
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeSelector {
    /// Required labels that nodes must have (all must match)
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub labels: HashMap<String, String>,
    /// Preferred labels (soft constraint, nodes with these are preferred)
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub prefer_labels: HashMap<String, String>,
}

/// Affinity hint for a single replica group's placement.
///
/// Three behaviors:
/// - `Spread`: try to put each replica on a different node (default).
/// - `Pack`: bin-pack onto the fewest nodes that can fit.
/// - `Pin`: pin all replicas to a single node, identified either by
///   node id (`"id=2"`) or label match (`"role=database"`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum GroupAffinity {
    /// Default: spread across distinct nodes.
    #[default]
    Spread,
    /// Pack onto fewest nodes.
    Pack,
    /// Pin to a specific node selector.
    ///
    /// Examples:
    /// - `Pin("id=2")` — exact node id match
    /// - `Pin("zone=us-east-1a")` — label match
    Pin(String),
}

/// Regex for [`ReplicaGroup::role`] validation. A valid DNS label: starts with
/// a lowercase letter, then any mix of lowercase letters, digits, or
/// internal hyphens, ending with a letter or digit. 1-30 chars total.
static REPLICA_GROUP_ROLE_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"^[a-z]([a-z0-9-]{0,28}[a-z0-9])?$").expect("valid regex literal")
});

/// One named replica group within a service.
///
/// When `ServiceSpec.replica_groups` is set, the service is composed of one
/// or more groups, each with its own count, optional overrides, and
/// affinity hint. Containers in each group get DNS names of the form
/// `<role>.<service>.<deployment>.zlayer.internal` and proxy backends
/// can target a single role via `EndpointSpec.target_role`.
///
/// Backward compat: services without `replica_groups` are treated as a
/// single implicit group `{role: "default", count: <scale.replicas>}`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Validate)]
#[serde(deny_unknown_fields)]
pub struct ReplicaGroup {
    /// Group identifier. Becomes part of container IDs and DNS names.
    /// Must be a valid DNS label: lowercase letters, digits, and hyphens;
    /// must not start or end with a hyphen; ≤ 30 chars.
    #[validate(length(min = 1, max = 30))]
    #[validate(regex(path = *REPLICA_GROUP_ROLE_RE))]
    pub role: String,

    /// Number of replicas in this group.
    #[validate(range(min = 1))]
    pub count: u32,

    /// Image override (inherits `ServiceSpec.image` when None).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<ImageSpec>,

    /// Environment variables MERGED on top of `ServiceSpec.env`. Entries
    /// in this map win on conflict (group overrides service default).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub env: HashMap<String, String>,

    /// Command override (inherits `ServiceSpec.command` when None).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<CommandSpec>,

    /// Resources override (inherits `ServiceSpec.resources` when None).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resources: Option<ResourcesSpec>,

    /// Affinity hint for placement of this group's replicas.
    #[serde(default)]
    pub affinity: GroupAffinity,
}

/// Validate that no two [`ReplicaGroup`]s share the same `role` within a
/// single [`ServiceSpec`].
///
/// Called from the deploy handler before storing the spec; not wired into
/// the `Validate` derive on `ServiceSpec` because validator 0.19's `custom`
/// only sees the field type (`Option<Vec<ReplicaGroup>>`) and not the
/// surrounding struct.
///
/// # Errors
/// Returns the duplicated role name on first collision.
pub fn validate_unique_replica_group_roles(groups: &[ReplicaGroup]) -> Result<(), String> {
    let mut seen = std::collections::HashSet::new();
    for g in groups {
        if !seen.insert(g.role.as_str()) {
            return Err(g.role.clone());
        }
    }
    Ok(())
}

/// Operating system a service needs to run on.
///
/// Mirrors the OS half of an OCI platform descriptor. Canonical wire strings
/// match Go's `GOOS` values (e.g. `"linux"`, `"windows"`, `"darwin"`).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, utoipa::ToSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum OsKind {
    Linux,
    Windows,
    Macos,
}

impl OsKind {
    /// Canonical OCI-style string (`"linux"` / `"windows"` / `"darwin"`).
    /// This is the same convention `Runtime.platform_resolver` uses.
    #[must_use]
    pub const fn as_oci_str(self) -> &'static str {
        match self {
            OsKind::Linux => "linux",
            OsKind::Windows => "windows",
            OsKind::Macos => "darwin",
        }
    }

    /// Detect from `std::env::consts::OS`. Unknown values return `None`.
    #[must_use]
    pub fn from_rust_os(s: &str) -> Option<Self> {
        match s {
            "linux" => Some(Self::Linux),
            "windows" => Some(Self::Windows),
            "macos" => Some(Self::Macos),
            _ => None,
        }
    }

    /// Parse the OCI-canonical OS string as written in an image manifest's
    /// `config.os` field (lowercase: `"linux"` / `"windows"` / `"darwin"`).
    /// Unknown or empty values return `None`.
    ///
    /// This is the inverse of [`Self::as_oci_str`] and is used by the
    /// registry's manifest-OS inspection (see `fetch_image_os`).
    #[must_use]
    pub fn from_oci_str(s: &str) -> Option<Self> {
        match s {
            "linux" => Some(Self::Linux),
            "windows" => Some(Self::Windows),
            "darwin" => Some(Self::Macos),
            _ => None,
        }
    }
}

/// CPU architecture a service needs. Mirrors the arch half of an OCI platform.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, utoipa::ToSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum ArchKind {
    Amd64,
    Arm64,
}

impl ArchKind {
    /// Canonical OCI-style string (`"amd64"` / `"arm64"`).
    #[must_use]
    pub const fn as_oci_str(self) -> &'static str {
        match self {
            ArchKind::Amd64 => "amd64",
            ArchKind::Arm64 => "arm64",
        }
    }

    /// Detect from `std::env::consts::ARCH`. Unknown values return `None`.
    #[must_use]
    pub fn from_rust_arch(s: &str) -> Option<Self> {
        match s {
            "x86_64" => Some(Self::Amd64),
            "aarch64" => Some(Self::Arm64),
            _ => None,
        }
    }
}

/// Platform a service targets. `None` on `ServiceSpec.platform` means
/// "any agent is acceptable" (preserves backward compatibility).
//
// NOTE: no `Copy`. `os_version: Option<String>` rules it out. `OsKind` / `ArchKind`
// are still `Copy`, so field-level borrows stay ergonomic.
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, utoipa::ToSchema,
)]
pub struct TargetPlatform {
    pub os: OsKind,
    pub arch: ArchKind,
    /// Optional OS version constraint — primarily for Windows multi-platform
    /// images, where `platform.os.version` in the OCI index distinguishes build
    /// families (e.g. `10.0.26100.*` for Server 2025 / Win11 24H2,
    /// `10.0.20348.*` for Server 2022). When set on a Windows target the
    /// registry platform resolver prefers manifest entries whose `os.version`
    /// matches this value exactly or shares a `major.minor.build` prefix.
    /// Unused on Linux/macOS platforms.
    #[serde(default, rename = "osVersion", skip_serializing_if = "Option::is_none")]
    pub os_version: Option<String>,
}

impl TargetPlatform {
    #[must_use]
    pub const fn new(os: OsKind, arch: ArchKind) -> Self {
        Self {
            os,
            arch,
            os_version: None,
        }
    }

    /// Constrain the platform to a specific `os.version` string.
    ///
    /// Applies to Windows targets: the registry resolver matches manifest
    /// entries whose `platform.os.version` equals this value or starts with it
    /// (treated as a `major.minor.build` prefix). Has no effect on Linux/macOS.
    #[must_use]
    pub fn with_os_version(mut self, v: impl Into<String>) -> Self {
        self.os_version = Some(v.into());
        self
    }

    /// Canonical OCI-style string (`"linux/amd64"`, `"windows/arm64"`).
    ///
    /// Does NOT include `os_version` — use [`Self::as_detailed_str`] when the
    /// version matters (e.g. for error/log messages that need to distinguish
    /// between Windows build families).
    #[must_use]
    pub fn as_oci_str(self) -> String {
        format!("{}/{}", self.os.as_oci_str(), self.arch.as_oci_str())
    }

    /// Like [`Self::as_oci_str`] but appends ` (os.version=…)` when an
    /// `os_version` constraint is set. Intended for diagnostics, not for
    /// matching against manifest entries.
    #[must_use]
    pub fn as_detailed_str(&self) -> String {
        match &self.os_version {
            Some(v) => format!(
                "{}/{} (os.version={v})",
                self.os.as_oci_str(),
                self.arch.as_oci_str()
            ),
            None => format!("{}/{}", self.os.as_oci_str(), self.arch.as_oci_str()),
        }
    }
}

impl std::fmt::Display for TargetPlatform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.os.as_oci_str(), self.arch.as_oci_str())
    }
}

/// Explicit capability declarations for WASM modules.
/// Controls which host interfaces are linked and available to the component.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct WasmCapabilities {
    /// Config interface access (zlayer:plugin/config)
    #[serde(default = "default_true")]
    pub config: bool,
    /// Key-value storage access (zlayer:plugin/keyvalue)
    #[serde(default = "default_true")]
    pub keyvalue: bool,
    /// Logging access (zlayer:plugin/logging)
    #[serde(default = "default_true")]
    pub logging: bool,
    /// Secrets access (zlayer:plugin/secrets)
    #[serde(default)]
    pub secrets: bool,
    /// Metrics emission (zlayer:plugin/metrics)
    #[serde(default = "default_true")]
    pub metrics: bool,
    /// HTTP client for outgoing requests (wasi:http/outgoing-handler)
    #[serde(default)]
    pub http_client: bool,
    /// WASI CLI access (args, env, stdio)
    #[serde(default)]
    pub cli: bool,
    /// WASI filesystem access
    #[serde(default)]
    pub filesystem: bool,
    /// WASI sockets access (TCP/UDP)
    #[serde(default)]
    pub sockets: bool,
}

impl Default for WasmCapabilities {
    fn default() -> Self {
        Self {
            config: true,
            keyvalue: true,
            logging: true,
            secrets: false,
            metrics: true,
            http_client: false,
            cli: false,
            filesystem: false,
            sockets: false,
        }
    }
}

/// Pre-opened directory for WASM filesystem access
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WasmPreopen {
    /// Host path to mount
    pub source: String,
    /// Guest path (visible to WASM module)
    pub target: String,
    /// Read-only access (default: false)
    #[serde(default)]
    pub readonly: bool,
}

/// Comprehensive configuration for all WASM service types.
///
/// Replaces the previous `WasmHttpConfig` with resource limits, capability
/// declarations, networking controls, and storage configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct WasmConfig {
    // --- Instance Management ---
    /// Minimum number of warm instances to keep ready
    #[serde(default = "default_min_instances")]
    pub min_instances: u32,
    /// Maximum number of instances to scale to
    #[serde(default = "default_max_instances")]
    pub max_instances: u32,
    /// Time before idle instances are terminated
    #[serde(default = "default_idle_timeout", with = "duration::required")]
    pub idle_timeout: std::time::Duration,
    /// Maximum time for a single request
    #[serde(default = "default_request_timeout", with = "duration::required")]
    pub request_timeout: std::time::Duration,

    // --- Resource Limits ---
    /// Maximum linear memory (e.g., "64Mi", "256Mi")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_memory: Option<String>,
    /// Maximum fuel (instruction count limit, 0 = unlimited)
    #[serde(default)]
    pub max_fuel: u64,
    /// Epoch interval for cooperative preemption
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "duration::option"
    )]
    pub epoch_interval: Option<std::time::Duration>,

    // --- Capabilities ---
    /// Explicit capability grants (overrides world defaults when restricting)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<WasmCapabilities>,

    // --- Networking ---
    /// Allow outgoing HTTP requests (default: true)
    #[serde(default = "default_true")]
    pub allow_http_outgoing: bool,
    /// Allowed outgoing HTTP hosts (empty = all allowed)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_hosts: Vec<String>,
    /// Allow raw TCP sockets (default: false)
    #[serde(default)]
    pub allow_tcp: bool,
    /// Allow raw UDP sockets (default: false)
    #[serde(default)]
    pub allow_udp: bool,

    // --- Storage ---
    /// Pre-opened directories (host path -> guest path)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub preopens: Vec<WasmPreopen>,
    /// Enable KV store access (default: true)
    #[serde(default = "default_true")]
    pub kv_enabled: bool,
    /// KV store namespace (default: service name)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kv_namespace: Option<String>,
    /// KV store max value size in bytes (default: 1MB)
    #[serde(default = "default_kv_max_value_size")]
    pub kv_max_value_size: u64,

    // --- Secrets ---
    /// Secret names accessible to this WASM module
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secrets: Vec<String>,

    // --- Performance ---
    /// Pre-compile on deploy to reduce cold start (default: true)
    #[serde(default = "default_true")]
    pub precompile: bool,
}

fn default_kv_max_value_size() -> u64 {
    1_048_576 // 1MB
}

impl Default for WasmConfig {
    fn default() -> Self {
        Self {
            min_instances: default_min_instances(),
            max_instances: default_max_instances(),
            idle_timeout: default_idle_timeout(),
            request_timeout: default_request_timeout(),
            max_memory: None,
            max_fuel: 0,
            epoch_interval: None,
            capabilities: None,
            allow_http_outgoing: true,
            allowed_hosts: Vec::new(),
            allow_tcp: false,
            allow_udp: false,
            preopens: Vec::new(),
            kv_enabled: true,
            kv_namespace: None,
            kv_max_value_size: default_kv_max_value_size(),
            secrets: Vec::new(),
            precompile: true,
        }
    }
}

/// Configuration for WASM HTTP services with instance pooling
#[deprecated(note = "Use WasmConfig instead")]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WasmHttpConfig {
    /// Minimum number of warm instances to keep ready
    #[serde(default = "default_min_instances")]
    pub min_instances: u32,
    /// Maximum number of instances to scale to
    #[serde(default = "default_max_instances")]
    pub max_instances: u32,
    /// Time before idle instances are terminated
    #[serde(default = "default_idle_timeout", with = "duration::required")]
    pub idle_timeout: std::time::Duration,
    /// Maximum time for a single request
    #[serde(default = "default_request_timeout", with = "duration::required")]
    pub request_timeout: std::time::Duration,
}

fn default_min_instances() -> u32 {
    0
}

fn default_max_instances() -> u32 {
    10
}

fn default_idle_timeout() -> std::time::Duration {
    std::time::Duration::from_secs(300)
}

fn default_request_timeout() -> std::time::Duration {
    std::time::Duration::from_secs(30)
}

#[allow(deprecated)]
impl Default for WasmHttpConfig {
    fn default() -> Self {
        Self {
            min_instances: default_min_instances(),
            max_instances: default_max_instances(),
            idle_timeout: default_idle_timeout(),
            request_timeout: default_request_timeout(),
        }
    }
}

#[allow(deprecated)]
impl From<WasmHttpConfig> for WasmConfig {
    fn from(old: WasmHttpConfig) -> Self {
        Self {
            min_instances: old.min_instances,
            max_instances: old.max_instances,
            idle_timeout: old.idle_timeout,
            request_timeout: old.request_timeout,
            ..Default::default()
        }
    }
}

impl ServiceType {
    /// Returns true if this is any WASM service type
    #[must_use]
    pub fn is_wasm(&self) -> bool {
        matches!(
            self,
            ServiceType::WasmHttp
                | ServiceType::WasmPlugin
                | ServiceType::WasmTransformer
                | ServiceType::WasmAuthenticator
                | ServiceType::WasmRateLimiter
                | ServiceType::WasmMiddleware
                | ServiceType::WasmRouter
        )
    }

    /// Returns the default capabilities for this WASM service type.
    /// Returns None for non-WASM types.
    #[must_use]
    pub fn default_wasm_capabilities(&self) -> Option<WasmCapabilities> {
        match self {
            ServiceType::WasmHttp | ServiceType::WasmRouter => Some(WasmCapabilities {
                config: true,
                keyvalue: true,
                logging: true,
                secrets: false,
                metrics: false,
                http_client: true,
                cli: false,
                filesystem: false,
                sockets: false,
            }),
            ServiceType::WasmPlugin => Some(WasmCapabilities {
                config: true,
                keyvalue: true,
                logging: true,
                secrets: true,
                metrics: true,
                http_client: true,
                cli: true,
                filesystem: true,
                sockets: false,
            }),
            ServiceType::WasmTransformer => Some(WasmCapabilities {
                config: false,
                keyvalue: false,
                logging: true,
                secrets: false,
                metrics: false,
                http_client: false,
                cli: true,
                filesystem: false,
                sockets: false,
            }),
            ServiceType::WasmAuthenticator => Some(WasmCapabilities {
                config: true,
                keyvalue: false,
                logging: true,
                secrets: true,
                metrics: false,
                http_client: true,
                cli: false,
                filesystem: false,
                sockets: false,
            }),
            ServiceType::WasmRateLimiter => Some(WasmCapabilities {
                config: true,
                keyvalue: true,
                logging: true,
                secrets: false,
                metrics: true,
                http_client: false,
                cli: true,
                filesystem: false,
                sockets: false,
            }),
            ServiceType::WasmMiddleware => Some(WasmCapabilities {
                config: true,
                keyvalue: false,
                logging: true,
                secrets: false,
                metrics: false,
                http_client: true,
                cli: false,
                filesystem: false,
                sockets: false,
            }),
            _ => None,
        }
    }
}

fn default_api_bind() -> String {
    "0.0.0.0:3669".to_string()
}

/// API server configuration (embedded in deploy/up flows)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ApiSpec {
    /// Enable the API server (default: true)
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Bind address (default: "0.0.0.0:3669")
    #[serde(default = "default_api_bind")]
    pub bind: String,
    /// JWT secret (reads `ZLAYER_JWT_SECRET` env var if not set)
    #[serde(default)]
    pub jwt_secret: Option<String>,
    /// Enable Swagger UI (default: true)
    #[serde(default = "default_true")]
    pub swagger: bool,
}

impl Default for ApiSpec {
    fn default() -> Self {
        Self {
            enabled: true,
            bind: default_api_bind(),
            jwt_secret: None,
            swagger: true,
        }
    }
}

/// Top-level deployment specification
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Validate)]
#[serde(deny_unknown_fields)]
pub struct DeploymentSpec {
    /// Spec version (must be "v1")
    #[validate(custom(function = "crate::spec::validate::validate_version_wrapper"))]
    pub version: String,

    /// Deployment name (used for overlays, DNS)
    #[validate(custom(function = "crate::spec::validate::validate_deployment_name_wrapper"))]
    pub deployment: String,

    /// Service definitions
    #[serde(default)]
    #[validate(nested)]
    pub services: HashMap<String, ServiceSpec>,

    /// External service definitions (proxy backends without containers)
    ///
    /// External services register static backend addresses with the proxy
    /// for host/path-based routing without starting any containers.
    /// Useful for proxying to services running outside of `ZLayer`
    /// (e.g., on other machines reachable via VPN).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    #[validate(nested)]
    pub externals: HashMap<String, ExternalSpec>,

    /// Top-level tunnel definitions (not tied to service endpoints)
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub tunnels: HashMap<String, TunnelDefinition>,

    /// API server configuration (enabled by default)
    #[serde(default)]
    pub api: ApiSpec,
}

/// External service specification (proxy backend without a container)
///
/// Defines a service that is not managed by `ZLayer` but should be proxied
/// through `ZLayer`'s reverse proxy. The proxy registers static backend
/// addresses and routes traffic based on endpoint host/path matching.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Validate)]
#[serde(deny_unknown_fields)]
pub struct ExternalSpec {
    /// Static backend addresses (e.g., `["100.64.1.5:8096", "192.168.1.10:8096"]`)
    ///
    /// These are the upstream addresses the proxy will forward traffic to.
    /// At least one backend is required.
    #[validate(length(min = 1, message = "at least one backend address is required"))]
    pub backends: Vec<String>,

    /// Endpoint definitions (proxy bindings)
    ///
    /// Defines how public/internal traffic is routed to this external service.
    #[serde(default)]
    #[validate(nested)]
    pub endpoints: Vec<EndpointSpec>,

    /// Health check configuration
    ///
    /// When specified, the proxy will health-check backends and remove
    /// unhealthy ones from the rotation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health: Option<HealthSpec>,
}

/// Top-level tunnel definition (not tied to a service endpoint)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TunnelDefinition {
    /// Source node
    pub from: String,

    /// Destination node
    pub to: String,

    /// Local port on source
    pub local_port: u16,

    /// Remote port on destination
    pub remote_port: u16,

    /// Protocol (tcp/udp, defaults to tcp)
    #[serde(default)]
    pub protocol: TunnelProtocol,

    /// Exposure type (defaults to internal)
    #[serde(default)]
    pub expose: ExposeType,
}

/// Protocol for tunnel connections (tcp or udp only)
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum TunnelProtocol {
    #[default]
    Tcp,
    Udp,
}

/// Log output configuration for services and jobs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LogsConfig {
    /// Where to write logs: "disk" (default) or "memory"
    #[serde(default = "default_logs_destination")]
    pub destination: String,

    /// Maximum log size in bytes (default: 100MB)
    #[serde(default = "default_logs_max_size")]
    pub max_size_bytes: u64,

    /// Log retention in seconds (default: 7 days)
    #[serde(default = "default_logs_retention")]
    pub retention_secs: u64,
}

fn default_logs_destination() -> String {
    "disk".to_string()
}

fn default_logs_max_size() -> u64 {
    100 * 1024 * 1024 // 100MB
}

fn default_logs_retention() -> u64 {
    7 * 24 * 60 * 60 // 7 days
}

impl Default for LogsConfig {
    fn default() -> Self {
        Self {
            destination: default_logs_destination(),
            max_size_bytes: default_logs_max_size(),
            retention_secs: default_logs_retention(),
        }
    }
}

/// Network mode for a service container.
///
/// Mirrors Docker's `HostConfig.NetworkMode` semantics. Accepts both an
/// enum-tagged form (e.g. `network_mode: { bridge: { name: my-net } }`) and a
/// string form (e.g. `"host"`, `"bridge:my-net"`, `"container:abc123"`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum NetworkMode {
    /// Default networking (overlay / bridge as configured by the platform).
    #[default]
    Default,
    /// Share the host network namespace (Docker `--network host`).
    Host,
    /// Disable networking entirely (Docker `--network none`).
    None,
    /// Attach to a Docker bridge network. When `name` is `None`, uses the
    /// default `bridge` network.
    Bridge {
        #[serde(default)]
        name: Option<String>,
    },
    /// Attach to another container's network namespace
    /// (Docker `--network container:<id>`).
    Container { id: String },
}

/// String-or-enum deserializer for [`NetworkMode`].
///
/// Accepts the same strings Docker accepts on `HostConfig.NetworkMode`:
/// `"default"`, `"host"`, `"none"`, `"bridge"`, `"bridge:<name>"`, and
/// `"container:<id>"`. Also accepts the enum-tagged YAML/JSON form produced by
/// the derived [`Serialize`] impl (e.g. `bridge: { name: my-net }`).
fn deserialize_network_mode<'de, D>(deserializer: D) -> Result<NetworkMode, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error;

    /// Inline mirror of [`NetworkMode`] used purely for the "object" form.
    /// We re-deserialize the captured YAML/JSON value into this and then map
    /// it back, which correctly drives `deserialize_enum` even when the input
    /// originally came from a `deserialize_any` path.
    #[derive(Deserialize)]
    #[serde(rename_all = "lowercase")]
    enum Inner {
        Default,
        Host,
        None,
        Bridge {
            #[serde(default)]
            name: Option<String>,
        },
        Container {
            id: String,
        },
    }

    impl From<Inner> for NetworkMode {
        fn from(i: Inner) -> Self {
            match i {
                Inner::Default => Self::Default,
                Inner::Host => Self::Host,
                Inner::None => Self::None,
                Inner::Bridge { name } => Self::Bridge { name },
                Inner::Container { id } => Self::Container { id },
            }
        }
    }

    // Capture the input as a self-describing serde value so we can branch
    // on whether it is a string (Docker-style) or an externally-tagged
    // enum (`{ bridge: { name } }`-style).
    let value = serde_yaml::Value::deserialize(deserializer)?;

    if let Some(s) = value.as_str() {
        return match s {
            "default" => Ok(NetworkMode::Default),
            "host" => Ok(NetworkMode::Host),
            "none" => Ok(NetworkMode::None),
            "bridge" => Ok(NetworkMode::Bridge { name: None }),
            _ => {
                if let Some(rest) = s.strip_prefix("bridge:") {
                    if rest.is_empty() {
                        Ok(NetworkMode::Bridge { name: None })
                    } else {
                        Ok(NetworkMode::Bridge {
                            name: Some(rest.to_string()),
                        })
                    }
                } else if let Some(rest) = s.strip_prefix("container:") {
                    if rest.is_empty() {
                        Err(D::Error::custom(
                            "network mode \"container:<id>\" requires a non-empty id",
                        ))
                    } else {
                        Ok(NetworkMode::Container {
                            id: rest.to_string(),
                        })
                    }
                } else {
                    Err(D::Error::custom(format!("unknown network mode: {s}")))
                }
            }
        };
    }

    let inner: Inner = serde_yaml::from_value(value).map_err(D::Error::custom)?;
    Ok(NetworkMode::from(inner))
}

/// Container isolation mode (Windows containers only; ignored on Linux/macOS).
///
/// * `Auto` (default) — runtime picks: Hyper-V on Windows client SKUs, Process on Server with matching build.
/// * `Process` — shared host kernel (fast, requires container OS build to match host).
/// * `Hyperv` — utility VM (stronger boundary, cross-version compatible).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum IsolationMode {
    #[default]
    Auto,
    Process,
    Hyperv,
}

/// Per-process resource limit (Docker `--ulimit` style).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UlimitSpec {
    /// Soft limit.
    #[serde(default)]
    pub soft: i64,
    /// Hard limit.
    #[serde(default)]
    pub hard: i64,
}

/// Per-service specification
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Validate)]
#[serde(from = "ServiceSpecCompat")]
#[allow(clippy::struct_excessive_bools)]
pub struct ServiceSpec {
    /// Resource type (service, job, cron)
    #[serde(default = "default_resource_type")]
    pub rtype: ResourceType,

    /// Cron schedule expression (only for rtype: cron)
    /// Uses 7-field cron syntax: "sec min hour day-of-month month day-of-week year"
    /// Examples:
    ///   - "0 0 0 * * * *" (daily at midnight)
    ///   - "0 */5 * * * * *" (every 5 minutes)
    ///   - "0 0 12 * * MON-FRI *" (weekdays at noon)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[validate(custom(function = "crate::spec::validate::validate_schedule_wrapper"))]
    pub schedule: Option<String>,

    /// Container image specification
    #[validate(nested)]
    pub image: ImageSpec,

    /// Resource limits
    #[serde(default)]
    #[validate(nested)]
    pub resources: ResourcesSpec,

    /// Environment variables for the service
    ///
    /// Values can be:
    /// - Plain strings: `"value"`
    /// - Host env refs: `$E:VAR_NAME`
    /// - Secret refs: `$S:secret-name` or `$S:@service/secret-name`
    #[serde(default)]
    pub env: HashMap<String, String>,

    /// Command override (entrypoint, args, workdir)
    #[serde(default)]
    pub command: CommandSpec,

    /// Network configuration
    #[serde(default)]
    pub network: ServiceNetworkSpec,

    /// Endpoint definitions (proxy bindings)
    #[serde(default)]
    #[validate(nested)]
    pub endpoints: Vec<EndpointSpec>,

    /// Scaling configuration
    #[serde(default)]
    #[validate(custom(function = "crate::spec::validate::validate_scale_spec"))]
    pub scale: ScaleSpec,

    /// Heterogeneous replica groups within this service.
    ///
    /// When set, the service is composed of multiple named groups (e.g.
    /// `primary` + `read` + `cache`) instead of a flat `scale.replicas`.
    /// Each group inherits `ServiceSpec` defaults (image, env, command,
    /// resources) and overrides per-group fields.
    ///
    /// When `None` (default), the service uses `scale` directly with an
    /// implicit single group `{role: "default", count: <scale.replicas>}`.
    /// This is the backward-compatible path used by all existing
    /// specifications.
    ///
    /// Cross-group role uniqueness is validated separately by
    /// [`validate_unique_replica_group_roles`] from the deploy handler.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[validate(nested)]
    pub replica_groups: Option<Vec<ReplicaGroup>>,

    /// Dependency specifications
    #[serde(default)]
    pub depends: Vec<DependsSpec>,

    /// Health check configuration
    #[serde(default = "default_health")]
    pub health: HealthSpec,

    /// Init actions (pre-start lifecycle steps)
    #[serde(default)]
    pub init: InitSpec,

    /// Error handling policies
    #[serde(default)]
    pub errors: ErrorsSpec,

    /// Container lifecycle policy (e.g., delete-on-exit).
    ///
    /// Purely declarative on this type; downstream layers (agent / API /
    /// scheduler) read this field to decide whether to clean up the
    /// container record after termination.
    #[serde(default)]
    pub lifecycle: LifecycleSpec,

    /// Container isolation mode (Windows containers only; ignored on Linux/macOS).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isolation: Option<IsolationMode>,

    /// Device passthrough (e.g., /dev/kvm for VMs)
    #[serde(default)]
    pub devices: Vec<DeviceSpec>,

    /// Storage mounts for the container
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub storage: Vec<StorageSpec>,

    /// Host-to-container port mappings (Docker's `-p host:container/proto`).
    ///
    /// Each entry publishes a container port on the host. When `host_port` is
    /// `None` (or zero), the daemon assigns an ephemeral host port.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub port_mappings: Vec<PortMapping>,

    /// Linux capabilities to add (e.g., `SYS_ADMIN`, `NET_ADMIN`).
    ///
    /// Also accepts the Docker-compatible alias `cap_add` on input.
    #[serde(default, alias = "cap_add", skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,

    /// Linux capabilities to drop (Docker `--cap-drop`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cap_drop: Vec<String>,

    /// Run container in privileged mode (all capabilities + all devices)
    #[serde(default)]
    pub privileged: bool,

    /// Node allocation mode (shared, dedicated, exclusive)
    #[serde(default)]
    pub node_mode: NodeMode,

    /// Node selection constraints (required/preferred labels)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_selector: Option<NodeSelector>,

    /// Placement affinity for this service's replicas when the service is NOT
    /// composed of `replica_groups` (each group carries its own affinity).
    ///
    /// `None` (the default) preserves historical shared-mode behavior:
    /// bin-pack / concentrate consecutive replicas onto the fewest nodes that
    /// fit. Set to `spread` for same-service anti-affinity (replicas land on
    /// distinct nodes for higher availability), `pack` to concentrate
    /// explicitly, or `pin` to bind all replicas to one node.
    ///
    /// Note: capacity always wins — a replica that does not fit on a node is
    /// placed elsewhere regardless of affinity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub affinity: Option<GroupAffinity>,

    /// Target platform for this service. When `None` (default), the service is
    /// eligible to run on any agent regardless of OS/architecture. When `Some`,
    /// the scheduler will only place replicas on agents whose platform matches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<TargetPlatform>,

    /// Service type (standard, `wasm_http`, `wasm_plugin`, etc.)
    #[serde(default)]
    pub service_type: ServiceType,

    /// WASM configuration (used when `service_type` is any Wasm* variant)
    /// Also accepts the deprecated `wasm_http` key for backward compatibility.
    #[serde(default, skip_serializing_if = "Option::is_none", alias = "wasm_http")]
    pub wasm: Option<WasmConfig>,

    /// Log output configuration. If not set, uses platform defaults.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logs: Option<LogsConfig>,

    /// Use host networking (container shares host network namespace)
    ///
    /// When true, the container will NOT get its own network namespace.
    /// This is set programmatically via the `--host-network` CLI flag, not in YAML specs.
    #[serde(skip)]
    pub host_network: bool,

    /// Container hostname (maps to Docker's `--hostname`).
    ///
    /// When set, the container's `/etc/hostname` and initial kernel hostname
    /// are configured to this value. Ignored when `host_network` is true
    /// (the container inherits the host's hostname).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,

    /// Additional DNS servers for the container (maps to Docker's `--dns`).
    ///
    /// Each entry must be a plausible IPv4 or IPv6 address. Forwarded to the
    /// container runtime as resolver addresses ahead of the platform defaults.
    /// Ignored when `host_network` is true.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dns: Vec<String>,

    /// Extra `hostname:ip` entries appended to `/etc/hosts` (maps to Docker's
    /// `--add-host`).
    ///
    /// Each entry must be in the form `"<hostname>:<ip>"`. The special literal
    /// `host-gateway` is accepted as the `<ip>` half (resolved by Docker /
    /// bollard to the host-visible gateway address, commonly used with
    /// `host.docker.internal:host-gateway`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra_hosts: Vec<String>,

    /// Container restart policy (Docker-style).
    ///
    /// Controls when the runtime should automatically restart the container
    /// after it exits. Maps to Docker's `HostConfig.RestartPolicy`. Named
    /// `ContainerRestartPolicy` to avoid colliding with `ZLayer`'s existing
    /// `PanicPolicy` (which controls post-panic behavior, not runtime-level
    /// restarts).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restart_policy: Option<ContainerRestartPolicy>,

    /// Free-form key/value labels attached to the container
    /// (Docker `--label`).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub labels: HashMap<String, String>,

    /// User and group override for the container's main process
    /// (Docker `--user uid:gid`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,

    /// Signal sent to the container's main process to request a graceful
    /// shutdown (Docker `--stop-signal`). Accepts e.g. `"SIGTERM"` or `"15"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_signal: Option<String>,

    /// Grace period to wait between the stop signal and a forced kill
    /// (Docker `--stop-timeout`).
    #[serde(
        default,
        with = "duration::option",
        skip_serializing_if = "Option::is_none"
    )]
    pub stop_grace_period: Option<std::time::Duration>,

    /// Kernel sysctl overrides (Docker `--sysctl`).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub sysctls: HashMap<String, String>,

    /// Per-process ulimits (Docker `--ulimit`).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub ulimits: HashMap<String, UlimitSpec>,

    /// Security options such as `apparmor=...`, `seccomp=...`,
    /// `no-new-privileges:true` (Docker `--security-opt`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub security_opt: Vec<String>,

    /// PID namespace mode (Docker `--pid`). Accepts e.g. `"host"` or
    /// `"container:<id>"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid_mode: Option<String>,

    /// IPC namespace mode (Docker `--ipc`). Accepts e.g. `"host"`,
    /// `"shareable"`, `"private"`, or `"container:<id>"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ipc_mode: Option<String>,

    /// Network mode (Docker `--network`). Accepts both the enum-tagged form
    /// and the Docker-style strings (`"host"`, `"none"`, `"bridge"`,
    /// `"bridge:<name>"`, `"container:<id>"`).
    #[serde(default, deserialize_with = "deserialize_network_mode")]
    pub network_mode: NetworkMode,

    /// Additional groups to add to the container process
    /// (Docker `--group-add`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra_groups: Vec<String>,

    /// Mount the container's root filesystem read-only (Docker `--read-only`).
    #[serde(default)]
    pub read_only_root_fs: bool,

    /// Run a Docker-supplied init process (PID 1) inside the container
    /// (Docker `--init`). Distinct from [`ServiceSpec::init`] which controls
    /// `ZLayer`'s pre-start init actions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub init_container: Option<bool>,

    /// Allocate a TTY for the container's main process (Docker `--tty`,
    /// compose `tty: true`).
    #[serde(default)]
    pub tty: bool,

    /// Keep STDIN open even when nothing is attached (Docker `--interactive`,
    /// compose `stdin_open: true`).
    #[serde(default)]
    pub stdin_open: bool,

    /// User namespace mode (Docker `--userns`). Accepts e.g. `"host"` or
    /// a remap-spec name configured on the daemon.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub userns_mode: Option<String>,

    /// Cgroup parent path (Docker `--cgroup-parent`). When set, the runtime
    /// places the container under the given cgroup hierarchy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cgroup_parent: Option<String>,

    /// Container ports exposed but not published to the host (compose
    /// `expose:`). Each entry is a port string, optionally `port/proto`
    /// (e.g. `"3000"`, `"8080/tcp"`). Treated as documentation by the
    /// runtime; downstream networking layers may use this list to allow
    /// inter-service traffic without publishing to the host.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub expose: Vec<String>,

    /// Per-service overlay-network configuration.
    ///
    /// When `None` (default), the daemon uses the cluster-level overlay
    /// default. When `Some`, the service opts into an explicit mode /
    /// parent. See [`crate::overlay::OverlayConfig`] for the v0.51
    /// implementation status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overlay: Option<crate::overlay::OverlayConfig>,
}

/// Deserialization shim for [`ServiceSpec`].
///
/// Mirrors `ServiceSpec`'s field shape so that the derived `Deserialize` impl
/// can pick up the YAML/JSON value, then [`From::from`] folds the deprecated
/// `host_network: bool` flag into the typed [`NetworkMode`] before handing the
/// finalized struct back to the caller.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
struct ServiceSpecCompat {
    #[serde(default = "default_resource_type")]
    rtype: ResourceType,
    #[serde(default)]
    schedule: Option<String>,
    image: ImageSpec,
    #[serde(default)]
    resources: ResourcesSpec,
    #[serde(default)]
    env: HashMap<String, String>,
    #[serde(default)]
    command: CommandSpec,
    #[serde(default)]
    network: ServiceNetworkSpec,
    #[serde(default)]
    endpoints: Vec<EndpointSpec>,
    #[serde(default)]
    scale: ScaleSpec,
    #[serde(default)]
    replica_groups: Option<Vec<ReplicaGroup>>,
    #[serde(default)]
    depends: Vec<DependsSpec>,
    #[serde(default = "default_health")]
    health: HealthSpec,
    #[serde(default)]
    init: InitSpec,
    #[serde(default)]
    errors: ErrorsSpec,
    #[serde(default)]
    lifecycle: LifecycleSpec,
    #[serde(default)]
    isolation: Option<IsolationMode>,
    #[serde(default)]
    devices: Vec<DeviceSpec>,
    #[serde(default)]
    storage: Vec<StorageSpec>,
    #[serde(default)]
    port_mappings: Vec<PortMapping>,
    #[serde(default, alias = "cap_add")]
    capabilities: Vec<String>,
    #[serde(default)]
    cap_drop: Vec<String>,
    #[serde(default)]
    privileged: bool,
    #[serde(default)]
    node_mode: NodeMode,
    #[serde(default)]
    node_selector: Option<NodeSelector>,
    #[serde(default)]
    affinity: Option<GroupAffinity>,
    #[serde(default)]
    platform: Option<TargetPlatform>,
    #[serde(default)]
    service_type: ServiceType,
    #[serde(default, alias = "wasm_http")]
    wasm: Option<WasmConfig>,
    #[serde(default)]
    logs: Option<LogsConfig>,
    /// Backwards-compat shim: when `host_network: true` is present in the input,
    /// it is folded into `network_mode = NetworkMode::Host` during conversion.
    #[serde(default)]
    host_network: Option<bool>,
    #[serde(default)]
    hostname: Option<String>,
    #[serde(default)]
    dns: Vec<String>,
    #[serde(default)]
    extra_hosts: Vec<String>,
    #[serde(default)]
    restart_policy: Option<ContainerRestartPolicy>,
    #[serde(default)]
    labels: HashMap<String, String>,
    #[serde(default)]
    user: Option<String>,
    #[serde(default)]
    stop_signal: Option<String>,
    #[serde(default, with = "duration::option")]
    stop_grace_period: Option<std::time::Duration>,
    #[serde(default)]
    sysctls: HashMap<String, String>,
    #[serde(default)]
    ulimits: HashMap<String, UlimitSpec>,
    #[serde(default)]
    security_opt: Vec<String>,
    #[serde(default)]
    pid_mode: Option<String>,
    #[serde(default)]
    ipc_mode: Option<String>,
    #[serde(default, deserialize_with = "deserialize_network_mode")]
    network_mode: NetworkMode,
    #[serde(default)]
    extra_groups: Vec<String>,
    #[serde(default)]
    read_only_root_fs: bool,
    #[serde(default)]
    init_container: Option<bool>,
    #[serde(default)]
    tty: bool,
    #[serde(default)]
    stdin_open: bool,
    #[serde(default)]
    userns_mode: Option<String>,
    #[serde(default)]
    cgroup_parent: Option<String>,
    #[serde(default)]
    expose: Vec<String>,
    #[serde(default)]
    overlay: Option<crate::overlay::OverlayConfig>,
}

impl From<ServiceSpecCompat> for ServiceSpec {
    fn from(c: ServiceSpecCompat) -> Self {
        // If the deprecated `host_network: true` flag is set, fold it into
        // the typed network mode unless the caller already supplied a
        // non-default value. This keeps existing in-process callers and
        // any legacy YAML that still emits `host_network: true` working.
        let network_mode = match (c.host_network, &c.network_mode) {
            (Some(true), NetworkMode::Default) => NetworkMode::Host,
            _ => c.network_mode,
        };
        let host_network = c.host_network.unwrap_or(false) || network_mode == NetworkMode::Host;

        Self {
            rtype: c.rtype,
            schedule: c.schedule,
            image: c.image,
            resources: c.resources,
            env: c.env,
            command: c.command,
            network: c.network,
            endpoints: c.endpoints,
            scale: c.scale,
            replica_groups: c.replica_groups,
            depends: c.depends,
            health: c.health,
            init: c.init,
            errors: c.errors,
            lifecycle: c.lifecycle,
            isolation: c.isolation,
            devices: c.devices,
            storage: c.storage,
            port_mappings: c.port_mappings,
            capabilities: c.capabilities,
            cap_drop: c.cap_drop,
            privileged: c.privileged,
            node_mode: c.node_mode,
            node_selector: c.node_selector,
            affinity: c.affinity,
            platform: c.platform,
            service_type: c.service_type,
            wasm: c.wasm,
            logs: c.logs,
            host_network,
            hostname: c.hostname,
            dns: c.dns,
            extra_hosts: c.extra_hosts,
            restart_policy: c.restart_policy,
            labels: c.labels,
            user: c.user,
            stop_signal: c.stop_signal,
            stop_grace_period: c.stop_grace_period,
            sysctls: c.sysctls,
            ulimits: c.ulimits,
            security_opt: c.security_opt,
            pid_mode: c.pid_mode,
            ipc_mode: c.ipc_mode,
            network_mode,
            extra_groups: c.extra_groups,
            read_only_root_fs: c.read_only_root_fs,
            init_container: c.init_container,
            tty: c.tty,
            stdin_open: c.stdin_open,
            userns_mode: c.userns_mode,
            cgroup_parent: c.cgroup_parent,
            expose: c.expose,
            overlay: c.overlay,
        }
    }
}

impl ServiceSpec {
    /// Construct a minimally-populated [`ServiceSpec`] with just the two
    /// fields callers always have to supply explicitly: the logical service
    /// name (used for diagnostics / labels at the call site — this struct
    /// does not carry the service name itself; it is the key in
    /// [`DeploymentSpec::services`]) and the container image. Every other
    /// field is filled in from [`Default::default`].
    ///
    /// Intended for tests and one-off in-memory fixtures. Production code
    /// paths that build a `ServiceSpec` from user input should still go
    /// through `serde` deserialization or an explicit struct literal so that
    /// every field is consciously set.
    ///
    /// # Examples
    /// ```ignore
    /// let spec = ServiceSpec::minimal("api", "ghcr.io/acme/api:1.2");
    /// ```
    ///
    /// # Panics
    /// Panics only if the fixed fallback string `"scratch:latest"` cannot
    /// be parsed as an [`ImageReference`] — which would indicate a bug in
    /// the OCI reference parser, not in caller input.
    #[must_use]
    pub fn minimal(_name: impl Into<String>, image: impl Into<String>) -> Self {
        use std::str::FromStr;
        let image_str = image.into();
        let image_ref = crate::ImageRef::from_str(&image_str).unwrap_or_else(|_| {
            crate::ImageRef::from_str("scratch:latest")
                .expect("'scratch:latest' is a valid image reference")
        });
        Self {
            image: ImageSpec {
                name: image_ref,
                pull_policy: default_pull_policy(),
            },
            ..Self::default()
        }
    }
}

/// Command override specification (Section 5.5)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
pub struct CommandSpec {
    /// Override image ENTRYPOINT
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entrypoint: Option<Vec<String>>,

    /// Override image CMD
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,

    /// Override working directory
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workdir: Option<String>,
}

fn default_resource_type() -> ResourceType {
    ResourceType::Service
}

fn default_health() -> HealthSpec {
    HealthSpec {
        start_grace: Some(std::time::Duration::from_secs(5)),
        interval: None,
        timeout: None,
        retries: 3,
        check: HealthCheck::Tcp { port: 0 },
    }
}

/// Resource type - determines container lifecycle
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ResourceType {
    /// Long-running container, receives traffic, load-balanced
    #[default]
    Service,
    /// Run-to-completion, triggered by endpoint/CLI/internal system
    Job,
    /// Scheduled run-to-completion, time-triggered
    Cron,
}

/// Container image specification
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Validate)]
#[serde(deny_unknown_fields)]
pub struct ImageSpec {
    /// Image name (e.g., "ghcr.io/org/api:latest")
    pub name: crate::ImageRef,

    /// When to pull the image
    #[serde(default = "default_pull_policy")]
    pub pull_policy: PullPolicy,
}

fn default_pull_policy() -> PullPolicy {
    PullPolicy::IfNotPresent
}

impl Default for ImageSpec {
    /// Placeholder default used by [`ServiceSpec::default`] (and downstream
    /// tests). The wrapped reference (`scratch:latest`) is not meaningful on
    /// its own — every real construction path should override this via
    /// [`ServiceSpec::minimal`] or an explicit literal. The point of having a
    /// `Default` is to make `ServiceSpec` itself `Default`-able so adding a new
    /// optional field on it does not force every existing literal site to be
    /// touched.
    fn default() -> Self {
        use std::str::FromStr;
        Self {
            name: crate::ImageRef::from_str("scratch:latest")
                .expect("'scratch:latest' is a valid image reference"),
            pull_policy: default_pull_policy(),
        }
    }
}

/// Image pull policy
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PullPolicy {
    /// Always pull the image, even if cached.
    Always,
    /// Resolve remote digest; pull and recreate when it differs from local/running.
    Newer,
    /// Use the local image if present; otherwise pull. Never contact a
    /// registry for revalidation when the image is already cached locally.
    /// This is the literal Docker/Kubernetes semantics — no silent upgrade
    /// to `Newer` for `:latest` tags (set `pull_policy: newer` explicitly
    /// when you want redeploy-picks-up-new-latest behavior).
    IfNotPresent,
    /// Never pull, use local image only.
    Never,
}

/// Device passthrough specification
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Validate, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DeviceSpec {
    /// Host device path (e.g., /dev/kvm, /dev/net/tun)
    #[validate(length(min = 1, message = "device path cannot be empty"))]
    pub path: String,

    /// Allow read access
    #[serde(default = "default_true")]
    pub read: bool,

    /// Allow write access
    #[serde(default = "default_true")]
    pub write: bool,

    /// Allow mknod (create device nodes)
    #[serde(default)]
    pub mknod: bool,
}

fn default_true() -> bool {
    true
}

/// Storage mount specification
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, tag = "type", rename_all = "snake_case")]
pub enum StorageSpec {
    /// Bind mount from host path to container
    Bind {
        source: String,
        target: String,
        #[serde(default)]
        readonly: bool,
    },
    /// Named persistent storage volume
    Named {
        name: String,
        target: String,
        #[serde(default)]
        readonly: bool,
        /// Performance tier (default: local, SQLite-safe)
        #[serde(default)]
        tier: StorageTier,
        /// Optional size limit (e.g., "1Gi", "512Mi")
        #[serde(default, skip_serializing_if = "Option::is_none")]
        size: Option<String>,
    },
    /// Anonymous storage (auto-named, container lifecycle)
    Anonymous {
        target: String,
        /// Performance tier (default: local)
        #[serde(default)]
        tier: StorageTier,
    },
    /// Memory-backed tmpfs mount
    Tmpfs {
        target: String,
        #[serde(default)]
        size: Option<String>,
        #[serde(default)]
        mode: Option<u32>,
    },
    /// S3-backed FUSE mount
    S3 {
        bucket: String,
        #[serde(default)]
        prefix: Option<String>,
        target: String,
        #[serde(default)]
        readonly: bool,
        #[serde(default)]
        endpoint: Option<String>,
        #[serde(default)]
        credentials: Option<String>,
    },
}

/// Resource limits (upper bounds, not reservations)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default, Validate)]
#[serde(deny_unknown_fields)]
pub struct ResourcesSpec {
    /// CPU limit (cores, e.g., 0.5, 1, 2)
    #[serde(default)]
    #[validate(custom(function = "crate::spec::validate::validate_cpu_option_wrapper"))]
    pub cpu: Option<f64>,

    /// Memory limit (e.g., "512Mi", "1Gi", "2Gi")
    #[serde(default)]
    #[validate(custom(function = "crate::spec::validate::validate_memory_option_wrapper"))]
    pub memory: Option<String>,

    /// GPU resource request
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu: Option<GpuSpec>,

    /// Maximum number of processes the container may spawn
    /// (Docker `--pids-limit`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pids_limit: Option<i64>,

    /// CPUs that the container is allowed to execute on (Docker `--cpuset-cpus`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpuset: Option<String>,

    /// Relative CPU shares (Docker `--cpu-shares`). Default weight is 1024.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_shares: Option<u32>,

    /// Total memory limit including swap (Docker `--memory-swap`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_swap: Option<String>,

    /// Soft memory limit (Docker `--memory-reservation`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_reservation: Option<String>,

    /// Container memory swappiness, 0-100 (Docker `--memory-swappiness`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_swappiness: Option<u8>,

    /// OOM-killer score adjustment (Docker `--oom-score-adj`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oom_score_adj: Option<i32>,

    /// Disable the OOM killer for the container (Docker `--oom-kill-disable`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oom_kill_disable: Option<bool>,

    /// Block IO weight, 10-1000 (Docker `--blkio-weight`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blkio_weight: Option<u16>,
}

/// Scheduling policy for GPU workloads
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum SchedulingPolicy {
    /// Place as many replicas as possible; partial placement is acceptable (default)
    #[default]
    BestEffort,
    /// All replicas must be placed or none are; prevents partial GPU job deployment
    Gang,
    /// Spread replicas across nodes to maximize GPU distribution
    Spread,
}

/// GPU sharing mode controlling how GPU resources are multiplexed.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum GpuSharingMode {
    /// Whole GPU per container (default). No sharing.
    #[default]
    Exclusive,
    /// NVIDIA Multi-Process Service: concurrent GPU compute sharing.
    /// Multiple containers run GPU kernels simultaneously with hardware isolation.
    Mps,
    /// NVIDIA time-slicing: round-robin GPU access across containers.
    /// Lower overhead than MPS but no concurrent execution.
    TimeSlice,
}

/// Configuration for distributed GPU job coordination.
///
/// When enabled on a multi-replica GPU service, `ZLayer` injects standard
/// distributed training environment variables (`MASTER_ADDR`, `MASTER_PORT`,
/// `WORLD_SIZE`, `RANK`, `LOCAL_RANK`) so frameworks like `PyTorch`, `Horovod`,
/// and `DeepSpeed` can coordinate automatically.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Validate)]
#[serde(deny_unknown_fields)]
pub struct DistributedConfig {
    /// Communication backend: "nccl" (default), "gloo", or "mpi"
    #[serde(default = "default_dist_backend")]
    pub backend: String,
    /// Port for rank-0 master coordination (default: 29500)
    #[serde(default = "default_dist_port")]
    pub master_port: u16,
}

fn default_dist_backend() -> String {
    "nccl".to_string()
}

fn default_dist_port() -> u16 {
    29500
}

/// GPU resource specification
///
/// Supported vendors:
/// - `nvidia` - NVIDIA GPUs via NVIDIA Container Toolkit (default)
/// - `amd` - AMD GPUs via `ROCm` (/dev/kfd + /dev/dri/renderD*)
/// - `intel` - Intel GPUs via VAAPI/i915 (/dev/dri/renderD*)
/// - `apple` - Apple Silicon GPUs via Metal/MPS (macOS only)
///
/// Unknown vendors fall back to DRI render node passthrough.
///
/// ## GPU mode (macOS only)
///
/// When `vendor` is `"apple"`, the `mode` field controls how GPU access is provided:
/// - `"native"` -- Seatbelt sandbox with direct Metal/MPS access (lowest overhead)
/// - `"vm"` -- libkrun micro-VM with GPU forwarding (stronger isolation)
/// - `None` (default) -- Auto-select based on platform and vendor
///
/// On Linux, `mode` is ignored; GPU passthrough always uses device node binding.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Validate)]
#[serde(deny_unknown_fields)]
pub struct GpuSpec {
    /// Number of GPUs to request
    #[serde(default = "default_gpu_count")]
    pub count: u32,
    /// GPU vendor (`nvidia`, `amd`, `intel`, `apple`) - defaults to `nvidia`
    #[serde(default = "default_gpu_vendor")]
    pub vendor: String,
    /// GPU access mode (macOS only): `"native"`, `"vm"`, or `None` for auto-select
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// Pin to a specific GPU model (e.g. "A100", "H100").
    /// Substring match against detected GPU model names.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Scheduling policy for GPU workloads.
    /// - `best-effort` (default): place what fits
    /// - `gang`: all-or-nothing for distributed jobs
    /// - `spread`: distribute across nodes
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheduling: Option<SchedulingPolicy>,
    /// Distributed GPU job coordination.
    /// When set, injects `MASTER_ADDR`, `WORLD_SIZE`, `RANK`, `LOCAL_RANK` env vars.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distributed: Option<DistributedConfig>,
    /// GPU sharing mode: exclusive (default), mps, or time-slice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sharing: Option<GpuSharingMode>,
    /// Host directory for the NVIDIA MPS control pipe.
    ///
    /// Only consulted when `sharing == Mps`. Defaults to `/tmp/nvidia-mps`
    /// when unset. The directory MUST exist on the host (created by the
    /// `nvidia-cuda-mps-control` daemon). It is bind-mounted into the
    /// container at the same path and exported as `CUDA_MPS_PIPE_DIRECTORY`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mps_pipe_dir: Option<String>,
    /// Host directory for NVIDIA MPS log output.
    ///
    /// Only consulted when `sharing == Mps`. Defaults to `/tmp/nvidia-log`
    /// when unset. The directory MUST exist on the host. It is bind-mounted
    /// into the container and exported as `CUDA_MPS_LOG_DIRECTORY`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mps_log_dir: Option<String>,
    /// CUDA device index this replica should see when `sharing == TimeSlice`.
    ///
    /// Emitted as `CUDA_VISIBLE_DEVICES=<slice_index>`, overriding the default
    /// 0..count visibility list. Use this together with a host-side NVIDIA
    /// time-slicing config to advertise a single physical GPU as multiple
    /// virtual slices.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_slice_index: Option<u32>,
    /// Optional host path to a NVIDIA time-slicing config YAML.
    ///
    /// When set, the file is bind-mounted read-only at
    /// `/etc/nvidia/gpu-time-slicing.yaml` inside the container so tools that
    /// inspect the slicing topology (e.g. monitoring sidecars) can read it.
    /// The file is not interpreted by `ZLayer` — it's purely informational for
    /// the workload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_slicing_config_path: Option<String>,
}

fn default_gpu_count() -> u32 {
    1
}

fn default_gpu_vendor() -> String {
    "nvidia".to_string()
}

/// Per-service network configuration (overlay + join policy).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[derive(Default)]
pub struct ServiceNetworkSpec {
    /// Overlay network configuration
    #[serde(default)]
    pub overlays: OverlayConfig,

    /// Join policy (who can join this service)
    #[serde(default)]
    pub join: JoinPolicy,
}

/// Overlay network configuration
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OverlayConfig {
    /// Service-scoped overlay (service replicas only)
    #[serde(default)]
    pub service: OverlaySettings,

    /// Global overlay (all services in deployment)
    #[serde(default)]
    pub global: OverlaySettings,
}

impl Default for OverlayConfig {
    fn default() -> Self {
        Self {
            service: OverlaySettings {
                enabled: true,
                encrypted: true,
                isolated: true,
            },
            global: OverlaySettings {
                enabled: true,
                encrypted: true,
                isolated: false,
            },
        }
    }
}

/// Overlay network settings
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
pub struct OverlaySettings {
    /// Enable this overlay
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Use encryption
    #[serde(default = "default_encrypted")]
    pub encrypted: bool,

    /// Isolate from other services/groups
    #[serde(default)]
    pub isolated: bool,
}

fn default_enabled() -> bool {
    true
}

fn default_encrypted() -> bool {
    true
}

/// Join policy - controls who can join a service
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct JoinPolicy {
    /// Join mode
    #[serde(default = "default_join_mode")]
    pub mode: JoinMode,

    /// Scope of join
    #[serde(default = "default_join_scope")]
    pub scope: JoinScope,
}

impl Default for JoinPolicy {
    fn default() -> Self {
        Self {
            mode: default_join_mode(),
            scope: default_join_scope(),
        }
    }
}

fn default_join_mode() -> JoinMode {
    JoinMode::Token
}

fn default_join_scope() -> JoinScope {
    JoinScope::Service
}

/// Join mode
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JoinMode {
    /// Any trusted node in deployment can self-enroll
    Open,
    /// Requires a join key (recommended)
    Token,
    /// Only control-plane/scheduler can place replicas
    Closed,
}

/// Join scope
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JoinScope {
    /// Join this specific service
    Service,
    /// Join all services in deployment
    Global,
}

/// Endpoint specification (proxy binding)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Validate)]
#[serde(deny_unknown_fields)]
pub struct EndpointSpec {
    /// Endpoint name (for routing)
    #[validate(length(min = 1, message = "endpoint name cannot be empty"))]
    pub name: String,

    /// Protocol
    pub protocol: Protocol,

    /// Proxy listen port (external-facing port)
    #[validate(custom(function = "crate::spec::validate::validate_port_wrapper"))]
    pub port: u16,

    /// Container port the service actually listens on.
    /// Defaults to `port` when not specified.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_port: Option<u16>,

    /// URL path prefix (for http/https/websocket)
    pub path: Option<String>,

    /// Host pattern for routing (e.g. "api.example.com" or "*.example.com").
    /// `None` means match any host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,

    /// Exposure type
    #[serde(default = "default_expose")]
    pub expose: ExposeType,

    /// Optional stream (L4) proxy configuration
    /// Only applicable when protocol is tcp or udp
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream: Option<StreamEndpointConfig>,

    /// Restrict this endpoint to backends in a specific replica role.
    ///
    /// When `Some`, only containers whose `replica_groups.role` matches this
    /// value receive traffic from this endpoint. When `None` (default), the
    /// endpoint accepts all containers of the service (legacy behavior).
    ///
    /// Validation: when set, the role MUST appear in the parent
    /// `ServiceSpec.replica_groups` (enforced at deploy time in the API
    /// handler, not via derive(Validate)).
    ///
    /// Example (a postgres service with primary + read replicas):
    ///
    /// ```yaml
    /// endpoints:
    ///   - name: write
    ///     port: 5432
    ///     protocol: tcp
    ///     target_role: primary
    ///   - name: read
    ///     port: 5433
    ///     protocol: tcp
    ///     target_role: read
    /// ```
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_role: Option<String>,

    /// Optional tunnel configuration for this endpoint
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tunnel: Option<EndpointTunnelConfig>,
}

impl EndpointSpec {
    /// Returns the port the container actually listens on.
    /// Falls back to `port` when `target_port` is not specified.
    #[must_use]
    pub fn target_port(&self) -> u16 {
        self.target_port.unwrap_or(self.port)
    }
}

/// Tunnel configuration for an endpoint
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
pub struct EndpointTunnelConfig {
    /// Enable tunneling for this endpoint
    #[serde(default)]
    pub enabled: bool,

    /// Source node name (defaults to service's node)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,

    /// Destination node name (defaults to cluster ingress)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,

    /// Remote port to expose (0 = auto-assign)
    #[serde(default)]
    pub remote_port: u16,

    /// Override exposure for tunnel (public/internal)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expose: Option<ExposeType>,

    /// On-demand access configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<TunnelAccessConfig>,
}

/// On-demand access settings for `zlayer tunnel access`
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
pub struct TunnelAccessConfig {
    /// Allow on-demand access via CLI
    #[serde(default)]
    pub enabled: bool,

    /// Maximum session duration (e.g., "4h", "30m")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_ttl: Option<String>,

    /// Log all access sessions
    #[serde(default)]
    pub audit: bool,
}

fn default_expose() -> ExposeType {
    ExposeType::Internal
}

/// Protocol type
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    Http,
    Https,
    Tcp,
    Udp,
    Websocket,
}

/// Exposure type
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ExposeType {
    Public,
    #[default]
    Internal,
}

/// Stream (L4) proxy configuration for TCP/UDP endpoints
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
pub struct StreamEndpointConfig {
    /// Enable TLS termination for TCP (auto-provision cert)
    #[serde(default)]
    pub tls: bool,

    /// Enable PROXY protocol for passing client IP
    #[serde(default)]
    pub proxy_protocol: bool,

    /// Custom session timeout for UDP (default: 60s)
    /// Format: duration string like "60s", "5m"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_timeout: Option<String>,

    /// Health check configuration for L4
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health_check: Option<StreamHealthCheck>,
}

/// Health check types for stream (L4) endpoints
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamHealthCheck {
    /// TCP connect check - verifies port is accepting connections
    TcpConnect,
    /// UDP probe - sends request and optionally validates response
    UdpProbe {
        /// Request payload to send (can use hex escapes like \\xFF)
        request: String,
        /// Expected response pattern (optional regex)
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expect: Option<String>,
    },
}

/// Scaling configuration
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "mode", rename_all = "lowercase", deny_unknown_fields)]
pub enum ScaleSpec {
    /// Adaptive scaling with metrics
    #[serde(rename = "adaptive")]
    Adaptive {
        /// Minimum replicas
        min: u32,

        /// Maximum replicas
        max: u32,

        /// Cooldown period between scale events
        #[serde(default, with = "duration::option")]
        cooldown: Option<std::time::Duration>,

        /// Target metrics for scaling
        #[serde(default)]
        targets: ScaleTargets,
    },

    /// Fixed number of replicas
    #[serde(rename = "fixed")]
    Fixed { replicas: u32 },

    /// Manual scaling (no automatic scaling)
    #[serde(rename = "manual")]
    Manual,
}

impl Default for ScaleSpec {
    fn default() -> Self {
        Self::Adaptive {
            min: 1,
            max: 10,
            cooldown: Some(std::time::Duration::from_secs(30)),
            targets: ScaleTargets::default(),
        }
    }
}

/// Target metrics for adaptive scaling
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[derive(Default)]
pub struct ScaleTargets {
    /// CPU percentage threshold (0-100)
    #[serde(default)]
    pub cpu: Option<u8>,

    /// Memory percentage threshold (0-100)
    #[serde(default)]
    pub memory: Option<u8>,

    /// Requests per second threshold
    #[serde(default)]
    pub rps: Option<u32>,
}

/// Dependency specification
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DependsSpec {
    /// Service name to depend on
    pub service: String,

    /// Condition for dependency
    #[serde(default = "default_condition")]
    pub condition: DependencyCondition,

    /// Maximum time to wait
    #[serde(default = "default_timeout", with = "duration::option")]
    pub timeout: Option<std::time::Duration>,

    /// Action on timeout
    #[serde(default = "default_on_timeout")]
    pub on_timeout: TimeoutAction,
}

fn default_condition() -> DependencyCondition {
    DependencyCondition::Healthy
}

#[allow(clippy::unnecessary_wraps)]
fn default_timeout() -> Option<std::time::Duration> {
    Some(std::time::Duration::from_secs(300))
}

fn default_on_timeout() -> TimeoutAction {
    TimeoutAction::Fail
}

/// Dependency condition
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DependencyCondition {
    /// Container process exists
    Started,
    /// Health check passes
    Healthy,
    /// Service is available for routing
    Ready,
}

/// Timeout action
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TimeoutAction {
    Fail,
    Warn,
    Continue,
}

/// Health check specification
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HealthSpec {
    /// Grace period before first check
    #[serde(default, with = "duration::option")]
    pub start_grace: Option<std::time::Duration>,

    /// Interval between checks
    #[serde(default, with = "duration::option")]
    pub interval: Option<std::time::Duration>,

    /// Timeout per check
    #[serde(default, with = "duration::option")]
    pub timeout: Option<std::time::Duration>,

    /// Number of retries before marking unhealthy
    #[serde(default = "default_retries")]
    pub retries: u32,

    /// Health check type and parameters
    pub check: HealthCheck,
}

fn default_retries() -> u32 {
    3
}

impl Default for HealthSpec {
    /// Returns the same shape as the per-field serde defaults: a 5-second
    /// start grace, 3 retries, and a TCP check against port 0 ("use first
    /// endpoint"). Matches [`default_health`] which is the serde fallback
    /// when no `health:` block is supplied in a deployment spec.
    fn default() -> Self {
        default_health()
    }
}

/// Health check type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum HealthCheck {
    /// TCP port check
    Tcp {
        /// Port to check (0 = use first endpoint)
        port: u16,
    },

    /// HTTP check
    Http {
        /// URL to check
        url: String,
        /// Expected status code
        #[serde(default = "default_expect_status")]
        expect_status: u16,
    },

    /// Command check
    Command {
        /// Command to run
        command: String,
    },
}

fn default_expect_status() -> u16 {
    200
}

/// Init actions specification
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[derive(Default)]
pub struct InitSpec {
    /// Init steps to run before container starts
    #[serde(default)]
    pub steps: Vec<InitStep>,
}

/// Lifecycle policy for service / job / cron containers.
///
/// Currently exposes a single `delete_on_exit` knob that, when `true`,
/// instructs higher layers to remove the container record (and its bundle)
/// once it has terminated. Other layers consume this field; this type is
/// purely descriptive.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct LifecycleSpec {
    /// When true, terminated containers (and their bundles) are removed
    /// automatically rather than retained for inspection. Defaults to
    /// `false`, preserving the historical retain-on-exit behavior.
    #[serde(default)]
    pub delete_on_exit: bool,
}

/// Init action step
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InitStep {
    /// Step identifier
    pub id: String,

    /// Action to perform (e.g., "`init.wait_tcp`")
    pub uses: String,

    /// Parameters for the action
    #[serde(default)]
    pub with: InitParams,

    /// Number of retries
    #[serde(default)]
    pub retry: Option<u32>,

    /// Maximum time for this step
    #[serde(default, with = "duration::option")]
    pub timeout: Option<std::time::Duration>,

    /// Action on failure
    #[serde(default = "default_on_failure")]
    pub on_failure: FailureAction,
}

fn default_on_failure() -> FailureAction {
    FailureAction::Fail
}

/// Init action parameters
pub type InitParams = std::collections::HashMap<String, serde_json::Value>;

/// Failure action for init steps
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FailureAction {
    Fail,
    Warn,
    Continue,
}

/// Error handling policies
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[derive(Default)]
pub struct ErrorsSpec {
    /// Init failure policy
    #[serde(default)]
    pub on_init_failure: InitFailurePolicy,

    /// Panic/restart policy
    #[serde(default)]
    pub on_panic: PanicPolicy,
}

/// Init failure policy
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InitFailurePolicy {
    #[serde(default = "default_init_action")]
    pub action: InitFailureAction,
}

impl Default for InitFailurePolicy {
    fn default() -> Self {
        Self {
            action: default_init_action(),
        }
    }
}

fn default_init_action() -> InitFailureAction {
    InitFailureAction::Fail
}

/// Init failure action
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum InitFailureAction {
    Fail,
    Restart,
    Backoff,
}

/// Panic policy
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PanicPolicy {
    #[serde(default = "default_panic_action")]
    pub action: PanicAction,
}

impl Default for PanicPolicy {
    fn default() -> Self {
        Self {
            action: default_panic_action(),
        }
    }
}

fn default_panic_action() -> PanicAction {
    PanicAction::Restart
}

/// Panic action
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PanicAction {
    Restart,
    Shutdown,
    Isolate,
}

// ==========================================================================
// Network / Access Control types
// ==========================================================================

/// A network policy defines an access control group with membership rules
/// and service access policies.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct NetworkPolicySpec {
    /// Unique network name.
    pub name: String,

    /// Human-readable description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// CIDR ranges that belong to this network (e.g., "10.200.0.0/16", "192.168.1.0/24").
    #[serde(default)]
    pub cidrs: Vec<String>,

    /// Named members (users, groups, nodes) of this network.
    #[serde(default)]
    pub members: Vec<NetworkMember>,

    /// Access rules defining which services this network can reach.
    #[serde(default)]
    pub access_rules: Vec<AccessRule>,
}

/// A member of a network.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NetworkMember {
    /// Member identifier (username, group name, node ID, or CIDR).
    pub name: String,
    /// Type of member.
    #[serde(default)]
    pub kind: MemberKind,
}

/// Type of network member.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum MemberKind {
    /// An individual user identity.
    #[default]
    User,
    /// A group of users.
    Group,
    /// A specific cluster node.
    Node,
    /// A CIDR range (redundant with NetworkPolicySpec.cidrs but allows per-member CIDR).
    Cidr,
}

/// An access rule determining what a network can reach.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AccessRule {
    /// Target service name, or "*" for all services.
    #[serde(default = "wildcard")]
    pub service: String,

    /// Target deployment name, or "*" for all deployments.
    #[serde(default = "wildcard")]
    pub deployment: String,

    /// Specific ports allowed. None means all ports.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ports: Option<Vec<u16>>,

    /// Whether to allow or deny access.
    #[serde(default)]
    pub action: AccessAction,
}

fn wildcard() -> String {
    "*".to_string()
}

/// Access control action.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum AccessAction {
    /// Allow access (default).
    #[default]
    Allow,
    /// Deny access.
    Deny,
}

// ==========================================================================
// Container bridge / overlay network types (Docker-compatible)
// ==========================================================================
//
// These types model user-defined bridge or overlay networks that standalone
// containers can attach to — the Docker-style "docker network create" model.
// They are intentionally named `BridgeNetwork*` to avoid colliding with the
// CIDR-ACL `NetworkPolicySpec` types above, which model a completely
// different concept (access-control groups).

/// A user-defined bridge or overlay network that containers can attach to.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, utoipa::ToSchema)]
pub struct BridgeNetwork {
    /// Opaque server-generated identifier (UUID v4).
    pub id: String,

    /// Human-readable, unique name (must match `^[a-z0-9][a-z0-9_-]{0,63}$`).
    pub name: String,

    /// Driver backing the network (bridge vs. overlay).
    #[serde(default)]
    pub driver: BridgeNetworkDriver,

    /// IPv4/IPv6 subnet in CIDR notation (e.g. `"10.240.0.0/24"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subnet: Option<String>,

    /// Arbitrary key/value labels for filtering and grouping.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub labels: HashMap<String, String>,

    /// If true, containers attached to this network cannot reach the outside
    /// world — only other containers on the same network.
    #[serde(default)]
    pub internal: bool,

    /// Creation timestamp (UTC, RFC 3339).
    #[schema(value_type = String, format = "date-time")]
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Backing driver for a [`BridgeNetwork`].
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum BridgeNetworkDriver {
    /// Linux bridge on the local host (single-host, default).
    #[default]
    Bridge,
    /// Overlay network spanning multiple hosts.
    Overlay,
}

/// A container attached to a [`BridgeNetwork`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, utoipa::ToSchema)]
pub struct BridgeNetworkAttachment {
    /// Runtime-provided container id.
    pub container_id: String,

    /// Container name, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container_name: Option<String>,

    /// DNS aliases the container can be reached by on this network.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,

    /// Assigned IPv4 address on the network (if any).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ipv4: Option<String>,
}

// ==========================================================================
// Registry auth (inline, not persisted) — §3.10 of ZLAYER_SDK_FIXES.md
// ==========================================================================
//
// Inline credentials a client can attach to a single pull or container-create
// request without first POSTing them to `/api/v1/credentials/registry`. The
// daemon uses them exactly once — they are never logged, never persisted, and
// never echoed back on a response.
//
// For requests that instead want to reuse an already-stored credential, the
// `CreateContainerRequest` / `PullImageRequest` DTOs also accept a
// `registry_credential_id` pointing at the `RegistryCredentialStore`. Inline
// `RegistryAuth` takes precedence when both are provided.

/// Inline Docker/OCI registry credentials attached to a single pull request.
///
/// Prefer persistent credentials via `/api/v1/credentials/registry` for
/// long-lived services. Use this inline form for one-off pulls (e.g. CI
/// runners fetching a private image for a single job) where persisting a
/// credential is undesirable.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, utoipa::ToSchema)]
pub struct RegistryAuth {
    /// Username for the registry (for basic auth) or a placeholder
    /// identifier when `auth_type == Token`.
    pub username: String,
    /// Password or bearer token. **Never** logged or returned on any
    /// response — consumed once and dropped.
    pub password: String,
    /// Which authentication scheme to use against the registry.
    #[serde(default = "default_registry_auth_type")]
    pub auth_type: RegistryAuthType,
}

/// Authentication scheme for a [`RegistryAuth`].
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RegistryAuthType {
    /// HTTP Basic authentication (username + password). Default.
    #[default]
    Basic,
    /// Bearer token authentication. `password` carries the token; `username`
    /// is typically a placeholder such as `"oauth2accesstoken"` or `"<token>"`.
    Token,
}

/// Serde default for [`RegistryAuth::auth_type`]. Kept as a free function so
/// `#[serde(default = "...")]` can reference it.
#[must_use]
pub fn default_registry_auth_type() -> RegistryAuthType {
    RegistryAuthType::Basic
}

// ==========================================================================
// Container restart policy (Docker-style) — §3.4 of ZLAYER_SDK_FIXES.md
// ==========================================================================
//
// Named `ContainerRestartPolicy` / `ContainerRestartKind` rather than
// `RestartPolicy` / `RestartKind` to avoid colliding with ZLayer's existing
// `PanicPolicy`/`PanicAction` types and to make the runtime-level (as opposed
// to panic-driven) nature of this policy explicit.

/// Container-runtime-level restart policy.
///
/// Maps onto Docker's `HostConfig.RestartPolicy`. Distinct from
/// [`PanicPolicy`], which governs what `ZLayer` does in response to an
/// application panic (it does not set a Docker restart policy).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, utoipa::ToSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct ContainerRestartPolicy {
    /// Which restart policy to apply.
    pub kind: ContainerRestartKind,

    /// For `on_failure` only: maximum number of restart attempts before
    /// giving up. Ignored by other kinds. `None` means "retry forever".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_attempts: Option<u32>,

    /// Humantime-formatted delay between restarts (e.g. `"500ms"`,
    /// `"2s"`). Accepted for forward-compatibility but currently ignored
    /// by the Docker backend: bollard's `RestartPolicy` has no per-kind
    /// delay field. When set, the runtime emits a warning.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delay: Option<String>,
}

/// Which flavor of container restart policy to apply.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContainerRestartKind {
    /// Never restart (Docker's `"no"`).
    No,
    /// Always restart (Docker's `"always"`).
    Always,
    /// Restart unless the user explicitly stopped the container
    /// (Docker's `"unless-stopped"`).
    UnlessStopped,
    /// Restart only when the container exits with a non-zero code
    /// (Docker's `"on-failure"`). Respects `max_attempts`.
    OnFailure,
}

// ==========================================================================
// Port mappings (Docker-style container port publishing)
// ==========================================================================

/// Transport protocol for a published container port.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PortProtocol {
    /// TCP (default).
    Tcp,
    /// UDP.
    Udp,
}

impl Default for PortProtocol {
    fn default() -> Self {
        default_port_protocol()
    }
}

impl PortProtocol {
    /// Return the lowercase string form Docker uses in port-binding keys
    /// (e.g. `"tcp"` or `"udp"`).
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            PortProtocol::Tcp => "tcp",
            PortProtocol::Udp => "udp",
        }
    }
}

fn default_port_protocol() -> PortProtocol {
    PortProtocol::Tcp
}

fn default_host_ip() -> String {
    "0.0.0.0".to_string()
}

/// A single host-to-container port publish rule (Docker's `-p`).
///
/// When `host_port` is `None` (or explicitly `Some(0)`), the container runtime
/// assigns an ephemeral host port. `host_ip` defaults to `"0.0.0.0"` to bind
/// on all interfaces.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub struct PortMapping {
    /// Host port. `None` (or zero) means "assign an ephemeral port".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_port: Option<u16>,
    /// Container-side port.
    pub container_port: u16,
    /// Transport protocol (defaults to TCP).
    #[serde(default = "default_port_protocol")]
    pub protocol: PortProtocol,
    /// Host interface to bind on. Defaults to `"0.0.0.0"` (all interfaces).
    #[serde(default = "default_host_ip", skip_serializing_if = "String::is_empty")]
    pub host_ip: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_spec_default_round_trips_through_json() {
        // Building `ServiceSpec::default()` must succeed (no panics on the
        // placeholder image reference) and the result must round-trip through
        // serde_json so callers can store / transport a default spec without
        // surprises.
        let spec = ServiceSpec::default();

        // Sanity on a handful of fields that depend on custom Default impls.
        assert_eq!(spec.rtype, ResourceType::Service);
        assert_eq!(spec.image.pull_policy, PullPolicy::IfNotPresent);
        assert_eq!(spec.health.retries, 3);
        assert_eq!(spec.network_mode, NetworkMode::Default);
        assert!(spec.env.is_empty());
        assert!(spec.endpoints.is_empty());
        assert!(spec.overlay.is_none());

        let json = serde_json::to_string(&spec).expect("serialize default ServiceSpec");
        let parsed: ServiceSpec =
            serde_json::from_str(&json).expect("re-parse default ServiceSpec");
        assert_eq!(spec, parsed);
    }

    #[test]
    fn service_spec_minimal_sets_name_and_image() {
        let spec = ServiceSpec::minimal("api", "ghcr.io/acme/api:1.2");
        assert_eq!(spec.image.name.repository(), "acme/api");
        assert_eq!(spec.image.name.tag(), Some("1.2"));
        // Everything else should match Default exactly.
        let baseline = ServiceSpec::default();
        assert_eq!(spec.rtype, baseline.rtype);
        assert_eq!(spec.scale, baseline.scale);
        assert_eq!(spec.network_mode, baseline.network_mode);
    }

    #[test]
    fn port_mapping_defaults_via_serde() {
        // Minimal JSON: only container_port. host_port omitted, protocol defaults
        // to "tcp", host_ip defaults to "0.0.0.0".
        let json = r#"{"container_port": 8080}"#;
        let m: PortMapping = serde_json::from_str(json).expect("parse minimal PortMapping");
        assert_eq!(m.container_port, 8080);
        assert_eq!(m.host_port, None);
        assert_eq!(m.protocol, PortProtocol::Tcp);
        assert_eq!(m.host_ip, "0.0.0.0");
    }

    #[test]
    fn port_mapping_skips_none_host_port_and_empty_host_ip() {
        let m = PortMapping {
            host_port: None,
            container_port: 443,
            protocol: PortProtocol::Tcp,
            host_ip: String::new(),
        };
        let s = serde_json::to_string(&m).expect("serialize");
        // host_port = None should be skipped, host_ip = "" should be skipped.
        assert!(!s.contains("host_port"), "host_port should be skipped: {s}");
        assert!(!s.contains("host_ip"), "host_ip should be skipped: {s}");
        assert!(s.contains("\"container_port\":443"));
        assert!(s.contains("\"protocol\":\"tcp\""));
    }

    #[test]
    fn test_parse_simple_spec() {
        let yaml = r"
version: v1
deployment: test
services:
  hello:
    rtype: service
    image:
      name: hello-world:latest
    endpoints:
      - name: http
        protocol: http
        port: 8080
        expose: public
";

        let spec: DeploymentSpec = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(spec.version, "v1");
        assert_eq!(spec.deployment, "test");
        assert!(spec.services.contains_key("hello"));
    }

    #[test]
    fn test_parse_duration() {
        let yaml = r"
version: v1
deployment: test
services:
  test:
    rtype: service
    image:
      name: test:latest
    health:
      timeout: 30s
      interval: 1m
      start_grace: 5s
      check:
        type: tcp
        port: 8080
";

        let spec: DeploymentSpec = serde_yaml::from_str(yaml).unwrap();
        let health = &spec.services["test"].health;
        assert_eq!(health.timeout, Some(std::time::Duration::from_secs(30)));
        assert_eq!(health.interval, Some(std::time::Duration::from_secs(60)));
        assert_eq!(health.start_grace, Some(std::time::Duration::from_secs(5)));
        match &health.check {
            HealthCheck::Tcp { port } => assert_eq!(*port, 8080),
            _ => panic!("Expected TCP health check"),
        }
    }

    #[test]
    fn test_parse_adaptive_scale() {
        let yaml = r"
version: v1
deployment: test
services:
  test:
    rtype: service
    image:
      name: test:latest
    scale:
      mode: adaptive
      min: 2
      max: 10
      cooldown: 15s
      targets:
        cpu: 70
        rps: 800
";

        let spec: DeploymentSpec = serde_yaml::from_str(yaml).unwrap();
        let scale = &spec.services["test"].scale;
        match scale {
            ScaleSpec::Adaptive {
                min,
                max,
                cooldown,
                targets,
            } => {
                assert_eq!(*min, 2);
                assert_eq!(*max, 10);
                assert_eq!(*cooldown, Some(std::time::Duration::from_secs(15)));
                assert_eq!(targets.cpu, Some(70));
                assert_eq!(targets.rps, Some(800));
            }
            _ => panic!("Expected Adaptive scale mode"),
        }
    }

    #[test]
    fn test_node_mode_default() {
        let yaml = r"
version: v1
deployment: test
services:
  hello:
    rtype: service
    image:
      name: hello-world:latest
";

        let spec: DeploymentSpec = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(spec.services["hello"].node_mode, NodeMode::Shared);
        assert!(spec.services["hello"].node_selector.is_none());
    }

    #[test]
    fn test_node_mode_dedicated() {
        let yaml = r"
version: v1
deployment: test
services:
  api:
    rtype: service
    image:
      name: api:latest
    node_mode: dedicated
";

        let spec: DeploymentSpec = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(spec.services["api"].node_mode, NodeMode::Dedicated);
    }

    #[test]
    fn test_node_mode_exclusive() {
        let yaml = r"
version: v1
deployment: test
services:
  database:
    rtype: service
    image:
      name: postgres:15
    node_mode: exclusive
";

        let spec: DeploymentSpec = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(spec.services["database"].node_mode, NodeMode::Exclusive);
    }

    #[test]
    fn test_node_selector_with_labels() {
        let yaml = r#"
version: v1
deployment: test
services:
  ml-worker:
    rtype: service
    image:
      name: ml-worker:latest
    node_mode: dedicated
    node_selector:
      labels:
        gpu: "true"
        zone: us-east
      prefer_labels:
        storage: ssd
"#;

        let spec: DeploymentSpec = serde_yaml::from_str(yaml).unwrap();
        let service = &spec.services["ml-worker"];
        assert_eq!(service.node_mode, NodeMode::Dedicated);

        let selector = service.node_selector.as_ref().unwrap();
        assert_eq!(selector.labels.get("gpu"), Some(&"true".to_string()));
        assert_eq!(selector.labels.get("zone"), Some(&"us-east".to_string()));
        assert_eq!(
            selector.prefer_labels.get("storage"),
            Some(&"ssd".to_string())
        );
    }

    #[test]
    fn test_node_mode_serialization_roundtrip() {
        use serde_json;

        // Test all variants serialize/deserialize correctly
        let modes = [NodeMode::Shared, NodeMode::Dedicated, NodeMode::Exclusive];
        let expected_json = ["\"shared\"", "\"dedicated\"", "\"exclusive\""];

        for (mode, expected) in modes.iter().zip(expected_json.iter()) {
            let json = serde_json::to_string(mode).unwrap();
            assert_eq!(&json, *expected, "Serialization failed for {mode:?}");

            let deserialized: NodeMode = serde_json::from_str(&json).unwrap();
            assert_eq!(deserialized, *mode, "Roundtrip failed for {mode:?}");
        }
    }

    #[test]
    fn test_node_selector_empty() {
        let yaml = r"
version: v1
deployment: test
services:
  api:
    rtype: service
    image:
      name: api:latest
    node_selector:
      labels: {}
";

        let spec: DeploymentSpec = serde_yaml::from_str(yaml).unwrap();
        let selector = spec.services["api"].node_selector.as_ref().unwrap();
        assert!(selector.labels.is_empty());
        assert!(selector.prefer_labels.is_empty());
    }

    #[test]
    fn test_mixed_node_modes_in_deployment() {
        let yaml = r"
version: v1
deployment: test
services:
  redis:
    rtype: service
    image:
      name: redis:alpine
    # Default shared mode
  api:
    rtype: service
    image:
      name: api:latest
    node_mode: dedicated
  database:
    rtype: service
    image:
      name: postgres:15
    node_mode: exclusive
    node_selector:
      labels:
        storage: ssd
";

        let spec: DeploymentSpec = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(spec.services["redis"].node_mode, NodeMode::Shared);
        assert_eq!(spec.services["api"].node_mode, NodeMode::Dedicated);
        assert_eq!(spec.services["database"].node_mode, NodeMode::Exclusive);

        let db_selector = spec.services["database"].node_selector.as_ref().unwrap();
        assert_eq!(db_selector.labels.get("storage"), Some(&"ssd".to_string()));
    }

    #[test]
    fn test_storage_bind_mount() {
        let yaml = r"
version: v1
deployment: test
services:
  app:
    image:
      name: app:latest
    storage:
      - type: bind
        source: /host/data
        target: /app/data
        readonly: true
";
        let spec: DeploymentSpec = serde_yaml::from_str(yaml).unwrap();
        let storage = &spec.services["app"].storage;
        assert_eq!(storage.len(), 1);
        match &storage[0] {
            StorageSpec::Bind {
                source,
                target,
                readonly,
            } => {
                assert_eq!(source, "/host/data");
                assert_eq!(target, "/app/data");
                assert!(*readonly);
            }
            _ => panic!("Expected Bind storage"),
        }
    }

    #[test]
    fn test_storage_named_with_tier() {
        let yaml = r"
version: v1
deployment: test
services:
  app:
    image:
      name: app:latest
    storage:
      - type: named
        name: my-data
        target: /app/data
        tier: cached
";
        let spec: DeploymentSpec = serde_yaml::from_str(yaml).unwrap();
        let storage = &spec.services["app"].storage;
        match &storage[0] {
            StorageSpec::Named {
                name, target, tier, ..
            } => {
                assert_eq!(name, "my-data");
                assert_eq!(target, "/app/data");
                assert_eq!(*tier, StorageTier::Cached);
            }
            _ => panic!("Expected Named storage"),
        }
    }

    #[test]
    fn test_storage_anonymous() {
        let yaml = r"
version: v1
deployment: test
services:
  app:
    image:
      name: app:latest
    storage:
      - type: anonymous
        target: /app/cache
";
        let spec: DeploymentSpec = serde_yaml::from_str(yaml).unwrap();
        let storage = &spec.services["app"].storage;
        match &storage[0] {
            StorageSpec::Anonymous { target, tier } => {
                assert_eq!(target, "/app/cache");
                assert_eq!(*tier, StorageTier::Local); // default
            }
            _ => panic!("Expected Anonymous storage"),
        }
    }

    #[test]
    fn test_storage_tmpfs() {
        let yaml = r"
version: v1
deployment: test
services:
  app:
    image:
      name: app:latest
    storage:
      - type: tmpfs
        target: /app/tmp
        size: 256Mi
        mode: 1777
";
        let spec: DeploymentSpec = serde_yaml::from_str(yaml).unwrap();
        let storage = &spec.services["app"].storage;
        match &storage[0] {
            StorageSpec::Tmpfs { target, size, mode } => {
                assert_eq!(target, "/app/tmp");
                assert_eq!(size.as_deref(), Some("256Mi"));
                assert_eq!(*mode, Some(1777));
            }
            _ => panic!("Expected Tmpfs storage"),
        }
    }

    #[test]
    fn test_storage_s3() {
        let yaml = r"
version: v1
deployment: test
services:
  app:
    image:
      name: app:latest
    storage:
      - type: s3
        bucket: my-bucket
        prefix: models/
        target: /app/models
        readonly: true
        endpoint: https://s3.us-west-2.amazonaws.com
        credentials: aws-creds
";
        let spec: DeploymentSpec = serde_yaml::from_str(yaml).unwrap();
        let storage = &spec.services["app"].storage;
        match &storage[0] {
            StorageSpec::S3 {
                bucket,
                prefix,
                target,
                readonly,
                endpoint,
                credentials,
            } => {
                assert_eq!(bucket, "my-bucket");
                assert_eq!(prefix.as_deref(), Some("models/"));
                assert_eq!(target, "/app/models");
                assert!(*readonly);
                assert_eq!(
                    endpoint.as_deref(),
                    Some("https://s3.us-west-2.amazonaws.com")
                );
                assert_eq!(credentials.as_deref(), Some("aws-creds"));
            }
            _ => panic!("Expected S3 storage"),
        }
    }

    #[test]
    fn test_storage_multiple_types() {
        let yaml = r"
version: v1
deployment: test
services:
  app:
    image:
      name: app:latest
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
";
        let spec: DeploymentSpec = serde_yaml::from_str(yaml).unwrap();
        let storage = &spec.services["app"].storage;
        assert_eq!(storage.len(), 3);
        assert!(matches!(&storage[0], StorageSpec::Bind { .. }));
        assert!(matches!(&storage[1], StorageSpec::Named { .. }));
        assert!(matches!(&storage[2], StorageSpec::Tmpfs { .. }));
    }

    #[test]
    fn test_storage_tier_default() {
        let yaml = r"
version: v1
deployment: test
services:
  app:
    image:
      name: app:latest
    storage:
      - type: named
        name: data
        target: /data
";
        let spec: DeploymentSpec = serde_yaml::from_str(yaml).unwrap();
        match &spec.services["app"].storage[0] {
            StorageSpec::Named { tier, .. } => {
                assert_eq!(*tier, StorageTier::Local); // default should be Local
            }
            _ => panic!("Expected Named storage"),
        }
    }

    // ==========================================================================
    // Tunnel configuration tests
    // ==========================================================================

    #[test]
    fn test_endpoint_tunnel_config_basic() {
        let yaml = r"
version: v1
deployment: test
services:
  api:
    image:
      name: api:latest
    endpoints:
      - name: http
        protocol: http
        port: 8080
        tunnel:
          enabled: true
          remote_port: 8080
";
        let spec: DeploymentSpec = serde_yaml::from_str(yaml).unwrap();
        let endpoint = &spec.services["api"].endpoints[0];
        let tunnel = endpoint.tunnel.as_ref().unwrap();
        assert!(tunnel.enabled);
        assert_eq!(tunnel.remote_port, 8080);
        assert!(tunnel.from.is_none());
        assert!(tunnel.to.is_none());
    }

    #[test]
    fn test_endpoint_tunnel_config_full() {
        let yaml = r"
version: v1
deployment: test
services:
  api:
    image:
      name: api:latest
    endpoints:
      - name: http
        protocol: http
        port: 8080
        tunnel:
          enabled: true
          from: node-1
          to: ingress-node
          remote_port: 9000
          expose: public
          access:
            enabled: true
            max_ttl: 4h
            audit: true
";
        let spec: DeploymentSpec = serde_yaml::from_str(yaml).unwrap();
        let endpoint = &spec.services["api"].endpoints[0];
        let tunnel = endpoint.tunnel.as_ref().unwrap();
        assert!(tunnel.enabled);
        assert_eq!(tunnel.from, Some("node-1".to_string()));
        assert_eq!(tunnel.to, Some("ingress-node".to_string()));
        assert_eq!(tunnel.remote_port, 9000);
        assert_eq!(tunnel.expose, Some(ExposeType::Public));

        let access = tunnel.access.as_ref().unwrap();
        assert!(access.enabled);
        assert_eq!(access.max_ttl, Some("4h".to_string()));
        assert!(access.audit);
    }

    #[test]
    fn test_top_level_tunnel_definition() {
        let yaml = r"
version: v1
deployment: test
services: {}
tunnels:
  db-tunnel:
    from: app-node
    to: db-node
    local_port: 5432
    remote_port: 5432
    protocol: tcp
    expose: internal
";
        let spec: DeploymentSpec = serde_yaml::from_str(yaml).unwrap();
        let tunnel = spec.tunnels.get("db-tunnel").unwrap();
        assert_eq!(tunnel.from, "app-node");
        assert_eq!(tunnel.to, "db-node");
        assert_eq!(tunnel.local_port, 5432);
        assert_eq!(tunnel.remote_port, 5432);
        assert_eq!(tunnel.protocol, TunnelProtocol::Tcp);
        assert_eq!(tunnel.expose, ExposeType::Internal);
    }

    #[test]
    fn test_top_level_tunnel_defaults() {
        let yaml = r"
version: v1
deployment: test
services: {}
tunnels:
  simple-tunnel:
    from: node-a
    to: node-b
    local_port: 3000
    remote_port: 3000
";
        let spec: DeploymentSpec = serde_yaml::from_str(yaml).unwrap();
        let tunnel = spec.tunnels.get("simple-tunnel").unwrap();
        assert_eq!(tunnel.protocol, TunnelProtocol::Tcp); // default
        assert_eq!(tunnel.expose, ExposeType::Internal); // default
    }

    #[test]
    fn test_tunnel_protocol_udp() {
        let yaml = r"
version: v1
deployment: test
services: {}
tunnels:
  udp-tunnel:
    from: node-a
    to: node-b
    local_port: 5353
    remote_port: 5353
    protocol: udp
";
        let spec: DeploymentSpec = serde_yaml::from_str(yaml).unwrap();
        let tunnel = spec.tunnels.get("udp-tunnel").unwrap();
        assert_eq!(tunnel.protocol, TunnelProtocol::Udp);
    }

    #[test]
    fn test_endpoint_without_tunnel() {
        let yaml = r"
version: v1
deployment: test
services:
  api:
    image:
      name: api:latest
    endpoints:
      - name: http
        protocol: http
        port: 8080
";
        let spec: DeploymentSpec = serde_yaml::from_str(yaml).unwrap();
        let endpoint = &spec.services["api"].endpoints[0];
        assert!(endpoint.tunnel.is_none());
    }

    #[test]
    fn test_deployment_without_tunnels() {
        let yaml = r"
version: v1
deployment: test
services:
  api:
    image:
      name: api:latest
";
        let spec: DeploymentSpec = serde_yaml::from_str(yaml).unwrap();
        assert!(spec.tunnels.is_empty());
    }

    // ==========================================================================
    // ApiSpec tests
    // ==========================================================================

    #[test]
    fn test_spec_without_api_block_uses_defaults() {
        let yaml = r"
version: v1
deployment: test
services:
  hello:
    image:
      name: hello-world:latest
";
        let spec: DeploymentSpec = serde_yaml::from_str(yaml).unwrap();
        assert!(spec.api.enabled);
        assert_eq!(spec.api.bind, "0.0.0.0:3669");
        assert!(spec.api.jwt_secret.is_none());
        assert!(spec.api.swagger);
    }

    #[test]
    fn test_spec_with_explicit_api_block() {
        let yaml = r#"
version: v1
deployment: test
services:
  hello:
    image:
      name: hello-world:latest
api:
  enabled: false
  bind: "127.0.0.1:9090"
  jwt_secret: "my-secret"
  swagger: false
"#;
        let spec: DeploymentSpec = serde_yaml::from_str(yaml).unwrap();
        assert!(!spec.api.enabled);
        assert_eq!(spec.api.bind, "127.0.0.1:9090");
        assert_eq!(spec.api.jwt_secret, Some("my-secret".to_string()));
        assert!(!spec.api.swagger);
    }

    #[test]
    fn test_spec_with_partial_api_block() {
        let yaml = r#"
version: v1
deployment: test
services:
  hello:
    image:
      name: hello-world:latest
api:
  bind: "0.0.0.0:3000"
"#;
        let spec: DeploymentSpec = serde_yaml::from_str(yaml).unwrap();
        assert!(spec.api.enabled); // default true
        assert_eq!(spec.api.bind, "0.0.0.0:3000");
        assert!(spec.api.jwt_secret.is_none()); // default None
        assert!(spec.api.swagger); // default true
    }

    // ==========================================================================
    // NetworkPolicySpec tests
    // ==========================================================================

    #[test]
    fn test_network_policy_spec_roundtrip() {
        let spec = NetworkPolicySpec {
            name: "corp-vpn".to_string(),
            description: Some("Corporate VPN network".to_string()),
            cidrs: vec!["10.200.0.0/16".to_string()],
            members: vec![
                NetworkMember {
                    name: "alice".to_string(),
                    kind: MemberKind::User,
                },
                NetworkMember {
                    name: "ops-team".to_string(),
                    kind: MemberKind::Group,
                },
                NetworkMember {
                    name: "node-01".to_string(),
                    kind: MemberKind::Node,
                },
            ],
            access_rules: vec![
                AccessRule {
                    service: "api-gateway".to_string(),
                    deployment: "*".to_string(),
                    ports: Some(vec![443, 8080]),
                    action: AccessAction::Allow,
                },
                AccessRule {
                    service: "*".to_string(),
                    deployment: "staging".to_string(),
                    ports: None,
                    action: AccessAction::Deny,
                },
            ],
        };

        let yaml = serde_yaml::to_string(&spec).unwrap();
        let deserialized: NetworkPolicySpec = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(spec, deserialized);
    }

    #[test]
    fn test_network_policy_spec_defaults() {
        let yaml = r"
name: minimal
";
        let spec: NetworkPolicySpec = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(spec.name, "minimal");
        assert!(spec.description.is_none());
        assert!(spec.cidrs.is_empty());
        assert!(spec.members.is_empty());
        assert!(spec.access_rules.is_empty());
    }

    #[test]
    fn test_access_rule_defaults() {
        let yaml = "{}";
        let rule: AccessRule = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(rule.service, "*");
        assert_eq!(rule.deployment, "*");
        assert!(rule.ports.is_none());
        assert_eq!(rule.action, AccessAction::Allow);
    }

    #[test]
    fn test_member_kind_defaults_to_user() {
        let yaml = r"
name: bob
";
        let member: NetworkMember = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(member.name, "bob");
        assert_eq!(member.kind, MemberKind::User);
    }

    #[test]
    fn test_member_kind_variants() {
        for (input, expected) in [
            ("user", MemberKind::User),
            ("group", MemberKind::Group),
            ("node", MemberKind::Node),
            ("cidr", MemberKind::Cidr),
        ] {
            let yaml = format!("name: test\nkind: {input}");
            let member: NetworkMember = serde_yaml::from_str(&yaml).unwrap();
            assert_eq!(member.kind, expected);
        }
    }

    #[test]
    fn test_access_action_variants() {
        // Test via a wrapper struct since bare enums need a YAML tag
        #[derive(Debug, Deserialize)]
        struct Wrapper {
            action: AccessAction,
        }

        let allow: Wrapper = serde_yaml::from_str("action: allow").unwrap();
        let deny: Wrapper = serde_yaml::from_str("action: deny").unwrap();

        assert_eq!(allow.action, AccessAction::Allow);
        assert_eq!(deny.action, AccessAction::Deny);
    }

    #[test]
    fn test_network_policy_spec_default_impl() {
        let spec = NetworkPolicySpec::default();
        assert_eq!(spec.name, "");
        assert!(spec.description.is_none());
        assert!(spec.cidrs.is_empty());
        assert!(spec.members.is_empty());
        assert!(spec.access_rules.is_empty());
    }

    #[test]
    fn container_restart_policy_serde_roundtrip_all_kinds() {
        // Exercise every `ContainerRestartKind` variant via a JSON roundtrip.
        // Covers the `snake_case` rename (`unless_stopped`, `on_failure`) and
        // the optional `max_attempts` / `delay` fields. Validates the wire
        // format the API will expose under `/v1/containers`.
        let cases = [
            (
                ContainerRestartPolicy {
                    kind: ContainerRestartKind::No,
                    max_attempts: None,
                    delay: None,
                },
                r#"{"kind":"no"}"#,
            ),
            (
                ContainerRestartPolicy {
                    kind: ContainerRestartKind::Always,
                    max_attempts: None,
                    delay: Some("500ms".to_string()),
                },
                r#"{"kind":"always","delay":"500ms"}"#,
            ),
            (
                ContainerRestartPolicy {
                    kind: ContainerRestartKind::UnlessStopped,
                    max_attempts: None,
                    delay: None,
                },
                r#"{"kind":"unless_stopped"}"#,
            ),
            (
                ContainerRestartPolicy {
                    kind: ContainerRestartKind::OnFailure,
                    max_attempts: Some(5),
                    delay: None,
                },
                r#"{"kind":"on_failure","max_attempts":5}"#,
            ),
        ];

        for (value, expected_json) in &cases {
            let serialized = serde_json::to_string(value).expect("serialize");
            assert_eq!(&serialized, expected_json, "serialize mismatch");
            let round: ContainerRestartPolicy =
                serde_json::from_str(&serialized).expect("deserialize");
            assert_eq!(&round, value, "roundtrip mismatch");
        }
    }

    // -- §3.10: RegistryAuth ------------------------------------------------

    #[test]
    fn registry_auth_type_serializes_snake_case() {
        assert_eq!(
            serde_json::to_string(&RegistryAuthType::Basic).unwrap(),
            "\"basic\""
        );
        assert_eq!(
            serde_json::to_string(&RegistryAuthType::Token).unwrap(),
            "\"token\""
        );
    }

    #[test]
    fn registry_auth_default_auth_type_is_basic() {
        // When `auth_type` is omitted on the wire, the serde default kicks in.
        let json = r#"{"username":"u","password":"p"}"#;
        let parsed: RegistryAuth = serde_json::from_str(json).expect("parse");
        assert_eq!(parsed.auth_type, RegistryAuthType::Basic);
        assert_eq!(parsed.username, "u");
        assert_eq!(parsed.password, "p");
    }

    #[test]
    fn registry_auth_serde_roundtrip_both_variants() {
        for variant in [RegistryAuthType::Basic, RegistryAuthType::Token] {
            let cred = RegistryAuth {
                username: "ci-bot".to_string(),
                password: "s3cret".to_string(),
                auth_type: variant,
            };
            let serialized = serde_json::to_string(&cred).expect("serialize");
            let back: RegistryAuth = serde_json::from_str(&serialized).expect("deserialize");
            assert_eq!(back, cred, "roundtrip mismatch for {variant:?}");
        }
    }

    #[test]
    fn registry_auth_explicit_token_type_parses() {
        let json = r#"{"username":"oauth2accesstoken","password":"ghp_abc","auth_type":"token"}"#;
        let parsed: RegistryAuth = serde_json::from_str(json).expect("parse");
        assert_eq!(parsed.auth_type, RegistryAuthType::Token);
    }

    #[test]
    fn target_platform_as_oci_str() {
        assert_eq!(
            TargetPlatform::new(OsKind::Linux, ArchKind::Amd64).as_oci_str(),
            "linux/amd64"
        );
        assert_eq!(
            TargetPlatform::new(OsKind::Windows, ArchKind::Arm64).as_oci_str(),
            "windows/arm64"
        );
        assert_eq!(
            TargetPlatform::new(OsKind::Macos, ArchKind::Arm64).as_oci_str(),
            "darwin/arm64"
        );
    }

    #[test]
    fn os_kind_from_rust_consts() {
        assert_eq!(OsKind::from_rust_os("linux"), Some(OsKind::Linux));
        assert_eq!(OsKind::from_rust_os("windows"), Some(OsKind::Windows));
        assert_eq!(OsKind::from_rust_os("macos"), Some(OsKind::Macos));
        assert_eq!(OsKind::from_rust_os("freebsd"), None);
    }

    #[test]
    fn arch_kind_from_rust_consts() {
        assert_eq!(ArchKind::from_rust_arch("x86_64"), Some(ArchKind::Amd64));
        assert_eq!(ArchKind::from_rust_arch("aarch64"), Some(ArchKind::Arm64));
        assert_eq!(ArchKind::from_rust_arch("riscv64"), None);
    }

    #[test]
    fn service_spec_platform_yaml_round_trip_none() {
        // Omitting `platform` from YAML should deserialize as None without error,
        // even though ServiceSpec has `#[serde(deny_unknown_fields)]`.
        let yaml = r"
version: v1
deployment: test
services:
  app:
    rtype: service
    image:
      name: nginx:latest
";
        let spec: DeploymentSpec = serde_yaml::from_str(yaml).expect("yaml parse");
        assert!(spec.services["app"].platform.is_none());
    }

    #[test]
    fn service_spec_platform_yaml_round_trip_some() {
        let yaml = r"
version: v1
deployment: test
services:
  app:
    rtype: service
    image:
      name: nginx:latest
    platform:
      os: windows
      arch: amd64
";
        let spec: DeploymentSpec = serde_yaml::from_str(yaml).expect("yaml parse");
        assert_eq!(
            spec.services["app"].platform,
            Some(TargetPlatform::new(OsKind::Windows, ArchKind::Amd64))
        );
    }

    #[test]
    fn service_spec_platform_serializes_omitted_when_none() {
        // Build a minimal ServiceSpec via YAML to avoid enumerating every field
        // (ServiceSpec has no Default impl and no named-struct helper).
        let yaml = r"
version: v1
deployment: test
services:
  app:
    rtype: service
    image:
      name: nginx:latest
";
        let mut spec: DeploymentSpec = serde_yaml::from_str(yaml).expect("yaml parse");
        let service = spec.services.get_mut("app").expect("service present");
        service.platform = None;
        let rendered = serde_yaml::to_string(service).expect("render");
        assert!(
            !rendered.contains("platform"),
            "platform must be omitted when None: {rendered}"
        );
    }

    #[test]
    fn target_platform_os_version_builder() {
        let p =
            TargetPlatform::new(OsKind::Windows, ArchKind::Amd64).with_os_version("10.0.26100.1");
        assert_eq!(p.os_version.as_deref(), Some("10.0.26100.1"));
        assert_eq!(p.os, OsKind::Windows);
        assert_eq!(p.arch, ArchKind::Amd64);
    }

    #[test]
    fn target_platform_os_version_yaml_roundtrip() {
        let yaml = "os: windows\narch: amd64\nosVersion: 10.0.26100.1\n";
        let p: TargetPlatform = serde_yaml::from_str(yaml).expect("yaml parse");
        assert_eq!(p.os_version.as_deref(), Some("10.0.26100.1"));
        assert_eq!(p.os, OsKind::Windows);
        assert_eq!(p.arch, ArchKind::Amd64);
    }

    #[test]
    fn target_platform_os_version_yaml_omits_when_none() {
        let p = TargetPlatform::new(OsKind::Linux, ArchKind::Amd64);
        let rendered = serde_yaml::to_string(&p).expect("render");
        assert!(
            !rendered.contains("osVersion"),
            "osVersion must be omitted when None: {rendered}"
        );
    }

    #[test]
    fn target_platform_as_detailed_str_includes_version() {
        let without = TargetPlatform::new(OsKind::Windows, ArchKind::Amd64).as_detailed_str();
        assert_eq!(without, "windows/amd64");

        let with = TargetPlatform::new(OsKind::Windows, ArchKind::Amd64)
            .with_os_version("10.0.26100.1")
            .as_detailed_str();
        assert_eq!(with, "windows/amd64 (os.version=10.0.26100.1)");
    }

    #[test]
    fn target_platform_display_ignores_version() {
        // Display deliberately stays terse so existing log lines don't change.
        let p =
            TargetPlatform::new(OsKind::Windows, ArchKind::Amd64).with_os_version("10.0.26100.1");
        assert_eq!(format!("{p}"), "windows/amd64");
    }

    // ----------------------------------------------------------------------
    // Phase 1 Task 1.1: Docker-compat ServiceSpec/ResourcesSpec extensions.
    // ----------------------------------------------------------------------

    /// Build a minimal-but-valid `ServiceSpec` for round-trip tests.
    fn fixture_service_spec_full() -> ServiceSpec {
        let yaml = r"
version: v1
deployment: phase1-task1
services:
  hello:
    rtype: service
    image:
      name: hello-world:latest
";
        let spec: DeploymentSpec = serde_yaml::from_str(yaml).expect("parse fixture");
        spec.services.get("hello").expect("hello service").clone()
    }

    #[test]
    fn service_spec_round_trip_with_all_new_fields() {
        let mut spec = fixture_service_spec_full();
        spec.labels
            .insert("zlayer.team".to_string(), "platform".to_string());
        spec.user = Some("1000:1000".to_string());
        spec.stop_signal = Some("SIGTERM".to_string());
        spec.stop_grace_period = Some(std::time::Duration::from_secs(30));
        spec.sysctls
            .insert("net.core.somaxconn".to_string(), "1024".to_string());
        spec.ulimits.insert(
            "nofile".to_string(),
            UlimitSpec {
                soft: 65_536,
                hard: 65_536,
            },
        );
        spec.security_opt.push("no-new-privileges:true".to_string());
        spec.pid_mode = Some("host".to_string());
        spec.ipc_mode = Some("private".to_string());
        spec.network_mode = NetworkMode::Bridge {
            name: Some("custom-net".to_string()),
        };
        spec.cap_drop.push("NET_RAW".to_string());
        spec.extra_groups.push("docker".to_string());
        spec.read_only_root_fs = true;
        spec.init_container = Some(true);
        spec.resources.pids_limit = Some(2048);
        spec.resources.cpuset = Some("0-3".to_string());
        spec.resources.cpu_shares = Some(1024);
        spec.resources.memory_swap = Some("2Gi".to_string());
        spec.resources.memory_reservation = Some("256Mi".to_string());
        spec.resources.memory_swappiness = Some(10);
        spec.resources.oom_score_adj = Some(-500);
        spec.resources.oom_kill_disable = Some(false);
        spec.resources.blkio_weight = Some(500);

        let yaml = serde_yaml::to_string(&spec).expect("serialize");
        let round: ServiceSpec = serde_yaml::from_str(&yaml).expect("deserialize");
        assert_eq!(spec, round, "round-trip mismatch:\n{yaml}");
    }

    #[test]
    fn network_mode_string_form_round_trip() {
        let cases: &[(&str, NetworkMode)] = &[
            ("default", NetworkMode::Default),
            ("host", NetworkMode::Host),
            ("none", NetworkMode::None),
            ("bridge", NetworkMode::Bridge { name: None }),
            (
                "bridge:custom",
                NetworkMode::Bridge {
                    name: Some("custom".to_string()),
                },
            ),
            (
                "container:abc123",
                NetworkMode::Container {
                    id: "abc123".to_string(),
                },
            ),
        ];

        for (input, expected) in cases {
            #[derive(Deserialize)]
            struct Wrap {
                #[serde(deserialize_with = "deserialize_network_mode")]
                m: NetworkMode,
            }
            let yaml = format!("m: \"{input}\"\n");
            let parsed: Wrap = serde_yaml::from_str(&yaml).expect("parse network mode");
            assert_eq!(&parsed.m, expected, "mismatch for {input}");
        }
    }

    #[test]
    fn ulimit_spec_round_trip() {
        let u = UlimitSpec {
            soft: 1024,
            hard: 65_536,
        };
        let yaml = serde_yaml::to_string(&u).expect("serialize");
        let parsed: UlimitSpec = serde_yaml::from_str(&yaml).expect("parse");
        assert_eq!(u, parsed);
    }

    #[test]
    fn host_network_true_yaml_promotes_to_network_mode_host() {
        let yaml = r"
version: v1
deployment: bc-test
services:
  hello:
    rtype: service
    image:
      name: hello-world:latest
    host_network: true
";
        let dep: DeploymentSpec = serde_yaml::from_str(yaml).expect("parse");
        let svc = dep.services.get("hello").expect("hello service");
        assert_eq!(svc.network_mode, NetworkMode::Host);
        // The legacy bool stays mirrored so in-process callers that still
        // read `host_network` continue to work.
        assert!(svc.host_network);
    }

    #[test]
    fn capabilities_yaml_alias_cap_add_round_trip() {
        // Forward-compat: ZLayer keeps the field named `capabilities`, but the
        // Docker-style key `cap_add` must also deserialize into it.
        let yaml = r"
version: v1
deployment: cap-test
services:
  hello:
    rtype: service
    image:
      name: hello-world:latest
    cap_add:
      - NET_ADMIN
      - SYS_PTRACE
";
        let dep: DeploymentSpec = serde_yaml::from_str(yaml).expect("parse cap_add alias");
        let svc = dep.services.get("hello").expect("hello service");
        assert_eq!(
            svc.capabilities,
            vec!["NET_ADMIN".to_string(), "SYS_PTRACE".to_string()]
        );
    }

    #[test]
    fn lifecycle_omitted_defaults_to_false() {
        // When `lifecycle` is absent from the YAML/JSON entirely, the
        // deserialized service must fall back to `LifecycleSpec::default()`,
        // i.e. `delete_on_exit: false` — the historical retain-on-exit
        // behavior. This guards against accidental policy flips when the
        // field is added to existing specs.
        let yaml = r"
version: v1
deployment: lifecycle-default-test
services:
  app:
    rtype: service
    image:
      name: hello-world:latest
";
        let dep: DeploymentSpec = serde_yaml::from_str(yaml).expect("parse spec without lifecycle");
        let svc = dep.services.get("app").expect("app service");
        assert_eq!(svc.lifecycle, LifecycleSpec::default());
        assert!(!svc.lifecycle.delete_on_exit);
    }

    #[test]
    fn lifecycle_delete_on_exit_round_trips() {
        // `lifecycle.delete_on_exit: true` must survive a full YAML
        // deserialize → serialize → deserialize cycle, and the explicit
        // value must propagate into the parsed `ServiceSpec`.
        let yaml = r"
version: v1
deployment: lifecycle-delete-test
services:
  app:
    rtype: service
    image:
      name: hello-world:latest
    lifecycle:
      delete_on_exit: true
";
        let dep: DeploymentSpec = serde_yaml::from_str(yaml).expect("parse spec with lifecycle");
        let svc = dep.services.get("app").expect("app service");
        assert!(svc.lifecycle.delete_on_exit);

        // Round-trip via YAML to confirm Serialize emits the field and
        // Deserialize folds it back identically.
        let dumped = serde_yaml::to_string(&dep).expect("serialize spec with lifecycle");
        let reparsed: DeploymentSpec =
            serde_yaml::from_str(&dumped).expect("reparse round-tripped spec");
        let reparsed_svc = reparsed.services.get("app").expect("app service after rt");
        assert!(reparsed_svc.lifecycle.delete_on_exit);
        assert_eq!(svc.lifecycle, reparsed_svc.lifecycle);
    }
}

#[cfg(test)]
mod replica_group_tests {
    use super::{
        validate_unique_replica_group_roles, EndpointSpec, GroupAffinity, ReplicaGroup,
        REPLICA_GROUP_ROLE_RE,
    };

    #[test]
    fn yaml_roundtrip_basic_group() {
        let yaml = r"
role: primary
count: 1
env:
  POSTGRES_REPLICATION_MODE: primary
affinity: spread
";
        let group: ReplicaGroup = serde_yaml::from_str(yaml).expect("parse basic group");
        assert_eq!(group.role, "primary");
        assert_eq!(group.count, 1);
        assert_eq!(group.affinity, GroupAffinity::Spread);
        assert_eq!(
            group.env.get("POSTGRES_REPLICATION_MODE"),
            Some(&"primary".to_string())
        );
    }

    #[test]
    fn yaml_default_affinity_is_spread() {
        let yaml = "role: x\ncount: 2\n";
        let group: ReplicaGroup = serde_yaml::from_str(yaml).expect("parse minimal group");
        assert_eq!(group.affinity, GroupAffinity::Spread);
    }

    #[test]
    fn role_regex_accepts_valid_labels() {
        for ok in ["a", "primary", "read-only", "x1", "ab-cd-ef"] {
            assert!(
                REPLICA_GROUP_ROLE_RE.is_match(ok),
                "regex should accept: {ok}"
            );
        }
    }

    #[test]
    fn role_regex_rejects_invalid_labels() {
        for bad in [
            "",
            "-primary",
            "primary-",
            "Primary",
            "0primary",
            "primary_role",
            "this-is-way-too-long-of-a-role-name-here",
        ] {
            assert!(
                !REPLICA_GROUP_ROLE_RE.is_match(bad),
                "regex should reject: {bad}"
            );
        }
    }

    #[test]
    fn group_affinity_pin_roundtrips_via_serde_yaml() {
        // Externally-tagged enum with a single string payload serializes as
        // a mapping `pin: <value>` under snake_case naming.
        let pinned = GroupAffinity::Pin("id=2".to_string());
        let dumped = serde_yaml::to_string(&pinned).expect("serialize pin");
        let reparsed: GroupAffinity = serde_yaml::from_str(&dumped).expect("reparse pin");
        match reparsed {
            GroupAffinity::Pin(s) => assert_eq!(s, "id=2"),
            other => panic!("expected Pin, got {other:?}"),
        }
    }

    #[test]
    fn unique_role_validator_rejects_duplicates() {
        let mk = |role: &str| ReplicaGroup {
            role: role.to_string(),
            count: 1,
            image: None,
            env: std::collections::HashMap::new(),
            command: None,
            resources: None,
            affinity: GroupAffinity::Spread,
        };
        assert!(validate_unique_replica_group_roles(&[mk("a"), mk("b")]).is_ok());
        let err = validate_unique_replica_group_roles(&[mk("a"), mk("a")])
            .expect_err("duplicate should fail");
        assert_eq!(err, "a");
    }

    #[test]
    fn endpoint_target_role_yaml_roundtrip() {
        let yaml = "name: read\nprotocol: tcp\nport: 5433\ntarget_role: read\n";
        let ep: EndpointSpec = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(ep.target_role, Some("read".to_string()));
    }

    #[test]
    fn endpoint_without_target_role_is_none() {
        let yaml = "name: any\nprotocol: tcp\nport: 5432\n";
        let ep: EndpointSpec = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(ep.target_role, None);
    }
}
