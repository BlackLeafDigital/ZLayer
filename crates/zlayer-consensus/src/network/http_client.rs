//! HTTP client for Raft network communication using postcard2 serialization.
//!
//! Implements `RaftNetworkFactory` and `RaftNetwork` traits from openraft,
//! using reqwest with connection pooling and split timeouts (short for
//! vote/append, long for snapshots).

use std::future::Future;
use std::io::Cursor;
use std::sync::Arc;
use std::time::Duration;

use openraft::error::{
    Fatal, InstallSnapshotError, RPCError, RaftError, ReplicationClosed, StreamingError,
    Unreachable,
};
use openraft::network::{RPCOption, RaftNetwork, RaftNetworkFactory};
use openraft::raft::{
    AppendEntriesRequest, AppendEntriesResponse, InstallSnapshotRequest, InstallSnapshotResponse,
    SnapshotResponse, VoteRequest, VoteResponse,
};
use openraft::{BasicNode, OptionalSend, RaftTypeConfig, Snapshot, SnapshotMeta, Vote};
use reqwest::Client;
use tracing::debug;

use crate::types::NodeId;

/// Wire protocol version emitted on every Raft RPC and validated by the server.
pub const RAFT_PROTOCOL_VERSION: &str = "1";

// ---------------------------------------------------------------------------
// HTTP client
// ---------------------------------------------------------------------------

/// HTTP client for Raft RPCs using **postcard2** serialization.
///
/// Maintains separate timeout configurations for regular RPCs and
/// snapshot transfers.  Optionally attaches a bearer token to every
/// outgoing request for authentication against the Raft service.
#[derive(Clone)]
pub struct RaftHttpClient {
    /// Client for regular RPCs (vote, `append_entries`).
    rpc_client: Client,
    /// Client for snapshot transfers (longer timeout).
    snapshot_client: Client,
    /// Precomputed `Authorization: Bearer …` header value.
    auth_header: Option<reqwest::header::HeaderValue>,
}

impl RaftHttpClient {
    /// Create a new client with the specified timeouts and no auth token.
    #[must_use]
    pub fn new(rpc_timeout: Duration, snapshot_timeout: Duration) -> Self {
        Self::with_auth(rpc_timeout, snapshot_timeout, None)
    }

    /// Create a new client with the specified timeouts and an optional auth token.
    ///
    /// # Panics
    /// Panics if the HTTP client builders fail to build (should not happen in practice).
    #[must_use]
    pub fn with_auth(
        rpc_timeout: Duration,
        snapshot_timeout: Duration,
        auth_token: Option<String>,
    ) -> Self {
        let rpc_client = Client::builder()
            .timeout(rpc_timeout)
            .pool_max_idle_per_host(2)
            .pool_idle_timeout(Duration::from_secs(90))
            .http2_prior_knowledge()
            .http2_keep_alive_interval(std::time::Duration::from_secs(10))
            .http2_keep_alive_while_idle(true)
            .build()
            .expect("Failed to build RPC HTTP client");

        let snapshot_client = Client::builder()
            .timeout(snapshot_timeout)
            .pool_max_idle_per_host(2)
            .pool_idle_timeout(Duration::from_secs(90))
            .http2_prior_knowledge()
            .http2_keep_alive_interval(std::time::Duration::from_secs(10))
            .http2_keep_alive_while_idle(true)
            .build()
            .expect("Failed to build snapshot HTTP client");

        let auth_header = auth_token.map(|token| {
            let mut header = reqwest::header::HeaderValue::from_str(&format!("Bearer {token}"))
                .expect("valid bearer token");
            header.set_sensitive(true);
            header
        });

        Self {
            rpc_client,
            snapshot_client,
            auth_header,
        }
    }

    /// Send a postcard2-encoded POST request and decode the response.
    async fn postcard_post<Req, Resp>(
        client: &Client,
        url: &str,
        request: &Req,
        auth_header: Option<&reqwest::header::HeaderValue>,
    ) -> Result<Resp, String>
    where
        Req: serde::Serialize,
        Resp: serde::de::DeserializeOwned,
    {
        let body =
            postcard2::to_vec(request).map_err(|e| format!("postcard2 serialize error: {e}"))?;

        let mut builder = client
            .post(url)
            .header("Content-Type", "application/octet-stream");

        builder = builder.header("X-ZLayer-Raft-Protocol", RAFT_PROTOCOL_VERSION);

        if let Some(header) = auth_header {
            builder = builder.header(reqwest::header::AUTHORIZATION, header.clone());
        }

        let response = builder.body(body).send().await.map_err(|e| {
            if e.is_timeout() {
                format!("timeout: {e}")
            } else if e.is_connect() {
                format!("unreachable: {e}")
            } else {
                format!("http error: {e}")
            }
        })?;

        if !response.status().is_success() {
            let status = response.status();
            if status == reqwest::StatusCode::UPGRADE_REQUIRED {
                let server_version = response
                    .headers()
                    .get("X-ZLayer-Raft-Protocol-Supported")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("<unknown>")
                    .to_string();
                return Err(format!(
                    "protocol version mismatch: server supports {server_version}"
                ));
            }
            let text = response.text().await.unwrap_or_default();
            return Err(format!("HTTP {status}: {text}"));
        }

        let bytes = response
            .bytes()
            .await
            .map_err(|e| format!("read body error: {e}"))?;
        postcard2::from_bytes(&bytes).map_err(|e| format!("postcard2 deserialize error: {e}"))
    }
}

