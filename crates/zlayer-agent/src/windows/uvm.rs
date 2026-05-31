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

/// Subdirectory inside the per-container UVM directory exposed to the guest
/// as a WRITABLE VSMB share named `zlayer-debug` (mounted at
/// `\\?\VMSMB\VSMB-{dcc079ae-…}\zlayer-debug` inside the guest). Used as a
/// network-free, GCS-free guest→host diagnostic channel: a guest-side
/// injected service writes `sc query gcs`, `wevtutil qe System`, etc., here
/// at first boot and the host reads them back after the step-4 accept
/// timeout.
const DEBUG_SHARE_DIR: &str = "debug";

/// Per-UVM dump-capture directory under `<storage_root>/uvms/<id>/crash/`.
///
/// This is the HOST path the UVM doc's `DebugOptions` (saved-state `.vmrs`) and
/// `GuestCrashReporting.WindowsCrashSettings.DumpFileName` (kernel `.dmp`)
/// point at. It deliberately does NOT overlap with `DEBUG_SHARE_DIR`: that
/// directory is exported into the guest as the writable VSMB share
/// `zlayer-debug`, and HCS refuses to (or silently elides) writing crash
/// artifacts to a path that is also being projected into the guest. Run on
/// 2026-05-29: with `DebugOptions` pointed at the debug-share dir, no `.dmp` /
/// `.vmrs` ever materialised even though Hyper-V-Worker event 18590 confirmed
/// the bugcheck. A dedicated sibling dir works around the conflict.
const CRASH_DIR: &str = "crash";

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
    /// Absolute path to the per-container writable debug directory exposed
    /// to the guest as the VSMB share `zlayer-debug`. The guest writes
    /// diagnostic files (sc query, event-log dumps, etc.) here at first boot;
    /// the host reads them back after the accept timeout.
    debug_dir: PathBuf,
    /// Absolute path to the per-container host-only dump-capture directory
    /// (NOT a VSMB share — see [`CRASH_DIR`]). HCS writes guest crash dumps
    /// (`guest-crash.dmp`) and bugcheck saved-state (`.vmrs`) here.
    crash_dir: PathBuf,
    /// Whether the sandbox VHDX still needs deleting on drop. Cleared by
    /// [`Self::cleanup`] so the `Drop` impl does not double-delete.
    needs_cleanup: bool,
    /// The hvsock VM-ID GUID HCS uses to route GCS-bridge connections into
    /// this UVM. Generated fresh at [`Uvm::create`] time and stashed here so
    /// the caller can (a) inject it into the UVM doc's `RuntimeId` field
    /// before `HcsCreateComputeSystem` and (b) use it to open the GCS
    /// bridge via [`zlayer_gcs::bridge::GcsBridge::connect`]. Mirrors
    /// hcsshim's `internal/uvm/runtime_id.go` — the runtime GUID is the VM
    /// id, one-to-one with the UVM. Container IDs in this codebase are
    /// short slugs (`fallthrough-svc-rep-0`), not GUIDs, so we cannot
    /// derive this from the container id.
    runtime_id: windows::core::GUID,
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
        let debug_dir = uvm_dir.join(DEBUG_SHARE_DIR);
        let crash_dir = uvm_dir.join(CRASH_DIR);

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

        // Create the writable debug directory. Empty at start; the guest
        // diagnostic injector writes files here at first boot.
        std::fs::create_dir_all(&debug_dir).map_err(|e| {
            io::Error::new(
                e.kind(),
                format!("create UVM debug dir {}: {e}", debug_dir.display()),
            )
        })?;

        // Create the host-only crash-capture directory (NOT a VSMB share) so
        // HCS has a write target for guest bugcheck dumps + saved-state files.
        std::fs::create_dir_all(&crash_dir).map_err(|e| {
            io::Error::new(
                e.kind(),
                format!("create UVM crash dir {}: {e}", crash_dir.display()),
            )
        })?;

        // Allocate the UVM's hvsock VM-ID GUID up-front so the caller can
        // both inject it into the UVM doc's `RuntimeId` field AND hand it to
        // [`zlayer_gcs::bridge::GcsBridge::connect`] after start. Falls back
        // to a zeroed GUID if CoCreateGuid fails — defensive only; in
        // practice this RPC never fails on a healthy host.
        let runtime_id =
            windows::core::GUID::new().unwrap_or_else(|_| windows::core::GUID::zeroed());

        // Copy the image's read-only `SystemTemplate.vhdx` to the per-UVM
        // sandbox path. This is hcsshim's documented flow: the sandbox is a
        // writable copy of the template, not an empty disk. Multi-GB copies
        // on Windows are slow via `std::fs::copy` but acceptable for now —
        // hcsshim itself does the same.
        match std::fs::copy(&boot_files.system_template_vhdx, &scratch_vhdx) {
            Ok(_) => {
                // hcsshim calls HcsGrantVmAccess on every host file/dir
                // projected into the UVM so its per-VM virtual SID
                // (`NT VIRTUAL MACHINE\<runtime_id>`) has read access. Without
                // these grants, `HcsStartComputeSystem` fails with
                // `0x80070005 (Access is denied)` at the synthetic storage
                // device's `PowerOnCold` step before the GCS bridge ever
                // comes up.
                //
                // We grant on:
                //  1. The per-UVM sandbox VHDX (mounted via SCSI) — THIS is
                //     the file that fails today.
                //  2. The image's `UtilityVM\Files` directory, surfaced via
                //     the `"os"` VSMB share.
                //  3. The original `SystemTemplate.vhdx` we just copied —
                //     hcsshim grants on the source too even though we copy,
                //     for parity with any code path that re-opens it.
                //
                // Parent layer dirs (also surfaced as VSMB shares) are
                // constructed by the caller in
                // `runtimes::hcs::build_virtual_machine_doc`; granting on
                // those is tracked separately and lives in that file so we
                // don't have to plumb the parent chain through `Uvm::create`.
                // TODO(B-verify.5): grant on each parent layer path in
                // `runtimes::hcs::hyperv_create_via_gcs` before
                // `system.start("")`.
                for path in [
                    scratch_vhdx.as_path(),
                    boot_files.os_files_dir.as_path(),
                    boot_files.system_template_vhdx.as_path(),
                    debug_dir.as_path(),
                    crash_dir.as_path(),
                ] {
                    if let Err(e) = crate::windows::wclayer::grant_vm_access(runtime_id, path) {
                        // Roll back the sandbox copy + UVM directory so we
                        // leave no per-container state behind on failure.
                        let _ = std::fs::remove_file(&scratch_vhdx);
                        let _ = std::fs::remove_dir_all(&debug_dir);
                        let _ = std::fs::remove_dir_all(&crash_dir);
                        let _ = std::fs::remove_dir(&uvm_dir);
                        return Err(io::Error::new(
                            e.kind(),
                            format!("HcsGrantVmAccess({}) failed: {e}", path.display()),
                        ));
                    }
                }
                Ok(Self {
                    container_id: container_id.to_string(),
                    scratch_vhdx,
                    os_files_dir: boot_files.os_files_dir.clone(),
                    debug_dir,
                    crash_dir,
                    needs_cleanup: true,
                    runtime_id,
                })
            }
            Err(e) => {
                // Roll back the directory we just created so we don't leave
                // empty `<storage_root>/uvms/<id>/` litter on failure.
                let _ = std::fs::remove_dir_all(&debug_dir);
                let _ = std::fs::remove_dir_all(&crash_dir);
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

    /// Path to the per-container writable debug directory exposed to the
    /// guest as the VSMB share `zlayer-debug` (mounted at
    /// `\\?\VMSMB\VSMB-{dcc079ae-…}\zlayer-debug` inside the guest).
    #[must_use]
    pub fn debug_dir(&self) -> &Path {
        &self.debug_dir
    }

    /// Path to the per-container host-only crash-capture directory. Used as
    /// the target for the UVM doc's `DebugOptions` saved-state files AND
    /// `GuestCrashReporting.WindowsCrashSettings.DumpFileName`. Distinct
    /// from [`Self::debug_dir`] because that path is exported to the guest
    /// as a writable VSMB share and HCS does not reliably write crash
    /// artifacts there (see [`CRASH_DIR`]).
    #[must_use]
    pub fn crash_dir(&self) -> &Path {
        &self.crash_dir
    }

    /// Container ID this UVM is associated with.
    #[must_use]
    pub fn container_id(&self) -> &str {
        &self.container_id
    }

    /// The hvsock VM-ID GUID HCS uses to route GCS connections into this
    /// UVM. The same GUID is what the caller must:
    ///
    /// 1. Inject into the UVM compute-system doc's `RuntimeId` field
    ///    (`VirtualMachine.RuntimeId`) BEFORE `HcsCreateComputeSystem` so
    ///    HCS pins this VM id rather than auto-generating one.
    /// 2. Pass to [`zlayer_gcs::bridge::GcsBridge::connect`] AFTER the UVM
    ///    is started, so the host-side bridge can address the in-guest GCS
    ///    listener over hvsock.
    ///
    /// Mirrors hcsshim's `internal/uvm/runtime_id.go` — the runtime GUID
    /// IS the VM id, one-to-one with the UVM.
    #[must_use]
    pub fn runtime_id(&self) -> windows::core::GUID {
        self.runtime_id
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
        // Use siblings of the scratch path for the test-only debug + crash
        // dirs; never touched by tests since `needs_cleanup = false`.
        let debug_dir = scratch_vhdx
            .parent()
            .map_or_else(|| scratch_vhdx.with_extension("debug"), |p| p.join("debug"));
        let crash_dir = scratch_vhdx
            .parent()
            .map_or_else(|| scratch_vhdx.with_extension("crash"), |p| p.join("crash"));
        Self {
            container_id: container_id.to_string(),
            scratch_vhdx,
            os_files_dir,
            debug_dir,
            crash_dir,
            needs_cleanup: false,
            // Deterministic per-test GUID. Tests don't open a GCS bridge,
            // so the exact value doesn't matter — only that it's stable.
            runtime_id: windows::core::GUID::from_u128(0xdead_beef_cafe_f00d_1234_5678_9abc_def0),
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
        if keep_uvm_on_failure() {
            tracing::warn!(
                container_id = %self.container_id,
                scratch_vhdx = %self.scratch_vhdx.display(),
                debug_dir = %self.debug_dir.display(),
                "ZLAYER_KEEP_UVM_ON_FAILURE=1 — skipping UVM cleanup (sandbox VHDX and debug dir kept for offline inspection)",
            );
            self.needs_cleanup = false;
            return Ok(());
        }
        let res = remove_sandbox_vhdx(&self.scratch_vhdx);
        let _ = std::fs::remove_dir_all(&self.debug_dir);
        let _ = std::fs::remove_dir_all(&self.crash_dir);
        self.needs_cleanup = false;
        // Best-effort prune of the now-empty container UVM directory.
        if let Some(parent) = self.scratch_vhdx.parent() {
            let _ = std::fs::remove_dir(parent);
        }
        res
    }
}

/// `true` when the operator has set `ZLAYER_KEEP_UVM_ON_FAILURE=1` to keep
/// the per-UVM sandbox VHDX and writable debug directory after a failure so
/// they can be inspected offline / by a subsequent debug script. The default
/// (unset / any other value) is to clean up as before.
fn keep_uvm_on_failure() -> bool {
    matches!(
        std::env::var("ZLAYER_KEEP_UVM_ON_FAILURE").as_deref(),
        Ok("1")
    )
}

impl Drop for Uvm {
    fn drop(&mut self) {
        if !self.needs_cleanup {
            return;
        }
        if keep_uvm_on_failure() {
            tracing::warn!(
                container_id = %self.container_id,
                scratch_vhdx = %self.scratch_vhdx.display(),
                debug_dir = %self.debug_dir.display(),
                "ZLAYER_KEEP_UVM_ON_FAILURE=1 — skipping UVM drop cleanup",
            );
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
        let _ = std::fs::remove_dir_all(&self.debug_dir);
        let _ = std::fs::remove_dir_all(&self.crash_dir);
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
