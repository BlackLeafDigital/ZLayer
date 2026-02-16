//! Runtime class for Python bindings
//!
//! Provides deployment-level container orchestration for Python users.

use crate::container::Container;
use crate::error::{to_py_result, ZLayerError};
use pyo3::prelude::*;
use pyo3::types::PyDict;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use zlayer_agent::{
    ContainerId, MockRuntime, Runtime as AgentRuntime, RuntimeConfig, ServiceManager,
};
use zlayer_spec::DeploymentSpec;

/// Runtime configuration options
#[pyclass]
#[derive(Clone, Default)]
pub struct RuntimeOptions {
    /// Use mock runtime for testing (default: false for production)
    #[pyo3(get, set)]
    pub mock: bool,

    /// Root directory for container state
    #[pyo3(get, set)]
    pub state_dir: Option<String>,

    /// Root directory for container storage
    #[pyo3(get, set)]
    pub storage_dir: Option<String>,
}

#[pymethods]
impl RuntimeOptions {
    #[new]
    #[pyo3(signature = (mock=false, state_dir=None, storage_dir=None))]
    fn new(mock: bool, state_dir: Option<String>, storage_dir: Option<String>) -> Self {
        Self {
            mock,
            state_dir,
            storage_dir,
        }
    }

    fn __repr__(&self) -> String {
        format!(
            "RuntimeOptions(mock={}, state_dir={:?}, storage_dir={:?})",
            self.mock, self.state_dir, self.storage_dir
        )
    }
}

/// Internal state for the Runtime
struct RuntimeInner {
    runtime: Arc<dyn AgentRuntime + Send + Sync>,
    service_manager: ServiceManager,
    deployment_name: Option<String>,
}

/// The ZLayer container runtime
///
/// This class provides high-level orchestration capabilities for managing
/// containers based on ZLayer deployment specifications.
///
/// Example:
///     >>> runtime = Runtime()
///     >>> await runtime.deploy_spec("deployment.yaml")
///     >>> await runtime.scale("my-deployment", "api", 3)
///     >>> print(await runtime.status())
#[pyclass]
pub struct Runtime {
    inner: Arc<RwLock<RuntimeInner>>,
}

#[pymethods]
impl Runtime {
    /// Create a new Runtime instance
    ///
    /// Args:
    ///     options: Optional RuntimeOptions for configuration
    ///
    /// Returns:
    ///     A new Runtime instance
    ///
    /// Raises:
    ///     RuntimeError: If runtime initialization fails
    #[new]
    #[pyo3(signature = (options=None))]
    fn new(options: Option<RuntimeOptions>) -> PyResult<Self> {
        let opts = options.unwrap_or_default();

        // Create the appropriate runtime based on options
        let runtime: Arc<dyn AgentRuntime + Send + Sync> = if opts.mock {
            Arc::new(MockRuntime::new())
        } else {
            // For now, default to mock runtime
            // In production, this would create a YoukiRuntime
            Arc::new(MockRuntime::new())
        };

        let service_manager = ServiceManager::builder(runtime.clone()).build();

        Ok(Self {
            inner: Arc::new(RwLock::new(RuntimeInner {
                runtime,
                service_manager,
                deployment_name: None,
            })),
        })
    }

