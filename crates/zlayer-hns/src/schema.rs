//! Serde JSON types matching the `hcsshim/hcn` v2 schemas.
//!
//! The Windows Host Compute Network Service consumes/produces JSON for every
//! `Hcn*` call. These types mirror the Go definitions in
//! [microsoft/hcsshim](https://github.com/microsoft/hcsshim/tree/main/hcn)
//! closely enough to ser/de round-trip them without data loss, but only cover
//! the subset `ZLayer` Phase C needs (networks, endpoints, namespaces, stats,
//! and the modify-request envelopes).
//!
//! # `PascalCase` quirks
//!
//! HCN field names are *mostly* `PascalCase` but have two well-known
//! inconsistencies you must reproduce exactly or the API rejects the request:
//!
//! - `IPConfigurations` uses two capitals.
//! - `IpAddress` uses one.
//!
//! Both are handled via explicit `#[serde(rename = ...)]` below.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Schema version
// ---------------------------------------------------------------------------

/// HCN schema version envelope. Every top-level HCN object carries one.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SchemaVersion {
    #[serde(rename = "Major")]
    pub major: u32,
    #[serde(rename = "Minor")]
    pub minor: u32,
}

impl Default for SchemaVersion {
    /// HCN v2.0 — the Server 2019+ / Win11 baseline.
    fn default() -> Self {
        Self { major: 2, minor: 0 }
    }
}

// ---------------------------------------------------------------------------
// Network
// ---------------------------------------------------------------------------

/// Network type as enumerated by HCN. Serialized as the canonical string
/// (`"NAT"`, `"L2Bridge"`, ...).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum NetworkType {
    #[serde(rename = "NAT")]
    #[default]
    Nat,
    #[serde(rename = "Transparent")]
    Transparent,
    #[serde(rename = "L2Bridge")]
    L2Bridge,
    #[serde(rename = "L2Tunnel")]
    L2Tunnel,
    #[serde(rename = "Overlay")]
    Overlay,
    #[serde(rename = "Internal")]
    Internal,
    #[serde(rename = "Private")]
    Private,
    #[serde(rename = "ICS")]
    Ics,
}

