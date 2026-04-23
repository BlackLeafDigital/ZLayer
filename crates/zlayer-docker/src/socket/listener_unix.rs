//! Unix-domain-socket transport for the Docker Engine API server.

use std::path::Path;

use axum::Router;

pub async fn serve_on(socket_path: &Path, app: Router) -> anyhow::Result<()> {
    if socket_path.exists() {
        tokio::fs::remove_file(socket_path).await?;
    }
    if let Some(parent) = socket_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tracing::info!("Docker API socket listening on {}", socket_path.display());
    let listener = tokio::net::UnixListener::bind(socket_path)?;
    axum::serve(listener, app.into_make_service()).await?;
    Ok(())
}
