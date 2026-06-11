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
//!
//! ## Layout version marker
//!
//! `{data_dir}/layout_version` holds the on-disk layout version this build
//! understands, as a plain ASCII integer with a trailing newline (e.g.
//! `"1\n"`). It is written after migrations succeed so that:
//!
//! * a NEWER data dir opened by an OLDER daemon is detected and refused
//!   loudly (downgrade detection) instead of silently opening fresh stores
//!   over data it cannot read, and
//! * future layout bumps have a keyed anchor to migrate from.
//!
//! Existing healthy installs that predate the marker are treated as
//! layout version 1: the marker is back-filled, never refused.

use std::fs;
use std::path::Path;

use anyhow::{bail, Context, Result};

/// `SQLite` database magic header (first 16 bytes of every `SQLite` 3 file).
const SQLITE_MAGIC: &[u8] = b"SQLite format 3\0";

/// On-disk layout version this build reads and writes.
///
/// Bump this whenever the data-directory layout changes in a way that needs a
/// migration step, and add the corresponding keyed step in
/// `run_keyed_migrations`. The marker file (`layout_version`) records the
/// version the data dir was last migrated to.
const CURRENT_LAYOUT_VERSION: u32 = 1;

/// Name of the layout-version marker file inside the data directory.
const LAYOUT_VERSION_FILE: &str = "layout_version";

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
/// After the per-step migrations succeed, the `{data_dir}/layout_version`
/// marker is written/refreshed so that a future daemon can detect a
/// downgrade (a data dir written by a newer build) and refuse loudly
/// instead of opening fresh, empty stores over data it cannot read.
///
/// # Errors
/// Returns an error if filesystem metadata cannot be read, if the legacy
/// `secrets` path is an unexpected type (symlink, socket, FIFO), if the
/// staging path already exists, if the data dir contains store formats this
/// build cannot read (see [`detect_incompatible_stores`]), if the layout
/// marker reports a version newer than this build understands (downgrade),
/// or if any of the `rename`/`create_dir`/marker-write operations fail.
pub fn migrate_data_dir(data_dir: &Path) -> Result<MigrationReport> {
    let mut report = MigrationReport::default();

    if !data_dir.exists() {
        // Fresh install: nothing to migrate. The daemon's own bootstrap
        // will create the data dir with the correct layout.
        return Ok(report);
    }

    // Refuse loudly before touching anything if the data dir holds store
    // formats this build cannot read. Doing this first means we never
    // half-migrate a directory we are going to reject anyway.
    if let Some(desc) = detect_incompatible_stores(data_dir) {
        bail!(
            "refusing to start against an incompatible data directory: {desc}. \
             This zlayer build cannot read the stores already present in {}. \
             Set ZLAYER_DATA_DIR to a fresh directory, or remove the \
             incompatible stores listed above and restart.",
            data_dir.display(),
        );
    }

    // Read the layout marker (if any) up front so we can detect a downgrade
    // before running migrations.
    let marker = read_layout_marker(data_dir)?;

    if let Some(found) = marker {
        if found > CURRENT_LAYOUT_VERSION {
            bail!(
                "refusing to start: {} reports layout version {found}, but this \
                 zlayer build only understands version {CURRENT_LAYOUT_VERSION}. \
                 The data dir was written by a newer zlayer; downgrades are not \
                 supported. Set ZLAYER_DATA_DIR to a fresh directory, or run a \
                 zlayer build new enough to read it.",
                layout_marker_path(data_dir).display(),
            );
        }
    }

    // Per-step migrations (idempotent). These run on every boot regardless of
    // the marker so that an install which predates the marker is still
    // self-healed before the marker is back-filled below.
    migrate_legacy_secrets_layout(data_dir, &mut report)?;

    // Keyed migrations advance the layout from `from_version` up to
    // `CURRENT_LAYOUT_VERSION`. When the marker is absent we treat an existing
    // installation as legacy version 1 (back-fill) and a fresh/empty dir as
    // already at the current version (just write the marker).
    let from_version = match marker {
        Some(v) => v,
        None if data_dir_has_existing_install(data_dir) => 1,
        None => CURRENT_LAYOUT_VERSION,
    };

    run_keyed_migrations(data_dir, from_version, &mut report)?;

    // Record the version we just migrated to. This is the only place that
    // advances the marker; keyed steps above must leave the dir in the
    // `CURRENT_LAYOUT_VERSION` shape before we get here.
    if marker != Some(CURRENT_LAYOUT_VERSION) {
        write_layout_marker(data_dir, CURRENT_LAYOUT_VERSION)?;
        report.record(format!(
            "wrote layout version marker {} = {CURRENT_LAYOUT_VERSION}",
            layout_marker_path(data_dir).display(),
        ));
    }

    Ok(report)
}

/// Path of the layout-version marker file inside `data_dir`.
fn layout_marker_path(data_dir: &Path) -> std::path::PathBuf {
    data_dir.join(LAYOUT_VERSION_FILE)
}

