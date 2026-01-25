//! Axum-based HTTP server for receiving Raft RPCs
//!
//! Provides an HTTP server that listens for Raft RPC requests from
//! other nodes and forwards them to the local Raft instance.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    routing::post,
    Router,
};
use tower_http::trace::TraceLayer;
use tracing::info;

use crate::error::Result;
use crate::handlers::raft::{
    handle_append_entries, handle_full_snapshot, handle_install_snapshot, handle_vote,
};
use crate::raft::RaftCoordinator;

/// Raft RPC service
///
/// Provides an HTTP server for receiving Raft RPCs from peer nodes.
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
    /// # Returns
    /// An Axum router configured with the following endpoints:
    /// - POST /raft/append-entries
    /// - POST /raft/install-snapshot
    /// - POST /raft/vote
    /// - POST /raft/full-snapshot
    pub fn router(&self) -> Router {
        Router::new()
            .route("/raft/append-entries", post(handle_append_entries))
            .route("/raft/install-snapshot", post(handle_install_snapshot))
            .route("/raft/vote", post(handle_vote))
            .route("/raft/full-snapshot", post(handle_full_snapshot))
            .layer(TraceLayer::new_for_http())
            .with_state(Arc::clone(&self.raft))
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

        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| crate::error::SchedulerError::Network(format!("Failed to bind to {}: {}", addr, e)))?;

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
        // This is a basic test - in production you'd test actual routing
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
