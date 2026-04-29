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
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};
use zlayer_types::ImageReference;

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

/// Subdirectory name (under the platform cache dir) where per-distro shard
/// files are stored. Bumped from `package-maps-v2` to invalidate caches
/// that pre-date the cross-distro `common/` shard set + zombie-purging
/// generator overwrite — old caches still serve unversioned `openssl`,
/// `python`, `node` for cross-distro names (e.g. centos's `libssl-dev`).
/// Old dirs are abandoned, not migrated.
const PACKAGE_MAP_CACHE_SUBDIR: &str = "package-maps-v3";

/// Env var holding the shared secret used to sign POST requests to the
/// `RepoSourceSyncer` formula-cache endpoint. Must match the value the
/// function expects in its own `REPOSYNC_HMAC_SECRET` env var. If unset, the
/// resolver skips the cache-warming POST entirely.
const REPOSYNC_HMAC_SECRET_ENV: &str = "ZLAYER_REPOSYNC_HMAC_SECRET";

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
    /// Shard letter (`a`-`z` or `_misc`) for sharded files. `None` for
    /// hand-written test fixtures or pre-sharding archives.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    shard: Option<String>,
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

    // Split into name and tag using the canonical parser (handles host:port,
    // digests, and missing tags correctly).
    let (name, tag) = match ImageReference::from_str(&stripped) {
        Ok(r) => (
            r.repository().to_string(),
            r.tag().unwrap_or("latest").to_string(),
        ),
        Err(_) => (stripped.clone(), "latest".to_string()),
    };
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

    // Write config.json (include source hash for cache invalidation)
    let mut config = toolchain_spec_to_config(spec);
    {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(format!("{spec:?}").as_bytes());
        config.source_hash = Some(format!("{:x}", hasher.finalize()));
    }
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

    // Write a default config.json (include source hash for cache invalidation)
    let source_hash = {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(format!("base_image:{image_ref}").as_bytes());
        format!("{:x}", hasher.finalize())
    };
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
        source_hash: Some(source_hash),
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
        source_hash: None,
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

/// Synthesize a `BrewFormulaInfo` from a pinned `LockedBottle`. Used by
/// `resolve_package` to short-circuit live brew API calls when the build
/// has a `zlayer-bottles.lock` entry for the requested formula.
fn brew_info_from_locked(locked: &crate::bottle_lockfile::LockedBottle) -> BrewFormulaInfo {
    let files = locked
        .urls
        .iter()
        .map(|(tag, url)| (tag.clone(), BrewBottleFile { url: url.clone() }))
        .collect();
    BrewFormulaInfo {
        bottle: BrewBottle {
            stable: BrewBottleStable { files },
        },
        dependencies: locked.deps.clone(),
        versions: BrewVersions {
            stable: Some(locked.version.clone()),
        },
    }
}

/// Convert a freshly-resolved `BrewFormulaInfo` into a `LockedBottle`
/// suitable for writing to the lockfile. Captures every per-platform URL
/// brew published so one lockfile works across macOS versions on a team.
fn locked_bottle_from_info(
    formula: &str,
    info: &BrewFormulaInfo,
) -> crate::bottle_lockfile::LockedBottle {
    let urls = info
        .bottle
        .stable
        .files
        .iter()
        .map(|(tag, file)| (tag.clone(), file.url.clone()))
        .collect();
    crate::bottle_lockfile::LockedBottle {
        formula: formula.to_string(),
        version: info
            .versions
            .stable
            .clone()
            .unwrap_or_else(|| "unknown".to_string()),
        deps: info.dependencies.clone(),
        urls,
    }
}

/// A resolved package from any source -- not just Homebrew.
#[derive(Debug)]
enum ResolvedPackage {
    /// Standard Homebrew bottle from homebrew-core
    HomebrewBottle(BrewFormulaInfo),
    /// Direct download from a forge release (GitHub, GitLab, Codeberg, Forgejo)
    DirectRelease {
        name: String,
        #[allow(dead_code)]
        source: String,
        url: String,
        asset_name: String,
    },
    /// Formula from a Homebrew tap
    Tap {
        name: String,
        #[allow(dead_code)]
        tap: String,
        url: String,
    },
    /// Python managed by uv
    UvPython { version: String },
}

/// Response from the `RepoSourceSyncer` discovery endpoint.
#[derive(Debug, Deserialize)]
struct DiscoveryResponse {
    name: String,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    source_url: Option<String>,
    #[serde(default)]
    data: Option<serde_json::Value>,
}

