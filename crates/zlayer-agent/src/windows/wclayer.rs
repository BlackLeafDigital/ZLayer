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
use windows::core::{HSTRING, PCWSTR};
use windows::Win32::System::HostComputeSystem::{HcsDestroyLayer, HcsExportLayer, HcsImportLayer};

use zlayer_hcs::schema::{Layer, SchemaVersion};

// ---------------------------------------------------------------------------
// Shared FFI types for `vmcompute.dll` legacy `wclayer` APIs.
//
// `CreateSandboxLayer`, `ActivateLayer`, `PrepareLayer`, `UnprepareLayer`,
// `DeactivateLayer`, and `GetLayerMountPath` all take a `WC_DRIVER_INFO*` as
// their first argument. `PrepareLayer` and `CreateSandboxLayer` additionally
// take an array of `WC_LAYER_DESCRIPTOR`. These structs and the empty
// home-dir buffer are shared across every wrapper below.
// ---------------------------------------------------------------------------

/// `WC_DRIVER_INFO`. See [`create_sandbox_layer`] for the full ABI write-up
/// (flavour discriminant width, `info_buffer` null-pointer trap, etc.).
#[repr(C)]
struct WcDriverInfo {
    flavour: u32,
    info_buffer: *const u16,
}

/// `WC_LAYER_DESCRIPTOR`: 16-byte GUID + 4-byte flags + 4-byte pad + 8-byte
/// path pointer = 32 bytes on x64. Matches hcsshim's `WC_LAYER_DESCRIPTOR`
/// and the .NET `LayerDescriptor` reference.
#[repr(C)]
struct WcLayerDescriptor {
    layer_id: windows::core::GUID,
    flags: u32,
    _flags_pad: u32,
    path: *const u16,
}

/// Empty null-terminated UTF-16 string the driver-info points at. Static so
/// the pointer is stable for any caller's `WcDriverInfo` literal — matches
/// hcsshim's `var utf16EmptyString uint16` + `&utf16EmptyString`.
static EMPTY_HOME_DIR: [u16; 1] = [0];

/// Build a `WcDriverInfo` configured for the filter-driver flavour (the only
/// one we ever use). The returned struct borrows `&EMPTY_HOME_DIR` which has
/// `'static` lifetime, so the returned value is safe to pass through to FFI.
fn driver_info() -> WcDriverInfo {
    WcDriverInfo {
        flavour: 1,
        info_buffer: EMPTY_HOME_DIR.as_ptr(),
    }
}

/// Convert a parent chain into the parallel `(path_buffers, descriptors)`
/// vectors expected by `CreateSandboxLayer` / `PrepareLayer`.
///
/// `path_buffers` owns the wide-string path data that each descriptor's
/// `path` pointer refers to; it MUST outlive the descriptor slice across the
/// FFI call.
fn parent_chain_to_descriptors(
    parent_chain: &LayerChain,
) -> io::Result<(Vec<Vec<u16>>, Vec<WcLayerDescriptor>)> {
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
                _flags_pad: 0,
                path: buf.as_ptr(),
            })
        })
        .collect::<io::Result<Vec<_>>>()?;
    Ok((path_buffers, descriptors))
}

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

/// Activate a layer at `layer_path` (mount its sandbox.vhdx via the layer
/// filter driver, allowing subsequent `PrepareLayer` + `GetLayerMountPath`).
///
/// Wraps `vmcompute.dll!ActivateLayer`. Matches hcsshim's
/// `internal/wclayer/activatelayer.go::ActivateLayer`. Pair every successful
/// `activate_layer` with a `deactivate_layer` once the consumer is done.
///
/// # Errors
///
/// Returns an [`io::Error`] if `ActivateLayer` returns a non-success HRESULT.
pub fn activate_layer(layer_path: &Path) -> io::Result<()> {
    windows::core::link!(
        "vmcompute.dll" "system" fn ActivateLayer(
            info: *const WcDriverInfo,
            id: PCWSTR,
        ) -> windows::core::HRESULT
    );
    let info = driver_info();
    let lp = HSTRING::from(layer_path.as_os_str());
    // SAFETY: `info` borrows `EMPTY_HOME_DIR` (static); `lp` is a live
    // `HSTRING` whose null-terminated buffer outlives the call.
    let hr = unsafe { ActivateLayer(&info, PCWSTR::from_raw(lp.as_ptr())) };
    hr.ok()
        .map_err(|e| io::Error::other(format!("ActivateLayer: {e}")))?;
    Ok(())
}

