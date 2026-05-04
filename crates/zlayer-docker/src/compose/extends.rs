//! Resolution of the Compose `extends:` directive.
//!
//! `extends:` lets a service inherit fields from another service, either in
//! the same file or in an external file. The Compose spec rules:
//!
//! 1. The parent service is loaded and merged into the child using the
//!    same merge semantics as `docker compose -f a.yaml -f b.yaml`
//!    (mappings recurse, sequences replace, scalars override). The child
//!    *wins* on every conflict.
//! 2. The parent's own `extends:` directive is resolved first
//!    (recursively), so chains compose left-to-right from the deepest
//!    ancestor down to the child.
//! 3. `depends_on`, `volumes_from`, and `links` are explicitly *not*
//!    inherited per the upstream spec — those refer to services in the
//!    parent's own compose file and would not make sense on the child.
//!
//! Cross-file extends supports a single level of indirection: the external
//! file is loaded as a standalone [`ComposeFile`] and the named service is
//! pulled from it. Recursive cross-file extends inside the parent file are
//! also resolved before the parent merges into the child.
//!
//! # Errors
//!
//! Returns [`ExtendsError`] when:
//! * The named parent service does not exist in the local file (or in the
//!   referenced external file).
//! * A cross-file `extends:` references a path that cannot be read or
//!   parsed.
//! * A cycle is detected in the extends chain.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use thiserror::Error;

use super::merge::merge_values;
use super::types::{ComposeExtends, ComposeFile};

/// Errors raised while resolving `extends:`.
#[derive(Debug, Error)]
pub enum ExtendsError {
    /// The parent service named by `extends:` was not found.
    #[error("service `{child}` extends unknown service `{parent}`")]
    UnknownParent {
        /// Service issuing the extends directive.
        child: String,
        /// Parent service name that could not be resolved.
        parent: String,
    },

    /// Cross-file `extends:` references a path that cannot be read.
    #[error("service `{child}` extends file `{path}` which cannot be read: {source}")]
    ReadFile {
        /// Service issuing the extends directive.
        child: String,
        /// Path attempted.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// Cross-file `extends:` references a malformed YAML file.
    #[error("service `{child}` extends file `{path}` which is malformed: {source}")]
    ParseFile {
        /// Service issuing the extends directive.
        child: String,
        /// Path attempted.
        path: PathBuf,
        /// Underlying YAML error.
        #[source]
        source: serde_yaml::Error,
    },

    /// Cross-file `extends:` requires a base directory but none is
    /// available (e.g. the user parsed a YAML string with no path).
    #[error(
        "service `{child}` extends external file `{file}` but no base directory is available; \
         use `parse_compose_file` instead of `parse_compose`"
    )]
    NoBaseDir {
        /// Service issuing the extends directive.
        child: String,
        /// External file path that could not be resolved.
        file: String,
    },

    /// A cycle was detected in the extends chain.
    #[error("cycle detected in `extends:` chain involving service `{service}`")]
    Cycle {
        /// Service that triggered the cycle detection.
        service: String,
    },
}

