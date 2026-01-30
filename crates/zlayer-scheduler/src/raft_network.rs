//! HTTP client for Raft network communication
//!
//! Provides a reqwest-based HTTP client for making RPC calls to other
//! Raft nodes in the cluster.

use std::time::Duration;

use openraft::error::{InstallSnapshotError, RPCError, RaftError, Unreachable};
use openraft::raft::{
    AppendEntriesRequest, AppendEntriesResponse, InstallSnapshotRequest, InstallSnapshotResponse,
    SnapshotResponse, VoteRequest, VoteResponse,
};
use openraft::{BasicNode, Vote};
use reqwest::Client;
use serde::Serialize;

use crate::error::RaftNetworkError;
use crate::raft::{NodeId, TypeConfig};

/// HTTP client for Raft RPC calls
///
/// This client uses reqwest to send JSON-encoded Raft messages
/// to peer nodes over HTTP.
#[derive(Clone)]
pub struct RaftHttpClient {
    client: Client,
    timeout: Duration,
}

impl RaftHttpClient {
    /// Create a new Raft HTTP client with the specified timeout
    ///
    /// # Arguments
    /// * `timeout_ms` - Request timeout in milliseconds
    pub fn new(timeout_ms: u64) -> Self {
        let timeout = Duration::from_millis(timeout_ms);
        let client = Client::builder()
            .timeout(timeout)
            .pool_max_idle_per_host(10)
            .pool_idle_timeout(Duration::from_secs(90))
            .build()
            .expect("Failed to build HTTP client");

        Self { client, timeout }
    }

    /// Send an AppendEntries RPC to the target node
    ///
    /// # Arguments
    /// * `target` - Target node address (e.g., "http://192.168.1.10:9000")
    /// * `request` - AppendEntries request
    ///
    /// # Returns
    /// The AppendEntries response or a network error
    pub async fn append_entries(
        &self,
        target: &str,
        request: AppendEntriesRequest<TypeConfig>,
    ) -> Result<AppendEntriesResponse<NodeId>, RaftNetworkError> {
        let url = format!("{}/raft/append-entries", normalize_addr(target));

        let response = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    RaftNetworkError::Timeout
                } else if e.is_connect() {
                    RaftNetworkError::Unreachable(format!("Failed to connect to {}: {}", url, e))
                } else {
                    RaftNetworkError::Http(e)
                }
            })?;

        if !response.status().is_success() {
            return Err(RaftNetworkError::InvalidResponse(format!(
                "HTTP {}: {}",
                response.status(),
                response.text().await.unwrap_or_default()
            )));
        }

        response
            .json()
            .await
            .map_err(|e| RaftNetworkError::Serialization(e.to_string()))
    }

    /// Send an InstallSnapshot RPC to the target node
    ///
    /// # Arguments
    /// * `target` - Target node address
    /// * `request` - InstallSnapshot request
    ///
    /// # Returns
    /// The InstallSnapshot response or a network error
    pub async fn install_snapshot(
        &self,
        target: &str,
        request: InstallSnapshotRequest<TypeConfig>,
    ) -> Result<InstallSnapshotResponse<NodeId>, RaftNetworkError> {
        let url = format!("{}/raft/install-snapshot", normalize_addr(target));

        let response = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    RaftNetworkError::Timeout
                } else if e.is_connect() {
                    RaftNetworkError::Unreachable(format!("Failed to connect to {}: {}", url, e))
                } else {
                    RaftNetworkError::Http(e)
                }
            })?;

        if !response.status().is_success() {
            return Err(RaftNetworkError::InvalidResponse(format!(
                "HTTP {}: {}",
                response.status(),
                response.text().await.unwrap_or_default()
            )));
        }

        response
            .json()
            .await
            .map_err(|e| RaftNetworkError::Serialization(e.to_string()))
    }

    /// Send a Vote RPC to the target node
    ///
    /// # Arguments
    /// * `target` - Target node address
    /// * `request` - Vote request
    ///
    /// # Returns
    /// The Vote response or a network error
    pub async fn vote(
        &self,
        target: &str,
        request: VoteRequest<NodeId>,
    ) -> Result<VoteResponse<NodeId>, RaftNetworkError> {
        let url = format!("{}/raft/vote", normalize_addr(target));

        let response = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    RaftNetworkError::Timeout
                } else if e.is_connect() {
                    RaftNetworkError::Unreachable(format!("Failed to connect to {}: {}", url, e))
                } else {
                    RaftNetworkError::Http(e)
                }
            })?;

        if !response.status().is_success() {
            return Err(RaftNetworkError::InvalidResponse(format!(
                "HTTP {}: {}",
                response.status(),
                response.text().await.unwrap_or_default()
            )));
        }

        response
            .json()
            .await
            .map_err(|e| RaftNetworkError::Serialization(e.to_string()))
    }

    /// Send a full snapshot to the target node
    ///
    /// # Arguments
    /// * `target` - Target node address
    /// * `vote` - Current vote information
    /// * `snapshot_data` - The snapshot data as bytes
    ///
    /// # Returns
    /// The snapshot response or a network error
    pub async fn full_snapshot(
        &self,
        target: &str,
        vote: Vote<NodeId>,
        snapshot_data: Vec<u8>,
    ) -> Result<SnapshotResponse<NodeId>, RaftNetworkError> {
        let url = format!("{}/raft/full-snapshot", normalize_addr(target));

        #[derive(Serialize)]
        struct FullSnapshotRequest {
            vote: Vote<NodeId>,
            snapshot_data: Vec<u8>,
        }

        let request = FullSnapshotRequest {
            vote,
            snapshot_data,
        };

        let response = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    RaftNetworkError::Timeout
                } else if e.is_connect() {
                    RaftNetworkError::Unreachable(format!("Failed to connect to {}: {}", url, e))
                } else {
                    RaftNetworkError::Http(e)
                }
            })?;

        if !response.status().is_success() {
            return Err(RaftNetworkError::InvalidResponse(format!(
                "HTTP {}: {}",
                response.status(),
                response.text().await.unwrap_or_default()
            )));
        }

        response
            .json()
            .await
            .map_err(|e| RaftNetworkError::Serialization(e.to_string()))
    }

    /// Get the configured timeout
    pub fn timeout(&self) -> Duration {
        self.timeout
    }
}

