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
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use serde::Serialize;
use windows::core::{HSTRING, PCWSTR, PWSTR};
use windows::Win32::Foundation::{LocalFree, HANDLE, HLOCAL};
use windows::Win32::System::HostComputeSystem::{
    HcsAttachLayerStorageFilter, HcsDestroyLayer, HcsDetachLayerStorageFilter, HcsExportLayer,
    HcsFormatWritableLayerVhd, HcsGetLayerVhdMountPath, HcsImportLayer, HcsSetupBaseOSLayer,
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
/// Typical use: after [`create_sandbox_layer`], attach the filter so the
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

/// Materialize the read-only base OS layer at `layer_path` after a successful
/// [`import_layer`] of a base layer (i.e. one imported with an empty
/// parent chain). This is the rough equivalent of hcsshim's
/// `wclayer.ProcessBaseLayer` — it translates the staged `Hives/*` registry
/// hive exports into the live `Files\Windows\System32\config\*` files that
/// HCS expects when a child layer chain-walks back to this base.
///
/// Without this call, importing a child layer that lists this base in its
/// parent chain can fail with `0x80070002` ("file not found") because the
/// derived `config\` materializations don't exist yet and HCS's
/// `NtQueryDirectoryFile` walk hits an absent directory.
///
/// Backing FFI: `vmcompute.dll!ProcessBaseImage(path: *uint16) -> HRESULT`
/// (matches `hcsshim/internal/wclayer/zsyscall_windows.go`). This symbol is
/// not exposed by `windows::Win32::System::HostComputeSystem` in
/// `windows-rs 0.62`, so we declare the link inline.
///
/// # Errors
///
/// Returns an [`io::Error`] if the syscall returns a non-success HRESULT.
pub fn process_base_layer(layer_path: &Path) -> io::Result<()> {
    let lp = path_to_hstring(layer_path);
    // Link directly to vmcompute.dll's `ProcessBaseImage`. Single PCWSTR in,
    // HRESULT out.
    windows::core::link!(
        "vmcompute.dll" "system" fn ProcessBaseImage(path: PCWSTR) -> windows::core::HRESULT
    );
    // SAFETY: `lp` is a live `HSTRING` whose backing buffer is null-terminated
    // UTF-16 (per `HSTRING`'s `Deref<Target = [u16]>` invariant) and outlives
    // the call. `ProcessBaseImage` only reads the wide-string path argument
    // and returns an HRESULT; no out-pointers or shared resources.
    let hr = unsafe { ProcessBaseImage(PCWSTR::from_raw(lp.as_ptr())) };
    hr.ok()
        .map_err(|e| io::Error::other(format!("ProcessBaseImage: {e}")))?;
    Ok(())
}

/// Create a fresh scratch (writable) layer on disk via
/// `vmcompute.dll!CreateSandboxLayer`.
///
/// This is the canonical scratch-layer-creation path for Windows containers:
/// matches `hcsshim/internal/wclayer/createscratchlayer.go::CreateScratchLayer`,
/// which is the production code path used by hcsshim, containerd-shim-runhcs,
/// runhcs, and Moby. Allocates a sparse `sandbox.vhdx` and supporting metadata
/// inside `layer_path`; the caller then drives `HcsFormatWritableLayerVhd`,
/// optional `HcsSetupBaseOSLayer`, `HcsGetLayerVhdMountPath`, and
/// `HcsAttachLayerStorageFilter` to complete the writable layer.
///
/// `parent_chain` is child-to-parent ordered (first entry = immediate parent,
/// last entry = base OS layer).
///
/// # Errors
///
/// Returns an [`io::Error`] if `CreateSandboxLayer` returns a non-success
/// HRESULT or if the descriptor count overflows `u32`.
pub fn create_sandbox_layer(layer_path: &Path, parent_chain: &LayerChain) -> io::Result<()> {
    // Mirrors `WC_DRIVER_INFO` from the Windows SDK: a USHORT flavour followed
    // by an LPCWSTR info buffer. `#[repr(C)]` keeps the field ordering and lets
    // the compiler insert the natural 6-byte pad to 8-byte align the pointer
    // on x64 (total size = 16 bytes), which matches what vmcompute.dll expects.
    #[repr(C)]
    struct WcDriverInfo {
        flavour: u16,
        info_buffer: *const u16,
    }
    // Mirrors `WC_LAYER_DESCRIPTOR`: 16-byte GUID, 8-byte flags, 8-byte path
    // pointer = 32 bytes total on x64. `#[repr(C)]` preserves layout.
    #[repr(C)]
    struct WcLayerDescriptor {
        layer_id: windows::core::GUID,
        flags: u64,
        path: *const u16,
    }

    // Own the wide-string path buffers locally so the raw pointers stored in
    // each descriptor stay valid across the FFI call.
    let path_buffers: Vec<Vec<u16>> = parent_chain
        .0
        .iter()
        .map(|l| {
            let mut w: Vec<u16> = Path::new(&l.path).as_os_str().encode_wide().collect();
            w.push(0);
            w
        })
        .collect();

    let descriptors: Vec<WcLayerDescriptor> = parent_chain
        .0
        .iter()
        .zip(path_buffers.iter())
        .map(|(l, buf)| {
            let id = parse_guid_str(&l.id)?;
            Ok(WcLayerDescriptor {
                layer_id: id,
                flags: 0,
                path: buf.as_ptr(),
            })
        })
        .collect::<io::Result<Vec<_>>>()?;

    let info = WcDriverInfo {
        flavour: 0,
        info_buffer: std::ptr::null(),
    };
    let layer_path_w = HSTRING::from(layer_path.as_os_str());

    windows::core::link!(
        "vmcompute.dll" "system" fn CreateSandboxLayer(
            info: *const WcDriverInfo,
            id: PCWSTR,
            parent: usize,
            descriptors: *const WcLayerDescriptor,
            descriptor_count: u32,
        ) -> windows::core::HRESULT
    );

    // Descriptor count is well under 2^32 — chains are a handful of layers,
    // never billions — so the `as u32` cast cannot truncate in practice.
    let descriptor_count = u32::try_from(descriptors.len())
        .map_err(|_| io::Error::other("CreateSandboxLayer: descriptor count overflows u32"))?;

    // SAFETY: `info`, `layer_path_w`, `descriptors`, and `path_buffers` all
    // outlive the call. The descriptors point into `path_buffers` which we
    // also hold by value. `CreateSandboxLayer` only reads through these
    // pointers and returns an HRESULT.
    let hr = unsafe {
        CreateSandboxLayer(
            &info,
            PCWSTR::from_raw(layer_path_w.as_ptr()),
            0,
            descriptors.as_ptr(),
            descriptor_count,
        )
    };
    hr.ok()
        .map_err(|e| io::Error::other(format!("CreateSandboxLayer: {e}")))?;
    Ok(())
}

/// Parse a canonical-format GUID string (lowercase 8-4-4-4-12) into a
/// `windows::core::GUID`. Returns an error if `s` is not exactly 36 chars in
/// the expected layout. The `layer_id_for_path` helper produces this format.
fn parse_guid_str(s: &str) -> io::Result<windows::core::GUID> {
    let bytes = s.as_bytes();
    if bytes.len() != 36
        || bytes[8] != b'-'
        || bytes[13] != b'-'
        || bytes[18] != b'-'
        || bytes[23] != b'-'
    {
        return Err(io::Error::other(format!(
            "parse_guid_str: malformed GUID {s:?}"
        )));
    }
    let d1 = u32::from_str_radix(&s[0..8], 16)
        .map_err(|e| io::Error::other(format!("parse_guid_str d1: {e}")))?;
    let d2 = u16::from_str_radix(&s[9..13], 16)
        .map_err(|e| io::Error::other(format!("parse_guid_str d2: {e}")))?;
    let d3 = u16::from_str_radix(&s[14..18], 16)
        .map_err(|e| io::Error::other(format!("parse_guid_str d3: {e}")))?;
    let mut d4 = [0u8; 8];
    d4[0] = u8::from_str_radix(&s[19..21], 16)
        .map_err(|e| io::Error::other(format!("parse_guid_str d4[0]: {e}")))?;
    d4[1] = u8::from_str_radix(&s[21..23], 16)
        .map_err(|e| io::Error::other(format!("parse_guid_str d4[1]: {e}")))?;
    for i in 0..6 {
        let start = 24 + i * 2;
        d4[2 + i] = u8::from_str_radix(&s[start..start + 2], 16)
            .map_err(|e| io::Error::other(format!("parse_guid_str d4[{}]: {e}", 2 + i)))?;
    }
    Ok(windows::core::GUID {
        data1: d1,
        data2: d2,
        data3: d3,
        data4: d4,
    })
}

/// Derive the HCS layer-id for a given on-disk layer path.
///
/// HCS keys parent layers by `NameToGuid(basename(layer_path))`, NOT by any
/// caller-supplied id. The id field in a `LayerData` JSON record MUST be the
/// GUID returned here for HCS to find the parent's backing VHD during
/// `HcsImportLayer` / `CreateSandboxLayer` / `HcsAttachLayerStorageFilter`
/// chain walks. Passing an unrelated UUID yields `ERROR_PATH_NOT_FOUND`
/// (`0x80070003`) the moment HCS tries to resolve a parent.
///
/// Equivalent to hcsshim's `internal/wclayer/layerid.go::LayerID(path)`. Must
/// be called for every layer path that goes into a [`LayerChain`] consumed by
/// HCS.
///
/// Backing FFI: `vmcompute.dll!NameToGuid(name: PCWSTR, guid: *mut GUID)
/// -> HRESULT` — not surfaced by `windows::Win32::System::HostComputeSystem`
/// in `windows-rs 0.62`, so the link is declared inline.
///
/// # Errors
///
/// Returns an [`io::Error`] if the path has no basename or if `NameToGuid`
/// returns a non-success HRESULT.
pub fn layer_id_for_path(layer_path: &Path) -> io::Result<String> {
    let basename = layer_path.file_name().ok_or_else(|| {
        io::Error::other(format!(
            "layer_id_for_path: no basename in {}",
            layer_path.display()
        ))
    })?;
    let name_w = HSTRING::from(basename);
    windows::core::link!(
        "vmcompute.dll" "system" fn NameToGuid(name: PCWSTR, guid: *mut windows::core::GUID) -> windows::core::HRESULT
    );
    let mut guid = windows::core::GUID::zeroed();
    // SAFETY: `name_w` is a live `HSTRING` whose null-terminated UTF-16 buffer
    // outlives the call; `&mut guid` is a valid, exclusively-borrowed out-pointer.
    let hr = unsafe { NameToGuid(PCWSTR::from_raw(name_w.as_ptr()), &mut guid) };
    hr.ok()
        .map_err(|e| io::Error::other(format!("NameToGuid({basename:?}): {e}")))?;
    // Canonical lowercase 8-4-4-4-12 form (matches hcsshim's `LayerID` output).
    Ok(format!(
        "{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        guid.data1,
        guid.data2,
        guid.data3,
        guid.data4[0],
        guid.data4[1],
        guid.data4[2],
        guid.data4[3],
        guid.data4[4],
        guid.data4[5],
        guid.data4[6],
        guid.data4[7],
    ))
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
