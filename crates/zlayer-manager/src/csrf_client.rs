//! Browser-side CSRF echo.
//!
//! The daemon issues two cookies on `/auth/login`:
//! - `zlayer_session` — HttpOnly, the JWT.
//! - `zlayer_csrf` — JS-readable on purpose, the CSRF double-submit token.
//!
//! Mutating requests (POST/PATCH/PUT/DELETE) must echo the CSRF cookie
//! value as an `x-csrf-token` request header so
//! `crates/zlayer-api/src/middleware/csrf.rs` can verify the double-submit.
//! GET-style requests are exempt from the check, but echoing on every
//! request is harmless and keeps the wrapper trivial.
//!
//! Installs a one-shot wrapper around `window.fetch` at hydrate startup.
//! Reads the cookie on every fetch (so post-login flows pick the token
//! up immediately, with no app-level state to thread through). Idempotent
//! via a `window.__zlayer_csrf_hook_installed__` flag.

#![cfg(feature = "hydrate")]

const INSTALL_JS: &str = r#"
(function() {
    if (window.__zlayer_csrf_hook_installed__) { return; }
    window.__zlayer_csrf_hook_installed__ = true;
    var original = window.fetch.bind(window);
    window.fetch = function(input, init) {
        try {
            var cookie = document.cookie || '';
            var m = cookie.match(/(?:^|;\s*)zlayer_csrf=([^;]*)/);
            if (m) {
                init = init || {};
                var existing = init.headers
                    || (input && typeof Request !== 'undefined' && input instanceof Request
                        ? input.headers
                        : undefined);
                var headers = new Headers(existing || {});
                headers.set('x-csrf-token', m[1]);
                init.headers = headers;
            }
        } catch (e) {
            // Never break fetch on hook failure; let the original through.
        }
        return original(input, init);
    };
})();
"#;

/// Install the global `window.fetch` wrapper. Safe to call more than once.
pub fn install_fetch_csrf_hook() {
    if let Err(err) = js_sys::eval(INSTALL_JS) {
        web_sys::console::warn_2(
            &wasm_bindgen::JsValue::from_str("zlayer: CSRF fetch hook install failed"),
            &err,
        );
    }
}
