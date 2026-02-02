//! Spec classes for Python bindings
//!
//! Provides Python-friendly wrappers for ZLayer spec types.

use crate::error::ZLayerError;
use pyo3::prelude::*;
use pyo3::types::PyDict;
use std::collections::HashMap;
use std::time::Duration;

/// A deployment specification
///
/// This class represents a parsed ZLayer deployment specification,
/// providing access to its services and configuration.
#[pyclass]
#[derive(Clone)]
pub struct Spec {
    inner: zlayer_spec::DeploymentSpec,
}

#[pymethods]
impl Spec {
    /// Parse a spec from a YAML string
    ///
    /// Args:
    ///     yaml: The YAML string to parse
    ///
    /// Returns:
    ///     A Spec instance
    ///
    /// Raises:
    ///     ValueError: If the YAML is invalid
    #[staticmethod]
    fn from_yaml(yaml: &str) -> PyResult<Self> {
        let spec = zlayer_spec::from_yaml_str(yaml).map_err(ZLayerError::from)?;
        Ok(Self { inner: spec })
    }

    /// Parse a spec from a YAML file
    ///
    /// Args:
    ///     path: Path to the YAML file
    ///
    /// Returns:
    ///     A Spec instance
    ///
    /// Raises:
    ///     ValueError: If the file is invalid
    ///     IOError: If the file cannot be read
    #[staticmethod]
    fn from_file(path: &str) -> PyResult<Self> {
        let path = std::path::Path::new(path);
        let spec = zlayer_spec::from_yaml_file(path).map_err(ZLayerError::from)?;
        Ok(Self { inner: spec })
    }

    /// The spec version
    #[getter]
    fn version(&self) -> &str {
        &self.inner.version
    }

    /// The deployment name
    #[getter]
    fn deployment(&self) -> &str {
        &self.inner.deployment
    }

    /// Service names in this spec
    #[getter]
    fn services(&self) -> Vec<String> {
        self.inner.services.keys().cloned().collect()
    }

    /// Get a service spec by name
    ///
    /// Args:
    ///     name: The service name
    ///
    /// Returns:
    ///     A ServiceSpec instance, or None if not found
    fn get_service(&self, name: &str) -> Option<ServiceSpec> {
        self.inner
            .services
            .get(name)
            .cloned()
            .map(|spec| ServiceSpec {
                name: name.to_string(),
                inner: spec,
            })
    }

    /// Convert to a dict representation
    fn to_dict<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let dict = PyDict::new(py);
        dict.set_item("version", &self.inner.version)?;
        dict.set_item("deployment", &self.inner.deployment)?;

        let services_dict = PyDict::new(py);
        for name in self.inner.services.keys() {
            services_dict.set_item(name, name)?; // Simplified - just show names
        }
        dict.set_item("services", services_dict)?;

        Ok(dict)
    }

    /// Convert to YAML string
    fn to_yaml(&self) -> PyResult<String> {
        serde_yaml::to_string(&self.inner).map_err(|e| ZLayerError::Spec(e.to_string()).into())
    }

    fn __repr__(&self) -> String {
        format!(
            "Spec(deployment='{}', services={:?})",
            self.inner.deployment,
            self.services()
        )
    }
}

/// A service specification
#[pyclass]
#[derive(Clone)]
pub struct ServiceSpec {
    name: String,
    inner: zlayer_spec::ServiceSpec,
}

#[pymethods]
impl ServiceSpec {
    /// The service name
    #[getter]
    fn name(&self) -> &str {
        &self.name
    }

    /// The resource type (service, job, or cron)
    #[getter]
    fn rtype(&self) -> &str {
        match self.inner.rtype {
            zlayer_spec::ResourceType::Service => "service",
            zlayer_spec::ResourceType::Job => "job",
            zlayer_spec::ResourceType::Cron => "cron",
        }
    }

    /// The container image name
    #[getter]
    fn image(&self) -> &str {
        &self.inner.image.name
    }

