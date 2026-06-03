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
    /// Use this on the path that has *just spawned* overlayd and legitimately
    /// needs to wait for it to bind (the supervisor). On hot paths where a dead
    /// overlayd must degrade fast (overlay setup is non-fatal), prefer
    /// [`Self::connect_with_attempts`] with a small budget so daemon startup is
    /// not held hostage by an unreachable overlayd.
    ///
    /// # Errors
    /// Returns the last connect error if every attempt fails.
    pub async fn connect_with_backoff(endpoint: &Path) -> Result<Self> {
        Self::connect_with_attempts(endpoint, 20).await
    }

    /// Connect with bounded exponential backoff (100ms → 1s) for at most
    /// `max_attempts` attempts. Sleeps only *between* attempts (never after the
    /// final one), so the worst-case wall time for a dead endpoint is bounded
    /// and predictable: `max_attempts = 6` ≈ 100+200+400+800+1000 = ~2.5s.
    ///
    /// # Errors
    /// Returns the last connect error if every attempt fails.
    pub async fn connect_with_attempts(endpoint: &Path, max_attempts: u32) -> Result<Self> {
        let attempts = max_attempts.max(1);
        let mut delay = Duration::from_millis(100);
        let max_delay = Duration::from_secs(1);
        let mut last_err: Option<OverlaydError> = None;
        for attempt in 0..attempts {
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
                    // Don't sleep after the final attempt — it's wasted latency.
                    if attempt + 1 < attempts {
                        tokio::time::sleep(delay).await;
                        delay = std::cmp::min(delay * 2, max_delay);
                    }
                }
            }
        }
        Err(last_err.unwrap_or_else(|| OverlaydError::Other("overlayd connect failed".to_string())))
    }

    /// The endpoint this client is connected to.
    #[must_use]
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    /// A single attempt against a dead endpoint must NOT sleep (the trailing-sleep
    /// fix): it should fail effectively immediately, not after the first backoff.
    #[tokio::test]
    async fn connect_single_attempt_does_not_sleep() {
        let dead = Path::new("/nonexistent/zlayer-overlayd-test.sock");
        let start = Instant::now();
        let res = OverlaydClient::connect_with_attempts(dead, 1).await;
        let elapsed = start.elapsed();
        assert!(res.is_err(), "connect to a nonexistent socket must fail");
        assert!(
            elapsed < Duration::from_millis(80),
            "1 attempt must not pay a backoff sleep (took {elapsed:?})"
        );
    }

    /// `connect_with_attempts` is bounded: against a dead endpoint it returns an
    /// error after roughly the sum of the inter-attempt backoffs, never hanging.
    /// 4 attempts ⇒ sleeps of 100+200+400 ≈ 700ms (no sleep after the 4th).
    #[tokio::test]
    async fn connect_with_attempts_is_bounded() {
        let dead = Path::new("/nonexistent/zlayer-overlayd-test.sock");
        let start = Instant::now();
        let res = OverlaydClient::connect_with_attempts(dead, 4).await;
        let elapsed = start.elapsed();
        assert!(res.is_err());
        // Lower bound proves it actually retried with backoff; upper bound proves
        // it stayed bounded (and didn't sleep after the final attempt).
        assert!(
            elapsed >= Duration::from_millis(600),
            "4 attempts should back off ~700ms (took {elapsed:?})"
        );
        assert!(
            elapsed < Duration::from_millis(1500),
            "4 attempts must stay bounded (took {elapsed:?})"
        );
    }

    /// `max_attempts = 0` is clamped to 1 (one try, no sleep), never an infinite
    /// or zero-attempt loop.
    #[tokio::test]
    async fn connect_zero_attempts_clamped_to_one() {
        let dead = Path::new("/nonexistent/zlayer-overlayd-test.sock");
        let res = OverlaydClient::connect_with_attempts(dead, 0).await;
        assert!(res.is_err());
    }
}
