//! OCI distribution client for pulling and pushing images

use crate::cache::BlobCacheBackend;
use crate::error::{RegistryError, Result};
use crate::image_config::{ImageConfig, OciImageConfigRoot};
use crate::wasm::{detect_artifact_type, extract_wasm_info, ArtifactType, WasmArtifactInfo};
use oci_client::{
    client::{ClientConfig, ClientProtocol},
    manifest::{ImageIndexEntry, OciImageIndex, OciImageManifest, OciManifest},
    secrets::RegistryAuth,
    Reference, RegistryOperation,
};
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::Semaphore;
use tracing::instrument;
use zlayer_spec::PullPolicy;

#[cfg(feature = "local")]
use crate::wasm_export::WasmExportResult;

/// Normalize an image reference to the canonical form used as the *stem* of
/// every manifest cache key.
///
/// This is the load-bearing fix for the macOS image-OS-resolution bug. The
/// pull path and the OS-inspect path receive the SAME image through different
/// spellings: the VZ-Linux delegate pulls `docker.io/library/alpine:latest`
/// (already qualified by the service layer) and writes the manifest under that
/// key, while the composite runtime's `inspect_image_os` is invoked with the
/// BARE `alpine:latest`. With a raw `format!("manifest:{image}")` key those two
/// land in different cache slots, so the inspect misses the cached manifest and
/// goes to the network — which then 429s under a Docker Hub rate-limit.
///
/// Normalizing both spellings through `oci_client::Reference` collapses them
/// onto one canonical `registry/repository:tag` (or `…@sha256:…`) key, so the
/// reader queries exactly what the writer wrote. Inputs that don't parse as a
/// reference (should not happen for real images) fall back to the raw string so
/// the key is still deterministic.
#[must_use]
fn canonical_manifest_ref(image: &str) -> String {
    use std::str::FromStr;
    match oci_client::Reference::from_str(image) {
        Ok(reference) => reference.whole(),
        Err(_) => image.to_string(),
    }
}

/// Blob-cache key under which the OCI manifest body for `image` is stored.
///
/// Both the registry-side pull paths and the agent-side runtimes
/// (`crates/zlayer-agent/src/runtimes/youki.rs`) must construct this key
/// via this function — never via raw `format!("manifest:{image}")`. A
/// drift between writer and reader silently breaks image lookup; we just
/// burned one debugging session on exactly that bug class for the digest
/// sidecar key (see [`manifest_digest_cache_key`]).
///
/// The key stem is the *canonical* reference (see [`canonical_manifest_ref`]),
/// so a bare `alpine:latest` and a qualified `docker.io/library/alpine:latest`
/// resolve to the SAME entry — the writer (pull, qualified) and the reader
/// (OS inspect, bare) always agree.
#[must_use]
pub fn manifest_cache_key(image: &str) -> String {
    format!("manifest:{}", canonical_manifest_ref(image))
}

/// Blob-cache key under which the registry's content-addressable manifest
/// digest is stored alongside the manifest body. Both the registry-side
/// pull paths and the agent-side runtimes (e.g. youki's `list_images`)
/// must agree on this key — otherwise readers silently miss the digest
/// and image-drift detection short-circuits to "no recreate."
///
/// Keyed on the canonical reference (see [`manifest_cache_key`]) so the digest
/// sidecar tracks the manifest body across bare/qualified spellings.
#[must_use]
pub fn manifest_digest_cache_key(image: &str) -> String {
    format!("manifest:digest-{}", canonical_manifest_ref(image))
}

/// Blob-cache key under which the user's ORIGINAL (as-typed) reference for a
/// cached manifest is stored, so `list_images` can surface what the user typed
/// (`alpine:latest`) rather than the canonical form used as the cache key
/// (`docker.io/library/alpine:latest`).
///
/// The prefix is `manifest-orig:` (a hyphen, NOT `manifest:`) so it is NOT
/// enumerated by `keys_with_prefix("manifest:")` in `list_images`. Keyed on the
/// canonical ref (like [`manifest_cache_key`]) so writer and reader agree.
#[must_use]
pub fn manifest_orig_cache_key(image: &str) -> String {
    format!("manifest-orig:{}", canonical_manifest_ref(image))
}

/// Errors that can occur during push operations
#[derive(Debug, Error)]
pub enum PushError {
    /// Authentication failed for the registry
    #[error("authentication failed for registry {registry}: {reason}")]
    AuthenticationFailed {
        /// Registry hostname
        registry: String,
        /// Reason for failure
        reason: String,
    },

    /// Failed to upload a blob
    #[error("failed to upload blob {digest}: {reason}")]
    BlobUploadFailed {
        /// Digest of the blob that failed to upload
        digest: String,
        /// Reason for failure
        reason: String,
    },

    /// Failed to upload a manifest
    #[error("failed to upload manifest: {reason}")]
    ManifestUploadFailed {
        /// Reason for failure
        reason: String,
    },

    /// Network error during push
    #[error("network error: {0}")]
    NetworkError(String),

    /// Invalid image reference
    #[error("invalid image reference: {reference}")]
    InvalidReference {
        /// The invalid reference string
        reference: String,
    },

    /// OCI distribution error
    #[error("OCI distribution error: {0}")]
    OciError(#[from] oci_client::errors::OciDistributionError),
}

/// Result of a successful push operation
#[derive(Debug, Clone)]
pub struct PushResult {
    /// Digest of the pushed manifest (sha256:...)
    pub manifest_digest: String,
    /// List of blob digests that were pushed
    pub blobs_pushed: Vec<String>,
    /// Full reference to the pushed image
    pub reference: String,
}

/// One layer to publish as part of a generic OCI artifact via
/// [`ImagePuller::push_artifact`]. `data` must already be in its final
/// (compressed) form; the digest is computed over exactly these bytes.
#[cfg(feature = "local")]
#[derive(Debug, Clone)]
pub struct ArtifactLayer {
    /// Raw layer bytes (already compressed, matching `media_type`).
    pub data: Vec<u8>,
    /// OCI media type for the layer (e.g.
    /// `application/vnd.oci.image.layer.v1.tar+zstd`).
    pub media_type: String,
    /// Optional `org.opencontainers.image.title` annotation — a filename hint
    /// surfaced to tooling that inspects the layer.
    pub title: Option<String>,
}

/// Compute the `sha256:<hex>` content digest of a blob, as expected by the
/// OCI distribution API.
#[cfg(feature = "local")]
#[must_use]
pub fn oci_sha256_digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("sha256:{}", hex::encode(hasher.finalize()))
}

/// Map Rust architecture names to Go/OCI architecture names.
fn go_arch_name() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "x86" => "amd",
        "aarch64" => "arm64",
        "powerpc64" => "ppc64le",
        other => other,
    }
}

/// Map Rust OS names to Go/OCI OS names.
fn go_os_name() -> &'static str {
    match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    }
}

/// Returns true when the image reference uses a "mutable" tag whose meaning
/// can change over time (e.g. `:latest`, `:dev`), so cached manifests for it
/// must be revalidated against the registry rather than trusted forever.
///
/// Returns false for pinned tags (`:v1.2.3`), digest references
/// (`image@sha256:...`), and anything else not in the known-mutable set.
fn is_mutable_tag(image: &str) -> bool {
    // Digest references are always immutable — strip anything from `@` onward
    // and, if a digest was present, the reference is content-addressed.
    let (without_digest, had_digest) = match image.find('@') {
        Some(idx) => (&image[..idx], true),
        None => (image, false),
    };
    if had_digest {
        return false;
    }

    // Find the tag by looking at the LAST `:`. If everything after it contains
    // a `/`, that colon was part of a registry host:port, not a tag separator.
    let tag = match without_digest.rfind(':') {
        Some(idx) => {
            let candidate = &without_digest[idx + 1..];
            if candidate.contains('/') {
                // Colon belonged to a host:port, so there is no explicit tag.
                None
            } else {
                Some(candidate)
            }
        }
        None => None,
    };

    match tag {
        // No tag at all — Docker defaults to `latest`, which is mutable.
        // Empty tag (e.g. `nginx:`) is treated the same as a missing tag.
        None | Some("") => true,
        Some(t) => matches!(t, "latest" | "dev" | "edge" | "main" | "master"),
    }
}

/// Generate candidate `(name, reference)` pairs to try against the local
/// registry, in priority order.
///
/// `oci_client::Reference::from_str` normalizes bare names like
/// `zarcrunner-executor:latest` into `docker.io/library/zarcrunner-executor:latest`,
/// but locally-built images are stored under whatever name the user passed to
/// `zlayer build`. To bridge that gap we probe several plausible name forms
/// before giving up and falling through to a remote pull.
///
/// Returns candidates in this priority order (deduplicated, preserving the
/// first occurrence):
/// 1. The primary name as parsed.
/// 2. With `docker.io/` prefix stripped.
/// 3. With `docker.io/library/` prefix stripped.
/// 4. With `library/` prefix stripped.
/// 5. With `library/` prefix added (only when the primary name has no `/`).
/// 6. The bare last path segment.
#[cfg(feature = "local")]
fn local_image_ref_candidates(image: &str) -> Vec<(String, String)> {
    // Delegates to the shared, non-cfg helper in `zlayer-types` so the agent
    // crate (VZ rootfs cross-spelling lookup) and the registry use identical
    // candidate logic. See `zlayer_types::image_ref_candidates`.
    zlayer_types::image_ref_candidates(image)
}

#[cfg(all(test, feature = "local"))]
mod local_image_ref_candidates_tests {
    use super::local_image_ref_candidates;

    fn contains(candidates: &[(String, String)], name: &str, reference: &str) -> bool {
        candidates.iter().any(|(n, r)| n == name && r == reference)
    }

    #[test]
    fn bare_name_with_tag_produces_library_and_self() {
        let c = local_image_ref_candidates("zarcrunner-executor:latest");
        assert!(
            contains(&c, "zarcrunner-executor", "latest"),
            "missing bare candidate in {c:?}",
        );
        assert!(
            contains(&c, "library/zarcrunner-executor", "latest"),
            "missing library/ candidate in {c:?}",
        );
    }

    #[test]
    fn fully_qualified_docker_hub_strips_prefixes() {
        let c = local_image_ref_candidates("docker.io/library/zarcrunner-executor:latest");
        assert!(
            contains(&c, "docker.io/library/zarcrunner-executor", "latest"),
            "missing primary in {c:?}",
        );
        assert!(
            contains(&c, "library/zarcrunner-executor", "latest"),
            "missing docker.io-stripped form in {c:?}",
        );
        assert!(
            contains(&c, "zarcrunner-executor", "latest"),
            "missing docker.io/library-stripped form in {c:?}",
        );
    }

    #[test]
    fn ghcr_keeps_qualified_and_bare_segment_but_no_library_prefix() {
        let c = local_image_ref_candidates("ghcr.io/team/svc:1.2");
        assert!(
            contains(&c, "ghcr.io/team/svc", "1.2"),
            "missing primary in {c:?}",
        );
        assert!(
            contains(&c, "svc", "1.2"),
            "missing bare last-segment fallback in {c:?}",
        );
        // The `library/` prefix only applies to bare (slash-free) names. A
        // fully-qualified non-docker-hub name must NOT acquire `library/...`.
        assert!(
            !c.iter().any(|(n, _)| n.starts_with("library/")),
            "unexpected library/ prefix in {c:?}",
        );
    }

    #[test]
    fn digest_reference_is_preserved() {
        let digest = "sha256:0000000000000000000000000000000000000000000000000000000000000000";
        let input = format!("foo@{digest}");
        let c = local_image_ref_candidates(&input);
        assert!(
            contains(&c, "foo", digest),
            "missing foo@digest candidate in {c:?}",
        );
        assert!(
            contains(&c, "library/foo", digest),
            "missing library/foo@digest candidate in {c:?}",
        );
        // The reference column is always the digest, never "latest".
        for (_, r) in &c {
            assert_eq!(r, digest, "non-digest reference leaked into {c:?}");
        }
    }

    #[test]
    fn deterministic_input_has_no_duplicates() {
        for input in [
            "zarcrunner-executor:latest",
            "docker.io/library/zarcrunner-executor:latest",
            "ghcr.io/team/svc:1.2",
            "foo",
            "library/foo:1.0",
            "foo@sha256:0000000000000000000000000000000000000000000000000000000000000000",
        ] {
            let c = local_image_ref_candidates(input);
            let mut sorted: Vec<_> = c.clone();
            sorted.sort();
            let original_len = sorted.len();
            sorted.dedup();
            assert_eq!(
                original_len,
                sorted.len(),
                "duplicate candidates produced for {input}: {c:?}",
            );
        }
    }
}

/// Build a `platform_resolver` closure that picks manifests matching `target`.
///
/// When `target` is `Some`, the closure looks for the exact `{os}/{arch}` in
/// the image index and ignores any host-specific fallback (the caller has
/// asked for a specific platform, so respect that intent).
///
/// When `target` is `None`, the closure falls back to the process's runtime
/// platform (via `go_os_name()` / `go_arch_name()`), preserving the previous
/// hardcoded resolver's behavior — including the macOS-specific two-pass
/// search that first tries `darwin/{arch}` (native zlayer sandbox images)
/// and then `linux/{arch}` (standard Docker Hub images which run inside a
/// Linux VM via Docker Desktop or similar).
fn build_platform_resolver(
    target: Option<zlayer_spec::TargetPlatform>,
) -> impl Fn(&[ImageIndexEntry]) -> Option<String> + Send + Sync + 'static {
    // Resolve once at construction time so env isn't read on every invocation.
    //
    // Windows multi-platform indexes distinguish Server/Desktop build families
    // via `platform.os.version` (e.g. `10.0.26100.*` for Server 2025 / Win11
    // 24H2, `10.0.20348.*` for Server 2022). When the caller pinned a target
    // os_version, prefer manifest entries whose os.version equals the
    // constraint OR shares the same `major.minor.build` prefix.
    //
    // Macos-fallback only applies when we're using the host's default
    // (no explicit override) AND the host is macOS — an explicit
    // `darwin/...` override from a caller should NOT silently match linux.
    let has_target = target.is_some();
    let (target_os, target_arch, target_os_version): (String, String, Option<String>) = match target
    {
        Some(tp) => (
            tp.os.as_oci_str().to_string(),
            tp.arch.as_oci_str().to_string(),
            tp.os_version,
        ),
        None => (go_os_name().to_string(), go_arch_name().to_string(), None),
    };
    let use_macos_fallback = !has_target && cfg!(target_os = "macos");

    move |manifests: &[ImageIndexEntry]| -> Option<String> {
        // Pass 1a: Windows + pinned os_version — prefer an entry whose
        // `os.version` matches exactly or shares the requested prefix.
        if target_os == "windows" {
            if let Some(want_version) = target_os_version.as_deref() {
                if let Some(entry) = manifests.iter().find(|entry| {
                    entry.platform.as_ref().is_some_and(|p| {
                        p.os == target_os
                            && p.architecture == target_arch
                            && p.os_version
                                .as_deref()
                                .is_some_and(|v| v == want_version || v.starts_with(want_version))
                    })
                }) {
                    return Some(entry.digest.clone());
                }
            }
        }

        // Pass 1: exact {os}/{arch} match (os.version ignored — back-compat,
        // and used as the fallback when no Windows os.version match was found).
        if let Some(entry) = manifests.iter().find(|entry| {
            entry
                .platform
                .as_ref()
                .is_some_and(|p| p.os == target_os && p.architecture == target_arch)
        }) {
            return Some(entry.digest.clone());
        }

        // Pass 2: macOS-only fallback to linux/{arch} (for Docker Hub images
        // run inside a Linux VM). Only runs when we're using host defaults.
        if use_macos_fallback {
            return manifests
                .iter()
                .find(|entry| {
                    entry
                        .platform
                        .as_ref()
                        .is_some_and(|p| p.os == "linux" && p.architecture == target_arch)
                })
                .map(|entry| entry.digest.clone());
        }

        None
    }
}

/// Build a [`ClientConfig`] with the zlayer platform resolver and sensible
/// defaults for timeouts and protocol.
///
/// `target` overrides the platform used by the resolver; `None` preserves the
/// historical behavior of matching the process's runtime platform (with the
/// macOS `darwin → linux` fallback).
fn build_client_config(target: Option<zlayer_spec::TargetPlatform>) -> ClientConfig {
    ClientConfig {
        protocol: ClientProtocol::Https,
        connect_timeout: Some(std::time::Duration::from_secs(30)),
        read_timeout: Some(std::time::Duration::from_secs(300)), // 5 minutes for large layers
        platform_resolver: Some(Box::new(build_platform_resolver(target))),
        ..Default::default()
    }
}

/// OCI image puller with caching
pub struct ImagePuller {
    client: oci_client::Client,
    cache: Arc<Box<dyn BlobCacheBackend>>,
    concurrency_limit: Arc<Semaphore>,
    /// Count of network round-trips actually issued against the remote registry
    /// through `self.client`. Bumped by [`ImagePuller::note_network_call`] at
    /// every method that talks to the registry over the wire. It exists so the
    /// *local-only* resolution paths (`resolve_manifest_local_only`,
    /// `image_os_in_cache_only`, `image_runtime_marker_in_cache_only`) can be
    /// proven, in tests, to issue ZERO network calls even when the cache misses
    /// — the macOS Docker-Hub-429 regression was precisely a "local-only" path
    /// silently falling through to the network.
    network_calls: Arc<std::sync::atomic::AtomicUsize>,
    /// Optional SHARED/remote blob cache (typically S3) probed AFTER the
    /// primary `cache` and BEFORE the network. A hit here is written through to
    /// `cache` (warm) on the way out; a fresh network pull is propagated down
    /// into it. `None` = no S3 tier (single-cache behavior, unchanged).
    s3_cache: Option<Arc<Box<dyn BlobCacheBackend>>>,
    /// Last-resort default registry host for bare/unqualified names found
    /// nowhere in the chain (e.g. `docker.io`). `None` = a bare name resolves
    /// nowhere → actionable error; `ZLayer` never silently invents docker.io.
    default_registry: Option<String>,
    /// Per-image override of the resolution chain order (default
    /// [`zlayer_spec::SourcePolicy::LocalFirst`] = the full chain). Read once in
    /// [`Self::pull_manifest_inner`].
    source_policy: zlayer_spec::SourcePolicy,
    #[cfg(feature = "local")]
    local_registry: Option<std::sync::Arc<crate::local_registry::LocalRegistry>>,
}

impl ImagePuller {
    /// Create a new image puller with any cache backend.
    ///
    /// Pulls will target the process's runtime platform (with the historical
    /// macOS `darwin → linux` fallback). To pull a specific platform, use
    /// [`ImagePuller::with_platform`].
    pub fn new<C: BlobCacheBackend + 'static>(cache: C) -> Self {
        let client = oci_client::Client::new(build_client_config(None));

