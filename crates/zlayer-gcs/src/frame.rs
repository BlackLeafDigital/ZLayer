//! GCS protocol frame codec.
//!
//! Each GCS message on the wire is a 16-byte little-endian header followed
//! by a UTF-8 JSON payload. Matches hcsshim's
//! `internal/gcs/prot/protocol.go::HdrLength`/`MessageHeader`/`MessageType`
//! constants.

use crate::error::{GcsError, GcsResult};

/// Fixed header length in bytes.
pub const HEADER_LEN: usize = 16;

/// Maximum payload we accept on decode — guards against absurd `Size` values
/// from a malicious or buggy guest. 4 MiB is far above any real GCS message.
pub const MAX_PAYLOAD_LEN: usize = 4 * 1024 * 1024;

/// Top 4 bits of `MessageType` distinguish request / response / notify.
/// Mirrors hcsshim `internal/gcs/prot/protocol.go::MsgType{Request,Response,Notify,Mask}`.
pub const MSG_TYPE_REQUEST: u32 = 0x1000_0000;
pub const MSG_TYPE_RESPONSE: u32 = 0x2000_0000;
pub const MSG_TYPE_NOTIFY: u32 = 0x3000_0000;
pub const MSG_TYPE_MASK: u32 = 0xF000_0000;

/// Category for compute-system / container RPCs. Mirrors hcsshim
/// `ComputeSystem = 0x00100000`.
pub const CATEGORY_COMPUTE_SYSTEM: u32 = 0x0010_0000;

/// Category for compute-service RPCs (e.g. log forwarding). Mirrors hcsshim
/// `ComputeService = 0x00200000`.
pub const CATEGORY_COMPUTE_SERVICE: u32 = 0x0020_0000;

/// RPC type codes for the `ComputeSystem` category.
///
/// Each value already encodes `(iota+1)<<8 | 1` per hcsshim's
/// `RPCProc = Category | (iota+1)<<8 | 1` formula in
/// `internal/gcs/prot/protocol.go`, so a `NegotiateProtocol` REQUEST frame's
/// wire `type` is exactly `MSG_TYPE_REQUEST | CATEGORY_COMPUTE_SYSTEM |
/// (rpc as u32)` = `0x10100B01`. An earlier iteration of this enum used
/// `0x0001..=0x000A` and was missing both the per-RPC `(iota+1)<<8` byte
/// AND the `MSG_TYPE_REQUEST` marker, causing the in-guest GCS to close
/// the bridge the moment it saw a frame with an unrecognized type
/// (verified via `gcs-bridge-reader: header read failed after 0 frame(s):
/// bridge closed` against `nanoserver:ltsc2022` with the dep-override
/// applied).
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RpcMessageType {
    Create = 0x0101,
    Start = 0x0201,
    ShutdownGraceful = 0x0301,
    ShutdownForced = 0x0401,
    ExecuteProcess = 0x0501,
    WaitForProcess = 0x0601,
    SignalProcess = 0x0701,
    ResizeConsole = 0x0801,
    GetProperties = 0x0901,
    ModifySettings = 0x0A01,
    NegotiateProtocol = 0x0B01,
    DumpStacks = 0x0C01,
    DeleteContainerState = 0x0D01,
    UpdateContainer = 0x0E01,
    LifecycleNotification = 0x0F01,
    /// `RPCModifyServiceSettings` — the ONLY RPC in hcsshim's `ComputeService`
    /// category (`internal/gcs/prot/protocol.go`):
    /// `RPCModifyServiceSettings RPCProc = ComputeService | (iota+1)<<8 | 1`.
    /// `iota` RESETS to 0 in that second `const` block, so the per-RPC byte is
    /// `(0+1)<<8 = 0x100` and the low 16 bits are `0x0101` — identical to
    /// `Create`'s low bits, but it lives in a DIFFERENT category
    /// (`ComputeService = 0x0020_0000`, not `ComputeSystem = 0x0010_0000`).
    /// Used to drive the in-guest GCS log-forward service
    /// (`internal/uvm/log_wcow.go`). Because the category differs, the
    /// discriminant alone cannot be OR'd with `CATEGORY_COMPUTE_SYSTEM` —
    /// [`RpcMessageType::as_request_type`] special-cases it. The discriminant
    /// is offset into a private range so it does not numerically collide with
    /// `Create = 0x0101` inside the Rust enum.
    ModifyServiceSettings = 0x1_0101,
}

