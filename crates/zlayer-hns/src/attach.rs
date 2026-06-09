//! High-level HCN attach primitives — create an endpoint on a network,
//! attach it to a fresh namespace, and tear everything down cleanly.
//!
//! The canonical 2026 flow is: create a `HostDefault` namespace, create
//! one endpoint per container on the target network, attach the endpoint
//! to the namespace via `HcnModifyNamespace`, then reference the namespace
//! GUID in the HCS compute-system document. This module wraps that idiom.
//!
//! # Ownership and orphans
//!
//! HCN has no built-in "owner" field for endpoints or namespaces, so we
//! tag ZLayer-created endpoints by prefixing their `Name` with an
//! `owner_tag` (e.g. `"zlayer"`). [`list_owned_endpoints`] uses that
//! prefix at agent startup to discover zombies left behind by a previous
//! crash; the caller cross-references the list against live containers
//! and calls [`delete_endpoint_and_namespace`] on each orphan they decide
//! to reap. The reconciliation policy itself lives in `zlayer-agent`, not
//! here — this module only supplies the scan + cleanup helpers.

#![allow(clippy::missing_errors_doc)]

use windows::core::GUID;

use crate::endpoint::{self, Endpoint};
use crate::error::{HnsError, HnsResult};
use crate::namespace::Namespace;
use crate::schema::{Dns, EndpointPolicy, HostComputeEndpoint, IpConfig, Route, SchemaVersion};

/// An active endpoint-in-namespace attachment. Keep it alive for the lifetime
/// of the container; call [`EndpointAttachment::teardown`] when removing the
/// container.
#[derive(Debug)]
pub struct EndpointAttachment {
    endpoint_id: GUID,
    namespace_id: GUID,
    ip: Option<String>,
}

