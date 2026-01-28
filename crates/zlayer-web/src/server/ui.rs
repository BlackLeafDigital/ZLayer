//! UI server implementation with Leptos SSR + Hydration

use axum::{body::Body, http::Request, Router};
use leptos::config::{Env, LeptosOptions};
use leptos::prelude::*;
use leptos_axum::{generate_route_list, handle_server_fns_with_context, LeptosRoutes};
use std::net::SocketAddr;
use tower_http::services::ServeDir;
use tracing::info;

use crate::app::{app_css, App};

/// HTML shell wrapper that includes CSS and hydration scripts
/// The HydrationScripts component loads the WASM bundle for client-side interactivity
fn shell(options: LeptosOptions) -> impl IntoView {
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

/// Start UI server with Leptos SSR + Hydration
pub async fn start_ui_server() -> Result<(), Box<dyn std::error::Error>> {
    // Parse bind address from environment or use default
    let bind_address =
        std::env::var("ZLAYER_WEB_ADDR").unwrap_or_else(|_| "0.0.0.0:3000".to_string());

    let addr = bind_address
        .parse::<SocketAddr>()
        .map_err(|e| format!("Invalid bind address '{}': {}", bind_address, e))?;

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

    // Clone options for use in shell closure
    let options_for_shell = leptos_options.clone();
    let shell_fn = move || shell(options_for_shell.clone());

    // Generate route list for SSR
    let routes = generate_route_list(shell_fn.clone());

    // Context provider closure for server functions
    let provide_context = move || {
        // Future: provide ZLayer services as context here
        // provide_context(zlayer_agent.clone());
    };

    // Main app router with Leptos SSR + Hydration
    let leptos_router = Router::new().leptos_routes_with_context(
        &leptos_options,
        routes,
        provide_context.clone(),
        shell_fn,
    );

    // Server function handler with the same context
    let server_fn_handler = {
        let context_fn = provide_context.clone();
        move |req: Request<Body>| handle_server_fns_with_context(context_fn.clone(), req)
    };

    // Serve static files (WASM pkg directory) for client-side hydration
    let pkg_dir = format!("{}/pkg", site_root);
    info!("Serving WASM package from {}", pkg_dir);

    // Determine assets directory - relative to crate in dev, or use site root in prod
    let assets_dir = if cfg!(debug_assertions) {
        // In dev mode, assets are in the crate directory
        "crates/zlayer-web/assets".to_string()
    } else {
        // In prod, assets should be copied to site root
        format!("{}/assets", site_root)
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
