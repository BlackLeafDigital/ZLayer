#![allow(clippy::missing_errors_doc, clippy::doc_markdown)]
//! OpenID Connect / SSO integration.
//!
//! Configuration comes from `ZLAYER_OIDC_<PROVIDER>_*` env vars (see
//! [`config::OidcProviderConfig::load_all`]). Discovery metadata (JWKS +
//! endpoints) is fetched lazily on first use per provider and cached in
//! [`client::OidcClient`]. The in-memory [`state::StateTokenStore`] holds
//! per-request CSRF + PKCE state for the ~minute between the browser hitting
//! `/auth/oidc/:provider/start` and coming back to `/callback`.

pub mod client;
pub mod config;
pub mod state;

pub use client::{OidcClient, OidcError};
pub use config::{OidcProviderConfig, OidcProviderPublic};
pub use state::{StateTokenPayload, StateTokenStore};
