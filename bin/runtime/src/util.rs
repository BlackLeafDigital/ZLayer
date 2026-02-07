//! Shared utility functions for the ZLayer runtime CLI.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use tracing::info;
use zlayer_spec::DeploymentSpec;

/// Discover spec path from explicit argument or auto-discovery.
///
/// Priority order:
/// 1. Explicit path argument (returned as-is)
/// 2. Well-known exact filenames: `.zlayer.yml`, `.zlayer.yaml`, `zlayer.yml`, `zlayer.yaml`
/// 3. Glob match: any file matching `*.zlayer.yml` or `*.zlayer.yaml` in the current directory
///    - Exactly one match: use it
///    - Multiple matches: error listing them so the user can choose
///    - Zero matches: error with guidance
pub(crate) fn discover_spec_path(explicit: Option<&Path>) -> Result<PathBuf> {
    discover_spec_path_in_dir(explicit, Path::new("."))
}

/// Inner implementation that accepts a base directory, making it testable without chdir.
fn discover_spec_path_in_dir(explicit: Option<&Path>, dir: &Path) -> Result<PathBuf> {
    if let Some(path) = explicit {
        return Ok(path.to_path_buf());
    }

    // Priority 1: well-known exact filenames
    let candidates = [".zlayer.yml", ".zlayer.yaml", "zlayer.yml", "zlayer.yaml"];
    for candidate in &candidates {
        let path = dir.join(candidate);
        if path.exists() {
            return Ok(path);
        }
    }

    // Priority 2: glob for *.zlayer.yml / *.zlayer.yaml in the directory
    let glob_matches = find_zlayer_specs_in_dir(dir)?;

    match glob_matches.len() {
        1 => Ok(glob_matches.into_iter().next().unwrap()),
        0 => {
            anyhow::bail!(
                "No deployment spec found. Create a .zlayer.yml (or <name>.zlayer.yml) file, \
                 or specify a path explicitly."
            );
        }
        _ => {
            let listing: Vec<String> = glob_matches
                .iter()
                .map(|p| format!("  - {}", p.display()))
                .collect();
            anyhow::bail!(
                "Multiple deployment specs found. Specify which one to use:\n{}",
                listing.join("\n")
            );
        }
    }
}

/// Scan `dir` for files matching `*.zlayer.yml` or `*.zlayer.yaml`.
///
/// Returns a sorted vec of matching paths (sorted for deterministic output).
fn find_zlayer_specs_in_dir<P: AsRef<Path>>(dir: P) -> Result<Vec<PathBuf>> {
    let mut matches = Vec::new();

    let entries = std::fs::read_dir(dir.as_ref())
        .with_context(|| format!("Failed to read directory: {}", dir.as_ref().display()))?;

    for entry in entries {
        let entry = entry?;
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy();

        if (name.ends_with(".zlayer.yml") || name.ends_with(".zlayer.yaml"))
            && entry.file_type()?.is_file()
        {
            matches.push(entry.path());
        }
    }

    matches.sort();
    Ok(matches)
}

/// Parse and validate a deployment spec file
pub(crate) fn parse_spec(spec_path: &Path) -> Result<DeploymentSpec> {
    info!(path = %spec_path.display(), "Parsing deployment spec");

    let spec = zlayer_spec::from_yaml_file(spec_path)
        .with_context(|| format!("Failed to parse spec file: {}", spec_path.display()))?;

    info!(
        deployment = %spec.deployment,
        version = %spec.version,
        services = spec.services.len(),
        "Spec parsed successfully"
    );

    Ok(spec)
}

/// Parse an image reference into name and tag components
pub(crate) fn parse_image_reference(image: &str) -> (String, String) {
    // Handle digest references (image@sha256:...)
    if let Some(at_pos) = image.rfind('@') {
        let name = &image[..at_pos];
        let digest = &image[at_pos + 1..];
        return (name.to_string(), digest.to_string());
    }

    // Handle tag references (image:tag)
    if let Some(colon_pos) = image.rfind(':') {
        // Make sure we're not splitting on a port number
        let after_colon = &image[colon_pos + 1..];
        if !after_colon.contains('/') {
            let name = &image[..colon_pos];
            let tag = after_colon;
            return (name.to_string(), tag.to_string());
        }
    }

    // No tag specified, use "latest"
    (image.to_string(), "latest".to_string())
}

