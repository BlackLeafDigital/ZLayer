//! `ZLayer` V1 Service Specification (shim).
//!
//! This crate is a thin re-export of [`zlayer_types::spec`]. It exists for
//! backward compatibility with the 14+ consumers that import from
//! `zlayer-spec` directly. New code should import from `zlayer_types::spec`
//! instead.

pub use zlayer_types::spec::*;