        Self {
            client,
            cache: Arc::new(Box::new(cache) as Box<dyn BlobCacheBackend>),
            concurrency_limit: Arc::new(Semaphore::new(3)),
            network_calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            s3_cache: None,
            default_registry: None,
            source_policy: zlayer_spec::SourcePolicy::LocalFirst,
            #[cfg(feature = "local")]
            local_registry: None,
        }
    }

    /// Create a new image puller with boxed cache backend.
    ///
    /// Pulls will target the process's runtime platform (with the historical
    /// macOS `darwin → linux` fallback). To pull a specific platform, use
    /// [`ImagePuller::with_platform`].
    #[must_use]
    pub fn with_cache(cache: Arc<Box<dyn BlobCacheBackend>>) -> Self {
        let client = oci_client::Client::new(build_client_config(None));

        Self {
            client,
            cache,
            concurrency_limit: Arc::new(Semaphore::new(3)),
            network_calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            s3_cache: None,
            default_registry: None,
            source_policy: zlayer_spec::SourcePolicy::LocalFirst,
            #[cfg(feature = "local")]
            local_registry: None,
        }
    }

    /// Create a new image puller targeting a specific OCI platform.
    ///
    /// The puller's internal `oci-client` will select the manifest whose
    /// `{os}/{arch}` matches `target` from a multi-platform image index.
    /// Unlike [`ImagePuller::new`], no macOS `darwin → linux` fallback is
    /// applied — an explicit target is respected exactly.
    #[must_use]
    pub fn with_platform(
        cache: Arc<Box<dyn BlobCacheBackend>>,
        target: zlayer_spec::TargetPlatform,
    ) -> Self {
        let client = oci_client::Client::new(build_client_config(Some(target)));

        Self {
            client,
            cache,
            concurrency_limit: Arc::new(Semaphore::new(3)),
            network_calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            s3_cache: None,
            default_registry: None,
            source_policy: zlayer_spec::SourcePolicy::LocalFirst,
            #[cfg(feature = "local")]
            local_registry: None,
        }
    }

    /// Record that a network round-trip is about to be issued against the
    /// remote registry. Every method that calls `self.client` over the wire
    /// must invoke this first so that the local-only paths can be *proven* (in
    /// tests) to never reach the network. Cheap relaxed atomic increment.
    fn note_network_call(&self) {
        self.network_calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    /// Number of network round-trips issued against the remote registry so far.
    /// Test-only spy used to assert that the local-only resolution paths stay
    /// offline (see the `*_in_cache_only` regression tests).
    #[cfg(test)]
    fn network_call_count(&self) -> usize {
        self.network_calls
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Store authentication for a registry
    ///
    /// This ensures the client has auth credentials before attempting pulls.
    async fn store_auth(&self, image: &str, auth: &RegistryAuth) -> Result<()> {
        let reference: Reference = image.parse().map_err(|_| RegistryError::NotFound {
            registry: "unknown".to_string(),
            image: image.to_string(),
        })?;

        // Store auth in the client's internal cache
        // This is called by pull_image_manifest, but we call it explicitly here
        // to ensure auth is available for blob pulls
        self.client
            .store_auth_if_needed(reference.resolve_registry(), auth)
            .await;

        Ok(())
    }

    /// Set concurrency limit for blob downloads
    #[must_use]
    pub fn with_concurrency_limit(mut self, limit: usize) -> Self {
        self.concurrency_limit = Arc::new(Semaphore::new(limit));
        self
    }

    /// Attach a SHARED/remote blob cache (typically S3) as the write-through
    /// tier probed after the primary cache and before the network. See the
    /// `s3_cache` field. Build one from the environment with
    /// [`crate::s3_cache_from_env`].
    #[must_use]
    pub fn with_s3_cache(mut self, s3: Arc<Box<dyn BlobCacheBackend>>) -> Self {
        self.s3_cache = Some(s3);
        self
    }

    /// Set the last-resort default registry host for bare/unqualified names
    /// (e.g. `docker.io`). With no default set, a bare name found nowhere in the
    /// chain is an error — `ZLayer` never silently invents docker.io.
    #[must_use]
    pub fn with_default_registry(mut self, registry: impl Into<String>) -> Self {
        let host = registry.into();
        self.default_registry = if host.trim().is_empty() {
            None
        } else {
            Some(host)
        };
        self
    }

    /// Override the resolution chain order for this puller (per-image policy).
    #[must_use]
    pub fn with_source_policy(mut self, source: zlayer_spec::SourcePolicy) -> Self {
        self.source_policy = source;
        self
    }

    /// Central runtime-puller constructor: wraps `cache` and wires the shared S3
    /// tier (`ZLAYER_S3_BUCKET`) + last-resort default registry
    /// (`ZLAYER_DEFAULT_REGISTRY`) from the environment, plus the per-image
    /// `source` policy. This is the SINGLE place runtimes get the full source
    /// chain — they must not re-implement the env wiring. Callers add
    /// `.with_local_registry(..)` afterward when they have one (it is
    /// feature-gated, so it stays at the call site).
    pub async fn from_env_for_runtime(
        cache: Arc<Box<dyn BlobCacheBackend>>,
        source: zlayer_spec::SourcePolicy,
    ) -> Self {
        let mut puller = Self::with_cache(cache).with_source_policy(source);
        match crate::cache_config::s3_cache_from_env().await {
            Ok(Some(s3)) => puller = puller.with_s3_cache(s3),
            Ok(None) => {}
            Err(e) => {
                tracing::warn!(error = %e, "S3 cache tier configured but failed to initialize; continuing without it");
            }
        }
        if let Some(default_registry) = crate::cache_config::default_registry_from_env() {
            puller = puller.with_default_registry(default_registry);
        }
        puller
    }

    /// Set a local OCI registry for image resolution.
    #[cfg(feature = "local")]
    #[must_use]
    pub fn with_local_registry(
        mut self,
        registry: std::sync::Arc<crate::local_registry::LocalRegistry>,
    ) -> Self {
        self.local_registry = Some(registry);
        self
    }

    /// Pull a single blob and cache it
    ///
    /// Uses the provided authentication credentials to pull blobs from the registry.
    ///
    /// # Errors
    ///
    /// Returns an error if the blob cannot be pulled from the registry or cached.
    ///
    /// # Panics
    ///
    /// Panics if the concurrency semaphore is closed.
    pub async fn pull_blob(
        &self,
        image: &str,
        digest: &str,
        auth: &RegistryAuth,
    ) -> Result<Vec<u8>> {
        self.pull_blob_with_urls(image, digest, auth, &[], None)
            .await
    }

    /// Pull a blob, with foreign-layer `urls[]` redirect fallback on 404.
    ///
    /// Behaviour matches [`pull_blob`] for ordinary layers. For descriptors that
    /// carry a non-empty `urls` list (typically Windows base layers with media
    /// type `application/vnd.docker.image.rootfs.foreign.diff.tar.gzip` or the
    /// OCI nondistributable layer types served by Microsoft Container Registry),
    /// a primary-registry 404 triggers sequential `GET`s against each URL.
    /// Each redirect response is digest-verified (and size-verified when
    /// `expected_size` is provided); the first successful match is cached and
    /// returned.
    ///
    /// At most the first `MAX_FOREIGN_LAYER_REDIRECTS` URLs are attempted; any
    /// additional URLs are ignored to guard against circular redirect spam.
    ///
    /// # Errors
    ///
    /// Returns an error if the blob cannot be pulled from the primary registry
    /// and every `urls[]` fallback also fails.
    ///
    /// # Panics
    ///
    /// Panics if the concurrency semaphore is closed.
    pub async fn pull_blob_with_urls(
        &self,
        image: &str,
        digest: &str,
        auth: &RegistryAuth,
        urls: &[String],
        expected_size: Option<i64>,
    ) -> Result<Vec<u8>> {
        // Check the local cache first (now async).
        if let Some(data) = self.cache.get(digest).await? {
            tracing::debug!(digest = %digest, "blob found in cache");
            return Ok(data);
        }

        // Then the shared S3 tier (digest-addressed → immutable, no
        // revalidation). A hit warms the local cache so the next pull is local.
        if let Some(s3) = &self.s3_cache {
            if let Ok(Some(data)) = s3.get(digest).await {
                tracing::debug!(digest = %digest, "blob found in S3 tier; warming local cache");
                let _ = self.cache.put(digest, &data).await;
                return Ok(data);
            }
        }

        // Then the daemon-wide local OCI registry (`zlayer import` / `zlayer
        // build` store). Blobs are digest-addressed → immutable, so a hit is
        // authoritative; warm the local cache so subsequent pulls are pure
        // cache hits. This is what lets an IMPORTED image's layers resolve
        // without ever touching a remote registry (the manifest already
        // resolves via `try_local_registry`).
        #[cfg(feature = "local")]
        if let Some(registry) = &self.local_registry {
            if let Ok(data) = registry.get_blob(digest).await {
                if !data.is_empty() {
                    tracing::debug!(
                        digest = %digest,
                        "blob found in local registry; warming local cache"
                    );
                    let _ = self.cache.put(digest, &data).await;
                    return Ok(data);
                }
            }
        }

        // Store auth before pulling blob
        self.store_auth(image, auth).await?;

        let reference: Reference = image.parse().map_err(|_| RegistryError::NotFound {
            registry: "unknown".to_string(),
            image: image.to_string(),
        })?;

        let _permit = self.concurrency_limit.acquire().await.unwrap();

        tracing::debug!(digest = %digest, image = %image, "pulling blob from registry");

        // Pull from registry into memory
        // Note: We pass &mut buffer directly, not wrapped in BufWriter.
        // Vec<u8> implements AsyncWrite and oci_client's pull_blob writes directly to it.
        // Using BufWriter was causing data to not be flushed properly.
        let mut buffer = Vec::new();
        let primary_result = self.client.pull_blob(&reference, digest, &mut buffer).await;

        let buffer = match primary_result {
            Ok(()) => buffer,
            Err(err) if is_blob_not_found(&err) && !urls.is_empty() => {
                tracing::info!(
                    digest = %digest,
                    urls_count = urls.len(),
                    "primary registry missed foreign layer; attempting urls[] redirects"
                );
                let mut last_err: Option<RegistryError> = None;
                let mut recovered: Option<Vec<u8>> = None;
                for url in urls.iter().take(MAX_FOREIGN_LAYER_REDIRECTS) {
                    match fetch_blob_from_url(url, digest, expected_size).await {
                        Ok(bytes) => {
                            tracing::info!(url = %url, "foreign layer recovered via redirect");
                            recovered = Some(bytes);
                            break;
                        }
                        Err(e) => {
                            tracing::warn!(url = %url, error = %e, "redirect attempt failed");
                            last_err = Some(e);
                        }
                    }
                }
                if let Some(bytes) = recovered {
                    bytes
                } else {
                    tracing::error!(
                        digest = %digest,
                        image = %image,
                        "foreign layer redirect fallback exhausted"
                    );
                    return Err(last_err.unwrap_or(RegistryError::Oci(err)));
                }
            }
            Err(err) => {
                tracing::error!(error = %err, digest = %digest, image = %image, "failed to pull blob from registry");
                return Err(RegistryError::Oci(err));
            }
        };

        // Validate blob data is not empty
        if buffer.is_empty() {
            return Err(RegistryError::Cache(crate::error::CacheError::Corrupted(
                format!("blob {digest} was empty after pulling from registry"),
            )));
        }

        // Cache the blob locally (now async).
        self.cache.put(digest, &buffer).await?;

        // Propagate DOWN into the shared S3 tier so other nodes (and this node
        // after a cold restart) can serve it without re-hitting the origin.
        // Best-effort: an S3 write failure must not fail an otherwise-good pull.
        if let Some(s3) = &self.s3_cache {
            if let Err(e) = s3.put(digest, &buffer).await {
                tracing::debug!(digest = %digest, error = %e, "failed to propagate blob to S3 tier");
            }
        }

        tracing::debug!(digest = %digest, size = buffer.len(), "blob cached successfully");

        Ok(buffer)
    }

    /// Fetch the current upstream manifest digest for `image` from the registry
    /// without pulling the manifest body. Uses `HEAD /v2/{repo}/manifests/{tag}`.
    ///
    /// Returns `Ok(Some(digest))` when the registry reports a current digest,
    /// `Ok(None)` when the image or tag is not found on the registry, and `Err`
    /// on transport / auth / protocol failures.
    ///
    /// This exists so [`pull_manifest`] can revalidate its cache entry against
    /// the live registry state for mutable tags (`:latest`, `:dev`, ...) without
    /// paying for a full manifest download on every pull.
    ///
    /// # Errors
    ///
    /// Returns an error if the image reference is invalid or the registry
    /// returns a transport, auth, or protocol-level failure.
    pub async fn remote_manifest_digest(
        &self,
        image: &str,
        auth: &RegistryAuth,
    ) -> Result<Option<String>> {
        let reference: Reference = image.parse().map_err(|_| RegistryError::NotFound {
            registry: "unknown".to_string(),
            image: image.to_string(),
        })?;

        self.note_network_call();
        match self.client.fetch_manifest_digest(&reference, auth).await {
            Ok(digest) => Ok(Some(digest)),
            Err(oci_client::errors::OciDistributionError::ImageManifestNotFoundError(_)) => {
                tracing::debug!(image = %image, "remote manifest not found");
                Ok(None)
            }
            Err(e) => {
                tracing::warn!(error = %e, image = %image, "failed to fetch remote manifest digest");
                Err(RegistryError::Oci(e))
            }
        }
    }

    /// Try to satisfy a manifest request from the persistent cache.
    ///
    /// Returns `Some((manifest, digest))` when the cached entry is usable
    /// (either because the image ref is pinned, or because a HEAD revalidation
    /// against the registry confirmed the cached digest is still current, or
    /// because the registry was unreachable and we're falling back to stale).
    /// Returns `None` when the caller should proceed to fetch the manifest
    /// fresh from the registry — i.e. there is no cached entry, the cached
    /// bytes could not be deserialized, or revalidation detected a newer
    /// digest and already invalidated the cache entries.
    async fn try_cached_manifest(
        &self,
        image: &str,
        auth: &RegistryAuth,
        cache_key: &str,
        digest_key: &str,
        policy: PullPolicy,
    ) -> Option<(OciImageManifest, String)> {
        let data = self.cache.get(cache_key).await.ok().flatten()?;
        let manifest = serde_json::from_slice::<OciImageManifest>(&data).ok()?;

        let cached_digest = self
            .cache
            .get(digest_key)
            .await
            .ok()
            .flatten()
            .and_then(|bytes| String::from_utf8(bytes).ok());

        // IfNotPresent / Never: cached manifest is good enough, regardless of mutable tag.
        if matches!(policy, PullPolicy::IfNotPresent | PullPolicy::Never) {
            let digest = cached_digest.unwrap_or_else(|| crate::cache::compute_digest(&data));
            tracing::debug!(image = %image, "manifest cache hit (IfNotPresent/Never, no revalidation)");
            return Some((manifest, digest));
        }

        // Pinned refs: trust the cache without revalidation.
        if !is_mutable_tag(image) {
            let digest = cached_digest.unwrap_or_else(|| crate::cache::compute_digest(&data));
            tracing::debug!(image = %image, "manifest cache hit (pinned ref)");
            return Some((manifest, digest));
        }

        // Mutable tag: revalidate against the registry.
        match self.remote_manifest_digest(image, auth).await {
            Ok(Some(remote_digest)) => {
                if let Some(cached) = cached_digest.as_deref() {
                    if cached == remote_digest {
                        tracing::debug!(image = %image, "manifest cache hit (revalidated)");
                        return Some((manifest, remote_digest));
                    }
                    tracing::info!(
                        image = %image,
                        cached = %cached,
                        remote = %remote_digest,
                        "cached manifest is stale, refetching"
                    );
                } else {
                    // Legacy pre-revalidation entry with no cached digest:
                    // treat as stale so we refetch once and populate both
                    // keys for future revalidations.
                    tracing::info!(
                        image = %image,
                        remote = %remote_digest,
                        "cached manifest has no stored digest, refetching"
                    );
                }
                if let Err(e) = self.cache.delete(cache_key).await {
                    tracing::warn!(
                        image = %image,
                        error = %e,
                        "failed to delete stale manifest cache entry"
                    );
                }
                if let Err(e) = self.cache.delete(digest_key).await {
                    tracing::warn!(
                        image = %image,
                        error = %e,
                        "failed to delete stale manifest digest cache entry"
                    );
                }
                None
            }
            Ok(None) => {
                tracing::warn!(image = %image, "remote manifest not found, using stale cache");
                let digest = cached_digest.unwrap_or_else(|| crate::cache::compute_digest(&data));
                Some((manifest, digest))
            }
            Err(e) => {
                tracing::warn!(
                    image = %image,
                    error = %e,
                    "manifest revalidation failed, using stale cache"
                );
                let digest = cached_digest.unwrap_or_else(|| crate::cache::compute_digest(&data));
                Some((manifest, digest))
            }
        }
    }

    /// Try to resolve a manifest from the local registry.
    ///
    /// Probes every candidate `(name, reference)` form produced by
    /// [`local_image_ref_candidates`] in order, returning the first that
    /// resolves to a parseable `OciImageManifest`. This is what lets a
    /// deployment spec say `image: zarcrunner-executor:latest` and still hit
    /// a locally-built image, even though `oci_client::Reference` normalized
    /// the lookup name to `docker.io/library/zarcrunner-executor`.
    #[cfg(feature = "local")]
    async fn try_local_registry(&self, image: &str) -> Option<(OciImageManifest, String)> {
        let registry = self.local_registry.as_ref()?;
        let candidates = local_image_ref_candidates(image);
        let primary = candidates.first().map(|(n, _)| n.clone());
        for (name, reference) in candidates {
            // `Err` arm (miss on this candidate) is a no-op — we just try the
            // next form, so `if let Ok` is clearer than a `match` with a stub
            // `Err(_)` arm (clippy::single-match-else).
            if let Ok(data) = registry.get_manifest(&name, &reference).await {
                match serde_json::from_slice::<OciImageManifest>(&data) {
                    Ok(manifest) => {
                        let digest = crate::cache::compute_digest(&data);
                        if primary.as_deref() == Some(name.as_str()) {
                            tracing::debug!(
                                image = %image,
                                digest = %digest,
                                "manifest found in local registry",
                            );
                        } else {
                            tracing::debug!(
                                image = %image,
                                matched_name = %name,
                                digest = %digest,
                                "manifest found in local registry via name-form fallback",
                            );
                        }
                        return Some((manifest, digest));
                    }
                    Err(e) => {
                        tracing::warn!(
                            image = %image,
                            candidate = %name,
                            error = %e,
                            "local registry manifest parse failed",
                        );
                        // Keep probing — the next candidate may still parse.
                    }
                }
            }
        }

        // Suffix-match fallback (§4.1): a locally-built/pushed image may be
        // stored under a FULLER name than any canonical candidate form — e.g.
        // `zarcrunner-executor:latest` stored as `…/library/zarcrunner-executor:latest`.
        // For an unqualified input that missed every candidate, scan the local
        // registry and match by final path segment (optionally under a
        // `library/` namespace), so the daemon resolves the local image instead
        // of falling through to a doomed Docker Hub pull.
        if zlayer_types::image_str_is_unqualified(image) {
            if let Some((want_name, want_ref)) =
                local_image_ref_candidates(image).into_iter().next()
            {
                let want_leaf = want_name
                    .rsplit('/')
                    .next()
                    .unwrap_or(&want_name)
                    .to_string();
                if let Ok(stored) = registry.list_images().await {
                    for s in stored {
                        let s_leaf = s.rsplit('/').next().unwrap_or(&s);
                        // Match the requested leaf either exactly, or as the
                        // leaf of a stored namespaced name (`…/<leaf>` /
                        // `…/library/<leaf>`). The tag must also resolve below,
                        // which guards against a same-leaf-different-tag miss.
                        if s_leaf != want_leaf {
                            continue;
                        }
                        if let Ok(data) = registry.get_manifest(&s, &want_ref).await {
                            if let Ok(manifest) = serde_json::from_slice::<OciImageManifest>(&data)
                            {
                                let digest = crate::cache::compute_digest(&data);
                                tracing::debug!(
                                    image = %image,
                                    matched_name = %s,
                                    reference = %want_ref,
                                    digest = %digest,
                                    "manifest found in local registry via unqualified suffix-match",
                                );
                                return Some((manifest, digest));
                            }
                        }
                    }
                }
            }
        }

        None
    }

    /// Pull an image manifest from the registry.
    ///
    /// Returns the manifest and its registry-reported digest. Manifests are
    /// cached to avoid repeated network requests for the same image reference.
    ///
    /// For mutable tags (`:latest`, `:dev`, `:edge`, `:main`, `:master`, or
    /// missing/empty tags), a cache hit triggers a `HEAD` revalidation against
    /// the upstream registry. If the cached digest still matches, the cached
    /// manifest is returned. If the remote digest differs, the cache entry is
    /// invalidated and the manifest is refetched. If the revalidation request
    /// itself fails (e.g. the registry is unreachable), the stale cached
    /// manifest is returned rather than failing the caller — serving stale
    /// beats crashing the deploy.
    ///
    /// Pinned tags (e.g. `:v1.2.3`) and digest references (`img@sha256:...`)
    /// are treated as immutable: a cache hit is returned without revalidation.
    ///
    /// # Errors
    ///
    /// Returns an error if the manifest cannot be pulled or the image reference is invalid.
    pub async fn pull_manifest(
        &self,
        image: &str,
        auth: &RegistryAuth,
    ) -> Result<(OciImageManifest, String)> {
        self.pull_manifest_inner(image, auth, PullPolicy::Newer)
            .await
    }

    /// Internal manifest pull driven by an explicit [`PullPolicy`].
    ///
    /// `PullPolicy::IfNotPresent` and `PullPolicy::Never` short-circuit any
    /// remote revalidation when the manifest is already available locally
    /// (either in the blob cache or in the local registry). `PullPolicy::Newer`
    /// preserves the legacy behavior of revalidating mutable tags against the
    /// remote. `PullPolicy::Always` is handled at the `*_with_policy` entry
    /// points by invalidating the cache before delegating here.
    async fn pull_manifest_inner(
        &self,
        image: &str,
        auth: &RegistryAuth,
        policy: PullPolicy,
    ) -> Result<(OciImageManifest, String)> {
        use zlayer_spec::SourcePolicy;
        let cache_key = manifest_cache_key(image);
        let digest_key = manifest_digest_cache_key(image);
        let source = self.source_policy;

        // The resolution chain — LOCAL store -> local CACHE -> shared S3 tier ->
        // the ref's own registry (URL) -> last-resort default registry. Lower
        // tiers are populated ("propagated down") from a higher-authority hit so
        // the chain stays consistent. Mutable tags revalidate against the ORIGIN;
        // on any upstream-check error we serve the copy we already have
        // (fail-safe — serving stale beats crashing a redeploy on a Docker Hub
        // 429). Pinned tags / digests are immutable and never re-checked.
        //
        // `source` (the per-image [`SourcePolicy`]) selects which tiers run:
        //   - LocalFirst (default): the full chain, local CACHE before S3.
        //   - S3First:    like LocalFirst but probe S3 BEFORE the local cache.
        //   - LocalOnly:  local store + cache only; never S3/network/fallback.
        //   - RemoteOnly: skip every local/cached/S3 source; go to origin.
        // The local in-process CACHE is probed before the network S3 tier under
        // LocalFirst (faster; byte-identical for immutable, both revalidate vs
        // origin for mutable).
        let use_local = !matches!(source, SourcePolicy::RemoteOnly);
        let use_s3 = matches!(source, SourcePolicy::LocalFirst | SourcePolicy::S3First);
        let use_origin = !matches!(source, SourcePolicy::LocalOnly);

        // 1. LOCAL store — owned/built images via the local registry. Highest
        //    authority for "we already have THIS image".
        #[cfg(feature = "local")]
        if use_local {
            if let Some((manifest, digest)) = self.try_local_registry(image).await {
                // Owned images warm the LOCAL cache only — never propagate a
                // locally-built manifest up into the shared S3 tier.
                self.warm_local_cache(image, &cache_key, &digest_key, &manifest, &digest)
                    .await;

                // Serve local unless we may AND should revalidate a mutable tag
                // against the origin (never under LocalOnly / IfNotPresent / Never
                // / a pinned ref).
                let revalidate = use_origin
                    && !matches!(policy, PullPolicy::IfNotPresent | PullPolicy::Never)
                    && is_mutable_tag(image);
                if !revalidate {
                    return Ok((manifest, digest));
                }
                match self.remote_manifest_digest(image, auth).await {
                    Ok(Some(remote_digest)) if remote_digest != digest => {
                        tracing::info!(
                            image = %image,
                            local = %digest,
                            remote = %remote_digest,
                            "remote has newer manifest, pulling fresh"
                        );
                        // Newer upstream — fall through to the origin pull.
                    }
                    Ok(Some(_) | None) => return Ok((manifest, digest)),
                    Err(e) => {
                        tracing::warn!(image = %image, error = %e, "remote check failed, using local manifest");
                        return Ok((manifest, digest));
                    }
                }
                return self
                    .pull_from_origin(image, auth, &cache_key, &digest_key)
                    .await;
            }
        }

        // 2 & 3. Local blob/manifest CACHE and the shared S3 tier, ordered by
        //    policy. Skipped entirely under RemoteOnly.
        if use_local {
            if matches!(source, SourcePolicy::S3First) {
                if let Some(hit) = self
                    .try_s3_manifest(image, auth, &cache_key, &digest_key, policy)
                    .await
                {
                    return Ok(hit);
                }
                if let Some(hit) = self
                    .try_cached_manifest(image, auth, &cache_key, &digest_key, policy)
                    .await
                {
                    return Ok(hit);
                }
            } else {
                if let Some(hit) = self
                    .try_cached_manifest(image, auth, &cache_key, &digest_key, policy)
                    .await
                {
                    return Ok(hit);
                }
                if use_s3 {
                    if let Some(hit) = self
                        .try_s3_manifest(image, auth, &cache_key, &digest_key, policy)
                        .await
                    {
                        return Ok(hit);
                    }
                }
            }
        }

        // 4 & 5. Origin (the ref's own registry / configured default registry).
        //    Skipped under LocalOnly — a local miss there is a clean error.
        if !use_origin {
            return Err(RegistryError::NotFound {
                registry: "local".to_string(),
                image: format!(
                    "{image} (source_policy=local_only: not present in any local source \
                     and remote resolution is disabled for this image)"
                ),
            });
        }
        self.pull_from_origin(image, auth, &cache_key, &digest_key)
            .await
    }

    /// Steps 4 & 5 of the chain: pull the manifest from the ref's OWN registry
    /// (URL), or — for a bare/unqualified name found nowhere locally — from the
    /// configured last-resort default registry. `ZLayer` NEVER silently invents
    /// docker.io: a bare name with no default registry configured is an
    /// actionable error. A successful pull is propagated DOWN the chain (local
    /// cache + S3). On remote failure, the local registry is the last resort.
    async fn pull_from_origin(
        &self,
        image: &str,
        auth: &RegistryAuth,
        cache_key: &str,
        digest_key: &str,
    ) -> Result<(OciImageManifest, String)> {
        let pull_ref: Reference = if zlayer_types::image_str_is_unqualified(image) {
            let Some(default) = self.default_registry.as_deref() else {
                return Err(RegistryError::NotFound {
                    registry: "local".to_string(),
                    image: format!(
                        "{image} (unqualified image not found locally; ZLayer does not silently \
                         pull bare names from a public registry — use a full registry URL such as \
                         registry.example.com/{image} for a remote pull, configure a default with \
                         `zlayer login --default <host>` (or ZLAYER_DEFAULT_REGISTRY), or run \
                         `zlayer build` to produce it locally)"
                    ),
                });
            };
            let qualified = format!("{}/{}", default.trim_end_matches('/'), image);
            tracing::info!(
                image = %image,
                default_registry = %default,
                qualified = %qualified,
                "bare name resolved nowhere; using configured last-resort default registry"
            );
            qualified.parse().map_err(|_| RegistryError::NotFound {
                registry: default.to_string(),
                image: qualified.clone(),
            })?
        } else {
            image.parse().map_err(|_| RegistryError::NotFound {
                registry: "unknown".to_string(),
                image: image.to_string(),
            })?
        };

        tracing::info!(image = %image, reference = %pull_ref, "pulling manifest from registry");

        self.note_network_call();
        match self.client.pull_image_manifest(&pull_ref, auth).await {
            Ok((manifest, digest)) => {
                tracing::debug!(
                    image = %image,
                    digest = %digest,
                    layers = manifest.layers.len(),
                    "manifest pulled successfully"
                );

                // Propagate DOWN the chain: both the local cache AND the shared
                // S3 tier get the fresh body so later resolves (here or on other
                // nodes) stay consistent.
                self.propagate_manifest_down(image, cache_key, digest_key, &manifest, &digest)
                    .await;

                Ok((manifest, digest))
            }
            Err(remote_err) => {
                // Remote failed — try local registry as last resort.
                #[cfg(feature = "local")]
                if let Some((manifest, digest)) = self.try_local_registry(image).await {
                    tracing::warn!(
                        image = %image,
                        error = %remote_err,
                        "remote pull failed, falling back to local registry"
                    );
                    return Ok((manifest, digest));
                }

                tracing::error!(error = %remote_err, image = %image, "failed to pull manifest");
                Err(RegistryError::Oci(remote_err))
            }
        }
    }

    /// Record the user's ORIGINAL (as-typed) `image` ref in the local cache so
    /// `list_images` can surface it instead of the canonical key. Best-effort.
    /// One place writes this sidecar (called from both warm + propagate).
    async fn record_original_ref(&self, image: &str) {
        let _ = self
            .cache
            .put(&manifest_orig_cache_key(image), image.as_bytes())
            .await;
    }

    /// Write a manifest body + digest into the LOCAL cache only (warming it from
    /// a higher-authority tier) and record the original ref. Best-effort —
    /// failures are ignored; the already-resolved manifest is still returned.
    async fn warm_local_cache(
        &self,
        image: &str,
        cache_key: &str,
        digest_key: &str,
        manifest: &OciImageManifest,
        digest: &str,
    ) {
        if let Ok(bytes) = serde_json::to_vec(manifest) {
            let _ = self.cache.put(cache_key, &bytes).await;
            let _ = self.cache.put(digest_key, digest.as_bytes()).await;
        }
        self.record_original_ref(image).await;
    }

    /// Propagate a remote-origin manifest DOWN the chain: into the local cache
    /// AND the shared S3 tier (when configured), and record the original ref.
    /// Best-effort; an S3 write failure is logged and never fails the resolve.
    async fn propagate_manifest_down(
        &self,
        image: &str,
        cache_key: &str,
        digest_key: &str,
        manifest: &OciImageManifest,
        digest: &str,
    ) {
        let Ok(bytes) = serde_json::to_vec(manifest) else {
            return;
        };
        let _ = self.cache.put(cache_key, &bytes).await;
        let _ = self.cache.put(digest_key, digest.as_bytes()).await;
        self.record_original_ref(image).await;
        if let Some(s3) = &self.s3_cache {
            if let Err(e) = s3.put(cache_key, &bytes).await {
                tracing::debug!(error = %e, "failed to propagate manifest body to S3 tier");
            }
            if let Err(e) = s3.put(digest_key, digest.as_bytes()).await {
                tracing::debug!(error = %e, "failed to propagate manifest digest to S3 tier");
            }
        }
    }

    /// Probe the shared S3 tier for a cached manifest, mirroring
    /// [`Self::try_cached_manifest`]'s revalidation policy. A hit is written
    /// through to the LOCAL cache (warming it) before returning. Returns `None`
    /// when no S3 tier is configured, it misses, or its copy is stale vs origin.
    async fn try_s3_manifest(
        &self,
        image: &str,
        auth: &RegistryAuth,
        cache_key: &str,
        digest_key: &str,
        policy: PullPolicy,
    ) -> Option<(OciImageManifest, String)> {
        let s3 = self.s3_cache.as_ref()?;
        let data = s3.get(cache_key).await.ok().flatten()?;
        let manifest = serde_json::from_slice::<OciImageManifest>(&data).ok()?;
        let cached_digest = s3
            .get(digest_key)
            .await
            .ok()
            .flatten()
            .and_then(|b| String::from_utf8(b).ok());

        // IfNotPresent/Never or pinned ref: serve without revalidation.
        if matches!(policy, PullPolicy::IfNotPresent | PullPolicy::Never) || !is_mutable_tag(image)
        {
            let digest = cached_digest.unwrap_or_else(|| crate::cache::compute_digest(&data));
            self.warm_local_cache(image, cache_key, digest_key, &manifest, &digest)
                .await;
            tracing::debug!(image = %image, "manifest S3 tier hit");
            return Some((manifest, digest));
        }

        // Mutable + Newer: revalidate against the origin registry.
        match self.remote_manifest_digest(image, auth).await {
            Ok(Some(remote_digest)) if cached_digest.as_deref() == Some(remote_digest.as_str()) => {
                self.warm_local_cache(image, cache_key, digest_key, &manifest, &remote_digest)
                    .await;
                tracing::debug!(image = %image, "manifest S3 tier hit (revalidated)");
                Some((manifest, remote_digest))
            }
            Ok(Some(_)) => {
                tracing::info!(image = %image, "S3 manifest stale vs origin, refetching");
                None
            }
            Ok(None) | Err(_) => {
                let digest = cached_digest.unwrap_or_else(|| crate::cache::compute_digest(&data));
                self.warm_local_cache(image, cache_key, digest_key, &manifest, &digest)
                    .await;
                tracing::warn!(image = %image, "S3 manifest revalidation unavailable, serving S3 copy");
                Some((manifest, digest))
            }
        }
    }

    /// Resolve a manifest STRICTLY from local stores — the blob cache and (with
    /// the `local` feature) the local registry — with NO network call.
    ///
    /// Returns `Ok(Some(..))` on a local hit, `Ok(None)` when the image is not
    /// present locally. Unlike [`pull_manifest_inner`], a local miss never falls
    /// through to a remote pull, so a caller can probe several caches in turn
    /// without each miss triggering (and rate-limiting) a Docker Hub round-trip.
    ///
    /// This is the no-network half of the macOS image-OS-resolution fix: the
    /// composite runtime probes the VZ-Linux cache and then the primary Sandbox
    /// cache, and only hits the network as a deliberate final fallback.
    async fn resolve_manifest_local_only(&self, image: &str) -> Option<(OciImageManifest, String)> {
        let cache_key = manifest_cache_key(image);
        let digest_key = manifest_digest_cache_key(image);

        // 1. Blob-cache content read FIRST — this is the authoritative source and
        //    must win before anything else. The pull writes the *resolved* (per-
        //    arch) manifest body under `manifest_cache_key(image)` (e.g.
        //    `manifest:docker.io/library/alpine:latest`), and the canonical key
        //    normalization (see `canonical_manifest_ref`) makes a BARE
        //    `alpine:latest` inspect collide with that qualified writer key.
        //
        //    Under `IfNotPresent`, `try_cached_manifest` returns the cached body
        //    with NO digest revalidation and NO network call (the network-only
        //    `Newer`/`Always` + mutable-tag branch is unreachable here). We must
        //    NOT route through the `manifest:digest-<ref>` sidecar to drive a
        //    by-digest fetch: that sidecar holds the multi-arch INDEX digest,
        //    which is never stored as content and is irrelevant to reading the
        //    already-resolved manifest. The blob-cache content read below is the
        //    one and only manifest source in the local-only path.
        if let Some(hit) = self
            .try_cached_manifest(
                image,
                &RegistryAuth::Anonymous,
                &cache_key,
                &digest_key,
                PullPolicy::IfNotPresent,
            )
            .await
        {
            return Some(hit);
        }

        // 2. ONLY THEN consult the local registry, and treat every outcome as a
        //    pure hit-or-miss — `try_local_registry` returns `Option`, so a
        //    bare-name miss (and its "not found in registry local" log) is a
        //    `None` we fall through on, NEVER a propagated error that could
        //    short-circuit the caller before the blob-cache content was tried.
        //    On a hit, populate the blob cache so the next inspect is a direct
        //    content hit. This probe is local-disk only — no network.
        #[cfg(feature = "local")]
        if let Some((manifest, digest)) = self.try_local_registry(image).await {
            if let Ok(bytes) = serde_json::to_vec(&manifest) {
                let _ = self.cache.put(&cache_key, &bytes).await;
                let _ = self.cache.put(&digest_key, digest.as_bytes()).await;
            }
            return Some((manifest, digest));
        }

        // 3. Genuine local miss. Return `None` (→ `Ok(None)` at the callers) so
        //    the composite runtime probes the next cache. NEVER fall through to
        //    a network fetch from a `*_local_only` / `*_in_cache_only` path.
        None
    }

    /// Resolve `image`'s OCI `os` field STRICTLY from local stores, with no
    /// network call. Returns `Ok(None)` when the image (its manifest/config) is
    /// not present locally — the caller should then probe the next cache or, as
    /// a last resort, the network.
    ///
    /// # Errors
    /// Returns an error only when a locally-present config blob cannot be read
    /// or parsed — never for a plain local miss (that is `Ok(None)`).
    pub async fn image_os_in_cache_only(&self, image: &str) -> Result<Option<zlayer_spec::OsKind>> {
        let Some((manifest, _digest)) = self.resolve_manifest_local_only(image).await else {
            return Ok(None);
        };

        let config_digest = &manifest.config.digest;
        // The config blob is content-addressed by digest; a local-only blob
        // read never hits the network. If it is somehow absent, treat it as a
        // local miss (Ok(None)) so the caller falls through rather than failing.
        let Some(config_blob) = self.cache.get(config_digest).await.ok().flatten() else {
            return Ok(None);
        };

        let config_root: OciImageConfigRoot =
            serde_json::from_slice(&config_blob).map_err(|e| {
                tracing::error!(
                    error = %e,
                    image = %image,
                    config_digest = %config_digest,
                    "failed to parse cached image config JSON for OS inspection"
                );
                RegistryError::Cache(crate::error::CacheError::Corrupted(format!(
                    "failed to parse image config for {image}: {e}"
                )))
            })?;

        Ok(config_root
            .os
            .as_deref()
            .and_then(zlayer_spec::OsKind::from_oci_str))
    }

    /// Resolve `image`'s `com.zlayer.runtime` manifest annotation STRICTLY from
    /// local stores, with no network call. `Ok(None)` on a local miss (or when
    /// the annotation is absent).
    ///
    /// # Errors
    /// Currently infallible in practice; the `Result` mirrors
    /// [`ImagePuller::image_os_in_cache_only`] for a uniform caller contract.
    pub async fn image_runtime_marker_in_cache_only(&self, image: &str) -> Result<Option<String>> {
        let Some((manifest, _digest)) = self.resolve_manifest_local_only(image).await else {
            return Ok(None);
        };
        Ok(manifest
            .annotations
            .as_ref()
            .and_then(|a| a.get(crate::ZLAYER_RUNTIME_ANNOTATION).cloned()))
    }

    /// Pull an image manifest, honoring the requested [`PullPolicy`].
    ///
    /// `PullPolicy::Always` invalidates any cached manifest entry before the
    /// fetch, guaranteeing a fresh round-trip to the registry.
    /// `PullPolicy::Newer` preserves the legacy revalidate-mutable-tag
    /// behavior. `PullPolicy::IfNotPresent` and `PullPolicy::Never` trust the
    /// blob cache / local registry and skip the remote HEAD revalidation that
    /// would otherwise fail for locally-built images sitting behind a 401.
    ///
    /// # Errors
    ///
    /// Returns an error if the manifest cannot be pulled or the image
    /// reference is invalid.
    pub async fn pull_manifest_with_policy(
        &self,
        image: &str,
        auth: &RegistryAuth,
        policy: PullPolicy,
    ) -> Result<(OciImageManifest, String)> {
        let force_refresh = matches!(policy, PullPolicy::Always);
        if force_refresh {
            let cache_key = manifest_cache_key(image);
            let digest_key = manifest_digest_cache_key(image);
            if let Err(e) = self.cache.delete(&cache_key).await {
                tracing::warn!(
                    image = %image,
                    error = %e,
                    "failed to invalidate manifest cache for force refresh"
                );
            }
            if let Err(e) = self.cache.delete(&digest_key).await {
                tracing::warn!(
                    image = %image,
                    error = %e,
                    "failed to invalidate manifest digest cache for force refresh"
                );
            }
        }
        self.pull_manifest_inner(image, auth, policy).await
    }

    /// Pull and parse the image configuration from the registry.
    ///
    /// This fetches the manifest, extracts the config blob digest, pulls the
    /// config blob, and parses it to extract container runtime defaults like
    /// `Entrypoint`, `Cmd`, `WorkingDir`, `Env`, and `User`.
    ///
    /// # Arguments
    ///
    /// * `image` - Image reference (e.g., "docker.io/library/nginx:latest")
    /// * `auth` - Registry authentication credentials
    ///
    /// # Returns
    ///
    /// Returns the parsed `ImageConfig` containing the container runtime defaults.
    /// If the image config blob has no `config` section, returns a default (empty)
    /// `ImageConfig`.
    ///
    /// # Errors
    ///
    /// Returns an error if the manifest or config blob cannot be fetched, or if
    /// the config blob cannot be parsed as valid JSON.
    pub async fn pull_image_config(&self, image: &str, auth: &RegistryAuth) -> Result<ImageConfig> {
        self.pull_image_config_inner(image, auth, PullPolicy::Newer)
            .await
    }

    /// Internal config pull driven by an explicit [`PullPolicy`].
    async fn pull_image_config_inner(
        &self,
        image: &str,
        auth: &RegistryAuth,
        policy: PullPolicy,
    ) -> Result<ImageConfig> {
        let (manifest, _digest) = self.pull_manifest_inner(image, auth, policy).await?;

        let config_digest = &manifest.config.digest;

        tracing::debug!(
            image = %image,
            config_digest = %config_digest,
            config_media_type = %manifest.config.media_type,
            "fetching image config blob"
        );

        let config_blob = self.pull_blob(image, config_digest, auth).await?;

        let config_root: OciImageConfigRoot =
            serde_json::from_slice(&config_blob).map_err(|e| {
                tracing::error!(
                    error = %e,
                    image = %image,
                    config_digest = %config_digest,
                    "failed to parse image config JSON"
                );
                RegistryError::Cache(crate::error::CacheError::Corrupted(format!(
                    "failed to parse image config for {image}: {e}"
                )))
            })?;

        let config = config_root.config.unwrap_or_default();

        tracing::info!(
            image = %image,
            has_entrypoint = config.entrypoint.is_some(),
            has_cmd = config.cmd.is_some(),
            has_working_dir = config.working_dir.is_some(),
            has_user = config.user.is_some(),
            env_count = config.env.as_ref().map_or(0, std::vec::Vec::len),
            "image config parsed successfully"
        );

        Ok(config)
    }

    /// Pull and parse the image configuration, honoring the requested
    /// [`PullPolicy`].
    ///
    /// `PullPolicy::Always` invalidates the cached manifest entry before
    /// fetching. The config blob itself is content-addressed by digest, so it
    /// does not need explicit invalidation — once the refreshed manifest
    /// points at a new config digest, the existing blob-cache lookup will
    /// naturally miss and refetch. `PullPolicy::IfNotPresent` and
    /// `PullPolicy::Never` skip remote revalidation entirely when a manifest
    /// is already cached.
    ///
    /// # Errors
    ///
    /// Returns an error if the manifest or config blob cannot be fetched, or
    /// if the config blob cannot be parsed as valid JSON.
    pub async fn pull_image_config_with_policy(
        &self,
        image: &str,
        auth: &RegistryAuth,
        policy: PullPolicy,
    ) -> Result<ImageConfig> {
        let force_refresh = matches!(policy, PullPolicy::Always);
        if force_refresh {
            let cache_key = manifest_cache_key(image);
            let digest_key = manifest_digest_cache_key(image);
            if let Err(e) = self.cache.delete(&cache_key).await {
                tracing::warn!(
                    image = %image,
                    error = %e,
                    "failed to invalidate manifest cache for force refresh"
                );
            }
            if let Err(e) = self.cache.delete(&digest_key).await {
                tracing::warn!(
                    image = %image,
                    error = %e,
                    "failed to invalidate manifest digest cache for force refresh"
                );
            }
        }
        self.pull_image_config_inner(image, auth, policy).await
    }

    /// Fetch the operating system targeted by `image` from its OCI config blob.
    ///
    /// Reads the top-level `os` field (OCI-canonical lowercase, e.g.
    /// `"linux"` / `"windows"` / `"darwin"`) from the image config and
    /// converts it via [`zlayer_spec::OsKind::from_oci_str`]. Multi-platform
    /// indexes are resolved by the puller's configured `platform_resolver`
    /// (set at construction time), so the answer reflects the manifest that
    /// would actually be pulled for this host.
    ///
    /// Returns:
    /// * `Ok(Some(os))` when the config blob carries a recognized OS.
    /// * `Ok(None)` when the `os` field is absent or holds an unknown value —
    ///   the caller should treat this as "fall through to a platform-agnostic
    ///   default" rather than an error.
    ///
    /// # Errors
    ///
    /// Returns an error if the manifest or config blob cannot be fetched, or
    /// if the config blob cannot be parsed as valid JSON.
    pub async fn image_os(
        &self,
        image: &str,
        auth: &RegistryAuth,
    ) -> Result<Option<zlayer_spec::OsKind>> {
        self.image_os_with_policy(image, auth, PullPolicy::Newer)
            .await
    }

    /// Like [`ImagePuller::image_os`] but honoring an explicit [`PullPolicy`].
    ///
    /// The composite runtime's OS dispatch path drives this with
    /// [`PullPolicy::IfNotPresent`] so that a locally-cached image (its config
    /// blob already in the persistent blob cache / local registry) is inspected
    /// WITHOUT any network round-trip. This is what keeps a Linux image routing
    /// to the VZ-Linux runtime under a Docker Hub rate-limit: the OS comes from
    /// the local store the pull already wrote, never a re-inspection over the
    /// wire. `IfNotPresent`/`Never` cause [`pull_manifest_inner`] to trust the
    /// cache without the mutable-tag HEAD revalidation that `Newer` would do.
    ///
    /// # Errors
    ///
    /// Returns an error if the manifest or config blob cannot be fetched, or
    /// if the config blob cannot be parsed as valid JSON.
    pub async fn image_os_with_policy(
        &self,
        image: &str,
        auth: &RegistryAuth,
        policy: PullPolicy,
    ) -> Result<Option<zlayer_spec::OsKind>> {
        let (manifest, _digest) = self.pull_manifest_inner(image, auth, policy).await?;

        let config_digest = &manifest.config.digest;
        let config_blob = self.pull_blob(image, config_digest, auth).await?;

        let config_root: OciImageConfigRoot =
            serde_json::from_slice(&config_blob).map_err(|e| {
                tracing::error!(
                    error = %e,
                    image = %image,
                    config_digest = %config_digest,
                    "failed to parse image config JSON for OS inspection"
                );
                RegistryError::Cache(crate::error::CacheError::Corrupted(format!(
                    "failed to parse image config for {image}: {e}"
                )))
            })?;

        Ok(config_root
            .os
            .as_deref()
            .and_then(zlayer_spec::OsKind::from_oci_str))
    }

    /// Read the [`crate::ZLAYER_RUNTIME_ANNOTATION`] manifest annotation, used by
    /// the agent's composite runtime to auto-detect runtime-specific bundles
    /// (e.g. a macOS VZ base bundle published by `zlayer vz build-base`, which
    /// stamps `com.zlayer.runtime=vz`). Returns `Ok(None)` for ordinary images.
    ///
    /// # Errors
    /// Returns an error if the manifest cannot be fetched or parsed.
    pub async fn image_runtime_marker(
        &self,
        image: &str,
        auth: &RegistryAuth,
    ) -> Result<Option<String>> {
        self.image_runtime_marker_with_policy(image, auth, PullPolicy::Newer)
            .await
    }

    /// Like [`ImagePuller::image_runtime_marker`] but honoring an explicit
    /// [`PullPolicy`]. The composite runtime drives this with
    /// [`PullPolicy::IfNotPresent`] so the `com.zlayer.runtime` marker resolves
    /// from the already-pulled local manifest without a network re-fetch.
    ///
    /// # Errors
    /// Returns an error if the manifest cannot be fetched or parsed.
    pub async fn image_runtime_marker_with_policy(
        &self,
        image: &str,
        auth: &RegistryAuth,
        policy: PullPolicy,
    ) -> Result<Option<String>> {
        let (manifest, _digest) = self.pull_manifest_inner(image, auth, policy).await?;
        Ok(manifest
            .annotations
            .as_ref()
            .and_then(|a| a.get(crate::ZLAYER_RUNTIME_ANNOTATION).cloned()))
    }

    /// Fetch the `os.version` string carried by `image`'s OCI config blob.
    ///
    /// For Windows images this corresponds to the host build identifier
    /// (e.g. `"10.0.20348.2031"`) recorded by the image builder, and is the
    /// value the HCS runtime needs to auto-resolve process vs. Hyper-V
    /// isolation against the running host's build. Most Linux images do not
    /// record this field.
    ///
    /// Returns:
    /// * `Ok(Some(version))` when the config blob carries an `os.version`.
    /// * `Ok(None)` when the field is absent — callers should treat this as
    ///   "no builder-asserted version, fall through to a runtime default"
    ///   rather than an error.
    ///
    /// # Errors
    ///
    /// Returns an error if the manifest or config blob cannot be fetched, or
    /// if the config blob cannot be parsed as valid JSON.
    pub async fn image_os_version(
        &self,
        image: &str,
        auth: &RegistryAuth,
    ) -> Result<Option<String>> {
        let (manifest, _digest) = self.pull_manifest(image, auth).await?;

        let config_digest = &manifest.config.digest;
        let config_blob = self.pull_blob(image, config_digest, auth).await?;

        let config_root: OciImageConfigRoot =
            serde_json::from_slice(&config_blob).map_err(|e| {
                tracing::error!(
                    error = %e,
                    image = %image,
                    config_digest = %config_digest,
                    "failed to parse image config JSON for os.version inspection"
                );
                RegistryError::Cache(crate::error::CacheError::Corrupted(format!(
                    "failed to parse image config for {image}: {e}"
                )))
            })?;

        Ok(config_root.os_version)
    }

    /// Detect the artifact type of an image from its manifest
    ///
    /// This method pulls the manifest and determines whether the image is a
    /// traditional container image or a WASM artifact.
    ///
    /// # Arguments
    ///
    /// * `image` - Image reference (e.g., "ghcr.io/myorg/mymodule:v1.0")
    /// * `auth` - Registry authentication credentials
    ///
    /// # Returns
    ///
    /// Returns the detected `ArtifactType` along with the manifest and digest.
    ///
    /// # Errors
    ///
    /// Returns an error if the manifest cannot be pulled.
    pub async fn detect_artifact_type(
        &self,
        image: &str,
        auth: &RegistryAuth,
    ) -> Result<(ArtifactType, OciImageManifest, String)> {
        let (manifest, digest) = self.pull_manifest(image, auth).await?;
        let artifact_type = detect_artifact_type(&manifest);

        tracing::info!(
            image = %image,
            artifact_type = %artifact_type,
            "detected artifact type"
        );

        Ok((artifact_type, manifest, digest))
    }

    /// Extract WASM artifact information from an image
    ///
    /// This method pulls the manifest and extracts detailed information about
    /// a WASM artifact, including WASI version, layer digest, and module name.
    ///
    /// # Arguments
    ///
    /// * `image` - Image reference (e.g., "ghcr.io/myorg/mymodule:v1.0")
    /// * `auth` - Registry authentication credentials
    ///
    /// # Returns
    ///
    /// Returns `Some(WasmArtifactInfo)` if this is a WASM artifact, `None` otherwise.
    ///
    /// # Errors
    ///
    /// Returns an error if the manifest cannot be pulled.
    pub async fn get_wasm_info(
        &self,
        image: &str,
        auth: &RegistryAuth,
    ) -> Result<Option<WasmArtifactInfo>> {
        let (manifest, _digest) = self.pull_manifest(image, auth).await?;
        Ok(extract_wasm_info(&manifest))
    }

    /// Pull a WASM binary from an image
    ///
    /// This method pulls the manifest, verifies it's a WASM artifact, and
    /// returns the raw WASM binary bytes.
    ///
    /// # Arguments
    ///
    /// * `image` - Image reference (e.g., "ghcr.io/myorg/mymodule:v1.0")
    /// * `auth` - Registry authentication credentials
    ///
    /// # Returns
    ///
    /// Returns the WASM binary bytes if this is a WASM artifact.
    ///
    /// # Errors
    ///
    /// Returns an error if this is not a WASM artifact or if the WASM layer
    /// cannot be found.
    pub async fn pull_wasm(
        &self,
        image: &str,
        auth: &RegistryAuth,
    ) -> Result<(Vec<u8>, WasmArtifactInfo)> {
        let (manifest, _digest) = self.pull_manifest(image, auth).await?;

        let wasm_info = extract_wasm_info(&manifest).ok_or_else(|| RegistryError::NotFound {
            registry: "unknown".to_string(),
            image: format!("{image} (not a WASM artifact)"),
        })?;

        let wasm_digest =
            wasm_info
                .wasm_layer_digest
                .as_ref()
                .ok_or_else(|| RegistryError::NotFound {
                    registry: "unknown".to_string(),
                    image: format!("{image} (no WASM layer found)"),
                })?;

        tracing::info!(
            image = %image,
            wasi_version = %wasm_info.wasi_version,
            wasm_digest = %wasm_digest,
            "pulling WASM binary"
        );

        let wasm_bytes = self.pull_blob(image, wasm_digest, auth).await?;

        tracing::info!(
            image = %image,
            wasm_size = wasm_bytes.len(),
            "WASM binary pulled successfully"
        );

        Ok((wasm_bytes, wasm_info))
    }

    /// Pull a complete image (manifest + all layers)
    ///
    /// Returns a vector of (`layer_data`, `media_type`) tuples in order (base layer first).
    ///
    /// # Errors
    ///
    /// Returns an error if the manifest or any layer cannot be pulled.
    pub async fn pull_image(
        &self,
        image: &str,
        auth: &RegistryAuth,
    ) -> Result<Vec<(Vec<u8>, String)>> {
        self.pull_image_inner(image, auth, PullPolicy::Newer).await
    }

    /// Internal image pull driven by an explicit [`PullPolicy`].
    async fn pull_image_inner(
        &self,
        image: &str,
        auth: &RegistryAuth,
        policy: PullPolicy,
    ) -> Result<Vec<(Vec<u8>, String)>> {
        // Pull manifest first
        let (manifest, _digest) = self.pull_manifest_inner(image, auth, policy).await?;

        tracing::info!(
            image = %image,
            layer_count = manifest.layers.len(),
            "pulling image layers"
        );

        // Pull each layer in order
        let mut layers = Vec::with_capacity(manifest.layers.len());
        for (i, layer) in manifest.layers.iter().enumerate() {
            tracing::debug!(
                layer = i,
                digest = %layer.digest,
                media_type = %layer.media_type,
                size = layer.size,
                "pulling layer"
            );

            let layer_urls: &[String] = layer.urls.as_deref().unwrap_or(&[]);
            let data = self
                .pull_blob_with_urls(image, &layer.digest, auth, layer_urls, Some(layer.size))
                .await?;

            // Validate layer data is not empty
            if data.is_empty() {
                return Err(RegistryError::Cache(crate::error::CacheError::Corrupted(
                    format!("layer {i} ({}) is empty after pull", layer.digest),
                )));
            }

            layers.push((data, layer.media_type.clone()));
        }

        tracing::info!(
            image = %image,
            layers_pulled = layers.len(),
            "image pull complete"
        );

        Ok(layers)
    }

    /// Pull a complete image, honoring the requested [`PullPolicy`].
    ///
    /// `PullPolicy::Always` invalidates the cached manifest entry before
    /// fetching. Layer blobs are content-addressed by digest, so they do not
    /// need explicit invalidation — once the refreshed manifest points at new
    /// layer digests, the existing blob-cache lookups will naturally miss and
    /// refetch the new layers. Shared layer blobs remain valid cache hits.
    /// `PullPolicy::IfNotPresent` and `PullPolicy::Never` trust the cache /
    /// local registry without revalidating mutable tags against the remote.
    ///
    /// # Errors
    ///
    /// Returns an error if the manifest or any layer cannot be pulled.
    pub async fn pull_image_with_policy(
        &self,
        image: &str,
        auth: &RegistryAuth,
        policy: PullPolicy,
    ) -> Result<Vec<(Vec<u8>, String)>> {
        let force_refresh = matches!(policy, PullPolicy::Always);
        if force_refresh {
            let cache_key = manifest_cache_key(image);
            let digest_key = manifest_digest_cache_key(image);
            if let Err(e) = self.cache.delete(&cache_key).await {
                tracing::warn!(
                    image = %image,
                    error = %e,
                    "failed to invalidate manifest cache for force refresh"
                );
            }
            if let Err(e) = self.cache.delete(&digest_key).await {
                tracing::warn!(
                    image = %image,
                    error = %e,
                    "failed to invalidate manifest digest cache for force refresh"
                );
            }
        }
        self.pull_image_inner(image, auth, policy).await
    }

    /// Push a blob to a remote registry
    ///
    /// Uploads a blob (layer or config) to the specified registry. The blob is
    /// identified by its digest and will be stored at the repository specified
    /// in the reference.
    ///
    /// # Arguments
    ///
    /// * `reference` - Image reference (e.g., "ghcr.io/myorg/mymodule:v1.0")
    /// * `digest` - Content digest of the blob (e.g., "sha256:abc123...")
    /// * `data` - Raw blob data to upload
    /// * `_media_type` - MIME type of the blob content (reserved for future use)
    /// * `auth` - Registry authentication credentials
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The reference is invalid
    /// - Authentication fails
    /// - The blob upload fails
    #[instrument(
        name = "push_blob",
        skip(self, data, auth, _media_type),
        fields(
            reference = %reference,
            digest = %digest,
            size = data.len(),
        )
    )]
    pub async fn push_blob(
        &self,
        reference: &str,
        digest: &str,
        data: &[u8],
        _media_type: &str,
        auth: &RegistryAuth,
    ) -> std::result::Result<(), PushError> {
        let image_ref: Reference = reference.parse().map_err(|_| PushError::InvalidReference {
            reference: reference.to_string(),
        })?;

        tracing::debug!(
            reference = %reference,
            digest = %digest,
            size = data.len(),
            "pushing blob to registry"
        );

        // Authenticate for push operation
        self.client
            .auth(&image_ref, auth, RegistryOperation::Push)
            .await
            .map_err(|e| PushError::AuthenticationFailed {
                registry: image_ref.resolve_registry().to_string(),
                reason: e.to_string(),
            })?;

        // Push the blob
        self.client
            .push_blob(&image_ref, data, digest)
            .await
            .map_err(|e| PushError::BlobUploadFailed {
                digest: digest.to_string(),
                reason: e.to_string(),
            })?;

        tracing::info!(
            reference = %reference,
            digest = %digest,
            size = data.len(),
            "blob pushed successfully"
        );

        Ok(())
    }

    /// Push a manifest to a remote registry
    ///
    /// Uploads an OCI image manifest to the specified registry. The manifest
    /// should reference blobs that have already been uploaded.
    ///
    /// # Arguments
    ///
    /// * `reference` - Image reference (e.g., "ghcr.io/myorg/mymodule:v1.0")
    /// * `manifest` - OCI image manifest to upload
    /// * `auth` - Registry authentication credentials
    ///
    /// # Returns
    ///
    /// Returns the manifest digest on success.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The reference is invalid
    /// - Authentication fails
    /// - The manifest upload fails
    #[instrument(
        name = "push_manifest_to_registry",
        skip(self, manifest, auth),
        fields(
            reference = %reference,
            layers = manifest.layers.len(),
        )
    )]
    pub async fn push_manifest_to_registry(
        &self,
        reference: &str,
        manifest: &OciImageManifest,
        auth: &RegistryAuth,
    ) -> std::result::Result<String, PushError> {
        let image_ref: Reference = reference.parse().map_err(|_| PushError::InvalidReference {
            reference: reference.to_string(),
        })?;

        tracing::debug!(
            reference = %reference,
            layers = manifest.layers.len(),
            config_digest = %manifest.config.digest,
            "pushing manifest to registry"
        );

        // Authenticate for push operation
        self.client
            .auth(&image_ref, auth, RegistryOperation::Push)
            .await
            .map_err(|e| PushError::AuthenticationFailed {
                registry: image_ref.resolve_registry().to_string(),
                reason: e.to_string(),
            })?;

        // Push the manifest
        let manifest_url = self
            .client
            .push_manifest(&image_ref, &OciManifest::Image(manifest.clone()))
            .await
            .map_err(|e| PushError::ManifestUploadFailed {
                reason: e.to_string(),
            })?;

        // Extract digest from the manifest URL or compute it
        // The manifest URL typically contains the digest after the @ symbol
        let digest = if let Some(digest_start) = manifest_url.rfind('@') {
            manifest_url[digest_start + 1..].to_string()
        } else {
            // Compute digest from manifest JSON
            let manifest_json =
                serde_json::to_vec(manifest).map_err(|e| PushError::ManifestUploadFailed {
                    reason: format!("failed to serialize manifest: {e}"),
                })?;
            crate::cache::compute_digest(&manifest_json)
        };

        tracing::info!(
            reference = %reference,
            digest = %digest,
            manifest_url = %manifest_url,
            "manifest pushed successfully"
        );

        Ok(digest)
    }

    /// Push an OCI image index (manifest list) to a remote registry.
    ///
    /// Used for multi-platform images where each platform has its own manifest
    /// and the index ties them together.
    ///
    /// # Errors
    ///
    /// Returns an error if the reference is invalid, authentication fails, or
    /// the index upload fails.
    #[instrument(
        name = "push_image_index_to_registry",
        skip(self, index, auth),
        fields(
            reference = %reference,
            manifests = index.manifests.len(),
        )
    )]
    pub async fn push_image_index_to_registry(
        &self,
        reference: &str,
        index: &OciImageIndex,
        auth: &RegistryAuth,
    ) -> std::result::Result<String, PushError> {
        let image_ref: Reference = reference.parse().map_err(|_| PushError::InvalidReference {
            reference: reference.to_string(),
        })?;

        tracing::debug!(
            reference = %reference,
            manifests = index.manifests.len(),
            "pushing image index to registry"
        );

        // Authenticate for push operation
        self.client
            .auth(&image_ref, auth, RegistryOperation::Push)
            .await
            .map_err(|e| PushError::AuthenticationFailed {
                registry: image_ref.resolve_registry().to_string(),
                reason: e.to_string(),
            })?;

        // Push the image index
        let manifest_url = self
            .client
            .push_manifest(&image_ref, &OciManifest::ImageIndex(index.clone()))
            .await
            .map_err(|e| PushError::ManifestUploadFailed {
                reason: e.to_string(),
            })?;

        let digest = if let Some(digest_start) = manifest_url.rfind('@') {
            manifest_url[digest_start + 1..].to_string()
        } else {
            let index_json =
                serde_json::to_vec(index).map_err(|e| PushError::ManifestUploadFailed {
                    reason: format!("failed to serialize image index: {e}"),
                })?;
            crate::cache::compute_digest(&index_json)
        };

        tracing::info!(
            reference = %reference,
            digest = %digest,
            "image index pushed successfully"
        );

        Ok(digest)
    }

    /// Push a pre-serialised manifest blob verbatim to a remote registry.
    ///
    /// Unlike [`Self::push_manifest_to_registry`], which round-trips through
    /// `OciImageManifest` and so re-serialises the JSON (potentially losing
    /// byte-level fidelity with the locally-computed digest), this PUTs the
    /// exact bytes the caller already computed a sha256 over. This matters
    /// for Windows WCOW manifests where the layer descriptors carry foreign
    /// `urls[]` arrays that must round-trip unmodified so Windows daemons
    /// recognise the layers as foreign and skip download.
    ///
    /// The `content_type` is sent verbatim as the `Content-Type` header; for
    /// a Docker manifest pass `"application/vnd.docker.distribution.manifest.v2+json"`,
    /// for an OCI image manifest pass `"application/vnd.oci.image.manifest.v1+json"`.
    ///
    /// # Errors
    ///
    /// - [`PushError::InvalidReference`] if `reference` is not a parseable image ref.
    /// - [`PushError::AuthenticationFailed`] on auth failure for the target registry.
    /// - [`PushError::ManifestUploadFailed`] if the underlying PUT fails or the
    ///   provided `content_type` is not a valid HTTP header value.
    #[instrument(
        name = "push_manifest_blob",
        skip(self, manifest_bytes, auth),
        fields(
            reference = %reference,
            content_type = %content_type,
            size = manifest_bytes.len(),
        )
    )]
    pub async fn push_manifest_blob(
        &self,
        reference: &str,
        manifest_bytes: Vec<u8>,
        content_type: &str,
        auth: &RegistryAuth,
    ) -> std::result::Result<String, PushError> {
        let image_ref: Reference = reference.parse().map_err(|_| PushError::InvalidReference {
            reference: reference.to_string(),
        })?;

        let header_value =
            content_type
                .parse()
                .map_err(|e: reqwest::header::InvalidHeaderValue| {
                    PushError::ManifestUploadFailed {
                        reason: format!("invalid Content-Type {content_type:?}: {e}"),
                    }
                })?;

        // Authenticate for push operation
        self.client
            .auth(&image_ref, auth, RegistryOperation::Push)
            .await
            .map_err(|e| PushError::AuthenticationFailed {
                registry: image_ref.resolve_registry().to_string(),
                reason: e.to_string(),
            })?;

        let manifest_url = self
            .client
            .push_manifest_raw(&image_ref, manifest_bytes.clone(), header_value)
            .await
            .map_err(|e| PushError::ManifestUploadFailed {
                reason: e.to_string(),
            })?;

        let digest = if let Some(digest_start) = manifest_url.rfind('@') {
            manifest_url[digest_start + 1..].to_string()
        } else {
            crate::cache::compute_digest(&manifest_bytes)
        };

        tracing::info!(
            reference = %reference,
            digest = %digest,
            "manifest blob pushed successfully"
        );

        Ok(digest)
    }

    /// Push a WASM artifact to a remote registry
    ///
    /// This method pushes a complete WASM artifact including the config blob,
    /// WASM binary layer, and manifest. It uses the result from `export_wasm_as_oci`
    /// to obtain the properly formatted blobs.
    ///
    /// # Arguments
    ///
    /// * `reference` - Image reference (e.g., "ghcr.io/myorg/mymodule:v1.0")
    /// * `export_result` - Result from `export_wasm_as_oci` containing all blobs
    /// * `auth` - Registry authentication credentials
    ///
    /// # Returns
    ///
    /// Returns a `PushResult` containing the manifest digest and list of pushed blobs.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The reference is invalid
    /// - Authentication fails
    /// - Any blob or manifest upload fails
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use zlayer_registry::{ImagePuller, BlobCache};
    /// use zlayer_registry::wasm_export::{WasmExportConfig, export_wasm_as_oci};
    /// use oci_client::secrets::RegistryAuth;
    /// use std::path::PathBuf;
    ///
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let puller = ImagePuller::new(BlobCache::new()?);
    ///
    /// let export_config = WasmExportConfig {
    ///     wasm_path: PathBuf::from("./my_module.wasm"),
    ///     module_name: "my-module".to_string(),
    ///     wasi_version: None,
    ///     annotations: Default::default(),
    /// };
    ///
    /// let export_result = export_wasm_as_oci(&export_config).await?;
    ///
    /// let auth = RegistryAuth::Basic("user".to_string(), "token".to_string());
    /// let push_result = puller.push_wasm(
    ///     "ghcr.io/myorg/my-module:v1.0",
    ///     &export_result,
    ///     &auth,
    /// ).await?;
    ///
    /// println!("Pushed manifest: {}", push_result.manifest_digest);
    /// # Ok(())
    /// # }
    /// ```
    #[cfg(feature = "local")]
    #[allow(clippy::too_many_lines)]
    #[instrument(
        name = "push_wasm",
        skip(self, export_result, auth),
        fields(
            reference = %reference,
            wasm_size = export_result.wasm_size,
            wasi_version = %export_result.wasi_version,
        )
    )]
    pub async fn push_wasm(
        &self,
        reference: &str,
        export_result: &WasmExportResult,
        auth: &RegistryAuth,
    ) -> std::result::Result<PushResult, PushError> {
        use crate::wasm::{WASM_CONFIG_MEDIA_TYPE_V0, WASM_LAYER_MEDIA_TYPE_GENERIC};

        let image_ref: Reference = reference.parse().map_err(|_| PushError::InvalidReference {
            reference: reference.to_string(),
        })?;

        tracing::info!(
            reference = %reference,
            wasm_size = export_result.wasm_size,
            wasi_version = %export_result.wasi_version,
            artifact_type = %export_result.artifact_type,
            "pushing WASM artifact to registry"
        );

        // Authenticate for push operation
        self.client
            .auth(&image_ref, auth, RegistryOperation::Push)
            .await
            .map_err(|e| PushError::AuthenticationFailed {
                registry: image_ref.resolve_registry().to_string(),
                reason: e.to_string(),
            })?;

        let mut blobs_pushed = Vec::new();

        // Push config blob
        tracing::debug!(
            digest = %export_result.config_digest,
            size = export_result.config_size,
            "pushing config blob"
        );
        self.client
            .push_blob(
                &image_ref,
                &export_result.config_blob,
                &export_result.config_digest,
            )
            .await
            .map_err(|e| PushError::BlobUploadFailed {
                digest: export_result.config_digest.clone(),
                reason: e.to_string(),
            })?;
        blobs_pushed.push(export_result.config_digest.clone());

        // Push WASM binary blob
        tracing::debug!(
            digest = %export_result.wasm_layer_digest,
            size = export_result.wasm_size,
            "pushing WASM layer blob"
        );
        self.client
            .push_blob(
                &image_ref,
                &export_result.wasm_binary,
                &export_result.wasm_layer_digest,
            )
            .await
            .map_err(|e| PushError::BlobUploadFailed {
                digest: export_result.wasm_layer_digest.clone(),
                reason: e.to_string(),
            })?;
        blobs_pushed.push(export_result.wasm_layer_digest.clone());

        // Build the manifest from the export result
        #[allow(clippy::cast_possible_wrap)]
        let manifest = OciImageManifest {
            schema_version: 2,
            media_type: Some("application/vnd.oci.image.manifest.v1+json".to_string()),
            artifact_type: Some(export_result.artifact_type.clone()),
            config: oci_client::manifest::OciDescriptor {
                media_type: WASM_CONFIG_MEDIA_TYPE_V0.to_string(),
                digest: export_result.config_digest.clone(),
                size: export_result.config_size as i64,
                urls: None,
                annotations: None,
            },
            layers: vec![oci_client::manifest::OciDescriptor {
                media_type: WASM_LAYER_MEDIA_TYPE_GENERIC.to_string(),
                digest: export_result.wasm_layer_digest.clone(),
                size: export_result.wasm_size as i64,
                urls: None,
                annotations: None,
            }],
            annotations: None,
            subject: None,
        };

        // Push manifest
        tracing::debug!(
            config_digest = %export_result.config_digest,
            wasm_digest = %export_result.wasm_layer_digest,
            "pushing manifest"
        );
        let manifest_url = self
            .client
            .push_manifest(&image_ref, &OciManifest::Image(manifest))
            .await
            .map_err(|e| PushError::ManifestUploadFailed {
                reason: e.to_string(),
            })?;

        // Extract digest from manifest URL or use the precomputed one
        let manifest_digest = if let Some(digest_start) = manifest_url.rfind('@') {
            manifest_url[digest_start + 1..].to_string()
        } else {
            export_result.manifest_digest.clone()
        };

        tracing::info!(
            reference = %reference,
            manifest_digest = %manifest_digest,
            blobs_pushed = blobs_pushed.len(),
            "WASM artifact pushed successfully"
        );

        Ok(PushResult {
            manifest_digest,
            blobs_pushed,
            reference: reference.to_string(),
        })
    }

    /// Push a generic OCI artifact: a config blob plus an ordered set of
    /// [`ArtifactLayer`]s, with arbitrary manifest-level annotations.
    ///
    /// This is the registry-agnostic primitive behind `zlayer vz build-base`,
    /// which publishes a macOS VZ base bundle (`disk.img`, `hardware-model.bin`,
    /// `aux.img`) packed by [`crate::pack::pack_files_tar_zstd`] as a single
    /// `tar+zstd` layer. The manifest annotations carry the routing marker
    /// (`com.zlayer.runtime=vz`) the agent's composite runtime reads to prefer
    /// the VZ runtime for that image.
    ///
    /// Blob digests are computed here over the exact bytes provided. `oci-client`
    /// 0.15 has no streaming blob upload, so each blob is held in memory during
    /// its (chunked-over-the-wire) push; size the host accordingly for
    /// multi-gigabyte disk layers.
    ///
    /// # Errors
    /// Returns [`PushError`] on an invalid reference, auth failure, or a blob /
    /// manifest upload failure.
    #[cfg(feature = "local")]
    #[allow(clippy::too_many_arguments)]
    #[instrument(name = "push_artifact", skip(self, config_blob, layers, annotations, auth), fields(reference = %reference, artifact_type = %artifact_type, layers = layers.len()))]
    pub async fn push_artifact(
        &self,
        reference: &str,
        artifact_type: &str,
        config_media_type: &str,
        config_blob: &[u8],
        layers: &[ArtifactLayer],
        annotations: std::collections::BTreeMap<String, String>,
        auth: &RegistryAuth,
    ) -> std::result::Result<PushResult, PushError> {
        let image_ref: Reference = reference.parse().map_err(|_| PushError::InvalidReference {
            reference: reference.to_string(),
        })?;

        self.client
            .auth(&image_ref, auth, RegistryOperation::Push)
            .await
            .map_err(|e| PushError::AuthenticationFailed {
                registry: image_ref.resolve_registry().to_string(),
                reason: e.to_string(),
            })?;

        let mut blobs_pushed = Vec::new();

        // --- config blob ---
        let config_digest = oci_sha256_digest(config_blob);
        self.client
            .push_blob(&image_ref, config_blob, &config_digest)
            .await
            .map_err(|e| PushError::BlobUploadFailed {
                digest: config_digest.clone(),
                reason: e.to_string(),
            })?;
        blobs_pushed.push(config_digest.clone());

        // --- layer blobs ---
        let mut layer_descriptors = Vec::with_capacity(layers.len());
        for layer in layers {
            let digest = oci_sha256_digest(&layer.data);
            tracing::debug!(
                digest = %digest,
                size = layer.data.len(),
                media_type = %layer.media_type,
                "pushing artifact layer blob"
            );
            self.client
                .push_blob(&image_ref, &layer.data, &digest)
                .await
                .map_err(|e| PushError::BlobUploadFailed {
                    digest: digest.clone(),
                    reason: e.to_string(),
                })?;
            blobs_pushed.push(digest.clone());

            let layer_annotations = layer.title.as_ref().map(|title| {
                let mut m = std::collections::BTreeMap::new();
                m.insert("org.opencontainers.image.title".to_string(), title.clone());
                m
            });
            #[allow(clippy::cast_possible_wrap)]
            layer_descriptors.push(oci_client::manifest::OciDescriptor {
                media_type: layer.media_type.clone(),
                digest,
                size: layer.data.len() as i64,
                urls: None,
                annotations: layer_annotations,
            });
        }

        let manifest_annotations = if annotations.is_empty() {
            None
        } else {
            Some(annotations)
        };

        #[allow(clippy::cast_possible_wrap)]
        let manifest = OciImageManifest {
            schema_version: 2,
            media_type: Some("application/vnd.oci.image.manifest.v1+json".to_string()),
            artifact_type: Some(artifact_type.to_string()),
            config: oci_client::manifest::OciDescriptor {
                media_type: config_media_type.to_string(),
                digest: config_digest.clone(),
                size: config_blob.len() as i64,
                urls: None,
                annotations: None,
            },
            layers: layer_descriptors,
            annotations: manifest_annotations,
            subject: None,
        };

        let manifest_url = self
            .client
            .push_manifest(&image_ref, &OciManifest::Image(manifest))
            .await
            .map_err(|e| PushError::ManifestUploadFailed {
                reason: e.to_string(),
            })?;

        let manifest_digest = manifest_url
            .rfind('@')
            .map(|i| manifest_url[i + 1..].to_string())
            .unwrap_or(manifest_url);

        tracing::info!(
            reference = %reference,
            manifest_digest = %manifest_digest,
            blobs_pushed = blobs_pushed.len(),
            "OCI artifact pushed successfully"
        );

        Ok(PushResult {
            manifest_digest,
            blobs_pushed,
            reference: reference.to_string(),
        })
    }
}

