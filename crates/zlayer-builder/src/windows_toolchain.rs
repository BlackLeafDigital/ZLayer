//! Windows toolchain provisioning for sandbox builds.
//!
//! Instead of translating Linux package manager commands to a Windows package
//! manager (which mutates global state and breaks parallel builds), this module
//! provisions language toolchains directly into a build's rootfs.
//!
//! Each major language has a version listing API and self-contained Windows
//! binaries. The provisioner resolves a version to a download URL, downloads the
//! archive, and extracts it into the rootfs — all scoped to the build.
//!
//! This is the Windows mirror of the macOS provisioner (`macos_toolchain`).
//! Scope is limited to the toolchains that ship native Windows builds we can
//! provision unattended: `go`, `node`, `rust`, `python`, `deno`, and `bun`.
//! Swift/Zig/Java/`GraalVM` are intentionally omitted here.
//!
//! This module is only compiled on Windows (`#[cfg(target_os = "windows")]`).

#![cfg(target_os = "windows")]

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use tracing::{debug, info};

use std::str::FromStr;
use zlayer_types::ImageReference;

use crate::error::{BuildError, Result};

// ---------------------------------------------------------------------------
// ToolchainSpec
// ---------------------------------------------------------------------------

/// Specification for a language toolchain to provision into a build rootfs.
///
/// Install dirs and `PATH` entries use Windows-style absolute paths under
/// `C:\toolchains\<lang>`. The sandbox builder prefixes these with the build
/// rootfs at execution time.
#[derive(Debug, Clone)]
pub struct ToolchainSpec {
    /// Language identifier (e.g., `"go"`, `"node"`, `"rust"`).
    pub language: String,
    /// Requested version (e.g., `"1.23"`, `"20"`, `"latest"`).
    pub version: String,
    /// Directory inside rootfs where the toolchain is installed.
    pub install_dir: String,
    /// Directories to add to `PATH` (relative to rootfs root).
    pub path_dirs: Vec<String>,
    /// Extra environment variables to set.
    pub env: HashMap<String, String>,
}

impl ToolchainSpec {
    /// Create a Go toolchain spec.
    #[must_use]
    pub fn go(version: &str) -> Self {
        // GOROOT is set relative to the rootfs — the sandbox builder will
        // prefix it with the rootfs path at execution time. The Windows Go
        // archive nests everything under `go\`, with `go.exe` at `go\bin`.
        let mut env = HashMap::new();
        env.insert("GOROOT".to_string(), "C:\\toolchains\\go".to_string());
        // Disable VCS stamping — git in the sandbox may be unavailable.
        env.insert("GOFLAGS".to_string(), "-buildvcs=false".to_string());
        Self {
            language: "go".to_string(),
            version: version.to_string(),
            install_dir: "C:\\toolchains\\go".to_string(),
            path_dirs: vec!["C:\\toolchains\\go\\bin".to_string()],
            env,
        }
    }

    /// Create a Node.js toolchain spec.
    ///
    /// On Windows the Node archive places `node.exe` and `npm` at the package
    /// root (not in a `bin` subdirectory), so the `PATH` entry is the install
    /// directory itself.
    #[must_use]
    pub fn node(version: &str) -> Self {
        Self {
            language: "node".to_string(),
            version: version.to_string(),
            install_dir: "C:\\toolchains\\node".to_string(),
            path_dirs: vec!["C:\\toolchains\\node".to_string()],
            env: HashMap::new(),
        }
    }

    /// Create a Rust toolchain spec.
    ///
    /// Rust is installed via `rustup-init.exe` with `CARGO_HOME` and
    /// `RUSTUP_HOME` pointed at the cache, so `cargo.exe`/`rustc.exe` land in
    /// `C:\toolchains\cargo\bin`.
    #[must_use]
    pub fn rust(version: &str) -> Self {
        let mut env = HashMap::new();
        env.insert(
            "CARGO_HOME".to_string(),
            "C:\\toolchains\\cargo".to_string(),
        );
        env.insert(
            "RUSTUP_HOME".to_string(),
            "C:\\toolchains\\rustup".to_string(),
        );
        Self {
            language: "rust".to_string(),
            version: version.to_string(),
            install_dir: "C:\\toolchains\\rust".to_string(),
            path_dirs: vec!["C:\\toolchains\\cargo\\bin".to_string()],
            env,
        }
    }

