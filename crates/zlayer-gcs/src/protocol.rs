//! GCS RPC message body types.
//!
//! Every RPC over the GCS bridge is a `(frame_header, body)` pair where the
//! body is a UTF-8 JSON document with shapes documented here. Mirrors
//! hcsshim's `internal/gcs/prot/protocol.go::Msg{Negotiate,Create,...}`.
//!
//! The frame header carries the message id + type code (see [`crate::frame`]);
//! these types only describe the payload.

use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize, Serializer};
use uuid::Uuid;

/// hcsshim's `AnyInString` — a JSON value that is wire-encoded as a JSON
/// **string** whose content is the escaped JSON of the inner value
/// (double-encoded).
///
/// Mirrors hcsshim's `internal/gcs/prot/protocol.go`:
/// ```go
/// type AnyInString struct { Value interface{} }
/// func (a *AnyInString) MarshalText() ([]byte, error) { return json.Marshal(a.Value) }
/// ```
/// Because `AnyInString` implements `MarshalText`, the outer `json.Marshal`
/// emits the field as a JSON string containing the escaped JSON of the inner
/// value. The inbox guest GCS uses a STRICT JSON unmarshaller (hcsshim issue
/// #2714) that rejects unknown/misnamed fields and tears down the VM, so the
/// double-encoding must match exactly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnyInString(pub serde_json::Value);

impl AnyInString {
    /// Wrap a JSON value for double-encoded `AnyInString` serialization.
    #[must_use]
    pub const fn new(value: serde_json::Value) -> Self {
        Self(value)
    }

    /// Borrow the inner JSON value.
    #[must_use]
    pub const fn value(&self) -> &serde_json::Value {
        &self.0
    }

    /// Consume and return the inner JSON value.
    #[must_use]
    pub fn into_value(self) -> serde_json::Value {
        self.0
    }
}

impl From<serde_json::Value> for AnyInString {
    fn from(value: serde_json::Value) -> Self {
        Self(value)
    }
}

impl Serialize for AnyInString {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Serialize the inner value to a JSON string, then serialize THAT
        // string — yielding a double-encoded `"{...escaped...}"` on the wire,
        // exactly as hcsshim's `MarshalText` does.
        let inner =
            serde_json::to_string(&self.0).map_err(<S::Error as serde::ser::Error>::custom)?;
        serializer.serialize_str(&inner)
    }
}

impl<'de> Deserialize<'de> for AnyInString {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        // Read the outer JSON string, then parse its escaped JSON content
        // back into a `serde_json::Value`.
        let s = String::deserialize(deserializer)?;
        let value = serde_json::from_str(&s).map_err(de::Error::custom)?;
        Ok(Self(value))
    }
}

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
    /// On failure, the in-guest GCS populates this with a JSON **array** of
    /// `ErrorRecord` objects (e.g.
    /// `[{"Result":-1070137077,"Message":"...","ModuleName":"vmcomputeagent.exe",...}]`).
    /// It is kept as a raw [`serde_json::Value`] so callers can render or parse
    /// it on demand; a `String` field here fails to deserialize the guest's
    /// array shape with "invalid type: sequence, expected a string".
    #[serde(
        default,
        rename = "ErrorRecords",
        skip_serializing_if = "serde_json::Value::is_null"
    )]
    pub error_records: serde_json::Value,
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
    /// Whether the guest advertises support for the `ModifyServiceSettings`
    /// RPC (hcsshim's `GcsCapabilities.ModifyServiceSettingsSupported`). The
    /// Server 2025 inbox GCS does NOT advertise this, and sending the RPC
    /// (e.g. `StartLogForwarding`) yields `Message Type 270532865 unknown`.
    /// hcsshim gates `StartLogForwarding` on this flag; so do we.
    #[serde(default)]
    pub modify_service_settings_supported: bool,
}

