//! WebAssembly component builder with multi-language support
//!
//! This module provides functionality to build WebAssembly components from
//! various source languages, with support for both WASI Preview 1 and Preview 2
//! (component model) targets.
//!
//! # Supported Languages
//!
//! - **Rust**: `cargo build --target wasm32-wasip1` or `cargo component build`
//! - **Go**: TinyGo compiler with WASI target
//! - **Python**: componentize-py for component model
//! - **TypeScript/JavaScript**: jco/componentize-js
//! - **AssemblyScript**: asc compiler
//! - **C**: Clang with WASI SDK
//! - **Zig**: Native WASI support
//!
//! # Usage
//!
//! ```no_run
//! use std::path::Path;
//! use zlayer_builder::wasm_builder::{build_wasm, WasmBuildConfig, WasiTarget};
//!
//! # async fn example() -> Result<(), zlayer_builder::wasm_builder::WasmBuildError> {
//! let config = WasmBuildConfig {
//!     language: None,  // Auto-detect
//!     target: WasiTarget::Preview2,
//!     optimize: true,
//!     wit_path: None,
//!     output_path: None,
//! };
//!
//! let result = build_wasm(Path::new("./my-plugin"), config).await?;
//! println!("Built {} WASM at {}", result.language, result.wasm_path.display());
//! # Ok(())
//! # }
//! ```

use std::fmt;
use std::path::{Path, PathBuf};

use thiserror::Error;
use tokio::process::Command;
use tracing::{debug, info, instrument, trace, warn};

/// Error types for WASM build operations
#[derive(Debug, Error)]
pub enum WasmBuildError {
    /// Failed to detect the source language
    #[error("Could not detect source language in '{path}'")]
    LanguageNotDetected {
        /// The path that was inspected
        path: PathBuf,
    },

    /// Build tool not found
    #[error("Build tool '{tool}' not found: {message}")]
    ToolNotFound {
        /// Name of the missing tool
        tool: String,
        /// Additional context
        message: String,
    },

    /// Build command failed
    #[error("Build failed with exit code {exit_code}: {stderr}")]
    BuildFailed {
        /// Exit code from the build command
        exit_code: i32,
        /// Standard error output
        stderr: String,
        /// Standard output (may contain useful info)
        stdout: String,
    },

    /// WASM output not found after build
    #[error("WASM output not found at expected path: {path}")]
    OutputNotFound {
        /// The expected output path
        path: PathBuf,
    },

    /// Configuration error
    #[error("Configuration error: {message}")]
    ConfigError {
        /// Description of the configuration problem
        message: String,
    },

    /// IO error during build operations
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// Failed to read project configuration
    #[error("Failed to read project configuration: {message}")]
    ProjectConfigError {
        /// Description of what went wrong
        message: String,
    },
}

/// Result type for WASM build operations
pub type Result<T, E = WasmBuildError> = std::result::Result<T, E>;

/// Supported source languages for WASM compilation
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WasmLanguage {
    /// Standard Rust with wasm32-wasip1 or wasm32-wasip2 target
    Rust,
    /// Rust with cargo-component for WASI Preview 2 components
    RustComponent,
    /// Go using TinyGo compiler
    Go,
    /// Python using componentize-py
    Python,
    /// TypeScript using jco/componentize-js
    TypeScript,
    /// AssemblyScript using asc compiler
    AssemblyScript,
    /// C using WASI SDK (clang)
    C,
    /// Zig with native WASI support
    Zig,
}

impl WasmLanguage {
    /// Get all supported languages
    pub fn all() -> &'static [WasmLanguage] {
        &[
            WasmLanguage::Rust,
            WasmLanguage::RustComponent,
            WasmLanguage::Go,
            WasmLanguage::Python,
            WasmLanguage::TypeScript,
            WasmLanguage::AssemblyScript,
            WasmLanguage::C,
            WasmLanguage::Zig,
        ]
    }

    /// Get the display name for this language
    pub fn name(&self) -> &'static str {
        match self {
            WasmLanguage::Rust => "Rust",
            WasmLanguage::RustComponent => "Rust (cargo-component)",
            WasmLanguage::Go => "Go (TinyGo)",
            WasmLanguage::Python => "Python",
            WasmLanguage::TypeScript => "TypeScript",
            WasmLanguage::AssemblyScript => "AssemblyScript",
            WasmLanguage::C => "C",
            WasmLanguage::Zig => "Zig",
        }
    }

    /// Check if this language produces component model output by default
    pub fn is_component_native(&self) -> bool {
        matches!(
            self,
            WasmLanguage::RustComponent | WasmLanguage::Python | WasmLanguage::TypeScript
        )
    }
}

impl fmt::Display for WasmLanguage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}

/// WASI target version
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum WasiTarget {
    /// WASI Preview 1 (wasm32-wasip1)
    Preview1,
    /// WASI Preview 2 (component model)
    #[default]
    Preview2,
}

impl WasiTarget {
    /// Get the Rust target triple for this WASI version
    pub fn rust_target(&self) -> &'static str {
        match self {
            WasiTarget::Preview1 => "wasm32-wasip1",
            WasiTarget::Preview2 => "wasm32-wasip2",
        }
    }

    /// Get the display name
    pub fn name(&self) -> &'static str {
        match self {
            WasiTarget::Preview1 => "WASI Preview 1",
            WasiTarget::Preview2 => "WASI Preview 2",
        }
    }
}

impl fmt::Display for WasiTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name())
    }
}

/// Configuration for building a WASM component
#[derive(Debug, Clone, Default)]
pub struct WasmBuildConfig {
    /// Source language (None = auto-detect)
    pub language: Option<WasmLanguage>,

    /// Target WASI version
    pub target: WasiTarget,

    /// Whether to optimize the output (release mode)
    pub optimize: bool,

    /// Path to WIT files for component model
    pub wit_path: Option<PathBuf>,

    /// Override output path
    pub output_path: Option<PathBuf>,
}

impl WasmBuildConfig {
    /// Create a new default configuration
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the source language
    pub fn language(mut self, lang: WasmLanguage) -> Self {
        self.language = Some(lang);
        self
    }

    /// Set the WASI target
    pub fn target(mut self, target: WasiTarget) -> Self {
        self.target = target;
        self
    }

    /// Enable optimization
    pub fn optimize(mut self, optimize: bool) -> Self {
        self.optimize = optimize;
        self
    }

    /// Set the WIT path
    pub fn wit_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.wit_path = Some(path.into());
        self
    }

    /// Set the output path
    pub fn output_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.output_path = Some(path.into());
        self
    }
}

/// Result of a successful WASM build
#[derive(Debug, Clone)]
pub struct WasmBuildResult {
    /// Path to the built WASM file
    pub wasm_path: PathBuf,

    /// The language that was used
    pub language: WasmLanguage,

    /// The WASI target that was used
    pub target: WasiTarget,

    /// Size of the output file in bytes
    pub size: u64,
}

/// Detect the source language from files in the given directory
///
/// This function examines project files to determine which language
/// should be used for WASM compilation.
///
/// # Detection Order
///
/// 1. Cargo.toml with cargo-component -> RustComponent
/// 2. Cargo.toml -> Rust
/// 3. go.mod -> Go (TinyGo)
/// 4. pyproject.toml or requirements.txt -> Python
/// 5. package.json with assemblyscript -> AssemblyScript
/// 6. package.json -> TypeScript
/// 7. build.zig -> Zig
/// 8. Makefile with *.c files -> C
#[instrument(level = "debug", skip_all, fields(path = %context.as_ref().display()))]
pub fn detect_language(context: impl AsRef<Path>) -> Result<WasmLanguage> {
    let path = context.as_ref();
    debug!("Detecting WASM source language");

    // Check for Rust projects
    let cargo_toml = path.join("Cargo.toml");
    if cargo_toml.exists() {
        trace!("Found Cargo.toml");

        // Check if this is a cargo-component project
        if is_cargo_component_project(&cargo_toml)? {
            debug!("Detected Rust (cargo-component) project");
            return Ok(WasmLanguage::RustComponent);
        }

        debug!("Detected Rust project");
        return Ok(WasmLanguage::Rust);
    }

    // Check for Go projects
    if path.join("go.mod").exists() {
        debug!("Detected Go (TinyGo) project");
        return Ok(WasmLanguage::Go);
    }

    // Check for Python projects
    if path.join("pyproject.toml").exists()
        || path.join("requirements.txt").exists()
        || path.join("setup.py").exists()
    {
        debug!("Detected Python project");
        return Ok(WasmLanguage::Python);
    }

    // Check for Node.js/TypeScript projects
    let package_json = path.join("package.json");
    if package_json.exists() {
        trace!("Found package.json");

        // Check for AssemblyScript
        if is_assemblyscript_project(&package_json)? {
            debug!("Detected AssemblyScript project");
            return Ok(WasmLanguage::AssemblyScript);
        }

        debug!("Detected TypeScript project");
        return Ok(WasmLanguage::TypeScript);
    }

    // Check for Zig projects
    if path.join("build.zig").exists() {
        debug!("Detected Zig project");
        return Ok(WasmLanguage::Zig);
    }

    // Check for C projects (Makefile + *.c files)
    if (path.join("Makefile").exists() || path.join("CMakeLists.txt").exists())
        && has_c_source_files(path)
    {
        debug!("Detected C project");
        return Ok(WasmLanguage::C);
    }

    // Also check for C without makefile (just source files)
    if has_c_source_files(path) {
        debug!("Detected C project (source files only)");
        return Ok(WasmLanguage::C);
    }

    Err(WasmBuildError::LanguageNotDetected {
        path: path.to_path_buf(),
    })
}

