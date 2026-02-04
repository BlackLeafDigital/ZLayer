//! ZImagefile types - YAML-based image build format
//!
//! This module defines all serde-deserializable types for the ZImagefile format,
//! an alternative to Dockerfiles using YAML syntax. The format supports four
//! mutually exclusive build modes:
//!
//! 1. **Runtime template** - shorthand like `runtime: node22`
//! 2. **Single-stage** - `base:` or `build:` + `steps:` at top level
//! 3. **Multi-stage** - `stages:` map (IndexMap for insertion order, last = output)
//! 4. **WASM** - `wasm:` configuration for WebAssembly builds

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Build context (for `build:` directive)
// ---------------------------------------------------------------------------

/// Build context for building a base image from a local Dockerfile or ZImagefile.
///
/// Supports two forms:
///
/// ```yaml
/// # Short form: just a path (defaults to auto-detecting build file)
/// build: "."
///
/// # Long form: explicit configuration
/// build:
///   context: "./subdir"     # or use `workdir:` (context is an alias)
///   file: "ZImagefile.prod" # specific build file
///   args:
///     RUST_VERSION: "1.90"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ZBuildContext {
    /// Short form: just a path to the build context directory.
    Short(String),
    /// Long form: explicit build configuration.
    Full {
        /// Build context directory. Defaults to the current working directory.
        /// `context` is accepted as an alias for Docker Compose compatibility.
        #[serde(alias = "context", default)]
        workdir: Option<String>,
        /// Path to the build file (Dockerfile or ZImagefile).
        /// Auto-detected if omitted (prefers ZImagefile over Dockerfile).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        file: Option<String>,
        /// Build arguments passed to the nested build.
        #[serde(default, skip_serializing_if = "HashMap::is_empty")]
        args: HashMap<String, String>,
    },
}

impl ZBuildContext {
    /// Resolve the build context directory relative to a base path.
    pub fn context_dir(&self, base: &Path) -> PathBuf {
        match self {
            Self::Short(path) => base.join(path),
            Self::Full { workdir, .. } => match workdir {
                Some(dir) => base.join(dir),
                None => base.to_path_buf(),
            },
        }
    }

    /// Get the explicit build file path, if specified.
    pub fn file(&self) -> Option<&str> {
        match self {
            Self::Short(_) => None,
            Self::Full { file, .. } => file.as_deref(),
        }
    }