/// `RpcCreate` — create the COMPUTE-system root.
///
/// For a cold-start UVM this is the null container `00000000-...` with a
/// `ContainerConfig` block describing the UVM container kind
/// (`{"SystemType":"Container"}`).
///
/// The `ContainerConfig` field is hcsshim's `AnyInString`: it is wire-encoded
/// as a JSON **string** containing the escaped JSON of the config value
/// (double-encoded). The field name is literally `ContainerConfig`, NOT
/// `Settings` — the inbox guest GCS strictly rejects any other shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct CreateRequest {
    #[serde(flatten)]
    pub base: RequestBase,
    /// Container creation config — double-encoded JSON string on the wire.
    #[serde(rename = "ContainerConfig")]
    pub container_config: AnyInString,
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
    fn any_in_string_double_encodes_as_json_string() {
        let v = AnyInString::new(json!({"SystemType": "Container"}));
        let s = serde_json::to_string(&v).unwrap();
        // Top-level serialization is a JSON STRING (starts/ends with a quote),
        // and the inner braces/quotes are escaped.
        assert_eq!(s, "\"{\\\"SystemType\\\":\\\"Container\\\"}\"");
        // Round-trips back to the same value.
        let back: AnyInString = serde_json::from_str(&s).unwrap();
        assert_eq!(back, v);
    }

    #[test]
    fn create_request_container_config_is_escaped_string() {
        let req = CreateRequest {
            base: RequestBase {
                activity_id: Uuid::nil(),
                container_id: "00000000-0000-0000-0000-000000000000".into(),
            },
            container_config: AnyInString::new(json!({
                "SystemType": "Container",
            })),
        };
        let s = serde_json::to_string(&req).unwrap();
        // Field name is literally `ContainerConfig` (NOT `Settings`), and its
        // value is a STRING containing escaped JSON (double-encoded).
        assert!(
            s.contains("\"ContainerConfig\":\"{\\\"SystemType\\\":\\\"Container\\\"}\""),
            "expected double-encoded ContainerConfig string, got: {s}"
        );
        assert!(
            !s.contains("\"Settings\""),
            "must not emit a Settings field"
        );
        assert!(s.contains("\"ContainerId\":\"00000000-0000-0000-0000-000000000000\""));
        assert!(s.contains("\"ActivityId\""));

        // Full round-trip back into a CreateRequest.
        let back: CreateRequest = serde_json::from_str(&s).unwrap();
        assert_eq!(back.base.container_id, req.base.container_id);
        assert_eq!(back.container_config, req.container_config);
    }

    #[test]
    fn response_base_parses_error_records_array() {
        // The in-guest GCS sends `ErrorRecords` as a JSON ARRAY. Previously
        // `error_records: String` failed this with "invalid type: sequence,
        // expected a string". It must now deserialize into a Value array.
        let wire = r#"{
            "Result": -1070137077,
            "ActivityId": "00000000-0000-0000-0000-000000000000",
            "ErrorRecords": [
                {
                    "Result": -1070137077,
                    "Message": "Message Type 270532865 unknown (215 byte)",
                    "ModuleName": "vmcomputeagent.exe"
                }
            ]
        }"#;
        let base: ResponseBase = serde_json::from_str(wire).unwrap();
        assert_eq!(base.result, -1_070_137_077);
        assert!(
            base.error_records.is_array(),
            "error_records must be an array"
        );
        assert_eq!(
            base.error_records[0]["Message"],
            json!("Message Type 270532865 unknown (215 byte)")
        );

        // A response with no ErrorRecords still parses, leaving a null Value
        // that is skipped on re-serialization.
        let empty: ResponseBase = serde_json::from_str(r#"{"Result":0}"#).unwrap();
        assert!(empty.error_records.is_null());
        let s = serde_json::to_string(&empty).unwrap();
        assert!(!s.contains("ErrorRecords"), "null records must be skipped");
    }

    #[test]
    fn protocol_support_parses_modify_service_settings_supported() {
        // Guest that DOES advertise the capability.
        let with: ProtocolSupport =
            serde_json::from_str(r#"{"ModifyServiceSettingsSupported": true}"#).unwrap();
        assert!(with.modify_service_settings_supported);
        // Guest (Server 2025 inbox) that omits it → defaults to false.
        let without: ProtocolSupport = serde_json::from_str(r"{}").unwrap();
        assert!(!without.modify_service_settings_supported);
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
