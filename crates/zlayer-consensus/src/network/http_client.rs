//! HTTP client for Raft network communication using bincode serialization.
//!
//! Implements `RaftNetworkFactory` and `RaftNetwork` traits from openraft,
//! using reqwest with connection pooling and split timeouts (short for
//! vote/append, long for snapshots).

use std::collections::HashMap;
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
use tokio::sync::RwLock;
use tracing::debug;

use crate::types::NodeId;

// ---------------------------------------------------------------------------
// HTTP client
// ---------------------------------------------------------------------------

/// HTTP client for Raft RPCs using **bincode** serialization.
///
/// Maintains separate timeout configurations for regular RPCs and
/// snapshot transfers.
#[derive(Clone)]
pub struct RaftHttpClient {
    /// Client for regular RPCs (vote, append_entries).
    rpc_client: Client,
    /// Client for snapshot transfers (longer timeout).
    snapshot_client: Client,
}

impl RaftHttpClient {
    /// Create a new client with the specified timeouts.
    pub fn new(rpc_timeout: Duration, snapshot_timeout: Duration) -> Self {
        let rpc_client = Client::builder()
            .timeout(rpc_timeout)
            .pool_max_idle_per_host(10)
            .pool_idle_timeout(Duration::from_secs(90))
            .build()
            .expect("Failed to build RPC HTTP client");

        let snapshot_client = Client::builder()
            .timeout(snapshot_timeout)
            .pool_max_idle_per_host(5)
            .pool_idle_timeout(Duration::from_secs(90))
            .build()
            .expect("Failed to build snapshot HTTP client");

        Self {
            rpc_client,
            snapshot_client,
        }
    }

    /// Send a bincode-encoded POST request and decode the response.
    async fn bincode_post<Req, Resp>(
        client: &Client,
        url: &str,
        request: &Req,
    ) -> Result<Resp, String>
    where
        Req: serde::Serialize,
        Resp: serde::de::DeserializeOwned,
    {
        let body =
            bincode::serialize(request).map_err(|e| format!("bincode serialize error: {e}"))?;

        let response = client
            .post(url)
            .header("Content-Type", "application/octet-stream")
            .body(body)
            .send()
            .await
            .map_err(|e| {
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
            let text = response.text().await.unwrap_or_default();
            return Err(format!("HTTP {status}: {text}"));
        }

        let bytes = response
            .bytes()
            .await
            .map_err(|e| format!("read body error: {e}"))?;
        bincode::deserialize(&bytes).map_err(|e| format!("bincode deserialize error: {e}"))
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

/// Network factory that creates HTTP connections to Raft peers.
///
/// Generic over the `RaftTypeConfig` so any application can use it.
pub struct HttpNetwork<C: RaftTypeConfig<NodeId = NodeId>> {
    /// Known peers (for informational purposes / peer management).
    peers: Arc<RwLock<HashMap<NodeId, String>>>,
    /// Shared HTTP client.
    client: Arc<RaftHttpClient>,
    _phantom: std::marker::PhantomData<C>,
}

impl<C: RaftTypeConfig<NodeId = NodeId>> HttpNetwork<C> {
    /// Create a new network layer with default timeouts (5s RPC, 60s snapshot).
    pub fn new() -> Self {
        Self::with_client(RaftHttpClient::default())
    }

    /// Create a new network layer with a custom client.
    pub fn with_client(client: RaftHttpClient) -> Self {
        Self {
            peers: Arc::new(RwLock::new(HashMap::new())),
            client: Arc::new(client),
            _phantom: std::marker::PhantomData,
        }
    }

    /// Create a new network layer with custom timeouts.
    pub fn with_timeouts(rpc_timeout: Duration, snapshot_timeout: Duration) -> Self {
        Self::with_client(RaftHttpClient::new(rpc_timeout, snapshot_timeout))
    }

    /// Add a peer address.
    pub async fn add_peer(&self, node_id: NodeId, address: String) {
        self.peers.write().await.insert(node_id, address);
    }

    /// Remove a peer.
    pub async fn remove_peer(&self, node_id: NodeId) {
        self.peers.write().await.remove(&node_id);
    }

    /// Get all known peers.
    pub async fn peers(&self) -> HashMap<NodeId, String> {
        self.peers.read().await.clone()
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
            peers: Arc::clone(&self.peers),
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
        HttpConnection {
            target_addr: node.addr.clone(),
            client: Arc::clone(&self.client),
            _phantom: std::marker::PhantomData,
        }
    }
}

/// A single connection to a Raft peer.
pub struct HttpConnection<C: RaftTypeConfig<NodeId = NodeId>> {
    target_addr: String,
    client: Arc<RaftHttpClient>,
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

/// Convert a string error to an RPCError::Unreachable.
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
        let url = format!("{}/raft/append", normalize_addr(&self.target_addr));
        debug!(target_addr = %self.target_addr, "Sending append_entries RPC");

        RaftHttpClient::bincode_post(&self.client.rpc_client, &url, &rpc)
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
        let url = format!("{}/raft/snapshot", normalize_addr(&self.target_addr));
        debug!(target_addr = %self.target_addr, "Sending install_snapshot RPC");

        RaftHttpClient::bincode_post(&self.client.snapshot_client, &url, &rpc)
            .await
            .map_err(to_unreachable)
    }

    async fn vote(
        &mut self,
        rpc: VoteRequest<NodeId>,
        _option: RPCOption,
    ) -> Result<VoteResponse<NodeId>, RPCError<NodeId, BasicNode, RaftError<NodeId>>> {
        let url = format!("{}/raft/vote", normalize_addr(&self.target_addr));
        debug!(target_addr = %self.target_addr, "Sending vote RPC");

        RaftHttpClient::bincode_post(&self.client.rpc_client, &url, &rpc)
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
        let url = format!("{}/raft/full-snapshot", normalize_addr(&self.target_addr));
        debug!(target_addr = %self.target_addr, "Sending full_snapshot RPC");

        let snapshot_data = snapshot.snapshot.into_inner();

        #[derive(serde::Serialize)]
        struct FullSnapshotReq {
            vote: Vote<NodeId>,
            meta: SnapshotMeta<NodeId, BasicNode>,
            snapshot_data: Vec<u8>,
        }

        let req = FullSnapshotReq {
            vote,
            meta: snapshot.meta,
            snapshot_data,
        };

        RaftHttpClient::bincode_post::<FullSnapshotReq, SnapshotResponse<NodeId>>(
            &self.client.snapshot_client,
            &url,
            &req,
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
}