/// Image reference information
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    /// Registry host (e.g., "docker.io", "ghcr.io")
    pub registry: String,
    /// Repository name (e.g., "library/nginx")
    pub repository: String,
    /// Tag (e.g., "latest", "v1.0.0")
    pub tag: String,
}

impl std::fmt::Display for Image {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}:{}", self.registry, self.repository, self.tag)
    }
}

/// Maximum number of `urls[]` entries consulted when a foreign layer 404s on
/// the primary registry. Descriptors rarely carry more than one or two URLs;
/// the cap is a defence against circular / abusive redirect lists.
const MAX_FOREIGN_LAYER_REDIRECTS: usize = 5;

/// Return `true` when an `OciDistributionError` reports that the blob does not
/// exist on the primary registry — i.e. the conditions under which we should
/// fall back to the descriptor's `urls[]` list.
fn is_blob_not_found(err: &oci_client::errors::OciDistributionError) -> bool {
    use oci_client::errors::{OciDistributionError, OciErrorCode};
    match err {
        OciDistributionError::ImageManifestNotFoundError(_) => true,
        OciDistributionError::ServerError { code, .. } => *code == 404,
        OciDistributionError::RegistryError { envelope, .. } => envelope.errors.iter().any(|e| {
            matches!(
                e.code,
                OciErrorCode::BlobUnknown
                    | OciErrorCode::ManifestBlobUnknown
                    | OciErrorCode::ManifestUnknown
                    | OciErrorCode::NotFound
                    | OciErrorCode::NameUnknown
            )
        }),
        OciDistributionError::RequestError(req_err) => {
            req_err.status() == Some(reqwest::StatusCode::NOT_FOUND)
        }
        _ => false,
    }
}

