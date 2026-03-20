//! macOS toolchain provisioning for sandbox builds.
//!
//! Instead of translating Linux package manager commands to `brew install`
//! (which mutates global state and breaks parallel builds), this module
//! provisions language toolchains directly into a build's rootfs.
//!
//! Each major language has a version listing API and self-contained macOS
//! binaries. The provisioner resolves a version → download URL, downloads the
//! tarball, and extracts it into the rootfs — all scoped to the build.
//!
//! This module is only compiled on macOS (`#[cfg(target_os = "macos")]`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use tracing::{debug, info};

use crate::error::{BuildError, Result};

// ---------------------------------------------------------------------------
// ToolchainSpec
// ---------------------------------------------------------------------------

/// Specification for a language toolchain to provision into a build rootfs.
#[derive(Debug, Clone)]
pub struct ToolchainSpec {
    /// Language identifier (e.g., "go", "node", "rust").
    pub language: String,
    /// Requested version (e.g., "1.23", "20", "latest").
    pub version: String,
    /// Directory inside rootfs where the toolchain is installed.
    pub install_dir: String,
    /// Directories to add to PATH (relative to rootfs root).
    pub path_dirs: Vec<String>,
    /// Extra environment variables to set.
    pub env: HashMap<String, String>,
}

impl ToolchainSpec {
    /// Create a Go toolchain spec.
    #[must_use]
    pub fn go(version: &str) -> Self {
        // GOROOT and GOPATH are set relative to rootfs — the sandbox builder
        // will prefix them with the rootfs path at execution time.
        // GOPATH uses a writable location under the build home dir.
        let mut env = HashMap::new();
        env.insert("GOROOT".to_string(), "/usr/local/go".to_string());
        // Disable VCS stamping — git in the sandbox may be killed by Seatbelt
        env.insert("GOFLAGS".to_string(), "-buildvcs=false".to_string());
        Self {
            language: "go".to_string(),
            version: version.to_string(),
            install_dir: "/usr/local/go".to_string(),
            path_dirs: vec!["/usr/local/go/bin".to_string()],
            env,
        }
    }

    /// Create a Node.js toolchain spec.
    #[must_use]
    pub fn node(version: &str) -> Self {
        Self {
            language: "node".to_string(),
            version: version.to_string(),
            install_dir: "/usr/local/node".to_string(),
            path_dirs: vec!["/usr/local/node/bin".to_string()],
            env: HashMap::new(),
        }
    }

    /// Create a Rust toolchain spec.
    #[must_use]
    pub fn rust(version: &str) -> Self {
        let mut env = HashMap::new();
        env.insert("CARGO_HOME".to_string(), "/usr/local/cargo".to_string());
        env.insert("RUSTUP_HOME".to_string(), "/usr/local/rustup".to_string());
        Self {
            language: "rust".to_string(),
            version: version.to_string(),
            install_dir: "/usr/local/rust".to_string(),
            path_dirs: vec![
                "/usr/local/cargo/bin".to_string(),
                "/usr/local/rust/bin".to_string(),
            ],
            env,
        }
    }

    /// Create a Python toolchain spec.
    #[must_use]
    pub fn python(version: &str) -> Self {
        Self {
            language: "python".to_string(),
            version: version.to_string(),
            install_dir: "/usr/local/python".to_string(),
            path_dirs: vec!["/usr/local/python/bin".to_string()],
            env: HashMap::new(),
        }
    }

    /// Create a Deno toolchain spec.
    #[must_use]
    pub fn deno(version: &str) -> Self {
        Self {
            language: "deno".to_string(),
            version: version.to_string(),
            install_dir: "/usr/local/deno".to_string(),
            path_dirs: vec!["/usr/local/deno/bin".to_string()],
            env: HashMap::new(),
        }
    }

    /// Create a Bun toolchain spec.
    #[must_use]
    pub fn bun(version: &str) -> Self {
        Self {
            language: "bun".to_string(),
            version: version.to_string(),
            install_dir: "/usr/local/bun".to_string(),
            path_dirs: vec!["/usr/local/bun/bin".to_string()],
            env: HashMap::new(),
        }
    }

    /// Create a Java (Adoptium) toolchain spec.
    #[must_use]
    pub fn java(version: &str) -> Self {
        let mut env = HashMap::new();
        env.insert("JAVA_HOME".to_string(), "/usr/local/java".to_string());
        Self {
            language: "java".to_string(),
            version: version.to_string(),
            install_dir: "/usr/local/java".to_string(),
            path_dirs: vec!["/usr/local/java/bin".to_string()],
            env,
        }
    }

    /// Create a Zig toolchain spec.
    #[must_use]
    pub fn zig(version: &str) -> Self {
        Self {
            language: "zig".to_string(),
            version: version.to_string(),
            install_dir: "/usr/local/zig".to_string(),
            path_dirs: vec!["/usr/local/zig".to_string()],
            env: HashMap::new(),
        }
    }

    /// Create a Swift toolchain spec.
    ///
    /// On macOS, Swift is provisioned from the host system's Xcode Command Line
    /// Tools rather than downloaded. The version field is informational — the
    /// host's installed Swift version is used regardless.
    #[must_use]
    pub fn swift(version: &str) -> Self {
        Self {
            language: "swift".to_string(),
            version: version.to_string(),
            install_dir: "/usr/local/swift".to_string(),
            path_dirs: vec!["/usr/local/swift/usr/bin".to_string()],
            env: HashMap::new(),
        }
    }

    /// Create a `GraalVM` CE toolchain spec.
    ///
    /// Sets both `JAVA_HOME` and `GRAALVM_HOME` to the install directory so
    /// that tools like `native-image` and `gu` are discoverable.
    #[must_use]
    pub fn graalvm(version: &str) -> Self {
        let mut env = HashMap::new();
        env.insert("JAVA_HOME".to_string(), "/usr/local/graalvm".to_string());
        env.insert("GRAALVM_HOME".to_string(), "/usr/local/graalvm".to_string());
        Self {
            language: "graalvm".to_string(),
            version: version.to_string(),
            install_dir: "/usr/local/graalvm".to_string(),
            path_dirs: vec!["/usr/local/graalvm/bin".to_string()],
            env,
        }
    }
}

// ---------------------------------------------------------------------------
// Toolchain detection from image reference
// ---------------------------------------------------------------------------

/// Detect the language toolchain from a Docker/OCI base image reference.
///
/// Parses image names like `golang:1.23-alpine` or `node:20-slim` to determine
/// the language and version.
#[must_use]
pub fn detect_toolchain(image_ref: &str) -> Option<ToolchainSpec> {
    let (name, tag) = split_image_name_tag(image_ref);

    // Strip registry prefix: "docker.io/library/golang" → "golang"
    let base_name = name.rsplit('/').next().unwrap_or(&name);

    let version = extract_version_from_tag(&tag);

    match base_name {
        "golang" | "go" => Some(ToolchainSpec::go(&version)),
        "node" => Some(ToolchainSpec::node(&version)),
        "rust" => Some(ToolchainSpec::rust(&version)),
        "python" | "python3" => Some(ToolchainSpec::python(&version)),
        "deno" => Some(ToolchainSpec::deno(&version)),
        "bun" => Some(ToolchainSpec::bun(&version)),
        "eclipse-temurin" | "amazoncorretto" | "openjdk" => Some(ToolchainSpec::java(&version)),
        "zig" => Some(ToolchainSpec::zig(&version)),
        "swift" => Some(ToolchainSpec::swift(&version)),
        name if name.contains("graalvm") => Some(ToolchainSpec::graalvm(&version)),
        // Base images and unknown — no toolchain needed
        _ => None,
    }
}

/// Split an image reference into name and tag.
fn split_image_name_tag(image_ref: &str) -> (String, String) {
    if let Some((name, tag)) = image_ref.rsplit_once(':') {
        (name.to_string(), tag.to_string())
    } else {
        (image_ref.to_string(), "latest".to_string())
    }
}

/// Extract a version number from a Docker image tag.
///
/// Examples:
/// - `"1.23-alpine"` → `"1.23"`
/// - `"20-slim"` → `"20"`
/// - `"3.12.1-bookworm"` → `"3.12.1"`
/// - `"latest"` → `"latest"`
/// - `"alpine"` → `"latest"` (no version component)
fn extract_version_from_tag(tag: &str) -> String {
    if tag == "latest" {
        return "latest".to_string();
    }

    // Try to extract a version-like prefix (digits and dots)
    let version_part: String = tag
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();

    if version_part.is_empty() {
        "latest".to_string()
    } else {
        // Trim trailing dot
        version_part.trim_end_matches('.').to_string()
    }
}

// ---------------------------------------------------------------------------
// Host architecture detection
// ---------------------------------------------------------------------------

/// Get the macOS host architecture as used in download URLs.
#[must_use]
pub fn host_arch() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "amd64"
    }
}

// ---------------------------------------------------------------------------
// Version resolution + download
// ---------------------------------------------------------------------------

/// Resolve a toolchain version to a download URL and provision it into rootfs.
///
/// The toolchain is downloaded and extracted into the cache directory
/// (`~/.zlayer/toolchains/{lang}-{version}-{arch}/`). The build rootfs receives
/// a symlink to the cached toolchain rather than a full copy.
///
/// # Errors
///
/// Returns an error if version resolution, download, or extraction fails.
pub async fn provision_toolchain(
    spec: &ToolchainSpec,
    rootfs_dir: &Path,
    cache_dir: &Path,
    tmp_dir: &Path,
) -> Result<()> {
    // Swift is provisioned from the host system, not downloaded.
    if spec.language == "swift" {
        return provision_swift_from_host(spec, rootfs_dir, cache_dir).await;
    }

    let arch = host_arch();
    let cache_key = format!("{}-{}-{}", spec.language, spec.version, arch);
    let cached_toolchain = cache_dir.join(&cache_key);

    // Check cache
    if cached_toolchain.exists() {
        info!(
            "Symlinking cached {} {} toolchain from {}",
            spec.language,
            spec.version,
            cached_toolchain.display()
        );
        copy_toolchain_to_rootfs(&cached_toolchain, rootfs_dir, spec).await?;
        return Ok(());
    }

    // Resolve version → download URL
    let (resolved_version, url) = resolve_download_url(spec, arch).await?;

    info!(
        "Downloading {} {} (resolved: {}) from {}",
        spec.language, spec.version, resolved_version, url
    );

    // Download to tmp
    let download_path = tmp_dir.join(format!("toolchain-{cache_key}.tar.gz"));
    download_file(&url, &download_path).await?;

    // Extract to cache
    tokio::fs::create_dir_all(&cached_toolchain).await?;
    extract_toolchain(&spec.language, &download_path, &cached_toolchain).await?;

    // Symlink cache into rootfs
    copy_toolchain_to_rootfs(&cached_toolchain, rootfs_dir, spec).await?;

    // Clean up download
    let _ = tokio::fs::remove_file(&download_path).await;

    info!(
        "Provisioned {} {} into rootfs",
        spec.language, resolved_version
    );
    Ok(())
}

