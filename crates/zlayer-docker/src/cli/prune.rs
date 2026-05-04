//! Docker `container prune` command.
//!
//! Implements `docker container prune` by forwarding to the local zlayer
//! daemon's `POST /api/v1/containers/prune` endpoint. (Docker also exposes
//! this verb under `docker system prune` and `docker container prune`; this
//! shim implements the container-only sub-form.)

use anyhow::Context;
use clap::Parser;
use zlayer_client::{default_socket_path, DaemonClient};

/// Arguments for `docker container prune`.
#[derive(Debug, Parser)]
pub struct PruneArgs {
    /// Skip the confirmation prompt. Always set in non-interactive runs;
    /// included for parity with Docker's CLI.
    #[clap(short, long)]
    pub force: bool,
}

/// Handle the `docker container prune` command.
///
/// # Errors
///
/// Returns an error when the daemon connection or the prune sweep fails.
pub async fn handle_prune(args: PruneArgs) -> anyhow::Result<()> {
    tracing::info!("docker container prune: force={}", args.force);

    let client = DaemonClient::connect_to(default_socket_path())
        .await
        .context("Failed to connect to zlayer daemon")?;

    let body = client
        .prune_standalone_containers()
        .await
        .context("Failed to prune containers via daemon")?;

    let deleted = body
        .get("ContainersDeleted")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let space_reclaimed = body
        .get("SpaceReclaimed")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);

    if deleted.is_empty() {
        println!("Total reclaimed space: {space_reclaimed}B");
    } else {
        println!("Deleted Containers:");
        for id in &deleted {
            println!("{id}");
        }
        println!();
        println!("Total reclaimed space: {space_reclaimed}B");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_default() {
        let args = PruneArgs::try_parse_from(["prune"]).unwrap();
        assert!(!args.force);
    }

    #[test]
    fn parses_force_flag() {
        let args = PruneArgs::try_parse_from(["prune", "--force"]).unwrap();
        assert!(args.force);
    }
}
