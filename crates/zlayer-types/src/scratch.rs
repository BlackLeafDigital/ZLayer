//! RAII scratch directory and file types anchored at the `ZLayer` data dir's
//! `tmp/` subdirectory. Constructed via [`zlayer_paths::ZLayerDirs::scratch_dir`]
//! and [`zlayer_paths::ZLayerDirs::scratch_file`]. These wrap `tempfile`'s
//! handles so call sites can name their return type without pulling in
//! `tempfile` directly.

use std::path::{Path, PathBuf};

/// RAII-cleaned scratch directory anchored under the `ZLayer` data dir's `tmp/`.
/// Drop removes the directory and its contents. Call [`Self::into_path`] to
/// keep the directory on disk past the guard's lifetime.
#[derive(Debug)]
pub struct Scratch {
    inner: tempfile::TempDir,
}

impl Scratch {
    /// Wrap an existing `tempfile::TempDir`. Prefer
    /// [`zlayer_paths::ZLayerDirs::scratch_dir`] over calling this directly.
    #[must_use]
    pub fn from_tempdir(inner: tempfile::TempDir) -> Self {
        Self { inner }
    }

    /// Path of the scratch directory on disk.
    #[must_use]
    pub fn path(&self) -> &Path {
        self.inner.path()
    }

    /// Consume the guard and keep the directory on disk.
    #[must_use]
    pub fn into_path(self) -> PathBuf {
        self.inner.keep()
    }

    /// Explicitly remove the directory now (vs. on Drop).
    ///
    /// # Errors
    ///
    /// Returns the underlying filesystem error if removal fails.
    pub fn close(self) -> std::io::Result<()> {
        self.inner.close()
    }
}

impl AsRef<Path> for Scratch {
    fn as_ref(&self) -> &Path {
        self.inner.path()
    }
}

/// RAII-cleaned scratch file anchored under the `ZLayer` data dir's `tmp/`.
/// Drop removes the file.
#[derive(Debug)]
pub struct ScratchFile {
    inner: tempfile::NamedTempFile,
}

impl ScratchFile {
    /// Wrap an existing `tempfile::NamedTempFile`. Prefer
    /// [`zlayer_paths::ZLayerDirs::scratch_file`] over calling this directly.
    #[must_use]
    pub fn from_named(inner: tempfile::NamedTempFile) -> Self {
        Self { inner }
    }

    /// Path of the scratch file on disk.
    #[must_use]
    pub fn path(&self) -> &Path {
        self.inner.path()
    }

    /// Read-only reference to the underlying file handle.
    #[must_use]
    pub fn as_file(&self) -> &std::fs::File {
        self.inner.as_file()
    }

    /// Mutable reference to the underlying file handle.
    pub fn as_file_mut(&mut self) -> &mut std::fs::File {
        self.inner.as_file_mut()
    }

    /// Open a fresh handle to the same on-disk file.
    ///
    /// # Errors
    ///
    /// Returns the underlying filesystem error if opening fails.
    pub fn reopen(&self) -> std::io::Result<std::fs::File> {
        self.inner.reopen()
    }

    /// Persist the file to a permanent location, consuming the guard.
    ///
    /// # Errors
    ///
    /// Returns the underlying `tempfile::PersistError` if the rename fails.
    pub fn persist(self, dest: impl AsRef<Path>) -> Result<std::fs::File, tempfile::PersistError> {
        self.inner.persist(dest)
    }

    /// Consume the guard and return the underlying `TempPath` (still
    /// RAII-cleaned, but without an open file handle).
    #[must_use]
    pub fn into_temp_path(self) -> tempfile::TempPath {
        self.inner.into_temp_path()
    }
}

impl AsRef<Path> for ScratchFile {
    fn as_ref(&self) -> &Path {
        self.inner.path()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scratch_path_under_parent_and_drop_removes_dir() {
        let parent = tempfile::tempdir().expect("parent tempdir");
        let inner = tempfile::Builder::new()
            .prefix("scratch-test-")
            .tempdir_in(parent.path())
            .expect("inner tempdir");
        let inner_path = inner.path().to_path_buf();
        assert!(inner_path.starts_with(parent.path()));
        let s = Scratch::from_tempdir(inner);
        assert!(s.path().is_dir());
        let kept = s.path().to_path_buf();
        drop(s);
        assert!(!kept.exists(), "Scratch Drop must remove the directory");
    }

    #[test]
    fn scratch_file_path_under_parent_and_drop_removes_file() {
        let parent = tempfile::tempdir().expect("parent tempdir");
        let nf = tempfile::Builder::new()
            .prefix("scratch-file-test-")
            .tempfile_in(parent.path())
            .expect("inner tempfile");
        let nf_path = nf.path().to_path_buf();
        assert!(nf_path.starts_with(parent.path()));
        let f = ScratchFile::from_named(nf);
        assert!(f.path().is_file());
        let kept = f.path().to_path_buf();
        drop(f);
        assert!(!kept.exists(), "ScratchFile Drop must remove the file");
    }

    #[test]
    fn scratch_into_path_keeps_dir() {
        let parent = tempfile::tempdir().expect("parent tempdir");
        let inner = tempfile::Builder::new()
            .prefix("scratch-keep-")
            .tempdir_in(parent.path())
            .expect("inner tempdir");
        let s = Scratch::from_tempdir(inner);
        let kept = s.into_path();
        assert!(kept.is_dir(), "into_path() must leave the dir on disk");
        std::fs::remove_dir_all(&kept).ok();
    }
}
