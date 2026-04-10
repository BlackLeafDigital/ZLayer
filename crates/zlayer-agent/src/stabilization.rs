//! Deployment stabilization polling.
//!
//! Provides a reusable function that waits for all services in a deployment to
//! reach their desired replica count and pass health checks, or time out.
//!
//! This module lives in `zlayer-agent` (a library crate) so that both the
//! runtime binary and the API server can share the same stabilization logic
//! instead of duplicating it.

use std::time::{Duration, Instant};

use crate::health::HealthState;
use crate::service::ServiceManager;
use zlayer_spec::{DeploymentSpec, Protocol, ScaleSpec};

/// Per-service health summary returned by stabilization polling.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ServiceHealthSummary {
    /// Service name
    pub name: String,
    /// Running replica count
    pub running: u32,
    /// Desired replica count from the spec
    pub desired: u32,
    /// Whether health checks are passing for all running replicas
    pub healthy: bool,
    /// Endpoint URLs for this service (e.g. "<http://localhost:8080>")
    pub endpoints: Vec<String>,
}

/// Outcome of the stabilization wait.
///
/// This is intentionally decoupled from `DeploymentStatus` (which lives in
/// `zlayer-api`) to avoid circular dependencies. Callers should map this to
/// their own status types.
#[derive(Debug, Clone)]
pub enum StabilizationOutcome {
    /// All services reached their desired state within the timeout.
    Ready,
    /// The timeout expired before all services stabilized.
    TimedOut {
        /// Human-readable description of which services were not ready.
        message: String,
    },
}

/// Result of waiting for a deployment to stabilize.
#[derive(Debug, Clone)]
pub struct StabilizationResult {
    /// Whether stabilization succeeded or timed out
    pub outcome: StabilizationOutcome,
    /// Per-service health summaries (always populated regardless of outcome)
    pub services: Vec<ServiceHealthSummary>,
}

/// Wait for all services in a deployment to reach their desired replica count
/// and pass health checks, or time out.
///
/// Polls every 500ms for up to `timeout`. Returns [`StabilizationOutcome::Ready`]
/// if all services reach their desired state, or [`StabilizationOutcome::TimedOut`]
/// if the timeout expires.
pub async fn wait_for_stabilization(
    manager: &ServiceManager,
    spec: &DeploymentSpec,
    timeout: Duration,
) -> StabilizationResult {
    let poll_interval = Duration::from_millis(500);
    let start = Instant::now();

    loop {
        let mut all_ready = true;
        let mut summaries = Vec::with_capacity(spec.services.len());

        for (name, service_spec) in &spec.services {
            let desired = match &service_spec.scale {
                ScaleSpec::Fixed { replicas } => *replicas,
                ScaleSpec::Adaptive { min, .. } => *min,
                ScaleSpec::Manual => 0,
            };

            #[allow(clippy::cast_possible_truncation)]
            let running = match manager.service_replica_count(name).await {
                Ok(count) => count as u32,
                Err(_) => 0,
            };

            // Check health states from the manager
            let health_states = manager.health_states();
            let states = health_states.read().await;
            let healthy = match states.get(name) {
                Some(HealthState::Healthy) => true,
                _ if desired == 0 => true, // Manual scaling / 0 replicas is trivially healthy
                _ => false,
            };
            drop(states);

            let service_ready = running == desired && healthy;
            if !service_ready && desired > 0 {
                all_ready = false;
            }

            // Build endpoint URLs from the spec
            let endpoints: Vec<String> = service_spec
                .endpoints
                .iter()
                .map(|ep| {
                    let proto = match ep.protocol {
                        Protocol::Http => "http",
                        Protocol::Https => "https",
                        Protocol::Tcp => "tcp",
                        Protocol::Udp => "udp",
                        Protocol::Websocket => "ws",
                    };
                    format!("{}://localhost:{}", proto, ep.port)
                })
                .collect();

            summaries.push(ServiceHealthSummary {
                name: name.clone(),
                running,
                desired,
                healthy,
                endpoints,
            });
        }

        if all_ready {
            return StabilizationResult {
                outcome: StabilizationOutcome::Ready,
                services: summaries,
            };
        }

        if start.elapsed() >= timeout {
            // Build a failure message from unhealthy services, including
            // the tail of each failing service's container logs so the
            // user sees the real cause (e.g. "GLIBC_2.38 not found",
            // "failed to prepare rootfs", "panicked at ...") instead of
            // just "1/1 replicas, healthy=false".
            let failing: Vec<&ServiceHealthSummary> = summaries
                .iter()
                .filter(|s| (s.running != s.desired || !s.healthy) && s.desired > 0)
                .collect();

            let mut parts: Vec<String> = Vec::with_capacity(failing.len());
            for s in &failing {
                let header = format!(
                    "{}: {}/{} replicas, healthy={}",
                    s.name, s.running, s.desired, s.healthy
                );
                // Try to fetch the last 20 log lines from this service's
                // replicas. A miss (no containers, runtime error) falls
                // back to just the header so we never block the error
                // on log retrieval.
                match manager.get_service_logs(&s.name, 20, None).await {
                    Ok(entries) if !entries.is_empty() => {
                        let body = entries
                            .iter()
                            .map(|e| format!("    {}", e.message))
                            .collect::<Vec<_>>()
                            .join("\n");
                        parts.push(format!("{header}\n  logs:\n{body}"));
                    }
                    _ => parts.push(header),
                }
            }

            let message = if parts.is_empty() {
                "Stabilization timed out".to_string()
            } else {
                format!("Stabilization timed out:\n  {}", parts.join("\n  "))
            };

            return StabilizationResult {
                outcome: StabilizationOutcome::TimedOut { message },
                services: summaries,
            };
        }

        tokio::time::sleep(poll_interval).await;
    }
}
