//! OCI layer unpacking and extraction
//!
//! This module handles extracting OCI image layers to a rootfs directory,
//! supporting gzip, zstd, and uncompressed tar formats.

use crate::error::{CacheError, RegistryError, Result};
use flate2::read::GzDecoder;
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use tar::Archive;

/// OCI layer media types
pub mod media_types {
    /// Uncompressed tar layer
    pub const TAR: &str = "application/vnd.oci.image.layer.v1.tar";
    /// Gzip compressed tar layer
    pub const TAR_GZIP: &str = "application/vnd.oci.image.layer.v1.tar+gzip";
    /// Zstd compressed tar layer
    pub const TAR_ZSTD: &str = "application/vnd.oci.image.layer.v1.tar+zstd";
    /// Docker gzip compressed layer (legacy)
    pub const DOCKER_TAR_GZIP: &str = "application/vnd.docker.image.rootfs.diff.tar.gzip";
}

/// Whiteout file prefix for deletions
const WHITEOUT_PREFIX: &str = ".wh.";
/// Opaque whiteout marker (delete entire directory contents)
const OPAQUE_WHITEOUT: &str = ".wh..wh..opq";

/// Compression type detection
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressionType {
    /// No compression (plain tar)
    None,
    /// Gzip compression
    Gzip,
    /// Zstd compression
    Zstd,
}

impl CompressionType {
    /// Detect compression type from media type string
    pub fn from_media_type(media_type: &str) -> Self {
        match media_type {
            media_types::TAR_GZIP | media_types::DOCKER_TAR_GZIP => Self::Gzip,
            media_types::TAR_ZSTD => Self::Zstd,
            media_types::TAR => Self::None,
            _ => Self::from_magic_bytes(&[]), // Unknown, will fallback to magic bytes
        }
    }

    /// Detect compression type from magic bytes
    pub fn from_magic_bytes(data: &[u8]) -> Self {
        if data.len() < 4 {
            return Self::None;
        }

        // Gzip magic: 0x1f 0x8b
        if data[0] == 0x1f && data[1] == 0x8b {
            return Self::Gzip;
        }

        // Zstd magic: 0x28 0xb5 0x2f 0xfd
        if data.len() >= 4 && data[0] == 0x28 && data[1] == 0xb5 && data[2] == 0x2f && data[3] == 0xfd
        {
            return Self::Zstd;
        }

        Self::None
    }

    /// Detect compression, preferring media type but falling back to magic bytes
    pub fn detect(media_type: &str, data: &[u8]) -> Self {
        let from_media = Self::from_media_type(media_type);
        if from_media != Self::None {
            return from_media;
        }
        Self::from_magic_bytes(data)
    }
}

/// Layer unpacker for extracting OCI layers to a rootfs
pub struct LayerUnpacker {
    /// Root filesystem directory
    rootfs_dir: PathBuf,
    /// Track deleted paths from whiteouts
    deleted_paths: HashSet<PathBuf>,
}

impl LayerUnpacker {
    /// Create a new layer unpacker targeting the specified rootfs directory
    pub fn new(rootfs_dir: PathBuf) -> Self {
        Self {
            rootfs_dir,
            deleted_paths: HashSet::new(),
        }
    }

    /// Get the rootfs directory
    pub fn rootfs_dir(&self) -> &Path {
        &self.rootfs_dir
    }

    /// Unpack a single layer to the rootfs
    ///
    /// # Arguments
    /// * `layer_data` - The compressed or uncompressed layer data
    /// * `media_type` - The OCI media type of the layer
    ///
    /// # Errors
    /// Returns an error if decompression or extraction fails
    pub async fn unpack_layer(&mut self, layer_data: &[u8], media_type: &str) -> Result<()> {
        // Ensure rootfs directory exists
        fs::create_dir_all(&self.rootfs_dir).map_err(|e| {
            RegistryError::Cache(CacheError::Io(std::io::Error::new(
                e.kind(),
                format!("failed to create rootfs directory: {}", e),
            )))
        })?;

        // Detect compression type
        let compression = CompressionType::detect(media_type, layer_data);

        // Decompress and extract
        match compression {
            CompressionType::Gzip => self.unpack_gzip(layer_data).await,
            CompressionType::Zstd => self.unpack_zstd(layer_data).await,
            CompressionType::None => self.unpack_tar(layer_data).await,
        }
    }

