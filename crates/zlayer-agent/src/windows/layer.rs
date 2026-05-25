//! Backup-stream encoding/decoding for Windows container layers.
//!
//! Windows OCI layers carry file content + NTFS metadata (reparse points,
//! extended attributes, security descriptors, alternate data streams) in
//! `BackupRead`/`BackupWrite` stream format. This module wraps those Win32
//! APIs with Rust-friendly readers and writers, and exposes the privilege
//! helper that `HcsImportLayer` needs to materialize a layer directory.
//!
//! See `hcsshim/internal/wclayer/legacy.go` for the reference implementation.

#![cfg(target_os = "windows")]
#![allow(unsafe_code)]

use std::ffi::OsStr;
use std::io::{self, Read, Write};
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE, LUID};
use windows::Win32::Security::{
    AdjustTokenPrivileges, LookupPrivilegeValueW, LUID_AND_ATTRIBUTES, SE_PRIVILEGE_ENABLED,
    TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
};
use windows::Win32::Storage::FileSystem::{
    BackupRead, BackupWrite, CreateFileW, CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// Convert a path to a UTF-16 null-terminated buffer suitable for
/// `CreateFileW`, with the Windows extended-length-path prefix
/// (`\\?\` or `\\?\UNC\`) applied so the call works for paths
/// longer than `MAX_PATH` (260 chars).
///
/// Required for unpacking deep Windows container layers
/// (e.g. `Files/Windows/WinSxS/…/<60-char SxS component>/<long filename>.dll`)
/// where the post-canonicalization absolute path easily exceeds 260 chars.
/// Without this prefix `CreateFileW` returns `ERROR_PATH_NOT_FOUND` (0x80070003)
/// even when `create_dir_all` has materialized every parent directory.
///
/// # Errors
///
/// Returns the OS error if the path cannot be made absolute (extremely rare —
/// typically only happens if `current_dir()` itself fails for a relative input).
pub(crate) fn to_extended_wide(path: &Path) -> io::Result<Vec<u16>> {
    // Hoisted constants so clippy::items_after_statements doesn't complain
    // about declaring `const` items mid-function. These describe the three
    // path prefixes Windows treats specially:
    //   * `\\?\`  - verbatim (skip kernel path normalisation, ~32k limit)
    //   * `\\.\`  - device namespace (e.g. `\\.\PhysicalDrive0`)
    //   * `\\`    - generic UNC (`\\server\share\...`)
    const VERBATIM_PREFIX: &[u16] = &['\\' as u16, '\\' as u16, '?' as u16, '\\' as u16];
    const DEVICE_PREFIX: &[u16] = &['\\' as u16, '\\' as u16, '.' as u16, '\\' as u16];
    const UNC_PREFIX: &[u16] = &['\\' as u16, '\\' as u16];

    // Resolve to an absolute path. Prefer `canonicalize` (resolves symlinks
    // and produces a verbatim form) when the path exists; fall back to
    // `std::path::absolute` (stable since 1.79) which only normalises
    // lexically, so it works for not-yet-created targets like the file we're
    // about to open with `CREATE_ALWAYS`. Last resort: join CWD by hand.
    let abs: PathBuf = match path.canonicalize() {
        Ok(p) => p,
        Err(_) => match std::path::absolute(path) {
            Ok(p) => p,
            Err(_) => {
                if path.is_absolute() {
                    path.to_path_buf()
                } else {
                    std::env::current_dir()?.join(path)
                }
            }
        },
    };

    let wide_in: Vec<u16> = abs.as_os_str().encode_wide().collect();

    // If already prefixed with `\\?\` or `\\.\`, pass through unchanged
    // (just null-terminate).
    if wide_in.starts_with(VERBATIM_PREFIX) || wide_in.starts_with(DEVICE_PREFIX) {
        let mut out = wide_in;
        out.push(0);
        return Ok(out);
    }

    // UNC path `\\server\share\...` -> `\\?\UNC\server\share\...`
    let mut out: Vec<u16>;
    if wide_in.starts_with(UNC_PREFIX) {
        out = Vec::with_capacity(wide_in.len() + 6);
        out.extend("\\\\?\\UNC".encode_utf16());
        // Drop the leading single backslash from `\\server\share\path` so we
        // end up with `\\?\UNC\server\share\path`.
        out.extend_from_slice(&wide_in[1..]);
    } else {
        // Local path -> `\\?\C:\path`.
        out = Vec::with_capacity(wide_in.len() + 4);
        out.extend("\\\\?\\".encode_utf16());
        out.extend_from_slice(&wide_in);
    }
    out.push(0);
    Ok(out)
}

/// Create (or truncate) a file at `path` using `CreateFileW(CREATE_ALWAYS)`
/// with the extended-length-path prefix so it works for paths longer than
/// `MAX_PATH`. Returns an owned [`std::fs::File`] wrapping the resulting
/// handle so the caller can `write_all` / `io::copy` into it without
/// re-opening the path through a non-long-path-aware API.
///
/// Used by the OCI Windows layer unpacker for non-`Files/` payloads
/// (`Hives/`, `tombstones.txt`, `UtilityVM/`) and as the inner implementation
/// of [`BackupStreamWriter::create_new`] (which immediately drops the
/// returned file and re-opens via `BACKUP_SEMANTICS`).
///
/// `std::fs::File::create` is intentionally avoided because it does not apply
/// the long-path prefix automatically, so it fails with
/// `ERROR_PATH_NOT_FOUND` (0x80070003) for the deep `WinSxS` paths that appear
/// in `mcr.microsoft.com/windows/nanoserver` layers.
///
/// # Errors
///
/// Returns the OS error from `CreateFileW`.
pub(crate) fn create_long_path_file(path: &Path) -> io::Result<std::fs::File> {
    use std::os::windows::io::FromRawHandle;
    let wide = to_extended_wide(path)?;
    // SAFETY: `wide` is a valid null-terminated UTF-16 buffer that outlives
    // the call. All other arguments are plain values.
    let handle = unsafe {
        CreateFileW(
            PCWSTR::from_raw(wide.as_ptr()),
            GENERIC_WRITE.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            CREATE_ALWAYS,
            FILE_ATTRIBUTE_NORMAL,
            None,
        )
    }
    .map_err(|e| io::Error::other(format!("CreateFileW(CREATE_ALWAYS): {e}")))?;
    // SAFETY: `handle` was just returned by a successful `CreateFileW` (so it
    // is non-null and a real Win32 file handle), and no other owner exists.
    // Transferring ownership to `std::fs::File` is sound; the `File`'s Drop
    // will call `CloseHandle` for us.
    let file = unsafe { std::fs::File::from_raw_handle(handle.0.cast()) };
    Ok(file)
}

/// Enable `SeBackupPrivilege` and `SeRestorePrivilege` on the current process
/// token. `HcsImportLayer` (and any `BackupWrite` into a system-protected path)
/// requires both. Idempotent — safe to call multiple times.
///
/// # Errors
///
/// Returns an error if the process token can't be opened, the privilege names
/// don't resolve, or `AdjustTokenPrivileges` reports a partial success (i.e.
/// the caller's account doesn't actually hold the privilege, typical for non-
/// admin users).
pub fn enable_backup_restore_privileges() -> io::Result<()> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
            &mut token,
        )
        .map_err(|e| io::Error::other(format!("OpenProcessToken: {e}")))?;

        let backup_res = enable_named_privilege(token, "SeBackupPrivilege");
        let restore_res = enable_named_privilege(token, "SeRestorePrivilege");

        // Close the token regardless of the result. Failure is non-fatal — log
        // and continue so we can still surface the underlying privilege error.
        if let Err(e) = CloseHandle(token) {
            tracing::debug!("CloseHandle(token) failed: {e}");
        }

        backup_res?;
        restore_res?;
    }
    Ok(())
}

