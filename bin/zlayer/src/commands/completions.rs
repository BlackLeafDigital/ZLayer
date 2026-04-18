//! Shell completion script generation.
//!
//! Emits a completion script for the requested shell to stdout, built from
//! the live `clap` command tree so completions always match the current CLI.

use clap::CommandFactory;
use clap_complete::{generate, Shell};

use crate::cli::Cli;

/// Generate a shell completion script for `shell` and write it to stdout.
#[allow(clippy::unnecessary_wraps)]
pub fn run(shell: Shell) -> anyhow::Result<()> {
    let mut cmd = Cli::command();
    generate(shell, &mut cmd, "zlayer", &mut std::io::stdout());
    Ok(())
}
