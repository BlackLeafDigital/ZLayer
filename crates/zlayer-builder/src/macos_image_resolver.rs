//! 3-tier macOS image resolution system
//!
//! On macOS, Linux container images cannot run natively. This module implements
//! a resolution pipeline that rewrites Docker Hub image references to macOS-native
//! equivalents:
//!
//! 1. **GHCR pre-built images**: Check if a pre-built macOS sandbox image exists
//!    at `ghcr.io/blackleafdigital/zlayer/{language}:{version}`.
//! 2. **Local toolchain build**: Install the toolchain directly (Go, Node, Rust,
//!    etc.) via `macos_toolchain` and assemble a sandbox rootfs.
//! 3. **Base image fallback**: For distro images (ubuntu, alpine, etc.) just
//!    create a minimal macOS rootfs with host binaries.
//!
//! Additionally provides Homebrew bottle fetching for installing packages into
//! sandbox rootfs on macOS (replacing `apt-get`/`apk` from Linux Dockerfiles).

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::error::{BuildError, Result};
use crate::macos_toolchain::{
    ensure_base_rootfs, extract_version_from_tag, provision_toolchain, ToolchainSpec,
};
use crate::sandbox_builder::SandboxImageConfig;

/// The `GHCR` registry prefix for pre-built `ZLayer` sandbox images.
const ZLAYER_REGISTRY: &str = "ghcr.io/blackleafdigital/zlayer";

/// Base URL for fetching package mapping files from `RepoSources` (`GitHub Pages`).
const REPO_SOURCES_BASE: &str = "https://zachhandley.github.io/RepoSources/maps";

/// How long a cached package-map file is considered fresh (7 days).
const PACKAGE_MAP_CACHE_TTL_SECS: u64 = 7 * 24 * 3600;

// ---------------------------------------------------------------------------
// Package map types (for RepoSources JSON files)
// ---------------------------------------------------------------------------

/// A package mapping file as published by `RepoSources`.
#[derive(Debug, Deserialize, Serialize)]
struct PackageMapFile {
    metadata: PackageMapMetadata,
    mappings: HashMap<String, String>,
}

/// Metadata header inside a package-map JSON file.
#[derive(Debug, Deserialize, Serialize)]
struct PackageMapMetadata {
    generated_at: String,
    source: String,
    distro: String,
    total_mappings: usize,
}

// ---------------------------------------------------------------------------
// Image rewriting
// ---------------------------------------------------------------------------

/// Rewrite a `Docker Hub` image reference to a `ZLayer` `GHCR` image reference.
///
/// Returns `None` if the image is already a `ZLayer` image, or if the image name
/// is not recognised as a known toolchain or base distro.
///
/// # Examples
///
/// ```ignore
/// assert_eq!(
///     rewrite_image_for_macos("golang:1.23-alpine"),
///     Some("ghcr.io/blackleafdigital/zlayer/golang:1.23".to_string()),
/// );
/// assert_eq!(
///     rewrite_image_for_macos("ubuntu:22.04"),
///     Some("ghcr.io/blackleafdigital/zlayer/base:latest".to_string()),
/// );
/// ```
#[must_use]
pub fn rewrite_image_for_macos(image_ref: &str) -> Option<String> {
    // Don't double-rewrite images that are already from our registry.
    if image_ref.starts_with(ZLAYER_REGISTRY) {
        return None;
    }

    // Strip the registry prefix (docker.io/library/, ghcr.io/foo/, etc.)
    let stripped = strip_registry_prefix(image_ref);

    // Split into name and tag.
    let (name, tag) = split_name_tag(&stripped);
    let base_name = name.rsplit('/').next().unwrap_or(&name);

    // Base distro images → base:latest
    if is_base_distro(base_name) {
        return Some(format!("{ZLAYER_REGISTRY}/base:latest"));
    }

    // Toolchain images → {zlayer_registry}/{canonical}:{version}
    let canonical = match base_name {
        "golang" | "go" => "golang",
        "node" => "node",
        "rust" => "rust",
        "python" | "python3" => "python",
        "deno" => "deno",
        "bun" => "bun",
        "swift" => "swift",
        "zig" => "zig",
        "eclipse-temurin" | "amazoncorretto" | "openjdk" => "java",
        name if name.contains("graalvm") => "graalvm",
        _ => return None,
    };

    let version = extract_version_from_tag(&tag);
    Some(format!("{ZLAYER_REGISTRY}/{canonical}:{version}"))
}

/// Check whether the given base image name is a Linux distribution / base image.
fn is_base_distro(name: &str) -> bool {
    matches!(
        name,
        "ubuntu"
            | "debian"
            | "alpine"
            | "centos"
            | "fedora"
            | "rockylinux"
            | "almalinux"
            | "archlinux"
            | "amazonlinux"
            | "busybox"
    )
}

/// Strip common registry prefixes from an image reference.
fn strip_registry_prefix(image_ref: &str) -> String {
    let prefixes = [
        "docker.io/library/",
        "docker.io/",
        "index.docker.io/library/",
        "index.docker.io/",
    ];
    for prefix in &prefixes {
        if let Some(rest) = image_ref.strip_prefix(prefix) {
            return rest.to_string();
        }
    }
    image_ref.to_string()
}

/// Split an image reference into (name, tag). Defaults tag to "latest".
fn split_name_tag(image_ref: &str) -> (String, String) {
    if let Some((name, tag)) = image_ref.rsplit_once(':') {
        (name.to_string(), tag.to_string())
    } else {
        (image_ref.to_string(), "latest".to_string())
    }
}

// ---------------------------------------------------------------------------
// GHCR authentication
// ---------------------------------------------------------------------------

