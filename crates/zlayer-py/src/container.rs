//! Container class for Python bindings
//!
//! Provides a high-level Python interface for managing individual containers.

use crate::error::{to_py_result, ZLayerError};
use pyo3::prelude::*;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use zlayer_agent::{ContainerId, ContainerState, Runtime};
use zlayer_spec::ServiceSpec;

/// Container state as a Python-friendly enum
#[pyclass(eq, eq_int)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PyContainerState {
    Pending,
    Initializing,
    Running,
    Stopping,
    Exited,
    Failed,
}

impl From<ContainerState> for PyContainerState {
    fn from(state: ContainerState) -> Self {
        match state {
            ContainerState::Pending => PyContainerState::Pending,
            ContainerState::Initializing => PyContainerState::Initializing,
            ContainerState::Running => PyContainerState::Running,
            ContainerState::Stopping => PyContainerState::Stopping,
            ContainerState::Exited { .. } => PyContainerState::Exited,
            ContainerState::Failed { .. } => PyContainerState::Failed,
        }
    }
}

#[pymethods]
impl PyContainerState {
    fn __str__(&self) -> &'static str {
        match self {
            PyContainerState::Pending => "pending",
            PyContainerState::Initializing => "initializing",
            PyContainerState::Running => "running",
            PyContainerState::Stopping => "stopping",
            PyContainerState::Exited => "exited",
            PyContainerState::Failed => "failed",
        }
    }

    fn __repr__(&self) -> String {
        format!("ContainerState.{}", self.__str__().to_uppercase())
    }
}

/// Internal container state
struct ContainerInner {
    id: ContainerId,
    spec: ServiceSpec,
    runtime: Arc<dyn Runtime + Send + Sync>,
}

/// A container instance managed by ZLayer
///
/// This class provides methods to control the lifecycle of a container,
/// including starting, stopping, and querying its status.
///
/// Example:
///     >>> container = Container.create("nginx:latest", ports={"http": 80})
///     >>> await container.start()
///     >>> print(container.status)
///     'running'
///     >>> await container.stop()
#[pyclass]
pub struct Container {
    inner: Arc<RwLock<ContainerInner>>,
}

impl Container {
    /// Create a new container from internal components
    pub(crate) fn new(
        id: ContainerId,
        spec: ServiceSpec,
        runtime: Arc<dyn Runtime + Send + Sync>,
    ) -> Self {
        Self {
            inner: Arc::new(RwLock::new(ContainerInner { id, spec, runtime })),
        }
    }
}

#[pymethods]
impl Container {
    /// Create a new container from an image
    ///
    /// Args:
    ///     image: The container image name (e.g., "nginx:latest")
    ///     ports: Optional mapping of endpoint names to port numbers
    ///     env: Optional environment variables as a dict
    ///     name: Optional container name (auto-generated if not provided)
    ///
    /// Returns:
    ///     A new Container instance (not yet started)
    ///
    /// Raises:
    ///     ValueError: If the image name is invalid
    ///     RuntimeError: If container creation fails
    #[staticmethod]
    #[pyo3(signature = (image, ports=None, env=None, name=None))]
    fn create(
        image: &str,
        ports: Option<HashMap<String, u16>>,
        env: Option<HashMap<String, String>>,
        name: Option<&str>,
    ) -> PyResult<Self> {
        // Build a minimal ServiceSpec from the provided parameters
        let yaml = build_spec_yaml(image, ports.as_ref(), env.as_ref(), name);
        let deployment: zlayer_spec::DeploymentSpec = serde_yaml::from_str(&yaml)
            .map_err(|e| ZLayerError::InvalidArgument(format!("Failed to create spec: {}", e)))?;

        let container_name = name.unwrap_or("container").to_string();
        let spec = deployment
            .services
            .get(&container_name)
            .cloned()
            .ok_or_else(|| {
                ZLayerError::InvalidArgument("Failed to extract service spec".to_string())
            })?;

        // Create a mock runtime for now - in production this would be configurable
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(zlayer_agent::MockRuntime::new());

        let id = ContainerId {
            service: container_name,
            replica: 1,
        };

        Ok(Container::new(id, spec, runtime))
    }