impl EndpointAttachment {
    /// Create an endpoint on `network_id` with a **caller-chosen overlay IP**
    /// and the three Transparent-network policies required for cluster
    /// routing to work end-to-end:
    ///
    /// 1. `OutBoundNAT` with `Exceptions=[cluster_cidr]` — SNAT egress to the
    ///    host IP unless the destination is another container on the overlay.
    /// 2. `SDNRoute { DestinationPrefix=cluster_cidr, NeedEncap=false }` —
    ///    tells HCN that cluster-CIDR traffic must not be VXLAN-encapsulated
    ///    (we encapsulate via `WireGuard` on the host, not via HCN Overlay).
    /// 3. `ACL { Allow, In, RemoteAddresses=cluster_cidr }` — allows inbound
    ///    traffic from any other overlay container.
    ///
    /// The endpoint's `IpConfigurations` is pre-populated with `ip/prefix_length`
    /// so the caller doesn't rely on HCN's IPAM — the IP comes from the agent's
    /// per-node slice allocator.
    ///
    /// `container_id` is embedded in the endpoint name (prefixed with
    /// `owner_tag`) to match the existing [`list_owned_endpoints`] scan idiom.
    ///
    /// `dns_server` and `dns_domain` populate the endpoint's `Dns` schema
    /// field so Windows containers attached to this endpoint resolve overlay
    /// service names via the overlay hickory DNS server. Pass `None` for both
    /// to skip DNS configuration (legacy behavior — the endpoint inherits the
    /// network-level DNS config, or none if the network has no DNS either).
    ///
    /// Note: Windows containers always query DNS on port 53 — HNS does not
    /// support setting a custom DNS port on the endpoint. The overlay hickory
    /// server's canonical listener is on port 15353 (Linux), so a separate
    /// port-53 listener must be bound on the overlay IP for Windows
    /// containers to actually reach the DNS server. See
    /// `DnsServer::bind_windows_fallback` in `zlayer-overlay`.
    ///
    /// # Errors
    ///
    /// Returns any error from endpoint/namespace creation or attachment. On
    /// mid-way failure, best-effort cleanup removes partially-created
    /// resources before propagating the error.
    #[allow(clippy::too_many_arguments)]
    pub fn create_overlay(
        network_id: GUID,
        owner_tag: &str,
        container_id: &str,
        ip: std::net::IpAddr,
        prefix_length: u8,
        cluster_cidr: &str,
        dns_server: Option<std::net::IpAddr>,
        dns_domain: Option<&str>,
    ) -> HnsResult<Self> {
        let endpoint_id = GUID::new().map_err(|e| HnsError::Other {
            hresult: e.code().0,
            message: format!("GUID::new(endpoint): {e}"),
        })?;

        let namespace = Namespace::create_host_default()?;
        let namespace_id = namespace.id();

        let ip_str = ip.to_string();
        // Gateway for the endpoint's default route: the first address in the
        // per-node slice. For a /28 at 10.200.42.0/28, this is 10.200.42.1 —
        // conventionally the vSwitch's address on the Transparent network.
        let gateway = gateway_for_slice(ip, prefix_length);

        let dns = build_endpoint_dns(dns_server, dns_domain);

        let settings = HostComputeEndpoint {
            name: format!("{owner_tag}-{container_id}"),
            host_compute_network: format!("{network_id:?}"),
            schema_version: SchemaVersion::default(),
            ip_configurations: vec![IpConfig {
                ip_address: ip_str,
                prefix_length,
            }],
            policies: vec![
                EndpointPolicy::out_bound_nat(vec![cluster_cidr.to_string()]).into(),
                EndpointPolicy::sdn_route(cluster_cidr, false).into(),
                EndpointPolicy::acl_in_allow(cluster_cidr).into(),
            ],
            routes: vec![Route {
                next_hop: gateway.clone(),
                destination_prefix: "0.0.0.0/0".to_string(),
                metric: None,
            }],
            dns,
            ..HostComputeEndpoint::default()
        };

        let cleanup_namespace = |ns_id: GUID| {
            if let Err(e) = Namespace::delete(ns_id) {
                tracing::warn!(
                    ns = %format!("{ns_id:?}"),
                    error = %e,
                    "cleanup: failed to delete namespace after partial overlay attach",
                );
            }
        };

        let _endpoint = match Endpoint::create(network_id, endpoint_id, &settings) {
            Ok(e) => e,
            Err(err) => {
                cleanup_namespace(namespace_id);
                return Err(err);
            }
        };

        if let Err(err) = namespace.add_endpoint(endpoint_id) {
            if let Err(e2) = Endpoint::delete(endpoint_id) {
                tracing::warn!(
                    ep = %format!("{endpoint_id:?}"),
                    error = %e2,
                    "cleanup: failed to delete overlay endpoint after namespace-attach failure",
                );
            }
            cleanup_namespace(namespace_id);
            return Err(err);
        }

        Ok(Self {
            endpoint_id,
            namespace_id,
            ip: Some(ip.to_string()),
        })
    }

    /// Detach + delete both endpoint and namespace.
    ///
    /// HCN auto-detaches endpoints when they are deleted, so we skip an
    /// explicit `remove_endpoint` modify and rely on `HcnDeleteEndpoint`
    /// followed by `HcnDeleteNamespace`. Best-effort on partial failure:
    /// warns on each failure and returns the first error encountered,
    /// preferring the endpoint error since the endpoint is the primary
    /// resource holding kernel state.
    pub fn teardown(self) -> HnsResult<()> {
        let ep_res = Endpoint::delete(self.endpoint_id);
        if let Err(ref e) = ep_res {
            tracing::warn!(
                ep = %format!("{:?}", self.endpoint_id),
                error = %e,
                "teardown: HcnDeleteEndpoint failed",
            );
        }
        let ns_res = Namespace::delete(self.namespace_id);
        if let Err(ref e) = ns_res {
            tracing::warn!(
                ns = %format!("{:?}", self.namespace_id),
                error = %e,
                "teardown: HcnDeleteNamespace failed",
            );
        }
        ep_res.or(ns_res)
    }

