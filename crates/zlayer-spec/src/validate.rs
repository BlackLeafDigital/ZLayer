//! Validation functions for `ZLayer` deployment specifications
//!
//! This module provides validators for all spec fields with proper error reporting.

use crate::error::{ValidationError, ValidationErrorKind};
use crate::types::{
    DeploymentSpec, EndpointSpec, EndpointTunnelConfig, Protocol, ResourceType, ScaleSpec,
    ServiceSpec, ServiceType, TunnelAccessConfig, TunnelDefinition,
};
use cron::Schedule;
use std::collections::HashSet;
use std::str::FromStr;

// =============================================================================
// Validator crate wrapper functions
// =============================================================================
// These functions match the signature expected by #[validate(custom(function = "..."))]
// They return Result<(), validator::ValidationError>

fn make_validation_error(
    code: &'static str,
    message: impl Into<std::borrow::Cow<'static, str>>,
) -> validator::ValidationError {
    let mut err = validator::ValidationError::new(code);
    err.message = Some(message.into());
    err
}

/// Wrapper for `validate_version` for use with validator crate
///
/// # Errors
///
/// Returns a validation error if the version is not "v1".
pub fn validate_version_wrapper(version: &str) -> Result<(), validator::ValidationError> {
    if version == "v1" {
        Ok(())
    } else {
        Err(make_validation_error(
            "invalid_version",
            format!("version must be 'v1', found '{version}'"),
        ))
    }
}

/// Wrapper for `validate_deployment_name` for use with validator crate
///
/// # Errors
///
/// Returns a validation error if the name is invalid.
pub fn validate_deployment_name_wrapper(name: &str) -> Result<(), validator::ValidationError> {
    // Check length
    if name.len() < 3 || name.len() > 63 {
        return Err(make_validation_error(
            "invalid_deployment_name",
            "deployment name must be 3-63 characters",
        ));
    }

    // Check first character is alphanumeric
    if let Some(first) = name.chars().next() {
        if !first.is_ascii_alphanumeric() {
            return Err(make_validation_error(
                "invalid_deployment_name",
                "deployment name must start with alphanumeric character",
            ));
        }
    }

    // Check all characters are alphanumeric or hyphens
    for c in name.chars() {
        if !c.is_ascii_alphanumeric() && c != '-' {
            return Err(make_validation_error(
                "invalid_deployment_name",
                "deployment name can only contain alphanumeric characters and hyphens",
            ));
        }
    }

    Ok(())
}

/// Wrapper for `validate_image_name` for use with validator crate
///
/// # Errors
///
/// Returns a validation error if the image name is empty.
pub fn validate_image_name_wrapper(name: &str) -> Result<(), validator::ValidationError> {
    if name.is_empty() || name.trim().is_empty() {
        Err(make_validation_error(
            "empty_image_name",
            "image name cannot be empty",
        ))
    } else {
        Ok(())
    }
}

/// Wrapper for `validate_cpu` for use with validator crate
/// Note: For `Option<f64>` fields, validator crate unwraps and passes the inner `f64`
///
/// # Errors
///
/// Returns a validation error if CPU is <= 0.
pub fn validate_cpu_option_wrapper(cpu: f64) -> Result<(), validator::ValidationError> {
    if cpu <= 0.0 {
        Err(make_validation_error(
            "invalid_cpu",
            format!("CPU limit must be > 0, found {cpu}"),
        ))
    } else {
        Ok(())
    }
}

/// Wrapper for `validate_memory_format` for use with validator crate
/// Note: For `Option<String>` fields, validator crate unwraps and passes `&String`
///
/// # Errors
///
/// Returns a validation error if the memory format is invalid.
pub fn validate_memory_option_wrapper(value: &String) -> Result<(), validator::ValidationError> {
    const VALID_SUFFIXES: [&str; 4] = ["Ki", "Mi", "Gi", "Ti"];

    let suffix_match = VALID_SUFFIXES
        .iter()
        .find(|&&suffix| value.ends_with(suffix));

    match suffix_match {
        Some(suffix) => {
            let numeric_part = &value[..value.len() - suffix.len()];
            match numeric_part.parse::<u64>() {
                Ok(n) if n > 0 => Ok(()),
                _ => Err(make_validation_error(
                    "invalid_memory_format",
                    format!("invalid memory format: '{value}'"),
                )),
            }
        }
        None => Err(make_validation_error(
            "invalid_memory_format",
            format!("invalid memory format: '{value}' (use Ki, Mi, Gi, or Ti suffix)"),
        )),
    }
}

/// Wrapper for `validate_port` for use with validator crate
/// Note: validator crate passes primitive types by value for custom validators
///
/// # Errors
///
/// Returns a validation error if the port is 0.
pub fn validate_port_wrapper(port: u16) -> Result<(), validator::ValidationError> {
    if port >= 1 {
        Ok(())
    } else {
        Err(make_validation_error(
            "invalid_port",
            "port must be between 1-65535",
        ))
    }
}

/// Validate scale range (min <= max) for `ScaleSpec`
///
/// # Errors
///
/// Returns a validation error if min > max.
pub fn validate_scale_spec(scale: &ScaleSpec) -> Result<(), validator::ValidationError> {
    if let ScaleSpec::Adaptive { min, max, .. } = scale {
        if *min > *max {
            return Err(make_validation_error(
                "invalid_scale_range",
                format!("scale min ({min}) cannot be greater than max ({max})"),
            ));
        }
    }
    Ok(())
}

