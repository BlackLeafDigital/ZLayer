//! Layered environment-variable sources for Docker Compose-compatible
//! interpolation.
//!
//! Priority (later overrides earlier), matching `docker compose`:
//! 1. `<project_dir>/.env` (lowest)
//! 2. `--env-file` files, processed in order (later files override earlier)
//! 3. The current process environment (highest)

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use thiserror::Error;

/// Errors raised while collecting environment sources.
#[derive(Debug, Error)]
pub enum EnvSourceError {
    /// An env file failed to read from disk.
    #[error("failed to read env file `{path}`: {source}")]
    Io {
        /// The path that failed.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// An env file contained a malformed line.
    #[error("env file `{path}` line {line}: {message}")]
    Parse {
        /// The path that failed.
        path: PathBuf,
        /// 1-based line number.
        line: usize,
        /// Diagnostic message.
        message: String,
    },
}

/// Collect environment variables from all sources, applying Docker Compose's
/// precedence rules.
///
/// Priority (highest wins):
/// 1. Process environment (`std::env::vars`)
/// 2. `--env-file` files in the order given (later overrides earlier)
/// 3. `<project_dir>/.env` (only if it exists)
///
/// # Errors
///
/// Returns an error if any explicitly-named env file (in `env_files`) cannot
/// be read or parsed. A missing `<project_dir>/.env` is *not* an error — that
/// file is optional.
pub fn collect_env_sources(
    project_dir: &Path,
    env_files: &[PathBuf],
) -> Result<HashMap<String, String>, EnvSourceError> {
    let mut env: HashMap<String, String> = HashMap::new();

    // 1. Lowest priority: <project_dir>/.env (optional).
    let project_env = project_dir.join(".env");
    if project_env.is_file() {
        let content = std::fs::read_to_string(&project_env).map_err(|e| EnvSourceError::Io {
            path: project_env.clone(),
            source: e,
        })?;
        let pairs = parse_env_file(&content).map_err(|e| annotate_path(e, &project_env))?;
        for (k, v) in pairs {
            env.insert(k, v);
        }
    }

    // 2. --env-file files in order; later overrides earlier.
    for path in env_files {
        let content = std::fs::read_to_string(path).map_err(|e| EnvSourceError::Io {
            path: path.clone(),
            source: e,
        })?;
        let pairs = parse_env_file(&content).map_err(|e| annotate_path(e, path))?;
        for (k, v) in pairs {
            env.insert(k, v);
        }
    }

    // 3. Highest priority: process environment.
    for (k, v) in std::env::vars() {
        env.insert(k, v);
    }

    Ok(env)
}

/// Re-tag an [`EnvSourceError::Parse`] coming from [`parse_env_file`] with the
/// originating path. `parse_env_file` doesn't know its own path, so we patch
/// it here.
fn annotate_path(err: EnvSourceError, path: &Path) -> EnvSourceError {
    match err {
        EnvSourceError::Parse { line, message, .. } => EnvSourceError::Parse {
            path: path.to_path_buf(),
            line,
            message,
        },
        EnvSourceError::Io { .. } => err,
    }
}

/// Parse the textual contents of a `.env` / `--env-file` file.
///
/// Recognised forms (one per line):
/// - `KEY=VALUE`
/// - `KEY="quoted value"` — escapes `\\`, `\"`, `\n`, `\r`, `\t`
/// - `KEY='quoted value'` — no escapes; literal contents
/// - `export KEY=VALUE` — `export ` prefix is accepted and stripped
/// - `# comment lines` and blank lines are ignored
/// - Trailing inline comments after an unquoted value are NOT stripped
///   (Docker Compose's loader treats them as part of the value); for parity
///   with Compose's `dotenv` behaviour we treat the entire post-`=` text as
///   the value when unquoted, only trimming surrounding whitespace.
///
/// # Errors
///
/// Returns [`EnvSourceError::Parse`] (with `path` left as an empty
/// `PathBuf` — callers should annotate via [`annotate_path`]) when a line is
/// not blank, not a comment, and does not contain `=`, or when a quoted value
/// is unterminated, or when the key is empty/invalid.
pub fn parse_env_file(content: &str) -> Result<Vec<(String, String)>, EnvSourceError> {
    let mut out = Vec::new();
    for (idx, raw_line) in content.lines().enumerate() {
        let line_no = idx + 1;
        let trimmed = raw_line.trim_start();

        // Blank or comment.
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // Optional `export ` prefix.
        let after_export = if let Some(rest) = trimmed.strip_prefix("export ") {
            rest.trim_start()
        } else if let Some(rest) = trimmed.strip_prefix("export\t") {
            rest.trim_start()
        } else {
            trimmed
        };

        let eq_pos = after_export
            .find('=')
            .ok_or_else(|| EnvSourceError::Parse {
                path: PathBuf::new(),
                line: line_no,
                message: format!("missing `=` in line: `{raw_line}`"),
            })?;

        let key = after_export[..eq_pos].trim();
        if key.is_empty() {
            return Err(EnvSourceError::Parse {
                path: PathBuf::new(),
                line: line_no,
                message: "empty key".into(),
            });
        }
        if !is_valid_key(key) {
            return Err(EnvSourceError::Parse {
                path: PathBuf::new(),
                line: line_no,
                message: format!("invalid key `{key}`"),
            });
        }

        let raw_value = &after_export[eq_pos + 1..];
        let value = parse_value(raw_value, line_no)?;

        out.push((key.to_string(), value));
    }
    Ok(out)
}

fn is_valid_key(key: &str) -> bool {
    let mut chars = key.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn parse_value(raw: &str, line_no: usize) -> Result<String, EnvSourceError> {
    // Trim leading whitespace before checking for quotes; trailing whitespace
    // is preserved inside quotes but trimmed for unquoted values.
    let trimmed_start = raw.trim_start();

    if let Some(rest) = trimmed_start.strip_prefix('"') {
        // Double-quoted: process escapes; require terminating unescaped `"`.
        let mut out = String::with_capacity(rest.len());
        let mut chars = rest.chars();
        loop {
            match chars.next() {
                None => {
                    return Err(EnvSourceError::Parse {
                        path: PathBuf::new(),
                        line: line_no,
                        message: "unterminated double-quoted value".into(),
                    });
                }
                Some('"') => {
                    // End of quoted region. Trailing junk after the close
                    // quote is ignored (Docker dotenv behaviour).
                    return Ok(out);
                }
                Some('\\') => match chars.next() {
                    Some('n') => out.push('\n'),
                    Some('r') => out.push('\r'),
                    Some('t') => out.push('\t'),
                    Some('"') => out.push('"'),
                    Some('\\') => out.push('\\'),
                    Some(other) => {
                        // Unknown escape: keep the backslash literal.
                        out.push('\\');
                        out.push(other);
                    }
                    None => {
                        return Err(EnvSourceError::Parse {
                            path: PathBuf::new(),
                            line: line_no,
                            message: "trailing backslash in double-quoted value".into(),
                        });
                    }
                },
                Some(c) => out.push(c),
            }
        }
    }

    if let Some(rest) = trimmed_start.strip_prefix('\'') {
        // Single-quoted: literal contents, no escapes; require terminating `'`.
        if let Some(end) = rest.find('\'') {
            return Ok(rest[..end].to_string());
        }
        return Err(EnvSourceError::Parse {
            path: PathBuf::new(),
            line: line_no,
            message: "unterminated single-quoted value".into(),
        });
    }

    // Unquoted: trim trailing whitespace, keep the rest verbatim.
    Ok(trimmed_start.trim_end().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::{Mutex, OnceLock};
    use zlayer_paths::ZLayerDirs;

    /// Serialise tests that mutate the process environment.
    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn parse_basic_pairs() {
        let content = "FOO=bar\nBAZ=qux\n";
        let pairs = parse_env_file(content).unwrap();
        assert_eq!(
            pairs,
            vec![
                ("FOO".to_string(), "bar".to_string()),
                ("BAZ".to_string(), "qux".to_string()),
            ]
        );
    }

    #[test]
    fn parse_skips_blank_and_comment_lines() {
        let content = "\n# leading comment\nFOO=1\n\n  # indented comment\nBAR=2\n";
        let pairs = parse_env_file(content).unwrap();
        assert_eq!(
            pairs,
            vec![
                ("FOO".to_string(), "1".to_string()),
                ("BAR".to_string(), "2".to_string()),
            ]
        );
    }

    #[test]
    fn parse_export_prefix() {
        let content = "export FOO=bar\nexport\tBAZ=qux\n";
        let pairs = parse_env_file(content).unwrap();
        assert_eq!(
            pairs,
            vec![
                ("FOO".to_string(), "bar".to_string()),
                ("BAZ".to_string(), "qux".to_string()),
            ]
        );
    }

    #[test]
    fn parse_double_quoted_with_escapes() {
        let content = r#"GREETING="hello\nworld"
TAB="a\tb"
QUOTE="say \"hi\""
"#;
        let pairs = parse_env_file(content).unwrap();
        assert_eq!(
            pairs,
            vec![
                ("GREETING".to_string(), "hello\nworld".to_string()),
                ("TAB".to_string(), "a\tb".to_string()),
                ("QUOTE".to_string(), "say \"hi\"".to_string()),
            ]
        );
    }

    #[test]
    fn parse_single_quoted_literal() {
        let content = "RAW='hello\\nworld'\n";
        let pairs = parse_env_file(content).unwrap();
        assert_eq!(
            pairs,
            vec![("RAW".to_string(), "hello\\nworld".to_string())]
        );
    }

    #[test]
    fn parse_unquoted_trims_trailing_ws() {
        let content = "FOO=  bar  \n";
        let pairs = parse_env_file(content).unwrap();
        assert_eq!(pairs, vec![("FOO".to_string(), "bar".to_string())]);
    }

    #[test]
    fn parse_value_with_equals_sign() {
        let content = "URL=http://example.com/?a=1&b=2\n";
        let pairs = parse_env_file(content).unwrap();
        assert_eq!(
            pairs,
            vec![("URL".to_string(), "http://example.com/?a=1&b=2".to_string())]
        );
    }

    #[test]
    fn parse_missing_equals_errors() {
        let content = "JUSTAKEY\n";
        let err = parse_env_file(content).unwrap_err();
        match err {
            EnvSourceError::Parse { line, .. } => assert_eq!(line, 1),
            other @ EnvSourceError::Io { .. } => panic!("expected Parse, got {other:?}"),
        }
    }

    #[test]
    fn parse_empty_key_errors() {
        let content = "=value\n";
        assert!(matches!(
            parse_env_file(content),
            Err(EnvSourceError::Parse { .. })
        ));
    }

    #[test]
    fn parse_invalid_key_errors() {
        let content = "1FOO=bar\n";
        assert!(matches!(
            parse_env_file(content),
            Err(EnvSourceError::Parse { .. })
        ));
    }

    #[test]
    fn parse_unterminated_double_quote_errors() {
        let content = "FOO=\"oops\n";
        assert!(matches!(
            parse_env_file(content),
            Err(EnvSourceError::Parse { .. })
        ));
    }

    #[test]
    fn parse_unterminated_single_quote_errors() {
        let content = "FOO='oops\n";
        assert!(matches!(
            parse_env_file(content),
            Err(EnvSourceError::Parse { .. })
        ));
    }

    #[test]
    fn collect_priority_process_overrides_env_file() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = ZLayerDirs::system_default()
            .scratch_dir("collect-priority-process-overrides-env-file-")
            .unwrap();
        let env_file = dir.path().join("custom.env");
        let mut f = std::fs::File::create(&env_file).unwrap();
        writeln!(f, "ZLAYER_TEST_PRIORITY_VAR=from_file").unwrap();

        // SAFETY: serialised by `env_lock()` so no other thread can read or
        // mutate this var concurrently while the guard is held.
        #[allow(unsafe_code)]
        unsafe {
            std::env::set_var("ZLAYER_TEST_PRIORITY_VAR", "from_process");
        }

        let env = collect_env_sources(dir.path(), std::slice::from_ref(&env_file)).unwrap();
        assert_eq!(
            env.get("ZLAYER_TEST_PRIORITY_VAR").map(String::as_str),
            Some("from_process")
        );

        // SAFETY: see comment above.
        #[allow(unsafe_code)]
        unsafe {
            std::env::remove_var("ZLAYER_TEST_PRIORITY_VAR");
        }
    }

    #[test]
    fn collect_priority_env_file_overrides_dotenv() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = ZLayerDirs::system_default()
            .scratch_dir("collect-priority-env-file-overrides-dotenv-")
            .unwrap();

        let dotenv = dir.path().join(".env");
        let mut f = std::fs::File::create(&dotenv).unwrap();
        writeln!(f, "ZLAYER_TEST_LAYER_VAR=from_dotenv").unwrap();
        writeln!(f, "ONLY_IN_DOTENV=yes").unwrap();
        drop(f);

        let env_file = dir.path().join("override.env");
        let mut f = std::fs::File::create(&env_file).unwrap();
        writeln!(f, "ZLAYER_TEST_LAYER_VAR=from_envfile").unwrap();
        drop(f);

        // SAFETY: serialised by `env_lock()`; nothing else may touch these
        // vars while the guard is held.
        #[allow(unsafe_code)]
        unsafe {
            std::env::remove_var("ZLAYER_TEST_LAYER_VAR");
            std::env::remove_var("ONLY_IN_DOTENV");
        }

        let env = collect_env_sources(dir.path(), &[env_file]).unwrap();
        assert_eq!(
            env.get("ZLAYER_TEST_LAYER_VAR").map(String::as_str),
            Some("from_envfile")
        );
        assert_eq!(env.get("ONLY_IN_DOTENV").map(String::as_str), Some("yes"));
    }

    #[test]
    fn collect_priority_later_env_file_overrides_earlier() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = ZLayerDirs::system_default()
            .scratch_dir("collect-priority-later-env-file-overrides-earlier-")
            .unwrap();
        let a = dir.path().join("a.env");
        let b = dir.path().join("b.env");
        std::fs::write(&a, "ZLAYER_TEST_ORDER_VAR=from_a\nONLY_A=yes\n").unwrap();
        std::fs::write(&b, "ZLAYER_TEST_ORDER_VAR=from_b\n").unwrap();

        // SAFETY: serialised by `env_lock()`.
        #[allow(unsafe_code)]
        unsafe {
            std::env::remove_var("ZLAYER_TEST_ORDER_VAR");
            std::env::remove_var("ONLY_A");
        }

        let env = collect_env_sources(dir.path(), &[a, b]).unwrap();
        assert_eq!(
            env.get("ZLAYER_TEST_ORDER_VAR").map(String::as_str),
            Some("from_b")
        );
        assert_eq!(env.get("ONLY_A").map(String::as_str), Some("yes"));
    }

