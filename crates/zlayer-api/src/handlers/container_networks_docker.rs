//! Docker-backed implementation of [`BridgeNetworkRuntime`].
//!
//! Compiled only when the `docker` feature is enabled. Uses [`bollard`] to
//! talk to the local Docker daemon, mirroring the same connection defaults
//! that `zlayer-agent::runtimes::docker::DockerRuntime` uses (Unix socket on
//! Linux / macOS, named pipe on Windows).
//!
//! The trait itself lives in
//! [`crate::handlers::container_networks`]. Keeping the trait in `zlayer-api`
//! and this implementation behind an optional dep lets the trait object be
//! passed down from the binary without forcing every consumer of the API
//! layer to link bollard.

use std::collections::HashMap;

use async_trait::async_trait;
use bollard::errors::Error as BollardError;
use bollard::models::{
    EndpointIpamConfig, EndpointSettings, Ipam, IpamConfig, NetworkConnectRequest,
    NetworkCreateRequest, NetworkDisconnectRequest,
};
use bollard::Docker;

use super::container_networks::{BridgeNetworkRuntime, RuntimeError};
use zlayer_spec::{BridgeNetwork, BridgeNetworkAttachment, BridgeNetworkDriver};

/// Docker daemon-backed realization of [`BridgeNetworkRuntime`].
///
/// Holds a cheap-to-clone [`Docker`] client. A single instance is shared by
/// every handler invocation via the `Arc<dyn BridgeNetworkRuntime>` stored on
/// [`crate::handlers::container_networks::BridgeNetworkApiState`].
pub struct DockerBridgeNetworkRuntime {
    docker: Docker,
}

impl std::fmt::Debug for DockerBridgeNetworkRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DockerBridgeNetworkRuntime")
            .finish_non_exhaustive()
    }
}

impl DockerBridgeNetworkRuntime {
    /// Connect to the local Docker daemon using bollard's
    /// platform-appropriate defaults and ping to verify connectivity.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::Failed`] if the Docker daemon cannot be
    /// reached — the caller is expected to treat this as "Docker unavailable"
    /// and leave [`BridgeNetworkApiState`] in metadata-only mode.
    ///
    /// [`BridgeNetworkApiState`]: crate::handlers::container_networks::BridgeNetworkApiState
    pub async fn connect_local() -> std::result::Result<Self, RuntimeError> {
        let docker = Docker::connect_with_local_defaults()
            .map_err(|e| RuntimeError::Failed(format!("failed to connect to Docker: {e}")))?;
        docker
            .ping()
            .await
            .map_err(|e| RuntimeError::Failed(format!("Docker ping failed: {e}")))?;
        Ok(Self { docker })
    }

    /// Build from an existing bollard [`Docker`] handle. Useful for tests and
    /// for callers that already have a configured client.
    #[must_use]
    pub fn with_client(docker: Docker) -> Self {
        Self { docker }
    }
}

/// Map a [`BollardError`] to a [`RuntimeError`] by matching status codes
/// (for the structured `DockerResponseServerError` variant) and falling back
/// to string matching against the error's `Display`.
///
/// Docker returns:
/// - `404` for unknown network/container names — surfaced as
///   [`RuntimeError::NotFound`].
/// - `409` for "already exists" (duplicate network name / container already
///   attached) — surfaced as [`RuntimeError::AlreadyExists`].
/// - Everything else collapses to [`RuntimeError::Failed`].
fn map_bollard_err(err: &BollardError, context: &str) -> RuntimeError {
    if let BollardError::DockerResponseServerError {
        status_code,
        message,
    } = err
    {
        return match *status_code {
            404 => RuntimeError::NotFound(format!("{context}: {message}")),
            409 => RuntimeError::AlreadyExists(format!("{context}: {message}")),
            _ => RuntimeError::Failed(format!("{context}: {message}")),
        };
    }

    // Fallback: string-match against the rendered message. `bollard` doesn't
    // expose a structured variant for every network error mode (e.g. some
    // failures surface as plain I/O or hyper errors), so this catches the
    // "already exists" / "not found" cases that come through those paths.
    let s = err.to_string();
    let lower = s.to_ascii_lowercase();
    if lower.contains("not found") || lower.contains("no such network") {
        RuntimeError::NotFound(format!("{context}: {s}"))
    } else if lower.contains("already exists") {
        RuntimeError::AlreadyExists(format!("{context}: {s}"))
    } else {
        RuntimeError::Failed(format!("{context}: {s}"))
    }
}

/// Map a [`BridgeNetworkDriver`] to the Docker driver string. Docker's
/// built-in drivers use lowercase names.
fn driver_name(driver: BridgeNetworkDriver) -> &'static str {
    match driver {
        BridgeNetworkDriver::Bridge => "bridge",
        BridgeNetworkDriver::Overlay => "overlay",
    }
}

/// Build an [`Ipam`] config from an optional subnet string. Docker's default
/// IPAM driver is named `"default"` — leaving it `None` is equivalent but we
/// set it explicitly for clarity.
fn build_ipam(subnet: Option<&str>) -> Option<Ipam> {
    let subnet = subnet?;
    Some(Ipam {
        driver: Some("default".to_string()),
        config: Some(vec![IpamConfig {
            subnet: Some(subnet.to_string()),
            ..Default::default()
        }]),
        options: None,
    })
}

