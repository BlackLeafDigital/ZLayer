//! Writable (scratch) sandbox layer for a Windows container.
//!
//! Mirrors hcsshim's canonical scratch-creation + mount sequence:
//!
//! * `internal/wclayer/createscratchlayer.go::CreateScratchLayer` (a single
//!   `vmcompute.dll!CreateSandboxLayer` call that scaffolds the layer dir
//!   AND fully formats the backing `sandbox.vhdx`).
//! * `internal/layers/wcow_mount.go::mountProcessIsolatedWCIFSLayers`
//!   (`ActivateLayer` → `PrepareLayer` → `GetLayerMountPath`).
//!
//! There is intentionally no separate "format VHD" step. `CreateSandboxLayer`
//! produces a fully-formatted VHD; an additional `HcsFormatWritableLayerVhd`
//! call against the resulting VHD returns `E_INVALIDARG` because the VHD is
//! already formatted. (`HcsFormatWritableLayerVhd` is for the *other* HCS API
//! pattern where the caller creates a VHD via `CreateVirtualDisk` themselves.)
//!
//! Teardown is the inverse order: `UnprepareLayer` → `DeactivateLayer` →
//! `destroy_layer`.

#![cfg(target_os = "windows")]
#![allow(unsafe_code)]

use std::io;
use std::path::{Path, PathBuf};

use crate::windows::wclayer::{self, LayerChain};

/// A live writable layer with its filter activated + prepared. Dropping
/// unprepares + deactivates + destroys the layer directory.
#[derive(Debug)]
pub struct WritableLayer {
    layer_path: PathBuf,
    mount_path: String,
    /// `true` while `PrepareLayer` has been called and `UnprepareLayer` has
    /// not. Cleared after an explicit [`Self::detach_and_destroy`] so the
    /// `Drop` impl does not double-unprepare.
    prepared: bool,
    /// `true` while `ActivateLayer` has been called and `DeactivateLayer` has
    /// not. Cleared after an explicit [`Self::detach_and_destroy`] so the
    /// `Drop` impl does not double-deactivate.
    activated: bool,
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

    /// Host mount path of the prepared layer (as reported by
    /// `GetLayerMountPath`). Plug this into the container/VM document.
    #[must_use]
    pub fn vhd_mount_path(&self) -> &str {
        &self.mount_path
    }

    /// Explicitly tear down the layer: `UnprepareLayer` → `DeactivateLayer`
    /// → `destroy_layer`.
    ///
    /// Prefer this over relying on `Drop` when you want to surface teardown
    /// errors to the caller. After success, `Drop` becomes a no-op.
    ///
    /// # Errors
    ///
    /// Returns the first error from any of the three steps; the remaining
    /// steps are still attempted so kernel state and disk are not leaked.
    pub fn detach_and_destroy(mut self) -> io::Result<()> {
        let unprepare_res = if self.prepared {
            wclayer::unprepare_layer(&self.layer_path)
        } else {
            Ok(())
        };
        self.prepared = false;

        let deactivate_res = if self.activated {
            wclayer::deactivate_layer(&self.layer_path)
        } else {
            Ok(())
        };
        self.activated = false;

        let destroy_res = if self.needs_destroy {
            wclayer::destroy_layer(&self.layer_path)
        } else {
            Ok(())
        };
        self.needs_destroy = false;

        unprepare_res.and(deactivate_res).and(destroy_res)
    }
}

impl Drop for WritableLayer {
    fn drop(&mut self) {
        if self.prepared {
            if let Err(e) = wclayer::unprepare_layer(&self.layer_path) {
                tracing::warn!(
                    layer = %self.layer_path.display(),
                    error = %e,
                    "unprepare_layer failed on drop",
                );
            }
        }
        if self.activated {
            if let Err(e) = wclayer::deactivate_layer(&self.layer_path) {
                tracing::warn!(
                    layer = %self.layer_path.display(),
                    error = %e,
                    "deactivate_layer failed on drop",
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

/// Build a fresh writable scratch layer over `parent_chain`.
///
/// `parent_chain` is the child-to-parent chain of the underlying read-only
/// image layers (the first entry is the immediate parent; the last is the
/// base OS layer).
///
/// Requires `SeBackupPrivilege` + `SeRestorePrivilege` on the calling process
/// token (see [`crate::windows::layer::enable_backup_restore_privileges`]).
///
/// # Errors
///
/// Returns an [`io::Error`] on any HCS or filesystem failure. On error, any
/// partially-initialized layer directory is best-effort torn down before
/// returning.
pub fn create(layer_path: &Path, parent_chain: &LayerChain) -> io::Result<WritableLayer> {
    std::fs::create_dir_all(layer_path)?;

    // 1. `CreateSandboxLayer` scaffolds the layer directory AND fully
    //    formats the backing sandbox.vhdx. Single call — no separate
    //    format step.
    if let Err(e) = wclayer::create_sandbox_layer(layer_path, parent_chain) {
        cleanup_best_effort(layer_path, false, false);
        return Err(e);
    }

    // 2. `ActivateLayer` mounts the sandbox via the layer filter so
    //    subsequent layer operations can address it.
    if let Err(e) = wclayer::activate_layer(layer_path) {
        cleanup_best_effort(layer_path, false, false);
        return Err(e);
    }

    // 3. `PrepareLayer` overlays the parent chain on top of the scratch so
    //    the merged view is materialised for the consumer.
    if let Err(e) = wclayer::prepare_layer(layer_path, parent_chain) {
        cleanup_best_effort(layer_path, false, true);
        return Err(e);
    }

    // 4. `GetLayerMountPath` returns the host volume path the container
    //    document will reference.
    let mount_path = match wclayer::get_layer_mount_path(layer_path) {
        Ok(p) => p,
        Err(e) => {
            cleanup_best_effort(layer_path, true, true);
            return Err(e);
        }
    };

    Ok(WritableLayer {
        layer_path: layer_path.to_path_buf(),
        mount_path,
        prepared: true,
        activated: true,
        needs_destroy: true,
    })
}

/// Best-effort teardown used when `create` fails partway through. Never
/// panics; emits warnings on failure.
///
/// `prepared` and `activated` must be `true` only if the matching
/// `PrepareLayer` / `ActivateLayer` call previously succeeded for
/// `layer_path` and has not yet been undone.
fn cleanup_best_effort(layer_path: &Path, prepared: bool, activated: bool) {
    if prepared {
        if let Err(e) = wclayer::unprepare_layer(layer_path) {
            tracing::warn!(
                layer = %layer_path.display(),
                error = %e,
                "unprepare_layer failed during rollback",
            );
        }
    }
    if activated {
        if let Err(e) = wclayer::deactivate_layer(layer_path) {
            tracing::warn!(
                layer = %layer_path.display(),
                error = %e,
                "deactivate_layer failed during rollback",
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