/// Wrapper for `validate_cron_schedule` for use with validator crate
/// Note: For `Option<String>` fields, validator crate unwraps and passes `&String`
///
/// # Errors
///
/// Returns a validation error if the cron expression is invalid.
pub fn validate_schedule_wrapper(schedule: &String) -> Result<(), validator::ValidationError> {
    Schedule::from_str(schedule).map(|_| ()).map_err(|e| {
        make_validation_error(
            "invalid_cron_schedule",
            format!("invalid cron schedule '{schedule}': {e}"),
        )
    })
}

/// Validate a secret reference name format
///
/// Secret names must:
/// - Start with a letter (a-z, A-Z)
/// - Contain only alphanumeric characters, hyphens, and underscores
/// - Optionally be prefixed with `@service/` for cross-service references
///
/// Examples of valid secret refs:
/// - `$S:my-secret`
/// - `$S:api_key`
/// - `$S:@auth-service/jwt-secret`
///
/// # Errors
///
/// Returns a validation error if the secret reference format is invalid.
///
/// # Panics
///
/// Panics if the secret name is empty after validation (unreachable in practice).
pub fn validate_secret_reference(value: &str) -> Result<(), validator::ValidationError> {
    // Only validate values that start with $S:
    if !value.starts_with("$S:") {
        return Ok(());
    }

    let secret_ref = &value[3..]; // Remove "$S:" prefix

    if secret_ref.is_empty() {
        return Err(make_validation_error(
            "invalid_secret_reference",
            "secret reference cannot be empty after $S:",
        ));
    }

    // Check for cross-service reference format: @service/secret-name
    let secret_name = if let Some(rest) = secret_ref.strip_prefix('@') {
        // Cross-service reference: @service/secret-name
        let parts: Vec<&str> = rest.splitn(2, '/').collect();
        if parts.len() != 2 {
            return Err(make_validation_error(
                "invalid_secret_reference",
                format!(
                    "cross-service secret reference '{value}' must have format @service/secret-name"
                ),
            ));
        }

        let service_name = parts[0];
        let secret_name = parts[1];

        // Validate service name part
        if service_name.is_empty() {
            return Err(make_validation_error(
                "invalid_secret_reference",
                format!("service name in secret reference '{value}' cannot be empty"),
            ));
        }

        if !service_name.chars().next().unwrap().is_ascii_alphabetic() {
            return Err(make_validation_error(
                "invalid_secret_reference",
                format!("service name in secret reference '{value}' must start with a letter"),
            ));
        }

        for c in service_name.chars() {
            if !c.is_ascii_alphanumeric() && c != '-' && c != '_' {
                return Err(make_validation_error(
                    "invalid_secret_reference",
                    format!(
                        "service name in secret reference '{value}' contains invalid character '{c}'"
                    ),
                ));
            }
        }

        secret_name
    } else {
        secret_ref
    };

    // Validate the secret name
    if secret_name.is_empty() {
        return Err(make_validation_error(
            "invalid_secret_reference",
            format!("secret name in '{value}' cannot be empty"),
        ));
    }

    // Must start with a letter
    let first_char = secret_name.chars().next().unwrap();
    if !first_char.is_ascii_alphabetic() {
        return Err(make_validation_error(
            "invalid_secret_reference",
            format!("secret name in '{value}' must start with a letter, found '{first_char}'"),
        ));
    }

    // All characters must be alphanumeric, hyphen, or underscore
    for c in secret_name.chars() {
        if !c.is_ascii_alphanumeric() && c != '-' && c != '_' {
            return Err(make_validation_error(
                "invalid_secret_reference",
                format!(
                    "secret name in '{value}' contains invalid character '{c}' (only alphanumeric, hyphens, underscores allowed)"
                ),
            ));
        }
    }

    Ok(())
}

/// Validate all environment variable values in a service spec
///
/// # Errors
///
/// Returns a validation error if any env var has an invalid secret reference.
#[allow(clippy::implicit_hasher)]
pub fn validate_env_vars(
    service_name: &str,
    env: &std::collections::HashMap<String, String>,
) -> Result<(), crate::error::ValidationError> {
    for (key, value) in env {
        if let Err(e) = validate_secret_reference(value) {
            return Err(crate::error::ValidationError {
                kind: crate::error::ValidationErrorKind::InvalidEnvVar {
                    key: key.clone(),
                    reason: e
                        .message
                        .map_or_else(|| "invalid secret reference".to_string(), |m| m.to_string()),
                },
                path: format!("services.{service_name}.env.{key}"),
            });
        }
    }
    Ok(())
}

/// Validate storage name format (lowercase alphanumeric with hyphens)
///
/// # Errors
///
/// Returns a validation error if the storage name format is invalid.
///
/// # Panics
///
/// Panics if the regex pattern is invalid (should never happen with a static pattern).
pub fn validate_storage_name(name: &str) -> Result<(), validator::ValidationError> {
    // Must be lowercase alphanumeric with hyphens, not starting/ending with hyphen
    let re = regex::Regex::new(r"^[a-z0-9]([a-z0-9-]*[a-z0-9])?$").unwrap();
    if !re.is_match(name) || name.len() > 63 {
        return Err(make_validation_error(
            "invalid_storage_name",
            format!("storage name '{name}' must be lowercase alphanumeric with hyphens, 1-63 chars, not starting/ending with hyphen"),
        ));
    }
    Ok(())
}

/// Wrapper for `validate_storage_name` for use with validator crate
///
/// # Errors
///
/// Returns a validation error if the storage name format is invalid.
pub fn validate_storage_name_wrapper(name: &str) -> Result<(), validator::ValidationError> {
    validate_storage_name(name)
}

// =============================================================================
// Cross-field validation functions (called from lib.rs)
// =============================================================================

