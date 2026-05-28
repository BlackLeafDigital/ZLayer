//! Hyper-V utility VM (UVM) lifecycle.
//!
//! One UVM backs one isolated container. Provisions a per-container sandbox
//! VHDX (a writable copy of the image's `UtilityVM\SystemTemplate.vhdx`) and
//! holds the path to the image's `UtilityVM\Files` directory — the OS root
//! the UVM boots from over the `"os"` VSMB share. Cleans up the sandbox VHDX
//! on drop.
//!
//! Unlike legacy hcsshim flows that probe the host's
//! `%ProgramData%\Microsoft\Windows\Hyper-V\Containers` directory for boot
//! files, `ZLayer` treats the bundled Windows OCI image as the authoritative
//! source of UVM payload — the image carries its own `UtilityVM\Files` tree
//! and `UtilityVM\SystemTemplate.vhdx`. The caller resolves these via
//! [`crate::windows::unpacker::locate_uvm_boot_files`] before invoking
//! [`Uvm::create`].
//!
//! This module owns provisioning + teardown only. It does NOT build the
//! `VirtualMachine` HCS schema document — that's the caller's job (3.D).

#![cfg(target_os = "windows")]

use std::io;
use std::path::{Path, PathBuf};

use crate::windows::unpacker::UvmBootFiles;

/// Subdirectory under `<storage_root>` where per-container UVM state lives.
/// Each container's UVM gets its own subdir at `<storage_root>/uvms/<container_id>/`.
const UVMS_SUBDIR: &str = "uvms";

/// Filename of the sandbox VHDX inside the per-container UVM directory.
/// Produced by copying the image's `UtilityVM\SystemTemplate.vhdx` so the
/// UVM has a writable system disk per instance.
const SANDBOX_VHDX: &str = "sandbox.vhdx";

/// One Hyper-V utility VM that backs a single isolated container.
///
/// Holds the paths the caller needs to populate a `VirtualMachine` compute-
/// system document and owns the per-container sandbox VHDX on disk. The
/// sandbox VHDX is removed best-effort on `Drop`; the image's OS-files
/// directory is image-owned and never touched.
#[derive(Debug)]
pub struct Uvm {
    /// Container ID this UVM belongs to. Used for log context.
    container_id: String,
    /// Absolute path to the per-container sandbox VHDX created at
    /// `<storage_root>/uvms/<container_id>/sandbox.vhdx` by copying the
    /// image's `UtilityVM\SystemTemplate.vhdx`.
    scratch_vhdx: PathBuf,
    /// Absolute path to the image's `UtilityVM\Files` directory — surfaced
    /// to the UVM over the `"os"` VSMB share as its OS root.
    os_files_dir: PathBuf,
    /// Whether the sandbox VHDX still needs deleting on drop. Cleared by
    /// [`Self::cleanup`] so the `Drop` impl does not double-delete.
    needs_cleanup: bool,
}

