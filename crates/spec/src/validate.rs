//! Validation functions for ZLayer deployment specifications
//!
//! This module provides validators for all spec fields with proper error reporting.

use crate::error::{ValidationError, ValidationErrorKind};
use crate::types::{DeploymentSpec, EndpointSpec, ResourceType, ScaleSpec, ServiceSpec};
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

/// Wrapper for validate_version for use with validator crate
pub fn validate_version_wrapper(version: &str) -> Result<(), validator::ValidationError> {
    if version == "v1" {
        Ok(())
    } else {
        Err(make_validation_error(
            "invalid_version",
            format!("version must be 'v1', found '{}'", version),
        ))
    }
}

/// Wrapper for validate_deployment_name for use with validator crate
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

/// Wrapper for validate_image_name for use with validator crate
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

/// Wrapper for validate_cpu for use with validator crate
/// Note: For Option<f64> fields, validator crate unwraps and passes the inner f64
pub fn validate_cpu_option_wrapper(cpu: f64) -> Result<(), validator::ValidationError> {
    if cpu <= 0.0 {
        Err(make_validation_error(
            "invalid_cpu",
            format!("CPU limit must be > 0, found {}", cpu),
        ))
    } else {
        Ok(())
    }
}

/// Wrapper for validate_memory_format for use with validator crate
/// Note: For Option<String> fields, validator crate unwraps and passes &String
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
                    format!("invalid memory format: '{}'", value),
                )),
            }
        }
        None => Err(make_validation_error(
            "invalid_memory_format",
            format!(
                "invalid memory format: '{}' (use Ki, Mi, Gi, or Ti suffix)",
                value
            ),
        )),
    }
}

/// Wrapper for validate_port for use with validator crate
/// Note: validator crate passes primitive types by value for custom validators
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

/// Validate scale range (min <= max) for ScaleSpec
pub fn validate_scale_spec(scale: &ScaleSpec) -> Result<(), validator::ValidationError> {
    if let ScaleSpec::Adaptive { min, max, .. } = scale {
        if *min > *max {
            return Err(make_validation_error(
                "invalid_scale_range",
                format!("scale min ({}) cannot be greater than max ({})", min, max),
            ));
        }
    }
    Ok(())
}

/// Wrapper for validate_cron_schedule for use with validator crate
/// Note: For Option<String> fields, validator crate unwraps and passes &String
pub fn validate_schedule_wrapper(schedule: &String) -> Result<(), validator::ValidationError> {
    Schedule::from_str(schedule).map(|_| ()).map_err(|e| {
        make_validation_error(
            "invalid_cron_schedule",
            format!("invalid cron schedule '{}': {}", schedule, e),
        )
    })
}

// =============================================================================
// Cross-field validation functions (called from lib.rs)
// =============================================================================

/// Validate that all dependency service references exist
pub fn validate_dependencies(spec: &DeploymentSpec) -> Result<(), ValidationError> {
    let service_names: HashSet<&str> = spec.services.keys().map(|s| s.as_str()).collect();

    for (service_name, service_spec) in &spec.services {
        for dep in &service_spec.depends {
            if !service_names.contains(dep.service.as_str()) {
                return Err(ValidationError {
                    kind: ValidationErrorKind::UnknownDependency {
                        service: dep.service.clone(),
                    },
                    path: format!("services.{}.depends", service_name),
                });
            }
        }
    }

    Ok(())
}

/// Validate that each service has unique endpoint names
pub fn validate_unique_service_endpoints(spec: &DeploymentSpec) -> Result<(), ValidationError> {
    for (service_name, service_spec) in &spec.services {
        let mut seen = HashSet::new();
        for endpoint in &service_spec.endpoints {
            if !seen.insert(&endpoint.name) {
                return Err(ValidationError {
                    kind: ValidationErrorKind::DuplicateEndpoint {
                        name: endpoint.name.clone(),
                    },
                    path: format!("services.{}.endpoints", service_name),
                });
            }
        }
    }

    Ok(())
}

/// Validate schedule/rtype consistency for all services
pub fn validate_cron_schedules(spec: &DeploymentSpec) -> Result<(), ValidationError> {
    for (service_name, service_spec) in &spec.services {
        validate_service_schedule(service_name, service_spec)?;
    }
    Ok(())
}

/// Validate schedule/rtype consistency for a single service
pub fn validate_service_schedule(
    service_name: &str,
    spec: &ServiceSpec,
) -> Result<(), ValidationError> {
    // If schedule is set, rtype must be Cron
    if spec.schedule.is_some() && spec.rtype != ResourceType::Cron {
        return Err(ValidationError {
            kind: ValidationErrorKind::ScheduleOnlyForCron,
            path: format!("services.{}.schedule", service_name),
        });
    }

    // If rtype is Cron, schedule must be set
    if spec.rtype == ResourceType::Cron && spec.schedule.is_none() {
        return Err(ValidationError {
            kind: ValidationErrorKind::CronRequiresSchedule,
            path: format!("services.{}.schedule", service_name),
        });
    }

    Ok(())
}

// =============================================================================
// Original validation functions (for direct use)
// =============================================================================

/// Validate that the version is "v1"
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
pub fn validate_port(port: &u16) -> Result<(), ValidationError> {
    if *port >= 1 {
        Ok(())
    } else {
        Err(ValidationError {
            kind: ValidationErrorKind::InvalidPort { port: *port as u32 },
            path: "endpoints[].port".to_string(),
        })
    }
}

/// Validate that endpoint names are unique within a service
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
                path: None,
                expose: ExposeType::Public,
            },
            EndpointSpec {
                name: "grpc".to_string(),
                protocol: Protocol::Tcp,
                port: 9090,
                path: None,
                expose: ExposeType::Internal,
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
                path: None,
                expose: ExposeType::Public,
            },
            EndpointSpec {
                name: "http".to_string(), // duplicate name
                protocol: Protocol::Https,
                port: 8443,
                path: None,
                expose: ExposeType::Public,
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
        assert!(validate_schedule_wrapper(&"".to_string()).is_err()); // Empty
        assert!(validate_schedule_wrapper(&"not a cron".to_string()).is_err()); // Plain text
        assert!(validate_schedule_wrapper(&"0 0 * * *".to_string()).is_err()); // 5-field (standard unix cron) not supported
        assert!(validate_schedule_wrapper(&"60 0 0 * * * *".to_string()).is_err());
        // Invalid second (60)
    }
}