/// Resolve authentication credentials for GHCR.
///
/// Checks, in order:
/// 1. `GHCR_TOKEN` environment variable
/// 2. `GITHUB_TOKEN` environment variable
/// 3. Docker config file (`~/.docker/config.json`) for saved `ghcr.io` creds
/// 4. Falls back to `Anonymous`
pub fn resolve_ghcr_auth() -> zlayer_registry::RegistryAuth {
    use zlayer_registry::RegistryAuth;

    // 1. GHCR_TOKEN
    if let Ok(token) = std::env::var("GHCR_TOKEN") {
        if !token.is_empty() {
            debug!("Using GHCR_TOKEN for registry auth");
            return RegistryAuth::Basic("_token".to_string(), token);
        }
    }

    // 2. GITHUB_TOKEN
    if let Ok(token) = std::env::var("GITHUB_TOKEN") {
        if !token.is_empty() {
            debug!("Using GITHUB_TOKEN for registry auth");
            return RegistryAuth::Basic("_token".to_string(), token);
        }
    }

    // 3. Docker config
    if let Some(creds) = read_docker_config_ghcr_auth() {
        debug!("Using Docker config credentials for GHCR");
        return creds;
    }

    // 4. Anonymous
    debug!("No GHCR credentials found, using anonymous auth");
    RegistryAuth::Anonymous
}

/// Attempt to read GHCR credentials from `~/.docker/config.json`.
fn read_docker_config_ghcr_auth() -> Option<zlayer_registry::RegistryAuth> {
    use base64::prelude::*;
    use zlayer_registry::RegistryAuth;

    let home = dirs::home_dir()?;
    let config_path = home.join(".docker").join("config.json");
    let contents = std::fs::read_to_string(&config_path).ok()?;
    let config: serde_json::Value = serde_json::from_str(&contents).ok()?;

    let auth_b64 = config.get("auths")?.get("ghcr.io")?.get("auth")?.as_str()?;

    let decoded = BASE64_STANDARD.decode(auth_b64).ok()?;
    let decoded_str = String::from_utf8(decoded).ok()?;

    let (user, pass) = decoded_str.split_once(':')?;
    Some(RegistryAuth::Basic(user.to_string(), pass.to_string()))
}

// ---------------------------------------------------------------------------
// Tier 1: Pull pre-built image from GHCR
// ---------------------------------------------------------------------------

/// Try to pull a pre-built `ZLayer` sandbox image from `GHCR`.
///
/// Returns `Ok(true)` if the image was successfully pulled and extracted,
/// `Ok(false)` if the image could not be pulled (auth failure, not found,
/// network error, etc.). Errors are logged as warnings but never propagated
/// — the caller should fall through to the next resolution tier.
///
/// # Errors
///
/// Returns an error if directory creation or layer unpacking fails with an
/// unrecoverable I/O error.
#[cfg(feature = "cache")]
pub async fn try_pull_zlayer_image(
    image_ref: &str,
    image_dir: &Path,
    rootfs_dir: &Path,
) -> Result<bool> {
    use zlayer_registry::{BlobCache, ImagePuller, LayerUnpacker};

    info!("Attempting to pull ZLayer image: {}", image_ref);

    let cache = match BlobCache::new() {
        Ok(c) => c,
        Err(e) => {
            warn!("Failed to create blob cache for GHCR pull: {e}");
            return Ok(false);
        }
    };
    let puller = ImagePuller::new(cache);
    let auth = resolve_ghcr_auth();

    // Pull layers
    let layers = match puller.pull_image(image_ref, &auth).await {
        Ok(l) => l,
        Err(e) => {
            warn!("Failed to pull ZLayer image {image_ref}: {e}");
            return Ok(false);
        }
    };

    info!(
        "Pulled {} layers for {}, extracting to rootfs",
        layers.len(),
        image_ref
    );

    // Ensure directories exist
    tokio::fs::create_dir_all(rootfs_dir).await?;

    // Unpack layers
    let mut unpacker = LayerUnpacker::new(rootfs_dir.to_path_buf());
    let layer_refs: Vec<(Vec<u8>, String)> = layers;
    if let Err(e) = unpacker.unpack_layers(&layer_refs).await {
        warn!("Failed to unpack layers for {image_ref}: {e}");
        return Ok(false);
    }

    // Pull and save image config if possible
    match puller.pull_image_config(image_ref, &auth).await {
        Ok(ic) => {
            if let Ok(json) = serde_json::to_string_pretty(&ic) {
                let _ = tokio::fs::write(image_dir.join("image_config.json"), json).await;
            }
        }
        Err(e) => debug!("Could not pull image config for {image_ref}: {e}"),
    }

    info!(
        "Successfully pulled and extracted ZLayer image: {}",
        image_ref
    );
    Ok(true)
}

// ---------------------------------------------------------------------------
// Tier 2: Build toolchain as a local image
// ---------------------------------------------------------------------------

