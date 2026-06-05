//! vsock wire protocol shared by the host (`zlayer-agent`) and the in-guest
//! PID1 agent.
//!
//! # Framing
//!
//! Every message on the wire is a single length-prefixed frame:
//!
//! ```text
//! +-----------------+--------+------------------------+
//! | u32 length (LE) | u8 tag | postcard payload bytes |
//! +-----------------+--------+------------------------+
//!         4 bytes     1 byte    (length - 1) bytes
//! ```
//!
//! `length` counts the tag byte plus the payload, i.e. everything after the
//! 4-byte length field. The payload is a [`postcard`]-serialized representation
//! of the message variant's fields. `postcard` is chosen because it is tiny,
//! `no_std`/`alloc`-friendly, and links cleanly into a static musl agent.
//!
//! The framing is deliberately codec-agnostic at the read/write boundary:
//! [`read_frame`]/[`write_frame`] operate over any [`std::io::Read`]/
//! [`std::io::Write`] (e.g. an `AF_VSOCK` stream socket, a pipe, or an
//! in-memory buffer in tests), so this module has **no** Linux-only
//! dependencies and compiles on every host platform.

use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};

/// Fixed vsock control port the guest agent listens on. The host connects to
/// `(guest_cid, CONTROL_PORT)` to drive the workload.
pub const CONTROL_PORT: u32 = 1024;

/// Maximum frame body (tag + payload) we will accept on read, as a denial-of-
/// service guard against a corrupt or malicious length prefix. 16 MiB is far
/// larger than any control message or stdout/stderr chunk we emit, while still
/// bounding a single allocation.
pub const MAX_FRAME_LEN: u32 = 16 * 1024 * 1024;

/// Wire message exchanged between the host and the in-guest agent.
///
/// The discriminant order here is **load-bearing**: it defines the on-wire tag
/// byte assigned by [`Msg::tag`] and validated by [`decode`]. Never reorder
/// existing variants — only append.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Msg {
    /// Host → guest: run the container entrypoint as the workload's primary
    /// (PID-1-of-container) process.
    Run {
        /// Argument vector; `argv[0]` is the program to execute.
        argv: Vec<String>,
        /// Environment as `(KEY, VALUE)` pairs.
        env: Vec<(String, String)>,
        /// Working directory inside the container root, if any.
        cwd: Option<String>,
        /// User id to drop to before exec.
        uid: u32,
        /// Group id to drop to before exec.
        gid: u32,
    },
    /// Host → guest: spawn an additional process alongside a running workload
    /// (the `exec` path), entering the workload's namespaces.
    Exec {
        /// Argument vector; `argv[0]` is the program to execute.
        argv: Vec<String>,
        /// Environment as `(KEY, VALUE)` pairs.
        env: Vec<(String, String)>,
    },
    /// Host → guest: deliver a signal to the workload process.
    Signal {
        /// POSIX signal number (e.g. `15` for `SIGTERM`).
        signum: i32,
    },
    /// Guest → host: a chunk of the workload's standard output.
    Stdout(Vec<u8>),
    /// Guest → host: a chunk of the workload's standard error.
    Stderr(Vec<u8>),
    /// Guest → host: the workload process has been spawned.
    Started {
        /// PID of the spawned process inside the guest.
        pid: i32,
    },
    /// Guest → host: the workload process exited with the given status code.
    Exited {
        /// Exit code (or `128 + signum` for signal-terminated processes).
        code: i32,
    },
    /// Either direction: a transport- or operation-level error.
    Error {
        /// Human-readable error description.
        message: String,
    },
    /// Host → guest: turn **this** vsock connection into a transparent byte
    /// tunnel to `127.0.0.1:<port>` inside the guest.
    ///
    /// This must be the **first** frame on a connection. After the guest reads
    /// it, no further protocol framing is exchanged on this connection: the
    /// guest opens `TcpStream::connect(("127.0.0.1", port))` and bidirectionally
    /// pipes raw bytes between the vsock fd and the TCP stream until either side
    /// reaches EOF/error. It is the mechanism behind host→guest published-port
    /// reachability on the macOS VZ-Linux runtime, where VZ NAT establishes no
    /// usable guest lease and a host loopback listener must tunnel each
    /// connection over the (working) vsock channel instead.
    Forward {
        /// Guest-local TCP port to splice this connection to (on `127.0.0.1`).
        port: u16,
    },
}