    #[must_use]
    pub fn endpoint_id(&self) -> GUID {
        self.endpoint_id
    }

    #[must_use]
    pub fn namespace_id(&self) -> GUID {
        self.namespace_id
    }

    #[must_use]
    pub fn ip(&self) -> Option<&str> {
        self.ip.as_deref()
    }
}

/// Build an endpoint-level `Dns` struct for the HNS schema.
///
/// Returns `None` when both `dns_server` and `dns_domain` are absent —
/// preserves the legacy behavior where the endpoint inherits the
/// network-level DNS config (or none if the network has no DNS either).
///
/// When either is set, emits a `Dns` struct with:
/// - `server_list`: `[dns_server.to_string()]` if provided, else empty.
/// - `domain`: `dns_domain` if provided, else empty.
/// - `search`: a single entry equal to `dns_domain` when provided, so short
///   names (`svc-a`) resolve to `svc-a.<domain>` without the container
///   needing an explicit search list.
fn build_endpoint_dns(
    dns_server: Option<std::net::IpAddr>,
    dns_domain: Option<&str>,
) -> Option<Dns> {
    if dns_server.is_none() && dns_domain.is_none() {
        return None;
    }
    let server_list = dns_server
        .map(|ip| vec![ip.to_string()])
        .unwrap_or_default();
    let (domain, search) = match dns_domain {
        Some(d) => (d.to_string(), vec![d.to_string()]),
        None => (String::new(), Vec::new()),
    };
    Some(Dns {
        domain,
        search,
        server_list,
        options: Vec::new(),
    })
}

/// Compute the default gateway address for a slice, given an IP inside it and
/// its prefix length. Returns the network address + 1 (the first usable host
/// in the slice), which is the conventional vSwitch gateway for an HCN
/// Transparent subnet.
///
/// Examples:
/// - `10.200.42.5/28` → `10.200.42.1` (slice network `.0` + 1)
/// - `10.200.0.5/24` → `10.200.0.1`
fn gateway_for_slice(ip: std::net::IpAddr, prefix_length: u8) -> String {
    match ip {
        std::net::IpAddr::V4(v4) => {
            let bits = u32::from(v4);
            let mask = if prefix_length == 0 {
                0u32
            } else {
                !0u32 << (32 - u32::from(prefix_length))
            };
            let network = bits & mask;
            let gateway = network.wrapping_add(1);
            std::net::Ipv4Addr::from(gateway).to_string()
        }
        std::net::IpAddr::V6(v6) => {
            let bits = u128::from(v6);
            let mask = if prefix_length == 0 {
                0u128
            } else {
                !0u128 << (128 - u32::from(prefix_length))
            };
            let network = bits & mask;
            let gateway = network.wrapping_add(1);
            std::net::Ipv6Addr::from(gateway).to_string()
        }
    }
}

/// Scan every HCN endpoint on the host and return those whose `Name` starts
/// with `owner_tag`. Use at daemon startup to discover zombie endpoints left
/// behind by a previous crash.
///
/// This is a startup-only scan: it opens each endpoint in turn to read its
/// name property, so it is O(n) HCN round-trips in the number of endpoints
/// on the host. Not intended for the hot path.
///
/// Individual open/query failures are logged and skipped so a single corrupt
/// endpoint cannot block reconciliation.
pub fn list_owned_endpoints(owner_tag: &str) -> HnsResult<Vec<(GUID, String)>> {
    let guids = endpoint::list("{}")?;
    let mut found = Vec::new();
    for g in guids {
        match Endpoint::open(g) {
            Ok(ep) => match ep.query_properties("{}") {
                Ok(props) => {
                    if props.name.starts_with(owner_tag) {
                        found.push((g, props.name));
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        ep = %format!("{g:?}"),
                        error = %e,
                        "reconcile: query_properties failed; skipping",
                    );
                }
            },
            Err(e) => {
                tracing::warn!(
                    ep = %format!("{g:?}"),
                    error = %e,
                    "reconcile: open failed; skipping",
                );
            }
        }
    }
    Ok(found)
}

