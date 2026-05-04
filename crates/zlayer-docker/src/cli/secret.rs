//! Docker `secret` command — redirected to `zlayer secret`.
//!
//! `ZLayer`'s secret manager covers the same surface as Docker swarm secrets.
//! Each `docker secret <verb>` prints a redirect to the matching
//! `zlayer secret <verb>` invocation.

use clap::Subcommand;

/// Subcommands of `docker secret <verb>`.
#[derive(Debug, Subcommand)]
pub enum SecretCommand {
    /// Create a secret — redirected to `zlayer secret create`.
    Create {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Inspect a secret — redirected to `zlayer secret inspect`.
    Inspect {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// List secrets — redirected to `zlayer secret ls`.
    Ls {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Remove one or more secrets — redirected to `zlayer secret rm`.
    Rm {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

/// Handle a `docker secret <verb>` invocation by printing a redirect.
///
/// # Errors
///
/// Never returns an error — always exits with status `0`.
pub fn handle(cmd: &SecretCommand) -> anyhow::Result<()> {
    let (docker_cmd, native) = match cmd {
        SecretCommand::Create { .. } => ("docker secret create", "zlayer secret create"),
        SecretCommand::Inspect { .. } => ("docker secret inspect", "zlayer secret inspect"),
        SecretCommand::Ls { .. } => ("docker secret ls", "zlayer secret ls"),
        SecretCommand::Rm { .. } => ("docker secret rm", "zlayer secret rm"),
    };
    eprintln!("{docker_cmd} is handled by ZLayer's native secret manager.");
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
        cmd: SecretCommand,
    }

    #[test]
    fn parses_create_with_trailing_flags() {
        let cli = TestCli::try_parse_from(["secret", "create", "my-secret", "--driver", "default"])
            .expect("parse should succeed");
        assert!(matches!(cli.cmd, SecretCommand::Create { .. }));
    }

    #[test]
    fn handle_returns_ok_for_every_variant() {
        for cmd in [
            SecretCommand::Create { args: vec![] },
            SecretCommand::Inspect { args: vec![] },
            SecretCommand::Ls { args: vec![] },
            SecretCommand::Rm { args: vec![] },
        ] {
            assert!(handle(&cmd).is_ok());
        }
    }
}