/// Resolve a toolchain spec to a concrete download URL.
async fn resolve_download_url(spec: &ToolchainSpec, arch: &str) -> Result<(String, String)> {
    match spec.language.as_str() {
        "go" => resolve_go(&spec.version, arch).await,
        "node" => resolve_node(&spec.version, arch).await,
        "rust" => resolve_rust(&spec.version, arch).await,
        "python" => resolve_python(&spec.version, arch).await,
        "deno" => resolve_deno(&spec.version, arch).await,
        "bun" => resolve_bun(&spec.version, arch).await,
        "zig" => resolve_zig(&spec.version, arch).await,
        "java" => resolve_java(&spec.version, arch).await,
        "graalvm" => resolve_graalvm(&spec.version, arch).await,
        "swift" => Err(BuildError::RegistryError {
            message: "Swift is provisioned from the host system, not downloaded. \
                      This code path should not be reached."
                .to_string(),
        }),
        other => Err(BuildError::RegistryError {
            message: format!(
                "No toolchain provisioner for '{other}'. \
                 Supported: go, node, rust, python, deno, bun, zig, java, graalvm, swift. \
                 Use a pre-built zlayer/ base image or specify a toolchain URL."
            ),
        }),
    }
}

// ---------------------------------------------------------------------------
// Swift provisioner (host-based)
// ---------------------------------------------------------------------------

/// Provision Swift from the macOS host system's Xcode Command Line Tools.
///
/// Unlike other toolchain resolvers that download binaries, Swift on macOS is
/// a first-class citizen — it ships with Xcode and the Command Line Tools.
/// This function locates the host Swift toolchain via `xcrun`, copies it into
/// the cache directory, and symlinks it into the build rootfs.
///
/// The function:
/// 1. Runs `xcrun --find swiftc` to locate the Swift compiler
/// 2. Derives the toolchain root (the `usr/` parent of the `bin/` directory)
/// 3. Copies the toolchain into the cache keyed by the detected version
/// 4. Symlinks the cached toolchain into the rootfs at `/usr/local/swift`
async fn provision_swift_from_host(
    spec: &ToolchainSpec,
    rootfs_dir: &Path,
    cache_dir: &Path,
) -> Result<()> {
    // Locate the Swift compiler via xcrun
    let xcrun_output = tokio::process::Command::new("xcrun")
        .args(["--find", "swiftc"])
        .output()
        .await
        .map_err(|e| BuildError::RegistryError {
            message: format!(
                "Failed to run 'xcrun --find swiftc'. \
                 Ensure Xcode Command Line Tools are installed (xcode-select --install): {e}"
            ),
        })?;

    if !xcrun_output.status.success() {
        let stderr = String::from_utf8_lossy(&xcrun_output.stderr);
        return Err(BuildError::RegistryError {
            message: format!(
                "xcrun --find swiftc failed: {stderr}. \
                 Install Xcode Command Line Tools with: xcode-select --install"
            ),
        });
    }

    let swiftc_path = PathBuf::from(
        String::from_utf8_lossy(&xcrun_output.stdout)
            .trim()
            .to_string(),
    );
    debug!("Found swiftc at: {}", swiftc_path.display());

    // Derive the toolchain root.
    // swiftc is typically at:
    //   /Library/Developer/CommandLineTools/usr/bin/swiftc
    //   /Applications/Xcode.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/bin/swiftc
    // We want the directory containing `usr/bin/swiftc` — i.e., grandparent of
    // the binary gives us `usr/`, and its parent is the toolchain root.
    let toolchain_root = swiftc_path
        .parent() // .../usr/bin
        .and_then(|p| p.parent()) // .../usr
        .and_then(|p| p.parent()) // .../  (toolchain root)
        .ok_or_else(|| BuildError::RegistryError {
            message: format!(
                "Could not determine Swift toolchain root from swiftc path: {}",
                swiftc_path.display()
            ),
        })?;

    info!("Swift toolchain root: {}", toolchain_root.display());

    // Detect the installed Swift version for cache keying
    let version_output = tokio::process::Command::new(&swiftc_path)
        .arg("--version")
        .output()
        .await
        .map_err(|e| BuildError::RegistryError {
            message: format!("Failed to run 'swiftc --version': {e}"),
        })?;

    let version_str = String::from_utf8_lossy(&version_output.stdout);
    let detected_version = extract_swift_version(&version_str);
    info!("Detected host Swift version: {detected_version}");

    // Cache key uses the detected version to avoid re-copying on repeated builds
    let cache_key = format!("swift-{}-{}", detected_version, host_arch());
    let cached_toolchain = cache_dir.join(&cache_key);

    if cached_toolchain.exists() {
        info!(
            "Using cached Swift {} toolchain from {}",
            detected_version,
            cached_toolchain.display()
        );
    } else {
        info!(
            "Copying host Swift toolchain into cache at {}",
            cached_toolchain.display()
        );
        tokio::fs::create_dir_all(&cached_toolchain).await?;

        // Copy the toolchain's usr/ directory into the cache.
        // We use cp -R to preserve symlinks within the toolchain.
        let usr_src = toolchain_root.join("usr");
        let usr_dest = cached_toolchain.join("usr");

        let cp_output = tokio::process::Command::new("cp")
            .args(["-R"])
            .arg(format!("{}/", usr_src.display()))
            .arg(usr_dest.display().to_string())
            .output()
            .await
            .map_err(|e| BuildError::RegistryError {
                message: format!("Failed to copy Swift toolchain: {e}"),
            })?;

        if !cp_output.status.success() {
            let stderr = String::from_utf8_lossy(&cp_output.stderr);
            let _ = tokio::fs::remove_dir_all(&cached_toolchain).await;
            return Err(BuildError::RegistryError {
                message: format!("Failed to copy Swift toolchain: {stderr}"),
            });
        }

        // Also copy Swift libraries if they exist separately
        let lib_src = toolchain_root.join("lib");
        if lib_src.exists() {
            let lib_dest = cached_toolchain.join("lib");
            let _ = tokio::process::Command::new("cp")
                .args(["-R"])
                .arg(format!("{}/", lib_src.display()))
                .arg(lib_dest.display().to_string())
                .output()
                .await;
        }

        info!("Cached Swift toolchain at {}", cached_toolchain.display());
    }

    // Symlink cache into rootfs
    copy_toolchain_to_rootfs(&cached_toolchain, rootfs_dir, spec).await?;

    info!("Provisioned Swift {} (host) into rootfs", detected_version);
    Ok(())
}

/// Extract the Swift version number from `swiftc --version` output.
///
/// Typical output:
/// ```text
/// Apple Swift version 6.1 (swiftlang-6.1.0.110.21 clang-1700.0.13.3)
/// Target: arm64-apple-macosx15.0
/// ```
///
/// Returns the version string (e.g., `"6.1"`) or `"unknown"` if parsing fails.
fn extract_swift_version(version_output: &str) -> String {
    // Look for "Swift version X.Y" or "Swift version X.Y.Z"
    if let Some(pos) = version_output.find("Swift version ") {
        let after = &version_output[pos + "Swift version ".len()..];
        let version: String = after
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        let version = version.trim_end_matches('.');
        if !version.is_empty() {
            return version.to_string();
        }
    }
    "unknown".to_string()
}

// ---------------------------------------------------------------------------
// Go resolver
// ---------------------------------------------------------------------------

/// Resolve a Go version to a download URL.
///
/// Supports:
/// - Exact: `"1.23.6"` → direct URL construction
/// - Partial: `"1.23"` → fetches go.dev API to find latest patch, falls back to `.0`
/// - Latest: `"latest"` → fetches go.dev API for latest stable
async fn resolve_go(version: &str, arch: &str) -> Result<(String, String)> {
    let resolved = if version == "latest" {
        resolve_go_version_from_api(version).await?
    } else if version.matches('.').count() < 2 {
        // Partial version like "1.23" — try API first, fall back to "{version}.0"
        // The Go API only lists the 2 most recent release series, so older
        // versions won't be found. Construct "{version}.0" as a fallback since
        // Go always has a .0 release for each minor version.
        resolve_go_version_from_api(version)
            .await
            .unwrap_or_else(|_| format!("{version}.0"))
    } else {
        version.to_string()
    };

    let url = format!("https://go.dev/dl/go{resolved}.darwin-{arch}.tar.gz");
    Ok((resolved, url))
}

/// Fetch the Go downloads API and resolve a version.
async fn resolve_go_version_from_api(version_prefix: &str) -> Result<String> {
    let api_url = "https://go.dev/dl/?mode=json";
    let response = reqwest::get(api_url)
        .await
        .map_err(|e| BuildError::RegistryError {
            message: format!("Failed to fetch Go versions from {api_url}: {e}"),
        })?;

    let releases: Vec<GoRelease> =
        response
            .json()
            .await
            .map_err(|e| BuildError::RegistryError {
                message: format!("Failed to parse Go versions JSON: {e}"),
            })?;

    if version_prefix == "latest" {
        // Return the first stable release
        return releases
            .first()
            .map(|r| {
                r.version
                    .strip_prefix("go")
                    .unwrap_or(&r.version)
                    .to_string()
            })
            .ok_or_else(|| BuildError::RegistryError {
                message: "No Go releases found".to_string(),
            });
    }

    // Find latest patch for the given minor version prefix.
    // Match "go1.23." (with trailing dot) to avoid matching "go1.23rc1".
    let prefix_dot = format!("go{version_prefix}.");
    let prefix_exact = format!("go{version_prefix}");
    for release in &releases {
        if (release.version.starts_with(&prefix_dot) || release.version == prefix_exact)
            && release.stable
        {
            return Ok(release
                .version
                .strip_prefix("go")
                .unwrap_or(&release.version)
                .to_string());
        }
    }

    // If no stable match, try any match
    for release in &releases {
        if release.version.starts_with(&prefix_dot) || release.version == prefix_exact {
            return Ok(release
                .version
                .strip_prefix("go")
                .unwrap_or(&release.version)
                .to_string());
        }
    }

    Err(BuildError::RegistryError {
        message: format!("No Go release found matching version '{version_prefix}'"),
    })
}

