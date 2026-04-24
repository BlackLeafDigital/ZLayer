//! Docker Engine API socket emulation.
//!
//! Provides a Docker-compatible HTTP API over a Unix domain socket on
//! Linux/macOS or a Windows named pipe, allowing tools like the VS Code
//! Docker extension, CI systems, and other Docker-aware tools to interact
//! with `ZLayer`.

mod containers;
mod images;
#[cfg(unix)]
mod listener_unix;
#[cfg(windows)]
mod listener_windows;
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
/// Dispatches to the Unix-domain-socket transport on Linux/macOS or
/// the named-pipe transport on Windows.
///
/// # Errors
///
/// Returns an error if the socket/pipe cannot be bound, the daemon
/// cannot be contacted, or the server fails.
pub async fn serve(socket_path: &Path) -> anyhow::Result<()> {
    let client = DaemonClient::connect().await?;
    let state = SocketState {
        client: Arc::new(client),
    };
    let app = router(state);

    #[cfg(unix)]
    {
        listener_unix::serve_on(socket_path, app).await
    }
    #[cfg(windows)]
    {
        listener_windows::serve_on(socket_path, app).await
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (socket_path, app);
        anyhow::bail!("platform not supported for Docker API server");
    }
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
