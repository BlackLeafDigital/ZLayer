//! HTTP-over-Unix-socket client for CLI-to-daemon communication.
//!
//! Provides a typed [`DaemonClient`] that communicates with the `zlayer-runtime serve`
//! daemon via its Unix domain socket (platform-dependent path; see
//! [`default_socket_path()`]).  If the daemon is not running,
//! [`DaemonClient::connect`] will auto-start it and wait with exponential backoff
//! until the socket becomes available.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use anyhow::{bail, Context as _, Result};
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::Uri;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use tokio::net::UnixStream;
use tracing::{debug, info};

/// Default path for the daemon Unix socket.
///
/// On macOS: `~/.local/share/zlayer/run/zlayer.sock`.
/// On Linux: `/var/run/zlayer.sock`.
pub fn default_socket_path() -> String {
    crate::cli::default_socket_path(&crate::cli::default_data_dir())
}

// ---------------------------------------------------------------------------
// Unix socket connector (tower::Service<Uri>)
// ---------------------------------------------------------------------------

/// A [`tower::Service`] connector that routes every request to a fixed Unix
/// domain socket, regardless of the URI's host/port.  This lets us reuse the
/// standard `hyper_util::client::legacy::Client` for HTTP-over-UDS.
#[derive(Clone)]
struct UnixConnector {
    socket_path: PathBuf,
}

impl UnixConnector {
    fn new(socket_path: impl Into<PathBuf>) -> Self {
        Self {
            socket_path: socket_path.into(),
        }
    }
}

impl tower::Service<Uri> for UnixConnector {
    type Response = hyper_util::rt::TokioIo<UnixStream>;
    type Error = std::io::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, _uri: Uri) -> Self::Future {
        let path = self.socket_path.clone();
        Box::pin(async move {
            let stream = UnixStream::connect(&path).await?;
            Ok(hyper_util::rt::TokioIo::new(stream))
        })
    }
}

// ---------------------------------------------------------------------------
// DaemonClient
// ---------------------------------------------------------------------------

/// HTTP client that communicates with the zlayer daemon over a Unix socket.
///
/// All API methods send HTTP requests to the daemon's REST API.  The URI host
/// component is set to `localhost` (ignored by the connector) and the path
/// matches the route definitions in `zlayer-api`.
pub struct DaemonClient {
    /// Inner hyper client wired to the Unix connector.
    client: Client<UnixConnector, Full<Bytes>>,
    /// Path to the Unix socket file.
    socket_path: PathBuf,
}

impl DaemonClient {
    // ------------------------------------------------------------------
    // Construction / connection
    // ------------------------------------------------------------------

    /// Connect to the daemon, auto-starting it if it is not already running.
    ///
    /// Checks for the Unix socket at [`default_socket_path()`].  If the socket
    /// does not exist and no daemon PID is found, `zlayer-runtime serve --daemon`
    /// is spawned and the function polls the socket with exponential backoff
    /// (100 ms -> 200 ms -> 400 ms -> ... -> 1 s, max 20 attempts, ~10 s total).
    pub async fn connect() -> Result<Self> {
        Self::connect_to(&default_socket_path()).await
    }

    /// Like [`connect`](Self::connect) but with a custom socket path.
    pub async fn connect_to(socket_path: impl AsRef<Path>) -> Result<Self> {
        let socket_path = socket_path.as_ref().to_path_buf();

        // Fast path: socket file exists and we can talk to it.
        if socket_path.exists() {
            if let Ok(client) = Self::try_build(&socket_path).await {
                return Ok(client);
            }
            // Socket file exists but daemon may be dead; fall through to
            // auto-start logic.
        }

        // No running daemon detected -- start one.
        info!("Daemon not running, auto-starting...");
        Self::auto_start_daemon(&socket_path).await?;

        Self::try_build(&socket_path)
            .await
            .context("Failed to connect to daemon after auto-start")
    }

