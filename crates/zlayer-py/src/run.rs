//! Convenience run() function for quick container execution
//!
//! This module provides a simple, high-level function for running containers
//! without needing to manage Container or Runtime objects directly.

use crate::container::Container;
use crate::error::{to_py_result, ZLayerError};
use pyo3::prelude::*;
use std::collections::HashMap;
use std::sync::Arc;
use zlayer_agent::{ContainerId, ContainerState, MockRuntime, Runtime};
use zlayer_spec::DeploymentSpec;

/// Run a container with a simple, Docker-like interface
///
/// This is a convenience function for quickly running containers without
/// needing to manage Runtime or Container objects directly.
///
/// Args:
///     image: The container image to run (e.g., "nginx:latest", "redis:alpine")
///     name: Optional container name. If not provided, one is auto-generated.
///     ports: Optional port mappings as {container_port: host_port}.
///         Example: {80: 8080} maps container port 80 to host port 8080.
///     env: Optional environment variables as {name: value}.
///         Example: {"REDIS_PASSWORD": "secret"}
///     command: Optional command to run in the container.
///         Example: ["python", "-c", "print('hello')"]
///     detach: If True (default), run in background and return immediately.
///         If False, wait for the container to finish.
///
/// Returns:
///     A Container instance that can be used for further operations like
///     stop(), logs(), exec(), etc.
///
/// Raises:
///     ValueError: If the image name is empty or invalid.
///     RuntimeError: If container creation or startup fails.
///
/// Example:
///     >>> container = await zlayer.run("nginx:latest", ports={80: 8080})
///     >>> container = await zlayer.run("redis:alpine", name="my-redis")
///     >>> container = await zlayer.run("python:3.12", command=["python", "-c", "print('hello')"], detach=False)
#[pyfunction]
#[pyo3(signature = (image, name=None, ports=None, env=None, command=None, detach=true))]
pub fn run<'py>(
    py: Python<'py>,
    image: &str,
    name: Option<&str>,
    ports: Option<HashMap<u16, u16>>,
    env: Option<HashMap<String, String>>,
    command: Option<Vec<String>>,
    detach: bool,
) -> PyResult<Bound<'py, PyAny>> {
    // Validate image
    if image.is_empty() {
        return Err(ZLayerError::InvalidArgument("Image name cannot be empty".to_string()).into());
    }

    let image = image.to_string();
    let name = name.map(|s| s.to_string());
    let ports = ports.unwrap_or_default();
    let env = env.unwrap_or_default();
    let command = command.unwrap_or_default();

    pyo3_async_runtimes::tokio::future_into_py(py, async move {
        // Generate a name if not provided
        let container_name = name.unwrap_or_else(|| generate_container_name(&image));

        // Build the spec YAML
        let yaml = build_run_spec_yaml(&image, &container_name, &ports, &env, &command);

        // Parse the spec
        let deployment: DeploymentSpec = serde_yaml::from_str(&yaml).map_err(|e| {
            ZLayerError::InvalidArgument(format!("Failed to create container spec: {}", e))
        })?;

        let spec = deployment
            .services
            .get(&container_name)
            .cloned()
            .ok_or_else(|| {
                ZLayerError::InvalidArgument("Failed to extract service spec".to_string())
            })?;

        // Create the runtime (mock for now, will be configurable later)
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());

        let id = ContainerId {
            service: container_name.clone(),
            replica: 1,
        };

        // Create the container wrapper
        let container = Container::new(id.clone(), spec.clone(), runtime.clone());

        // Pull the image
        runtime
            .pull_image(&image)
            .await
            .map_err(ZLayerError::from)?;

        // Create the container
        runtime
            .create_container(&id, &spec)
            .await
            .map_err(ZLayerError::from)?;

        // Start the container
        runtime
            .start_container(&id)
            .await
            .map_err(ZLayerError::from)?;

        // If not detaching, wait for the container to finish
        if !detach {
            loop {
                let state = runtime
                    .container_state(&id)
                    .await
                    .map_err(ZLayerError::from)?;

                match state {
                    ContainerState::Exited { code } => {
                        if code != 0 {
                            return Err(ZLayerError::Container(format!(
                                "Container exited with code {}",
                                code
                            ))
                            .into());
                        }
                        break;
                    }
                    ContainerState::Failed { reason } => {
                        return Err(ZLayerError::Container(format!(
                            "Container failed: {}",
                            reason
                        ))
                        .into());
                    }
                    ContainerState::Running | ContainerState::Initializing => {
                        // Still running, wait a bit
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    }
                    ContainerState::Pending => {
                        // Still starting, wait a bit
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    }
                    ContainerState::Stopping => {
                        // Stopping, wait a bit more
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    }
                }
            }
        }

        to_py_result(Ok(container))
    })
}

