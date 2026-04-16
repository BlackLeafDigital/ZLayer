//! Wire types for manager server functions.
//!
//! Types here are simple `#[derive(Serialize, Deserialize, Clone, Debug)]`
//! structs shared between SSR and hydrate builds. They must NOT depend on
//! `zlayer-api` — hydrate compiles as WASM and has no access to that crate.

pub mod audit;
pub mod common;
pub mod groups;
pub mod notifiers;
pub mod permissions;
pub mod projects;
pub mod secrets;
pub mod tasks;
pub mod variables;
pub mod workflows;
// Other page-specific modules will be added as each page lands.
