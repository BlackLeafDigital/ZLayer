//! Resolution of the top-level Compose `include:` directive.
//!
//! `include:` lets a compose file pull in fragments from external files.
//! Each include entry is loaded, then merged into the running document
//! *underneath* the current file — i.e. the current file overrides whatever
//! the includes declared. This mirrors the upstream Docker Compose
//! semantics where `include:` provides a base layer and the including file
//! is treated like a `-f override.yaml`.
//!
//! Includes are processed depth-first: each included file's own `include:`
//! is resolved before that file is merged into its parent.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use thiserror::Error;

use super::merge::merge_values;
use super::types::ComposeFile;

/// Errors raised while resolving the `include:` directive.
#[derive(Debug, Error)]
pub enum IncludeError {
    /// The top-level compose file at `path` could not be read.
    #[error("failed to read compose file `{path}`: {source}")]
    ReadFile {
        /// File that could not be read.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// A YAML document failed to parse.
    #[error("failed to parse compose file `{path}`: {source}")]
    ParseFile {
        /// File that failed to parse.
        path: PathBuf,
        /// Underlying YAML error.
        #[source]
        source: serde_yaml::Error,
    },

    /// An include entry has an empty `path:` list.
    #[error("compose `include:` entry in `{file}` has no `path:`")]
    EmptyInclude {
        /// File that contained the empty include entry.
        file: PathBuf,
    },

    /// An include cycle was detected.
    #[error("compose `include:` cycle detected at `{path}`")]
    Cycle {
        /// Path that re-entered the include chain.
        path: PathBuf,
    },
}

/// Load the compose file at `path`, resolving any top-level `include:`
/// directives recursively.
///
/// Includes are merged underneath the current file: the current file always
/// wins on conflicts, mirroring `docker compose -f include.yaml -f main.yaml`.
///
/// # Errors
///
/// See [`IncludeError`].
pub fn resolve_includes(path: &Path) -> Result<ComposeFile, IncludeError> {
    let mut visited = HashSet::new();
    let merged = load_with_includes(path, &mut visited)?;
    serde_yaml::from_value::<ComposeFile>(merged).map_err(|e| IncludeError::ParseFile {
        path: path.to_path_buf(),
        source: e,
    })
}

/// Load `path`, recursively splicing in its includes, returning the merged
/// YAML value.
fn load_with_includes(
    path: &Path,
    visited: &mut HashSet<PathBuf>,
) -> Result<serde_yaml::Value, IncludeError> {
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if !visited.insert(canonical.clone()) {
        return Err(IncludeError::Cycle { path: canonical });
    }

    let contents = std::fs::read_to_string(path).map_err(|e| IncludeError::ReadFile {
        path: path.to_path_buf(),
        source: e,
    })?;
    let mut doc: serde_yaml::Value =
        serde_yaml::from_str(&contents).map_err(|e| IncludeError::ParseFile {
            path: path.to_path_buf(),
            source: e,
        })?;

    let includes = take_includes(&mut doc);

    if includes.is_empty() {
        visited.remove(&canonical);
        return Ok(doc);
    }

    let base_dir = path.parent().unwrap_or_else(|| Path::new("."));

    // Build the merged base by chaining all includes left-to-right (each
    // later include overrides earlier ones, just like multi-file merge).
    let mut base: Option<serde_yaml::Value> = None;
    for entry in includes {
        let mut paths_iter = entry.into_iter();
        let Some(first) = paths_iter.next() else {
            return Err(IncludeError::EmptyInclude {
                file: path.to_path_buf(),
            });
        };
        let mut chunk = load_one_include(&first, base_dir, visited)?;
        for extra in paths_iter {
            let layer = load_one_include(&extra, base_dir, visited)?;
            chunk = merge_values(chunk, layer);
        }
        base = Some(match base {
            None => chunk,
            Some(existing) => merge_values(existing, chunk),
        });
    }

    let merged = match base {
        None => doc,
        Some(base) => merge_values(base, doc),
    };

    visited.remove(&canonical);
    Ok(merged)
}

/// Pull the `include:` field off a top-level compose mapping. Returns a
/// list of include groups; each group is a list of one or more file paths.
fn take_includes(doc: &mut serde_yaml::Value) -> Vec<Vec<String>> {
    let serde_yaml::Value::Mapping(map) = doc else {
        return Vec::new();
    };
    let Some(raw) = map.remove(serde_yaml::Value::String("include".to_string())) else {
        return Vec::new();
    };
    let serde_yaml::Value::Sequence(items) = raw else {
        return Vec::new();
    };
    items.into_iter().filter_map(parse_include_entry).collect()
}

/// Convert a single `include[]` element into a list of file paths.
fn parse_include_entry(value: serde_yaml::Value) -> Option<Vec<String>> {
    match value {
        serde_yaml::Value::String(s) => Some(vec![s]),
        serde_yaml::Value::Mapping(m) => {
            let path = m.get(serde_yaml::Value::String("path".to_string()))?;
            match path {
                serde_yaml::Value::String(s) => Some(vec![s.clone()]),
                serde_yaml::Value::Sequence(seq) => {
                    let paths: Vec<String> = seq
                        .iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect();
                    if paths.is_empty() {
                        None
                    } else {
                        Some(paths)
                    }
                }
                _ => None,
            }
        }
        _ => None,
    }
}

/// Load a single included file, recursing through its own `include:`.
fn load_one_include(
    rel: &str,
    base_dir: &Path,
    visited: &mut HashSet<PathBuf>,
) -> Result<serde_yaml::Value, IncludeError> {
    let abs = if Path::new(rel).is_absolute() {
        PathBuf::from(rel)
    } else {
        base_dir.join(rel)
    };
    load_with_includes(&abs, visited)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(body.as_bytes()).unwrap();
        path
    }

