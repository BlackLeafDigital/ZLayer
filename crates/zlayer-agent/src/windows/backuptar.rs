//! Translate OCI Windows layer tar entries into the hcsshim staging-dir
//! on-disk format consumed by `HcsImportLayer`.
//!
//! OCI Windows base layers (e.g. `mcr.microsoft.com/windows/nanoserver:ltsc2022`)
//! distribute file content + NTFS metadata as ordinary tar entries with the
//! NTFS bits carried in PAX extended headers (`MSWINDOWS.rawsd`,
//! `MSWINDOWS.reparse`, `MSWINDOWS.eas`, `MSWINDOWS.fileattr`).
//!
//! `HcsImportLayer` reads back each staged `Files\…` entry via hcsshim's
//! `legacyLayerReader.Next`, which expects the file to start with a 4-byte
//! little-endian `uint32` `FileAttributes` header followed by a sequence of
//! `WIN32_STREAM_ID`-framed `BACKUP_*` records (`BACKUP_SECURITY_DATA`,
//! `BACKUP_REPARSE_DATA`, `BACKUP_EA_DATA`, `BACKUP_DATA`). Those framing
//! bytes are persisted **verbatim** — they are NOT applied through `BackupWrite`
//! / `BackupRead`, since the kernel-side importer re-reads them itself.
//!
//! This module synthesises that on-disk format from a tar entry's body + PAX
//! headers and writes it to a regular file at the staging-dir destination,
//! mirroring the behaviour of hcsshim's `legacyLayerWriter.Add` in
//! `internal/wclayer/legacy.go`.

#![cfg(target_os = "windows")]

use std::io::{self, Read, Write};
use std::path::Path;

use base64::Engine as _;

/// `WIN32_STREAM_ID::dwStreamId` values, per
/// `winnt.h` (`BACKUP_*`). The set we synthesise is a subset of the full
/// list — see the Microsoft docs for the rest (`BACKUP_LINK`,
/// `BACKUP_PROPERTY_DATA`, `BACKUP_OBJECT_ID`, `BACKUP_TXFS_DATA`,
/// `BACKUP_ALTERNATE_DATA`). Those streams are not carried in OCI Windows
/// layer PAX headers, so we never emit them.
pub(crate) const BACKUP_DATA: u32 = 0x0000_0001;
pub(crate) const BACKUP_EA_DATA: u32 = 0x0000_0002;
pub(crate) const BACKUP_SECURITY_DATA: u32 = 0x0000_0003;
pub(crate) const BACKUP_REPARSE_DATA: u32 = 0x0000_0008;

/// Write a single `WIN32_STREAM_ID` header into `writer`.
///
/// The `WIN32_STREAM_ID` C struct is 20 bytes fixed prefix:
/// `dwStreamId(u32) | dwStreamAttributes(u32) | Size(i64) | dwStreamNameSize(u32)`,
/// followed by `dwStreamNameSize` bytes of UTF-16LE stream name (only used for
/// named alternate-data-streams). All three default-stream IDs we emit
/// (`BACKUP_DATA`, `BACKUP_SECURITY_DATA`, `BACKUP_REPARSE_DATA`,
/// `BACKUP_EA_DATA`) have an empty name, so `dwStreamNameSize` is always 0
/// and the trailing wide-char buffer is empty.
pub(crate) fn write_stream_header(
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

/// `FILE_ATTRIBUTE_ARCHIVE` — hcsshim's effective default for staged files
/// when no `MSWINDOWS.fileattr` PAX header is present.
const FILE_ATTRIBUTE_ARCHIVE: u32 = 0x0000_0020;

/// PAX metadata extracted from a tar entry's extended headers.
///
/// Each blob value is the **decoded** payload as written by the OCI layer
/// publisher (Microsoft's image build pipeline base64-encodes the raw
/// security descriptor / reparse blob / EA blob into the PAX header
/// value). `file_attributes` is parsed from decimal ASCII per the
/// `MSWINDOWS.fileattr` convention used by go-winio's `backuptar`.
#[derive(Default)]
struct PaxMetadata {
    security_descriptor: Option<Vec<u8>>,
    reparse_data: Option<Vec<u8>>,
    extended_attributes: Option<Vec<u8>>,
    file_attributes: Option<u32>,
}

/// Pull the MSWINDOWS.* PAX extensions off a tar entry, base64-decoding
/// each blob into raw bytes and parsing `MSWINDOWS.fileattr` as decimal
/// ASCII.
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
            "MSWINDOWS.fileattr" => {
                // Decimal ASCII per go-winio's `backuptar.WriteTarFileFromBackupStream` /
                // hcsshim's `legacyLayerWriter.Add` — e.g. "32" for FILE_ATTRIBUTE_ARCHIVE.
                let s = std::str::from_utf8(val)
                    .map_err(|e| io::Error::other(format!("PAX MSWINDOWS.fileattr utf8: {e}")))?;
                let n = s
                    .trim()
                    .parse::<u32>()
                    .map_err(|e| io::Error::other(format!("PAX MSWINDOWS.fileattr parse: {e}")))?;
                out.file_attributes = Some(n);
            }
            _ => {}
        }
    }
    Ok(out)
}