/// Build a sandbox image by installing the toolchain locally.
///
/// Creates an image directory at `data_dir/images/{sanitized_name}/` containing
/// a `rootfs/` with host binaries and the provisioned toolchain, plus a
/// `config.json` with appropriate environment variables.
///
/// # Errors
///
/// Returns an error if directory creation, toolchain provisioning, or config
/// serialization fails.
pub async fn build_toolchain_as_image(
    spec: &ToolchainSpec,
    image_ref: &str,
    data_dir: &Path,
) -> Result<PathBuf> {
    let image_name = sanitize_image_name(image_ref);
    let image_dir = data_dir.join("images").join(&image_name);
    let rootfs_dir = image_dir.join("rootfs");

    tokio::fs::create_dir_all(&rootfs_dir).await?;

    // Lay down the base rootfs (host binaries, SSL certs, directory structure)
    ensure_base_rootfs(&rootfs_dir).await?;

    // Provision the toolchain into the rootfs
    let cache_dir = data_dir.join("toolchain-cache");
    let tmp_dir = data_dir.join("tmp");
    tokio::fs::create_dir_all(&cache_dir).await?;
    tokio::fs::create_dir_all(&tmp_dir).await?;

    provision_toolchain(spec, &rootfs_dir, &cache_dir, &tmp_dir).await?;

    // Write config.json
    let config = toolchain_spec_to_config(spec);
    let config_json =
        serde_json::to_string_pretty(&config).map_err(|e| BuildError::CacheError {
            message: format!("failed to serialise image config: {e}"),
        })?;
    tokio::fs::write(image_dir.join("config.json"), config_json).await?;

    info!(
        "Built toolchain image for {} v{} at {}",
        spec.language,
        spec.version,
        image_dir.display()
    );
    Ok(image_dir)
}

// ---------------------------------------------------------------------------
// Tier 3: Base image (distro) fallback
// ---------------------------------------------------------------------------

/// Build a minimal base image (no toolchain) for distro images like ubuntu/alpine.
///
/// Creates a rootfs with host binaries, SSL certs, and a default config.
///
/// # Errors
///
/// Returns an error if directory creation, base rootfs setup, or config
/// serialization fails.
pub async fn build_base_image(image_ref: &str, data_dir: &Path) -> Result<PathBuf> {
    let image_name = sanitize_image_name(image_ref);
    let image_dir = data_dir.join("images").join(&image_name);
    let rootfs_dir = image_dir.join("rootfs");

    tokio::fs::create_dir_all(&rootfs_dir).await?;

    // Lay down the base rootfs
    ensure_base_rootfs(&rootfs_dir).await?;

    // Write a default config.json
    let config = SandboxImageConfig {
        env: vec![
            "PATH=/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin".to_string(),
            "HOME=/root".to_string(),
        ],
        working_dir: "/".to_string(),
        entrypoint: None,
        cmd: Some(vec!["/bin/sh".to_string()]),
        exposed_ports: HashMap::new(),
        labels: HashMap::new(),
        user: None,
        volumes: Vec::new(),
        stop_signal: None,
        shell: None,
        healthcheck: None,
    };
    let config_json =
        serde_json::to_string_pretty(&config).map_err(|e| BuildError::CacheError {
            message: format!("failed to serialise image config: {e}"),
        })?;
    tokio::fs::write(image_dir.join("config.json"), config_json).await?;

    info!("Built base image at {}", image_dir.display());
    Ok(image_dir)
}

// ---------------------------------------------------------------------------
// Config conversion
// ---------------------------------------------------------------------------

/// Convert a [`ToolchainSpec`] into a [`SandboxImageConfig`].
///
/// Maps the spec's environment variables and PATH directories into the
/// container-style config format used by the sandbox builder.
#[must_use]
pub fn toolchain_spec_to_config(spec: &ToolchainSpec) -> SandboxImageConfig {
    // Build environment variables
    let mut env: Vec<String> = spec.env.iter().map(|(k, v)| format!("{k}={v}")).collect();

    // Build PATH from spec.path_dirs + standard paths
    let mut path_parts: Vec<&str> = spec.path_dirs.iter().map(String::as_str).collect();
    path_parts.extend(["/usr/local/bin", "/usr/bin", "/bin", "/usr/sbin", "/sbin"]);
    let path_value = path_parts.join(":");
    env.push(format!("PATH={path_value}"));

    env.push("HOME=/root".to_string());

    SandboxImageConfig {
        env,
        working_dir: "/".to_string(),
        entrypoint: None,
        cmd: Some(vec!["/bin/sh".to_string()]),
        exposed_ports: HashMap::new(),
        labels: HashMap::new(),
        user: None,
        volumes: Vec::new(),
        stop_signal: None,
        shell: None,
        healthcheck: None,
    }
}

// ---------------------------------------------------------------------------
// Homebrew bottle fetching
// ---------------------------------------------------------------------------

/// Homebrew formula API response (subset of fields we care about).
#[derive(Debug, Deserialize)]
struct BrewFormulaInfo {
    bottle: BrewBottle,
    #[serde(default)]
    dependencies: Vec<String>,
    #[serde(default)]
    versions: BrewVersions,
}

