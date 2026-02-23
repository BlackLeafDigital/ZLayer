//! `ZLayer` Manager Server Entry Point
//!
//! This is the main entry point for the `ZLayer` Manager web server.
//! It starts the Leptos SSR server with Axum.

#![recursion_limit = "512"]
#![allow(clippy::doc_markdown)]

#[cfg(feature = "ssr")]
mod cli;

#[cfg(feature = "ssr")]
#[tokio::main]
async fn main() {
    use clap::Parser;
    use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

    use cli::Args;

    // Initialize tracing
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "zlayer_manager=debug,tower_http=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    // Parse CLI arguments
    let args = Args::parse();
    let port = args.port;

    tracing::info!("Starting ZLayer Manager Server on port {}", port);

    if let Err(e) = start_server(port).await {
        tracing::error!("Server error: {}", e);
        std::process::exit(1);
    }
}

/// Start the Leptos SSR server on the given port.
#[cfg(feature = "ssr")]
async fn start_server(port: u16) -> Result<(), Box<dyn std::error::Error>> {
    use axum::Router;
    use leptos::config::{Env, LeptosOptions};
    use leptos_axum::{generate_route_list, LeptosRoutes};
    use std::net::SocketAddr;
    use tower_http::services::ServeDir;
    use zlayer_manager::app::shell;

    let addr = SocketAddr::from(([0, 0, 0, 0], port));

    // Determine site root from environment or use default
    let site_root = std::env::var("LEPTOS_SITE_ROOT").unwrap_or_else(|_| {
        if cfg!(debug_assertions) {
            "target/site".to_string()
        } else {
            "/app/target/site".to_string()
        }
    });

    // Leptos configuration for SSR + Hydration
    let leptos_options = LeptosOptions::builder()
        .output_name("zlayer-manager")
        .site_root(site_root.as_str())
        .site_pkg_dir("pkg")
        .site_addr(addr)
        .env(if cfg!(debug_assertions) {
            Env::DEV
        } else {
            Env::PROD
        })
        .build();

    let opts_for_route_gen = leptos_options.clone();
    let opts_for_ssr = leptos_options.clone();

    // Generate route list for SSR
    let routes = generate_route_list(move || shell(opts_for_route_gen.clone()));

    // Main app router with Leptos SSR + Hydration
    let leptos_router =
        Router::new().leptos_routes(&leptos_options, routes, move || shell(opts_for_ssr.clone()));

    // Serve static files (WASM pkg directory) for client-side hydration
    let pkg_dir = format!("{site_root}/pkg");
    tracing::info!("Serving WASM package from {}", pkg_dir);

    let app = leptos_router
        .nest_service("/pkg", ServeDir::new(&pkg_dir))
        .with_state(leptos_options);

    tracing::info!("Binding TCP listener to {}", addr);
    let listener = tokio::net::TcpListener::bind(&addr).await?;

    tracing::info!("ZLayer Manager Server listening on http://{}", addr);

    axum::serve(listener, app).await?;

    Ok(())
}

#[cfg(not(feature = "ssr"))]
fn main() {
    // This main function is only used when building without SSR.
    // The actual entry point for WASM is the hydrate() function in lib.rs
    panic!("This binary requires the 'ssr' feature to be enabled");
}
