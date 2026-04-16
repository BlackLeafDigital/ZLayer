//! `ZLayer` Manager - Leptos 0.8 WASM Application
//!
//! This crate provides the management interface for `ZLayer` container orchestration.
//!
//! Compilation modes:
//! - `ssr` feature: Server-side rendering (server binary)
//! - `hydrate` feature: Client-side hydration (WASM)

// Leptos 0.8's tachys view builder nests deeply-parameterised generics; pages
// with many nested DaisyUI modals (Secrets, Projects) overflow the default
// 128 limit under `--all-features` where both `ssr` and `hydrate` codegen
// paths materialise simultaneously.
#![recursion_limit = "512"]
// Allow clippy lints that are noisy for Leptos component code
#![allow(clippy::must_use_candidate)]
#![allow(clippy::doc_markdown)]

#[cfg(feature = "ssr")]
pub mod api_client;
pub mod app;
pub mod wire;

/// Hydrate the client-side application.
///
/// Called from JavaScript after WASM module loads to make server-rendered HTML interactive.
#[cfg(feature = "hydrate")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn hydrate() {
    console_error_panic_hook::set_once();
    leptos::mount::hydrate_body(app::App);
}
