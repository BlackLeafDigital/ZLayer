//! Remote daemon client for Python bindings
//!
//! Exposes [`Client`], a thin pyo3 wrapper around
//! [`zlayer_client::DaemonClient`].  This lets Python users drive a running
//! `zlayer serve` daemon over its Unix-socket REST API without spawning an
//! in-process container runtime.
//!
//! This module is compiled only on Unix targets (the parent module gates
//! it with `#[cfg(unix)]`); the daemon socket is Unix only.  On Windows
//! the class is simply not registered, so `zlayer.Client(...)` raises
//! `AttributeError`, matching the platform gate on `Runtime` for non-Linux
//! targets.

use std::path::PathBuf;
use std::sync::Arc;

use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};

use crate::error::ZLayerError;

/// Convert a [`serde_json::Value`] into a native Python object.
///
/// Maps: Null → None, Bool → bool, Number → int/float, String → str,
/// Array → list, Object → dict.  Used to hand back daemon responses as
/// ordinary Python data structures instead of an opaque Rust handle.
fn json_to_py(py: Python<'_>, value: &serde_json::Value) -> PyResult<Py<PyAny>> {
    match value {
        serde_json::Value::Null => Ok(py.None()),
        serde_json::Value::Bool(b) => Ok(b.into_pyobject(py)?.to_owned().unbind().into_any()),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(i.into_pyobject(py)?.unbind().into_any())
            } else if let Some(u) = n.as_u64() {
                Ok(u.into_pyobject(py)?.unbind().into_any())
            } else if let Some(f) = n.as_f64() {
                Ok(f.into_pyobject(py)?.unbind().into_any())
            } else {
                // Should be unreachable, but fall back to string form.
                Ok(n.to_string().into_pyobject(py)?.unbind().into_any())
            }
        }
        serde_json::Value::String(s) => Ok(s.into_pyobject(py)?.unbind().into_any()),
        serde_json::Value::Array(arr) => {
            let list = PyList::empty(py);
            for item in arr {
                list.append(json_to_py(py, item)?)?;
            }
            Ok(list.unbind().into_any())
        }
        serde_json::Value::Object(map) => {
            let dict = PyDict::new(py);
            for (k, v) in map {
                dict.set_item(k, json_to_py(py, v)?)?;
            }
            Ok(dict.unbind().into_any())
        }
    }
}

/// Map any `Display`-able error (notably `anyhow::Error` from
/// `DaemonClient`) into the crate's [`ZLayerError::Runtime`] variant so
/// Python callers see a uniform exception type.
fn map_client_err<E: std::fmt::Display>(err: E) -> ZLayerError {
    ZLayerError::Runtime(format!("daemon client: {err}"))
}

/// Remote `ZLayer` daemon client.
///
/// Wraps [`zlayer_client::DaemonClient`], which speaks the daemon's REST API
/// over a Unix domain socket.  Prefer this over [`crate::Runtime`] when you
/// just want to drive an already-running `zlayer serve` from Python
/// (deploy/ps/logs/exec/stop/status/scale), or when you're on a non-Linux
/// dev host and the daemon lives on a Linux box you can reach via SSH-tunnel
/// forwarding the socket.
///
/// Construction is synchronous (it does not dial the daemon); every method
/// is `async`.
///
/// Example:
///
/// ```python
/// import asyncio
/// from zlayer import Client
///
/// async def main():
///     c = Client()                          # default socket path
///     print(await c.ps())
///     await c.deploy(open("app.yaml").read())
///
/// asyncio.run(main())
/// ```
#[pyclass(name = "Client")]
pub struct Client {
    inner: Arc<zlayer_client::DaemonClient>,
}

#[pymethods]
impl Client {
    /// Construct a client bound to a daemon socket.
    ///
    /// The `host` argument is reserved for future TCP/HTTPS transports and
    /// is currently ignored — the daemon only listens on a Unix socket.
    /// Passing a non-None `host` raises `ValueError` so callers notice
    /// instead of silently talking to the wrong endpoint.
    ///
    /// Args:
    ///     socket: Optional path to the daemon Unix socket. When None, uses
    ///         `zlayer_client::default_socket_path()`.
    ///     host: Reserved; must be None for now.
    ///
    /// Raises:
    ///     `ValueError`: If `host` is provided (not yet supported).
    ///     `RuntimeError`: If the daemon is unreachable (health check fails).
    #[new]
    #[pyo3(signature = (socket=None, host=None))]
    #[allow(clippy::needless_pass_by_value)]
    fn new(socket: Option<PathBuf>, host: Option<String>) -> PyResult<Self> {
        if host.is_some() {
            return Err(ZLayerError::InvalidArgument(
                "Client(host=...) is not supported yet; the daemon only listens on a Unix \
                 socket. Pass socket=<path> instead, or forward the remote socket locally."
                    .to_string(),
            )
            .into());
        }

        let socket_path =
            socket.unwrap_or_else(|| PathBuf::from(zlayer_client::default_socket_path()));

        // Block here to perform the initial health check.  `DaemonClient::connect_to`
        // will auto-start a local daemon if one isn't already running; that's the
        // same behavior the `zlayer` CLI has, so Python users get parity.
        let dc = pyo3_async_runtimes::tokio::get_runtime()
            .block_on(zlayer_client::DaemonClient::connect_to(&socket_path))
            .map_err(map_client_err)?;

        Ok(Self {
            inner: Arc::new(dc),
        })
    }

