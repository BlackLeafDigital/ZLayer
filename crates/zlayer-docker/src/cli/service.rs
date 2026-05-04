//! Docker `service` command — redirected to `zlayer deploy`.
//!
//! `ZLayer`'s deployment specs cover the same surface as Docker services. Each
//! `docker service <verb>` subcommand prints a redirect to the appropriate
//! `zlayer deploy` / `zlayer ps` / `zlayer logs` invocation.

use clap::Subcommand;

/// Subcommands of `docker service <verb>`.
#[derive(Debug, Subcommand)]
pub enum ServiceCommand {
    /// Create a service — redirected to `zlayer deploy`.
    Create {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Inspect a service — redirected to `zlayer status`.
    Inspect {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// List services — redirected to `zlayer ps`.
    Ls {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Remove a service — redirected to `zlayer stop`.
    Rm {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Scale a service — redirected to `zlayer deploy --scale`.
    Scale {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Update a service — redirected to `zlayer deploy`.
    Update {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Roll back a service — redirected to `zlayer deploy --rollback`.
    Rollback {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// List the tasks of a service — redirected to `zlayer ps`.
    Ps {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Fetch the logs of a service — redirected to `zlayer logs`.
    Logs {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

/// Handle a `docker service <verb>` invocation by printing a redirect.
///
/// # Errors
///
/// Never returns an error — always exits with status `0`.
pub fn handle(cmd: &ServiceCommand) -> anyhow::Result<()> {
    let (docker_cmd, native) = match cmd {
        ServiceCommand::Create { .. } => ("docker service create", "zlayer deploy <spec.yaml>"),
        ServiceCommand::Inspect { .. } => ("docker service inspect", "zlayer status <name>"),
        ServiceCommand::Ls { .. } => ("docker service ls", "zlayer ps"),
        ServiceCommand::Rm { .. } => ("docker service rm", "zlayer stop <name>"),
        ServiceCommand::Scale { .. } => {
            ("docker service scale", "zlayer deploy --scale <name>=<n>")
        }
        ServiceCommand::Update { .. } => ("docker service update", "zlayer deploy <spec.yaml>"),
        ServiceCommand::Rollback { .. } => {
            ("docker service rollback", "zlayer deploy --rollback <name>")
        }
        ServiceCommand::Ps { .. } => ("docker service ps", "zlayer ps <name>"),
        ServiceCommand::Logs { .. } => ("docker service logs", "zlayer logs <name>"),
    };
    eprintln!("{docker_cmd} is handled by ZLayer's native deploy/ps/logs commands.");
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
        cmd: ServiceCommand,
    }

    #[test]
    fn parses_create_with_trailing_flags() {
        let cli = TestCli::try_parse_from([
            "service",
            "create",
            "--name",
            "web",
            "--replicas",
            "3",
            "nginx",
        ])
        .expect("parse should succeed");
        assert!(matches!(cli.cmd, ServiceCommand::Create { .. }));
    }

    #[test]
    fn handle_returns_ok_for_every_variant() {
        for cmd in [
            ServiceCommand::Create { args: vec![] },
            ServiceCommand::Inspect { args: vec![] },
            ServiceCommand::Ls { args: vec![] },
            ServiceCommand::Rm { args: vec![] },
            ServiceCommand::Scale { args: vec![] },
            ServiceCommand::Update { args: vec![] },
            ServiceCommand::Rollback { args: vec![] },
            ServiceCommand::Ps { args: vec![] },
            ServiceCommand::Logs { args: vec![] },
        ] {
            assert!(handle(&cmd).is_ok());
        }
    }
}
