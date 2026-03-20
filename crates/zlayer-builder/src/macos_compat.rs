//! macOS compatibility layer for translating Linux shell commands.
//!
//! When building container images on macOS via the sandbox builder, Dockerfile
//! `RUN` instructions are executed directly on the host. Linux-specific commands
//! (package managers, user management) must be translated to macOS equivalents.
//!
//! This module is only compiled on macOS (`#[cfg(target_os = "macos")]`).

use tracing::debug;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Result of translating a Linux command to macOS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranslatedCommand {
    /// The translated command string ready for execution.
    pub command: String,
    /// Human-readable log of what translations were applied.
    pub translations: Vec<String>,
    /// Whether the command was modified at all.
    pub was_modified: bool,
}

/// Translate a Linux shell command string to a macOS-compatible equivalent.
///
/// Handles compound commands (`&&`, `||`, `;`), Linux package managers,
/// user/group management, and apt/apk cleanup patterns.
#[must_use]
pub fn translate_linux_command(cmd: &str) -> TranslatedCommand {
    let segments = split_compound_command(cmd);

    if segments.is_empty() {
        return TranslatedCommand {
            command: cmd.to_string(),
            translations: vec![],
            was_modified: false,
        };
    }

    let mut translated_parts: Vec<String> = Vec::new();
    let mut translations: Vec<String> = Vec::new();
    let mut was_modified = false;

    for seg in &segments {
        let trimmed = seg.command.trim();
        let (translated, log_msg) = translate_segment(trimmed);

        if let Some(msg) = log_msg {
            translations.push(msg);
            was_modified = true;
        }

        if seg.separator.is_empty() {
            translated_parts.push(translated);
        } else {
            translated_parts.push(format!("{} {}", translated, seg.separator));
        }
    }

    let command = translated_parts.join(" ");

    TranslatedCommand {
        command,
        translations,
        was_modified,
    }
}

// ---------------------------------------------------------------------------
// Compound command splitting
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct CommandSegment {
    command: String,
    separator: String, // "&&", "||", ";", or "" for last
}

/// Split a shell command on `&&`, `||`, and `;` delimiters, respecting quotes.
fn split_compound_command(cmd: &str) -> Vec<CommandSegment> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let chars: Vec<char> = cmd.chars().collect();
    let len = chars.len();
    let mut i = 0;
    let mut in_single_quote = false;
    let mut in_double_quote = false;

    while i < len {
        let ch = chars[i];

        // Handle quote state
        if ch == '\'' && !in_double_quote {
            in_single_quote = !in_single_quote;
            current.push(ch);
            i += 1;
            continue;
        }
        if ch == '"' && !in_single_quote {
            in_double_quote = !in_double_quote;
            current.push(ch);
            i += 1;
            continue;
        }

        // Skip escaped characters
        if ch == '\\' && i + 1 < len {
            current.push(ch);
            current.push(chars[i + 1]);
            i += 2;
            continue;
        }

        // Only split when not inside quotes
        if !in_single_quote && !in_double_quote {
            // Check for &&
            if ch == '&' && i + 1 < len && chars[i + 1] == '&' {
                segments.push(CommandSegment {
                    command: current.trim().to_string(),
                    separator: "&&".to_string(),
                });
                current.clear();
                i += 2;
                continue;
            }
            // Check for ||
            if ch == '|' && i + 1 < len && chars[i + 1] == '|' {
                segments.push(CommandSegment {
                    command: current.trim().to_string(),
                    separator: "||".to_string(),
                });
                current.clear();
                i += 2;
                continue;
            }
            // Check for ;
            if ch == ';' {
                segments.push(CommandSegment {
                    command: current.trim().to_string(),
                    separator: ";".to_string(),
                });
                current.clear();
                i += 1;
                continue;
            }
        }

        current.push(ch);
        i += 1;
    }

    // Push the last segment
    let trimmed = current.trim().to_string();
    if !trimmed.is_empty() || !segments.is_empty() {
        segments.push(CommandSegment {
            command: trimmed,
            separator: String::new(),
        });
    }

    segments
}

// ---------------------------------------------------------------------------
// Per-segment translation
// ---------------------------------------------------------------------------

/// Translate a single command segment. Returns (`translated_cmd`, optional log message).
fn translate_segment(segment: &str) -> (String, Option<String>) {
    if segment.is_empty() {
        return ("true".to_string(), None);
    }

    // Strip leading `sudo` if present
    let stripped = segment
        .strip_prefix("sudo ")
        .unwrap_or(segment)
        .trim_start();

    // Get the first word (the binary name)
    let first_word = stripped.split_whitespace().next().unwrap_or("");

    // Package manager translations
    match first_word {
        "apk" => return translate_apk(stripped),
        "apt-get" | "apt" => return translate_apt_get(stripped),
        "yum" | "dnf" => return translate_yum_dnf(stripped),
        _ => {}
    }

    // User/group management — skip entirely
    if is_user_group_command(first_word) {
        return (
            "true".to_string(),
            Some(format!("Skipped user/group command: {segment}")),
        );
    }

    // apt/apk cleanup patterns
    if is_package_cache_cleanup(segment) {
        return (
            "true".to_string(),
            Some(format!("Skipped cache cleanup: {segment}")),
        );
    }

    // No translation needed
    (segment.to_string(), None)
}

