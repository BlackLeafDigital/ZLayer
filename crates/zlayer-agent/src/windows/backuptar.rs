//! Translate OCI Windows layer tar entries into `BackupWrite` stream framing.
//!
//! OCI Windows base layers (e.g. `mcr.microsoft.com/windows/nanoserver:ltsc2022`)
//! distribute file content + NTFS metadata as ordinary tar entries with the
//! NTFS bits carried in PAX extended headers (`MSWINDOWS.rawsd`,
//! `MSWINDOWS.reparse`, `MSWINDOWS.eas`). The wclayer materialiser, however,
//! consumes data via `BackupWrite`, which expects each stream to be prefixed
//! with a `WIN32_STREAM_ID` record identifying its type (data, security,
//! reparse, EAs, sparse blocks, ...).
//!
//! This module synthesises the `WIN32_STREAM_ID`-framed records from a tar
//! entry's body + PAX headers and writes them through [`BackupStreamWriter`],
//! mirroring the behaviour of go-winio's
//! `backuptar.WriteBackupStreamFromTarFile` and hcsshim's
//! `ext4/ociwclayer/import.go`.

#![cfg(target_os = "windows")]

use std::io::{self, Read, Write};
use std::path::Path;

use base64::Engine as _;

use crate::windows::layer::BackupStreamWriter;

/// `WIN32_STREAM_ID::dwStreamId` values, per
/// `winnt.h` (`BACKUP_*`). The set we synthesise is a subset of the full
/// list — see the Microsoft docs for the rest (`BACKUP_LINK`,
/// `BACKUP_PROPERTY_DATA`, `BACKUP_OBJECT_ID`, `BACKUP_TXFS_DATA`,
/// `BACKUP_ALTERNATE_DATA`). Those streams are not carried in OCI Windows
/// layer PAX headers, so we never emit them.
const BACKUP_DATA: u32 = 0x0000_0001;
const BACKUP_EA_DATA: u32 = 0x0000_0002;
const BACKUP_SECURITY_DATA: u32 = 0x0000_0003;
const BACKUP_REPARSE_DATA: u32 = 0x0000_0008;

/// Write a single `WIN32_STREAM_ID` header into `writer`.
///
/// The `WIN32_STREAM_ID` C struct is 20 bytes fixed prefix:
/// `dwStreamId(u32) | dwStreamAttributes(u32) | Size(i64) | dwStreamNameSize(u32)`,
/// followed by `dwStreamNameSize` bytes of UTF-16LE stream name (only used for
/// named alternate-data-streams). All three default-stream IDs we emit
/// (`BACKUP_DATA`, `BACKUP_SECURITY_DATA`, `BACKUP_REPARSE_DATA`,
/// `BACKUP_EA_DATA`) have an empty name, so `dwStreamNameSize` is always 0
/// and the trailing wide-char buffer is empty.
fn write_stream_header(
    writer: &mut impl Write,
    stream_id: u32,
    attributes: u32,
    size: u64,
) -> io::Result<()> {
    writer.write_all(&stream_id.to_le_bytes())?;
    writer.write_all(&attributes.to_le_bytes())?;
    writer.write_all(&size.to_le_bytes())?;
    writer.write_all(&0u32.to_le_bytes())?; // dwStreamNameSize = 0
    Ok(())
}

/// PAX metadata extracted from a tar entry's extended headers.
///
/// Each value is the **decoded** payload as written by the OCI layer
/// publisher (Microsoft's image build pipeline base64-encodes the raw
/// security descriptor / reparse blob / EA blob into the PAX header
/// value).
#[derive(Default)]
struct PaxMetadata {
    security_descriptor: Option<Vec<u8>>,
    reparse_data: Option<Vec<u8>>,
    extended_attributes: Option<Vec<u8>>,
}

/// Pull the MSWINDOWS.* PAX extensions off a tar entry, base64-decoding
/// each into raw bytes.
fn collect_pax_metadata<R: Read>(entry: &mut tar::Entry<'_, R>) -> io::Result<PaxMetadata> {
    let mut out = PaxMetadata::default();
    let Some(pax) = entry.pax_extensions()? else {
        return Ok(out);
    };
    let engine = base64::engine::general_purpose::STANDARD;
    for ext in pax {
        let ext = ext?;
        let key = ext.key().unwrap_or("");
        let val = ext.value_bytes();
        match key {
            "MSWINDOWS.rawsd" => {
                out.security_descriptor = Some(engine.decode(val).map_err(|e| {
                    io::Error::other(format!("PAX MSWINDOWS.rawsd base64 decode: {e}"))
                })?);
            }
            "MSWINDOWS.reparse" => {
                out.reparse_data = Some(engine.decode(val).map_err(|e| {
                    io::Error::other(format!("PAX MSWINDOWS.reparse base64 decode: {e}"))
                })?);
            }
            "MSWINDOWS.eas" => {
                out.extended_attributes = Some(engine.decode(val).map_err(|e| {
                    io::Error::other(format!("PAX MSWINDOWS.eas base64 decode: {e}"))
                })?);
            }
            _ => {}
        }
    }
    Ok(out)
}