#[derive(serde::Deserialize)]
struct GoRelease {
    version: String,
    stable: bool,
}

// ---------------------------------------------------------------------------
// Node.js resolver
// ---------------------------------------------------------------------------

/// Resolve a Node.js version to a download URL.
async fn resolve_node(version: &str, arch: &str) -> Result<(String, String)> {
    let node_arch = match arch {
        "arm64" => "arm64",
        _ => "x64",
    };

    let resolved = if version == "latest" || !version.contains('.') {
        resolve_node_version_from_api(version).await?
    } else {
        version.to_string()
    };

    let url =
        format!("https://nodejs.org/dist/v{resolved}/node-v{resolved}-darwin-{node_arch}.tar.gz");
    Ok((resolved, url))
}

/// Fetch the Node.js dist index and resolve a version.
async fn resolve_node_version_from_api(version_prefix: &str) -> Result<String> {
    let api_url = "https://nodejs.org/dist/index.json";
    let response = reqwest::get(api_url)
        .await
        .map_err(|e| BuildError::RegistryError {
            message: format!("Failed to fetch Node.js versions from {api_url}: {e}"),
        })?;

    let releases: Vec<NodeRelease> =
        response
            .json()
            .await
            .map_err(|e| BuildError::RegistryError {
                message: format!("Failed to parse Node.js versions JSON: {e}"),
            })?;

    if version_prefix == "latest" {
        return releases
            .first()
            .map(|r| {
                r.version
                    .strip_prefix('v')
                    .unwrap_or(&r.version)
                    .to_string()
            })
            .ok_or_else(|| BuildError::RegistryError {
                message: "No Node.js releases found".to_string(),
            });
    }

    // Find latest version matching the major (e.g., "20" → "20.18.1")
    let prefix = format!("v{version_prefix}");
    for release in &releases {
        if release.version.starts_with(&prefix)
            && release
                .version
                .chars()
                .nth(prefix.len())
                .is_none_or(|c| c == '.')
        {
            return Ok(release
                .version
                .strip_prefix('v')
                .unwrap_or(&release.version)
                .to_string());
        }
    }

    Err(BuildError::RegistryError {
        message: format!("No Node.js release found matching version '{version_prefix}'"),
    })
}

#[derive(serde::Deserialize)]
struct NodeRelease {
    version: String,
}

// ---------------------------------------------------------------------------
// Rust resolver
// ---------------------------------------------------------------------------

/// Resolve a Rust version to a download URL.
///
/// Supports:
/// - Exact: `"1.82.0"` -> direct URL construction
/// - Partial: `"1.82"` -> appends `.0` (Rust always has a `.0` patch release)
/// - Latest: `"latest"` -> fetches the stable channel TOML and extracts version
///
/// Rust standalone installers are at:
/// `https://static.rust-lang.org/dist/rust-{version}-{target}.tar.gz`
async fn resolve_rust(version: &str, arch: &str) -> Result<(String, String)> {
    let rust_target = match arch {
        "arm64" => "aarch64-apple-darwin",
        _ => "x86_64-apple-darwin",
    };

    let resolved = if version == "latest" {
        resolve_rust_latest_version().await?
    } else if version.matches('.').count() < 2 {
        // Partial version like "1.82" — Rust always releases x.y.0 for each
        // minor version, so appending ".0" is safe.
        format!("{version}.0")
    } else {
        version.to_string()
    };

    let url = format!("https://static.rust-lang.org/dist/rust-{resolved}-{rust_target}.tar.gz");
    Ok((resolved, url))
}

/// Fetch the Rust stable channel TOML and extract the current stable version.
///
/// The TOML contains a `[pkg.rust]` section with a `version = "1.XX.Y (hash date)"`
/// line. We extract just the semver portion with a simple regex-style scan rather
/// than pulling in a TOML parser.
async fn resolve_rust_latest_version() -> Result<String> {
    let channel_url = "https://static.rust-lang.org/dist/channel-rust-stable.toml";
    let response = reqwest::get(channel_url)
        .await
        .map_err(|e| BuildError::RegistryError {
            message: format!("Failed to fetch Rust stable channel from {channel_url}: {e}"),
        })?;

    let body = response
        .text()
        .await
        .map_err(|e| BuildError::RegistryError {
            message: format!("Failed to read Rust stable channel response: {e}"),
        })?;

    // Look for: version = "1.82.0 (hash date)"
    // We search for the first `version = "X.Y.Z` pattern after `[pkg.rust]`.
    let pkg_rust_pos = body
        .find("[pkg.rust]")
        .ok_or_else(|| BuildError::RegistryError {
            message: "Rust stable channel TOML missing [pkg.rust] section".to_string(),
        })?;

    let after_pkg = &body[pkg_rust_pos..];
    // Find the first `version = "` line in this section
    let version_prefix = "version = \"";
    let ver_start = after_pkg
        .find(version_prefix)
        .ok_or_else(|| BuildError::RegistryError {
            message: "No version field found in [pkg.rust] section".to_string(),
        })?
        + version_prefix.len();

    // Extract just the semver part (digits and dots, before the space or quote)
    let ver_str: String = after_pkg[ver_start..]
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();

    if ver_str.is_empty() {
        return Err(BuildError::RegistryError {
            message: "Failed to parse Rust version from stable channel".to_string(),
        });
    }

    debug!("Resolved Rust latest stable version: {ver_str}");
    Ok(ver_str)
}

// ---------------------------------------------------------------------------
// Python resolver
// ---------------------------------------------------------------------------

/// Resolve a Python version to a download URL.
///
/// Uses standalone Python builds from `astral-sh/python-build-standalone`.
///
/// Supports:
/// - Exact: `"3.12.1"` → scans GitHub releases for matching asset
/// - Partial: `"3.12"` → finds the latest 3.12.x release
/// - Latest: `"latest"` → finds the newest release across all versions
///
/// Asset naming pattern:
/// `cpython-{version}+{timestamp}-{target}-install_only_stripped.tar.gz`
async fn resolve_python(version: &str, arch: &str) -> Result<(String, String)> {
    let python_target = match arch {
        "arm64" => "aarch64-apple-darwin",
        _ => "x86_64-apple-darwin",
    };

    resolve_python_from_github(version, python_target).await
}

/// Fetch the GitHub releases API for `astral-sh/python-build-standalone` and
/// find a matching asset for the requested version and target.
async fn resolve_python_from_github(
    version_prefix: &str,
    target: &str,
) -> Result<(String, String)> {
    let api_url =
        "https://api.github.com/repos/astral-sh/python-build-standalone/releases?per_page=25";

    // GitHub API requires a User-Agent header.
    let client = reqwest::Client::builder()
        .user_agent("zlayer")
        .build()
        .map_err(|e| BuildError::RegistryError {
            message: format!("Failed to build HTTP client: {e}"),
        })?;

    let response = client
        .get(api_url)
        .send()
        .await
        .map_err(|e| BuildError::RegistryError {
            message: format!("Failed to fetch Python releases from GitHub: {e}"),
        })?;

    if !response.status().is_success() {
        return Err(BuildError::RegistryError {
            message: format!(
                "GitHub API returned status {} fetching Python releases",
                response.status()
            ),
        });
    }

    let releases: Vec<GitHubRelease> =
        response
            .json()
            .await
            .map_err(|e| BuildError::RegistryError {
                message: format!("Failed to parse GitHub releases JSON: {e}"),
            })?;

    // Build the suffix we expect in asset names for this target.
    let target_suffix = format!("{target}-install_only_stripped.tar.gz");

    // For "latest", find the first asset matching our target across all releases.
    if version_prefix == "latest" {
        for release in &releases {
            for asset in &release.assets {
                if asset.name.starts_with("cpython-")
                    && asset.name.ends_with(&target_suffix)
                    && asset.name.contains("install_only")
                {
                    let py_version = extract_python_version_from_asset(&asset.name);
                    if !py_version.is_empty() {
                        debug!("Resolved Python latest to {py_version}");
                        return Ok((py_version, asset.browser_download_url.clone()));
                    }
                }
            }
        }
        return Err(BuildError::RegistryError {
            message: format!(
                "No Python release found for target '{target}' in recent GitHub releases"
            ),
        });
    }

    // For exact or partial versions, search for a matching asset.
    // Exact: "3.12.1" matches "cpython-3.12.1+..."
    // Partial: "3.12" matches "cpython-3.12.{anything}+..."
    let exact_prefix = format!("cpython-{version_prefix}+");
    let partial_prefix = format!("cpython-{version_prefix}.");

    for release in &releases {
        for asset in &release.assets {
            if !asset.name.ends_with(&target_suffix) {
                continue;
            }

            // Check exact match first (e.g., "cpython-3.12.1+...")
            if asset.name.starts_with(&exact_prefix) {
                let py_version = extract_python_version_from_asset(&asset.name);
                debug!("Resolved Python {version_prefix} to {py_version} (exact)");
                return Ok((py_version, asset.browser_download_url.clone()));
            }

            // Check partial match (e.g., "cpython-3.12.8+..." for "3.12")
            if asset.name.starts_with(&partial_prefix) {
                let py_version = extract_python_version_from_asset(&asset.name);
                debug!("Resolved Python {version_prefix} to {py_version} (partial)");
                return Ok((py_version, asset.browser_download_url.clone()));
            }
        }
    }

    Err(BuildError::RegistryError {
        message: format!("No Python release found matching version '{version_prefix}'"),
    })
}

/// Extract the Python version from an asset name like
/// `cpython-3.12.8+20250106-aarch64-apple-darwin-install_only_stripped.tar.gz`.
///
/// Returns the version string (e.g., `"3.12.8"`).
fn extract_python_version_from_asset(asset_name: &str) -> String {
    // Strip "cpython-" prefix, then take everything up to the first '+'.
    asset_name
        .strip_prefix("cpython-")
        .and_then(|s| s.split('+').next())
        .unwrap_or("")
        .to_string()
}

#[derive(serde::Deserialize)]
struct GitHubRelease {
    /// Release tag (e.g., `"v2.1.4"`). Optional because not all API consumers
    /// need it (Python resolver uses asset names instead).
    tag_name: Option<String>,
    assets: Vec<GitHubAsset>,
}

#[derive(serde::Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
}

// ---------------------------------------------------------------------------
// Deno resolver
// ---------------------------------------------------------------------------

