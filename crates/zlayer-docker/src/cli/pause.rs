//! Docker `pause` command.
//!
//! Implements `docker pause CONTAINER [CONTAINER...]` by forwarding each
//! container ID/name to the local zlayer daemon via [`DaemonClient`].

use anyhow::Context;
use clap::Parser;
use zlayer_client::{default_socket_path, DaemonClient};

/// Arguments for `docker pause`.
#[derive(Debug, Parser)]
pub struct PauseArgs {
    /// Container name(s) or ID(s) to pause.
    pub containers: Vec<String>,
}

/// Handle the `docker pause` command.
///
/// # Errors
///
/// Returns the first daemon error after attempting all targets.
pub async fn handle_pause(args: PauseArgs) -> anyhow::Result<()> {
    tracing::info!("docker pause: containers={:?}", args.containers);

    if args.containers.is_empty() {
        anyhow::bail!("at least one container name or ID is required");
    }

    let client = DaemonClient::connect_to(default_socket_path())
        .await
        .context("Failed to connect to zlayer daemon")?;

    let mut first_err: Option<anyhow::Error> = None;
    for id in &args.containers {
        match client.pause_container(id).await {
            Ok(()) => println!("{id}"),
            Err(e) => {
                eprintln!("Error response from daemon: {e}");
                if first_err.is_none() {
                    first_err = Some(e.context(format!("Failed to pause container '{id}'")));
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
        let args = PauseArgs::try_parse_from(["pause", "foo"]).unwrap();
        assert_eq!(args.containers, vec!["foo"]);
    }

    #[test]
    fn parses_multiple_positional_args() {
        let args = PauseArgs::try_parse_from(["pause", "foo", "bar"]).unwrap();
        assert_eq!(args.containers, vec!["foo", "bar"]);
    }
}