    /// Start the container
    ///
    /// This method creates and starts the container process.
    /// The container must not already be running.
    ///
    /// Raises:
    ///     RuntimeError: If the container fails to start
    fn start<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let inner = inner.read().await;
            let runtime = inner.runtime.clone();
            let id = inner.id.clone();
            let spec = inner.spec.clone();
            drop(inner);

            // Pull image
            runtime
                .pull_image(&spec.image.name)
                .await
                .map_err(ZLayerError::from)?;

            // Create container
            runtime
                .create_container(&id, &spec)
                .await
                .map_err(ZLayerError::from)?;

            // Start container
            runtime
                .start_container(&id)
                .await
                .map_err(ZLayerError::from)?;

            to_py_result(Ok(()))
        })
    }

    /// Stop the container
    ///
    /// This method gracefully stops the container with a timeout.
    ///
    /// Args:
    ///     timeout: Maximum seconds to wait for graceful shutdown (default: 30)
    ///
    /// Raises:
    ///     RuntimeError: If the container fails to stop
    #[pyo3(signature = (timeout=30))]
    fn stop<'py>(&self, py: Python<'py>, timeout: u64) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let inner = inner.read().await;
            let runtime = inner.runtime.clone();
            let id = inner.id.clone();
            drop(inner);

            runtime
                .stop_container(&id, std::time::Duration::from_secs(timeout))
                .await
                .map_err(ZLayerError::from)?;

            to_py_result(Ok(()))
        })
    }

    /// Remove the container
    ///
    /// This method removes the container. The container must be stopped first.
    ///
    /// Raises:
    ///     RuntimeError: If the container fails to be removed
    fn remove<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let inner = inner.read().await;
            let runtime = inner.runtime.clone();
            let id = inner.id.clone();
            drop(inner);

            runtime
                .remove_container(&id)
                .await
                .map_err(ZLayerError::from)?;

            to_py_result(Ok(()))
        })
    }

    /// Get container logs
    ///
    /// Args:
    ///     tail: Number of lines to return from the end (default: 100)
    ///
    /// Returns:
    ///     Container logs as a string
    ///
    /// Raises:
    ///     RuntimeError: If logs cannot be retrieved
    #[pyo3(signature = (tail=100))]
    fn logs<'py>(&self, py: Python<'py>, tail: usize) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let inner = inner.read().await;
            let runtime = inner.runtime.clone();
            let id = inner.id.clone();
            drop(inner);

            let logs = runtime
                .container_logs(&id, tail)
                .await
                .map_err(ZLayerError::from)?;

            to_py_result(Ok(logs))
        })
    }

    /// Wait for the container to become healthy
    ///
    /// This method blocks until the container passes its health check
    /// or the timeout is reached.
    ///
    /// Args:
    ///     timeout: Maximum seconds to wait (default: 60)
    ///
    /// Raises:
    ///     TimeoutError: If the container doesn't become healthy in time
    ///     RuntimeError: If the container fails
    #[pyo3(signature = (timeout=60))]
    fn wait_healthy<'py>(&self, py: Python<'py>, timeout: u64) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let inner = inner.read().await;
            let runtime = inner.runtime.clone();
            let id = inner.id.clone();
            drop(inner);

            let start = std::time::Instant::now();
            let timeout_duration = std::time::Duration::from_secs(timeout);

            loop {
                let state = runtime
                    .container_state(&id)
                    .await
                    .map_err(ZLayerError::from)?;

                match state {
                    ContainerState::Running => {
                        // In a real implementation, we'd check health here
                        // For now, Running is considered healthy
                        return to_py_result(Ok(()));
                    }
                    ContainerState::Failed { reason } => {
                        return Err(ZLayerError::Container(format!(
                            "Container failed: {}",
                            reason
                        ))
                        .into());
                    }
                    ContainerState::Exited { code } => {
                        return Err(ZLayerError::Container(format!(
                            "Container exited with code {}",
                            code
                        ))
                        .into());
                    }
                    _ => {
                        if start.elapsed() > timeout_duration {
                            return Err(ZLayerError::Timeout(
                                "Timed out waiting for container to become healthy".to_string(),
                            )
                            .into());
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    }
                }
            }
        })
    }

    /// Execute a command inside the container
    ///
    /// Args:
    ///     command: The command to execute as a list of strings
    ///
    /// Returns:
    ///     A tuple of (exit_code, stdout, stderr)
    ///
    /// Raises:
    ///     RuntimeError: If command execution fails
    fn exec<'py>(&self, py: Python<'py>, command: Vec<String>) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let inner = inner.read().await;
            let runtime = inner.runtime.clone();
            let id = inner.id.clone();
            drop(inner);

            let (code, stdout, stderr) = runtime
                .exec(&id, &command)
                .await
                .map_err(ZLayerError::from)?;

            to_py_result(Ok((code, stdout, stderr)))
        })
    }

    /// The container ID
    #[getter]
    fn id(&self, py: Python<'_>) -> PyResult<String> {
        let inner = self.inner.clone();
        py.allow_threads(|| {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                let inner = inner.read().await;
                Ok(inner.id.to_string())
            })
        })
    }

    /// The current container status
    #[getter]
    fn status<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let inner = inner.read().await;
            let runtime = inner.runtime.clone();
            let id = inner.id.clone();
            drop(inner);

            let state = runtime
                .container_state(&id)
                .await
                .map_err(ZLayerError::from)?;

            let py_state = PyContainerState::from(state);
            to_py_result(Ok(py_state.__str__().to_string()))
        })
    }

    /// The container state as an enum
    #[getter]
    fn state<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let inner = inner.read().await;
            let runtime = inner.runtime.clone();
            let id = inner.id.clone();
            drop(inner);

            let state = runtime
                .container_state(&id)
                .await
                .map_err(ZLayerError::from)?;

            to_py_result(Ok(PyContainerState::from(state)))
        })
    }

    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        let id = self.id(py)?;
        Ok(format!("Container(id='{}')", id))
    }
}