impl RpcMessageType {
    /// hcsshim message category for this RPC. Every container/system RPC is
    /// `ComputeSystem`; only [`RpcMessageType::ModifyServiceSettings`] is
    /// `ComputeService`.
    #[must_use]
    const fn category(self) -> u32 {
        match self {
            Self::ModifyServiceSettings => CATEGORY_COMPUTE_SERVICE,
            _ => CATEGORY_COMPUTE_SYSTEM,
        }
    }

    /// The low-16-bit `(iota+1)<<8 | 1` RPC code, stripped of the synthetic
    /// enum-disambiguation offset carried by [`RpcMessageType::ModifyServiceSettings`].
    #[must_use]
    const fn proc_code(self) -> u32 {
        (self as u32) & 0xFFFF
    }

    /// Encode as the on-wire request `type` u32: `MSG_TYPE_REQUEST | category |
    /// rpc`.
    #[must_use]
    pub const fn as_request_type(self) -> u32 {
        MSG_TYPE_REQUEST | self.category() | self.proc_code()
    }

    /// Encode as the expected on-wire response `type` u32:
    /// `MSG_TYPE_RESPONSE | category | rpc`.
    #[must_use]
    pub const fn as_response_type(self) -> u32 {
        MSG_TYPE_RESPONSE | self.category() | self.proc_code()
    }
}

/// Parsed frame header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameHeader {
    pub r#type: u32,
    pub size: u32,
    pub message_id: u64,
}

