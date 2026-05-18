//! Conversions between gRPC proto types (this crate) and the worker-tier
//! wire types in `zlayer_types::cluster`.
//!
//! Direction conventions:
//!
//! * Where the mapping is total (no failure cases) we implement
//!   `From<Rust> for Proto` AND `From<Proto> for Rust`.
//! * Where the proto side has `Option`-wrapped sub-messages or a oneof
//!   discriminant that may be `None`, we implement `TryFrom<Proto> for Rust`
//!   returning `Result<Rust, ConvertError>`.
//!
//! ## Schema gaps
//!
//! The proto schema is the future-facing Phase 3 wire format (mTLS, CSR,
//! adaptive heartbeat with cert TTL, gossip seeds, per-(role,index)
//! assignment events). The Rust types currently in `zlayer_types::cluster`
//! describe the simpler Phase 3 MVP wire format (bearer token, no CSR,
//! per-service assignment events bundling many indices). Where one side
//! carries fields the other does not, we drop them on translation:
//!
//! * `RegisterRequest::csr_der`            — proto only (Rust round-trip drops)
//! * `NodeProfile::caps.disk_bytes`        — proto only
//! * `NodeProfile::version`                — proto only
//! * `WorkerProfile::api_addr`             — Rust only (proto round-trip drops)
//! * `RegisterResponse::signed_cert_der`,  — proto only
//!   `ca_chain_der`, `gossip_seeds`
//! * `RegisterResponse::cluster_id`,       — Rust only
//!   `internal_token`
//! * `ContainerStatus::message`,           — proto only
//!   `started_at`, `restart_count`
//! * `WorkerContainerStatus::overlay_ip`   — Rust only
//! * `NodeResourceUsage::disk_used_bytes`  — proto only
//! * `WorkerResourceUsage::gpu_used`       — Rust only
//! * `StatusAck::accepted_revision`        — proto only
//! * `AssignmentSet::spec_json`            — proto only (worker resolves
//!   from in-memory `ServiceSpec` table keyed by service+role+revision)
//!
//! These gaps are intentional for the migration window. Once the worker
//! protocol moves entirely to gRPC, the Rust types in `zlayer_types::cluster`
//! will be replaced by direct re-exports of the proto types and these
//! converters can be deleted.

use std::time::{SystemTime, UNIX_EPOCH};

use thiserror::Error;
use zlayer_types::cluster::{
    ScaleAssignment, WorkerAssignmentEvent, WorkerContainerStatus, WorkerProfile,
    WorkerRegisterRequest, WorkerRegisterResponse, WorkerResourceUsage, WorkerStatusAck,
    WorkerStatusReport,
};

use crate::proto;