/// Fetch raw bytes from an HTTP(S) URL with optional HTTP Basic authentication.
///
/// This is the low-level primitive used by all other `fetch_*_from_url` helpers
/// in this module — `fetch_blob_from_url` (with digest/size verification),
/// `fetch_archive_from_url` (non-empty archive assertion), and any future
/// content-type-specific fetcher. It performs no validation on the response
/// body beyond HTTP status; callers are responsible for any content-specific
/// checks. `context` is a short human-readable label used to prefix error
/// messages (e.g. `"foreign-layer redirect"`, `"archive fetch"`).
///
/// The caller is responsible for supplying an HTTPS URL when credentials are
/// involved — TLS is never bypassed.
///
/// # Errors
///
/// Returns [`RegistryError::Cache`] on HTTP client construction failure,
/// network error, non-2xx status code, or I/O error while reading the
/// response body. The URL and `context` are included in the error for
/// diagnostics.
pub async fn fetch_from_url(
    url: &str,
    auth: Option<(&str, &str)>,
    context: &str,
) -> Result<Vec<u8>> {
    let client = reqwest::Client::builder().build().map_err(|e| {
        RegistryError::Cache(crate::error::CacheError::Corrupted(format!(
            "failed to build HTTP client for {context} {url}: {e}"
        )))
    })?;

    let mut req = client.get(url);
    if let Some((user, pw)) = auth {
        req = req.basic_auth(user, Some(pw));
    }

    let response = req.send().await.map_err(|e| {
        RegistryError::Cache(crate::error::CacheError::Corrupted(format!(
            "{context} GET {url} failed: {e}"
        )))
    })?;

    if !response.status().is_success() {
        return Err(RegistryError::Cache(crate::error::CacheError::Corrupted(
            format!("{context} {url} returned HTTP {}", response.status()),
        )));
    }

    let bytes = response.bytes().await.map_err(|e| {
        RegistryError::Cache(crate::error::CacheError::Corrupted(format!(
            "failed to read {context} body from {url}: {e}"
        )))
    })?;

    Ok(bytes.to_vec())
}

