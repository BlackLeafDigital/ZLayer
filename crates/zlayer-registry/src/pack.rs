//! OCI layer packing — the inverse of [`crate::unpack`].
//!
//! Bundles a set of files into a single `tar+zstd` layer whose bytes, when fed
//! back through [`crate::unpack::LayerUnpacker::unpack_layers`], restore the
//! files flat into the target directory. Used by the macOS VZ base-image builder
//! (`zlayer vz build-base`) to publish a Tart-style bundle (`disk.img`,
//! `hardware-model.bin`, `aux.img`) as an OCI artifact that the VZ runtime
//! (`crates/zlayer-agent/src/runtimes/macos_vz.rs`) can pull.
//!
//! The uncompressed tar is never fully materialized: `tar::Builder` streams each
//! entry straight into the zstd encoder, so peak memory is the zstd window plus
//! the final compressed buffer (which the registry push needs in memory anyway,
//! as `oci-client` 0.15 has no chunked blob upload).

use crate::unpack::media_types;
use std::io;
use std::path::Path;

/// Default zstd compression level for packed layers. Level 3 is the zstd default
/// — a good speed/ratio tradeoff for multi-gigabyte disk images.
pub const DEFAULT_ZSTD_LEVEL: i32 = 3;

/// Pack the named files (relative to `root`, stored under their given names) into
/// a single `tar+zstd` layer.
///
/// Each entry's header is derived from the on-disk file metadata. The returned
/// tuple is `(compressed_bytes, media_type)` ready to hand to a registry push as
/// an OCI layer; `media_type` is the standard
/// `application/vnd.oci.image.layer.v1.tar+zstd`.
///
/// # Errors
/// Returns an [`io::Error`] if any file cannot be opened/read or if the
/// tar/zstd streams fail to finalize.
pub fn pack_files_tar_zstd(
    root: &Path,
    files: &[&str],
    level: i32,
) -> io::Result<(Vec<u8>, String)> {
    let encoder = zstd::stream::write::Encoder::new(Vec::new(), level)?;
    let mut builder = tar::Builder::new(encoder);
    for name in files {
        let path = root.join(name);
        let mut f = std::fs::File::open(&path).map_err(|e| {
            io::Error::new(
                e.kind(),
                format!("open {} for packing: {e}", path.display()),
            )
        })?;
        builder
            .append_file(name, &mut f)
            .map_err(|e| io::Error::other(format!("append {} to layer: {e}", path.display())))?;
    }
    // `into_inner` writes the tar trailer and hands back the zstd encoder.
    let encoder = builder
        .into_inner()
        .map_err(|e| io::Error::other(format!("finalize tar: {e}")))?;
    let compressed = encoder
        .finish()
        .map_err(|e| io::Error::other(format!("finalize zstd: {e}")))?;
    Ok((compressed, media_types::TAR_ZSTD.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::unpack::LayerUnpacker;

    #[tokio::test]
    async fn pack_then_unpack_roundtrips_files_flat() {
        let src = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("disk.img"), b"DISK-CONTENT").unwrap();
        std::fs::write(src.path().join("hardware-model.bin"), b"HWMODEL").unwrap();
        std::fs::write(src.path().join("aux.img"), b"AUXDATA").unwrap();

        let (bytes, media_type) = pack_files_tar_zstd(
            src.path(),
            &["disk.img", "hardware-model.bin", "aux.img"],
            DEFAULT_ZSTD_LEVEL,
        )
        .unwrap();
        assert_eq!(media_type, media_types::TAR_ZSTD);
        assert!(!bytes.is_empty());

        let out = tempfile::tempdir().unwrap();
        let mut unpacker = LayerUnpacker::new(out.path().to_path_buf());
        unpacker
            .unpack_layers(&[(bytes, media_type)])
            .await
            .unwrap();

        assert_eq!(
            std::fs::read(out.path().join("disk.img")).unwrap(),
            b"DISK-CONTENT"
        );
        assert_eq!(
            std::fs::read(out.path().join("hardware-model.bin")).unwrap(),
            b"HWMODEL"
        );
        assert_eq!(
            std::fs::read(out.path().join("aux.img")).unwrap(),
            b"AUXDATA"
        );
    }

    #[test]
    fn missing_file_is_an_error() {
        let src = tempfile::tempdir().unwrap();
        let err = pack_files_tar_zstd(src.path(), &["nope.img"], DEFAULT_ZSTD_LEVEL).unwrap_err();
        assert!(err.to_string().contains("nope.img"));
    }
}