/// Errors that can arise when a proto message lacks a required sub-message
/// or oneof discriminant that the richer Rust type needs.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ConvertError {
    /// A required `Option<Message>` sub-field was `None` on the wire.
    #[error("missing field: {0}")]
    MissingField(&'static str),
    /// A oneof field arrived with an unrecognized variant index.
    #[error("unknown variant: {0}")]
    UnknownVariant(&'static str),
    /// A `WorkerProfile::api_addr` could not be parsed (when synthesizing
    /// from a proto `NodeProfile` that has no equivalent field).
    #[error("invalid socket addr: {0}")]
    InvalidSocketAddr(String),
    /// A Rust `WorkerAssignmentEvent::Set` carried 0 or >1 effective
    /// (role,index) pairs and cannot be flattened into a single proto
    /// `AssignmentSet`. Use [`worker_assignment_event_to_protos`] for the
    /// fan-out case.
    #[error("ambiguous fan-out: {0}")]
    AmbiguousFanOut(&'static str),
}

// ----------------------------------------------------------------------------
// Timestamp helpers
// ----------------------------------------------------------------------------

fn systemtime_to_proto(t: SystemTime) -> prost_types::Timestamp {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => prost_types::Timestamp {
            seconds: i64::try_from(d.as_secs()).unwrap_or(i64::MAX),
            nanos: i32::try_from(d.subsec_nanos()).unwrap_or(0),
        },
        Err(_) => prost_types::Timestamp {
            seconds: 0,
            nanos: 0,
        },
    }
}

fn ts_ns_to_proto(ts_ns: u64) -> prost_types::Timestamp {
    let secs = i64::try_from(ts_ns / 1_000_000_000).unwrap_or(i64::MAX);
    let nanos = i32::try_from(ts_ns % 1_000_000_000).unwrap_or(0);
    prost_types::Timestamp {
        seconds: secs,
        nanos,
    }
}

fn proto_to_ts_ns(ts: &prost_types::Timestamp) -> u64 {
    let secs = u64::try_from(ts.seconds).unwrap_or(0);
    let nanos = u32::try_from(ts.nanos).unwrap_or(0);
    secs.saturating_mul(1_000_000_000)
        .saturating_add(u64::from(nanos))
}

// ----------------------------------------------------------------------------
// WorkerProfile <-> proto::NodeProfile
// ----------------------------------------------------------------------------

impl From<WorkerProfile> for proto::NodeProfile {
    fn from(p: WorkerProfile) -> Self {
        proto::NodeProfile {
            os: p.os,
            arch: p.arch,
            labels: p.labels,
            caps: Some(proto::ResourceCaps {
                cpu_cores: f64::from(p.cpu_total),
                memory_bytes: p.memory_total_bytes,
                disk_bytes: 0,
            }),
            version: String::new(),
        }
    }
}

impl TryFrom<proto::NodeProfile> for WorkerProfile {
    type Error = ConvertError;

    fn try_from(p: proto::NodeProfile) -> Result<Self, Self::Error> {
        let caps = p.caps.unwrap_or(proto::ResourceCaps {
            cpu_cores: 0.0,
            memory_bytes: 0,
            disk_bytes: 0,
        });
        let api_addr = "0.0.0.0:0".parse().map_err(|e: std::net::AddrParseError| {
            ConvertError::InvalidSocketAddr(e.to_string())
        })?;
        // Clamp into u32 range; cpu_cores is a `double` on the wire but our
        // Rust side stores whole cores. Negative / out-of-range / NaN map to 0.
        let cpu_total: u32 = {
            let v = caps.cpu_cores;
            if v.is_finite() && v >= 0.0 && v <= f64::from(u32::MAX) {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let cast = v as u32;
                cast
            } else if v > f64::from(u32::MAX) {
                u32::MAX
            } else {
                0
            }
        };
        Ok(WorkerProfile {
            api_addr,
            os: p.os,
            arch: p.arch,
            labels: p.labels,
            cpu_total,
            memory_total_bytes: caps.memory_bytes,
        })
    }
}

// ----------------------------------------------------------------------------
// WorkerRegisterRequest <-> proto::RegisterRequest
// ----------------------------------------------------------------------------

impl From<WorkerRegisterRequest> for proto::RegisterRequest {
    fn from(r: WorkerRegisterRequest) -> Self {
        proto::RegisterRequest {
            bootstrap_token: r.token,
            // proto3 scalar; 0 means "unset"
            desired_node_id: r.desired_node_id.unwrap_or(0),
            profile: Some(r.profile.into()),
            csr_der: Vec::new(),
        }
    }
}

impl TryFrom<proto::RegisterRequest> for WorkerRegisterRequest {
    type Error = ConvertError;

    fn try_from(p: proto::RegisterRequest) -> Result<Self, Self::Error> {
        let profile_proto = p
            .profile
            .ok_or(ConvertError::MissingField("RegisterRequest.profile"))?;
        let profile = WorkerProfile::try_from(profile_proto)?;
        let desired_node_id = if p.desired_node_id == 0 {
            None
        } else {
            Some(p.desired_node_id)
        };
        Ok(WorkerRegisterRequest {
            token: p.bootstrap_token,
            desired_node_id,
            profile,
        })
    }
}

// ----------------------------------------------------------------------------
// WorkerRegisterResponse <-> proto::RegisterResponse
// ----------------------------------------------------------------------------

impl From<WorkerRegisterResponse> for proto::RegisterResponse {
    fn from(r: WorkerRegisterResponse) -> Self {
        proto::RegisterResponse {
            node_id: r.node_id,
            signed_cert_der: Vec::new(),
            ca_chain_der: Vec::new(),
            heartbeat_ttl_secs: r.heartbeat_ttl_secs,
            heartbeat_grace_secs: r.heartbeat_grace_secs,
            gossip_seeds: Vec::new(),
        }
    }
}

impl From<proto::RegisterResponse> for WorkerRegisterResponse {
    fn from(p: proto::RegisterResponse) -> Self {
        WorkerRegisterResponse {
            node_id: p.node_id,
            cluster_id: String::new(),
            heartbeat_ttl_secs: p.heartbeat_ttl_secs,
            heartbeat_grace_secs: p.heartbeat_grace_secs,
            internal_token: String::new(),
        }
    }
}

// ----------------------------------------------------------------------------
// WorkerContainerStatus <-> proto::ContainerStatus
// ----------------------------------------------------------------------------

fn state_str_to_phase(state: &str) -> proto::ContainerPhase {
    match state {
        "pending" => proto::ContainerPhase::Pending,
        "starting" | "creating" | "created" => proto::ContainerPhase::Starting,
        "running" => proto::ContainerPhase::Running,
        "degraded" => proto::ContainerPhase::Degraded,
        "stopping" => proto::ContainerPhase::Stopping,
        "stopped" | "exited" => proto::ContainerPhase::Stopped,
        "failed" => proto::ContainerPhase::Failed,
        _ => proto::ContainerPhase::Unspecified,
    }
}

fn phase_to_state_str(phase: proto::ContainerPhase) -> &'static str {
    match phase {
        proto::ContainerPhase::Unspecified => "unknown",
        proto::ContainerPhase::Pending => "pending",
        proto::ContainerPhase::Starting => "starting",
        proto::ContainerPhase::Running => "running",
        proto::ContainerPhase::Degraded => "degraded",
        proto::ContainerPhase::Stopping => "stopping",
        proto::ContainerPhase::Stopped => "stopped",
        proto::ContainerPhase::Failed => "failed",
    }
}