/// Validate that all dependency service references exist
///
/// # Errors
///
/// Returns a validation error if a dependency references an unknown service.
pub fn validate_dependencies(spec: &DeploymentSpec) -> Result<(), ValidationError> {
    let service_names: HashSet<&str> = spec
        .services
        .keys()
        .map(std::string::String::as_str)
        .collect();

    for (service_name, service_spec) in &spec.services {
        for dep in &service_spec.depends {
            if !service_names.contains(dep.service.as_str()) {
                return Err(ValidationError {
                    kind: ValidationErrorKind::UnknownDependency {
                        service: dep.service.clone(),
                    },
                    path: format!("services.{service_name}.depends"),
                });
            }
        }
    }

    Ok(())
}

/// Validate that each service has unique endpoint names
///
/// # Errors
///
/// Returns a validation error if any service has duplicate endpoint names.
pub fn validate_unique_service_endpoints(spec: &DeploymentSpec) -> Result<(), ValidationError> {
    for (service_name, service_spec) in &spec.services {
        let mut seen = HashSet::new();
        for endpoint in &service_spec.endpoints {
            if !seen.insert(&endpoint.name) {
                return Err(ValidationError {
                    kind: ValidationErrorKind::DuplicateEndpoint {
                        name: endpoint.name.clone(),
                    },
                    path: format!("services.{service_name}.endpoints"),
                });
            }
        }
    }

    Ok(())
}

/// Validate schedule/rtype consistency for all services
///
/// # Errors
///
/// Returns a validation error if schedule/rtype are inconsistent.
pub fn validate_cron_schedules(spec: &DeploymentSpec) -> Result<(), ValidationError> {
    for (service_name, service_spec) in &spec.services {
        validate_service_schedule(service_name, service_spec)?;
    }
    Ok(())
}

/// Validate schedule/rtype consistency for a single service
///
/// # Errors
///
/// Returns a validation error if a non-cron service has a schedule, or vice versa.
pub fn validate_service_schedule(
    service_name: &str,
    spec: &ServiceSpec,
) -> Result<(), ValidationError> {
    // If schedule is set, rtype must be Cron
    if spec.schedule.is_some() && spec.rtype != ResourceType::Cron {
        return Err(ValidationError {
            kind: ValidationErrorKind::ScheduleOnlyForCron,
            path: format!("services.{service_name}.schedule"),
        });
    }

    // If rtype is Cron, schedule must be set
    if spec.rtype == ResourceType::Cron && spec.schedule.is_none() {
        return Err(ValidationError {
            kind: ValidationErrorKind::CronRequiresSchedule,
            path: format!("services.{service_name}.schedule"),
        });
    }

    Ok(())
}

// =============================================================================
// Original validation functions (for direct use)
// =============================================================================

/// Validate that the version is "v1"
///
/// # Errors
///
/// Returns a validation error if the version is not "v1".
pub fn validate_version(version: &str) -> Result<(), ValidationError> {
    if version == "v1" {
        Ok(())
    } else {
        Err(ValidationError {
            kind: ValidationErrorKind::InvalidVersion {
                found: version.to_string(),
            },
            path: "version".to_string(),
        })
    }
}

/// Validate a deployment name
///
/// Requirements:
/// - 3-63 characters
/// - Alphanumeric + hyphens only
/// - Must start with alphanumeric character
///
/// # Errors
///
/// Returns a validation error if the deployment name is invalid.
pub fn validate_deployment_name(name: &str) -> Result<(), ValidationError> {
    // Check length
    if name.len() < 3 || name.len() > 63 {
        return Err(ValidationError {
            kind: ValidationErrorKind::EmptyDeploymentName,
            path: "deployment".to_string(),
        });
    }

    // Check first character is alphanumeric
    if let Some(first) = name.chars().next() {
        if !first.is_ascii_alphanumeric() {
            return Err(ValidationError {
                kind: ValidationErrorKind::EmptyDeploymentName,
                path: "deployment".to_string(),
            });
        }
    }

    // Check all characters are alphanumeric or hyphens
    for c in name.chars() {
        if !c.is_ascii_alphanumeric() && c != '-' {
            return Err(ValidationError {
                kind: ValidationErrorKind::EmptyDeploymentName,
                path: "deployment".to_string(),
            });
        }
    }

    Ok(())
}

/// Validate an image name
///
/// Requirements:
/// - Non-empty
/// - Not whitespace-only
///
/// # Errors
///
/// Returns a validation error if the image name is empty or whitespace-only.
pub fn validate_image_name(name: &str) -> Result<(), ValidationError> {
    if name.is_empty() || name.trim().is_empty() {
        Err(ValidationError {
            kind: ValidationErrorKind::EmptyImageName,
            path: "image.name".to_string(),
        })
    } else {
        Ok(())
    }
}

/// Validate CPU limit
///
/// Requirements:
/// - Must be > 0
///
/// # Errors
///
/// Returns a validation error if CPU is <= 0.
pub fn validate_cpu(cpu: &f64) -> Result<(), ValidationError> {
    if *cpu > 0.0 {
        Ok(())
    } else {
        Err(ValidationError {
            kind: ValidationErrorKind::InvalidCpu { cpu: *cpu },
            path: "resources.cpu".to_string(),
        })
    }
}

