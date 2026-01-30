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

/// Push files to S3 from a local path
#[cfg(feature = "s3")]
pub struct S3Push {
    /// Local source path (file or directory)
    pub source: String,
    /// S3 bucket name
    pub bucket: String,
    /// S3 key prefix
    pub key: String,
    /// Custom S3 endpoint (for S3-compatible services)
    pub endpoint: Option<String>,
    /// Region
    pub region: Option<String>,
    /// Upload timeout
    pub timeout: Duration,
}

#[cfg(feature = "s3")]
impl S3Push {
    pub async fn execute(&self) -> Result<()> {
        use aws_sdk_s3::Client;

        // Build AWS config
        let mut config_loader = aws_config::defaults(aws_config::BehaviorVersion::latest());
        if let Some(ref region) = self.region {
            config_loader = config_loader.region(aws_config::Region::new(region.clone()));
        }
        let sdk_config = config_loader.load().await;

        // Build S3 client
        let mut s3_config = aws_sdk_s3::config::Builder::from(&sdk_config);
        if let Some(ref endpoint) = self.endpoint {
            s3_config = s3_config.endpoint_url(endpoint).force_path_style(true);
        }
        let client = Client::from_conf(s3_config.build());

        let source_path = std::path::Path::new(&self.source);

        if source_path.is_file() {
            // Upload single file
            self.upload_file(&client, source_path, &self.key).await?;
        } else if source_path.is_dir() {
            // Upload directory recursively
            self.upload_directory(&client, source_path, &self.key)
                .await?;
        } else {
            return Err(InitError::S3Failed {
                bucket: self.bucket.clone(),
                key: self.key.clone(),
                reason: format!("source path '{}' does not exist", self.source),
            });
        }

        Ok(())
    }

    #[cfg(feature = "s3")]
    async fn upload_file(
        &self,
        client: &aws_sdk_s3::Client,
        path: &std::path::Path,
        key: &str,
    ) -> Result<()> {
        use aws_sdk_s3::primitives::ByteStream;

        tracing::info!(
            bucket = %self.bucket,
            key = %key,
            source = %path.display(),
            "pushing file to S3"
        );

        let data = tokio::fs::read(path)
            .await
            .map_err(|e| InitError::S3Failed {
                bucket: self.bucket.clone(),
                key: key.to_string(),
                reason: format!("failed to read file: {}", e),
            })?;

        tokio::time::timeout(
            self.timeout,
            client
                .put_object()
                .bucket(&self.bucket)
                .key(key)
                .body(ByteStream::from(data))
                .content_type("application/octet-stream")
                .send(),
        )
        .await
        .map_err(|_| InitError::Timeout {
            timeout: self.timeout,
        })?
        .map_err(|e| InitError::S3Failed {
            bucket: self.bucket.clone(),
            key: key.to_string(),
            reason: format!("put_object failed: {}", e),
        })?;

        tracing::info!(bucket = %self.bucket, key = %key, "S3 push complete");
        Ok(())
    }

    #[cfg(feature = "s3")]
    async fn upload_directory(
        &self,
        client: &aws_sdk_s3::Client,
        dir: &std::path::Path,
        prefix: &str,
    ) -> Result<()> {
        let mut entries = tokio::fs::read_dir(dir)
            .await
            .map_err(|e| InitError::S3Failed {
                bucket: self.bucket.clone(),
                key: prefix.to_string(),
                reason: format!("failed to read directory: {}", e),
            })?;

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| InitError::S3Failed {
                bucket: self.bucket.clone(),
                key: prefix.to_string(),
                reason: format!("failed to read directory entry: {}", e),
            })?
        {
            let path = entry.path();
            let file_name = entry.file_name();
            let key = format!(
                "{}/{}",
                prefix.trim_end_matches('/'),
                file_name.to_string_lossy()
            );

            if path.is_file() {
                self.upload_file(client, &path, &key).await?;
            } else if path.is_dir() {
                // Use Box::pin for recursive async
                Box::pin(self.upload_directory(client, &path, &key)).await?;
            }
        }

        Ok(())
    }
}

/// Pull files from S3 to a local path
#[cfg(feature = "s3")]
pub struct S3Pull {
    /// S3 bucket name
    pub bucket: String,
    /// S3 key or prefix to download
    pub key: String,
    /// Local destination path
    pub destination: String,
    /// Custom S3 endpoint (for S3-compatible services)
    pub endpoint: Option<String>,
    /// Region
    pub region: Option<String>,
    /// Download timeout
    pub timeout: Duration,
}

