//! `ZLayer` Web Server Entry Point
//!
//! This is the main entry point for the `ZLayer` web server.
//! It starts the Leptos SSR server with Axum.

#![recursion_limit = "512"]

#[cfg(feature = "ssr")]
#[tokio::main]
async fn main() {
    use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

    // Initialize tracing
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "zlayer_web=debug,tower_http=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    tracing::info!("Starting ZLayer Web Server");

    if let Err(e) = zlayer_web::server::ui::start_ui_server().await {
        tracing::error!("Server error: {}", e);
        std::process::exit(1);
    }
}

#[cfg(not(feature = "ssr"))]
fn main() {
    // This main function is only used when building without SSR.
    // The actual entry point for WASM is the hydrate() function in lib.rs
    panic!("This binary requires the 'ssr' feature to be enabled");
}