/// Version information from the Homebrew API.
#[derive(Debug, Default, Deserialize)]
struct BrewVersions {
    #[serde(default)]
    stable: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BrewBottle {
    stable: BrewBottleStable,
}

#[derive(Debug, Deserialize)]
struct BrewBottleStable {
    files: HashMap<String, BrewBottleFile>,
}

#[derive(Debug, Deserialize)]
struct BrewBottleFile {
    url: String,
}

/// Fetch formula metadata, checking `RepoSources` cache first, then Homebrew API.
///
/// After a successful Homebrew API fetch, fires a non-blocking POST to
/// `reposync.blackleafdigital.com` so the formula gets cached for everyone.
///
/// # Errors
///
/// Returns an error if both the cache and the API fail.
async fn fetch_formula_info(formula: &str) -> Result<BrewFormulaInfo> {
    // 1. Try RepoSources cached formula
    let cached_url = format!("{REPO_SOURCES_BASE}/../formulas/{formula}.json");
    if let Ok(resp) = reqwest::get(&cached_url).await {
        if resp.status().is_success() {
            if let Ok(info) = resp.json::<BrewFormulaInfo>().await {
                debug!("Using cached formula from RepoSources: {}", formula);
                return Ok(info);
            }
        }
    }

    // 2. Fetch from Homebrew API
    let api_url = format!("https://formulae.brew.sh/api/formula/{formula}.json");
    info!("Fetching Homebrew formula info for: {}", formula);

    let response = reqwest::get(&api_url)
        .await
        .map_err(|e| BuildError::RegistryError {
            message: format!("failed to fetch Homebrew formula info for {formula}: {e}"),
        })?;

    if !response.status().is_success() {
        return Err(BuildError::RegistryError {
            message: format!(
                "Homebrew formula API returned {} for {formula}",
                response.status()
            ),
        });
    }

    let body = response
        .bytes()
        .await
        .map_err(|e| BuildError::RegistryError {
            message: format!("failed to read Homebrew formula response for {formula}: {e}"),
        })?;

    let info: BrewFormulaInfo =
        serde_json::from_slice(&body).map_err(|e| BuildError::RegistryError {
            message: format!("failed to parse Homebrew formula JSON for {formula}: {e}"),
        })?;

    // 3. Fire-and-forget POST to reposync so it gets cached
    let formula_name = formula.to_string();
    let body_clone = body.to_vec();
    tokio::spawn(async move {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .to_string();

        let _ = reqwest::Client::new()
            .post("https://reposync.blackleafdigital.com/formula")
            .header("x-zlayer-repo-sync", &timestamp)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"name":"{}","data":{}}}"#,
                formula_name,
                String::from_utf8_lossy(&body_clone)
            ))
            .send()
            .await;
    });

    Ok(info)
}

/// Download and extract a single Homebrew bottle into the sandbox rootfs.
///
/// Finds the correct bottle for the current platform, downloads it (via the OCI
/// registry client for GHCR URLs, or direct HTTP for others), extracts into
/// `{rootfs}/opt/homebrew/Cellar/{formula}/{version}/`, and symlinks binaries
/// into `{rootfs}/opt/homebrew/bin/`.
///
/// # Errors
///
/// Returns an error if no bottle is available for the current platform, download
/// or extraction fails, or filesystem operations (directory creation, symlink)
/// fail.
#[allow(clippy::too_many_lines)]
async fn install_single_bottle(
    formula: &str,
    info: &BrewFormulaInfo,
    rootfs_dir: &Path,
    tmp_dir: &Path,
) -> Result<()> {
    // Find bottle for current platform
    let platform_tag = bottle_platform_tag();
    let bottle_file = info
        .bottle
        .stable
        .files
        .get(&platform_tag)
        .or_else(|| info.bottle.stable.files.get("all"))
        .ok_or_else(|| BuildError::RegistryError {
            message: format!(
                "no Homebrew bottle for {formula} on platform {platform_tag}; \
                 available: {:?}",
                info.bottle.stable.files.keys().collect::<Vec<_>>()
            ),
        })?;

    // Download the bottle tarball
    let tarball_path = tmp_dir.join(format!("{formula}.tar.gz"));
    tokio::fs::create_dir_all(tmp_dir).await?;

    info!(
        "Downloading bottle for {} from {}",
        formula, bottle_file.url
    );

    // Use the OCI registry client to download GHCR blobs properly
    // (handles auth, redirects, and OCI protocol headers)
    let bottle_bytes = if let Some((image_ref, digest)) = parse_ghcr_blob_url(&bottle_file.url) {
        let cache = zlayer_registry::BlobCache::new().map_err(|e| BuildError::RegistryError {
            message: format!("failed to create blob cache: {e}"),
        })?;
        let puller = zlayer_registry::ImagePuller::new(cache);
        let auth = zlayer_registry::RegistryAuth::Anonymous;
        let data = puller
            .pull_blob(&image_ref, &digest, &auth)
            .await
            .map_err(|e| BuildError::RegistryError {
                message: format!("failed to download bottle for {formula}: {e}"),
            })?;
        data
    } else {
        // Fallback: direct HTTP download for non-GHCR URLs
        reqwest::get(&bottle_file.url)
            .await
            .map_err(|e| BuildError::RegistryError {
                message: format!("failed to download bottle for {formula}: {e}"),
            })?
            .bytes()
            .await
            .map_err(|e| BuildError::RegistryError {
                message: format!("failed to read bottle bytes for {formula}: {e}"),
            })?
            .to_vec()
    };

    tokio::fs::write(&tarball_path, &bottle_bytes).await?;

    // Extract version from the tarball (first path component after stripping 2)
    // We'll extract to a temp dir first to discover the version directory name.
    let extract_tmp = tmp_dir.join(format!("{formula}_extract"));
    tokio::fs::create_dir_all(&extract_tmp).await?;

    let output = tokio::process::Command::new("tar")
        .args(["xzf"])
        .arg(&tarball_path)
        .arg("-C")
        .arg(&extract_tmp)
        .output()
        .await?;

    if !output.status.success() {
        return Err(BuildError::RegistryError {
            message: format!(
                "failed to extract bottle for {formula}: {}",
                String::from_utf8_lossy(&output.stderr)
            ),
        });
    }

    // The tarball structure is {formula}/{version}/... — find the version dir
    let formula_dir = extract_tmp.join(formula);
    let version = if formula_dir.exists() {
        let mut entries = tokio::fs::read_dir(&formula_dir).await?;
        let mut found_version = String::from("unknown");
        if let Some(entry) = entries.next_entry().await? {
            found_version = entry.file_name().to_string_lossy().to_string();
        }
        found_version
    } else {
        // Fallback: just use "latest"
        "latest".to_string()
    };

    // Create the Cellar directory in rootfs
    let cellar_dir = rootfs_dir
        .join("opt/homebrew/Cellar")
        .join(formula)
        .join(&version);
    tokio::fs::create_dir_all(&cellar_dir).await?;

    // Copy the extracted contents to the Cellar
    let src_version_dir = formula_dir.join(&version);
    if src_version_dir.exists() {
        let cp_output = tokio::process::Command::new("cp")
            .args(["-R"])
            .arg(format!("{}/", src_version_dir.display()))
            .arg(format!("{}/", cellar_dir.display()))
            .output()
            .await?;

        if !cp_output.status.success() {
            // Try alternative cp invocation
            let _ = tokio::process::Command::new("cp")
                .args(["-R"])
                .arg(format!("{}/.", src_version_dir.display()))
                .arg(cellar_dir.display().to_string())
                .output()
                .await;
        }
    }

    // Symlink binaries into opt/homebrew/bin/
    let homebrew_bin = rootfs_dir.join("opt/homebrew/bin");
    tokio::fs::create_dir_all(&homebrew_bin).await?;

    let cellar_bin = cellar_dir.join("bin");
    if cellar_bin.exists() {
        let mut bin_entries = tokio::fs::read_dir(&cellar_bin).await?;
        while let Some(entry) = bin_entries.next_entry().await? {
            let entry_path = entry.path();
            let file_name = entry.file_name();
            let link_path = homebrew_bin.join(&file_name);

            // Remove existing symlink/file before creating new one
            let _ = tokio::fs::remove_file(&link_path).await;
            if let Err(e) = tokio::fs::symlink(&entry_path, &link_path).await {
                debug!("Failed to symlink {}: {e}", file_name.to_string_lossy());
            }
        }
    }

    // Cleanup temp files
    let _ = tokio::fs::remove_dir_all(&extract_tmp).await;
    let _ = tokio::fs::remove_file(&tarball_path).await;

    Ok(())
}