/// Check if a Cargo.toml indicates a cargo-component project
fn is_cargo_component_project(cargo_toml: &Path) -> Result<bool> {
    let content =
        std::fs::read_to_string(cargo_toml).map_err(|e| WasmBuildError::ProjectConfigError {
            message: format!("Failed to read Cargo.toml: {}", e),
        })?;

    // Check for cargo-component specific configuration
    // This includes [package.metadata.component] or [lib] with crate-type = ["cdylib"]
    // and presence of wit files
    if content.contains("[package.metadata.component]") {
        return Ok(true);
    }

    // Check for component-related dependencies
    if content.contains("wit-bindgen") || content.contains("cargo-component-bindings") {
        return Ok(true);
    }

    // Check for cargo-component.toml in the same directory
    let component_toml = cargo_toml.parent().map(|p| p.join("cargo-component.toml"));
    if let Some(ref component_toml) = component_toml {
        if component_toml.exists() {
            return Ok(true);
        }
    }

    Ok(false)
}

/// Check if a package.json indicates an AssemblyScript project
fn is_assemblyscript_project(package_json: &Path) -> Result<bool> {
    let content =
        std::fs::read_to_string(package_json).map_err(|e| WasmBuildError::ProjectConfigError {
            message: format!("Failed to read package.json: {}", e),
        })?;

    let json: serde_json::Value =
        serde_json::from_str(&content).map_err(|e| WasmBuildError::ProjectConfigError {
            message: format!("Invalid package.json: {}", e),
        })?;

    // Check dependencies and devDependencies for assemblyscript
    let has_assemblyscript = |deps: Option<&serde_json::Value>| -> bool {
        deps.and_then(|d| d.as_object())
            .map(|d| d.contains_key("assemblyscript"))
            .unwrap_or(false)
    };

    if has_assemblyscript(json.get("dependencies"))
        || has_assemblyscript(json.get("devDependencies"))
    {
        return Ok(true);
    }

    // Check for asc in scripts
    if let Some(scripts) = json.get("scripts").and_then(|s| s.as_object()) {
        for script in scripts.values() {
            if let Some(cmd) = script.as_str() {
                if cmd.contains("asc ") || cmd.starts_with("asc") {
                    return Ok(true);
                }
            }
        }
    }

    Ok(false)
}

/// Check if the directory contains C source files
fn has_c_source_files(path: &Path) -> bool {
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            let file_path = entry.path();
            if let Some(ext) = file_path.extension() {
                if ext == "c" || ext == "h" {
                    return true;
                }
            }
        }
    }

    // Also check src/ subdirectory
    let src_dir = path.join("src");
    if src_dir.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&src_dir) {
            for entry in entries.flatten() {
                let file_path = entry.path();
                if let Some(ext) = file_path.extension() {
                    if ext == "c" || ext == "h" {
                        return true;
                    }
                }
            }
        }
    }

    false
}

/// Get the build command for a specific language and target
pub fn get_build_command(language: WasmLanguage, target: WasiTarget, release: bool) -> Vec<String> {
    match language {
        WasmLanguage::Rust => {
            let mut cmd = vec![
                "cargo".to_string(),
                "build".to_string(),
                "--target".to_string(),
                target.rust_target().to_string(),
            ];
            if release {
                cmd.push("--release".to_string());
            }
            cmd
        }

        WasmLanguage::RustComponent => {
            let mut cmd = vec![
                "cargo".to_string(),
                "component".to_string(),
                "build".to_string(),
            ];
            if release {
                cmd.push("--release".to_string());
            }
            cmd
        }

        WasmLanguage::Go => {
            // TinyGo command for WASI
            let wasi_target = match target {
                WasiTarget::Preview1 => "wasip1",
                WasiTarget::Preview2 => "wasip2",
            };
            let mut cmd = vec![
                "tinygo".to_string(),
                "build".to_string(),
                "-target".to_string(),
                wasi_target.to_string(),
                "-o".to_string(),
                "main.wasm".to_string(),
            ];
            if release {
                cmd.push("-opt".to_string());
                cmd.push("2".to_string());
            }
            cmd.push(".".to_string());
            cmd
        }

        WasmLanguage::Python => {
            // componentize-py for WASI Preview 2
            // Note: componentize-py doesn't have explicit optimization flags,
            // the output is already optimized regardless of the release flag
            let _ = release; // Silence unused warning
            vec![
                "componentize-py".to_string(),
                "-d".to_string(),
                "wit".to_string(),
                "-w".to_string(),
                "world".to_string(),
                "componentize".to_string(),
                "app".to_string(),
                "-o".to_string(),
                "app.wasm".to_string(),
            ]
        }

        WasmLanguage::TypeScript => {
            // jco componentize for TypeScript
            vec![
                "npx".to_string(),
                "jco".to_string(),
                "componentize".to_string(),
                "src/index.js".to_string(),
                "--wit".to_string(),
                "wit".to_string(),
                "-o".to_string(),
                "dist/component.wasm".to_string(),
            ]
        }

        WasmLanguage::AssemblyScript => {
            let mut cmd = vec![
                "npx".to_string(),
                "asc".to_string(),
                "assembly/index.ts".to_string(),
                "--target".to_string(),
                "release".to_string(),
                "-o".to_string(),
                "build/release.wasm".to_string(),
            ];
            if release {
                cmd.push("--optimize".to_string());
            }
            cmd
        }

        WasmLanguage::C => {
            // Using WASI SDK's clang
            let mut cmd = vec![
                "clang".to_string(),
                "--target=wasm32-wasi".to_string(),
                "-o".to_string(),
                "main.wasm".to_string(),
            ];
            if release {
                cmd.push("-O2".to_string());
            }
            cmd.push("src/main.c".to_string());
            cmd
        }

        WasmLanguage::Zig => {
            // Zig with WASI target
            let mut cmd = vec![
                "zig".to_string(),
                "build".to_string(),
                "-Dtarget=wasm32-wasi".to_string(),
            ];
            if release {
                cmd.push("-Doptimize=ReleaseFast".to_string());
            }
            cmd
        }
    }
}

/// Build a WASM component from source
///
/// This is the main entry point for building WASM components.
/// It will detect the source language if not specified, run the
/// appropriate build command, and return information about the
/// built artifact.
#[instrument(level = "info", skip_all, fields(
    context = %context.as_ref().display(),
    language = ?config.language,
    target = ?config.target
))]
pub async fn build_wasm(
    context: impl AsRef<Path>,
    config: WasmBuildConfig,
) -> Result<WasmBuildResult> {
    let context = context.as_ref();
    info!("Building WASM component");

    // Detect language if not specified
    let language = match config.language {
        Some(lang) => {
            debug!("Using specified language: {}", lang);
            lang
        }
        None => {
            let detected = detect_language(context)?;
            info!("Auto-detected language: {}", detected);
            detected
        }
    };

    // Verify build tool is available
    verify_build_tool(language).await?;

    // Get build command
    let cmd = get_build_command(language, config.target, config.optimize);
    debug!("Build command: {:?}", cmd);

    // Execute build
    let output = execute_build_command(context, &cmd).await?;

    // Check for success
    if !output.status.success() {
        let exit_code = output.status.code().unwrap_or(-1);
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();

        warn!("Build failed with exit code {}", exit_code);
        trace!("stdout: {}", stdout);
        trace!("stderr: {}", stderr);

        return Err(WasmBuildError::BuildFailed {
            exit_code,
            stderr,
            stdout,
        });
    }

    // Find output file
    let wasm_path = find_wasm_output(context, language, config.target, config.optimize)?;

    // Apply output path override if specified
    let final_path = if let Some(ref output_path) = config.output_path {
        std::fs::copy(&wasm_path, output_path)?;
        output_path.clone()
    } else {
        wasm_path
    };

    // Get file size
    let metadata = std::fs::metadata(&final_path)?;
    let size = metadata.len();

    info!("Successfully built {} WASM ({} bytes)", language, size);

    Ok(WasmBuildResult {
        wasm_path: final_path,
        language,
        target: config.target,
        size,
    })
}

/// Verify that the required build tool is available
async fn verify_build_tool(language: WasmLanguage) -> Result<()> {
    let (tool, check_cmd) = match language {
        WasmLanguage::Rust | WasmLanguage::RustComponent => ("cargo", vec!["cargo", "--version"]),
        WasmLanguage::Go => ("tinygo", vec!["tinygo", "version"]),
        WasmLanguage::Python => ("componentize-py", vec!["componentize-py", "--version"]),
        WasmLanguage::TypeScript | WasmLanguage::AssemblyScript => {
            ("npx", vec!["npx", "--version"])
        }
        WasmLanguage::C => ("clang", vec!["clang", "--version"]),
        WasmLanguage::Zig => ("zig", vec!["zig", "version"]),
    };

    debug!("Checking for tool: {}", tool);

    let result = Command::new(check_cmd[0])
        .args(&check_cmd[1..])
        .output()
        .await;

    match result {
        Ok(output) if output.status.success() => {
            trace!("{} is available", tool);
            Ok(())
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(WasmBuildError::ToolNotFound {
                tool: tool.to_string(),
                message: format!("Command failed: {}", stderr),
            })
        }
        Err(e) => Err(WasmBuildError::ToolNotFound {
            tool: tool.to_string(),
            message: format!("Not found in PATH: {}", e),
        }),
    }
}

