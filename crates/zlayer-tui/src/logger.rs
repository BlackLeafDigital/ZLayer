//! Color detection and ANSI formatting for plain-text loggers.
//!
//! Provides [`detect_color_support`] to check environment variables for
//! color capability, and [`colorize`] to conditionally wrap text in ANSI
//! escape codes.
//!
//! For the ANSI color constants themselves, use [`crate::palette::ansi`]
//! directly (e.g. `crate::palette::ansi::GREEN`).

use crate::palette::ansi;

/// Detect if the terminal supports ANSI colors.
///
/// Checks the following environment variables in order:
/// - `NO_COLOR` -- if set, colors are disabled (see <https://no-color.org>)
/// - `FORCE_COLOR` -- if set, colors are forced on
/// - `CI` -- CI environments typically support colors
/// - `TERM` -- colors are disabled only when set to `"dumb"`
///
/// Returns `false` when none of the above variables are set.
pub fn detect_color_support() -> bool {
    if std::env::var("NO_COLOR").is_ok() {
        return false;
    }
    if std::env::var("FORCE_COLOR").is_ok() {
        return true;
    }
    if std::env::var("CI").is_ok() {
        return true;
    }
    if let Ok(term) = std::env::var("TERM") {
        return term != "dumb";
    }
    false
}

/// Wrap `text` with an ANSI color code if `enabled` is true.
///
/// When `enabled` is `true`, returns `format!("{color}{text}{RESET}")`.
/// When `false`, returns a plain copy of the text with no escape codes.
///
/// # Example
///
/// ```
/// use zlayer_tui::logger::colorize;
/// use zlayer_tui::palette::ansi;
///
/// let green = colorize("ok", ansi::GREEN, true);
/// assert!(green.contains("\x1b[32m"));
///
/// let plain = colorize("ok", ansi::GREEN, false);
/// assert_eq!(plain, "ok");
/// ```
pub fn colorize(text: &str, color: &str, enabled: bool) -> String {
    if enabled {
        format!("{}{}{}", color, text, ansi::RESET)
    } else {
        text.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::palette::ansi;

    #[test]
    fn colorize_enabled_wraps_with_ansi() {
        let result = colorize("hello", ansi::GREEN, true);
        assert_eq!(result, format!("{}hello{}", ansi::GREEN, ansi::RESET));
    }

    #[test]
    fn colorize_disabled_returns_plain_text() {
        let result = colorize("hello", ansi::RED, false);
        assert_eq!(result, "hello");
    }

    #[test]
    fn colorize_empty_text() {
        let result = colorize("", ansi::CYAN, true);
        assert_eq!(result, format!("{}{}", ansi::CYAN, ansi::RESET));
    }

    #[test]
    fn detect_color_support_returns_bool() {
        // Primarily a smoke test -- we cannot easily control the
        // environment in unit tests, but this verifies the function
        // runs without panicking and returns a boolean.
        let _supports_color: bool = detect_color_support();
    }
}