/// Validate memory format
///
/// Valid formats: number followed by Ki, Mi, Gi, or Ti suffix
/// Examples: "512Mi", "1Gi", "2Ti", "256Ki"
///
/// # Errors
///
/// Returns a validation error if the memory format is invalid.
pub fn validate_memory_format(value: &str) -> Result<(), ValidationError> {
    // Valid suffixes
    const VALID_SUFFIXES: [&str; 4] = ["Ki", "Mi", "Gi", "Ti"];

    // Find which suffix is used, if any
    let suffix_match = VALID_SUFFIXES
        .iter()
        .find(|&&suffix| value.ends_with(suffix));

    match suffix_match {
        Some(suffix) => {
            // Extract the numeric part
            let numeric_part = &value[..value.len() - suffix.len()];

            // Check that numeric part is a valid positive number
            match numeric_part.parse::<u64>() {
                Ok(n) if n > 0 => Ok(()),
                _ => Err(ValidationError {
                    kind: ValidationErrorKind::InvalidMemoryFormat {
                        value: value.to_string(),
                    },
                    path: "resources.memory".to_string(),
                }),
            }
        }
        None => Err(ValidationError {
            kind: ValidationErrorKind::InvalidMemoryFormat {
                value: value.to_string(),
            },
            path: "resources.memory".to_string(),
        }),
    }
}

/// Validate a port number
///
/// Requirements:
/// - Must be 1-65535 (not 0)
///
/// # Errors
///
/// Returns a validation error if the port is 0.
pub fn validate_port(port: &u16) -> Result<(), ValidationError> {
    if *port >= 1 {
        Ok(())
    } else {
        Err(ValidationError {
            kind: ValidationErrorKind::InvalidPort {
                port: u32::from(*port),
            },
            path: "endpoints[].port".to_string(),
        })
    }
}

/// Validate that endpoint names are unique within a service
///
/// # Errors
///
/// Returns a validation error if duplicate endpoint names are found.
pub fn validate_unique_endpoints(endpoints: &[EndpointSpec]) -> Result<(), ValidationError> {
    let mut seen = HashSet::new();

    for endpoint in endpoints {
        if !seen.insert(&endpoint.name) {
            return Err(ValidationError {
                kind: ValidationErrorKind::DuplicateEndpoint {
                    name: endpoint.name.clone(),
                },
                path: "endpoints".to_string(),
            });
        }
    }

    Ok(())
}

/// Validate scale range
///
/// Requirements:
/// - min <= max
///
/// # Errors
///
/// Returns a validation error if min > max.
pub fn validate_scale_range(min: u32, max: u32) -> Result<(), ValidationError> {
    if min <= max {
        Ok(())
    } else {
        Err(ValidationError {
            kind: ValidationErrorKind::InvalidScaleRange { min, max },
            path: "scale".to_string(),
        })
    }
}

// =============================================================================
// Tunnel validation functions
// =============================================================================

/// Validate tunnel TTL format (e.g., "4h", "30m", "1d")
///
/// # Errors
///
/// Returns a validation error if the TTL format is invalid.
pub fn validate_tunnel_ttl(ttl: &str) -> Result<(), validator::ValidationError> {
    humantime::parse_duration(ttl).map(|_| ()).map_err(|e| {
        make_validation_error(
            "invalid_tunnel_ttl",
            format!("invalid TTL format '{ttl}': {e}"),
        )
    })
}

/// Validate a `TunnelAccessConfig`
///
/// # Errors
///
/// Returns a validation error if the TTL format is invalid.
pub fn validate_tunnel_access_config(
    config: &TunnelAccessConfig,
    path: &str,
) -> Result<(), ValidationError> {
    if let Some(ref max_ttl) = config.max_ttl {
        validate_tunnel_ttl(max_ttl).map_err(|e| ValidationError {
            kind: ValidationErrorKind::InvalidTunnelTtl {
                value: max_ttl.clone(),
                reason: e
                    .message
                    .map_or_else(|| "invalid duration format".to_string(), |m| m.to_string()),
            },
            path: format!("{path}.access.max_ttl"),
        })?;
    }
    Ok(())
}

/// Validate an `EndpointTunnelConfig`
///
/// # Errors
///
/// Returns a validation error if the access config has an invalid TTL.
pub fn validate_endpoint_tunnel_config(
    config: &EndpointTunnelConfig,
    path: &str,
) -> Result<(), ValidationError> {
    // remote_port: 0 means auto-assign, otherwise must be valid port
    // Note: u16 already constrains to 0-65535, so no additional check needed

    // Validate access config if present
    if let Some(ref access) = config.access {
        validate_tunnel_access_config(access, path)?;
    }

    Ok(())
}

/// Validate a top-level `TunnelDefinition`
///
/// # Errors
///
/// Returns a validation error if any port is 0.
pub fn validate_tunnel_definition(
    name: &str,
    tunnel: &TunnelDefinition,
) -> Result<(), ValidationError> {
    let path = format!("tunnels.{name}");

    // Validate local_port (must be 1-65535, not 0)
    if tunnel.local_port == 0 {
        return Err(ValidationError {
            kind: ValidationErrorKind::InvalidTunnelPort {
                port: tunnel.local_port,
                field: "local_port".to_string(),
            },
            path: format!("{path}.local_port"),
        });
    }

    // Validate remote_port (must be 1-65535, not 0)
    if tunnel.remote_port == 0 {
        return Err(ValidationError {
            kind: ValidationErrorKind::InvalidTunnelPort {
                port: tunnel.remote_port,
                field: "remote_port".to_string(),
            },
            path: format!("{path}.remote_port"),
        });
    }

    Ok(())
}

/// Validate all tunnels in a deployment spec
///
/// # Errors
///
/// Returns a validation error if any tunnel configuration is invalid.
pub fn validate_tunnels(spec: &DeploymentSpec) -> Result<(), ValidationError> {
    // Validate top-level tunnels
    for (name, tunnel) in &spec.tunnels {
        validate_tunnel_definition(name, tunnel)?;
    }

    // Validate endpoint tunnels
    for (service_name, service_spec) in &spec.services {
        for (idx, endpoint) in service_spec.endpoints.iter().enumerate() {
            if let Some(ref tunnel_config) = endpoint.tunnel {
                let path = format!("services.{service_name}.endpoints[{idx}].tunnel");
                validate_endpoint_tunnel_config(tunnel_config, &path)?;
            }
        }
    }

    Ok(())
}

