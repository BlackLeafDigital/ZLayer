//! Y/n consent prompts for privileged side-effects (e.g. WSL2 auto-install).
//!
//! The WSL2 auto-install path needs to be gated behind an explicit user
//! decision because it triggers a UAC prompt and, on some SKUs, a reboot.
//! This module provides a tiny helper — deliberately smaller than pulling in
//! `dialoguer` for a single Y/n question — plus a CLI-facing
//! [`ConsentMode`] enum so callers can force a decision (`--install-wsl yes`
//! / `no`) or fall back to an interactive prompt (`--install-wsl ask`,
//! the default).
//!
//! All interactive I/O is routed through a [`BufRead`] / [`Write`] pair so
//! tests can feed canned input without touching stdin.

use std::io::{BufRead, IsTerminal, Write};

use clap::ValueEnum;

/// CLI-facing consent policy.
///
/// Wired up via `--install-wsl <yes|no|ask>` (and `ZLAYER_INSTALL_WSL=`) on
/// subcommands that may need to bring WSL2 online (`node init`, `node join`,
/// `join <url>`, `daemon install`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Default)]
#[clap(rename_all = "lower")]
pub enum ConsentMode {
    /// Interactive Y/n prompt on the controlling terminal. If stdin is not a
    /// TTY this returns an error pointing the user at `--install-wsl yes|no`.
    #[default]
    Ask,
    /// Auto-grant — equivalent to the user answering "Y" at the prompt.
    Yes,
    /// Auto-refuse — equivalent to the user answering "n" at the prompt.
    No,
}

/// Outcome of a consent prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsentDecision {
    /// User (or `--install-wsl yes`) approved the action.
    Granted,
    /// User (or `--install-wsl no`) declined the action.
    Refused,
}

/// Prompt text shown in interactive (`Ask`) mode.
const PROMPT_TEXT: &str = "\
ZLayer needs WSL2 to run Linux containers locally.
This will run `wsl --install --no-distribution` elevated (UAC prompt).
Install WSL2 now? [Y/n]: ";

/// Maximum number of invalid answers before we fall back to `Refused` in
/// interactive mode. Matches the "be polite, don't loop forever" policy from
/// other privileged confirmation helpers in this binary.
const MAX_REPROMPTS: usize = 3;

/// Ask the user whether to auto-install WSL2.
///
/// Reads from stdin when `mode` is [`ConsentMode::Ask`] and stdin is a TTY.
/// If stdin is **not** a TTY and the caller requested `Ask`, returns an
/// error telling the operator to pass `--install-wsl yes|no` (or set
/// `ZLAYER_INSTALL_WSL`) explicitly — we refuse to guess inside a
/// non-interactive context.
///
/// # Errors
///
/// - stdin is not a TTY but `mode == Ask`.
/// - Reading from stdin fails.
/// - Writing the prompt to stdout fails.
pub fn wsl2_install_consent(mode: ConsentMode) -> anyhow::Result<ConsentDecision> {
    match mode {
        ConsentMode::Yes => Ok(ConsentDecision::Granted),
        ConsentMode::No => Ok(ConsentDecision::Refused),
        ConsentMode::Ask => {
            if !std::io::stdin().is_terminal() {
                anyhow::bail!(
                    "WSL2 auto-install consent requested but stdin is not a terminal.\n\
                     Pass `--install-wsl yes` (or `--install-wsl no`) — or set the\n\
                     `ZLAYER_INSTALL_WSL` environment variable — to decide non-interactively."
                );
            }
            let stdin = std::io::stdin();
            let mut reader = stdin.lock();
            let mut writer = std::io::stdout();
            wsl2_install_consent_with_io(&mut reader, &mut writer)
        }
    }
}

