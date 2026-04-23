//! Detection + managed symlink for `/var/run/docker.sock` on Unix.
//!
//! Named pipes on Windows cannot be symlinked, so `offer_symlink` is a
//! no-op there (the Windows install flow relies on `DOCKER_HOST` alone).

#![cfg_attr(windows, allow(dead_code, unused_imports))]

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};

/// The state of a file at the candidate docker-sock path.
#[derive(Debug, PartialEq, Eq)]
pub enum ExistingSock {
    /// Nothing at the candidate path.
    Missing,
    /// A symlink already pointing at our target socket path.
    OursAlready,
    /// A symlink pointing at some other target.
    ForeignSymlink(PathBuf),
    /// A real file / socket / directory not owned by us.
    ForeignFile,
}

/// Action taken by [`offer_symlink`].
#[derive(Debug, PartialEq, Eq)]
pub enum SymlinkAction {
    /// Symlink was created fresh.
    Created { at: PathBuf, target: PathBuf },
    /// Existing foreign sock was backed up then replaced by the symlink.
    ReplacedWithBackup {
        at: PathBuf,
        target: PathBuf,
        backup: PathBuf,
    },
    /// Candidate path already pointed at our target — nothing to do.
    AlreadyOurs(PathBuf),
    /// User declined, skip flag set, or platform does not support
    /// filesystem symlinks at that path.
    Skipped(&'static str),
}

/// Inspect the file at `candidate`.
///
/// # Errors
///
/// Returns an error if the filesystem cannot be read (other than the
/// candidate being absent, which is a normal outcome).
pub fn inspect(candidate: &Path, expected_target: &Path) -> Result<ExistingSock> {
    let md = match fs::symlink_metadata(candidate) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ExistingSock::Missing);
        }
        Err(e) => {
            return Err(e).with_context(|| format!("symlink_metadata({})", candidate.display()));
        }
    };
    if md.file_type().is_symlink() {
        let link_target = fs::read_link(candidate)
            .with_context(|| format!("read_link({})", candidate.display()))?;
        if link_target == expected_target {
            return Ok(ExistingSock::OursAlready);
        }
        return Ok(ExistingSock::ForeignSymlink(link_target));
    }
    Ok(ExistingSock::ForeignFile)
}

/// Offer to symlink `at` → `target`.
///
/// Behavior:
/// - On Windows: returns [`SymlinkAction::Skipped`] with a note about
///   named pipes.
/// - If `inspect` reports `Missing`: creates the symlink.
/// - `OursAlready`: no-op, returns [`SymlinkAction::AlreadyOurs`].
/// - `ForeignSymlink` / `ForeignFile`: if `non_interactive == true`,
///   returns `Skipped` unless `force == true`. If interactive, prompts
///   via stdin y/N. Backs up the existing entry to
///   `{at}.zlayer-backup-<unix_ts>` before creating the new symlink.
///
/// # Errors
///
/// Returns an error if a write (backup rename, symlink create) fails.
pub fn offer_symlink(
    target: &Path,
    at: &Path,
    non_interactive: bool,
    force: bool,
) -> Result<SymlinkAction> {
    #[cfg(windows)]
    {
        let _ = (target, at, non_interactive, force);
        Ok(SymlinkAction::Skipped(
            "named pipes cannot be symlinked on Windows; DOCKER_HOST env is set instead",
        ))
    }

    #[cfg(not(windows))]
    {
        match inspect(at, target)? {
            ExistingSock::Missing => {
                create_parent(at)?;
                unix_symlink(target, at)?;
                Ok(SymlinkAction::Created {
                    at: at.to_path_buf(),
                    target: target.to_path_buf(),
                })
            }
            ExistingSock::OursAlready => Ok(SymlinkAction::AlreadyOurs(at.to_path_buf())),
            existing @ (ExistingSock::ForeignSymlink(_) | ExistingSock::ForeignFile) => {
                let proceed = if force {
                    true
                } else if non_interactive {
                    false
                } else {
                    prompt_replace(at, &existing)?
                };
                if !proceed {
                    return Ok(SymlinkAction::Skipped(
                        "user declined to replace existing docker.sock",
                    ));
                }
                let backup = backup_path_for(at);
                fs::rename(at, &backup).with_context(|| {
                    format!("failed to back up {} to {}", at.display(), backup.display())
                })?;
                unix_symlink(target, at)?;
                Ok(SymlinkAction::ReplacedWithBackup {
                    at: at.to_path_buf(),
                    target: target.to_path_buf(),
                    backup,
                })
            }
        }
    }
}

