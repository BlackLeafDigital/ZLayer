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
            Some(s) => {
                humantime::parse_duration(&s)
                    .map(Some)
                    .map_err(|e| D::Error::custom(format!("invalid duration: {}", e)))
            }
            None => Ok(None),
        }
    }

    pub mod option {
        pub use super::*;
    }
}

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use validator::Validate;

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
    #[validate]
    pub services: HashMap<String, ServiceSpec>,
}

/// Per-service specification
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Validate)]
#[serde(deny_unknown_fields)]
pub struct ServiceSpec {
    /// Resource type (service, job, cron)
    #[serde(default = "default_resource_type")]
    pub rtype: ResourceType,

    /// Container image specification
    #[validate]
    pub image: ImageSpec,

    /// Resource limits
    #[serde(default)]
    #[validate]
    pub resources: ResourcesSpec,

    /// Environment variables
    #[serde(default)]
    pub env: HashMap<String, String>,

    /// Network configuration
    #[serde(default)]
    pub network: NetworkSpec,

    /// Endpoint definitions (proxy bindings)
    #[serde(default)]
    #[validate]
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
pub struct NetworkSpec {
    /// Overlay network configuration
    #[serde(default)]
    pub overlays: OverlayConfig,

    /// Join policy (who can join this service)
    #[serde(default)]
    pub join: JoinPolicy,
}

impl Default for NetworkSpec {
    fn default() -> Self {
        Self {
            overlays: Default::default(),
            join: Default::default(),
        }
    }
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

impl Default for ScaleTargets {
    fn default() -> Self {
        Self {
            cpu: None,
            memory: None,
            rps: None,
        }
    }
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
pub struct InitSpec {
    /// Init steps to run before container starts
    #[serde(default)]
    pub steps: Vec<InitStep>,
}

impl Default for InitSpec {
    fn default() -> Self {
        Self { steps: Vec::new() }
    }
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
pub struct ErrorsSpec {
    /// Init failure policy
    #[serde(default)]
    pub on_init_failure: InitFailurePolicy,

    /// Panic/restart policy
    #[serde(default)]
    pub on_panic: PanicPolicy,
}

impl Default for ErrorsSpec {
    fn default() -> Self {
        Self {
            on_init_failure: Default::default(),
            on_panic: Default::default(),
        }
    }
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
}