    /// Environment variables
    #[getter]
    fn env(&self) -> HashMap<String, String> {
        self.inner.env.clone()
    }

    /// Endpoint definitions
    #[getter]
    fn endpoints(&self) -> Vec<EndpointSpec> {
        self.inner
            .endpoints
            .iter()
            .map(|ep| EndpointSpec { inner: ep.clone() })
            .collect()
    }

    /// The scale configuration
    #[getter]
    fn scale(&self) -> ScaleSpec {
        ScaleSpec {
            inner: self.inner.scale.clone(),
        }
    }

    /// Dependencies
    #[getter]
    fn depends(&self) -> Vec<DependsSpec> {
        self.inner
            .depends
            .iter()
            .map(|d| DependsSpec { inner: d.clone() })
            .collect()
    }

    /// Resource limits
    #[getter]
    fn resources(&self) -> ResourcesSpec {
        ResourcesSpec {
            inner: self.inner.resources.clone(),
        }
    }

    /// Health check configuration
    #[getter]
    fn health(&self) -> HealthSpec {
        HealthSpec {
            inner: self.inner.health.clone(),
        }
    }

    /// Cron schedule (for cron jobs)
    #[getter]
    fn schedule(&self) -> Option<String> {
        self.inner.schedule.clone()
    }

    /// Convert to a dict representation
    fn to_dict<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let dict = PyDict::new(py);
        dict.set_item("name", &self.name)?;
        dict.set_item("rtype", self.rtype())?;
        dict.set_item("image", self.image())?;
        dict.set_item("env", self.env())?;
        if let Some(schedule) = &self.inner.schedule {
            dict.set_item("schedule", schedule)?;
        }
        Ok(dict)
    }

    fn __repr__(&self) -> String {
        format!(
            "ServiceSpec(name='{}', image='{}', rtype='{}')",
            self.name,
            self.image(),
            self.rtype()
        )
    }
}

/// An endpoint specification
#[pyclass]
#[derive(Clone)]
pub struct EndpointSpec {
    inner: zlayer_spec::EndpointSpec,
}

#[pymethods]
impl EndpointSpec {
    /// The endpoint name
    #[getter]
    fn name(&self) -> &str {
        &self.inner.name
    }

    /// The protocol (http, https, tcp, udp, websocket)
    #[getter]
    fn protocol(&self) -> &str {
        match self.inner.protocol {
            zlayer_spec::Protocol::Http => "http",
            zlayer_spec::Protocol::Https => "https",
            zlayer_spec::Protocol::Tcp => "tcp",
            zlayer_spec::Protocol::Udp => "udp",
            zlayer_spec::Protocol::Websocket => "websocket",
        }
    }

    /// The port number
    #[getter]
    fn port(&self) -> u16 {
        self.inner.port
    }

    /// The URL path prefix (for HTTP)
    #[getter]
    fn path(&self) -> Option<String> {
        self.inner.path.clone()
    }

    /// The exposure type (public or internal)
    #[getter]
    fn expose(&self) -> &str {
        match self.inner.expose {
            zlayer_spec::ExposeType::Public => "public",
            zlayer_spec::ExposeType::Internal => "internal",
        }
    }

    fn __repr__(&self) -> String {
        format!(
            "EndpointSpec(name='{}', protocol='{}', port={})",
            self.name(),
            self.protocol(),
            self.port()
        )
    }
}

/// A scale specification
#[pyclass]
#[derive(Clone)]
pub struct ScaleSpec {
    inner: zlayer_spec::ScaleSpec,
}

#[pymethods]
impl ScaleSpec {
    /// The scaling mode (adaptive, fixed, or manual)
    #[getter]
    fn mode(&self) -> &str {
        match &self.inner {
            zlayer_spec::ScaleSpec::Adaptive { .. } => "adaptive",
            zlayer_spec::ScaleSpec::Fixed { .. } => "fixed",
            zlayer_spec::ScaleSpec::Manual => "manual",
        }
    }

