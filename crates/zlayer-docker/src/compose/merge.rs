//! Multi-file Compose YAML merge per the Compose specification.
//!
//! `docker compose -f a.yaml -f b.yaml ...` merges every file together, with
//! later files **overriding** earlier ones.  The merge rules implemented here
//! mirror the public Compose spec
//! (<https://docs.docker.com/compose/compose-file/13-merge/>):
//!
//! * **Scalars** (strings, numbers, booleans, null) — later wins.
//! * **Mappings** — recursively merged. Keys present in both sides are merged
//!   per the rule for the value's type. Keys present only in one side carry
//!   over unchanged.
//! * **Sequences (lists)** — **replaced** wholesale by the later side. The
//!   Compose spec is explicit that lists are not appended; users who want to
//!   accumulate values must use long-form mapped syntax (e.g. named volumes
//!   or networks) which serde sees as mappings, not sequences.
//!
//! Note that this module operates on `serde_yaml::Value` shapes — it does not
//! attempt to know which keys are "named maps" (e.g. `services`, `volumes`,
//! `networks`) versus plain object fields. The Compose grammar happens to
//! make every named collection a mapping, so the generic mapping-merge rule
//! does the right thing for them. The only field that meaningfully wants
//! list-replace semantics in vanilla Compose merging is sequences like
//! `command`, `entrypoint`, and `ports`, which is exactly what we provide.
//!
//! # Errors
//!
//! Returns [`MergeError`] only for I/O or top-level YAML failures. The
//! spec-level merge itself is total — any combination of well-formed YAML
//! values produces a well-formed merged value.

use std::path::{Path, PathBuf};

use thiserror::Error;

use super::types::ComposeFile;

/// Errors raised while merging multiple compose files.
#[derive(Debug, Error)]
pub enum MergeError {
    /// `compose -f` was invoked with zero files.
    #[error("at least one compose file must be supplied")]
    NoFiles,

