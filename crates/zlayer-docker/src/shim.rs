//! Shell/`.cmd` shim helpers for making `docker` and `docker-compose`
//! CLI invocations dispatch to `zlayer docker ...`.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Outcome of an `install_shim` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShimInstalled {
    /// Shim written; no prior file existed at the target path.
    Fresh(PathBuf),
    /// Shim written; the prior (foreign, non-zlayer) file was moved to
    /// `{path}.zlayer-backup`.
    ReplacedExisting { shim: PathBuf, backup: PathBuf },
    /// Shim already existed and was already our shim (content-match); no-op.
    AlreadyOurs(PathBuf),
}

/// Install a shell shim (Unix) or `.cmd` wrapper (Windows) at
/// `shim_dir/<name>` (Unix) or `shim_dir/<name>.cmd` (Windows) that execs
/// `{target_invocation} "$@"`.
///
/// `target_invocation` should be the raw invocation text, e.g.
/// `"zlayer docker"` or `"zlayer docker compose"`. On Unix the shim body
/// is:
///
/// ```sh
/// #!/bin/sh
/// exec {target_invocation} "$@"
/// ```
///
/// On Windows:
///
/// ```cmd
/// @echo off
/// {target_invocation} %*
/// ```
///
/// If a non-shim file already exists at the target path, it is moved to
/// `{path}.zlayer-backup` before writing the new shim. If the existing
/// file IS already our shim (identical content), the function is a no-op
/// and returns [`ShimInstalled::AlreadyOurs`].
///
/// The `shim_dir` must already exist; this function does not `mkdir -p`.
///
/// # Errors
///
/// Returns an error if the shim directory is missing, the target file
/// cannot be written, or the backup rename fails.
pub fn install_shim(shim_dir: &Path, name: &str, target_invocation: &str) -> Result<ShimInstalled> {
    let shim_path = shim_path_for(shim_dir, name);
    let new_content = render_shim(target_invocation);

    if shim_path.exists() {
        let existing = fs::read_to_string(&shim_path)
            .with_context(|| format!("failed to read existing file at {}", shim_path.display()))?;
        if existing == new_content {
            return Ok(ShimInstalled::AlreadyOurs(shim_path));
        }
        let backup = backup_path(&shim_path);
        fs::rename(&shim_path, &backup).with_context(|| {
            format!(
                "failed to back up existing file {} to {}",
                shim_path.display(),
                backup.display()
            )
        })?;
        write_shim(&shim_path, &new_content)?;
        return Ok(ShimInstalled::ReplacedExisting {
            shim: shim_path,
            backup,
        });
    }

    write_shim(&shim_path, &new_content)?;
    Ok(ShimInstalled::Fresh(shim_path))
}

/// Outcome of an `uninstall_shim` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShimUninstalled {
    /// Shim was not present; nothing to do.
    NotPresent,
    /// Shim was removed; no backup existed to restore.
    Removed(PathBuf),
    /// Shim was removed and the `{path}.zlayer-backup` file was restored
    /// in place of the shim.
    RemovedAndRestored {
        shim: PathBuf,
        restored_from: PathBuf,
    },
}

/// Remove a shim installed by [`install_shim`]. Optionally restores a
/// `{path}.zlayer-backup` file if one exists.
///
/// If the file at the target path is NOT our shim (content mismatch),
/// this function leaves it untouched and returns [`ShimUninstalled::NotPresent`]
/// — we do not want to remove user-installed binaries.
///
/// # Errors
///
/// Returns an error if the shim or backup exists but cannot be removed /
/// restored.
pub fn uninstall_shim(
    shim_dir: &Path,
    name: &str,
    target_invocation: &str,
    restore_backup: bool,
) -> Result<ShimUninstalled> {
    let shim_path = shim_path_for(shim_dir, name);
    if !shim_path.exists() {
        // Even if no shim, a backup might still be around from a prior
        // install; we leave it alone — uninstall only restores on
        // explicit request AND when we also removed the shim.
        return Ok(ShimUninstalled::NotPresent);
    }

    let expected = render_shim(target_invocation);
    let actual = fs::read_to_string(&shim_path)
        .with_context(|| format!("failed to read {}", shim_path.display()))?;
    if actual != expected {
        // Not our shim — leave user's file alone.
        return Ok(ShimUninstalled::NotPresent);
    }

    fs::remove_file(&shim_path)
        .with_context(|| format!("failed to remove {}", shim_path.display()))?;

    if restore_backup {
        let backup = backup_path(&shim_path);
        if backup.exists() {
            fs::rename(&backup, &shim_path).with_context(|| {
                format!(
                    "failed to restore backup {} to {}",
                    backup.display(),
                    shim_path.display()
                )
            })?;
            return Ok(ShimUninstalled::RemovedAndRestored {
                shim: shim_path,
                restored_from: backup,
            });
        }
    }
    Ok(ShimUninstalled::Removed(shim_path))
}

