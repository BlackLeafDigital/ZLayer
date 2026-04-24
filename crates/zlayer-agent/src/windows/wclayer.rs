//! Safe Rust wrappers for HCS layer-storage operations.
//!
//! HCS exposes a set of synchronous functions in `computestorage.dll` for
//! materializing container image layers on disk, creating scratch (writable)
//! sandbox layers, and attaching/detaching the WCIFS filter driver to present
//! a layered root filesystem to a container. This module wraps them with
//! Rust-friendly paths, serialized `LayerData` JSON, and typed errors.
//!
//! All of these operations are synchronous — unlike the asynchronous
//! compute-system lifecycle APIs in [`zlayer_hcs`] — so they live in this
//! module rather than in the HCS crate.
//!
//! The higher-level orchestration (OCI tar → wclayer directory layout, parent
//! chain management, VHD open/close for scratch setup) lives in
//! `windows::unpacker` and `windows::scratch`; this module only provides the
//! 1:1 FFI bindings.

#![cfg(target_os = "windows")]
#![allow(unsafe_code)]

use std::io;
use std::path::Path;

use serde::Serialize;
use windows::core::{HSTRING, PWSTR};
use windows::Win32::Foundation::{LocalFree, HANDLE, HLOCAL};
use windows::Win32::System::HostComputeSystem::{
    HcsAttachLayerStorageFilter, HcsDestroyLayer, HcsDetachLayerStorageFilter, HcsExportLayer,
    HcsFormatWritableLayerVhd, HcsGetLayerVhdMountPath, HcsImportLayer, HcsInitializeWritableLayer,
    HcsSetupBaseOSLayer,
};

use zlayer_hcs::schema::{Layer, SchemaVersion};

// ---------------------------------------------------------------------------
// Parent chain + LayerData serialization
// ---------------------------------------------------------------------------

/// Parent-layer chain passed to HCS layer-storage operations.
///
/// Order is **child-to-parent**: the first element is the immediate parent of
/// the layer being imported, attached, or set up; the last is the base OS
/// layer. An empty chain is valid for base-layer import operations.
#[derive(Debug, Default, Clone)]
pub struct LayerChain(pub Vec<Layer>);

impl LayerChain {
    /// Build a new chain from an owned `Vec<Layer>`.
    #[must_use]
    pub fn new(layers: Vec<Layer>) -> Self {
        Self(layers)
    }

    /// Serialize this chain into the `LayerData` JSON document shape HCS
    /// expects. The document always carries a schema-version tag; the
    /// `Layers` array is omitted when the chain is empty.
    fn to_layer_data_json(&self) -> io::Result<String> {
        // `LayerData` is a schema-v2.1 document with a `SchemaVersion` tag
        // and an optional `Layers` array. HCS tolerates both presentations
        // (empty array or missing key), but the hcsshim reference omits the
        // key entirely when there are no parents.
        #[derive(Serialize)]
        #[serde(rename_all = "PascalCase")]
        struct LayerData<'a> {
            schema_version: SchemaVersion,
            #[serde(skip_serializing_if = "<[Layer]>::is_empty")]
            layers: &'a [Layer],
        }
        let data = LayerData {
            schema_version: SchemaVersion::default(),
            layers: &self.0,
        };
        serde_json::to_string(&data)
            .map_err(|e| io::Error::other(format!("serialize LayerData: {e}")))
    }
}

/// Convert a filesystem [`Path`] into the wide-string form HCS expects.
///
/// Windows HCS APIs take `PCWSTR` for every path. `HSTRING::from(OsStr)` does
/// the WTF-8 → UTF-16 conversion for us and the resulting `HSTRING` implements
/// `Param<PCWSTR>`, so it can be passed to the FFI call directly.
fn path_to_hstring(path: &Path) -> HSTRING {
    HSTRING::from(path.as_os_str())
}

// ---------------------------------------------------------------------------
// Layer-storage wrappers (1:1 with HCS functions)
// ---------------------------------------------------------------------------

/// Import a previously-staged layer directory at `layer_path` with the given
/// parent chain. The `source_folder` contains the unpacked OCI tar converted
/// into the wclayer folder layout (`Files/`, `Hives/`, `tombstones.txt`, ...);
/// on success, HCS has materialized the layer into `layer_path`.
///
/// Callers must hold `SeBackupPrivilege` + `SeRestorePrivilege` on the current
/// process token — see [`crate::windows::layer::enable_backup_restore_privileges`].
///
/// # Errors
///
/// Returns an [`io::Error`] if HCS returns a non-success HRESULT.
pub fn import_layer(
    layer_path: &Path,
    source_folder: &Path,
    parent_chain: &LayerChain,
) -> io::Result<()> {
    let layer_data = parent_chain.to_layer_data_json()?;
    let lp = path_to_hstring(layer_path);
    let sf = path_to_hstring(source_folder);
    let ld = HSTRING::from(layer_data);
    // SAFETY: All three arguments are live `HSTRING`s that outlive the call.
    unsafe {
        HcsImportLayer(&lp, &sf, &ld)
            .map_err(|e| io::Error::other(format!("HcsImportLayer: {e}")))?;
    }
    Ok(())
}

