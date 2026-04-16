//! GitOps-style sync: read resource YAMLs from a git checkout and
//! reconcile against the API.
//!
//! A sync resource points at a directory containing `ZLayer` resource YAMLs
//! (deployment specs, job definitions, etc.).  [`scan_resources`] walks the
//! directory for `*.yaml` / `*.yml` files and extracts a [`SyncResource`] from
//! each.  [`compute_diff`] then compares the local set against a list of
//! remote resource names to produce a [`SyncDiff`] describing what needs to be
//! created, updated, or deleted.

use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// A resource parsed from a YAML file in the sync directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncResource {
    /// Relative path of the source file within the sync directory.
    pub file_path: String,
    /// Resource kind: `"deployment"`, `"job"`, `"cron"`, etc.
    pub kind: String,
    /// Resource name extracted from the YAML.
    pub name: String,
    /// Raw YAML content of the file.
    pub content: String,
}

/// Diff between local (git) resources and remote (API) resources.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncDiff {
    /// Resources present locally but not remotely — need to be created.
    pub to_create: Vec<SyncResource>,
    /// Resources present in both — flagged for update (v1: always listed).
    pub to_update: Vec<SyncResource>,
    /// Resource names present remotely but not locally — candidates for deletion.
    pub to_delete: Vec<String>,
}

/// Scan a directory for YAML files and parse each into a [`SyncResource`].
///
/// Walks `dir` one level deep (non-recursive) for files ending in `.yaml` or
/// `.yml`.  Each file is read and inspected for a resource kind and name.
///
/// # Errors
///
/// Returns an error if `dir` cannot be read or a YAML file cannot be parsed.
pub fn scan_resources(dir: &Path) -> Result<Vec<SyncResource>> {
    let entries = std::fs::read_dir(dir)
        .with_context(|| format!("reading sync directory {}", dir.display()))?;

    let mut resources = Vec::new();

    for entry in entries {
        let entry = entry.context("reading directory entry")?;
        let path = entry.path();

        // Only process regular files with YAML extensions.
        if !path.is_file() {
            continue;
        }
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default();
        if ext != "yaml" && ext != "yml" {
            continue;
        }

        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;

        let (kind, name) = extract_kind_and_name(&content)
            .with_context(|| format!("parsing resource metadata from {}", path.display()))?;

        let file_path = path
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default();

        resources.push(SyncResource {
            file_path,
            kind,
            name,
            content,
        });
    }

    // Sort by file path for deterministic ordering.
    resources.sort_by(|a, b| a.file_path.cmp(&b.file_path));

    Ok(resources)
}

