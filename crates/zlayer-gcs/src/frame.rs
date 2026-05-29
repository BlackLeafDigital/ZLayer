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

/// Bit flag in `MessageType.high` marking a response (bit 28).
pub const RESPONSE_TYPE_OFFSET: u32 = 0x1000_0000;

/// Category for compute-system / container RPCs (high 8 bits = `0x00100000`).
pub const CATEGORY_COMPUTE_SYSTEM: u32 = 0x0010_0000;

/// Category for stream-level events (notifications, stdout/stderr).
pub const CATEGORY_STREAM: u32 = 0x0040_0000;

/// RPC type codes (low 24 bits). High bits are OR'd from the category.
/// See hcsshim `internal/gcs/prot/protocol.go::MsgType` constants.
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RpcMessageType {
    NegotiateProtocol = 0x0001,
    Create = 0x0002,
    Start = 0x0003,
    Shutdown = 0x0004,
    ExecuteProcess = 0x0005,
    WaitForProcess = 0x0006,
    SignalProcess = 0x0007,
    GetProperties = 0x0008,
    ModifySettings = 0x0009,
    NegotiateProtocolV2 = 0x000A,
}

impl RpcMessageType {
    /// Encode as the on-wire u32 (compute-system category | low type code).
    #[must_use]
    pub const fn as_request_type(self) -> u32 {
        CATEGORY_COMPUTE_SYSTEM | (self as u32)
    }

    /// Encode as the expected on-wire response type (same as request | response bit).
    #[must_use]
    pub const fn as_response_type(self) -> u32 {
        self.as_request_type() | RESPONSE_TYPE_OFFSET
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
        assert_eq!(resp, req | RESPONSE_TYPE_OFFSET);
        assert_eq!(req & CATEGORY_COMPUTE_SYSTEM, CATEGORY_COMPUTE_SYSTEM);
    }
}
