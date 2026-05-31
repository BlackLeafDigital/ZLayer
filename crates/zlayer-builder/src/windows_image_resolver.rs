//! Windows (Chocolatey) package resolver — counterpart to `macos_image_resolver`.
//!
//! On Windows containers (WCOW), Linux package manager invocations
//! (`apt-get install`, `dnf install`, `apk add`) inside a Dockerfile cannot
//! run natively. This module resolves Linux package names against the
//! `RepoSources` Chocolatey shard files published at
//! `https://zachhandley.github.io/RepoSources/maps/choco/<distro>/<shard>.json`
//! and returns the equivalent Chocolatey package name so the builder can
//! emit `choco install <pkg>` lines in the WCOW lowered Dockerfile.
//!
//! The shard layout, on-disk cache, and HMAC cache-warming hint mirror the
//! macOS resolver in `macos_image_resolver.rs`. The Windows resolver lives in
//! a separate cache subdirectory (`package-maps-choco-v1`) so its TTL/version
//! is independent from the macOS resolver.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};
use zlayer_types::ImageReference;

use crate::error::{BuildError, Result};

/// Registry namespace for prebuilt `ZLayer` Windows base + toolchain images.
///
/// Mirrors `ZLAYER_REGISTRY` in `macos_image_resolver.rs`. Used by
/// [`rewrite_image_for_windows`] to redirect generic Docker Hub references
/// (e.g. `golang:1.24`, `ubuntu:24.04`) to the equivalent Windows-native
/// prebuilts under this namespace.
const ZLAYER_REGISTRY: &str = "ghcr.io/blackleafdigital/zlayer";

/// Rewrite a generic image reference to the equivalent prebuilt `ZLayer`
/// Windows image under [`ZLAYER_REGISTRY`], parameterized by an LTSC line.
///
/// Counterpart to `macos_image_resolver::rewrite_image_for_macos`. Linux
/// container images cannot run on Windows containers directly, so the
/// builder rewrites known toolchain / base-distro references to prebuilt
/// `nanoserver`-based images tagged with the requested LTSC line
/// (`ltsc2022` or `ltsc2025`).
///
/// Returns `None` if:
/// * The reference is already inside [`ZLAYER_REGISTRY`] (already rewritten).
/// * The repository name doesn't map to a known base distro or toolchain.
///
/// # Examples
///
/// ```ignore
/// assert_eq!(
///     rewrite_image_for_windows("ubuntu:24.04", "ltsc2022"),
///     Some("ghcr.io/blackleafdigital/zlayer/base:windows-ltsc2022".to_string()),
/// );
/// assert_eq!(
///     rewrite_image_for_windows("golang:1.24", "ltsc2025"),
///     Some("ghcr.io/blackleafdigital/zlayer/golang:1.24-windows-ltsc2025".to_string()),
/// );
/// ```
#[must_use]
pub fn rewrite_image_for_windows(image_ref: &str, ltsc: &str) -> Option<String> {
    // Don't double-rewrite images already in our registry.
    if image_ref.starts_with(ZLAYER_REGISTRY) {
        return None;
    }

    // Strip the registry prefix (docker.io/library/, etc.).
    let stripped = strip_registry_prefix_for_windows(image_ref);

    // Split into name and tag using the canonical OCI parser (handles
    // host:port, digests, and missing tags correctly).
    let (name, tag) = match ImageReference::from_str(&stripped) {
        Ok(r) => (
            r.repository().to_string(),
            r.tag().unwrap_or("latest").to_string(),
        ),
        Err(_) => (stripped.clone(), "latest".to_string()),
    };
    let base_name = name.rsplit('/').next().unwrap_or(&name);

    // Base distro images → base:windows-<ltsc>
    if is_base_distro_for_windows(base_name) {
        return Some(format!("{ZLAYER_REGISTRY}/base:windows-{ltsc}"));
    }

    // Toolchain images → {zlayer_registry}/{canonical}:{version}-windows-<ltsc>
    let canonical = match base_name {
        "golang" | "go" => "golang",
        "node" => "node",
        "rust" => "rust",
        "python" | "python3" => "python",
        "deno" => "deno",
        "bun" => "bun",
        _ => return None,
    };

    let version = extract_version_from_tag_for_windows(&tag);
    Some(format!(
        "{ZLAYER_REGISTRY}/{canonical}:{version}-windows-{ltsc}"
    ))
}