    /// Get build arguments.
    pub fn args(&self) -> HashMap<String, String> {
        match self {
            Self::Short(_) => HashMap::new(),
            Self::Full { args, .. } => args.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Top-level ZImage
// ---------------------------------------------------------------------------

/// Top-level ZImagefile representation.
///
/// Exactly one of the four mode fields must be set:
/// - `runtime` for runtime template shorthand
/// - `base` + `steps` for single-stage builds
/// - `stages` for multi-stage builds
/// - `wasm` for WebAssembly component builds
///
/// Common image metadata fields (env, workdir, expose, cmd, etc.) apply to
/// the final output image regardless of mode.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ZImage {
    /// ZImagefile format version (currently must be "1")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,

    // -- Mode 1: runtime template shorthand --
    /// Runtime template name, e.g. "node22", "python313", "rust"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<String>,

    // -- Mode 2: single-stage --
    /// Base image for single-stage builds (e.g. "alpine:3.19").
    /// Mutually exclusive with `build`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,

    /// Build a base image from a local Dockerfile/ZImagefile context.
    /// Mutually exclusive with `base`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build: Option<ZBuildContext>,

    /// Build steps for single-stage mode
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<ZStep>,

    /// Target platform for single-stage mode (e.g. "linux/amd64")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,

    // -- Mode 3: multi-stage --
    /// Named stages for multi-stage builds. Insertion order is preserved;
    /// the last stage is the output image.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stages: Option<IndexMap<String, ZStage>>,

    // -- Mode 4: WASM --
    /// WebAssembly build configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wasm: Option<ZWasmConfig>,

    // -- Common image metadata --
    /// Environment variables applied to the final image
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub env: HashMap<String, String>,

    /// Working directory for the final image
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workdir: Option<String>,

    /// Ports to expose
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expose: Option<ZExpose>,

    /// Default command (CMD equivalent)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cmd: Option<ZCommand>,

    /// Entrypoint command
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entrypoint: Option<ZCommand>,

    /// User to run as in the final image
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,

    /// Image labels / metadata
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub labels: HashMap<String, String>,

    /// Volume mount points
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub volumes: Vec<String>,

    /// Healthcheck configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub healthcheck: Option<ZHealthcheck>,

    /// Signal to send when stopping the container
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopsignal: Option<String>,

    /// Build arguments (name -> default value)
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub args: HashMap<String, String>,
}

// ---------------------------------------------------------------------------
// Stage
// ---------------------------------------------------------------------------

/// A single build stage in a multi-stage ZImagefile.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ZStage {
    /// Base image for this stage (e.g. "node:22-alpine").
    /// Mutually exclusive with `build`. One of `base` or `build` must be set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,

    /// Build a base image from a local Dockerfile/ZImagefile context.
    /// Mutually exclusive with `base`. One of `base` or `build` must be set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build: Option<ZBuildContext>,

    /// Target platform override (e.g. "linux/arm64")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,

    /// Build arguments scoped to this stage
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub args: HashMap<String, String>,

    /// Environment variables for this stage
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub env: HashMap<String, String>,

    /// Working directory for this stage
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workdir: Option<String>,

    /// Ordered build steps
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<ZStep>,

    /// Labels for this stage
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub labels: HashMap<String, String>,

    /// Ports to expose
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expose: Option<ZExpose>,

    /// User to run as
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,

    /// Entrypoint command
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entrypoint: Option<ZCommand>,

    /// Default command
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cmd: Option<ZCommand>,

    /// Volume mount points
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub volumes: Vec<String>,

    /// Healthcheck configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub healthcheck: Option<ZHealthcheck>,

    /// Signal to send when stopping the container
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopsignal: Option<String>,
}

// ---------------------------------------------------------------------------
// Step
// ---------------------------------------------------------------------------

/// A single build instruction within a stage.
///
/// Exactly one of the action fields (`run`, `copy`, `add`, `env`, `workdir`,
/// `user`) should be set. The remaining fields are modifiers that apply to
/// the chosen action.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ZStep {
    // -- Mutually exclusive action fields --
    /// Shell command or exec-form command to run
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<ZCommand>,

    /// Source path(s) to copy into the image
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub copy: Option<ZCopySources>,

    /// Source path(s) to add (supports URLs and auto-extraction)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub add: Option<ZCopySources>,

    /// Environment variables to set
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<HashMap<String, String>>,

    /// Change working directory
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workdir: Option<String>,

    /// Change user
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,

    // -- Shared modifier fields --
    /// Destination path (for copy/add actions)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,

    /// Source stage name for cross-stage copy (replaces `--from`)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,

    /// File ownership (replaces `--chown`)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,

    /// File permissions (replaces `--chmod`)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chmod: Option<String>,

    /// Cache mounts for RUN steps
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cache: Vec<ZCacheMount>,
}

// ---------------------------------------------------------------------------
// Cache mount
// ---------------------------------------------------------------------------

/// A cache mount specification for RUN steps.
///
/// Maps to `--mount=type=cache` in Dockerfile/buildah syntax.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ZCacheMount {
    /// Target path inside the container where the cache is mounted
    pub target: String,

    /// Cache identifier (shared across builds with the same id)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,

    /// Sharing mode: "locked", "shared", or "private"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sharing: Option<String>,

    /// Whether the mount is read-only
    #[serde(default, skip_serializing_if = "crate::zimage::types::is_false")]
    pub readonly: bool,
}

// ---------------------------------------------------------------------------
// Command (shell string or exec array)
// ---------------------------------------------------------------------------