/// Read and parse the `{data_dir}/layout_version` marker.
///
/// Returns `Ok(None)` when the marker is absent (pre-marker install or fresh
/// dir). Returns an error when the marker exists but cannot be read or does
/// not contain a plain ASCII integer — a corrupt marker is treated as a hard
/// failure rather than silently re-initialised, because guessing wrong risks
/// the exact silent-data-loss this marker exists to prevent.
fn read_layout_marker(data_dir: &Path) -> Result<Option<u32>> {
    let path = layout_marker_path(data_dir);
    let raw = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(e).with_context(|| format!("failed to read {}", path.display()));
        }
    };

    let trimmed = raw.trim();
    let version: u32 = trimmed.parse().with_context(|| {
        format!(
            "{} contains {trimmed:?}, which is not a valid layout version (expected a \
             plain integer); refusing to guess. Remove the file only if you are sure the \
             data dir is otherwise intact.",
            path.display(),
        )
    })?;

    Ok(Some(version))
}

/// Atomically write the layout-version marker as a plain ASCII integer with a
/// trailing newline (e.g. `"1\n"`).
fn write_layout_marker(data_dir: &Path, version: u32) -> Result<()> {
    let path = layout_marker_path(data_dir);
    let tmp = data_dir.join(".layout_version.tmp");

    fs::write(&tmp, format!("{version}\n"))
        .with_context(|| format!("failed to write {}", tmp.display()))?;
    fs::rename(&tmp, &path)
        .with_context(|| format!("failed to rename {} -> {}", tmp.display(), path.display(),))?;

    Ok(())
}

/// Whether `data_dir` clearly contains an existing zlayer installation, as
/// opposed to an empty/fresh directory that just happens to exist.
///
/// Used only to decide how to treat a *missing* marker: an existing install
/// is back-filled as legacy layout version 1, a fresh dir is stamped with the
/// current version directly. The probes mirror the top-level artifacts the
/// daemon creates: `daemon.json`, `secrets/`, and the various `raft*` paths.
fn data_dir_has_existing_install(data_dir: &Path) -> bool {
    const MARKERS: &[&str] = &["daemon.json", "secrets", "raft", "raft-log", "raft-sm"];
    MARKERS.iter().any(|name| data_dir.join(name).exists())
}

/// Apply layout migrations keyed to the version the data dir was last
/// migrated to, advancing it up to [`CURRENT_LAYOUT_VERSION`].
///
/// There are currently no keyed steps beyond the version-1 baseline (the
/// legacy-secrets move in [`migrate_legacy_secrets_layout`] runs
/// unconditionally above, before this function). When a future layout bump
/// lands, add a `if from_version < N { … }` block here in ascending order.
#[allow(clippy::unnecessary_wraps)]
fn run_keyed_migrations(
    _data_dir: &Path,
    from_version: u32,
    _report: &mut MigrationReport,
) -> Result<()> {
    debug_assert!(
        from_version <= CURRENT_LAYOUT_VERSION,
        "downgrade must be rejected before keyed migrations run",
    );
    // No keyed steps yet. Future bumps slot in here, e.g.:
    //   if from_version < 2 { migrate_v1_to_v2(_data_dir, _report)?; }
    Ok(())
}

