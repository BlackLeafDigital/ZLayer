//! Docker Compose-style variable interpolation.
//!
//! Implements the variable substitution syntax documented for Docker Compose:
//! <https://docs.docker.com/compose/compose-file/12-interpolation/>
//!
//! Supported forms:
//! - `$VAR` and `${VAR}`
//! - `${VAR:-default}` (default if unset OR empty)
//! - `${VAR-default}` (default if unset only)
//! - `${VAR:?error}` (error if unset OR empty)
//! - `${VAR?error}` (error if unset only)
//! - `${VAR:+alt}` (alt if set AND non-empty)
//! - `${VAR+alt}` (alt if set, even if empty)
//! - `$$` escape (literal `$`)

use std::collections::HashMap;
use std::hash::BuildHasher;

use thiserror::Error;

/// Errors raised during variable interpolation.
#[derive(Debug, Error)]
pub enum InterpolateError {
    /// A `${VAR:?msg}` or `${VAR?msg}` form was used and the variable was
    /// considered missing.
    #[error("required variable `{name}` is missing: {message}")]
    Required {
        /// Name of the missing variable.
        name: String,
        /// User-supplied diagnostic message.
        message: String,
    },

    /// A `${...}` block was opened but never closed.
    #[error("unterminated `${{...}}` block in input")]
    UnterminatedBrace,

    /// A `${...}` block contained an empty variable name.
    #[error("empty variable name in `${{...}}` block")]
    EmptyName,

    /// A variable name contained an invalid character.
    #[error("invalid character `{ch}` in variable name `{name}`")]
    InvalidName {
        /// The offending name.
        name: String,
        /// The character that broke validation.
        ch: char,
    },
}

/// Recursively interpolate variables in a YAML value.
///
/// Strings are expanded in place; sequences and mappings are walked. Mapping
/// keys that are strings are also expanded (Docker Compose itself does not
/// require this, but interpolating keys keeps round-tripping predictable).
///
/// # Errors
///
/// Returns the first [`InterpolateError`] encountered in any contained string.
pub fn interpolate_yaml_value<S: BuildHasher>(
    value: &mut serde_yaml::Value,
    env: &HashMap<String, String, S>,
) -> Result<(), InterpolateError> {
    match value {
        serde_yaml::Value::String(s) => {
            let expanded = interpolate_string(s, env)?;
            *s = expanded;
        }
        serde_yaml::Value::Sequence(seq) => {
            for item in seq {
                interpolate_yaml_value(item, env)?;
            }
        }
        serde_yaml::Value::Mapping(map) => {
            // We can't mutate keys in place via iter_mut, so rebuild.
            let entries: Vec<(serde_yaml::Value, serde_yaml::Value)> =
                map.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            map.clear();
            for (mut k, mut v) in entries {
                interpolate_yaml_value(&mut k, env)?;
                interpolate_yaml_value(&mut v, env)?;
                map.insert(k, v);
            }
        }
        // Tagged values are rare in compose; recurse into the inner value.
        serde_yaml::Value::Tagged(tagged) => {
            interpolate_yaml_value(&mut tagged.value, env)?;
        }
        // Null, Bool, Number — nothing to interpolate.
        _ => {}
    }
    Ok(())
}

/// Expand all `$VAR` / `${VAR...}` references in `input`.
///
/// `$$` is an escape that produces a literal `$`.
///
/// # Errors
///
/// Returns an error if a required variable is missing, a `${...}` block is
/// malformed, or a variable name contains invalid characters.
pub fn interpolate_string<S: BuildHasher>(
    input: &str,
    env: &HashMap<String, String, S>,
) -> Result<String, InterpolateError> {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        let c = bytes[i];
        if c != b'$' {
            // Multi-byte UTF-8 safe: copy this codepoint as-is.
            let ch_len = utf8_char_len(c);
            out.push_str(&input[i..i + ch_len]);
            i += ch_len;
            continue;
        }

        // c == '$'
        if i + 1 >= bytes.len() {
            // Trailing '$' with nothing after — Docker treats this as literal.
            out.push('$');
            i += 1;
            continue;
        }

        let next = bytes[i + 1];
        if next == b'$' {
            // Escape: $$ -> $
            out.push('$');
            i += 2;
            continue;
        }

        if next == b'{' {
            // Find matching closing brace. Compose syntax does not support
            // nested braces in the interior of an expression, so a simple
            // scan suffices.
            let Some(close) = find_closing_brace(bytes, i + 2) else {
                return Err(InterpolateError::UnterminatedBrace);
            };
            let inner = &input[i + 2..close];
            let expanded = expand_braced(inner, env)?;
            out.push_str(&expanded);
            i = close + 1;
            continue;
        }

        if is_name_start(next) {
            // Bare $NAME form — read max-munch identifier.
            let start = i + 1;
            let mut end = start;
            while end < bytes.len() && is_name_cont(bytes[end]) {
                end += 1;
            }
            let name = &input[start..end];
            if let Some(v) = env.get(name) {
                out.push_str(v);
            }
            // Unset bare variable expands to empty string (compose behaviour).
            i = end;
            continue;
        }

        // `$` followed by something that doesn't start a name (e.g. `$ `,
        // `$1`, `$-`). Compose-spec behaviour: keep the `$` literal.
        out.push('$');
        i += 1;
    }

    Ok(out)
}

