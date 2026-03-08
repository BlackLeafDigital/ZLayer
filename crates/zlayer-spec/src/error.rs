//! Error types for the spec crate

use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;
use thiserror::Error;

/// Errors that can occur when parsing or validating a spec
#[derive(Debug, Error)]
pub enum SpecError {
    /// YAML parsing error
    #[error("YAML parse error: {0}")]
    YamlError(#[from] serde_yaml::Error),

    /// Validation error
    #[error("Validation error: {0}")]
    Validation(#[from] ValidationError),

    /// IO error when reading spec file
    #[error("IO error reading {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl From<std::io::Error> for SpecError {
    fn from(err: std::io::Error) -> Self {
        SpecError::Io {
            path: PathBuf::from("<unknown>"),
            source: err,
        }
    }
}

/// Validation errors for deployment specs
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ValidationError {
    /// The kind of validation error
    pub kind: ValidationErrorKind,

    /// JSON path to the invalid field
    pub path: String,
}

/// The specific kind of validation error
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ValidationErrorKind {
    /// Version is not "v1"
    InvalidVersion { found: String },

    /// Deployment name is empty
    EmptyDeploymentName,

    /// Service name is empty
    EmptyServiceName,

    /// Image name is empty
    EmptyImageName,

    /// Port is out of valid range (1-65535)
    InvalidPort { port: u32 },

    /// CPU limit is invalid (must be > 0)
    InvalidCpu { cpu: f64 },

    /// Memory format is invalid
    InvalidMemoryFormat { value: String },

    /// Duration format is invalid
    InvalidDuration { value: String },

    /// Service has duplicate endpoints
    DuplicateEndpoint { name: String },

    /// Unknown init action
    UnknownInitAction { action: String },

    /// Dependency references unknown service
    UnknownDependency { service: String },

    /// Circular dependency detected
    CircularDependency { service: String, depends_on: String },

    /// Scale min > max
    InvalidScaleRange { min: u32, max: u32 },

    /// Scale targets are empty in adaptive mode
    EmptyScaleTargets,

    /// Invalid environment variable
    InvalidEnvVar { key: String, reason: String },

    /// Invalid cron schedule expression
    InvalidCronSchedule { schedule: String, reason: String },

    /// Schedule field is only valid for rtype: cron
    ScheduleOnlyForCron,

    /// rtype: cron requires a schedule field
    CronRequiresSchedule,

    /// Generic validation error (from validator crate)
    Generic { message: String },

    /// Not enough nodes available for dedicated/exclusive placement
    InsufficientNodes {
        required: usize,
        available: usize,
        message: String,
    },

    /// Invalid tunnel protocol (must be tcp or udp)
    InvalidTunnelProtocol { protocol: String },

    /// Invalid tunnel port (must be 0 or 1-65535)
    InvalidTunnelPort { port: u16, field: String },

    /// Invalid tunnel TTL format
    InvalidTunnelTtl { value: String, reason: String },

    /// WASM config present on non-WASM service type
    WasmConfigOnNonWasmType,

    /// WASM `min_instances` > `max_instances`
    InvalidWasmInstanceRange { min: u32, max: u32 },

    /// WASM capability not available for this service type
    WasmCapabilityNotAvailable {
        capability: String,
        service_type: String,
    },

    /// `WasmHttp` service missing HTTP endpoint
    WasmHttpMissingHttpEndpoint,

    /// WASM preopen with empty source or target
    WasmPreopenEmpty { index: usize, field: String },
}

impl fmt::Display for ValidationErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidVersion { found } => write!(f, "invalid version (found: {found})"),
            Self::EmptyDeploymentName => write!(f, "deployment name is empty"),
            Self::EmptyServiceName => write!(f, "service name is empty"),
            Self::EmptyImageName => write!(f, "image name is empty"),
            Self::InvalidPort { port } => {
                write!(f, "port {port} is out of valid range (1-65535)")
            }
            Self::InvalidCpu { cpu } => write!(f, "CPU limit {cpu} is invalid (must be > 0)"),
            Self::InvalidMemoryFormat { value } => {
                write!(f, "memory format '{value}' is invalid")
            }
            Self::InvalidDuration { value } => write!(f, "duration format '{value}' is invalid"),
            Self::DuplicateEndpoint { name } => write!(f, "duplicate endpoint '{name}'"),
            Self::UnknownInitAction { action } => write!(f, "unknown init action '{action}'"),
            Self::UnknownDependency { service } => {
                write!(f, "dependency references unknown service '{service}'")
            }
            Self::CircularDependency {
                service,
                depends_on,
            } => write!(
                f,
                "circular dependency detected: '{service}' depends on '{depends_on}'"
            ),
            Self::InvalidScaleRange { min, max } => {
                write!(f, "invalid scale range: min {min} > max {max}")
            }
            Self::EmptyScaleTargets => write!(f, "scale targets are empty in adaptive mode"),
            Self::InvalidEnvVar { key, reason } => {
                write!(f, "invalid environment variable '{key}': {reason}")
            }
            Self::InvalidCronSchedule { schedule, reason } => {
                write!(f, "invalid cron schedule '{schedule}': {reason}")
            }
            Self::ScheduleOnlyForCron => {
                write!(f, "schedule field is only valid for rtype: cron")
            }
            Self::CronRequiresSchedule => {
                write!(f, "rtype: cron requires a schedule field")
            }
            Self::Generic { message } => write!(f, "{message}"),
            Self::InsufficientNodes {
                required,
                available,
                message,
            } => write!(
                f,
                "insufficient nodes: need {required} but only {available} available - {message}"
            ),
            Self::InvalidTunnelProtocol { protocol } => write!(
                f,
                "invalid tunnel protocol '{protocol}' (must be tcp or udp)"
            ),
            Self::InvalidTunnelPort { port, field } => {
                write!(
                    f,
                    "invalid tunnel {field} port: {port} (must be 0 or 1-65535)"
                )
            }
            Self::InvalidTunnelTtl { value, reason } => {
                write!(f, "invalid tunnel max_ttl '{value}': {reason}")
            }
            Self::WasmConfigOnNonWasmType => {
                write!(f, "wasm config provided but service_type is not a WASM type")
            }
            Self::InvalidWasmInstanceRange { min, max } => {
                write!(
                    f,
                    "wasm min_instances ({min}) > max_instances ({max})"
                )
            }
            Self::WasmCapabilityNotAvailable {
                capability,
                service_type,
            } => write!(
                f,
                "capability '{capability}' is not available for WASM service type '{service_type}' (world does not import it)"
            ),
            Self::WasmHttpMissingHttpEndpoint => {
                write!(
                    f,
                    "wasm_http service type should have at least one HTTP endpoint"
                )
            }
            Self::WasmPreopenEmpty { index, field } => {
                write!(f, "wasm preopen[{index}].{field} cannot be empty")
            }
        }
    }
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at {}", self.kind, self.path)
    }
}

impl std::error::Error for ValidationError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validation_error_display() {
        let err = ValidationError {
            kind: ValidationErrorKind::InvalidVersion {
                found: "v2".to_string(),
            },
            path: "version".to_string(),
        };
        assert!(err.to_string().contains("invalid version"));
    }
}
