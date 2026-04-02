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

use axum::Router;

/// Start the Docker API socket server.
///
/// Listens on the given Unix socket path and routes Docker Engine API
/// requests to `ZLayer`'s runtime.
///
/// # Errors
///
/// Returns an error if the socket cannot be bound or the server fails.
pub async fn serve(socket_path: &Path) -> anyhow::Result<()> {
    // Remove stale socket file if it exists from a previous run
    if socket_path.exists() {
        tokio::fs::remove_file(socket_path).await?;
    }

    // Ensure the parent directory exists
    if let Some(parent) = socket_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let app = router();

    tracing::info!("Docker API socket listening on {}", socket_path.display());

    let listener = tokio::net::UnixListener::bind(socket_path)?;

    axum::serve(listener, app.into_make_service()).await?;

    Ok(())
}

/// Build the Docker Engine API router.
fn router() -> Router {
    Router::new()
        .merge(containers::routes())
        .merge(images::routes())
        .merge(volumes::routes())
        .merge(networks::routes())
        .merge(system::routes())
}