/// Fetch and extract a Homebrew bottle (and all its dependencies) into the
/// sandbox rootfs.
///
/// Performs a BFS traversal of the formula's dependency tree, installing each
/// dependency before the formula itself. Formulas that are already present in
/// the rootfs Cellar are skipped. Failures on individual dependencies are
/// logged as warnings but do not abort the overall installation.
///
/// # Errors
///
/// This function is infallible at the top level — individual formula failures
/// are logged and skipped so that as many packages as possible are installed.
pub async fn fetch_and_extract_bottle(
    formula: &str,
    rootfs_dir: &Path,
    tmp_dir: &Path,
) -> Result<()> {
    let mut installed = HashSet::new();
    let mut queue = VecDeque::new();
    queue.push_back(formula.to_string());

    while let Some(current) = queue.pop_front() {
        if installed.contains(&current) {
            continue;
        }

        // Check if already in rootfs (from a previous build or earlier in this build)
        let cellar = rootfs_dir.join("opt/homebrew/Cellar").join(&current);
        if cellar.exists() {
            debug!("Skipping {} (already in rootfs)", current);
            installed.insert(current);
            continue;
        }

        let info = match fetch_formula_info(&current).await {
            Ok(info) => info,
            Err(e) => {
                warn!(
                    "Failed to fetch formula info for {}: {} (skipping)",
                    current, e
                );
                installed.insert(current);
                continue;
            }
        };

        // Queue dependencies
        for dep in &info.dependencies {
            if !installed.contains(dep) {
                queue.push_back(dep.clone());
            }
        }

        // Install this formula
        match install_single_bottle(&current, &info, rootfs_dir, tmp_dir).await {
            Ok(()) => {
                let version_str = info.versions.stable.as_deref().unwrap_or("unknown");
                info!("Installed {} v{} via Homebrew bottle", current, version_str);
            }
            Err(e) => {
                warn!(
                    "Failed to install bottle for {}: {} (continuing)",
                    current, e
                );
            }
        }
        installed.insert(current);
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Linux package name mapping
// ---------------------------------------------------------------------------

/// Map Linux package names (apt/apk) to Homebrew formula names.
///
/// Resolution order for each package:
/// 1. Cached/fetched package map from `RepoSources` (`{cache_dir}/package-maps/debian.json`)
/// 2. Name transformation heuristics (strip `-dev`, `lib` prefix, version digits)
/// 3. Hardcoded fallback mapping
///
/// Returns a vec of `(brew_formula, skipped)` tuples. `skipped` is `true` for
/// packages that are Linux-only and have no macOS equivalent (e.g. `musl-dev`,
/// `ca-certificates`).
pub async fn map_linux_packages(
    packages: &[&str],
    distro: &str,
    cache_dir: &Path,
) -> Vec<(String, bool)> {
    let map = load_or_fetch_package_map(distro, cache_dir).await;

    packages
        .iter()
        .map(|&pkg| resolve_single_package(pkg, &map))
        .collect()
}

/// Resolve a single Linux package name to a `(brew_formula, skipped)` pair.
///
/// Tries the remote/cached map first, then name transformations, then the
/// hardcoded fallback.
fn resolve_single_package(pkg: &str, map: &HashMap<String, String>) -> (String, bool) {
    // 1. Check if it's a Linux-only skip package (always hardcoded — these never
    //    have a brew equivalent).
    if is_linux_only_package(pkg) {
        return (pkg.to_string(), true);
    }

    // 2. Look up in the RepoSources map.
    if let Some(brew) = map.get(pkg) {
        return (brew.clone(), false);
    }

    // 3. Try name transformations and look those up in the map.
    if let Some(brew) = try_name_transforms(pkg, map) {
        return (brew, false);
    }

    // 4. Fall back to the hardcoded per-package mapping.
    map_single_package_hardcoded(pkg)
}

/// Returns `true` for packages that are Linux-only and should be skipped on macOS.
fn is_linux_only_package(pkg: &str) -> bool {
    matches!(
        pkg,
        "build-essential"
            | "gcc"
            | "g++"
            | "make"
            | "ca-certificates"
            | "apt-transport-https"
            | "gnupg"
            | "gnupg2"
            | "musl-dev"
            | "musl-tools"
            | "musl"
            | "libc-dev"
            | "libc6-dev"
            | "linux-headers"
            | "linux-headers-generic"
            | "software-properties-common"
            | "procps"
    )
}

/// Try common Linux-to-Homebrew name transformations and look the result up
/// in the map.
///
/// Transformations attempted (in order):
/// 1. Strip `-dev` suffix
/// 2. Strip `lib` prefix
/// 3. Strip both `lib` prefix and `-dev` suffix
/// 4. Remove trailing version digits (e.g. `libfoo3` -> `libfoo`)
fn try_name_transforms(pkg: &str, map: &HashMap<String, String>) -> Option<String> {
    // Strip -dev suffix
    if let Some(base) = pkg.strip_suffix("-dev") {
        if let Some(brew) = map.get(base) {
            return Some(brew.clone());
        }
    }

    // Strip lib prefix
    if let Some(rest) = pkg.strip_prefix("lib") {
        if let Some(brew) = map.get(rest) {
            return Some(brew.clone());
        }

        // Strip both lib prefix and -dev suffix
        if let Some(base) = rest.strip_suffix("-dev") {
            if let Some(brew) = map.get(base) {
                return Some(brew.clone());
            }
        }
    }

    // Remove trailing version digits (e.g. "zlib1g" -> "zlib", "libfoo3" -> "libfoo")
    let without_digits = pkg.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.');
    if without_digits != pkg && !without_digits.is_empty() {
        if let Some(brew) = map.get(without_digits) {
            return Some(brew.clone());
        }
        // Also try without trailing 'g' (e.g. "zlib1g-dev" already handled by -dev strip)
        let without_g = without_digits.trim_end_matches('g');
        if without_g != without_digits && !without_g.is_empty() {
            if let Some(brew) = map.get(without_g) {
                return Some(brew.clone());
            }
        }
    }

    None
}

/// Hardcoded fallback mapping for a single Linux package to its Homebrew
/// equivalent. This is the safety net when the `RepoSources` map is unavailable
/// or doesn't contain the package.
fn map_single_package_hardcoded(pkg: &str) -> (String, bool) {
    let (brew_name, skip) = match pkg {
        // Direct mappings (same name)
        "curl" | "libcurl4-openssl-dev" | "libcurl-dev" => ("curl", false),
        "git" => ("git", false),
        "wget" => ("wget", false),
        "jq" => ("jq", false),
        "cmake" => ("cmake", false),
        "pkg-config" => ("pkg-config", false),
        "autoconf" => ("autoconf", false),
        "automake" => ("automake", false),
        "unzip" => ("unzip", false),
        "zip" => ("zip", false),
        "rsync" => ("rsync", false),
        "tree" => ("tree", false),
        "htop" => ("htop", false),
        "tmux" => ("tmux", false),
        "vim" => ("vim", false),

        // Name mappings
        "libssl-dev" | "openssl-dev" | "libssl3" => ("openssl", false),
        "libpq-dev" | "postgresql-client" => ("libpq", false),
        "libsqlite3-dev" | "sqlite-dev" => ("sqlite", false),
        "libffi-dev" => ("libffi", false),
        "libxml2-dev" | "libxml2" => ("libxml2", false),
        "libyaml-dev" => ("libyaml", false),
        "libreadline-dev" => ("readline", false),
        "libncurses-dev" | "libncurses5-dev" | "ncurses-dev" => ("ncurses", false),
        "zlib1g-dev" | "zlib-dev" => ("zlib", false),
        "libbz2-dev" => ("bzip2", false),
        "liblzma-dev" | "xz-dev" => ("xz", false),
        "libzstd-dev" => ("zstd", false),
        "python3" | "python3-dev" | "python3-pip" => ("python@3", false),
        "nodejs" => ("node", false),
        "default-jdk" | "openjdk-17-jdk" | "openjdk-21-jdk" => ("openjdk", false),
        "imagemagick" | "libmagickwand-dev" => ("imagemagick", false),
        "ffmpeg" | "libavcodec-dev" => ("ffmpeg", false),
        "libprotobuf-dev" | "protobuf-compiler" => ("protobuf", false),

        // Unknown packages pass through as-is
        other => (other, false),
    };
    (brew_name.to_string(), skip)
}

/// Load the package mapping for a distro, fetching from `RepoSources` if not
/// cached or stale.
///
/// Resolution order:
/// 1. `{cache_dir}/package-maps/{distro}.json` — if present and < 7 days old
/// 2. Fetch from `{REPO_SOURCES_BASE}/{distro}.json` and cache to disk
/// 3. If fetch fails, use stale cache if available
/// 4. If nothing available, return an empty map (caller falls through to hardcoded)
async fn load_or_fetch_package_map(distro: &str, cache_dir: &Path) -> HashMap<String, String> {
    let map_dir = cache_dir.join("package-maps");
    let cache_path = map_dir.join(format!("{distro}.json"));

    // 1. Check local cache freshness.
    if let Ok(meta) = tokio::fs::metadata(&cache_path).await {
        if let Ok(modified) = meta.modified() {
            let age = modified
                .elapsed()
                .unwrap_or(std::time::Duration::from_secs(u64::MAX));
            if age.as_secs() < PACKAGE_MAP_CACHE_TTL_SECS {
                if let Some(map) = read_cached_map(&cache_path).await {
                    debug!(
                        "Using cached package map for {distro} ({} mappings, age {}s)",
                        map.len(),
                        age.as_secs()
                    );
                    return map;
                }
            }
        }
    }

    // 2. Fetch from RepoSources.
    let url = format!("{REPO_SOURCES_BASE}/{distro}.json");
    debug!("Fetching package map from {url}");

    match fetch_package_map(&url).await {
        Ok(map_file) => {
            info!(
                "Fetched {} package mappings for {distro} from RepoSources",
                map_file.mappings.len()
            );
            // Cache to disk (best-effort).
            if let Err(e) = write_cached_map(&map_dir, &cache_path, &map_file).await {
                warn!("Failed to cache package map for {distro}: {e}");
            }
            map_file.mappings
        }
        Err(e) => {
            debug!("Failed to fetch package map for {distro}: {e}");

            // 3. Try stale cache.
            if let Some(map) = read_cached_map(&cache_path).await {
                info!(
                    "Using stale cached package map for {distro} ({} mappings)",
                    map.len()
                );
                return map;
            }

            // 4. Nothing available — return empty and let hardcoded fallback handle it.
            debug!("No package map available for {distro}, using hardcoded fallback only");
            HashMap::new()
        }
    }
}

/// Fetch a `PackageMapFile` from the given URL.
async fn fetch_package_map(url: &str) -> std::result::Result<PackageMapFile, String> {
    let response = reqwest::get(url)
        .await
        .map_err(|e| format!("HTTP request failed: {e}"))?;

    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status()));
    }

    response
        .json::<PackageMapFile>()
        .await
        .map_err(|e| format!("JSON parse failed: {e}"))
}