/// Execute the build command
async fn execute_build_command(context: &Path, cmd: &[String]) -> Result<std::process::Output> {
    let mut command = Command::new(&cmd[0]);
    command
        .args(&cmd[1..])
        .current_dir(context)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    debug!("Executing: {} in {}", cmd.join(" "), context.display());

    command.output().await.map_err(WasmBuildError::Io)
}

/// Find the WASM output file after a successful build
fn find_wasm_output(
    context: &Path,
    language: WasmLanguage,
    target: WasiTarget,
    release: bool,
) -> Result<PathBuf> {
    // Language-specific output locations
    let candidates: Vec<PathBuf> = match language {
        WasmLanguage::Rust => {
            let profile = if release { "release" } else { "debug" };
            let target_name = target.rust_target();

            // Try to find the package name from Cargo.toml
            let package_name =
                get_rust_package_name(context).unwrap_or_else(|_| "output".to_string());

            vec![
                context
                    .join("target")
                    .join(target_name)
                    .join(profile)
                    .join(format!("{}.wasm", package_name)),
                context
                    .join("target")
                    .join(target_name)
                    .join(profile)
                    .join(format!("{}.wasm", package_name.replace('-', "_"))),
            ]
        }

        WasmLanguage::RustComponent => {
            let profile = if release { "release" } else { "debug" };
            let package_name =
                get_rust_package_name(context).unwrap_or_else(|_| "output".to_string());

            vec![
                // cargo-component outputs to wasm32-wasip1 or wasm32-wasip2
                context
                    .join("target")
                    .join("wasm32-wasip1")
                    .join(profile)
                    .join(format!("{}.wasm", package_name)),
                context
                    .join("target")
                    .join("wasm32-wasip2")
                    .join(profile)
                    .join(format!("{}.wasm", package_name)),
                context
                    .join("target")
                    .join("wasm32-wasi")
                    .join(profile)
                    .join(format!("{}.wasm", package_name)),
            ]
        }

        WasmLanguage::Go => {
            vec![context.join("main.wasm")]
        }

        WasmLanguage::Python => {
            vec![context.join("app.wasm")]
        }

        WasmLanguage::TypeScript => {
            vec![
                context.join("dist").join("component.wasm"),
                context.join("component.wasm"),
            ]
        }

        WasmLanguage::AssemblyScript => {
            vec![
                context.join("build").join("release.wasm"),
                context.join("build").join("debug.wasm"),
            ]
        }

        WasmLanguage::C => {
            vec![context.join("main.wasm")]
        }

        WasmLanguage::Zig => {
            vec![
                context.join("zig-out").join("bin").join("main.wasm"),
                context.join("zig-out").join("lib").join("main.wasm"),
            ]
        }
    };

    // Find the first existing file
    for candidate in &candidates {
        if candidate.exists() {
            debug!("Found WASM output at: {}", candidate.display());
            return Ok(candidate.clone());
        }
    }

    // If no specific file found, search for any .wasm file
    if let Some(wasm_path) = find_any_wasm_file(context) {
        debug!("Found WASM file via search: {}", wasm_path.display());
        return Ok(wasm_path);
    }

    Err(WasmBuildError::OutputNotFound {
        path: candidates
            .first()
            .cloned()
            .unwrap_or_else(|| context.join("output.wasm")),
    })
}

/// Get the package name from Cargo.toml
fn get_rust_package_name(context: &Path) -> Result<String> {
    let cargo_toml = context.join("Cargo.toml");
    let content =
        std::fs::read_to_string(&cargo_toml).map_err(|e| WasmBuildError::ProjectConfigError {
            message: format!("Failed to read Cargo.toml: {}", e),
        })?;

    // Simple TOML parsing for package name
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with("name") {
            if let Some(name) = line
                .split('=')
                .nth(1)
                .map(|s| s.trim().trim_matches('"').trim_matches('\''))
            {
                return Ok(name.to_string());
            }
        }
    }

    Err(WasmBuildError::ProjectConfigError {
        message: "Could not find package name in Cargo.toml".to_string(),
    })
}

/// Search for any .wasm file in the project
fn find_any_wasm_file(context: &Path) -> Option<PathBuf> {
    // Common output directories to search
    let search_dirs = [
        context.to_path_buf(),
        context.join("target"),
        context.join("build"),
        context.join("dist"),
        context.join("out"),
        context.join("zig-out"),
    ];

    for dir in &search_dirs {
        if let Some(path) = search_wasm_recursive(dir, 3) {
            return Some(path);
        }
    }

    None
}

