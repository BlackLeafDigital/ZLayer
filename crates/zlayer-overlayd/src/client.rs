//! Client the main `zlayer` daemon uses to drive overlayd over IPC.
//!
//! Holds one framed connection and issues request/response round-trips. The
//! agent's `overlay_manager` shim wraps this (behind a `Mutex`) and maps its
//! public methods to [`OverlaydRequest`]s.

use std::path::{Path, PathBuf};
use std::time::Duration;

use zlayer_types::overlayd::{OverlaydFrame, OverlaydRequest, OverlaydResponse};

use crate::error::{OverlaydError, Result};
use crate::transport::{self, ClientConn};

/// A connected overlayd client.
pub struct OverlaydClient {
    conn: ClientConn,
    next_id: u64,
    endpoint: PathBuf,
}

impl OverlaydClient {
    /// Connect once to the overlayd endpoint.
    ///
    /// # Errors
    /// Propagates the underlying connect error.
    pub async fn connect(endpoint: &Path) -> Result<Self> {
        let conn = transport::connect(endpoint).await?;
        Ok(Self {
            conn,
            next_id: 1,
            endpoint: endpoint.to_path_buf(),
        })
    }

    /// Connect with exponential backoff (100ms → 1s, ~20 attempts) so the main
    /// daemon can start before overlayd has finished binding its socket.
    ///
    /// # Errors
    /// Returns the last connect error if every attempt fails.
    pub async fn connect_with_backoff(endpoint: &Path) -> Result<Self> {
        let mut delay = Duration::from_millis(100);
        let max_delay = Duration::from_secs(1);
        let mut last_err: Option<OverlaydError> = None;
        for _ in 0..20 {
            match transport::connect(endpoint).await {
                Ok(conn) => {
                    return Ok(Self {
                        conn,
                        next_id: 1,
                        endpoint: endpoint.to_path_buf(),
                    });
                }
                Err(e) => {
                    last_err = Some(e);
                    tokio::time::sleep(delay).await;
                    delay = std::cmp::min(delay * 2, max_delay);
                }
            }
        }
        Err(last_err.unwrap_or_else(|| OverlaydError::Other("overlayd connect failed".to_string())))
    }

    /// The endpoint this client is connected to.
    pub fn endpoint(&self) -> &Path {
        &self.endpoint
    }

    /// Send a request and await its matching response. Event frames that arrive
    /// while waiting are logged and skipped (a request/response client does not
    /// subscribe to events).
    ///
    /// # Errors
    /// Propagates transport errors. The returned [`OverlaydResponse`] may itself
    /// be [`OverlaydResponse::Err`]; use [`Self::call`] to fold that into an
    /// error.
    pub async fn request(&mut self, request: OverlaydRequest) -> Result<OverlaydResponse> {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        self.conn
            .send(&OverlaydFrame::Request { id, request })
            .await?;
        loop {
            match self.conn.recv().await? {
                OverlaydFrame::Response { id: rid, response } if rid == id => return Ok(response),
                OverlaydFrame::Response { id: rid, .. } => {
                    tracing::warn!(
                        expected = id,
                        got = rid,
                        "overlayd: out-of-order response id"
                    );
                }
                OverlaydFrame::Event(ev) => {
                    tracing::debug!(?ev, "overlayd: event with no subscriber; dropping");
                }
                OverlaydFrame::Request { .. } => {
                    return Err(OverlaydError::Other(
                        "overlayd sent a Request frame to a client".to_string(),
                    ));
                }
            }
        }
    }

    /// Like [`Self::request`] but maps an `Err` response into
    /// [`OverlaydError::Overlay`], so callers get a single `Result`.
    ///
    /// # Errors
    /// Transport errors, or [`OverlaydError::Overlay`] if overlayd returned an
    /// error response.
    pub async fn call(&mut self, request: OverlaydRequest) -> Result<OverlaydResponse> {
        match self.request(request).await? {
            OverlaydResponse::Err { message } => Err(OverlaydError::Overlay(message)),
            other => Ok(other),
        }
    }
}