/// Check if a command is a user/group management command.
fn is_user_group_command(first_word: &str) -> bool {
    matches!(
        first_word,
        "adduser" | "addgroup" | "useradd" | "groupadd" | "usermod" | "deluser" | "delgroup"
    )
}

/// Check if a command is cleaning up package manager caches.
fn is_package_cache_cleanup(cmd: &str) -> bool {
    // rm -rf /var/lib/apt/lists/* or /var/cache/apk/*
    if cmd.contains("/var/lib/apt") || cmd.contains("/var/cache/apk") {
        let trimmed = cmd.trim();
        if trimmed.starts_with("rm ") {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Package manager translators
// ---------------------------------------------------------------------------

/// Translate `apk` commands.
fn translate_apk(cmd: &str) -> (String, Option<String>) {
    let args: Vec<&str> = cmd.split_whitespace().collect();
    if args.len() < 2 {
        return ("true".to_string(), Some(format!("Skipped bare apk: {cmd}")));
    }

    match args[1] {
        "add" => {
            let packages = extract_packages_after_flags(&args[2..]);
            brew_install_from(cmd, &packages)
        }
        "update" | "upgrade" | "cache" => (
            "true".to_string(),
            Some(format!("Skipped `apk {}` (no macOS equivalent)", args[1])),
        ),
        "del" | "remove" => (
            "true".to_string(),
            Some(format!("Skipped `apk {}`", args[1])),
        ),
        _ => (cmd.to_string(), None),
    }
}

/// Translate `apt-get` / `apt` commands.
fn translate_apt_get(cmd: &str) -> (String, Option<String>) {
    let args: Vec<&str> = cmd.split_whitespace().collect();
    if args.len() < 2 {
        return (
            "true".to_string(),
            Some(format!("Skipped bare apt-get: {cmd}")),
        );
    }

    match args[1] {
        "install" => {
            let packages = extract_packages_after_flags(&args[2..]);
            brew_install_from(cmd, &packages)
        }
        "update" | "upgrade" | "clean" | "autoremove" | "autoclean" | "purge" | "remove" => (
            "true".to_string(),
            Some(format!(
                "Skipped `{} {}` (no macOS equivalent)",
                args[0], args[1]
            )),
        ),
        _ => (cmd.to_string(), None),
    }
}

/// Translate `yum` / `dnf` commands.
fn translate_yum_dnf(cmd: &str) -> (String, Option<String>) {
    let args: Vec<&str> = cmd.split_whitespace().collect();
    if args.len() < 2 {
        return (
            "true".to_string(),
            Some(format!("Skipped bare {}: {cmd}", args[0])),
        );
    }

    match args[1] {
        "install" => {
            let packages = extract_packages_after_flags(&args[2..]);
            brew_install_from(cmd, &packages)
        }
        "update" | "upgrade" | "clean" | "remove" | "erase" => (
            "true".to_string(),
            Some(format!(
                "Skipped `{} {}` (no macOS equivalent)",
                args[0], args[1]
            )),
        ),
        _ => (cmd.to_string(), None),
    }
}

// ---------------------------------------------------------------------------
// Package name mapping
// ---------------------------------------------------------------------------

/// Extract package names from args, skipping flags (anything starting with `-`).
fn extract_packages_after_flags<'a>(args: &[&'a str]) -> Vec<&'a str> {
    args.iter()
        .copied()
        .filter(|a| !a.starts_with('-'))
        .collect()
}

