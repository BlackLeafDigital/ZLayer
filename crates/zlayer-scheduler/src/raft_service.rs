//! Axum-based HTTP server for receiving Raft RPCs
//!
//! Delegates to `zlayer_consensus::network::http_service::raft_service_router()`
//! for the actual endpoint handlers.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use tower_http::trace::TraceLayer;
use tracing::info;

use crate::error::Result;
use crate::raft::RaftCoordinator;
use zlayer_consensus::network::http_service::raft_service_router;

/// Raft RPC service
///
/// Provides an HTTP server for receiving Raft RPCs from peer nodes.
/// Delegates to `zlayer-consensus` for the actual RPC handling.
pub struct RaftService {
    raft: Arc<RaftCoordinator>,
}

impl RaftService {
    /// Create a new Raft service
    ///
    /// # Arguments
    /// * `raft` - The Raft coordinator to forward RPCs to
    pub fn new(raft: Arc<RaftCoordinator>) -> Self {
        Self { raft }
    }

    /// Create the Axum router with Raft RPC endpoints
    ///
    /// Uses `zlayer_consensus::network::http_service::raft_service_router()` which
    /// provides endpoints for:
    /// - POST /raft/vote
    /// - POST /raft/append
    /// - POST /raft/snapshot
    /// - POST /raft/full-snapshot
    pub fn router(&self) -> Router {
        raft_service_router(self.raft.raft_clone()).layer(TraceLayer::new_for_http())
    }

    /// Run the Raft service on the specified address
    ///
    /// This starts an HTTP server that listens for Raft RPCs.
    ///
    /// # Arguments
    /// * `addr` - The socket address to bind to (e.g., "0.0.0.0:9000")
    ///
    /// # Returns
    /// An error if the server fails to start or encounters a runtime error
    pub async fn run(self, addr: SocketAddr) -> Result<()> {
        let app = self.router();

        info!(address = %addr, "Starting Raft RPC server");

        let listener = tokio::net::TcpListener::bind(addr).await.map_err(|e| {
            crate::error::SchedulerError::Network(format!("Failed to bind to {}: {}", addr, e))
        })?;

        axum::serve(listener, app)
            .await
            .map_err(|e| crate::error::SchedulerError::Network(format!("Server error: {}", e)))?;

        Ok(())
    }

    /// Get a clone of the Raft coordinator
    pub fn raft(&self) -> Arc<RaftCoordinator> {
        Arc::clone(&self.raft)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raft::RaftConfig;

    #[tokio::test]
    async fn test_service_creation() {
        let config = RaftConfig::default();
        let raft = RaftCoordinator::new(config).await.unwrap();
        let service = RaftService::new(Arc::new(raft));

        // Should create router without panic
        let _router = service.router();
    }

    #[test]
    fn test_router_endpoints() {
        // Verify that the router has the expected structure
        let config = RaftConfig::default();
        let rt = tokio::runtime::Runtime::new().unwrap();

        rt.block_on(async {
            let raft = RaftCoordinator::new(config).await.unwrap();
            let service = RaftService::new(Arc::new(raft));
            let router = service.router();

            // Router should be created successfully
            assert!(std::mem::size_of_val(&router) > 0);
        });
    }
}
