//! Pipeline build support for building multiple images from a manifest
//!
//! This module provides types and execution for ZPipeline.yaml files,
//! which coordinate building multiple ZImagefiles with dependency ordering,
//! shared caches, and coordinated pushing.
//!
//! # Overview
//!
//! A ZPipeline defines:
//! - **Global variables** for template substitution (`${VAR}` syntax)
//! - **Default settings** inherited by all image builds
//! - **Multiple images** with optional dependency relationships
//! - **Push configuration** for coordinated registry operations
//!
//! # Execution Model
//!
//! The [`PipelineExecutor`] builds images in "waves" based on dependency depth:
//! - **Wave 0**: Images with no dependencies (run in parallel)
//! - **Wave 1**: Images depending only on Wave 0 images
//! - **Wave N**: Images depending only on earlier waves
//!
//! # Example
//!
//! ```yaml
//! version: "1"
//!
//! vars:
//!   VERSION: "dev"
//!   REGISTRY: "ghcr.io/myorg"
//!
//! defaults:
//!   format: oci
//!   build_args:
//!     RUST_VERSION: "1.90"
//!
//! images:
//!   base:
//!     file: docker/Dockerfile.base
//!     tags:
//!       - "${REGISTRY}/base:${VERSION}"
//!   app:
//!     file: docker/Dockerfile.app
//!     depends_on: [base]
//!     tags:
//!       - "${REGISTRY}/app:${VERSION}"
//!
//! push:
//!   after_all: true
//! ```
//!
//! # Usage
//!
//! ```no_run
//! use zlayer_builder::pipeline::{PipelineExecutor, parse_pipeline};
//! use zlayer_builder::BuildahExecutor;
//! use std::path::PathBuf;
//!
//! # async fn example() -> Result<(), zlayer_builder::BuildError> {
//! let yaml = std::fs::read_to_string("ZPipeline.yaml")?;
//! let pipeline = parse_pipeline(&yaml)?;
//!
//! let executor = BuildahExecutor::new_async().await?;
//! let result = PipelineExecutor::new(pipeline, PathBuf::from("."), executor)
//!     .fail_fast(true)
//!     .run()
//!     .await?;
//!
//! println!("Built {} images in {}ms", result.succeeded.len(), result.total_time_ms);
//! # Ok(())
//! # }
//! ```

pub mod executor;
pub mod types;

pub use executor::{PipelineExecutor, PipelineResult};
pub use types::{PipelineDefaults, PipelineImage, PushConfig, ZPipeline};

use crate::error::{BuildError, Result};