/// Enable a single named privilege via `LookupPrivilegeValueW` +
/// `AdjustTokenPrivileges`.
///
/// # Safety
///
/// `token` must be a valid process-token handle opened with at least
/// `TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY` access, and must remain valid for
/// the duration of this call.
unsafe fn enable_named_privilege(token: HANDLE, name: &str) -> io::Result<()> {
    let name_w: Vec<u16> = OsStr::new(name)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut luid = LUID::default();
    unsafe {
        LookupPrivilegeValueW(PCWSTR::null(), PCWSTR::from_raw(name_w.as_ptr()), &mut luid)
            .map_err(|e| io::Error::other(format!("LookupPrivilegeValueW({name}): {e}")))?;

        let privs = TOKEN_PRIVILEGES {
            PrivilegeCount: 1,
            Privileges: [LUID_AND_ATTRIBUTES {
                Luid: luid,
                Attributes: SE_PRIVILEGE_ENABLED,
            }],
        };
        AdjustTokenPrivileges(token, false, Some(&raw const privs), 0, None, None)
            .map_err(|e| io::Error::other(format!("AdjustTokenPrivileges({name}): {e}")))?;
    }
    Ok(())
}

/// Thin writer that streams bytes into a file via `BackupWrite`, preserving
/// Win32 backup stream headers. Used when applying an incoming wclayer tar
/// entry to the destination folder.
///
/// The caller is responsible for producing the backup-stream framing
/// (`WIN32_STREAM_ID` headers + payload) and feeding that into [`Write::write`]
/// in a single contiguous stream. `BackupWrite` internally tracks state across
/// calls through the `lpContext` pointer we hold in [`Self::context`].
pub struct BackupStreamWriter {
    handle: HANDLE,
    context: *mut core::ffi::c_void,
}