/// A command that can be specified as either a shell string or an exec-form
/// array of strings.
///
/// # YAML Examples
///
/// Shell form:
/// ```yaml
/// run: "apt-get update && apt-get install -y curl"
/// ```
///
/// Exec form:
/// ```yaml
/// cmd: ["node", "server.js"]
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ZCommand {
    /// Shell form - passed to `/bin/sh -c`
    Shell(String),
    /// Exec form - executed directly
    Exec(Vec<String>),
}

// ---------------------------------------------------------------------------
// Copy sources
// ---------------------------------------------------------------------------

/// Source specification for copy/add steps. Can be a single path or multiple.
///
/// # YAML Examples
///
/// Single source:
/// ```yaml
/// copy: "package.json"
/// ```
///
/// Multiple sources:
/// ```yaml
/// copy: ["package.json", "package-lock.json"]
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ZCopySources {
    /// A single source path
    Single(String),
    /// Multiple source paths
    Multiple(Vec<String>),
}

impl ZCopySources {
    /// Convert to a vector of source paths regardless of variant.
    pub fn to_vec(&self) -> Vec<String> {
        match self {
            Self::Single(s) => vec![s.clone()],
            Self::Multiple(v) => v.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// Expose
// ---------------------------------------------------------------------------

/// Port exposure specification. Can be a single port or multiple port specs.
///
/// # YAML Examples
///
/// Single port:
/// ```yaml
/// expose: 8080
/// ```
///
/// Multiple ports with optional protocol:
/// ```yaml
/// expose:
///   - 8080
///   - "9090/udp"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ZExpose {
    /// A single port number
    Single(u16),
    /// Multiple port specifications
    Multiple(Vec<ZPortSpec>),
}

// ---------------------------------------------------------------------------
// Port spec
// ---------------------------------------------------------------------------

/// A single port specification, either a bare port number or a port with
/// protocol suffix.
///
/// # YAML Examples
///
/// ```yaml
/// - 8080        # bare number, defaults to TCP
/// - "8080/tcp"  # explicit TCP
/// - "53/udp"    # explicit UDP
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ZPortSpec {
    /// Bare port number (defaults to TCP)
    Number(u16),
    /// Port with protocol, e.g. "8080/tcp" or "53/udp"
    WithProtocol(String),
}

// ---------------------------------------------------------------------------
// Healthcheck
// ---------------------------------------------------------------------------

/// Healthcheck configuration for the container.
///
/// # YAML Example
///
/// ```yaml
/// healthcheck:
///   cmd: "curl -f http://localhost:8080/health || exit 1"
///   interval: "30s"
///   timeout: "10s"
///   start_period: "5s"
///   retries: 3
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ZHealthcheck {
    /// Command to run for the health check
    pub cmd: ZCommand,

    /// Interval between health checks (e.g. "30s", "1m")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval: Option<String>,

    /// Timeout for each health check (e.g. "10s")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<String>,

    /// Grace period before first check (e.g. "5s")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_period: Option<String>,

    /// Number of consecutive failures before unhealthy
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retries: Option<u32>,
}

// ---------------------------------------------------------------------------
// WASM config
// ---------------------------------------------------------------------------

/// WebAssembly build configuration for WASM mode.
///
/// # YAML Example
///
/// ```yaml
/// wasm:
///   target: "preview2"
///   optimize: true
///   language: "rust"
///   wit: "./wit"
///   output: "./output.wasm"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ZWasmConfig {
    /// WASI target version (default: "preview2")
    #[serde(default = "default_wasm_target")]
    pub target: String,

    /// Whether to run wasm-opt on the output
    #[serde(default, skip_serializing_if = "crate::zimage::types::is_false")]
    pub optimize: bool,

    /// Source language (auto-detected if omitted)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,

    /// Path to WIT definitions
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wit: Option<String>,

    /// Output path for the compiled WASM file
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Default WASM target version.
fn default_wasm_target() -> String {
    "preview2".to_string()
}

/// Helper for `skip_serializing_if` on boolean fields.
fn is_false(v: &bool) -> bool {
    !v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_runtime_mode_deserialize() {
        let yaml = r#"
runtime: node22
cmd: "node server.js"
"#;
        let img: ZImage = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(img.runtime.as_deref(), Some("node22"));
        assert!(matches!(img.cmd, Some(ZCommand::Shell(ref s)) if s == "node server.js"));
    }

    #[test]
    fn test_single_stage_deserialize() {
        let yaml = r#"
base: "alpine:3.19"
steps:
  - run: "apk add --no-cache curl"
  - copy: "app.sh"
    to: "/usr/local/bin/app.sh"
    chmod: "755"
  - workdir: "/app"
env:
  NODE_ENV: production
expose: 8080
cmd: ["./app.sh"]
"#;
        let img: ZImage = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(img.base.as_deref(), Some("alpine:3.19"));
        assert_eq!(img.steps.len(), 3);
        assert_eq!(img.env.get("NODE_ENV").unwrap(), "production");
        assert!(matches!(img.expose, Some(ZExpose::Single(8080))));
        assert!(matches!(img.cmd, Some(ZCommand::Exec(ref v)) if v.len() == 1));
    }

    #[test]
    fn test_multi_stage_deserialize() {
        let yaml = r#"
stages:
  builder:
    base: "node:22-alpine"
    workdir: "/src"
    steps:
      - copy: ["package.json", "package-lock.json"]
        to: "./"
      - run: "npm ci"
      - copy: "."
        to: "."
      - run: "npm run build"
  runtime:
    base: "node:22-alpine"
    workdir: "/app"
    steps:
      - copy: "dist"
        from: builder
        to: "/app"
    cmd: ["node", "dist/index.js"]
expose: 3000
"#;
        let img: ZImage = serde_yaml::from_str(yaml).unwrap();
        let stages = img.stages.as_ref().unwrap();
        assert_eq!(stages.len(), 2);

        // Verify insertion order is preserved
        let keys: Vec<&String> = stages.keys().collect();
        assert_eq!(keys, vec!["builder", "runtime"]);

        let builder = &stages["builder"];
        assert_eq!(builder.base.as_deref(), Some("node:22-alpine"));
        assert_eq!(builder.steps.len(), 4);

        let runtime = &stages["runtime"];
        assert_eq!(runtime.steps.len(), 1);
        assert_eq!(runtime.steps[0].from.as_deref(), Some("builder"));
    }

    #[test]
    fn test_wasm_mode_deserialize() {
        let yaml = r#"
wasm:
  target: preview2
  optimize: true
  language: rust
  wit: "./wit"
  output: "./output.wasm"
"#;
        let img: ZImage = serde_yaml::from_str(yaml).unwrap();
        let wasm = img.wasm.as_ref().unwrap();
        assert_eq!(wasm.target, "preview2");
        assert!(wasm.optimize);
        assert_eq!(wasm.language.as_deref(), Some("rust"));
        assert_eq!(wasm.wit.as_deref(), Some("./wit"));
        assert_eq!(wasm.output.as_deref(), Some("./output.wasm"));
    }

    #[test]
    fn test_wasm_defaults() {
        let yaml = r#"
wasm: {}
"#;
        let img: ZImage = serde_yaml::from_str(yaml).unwrap();
        let wasm = img.wasm.as_ref().unwrap();
        assert_eq!(wasm.target, "preview2");
        assert!(!wasm.optimize);
        assert!(wasm.language.is_none());
    }

    #[test]
    fn test_zcommand_shell() {
        let yaml = r#""echo hello""#;
        let cmd: ZCommand = serde_yaml::from_str(yaml).unwrap();
        assert!(matches!(cmd, ZCommand::Shell(ref s) if s == "echo hello"));
    }

    #[test]
    fn test_zcommand_exec() {
        let yaml = r#"["echo", "hello"]"#;
        let cmd: ZCommand = serde_yaml::from_str(yaml).unwrap();
        assert!(matches!(cmd, ZCommand::Exec(ref v) if v == &["echo", "hello"]));
    }

    #[test]
    fn test_zcopy_sources_single() {
        let yaml = r#""package.json""#;
        let src: ZCopySources = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(src.to_vec(), vec!["package.json"]);
    }

    #[test]
    fn test_zcopy_sources_multiple() {
        let yaml = r#"["package.json", "tsconfig.json"]"#;
        let src: ZCopySources = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(src.to_vec(), vec!["package.json", "tsconfig.json"]);
    }

    #[test]
    fn test_zexpose_single() {
        let yaml = "8080";
        let exp: ZExpose = serde_yaml::from_str(yaml).unwrap();
        assert!(matches!(exp, ZExpose::Single(8080)));
    }

    #[test]
    fn test_zexpose_multiple() {
        let yaml = r#"
- 8080
- "9090/udp"
"#;
        let exp: ZExpose = serde_yaml::from_str(yaml).unwrap();
        if let ZExpose::Multiple(ports) = exp {
            assert_eq!(ports.len(), 2);
            assert!(matches!(ports[0], ZPortSpec::Number(8080)));
            assert!(matches!(ports[1], ZPortSpec::WithProtocol(ref s) if s == "9090/udp"));
        } else {
            panic!("Expected ZExpose::Multiple");
        }
    }

    #[test]
    fn test_healthcheck_deserialize() {
        let yaml = r#"
cmd: "curl -f http://localhost/ || exit 1"
interval: "30s"
timeout: "10s"
start_period: "5s"
retries: 3
"#;
        let hc: ZHealthcheck = serde_yaml::from_str(yaml).unwrap();
        assert!(matches!(hc.cmd, ZCommand::Shell(_)));
        assert_eq!(hc.interval.as_deref(), Some("30s"));
        assert_eq!(hc.timeout.as_deref(), Some("10s"));
        assert_eq!(hc.start_period.as_deref(), Some("5s"));
        assert_eq!(hc.retries, Some(3));
    }

    #[test]
    fn test_cache_mount_deserialize() {
        let yaml = r#"
target: /var/cache/apt
id: apt-cache
sharing: shared
readonly: false
"#;
        let cm: ZCacheMount = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cm.target, "/var/cache/apt");
        assert_eq!(cm.id.as_deref(), Some("apt-cache"));
        assert_eq!(cm.sharing.as_deref(), Some("shared"));
        assert!(!cm.readonly);
    }

    #[test]
    fn test_step_with_cache_mounts() {
        let yaml = r#"
run: "apt-get update && apt-get install -y curl"
cache:
  - target: /var/cache/apt
    id: apt-cache
    sharing: shared
  - target: /var/lib/apt
    readonly: true
"#;
        let step: ZStep = serde_yaml::from_str(yaml).unwrap();
        assert!(step.run.is_some());
        assert_eq!(step.cache.len(), 2);
        assert_eq!(step.cache[0].target, "/var/cache/apt");
        assert!(step.cache[1].readonly);
    }

    #[test]
    fn test_deny_unknown_fields_zimage() {
        let yaml = r#"
base: "alpine:3.19"
bogus_field: "should fail"
"#;
        let result: Result<ZImage, _> = serde_yaml::from_str(yaml);
        assert!(result.is_err(), "Should reject unknown fields");
    }

    #[test]
    fn test_deny_unknown_fields_zstep() {
        let yaml = r#"
run: "echo hello"
bogus: "nope"
"#;
        let result: Result<ZStep, _> = serde_yaml::from_str(yaml);
        assert!(result.is_err(), "Should reject unknown fields on ZStep");
    }

    #[test]
    fn test_roundtrip_serialize() {
        let yaml = r#"
base: "alpine:3.19"
steps:
  - run: "echo hello"
  - copy: "."
    to: "/app"
cmd: "echo done"
"#;
        let img: ZImage = serde_yaml::from_str(yaml).unwrap();
        let serialized = serde_yaml::to_string(&img).unwrap();
        let img2: ZImage = serde_yaml::from_str(&serialized).unwrap();
        assert_eq!(img.base, img2.base);
        assert_eq!(img.steps.len(), img2.steps.len());
    }
}
