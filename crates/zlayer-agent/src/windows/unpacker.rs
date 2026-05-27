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
//! - Tombstones and `Hives/` entries are written as raw byte copies (hcsshim's
//!   `BackupFileWriter` interprets framing on the fly and writes only the body
//!   to disk, so the staging layout on disk is unframed). The `Files/` subtree
//!   is translated through [`backuptar`] which synthesises the full
//!   `WIN32_STREAM_ID`-framed records (`BACKUP_DATA`, plus optional
//!   `BACKUP_SECURITY_DATA` / `BACKUP_REPARSE_DATA` / `BACKUP_EA_DATA` pulled
//!   from the entry's PAX extensions) that [`BackupStreamWriter`] expects.
//!
//! See `hcsshim/internal/wclayer/legacy.go` and `go-winio/backuptar/tar.go`
//! for the reference flow.

#![cfg(target_os = "windows")]

use std::io;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use oci_client::secrets::RegistryAuth;
use zlayer_hcs::schema::Layer;

use crate::windows::backuptar;
use crate::windows::layer;
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
    /// directly to [`wclayer::create_sandbox_layer`] or
    /// [`wclayer::prepare_layer`].
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

    for (layer_idx, desc) in layers.iter().enumerate() {
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

        let is_base_layer = chain_so_far.is_empty();
        if is_base_layer {
            // hcsshim's `baseLayerWriter` writes real NTFS files directly into
            // the final layer dir and finalizes via `ProcessBaseLayer` — no
            // `HcsImportLayer` involved (that's only for `legacyLayerWriter`
            // staging dirs, i.e. diff layers).
            extract_tar_to_backup_stream(&raw, &layer_path, true)?;
            wclayer::process_base_layer(&layer_path).map_err(|e| {
                io::Error::other(format!(
                    "ProcessBaseLayer(layer={layer_idx} digest={} dest={}): {e}",
                    desc.digest,
                    layer_path.display()
                ))
            })?;
        } else {
            // Diff layer: materialize the legacy framed staging format, then
            // hand it to `HcsImportLayer` (source must differ from dest — see
            // `wclayer.rs:91-94`).
            let staging_path = dest_root.join(format!("{layer_id}.staging"));
            std::fs::create_dir_all(&staging_path)?;
            extract_tar_to_backup_stream(&raw, &staging_path, false)?;
            let parent_chain = build_parent_chain(&chain_so_far);
            wclayer::import_layer(&layer_path, &staging_path, &parent_chain).map_err(|e| {
                io::Error::other(format!(
                    "HcsImportLayer(layer={layer_idx} digest={} dest={}): {e}",
                    desc.digest,
                    layer_path.display()
                ))
            })?;
            let _ = std::fs::remove_dir_all(&staging_path);
        }

        // HCS keys parent layers by `NameToGuid(basename(path))`, not by the
        // directory's UUID name. Derive the canonical id so HCS can chain-walk.
        chain_so_far.push(Layer {
            id: wclayer::layer_id_for_path(&layer_path)?,
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

/// Walk an OCI Windows layer tar and materialize each entry under `layer_path`.
///
/// Three categories of entry get special treatment beyond the plain
/// `Files/`-vs-other split:
///
/// 1. **Directory entries** (`tar::EntryType::Dir`). These MUST be created on
///    disk — `mcr.microsoft.com/windows/nanoserver` ships thousands of empty
///    leaf directories (`Files/Windows/INF/`, `Files/Windows/System32/Catroot/`,
///    registry/UtilityVM scaffolding) that HCS's `NtQueryDirectoryFile` walk
///    expects to find during `HcsImportLayer`. Silently dropping them yields
///    `STATUS_OBJECT_NAME_NOT_FOUND` → `ERROR_FILE_NOT_FOUND` (`0x80070002`).
/// 2. **Hardlink entries** (`tar::EntryType::Link`). We replay them after the
///    main walk because the tar ordering of `Link` vs. the target it references
///    is not guaranteed in OCI layers.
/// 3. **Whiteouts** — basename-prefixed `.wh.<name>` entries (the OCI
///    overlayfs-style convention). These are NOT extracted as files; instead
///    we append the target path (sibling with `.wh.` stripped) to
///    `tombstones.txt`, which is the on-disk whiteout manifest the wclayer
///    importer consumes. If a raw `tombstones.txt` entry was also present in
///    the tar we APPEND to (not overwrite) it.
///
/// `Files/`-prefixed regular files carry raw file bodies + PAX-encoded NTFS
/// metadata in the OCI format; we hand them to
/// [`backuptar::write_oci_entry_to_backup_stream`] which synthesises the
/// `WIN32_STREAM_ID`-framed records expected by `BackupWrite`. Everything
/// else (raw `tombstones.txt`, `Hives/*`, `UtilityVM/`) is written as a raw
/// byte copy via the long-path-aware helper — hcsshim's `BackupFileWriter`
/// interprets framing on the fly, so the staging-dir layout on disk is
/// unframed.
fn extract_tar_to_backup_stream(
    tar_bytes: &[u8],
    layer_path: &Path,
    is_base_layer: bool,
) -> io::Result<()> {
    if is_base_layer {
        extract_tar_as_base_layer(tar_bytes, layer_path)
    } else {
        extract_tar_as_diff_layer(tar_bytes, layer_path)
    }
}

/// Diff-layer (parent-chain non-empty) tar walk — emits the hcsshim
/// `legacyLayerWriter` on-disk staging format: `.$wcidirs$` markers,
/// 4-byte LE `FileAttributes` headers, verbatim `WIN32_STREAM_ID` records,
/// raw `Hives/*` byte copies, and a `tombstones.txt` whiteout manifest.
fn extract_tar_as_diff_layer(tar_bytes: &[u8], layer_path: &Path) -> io::Result<()> {
    use std::collections::HashSet;

    let mut archive = tar::Archive::new(tar_bytes);
    // Hardlink replay queue: (link_path, target_path), both relative.
    let mut pending_links: Vec<(PathBuf, PathBuf)> = Vec::new();
    // Whiteout collector: relative target paths (forward-slash, no `.wh.`).
    let mut pending_tombstones: Vec<String> = Vec::new();
    // Track raw `tombstones.txt` presence so we APPEND rather than overwrite.
    let mut raw_tombstones_written = false;

    for entry in archive.entries()? {
        let mut entry = entry?;
        let rel_path = entry.path()?.into_owned();
        let entry_type = entry.header().entry_type();

        // (1) Directory entries: create the directory (with all ancestors via
        // the long-path-aware helper) and move on. We deliberately do NOT
        // skip them anymore.
        if entry_type.is_dir() {
            let dest = layer_path.join(&rel_path);
            create_long_path_dir_all(&dest)?;
            write_wcidirs_sidecar(&mut entry, &rel_path, &dest)?;
            continue;
        }

        // (3) Whiteout detection — check basename for `.wh.` prefix. We do
        // this before path construction so we never materialise a `.wh.*`
        // file on disk (HCS would otherwise see it as a literal payload).
        if let Some(basename) = rel_path.file_name().and_then(|s| s.to_str()) {
            if let Some(stripped) = basename.strip_prefix(".wh.") {
                // Reconstruct the tombstone target as the sibling path with
                // `.wh.` stripped, normalised to forward slashes.
                let sibling: PathBuf = match rel_path.parent() {
                    Some(p) if !p.as_os_str().is_empty() => p.join(stripped),
                    _ => PathBuf::from(stripped),
                };
                let normalised: String = sibling
                    .to_string_lossy()
                    .replace('\\', "/")
                    .trim_start_matches('/')
                    .to_string();
                if !normalised.is_empty() {
                    pending_tombstones.push(normalised);
                }
                continue;
            }
        }

        let dest = layer_path.join(&rel_path);
        if let Some(parent) = dest.parent() {
            create_long_path_dir_all(parent)?;
        }

        // (2) Hardlink entries: defer until all targets are materialised.
        if entry_type == tar::EntryType::Link {
            let link_target = entry.link_name()?.ok_or_else(|| {
                io::Error::other(format!(
                    "tar hardlink entry missing link_name: {}",
                    rel_path.display()
                ))
            })?;
            pending_links.push((rel_path.clone(), link_target.into_owned()));
            continue;
        }

        // OCI Windows layer tar layout:
        //   Files/...            -> raw file body + PAX-encoded NTFS metadata
        //                           (translated to BackupWrite framing)
        //   Hives/...            -> raw NTFS registry hive exports
        //   tombstones.txt       -> plain-text whiteout manifest
        //   UtilityVM/...        -> raw byte copies (Hyper-V UVM scratch)
        let rel_str = rel_path.to_string_lossy();
        let is_files_payload = rel_str.starts_with("Files/") || rel_str.starts_with("Files\\");
        if is_files_payload {
            backuptar::write_oci_entry_to_backup_stream(&mut entry, &dest)?;
        } else {
            // Non-`Files/` payloads (`tombstones.txt`, `Hives/*`, `UtilityVM/`)
            // are raw byte copies. `std::fs::File::create` here would hit
            // ERROR_PATH_NOT_FOUND (0x80070003) on the deep UtilityVM/WinSxS
            // paths embedded in nanoserver layers, so we route through the
            // long-path-aware helper which adds the `\\?\` prefix.
            let mut f = layer::create_long_path_file(&dest)?;
            std::io::copy(&mut entry, &mut f)?;
            if rel_str == "tombstones.txt" || rel_str == "tombstones" {
                raw_tombstones_written = true;
            }
        }
    }

    // Replay pending hardlinks. The target must exist on disk by now (the
    // main walk is complete) — if it doesn't we surface the error rather
    // than silently degrade, since downstream HCS will fail anyway.
    for (link_rel, target_rel) in pending_links {
        let link_abs = layer_path.join(&link_rel);
        // Resolve target relative to the staging root. OCI hardlinks use
        // archive-relative target paths.
        let target_abs = layer_path.join(&target_rel);
        if let Some(parent) = link_abs.parent() {
            create_long_path_dir_all(parent)?;
        }
        if let Err(e) = std::fs::hard_link(&target_abs, &link_abs) {
            return Err(io::Error::other(format!(
                "hard_link({} -> {}): {e}",
                link_abs.display(),
                target_abs.display()
            )));
        }
    }

    // Replay whiteouts into `tombstones.txt`. De-duplicate so a malformed
    // tar that lists the same `.wh.foo` twice doesn't bloat the manifest.
    if !pending_tombstones.is_empty() {
        let tombstones_path = layer_path.join("tombstones.txt");
        let mut existing: Vec<String> = Vec::new();
        if raw_tombstones_written && tombstones_path.exists() {
            // Pull current contents back via the long-path helper to avoid
            // a `0x80070003` re-read of a deep path. tombstones.txt itself
            // lives at the layer root so this is mostly defensive.
            let bytes = std::fs::read(&tombstones_path)?;
            for line in bytes.split(|&b| b == b'\n') {
                if line.is_empty() {
                    continue;
                }
                existing.push(String::from_utf8_lossy(line).trim().to_string());
            }
        }
        let mut seen: HashSet<String> = existing.iter().cloned().collect();
        let mut all_lines = existing;
        for line in pending_tombstones {
            if seen.insert(line.clone()) {
                all_lines.push(line);
            }
        }
        let body = all_lines.join("\n") + "\n";
        let mut f = layer::create_long_path_file(&tombstones_path)?;
        std::io::Write::write_all(&mut f, body.as_bytes())?;
    }

    Ok(())
}

/// Base-layer (parent-chain empty) tar walk — emits the hcsshim
/// `baseLayerWriter` on-disk format: REAL NTFS directories (no
/// `.$wcidirs$` markers), real NTFS files with metadata stamped via
/// `BackupWrite` (no 4-byte attr header, no verbatim framing), and NO
/// tombstones (`baseLayerWriter.Remove` explicitly rejects them).
///
/// Hardlinks are deferred to a replay pass exactly like the diff layer so
/// out-of-order tar entries still resolve.
fn extract_tar_as_base_layer(tar_bytes: &[u8], layer_path: &Path) -> io::Result<()> {
    let mut archive = tar::Archive::new(tar_bytes);
    let mut pending_links: Vec<(PathBuf, PathBuf)> = Vec::new();

    for entry in archive.entries()? {
        let mut entry = entry?;
        let rel_path = entry.path()?.into_owned();
        let entry_type = entry.header().entry_type();

        // Directory entries: materialize a real NTFS directory. No
        // `.$wcidirs$` sibling marker — baseLayerWriter does not emit one.
        if entry_type.is_dir() {
            let dest = layer_path.join(&rel_path);
            create_long_path_dir_all(&dest)?;
            continue;
        }

        // Whiteouts are illegal in base layers — hcsshim's baseLayerWriter
        // returns `errors.New("base layer cannot have tombstones")`.
        if let Some(basename) = rel_path.file_name().and_then(|s| s.to_str()) {
            if basename.starts_with(".wh.") {
                return Err(io::Error::other(format!(
                    "base layer cannot have tombstones (got whiteout entry {})",
                    rel_path.display()
                )));
            }
        }

        let dest = layer_path.join(&rel_path);
        if let Some(parent) = dest.parent() {
            create_long_path_dir_all(parent)?;
        }

        // Defer hardlinks until all primary entries have been materialized.
        if entry_type == tar::EntryType::Link {
            let link_target = entry.link_name()?.ok_or_else(|| {
                io::Error::other(format!(
                    "tar hardlink entry missing link_name: {}",
                    rel_path.display()
                ))
            })?;
            pending_links.push((rel_path.clone(), link_target.into_owned()));
            continue;
        }

        let rel_str = rel_path.to_string_lossy();
        let is_files_payload = rel_str.starts_with("Files/")
            || rel_str.starts_with("Files\\")
            || rel_str.starts_with("UtilityVM/")
            || rel_str.starts_with("UtilityVM\\");
        if is_files_payload {
            // Real NTFS file: metadata stamped via BackupWrite, no verbatim
            // framing on disk.
            backuptar::write_oci_entry_as_base_layer(&mut entry, &dest)?;
        } else {
            // `Hives/*` and any auxiliary base-layer files: raw byte copies
            // through the long-path-aware helper (hcsshim's baseLayerWriter
            // also raw-copies Hives — they are NTFS registry hive exports,
            // not BackupStream blobs).
            let mut f = layer::create_long_path_file(&dest)?;
            std::io::copy(&mut entry, &mut f)?;
        }
    }

    // Replay pending hardlinks now that every primary file is on disk.
    for (link_rel, target_rel) in pending_links {
        let link_abs = layer_path.join(&link_rel);
        let target_abs = layer_path.join(&target_rel);
        if let Some(parent) = link_abs.parent() {
            create_long_path_dir_all(parent)?;
        }
        if let Err(e) = std::fs::hard_link(&target_abs, &link_abs) {
            return Err(io::Error::other(format!(
                "hard_link({} -> {}): {e}",
                link_abs.display(),
                target_abs.display()
            )));
        }
    }

    Ok(())
}

/// Write the `<dirname>.$wcidirs$` sibling marker that hcsshim's
/// `legacyLayerWriter.Add` emits for every non-UVM directory. Without it,
/// `HcsImportLayer`'s NTFS walker returns `0x80070002`. The marker carries
/// a 4-byte LE `FileAttributes` header, optionally followed by a
/// `BACKUP_REPARSE_DATA` record when the tar entry has `MSWINDOWS.reparse`.
fn write_wcidirs_sidecar<R: std::io::Read>(
    entry: &mut tar::Entry<'_, R>,
    rel_path: &Path,
    dest: &Path,
) -> io::Result<()> {
    let rel_norm = rel_path.to_string_lossy().replace('\\', "/");
    let is_uvm =
        rel_norm == "UtilityVM" || rel_norm == "UtilityVM/" || rel_norm.starts_with("UtilityVM/");
    if is_uvm {
        return Ok(());
    }

    let mut attrs: u32 = 0x0000_0010;
    let mut reparse: Option<Vec<u8>> = None;
    if let Some(pax) = entry.pax_extensions()? {
        let engine = base64::engine::general_purpose::STANDARD;
        for ext in pax {
            let ext = ext?;
            match ext.key().unwrap_or("") {
                "MSWINDOWS.fileattr" => {
                    if let Some(parsed) = std::str::from_utf8(ext.value_bytes())
                        .ok()
                        .and_then(|s| s.trim().parse::<u32>().ok())
                    {
                        attrs = parsed;
                    }
                }
                "MSWINDOWS.reparse" => {
                    reparse = Some(engine.decode(ext.value_bytes()).map_err(|e| {
                        io::Error::other(format!("PAX MSWINDOWS.reparse base64 decode: {e}"))
                    })?);
                }
                _ => {}
            }
        }
    }

    let dirname = dest.file_name().ok_or_else(|| {
        io::Error::other(format!(
            "tar dir entry has no file_name: {}",
            rel_path.display()
        ))
    })?;
    let mut marker_name = dirname.to_os_string();
    marker_name.push(".$wcidirs$");
    let marker_path = match dest.parent() {
        Some(p) => p.join(&marker_name),
        None => PathBuf::from(&marker_name),
    };

    let mut f = layer::create_long_path_file(&marker_path)?;
    std::io::Write::write_all(&mut f, &attrs.to_le_bytes())?;
    if let Some(rp) = reparse {
        backuptar::write_stream_header(&mut f, backuptar::BACKUP_REPARSE_DATA, 0, rp.len() as u64)?;
        std::io::Write::write_all(&mut f, &rp)?;
    }
    Ok(())
}

/// `std::fs::create_dir_all` analogue that uses the `\\?\`-prefixed long-path
/// helper so we don't trip `ERROR_PATH_NOT_FOUND` (`0x80070003`) on the deep
/// `WinSxS`/`Catroot`/`UtilityVM` directories embedded in nanoserver layers.
///
/// This walks ancestors top-down and calls a `CreateDirectoryW`-equivalent
/// for each; we re-use the file helper to materialise a placeholder when the
/// directory does not yet exist, except we route to the dedicated dir API.
fn create_long_path_dir_all(dir: &Path) -> io::Result<()> {
    if dir.as_os_str().is_empty() {
        return Ok(());
    }
    // Fast path: std handles short paths fine. Try it first; on
    // `ERROR_PATH_NOT_FOUND` / `ERROR_FILENAME_EXCED_RANGE` fall back.
    match std::fs::create_dir_all(dir) {
        Ok(()) => Ok(()),
        Err(e) => {
            // Walk ancestors and create them one-by-one through the long-path
            // helper. We accumulate the components first because `Path` doesn't
            // give us a top-down ancestor iterator without an extra collect.
            let mut to_create: Vec<&Path> = dir.ancestors().collect();
            to_create.reverse();
            for component in to_create {
                if component.as_os_str().is_empty() {
                    continue;
                }
                if component.is_dir() {
                    continue;
                }
                layer::create_long_path_dir(component).map_err(|inner| {
                    io::Error::other(format!(
                        "create_dir_all fallback for {} (initial {e}): {inner}",
                        component.display()
                    ))
                })?;
            }
            Ok(())
        }
    }
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