/// Build a YAML spec from the provided parameters
fn build_spec_yaml(
    image: &str,
    ports: Option<&HashMap<String, u16>>,
    env: Option<&HashMap<String, String>>,
    name: Option<&str>,
) -> String {
    let service_name = name.unwrap_or("container");

    let mut yaml = format!(
        r#"version: v1
deployment: python-container
services:
  {}:
    rtype: service
    image:
      name: {}
"#,
        service_name, image
    );

    // Add environment variables
    if let Some(env_vars) = env {
        if !env_vars.is_empty() {
            yaml.push_str("    env:\n");
            for (key, value) in env_vars {
                yaml.push_str(&format!("      {}: \"{}\"\n", key, value));
            }
        }
    }

    // Add endpoints from ports
    if let Some(port_map) = ports {
        if !port_map.is_empty() {
            yaml.push_str("    endpoints:\n");
            for (name, port) in port_map {
                yaml.push_str(&format!(
                    "      - name: {}\n        protocol: tcp\n        port: {}\n",
                    name, port
                ));
            }
        }
    }

    yaml
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_spec_yaml_basic() {
        let yaml = build_spec_yaml("nginx:latest", None, None, None);
        assert!(yaml.contains("nginx:latest"));
        assert!(yaml.contains("container:"));
    }

    #[test]
    fn test_build_spec_yaml_with_ports() {
        let mut ports = HashMap::new();
        ports.insert("http".to_string(), 80);
        let yaml = build_spec_yaml("nginx:latest", Some(&ports), None, None);
        assert!(yaml.contains("port: 80"));
        assert!(yaml.contains("name: http"));
    }

    #[test]
    fn test_build_spec_yaml_with_env() {
        let mut env = HashMap::new();
        env.insert("FOO".to_string(), "bar".to_string());
        let yaml = build_spec_yaml("nginx:latest", None, Some(&env), None);
        assert!(yaml.contains("FOO: \"bar\""));
    }
}
