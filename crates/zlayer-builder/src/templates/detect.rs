//! Runtime auto-detection
//!
//! This module provides functionality to automatically detect the runtime
//! of a project based on the files present in the project directory.

use std::path::Path;

use super::{Runtime, WasmTargetHint};

/// Detect the runtime from files in the given directory.
///
/// This function examines the project directory for characteristic files
/// that indicate which runtime the project uses. Detection order matters:
/// more specific runtimes (like Bun, Deno) are checked before generic ones (Node.js).
///
/// # Examples
///
/// ```no_run
/// use zlayer_builder::templates::detect_runtime;
///
/// let runtime = detect_runtime("/path/to/project");
/// if let Some(rt) = runtime {
///     println!("Detected runtime: {:?}", rt);
/// }
/// ```
pub fn detect_runtime(context_path: impl AsRef<Path>) -> Option<Runtime> {
    let path = context_path.as_ref();

    // WASM takes priority over language runtimes because a `Cargo.toml` with a
    // `wasm32-wasip*` target (or a `package.json` pulling in `jco`) still
    // looks like a regular Rust / Node project on the surface. We want the
    // explicit WASM indicators to win.
    if let Some(hint) = detect_wasm_hint(path) {
        return Some(Runtime::Wasm(hint));
    }

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

/// Detect whether the project at `path` targets `WebAssembly`, and if so
/// whether it builds a raw module or a WASI component.
///
/// Detection priority (first match wins):
/// 1. `cargo-component.toml` → `Component` (Rust component tooling).
/// 2. `Cargo.toml` with `[package.metadata.component]` → `Component`.
/// 3. `componentize-py.config` → `Component` (Python component tooling).
/// 4. `package.json` with `jco` / `@bytecodealliance/jco` in any dep
///    section → `Component` (JS component tooling).
/// 5. `.cargo/config.toml` selecting a `wasm32-wasip*` target → `Module`.
///
/// Returns `None` when the project shows no WASM indicators at all so the
/// caller can continue with regular language runtime detection.
fn detect_wasm_hint(path: &Path) -> Option<WasmTargetHint> {
    // 1. cargo-component.toml is a definitive Component signal.
    if path.join("cargo-component.toml").exists() {
        return Some(WasmTargetHint::Component);
    }

    // 2. Cargo.toml with [package.metadata.component] → Component.
    //    (Also: if it only has a wasip* target without component metadata,
    //    we'll fall through to step 5 and classify as Module.)
    let cargo_toml = path.join("Cargo.toml");
    let cargo_has_component_metadata = if cargo_toml.exists() {
        cargo_toml_has_component_metadata(&cargo_toml)
    } else {
        false
    };
    if cargo_has_component_metadata {
        return Some(WasmTargetHint::Component);
    }

    // 3. componentize-py.config → Component.
    if path.join("componentize-py.config").exists() {
        return Some(WasmTargetHint::Component);
    }

    // 4. package.json with jco / @bytecodealliance/jco → Component.
    if path.join("package.json").exists() && package_json_uses_jco(&path.join("package.json")) {
        return Some(WasmTargetHint::Component);
    }

    // 5. Cargo.toml + .cargo/config.toml with wasm32-wasip1 / wasip2 target → Module.
    if cargo_toml.exists() && cargo_config_targets_wasip(path) {
        return Some(WasmTargetHint::Module);
    }

    None
}

/// Return true when `Cargo.toml` declares `[package.metadata.component]`.
///
/// Uses a line-scan (no extra TOML parser dependency) which is sufficient for
/// the documented `cargo-component` layout.
fn cargo_toml_has_component_metadata(cargo_toml: &Path) -> bool {
    let Ok(content) = std::fs::read_to_string(cargo_toml) else {
        return false;
    };
    content.lines().any(|line| {
        let trimmed = line.trim();
        trimmed == "[package.metadata.component]"
            || trimmed.starts_with("[package.metadata.component.")
    })
}

/// `package.json` dependency sections scanned for the `jco` tool.
const JCO_DEP_SECTIONS: &[&str] = &[
    "dependencies",
    "devDependencies",
    "peerDependencies",
    "optionalDependencies",
];

/// Return true when `package.json` references `jco` / `@bytecodealliance/jco`
/// in any of its dependency sections.
fn package_json_uses_jco(package_json: &Path) -> bool {
    let Ok(content) = std::fs::read_to_string(package_json) else {
        return false;
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) else {
        return false;
    };

    for section in JCO_DEP_SECTIONS {
        if let Some(obj) = json.get(section).and_then(|v| v.as_object()) {
            if obj.contains_key("jco") || obj.contains_key("@bytecodealliance/jco") {
                return true;
            }
        }
    }
    false
}

/// Return true when `.cargo/config.toml` selects a `wasm32-wasip1`/`wasip2`
/// build target (line-scan, no TOML dependency).
fn cargo_config_targets_wasip(path: &Path) -> bool {
    let config = path.join(".cargo").join("config.toml");
    let Ok(content) = std::fs::read_to_string(&config) else {
        return false;
    };
    content.lines().any(|line| {
        let trimmed = line.trim();
        trimmed.contains("wasm32-wasip1") || trimmed.contains("wasm32-wasip2")
    })
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

    // -- WASM detection --------------------------------------------------

    #[test]
    fn test_detect_wasm_cargo_component_toml() {
        let dir = create_temp_dir();
        fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"foo\"").unwrap();
        fs::write(dir.path().join("cargo-component.toml"), "").unwrap();

        let runtime = detect_runtime(dir.path());
        assert_eq!(runtime, Some(Runtime::Wasm(WasmTargetHint::Component)));
    }

    #[test]
    fn test_detect_wasm_cargo_metadata_component() {
        let dir = create_temp_dir();
        let toml = r#"
[package]
name = "foo"
version = "0.1.0"

[package.metadata.component]
package = "zlayer:example"
"#;
        fs::write(dir.path().join("Cargo.toml"), toml).unwrap();

        let runtime = detect_runtime(dir.path());
        assert_eq!(runtime, Some(Runtime::Wasm(WasmTargetHint::Component)));
    }

    #[test]
    fn test_detect_wasm_componentize_py() {
        let dir = create_temp_dir();
        fs::write(dir.path().join("pyproject.toml"), "[project]").unwrap();
        fs::write(dir.path().join("componentize-py.config"), "").unwrap();

        let runtime = detect_runtime(dir.path());
        assert_eq!(runtime, Some(Runtime::Wasm(WasmTargetHint::Component)));
    }

    #[test]
    fn test_detect_wasm_jco_in_package_json() {
        let dir = create_temp_dir();
        let pkg = r#"{
  "name": "foo",
  "devDependencies": { "@bytecodealliance/jco": "^1.0.0" }
}"#;
        fs::write(dir.path().join("package.json"), pkg).unwrap();

        let runtime = detect_runtime(dir.path());
        assert_eq!(runtime, Some(Runtime::Wasm(WasmTargetHint::Component)));
    }

    #[test]
    fn test_detect_wasm_cargo_config_wasip1_module() {
        let dir = create_temp_dir();
        fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"foo\"").unwrap();
        fs::create_dir_all(dir.path().join(".cargo")).unwrap();
        fs::write(
            dir.path().join(".cargo").join("config.toml"),
            "[build]\ntarget = \"wasm32-wasip1\"\n",
        )
        .unwrap();

        let runtime = detect_runtime(dir.path());
        assert_eq!(runtime, Some(Runtime::Wasm(WasmTargetHint::Module)));
    }

    #[test]
    fn test_plain_rust_project_is_not_wasm() {
        // Cargo.toml without component metadata and without a wasip config
        // should still detect as plain Rust.
        let dir = create_temp_dir();
        fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"foo\"").unwrap();

        let runtime = detect_runtime(dir.path());
        assert_eq!(runtime, Some(Runtime::Rust));
    }

    #[test]
    fn test_plain_node_project_is_not_wasm() {
        // package.json without jco should detect as Node, not Wasm.
        let dir = create_temp_dir();
        fs::write(
            dir.path().join("package.json"),
            r#"{"dependencies":{"express":"^4"}}"#,
        )
        .unwrap();

        let runtime = detect_runtime(dir.path());
        assert_eq!(runtime, Some(Runtime::Node20));
    }
}