/// Delete an endpoint + namespace pair. Used by the agent to clean up an
/// orphan once it has decided the endpoint is stale. Best-effort: attempts
/// both deletes and returns the first error encountered (endpoint error
/// preferred, since it is the primary kernel-state holder).
pub fn delete_endpoint_and_namespace(endpoint_id: GUID, namespace_id: GUID) -> HnsResult<()> {
    let ep_res = Endpoint::delete(endpoint_id);
    if let Err(ref e) = ep_res {
        tracing::warn!(
            ep = %format!("{endpoint_id:?}"),
            error = %e,
            "reap: HcnDeleteEndpoint failed",
        );
    }
    let ns_res = Namespace::delete(namespace_id);
    if let Err(ref e) = ns_res {
        tracing::warn!(
            ns = %format!("{namespace_id:?}"),
            error = %e,
            "reap: HcnDeleteNamespace failed",
        );
    }
    ep_res.or(ns_res)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_tag_name_format_is_prefix_matchable() {
        // Verify the name format we emit in `create_overlay` is matchable by
        // `list_owned_endpoints`'s `starts_with` check. If this format ever
        // drifts, update both sides together.
        let owner = "zlayer";
        let cid = "abc123";
        let name = format!("{owner}-{cid}");
        assert!(name.starts_with(owner), "name must start with owner_tag");
        assert_eq!(name, "zlayer-abc123");
    }

    #[test]
    fn endpoint_attachment_getters_return_stored_values() {
        let ep = GUID::from_u128(0x1111_2222_3333_4444_5555_6666_7777_8888);
        let ns = GUID::from_u128(0xaaaa_bbbb_cccc_dddd_eeee_ffff_0000_1111);
        let att = EndpointAttachment {
            endpoint_id: ep,
            namespace_id: ns,
            ip: Some("10.0.0.42".to_string()),
        };
        assert_eq!(att.endpoint_id(), ep);
        assert_eq!(att.namespace_id(), ns);
        assert_eq!(att.ip(), Some("10.0.0.42"));
    }

    #[test]
    fn endpoint_attachment_ip_none_is_returned_as_none() {
        let att = EndpointAttachment {
            endpoint_id: GUID::zeroed(),
            namespace_id: GUID::zeroed(),
            ip: None,
        };
        assert!(att.ip().is_none());
    }

    #[test]
    fn gateway_for_slice_v4_28() {
        let ip: std::net::IpAddr = "10.200.42.5".parse().unwrap();
        assert_eq!(super::gateway_for_slice(ip, 28), "10.200.42.1");
    }

    #[test]
    fn gateway_for_slice_v4_24() {
        let ip: std::net::IpAddr = "10.200.7.42".parse().unwrap();
        assert_eq!(super::gateway_for_slice(ip, 24), "10.200.7.1");
    }

    #[test]
    fn gateway_for_slice_v4_16() {
        let ip: std::net::IpAddr = "10.200.42.5".parse().unwrap();
        assert_eq!(super::gateway_for_slice(ip, 16), "10.200.0.1");
    }

    #[test]
    fn gateway_for_slice_v4_endpoint_is_first_host() {
        // Gateway for .0 network address itself must still be .1.
        let ip: std::net::IpAddr = "10.200.42.0".parse().unwrap();
        assert_eq!(super::gateway_for_slice(ip, 28), "10.200.42.1");
        // And for .15 (broadcast of /28), still .1.
        let ip: std::net::IpAddr = "10.200.42.15".parse().unwrap();
        assert_eq!(super::gateway_for_slice(ip, 28), "10.200.42.1");
    }

    #[test]
    fn gateway_for_slice_v6_64() {
        let ip: std::net::IpAddr = "fd00:200:42::5".parse().unwrap();
        assert_eq!(super::gateway_for_slice(ip, 64), "fd00:200:42::1");
    }
}