fn shim_path_for(shim_dir: &Path, name: &str) -> PathBuf {
    #[cfg(windows)]
    {
        shim_dir.join(format!("{name}.cmd"))
    }
    #[cfg(not(windows))]
    {
        shim_dir.join(name)
    }
}

fn backup_path(shim_path: &Path) -> PathBuf {
    let mut s = shim_path.as_os_str().to_os_string();
    s.push(".zlayer-backup");
    PathBuf::from(s)
}

fn render_shim(target_invocation: &str) -> String {
    #[cfg(windows)]
    {
        format!("@echo off\r\n{target_invocation} %*\r\n")
    }
    #[cfg(not(windows))]
    {
        format!("#!/bin/sh\nexec {target_invocation} \"$@\"\n")
    }
}

fn write_shim(path: &Path, content: &str) -> Result<()> {
    fs::write(path, content)
        .with_context(|| format!("failed to write shim at {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))
            .with_context(|| format!("failed to chmod 0755 {}", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn shim_name() -> &'static str {
        "docker"
    }

    #[test]
    fn install_shim_writes_fresh_file() {
        let dir = TempDir::new().unwrap();
        let result = install_shim(dir.path(), shim_name(), "zlayer docker").unwrap();
        match result {
            ShimInstalled::Fresh(p) => {
                assert!(p.exists());
                let content = fs::read_to_string(&p).unwrap();
                #[cfg(windows)]
                assert!(content.contains("zlayer docker %*"));
                #[cfg(not(windows))]
                {
                    assert!(content.starts_with("#!/bin/sh"));
                    assert!(content.contains("exec zlayer docker \"$@\""));
                }
            }
            other => panic!("expected Fresh, got {other:?}"),
        }
    }

    #[test]
    fn install_shim_is_idempotent() {
        let dir = TempDir::new().unwrap();
        let _ = install_shim(dir.path(), shim_name(), "zlayer docker").unwrap();
        let second = install_shim(dir.path(), shim_name(), "zlayer docker").unwrap();
        assert!(matches!(second, ShimInstalled::AlreadyOurs(_)));
    }

    #[test]
    fn install_shim_backs_up_foreign_file() {
        let dir = TempDir::new().unwrap();
        let shim_file = shim_path_for(dir.path(), shim_name());
        fs::write(&shim_file, "foreign content").unwrap();
        let result = install_shim(dir.path(), shim_name(), "zlayer docker").unwrap();
        match result {
            ShimInstalled::ReplacedExisting { shim, backup } => {
                assert_eq!(shim, shim_file);
                assert!(backup.exists());
                assert_eq!(fs::read_to_string(&backup).unwrap(), "foreign content");
            }
            other => panic!("expected ReplacedExisting, got {other:?}"),
        }
    }

    #[test]
    fn uninstall_shim_removes_our_shim() {
        let dir = TempDir::new().unwrap();
        let _ = install_shim(dir.path(), shim_name(), "zlayer docker").unwrap();
        let result = uninstall_shim(dir.path(), shim_name(), "zlayer docker", false).unwrap();
        assert!(matches!(result, ShimUninstalled::Removed(_)));
        assert!(!shim_path_for(dir.path(), shim_name()).exists());
    }

    #[test]
    fn uninstall_shim_leaves_foreign_file() {
        let dir = TempDir::new().unwrap();
        let shim_file = shim_path_for(dir.path(), shim_name());
        fs::write(&shim_file, "foreign").unwrap();
        let result = uninstall_shim(dir.path(), shim_name(), "zlayer docker", false).unwrap();
        assert_eq!(result, ShimUninstalled::NotPresent);
        assert!(shim_file.exists()); // preserved
    }

    #[test]
    fn uninstall_shim_restores_backup_when_requested() {
        let dir = TempDir::new().unwrap();
        let shim_file = shim_path_for(dir.path(), shim_name());
        fs::write(&shim_file, "foreign").unwrap();
        let _ = install_shim(dir.path(), shim_name(), "zlayer docker").unwrap();
        let result = uninstall_shim(dir.path(), shim_name(), "zlayer docker", true).unwrap();
        assert!(matches!(result, ShimUninstalled::RemovedAndRestored { .. }));
        assert_eq!(fs::read_to_string(&shim_file).unwrap(), "foreign");
    }
}
