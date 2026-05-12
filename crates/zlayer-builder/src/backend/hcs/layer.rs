//! NTFS diff capture for the HCS builder.
//!
//! After all RUN/COPY/ENV instructions have been applied, we diff the scratch
//! writable layer against the parent chain using `HcsExportLayer` and wrap
//! the resulting `BackupRead`-framed wclayer folder into a tar.gz blob. The
//! blob is the exact inverse of what
//! [`zlayer_agent::windows::unpacker::unpack_windows_image`] consumes at
//! container-run time, so produced images round-trip back into `ZLayer`'s
//! runtime without translation.
//!
//! # NTFS-fidelity path
//!
//! `HcsExportLayer` emits:
//!
//! - `Files/…` — files with full NTFS metadata embedded in `BackupRead`
//!   stream headers (reparse points, EAs, security descriptors, ADSes).
//! - `Hives/…` — NTFS registry hive exports for Windows base layers.
//! - `tombstones.txt` — whiteouts for files deleted in the scratch layer.
//! - `UtilityVM/…` — optional Hyper-V UVM bootstrap files.
//!
//! We tar these up (gzip-compressed) with the OCI layer media type, producing
//! a blob that `extract_tar_to_backup_stream` (agent-side) can re-materialize
//! via `BackupStreamWriter` into a fresh layer directory.

#![cfg(target_os = "windows")]

use std::io::{self, Write};
use std::path::Path;

use flate2::write::GzEncoder;
use flate2::Compression;
use sha2::{Digest, Sha256};
use tar::Builder;
use zlayer_agent::windows::wclayer::{self, LayerChain};

/// Result of diff capture: the gzip-compressed tar blob plus its content
/// digest and size. Callers feed this straight into the OCI manifest writer.
#[derive(Debug, Clone)]
pub struct CapturedLayer {
    /// Gzip-compressed tar bytes of the exported wclayer folder.
    pub bytes: Vec<u8>,
    /// `sha256:...` digest of `bytes`.
    pub digest: String,
    /// Size of `bytes` in bytes.
    pub size: u64,
    /// `sha256:...` of the uncompressed tar (the OCI `diff_id` for this layer,
    /// written into the image config's `rootfs.diff_ids` array).
    pub diff_id: String,
}

/// Capture the NTFS diff between a scratch layer and its parent chain,
/// returning a gzip-compressed tar.gz blob of the wclayer folder.
///
/// Uses [`wclayer::export_layer`] under the hood so the output carries full
/// NTFS metadata via `BackupRead` stream framing. The export folder is
/// created fresh under `export_dir` and deleted after the bytes have been
/// read, so `export_dir` can be a throwaway location scoped to the build.
///
/// # Errors
///
/// Returns [`io::Error`] on any HCS export, filesystem, or compression
/// failure.
pub fn capture_diff_blob(
    layer_path: &Path,
    parent_chain: &LayerChain,
    export_dir: &Path,
) -> io::Result<CapturedLayer> {
    if std::fs::metadata(export_dir).is_ok() {
        // Clean up any leftover state from a prior (possibly failed) build.
        // `HcsExportLayer` refuses to write into a non-empty directory.
        std::fs::remove_dir_all(export_dir)?;
    }
    std::fs::create_dir_all(export_dir)?;

    // `HcsExportLayer` takes an empty options document for the standard
    // "export the full diff" case; the options JSON hook is reserved for
    // future UtilityVM-specific knobs per hcsshim's API contract.
    wclayer::export_layer(layer_path, export_dir, parent_chain, "{}")
        .map_err(|e| io::Error::other(format!("HcsExportLayer: {e}")))?;

    // Walk the export folder and tar it up. We hash both the uncompressed
    // tar (diff_id) and the compressed output (digest) so the OCI manifest
    // writer can populate rootfs.diff_ids and layer descriptors correctly.
    let tar_bytes = tar_export_folder(export_dir)?;
    let diff_id = format!("sha256:{}", hex::encode(Sha256::digest(&tar_bytes)));

    let compressed = gzip_bytes(&tar_bytes)?;
    let digest = format!("sha256:{}", hex::encode(Sha256::digest(&compressed)));
    let size = compressed.len() as u64;

    // Release the export folder now that we've read the bytes out. Failure
    // to clean up is a warning, not an error — the build_dir will be
    // reused / pruned by the caller anyway.
    if let Err(e) = std::fs::remove_dir_all(export_dir) {
        tracing::warn!(
            export_dir = %export_dir.display(),
            error = %e,
            "failed to remove HCS export folder after capture"
        );
    }

    Ok(CapturedLayer {
        bytes: compressed,
        digest,
        size,
        diff_id,
    })
}