/// Outcome of [`remove_symlink`].
#[derive(Debug, PartialEq, Eq)]
pub enum RemoveAction {
    /// The file at `at` was our symlink and has been removed.
    Removed(PathBuf),
    /// The file at `at` was our symlink and has been removed; a
    /// `.zlayer-backup-<ts>` was restored back in place.
    RemovedAndRestored { at: PathBuf, restored_from: PathBuf },
    /// Nothing was there, or the file was not our symlink.
    Skipped,
}

/// Remove the symlink installed by [`offer_symlink`].
///
/// If `restore_backup == true` and a `{at}.zlayer-backup-*` file exists,
/// it is renamed back to `at`. Only the most recent backup is restored.
///
/// # Errors
///
/// Returns an error if filesystem operations fail.
pub fn remove_symlink(target: &Path, at: &Path, restore_backup: bool) -> Result<RemoveAction> {
    #[cfg(windows)]
    {
        let _ = (target, at, restore_backup);
        Ok(RemoveAction::Skipped)
    }

    #[cfg(not(windows))]
    {
        match inspect(at, target)? {
            ExistingSock::OursAlready => {
                fs::remove_file(at)
                    .with_context(|| format!("failed to remove {}", at.display()))?;
                if restore_backup {
                    if let Some(backup) = newest_backup(at)? {
                        fs::rename(&backup, at).with_context(|| {
                            format!("failed to restore {} to {}", backup.display(), at.display())
                        })?;
                        return Ok(RemoveAction::RemovedAndRestored {
                            at: at.to_path_buf(),
                            restored_from: backup,
                        });
                    }
                }
                Ok(RemoveAction::Removed(at.to_path_buf()))
            }
            _ => Ok(RemoveAction::Skipped),
        }
    }
}

#[cfg(not(windows))]
fn unix_symlink(target: &Path, at: &Path) -> Result<()> {
    std::os::unix::fs::symlink(target, at).with_context(|| {
        format!(
            "failed to create symlink {} -> {}",
            at.display(),
            target.display()
        )
    })
}

#[cfg(not(windows))]
fn create_parent(at: &Path) -> Result<()> {
    if let Some(parent) = at.parent() {
        if !parent.exists() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
    }
    Ok(())
}

fn backup_path_for(at: &Path) -> PathBuf {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut s = at.as_os_str().to_os_string();
    s.push(format!(".zlayer-backup-{ts}"));
    PathBuf::from(s)
}

#[cfg(not(windows))]
fn newest_backup(at: &Path) -> Result<Option<PathBuf>> {
    let parent = match at.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    let base_name = match at.file_name() {
        Some(n) => n.to_os_string(),
        None => return Ok(None),
    };
    let mut prefix = base_name.clone();
    prefix.push(".zlayer-backup-");
    let prefix_str = prefix.to_string_lossy().into_owned();
    let entries = match fs::read_dir(parent) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(e).with_context(|| format!("read_dir({})", parent.display()));
        }
    };
    let mut best: Option<(u64, PathBuf)> = None;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with(&prefix_str) {
            continue;
        }
        if let Some(ts_str) = name.strip_prefix(&prefix_str) {
            if let Ok(ts) = ts_str.parse::<u64>() {
                match best {
                    Some((cur_ts, _)) if cur_ts >= ts => {}
                    _ => best = Some((ts, entry.path())),
                }
            }
        }
    }
    Ok(best.map(|(_, p)| p))
}

#[cfg(not(windows))]
fn prompt_replace(at: &Path, existing: &ExistingSock) -> Result<bool> {
    use std::io::{self, BufRead, Write};
    let description = match existing {
        ExistingSock::ForeignSymlink(target) => {
            format!("existing symlink pointing at {}", target.display())
        }
        ExistingSock::ForeignFile => "existing file/socket (not managed by zlayer)".to_string(),
        _ => return Ok(true),
    };
    print!(
        "{} is already present: {description}.\nBack it up and replace with a ZLayer-managed symlink? [y/N]: ",
        at.display()
    );
    io::stdout().flush().ok();
    let stdin = io::stdin();
    let mut line = String::new();
    match stdin.lock().read_line(&mut line) {
        Ok(_) => {
            let trimmed = line.trim().to_ascii_lowercase();
            Ok(matches!(trimmed.as_str(), "y" | "yes"))
        }
        Err(e) => bail!("failed to read stdin: {e}"),
    }
}

