//! Persistent CLI session file at `~/.zlayer/session.json` (mode 0600 on Unix).
//!
//! Stores the JWT returned by `POST /auth/token` (or `/auth/login`) so
//! subsequent CLI invocations don't need to re-authenticate. The daemon client
//! attaches `Authorization: Bearer <token>` from this file when present.
//!
//! The file is NOT required when the CLI talks to the local daemon over its
//! Unix socket -- that path has an auto-injected admin bearer. The session
//! file matters for (a) remote TCP access and (b) running CLI commands as a
//! specific user rather than the local admin.

use std::path::PathBuf;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::debug;

/// Filename under `~/.zlayer/`.
const SESSION_FILENAME: &str = "session.json";

/// Persisted session record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    /// JWT access token.
    pub token: String,
    /// Email of the authenticated user (for `whoami` display).
    pub email: String,
    /// Token expiry. Used to warn the user before the token actually expires.
    pub expires_at: DateTime<Utc>,
}

impl Session {
    /// Whether the token is past its expiry.
    #[must_use]
    pub fn is_expired(&self) -> bool {
        Utc::now() >= self.expires_at
    }
}

/// Default session file path (`~/.zlayer/session.json`). Errors if the home
/// directory cannot be determined.
///
/// # Errors
///
/// Returns an error if neither `HOME` nor `USERPROFILE` is set in the
/// environment (no way to locate a per-user config directory).
pub fn default_session_path() -> Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .context("neither HOME nor USERPROFILE is set")?;
    Ok(PathBuf::from(home).join(".zlayer").join(SESSION_FILENAME))
}

/// Read the current session from disk. Returns `Ok(None)` when the file is
/// absent (not an error -- unauthenticated CLI is the normal startup state).
///
/// # Errors
///
/// Returns an error if the file exists but cannot be read or parsed.
pub fn read_session() -> Result<Option<Session>> {
    let path = default_session_path()?;
    if !path.exists() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("reading session file at {}", path.display()))?;
    let session: Session = serde_json::from_str(&raw)
        .with_context(|| format!("parsing session file at {}", path.display()))?;
    Ok(Some(session))
}

/// Write `session` atomically to `~/.zlayer/session.json` with mode 0600 on
/// Unix. Creates the parent directory if needed.
///
/// Uses a temp-file-then-rename dance so a crash mid-write never leaves a
/// half-written session.
///
/// # Errors
///
/// Returns an error if the parent directory, temp file, or rename fails.
pub fn write_session(session: &Session) -> Result<()> {
    let path = default_session_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }

    let json = serde_json::to_string_pretty(session).context("serialising session")?;

    // Atomic write: temp file in the same directory, then rename.
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &json)
        .with_context(|| format!("writing temp session file at {}", tmp.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(&tmp, perms)
            .with_context(|| format!("chmod 0600 on {}", tmp.display()))?;
    }

    std::fs::rename(&tmp, &path)
        .with_context(|| format!("renaming {} -> {}", tmp.display(), path.display()))?;

    debug!(path = %path.display(), email = %session.email, "Wrote session file");
    Ok(())
}

/// Delete the session file if it exists. Returns `Ok(false)` if there was
/// nothing to delete.
///
/// # Errors
///
/// Returns an error only for unexpected filesystem failures (permission
/// denied, etc.) -- a missing file is NOT an error.
pub fn delete_session() -> Result<bool> {
    let path = default_session_path()?;
    match std::fs::remove_file(&path) {
        Ok(()) => {
            debug!(path = %path.display(), "Deleted session file");
            Ok(true)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(anyhow::anyhow!("deleting {}: {e}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn test_session() -> Session {
        Session {
            token: "eyJ.fake.jwt".to_string(),
            email: "admin@example.com".to_string(),
            expires_at: Utc::now() + Duration::hours(1),
        }
    }

    #[test]
    fn session_is_expired_returns_true_after_expiry() {
        let mut s = test_session();
        s.expires_at = Utc::now() - Duration::seconds(1);
        assert!(s.is_expired());
    }

    #[test]
    fn session_is_not_expired_when_in_future() {
        let s = test_session();
        assert!(!s.is_expired());
    }

    #[test]
    fn session_roundtrip_via_json() {
        let s = test_session();
        let json = serde_json::to_string(&s).unwrap();
        let parsed: Session = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.token, s.token);
        assert_eq!(parsed.email, s.email);
    }

    /// Combined write/read/delete + mode-0600 check.
    ///
    /// These are fused into a single test so the HOME-swap happens exactly
    /// once; splitting them would race because cargo runs `#[test]` functions
    /// in parallel by default and they'd clobber each other's `HOME` env var.
    ///
    /// `#[allow(unsafe_code)]` is required because `std::env::set_var` is
    /// `unsafe` in the 2024 edition, and the workspace lint policy is
    /// `unsafe_code = "warn"` promoted to error by `-D warnings`.
    #[allow(unsafe_code)]
    #[test]
    fn test_write_read_delete_and_mode() {
        // Point HOME at a temp dir so the test never touches the real ~/.zlayer.
        let tmp = tempfile::tempdir().unwrap();
        let prev_home = std::env::var_os("HOME");
        // SAFETY: single-threaded test, env var mutation scope limited to this test.
        unsafe {
            std::env::set_var("HOME", tmp.path());
        }

        let s = test_session();
        write_session(&s).unwrap();

        let back = read_session()
            .unwrap()
            .expect("session must exist after write");
        assert_eq!(back.token, s.token);
        assert_eq!(back.email, s.email);

        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let path = default_session_path().unwrap();
            let meta = std::fs::metadata(&path).unwrap();
            let mode = meta.mode() & 0o777;
            assert_eq!(mode, 0o600, "expected 0o600, got {mode:o}");
        }

        let deleted = delete_session().unwrap();
        assert!(deleted);

        let after = read_session().unwrap();
        assert!(after.is_none());

        // Deleting a missing file is not an error.
        let deleted_again = delete_session().unwrap();
        assert!(!deleted_again);

        // Restore HOME.
        // SAFETY: single-threaded test, restoring previous value.
        unsafe {
            match prev_home {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
        }
    }
}
