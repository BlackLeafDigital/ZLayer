//! IPC wire protocol between the main `zlayer` daemon and `zlayer-overlayd`.
//!
//! `zlayer-overlayd` is a standalone, long-lived daemon that owns every
//! mechanism touching the overlay/network plane (the `WireGuard` device +
//! adapter, peers, `AllowedIPs`/service subnets, IP allocation, DNS, NAT,
//! Linux bridges + veth/netns attach, and the Windows HCN Internal network +
//! endpoints). The main `zlayer` daemon keeps the cluster brain (Raft, the
//! scheduler, the service registry, container/HCS process lifecycle) and
//! drives overlayd over a length-prefixed-JSON IPC channel (a Unix domain
//! socket on Unix, a named pipe on Windows).
//!
//! This module is that channel's **wire contract** — the only thing both
//! sides must agree on. It lives in `zlayer-types` (a leaf crate) so the
//! daemon, the overlayd server, and the overlayd client can all depend on it
//! without a dependency cycle.
//!
//! ## Framing
//!
//! One connection multiplexes request/response and server→client event push.
//! Each frame is a [`OverlaydFrame`] serialized as JSON and written with a
//! `u32` little-endian length prefix (the framing itself lives in
//! `zlayer-overlayd`'s transport module, not here). The main daemon sends
//! [`OverlaydFrame::Request`]s each carrying a client-chosen `id`; overlayd
//! replies with a [`OverlaydFrame::Response`] echoing that `id`, and may at
//! any time push an unsolicited [`OverlaydFrame::Event`].
//!
//! ## Wire-type conventions
//!
//! - Windows HCN GUIDs cross the wire as **bare lowercase strings**
//!   (`aabbccdd-eeff-...`, no braces) — `windows::core::GUID` is not
//!   `serde`-serializable and `zlayer-types` must not depend on `windows`.
//! - CIDRs cross as `String` (e.g. `"10.200.0.0/28"`); endpoints as `String`
//!   (`"host:port"`, kept textual so an unresolved/hostname endpoint survives).
//! - Addresses use [`std::net::IpAddr`] (serde-serializable via `std`).

use std::net::IpAddr;

use serde::{Deserialize, Serialize};

pub use crate::overlay::{OverlayConfig, OverlayMode};

/// Wire-protocol version. Bump on any breaking change to the frame/request/
/// response/event shapes so a version-skewed daemon/overlayd pair can detect
/// the mismatch instead of silently misparsing.
pub const PROTOCOL_VERSION: u32 = 1;

/// A multiplexed frame on the overlayd IPC connection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "frame", rename_all = "snake_case")]
pub enum OverlaydFrame {
    /// Main daemon → overlayd. `id` is echoed back on the matching response.
    Request { id: u64, request: OverlaydRequest },
    /// overlayd → main daemon, answering the request with the same `id`.
    Response { id: u64, response: OverlaydResponse },
    /// overlayd → main daemon, unsolicited (no `id`).
    Event(OverlaydEvent),
}

/// Identifies the container overlayd must wire into the overlay. The agent
/// owns the container's process/compute-system lifecycle and hands overlayd
/// just enough to attach it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "platform", rename_all = "snake_case")]
pub enum AttachHandle {
    /// Linux: the container's PID. overlayd opens `/proc/<pid>/ns/net` and
    /// creates the veth pair into that network namespace.
    LinuxPid { pid: u32 },
    /// Windows: the HCS container id (+ the IP the agent reserved, if any).
    /// overlayd creates the HCN endpoint + per-container namespace on its HCN
    /// Internal network and returns the bare-lowercase namespace GUID
    /// ([`AttachResult::namespace_guid`]) for the agent to embed in the
    /// compute-system document's `Container.Networking.Namespace`.
    WindowsContainer {
        container_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ip: Option<IpAddr>,
    },
    /// A guest that manages its own overlay interface (the macOS VZ-Linux VM
    /// runtime). overlayd cannot enter the guest's netns (it is a VM, not a host
    /// process), so instead of building a veth it **allocates the overlay
    /// identity** — keypair, address, and the current peer set — registers the
    /// generated public key in the mesh, and returns it as
    /// [`OverlaydResponse::GuestConfig`]. The caller ships that config into the
    /// guest (over vsock) where a kernel `WireGuard` device is brought up. `id` is
    /// the opaque container id used to scope the allocation + the registered
    /// peer so `DetachContainer` can release it.
    GuestManaged { id: String },
}