/// Parse a ZPipeline YAML file from its contents.
///
/// # Arguments
///
/// * `content` - The YAML content to parse
///
/// # Returns
///
/// The parsed `ZPipeline` or a `BuildError` if parsing fails.
///
/// # Example
///
/// ```
/// use zlayer_builder::pipeline::parse_pipeline;
///
/// let yaml = r#"
/// images:
///   app:
///     file: Dockerfile
/// "#;
///
/// let pipeline = parse_pipeline(yaml).unwrap();
/// assert!(pipeline.images.contains_key("app"));
/// ```
pub fn parse_pipeline(content: &str) -> Result<ZPipeline> {
    serde_yaml::from_str(content)
        .map_err(|e| BuildError::zimagefile_parse(format!("Pipeline parse error: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_minimal_pipeline() {
        let yaml = r#"
images:
  app:
    file: Dockerfile
"#;
        let pipeline = parse_pipeline(yaml).unwrap();
        assert_eq!(pipeline.images.len(), 1);
        assert!(pipeline.images.contains_key("app"));
    }

    #[test]
    fn test_parse_full_pipeline() {
        let yaml = r#"
version: "1"
vars:
  VERSION: "1.0.0"
defaults:
  format: oci
images:
  base:
    file: docker/Dockerfile.base
    tags:
      - "myapp/base:${VERSION}"
  app:
    file: docker/Dockerfile.app
    context: "."
    depends_on: [base]
    tags:
      - "myapp/app:${VERSION}"
push:
  after_all: true
"#;
        let pipeline = parse_pipeline(yaml).unwrap();
        assert_eq!(pipeline.version, Some("1".to_string()));
        assert_eq!(pipeline.vars.get("VERSION"), Some(&"1.0.0".to_string()));
        assert_eq!(pipeline.defaults.format, Some("oci".to_string()));
        assert_eq!(pipeline.images.len(), 2);
        assert!(pipeline.push.after_all);

        let app = &pipeline.images["app"];
        assert_eq!(app.depends_on, vec!["base"]);
    }

    #[test]
    fn test_rejects_unknown_fields() {
        let yaml = r#"
images:
  app:
    file: Dockerfile
    unknown_field: "should fail"
"#;
        assert!(parse_pipeline(yaml).is_err());
    }

    #[test]
    fn test_image_order_preserved() {
        let yaml = r#"
images:
  third:
    file: Dockerfile.third
  first:
    file: Dockerfile.first
  second:
    file: Dockerfile.second
"#;
        let pipeline = parse_pipeline(yaml).unwrap();
        let keys: Vec<&String> = pipeline.images.keys().collect();
        assert_eq!(keys, vec!["third", "first", "second"]);
    }

    #[test]
    fn test_vars_and_defaults() {
        let yaml = r#"
vars:
  REGISTRY: "ghcr.io/myorg"
  VERSION: "v1.2.3"
defaults:
  format: docker
  build_args:
    RUST_VERSION: "1.90"
  no_cache: true
images:
  app:
    file: Dockerfile
"#;
        let pipeline = parse_pipeline(yaml).unwrap();
        assert_eq!(
            pipeline.vars.get("REGISTRY"),
            Some(&"ghcr.io/myorg".to_string())
        );
        assert_eq!(pipeline.vars.get("VERSION"), Some(&"v1.2.3".to_string()));
        assert_eq!(pipeline.defaults.format, Some("docker".to_string()));
        assert_eq!(
            pipeline.defaults.build_args.get("RUST_VERSION"),
            Some(&"1.90".to_string())
        );
        assert!(pipeline.defaults.no_cache);
    }

    #[test]
    fn test_image_with_all_fields() {
        let yaml = r#"
images:
  app:
    file: docker/Dockerfile.app
    context: "./app"
    tags:
      - "myapp:latest"
      - "myapp:v1.0.0"
    build_args:
      NODE_ENV: production
      DEBUG: "false"
    depends_on:
      - base
      - utils
    no_cache: true
    format: oci
"#;
        let pipeline = parse_pipeline(yaml).unwrap();
        let app = &pipeline.images["app"];

        assert_eq!(app.file.to_string_lossy(), "docker/Dockerfile.app");
        assert_eq!(app.context.to_string_lossy(), "./app");
        assert_eq!(app.tags.len(), 2);
        assert_eq!(app.build_args.get("NODE_ENV"), Some(&"production".to_string()));
        assert_eq!(app.depends_on, vec!["base", "utils"]);
        assert_eq!(app.no_cache, Some(true));
        assert_eq!(app.format, Some("oci".to_string()));
    }

    #[test]
    fn test_empty_vars_and_defaults() {
        let yaml = r#"
images:
  app:
    file: Dockerfile
"#;
        let pipeline = parse_pipeline(yaml).unwrap();
        assert!(pipeline.vars.is_empty());
        assert!(pipeline.defaults.format.is_none());
        assert!(pipeline.defaults.build_args.is_empty());
        assert!(!pipeline.defaults.no_cache);
        assert!(!pipeline.push.after_all);
    }

    #[test]
    fn test_roundtrip_serialization() {
        let yaml = r#"
version: "1"
vars:
  VERSION: "1.0.0"
images:
  app:
    file: Dockerfile
    tags:
      - "myapp:latest"
"#;
        let pipeline = parse_pipeline(yaml).unwrap();
        let serialized = serde_yaml::to_string(&pipeline).unwrap();
        let pipeline2 = parse_pipeline(&serialized).unwrap();

        assert_eq!(pipeline.version, pipeline2.version);
        assert_eq!(pipeline.vars, pipeline2.vars);
        assert_eq!(pipeline.images.len(), pipeline2.images.len());
    }

    #[test]
    fn test_parse_error_message() {
        let yaml = r#"
images:
  - this is invalid yaml structure
"#;
        let result = parse_pipeline(yaml);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("Pipeline parse error"));
    }
}