/// Deactivate a layer previously activated via [`activate_layer`]. Must be
/// called after `unprepare_layer` and before `destroy_layer`.
///
/// Wraps `vmcompute.dll!DeactivateLayer`. Matches hcsshim's
/// `internal/wclayer/deactivatelayer.go::DeactivateLayer`.
///
/// # Errors
///
/// Returns an [`io::Error`] if `DeactivateLayer` returns a non-success HRESULT.
pub fn deactivate_layer(layer_path: &Path) -> io::Result<()> {
    windows::core::link!(
        "vmcompute.dll" "system" fn DeactivateLayer(
            info: *const WcDriverInfo,
            id: PCWSTR,
        ) -> windows::core::HRESULT
    );
    let info = driver_info();
    let lp = HSTRING::from(layer_path.as_os_str());
    // SAFETY: same as `activate_layer`.
    let hr = unsafe { DeactivateLayer(&info, PCWSTR::from_raw(lp.as_ptr())) };
    hr.ok()
        .map_err(|e| io::Error::other(format!("DeactivateLayer: {e}")))?;
    Ok(())
}

/// Prepare a writable layer at `layer_path` over `parent_chain` so that the
/// host (or a container) can read/write through the layered view. Must be
/// called after [`activate_layer`] and before [`get_layer_mount_path`].
///
/// Wraps `vmcompute.dll!PrepareLayer`. Matches hcsshim's
/// `internal/wclayer/preparelayer.go::PrepareLayer`. Pair every successful
/// `prepare_layer` with an `unprepare_layer` once the consumer is done.
///
/// # Errors
///
/// Returns an [`io::Error`] if `PrepareLayer` returns a non-success HRESULT.
pub fn prepare_layer(layer_path: &Path, parent_chain: &LayerChain) -> io::Result<()> {
    windows::core::link!(
        "vmcompute.dll" "system" fn PrepareLayer(
            info: *const WcDriverInfo,
            id: PCWSTR,
            descriptors: *const WcLayerDescriptor,
            descriptor_count: usize,
        ) -> windows::core::HRESULT
    );
    let info = driver_info();
    let lp = HSTRING::from(layer_path.as_os_str());
    let (_path_buffers, descriptors) = parent_chain_to_descriptors(parent_chain)?;
    // SAFETY: `info` borrows `EMPTY_HOME_DIR` (static); `lp` is a live
    // `HSTRING`; `descriptors` and the wide-string buffers it points into
    // (`_path_buffers`) outlive the call.
    let hr = unsafe {
        PrepareLayer(
            &info,
            PCWSTR::from_raw(lp.as_ptr()),
            descriptors.as_ptr(),
            descriptors.len(),
        )
    };
    hr.ok()
        .map_err(|e| io::Error::other(format!("PrepareLayer: {e}")))?;
    Ok(())
}