/// A request from the main daemon to overlayd.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum OverlaydRequest {
    /// Push this node's Raft id (cluster-brain context overlayd scopes by).
    SetLocalNodeId { node_id: u64 },
    /// Push this node's `WireGuard` public key (base64).
    SetLocalWgPubkey { pubkey: String },

    /// Bring up (or reuse) this node's base/global overlay. Idempotent: if the
    /// overlay network already exists (recorded in overlayd's marker), it is
    /// reused rather than recreated. This is the only place the base overlay
    /// is created; it is torn down only on a full uninstall.
    SetupGlobalOverlay {
        deployment: String,
        instance_id: String,
        /// Full cluster CIDR, e.g. `"10.200.0.0/16"`.
        cluster_cidr: String,
        /// This node's per-node slice, e.g. `"10.200.0.0/28"`. `None` until the
        /// leader assigns one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        slice_cidr: Option<String>,
        wg_port: u16,
        nat_enabled: bool,
    },
    /// Tear down the node's base overlay (e.g. on full uninstall).
    TeardownGlobalOverlay,

    /// Create the per-service overlay segment (Linux bridge / Windows HCN
    /// Internal network) for `service`. Returns [`OverlaydResponse::BridgeName`].
    SetupServiceOverlay { service: String, mode: OverlayMode },
    /// Remove the per-service overlay segment.
    TeardownServiceOverlay { service: String },

    /// Allocate (or, with `ip` set on a later attach, validate) an overlay IP
    /// from the node slice for a container on `service`.
    AllocateIp { service: String, join_global: bool },
    /// Return an overlay IP to the allocator.
    ReleaseIp { ip: IpAddr },

    /// Wire a container into the overlay. Returns [`OverlaydResponse::Attached`].
    AttachContainer {
        handle: AttachHandle,
        service: String,
        join_global: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        dns_server: Option<IpAddr>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        dns_domain: Option<String>,
    },
    /// Tear down a container's overlay attachment and release its IP.
    DetachContainer { handle: AttachHandle },

    /// Add a `WireGuard` peer to the base overlay.
    AddPeer {
        #[serde(flatten)]
        peer: PeerSpec,
        #[serde(default)]
        scope: PeerScope,
    },
    /// Remove a peer by its base64 public key.
    RemovePeer {
        pubkey: String,
        #[serde(default)]
        scope: PeerScope,
    },
    /// Plumb a service subnet into a peer's `AllowedIPs`.
    AddAllowedIp {
        pubkey: String,
        cidr: String,
        #[serde(default)]
        scope: PeerScope,
    },
    /// Remove a service subnet from a peer's `AllowedIPs`.
    RemoveAllowedIp {
        pubkey: String,
        cidr: String,
        #[serde(default)]
        scope: PeerScope,
    },

    /// Register an overlay DNS A/AAAA record.
    RegisterDns { name: String, ip: IpAddr },
    /// Remove an overlay DNS record.
    UnregisterDns { name: String },

    /// Snapshot overlay state for diagnostics. Returns [`OverlaydResponse::Status`].
    Status,
    /// Run one NAT-traversal maintenance tick (probe/refresh endpoints).
    NatTick,
    /// Ask overlayd to shut down gracefully (drops the adapter).
    Shutdown,
}

/// overlayd's answer to an [`OverlaydRequest`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum OverlaydResponse {
    /// Generic success with no payload.
    Ok,
    /// An allocated/validated overlay IP (`AllocateIp`).
    Ip { ip: IpAddr },
    /// A completed container attach.
    Attached(AttachResult),
    /// The overlay identity for a guest-managed attach
    /// ([`AttachHandle::GuestManaged`]): the keypair, allocated address, and the
    /// peer set the guest should configure on its own `WireGuard` device.
    GuestConfig(GuestOverlayConfig),
    /// The interface/bridge/network name created (`SetupServiceOverlay`,
    /// `SetupGlobalOverlay`).
    BridgeName { name: String },
    /// A diagnostics snapshot (`Status`).
    Status(StatusSnapshot),
    /// A dedicated per-service overlay device's identity (`SetupServiceOverlay`
    /// in Dedicated mode). Not yet produced by the server — the server still
    /// returns [`OverlaydResponse::BridgeName`] for now; this variant is the
    /// wire contract for a later task that switches Dedicated setup over.
    ServiceOverlay(ServiceOverlayInfo),
    /// The request failed; `message` is a human-readable reason.
    Err { message: String },
}