/// Read a cached package-map file from disk and return just the mappings.
async fn read_cached_map(path: &Path) -> Option<HashMap<String, String>> {
    let contents = tokio::fs::read_to_string(path).await.ok()?;
    let map_file: PackageMapFile = serde_json::from_str(&contents).ok()?;
    Some(map_file.mappings)
}

/// Write a `PackageMapFile` to disk, creating the directory if needed.
async fn write_cached_map(
    map_dir: &Path,
    cache_path: &Path,
    map_file: &PackageMapFile,
) -> std::result::Result<(), String> {
    tokio::fs::create_dir_all(map_dir)
        .await
        .map_err(|e| format!("create dir: {e}"))?;

    let json = serde_json::to_string_pretty(map_file).map_err(|e| format!("serialize: {e}"))?;

    tokio::fs::write(cache_path, json)
        .await
        .map_err(|e| format!("write: {e}"))
}

// ---------------------------------------------------------------------------
// Homebrew platform tag
// ---------------------------------------------------------------------------

/// Determine the Homebrew bottle platform tag for the current system.
///
/// Parse a GHCR blob URL into an image reference and digest.
///
/// Example: `https://ghcr.io/v2/homebrew/core/go/blobs/sha256:abc123`
/// returns `("ghcr.io/homebrew/core/go:latest", "sha256:abc123")`.
fn parse_ghcr_blob_url(url: &str) -> Option<(String, String)> {
    // Strip protocol
    let without_proto = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;

    // Split on /v2/ to get registry and the rest
    let (registry, rest) = without_proto.split_once("/v2/")?;

    // Split on /blobs/ to get repo and digest
    let (repo, digest) = rest.split_once("/blobs/")?;

    let image_ref = format!("{registry}/{repo}:latest");
    Some((image_ref, digest.to_string()))
}