/// Fetch a foreign layer blob from an out-of-registry URL (e.g. MCR) and
/// verify that its SHA-256 digest matches `expected_digest`. Size is verified
/// against `expected_size` when the value is `Some(n)` and `n >= 0`.
async fn fetch_blob_from_url(
    url: &str,
    expected_digest: &str,
    expected_size: Option<i64>,
) -> Result<Vec<u8>> {
    use sha2::{Digest, Sha256};

    let bytes = fetch_from_url(url, None, "foreign-layer redirect").await?;

    if bytes.is_empty() {
        return Err(RegistryError::Cache(crate::error::CacheError::Corrupted(
            format!("foreign-layer redirect {url} returned empty body"),
        )));
    }

    if let Some(expected) = expected_size {
        if let Ok(expected_u) = u64::try_from(expected) {
            if bytes.len() as u64 != expected_u {
                return Err(RegistryError::Cache(crate::error::CacheError::Corrupted(
                    format!(
                        "foreign-layer redirect {url}: size mismatch (expected {expected}, got {})",
                        bytes.len()
                    ),
                )));
            }
        }
    }

    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let actual = hex::encode(hasher.finalize());
    let expected = expected_digest
        .strip_prefix("sha256:")
        .unwrap_or(expected_digest);
    if !actual.eq_ignore_ascii_case(expected) {
        return Err(RegistryError::Cache(crate::error::CacheError::Corrupted(
            format!(
                "foreign-layer redirect {url}: digest mismatch (expected {expected_digest}, got sha256:{actual})"
            ),
        )));
    }

    Ok(bytes)
}

/// Fetch an OCI tar archive (or tar.gz) from a remote HTTP(S) URL with optional
/// HTTP Basic authentication.
///
/// Unlike [`fetch_blob_from_url`], there is no digest verification — OCI
/// archives are not content-addressable blobs by URL. Validation happens
/// inside the importer via the embedded `oci-layout` + manifest digests.
/// Only asserts the response body is non-empty so callers get a clean error
/// message on accidental 200-with-empty-body responses from misconfigured
/// object stores.
///
/// Suitable for authenticating against Forgejo / Gitea generic-package APIs,
/// Nexus raw repositories, and similar file-blob endpoints.
///
/// # Errors
///
/// Returns [`RegistryError::Cache`] on any HTTP/network error (propagated
/// from [`fetch_from_url`]) or when the response body is empty.
pub async fn fetch_archive_from_url(url: &str, auth: Option<(&str, &str)>) -> Result<Vec<u8>> {
    let bytes = fetch_from_url(url, auth, "archive fetch").await?;

    if bytes.is_empty() {
        return Err(RegistryError::Cache(crate::error::CacheError::Corrupted(
            format!("archive fetch {url} returned empty body"),
        )));
    }

    Ok(bytes)
}

/// Convert a [`zlayer_spec::RegistryAuth`] into the [`RegistryAuth`] shape
/// that the OCI client speaks. Anonymous is returned when `auth` is `None`.
///
/// Basic and Token map onto the same `(username, password)` shape today —
/// the client's bearer-token path is a parsing detail handled by
/// `oci-client` itself. The explicit match keeps us honest when a future
/// [`zlayer_spec::RegistryAuthType`] variant lands with different semantics.
#[must_use]
pub fn spec_auth_to_oci(auth: Option<&zlayer_spec::RegistryAuth>) -> RegistryAuth {
    let Some(a) = auth else {
        return RegistryAuth::Anonymous;
    };
    match a.auth_type {
        zlayer_spec::RegistryAuthType::Basic | zlayer_spec::RegistryAuthType::Token => {
            RegistryAuth::Basic(a.username.clone(), a.password.clone())
        }
    }
}

/// Build an [`ImagePuller`] for one-shot OS / runtime-marker inspection,
/// reusing an existing blob cache (and, when compiled with the `local`
/// feature, an optional [`LocalRegistry`]) so the lookup sees the SAME store
/// the daemon's pull already wrote its manifest and config blob to.
///
/// `cache` should be the persistent blob cache the runtime pulled into (e.g.
/// the VZ-Linux runtime's `{data_dir}/vz/linux/images/blobs.redb`). When that
/// store holds the image, [`ImagePuller::image_os_with_policy`] driven with
/// [`PullPolicy::IfNotPresent`] resolves the OS with no network call.
// Only the `local`-gated `fetch_*_in_cache*` helpers construct an inspection
// puller (they are the entry points that take an explicit on-disk store plus
// an optional local registry), so this helper is itself `local`-gated. Without
// the feature there are no callers, and a non-`local` variant would be dead.
#[cfg(feature = "local")]
fn inspection_puller(
    cache: Arc<Box<dyn BlobCacheBackend>>,
    local_registry: Option<std::sync::Arc<crate::local_registry::LocalRegistry>>,
) -> ImagePuller {
    let puller = ImagePuller::with_cache(cache);
    match local_registry {
        Some(reg) => puller.with_local_registry(reg),
        None => puller,
    }
}

/// Fetch the OCI operating system of `image` LOCAL-FIRST, without pulling any
/// layers and without a network round-trip when the image is already cached.
///
/// Constructs an ephemeral [`ImagePuller`] backed by an in-memory
/// [`BlobCache`] and calls [`ImagePuller::image_os_with_policy`] with
/// [`PullPolicy::IfNotPresent`]. Intended for callers that need a one-shot OS
/// inspection and don't otherwise own a long-lived puller.
///
/// NOTE: because this overload uses a *fresh empty* in-memory cache, the
/// image will not be found locally and a network fetch is required. Callers
/// that own the daemon's persistent store (e.g. the agent's
/// `CompositeRuntime`) should prefer [`fetch_image_os_in_cache`] so the OS is
/// resolved from the already-pulled config blob even under a registry
/// rate-limit. This bare wrapper is kept for callers without a store.
///
/// Auth is the same [`zlayer_spec::RegistryAuth`] carried on the
/// [`Runtime::pull_image_with_policy`](crate) trait; `None` maps to
/// anonymous. The manifest is resolved for the process's runtime platform
/// (with the historical macOS `darwin → linux` fallback) — matching the
/// behavior of [`ImagePuller::new`].
///
/// # Errors
///
/// Returns an error if the in-memory cache cannot be initialized, or if the
/// manifest or config blob cannot be fetched or parsed. Callers in the hot
/// path (e.g. `CompositeRuntime::pull_image_with_policy`) should treat any
/// error as non-fatal: the safe fall-through is "dispatch to the primary
/// runtime" rather than failing the pull.
pub async fn fetch_image_os(
    image: &str,
    auth: Option<&zlayer_spec::RegistryAuth>,
) -> Result<Option<zlayer_spec::OsKind>> {
    let cache = crate::cache::BlobCache::new()?;
    let puller = ImagePuller::new(cache);

    let oci_auth = spec_auth_to_oci(auth);
    puller
        .image_os_with_policy(image, &oci_auth, PullPolicy::IfNotPresent)
        .await
}