/// Identity of a dedicated per-service overlay device, reported by
/// `SetupServiceOverlay` once Dedicated mode is wired up. Shared-mode setups
/// leave the `wg_*`/`overlay_ip`/`subnet` fields `None`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceOverlayInfo {
    pub name: String,
    pub mode: crate::overlay::OverlayMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wg_public_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wg_port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overlay_ip: Option<std::net::IpAddr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subnet: Option<String>,
}

/// Result of [`OverlaydRequest::AttachContainer`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachResult {
    /// The container's overlay IP.
    pub ip: IpAddr,
    /// Windows only: the bare-lowercase HCN namespace GUID the agent embeds in
    /// the compute-system document. `None` on Linux (no HCN namespace).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace_guid: Option<String>,
}

/// Overlay identity returned for a guest-managed attach
/// ([`AttachHandle::GuestManaged`] → [`OverlaydResponse::GuestConfig`]).
///
/// The host allocated the address from the node slice, generated the keypair,
/// and registered `public_key` in the mesh (so peers route to the guest). The
/// caller ships everything except `public_key` into the guest; `public_key` is
/// echoed back so the caller can record/deregister the peer it represents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuestOverlayConfig {
    /// The guest's allocated overlay address.
    pub overlay_ip: IpAddr,
    /// Prefix length of the overlay network (interface address + on-link route).
    pub prefix_len: u8,
    /// Base64 `WireGuard` private key for the guest's overlay endpoint.
    pub private_key: String,
    /// Base64 `WireGuard` public key matching `private_key` (registered in the
    /// mesh by overlayd; echoed for the caller's bookkeeping).
    pub public_key: String,
    /// UDP port the guest's `WireGuard` device should listen on.
    pub listen_port: u16,
    /// The peers the guest should configure (other nodes/containers).
    pub peers: Vec<PeerSpec>,
    /// Overlay DNS resolver IP for the container, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dns_server: Option<IpAddr>,
    /// Overlay DNS search domain, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dns_domain: Option<String>,
}

/// Which overlay device a peer / `AllowedIP` op targets. `Global` (default, and
/// the only value pre-Dedicated senders emit) = the single cluster transport.
/// `Service` = that service's dedicated per-service transport.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "scope", rename_all = "snake_case")]
pub enum PeerScope {
    #[default]
    Global,
    Service {
        service: String,
    },
}

/// A `WireGuard` peer to add to the base overlay. Mirrors
/// `zlayer_overlay::PeerInfo` but with wire-safe field types.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerSpec {
    /// base64 `WireGuard` public key.
    pub public_key: String,
    /// `host:port` (textual so an unresolved/hostname endpoint survives).
    pub endpoint: String,
    /// Comma-separated CIDR list (e.g. `"10.200.0.5/32,10.200.1.0/24"`).
    pub allowed_ips: String,
    /// Persistent-keepalive interval, in seconds (0 = disabled).
    pub persistent_keepalive_secs: u64,
}

/// An unsolicited notification pushed from overlayd to the main daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum OverlaydEvent {
    /// A peer's liveness changed (handshake seen / lost).
    PeerHealthChanged { pubkey: String, healthy: bool },
    /// NAT traversal moved a peer to a new endpoint.
    NatEndpointChanged { pubkey: String, endpoint: String },
}

/// Diagnostics snapshot returned by [`OverlaydRequest::Status`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusSnapshot {
    /// Base overlay interface name (e.g. `"zl-overlay0"`), if up.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interface: Option<String>,
    /// This node's overlay IP, if assigned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_ip: Option<IpAddr>,
    /// This node's `WireGuard` public key (base64), if up.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_key: Option<String>,
    /// Full cluster CIDR.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overlay_cidr: Option<String>,
    /// This node's per-node slice CIDR.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slice_cidr: Option<String>,
    /// Number of base-overlay peers.
    pub peer_count: u32,
    /// Number of per-service overlays set up on this node.
    pub service_count: u32,
    /// Per-peer status.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub peers: Vec<PeerStatus>,
    /// Per dedicated per-service overlay device status. Empty unless one or
    /// more services run in `OverlayMode::Dedicated` on this node.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dedicated_services: Vec<DedicatedServiceStatus>,
}