/// Encode a frame: writes `HEADER_LEN + payload.len()` bytes into `out`
/// (preallocates / extends as needed).
///
/// # Panics
/// Panics if `HEADER_LEN + payload.len()` does not fit in a `u32` (i.e. the
/// payload is ~4 GiB). Real GCS messages are bounded by [`MAX_PAYLOAD_LEN`]
/// (4 MiB), so this is a programmer-error guard rather than a runtime path.
pub fn encode_frame(r#type: u32, message_id: u64, payload: &[u8], out: &mut Vec<u8>) {
    let total =
        u32::try_from(HEADER_LEN + payload.len()).expect("frame total length must fit in u32");
    out.clear();
    out.reserve(HEADER_LEN + payload.len());
    out.extend_from_slice(&r#type.to_le_bytes());
    out.extend_from_slice(&total.to_le_bytes());
    out.extend_from_slice(&message_id.to_le_bytes());
    out.extend_from_slice(payload);
}

/// Decode just the header from a 16-byte slice. Validates `size >= HEADER_LEN`
/// and `size <= HEADER_LEN + MAX_PAYLOAD_LEN`.
pub fn decode_header(bytes: &[u8; HEADER_LEN]) -> GcsResult<FrameHeader> {
    // The 4/8-byte sub-slices are guaranteed to fit into the fixed-size arrays
    // because `bytes` is a `&[u8; HEADER_LEN]` (HEADER_LEN == 16). `expect` is
    // unreachable but preferred over `unwrap` per crate lint floor.
    let r#type = u32::from_le_bytes(
        bytes[0..4]
            .try_into()
            .expect("static 4-byte slice of 16-byte header"),
    );
    let size = u32::from_le_bytes(
        bytes[4..8]
            .try_into()
            .expect("static 4-byte slice of 16-byte header"),
    );
    let message_id = u64::from_le_bytes(
        bytes[8..16]
            .try_into()
            .expect("static 8-byte slice of 16-byte header"),
    );
    if (size as usize) < HEADER_LEN {
        return Err(GcsError::Protocol(format!(
            "frame size {size} < header length {HEADER_LEN}"
        )));
    }
    if (size as usize) > HEADER_LEN + MAX_PAYLOAD_LEN {
        return Err(GcsError::Protocol(format!(
            "frame size {size} exceeds MAX_PAYLOAD_LEN+header={}",
            HEADER_LEN + MAX_PAYLOAD_LEN
        )));
    }
    Ok(FrameHeader {
        r#type,
        size,
        message_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_empty_payload() {
        let mut buf = Vec::new();
        encode_frame(0x0010_0001, 42, b"", &mut buf);
        assert_eq!(buf.len(), HEADER_LEN);
        let hdr_bytes: [u8; HEADER_LEN] = buf[..HEADER_LEN]
            .try_into()
            .expect("buf has HEADER_LEN bytes after encode_frame");
        let h = decode_header(&hdr_bytes).unwrap();
        assert_eq!(h.r#type, 0x0010_0001);
        assert_eq!(h.size as usize, HEADER_LEN);
        assert_eq!(h.message_id, 42);
    }

    #[test]
    fn round_trip_with_payload() {
        let payload = br#"{"hello":"world"}"#;
        let mut buf = Vec::new();
        encode_frame(0x1010_0001, 99, payload, &mut buf);
        assert_eq!(buf.len(), HEADER_LEN + payload.len());
        let hdr_bytes: [u8; HEADER_LEN] = buf[..HEADER_LEN]
            .try_into()
            .expect("buf has HEADER_LEN bytes after encode_frame");
        let h = decode_header(&hdr_bytes).unwrap();
        assert_eq!(h.size as usize, HEADER_LEN + payload.len());
        assert_eq!(&buf[HEADER_LEN..], payload);
    }

    #[test]
    fn decode_rejects_undersized_size_field() {
        let mut bytes = [0u8; HEADER_LEN];
        bytes[4..8].copy_from_slice(&8u32.to_le_bytes()); // size=8 < HEADER_LEN=16
        let err = decode_header(&bytes).unwrap_err();
        assert!(matches!(err, GcsError::Protocol(_)));
    }

    #[test]
    fn decode_rejects_oversized_size_field() {
        let mut bytes = [0u8; HEADER_LEN];
        let bad_size: u32 =
            u32::try_from(HEADER_LEN + MAX_PAYLOAD_LEN + 1).expect("test constant fits in u32");
        bytes[4..8].copy_from_slice(&bad_size.to_le_bytes());
        let err = decode_header(&bytes).unwrap_err();
        assert!(matches!(err, GcsError::Protocol(_)));
    }

    #[test]
    fn request_vs_response_type_bit() {
        let req = RpcMessageType::Create.as_request_type();
        let resp = RpcMessageType::Create.as_response_type();
        // Request: 0x10100101, Response: 0x20100101 — differ only in the
        // top 4 bits per hcsshim's `MsgTypeMask`.
        assert_eq!(req & MSG_TYPE_MASK, MSG_TYPE_REQUEST);
        assert_eq!(resp & MSG_TYPE_MASK, MSG_TYPE_RESPONSE);
        assert_eq!(req & !MSG_TYPE_MASK, resp & !MSG_TYPE_MASK);
        assert_eq!(req & CATEGORY_COMPUTE_SYSTEM, CATEGORY_COMPUTE_SYSTEM);
    }

    /// Pin the on-wire `NegotiateProtocol` REQUEST type to the exact
    /// 32-bit value hcsshim's in-guest GCS expects (`0x10100B01`). If
    /// this number changes, every WCOW UVM under `nanoserver:ltsc2022`
    /// will reject the connection at the first frame.
    #[test]
    fn negotiate_protocol_wire_type_pinned() {
        assert_eq!(
            RpcMessageType::NegotiateProtocol.as_request_type(),
            0x1010_0B01
        );
        assert_eq!(
            RpcMessageType::NegotiateProtocol.as_response_type(),
            0x2010_0B01
        );
    }

    /// Pin the on-wire `ModifyServiceSettings` REQUEST/RESPONSE types. This RPC
    /// lives in hcsshim's `ComputeService` category (not `ComputeSystem`), so
    /// its wire value is `MSG_TYPE_* | 0x0020_0000 | 0x0101`. The Rust enum
    /// discriminant carries a synthetic `0x1_0000` offset to avoid colliding
    /// with `Create = 0x0101`; `proc_code()` must strip it so the wire bytes
    /// are exactly `0x1020_0101` (request) / `0x2020_0101` (response). If this
    /// drifts, the in-guest log-forward service will reject the RPC and the
    /// guest GCS log will never reach the host.
    #[test]
    fn modify_service_settings_wire_type_pinned() {
        let req = RpcMessageType::ModifyServiceSettings.as_request_type();
        let resp = RpcMessageType::ModifyServiceSettings.as_response_type();
        assert_eq!(req, 0x1020_0101);
        assert_eq!(resp, 0x2020_0101);
        // Category bits must be ComputeService, NOT ComputeSystem.
        assert_eq!(req & CATEGORY_COMPUTE_SERVICE, CATEGORY_COMPUTE_SERVICE);
        assert_eq!(req & CATEGORY_COMPUTE_SYSTEM, 0);
        // No synthetic enum-offset bits leak onto the wire.
        assert_eq!(req & 0x1_0000, 0);
    }
}