/// Top-level HCN network object.
///
/// `#[serde(default)]` on the container makes deserialization tolerant of
/// arbitrary host networks (WSL, Default Switch, etc.) whose query response
/// omits fields like `SchemaVersion` or even `Name`. This affects the
/// **deserialize** side only — the create/serialize path still emits every
/// populated field (including `SchemaVersion`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct HostComputeNetwork {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>, // GUID string
    pub name: String,
    #[serde(rename = "Type")]
    pub ty: NetworkType,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub policies: Vec<serde_json::Value>, // network-level policies (passthrough)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mac_pool: Option<MacPool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dns: Option<Dns>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ipams: Vec<Ipam>,
    pub flags: u32,
    pub schema_version: SchemaVersion,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct MacPool {
    pub ranges: Vec<MacRange>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct MacRange {
    pub start_mac_address: String,
    pub end_mac_address: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Dns {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub domain: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub search: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub server_list: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Ipam {
    /// `"Static"` or `"DHCP"`. Defaults to `"Static"` when missing on the wire.
    #[serde(rename = "Type", default = "default_ipam_type")]
    pub ty: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subnets: Vec<Subnet>,
}

fn default_ipam_type() -> String {
    "Static".to_string()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Subnet {
    pub ip_address_prefix: String, // CIDR
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub routes: Vec<Route>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub policies: Vec<serde_json::Value>, // SubnetPolicy passthrough
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Route {
    pub next_hop: String,
    pub destination_prefix: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metric: Option<u32>,
}

// ---------------------------------------------------------------------------
// Endpoint
// ---------------------------------------------------------------------------

/// Top-level HCN endpoint object.
///
/// `#[serde(default)]` on the container makes deserialization tolerant of HCN
/// *query* responses that omit fields the create-time doc always carries
/// (`Name`, `HostComputeNetwork`, `SchemaVersion` are all absent from some
/// query-backs — e.g. an endpoint attached to a namespace on an Internal
/// network). This affects the **deserialize** side only; the create/serialize
/// path still emits every populated field. Mirrors [`HostComputeNetwork`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct HostComputeEndpoint {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub name: String,
    pub host_compute_network: String, // network GUID
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_compute_namespace: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub policies: Vec<serde_json::Value>,
    /// Serializes as `IPConfigurations` (two capitals). Matches hcsshim.
    #[serde(
        rename = "IPConfigurations",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub ip_configurations: Vec<IpConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dns: Option<Dns>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub routes: Vec<Route>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub mac_address: String,
    #[serde(default)]
    pub flags: u32,
    pub schema_version: SchemaVersion,
}

/// A single IPv4/IPv6 assignment on an endpoint.
///
/// Note: serializes as `IpAddress` (one cap) to match hcsshim exactly.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct IpConfig {
    pub ip_address: String,
    pub prefix_length: u8,
}

// ---------------------------------------------------------------------------
// Endpoint policies
// ---------------------------------------------------------------------------
//
// Source of truth: hcsshim/hcn/hcnpolicy.go (see
// <https://github.com/microsoft/hcsshim/blob/main/hcn/hcnpolicy.go>).
//
// Important wire-format notes (verified against hcsshim + Calico-Windows
// fixtures, April 2026):
//
// - The envelope is `{"Type": "...", "Settings": {...}}`. `Type` takes the
//   canonical camel/PascalCase spelling exactly as shown in `EndpointPolicyType`
//   below. Notably `OutBoundNAT` (not `OutboundNat`) and `SDNRoute` (not
//   `SDNROUTE`).
// - ACL addresses / ports / protocols are comma-separated strings, **not**
//   JSON arrays. e.g. `"Protocols": "6,17"` means TCP+UDP.
// - `HostComputeEndpoint.policies` stays typed as `Vec<serde_json::Value>` so
//   unknown/experimental policy types round-trip unmodified. The
//   `From<EndpointPolicy> for serde_json::Value` impl below lets callers push
//   typed values into that Vec without breaking the wire format.

/// Typed wrapper for a single HCN endpoint policy.
///
/// The Settings payload is `serde_json::Value` because each policy type has a
/// different settings shape; use the constructor methods
/// (`out_bound_nat`, `sdn_route`, `acl_in_allow`) which serialize the matching
/// typed settings struct into `settings`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub struct EndpointPolicy {
    #[serde(rename = "Type")]
    pub ty: EndpointPolicyType,
    pub settings: serde_json::Value,
}

/// Enumerates every `EndpointPolicyType` hcsshim recognises (April 2026).
///
/// The on-wire string uses the hcsshim Go constant's literal spelling, not a
/// normalized `PascalCase` — hence the explicit `#[serde(rename = ...)]` on each
/// variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EndpointPolicyType {
    #[serde(rename = "PortMapping")]
    PortMapping,
    #[serde(rename = "ACL")]
    Acl,
    #[serde(rename = "QOS")]
    Qos,
    #[serde(rename = "OutBoundNAT")]
    OutBoundNat,
    #[serde(rename = "SDNRoute")]
    SdnRoute,
    #[serde(rename = "L4Proxy")]
    L4Proxy,
    #[serde(rename = "L4WFPPROXY")]
    L4WfpProxy,
    #[serde(rename = "PortName")]
    PortName,
    #[serde(rename = "EncapOverhead")]
    EncapOverhead,
    #[serde(rename = "InterfaceConstraint")]
    InterfaceConstraint,
    #[serde(rename = "ProviderAddress")]
    ProviderAddress,
    #[serde(rename = "Iov")]
    Iov,
    #[serde(rename = "TierAcl")]
    TierAcl,
    #[serde(rename = "NetworkACL")]
    NetworkAcl,
    #[serde(rename = "NetworkL4Proxy")]
    NetworkL4Proxy,
}

/// `OutBoundNAT` policy settings: SNAT egress packets to the host IP unless
/// the destination is in `exceptions`.
///
/// For overlay networks, put the cluster CIDR (e.g. `10.200.0.0/16`) in
/// `exceptions` so inter-container traffic isn't SNAT'd.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub struct OutBoundNatPolicySetting {
    #[serde(
        default,
        skip_serializing_if = "String::is_empty",
        rename = "VirtualIP"
    )]
    pub virtual_ip: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exceptions: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub destinations: Vec<String>,
    #[serde(default, skip_serializing_if = "u32_is_zero")]
    pub flags: u32,
    #[serde(default, skip_serializing_if = "u16_is_zero")]
    pub max_port_pool_usage: u16,
}