/// Length in bytes of the UTF-8 codepoint that begins with `lead`.
fn utf8_char_len(lead: u8) -> usize {
    if lead < 0x80 {
        1
    } else if lead < 0xC0 {
        // Continuation byte encountered as a leading byte — invalid UTF-8,
        // but `&str` guarantees this won't happen. Fall back to 1.
        1
    } else if lead < 0xE0 {
        2
    } else if lead < 0xF0 {
        3
    } else {
        4
    }
}

fn is_name_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

fn is_name_cont(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn find_closing_brace(bytes: &[u8], start: usize) -> Option<usize> {
    let mut i = start;
    while i < bytes.len() {
        if bytes[i] == b'}' {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Expand the contents of `${...}` (without the braces).
///
/// Forms (checked in order):
/// - `NAME:-default`, `NAME-default`
/// - `NAME:?msg`, `NAME?msg`
/// - `NAME:+alt`, `NAME+alt`
/// - `NAME`
fn expand_braced<S: BuildHasher>(
    inner: &str,
    env: &HashMap<String, String, S>,
) -> Result<String, InterpolateError> {
    if inner.is_empty() {
        return Err(InterpolateError::EmptyName);
    }

    // Find the operator (first occurrence of `:-`, `:?`, `:+`, `-`, `?`, `+`)
    // after a valid name prefix.
    let bytes = inner.as_bytes();
    let mut name_end = 0;
    while name_end < bytes.len() && is_name_cont(bytes[name_end]) {
        name_end += 1;
    }

    if name_end == 0 {
        return Err(InterpolateError::EmptyName);
    }

    let name = &inner[..name_end];
    // Validate: name must start with letter/underscore.
    if !is_name_start(bytes[0]) {
        // First char wasn't a valid start (e.g. starts with a digit).
        return Err(InterpolateError::InvalidName {
            name: name.to_string(),
            ch: inner.chars().next().unwrap_or('?'),
        });
    }

    if name_end == inner.len() {
        // Plain ${NAME} — empty string for unset, value otherwise.
        return Ok(env.get(name).cloned().unwrap_or_default());
    }

    let rest = &inner[name_end..];
    let rest_bytes = rest.as_bytes();

    // Two-char operators first.
    if rest_bytes.len() >= 2 && rest_bytes[0] == b':' {
        let op = rest_bytes[1];
        let arg = &rest[2..];
        return match op {
            b'-' => Ok(apply_default(env.get(name), arg, true)),
            b'?' => apply_required(name, env.get(name), arg, true),
            b'+' => Ok(apply_alternate(env.get(name), arg, true)),
            _ => Err(InterpolateError::InvalidName {
                name: name.to_string(),
                ch: ':',
            }),
        };
    }

    // Single-char operators.
    let op = rest_bytes[0];
    let arg = &rest[1..];
    match op {
        b'-' => Ok(apply_default(env.get(name), arg, false)),
        b'?' => apply_required(name, env.get(name), arg, false),
        b'+' => Ok(apply_alternate(env.get(name), arg, false)),
        other => Err(InterpolateError::InvalidName {
            name: name.to_string(),
            ch: other as char,
        }),
    }
}

/// `${VAR:-default}` (`treat_empty=true`) or `${VAR-default}` (`treat_empty=false`).
fn apply_default(value: Option<&String>, default: &str, treat_empty_as_unset: bool) -> String {
    match value {
        Some(v) if treat_empty_as_unset && v.is_empty() => default.to_string(),
        Some(v) => v.clone(),
        None => default.to_string(),
    }
}

/// `${VAR:?msg}` (`treat_empty=true`) or `${VAR?msg}` (`treat_empty=false`).
fn apply_required(
    name: &str,
    value: Option<&String>,
    message: &str,
    treat_empty_as_unset: bool,
) -> Result<String, InterpolateError> {
    match value {
        Some(v) if treat_empty_as_unset && v.is_empty() => Err(InterpolateError::Required {
            name: name.to_string(),
            message: message.to_string(),
        }),
        Some(v) => Ok(v.clone()),
        None => Err(InterpolateError::Required {
            name: name.to_string(),
            message: message.to_string(),
        }),
    }
}

/// `${VAR:+alt}` (`require_non_empty=true`) or `${VAR+alt}` (`require_non_empty=false`).
fn apply_alternate(value: Option<&String>, alt: &str, require_non_empty: bool) -> String {
    match value {
        Some(v) if require_non_empty && v.is_empty() => String::new(),
        Some(_) => alt.to_string(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn bare_dollar_var() {
        let e = env(&[("FOO", "bar")]);
        assert_eq!(interpolate_string("$FOO", &e).unwrap(), "bar");
        assert_eq!(interpolate_string("x-$FOO-y", &e).unwrap(), "x-bar-y");
    }

    #[test]
    fn braced_var() {
        let e = env(&[("FOO", "bar")]);
        assert_eq!(interpolate_string("${FOO}", &e).unwrap(), "bar");
        assert_eq!(interpolate_string("a${FOO}b", &e).unwrap(), "abarb");
    }

    #[test]
    fn default_unset_or_empty() {
        // ${VAR:-default}
        let e_empty = env(&[("X", "")]);
        let e_set = env(&[("X", "hello")]);
        let e_none = env(&[]);
        assert_eq!(interpolate_string("${X:-fb}", &e_empty).unwrap(), "fb");
        assert_eq!(interpolate_string("${X:-fb}", &e_none).unwrap(), "fb");
        assert_eq!(interpolate_string("${X:-fb}", &e_set).unwrap(), "hello");
    }

    #[test]
    fn default_unset_only() {
        // ${VAR-default}
        let e_empty = env(&[("X", "")]);
        let e_set = env(&[("X", "hello")]);
        let e_none = env(&[]);
        assert_eq!(interpolate_string("${X-fb}", &e_empty).unwrap(), "");
        assert_eq!(interpolate_string("${X-fb}", &e_none).unwrap(), "fb");
        assert_eq!(interpolate_string("${X-fb}", &e_set).unwrap(), "hello");
    }

    #[test]
    fn required_unset_or_empty() {
        // ${VAR:?msg}
        let e_empty = env(&[("X", "")]);
        let e_set = env(&[("X", "hello")]);
        let e_none = env(&[]);
        assert!(matches!(
            interpolate_string("${X:?missing}", &e_empty),
            Err(InterpolateError::Required { .. })
        ));
        assert!(matches!(
            interpolate_string("${X:?missing}", &e_none),
            Err(InterpolateError::Required { .. })
        ));
        assert_eq!(
            interpolate_string("${X:?missing}", &e_set).unwrap(),
            "hello"
        );
    }

    #[test]
    fn required_unset_only() {
        // ${VAR?msg}
        let e_empty = env(&[("X", "")]);
        let e_none = env(&[]);
        assert_eq!(interpolate_string("${X?missing}", &e_empty).unwrap(), "");
        let err = interpolate_string("${X?missing}", &e_none).unwrap_err();
        match err {
            InterpolateError::Required { name, message } => {
                assert_eq!(name, "X");
                assert_eq!(message, "missing");
            }
            other => panic!("expected Required, got {other:?}"),
        }
    }

    #[test]
    fn alternate_set_non_empty() {
        // ${VAR:+alt}
        let e_empty = env(&[("X", "")]);
        let e_set = env(&[("X", "v")]);
        let e_none = env(&[]);
        assert_eq!(interpolate_string("${X:+alt}", &e_empty).unwrap(), "");
        assert_eq!(interpolate_string("${X:+alt}", &e_set).unwrap(), "alt");
        assert_eq!(interpolate_string("${X:+alt}", &e_none).unwrap(), "");
    }

    #[test]
    fn alternate_set_only() {
        // ${VAR+alt}
        let e_empty = env(&[("X", "")]);
        let e_set = env(&[("X", "v")]);
        let e_none = env(&[]);
        assert_eq!(interpolate_string("${X+alt}", &e_empty).unwrap(), "alt");
        assert_eq!(interpolate_string("${X+alt}", &e_set).unwrap(), "alt");
        assert_eq!(interpolate_string("${X+alt}", &e_none).unwrap(), "");
    }

    #[test]
    fn dollar_dollar_escape() {
        let e = env(&[("FOO", "bar")]);
        assert_eq!(interpolate_string("$$FOO", &e).unwrap(), "$FOO");
        assert_eq!(interpolate_string("price: $$5", &e).unwrap(), "price: $5");
        assert_eq!(interpolate_string("$$$FOO", &e).unwrap(), "$bar");
    }

    #[test]
    fn unset_bare_expands_empty() {
        let e = env(&[]);
        assert_eq!(interpolate_string("[$MISSING]", &e).unwrap(), "[]");
        assert_eq!(interpolate_string("[${MISSING}]", &e).unwrap(), "[]");
    }

    #[test]
    fn unterminated_brace() {
        let e = env(&[]);
        assert!(matches!(
            interpolate_string("${FOO", &e),
            Err(InterpolateError::UnterminatedBrace)
        ));
    }

    #[test]
    fn empty_name_in_braces() {
        let e = env(&[]);
        assert!(matches!(
            interpolate_string("${}", &e),
            Err(InterpolateError::EmptyName)
        ));
    }

    #[test]
    fn invalid_name_starts_with_digit() {
        let e = env(&[]);
        assert!(matches!(
            interpolate_string("${1FOO}", &e),
            Err(InterpolateError::InvalidName { .. })
        ));
    }

    #[test]
    fn dollar_with_non_name_kept_literal() {
        let e = env(&[]);
        assert_eq!(interpolate_string("$ price", &e).unwrap(), "$ price");
        assert_eq!(interpolate_string("price$", &e).unwrap(), "price$");
        assert_eq!(interpolate_string("$1", &e).unwrap(), "$1");
    }

    #[test]
    fn multi_var_in_one_string() {
        let e = env(&[("A", "1"), ("B", "2")]);
        assert_eq!(interpolate_string("$A-${B}-${C:-3}", &e).unwrap(), "1-2-3");
    }

    #[test]
    fn name_with_underscore_and_digits() {
        let e = env(&[("FOO_BAR_2", "ok")]);
        assert_eq!(interpolate_string("$FOO_BAR_2", &e).unwrap(), "ok");
        assert_eq!(interpolate_string("${FOO_BAR_2}", &e).unwrap(), "ok");
    }

    #[test]
    fn utf8_passthrough() {
        let e = env(&[("X", "v")]);
        assert_eq!(
            interpolate_string("héllo $X wörld", &e).unwrap(),
            "héllo v wörld"
        );
    }

    #[test]
    fn yaml_value_recurses_strings() {
        let e = env(&[("HOST", "example.com"), ("PORT", "8080")]);
        let yaml = "url: http://${HOST}:${PORT}/\nlist:\n  - $HOST\n  - ${PORT}\n";
        let mut v: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
        interpolate_yaml_value(&mut v, &e).unwrap();
        let map = v.as_mapping().unwrap();
        assert_eq!(
            map.get(serde_yaml::Value::String("url".into()))
                .unwrap()
                .as_str()
                .unwrap(),
            "http://example.com:8080/"
        );
        let list = map
            .get(serde_yaml::Value::String("list".into()))
            .unwrap()
            .as_sequence()
            .unwrap();
        assert_eq!(list[0].as_str().unwrap(), "example.com");
        assert_eq!(list[1].as_str().unwrap(), "8080");
    }

    #[test]
    fn yaml_value_does_not_touch_non_strings() {
        let e = env(&[]);
        let yaml = "count: 5\nflag: true\nratio: 1.5\nnothing: ~\n";
        let mut v: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
        interpolate_yaml_value(&mut v, &e).unwrap();
        let map = v.as_mapping().unwrap();
        assert_eq!(
            map.get(serde_yaml::Value::String("count".into()))
                .and_then(serde_yaml::Value::as_i64),
            Some(5)
        );
        assert_eq!(
            map.get(serde_yaml::Value::String("flag".into()))
                .and_then(serde_yaml::Value::as_bool),
            Some(true)
        );
        assert!(map
            .get(serde_yaml::Value::String("nothing".into()))
            .unwrap()
            .is_null());
    }

    #[test]
    fn yaml_value_propagates_required_error() {
        let e = env(&[]);
        let yaml = "url: ${MISSING:?need it}\n";
        let mut v: serde_yaml::Value = serde_yaml::from_str(yaml).unwrap();
        let err = interpolate_yaml_value(&mut v, &e).unwrap_err();
        assert!(matches!(err, InterpolateError::Required { .. }));
    }
}