/// Generate a container name from the image name
fn generate_container_name(image: &str) -> String {
    // Extract the image name without registry and tag
    let base = image
        .rsplit('/')
        .next()
        .unwrap_or(image)
        .split(':')
        .next()
        .unwrap_or(image);

    // Generate a short random suffix
    let suffix: u32 = rand::random::<u32>() % 10000;

    format!("{}-{}", base, suffix)
}

/// Build a YAML spec from run() parameters
fn build_run_spec_yaml(
    image: &str,
    name: &str,
    ports: &HashMap<u16, u16>,
    env: &HashMap<String, String>,
    command: &[String],
) -> String {
    let mut yaml = format!(
        r#"version: v1
deployment: zlayer-run
services:
  {}:
    rtype: service
    image:
      name: {}
"#,
        name, image
    );

    // Add command if provided
    if !command.is_empty() {
        yaml.push_str("    command:\n");
        for arg in command {
            // Escape quotes in the argument
            let escaped = arg.replace('"', "\\\"");
            yaml.push_str(&format!("      - \"{}\"\n", escaped));
        }
    }

    // Add environment variables
    if !env.is_empty() {
        yaml.push_str("    env:\n");
        for (key, value) in env {
            // Escape quotes in the value
            let escaped_value = value.replace('"', "\\\"");
            yaml.push_str(&format!("      {}: \"{}\"\n", key, escaped_value));
        }
    }

    // Add endpoints from ports
    // Convert {container_port: host_port} to endpoint specs
    if !ports.is_empty() {
        yaml.push_str("    endpoints:\n");
        for (container_port, host_port) in ports {
            yaml.push_str(&format!(
                "      - name: port-{}\n        protocol: tcp\n        port: {}\n        host_port: {}\n",
                container_port, container_port, host_port
            ));
        }
    }

    yaml
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_container_name() {
        let name = generate_container_name("nginx:latest");
        assert!(name.starts_with("nginx-"));

        let name = generate_container_name("docker.io/library/redis:alpine");
        assert!(name.starts_with("redis-"));

        let name = generate_container_name("myregistry.com/myapp");
        assert!(name.starts_with("myapp-"));
    }

    #[test]
    fn test_build_run_spec_yaml_basic() {
        let yaml = build_run_spec_yaml(
            "nginx:latest",
            "test-container",
            &HashMap::new(),
            &HashMap::new(),
            &[],
        );
        assert!(yaml.contains("nginx:latest"));
        assert!(yaml.contains("test-container:"));
    }

    #[test]
    fn test_build_run_spec_yaml_with_ports() {
        let mut ports = HashMap::new();
        ports.insert(80, 8080);
        ports.insert(443, 8443);

        let yaml = build_run_spec_yaml(
            "nginx:latest",
            "test-container",
            &ports,
            &HashMap::new(),
            &[],
        );
        assert!(yaml.contains("port: 80"));
        assert!(yaml.contains("host_port: 8080"));
    }

    #[test]
    fn test_build_run_spec_yaml_with_env() {
        let mut env = HashMap::new();
        env.insert("FOO".to_string(), "bar".to_string());
        env.insert("BAZ".to_string(), "qux".to_string());

        let yaml =
            build_run_spec_yaml("nginx:latest", "test-container", &HashMap::new(), &env, &[]);
        assert!(yaml.contains("FOO: \"bar\""));
        assert!(yaml.contains("BAZ: \"qux\""));
    }

    #[test]
    fn test_build_run_spec_yaml_with_command() {
        let command = vec![
            "python".to_string(),
            "-c".to_string(),
            "print('hello')".to_string(),
        ];

        let yaml = build_run_spec_yaml(
            "python:3.12",
            "test-container",
            &HashMap::new(),
            &HashMap::new(),
            &command,
        );
        assert!(yaml.contains("command:"));
        assert!(yaml.contains("- \"python\""));
        assert!(yaml.contains("- \"-c\""));
        assert!(yaml.contains("print('hello')"));
    }

    #[test]
    fn test_build_run_spec_yaml_full() {
        let mut ports = HashMap::new();
        ports.insert(5432, 5432);

        let mut env = HashMap::new();
        env.insert("POSTGRES_PASSWORD".to_string(), "secret".to_string());

        let yaml = build_run_spec_yaml("postgres:16", "my-postgres", &ports, &env, &[]);

        assert!(yaml.contains("postgres:16"));
        assert!(yaml.contains("my-postgres:"));
        assert!(yaml.contains("POSTGRES_PASSWORD: \"secret\""));
        assert!(yaml.contains("port: 5432"));
        assert!(yaml.contains("host_port: 5432"));
    }
}
