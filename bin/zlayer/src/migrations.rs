//! On-disk layout migrations for the zlayer data directory.
//!
//! These helpers are idempotent: every step short-circuits when the
//! filesystem is already in the target shape, so it is safe to call
//! `migrate_data_dir` on every install, daemon boot, and on demand via
//! `zlayer daemon migrate`.
//!
//! ## 0.11.20: `secrets` file -> `secrets/` directory
//!
//! Pre-0.11.20 the secrets store lived in a single `SQLite` file at
//! `{data_dir}/secrets`. 0.11.20 promoted that path to a directory which
//! holds `secrets.sqlite`, `node_secrets.key`, and other per-node material.
//! Without migration the upgraded daemon hits `Not a directory (os error 20)`
//! when `key_manager.rs` calls `fs::create_dir_all({data_dir}/secrets)` on a
//! path that is still a regular file.

use std::fs;
use std::path::Path;

use anyhow::{bail, Context, Result};

/// `SQLite` database magic header (first 16 bytes of every `SQLite` 3 file).
const SQLITE_MAGIC: &[u8] = b"SQLite format 3\0";

/// Summary of migration steps that actually executed.
///
/// An empty report (`changed() == false`) means everything was already in
/// the target shape and the caller can stay quiet.
#[derive(Default, Debug)]
pub struct MigrationReport {
    pub steps: Vec<String>,
}

impl MigrationReport {
    /// Record a human-readable description of a migration step that ran.
    pub fn record(&mut self, msg: impl Into<String>) {
        self.steps.push(msg.into());
    }

    /// Whether any step ran (i.e. the data dir was actually changed).
    #[must_use]
    pub fn changed(&self) -> bool {
        !self.steps.is_empty()
    }
}

/// Run all known data-directory migrations against `data_dir`.
///
/// Returns an empty report when `data_dir` does not exist yet (fresh
/// install) or when every migration is already a no-op.
///
/// # Errors
/// Returns an error if filesystem metadata cannot be read, if the legacy
/// `secrets` path is an unexpected type (symlink, socket, FIFO), if the
/// staging path already exists, or if any of the `rename`/`create_dir`
/// operations fail.
pub fn migrate_data_dir(data_dir: &Path) -> Result<MigrationReport> {
    let mut report = MigrationReport::default();

    if !data_dir.exists() {
        // Fresh install: nothing to migrate. The daemon's own bootstrap
        // will create the data dir with the correct layout.
        return Ok(report);
    }

    migrate_legacy_secrets_layout(data_dir, &mut report)?;

    Ok(report)
}

