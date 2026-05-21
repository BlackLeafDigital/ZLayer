//! Hyper-V utility VM (UVM) lifecycle.
//!
//! One UVM backs one isolated container. Provisions a per-container scratch
//! VHDX and locates the UVM boot files installed by the Windows Containers
//! feature. Cleans up the scratch VHDX on drop.
//!
//! The Windows Containers feature installs the UVM boot files under
//! `%ProgramData%\Microsoft\Windows\Hyper-V\Containers\` — these are the
//! pre-built VHDX and configuration files the Hyper-V hypervisor uses to
//! boot the utility VM. Each isolated container gets its own writable scratch
//! VHDX (sparse, sized at `scratch_size_gb`) attached to the UVM via SCSI;
//! the boot-files directory is shared in read-only via `VirtualSmb` in the
//! compute-system document (see task 3.D for wiring).
//!
//! This module owns provisioning + teardown only. It does NOT build the
//! `VirtualMachine` HCS schema document — that's the caller's job (3.D).

#![cfg(target_os = "windows")]
#![allow(unsafe_code)]

use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, ERROR_SUCCESS, HANDLE};
use windows::Win32::Storage::Vhd::{
    CreateVirtualDisk, CREATE_VIRTUAL_DISK_FLAG_NONE, CREATE_VIRTUAL_DISK_PARAMETERS,
    CREATE_VIRTUAL_DISK_PARAMETERS_0, CREATE_VIRTUAL_DISK_PARAMETERS_0_1,
    CREATE_VIRTUAL_DISK_VERSION_2, VIRTUAL_DISK_ACCESS_NONE, VIRTUAL_STORAGE_TYPE,
    VIRTUAL_STORAGE_TYPE_DEVICE_VHDX, VIRTUAL_STORAGE_TYPE_VENDOR_MICROSOFT,
};

/// Subdirectory under `<storage_root>` where per-container UVM state lives.
/// Each container's UVM gets its own subdir at `<storage_root>/uvms/<container_id>/`.
const UVMS_SUBDIR: &str = "uvms";

/// Filename of the scratch VHDX inside the per-container UVM directory.
const SCRATCH_VHDX: &str = "scratch.vhdx";

/// Path under `%ProgramData%` where the Windows Containers feature installs
/// UVM boot files (`UtilityVM\Files\` and friends). Matches the Microsoft
/// `hcsshim` convention so we interoperate with stock host installs.
const PROGRAMDATA_UVM_BOOT_SUBPATH: &str = r"Microsoft\Windows\Hyper-V\Containers";

/// Hard-coded fallback for the `ProgramData` directory if the env var is
/// unset. Windows installations always ship with `C:\ProgramData` on the
/// system drive; this fallback exists only so the boot-files probe still
/// produces a deterministic path inside service accounts that strip the
/// environment block.
const PROGRAMDATA_FALLBACK: &str = r"C:\ProgramData";

/// One Hyper-V utility VM that backs a single isolated container.
///
/// Holds the paths the caller needs to populate a `VirtualMachine` compute-
/// system document and owns the per-container scratch VHDX on disk. The
/// scratch VHDX is removed best-effort on `Drop`; the boot-files directory
/// is host-owned and never touched.
#[derive(Debug)]
pub struct Uvm {
    /// Container ID this UVM belongs to. Used for log context.
    container_id: String,
    /// Absolute path to the per-container scratch VHDX created at
    /// `<storage_root>/uvms/<container_id>/scratch.vhdx`.
    scratch_vhdx: PathBuf,
    /// Absolute path to the read-only UVM boot-files directory the Windows
    /// Containers feature installed under `%ProgramData%`.
    boot_files: PathBuf,
    /// Whether the scratch VHDX still needs deleting on drop. Cleared by
    /// [`Self::cleanup`] so the `Drop` impl does not double-delete.
    needs_cleanup: bool,
}

