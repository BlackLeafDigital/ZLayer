//! Health checking for containers

use crate::error::{AgentError, Result};
use crate::runtime::ContainerId;
use std::time::Duration;
use tokio::time::timeout;
use zlayer_spec::HealthCheck;

/// Health checker for containers
pub struct HealthChecker {
    pub check: HealthCheck,
}

impl HealthChecker {
    /// Create a new health checker
    pub fn new(check: HealthCheck) -> Self {
        Self { check }
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
        // Try to connect to localhost:port
        let addr = format!("127.0.0.1:{}", port);
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
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .map_err(|e| AgentError::HealthCheckFailed {
                id: id.to_string(),
                reason: format!("failed to create HTTP client: {}", e),
            })?;

        match timeout(timeout_dur, client.get(url).send()).await {
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
        }
    }

    /// Start monitoring (spawns background task)
    pub fn start(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut failures = 0u32;

            loop {
                // Update state to checking
                *self.state.write().await = HealthState::Checking;

                match self.checker.check(&self.id, self.interval).await {
                    Ok(()) => {
                        failures = 0;
                        *self.state.write().await = HealthState::Healthy;
                    }
                    Err(e) => {
                        failures += 1;

                        *self.state.write().await = HealthState::Unhealthy {
                            failures,
                            reason: e.to_string(),
                        };

                        if failures >= self.retries {
                            // TODO: notify of unhealthy state
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
