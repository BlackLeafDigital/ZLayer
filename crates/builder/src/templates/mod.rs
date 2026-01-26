//! Runtime templates for ZLayer builder
//!
//! This module provides pre-built Dockerfile templates for common runtimes,
//! allowing users to build container images without writing Dockerfiles.
//!
//! # Usage
//!
//! Templates can be used via the `zlayer build` command:
//!
//! ```bash
//! # Use a specific runtime template
//! zlayer build --runtime node20
//!
//! # Auto-detect runtime from project files
//! zlayer build --detect-runtime
//! ```
//!
//! # Available Runtimes
//!
//! - **Node.js 20** (`node20`): Production-ready Node.js 20 with Alpine base
//! - **Node.js 22** (`node22`): Production-ready Node.js 22 with Alpine base
//! - **Python 3.12** (`python312`): Python 3.12 slim with pip packages
//! - **Python 3.13** (`python313`): Python 3.13 slim with pip packages
//! - **Rust** (`rust`): Static binary build with musl
//! - **Go** (`go`): Static binary build with Alpine
//! - **Deno** (`deno`): Official Deno runtime
//! - **Bun** (`bun`): Official Bun runtime
//!
//! # Auto-Detection
//!
//! The [`detect_runtime`] function can automatically detect the appropriate
//! runtime based on files present in the project directory:
//!
//! - `package.json` -> Node.js (unless Bun or Deno indicators present)
//! - `bun.lockb` -> Bun
//! - `deno.json` or `deno.jsonc` -> Deno
//! - `Cargo.toml` -> Rust
//! - `requirements.txt`, `pyproject.toml`, `setup.py` -> Python
//! - `go.mod` -> Go

mod detect;

use std::fmt;
use std::path::Path;
use std::str::FromStr;

pub use detect::{detect_runtime, detect_runtime_with_version};

/// Supported runtime environments
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Runtime {
    /// Node.js 20 (LTS)
    Node20,
    /// Node.js 22 (Current)
    Node22,
    /// Python 3.12
    Python312,
    /// Python 3.13
    Python313,
    /// Rust (latest stable)
    Rust,
    /// Go (latest stable)
    Go,
    /// Deno (latest)
    Deno,
    /// Bun (latest)
    Bun,
}