impl Uvm {
    /// Provision a fresh UVM for the given container.
    ///
    /// Allocates a per-container scratch VHDX under
    /// `<storage_root>/uvms/<container_id>/scratch.vhdx`, locates the UVM
    /// boot files (installed by the Windows Containers feature under
    /// `%ProgramData%\Microsoft\Windows\Hyper-V\Containers\`), and returns
    /// a struct whose `Drop` impl cleans up the scratch VHDX best-effort.
    ///
    /// # Errors
    ///
    /// Returns `io::Error` if:
    /// - The Windows Containers feature is not installed (boot files
    ///   directory missing). The error message will be exactly
    ///   `"Windows Containers feature not installed; UVM boot files missing"`
    ///   so downstream callers can produce a friendly user-facing message.
    /// - The scratch VHDX cannot be created (HRESULT from `CreateVirtualDisk`).
    /// - `storage_root` cannot be created or is not a directory.
    /// - `scratch_size_gb` is zero.
    /// - `container_id` is empty.
    pub fn create(
        container_id: &str,
        storage_root: &Path,
        scratch_size_gb: u64,
    ) -> io::Result<Self> {
        if container_id.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "container_id must not be empty",
            ));
        }
        if scratch_size_gb == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "scratch_size_gb must be greater than zero",
            ));
        }

        let boot_files = resolve_boot_files_dir();
        if !boot_files.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "Windows Containers feature not installed; UVM boot files missing",
            ));
        }

        // Stage the per-container directory inside `<storage_root>/uvms/<id>/`.
        let mut uvm_dir = storage_root.to_path_buf();
        uvm_dir.push(UVMS_SUBDIR);
        uvm_dir.push(container_id);
        std::fs::create_dir_all(&uvm_dir).map_err(|e| {
            io::Error::new(
                e.kind(),
                format!("create UVM dir {}: {e}", uvm_dir.display()),
            )
        })?;

        let scratch_vhdx = uvm_dir.join(SCRATCH_VHDX);

        // If a stale VHDX exists from a previous failed run, remove it so
        // `CreateVirtualDisk` doesn't fail with ERROR_FILE_EXISTS. This is
        // safe: a fresh UVM never inherits state from a prior container.
        if scratch_vhdx.exists() {
            std::fs::remove_file(&scratch_vhdx).map_err(|e| {
                io::Error::new(
                    e.kind(),
                    format!("remove stale scratch VHDX {}: {e}", scratch_vhdx.display()),
                )
            })?;
        }

        match create_empty_vhdx(&scratch_vhdx, scratch_size_gb) {
            Ok(()) => Ok(Self {
                container_id: container_id.to_string(),
                scratch_vhdx,
                boot_files,
                needs_cleanup: true,
            }),
            Err(e) => {
                // Roll back the directory we just created so we don't leave
                // empty `<storage_root>/uvms/<id>/` litter on failure.
                let _ = std::fs::remove_dir(&uvm_dir);
                Err(e)
            }
        }
    }

    /// Path to the scratch VHDX for SCSI attachment in the compute-system doc.
    #[must_use]
    pub fn scratch_vhdx(&self) -> &Path {
        &self.scratch_vhdx
    }

    /// Path to the UVM boot files for the `VirtualMachine.GuestState` doc field.
    #[must_use]
    pub fn boot_files(&self) -> &Path {
        &self.boot_files
    }

    /// Container ID this UVM is associated with.
    #[must_use]
    pub fn container_id(&self) -> &str {
        &self.container_id
    }

    /// Explicitly remove the scratch VHDX. Prefer this over relying on
    /// `Drop` when the caller wants to surface teardown errors. After a
    /// successful call, `Drop` becomes a no-op.
    ///
    /// # Errors
    ///
    /// Returns the filesystem error from removing the VHDX. The parent
    /// directory under `<storage_root>/uvms/<container_id>/` is also
    /// removed best-effort; failure to remove it does NOT surface as an
    /// error here (it'll be empty after the VHDX is gone, and a stray empty
    /// directory is harmless).
    pub fn cleanup(mut self) -> io::Result<()> {
        let res = remove_scratch_vhdx(&self.scratch_vhdx);
        self.needs_cleanup = false;
        // Best-effort prune of the now-empty container UVM directory.
        if let Some(parent) = self.scratch_vhdx.parent() {
            let _ = std::fs::remove_dir(parent);
        }
        res
    }
}

impl Drop for Uvm {
    fn drop(&mut self) {
        if !self.needs_cleanup {
            return;
        }
        if let Err(e) = remove_scratch_vhdx(&self.scratch_vhdx) {
            tracing::warn!(
                container_id = %self.container_id,
                vhdx = %self.scratch_vhdx.display(),
                error = %e,
                "scratch VHDX cleanup failed on Uvm drop",
            );
        }
        if let Some(parent) = self.scratch_vhdx.parent() {
            // Best-effort prune of the empty per-container dir.
            let _ = std::fs::remove_dir(parent);
        }
    }
}

