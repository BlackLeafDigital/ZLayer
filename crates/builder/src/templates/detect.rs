//! Runtime auto-detection
//!
//! This module provides functionality to automatically detect the runtime
//! of a project based on the files present in the project directory.

use std::path::Path;

use super::Runtime;

/// Detect the runtime from files in the given directory.
///
/// This function examines the project directory for characteristic files
/// that indicate which runtime the project uses. Detection order matters:
/// more specific runtimes (like Bun, Deno) are checked before generic ones (Node.js).
///
/// # Examples
///
/// ```no_run
/// use builder::templates::detect_runtime;
///
/// let runtime = detect_runtime("/path/to/project");
/// if let Some(rt) = runtime {
///     println!("Detected runtime: {:?}", rt);
/// }
/// ```
pub fn detect_runtime(context_path: impl AsRef<Path>) -> Option<Runtime> {
    let path = context_path.as_ref();

    // Check for Node.js ecosystem first (most common)
    if path.join("package.json").exists() {
        // Check for Bun - bun.lockb is definitive
        if path.join("bun.lockb").exists() {
            return Some(Runtime::Bun);
        }

        // Check for Deno - deno.json or deno.jsonc alongside package.json
        if path.join("deno.json").exists() || path.join("deno.jsonc").exists() {
            return Some(Runtime::Deno);
        }

        // Default to Node.js 20 for generic package.json projects
        return Some(Runtime::Node20);
    }

    // Check for Deno without package.json
    if path.join("deno.json").exists()
        || path.join("deno.jsonc").exists()
        || path.join("deno.lock").exists()
    {
        return Some(Runtime::Deno);
    }

    // Check for Rust
    if path.join("Cargo.toml").exists() {
        return Some(Runtime::Rust);
    }

    // Check for Python (check multiple indicators)
    if path.join("pyproject.toml").exists()
        || path.join("requirements.txt").exists()
        || path.join("setup.py").exists()
        || path.join("Pipfile").exists()
        || path.join("poetry.lock").exists()
    {
        return Some(Runtime::Python312);
    }

    // Check for Go
    if path.join("go.mod").exists() {
        return Some(Runtime::Go);
    }

    None
}

/// Detect runtime with version hints from project files.
///
/// This extends basic detection by attempting to read version specifications
/// from configuration files to choose the most appropriate version.
pub fn detect_runtime_with_version(context_path: impl AsRef<Path>) -> Option<Runtime> {
    let path = context_path.as_ref();

    // First do basic detection
    let base_runtime = detect_runtime(path)?;

    // Try to refine version based on configuration files
    match base_runtime {
        Runtime::Node20 | Runtime::Node22 => {
            // Try to read .nvmrc or .node-version
            if let Some(version) = read_node_version(path) {
                if version.starts_with("22") || version.starts_with("v22") {
                    return Some(Runtime::Node22);
                }
                if version.starts_with("20") || version.starts_with("v20") {
                    return Some(Runtime::Node20);
                }
            }

            // Try to read engines.node from package.json
            if let Some(version) = read_package_node_version(path) {
                if version.contains("22") {
                    return Some(Runtime::Node22);
                }
            }

            Some(Runtime::Node20)
        }
        Runtime::Python312 | Runtime::Python313 => {
            // Try to read python version from pyproject.toml or .python-version
            if let Some(version) = read_python_version(path) {
                if version.starts_with("3.13") {
                    return Some(Runtime::Python313);
                }
            }

            Some(Runtime::Python312)
        }
        other => Some(other),
    }
}

/// Read Node.js version from .nvmrc or .node-version files
fn read_node_version(path: &Path) -> Option<String> {
    for filename in &[".nvmrc", ".node-version"] {
        let version_file = path.join(filename);
        if version_file.exists() {
            if let Ok(content) = std::fs::read_to_string(&version_file) {
                let version = content.trim().to_string();
                if !version.is_empty() {
                    return Some(version);
                }
            }
        }
    }
    None
}

/// Read Node.js version from package.json engines field
fn read_package_node_version(path: &Path) -> Option<String> {
    let package_json = path.join("package.json");
    if package_json.exists() {
        if let Ok(content) = std::fs::read_to_string(&package_json) {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) {
                if let Some(engines) = json.get("engines") {
                    if let Some(node) = engines.get("node") {
                        if let Some(version) = node.as_str() {
                            return Some(version.to_string());
                        }
                    }
                }
            }
        }
    }
    None
}

