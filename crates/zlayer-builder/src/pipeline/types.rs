//! ZPipeline types - YAML-based multi-image build pipeline format
//!
//! This module defines all serde-deserializable types for the ZPipeline format,
//! which coordinates building multiple container images from a single manifest.
//! The format supports:
//!
//! - **Global variables** - Template substitution via `${VAR}` syntax
//! - **Default settings** - Shared configuration inherited by all images
//! - **Dependency ordering** - `depends_on` declares build order constraints
//! - **Coordinated push** - Push all images after successful builds

use std::collections::HashMap;
use std::path::PathBuf;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Top-level ZPipeline
// ---------------------------------------------------------------------------

/// Top-level ZPipeline.yaml representation.
///
/// A pipeline coordinates building multiple container images with dependency
/// ordering, shared configuration, and coordinated push operations.
///
/// # YAML Example
///
/// ```yaml
/// version: "1"
///
/// vars:
///   VERSION: "1.0.0"
///   REGISTRY: "ghcr.io/myorg"
///
/// defaults:
///   format: oci
///   build_args:
///     RUST_VERSION: "1.90"
///
/// images:
///   base:
///     file: docker/Dockerfile.base
///     tags:
///       - "${REGISTRY}/base:${VERSION}"
///   app:
///     file: docker/Dockerfile.app
///     depends_on: [base]
///     tags:
///       - "${REGISTRY}/app:${VERSION}"
///
/// push:
///   after_all: true
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ZPipeline {
    /// Pipeline format version (currently "1")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,

    /// Global variables for template substitution.
    /// Referenced in tags and other string fields as `${VAR_NAME}`.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub vars: HashMap<String, String>,

    /// Default settings inherited by all images.
    /// Individual image settings override these defaults.
    #[serde(default)]
    pub defaults: PipelineDefaults,

    /// Named images to build. Order is preserved (IndexMap).
    /// Keys are image names used for `depends_on` references.
    pub images: IndexMap<String, PipelineImage>,

    /// Push configuration.
    #[serde(default)]
    pub push: PushConfig,
}

// ---------------------------------------------------------------------------
// Pipeline defaults
// ---------------------------------------------------------------------------

/// Default settings applied to all images in the pipeline.
///
/// Individual image settings take precedence over these defaults.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PipelineDefaults {
    /// Default output format: "oci" or "docker"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,

    /// Default build arguments passed to all image builds
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub build_args: HashMap<String, String>,

    /// Whether to skip cache by default
    #[serde(default, skip_serializing_if = "is_false")]
    pub no_cache: bool,
}

// ---------------------------------------------------------------------------
// Pipeline image
// ---------------------------------------------------------------------------

/// Configuration for a single image in the pipeline.
///
/// # YAML Example
///
/// ```yaml
/// zlayer-app:
///   file: docker/ZImagefile.app
///   context: "."
///   tags:
///     - "${REGISTRY}/app:${VERSION}"
///     - "${REGISTRY}/app:latest"
///   depends_on: [zlayer-base]
///   build_args:
///     EXTRA_ARG: "value"
///   no_cache: false
///   format: oci
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PipelineImage {
    /// Path to the build file (Dockerfile, ZImagefile, etc.)
    pub file: PathBuf,

    /// Build context directory. Defaults to "."
    #[serde(default = "default_context", skip_serializing_if = "is_default_context")]
    pub context: PathBuf,

    /// Image tags to apply. Supports variable substitution.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,

    /// Build arguments specific to this image.
    /// Merged with (and overrides) defaults.build_args.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub build_args: HashMap<String, String>,

    /// Names of images that must be built before this one.
    /// Creates a dependency graph for build ordering.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,

    /// Override no_cache setting for this image.
    /// If None, inherits from defaults.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub no_cache: Option<bool>,

    /// Override output format for this image.
    /// If None, inherits from defaults.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
}

// ---------------------------------------------------------------------------
// Push configuration
// ---------------------------------------------------------------------------

/// Configuration for pushing built images to registries.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PushConfig {
    /// If true, push all images only after all builds succeed.
    /// If false (default), images are not pushed automatically.
    #[serde(default, skip_serializing_if = "is_false")]
    pub after_all: bool,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Default build context directory.
