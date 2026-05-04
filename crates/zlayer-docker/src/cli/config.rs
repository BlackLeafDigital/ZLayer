//! Docker `config` command — redirected to `zlayer variable`.
//!
//! `ZLayer` stores non-secret configuration via the `variable` subcommand.
//! Each `docker config <verb>` prints a redirect to the matching
//! `zlayer variable <verb>` invocation.

use clap::Subcommand;

/// Subcommands of `docker config <verb>`.
#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Create a config — redirected to `zlayer variable set`.
    Create {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Inspect a config — redirected to `zlayer variable get`.
    Inspect {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// List configs — redirected to `zlayer variable ls`.
    Ls {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Remove one or more configs — redirected to `zlayer variable rm`.
    Rm {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

/// Handle a `docker config <verb>` invocation by printing a redirect.
///
/// # Errors
///
/// Never returns an error — always exits with status `0`.
pub fn handle(cmd: &ConfigCommand) -> anyhow::Result<()> {
    let (docker_cmd, native) = match cmd {
        ConfigCommand::Create { .. } => ("docker config create", "zlayer variable set"),
        ConfigCommand::Inspect { .. } => ("docker config inspect", "zlayer variable get"),
        ConfigCommand::Ls { .. } => ("docker config ls", "zlayer variable ls"),
        ConfigCommand::Rm { .. } => ("docker config rm", "zlayer variable rm"),
    };
    eprintln!("{docker_cmd} is handled by ZLayer's native variable store.");
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
        cmd: ConfigCommand,
    }

    #[test]
    fn parses_create_with_trailing_flags() {
        let cli = TestCli::try_parse_from(["config", "create", "my-config", "./file.txt"])
            .expect("parse should succeed");
        assert!(matches!(cli.cmd, ConfigCommand::Create { .. }));
    }

    #[test]
    fn handle_returns_ok_for_every_variant() {
        for cmd in [
            ConfigCommand::Create { args: vec![] },
            ConfigCommand::Inspect { args: vec![] },
            ConfigCommand::Ls { args: vec![] },
            ConfigCommand::Rm { args: vec![] },
        ] {
            assert!(handle(&cmd).is_ok());
        }
    }
}
