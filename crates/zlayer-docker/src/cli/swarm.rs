//! Docker `swarm` command — redirected to `ZLayer` native cluster commands.
//!
//! `ZLayer` ships a Raft-based cluster manager rather than Docker Swarm. Each
//! `docker swarm <verb>` subcommand prints a short redirect to the equivalent
//! `zlayer cluster <verb>` invocation and exits with status `0` so that
//! scripts running under `set -e` are not aborted.

use clap::Subcommand;

/// Subcommands of `docker swarm <verb>`.
///
/// Every variant accepts arbitrary trailing arguments so that users can paste
/// a full Docker command (with all its flags) without clap rejecting Docker's
/// flag syntax. The arguments are intentionally discarded — only the
/// redirect message is emitted.
#[derive(Debug, Subcommand)]
pub enum SwarmCommand {
    /// Initialize a swarm — redirected to `zlayer cluster init`.
    Init {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Join a swarm as a node and/or manager — redirected to `zlayer cluster join`.
    Join {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Leave the swarm — redirected to `zlayer cluster leave`.
    Leave {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Update the swarm — redirected to `zlayer cluster update`.
    Update {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Manage join tokens — redirected to `zlayer cluster join-token`.
    #[clap(name = "join-token")]
    JoinToken {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Manage the unlock key — redirected to `zlayer cluster unlock-key`.
    #[clap(name = "unlock-key")]
    UnlockKey {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Unlock the swarm — redirected to `zlayer cluster unlock`.
    Unlock {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Display and rotate the root CA — redirected to `zlayer cluster ca`.
    Ca {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

/// Handle a `docker swarm <verb>` invocation by printing a redirect to the
/// matching `zlayer cluster <verb>` command. Always returns `Ok(())`.
///
/// # Errors
///
/// This function never returns an error — it always succeeds with exit `0`
/// to keep `set -e` shell scripts running.
pub fn handle(cmd: &SwarmCommand) -> anyhow::Result<()> {
    let (docker_cmd, native) = match cmd {
        SwarmCommand::Init { .. } => ("docker swarm init", "zlayer cluster init"),
        SwarmCommand::Join { .. } => ("docker swarm join", "zlayer cluster join"),
        SwarmCommand::Leave { .. } => ("docker swarm leave", "zlayer cluster leave"),
        SwarmCommand::Update { .. } => ("docker swarm update", "zlayer cluster update"),
        SwarmCommand::JoinToken { .. } => ("docker swarm join-token", "zlayer cluster join-token"),
        SwarmCommand::UnlockKey { .. } => ("docker swarm unlock-key", "zlayer cluster unlock-key"),
        SwarmCommand::Unlock { .. } => ("docker swarm unlock", "zlayer cluster unlock"),
        SwarmCommand::Ca { .. } => ("docker swarm ca", "zlayer cluster ca"),
    };
    eprintln!("{docker_cmd} is handled by ZLayer's native cluster.");
    eprintln!("Run `{native}` instead.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Debug, Parser)]
    struct TestCli {
        #[clap(subcommand)]
        cmd: SwarmCommand,
    }

    #[test]
    fn parses_init_with_trailing_flags() {
        let cli = TestCli::try_parse_from(["swarm", "init", "--advertise-addr", "1.2.3.4"])
            .expect("parse should succeed");
        assert!(matches!(cli.cmd, SwarmCommand::Init { .. }));
    }

    #[test]
    fn handle_returns_ok_for_every_variant() {
        for cmd in [
            SwarmCommand::Init { args: vec![] },
            SwarmCommand::Join { args: vec![] },
            SwarmCommand::Leave { args: vec![] },
            SwarmCommand::Update { args: vec![] },
            SwarmCommand::JoinToken { args: vec![] },
            SwarmCommand::UnlockKey { args: vec![] },
            SwarmCommand::Unlock { args: vec![] },
            SwarmCommand::Ca { args: vec![] },
        ] {
            assert!(handle(&cmd).is_ok());
        }
    }
}