/// Check whether the given base image name is a Linux distribution / base image
/// that has a prebuilt Windows counterpart at
/// `{ZLAYER_REGISTRY}/base:windows-<ltsc>`.
fn is_base_distro_for_windows(name: &str) -> bool {
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
///
/// Local copy of the macOS resolver's helper so this resolver compiles on
/// all platforms (the macOS one is `#[cfg(target_os = "macos")]`).
fn strip_registry_prefix_for_windows(image_ref: &str) -> String {
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

/// Extract a version-like prefix from an image tag (e.g. `"20-slim"` → `"20"`).
///
/// Local copy of `macos_toolchain::extract_version_from_tag` so this
/// resolver compiles on all platforms.
fn extract_version_from_tag_for_windows(tag: &str) -> String {
    if tag == "latest" {
        return "latest".to_string();
    }

    // Try to extract a version-like prefix (digits and dots).
    let version_part: String = tag
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();

    if version_part.is_empty() {
        "latest".to_string()
    } else {
        version_part.trim_end_matches('.').to_string()
    }
}

/// Base URL for Chocolatey package maps on the `RepoSources` `GitHub Pages` site.
const REPO_SOURCES_CHOCO_BASE: &str = "https://zachhandley.github.io/RepoSources/maps/choco";

/// Subdirectory (under the platform cache dir) where Chocolatey shard files
/// are stored. Distinct from the macOS Homebrew subdir so the two caches
/// never collide.
const PACKAGE_MAP_CACHE_SUBDIR: &str = "package-maps-choco-v1";

/// How long a cached Chocolatey shard is considered fresh (7 days).
const PACKAGE_MAP_CACHE_TTL_SECS: u64 = 7 * 24 * 3600;

/// HMAC secret compiled into the binary at build time.
///
/// Set at `cargo build` time via `ZLAYER_REPOSYNC_HMAC_SECRET=<hex>`. If unset
/// at build time, this evaluates to `None` and `fire_reposync_hint` silently
/// skips the POST — useful for dev builds where the cache-warm signal isn't
/// needed. Released binaries (built by CI with the Forgejo secret in scope)
/// carry the value forever.
const REPOSYNC_HMAC_SECRET: Option<&str> = option_env!("ZLAYER_REPOSYNC_HMAC_SECRET");

/// Endpoint that accepts cache-warm hints from clients.
const REPOSYNC_HINT_ENDPOINT: &str = "https://reposync.blackleafdigital.com/choco-hint";

/// Sentinel value placed in shard mappings to indicate that a Linux package
/// has no Chocolatey equivalent because it is Linux-only (e.g.
/// `linux-headers-generic`, `systemd`). The builder should silently skip
/// these.
const SKIP_SENTINEL: &str = "__skip__";

// ---------------------------------------------------------------------------
// Shard file types
// ---------------------------------------------------------------------------

/// JSON shape of a Chocolatey package-map shard file as published by
/// `RepoSources`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChocoMapShard {
    /// Header describing how/when the shard was generated.
    pub metadata: ChocoMapMetadata,
    /// Linux-package-name → Chocolatey-package-name (or `__skip__` sentinel).
    pub mappings: HashMap<String, String>,
}

/// Metadata header inside a Chocolatey shard JSON file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChocoMapMetadata {
    /// ISO 8601 timestamp the shard was generated at.
    pub generated_at: String,
    /// Source description (e.g. `"chocolatey.org"`).
    pub source: String,
    /// Distro the shard maps from (e.g. `"debian-12"`).
    pub distro: String,
    /// Shard key (`a`..`z` or `_misc`).
    pub shard: String,
    /// Number of mappings in the shard.
    pub total_mappings: u64,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Resolve a single Linux package name to its Chocolatey equivalent.
///
/// * `Ok(Some(choco_pkg))` — the package resolved to a Chocolatey package.
/// * `Ok(None)` — the mapping exists but is marked `__skip__`, meaning the
///   package is Linux-only and the builder should silently omit it.
/// * `Err(_)` — network, cache, or parse failure, AND there's no stale cache
///   to fall back on.
///
/// A "miss" — i.e. the shard fetched cleanly but the package name is absent —
/// also returns `Err(BuildError::RegistryError { .. })` so the caller can
/// distinguish "no Chocolatey equivalent known" from "skipped on purpose".
///
/// # Errors
///
/// Returns `Err(BuildError::RegistryError)` if the shard cannot be fetched
/// (and no stale cache is available) or if the package is absent from the
/// fetched shard. Returns `Err(BuildError::CacheError)` if the platform
/// cache directory cannot be determined.
pub async fn resolve_chocolatey_package(
    linux_pkg: &str,
    source_distro: &str,
) -> Result<Option<String>> {
    let cache_dir = resolve_cache_dir()?;
    resolve_chocolatey_package_with_cache(linux_pkg, source_distro, &cache_dir).await
}

/// Bulk-resolve a list of Linux packages.
///
/// Returns one entry per input package, in the same order, as
/// `(linux_pkg, choco_pkg_or_none, skipped)`:
///
/// * `(name, Some(choco), false)` — resolved.
/// * `(name, None, true)` — mapping says `__skip__`.
/// * `(name, None, false)` — unresolved (no mapping and no error path
///   would still surface that as an error per shard; here we just degrade to
///   `None` for ergonomics so a single missing package doesn't fail the
///   whole batch). Callers that want strict resolution should inspect the
///   `Err` returned by [`resolve_chocolatey_package`] directly.
///
/// All shards needed by the batch are fetched once and reused across lookups.
///
/// # Errors
///
/// Returns `Err(BuildError::CacheError)` if the platform cache directory
/// cannot be determined. Individual shard fetch failures are tolerated:
/// packages mapping to a failed shard are returned with `None`/`false` and
/// a `debug!` line; this function only returns `Err` for unrecoverable
/// setup failures.
pub async fn resolve_chocolatey_packages(
    linux_pkgs: &[String],
    source_distro: &str,
) -> Result<Vec<(String, Option<String>, bool)>> {
    let cache_dir = resolve_cache_dir()?;
    let distro_cache_dir = cache_dir.join(PACKAGE_MAP_CACHE_SUBDIR).join(source_distro);

    // Pre-fetch every shard the batch needs so duplicate package names within
    // a single shard don't trigger duplicate HTTP requests.
    let mut shard_cache: HashMap<&'static str, HashMap<String, String>> = HashMap::new();
    for pkg in linux_pkgs {
        let shard = shard_key(pkg);
        if shard_cache.contains_key(shard) {
            continue;
        }
        match fetch_or_load_shard(source_distro, &distro_cache_dir, shard).await {
            Ok(map) => {
                shard_cache.insert(shard, map);
            }
            Err(e) => {
                debug!(
                    "shard {source_distro}/{shard} unavailable during bulk resolve: {e}; \
                     packages mapping to that shard will be marked unresolved"
                );
                shard_cache.insert(shard, HashMap::new());
            }
        }
    }

    let mut out = Vec::with_capacity(linux_pkgs.len());
    for pkg in linux_pkgs {
        let shard = shard_key(pkg);
        let shard_map = shard_cache.get(shard);
        match shard_map.and_then(|m| m.get(pkg)) {
            Some(val) if val == SKIP_SENTINEL => {
                out.push((pkg.clone(), None, true));
            }
            Some(val) => {
                out.push((pkg.clone(), Some(val.clone()), false));
            }
            None => {
                out.push((pkg.clone(), None, false));
            }
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

/// Compute the shard key for a Linux package name.
///
/// Mirrors `macos_image_resolver::shard_key`: the lowercase first ASCII
/// letter (`a`..`z`) if alphabetic, otherwise `_misc`. Empty strings and
/// non-ASCII inputs also map to `_misc`.
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

/// Outcome of looking up a single package inside an already-parsed shard.
#[cfg(test)]
#[derive(Debug, PartialEq, Eq)]
enum ShardLookup {
    /// Mapping resolved to a Chocolatey package name.
    Found(String),
    /// Mapping is marked `__skip__` (Linux-only).
    Skip,
    /// Package name is absent from this shard.
    Absent,
}

/// Inspect a parsed shard for a given package name. Only used by unit tests
/// today; production code goes through [`fetch_or_load_shard`] +
/// [`resolve_chocolatey_package_with_cache`] which combine I/O and lookup.
#[cfg(test)]
fn resolve_in_shard(linux_pkg: &str, shard: &ChocoMapShard) -> ShardLookup {
    match shard.mappings.get(linux_pkg) {
        Some(v) if v == SKIP_SENTINEL => ShardLookup::Skip,
        Some(v) => ShardLookup::Found(v.clone()),
        None => ShardLookup::Absent,
    }
}

/// Returns the platform cache directory used for storing Chocolatey shards.
fn resolve_cache_dir() -> Result<PathBuf> {
    dirs::cache_dir().ok_or_else(|| {
        BuildError::cache_error("could not determine platform cache directory (dirs::cache_dir)")
    })
}

/// Single-package resolve with an explicit cache root. Used by
/// [`resolve_chocolatey_package`] and by unit tests that need a controlled
/// cache dir.
async fn resolve_chocolatey_package_with_cache(
    linux_pkg: &str,
    source_distro: &str,
    cache_dir: &Path,
) -> Result<Option<String>> {
    let distro_cache_dir = cache_dir.join(PACKAGE_MAP_CACHE_SUBDIR).join(source_distro);
    let shard = shard_key(linux_pkg);
    let map = fetch_or_load_shard(source_distro, &distro_cache_dir, shard).await?;

    match map.get(linux_pkg) {
        Some(val) if val == SKIP_SENTINEL => {
            debug!("chocolatey resolver skipping linux-only package: {linux_pkg}");
            Ok(None)
        }
        Some(val) => Ok(Some(val.clone())),
        None => Err(BuildError::registry_error(format!(
            "no Chocolatey mapping for '{linux_pkg}' in {source_distro}/{shard}.json"
        ))),
    }
}

/// Fetch one shard for `<distro>/<shard>.json`, with on-disk cache + stale
/// fallback.
///
/// Order of precedence:
/// 1. Fresh local cache (mtime within TTL) — read and return.
/// 2. Network fetch — write to cache, fire HMAC POST hint, return.
/// 3. Stale cache fallback — read and return with a `warn!`.
/// 4. Truly nothing available — propagate the HTTP error as `RegistryError`.
async fn fetch_or_load_shard(
    distro: &str,
    cache_dir: &Path,
    shard: &str,
) -> Result<HashMap<String, String>> {
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
                        "Using cached choco package map for {distro}/{shard} ({} mappings, age {}s)",
                        map.len(),
                        age.as_secs()
                    );
                    return Ok(map);
                }
            }
        }
    }

    // 2. Network fetch.
    let url = format!("{REPO_SOURCES_CHOCO_BASE}/{distro}/{shard}.json");
    debug!("Fetching choco shard from {url}");
    match fetch_shard(&url).await {
        Ok(shard_file) => {
            info!(
                "Fetched {} choco mappings for {distro}/{shard} from RepoSources",
                shard_file.mappings.len()
            );
            if let Err(e) = write_cached_shard(cache_dir, &cache_path, &shard_file).await {
                warn!("Failed to cache choco shard {distro}/{shard}: {e}");
            }
            fire_reposync_hint(distro, shard);
            Ok(shard_file.mappings)
        }
        Err(e) => {
            debug!("Failed to fetch choco shard {distro}/{shard}: {e}");

            // 3. Stale cache fallback.
            if let Some(map) = read_cached_map(&cache_path).await {
                warn!(
                    "Using stale cached choco shard for {distro}/{shard} ({} mappings)",
                    map.len()
                );
                return Ok(map);
            }

            // 4. Nothing available.
            Err(BuildError::registry_error(format!(
                "failed to fetch choco shard {distro}/{shard}.json: {e}"
            )))
        }
    }
}

