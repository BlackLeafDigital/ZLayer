//! Registry authentication module
//!
//! This module provides authentication resolution for OCI registries, supporting
//! multiple authentication sources including Docker config.json, environment variables,
//! and explicit credentials.

pub mod docker_config;
pub mod resolver;

pub use docker_config::DockerConfigAuth;
pub use resolver::{AuthConfig, AuthResolver, AuthSource, RegistryAuthConfig};
