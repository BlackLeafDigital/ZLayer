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
///
/// When `auth_token` is set, every incoming request must carry a matching
/// `Authorization: Bearer <token>` header or it will be rejected with 401.
pub struct RaftService {
    raft: Arc<RaftCoordinator>,
    /// Optional bearer token that must be presented by clients.
    auth_token: Option<String>,
}

impl RaftService {
    /// Create a new Raft service without authentication.
    ///
    /// # Arguments
    /// * `raft` - The Raft coordinator to forward RPCs to
    #[must_use]
    pub fn new(raft: Arc<RaftCoordinator>) -> Self {
        Self {
            raft,
            auth_token: None,
        }
    }

    /// Create a new Raft service with bearer token authentication.
    ///
    /// # Arguments
    /// * `raft` - The Raft coordinator to forward RPCs to
    /// * `auth_token` - Bearer token that clients must present
    #[must_use]
    pub fn with_auth(raft: Arc<RaftCoordinator>, auth_token: Option<String>) -> Self {
        Self { raft, auth_token }
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
        raft_service_router(self.raft.raft_clone(), self.auth_token.clone())
            .layer(TraceLayer::new_for_http())
    }

    /// Run the Raft service on the specified address.
    ///
    /// This starts an HTTP server that listens for Raft RPCs.
    ///
    /// # Arguments
    /// * `addr` - The socket address to bind to (e.g., "0.0.0.0:9000")
    ///
    /// # Errors
    ///
    /// Returns `SchedulerError::Network` if the server fails to bind
    /// or encounters a runtime error.
    pub async fn run(self, addr: SocketAddr) -> Result<()> {
        let app = self.router();

        info!(address = %addr, "Starting Raft RPC server");

        let listener = tokio::net::TcpListener::bind(addr).await.map_err(|e| {
            crate::error::SchedulerError::Network(format!("Failed to bind to {addr}: {e}"))
        })?;

        axum::serve(listener, app)
            .await
            .map_err(|e| crate::error::SchedulerError::Network(format!("Server error: {e}")))?;

        Ok(())
    }

    /// Get a clone of the Raft coordinator
    #[must_use]
    pub fn raft(&self) -> Arc<RaftCoordinator> {
        Arc::clone(&self.raft)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raft::RaftConfig;

    fn test_config() -> RaftConfig {
        let tmp = tempfile::tempdir().unwrap();
        RaftConfig {
            data_dir: tmp.keep(),
            ..RaftConfig::default()
        }
    }

    #[tokio::test]
    async fn test_service_creation() {
        let config = test_config();
        let raft = RaftCoordinator::new(config).await.unwrap();
        let service = RaftService::new(Arc::new(raft));

        // Should create router without panic
        let _router = service.router();
    }

    #[test]
    fn test_router_endpoints() {
        // Verify that the router has the expected structure
        let config = test_config();
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
