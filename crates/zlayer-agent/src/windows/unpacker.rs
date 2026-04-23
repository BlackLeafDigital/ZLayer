//! Windows OCI layer unpack orchestrator.
//!
//! Given a resolved manifest and a scratch directory, this module pulls each
//! layer blob from the registry (with MCR `urls[]` redirect fallback for
//! foreign Windows layers), decompresses the tar stream, and materializes the
//! wclayer folder layout via [`BackupStreamWriter`] before calling
//! `HcsImportLayer`. The result is a parent chain ready to be paired with a
//! scratch writable layer and attached to a compute system.
//!
//! # Simplifications (provisional first cut)
//!
//! - Layers are buffered fully in memory. A production-grade pipeline would
//!   stream each blob to disk to cap RAM at large-image boundaries.
//! - Digest verification re-hashes the raw (compressed) blob bytes; we don't
//!   independently verify `DiffIDs` on the decompressed tar.
//! - Tombstones and `Hives/` entries are written as raw byte copies. The
//!   `Files/` subtree is fed through [`BackupStreamWriter`] because its
//!   entries are already BackupRead-framed in the wclayer tar.
//!
//! See `hcsshim/internal/wclayer/legacy.go` for the reference flow.

#![cfg(target_os = "windows")]

use std::io;
use std::path::{Path, PathBuf};

use oci_client::secrets::RegistryAuth;
use zlayer_hcs::schema::Layer;

use crate::windows::layer::{self, BackupStreamWriter};
use crate::windows::wclayer::{self, LayerChain};

/// Layer descriptor passed to the unpacker. Structurally mirrors the subset of
/// the OCI `Descriptor` fields we actually consume.
#[derive(Debug, Clone)]
pub struct ResolvedLayerDescriptor {
    /// Content-addressable digest (`sha256:...`) of the blob.
    pub digest: String,
    /// OCI media type — drives the decompressor selection and the
    /// foreign-layer `urls[]` fallback in the registry client.
    pub media_type: String,
    /// Size in bytes, used for the redirect-fallback sanity check.
    pub size: i64,
    /// Optional mirror URLs for foreign layers (non-empty for MCR Windows
    /// base layers; empty for ordinary OCI layers).
    pub urls: Vec<String>,
}

/// Result of an unpack: every layer that was materialized on disk.
#[derive(Debug, Clone)]
pub struct UnpackedImage {
    /// HCS-ordered parent chain (child-to-parent). The first element is the
    /// topmost image layer; the last is the base OS layer. Safe to hand
    /// directly to [`wclayer::attach_layer_storage_filter`] or
    /// [`wclayer::initialize_writable_layer`].
    pub chain: LayerChain,
    /// Root directory that holds all the individual `<layer_id>/` folders.
    pub root: PathBuf,
}

