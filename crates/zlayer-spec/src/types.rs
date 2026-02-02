//! ZLayer V1 Service Specification Types
//!
//! This module defines all types for parsing and validating ZLayer deployment specs.

mod duration {
    use humantime::format_duration;
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::Duration;

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
                .map_err(|e| D::Error::custom(format!("invalid duration: {}", e))),
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
                .map_err(|e| D::Error::custom(format!("invalid duration: {}", e)))
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
#[serde(rename_all = "lowercase")]
pub enum ServiceType {
    /// Standard long-running container service
    #[default]
    Standard,
    /// WASM-based HTTP service with instance pooling
    WasmHttp,
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
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeSelector {
    /// Required labels that nodes must have (all must match)
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub labels: HashMap<String, String>,
    /// Preferred labels (soft constraint, nodes with these are preferred)
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub prefer_labels: HashMap<String, String>,
}

/// Configuration for WASM HTTP services with instance pooling
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

/// Top-level deployment specification
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Validate)]
#[serde(deny_unknown_fields)]
pub struct DeploymentSpec {
    /// Spec version (must be "v1")
    #[validate(custom(function = "crate::validate::validate_version_wrapper"))]
    pub version: String,

    /// Deployment name (used for overlays, DNS)
    #[validate(custom(function = "crate::validate::validate_deployment_name_wrapper"))]
    pub deployment: String,

    /// Service definitions
    #[serde(default)]
    #[validate(nested)]
    pub services: HashMap<String, ServiceSpec>,
}

/// Per-service specification
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Validate)]
#[serde(deny_unknown_fields)]
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
    #[validate(custom(function = "crate::validate::validate_schedule_wrapper"))]
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
    pub network: NetworkSpec,

    /// Endpoint definitions (proxy bindings)
    #[serde(default)]
    #[validate(nested)]
    pub endpoints: Vec<EndpointSpec>,

    /// Scaling configuration
    #[serde(default)]
    #[validate(custom(function = "crate::validate::validate_scale_spec"))]
    pub scale: ScaleSpec,

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

    /// Device passthrough (e.g., /dev/kvm for VMs)
    #[serde(default)]
    pub devices: Vec<DeviceSpec>,

    /// Storage mounts for the container
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub storage: Vec<StorageSpec>,

    /// Linux capabilities to add (e.g., SYS_ADMIN, NET_ADMIN)
    #[serde(default)]
    pub capabilities: Vec<String>,

    /// Run container in privileged mode (all capabilities + all devices)
    #[serde(default)]
    pub privileged: bool,

    /// Node allocation mode (shared, dedicated, exclusive)
    #[serde(default)]
    pub node_mode: NodeMode,

    /// Node selection constraints (required/preferred labels)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_selector: Option<NodeSelector>,

    /// Service type (standard, wasm_http, job)
    #[serde(default)]
    pub service_type: ServiceType,

    /// WASM HTTP configuration (only used when service_type is WasmHttp)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wasm_http: Option<WasmHttpConfig>,
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
        start_grace: None,
        interval: None,
        timeout: None,
        retries: 3,
        check: HealthCheck::Tcp { port: 0 },
    }
}

/// Resource type - determines container lifecycle
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ResourceType {
    /// Long-running container, receives traffic, load-balanced
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
    #[validate(custom(function = "crate::validate::validate_image_name_wrapper"))]
    pub name: String,

    /// When to pull the image
    #[serde(default = "default_pull_policy")]
    pub pull_policy: PullPolicy,
}

fn default_pull_policy() -> PullPolicy {
    PullPolicy::IfNotPresent
}

/// Image pull policy
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PullPolicy {
    /// Always pull the image
    Always,
    /// Pull only if not present locally
    IfNotPresent,
    /// Never pull, use local image only
    Never,
}

/// Device passthrough specification
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Validate)]
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
    #[validate(custom(function = "crate::validate::validate_cpu_option_wrapper"))]
    pub cpu: Option<f64>,

    /// Memory limit (e.g., "512Mi", "1Gi", "2Gi")
    #[serde(default)]
    #[validate(custom(function = "crate::validate::validate_memory_option_wrapper"))]
    pub memory: Option<String>,
}

/// Network configuration
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[derive(Default)]
pub struct NetworkSpec {
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

    /// Container port
    #[validate(custom(function = "crate::validate::validate_port_wrapper"))]
    pub port: u16,

