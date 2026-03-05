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
    use clap::{CommandFactory, FromArgMatches};
    use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

    use cli::{port_explicitly_set, Args};

    // Initialize tracing
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "zlayer_manager=debug,tower_http=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    // Parse CLI arguments, keeping the raw ArgMatches so we can detect
    // whether --port was explicitly provided vs. falling back to the default.
    let matches = Args::command().get_matches();
    let port_override = port_explicitly_set(&matches);
    let args = Args::from_arg_matches(&matches).expect("CLI argument parsing failed");

    tracing::info!("Starting ZLayer Manager Server on port {}", args.port);

    if let Err(e) = start_server(&args, port_override).await {
        tracing::error!("Server error: {}", e);
        std::process::exit(1);
    }
}

/// Start the Leptos SSR server.
///
/// Configuration is loaded from `LEPTOS_*` env vars (set automatically by
/// `cargo-leptos`) via [`leptos::config::get_configuration`].  The CLI
/// `--port` / `PORT` env var can override the listen port when explicitly
/// provided.
#[cfg(feature = "ssr")]
async fn start_server(
    args: &cli::Args,
    port_override: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    use axum::{routing::get, Router};
    use leptos::config::get_configuration;
    use leptos_axum::{generate_route_list, LeptosRoutes};
    use std::net::SocketAddr;
    use tower_http::services::ServeDir;
    use zlayer_manager::app::shell;

    // Load Leptos configuration from LEPTOS_* env vars (cargo-leptos sets
    // these from [package.metadata.leptos] in Cargo.toml).  Passing `None`
    // means "don't read Cargo.toml directly; rely on env vars / defaults".
    let conf = get_configuration(None).expect(
        "Failed to load Leptos configuration. \
         Set LEPTOS_OUTPUT_NAME env var or run via `cargo leptos watch`.",
    );
    let mut leptos_options = conf.leptos_options;

    // If the user explicitly passed --port or set PORT, override the
    // address Leptos will listen on while keeping the IP from config.
    if port_override {
        let mut addr = leptos_options.site_addr;
        addr.set_port(args.port);
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

    tracing::info!(
        "ZLayer Manager Server listening on http://{}",
        SocketAddr::from(addr)
    );

    axum::serve(listener, app).await?;

    Ok(())
}

#[cfg(not(feature = "ssr"))]
fn main() {
    // This main function is only used when building without SSR.
    // The actual entry point for WASM is the hydrate() function in lib.rs
    panic!("This binary requires the 'ssr' feature to be enabled");
}
