//! Layer snapshot creation and extraction
//!
//! Handles tarball creation from OverlayFS upper layers with compression.

use crate::error::{LayerStorageError, Result};
use crate::types::LayerSnapshot;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::Path;
use tar::Builder;
use tracing::{debug, info, instrument};

/// Create a compressed tarball snapshot from a directory
///
/// Returns the snapshot metadata and path to the compressed tarball.
#[instrument(skip(source_dir, output_path), fields(source = %source_dir.as_ref().display()))]
pub fn create_snapshot(
    source_dir: impl AsRef<Path>,
    output_path: impl AsRef<Path>,
    compression_level: i32,
) -> Result<LayerSnapshot> {
    let source_dir = source_dir.as_ref();
    let output_path = output_path.as_ref();

    info!("Creating layer snapshot from {}", source_dir.display());

    // Create temporary uncompressed tarball first to calculate digest
    let tar_temp_path = output_path.with_extension("tar.tmp");

    // Build the tar archive
    let tar_file = File::create(&tar_temp_path)?;
    let mut tar_builder = Builder::new(BufWriter::new(tar_file));

    let mut file_count = 0u64;
    tar_builder.append_dir_all(".", source_dir)?;

    // Finish writing the tar
    tar_builder.into_inner()?.flush()?;

    // Calculate SHA256 of uncompressed tar and count files
    let mut hasher = Sha256::new();
    let tar_file = File::open(&tar_temp_path)?;
    let uncompressed_size = tar_file.metadata()?.len();
    let mut reader = BufReader::new(tar_file);

    // Count files while hashing
    let mut buffer = [0u8; 8192];
    loop {
        let bytes_read = reader.read(&mut buffer)?;
        if bytes_read == 0 {
            break;
        }
        hasher.update(&buffer[..bytes_read]);
    }

    // Count entries in tar
    let tar_file = File::open(&tar_temp_path)?;
    let mut archive = tar::Archive::new(tar_file);
    for entry in archive.entries()? {
        let _ = entry?;
        file_count += 1;
    }

    let digest = hex::encode(hasher.finalize());
    debug!("Layer digest: {}", digest);

    // Compress with zstd
    let tar_file = File::open(&tar_temp_path)?;
    let compressed_file = File::create(output_path)?;
    let mut encoder =
        zstd::stream::Encoder::new(BufWriter::new(compressed_file), compression_level)?;

    let mut reader = BufReader::new(tar_file);
    std::io::copy(&mut reader, &mut encoder)?;
    encoder.finish()?.flush()?;

    // Get compressed size
    let compressed_size = std::fs::metadata(output_path)?.len();

    // Clean up temp file
    std::fs::remove_file(&tar_temp_path)?;

    let snapshot = LayerSnapshot {
        digest,
        size_bytes: uncompressed_size,
        compressed_size_bytes: compressed_size,
        created_at: chrono::Utc::now(),
        file_count,
    };

    info!(
        "Created snapshot: {} bytes -> {} bytes ({:.1}% compression), {} files",
        uncompressed_size,
        compressed_size,
        (1.0 - (compressed_size as f64 / uncompressed_size as f64)) * 100.0,
        file_count
    );

    Ok(snapshot)
}

/// Extract a compressed tarball snapshot to a directory
#[instrument(skip(tarball_path, target_dir), fields(tarball = %tarball_path.as_ref().display()))]
pub fn extract_snapshot(
    tarball_path: impl AsRef<Path>,
    target_dir: impl AsRef<Path>,
    expected_digest: Option<&str>,
) -> Result<()> {
    let tarball_path = tarball_path.as_ref();
    let target_dir = target_dir.as_ref();

    info!("Extracting layer snapshot to {}", target_dir.display());

    // Decompress
    let compressed_file = File::open(tarball_path)?;
    let decoder = zstd::stream::Decoder::new(BufReader::new(compressed_file))?;

    // If we need to verify digest, decompress to temp file first
    if let Some(expected) = expected_digest {
        let temp_tar = tarball_path.with_extension("tar.verify");
        {
            let mut temp_file = BufWriter::new(File::create(&temp_tar)?);
            let mut decoder =
                zstd::stream::Decoder::new(BufReader::new(File::open(tarball_path)?))?;
            std::io::copy(&mut decoder, &mut temp_file)?;
            temp_file.flush()?;
        }

        // Calculate digest
        let mut hasher = Sha256::new();
        let mut file = BufReader::new(File::open(&temp_tar)?);
        let mut buffer = [0u8; 8192];
        loop {
            let bytes_read = file.read(&mut buffer)?;
            if bytes_read == 0 {
                break;
            }
            hasher.update(&buffer[..bytes_read]);
        }

        let actual_digest = hex::encode(hasher.finalize());
        if actual_digest != expected {
            std::fs::remove_file(&temp_tar)?;
            return Err(LayerStorageError::ChecksumMismatch {
                expected: expected.to_string(),
                actual: actual_digest,
            });
        }

        // Extract from verified temp file
        let file = File::open(&temp_tar)?;
        let mut archive = tar::Archive::new(file);
        archive.unpack(target_dir)?;

        std::fs::remove_file(&temp_tar)?;
    } else {
        // Extract directly without verification
        let mut archive = tar::Archive::new(decoder);
        archive.unpack(target_dir)?;
    }

    info!("Extraction complete");
    Ok(())
}

