//! Docker `PortBindings` -> `ZLayer` `PortMapping[]`.
//!
//! Docker's `HostConfig.PortBindings` is a `map<"<port>/<proto>", null | [HostBinding]>`
//! shape. Each map entry expands into zero or more [`PortMapping`] entries:
//! a missing/null/empty binding list emits a "container-only" mapping with no
//! host port (Docker's `EXPOSE`-equivalent), while a populated list emits one
//! [`PortMapping`] per host binding.

use std::collections::HashMap;

use zlayer_types::spec::{PortMapping, PortProtocol};

use crate::socket::types::container_create::PortBindingHost;

/// Default host IP used when Docker emits an empty string for `HostIp`.
///
/// Mirrors the default applied by `PortMapping`'s serde defaults.
const DEFAULT_HOST_IP: &str = "0.0.0.0";

/// Translate a Docker `PortBindings` map into a flat `Vec<PortMapping>`.
///
/// Unparseable port keys are silently dropped — Docker itself rejects them at
/// the daemon, but the translator is permissive so a partial body still
/// produces a usable spec. Unknown protocols (anything other than `tcp`/`udp`)
/// also fall through to TCP, matching `ZLayer`'s [`PortProtocol`] coverage.
#[must_use]
pub fn translate(
    port_bindings: &HashMap<String, Option<Vec<PortBindingHost>>>,
) -> Vec<PortMapping> {
    let mut out = Vec::new();
    for (key, bindings) in port_bindings {
        let (port_str, proto_str) = key.split_once('/').unwrap_or((key.as_str(), "tcp"));
        let Ok(container_port) = port_str.parse::<u16>() else {
            continue;
        };
        let protocol = match proto_str.to_ascii_lowercase().as_str() {
            "udp" => PortProtocol::Udp,
            // ZLayer doesn't currently model SCTP — fall back to TCP rather
            // than dropping the mapping, so the container still gets a
            // best-effort published port.
            _ => PortProtocol::Tcp,
        };

        match bindings {
            Some(list) if !list.is_empty() => {
                for binding in list {
                    let host_port = binding
                        .host_port
                        .as_deref()
                        .filter(|s| !s.is_empty())
                        .and_then(|s| s.parse::<u16>().ok());
                    let host_ip = binding
                        .host_ip
                        .clone()
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| DEFAULT_HOST_IP.to_string());
                    out.push(PortMapping {
                        host_port,
                        container_port,
                        protocol,
                        host_ip,
                    });
                }
            }
            _ => {
                // No host binding requested: emit a container-only mapping
                // (the Docker `EXPOSE` analogue).
                out.push(PortMapping {
                    host_port: None,
                    container_port,
                    protocol,
                    host_ip: DEFAULT_HOST_IP.to_string(),
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding(host_ip: &str, host_port: &str) -> PortBindingHost {
        PortBindingHost {
            host_ip: Some(host_ip.to_string()),
            host_port: Some(host_port.to_string()),
        }
    }

    #[test]
    fn translates_tcp_with_explicit_host_port() {
        let mut m = HashMap::new();
        m.insert("80/tcp".to_string(), Some(vec![binding("", "8080")]));

        let out = translate(&m);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].container_port, 80);
        assert_eq!(out[0].host_port, Some(8080));
        assert_eq!(out[0].protocol, PortProtocol::Tcp);
        assert_eq!(out[0].host_ip, "0.0.0.0");
    }

    #[test]
    fn translates_udp_with_explicit_host_ip() {
        let mut m = HashMap::new();
        m.insert(
            "53/udp".to_string(),
            Some(vec![binding("127.0.0.1", "5353")]),
        );

        let out = translate(&m);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].container_port, 53);
        assert_eq!(out[0].protocol, PortProtocol::Udp);
        assert_eq!(out[0].host_port, Some(5353));
        assert_eq!(out[0].host_ip, "127.0.0.1");
    }

    #[test]
    fn null_or_empty_bindings_emit_container_only_mapping() {
        let mut m = HashMap::new();
        m.insert("443/tcp".to_string(), None);
        m.insert("9000/tcp".to_string(), Some(vec![]));

        let mut out = translate(&m);
        out.sort_by_key(|p| p.container_port);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].container_port, 443);
        assert_eq!(out[0].host_port, None);
        assert_eq!(out[1].container_port, 9000);
        assert_eq!(out[1].host_port, None);
    }

    #[test]
    fn empty_host_port_string_means_ephemeral() {
        let mut m = HashMap::new();
        m.insert("80/tcp".to_string(), Some(vec![binding("0.0.0.0", "")]));

        let out = translate(&m);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].host_port, None);
        assert_eq!(out[0].host_ip, "0.0.0.0");
    }

    #[test]
    fn unparseable_port_key_is_dropped() {
        let mut m = HashMap::new();
        m.insert("not-a-port/tcp".to_string(), Some(vec![binding("", "80")]));
        m.insert("80/tcp".to_string(), Some(vec![binding("", "8080")]));

        let out = translate(&m);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].container_port, 80);
    }

    #[test]
    fn missing_protocol_segment_defaults_to_tcp() {
        let mut m = HashMap::new();
        m.insert("80".to_string(), Some(vec![binding("", "8080")]));

        let out = translate(&m);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].protocol, PortProtocol::Tcp);
    }

    #[test]
    fn multiple_host_bindings_per_key_expand() {
        let mut m = HashMap::new();
        m.insert(
            "80/tcp".to_string(),
            Some(vec![binding("0.0.0.0", "8080"), binding("::", "9090")]),
        );

        let out = translate(&m);
        assert_eq!(out.len(), 2);
        let ports: Vec<_> = out.iter().map(|p| p.host_port).collect();
        assert!(ports.contains(&Some(8080)));
        assert!(ports.contains(&Some(9090)));
    }
}