impl Uvm {
    /// Provision a fresh UVM for the given container.
    ///
    /// Stages a per-container sandbox VHDX under
    /// `<storage_root>/uvms/<container_id>/sandbox.vhdx` by COPYING
    /// `boot_files.system_template_vhdx` (the image's read-only system
    /// template). Records the image's `UtilityVM\Files` path so the caller
    /// can wire the `"os"` VSMB share into the compute-system document.
    ///
    /// The returned struct's `Drop` impl cleans up the sandbox VHDX
    /// best-effort.
    ///
    /// # Errors
    ///
    /// Returns `io::Error` if:
    /// - `container_id` is empty.
    /// - The per-container UVM directory cannot be created under
    ///   `<storage_root>/uvms/<container_id>/`.
    /// - Copying `system_template_vhdx` → `sandbox.vhdx` fails (most likely
    ///   from a missing template file, disk full, or insufficient
    ///   permissions on `storage_root`).
    pub fn create(
        container_id: &str,
        storage_root: &Path,
        boot_files: &UvmBootFiles,
    ) -> io::Result<Self> {
        if container_id.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "container_id must not be empty",
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

        let scratch_vhdx = uvm_dir.join(SANDBOX_VHDX);

        // If a stale VHDX exists from a previous failed run, remove it so
        // `std::fs::copy` overwrites cleanly without surfacing surprising
        // partial-state errors. Safe: a fresh UVM never inherits state from
        // a prior container.
        if scratch_vhdx.exists() {
            std::fs::remove_file(&scratch_vhdx).map_err(|e| {
                io::Error::new(
                    e.kind(),
                    format!("remove stale sandbox VHDX {}: {e}", scratch_vhdx.display()),
                )
            })?;
        }

        // Copy the image's read-only `SystemTemplate.vhdx` to the per-UVM
        // sandbox path. This is hcsshim's documented flow: the sandbox is a
        // writable copy of the template, not an empty disk. Multi-GB copies
        // on Windows are slow via `std::fs::copy` but acceptable for now —
        // hcsshim itself does the same.
        match std::fs::copy(&boot_files.system_template_vhdx, &scratch_vhdx) {
            Ok(_) => Ok(Self {
                container_id: container_id.to_string(),
                scratch_vhdx,
                os_files_dir: boot_files.os_files_dir.clone(),
                needs_cleanup: true,
            }),
            Err(e) => {
                // Roll back the directory we just created so we don't leave
                // empty `<storage_root>/uvms/<id>/` litter on failure.
                let _ = std::fs::remove_dir(&uvm_dir);
                Err(io::Error::new(
                    e.kind(),
                    format!(
                        "copy UVM SystemTemplate {} -> {}: {e}",
                        boot_files.system_template_vhdx.display(),
                        scratch_vhdx.display(),
                    ),
                ))
            }
        }
    }

    /// Path to the sandbox VHDX for SCSI attachment in the compute-system doc.
    #[must_use]
    pub fn scratch_vhdx(&self) -> &Path {
        &self.scratch_vhdx
    }

    /// Path to the image's `UtilityVM\Files` directory — surfaced to the UVM
    /// over the `"os"` VSMB share.
    #[must_use]
    pub fn os_files_dir(&self) -> &Path {
        &self.os_files_dir
    }

    /// Container ID this UVM is associated with.
    #[must_use]
    pub fn container_id(&self) -> &str {
        &self.container_id
    }

    /// Test-only constructor that fabricates a [`Uvm`] from caller-supplied
    /// paths without touching the filesystem or copying any VHDX. Used by
    /// integration tests in [`crate::runtimes::hcs`] that exercise the
    /// Hyper-V branch of `build_compute_system_doc` without a live Windows
    /// host.
    ///
    /// Sets `needs_cleanup = false` so `Drop` never tries to delete the
    /// caller's fixture files.
    #[cfg(test)]
    #[must_use]
    pub fn for_test(container_id: &str, scratch_vhdx: PathBuf, os_files_dir: PathBuf) -> Self {
        Self {
            container_id: container_id.to_string(),
            scratch_vhdx,
            os_files_dir,
            needs_cleanup: false,
        }
    }

    /// Explicitly remove the sandbox VHDX. Prefer this over relying on
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
        let res = remove_sandbox_vhdx(&self.scratch_vhdx);
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
        if let Err(e) = remove_sandbox_vhdx(&self.scratch_vhdx) {
            tracing::warn!(
                container_id = %self.container_id,
                vhdx = %self.scratch_vhdx.display(),
                error = %e,
                "sandbox VHDX cleanup failed on Uvm drop",
            );
        }
        if let Some(parent) = self.scratch_vhdx.parent() {
            // Best-effort prune of the empty per-container dir.
            let _ = std::fs::remove_dir(parent);
        }
    }
}