/// Probe `data_dir` for store formats THIS build cannot read and return a
/// human-readable description of the first incompatibility found, or `None`
/// when everything present is readable by this build.
///
/// ## Fork contract
///
/// This public build currently recognises every store format it ships with,
/// so the check list is empty and this function always returns `None`. It is
/// deliberately structured as a list of `(probe, message)` pairs so that a
/// downstream fork can append its own probe via a patch without reshaping the
/// caller. For example, the private ZQL fork — whose secrets store is a
/// directory of ZQL segments, not `secrets/secrets.sqlite` — adds a probe
/// that fires when a plain `secrets/secrets.sqlite` is present (a `SQLite`
/// store this fork cannot read), returning a message naming that exact path.
///
/// Each probe is a closure that inspects `data_dir` and returns `Some(msg)`
/// when its incompatibility is present. The first match wins; `msg` should
/// name the exact offending path(s) so the caller can surface remediation.
fn detect_incompatible_stores(data_dir: &Path) -> Option<String> {
    // Each entry: a closure returning Some(description) when the
    // incompatibility it probes for is present in `data_dir`.
    //
    // Public build: empty. Fork patches push their probe(s) onto this slice.
    type Probe<'a> = &'a dyn Fn(&Path) -> Option<String>;
    let probes: &[Probe<'_>] = &[];

    probes.iter().find_map(|probe| probe(data_dir))
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
    // `migrate_data_dir`), and `secrets_path` is now vacant after the
    // rename above. We use `create_dir_all` defensively: if a concurrent
    // process (or a partially-completed prior migration) has already
    // recreated the directory, we want the idempotent no-op instead of
    // EEXIST. `create_dir_all` still surfaces real errors (permission,
    // ENOENT on parent, etc.).
    fs::create_dir_all(&secrets_path).with_context(|| {
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
    fn already_migrated_dir_backfills_marker() {
        let tmp = TempDir::new().unwrap();
        let data_dir = tmp.path();
        let secrets_dir = data_dir.join("secrets");
        fs::create_dir_all(&secrets_dir).unwrap();
        let db_path = secrets_dir.join("secrets.sqlite");
        fs::write(&db_path, fake_sqlite_bytes()).unwrap();

        // Secrets layout is already current, but there is no marker yet: this
        // is a healthy install predating the marker, so we back-fill it.
        let report = migrate_data_dir(data_dir).expect("migration should succeed");
        assert!(report.changed(), "marker back-fill should be recorded");
        assert!(
            secrets_dir.is_dir(),
            "secrets dir should still be a directory"
        );
        assert!(db_path.is_file(), "secrets.sqlite should still exist");

        let marker = fs::read_to_string(layout_marker_path(data_dir)).unwrap();
        assert_eq!(marker, format!("{CURRENT_LAYOUT_VERSION}\n"));
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
        // Two steps: the legacy-secrets move and the marker write.
        assert_eq!(report.steps.len(), 2, "report: {report:?}");

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

        let marker = fs::read_to_string(layout_marker_path(data_dir)).unwrap();
        assert_eq!(marker, format!("{CURRENT_LAYOUT_VERSION}\n"));
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

    #[test]
    fn fresh_existing_dir_writes_marker() {
        // The data dir exists but is empty (no daemon.json, no stores): a
        // fresh bootstrap. We stamp the current version directly.
        let tmp = TempDir::new().unwrap();
        let data_dir = tmp.path();

        let report = migrate_data_dir(data_dir).expect("migration should succeed");
        assert!(report.changed(), "marker write should be recorded");

        let marker = fs::read_to_string(layout_marker_path(data_dir)).unwrap();
        assert_eq!(marker, format!("{CURRENT_LAYOUT_VERSION}\n"));
    }

    #[test]
    fn legacy_install_without_marker_migrates_and_writes_marker() {
        // An existing install (daemon.json present) with no marker is treated
        // as legacy layout version 1 and back-filled, never refused.
        let tmp = TempDir::new().unwrap();
        let data_dir = tmp.path();
        fs::write(data_dir.join("daemon.json"), b"{}").unwrap();

        let report = migrate_data_dir(data_dir).expect("migration should succeed");
        assert!(report.changed(), "marker back-fill should be recorded");

        let marker = fs::read_to_string(layout_marker_path(data_dir)).unwrap();
        assert_eq!(marker, format!("{CURRENT_LAYOUT_VERSION}\n"));

        // daemon.json must be left untouched.
        assert!(data_dir.join("daemon.json").is_file());
    }

    #[test]
    fn newer_marker_is_refused_as_downgrade() {
        let tmp = TempDir::new().unwrap();
        let data_dir = tmp.path();
        write_layout_marker(data_dir, CURRENT_LAYOUT_VERSION + 1).unwrap();

        let err = migrate_data_dir(data_dir).expect_err("downgrade must be refused");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("written by a newer zlayer"),
            "error should explain the downgrade, got: {msg}"
        );
    }

    #[test]
    fn current_marker_is_noop() {
        let tmp = TempDir::new().unwrap();
        let data_dir = tmp.path();
        // Fully-migrated install: current marker plus a current secrets dir.
        let secrets_dir = data_dir.join("secrets");
        fs::create_dir_all(&secrets_dir).unwrap();
        fs::write(secrets_dir.join("secrets.sqlite"), fake_sqlite_bytes()).unwrap();
        write_layout_marker(data_dir, CURRENT_LAYOUT_VERSION).unwrap();

        let report = migrate_data_dir(data_dir).expect("migration should succeed");
        assert!(
            !report.changed(),
            "current marker should be a no-op: {report:?}"
        );

        let marker = fs::read_to_string(layout_marker_path(data_dir)).unwrap();
        assert_eq!(marker, format!("{CURRENT_LAYOUT_VERSION}\n"));
    }

    #[test]
    fn corrupt_marker_is_refused() {
        let tmp = TempDir::new().unwrap();
        let data_dir = tmp.path();
        fs::write(layout_marker_path(data_dir), b"not-a-number").unwrap();

        let err = migrate_data_dir(data_dir).expect_err("corrupt marker must be refused");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("not a valid layout version"),
            "error should explain the corrupt marker, got: {msg}"
        );
    }

    #[test]
    fn detect_incompatible_stores_is_none_for_public_build() {
        let tmp = TempDir::new().unwrap();
        let data_dir = tmp.path();
        let secrets_dir = data_dir.join("secrets");
        fs::create_dir_all(&secrets_dir).unwrap();
        fs::write(secrets_dir.join("secrets.sqlite"), fake_sqlite_bytes()).unwrap();

        assert!(
            detect_incompatible_stores(data_dir).is_none(),
            "public build recognises all stores it ships with"
        );
    }
}