    /// Create a Python toolchain spec.
    ///
    /// The `python-build-standalone` Windows archive places `python.exe` at the
    /// package root and console scripts under `Scripts`.
    #[must_use]
    pub fn python(version: &str) -> Self {
        Self {
            language: "python".to_string(),
            version: version.to_string(),
            install_dir: "C:\\toolchains\\python".to_string(),
            path_dirs: vec![
                "C:\\toolchains\\python".to_string(),
                "C:\\toolchains\\python\\Scripts".to_string(),
            ],
            env: HashMap::new(),
        }
    }

    /// Create a Deno toolchain spec.
    ///
    /// The Deno Windows zip contains a single `deno.exe` placed at the package
    /// root.
    #[must_use]
    pub fn deno(version: &str) -> Self {
        Self {
            language: "deno".to_string(),
            version: version.to_string(),
            install_dir: "C:\\toolchains\\deno".to_string(),
            path_dirs: vec!["C:\\toolchains\\deno".to_string()],
            env: HashMap::new(),
        }
    }

    /// Create a Bun toolchain spec.
    ///
    /// The Bun Windows zip contains a single `bun.exe` placed at the package
    /// root.
    #[must_use]
    pub fn bun(version: &str) -> Self {
        Self {
            language: "bun".to_string(),
            version: version.to_string(),
            install_dir: "C:\\toolchains\\bun".to_string(),
            path_dirs: vec!["C:\\toolchains\\bun".to_string()],
            env: HashMap::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Toolchain detection from image reference
// ---------------------------------------------------------------------------

/// Detect the language toolchain from a Docker/OCI base image reference.
///
/// Parses image names like `golang:1.23-alpine` or `node:20-slim` to determine
/// the language and version. Returns `None` for base images and toolchains that
/// have no Windows provisioner (swift/zig/java/graalvm).
#[must_use]
pub fn detect_toolchain(image_ref: &str) -> Option<ToolchainSpec> {
    let (name, tag) = match ImageReference::from_str(image_ref) {
        Ok(r) => (
            r.repository().to_string(),
            r.tag().unwrap_or("latest").to_string(),
        ),
        Err(_) => {
            // Inputs that aren't valid OCI refs (e.g. bare stage names) — fall back to the raw input.
            (image_ref.to_string(), "latest".to_string())
        }
    };

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
        // Base images and toolchains without a Windows provisioner — no toolchain.
        _ => None,
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
pub(crate) fn extract_version_from_tag(tag: &str) -> String {
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

/// Get the Windows host architecture as used in download URLs.
///
/// Windows build runners are amd64, so this returns `"x86_64"`. The `aarch64`
/// arm is mapped for completeness on Windows-on-ARM hosts.
#[must_use]
pub fn host_arch() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "x86_64"
    }
}

// ---------------------------------------------------------------------------
// Version resolution + download
// ---------------------------------------------------------------------------

/// Resolve a toolchain version to a download URL and provision it into rootfs.
///
/// The toolchain is downloaded and extracted into the cache directory
/// (`{cache_dir}/{lang}-{version}-{arch}/`). The build rootfs receives a copy
/// (or junction, see [`link_toolchain_into_rootfs`]) of the cached toolchain.
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
    let arch = host_arch();
    let cache_key = format!("{}-{}-{}", spec.language, spec.version, arch);
    let cached_toolchain = cache_dir.join(&cache_key);

    // Check cache
    if cached_toolchain.exists() {
        info!(
            "Linking cached {} {} toolchain from {}",
            spec.language,
            spec.version,
            cached_toolchain.display()
        );
        link_toolchain_into_rootfs(&cached_toolchain, rootfs_dir, spec).await?;
        return Ok(());
    }

    // Resolve version → download URL
    let (resolved_version, url) = resolve_download_url(spec, arch).await?;

    info!(
        "Downloading {} {} (resolved: {}) from {}",
        spec.language, spec.version, resolved_version, url
    );

    // Rust uses rustup-init.exe rather than an archive — handle it specially.
    if spec.language == "rust" {
        tokio::fs::create_dir_all(&cached_toolchain).await?;
        provision_rust_via_rustup(&resolved_version, &url, &cached_toolchain, tmp_dir).await?;
        link_toolchain_into_rootfs(&cached_toolchain, rootfs_dir, spec).await?;
        info!(
            "Provisioned {} {} into rootfs",
            spec.language, resolved_version
        );
        return Ok(());
    }

    // Choose the download extension based on the archive type.
    let ext = if spec.language == "python" {
        "tar.gz"
    } else {
        // go / node / deno / bun on Windows are all .zip
        "zip"
    };
    let download_path = tmp_dir.join(format!("toolchain-{cache_key}.{ext}"));
    download_file(&url, &download_path).await?;

    // Extract to cache
    tokio::fs::create_dir_all(&cached_toolchain).await?;
    extract_toolchain(&spec.language, &download_path, &cached_toolchain).await?;

    // Place cache into rootfs
    link_toolchain_into_rootfs(&cached_toolchain, rootfs_dir, spec).await?;

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
        other => Err(BuildError::RegistryError {
            message: format!(
                "No Windows toolchain provisioner for '{other}'. \
                 Supported: go, node, rust, python, deno, bun. \
                 Use a pre-built zlayer/ base image or specify a toolchain URL."
            ),
        }),
    }
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
async fn resolve_go(version: &str, _arch: &str) -> Result<(String, String)> {
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

    // Windows Go ships as a .zip for amd64.
    let url = format!("https://go.dev/dl/go{resolved}.windows-amd64.zip");
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
async fn resolve_node(version: &str, _arch: &str) -> Result<(String, String)> {
    let resolved = if version == "latest" || !version.contains('.') {
        resolve_node_version_from_api(version).await?
    } else {
        version.to_string()
    };

    // Windows Node ships as a .zip for win-x64.
    let url = format!("https://nodejs.org/dist/v{resolved}/node-v{resolved}-win-x64.zip");
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

/// Resolve a Rust version to a `rustup-init.exe` download URL.
///
/// On Windows we do not download a standalone tarball; instead we fetch
/// `rustup-init.exe` and let it install the requested toolchain into the cache.
/// The returned URL is always the `x86_64-pc-windows-msvc` `rustup-init.exe`
/// (the bootstrapper is arch-specific but the toolchain it installs is selected
/// by `--default-toolchain`).
///
/// Supports:
/// - Exact: `"1.82.0"` → installed as-is
/// - Partial: `"1.82"` → appends `.0` (Rust always has a `.0` patch release)
/// - Latest: `"latest"` → fetches the stable channel TOML and extracts version
async fn resolve_rust(version: &str, _arch: &str) -> Result<(String, String)> {
    let resolved = if version == "latest" {
        resolve_rust_latest_version().await?
    } else if version.matches('.').count() < 2 {
        // Partial version like "1.82" — Rust always releases x.y.0 for each
        // minor version, so appending ".0" is safe.
        format!("{version}.0")
    } else {
        version.to_string()
    };

    let url = "https://static.rust-lang.org/rustup/dist/x86_64-pc-windows-msvc/rustup-init.exe"
        .to_string();
    Ok((resolved, url))
}

/// Fetch the Rust stable channel TOML and extract the current stable version.
///
/// The TOML contains a `[pkg.rust]` section with a `version = "1.XX.Y (hash date)"`
/// line. We extract just the semver portion with a simple scan rather than
/// pulling in a TOML parser.
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

/// Download `rustup-init.exe` and run it to install a Rust toolchain into the
/// cache directory.
///
/// `CARGO_HOME` and `RUSTUP_HOME` are pointed at the cache so that the resulting
/// `cargo.exe`/`rustc.exe` land under `{cached}\cargo\bin`. The installer runs
/// non-interactively with the minimal profile and does not touch the host
/// `PATH`.
async fn provision_rust_via_rustup(
    version: &str,
    url: &str,
    cached: &Path,
    tmp_dir: &Path,
) -> Result<()> {
    let init_path = tmp_dir.join("rustup-init.exe");
    download_file(url, &init_path).await?;

    let cargo_home = cached.join("cargo");
    let rustup_home = cached.join("rustup");
    tokio::fs::create_dir_all(&cargo_home).await?;
    tokio::fs::create_dir_all(&rustup_home).await?;

    let output = tokio::process::Command::new(&init_path)
        .args([
            "-y",
            "--default-toolchain",
            version,
            "--profile",
            "minimal",
            "--no-modify-path",
        ])
        .env("CARGO_HOME", &cargo_home)
        .env("RUSTUP_HOME", &rustup_home)
        .output()
        .await
        .map_err(|e| BuildError::RegistryError {
            message: format!("Failed to run rustup-init.exe: {e}"),
        })?;

    // Clean up the installer regardless of outcome.
    let _ = tokio::fs::remove_file(&init_path).await;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(BuildError::RegistryError {
            message: format!("rustup-init.exe failed installing {version}: {stderr}"),
        });
    }

    Ok(())
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
/// Asset naming pattern (Windows):
/// `cpython-{version}+{timestamp}-x86_64-pc-windows-msvc-install_only.tar.gz`
async fn resolve_python(version: &str, _arch: &str) -> Result<(String, String)> {
    // Windows standalone Python is published for the MSVC target. We prefer the
    // plain `install_only` asset (Windows builds have no `_stripped` variant).
    let python_target = "x86_64-pc-windows-msvc";
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
    let target_suffix = format!("{target}-install_only.tar.gz");

    // For "latest", find the first asset matching our target across all releases.
    if version_prefix == "latest" {
        for release in &releases {
            for asset in &release.assets {
                if asset.name.starts_with("cpython-") && asset.name.ends_with(&target_suffix) {
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
/// `cpython-3.12.8+20250106-x86_64-pc-windows-msvc-install_only.tar.gz`.
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
    /// need it (the Python resolver uses asset names instead).
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
/// Each release asset is a zip containing a single `deno.exe` binary.
///
/// Supports:
/// - Exact: `"2.1.4"` → constructs URL directly from tag `v2.1.4`
/// - Partial: `"2"` → scans GitHub releases for the latest `v2.x.y` tag
/// - Latest: `"latest"` → fetches the most recent release
///
/// Asset naming: `deno-x86_64-pc-windows-msvc.zip`.
async fn resolve_deno(version: &str, _arch: &str) -> Result<(String, String)> {
    let deno_target = "x86_64-pc-windows-msvc";

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
/// Each release asset is a zip containing a directory with a single `bun.exe`.
///
/// Supports:
/// - Exact: `"1.2.3"` → constructs URL directly from tag `bun-v1.2.3`
/// - Partial: `"1"` → scans GitHub releases for the latest `bun-v1.x.y` tag
/// - Latest: `"latest"` → fetches the most recent release
///
/// Asset naming: `bun-windows-x64.zip`, which extracts as
/// `bun-windows-x64/bun.exe`.
async fn resolve_bun(version: &str, _arch: &str) -> Result<(String, String)> {
    let bun_target = "windows-x64";

    if version == "latest" || !version.contains('.') {
        // "latest" or partial like "1" — scan releases API.
        resolve_bun_from_github(version, bun_target).await
    } else {
        // Exact version like "1.2.3" — construct the URL directly.
        let url = format!(
            "https://github.com/oven-sh/bun/releases/download/bun-v{version}/bun-{bun_target}.zip"
        );
        Ok((version.to_string(), url))
    }
}

/// Fetch the GitHub releases API for `oven-sh/bun` and find a matching
/// release for the requested version.
async fn resolve_bun_from_github(
    version_prefix: &str,
    bun_target: &str,
) -> Result<(String, String)> {
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

    let asset_name = format!("bun-{bun_target}.zip");

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
                "No Bun release found for target '{bun_target}' in recent GitHub releases"
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
///
/// Uses pure-Rust extraction (the `zip` crate for `.zip`, `tar` + `flate2` for
/// `.tar.gz`) rather than shelling out, since the build host cross-compiles for
/// Windows and cannot rely on `tar.exe`/`unzip.exe` being present. Where the
/// archive nests everything under a single top-level directory (Go's `go\`,
/// Node's `node-v...\`, Bun's `bun-windows-x64\`), that directory is stripped so
/// the toolchain contents land directly in `target_dir`.
async fn extract_toolchain(language: &str, archive: &Path, target_dir: &Path) -> Result<()> {
    let archive = archive.to_path_buf();
    let target_dir = target_dir.to_path_buf();
    let language = language.to_string();

    // Extraction is blocking (synchronous I/O), so run it on a blocking thread.
    tokio::task::spawn_blocking(move || {
        extract_toolchain_blocking(&language, &archive, &target_dir)
    })
    .await
    .map_err(|e| BuildError::RegistryError {
        message: format!("Toolchain extraction task panicked: {e}"),
    })?
}

/// Synchronous extraction worker. See [`extract_toolchain`].
fn extract_toolchain_blocking(language: &str, archive: &Path, target_dir: &Path) -> Result<()> {
    match language {
        // Python ships as `install_only.tar.gz`, nesting everything under
        // `python/`. Strip that leading component so `python.exe` lands at the
        // package root.
        "python" => extract_tar_gz(archive, target_dir, 1),
        // Go (`go/`), Node (`node-v.../`), and Bun (`bun-windows-x64/`) nest a
        // single top-level dir — strip it. Deno is a flat zip with `deno.exe`
        // at the root, so nothing is stripped.
        "go" | "node" | "bun" => extract_zip(archive, target_dir, 1),
        "deno" => extract_zip(archive, target_dir, 0),
        other => Err(BuildError::RegistryError {
            message: format!("No extraction strategy for toolchain '{other}'"),
        }),
    }
}

/// Extract a `.zip` archive into `target_dir`, stripping `strip_components`
/// leading path components from each entry.
fn extract_zip(archive: &Path, target_dir: &Path, strip_components: usize) -> Result<()> {
    let file = std::fs::File::open(archive).map_err(|e| {
        BuildError::IoError(std::io::Error::new(
            e.kind(),
            format!("failed to open zip archive {}: {e}", archive.display()),
        ))
    })?;

    let mut zip = zip::ZipArchive::new(file).map_err(|e| {
        BuildError::IoError(std::io::Error::other(format!(
            "failed to read zip archive: {e}"
        )))
    })?;

    std::fs::create_dir_all(target_dir)?;

    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| {
            BuildError::IoError(std::io::Error::other(format!(
                "failed to read zip entry {i}: {e}"
            )))
        })?;

        let Some(enclosed) = entry.enclosed_name() else {
            debug!("Skipping potentially unsafe zip entry");
            continue;
        };
        let Some(stripped) = strip_leading_components(&enclosed, strip_components) else {
            continue;
        };
        let out_path = target_dir.join(stripped);

        if entry.is_dir() {
            std::fs::create_dir_all(&out_path)?;
        } else {
            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut out_file = std::fs::File::create(&out_path).map_err(|e| {
                BuildError::IoError(std::io::Error::new(
                    e.kind(),
                    format!("failed to create file {}: {e}", out_path.display()),
                ))
            })?;
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf).map_err(|e| {
                BuildError::IoError(std::io::Error::new(
                    e.kind(),
                    format!("failed to read zip entry: {e}"),
                ))
            })?;
            std::io::Write::write_all(&mut out_file, &buf)?;
        }
    }

    Ok(())
}

/// Extract a `.tar.gz` archive into `target_dir`, stripping `strip_components`
/// leading path components from each entry.
fn extract_tar_gz(archive: &Path, target_dir: &Path, strip_components: usize) -> Result<()> {
    let file = std::fs::File::open(archive).map_err(|e| {
        BuildError::IoError(std::io::Error::new(
            e.kind(),
            format!("failed to open tarball {}: {e}", archive.display()),
        ))
    })?;

    std::fs::create_dir_all(target_dir)?;

    let decoder = flate2::read::GzDecoder::new(file);
    let mut tar = tar::Archive::new(decoder);

    let entries = tar.entries().map_err(|e| {
        BuildError::IoError(std::io::Error::other(format!(
            "failed to read tar entries: {e}"
        )))
    })?;

    for entry in entries {
        let mut entry = entry.map_err(|e| {
            BuildError::IoError(std::io::Error::other(format!(
                "failed to read tar entry: {e}"
            )))
        })?;

        let path = entry
            .path()
            .map_err(|e| {
                BuildError::IoError(std::io::Error::other(format!(
                    "failed to read tar entry path: {e}"
                )))
            })?
            .into_owned();

        let Some(stripped) = strip_leading_components(&path, strip_components) else {
            continue;
        };
        let out_path = target_dir.join(stripped);

        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        entry.unpack(&out_path).map_err(|e| {
            BuildError::IoError(std::io::Error::new(
                e.kind(),
                format!("failed to unpack tar entry to {}: {e}", out_path.display()),
            ))
        })?;
    }

    Ok(())
}

/// Strip the first `n` path components from `path`.
///
/// Returns `None` if the entry is exactly the stripped prefix (i.e. nothing
/// remains after stripping), so callers can skip the now-empty entry.
fn strip_leading_components(path: &Path, n: usize) -> Option<PathBuf> {
    if n == 0 {
        return Some(path.to_path_buf());
    }
    let mut comps = path.components();
    for _ in 0..n {
        comps.next()?;
    }
    let rest: PathBuf = comps.as_path().to_path_buf();
    if rest.as_os_str().is_empty() {
        None
    } else {
        Some(rest)
    }
}

/// Place a cached toolchain into the build rootfs at the spec's `install_dir`.
///
/// On Windows there is no equivalent to a Unix symlink that works without
/// elevated privileges for arbitrary callers, so this prefers a directory
/// **junction** (`mklink /J`, no privilege required) and falls back to a
/// recursive **copy** if the junction cannot be created (e.g. cross-volume).
/// The cache remains the immutable source of truth; the rootfs entry is a
/// junction/copy pointing at it.
async fn link_toolchain_into_rootfs(
    cached: &Path,
    rootfs_dir: &Path,
    spec: &ToolchainSpec,
) -> Result<()> {
    // The install_dir is a Windows absolute path like `C:\toolchains\go`. Strip
    // the drive prefix so it nests under the rootfs.
    let rel = strip_windows_drive(&spec.install_dir);
    let install_target = rootfs_dir.join(rel);

    // Ensure parent directory exists.
    if let Some(parent) = install_target.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    // Remove existing target if present.
    if install_target.exists() {
        let _ = tokio::fs::remove_dir_all(&install_target).await;
    }

    // Try a directory junction first — it is cheap and needs no elevation.
    let junction_ok = tokio::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(install_target.as_os_str())
        .arg(cached.as_os_str())
        .output()
        .await
        .is_ok_and(|o| o.status.success());

    if junction_ok {
        debug!(
            "Junctioned toolchain {} → {}",
            install_target.display(),
            cached.display()
        );
        return Ok(());
    }

    // Junction failed (cross-volume, sandbox without cmd, etc.) — recursively
    // copy the cached toolchain into the rootfs instead.
    debug!(
        "Junction unavailable; copying toolchain {} → {}",
        cached.display(),
        install_target.display()
    );
    copy_dir_recursive(cached, &install_target).await
}

/// Strip a leading Windows drive prefix (e.g. `C:\`) from an absolute path,
/// returning a relative path suitable for nesting under a rootfs.
fn strip_windows_drive(path: &str) -> String {
    // Normalize separators, then drop a leading `X:` and any following slashes.
    let trimmed = path.trim_start_matches(|c: char| c.is_ascii_alphabetic());
    let trimmed = trimmed.strip_prefix(':').unwrap_or(trimmed);
    trimmed.trim_start_matches(['\\', '/']).replace('\\', "/")
}

/// Recursively copy a directory tree from `src` to `dst`.
async fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    let src = src.to_path_buf();
    let dst = dst.to_path_buf();
    tokio::task::spawn_blocking(move || copy_dir_recursive_blocking(&src, &dst))
        .await
        .map_err(|e| BuildError::RegistryError {
            message: format!("Toolchain copy task panicked: {e}"),
        })?
}

/// Synchronous recursive directory copy. See [`copy_dir_recursive`].
fn copy_dir_recursive_blocking(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive_blocking(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)?;
        }
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
        assert_eq!(spec.install_dir, "C:\\toolchains\\go");
        assert_eq!(
            spec.env.get("GOROOT"),
            Some(&"C:\\toolchains\\go".to_string())
        );
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
        // node.exe is at the package root on Windows — PATH dir is install_dir.
        assert!(spec.path_dirs.contains(&"C:\\toolchains\\node".to_string()));
    }

    #[test]
    fn test_detect_python() {
        let spec = detect_toolchain("python:3.12-bookworm").unwrap();
        assert_eq!(spec.language, "python");
        assert_eq!(spec.version, "3.12");
        assert!(spec
            .path_dirs
            .contains(&"C:\\toolchains\\python\\Scripts".to_string()));
    }

    #[test]
    fn test_detect_rust() {
        let spec = detect_toolchain("rust:1.82-alpine").unwrap();
        assert_eq!(spec.language, "rust");
        assert_eq!(spec.version, "1.82");
        assert_eq!(
            spec.env.get("CARGO_HOME"),
            Some(&"C:\\toolchains\\cargo".to_string())
        );
        assert_eq!(
            spec.env.get("RUSTUP_HOME"),
            Some(&"C:\\toolchains\\rustup".to_string())
        );
        assert!(spec
            .path_dirs
            .contains(&"C:\\toolchains\\cargo\\bin".to_string()));
    }

    #[test]
    fn test_detect_alpine_no_toolchain() {
        assert!(detect_toolchain("alpine:latest").is_none());
    }

    #[test]
    fn test_detect_swift_omitted_on_windows() {
        // Swift is macOS-only; no Windows provisioner.
        assert!(detect_toolchain("swift:6.1").is_none());
    }

    #[test]
    fn test_detect_java_omitted_on_windows() {
        assert!(detect_toolchain("eclipse-temurin:21-jdk").is_none());
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
    fn test_extract_version_from_tag_latest() {
        assert_eq!(extract_version_from_tag("latest"), "latest");
        assert_eq!(extract_version_from_tag("alpine"), "latest");
    }

    #[test]
    fn test_host_arch_is_x86_64_on_amd64() {
        // On the amd64 Windows runners host_arch must be "x86_64".
        if cfg!(target_arch = "x86_64") {
            assert_eq!(host_arch(), "x86_64");
        }
    }

    #[test]
    fn test_extract_python_version_from_asset_name() {
        assert_eq!(
            extract_python_version_from_asset(
                "cpython-3.12.8+20250106-x86_64-pc-windows-msvc-install_only.tar.gz"
            ),
            "3.12.8"
        );
        assert_eq!(
            extract_python_version_from_asset(
                "cpython-3.11.11+20250106-x86_64-pc-windows-msvc-install_only.tar.gz"
            ),
            "3.11.11"
        );
    }

    #[test]
    fn test_extract_python_version_from_asset_edge_cases() {
        assert_eq!(extract_python_version_from_asset("not-a-cpython-asset"), "");
        assert_eq!(extract_python_version_from_asset("cpython-"), "");
    }

    #[tokio::test]
    async fn test_resolve_go_exact_url() {
        let (version, url) = resolve_go("1.23.6", "x86_64").await.unwrap();
        assert_eq!(version, "1.23.6");
        assert_eq!(url, "https://go.dev/dl/go1.23.6.windows-amd64.zip");
    }

    #[tokio::test]
    async fn test_resolve_node_exact_url() {
        let (version, url) = resolve_node("20.18.1", "x86_64").await.unwrap();
        assert_eq!(version, "20.18.1");
        assert_eq!(
            url,
            "https://nodejs.org/dist/v20.18.1/node-v20.18.1-win-x64.zip"
        );
    }

    #[tokio::test]
    async fn test_resolve_rust_partial_url() {
        // Partial version appends .0; the URL is always the msvc rustup-init.exe.
        let (version, url) = resolve_rust("1.82", "x86_64").await.unwrap();
        assert_eq!(version, "1.82.0");
        assert_eq!(
            url,
            "https://static.rust-lang.org/rustup/dist/x86_64-pc-windows-msvc/rustup-init.exe"
        );
    }

    #[tokio::test]
    async fn test_resolve_deno_exact_url() {
        let (version, url) = resolve_deno("2.1.4", "x86_64").await.unwrap();
        assert_eq!(version, "2.1.4");
        assert_eq!(
            url,
            "https://github.com/denoland/deno/releases/download/v2.1.4/deno-x86_64-pc-windows-msvc.zip"
        );
    }

    #[tokio::test]
    async fn test_resolve_bun_exact_url() {
        let (version, url) = resolve_bun("1.2.3", "x86_64").await.unwrap();
        assert_eq!(version, "1.2.3");
        assert_eq!(
            url,
            "https://github.com/oven-sh/bun/releases/download/bun-v1.2.3/bun-windows-x64.zip"
        );
    }

    #[test]
    fn test_strip_windows_drive() {
        assert_eq!(strip_windows_drive("C:\\toolchains\\go"), "toolchains/go");
        assert_eq!(
            strip_windows_drive("C:\\toolchains\\python\\Scripts"),
            "toolchains/python/Scripts"
        );
    }

    #[test]
    fn test_strip_leading_components() {
        let p = Path::new("go/bin/go.exe");
        assert_eq!(
            strip_leading_components(p, 1),
            Some(PathBuf::from("bin/go.exe"))
        );
        assert_eq!(strip_leading_components(p, 0), Some(PathBuf::from(p)));
        // Stripping the only component leaves nothing.
        assert_eq!(strip_leading_components(Path::new("go"), 1), None);
    }
}