    /// Build the client and verify that the daemon is reachable via a health
    /// check.  Returns `Err` if the health probe fails.
    async fn try_build(socket_path: &Path) -> Result<Self> {
        let connector = UnixConnector::new(socket_path);
        let client = Client::builder(TokioExecutor::new()).build(connector);

        let dc = Self {
            client,
            socket_path: socket_path.to_path_buf(),
        };

        // Quick health probe -- if this fails the daemon is not (yet) ready.
        if dc.health_check().await? {
            Ok(dc)
        } else {
            bail!("Daemon health check returned unhealthy")
        }
    }

    // ------------------------------------------------------------------
    // Auto-start
    // ------------------------------------------------------------------

    /// Locate the `zlayer-runtime` binary using the same search strategy as
    /// `bin/zlayer/src/cli.rs::which_runtime()`.
    fn find_runtime_binary() -> Result<PathBuf> {
        // Well-known install locations
        for path in &["/usr/local/bin/zlayer-runtime", "/usr/bin/zlayer-runtime"] {
            let p = PathBuf::from(path);
            if p.exists() {
                return Ok(p);
            }
        }

        // Next to the current executable
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                let sibling = dir.join("zlayer-runtime");
                if sibling.exists() {
                    return Ok(sibling);
                }
            }
        }

        // Fall back to $PATH
        if let Ok(output) = std::process::Command::new("which")
            .arg("zlayer-runtime")
            .output()
        {
            if output.status.success() {
                let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !path.is_empty() {
                    return Ok(PathBuf::from(path));
                }
            }
        }

        bail!(
            "zlayer-runtime not found. Install it or add it to your PATH.\n\
             The daemon requires zlayer-runtime to be available."
        )
    }

    /// Spawn `zlayer-runtime serve --daemon` and poll the socket with
    /// exponential backoff until the daemon is reachable or we give up.
    async fn auto_start_daemon(socket_path: &Path) -> Result<()> {
        let runtime_bin = Self::find_runtime_binary()?;

        debug!(binary = %runtime_bin.display(), "Spawning daemon");

        let mut cmd = std::process::Command::new(&runtime_bin);

        // Ensure the child process knows the correct data directory.
        // If ZLAYER_DATA_DIR is already set in the environment it will be
        // inherited automatically (clap picks it up via `env = "ZLAYER_DATA_DIR"`).
        // Otherwise, explicitly pass --data-dir so the child doesn't fall back
        // to /var/lib/zlayer when $HOME is unavailable (e.g. launchd context).
        if std::env::var_os("ZLAYER_DATA_DIR").is_none() {
            let data_dir = crate::cli::default_data_dir();
            cmd.arg("--data-dir").arg(&data_dir);
        }

        cmd.arg("serve")
            .arg("--daemon")
            .arg("--socket")
            .arg(socket_path.as_os_str());

        let status = cmd
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .with_context(|| {
                format!("Failed to spawn daemon process: {}", runtime_bin.display())
            })?;

        // The `--daemon` flag causes fork()+setsid() so the parent returns
        // immediately with exit code 0 while the child continues in the
        // background.  If the parent itself fails, that's a real error.
        if !status.success() {
            bail!(
                "Daemon process exited with status {} (binary: {})",
                status,
                runtime_bin.display()
            );
        }

        // Poll socket with exponential backoff: 100ms, 200ms, 400ms, 800ms,
        // then 1s for the remaining attempts.  Max 20 attempts (~10 s).
        let mut delay = Duration::from_millis(100);
        let max_delay = Duration::from_secs(1);
        let max_attempts = 20u32;

        for attempt in 1..=max_attempts {
            tokio::time::sleep(delay).await;

            if socket_path.exists() {
                // Try connecting to confirm the daemon is accepting connections.
                match UnixStream::connect(socket_path).await {
                    Ok(_) => {
                        info!(attempts = attempt, "Daemon socket is accepting connections");
                        return Ok(());
                    }
                    Err(e) => {
                        debug!(
                            attempt,
                            error = %e,
                            "Socket exists but not yet accepting connections"
                        );
                    }
                }
            } else {
                debug!(attempt, "Waiting for daemon socket to appear...");
            }

            delay = std::cmp::min(delay * 2, max_delay);
        }

        bail!(
            "Timed out waiting for daemon to start after {} attempts (~10 s). \
             Socket path: {}",
            max_attempts,
            socket_path.display()
        )
    }

    // ------------------------------------------------------------------
    // Low-level HTTP helpers
    // ------------------------------------------------------------------

    /// Send a GET request and return the response body as bytes.
    async fn get(&self, path: &str) -> Result<(hyper::StatusCode, Bytes)> {
        let uri: Uri = format!("http://localhost{path}")
            .parse()
            .with_context(|| format!("Invalid request path: {path}"))?;

        let req = hyper::Request::builder()
            .method(hyper::Method::GET)
            .uri(uri)
            .header("Host", "localhost")
            .body(Full::new(Bytes::new()))
            .context("Failed to build GET request")?;

        let resp = self
            .client
            .request(req)
            .await
            .with_context(|| format!("GET {path} failed"))?;

        let status = resp.status();
        let body = resp
            .into_body()
            .collect()
            .await
            .context("Failed to read response body")?
            .to_bytes();

        Ok((status, body))
    }

    /// Send a POST request with a JSON body and return the response.
    async fn post_json(&self, path: &str, body: &str) -> Result<(hyper::StatusCode, Bytes)> {
        let uri: Uri = format!("http://localhost{path}")
            .parse()
            .with_context(|| format!("Invalid request path: {path}"))?;

        let req = hyper::Request::builder()
            .method(hyper::Method::POST)
            .uri(uri)
            .header("Host", "localhost")
            .header("Content-Type", "application/json")
            .body(Full::new(Bytes::from(body.to_owned())))
            .context("Failed to build POST request")?;

        let resp = self
            .client
            .request(req)
            .await
            .with_context(|| format!("POST {path} failed"))?;

        let status = resp.status();
        let body = resp
            .into_body()
            .collect()
            .await
            .context("Failed to read response body")?
            .to_bytes();

        Ok((status, body))
    }

    /// Send a DELETE request and return the response.
    async fn delete(&self, path: &str) -> Result<(hyper::StatusCode, Bytes)> {
        let uri: Uri = format!("http://localhost{path}")
            .parse()
            .with_context(|| format!("Invalid request path: {path}"))?;

        let req = hyper::Request::builder()
            .method(hyper::Method::DELETE)
            .uri(uri)
            .header("Host", "localhost")
            .body(Full::new(Bytes::new()))
            .context("Failed to build DELETE request")?;

        let resp = self
            .client
            .request(req)
            .await
            .with_context(|| format!("DELETE {path} failed"))?;

        let status = resp.status();
        let body = resp
            .into_body()
            .collect()
            .await
            .context("Failed to read response body")?
            .to_bytes();

        Ok((status, body))
    }

    /// Parse a JSON response body, returning a contextual error on failure.
    fn parse_json<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T> {
        serde_json::from_slice(body).context("Failed to parse JSON response from daemon")
    }

    /// Check that a response has a successful (2xx) status code.  If not,
    /// extract the error message from the body and return it.
    fn check_status(status: hyper::StatusCode, body: &[u8]) -> Result<()> {
        if status.is_success() {
            return Ok(());
        }

        // Try to extract an error message from the JSON body.
        let msg = if let Ok(val) = serde_json::from_slice::<serde_json::Value>(body) {
            val.get("error")
                .and_then(|e| e.as_str())
                .unwrap_or("unknown error")
                .to_string()
        } else {
            String::from_utf8_lossy(body).into_owned()
        };

        bail!("Daemon returned {} -- {}", status, msg)
    }

    // ------------------------------------------------------------------
    // Typed API methods
    // ------------------------------------------------------------------

    /// Check if the daemon is alive and healthy.
    ///
    /// Hits `GET /health/live`.
    pub async fn health_check(&self) -> Result<bool> {
        match self.get("/health/live").await {
            Ok((status, _body)) => Ok(status.is_success()),
            // Connection refused / broken pipe => daemon is not up.
            Err(_) => Ok(false),
        }
    }

    /// Create a new deployment from a YAML spec string.
    ///
    /// `POST /api/v1/deployments` with `{"spec": "<yaml>"}`
    pub async fn create_deployment(&self, spec_yaml: &str) -> Result<serde_json::Value> {
        let body = serde_json::json!({ "spec": spec_yaml }).to_string();
        let (status, resp) = self.post_json("/api/v1/deployments", &body).await?;
        // 201 Created is the expected success code.
        if !status.is_success() {
            Self::check_status(status, &resp)?;
        }
        Self::parse_json(&resp)
    }

    /// Get deployment details by name.
    ///
    /// `GET /api/v1/deployments/{name}`
    pub async fn get_deployment(&self, name: &str) -> Result<serde_json::Value> {
        let path = format!("/api/v1/deployments/{}", urlencoding(name));
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Delete a deployment by name.
    ///
    /// `DELETE /api/v1/deployments/{name}` -- returns 204 No Content on success.
    pub async fn delete_deployment(&self, name: &str) -> Result<()> {
        let path = format!("/api/v1/deployments/{}", urlencoding(name));
        let (status, body) = self.delete(&path).await?;
        Self::check_status(status, &body)?;
        Ok(())
    }

    /// List all deployments.
    ///
    /// `GET /api/v1/deployments`
    pub async fn list_deployments(&self) -> Result<Vec<serde_json::Value>> {
        let (status, body) = self.get("/api/v1/deployments").await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// List services in a deployment.
    ///
    /// `GET /api/v1/deployments/{deployment}/services`
    pub async fn list_services(&self, deployment: &str) -> Result<Vec<serde_json::Value>> {
        let path = format!("/api/v1/deployments/{}/services", urlencoding(deployment));
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Get details for a specific service.
    ///
    /// `GET /api/v1/deployments/{deployment}/services/{service}`
    pub async fn get_service(&self, deployment: &str, service: &str) -> Result<serde_json::Value> {
        let path = format!(
            "/api/v1/deployments/{}/services/{}",
            urlencoding(deployment),
            urlencoding(service),
        );
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Scale a service to a specific replica count.
    ///
    /// `POST /api/v1/deployments/{deployment}/services/{service}/scale`
    pub async fn scale_service(
        &self,
        deployment: &str,
        service: &str,
        replicas: u32,
    ) -> Result<serde_json::Value> {
        let path = format!(
            "/api/v1/deployments/{}/services/{}/scale",
            urlencoding(deployment),
            urlencoding(service),
        );
        let body = serde_json::json!({ "replicas": replicas }).to_string();
        let (status, resp) = self.post_json(&path, &body).await?;
        Self::check_status(status, &resp)?;
        Self::parse_json(&resp)
    }

    /// Get logs for a service (non-streaming).
    ///
    /// `GET /api/v1/deployments/{deployment}/services/{service}/logs?lines=N&follow=false[&instance=ID]`
    pub async fn get_logs(
        &self,
        deployment: &str,
        service: &str,
        lines: u32,
        follow: bool,
        instance: Option<&str>,
    ) -> Result<String> {
        self.get_logs_with_instance(deployment, service, lines, follow, instance)
            .await
    }

    /// Get logs for a service, optionally filtered to a specific instance.
    ///
    /// `GET /api/v1/deployments/{deployment}/services/{service}/logs?lines=N&follow=BOOL&instance=ID`
    pub async fn get_logs_with_instance(
        &self,
        deployment: &str,
        service: &str,
        lines: u32,
        follow: bool,
        instance: Option<&str>,
    ) -> Result<String> {
        let mut path = format!(
            "/api/v1/deployments/{}/services/{}/logs?lines={}&follow={}",
            urlencoding(deployment),
            urlencoding(service),
            lines,
            follow,
        );
        if let Some(inst) = instance {
            path.push_str(&format!("&instance={}", urlencoding(inst)));
        }
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Ok(String::from_utf8_lossy(&body).into_owned())
    }

    /// Stream logs for a service via SSE (Server-Sent Events).
    ///
    /// Makes a GET request with `follow=true` and returns a receiver that
    /// yields log lines as they arrive.  The caller should read from the
    /// returned `mpsc::Receiver<String>` until it closes (stream ended or
    /// connection dropped).
    ///
    /// `GET /api/v1/deployments/{deployment}/services/{service}/logs?lines=N&follow=true[&instance=ID]`
    pub async fn get_logs_streaming(
        &self,
        deployment: &str,
        service: &str,
        lines: u32,
        instance: Option<&str>,
    ) -> Result<tokio::sync::mpsc::Receiver<String>> {
        let mut path = format!(
            "/api/v1/deployments/{}/services/{}/logs?lines={}&follow=true",
            urlencoding(deployment),
            urlencoding(service),
            lines,
        );
        if let Some(inst) = instance {
            path.push_str(&format!("&instance={}", urlencoding(inst)));
        }

        let uri: Uri = format!("http://localhost{path}")
            .parse()
            .with_context(|| format!("Invalid request path: {path}"))?;

        let req = hyper::Request::builder()
            .method(hyper::Method::GET)
            .uri(uri)
            .header("Host", "localhost")
            .header("Accept", "text/event-stream")
            .body(Full::new(Bytes::new()))
            .context("Failed to build GET request for log streaming")?;

        let resp = self
            .client
            .request(req)
            .await
            .with_context(|| format!("GET {path} (streaming) failed"))?;

        let (parts, body) = resp.into_parts();

        if !parts.status.is_success() {
            let collected = body
                .collect()
                .await
                .context("Failed to read error response body")?
                .to_bytes();
            Self::check_status(parts.status, &collected)?;
            // check_status always bails on non-success, but satisfy the compiler:
            unreachable!();
        }

        // Spawn a task to read the SSE byte stream and parse `data:` lines
        let (tx, rx) = tokio::sync::mpsc::channel::<String>(256);

        tokio::spawn(async move {
            use http_body_util::BodyStream;
            use tokio_stream::StreamExt as _;

            let mut body_stream = BodyStream::new(body);
            let mut buf = String::new();

            while let Some(frame_result) = body_stream.next().await {
                let frame = match frame_result {
                    Ok(f) => f,
                    Err(e) => {
                        debug!("SSE body stream error: {}", e);
                        break;
                    }
                };

                if let Some(data) = frame.data_ref() {
                    buf.push_str(&String::from_utf8_lossy(data));

                    // Process complete SSE lines from buffer
                    while let Some(pos) = buf.find('\n') {
                        let line = buf[..pos].to_string();
                        buf.drain(..=pos);

                        // SSE `data:` prefix
                        if let Some(payload) = line.strip_prefix("data:") {
                            let payload = payload.trim_start().to_string();
                            if tx.send(payload).await.is_err() {
                                // Receiver dropped
                                return;
                            }
                        }
                        // Ignore other SSE fields (event:, id:, retry:, comments)
                    }
                }
            }
        });

        Ok(rx)
    }

    /// List containers for a specific service in a deployment.
    ///
    /// `GET /api/v1/deployments/{deployment}/services/{service}/containers`
    pub async fn list_containers(
        &self,
        deployment: &str,
        service: &str,
    ) -> Result<Vec<serde_json::Value>> {
        let path = format!(
            "/api/v1/deployments/{}/services/{}/containers",
            urlencoding(deployment),
            urlencoding(service),
        );
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Execute a command in a service container.
    ///
    /// `POST /api/v1/deployments/{deployment}/services/{service}/exec`
    /// with `{"command": [...], "replica": N}` (replica is optional).
    ///
    /// # Arguments
    /// * `deployment` - Deployment name
    /// * `service` - Service name
    /// * `cmd` - Command and arguments to execute
    ///
    /// # Returns
    /// JSON with `exit_code`, `stdout`, and `stderr` fields.
    pub async fn exec_command(
        &self,
        deployment: &str,
        service: &str,
        cmd: &[String],
    ) -> Result<serde_json::Value> {
        self.exec_command_with_replica(deployment, service, cmd, None)
            .await
    }

    /// Execute a command in a specific replica of a service container.
    ///
    /// `POST /api/v1/deployments/{deployment}/services/{service}/exec`
    /// with `{"command": [...], "replica": N}`.
    pub async fn exec_command_with_replica(
        &self,
        deployment: &str,
        service: &str,
        cmd: &[String],
        replica: Option<u32>,
    ) -> Result<serde_json::Value> {
        let path = format!(
            "/api/v1/deployments/{}/services/{}/exec",
            urlencoding(deployment),
            urlencoding(service),
        );
        let mut payload = serde_json::json!({ "command": cmd });
        if let Some(rep) = replica {
            payload["replica"] = serde_json::json!(rep);
        }
        let body = payload.to_string();
        let (status, resp) = self.post_json(&path, &body).await?;
        Self::check_status(status, &resp)?;
        Self::parse_json(&resp)
    }

    /// Get an authentication token from the daemon.
    ///
    /// `POST /auth/token` with `{"api_key": key, "api_secret": secret}`
    pub async fn get_token(&self, api_key: &str, api_secret: &str) -> Result<serde_json::Value> {
        let body = serde_json::json!({
            "api_key": api_key,
            "api_secret": api_secret,
        })
        .to_string();
        let (status, resp) = self.post_json("/auth/token", &body).await?;
        Self::check_status(status, &resp)?;
        Self::parse_json(&resp)
    }

    /// Get the socket path this client is connected to.
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Percent-encode a path segment for safe inclusion in a URL.
///
/// This handles deployment/service names that might contain special characters.
fn urlencoding(s: &str) -> String {
    // For path segments we only need to encode a few characters.
    // Deployment and service names are typically alphanumeric + hyphens,
    // but we handle the general case.
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => out.push(ch),
            _ => {
                for byte in ch.to_string().as_bytes() {
                    out.push('%');
                    out.push_str(&format!("{byte:02X}"));
                }
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_urlencoding_passthrough() {
        assert_eq!(urlencoding("my-deployment"), "my-deployment");
        assert_eq!(urlencoding("service_v2"), "service_v2");
        assert_eq!(urlencoding("app.web"), "app.web");
    }

    #[test]
    fn test_urlencoding_special_chars() {
        assert_eq!(urlencoding("my app"), "my%20app");
        assert_eq!(urlencoding("a/b"), "a%2Fb");
    }

    #[test]
    fn test_check_status_success() {
        let result = DaemonClient::check_status(hyper::StatusCode::OK, b"{}");
        assert!(result.is_ok());
    }

    #[test]
    fn test_check_status_created() {
        let result = DaemonClient::check_status(hyper::StatusCode::CREATED, b"{}");
        assert!(result.is_ok());
    }

    #[test]
    fn test_check_status_no_content() {
        let result = DaemonClient::check_status(hyper::StatusCode::NO_CONTENT, b"");
        assert!(result.is_ok());
    }

    #[test]
    fn test_check_status_not_found_json() {
        let body = br#"{"error":"Deployment 'foo' not found"}"#;
        let result = DaemonClient::check_status(hyper::StatusCode::NOT_FOUND, body);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("404"));
        assert!(msg.contains("Deployment 'foo' not found"));
    }

    #[test]
    fn test_check_status_error_plain_text() {
        let body = b"something went wrong";
        let result = DaemonClient::check_status(hyper::StatusCode::INTERNAL_SERVER_ERROR, body);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("500"));
        assert!(msg.contains("something went wrong"));
    }

    #[test]
    fn test_parse_json_valid() {
        let body = br#"{"name":"test","status":"running"}"#;
        let val: serde_json::Value = DaemonClient::parse_json(body).unwrap();
        assert_eq!(val["name"], "test");
        assert_eq!(val["status"], "running");
    }

    #[test]
    fn test_parse_json_invalid() {
        let body = b"not json";
        let result: Result<serde_json::Value> = DaemonClient::parse_json(body);
        assert!(result.is_err());
    }

    #[test]
    fn test_find_runtime_binary_current_exe() {
        // This test verifies the logic works without panicking.
        // In CI the binary may or may not be present, so we just
        // verify the function doesn't panic.
        let _ = DaemonClient::find_runtime_binary();
    }
}
