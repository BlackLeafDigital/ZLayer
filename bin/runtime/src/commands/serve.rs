use anyhow::{Context, Result};
use tracing::{info, warn};

/// Start the API server
pub(crate) async fn serve(bind: &str, jwt_secret: Option<String>, no_swagger: bool) -> Result<()> {
    let jwt_secret = jwt_secret.unwrap_or_else(|| {
        warn!("Using default JWT secret - NOT SAFE FOR PRODUCTION");
        "CHANGE_ME_IN_PRODUCTION".to_string()
    });

    let bind_addr = bind
        .parse()
        .context(format!("Invalid bind address: {}", bind))?;

    let config = zlayer_api::ApiConfig {
        bind: bind_addr,
        jwt_secret,
        swagger_enabled: !no_swagger,
        ..Default::default()
    };

    info!(
        bind = %config.bind,
        swagger = config.swagger_enabled,
        "Starting ZLayer API server"
    );

    let server = zlayer_api::ApiServer::new(config);

    // Setup graceful shutdown on SIGTERM/SIGINT
    let shutdown = async {
        let ctrl_c = async {
            tokio::signal::ctrl_c()
                .await
                .expect("Failed to install Ctrl+C handler");
        };

        #[cfg(unix)]
        let terminate = async {
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("Failed to install SIGTERM handler")
                .recv()
                .await;
        };

        #[cfg(not(unix))]
        let terminate = std::future::pending::<()>();

        tokio::select! {
            _ = ctrl_c => {},
            _ = terminate => {},
        }

        info!("Shutdown signal received, starting graceful shutdown");
    };

    server.run_with_shutdown(shutdown).await?;

    info!("Server shutdown complete");
    Ok(())
}
