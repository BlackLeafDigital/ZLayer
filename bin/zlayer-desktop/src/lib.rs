//! `ZLayer` Manager Desktop Application
//!
//! Wraps the Leptos SSR web UI in a Tauri v2 native window.
//! An embedded Axum server runs on a random localhost port and
//! the webview navigates to it — all Leptos server functions
//! work unchanged.

use tauri::Manager;

/// Run the desktop application.
pub fn run() {
    // Initialize tracing so the embedded server logs are visible
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "zlayer_manager=info,zlayer_desktop=info".into()),
        )
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_window_state::Builder::new().build())
        .setup(|app| {
            let handle = app.handle().clone();

            // Spawn the embedded Leptos SSR server on a random port
            tauri::async_runtime::spawn(async move {
                let random_addr = "127.0.0.1:0".parse().expect("valid socket address literal");

                let (bound_addr, server_future) =
                    zlayer_manager::server::start_server(None, Some(random_addr))
                        .await
                        .expect("Failed to start embedded ZLayer Manager server");

                tracing::info!("Embedded server bound to http://{bound_addr}");

                // Navigate the main window to the embedded server
                if let Some(window) = handle.get_webview_window("main") {
                    let url = format!("http://{bound_addr}");
                    let _ = window.navigate(url.parse().expect("valid URL"));
                }

                // Run the server until the app exits
                if let Err(e) = server_future.await {
                    tracing::error!("Embedded server error: {e}");
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![])
        .run(tauri::generate_context!())
        .expect("error while running ZLayer Desktop");
}
