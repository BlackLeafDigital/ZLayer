//! Writable (scratch) sandbox layer for a Windows container.
//!
//! The scratch layer overlays the parent read-only chain. HCS requires us to:
//!   1. Create a new layer directory on disk.
//!   2. `HcsInitializeWritableLayer` — scaffold the layer metadata and
//!      allocate a sparse `sandbox.vhdx` inside the directory.
//!   3. Open `sandbox.vhdx` for read/write and run
//!      `HcsFormatWritableLayerVhd` against the handle.
//!   4. For a fresh base-OS bootstrap, run `HcsSetupBaseOSLayer` to
//!      materialize the OS-bootstrap files inside the VHD.
//!   5. Query the VHD host mount path via `HcsGetLayerVhdMountPath` while the
//!      handle is still open (required — HCS looks it up against the handle).
//!   6. Close the VHD handle and then `HcsAttachLayerStorageFilter` to
//!      activate WCIFS over the layer.
//!
//! We return the mount path of the VHD so the caller can reference it in the
//! container/VM document. On drop we detach the filter and destroy the layer.
//!
//! The `sandbox.vhdx` file itself is created by `HcsInitializeWritableLayer`;
//! we do not `CreateFileW(CREATE_ALWAYS)` because that would overwrite the
//! VHD HCS just scaffolded. We open it with `OPEN_EXISTING` instead.

#![cfg(target_os = "windows")]
#![allow(unsafe_code)]

use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_READ,
    FILE_SHARE_WRITE, OPEN_EXISTING,
};

use crate::windows::wclayer::{self, LayerChain};

/// Filename HCS uses for the writable-layer VHD inside a scratch layer
/// directory. Matches Microsoft's `hcsshim` convention.
const SANDBOX_VHDX: &str = "sandbox.vhdx";

/// A live writable layer with WCIFS attached. Dropping detaches the filter
/// and destroys the layer directory (including the VHD).
#[derive(Debug)]
pub struct WritableLayer {
    layer_path: PathBuf,
    vhd_mount_path: String,
    /// Whether the WCIFS filter is currently attached. Set to `false` after
    /// an explicit [`Self::detach_and_destroy`] so the `Drop` impl does not
    /// double-detach.
    filter_attached: bool,
    /// Whether the layer directory still needs destroying on drop. Cleared
    /// after an explicit [`Self::detach_and_destroy`].
    needs_destroy: bool,
}

impl WritableLayer {
    /// On-disk path to the writable layer directory.
    #[must_use]
    pub fn layer_path(&self) -> &Path {
        &self.layer_path
    }

    /// Host mount path of the backing VHD, as reported by
    /// `HcsGetLayerVhdMountPath`. Plug this into the container/VM document.
    #[must_use]
    pub fn vhd_mount_path(&self) -> &str {
        &self.vhd_mount_path
    }

    /// Explicitly detach the WCIFS filter and destroy the layer directory.
    ///
    /// Prefer this over relying on `Drop` when you want to surface teardown
    /// errors to the caller. After success, `Drop` becomes a no-op.
    ///
    /// # Errors
    ///
    /// Returns the first error from detach or destroy; the other operation is
    /// still attempted so we do not leak either kernel state or disk.
    pub fn detach_and_destroy(mut self) -> io::Result<()> {
        let detach_res = if self.filter_attached {
            wclayer::detach_layer_storage_filter(&self.layer_path)
        } else {
            Ok(())
        };
        self.filter_attached = false;

        let destroy_res = if self.needs_destroy {
            wclayer::destroy_layer(&self.layer_path)
        } else {
            Ok(())
        };
        self.needs_destroy = false;

        detach_res.and(destroy_res)
    }
}

impl Drop for WritableLayer {
    fn drop(&mut self) {
        if self.filter_attached {
            if let Err(e) = wclayer::detach_layer_storage_filter(&self.layer_path) {
                tracing::warn!(
                    layer = %self.layer_path.display(),
                    error = %e,
                    "detach_layer_storage_filter failed on drop",
                );
            }
        }
        if self.needs_destroy {
            if let Err(e) = wclayer::destroy_layer(&self.layer_path) {
                tracing::warn!(
                    layer = %self.layer_path.display(),
                    error = %e,
                    "destroy_layer failed on drop",
                );
            }
        }
    }
}

