//! Static validation of Windows package-manager usage in parsed Dockerfiles.
//!
//! The `nanoserver` Windows base image is intentionally minimal: it ships no
//! `PowerShell`, no `choco`, no `winget`, and only the bare `cmd.exe` shell.
//! Users unfamiliar with Windows-container constraints routinely write
//!
//! ```dockerfile
//! FROM mcr.microsoft.com/windows/nanoserver:ltsc2022
//! RUN choco install nginx -y
//! ```
//!
//! which then fails deep inside the backend with an unhelpful
//! `'choco' is not recognized as an internal or external command` error. This
//! module catches that case at parse time and emits an actionable error
//! pointing users at `servercore` (which has `PowerShell`) or a multi-stage
//! build where the package install happens in a `servercore` stage and the
//! artifacts are `COPY --from=...`'d into the final nanoserver stage.
//!
//! # Scope (first iteration)
//!
//! - Detects `choco` and `winget` used as the effective RUN command, handling:
//!   - Exec form: `RUN ["choco", "install", "nginx"]`
//!   - Shell form: `RUN choco install nginx`
//!   - Via `cmd /c`: `RUN cmd /c choco install nginx`
//!   - Via `PowerShell`: `RUN powershell -Command "choco install nginx"`
//! - Flags only when the stage's base image is `nanoserver`. `servercore`
//!   (which bundles `PowerShell`) and non-Windows bases are skipped.
//! - Multi-stage Dockerfiles are validated per stage; each stage's own base
//!   image drives its verdict. A `servercore` builder stage that runs `choco`
//!   and `COPY --from=builder`s into a final `nanoserver` stage is the
//!   recommended remediation and passes validation.
//!
//! Future iterations may auto-inject the multi-stage rewrite; for now the
//! validator's job is to detect + error clearly.

use thiserror::Error;

use crate::dockerfile::{Dockerfile, ImageRef, Instruction, ShellOrExec};

/// Errors surfaced by the Windows dependency validator.
#[derive(Debug, Error)]
pub enum DepsError {
    /// The stage's base image is `nanoserver` but a `RUN` instruction tries
    /// to invoke `choco` or `winget`, neither of which exist on nanoserver.
    #[error(
        "`{package_manager}` requires a Windows base image with PowerShell \
         (e.g. mcr.microsoft.com/windows/servercore:ltsc2022). The nanoserver \
         base image has no package manager. Change the FROM line to servercore, \
         or install dependencies in a separate `servercore`-based build stage \
         and COPY them into the final nanoserver stage. Offending RUN \
         instruction #{instruction_index}."
    )]
    ChocoOnNanoserver {
        /// Zero-based index of the offending `RUN` instruction within the
        /// stage's instruction list.
        instruction_index: usize,
        /// The package manager that was detected (`"choco"` or `"winget"`).
        package_manager: String,
    },
}

/// Windows package-manager tokens we care about.
///
/// These are matched case-insensitively against the effective command in a
/// `RUN` instruction (ignoring wrapping shells like `cmd /c` or
/// `powershell -Command`).
const WINDOWS_PACKAGE_MANAGERS: &[&str] = &["choco", "winget"];

/// Walk every stage in `dockerfile` and error if any `RUN` on a
/// `nanoserver`-based stage uses `choco` or `winget`.
///
/// Non-Windows base images (Linux, scratch, other stage refs) and
/// `servercore` bases are skipped — they either don't apply or are capable of
/// running the package managers in question.
///
/// # Errors
///
/// Returns [`DepsError::ChocoOnNanoserver`] on the first offending `RUN`
/// instruction encountered. First-match-wins: we do not accumulate multiple
/// errors because fixing the first one usually reveals the rest.
pub fn validate_dockerfile(dockerfile: &Dockerfile) -> Result<(), DepsError> {
    for stage in &dockerfile.stages {
        let base_kind = classify_base_image(&stage.base_image);

        // Only nanoserver needs this guard. Servercore has `PowerShell` +
        // package managers; non-Windows bases use apt/yum/apk/etc.
        if base_kind != WindowsBase::Nanoserver {
            continue;
        }

        for (idx, instr) in stage.instructions.iter().enumerate() {
            if let Instruction::Run(run) = instr {
                if let Some(pm) = detect_package_manager(&run.command) {
                    return Err(DepsError::ChocoOnNanoserver {
                        instruction_index: idx,
                        package_manager: pm.to_string(),
                    });
                }
            }
        }
    }

    Ok(())
}

