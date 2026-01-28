//! `ZLayer` Web UI with Leptos SSR + Hydration
//!
//! This crate provides the web interface for `ZLayer`, a lightweight container orchestration system.
//!
//! It supports two compilation modes:
//! - `ssr` feature: Server-side rendering mode (for the server binary)
//! - `hydrate` feature: Client-side hydration mode (for WASM)

#![recursion_limit = "512"]

// App module is always needed (shared between server and client)
pub mod app;

// Server-only modules (only compiled with SSR feature)
#[cfg(feature = "ssr")]
pub mod server;

// ============================================================================
// Client-side hydration entry point (WASM)
// ============================================================================

/// Hydrate the client-side application.
///
/// This function is called from JavaScript after the WASM module is loaded.
/// It takes over the server-rendered HTML and makes it interactive.
#[cfg(feature = "hydrate")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn hydrate() {
    // Set up panic hook for better error messages in the browser console
    console_error_panic_hook::set_once();

    // Mount the app and hydrate the server-rendered HTML
    leptos::mount::hydrate_body(app::App);
}
