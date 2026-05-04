//! Docker `port` command.
//!
//! Implements `docker port CONTAINER [PRIVATE_PORT[/PROTO]]` by forwarding
//! the call to the local zlayer daemon's
//! `GET /api/v1/containers/{id}/port` endpoint.

use anyhow::Context;
use clap::Parser;
use zlayer_client::{default_socket_path, DaemonClient};

/// Arguments for `docker port`.
#[derive(Debug, Parser)]
pub struct PortArgs {
    /// Container name or ID.
    pub container: String,

    /// Optional `<port>[/<protocol>]` filter. When supplied, only the host
    /// bindings for that container port are printed.
    pub filter: Option<String>,
}

/// Handle the `docker port` command.
///
/// # Errors
///
/// Returns an error when the daemon connection fails or the lookup itself
/// fails.
pub async fn handle_port(args: PortArgs) -> anyhow::Result<()> {
    tracing::info!(
        "docker port: container={} filter={:?}",
        args.container,
        args.filter
    );

    let client = DaemonClient::connect_to(default_socket_path())
        .await
        .context("Failed to connect to zlayer daemon")?;

    let body = client
        .container_port(&args.container)
        .await
        .with_context(|| format!("Failed to fetch port mappings for '{}'", args.container))?;

    let ports = body.get("Ports").and_then(|v| v.as_object());

    // Resolve filter to a canonical "<port>/<proto>" key. Default protocol
    // is `tcp` when only a port number is supplied (matches Docker).
    let filter_key = args.filter.as_deref().map(|raw| {
        if raw.contains('/') {
            raw.to_string()
        } else {
            format!("{raw}/tcp")
        }
    });

    if let Some(map) = ports {
        let mut entries: Vec<(&String, &serde_json::Value)> = map.iter().collect();
        entries.sort_by(|a, b| a.0.cmp(b.0));
        for (key, bindings) in entries {
            if let Some(ref needle) = filter_key {
                if key != needle {
                    continue;
                }
            }
            let Some(arr) = bindings.as_array() else {
                continue;
            };
            for binding in arr {
                let host_ip = binding
                    .get("HostIp")
                    .and_then(|v| v.as_str())
                    .unwrap_or("0.0.0.0");
                let host_port = binding
                    .get("HostPort")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if filter_key.is_some() {
                    println!("{host_ip}:{host_port}");
                } else {
                    println!("{key} -> {host_ip}:{host_port}");
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_container_only() {
        let args = PortArgs::try_parse_from(["port", "foo"]).unwrap();
        assert_eq!(args.container, "foo");
        assert!(args.filter.is_none());
    }

    #[test]
    fn parses_container_with_filter() {
        let args = PortArgs::try_parse_from(["port", "foo", "80/tcp"]).unwrap();
        assert_eq!(args.container, "foo");
        assert_eq!(args.filter.as_deref(), Some("80/tcp"));
    }
}
