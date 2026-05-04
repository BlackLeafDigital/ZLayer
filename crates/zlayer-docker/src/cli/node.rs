//! Docker `node` command — redirected to `ZLayer` native cluster commands.
//!
//! Each `docker node <verb>` subcommand prints a redirect to the matching
//! `zlayer cluster nodes` / `zlayer cluster node` invocation and exits `0`.

use clap::Subcommand;

/// Subcommands of `docker node <verb>`.
#[derive(Debug, Subcommand)]
pub enum NodeCommand {
    /// List nodes — redirected to `zlayer cluster nodes`.
    Ls {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Inspect a node — redirected to `zlayer cluster node inspect`.
    Inspect {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Update a node — redirected to `zlayer cluster node update`.
    Update {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Remove one or more nodes — redirected to `zlayer cluster node rm`.
    Rm {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// List tasks running on a node — redirected to `zlayer cluster node ps`.
    Ps {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Demote a manager to worker — redirected to `zlayer cluster node demote`.
    Demote {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Promote a worker to manager — redirected to `zlayer cluster node promote`.
    Promote {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

/// Handle a `docker node <verb>` invocation by printing a redirect.
///
/// # Errors
///
/// Never returns an error — always exits with status `0`.
pub fn handle(cmd: &NodeCommand) -> anyhow::Result<()> {
    let (docker_cmd, native) = match cmd {
        NodeCommand::Ls { .. } => ("docker node ls", "zlayer cluster nodes"),
        NodeCommand::Inspect { .. } => ("docker node inspect", "zlayer cluster node inspect"),
        NodeCommand::Update { .. } => ("docker node update", "zlayer cluster node update"),
        NodeCommand::Rm { .. } => ("docker node rm", "zlayer cluster node rm"),
        NodeCommand::Ps { .. } => ("docker node ps", "zlayer cluster node ps"),
        NodeCommand::Demote { .. } => ("docker node demote", "zlayer cluster node demote"),
        NodeCommand::Promote { .. } => ("docker node promote", "zlayer cluster node promote"),
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
        cmd: NodeCommand,
    }

    #[test]
    fn parses_ls_with_trailing_flags() {
        let cli = TestCli::try_parse_from(["node", "ls", "--filter", "role=manager"])
            .expect("parse should succeed");
        assert!(matches!(cli.cmd, NodeCommand::Ls { .. }));
    }

    #[test]
    fn handle_returns_ok_for_every_variant() {
        for cmd in [
            NodeCommand::Ls { args: vec![] },
            NodeCommand::Inspect { args: vec![] },
            NodeCommand::Update { args: vec![] },
            NodeCommand::Rm { args: vec![] },
            NodeCommand::Ps { args: vec![] },
            NodeCommand::Demote { args: vec![] },
            NodeCommand::Promote { args: vec![] },
        ] {
            assert!(handle(&cmd).is_ok());
        }
    }
}