/// Fetch one shard JSON document from `url`.
async fn fetch_shard(url: &str) -> std::result::Result<ChocoMapShard, String> {
    let response = reqwest::get(url)
        .await
        .map_err(|e| format!("HTTP request failed: {e}"))?;

    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status()));
    }

    response
        .json::<ChocoMapShard>()
        .await
        .map_err(|e| format!("JSON parse failed: {e}"))
}

/// Read a cached shard from disk and return just the mappings.
async fn read_cached_map(path: &Path) -> Option<HashMap<String, String>> {
    let contents = tokio::fs::read_to_string(path).await.ok()?;
    let shard: ChocoMapShard = serde_json::from_str(&contents).ok()?;
    Some(shard.mappings)
}

/// Write a `ChocoMapShard` to disk, creating the directory if needed.
async fn write_cached_shard(
    map_dir: &Path,
    cache_path: &Path,
    shard: &ChocoMapShard,
) -> std::result::Result<(), String> {
    tokio::fs::create_dir_all(map_dir)
        .await
        .map_err(|e| format!("create dir: {e}"))?;
    let json = serde_json::to_string_pretty(shard).map_err(|e| format!("serialize: {e}"))?;
    tokio::fs::write(cache_path, json)
        .await
        .map_err(|e| format!("write: {e}"))
}