/// Writes the hcsshim staging-dir on-disk format
/// `[4-byte LE FileAttributes][BackupStream-framed records]` to a regular
/// file at `dest`.
///
/// This is NOT a `BackupWrite`-applied file — the framing bytes are
/// persisted verbatim because the kernel-side `HcsImportLayer` re-reads
/// them via hcsshim's `legacyLayerReader.Next`, which itself reads the
/// 4-byte `FileAttributes` header and then decodes the trailing bytes as
/// raw `WIN32_STREAM_ID` records.
///
/// On-disk layout written:
/// 1. 4 bytes LE `uint32` — `MSWINDOWS.fileattr` PAX value if present,
///    else `FILE_ATTRIBUTE_ARCHIVE` (0x20) per hcsshim's effective default.
/// 2. `BACKUP_SECURITY_DATA` framed record — if the entry carries
///    `MSWINDOWS.rawsd`.
/// 3. `BACKUP_REPARSE_DATA` framed record — if the entry carries
///    `MSWINDOWS.reparse`. For reparse points the file body is
///    conventionally empty; we still emit the data record below if
///    `size > 0` so we don't drop data.
/// 4. `BACKUP_EA_DATA` framed record — if the entry carries `MSWINDOWS.eas`.
/// 5. `BACKUP_DATA` framed record followed by `body_size` body bytes.
///    Skipped when the body is empty (zero-byte files and reparse-point
///    placeholders).
///
/// Sparse-block streams (`BACKUP_SPARSE_BLOCK`) are not currently
/// synthesised — the OCI Windows layer publishers materialise sparse
/// files as dense bodies in the tar, so the dense `BACKUP_DATA` record
/// is correct. Add sparse handling here if a future image carries an
/// `MSWINDOWS.sparse` PAX header.
///
/// # Errors
///
/// Returns the OS error from `CreateFileW`, any write failure, or a
/// base64/utf8 decode error on a malformed PAX value.
pub fn write_oci_entry_to_backup_stream<R: Read>(
    entry: &mut tar::Entry<'_, R>,
    dest: &Path,
) -> io::Result<()> {
    // Collect PAX metadata BEFORE the body — `pax_extensions()` snapshots
    // the entry's GNU/PAX headers and is safe to call before reading the
    // body. The body itself is consumed by the BACKUP_DATA copy below.
    let pax = collect_pax_metadata(entry)?;
    let body_size = entry.size();

    // Regular file via `\\?\` long-path-aware CreateFileW; the framing bytes
    // are written verbatim, NOT through BackupWrite.
    let mut writer = crate::windows::layer::create_long_path_file(dest)?;

    // 4-byte LE FileAttributes header expected by legacyLayerReader.Next.
    let attrs = pax.file_attributes.unwrap_or(FILE_ATTRIBUTE_ARCHIVE);
    writer.write_all(&attrs.to_le_bytes())?;

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

/// Base-layer counterpart to [`write_oci_entry_to_backup_stream`].
///
/// hcsshim's `baseLayerWriter` (see `internal/wclayer/baselayerwriter.go`)
/// produces REAL NTFS files for base (parent-less) layers — the `BackupStream`
/// records are APPLIED via `BackupWrite` to the destination file rather than
/// persisted verbatim. The on-disk format is therefore:
///
/// 1. NO 4-byte `FileAttributes` header (the diff-layer staging-format prefix
///    is absent — the kernel walks the file metadata directly).
/// 2. `BACKUP_SECURITY_DATA` / `BACKUP_REPARSE_DATA` / `BACKUP_EA_DATA` /
///    `BACKUP_DATA` records written through [`BackupStreamWriter`], which
///    funnels them into `BackupWrite()` — the kernel stamps the security
///    descriptor, reparse data, EAs, and body onto the file directly.
///
/// The same PAX collection logic is reused; the only differences relative to
/// the diff-layer writer are the destination (`BackupStreamWriter` instead of
/// a regular file) and the omission of the 4-byte attr header.
///
/// # Errors
///
/// Returns the OS error from `CreateFileW` / `BackupWrite`, any write
/// failure, or a base64/utf8 decode error on a malformed PAX value.
pub fn write_oci_entry_as_base_layer<R: Read>(
    entry: &mut tar::Entry<'_, R>,
    dest: &Path,
) -> io::Result<()> {
    let pax = collect_pax_metadata(entry)?;
    let body_size = entry.size();

    // `BackupStreamWriter::create_new` materializes the empty file via the
    // long-path-aware helper, then re-opens it with FILE_FLAG_BACKUP_SEMANTICS
    // so subsequent writes flow through BackupWrite() — which stamps each
    // WIN32_STREAM_ID record onto the real NTFS file metadata.
    let mut writer = crate::windows::layer::BackupStreamWriter::create_new(dest)?;

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
// against a captured buffer rather than a real file handle. The
// `write_oci_entry_to_backup_stream` flow writes to a real `\\?\…`
// long-path-aware file, so it only runs on Windows hosts; the end-to-end
// flow is also exercised by the integration tests in
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

    /// Build a minimal tar with a PAX `MSWINDOWS.fileattr=32` extension
    /// followed by a regular `Files/test.txt` entry whose body is `b"hi"`,
    /// then drive `write_oci_entry_to_backup_stream` against a temp file
    /// and assert the on-disk layout matches the hcsshim staging-dir format
    /// `[4-byte LE FileAttributes][BACKUP_DATA WIN32_STREAM_ID][body]`.
    #[test]
    fn write_oci_entry_writes_attr_header_then_body_for_minimal_file() {
        // 1. Synthesise the tar in memory.
        let mut tar_bytes: Vec<u8> = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_bytes);
            // PAX extended header: MSWINDOWS.fileattr = "32" (0x20 archive).
            builder
                .append_pax_extensions([("MSWINDOWS.fileattr", b"32".as_slice())])
                .unwrap();
            // Regular file entry.
            let body: &[u8] = b"hi";
            let mut header = tar::Header::new_ustar();
            header.set_path("Files/test.txt").unwrap();
            header.set_size(body.len() as u64);
            header.set_entry_type(tar::EntryType::Regular);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append(&header, body).unwrap();
            builder.finish().unwrap();
        }

        // 2. Walk the tar to the regular file entry and run the writer.
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("test.txt");
        {
            let mut archive = tar::Archive::new(io::Cursor::new(&tar_bytes));
            let mut entries = archive.entries().unwrap();
            let mut entry = loop {
                let e = entries
                    .next()
                    .expect("expected a regular file entry after the PAX header")
                    .unwrap();
                if e.header().entry_type() == tar::EntryType::Regular {
                    break e;
                }
            };
            write_oci_entry_to_backup_stream(&mut entry, &dest).unwrap();
        }

        // 3. Assert the on-disk layout.
        let out = std::fs::read(&dest).unwrap();
        assert_eq!(
            out.len(),
            4 + 20 + 2,
            "attr(4) + WIN32_STREAM_ID(20) + body(2)"
        );
        // 4-byte FileAttributes header = 0x20 LE.
        assert_eq!(&out[0..4], &[0x20, 0x00, 0x00, 0x00]);
        // BACKUP_DATA WIN32_STREAM_ID prefix: id=1, attrs=0, size=2, nameSize=0.
        assert_eq!(&out[4..8], &1u32.to_le_bytes());
        assert_eq!(&out[8..12], &0u32.to_le_bytes());
        assert_eq!(&out[12..20], &2u64.to_le_bytes());
        assert_eq!(&out[20..24], &0u32.to_le_bytes());
        // Body bytes.
        assert_eq!(&out[24..26], b"hi");
    }
}