/// Resolve a Deno version to a download URL.
///
/// Deno distributes pre-built binaries via GitHub releases (`denoland/deno`).
/// Each release asset is a zip containing a single `deno` binary.
///
/// Supports:
/// - Exact: `"2.1.4"` → constructs URL directly from tag `v2.1.4`
/// - Partial: `"2"` → scans GitHub releases for the latest `v2.x.y` tag
/// - Latest: `"latest"` → fetches the most recent release
///
/// Asset naming: `deno-{target}.zip` where target is
/// `aarch64-apple-darwin` (arm64) or `x86_64-apple-darwin` (amd64).
async fn resolve_deno(version: &str, arch: &str) -> Result<(String, String)> {
    let deno_target = match arch {
        "arm64" => "aarch64-apple-darwin",
        _ => "x86_64-apple-darwin",
    };

    if version == "latest" || !version.contains('.') {
        // "latest" or partial like "2" — scan releases API.
        resolve_deno_from_github(version, deno_target).await
    } else {
        // Exact version like "2.1.4" — construct the URL directly.
        let url = format!(
            "https://github.com/denoland/deno/releases/download/v{version}/deno-{deno_target}.zip"
        );
        Ok((version.to_string(), url))
    }
}

/// Fetch the GitHub releases API for `denoland/deno` and find a matching
/// release for the requested version and target.
async fn resolve_deno_from_github(version_prefix: &str, target: &str) -> Result<(String, String)> {
    let api_url = "https://api.github.com/repos/denoland/deno/releases?per_page=25";

    let client = reqwest::Client::builder()
        .user_agent("zlayer")
        .build()
        .map_err(|e| BuildError::RegistryError {
            message: format!("Failed to build HTTP client: {e}"),
        })?;

    let response = client
        .get(api_url)
        .send()
        .await
        .map_err(|e| BuildError::RegistryError {
            message: format!("Failed to fetch Deno releases from GitHub: {e}"),
        })?;

    if !response.status().is_success() {
        return Err(BuildError::RegistryError {
            message: format!(
                "GitHub API returned status {} fetching Deno releases",
                response.status()
            ),
        });
    }

    let releases: Vec<GitHubRelease> =
        response
            .json()
            .await
            .map_err(|e| BuildError::RegistryError {
                message: format!("Failed to parse GitHub releases JSON: {e}"),
            })?;

    let asset_name = format!("deno-{target}.zip");

    if version_prefix == "latest" {
        // Return the first release that has our target asset.
        for release in &releases {
            for asset in &release.assets {
                if asset.name == asset_name {
                    let tag = release
                        .tag_name
                        .as_deref()
                        .unwrap_or("")
                        .strip_prefix('v')
                        .unwrap_or(release.tag_name.as_deref().unwrap_or(""));
                    if !tag.is_empty() {
                        debug!("Resolved Deno latest to {tag}");
                        return Ok((tag.to_string(), asset.browser_download_url.clone()));
                    }
                }
            }
        }
        return Err(BuildError::RegistryError {
            message: format!(
                "No Deno release found for target '{target}' in recent GitHub releases"
            ),
        });
    }

    // Partial version like "2" — find the first release whose tag starts with
    // `v{prefix}.` (e.g., "v2." matches "v2.1.4").
    let tag_prefix = format!("v{version_prefix}.");
    for release in &releases {
        let tag = release.tag_name.as_deref().unwrap_or("");
        if tag.starts_with(&tag_prefix) {
            for asset in &release.assets {
                if asset.name == asset_name {
                    let ver = tag.strip_prefix('v').unwrap_or(tag);
                    debug!("Resolved Deno {version_prefix} to {ver} (partial)");
                    return Ok((ver.to_string(), asset.browser_download_url.clone()));
                }
            }
        }
    }

    Err(BuildError::RegistryError {
        message: format!("No Deno release found matching version '{version_prefix}'"),
    })
}

// ---------------------------------------------------------------------------
// Bun resolver
// ---------------------------------------------------------------------------

/// Resolve a Bun version to a download URL.
///
/// Bun distributes pre-built binaries via GitHub releases (`oven-sh/bun`).
/// Each release asset is a zip containing a directory with a single `bun` binary.
///
/// Supports:
/// - Exact: `"1.2.3"` -> constructs URL directly from tag `bun-v1.2.3`
/// - Partial: `"1"` -> scans GitHub releases for the latest `bun-v1.x.y` tag
/// - Latest: `"latest"` -> fetches the most recent release
///
/// Asset naming: `bun-darwin-{arch}.zip` where arch is `aarch64` (arm64) or
/// `x64` (amd64). The zip extracts as `bun-darwin-{arch}/bun`.
async fn resolve_bun(version: &str, arch: &str) -> Result<(String, String)> {
    let bun_arch = match arch {
        "arm64" => "aarch64",
        _ => "x64",
    };

    if version == "latest" || !version.contains('.') {
        // "latest" or partial like "1" — scan releases API.
        resolve_bun_from_github(version, bun_arch).await
    } else {
        // Exact version like "1.2.3" — construct the URL directly.
        let url = format!(
            "https://github.com/oven-sh/bun/releases/download/bun-v{version}/bun-darwin-{bun_arch}.zip"
        );
        Ok((version.to_string(), url))
    }
}

/// Fetch the GitHub releases API for `oven-sh/bun` and find a matching
/// release for the requested version.
async fn resolve_bun_from_github(version_prefix: &str, bun_arch: &str) -> Result<(String, String)> {
    let api_url = "https://api.github.com/repos/oven-sh/bun/releases?per_page=25";

    let client = reqwest::Client::builder()
        .user_agent("zlayer")
        .build()
        .map_err(|e| BuildError::RegistryError {
            message: format!("Failed to build HTTP client: {e}"),
        })?;

    let response = client
        .get(api_url)
        .send()
        .await
        .map_err(|e| BuildError::RegistryError {
            message: format!("Failed to fetch Bun releases from GitHub: {e}"),
        })?;

    if !response.status().is_success() {
        return Err(BuildError::RegistryError {
            message: format!(
                "GitHub API returned status {} fetching Bun releases",
                response.status()
            ),
        });
    }

    let releases: Vec<GitHubRelease> =
        response
            .json()
            .await
            .map_err(|e| BuildError::RegistryError {
                message: format!("Failed to parse GitHub releases JSON: {e}"),
            })?;

    let asset_name = format!("bun-darwin-{bun_arch}.zip");

    if version_prefix == "latest" {
        // Return the first release that has our target asset.
        for release in &releases {
            let tag = release.tag_name.as_deref().unwrap_or("");
            // Bun tags are "bun-v1.2.3"
            let ver = tag
                .strip_prefix("bun-v")
                .unwrap_or(tag.strip_prefix('v').unwrap_or(tag));
            if ver.is_empty() {
                continue;
            }
            for asset in &release.assets {
                if asset.name == asset_name {
                    debug!("Resolved Bun latest to {ver}");
                    return Ok((ver.to_string(), asset.browser_download_url.clone()));
                }
            }
        }
        return Err(BuildError::RegistryError {
            message: format!(
                "No Bun release found for arch '{bun_arch}' in recent GitHub releases"
            ),
        });
    }

    // Partial version like "1" — find the first release whose tag starts with
    // `bun-v{prefix}.` (e.g., "bun-v1." matches "bun-v1.2.3").
    let tag_prefix = format!("bun-v{version_prefix}.");
    for release in &releases {
        let tag = release.tag_name.as_deref().unwrap_or("");
        if tag.starts_with(&tag_prefix) {
            for asset in &release.assets {
                if asset.name == asset_name {
                    let ver = tag.strip_prefix("bun-v").unwrap_or(tag);
                    debug!("Resolved Bun {version_prefix} to {ver} (partial)");
                    return Ok((ver.to_string(), asset.browser_download_url.clone()));
                }
            }
        }
    }

    Err(BuildError::RegistryError {
        message: format!("No Bun release found matching version '{version_prefix}'"),
    })
}

// ---------------------------------------------------------------------------
// Zig resolver
// ---------------------------------------------------------------------------

/// Deserialize a single platform entry from the Zig download index.
///
/// Each version key maps to a platform map (e.g., `"aarch64-macos"`) whose
/// values contain at least a `tarball` URL.
#[derive(serde::Deserialize)]
struct ZigDownloadInfo {
    tarball: String,
}

/// Resolve a Zig version to a download URL.
///
/// Uses the official Zig download index at `https://ziglang.org/download/index.json`.
/// The index is a JSON object where keys are version strings (e.g., `"0.14.0"`,
/// `"master"`) and values contain platform-specific download info.
///
/// Supports:
/// - Exact: `"0.14.0"` -> direct lookup in the index
/// - Partial: `"0.14"` -> finds first key starting with that prefix
/// - Latest: `"latest"` -> finds the first non-`"master"` key (latest stable)
///
/// Zig tarballs are `.tar.xz` and extract as `zig-macos-{arch}-{version}/`
/// containing the `zig` binary directly (no `bin/` subdirectory).
async fn resolve_zig(version: &str, arch: &str) -> Result<(String, String)> {
    let platform_key = match arch {
        "arm64" => "aarch64-macos",
        _ => "x86_64-macos",
    };

    let index_url = "https://ziglang.org/download/index.json";
    let response = reqwest::get(index_url)
        .await
        .map_err(|e| BuildError::RegistryError {
            message: format!("Failed to fetch Zig download index from {index_url}: {e}"),
        })?;

    let index: HashMap<String, serde_json::Value> =
        response
            .json()
            .await
            .map_err(|e| BuildError::RegistryError {
                message: format!("Failed to parse Zig download index JSON: {e}"),
            })?;

    // Resolve the version string to a concrete version key in the index.
    let resolved = if version == "latest" {
        // Find the first non-"master" key. The index is an object (unordered),
        // so collect keys, filter out "master", sort descending, and take the
        // first — which is the highest (latest stable) version.
        let mut versions: Vec<&String> = index.keys().filter(|k| *k != "master").collect();
        versions.sort_by(|a, b| compare_version_strings(b, a));
        versions
            .first()
            .map(|v| (*v).clone())
            .ok_or_else(|| BuildError::RegistryError {
                message: "No stable Zig versions found in download index".to_string(),
            })?
    } else if index.contains_key(version) {
        // Exact match (e.g., "0.14.0")
        version.to_string()
    } else {
        // Partial match (e.g., "0.14" matches "0.14.0")
        let prefix = format!("{version}.");
        let mut candidates: Vec<&String> = index
            .keys()
            .filter(|k| *k != "master" && k.starts_with(&prefix))
            .collect();
        candidates.sort_by(|a, b| compare_version_strings(b, a));
        candidates
            .first()
            .map(|v| (*v).clone())
            .ok_or_else(|| BuildError::RegistryError {
                message: format!("No Zig version found matching '{version}'"),
            })?
    };

    // Extract the platform-specific download info.
    let version_data = index
        .get(&resolved)
        .ok_or_else(|| BuildError::RegistryError {
            message: format!("Zig version '{resolved}' not found in download index"),
        })?;

    let platform_data =
        version_data
            .get(platform_key)
            .ok_or_else(|| BuildError::RegistryError {
                message: format!(
                    "No Zig download found for platform '{platform_key}' in version '{resolved}'"
                ),
            })?;

    let info: ZigDownloadInfo =
        serde_json::from_value(platform_data.clone()).map_err(|e| BuildError::RegistryError {
            message: format!(
                "Failed to parse Zig download info for {platform_key}/{resolved}: {e}"
            ),
        })?;

    debug!("Resolved Zig {version} to {resolved}: {}", info.tarball);
    Ok((resolved, info.tarball))
}