impl Msg {
    /// On-wire tag byte for this variant.
    #[must_use]
    pub const fn tag(&self) -> u8 {
        match self {
            Msg::Run { .. } => 1,
            Msg::Exec { .. } => 2,
            Msg::Signal { .. } => 3,
            Msg::Stdout(_) => 4,
            Msg::Stderr(_) => 5,
            Msg::Started { .. } => 6,
            Msg::Exited { .. } => 7,
            Msg::Error { .. } => 8,
            Msg::Forward { .. } => 9,
        }
    }
}

/// Errors produced while encoding, decoding, or framing protocol messages.
#[derive(Debug)]
pub enum ProtoError {
    /// Underlying I/O failure on a [`Read`]/[`Write`].
    Io(io::Error),
    /// The payload could not be (de)serialized by postcard.
    Codec(postcard::Error),
    /// The frame's length prefix exceeded [`MAX_FRAME_LEN`].
    FrameTooLarge(u32),
    /// The frame body was empty (no room for even a tag byte).
    EmptyFrame,
    /// The leading tag byte did not correspond to a known variant.
    UnknownTag(u8),
}

impl std::fmt::Display for ProtoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProtoError::Io(e) => write!(f, "io error: {e}"),
            ProtoError::Codec(e) => write!(f, "codec error: {e}"),
            ProtoError::FrameTooLarge(n) => {
                write!(f, "frame length {n} exceeds maximum {MAX_FRAME_LEN}")
            }
            ProtoError::EmptyFrame => write!(f, "empty frame (missing tag byte)"),
            ProtoError::UnknownTag(t) => write!(f, "unknown message tag {t}"),
        }
    }
}

