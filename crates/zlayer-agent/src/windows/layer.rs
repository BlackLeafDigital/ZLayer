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
use std::path::Path;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE, LUID};
use windows::Win32::Security::{
    AdjustTokenPrivileges, LookupPrivilegeValueW, LUID_AND_ATTRIBUTES, SE_PRIVILEGE_ENABLED,
    TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
};
use windows::Win32::Storage::FileSystem::{
    BackupRead, BackupWrite, CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

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
        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
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
    /// an empty file via `File::create` (which maps to `CREATE_ALWAYS` on
    /// Windows) and then reopen it with the `BACKUP_SEMANTICS` flag via
    /// [`Self::create`]. The two-step dance avoids threading a creation-mode
    /// parameter through the raw `CreateFileW` call.
    ///
    /// # Errors
    ///
    /// Returns the OS error from either the initial `File::create` or the
    /// subsequent `CreateFileW` open.
    pub fn create_new(path: &Path) -> io::Result<Self> {
        // Touch the file so `CreateFileW(OPEN_EXISTING, ...)` below succeeds.
        // Drop the handle immediately — we only need the inode to exist.
        drop(std::fs::File::create(path)?);
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
        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
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
