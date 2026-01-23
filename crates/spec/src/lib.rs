//! ZLayer V1 Service Specification
//!
//! This crate provides types for parsing and validating ZLayer deployment specifications.

mod error;
mod types;
mod validate;

pub use error::*;
pub use types::*;
pub use validate::*;

use validator::Validate;

/// Parse a deployment spec from YAML string
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

    Ok(spec)
}

/// Parse a deployment spec from YAML file
pub fn from_yaml_file(path: &std::path::Path) -> Result<DeploymentSpec, SpecError> {
    let content = std::fs::read_to_string(path)?;
    from_yaml_str(&content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_from_yaml_str() {
        let yaml = r#"
version: v1
deployment: test
services:
  hello:
    rtype: service
    image:
      name: hello-world:latest
"#;
        let result = from_yaml_str(yaml);
        assert!(result.is_ok());
        let spec = result.unwrap();
        assert_eq!(spec.version, "v1");
        assert_eq!(spec.deployment, "test");
    }

    // =========================================================================
    // Integration tests for validation (B.12)
    // =========================================================================

    #[test]
    fn test_invalid_version_rejected() {
        let yaml = r#"
version: v2
deployment: my-app
services:
  hello:
    image:
      name: hello-world:latest
"#;
        let result = from_yaml_str(yaml);
        assert!(result.is_err());
        let err = result.unwrap_err();
        let err_str = err.to_string();
        assert!(
            err_str.contains("version") || err_str.contains("v1"),
            "Error should mention version: {}",
            err_str
        );
    }

    #[test]
    fn test_empty_deployment_name_rejected() {
        let yaml = r#"
version: v1
deployment: ab
services:
  hello:
    image:
      name: hello-world:latest
"#;
        let result = from_yaml_str(yaml);
        assert!(result.is_err());
        let err = result.unwrap_err();
        let err_str = err.to_string();
        assert!(
            err_str.contains("deployment") || err_str.contains("3-63"),
            "Error should mention deployment name: {}",
            err_str
        );
    }

    #[test]
    fn test_invalid_port_zero_rejected() {
        let yaml = r#"
version: v1
deployment: my-app
services:
  hello:
    image:
      name: hello-world:latest
    endpoints:
      - name: http
        protocol: http
        port: 0
"#;
        let result = from_yaml_str(yaml);
        assert!(result.is_err());
        let err = result.unwrap_err();
        let err_str = err.to_string();
        assert!(
            err_str.contains("port"),
            "Error should mention port: {}",
            err_str
        );
    }

    #[test]
    fn test_unknown_dependency_rejected() {
        let yaml = r#"
version: v1
deployment: my-app
services:
  api:
    image:
      name: api:latest
    depends:
      - service: database
"#;
        let result = from_yaml_str(yaml);
        assert!(result.is_err());
        let err = result.unwrap_err();
        let err_str = err.to_string();
        assert!(
            err_str.contains("database") || err_str.contains("unknown"),
            "Error should mention unknown dependency: {}",
            err_str
        );
    }

    #[test]
    fn test_duplicate_endpoint_names_rejected() {
        let yaml = r#"
version: v1
deployment: my-app
services:
  api:
    image:
      name: api:latest
    endpoints:
      - name: http
        protocol: http
        port: 8080
      - name: http
        protocol: https
        port: 8443
"#;
        let result = from_yaml_str(yaml);
        assert!(result.is_err());
        let err = result.unwrap_err();
        let err_str = err.to_string();
        assert!(
            err_str.contains("http") || err_str.contains("duplicate"),
            "Error should mention duplicate endpoint: {}",
            err_str
        );
    }

    #[test]
    fn test_invalid_scale_range_min_gt_max_rejected() {
        let yaml = r#"
version: v1
deployment: my-app
services:
  api:
    image:
      name: api:latest
    scale:
      mode: adaptive
      min: 10
      max: 5
"#;
        let result = from_yaml_str(yaml);
        assert!(result.is_err());
        let err = result.unwrap_err();
        let err_str = err.to_string();
        assert!(
            err_str.contains("scale") || err_str.contains("min") || err_str.contains("max"),
            "Error should mention scale range: {}",
            err_str
        );
    }

    #[test]
    fn test_invalid_cpu_zero_rejected() {
        let yaml = r#"
version: v1
deployment: my-app
services:
  api:
    image:
      name: api:latest
    resources:
      cpu: 0
"#;
        let result = from_yaml_str(yaml);
        assert!(result.is_err());
        let err = result.unwrap_err();
        let err_str = err.to_string();
        assert!(
            err_str.contains("cpu") || err_str.contains("CPU"),
            "Error should mention CPU: {}",
            err_str
        );
    }

    #[test]
    fn test_invalid_memory_format_rejected() {
        let yaml = r#"
version: v1
deployment: my-app
services:
  api:
    image:
      name: api:latest
    resources:
      memory: 512MB
"#;
        let result = from_yaml_str(yaml);
        assert!(result.is_err());
        let err = result.unwrap_err();
        let err_str = err.to_string();
        assert!(
            err_str.contains("memory"),
            "Error should mention memory format: {}",
            err_str
        );
    }

    #[test]
    fn test_empty_image_name_rejected() {
        let yaml = r#"
version: v1
deployment: my-app
services:
  api:
    image:
      name: ""
"#;
        let result = from_yaml_str(yaml);
        assert!(result.is_err());
        let err = result.unwrap_err();
        let err_str = err.to_string();
        assert!(
            err_str.contains("image") || err_str.contains("empty"),
            "Error should mention empty image name: {}",
            err_str
        );
    }

    #[test]
    fn test_valid_spec_passes_validation() {
        let yaml = r#"
version: v1
deployment: my-production-app
services:
  api:
    image:
      name: ghcr.io/myorg/api:v1.2.3
    resources:
      cpu: 0.5
      memory: 512Mi
    endpoints:
      - name: http
        protocol: http
        port: 8080
        expose: public
      - name: metrics
        protocol: http
        port: 9090
        expose: internal
    scale:
      mode: adaptive
      min: 2
      max: 10
      targets:
        cpu: 70
    depends:
      - service: database
        condition: healthy
  database:
    image:
      name: postgres:15
    endpoints:
      - name: postgres
        protocol: tcp
        port: 5432
    scale:
      mode: fixed
      replicas: 1
"#;
        let result = from_yaml_str(yaml);
        assert!(result.is_ok(), "Valid spec should pass: {:?}", result);
        let spec = result.unwrap();
        assert_eq!(spec.version, "v1");
        assert_eq!(spec.deployment, "my-production-app");
        assert_eq!(spec.services.len(), 2);
    }

    #[test]
    fn test_valid_dependency_passes() {
        let yaml = r#"
version: v1
deployment: my-app
services:
  api:
    image:
      name: api:latest
    depends:
      - service: database
  database:
    image:
      name: postgres:15
"#;
        let result = from_yaml_str(yaml);
        assert!(
            result.is_ok(),
            "Valid dependency should pass: {:?}",
            result
        );
    }
}