    /// Create a runtime asynchronously with full initialization
    ///
    /// This is the preferred way to create a Runtime when using the youki
    /// container runtime, as it requires async initialization.
    ///
    /// Args:
    ///     options: Optional RuntimeOptions for configuration
    ///
    /// Returns:
    ///     A fully initialized Runtime instance
    #[staticmethod]
    #[pyo3(signature = (options=None))]
    fn create<'py>(
        py: Python<'py>,
        options: Option<RuntimeOptions>,
    ) -> PyResult<Bound<'py, PyAny>> {
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let opts = options.unwrap_or_default();

            let runtime: Arc<dyn AgentRuntime + Send + Sync> = if opts.mock {
                Arc::new(MockRuntime::new())
            } else {
                #[cfg(target_os = "linux")]
                {
                    // Create youki runtime with proper initialization
                    let config = if let Some(state) = opts.state_dir.as_ref() {
                        zlayer_agent::YoukiConfig {
                            state_dir: PathBuf::from(state),
                            ..Default::default()
                        }
                    } else {
                        zlayer_agent::YoukiConfig::default()
                    };

                    zlayer_agent::create_runtime(RuntimeConfig::Youki(config))
                        .await
                        .map_err(ZLayerError::from)?
                }
                #[cfg(not(target_os = "linux"))]
                {
                    zlayer_agent::create_runtime(RuntimeConfig::Auto)
                        .await
                        .map_err(ZLayerError::from)?
                }
            };

            let service_manager = ServiceManager::builder(runtime.clone()).build();

            to_py_result(Ok(Runtime {
                inner: Arc::new(RwLock::new(RuntimeInner {
                    runtime,
                    service_manager,
                    deployment_name: None,
                })),
            }))
        })
    }

    /// Deploy services from a YAML specification file
    ///
    /// This method reads a ZLayer deployment spec and starts all services
    /// defined in it, respecting their dependencies.
    ///
    /// Args:
    ///     path: Path to the YAML specification file
    ///
    /// Raises:
    ///     ValueError: If the spec file is invalid
    ///     RuntimeError: If deployment fails
    fn deploy_spec<'py>(&self, py: Python<'py>, path: &str) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        let path = PathBuf::from(path);

        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            // Parse the spec file
            let spec = zlayer_spec::from_yaml_file(&path).map_err(ZLayerError::from)?;

            // Deploy with dependencies
            let mut inner = inner.write().await;
            inner.deployment_name = Some(spec.deployment.clone());
            #[allow(deprecated)]
            inner
                .service_manager
                .set_deployment_name(spec.deployment.clone());
            inner
                .service_manager
                .deploy_with_dependencies(spec.services)
                .await
                .map_err(ZLayerError::from)?;

            to_py_result(Ok(()))
        })
    }

    /// Deploy services from a YAML string
    ///
    /// This method parses a ZLayer deployment spec from a string and starts
    /// all services defined in it, respecting their dependencies.
    ///
    /// Args:
    ///     yaml: The YAML specification as a string
    ///
    /// Raises:
    ///     ValueError: If the spec is invalid
    ///     RuntimeError: If deployment fails
    fn deploy_yaml<'py>(&self, py: Python<'py>, yaml: &str) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        let yaml = yaml.to_string();

        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            // Parse the spec
            let spec = zlayer_spec::from_yaml_str(&yaml).map_err(ZLayerError::from)?;

            // Deploy with dependencies
            let mut inner = inner.write().await;
            inner.deployment_name = Some(spec.deployment.clone());
            #[allow(deprecated)]
            inner
                .service_manager
                .set_deployment_name(spec.deployment.clone());
            inner
                .service_manager
                .deploy_with_dependencies(spec.services)
                .await
                .map_err(ZLayerError::from)?;

            to_py_result(Ok(()))
        })
    }

    /// Scale a service to a specific number of replicas
    ///
    /// Args:
    ///     deployment: The deployment name (currently unused, for future multi-deployment support)
    ///     service: The service name to scale
    ///     replicas: The desired number of replicas
    ///
    /// Raises:
    ///     ValueError: If the service doesn't exist
    ///     RuntimeError: If scaling fails
    fn scale<'py>(
        &self,
        py: Python<'py>,
        deployment: &str,
        service: &str,
        replicas: u32,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        let _deployment = deployment.to_string();
        let service = service.to_string();

        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let inner = inner.read().await;
            inner
                .service_manager
                .scale_service(&service, replicas)
                .await
                .map_err(ZLayerError::from)?;

            to_py_result(Ok(()))
        })
    }

    /// Get the status of services in a deployment
    ///
    /// Args:
    ///     deployment: Optional deployment name to filter by
    ///
    /// Returns:
    ///     A dict mapping service names to their status information
    #[pyo3(signature = (deployment=None))]
    fn status<'py>(
        &self,
        py: Python<'py>,
        deployment: Option<&str>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        let _deployment = deployment.map(|s| s.to_string());

        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let inner = inner.read().await;
            let services = inner.service_manager.list_services().await;

            let mut status_map: HashMap<String, HashMap<String, String>> = HashMap::new();

            for service_name in services {
                let replica_count = inner
                    .service_manager
                    .service_replica_count(&service_name)
                    .await
                    .unwrap_or(0);

                let mut service_status = HashMap::new();
                service_status.insert("replicas".to_string(), replica_count.to_string());
                service_status.insert("status".to_string(), "running".to_string());

                status_map.insert(service_name, service_status);
            }

            to_py_result(Ok(status_map))
        })
    }

    /// List all services in the current deployment
    ///
    /// Returns:
    ///     A list of service names
    fn list_services<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();

        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let inner = inner.read().await;
            let services = inner.service_manager.list_services().await;
            to_py_result(Ok(services))
        })
    }

    /// Get or create a container for a service
    ///
    /// Args:
    ///     service: The service name
    ///     replica: The replica number (default: 1)
    ///
    /// Returns:
    ///     A Container instance for the specified replica
    #[pyo3(signature = (service, replica=1))]
    fn get_container<'py>(
        &self,
        py: Python<'py>,
        service: &str,
        replica: u32,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        let service = service.to_string();

        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let inner = inner.read().await;

            let container_id = ContainerId {
                service: service.clone(),
                replica,
            };

            // Verify the container exists by checking its state
            inner
                .runtime
                .container_state(&container_id)
                .await
                .map_err(ZLayerError::from)?;

            // Create a minimal spec for the container wrapper
            // In a real implementation, we'd fetch this from the service manager
            let yaml = format!(
                r#"version: v1
deployment: runtime
services:
  {}:
    rtype: service
    image:
      name: placeholder:latest
"#,
                service
            );
            let spec: DeploymentSpec = serde_yaml::from_str(&yaml).map_err(ZLayerError::from)?;
            let service_spec =
                spec.services.get(&service).cloned().ok_or_else(|| {
                    ZLayerError::NotFound(format!("Service {} not found", service))
                })?;

            to_py_result(Ok(Container::new(
                container_id,
                service_spec,
                inner.runtime.clone(),
            )))
        })
    }

    /// Remove a service and all its containers
    ///
    /// Args:
    ///     service: The service name to remove
    ///
    /// Raises:
    ///     ValueError: If the service doesn't exist
    ///     RuntimeError: If removal fails
    fn remove_service<'py>(&self, py: Python<'py>, service: &str) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        let service = service.to_string();

        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let inner = inner.read().await;
            inner
                .service_manager
                .remove_service(&service)
                .await
                .map_err(ZLayerError::from)?;

            to_py_result(Ok(()))
        })
    }

    /// Shutdown the runtime and all services
    ///
    /// This method gracefully stops all services and cleans up resources.
    fn shutdown<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();

        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let inner = inner.read().await;

            // Get all services and remove them
            let services = inner.service_manager.list_services().await;
            for service in services {
                let _ = inner.service_manager.remove_service(&service).await;
            }

            to_py_result(Ok(()))
        })
    }

    /// The current deployment name
    #[getter]
    fn deployment_name<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();

        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let inner = inner.read().await;
            to_py_result(Ok(inner.deployment_name.clone()))
        })
    }

    fn __repr__(&self) -> String {
        "Runtime()".to_string()
    }
}