/// Convert pre-0.11.20 `{data_dir}/secrets` (a `SQLite` file) into the
/// 0.11.20+ `{data_dir}/secrets/` directory with `secrets.sqlite` inside.
fn migrate_legacy_secrets_layout(data_dir: &Path, report: &mut MigrationReport) -> Result<()> {
    let secrets_path = data_dir.join("secrets");

    // Use symlink_metadata so we don't follow symlinks: a symlink at this
    // location is suspicious and we refuse to touch it.
    let meta = match fs::symlink_metadata(&secrets_path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // No legacy file and no current directory: the daemon will
            // create the directory itself on first run.
            return Ok(());
        }
        Err(e) => {
            return Err(e).with_context(|| format!("failed to stat {}", secrets_path.display()));
        }
    };

    let file_type = meta.file_type();

    if file_type.is_dir() {
        // Already migrated (or freshly created by a clean install).
        return Ok(());
    }

    if !file_type.is_file() {
        bail!(
            "refusing to migrate {}: expected a regular file or directory, \
             found {:?} (symlinks, sockets, and FIFOs are not supported). \
             Move this path aside manually if you really intend to upgrade.",
            secrets_path.display(),
            file_type,
        );
    }

    // Regular file: verify it actually looks like a SQLite database
    // before we touch it. We do NOT want to clobber an unrelated file
    // a user (or another tool) might have stashed at this path.
    let mut header = [0u8; 16];
    {
        use std::io::Read;
        let mut f = fs::File::open(&secrets_path)
            .with_context(|| format!("failed to open {}", secrets_path.display()))?;
        // `read` may return short reads; use `read_exact`-style loop but
        // tolerate files shorter than 16 bytes by reporting a clear error.
        let mut filled = 0;
        while filled < header.len() {
            match f.read(&mut header[filled..]) {
                Ok(0) => break,
                Ok(n) => filled += n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => (),
                Err(e) => {
                    return Err(e).with_context(|| {
                        format!("failed to read header of {}", secrets_path.display())
                    });
                }
            }
        }
        if filled < SQLITE_MAGIC.len() || !header.starts_with(SQLITE_MAGIC) {
            bail!(
                "refusing to migrate {}: file does not start with the SQLite database \
                 magic header. If this is an intentional override, move the file aside \
                 manually before upgrading.",
                secrets_path.display(),
            );
        }
    }

    // Stash the legacy DB under a dotfile in the same directory so the
    // rename into the new dir is also atomic (same filesystem).
    let staged_path = data_dir.join(".secrets.legacy.sqlite");

    if staged_path.exists() {
        bail!(
            "refusing to migrate: staging path {} already exists; \
             remove it manually if it is stale",
            staged_path.display(),
        );
    }

    fs::rename(&secrets_path, &staged_path).with_context(|| {
        format!(
            "failed to rename {} -> {}",
            secrets_path.display(),
            staged_path.display(),
        )
    })?;

    // Parent (`data_dir`) already exists (we checked at the top of
    // `migrate_data_dir`), and `secrets_path` is now vacant, so plain
    // `create_dir` is correct and surfaces collisions instead of hiding
    // them the way `create_dir_all` would.
    fs::create_dir(&secrets_path).with_context(|| {
        format!(
            "failed to create new secrets directory {}",
            secrets_path.display(),
        )
    })?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::Permissions::from_mode(0o750);
        fs::set_permissions(&secrets_path, perms).with_context(|| {
            format!(
                "failed to set permissions 0o750 on {}",
                secrets_path.display(),
            )
        })?;
    }

    #[cfg(windows)]
    {
        // Windows uses ACL-based permissions; the SCM service account
        // already inherits access from the parent data directory, so we
        // intentionally leave the new directory's ACLs as-is.
    }

    let final_db_path = secrets_path.join("secrets.sqlite");
    fs::rename(&staged_path, &final_db_path).with_context(|| {
        format!(
            "failed to rename {} -> {}",
            staged_path.display(),
            final_db_path.display(),
        )
    })?;

    report.record(format!(
        "migrated legacy secrets DB {} -> {}",
        secrets_path.display(),
        final_db_path.display(),
    ));

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;
    use tempfile::TempDir;

    /// Helper: build a 32-byte buffer that starts with the `SQLite` magic.
    fn fake_sqlite_bytes() -> Vec<u8> {
        let mut v = Vec::with_capacity(32);
        v.extend_from_slice(SQLITE_MAGIC);
        v.extend_from_slice(&[0u8; 16]);
        v
    }

    #[test]
    fn fresh_data_dir_is_noop() {
        let tmp = TempDir::new().unwrap();
        // Point at a child that does NOT exist.
        let data_dir = tmp.path().join("does_not_exist");
        assert!(!data_dir.exists());

        let report = migrate_data_dir(&data_dir).expect("migration should succeed");
        assert!(!report.changed(), "report should be empty: {report:?}");
        assert!(
            !data_dir.exists(),
            "migration must not create the data dir on a fresh install"
        );
    }

    #[test]
    fn already_migrated_dir_is_noop() {
        let tmp = TempDir::new().unwrap();
        let data_dir = tmp.path();
        let secrets_dir = data_dir.join("secrets");
        fs::create_dir_all(&secrets_dir).unwrap();
        let db_path = secrets_dir.join("secrets.sqlite");
        fs::write(&db_path, fake_sqlite_bytes()).unwrap();

        let report = migrate_data_dir(data_dir).expect("migration should succeed");
        assert!(!report.changed(), "report should be empty: {report:?}");
        assert!(
            secrets_dir.is_dir(),
            "secrets dir should still be a directory"
        );
        assert!(db_path.is_file(), "secrets.sqlite should still exist");
    }

    #[test]
    fn legacy_sqlite_file_migrated() {
        let tmp = TempDir::new().unwrap();
        let data_dir = tmp.path();
        let legacy = data_dir.join("secrets");
        let original = fake_sqlite_bytes();
        {
            let mut f = fs::File::create(&legacy).unwrap();
            f.write_all(&original).unwrap();
            f.sync_all().unwrap();
        }
        assert!(legacy.is_file());

        let report = migrate_data_dir(data_dir).expect("migration should succeed");
        assert!(report.changed(), "report should record the migration");
        assert_eq!(report.steps.len(), 1);

        assert!(
            legacy.is_dir(),
            "{} should now be a directory",
            legacy.display()
        );
        let migrated_db = legacy.join("secrets.sqlite");
        assert!(migrated_db.is_file(), "migrated DB should exist");

        let migrated_bytes = fs::read(&migrated_db).unwrap();
        assert_eq!(migrated_bytes, original, "DB contents must be preserved");

        let staged = data_dir.join(".secrets.legacy.sqlite");
        assert!(
            !staged.exists(),
            "staging path must be cleaned up after migration"
        );
    }

    #[test]
    fn non_sqlite_file_refused() {
        let tmp = TempDir::new().unwrap();
        let data_dir = tmp.path();
        let legacy = data_dir.join("secrets");
        let garbage = b"this is not a sqlite database, just some random bytes!!";
        fs::write(&legacy, garbage).unwrap();

        let err = migrate_data_dir(data_dir).expect_err("migration should refuse");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("SQLite database"),
            "error should mention 'SQLite database', got: {msg}"
        );

        // Original file must be untouched.
        assert!(
            legacy.is_file(),
            "legacy path should still be a regular file"
        );
        let after = fs::read(&legacy).unwrap();
        assert_eq!(after, garbage, "file contents must not be modified");
        assert!(
            !data_dir.join(".secrets.legacy.sqlite").exists(),
            "no staging file should have been created"
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlink_refused() {
        let tmp = TempDir::new().unwrap();
        let data_dir = tmp.path();
        let target = data_dir.join("real_target");
        fs::write(&target, fake_sqlite_bytes()).unwrap();

        let legacy = data_dir.join("secrets");
        std::os::unix::fs::symlink(&target, &legacy).unwrap();

        let err = migrate_data_dir(data_dir).expect_err("migration should refuse");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("refusing to migrate"),
            "error should explain the refusal, got: {msg}"
        );

        // Symlink must be unchanged.
        let meta = fs::symlink_metadata(&legacy).unwrap();
        assert!(
            meta.file_type().is_symlink(),
            "symlink at {} must remain a symlink",
            legacy.display()
        );
        let resolved = fs::read_link(&legacy).unwrap();
        assert_eq!(resolved, target, "symlink target must be unchanged");
    }
}
