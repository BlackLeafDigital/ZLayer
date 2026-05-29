//! GCS RPC message body types.
//!
//! Every RPC over the GCS bridge is a `(frame_header, body)` pair where the
//! body is a UTF-8 JSON document with shapes documented here. Mirrors
//! hcsshim's `internal/gcs/prot/protocol.go::Msg{Negotiate,Create,...}`.
//!
//! The frame header carries the message id + type code (see [`crate::frame`]);
//! these types only describe the payload.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Base fields present on every request body.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct RequestBase {
    pub activity_id: Uuid,
    pub container_id: String,
}

/// Base fields present on every response body.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct ResponseBase {
    #[serde(default)]
    pub activity_id: Uuid,
    /// HRESULT i32 — 0 = success, negative = failure. Matches Windows
    /// HRESULT layout (top bit = severity).
    #[serde(default)]
    pub result: i32,
    #[serde(default)]
    pub error_message: String,
    /// On failure, hcsshim populates this with a JSON-encoded array of
    /// `ErrorRecord` objects. We expose it as a raw string; callers that
    /// need parsed records parse on demand.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error_records: String,
}

/// `RpcNegotiateProtocol` request — proposes a min/max protocol version
/// the host supports. The guest responds with the version chosen and a
/// capabilities block.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct NegotiateProtocolRequest {
    #[serde(flatten)]
    pub base: RequestBase,
    pub minimum_version: u32,
    pub maximum_version: u32,
}

/// `RpcNegotiateProtocol` response.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct NegotiateProtocolResponse {
    #[serde(flatten)]
    pub base: ResponseBase,
    pub version: u32,
    pub capabilities: ProtocolSupport,
}

/// Capability block returned in the `NegotiateProtocol` response.
///
/// Field set is dictated by hcsshim's `ProtocolSupport` — we cannot collapse
/// the bool flags into an enum without diverging from the wire format.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct ProtocolSupport {
    #[serde(default)]
    pub protocol_version: u32,
    #[serde(default)]
    pub send_host_create_message: bool,
    #[serde(default)]
    pub send_host_start_message: bool,
    #[serde(default)]
    pub hv_socket_config_on_startup: bool,
    #[serde(default)]
    pub send_lifecycle_notifications: bool,
}

/// `RpcCreate` — create the COMPUTE-system root. For a cold-start UVM this
/// is the null container `00000000-...` with a `Settings` block describing
/// the UVM container kind (`{"SystemType":"Container"}`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct CreateRequest {
    #[serde(flatten)]
    pub base: RequestBase,
    /// Container creation settings — JSON value, schema varies by `SystemType`.
    pub settings: serde_json::Value,
}

pub type CreateResponse = ResponseBase;

/// `RpcStart` — start a previously-created container. No body beyond the base.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct StartRequest {
    #[serde(flatten)]
    pub base: RequestBase,
}

pub type StartResponse = ResponseBase;

/// `RpcShutdown` — graceful shutdown.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ShutdownRequest {
    #[serde(flatten)]
    pub base: RequestBase,
}

pub type ShutdownResponse = ResponseBase;

/// `RpcModifySettings` — hot-add/remove/update a setting on a running
/// compute system (VSMB share, SCSI attachment, network endpoint, ...).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ModifySettingsRequest {
    #[serde(flatten)]
    pub base: RequestBase,
    /// The `ModifySettingRequest` (singular) per hcsshim — a JSON value
    /// with `ResourcePath`, `RequestType`, `Settings` fields.
    pub request: serde_json::Value,
}

pub type ModifySettingsResponse = ResponseBase;

/// `RpcExecuteProcess` — run a process inside the hosted container.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ExecuteProcessRequest {
    #[serde(flatten)]
    pub base: RequestBase,
    /// `ProcessParameters` JSON: `{CommandLine, User, WorkingDirectory,
    /// CreateStdInPipe, CreateStdOutPipe, CreateStdErrPipe, ...}`.
    pub settings: ExecuteProcessSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ExecuteProcessSettings {
    pub process_parameters: serde_json::Value,
    /// Optional hvsock-listener GUIDs for stdio redirection. When `None`,
    /// the guest uses pipes within the container.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdio_relay_settings: Option<StdioRelaySettings>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct StdioRelaySettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdin_pipe: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdout_pipe: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stderr_pipe: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct ExecuteProcessResponse {
    #[serde(flatten)]
    pub base: ResponseBase,
    /// PID of the spawned process inside the guest.
    #[serde(default)]
    pub process_id: u32,
}

/// `RpcWaitForProcess` — wait for an `ExecuteProcess`-spawned PID to exit.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct WaitForProcessRequest {
    #[serde(flatten)]
    pub base: RequestBase,
    pub process_id: u32,
    /// Timeout in milliseconds. `u32::MAX` for "wait forever".
    pub timeout_in_ms: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct WaitForProcessResponse {
    #[serde(flatten)]
    pub base: ResponseBase,
    #[serde(default)]
    pub exit_code: u32,
}

/// `RpcSignalProcess` — send a signal to a spawned process.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SignalProcessRequest {
    #[serde(flatten)]
    pub base: RequestBase,
    pub process_id: u32,
    pub options: serde_json::Value,
}

pub type SignalProcessResponse = ResponseBase;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn negotiate_request_round_trip_pascal_case() {
        let req = NegotiateProtocolRequest {
            base: RequestBase {
                activity_id: Uuid::nil(),
                container_id: "abc".into(),
            },
            minimum_version: 1,
            maximum_version: 4,
        };
        let s = serde_json::to_string(&req).unwrap();
        assert!(s.contains("\"MinimumVersion\":1"));
        assert!(s.contains("\"MaximumVersion\":4"));
        assert!(s.contains("\"ContainerId\":\"abc\""));
        assert!(s.contains("\"ActivityId\""));
        let _back: NegotiateProtocolRequest = serde_json::from_str(&s).unwrap();
    }

    #[test]
    fn response_default_carries_zero_hresult() {
        let resp = NegotiateProtocolResponse::default();
        assert_eq!(resp.base.result, 0);
        assert!(resp.base.error_message.is_empty());
        let s = serde_json::to_string(&resp).unwrap();
        let back: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(back["Result"], json!(0));
    }

    #[test]
    fn execute_process_settings_optional_stdio_relay() {
        let s = ExecuteProcessSettings {
            process_parameters: json!({"CommandLine": "cmd /c ver"}),
            stdio_relay_settings: None,
        };
        let json_s = serde_json::to_string(&s).unwrap();
        assert!(!json_s.contains("StdioRelaySettings"), "absent when None");
    }
}