/// Resolve every `extends:` directive in `compose` in place.
///
/// `base_dir` is the directory used to resolve relative paths in cross-file
/// `extends: { file: ... }` entries. When `None`, cross-file extends raises
/// [`ExtendsError::NoBaseDir`].
///
/// # Errors
///
/// See [`ExtendsError`].
///
/// # Panics
///
/// Panics only if `serde_yaml::to_value` fails on a value built entirely
/// from `Serialize`-deriving types — an internal-invariant failure, never
/// a user-supplied input issue.
pub fn resolve_extends(
    compose: &mut ComposeFile,
    base_dir: Option<&Path>,
) -> Result<(), ExtendsError> {
    // Fast path: if no service in the file declares `extends:`, skip the
    // YAML round-trip entirely. Re-projecting `ComposeService` through
    // `serde_yaml::to_value` and back loses the bespoke Compose shorthand
    // shapes (e.g. port shorthand `"8080:80"` deserializes back as a
    // shorthand string but serializes as a structured map), so we only
    // pay that cost when there is real extends-resolution work to do.
    if compose.services.values().all(|s| s.extends.is_none()) {
        return Ok(());
    }

    // Project the whole file to a generic YAML value so we can do mapping-
    // level merges. Then patch the resolved services back in.
    let services_yaml = serde_yaml::to_value(&compose.services)
        .expect("ComposeService is always serializable to YAML");

    let serde_yaml::Value::Mapping(mut resolved) = services_yaml else {
        // Empty `services:` round-trips to a null/mapping; treat absence
        // as nothing-to-do.
        return Ok(());
    };

    // Collect names so we can iterate without holding a borrow on `resolved`.
    let names: Vec<String> = resolved
        .iter()
        .filter_map(|(k, _)| k.as_str().map(str::to_string))
        .collect();

    for name in names {
        let mut chain: HashSet<String> = HashSet::new();
        let merged = resolve_one(&name, &resolved, base_dir, &mut chain)?;
        resolved.insert(serde_yaml::Value::String(name), merged);
    }

    let projected: std::collections::HashMap<String, super::types::ComposeService> =
        serde_yaml::from_value(serde_yaml::Value::Mapping(resolved)).map_err(|e| {
            ExtendsError::ParseFile {
                child: "<resolved>".to_string(),
                path: PathBuf::new(),
                source: e,
            }
        })?;
    compose.services = projected;
    Ok(())
}

/// Resolve a single service, recursively merging in its parent if it
/// declares `extends:`.
fn resolve_one(
    name: &str,
    siblings: &serde_yaml::Mapping,
    base_dir: Option<&Path>,
    chain: &mut HashSet<String>,
) -> Result<serde_yaml::Value, ExtendsError> {
    if !chain.insert(name.to_string()) {
        return Err(ExtendsError::Cycle {
            service: name.to_string(),
        });
    }

    let key = serde_yaml::Value::String(name.to_string());
    let raw = siblings
        .get(&key)
        .cloned()
        .unwrap_or(serde_yaml::Value::Null);

    // Read out the `extends:` field if present, then strip it from the value
    // so it doesn't survive into the merged result.
    let (extends_value, raw) = strip_extends(raw);

    // Treat absent or null extends as a no-op. `serde_yaml` round-trips
    // `Option::None` through a flattened service map as a `null` mapping
    // entry rather than dropping the key, so we have to filter it here.
    let extends_value = match extends_value {
        Some(serde_yaml::Value::Null) | None => return Ok(raw),
        Some(v) => v,
    };

    let parent_value = match parse_extends_directive(&extends_value, name)? {
        ParentRef::Local(parent_name) => {
            let parent_key = serde_yaml::Value::String(parent_name.clone());
            if !siblings.contains_key(&parent_key) {
                return Err(ExtendsError::UnknownParent {
                    child: name.to_string(),
                    parent: parent_name,
                });
            }
            // Recursively resolve the parent first so chains compose.
            resolve_one(&parent_name, siblings, base_dir, chain)?
        }
        ParentRef::External {
            parent_name,
            file_path,
        } => load_external_parent(&parent_name, &file_path, name, base_dir)?,
    };

    // Strip fields the spec says are NOT inherited.
    let parent_value = strip_non_inheritable(parent_value);

    // The child mapping was produced by serialising a typed `ComposeService`,
    // so absent fields appear as explicit `null`s in the YAML value. Without
    // pruning those nulls, `merge_values` would overwrite the parent's real
    // values with the child's defaulted nulls (e.g. `image: null` from the
    // child winning over `image: alpine` from the parent). Strip them so the
    // merge only carries child-specified fields.
    let raw = strip_nulls(raw);

    Ok(merge_values(parent_value, raw))
}

/// Recursively drop `null`-valued entries from a YAML mapping. Used in the
/// extends merge path to keep child defaults from clobbering inherited
/// parent values.
fn strip_nulls(value: serde_yaml::Value) -> serde_yaml::Value {
    match value {
        serde_yaml::Value::Mapping(m) => {
            let mut out = serde_yaml::Mapping::new();
            for (k, v) in m {
                if matches!(v, serde_yaml::Value::Null) {
                    continue;
                }
                out.insert(k, strip_nulls(v));
            }
            serde_yaml::Value::Mapping(out)
        }
        other => other,
    }
}