// =============================================================================
// WASM validation functions
// =============================================================================

/// Validate WASM configuration for all services in a deployment spec
///
/// # Errors
///
/// Returns a validation error if any WASM configuration is invalid.
pub fn validate_wasm_configs(spec: &DeploymentSpec) -> Result<(), ValidationError> {
    for (service_name, service_spec) in &spec.services {
        validate_wasm_config(service_name, service_spec)?;
    }
    Ok(())
}

/// Validate WASM configuration for a single service
///
/// Checks:
/// - WASM config should not be present on non-WASM service types
/// - `min_instances` <= `max_instances`
/// - `max_memory` format validation
/// - Capability restriction validation (users can only restrict from defaults, not grant)
/// - `WasmHttp` must have at least one HTTP endpoint (if endpoints exist)
/// - Preopens must have non-empty source and target
///
/// # Errors
///
/// Returns a validation error if the WASM configuration is invalid.
pub fn validate_wasm_config(service_name: &str, spec: &ServiceSpec) -> Result<(), ValidationError> {
    // If service_type is NOT wasm but wasm config IS present, that's an error
    if !spec.service_type.is_wasm() && spec.wasm.is_some() {
        return Err(ValidationError {
            kind: ValidationErrorKind::WasmConfigOnNonWasmType,
            path: format!("services.{service_name}.wasm"),
        });
    }

    if let Some(ref wasm) = spec.wasm {
        validate_wasm_fields(service_name, wasm)?;
        validate_wasm_capabilities(service_name, spec, wasm)?;
        validate_wasm_http_endpoints(service_name, spec)?;
        validate_wasm_preopens(service_name, wasm)?;
    }

    Ok(())
}

/// Validate basic WASM config fields (memory format, instance range).
fn validate_wasm_fields(
    service_name: &str,
    wasm: &crate::types::WasmConfig,
) -> Result<(), ValidationError> {
    if let Some(ref max_mem) = wasm.max_memory {
        validate_memory_format(max_mem).map_err(|_| ValidationError {
            kind: ValidationErrorKind::InvalidMemoryFormat {
                value: max_mem.clone(),
            },
            path: format!("services.{service_name}.wasm.max_memory"),
        })?;
    }

    if wasm.min_instances > wasm.max_instances {
        return Err(ValidationError {
            kind: ValidationErrorKind::InvalidWasmInstanceRange {
                min: wasm.min_instances,
                max: wasm.max_instances,
            },
            path: format!("services.{service_name}.wasm"),
        });
    }

    Ok(())
}

/// Validate WASM capability restrictions against service type defaults.
fn validate_wasm_capabilities(
    service_name: &str,
    spec: &ServiceSpec,
    wasm: &crate::types::WasmConfig,
) -> Result<(), ValidationError> {
    let Some(ref caps) = wasm.capabilities else {
        return Ok(());
    };
    let Some(defaults) = spec.service_type.default_wasm_capabilities() else {
        return Ok(());
    };

    let checks: &[(&str, bool, bool)] = &[
        ("config", caps.config, defaults.config),
        ("keyvalue", caps.keyvalue, defaults.keyvalue),
        ("logging", caps.logging, defaults.logging),
        ("secrets", caps.secrets, defaults.secrets),
        ("metrics", caps.metrics, defaults.metrics),
        ("http_client", caps.http_client, defaults.http_client),
        ("cli", caps.cli, defaults.cli),
        ("filesystem", caps.filesystem, defaults.filesystem),
        ("sockets", caps.sockets, defaults.sockets),
    ];

    for &(cap_name, requested, default) in checks {
        validate_capability_restriction(
            service_name,
            spec.service_type,
            cap_name,
            requested,
            default,
        )?;
    }

    Ok(())
}

/// Validate that `WasmHttp` services have at least one HTTP endpoint when endpoints exist.
fn validate_wasm_http_endpoints(
    service_name: &str,
    spec: &ServiceSpec,
) -> Result<(), ValidationError> {
    if spec.service_type == ServiceType::WasmHttp && !spec.endpoints.is_empty() {
        let has_http_endpoint = spec
            .endpoints
            .iter()
            .any(|e| matches!(e.protocol, Protocol::Http | Protocol::Https));
        if !has_http_endpoint {
            return Err(ValidationError {
                kind: ValidationErrorKind::WasmHttpMissingHttpEndpoint,
                path: format!("services.{service_name}.endpoints"),
            });
        }
    }
    Ok(())
}

/// Validate that WASM preopens have non-empty source and target paths.
fn validate_wasm_preopens(
    service_name: &str,
    wasm: &crate::types::WasmConfig,
) -> Result<(), ValidationError> {
    for (i, preopen) in wasm.preopens.iter().enumerate() {
        if preopen.source.is_empty() {
            return Err(ValidationError {
                kind: ValidationErrorKind::WasmPreopenEmpty {
                    index: i,
                    field: "source".to_string(),
                },
                path: format!("services.{service_name}.wasm.preopens[{i}].source"),
            });
        }
        if preopen.target.is_empty() {
            return Err(ValidationError {
                kind: ValidationErrorKind::WasmPreopenEmpty {
                    index: i,
                    field: "target".to_string(),
                },
                path: format!("services.{service_name}.wasm.preopens[{i}].target"),
            });
        }
    }
    Ok(())
}

