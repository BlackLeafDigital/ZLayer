//! `ZLayer` V1 Service Specification
//!
//! This crate provides types for parsing and validating `ZLayer` deployment specifications.

pub mod error;
pub mod types;
pub mod validate;

pub use error::*;
pub use types::*;
pub use validate::*;

use validator::Validate;

/// Parse a deployment spec from YAML string
///
/// # Errors
///
/// Returns `SpecError` if parsing or validation fails.
pub fn from_yaml_str(yaml: &str) -> Result<DeploymentSpec, SpecError> {
    let spec: DeploymentSpec = serde_yaml::from_str(yaml)?;

    // Run validator crate validation
    spec.validate().map_err(|e| {
        SpecError::Validation(ValidationError {
            kind: ValidationErrorKind::Generic {
                message: e.to_string(),
            },
            path: String::new(),
        })
    })?;

    // Cross-field validation
    validate_dependencies(&spec)?;
    validate_unique_service_endpoints(&spec)?;
    validate_cron_schedules(&spec)?;
    validate_tunnels(&spec)?;
    validate_wasm_configs(&spec)?;

    Ok(spec)
}

/// Parse a deployment spec from YAML file
///
/// # Errors
///
/// Returns `SpecError` if the file cannot be read, or parsing/validation fails.
pub fn from_yaml_file(path: &std::path::Path) -> Result<DeploymentSpec, SpecError> {
    let content = std::fs::read_to_string(path)?;
    from_yaml_str(&content)
}