/// Interactive variant used by [`wsl2_install_consent`] and tests.
///
/// Exposed with `pub(crate)` visibility so the unit tests in this file and a
/// future TUI-driven prompt can drive the Y/n loop with canned input without
/// touching real stdin/stdout.
///
/// # Errors
///
/// Propagates any I/O error from `reader` or `writer`.
pub(crate) fn wsl2_install_consent_with_io<R, W>(
    reader: &mut R,
    writer: &mut W,
) -> anyhow::Result<ConsentDecision>
where
    R: BufRead + ?Sized,
    W: Write + ?Sized,
{
    let mut attempts = 0usize;
    loop {
        writer.write_all(PROMPT_TEXT.as_bytes())?;
        writer.flush()?;

        let mut line = String::new();
        let bytes = reader.read_line(&mut line)?;

        // EOF — treat as a decline rather than looping forever.
        if bytes == 0 {
            return Ok(ConsentDecision::Refused);
        }

        let trimmed = line.trim();
        match trimmed {
            // Empty line (bare Enter) defaults to Yes, matching the "[Y/n]" capitalisation.
            "" | "y" | "Y" | "yes" | "YES" | "Yes" => {
                return Ok(ConsentDecision::Granted);
            }
            "n" | "N" | "no" | "NO" | "No" => {
                return Ok(ConsentDecision::Refused);
            }
            _ => {
                attempts += 1;
                if attempts >= MAX_REPROMPTS {
                    writeln!(
                        writer,
                        "Too many invalid responses — treating as 'no'. \
                         Re-run with `--install-wsl yes` to auto-accept."
                    )?;
                    writer.flush()?;
                    return Ok(ConsentDecision::Refused);
                }
                writeln!(writer, "Please answer 'y' or 'n'.")?;
                writer.flush()?;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn consent_mode_yes_grants_without_io() {
        let decision = wsl2_install_consent(ConsentMode::Yes).expect("yes is infallible");
        assert_eq!(decision, ConsentDecision::Granted);
    }

    #[test]
    fn consent_mode_no_refuses_without_io() {
        let decision = wsl2_install_consent(ConsentMode::No).expect("no is infallible");
        assert_eq!(decision, ConsentDecision::Refused);
    }

    #[test]
    fn ask_with_empty_input_defaults_to_granted() {
        let mut reader = Cursor::new(b"\n".to_vec());
        let mut writer = Vec::<u8>::new();
        let decision = wsl2_install_consent_with_io(&mut reader, &mut writer).unwrap();
        assert_eq!(decision, ConsentDecision::Granted);
        // Prompt should have been written.
        let out = String::from_utf8(writer).unwrap();
        assert!(out.contains("Install WSL2 now?"), "got: {out}");
    }

    #[test]
    fn ask_with_lowercase_y_grants() {
        let mut reader = Cursor::new(b"y\n".to_vec());
        let mut writer = Vec::<u8>::new();
        let decision = wsl2_install_consent_with_io(&mut reader, &mut writer).unwrap();
        assert_eq!(decision, ConsentDecision::Granted);
    }

    #[test]
    fn ask_with_uppercase_yes_grants() {
        let mut reader = Cursor::new(b"YES\n".to_vec());
        let mut writer = Vec::<u8>::new();
        let decision = wsl2_install_consent_with_io(&mut reader, &mut writer).unwrap();
        assert_eq!(decision, ConsentDecision::Granted);
    }

    #[test]
    fn ask_with_n_refuses() {
        let mut reader = Cursor::new(b"n\n".to_vec());
        let mut writer = Vec::<u8>::new();
        let decision = wsl2_install_consent_with_io(&mut reader, &mut writer).unwrap();
        assert_eq!(decision, ConsentDecision::Refused);
    }

    #[test]
    fn ask_with_no_word_refuses() {
        let mut reader = Cursor::new(b"No\n".to_vec());
        let mut writer = Vec::<u8>::new();
        let decision = wsl2_install_consent_with_io(&mut reader, &mut writer).unwrap();
        assert_eq!(decision, ConsentDecision::Refused);
    }

    #[test]
    fn ask_reprompts_on_garbage_then_accepts() {
        let mut reader = Cursor::new(b"maybe\n?\ny\n".to_vec());
        let mut writer = Vec::<u8>::new();
        let decision = wsl2_install_consent_with_io(&mut reader, &mut writer).unwrap();
        assert_eq!(decision, ConsentDecision::Granted);
        let out = String::from_utf8(writer).unwrap();
        // Reprompts should have surfaced.
        assert!(out.contains("Please answer"), "got: {out}");
    }

    #[test]
    fn ask_gives_up_after_too_many_invalid_answers() {
        // Three invalid lines → refused without ever asking for a fourth.
        let mut reader = Cursor::new(b"foo\nbar\nbaz\n".to_vec());
        let mut writer = Vec::<u8>::new();
        let decision = wsl2_install_consent_with_io(&mut reader, &mut writer).unwrap();
        assert_eq!(decision, ConsentDecision::Refused);
        let out = String::from_utf8(writer).unwrap();
        assert!(
            out.contains("Too many invalid responses"),
            "give-up message should be shown, got: {out}"
        );
    }

    #[test]
    fn ask_on_eof_refuses_gracefully() {
        let mut reader = Cursor::new(Vec::new());
        let mut writer = Vec::<u8>::new();
        let decision = wsl2_install_consent_with_io(&mut reader, &mut writer).unwrap();
        assert_eq!(decision, ConsentDecision::Refused);
    }
}