/// Tear down the prepared state of a layer at `layer_path`. Pair with
/// [`prepare_layer`].
///
/// Wraps `vmcompute.dll!UnprepareLayer`. Matches hcsshim's
/// `internal/wclayer/unpreparelayer.go::UnprepareLayer`.
///
/// # Errors
///
/// Returns an [`io::Error`] if `UnprepareLayer` returns a non-success HRESULT.
pub fn unprepare_layer(layer_path: &Path) -> io::Result<()> {
    windows::core::link!(
        "vmcompute.dll" "system" fn UnprepareLayer(
            info: *const WcDriverInfo,
            id: PCWSTR,
        ) -> windows::core::HRESULT
    );
    let info = driver_info();
    let lp = HSTRING::from(layer_path.as_os_str());
    // SAFETY: same as `activate_layer`.
    let hr = unsafe { UnprepareLayer(&info, PCWSTR::from_raw(lp.as_ptr())) };
    hr.ok()
        .map_err(|e| io::Error::other(format!("UnprepareLayer: {e}")))?;
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
/// runhcs, and Moby. `CreateSandboxLayer` produces a fully-formatted
/// `sandbox.vhdx` and supporting metadata inside `layer_path` in a single
/// call — there is no separate `Format` step. To make the layer mountable,
/// the caller chains [`activate_layer`] → [`prepare_layer`] →
/// [`get_layer_mount_path`], mirroring hcsshim's
/// `internal/layers/wcow_mount.go::mountProcessIsolatedWCIFSLayers`.
///
/// `parent_chain` is child-to-parent ordered (first entry = immediate parent,
/// last entry = base OS layer).
///
/// # ABI notes
///
/// * `WC_DRIVER_INFO` carries a 4-byte flavour discriminant (.NET reference
///   `int Type; IntPtr Path;`) and an 8-byte pointer to a UTF-16 home-dir
///   string. Both hcsshim and the .NET reference initialise it with
///   `{flavour: 1, info_buffer: &""}` — `FilterDriver` flavour with a
///   pointer to an *empty UTF-16 string*, NOT a NULL pointer. `vmcompute`
///   unconditionally dereferences `info_buffer`; NULL crashes inside
///   `vmcompute.dll` with `STATUS_ACCESS_VIOLATION` at ~offset 0x30e6a.
/// * `WC_LAYER_DESCRIPTOR` is 16-byte GUID + 4-byte `Flags` + 4-byte pad +
///   8-byte path pointer = 32 bytes on x64.
/// * Descriptor count is passed as `usize` to match hcsshim's
///   `internal/wclayer/wclayer.go` Go binding; the underlying C signature
///   is `ULONG count`, and both bindings place a correctly-extended 4-byte
///   value in the stack slot.
///
/// # Errors
///
/// Returns an [`io::Error`] if `CreateSandboxLayer` returns a non-success
/// HRESULT or if the descriptor `id` field cannot be parsed as a GUID.
pub fn create_sandbox_layer(layer_path: &Path, parent_chain: &LayerChain) -> io::Result<()> {
    windows::core::link!(
        "vmcompute.dll" "system" fn CreateSandboxLayer(
            info: *const WcDriverInfo,
            id: PCWSTR,
            parent: usize,
            descriptors: *const WcLayerDescriptor,
            descriptor_count: usize,
        ) -> windows::core::HRESULT
    );

    let info = driver_info();
    let layer_path_w = HSTRING::from(layer_path.as_os_str());
    let (_path_buffers, descriptors) = parent_chain_to_descriptors(parent_chain)?;

    // SAFETY: `info` borrows `EMPTY_HOME_DIR` (static); `layer_path_w` is a
    // live `HSTRING`; `descriptors` and the wide-string buffers it points
    // into (`_path_buffers`) outlive the call. `CreateSandboxLayer` only
    // reads through these pointers and returns an HRESULT.
    let hr = unsafe {
        CreateSandboxLayer(
            &info,
            PCWSTR::from_raw(layer_path_w.as_ptr()),
            0,
            descriptors.as_ptr(),
            descriptors.len(),
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

/// Retrieve the host mount path of a layer at `layer_path`. The layer must
/// already be activated + prepared ([`activate_layer`] + [`prepare_layer`]).
///
/// Wraps `vmcompute.dll!GetLayerMountPath`. Matches hcsshim's
/// `internal/wclayer/getlayermountpath.go::GetLayerMountPath`: a two-call
/// pattern (first call with `buffer=NULL` to learn the required length,
/// second call to fill an allocated buffer of that length).
///
/// Returns an empty string when the layer has no mount path (e.g. an
/// activated read-only layer that has not been prepared).
///
/// # Errors
///
/// Returns an [`io::Error`] if either call returns a non-success HRESULT.
pub fn get_layer_mount_path(layer_path: &Path) -> io::Result<String> {
    windows::core::link!(
        "vmcompute.dll" "system" fn GetLayerMountPath(
            info: *const WcDriverInfo,
            id: PCWSTR,
            length: *mut usize,
            buffer: *mut u16,
        ) -> windows::core::HRESULT
    );
    let info = driver_info();
    let lp = HSTRING::from(layer_path.as_os_str());

    // Pass 1: discover the required wide-char length (HCS writes it through
    // `length`; `buffer=NULL` signals "size query only", same as hcsshim's
    // first `_getLayerMountPath` call).
    let mut length: usize = 0;
    // SAFETY: `info` borrows `EMPTY_HOME_DIR` (static); `lp` is live; `&mut
    // length` is an exclusively-borrowed out-pointer; the buffer is
    // explicitly null which `GetLayerMountPath` accepts for the size query.
    let hr = unsafe {
        GetLayerMountPath(
            &info,
            PCWSTR::from_raw(lp.as_ptr()),
            &mut length,
            std::ptr::null_mut(),
        )
    };
    hr.ok()
        .map_err(|e| io::Error::other(format!("GetLayerMountPath(size query): {e}")))?;

    if length == 0 {
        return Ok(String::new());
    }

    // Pass 2: allocate `length` u16s and fill them. `length` already includes
    // the null terminator.
    let mut buf = vec![0u16; length];
    // SAFETY: same as pass 1 plus `buf.as_mut_ptr()` is a valid writable
    // pointer to a `length`-element u16 buffer that outlives the call.
    let hr = unsafe {
        GetLayerMountPath(
            &info,
            PCWSTR::from_raw(lp.as_ptr()),
            &mut length,
            buf.as_mut_ptr(),
        )
    };
    hr.ok()
        .map_err(|e| io::Error::other(format!("GetLayerMountPath(fill): {e}")))?;

    // Decode up to the first null (HCS guarantees one within `length`).
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16(&buf[..end])
        .map_err(|e| io::Error::other(format!("UTF-16 mount path decode: {e}")))
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
