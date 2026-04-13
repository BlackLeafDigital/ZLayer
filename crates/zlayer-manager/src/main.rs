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
    // Build the listen address override when --port or PORT was set
    let addr_override = if port_override {
        use leptos::config::get_configuration;
        let conf = get_configuration(None)?;
        let mut addr = conf.leptos_options.site_addr;
        addr.set_port(args.port);
        Some(addr)
    } else {
        None
    };

    let (_bound_addr, server_future) =
        zlayer_manager::server::start_server(None, addr_override).await?;

    server_future.await?;
    Ok(())
}

#[cfg(not(feature = "ssr"))]
fn main() {
    // This main function is only used when building without SSR.
    // The actual entry point for WASM is the hydrate() function in lib.rs
    panic!("This binary requires the 'ssr' feature to be enabled");
}
