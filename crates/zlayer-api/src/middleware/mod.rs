//! Axum middleware used by the `ZLayer` API.
//!
//! Currently provides CSRF double-submit enforcement for cookie-authed
//! sessions. Bearer-authed requests (API clients using `Authorization: Bearer`)
//! are exempt and pass through untouched.

pub mod audit;
pub mod cookies;
pub mod csrf;