impl From<WorkerContainerStatus> for proto::ContainerStatus {
    fn from(c: WorkerContainerStatus) -> Self {
        proto::ContainerStatus {
            service: c.service,
            role: c.role,
            index: c.replica,
            phase: state_str_to_phase(&c.state) as i32,
            message: String::new(),
            started_at: None,
            restart_count: 0,
        }
    }
}

impl From<proto::ContainerStatus> for WorkerContainerStatus {
    fn from(p: proto::ContainerStatus) -> Self {
        let phase =
            proto::ContainerPhase::try_from(p.phase).unwrap_or(proto::ContainerPhase::Unspecified);
        WorkerContainerStatus {
            service: p.service,
            role: p.role,
            replica: p.index,
            state: phase_to_state_str(phase).to_string(),
            overlay_ip: None,
        }
    }
}

// ----------------------------------------------------------------------------
// WorkerResourceUsage <-> proto::NodeResourceUsage
// ----------------------------------------------------------------------------

impl From<WorkerResourceUsage> for proto::NodeResourceUsage {
    fn from(u: WorkerResourceUsage) -> Self {
        proto::NodeResourceUsage {
            cpu_used_cores: u.cpu_used,
            memory_used_bytes: u.memory_used_bytes,
            disk_used_bytes: 0,
        }
    }
}

