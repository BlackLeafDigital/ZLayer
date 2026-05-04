//! Docker `top` command.
//!
//! Implements `docker top CONTAINER [PS_ARGS...]` by forwarding to the
//! local zlayer daemon's `GET /api/v1/containers/{id}/top?ps_args=` endpoint.

use anyhow::Context;
use clap::Parser;
use zlayer_client::{default_socket_path, DaemonClient};

/// Arguments for `docker top`.
#[derive(Debug, Parser)]
pub struct TopArgs {
    /// Container name or ID.
    pub container: String,

    /// Optional `ps`-style arguments. Forwarded to the runtime as a single
    /// space-joined string.
    #[clap(trailing_var_arg = true)]
    pub ps_args: Vec<String>,
}

/// Handle the `docker top` command.
///
/// # Errors
///
/// Returns an error if the daemon connection fails or the runtime cannot
/// list processes for the requested container.
pub async fn handle_top(args: TopArgs) -> anyhow::Result<()> {
    tracing::info!(
        "docker top: container={} ps_args={:?}",
        args.container,
        args.ps_args
    );

    let client = DaemonClient::connect_to(default_socket_path())
        .await
        .context("Failed to connect to zlayer daemon")?;

    let ps_args_joined = if args.ps_args.is_empty() {
        None
    } else {
        Some(args.ps_args.join(" "))
    };

    let body = client
        .top_container(&args.container, ps_args_joined.as_deref())
        .await
        .with_context(|| format!("Failed to list processes in '{}'", args.container))?;

    // Render a minimal table — Docker prints `Titles` as a header followed
    // by tab-separated process rows. Falls back to JSON when the body shape
    // is unexpected.
    let titles = body
        .get("Titles")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|t| t.as_str())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let processes = body
        .get("Processes")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|row| row.as_array())
                .map(|row| {
                    row.iter()
                        .filter_map(|cell| cell.as_str())
                        .map(str::to_string)
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if titles.is_empty() {
        println!("{}", serde_json::to_string_pretty(&body)?);
        return Ok(());
    }

    println!("{}", titles.join("\t"));
    for row in &processes {
        println!("{}", row.join("\t"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_container_only() {
        let args = TopArgs::try_parse_from(["top", "foo"]).unwrap();
        assert_eq!(args.container, "foo");
        assert!(args.ps_args.is_empty());
    }

    #[test]
    fn parses_container_with_ps_args() {
        let args = TopArgs::try_parse_from(["top", "foo", "aux"]).unwrap();
        assert_eq!(args.container, "foo");
        assert_eq!(args.ps_args, vec!["aux"]);
    }
}