#[async_trait]
impl BridgeNetworkRuntime for DockerBridgeNetworkRuntime {
    async fn create(&self, spec: &BridgeNetwork) -> std::result::Result<(), RuntimeError> {
        let labels: HashMap<String, String> = spec.labels.clone();

        let request = NetworkCreateRequest {
            name: spec.name.clone(),
            driver: Some(driver_name(spec.driver).to_string()),
            internal: Some(spec.internal),
            ipam: build_ipam(spec.subnet.as_deref()),
            labels: if labels.is_empty() {
                None
            } else {
                Some(labels)
            },
            ..Default::default()
        };

        self.docker
            .create_network(request)
            .await
            .map(|_response| ())
            .map_err(|e| map_bollard_err(&e, &format!("create_network '{}'", spec.name)))
    }

    async fn delete(&self, id: &str) -> std::result::Result<(), RuntimeError> {
        self.docker
            .remove_network(id)
            .await
            .map_err(|e| map_bollard_err(&e, &format!("remove_network '{id}'")))
    }

    async fn connect(
        &self,
        network: &str,
        attachment: &BridgeNetworkAttachment,
    ) -> std::result::Result<(), RuntimeError> {
        let ipam_config = attachment.ipv4.as_ref().map(|ipv4| EndpointIpamConfig {
            ipv4_address: Some(ipv4.clone()),
            ..Default::default()
        });

        let aliases = if attachment.aliases.is_empty() {
            None
        } else {
            Some(attachment.aliases.clone())
        };

        let endpoint_config = if ipam_config.is_some() || aliases.is_some() {
            Some(EndpointSettings {
                ipam_config,
                aliases,
                ..Default::default()
            })
        } else {
            None
        };

        let request = NetworkConnectRequest {
            container: attachment.container_id.clone(),
            endpoint_config,
        };

        self.docker
            .connect_network(network, request)
            .await
            .map_err(|e| {
                map_bollard_err(
                    &e,
                    &format!(
                        "connect_network '{network}' <- '{}'",
                        attachment.container_id
                    ),
                )
            })
    }

    async fn disconnect(
        &self,
        network: &str,
        container: &str,
    ) -> std::result::Result<(), RuntimeError> {
        let request = NetworkDisconnectRequest {
            container: container.to_string(),
            force: Some(false),
        };

        self.docker
            .disconnect_network(network, request)
            .await
            .map_err(|e| {
                map_bollard_err(
                    &e,
                    &format!("disconnect_network '{network}' -x- '{container}'"),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn driver_name_maps_variants() {
        assert_eq!(driver_name(BridgeNetworkDriver::Bridge), "bridge");
        assert_eq!(driver_name(BridgeNetworkDriver::Overlay), "overlay");
    }

    #[test]
    fn build_ipam_none_returns_none() {
        assert!(build_ipam(None).is_none());
    }

    #[test]
    fn build_ipam_some_populates_subnet() {
        let ipam = build_ipam(Some("10.240.0.0/24")).expect("ipam built");
        assert_eq!(ipam.driver.as_deref(), Some("default"));
        let config = ipam.config.expect("config present");
        assert_eq!(config.len(), 1);
        assert_eq!(config[0].subnet.as_deref(), Some("10.240.0.0/24"));
    }

    #[test]
    fn map_bollard_err_404_becomes_not_found() {
        let err = BollardError::DockerResponseServerError {
            status_code: 404,
            message: "no such network: foo".to_string(),
        };
        assert!(matches!(
            map_bollard_err(&err, "op"),
            RuntimeError::NotFound(_)
        ));
    }

    #[test]
    fn map_bollard_err_409_becomes_already_exists() {
        let err = BollardError::DockerResponseServerError {
            status_code: 409,
            message: "network 'foo' already exists".to_string(),
        };
        assert!(matches!(
            map_bollard_err(&err, "op"),
            RuntimeError::AlreadyExists(_)
        ));
    }

    #[test]
    fn map_bollard_err_other_status_becomes_failed() {
        let err = BollardError::DockerResponseServerError {
            status_code: 500,
            message: "kaboom".to_string(),
        };
        assert!(matches!(
            map_bollard_err(&err, "op"),
            RuntimeError::Failed(_)
        ));
    }

    /// Ensures the string-fallback path catches "not found" errors that
    /// arrive as a non-`DockerResponseServerError` variant (e.g. I/O errors
    /// wrapping a transport-level failure).
    #[test]
    fn map_bollard_err_string_fallback_not_found() {
        let err = BollardError::IOError {
            err: std::io::Error::new(std::io::ErrorKind::NotFound, "no such network: foo"),
        };
        assert!(matches!(
            map_bollard_err(&err, "op"),
            RuntimeError::NotFound(_)
        ));
    }

    /// Build a plausible `BridgeNetwork` to confirm the struct construction
    /// path compiles with the fields we use from bollard's model types.
    #[test]
    fn request_construction_smoke() {
        let _ = NetworkCreateRequest {
            name: "n".to_string(),
            driver: Some("bridge".to_string()),
            internal: Some(false),
            ipam: build_ipam(Some("10.0.0.0/24")),
            labels: Some(HashMap::new()),
            ..Default::default()
        };
        let _ = NetworkConnectRequest {
            container: "cid".to_string(),
            endpoint_config: Some(EndpointSettings {
                aliases: Some(vec!["web".to_string()]),
                ipam_config: Some(EndpointIpamConfig {
                    ipv4_address: Some("10.0.0.5".to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            }),
        };
        let _ = NetworkDisconnectRequest {
            container: "cid".to_string(),
            force: Some(false),
        };
    }
}