/// Compare local resources against a list of remote resource names.
///
/// - Resources in `local` whose name is **not** in `remote_names` go into
///   `to_create`.
/// - Resources in `local` whose name **is** in `remote_names` go into
///   `to_update` (v1: always listed; the apply handler decides whether an
///   actual update is needed).
/// - Names in `remote_names` that have no matching local resource go into
///   `to_delete`.
pub fn compute_diff(local: &[SyncResource], remote_names: &[String]) -> SyncDiff {
    use std::collections::HashSet;

    let remote_set: HashSet<&str> = remote_names.iter().map(String::as_str).collect();
    let local_names: HashSet<&str> = local.iter().map(|r| r.name.as_str()).collect();

    let mut to_create = Vec::new();
    let mut to_update = Vec::new();

    for resource in local {
        if remote_set.contains(resource.name.as_str()) {
            to_update.push(resource.clone());
        } else {
            to_create.push(resource.clone());
        }
    }

    let to_delete: Vec<String> = remote_names
        .iter()
        .filter(|name| !local_names.contains(name.as_str()))
        .cloned()
        .collect();

    SyncDiff {
        to_create,
        to_update,
        to_delete,
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// A minimal YAML document used to extract `kind` / `name` / `deployment`.
#[derive(Deserialize)]
struct PartialResource {
    /// Explicit `kind` field (e.g. `kind: deployment`).
    kind: Option<String>,
    /// Explicit `name` field.
    name: Option<String>,
    /// Top-level `deployment` key used by `ZLayer` deployment specs.
    deployment: Option<String>,
    /// Top-level `job` key used by job specs.
    job: Option<String>,
    /// Top-level `cron` key used by cron specs.
    cron: Option<String>,
}

/// Extract the resource kind and name from raw YAML content.
///
/// Detection order:
/// 1. If an explicit `kind` field exists, use it (lowered). The name comes
///    from a `name` field, the `deployment` field, or the kind-specific
///    top-level key.
/// 2. Otherwise, infer the kind from whichever top-level key is present:
///    `deployment`, `job`, or `cron`.
fn extract_kind_and_name(yaml: &str) -> Result<(String, String)> {
    let partial: PartialResource =
        serde_yaml::from_str(yaml).context("YAML does not contain expected resource fields")?;

    let kind = if let Some(ref k) = partial.kind {
        k.to_lowercase()
    } else if partial.deployment.is_some() {
        "deployment".to_string()
    } else if partial.job.is_some() {
        "job".to_string()
    } else if partial.cron.is_some() {
        "cron".to_string()
    } else {
        anyhow::bail!(
            "cannot determine resource kind: no 'kind', 'deployment', 'job', or 'cron' key found"
        );
    };

    let name = partial
        .name
        .or(partial.deployment)
        .or(partial.job)
        .or(partial.cron)
        .context(
            "cannot determine resource name: no 'name', 'deployment', 'job', or 'cron' key found",
        )?;

    Ok((kind, name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn scan_empty_directory() {
        let dir = TempDir::new().unwrap();
        let resources = scan_resources(dir.path()).unwrap();
        assert!(resources.is_empty());
    }

    #[test]
    fn scan_ignores_non_yaml_files() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("readme.md"), "# hello").unwrap();
        std::fs::write(dir.path().join("config.json"), "{}").unwrap();
        let resources = scan_resources(dir.path()).unwrap();
        assert!(resources.is_empty());
    }

    #[test]
    fn scan_finds_yaml_and_yml() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("app.yaml"),
            "deployment: my-app\nversion: v1\nservices: {}\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("job.yml"),
            "job: nightly-backup\nversion: v1\n",
        )
        .unwrap();

        let resources = scan_resources(dir.path()).unwrap();
        assert_eq!(resources.len(), 2);

        let names: Vec<&str> = resources.iter().map(|r| r.name.as_str()).collect();
        assert!(names.contains(&"my-app"));
        assert!(names.contains(&"nightly-backup"));
    }

    #[test]
    fn scan_extracts_explicit_kind() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("svc.yaml"),
            "kind: Deployment\nname: web-frontend\n",
        )
        .unwrap();

        let resources = scan_resources(dir.path()).unwrap();
        assert_eq!(resources.len(), 1);
        assert_eq!(resources[0].kind, "deployment");
        assert_eq!(resources[0].name, "web-frontend");
    }

    #[test]
    fn scan_errors_on_unknown_kind() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("mystery.yaml"), "foo: bar\n").unwrap();

        let result = scan_resources(dir.path());
        assert!(result.is_err());
    }

    #[test]
    fn compute_diff_all_create() {
        let local = vec![
            SyncResource {
                file_path: "a.yaml".into(),
                kind: "deployment".into(),
                name: "app-a".into(),
                content: String::new(),
            },
            SyncResource {
                file_path: "b.yaml".into(),
                kind: "deployment".into(),
                name: "app-b".into(),
                content: String::new(),
            },
        ];
        let remote: Vec<String> = vec![];

        let diff = compute_diff(&local, &remote);
        assert_eq!(diff.to_create.len(), 2);
        assert!(diff.to_update.is_empty());
        assert!(diff.to_delete.is_empty());
    }

    #[test]
    fn compute_diff_all_update() {
        let local = vec![SyncResource {
            file_path: "a.yaml".into(),
            kind: "deployment".into(),
            name: "app-a".into(),
            content: String::new(),
        }];
        let remote = vec!["app-a".to_string()];

        let diff = compute_diff(&local, &remote);
        assert!(diff.to_create.is_empty());
        assert_eq!(diff.to_update.len(), 1);
        assert!(diff.to_delete.is_empty());
    }

    #[test]
    fn compute_diff_all_delete() {
        let local: Vec<SyncResource> = vec![];
        let remote = vec!["old-app".to_string()];

        let diff = compute_diff(&local, &remote);
        assert!(diff.to_create.is_empty());
        assert!(diff.to_update.is_empty());
        assert_eq!(diff.to_delete, vec!["old-app"]);
    }

    #[test]
    fn compute_diff_mixed() {
        let local = vec![
            SyncResource {
                file_path: "a.yaml".into(),
                kind: "deployment".into(),
                name: "new-app".into(),
                content: String::new(),
            },
            SyncResource {
                file_path: "b.yaml".into(),
                kind: "deployment".into(),
                name: "existing".into(),
                content: String::new(),
            },
        ];
        let remote = vec!["existing".to_string(), "stale".to_string()];

        let diff = compute_diff(&local, &remote);
        assert_eq!(diff.to_create.len(), 1);
        assert_eq!(diff.to_create[0].name, "new-app");
        assert_eq!(diff.to_update.len(), 1);
        assert_eq!(diff.to_update[0].name, "existing");
        assert_eq!(diff.to_delete, vec!["stale"]);
    }

    #[test]
    fn extract_kind_and_name_deployment() {
        let yaml = "deployment: my-app\nversion: v1\n";
        let (kind, name) = extract_kind_and_name(yaml).unwrap();
        assert_eq!(kind, "deployment");
        assert_eq!(name, "my-app");
    }

    #[test]
    fn extract_kind_and_name_job() {
        let yaml = "job: nightly\n";
        let (kind, name) = extract_kind_and_name(yaml).unwrap();
        assert_eq!(kind, "job");
        assert_eq!(name, "nightly");
    }

    #[test]
    fn extract_kind_and_name_cron() {
        let yaml = "cron: hourly-check\n";
        let (kind, name) = extract_kind_and_name(yaml).unwrap();
        assert_eq!(kind, "cron");
        assert_eq!(name, "hourly-check");
    }

    #[test]
    fn extract_kind_and_name_explicit_kind() {
        let yaml = "kind: Deployment\nname: web\n";
        let (kind, name) = extract_kind_and_name(yaml).unwrap();
        assert_eq!(kind, "deployment");
        assert_eq!(name, "web");
    }

    #[test]
    fn extract_kind_and_name_missing() {
        let yaml = "foo: bar\n";
        assert!(extract_kind_and_name(yaml).is_err());
    }
}