impl From<proto::NodeResourceUsage> for WorkerResourceUsage {
    fn from(p: proto::NodeResourceUsage) -> Self {
        WorkerResourceUsage {
            cpu_used: p.cpu_used_cores,
            memory_used_bytes: p.memory_used_bytes,
            gpu_used: 0,
        }
    }
}

// ----------------------------------------------------------------------------
// WorkerStatusReport <-> proto::StatusReport
// ----------------------------------------------------------------------------

impl From<WorkerStatusReport> for proto::StatusReport {
    fn from(r: WorkerStatusReport) -> Self {
        proto::StatusReport {
            node_id: r.node_id,
            ts: Some(ts_ns_to_proto(r.ts_ns)),
            containers: r.containers.into_iter().map(Into::into).collect(),
            resources: Some(r.resources.into()),
            full_snapshot: false,
        }
    }
}

impl TryFrom<proto::StatusReport> for WorkerStatusReport {
    type Error = ConvertError;

    fn try_from(p: proto::StatusReport) -> Result<Self, Self::Error> {
        let resources_proto = p
            .resources
            .ok_or(ConvertError::MissingField("StatusReport.resources"))?;
        let ts_ns = p.ts.as_ref().map_or(0, proto_to_ts_ns);
        Ok(WorkerStatusReport {
            node_id: p.node_id,
            ts_ns,
            containers: p.containers.into_iter().map(Into::into).collect(),
            resources: resources_proto.into(),
        })
    }
}

// ----------------------------------------------------------------------------
// WorkerStatusAck <-> proto::StatusAck
// ----------------------------------------------------------------------------

impl From<WorkerStatusAck> for proto::StatusAck {
    fn from(a: WorkerStatusAck) -> Self {
        proto::StatusAck {
            next_ttl_secs: a.next_ttl_secs,
            accepted_revision: 0,
        }
    }
}

impl From<proto::StatusAck> for WorkerStatusAck {
    fn from(p: proto::StatusAck) -> Self {
        WorkerStatusAck {
            next_ttl_secs: p.next_ttl_secs,
        }
    }
}

// ----------------------------------------------------------------------------
// WorkerAssignmentEvent <-> proto::AssignmentEvent
// ----------------------------------------------------------------------------

impl TryFrom<proto::AssignmentEvent> for WorkerAssignmentEvent {
    type Error = ConvertError;

    fn try_from(p: proto::AssignmentEvent) -> Result<Self, Self::Error> {
        let event = p
            .event
            .ok_or(ConvertError::MissingField("AssignmentEvent.event"))?;
        let revision = p.revision;
        match event {
            proto::assignment_event::Event::Set(s) => Ok(WorkerAssignmentEvent::Set {
                service: s.service,
                assignments: vec![ScaleAssignment {
                    role: s.role,
                    indices: vec![s.index],
                }],
                revision,
            }),
            proto::assignment_event::Event::Delete(d) => Ok(WorkerAssignmentEvent::Delete {
                service: d.service,
                revision,
            }),
            proto::assignment_event::Event::Drain(_) => {
                Ok(WorkerAssignmentEvent::Drain { revision })
            }
        }
    }
}

impl TryFrom<WorkerAssignmentEvent> for proto::AssignmentEvent {
    type Error = ConvertError;