impl Default for RaftHttpClient {
    fn default() -> Self {
        Self::new(5000) // 5 second default timeout
    }
}

/// Normalize an address to ensure it has a scheme
///
/// If the address doesn't start with "http://" or "https://",
/// prepend "http://" to it.
fn normalize_addr(addr: &str) -> String {
    if addr.starts_with("http://") || addr.starts_with("https://") {
        addr.to_string()
    } else {
        format!("http://{}", addr)
    }
}

/// Convert a RaftNetworkError to an OpenRaft RPCError
///
/// This helper function maps our internal network errors to the error type
/// expected by OpenRaft's RaftNetwork trait.
pub fn network_error_to_rpc_error<E>(
    error: RaftNetworkError,
) -> RPCError<NodeId, BasicNode, RaftError<NodeId, E>>
where
    E: std::error::Error,
{
    let io_error = std::io::Error::other(error.to_string());
    RPCError::Unreachable(Unreachable::new(&io_error))
}

/// Convert a RaftNetworkError to an OpenRaft RPCError for InstallSnapshot operations
///
/// This is a specialized version for the install_snapshot RPC which has
/// a different error type parameter.
pub fn network_error_to_install_snapshot_rpc_error(
    error: RaftNetworkError,
) -> RPCError<NodeId, BasicNode, RaftError<NodeId, InstallSnapshotError>> {
    network_error_to_rpc_error(error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_addr() {
        assert_eq!(
            normalize_addr("192.168.1.10:9000"),
            "http://192.168.1.10:9000"
        );
        assert_eq!(
            normalize_addr("http://192.168.1.10:9000"),
            "http://192.168.1.10:9000"
        );
        assert_eq!(
            normalize_addr("https://192.168.1.10:9000"),
            "https://192.168.1.10:9000"
        );
    }

    #[test]
    fn test_client_creation() {
        let client = RaftHttpClient::new(3000);
        assert_eq!(client.timeout(), Duration::from_millis(3000));
    }

    #[test]
    fn test_default_client() {
        let client = RaftHttpClient::default();
        assert_eq!(client.timeout(), Duration::from_millis(5000));
    }
}