    #[test]
    fn collect_dotenv_optional() {
        let dir = ZLayerDirs::system_default()
            .scratch_dir("collect-dotenv-optional-")
            .unwrap();
        // No .env, no env files — should succeed and just return process env.
        let env = collect_env_sources(dir.path(), &[]).unwrap();
        // Spot-check that PATH (essentially always present) made it through.
        // If it's absent (extremely sandboxed test envs), at least confirm
        // no error and a HashMap was produced.
        let _ = env.get("PATH");
    }

    #[test]
    fn collect_missing_explicit_env_file_errors() {
        let dir = ZLayerDirs::system_default()
            .scratch_dir("collect-missing-explicit-env-file-errors-")
            .unwrap();
        let missing = dir.path().join("nope.env");
        let err = collect_env_sources(dir.path(), std::slice::from_ref(&missing)).unwrap_err();
        match err {
            EnvSourceError::Io { path, .. } => assert_eq!(path, missing),
            other @ EnvSourceError::Parse { .. } => panic!("expected Io, got {other:?}"),
        }
    }

    #[test]
    fn collect_parse_error_carries_path() {
        let dir = ZLayerDirs::system_default()
            .scratch_dir("collect-parse-error-carries-path-")
            .unwrap();
        let bad = dir.path().join("bad.env");
        std::fs::write(&bad, "this is not valid\n").unwrap();
        let err = collect_env_sources(dir.path(), std::slice::from_ref(&bad)).unwrap_err();
        match err {
            EnvSourceError::Parse { path, line, .. } => {
                assert_eq!(path, bad);
                assert_eq!(line, 1);
            }
            other @ EnvSourceError::Io { .. } => panic!("expected Parse, got {other:?}"),
        }
    }
}