/// Compare two dotted version strings numerically for sorting.
///
/// Splits on `'.'` and compares each segment as a number, falling back to
/// lexicographic comparison for non-numeric segments. Used to find the
/// highest version when the Zig index keys are unordered.
fn compare_version_strings(a: &str, b: &str) -> std::cmp::Ordering {
    let a_parts: Vec<&str> = a.split('.').collect();
    let b_parts: Vec<&str> = b.split('.').collect();
    for (ap, bp) in a_parts.iter().zip(b_parts.iter()) {
        let ord = match (ap.parse::<u64>(), bp.parse::<u64>()) {
            (Ok(an), Ok(bn)) => an.cmp(&bn),
            _ => ap.cmp(bp),
        };
        if ord != std::cmp::Ordering::Equal {
            return ord;
        }
    }
    a_parts.len().cmp(&b_parts.len())
}

// ---------------------------------------------------------------------------
// Java (Adoptium/Temurin) resolver
// ---------------------------------------------------------------------------

/// Response from the Adoptium available releases API.
#[derive(serde::Deserialize)]
struct AdoptiumAvailableReleases {
    /// The most recent LTS feature version (e.g., 21).
    most_recent_lts: u32,
}

/// Resolve a Java (Adoptium/Temurin) version to a download URL.
///
/// Uses the Adoptium API to resolve versions and construct download URLs.
/// The binary endpoint (`/v3/binary/latest/{version}/...`) redirects (302) to
/// the actual `.tar.gz` — reqwest follows redirects by default so we can use
/// the URL directly.
///
/// Supports:
/// - Exact feature version: `"21"`, `"17"`, `"8"` → uses as feature version directly
/// - Latest: `"latest"` → fetches available releases API, uses `most_recent_lts`
///
/// macOS tarball structure:
/// ```text
/// jdk-21.0.5+11/
///   Contents/
///     Home/
///       bin/java
///       lib/
///       ...
/// ```
/// Extraction uses `--strip-components=3` to reach the JDK root.
async fn resolve_java(version: &str, arch: &str) -> Result<(String, String)> {
    let adoptium_arch = match arch {
        "arm64" => "aarch64",
        _ => "x64",
    };

    let feature_version = if version == "latest" {
        resolve_java_latest_lts().await?
    } else {
        // Strip any dotted sub-version — Adoptium API expects the major
        // feature version only (e.g., "21" not "21.0.5").
        version.split('.').next().unwrap_or(version).to_string()
    };

    let url = format!(
        "https://api.adoptium.net/v3/binary/latest/{feature_version}/ga/mac/{adoptium_arch}/jdk/hotspot/normal/eclipse"
    );

    Ok((feature_version, url))
}

/// Fetch the Adoptium available releases API and return the most recent LTS
/// feature version as a string (e.g., `"21"`).
async fn resolve_java_latest_lts() -> Result<String> {
    let api_url = "https://api.adoptium.net/v3/info/available_releases";
    let response = reqwest::get(api_url)
        .await
        .map_err(|e| BuildError::RegistryError {
            message: format!("Failed to fetch Adoptium available releases from {api_url}: {e}"),
        })?;

    if !response.status().is_success() {
        return Err(BuildError::RegistryError {
            message: format!(
                "Adoptium API returned status {} fetching available releases",
                response.status()
            ),
        });
    }

    let releases: AdoptiumAvailableReleases =
        response
            .json()
            .await
            .map_err(|e| BuildError::RegistryError {
                message: format!("Failed to parse Adoptium available releases JSON: {e}"),
            })?;

    let version = releases.most_recent_lts.to_string();
    debug!("Resolved Java latest LTS to feature version {version}");
    Ok(version)
}

// ---------------------------------------------------------------------------
// GraalVM CE resolver
// ---------------------------------------------------------------------------

/// Resolve a `GraalVM` CE version to a download URL.
///
/// Uses the GitHub releases API for `graalvm/graalvm-ce-builds`.
/// Release tags follow the pattern `jdk-{java_version}` (e.g., `jdk-21.0.5`).
///
/// Supports:
/// - Exact: `"21.0.5"` → constructs URL directly
/// - Partial (major only): `"21"` → scans GitHub releases for the latest `jdk-21.x.y` tag
/// - Latest: `"latest"` → fetches the most recent release tag
///
/// macOS tarball structure (same as Adoptium):
/// ```text
/// graalvm-community-openjdk-21.0.5+9.1/
///   Contents/
///     Home/
///       bin/java
///       bin/native-image
///       lib/
///       ...
/// ```
/// Extraction uses `--strip-components=3` to reach the JDK root.
async fn resolve_graalvm(version: &str, arch: &str) -> Result<(String, String)> {
    let graalvm_arch = match arch {
        "arm64" => "aarch64",
        _ => "x64",
    };

    if version == "latest" || !version.contains('.') {
        // "latest" or partial like "21" — scan GitHub releases API.
        resolve_graalvm_from_github(version, graalvm_arch).await
    } else {
        // Exact version like "21.0.5" — construct the URL directly.
        let url = format!(
            "https://github.com/graalvm/graalvm-ce-builds/releases/download/\
             jdk-{version}/graalvm-community-jdk-{version}_macos-{graalvm_arch}_bin.tar.gz"
        );
        Ok((version.to_string(), url))
    }
}

/// Fetch the GitHub releases API for `graalvm/graalvm-ce-builds` and find a
/// matching release for the requested version.
async fn resolve_graalvm_from_github(
    version_prefix: &str,
    graalvm_arch: &str,
) -> Result<(String, String)> {
    let api_url = "https://api.github.com/repos/graalvm/graalvm-ce-builds/releases?per_page=25";

    let client = reqwest::Client::builder()
        .user_agent("zlayer")
        .build()
        .map_err(|e| BuildError::RegistryError {
            message: format!("Failed to build HTTP client: {e}"),
        })?;

    let response = client
        .get(api_url)
        .send()
        .await
        .map_err(|e| BuildError::RegistryError {
            message: format!("Failed to fetch GraalVM releases from GitHub: {e}"),
        })?;

    if !response.status().is_success() {
        return Err(BuildError::RegistryError {
            message: format!(
                "GitHub API returned status {} fetching GraalVM releases",
                response.status()
            ),
        });
    }

    let releases: Vec<GitHubRelease> =
        response
            .json()
            .await
            .map_err(|e| BuildError::RegistryError {
                message: format!("Failed to parse GitHub releases JSON: {e}"),
            })?;

    if version_prefix == "latest" {
        // Return the first release that has a valid jdk- tag.
        for release in &releases {
            let tag = release.tag_name.as_deref().unwrap_or("");
            if let Some(jdk_version) = tag.strip_prefix("jdk-") {
                if jdk_version.is_empty() {
                    continue;
                }
                let url = format!(
                    "https://github.com/graalvm/graalvm-ce-builds/releases/download/\
                     {tag}/graalvm-community-jdk-{jdk_version}_macos-{graalvm_arch}_bin.tar.gz"
                );
                debug!("Resolved GraalVM latest to {jdk_version}");
                return Ok((jdk_version.to_string(), url));
            }
        }
        return Err(BuildError::RegistryError {
            message: "No GraalVM CE release found in recent GitHub releases".to_string(),
        });
    }

    // Partial version like "21" — find the first release whose tag starts with
    // `jdk-{prefix}.` (e.g., "jdk-21." matches "jdk-21.0.5").
    let tag_prefix = format!("jdk-{version_prefix}.");
    for release in &releases {
        let tag = release.tag_name.as_deref().unwrap_or("");
        if tag.starts_with(&tag_prefix) {
            if let Some(jdk_version) = tag.strip_prefix("jdk-") {
                let url = format!(
                    "https://github.com/graalvm/graalvm-ce-builds/releases/download/\
                     {tag}/graalvm-community-jdk-{jdk_version}_macos-{graalvm_arch}_bin.tar.gz"
                );
                debug!("Resolved GraalVM {version_prefix} to {jdk_version} (partial)");
                return Ok((jdk_version.to_string(), url));
            }
        }
    }

    Err(BuildError::RegistryError {
        message: format!("No GraalVM CE release found matching version '{version_prefix}'"),
    })
}

// ---------------------------------------------------------------------------
// Download + extraction helpers
// ---------------------------------------------------------------------------

/// Download a file from a URL.
async fn download_file(url: &str, dest: &Path) -> Result<()> {
    let response = reqwest::get(url)
        .await
        .map_err(|e| BuildError::RegistryError {
            message: format!("Failed to download {url}: {e}"),
        })?;

    if !response.status().is_success() {
        return Err(BuildError::RegistryError {
            message: format!("Download failed with status {}: {url}", response.status()),
        });
    }

    let bytes = response
        .bytes()
        .await
        .map_err(|e| BuildError::RegistryError {
            message: format!("Failed to read response body from {url}: {e}"),
        })?;

    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(dest, &bytes).await?;

    Ok(())
}