    /// URL path prefix (for http/https/websocket)
    pub path: Option<String>,

    /// Exposure type
    #[serde(default = "default_expose")]
    pub expose: ExposeType,

    /// Optional stream (L4) proxy configuration
    /// Only applicable when protocol is tcp or udp
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream: Option<StreamEndpointConfig>,
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
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ExposeType {
    Public,
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
            targets: Default::default(),
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

/// Init action step
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InitStep {
    /// Step identifier
    pub id: String,

    /// Action to perform (e.g., "init.wait_tcp")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_spec() {
        let yaml = r#"
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
"#;

        let spec: DeploymentSpec = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(spec.version, "v1");
        assert_eq!(spec.deployment, "test");
        assert!(spec.services.contains_key("hello"));
    }

    #[test]
    fn test_parse_duration() {
        let yaml = r#"
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
"#;

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
        let yaml = r#"
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
"#;

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
        let yaml = r#"
version: v1
deployment: test
services:
  hello:
    rtype: service
    image:
      name: hello-world:latest
"#;

        let spec: DeploymentSpec = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(spec.services["hello"].node_mode, NodeMode::Shared);
        assert!(spec.services["hello"].node_selector.is_none());
    }

    #[test]
    fn test_node_mode_dedicated() {
        let yaml = r#"
version: v1
deployment: test
services:
  api:
    rtype: service
    image:
      name: api:latest
    node_mode: dedicated
"#;

        let spec: DeploymentSpec = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(spec.services["api"].node_mode, NodeMode::Dedicated);
    }

    #[test]
    fn test_node_mode_exclusive() {
        let yaml = r#"
version: v1
deployment: test
services:
  database:
    rtype: service
    image:
      name: postgres:15
    node_mode: exclusive
"#;

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
            assert_eq!(&json, *expected, "Serialization failed for {:?}", mode);

            let deserialized: NodeMode = serde_json::from_str(&json).unwrap();
            assert_eq!(deserialized, *mode, "Roundtrip failed for {:?}", mode);
        }
    }

    #[test]
    fn test_node_selector_empty() {
        let yaml = r#"
version: v1
deployment: test
services:
  api:
    rtype: service
    image:
      name: api:latest
    node_selector:
      labels: {}
"#;

        let spec: DeploymentSpec = serde_yaml::from_str(yaml).unwrap();
        let selector = spec.services["api"].node_selector.as_ref().unwrap();
        assert!(selector.labels.is_empty());
        assert!(selector.prefer_labels.is_empty());
    }

    #[test]
    fn test_mixed_node_modes_in_deployment() {
        let yaml = r#"
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
"#;

        let spec: DeploymentSpec = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(spec.services["redis"].node_mode, NodeMode::Shared);
        assert_eq!(spec.services["api"].node_mode, NodeMode::Dedicated);
        assert_eq!(spec.services["database"].node_mode, NodeMode::Exclusive);

        let db_selector = spec.services["database"].node_selector.as_ref().unwrap();
        assert_eq!(db_selector.labels.get("storage"), Some(&"ssd".to_string()));
    }

    #[test]
    fn test_storage_bind_mount() {
        let yaml = r#"
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
"#;
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
        let yaml = r#"
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
"#;
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
        let yaml = r#"
version: v1
deployment: test
services:
  app:
    image:
      name: app:latest
    storage:
      - type: anonymous
        target: /app/cache
"#;
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
        let yaml = r#"
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
"#;
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
        let yaml = r#"
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
"#;
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
        let yaml = r#"
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
"#;
        let spec: DeploymentSpec = serde_yaml::from_str(yaml).unwrap();
        let storage = &spec.services["app"].storage;
        assert_eq!(storage.len(), 3);
        assert!(matches!(&storage[0], StorageSpec::Bind { .. }));
        assert!(matches!(&storage[1], StorageSpec::Named { .. }));
        assert!(matches!(&storage[2], StorageSpec::Tmpfs { .. }));
    }

    #[test]
    fn test_storage_tier_default() {
        let yaml = r#"
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
"#;
        let spec: DeploymentSpec = serde_yaml::from_str(yaml).unwrap();
        match &spec.services["app"].storage[0] {
            StorageSpec::Named { tier, .. } => {
                assert_eq!(*tier, StorageTier::Local); // default should be Local
            }
            _ => panic!("Expected Named storage"),
        }
    }
}
