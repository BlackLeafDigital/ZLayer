//! UI server implementation with Leptos SSR + Hydration

use axum::{body::Body, http::Request, Router};
use leptos::config::{Env, LeptosOptions};
use leptos::prelude::*;
use leptos_axum::{generate_route_list, handle_server_fns_with_context, LeptosRoutes};
use std::net::SocketAddr;
use std::sync::Arc;
use tower_http::services::ServeDir;
use tracing::info;

use crate::app::{app_css, App};
use crate::server::state::WebState;

/// HTML shell wrapper that includes CSS and hydration scripts.
///
/// The `HydrationScripts` component loads the WASM bundle for client-side interactivity.
fn shell(options: &LeptosOptions) -> impl IntoView {
    use leptos::hydration::HydrationScripts;

    view! {
        <!DOCTYPE html>
        <html lang="en">
            <head>
                <meta charset="UTF-8"/>
                <meta name="viewport" content="width=device-width, initial-scale=1.0"/>
                <meta name="description" content="ZLayer - Lightweight container orchestration for modern infrastructure"/>
                <title>"ZLayer - Lightweight Container Orchestration"</title>

                // Favicon
                <link rel="icon" type="image/svg+xml" href="data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24' fill='none' stroke='%233b82f6' stroke-width='2'%3E%3Cpath d='M21 16V8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16z'/%3E%3Cpolyline points='3.27 6.96 12 12.01 20.73 6.96'/%3E%3Cline x1='12' y1='22.08' x2='12' y2='12'/%3E%3C/svg%3E"/>

                // Inline CSS for fast first paint
                <style>{app_css()}</style>

                // Load WASM and hydration scripts for client-side interactivity
                <HydrationScripts options=options.clone()/>
            </head>
            <body>
                <App/>
            </body>
        </html>
    }
}

/// Start UI server with Leptos SSR + Hydration.
///
/// # Arguments
///
/// * `web_state` - Optional shared state for server functions. If `None`, a default
///   `WebState` with no service manager or metrics collector will be created.
///
/// # Errors
///
/// Returns an error if:
/// - The bind address is invalid or cannot be parsed
/// - The TCP listener fails to bind to the address
/// - The server encounters a runtime error
pub async fn start_ui_server(
    web_state: Option<WebState>,
) -> Result<(), Box<dyn std::error::Error>> {
    // Create or use provided WebState, wrapped in Arc for sharing across handlers
    let state = Arc::new(web_state.unwrap_or_default());
    // Parse bind address from environment or use default
    let bind_address =
        std::env::var("ZLAYER_WEB_ADDR").unwrap_or_else(|_| "0.0.0.0:3000".to_string());

    let addr = bind_address
        .parse::<SocketAddr>()
        .map_err(|e| format!("Invalid bind address '{bind_address}': {e}"))?;

    info!("Starting ZLayer Web Server on {}", addr);

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
        .output_name("zlayer-web")
        .site_root(site_root.as_str())
        .site_pkg_dir("pkg")
        .site_addr(addr)
        .env(if cfg!(debug_assertions) {
            Env::DEV
        } else {
            Env::PROD
        })
        .build();

    // Clone options for use in shell closures
    let opts_for_route_gen = leptos_options.clone();
    let opts_for_ssr = leptos_options.clone();

    // Generate route list for SSR
    let routes = generate_route_list(move || shell(&opts_for_route_gen));

    // Context provider closure for server functions
    // Clone state for use in closures - need separate clones for routes and server fn handler
    let state_for_routes = state.clone();
    let state_for_server_fns = state.clone();

    let provide_context_for_routes = move || {
        // Provide WebState as Leptos context for server functions
        leptos::prelude::provide_context(state_for_routes.clone());
    };

    let provide_context_for_server_fns = move || {
        // Provide WebState as Leptos context for server functions
        leptos::prelude::provide_context(state_for_server_fns.clone());
    };

    // Main app router with Leptos SSR + Hydration
    let leptos_router = Router::new().leptos_routes_with_context(
        &leptos_options,
        routes,
        provide_context_for_routes,
        move || shell(&opts_for_ssr),
    );

    // Server function handler with the same context
    let server_fn_handler = {
        move |req: Request<Body>| {
            handle_server_fns_with_context(provide_context_for_server_fns, req)
        }
    };

    // Serve static files (WASM pkg directory) for client-side hydration
    let pkg_dir = format!("{site_root}/pkg");
    info!("Serving WASM package from {}", pkg_dir);

    // Determine assets directory - relative to crate in dev, or use site root in prod
    let assets_dir = if cfg!(debug_assertions) {
        // In dev mode, assets are in the crate directory
        "crates/zlayer-web/assets".to_string()
    } else {
        // In prod, assets should be copied to site root
        format!("{site_root}/assets")
    };
    info!("Serving static assets from {}", assets_dir);

    let app = leptos_router
        .route(
            "/api/leptos/{*fn_name}",
            axum::routing::post(server_fn_handler),
        )
        // Serve the WASM package directory for client-side hydration
        .nest_service("/pkg", ServeDir::new(&pkg_dir))
        // Serve static assets (images, fonts, etc.)
        .nest_service("/assets", ServeDir::new(&assets_dir))
        .with_state(leptos_options);

    info!("Binding TCP listener to {}", addr);
    let listener = tokio::net::TcpListener::bind(&addr).await?;

    info!("ZLayer Web Server listening on http://{}", addr);
    info!("  Homepage: http://{}/", addr);
    info!("  Documentation: http://{}/docs", addr);
    info!("  Playground: http://{}/playground", addr);

    axum::serve(listener, app).await?;

    Ok(())
}