/// Coarse classification of a stage's `FROM` image for this validator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WindowsBase {
    /// Image reference mentions `nanoserver` (case-insensitive).
    Nanoserver,
    /// Image reference mentions `servercore` (case-insensitive) or otherwise
    /// looks like a Windows base (starts with `windows/` or
    /// `mcr.microsoft.com/windows/...`). These have `PowerShell`.
    ServerCoreOrOtherWindows,
    /// Linux image, `scratch`, stage reference, or anything we don't
    /// recognise as Windows. Skip validation — `apt`/`yum`/`apk` are fine.
    NotWindows,
}

/// Inspect the raw base-image string of a stage and decide which bucket it
/// falls into.
///
/// Matches are substring-based + case-insensitive so all of these resolve to
/// `Nanoserver`:
/// - `mcr.microsoft.com/windows/nanoserver:ltsc2022`
/// - `mcr.microsoft.com/windows/nanoserver:ltsc2019`
/// - `nanoserver` (short form, sometimes used with local registries)
/// - `my-registry.example.com/MyProject/Nanoserver:latest` (odd-casing)
fn classify_base_image(base: &ImageRef) -> WindowsBase {
    let image_str = match base {
        ImageRef::Registry { image, .. } => image.to_ascii_lowercase(),
        // Stage refs and `scratch` are not Windows bases in their own right;
        // if a later stage copies from them, that's still fine.
        ImageRef::Stage(_) | ImageRef::Scratch => return WindowsBase::NotWindows,
    };

    if image_str.contains("nanoserver") {
        WindowsBase::Nanoserver
    } else if image_str.contains("servercore") || image_str.contains("windows/") {
        WindowsBase::ServerCoreOrOtherWindows
    } else {
        WindowsBase::NotWindows
    }
}

/// If the effective command in `cmd` is `choco` or `winget`, return which
/// one. Otherwise return `None`.
///
/// Handles the common shell-wrapping idioms so that
/// `powershell -Command "choco install nginx"`,
/// `cmd /c winget install ...`, and plain `choco install ...` all trip.
fn detect_package_manager(cmd: &ShellOrExec) -> Option<&'static str> {
    match cmd {
        ShellOrExec::Exec(args) => {
            // Exec form: first arg is the executable. Strip an explicit
            // `cmd`/`powershell` wrapper if present and look at the real
            // target.
            detect_in_tokens(args)
        }
        ShellOrExec::Shell(s) => {
            let tokens = tokenize_shell(s);
            detect_in_tokens(&tokens)
        }
    }
}

/// Given the already-tokenised argv-ish representation of a RUN command,
/// peel off known wrappers (`cmd /c`, `powershell -Command`, …) and match
/// the first surviving token against the package-manager allowlist.
fn detect_in_tokens<S: AsRef<str>>(tokens: &[S]) -> Option<&'static str> {
    let stripped: Vec<String> = strip_wrapper(tokens);
    let first: &str = stripped.first()?.as_str();
    let lower = first.to_ascii_lowercase();
    // Also strip a trailing `.exe` so `choco.exe install ...` trips.
    let normalised = lower.strip_suffix(".exe").unwrap_or(&lower);

    WINDOWS_PACKAGE_MANAGERS
        .iter()
        .find(|pm| normalised == **pm)
        .copied()
}

