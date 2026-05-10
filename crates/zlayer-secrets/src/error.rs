//! Error type for secrets operations.
//!
//! The canonical definition lives in [`zlayer_types::secrets::error`]; this
//! module re-exports it for backward compatibility with existing call sites
//! that import `zlayer_secrets::SecretsError` / `zlayer_secrets::Result`.

pub use zlayer_types::secrets::error::{Result, SecretsError};