impl Runtime {
    /// Get all available runtimes
    pub fn all() -> &'static [RuntimeInfo] {
        &[
            RuntimeInfo {
                runtime: Runtime::Node20,
                name: "node20",
                description: "Node.js 20 (LTS) - Alpine-based, production optimized",
                detect_files: &["package.json"],
            },
            RuntimeInfo {
                runtime: Runtime::Node22,
                name: "node22",
                description: "Node.js 22 (Current) - Alpine-based, production optimized",
                detect_files: &["package.json"],
            },
            RuntimeInfo {
                runtime: Runtime::Python312,
                name: "python312",
                description: "Python 3.12 - Slim Debian-based with pip",
                detect_files: &["requirements.txt", "pyproject.toml", "setup.py"],
            },
            RuntimeInfo {
                runtime: Runtime::Python313,
                name: "python313",
                description: "Python 3.13 - Slim Debian-based with pip",
                detect_files: &["requirements.txt", "pyproject.toml", "setup.py"],
            },
            RuntimeInfo {
                runtime: Runtime::Rust,
                name: "rust",
                description: "Rust - Static musl binary, minimal Alpine runtime",
                detect_files: &["Cargo.toml"],
            },
            RuntimeInfo {
                runtime: Runtime::Go,
                name: "go",
                description: "Go - Static binary, minimal Alpine runtime",
                detect_files: &["go.mod"],
            },
            RuntimeInfo {
                runtime: Runtime::Deno,
                name: "deno",
                description: "Deno - Official runtime with TypeScript support",
                detect_files: &["deno.json", "deno.jsonc"],
            },
            RuntimeInfo {
                runtime: Runtime::Bun,
                name: "bun",
                description: "Bun - Fast JavaScript runtime and bundler",
                detect_files: &["bun.lockb"],
            },
        ]
    }

    /// Parse a runtime from its name
    pub fn from_name(name: &str) -> Option<Runtime> {
        let name_lower = name.to_lowercase();
        match name_lower.as_str() {
            "node20" | "node-20" | "nodejs20" | "node" => Some(Runtime::Node20),
            "node22" | "node-22" | "nodejs22" => Some(Runtime::Node22),
            "python312" | "python-312" | "python3.12" | "python" => Some(Runtime::Python312),
            "python313" | "python-313" | "python3.13" => Some(Runtime::Python313),
            "rust" | "rs" => Some(Runtime::Rust),
            "go" | "golang" => Some(Runtime::Go),
            "deno" => Some(Runtime::Deno),
            "bun" => Some(Runtime::Bun),
            _ => None,
        }
    }

    /// Get information about this runtime
    pub fn info(&self) -> &'static RuntimeInfo {
        Runtime::all()
            .iter()
            .find(|info| info.runtime == *self)
            .expect("All runtimes must have info")
    }

    /// Get the Dockerfile template for this runtime
    pub fn template(&self) -> &'static str {
        match self {
            Runtime::Node20 => include_str!("dockerfiles/node20.Dockerfile"),
            Runtime::Node22 => include_str!("dockerfiles/node22.Dockerfile"),
            Runtime::Python312 => include_str!("dockerfiles/python312.Dockerfile"),
            Runtime::Python313 => include_str!("dockerfiles/python313.Dockerfile"),
            Runtime::Rust => include_str!("dockerfiles/rust.Dockerfile"),
            Runtime::Go => include_str!("dockerfiles/go.Dockerfile"),
            Runtime::Deno => include_str!("dockerfiles/deno.Dockerfile"),
            Runtime::Bun => include_str!("dockerfiles/bun.Dockerfile"),
        }
    }

    /// Get the canonical name for this runtime
    pub fn name(&self) -> &'static str {
        self.info().name
    }
}

impl fmt::Display for Runtime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}

impl FromStr for Runtime {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Runtime::from_name(s).ok_or_else(|| format!("Unknown runtime: {}", s))
    }
}

/// Information about a runtime template
#[derive(Debug, Clone, Copy)]
pub struct RuntimeInfo {
    /// The runtime enum value
    pub runtime: Runtime,
    /// Short name used in CLI (e.g., "node20")
    pub name: &'static str,
    /// Human-readable description
    pub description: &'static str,
    /// Files that indicate this runtime should be used
    pub detect_files: &'static [&'static str],
}

/// List all available templates
pub fn list_templates() -> Vec<&'static RuntimeInfo> {
    Runtime::all().iter().collect()
}

/// Get template content for a runtime
pub fn get_template(runtime: Runtime) -> &'static str {
    runtime.template()
}

/// Get template content by runtime name
pub fn get_template_by_name(name: &str) -> Option<&'static str> {
    Runtime::from_name(name).map(|r| r.template())
}