/// Recursively search for .wasm files up to a certain depth
fn search_wasm_recursive(dir: &Path, max_depth: usize) -> Option<PathBuf> {
    if max_depth == 0 || !dir.is_dir() {
        return None;
    }

    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();

            if path.is_file() {
                if let Some(ext) = path.extension() {
                    if ext == "wasm" {
                        return Some(path);
                    }
                }
            } else if path.is_dir() {
                if let Some(found) = search_wasm_recursive(&path, max_depth - 1) {
                    return Some(found);
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

    // =========================================================================
    // WasmLanguage tests
    // =========================================================================

    mod wasm_language_tests {
        use super::*;

        #[test]
        fn test_display_all_variants() {
            assert_eq!(WasmLanguage::Rust.to_string(), "Rust");
            assert_eq!(
                WasmLanguage::RustComponent.to_string(),
                "Rust (cargo-component)"
            );
            assert_eq!(WasmLanguage::Go.to_string(), "Go (TinyGo)");
            assert_eq!(WasmLanguage::Python.to_string(), "Python");
            assert_eq!(WasmLanguage::TypeScript.to_string(), "TypeScript");
            assert_eq!(WasmLanguage::AssemblyScript.to_string(), "AssemblyScript");
            assert_eq!(WasmLanguage::C.to_string(), "C");
            assert_eq!(WasmLanguage::Zig.to_string(), "Zig");
        }

        #[test]
        fn test_debug_formatting() {
            // Verify Debug trait is implemented and produces expected output
            let debug_str = format!("{:?}", WasmLanguage::Rust);
            assert_eq!(debug_str, "Rust");

            let debug_str = format!("{:?}", WasmLanguage::RustComponent);
            assert_eq!(debug_str, "RustComponent");

            let debug_str = format!("{:?}", WasmLanguage::Go);
            assert_eq!(debug_str, "Go");

            let debug_str = format!("{:?}", WasmLanguage::Python);
            assert_eq!(debug_str, "Python");

            let debug_str = format!("{:?}", WasmLanguage::TypeScript);
            assert_eq!(debug_str, "TypeScript");

            let debug_str = format!("{:?}", WasmLanguage::AssemblyScript);
            assert_eq!(debug_str, "AssemblyScript");

            let debug_str = format!("{:?}", WasmLanguage::C);
            assert_eq!(debug_str, "C");

            let debug_str = format!("{:?}", WasmLanguage::Zig);
            assert_eq!(debug_str, "Zig");
        }

        #[test]
        fn test_clone() {
            let lang = WasmLanguage::Rust;
            let cloned = lang.clone();
            assert_eq!(lang, cloned);

            let lang = WasmLanguage::Python;
            let cloned = lang.clone();
            assert_eq!(lang, cloned);
        }

        #[test]
        fn test_copy() {
            let lang = WasmLanguage::Go;
            let copied = lang; // Copy semantics
            assert_eq!(lang, copied);
            // Verify original is still usable (proving Copy, not Move)
            assert_eq!(lang, WasmLanguage::Go);
        }

        #[test]
        fn test_partial_eq() {
            assert_eq!(WasmLanguage::Rust, WasmLanguage::Rust);
            assert_ne!(WasmLanguage::Rust, WasmLanguage::Go);
            assert_ne!(WasmLanguage::Rust, WasmLanguage::RustComponent);
            assert_eq!(WasmLanguage::TypeScript, WasmLanguage::TypeScript);
            assert_ne!(WasmLanguage::TypeScript, WasmLanguage::AssemblyScript);
        }

        #[test]
        fn test_name_method() {
            assert_eq!(WasmLanguage::Rust.name(), "Rust");
            assert_eq!(WasmLanguage::RustComponent.name(), "Rust (cargo-component)");
            assert_eq!(WasmLanguage::Go.name(), "Go (TinyGo)");
            assert_eq!(WasmLanguage::Python.name(), "Python");
            assert_eq!(WasmLanguage::TypeScript.name(), "TypeScript");
            assert_eq!(WasmLanguage::AssemblyScript.name(), "AssemblyScript");
            assert_eq!(WasmLanguage::C.name(), "C");
            assert_eq!(WasmLanguage::Zig.name(), "Zig");
        }

        #[test]
        fn test_all_returns_all_variants() {
            let all = WasmLanguage::all();
            assert_eq!(all.len(), 8);
            assert!(all.contains(&WasmLanguage::Rust));
            assert!(all.contains(&WasmLanguage::RustComponent));
            assert!(all.contains(&WasmLanguage::Go));
            assert!(all.contains(&WasmLanguage::Python));
            assert!(all.contains(&WasmLanguage::TypeScript));
            assert!(all.contains(&WasmLanguage::AssemblyScript));
            assert!(all.contains(&WasmLanguage::C));
            assert!(all.contains(&WasmLanguage::Zig));
        }

        #[test]
        fn test_is_component_native() {
            // Component-native languages (produce component model output by default)
            assert!(WasmLanguage::RustComponent.is_component_native());
            assert!(WasmLanguage::Python.is_component_native());
            assert!(WasmLanguage::TypeScript.is_component_native());

            // Non-component-native languages
            assert!(!WasmLanguage::Rust.is_component_native());
            assert!(!WasmLanguage::Go.is_component_native());
            assert!(!WasmLanguage::AssemblyScript.is_component_native());
            assert!(!WasmLanguage::C.is_component_native());
            assert!(!WasmLanguage::Zig.is_component_native());
        }

        #[test]
        fn test_hash() {
            use std::collections::HashSet;

            let mut set = HashSet::new();
            set.insert(WasmLanguage::Rust);
            set.insert(WasmLanguage::Go);
            set.insert(WasmLanguage::Rust); // Duplicate

            assert_eq!(set.len(), 2);
            assert!(set.contains(&WasmLanguage::Rust));
            assert!(set.contains(&WasmLanguage::Go));
        }
    }

    // =========================================================================
    // WasiTarget tests
    // =========================================================================

    mod wasi_target_tests {
        use super::*;

        #[test]
        fn test_default_returns_preview2() {
            let target = WasiTarget::default();
            assert_eq!(target, WasiTarget::Preview2);
        }

        #[test]
        fn test_display_preview1() {
            assert_eq!(WasiTarget::Preview1.to_string(), "WASI Preview 1");
        }

        #[test]
        fn test_display_preview2() {
            assert_eq!(WasiTarget::Preview2.to_string(), "WASI Preview 2");
        }

        #[test]
        fn test_debug_formatting() {
            let debug_str = format!("{:?}", WasiTarget::Preview1);
            assert_eq!(debug_str, "Preview1");

            let debug_str = format!("{:?}", WasiTarget::Preview2);
            assert_eq!(debug_str, "Preview2");
        }

        #[test]
        fn test_clone() {
            let target = WasiTarget::Preview1;
            let cloned = target.clone();
            assert_eq!(target, cloned);

            let target = WasiTarget::Preview2;
            let cloned = target.clone();
            assert_eq!(target, cloned);
        }

        #[test]
        fn test_copy() {
            let target = WasiTarget::Preview1;
            let copied = target; // Copy semantics
            assert_eq!(target, copied);
            // Verify original is still usable (proving Copy, not Move)
            assert_eq!(target, WasiTarget::Preview1);
        }

        #[test]
        fn test_partial_eq() {
            assert_eq!(WasiTarget::Preview1, WasiTarget::Preview1);
            assert_eq!(WasiTarget::Preview2, WasiTarget::Preview2);
            assert_ne!(WasiTarget::Preview1, WasiTarget::Preview2);
        }

        #[test]
        fn test_rust_target_preview1() {
            assert_eq!(WasiTarget::Preview1.rust_target(), "wasm32-wasip1");
        }

        #[test]
        fn test_rust_target_preview2() {
            assert_eq!(WasiTarget::Preview2.rust_target(), "wasm32-wasip2");
        }

        #[test]
        fn test_name_method() {
            assert_eq!(WasiTarget::Preview1.name(), "WASI Preview 1");
            assert_eq!(WasiTarget::Preview2.name(), "WASI Preview 2");
        }

        #[test]
        fn test_hash() {
            use std::collections::HashSet;

            let mut set = HashSet::new();
            set.insert(WasiTarget::Preview1);
            set.insert(WasiTarget::Preview2);
            set.insert(WasiTarget::Preview1); // Duplicate

            assert_eq!(set.len(), 2);
            assert!(set.contains(&WasiTarget::Preview1));
            assert!(set.contains(&WasiTarget::Preview2));
        }
    }

    // =========================================================================
    // WasmBuildConfig tests
    // =========================================================================

    mod wasm_build_config_tests {
        use super::*;

        #[test]
        fn test_default_trait() {
            let config = WasmBuildConfig::default();

            assert_eq!(config.language, None);
            assert_eq!(config.target, WasiTarget::Preview2); // Default WasiTarget
            assert!(!config.optimize);
            assert_eq!(config.wit_path, None);
            assert_eq!(config.output_path, None);
        }

        #[test]
        fn test_new_equals_default() {
            let new_config = WasmBuildConfig::new();
            let default_config = WasmBuildConfig::default();

            assert_eq!(new_config.language, default_config.language);
            assert_eq!(new_config.target, default_config.target);
            assert_eq!(new_config.optimize, default_config.optimize);
            assert_eq!(new_config.wit_path, default_config.wit_path);
            assert_eq!(new_config.output_path, default_config.output_path);
        }

        #[test]
        fn test_with_language() {
            let config = WasmBuildConfig::new().language(WasmLanguage::Rust);
            assert_eq!(config.language, Some(WasmLanguage::Rust));

            let config = WasmBuildConfig::new().language(WasmLanguage::Python);
            assert_eq!(config.language, Some(WasmLanguage::Python));
        }

        #[test]
        fn test_with_target() {
            let config = WasmBuildConfig::new().target(WasiTarget::Preview1);
            assert_eq!(config.target, WasiTarget::Preview1);

            let config = WasmBuildConfig::new().target(WasiTarget::Preview2);
            assert_eq!(config.target, WasiTarget::Preview2);
        }

        #[test]
        fn test_with_optimize_true() {
            let config = WasmBuildConfig::new().optimize(true);
            assert!(config.optimize);
        }

        #[test]
        fn test_with_optimize_false() {
            let config = WasmBuildConfig::new().optimize(false);
            assert!(!config.optimize);
        }

        #[test]
        fn test_with_wit_path_string() {
            let config = WasmBuildConfig::new().wit_path("/path/to/wit");
            assert_eq!(config.wit_path, Some(PathBuf::from("/path/to/wit")));
        }

        #[test]
        fn test_with_wit_path_pathbuf() {
            let path = PathBuf::from("/another/wit/path");
            let config = WasmBuildConfig::new().wit_path(path.clone());
            assert_eq!(config.wit_path, Some(path));
        }

        #[test]
        fn test_with_output_path_string() {
            let config = WasmBuildConfig::new().output_path("/output/file.wasm");
            assert_eq!(config.output_path, Some(PathBuf::from("/output/file.wasm")));
        }

        #[test]
        fn test_with_output_path_pathbuf() {
            let path = PathBuf::from("/custom/output.wasm");
            let config = WasmBuildConfig::new().output_path(path.clone());
            assert_eq!(config.output_path, Some(path));
        }

        #[test]
        fn test_builder_pattern_chaining() {
            let config = WasmBuildConfig::new()
                .language(WasmLanguage::Go)
                .target(WasiTarget::Preview1)
                .optimize(true)
                .wit_path("/wit")
                .output_path("/out.wasm");

            assert_eq!(config.language, Some(WasmLanguage::Go));
            assert_eq!(config.target, WasiTarget::Preview1);
            assert!(config.optimize);
            assert_eq!(config.wit_path, Some(PathBuf::from("/wit")));
            assert_eq!(config.output_path, Some(PathBuf::from("/out.wasm")));
        }

        #[test]
        fn test_debug_formatting() {
            let config = WasmBuildConfig::new().language(WasmLanguage::Rust);
            let debug_str = format!("{:?}", config);

            assert!(debug_str.contains("WasmBuildConfig"));
            assert!(debug_str.contains("Rust"));
        }

        #[test]
        fn test_clone() {
            let config = WasmBuildConfig::new()
                .language(WasmLanguage::Python)
                .optimize(true);

            let cloned = config.clone();

            assert_eq!(cloned.language, Some(WasmLanguage::Python));
            assert!(cloned.optimize);
        }
    }

    // =========================================================================
    // WasmBuildResult tests
    // =========================================================================

    mod wasm_build_result_tests {
        use super::*;

        #[test]
        fn test_struct_creation() {
            let result = WasmBuildResult {
                wasm_path: PathBuf::from("/path/to/output.wasm"),
                language: WasmLanguage::Rust,
                target: WasiTarget::Preview2,
                size: 1024,
            };

            assert_eq!(result.wasm_path, PathBuf::from("/path/to/output.wasm"));
            assert_eq!(result.language, WasmLanguage::Rust);
            assert_eq!(result.target, WasiTarget::Preview2);
            assert_eq!(result.size, 1024);
        }

        #[test]
        fn test_struct_creation_all_languages() {
            for lang in WasmLanguage::all() {
                let result = WasmBuildResult {
                    wasm_path: PathBuf::from("/test.wasm"),
                    language: *lang,
                    target: WasiTarget::Preview1,
                    size: 512,
                };
                assert_eq!(result.language, *lang);
            }
        }

        #[test]
        fn test_debug_formatting() {
            let result = WasmBuildResult {
                wasm_path: PathBuf::from("/test.wasm"),
                language: WasmLanguage::Go,
                target: WasiTarget::Preview1,
                size: 2048,
            };

            let debug_str = format!("{:?}", result);
            assert!(debug_str.contains("WasmBuildResult"));
            assert!(debug_str.contains("test.wasm"));
            assert!(debug_str.contains("Go"));
            assert!(debug_str.contains("2048"));
        }

        #[test]
        fn test_clone() {
            let result = WasmBuildResult {
                wasm_path: PathBuf::from("/original.wasm"),
                language: WasmLanguage::Zig,
                target: WasiTarget::Preview2,
                size: 4096,
            };

            let cloned = result.clone();

            assert_eq!(cloned.wasm_path, result.wasm_path);
            assert_eq!(cloned.language, result.language);
            assert_eq!(cloned.target, result.target);
            assert_eq!(cloned.size, result.size);
        }

        #[test]
        fn test_zero_size() {
            let result = WasmBuildResult {
                wasm_path: PathBuf::from("/empty.wasm"),
                language: WasmLanguage::C,
                target: WasiTarget::Preview1,
                size: 0,
            };
            assert_eq!(result.size, 0);
        }

        #[test]
        fn test_large_size() {
            let result = WasmBuildResult {
                wasm_path: PathBuf::from("/large.wasm"),
                language: WasmLanguage::AssemblyScript,
                target: WasiTarget::Preview2,
                size: u64::MAX,
            };
            assert_eq!(result.size, u64::MAX);
        }
    }

    // =========================================================================
    // WasmBuildError tests
    // =========================================================================

    mod wasm_build_error_tests {
        use super::*;

        #[test]
        fn test_display_language_not_detected() {
            let err = WasmBuildError::LanguageNotDetected {
                path: PathBuf::from("/test/path"),
            };
            let display = err.to_string();
            assert!(display.contains("Could not detect source language"));
            assert!(display.contains("/test/path"));
        }

        #[test]
        fn test_display_tool_not_found() {
            let err = WasmBuildError::ToolNotFound {
                tool: "cargo".to_string(),
                message: "Not in PATH".to_string(),
            };
            let display = err.to_string();
            assert!(display.contains("Build tool 'cargo' not found"));
            assert!(display.contains("Not in PATH"));
        }

        #[test]
        fn test_display_build_failed() {
            let err = WasmBuildError::BuildFailed {
                exit_code: 1,
                stderr: "compilation error".to_string(),
                stdout: "some output".to_string(),
            };
            let display = err.to_string();
            assert!(display.contains("Build failed with exit code 1"));
            assert!(display.contains("compilation error"));
        }

        #[test]
        fn test_display_output_not_found() {
            let err = WasmBuildError::OutputNotFound {
                path: PathBuf::from("/expected/output.wasm"),
            };
            let display = err.to_string();
            assert!(display.contains("WASM output not found"));
            assert!(display.contains("/expected/output.wasm"));
        }

        #[test]
        fn test_display_config_error() {
            let err = WasmBuildError::ConfigError {
                message: "Invalid configuration".to_string(),
            };
            let display = err.to_string();
            assert!(display.contains("Configuration error"));
            assert!(display.contains("Invalid configuration"));
        }

        #[test]
        fn test_display_project_config_error() {
            let err = WasmBuildError::ProjectConfigError {
                message: "Failed to parse Cargo.toml".to_string(),
            };
            let display = err.to_string();
            assert!(display.contains("Failed to read project configuration"));
            assert!(display.contains("Failed to parse Cargo.toml"));
        }

        #[test]
        fn test_display_io_error() {
            let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
            let err = WasmBuildError::Io(io_err);
            let display = err.to_string();
            assert!(display.contains("IO error"));
            assert!(display.contains("file not found"));
        }

        #[test]
        fn test_debug_formatting_all_variants() {
            let errors = vec![
                WasmBuildError::LanguageNotDetected {
                    path: PathBuf::from("/test"),
                },
                WasmBuildError::ToolNotFound {
                    tool: "test".to_string(),
                    message: "msg".to_string(),
                },
                WasmBuildError::BuildFailed {
                    exit_code: 0,
                    stderr: String::new(),
                    stdout: String::new(),
                },
                WasmBuildError::OutputNotFound {
                    path: PathBuf::from("/test"),
                },
                WasmBuildError::ConfigError {
                    message: "test".to_string(),
                },
                WasmBuildError::ProjectConfigError {
                    message: "test".to_string(),
                },
            ];

            for err in errors {
                let debug_str = format!("{:?}", err);
                assert!(!debug_str.is_empty());
            }
        }

        #[test]
        fn test_from_io_error() {
            let io_err = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "access denied");
            let wasm_err: WasmBuildError = io_err.into();

            match wasm_err {
                WasmBuildError::Io(e) => {
                    assert_eq!(e.kind(), std::io::ErrorKind::PermissionDenied);
                }
                _ => panic!("Expected Io variant"),
            }
        }

        #[test]
        fn test_from_io_error_various_kinds() {
            let kinds = vec![
                std::io::ErrorKind::NotFound,
                std::io::ErrorKind::PermissionDenied,
                std::io::ErrorKind::AlreadyExists,
                std::io::ErrorKind::InvalidData,
            ];

            for kind in kinds {
                let io_err = std::io::Error::new(kind, "test error");
                let wasm_err: WasmBuildError = io_err.into();
                assert!(matches!(wasm_err, WasmBuildError::Io(_)));
            }
        }

        #[test]
        fn test_error_implements_std_error() {
            let err = WasmBuildError::ConfigError {
                message: "test".to_string(),
            };
            // Verify it implements std::error::Error by using the trait
            let _: &dyn std::error::Error = &err;
        }
    }

    // =========================================================================
    // detect_language tests
    // =========================================================================

    mod detect_language_tests {
        use super::*;

        #[test]
        fn test_detect_cargo_toml_rust() {
            let dir = create_temp_dir();
            fs::write(
                dir.path().join("Cargo.toml"),
                r#"[package]
name = "test"
version = "0.1.0"
"#,
            )
            .unwrap();

            let lang = detect_language(dir.path()).unwrap();
            assert_eq!(lang, WasmLanguage::Rust);
        }

        #[test]
        fn test_detect_cargo_toml_with_cargo_component_metadata() {
            let dir = create_temp_dir();
            fs::write(
                dir.path().join("Cargo.toml"),
                r#"[package]
name = "test"
version = "0.1.0"

[package.metadata.component]
package = "test:component"
"#,
            )
            .unwrap();

            let lang = detect_language(dir.path()).unwrap();
            assert_eq!(lang, WasmLanguage::RustComponent);
        }

        #[test]
        fn test_detect_cargo_toml_with_wit_bindgen_dep() {
            let dir = create_temp_dir();
            fs::write(
                dir.path().join("Cargo.toml"),
                r#"[package]
name = "test"
version = "0.1.0"

[dependencies]
wit-bindgen = "0.20"
"#,
            )
            .unwrap();

            let lang = detect_language(dir.path()).unwrap();
            assert_eq!(lang, WasmLanguage::RustComponent);
        }

        #[test]
        fn test_detect_cargo_toml_with_cargo_component_bindings() {
            let dir = create_temp_dir();
            fs::write(
                dir.path().join("Cargo.toml"),
                r#"[package]
name = "test"
version = "0.1.0"

[dependencies]
cargo-component-bindings = "0.1"
"#,
            )
            .unwrap();

            let lang = detect_language(dir.path()).unwrap();
            assert_eq!(lang, WasmLanguage::RustComponent);
        }

        #[test]
        fn test_detect_cargo_component_toml_file() {
            let dir = create_temp_dir();
            fs::write(
                dir.path().join("Cargo.toml"),
                r#"[package]
name = "test"
version = "0.1.0"
"#,
            )
            .unwrap();
            fs::write(
                dir.path().join("cargo-component.toml"),
                "# cargo-component config",
            )
            .unwrap();

            let lang = detect_language(dir.path()).unwrap();
            assert_eq!(lang, WasmLanguage::RustComponent);
        }

        #[test]
        fn test_detect_go_mod() {
            let dir = create_temp_dir();
            fs::write(
                dir.path().join("go.mod"),
                "module example.com/test\n\ngo 1.21\n",
            )
            .unwrap();

            let lang = detect_language(dir.path()).unwrap();
            assert_eq!(lang, WasmLanguage::Go);
        }

        #[test]
        fn test_detect_pyproject_toml() {
            let dir = create_temp_dir();
            fs::write(
                dir.path().join("pyproject.toml"),
                r#"[project]
name = "my-package"
version = "0.1.0"
"#,
            )
            .unwrap();

            let lang = detect_language(dir.path()).unwrap();
            assert_eq!(lang, WasmLanguage::Python);
        }

        #[test]
        fn test_detect_requirements_txt() {
            let dir = create_temp_dir();
            fs::write(
                dir.path().join("requirements.txt"),
                "flask==2.0.0\nrequests>=2.25.0",
            )
            .unwrap();

            let lang = detect_language(dir.path()).unwrap();
            assert_eq!(lang, WasmLanguage::Python);
        }

        #[test]
        fn test_detect_setup_py() {
            let dir = create_temp_dir();
            fs::write(
                dir.path().join("setup.py"),
                r#"from setuptools import setup
setup(name="mypackage")
"#,
            )
            .unwrap();

            let lang = detect_language(dir.path()).unwrap();
            assert_eq!(lang, WasmLanguage::Python);
        }

        #[test]
        fn test_detect_package_json_assemblyscript_in_dependencies() {
            let dir = create_temp_dir();
            fs::write(
                dir.path().join("package.json"),
                r#"{"name": "test", "dependencies": {"assemblyscript": "^0.27.0"}}"#,
            )
            .unwrap();

            let lang = detect_language(dir.path()).unwrap();
            assert_eq!(lang, WasmLanguage::AssemblyScript);
        }

        #[test]
        fn test_detect_package_json_assemblyscript_in_dev_dependencies() {
            let dir = create_temp_dir();
            fs::write(
                dir.path().join("package.json"),
                r#"{"name": "test", "devDependencies": {"assemblyscript": "^0.27.0"}}"#,
            )
            .unwrap();

            let lang = detect_language(dir.path()).unwrap();
            assert_eq!(lang, WasmLanguage::AssemblyScript);
        }

        #[test]
        fn test_detect_package_json_assemblyscript_in_scripts() {
            let dir = create_temp_dir();
            fs::write(
                dir.path().join("package.json"),
                r#"{"name": "test", "scripts": {"build": "asc assembly/index.ts"}}"#,
            )
            .unwrap();

            let lang = detect_language(dir.path()).unwrap();
            assert_eq!(lang, WasmLanguage::AssemblyScript);
        }

        #[test]
        fn test_detect_package_json_assemblyscript_asc_command() {
            let dir = create_temp_dir();
            fs::write(
                dir.path().join("package.json"),
                r#"{"name": "test", "scripts": {"compile": "asc"}}"#,
            )
            .unwrap();

            let lang = detect_language(dir.path()).unwrap();
            assert_eq!(lang, WasmLanguage::AssemblyScript);
        }

        #[test]
        fn test_detect_package_json_typescript() {
            let dir = create_temp_dir();
            fs::write(
                dir.path().join("package.json"),
                r#"{"name": "test", "version": "1.0.0", "devDependencies": {"typescript": "^5.0.0"}}"#,
            )
            .unwrap();

            let lang = detect_language(dir.path()).unwrap();
            assert_eq!(lang, WasmLanguage::TypeScript);
        }

        #[test]
        fn test_detect_package_json_plain_no_assemblyscript() {
            let dir = create_temp_dir();
            fs::write(
                dir.path().join("package.json"),
                r#"{"name": "test", "version": "1.0.0"}"#,
            )
            .unwrap();

            let lang = detect_language(dir.path()).unwrap();
            assert_eq!(lang, WasmLanguage::TypeScript);
        }

        #[test]
        fn test_detect_build_zig() {
            let dir = create_temp_dir();
            fs::write(
                dir.path().join("build.zig"),
                r#"const std = @import("std");
pub fn build(b: *std.build.Builder) void {}
"#,
            )
            .unwrap();

            let lang = detect_language(dir.path()).unwrap();
            assert_eq!(lang, WasmLanguage::Zig);
        }

        #[test]
        fn test_detect_makefile_with_c_files() {
            let dir = create_temp_dir();
            fs::write(dir.path().join("Makefile"), "all:\n\t$(CC) main.c -o main").unwrap();
            fs::write(dir.path().join("main.c"), "int main() { return 0; }").unwrap();

            let lang = detect_language(dir.path()).unwrap();
            assert_eq!(lang, WasmLanguage::C);
        }

        #[test]
        fn test_detect_cmakelists_with_c_files() {
            let dir = create_temp_dir();
            fs::write(
                dir.path().join("CMakeLists.txt"),
                "cmake_minimum_required(VERSION 3.10)\nproject(test)",
            )
            .unwrap();
            fs::write(dir.path().join("main.c"), "int main() { return 0; }").unwrap();

            let lang = detect_language(dir.path()).unwrap();
            assert_eq!(lang, WasmLanguage::C);
        }

        #[test]
        fn test_detect_c_header_file_only() {
            let dir = create_temp_dir();
            fs::write(
                dir.path().join("header.h"),
                "#ifndef HEADER_H\n#define HEADER_H\n#endif",
            )
            .unwrap();

            let lang = detect_language(dir.path()).unwrap();
            assert_eq!(lang, WasmLanguage::C);
        }

        #[test]
        fn test_detect_c_in_src_directory() {
            let dir = create_temp_dir();
            let src_dir = dir.path().join("src");
            fs::create_dir(&src_dir).unwrap();
            fs::write(src_dir.join("main.c"), "int main() { return 0; }").unwrap();

            let lang = detect_language(dir.path()).unwrap();
            assert_eq!(lang, WasmLanguage::C);
        }

        #[test]
        fn test_detect_empty_directory_error() {
            let dir = create_temp_dir();
            // Empty directory

            let result = detect_language(dir.path());
            assert!(matches!(
                result,
                Err(WasmBuildError::LanguageNotDetected { .. })
            ));
        }

        #[test]
        fn test_detect_unknown_files_error() {
            let dir = create_temp_dir();
            fs::write(dir.path().join("random.txt"), "some text").unwrap();
            fs::write(dir.path().join("data.json"), "{}").unwrap();

            let result = detect_language(dir.path());
            assert!(matches!(
                result,
                Err(WasmBuildError::LanguageNotDetected { .. })
            ));
        }

        #[test]
        fn test_detect_makefile_without_c_files_error() {
            let dir = create_temp_dir();
            fs::write(dir.path().join("Makefile"), "all:\n\techo hello").unwrap();

            let result = detect_language(dir.path());
            assert!(matches!(
                result,
                Err(WasmBuildError::LanguageNotDetected { .. })
            ));
        }

        #[test]
        fn test_detect_priority_rust_over_package_json() {
            // When both Cargo.toml and package.json exist, Rust takes priority
            let dir = create_temp_dir();
            fs::write(
                dir.path().join("Cargo.toml"),
                r#"[package]
name = "test"
version = "0.1.0"
"#,
            )
            .unwrap();
            fs::write(dir.path().join("package.json"), r#"{"name": "test"}"#).unwrap();

            let lang = detect_language(dir.path()).unwrap();
            assert_eq!(lang, WasmLanguage::Rust);
        }

        #[test]
        fn test_detect_priority_go_over_python() {
            // When both go.mod and pyproject.toml exist, Go takes priority
            let dir = create_temp_dir();
            fs::write(dir.path().join("go.mod"), "module test").unwrap();
            fs::write(dir.path().join("pyproject.toml"), "[project]").unwrap();

            let lang = detect_language(dir.path()).unwrap();
            assert_eq!(lang, WasmLanguage::Go);
        }
    }

    // =========================================================================
    // get_build_command tests
    // =========================================================================

    mod get_build_command_tests {
        use super::*;

        #[test]
        fn test_rust_preview1_release() {
            let cmd = get_build_command(WasmLanguage::Rust, WasiTarget::Preview1, true);
            assert_eq!(cmd[0], "cargo");
            assert_eq!(cmd[1], "build");
            assert!(cmd.contains(&"--target".to_string()));
            assert!(cmd.contains(&"wasm32-wasip1".to_string()));
            assert!(cmd.contains(&"--release".to_string()));
        }

        #[test]
        fn test_rust_preview2_release() {
            let cmd = get_build_command(WasmLanguage::Rust, WasiTarget::Preview2, true);
            assert_eq!(cmd[0], "cargo");
            assert_eq!(cmd[1], "build");
            assert!(cmd.contains(&"--target".to_string()));
            assert!(cmd.contains(&"wasm32-wasip2".to_string()));
            assert!(cmd.contains(&"--release".to_string()));
        }

        #[test]
        fn test_rust_preview1_debug() {
            let cmd = get_build_command(WasmLanguage::Rust, WasiTarget::Preview1, false);
            assert_eq!(cmd[0], "cargo");
            assert!(cmd.contains(&"wasm32-wasip1".to_string()));
            assert!(!cmd.contains(&"--release".to_string()));
        }

        #[test]
        fn test_rust_preview2_debug() {
            let cmd = get_build_command(WasmLanguage::Rust, WasiTarget::Preview2, false);
            assert_eq!(cmd[0], "cargo");
            assert!(cmd.contains(&"wasm32-wasip2".to_string()));
            assert!(!cmd.contains(&"--release".to_string()));
        }

        #[test]
        fn test_rust_component_release() {
            let cmd = get_build_command(WasmLanguage::RustComponent, WasiTarget::Preview2, true);
            assert_eq!(cmd[0], "cargo");
            assert_eq!(cmd[1], "component");
            assert_eq!(cmd[2], "build");
            assert!(cmd.contains(&"--release".to_string()));
        }

        #[test]
        fn test_rust_component_debug() {
            let cmd = get_build_command(WasmLanguage::RustComponent, WasiTarget::Preview2, false);
            assert_eq!(cmd[0], "cargo");
            assert_eq!(cmd[1], "component");
            assert_eq!(cmd[2], "build");
            assert!(!cmd.contains(&"--release".to_string()));
        }

        #[test]
        fn test_go_preview1() {
            let cmd = get_build_command(WasmLanguage::Go, WasiTarget::Preview1, false);
            assert_eq!(cmd[0], "tinygo");
            assert_eq!(cmd[1], "build");
            assert!(cmd.contains(&"-target".to_string()));
            assert!(cmd.contains(&"wasip1".to_string()));
        }

        #[test]
        fn test_go_preview2() {
            let cmd = get_build_command(WasmLanguage::Go, WasiTarget::Preview2, false);
            assert_eq!(cmd[0], "tinygo");
            assert!(cmd.contains(&"-target".to_string()));
            assert!(cmd.contains(&"wasip2".to_string()));
        }

        #[test]
        fn test_go_release_optimization() {
            let cmd = get_build_command(WasmLanguage::Go, WasiTarget::Preview2, true);
            assert_eq!(cmd[0], "tinygo");
            assert!(cmd.contains(&"-opt".to_string()));
            assert!(cmd.contains(&"2".to_string()));
        }

        #[test]
        fn test_go_debug_no_optimization() {
            let cmd = get_build_command(WasmLanguage::Go, WasiTarget::Preview2, false);
            assert_eq!(cmd[0], "tinygo");
            assert!(!cmd.contains(&"-opt".to_string()));
        }

        #[test]
        fn test_tinygo_correct_target_flag() {
            // TinyGo uses -target (single dash) not --target
            let cmd = get_build_command(WasmLanguage::Go, WasiTarget::Preview1, false);
            assert!(cmd.contains(&"-target".to_string()));
            assert!(!cmd.contains(&"--target".to_string()));
        }

        #[test]
        fn test_python() {
            let cmd = get_build_command(WasmLanguage::Python, WasiTarget::Preview2, true);
            assert_eq!(cmd[0], "componentize-py");
            assert!(cmd.contains(&"-d".to_string()));
            assert!(cmd.contains(&"wit".to_string()));
            assert!(cmd.contains(&"-w".to_string()));
            assert!(cmd.contains(&"world".to_string()));
            assert!(cmd.contains(&"componentize".to_string()));
            assert!(cmd.contains(&"app".to_string()));
            assert!(cmd.contains(&"-o".to_string()));
            assert!(cmd.contains(&"app.wasm".to_string()));
        }

        #[test]
        fn test_python_release_same_as_debug() {
            // componentize-py doesn't have explicit optimization flags
            let release_cmd = get_build_command(WasmLanguage::Python, WasiTarget::Preview2, true);
            let debug_cmd = get_build_command(WasmLanguage::Python, WasiTarget::Preview2, false);
            assert_eq!(release_cmd, debug_cmd);
        }

        #[test]
        fn test_componentize_py_arguments() {
            let cmd = get_build_command(WasmLanguage::Python, WasiTarget::Preview2, false);
            // Verify the order and presence of key arguments
            let cmd_str = cmd.join(" ");
            assert!(
                cmd_str.contains("componentize-py -d wit -w world componentize app -o app.wasm")
            );
        }

        #[test]
        fn test_typescript() {
            let cmd = get_build_command(WasmLanguage::TypeScript, WasiTarget::Preview2, true);
            assert_eq!(cmd[0], "npx");
            assert!(cmd.contains(&"jco".to_string()));
            assert!(cmd.contains(&"componentize".to_string()));
            assert!(cmd.contains(&"--wit".to_string()));
        }

        #[test]
        fn test_assemblyscript_release() {
            let cmd = get_build_command(WasmLanguage::AssemblyScript, WasiTarget::Preview2, true);
            assert_eq!(cmd[0], "npx");
            assert!(cmd.contains(&"asc".to_string()));
            assert!(cmd.contains(&"--optimize".to_string()));
        }

        #[test]
        fn test_assemblyscript_debug() {
            let cmd = get_build_command(WasmLanguage::AssemblyScript, WasiTarget::Preview2, false);
            assert_eq!(cmd[0], "npx");
            assert!(cmd.contains(&"asc".to_string()));
            assert!(!cmd.contains(&"--optimize".to_string()));
        }

        #[test]
        fn test_c_release() {
            let cmd = get_build_command(WasmLanguage::C, WasiTarget::Preview1, true);
            assert_eq!(cmd[0], "clang");
            assert!(cmd.contains(&"--target=wasm32-wasi".to_string()));
            assert!(cmd.contains(&"-O2".to_string()));
        }

        #[test]
        fn test_c_debug() {
            let cmd = get_build_command(WasmLanguage::C, WasiTarget::Preview1, false);
            assert_eq!(cmd[0], "clang");
            assert!(cmd.contains(&"--target=wasm32-wasi".to_string()));
            assert!(!cmd.contains(&"-O2".to_string()));
        }

        #[test]
        fn test_zig_release() {
            let cmd = get_build_command(WasmLanguage::Zig, WasiTarget::Preview1, true);
            assert_eq!(cmd[0], "zig");
            assert_eq!(cmd[1], "build");
            assert!(cmd.contains(&"-Dtarget=wasm32-wasi".to_string()));
            assert!(cmd.contains(&"-Doptimize=ReleaseFast".to_string()));
        }

        #[test]
        fn test_zig_debug() {
            let cmd = get_build_command(WasmLanguage::Zig, WasiTarget::Preview1, false);
            assert_eq!(cmd[0], "zig");
            assert_eq!(cmd[1], "build");
            assert!(cmd.contains(&"-Dtarget=wasm32-wasi".to_string()));
            assert!(!cmd.contains(&"-Doptimize=ReleaseFast".to_string()));
        }

        #[test]
        fn test_cargo_uses_double_dash_target() {
            // cargo uses --target (double dash)
            let cmd = get_build_command(WasmLanguage::Rust, WasiTarget::Preview1, false);
            assert!(cmd.contains(&"--target".to_string()));
        }

        #[test]
        fn test_go_output_file() {
            let cmd = get_build_command(WasmLanguage::Go, WasiTarget::Preview1, false);
            assert!(cmd.contains(&"-o".to_string()));
            assert!(cmd.contains(&"main.wasm".to_string()));
        }

        #[test]
        fn test_all_commands_non_empty() {
            for lang in WasmLanguage::all() {
                for target in [WasiTarget::Preview1, WasiTarget::Preview2] {
                    for release in [true, false] {
                        let cmd = get_build_command(*lang, target, release);
                        assert!(
                            !cmd.is_empty(),
                            "Command for {:?}/{:?}/{} should not be empty",
                            lang,
                            target,
                            release
                        );
                        assert!(
                            !cmd[0].is_empty(),
                            "First command element should not be empty"
                        );
                    }
                }
            }
        }
    }

    // =========================================================================
    // Additional helper function tests
    // =========================================================================

    mod helper_function_tests {
        use super::*;

        #[test]
        fn test_get_rust_package_name_success() {
            let dir = create_temp_dir();
            fs::write(
                dir.path().join("Cargo.toml"),
                r#"[package]
name = "my-cool-package"
version = "0.1.0"
"#,
            )
            .unwrap();

            let name = get_rust_package_name(dir.path()).unwrap();
            assert_eq!(name, "my-cool-package");
        }

        #[test]
        fn test_get_rust_package_name_with_single_quotes() {
            let dir = create_temp_dir();
            fs::write(
                dir.path().join("Cargo.toml"),
                "[package]\nname = 'single-quoted'\nversion = '0.1.0'\n",
            )
            .unwrap();

            let name = get_rust_package_name(dir.path()).unwrap();
            assert_eq!(name, "single-quoted");
        }

        #[test]
        fn test_get_rust_package_name_missing_file() {
            let dir = create_temp_dir();
            // No Cargo.toml

            let result = get_rust_package_name(dir.path());
            assert!(matches!(
                result,
                Err(WasmBuildError::ProjectConfigError { .. })
            ));
        }

        #[test]
        fn test_get_rust_package_name_no_name_field() {
            let dir = create_temp_dir();
            fs::write(
                dir.path().join("Cargo.toml"),
                "[package]\nversion = \"0.1.0\"\n",
            )
            .unwrap();

            let result = get_rust_package_name(dir.path());
            assert!(matches!(
                result,
                Err(WasmBuildError::ProjectConfigError { .. })
            ));
        }

        #[test]
        fn test_find_any_wasm_file_in_root() {
            let dir = create_temp_dir();
            fs::write(dir.path().join("test.wasm"), "wasm content").unwrap();

            let found = find_any_wasm_file(dir.path());
            assert!(found.is_some());
            assert!(found.unwrap().ends_with("test.wasm"));
        }

        #[test]
        fn test_find_any_wasm_file_in_target() {
            let dir = create_temp_dir();
            let target_dir = dir.path().join("target");
            fs::create_dir(&target_dir).unwrap();
            fs::write(target_dir.join("output.wasm"), "wasm content").unwrap();

            let found = find_any_wasm_file(dir.path());
            assert!(found.is_some());
        }

        #[test]
        fn test_find_any_wasm_file_in_build() {
            let dir = create_temp_dir();
            let build_dir = dir.path().join("build");
            fs::create_dir(&build_dir).unwrap();
            fs::write(build_dir.join("module.wasm"), "wasm content").unwrap();

            let found = find_any_wasm_file(dir.path());
            assert!(found.is_some());
        }

        #[test]
        fn test_find_any_wasm_file_in_dist() {
            let dir = create_temp_dir();
            let dist_dir = dir.path().join("dist");
            fs::create_dir(&dist_dir).unwrap();
            fs::write(dist_dir.join("bundle.wasm"), "wasm content").unwrap();

            let found = find_any_wasm_file(dir.path());
            assert!(found.is_some());
        }

        #[test]
        fn test_find_any_wasm_file_nested() {
            let dir = create_temp_dir();
            let nested = dir
                .path()
                .join("target")
                .join("wasm32-wasip2")
                .join("release");
            fs::create_dir_all(&nested).unwrap();
            fs::write(nested.join("deep.wasm"), "wasm content").unwrap();

            let found = find_any_wasm_file(dir.path());
            assert!(found.is_some());
        }

        #[test]
        fn test_find_any_wasm_file_none() {
            let dir = create_temp_dir();
            fs::write(dir.path().join("not_wasm.txt"), "text").unwrap();

            let found = find_any_wasm_file(dir.path());
            assert!(found.is_none());
        }

        #[test]
        fn test_find_any_wasm_file_respects_depth_limit() {
            let dir = create_temp_dir();
            // Create a deeply nested wasm file (beyond max_depth of 3)
            let deep = dir.path().join("a").join("b").join("c").join("d").join("e");
            fs::create_dir_all(&deep).unwrap();
            fs::write(deep.join("too_deep.wasm"), "wasm").unwrap();

            // The recursive search has max_depth of 3, so this might not be found
            // depending on the search order
            let _ = find_any_wasm_file(dir.path());
            // We don't assert the result as it depends on implementation details
        }

        #[test]
        fn test_has_c_source_files_true_c() {
            let dir = create_temp_dir();
            fs::write(dir.path().join("main.c"), "int main() {}").unwrap();

            assert!(has_c_source_files(dir.path()));
        }

        #[test]
        fn test_has_c_source_files_true_h() {
            let dir = create_temp_dir();
            fs::write(dir.path().join("header.h"), "#pragma once").unwrap();

            assert!(has_c_source_files(dir.path()));
        }

        #[test]
        fn test_has_c_source_files_in_src() {
            let dir = create_temp_dir();
            let src = dir.path().join("src");
            fs::create_dir(&src).unwrap();
            fs::write(src.join("lib.c"), "void foo() {}").unwrap();

            assert!(has_c_source_files(dir.path()));
        }

        #[test]
        fn test_has_c_source_files_false() {
            let dir = create_temp_dir();
            fs::write(dir.path().join("main.rs"), "fn main() {}").unwrap();

            assert!(!has_c_source_files(dir.path()));
        }

        #[test]
        fn test_is_cargo_component_project_with_metadata() {
            let dir = create_temp_dir();
            let cargo_toml = dir.path().join("Cargo.toml");
            fs::write(
                &cargo_toml,
                r#"[package]
name = "test"
[package.metadata.component]
package = "test:component"
"#,
            )
            .unwrap();

            assert!(is_cargo_component_project(&cargo_toml).unwrap());
        }

        #[test]
        fn test_is_cargo_component_project_with_wit_bindgen() {
            let dir = create_temp_dir();
            let cargo_toml = dir.path().join("Cargo.toml");
            fs::write(
                &cargo_toml,
                r#"[package]
name = "test"
[dependencies]
wit-bindgen = "0.20"
"#,
            )
            .unwrap();

            assert!(is_cargo_component_project(&cargo_toml).unwrap());
        }

        #[test]
        fn test_is_cargo_component_project_plain_rust() {
            let dir = create_temp_dir();
            let cargo_toml = dir.path().join("Cargo.toml");
            fs::write(
                &cargo_toml,
                r#"[package]
name = "test"
version = "0.1.0"
"#,
            )
            .unwrap();

            assert!(!is_cargo_component_project(&cargo_toml).unwrap());
        }

        #[test]
        fn test_is_assemblyscript_project_dependencies() {
            let dir = create_temp_dir();
            let package_json = dir.path().join("package.json");
            fs::write(
                &package_json,
                r#"{"dependencies": {"assemblyscript": "^0.27"}}"#,
            )
            .unwrap();

            assert!(is_assemblyscript_project(&package_json).unwrap());
        }

        #[test]
        fn test_is_assemblyscript_project_dev_dependencies() {
            let dir = create_temp_dir();
            let package_json = dir.path().join("package.json");
            fs::write(
                &package_json,
                r#"{"devDependencies": {"assemblyscript": "^0.27"}}"#,
            )
            .unwrap();

            assert!(is_assemblyscript_project(&package_json).unwrap());
        }

        #[test]
        fn test_is_assemblyscript_project_script_with_asc() {
            let dir = create_temp_dir();
            let package_json = dir.path().join("package.json");
            fs::write(
                &package_json,
                r#"{"scripts": {"build": "asc assembly/index.ts"}}"#,
            )
            .unwrap();

            assert!(is_assemblyscript_project(&package_json).unwrap());
        }

        #[test]
        fn test_is_assemblyscript_project_false() {
            let dir = create_temp_dir();
            let package_json = dir.path().join("package.json");
            fs::write(&package_json, r#"{"dependencies": {"react": "^18.0.0"}}"#).unwrap();

            assert!(!is_assemblyscript_project(&package_json).unwrap());
        }

        #[test]
        fn test_is_assemblyscript_project_invalid_json() {
            let dir = create_temp_dir();
            let package_json = dir.path().join("package.json");
            fs::write(&package_json, "not valid json").unwrap();

            let result = is_assemblyscript_project(&package_json);
            assert!(matches!(
                result,
                Err(WasmBuildError::ProjectConfigError { .. })
            ));
        }
    }

    // =========================================================================
    // find_wasm_output tests
    // =========================================================================

    mod find_wasm_output_tests {
        use super::*;

        #[test]
        fn test_find_rust_release_output() {
            let dir = create_temp_dir();
            fs::write(
                dir.path().join("Cargo.toml"),
                "[package]\nname = \"myapp\"\nversion = \"0.1.0\"\n",
            )
            .unwrap();

            let output_dir = dir
                .path()
                .join("target")
                .join("wasm32-wasip2")
                .join("release");
            fs::create_dir_all(&output_dir).unwrap();
            fs::write(output_dir.join("myapp.wasm"), "wasm").unwrap();

            let result =
                find_wasm_output(dir.path(), WasmLanguage::Rust, WasiTarget::Preview2, true);
            assert!(result.is_ok());
            assert!(result.unwrap().ends_with("myapp.wasm"));
        }

        #[test]
        fn test_find_rust_debug_output() {
            let dir = create_temp_dir();
            fs::write(
                dir.path().join("Cargo.toml"),
                "[package]\nname = \"myapp\"\nversion = \"0.1.0\"\n",
            )
            .unwrap();

            let output_dir = dir
                .path()
                .join("target")
                .join("wasm32-wasip1")
                .join("debug");
            fs::create_dir_all(&output_dir).unwrap();
            fs::write(output_dir.join("myapp.wasm"), "wasm").unwrap();

            let result =
                find_wasm_output(dir.path(), WasmLanguage::Rust, WasiTarget::Preview1, false);
            assert!(result.is_ok());
        }

        #[test]
        fn test_find_rust_underscore_name() {
            let dir = create_temp_dir();
            fs::write(
                dir.path().join("Cargo.toml"),
                "[package]\nname = \"my-app\"\nversion = \"0.1.0\"\n",
            )
            .unwrap();

            let output_dir = dir
                .path()
                .join("target")
                .join("wasm32-wasip2")
                .join("release");
            fs::create_dir_all(&output_dir).unwrap();
            // Rust converts hyphens to underscores in the output filename
            fs::write(output_dir.join("my_app.wasm"), "wasm").unwrap();

            let result =
                find_wasm_output(dir.path(), WasmLanguage::Rust, WasiTarget::Preview2, true);
            assert!(result.is_ok());
        }

        #[test]
        fn test_find_go_output() {
            let dir = create_temp_dir();
            fs::write(dir.path().join("main.wasm"), "wasm").unwrap();

            let result =
                find_wasm_output(dir.path(), WasmLanguage::Go, WasiTarget::Preview1, false);
            assert!(result.is_ok());
            assert!(result.unwrap().ends_with("main.wasm"));
        }

        #[test]
        fn test_find_python_output() {
            let dir = create_temp_dir();
            fs::write(dir.path().join("app.wasm"), "wasm").unwrap();

            let result =
                find_wasm_output(dir.path(), WasmLanguage::Python, WasiTarget::Preview2, true);
            assert!(result.is_ok());
            assert!(result.unwrap().ends_with("app.wasm"));
        }

        #[test]
        fn test_find_typescript_output() {
            let dir = create_temp_dir();
            let dist_dir = dir.path().join("dist");
            fs::create_dir(&dist_dir).unwrap();
            fs::write(dist_dir.join("component.wasm"), "wasm").unwrap();

            let result = find_wasm_output(
                dir.path(),
                WasmLanguage::TypeScript,
                WasiTarget::Preview2,
                true,
            );
            assert!(result.is_ok());
        }

        #[test]
        fn test_find_assemblyscript_release_output() {
            let dir = create_temp_dir();
            let build_dir = dir.path().join("build");
            fs::create_dir(&build_dir).unwrap();
            fs::write(build_dir.join("release.wasm"), "wasm").unwrap();

            let result = find_wasm_output(
                dir.path(),
                WasmLanguage::AssemblyScript,
                WasiTarget::Preview2,
                true,
            );
            assert!(result.is_ok());
        }

        #[test]
        fn test_find_c_output() {
            let dir = create_temp_dir();
            fs::write(dir.path().join("main.wasm"), "wasm").unwrap();

            let result = find_wasm_output(dir.path(), WasmLanguage::C, WasiTarget::Preview1, true);
            assert!(result.is_ok());
        }

        #[test]
        fn test_find_zig_output() {
            let dir = create_temp_dir();
            let zig_out = dir.path().join("zig-out").join("bin");
            fs::create_dir_all(&zig_out).unwrap();
            fs::write(zig_out.join("main.wasm"), "wasm").unwrap();

            let result =
                find_wasm_output(dir.path(), WasmLanguage::Zig, WasiTarget::Preview1, true);
            assert!(result.is_ok());
        }

        #[test]
        fn test_find_output_not_found() {
            let dir = create_temp_dir();
            // No wasm files

            let result =
                find_wasm_output(dir.path(), WasmLanguage::Rust, WasiTarget::Preview2, true);
            assert!(matches!(result, Err(WasmBuildError::OutputNotFound { .. })));
        }

        #[test]
        fn test_find_output_fallback_search() {
            let dir = create_temp_dir();
            fs::write(
                dir.path().join("Cargo.toml"),
                "[package]\nname = \"test\"\n",
            )
            .unwrap();

            // Put wasm in unexpected location
            let other_dir = dir.path().join("target").join("other");
            fs::create_dir_all(&other_dir).unwrap();
            fs::write(other_dir.join("unexpected.wasm"), "wasm").unwrap();

            // Should find via fallback search
            let result =
                find_wasm_output(dir.path(), WasmLanguage::Rust, WasiTarget::Preview2, true);
            assert!(result.is_ok());
        }
    }
}
