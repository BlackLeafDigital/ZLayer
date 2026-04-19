//! Docker Engine API socket emulation.
//!
//! Provides a Docker-compatible HTTP API over a Unix domain socket,
//! allowing tools like VS Code Docker extension, CI systems, and
//! other Docker-aware tools to interact with `ZLayer`.

mod containers;
mod images;
mod networks;
mod system;
mod types;
mod volumes;

use std::path::Path;
use std::sync::Arc;

use axum::Router;
use zlayer_client::DaemonClient;

/// Shared state passed to Docker API socket handlers.
///
/// Carries a process-wide [`DaemonClient`] so each handler can dispatch
/// to the local daemon without re-opening the Unix socket per request.
#[derive(Clone)]
pub(crate) struct SocketState {
    pub(crate) client: Arc<DaemonClient>,
}

/// Start the Docker API socket server.
///
/// Listens on the given Unix socket path and routes Docker Engine API
/// requests to `ZLayer`'s runtime. Connects to the local zlayer daemon
/// (auto-starting it if needed) so that handlers can proxy requests via
/// [`DaemonClient`].
///
/// # Errors
///
/// Returns an error if the socket cannot be bound, the daemon cannot be
/// contacted, or the server fails.
pub async fn serve(socket_path: &Path) -> anyhow::Result<()> {
    // Remove stale socket file if it exists from a previous run
    if socket_path.exists() {
        tokio::fs::remove_file(socket_path).await?;
    }

    // Ensure the parent directory exists
    if let Some(parent) = socket_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let client = DaemonClient::connect().await?;
    let state = SocketState {
        client: Arc::new(client),
    };

    let app = router(state);

    tracing::info!("Docker API socket listening on {}", socket_path.display());

    let listener = tokio::net::UnixListener::bind(socket_path)?;

    axum::serve(listener, app.into_make_service()).await?;

    Ok(())
}

/// Build the Docker Engine API router.
fn router(state: SocketState) -> Router {
    Router::new()
        .merge(containers::routes(state.clone()))
        .merge(images::routes(state))
        .merge(volumes::routes())
        .merge(networks::routes())
        .merge(system::routes())
}