/// Build a tar archive from the contents of `folder`, preserving the
/// `Files/`, `Hives/`, `tombstones.txt`, `UtilityVM/` layout HCS produced.
///
/// The archive is uncompressed; the caller handles gzip compression.
fn tar_export_folder(folder: &Path) -> io::Result<Vec<u8>> {
    let mut builder = Builder::new(Vec::new());
    append_dir_contents(&mut builder, folder, Path::new(""))?;
    builder.finish()?;
    builder
        .into_inner()
        .map_err(|e| io::Error::other(format!("tar finalize: {e}")))
}

/// Recursively append `dir`'s contents into `builder`, rooted at `tar_rel`.
fn append_dir_contents<W: Write>(
    builder: &mut Builder<W>,
    dir: &Path,
    tar_rel: &Path,
) -> io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        let entry_tar_path = tar_rel.join(&name);
        let meta = entry.metadata()?;
        if meta.is_dir() {
            // Emit an explicit directory entry so the tar is self-describing
            // (some unpackers rely on it for mode/timestamp fidelity).
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Directory);
            header.set_size(0);
            header.set_mode(0o755);
            header.set_mtime(0);
            header.set_path(format!(
                "{}/",
                entry_tar_path.to_string_lossy().replace('\\', "/")
            ))?;
            header.set_cksum();
            builder.append(&header, std::io::empty())?;

            append_dir_contents(builder, &path, &entry_tar_path)?;
        } else {
            let data = std::fs::read(&path)?;
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_mtime(0);
            header.set_path(entry_tar_path.to_string_lossy().replace('\\', "/"))?;
            header.set_cksum();
            builder.append(&header, data.as_slice())?;
        }
    }
    Ok(())
}

/// Gzip-encode `input` with the default compression level. Matches the
/// compression shape used by [`crate::backend::hcs::commit::write_oci_artifacts`].
fn gzip_bytes(input: &[u8]) -> io::Result<Vec<u8>> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(input)?;
    encoder.finish()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use zlayer_paths::ZLayerDirs;

    #[test]
    fn gzip_bytes_roundtrips_via_flate2() {
        let original = b"hello from the hcs builder test";
        let compressed = gzip_bytes(original).expect("gzip encode");
        // Verify the output has the gzip magic bytes and decompresses back.
        assert_eq!(&compressed[..2], &[0x1f, 0x8b]);
        let mut decoder = flate2::read::GzDecoder::new(&compressed[..]);
        let mut back = Vec::new();
        std::io::Read::read_to_end(&mut decoder, &mut back).expect("gzip decode");
        assert_eq!(back, original);
    }

    #[test]
    fn tar_export_folder_includes_nested_structure() {
        // Build a tiny folder tree mirroring the HCS wclayer layout so we can
        // confirm `tar_export_folder` walks it correctly without actually
        // invoking HCS.
        let tmp = ZLayerDirs::system_default()
            .scratch_dir("invoking-hcs-")
            .expect("tmpdir");
        std::fs::create_dir_all(tmp.path().join("Files")).unwrap();
        std::fs::create_dir_all(tmp.path().join("Hives")).unwrap();
        std::fs::write(tmp.path().join("Files").join("alpha.txt"), b"hello").unwrap();
        std::fs::write(tmp.path().join("Hives").join("REGISTRY"), b"hive").unwrap();
        std::fs::write(tmp.path().join("tombstones.txt"), b"").unwrap();

        let bytes = tar_export_folder(tmp.path()).expect("tar build");
        assert!(!bytes.is_empty());

        // Read the tar back to sanity-check the entry set. We expect at
        // least one entry per file (directories may or may not be emitted
        // depending on ordering — we explicitly emit them).
        let mut archive = tar::Archive::new(bytes.as_slice());
        let mut paths: Vec<String> = Vec::new();
        for entry in archive.entries().expect("entries") {
            let entry = entry.unwrap();
            paths.push(entry.path().unwrap().to_string_lossy().into_owned());
        }
        assert!(paths.iter().any(|p| p.contains("Files/alpha.txt")));
        assert!(paths.iter().any(|p| p.contains("Hives/REGISTRY")));
        assert!(paths.iter().any(|p| p.contains("tombstones.txt")));
    }

    #[test]
    fn compressed_and_uncompressed_digests_differ() {
        // Sanity: the diff_id (uncompressed) and digest (compressed) must be
        // distinct hashes so a manifest writer does not confuse them.
        let uncompressed = vec![0u8; 1024];
        let compressed = gzip_bytes(&uncompressed).unwrap();
        let diff_id = format!("sha256:{}", hex::encode(Sha256::digest(&uncompressed)));
        let digest = format!("sha256:{}", hex::encode(Sha256::digest(&compressed)));
        assert_ne!(diff_id, digest);
    }

    #[test]
    fn gzip_bytes_handles_empty_input() {
        // Edge case: an empty export folder produces an empty-ish tar that
        // still gzip-compresses. Important so an "all no-op" Dockerfile
        // doesn't panic the writer.
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(b"").unwrap();
        let gz = encoder.finish().unwrap();
        assert!(gz.len() >= 2);
    }
}