    /// Minimum replicas (for adaptive mode)
    #[getter]
    fn min(&self) -> Option<u32> {
        match &self.inner {
            zlayer_spec::ScaleSpec::Adaptive { min, .. } => Some(*min),
            _ => None,
        }
    }

    /// Maximum replicas (for adaptive mode)
    #[getter]
    fn max(&self) -> Option<u32> {
        match &self.inner {
            zlayer_spec::ScaleSpec::Adaptive { max, .. } => Some(*max),
            _ => None,
        }
    }

    /// Fixed replica count (for fixed mode)
    #[getter]
    fn replicas(&self) -> Option<u32> {
        match &self.inner {
            zlayer_spec::ScaleSpec::Fixed { replicas } => Some(*replicas),
            _ => None,
        }
    }

    /// Cooldown period in seconds (for adaptive mode)
    #[getter]
    fn cooldown_secs(&self) -> Option<f64> {
        match &self.inner {
            zlayer_spec::ScaleSpec::Adaptive { cooldown, .. } => cooldown.map(|d| d.as_secs_f64()),
            _ => None,
        }
    }

    fn __repr__(&self) -> String {
        match &self.inner {
            zlayer_spec::ScaleSpec::Adaptive { min, max, .. } => {
                format!("ScaleSpec(mode='adaptive', min={}, max={})", min, max)
            }
            zlayer_spec::ScaleSpec::Fixed { replicas } => {
                format!("ScaleSpec(mode='fixed', replicas={})", replicas)
            }
            zlayer_spec::ScaleSpec::Manual => "ScaleSpec(mode='manual')".to_string(),
        }
    }
}

/// A dependency specification
#[pyclass]
#[derive(Clone)]
pub struct DependsSpec {
    inner: zlayer_spec::DependsSpec,
}

#[pymethods]
impl DependsSpec {
    /// The service this depends on
    #[getter]
    fn service(&self) -> &str {
        &self.inner.service
    }

    /// The dependency condition (started, healthy, or ready)
    #[getter]
    fn condition(&self) -> &str {
        match self.inner.condition {
            zlayer_spec::DependencyCondition::Started => "started",
            zlayer_spec::DependencyCondition::Healthy => "healthy",
            zlayer_spec::DependencyCondition::Ready => "ready",
        }
    }

    /// Timeout in seconds
    #[getter]
    fn timeout_secs(&self) -> Option<f64> {
        self.inner.timeout.map(|d| d.as_secs_f64())
    }

    /// Action on timeout (fail, warn, or continue)
    #[getter]
    fn on_timeout(&self) -> &str {
        match self.inner.on_timeout {
            zlayer_spec::TimeoutAction::Fail => "fail",
            zlayer_spec::TimeoutAction::Warn => "warn",
            zlayer_spec::TimeoutAction::Continue => "continue",
        }
    }

    fn __repr__(&self) -> String {
        format!(
            "DependsSpec(service='{}', condition='{}')",
            self.service(),
            self.condition()
        )
    }
}

/// Resource limits specification
#[pyclass]
#[derive(Clone)]
pub struct ResourcesSpec {
    inner: zlayer_spec::ResourcesSpec,
}

#[pymethods]
impl ResourcesSpec {
    /// CPU limit in cores
    #[getter]
    fn cpu(&self) -> Option<f64> {
        self.inner.cpu
    }

    /// Memory limit (e.g., "512Mi")
    #[getter]
    fn memory(&self) -> Option<String> {
        self.inner.memory.clone()
    }

    fn __repr__(&self) -> String {
        format!(
            "ResourcesSpec(cpu={:?}, memory={:?})",
            self.cpu(),
            self.memory()
        )
    }
}

/// Health check specification
#[pyclass]
#[derive(Clone)]
pub struct HealthSpec {
    inner: zlayer_spec::HealthSpec,
}