/// Pull the `extends:` field off a service mapping if present.
fn strip_extends(value: serde_yaml::Value) -> (Option<serde_yaml::Value>, serde_yaml::Value) {
    match value {
        serde_yaml::Value::Mapping(mut m) => {
            let extends = m.remove(serde_yaml::Value::String("extends".to_string()));
            (extends, serde_yaml::Value::Mapping(m))
        }
        other => (None, other),
    }
}

/// Per the Compose spec, these fields are *not* carried over from a parent
/// service — they always describe relationships within the parent's own
/// file and would refer to nothing meaningful in the child's file.
fn strip_non_inheritable(value: serde_yaml::Value) -> serde_yaml::Value {
    let serde_yaml::Value::Mapping(mut m) = value else {
        return value;
    };
    for field in ["depends_on", "volumes_from", "links"] {
        m.remove(serde_yaml::Value::String(field.to_string()));
    }
    serde_yaml::Value::Mapping(m)
}

/// Concrete reference produced by parsing an `extends:` directive.
enum ParentRef {
    Local(String),
    External {
        parent_name: String,
        file_path: String,
    },
}

/// Parse the YAML form of `extends:` into a [`ParentRef`].
fn parse_extends_directive(
    value: &serde_yaml::Value,
    child: &str,
) -> Result<ParentRef, ExtendsError> {
    let parsed: ComposeExtends =
        serde_yaml::from_value(value.clone()).map_err(|e| ExtendsError::ParseFile {
            child: child.to_string(),
            path: PathBuf::new(),
            source: e,
        })?;
    match parsed {
        ComposeExtends::Service(s) => Ok(ParentRef::Local(s)),
        ComposeExtends::Full(cfg) => match cfg.file {
            Some(file) => Ok(ParentRef::External {
                parent_name: cfg.service,
                file_path: file,
            }),
            None => Ok(ParentRef::Local(cfg.service)),
        },
    }
}

/// Load + resolve an external `extends.file` reference.
fn load_external_parent(
    parent_name: &str,
    file_path: &str,
    child: &str,
    base_dir: Option<&Path>,
) -> Result<serde_yaml::Value, ExtendsError> {
    let base = base_dir.ok_or_else(|| ExtendsError::NoBaseDir {
        child: child.to_string(),
        file: file_path.to_string(),
    })?;
    let abs = if Path::new(file_path).is_absolute() {
        PathBuf::from(file_path)
    } else {
        base.join(file_path)
    };
    let contents = std::fs::read_to_string(&abs).map_err(|e| ExtendsError::ReadFile {
        child: child.to_string(),
        path: abs.clone(),
        source: e,
    })?;
    let mut external: ComposeFile =
        serde_yaml::from_str(&contents).map_err(|e| ExtendsError::ParseFile {
            child: child.to_string(),
            path: abs.clone(),
            source: e,
        })?;
    // Recursively resolve extends inside the external file. Cross-file
    // extends inside the external file uses the external file's directory
    // as its own base.
    resolve_extends(&mut external, abs.parent())?;

    let parent = external
        .services
        .get(parent_name)
        .ok_or_else(|| ExtendsError::UnknownParent {
            child: child.to_string(),
            parent: parent_name.to_string(),
        })?
        .clone();
    Ok(serde_yaml::to_value(&parent).expect("ComposeService is always serializable to YAML"))
}

#[cfg(test)]
mod tests {
    use crate::compose::parse_compose;

    #[test]
    fn extends_same_file_inherits_image() {
        let yaml = r"
services:
  base:
    image: alpine
    environment:
      FOO: from_base
  child:
    extends: base
";
        let compose = parse_compose(yaml).unwrap();
        let child = &compose.services["child"];
        assert_eq!(child.image.as_deref(), Some("alpine"));
        assert_eq!(
            child.environment.get("FOO").map(String::as_str),
            Some("from_base")
        );
    }