// `BackupStreamWriter` owns its HANDLE and context pointer and is not shared
// across threads from within this module. We still implement `Send` explicitly
// because raw pointers are `!Send` by default — the underlying `BackupWrite`
// state is safe to move between threads as long as only one thread uses it at
// a time, which Rust's `&mut self` borrows guarantee.
unsafe impl Send for BackupStreamWriter {}

impl BackupStreamWriter {
    /// Open an **existing** target file for backup writing. Path must be
    /// UTF-16-encodable. The file must already exist — use [`Self::create_new`]
    /// when materializing a brand-new wclayer file from a tar entry.
    ///
    /// # Errors
    ///
    /// Returns the OS error from `CreateFileW` if the target can't be opened.
    pub fn create(path: &Path) -> io::Result<Self> {
        // `to_extended_wide` adds the `\\?\` prefix so we can open paths
        // longer than MAX_PATH (260 chars) — see the helper's doc comment.
        let wide = to_extended_wide(path)?;
        let handle = unsafe {
            CreateFileW(
                PCWSTR::from_raw(wide.as_ptr()),
                GENERIC_WRITE.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_ATTRIBUTE_NORMAL,
                None,
            )
        }
        .map_err(|e| io::Error::other(format!("CreateFileW: {e}")))?;

        Ok(Self {
            handle,
            context: std::ptr::null_mut(),
        })
    }

    /// Create (or truncate) the target file and open it for backup writing.
    ///
    /// This is the variant used by the OCI Windows layer unpacker: tar
    /// entries name files that don't yet exist on disk, so we first materialize
    /// an empty file via [`create_long_path_file`] (a `CreateFileW(CREATE_ALWAYS)`
    /// with the `\\?\` long-path prefix) and then reopen it with the
    /// `BACKUP_SEMANTICS` flag via [`Self::create`]. The two-step dance
    /// avoids threading a creation-mode parameter through the raw
    /// `CreateFileW` call.
    ///
    /// `std::fs::File::create` is intentionally avoided here because it does
    /// not apply the extended-length-path prefix, so it fails with
    /// `ERROR_PATH_NOT_FOUND` (0x80070003) for the deep `WinSxS` paths that
    /// appear in `mcr.microsoft.com/windows/nanoserver` layers.
    ///
    /// # Errors
    ///
    /// Returns the OS error from either the initial `CreateFileW(CREATE_ALWAYS)`
    /// or the subsequent `CreateFileW(OPEN_EXISTING)` open.
    pub fn create_new(path: &Path) -> io::Result<Self> {
        // Touch the file via the long-path-aware helper. We immediately drop
        // the returned `File` (closing the write handle) and re-open the same
        // path with `BACKUP_SEMANTICS` via `Self::create`.
        drop(create_long_path_file(path)?);
        Self::create(path)
    }
}

impl Write for BackupStreamWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut bytes_written: u32 = 0;
        unsafe {
            BackupWrite(
                self.handle,
                buf,
                &mut bytes_written,
                false,
                false,
                &mut self.context,
            )
        }
        .map_err(|e| io::Error::other(format!("BackupWrite: {e}")))?;
        Ok(bytes_written as usize)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for BackupStreamWriter {
    fn drop(&mut self) {
        // Tell BackupWrite to release context state with a final call where
        // `babort = true`. windows-rs 0.62 takes `&[u8]` directly (no Option),
        // so we pass an empty slice. Ignore errors; best-effort cleanup.
        let mut bytes_written: u32 = 0;
        unsafe {
            let _ = BackupWrite(
                self.handle,
                &[],
                &mut bytes_written,
                true,
                false,
                &mut self.context,
            );
            let _ = CloseHandle(self.handle);
        }
    }
}

