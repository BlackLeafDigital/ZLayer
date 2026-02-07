//! Health checking for containers

use crate::error::{AgentError, Result};
use crate::runtime::ContainerId;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;
use zlayer_spec::HealthCheck;

/// Callback type for health state changes.
/// Called with (container_id, is_healthy) when health state transitions.
pub type HealthCallback = Arc<dyn Fn(ContainerId, bool) + Send + Sync>;

/// Health checker for containers
pub struct HealthChecker {
    pub check: HealthCheck,
    /// Optional target IP address for health checks (e.g., container overlay IP).
    /// When set, TCP and HTTP checks connect to this address instead of 127.0.0.1/localhost.
    target_addr: Option<std::net::IpAddr>,
}

impl HealthChecker {
    /// Create a new health checker
    ///
    /// `target_addr` is the IP address to connect to for TCP/HTTP checks.
    /// Pass `Some(ip)` when the container has an overlay IP, or `None` to
    /// fall back to `127.0.0.1` / localhost.
    pub fn new(check: HealthCheck, target_addr: Option<std::net::IpAddr>) -> Self {
        Self { check, target_addr }
    }

    /// Perform the health check
    pub async fn check(&self, id: &ContainerId, timeout: Duration) -> Result<()> {
        match &self.check {
            HealthCheck::Tcp { port } => self.check_tcp(id, *port, timeout).await,
            HealthCheck::Http { url, expect_status } => {
                self.check_http(id, url, *expect_status, timeout).await
            }
            HealthCheck::Command { command } => self.check_command(id, command, timeout).await,
        }
    }

    async fn check_tcp(&self, id: &ContainerId, port: u16, timeout_dur: Duration) -> Result<()> {
        // Connect to the target address (overlay IP if set, otherwise localhost)
        let host = self
            .target_addr
            .map(|ip| ip.to_string())
            .unwrap_or_else(|| "127.0.0.1".to_string());
        let addr = format!("{}:{}", host, port);
        match timeout(timeout_dur, tokio::net::TcpStream::connect(&addr)).await {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(e)) => Err(AgentError::HealthCheckFailed {
                id: id.to_string(),
                reason: format!("TCP connection failed: {}", e),
            }),
            Err(_) => Err(AgentError::Timeout {
                timeout: timeout_dur,
            }),
        }
    }

    async fn check_http(
        &self,
        id: &ContainerId,
        url: &str,
        expect_status: u16,
        timeout_dur: Duration,
    ) -> Result<()> {
        // If a target address is set, replace localhost / 127.0.0.1 in the URL
        // so the health check actually reaches the container's overlay IP.
        let url = if let Some(ip) = self.target_addr {
            let ip_str = ip.to_string();
            url.replace("localhost", &ip_str)
                .replace("127.0.0.1", &ip_str)
        } else {
            url.to_string()
        };

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .map_err(|e| AgentError::HealthCheckFailed {
                id: id.to_string(),
                reason: format!("failed to create HTTP client: {}", e),
            })?;

        match timeout(timeout_dur, client.get(&url).send()).await {
            Ok(Ok(resp)) => {
                let status = resp.status().as_u16();
                if status == expect_status {
                    Ok(())
                } else {
                    Err(AgentError::HealthCheckFailed {
                        id: id.to_string(),
                        reason: format!(
                            "unexpected status: {} (expected {})",
                            status, expect_status
                        ),
                    })
                }
            }
            Ok(Err(e)) => Err(AgentError::HealthCheckFailed {
                id: id.to_string(),
                reason: format!("HTTP request failed: {}", e),
            }),
            Err(_) => Err(AgentError::Timeout {
                timeout: timeout_dur,
            }),
        }
    }

    async fn check_command(
        &self,
        id: &ContainerId,
        command: &str,
        timeout_dur: Duration,
    ) -> Result<()> {
        match timeout(
            timeout_dur,
            tokio::process::Command::new("sh")
                .arg("-c")
                .arg(command)
                .output(),
        )
        .await
        {
            Ok(Ok(output)) => {
                if output.status.success() {
                    Ok(())
                } else {
                    Err(AgentError::HealthCheckFailed {
                        id: id.to_string(),
                        reason: format!(
                            "command failed with code {}: {}",
                            output.status.code().unwrap_or(-1),
                            String::from_utf8_lossy(&output.stderr)
                        ),
                    })
                }
            }
            Ok(Err(e)) => Err(AgentError::HealthCheckFailed {
                id: id.to_string(),
                reason: format!("command execution failed: {}", e),
            }),
            Err(_) => Err(AgentError::Timeout {
                timeout: timeout_dur,
            }),
        }
    }
}

/// Continuous health monitor
pub struct HealthMonitor {
    id: ContainerId,
    checker: HealthChecker,
    interval: Duration,
    retries: u32,
    state: tokio::sync::RwLock<HealthState>,
    on_health_change: Option<HealthCallback>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HealthState {
    Unknown,
    Checking,
    Healthy,
    Unhealthy { failures: u32, reason: String },
}

impl HealthMonitor {
    pub fn new(id: ContainerId, checker: HealthChecker, interval: Duration, retries: u32) -> Self {
        Self {
            id,
            checker,
            interval,
            retries,
            state: tokio::sync::RwLock::new(HealthState::Unknown),
            on_health_change: None,
        }
    }

    /// Set a callback to be invoked when health state changes (healthy <-> unhealthy).
    pub fn with_callback(mut self, callback: HealthCallback) -> Self {
        self.on_health_change = Some(callback);
        self
    }

    /// Start monitoring (spawns background task)
    pub fn start(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut failures = 0u32;
            let mut was_healthy: Option<bool> = None;

            loop {
                // Update state to checking
                *self.state.write().await = HealthState::Checking;

                match self.checker.check(&self.id, self.interval).await {
                    Ok(()) => {
                        failures = 0;
                        *self.state.write().await = HealthState::Healthy;

                        // Check for state transition to healthy
                        if was_healthy != Some(true) {
                            if let Some(ref callback) = self.on_health_change {
                                callback(self.id.clone(), true);
                            }
                            was_healthy = Some(true);
                        }
                    }
                    Err(e) => {
                        failures += 1;

                        *self.state.write().await = HealthState::Unhealthy {
                            failures,
                            reason: e.to_string(),
                        };

                        // Check for state transition to unhealthy
                        if was_healthy != Some(false) {
                            if let Some(ref callback) = self.on_health_change {
                                callback(self.id.clone(), false);
                            }
                            was_healthy = Some(false);
                        }

                        if failures >= self.retries {
                            break;
                        }
                    }
                }

                tokio::time::sleep(self.interval).await;
            }
        })
    }

    /// Get current health state
    pub async fn state(&self) -> HealthState {
        self.state.read().await.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_health_state() {
        let state = HealthState::Unhealthy {
            failures: 3,
            reason: "connection refused".to_string(),
        };
        assert_eq!(
            state,
            HealthState::Unhealthy {
                failures: 3,
                reason: "connection refused".to_string()
            }
        );
    }
}
