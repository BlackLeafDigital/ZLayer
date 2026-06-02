//! Docker Engine API socket emulation.
//!
//! Provides a Docker-compatible HTTP API over a Unix domain socket on
//! Linux/macOS or a Windows named pipe, allowing tools like the VS Code
//! Docker extension, CI systems, and other Docker-aware tools to interact
//! with `ZLayer`.

pub mod auth;
mod build;
mod buildkit;
mod containers;
mod images;
#[cfg(unix)]
mod listener_unix;
#[cfg(windows)]
mod listener_windows;
mod manifest;
mod networks;
pub mod streaming;
mod swarm;
mod system;
mod translate;
mod trust;
mod types;
mod version_middleware;
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
/// `socket_path` is where the Docker shim listens. `daemon_socket` is
/// the path to THIS process's own zlayer daemon UDS (Unix) — needed
/// because the docker socket task runs INSIDE the daemon and must talk
/// back to it without triggering a competing auto-spawn during startup.
/// On Windows the daemon listens on TCP loopback, so `daemon_socket`
/// is ignored.
///
/// # Errors
///
/// Returns an error if the socket/pipe cannot be bound, the daemon
/// cannot be contacted, or the server fails.
pub async fn serve(socket_path: &Path, daemon_socket: &Path) -> anyhow::Result<()> {
    #[cfg(unix)]
    let client =
        DaemonClient::connect_to_no_autospawn(daemon_socket, std::time::Duration::from_secs(30))
            .await?;
    #[cfg(windows)]
    let client = {
        let _ = daemon_socket;
        DaemonClient::connect().await?
    };

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
///
/// The first seven `merge` calls register real, state-bearing handler
/// modules (`/containers`, `/images`, `/build`, `/volumes`, `/networks`,
/// `/system`, and `/swarm` -- the last bridges to `ZLayer`'s native
/// cluster primitives). The trailing three (`buildkit`, `manifest`,
/// `trust`) register stub routers that answer with a Docker-shape 501 /
/// 503 so Docker clients see a clean error instead of a 404. Each stub
/// module documents what a real implementation would need.
fn router(state: SocketState) -> Router {
    Router::new()
        .merge(containers::routes(state.clone()))
        .merge(images::routes(state.clone()))
        .merge(build::routes(state.clone()))
        .merge(volumes::routes(state.clone()))
        .merge(networks::routes(state.clone()))
        .merge(system::routes(state.clone()))
        .merge(buildkit::routes())
        .merge(swarm::routes(state))
        .merge(manifest::routes())
        .merge(trust::routes())
        .layer(axum::middleware::from_fn(version_middleware::strip_version))
}
