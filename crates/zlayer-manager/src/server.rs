//! Embedded Leptos SSR server for `ZLayer` Manager
//!
//! This module provides the core server startup logic so it can be reused
//! by both the standalone CLI binary and the Tauri desktop wrapper.

#![allow(clippy::missing_errors_doc, clippy::missing_panics_doc)]

use std::future::IntoFuture;
use std::net::SocketAddr;

use axum::{routing::get, Router};
use leptos::config::get_configuration;
use leptos_axum::{generate_route_list, LeptosRoutes};
use tower_http::services::ServeDir;

use crate::app::shell;

/// Start the Leptos SSR server.
///
/// Binds to `addr` (use port 0 for a random available port) and returns
/// the actual bound address.  The returned future completes only when
/// the server shuts down — callers should `tokio::spawn` it when they
/// need the address back immediately.
///
/// `site_root_override` replaces the `LEPTOS_SITE_ROOT` env var when
/// `Some`.  `addr_override` replaces the `LEPTOS_SITE_ADDR` listen
/// address when `Some`.
pub async fn start_server(
    site_root_override: Option<&str>,
    addr_override: Option<SocketAddr>,
) -> Result<
    (
        SocketAddr,
        impl std::future::Future<Output = Result<(), std::io::Error>>,
    ),
    Box<dyn std::error::Error>,
> {
    let conf = get_configuration(None).expect(
        "Failed to load Leptos configuration. \
         Set LEPTOS_OUTPUT_NAME env var or run via `cargo leptos watch`.",
    );
    let mut leptos_options = conf.leptos_options;

    // Apply overrides
    if let Some(root) = site_root_override {
        leptos_options.site_root = root.into();
    }
    if let Some(addr) = addr_override {
        leptos_options.site_addr = addr;
    }

    // Production fallback: if site_root is the default "target/site" but
    // doesn't exist on disk and the Docker-standard "/app/target/site"
    // does, use that instead.
    {
        let root: &str = &leptos_options.site_root;
        if root == "target/site"
            && !std::path::Path::new(root).exists()
            && std::path::Path::new("/app/target/site").exists()
        {
            tracing::info!("site_root \"target/site\" not found; using \"/app/target/site\"");
            leptos_options.site_root = "/app/target/site".into();
        }
    }

    let addr = leptos_options.site_addr;
    let site_root = leptos_options.site_root.to_string();

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

    let assets_dir = format!("{site_root}/assets");
    tracing::info!("Serving static assets from {}", assets_dir);

    let app = leptos_router
        .route("/health", get(|| async { "ok" }))
        .nest_service("/pkg", ServeDir::new(&pkg_dir))
        .nest_service("/assets", ServeDir::new(&assets_dir))
        .with_state(leptos_options);

    tracing::info!("Binding TCP listener to {}", addr);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    let bound_addr = listener.local_addr()?;

    tracing::info!("ZLayer Manager Server listening on http://{}", bound_addr);

    let server_future = axum::serve(listener, app).into_future();

    Ok((bound_addr, server_future))
}