/// Remove a leading `cmd /c`, `cmd.exe /c`, `powershell -Command`,
/// `pwsh -Command`, etc. from a tokenised RUN command. Returns the slice
/// positioned at the effective command.
///
/// For `powershell -Command "choco install nginx"` we receive (after
/// tokenisation) `["powershell", "-Command", "choco install nginx"]` — that
/// third token is itself a shell-y string, so we re-tokenise and recurse.
/// The recursion is bounded because each layer strictly shortens the
/// argv list.
fn strip_wrapper<S: AsRef<str>>(tokens: &[S]) -> Vec<String> {
    if tokens.is_empty() {
        return Vec::new();
    }
    let head = tokens[0].as_ref().to_ascii_lowercase();
    let head = head.strip_suffix(".exe").unwrap_or(&head).to_string();

    // cmd [/s] /c <rest>
    if head == "cmd" {
        // Skip any /S, /Q, /V:ON style switches, stop on /c (or /k) which
        // introduces the payload.
        let mut i = 1;
        while i < tokens.len() {
            let t = tokens[i].as_ref().to_ascii_lowercase();
            if t == "/c" || t == "/k" {
                i += 1;
                break;
            }
            if t.starts_with('/') {
                i += 1;
                continue;
            }
            // Unrecognised — treat as start of payload.
            break;
        }
        if i >= tokens.len() {
            return Vec::new();
        }
        // The payload may be a single quoted string (joined) or multiple
        // separate argv tokens. If it's exactly one token and it contains a
        // space, re-tokenise it.
        let rest: Vec<String> = tokens[i..].iter().map(|s| s.as_ref().to_string()).collect();
        if rest.len() == 1 && rest[0].contains(char::is_whitespace) {
            return tokenize_shell(&rest[0]);
        }
        return rest;
    }

    // powershell / pwsh -Command <rest>   (also -c, -EncodedCommand variants
    // we don't try to decode).
    if head == "powershell" || head == "pwsh" {
        let mut i = 1;
        while i < tokens.len() {
            let t = tokens[i].as_ref().to_ascii_lowercase();
            if t == "-command" || t == "-c" {
                i += 1;
                break;
            }
            if t.starts_with('-') {
                // Skip flags like -NoProfile, -ExecutionPolicy, etc.
                // -ExecutionPolicy takes a value; consume it too.
                if t == "-executionpolicy" || t == "-file" {
                    i += 2;
                } else {
                    i += 1;
                }
                continue;
            }
            break;
        }
        if i >= tokens.len() {
            return Vec::new();
        }
        let rest: Vec<String> = tokens[i..].iter().map(|s| s.as_ref().to_string()).collect();
        if rest.len() == 1 && rest[0].contains(char::is_whitespace) {
            return tokenize_shell(&rest[0]);
        }
        return rest;
    }

    // No known wrapper — return tokens as-is.
    tokens.iter().map(|s| s.as_ref().to_string()).collect()
}