/// Status of a single dedicated per-service overlay device within a
/// [`StatusSnapshot`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DedicatedServiceStatus {
    pub service: String,
    pub interface: String,
    pub public_key: String,
    pub listen_port: u16,
    pub overlay_ip: std::net::IpAddr,
    pub subnet: String,
    pub peer_count: u32,
}

/// Per-peer status within a [`StatusSnapshot`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerStatus {
    pub public_key: String,
    pub endpoint: String,
    pub allowed_ips: String,
    /// Last successful handshake, Unix seconds; `0` if never.
    pub last_handshake_unix_secs: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame round-trips through JSON unchanged (the core wire guarantee).
    fn roundtrip(frame: &OverlaydFrame) {
        let json = serde_json::to_string(frame).expect("serialize");
        let back: OverlaydFrame = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(frame, &back, "frame must round-trip; json was {json}");
    }

    #[test]
    fn request_frames_round_trip() {
        roundtrip(&OverlaydFrame::Request {
            id: 1,
            request: OverlaydRequest::SetupGlobalOverlay {
                deployment: "prod".into(),
                instance_id: "42".into(),
                cluster_cidr: "10.200.0.0/16".into(),
                slice_cidr: Some("10.200.0.0/28".into()),
                wg_port: 51820,
                nat_enabled: true,
            },
        });
        roundtrip(&OverlaydFrame::Request {
            id: 2,
            request: OverlaydRequest::AttachContainer {
                handle: AttachHandle::WindowsContainer {
                    container_id: "ctr-abc".into(),
                    ip: Some("10.200.0.5".parse().unwrap()),
                },
                service: "web".into(),
                join_global: false,
                dns_server: Some("10.200.0.1".parse().unwrap()),
                dns_domain: Some("overlay".into()),
            },
        });
        roundtrip(&OverlaydFrame::Request {
            id: 3,
            request: OverlaydRequest::AttachContainer {
                handle: AttachHandle::LinuxPid { pid: 4242 },
                service: "web".into(),
                join_global: true,
                dns_server: None,
                dns_domain: None,
            },
        });
    }

    #[test]
    fn response_and_event_frames_round_trip() {
        roundtrip(&OverlaydFrame::Response {
            id: 2,
            response: OverlaydResponse::Attached(AttachResult {
                ip: "10.200.0.5".parse().unwrap(),
                namespace_guid: Some("aabbccdd-eeff-0011-2233-445566778899".into()),
            }),
        });
        roundtrip(&OverlaydFrame::Response {
            id: 9,
            response: OverlaydResponse::Err {
                message: "no slice assigned".into(),
            },
        });
        roundtrip(&OverlaydFrame::Event(OverlaydEvent::PeerHealthChanged {
            pubkey: "base64key".into(),
            healthy: false,
        }));
    }

    #[test]
    fn status_snapshot_round_trips_and_defaults() {
        roundtrip(&OverlaydFrame::Response {
            id: 7,
            response: OverlaydResponse::Status(StatusSnapshot {
                interface: Some("zl-overlay0".into()),
                node_ip: Some("10.200.0.1".parse().unwrap()),
                peer_count: 2,
                service_count: 1,
                peers: vec![PeerStatus {
                    public_key: "k".into(),
                    endpoint: "1.2.3.4:51820".into(),
                    allowed_ips: "10.200.0.2/32".into(),
                    last_handshake_unix_secs: 0,
                }],
                ..StatusSnapshot::default()
            }),
        });
    }

    fn sample_peer() -> PeerSpec {
        PeerSpec {
            public_key: "base64key".into(),
            endpoint: "1.2.3.4:51820".into(),
            allowed_ips: "10.200.0.2/32".into(),
            persistent_keepalive_secs: 25,
        }
    }

    #[test]
    fn peer_ops_round_trip_both_scopes() {
        // AddPeer, global (default) + service scope.
        roundtrip(&OverlaydFrame::Request {
            id: 1,
            request: OverlaydRequest::AddPeer {
                peer: sample_peer(),
                scope: PeerScope::Global,
            },
        });
        roundtrip(&OverlaydFrame::Request {
            id: 2,
            request: OverlaydRequest::AddPeer {
                peer: sample_peer(),
                scope: PeerScope::Service {
                    service: "web".into(),
                },
            },
        });
        // RemovePeer.
        roundtrip(&OverlaydFrame::Request {
            id: 3,
            request: OverlaydRequest::RemovePeer {
                pubkey: "k".into(),
                scope: PeerScope::Global,
            },
        });
        roundtrip(&OverlaydFrame::Request {
            id: 4,
            request: OverlaydRequest::RemovePeer {
                pubkey: "k".into(),
                scope: PeerScope::Service {
                    service: "web".into(),
                },
            },
        });
        // AddAllowedIp.
        roundtrip(&OverlaydFrame::Request {
            id: 5,
            request: OverlaydRequest::AddAllowedIp {
                pubkey: "k".into(),
                cidr: "10.200.1.0/24".into(),
                scope: PeerScope::Global,
            },
        });
        roundtrip(&OverlaydFrame::Request {
            id: 6,
            request: OverlaydRequest::AddAllowedIp {
                pubkey: "k".into(),
                cidr: "10.200.1.0/24".into(),
                scope: PeerScope::Service {
                    service: "web".into(),
                },
            },
        });
        // RemoveAllowedIp.
        roundtrip(&OverlaydFrame::Request {
            id: 7,
            request: OverlaydRequest::RemoveAllowedIp {
                pubkey: "k".into(),
                cidr: "10.200.1.0/24".into(),
                scope: PeerScope::Global,
            },
        });
        roundtrip(&OverlaydFrame::Request {
            id: 8,
            request: OverlaydRequest::RemoveAllowedIp {
                pubkey: "k".into(),
                cidr: "10.200.1.0/24".into(),
                scope: PeerScope::Service {
                    service: "web".into(),
                },
            },
        });
    }

    #[test]
    fn add_peer_without_scope_defaults_to_global() {
        // A pre-Dedicated sender emits no `scope` field. The frame is tagged
        // `frame: "request"`, the request `op: "add_peer"`, and `PeerSpec` is
        // flattened so its fields sit at the request level.
        let json = r#"{
            "frame": "request",
            "id": 11,
            "request": {
                "op": "add_peer",
                "public_key": "base64key",
                "endpoint": "1.2.3.4:51820",
                "allowed_ips": "10.200.0.2/32",
                "persistent_keepalive_secs": 25
            }
        }"#;
        let frame: OverlaydFrame = serde_json::from_str(json).expect("deserialize");
        match frame {
            OverlaydFrame::Request {
                request: OverlaydRequest::AddPeer { scope, peer },
                ..
            } => {
                assert_eq!(scope, PeerScope::Global);
                assert_eq!(peer.public_key, "base64key");
            }
            other => panic!("expected AddPeer request, got {other:?}"),
        }
    }

    #[test]
    fn service_overlay_response_round_trips_both_shapes() {
        // Shared shape: identity fields are None.
        roundtrip(&OverlaydFrame::Response {
            id: 20,
            response: OverlaydResponse::ServiceOverlay(ServiceOverlayInfo {
                name: "web".into(),
                mode: crate::overlay::OverlayMode::Shared,
                wg_public_key: None,
                wg_port: None,
                overlay_ip: None,
                subnet: None,
            }),
        });
        // Dedicated shape: all identity fields populated.
        roundtrip(&OverlaydFrame::Response {
            id: 21,
            response: OverlaydResponse::ServiceOverlay(ServiceOverlayInfo {
                name: "web".into(),
                mode: crate::overlay::OverlayMode::Dedicated,
                wg_public_key: Some("svc-key".into()),
                wg_port: Some(51821),
                overlay_ip: Some("10.201.0.1".parse().unwrap()),
                subnet: Some("10.201.0.0/24".into()),
            }),
        });
    }

    #[test]
    fn status_snapshot_with_dedicated_service_round_trips() {
        roundtrip(&OverlaydFrame::Response {
            id: 22,
            response: OverlaydResponse::Status(StatusSnapshot {
                interface: Some("zl-overlay0".into()),
                node_ip: Some("10.200.0.1".parse().unwrap()),
                peer_count: 1,
                service_count: 1,
                dedicated_services: vec![DedicatedServiceStatus {
                    service: "web".into(),
                    interface: "zl-svc-web0".into(),
                    public_key: "svc-key".into(),
                    listen_port: 51821,
                    overlay_ip: "10.201.0.1".parse().unwrap(),
                    subnet: "10.201.0.0/24".into(),
                    peer_count: 3,
                }],
                ..StatusSnapshot::default()
            }),
        });
    }
}