impl std::error::Error for ProtoError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ProtoError::Io(e) => Some(e),
            ProtoError::Codec(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for ProtoError {
    fn from(e: io::Error) -> Self {
        ProtoError::Io(e)
    }
}

impl From<postcard::Error> for ProtoError {
    fn from(e: postcard::Error) -> Self {
        ProtoError::Codec(e)
    }
}

/// Result alias for protocol operations.
pub type Result<T> = std::result::Result<T, ProtoError>;

/// Encode a message into a complete length-prefixed frame:
/// `u32 LE length` + `u8 tag` + `postcard payload`.
///
/// # Errors
///
/// Returns [`ProtoError::Codec`] if postcard fails to serialize the payload.
pub fn encode(msg: &Msg) -> Result<Vec<u8>> {
    let payload = postcard::to_allocvec(msg)?;
    // body = tag byte + payload
    let body_len = u32::try_from(payload.len())
        .ok()
        .and_then(|p| p.checked_add(1))
        .ok_or(ProtoError::FrameTooLarge(u32::MAX))?;
    if body_len > MAX_FRAME_LEN {
        return Err(ProtoError::FrameTooLarge(body_len));
    }
    let mut out = Vec::with_capacity(4 + body_len as usize);
    out.extend_from_slice(&body_len.to_le_bytes());
    out.push(msg.tag());
    out.extend_from_slice(&payload);
    Ok(out)
}

/// Decode a frame **body** (tag byte + payload, i.e. the bytes *after* the
/// `u32` length prefix) into a [`Msg`].
///
/// The leading tag byte is validated against the known variant set before the
/// postcard payload is deserialized; this lets a reader reject garbage frames
/// without handing arbitrary bytes to the deserializer.
///
/// # Errors
///
/// * [`ProtoError::EmptyFrame`] if `body` is empty.
/// * [`ProtoError::UnknownTag`] if the tag does not match any variant.
/// * [`ProtoError::Codec`] if the payload fails to deserialize.
pub fn decode(body: &[u8]) -> Result<Msg> {
    let (&tag, payload) = body.split_first().ok_or(ProtoError::EmptyFrame)?;
    // Validate the tag up front so we surface a clear error instead of letting
    // postcard choke on a bad enum discriminant.
    if !(1..=9).contains(&tag) {
        return Err(ProtoError::UnknownTag(tag));
    }
    let msg: Msg = postcard::from_bytes(payload)?;
    // Defense in depth: the deserialized variant's tag must match the framing
    // tag, otherwise the frame is internally inconsistent.
    if msg.tag() != tag {
        return Err(ProtoError::UnknownTag(tag));
    }
    Ok(msg)
}

/// Write a single message as a length-prefixed frame to `w`.
///
/// This is async-free and works over any blocking writer (vsock socket, pipe,
/// buffer). The entire frame is flushed before returning.
///
/// # Errors
///
/// Propagates encode errors and any underlying write/flush I/O error.
pub fn write_frame<W: Write>(w: &mut W, msg: &Msg) -> Result<()> {
    let frame = encode(msg)?;
    w.write_all(&frame)?;
    w.flush()?;
    Ok(())
}

/// Read a single length-prefixed frame from `r` and decode it into a [`Msg`].
///
/// Blocks until a full frame is available. Returns [`ProtoError::Io`] with an
/// `UnexpectedEof` kind if the stream closes mid-frame.
///
/// # Errors
///
/// * [`ProtoError::FrameTooLarge`] if the length prefix exceeds
///   [`MAX_FRAME_LEN`].
/// * I/O / codec / framing errors as per [`decode`].
pub fn read_frame<R: Read>(r: &mut R) -> Result<Msg> {
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf)?;
    let body_len = u32::from_le_bytes(len_buf);
    if body_len == 0 {
        return Err(ProtoError::EmptyFrame);
    }
    if body_len > MAX_FRAME_LEN {
        return Err(ProtoError::FrameTooLarge(body_len));
    }
    let mut body = vec![0u8; body_len as usize];
    r.read_exact(&mut body)?;
    decode(&body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// Every message variant used to exercise round-trips.
    fn sample_messages() -> Vec<Msg> {
        vec![
            Msg::Run {
                argv: vec!["/bin/sh".into(), "-c".into(), "echo hi".into()],
                env: vec![
                    ("PATH".into(), "/usr/bin:/bin".into()),
                    ("HOME".into(), "/root".into()),
                ],
                cwd: Some("/app".into()),
                uid: 1000,
                gid: 1000,
            },
            // Run with no cwd and empty env exercises the Option::None + empty
            // Vec paths.
            Msg::Run {
                argv: vec!["/init".into()],
                env: vec![],
                cwd: None,
                uid: 0,
                gid: 0,
            },
            Msg::Exec {
                argv: vec!["/bin/ps".into(), "aux".into()],
                env: vec![("TERM".into(), "xterm".into())],
            },
            Msg::Signal { signum: 15 },
            Msg::Signal { signum: 9 },
            Msg::Stdout(b"hello stdout\n".to_vec()),
            Msg::Stdout(Vec::new()), // empty chunk
            Msg::Stderr(b"oops stderr\n".to_vec()),
            Msg::Started { pid: 4242 },
            Msg::Exited { code: 0 },
            Msg::Exited { code: 137 }, // 128 + SIGKILL
            Msg::Error {
                message: "rootfs mount failed".into(),
            },
            Msg::Forward { port: 8080 },
            Msg::Forward { port: 0 }, // edge: port 0
            Msg::Forward { port: u16::MAX },
        ]
    }

    #[test]
    fn forward_frame_roundtrips() {
        for port in [0u16, 80, 443, 8080, 65535] {
            let msg = Msg::Forward { port };
            // Tag is the appended discriminant (9), kept stable.
            assert_eq!(msg.tag(), 9, "Forward must use the appended tag 9");

            // encode/decode round-trip.
            let frame = encode(&msg).expect("encode Forward");
            assert_eq!(frame[4], 9, "framing tag byte must be 9");
            let decoded = decode(&frame[4..]).expect("decode Forward");
            assert_eq!(decoded, msg, "Forward encode/decode mismatch");

            // read_frame/write_frame round-trip over a stream.
            let mut buf = Vec::new();
            write_frame(&mut buf, &msg).expect("write Forward");
            let mut cursor = Cursor::new(buf);
            let got = read_frame(&mut cursor).expect("read Forward");
            assert_eq!(got, msg, "Forward frame round-trip mismatch");
        }
    }

    #[test]
    fn encode_decode_roundtrip_all_variants() {
        for msg in sample_messages() {
            let frame = encode(&msg).expect("encode");
            // Frame must begin with a correct LE length prefix.
            let declared = u32::from_le_bytes([frame[0], frame[1], frame[2], frame[3]]);
            assert_eq!(declared as usize, frame.len() - 4, "length prefix mismatch");
            // The byte right after the prefix is the variant tag.
            assert_eq!(frame[4], msg.tag(), "tag byte mismatch");
            // Decoding the body reproduces the message exactly.
            let decoded = decode(&frame[4..]).expect("decode");
            assert_eq!(decoded, msg, "round-trip mismatch");
        }
    }

    #[test]
    fn read_write_frame_roundtrip_all_variants() {
        for msg in sample_messages() {
            let mut buf = Vec::new();
            write_frame(&mut buf, &msg).expect("write_frame");
            let mut cursor = Cursor::new(buf);
            let got = read_frame(&mut cursor).expect("read_frame");
            assert_eq!(got, msg, "frame round-trip mismatch");
        }
    }

    #[test]
    fn multiple_frames_stream_in_order() {
        let msgs = sample_messages();
        let mut stream = Vec::new();
        for m in &msgs {
            write_frame(&mut stream, m).expect("write");
        }
        let mut cursor = Cursor::new(stream);
        for expected in &msgs {
            let got = read_frame(&mut cursor).expect("read");
            assert_eq!(&got, expected);
        }
        // Stream is now exhausted; a further read hits EOF.
        let err = read_frame(&mut cursor).expect_err("expected EOF");
        match err {
            ProtoError::Io(e) => assert_eq!(e.kind(), io::ErrorKind::UnexpectedEof),
            other => panic!("expected Io(UnexpectedEof), got {other:?}"),
        }
    }

    #[test]
    fn empty_body_is_rejected() {
        assert!(matches!(decode(&[]), Err(ProtoError::EmptyFrame)));
    }

    #[test]
    fn unknown_tag_is_rejected() {
        // Tag 0 and tag 99 are not assigned.
        assert!(matches!(decode(&[0]), Err(ProtoError::UnknownTag(0))));
        assert!(matches!(
            decode(&[99, 1, 2, 3]),
            Err(ProtoError::UnknownTag(99))
        ));
    }

    #[test]
    fn zero_length_prefix_is_rejected() {
        let mut cursor = Cursor::new(vec![0u8, 0, 0, 0]);
        assert!(matches!(
            read_frame(&mut cursor),
            Err(ProtoError::EmptyFrame)
        ));
    }

    #[test]
    fn oversize_length_prefix_is_rejected() {
        let huge = (MAX_FRAME_LEN + 1).to_le_bytes();
        let mut cursor = Cursor::new(huge.to_vec());
        assert!(matches!(
            read_frame(&mut cursor),
            Err(ProtoError::FrameTooLarge(_))
        ));
    }

    #[test]
    fn truncated_stream_returns_eof() {
        let msg = Msg::Started { pid: 7 };
        let frame = encode(&msg).expect("encode");
        // Drop the last byte to truncate the body.
        let mut cursor = Cursor::new(frame[..frame.len() - 1].to_vec());
        let err = read_frame(&mut cursor).expect_err("expected truncation error");
        assert!(matches!(err, ProtoError::Io(_)));
    }
}