/// Translate one OCI Windows layer tar entry into a `BackupWrite`
/// stream and materialise it on disk at `dest`.
///
/// Stream emission order:
/// 1. `BACKUP_SECURITY_DATA` — if the entry carries `MSWINDOWS.rawsd`.
/// 2. `BACKUP_REPARSE_DATA` — if the entry carries `MSWINDOWS.reparse`.
///    For reparse points the file body is conventionally empty; we still
///    emit the data stream below if `size > 0` so we don't drop data.
/// 3. `BACKUP_EA_DATA` — if the entry carries `MSWINDOWS.eas`.
/// 4. `BACKUP_DATA` — the file body. Skipped when the body is empty
///    (zero-byte files and reparse-point placeholders).
///
/// Sparse-block streams (`BACKUP_SPARSE_BLOCK`) are not currently
/// synthesised — the OCI Windows layer publishers materialise sparse
/// files as dense bodies in the tar, so the dense `BACKUP_DATA` stream
/// is correct. Add sparse handling here if a future image carries an
/// `MSWINDOWS.sparse` PAX header.
///
/// # Errors
///
/// Returns the OS error from `CreateFileW`, any `BackupWrite` failure,
/// or a base64 decode error on a malformed PAX value.
pub fn write_oci_entry_to_backup_stream<R: Read>(
    entry: &mut tar::Entry<'_, R>,
    dest: &Path,
) -> io::Result<()> {
    // Collect PAX metadata BEFORE the body — `pax_extensions()` snapshots
    // the entry's GNU/PAX headers and is safe to call before reading the
    // body. The body itself is consumed by the BACKUP_DATA copy below.
    let pax = collect_pax_metadata(entry)?;
    let body_size = entry.size();

    let mut writer = BackupStreamWriter::create_new(dest)?;

    if let Some(sd) = pax.security_descriptor {
        write_stream_header(&mut writer, BACKUP_SECURITY_DATA, 0, sd.len() as u64)?;
        writer.write_all(&sd)?;
    }

    if let Some(rp) = pax.reparse_data {
        write_stream_header(&mut writer, BACKUP_REPARSE_DATA, 0, rp.len() as u64)?;
        writer.write_all(&rp)?;
    }

    if let Some(eas) = pax.extended_attributes {
        write_stream_header(&mut writer, BACKUP_EA_DATA, 0, eas.len() as u64)?;
        writer.write_all(&eas)?;
    }

    if body_size > 0 {
        write_stream_header(&mut writer, BACKUP_DATA, 0, body_size)?;
        io::copy(entry, &mut writer)?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
//
// The header serialisation can be exercised on any platform — we test it
// against a captured buffer rather than a real file handle. The full
// `write_oci_entry_to_backup_stream` flow drives a real `BackupStreamWriter`
// which requires Win32 APIs; that's covered by the integration tests in
// `windows_hcs_hyperv_e2e.rs`.

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper writer that captures the bytes the framing code emits, so
    /// we can golden-test the `WIN32_STREAM_ID` header layout without
    /// going through `BackupWrite`.
    struct Capture {
        buf: Vec<u8>,
    }

    impl Write for Capture {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.buf.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn header_layout_is_20_bytes_le_with_zero_name_size() {
        let mut cap = Capture { buf: Vec::new() };
        write_stream_header(&mut cap, BACKUP_DATA, 0, 0xAA_BB_CC_DD).unwrap();
        assert_eq!(cap.buf.len(), 20, "WIN32_STREAM_ID prefix is 20 bytes");

        // dwStreamId
        assert_eq!(&cap.buf[0..4], &1u32.to_le_bytes());
        // dwStreamAttributes
        assert_eq!(&cap.buf[4..8], &0u32.to_le_bytes());
        // Size (LARGE_INTEGER, little-endian u64)
        assert_eq!(&cap.buf[8..16], &0xAA_BB_CC_DDu64.to_le_bytes());
        // dwStreamNameSize
        assert_eq!(&cap.buf[16..20], &0u32.to_le_bytes());
    }

    #[test]
    fn header_encodes_security_stream_id() {
        let mut cap = Capture { buf: Vec::new() };
        write_stream_header(&mut cap, BACKUP_SECURITY_DATA, 0, 16).unwrap();
        assert_eq!(&cap.buf[0..4], &3u32.to_le_bytes());
        assert_eq!(&cap.buf[8..16], &16u64.to_le_bytes());
    }

    #[test]
    fn header_encodes_reparse_stream_id() {
        let mut cap = Capture { buf: Vec::new() };
        write_stream_header(&mut cap, BACKUP_REPARSE_DATA, 0, 24).unwrap();
        assert_eq!(&cap.buf[0..4], &8u32.to_le_bytes());
        assert_eq!(&cap.buf[8..16], &24u64.to_le_bytes());
    }

    #[test]
    fn header_encodes_ea_stream_id() {
        let mut cap = Capture { buf: Vec::new() };
        write_stream_header(&mut cap, BACKUP_EA_DATA, 0, 7).unwrap();
        assert_eq!(&cap.buf[0..4], &2u32.to_le_bytes());
        assert_eq!(&cap.buf[8..16], &7u64.to_le_bytes());
    }
}