/// Validate that a requested capability does not exceed the default for the service type.
///
/// Users can only restrict capabilities from their defaults, not grant new ones.
///
/// # Errors
///
/// Returns a validation error if the user requests a capability that is not
/// available for this WASM service type.
fn validate_capability_restriction(
    service_name: &str,
    service_type: ServiceType,
    cap_name: &str,
    requested: bool,
    default: bool,
) -> Result<(), ValidationError> {
    if requested && !default {
        return Err(ValidationError {
            kind: ValidationErrorKind::WasmCapabilityNotAvailable {
                capability: cap_name.to_string(),
                service_type: format!("{service_type:?}"),
            },
            path: format!("services.{service_name}.wasm.capabilities.{cap_name}"),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ExposeType, Protocol};

    // Version validation tests
    #[test]
    fn test_validate_version_valid() {
        assert!(validate_version("v1").is_ok());
    }

    #[test]
    fn test_validate_version_invalid_v2() {
        let result = validate_version("v2");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(
            err.kind,
            ValidationErrorKind::InvalidVersion { found } if found == "v2"
        ));
    }

    #[test]
    fn test_validate_version_empty() {
        let result = validate_version("");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(
            err.kind,
            ValidationErrorKind::InvalidVersion { found } if found.is_empty()
        ));
    }

    // Deployment name validation tests
    #[test]
    fn test_validate_deployment_name_valid() {
        assert!(validate_deployment_name("my-app").is_ok());
        assert!(validate_deployment_name("api").is_ok());
        assert!(validate_deployment_name("my-service-123").is_ok());
        assert!(validate_deployment_name("a1b").is_ok());
    }

    #[test]
    fn test_validate_deployment_name_too_short() {
        assert!(validate_deployment_name("ab").is_err());
        assert!(validate_deployment_name("a").is_err());
        assert!(validate_deployment_name("").is_err());
    }

    #[test]
    fn test_validate_deployment_name_too_long() {
        let long_name = "a".repeat(64);
        assert!(validate_deployment_name(&long_name).is_err());
    }

    #[test]
    fn test_validate_deployment_name_invalid_chars() {
        assert!(validate_deployment_name("my_app").is_err()); // underscore
        assert!(validate_deployment_name("my.app").is_err()); // dot
        assert!(validate_deployment_name("my app").is_err()); // space
        assert!(validate_deployment_name("my@app").is_err()); // special char
    }

    #[test]
    fn test_validate_deployment_name_must_start_alphanumeric() {
        assert!(validate_deployment_name("-myapp").is_err());
        assert!(validate_deployment_name("_myapp").is_err());
    }

    // Image name validation tests
    #[test]
    fn test_validate_image_name_valid() {
        assert!(validate_image_name("nginx:latest").is_ok());
        assert!(validate_image_name("ghcr.io/org/api:v1.2.3").is_ok());
        assert!(validate_image_name("ubuntu").is_ok());
    }

    #[test]
    fn test_validate_image_name_empty() {
        let result = validate_image_name("");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err().kind,
            ValidationErrorKind::EmptyImageName
        ));
    }

    #[test]
    fn test_validate_image_name_whitespace_only() {
        assert!(validate_image_name("   ").is_err());
        assert!(validate_image_name("\t\n").is_err());
    }

    // CPU validation tests
    #[test]
    fn test_validate_cpu_valid() {
        assert!(validate_cpu(&0.5).is_ok());
        assert!(validate_cpu(&1.0).is_ok());
        assert!(validate_cpu(&2.0).is_ok());
        assert!(validate_cpu(&0.001).is_ok());
    }

    #[test]
    fn test_validate_cpu_zero() {
        let result = validate_cpu(&0.0);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err().kind,
            ValidationErrorKind::InvalidCpu { cpu } if cpu == 0.0
        ));
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn test_validate_cpu_negative() {
        let result = validate_cpu(&-1.0);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err().kind,
            ValidationErrorKind::InvalidCpu { cpu } if cpu == -1.0
        ));
    }

    // Memory format validation tests
    #[test]
    fn test_validate_memory_format_valid() {
        assert!(validate_memory_format("512Mi").is_ok());
        assert!(validate_memory_format("1Gi").is_ok());
        assert!(validate_memory_format("2Ti").is_ok());
        assert!(validate_memory_format("256Ki").is_ok());
        assert!(validate_memory_format("4096Mi").is_ok());
    }

    #[test]
    fn test_validate_memory_format_invalid_suffix() {
        assert!(validate_memory_format("512MB").is_err());
        assert!(validate_memory_format("1GB").is_err());
        assert!(validate_memory_format("512").is_err());
        assert!(validate_memory_format("512m").is_err());
    }

    #[test]
    fn test_validate_memory_format_no_number() {
        assert!(validate_memory_format("Mi").is_err());
        assert!(validate_memory_format("Gi").is_err());
    }

    #[test]
    fn test_validate_memory_format_invalid_number() {
        assert!(validate_memory_format("-512Mi").is_err());
        assert!(validate_memory_format("0Mi").is_err());
        assert!(validate_memory_format("abcMi").is_err());
    }

    // Port validation tests
    #[test]
    fn test_validate_port_valid() {
        assert!(validate_port(&1).is_ok());
        assert!(validate_port(&80).is_ok());
        assert!(validate_port(&443).is_ok());
        assert!(validate_port(&8080).is_ok());
        assert!(validate_port(&65535).is_ok());
    }

    #[test]
    fn test_validate_port_zero() {
        let result = validate_port(&0);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err().kind,
            ValidationErrorKind::InvalidPort { port } if port == 0
        ));
    }

    // Note: u16 cannot be negative, and max value 65535 is valid,
    // so we only need to test 0 as the invalid case

    // Unique endpoints validation tests
    #[test]
    fn test_validate_unique_endpoints_valid() {
        let endpoints = vec![
            EndpointSpec {
                name: "http".to_string(),
                protocol: Protocol::Http,
                port: 8080,
                target_port: None,
                path: None,
                host: None,
                expose: ExposeType::Public,
                stream: None,
                tunnel: None,
            },
            EndpointSpec {
                name: "grpc".to_string(),
                protocol: Protocol::Tcp,
                port: 9090,
                target_port: None,
                path: None,
                host: None,
                expose: ExposeType::Internal,
                stream: None,
                tunnel: None,
            },
        ];
        assert!(validate_unique_endpoints(&endpoints).is_ok());
    }

    #[test]
    fn test_validate_unique_endpoints_empty() {
        let endpoints: Vec<EndpointSpec> = vec![];
        assert!(validate_unique_endpoints(&endpoints).is_ok());
    }

    #[test]
    fn test_validate_unique_endpoints_duplicates() {
        let endpoints = vec![
            EndpointSpec {
                name: "http".to_string(),
                protocol: Protocol::Http,
                port: 8080,
                target_port: None,
                path: None,
                host: None,
                expose: ExposeType::Public,
                stream: None,
                tunnel: None,
            },
            EndpointSpec {
                name: "http".to_string(), // duplicate name
                protocol: Protocol::Https,
                port: 8443,
                target_port: None,
                path: None,
                host: None,
                expose: ExposeType::Public,
                stream: None,
                tunnel: None,
            },
        ];
        let result = validate_unique_endpoints(&endpoints);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err().kind,
            ValidationErrorKind::DuplicateEndpoint { name } if name == "http"
        ));
    }

    // Scale range validation tests
    #[test]
    fn test_validate_scale_range_valid() {
        assert!(validate_scale_range(1, 10).is_ok());
        assert!(validate_scale_range(1, 1).is_ok()); // min == max is valid
        assert!(validate_scale_range(0, 5).is_ok());
        assert!(validate_scale_range(5, 100).is_ok());
    }

    #[test]
    fn test_validate_scale_range_min_greater_than_max() {
        let result = validate_scale_range(10, 5);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(
            err.kind,
            ValidationErrorKind::InvalidScaleRange { min: 10, max: 5 }
        ));
    }

    #[test]
    fn test_validate_scale_range_large_gap() {
        // Large gap between min and max should still be valid
        assert!(validate_scale_range(1, 1000).is_ok());
    }

    // Cron schedule validation tests
    // Note: The `cron` crate uses 7-field format: "sec min hour day-of-month month day-of-week year"
    #[test]
    fn test_validate_schedule_wrapper_valid() {
        // Valid 7-field cron expressions (sec min hour dom month dow year)
        assert!(validate_schedule_wrapper(&"0 0 0 * * * *".to_string()).is_ok()); // Daily at midnight
        assert!(validate_schedule_wrapper(&"0 */5 * * * * *".to_string()).is_ok()); // Every 5 minutes
        assert!(validate_schedule_wrapper(&"0 0 12 * * MON-FRI *".to_string()).is_ok()); // Weekdays at noon
        assert!(validate_schedule_wrapper(&"0 30 2 1 * * *".to_string()).is_ok()); // Monthly at 2:30am on 1st
        assert!(validate_schedule_wrapper(&"*/10 * * * * * *".to_string()).is_ok());
        // Every 10 seconds
    }

    #[test]
    fn test_validate_schedule_wrapper_invalid() {
        // Invalid cron expressions
        assert!(validate_schedule_wrapper(&String::new()).is_err()); // Empty
        assert!(validate_schedule_wrapper(&"not a cron".to_string()).is_err()); // Plain text
        assert!(validate_schedule_wrapper(&"0 0 * * *".to_string()).is_err()); // 5-field (standard unix cron) not supported
        assert!(validate_schedule_wrapper(&"60 0 0 * * * *".to_string()).is_err());
        // Invalid second (60)
    }

    // Secret reference validation tests
    #[test]
    fn test_validate_secret_reference_plain_values() {
        // Plain values should pass (not secret refs)
        assert!(validate_secret_reference("my-value").is_ok());
        assert!(validate_secret_reference("").is_ok());
        assert!(validate_secret_reference("some string").is_ok());
        assert!(validate_secret_reference("$E:MY_VAR").is_ok()); // Host env ref, not secret
    }

    #[test]
    fn test_validate_secret_reference_valid() {
        // Valid secret references
        assert!(validate_secret_reference("$S:my-secret").is_ok());
        assert!(validate_secret_reference("$S:api_key").is_ok());
        assert!(validate_secret_reference("$S:MySecret123").is_ok());
        assert!(validate_secret_reference("$S:a").is_ok()); // Single letter is valid
    }

    #[test]
    fn test_validate_secret_reference_cross_service() {
        // Valid cross-service references
        assert!(validate_secret_reference("$S:@auth-service/jwt-secret").is_ok());
        assert!(validate_secret_reference("$S:@my_service/api_key").is_ok());
        assert!(validate_secret_reference("$S:@svc/secret").is_ok());
    }

    #[test]
    fn test_validate_secret_reference_empty_after_prefix() {
        // Empty after $S:
        assert!(validate_secret_reference("$S:").is_err());
    }

    #[test]
    fn test_validate_secret_reference_must_start_with_letter() {
        // Secret name must start with letter
        assert!(validate_secret_reference("$S:123-secret").is_err());
        assert!(validate_secret_reference("$S:-my-secret").is_err());
        assert!(validate_secret_reference("$S:_underscore").is_err());
    }

    #[test]
    fn test_validate_secret_reference_invalid_chars() {
        // Invalid characters in secret name
        assert!(validate_secret_reference("$S:my.secret").is_err());
        assert!(validate_secret_reference("$S:my secret").is_err());
        assert!(validate_secret_reference("$S:my@secret").is_err());
    }

    #[test]
    fn test_validate_secret_reference_cross_service_invalid() {
        // Missing slash in cross-service ref
        assert!(validate_secret_reference("$S:@service").is_err());
        // Empty service name
        assert!(validate_secret_reference("$S:@/secret").is_err());
        // Empty secret name
        assert!(validate_secret_reference("$S:@service/").is_err());
        // Service name must start with letter
        assert!(validate_secret_reference("$S:@123-service/secret").is_err());
    }

    // =========================================================================
    // Tunnel validation tests
    // =========================================================================

    #[test]
    fn test_validate_tunnel_ttl_valid() {
        assert!(validate_tunnel_ttl("30m").is_ok());
        assert!(validate_tunnel_ttl("4h").is_ok());
        assert!(validate_tunnel_ttl("1d").is_ok());
        assert!(validate_tunnel_ttl("1h 30m").is_ok());
        assert!(validate_tunnel_ttl("2h30m").is_ok());
    }

    #[test]
    fn test_validate_tunnel_ttl_invalid() {
        assert!(validate_tunnel_ttl("").is_err());
        assert!(validate_tunnel_ttl("invalid").is_err());
        assert!(validate_tunnel_ttl("30").is_err()); // Missing unit
        assert!(validate_tunnel_ttl("-1h").is_err()); // Negative
    }

    #[test]
    fn test_validate_tunnel_definition_valid() {
        let tunnel = TunnelDefinition {
            from: "node-a".to_string(),
            to: "node-b".to_string(),
            local_port: 8080,
            remote_port: 9000,
            protocol: crate::types::TunnelProtocol::Tcp,
            expose: ExposeType::Internal,
        };
        assert!(validate_tunnel_definition("test-tunnel", &tunnel).is_ok());
    }

    #[test]
    fn test_validate_tunnel_definition_local_port_zero() {
        let tunnel = TunnelDefinition {
            from: "node-a".to_string(),
            to: "node-b".to_string(),
            local_port: 0,
            remote_port: 9000,
            protocol: crate::types::TunnelProtocol::Tcp,
            expose: ExposeType::Internal,
        };
        let result = validate_tunnel_definition("test-tunnel", &tunnel);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err().kind,
            ValidationErrorKind::InvalidTunnelPort { field, .. } if field == "local_port"
        ));
    }

    #[test]
    fn test_validate_tunnel_definition_remote_port_zero() {
        let tunnel = TunnelDefinition {
            from: "node-a".to_string(),
            to: "node-b".to_string(),
            local_port: 8080,
            remote_port: 0,
            protocol: crate::types::TunnelProtocol::Tcp,
            expose: ExposeType::Internal,
        };
        let result = validate_tunnel_definition("test-tunnel", &tunnel);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err().kind,
            ValidationErrorKind::InvalidTunnelPort { field, .. } if field == "remote_port"
        ));
    }

    #[test]
    fn test_validate_endpoint_tunnel_config_valid() {
        let config = EndpointTunnelConfig {
            enabled: true,
            from: Some("node-1".to_string()),
            to: Some("ingress".to_string()),
            remote_port: 8080,
            expose: Some(ExposeType::Public),
            access: None,
        };
        assert!(validate_endpoint_tunnel_config(&config, "test.tunnel").is_ok());
    }

    #[test]
    fn test_validate_endpoint_tunnel_config_with_access() {
        let config = EndpointTunnelConfig {
            enabled: true,
            from: None,
            to: None,
            remote_port: 0, // auto-assign
            expose: None,
            access: Some(TunnelAccessConfig {
                enabled: true,
                max_ttl: Some("4h".to_string()),
                audit: true,
            }),
        };
        assert!(validate_endpoint_tunnel_config(&config, "test.tunnel").is_ok());
    }

    #[test]
    fn test_validate_endpoint_tunnel_config_invalid_ttl() {
        let config = EndpointTunnelConfig {
            enabled: true,
            from: None,
            to: None,
            remote_port: 0,
            expose: None,
            access: Some(TunnelAccessConfig {
                enabled: true,
                max_ttl: Some("invalid".to_string()),
                audit: false,
            }),
        };
        let result = validate_endpoint_tunnel_config(&config, "test.tunnel");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err().kind,
            ValidationErrorKind::InvalidTunnelTtl { .. }
        ));
    }

    // =========================================================================
    // WASM validation tests
    // =========================================================================

    #[test]
    fn test_validate_capability_restriction_allowed() {
        // Requesting a capability that defaults to true is fine
        let result = validate_capability_restriction(
            "test-svc",
            ServiceType::WasmHttp,
            "config",
            true,
            true,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_capability_restriction_restricting_is_ok() {
        // Restricting a capability (setting false when default is true) is fine
        let result = validate_capability_restriction(
            "test-svc",
            ServiceType::WasmHttp,
            "config",
            false,
            true,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_capability_restriction_granting_not_allowed() {
        // Requesting a capability that defaults to false is not allowed
        let result = validate_capability_restriction(
            "test-svc",
            ServiceType::WasmHttp,
            "secrets",
            true,
            false,
        );
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err().kind,
            ValidationErrorKind::WasmCapabilityNotAvailable { ref capability, .. }
            if capability == "secrets"
        ));
    }

    #[test]
    fn test_validate_capability_restriction_both_false_is_ok() {
        // Both false is fine (not requesting, not available)
        let result = validate_capability_restriction(
            "test-svc",
            ServiceType::WasmTransformer,
            "sockets",
            false,
            false,
        );
        assert!(result.is_ok());
    }
}
