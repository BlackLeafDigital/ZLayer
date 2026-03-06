//! `ZLayer` V1 Service Specification
//!
//! This crate provides types for parsing and validating `ZLayer` deployment specifications.

mod error;
mod types;
mod validate;

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
    let spec: DeploymentSpec = serde_yml::from_str(yaml)?;

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
        assert!(result.is_ok(), "Valid dependency should pass: {:?}", result);
    }

    // =========================================================================
    // Cron schedule validation tests (Feature 4, Phase 1)
    // =========================================================================

    #[test]
    fn test_valid_cron_job_with_schedule() {
        let yaml = r#"
version: v1
deployment: my-app
services:
  cleanup:
    rtype: cron
    image:
      name: cleanup:latest
    schedule: "0 0 0 * * * *"
"#;
        let result = from_yaml_str(yaml);
        assert!(result.is_ok(), "Valid cron job should pass: {:?}", result);
        let spec = result.unwrap();
        let cleanup = spec.services.get("cleanup").unwrap();
        assert_eq!(cleanup.rtype, ResourceType::Cron);
        assert_eq!(cleanup.schedule, Some("0 0 0 * * * *".to_string()));
    }

    #[test]
    fn test_cron_without_schedule_rejected() {
        let yaml = r#"
version: v1
deployment: my-app
services:
  cleanup:
    rtype: cron
    image:
      name: cleanup:latest
"#;
        let result = from_yaml_str(yaml);
        assert!(result.is_err());
        let err = result.unwrap_err();
        let err_str = err.to_string();
        assert!(
            err_str.contains("schedule") || err_str.contains("cron"),
            "Error should mention missing schedule: {}",
            err_str
        );
    }

    #[test]
    fn test_service_with_schedule_rejected() {
        let yaml = r#"
version: v1
deployment: my-app
services:
  api:
    rtype: service
    image:
      name: api:latest
    schedule: "0 0 0 * * * *"
"#;
        let result = from_yaml_str(yaml);
        assert!(result.is_err());
        let err = result.unwrap_err();
        let err_str = err.to_string();
        assert!(
            err_str.contains("schedule") || err_str.contains("cron"),
            "Error should mention schedule/cron mismatch: {}",
            err_str
        );
    }

    #[test]
    fn test_job_with_schedule_rejected() {
        let yaml = r#"
version: v1
deployment: my-app
services:
  backup:
    rtype: job
    image:
      name: backup:latest
    schedule: "0 0 0 * * * *"
"#;
        let result = from_yaml_str(yaml);
        assert!(result.is_err());
        let err = result.unwrap_err();
        let err_str = err.to_string();
        assert!(
            err_str.contains("schedule") || err_str.contains("cron"),
            "Error should mention schedule/cron mismatch: {}",
            err_str
        );
    }

    #[test]
    fn test_invalid_cron_expression_rejected() {
        let yaml = r#"
version: v1
deployment: my-app
services:
  cleanup:
    rtype: cron
    image:
      name: cleanup:latest
    schedule: "not a valid cron"
"#;
        let result = from_yaml_str(yaml);
        assert!(result.is_err());
        let err = result.unwrap_err();
        let err_str = err.to_string();
        assert!(
            err_str.contains("cron") || err_str.contains("schedule") || err_str.contains("invalid"),
            "Error should mention invalid cron expression: {}",
            err_str
        );
    }

    #[test]
    fn test_valid_extended_cron_expression() {
        let yaml = r#"
version: v1
deployment: my-app
services:
  cleanup:
    rtype: cron
    image:
      name: cleanup:latest
    schedule: "0 30 2 * * * *"
"#;
        let result = from_yaml_str(yaml);
        assert!(
            result.is_ok(),
            "Extended cron expression should be valid: {:?}",
            result
        );
    }

    #[test]
    fn test_mixed_service_types_valid() {
        let yaml = r#"
version: v1
deployment: my-app
services:
  api:
    rtype: service
    image:
      name: api:latest
    endpoints:
      - name: http
        protocol: http
        port: 8080
  backup:
    rtype: job
    image:
      name: backup:latest
  cleanup:
    rtype: cron
    image:
      name: cleanup:latest
    schedule: "0 0 0 * * * *"
"#;
        let result = from_yaml_str(yaml);
        assert!(
            result.is_ok(),
            "Mixed service types should be valid: {:?}",
            result
        );
        let spec = result.unwrap();
        assert_eq!(spec.services.len(), 3);
        assert_eq!(spec.services["api"].rtype, ResourceType::Service);
        assert_eq!(spec.services["backup"].rtype, ResourceType::Job);
        assert_eq!(spec.services["cleanup"].rtype, ResourceType::Cron);
    }

    // =========================================================================
    // Tunnel integration tests
    // =========================================================================

    #[test]
    fn test_valid_endpoint_tunnel() {
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
        tunnel:
          enabled: true
          remote_port: 8080
"#;
        let result = from_yaml_str(yaml);
        assert!(
            result.is_ok(),
            "Valid endpoint tunnel should pass: {:?}",
            result
        );
    }

    #[test]
    fn test_valid_top_level_tunnel() {
        let yaml = r#"
version: v1
deployment: my-app
services:
  api:
    image:
      name: api:latest
tunnels:
  db-access:
    from: api-node
    to: db-node
    local_port: 5432
    remote_port: 5432
"#;
        let result = from_yaml_str(yaml);
        assert!(
            result.is_ok(),
            "Valid top-level tunnel should pass: {:?}",
            result
        );
        let spec = result.unwrap();
        assert!(spec.tunnels.contains_key("db-access"));
    }

    #[test]
    fn test_invalid_tunnel_ttl_rejected() {
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
        tunnel:
          enabled: true
          access:
            enabled: true
            max_ttl: invalid
"#;
        let result = from_yaml_str(yaml);
        assert!(result.is_err());
        let err = result.unwrap_err();
        let err_str = err.to_string();
        assert!(
            err_str.contains("ttl") || err_str.contains("TTL") || err_str.contains("invalid"),
            "Error should mention invalid TTL: {}",
            err_str
        );
    }

    #[test]
    fn test_invalid_tunnel_local_port_zero_rejected() {
        let yaml = r#"
version: v1
deployment: my-app
services: {}
tunnels:
  bad-tunnel:
    from: node-a
    to: node-b
    local_port: 0
    remote_port: 8080
"#;
        let result = from_yaml_str(yaml);
        assert!(result.is_err());
        let err = result.unwrap_err();
        let err_str = err.to_string();
        assert!(
            err_str.contains("port") || err_str.contains("local"),
            "Error should mention invalid port: {}",
            err_str
        );
    }

    #[test]
    fn test_invalid_tunnel_remote_port_zero_rejected() {
        let yaml = r#"
version: v1
deployment: my-app
services: {}
tunnels:
  bad-tunnel:
    from: node-a
    to: node-b
    local_port: 8080
    remote_port: 0
"#;
        let result = from_yaml_str(yaml);
        assert!(result.is_err());
        let err = result.unwrap_err();
        let err_str = err.to_string();
        assert!(
            err_str.contains("port") || err_str.contains("remote"),
            "Error should mention invalid port: {}",
            err_str
        );
    }

    #[test]
    fn test_valid_tunnel_with_access_config() {
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
        tunnel:
          enabled: true
          remote_port: 0
          access:
            enabled: true
            max_ttl: 4h
            audit: true
"#;
        let result = from_yaml_str(yaml);
        assert!(
            result.is_ok(),
            "Valid tunnel with access config should pass: {:?}",
            result
        );
        let spec = result.unwrap();
        let tunnel = spec.services["api"].endpoints[0].tunnel.as_ref().unwrap();
        let access = tunnel.access.as_ref().unwrap();
        assert!(access.enabled);
        assert_eq!(access.max_ttl, Some("4h".to_string()));
        assert!(access.audit);
    }

    // =========================================================================
    // WASM validation integration tests
    // =========================================================================

    #[test]
    fn test_wasm_config_on_non_wasm_type_rejected() {
        let yaml = r#"
version: v1
deployment: my-app
services:
  api:
    image:
      name: api:latest
    service_type: standard
    wasm:
      min_instances: 1
      max_instances: 4
"#;
        let result = from_yaml_str(yaml);
        assert!(result.is_err());
        let err_str = result.unwrap_err().to_string();
        assert!(
            err_str.contains("wasm") || err_str.contains("WASM"),
            "Error should mention wasm config on non-wasm type: {}",
            err_str
        );
    }

    #[test]
    fn test_wasm_service_without_config_is_ok() {
        // WASM service type without explicit wasm config is fine (defaults will be used)
        let yaml = r#"
version: v1
deployment: my-app
services:
  handler:
    image:
      name: handler:latest
    service_type: wasm_http
"#;
        let result = from_yaml_str(yaml);
        assert!(
            result.is_ok(),
            "WASM service without explicit config should pass: {:?}",
            result
        );
    }

    #[test]
    fn test_wasm_min_instances_gt_max_rejected() {
        let yaml = r#"
version: v1
deployment: my-app
services:
  handler:
    image:
      name: handler:latest
    service_type: wasm_http
    wasm:
      min_instances: 10
      max_instances: 2
"#;
        let result = from_yaml_str(yaml);
        assert!(result.is_err());
        let err_str = result.unwrap_err().to_string();
        assert!(
            err_str.contains("min_instances") || err_str.contains("max_instances"),
            "Error should mention instance range: {}",
            err_str
        );
    }

    #[test]
    fn test_wasm_valid_instance_range() {
        let yaml = r#"
version: v1
deployment: my-app
services:
  handler:
    image:
      name: handler:latest
    service_type: wasm_http
    wasm:
      min_instances: 2
      max_instances: 10
"#;
        let result = from_yaml_str(yaml);
        assert!(
            result.is_ok(),
            "Valid WASM instance range should pass: {:?}",
            result
        );
    }

    #[test]
    fn test_wasm_invalid_max_memory_format() {
        let yaml = r#"
version: v1
deployment: my-app
services:
  handler:
    image:
      name: handler:latest
    service_type: wasm_http
    wasm:
      max_memory: "512MB"
"#;
        let result = from_yaml_str(yaml);
        assert!(result.is_err());
        let err_str = result.unwrap_err().to_string();
        assert!(
            err_str.contains("memory") || err_str.contains("512MB"),
            "Error should mention invalid memory format: {}",
            err_str
        );
    }

    #[test]
    fn test_wasm_valid_max_memory_format() {
        let yaml = r#"
version: v1
deployment: my-app
services:
  handler:
    image:
      name: handler:latest
    service_type: wasm_http
    wasm:
      max_memory: "256Mi"
"#;
        let result = from_yaml_str(yaml);
        assert!(
            result.is_ok(),
            "Valid WASM max_memory should pass: {:?}",
            result
        );
    }

    #[test]
    fn test_wasm_capability_escalation_rejected() {
        // WasmTransformer defaults: secrets=false, so requesting secrets=true should fail
        let yaml = r#"
version: v1
deployment: my-app
services:
  transform:
    image:
      name: transform:latest
    service_type: wasm_transformer
    wasm:
      capabilities:
        secrets: true
"#;
        let result = from_yaml_str(yaml);
        assert!(result.is_err());
        let err_str = result.unwrap_err().to_string();
        assert!(
            err_str.contains("secrets") || err_str.contains("capability"),
            "Error should mention capability escalation: {}",
            err_str
        );
    }

    #[test]
    fn test_wasm_capability_restriction_allowed() {
        // WasmHttp defaults: config=true, keyvalue=true, logging=true, http_client=true
        // and metrics=false, secrets=false, cli=false, filesystem=false, sockets=false.
        // Restricting config=false while keeping all others at or below defaults should pass.
        let yaml = r#"
version: v1
deployment: my-app
services:
  handler:
    image:
      name: handler:latest
    service_type: wasm_http
    wasm:
      capabilities:
        config: false
        keyvalue: true
        logging: true
        secrets: false
        metrics: false
        http_client: true
        cli: false
        filesystem: false
        sockets: false
"#;
        let result = from_yaml_str(yaml);
        assert!(
            result.is_ok(),
            "Restricting a default capability should pass: {:?}",
            result
        );
    }

    #[test]
    fn test_wasm_http_with_tcp_only_endpoints_rejected() {
        // WasmHttp with endpoints but none are HTTP should fail
        let yaml = r#"
version: v1
deployment: my-app
services:
  handler:
    image:
      name: handler:latest
    service_type: wasm_http
    wasm:
      min_instances: 1
    endpoints:
      - name: raw
        protocol: tcp
        port: 9090
"#;
        let result = from_yaml_str(yaml);
        assert!(result.is_err());
        let err_str = result.unwrap_err().to_string();
        assert!(
            err_str.contains("HTTP") || err_str.contains("http") || err_str.contains("endpoint"),
            "Error should mention missing HTTP endpoint: {}",
            err_str
        );
    }

    #[test]
    fn test_wasm_http_with_http_endpoint_passes() {
        let yaml = r#"
version: v1
deployment: my-app
services:
  handler:
    image:
      name: handler:latest
    service_type: wasm_http
    wasm:
      min_instances: 1
    endpoints:
      - name: web
        protocol: http
        port: 8080
"#;
        let result = from_yaml_str(yaml);
        assert!(
            result.is_ok(),
            "WasmHttp with HTTP endpoint should pass: {:?}",
            result
        );
    }

    #[test]
    fn test_wasm_http_with_no_endpoints_passes() {
        // WasmHttp with no endpoints at all is fine (endpoints are optional)
        let yaml = r#"
version: v1
deployment: my-app
services:
  handler:
    image:
      name: handler:latest
    service_type: wasm_http
    wasm:
      min_instances: 1
"#;
        let result = from_yaml_str(yaml);
        assert!(
            result.is_ok(),
            "WasmHttp with no endpoints should pass: {:?}",
            result
        );
    }

    #[test]
    fn test_wasm_preopen_empty_source_rejected() {
        let yaml = r#"
version: v1
deployment: my-app
services:
  handler:
    image:
      name: handler:latest
    service_type: wasm_plugin
    wasm:
      preopens:
        - source: ""
          target: /data
"#;
        let result = from_yaml_str(yaml);
        assert!(result.is_err());
        let err_str = result.unwrap_err().to_string();
        assert!(
            err_str.contains("preopen") || err_str.contains("source") || err_str.contains("empty"),
            "Error should mention empty preopen source: {}",
            err_str
        );
    }

    #[test]
    fn test_wasm_preopen_empty_target_rejected() {
        let yaml = r#"
version: v1
deployment: my-app
services:
  handler:
    image:
      name: handler:latest
    service_type: wasm_plugin
    wasm:
      preopens:
        - source: /host/data
          target: ""
"#;
        let result = from_yaml_str(yaml);
        assert!(result.is_err());
        let err_str = result.unwrap_err().to_string();
        assert!(
            err_str.contains("preopen") || err_str.contains("target") || err_str.contains("empty"),
            "Error should mention empty preopen target: {}",
            err_str
        );
    }

    #[test]
    fn test_wasm_valid_preopen_passes() {
        let yaml = r#"
version: v1
deployment: my-app
services:
  handler:
    image:
      name: handler:latest
    service_type: wasm_plugin
    wasm:
      preopens:
        - source: /host/data
          target: /data
          readonly: true
"#;
        let result = from_yaml_str(yaml);
        assert!(
            result.is_ok(),
            "Valid WASM preopen should pass: {:?}",
            result
        );
    }

    #[test]
    fn test_wasm_plugin_can_use_all_capabilities() {
        // WasmPlugin has all capabilities except sockets as defaults
        let yaml = r#"
version: v1
deployment: my-app
services:
  plugin:
    image:
      name: plugin:latest
    service_type: wasm_plugin
    wasm:
      capabilities:
        config: true
        keyvalue: true
        logging: true
        secrets: true
        metrics: true
        http_client: true
        cli: true
        filesystem: true
"#;
        let result = from_yaml_str(yaml);
        assert!(
            result.is_ok(),
            "WasmPlugin with all its default capabilities should pass: {:?}",
            result
        );
    }

    #[test]
    fn test_wasm_plugin_cannot_use_sockets() {
        // WasmPlugin defaults: sockets=false
        let yaml = r#"
version: v1
deployment: my-app
services:
  plugin:
    image:
      name: plugin:latest
    service_type: wasm_plugin
    wasm:
      capabilities:
        sockets: true
"#;
        let result = from_yaml_str(yaml);
        assert!(result.is_err());
        let err_str = result.unwrap_err().to_string();
        assert!(
            err_str.contains("sockets") || err_str.contains("capability"),
            "Error should mention sockets capability escalation: {}",
            err_str
        );
    }

    #[test]
    fn test_wasm_equal_min_max_instances_passes() {
        let yaml = r#"
version: v1
deployment: my-app
services:
  handler:
    image:
      name: handler:latest
    service_type: wasm_http
    wasm:
      min_instances: 5
      max_instances: 5
"#;
        let result = from_yaml_str(yaml);
        assert!(
            result.is_ok(),
            "Equal min/max instances should pass: {:?}",
            result
        );
    }
}
