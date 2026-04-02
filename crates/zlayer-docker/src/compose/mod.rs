//! Docker Compose file parsing and conversion to `ZLayer` deployment specs.

mod convert;
mod types;

#[cfg(test)]
mod tests;

pub use convert::compose_to_deployment;
pub use types::*;

use crate::Result;

/// Parse a docker-compose.yaml string into a `ComposeFile`.
///
/// # Errors
///
/// Returns an error if the YAML is malformed or does not match the
/// expected Docker Compose schema.
pub fn parse_compose(yaml: &str) -> Result<ComposeFile> {
    let compose: ComposeFile = serde_yaml::from_str(yaml)?;
    Ok(compose)
}