/// Read Python version from .python-version or pyproject.toml
fn read_python_version(path: &Path) -> Option<String> {
    // Check .python-version first (used by pyenv)
    let python_version = path.join(".python-version");
    if python_version.exists() {
        if let Ok(content) = std::fs::read_to_string(&python_version) {
            let version = content.trim().to_string();
            if !version.is_empty() {
                return Some(version);
            }
        }
    }

    // Check pyproject.toml for requires-python
    let pyproject = path.join("pyproject.toml");
    if pyproject.exists() {
        if let Ok(content) = std::fs::read_to_string(&pyproject) {
            // Simple parsing - look for requires-python
            for line in content.lines() {
                let line = line.trim();
                if line.starts_with("requires-python") {
                    if let Some(version) = line.split('=').nth(1) {
                        let version = version.trim().trim_matches('"').trim_matches('\'');
                        return Some(version.to_string());
                    }
                }
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn create_temp_dir() -> TempDir {
        TempDir::new().expect("Failed to create temp directory")
    }

    #[test]
    fn test_detect_nodejs_project() {
        let dir = create_temp_dir();
        fs::write(dir.path().join("package.json"), "{}").unwrap();

        let runtime = detect_runtime(dir.path());
        assert_eq!(runtime, Some(Runtime::Node20));
    }

    #[test]
    fn test_detect_bun_project() {
        let dir = create_temp_dir();
        fs::write(dir.path().join("package.json"), "{}").unwrap();
        fs::write(dir.path().join("bun.lockb"), "").unwrap();

        let runtime = detect_runtime(dir.path());
        assert_eq!(runtime, Some(Runtime::Bun));
    }

    #[test]
    fn test_detect_deno_project() {
        let dir = create_temp_dir();
        fs::write(dir.path().join("deno.json"), "{}").unwrap();

        let runtime = detect_runtime(dir.path());
        assert_eq!(runtime, Some(Runtime::Deno));
    }

    #[test]
    fn test_detect_deno_with_package_json() {
        let dir = create_temp_dir();
        fs::write(dir.path().join("package.json"), "{}").unwrap();
        fs::write(dir.path().join("deno.json"), "{}").unwrap();

        let runtime = detect_runtime(dir.path());
        assert_eq!(runtime, Some(Runtime::Deno));
    }

    #[test]
    fn test_detect_rust_project() {
        let dir = create_temp_dir();
        fs::write(dir.path().join("Cargo.toml"), "[package]").unwrap();

        let runtime = detect_runtime(dir.path());
        assert_eq!(runtime, Some(Runtime::Rust));
    }

    #[test]
    fn test_detect_python_requirements() {
        let dir = create_temp_dir();
        fs::write(dir.path().join("requirements.txt"), "flask==2.0").unwrap();

        let runtime = detect_runtime(dir.path());
        assert_eq!(runtime, Some(Runtime::Python312));
    }

    #[test]
    fn test_detect_python_pyproject() {
        let dir = create_temp_dir();
        fs::write(dir.path().join("pyproject.toml"), "[project]").unwrap();

        let runtime = detect_runtime(dir.path());
        assert_eq!(runtime, Some(Runtime::Python312));
    }

    #[test]
    fn test_detect_go_project() {
        let dir = create_temp_dir();
        fs::write(dir.path().join("go.mod"), "module example.com/app").unwrap();

        let runtime = detect_runtime(dir.path());
        assert_eq!(runtime, Some(Runtime::Go));
    }

    #[test]
    fn test_detect_no_runtime() {
        let dir = create_temp_dir();
        // Empty directory

        let runtime = detect_runtime(dir.path());
        assert_eq!(runtime, None);
    }

    #[test]
    fn test_detect_node22_from_nvmrc() {
        let dir = create_temp_dir();
        fs::write(dir.path().join("package.json"), "{}").unwrap();
        fs::write(dir.path().join(".nvmrc"), "22.0.0").unwrap();

        let runtime = detect_runtime_with_version(dir.path());
        assert_eq!(runtime, Some(Runtime::Node22));
    }

    #[test]
    fn test_detect_node22_from_package_engines() {
        let dir = create_temp_dir();
        let package_json = r#"{"engines": {"node": ">=22.0.0"}}"#;
        fs::write(dir.path().join("package.json"), package_json).unwrap();

        let runtime = detect_runtime_with_version(dir.path());
        assert_eq!(runtime, Some(Runtime::Node22));
    }

    #[test]
    fn test_detect_python313_from_version_file() {
        let dir = create_temp_dir();
        fs::write(dir.path().join("requirements.txt"), "flask").unwrap();
        fs::write(dir.path().join(".python-version"), "3.13.0").unwrap();

        let runtime = detect_runtime_with_version(dir.path());
        assert_eq!(runtime, Some(Runtime::Python313));
    }
}