#[cfg(all(test, not(windows)))]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn inspect_missing() {
        let dir = TempDir::new().unwrap();
        let at = dir.path().join("docker.sock");
        let target = dir.path().join("zlayer-docker.sock");
        assert_eq!(inspect(&at, &target).unwrap(), ExistingSock::Missing);
    }

    #[test]
    fn inspect_ours_already() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("zlayer-docker.sock");
        let at = dir.path().join("docker.sock");
        std::os::unix::fs::symlink(&target, &at).unwrap();
        assert_eq!(inspect(&at, &target).unwrap(), ExistingSock::OursAlready);
    }

    #[test]
    fn inspect_foreign_symlink() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("zlayer-docker.sock");
        let elsewhere = dir.path().join("other.sock");
        let at = dir.path().join("docker.sock");
        std::os::unix::fs::symlink(&elsewhere, &at).unwrap();
        match inspect(&at, &target).unwrap() {
            ExistingSock::ForeignSymlink(p) => assert_eq!(p, elsewhere),
            other => panic!("expected ForeignSymlink, got {other:?}"),
        }
    }

    #[test]
    fn inspect_foreign_file() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("zlayer-docker.sock");
        let at = dir.path().join("docker.sock");
        fs::write(&at, b"not a sock").unwrap();
        assert_eq!(inspect(&at, &target).unwrap(), ExistingSock::ForeignFile);
    }

    #[test]
    fn offer_symlink_creates_on_missing() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("zlayer-docker.sock");
        let at = dir.path().join("docker.sock");
        let action = offer_symlink(&target, &at, false, false).unwrap();
        assert!(matches!(action, SymlinkAction::Created { .. }));
        assert_eq!(fs::read_link(&at).unwrap(), target);
    }

    #[test]
    fn offer_symlink_non_interactive_declines_foreign() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("zlayer-docker.sock");
        let at = dir.path().join("docker.sock");
        fs::write(&at, b"foreign").unwrap();
        let action = offer_symlink(&target, &at, true, false).unwrap();
        assert!(matches!(action, SymlinkAction::Skipped(_)));
        // Foreign file preserved.
        assert_eq!(fs::read(&at).unwrap(), b"foreign");
    }

    #[test]
    fn offer_symlink_force_backs_up_and_replaces() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("zlayer-docker.sock");
        let at = dir.path().join("docker.sock");
        fs::write(&at, b"foreign").unwrap();
        let action = offer_symlink(&target, &at, true, true).unwrap();
        match action {
            SymlinkAction::ReplacedWithBackup { backup, .. } => {
                assert!(backup.exists());
                assert_eq!(fs::read(&backup).unwrap(), b"foreign");
                assert_eq!(fs::read_link(&at).unwrap(), target);
            }
            other => panic!("expected ReplacedWithBackup, got {other:?}"),
        }
    }

    #[test]
    fn remove_symlink_restores_backup() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("zlayer-docker.sock");
        let at = dir.path().join("docker.sock");
        fs::write(&at, b"foreign").unwrap();
        let _ = offer_symlink(&target, &at, true, true).unwrap();
        let action = remove_symlink(&target, &at, true).unwrap();
        assert!(matches!(action, RemoveAction::RemovedAndRestored { .. }));
        assert_eq!(fs::read(&at).unwrap(), b"foreign");
    }

    #[test]
    fn remove_symlink_skips_when_not_ours() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("zlayer-docker.sock");
        let at = dir.path().join("docker.sock");
        let other = dir.path().join("other.sock");
        std::os::unix::fs::symlink(&other, &at).unwrap();
        let action = remove_symlink(&target, &at, false).unwrap();
        assert_eq!(action, RemoveAction::Skipped);
        // Foreign symlink preserved.
        assert_eq!(fs::read_link(&at).unwrap(), other);
    }
}
