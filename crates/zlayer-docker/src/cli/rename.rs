//! Docker `rename` command.
//!
//! Implements the Docker CLI's `docker rename CONTAINER NEW_NAME` semantics
//! by forwarding the call to the local zlayer daemon via [`DaemonClient`].
//! The daemon-side handler in turn calls the runtime's `rename_container`
//! method (e.g. bollard's `POST /containers/{id}/rename` for the Docker
//! runtime).

use anyhow::Context;
use clap::Parser;
use zlayer_client::{default_socket_path, DaemonClient};

/// Arguments for `docker rename`.
///
/// Docker's CLI takes two positional arguments: the existing container
/// name/ID and the new name. We mirror that exactly so the binary is a
/// drop-in replacement.
///
/// Example: `docker rename my-old-name my-new-name`.
#[derive(Debug, Parser)]
pub struct RenameArgs {
    /// Container name or ID to rename.
    pub old_name: String,

    /// New name to assign to the container.
    pub new_name: String,
}

/// Handle the `docker rename` command.
///
/// # Errors
///
/// Returns an error when the daemon connection fails or the rename is
/// rejected by the runtime (e.g. the new name is already in use, or the
/// container does not exist).
pub async fn handle_rename(args: RenameArgs) -> anyhow::Result<()> {
    tracing::info!("docker rename: old={} new={}", args.old_name, args.new_name);

    if args.new_name.trim().is_empty() {
        anyhow::bail!("new container name must not be empty");
    }

    let client = DaemonClient::connect_to(default_socket_path())
        .await
        .context("Failed to connect to zlayer daemon")?;

    client
        .rename_container(&args.old_name, &args.new_name)
        .await
        .with_context(|| {
            format!(
                "Failed to rename container '{}' to '{}'",
                args.old_name, args.new_name
            )
        })?;

    println!("{}", args.old_name);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    /// `RenameArgs` must accept exactly two positional arguments and
    /// expose them as `old_name` / `new_name`. This matches Docker's
    /// `docker rename CONTAINER NEW_NAME` invocation.
    #[test]
    fn parses_two_positional_arguments() {
        let args = RenameArgs::try_parse_from(["rename", "my-old-name", "my-new-name"]).unwrap();
        assert_eq!(args.old_name, "my-old-name");
        assert_eq!(args.new_name, "my-new-name");
    }

    /// Without arguments clap must reject the invocation; both positional
    /// arguments are required to mirror Docker's CLI behaviour.
    #[test]
    fn rejects_missing_arguments() {
        assert!(RenameArgs::try_parse_from(["rename"]).is_err());
        assert!(RenameArgs::try_parse_from(["rename", "only-one-name"]).is_err());
    }

    /// The clap-generated help / about strings should mention both arguments
    /// so `docker rename --help` is informative.
    #[test]
    fn cli_metadata_lists_both_args() {
        let cmd = RenameArgs::command();
        let names: Vec<&str> = cmd
            .get_arguments()
            .map(clap::Arg::get_id)
            .map(clap::Id::as_str)
            .collect();
        assert!(names.contains(&"old_name"));
        assert!(names.contains(&"new_name"));
    }
}