/// Combines architecture and macOS release codename, e.g. `arm64_sequoia`
/// for Apple Silicon on macOS 15.x, or `sonoma` for Intel on macOS 14.x.
#[must_use]
pub fn bottle_platform_tag() -> String {
    let version_output = std::process::Command::new("sw_vers")
        .arg("-productVersion")
        .output();

    let macos_version = match version_output {
        Ok(ref out) if out.status.success() => {
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        }
        _ => {
            warn!("Could not determine macOS version via sw_vers, defaulting to sequoia");
            "15.0".to_string()
        }
    };

    let codename = if macos_version.starts_with("15.") || macos_version.starts_with("15") {
        "sequoia"
    } else if macos_version.starts_with("14.") || macos_version.starts_with("14") {
        "sonoma"
    } else if macos_version.starts_with("13.") || macos_version.starts_with("13") {
        "ventura"
    } else {
        // Default to the latest known codename
        "sequoia"
    };

    let arch = std::env::consts::ARCH;
    if arch == "aarch64" {
        format!("arm64_{codename}")
    } else {
        // x86_64 bottles use just the codename
        codename.to_string()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Sanitize an image reference for use as a directory name.
fn sanitize_image_name(image: &str) -> String {
    image.replace(['/', ':', '@'], "_")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- rewrite_image_for_macos tests --

    #[test]
    fn test_rewrite_golang() {
        let result = rewrite_image_for_macos("golang:1.23");
        assert_eq!(result, Some(format!("{ZLAYER_REGISTRY}/golang:1.23")));
    }

    #[test]
    fn test_rewrite_golang_alpine() {
        let result = rewrite_image_for_macos("golang:1.23-alpine");
        assert_eq!(result, Some(format!("{ZLAYER_REGISTRY}/golang:1.23")));
    }

    #[test]
    fn test_rewrite_ubuntu_to_base() {
        let result = rewrite_image_for_macos("ubuntu:22.04");
        assert_eq!(result, Some(format!("{ZLAYER_REGISTRY}/base:latest")));
    }

    #[test]
    fn test_rewrite_alpine_to_base() {
        let result = rewrite_image_for_macos("alpine:3.19");
        assert_eq!(result, Some(format!("{ZLAYER_REGISTRY}/base:latest")));
    }

    #[test]
    fn test_rewrite_node_latest() {
        let result = rewrite_image_for_macos("node:latest");
        assert_eq!(result, Some(format!("{ZLAYER_REGISTRY}/node:latest")));
    }

    #[test]
    fn test_rewrite_node_slim() {
        let result = rewrite_image_for_macos("node:20-slim");
        assert_eq!(result, Some(format!("{ZLAYER_REGISTRY}/node:20")));
    }

    #[test]
    fn test_rewrite_python_bookworm() {
        let result = rewrite_image_for_macos("python:3.12-bookworm");
        assert_eq!(result, Some(format!("{ZLAYER_REGISTRY}/python:3.12")));
    }

    #[test]
    fn test_rewrite_qualified_golang() {
        let result = rewrite_image_for_macos("docker.io/library/golang:1.22");
        assert_eq!(result, Some(format!("{ZLAYER_REGISTRY}/golang:1.22")));
    }

    #[test]
    fn test_no_rewrite_custom_image() {
        let result = rewrite_image_for_macos("myregistry.io/myteam/myapp:v1.0");
        assert_eq!(result, None);
    }

    #[test]
    fn test_no_rewrite_already_zlayer() {
        let result = rewrite_image_for_macos("ghcr.io/blackleafdigital/zlayer/golang:1.23");
        assert_eq!(result, None);
    }

    // -- toolchain_spec_to_config tests --

    #[test]
    fn test_toolchain_spec_to_config_go() {
        let spec = ToolchainSpec::go("1.23");
        let config = toolchain_spec_to_config(&spec);

        // Should have GOROOT, GOFLAGS, PATH, HOME
        assert!(config.env.iter().any(|e| e.starts_with("GOROOT=")));
        assert!(config.env.iter().any(|e| e.starts_with("PATH=")));

        // PATH should include go bin dir
        let path_entry = config.env.iter().find(|e| e.starts_with("PATH=")).unwrap();
        assert!(path_entry.contains("/usr/local/go/bin"));

        assert_eq!(config.working_dir, "/");
    }

    #[test]
    fn test_toolchain_spec_to_config_java() {
        let spec = ToolchainSpec::java("21");
        let config = toolchain_spec_to_config(&spec);

        assert!(config.env.iter().any(|e| e.starts_with("JAVA_HOME=")));

        let path_entry = config.env.iter().find(|e| e.starts_with("PATH=")).unwrap();
        assert!(path_entry.contains("/usr/local/java/bin"));
    }

    // -- map_linux_packages / resolve_single_package tests --

    #[test]
    fn test_resolve_common_packages_hardcoded() {
        let empty_map = HashMap::new();
        for pkg in &["curl", "git", "wget", "jq"] {
            let (name, skipped) = resolve_single_package(pkg, &empty_map);
            assert!(!skipped, "{pkg} should not be skipped");
            assert_eq!(name, *pkg);
        }
    }

    #[test]
    fn test_resolve_skip_linux_only() {
        let empty_map = HashMap::new();
        for pkg in &["build-essential", "ca-certificates", "musl-dev", "libc-dev"] {
            let (_name, skipped) = resolve_single_package(pkg, &empty_map);
            assert!(skipped, "{pkg} should be skipped");
        }
    }

    #[test]
    fn test_resolve_passthrough_unknown() {
        let empty_map = HashMap::new();
        let (name, skipped) = resolve_single_package("some-obscure-package", &empty_map);
        assert_eq!(name, "some-obscure-package");
        assert!(!skipped);
    }

    #[test]
    fn test_resolve_with_remote_map() {
        let mut map = HashMap::new();
        map.insert("libfoo-dev".to_string(), "foo".to_string());
        map.insert("custom-pkg".to_string(), "custom-brew".to_string());

        // Direct match from map
        let (name, skipped) = resolve_single_package("custom-pkg", &map);
        assert_eq!(name, "custom-brew");
        assert!(!skipped);

        // Direct match from map (with -dev suffix)
        let (name, skipped) = resolve_single_package("libfoo-dev", &map);
        assert_eq!(name, "foo");
        assert!(!skipped);
    }

    #[test]
    fn test_resolve_name_transforms() {
        let mut map = HashMap::new();
        map.insert("ssl".to_string(), "openssl".to_string());
        map.insert("yaml".to_string(), "libyaml".to_string());

        // Strip lib prefix to find "ssl" in map
        let (name, _) = resolve_single_package("libssl", &map);
        assert_eq!(name, "openssl");

        // Strip lib prefix + -dev suffix to find "yaml" in map
        let (name, _) = resolve_single_package("libyaml-dev", &map);
        assert_eq!(name, "libyaml");
    }

    #[tokio::test]
    async fn test_map_linux_packages_with_empty_cache() {
        let tmp = std::env::temp_dir().join("zlayer-test-pkg-map");
        let _ = tokio::fs::remove_dir_all(&tmp).await;

        let result =
            map_linux_packages(&["curl", "libssl-dev", "musl-dev"], "debian_12", &tmp).await;
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].0, "curl");
        assert!(!result[0].1);
        assert_eq!(result[1].0, "openssl@3");
        assert!(!result[1].1);
        assert_eq!(result[2].0, "musl-dev");
        assert!(result[2].1);

        let _ = tokio::fs::remove_dir_all(&tmp).await;
    }

    // -- bottle_platform_tag test --

    #[test]
    fn test_bottle_platform_tag() {
        // Just verify it doesn't panic and returns a non-empty string
        let tag = bottle_platform_tag();
        assert!(!tag.is_empty(), "platform tag should not be empty");
        // On macOS it should contain a codename
        assert!(
            tag.contains("sequoia") || tag.contains("sonoma") || tag.contains("ventura"),
            "unexpected platform tag: {tag}"
        );
    }
}