/// Extract a toolchain archive into the target directory.
#[allow(clippy::too_many_lines)]
async fn extract_toolchain(language: &str, archive: &Path, target_dir: &Path) -> Result<()> {
    let archive_str = archive.display().to_string();
    let target_str = target_dir.display().to_string();

    let output = match language {
        "go" => {
            // Go tarballs extract as `go/` — we want contents in target_dir
            tokio::process::Command::new("tar")
                .args([
                    "xzf",
                    &archive_str,
                    "-C",
                    &target_str,
                    "--strip-components=1",
                ])
                .output()
                .await?
        }
        "node" => {
            // Node tarballs extract as `node-v{version}-darwin-{arch}/`
            tokio::process::Command::new("tar")
                .args([
                    "xzf",
                    &archive_str,
                    "-C",
                    &target_str,
                    "--strip-components=1",
                ])
                .output()
                .await?
        }
        "rust" => {
            // Rust standalone tarballs contain an installer script. Extract to
            // a temporary directory, then run install.sh with --prefix to place
            // binaries (rustc, cargo, etc.) into the target cache directory.
            let extract_tmp = target_dir.join("_extract");
            tokio::fs::create_dir_all(&extract_tmp).await?;
            let extract_tmp_str = extract_tmp.display().to_string();

            let tar_out = tokio::process::Command::new("tar")
                .args([
                    "xzf",
                    &archive_str,
                    "-C",
                    &extract_tmp_str,
                    "--strip-components=1",
                ])
                .output()
                .await?;

            if !tar_out.status.success() {
                let stderr = String::from_utf8_lossy(&tar_out.stderr);
                let _ = tokio::fs::remove_dir_all(&extract_tmp).await;
                return Err(BuildError::RegistryError {
                    message: format!("Failed to extract Rust tarball: {stderr}"),
                });
            }

            // Run the bundled installer
            let install_sh = extract_tmp.join("install.sh");
            let install_out = tokio::process::Command::new("sh")
                .arg(install_sh.display().to_string())
                .arg(format!("--prefix={target_str}"))
                .arg("--disable-ldconfig")
                .output()
                .await?;

            // Clean up the temporary extraction directory regardless of outcome
            let _ = tokio::fs::remove_dir_all(&extract_tmp).await;

            if !install_out.status.success() {
                let stderr = String::from_utf8_lossy(&install_out.stderr);
                return Err(BuildError::RegistryError {
                    message: format!("Rust install.sh failed: {stderr}"),
                });
            }

            return Ok(());
        }
        "deno" => {
            // Deno zip contains a single `deno` binary. Unzip, then move into
            // bin/ structure so PATH symlinks work correctly.
            let out = tokio::process::Command::new("unzip")
                .args(["-o", &archive_str, "-d", &target_str])
                .output()
                .await?;
            if out.status.success() {
                let bin_dir = target_dir.join("bin");
                tokio::fs::create_dir_all(&bin_dir).await?;
                let deno_binary = target_dir.join("deno");
                if deno_binary.exists() {
                    tokio::fs::rename(&deno_binary, bin_dir.join("deno")).await?;
                }
            }
            out
        }
        "zig" => {
            // Zig uses .tar.xz format. The tarball extracts as
            // `zig-macos-{arch}-{version}/` containing the `zig` binary and
            // `lib/` directly — no `bin/` subdirectory. We strip the top-level
            // directory, then create `bin/` with a symlink to `../zig` so that
            // PATH-based resolution works.
            let out = tokio::process::Command::new("tar")
                .args([
                    "xJf",
                    &archive_str,
                    "-C",
                    &target_str,
                    "--strip-components=1",
                ])
                .output()
                .await?;
            if out.status.success() {
                let bin_dir = target_dir.join("bin");
                tokio::fs::create_dir_all(&bin_dir).await?;
                let zig_binary = target_dir.join("zig");
                if zig_binary.exists() && !bin_dir.join("zig").exists() {
                    tokio::fs::symlink(Path::new("../zig"), &bin_dir.join("zig")).await?;
                }
            }
            out
        }
        "java" | "graalvm" => {
            // Adoptium and GraalVM CE macOS tarballs extract as:
            //   jdk-{version}+{build}/Contents/Home/bin/java
            //   jdk-{version}+{build}/Contents/Home/lib/...
            // We need --strip-components=3 to get to the actual JDK root
            // (stripping jdk-X/Contents/Home/).
            tokio::process::Command::new("tar")
                .args([
                    "xzf",
                    &archive_str,
                    "-C",
                    &target_str,
                    "--strip-components=3",
                ])
                .output()
                .await?
        }
        "bun" => {
            // Bun extracts as bun-darwin-{arch}/bun. Move into bin/ structure.
            let out = tokio::process::Command::new("unzip")
                .args(["-o", &archive_str, "-d", &target_str])
                .output()
                .await?;
            if out.status.success() {
                let bin_dir = target_dir.join("bin");
                tokio::fs::create_dir_all(&bin_dir).await?;
                // Find the bun binary in any subdirectory
                if let Ok(mut entries) = tokio::fs::read_dir(target_dir).await {
                    while let Ok(Some(entry)) = entries.next_entry().await {
                        if entry.file_name().to_string_lossy().starts_with("bun-") {
                            let bun_binary = entry.path().join("bun");
                            if bun_binary.exists() {
                                tokio::fs::rename(&bun_binary, bin_dir.join("bun")).await?;
                                let _ = tokio::fs::remove_dir_all(entry.path()).await;
                            }
                        }
                    }
                }
            }
            out
        }
        _ => {
            // Default: tar.gz with strip
            tokio::process::Command::new("tar")
                .args([
                    "xzf",
                    &archive_str,
                    "-C",
                    &target_str,
                    "--strip-components=1",
                ])
                .output()
                .await?
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(BuildError::RegistryError {
            message: format!("Failed to extract {language} toolchain: {stderr}"),
        });
    }

    Ok(())
}

/// Symlink a cached toolchain into the build rootfs at the spec's `install_dir`.
///
/// The toolchain cache is the immutable source of truth. Each build's rootfs
/// gets a symlink pointing to the cached directory, avoiding redundant copies
/// and saving both time and disk space.
async fn copy_toolchain_to_rootfs(
    cached: &Path,
    rootfs_dir: &Path,
    spec: &ToolchainSpec,
) -> Result<()> {
    let install_target = rootfs_dir.join(
        spec.install_dir
            .strip_prefix('/')
            .unwrap_or(&spec.install_dir),
    );

    // Ensure parent directory exists
    if let Some(parent) = install_target.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    // Remove existing target if present — symlink creation fails otherwise
    if install_target.exists() || install_target.symlink_metadata().is_ok() {
        let _ = tokio::fs::remove_dir_all(&install_target).await;
        let _ = tokio::fs::remove_file(&install_target).await;
    }

    // Symlink the toolchain from cache — read-only, shared across builds
    tokio::fs::symlink(cached, &install_target)
        .await
        .map_err(|e| {
            BuildError::IoError(std::io::Error::new(
                e.kind(),
                format!(
                    "Failed to symlink toolchain {} → {}: {e}",
                    cached.display(),
                    install_target.display()
                ),
            ))
        })?;

    // Create PATH symlinks in usr/local/bin
    let bin_dir = rootfs_dir.join("usr/local/bin");
    tokio::fs::create_dir_all(&bin_dir).await?;

    // Symlink key binaries (relative symlinks within the rootfs)
    let toolchain_bin = install_target.join("bin");
    if toolchain_bin.exists() {
        if let Ok(mut entries) = tokio::fs::read_dir(&toolchain_bin).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                let name = entry.file_name();
                let link_path = bin_dir.join(&name);
                if !link_path.exists() {
                    let target = format!(
                        "../{}/bin/{}",
                        spec.install_dir
                            .strip_prefix("/usr/local/")
                            .unwrap_or(&spec.install_dir),
                        name.to_string_lossy()
                    );
                    let _ = tokio::fs::symlink(&target, &link_path).await;
                }
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Base rootfs bootstrapping
// ---------------------------------------------------------------------------

/// Ensure a base rootfs exists with essential macOS host binaries.
///
/// This replicates the logic from `images/macos/ZImagefile.base`: copies
/// common host binaries, SSL certs, and creates the basic directory structure.
///
/// # Errors
///
/// Returns an error if directory creation or binary copying fails.
pub async fn ensure_base_rootfs(rootfs_dir: &Path) -> Result<()> {
    // Create directory structure
    for dir in [
        "bin",
        "usr/bin",
        "usr/local/bin",
        "usr/local/lib",
        "etc/ssl",
        "tmp",
        "var/tmp",
        "var/log",
        "opt",
    ] {
        tokio::fs::create_dir_all(rootfs_dir.join(dir)).await?;
    }

    // Copy essential binaries from /bin
    let bin_binaries = [
        "sh", "bash", "cat", "cp", "echo", "ls", "mkdir", "mv", "rm", "sleep", "chmod", "ln",
        "test", "expr", "date", "dd", "ps", "kill", "hostname",
    ];
    for bin in &bin_binaries {
        let src = PathBuf::from("/bin").join(bin);
        if src.exists() {
            let _ = tokio::fs::copy(&src, rootfs_dir.join("bin").join(bin)).await;
        }
    }

    // Copy essential binaries from /usr/bin
    let usr_binaries = [
        "env", "which", "xargs", "tar", "gzip", "curl", "git", "make", "true", "false", "head",
        "tail", "grep", "sed", "awk", "sort", "uniq", "wc", "find", "tee", "touch", "cut", "tr",
        "dirname", "basename", "install", "id", "whoami", "zip", "unzip", "openssl",
    ];
    for bin in &usr_binaries {
        let src = PathBuf::from("/usr/bin").join(bin);
        if src.exists() {
            let _ = tokio::fs::copy(&src, rootfs_dir.join("usr/bin").join(bin)).await;
        }
    }

    // Copy SSL certificates
    let ssl_certs = PathBuf::from("/etc/ssl/certs");
    if ssl_certs.exists() {
        let target = rootfs_dir.join("etc/ssl/certs");
        tokio::fs::create_dir_all(&target).await?;
        let output = tokio::process::Command::new("cp")
            .args(["-R"])
            .arg(format!("{}/.", ssl_certs.display()))
            .arg(target.display().to_string())
            .output()
            .await;
        if let Err(e) = output {
            debug!("Could not copy SSL certs: {e}");
        }
    }
    let cert_pem = PathBuf::from("/etc/ssl/cert.pem");
    if cert_pem.exists() {
        let _ = tokio::fs::copy(&cert_pem, rootfs_dir.join("etc/ssl/cert.pem")).await;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_golang() {
        let spec = detect_toolchain("golang:1.23-alpine").unwrap();
        assert_eq!(spec.language, "go");
        assert_eq!(spec.version, "1.23");
    }

    #[test]
    fn test_detect_golang_exact() {
        let spec = detect_toolchain("golang:1.23.6").unwrap();
        assert_eq!(spec.language, "go");
        assert_eq!(spec.version, "1.23.6");
    }

    #[test]
    fn test_detect_node() {
        let spec = detect_toolchain("node:20-slim").unwrap();
        assert_eq!(spec.language, "node");
        assert_eq!(spec.version, "20");
    }

    #[test]
    fn test_detect_node_latest() {
        let spec = detect_toolchain("node:latest").unwrap();
        assert_eq!(spec.language, "node");
        assert_eq!(spec.version, "latest");
    }

    #[test]
    fn test_detect_python() {
        let spec = detect_toolchain("python:3.12-bookworm").unwrap();
        assert_eq!(spec.language, "python");
        assert_eq!(spec.version, "3.12");
    }

    #[test]
    fn test_detect_rust() {
        let spec = detect_toolchain("rust:1.82-alpine").unwrap();
        assert_eq!(spec.language, "rust");
        assert_eq!(spec.version, "1.82");
    }

    #[test]
    fn test_detect_alpine_no_toolchain() {
        assert!(detect_toolchain("alpine:latest").is_none());
    }

    #[test]
    fn test_detect_unknown_no_toolchain() {
        assert!(detect_toolchain("myapp:v2").is_none());
    }

    #[test]
    fn test_detect_docker_hub_qualified() {
        let spec = detect_toolchain("docker.io/library/golang:1.23-alpine").unwrap();
        assert_eq!(spec.language, "go");
        assert_eq!(spec.version, "1.23");
    }

    #[test]
    fn test_extract_version_from_tag_with_suffix() {
        assert_eq!(extract_version_from_tag("1.23-alpine"), "1.23");
        assert_eq!(extract_version_from_tag("20-slim"), "20");
        assert_eq!(extract_version_from_tag("3.12.1-bookworm"), "3.12.1");
    }

    #[test]
    fn test_extract_version_from_tag_bare() {
        assert_eq!(extract_version_from_tag("1.23.6"), "1.23.6");
        assert_eq!(extract_version_from_tag("20"), "20");
    }

    #[test]
    fn test_extract_version_from_tag_latest() {
        assert_eq!(extract_version_from_tag("latest"), "latest");
    }

    #[test]
    fn test_extract_version_from_tag_no_version() {
        assert_eq!(extract_version_from_tag("alpine"), "latest");
        assert_eq!(extract_version_from_tag("slim"), "latest");
    }

    #[test]
    fn test_go_spec_env() {
        let spec = ToolchainSpec::go("1.23");
        assert_eq!(spec.env.get("GOROOT"), Some(&"/usr/local/go".to_string()));
        assert_eq!(
            spec.env.get("GOFLAGS"),
            Some(&"-buildvcs=false".to_string())
        );
    }

    #[test]
    fn test_java_spec_env() {
        let spec = ToolchainSpec::java("21");
        assert_eq!(
            spec.env.get("JAVA_HOME"),
            Some(&"/usr/local/java".to_string())
        );
    }

    #[test]
    fn test_rust_spec_env() {
        let spec = ToolchainSpec::rust("1.82.0");
        assert_eq!(spec.language, "rust");
        assert_eq!(spec.version, "1.82.0");
        assert_eq!(spec.install_dir, "/usr/local/rust");
        assert_eq!(
            spec.env.get("CARGO_HOME"),
            Some(&"/usr/local/cargo".to_string())
        );
        assert_eq!(
            spec.env.get("RUSTUP_HOME"),
            Some(&"/usr/local/rustup".to_string())
        );
        assert!(spec.path_dirs.contains(&"/usr/local/cargo/bin".to_string()));
        assert!(spec.path_dirs.contains(&"/usr/local/rust/bin".to_string()));
    }

    #[test]
    fn test_detect_rust_exact() {
        let spec = detect_toolchain("rust:1.82.0").unwrap();
        assert_eq!(spec.language, "rust");
        assert_eq!(spec.version, "1.82.0");
    }

    #[test]
    fn test_detect_rust_latest() {
        let spec = detect_toolchain("rust:latest").unwrap();
        assert_eq!(spec.language, "rust");
        assert_eq!(spec.version, "latest");
    }

    #[test]
    fn test_detect_rust_no_tag() {
        let spec = detect_toolchain("rust").unwrap();
        assert_eq!(spec.language, "rust");
        assert_eq!(spec.version, "latest");
    }

    #[test]
    fn test_python_spec() {
        let spec = ToolchainSpec::python("3.12");
        assert_eq!(spec.language, "python");
        assert_eq!(spec.version, "3.12");
        assert_eq!(spec.install_dir, "/usr/local/python");
        assert!(spec
            .path_dirs
            .contains(&"/usr/local/python/bin".to_string()));
        assert!(spec.env.is_empty());
    }

    #[test]
    fn test_detect_python_partial() {
        let spec = detect_toolchain("python:3.12-bookworm").unwrap();
        assert_eq!(spec.language, "python");
        assert_eq!(spec.version, "3.12");
    }

    #[test]
    fn test_detect_python_exact() {
        let spec = detect_toolchain("python:3.12.1").unwrap();
        assert_eq!(spec.language, "python");
        assert_eq!(spec.version, "3.12.1");
    }

    #[test]
    fn test_detect_python3_alias() {
        let spec = detect_toolchain("python3:3.11-slim").unwrap();
        assert_eq!(spec.language, "python");
        assert_eq!(spec.version, "3.11");
    }

    #[test]
    fn test_detect_python_latest() {
        let spec = detect_toolchain("python:latest").unwrap();
        assert_eq!(spec.language, "python");
        assert_eq!(spec.version, "latest");
    }

    #[test]
    fn test_detect_python_no_tag() {
        let spec = detect_toolchain("python").unwrap();
        assert_eq!(spec.language, "python");
        assert_eq!(spec.version, "latest");
    }

    #[test]
    fn test_extract_python_version_from_asset_name() {
        assert_eq!(
            extract_python_version_from_asset(
                "cpython-3.12.8+20250106-aarch64-apple-darwin-install_only_stripped.tar.gz"
            ),
            "3.12.8"
        );
        assert_eq!(
            extract_python_version_from_asset(
                "cpython-3.11.11+20250106-x86_64-apple-darwin-install_only_stripped.tar.gz"
            ),
            "3.11.11"
        );
        assert_eq!(
            extract_python_version_from_asset(
                "cpython-3.13.1+20250106-aarch64-apple-darwin-install_only_stripped.tar.gz"
            ),
            "3.13.1"
        );
    }

    #[test]
    fn test_extract_python_version_from_asset_edge_cases() {
        // Malformed or unexpected names
        assert_eq!(extract_python_version_from_asset("not-a-cpython-asset"), "");
        assert_eq!(extract_python_version_from_asset("cpython-"), "");
    }

    #[test]
    fn test_deno_spec() {
        let spec = ToolchainSpec::deno("2.1.4");
        assert_eq!(spec.language, "deno");
        assert_eq!(spec.version, "2.1.4");
        assert_eq!(spec.install_dir, "/usr/local/deno");
        assert!(spec.path_dirs.contains(&"/usr/local/deno/bin".to_string()));
        assert!(spec.env.is_empty());
    }

    #[test]
    fn test_detect_deno() {
        let spec = detect_toolchain("deno:2.1.4").unwrap();
        assert_eq!(spec.language, "deno");
        assert_eq!(spec.version, "2.1.4");
    }

    #[test]
    fn test_detect_deno_partial() {
        let spec = detect_toolchain("deno:2-alpine").unwrap();
        assert_eq!(spec.language, "deno");
        assert_eq!(spec.version, "2");
    }

    #[test]
    fn test_detect_deno_latest() {
        let spec = detect_toolchain("deno:latest").unwrap();
        assert_eq!(spec.language, "deno");
        assert_eq!(spec.version, "latest");
    }

    #[test]
    fn test_detect_deno_no_tag() {
        let spec = detect_toolchain("deno").unwrap();
        assert_eq!(spec.language, "deno");
        assert_eq!(spec.version, "latest");
    }

    #[tokio::test]
    async fn test_resolve_deno_exact_url() {
        // Exact version should construct the URL directly without API calls.
        let (version, url) = resolve_deno("2.1.4", "arm64").await.unwrap();
        assert_eq!(version, "2.1.4");
        assert_eq!(
            url,
            "https://github.com/denoland/deno/releases/download/v2.1.4/deno-aarch64-apple-darwin.zip"
        );
    }

    #[tokio::test]
    async fn test_resolve_deno_exact_url_amd64() {
        let (version, url) = resolve_deno("1.46.3", "amd64").await.unwrap();
        assert_eq!(version, "1.46.3");
        assert_eq!(
            url,
            "https://github.com/denoland/deno/releases/download/v1.46.3/deno-x86_64-apple-darwin.zip"
        );
    }

    #[test]
    fn test_bun_spec() {
        let spec = ToolchainSpec::bun("1.2.3");
        assert_eq!(spec.language, "bun");
        assert_eq!(spec.version, "1.2.3");
        assert_eq!(spec.install_dir, "/usr/local/bun");
        assert!(spec.path_dirs.contains(&"/usr/local/bun/bin".to_string()));
        assert!(spec.env.is_empty());
    }

    #[test]
    fn test_detect_bun() {
        let spec = detect_toolchain("bun:1.2.3").unwrap();
        assert_eq!(spec.language, "bun");
        assert_eq!(spec.version, "1.2.3");
    }

    #[test]
    fn test_detect_bun_partial() {
        let spec = detect_toolchain("bun:1-alpine").unwrap();
        assert_eq!(spec.language, "bun");
        assert_eq!(spec.version, "1");
    }

    #[test]
    fn test_detect_bun_latest() {
        let spec = detect_toolchain("bun:latest").unwrap();
        assert_eq!(spec.language, "bun");
        assert_eq!(spec.version, "latest");
    }

    #[test]
    fn test_detect_bun_no_tag() {
        let spec = detect_toolchain("bun").unwrap();
        assert_eq!(spec.language, "bun");
        assert_eq!(spec.version, "latest");
    }

    #[tokio::test]
    async fn test_resolve_bun_exact_url() {
        // Exact version should construct the URL directly without API calls.
        let (version, url) = resolve_bun("1.2.3", "arm64").await.unwrap();
        assert_eq!(version, "1.2.3");
        assert_eq!(
            url,
            "https://github.com/oven-sh/bun/releases/download/bun-v1.2.3/bun-darwin-aarch64.zip"
        );
    }

    #[tokio::test]
    async fn test_resolve_bun_exact_url_amd64() {
        let (version, url) = resolve_bun("1.1.0", "amd64").await.unwrap();
        assert_eq!(version, "1.1.0");
        assert_eq!(
            url,
            "https://github.com/oven-sh/bun/releases/download/bun-v1.1.0/bun-darwin-x64.zip"
        );
    }

    #[test]
    fn test_zig_spec() {
        let spec = ToolchainSpec::zig("0.14.0");
        assert_eq!(spec.language, "zig");
        assert_eq!(spec.version, "0.14.0");
        assert_eq!(spec.install_dir, "/usr/local/zig");
        assert!(spec.path_dirs.contains(&"/usr/local/zig".to_string()));
        assert!(spec.env.is_empty());
    }

    #[test]
    fn test_detect_zig() {
        let spec = detect_toolchain("zig:0.14.0").unwrap();
        assert_eq!(spec.language, "zig");
        assert_eq!(spec.version, "0.14.0");
    }

    #[test]
    fn test_detect_zig_partial() {
        let spec = detect_toolchain("zig:0.14-alpine").unwrap();
        assert_eq!(spec.language, "zig");
        assert_eq!(spec.version, "0.14");
    }

    #[test]
    fn test_detect_zig_latest() {
        let spec = detect_toolchain("zig:latest").unwrap();
        assert_eq!(spec.language, "zig");
        assert_eq!(spec.version, "latest");
    }

    #[test]
    fn test_detect_zig_no_tag() {
        let spec = detect_toolchain("zig").unwrap();
        assert_eq!(spec.language, "zig");
        assert_eq!(spec.version, "latest");
    }

    #[test]
    fn test_compare_version_strings() {
        use std::cmp::Ordering;
        assert_eq!(
            compare_version_strings("0.14.0", "0.13.0"),
            Ordering::Greater
        );
        assert_eq!(compare_version_strings("0.13.0", "0.14.0"), Ordering::Less);
        assert_eq!(compare_version_strings("0.14.0", "0.14.0"), Ordering::Equal);
        assert_eq!(
            compare_version_strings("1.0.0", "0.14.0"),
            Ordering::Greater
        );
        assert_eq!(
            compare_version_strings("0.14.1", "0.14.0"),
            Ordering::Greater
        );
    }

    #[test]
    fn test_java_spec() {
        let spec = ToolchainSpec::java("21");
        assert_eq!(spec.language, "java");
        assert_eq!(spec.version, "21");
        assert_eq!(spec.install_dir, "/usr/local/java");
        assert!(spec.path_dirs.contains(&"/usr/local/java/bin".to_string()));
        assert_eq!(
            spec.env.get("JAVA_HOME"),
            Some(&"/usr/local/java".to_string())
        );
    }

    #[test]
    fn test_detect_eclipse_temurin() {
        let spec = detect_toolchain("eclipse-temurin:21-jdk").unwrap();
        assert_eq!(spec.language, "java");
        assert_eq!(spec.version, "21");
    }

    #[test]
    fn test_detect_amazoncorretto() {
        let spec = detect_toolchain("amazoncorretto:17").unwrap();
        assert_eq!(spec.language, "java");
        assert_eq!(spec.version, "17");
    }

    #[test]
    fn test_detect_openjdk() {
        let spec = detect_toolchain("openjdk:21-slim").unwrap();
        assert_eq!(spec.language, "java");
        assert_eq!(spec.version, "21");
    }

    #[test]
    fn test_detect_openjdk_latest() {
        let spec = detect_toolchain("openjdk:latest").unwrap();
        assert_eq!(spec.language, "java");
        assert_eq!(spec.version, "latest");
    }

    #[test]
    fn test_detect_openjdk_no_tag() {
        let spec = detect_toolchain("openjdk").unwrap();
        assert_eq!(spec.language, "java");
        assert_eq!(spec.version, "latest");
    }

    #[tokio::test]
    async fn test_resolve_java_exact_url() {
        // Exact feature version should construct the URL directly.
        let (version, url) = resolve_java("21", "arm64").await.unwrap();
        assert_eq!(version, "21");
        assert_eq!(
            url,
            "https://api.adoptium.net/v3/binary/latest/21/ga/mac/aarch64/jdk/hotspot/normal/eclipse"
        );
    }

    #[tokio::test]
    async fn test_resolve_java_exact_url_amd64() {
        let (version, url) = resolve_java("17", "amd64").await.unwrap();
        assert_eq!(version, "17");
        assert_eq!(
            url,
            "https://api.adoptium.net/v3/binary/latest/17/ga/mac/x64/jdk/hotspot/normal/eclipse"
        );
    }

    #[tokio::test]
    async fn test_resolve_java_dotted_version_strips_to_major() {
        // A dotted version like "21.0.5" should strip to feature version "21".
        let (version, url) = resolve_java("21.0.5", "arm64").await.unwrap();
        assert_eq!(version, "21");
        assert!(url.contains("/21/ga/mac/aarch64/"));
    }

    #[test]
    fn test_swift_spec() {
        let spec = ToolchainSpec::swift("6.1");
        assert_eq!(spec.language, "swift");
        assert_eq!(spec.version, "6.1");
        assert_eq!(spec.install_dir, "/usr/local/swift");
        assert!(spec
            .path_dirs
            .contains(&"/usr/local/swift/usr/bin".to_string()));
        assert!(spec.env.is_empty());
    }

    #[test]
    fn test_detect_swift() {
        let spec = detect_toolchain("swift:6.1").unwrap();
        assert_eq!(spec.language, "swift");
        assert_eq!(spec.version, "6.1");
    }

    #[test]
    fn test_detect_swift_exact() {
        let spec = detect_toolchain("swift:6.1.2").unwrap();
        assert_eq!(spec.language, "swift");
        assert_eq!(spec.version, "6.1.2");
    }

    #[test]
    fn test_detect_swift_latest() {
        let spec = detect_toolchain("swift:latest").unwrap();
        assert_eq!(spec.language, "swift");
        assert_eq!(spec.version, "latest");
    }

    #[test]
    fn test_detect_swift_no_tag() {
        let spec = detect_toolchain("swift").unwrap();
        assert_eq!(spec.language, "swift");
        assert_eq!(spec.version, "latest");
    }

    #[test]
    fn test_detect_swift_with_suffix() {
        let spec = detect_toolchain("swift:6.0-slim").unwrap();
        assert_eq!(spec.language, "swift");
        assert_eq!(spec.version, "6.0");
    }

    #[test]
    fn test_detect_swift_registry_qualified() {
        let spec = detect_toolchain("docker.io/library/swift:6.1").unwrap();
        assert_eq!(spec.language, "swift");
        assert_eq!(spec.version, "6.1");
    }

    #[test]
    fn test_extract_swift_version_standard() {
        let output =
            "Apple Swift version 6.1 (swiftlang-6.1.0.110.21 clang-1700.0.13.3)\nTarget: arm64-apple-macosx15.0\n";
        assert_eq!(extract_swift_version(output), "6.1");
    }

    #[test]
    fn test_extract_swift_version_patch() {
        let output =
            "Apple Swift version 5.10.1 (swift-5.10.1-RELEASE)\nTarget: x86_64-apple-macosx14.0\n";
        assert_eq!(extract_swift_version(output), "5.10.1");
    }

    #[test]
    fn test_extract_swift_version_unknown() {
        assert_eq!(extract_swift_version("not a version string"), "unknown");
        assert_eq!(extract_swift_version(""), "unknown");
    }

    #[test]
    fn test_graalvm_spec() {
        let spec = ToolchainSpec::graalvm("21.0.5");
        assert_eq!(spec.language, "graalvm");
        assert_eq!(spec.version, "21.0.5");
        assert_eq!(spec.install_dir, "/usr/local/graalvm");
        assert!(spec
            .path_dirs
            .contains(&"/usr/local/graalvm/bin".to_string()));
        assert_eq!(
            spec.env.get("JAVA_HOME"),
            Some(&"/usr/local/graalvm".to_string())
        );
        assert_eq!(
            spec.env.get("GRAALVM_HOME"),
            Some(&"/usr/local/graalvm".to_string())
        );
    }

    #[test]
    fn test_detect_graalvm() {
        let spec = detect_toolchain("graalvm/graalvm-ce:21.0.5").unwrap();
        assert_eq!(spec.language, "graalvm");
        assert_eq!(spec.version, "21.0.5");
    }

    #[test]
    fn test_detect_graalvm_ce() {
        let spec = detect_toolchain("graalvm-ce:17").unwrap();
        assert_eq!(spec.language, "graalvm");
        assert_eq!(spec.version, "17");
    }

    #[test]
    fn test_detect_graalvm_ghcr() {
        // ghcr.io/graalvm/jdk → base_name = "jdk" which does NOT contain "graalvm"
        // but ghcr.io/graalvm/native-image → base_name = "native-image" — also no match.
        // The match is on the base_name portion, so "graalvm-ce" or "graalvm" matches.
        let spec = detect_toolchain("ghcr.io/graalvm/graalvm-community:21").unwrap();
        assert_eq!(spec.language, "graalvm");
        assert_eq!(spec.version, "21");
    }

    #[test]
    fn test_detect_graalvm_latest() {
        let spec = detect_toolchain("graalvm-ce:latest").unwrap();
        assert_eq!(spec.language, "graalvm");
        assert_eq!(spec.version, "latest");
    }

    #[test]
    fn test_detect_graalvm_no_tag() {
        let spec = detect_toolchain("graalvm-ce").unwrap();
        assert_eq!(spec.language, "graalvm");
        assert_eq!(spec.version, "latest");
    }

    #[test]
    fn test_detect_graalvm_with_suffix() {
        let spec = detect_toolchain("graalvm-ce:21-slim").unwrap();
        assert_eq!(spec.language, "graalvm");
        assert_eq!(spec.version, "21");
    }

    #[tokio::test]
    async fn test_resolve_graalvm_exact_url() {
        // Exact version should construct the URL directly without API calls.
        let (version, url) = resolve_graalvm("21.0.5", "arm64").await.unwrap();
        assert_eq!(version, "21.0.5");
        assert_eq!(
            url,
            "https://github.com/graalvm/graalvm-ce-builds/releases/download/\
             jdk-21.0.5/graalvm-community-jdk-21.0.5_macos-aarch64_bin.tar.gz"
        );
    }

    #[tokio::test]
    async fn test_resolve_graalvm_exact_url_amd64() {
        let (version, url) = resolve_graalvm("17.0.12", "amd64").await.unwrap();
        assert_eq!(version, "17.0.12");
        assert_eq!(
            url,
            "https://github.com/graalvm/graalvm-ce-builds/releases/download/\
             jdk-17.0.12/graalvm-community-jdk-17.0.12_macos-x64_bin.tar.gz"
        );
    }
}