/// `SDNRoute` policy settings: adds a route inside the container's routing
/// table. Use with `need_encap: false` to tell HCN that traffic to
/// `destination_prefix` must *not* be VXLAN-encapsulated (we do encap via
/// `WireGuard` on the host).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub struct SdnRoutePolicySetting {
    pub destination_prefix: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub next_hop: String,
    #[serde(default)]
    pub need_encap: bool,
}

/// `ACL` policy settings: applied per-endpoint. Multiple values in a single
/// field (addresses, ports, protocols) are **comma-separated strings**, not
/// JSON arrays — a hcsshim quirk that differs from every other HCN policy type.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub struct AclPolicySetting {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub protocols: String,
    pub action: AclAction,
    pub direction: AclDirection,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub local_addresses: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub remote_addresses: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub local_ports: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub remote_ports: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub rule_type: String,
    #[serde(default, skip_serializing_if = "u16_is_zero")]
    pub priority: u16,
}

/// ACL verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AclAction {
    Allow,
    Block,
    Pass,
}

/// ACL packet direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AclDirection {
    In,
    Out,
}

// serde's `skip_serializing_if` requires `fn(&T) -> bool`, so these
// helpers must take their argument by reference even though u16/u32 fit
// in a register.
#[allow(clippy::trivially_copy_pass_by_ref)]
fn u16_is_zero(v: &u16) -> bool {
    *v == 0
}
#[allow(clippy::trivially_copy_pass_by_ref)]
fn u32_is_zero(v: &u32) -> bool {
    *v == 0
}

impl EndpointPolicy {
    /// Build an `OutBoundNAT` policy with a single exceptions list (typically
    /// the cluster CIDR).
    ///
    /// # Panics
    ///
    /// Never panics in practice: `OutBoundNatPolicySetting` is plain data
    /// (`Vec<String>` + `String`) so `serde_json::to_value` cannot fail for
    /// it. The `.expect()` below is defensive.
    #[must_use]
    pub fn out_bound_nat(exceptions: Vec<String>) -> Self {
        let settings = OutBoundNatPolicySetting {
            exceptions,
            ..OutBoundNatPolicySetting::default()
        };
        Self {
            ty: EndpointPolicyType::OutBoundNat,
            settings: serde_json::to_value(settings)
                .expect("OutBoundNatPolicySetting is plain data, cannot fail"),
        }
    }

    /// Build an `SDNRoute` policy declaring that the given destination prefix
    /// does (or does not) need HCN-managed encapsulation.
    ///
    /// # Panics
    ///
    /// Never panics in practice: `SdnRoutePolicySetting` is plain data
    /// (`String` + `bool` + `u16`) so `serde_json::to_value` cannot fail
    /// for it. The `.expect()` below is defensive.
    #[must_use]
    pub fn sdn_route(destination_prefix: impl Into<String>, need_encap: bool) -> Self {
        let settings = SdnRoutePolicySetting {
            destination_prefix: destination_prefix.into(),
            need_encap,
            ..SdnRoutePolicySetting::default()
        };
        Self {
            ty: EndpointPolicyType::SdnRoute,
            settings: serde_json::to_value(settings)
                .expect("SdnRoutePolicySetting is plain data, cannot fail"),
        }
    }

    /// Build an inbound `Allow` ACL for a remote address range, typically the
    /// cluster CIDR to allow overlay-sourced traffic into the endpoint.
    ///
    /// # Panics
    ///
    /// Never panics in practice: `AclPolicySetting` is plain data (all
    /// `String`s + two unit enums + `u16`) so `serde_json::to_value` cannot
    /// fail for it. The `.expect()` below is defensive.
    #[must_use]
    pub fn acl_in_allow(remote_addresses: impl Into<String>) -> Self {
        let settings = AclPolicySetting {
            protocols: String::new(),
            action: AclAction::Allow,
            direction: AclDirection::In,
            local_addresses: String::new(),
            remote_addresses: remote_addresses.into(),
            local_ports: String::new(),
            remote_ports: String::new(),
            // HCN requires both RuleType and Priority on an ACL applied to a
            // Transparent-network endpoint; omitting either gets the whole
            // endpoint create rejected with `HCN policy rejected`. "Switch"
            // RuleType applies the ACL at the vSwitch port; 65500 is the
            // low-precedence priority hcsshim/Calico-Windows use for a
            // broad allow rule.
            rule_type: "Switch".to_string(),
            priority: 65500,
        };
        Self {
            ty: EndpointPolicyType::Acl,
            settings: serde_json::to_value(settings)
                .expect("AclPolicySetting is plain data, cannot fail"),
        }
    }
}

