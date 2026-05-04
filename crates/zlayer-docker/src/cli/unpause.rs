//! Docker `unpause` command.
//!
//! Implements `docker unpause CONTAINER [CONTAINER...]` by forwarding each
//! container ID/name to the local zlayer daemon via [`DaemonClient`].

use anyhow::Context;
use clap::Parser;
use zlayer_client::{default_socket_path, DaemonClient};

/// Arguments for `docker unpause`.
#[derive(Debug, Parser)]
pub struct UnpauseArgs {
    /// Container name(s) or ID(s) to resume.
    pub containers: Vec<String>,
}

/// Handle the `docker unpause` command.
///
/// # Errors
///
/// Returns the first daemon error after attempting all targets.
pub async fn handle_unpause(args: UnpauseArgs) -> anyhow::Result<()> {
    tracing::info!("docker unpause: containers={:?}", args.containers);

    if args.containers.is_empty() {
        anyhow::bail!("at least one container name or ID is required");
    }

    let client = DaemonClient::connect_to(default_socket_path())
        .await
        .context("Failed to connect to zlayer daemon")?;

    let mut first_err: Option<anyhow::Error> = None;
    for id in &args.containers {
        match client.unpause_container(id).await {
            Ok(()) => println!("{id}"),
            Err(e) => {
                eprintln!("Error response from daemon: {e}");
                if first_err.is_none() {
                    first_err = Some(e.context(format!("Failed to unpause container '{id}'")));
                }
            }
        }
    }
    if let Some(e) = first_err {
        return Err(e);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_single_positional_arg() {
        let args = UnpauseArgs::try_parse_from(["unpause", "foo"]).unwrap();
        assert_eq!(args.containers, vec!["foo"]);
    }

    #[test]
    fn rejects_missing_args() {
        // unpause with no positional arg is a no-op at clap level, but the
        // handler bails. The clap-level bail keeps the test focused on parse.
        let args = UnpauseArgs::try_parse_from(["unpause"]).unwrap();
        assert!(args.containers.is_empty());
    }
}