    #[test]
    fn extends_child_overrides_parent_scalar() {
        let yaml = r"
services:
  base:
    image: alpine:3.18
    environment:
      LEVEL: base
      ONLY_BASE: yes
  child:
    extends:
      service: base
    image: alpine:3.19
    environment:
      LEVEL: child
      ONLY_CHILD: yes
";
        let compose = parse_compose(yaml).unwrap();
        let child = &compose.services["child"];
        assert_eq!(child.image.as_deref(), Some("alpine:3.19"));
        assert_eq!(
            child.environment.get("LEVEL").map(String::as_str),
            Some("child")
        );
        assert_eq!(
            child.environment.get("ONLY_BASE").map(String::as_str),
            Some("yes")
        );
        assert_eq!(
            child.environment.get("ONLY_CHILD").map(String::as_str),
            Some("yes")
        );
    }

    #[test]
    fn extends_chain_resolves_recursively() {
        let yaml = r"
services:
  grandparent:
    image: ubuntu:24.04
    environment:
      G: 1
  parent:
    extends: grandparent
    environment:
      P: 1
  child:
    extends: parent
    environment:
      C: 1
";
        let compose = parse_compose(yaml).unwrap();
        let child = &compose.services["child"];
        assert_eq!(child.image.as_deref(), Some("ubuntu:24.04"));
        assert_eq!(child.environment.get("G").map(String::as_str), Some("1"));
        assert_eq!(child.environment.get("P").map(String::as_str), Some("1"));
        assert_eq!(child.environment.get("C").map(String::as_str), Some("1"));
    }

    #[test]
    fn extends_unknown_parent_errors() {
        let yaml = r"
services:
  child:
    extends: missing
";
        let err = parse_compose(yaml).unwrap_err().to_string();
        assert!(
            err.contains("missing"),
            "expected unknown-parent error, got: {err}"
        );
    }

    #[test]
    fn extends_cycle_errors() {
        let yaml = r"
services:
  a:
    extends: b
  b:
    extends: a
";
        let err = parse_compose(yaml).unwrap_err().to_string();
        assert!(
            err.to_lowercase().contains("cycle"),
            "expected cycle error, got: {err}"
        );
    }

    #[test]
    fn extends_does_not_inherit_depends_on() {
        let yaml = r"
services:
  base:
    image: alpine
    depends_on:
      - somethingelse
  child:
    extends: base
";
        let compose = parse_compose(yaml).unwrap();
        // depends_on is intentionally NOT inherited.
        assert!(compose.services["child"].depends_on.is_empty());
    }

    #[test]
    fn extends_external_file_requires_base_dir() {
        // Same-file extends works with parse_compose; cross-file does not.
        let yaml = r"
services:
  child:
    extends:
      service: parent
      file: ./other.yaml
";
        let err = parse_compose(yaml).unwrap_err().to_string();
        assert!(
            err.contains("base directory") || err.contains("parse_compose_file"),
            "expected base-dir error, got: {err}"
        );
    }

    #[test]
    fn extends_external_file_resolves_via_parse_compose_file() {
        let dir = tempfile::tempdir().unwrap();
        let parent_path = dir.path().join("parent.yaml");
        std::fs::write(
            &parent_path,
            r"
services:
  shared:
    image: redis:7
    environment:
      ROLE: shared
",
        )
        .unwrap();
        let main_path = dir.path().join("compose.yaml");
        std::fs::write(
            &main_path,
            r"
services:
  cache:
    extends:
      service: shared
      file: parent.yaml
    environment:
      EXTRA: yes
",
        )
        .unwrap();

        let compose = crate::compose::parse_compose_file(&main_path).unwrap();
        let cache = &compose.services["cache"];
        assert_eq!(cache.image.as_deref(), Some("redis:7"));
        assert_eq!(
            cache.environment.get("ROLE").map(String::as_str),
            Some("shared")
        );
        assert_eq!(
            cache.environment.get("EXTRA").map(String::as_str),
            Some("yes")
        );
    }
}