/// Resolve the OCI operating system of `image` from an EXISTING local store
/// (the daemon's persistent blob cache plus an optional [`LocalRegistry`])
/// without a network call when the image is present.
///
/// This is the fix for the macOS image-OS-resolution bug: the composite
/// runtime's pull writes the manifest and config blob into a persistent blob
/// cache, then needs to inspect `config.os` to route the workload. Passing
/// that same `cache` (and `local_registry`) here lets
/// [`ImagePuller::image_os_with_policy`] satisfy the lookup from the cached
/// config blob under [`PullPolicy::IfNotPresent`] — so a locally-cached Linux
/// image is detected as Linux and routed to VZ-Linux even when Docker Hub is
/// returning `429 Too Many Requests`. Only a genuinely-absent image falls
/// back to the network.
///
/// # Errors
///
/// Returns an error if the manifest or config blob cannot be resolved from the
/// local store AND a fallback network fetch also fails, or if the config blob
/// cannot be parsed. Hot-path callers should treat any error as non-fatal.
#[cfg(feature = "local")]
pub async fn fetch_image_os_in_cache(
    image: &str,
    auth: Option<&zlayer_spec::RegistryAuth>,
    cache: Arc<Box<dyn BlobCacheBackend>>,
    local_registry: Option<std::sync::Arc<crate::local_registry::LocalRegistry>>,
) -> Result<Option<zlayer_spec::OsKind>> {
    let puller = inspection_puller(cache, local_registry);
    let oci_auth = spec_auth_to_oci(auth);
    puller
        .image_os_with_policy(image, &oci_auth, PullPolicy::IfNotPresent)
        .await
}

/// Convenience wrapper around [`ImagePuller::image_runtime_marker_with_policy`]
/// using a fresh default [`BlobCache`] and [`PullPolicy::IfNotPresent`].
/// Returns the `com.zlayer.runtime` manifest annotation (e.g. `"vz"`) if
/// present, else `Ok(None)`.
///
/// Prefer [`fetch_image_runtime_marker_in_cache`] when the daemon's persistent
/// store is available, so the marker resolves with no network round-trip.
///
/// # Errors
/// Returns an error if a blob cache cannot be created, or the manifest cannot
/// be fetched/parsed.
pub async fn fetch_image_runtime_marker(
    image: &str,
    auth: Option<&zlayer_spec::RegistryAuth>,
) -> Result<Option<String>> {
    let cache = crate::cache::BlobCache::new()?;
    let puller = ImagePuller::new(cache);

    let oci_auth = spec_auth_to_oci(auth);
    puller
        .image_runtime_marker_with_policy(image, &oci_auth, PullPolicy::IfNotPresent)
        .await
}

/// Resolve the `com.zlayer.runtime` marker of `image` from an EXISTING local
/// store (persistent blob cache + optional [`LocalRegistry`]) without a
/// network call when the image is present. The marker counterpart to
/// [`fetch_image_os_in_cache`].
///
/// # Errors
/// Returns an error if the manifest cannot be resolved locally and a fallback
/// network fetch also fails, or the manifest cannot be parsed.
#[cfg(feature = "local")]
pub async fn fetch_image_runtime_marker_in_cache(
    image: &str,
    auth: Option<&zlayer_spec::RegistryAuth>,
    cache: Arc<Box<dyn BlobCacheBackend>>,
    local_registry: Option<std::sync::Arc<crate::local_registry::LocalRegistry>>,
) -> Result<Option<String>> {
    let puller = inspection_puller(cache, local_registry);
    let oci_auth = spec_auth_to_oci(auth);
    puller
        .image_runtime_marker_with_policy(image, &oci_auth, PullPolicy::IfNotPresent)
        .await
}

/// Resolve `image`'s OCI operating system from an EXISTING local store with NO
/// network fallback — `Ok(None)` signals a clean local miss.
///
/// This differs from [`fetch_image_os_in_cache`], which falls through to the
/// network when the image is absent. Use this when probing SEVERAL caches in
/// turn (e.g. the composite runtime's VZ-Linux cache then the primary Sandbox
/// cache): each miss must NOT trigger a Docker Hub round-trip, or the rate-limit
/// the whole feature exists to avoid would fire on the first empty cache.
///
/// # Errors
/// Returns an error only when a locally-present config blob cannot be parsed.
#[cfg(feature = "local")]
pub async fn fetch_image_os_in_cache_only(
    image: &str,
    cache: Arc<Box<dyn BlobCacheBackend>>,
    local_registry: Option<std::sync::Arc<crate::local_registry::LocalRegistry>>,
) -> Result<Option<zlayer_spec::OsKind>> {
    let puller = inspection_puller(cache, local_registry);
    puller.image_os_in_cache_only(image).await
}