    /// Converts a single Rust event to a single proto event. For `Set` this
    /// requires exactly one role with exactly one index — see
    /// [`worker_assignment_event_to_protos`] for the fan-out helper used by
    /// the leader dispatcher.
    fn try_from(r: WorkerAssignmentEvent) -> Result<Self, Self::Error> {
        match r {
            WorkerAssignmentEvent::Set {
                service,
                assignments,
                revision,
            } => {
                if assignments.len() != 1 {
                    return Err(ConvertError::AmbiguousFanOut(
                        "WorkerAssignmentEvent::Set carried !=1 roles",
                    ));
                }
                let role_entry = assignments.into_iter().next().expect("len==1");
                if role_entry.indices.len() != 1 {
                    return Err(ConvertError::AmbiguousFanOut(
                        "WorkerAssignmentEvent::Set carried !=1 indices",
                    ));
                }
                let index = role_entry.indices[0];
                Ok(proto::AssignmentEvent {
                    revision,
                    ts: Some(systemtime_to_proto(SystemTime::now())),
                    event: Some(proto::assignment_event::Event::Set(proto::AssignmentSet {
                        service,
                        role: role_entry.role,
                        index,
                        spec_json: Vec::new(),
                    })),
                })
            }
            WorkerAssignmentEvent::Delete { service, revision } => Ok(proto::AssignmentEvent {
                revision,
                ts: Some(systemtime_to_proto(SystemTime::now())),
                event: Some(proto::assignment_event::Event::Delete(
                    proto::AssignmentDelete {
                        service,
                        role: String::new(),
                        index: 0,
                        force: false,
                    },
                )),
            }),
            WorkerAssignmentEvent::Drain { revision } => Ok(proto::AssignmentEvent {
                revision,
                ts: Some(systemtime_to_proto(SystemTime::now())),
                event: Some(proto::assignment_event::Event::Drain(
                    proto::AssignmentDrain { grace_secs: 0 },
                )),
            }),
        }
    }
}

/// Fan-out helper: explode one Rust [`WorkerAssignmentEvent::Set`] (which can
/// carry many role/index pairs) into one proto [`proto::AssignmentEvent`] per
/// `(role, index)` pair. `Delete` and `Drain` produce a single-element `Vec`.
///
/// The leader-side dispatcher uses this so that each proto event matches a
/// single container placement decision.
///
/// # Panics
///
/// This function never panics in practice — the internal `expect` only fires
/// if the single-event `TryFrom<WorkerAssignmentEvent> for proto::AssignmentEvent`
/// implementation ever rejects a `Delete` or `Drain` variant (it does not).
#[must_use]
pub fn worker_assignment_event_to_protos(e: WorkerAssignmentEvent) -> Vec<proto::AssignmentEvent> {
    match e {
        WorkerAssignmentEvent::Set {
            service,
            assignments,
            revision,
        } => {
            let mut out = Vec::new();
            for asg in assignments {
                for idx in asg.indices {
                    out.push(proto::AssignmentEvent {
                        revision,
                        ts: Some(systemtime_to_proto(SystemTime::now())),
                        event: Some(proto::assignment_event::Event::Set(proto::AssignmentSet {
                            service: service.clone(),
                            role: asg.role.clone(),
                            index: idx,
                            spec_json: Vec::new(),
                        })),
                    });
                }
            }
            out
        }
        e @ (WorkerAssignmentEvent::Delete { .. } | WorkerAssignmentEvent::Drain { .. }) => {
            // Total: single-event try_from cannot fail for these arms.
            vec![proto::AssignmentEvent::try_from(e).expect("Delete/Drain are infallible")]
        }
    }
}