/// Parse a duration string like "1h", "30m", "3600s"
pub(crate) fn parse_duration(s: &str) -> Result<u64> {
    let s = s.trim();

    // Try to parse as plain seconds
    if let Ok(secs) = s.parse::<u64>() {
        return Ok(secs);
    }

    // Parse with suffix
    if let Some(num_str) = s.strip_suffix('s') {
        let num: u64 = num_str
            .trim()
            .parse()
            .with_context(|| format!("Invalid duration number: {}", num_str))?;
        return Ok(num);
    }

    if let Some(num_str) = s.strip_suffix('m') {
        let num: u64 = num_str
            .trim()
            .parse()
            .with_context(|| format!("Invalid duration number: {}", num_str))?;
        return Ok(num * 60);
    }

    if let Some(num_str) = s.strip_suffix('h') {
        let num: u64 = num_str
            .trim()
            .parse()
            .with_context(|| format!("Invalid duration number: {}", num_str))?;
        return Ok(num * 3600);
    }

    anyhow::bail!(
        "Invalid duration format: {}. Use formats like: 1h, 30m, 3600s",
        s
    )
}

/// Decode a base64url-encoded JSON string
pub(crate) fn decode_base64_json(input: &str) -> Result<serde_json::Value> {
    use base64::Engine;

    // JWT uses base64url encoding without padding
    // Try URL-safe first, then standard base64
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(input)
        .or_else(|_| {
            // Add padding if needed and try again
            let padded = match input.len() % 4 {
                2 => format!("{}==", input),
                3 => format!("{}=", input),
                _ => input.to_string(),
            };
            base64::engine::general_purpose::URL_SAFE.decode(&padded)
        })
        .or_else(|_| base64::engine::general_purpose::STANDARD.decode(input))
        .context("Failed to decode base64")?;

    serde_json::from_slice(&decoded).context("Failed to parse JSON")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Create a unique temp directory for each test to avoid parallel test interference.
    fn make_temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zlayer_test_{}_{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("failed to create temp dir");
        dir
    }

    fn touch(dir: &Path, filename: &str) {
        fs::write(dir.join(filename), "").expect("failed to create file");
    }

    fn cleanup(dir: &Path) {
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn exact_name_takes_priority_over_glob() {
        let dir = make_temp_dir("exact_priority");
        // Create both an exact-name match and a glob match
        touch(&dir, ".zlayer.yml");
        touch(&dir, "myapp.zlayer.yml");

        let result = discover_spec_path_in_dir(None, &dir).unwrap();
        assert_eq!(result, dir.join(".zlayer.yml"));

        cleanup(&dir);
    }

    #[test]
    fn single_glob_match_is_found() {
        let dir = make_temp_dir("single_glob");
        // No exact-name files, only a glob match
        touch(&dir, "manager.zlayer.yml");

        let result = discover_spec_path_in_dir(None, &dir).unwrap();
        assert_eq!(result, dir.join("manager.zlayer.yml"));

        cleanup(&dir);
    }

    #[test]
    fn single_glob_match_yaml_extension() {
        let dir = make_temp_dir("single_glob_yaml");
        touch(&dir, "myservice.zlayer.yaml");

        let result = discover_spec_path_in_dir(None, &dir).unwrap();
        assert_eq!(result, dir.join("myservice.zlayer.yaml"));

        cleanup(&dir);
    }

    #[test]
    fn multiple_glob_matches_produce_error() {
        let dir = make_temp_dir("multi_glob");
        touch(&dir, "alpha.zlayer.yml");
        touch(&dir, "beta.zlayer.yml");

        let err = discover_spec_path_in_dir(None, &dir).unwrap_err();
        let msg = format!("{}", err);
        assert!(
            msg.contains("Multiple deployment specs found"),
            "expected 'Multiple deployment specs found', got: {}",
            msg
        );
        assert!(msg.contains("alpha.zlayer.yml"), "should list alpha: {}", msg);
        assert!(msg.contains("beta.zlayer.yml"), "should list beta: {}", msg);

        cleanup(&dir);
    }

    #[test]
    fn no_matches_produce_error() {
        let dir = make_temp_dir("no_match");
        // Create an unrelated file so the directory is not empty
        touch(&dir, "README.md");

        let err = discover_spec_path_in_dir(None, &dir).unwrap_err();
        let msg = format!("{}", err);
        assert!(
            msg.contains("No deployment spec found"),
            "expected 'No deployment spec found', got: {}",
            msg
        );

        cleanup(&dir);
    }

    #[test]
    fn explicit_path_bypasses_discovery() {
        let dir = make_temp_dir("explicit");
        let explicit = dir.join("custom.yml");
        touch(&dir, "custom.yml");

        let result = discover_spec_path_in_dir(Some(&explicit), &dir).unwrap();
        assert_eq!(result, explicit);

        cleanup(&dir);
    }

    #[test]
    fn non_file_entries_are_ignored() {
        let dir = make_temp_dir("non_file");
        // Create a subdirectory that matches the glob pattern name-wise
        let subdir = dir.join("sneaky.zlayer.yml");
        fs::create_dir_all(&subdir).expect("failed to create subdir");

        let err = discover_spec_path_in_dir(None, &dir).unwrap_err();
        let msg = format!("{}", err);
        assert!(
            msg.contains("No deployment spec found"),
            "directories should not match: {}",
            msg
        );

        cleanup(&dir);
    }
}