/// Unpack a Windows OCI image into `dest_root`.
///
/// `layers` is the descriptor list from a platform-resolved manifest, in
/// **manifest order** (base first). For each layer, this function:
///
/// 1. Pulls the blob via [`zlayer_registry::client::ImagePuller::pull_blob_with_urls`]
///    (with MCR redirect fallback when a foreign-layer 404 lands).
/// 2. Decompresses the blob according to its media type (`gzip`, `zstd`, or
///    raw tar).
/// 3. Re-hashes and verifies the digest.
/// 4. Allocates a fresh layer id + directory under `dest_root/<uuid>/`.
/// 5. Walks the tar, writing `Files/` entries through [`BackupStreamWriter`]
///    and everything else as raw byte copies.
/// 6. Calls [`wclayer::import_layer`] with the parents collected so far
///    (child-to-parent order).
///
/// Callers must have already resolved a manifest that matches the node's
/// target platform (see [`zlayer_registry::client::ImagePuller::with_platform`]).
///
/// # Errors
///
/// Returns [`io::Error`] on any failure — blob pull, decompress, digest
/// mismatch, `BackupWrite`, or `HcsImportLayer`.
pub async fn unpack_windows_image(
    puller: &zlayer_registry::client::ImagePuller,
    image: &str,
    auth: &RegistryAuth,
    layers: &[ResolvedLayerDescriptor],
    dest_root: &Path,
) -> io::Result<UnpackedImage> {
    // HcsImportLayer requires SeBackupPrivilege + SeRestorePrivilege; enable
    // them once up-front so every subsequent call inherits the adjustment.
    layer::enable_backup_restore_privileges()?;

    std::fs::create_dir_all(dest_root)?;

    // `chain_so_far` grows base-first (manifest order). For each new import
    // we reverse it into child-to-parent to satisfy HCS.
    let mut chain_so_far: Vec<Layer> = Vec::with_capacity(layers.len());

    for desc in layers {
        let layer_id = new_layer_id();
        let layer_path = dest_root.join(&layer_id);
        std::fs::create_dir_all(&layer_path)?;

        // 1. Pull blob (MCR `urls[]` redirect fallback lives inside the
        //    registry client; we just hand it the descriptor.urls list).
        let bytes = puller
            .pull_blob_with_urls(image, &desc.digest, auth, &desc.urls, Some(desc.size))
            .await
            .map_err(|e| io::Error::other(format!("pull blob {}: {e}", desc.digest)))?;

        // 2+3. Digest verification first (cheap, compressed-bytes hash) then
        //      decompress. Registry has already verified on its own path but
        //      double-checking here catches corruption between cache and us.
        verify_digest(&bytes, &desc.digest)?;
        let raw = decompress(&bytes, &desc.media_type)?;

        // 4+5. Materialize tar entries via BackupStreamWriter.
        extract_tar_to_backup_stream(&raw, &layer_path)?;

        // 6. Build parent chain and call HcsImportLayer.
        let parent_chain = build_parent_chain(&chain_so_far);
        wclayer::import_layer(&layer_path, &layer_path, &parent_chain).map_err(|e| {
            io::Error::other(format!("HcsImportLayer({}): {e}", layer_path.display()))
        })?;

        chain_so_far.push(Layer {
            id: layer_id,
            path: layer_path.to_string_lossy().into_owned(),
        });
    }

    // Final external chain is child-to-parent; our accumulator is base-first.
    let mut final_chain = chain_so_far;
    final_chain.reverse();

    Ok(UnpackedImage {
        chain: LayerChain::new(final_chain),
        root: dest_root.to_path_buf(),
    })
}

/// Build the HCS parent chain (child-to-parent) from the base-first accumulator.
fn build_parent_chain(base_first: &[Layer]) -> LayerChain {
    let parents: Vec<Layer> = base_first.iter().rev().cloned().collect();
    LayerChain::new(parents)
}

/// Allocate a fresh layer identifier.
fn new_layer_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Decompress `bytes` according to `media_type`, returning the raw tar stream.
///
/// Unknown media types are treated as already-uncompressed tar.
fn decompress(bytes: &[u8], media_type: &str) -> io::Result<Vec<u8>> {
    use std::io::Read as _;
    let mt = media_type.to_ascii_lowercase();
    if mt.ends_with("+gzip") || mt.ends_with(".tar.gzip") {
        let mut d = flate2::read::GzDecoder::new(bytes);
        let mut out = Vec::new();
        d.read_to_end(&mut out)?;
        Ok(out)
    } else if mt.ends_with("+zstd") || mt.ends_with(".tar.zstd") {
        let mut d = zstd::stream::read::Decoder::new(bytes)?;
        let mut out = Vec::new();
        d.read_to_end(&mut out)?;
        Ok(out)
    } else {
        Ok(bytes.to_vec())
    }
}

/// SHA-256 the blob bytes and compare against `expected` (`sha256:<hex>`).
fn verify_digest(bytes: &[u8], expected: &str) -> io::Result<()> {
    use sha2::{Digest, Sha256};
    let expected_hex = expected.trim_start_matches("sha256:");
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let got = hex::encode(hasher.finalize());
    if !got.eq_ignore_ascii_case(expected_hex) {
        return Err(io::Error::other(format!(
            "blob digest mismatch: expected sha256:{expected_hex}, got sha256:{got}"
        )));
    }
    Ok(())
}