impl From<EndpointPolicy> for serde_json::Value {
    fn from(policy: EndpointPolicy) -> Self {
        serde_json::to_value(policy).expect("EndpointPolicy is plain data, cannot fail")
    }
}

// ---------------------------------------------------------------------------
// Namespace
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum NamespaceType {
    /// Per-container host namespace. Each call to `HcnCreateNamespace` with
    /// `Type=Host` returns a fresh unique GUID. This is what `containerd-shim-runhcs-v1`
    /// creates for every process-isolated WCOW container (verified May 2026
    /// via ETW capture of `Microsoft-Windows-Hyper-V-Compute`).
    #[serde(rename = "Host")]
    #[default]
    Host,
    /// Singleton host namespace shared by every process-isolated container on
    /// the host. `HcnCreateNamespace` ignores the GUID passed in and always
    /// returns the system-assigned ID (`910F7D92-…`). Was previously the
    /// default — DON'T USE for new container attachments; HCS `Construct`
    /// rejects compute-system docs that reference `HostDefault` with
    /// `E_INVALIDARG (0x80070057)`.
    #[serde(rename = "HostDefault")]
    HostDefault,
    #[serde(rename = "HostInterface")]
    HostInterface,
    #[serde(rename = "Guest")]
    Guest,
    #[serde(rename = "GuestInterface")]
    GuestInterface,
}

/// Top-level HCN namespace object.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct HostComputeNamespace {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default)]
    pub namespace_id: u32, // kernel-level compartment id
    #[serde(rename = "Type", default)]
    pub ty: NamespaceType,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resources: Vec<NamespaceResource>,
    pub schema_version: SchemaVersion,
}

/// A single resource (endpoint or container) attached to a namespace.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct NamespaceResource {
    #[serde(rename = "Type")]
    pub ty: String, // "Endpoint" | "Container"
    #[serde(default)]
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Stats
// ---------------------------------------------------------------------------

/// Counters returned by `HcnQueryEndpointStats`. All counters are `u64`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct EndpointStats {
    pub endpoint_id: String,
    pub instance_id: String,
    pub bytes_received: u64,
    pub bytes_sent: u64,
    pub packets_received: u64,
    pub packets_sent: u64,
    pub dropped_packets_incoming: u64,
    pub dropped_packets_outgoing: u64,
}

// ---------------------------------------------------------------------------
// Modify requests
// ---------------------------------------------------------------------------

/// Operation verb for an HCN modify request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum ModifyRequestType {
    Add,
    Remove,
    Update,
    Refresh,
}

/// Envelope for `HcnModifyNamespace`. Used to attach or detach an endpoint
/// from a namespace.
///
/// `resource_type` is the **string** `"Endpoint"` (or `"Container"`), matching
/// hcsshim's `NamespaceResourceType` constants
/// (`hcn/hcnnamespace.go::NamespaceResourceTypeEndpoint = "Endpoint"`). HCN
/// rejects the request with a bare `HcnModifyNamespace` failure if this is sent
/// as the integer `1` instead of the string.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ModifyNamespaceSettingRequest {
    pub resource_type: String,
    pub request_type: ModifyRequestType,
    pub settings: serde_json::Value, // e.g. { "EndpointId": "<guid>" }
}