    /// A file failed to read.
    #[error("failed to read compose file `{path}`: {source}")]
    Io {
        /// Path that could not be read.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// A file failed to parse as YAML.
    #[error("failed to parse compose file `{path}`: {source}")]
    Yaml {
        /// Path that failed to parse.
        path: PathBuf,
        /// Underlying YAML error.
        #[source]
        source: serde_yaml::Error,
    },

    /// The merged result failed to project back into a [`ComposeFile`].
    #[error("merged compose document is not valid: {source}")]
    Schema {
        /// Underlying YAML error from the final type-driven deserialization.
        #[source]
        source: serde_yaml::Error,
    },
}

/// Merge a list of compose files into a single [`ComposeFile`].
///
/// Files are processed in order; each one is deep-merged into the running
/// accumulator with later files overriding earlier ones per the rules
/// documented at the module level.
///
/// # Errors
///
/// Returns [`MergeError`] when:
/// * `files` is empty.
/// * Any file cannot be read or parsed as YAML.
/// * The merged document does not match the [`ComposeFile`] schema.
///
/// # Panics
///
/// Panics only if the internal accumulator is unexpectedly `None` after a
/// non-empty input slice has been processed (which the early-return guard
/// already rules out, so this is guaranteed unreachable in practice).
pub fn merge_compose_files(files: &[PathBuf]) -> Result<ComposeFile, MergeError> {
    if files.is_empty() {
        return Err(MergeError::NoFiles);
    }

    let mut acc: Option<serde_yaml::Value> = None;

    for path in files {
        let contents = std::fs::read_to_string(path).map_err(|e| MergeError::Io {
            path: path.clone(),
            source: e,
        })?;
        let value: serde_yaml::Value =
            serde_yaml::from_str(&contents).map_err(|e| MergeError::Yaml {
                path: path.clone(),
                source: e,
            })?;
        acc = Some(match acc {
            None => value,
            Some(existing) => merge_values(existing, value),
        });
    }

    let merged = acc.expect("acc populated by non-empty loop");

    serde_yaml::from_value::<ComposeFile>(merged).map_err(|e| MergeError::Schema { source: e })
}

/// Merge two YAML values per the Compose spec. `override_with` wins.
///
/// * Two mappings: recursively merged key-by-key.
/// * Two sequences: `override_with` replaces `base` wholesale.
/// * Anything else (scalar, null, mismatched types): `override_with` wins.
#[must_use]
pub fn merge_values(
    base: serde_yaml::Value,
    override_with: serde_yaml::Value,
) -> serde_yaml::Value {
    match (base, override_with) {
        (serde_yaml::Value::Mapping(mut base_map), serde_yaml::Value::Mapping(override_map)) => {
            for (k, v) in override_map {
                let merged = match base_map.remove(&k) {
                    Some(existing) => merge_values(existing, v),
                    None => v,
                };
                base_map.insert(k, merged);
            }
            serde_yaml::Value::Mapping(base_map)
        }
        // Lists are replaced, not appended.
        (_, override_with) => override_with,
    }
}

/// Resolve a list of compose-file paths against a project directory.
///
/// Relative paths are joined onto `project_dir`. Absolute paths are kept as
/// supplied so that explicit `-f /etc/compose.yaml` semantics still work.
#[must_use]
pub fn resolve_paths(project_dir: &Path, files: &[PathBuf]) -> Vec<PathBuf> {
    files
        .iter()
        .map(|p| {
            if p.is_absolute() {
                p.clone()
            } else {
                project_dir.join(p)
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zlayer_paths::ZLayerDirs;

    fn write_file(dir: &Path, name: &str, contents: &str) -> PathBuf {
        let path = dir.join(name);
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(contents.as_bytes()).unwrap();
        path
    }

    #[test]
    fn merge_no_files_errors() {
        let err = merge_compose_files(&[]).unwrap_err();
        assert!(matches!(err, MergeError::NoFiles));
    }

    #[test]
    fn merge_single_file_round_trips() {
        let dir = ZLayerDirs::system_default()
            .scratch_dir("merge-single-file-round-trips-")
            .unwrap();
        let f = write_file(
            dir.path(),
            "compose.yaml",
            r"
services:
  web:
    image: nginx:1.25
    ports:
      - '8080:80'
",
        );
        let merged = merge_compose_files(&[f]).unwrap();
        let web = merged.services.get("web").expect("web present");
        assert_eq!(web.image.as_deref(), Some("nginx:1.25"));
    }

    #[test]
    fn merge_two_files_later_overrides_scalar() {
        let dir = ZLayerDirs::system_default()
            .scratch_dir("merge-two-files-later-overrides-scalar-")
            .unwrap();
        let a = write_file(
            dir.path(),
            "a.yaml",
            r"
services:
  web:
    image: nginx:1.24
",
        );
        let b = write_file(
            dir.path(),
            "b.yaml",
            r"
services:
  web:
    image: nginx:1.25
",
        );
        let merged = merge_compose_files(&[a, b]).unwrap();
        let web = merged.services.get("web").expect("web present");
        assert_eq!(web.image.as_deref(), Some("nginx:1.25"));
    }

    #[test]
    fn merge_two_files_named_maps_are_merged() {
        let dir = ZLayerDirs::system_default()
            .scratch_dir("merge-two-files-named-maps-are-merged-")
            .unwrap();
        let a = write_file(
            dir.path(),
            "a.yaml",
            r"
services:
  web:
    image: nginx
  db:
    image: postgres:15
",
        );
        let b = write_file(
            dir.path(),
            "b.yaml",
            r"
services:
  cache:
    image: redis:7
  db:
    image: postgres:16
",
        );
        let merged = merge_compose_files(&[a, b]).unwrap();
        // web present from a, untouched.
        assert_eq!(
            merged.services.get("web").and_then(|s| s.image.as_deref()),
            Some("nginx")
        );
        // db present in both; b wins.
        assert_eq!(
            merged.services.get("db").and_then(|s| s.image.as_deref()),
            Some("postgres:16")
        );
        // cache present only in b.
        assert_eq!(
            merged
                .services
                .get("cache")
                .and_then(|s| s.image.as_deref()),
            Some("redis:7")
        );
    }

    #[test]
    fn merge_lists_are_replaced_not_appended() {
        // The Compose spec states that lists are replaced wholesale by the
        // override.  This is the property regression-tested here.
        let dir = ZLayerDirs::system_default()
            .scratch_dir("merge-lists-are-replaced-not-appended-")
            .unwrap();
        let a = write_file(
            dir.path(),
            "a.yaml",
            r"
services:
  web:
    image: nginx
    command: ['nginx', '-g', 'daemon off;']
",
        );
        let b = write_file(
            dir.path(),
            "b.yaml",
            r"
services:
  web:
    command: ['sleep', 'infinity']
",
        );
        let merged = merge_compose_files(&[a, b]).unwrap();
        let web = merged.services.get("web").expect("web present");
        assert_eq!(
            web.command,
            vec!["sleep".to_string(), "infinity".to_string()]
        );
    }

    #[test]
    fn merge_environment_maps_are_merged() {
        // When `environment` arrives as a YAML mapping it merges per-key.
        let dir = ZLayerDirs::system_default()
            .scratch_dir("merge-environment-maps-are-merged-")
            .unwrap();
        let a = write_file(
            dir.path(),
            "a.yaml",
            r"
services:
  web:
    image: nginx
    environment:
      A: '1'
      B: '2'
",
        );
        let b = write_file(
            dir.path(),
            "b.yaml",
            r"
services:
  web:
    environment:
      B: '20'
      C: '3'
",
        );
        let merged = merge_compose_files(&[a, b]).unwrap();
        let web = merged.services.get("web").expect("web present");
        assert_eq!(web.environment.get("A").map(String::as_str), Some("1"));
        // B overridden by b.
        assert_eq!(web.environment.get("B").map(String::as_str), Some("20"));
        // C added by b.
        assert_eq!(web.environment.get("C").map(String::as_str), Some("3"));
    }

    #[test]
    fn merge_three_files_chain_left_to_right() {
        let dir = ZLayerDirs::system_default()
            .scratch_dir("merge-three-files-chain-left-to-right-")
            .unwrap();
        let a = write_file(
            dir.path(),
            "a.yaml",
            r"
services:
  web:
    image: nginx:1.20
    environment:
      LEVEL: a
",
        );
        let b = write_file(
            dir.path(),
            "b.yaml",
            r"
services:
  web:
    image: nginx:1.24
    environment:
      LEVEL: b
",
        );
        let c = write_file(
            dir.path(),
            "c.yaml",
            r"
services:
  web:
    environment:
      LEVEL: c
",
        );
        let merged = merge_compose_files(&[a, b, c]).unwrap();
        let web = merged.services.get("web").expect("web present");
        // image: a -> b override -> not touched by c -> b's value.
        assert_eq!(web.image.as_deref(), Some("nginx:1.24"));
        // environment.LEVEL: a -> b -> c -> c's value.
        assert_eq!(web.environment.get("LEVEL").map(String::as_str), Some("c"));
    }

    #[test]
    fn merge_rejects_missing_file() {
        let err = merge_compose_files(&[PathBuf::from("/nonexistent/compose.yaml")]).unwrap_err();
        match err {
            MergeError::Io { path, .. } => {
                assert_eq!(path, PathBuf::from("/nonexistent/compose.yaml"));
            }
            other => panic!("expected Io error, got {other:?}"),
        }
    }

    #[test]
    fn merge_rejects_invalid_yaml() {
        let dir = ZLayerDirs::system_default()
            .scratch_dir("merge-rejects-invalid-yaml-")
            .unwrap();
        let f = write_file(dir.path(), "bad.yaml", ":\n  - oops\n   bad indent\n");
        let err = merge_compose_files(std::slice::from_ref(&f)).unwrap_err();
        match err {
            MergeError::Yaml { path, .. } => assert_eq!(path, f),
            other => panic!("expected Yaml error, got {other:?}"),
        }
    }

    #[test]
    fn merge_values_scalar_replace() {
        let base = serde_yaml::Value::String("a".into());
        let over = serde_yaml::Value::String("b".into());
        assert_eq!(
            merge_values(base, over),
            serde_yaml::Value::String("b".into())
        );
    }

    #[test]
    fn merge_values_type_mismatch_replace() {
        let base = serde_yaml::Value::String("a".into());
        let over = serde_yaml::Value::Sequence(vec![serde_yaml::Value::String("x".into())]);
        let merged = merge_values(base, over.clone());
        assert_eq!(merged, over);
    }

    #[test]
    fn merge_values_sequence_replace() {
        let base = serde_yaml::Value::Sequence(vec![
            serde_yaml::Value::String("a".into()),
            serde_yaml::Value::String("b".into()),
        ]);
        let over = serde_yaml::Value::Sequence(vec![serde_yaml::Value::String("c".into())]);
        let merged = merge_values(base, over.clone());
        assert_eq!(merged, over);
    }

    #[test]
    fn merge_values_nested_mapping() {
        let base: serde_yaml::Value = serde_yaml::from_str(
            r"
a:
  b: 1
  c: 2
",
        )
        .unwrap();
        let over: serde_yaml::Value = serde_yaml::from_str(
            r"
a:
  c: 20
  d: 4
",
        )
        .unwrap();
        let merged = merge_values(base, over);
        let expected: serde_yaml::Value = serde_yaml::from_str(
            r"
a:
  b: 1
  c: 20
  d: 4
",
        )
        .unwrap();
        assert_eq!(merged, expected);
    }

    #[test]
    fn resolve_paths_handles_relative_and_absolute() {
        let dir = PathBuf::from("/home/zach/projects/demo");
        let resolved = resolve_paths(
            &dir,
            &[
                PathBuf::from("compose.yaml"),
                PathBuf::from("/etc/compose.override.yaml"),
                PathBuf::from("./extras/dev.yaml"),
            ],
        );
        assert_eq!(
            resolved,
            vec![
                PathBuf::from("/home/zach/projects/demo/compose.yaml"),
                PathBuf::from("/etc/compose.override.yaml"),
                PathBuf::from("/home/zach/projects/demo/./extras/dev.yaml"),
            ]
        );
    }
}