/// Walk a wclayer-formatted tar and materialize each entry under `layer_path`.
///
/// Directory entries are skipped (their paths are implicit in child entries).
/// `Files/`-prefixed entries are fed through [`BackupStreamWriter`] because
/// their payload is already BackupRead-framed in the tar. Everything else
/// (e.g. `tombstones.txt`, `Hives/`) is written as a raw byte copy.
fn extract_tar_to_backup_stream(tar_bytes: &[u8], layer_path: &Path) -> io::Result<()> {
    let mut archive = tar::Archive::new(tar_bytes);
    for entry in archive.entries()? {
        let mut entry = entry?;
        if entry.header().entry_type().is_dir() {
            continue;
        }
        let rel_path = entry.path()?.into_owned();
        let dest = layer_path.join(&rel_path);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // wclayer tar layout:
        //   Files/...            -> BackupWrite stream bodies
        //   Hives/...            -> raw NTFS registry hive exports
        //   tombstones.txt       -> plain-text whiteout manifest
        //   UtilityVM/...        -> raw byte copies (Hyper-V UVM scratch)
        let rel_str = rel_path.to_string_lossy();
        let is_files_payload = rel_str.starts_with("Files/") || rel_str.starts_with("Files\\");
        if is_files_payload {
            let mut writer = BackupStreamWriter::create_new(&dest)?;
            std::io::copy(&mut entry, &mut writer)?;
        } else {
            let mut f = std::fs::File::create(&dest)?;
            std::io::copy(&mut entry, &mut f)?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests (no HCS calls, no registry)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_verify_accepts_matching_hash() {
        let bytes = b"hello world";
        // Known SHA-256 of "hello world".
        let digest = "sha256:b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";
        verify_digest(bytes, digest).expect("should match");
    }

    #[test]
    fn digest_verify_rejects_mismatch() {
        let err = verify_digest(
            b"hello world",
            "sha256:0000000000000000000000000000000000000000000000000000000000000000",
        )
        .expect_err("should reject");
        assert!(err.to_string().contains("digest mismatch"));
    }

    #[test]
    fn digest_verify_is_case_insensitive() {
        let bytes = b"hello world";
        let upper = "sha256:B94D27B9934D3E08A52E52D7DA7DABFAC484EFE37A5380EE9088F7ACE2EFCDE9";
        verify_digest(bytes, upper).expect("should match");
    }

    #[test]
    fn decompress_passthrough_for_unknown_media_type() {
        let out = decompress(b"not compressed", "application/octet-stream").expect("ok");
        assert_eq!(out, b"not compressed");
    }

    #[test]
    fn decompress_gzip_roundtrip() {
        use std::io::Write as _;
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(b"hello tar").unwrap();
        let compressed = gz.finish().unwrap();
        let out = decompress(&compressed, "application/vnd.oci.image.layer.v1.tar+gzip")
            .expect("decompress");
        assert_eq!(out, b"hello tar");
    }

    #[test]
    fn build_parent_chain_reverses_base_first_to_child_first() {
        let base_first = vec![
            Layer {
                id: "base".into(),
                path: r"C:\l\base".into(),
            },
            Layer {
                id: "mid".into(),
                path: r"C:\l\mid".into(),
            },
            Layer {
                id: "top".into(),
                path: r"C:\l\top".into(),
            },
        ];
        let chain = build_parent_chain(&base_first);
        assert_eq!(chain.0.len(), 3);
        assert_eq!(chain.0[0].id, "top");
        assert_eq!(chain.0[1].id, "mid");
        assert_eq!(chain.0[2].id, "base");
    }

    #[test]
    fn build_parent_chain_handles_empty() {
        let chain = build_parent_chain(&[]);
        assert!(chain.0.is_empty());
    }

    #[test]
    fn new_layer_id_is_unique_and_uuid_shaped() {
        let a = new_layer_id();
        let b = new_layer_id();
        assert_ne!(a, b);
        assert_eq!(a.len(), 36); // 8-4-4-4-12 with hyphens
    }
}