    /// Unpack multiple layers in order (base layer first)
    ///
    /// # Arguments
    /// * `layers` - Slice of (layer_data, media_type) tuples in order
    ///
    /// # Errors
    /// Returns an error if any layer extraction fails
    pub async fn unpack_layers(&mut self, layers: &[(Vec<u8>, String)]) -> Result<()> {
        for (i, (data, media_type)) in layers.iter().enumerate() {
            tracing::debug!(
                layer = i,
                media_type = %media_type,
                size = data.len(),
                "unpacking layer"
            );
            self.unpack_layer(data, media_type).await?;
        }
        Ok(())
    }

    /// Unpack a gzip-compressed tar archive
    async fn unpack_gzip(&mut self, data: &[u8]) -> Result<()> {
        let cursor = Cursor::new(data);
        let decoder = GzDecoder::new(cursor);
        let mut archive = Archive::new(decoder);
        self.extract_archive(&mut archive)
    }

    /// Unpack a zstd-compressed tar archive
    async fn unpack_zstd(&mut self, data: &[u8]) -> Result<()> {
        let cursor = Cursor::new(data);
        let decoder = zstd::stream::Decoder::new(cursor).map_err(|e| {
            RegistryError::Cache(CacheError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("zstd decompression failed: {}", e),
            )))
        })?;
        let mut archive = Archive::new(decoder);
        self.extract_archive(&mut archive)
    }

    /// Unpack an uncompressed tar archive
    async fn unpack_tar(&mut self, data: &[u8]) -> Result<()> {
        let cursor = Cursor::new(data);
        let mut archive = Archive::new(cursor);
        self.extract_archive(&mut archive)
    }

    /// Extract files from a tar archive, handling whiteouts
    fn extract_archive<R: Read>(&mut self, archive: &mut Archive<R>) -> Result<()> {
        let entries = archive.entries().map_err(|e| {
            RegistryError::Cache(CacheError::Io(std::io::Error::new(
                e.kind(),
                format!("failed to read tar entries: {}", e),
            )))
        })?;

        for entry in entries {
            let mut entry = entry.map_err(|e| {
                RegistryError::Cache(CacheError::Io(std::io::Error::new(
                    e.kind(),
                    format!("failed to read tar entry: {}", e),
                )))
            })?;

            let path = entry.path().map_err(|e| {
                RegistryError::Cache(CacheError::Io(std::io::Error::new(
                    e.kind(),
                    format!("failed to get entry path: {}", e),
                )))
            })?;
            let path = path.to_path_buf();

            // Security: Validate path doesn't escape rootfs
            if !self.is_safe_path(&path) {
                tracing::warn!(path = %path.display(), "skipping potentially unsafe path");
                continue;
            }

            let file_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();

            // Handle opaque whiteout (delete all contents in directory)
            if file_name == OPAQUE_WHITEOUT {
                if let Some(parent) = path.parent() {
                    self.apply_opaque_whiteout(parent)?;
                }
                continue;
            }

            // Handle regular whiteout (delete specific file/dir)
            if let Some(target_name) = file_name.strip_prefix(WHITEOUT_PREFIX) {
                if let Some(parent) = path.parent() {
                    let target_path = parent.join(target_name);
                    self.apply_whiteout(&target_path)?;
                }
                continue;
            }

            // Skip if this path was deleted by a whiteout
            let full_path = self.rootfs_dir.join(&path);
            if self.is_deleted(&path) {
                continue;
            }

            // Extract the entry
            self.extract_entry(&mut entry, &full_path)?;
        }

        Ok(())
    }

    /// Validate that a path doesn't escape the rootfs directory
    fn is_safe_path(&self, path: &Path) -> bool {
        // Reject absolute paths
        if path.is_absolute() {
            return false;
        }

        // Reject paths with .. components
        for component in path.components() {
            if let std::path::Component::ParentDir = component {
                return false;
            }
        }

        true
    }

    /// Check if a path has been marked as deleted
    fn is_deleted(&self, path: &Path) -> bool {
        // Check if this exact path or any parent is deleted
        let mut current = path.to_path_buf();
        loop {
            if self.deleted_paths.contains(&current) {
                return true;
            }
            if !current.pop() {
                break;
            }
        }
        false
    }

    /// Apply a regular whiteout (delete a specific file or directory)
    fn apply_whiteout(&mut self, path: &Path) -> Result<()> {
        let full_path = self.rootfs_dir.join(path);
        self.deleted_paths.insert(path.to_path_buf());

        if full_path.exists() {
            if full_path.is_dir() {
                fs::remove_dir_all(&full_path).map_err(|e| {
                    RegistryError::Cache(CacheError::Io(std::io::Error::new(
                        e.kind(),
                        format!("failed to remove directory {}: {}", full_path.display(), e),
                    )))
                })?;
            } else {
                fs::remove_file(&full_path).map_err(|e| {
                    RegistryError::Cache(CacheError::Io(std::io::Error::new(
                        e.kind(),
                        format!("failed to remove file {}: {}", full_path.display(), e),
                    )))
                })?;
            }
        }

        Ok(())
    }

    /// Apply an opaque whiteout (delete all contents in a directory)
    fn apply_opaque_whiteout(&mut self, dir_path: &Path) -> Result<()> {
        let full_path = self.rootfs_dir.join(dir_path);

        if full_path.is_dir() {
            // Remove all contents but keep the directory
            for entry in fs::read_dir(&full_path).map_err(|e| {
                RegistryError::Cache(CacheError::Io(std::io::Error::new(
                    e.kind(),
                    format!("failed to read directory {}: {}", full_path.display(), e),
                )))
            })? {
                let entry = entry.map_err(|e| {
                    RegistryError::Cache(CacheError::Io(std::io::Error::new(
                        e.kind(),
                        format!("failed to read directory entry: {}", e),
                    )))
                })?;
                let entry_path = entry.path();
                let relative = entry_path
                    .strip_prefix(&self.rootfs_dir)
                    .unwrap_or(&entry_path);
                self.deleted_paths.insert(relative.to_path_buf());

                if entry_path.is_dir() {
                    fs::remove_dir_all(&entry_path).map_err(|e| {
                        RegistryError::Cache(CacheError::Io(std::io::Error::new(
                            e.kind(),
                            format!("failed to remove directory {}: {}", entry_path.display(), e),
                        )))
                    })?;
                } else {
                    fs::remove_file(&entry_path).map_err(|e| {
                        RegistryError::Cache(CacheError::Io(std::io::Error::new(
                            e.kind(),
                            format!("failed to remove file {}: {}", entry_path.display(), e),
                        )))
                    })?;
                }
            }
        }

        Ok(())
    }

    /// Extract a single tar entry to the filesystem
    fn extract_entry<R: Read>(
        &self,
        entry: &mut tar::Entry<'_, R>,
        full_path: &Path,
    ) -> Result<()> {
        let entry_type = entry.header().entry_type();

        // Ensure parent directory exists
        if let Some(parent) = full_path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                RegistryError::Cache(CacheError::Io(std::io::Error::new(
                    e.kind(),
                    format!("failed to create parent directory: {}", e),
                )))
            })?;
        }

        match entry_type {
            tar::EntryType::Regular | tar::EntryType::Continuous => {
                self.extract_file(entry, full_path)?;
            }
            tar::EntryType::Directory => {
                fs::create_dir_all(full_path).map_err(|e| {
                    RegistryError::Cache(CacheError::Io(std::io::Error::new(
                        e.kind(),
                        format!("failed to create directory {}: {}", full_path.display(), e),
                    )))
                })?;
                self.set_permissions(entry, full_path)?;
            }
            tar::EntryType::Symlink => {
                if let Ok(Some(target)) = entry.link_name() {
                    // Remove existing file/link if present
                    let _ = fs::remove_file(full_path);
                    #[cfg(unix)]
                    {
                        std::os::unix::fs::symlink(&target, full_path).map_err(|e| {
                            RegistryError::Cache(CacheError::Io(std::io::Error::new(
                                e.kind(),
                                format!(
                                    "failed to create symlink {} -> {}: {}",
                                    full_path.display(),
                                    target.display(),
                                    e
                                ),
                            )))
                        })?;
                    }
                    #[cfg(not(unix))]
                    {
                        // On non-Unix, try to copy the file or create a placeholder
                        tracing::warn!(
                            path = %full_path.display(),
                            target = %target.display(),
                            "symlinks not fully supported on this platform"
                        );
                    }
                }
            }
            tar::EntryType::Link => {
                if let Ok(Some(target)) = entry.link_name() {
                    let target_full = self.rootfs_dir.join(&*target);
                    // Remove existing file if present
                    let _ = fs::remove_file(full_path);
                    fs::hard_link(&target_full, full_path).map_err(|e| {
                        RegistryError::Cache(CacheError::Io(std::io::Error::new(
                            e.kind(),
                            format!(
                                "failed to create hard link {} -> {}: {}",
                                full_path.display(),
                                target_full.display(),
                                e
                            ),
                        )))
                    })?;
                }
            }
            tar::EntryType::Char | tar::EntryType::Block | tar::EntryType::Fifo => {
                // Device nodes and FIFOs require special handling and usually root
                tracing::debug!(
                    path = %full_path.display(),
                    entry_type = ?entry_type,
                    "skipping special file (requires privileges)"
                );
            }
            _ => {
                tracing::trace!(
                    path = %full_path.display(),
                    entry_type = ?entry_type,
                    "skipping unknown entry type"
                );
            }
        }

        Ok(())
    }

    /// Extract a regular file
    fn extract_file<R: Read>(
        &self,
        entry: &mut tar::Entry<'_, R>,
        full_path: &Path,
    ) -> Result<()> {
        // Remove existing file if present (might be a different type)
        let _ = fs::remove_file(full_path);

        let mut file = File::create(full_path).map_err(|e| {
            RegistryError::Cache(CacheError::Io(std::io::Error::new(
                e.kind(),
                format!("failed to create file {}: {}", full_path.display(), e),
            )))
        })?;

        std::io::copy(entry, &mut file).map_err(|e| {
            RegistryError::Cache(CacheError::Io(std::io::Error::new(
                e.kind(),
                format!("failed to write file {}: {}", full_path.display(), e),
            )))
        })?;

        self.set_permissions(entry, full_path)?;

        Ok(())
    }

    /// Set file permissions from tar entry
    fn set_permissions<R: Read>(
        &self,
        entry: &tar::Entry<'_, R>,
        full_path: &Path,
    ) -> Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            if let Ok(mode) = entry.header().mode() {
                let permissions = fs::Permissions::from_mode(mode);
                fs::set_permissions(full_path, permissions).map_err(|e| {
                    RegistryError::Cache(CacheError::Io(std::io::Error::new(
                        e.kind(),
                        format!(
                            "failed to set permissions on {}: {}",
                            full_path.display(),
                            e
                        ),
                    )))
                })?;
            }
        }

        #[cfg(not(unix))]
        {
            let _ = entry;
            let _ = full_path;
        }

        Ok(())
    }

    /// Clear the deleted paths tracking (useful between image extractions)
    pub fn reset(&mut self) {
        self.deleted_paths.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    fn create_tar_gz(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());

        for (path, content) in files {
            let mut header = tar::Header::new_gnu();
            header.set_path(path).unwrap();
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append(&header, *content).unwrap();
        }

        let tar_data = builder.into_inner().unwrap();

        // Compress with gzip
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&tar_data).unwrap();
        encoder.finish().unwrap()
    }

    fn create_tar_zstd(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());

        for (path, content) in files {
            let mut header = tar::Header::new_gnu();
            header.set_path(path).unwrap();
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append(&header, *content).unwrap();
        }

        let tar_data = builder.into_inner().unwrap();

        // Compress with zstd
        zstd::encode_all(std::io::Cursor::new(tar_data), 3).unwrap()
    }

    #[tokio::test]
    async fn test_unpack_gzip_layer() {
        let temp_dir = TempDir::new().unwrap();
        let rootfs = temp_dir.path().join("rootfs");

        let layer_data = create_tar_gz(&[
            ("file.txt", b"hello world"),
            ("subdir/nested.txt", b"nested content"),
        ]);

        let mut unpacker = LayerUnpacker::new(rootfs.clone());
        unpacker
            .unpack_layer(&layer_data, media_types::TAR_GZIP)
            .await
            .unwrap();

        assert!(rootfs.join("file.txt").exists());
        assert_eq!(
            fs::read_to_string(rootfs.join("file.txt")).unwrap(),
            "hello world"
        );
        assert!(rootfs.join("subdir/nested.txt").exists());
        assert_eq!(
            fs::read_to_string(rootfs.join("subdir/nested.txt")).unwrap(),
            "nested content"
        );
    }

    #[tokio::test]
    async fn test_unpack_zstd_layer() {
        let temp_dir = TempDir::new().unwrap();
        let rootfs = temp_dir.path().join("rootfs");

        let layer_data = create_tar_zstd(&[("zstd_file.txt", b"zstd compressed")]);

        let mut unpacker = LayerUnpacker::new(rootfs.clone());
        unpacker
            .unpack_layer(&layer_data, media_types::TAR_ZSTD)
            .await
            .unwrap();

        assert!(rootfs.join("zstd_file.txt").exists());
        assert_eq!(
            fs::read_to_string(rootfs.join("zstd_file.txt")).unwrap(),
            "zstd compressed"
        );
    }

    #[tokio::test]
    async fn test_compression_detection_magic_bytes() {
        // Gzip magic bytes
        let gzip_data = vec![0x1f, 0x8b, 0x08, 0x00];
        assert_eq!(
            CompressionType::from_magic_bytes(&gzip_data),
            CompressionType::Gzip
        );

        // Zstd magic bytes
        let zstd_data = vec![0x28, 0xb5, 0x2f, 0xfd];
        assert_eq!(
            CompressionType::from_magic_bytes(&zstd_data),
            CompressionType::Zstd
        );

        // No compression
        let tar_data = vec![0x00, 0x00, 0x00, 0x00];
        assert_eq!(
            CompressionType::from_magic_bytes(&tar_data),
            CompressionType::None
        );
    }

    #[tokio::test]
    async fn test_whiteout_file_deletion() {
        let temp_dir = TempDir::new().unwrap();
        let rootfs = temp_dir.path().join("rootfs");

        // First layer: create a file
        let layer1 = create_tar_gz(&[("to_delete.txt", b"will be deleted")]);

        // Second layer: whiteout file
        let layer2 = create_tar_gz(&[(".wh.to_delete.txt", b"")]);

        let mut unpacker = LayerUnpacker::new(rootfs.clone());
        unpacker
            .unpack_layers(&[
                (layer1, media_types::TAR_GZIP.to_string()),
                (layer2, media_types::TAR_GZIP.to_string()),
            ])
            .await
            .unwrap();

        // File should be deleted
        assert!(!rootfs.join("to_delete.txt").exists());
    }

    #[tokio::test]
    async fn test_path_traversal_prevention() {
        let temp_dir = TempDir::new().unwrap();
        let rootfs = temp_dir.path().join("rootfs");

        let unpacker = LayerUnpacker::new(rootfs);

        // These paths should be rejected
        assert!(!unpacker.is_safe_path(Path::new("/etc/passwd")));
        assert!(!unpacker.is_safe_path(Path::new("../../../etc/passwd")));
        assert!(!unpacker.is_safe_path(Path::new("foo/../../../etc/passwd")));

        // These paths should be accepted
        assert!(unpacker.is_safe_path(Path::new("normal/path/file.txt")));
        assert!(unpacker.is_safe_path(Path::new("file.txt")));
    }

    #[tokio::test]
    async fn test_layer_ordering() {
        let temp_dir = TempDir::new().unwrap();
        let rootfs = temp_dir.path().join("rootfs");

        // First layer: create a file
        let layer1 = create_tar_gz(&[("file.txt", b"original")]);

        // Second layer: modify the file
        let layer2 = create_tar_gz(&[("file.txt", b"modified")]);

        let mut unpacker = LayerUnpacker::new(rootfs.clone());
        unpacker
            .unpack_layers(&[
                (layer1, media_types::TAR_GZIP.to_string()),
                (layer2, media_types::TAR_GZIP.to_string()),
            ])
            .await
            .unwrap();

        // Should have the modified content
        assert_eq!(
            fs::read_to_string(rootfs.join("file.txt")).unwrap(),
            "modified"
        );
    }

    #[test]
    fn test_media_type_detection() {
        assert_eq!(
            CompressionType::from_media_type(media_types::TAR_GZIP),
            CompressionType::Gzip
        );
        assert_eq!(
            CompressionType::from_media_type(media_types::DOCKER_TAR_GZIP),
            CompressionType::Gzip
        );
        assert_eq!(
            CompressionType::from_media_type(media_types::TAR_ZSTD),
            CompressionType::Zstd
        );
        assert_eq!(
            CompressionType::from_media_type(media_types::TAR),
            CompressionType::None
        );
    }
}