/// Export a materialized layer at `layer_path` back into `export_folder` in
/// the wclayer directory layout. Empty `options` is the common case.
///
/// # Errors
///
/// Returns an [`io::Error`] if HCS returns a non-success HRESULT.
pub fn export_layer(
    layer_path: &Path,
    export_folder: &Path,
    parent_chain: &LayerChain,
    options_json: &str,
) -> io::Result<()> {
    let layer_data = parent_chain.to_layer_data_json()?;
    let lp = path_to_hstring(layer_path);
    let ef = path_to_hstring(export_folder);
    let ld = HSTRING::from(layer_data);
    let opts = HSTRING::from(options_json);
    // SAFETY: All four arguments are live `HSTRING`s that outlive the call.
    unsafe {
        HcsExportLayer(&lp, &ef, &ld, &opts)
            .map_err(|e| io::Error::other(format!("HcsExportLayer: {e}")))?;
    }
    Ok(())
}

/// Destroy the layer directory at `layer_path`. This tears down the on-disk
/// representation (including any VHD backing store HCS created during
/// import/init) and is the correct cleanup for both read-only and writable
/// layers.
///
/// # Errors
///
/// Returns an [`io::Error`] if HCS returns a non-success HRESULT.
pub fn destroy_layer(layer_path: &Path) -> io::Result<()> {
    let lp = path_to_hstring(layer_path);
    // SAFETY: `lp` is a live `HSTRING` that outlives the call.
    unsafe {
        HcsDestroyLayer(&lp).map_err(|e| io::Error::other(format!("HcsDestroyLayer: {e}")))?;
    }
    Ok(())
}

/// Attach the WCIFS storage filter onto a scratch layer at `layer_path`,
/// using `parent_chain` as the read-only parents. After a successful call,
/// reads from `layer_path` present the merged filesystem view to the host.
///
/// Typical use: after [`initialize_writable_layer`], attach the filter so the
/// compute system can bind-mount `layer_path` as its root filesystem.
///
/// # Errors
///
/// Returns an [`io::Error`] if HCS returns a non-success HRESULT.
pub fn attach_layer_storage_filter(layer_path: &Path, parent_chain: &LayerChain) -> io::Result<()> {
    let layer_data = parent_chain.to_layer_data_json()?;
    let lp = path_to_hstring(layer_path);
    let ld = HSTRING::from(layer_data);
    // SAFETY: Both arguments are live `HSTRING`s that outlive the call.
    unsafe {
        HcsAttachLayerStorageFilter(&lp, &ld)
            .map_err(|e| io::Error::other(format!("HcsAttachLayerStorageFilter: {e}")))?;
    }
    Ok(())
}

/// Detach the WCIFS storage filter from a scratch layer previously attached
/// via [`attach_layer_storage_filter`]. Must be called before
/// [`destroy_layer`] on writable layers.
///
/// # Errors
///
/// Returns an [`io::Error`] if HCS returns a non-success HRESULT.
pub fn detach_layer_storage_filter(layer_path: &Path) -> io::Result<()> {
    let lp = path_to_hstring(layer_path);
    // SAFETY: `lp` is a live `HSTRING` that outlives the call.
    unsafe {
        HcsDetachLayerStorageFilter(&lp)
            .map_err(|e| io::Error::other(format!("HcsDetachLayerStorageFilter: {e}")))?;
    }
    Ok(())
}

/// Initialize a writable (scratch) sandbox layer at `writable_layer_path`.
/// HCS creates a sparse `sandbox.vhdx` inside the directory and, together
/// with `parent_chain`, prepares it for use as the container's scratch space.
///
/// `options_json` is an optional JSON document with sandbox-specific knobs
/// (e.g. `{"SandboxSize": 21474836480}`). Pass `""` for HCS defaults.
///
/// # Errors
///
/// Returns an [`io::Error`] if HCS returns a non-success HRESULT.
pub fn initialize_writable_layer(
    writable_layer_path: &Path,
    parent_chain: &LayerChain,
    options_json: &str,
) -> io::Result<()> {
    let layer_data = parent_chain.to_layer_data_json()?;
    let wp = path_to_hstring(writable_layer_path);
    let ld = HSTRING::from(layer_data);
    let opts = HSTRING::from(options_json);
    // SAFETY: All three arguments are live `HSTRING`s that outlive the call.
    unsafe {
        HcsInitializeWritableLayer(&wp, &ld, &opts)
            .map_err(|e| io::Error::other(format!("HcsInitializeWritableLayer: {e}")))?;
    }
    Ok(())
}

