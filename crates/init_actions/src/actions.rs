//! Built-in init actions

use crate::error::{InitError, Result};
use std::collections::HashMap;
use std::time::Duration;
use tokio::process::Command;
use tokio::time::{sleep, timeout};

/// Wait for a TCP port to be open
pub struct WaitTcp {
    pub host: String,
    pub port: u16,
    pub timeout: Duration,
    pub interval: Duration,
}

impl WaitTcp {
    pub async fn execute(&self) -> Result<()> {
        let start = std::time::Instant::now();

        loop {
            if tokio::net::TcpStream::connect(&format!("{}:{}", self.host, self.port))
                .await
                .is_ok()
            {
                return Ok(());
            }

            if start.elapsed() >= self.timeout {
                return Err(InitError::TcpFailed {
                    host: self.host.clone(),
                    port: self.port,
                    reason: format!("timeout after {:?}", self.timeout),
                });
            }

            sleep(self.interval).await;
        }
    }
}

/// Wait for an HTTP endpoint to respond
pub struct WaitHttp {
    pub url: String,
    pub expect_status: Option<u16>,
    pub timeout: Duration,
    pub interval: Duration,
}

impl WaitHttp {
    pub async fn execute(&self) -> Result<()> {
        let start = std::time::Instant::now();
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .map_err(|e| InitError::HttpFailed {
                url: self.url.clone(),
                reason: format!("failed to create client: {}", e),
            })?;

        loop {
            let response = client.get(&self.url).send().await;

            if let Ok(resp) = response {
                let status = resp.status().as_u16();

                if let Some(expected) = self.expect_status {
                    if status == expected {
                        return Ok(());
                    }
                } else if (200..300).contains(&status) {
                    return Ok(());
                }
            }

            if start.elapsed() >= self.timeout {
                return Err(InitError::HttpFailed {
                    url: self.url.clone(),
                    reason: format!("timeout after {:?}", self.timeout),
                });
            }

            sleep(self.interval).await;
        }
    }
}

/// Run a shell command
pub struct RunCommand {
    pub command: String,
    pub timeout: Duration,
}

impl RunCommand {
    pub async fn execute(&self) -> Result<()> {
        match timeout(
            self.timeout,
            Command::new("sh").arg("-c").arg(&self.command).output(),
        )
        .await
        {
            Ok(Ok(output)) => {
                if output.status.success() {
                    Ok(())
                } else {
                    Err(InitError::CommandFailed {
                        command: self.command.clone(),
                        code: output.status.code().unwrap_or(-1),
                        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
                        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
                    })
                }
            }
            Ok(Err(_)) => Err(InitError::CommandFailed {
                command: self.command.clone(),
                code: -1,
                stdout: String::new(),
                stderr: "timeout".to_string(),
            }),
            Err(_) => Err(InitError::Timeout {
                timeout: self.timeout,
            }),
        }
    }
}

/// Create an init action from the spec
pub fn from_spec(
    action: &str,
    params: &HashMap<String, serde_json::Value>,
    _default_timeout: Duration,
) -> Result<InitAction> {
    match action {
        "init.wait_tcp" => {
            let host = params
                .get("host")
                .and_then(|v| v.as_str())
                .ok_or_else(|| InitError::InvalidParams {
                    action: action.to_string(),
                    reason: "missing 'host' parameter".to_string(),
                })?
                .to_string();

            let port = params.get("port").and_then(|v| v.as_u64()).ok_or_else(|| {
                InitError::InvalidParams {
                    action: action.to_string(),
                    reason: "missing or invalid 'port' parameter".to_string(),
                }
            })? as u16;

            let timeout_secs = params.get("timeout").and_then(|v| v.as_u64()).unwrap_or(30);

            Ok(InitAction::WaitTcp(WaitTcp {
                host,
                port,
                timeout: Duration::from_secs(timeout_secs),
                interval: Duration::from_secs(2),
            }))
        }

        "init.wait_http" => {
            let url = params
                .get("url")
                .and_then(|v| v.as_str())
                .ok_or_else(|| InitError::InvalidParams {
                    action: action.to_string(),
                    reason: "missing 'url' parameter".to_string(),
                })?
                .to_string();

            let expect_status = params
                .get("expect_status")
                .and_then(|v| v.as_u64())
                .map(|v| v as u16);

            let timeout_secs = params.get("timeout").and_then(|v| v.as_u64()).unwrap_or(30);

            Ok(InitAction::WaitHttp(WaitHttp {
                url,
                expect_status,
                timeout: Duration::from_secs(timeout_secs),
                interval: Duration::from_secs(2),
            }))
        }

        "init.run" => {
            let command = params
                .get("command")
                .and_then(|v| v.as_str())
                .ok_or_else(|| InitError::InvalidParams {
                    action: action.to_string(),
                    reason: "missing 'command' parameter".to_string(),
                })?
                .to_string();

            let timeout_secs = params
                .get("timeout")
                .and_then(|v| v.as_u64())
                .unwrap_or(300);

            Ok(InitAction::Run(RunCommand {
                command,
                timeout: Duration::from_secs(timeout_secs),
            }))
        }

        _ => Err(InitError::UnknownAction(action.to_string())),
    }
}

/// Enum of all init actions
pub enum InitAction {
    WaitTcp(WaitTcp),
    WaitHttp(WaitHttp),
    Run(RunCommand),
}

impl InitAction {
    pub async fn execute(&self) -> Result<()> {
        match self {
            InitAction::WaitTcp(a) => a.execute().await,
            InitAction::WaitHttp(a) => a.execute().await,
            InitAction::Run(a) => a.execute().await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_run_command_success() {
        let action = RunCommand {
            command: "echo hello".to_string(),
            timeout: Duration::from_secs(5),
        };
        action.execute().await.unwrap();
    }

    #[tokio::test]
    async fn test_run_command_failure() {
        let action = RunCommand {
            command: "exit 1".to_string(),
            timeout: Duration::from_secs(5),
        };
        let result = action.execute().await;
        assert!(result.is_err());
    }

    #[test]
    fn test_from_spec_wait_tcp() {
        let mut params = HashMap::new();
        params.insert("host".to_string(), serde_json::json!("localhost"));
        params.insert("port".to_string(), serde_json::json!(8080));

        let action = from_spec("init.wait_tcp", &params, Duration::from_secs(30)).unwrap();
        match action {
            InitAction::WaitTcp(_) => {}
            _ => panic!("Expected WaitTcp action"),
        }
    }

    #[test]
    fn test_from_spec_unknown() {
        let params = HashMap::new();
        let result = from_spec("unknown.action", &params, Duration::from_secs(30));
        assert!(result.is_err());
    }
}