#[cfg(feature = "s3")]
impl S3Pull {
    pub async fn execute(&self) -> Result<()> {
        use aws_sdk_s3::Client;
        use tokio::io::AsyncWriteExt;

        // Build AWS config
        let mut config_loader = aws_config::defaults(aws_config::BehaviorVersion::latest());
        if let Some(ref region) = self.region {
            config_loader = config_loader.region(aws_config::Region::new(region.clone()));
        }
        let sdk_config = config_loader.load().await;

        // Build S3 client
        let mut s3_config = aws_sdk_s3::config::Builder::from(&sdk_config);
        if let Some(ref endpoint) = self.endpoint {
            s3_config = s3_config.endpoint_url(endpoint).force_path_style(true);
        }
        let client = Client::from_conf(s3_config.build());

        tracing::info!(
            bucket = %self.bucket,
            key = %self.key,
            destination = %self.destination,
            "pulling from S3"
        );

        // Get object from S3
        let result = tokio::time::timeout(
            self.timeout,
            client
                .get_object()
                .bucket(&self.bucket)
                .key(&self.key)
                .send(),
        )
        .await
        .map_err(|_| InitError::Timeout {
            timeout: self.timeout,
        })?
        .map_err(|e| InitError::S3Failed {
            bucket: self.bucket.clone(),
            key: self.key.clone(),
            reason: format!("get_object failed: {}", e),
        })?;

        // Read body
        let data = result
            .body
            .collect()
            .await
            .map_err(|e| InitError::S3Failed {
                bucket: self.bucket.clone(),
                key: self.key.clone(),
                reason: format!("failed to read body: {}", e),
            })?
            .into_bytes();

        // Write to destination
        let dest_path = std::path::Path::new(&self.destination);
        if let Some(parent) = dest_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| InitError::S3Failed {
                    bucket: self.bucket.clone(),
                    key: self.key.clone(),
                    reason: format!("failed to create destination directory: {}", e),
                })?;
        }

        let mut file = tokio::fs::File::create(&self.destination)
            .await
            .map_err(|e| InitError::S3Failed {
                bucket: self.bucket.clone(),
                key: self.key.clone(),
                reason: format!("failed to create file: {}", e),
            })?;

        file.write_all(&data)
            .await
            .map_err(|e| InitError::S3Failed {
                bucket: self.bucket.clone(),
                key: self.key.clone(),
                reason: format!("failed to write file: {}", e),
            })?;

        tracing::info!(
            bucket = %self.bucket,
            key = %self.key,
            bytes = data.len(),
            "S3 pull complete"
        );

        Ok(())
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

        #[cfg(feature = "s3")]
        "init.s3_push" => {
            let source = params
                .get("source")
                .and_then(|v| v.as_str())
                .ok_or_else(|| InitError::InvalidParams {
                    action: action.to_string(),
                    reason: "missing 'source' parameter".to_string(),
                })?
                .to_string();

            let bucket = params
                .get("bucket")
                .and_then(|v| v.as_str())
                .ok_or_else(|| InitError::InvalidParams {
                    action: action.to_string(),
                    reason: "missing 'bucket' parameter".to_string(),
                })?
                .to_string();

            let key = params
                .get("key")
                .and_then(|v| v.as_str())
                .ok_or_else(|| InitError::InvalidParams {
                    action: action.to_string(),
                    reason: "missing 'key' parameter".to_string(),
                })?
                .to_string();

            let endpoint = params
                .get("endpoint")
                .and_then(|v| v.as_str())
                .map(String::from);
            let region = params
                .get("region")
                .and_then(|v| v.as_str())
                .map(String::from);
            let timeout_secs = params
                .get("timeout")
                .and_then(|v| v.as_u64())
                .unwrap_or(300);

            Ok(InitAction::S3Push(S3Push {
                source,
                bucket,
                key,
                endpoint,
                region,
                timeout: Duration::from_secs(timeout_secs),
            }))
        }

        #[cfg(feature = "s3")]
        "init.s3_pull" => {
            let bucket = params
                .get("bucket")
                .and_then(|v| v.as_str())
                .ok_or_else(|| InitError::InvalidParams {
                    action: action.to_string(),
                    reason: "missing 'bucket' parameter".to_string(),
                })?
                .to_string();

            let key = params
                .get("key")
                .and_then(|v| v.as_str())
                .ok_or_else(|| InitError::InvalidParams {
                    action: action.to_string(),
                    reason: "missing 'key' parameter".to_string(),
                })?
                .to_string();

            let destination = params
                .get("destination")
                .and_then(|v| v.as_str())
                .ok_or_else(|| InitError::InvalidParams {
                    action: action.to_string(),
                    reason: "missing 'destination' parameter".to_string(),
                })?
                .to_string();

            let endpoint = params
                .get("endpoint")
                .and_then(|v| v.as_str())
                .map(String::from);
            let region = params
                .get("region")
                .and_then(|v| v.as_str())
                .map(String::from);
            let timeout_secs = params
                .get("timeout")
                .and_then(|v| v.as_u64())
                .unwrap_or(300);

            Ok(InitAction::S3Pull(S3Pull {
                bucket,
                key,
                destination,
                endpoint,
                region,
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
    #[cfg(feature = "s3")]
    S3Push(S3Push),
    #[cfg(feature = "s3")]
    S3Pull(S3Pull),
}

impl InitAction {
    pub async fn execute(&self) -> Result<()> {
        match self {
            InitAction::WaitTcp(a) => a.execute().await,
            InitAction::WaitHttp(a) => a.execute().await,
            InitAction::Run(a) => a.execute().await,
            #[cfg(feature = "s3")]
            InitAction::S3Push(a) => a.execute().await,
            #[cfg(feature = "s3")]
            InitAction::S3Pull(a) => a.execute().await,
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