/// Best-effort sandbox VHDX removal. Ignores "not found" — if the file is
/// already gone someone else cleaned up first.
fn remove_sandbox_vhdx(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

// ---------------------------------------------------------------------------
// Tests — Windows-only. Mirrors the `#![cfg(target_os = "windows")]` pattern
// used by `scratch.rs` and `wclayer.rs`; the parent `windows` module itself
// is gated at lib.rs, so this file is never compiled on non-Windows targets.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a fake [`UvmBootFiles`] whose `system_template_vhdx` points at a
    /// real on-disk file inside `tmp` so [`Uvm::create`] can copy it. The
    /// other path fields are filled with placeholders.
    fn fixture_boot_files(tmp: &Path, template_bytes: &[u8]) -> UvmBootFiles {
        let uvm_dir = tmp.join("layer-uvm");
        let files_dir = uvm_dir.join("Files");
        let template = uvm_dir.join("SystemTemplate.vhdx");
        std::fs::create_dir_all(&files_dir).expect("mkdir files");
        std::fs::write(&template, template_bytes).expect("write template");
        UvmBootFiles {
            uvm_layer_dir: uvm_dir,
            os_files_dir: files_dir,
            system_template_vhdx: template,
            boot_rel_path: r"\EFI\Microsoft\Boot\bootmgfw.efi",
        }
    }

    #[test]
    fn remove_sandbox_vhdx_is_idempotent_when_missing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let absent = tmp.path().join("never_existed.vhdx");
        remove_sandbox_vhdx(&absent).expect("idempotent remove");
    }

    #[test]
    fn create_with_empty_container_id_fails() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let boot = fixture_boot_files(tmp.path(), b"fake vhdx");
        let err = Uvm::create("", tmp.path(), &boot).expect_err("empty container id must fail");
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    /// When the bundled `SystemTemplate.vhdx` is missing, `create` surfaces
    /// the underlying filesystem error from `std::fs::copy` rather than
    /// silently producing an empty disk.
    #[test]
    fn create_with_missing_template_fails() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut boot = fixture_boot_files(tmp.path(), b"fake vhdx");
        // Delete the template after construction so the `is_file()` check
        // would have passed at locate time but the copy still fails.
        std::fs::remove_file(&boot.system_template_vhdx).expect("rm template");
        // Re-aim at a definitely-absent path too, defensively.
        boot.system_template_vhdx = tmp.path().join("does-not-exist.vhdx");

        let err = Uvm::create("ctr-no-template", tmp.path(), &boot)
            .expect_err("missing template must fail");
        assert!(
            err.kind() == io::ErrorKind::NotFound || err.kind() == io::ErrorKind::PermissionDenied,
            "unexpected error kind: {:?}",
            err.kind(),
        );
    }

    /// Happy-path: copying the template produces a sandbox VHDX with the
    /// same bytes, and `Drop` cleans it up.
    #[test]
    fn create_copies_template_to_sandbox_vhdx() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let payload = b"PRETEND THIS IS A VHDX HEADER";
        let boot = fixture_boot_files(tmp.path(), payload);

        let storage_root = tmp.path().join("agent-state");
        let sandbox_path = {
            let uvm = Uvm::create("ctr-copy", &storage_root, &boot).expect("create UVM");
            assert!(uvm.scratch_vhdx().is_file(), "sandbox VHDX should exist");
            assert_eq!(
                std::fs::read(uvm.scratch_vhdx()).expect("read sandbox"),
                payload,
                "sandbox VHDX must be a byte-for-byte copy of the template",
            );
            assert_eq!(uvm.container_id(), "ctr-copy");
            assert_eq!(uvm.os_files_dir(), boot.os_files_dir.as_path());
            uvm.scratch_vhdx().to_path_buf()
        };
        // After Drop, the sandbox VHDX should be cleaned up.
        assert!(
            !sandbox_path.exists(),
            "sandbox VHDX must be removed on Uvm drop",
        );
    }
}
