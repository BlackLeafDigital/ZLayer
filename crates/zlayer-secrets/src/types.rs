//! Secret data types.
//!
//! The canonical definitions live in [`zlayer_types::secrets::types`]; this
//! module re-exports them for backward compatibility with existing call
//! sites that import `zlayer_secrets::Secret`, `zlayer_secrets::SecretMetadata`,
//! etc.

pub use zlayer_types::secrets::types::{
    RotationResult, Secret, SecretMetadata, SecretRef, SecretScope,
};
