//! Base-image unpack + writable scratch layer creation for the HCS builder.
//!
//! Wraps [`zlayer_agent::windows::scratch::create`] and the agent's
//! [`unpacker::unpack_windows_image`] helper so the builder can, in two
//! calls:
//!
//! 1. Materialize the read-only parent layer chain from an OCI registry
//!    (`windows/amd64` target); capture the base image's config JSON and
//!    compressed layer blobs for the final manifest.
//! 2. Layer a fresh writable sandbox on top with WCIFS attached.
//!
//! Both steps are synchronous w.r.t. HCS (HCS's storage-layer APIs are all
//! synchronous), but the OCI fetch is async — this module exposes one async
//! function for the fetch half and one sync function for the HCS half so the
//! call site can `.await` only where needed.

#![cfg(target_os = "windows")]

use std::io;
use std::path::{Path, PathBuf};

use zlayer_agent::windows::scratch::WritableLayer;
use zlayer_agent::windows::unpacker::{self, ResolvedLayerDescriptor, UnpackedImage};
use zlayer_agent::windows::wclayer::LayerChain;
use zlayer_registry::image_config::ImageConfig;
use zlayer_registry::{ImagePuller, OciImageManifest, RegistryAuth};

/// Everything the builder needs to know about a pulled Windows base image:
/// the parent chain (ready for [`WritableLayer`] construction), the compressed
/// layer blobs it carried (passed straight through to the final manifest), the
/// base image's runtime config, and the `os.version` seen on the selected
/// manifest (required on the final image config per the OCI Windows spec).
#[derive(Debug)]
pub struct BaseChainArtifacts {
    /// HCS parent chain (child-to-parent order) — ready for
    /// [`zlayer_agent::windows::scratch::create`].
    pub parent_chain: LayerChain,
    /// Raw (compressed) layer blobs that make up the base chain.
    ///
    /// Each entry is `(media_type, digest, bytes)`. Ordered base-first so
    /// the final OCI manifest can append the new diff layer on top and keep
    /// the chain in spec-mandated order.
    pub layer_blobs: Vec<BaseLayerBlob>,
    /// Parsed `config` block from the base image's config JSON, if present.
    /// Carried forward into the final image config by
    /// [`crate::backend::hcs::ImageConfigBuilder::inherit_from_base`].
    pub base_config: Option<ImageConfig>,
    /// `os.version` reported by the base manifest's platform descriptor. Used
    /// to populate the final image config's `os.version` field — HCS will
    /// refuse to run a container whose image was built against a different
    /// Windows build family than the host.
    pub os_version: Option<String>,
    /// On-disk location of the unpacked parent chain. Kept alive (not
    /// destroyed) after the build completes so subsequent builds can reuse
    /// it. First-cut: never pruned; future work.
    ///
    /// Not read today (the scratch-layer provisioning below owns the
    /// lifetime); a follow-up commit will wire reuse into `commit.rs`.
    #[allow(dead_code)]
    pub unpacked_root: PathBuf,
}

/// One pre-existing base layer blob, recorded for inclusion in the final
/// OCI manifest.
#[derive(Debug, Clone)]
pub struct BaseLayerBlob {
    /// Media type (`application/vnd.docker.image.rootfs.foreign.diff.tar.gzip`
    /// or `application/vnd.oci.image.layer.v1.tar+gzip` depending on source).
    pub media_type: String,
    /// Content-addressable digest of the compressed blob (`sha256:...`).
    pub digest: String,
    /// Compressed blob bytes. Required in the manifest unchanged; we emit
    /// them as-is into the output OCI layout.
    pub bytes: Vec<u8>,
    /// Optional `urls[]` (non-empty for MCR foreign layers); surfaced on the
    /// manifest descriptor so downstream consumers can follow the fallback.
    pub urls: Vec<String>,
}

