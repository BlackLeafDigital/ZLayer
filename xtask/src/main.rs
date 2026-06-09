//! `xtask` — workspace task runner for `ZLayer`.
//!
//! Invoked through the `cargo xtask` alias (configured in
//! `.cargo/config.toml`). Wraps repetitive maintenance / CI tasks so
//! contributors can run them with a single command and discover them
//! through `cargo xtask --help` rather than chasing shell scripts.
//!
//! Subcommands:
//!
//! * `ci` — `fmt --check`, `clippy`, `check`, `test` across the workspace.
//! * `win-check` — rsync the working tree to the Windows runner and run
//!   `cargo check` + `cargo clippy` for both the default and
//!   `hcs-runtime,wsl` composite feature builds. Thin wrapper around
//!   `scripts/windows-remote-check.sh`.

use std::env;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const USAGE_EXIT_CODE: u8 = 2;
const SIGNAL_EXIT_CODE: u8 = 1;

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    let Some(sub) = args.next() else {
        print_usage_to_stderr();
        return ExitCode::from(USAGE_EXIT_CODE);
    };

    let rest: Vec<String> = args.collect();
    match sub.as_str() {
        "-h" | "--help" | "help" => {
            print_usage_to_stdout();
            ExitCode::SUCCESS
        }
        "ci" => run_ci(),
        "win-check" => run_win_check(&rest),
        unknown => {
            eprintln!("xtask: unknown subcommand `{unknown}`");
            print_usage_to_stderr();
            ExitCode::from(USAGE_EXIT_CODE)
        }
    }
}

fn print_usage_to_stderr() {
    eprintln!("{}", usage_text());
}

fn print_usage_to_stdout() {
    println!("{}", usage_text());
}

fn usage_text() -> &'static str {
    "\
Usage: cargo xtask <SUBCOMMAND>

Subcommands:
  ci          Run fmt --check, clippy, check, and test across the workspace.
  win-check   Run cargo check + clippy on the MiniWindows host via SSH
              (rsync + remote cargo). Requires ZLAYER_WIN_HOST in the env,
              e.g. ZLAYER_WIN_HOST=MiniWindows@192.168.68.92.

Flags:
  -h, --help    Show this message.
"
}

/// Local CI sequence — same set the pre-commit hook runs.
fn run_ci() -> ExitCode {
    let steps: &[(&str, &[&str])] = &[
        ("cargo fmt --all --check", &["fmt", "--all", "--check"]),
        (
            "cargo clippy --workspace --all-targets -- -D warnings",
            &[
                "clippy",
                "--workspace",
                "--all-targets",
                "--",
                "-D",
                "warnings",
            ],
        ),
        (
            "cargo check --workspace --all-targets",
            &["check", "--workspace", "--all-targets"],
        ),
        ("cargo test --workspace", &["test", "--workspace"]),
    ];

    for (label, argv) in steps {
        eprintln!("xtask ci: running `{label}`");
        let code = run_cargo(argv.iter().copied());
        if code != 0 {
            eprintln!("xtask ci: step failed: `{label}` (exit {code})");
            return ExitCode::from(clamp_exit_code(code));
        }
    }

    eprintln!("CI green");
    ExitCode::SUCCESS
}

/// Run the Windows remote-check script (rsync + cargo check/clippy on
/// `MiniWindows`). Forwards any extra arguments to the shell script.
///
/// The script lives at `scripts/windows-remote-check.sh` in the
/// workspace root and is the authoritative interactive equivalent of
/// the CI `check-windows` job (see `docs/windows-ci-runner.md`).
fn run_win_check(extra_args: &[String]) -> ExitCode {
    let root = match workspace_root() {
        Ok(r) => r,
        Err(err) => {
            eprintln!("xtask win-check: {err}");
            return ExitCode::from(SIGNAL_EXIT_CODE);
        }
    };
    let script = root.join("scripts").join("windows-remote-check.sh");
    if !script.exists() {
        eprintln!(
            "xtask win-check: script not found at {} — has it been removed?",
            script.display()
        );
        return ExitCode::from(SIGNAL_EXIT_CODE);
    }

    if env::var_os("ZLAYER_WIN_HOST").is_none() {
        eprintln!(
            "xtask win-check: ZLAYER_WIN_HOST is required \
             (e.g. ZLAYER_WIN_HOST=MiniWindows@192.168.68.92)."
        );
        return ExitCode::from(USAGE_EXIT_CODE);
    }

    eprintln!("xtask win-check: invoking {}", script.display());
    let status = Command::new("bash").arg(&script).args(extra_args).status();
    match status {
        Ok(s) if s.success() => ExitCode::SUCCESS,
        Ok(s) => {
            let code = exit_code_of(s);
            eprintln!("xtask win-check: failed (exit {code})");
            ExitCode::from(clamp_exit_code(code))
        }
        Err(err) => {
            eprintln!("xtask win-check: failed to spawn bash: {err}");
            ExitCode::from(SIGNAL_EXIT_CODE)
        }
    }
}

fn run_cargo<I, S>(args: I) -> i32
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let cargo = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    match Command::new(cargo).args(args).status() {
        Ok(status) => exit_code_of(status),
        Err(err) => {
            eprintln!("xtask: failed to spawn cargo: {err}");
            i32::from(SIGNAL_EXIT_CODE)
        }
    }
}

fn workspace_root() -> Result<PathBuf, String> {
    let manifest_dir = env::var_os("CARGO_MANIFEST_DIR")
        .ok_or_else(|| "CARGO_MANIFEST_DIR is not set (run via `cargo xtask`)".to_string())?;
    let path = Path::new(&manifest_dir);
    path.parent().map(Path::to_path_buf).ok_or_else(|| {
        format!(
            "CARGO_MANIFEST_DIR `{}` has no parent directory",
            path.display()
        )
    })
}

fn exit_code_of(status: std::process::ExitStatus) -> i32 {
    status.code().unwrap_or(i32::from(SIGNAL_EXIT_CODE))
}

fn clamp_exit_code(code: i32) -> u8 {
    let truncated = code & 0xFF;
    if truncated == 0 {
        SIGNAL_EXIT_CODE
    } else {
        u8::try_from(truncated).unwrap_or(SIGNAL_EXIT_CODE)
    }
}