#[pymethods]
impl HealthSpec {
    /// Grace period before first check (seconds)
    #[getter]
    fn start_grace_secs(&self) -> Option<f64> {
        self.inner.start_grace.map(|d| d.as_secs_f64())
    }

    /// Interval between checks (seconds)
    #[getter]
    fn interval_secs(&self) -> Option<f64> {
        self.inner.interval.map(|d| d.as_secs_f64())
    }

    /// Timeout per check (seconds)
    #[getter]
    fn timeout_secs(&self) -> Option<f64> {
        self.inner.timeout.map(|d| d.as_secs_f64())
    }

    /// Number of retries before marking unhealthy
    #[getter]
    fn retries(&self) -> u32 {
        self.inner.retries
    }

    /// The health check type (tcp, http, or command)
    #[getter]
    fn check_type(&self) -> &str {
        match &self.inner.check {
            zlayer_spec::HealthCheck::Tcp { .. } => "tcp",
            zlayer_spec::HealthCheck::Http { .. } => "http",
            zlayer_spec::HealthCheck::Command { .. } => "command",
        }
    }

    /// The health check configuration as a dict
    fn check<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let dict = PyDict::new(py);
        match &self.inner.check {
            zlayer_spec::HealthCheck::Tcp { port } => {
                dict.set_item("type", "tcp")?;
                dict.set_item("port", *port)?;
            }
            zlayer_spec::HealthCheck::Http { url, expect_status } => {
                dict.set_item("type", "http")?;
                dict.set_item("url", url)?;
                dict.set_item("expect_status", *expect_status)?;
            }
            zlayer_spec::HealthCheck::Command { command } => {
                dict.set_item("type", "command")?;
                dict.set_item("command", command)?;
            }
        }
        Ok(dict)
    }

    fn __repr__(&self) -> String {
        format!(
            "HealthSpec(type='{}', retries={})",
            self.check_type(),
            self.retries()
        )
    }
}

/// Create a minimal service spec programmatically
///
/// Args:
///     name: The service name
///     image: The container image
///     port: Optional port to expose
///
/// Returns:
///     A ServiceSpec instance
#[pyfunction]
#[pyo3(signature = (name, image, port=None))]
pub fn create_service_spec(name: &str, image: &str, port: Option<u16>) -> PyResult<ServiceSpec> {
    let mut endpoints = Vec::new();
    if let Some(p) = port {
        endpoints.push(zlayer_spec::EndpointSpec {
            name: "http".to_string(),
            protocol: zlayer_spec::Protocol::Http,
            port: p,
            path: None,
            expose: zlayer_spec::ExposeType::Internal,
            stream: None,
        });
    }

    let spec = zlayer_spec::ServiceSpec {
        rtype: zlayer_spec::ResourceType::Service,
        schedule: None,
        image: zlayer_spec::ImageSpec {
            name: image.to_string(),
            pull_policy: zlayer_spec::PullPolicy::IfNotPresent,
        },
        resources: zlayer_spec::ResourcesSpec::default(),
        env: HashMap::new(),
        command: zlayer_spec::CommandSpec::default(),
        network: zlayer_spec::NetworkSpec::default(),
        endpoints,
        scale: zlayer_spec::ScaleSpec::default(),
        depends: Vec::new(),
        health: zlayer_spec::HealthSpec {
            start_grace: None,
            interval: Some(Duration::from_secs(10)),
            timeout: None,
            retries: 3,
            check: zlayer_spec::HealthCheck::Tcp {
                port: port.unwrap_or(0),
            },
        },
        init: zlayer_spec::InitSpec::default(),
        errors: zlayer_spec::ErrorsSpec::default(),
        devices: Vec::new(),
        storage: Vec::new(),
        capabilities: Vec::new(),
        privileged: false,
        node_mode: zlayer_spec::NodeMode::default(),
        node_selector: None,
        service_type: zlayer_spec::ServiceType::default(),
        wasm_http: None,
    };

    Ok(ServiceSpec {
        name: name.to_string(),
        inner: spec,
    })
}