/// Build a fresh writable scratch layer backed by a new `sandbox.vhdx` sized
/// at `size_gb`. `parent_chain` is the child-to-parent chain of the
/// underlying read-only image layers (the first entry is the immediate
/// parent; the last is the base OS layer).
///
/// If `is_base_os_bootstrap` is `true`, runs `HcsSetupBaseOSLayer` — needed
/// only the first time an image's base OS layer is prepared. For subsequent
/// container starts off the same base, pass `false`.
///
/// Requires `SeBackupPrivilege` + `SeRestorePrivilege` on the calling process
/// token (see [`crate::windows::layer::enable_backup_restore_privileges`]).
///
/// # Errors
///
/// Returns an [`io::Error`] on any HCS or filesystem failure. On error, any
/// partially-initialized layer directory is best-effort torn down before
/// returning.
pub fn create(
    layer_path: &Path,
    parent_chain: &LayerChain,
    size_gb: u64,
    is_base_os_bootstrap: bool,
) -> io::Result<WritableLayer> {
    std::fs::create_dir_all(layer_path)?;

    // 1. Initialize the writable layer. HCS scaffolds metadata files and
    //    allocates a sparse sandbox.vhdx inside `layer_path`. The options
    //    JSON carries the requested sandbox size in bytes.
    let options_json = build_init_options_json(size_gb);
    if let Err(e) = wclayer::initialize_writable_layer(layer_path, parent_chain, &options_json) {
        cleanup_best_effort(layer_path, false);
        return Err(e);
    }

    // 2. Open the sandbox.vhdx HCS just created. OPEN_EXISTING — do not
    //    overwrite the freshly-scaffolded file.
    let vhd_path = layer_path.join(SANDBOX_VHDX);
    let vhd_handle = match open_sandbox_vhd(&vhd_path) {
        Ok(h) => h,
        Err(e) => {
            cleanup_best_effort(layer_path, false);
            return Err(e);
        }
    };

    // 3. Format, optionally bootstrap base OS, and capture the mount path —
    //    all while the VHD handle is open. We intentionally accumulate the
    //    results and close the handle in all paths before returning.
    let format_result = wclayer::format_writable_layer_vhd(vhd_handle);

    let base_os_result = if format_result.is_ok() && is_base_os_bootstrap {
        // Empty JSON object = HCS defaults; some host builds want
        // `{"Kind":"BaseOSLayer"}` but `{}` is the broadly-compatible choice.
        wclayer::setup_base_os_layer(layer_path, vhd_handle, "{}")
    } else {
        Ok(())
    };

    let mount_result = if format_result.is_ok() && base_os_result.is_ok() {
        wclayer::get_layer_vhd_mount_path(vhd_handle)
    } else {
        Ok(String::new())
    };

    // 4. Close the VHD handle before attaching the filter. WCIFS takes its
    //    own reference to the VHD internally; leaving our handle open here
    //    has historically caused sharing-violation failures on attach.
    //
    // SAFETY: we opened `vhd_handle` above via CreateFileW and nothing else
    // holds it; closing it here is sound regardless of earlier errors.
    unsafe {
        if let Err(e) = CloseHandle(vhd_handle) {
            tracing::warn!(error = %e, "CloseHandle(sandbox.vhdx) failed");
        }
    }

    // Propagate the first error (format → setup → mount-path) now that we've
    // cleaned up the handle.
    if let Err(e) = format_result {
        cleanup_best_effort(layer_path, false);
        return Err(e);
    }
    if let Err(e) = base_os_result {
        cleanup_best_effort(layer_path, false);
        return Err(e);
    }
    let vhd_mount_path = match mount_result {
        Ok(p) => p,
        Err(e) => {
            cleanup_best_effort(layer_path, false);
            return Err(e);
        }
    };

    // 5. Attach WCIFS. After this, reads from `layer_path` present the
    //    merged view to the host.
    if let Err(e) = wclayer::attach_layer_storage_filter(layer_path, parent_chain) {
        cleanup_best_effort(layer_path, false);
        return Err(e);
    }

    Ok(WritableLayer {
        layer_path: layer_path.to_path_buf(),
        vhd_mount_path,
        filter_attached: true,
        needs_destroy: true,
    })
}

/// Build the `options_json` argument passed to `HcsInitializeWritableLayer`.
///
/// HCS recognizes `SandboxSize` (bytes) as the allocation hint for the
/// sparse backing VHDX. `0` means "use HCS default".
fn build_init_options_json(size_gb: u64) -> String {
    if size_gb == 0 {
        return String::from("{}");
    }
    let bytes = size_gb.saturating_mul(1024 * 1024 * 1024);
    format!(r#"{{"SandboxSize":{bytes}}}"#)
}

/// Open an existing `sandbox.vhdx` for read/write. `FILE_FLAG_BACKUP_SEMANTICS`
/// mirrors the flags hcsshim uses and lets the VHD driver service the handle
/// for formatting.
fn open_sandbox_vhd(path: &Path) -> io::Result<HANDLE> {
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: `wide` is a valid null-terminated UTF-16 buffer that outlives
    // the call; all other arguments are plain values.
    unsafe {
        CreateFileW(
            PCWSTR::from_raw(wide.as_ptr()),
            GENERIC_READ.0 | GENERIC_WRITE.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_BACKUP_SEMANTICS,
            None,
        )
    }
    .map_err(|e| io::Error::other(format!("CreateFileW({}): {e}", path.display())))
}

/// Best-effort teardown used when `create` fails partway through. Never
/// panics; emits warnings on failure.
///
/// `filter_attached` must be `true` only if a successful
/// `HcsAttachLayerStorageFilter` call is on the books for `layer_path`.
fn cleanup_best_effort(layer_path: &Path, filter_attached: bool) {
    if filter_attached {
        if let Err(e) = wclayer::detach_layer_storage_filter(layer_path) {
            tracing::warn!(
                layer = %layer_path.display(),
                error = %e,
                "detach_layer_storage_filter failed during rollback",
            );
        }
    }
    if let Err(e) = wclayer::destroy_layer(layer_path) {
        tracing::warn!(
            layer = %layer_path.display(),
            error = %e,
            "destroy_layer failed during rollback",
        );
    }
}

// ---------------------------------------------------------------------------
// Tests (no HCS calls — only pure helpers).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_options_zero_size_is_defaults() {
        assert_eq!(build_init_options_json(0), "{}");
    }

    #[test]
    fn init_options_size_encodes_bytes() {
        let json = build_init_options_json(20);
        assert!(json.contains("\"SandboxSize\""));
        assert!(json.contains(&(20u64 * 1024 * 1024 * 1024).to_string()));
    }

    #[test]
    fn init_options_saturates_on_huge_size() {
        // No panic on absurd inputs.
        let json = build_init_options_json(u64::MAX);
        assert!(json.starts_with("{\"SandboxSize\":"));
    }
}
