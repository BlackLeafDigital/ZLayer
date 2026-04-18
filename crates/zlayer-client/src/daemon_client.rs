//! HTTP-over-Unix-socket client for CLI-to-daemon communication.
//!
//! Provides a typed [`DaemonClient`] that communicates with the `zlayer serve`
//! daemon via its Unix domain socket (platform-dependent path; see
//! [`default_socket_path()`]).  If the daemon is not running,
//! [`DaemonClient::connect`] will auto-start it and wait with exponential backoff
//! until the socket becomes available.

use std::collections::HashMap;
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
use serde::{Deserialize, Serialize};
use tokio::net::UnixStream;
use tracing::{debug, info};
use zlayer_api::handlers::auth::{BootstrapRequest, LoginRequest, LoginResponse, UserView};
use zlayer_api::handlers::containers::ContainerExecResponse;
use zlayer_api::handlers::images::{ImageInfoDto, PruneResultDto, PullImageResponse};
use zlayer_api::handlers::secrets::SecretMetadataResponse;
use zlayer_api::handlers::users::{CreateUserRequest, SetPasswordRequest, UpdateUserRequest};
use zlayer_api::storage::{StoredEnvironment, StoredVariable};

/// Default path for the daemon Unix socket.
///
/// On macOS: `~/.zlayer/run/zlayer.sock`.
/// On Linux: `/var/run/zlayer.sock`.
#[must_use]
pub fn default_socket_path() -> String {
    zlayer_paths::ZLayerDirs::default_socket_path()
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

#[allow(clippy::missing_errors_doc)]
impl DaemonClient {
    // ------------------------------------------------------------------
    // Construction / connection
    // ------------------------------------------------------------------

    /// Connect to the daemon, auto-starting it if it is not already running.
    ///
    /// Checks for the Unix socket at [`default_socket_path()`].  If the socket
    /// does not exist and no daemon PID is found, `zlayer serve --daemon`
    /// is spawned and the function polls the socket with exponential backoff
    /// (100 ms -> 200 ms -> 400 ms -> ... -> 1 s, max 20 attempts, ~10 s total).
    pub async fn connect() -> Result<Self> {
        Self::connect_to(&default_socket_path()).await
    }

    /// Try to connect to a running daemon without auto-starting.
    ///
    /// Returns `Ok(Some(client))` if the daemon is running and healthy,
    /// `Ok(None)` if the daemon is not running, or `Err` on unexpected errors.
    pub async fn try_connect() -> Result<Option<Self>> {
        Self::try_connect_to(&default_socket_path()).await
    }

    /// Like [`try_connect`](Self::try_connect) but with a custom socket path.
    pub async fn try_connect_to(socket_path: impl AsRef<Path>) -> Result<Option<Self>> {
        let socket_path = socket_path.as_ref().to_path_buf();

        if socket_path.exists() {
            match Self::try_build(&socket_path).await {
                Ok(client) => Ok(Some(client)),
                Err(_) => Ok(None),
            }
        } else {
            Ok(None)
        }
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
        eprintln!("ZLayer daemon not running. Starting...");
        Self::auto_start_daemon(&socket_path).await?;

        Self::try_build(&socket_path)
            .await
            .context("Cannot connect to ZLayer daemon. Run 'zlayer status' for details.")
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

    /// Resolve the path to the current executable (since zlayer is now a
    /// single binary, we just re-invoke ourselves).
    fn find_self_binary() -> Result<PathBuf> {
        std::env::current_exe().context("Failed to resolve current executable path")
    }

    /// Spawn `zlayer serve --daemon` and poll the socket with
    /// exponential backoff until the daemon is reachable or we give up.
    async fn auto_start_daemon(socket_path: &Path) -> Result<()> {
        let runtime_bin = Self::find_self_binary()?;

        debug!(binary = %runtime_bin.display(), "Spawning daemon");

        let mut cmd = std::process::Command::new(&runtime_bin);

        // Ensure the child process knows the correct data directory.
        // If ZLAYER_DATA_DIR is already set in the environment it will be
        // inherited automatically (clap picks it up via `env = "ZLAYER_DATA_DIR"`).
        // Otherwise, explicitly pass --data-dir so the child doesn't fall back
        // to /var/lib/zlayer when $HOME is unavailable (e.g. launchd context).
        if std::env::var_os("ZLAYER_DATA_DIR").is_none() {
            let data_dir = zlayer_paths::ZLayerDirs::detect_data_dir();
            cmd.arg("--data-dir").arg(&data_dir);
        }

        cmd.env("ZLAYER_SPAWNER_PID", std::process::id().to_string());

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

        let log_dir = zlayer_paths::ZLayerDirs::default_log_dir();
        let log_path = log_dir.join("daemon.log");
        let timeout_secs = 10;
        eprintln!("Failed to start ZLayer daemon after {timeout_secs}s.");
        eprintln!("  Check logs: {}", log_path.display());
        eprintln!("  Start manually: zlayer serve");
        bail!(
            "Timed out waiting for daemon to start after {} attempts (~{} s). \
             Socket path: {}",
            max_attempts,
            timeout_secs,
            socket_path.display()
        )
    }

    // ------------------------------------------------------------------
    // Low-level HTTP helpers
    // ------------------------------------------------------------------

    /// If the user has a saved session (`~/.zlayer/session.json`) with an
    /// unexpired token, attach it as an `Authorization: Bearer <token>` header.
    /// When the file is absent or the token has expired, this is a no-op -- the
    /// daemon's Unix-socket middleware will inject the local-admin token.
    fn apply_session_auth(
        mut builder: hyper::http::request::Builder,
    ) -> hyper::http::request::Builder {
        match crate::session::read_session() {
            Ok(Some(session)) if !session.is_expired() => {
                builder = builder.header(
                    hyper::header::AUTHORIZATION,
                    format!("Bearer {}", session.token),
                );
            }
            Ok(Some(expired)) => {
                tracing::debug!(
                    email = %expired.email,
                    "Session file present but token has expired; ignoring"
                );
            }
            Ok(None) => {}
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "Failed to read session file; proceeding without bearer"
                );
            }
        }
        builder
    }

    /// Send a GET request and return the response body as bytes.
    async fn get(&self, path: &str) -> Result<(hyper::StatusCode, Bytes)> {
        let uri: Uri = format!("http://localhost{path}")
            .parse()
            .with_context(|| format!("Invalid request path: {path}"))?;

        let builder = hyper::Request::builder()
            .method(hyper::Method::GET)
            .uri(uri)
            .header("Host", "localhost");
        let builder = Self::apply_session_auth(builder);
        let req = builder
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

        let builder = hyper::Request::builder()
            .method(hyper::Method::POST)
            .uri(uri)
            .header("Host", "localhost")
            .header("Content-Type", "application/json");
        let builder = Self::apply_session_auth(builder);
        let req = builder
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

        let builder = hyper::Request::builder()
            .method(hyper::Method::DELETE)
            .uri(uri)
            .header("Host", "localhost");
        let builder = Self::apply_session_auth(builder);
        let req = builder
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

    /// Send a POST request with a plain-text body and return the response.
    ///
    /// Used for endpoints whose body is not JSON (e.g. dotenv bulk imports).
    async fn post_text(&self, path: &str, body: &str) -> Result<(hyper::StatusCode, Bytes)> {
        let uri: Uri = format!("http://localhost{path}")
            .parse()
            .with_context(|| format!("Invalid request path: {path}"))?;

        let builder = hyper::Request::builder()
            .method(hyper::Method::POST)
            .uri(uri)
            .header("Host", "localhost")
            .header("Content-Type", "text/plain; charset=utf-8");
        let builder = Self::apply_session_auth(builder);
        let req = builder
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

    /// Send a PATCH request with a JSON body and return the response.
    async fn patch_json(&self, path: &str, body: &str) -> Result<(hyper::StatusCode, Bytes)> {
        let uri: Uri = format!("http://localhost{path}")
            .parse()
            .with_context(|| format!("Invalid request path: {path}"))?;

        let builder = hyper::Request::builder()
            .method(hyper::Method::PATCH)
            .uri(uri)
            .header("Host", "localhost")
            .header("Content-Type", "application/json");
        let builder = Self::apply_session_auth(builder);
        let req = builder
            .body(Full::new(Bytes::from(body.to_owned())))
            .context("Failed to build PATCH request")?;

        let resp = self
            .client
            .request(req)
            .await
            .with_context(|| format!("PATCH {path} failed"))?;

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

    /// Decode a response body as a UTF-8 `String`, returning a contextual
    /// error on invalid UTF-8.  Used for endpoints (such as container logs)
    /// that return plain text rather than JSON.
    fn decode_text_body(body: &[u8]) -> Result<String> {
        String::from_utf8(body.to_vec()).context("Container logs body was not valid UTF-8")
    }

    /// Check that a response has a successful (2xx) status code.  If not,
    /// extract the error message from the body and return it with actionable
    /// guidance.
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

        match status.as_u16() {
            404 => bail!("404 Not Found: {msg}. Check deployment name with 'zlayer ps'."),
            500 => {
                let log_dir = zlayer_paths::ZLayerDirs::default_log_dir();
                let log_path = log_dir.join("daemon.log");
                bail!(
                    "500 Internal Server Error: {}. Check logs at {}",
                    msg,
                    log_path.display()
                )
            }
            _ => bail!("Daemon returned {status} -- {msg}"),
        }
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

    /// Check if the daemon is ready (fully initialized).
    ///
    /// Hits `GET /health/ready` and returns the parsed response.
    pub async fn health_ready(&self) -> Result<serde_json::Value> {
        let (status, body) = self.get("/health/ready").await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
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

    /// Watch deployment orchestration progress via SSE.
    ///
    /// `GET /api/v1/deployments/{name}/events`
    ///
    /// Returns a receiver that yields `(event_type, data_json)` tuples as SSE
    /// events arrive. The receiver closes when the server ends the stream
    /// (deployment reached terminal state) or the connection drops.
    pub async fn watch_deployment(
        &self,
        name: &str,
    ) -> Result<tokio::sync::mpsc::Receiver<(String, String)>> {
        let path = format!("/api/v1/deployments/{}/events", urlencoding(name));

        let uri: Uri = format!("http://localhost{path}")
            .parse()
            .with_context(|| format!("Invalid request path: {path}"))?;

        let req = hyper::Request::builder()
            .method(hyper::Method::GET)
            .uri(uri)
            .header("Host", "localhost")
            .header("Accept", "text/event-stream")
            .body(Full::new(Bytes::new()))
            .context("Failed to build GET request for deployment event streaming")?;

        let resp = self
            .client
            .request(req)
            .await
            .with_context(|| format!("GET {path} (SSE) failed"))?;

        let (parts, body) = resp.into_parts();

        if !parts.status.is_success() {
            let collected = body
                .collect()
                .await
                .context("Failed to read error response body")?
                .to_bytes();
            Self::check_status(parts.status, &collected)?;
            unreachable!();
        }

        // Spawn a task that parses SSE frames from the byte stream.
        // Each complete SSE message yields (event_type, data) on the channel.
        let (tx, rx) = tokio::sync::mpsc::channel::<(String, String)>(256);

        tokio::spawn(async move {
            use http_body_util::BodyStream;
            use tokio_stream::StreamExt as _;

            let mut body_stream = BodyStream::new(body);
            let mut buf = String::new();
            let mut current_event = String::new();
            let mut current_data = String::new();

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

                    // Process complete lines from buffer
                    while let Some(pos) = buf.find('\n') {
                        let line = buf[..pos].to_string();
                        buf.drain(..=pos);

                        if line.is_empty() {
                            // Blank line = end of SSE message
                            if !current_data.is_empty() {
                                let event_type = if current_event.is_empty() {
                                    "message".to_string()
                                } else {
                                    std::mem::take(&mut current_event)
                                };
                                let data = std::mem::take(&mut current_data);
                                if tx.send((event_type, data)).await.is_err() {
                                    return; // receiver dropped
                                }
                            }
                            current_event.clear();
                            current_data.clear();
                        } else if let Some(value) = line.strip_prefix("event:") {
                            current_event = value.trim().to_string();
                        } else if let Some(value) = line.strip_prefix("data:") {
                            current_data = value.trim_start().to_string();
                        }
                        // Ignore other SSE fields (id:, retry:, comments)
                    }
                }
            }

            // Flush any remaining partial message
            if !current_data.is_empty() {
                let event_type = if current_event.is_empty() {
                    "message".to_string()
                } else {
                    current_event
                };
                let _ = tx.send((event_type, current_data)).await;
            }
        });

        Ok(rx)
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
            use std::fmt::Write;
            let _ = write!(path, "&instance={}", urlencoding(inst));
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
            use std::fmt::Write;
            let _ = write!(path, "&instance={}", urlencoding(inst));
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
    #[must_use]
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    // ------------------------------------------------------------------
    // Image management
    // ------------------------------------------------------------------

    /// List cached images from the daemon.
    ///
    /// `GET /api/v1/images`
    pub async fn list_images(&self) -> Result<Vec<ImageInfoDto>> {
        let (status, body) = self.get("/api/v1/images").await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Remove a cached image from the daemon.
    ///
    /// `DELETE /api/v1/images/{image}?force=BOOL` -- returns 204 No Content.
    pub async fn remove_image(&self, image: &str, force: bool) -> Result<()> {
        let path = format!("/api/v1/images/{}?force={}", urlencoding(image), force);
        let (status, body) = self.delete(&path).await?;
        Self::check_status(status, &body)?;
        Ok(())
    }

    /// Prune dangling / unused cached images from the daemon.
    ///
    /// `POST /api/v1/system/prune`
    pub async fn prune_images(&self) -> Result<PruneResultDto> {
        // Send an empty JSON object as body to keep Content-Type consistent.
        let (status, body) = self.post_json("/api/v1/system/prune", "{}").await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Push an image to a remote registry.
    ///
    /// `POST /api/v1/images/push`
    pub async fn push_image(
        &self,
        image: &str,
        username: Option<&str>,
        password: Option<&str>,
    ) -> Result<serde_json::Value> {
        let payload = serde_json::json!({
            "image": image,
            "username": username,
            "password": password,
        });
        let (status, body) = self
            .post_json("/api/v1/images/push", &payload.to_string())
            .await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Inspect an image — retrieve manifest and configuration details.
    ///
    /// `GET /api/v1/images/{image}/inspect`
    pub async fn inspect_image(&self, image: &str) -> Result<serde_json::Value> {
        let path = format!("/api/v1/images/{}/inspect", urlencoding(image));
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Ask the daemon to pull an OCI image reference into its local cache.
    ///
    /// `POST /api/v1/images/pull` with
    /// `{"reference": "<ref>", "pull_policy": "always"|"if_not_present"|"never"}`.
    ///
    /// `pull_policy` is optional and defaults to `"always"` server-side when
    /// omitted. Returns the canonical reference plus the best-effort digest
    /// and on-disk size reported by the runtime.
    pub async fn pull_image_from_server(
        &self,
        reference: &str,
        pull_policy: Option<&str>,
    ) -> Result<PullImageResponse> {
        let mut payload = serde_json::json!({ "reference": reference });
        if let Some(policy) = pull_policy {
            payload["pull_policy"] = serde_json::json!(policy);
        }
        let (status, body) = self
            .post_json("/api/v1/images/pull", &payload.to_string())
            .await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Create a new reference (`target`) pointing at an already-cached
    /// image (`source`). Matches Docker-compat `docker tag` semantics.
    ///
    /// `POST /api/v1/images/tag` with `{"source": "<ref>", "target": "<ref>"}`.
    /// Returns `Ok(())` on 204 No Content.
    pub async fn tag_image(&self, source: &str, target: &str) -> Result<()> {
        let payload = serde_json::json!({
            "source": source,
            "target": target,
        });
        let (status, body) = self
            .post_json("/api/v1/images/tag", &payload.to_string())
            .await?;
        Self::check_status(status, &body)?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Container management
    // ------------------------------------------------------------------

    /// List all containers from the daemon.
    ///
    /// `GET /api/v1/containers`
    pub async fn get_all_containers(&self) -> Result<serde_json::Value> {
        let (status, body) = self.get("/api/v1/containers").await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Get detailed information about a single container.
    ///
    /// `GET /api/v1/containers/{id}`
    pub async fn get_container(&self, id: &str) -> Result<serde_json::Value> {
        let path = format!("/api/v1/containers/{}", urlencoding(id));
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Delete a container.
    ///
    /// `DELETE /api/v1/containers/{id}?force=BOOL`
    pub async fn delete_container(&self, id: &str, force: bool) -> Result<()> {
        let path = format!("/api/v1/containers/{}?force={}", urlencoding(id), force);
        let (status, body) = self.delete(&path).await?;
        Self::check_status(status, &body)?;
        Ok(())
    }

    /// Get logs for a container.
    ///
    /// `GET /api/v1/containers/{id}/logs[?tail=N]`
    ///
    /// The daemon returns the non-follow log response as plain UTF-8 text
    /// (entries joined by newlines), so the body is decoded as a `String`
    /// rather than parsed as JSON.
    pub async fn get_container_logs(&self, id: &str, tail: Option<u32>) -> Result<String> {
        let mut path = format!("/api/v1/containers/{}/logs", urlencoding(id));
        if let Some(n) = tail {
            use std::fmt::Write;
            let _ = write!(path, "?tail={n}");
        }
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::decode_text_body(&body)
    }

    /// Get resource statistics for a container.
    ///
    /// `GET /api/v1/containers/{id}/stats`
    pub async fn get_container_stats(&self, id: &str) -> Result<serde_json::Value> {
        let path = format!("/api/v1/containers/{}/stats", urlencoding(id));
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Stop a running standalone container.
    ///
    /// `POST /api/v1/containers/{id}/stop` with `{"timeout": <secs>}`.
    /// `timeout` is the graceful-shutdown window in seconds before the
    /// runtime force-kills the container; the server defaults to 30s when
    /// omitted. Returns `Ok(())` on 204 No Content.
    pub async fn stop_container(&self, id: &str, timeout: Option<u64>) -> Result<()> {
        let path = format!("/api/v1/containers/{}/stop", urlencoding(id));
        let payload = match timeout {
            Some(t) => serde_json::json!({ "timeout": t }),
            None => serde_json::json!({}),
        };
        let (status, body) = self.post_json(&path, &payload.to_string()).await?;
        Self::check_status(status, &body)?;
        Ok(())
    }

    /// Start a previously-created standalone container.
    ///
    /// `POST /api/v1/containers/{id}/start` with an empty body.
    /// Returns `Ok(())` on 204 No Content.
    pub async fn start_container(&self, id: &str) -> Result<()> {
        let path = format!("/api/v1/containers/{}/start", urlencoding(id));
        let (status, body) = self.post_json(&path, "{}").await?;
        Self::check_status(status, &body)?;
        Ok(())
    }

    /// Restart a standalone container (stop then start).
    ///
    /// `POST /api/v1/containers/{id}/restart` with `{"timeout": <secs>}`.
    /// `timeout` is the graceful-shutdown window in seconds before force-kill;
    /// the server defaults to 30s when omitted. Returns `Ok(())` on 204.
    pub async fn restart_container(&self, id: &str, timeout: Option<u64>) -> Result<()> {
        let path = format!("/api/v1/containers/{}/restart", urlencoding(id));
        let payload = match timeout {
            Some(t) => serde_json::json!({ "timeout": t }),
            None => serde_json::json!({}),
        };
        let (status, body) = self.post_json(&path, &payload.to_string()).await?;
        Self::check_status(status, &body)?;
        Ok(())
    }

    /// Send a signal to a running standalone container.
    ///
    /// `POST /api/v1/containers/{id}/kill` with `{"signal": "<name>"}`.
    /// `signal` accepts either the `SIG`-prefixed or bare form (e.g.
    /// `"SIGTERM"` or `"TERM"`). When omitted, the server defaults to
    /// `SIGKILL`. Returns `Ok(())` on 204 No Content.
    pub async fn kill_container(&self, id: &str, signal: Option<&str>) -> Result<()> {
        let path = format!("/api/v1/containers/{}/kill", urlencoding(id));
        let payload = match signal {
            Some(s) => serde_json::json!({ "signal": s }),
            None => serde_json::json!({}),
        };
        let (status, body) = self.post_json(&path, &payload.to_string()).await?;
        Self::check_status(status, &body)?;
        Ok(())
    }

    /// Execute a command inside a standalone container and wait for it to
    /// finish.
    ///
    /// `POST /api/v1/containers/{id}/exec` with `{"command": [...]}`.
    /// Returns a [`ContainerExecResponse`] with `exit_code`, `stdout`, and
    /// `stderr`.
    ///
    /// Note: the daemon's container exec endpoint is non-interactive -- it
    /// runs the command to completion and buffers the output. It does not
    /// accept `tty` or `interactive` flags. For attached/TTY exec against a
    /// managed service, use
    /// [`exec_command`](Self::exec_command) instead.
    pub async fn exec_in_container(
        &self,
        id: &str,
        cmd: Vec<String>,
    ) -> Result<ContainerExecResponse> {
        let path = format!("/api/v1/containers/{}/exec", urlencoding(id));
        let payload = serde_json::json!({ "command": cmd });
        let (status, body) = self.post_json(&path, &payload.to_string()).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    // ------------------------------------------------------------------
    // Tunnel management
    // ------------------------------------------------------------------

    /// List all tunnels.
    ///
    /// `GET /api/v1/tunnels`
    pub async fn list_tunnels(&self) -> Result<serde_json::Value> {
        let (status, body) = self.get("/api/v1/tunnels").await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Revoke (delete) a tunnel by ID.
    ///
    /// `DELETE /api/v1/tunnels/{id}`
    pub async fn revoke_tunnel(&self, id: &str) -> Result<serde_json::Value> {
        let path = format!("/api/v1/tunnels/{}", urlencoding(id));
        let (status, body) = self.delete(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Get tunnel status by ID.
    ///
    /// `GET /api/v1/tunnels/{id}/status`
    pub async fn get_tunnel_status(&self, id: &str) -> Result<serde_json::Value> {
        let path = format!("/api/v1/tunnels/{}/status", urlencoding(id));
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Create a node-to-node tunnel.
    ///
    /// `POST /api/v1/tunnels/node`
    pub async fn add_node_tunnel(
        &self,
        name: &str,
        from_node: &str,
        to_node: &str,
        local_port: u16,
        remote_port: u16,
        expose: &str,
    ) -> Result<serde_json::Value> {
        let payload = serde_json::json!({
            "name": name,
            "from_node": from_node,
            "to_node": to_node,
            "local_port": local_port,
            "remote_port": remote_port,
            "expose": expose,
        });
        let (status, body) = self
            .post_json("/api/v1/tunnels/node", &payload.to_string())
            .await?;
        if !status.is_success() {
            Self::check_status(status, &body)?;
        }
        Self::parse_json(&body)
    }

    /// Remove a node-to-node tunnel by name.
    ///
    /// `DELETE /api/v1/tunnels/node/{name}`
    pub async fn remove_node_tunnel(&self, name: &str) -> Result<serde_json::Value> {
        let path = format!("/api/v1/tunnels/node/{}", urlencoding(name));
        let (status, body) = self.delete(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    // ------------------------------------------------------------------
    // Secrets management
    // ------------------------------------------------------------------

    /// List all secrets from the daemon.
    ///
    /// `GET /api/v1/secrets`
    pub async fn list_secrets(&self) -> Result<Vec<serde_json::Value>> {
        let (status, body) = self.get("/api/v1/secrets").await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Create or update a secret.
    ///
    /// `POST /api/v1/secrets` with `{"name": "<name>", "value": "<value>"}`
    pub async fn create_secret(&self, name: &str, value: &str) -> Result<serde_json::Value> {
        let payload = serde_json::json!({ "name": name, "value": value });
        let (status, body) = self
            .post_json("/api/v1/secrets", &payload.to_string())
            .await?;
        if !status.is_success() {
            Self::check_status(status, &body)?;
        }
        Self::parse_json(&body)
    }

    /// Get metadata for a specific secret (value is never returned).
    ///
    /// `GET /api/v1/secrets/{name}`
    pub async fn get_secret(&self, name: &str) -> Result<serde_json::Value> {
        let path = format!("/api/v1/secrets/{}", urlencoding(name));
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Delete a secret by name.
    ///
    /// `DELETE /api/v1/secrets/{name}` -- returns 204 No Content on success.
    pub async fn delete_secret(&self, name: &str) -> Result<()> {
        let path = format!("/api/v1/secrets/{}", urlencoding(name));
        let (status, body) = self.delete(&path).await?;
        Self::check_status(status, &body)?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Network management
    // ------------------------------------------------------------------

    /// List all networks.
    ///
    /// `GET /api/v1/networks`
    pub async fn list_networks(&self) -> Result<Vec<serde_json::Value>> {
        let (status, body) = self.get("/api/v1/networks").await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Get details for a specific network by name.
    ///
    /// `GET /api/v1/networks/{name}`
    pub async fn get_network(&self, name: &str) -> Result<serde_json::Value> {
        let path = format!("/api/v1/networks/{}", urlencoding(name));
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Create a new network.
    ///
    /// `POST /api/v1/networks` with `{"name": "<name>"}`
    pub async fn create_network(&self, name: &str) -> Result<serde_json::Value> {
        let payload = serde_json::json!({ "name": name });
        let (status, body) = self
            .post_json("/api/v1/networks", &payload.to_string())
            .await?;
        if !status.is_success() {
            Self::check_status(status, &body)?;
        }
        Self::parse_json(&body)
    }

    /// Delete a network by name.
    ///
    /// `DELETE /api/v1/networks/{name}` -- returns 204 No Content on success.
    pub async fn delete_network(&self, name: &str) -> Result<()> {
        let path = format!("/api/v1/networks/{}", urlencoding(name));
        let (status, body) = self.delete(&path).await?;
        Self::check_status(status, &body)?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Volume management
    // ------------------------------------------------------------------

    /// List all volumes.
    ///
    /// `GET /api/v1/volumes`
    pub async fn list_volumes(&self) -> Result<Vec<serde_json::Value>> {
        let (status, body) = self.get("/api/v1/volumes").await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Delete a volume by name.
    ///
    /// `DELETE /api/v1/volumes/{name}?force={force}` -- returns 204 No Content on success.
    pub async fn delete_volume(&self, name: &str, force: bool) -> Result<()> {
        let path = format!("/api/v1/volumes/{}?force={}", urlencoding(name), force,);
        let (status, body) = self.delete(&path).await?;
        Self::check_status(status, &body)?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Overlay network
    // ------------------------------------------------------------------

    /// Get overlay network status.
    ///
    /// `GET /api/v1/overlay/status`
    pub async fn get_overlay_status(&self) -> Result<serde_json::Value> {
        let (status, body) = self.get("/api/v1/overlay/status").await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Get overlay peer list.
    ///
    /// `GET /api/v1/overlay/peers`
    pub async fn get_overlay_peers(&self) -> Result<serde_json::Value> {
        let (status, body) = self.get("/api/v1/overlay/peers").await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Get overlay DNS entries.
    ///
    /// `GET /api/v1/overlay/dns`
    pub async fn get_overlay_dns(&self) -> Result<serde_json::Value> {
        let (status, body) = self.get("/api/v1/overlay/dns").await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    // ------------------------------------------------------------------
    // Job management
    // ------------------------------------------------------------------

    /// List all jobs.
    ///
    /// `GET /api/v1/jobs`
    pub async fn list_jobs(&self) -> Result<serde_json::Value> {
        let (status, body) = self.get("/api/v1/jobs").await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Trigger a job execution.
    ///
    /// `POST /api/v1/deployments/{deployment}/jobs/{job}/trigger`
    pub async fn trigger_job(&self, deployment: &str, job: &str) -> Result<serde_json::Value> {
        let path = format!(
            "/api/v1/deployments/{}/jobs/{}/trigger",
            urlencoding(deployment),
            urlencoding(job),
        );
        let (status, body) = self.post_json(&path, "{}").await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Get job status.
    ///
    /// `GET /api/v1/deployments/{deployment}/jobs/{job}`
    pub async fn get_job_status(&self, deployment: &str, job: &str) -> Result<serde_json::Value> {
        let path = format!(
            "/api/v1/deployments/{}/jobs/{}",
            urlencoding(deployment),
            urlencoding(job),
        );
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// List all cron jobs.
    ///
    /// `GET /api/v1/cron`
    pub async fn list_cron_jobs(&self) -> Result<serde_json::Value> {
        let (status, body) = self.get("/api/v1/cron").await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Get cron job status.
    ///
    /// `GET /api/v1/deployments/{deployment}/cron/{cron}`
    pub async fn get_cron_status(&self, deployment: &str, cron: &str) -> Result<serde_json::Value> {
        let path = format!(
            "/api/v1/deployments/{}/cron/{}",
            urlencoding(deployment),
            urlencoding(cron),
        );
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    // ------------------------------------------------------------------
    // Auth
    // ------------------------------------------------------------------

    /// POST /auth/bootstrap -- create the first admin user. Returns the user view and csrf token.
    pub async fn auth_bootstrap(&self, req: &BootstrapRequest) -> Result<LoginResponse> {
        let body = serde_json::to_string(req).context("Failed to serialise BootstrapRequest")?;
        let (status, resp_body) = self.post_json("/auth/bootstrap", &body).await?;
        Self::check_status(status, &resp_body)?;
        Self::parse_json(&resp_body)
    }

    /// POST /auth/login -- authenticate by email+password. Returns the session cookie
    /// via HTTP headers (discarded here -- CLI stores the separately-obtained token from
    /// /auth/token) and the login response body.
    pub async fn auth_login(&self, req: &LoginRequest) -> Result<LoginResponse> {
        let body = serde_json::to_string(req).context("Failed to serialise LoginRequest")?;
        let (status, resp_body) = self.post_json("/auth/login", &body).await?;
        Self::check_status(status, &resp_body)?;
        Self::parse_json(&resp_body)
    }

    /// POST /auth/logout -- invalidate the session cookie on the server.
    /// CLI-side session-file deletion happens separately in commands/auth.rs.
    pub async fn auth_logout(&self) -> Result<()> {
        let (status, resp_body) = self.post_json("/auth/logout", "").await?;
        // 204 No Content is success.
        if status.is_success() {
            Ok(())
        } else {
            Self::check_status(status, &resp_body)
        }
    }

    /// GET /auth/me -- current user from the attached session/Bearer auth.
    pub async fn auth_whoami(&self) -> Result<UserView> {
        let (status, body) = self.get("/auth/me").await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// POST /auth/token -- exchange email+password for a JWT. Used by `zlayer auth
    /// login` to persist a token in the session file. Returns a raw JSON value so
    /// the caller can extract `access_token` without coupling to zlayer-api types
    /// that aren't re-exported there.
    pub async fn auth_token(&self, api_key: &str, api_secret: &str) -> Result<serde_json::Value> {
        let body = serde_json::json!({
            "api_key": api_key,
            "api_secret": api_secret,
        })
        .to_string();
        let (status, resp_body) = self.post_json("/auth/token", &body).await?;
        Self::check_status(status, &resp_body)?;
        Self::parse_json(&resp_body)
    }

    // ------------------------------------------------------------------
    // Users
    // ------------------------------------------------------------------

    /// GET /api/v1/users -- list all users (admin).
    pub async fn list_users(&self) -> Result<Vec<UserView>> {
        let (status, body) = self.get("/api/v1/users").await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// POST /api/v1/users -- create a new user (admin).
    pub async fn create_user(&self, req: &CreateUserRequest) -> Result<UserView> {
        let body = serde_json::to_string(req).context("Failed to serialise CreateUserRequest")?;
        let (status, resp_body) = self.post_json("/api/v1/users", &body).await?;
        Self::check_status(status, &resp_body)?;
        Self::parse_json(&resp_body)
    }

    /// GET /api/v1/users/{id}.
    pub async fn get_user(&self, id: &str) -> Result<UserView> {
        let path = format!("/api/v1/users/{}", urlencoding(id));
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// PATCH /api/v1/users/{id} -- update user fields.
    pub async fn update_user(&self, id: &str, req: &UpdateUserRequest) -> Result<UserView> {
        let body = serde_json::to_string(req).context("Failed to serialise UpdateUserRequest")?;
        let path = format!("/api/v1/users/{}", urlencoding(id));
        let (status, resp_body) = self.patch_json(&path, &body).await?;
        Self::check_status(status, &resp_body)?;
        Self::parse_json(&resp_body)
    }

    /// DELETE /api/v1/users/{id}.
    pub async fn delete_user(&self, id: &str) -> Result<()> {
        let path = format!("/api/v1/users/{}", urlencoding(id));
        let (status, body) = self.delete(&path).await?;
        if status.is_success() {
            Ok(())
        } else {
            Self::check_status(status, &body)
        }
    }

    /// POST /api/v1/users/{id}/password -- set a user's password (admin or self-service).
    pub async fn set_user_password(&self, id: &str, req: &SetPasswordRequest) -> Result<()> {
        let body = serde_json::to_string(req).context("Failed to serialise SetPasswordRequest")?;
        let path = format!("/api/v1/users/{}/password", urlencoding(id));
        let (status, resp_body) = self.post_json(&path, &body).await?;
        if status.is_success() {
            Ok(())
        } else {
            Self::check_status(status, &resp_body)
        }
    }

    // ------------------------------------------------------------------
    // Environments
    // ------------------------------------------------------------------

    /// List environments, optionally filtered by project id.
    ///
    /// `GET /api/v1/environments?project={id}` (omit `project` for globals,
    /// pass `*` to list every environment across projects + globals).
    pub async fn list_environments(
        &self,
        project_id: Option<&str>,
    ) -> Result<Vec<StoredEnvironment>> {
        let path = match project_id {
            Some(p) => format!("/api/v1/environments?project={}", urlencoding(p)),
            None => "/api/v1/environments".to_string(),
        };
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Create a new environment.
    ///
    /// `POST /api/v1/environments` with `{ name, project_id?, description? }`.
    pub async fn create_environment(
        &self,
        name: &str,
        project_id: Option<&str>,
        description: Option<&str>,
    ) -> Result<StoredEnvironment> {
        let body = serde_json::json!({
            "name": name,
            "project_id": project_id,
            "description": description,
        })
        .to_string();
        let (status, resp) = self.post_json("/api/v1/environments", &body).await?;
        if !status.is_success() {
            Self::check_status(status, &resp)?;
        }
        Self::parse_json(&resp)
    }

    /// Fetch a single environment by id.
    ///
    /// `GET /api/v1/environments/{id}`.
    pub async fn get_environment(&self, id: &str) -> Result<StoredEnvironment> {
        let path = format!("/api/v1/environments/{}", urlencoding(id));
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Update an environment's name and/or description.
    ///
    /// `PATCH /api/v1/environments/{id}` with `{ name?, description? }`.
    pub async fn update_environment(
        &self,
        id: &str,
        name: Option<&str>,
        description: Option<&str>,
    ) -> Result<StoredEnvironment> {
        let body = serde_json::json!({
            "name": name,
            "description": description,
        })
        .to_string();
        let path = format!("/api/v1/environments/{}", urlencoding(id));
        let (status, resp) = self.patch_json(&path, &body).await?;
        Self::check_status(status, &resp)?;
        Self::parse_json(&resp)
    }

    /// Delete an environment.
    ///
    /// `DELETE /api/v1/environments/{id}` — 204 on success, 409 if the
    /// environment still has secrets.
    pub async fn delete_environment(&self, id: &str) -> Result<()> {
        let path = format!("/api/v1/environments/{}", urlencoding(id));
        let (status, body) = self.delete(&path).await?;
        Self::check_status(status, &body)?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Variables
    // ------------------------------------------------------------------

    /// List variables, optionally filtered by scope.
    ///
    /// `GET /api/v1/variables?scope={id}` (omit `scope` for globals).
    pub async fn list_variables(&self, scope: Option<&str>) -> Result<Vec<StoredVariable>> {
        let path = match scope {
            Some(s) => format!("/api/v1/variables?scope={}", urlencoding(s)),
            None => "/api/v1/variables".to_string(),
        };
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Create a new variable.
    ///
    /// `POST /api/v1/variables` with `{ name, value, scope? }`.
    pub async fn create_variable(
        &self,
        name: &str,
        value: &str,
        scope: Option<&str>,
    ) -> Result<StoredVariable> {
        let body = serde_json::json!({
            "name": name,
            "value": value,
            "scope": scope,
        })
        .to_string();
        let (status, resp) = self.post_json("/api/v1/variables", &body).await?;
        if !status.is_success() {
            Self::check_status(status, &resp)?;
        }
        Self::parse_json(&resp)
    }

    /// Fetch a single variable by id.
    ///
    /// `GET /api/v1/variables/{id}`.
    pub async fn get_variable(&self, id: &str) -> Result<StoredVariable> {
        let path = format!("/api/v1/variables/{}", urlencoding(id));
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Update a variable's name and/or value.
    ///
    /// `PATCH /api/v1/variables/{id}` with `{ name?, value? }`.
    pub async fn update_variable(
        &self,
        id: &str,
        name: Option<&str>,
        value: Option<&str>,
    ) -> Result<StoredVariable> {
        let body = serde_json::json!({
            "name": name,
            "value": value,
        })
        .to_string();
        let path = format!("/api/v1/variables/{}", urlencoding(id));
        let (status, resp) = self.patch_json(&path, &body).await?;
        Self::check_status(status, &resp)?;
        Self::parse_json(&resp)
    }

    /// Delete a variable.
    ///
    /// `DELETE /api/v1/variables/{id}` -- 204 on success.
    pub async fn delete_variable(&self, id: &str) -> Result<()> {
        let path = format!("/api/v1/variables/{}", urlencoding(id));
        let (status, body) = self.delete(&path).await?;
        Self::check_status(status, &body)?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Projects
    // ------------------------------------------------------------------

    /// List all projects.
    ///
    /// `GET /api/v1/projects`
    pub async fn list_projects(&self) -> Result<Vec<serde_json::Value>> {
        let (status, body) = self.get("/api/v1/projects").await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Create a new project.
    ///
    /// `POST /api/v1/projects`
    pub async fn create_project(&self, body: &serde_json::Value) -> Result<serde_json::Value> {
        let (status, resp) = self
            .post_json("/api/v1/projects", &body.to_string())
            .await?;
        if !status.is_success() {
            Self::check_status(status, &resp)?;
        }
        Self::parse_json(&resp)
    }

    /// Get a project by id.
    ///
    /// `GET /api/v1/projects/{id}`
    pub async fn get_project(&self, id: &str) -> Result<serde_json::Value> {
        let path = format!("/api/v1/projects/{}", urlencoding(id));
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Update a project (partial).
    ///
    /// `PATCH /api/v1/projects/{id}`
    pub async fn update_project(
        &self,
        id: &str,
        body: &serde_json::Value,
    ) -> Result<serde_json::Value> {
        let path = format!("/api/v1/projects/{}", urlencoding(id));
        let (status, resp) = self.patch_json(&path, &body.to_string()).await?;
        Self::check_status(status, &resp)?;
        Self::parse_json(&resp)
    }

    /// Delete a project.
    ///
    /// `DELETE /api/v1/projects/{id}` -- 204 on success.
    pub async fn delete_project(&self, id: &str) -> Result<()> {
        let path = format!("/api/v1/projects/{}", urlencoding(id));
        let (status, body) = self.delete(&path).await?;
        Self::check_status(status, &body)?;
        Ok(())
    }

    /// List deployments linked to a project.
    ///
    /// `GET /api/v1/projects/{id}/deployments`
    pub async fn list_project_deployments(&self, id: &str) -> Result<Vec<String>> {
        let path = format!("/api/v1/projects/{}/deployments", urlencoding(id));
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Link a deployment to a project.
    ///
    /// `POST /api/v1/projects/{id}/deployments` with `{"deployment_name": "..."}`
    pub async fn link_project_deployment(&self, id: &str, deployment_name: &str) -> Result<()> {
        let path = format!("/api/v1/projects/{}/deployments", urlencoding(id));
        let payload = serde_json::json!({ "deployment_name": deployment_name });
        let (status, body) = self.post_json(&path, &payload.to_string()).await?;
        if !status.is_success() {
            Self::check_status(status, &body)?;
        }
        Ok(())
    }

    /// Unlink a deployment from a project.
    ///
    /// `DELETE /api/v1/projects/{id}/deployments/{name}`
    pub async fn unlink_project_deployment(&self, id: &str, deployment_name: &str) -> Result<()> {
        let path = format!(
            "/api/v1/projects/{}/deployments/{}",
            urlencoding(id),
            urlencoding(deployment_name),
        );
        let (status, body) = self.delete(&path).await?;
        Self::check_status(status, &body)?;
        Ok(())
    }

    /// Clone or fast-forward pull a project's git repository on the daemon.
    ///
    /// `POST /api/v1/projects/{id}/pull` -- returns `{ project_id, git_url,
    /// branch, sha, path }`.
    pub async fn project_pull(&self, id: &str) -> Result<serde_json::Value> {
        let path = format!("/api/v1/projects/{}/pull", urlencoding(id));
        // POST with an empty body; `post_json` requires a serialised payload
        // so we send `{}` here to match the route's lack of a request body.
        let (status, resp) = self.post_json(&path, "{}").await?;
        Self::check_status(status, &resp)?;
        Self::parse_json(&resp)
    }

    // ------------------------------------------------------------------
    // Webhooks
    // ------------------------------------------------------------------

    /// Get webhook configuration (URL + secret) for a project.
    ///
    /// `GET /api/v1/projects/{id}/webhook`
    pub async fn get_project_webhook(&self, id: &str) -> Result<serde_json::Value> {
        let path = format!("/api/v1/projects/{}/webhook", urlencoding(id));
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Rotate the webhook secret for a project.
    ///
    /// `POST /api/v1/projects/{id}/webhook/rotate`
    pub async fn rotate_project_webhook(&self, id: &str) -> Result<serde_json::Value> {
        let path = format!("/api/v1/projects/{}/webhook/rotate", urlencoding(id));
        let (status, resp) = self.post_json(&path, "{}").await?;
        Self::check_status(status, &resp)?;
        Self::parse_json(&resp)
    }

    // ------------------------------------------------------------------
    // Credentials
    // ------------------------------------------------------------------

    /// List registry credentials.
    ///
    /// `GET /api/v1/credentials/registry`
    pub async fn list_registry_credentials(&self) -> Result<Vec<serde_json::Value>> {
        let (status, body) = self.get("/api/v1/credentials/registry").await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Create a registry credential.
    ///
    /// `POST /api/v1/credentials/registry`
    pub async fn create_registry_credential(
        &self,
        body: &serde_json::Value,
    ) -> Result<serde_json::Value> {
        let (status, resp) = self
            .post_json("/api/v1/credentials/registry", &body.to_string())
            .await?;
        if !status.is_success() {
            Self::check_status(status, &resp)?;
        }
        Self::parse_json(&resp)
    }

    /// Delete a registry credential.
    ///
    /// `DELETE /api/v1/credentials/registry/{id}`
    pub async fn delete_registry_credential(&self, id: &str) -> Result<()> {
        let path = format!("/api/v1/credentials/registry/{}", urlencoding(id));
        let (status, body) = self.delete(&path).await?;
        Self::check_status(status, &body)?;
        Ok(())
    }

    /// List git credentials.
    ///
    /// `GET /api/v1/credentials/git`
    pub async fn list_git_credentials(&self) -> Result<Vec<serde_json::Value>> {
        let (status, body) = self.get("/api/v1/credentials/git").await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Create a git credential.
    ///
    /// `POST /api/v1/credentials/git`
    pub async fn create_git_credential(
        &self,
        body: &serde_json::Value,
    ) -> Result<serde_json::Value> {
        let (status, resp) = self
            .post_json("/api/v1/credentials/git", &body.to_string())
            .await?;
        if !status.is_success() {
            Self::check_status(status, &resp)?;
        }
        Self::parse_json(&resp)
    }

    /// Delete a git credential.
    ///
    /// `DELETE /api/v1/credentials/git/{id}`
    pub async fn delete_git_credential(&self, id: &str) -> Result<()> {
        let path = format!("/api/v1/credentials/git/{}", urlencoding(id));
        let (status, body) = self.delete(&path).await?;
        Self::check_status(status, &body)?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Env-scoped secrets
    // ------------------------------------------------------------------

    /// List secrets under a specific environment.
    ///
    /// `GET /api/v1/secrets?environment={env_id}`.
    pub async fn list_secrets_in_env(&self, env_id: &str) -> Result<Vec<SecretMetadataResponse>> {
        let path = format!("/api/v1/secrets?environment={}", urlencoding(env_id));
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Set (create or update) a secret under a specific environment.
    ///
    /// `POST /api/v1/secrets?environment={env_id}` with `{ name, value }`.
    pub async fn set_secret_in_env(
        &self,
        env_id: &str,
        name: &str,
        value: &str,
    ) -> Result<SecretMetadataResponse> {
        let path = format!("/api/v1/secrets?environment={}", urlencoding(env_id));
        let body = serde_json::json!({ "name": name, "value": value }).to_string();
        let (status, resp) = self.post_json(&path, &body).await?;
        if !status.is_success() {
            Self::check_status(status, &resp)?;
        }
        Self::parse_json(&resp)
    }

    /// Delete a secret under a specific environment.
    ///
    /// `DELETE /api/v1/secrets/{name}?environment={env_id}` — 204 on success.
    pub async fn delete_secret_in_env(&self, env_id: &str, name: &str) -> Result<()> {
        let path = format!(
            "/api/v1/secrets/{}?environment={}",
            urlencoding(name),
            urlencoding(env_id),
        );
        let (status, body) = self.delete(&path).await?;
        Self::check_status(status, &body)?;
        Ok(())
    }

    /// Reveal a secret value under a specific environment (admin only).
    ///
    /// `GET /api/v1/secrets/{name}?environment={env_id}&reveal=true`.
    /// Returns the plaintext value extracted from the `SecretMetadataResponse`.
    pub async fn reveal_secret_in_env(&self, env_id: &str, name: &str) -> Result<String> {
        let path = format!(
            "/api/v1/secrets/{}?environment={}&reveal=true",
            urlencoding(name),
            urlencoding(env_id),
        );
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        let resp: SecretMetadataResponse = Self::parse_json(&body)?;
        resp.value
            .ok_or_else(|| anyhow::anyhow!("Daemon did not return a value for revealed secret"))
    }

    /// Bulk-import secrets from a `.env`-style body into a specific environment.
    ///
    /// `POST /api/v1/secrets/bulk-import?environment={env_id}` with a
    /// `text/plain` body containing `KEY=VALUE` lines. Returns the raw import
    /// summary ({ created, updated, errors }) as a JSON value so the caller
    /// can render it verbatim.
    pub async fn bulk_import_secrets(
        &self,
        env_id: &str,
        dotenv_body: &str,
    ) -> Result<serde_json::Value> {
        let path = format!(
            "/api/v1/secrets/bulk-import?environment={}",
            urlencoding(env_id),
        );
        let (status, resp) = self.post_text(&path, dotenv_body).await?;
        Self::check_status(status, &resp)?;
        Self::parse_json(&resp)
    }

    // ------------------------------------------------------------------
    // Syncs
    // ------------------------------------------------------------------

    /// List all syncs.
    ///
    /// `GET /api/v1/syncs`
    pub async fn list_syncs(&self) -> Result<Vec<serde_json::Value>> {
        let (status, body) = self.get("/api/v1/syncs").await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Create a new sync.
    ///
    /// `POST /api/v1/syncs`
    pub async fn create_sync(&self, body: &serde_json::Value) -> Result<serde_json::Value> {
        let (status, resp) = self.post_json("/api/v1/syncs", &body.to_string()).await?;
        if !status.is_success() {
            Self::check_status(status, &resp)?;
        }
        Self::parse_json(&resp)
    }

    /// Get the diff for a sync.
    ///
    /// `GET /api/v1/syncs/{id}/diff`
    pub async fn diff_sync(&self, id: &str) -> Result<serde_json::Value> {
        let path = format!("/api/v1/syncs/{}/diff", urlencoding(id));
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Apply a sync (dry-run in v1).
    ///
    /// `POST /api/v1/syncs/{id}/apply`
    pub async fn apply_sync(&self, id: &str) -> Result<serde_json::Value> {
        let path = format!("/api/v1/syncs/{}/apply", urlencoding(id));
        let (status, body) = self.post_json(&path, "{}").await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Delete a sync.
    ///
    /// `DELETE /api/v1/syncs/{id}` -- 204 on success.
    pub async fn delete_sync(&self, id: &str) -> Result<()> {
        let path = format!("/api/v1/syncs/{}", urlencoding(id));
        let (status, body) = self.delete(&path).await?;
        Self::check_status(status, &body)?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Tasks
    // ------------------------------------------------------------------

    /// List all tasks, optionally filtered by project id.
    ///
    /// `GET /api/v1/tasks[?project_id={id}]`
    pub async fn list_tasks(
        &self,
        project_id: Option<&str>,
    ) -> Result<Vec<zlayer_api::StoredTask>> {
        let path = match project_id {
            Some(pid) => format!("/api/v1/tasks?project_id={}", urlencoding(pid)),
            None => "/api/v1/tasks".to_string(),
        };
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Create a new task.
    ///
    /// `POST /api/v1/tasks` with `{ name, kind, body, project_id? }`.
    pub async fn create_task(
        &self,
        name: &str,
        kind: &str,
        body_script: &str,
        project_id: Option<&str>,
    ) -> Result<zlayer_api::StoredTask> {
        let payload = serde_json::json!({
            "name": name,
            "kind": kind,
            "body": body_script,
            "project_id": project_id,
        })
        .to_string();
        let (status, resp) = self.post_json("/api/v1/tasks", &payload).await?;
        if !status.is_success() {
            Self::check_status(status, &resp)?;
        }
        Self::parse_json(&resp)
    }

    /// Fetch a single task by id.
    ///
    /// `GET /api/v1/tasks/{id}`.
    pub async fn get_task(&self, id: &str) -> Result<zlayer_api::StoredTask> {
        let path = format!("/api/v1/tasks/{}", urlencoding(id));
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Execute a task synchronously.
    ///
    /// `POST /api/v1/tasks/{id}/run`.
    pub async fn run_task(&self, id: &str) -> Result<zlayer_api::TaskRun> {
        let path = format!("/api/v1/tasks/{}/run", urlencoding(id));
        let (status, body) = self.post_json(&path, "{}").await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// List past runs for a task.
    ///
    /// `GET /api/v1/tasks/{id}/runs`.
    pub async fn list_task_runs(&self, id: &str) -> Result<Vec<zlayer_api::TaskRun>> {
        let path = format!("/api/v1/tasks/{}/runs", urlencoding(id));
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Delete a task.
    ///
    /// `DELETE /api/v1/tasks/{id}` -- 204 on success.
    pub async fn delete_task(&self, id: &str) -> Result<()> {
        let path = format!("/api/v1/tasks/{}", urlencoding(id));
        let (status, body) = self.delete(&path).await?;
        Self::check_status(status, &body)?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Workflows
    // ------------------------------------------------------------------

    /// List all workflows.
    ///
    /// `GET /api/v1/workflows`
    pub async fn list_workflows(&self) -> Result<Vec<zlayer_api::StoredWorkflow>> {
        let (status, body) = self.get("/api/v1/workflows").await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Create a new workflow.
    ///
    /// `POST /api/v1/workflows` with `{ name, steps, project_id? }`.
    ///
    /// `steps_json` is a raw JSON array of workflow steps.
    pub async fn create_workflow(
        &self,
        name: &str,
        steps_json: &str,
        project_id: Option<&str>,
    ) -> Result<zlayer_api::StoredWorkflow> {
        // Parse the steps JSON to validate it before sending
        let steps: serde_json::Value =
            serde_json::from_str(steps_json).context("Invalid JSON for --steps")?;

        let payload = serde_json::json!({
            "name": name,
            "steps": steps,
            "project_id": project_id,
        })
        .to_string();
        let (status, resp) = self.post_json("/api/v1/workflows", &payload).await?;
        if !status.is_success() {
            Self::check_status(status, &resp)?;
        }
        Self::parse_json(&resp)
    }

    /// Execute a workflow synchronously.
    ///
    /// `POST /api/v1/workflows/{id}/run`.
    pub async fn run_workflow(&self, id: &str) -> Result<zlayer_api::WorkflowRun> {
        let path = format!("/api/v1/workflows/{}/run", urlencoding(id));
        let (status, body) = self.post_json(&path, "{}").await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// List past runs for a workflow.
    ///
    /// `GET /api/v1/workflows/{id}/runs`.
    pub async fn list_workflow_runs(&self, id: &str) -> Result<Vec<zlayer_api::WorkflowRun>> {
        let path = format!("/api/v1/workflows/{}/runs", urlencoding(id));
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Delete a workflow.
    ///
    /// `DELETE /api/v1/workflows/{id}` -- 204 on success.
    pub async fn delete_workflow(&self, id: &str) -> Result<()> {
        let path = format!("/api/v1/workflows/{}", urlencoding(id));
        let (status, body) = self.delete(&path).await?;
        Self::check_status(status, &body)?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Notifiers
    // ------------------------------------------------------------------

    /// List all notifiers.
    ///
    /// `GET /api/v1/notifiers`
    pub async fn list_notifiers(&self) -> Result<Vec<zlayer_api::StoredNotifier>> {
        let (status, body) = self.get("/api/v1/notifiers").await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Create a new notifier.
    ///
    /// `POST /api/v1/notifiers` with `{ name, kind, config }`.
    pub async fn create_notifier(
        &self,
        name: &str,
        kind: zlayer_api::NotifierKind,
        config: &zlayer_api::NotifierConfig,
    ) -> Result<zlayer_api::StoredNotifier> {
        let payload = serde_json::json!({
            "name": name,
            "kind": kind,
            "config": config,
        })
        .to_string();
        let (status, resp) = self.post_json("/api/v1/notifiers", &payload).await?;
        if !status.is_success() {
            Self::check_status(status, &resp)?;
        }
        Self::parse_json(&resp)
    }

    /// Send a test notification through a notifier.
    ///
    /// `POST /api/v1/notifiers/{id}/test`.
    pub async fn test_notifier(&self, id: &str) -> Result<zlayer_api::TestNotifierResponse> {
        let path = format!("/api/v1/notifiers/{}/test", urlencoding(id));
        let (status, body) = self.post_json(&path, "{}").await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Delete a notifier.
    ///
    /// `DELETE /api/v1/notifiers/{id}` -- 204 on success.
    pub async fn delete_notifier(&self, id: &str) -> Result<()> {
        let path = format!("/api/v1/notifiers/{}", urlencoding(id));
        let (status, body) = self.delete(&path).await?;
        Self::check_status(status, &body)?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Groups
    // ------------------------------------------------------------------

    /// List all groups.
    ///
    /// `GET /api/v1/groups`
    pub async fn list_groups(&self) -> Result<Vec<zlayer_api::storage::StoredUserGroup>> {
        let (status, body) = self.get("/api/v1/groups").await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Create a group.
    ///
    /// `POST /api/v1/groups`
    pub async fn create_group(
        &self,
        req: &zlayer_api::CreateGroupRequest,
    ) -> Result<zlayer_api::storage::StoredUserGroup> {
        let body = serde_json::to_string(req).context("Failed to serialise CreateGroupRequest")?;
        let (status, resp_body) = self.post_json("/api/v1/groups", &body).await?;
        Self::check_status(status, &resp_body)?;
        Self::parse_json(&resp_body)
    }

    /// Delete a group.
    ///
    /// `DELETE /api/v1/groups/{id}` -- 204 on success.
    pub async fn delete_group(&self, id: &str) -> Result<()> {
        let path = format!("/api/v1/groups/{}", urlencoding(id));
        let (status, body) = self.delete(&path).await?;
        Self::check_status(status, &body)?;
        Ok(())
    }

    /// Add a member to a group.
    ///
    /// `POST /api/v1/groups/{id}/members`
    pub async fn add_group_member(&self, group_id: &str, user_id: &str) -> Result<()> {
        let req = zlayer_api::AddMemberRequest {
            user_id: user_id.to_string(),
        };
        let body = serde_json::to_string(&req).context("Failed to serialise AddMemberRequest")?;
        let path = format!("/api/v1/groups/{}/members", urlencoding(group_id));
        let (status, resp_body) = self.post_json(&path, &body).await?;
        Self::check_status(status, &resp_body)?;
        Ok(())
    }

    /// Remove a member from a group.
    ///
    /// `DELETE /api/v1/groups/{id}/members/{user_id}`
    pub async fn remove_group_member(&self, group_id: &str, user_id: &str) -> Result<()> {
        let path = format!(
            "/api/v1/groups/{}/members/{}",
            urlencoding(group_id),
            urlencoding(user_id)
        );
        let (status, body) = self.delete(&path).await?;
        Self::check_status(status, &body)?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Permissions
    // ------------------------------------------------------------------

    /// List permissions for a subject.
    ///
    /// `GET /api/v1/permissions?user={id}` or `?group={id}`
    pub async fn list_permissions(
        &self,
        user: Option<&str>,
        group: Option<&str>,
    ) -> Result<Vec<zlayer_api::storage::StoredPermission>> {
        let mut path = "/api/v1/permissions".to_string();
        if let Some(uid) = user {
            path = format!("{path}?user={}", urlencoding(uid));
        } else if let Some(gid) = group {
            path = format!("{path}?group={}", urlencoding(gid));
        }
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    /// Grant a permission.
    ///
    /// `POST /api/v1/permissions`
    pub async fn grant_permission(
        &self,
        req: &zlayer_api::GrantPermissionRequest,
    ) -> Result<zlayer_api::storage::StoredPermission> {
        let body =
            serde_json::to_string(req).context("Failed to serialise GrantPermissionRequest")?;
        let (status, resp_body) = self.post_json("/api/v1/permissions", &body).await?;
        Self::check_status(status, &resp_body)?;
        Self::parse_json(&resp_body)
    }

    /// Revoke a permission.
    ///
    /// `DELETE /api/v1/permissions/{id}` -- 204 on success.
    pub async fn revoke_permission(&self, id: &str) -> Result<()> {
        let path = format!("/api/v1/permissions/{}", urlencoding(id));
        let (status, body) = self.delete(&path).await?;
        Self::check_status(status, &body)?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Audit
    // ------------------------------------------------------------------

    /// List audit entries with optional filters.
    ///
    /// `GET /api/v1/audit?user={id}&resource_kind={k}&limit={n}`
    pub async fn list_audit(
        &self,
        user: Option<&str>,
        resource_kind: Option<&str>,
        limit: Option<usize>,
    ) -> Result<Vec<zlayer_api::storage::AuditEntry>> {
        let mut params = Vec::new();
        if let Some(u) = user {
            params.push(format!("user={}", urlencoding(u)));
        }
        if let Some(rk) = resource_kind {
            params.push(format!("resource_kind={}", urlencoding(rk)));
        }
        if let Some(l) = limit {
            params.push(format!("limit={l}"));
        }
        let path = if params.is_empty() {
            "/api/v1/audit".to_string()
        } else {
            format!("/api/v1/audit?{}", params.join("&"))
        };
        let (status, body) = self.get(&path).await?;
        Self::check_status(status, &body)?;
        Self::parse_json(&body)
    }

    // ------------------------------------------------------------------
    // Image builds
    // ------------------------------------------------------------------

    /// Kick off an image build against a server-side context path.
    ///
    /// `POST /api/v1/build/json` with a JSON [`BuildSpec`]
    /// (`context_path`, `runtime`, `build_args`, `target`, `tags`,
    /// `no_cache`, `push`). The daemon returns `202 Accepted` with a
    /// [`BuildHandle`] carrying the `build_id` used to poll
    /// `GET /api/v1/build/{id}`, stream via
    /// `GET /api/v1/build/{id}/stream`, or collect logs from
    /// `GET /api/v1/build/{id}/logs`.
    ///
    /// The `context_path` must reference a directory that already exists on
    /// the daemon host; this method does not upload a tarball. For
    /// multipart-upload builds use the `/api/v1/build` endpoint directly.
    pub async fn start_build(&self, spec: BuildSpec) -> Result<BuildHandle> {
        let body = serde_json::to_string(&spec).context("Failed to serialize BuildSpec")?;
        let (status, resp) = self.post_json("/api/v1/build/json", &body).await?;
        Self::check_status(status, &resp)?;
        Self::parse_json(&resp)
    }
}

// ---------------------------------------------------------------------------
// Build DTOs (client-side mirrors of zlayer-api build request/response shapes)
// ---------------------------------------------------------------------------

/// Specification sent to the daemon's `POST /api/v1/build/json` endpoint to
/// start an image build against a server-side context path.
///
/// Mirrors `zlayer_api::handlers::build::BuildRequestWithContext` on the
/// wire; the server-side type is `Deserialize`-only so we carry a
/// `Serialize` mirror here.
#[derive(Debug, Clone, Default, Serialize)]
pub struct BuildSpec {
    /// Path to the build context on the daemon host. Must already exist.
    pub context_path: String,
    /// Runtime template name (e.g. `"node20"`) to use instead of a Dockerfile.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime: Option<String>,
    /// Build arguments (Dockerfile `ARG` values).
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub build_args: HashMap<String, String>,
    /// Target stage for multi-stage Dockerfiles.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// Tags to apply to the resulting image.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Disable the buildah layer cache.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub no_cache: bool,
    /// Push the resulting image to its registry after the build succeeds.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub push: bool,
}

/// Handle returned by [`DaemonClient::start_build`]. Mirrors the wire shape
/// of `zlayer_api::handlers::build::TriggerBuildResponse` (which is
/// `Serialize`-only, so we carry a `Deserialize` mirror here).
///
/// The `build_id` can be fed to `GET /api/v1/build/{id}`,
/// `GET /api/v1/build/{id}/stream`, and `GET /api/v1/build/{id}/logs`.
#[derive(Debug, Clone, Deserialize)]
pub struct BuildHandle {
    /// Unique build ID for tracking.
    pub build_id: String,
    /// Human-readable message from the daemon.
    pub message: String,
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
                    use std::fmt::Write;
                    let _ = write!(out, "%{byte:02X}");
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
    fn container_logs_cli_decodes_plain_text() {
        let body: &[u8] = b"[stdout] hello\n[stderr] world\n";
        let out = DaemonClient::decode_text_body(body).expect("should decode");
        assert_eq!(out, "[stdout] hello\n[stderr] world\n");
    }

    #[test]
    fn container_logs_cli_rejects_invalid_utf8() {
        // 0xFF is not valid UTF-8
        let bad: &[u8] = &[0xFF, 0xFE, 0xFD];
        assert!(DaemonClient::decode_text_body(bad).is_err());
    }

    #[test]
    fn test_find_self_binary() {
        // This test verifies the logic works without panicking.
        // current_exe() should always succeed in a test runner.
        let result = DaemonClient::find_self_binary();
        assert!(result.is_ok());
    }
}