/// Resolve a package from any available source.
///
/// Resolution order:
/// 1. Special-case: `python3` / `python` -> `UvPython`
/// 2. `RepoSources` cached formula -> `HomebrewBottle` if parseable
/// 3. Homebrew API -> `HomebrewBottle` (fires a non-blocking POST to reposync)
/// 4. `RepoSourceSyncer` `?discover=true` -> `DirectRelease`, `Tap`, or `HomebrewBottle`
///
/// # Errors
///
/// Returns an error if no source can provide the package.
#[allow(clippy::too_many_lines)]
async fn resolve_package(
    formula: &str,
    lockfile: Option<&crate::bottle_lockfile::BottleLockfile>,
) -> Result<ResolvedPackage> {
    // 0. Special case: python → UvPython
    // Phase 2 generator may emit `python@3.14` etc. — match the whole `python@3.x` family so uv-managed Python keeps firing.
    if formula == "python3"
        || formula == "python"
        || formula == "python@3"
        || formula
            .strip_prefix("python@3.")
            .is_some_and(|rest| !rest.is_empty())
    {
        return Ok(ResolvedPackage::UvPython {
            version: "3".to_string(),
        });
    }

    // Lockfile short-circuit. If the caller provided a lockfile and it has
    // an entry for this formula, synthesize a HomebrewBottle from the
    // pinned URL/version/deps and return — no live HTTP at all. This is
    // the reproducibility guarantee.
    if let Some(lock) = lockfile {
        if let Some(entry) = lock.get(formula) {
            debug!("Using locked bottle for {} (v{})", formula, entry.version);
            return Ok(ResolvedPackage::HomebrewBottle(brew_info_from_locked(
                entry,
            )));
        }
    }

    // 1. Try RepoSources cached formula
    let cached_url = format!("{REPO_SOURCES_BASE}/../formulas/{formula}.json");
    if let Ok(resp) = reqwest::get(&cached_url).await {
        if resp.status().is_success() {
            if let Ok(info) = resp.json::<BrewFormulaInfo>().await {
                debug!("Using cached formula from RepoSources: {}", formula);
                return Ok(ResolvedPackage::HomebrewBottle(info));
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

    if response.status().is_success() {
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

        // Fire-and-forget POST to reposync so it gets cached. Authenticated
        // via HMAC-SHA256 over the request body using a shared secret. If the
        // secret env var is unset (developer machine, CI without the
        // credential), skip the POST — the resolver still works, the upstream
        // cache just doesn't get warmed.
        if let Ok(secret) = std::env::var(REPOSYNC_HMAC_SECRET_ENV) {
            let formula_name = formula.to_string();
            let body_clone = body.to_vec();
            tokio::spawn(async move {
                let payload = format!(
                    r#"{{"name":"{}","data":{}}}"#,
                    formula_name,
                    String::from_utf8_lossy(&body_clone)
                );
                let signature = compute_reposync_signature(&secret, payload.as_bytes());
                let _ = reqwest::Client::new()
                    .post("https://reposync.blackleafdigital.com/formula")
                    .header("x-reposync-signature", signature)
                    .header("content-type", "application/json")
                    .body(payload)
                    .send()
                    .await;
            });
        } else {
            debug!(
                "REPOSYNC_HMAC_SECRET unset; skipping reposync cache warm for {}",
                formula
            );
        }

        return Ok(ResolvedPackage::HomebrewBottle(info));
    }

    // 3. Homebrew API failed (404 or other) — ask RepoSourceSyncer to discover it
    info!(
        "Homebrew API returned {} for {}, asking RepoSourceSyncer to discover",
        response.status(),
        formula
    );

    let discover_url =
        format!("https://reposync.blackleafdigital.com/formula/{formula}?discover=true");
    if let Ok(resp) = reqwest::get(&discover_url).await {
        if resp.status().is_success() {
            if let Ok(text) = resp.text().await {
                // Try to parse as DiscoveryResponse first
                if let Ok(discovery) = serde_json::from_str::<DiscoveryResponse>(&text) {
                    if let Some(ref source) = discovery.source {
                        if let Some(tap_name) = source.strip_prefix("tap:") {
                            let url = discovery.source_url.unwrap_or_default();
                            info!(
                                "Discovered {} as tap {} via RepoSourceSyncer",
                                formula, tap_name
                            );
                            return Ok(ResolvedPackage::Tap {
                                name: discovery.name,
                                tap: tap_name.to_string(),
                                url,
                            });
                        }
                        if source.starts_with("github-release:")
                            || source.starts_with("gitlab-release:")
                            || source.starts_with("codeberg-release:")
                            || source.starts_with("forgejo-release:")
                        {
                            let url = discovery.source_url.clone().unwrap_or_default();
                            let asset_name = url.rsplit('/').next().unwrap_or(formula).to_string();
                            info!(
                                "Discovered {} as direct release via RepoSourceSyncer",
                                formula
                            );
                            return Ok(ResolvedPackage::DirectRelease {
                                name: discovery.name,
                                source: source.clone(),
                                url,
                                asset_name,
                            });
                        }
                    }
                    // No source but has data — try as BrewFormulaInfo
                    if let Some(ref data) = discovery.data {
                        if let Ok(info) = serde_json::from_value::<BrewFormulaInfo>(data.clone()) {
                            info!("Discovered {} via RepoSourceSyncer (bottle data)", formula);
                            return Ok(ResolvedPackage::HomebrewBottle(info));
                        }
                    }
                }
                // Fall back: try parsing the whole response as BrewFormulaInfo
                if let Ok(info) = serde_json::from_str::<BrewFormulaInfo>(&text) {
                    info!("Discovered {} via RepoSourceSyncer", formula);
                    return Ok(ResolvedPackage::HomebrewBottle(info));
                }
                debug!(
                    "RepoSourceSyncer returned data for {} but not in any recognised format",
                    formula
                );
            }
        }
    }

    Err(BuildError::RegistryError {
        message: format!(
            "Formula '{formula}' not found in Homebrew, RepoSources, or forge discovery"
        ),
    })
}

/// Install a resolved package into the sandbox rootfs.
///
/// Dispatches to the appropriate installer based on the [`ResolvedPackage`] variant.
///
/// # Errors
///
/// Returns an error if installation fails.
#[allow(clippy::too_many_lines)]
async fn install_package(
    formula: &str,
    package: &ResolvedPackage,
    rootfs_dir: &Path,
    tmp_dir: &Path,
) -> Result<()> {
    match package {
        ResolvedPackage::HomebrewBottle(info) => {
            install_homebrew_bottle(formula, info, rootfs_dir, tmp_dir).await
        }
        ResolvedPackage::DirectRelease {
            name,
            url,
            asset_name,
            ..
        } => install_direct_release(name, url, asset_name, rootfs_dir, tmp_dir).await,
        ResolvedPackage::Tap { name, url, .. } => {
            // For taps, download and extract the same way as a direct release
            let asset_name = url.rsplit('/').next().unwrap_or(name);
            install_direct_release(name, url, asset_name, rootfs_dir, tmp_dir).await
        }
        ResolvedPackage::UvPython { version } => {
            install_uv_python(version, rootfs_dir, tmp_dir).await
        }
    }
}

/// Download a release asset, extract it, and copy executables to `{rootfs}/usr/local/bin/`.
///
/// Archive type is detected from the asset name:
/// - `.zip` -> `unzip -o`
/// - `.tar.gz` / `.tgz` -> `tar xzf`
/// - `.tar.xz` -> `tar xJf`
/// - No recognised extension -> treated as a single binary
///
/// After extraction, walks the tree looking for Mach-O executables (via `file`)
/// and copies them to `{rootfs}/usr/local/bin/` with `chmod +x`.
///
/// # Errors
///
/// Returns an error if download, extraction, or filesystem operations fail.
#[allow(
    clippy::too_many_lines,
    clippy::case_sensitive_file_extension_comparisons
)]
async fn install_direct_release(
    name: &str,
    url: &str,
    asset_name: &str,
    rootfs_dir: &Path,
    tmp_dir: &Path,
) -> Result<()> {
    tokio::fs::create_dir_all(tmp_dir).await?;

    let download_path = tmp_dir.join(asset_name);

    // Download the asset
    info!("Downloading release asset for {} from {}", name, url);
    let bytes = reqwest::get(url)
        .await
        .map_err(|e| BuildError::RegistryError {
            message: format!("failed to download release for {name}: {e}"),
        })?
        .bytes()
        .await
        .map_err(|e| BuildError::RegistryError {
            message: format!("failed to read release bytes for {name}: {e}"),
        })?;
    tokio::fs::write(&download_path, &bytes).await?;

    let usr_local_bin = rootfs_dir.join("usr/local/bin");
    tokio::fs::create_dir_all(&usr_local_bin).await?;

    let extract_dir = tmp_dir.join(format!("{name}_release_extract"));
    let _ = tokio::fs::remove_dir_all(&extract_dir).await;
    tokio::fs::create_dir_all(&extract_dir).await?;

    let lower = asset_name.to_lowercase();
    let is_archive = lower.ends_with(".zip")
        || lower.ends_with(".tar.gz")
        || lower.ends_with(".tgz")
        || lower.ends_with(".tar.xz");

    if is_archive {
        // Extract based on extension
        let output = if lower.ends_with(".zip") {
            tokio::process::Command::new("unzip")
                .args(["-o"])
                .arg(&download_path)
                .arg("-d")
                .arg(&extract_dir)
                .output()
                .await?
        } else if lower.ends_with(".tar.gz") || lower.ends_with(".tgz") {
            tokio::process::Command::new("tar")
                .args(["xzf"])
                .arg(&download_path)
                .arg("-C")
                .arg(&extract_dir)
                .output()
                .await?
        } else {
            // .tar.xz
            tokio::process::Command::new("tar")
                .args(["xJf"])
                .arg(&download_path)
                .arg("-C")
                .arg(&extract_dir)
                .output()
                .await?
        };

        if !output.status.success() {
            return Err(BuildError::RegistryError {
                message: format!(
                    "failed to extract release archive for {name}: {}",
                    String::from_utf8_lossy(&output.stderr)
                ),
            });
        }

        // Walk the extracted tree and find Mach-O executables
        let find_output = tokio::process::Command::new("find")
            .arg(&extract_dir)
            .args(["-type", "f"])
            .output()
            .await?;

        let files_list = String::from_utf8_lossy(&find_output.stdout);
        for line in files_list.lines() {
            let path = Path::new(line.trim());
            if !path.exists() {
                continue;
            }
            // Use `file` command to check for Mach-O binary
            let file_output = tokio::process::Command::new("file")
                .arg(path)
                .output()
                .await;
            if let Ok(fo) = file_output {
                let file_desc = String::from_utf8_lossy(&fo.stdout);
                if file_desc.contains("Mach-O") {
                    if let Some(file_name) = path.file_name() {
                        let dest = usr_local_bin.join(file_name);
                        let _ = tokio::fs::copy(path, &dest).await;
                        let _ = tokio::process::Command::new("chmod")
                            .args(["+x"])
                            .arg(&dest)
                            .status()
                            .await;
                        info!(
                            "Installed binary {} from release {}",
                            file_name.to_string_lossy(),
                            name
                        );
                    }
                }
            }
        }
    } else {
        // No recognised archive extension — treat as a single binary
        let dest = usr_local_bin.join(name);
        tokio::fs::copy(&download_path, &dest).await?;
        let _ = tokio::process::Command::new("chmod")
            .args(["+x"])
            .arg(&dest)
            .status()
            .await;
        info!("Installed single binary {} from release", name);
    }

    // Cleanup
    let _ = tokio::fs::remove_file(&download_path).await;
    let _ = tokio::process::Command::new("rm")
        .args(["-rf"])
        .arg(&extract_dir)
        .status()
        .await;

    Ok(())
}

/// Install Python via uv.
///
/// Ensures `uv` is available (installs it as a Homebrew bottle if not found),
/// then runs `uv python install {version}` with the install dir set to
/// `{rootfs}/usr/local/python`. Symlinks `python3` and `python` into
/// `{rootfs}/usr/local/bin/`.
///
/// # Errors
///
/// Returns an error if uv installation or python provisioning fails.
async fn install_uv_python(version: &str, rootfs_dir: &Path, tmp_dir: &Path) -> Result<()> {
    let uv_bin = rootfs_dir.join("opt/homebrew/bin/uv");

    // Ensure uv is installed
    if !uv_bin.exists() {
        info!("uv not found in rootfs, installing via Homebrew bottle");
        // uv bootstrap is internal infrastructure; not subject to per-spec lockfile pinning.
        let uv_pkg = resolve_package("uv", None).await?;
        Box::pin(install_package("uv", &uv_pkg, rootfs_dir, tmp_dir)).await?;
    }

    if !uv_bin.exists() {
        return Err(BuildError::RegistryError {
            message: "failed to install uv — binary not found after installation".to_string(),
        });
    }

    let python_install_dir = rootfs_dir.join("usr/local/python");
    tokio::fs::create_dir_all(&python_install_dir).await?;

    info!("Installing Python {} via uv", version);
    let output = tokio::process::Command::new(&uv_bin)
        .args(["python", "install", version])
        .env("UV_PYTHON_INSTALL_DIR", &python_install_dir)
        .output()
        .await?;

    if !output.status.success() {
        return Err(BuildError::RegistryError {
            message: format!(
                "uv python install failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ),
        });
    }

    // Symlink python3 and python into usr/local/bin
    let usr_local_bin = rootfs_dir.join("usr/local/bin");
    tokio::fs::create_dir_all(&usr_local_bin).await?;

    // Find the python binary uv installed
    let find_output = tokio::process::Command::new("find")
        .arg(&python_install_dir)
        .args(["-name", "python3", "-type", "f"])
        .output()
        .await?;

    let python_bin_path = String::from_utf8_lossy(&find_output.stdout);
    if let Some(line) = python_bin_path.lines().next() {
        let python_path = PathBuf::from(line.trim());
        if python_path.exists() {
            let symlink_python3 = usr_local_bin.join("python3");
            let symlink_python = usr_local_bin.join("python");
            let _ = tokio::fs::remove_file(&symlink_python3).await;
            let _ = tokio::fs::remove_file(&symlink_python).await;
            let _ = tokio::fs::symlink(&python_path, &symlink_python3).await;
            let _ = tokio::fs::symlink(&python_path, &symlink_python).await;
            info!("Symlinked python3 and python to {}", python_path.display());
        }
    }

    Ok(())
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
async fn install_homebrew_bottle(
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
    let _ = tokio::process::Command::new("chmod")
        .args(["-R", "u+w"])
        .arg(&extract_tmp)
        .status()
        .await;
    let _ = tokio::process::Command::new("rm")
        .args(["-rf"])
        .arg(&extract_tmp)
        .status()
        .await;
    let _ = tokio::fs::remove_file(&tarball_path).await;

    Ok(())
}

/// Resolve and install a package (with all Homebrew dependencies if applicable)
/// into the sandbox rootfs.
///
/// Performs a BFS traversal of the dependency tree for Homebrew bottles,
/// installing each dependency before the formula itself. Packages that are
/// already present in the rootfs Cellar are skipped.
///
/// Failure handling distinguishes the **root** package (the caller-supplied
/// `formula`) from **transitive** dependencies discovered during the BFS:
///   - Resolve/install failure on the root → returns `Err`. The caller's
///     `apt-get install foo` or equivalent must hard-fail rather than silently
///     produce a sandbox missing `foo`.
///   - Resolve/install failure on a transitive dep → logged as `warn!` and
///     skipped. A missing optional dep should not kill the install of an
///     otherwise-functional package.
///
/// For non-Homebrew packages (`DirectRelease`, `Tap`, `UvPython`) the
/// dependency walk is skipped since those sources do not expose brew-style
/// dependency metadata.
///
/// `lockfile`, when `Some`, is consulted by `resolve_package` before any brew
/// API call: a hit on `formula` (or any transitive dep) short-circuits to a
/// pinned `HomebrewBottle`. `captured` accumulates a `LockedBottle` for every
/// successfully-resolved `HomebrewBottle` in this BFS so the caller can
/// rewrite `zlayer-bottles.lock` after the build.
///
/// # Errors
///
/// Returns `Err` if the root formula cannot be resolved or installed.
pub async fn install_with_deps(
    formula: &str,
    rootfs_dir: &Path,
    tmp_dir: &Path,
    lockfile: Option<&crate::bottle_lockfile::BottleLockfile>,
    captured: &mut Vec<crate::bottle_lockfile::LockedBottle>,
) -> Result<()> {
    let root = formula.to_string();
    let mut installed = HashSet::new();
    let mut queue = VecDeque::new();
    queue.push_back(root.clone());

    while let Some(current) = queue.pop_front() {
        if installed.contains(&current) {
            continue;
        }

        let is_root = current == root;

        // Check if already in rootfs (from a previous build or earlier in this build)
        let cellar = rootfs_dir.join("opt/homebrew/Cellar").join(&current);
        let usr_bin = rootfs_dir.join("usr/local/bin").join(&current);
        if cellar.exists() || usr_bin.exists() {
            debug!("Skipping {} (already in rootfs)", current);
            installed.insert(current);
            continue;
        }

        let package = match resolve_package(&current, lockfile).await {
            Ok(pkg) => pkg,
            Err(e) => {
                if is_root {
                    return Err(BuildError::RegistryError {
                        message: format!("failed to resolve Homebrew formula '{current}': {e}"),
                    });
                }
                warn!(
                    "Failed to resolve transitive dep {}: {} (skipping)",
                    current, e
                );
                installed.insert(current);
                continue;
            }
        };

        // Queue dependencies only for HomebrewBottle (other sources don't have brew deps)
        if let ResolvedPackage::HomebrewBottle(ref info) = package {
            for dep in &info.dependencies {
                if !installed.contains(dep) {
                    queue.push_back(dep.clone());
                }
            }
        }

        // Install this package
        match install_package(&current, &package, rootfs_dir, tmp_dir).await {
            Ok(()) => {
                let version_str = match &package {
                    ResolvedPackage::HomebrewBottle(info) => info
                        .versions
                        .stable
                        .as_deref()
                        .unwrap_or("unknown")
                        .to_string(),
                    ResolvedPackage::UvPython { version } => version.clone(),
                    _ => "latest".to_string(),
                };
                info!("Installed {} v{}", current, version_str);
            }
            Err(e) => {
                if is_root {
                    return Err(BuildError::RegistryError {
                        message: format!("failed to install Homebrew formula '{current}': {e}"),
                    });
                }
                warn!(
                    "Failed to install transitive dep {}: {} (continuing)",
                    current, e
                );
            }
        }

        // Capture the resolved bottle for the lockfile. Only HomebrewBottle
        // is captured; DirectRelease / Tap / UvPython use different
        // install machinery that the lockfile schema doesn't model in v1.
        if let ResolvedPackage::HomebrewBottle(ref info) = package {
            captured.push(locked_bottle_from_info(&current, info));
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
/// 1. Cached/fetched package map from `RepoSources` (`{cache_dir}/package-maps-v2/debian.json`)
/// 2. Name transformation heuristics (strip `-dev`, `lib` prefix, version digits)
/// 3. Hardcoded fallback mapping
///
/// Returns a vec of `(linux_name, brew_formula, skipped)` tuples. `linux_name`
/// is preserved for error reporting. `skipped` is `true` for packages that are
/// Linux-only and have no macOS equivalent (e.g. `musl-dev`, `ca-certificates`).
pub async fn map_linux_packages(
    packages: &[&str],
    distro: &str,
    cache_dir: &Path,
) -> Vec<(String, String, bool)> {
    let map = load_or_fetch_package_map(distro, packages, cache_dir).await;

    packages
        .iter()
        .map(|&pkg| {
            let (brew, skipped) = resolve_single_package(pkg, &map);
            (pkg.to_string(), brew, skipped)
        })
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
//
// Force-skipped Linux-only names. Some entries here (gcc, make, gnupg,
// procps) do exist as brew formulas, but installing them on macOS would
// shadow Apple's system toolchain or duplicate already-present binaries.
// We intentionally drop these from the install set rather than mapping
// them through brew. If a user genuinely needs the brew version they
// can ask for it by brew name explicitly (e.g. `brew install gcc@14`).
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
    // Pass-through: when RepoSources is unreachable AND name transforms
    // miss, hand the Linux name to install_with_deps as-is. Phase 2's
    // dynamic resolution in scripts/generate-package-maps.py:pick_winner
    // produces the right brew formula name (including `@MAJOR` pins like
    // openssl@3); the prior hardcoded fallback table (libssl-dev →
    // openssl, python3 → python@3, etc.) actively contradicted that
    // output and would 404 for alias-only names like python@3.
    (pkg.to_string(), false)
}

/// Compute the shard key for a package name.
///
/// Mirrors the sharding rule used by `scripts/generate-package-maps.py`:
/// the lowercase first ASCII letter if `a..z`, otherwise `_misc`. Empty
/// strings also map to `_misc`.
fn shard_key(name: &str) -> &'static str {
    let first = name.chars().next().map(|c| c.to_ascii_lowercase());
    match first {
        Some(c) if c.is_ascii_lowercase() => match c {
            'a' => "a",
            'b' => "b",
            'c' => "c",
            'd' => "d",
            'e' => "e",
            'f' => "f",
            'g' => "g",
            'h' => "h",
            'i' => "i",
            'j' => "j",
            'k' => "k",
            'l' => "l",
            'm' => "m",
            'n' => "n",
            'o' => "o",
            'p' => "p",
            'q' => "q",
            'r' => "r",
            's' => "s",
            't' => "t",
            'u' => "u",
            'v' => "v",
            'w' => "w",
            'x' => "x",
            'y' => "y",
            'z' => "z",
            _ => "_misc",
        },
        _ => "_misc",
    }
}

/// Load the package mappings for a distro, fetching the relevant per-letter
/// shards from `RepoSources` if not cached or stale.
///
/// Cascading lookup: for each shard letter implied by `packages`, fetch
/// both `common/<shard>.json` (cross-distro consensus) AND
/// `<distro>/<shard>.json` (distro-specific, more authoritative). Merge
/// with **per-distro winning** on conflict — distro-specific is a tighter
/// answer when known; common is the fallback for names that distro
/// doesn't have (e.g. `libssl-dev` on centos).
async fn load_or_fetch_package_map(
    distro: &str,
    packages: &[&str],
    cache_dir: &Path,
) -> HashMap<String, String> {
    let cache_root = cache_dir.join(PACKAGE_MAP_CACHE_SUBDIR);
    let common_cache_dir = cache_root.join("common");
    let distro_cache_dir = cache_root.join(distro);

    let mut shards: HashSet<&'static str> = HashSet::new();
    for pkg in packages {
        shards.insert(shard_key(pkg));
    }

    let mut common_merged: HashMap<String, String> = HashMap::new();
    let mut distro_merged: HashMap<String, String> = HashMap::new();

    for shard in shards {
        if let Some(map) = fetch_or_load_shard("common", &common_cache_dir, shard).await {
            common_merged.extend(map);
        }
        if let Some(map) = fetch_or_load_shard(distro, &distro_cache_dir, shard).await {
            distro_merged.extend(map);
        }
    }

    // Per-distro values WIN over common — distro is more specific. Insertion
    // order: common first, distro overrides on conflict.
    let mut merged = common_merged;
    merged.extend(distro_merged);
    merged
}

/// Fetch one shard for a label (`<distro>` or `"common"`), with on-disk
/// cache + stale fallback. Returns `None` only when there's no fresh
/// fetch, no usable stale cache — i.e. truly nothing available.
async fn fetch_or_load_shard(
    label: &str,
    cache_dir: &Path,
    shard: &str,
) -> Option<HashMap<String, String>> {
    let cache_path = cache_dir.join(format!("{shard}.json"));

    // 1. Fresh local cache.
    if let Ok(meta) = tokio::fs::metadata(&cache_path).await {
        if let Ok(modified) = meta.modified() {
            let age = modified
                .elapsed()
                .unwrap_or(std::time::Duration::from_secs(u64::MAX));
            if age.as_secs() < PACKAGE_MAP_CACHE_TTL_SECS {
                if let Some(map) = read_cached_map(&cache_path).await {
                    debug!(
                        "Using cached package map for {label}/{shard} ({} mappings, age {}s)",
                        map.len(),
                        age.as_secs()
                    );
                    return Some(map);
                }
            }
        }
    }

    // 2. Fetch from RepoSources.
    let url = format!("{REPO_SOURCES_BASE}/{label}/{shard}.json");
    debug!("Fetching package map shard from {url}");

    match fetch_package_map(&url).await {
        Ok(map_file) => {
            info!(
                "Fetched {} package mappings for {label}/{shard} from RepoSources",
                map_file.mappings.len()
            );
            if let Err(e) = write_cached_map(cache_dir, &cache_path, &map_file).await {
                warn!("Failed to cache package map for {label}/{shard}: {e}");
            }
            Some(map_file.mappings)
        }
        Err(e) => {
            debug!("Failed to fetch package map for {label}/{shard}: {e}");

            // 3. Stale cache fallback.
            if let Some(map) = read_cached_map(&cache_path).await {
                info!(
                    "Using stale cached package map for {label}/{shard} ({} mappings)",
                    map.len()
                );
                return Some(map);
            }

            // 4. Nothing — caller (load_or_fetch_package_map) will quietly
            //    proceed with whatever's already merged. A blanket warning
            //    here would fire for every shard miss in `common/` even
            //    when the per-distro shard succeeds; let the resolver-level
            //    miss path produce the user-facing message.
            None
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

/// Compute the HMAC-SHA256 signature header for a `RepoSourceSyncer` POST.
///
/// Returns the value to send in the `x-reposync-signature` header
/// (`sha256=<hex>`). The server recomputes the same HMAC over the raw body
/// and compares with `crypto.timingSafeEqual` — see
/// `BlackLeafDigital/functions/RepoSourceSyncer/src/helpers.ts`.
fn compute_reposync_signature(secret: &str, body: &[u8]) -> String {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
    mac.update(body);
    let bytes = mac.finalize().into_bytes();
    format!("sha256={}", hex::encode(bytes))
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

    /// Live smoke test against the published `RepoSources` JSON.
    ///
    /// Hits live `RepoSources` (`https://zachhandley.github.io/RepoSources/maps/...`)
    /// and asserts the canonical Homebrew names. Stays `#[ignore]` because
    /// it's network-bound and brew rotates default majors yearly; pattern
    /// match the verify-reposources.yml globs (`openssl@3`, `node@<MAJOR>`).
    #[tokio::test]
    #[ignore = "live network test; runs against published RepoSources"]
    async fn test_map_linux_packages_live_reposources() {
        let tmp = std::env::temp_dir().join("zlayer-test-pkg-map-live");
        let _ = tokio::process::Command::new("chmod")
            .args(["-R", "u+w"])
            .arg(&tmp)
            .status()
            .await;
        let _ = tokio::process::Command::new("rm")
            .args(["-rf"])
            .arg(&tmp)
            .status()
            .await;

        let result =
            map_linux_packages(&["curl", "libssl-dev", "musl-dev"], "debian_12", &tmp).await;
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].0, "curl");
        assert_eq!(result[0].1, "curl");
        assert!(!result[0].2);
        assert_eq!(result[1].0, "libssl-dev");
        assert_eq!(result[1].1, "openssl@3");
        assert!(!result[1].2);
        assert_eq!(result[2].0, "musl-dev");
        assert!(result[2].2);

        let _ = tokio::process::Command::new("chmod")
            .args(["-R", "u+w"])
            .arg(&tmp)
            .status()
            .await;
        let _ = tokio::process::Command::new("rm")
            .args(["-rf"])
            .arg(&tmp)
            .status()
            .await;
    }

    /// Hermetic test: writes per-shard fixture `PackageMapFile`s to disk, then
    /// verifies `map_linux_packages` returns the expected
    /// `(linux_name, brew, skipped)` triples. No network. Always runs in CI —
    /// this is the regression guard for the sharded layout.
    async fn write_shard(
        label_dir: &Path,
        label: &str,
        shard: &str,
        mappings: HashMap<String, String>,
    ) {
        tokio::fs::create_dir_all(label_dir)
            .await
            .expect("create label dir");
        let fixture = PackageMapFile {
            metadata: PackageMapMetadata {
                generated_at: "2026-04-29T00:00:00Z".to_string(),
                source: "test-fixture".to_string(),
                distro: label.to_string(),
                shard: Some(shard.to_string()),
                total_mappings: mappings.len(),
            },
            mappings,
        };
        let json = serde_json::to_string_pretty(&fixture).expect("serialize fixture");
        tokio::fs::write(label_dir.join(format!("{shard}.json")), json)
            .await
            .expect("write shard fixture");
    }

    #[tokio::test]
    async fn test_map_linux_packages_with_seeded_cache() {
        let tmp = tempfile::tempdir().expect("create tmpdir");
        let cache_dir = tmp.path().to_path_buf();
        let cache_root = cache_dir.join(PACKAGE_MAP_CACHE_SUBDIR);
        let distro_dir = cache_root.join("debian_12");

        let mut c_mappings = HashMap::new();
        c_mappings.insert("curl".to_string(), "curl".to_string());
        write_shard(&distro_dir, "debian_12", "c", c_mappings).await;

        let mut l_mappings = HashMap::new();
        l_mappings.insert("libssl-dev".to_string(), "openssl@3".to_string());
        write_shard(&distro_dir, "debian_12", "l", l_mappings).await;

        let mut n_mappings = HashMap::new();
        n_mappings.insert("nodejs".to_string(), "node@24".to_string());
        write_shard(&distro_dir, "debian_12", "n", n_mappings).await;

        let result = map_linux_packages(
            &["curl", "libssl-dev", "musl-dev", "nodejs"],
            "debian_12",
            &cache_dir,
        )
        .await;

        assert_eq!(result.len(), 4);
        assert_eq!(result[0].0, "curl");
        assert_eq!(result[0].1, "curl");
        assert!(!result[0].2);
        assert_eq!(result[1].0, "libssl-dev");
        assert_eq!(result[1].1, "openssl@3");
        assert!(!result[1].2);
        assert_eq!(result[2].0, "musl-dev");
        assert!(result[2].2);
        assert_eq!(result[3].0, "nodejs");
        assert_eq!(result[3].1, "node@24");
        assert!(!result[3].2);
    }

    /// Cascading lookup: a name absent from the per-distro shard but
    /// present in `common/` should resolve via the common fallback.
    /// Models the real-world case where centos's `l.json` doesn't list
    /// `libssl-dev` (debian-only name) but `common/l.json` does.
    #[tokio::test]
    async fn test_map_linux_packages_falls_back_to_common() {
        let tmp = tempfile::tempdir().expect("create tmpdir");
        let cache_dir = tmp.path().to_path_buf();
        let cache_root = cache_dir.join(PACKAGE_MAP_CACHE_SUBDIR);
        let distro_dir = cache_root.join("centos_8");
        let common_dir = cache_root.join("common");

        // centos_8 has its own openssl-devel mapping but NOT libssl-dev.
        let mut distro_o = HashMap::new();
        distro_o.insert("openssl-devel".to_string(), "openssl@3".to_string());
        write_shard(&distro_dir, "centos_8", "o", distro_o).await;

        // common/l.json carries the cross-distro libssl-dev mapping, sourced
        // from debian's view.
        let mut common_l = HashMap::new();
        common_l.insert("libssl-dev".to_string(), "openssl@3".to_string());
        write_shard(&common_dir, "common", "l", common_l).await;

        let result =
            map_linux_packages(&["openssl-devel", "libssl-dev"], "centos_8", &cache_dir).await;

        assert_eq!(result.len(), 2);
        assert_eq!(result[0].0, "openssl-devel");
        assert_eq!(result[0].1, "openssl@3");
        assert!(!result[0].2);
        // libssl-dev resolves via common, even though centos_8's `l` shard
        // wasn't seeded.
        assert_eq!(result[1].0, "libssl-dev");
        assert_eq!(result[1].1, "openssl@3");
        assert!(!result[1].2);
    }

    /// Cascading lookup: when both per-distro AND `common/` define the
    /// same name with different values, per-distro wins. Per-distro is
    /// the more-specific source of truth for that distro.
    #[tokio::test]
    async fn test_map_linux_packages_distro_overrides_common() {
        let tmp = tempfile::tempdir().expect("create tmpdir");
        let cache_dir = tmp.path().to_path_buf();
        let cache_root = cache_dir.join(PACKAGE_MAP_CACHE_SUBDIR);
        let distro_dir = cache_root.join("alpine_3_20");
        let common_dir = cache_root.join("common");

        let mut distro_p = HashMap::new();
        distro_p.insert("python3".to_string(), "python@3.13".to_string());
        write_shard(&distro_dir, "alpine_3_20", "p", distro_p).await;

        let mut common_p = HashMap::new();
        common_p.insert("python3".to_string(), "python@3.14".to_string());
        write_shard(&common_dir, "common", "p", common_p).await;

        let result = map_linux_packages(&["python3"], "alpine_3_20", &cache_dir).await;

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].0, "python3");
        // Per-distro wins (3.13), not common's 3.14.
        assert_eq!(result[0].1, "python@3.13");
        assert!(!result[0].2);
    }

    // -- HMAC signature test --

    #[test]
    fn test_compute_reposync_signature_matches_node_crypto() {
        // Reference vector — re-derive in Node with:
        //   crypto.createHmac('sha256', 'test-secret').update('{"foo":"bar"}').digest('hex')
        let sig = compute_reposync_signature("test-secret", br#"{"foo":"bar"}"#);
        assert_eq!(
            sig,
            "sha256=9b1abf7d901bda91325d00f6b397fb0dc257937939b27d4dc67848ab9e08f6c0"
        );
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
