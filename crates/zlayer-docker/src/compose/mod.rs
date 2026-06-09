//! Docker Compose file parsing and conversion to `ZLayer` deployment specs.

pub mod build;
mod convert;
pub mod env_source;
pub mod extends;
pub mod include;
pub mod interpolate;
pub mod merge;
pub mod profiles;
mod types;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

pub use convert::{build_image_tag, compose_to_deployment};
pub use extends::{resolve_extends, ExtendsError};
pub use include::{resolve_includes, IncludeError};
pub use merge::{
    merge_compose_files, merge_compose_files_to_value, merge_values, resolve_paths, MergeError,
};
pub use types::*;

use crate::Result;

/// Parse a docker-compose.yaml string into a `ComposeFile`.
///
/// Resolves same-file `extends:` directives so child services inherit from
/// their parents. Cross-file `extends:` and the top-level `include:`
/// directive both require a base path and are skipped here; use
/// [`parse_compose_file`] when those features are needed.
///
/// # Errors
///
/// Returns an error if the YAML is malformed, does not match the expected
/// Docker Compose schema, or contains an unresolvable same-file `extends:`
/// reference.
pub fn parse_compose(yaml: &str) -> Result<ComposeFile> {
    let mut compose: ComposeFile = serde_yaml::from_str(yaml)?;
    resolve_extends(&mut compose, None)
        .map_err(|e| crate::DockerError::ComposeParse(e.to_string()))?;
    Ok(compose)
}

/// Parse a docker-compose.yaml file from disk, resolving the top-level
/// `include:` directive and `extends:` (both same-file and cross-file).
///
/// `extends:` and `include:` both reference external files via paths
/// relative to the compose file's directory, so this entry point requires a
/// concrete `Path` rather than a YAML string.
///
/// # Errors
///
/// Returns a [`crate::DockerError::ComposeParse`] when:
/// * The file cannot be read or parsed as YAML.
/// * An `include:` entry references a missing or malformed file.
/// * An `extends:` entry cannot be resolved to a known service.
pub fn parse_compose_file(path: &Path) -> Result<ComposeFile> {
    let mut compose =
        resolve_includes(path).map_err(|e| crate::DockerError::ComposeParse(e.to_string()))?;
    resolve_extends(&mut compose, path.parent())
        .map_err(|e| crate::DockerError::ComposeParse(e.to_string()))?;
    Ok(compose)
}

/// Parse and merge multiple compose files into a single [`ComposeFile`].
///
/// Files are processed in the order supplied; later files override earlier
/// ones per the Compose spec merge rules (mappings recurse, sequences
/// replace, scalars take the latter value). See [`merge::merge_compose_files`]
/// for the full rule set.
///
/// After merging, top-level `include:` directives and `extends:` references
/// are resolved using the directory of the first file as the base for
/// relative paths.
///
/// # Errors
///
/// Returns a [`crate::DockerError::ComposeParse`] wrapping the underlying
/// [`MergeError`] when:
///
/// * `files` is empty.
/// * Any file fails to read.
/// * Any file fails to parse as YAML.
/// * The merged document fails to project back into a [`ComposeFile`].
pub fn parse_compose_with_layers(files: &[PathBuf]) -> Result<ComposeFile> {
    let mut compose =
        merge_compose_files(files).map_err(|e| crate::DockerError::ComposeParse(e.to_string()))?;
    let base = files.first().and_then(|p| p.parent());
    resolve_extends(&mut compose, base)
        .map_err(|e| crate::DockerError::ComposeParse(e.to_string()))?;
    Ok(compose)
}