/// Pull + unpack the parent layer chain for a Windows base image.
///
/// This is the async half of base-layer preparation. After this returns,
/// [`create_writable_layer`] can be called synchronously to layer a scratch
/// sandbox on top of the returned chain.
///
/// # Errors
///
/// Returns [`io::Error`] on any registry, network, or HCS unpack failure.
pub async fn prepare_base_chain(
    puller: &ImagePuller,
    image: &str,
    dest_root: &Path,
) -> io::Result<BaseChainArtifacts> {
    std::fs::create_dir_all(dest_root)?;

    let auth = RegistryAuth::Anonymous;
    // Pull the manifest (multi-platform images resolve to the puller's
    // configured `windows/amd64` target via its platform_resolver). The
    // returned manifest is the platform-specific one — no further index
    // walking required.
    let (manifest, _manifest_digest) = puller
        .pull_manifest(image, &auth)
        .await
        .map_err(|e| io::Error::other(format!("pull manifest {image}: {e}")))?;

    // Grab the base image config JSON so we can (a) surface it into the
    // final image config (Env/Entrypoint/WorkingDir defaults), and (b) read
    // back the os.version the base was built against.
    let (base_config, os_version) = fetch_base_config_and_version(puller, image, &auth, &manifest)
        .await
        .unwrap_or((None, None));

    // Collect descriptors in manifest (= base-first) order.
    let descriptors: Vec<ResolvedLayerDescriptor> = manifest
        .layers
        .iter()
        .map(|layer| ResolvedLayerDescriptor {
            digest: layer.digest.clone(),
            media_type: layer.media_type.clone(),
            size: layer.size,
            urls: layer.urls.clone().unwrap_or_default(),
        })
        .collect();

    // Unpack into `<dest_root>/unpacked/`; materialises each layer into a
    // wclayer folder + calls HcsImportLayer with the correct parent chain.
    let unpacked_root = dest_root.join("unpacked");
    std::fs::create_dir_all(&unpacked_root)?;
    let UnpackedImage { chain, root } =
        unpacker::unpack_windows_image(puller, image, &auth, &descriptors, &unpacked_root).await?;

    // Pull every blob back out of the cache so we can reproduce the source
    // descriptors on the final image's manifest unchanged. Digest + size
    // verification is already done by the unpacker on the way in; a second
    // cache fetch is cheap.
    let mut layer_blobs = Vec::with_capacity(descriptors.len());
    for desc in &descriptors {
        let bytes = puller
            .pull_blob_with_urls(image, &desc.digest, &auth, &desc.urls, Some(desc.size))
            .await
            .map_err(|e| io::Error::other(format!("refetch base layer {}: {e}", desc.digest)))?;
        layer_blobs.push(BaseLayerBlob {
            media_type: desc.media_type.clone(),
            digest: desc.digest.clone(),
            bytes,
            urls: desc.urls.clone(),
        });
    }

    Ok(BaseChainArtifacts {
        parent_chain: chain,
        layer_blobs,
        base_config,
        os_version,
        unpacked_root: root,
    })
}

/// Create the writable (scratch) layer on top of a prepared parent chain.
///
/// Wraps [`zlayer_agent::windows::scratch::create`] with a default
/// `is_base_os_bootstrap = false` — the base OS layer has already been
/// bootstrapped by [`prepare_base_chain`]'s call to `HcsImportLayer`. If the
/// caller passes a zero-length `parent_chain` (e.g. for a scratch-from-nothing
/// test) this returns an error without touching HCS.
///
/// # Errors
///
/// Returns [`io::Error`] on HCS scratch creation failure.
pub fn create_writable_layer(
    layer_path: &Path,
    parent_chain: &LayerChain,
) -> io::Result<WritableLayer> {
    if parent_chain.0.is_empty() {
        return Err(io::Error::other(
            "HCS scratch layer requires a non-empty parent chain; \
             FROM scratch is not supported by the HCS builder",
        ));
    }
    // Enable privileges once up front — layer-storage operations that touch
    // NTFS reparse / security descriptors require SeBackupPrivilege +
    // SeRestorePrivilege on the current process token.
    zlayer_agent::windows::layer::enable_backup_restore_privileges()?;

    zlayer_agent::windows::scratch::create(
        layer_path,
        parent_chain,
        /* is_base_os_bootstrap = */ false,
    )
}

// ---------------------------------------------------------------------------
// Base image config / os.version extraction
// ---------------------------------------------------------------------------

/// Fetch the base image's config JSON and extract the `os.version`
/// embedded in the manifest's platform descriptor.
///
/// Returns `(Some(config), Some(version))` when both are available,
/// `(None, None)` otherwise. Callers should accept either half being absent.
async fn fetch_base_config_and_version(
    puller: &ImagePuller,
    image: &str,
    auth: &RegistryAuth,
    manifest: &OciImageManifest,
) -> io::Result<(Option<ImageConfig>, Option<String>)> {
    // `manifest.config` is a plain descriptor. Pull the blob, parse the
    // top-level JSON object, look for both the inner `config` block and the
    // top-level `os.version` (OCI image-spec §6).
    let config_blob = puller
        .pull_blob(image, &manifest.config.digest, auth)
        .await
        .map_err(|e| io::Error::other(format!("pull base config: {e}")))?;

    let value: serde_json::Value = serde_json::from_slice(&config_blob)?;
    let os_version = value
        .get("os.version")
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string);
    let image_config = value
        .get("config")
        .cloned()
        .and_then(|v| serde_json::from_value::<ImageConfig>(v).ok());

    Ok((image_config, os_version))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use zlayer_paths::ZLayerDirs;

    #[test]
    fn empty_parent_chain_rejected_before_hcs_call() {
        // We deliberately never touch HCS in this path — an empty chain
        // short-circuits with a clear message so the test runs on any host.
        let tmp = ZLayerDirs::system_default()
            .scratch_dir("empty-parent-chain-rejected-before-hcs-call-")
            .expect("tmpdir");
        let chain = LayerChain::default();
        let err = create_writable_layer(tmp.path(), &chain)
            .expect_err("empty chain must fail before calling HCS");
        assert!(
            err.to_string().contains("non-empty parent chain"),
            "unexpected error: {err}"
        );
    }
}