impl Default for RaftHttpClient {
    fn default() -> Self {
        Self::new(Duration::from_secs(5), Duration::from_secs(60))
    }
}

// ---------------------------------------------------------------------------
// Network factory + connection
// ---------------------------------------------------------------------------

/// Build the four per-connection URLs for a given peer address.
fn build_urls(addr: &str) -> [String; 4] {
    let base = normalize_addr(addr);
    [
        format!("{base}/raft/append"),
        format!("{base}/raft/vote"),
        format!("{base}/raft/snapshot"),
        format!("{base}/raft/full-snapshot"),
    ]
}

/// Network factory that creates HTTP connections to Raft peers.
///
/// Generic over the `RaftTypeConfig` so any application can use it.
pub struct HttpNetwork<C: RaftTypeConfig<NodeId = NodeId>> {
    /// Shared HTTP client.
    client: Arc<RaftHttpClient>,
    _phantom: std::marker::PhantomData<C>,
}

impl<C: RaftTypeConfig<NodeId = NodeId>> HttpNetwork<C> {
    /// Create a new network layer with default timeouts (5s RPC, 60s snapshot).
    #[must_use]
    pub fn new() -> Self {
        Self::with_client(RaftHttpClient::default())
    }

    /// Create a new network layer with a custom client.
    #[must_use]
    pub fn with_client(client: RaftHttpClient) -> Self {
        Self {
            client: Arc::new(client),
            _phantom: std::marker::PhantomData,
        }
    }

    /// Create a new network layer with custom timeouts.
    #[must_use]
    pub fn with_timeouts(rpc_timeout: Duration, snapshot_timeout: Duration) -> Self {
        Self::with_client(RaftHttpClient::new(rpc_timeout, snapshot_timeout))
    }

    /// Create a new network layer with custom timeouts and an optional auth token.
    #[must_use]
    pub fn with_timeouts_and_auth(
        rpc_timeout: Duration,
        snapshot_timeout: Duration,
        auth_token: Option<String>,
    ) -> Self {
        Self::with_client(RaftHttpClient::with_auth(
            rpc_timeout,
            snapshot_timeout,
            auth_token,
        ))
    }
}

impl<C: RaftTypeConfig<NodeId = NodeId>> Default for HttpNetwork<C> {
    fn default() -> Self {
        Self::new()
    }
}

impl<C: RaftTypeConfig<NodeId = NodeId>> Clone for HttpNetwork<C> {
    fn clone(&self) -> Self {
        Self {
            client: Arc::clone(&self.client),
            _phantom: std::marker::PhantomData,
        }
    }
}

impl<C> RaftNetworkFactory<C> for HttpNetwork<C>
where
    C: RaftTypeConfig<NodeId = NodeId, Node = BasicNode, SnapshotData = Cursor<Vec<u8>>>,
    C::D: serde::Serialize + serde::de::DeserializeOwned,
    C::R: serde::Serialize + serde::de::DeserializeOwned,
    C::Entry: serde::Serialize + serde::de::DeserializeOwned,
{
    type Network = HttpConnection<C>;

    async fn new_client(&mut self, _target: NodeId, node: &BasicNode) -> Self::Network {
        let [append, vote, snapshot, full_snapshot] = build_urls(&node.addr);
        HttpConnection {
            target_addr: Arc::<str>::from(node.addr.as_str()),
            client: Arc::clone(&self.client),
            auth_header: self.client.auth_header.clone(),
            append_url: Arc::<str>::from(append.as_str()),
            vote_url: Arc::<str>::from(vote.as_str()),
            snapshot_url: Arc::<str>::from(snapshot.as_str()),
            full_snapshot_url: Arc::<str>::from(full_snapshot.as_str()),
            _phantom: std::marker::PhantomData,
        }
    }
}

/// A single connection to a Raft peer.
pub struct HttpConnection<C: RaftTypeConfig<NodeId = NodeId>> {
    target_addr: Arc<str>,
    client: Arc<RaftHttpClient>,
    /// Precomputed `Authorization` header for outgoing RPCs.
    auth_header: Option<reqwest::header::HeaderValue>,
    append_url: Arc<str>,
    vote_url: Arc<str>,
    snapshot_url: Arc<str>,
    full_snapshot_url: Arc<str>,
    _phantom: std::marker::PhantomData<C>,
}