    #[test]
    fn include_short_form_string_path() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "base.yaml",
            r"
services:
  base:
    image: alpine
",
        );
        let main = write(
            dir.path(),
            "compose.yaml",
            r"
include:
  - base.yaml

services:
  app:
    image: nginx
",
        );
        let compose = resolve_includes(&main).unwrap();
        assert!(compose.services.contains_key("base"));
        assert!(compose.services.contains_key("app"));
        assert_eq!(compose.services["base"].image.as_deref(), Some("alpine"));
        assert_eq!(compose.services["app"].image.as_deref(), Some("nginx"));
    }

    #[test]
    fn include_long_form_object() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "shared.yaml",
            r"
services:
  shared:
    image: redis:7
",
        );
        let main = write(
            dir.path(),
            "compose.yaml",
            r"
include:
  - path: shared.yaml

services:
  app:
    image: nginx
",
        );
        let compose = resolve_includes(&main).unwrap();
        assert_eq!(compose.services["shared"].image.as_deref(), Some("redis:7"));
        assert_eq!(compose.services["app"].image.as_deref(), Some("nginx"));
    }

    #[test]
    fn include_main_overrides_included_value() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "base.yaml",
            r"
services:
  api:
    image: nginx:1.20
    environment:
      LEVEL: base
",
        );
        let main = write(
            dir.path(),
            "compose.yaml",
            r"
include:
  - base.yaml

services:
  api:
    image: nginx:1.25
    environment:
      LEVEL: main
      EXTRA: yes
",
        );
        let compose = resolve_includes(&main).unwrap();
        let api = &compose.services["api"];
        // Main file wins on image.
        assert_eq!(api.image.as_deref(), Some("nginx:1.25"));
        // Main file wins on conflicting env key.
        assert_eq!(
            api.environment.get("LEVEL").map(String::as_str),
            Some("main")
        );
        // Main-only key is preserved.
        assert_eq!(
            api.environment.get("EXTRA").map(String::as_str),
            Some("yes")
        );
    }

    #[test]
    fn include_chains_recursively() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "leaf.yaml",
            r"
services:
  leaf:
    image: leaf:latest
",
        );
        write(
            dir.path(),
            "middle.yaml",
            r"
include:
  - leaf.yaml

services:
  middle:
    image: middle:latest
",
        );
        let main = write(
            dir.path(),
            "compose.yaml",
            r"
include:
  - middle.yaml

services:
  app:
    image: app:latest
",
        );
        let compose = resolve_includes(&main).unwrap();
        assert!(compose.services.contains_key("leaf"));
        assert!(compose.services.contains_key("middle"));
        assert!(compose.services.contains_key("app"));
    }

    #[test]
    fn include_missing_file_errors() {
        let dir = tempfile::tempdir().unwrap();
        let main = write(
            dir.path(),
            "compose.yaml",
            r"
include:
  - nope.yaml

services:
  app:
    image: nginx
",
        );
        let err = resolve_includes(&main).unwrap_err();
        assert!(matches!(err, IncludeError::ReadFile { .. }));
    }
}
