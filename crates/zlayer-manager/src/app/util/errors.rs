//! Shared error-display helpers.
//!
//! Server functions bubble up raw `ServerFnError` strings that often contain
//! developer-facing prefixes ("deserialise: …", "Create user failed (409):
//! …"). These helpers turn those into friendlier one-liners for the UI.

use leptos::prelude::ServerFnError;

/// Rewrite a raw upstream/server-fn error string into a UI-friendly message.
///
/// Pattern-matches on common substrings. Unknown strings pass through
/// unchanged — callers can surface them verbatim without hiding debugging
/// information.
#[must_use]
pub fn humanize_error(raw: &str) -> String {
    let lc = raw.to_ascii_lowercase();
    if lc.contains("conflict") || lc.contains("409") || lc.contains("bootstrap already") {
        "Bootstrap already completed. Please sign in at /login.".to_string()
    } else if lc.contains("bootstrap failed") {
        raw.to_string()
    } else if lc.contains("network") {
        "Could not reach the server. Check your connection and try again.".to_string()
    } else {
        raw.to_string()
    }
}

/// Humanize a `ServerFnError` the same way as [`humanize_error`].
///
/// Convenience wrapper so callers don't have to stringify the error inline.
#[must_use]
pub fn format_server_error(err: &ServerFnError) -> String {
    humanize_error(&err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conflict_is_mapped_to_bootstrap_notice() {
        assert_eq!(
            humanize_error("Conflict: bootstrap already completed"),
            "Bootstrap already completed. Please sign in at /login."
        );
    }

    #[test]
    fn http_409_is_mapped_to_bootstrap_notice() {
        assert_eq!(
            humanize_error("status 409 Conflict"),
            "Bootstrap already completed. Please sign in at /login."
        );
    }

    #[test]
    fn bootstrap_failed_passes_through_verbatim() {
        let raw = "bootstrap failed: invalid email";
        assert_eq!(humanize_error(raw), raw);
    }

    #[test]
    fn network_messages_are_generalised() {
        assert_eq!(
            humanize_error("network error: connection refused"),
            "Could not reach the server. Check your connection and try again."
        );
    }

    #[test]
    fn unknown_strings_pass_through() {
        assert_eq!(
            humanize_error("weird internal trouble"),
            "weird internal trouble"
        );
    }
}