/// Resolve runtime from either explicit name or auto-detection
pub fn resolve_runtime(
    runtime_name: Option<&str>,
    context_path: impl AsRef<Path>,
    use_version_hints: bool,
) -> Option<Runtime> {
    // If explicitly specified, use that
    if let Some(name) = runtime_name {
        return Runtime::from_name(name);
    }

    // Otherwise, auto-detect
    if use_version_hints {
        detect_runtime_with_version(context_path)
    } else {
        detect_runtime(context_path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Dockerfile;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_runtime_from_name() {
        assert_eq!(Runtime::from_name("node20"), Some(Runtime::Node20));
        assert_eq!(Runtime::from_name("Node20"), Some(Runtime::Node20));
        assert_eq!(Runtime::from_name("node"), Some(Runtime::Node20));
        assert_eq!(Runtime::from_name("python"), Some(Runtime::Python312));
        assert_eq!(Runtime::from_name("rust"), Some(Runtime::Rust));
        assert_eq!(Runtime::from_name("go"), Some(Runtime::Go));
        assert_eq!(Runtime::from_name("golang"), Some(Runtime::Go));
        assert_eq!(Runtime::from_name("deno"), Some(Runtime::Deno));
        assert_eq!(Runtime::from_name("bun"), Some(Runtime::Bun));
        assert_eq!(Runtime::from_name("unknown"), None);
    }

    #[test]
    fn test_runtime_info() {
        let info = Runtime::Node20.info();
        assert_eq!(info.name, "node20");
        assert!(info.description.contains("Node.js"));
        assert!(info.detect_files.contains(&"package.json"));
    }

    #[test]
    fn test_all_templates_parse_correctly() {
        for info in Runtime::all() {
            let template = info.runtime.template();
            let result = Dockerfile::parse(template);
            assert!(
                result.is_ok(),
                "Template {} failed to parse: {:?}",
                info.name,
                result.err()
            );

            let dockerfile = result.unwrap();
            assert!(
                !dockerfile.stages.is_empty(),
                "Template {} has no stages",
                info.name
            );
        }
    }

    #[test]
    fn test_node20_template_structure() {
        let template = Runtime::Node20.template();
        let dockerfile = Dockerfile::parse(template).expect("Should parse");

        // Should be multi-stage
        assert_eq!(dockerfile.stages.len(), 2);

        // First stage is builder
        assert_eq!(dockerfile.stages[0].name, Some("builder".to_string()));

        // Final stage should have USER instruction for security
        let final_stage = dockerfile.final_stage().unwrap();
        let has_user = final_stage
            .instructions
            .iter()
            .any(|i| matches!(i, crate::Instruction::User(_)));
        assert!(has_user, "Node template should run as non-root user");
    }

    #[test]
    fn test_rust_template_structure() {
        let template = Runtime::Rust.template();
        let dockerfile = Dockerfile::parse(template).expect("Should parse");

        // Should be multi-stage
        assert_eq!(dockerfile.stages.len(), 2);

        // First stage is builder
        assert_eq!(dockerfile.stages[0].name, Some("builder".to_string()));
    }

    #[test]
    fn test_list_templates() {
        let templates = list_templates();
        assert!(!templates.is_empty());
        assert!(templates.iter().any(|t| t.name == "node20"));
        assert!(templates.iter().any(|t| t.name == "rust"));
        assert!(templates.iter().any(|t| t.name == "go"));
    }

    #[test]
    fn test_get_template_by_name() {
        let template = get_template_by_name("node20");
        assert!(template.is_some());
        assert!(template.unwrap().contains("node:20"));

        let template = get_template_by_name("unknown");
        assert!(template.is_none());
    }

    #[test]
    fn test_resolve_runtime_explicit() {
        let dir = TempDir::new().unwrap();

        // Explicit name takes precedence
        let runtime = resolve_runtime(Some("rust"), dir.path(), false);
        assert_eq!(runtime, Some(Runtime::Rust));
    }

    #[test]
    fn test_resolve_runtime_detect() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("Cargo.toml"), "[package]").unwrap();

        // Auto-detect when no name given
        let runtime = resolve_runtime(None, dir.path(), false);
        assert_eq!(runtime, Some(Runtime::Rust));
    }

    #[test]
    fn test_runtime_display() {
        assert_eq!(format!("{}", Runtime::Node20), "node20");
        assert_eq!(format!("{}", Runtime::Rust), "rust");
    }

    #[test]
    fn test_runtime_from_str() {
        let runtime: Result<Runtime, _> = "node20".parse();
        assert_eq!(runtime, Ok(Runtime::Node20));

        let runtime: Result<Runtime, _> = "unknown".parse();
        assert!(runtime.is_err());
    }
}
