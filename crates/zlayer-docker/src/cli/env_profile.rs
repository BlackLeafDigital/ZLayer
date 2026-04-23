//! Shell-profile env writer for `zlayer docker install` (Unix).
//!
//! Writes `~/.zlayer/env.sh` (POSIX) and
//! `~/.config/fish/conf.d/zlayer-docker.fish` (fish) and appends a guarded
//! block to `~/.bashrc`, `~/.zshrc`, and `~/.profile` that sources the
//! POSIX env file. Windows uses a separate registry-based writer.

#![cfg_attr(windows, allow(dead_code, unused_imports))]

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

const GUARD_BEGIN: &str = "# >>> zlayer docker compat >>>";
const GUARD_END: &str = "# <<< zlayer docker compat <<<";
const ENV_SH_REL: &str = ".zlayer/env.sh";
const FISH_CONF_REL: &str = ".config/fish/conf.d/zlayer-docker.fish";
const POSIX_RC_FILES: &[&str] = &[".bashrc", ".zshrc", ".profile"];

/// Install shell env snippets. Returns the list of paths written or
/// modified.
///
/// `socket_uri` should be the full URI, e.g.
/// `unix:///var/run/zlayer/docker.sock`. On Windows this is a no-op and
/// returns an empty vec.
///
/// # Errors
///
/// Returns an error if any file write fails.
pub fn install_env_snippet(socket_uri: &str) -> Result<Vec<PathBuf>> {
    #[cfg(windows)]
    {
        let _ = socket_uri;
        Ok(Vec::new())
    }
    #[cfg(not(windows))]
    {
        install_env_snippet_impl(socket_uri, &home_dir()?)
    }
}

/// Uninstall shell env snippets.
///
/// # Errors
///
/// Returns an error if a file exists but cannot be read/written.
pub fn uninstall_env_snippet() -> Result<Vec<PathBuf>> {
    #[cfg(windows)]
    {
        Ok(Vec::new())
    }
    #[cfg(not(windows))]
    {
        uninstall_env_snippet_impl(&home_dir()?)
    }
}

#[cfg(not(windows))]
fn home_dir() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME env var is not set")
}

#[cfg(not(windows))]
fn install_env_snippet_impl(socket_uri: &str, home: &Path) -> Result<Vec<PathBuf>> {
    let mut written: Vec<PathBuf> = Vec::new();

    let env_sh = home.join(ENV_SH_REL);
    if let Some(parent) = env_sh.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let env_sh_content = format!(
        "# Written by `zlayer docker install`. Safe to source from any POSIX shell.\nexport DOCKER_HOST='{socket_uri}'\nexport DOCKER_BUILDKIT=0\n"
    );
    fs::write(&env_sh, env_sh_content.as_bytes())
        .with_context(|| format!("failed to write {}", env_sh.display()))?;
    written.push(env_sh);

    let fish_conf = home.join(FISH_CONF_REL);
    if let Some(parent) = fish_conf.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let fish_content = format!(
        "# Written by `zlayer docker install`.\nset -gx DOCKER_HOST '{socket_uri}'\nset -gx DOCKER_BUILDKIT 0\n"
    );
    fs::write(&fish_conf, fish_content.as_bytes())
        .with_context(|| format!("failed to write {}", fish_conf.display()))?;
    written.push(fish_conf);

    let guard_block = format!(
        "\n{GUARD_BEGIN}\n[ -f \"$HOME/.zlayer/env.sh\" ] && . \"$HOME/.zlayer/env.sh\"\n{GUARD_END}\n"
    );
    for rc_name in POSIX_RC_FILES {
        let rc_path = home.join(rc_name);
        if !rc_path.exists() {
            continue;
        }
        let existing = fs::read_to_string(&rc_path)
            .with_context(|| format!("failed to read {}", rc_path.display()))?;
        if existing.contains(GUARD_BEGIN) {
            continue;
        }
        let mut updated = existing;
        if !updated.ends_with('\n') {
            updated.push('\n');
        }
        updated.push_str(&guard_block);
        fs::write(&rc_path, updated.as_bytes())
            .with_context(|| format!("failed to write {}", rc_path.display()))?;
        written.push(rc_path);
    }

    Ok(written)
}