/// Format an open VHDX file handle as a scratch (writable) layer volume.
/// The handle must have been opened with read/write access and refer to a
/// pre-existing, correctly-sized VHDX file.
///
/// # Errors
///
/// Returns an [`io::Error`] if HCS returns a non-success HRESULT.
pub fn format_writable_layer_vhd(vhd_handle: HANDLE) -> io::Result<()> {
    // SAFETY: The caller owns `vhd_handle` and guarantees it is open with
    // read/write access for the duration of this call.
    unsafe {
        HcsFormatWritableLayerVhd(vhd_handle)
            .map_err(|e| io::Error::other(format!("HcsFormatWritableLayerVhd: {e}")))?;
    }
    Ok(())
}

/// Populate the base-OS VHD backing `layer_path` using the NT filesystem
/// contents already staged there. Called once per base image to transform a
/// "raw" imported base layer into one HCS can chain.
///
/// `options_json` is typically `""` or `"{\"Kind\":\"BaseOSLayer\"}"`
/// depending on host version; pass an empty string for defaults.
///
/// # Errors
///
/// Returns an [`io::Error`] if HCS returns a non-success HRESULT.
pub fn setup_base_os_layer(
    layer_path: &Path,
    vhd_handle: HANDLE,
    options_json: &str,
) -> io::Result<()> {
    let lp = path_to_hstring(layer_path);
    let opts = HSTRING::from(options_json);
    // SAFETY: `lp` and `opts` are live `HSTRING`s; `vhd_handle` is owned by
    // the caller and valid for the duration of the call.
    unsafe {
        HcsSetupBaseOSLayer(&lp, vhd_handle, &opts)
            .map_err(|e| io::Error::other(format!("HcsSetupBaseOSLayer: {e}")))?;
    }
    Ok(())
}

/// Retrieve the host mount path of a layer's VHD, given an open VHD handle.
///
/// HCS allocates the returned wide string with `LocalAlloc`; this wrapper
/// copies it into an owned `String` and frees the original with `LocalFree`
/// before returning, so callers never deal with the raw buffer.
///
/// # Errors
///
/// Returns an [`io::Error`] if HCS returns a non-success HRESULT or if the
/// returned wide string is not valid UTF-16.
pub fn get_layer_vhd_mount_path(vhd_handle: HANDLE) -> io::Result<String> {
    // The windows-rs 0.62 wrapper already threads the `*mut PWSTR` out-param
    // for us and returns `Result<PWSTR>`. The underlying HCS contract is that
    // the returned buffer was allocated with `LocalAlloc` and must be freed
    // by the caller via `LocalFree` (matches hcsshim's Go binding).
    //
    // SAFETY: The caller owns `vhd_handle` and guarantees it is valid for the
    // duration of the call.
    let pwstr: PWSTR = unsafe {
        HcsGetLayerVhdMountPath(vhd_handle)
            .map_err(|e| io::Error::other(format!("HcsGetLayerVhdMountPath: {e}")))?
    };

    if pwstr.is_null() {
        return Ok(String::new());
    }

    // Decode then free. We intentionally decode before LocalFree so that a
    // UTF-16 error path still releases the buffer.
    // SAFETY: `pwstr` is non-null and points at a HCS-allocated, null-
    // terminated UTF-16 buffer that remains valid until we `LocalFree` it.
    let decoded = unsafe { pwstr.to_string() };

    // SAFETY: `pwstr.0` was allocated by HCS via `LocalAlloc` (per
    // `HcsGetLayerVhdMountPath` docs), so `LocalFree` on the same pointer
    // correctly releases it.
    unsafe {
        let _ = LocalFree(Some(HLOCAL(pwstr.0.cast())));
    }

    decoded.map_err(|e| io::Error::other(format!("UTF-16 mount path decode: {e}")))
}

// ---------------------------------------------------------------------------
// Tests (no HCS calls)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_chain_serializes_without_layers_key() {
        let chain = LayerChain::default();
        let json = chain.to_layer_data_json().expect("serialize");
        // Must always carry the schema tag, and must omit the empty array.
        assert!(json.contains("\"SchemaVersion\""));
        assert!(!json.contains("\"Layers\""));
    }

    #[test]
    fn non_empty_chain_serializes_parents_in_order() {
        let chain = LayerChain::new(vec![
            Layer {
                id: "1111".into(),
                path: r"C:\layers\a".into(),
            },
            Layer {
                id: "2222".into(),
                path: r"C:\layers\b".into(),
            },
        ]);
        let json = chain.to_layer_data_json().expect("serialize");
        assert!(json.contains("\"Layers\""));
        // Child-to-parent ordering preserved.
        let a = json.find("1111").expect("first id present");
        let b = json.find("2222").expect("second id present");
        assert!(a < b, "child layer must appear before parent");
        // PascalCase field names (matches HCS expectations).
        assert!(json.contains("\"Id\""));
        assert!(json.contains("\"Path\""));
    }
}