/// Mirror of [`BackupStreamWriter`] for reading existing wclayer content
/// back out (used by `HcsExportLayer` paths).
///
/// The reader emits `BackupRead`-framed bytes: `WIN32_STREAM_ID` headers
/// followed by their payloads. Call [`Read::read`] repeatedly until it
/// returns `Ok(0)` to drain the full stream.
pub struct BackupStreamReader {
    handle: HANDLE,
    context: *mut core::ffi::c_void,
}

// See safety note on `BackupStreamWriter`'s Send impl.
unsafe impl Send for BackupStreamReader {}

impl BackupStreamReader {
    /// Open the target file for backup reading.
    ///
    /// # Errors
    ///
    /// Returns the OS error from `CreateFileW`.
    pub fn open(path: &Path) -> io::Result<Self> {
        // `to_extended_wide` adds the `\\?\` prefix so we can open paths
        // longer than MAX_PATH (260 chars) — see the helper's doc comment.
        let wide = to_extended_wide(path)?;
        let handle = unsafe {
            CreateFileW(
                PCWSTR::from_raw(wide.as_ptr()),
                GENERIC_READ.0,
                FILE_SHARE_READ,
                None,
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS,
                None,
            )
        }
        .map_err(|e| io::Error::other(format!("CreateFileW: {e}")))?;
        Ok(Self {
            handle,
            context: std::ptr::null_mut(),
        })
    }
}

impl Read for BackupStreamReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut bytes_read: u32 = 0;
        unsafe {
            BackupRead(
                self.handle,
                buf,
                &mut bytes_read,
                false,
                false,
                &mut self.context,
            )
        }
        .map_err(|e| io::Error::other(format!("BackupRead: {e}")))?;
        Ok(bytes_read as usize)
    }
}

impl Drop for BackupStreamReader {
    fn drop(&mut self) {
        // Final abort call to release internal context state. See the note on
        // BackupStreamWriter::drop for the empty-slice rationale.
        let mut bytes_read: u32 = 0;
        let mut scratch: [u8; 1] = [0];
        unsafe {
            let _ = BackupRead(
                self.handle,
                &mut scratch[..0],
                &mut bytes_read,
                true,
                false,
                &mut self.context,
            );
            let _ = CloseHandle(self.handle);
        }
    }
}

// ---------------------------------------------------------------------------
// Tests for pure helpers — no Win32 calls, safe to run on any Windows host
// (and they only operate on string-level path transformations, so they don't
// need real files to exist).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: render a wide buffer back to `String`, stripping the trailing
    /// null terminator that `to_extended_wide` always appends.
    fn render(wide: &[u16]) -> String {
        let nul_pos = wide
            .iter()
            .position(|&c| c == 0)
            .expect("to_extended_wide must null-terminate");
        String::from_utf16(&wide[..nul_pos]).expect("valid UTF-16")
    }

    #[test]
    fn extended_wide_prefixes_local_absolute_path() {
        // Use a path that almost certainly does not exist so we exercise the
        // `std::path::absolute` fallback rather than `canonicalize`.
        let p = Path::new(r"C:\foo\bar\does-not-exist-zlayer-test");
        let wide = to_extended_wide(p).expect("absolutize ok");
        let s = render(&wide);
        assert_eq!(s, r"\\?\C:\foo\bar\does-not-exist-zlayer-test");
        assert_eq!(*wide.last().unwrap(), 0u16, "must be null-terminated");
    }

    #[test]
    fn extended_wide_converts_unc_path_to_unc_verbatim() {
        let p = Path::new(r"\\server\share\baz\zlayer-test");
        let wide = to_extended_wide(p).expect("absolutize ok");
        let s = render(&wide);
        assert_eq!(s, r"\\?\UNC\server\share\baz\zlayer-test");
    }

    #[test]
    fn extended_wide_passes_through_already_verbatim_prefix() {
        let p = Path::new(r"\\?\C:\already\zlayer-test");
        let wide = to_extended_wide(p).expect("absolutize ok");
        let s = render(&wide);
        assert_eq!(s, r"\\?\C:\already\zlayer-test");
    }

    #[test]
    fn extended_wide_passes_through_device_prefix() {
        let p = Path::new(r"\\.\PhysicalDrive0");
        let wide = to_extended_wide(p).expect("absolutize ok");
        let s = render(&wide);
        assert_eq!(s, r"\\.\PhysicalDrive0");
    }
}