/// Skip package install commands — toolchains are provisioned directly into
/// the rootfs by `macos_toolchain.rs`, so package manager invocations are
/// no-ops on macOS sandbox builds.
fn brew_install_from(original: &str, linux_packages: &[&str]) -> (String, Option<String>) {
    let log = format!(
        "Skipped package install (toolchain provisioned in rootfs): {original} [{}]",
        linux_packages.join(", ")
    );
    debug!("{}", log);
    ("true".to_string(), Some(log))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // All package manager commands become no-ops (toolchains provisioned in rootfs)

    #[test]
    fn test_simple_apk_add() {
        let result = translate_linux_command("apk add --no-cache git curl");
        assert!(result.was_modified);
        assert_eq!(result.command, "true");
    }

    #[test]
    fn test_apk_all_skipped() {
        let result =
            translate_linux_command("apk add --no-cache ca-certificates build-base musl-dev");
        assert!(result.was_modified);
        assert_eq!(result.command, "true");
    }

    #[test]
    fn test_apt_get_install() {
        let result = translate_linux_command(
            "apt-get install -y --no-install-recommends protobuf-compiler libseccomp-dev pkg-config cmake",
        );
        assert!(result.was_modified);
        assert_eq!(result.command, "true");
    }

    #[test]
    fn test_apt_get_update_install_chain() {
        let result = translate_linux_command(
            "apt-get update && apt-get install -y curl && rm -rf /var/lib/apt/lists/*",
        );
        assert!(result.was_modified);
        assert_eq!(result.command, "true && true && true");
    }

    #[test]
    fn test_user_group_management() {
        let result = translate_linux_command(
            "addgroup -S zarcrunner && adduser -S -G zarcrunner zarcrunner",
        );
        assert!(result.was_modified);
        assert_eq!(result.command, "true && true");
    }

    #[test]
    fn test_mixed_compound() {
        let result =
            translate_linux_command("apk add --no-cache git && mkdir -p /app && adduser -S app");
        assert!(result.was_modified);
        assert_eq!(result.command, "true && mkdir -p /app && true");
    }

    #[test]
    fn test_passthrough() {
        let result = translate_linux_command("echo hello && mkdir -p /app");
        assert!(!result.was_modified);
        assert_eq!(result.command, "echo hello && mkdir -p /app");
    }

    #[test]
    fn test_real_world_zlayer_node() {
        let result = translate_linux_command(
            "apt-get update && apt-get install -y --no-install-recommends containerd runc iptables iproute2 ca-certificates wireguard-tools curl && apt-get clean && rm -rf /var/lib/apt/lists/*",
        );
        assert!(result.was_modified);
        assert_eq!(result.command, "true && true && true && true");
    }

    #[test]
    fn test_semicolon_separator() {
        let result = translate_linux_command("apk update; apk add git");
        assert!(result.was_modified);
        assert_eq!(result.command, "true ; true");
    }

    #[test]
    fn test_sudo_stripped() {
        let result = translate_linux_command("sudo apt-get install -y curl");
        assert!(result.was_modified);
        assert_eq!(result.command, "true");
    }

    #[test]
    fn test_quoted_ampersand_not_split() {
        let result = translate_linux_command("echo \"foo && bar\"");
        assert!(!result.was_modified);
        assert_eq!(result.command, "echo \"foo && bar\"");
    }

    #[test]
    fn test_apk_update() {
        let result = translate_linux_command("apk update");
        assert!(result.was_modified);
        assert_eq!(result.command, "true");
    }

    #[test]
    fn test_yum_install() {
        let result = translate_linux_command("yum install -y git cmake");
        assert!(result.was_modified);
        assert_eq!(result.command, "true");
    }

    #[test]
    fn test_real_world_zarcrunner_zimage() {
        let result = translate_linux_command("apk add --no-cache git build-base");
        assert!(result.was_modified);
        assert_eq!(result.command, "true");
    }

    #[test]
    fn test_real_world_zarcrunner_runtime() {
        let result = translate_linux_command("apk add --no-cache ca-certificates git bash curl");
        assert!(result.was_modified);
        assert_eq!(result.command, "true");
    }

    #[test]
    fn test_real_world_zarcrunner_full() {
        let result = translate_linux_command(
            "apk add --no-cache ca-certificates git bash && addgroup -S zarcrunner && adduser -S -G zarcrunner -h /data zarcrunner",
        );
        assert!(result.was_modified);
        assert_eq!(result.command, "true && true && true");
    }

    #[test]
    fn test_openssl_dev_mapping() {
        let result = translate_linux_command("apt-get install -y libssl-dev");
        assert!(result.was_modified);
        assert_eq!(result.command, "true");
    }

    #[test]
    fn test_empty_command() {
        let result = translate_linux_command("");
        assert!(!result.was_modified);
    }

    #[test]
    fn test_or_separator() {
        let result = translate_linux_command("apk add git || apt-get install -y git");
        assert!(result.was_modified);
        assert_eq!(result.command, "true || true");
    }

    #[test]
    fn test_cache_cleanup_apk() {
        let result = translate_linux_command("rm -rf /var/cache/apk/*");
        assert!(result.was_modified);
        assert_eq!(result.command, "true");
    }

    #[test]
    fn test_npm_pip_not_translated() {
        let result = translate_linux_command("npm install && pip install flask");
        assert!(!result.was_modified);
        assert_eq!(result.command, "npm install && pip install flask");
    }

    #[test]
    fn test_go_mod_download_not_translated() {
        let result = translate_linux_command("go mod download");
        assert!(!result.was_modified);
        assert_eq!(result.command, "go mod download");
    }

    #[test]
    fn test_zlayer_web_builder() {
        let result = translate_linux_command(
            "apt-get update && apt-get install -y protobuf-compiler libseccomp-dev pkg-config cmake libssl-dev && rm -rf /var/lib/apt/lists/*",
        );
        assert!(result.was_modified);
        assert_eq!(result.command, "true && true && true");
    }
}