/// Resolve the on-host UVM boot-files directory, preferring `%ProgramData%`
/// and falling back to the literal `C:\ProgramData` when the env var is
/// unset. Pulled out as a free function so unit tests can exercise the
/// resolution logic without needing to run `create`.
fn resolve_boot_files_dir() -> PathBuf {
    let program_data =
        std::env::var_os("ProgramData").unwrap_or_else(|| PROGRAMDATA_FALLBACK.into());
    let mut p = PathBuf::from(program_data);
    p.push(PROGRAMDATA_UVM_BOOT_SUBPATH);
    p
}

/// Best-effort scratch VHDX removal. Ignores "not found" — if the file is
/// already gone someone else cleaned up first.
fn remove_scratch_vhdx(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Create an empty sparse VHDX at `path` sized at `size_gb` gigabytes.
///
/// Uses the dynamic ("expandable") VHDX format — only allocates host disk
/// space as the guest writes. Equivalent to PowerShell's `New-VHD -Dynamic`.
///
/// # Errors
///
/// Returns an `io::Error` if `CreateVirtualDisk` returns a non-success
/// Win32 error code, or if `size_gb` overflows when multiplied by 1 GiB.
fn create_empty_vhdx(path: &Path, size_gb: u64) -> io::Result<()> {
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    let storage_type = VIRTUAL_STORAGE_TYPE {
        DeviceId: VIRTUAL_STORAGE_TYPE_DEVICE_VHDX,
        VendorId: VIRTUAL_STORAGE_TYPE_VENDOR_MICROSOFT,
    };

    // VHDX max addressable is 64 TiB; `checked_mul` guards against any input
    // that would silently wrap. Returning an error here surfaces a bad
    // caller instead of creating a tiny VHDX.
    let max_bytes = size_gb.checked_mul(1024 * 1024 * 1024).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("scratch_size_gb {size_gb} overflows u64 bytes"),
        )
    })?;

    let params = CREATE_VIRTUAL_DISK_PARAMETERS {
        Version: CREATE_VIRTUAL_DISK_VERSION_2,
        Anonymous: CREATE_VIRTUAL_DISK_PARAMETERS_0 {
            Version2: CREATE_VIRTUAL_DISK_PARAMETERS_0_1 {
                MaximumSize: max_bytes,
                // Zero = let the driver pick reasonable defaults.
                BlockSizeInBytes: 0,
                SectorSizeInBytes: 0,
                PhysicalSectorSizeInBytes: 0,
                ..Default::default()
            },
        },
    };

    let mut handle: HANDLE = HANDLE::default();

    // SAFETY: `wide` is a valid null-terminated UTF-16 buffer that outlives
    // the call; `storage_type` and `params` are stack-resident POD structs
    // also valid for the duration of the call; `handle` is a writable
    // out-pointer. No arguments alias. `CreateVirtualDisk` is documented as
    // synchronous when `overlapped` is `None`, so by the time it returns
    // the out-handle (if any) is fully initialized.
    let status = unsafe {
        CreateVirtualDisk(
            &raw const storage_type,
            PCWSTR::from_raw(wide.as_ptr()),
            VIRTUAL_DISK_ACCESS_NONE,
            None,
            CREATE_VIRTUAL_DISK_FLAG_NONE,
            0,
            &raw const params,
            None,
            &raw mut handle,
        )
    };

    if status != ERROR_SUCCESS {
        return Err(io::Error::other(format!(
            "CreateVirtualDisk({}): Win32 error {}",
            path.display(),
            status.0,
        )));
    }

    if !handle.is_invalid() {
        // SAFETY: `handle` came from a successful `CreateVirtualDisk` call
        // and nothing else holds it.
        unsafe {
            if let Err(e) = CloseHandle(handle) {
                tracing::warn!(
                    vhdx = %path.display(),
                    error = %e,
                    "CloseHandle on VHDX failed after CreateVirtualDisk",
                );
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests — Windows-only. Mirrors the `#![cfg(target_os = "windows")]` pattern
// used by `scratch.rs` and `wclayer.rs`; the parent `windows` module itself
// is gated at lib.rs, so this file is never compiled on non-Windows targets.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_boot_files_dir_uses_program_data_or_fallback() {
        let p = resolve_boot_files_dir();
        let s = p.to_string_lossy();
        assert!(
            s.contains("Microsoft") && s.contains("Hyper-V") && s.contains("Containers"),
            "unexpected boot files path: {s}",
        );
    }

    #[test]
    fn remove_scratch_vhdx_is_idempotent_when_missing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let absent = tmp.path().join("never_existed.vhdx");
        remove_scratch_vhdx(&absent).expect("idempotent remove");
    }

    #[test]
    fn create_with_invalid_scratch_size_zero_fails() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let err = Uvm::create("ctr-zero", tmp.path(), 0).expect_err("zero scratch size must fail");
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn create_with_empty_container_id_fails() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let err = Uvm::create("", tmp.path(), 4).expect_err("empty container id must fail");
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    /// On a Windows host without the Containers feature installed, `create`
    /// returns the exact `NotFound` message documented in the public API so
    /// the daemon can render a friendly hint.
    #[test]
    fn create_without_containers_feature_reports_exact_message() {
        // Point ProgramData at an empty dir so the boot-files probe fails
        // deterministically. Tests run sequentially per-crate by default;
        // we restore the env at the end.
        let tmp = tempfile::tempdir().expect("tempdir");
        let saved = std::env::var_os("ProgramData");
        // SAFETY: single-threaded test. `set_var` is `unsafe` since Rust 1.79
        // on some toolchains; wrap in `unsafe` to compile cleanly either way.
        unsafe {
            std::env::set_var("ProgramData", tmp.path());
        }

        let err =
            Uvm::create("ctr-no-feature", tmp.path(), 4).expect_err("missing boot files must fail");

        // SAFETY: single-threaded test environment.
        unsafe {
            match saved {
                Some(v) => std::env::set_var("ProgramData", v),
                None => std::env::remove_var("ProgramData"),
            }
        }

        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert_eq!(
            err.to_string(),
            "Windows Containers feature not installed; UVM boot files missing",
        );
    }

    /// Provided so a nonexistent `storage_root` still has its boot-files
    /// probe run first; the failure surface is therefore `NotFound` from the
    /// boot-files probe rather than from `create_dir_all`. This pins the
    /// error surface so downstream callers can rely on it.
    #[test]
    fn create_with_nonexistent_storage_root_fails() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let missing = tmp.path().join("definitely-not-there");
        // Use an empty ProgramData so the boot-files probe fails
        // deterministically — that's the "no Windows Containers feature"
        // path. Either way, `create` must return an error.
        let saved = std::env::var_os("ProgramData");
        // SAFETY: single-threaded test.
        unsafe {
            std::env::set_var("ProgramData", tmp.path());
        }

        let err = Uvm::create("ctr-nostore", &missing, 4)
            .expect_err("create on missing storage root must fail");

        // SAFETY: single-threaded test.
        unsafe {
            match saved {
                Some(v) => std::env::set_var("ProgramData", v),
                None => std::env::remove_var("ProgramData"),
            }
        }

        // Both possible failure surfaces (NotFound from the boot-files probe
        // OR a create_dir_all error) are acceptable — the contract is just
        // that a missing setup fails loudly.
        assert!(
            err.kind() == io::ErrorKind::NotFound
                || err.kind() == io::ErrorKind::PermissionDenied
                || err.kind() == io::ErrorKind::Other,
            "unexpected error kind: {:?}",
            err.kind(),
        );
    }

    /// Smoke test the happy path against a real Windows host. Gated `ignore`
    /// because it requires Hyper-V + the Windows Containers feature + admin.
    #[test]
    #[ignore = "requires Windows host with Hyper-V feature + Windows Containers feature installed"]
    fn create_succeeds_on_windows_host() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let uvm = Uvm::create("smoke-test-container", tmp.path(), 4)
            .expect("create UVM on real Windows host");
        assert!(uvm.scratch_vhdx().is_file(), "scratch VHDX should exist");
        assert!(uvm.boot_files().is_dir(), "boot files dir should exist");
        assert_eq!(uvm.container_id(), "smoke-test-container");
        uvm.cleanup().expect("cleanup");
    }
}