/// Envelope for `HcnModifyEndpoint` (policy/port updates).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ModifyEndpointSettingRequest {
    #[serde(rename = "ResourceType")]
    pub resource_type: String, // "Policy" or "Port"
    pub request_type: ModifyRequestType,
    pub settings: serde_json::Value,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::{
        Dns, EndpointStats, HostComputeEndpoint, HostComputeNetwork, IpConfig, Ipam,
        ModifyNamespaceSettingRequest, ModifyRequestType, NetworkType, Route, SchemaVersion,
        Subnet,
    };
    use serde_json::json;

    #[test]
    fn schema_version_default_is_v2_0() {
        let v = SchemaVersion::default();
        assert_eq!(v.major, 2);
        assert_eq!(v.minor, 0);
    }

    #[test]
    fn endpoint_json_preserves_two_cap_ip_configurations() {
        let ep = HostComputeEndpoint {
            name: "ep1".to_string(),
            host_compute_network: "net-guid".to_string(),
            ip_configurations: vec![IpConfig {
                ip_address: "10.0.0.5".to_string(),
                prefix_length: 24,
            }],
            ..HostComputeEndpoint::default()
        };
        let s = serde_json::to_string(&ep).unwrap();
        assert!(
            s.contains("\"IPConfigurations\":["),
            "expected two-cap IPConfigurations in JSON: {s}"
        );
        assert!(
            !s.contains("\"IpConfigurations\""),
            "must not emit one-cap IpConfigurations: {s}"
        );
        assert!(
            s.contains("\"IpAddress\":\"10.0.0.5\""),
            "expected one-cap IpAddress in JSON: {s}"
        );
    }

    #[test]
    fn ipam_static_default_type() {
        // Missing Type field on the wire should default to "Static".
        let raw = json!({
            "Subnets": []
        });
        let ipam: Ipam = serde_json::from_value(raw).unwrap();
        assert_eq!(ipam.ty, "Static");
    }

    #[test]
    fn network_round_trip_nat() {
        let net = HostComputeNetwork {
            id: Some("netid".to_string()),
            name: "zlayer-nat".to_string(),
            ty: NetworkType::Nat,
            ipams: vec![Ipam {
                ty: "Static".to_string(),
                subnets: vec![Subnet {
                    ip_address_prefix: "10.0.0.0/24".to_string(),
                    routes: vec![Route {
                        next_hop: "10.0.0.1".to_string(),
                        destination_prefix: "0.0.0.0/0".to_string(),
                        metric: Some(100),
                    }],
                    policies: vec![],
                }],
            }],
            dns: Some(Dns {
                server_list: vec!["1.1.1.1".to_string()],
                ..Dns::default()
            }),
            schema_version: SchemaVersion::default(),
            ..HostComputeNetwork::default()
        };

        let s = serde_json::to_string(&net).unwrap();
        let back: HostComputeNetwork = serde_json::from_str(&s).unwrap();

        assert_eq!(back.name, "zlayer-nat");
        assert!(matches!(back.ty, NetworkType::Nat));
        assert_eq!(back.ipams.len(), 1);
        assert_eq!(back.ipams[0].subnets[0].ip_address_prefix, "10.0.0.0/24");
        assert_eq!(back.ipams[0].subnets[0].routes[0].next_hop, "10.0.0.1");
        assert_eq!(back.ipams[0].subnets[0].routes[0].metric, Some(100));
        assert_eq!(
            back.dns.as_ref().unwrap().server_list,
            vec!["1.1.1.1".to_string()]
        );
        assert_eq!(back.schema_version, SchemaVersion::default());
    }

    #[test]
    fn network_query_tolerates_missing_schema_version() {
        // A real host network (WSL / Default Switch) query response can omit
        // SchemaVersion entirely. Deserialization must still succeed.
        let raw = json!({
            "Name": "Default Switch",
            "Type": "ICS"
        });
        let net: HostComputeNetwork = serde_json::from_value(raw).unwrap();
        assert_eq!(net.name, "Default Switch");
        assert!(matches!(net.ty, NetworkType::Ics));
        // Absent SchemaVersion falls back to the v2.0 default.
        assert_eq!(net.schema_version, SchemaVersion::default());
    }

    #[test]
    fn network_query_tolerates_empty_object() {
        // Even an empty object must deserialize (every field is defaulted).
        let net: HostComputeNetwork = serde_json::from_value(json!({})).unwrap();
        assert_eq!(net.name, "");
        assert_eq!(net.schema_version, SchemaVersion::default());
    }

    #[test]
    fn endpoint_stats_all_u64_fields_roundtrip() {
        let raw = json!({
            "EndpointId": "ep-guid",
            "InstanceId": "inst-guid",
            "BytesReceived": 10_000_000_001u64,
            "BytesSent": 20_000_000_002u64,
            "PacketsReceived": 300u64,
            "PacketsSent": 400u64,
            "DroppedPacketsIncoming": 5u64,
            "DroppedPacketsOutgoing": 6u64
        });
        let stats: EndpointStats = serde_json::from_value(raw).unwrap();
        assert_eq!(stats.endpoint_id, "ep-guid");
        assert_eq!(stats.instance_id, "inst-guid");
        assert_eq!(stats.bytes_received, 10_000_000_001);
        assert_eq!(stats.bytes_sent, 20_000_000_002);
        assert_eq!(stats.packets_received, 300);
        assert_eq!(stats.packets_sent, 400);
        assert_eq!(stats.dropped_packets_incoming, 5);
        assert_eq!(stats.dropped_packets_outgoing, 6);
    }

    #[test]
    fn modify_namespace_request_add_endpoint() {
        let req = ModifyNamespaceSettingRequest {
            resource_type: "Endpoint".to_string(),
            request_type: ModifyRequestType::Add,
            settings: json!({ "EndpointId": "ep-guid" }),
        };
        let v: serde_json::Value = serde_json::to_value(&req).unwrap();
        assert_eq!(v["ResourceType"], json!("Endpoint"));
        assert_eq!(v["RequestType"], json!("Add"));
        assert_eq!(v["Settings"]["EndpointId"], json!("ep-guid"));
    }

    // -----------------------------------------------------------------------
    // EndpointPolicy tests — byte-for-byte match against hcsshim wire format.
    // -----------------------------------------------------------------------

    use super::{
        AclAction, AclDirection, AclPolicySetting, EndpointPolicy, EndpointPolicyType,
        OutBoundNatPolicySetting, SdnRoutePolicySetting,
    };

    #[test]
    fn out_bound_nat_wire_format_exactly_matches_hcsshim() {
        let policy = EndpointPolicy::out_bound_nat(vec!["10.200.0.0/16".to_string()]);
        let v: serde_json::Value = serde_json::to_value(&policy).unwrap();

        // Full expected shape, verified against hcsshim hcn/hcnpolicy.go
        // (OutboundNatPolicySetting, `json:"Exceptions,omitempty"`).
        let expected = json!({
            "Type": "OutBoundNAT",
            "Settings": { "Exceptions": ["10.200.0.0/16"] }
        });
        assert_eq!(v, expected);
    }

    #[test]
    fn out_bound_nat_empty_exceptions_omits_field() {
        // With no exceptions, Settings should serialize as an empty object —
        // Exceptions/Destinations/VirtualIP/Flags/MaxPortPoolUsage all
        // skip-if-default.
        let policy = EndpointPolicy {
            ty: EndpointPolicyType::OutBoundNat,
            settings: serde_json::to_value(OutBoundNatPolicySetting::default()).unwrap(),
        };
        let v: serde_json::Value = serde_json::to_value(&policy).unwrap();
        assert_eq!(v, json!({"Type": "OutBoundNAT", "Settings": {}}));
    }

    #[test]
    fn sdn_route_wire_format_no_encap() {
        let policy = EndpointPolicy::sdn_route("10.200.0.0/16", false);
        let v: serde_json::Value = serde_json::to_value(&policy).unwrap();

        // NeedEncap is not marked skip_if_false because the field is always
        // meaningful; but Calico-Windows emits it explicitly for both values,
        // so `false` stays on the wire.
        let expected = json!({
            "Type": "SDNRoute",
            "Settings": {
                "DestinationPrefix": "10.200.0.0/16",
                "NeedEncap": false,
            }
        });
        assert_eq!(v, expected);
    }

    #[test]
    fn sdn_route_wire_format_need_encap_true() {
        let policy = EndpointPolicy::sdn_route("10.96.0.0/12", true);
        let v: serde_json::Value = serde_json::to_value(&policy).unwrap();
        let expected = json!({
            "Type": "SDNRoute",
            "Settings": {
                "DestinationPrefix": "10.96.0.0/12",
                "NeedEncap": true,
            }
        });
        assert_eq!(v, expected);
    }

    #[test]
    fn acl_in_allow_wire_format() {
        let policy = EndpointPolicy::acl_in_allow("10.200.0.0/16");
        let v: serde_json::Value = serde_json::to_value(&policy).unwrap();

        // Action + Direction always emit (no skip_if_default). Protocols +
        // address/port fields all skip-if-empty. RuleType + Priority are
        // always set by the builder because a Transparent-network endpoint
        // rejects an ACL that omits either (see `acl_in_allow`).
        let expected = json!({
            "Type": "ACL",
            "Settings": {
                "Action": "Allow",
                "Direction": "In",
                "RemoteAddresses": "10.200.0.0/16",
                "RuleType": "Switch",
                "Priority": 65500,
            }
        });
        assert_eq!(v, expected);
    }

    #[test]
    fn acl_full_form_with_protocols_ports_priority() {
        // Hand-build the full form (comma-separated strings, not arrays) and
        // verify the wire output matches the hcsshim hcnutils_test.go fixture.
        let settings = AclPolicySetting {
            protocols: "6,17".to_string(),
            action: AclAction::Block,
            direction: AclDirection::Out,
            local_addresses: "10.0.0.21".to_string(),
            remote_addresses: "192.168.100.0/24".to_string(),
            local_ports: "80,8080".to_string(),
            remote_ports: "80,8080".to_string(),
            rule_type: "Switch".to_string(),
            priority: 200,
        };
        let v = serde_json::to_value(settings).unwrap();
        assert_eq!(
            v,
            json!({
                "Protocols": "6,17",
                "Action": "Block",
                "Direction": "Out",
                "LocalAddresses": "10.0.0.21",
                "RemoteAddresses": "192.168.100.0/24",
                "LocalPorts": "80,8080",
                "RemotePorts": "80,8080",
                "RuleType": "Switch",
                "Priority": 200,
            })
        );
    }

    #[test]
    fn endpoint_policy_type_tag_spellings() {
        // Every Type tag must round-trip to exactly the string hcsshim emits.
        let cases = [
            (EndpointPolicyType::PortMapping, "PortMapping"),
            (EndpointPolicyType::Acl, "ACL"),
            (EndpointPolicyType::Qos, "QOS"),
            (EndpointPolicyType::OutBoundNat, "OutBoundNAT"),
            (EndpointPolicyType::SdnRoute, "SDNRoute"),
            (EndpointPolicyType::L4Proxy, "L4Proxy"),
            (EndpointPolicyType::L4WfpProxy, "L4WFPPROXY"),
            (EndpointPolicyType::PortName, "PortName"),
            (EndpointPolicyType::EncapOverhead, "EncapOverhead"),
            (
                EndpointPolicyType::InterfaceConstraint,
                "InterfaceConstraint",
            ),
            (EndpointPolicyType::ProviderAddress, "ProviderAddress"),
            (EndpointPolicyType::Iov, "Iov"),
            (EndpointPolicyType::TierAcl, "TierAcl"),
            (EndpointPolicyType::NetworkAcl, "NetworkACL"),
            (EndpointPolicyType::NetworkL4Proxy, "NetworkL4Proxy"),
        ];
        for (variant, expected) in cases {
            let v = serde_json::to_value(variant).unwrap();
            assert_eq!(v, serde_json::Value::String(expected.to_string()));
            let back: EndpointPolicyType = serde_json::from_value(v).unwrap();
            assert_eq!(back, variant);
        }
    }

    #[test]
    fn endpoint_policy_into_value_pushes_into_policies_vec() {
        // The motivating use case: keep HostComputeEndpoint.policies as
        // Vec<serde_json::Value> (escape hatch for unknown types) and push
        // typed EndpointPolicy values into it via `.into()`.
        let policies: Vec<serde_json::Value> = vec![
            EndpointPolicy::out_bound_nat(vec!["10.200.0.0/16".to_string()]).into(),
            EndpointPolicy::sdn_route("10.200.0.0/16", false).into(),
            EndpointPolicy::acl_in_allow("10.200.0.0/16").into(),
        ];
        let ep = HostComputeEndpoint {
            name: "ep".to_string(),
            host_compute_network: "net-guid".to_string(),
            policies,
            ..HostComputeEndpoint::default()
        };
        let s = serde_json::to_string(&ep).unwrap();
        assert!(s.contains("\"OutBoundNAT\""));
        assert!(s.contains("\"SDNRoute\""));
        assert!(s.contains("\"ACL\""));
    }

    #[test]
    fn sdn_route_with_next_hop_round_trips() {
        let settings = SdnRoutePolicySetting {
            destination_prefix: "10.96.0.0/12".to_string(),
            next_hop: "10.200.0.1".to_string(),
            need_encap: true,
        };
        let v = serde_json::to_value(settings.clone()).unwrap();
        let back: SdnRoutePolicySetting = serde_json::from_value(v).unwrap();
        assert_eq!(settings, back);
    }
}