fn default_context() -> PathBuf {
    PathBuf::from(".")
}

/// Check if path is the default context.
fn is_default_context(path: &PathBuf) -> bool {
    *path == PathBuf::from(".")
}

/// Helper for `skip_serializing_if` on boolean fields.
fn is_false(v: &bool) -> bool {
    !v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pipeline_image_defaults() {
        let yaml = r#"
file: Dockerfile
"#;
        let img: PipelineImage = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(img.file, PathBuf::from("Dockerfile"));
        assert_eq!(img.context, PathBuf::from("."));
        assert!(img.tags.is_empty());
        assert!(img.build_args.is_empty());
        assert!(img.depends_on.is_empty());
        assert!(img.no_cache.is_none());
        assert!(img.format.is_none());
    }

    #[test]
    fn test_pipeline_defaults_empty() {
        let yaml = "{}";
        let defaults: PipelineDefaults = serde_yaml::from_str(yaml).unwrap();
        assert!(defaults.format.is_none());
        assert!(defaults.build_args.is_empty());
        assert!(!defaults.no_cache);
    }

    #[test]
    fn test_pipeline_defaults_full() {
        let yaml = r#"
format: oci
build_args:
  RUST_VERSION: "1.90"
no_cache: true
"#;
        let defaults: PipelineDefaults = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(defaults.format, Some("oci".to_string()));
        assert_eq!(
            defaults.build_args.get("RUST_VERSION"),
            Some(&"1.90".to_string())
        );
        assert!(defaults.no_cache);
    }

    #[test]
    fn test_push_config_defaults() {
        let yaml = "{}";
        let push: PushConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(!push.after_all);
    }

    #[test]
    fn test_push_config_after_all() {
        let yaml = "after_all: true";
        let push: PushConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(push.after_all);
    }

    #[test]
    fn test_deny_unknown_fields_pipeline_image() {
        let yaml = r#"
file: Dockerfile
unknown_field: "should fail"
"#;
        let result: Result<PipelineImage, _> = serde_yaml::from_str(yaml);
        assert!(result.is_err(), "Should reject unknown fields");
    }

    #[test]
    fn test_deny_unknown_fields_pipeline_defaults() {
        let yaml = r#"
format: oci
bogus: "nope"
"#;
        let result: Result<PipelineDefaults, _> = serde_yaml::from_str(yaml);
        assert!(result.is_err(), "Should reject unknown fields");
    }

    #[test]
    fn test_deny_unknown_fields_push_config() {
        let yaml = r#"
after_all: true
extra: "bad"
"#;
        let result: Result<PushConfig, _> = serde_yaml::from_str(yaml);
        assert!(result.is_err(), "Should reject unknown fields");
    }

    #[test]
    fn test_serialization_skips_defaults() {
        let img = PipelineImage {
            file: PathBuf::from("Dockerfile"),
            context: PathBuf::from("."),
            tags: vec![],
            build_args: HashMap::new(),
            depends_on: vec![],
            no_cache: None,
            format: None,
        };
        let serialized = serde_yaml::to_string(&img).unwrap();
        // Should only contain "file" since everything else is default/empty
        assert!(serialized.contains("file:"));
        assert!(!serialized.contains("context:"));
        assert!(!serialized.contains("tags:"));
        assert!(!serialized.contains("build_args:"));
        assert!(!serialized.contains("depends_on:"));
        assert!(!serialized.contains("no_cache:"));
        assert!(!serialized.contains("format:"));
    }

    #[test]
    fn test_serialization_includes_non_defaults() {
        let img = PipelineImage {
            file: PathBuf::from("Dockerfile"),
            context: PathBuf::from("./subdir"),
            tags: vec!["myimage:latest".to_string()],
            build_args: HashMap::from([("KEY".to_string(), "value".to_string())]),
            depends_on: vec!["base".to_string()],
            no_cache: Some(true),
            format: Some("docker".to_string()),
        };
        let serialized = serde_yaml::to_string(&img).unwrap();
        assert!(serialized.contains("context:"));
        assert!(serialized.contains("tags:"));
        assert!(serialized.contains("build_args:"));
        assert!(serialized.contains("depends_on:"));
        assert!(serialized.contains("no_cache:"));
        assert!(serialized.contains("format:"));
    }
}