/// Calculate the SHA256 digest of a directory's contents (for change detection)
#[instrument(skip(dir), fields(dir = %dir.as_ref().display()))]
pub fn calculate_directory_digest(dir: impl AsRef<Path>) -> Result<String> {
    let dir = dir.as_ref();
    let mut hasher = Sha256::new();

    // Walk directory and hash file contents and metadata
    fn hash_dir(hasher: &mut Sha256, dir: &Path, prefix: &Path) -> Result<()> {
        let mut entries: Vec<_> = std::fs::read_dir(dir)?.filter_map(|e| e.ok()).collect();

        // Sort for deterministic ordering
        entries.sort_by_key(|e| e.file_name());

        for entry in entries {
            let path = entry.path();
            let relative = path.strip_prefix(prefix).unwrap_or(&path);

            // Hash the relative path
            hasher.update(relative.to_string_lossy().as_bytes());

            let metadata = entry.metadata()?;
            if metadata.is_file() {
                // Hash file size and contents
                hasher.update(metadata.len().to_le_bytes());

                let mut file = BufReader::new(File::open(&path)?);
                let mut buffer = [0u8; 8192];
                loop {
                    let bytes_read = file.read(&mut buffer)?;
                    if bytes_read == 0 {
                        break;
                    }
                    hasher.update(&buffer[..bytes_read]);
                }
            } else if metadata.is_dir() {
                hash_dir(hasher, &path, prefix)?;
            }
            // Skip symlinks and other special files for now
        }

        Ok(())
    }

    hash_dir(&mut hasher, dir, dir)?;
    Ok(hex::encode(hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_snapshot_roundtrip() {
        let source = TempDir::new().unwrap();
        let staging = TempDir::new().unwrap();
        let target = TempDir::new().unwrap();

        // Create some test files
        std::fs::write(source.path().join("test.txt"), "hello world").unwrap();
        std::fs::create_dir(source.path().join("subdir")).unwrap();
        std::fs::write(source.path().join("subdir/nested.txt"), "nested content").unwrap();

        // Create snapshot
        let tarball_path = staging.path().join("layer.tar.zst");
        let snapshot = create_snapshot(source.path(), &tarball_path, 3).unwrap();

        assert!(!snapshot.digest.is_empty());
        assert!(snapshot.size_bytes > 0);
        assert!(snapshot.compressed_size_bytes > 0);
        assert!(snapshot.file_count >= 2);

        // Extract and verify
        extract_snapshot(&tarball_path, target.path(), Some(&snapshot.digest)).unwrap();

        assert_eq!(
            std::fs::read_to_string(target.path().join("test.txt")).unwrap(),
            "hello world"
        );
        assert_eq!(
            std::fs::read_to_string(target.path().join("subdir/nested.txt")).unwrap(),
            "nested content"
        );
    }

    #[test]
    fn test_directory_digest() {
        let dir = TempDir::new().unwrap();

        std::fs::write(dir.path().join("file1.txt"), "content1").unwrap();
        std::fs::write(dir.path().join("file2.txt"), "content2").unwrap();

        let digest1 = calculate_directory_digest(dir.path()).unwrap();

        // Modify a file
        std::fs::write(dir.path().join("file1.txt"), "modified").unwrap();

        let digest2 = calculate_directory_digest(dir.path()).unwrap();

        // Digests should differ
        assert_ne!(digest1, digest2);
    }
}