/// Very small shell tokenizer: splits on whitespace, respects single and
/// double quotes (no escape handling beyond `\"` / `\'`). Good enough to pull
/// the first executable out of a `RUN` shell-form string.
///
/// We don't need a full POSIX shell parser here: the only thing this function
/// has to get right is identifying the first real argv entry. If a
/// pathological RUN line slips through, the worst case is we miss a
/// `choco`/`winget` detection and the build later fails with the same
/// error the user sees today — i.e. we strictly improve on the status quo.
fn tokenize_shell(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut chars = input.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '\'' if !in_double => {
                in_single = !in_single;
            }
            '"' if !in_single => {
                in_double = !in_double;
            }
            '\\' if in_double => {
                // Preserve the escaped char verbatim if present.
                if let Some(&next) = chars.peek() {
                    current.push(next);
                    chars.next();
                } else {
                    current.push('\\');
                }
            }
            c if c.is_whitespace() && !in_single && !in_double => {
                if !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dockerfile::Dockerfile;

    fn parse(s: &str) -> Dockerfile {
        Dockerfile::parse(s).expect("test Dockerfile should parse")
    }

    #[test]
    fn nanoserver_plus_choco_errors() {
        let df = parse(
            r"
FROM mcr.microsoft.com/windows/nanoserver:ltsc2022
RUN choco install nginx -y
",
        );
        let err = validate_dockerfile(&df).expect_err("should flag choco on nanoserver");
        match err {
            DepsError::ChocoOnNanoserver {
                instruction_index,
                package_manager,
            } => {
                assert_eq!(instruction_index, 0);
                assert_eq!(package_manager, "choco");
            }
        }
    }

    #[test]
    fn nanoserver_plus_winget_errors() {
        let df = parse(
            r"
FROM mcr.microsoft.com/windows/nanoserver:ltsc2022
RUN winget install --id Git.Git
",
        );
        let err = validate_dockerfile(&df).expect_err("should flag winget on nanoserver");
        assert!(matches!(
            err,
            DepsError::ChocoOnNanoserver { ref package_manager, .. }
                if package_manager == "winget"
        ));
    }

    #[test]
    fn nanoserver_without_package_manager_is_ok() {
        let df = parse(
            r"
FROM mcr.microsoft.com/windows/nanoserver:ltsc2022
RUN cmd /c echo hello
COPY . C:\\app
",
        );
        validate_dockerfile(&df).expect("plain cmd /c echo should pass");
    }

    #[test]
    fn servercore_plus_choco_is_ok() {
        let df = parse(
            r"
FROM mcr.microsoft.com/windows/servercore:ltsc2022
RUN choco install nginx -y
",
        );
        validate_dockerfile(&df).expect("choco on servercore should pass");
    }

    #[test]
    fn servercore_plus_powershell_choco_is_ok() {
        let df = parse(
            r#"
FROM mcr.microsoft.com/windows/servercore:ltsc2022
RUN powershell -Command "choco install nginx -y"
"#,
        );
        validate_dockerfile(&df).expect("powershell-wrapped choco on servercore should pass");
    }

    #[test]
    fn nanoserver_plus_powershell_choco_errors() {
        // Nanoserver does not actually ship powershell, but the choco token
        // is what this validator is responsible for. We still flag it.
        let df = parse(
            r#"
FROM mcr.microsoft.com/windows/nanoserver:ltsc2022
RUN powershell -Command "choco install nginx -y"
"#,
        );
        let err = validate_dockerfile(&df)
            .expect_err("powershell-wrapped choco on nanoserver should still be flagged");
        assert!(matches!(
            err,
            DepsError::ChocoOnNanoserver { ref package_manager, .. }
                if package_manager == "choco"
        ));
    }

    #[test]
    fn linux_base_is_skipped() {
        let df = parse(
            r"
FROM alpine:3.19
RUN apk add --no-cache nginx
",
        );
        validate_dockerfile(&df).expect("alpine + apk has nothing to do with this validator");
    }

    #[test]
    fn multi_stage_servercore_then_nanoserver_is_ok() {
        // The canonical remediation: install tooling in a servercore stage,
        // COPY the resulting artifacts into a lean nanoserver runtime stage.
        let df = parse(
            r"
FROM mcr.microsoft.com/windows/servercore:ltsc2022 AS builder
RUN choco install nginx -y

FROM mcr.microsoft.com/windows/nanoserver:ltsc2022
COPY --from=builder C:\\nginx C:\\nginx
",
        );
        validate_dockerfile(&df).expect("multi-stage canonical pattern should pass");
    }

    #[test]
    fn nanoserver_cmd_c_choco_errors() {
        let df = parse(
            r"
FROM mcr.microsoft.com/windows/nanoserver:ltsc2022
RUN cmd /c choco install nginx -y
",
        );
        let err = validate_dockerfile(&df).expect_err("cmd /c wrapping choco should still trip");
        assert!(matches!(
            err,
            DepsError::ChocoOnNanoserver { ref package_manager, .. }
                if package_manager == "choco"
        ));
    }

    #[test]
    fn nanoserver_exec_form_winget_errors() {
        // Exec form: RUN ["winget", "install", ...]
        let df = parse(
            r#"
FROM mcr.microsoft.com/windows/nanoserver:ltsc2022
RUN ["winget", "install", "--id", "Git.Git"]
"#,
        );
        let err = validate_dockerfile(&df).expect_err("exec-form winget on nanoserver should trip");
        assert!(matches!(
            err,
            DepsError::ChocoOnNanoserver { ref package_manager, .. }
                if package_manager == "winget"
        ));
    }

    #[test]
    fn nanoserver_choco_exe_errors() {
        let df = parse(
            r"
FROM mcr.microsoft.com/windows/nanoserver:ltsc2022
RUN choco.exe install nginx -y
",
        );
        let err = validate_dockerfile(&df).expect_err("choco.exe should normalise to choco");
        assert!(matches!(
            err,
            DepsError::ChocoOnNanoserver { ref package_manager, .. }
                if package_manager == "choco"
        ));
    }

    #[test]
    fn nanoserver_reports_correct_instruction_index() {
        let df = parse(
            r"
FROM mcr.microsoft.com/windows/nanoserver:ltsc2022
COPY . C:\\app
RUN cmd /c echo build step
RUN choco install nginx -y
",
        );
        let err = validate_dockerfile(&df).expect_err("should flag third instruction");
        match err {
            DepsError::ChocoOnNanoserver {
                instruction_index, ..
            } => {
                // Instructions in the stage: COPY, RUN echo, RUN choco → idx 2.
                assert_eq!(instruction_index, 2);
            }
        }
    }

    #[test]
    fn error_message_points_at_servercore() {
        let df = parse(
            r"
FROM mcr.microsoft.com/windows/nanoserver:ltsc2022
RUN choco install nginx -y
",
        );
        let err = validate_dockerfile(&df).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("servercore"),
            "error message should steer users at servercore, got: {msg}"
        );
        assert!(
            msg.contains("COPY them into the final nanoserver stage"),
            "error message should mention the multi-stage remediation, got: {msg}"
        );
    }

    // ------------------------------------------------------------------
    // Tokenizer unit tests
    // ------------------------------------------------------------------

    #[test]
    fn tokenize_handles_double_quoted_payload() {
        let toks = tokenize_shell(r#"powershell -Command "choco install nginx -y""#);
        assert_eq!(toks.len(), 3);
        assert_eq!(toks[0], "powershell");
        assert_eq!(toks[1], "-Command");
        assert_eq!(toks[2], "choco install nginx -y");
    }

    #[test]
    fn tokenize_handles_single_quoted_payload() {
        let toks = tokenize_shell(r"cmd /c 'choco install nginx'");
        assert_eq!(toks, vec!["cmd", "/c", "choco install nginx"]);
    }

    #[test]
    fn strip_wrapper_peels_cmd_c() {
        let toks = vec!["cmd", "/c", "choco", "install", "nginx"];
        let stripped = strip_wrapper(&toks);
        assert_eq!(stripped, vec!["choco", "install", "nginx"]);
    }

    #[test]
    fn strip_wrapper_peels_powershell_joined_payload() {
        let toks = vec!["powershell", "-Command", "choco install nginx"];
        let stripped = strip_wrapper(&toks);
        assert_eq!(stripped, vec!["choco", "install", "nginx"]);
    }

    #[test]
    fn strip_wrapper_leaves_non_wrappers_alone() {
        let toks = vec!["apt-get", "install", "-y", "nginx"];
        let stripped = strip_wrapper(&toks);
        assert_eq!(stripped, vec!["apt-get", "install", "-y", "nginx"]);
    }
}