// ----------------------------------------------------------------------------
// Tests
// ----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::net::SocketAddr;
    use zlayer_types::cluster::{
        ScaleAssignment, WorkerAssignmentEvent, WorkerContainerStatus, WorkerProfile,
        WorkerRegisterRequest, WorkerRegisterResponse, WorkerResourceUsage, WorkerStatusAck,
        WorkerStatusReport,
    };

    fn make_profile() -> WorkerProfile {
        WorkerProfile {
            // Round-tripping through proto drops api_addr (proto has no field
            // for it), so we use the same default the converter inserts.
            api_addr: "0.0.0.0:0".parse::<SocketAddr>().unwrap(),
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            labels: HashMap::from([("region".to_string(), "us-east".to_string())]),
            cpu_total: 8,
            memory_total_bytes: 16_000_000_000,
        }
    }

    #[test]
    fn round_trip_worker_register_request() {
        let rust = WorkerRegisterRequest {
            token: "tok123".to_string(),
            desired_node_id: Some(42),
            profile: make_profile(),
        };
        let p: proto::RegisterRequest = rust.clone().into();
        assert_eq!(p.bootstrap_token, "tok123");
        assert_eq!(p.desired_node_id, 42);
        let back = WorkerRegisterRequest::try_from(p).expect("from proto");
        assert_eq!(back.token, rust.token);
        assert_eq!(back.desired_node_id, rust.desired_node_id);
        assert_eq!(back.profile.os, rust.profile.os);
        assert_eq!(back.profile.arch, rust.profile.arch);
        assert_eq!(back.profile.labels, rust.profile.labels);
        assert_eq!(back.profile.cpu_total, rust.profile.cpu_total);
        assert_eq!(
            back.profile.memory_total_bytes,
            rust.profile.memory_total_bytes
        );
        assert_eq!(back.profile.api_addr, rust.profile.api_addr);
    }

    #[test]
    fn desired_node_id_zero_becomes_none() {
        let p = proto::RegisterRequest {
            bootstrap_token: "t".to_string(),
            desired_node_id: 0,
            profile: Some(make_profile().into()),
            csr_der: Vec::new(),
        };
        let r = WorkerRegisterRequest::try_from(p).expect("from proto");
        assert_eq!(r.desired_node_id, None);
    }

    #[test]
    fn missing_profile_errors() {
        let p = proto::RegisterRequest {
            bootstrap_token: "t".to_string(),
            desired_node_id: 1,
            profile: None,
            csr_der: Vec::new(),
        };
        let err = WorkerRegisterRequest::try_from(p).unwrap_err();
        assert_eq!(err, ConvertError::MissingField("RegisterRequest.profile"));
    }

    #[test]
    fn round_trip_register_response_lossy_fields_dropped() {
        let rust = WorkerRegisterResponse {
            node_id: 7,
            cluster_id: "drops".to_string(),
            heartbeat_ttl_secs: 30,
            heartbeat_grace_secs: 10,
            internal_token: "drops".to_string(),
        };
        let p: proto::RegisterResponse = rust.into();
        let back: WorkerRegisterResponse = p.into();
        assert_eq!(back.node_id, 7);
        assert_eq!(back.heartbeat_ttl_secs, 30);
        assert_eq!(back.heartbeat_grace_secs, 10);
        // Lossy: cluster_id + internal_token do not survive proto.
        assert!(back.cluster_id.is_empty());
        assert!(back.internal_token.is_empty());
    }

    #[test]
    fn round_trip_status_report() {
        let rust = WorkerStatusReport {
            node_id: 5,
            ts_ns: 1_700_000_000_123_456_789,
            containers: vec![WorkerContainerStatus {
                service: "web".to_string(),
                role: "primary".to_string(),
                replica: 2,
                state: "running".to_string(),
                overlay_ip: None,
            }],
            resources: WorkerResourceUsage {
                cpu_used: 1.5,
                memory_used_bytes: 1_000_000,
                gpu_used: 0,
            },
        };
        let p: proto::StatusReport = rust.clone().into();
        let back = WorkerStatusReport::try_from(p).expect("from proto");
        assert_eq!(back.node_id, rust.node_id);
        assert_eq!(back.ts_ns, rust.ts_ns);
        assert_eq!(back.containers.len(), 1);
        assert_eq!(back.containers[0].service, "web");
        assert_eq!(back.containers[0].role, "primary");
        assert_eq!(back.containers[0].replica, 2);
        assert_eq!(back.containers[0].state, "running");
        assert!((back.resources.cpu_used - 1.5).abs() < 1e-9);
        assert_eq!(back.resources.memory_used_bytes, 1_000_000);
    }

    #[test]
    fn status_ack_round_trip() {
        let rust = WorkerStatusAck { next_ttl_secs: 42 };
        let p: proto::StatusAck = rust.into();
        assert_eq!(p.next_ttl_secs, 42);
        let back: WorkerStatusAck = p.into();
        assert_eq!(back.next_ttl_secs, 42);
    }

    #[test]
    fn assignment_event_set_single_round_trip() {
        let rust = WorkerAssignmentEvent::Set {
            service: "web".to_string(),
            assignments: vec![ScaleAssignment {
                role: "primary".to_string(),
                indices: vec![3],
            }],
            revision: 100,
        };
        let p = proto::AssignmentEvent::try_from(rust).expect("to proto");
        assert_eq!(p.revision, 100);
        match p.event {
            Some(proto::assignment_event::Event::Set(ref s)) => {
                assert_eq!(s.service, "web");
                assert_eq!(s.role, "primary");
                assert_eq!(s.index, 3);
            }
            _ => panic!("expected Set"),
        }
        let back = WorkerAssignmentEvent::try_from(p).expect("from proto");
        match back {
            WorkerAssignmentEvent::Set {
                service,
                assignments,
                revision,
            } => {
                assert_eq!(service, "web");
                assert_eq!(revision, 100);
                assert_eq!(assignments.len(), 1);
                assert_eq!(assignments[0].role, "primary");
                assert_eq!(assignments[0].indices, vec![3]);
            }
            _ => panic!("expected Set"),
        }
    }

    #[test]
    fn assignment_event_set_multi_fans_out() {
        let rust = WorkerAssignmentEvent::Set {
            service: "web".to_string(),
            assignments: vec![
                ScaleAssignment {
                    role: "primary".to_string(),
                    indices: vec![0, 1],
                },
                ScaleAssignment {
                    role: "replica".to_string(),
                    indices: vec![0],
                },
            ],
            revision: 5,
        };
        let protos = worker_assignment_event_to_protos(rust);
        assert_eq!(protos.len(), 3);
        // Single-event try_from must reject this case.
        let rust2 = WorkerAssignmentEvent::Set {
            service: "web".to_string(),
            assignments: vec![
                ScaleAssignment {
                    role: "a".to_string(),
                    indices: vec![0],
                },
                ScaleAssignment {
                    role: "b".to_string(),
                    indices: vec![0],
                },
            ],
            revision: 1,
        };
        let err = proto::AssignmentEvent::try_from(rust2).unwrap_err();
        assert!(matches!(err, ConvertError::AmbiguousFanOut(_)));
    }

    #[test]
    fn assignment_event_delete_round_trip() {
        let rust = WorkerAssignmentEvent::Delete {
            service: "web".to_string(),
            revision: 9,
        };
        let p = proto::AssignmentEvent::try_from(rust).expect("to proto");
        assert!(matches!(
            p.event,
            Some(proto::assignment_event::Event::Delete(_))
        ));
        let back = WorkerAssignmentEvent::try_from(p).expect("from proto");
        match back {
            WorkerAssignmentEvent::Delete { service, revision } => {
                assert_eq!(service, "web");
                assert_eq!(revision, 9);
            }
            _ => panic!("expected Delete"),
        }
    }

    #[test]
    fn assignment_event_drain_round_trip() {
        let rust = WorkerAssignmentEvent::Drain { revision: 11 };
        let p = proto::AssignmentEvent::try_from(rust).expect("to proto");
        assert!(matches!(
            p.event,
            Some(proto::assignment_event::Event::Drain(_))
        ));
        let back = WorkerAssignmentEvent::try_from(p).expect("from proto");
        match back {
            WorkerAssignmentEvent::Drain { revision } => assert_eq!(revision, 11),
            _ => panic!("expected Drain"),
        }
    }

    #[test]
    fn assignment_event_missing_oneof_errors() {
        let p = proto::AssignmentEvent {
            revision: 1,
            ts: None,
            event: None,
        };
        let err = WorkerAssignmentEvent::try_from(p).unwrap_err();
        assert_eq!(err, ConvertError::MissingField("AssignmentEvent.event"));
    }
}