/// Resolve `image`'s `com.zlayer.runtime` marker from an EXISTING local store
/// with NO network fallback. The marker counterpart to
/// [`fetch_image_os_in_cache_only`].
///
/// # Errors
/// Mirrors [`fetch_image_os_in_cache_only`]; infallible in practice.
#[cfg(feature = "local")]
pub async fn fetch_image_runtime_marker_in_cache_only(
    image: &str,
    cache: Arc<Box<dyn BlobCacheBackend>>,
    local_registry: Option<std::sync::Arc<crate::local_registry::LocalRegistry>>,
) -> Result<Option<String>> {
    let puller = inspection_puller(cache, local_registry);
    puller.image_runtime_marker_in_cache_only(image).await
}

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // Image Display Tests
    // =========================================================================

    #[test]
    fn test_image_display() {
        let image = Image {
            registry: "docker.io".to_string(),
            repository: "library/nginx".to_string(),
            tag: "latest".to_string(),
        };
        assert_eq!(image.to_string(), "docker.io/library/nginx:latest");
    }

    #[test]
    fn test_image_display_ghcr() {
        let image = Image {
            registry: "ghcr.io".to_string(),
            repository: "myorg/myrepo".to_string(),
            tag: "v1.2.3".to_string(),
        };
        assert_eq!(image.to_string(), "ghcr.io/myorg/myrepo:v1.2.3");
    }

    #[test]
    fn test_image_display_with_nested_repo() {
        let image = Image {
            registry: "gcr.io".to_string(),
            repository: "project/subdir/image".to_string(),
            tag: "sha-abc123".to_string(),
        };
        assert_eq!(image.to_string(), "gcr.io/project/subdir/image:sha-abc123");
    }

    #[test]
    fn test_image_clone() {
        let image = Image {
            registry: "docker.io".to_string(),
            repository: "library/nginx".to_string(),
            tag: "latest".to_string(),
        };
        let cloned = image.clone();
        assert_eq!(image, cloned);
        assert_eq!(cloned.registry, "docker.io");
        assert_eq!(cloned.repository, "library/nginx");
        assert_eq!(cloned.tag, "latest");
    }

    #[test]
    fn test_image_equality() {
        let image1 = Image {
            registry: "docker.io".to_string(),
            repository: "library/nginx".to_string(),
            tag: "latest".to_string(),
        };
        let image2 = Image {
            registry: "docker.io".to_string(),
            repository: "library/nginx".to_string(),
            tag: "latest".to_string(),
        };
        let image3 = Image {
            registry: "ghcr.io".to_string(),
            repository: "library/nginx".to_string(),
            tag: "latest".to_string(),
        };

        assert_eq!(image1, image2);
        assert_ne!(image1, image3);
    }

    #[test]
    fn test_image_debug() {
        let image = Image {
            registry: "docker.io".to_string(),
            repository: "test".to_string(),
            tag: "v1".to_string(),
        };
        let debug_str = format!("{image:?}");
        assert!(debug_str.contains("Image"));
        assert!(debug_str.contains("docker.io"));
        assert!(debug_str.contains("test"));
        assert!(debug_str.contains("v1"));
    }

    // =========================================================================
    // PushError Display Tests
    // =========================================================================

    #[test]
    fn test_push_error_display_authentication_failed() {
        let err = PushError::AuthenticationFailed {
            registry: "ghcr.io".to_string(),
            reason: "invalid token".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "authentication failed for registry ghcr.io: invalid token"
        );
    }

    #[test]
    fn test_push_error_display_authentication_failed_empty_reason() {
        let err = PushError::AuthenticationFailed {
            registry: "docker.io".to_string(),
            reason: String::new(),
        };
        assert_eq!(
            err.to_string(),
            "authentication failed for registry docker.io: "
        );
    }

    #[test]
    fn test_push_error_display_authentication_failed_long_reason() {
        let long_reason = "a]".repeat(100);
        let err = PushError::AuthenticationFailed {
            registry: "ghcr.io".to_string(),
            reason: long_reason.clone(),
        };
        assert!(err.to_string().contains(&long_reason));
    }

    #[test]
    fn test_push_error_display_blob_upload_failed() {
        let err = PushError::BlobUploadFailed {
            digest: "sha256:abc123".to_string(),
            reason: "connection reset".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "failed to upload blob sha256:abc123: connection reset"
        );
    }

    #[test]
    fn test_push_error_display_blob_upload_failed_with_full_digest() {
        let digest =
            "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".to_string();
        let err = PushError::BlobUploadFailed {
            digest: digest.clone(),
            reason: "timeout".to_string(),
        };
        assert!(err.to_string().contains(&digest));
        assert!(err.to_string().contains("timeout"));
    }

    #[test]
    fn test_push_error_display_manifest_upload_failed() {
        let err = PushError::ManifestUploadFailed {
            reason: "invalid manifest".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "failed to upload manifest: invalid manifest"
        );
    }

    #[test]
    fn test_push_error_display_manifest_upload_failed_json_error() {
        let err = PushError::ManifestUploadFailed {
            reason: "invalid JSON: unexpected token at position 42".to_string(),
        };
        assert!(err.to_string().contains("invalid JSON"));
        assert!(err.to_string().contains("position 42"));
    }

    #[test]
    fn test_push_error_display_network_error() {
        let err = PushError::NetworkError("timeout".to_string());
        assert_eq!(err.to_string(), "network error: timeout");
    }

    #[test]
    fn test_push_error_display_network_error_connection_refused() {
        let err = PushError::NetworkError("connection refused: 127.0.0.1:5000".to_string());
        assert_eq!(
            err.to_string(),
            "network error: connection refused: 127.0.0.1:5000"
        );
    }

    #[test]
    fn test_push_error_display_network_error_dns() {
        let err =
            PushError::NetworkError("DNS resolution failed for registry.example.com".to_string());
        assert!(err.to_string().contains("DNS resolution failed"));
    }

    #[test]
    fn test_push_error_display_invalid_reference() {
        let err = PushError::InvalidReference {
            reference: "invalid::ref".to_string(),
        };
        assert_eq!(err.to_string(), "invalid image reference: invalid::ref");
    }

    #[test]
    fn test_push_error_display_invalid_reference_empty() {
        let err = PushError::InvalidReference {
            reference: String::new(),
        };
        assert_eq!(err.to_string(), "invalid image reference: ");
    }

    #[test]
    fn test_push_error_display_invalid_reference_special_chars() {
        let err = PushError::InvalidReference {
            reference: "ghcr.io/test/image:tag@sha256:abc".to_string(),
        };
        assert!(err
            .to_string()
            .contains("ghcr.io/test/image:tag@sha256:abc"));
    }

    // =========================================================================
    // PushError Debug Tests
    // =========================================================================

    #[test]
    fn test_push_error_debug_authentication_failed() {
        let err = PushError::AuthenticationFailed {
            registry: "ghcr.io".to_string(),
            reason: "invalid token".to_string(),
        };
        let debug_str = format!("{err:?}");
        assert!(debug_str.contains("AuthenticationFailed"));
        assert!(debug_str.contains("ghcr.io"));
        assert!(debug_str.contains("invalid token"));
    }

    #[test]
    fn test_push_error_debug_blob_upload_failed() {
        let err = PushError::BlobUploadFailed {
            digest: "sha256:abc123".to_string(),
            reason: "network error".to_string(),
        };
        let debug_str = format!("{err:?}");
        assert!(debug_str.contains("BlobUploadFailed"));
        assert!(debug_str.contains("sha256:abc123"));
        assert!(debug_str.contains("network error"));
    }

    #[test]
    fn test_push_error_debug_manifest_upload_failed() {
        let err = PushError::ManifestUploadFailed {
            reason: "schema validation failed".to_string(),
        };
        let debug_str = format!("{err:?}");
        assert!(debug_str.contains("ManifestUploadFailed"));
        assert!(debug_str.contains("schema validation failed"));
    }

    #[test]
    fn test_push_error_debug_network_error() {
        let err = PushError::NetworkError("connection timed out".to_string());
        let debug_str = format!("{err:?}");
        assert!(debug_str.contains("NetworkError"));
        assert!(debug_str.contains("connection timed out"));
    }

    #[test]
    fn test_push_error_debug_invalid_reference() {
        let err = PushError::InvalidReference {
            reference: "bad-ref".to_string(),
        };
        let debug_str = format!("{err:?}");
        assert!(debug_str.contains("InvalidReference"));
        assert!(debug_str.contains("bad-ref"));
    }

    #[test]
    fn test_push_error_debug_oci_error() {
        // Create an OCI error using a valid variant (GenericError)
        let oci_err =
            oci_client::errors::OciDistributionError::GenericError(Some("test error".to_string()));
        let err = PushError::OciError(oci_err);
        let debug_str = format!("{err:?}");
        assert!(debug_str.contains("OciError"));
    }

    // =========================================================================
    // PushError From OCI Error Tests
    // =========================================================================

    #[test]
    fn test_push_error_from_oci_generic_error() {
        let oci_err = oci_client::errors::OciDistributionError::GenericError(Some(
            "generic error message".to_string(),
        ));
        let push_err: PushError = oci_err.into();
        match push_err {
            PushError::OciError(inner) => {
                assert!(inner.to_string().contains("generic error message"));
            }
            _ => panic!("Expected OciError variant"),
        }
    }

    #[test]
    fn test_push_error_from_oci_config_conversion_error() {
        let oci_err = oci_client::errors::OciDistributionError::ConfigConversionError(
            "config conversion failed".to_string(),
        );
        let push_err: PushError = oci_err.into();
        match push_err {
            PushError::OciError(inner) => {
                assert!(inner.to_string().contains("config conversion"));
            }
            _ => panic!("Expected OciError variant"),
        }
    }

    #[test]
    fn test_push_error_from_oci_manifest_parsing_error() {
        let oci_err = oci_client::errors::OciDistributionError::ManifestParsingError(
            "invalid manifest JSON".to_string(),
        );
        let push_err: PushError = oci_err.into();
        match push_err {
            PushError::OciError(inner) => {
                assert!(
                    inner.to_string().contains("manifest") || inner.to_string().contains("JSON")
                );
            }
            _ => panic!("Expected OciError variant"),
        }
    }

    #[test]
    fn test_push_error_from_oci_image_manifest_not_found() {
        let oci_err = oci_client::errors::OciDistributionError::ImageManifestNotFoundError(
            "image:tag".to_string(),
        );
        let push_err: PushError = oci_err.into();
        match push_err {
            PushError::OciError(inner) => {
                assert!(inner.to_string().contains("image:tag"));
            }
            _ => panic!("Expected OciError variant"),
        }
    }

    #[test]
    fn test_push_error_from_oci_url_parse_error() {
        let oci_err =
            oci_client::errors::OciDistributionError::UrlParseError("invalid URL".to_string());
        let push_err: PushError = oci_err.into();
        match push_err {
            PushError::OciError(inner) => {
                assert!(inner.to_string().contains("URL"));
            }
            _ => panic!("Expected OciError variant"),
        }
    }

    #[test]
    fn test_push_error_from_oci_unsupported_schema_version() {
        let oci_err = oci_client::errors::OciDistributionError::UnsupportedSchemaVersionError(99);
        let push_err: PushError = oci_err.into();
        match push_err {
            PushError::OciError(inner) => {
                // The error should contain information about the version
                let _ = inner.to_string();
            }
            _ => panic!("Expected OciError variant"),
        }
    }

    #[test]
    fn test_push_error_from_oci_unsupported_media_type() {
        let oci_err = oci_client::errors::OciDistributionError::UnsupportedMediaTypeError(
            "application/unknown".to_string(),
        );
        let push_err: PushError = oci_err.into();
        match push_err {
            PushError::OciError(inner) => {
                assert!(inner.to_string().contains("application/unknown"));
            }
            _ => panic!("Expected OciError variant"),
        }
    }

    #[test]
    fn test_push_error_oci_error_display() {
        let oci_err = oci_client::errors::OciDistributionError::GenericError(Some(
            "version mismatch".to_string(),
        ));
        let push_err = PushError::OciError(oci_err);
        assert!(push_err.to_string().contains("OCI distribution error"));
    }

    #[test]
    fn test_push_error_from_oci_spec_violation() {
        let oci_err = oci_client::errors::OciDistributionError::SpecViolationError(
            "OCI spec violation".to_string(),
        );
        let push_err: PushError = oci_err.into();
        match push_err {
            PushError::OciError(inner) => {
                assert!(
                    inner.to_string().contains("spec") || inner.to_string().contains("violation")
                );
            }
            _ => panic!("Expected OciError variant"),
        }
    }

    #[test]
    fn test_push_error_from_oci_versioned_parsing_error() {
        let oci_err = oci_client::errors::OciDistributionError::VersionedParsingError(
            "failed to parse versioned content".to_string(),
        );
        let push_err: PushError = oci_err.into();
        match push_err {
            PushError::OciError(inner) => {
                let _ = inner.to_string();
            }
            _ => panic!("Expected OciError variant"),
        }
    }

    #[test]
    fn test_push_error_from_oci_registry_token_decode() {
        let oci_err = oci_client::errors::OciDistributionError::RegistryTokenDecodeError(
            "invalid token format".to_string(),
        );
        let push_err: PushError = oci_err.into();
        match push_err {
            PushError::OciError(inner) => {
                assert!(inner.to_string().contains("token"));
            }
            _ => panic!("Expected OciError variant"),
        }
    }

    #[test]
    fn test_push_error_from_oci_incompatible_layer_media_type() {
        let oci_err = oci_client::errors::OciDistributionError::IncompatibleLayerMediaTypeError(
            "application/vnd.incompatible".to_string(),
        );
        let push_err: PushError = oci_err.into();
        match push_err {
            PushError::OciError(inner) => {
                assert!(
                    inner.to_string().contains("incompatible")
                        || inner.to_string().contains("layer")
                );
            }
            _ => panic!("Expected OciError variant"),
        }
    }

    // =========================================================================
    // PushResult Tests
    // =========================================================================

    #[test]
    fn test_push_result_creation_all_fields() {
        let result = PushResult {
            manifest_digest: "sha256:abc123".to_string(),
            blobs_pushed: vec!["sha256:def456".to_string(), "sha256:ghi789".to_string()],
            reference: "ghcr.io/test/image:v1.0".to_string(),
        };
        assert_eq!(result.manifest_digest, "sha256:abc123");
        assert_eq!(result.blobs_pushed.len(), 2);
        assert_eq!(result.blobs_pushed[0], "sha256:def456");
        assert_eq!(result.blobs_pushed[1], "sha256:ghi789");
        assert_eq!(result.reference, "ghcr.io/test/image:v1.0");
    }

    #[test]
    fn test_push_result_creation_empty_blobs() {
        let result = PushResult {
            manifest_digest: "sha256:empty".to_string(),
            blobs_pushed: vec![],
            reference: "docker.io/test:latest".to_string(),
        };
        assert!(result.blobs_pushed.is_empty());
        assert_eq!(result.manifest_digest, "sha256:empty");
    }

    #[test]
    fn test_push_result_creation_single_blob() {
        let result = PushResult {
            manifest_digest: "sha256:single".to_string(),
            blobs_pushed: vec!["sha256:only_one".to_string()],
            reference: "registry.example.com/repo:tag".to_string(),
        };
        assert_eq!(result.blobs_pushed.len(), 1);
    }

    #[test]
    fn test_push_result_creation_many_blobs() {
        let blobs: Vec<String> = (0..100).map(|i| format!("sha256:blob{i:03}")).collect();
        let result = PushResult {
            manifest_digest: "sha256:many".to_string(),
            blobs_pushed: blobs,
            reference: "test/image:v1".to_string(),
        };
        assert_eq!(result.blobs_pushed.len(), 100);
        assert_eq!(result.blobs_pushed[0], "sha256:blob000");
        assert_eq!(result.blobs_pushed[99], "sha256:blob099");
    }

    #[test]
    fn test_push_result_clone() {
        let result = PushResult {
            manifest_digest: "sha256:abc".to_string(),
            blobs_pushed: vec!["sha256:def".to_string()],
            reference: "test:v1".to_string(),
        };
        let cloned = result.clone();
        assert_eq!(result.manifest_digest, cloned.manifest_digest);
        assert_eq!(result.blobs_pushed, cloned.blobs_pushed);
        assert_eq!(result.reference, cloned.reference);
    }

    #[test]
    fn test_push_result_clone_independence() {
        let result = PushResult {
            manifest_digest: "sha256:original".to_string(),
            blobs_pushed: vec!["sha256:blob1".to_string()],
            reference: "original:v1".to_string(),
        };
        let mut cloned = result.clone();
        cloned.manifest_digest = "sha256:modified".to_string();
        cloned.blobs_pushed.push("sha256:blob2".to_string());

        // Original should be unchanged
        assert_eq!(result.manifest_digest, "sha256:original");
        assert_eq!(result.blobs_pushed.len(), 1);

        // Cloned should have changes
        assert_eq!(cloned.manifest_digest, "sha256:modified");
        assert_eq!(cloned.blobs_pushed.len(), 2);
    }

    #[test]
    fn test_push_result_debug() {
        let result = PushResult {
            manifest_digest: "sha256:test".to_string(),
            blobs_pushed: vec!["sha256:blob1".to_string(), "sha256:blob2".to_string()],
            reference: "ghcr.io/org/repo:tag".to_string(),
        };
        let debug_str = format!("{result:?}");
        assert!(debug_str.contains("PushResult"));
        assert!(debug_str.contains("sha256:test"));
        assert!(debug_str.contains("sha256:blob1"));
        assert!(debug_str.contains("sha256:blob2"));
        assert!(debug_str.contains("ghcr.io/org/repo:tag"));
    }

    #[test]
    fn test_push_result_with_full_sha256_digest() {
        let full_digest = "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        let result = PushResult {
            manifest_digest: full_digest.to_string(),
            blobs_pushed: vec![full_digest.to_string()],
            reference: "test:latest".to_string(),
        };
        assert_eq!(result.manifest_digest.len(), 71); // "sha256:" + 64 hex chars
    }

    // =========================================================================
    // Reference Parsing Tests (for push methods)
    // =========================================================================

    #[test]
    fn test_valid_reference_parsing_docker_hub() {
        let reference = "docker.io/library/nginx:latest";
        let parsed: Result<Reference, _> = reference.parse();
        assert!(parsed.is_ok());
    }

    #[test]
    fn test_valid_reference_parsing_ghcr() {
        let reference = "ghcr.io/myorg/myrepo:v1.0.0";
        let parsed: Result<Reference, _> = reference.parse();
        assert!(parsed.is_ok());
    }

    #[test]
    fn test_valid_reference_parsing_with_digest() {
        let reference = "ghcr.io/test/image@sha256:abc123def456789012345678901234567890123456789012345678901234";
        let parsed: Result<Reference, _> = reference.parse();
        // Note: The oci_client may or may not accept this format
        // This test documents the current behavior
        let _ = parsed;
    }

    #[test]
    fn test_valid_reference_parsing_nested_repo() {
        let reference = "gcr.io/project-id/subdir/image:tag";
        let parsed: Result<Reference, _> = reference.parse();
        assert!(parsed.is_ok());
    }

    #[test]
    fn test_invalid_reference_empty() {
        let reference = "";
        let parsed: Result<Reference, _> = reference.parse();
        assert!(parsed.is_err());
    }

    #[test]
    fn test_invalid_reference_double_colon() {
        let reference = "invalid::reference";
        let parsed: Result<Reference, _> = reference.parse();
        assert!(parsed.is_err());
    }

    // =========================================================================
    // PushError Construction Helper Tests
    // =========================================================================

    #[test]
    fn test_push_error_authentication_failed_construction() {
        let err = PushError::AuthenticationFailed {
            registry: "test.registry.io".to_string(),
            reason: "credentials expired".to_string(),
        };
        // Verify the error contains the expected information
        let display = err.to_string();
        assert!(display.contains("test.registry.io"));
        assert!(display.contains("credentials expired"));
    }

    #[test]
    fn test_push_error_blob_upload_failed_construction() {
        let digest = "sha256:1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef";
        let err = PushError::BlobUploadFailed {
            digest: digest.to_string(),
            reason: "server returned 500".to_string(),
        };
        let display = err.to_string();
        assert!(display.contains(digest));
        assert!(display.contains("server returned 500"));
    }

    #[test]
    fn test_push_error_manifest_upload_failed_construction() {
        let err = PushError::ManifestUploadFailed {
            reason: "manifest already exists with different content".to_string(),
        };
        let display = err.to_string();
        assert!(display.contains("manifest already exists"));
    }

    #[test]
    fn test_push_error_network_error_construction() {
        let err = PushError::NetworkError("TLS handshake failed".to_string());
        let display = err.to_string();
        assert!(display.contains("TLS handshake failed"));
    }

    #[test]
    fn test_push_error_invalid_reference_construction() {
        let err = PushError::InvalidReference {
            reference: "not/a/valid/reference!!!".to_string(),
        };
        let display = err.to_string();
        assert!(display.contains("not/a/valid/reference!!!"));
    }

    // =========================================================================
    // Error Variant Discrimination Tests
    // =========================================================================

    #[test]
    fn test_push_error_is_authentication_failed() {
        let err = PushError::AuthenticationFailed {
            registry: "test".to_string(),
            reason: "test".to_string(),
        };
        assert!(matches!(err, PushError::AuthenticationFailed { .. }));
        assert!(!matches!(err, PushError::BlobUploadFailed { .. }));
        assert!(!matches!(err, PushError::ManifestUploadFailed { .. }));
        assert!(!matches!(err, PushError::NetworkError(_)));
        assert!(!matches!(err, PushError::InvalidReference { .. }));
        assert!(!matches!(err, PushError::OciError(_)));
    }

    #[test]
    fn test_push_error_is_blob_upload_failed() {
        let err = PushError::BlobUploadFailed {
            digest: "test".to_string(),
            reason: "test".to_string(),
        };
        assert!(!matches!(err, PushError::AuthenticationFailed { .. }));
        assert!(matches!(err, PushError::BlobUploadFailed { .. }));
        assert!(!matches!(err, PushError::ManifestUploadFailed { .. }));
        assert!(!matches!(err, PushError::NetworkError(_)));
        assert!(!matches!(err, PushError::InvalidReference { .. }));
        assert!(!matches!(err, PushError::OciError(_)));
    }

    #[test]
    fn test_push_error_is_manifest_upload_failed() {
        let err = PushError::ManifestUploadFailed {
            reason: "test".to_string(),
        };
        assert!(!matches!(err, PushError::AuthenticationFailed { .. }));
        assert!(!matches!(err, PushError::BlobUploadFailed { .. }));
        assert!(matches!(err, PushError::ManifestUploadFailed { .. }));
        assert!(!matches!(err, PushError::NetworkError(_)));
        assert!(!matches!(err, PushError::InvalidReference { .. }));
        assert!(!matches!(err, PushError::OciError(_)));
    }

    #[test]
    fn test_push_error_is_network_error() {
        let err = PushError::NetworkError("test".to_string());
        assert!(!matches!(err, PushError::AuthenticationFailed { .. }));
        assert!(!matches!(err, PushError::BlobUploadFailed { .. }));
        assert!(!matches!(err, PushError::ManifestUploadFailed { .. }));
        assert!(matches!(err, PushError::NetworkError(_)));
        assert!(!matches!(err, PushError::InvalidReference { .. }));
        assert!(!matches!(err, PushError::OciError(_)));
    }

    #[test]
    fn test_push_error_is_invalid_reference() {
        let err = PushError::InvalidReference {
            reference: "test".to_string(),
        };
        assert!(!matches!(err, PushError::AuthenticationFailed { .. }));
        assert!(!matches!(err, PushError::BlobUploadFailed { .. }));
        assert!(!matches!(err, PushError::ManifestUploadFailed { .. }));
        assert!(!matches!(err, PushError::NetworkError(_)));
        assert!(matches!(err, PushError::InvalidReference { .. }));
        assert!(!matches!(err, PushError::OciError(_)));
    }

    #[test]
    fn test_push_error_is_oci_error() {
        let oci_err =
            oci_client::errors::OciDistributionError::GenericError(Some("test".to_string()));
        let err = PushError::OciError(oci_err);
        assert!(!matches!(err, PushError::AuthenticationFailed { .. }));
        assert!(!matches!(err, PushError::BlobUploadFailed { .. }));
        assert!(!matches!(err, PushError::ManifestUploadFailed { .. }));
        assert!(!matches!(err, PushError::NetworkError(_)));
        assert!(!matches!(err, PushError::InvalidReference { .. }));
        assert!(matches!(err, PushError::OciError(_)));
    }

    // =========================================================================
    // Error std::error::Error Trait Tests
    // =========================================================================

    #[test]
    fn test_push_error_implements_error_trait() {
        let err = PushError::NetworkError("test".to_string());
        // Verify it implements std::error::Error by using the Error trait
        let _: &dyn std::error::Error = &err;
    }

    #[test]
    fn test_push_error_source_for_oci_error() {
        use std::error::Error;
        let oci_err =
            oci_client::errors::OciDistributionError::GenericError(Some("test".to_string()));
        let err = PushError::OciError(oci_err);
        // OciError variant should have a source
        let source = err.source();
        assert!(source.is_some());
    }

    #[test]
    fn test_push_error_source_for_other_variants() {
        use std::error::Error;

        let err1 = PushError::AuthenticationFailed {
            registry: "test".to_string(),
            reason: "test".to_string(),
        };
        assert!(err1.source().is_none());

        let err2 = PushError::BlobUploadFailed {
            digest: "test".to_string(),
            reason: "test".to_string(),
        };
        assert!(err2.source().is_none());

        let err3 = PushError::ManifestUploadFailed {
            reason: "test".to_string(),
        };
        assert!(err3.source().is_none());

        let err4 = PushError::NetworkError("test".to_string());
        assert!(err4.source().is_none());

        let err5 = PushError::InvalidReference {
            reference: "test".to_string(),
        };
        assert!(err5.source().is_none());
    }

    // =========================================================================
    // ImagePuller Creation Tests (without network)
    // =========================================================================

    #[test]
    fn test_image_puller_creation_with_blob_cache() {
        let cache = crate::cache::BlobCache::new().unwrap();
        let puller = ImagePuller::new(cache);
        // Just verify it was created successfully
        let _ = puller;
    }

    #[test]
    fn test_image_puller_with_concurrency_limit() {
        let cache = crate::cache::BlobCache::new().unwrap();
        let puller = ImagePuller::new(cache).with_concurrency_limit(5);
        let _ = puller;
    }

    #[test]
    fn test_image_puller_with_concurrency_limit_one() {
        let cache = crate::cache::BlobCache::new().unwrap();
        let puller = ImagePuller::new(cache).with_concurrency_limit(1);
        let _ = puller;
    }

    #[test]
    fn test_image_puller_with_shared_cache() {
        let cache = crate::cache::BlobCache::new().unwrap();
        let shared: Arc<Box<dyn BlobCacheBackend>> = Arc::new(Box::new(cache));
        let puller = ImagePuller::with_cache(shared);
        let _ = puller;
    }

    // =========================================================================
    // Push Method Reference Validation Tests (async)
    // =========================================================================

    #[tokio::test]
    async fn test_push_blob_invalid_reference_returns_error() {
        let cache = crate::cache::BlobCache::new().unwrap();
        let puller = ImagePuller::new(cache);

        let result = puller
            .push_blob(
                "invalid::reference",
                "sha256:test",
                b"test data",
                "application/octet-stream",
                &RegistryAuth::Anonymous,
            )
            .await;

        assert!(result.is_err());
        match result {
            Err(PushError::InvalidReference { reference }) => {
                assert_eq!(reference, "invalid::reference");
            }
            other => panic!("Expected InvalidReference error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_push_blob_empty_reference_returns_error() {
        let cache = crate::cache::BlobCache::new().unwrap();
        let puller = ImagePuller::new(cache);

        let result = puller
            .push_blob(
                "",
                "sha256:test",
                b"test data",
                "application/octet-stream",
                &RegistryAuth::Anonymous,
            )
            .await;

        assert!(result.is_err());
        match result {
            Err(PushError::InvalidReference { reference }) => {
                assert!(reference.is_empty());
            }
            other => panic!("Expected InvalidReference error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_push_manifest_invalid_reference_returns_error() {
        let cache = crate::cache::BlobCache::new().unwrap();
        let puller = ImagePuller::new(cache);

        let manifest = oci_client::manifest::OciImageManifest::default();

        let result = puller
            .push_manifest_to_registry("invalid::reference", &manifest, &RegistryAuth::Anonymous)
            .await;

        assert!(result.is_err());
        match result {
            Err(PushError::InvalidReference { reference }) => {
                assert_eq!(reference, "invalid::reference");
            }
            other => panic!("Expected InvalidReference error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_push_manifest_empty_reference_returns_error() {
        let cache = crate::cache::BlobCache::new().unwrap();
        let puller = ImagePuller::new(cache);

        let manifest = oci_client::manifest::OciImageManifest::default();

        let result = puller
            .push_manifest_to_registry("", &manifest, &RegistryAuth::Anonymous)
            .await;

        assert!(result.is_err());
        match result {
            Err(PushError::InvalidReference { reference }) => {
                assert!(reference.is_empty());
            }
            other => panic!("Expected InvalidReference error, got {other:?}"),
        }
    }

    // =========================================================================
    // Push WASM Reference Validation Tests (async, feature-gated)
    // =========================================================================

    #[cfg(feature = "local")]
    #[tokio::test]
    async fn test_push_wasm_invalid_reference_returns_error() {
        use crate::wasm::WasiVersion;
        use crate::wasm_export::WasmExportResult;

        let cache = crate::cache::BlobCache::new().unwrap();
        let puller = ImagePuller::new(cache);

        let export_result = WasmExportResult {
            manifest_digest: "sha256:test".to_string(),
            manifest_size: 100,
            wasm_layer_digest: "sha256:wasm".to_string(),
            wasm_size: 1000,
            config_digest: "sha256:config".to_string(),
            config_size: 2,
            wasi_version: WasiVersion::Preview1,
            artifact_type: "application/vnd.wasm.module.v1+wasm".to_string(),
            manifest_json: b"{}".to_vec(),
            config_blob: b"{}".to_vec(),
            wasm_binary: vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00],
        };

        let result = puller
            .push_wasm(
                "invalid::reference",
                &export_result,
                &RegistryAuth::Anonymous,
            )
            .await;

        assert!(result.is_err());
        match result {
            Err(PushError::InvalidReference { reference }) => {
                assert_eq!(reference, "invalid::reference");
            }
            other => panic!("Expected InvalidReference error, got {other:?}"),
        }
    }

    #[cfg(feature = "local")]
    #[tokio::test]
    async fn test_push_wasm_empty_reference_returns_error() {
        use crate::wasm::WasiVersion;
        use crate::wasm_export::WasmExportResult;

        let cache = crate::cache::BlobCache::new().unwrap();
        let puller = ImagePuller::new(cache);

        let export_result = WasmExportResult {
            manifest_digest: "sha256:test".to_string(),
            manifest_size: 100,
            wasm_layer_digest: "sha256:wasm".to_string(),
            wasm_size: 1000,
            config_digest: "sha256:config".to_string(),
            config_size: 2,
            wasi_version: WasiVersion::Preview2,
            artifact_type: "application/vnd.wasm.component.v1+wasm".to_string(),
            manifest_json: b"{}".to_vec(),
            config_blob: b"{}".to_vec(),
            wasm_binary: vec![0x00, 0x61, 0x73, 0x6d, 0x0d, 0x00, 0x01, 0x00],
        };

        let result = puller
            .push_wasm("", &export_result, &RegistryAuth::Anonymous)
            .await;

        assert!(result.is_err());
        match result {
            Err(PushError::InvalidReference { reference }) => {
                assert!(reference.is_empty());
            }
            other => panic!("Expected InvalidReference error, got {other:?}"),
        }
    }

    // =========================================================================
    // WasmExportResult Field Verification Tests (feature-gated)
    // =========================================================================

    #[cfg(feature = "local")]
    #[test]
    fn test_wasm_export_result_debug_formatting() {
        use crate::wasm::WasiVersion;
        use crate::wasm_export::WasmExportResult;

        let result = WasmExportResult {
            manifest_digest: "sha256:manifest".to_string(),
            manifest_size: 500,
            wasm_layer_digest: "sha256:layer".to_string(),
            wasm_size: 10000,
            config_digest: "sha256:cfg".to_string(),
            config_size: 2,
            wasi_version: WasiVersion::Preview2,
            artifact_type: "application/vnd.wasm.component.v1+wasm".to_string(),
            manifest_json: vec![],
            config_blob: vec![],
            wasm_binary: vec![],
        };

        let debug_str = format!("{result:?}");
        assert!(debug_str.contains("WasmExportResult"));
        assert!(debug_str.contains("sha256:manifest"));
        assert!(debug_str.contains("sha256:layer"));
        assert!(debug_str.contains("Preview2"));
    }

    #[cfg(feature = "local")]
    #[test]
    fn test_wasm_export_result_clone_independence() {
        use crate::wasm::WasiVersion;
        use crate::wasm_export::WasmExportResult;

        let result = WasmExportResult {
            manifest_digest: "sha256:original".to_string(),
            manifest_size: 100,
            wasm_layer_digest: "sha256:wasm".to_string(),
            wasm_size: 1000,
            config_digest: "sha256:config".to_string(),
            config_size: 2,
            wasi_version: WasiVersion::Preview1,
            artifact_type: "application/vnd.wasm.module.v1+wasm".to_string(),
            manifest_json: b"{}".to_vec(),
            config_blob: b"{}".to_vec(),
            wasm_binary: vec![0x00, 0x61, 0x73, 0x6d],
        };

        let mut cloned = result.clone();
        cloned.manifest_digest = "sha256:modified".to_string();

        // Original should be unchanged
        assert_eq!(result.manifest_digest, "sha256:original");
        assert_eq!(cloned.manifest_digest, "sha256:modified");
    }

    #[cfg(feature = "local")]
    #[test]
    fn test_wasm_export_result_all_fields_populated() {
        use crate::wasm::WasiVersion;
        use crate::wasm_export::WasmExportResult;

        let wasm_binary = vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
        let config_blob = b"{}".to_vec();
        let manifest_json = b"{\"schemaVersion\": 2}".to_vec();

        let result = WasmExportResult {
            manifest_digest: "sha256:abc".to_string(),
            manifest_size: manifest_json.len() as u64,
            wasm_layer_digest: "sha256:def".to_string(),
            wasm_size: wasm_binary.len() as u64,
            config_digest: "sha256:ghi".to_string(),
            config_size: config_blob.len() as u64,
            wasi_version: WasiVersion::Preview1,
            artifact_type: "application/vnd.wasm.module.v1+wasm".to_string(),
            manifest_json: manifest_json.clone(),
            config_blob: config_blob.clone(),
            wasm_binary: wasm_binary.clone(),
        };

        // Verify all fields
        assert!(result.manifest_digest.starts_with("sha256:"));
        assert_eq!(result.manifest_size, manifest_json.len() as u64);
        assert!(result.wasm_layer_digest.starts_with("sha256:"));
        assert_eq!(result.wasm_size, wasm_binary.len() as u64);
        assert!(result.config_digest.starts_with("sha256:"));
        assert_eq!(result.config_size, config_blob.len() as u64);
        assert_eq!(result.wasi_version, WasiVersion::Preview1);
        assert!(result.artifact_type.contains("wasm"));
        assert!(!result.manifest_json.is_empty());
        assert!(!result.config_blob.is_empty());
        assert!(!result.wasm_binary.is_empty());
    }

    // =========================================================================
    // Integration Tests for Push Flow (Documenting Expected Behavior)
    // =========================================================================
    // Note: These tests document the expected behavior of push operations.
    // Full integration tests would require a mock registry server.

    #[test]
    fn test_push_flow_documentation_blob_before_manifest() {
        // This test documents that blobs must be pushed before the manifest.
        // The manifest references blobs by digest, so blobs must exist first.
        //
        // Expected flow:
        // 1. Push config blob -> get config digest
        // 2. Push layer blob(s) -> get layer digest(s)
        // 3. Build manifest referencing the digests
        // 4. Push manifest
        //
        // This is enforced by the push_wasm method which pushes blobs first.
    }

    #[test]
    fn test_push_flow_documentation_authentication_order() {
        // This test documents that authentication happens before each push operation.
        // The client authenticates separately for blob and manifest pushes.
        //
        // Expected flow for push_blob:
        // 1. Parse reference
        // 2. Authenticate for push operation
        // 3. Push blob data
        //
        // Expected flow for push_manifest:
        // 1. Parse reference
        // 2. Authenticate for push operation
        // 3. Push manifest
    }

    #[test]
    fn test_push_result_blobs_pushed_order() {
        // This test documents that blobs_pushed should contain digests
        // in the order they were pushed (config first, then layers).
        //
        // For WASM artifacts:
        // - blobs_pushed[0] = config digest (empty JSON)
        // - blobs_pushed[1] = wasm layer digest
        let result = PushResult {
            manifest_digest: "sha256:manifest".to_string(),
            blobs_pushed: vec!["sha256:config".to_string(), "sha256:wasm_layer".to_string()],
            reference: "test:v1".to_string(),
        };

        assert_eq!(result.blobs_pushed.len(), 2);
        // Config is pushed first
        assert!(result.blobs_pushed[0].contains("config"));
        // Layer is pushed second
        assert!(result.blobs_pushed[1].contains("layer"));
    }

    // =========================================================================
    // is_mutable_tag Tests
    // =========================================================================

    #[test]
    fn is_mutable_tag_recognises_no_tag() {
        assert!(is_mutable_tag("nginx"));
        assert!(is_mutable_tag("zachhandley/zlayer-manager"));
        assert!(is_mutable_tag("registry.example.com:5000/img"));
    }

    #[test]
    fn is_mutable_tag_recognises_empty_tag() {
        assert!(is_mutable_tag("nginx:"));
    }

    #[test]
    fn is_mutable_tag_recognises_known_mutable_tags() {
        assert!(is_mutable_tag("nginx:latest"));
        assert!(is_mutable_tag("nginx:dev"));
        assert!(is_mutable_tag("nginx:edge"));
        assert!(is_mutable_tag("nginx:main"));
        assert!(is_mutable_tag("nginx:master"));
        assert!(is_mutable_tag("zachhandley/zlayer-manager:latest"));
        assert!(is_mutable_tag("registry.example.com:5000/img:latest"));
    }

    #[test]
    fn is_mutable_tag_rejects_pinned_tags() {
        assert!(!is_mutable_tag("nginx:1.25"));
        assert!(!is_mutable_tag("nginx:1.25.3"));
        assert!(!is_mutable_tag("zachhandley/zlayer-manager:v0.10.70"));
        assert!(!is_mutable_tag("registry.example.com:5000/img:v1"));
    }

    #[test]
    fn is_mutable_tag_rejects_digest_refs() {
        assert!(!is_mutable_tag(
            "img@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        ));
        assert!(!is_mutable_tag(
            "nginx:latest@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        ));
        assert!(!is_mutable_tag(
            "registry.example.com:5000/img@sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        ));
    }

    // =========================================================================
    // Foreign-layer redirect fallback tests (MCR urls[])
    // =========================================================================

    #[test]
    fn is_blob_not_found_detects_404_server_error() {
        use oci_client::errors::OciDistributionError;
        let err = OciDistributionError::ServerError {
            code: 404,
            url: "https://example.com/v2/foo/blobs/sha256:abc".to_string(),
            message: "not found".to_string(),
        };
        assert!(is_blob_not_found(&err));
    }

    #[test]
    fn is_blob_not_found_detects_manifest_not_found() {
        use oci_client::errors::OciDistributionError;
        let err = OciDistributionError::ImageManifestNotFoundError("foo".to_string());
        assert!(is_blob_not_found(&err));
    }

    #[test]
    fn is_blob_not_found_detects_blob_unknown_registry_error() {
        use oci_client::errors::{OciDistributionError, OciEnvelope, OciError, OciErrorCode};
        let envelope = OciEnvelope {
            errors: vec![OciError {
                code: OciErrorCode::BlobUnknown,
                message: "blob gone".to_string(),
                detail: serde_json::Value::Null,
            }],
        };
        let err = OciDistributionError::RegistryError {
            envelope,
            url: "https://example.com/v2/foo/blobs/sha256:abc".to_string(),
        };
        assert!(is_blob_not_found(&err));
    }

    #[test]
    fn is_blob_not_found_rejects_non_404_server_error() {
        use oci_client::errors::OciDistributionError;
        let err = OciDistributionError::ServerError {
            code: 500,
            url: "https://example.com/v2/foo/blobs/sha256:abc".to_string(),
            message: "boom".to_string(),
        };
        assert!(!is_blob_not_found(&err));
    }

    #[test]
    fn is_blob_not_found_rejects_auth_failure() {
        use oci_client::errors::OciDistributionError;
        let err = OciDistributionError::AuthenticationFailure("bad creds".to_string());
        assert!(!is_blob_not_found(&err));
    }

    #[test]
    fn max_foreign_layer_redirects_is_capped() {
        // Guard against silently lifting the redirect cap — the pull path uses
        // .take(MAX_FOREIGN_LAYER_REDIRECTS) to avoid redirect spam.
        assert_eq!(MAX_FOREIGN_LAYER_REDIRECTS, 5);
    }

    #[tokio::test]
    async fn fetch_blob_from_url_rejects_invalid_url() {
        let result = fetch_blob_from_url(
            "not-a-url",
            "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            None,
        )
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn fetch_archive_from_url_rejects_invalid_url() {
        let result = fetch_archive_from_url("not-a-url", None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn fetch_archive_from_url_rejects_unreachable_host() {
        // 127.0.0.1:1 is not bound; connect refused happens synchronously.
        let result = fetch_archive_from_url("http://127.0.0.1:1/archive.tar", None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn fetch_archive_from_url_accepts_basic_auth_without_panic() {
        // Auth should be forwarded to the reqwest builder; errors still surface
        // cleanly when the host is unreachable. Regression guard for the
        // `.basic_auth(user, Some(pw))` call path.
        let result =
            fetch_archive_from_url("http://127.0.0.1:1/archive.tar", Some(("user", "password")))
                .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn fetch_from_url_surfaces_context_in_error() {
        let result = fetch_from_url("http://127.0.0.1:1/thing", None, "test-context").await;
        let Err(err) = result else {
            panic!("expected connect-refused error");
        };
        let msg = err.to_string();
        assert!(
            msg.contains("test-context"),
            "error should include context label, got: {msg}"
        );
    }

    #[tokio::test]
    async fn fetch_blob_from_url_rejects_unreachable_host() {
        // 127.0.0.1:1 is not bound; connect refused happens synchronously.
        let result = fetch_blob_from_url(
            "http://127.0.0.1:1/foo",
            "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            None,
        )
        .await;
        assert!(result.is_err());
    }

    // =========================================================================
    // build_platform_resolver tests — Windows os.version handling
    // =========================================================================

    /// Helper: build a minimal `ImageIndexEntry` with `os` / `arch` / optional
    /// `os.version` — all we need for resolver tests.
    fn mk_entry(os: &str, arch: &str, os_version: Option<&str>, digest: &str) -> ImageIndexEntry {
        ImageIndexEntry {
            media_type: "application/vnd.oci.image.manifest.v1+json".to_string(),
            digest: digest.to_string(),
            size: 0,
            platform: Some(oci_client::manifest::Platform {
                architecture: arch.to_string(),
                os: os.to_string(),
                os_version: os_version.map(str::to_string),
                os_features: None,
                variant: None,
                features: None,
            }),
            annotations: None,
        }
    }

    #[test]
    fn platform_resolver_windows_os_version_prefers_prefix_match() {
        use zlayer_spec::{ArchKind, OsKind, TargetPlatform};
        // Two windows/amd64 entries differing only by os.version. Target
        // matches the 26100 family (Server 2025 / Win11 24H2).
        let manifests = vec![
            mk_entry(
                "windows",
                "amd64",
                Some("10.0.20348.2113"),
                "sha256:server2022",
            ),
            mk_entry(
                "windows",
                "amd64",
                Some("10.0.26100.1742"),
                "sha256:server2025",
            ),
        ];
        let target =
            TargetPlatform::new(OsKind::Windows, ArchKind::Amd64).with_os_version("10.0.26100");
        let resolver = build_platform_resolver(Some(target));
        assert_eq!(resolver(&manifests).as_deref(), Some("sha256:server2025"));
    }

    #[test]
    fn platform_resolver_windows_os_version_exact_match() {
        use zlayer_spec::{ArchKind, OsKind, TargetPlatform};
        let manifests = vec![
            mk_entry(
                "windows",
                "amd64",
                Some("10.0.20348.2113"),
                "sha256:server2022",
            ),
            mk_entry(
                "windows",
                "amd64",
                Some("10.0.26100.1742"),
                "sha256:server2025",
            ),
        ];
        let target = TargetPlatform::new(OsKind::Windows, ArchKind::Amd64)
            .with_os_version("10.0.26100.1742");
        let resolver = build_platform_resolver(Some(target));
        assert_eq!(resolver(&manifests).as_deref(), Some("sha256:server2025"));
    }

    #[test]
    fn platform_resolver_windows_no_os_version_falls_back_to_first_match() {
        use zlayer_spec::{ArchKind, OsKind, TargetPlatform};
        // When the caller doesn't pin os_version the resolver's behavior is
        // unchanged from Phase A: it picks the first os+arch match.
        let manifests = vec![
            mk_entry("windows", "amd64", Some("10.0.20348.2113"), "sha256:first"),
            mk_entry("windows", "amd64", Some("10.0.26100.1742"), "sha256:second"),
        ];
        let target = TargetPlatform::new(OsKind::Windows, ArchKind::Amd64);
        let resolver = build_platform_resolver(Some(target));
        assert_eq!(resolver(&manifests).as_deref(), Some("sha256:first"));
    }

    #[test]
    fn platform_resolver_windows_os_version_falls_back_when_no_version_match() {
        use zlayer_spec::{ArchKind, OsKind, TargetPlatform};
        // Target wants 26100 but the index only has 20348. Fall back to the
        // any-windows/amd64 entry rather than returning None — otherwise a
        // bootstrap image that ships a single old-build manifest is
        // unpullable on a new host.
        let manifests = vec![mk_entry(
            "windows",
            "amd64",
            Some("10.0.20348.2113"),
            "sha256:only",
        )];
        let target =
            TargetPlatform::new(OsKind::Windows, ArchKind::Amd64).with_os_version("10.0.26100");
        let resolver = build_platform_resolver(Some(target));
        assert_eq!(resolver(&manifests).as_deref(), Some("sha256:only"));
    }

    #[test]
    fn platform_resolver_non_windows_ignores_os_version() {
        use zlayer_spec::{ArchKind, OsKind, TargetPlatform};
        // os_version is a Windows-only concept; setting it on a Linux target
        // must NOT accidentally filter out a valid linux/amd64 manifest.
        let manifests = vec![mk_entry("linux", "amd64", None, "sha256:linux")];
        let target =
            TargetPlatform::new(OsKind::Linux, ArchKind::Amd64).with_os_version("ignored.on.linux");
        let resolver = build_platform_resolver(Some(target));
        assert_eq!(resolver(&manifests).as_deref(), Some("sha256:linux"));
    }

    #[test]
    fn manifest_cache_key_is_stable() {
        // The key stem is the CANONICAL reference, so a bare Docker Hub name is
        // normalized to its `docker.io/library/...` form (this is the macOS
        // image-OS-resolution fix: the qualified pull and the bare inspect must
        // land on the SAME key).
        assert_eq!(
            manifest_cache_key("alpine:latest"),
            "manifest:docker.io/library/alpine:latest"
        );
        // An already-qualified ref is unchanged.
        assert_eq!(
            manifest_cache_key("docker.io/library/alpine:latest"),
            "manifest:docker.io/library/alpine:latest"
        );
        // A non-parseable input falls back to the raw string (deterministic).
        assert_eq!(manifest_cache_key(""), "manifest:");
    }

    #[test]
    fn manifest_cache_key_normalizes_bare_and_qualified_to_same_key() {
        // This is bug #1 from the live evidence: the VZ-Linux pull wrote the
        // manifest under the QUALIFIED ref while the composite's inspect queried
        // the BARE ref. Both must hash to one key.
        assert_eq!(
            manifest_cache_key("alpine:latest"),
            manifest_cache_key("docker.io/library/alpine:latest"),
        );
        assert_eq!(
            manifest_digest_cache_key("alpine:latest"),
            manifest_digest_cache_key("docker.io/library/alpine:latest"),
        );
    }

    #[test]
    fn manifest_digest_cache_key_is_stable() {
        assert_eq!(
            manifest_digest_cache_key("alpine:latest"),
            "manifest:digest-docker.io/library/alpine:latest"
        );
    }

    #[test]
    fn manifest_and_digest_keys_never_collide() {
        // The digest variant always has `:digest-` after the leading
        // `manifest`, so for any pair of non-empty image references the two
        // keys are distinct. Spot-check a handful of inputs.
        for image in [
            "alpine",
            "alpine:latest",
            "library/redis:7",
            "ghcr.io/x/y:tag",
        ] {
            let body = manifest_cache_key(image);
            let digest = manifest_digest_cache_key(image);
            assert_ne!(body, digest, "cache-key collision for {image}");
            assert!(body.starts_with("manifest:"), "manifest key shape: {body}");
            assert!(
                digest.starts_with("manifest:digest-"),
                "digest key shape: {digest}"
            );
        }
    }

    // =========================================================================
    // Local-first image-OS / runtime-marker inspection (the macOS rate-limit
    // routing fix). These resolve OS / marker from a pre-seeded blob cache with
    // NO network access, exercising the real `pull_manifest_inner` →
    // `try_cached_manifest` → `pull_blob` (cache-hit) path under
    // `PullPolicy::IfNotPresent`.
    // =========================================================================

    /// Seed `cache` with a manifest + config blob for `image` whose config
    /// declares `os = <os>` and the given optional `com.zlayer.runtime` marker.
    /// Returns the in-memory cache wrapped for `ImagePuller::with_cache`.
    fn seed_cache_with_os(
        image: &str,
        os: &str,
        runtime_marker: Option<&str>,
    ) -> Arc<Box<dyn BlobCacheBackend>> {
        let cache = crate::cache::BlobCache::new().unwrap();

        // Config blob carrying the OS field.
        let config_json = serde_json::json!({
            "architecture": "arm64",
            "os": os,
            "config": {},
        });
        let config_bytes = serde_json::to_vec(&config_json).unwrap();
        let config_digest = crate::cache::compute_digest(&config_bytes);
        cache.put(&config_digest, &config_bytes).unwrap();

        let annotations = runtime_marker.map(|m| {
            let mut a = std::collections::BTreeMap::new();
            a.insert(crate::ZLAYER_RUNTIME_ANNOTATION.to_string(), m.to_string());
            a
        });

        let manifest = OciImageManifest {
            schema_version: 2,
            media_type: Some("application/vnd.oci.image.manifest.v1+json".to_string()),
            artifact_type: None,
            config: oci_client::manifest::OciDescriptor {
                media_type: "application/vnd.oci.image.config.v1+json".to_string(),
                digest: config_digest.clone(),
                size: i64::try_from(config_bytes.len()).unwrap(),
                urls: None,
                annotations: None,
            },
            layers: vec![],
            annotations,
            subject: None,
        };
        let manifest_bytes = serde_json::to_vec(&manifest).unwrap();
        let manifest_digest = crate::cache::compute_digest(&manifest_bytes);

        // Store under the exact keys the puller reads.
        cache
            .put(&manifest_cache_key(image), &manifest_bytes)
            .unwrap();
        cache
            .put(
                &manifest_digest_cache_key(image),
                manifest_digest.as_bytes(),
            )
            .unwrap();

        Arc::new(Box::new(cache) as Box<dyn BlobCacheBackend>)
    }

    #[tokio::test]
    async fn image_os_with_policy_resolves_linux_from_local_cache_no_network() {
        // A locally-cached Linux image must resolve to `OsKind::Linux` purely
        // from the seeded cache — no network. We use a MUTABLE tag (`:latest`)
        // on purpose: under `IfNotPresent` the cache is authoritative and NO
        // remote HEAD revalidation is performed, which is exactly the behaviour
        // that survives a Docker Hub 429.
        let image = "docker.io/library/alpine:latest";
        let cache = seed_cache_with_os(image, "linux", None);
        let puller = ImagePuller::with_cache(cache);

        let os = puller
            .image_os_with_policy(image, &RegistryAuth::Anonymous, PullPolicy::IfNotPresent)
            .await
            .expect("local-first OS inspection must not error");
        assert_eq!(os, Some(zlayer_spec::OsKind::Linux));
    }

    #[cfg(feature = "local")]
    #[tokio::test]
    async fn fetch_image_os_in_cache_resolves_linux_no_network() {
        // Drive the public `fetch_image_os_in_cache` entry point (the one the
        // composite uses) against a seeded cache and confirm it returns
        // `Linux` with no network call.
        let image = "docker.io/library/alpine:latest";
        let cache = seed_cache_with_os(image, "linux", None);

        let os = fetch_image_os_in_cache(image, None, cache, None)
            .await
            .expect("local-first OS inspection must not error");
        assert_eq!(os, Some(zlayer_spec::OsKind::Linux));
    }

    #[cfg(feature = "local")]
    #[tokio::test]
    async fn fetch_image_runtime_marker_in_cache_resolves_marker_no_network() {
        let image = "ghcr.io/example/vz-base:1.0";
        let cache = seed_cache_with_os(image, "linux", Some("vz"));

        let marker = fetch_image_runtime_marker_in_cache(image, None, cache, None)
            .await
            .expect("local-first marker inspection must not error");
        assert_eq!(marker.as_deref(), Some("vz"));
    }

    // -- Live-bug reproductions ------------------------------------------------

    #[tokio::test]
    async fn bare_ref_resolves_from_cache_seeded_under_qualified_ref() {
        // LIVE BUG #1: the VZ-Linux pull wrote the manifest under the QUALIFIED
        // `docker.io/library/alpine:latest`, but the composite's inspect was
        // invoked with the BARE `alpine:latest`. With the canonical key
        // normalization, the bare-ref inspect now hits the qualified-seeded
        // cache — with NO network call.
        let cache = seed_cache_with_os("docker.io/library/alpine:latest", "linux", None);
        let puller = ImagePuller::with_cache(cache);

        let os = puller
            .image_os_in_cache_only("alpine:latest")
            .await
            .expect("bare-ref local inspection must not error");
        assert_eq!(
            os,
            Some(zlayer_spec::OsKind::Linux),
            "bare `alpine:latest` must resolve from a cache seeded under the qualified ref",
        );
    }

    #[cfg(feature = "local")]
    #[tokio::test]
    async fn fetch_image_os_in_cache_only_bare_ref_no_network() {
        // Same bug, driven through the public entry point the composite uses.
        let cache = seed_cache_with_os("docker.io/library/alpine:latest", "linux", None);
        let os = fetch_image_os_in_cache_only("alpine:latest", cache, None)
            .await
            .expect("local-only inspection must not error");
        assert_eq!(os, Some(zlayer_spec::OsKind::Linux));
    }

    #[tokio::test]
    async fn image_os_in_cache_only_no_network_when_manifest_and_config_present() {
        // LIVE BUG #2: resolution must succeed with NO network when the
        // manifest + config are present under `IfNotPresent`. `image_os_in_cache_only`
        // never touches the network, so a green assertion here proves the
        // short-circuit. We use a MUTABLE tag on purpose — the cache must be
        // authoritative without a remote HEAD revalidation (the path that
        // survives a Docker Hub 429).
        let image = "docker.io/library/alpine:latest";
        let cache = seed_cache_with_os(image, "linux", None);
        let puller = ImagePuller::with_cache(cache);

        let os = puller
            .image_os_in_cache_only(image)
            .await
            .expect("cached manifest+config must resolve OS with no network");
        assert_eq!(os, Some(zlayer_spec::OsKind::Linux));
    }

    #[tokio::test]
    async fn image_os_in_cache_only_returns_none_on_clean_local_miss() {
        // A genuinely-absent image must be a clean `Ok(None)` (so the composite
        // can probe the NEXT cache), NOT a network fetch or an error. The cache
        // is empty, so there is nothing to hit the network with anyway.
        let cache: Arc<Box<dyn BlobCacheBackend>> = Arc::new(Box::new(
            crate::cache::BlobCache::new().unwrap(),
        )
            as Box<dyn BlobCacheBackend>);
        let puller = ImagePuller::with_cache(cache);

        let os = puller
            .image_os_in_cache_only("docker.io/library/absent:latest")
            .await
            .expect("a local miss must be Ok(None), never an error");
        assert_eq!(os, None);
        // Even on a clean MISS the local-only path must stay offline.
        assert_eq!(
            puller.network_call_count(),
            0,
            "image_os_in_cache_only must never issue a network call, even on a local miss",
        );
    }

    #[tokio::test]
    async fn image_os_in_cache_only_bare_ref_issues_zero_network_calls() {
        // The load-bearing regression assertion: the BARE `alpine:latest`
        // inspect, served from a cache seeded under the QUALIFIED writer key,
        // must resolve to `Linux` while the network-call spy stays at zero.
        // This is the exact macOS path that previously re-inspected Docker Hub
        // and 429'd, misrouting a cached Linux image to a sandbox (exit 127).
        let cache = seed_cache_with_os("docker.io/library/alpine:latest", "linux", None);
        let puller = ImagePuller::with_cache(cache);

        let os = puller
            .image_os_in_cache_only("alpine:latest")
            .await
            .expect("bare-ref local inspection must not error");

        assert_eq!(os, Some(zlayer_spec::OsKind::Linux));
        assert_eq!(
            puller.network_call_count(),
            0,
            "a cached image's OS must resolve with ZERO network calls (no Docker Hub re-inspect)",
        );
    }

    #[tokio::test]
    async fn image_runtime_marker_in_cache_only_bare_ref_zero_network() {
        // Mirror of the OS path for the `com.zlayer.runtime` annotation: bare
        // ref, qualified-seeded cache, marker resolved, network spy at zero.
        let cache = seed_cache_with_os("docker.io/library/alpine:latest", "linux", Some("vz"));
        let puller = ImagePuller::with_cache(cache);

        let marker = puller
            .image_runtime_marker_in_cache_only("alpine:latest")
            .await
            .expect("bare-ref marker inspection must not error");

        assert_eq!(marker.as_deref(), Some("vz"));
        assert_eq!(
            puller.network_call_count(),
            0,
            "runtime-marker inspection must resolve from cache with ZERO network calls",
        );
    }

    #[tokio::test]
    async fn manifest_cache_key_collides_bare_qualified_and_literal() {
        // Bug #1, asserted end-to-end: the bare ref, the qualified ref, and the
        // exact on-disk writer key must all be ONE key. Reader (bare inspect)
        // and writer (qualified pull) collide → the cache hit lands.
        assert_eq!(
            manifest_cache_key("alpine:latest"),
            "manifest:docker.io/library/alpine:latest",
        );
        assert_eq!(
            manifest_cache_key("docker.io/library/alpine:latest"),
            "manifest:docker.io/library/alpine:latest",
        );
        assert_eq!(
            manifest_cache_key("alpine:latest"),
            manifest_cache_key("docker.io/library/alpine:latest"),
        );
    }
}