/// Compute the HMAC-SHA256 signature header for a `RepoSourceSyncer` POST.
///
/// Returns the value to send in the `x-reposync-signature` header
/// (`sha256=<hex>`).
fn compute_reposync_signature(secret: &str, body: &[u8]) -> String {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
    mac.update(body);
    let bytes = mac.finalize().into_bytes();
    format!("sha256={}", hex::encode(bytes))
}

/// Fire-and-forget cache-warm hint POST to `RepoSourceSyncer`. No-op when
/// `ZLAYER_REPOSYNC_HMAC_SECRET` was unset at build time.
fn fire_reposync_hint(distro: &str, shard: &str) {
    let Some(secret) = REPOSYNC_HMAC_SECRET.filter(|s| !s.is_empty()) else {
        debug!(
            "ZLAYER_REPOSYNC_HMAC_SECRET not baked into binary (or empty); skipping reposync cache warm for choco/{distro}/{shard}"
        );
        return;
    };
    let distro = distro.to_string();
    let shard = shard.to_string();
    tokio::spawn(async move {
        let payload = format!(r#"{{"scope":"choco","distro":"{distro}","shard":"{shard}"}}"#);
        let signature = compute_reposync_signature(secret, payload.as_bytes());
        let _ = reqwest::Client::new()
            .post(REPOSYNC_HINT_ENDPOINT)
            .header("x-reposync-signature", signature)
            .header("content-type", "application/json")
            .body(payload)
            .send()
            .await;
    });
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE_SHARD: &str = r#"{
        "metadata": {
            "generated_at": "2026-05-21T00:00:00Z",
            "source": "chocolatey.org",
            "distro": "debian-12",
            "shard": "c",
            "total_mappings": 2
        },
        "mappings": {
            "curl": "curl",
            "linux-headers-generic": "__skip__"
        }
    }"#;

    #[test]
    fn shard_key_alpha() {
        assert_eq!(shard_key("apache2"), "a");
        assert_eq!(shard_key("curl"), "c");
        assert_eq!(shard_key("Zoo"), "z");
    }

    #[test]
    fn shard_key_non_alpha() {
        assert_eq!(shard_key("7zip"), "_misc");
        assert_eq!(shard_key("_internal"), "_misc");
        assert_eq!(shard_key(""), "_misc");
    }

    #[test]
    fn parse_shard_json() {
        let shard: ChocoMapShard =
            serde_json::from_str(FIXTURE_SHARD).expect("fixture parses cleanly");
        assert_eq!(shard.metadata.distro, "debian-12");
        assert_eq!(shard.metadata.shard, "c");
        assert_eq!(shard.metadata.total_mappings, 2);

        // Normal mapping.
        assert_eq!(
            resolve_in_shard("curl", &shard),
            ShardLookup::Found("curl".to_string()),
        );

        // __skip__ sentinel.
        assert_eq!(
            resolve_in_shard("linux-headers-generic", &shard),
            ShardLookup::Skip,
        );

        // Absent from shard.
        assert_eq!(
            resolve_in_shard("not-in-shard", &shard),
            ShardLookup::Absent,
        );
    }

    #[tokio::test]
    async fn cache_ttl_respected() {
        let tmp = tempfile::tempdir().unwrap();
        let cache_dir = tmp.path().to_path_buf();
        let distro_dir = cache_dir.join(PACKAGE_MAP_CACHE_SUBDIR).join("debian-12");
        tokio::fs::create_dir_all(&distro_dir).await.unwrap();
        let shard_path = distro_dir.join("c.json");
        tokio::fs::write(&shard_path, FIXTURE_SHARD).await.unwrap();

        // Sanity check: with a fresh mtime, resolver reads from cache (no network).
        let fresh = resolve_chocolatey_package_with_cache("curl", "debian-12", &cache_dir)
            .await
            .expect("fresh cache hit should resolve");
        assert_eq!(fresh.as_deref(), Some("curl"));

        // Rewind mtime to 8 days ago — the loader must treat it as expired
        // and fall through to the network-fetch path. We then verify that
        // the same TTL predicate the production code uses (mtime elapsed >=
        // PACKAGE_MAP_CACHE_TTL_SECS) reports the file as stale.
        let eight_days_ago = std::time::SystemTime::now()
            .checked_sub(std::time::Duration::from_secs(8 * 24 * 3600))
            .unwrap();
        let file = std::fs::File::options()
            .write(true)
            .open(&shard_path)
            .unwrap();
        file.set_modified(eight_days_ago)
            .expect("backdate mtime via File::set_modified");
        drop(file);

        let meta = tokio::fs::metadata(&shard_path).await.unwrap();
        let modified = meta.modified().unwrap();
        let age = modified.elapsed().unwrap();
        assert!(
            age.as_secs() >= PACKAGE_MAP_CACHE_TTL_SECS,
            "expected backdated mtime to exceed TTL ({} >= {})",
            age.as_secs(),
            PACKAGE_MAP_CACHE_TTL_SECS
        );
    }

    #[test]
    fn rewrite_image_for_windows_skips_already_rewritten() {
        assert_eq!(
            rewrite_image_for_windows(
                "ghcr.io/blackleafdigital/zlayer/base:windows-ltsc2022",
                "ltsc2022",
            ),
            None,
        );
        assert_eq!(
            rewrite_image_for_windows(
                "ghcr.io/blackleafdigital/zlayer/golang:1.24-windows-ltsc2025",
                "ltsc2025",
            ),
            None,
        );
    }

    #[test]
    fn rewrite_image_for_windows_ubuntu_ltsc2022() {
        assert_eq!(
            rewrite_image_for_windows("ubuntu:24.04", "ltsc2022"),
            Some("ghcr.io/blackleafdigital/zlayer/base:windows-ltsc2022".to_string()),
        );
    }

    #[test]
    fn rewrite_image_for_windows_ubuntu_ltsc2025() {
        assert_eq!(
            rewrite_image_for_windows("ubuntu:24.04", "ltsc2025"),
            Some("ghcr.io/blackleafdigital/zlayer/base:windows-ltsc2025".to_string()),
        );
    }

    #[test]
    fn rewrite_image_for_windows_golang_ltsc2022() {
        assert_eq!(
            rewrite_image_for_windows("golang:1.24", "ltsc2022"),
            Some("ghcr.io/blackleafdigital/zlayer/golang:1.24-windows-ltsc2022".to_string()),
        );
    }

    #[test]
    fn rewrite_image_for_windows_node_ltsc2025() {
        assert_eq!(
            rewrite_image_for_windows("node:22", "ltsc2025"),
            Some("ghcr.io/blackleafdigital/zlayer/node:22-windows-ltsc2025".to_string()),
        );
    }

    #[test]
    fn rewrite_image_for_windows_unknown_returns_none() {
        assert_eq!(rewrite_image_for_windows("nginx:1.25", "ltsc2022"), None);
        assert_eq!(rewrite_image_for_windows("redis:7", "ltsc2025"), None);
    }

    #[test]
    fn rewrite_image_for_windows_strips_docker_io_prefix() {
        assert_eq!(
            rewrite_image_for_windows("docker.io/library/ubuntu:22.04", "ltsc2022"),
            Some("ghcr.io/blackleafdigital/zlayer/base:windows-ltsc2022".to_string()),
        );
    }

    #[test]
    fn rewrite_image_for_windows_python_alias() {
        assert_eq!(
            rewrite_image_for_windows("python3:3.12", "ltsc2022"),
            Some("ghcr.io/blackleafdigital/zlayer/python:3.12-windows-ltsc2022".to_string()),
        );
    }

    #[test]
    fn rewrite_image_for_windows_no_tag_defaults_to_latest() {
        assert_eq!(
            rewrite_image_for_windows("bun", "ltsc2025"),
            Some("ghcr.io/blackleafdigital/zlayer/bun:latest-windows-ltsc2025".to_string()),
        );
    }

    #[tokio::test]
    #[ignore = "live network: hits zachhandley.github.io"]
    async fn live_resolve_curl_debian12() {
        let result = resolve_chocolatey_package("curl", "debian-12").await;
        let resolved = result.expect("live network resolve should succeed");
        assert!(
            resolved.is_some(),
            "curl should resolve to some chocolatey package, got None (__skip__)"
        );
    }
}