/// Parse a deployment spec from YAML
///
/// This is a standalone function for parsing specs without a Runtime.
///
/// Args:
///     yaml: The YAML string to parse
///
/// Returns:
///     A dict representation of the parsed spec
///
/// Raises:
///     ValueError: If the YAML is invalid
#[pyfunction]
pub fn parse_spec<'py>(py: Python<'py>, yaml: &str) -> PyResult<Bound<'py, PyDict>> {
    let spec = zlayer_spec::from_yaml_str(yaml).map_err(ZLayerError::from)?;

    let dict = PyDict::new(py);
    dict.set_item("version", &spec.version)?;
    dict.set_item("deployment", &spec.deployment)?;

    let services_dict = PyDict::new(py);
    for name in spec.services.keys() {
        services_dict.set_item(name, name)?;
    }
    dict.set_item("services", services_dict)?;

    Ok(dict)
}

/// Validate a deployment spec from YAML
///
/// This function validates a spec without deploying it.
///
/// Args:
///     yaml: The YAML string to validate
///
/// Returns:
///     True if valid
///
/// Raises:
///     ValueError: If the spec is invalid
#[pyfunction]
pub fn validate_spec(yaml: &str) -> PyResult<bool> {
    zlayer_spec::from_yaml_str(yaml).map_err(ZLayerError::from)?;
    Ok(true)
}