    #[allow(clippy::unused_self)]
    fn __repr__(&self) -> String {
        format!(
            "Client(socket={:?})",
            self.inner.socket_path().to_string_lossy()
        )
    }

    /// List all containers the daemon knows about.
    ///
    /// Wraps `DaemonClient::get_all_containers`
    /// (`GET /api/v1/containers`).
    ///
    /// Returns:
    ///     A Python object (dict/list) mirroring the JSON response.
    fn ps<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let value = inner.get_all_containers().await.map_err(map_client_err)?;
            Python::attach(|py| json_to_py(py, &value))
        })
    }

    /// Create a new deployment from a YAML spec string.
    ///
    /// Wraps `DaemonClient::create_deployment`
    /// (`POST /api/v1/deployments` with `{"spec": "<yaml>"}`).
    ///
    /// Args:
    ///     `spec_yaml`: The full deployment YAML as a string.
    ///
    /// Returns:
    ///     A dict describing the created deployment.
    fn deploy<'py>(&self, py: Python<'py>, spec_yaml: &str) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        let spec_yaml = spec_yaml.to_string();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let value = inner
                .create_deployment(&spec_yaml)
                .await
                .map_err(map_client_err)?;
            Python::attach(|py| json_to_py(py, &value))
        })
    }

    /// Fetch (non-streaming) logs for a single container.
    ///
    /// Wraps `DaemonClient::get_container_logs`
    /// (`GET /api/v1/containers/{id}/logs[?tail=N]`).
    ///
    /// Note:
    ///     `follow=True` is accepted for API shape parity but ignored —
    ///     the underlying endpoint returns a finite snapshot. Streaming
    ///     container logs will be added once `DaemonClient` gains a
    ///     dedicated helper for the SSE variant.
    ///
    /// Args:
    ///     `container_id`: The container id the daemon issued (e.g. from `ps`).
    ///     follow: Reserved; currently ignored.
    ///     tail: Optional max number of trailing log lines to return.
    ///
    /// Returns:
    ///     A string containing the concatenated log lines.
    #[pyo3(signature = (container_id, follow=false, tail=None))]
    fn logs<'py>(
        &self,
        py: Python<'py>,
        container_id: &str,
        follow: bool,
        tail: Option<u32>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        let container_id = container_id.to_string();
        let _ = follow; // see docstring — not yet wired.
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let text = inner
                .get_container_logs(&container_id, tail)
                .await
                .map_err(map_client_err)?;
            Ok(text)
        })
    }

    /// Execute a command inside the first replica of a service.
    ///
    /// Wraps `DaemonClient::exec_command`
    /// (`POST /api/v1/deployments/{deployment}/services/{service}/exec`).
    ///
    /// Args:
    ///     deployment: Deployment name.
    ///     service: Service name within the deployment.
    ///     cmd: The command and its arguments (e.g. `["sh", "-c", "ls /"]`).
    ///
    /// Returns:
    ///     A dict with `exit_code`, `stdout`, and `stderr` keys.
    fn exec<'py>(
        &self,
        py: Python<'py>,
        deployment: &str,
        service: &str,
        cmd: Vec<String>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        let deployment = deployment.to_string();
        let service = service.to_string();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let value = inner
                .exec_command(&deployment, &service, &cmd)
                .await
                .map_err(map_client_err)?;
            Python::attach(|py| json_to_py(py, &value))
        })
    }

    /// Delete a deployment (and stop all its services).
    ///
    /// Wraps `DaemonClient::delete_deployment`
    /// (`DELETE /api/v1/deployments/{name}`).
    ///
    /// Args:
    ///     deployment: The deployment name to stop.
    fn stop<'py>(&self, py: Python<'py>, deployment: &str) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        let deployment = deployment.to_string();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            inner
                .delete_deployment(&deployment)
                .await
                .map_err(map_client_err)?;
            Ok(())
        })
    }

    /// Get deployment details (status, services, replicas, etc.).
    ///
    /// Wraps `DaemonClient::get_deployment`
    /// (`GET /api/v1/deployments/{name}`).
    ///
    /// Args:
    ///     deployment: The deployment name to inspect.
    ///
    /// Returns:
    ///     A dict with the full deployment record as stored by the daemon.
    fn status<'py>(&self, py: Python<'py>, deployment: &str) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        let deployment = deployment.to_string();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let value = inner
                .get_deployment(&deployment)
                .await
                .map_err(map_client_err)?;
            Python::attach(|py| json_to_py(py, &value))
        })
    }

    /// Scale a service to a specific replica count.
    ///
    /// Wraps `DaemonClient::scale_service`
    /// (`POST /api/v1/deployments/{deployment}/services/{service}/scale`).
    ///
    /// Args:
    ///     deployment: Deployment name.
    ///     service: Service name within the deployment.
    ///     replicas: Desired replica count (>= 0).
    ///
    /// Returns:
    ///     A dict describing the updated service.
    fn scale<'py>(
        &self,
        py: Python<'py>,
        deployment: &str,
        service: &str,
        replicas: u32,
    ) -> PyResult<Bound<'py, PyAny>> {
        let inner = self.inner.clone();
        let deployment = deployment.to_string();
        let service = service.to_string();
        pyo3_async_runtimes::tokio::future_into_py(py, async move {
            let value = inner
                .scale_service(&deployment, &service, replicas)
                .await
                .map_err(map_client_err)?;
            Python::attach(|py| json_to_py(py, &value))
        })
    }
}