/// Normalize an address to ensure it has an HTTP scheme.
fn normalize_addr(addr: &str) -> String {
    if addr.starts_with("http://") || addr.starts_with("https://") {
        addr.to_string()
    } else {
        format!("http://{addr}")
    }
}

/// Convert a string error to an `RPCError::Unreachable`.
fn to_unreachable<E: std::error::Error>(msg: String) -> RPCError<NodeId, BasicNode, E> {
    RPCError::Unreachable(Unreachable::new(&std::io::Error::other(msg)))
}

impl<C> RaftNetwork<C> for HttpConnection<C>
where
    C: RaftTypeConfig<NodeId = NodeId, Node = BasicNode, SnapshotData = Cursor<Vec<u8>>>,
    C::D: serde::Serialize + serde::de::DeserializeOwned,
    C::R: serde::Serialize + serde::de::DeserializeOwned,
    C::Entry: serde::Serialize + serde::de::DeserializeOwned,
{
    async fn append_entries(
        &mut self,
        rpc: AppendEntriesRequest<C>,
        _option: RPCOption,
    ) -> Result<AppendEntriesResponse<NodeId>, RPCError<NodeId, BasicNode, RaftError<NodeId>>> {
        debug!(target_addr = %self.target_addr, "Sending append_entries RPC");

        RaftHttpClient::postcard_post(
            &self.client.rpc_client,
            &self.append_url,
            &rpc,
            self.auth_header.as_ref(),
        )
        .await
        .map_err(to_unreachable)
    }

    async fn install_snapshot(
        &mut self,
        rpc: InstallSnapshotRequest<C>,
        _option: RPCOption,
    ) -> Result<
        InstallSnapshotResponse<NodeId>,
        RPCError<NodeId, BasicNode, RaftError<NodeId, InstallSnapshotError>>,
    > {
        debug!(target_addr = %self.target_addr, "Sending install_snapshot RPC");

        RaftHttpClient::postcard_post(
            &self.client.snapshot_client,
            &self.snapshot_url,
            &rpc,
            self.auth_header.as_ref(),
        )
        .await
        .map_err(to_unreachable)
    }

    async fn vote(
        &mut self,
        rpc: VoteRequest<NodeId>,
        _option: RPCOption,
    ) -> Result<VoteResponse<NodeId>, RPCError<NodeId, BasicNode, RaftError<NodeId>>> {
        debug!(target_addr = %self.target_addr, "Sending vote RPC");

        RaftHttpClient::postcard_post(
            &self.client.rpc_client,
            &self.vote_url,
            &rpc,
            self.auth_header.as_ref(),
        )
        .await
        .map_err(to_unreachable)
    }

    async fn full_snapshot(
        &mut self,
        vote: Vote<NodeId>,
        snapshot: Snapshot<C>,
        _cancel: impl Future<Output = ReplicationClosed> + OptionalSend + 'static,
        _option: RPCOption,
    ) -> Result<SnapshotResponse<NodeId>, StreamingError<C, Fatal<NodeId>>> {
        #[derive(serde::Serialize)]
        struct FullSnapshotReq {
            vote: Vote<NodeId>,
            meta: SnapshotMeta<NodeId, BasicNode>,
            snapshot_data: Vec<u8>,
        }

        debug!(target_addr = %self.target_addr, "Sending full_snapshot RPC");

        let snapshot_data = snapshot.snapshot.into_inner();

        let req = FullSnapshotReq {
            vote,
            meta: snapshot.meta,
            snapshot_data,
        };

        RaftHttpClient::postcard_post::<FullSnapshotReq, SnapshotResponse<NodeId>>(
            &self.client.snapshot_client,
            &self.full_snapshot_url,
            &req,
            self.auth_header.as_ref(),
        )
        .await
        .map_err(|e| StreamingError::Unreachable(Unreachable::new(&std::io::Error::other(e))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_addr() {
        assert_eq!(normalize_addr("10.0.0.1:9000"), "http://10.0.0.1:9000");
        assert_eq!(
            normalize_addr("http://10.0.0.1:9000"),
            "http://10.0.0.1:9000"
        );
        assert_eq!(
            normalize_addr("https://10.0.0.1:9000"),
            "https://10.0.0.1:9000"
        );
    }

    #[test]
    fn test_client_creation() {
        let _client = RaftHttpClient::new(Duration::from_secs(3), Duration::from_secs(30));
    }

    #[test]
    fn test_protocol_version_constant() {
        assert_eq!(RAFT_PROTOCOL_VERSION, "1");
    }

    #[test]
    fn test_connection_urls_precomputed() {
        let [append, vote, snapshot, full_snapshot] = build_urls("10.0.0.1:9000");
        assert_eq!(append, "http://10.0.0.1:9000/raft/append");
        assert_eq!(vote, "http://10.0.0.1:9000/raft/vote");
        assert_eq!(snapshot, "http://10.0.0.1:9000/raft/snapshot");
        assert_eq!(full_snapshot, "http://10.0.0.1:9000/raft/full-snapshot");
    }
}