#[cfg(not(windows))]
fn uninstall_env_snippet_impl(home: &Path) -> Result<Vec<PathBuf>> {
    let mut touched: Vec<PathBuf> = Vec::new();

    let env_sh = home.join(ENV_SH_REL);
    if env_sh.exists() {
        fs::remove_file(&env_sh)
            .with_context(|| format!("failed to remove {}", env_sh.display()))?;
        touched.push(env_sh);
    }

    let fish_conf = home.join(FISH_CONF_REL);
    if fish_conf.exists() {
        fs::remove_file(&fish_conf)
            .with_context(|| format!("failed to remove {}", fish_conf.display()))?;
        touched.push(fish_conf);
    }

    for rc_name in POSIX_RC_FILES {
        let rc_path = home.join(rc_name);
        if !rc_path.exists() {
            continue;
        }
        let existing = fs::read_to_string(&rc_path)
            .with_context(|| format!("failed to read {}", rc_path.display()))?;
        if !existing.contains(GUARD_BEGIN) {
            continue;
        }
        let stripped = strip_guarded_block(&existing);
        if stripped != existing {
            fs::write(&rc_path, stripped.as_bytes())
                .with_context(|| format!("failed to write {}", rc_path.display()))?;
            touched.push(rc_path);
        }
    }

    Ok(touched)
}

#[cfg(not(windows))]
fn strip_guarded_block(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut skipping = false;
    for line in input.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if !skipping {
            if trimmed == GUARD_BEGIN {
                skipping = true;
                if out.ends_with("\n\n") {
                    out.pop();
                }
                continue;
            }
            out.push_str(line);
        } else if trimmed == GUARD_END {
            skipping = false;
        }
    }
    out
}

#[cfg(all(test, not(windows)))]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn seed_rc(dir: &Path, name: &str, content: &str) -> PathBuf {
        let p = dir.join(name);
        fs::write(&p, content).unwrap();
        p
    }

    #[test]
    fn install_writes_env_sh_and_fish_conf() {
        let home = TempDir::new().unwrap();
        let written =
            install_env_snippet_impl("unix:///tmp/zlayer-docker.sock", home.path()).unwrap();
        let env_sh = home.path().join(ENV_SH_REL);
        let fish = home.path().join(FISH_CONF_REL);
        assert!(env_sh.exists());
        assert!(fish.exists());
        let body = fs::read_to_string(&env_sh).unwrap();
        assert!(body.contains("DOCKER_HOST='unix:///tmp/zlayer-docker.sock'"));
        assert!(body.contains("DOCKER_BUILDKIT=0"));
        let fbody = fs::read_to_string(&fish).unwrap();
        assert!(fbody.contains("set -gx DOCKER_HOST 'unix:///tmp/zlayer-docker.sock'"));
        assert!(written.iter().any(|p| p == &env_sh));
        assert!(written.iter().any(|p| p == &fish));
    }

    #[test]
    fn install_appends_guard_to_existing_rc_files() {
        let home = TempDir::new().unwrap();
        let bashrc = seed_rc(
            home.path(),
            ".bashrc",
            "# existing user content\nalias x='y'\n",
        );
        let _ = install_env_snippet_impl("unix:///sock", home.path()).unwrap();
        let body = fs::read_to_string(&bashrc).unwrap();
        assert!(body.contains("# existing user content"));
        assert!(body.contains(GUARD_BEGIN));
        assert!(body.contains(GUARD_END));
        assert!(body.contains("$HOME/.zlayer/env.sh"));
    }

    #[test]
    fn install_skips_nonexistent_rc_files() {
        let home = TempDir::new().unwrap();
        let _ = install_env_snippet_impl("unix:///sock", home.path()).unwrap();
        for rc in POSIX_RC_FILES {
            assert!(
                !home.path().join(rc).exists(),
                "{rc} should not have been created"
            );
        }
    }

    #[test]
    fn install_is_idempotent_on_rc_files() {
        let home = TempDir::new().unwrap();
        let bashrc = seed_rc(home.path(), ".bashrc", "alias x='y'\n");
        let _ = install_env_snippet_impl("unix:///sock1", home.path()).unwrap();
        let first = fs::read_to_string(&bashrc).unwrap();
        let _ = install_env_snippet_impl("unix:///sock2", home.path()).unwrap();
        let second = fs::read_to_string(&bashrc).unwrap();
        assert_eq!(
            first, second,
            "rc file should not gain duplicate guard block"
        );
    }

    #[test]
    fn uninstall_removes_everything_written_by_install() {
        let home = TempDir::new().unwrap();
        let bashrc = seed_rc(home.path(), ".bashrc", "alias x='y'\n");
        let _ = install_env_snippet_impl("unix:///sock", home.path()).unwrap();
        let _ = uninstall_env_snippet_impl(home.path()).unwrap();
        assert!(!home.path().join(ENV_SH_REL).exists());
        assert!(!home.path().join(FISH_CONF_REL).exists());
        let body = fs::read_to_string(&bashrc).unwrap();
        assert!(
            !body.contains(GUARD_BEGIN),
            "bashrc should not contain guard marker after uninstall"
        );
        assert!(
            body.contains("alias x='y'"),
            "user content must be preserved"
        );
    }

    #[test]
    fn uninstall_tolerates_missing_files() {
        let home = TempDir::new().unwrap();
        let touched = uninstall_env_snippet_impl(home.path()).unwrap();
        assert!(touched.is_empty());
    }
}
