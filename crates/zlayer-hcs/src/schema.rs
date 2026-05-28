//! HCS document schema types (subset of `hcsshim/internal/hcs/schema2`).
//!
//! These `serde` types model the JSON documents HCS accepts on creation and
//! emits on `GetProperties` calls. Only the fields `ZLayer` uses today are
//! modeled — extend as new features are added. All structs default to
//! schema v2.1 which is supported on every Windows host since Server 2019.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Schema version
// ---------------------------------------------------------------------------

/// HCS schema version tag embedded in every compute-system document.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub struct SchemaVersion {
    /// Major schema version.
    pub major: u32,
    /// Minor schema version.
    pub minor: u32,
}

impl Default for SchemaVersion {
    /// Target schema 2.1 — supported on all Windows hosts since Server 2019.
    fn default() -> Self {
        Self { major: 2, minor: 1 }
    }
}

// ---------------------------------------------------------------------------
// Top-level compute-system document
// ---------------------------------------------------------------------------

/// Top-level compute-system document passed to `HcsCreateComputeSystem`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct ComputeSystem {
    /// Caller-supplied owner tag (typically the orchestrator name).
    pub owner: String,
    /// Schema version this document targets.
    pub schema_version: SchemaVersion,
    /// Id of the hosting system when attaching a container to an existing
    /// utility VM — empty for standalone systems.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub hosting_system_id: String,
    /// Container body when this document describes a process-isolated or
    /// utility-VM-hosted container.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container: Option<Container>,
    /// Virtual-machine body when this document describes a utility VM for
    /// Hyper-V-isolated workloads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub virtual_machine: Option<VirtualMachine>,
    /// When true, HCS terminates the compute system once the last handle
    /// closes. Containerd's `containerd-shim-runhcs-v1` sets this to `true`
    /// on every container it creates — verified May 2026 via ETW capture of
    /// `Microsoft-Windows-Hyper-V-Compute` provider during a `ctr run`.
    /// Omitting it was an over-correction in an earlier iteration; HCS
    /// `Construct` accepts the doc with or without it, but omitting it
    /// leaves orphaned systems if the agent crashes mid-run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub should_terminate_on_last_handle_closed: Option<bool>,
}

// ---------------------------------------------------------------------------
// Container
// ---------------------------------------------------------------------------

/// Container body within a `ComputeSystem`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct Container {
    /// Guest-OS-level configuration. Holds the container's `NetBIOS` hostname.
    /// HCS requires this field to be present (with a non-empty `HostName`)
    /// when `Networking.Namespace` is set, otherwise `HcsCreateComputeSystem`
    /// rejects the doc with `E_INVALIDARG` (`0x80070057`) at
    /// `OperationFailure.Detail="Construct"`. Matches hcsshim
    /// `internal/hcs/schema2/container.go` (`GuestOs *GuestOs`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guest_os: Option<GuestOs>,
    /// Layered storage configuration (parent layers + scratch path).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage: Option<Storage>,
    /// HCN-namespace attachment and DNS options.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub networking: Option<ContainerNetworking>,
    /// Host-to-container bind mounts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mapped_directories: Vec<MappedDirectory>,
    /// Host named pipes projected into the container.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mapped_pipes: Vec<MappedPipe>,
    /// CPU resource constraints.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub processor: Option<ContainerProcessor>,
    /// Memory resource constraints.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<ContainerMemory>,
}

/// Guest-OS configuration nested under [`Container`]. Currently only carries
/// the container's `NetBIOS` hostname; further fields (e.g. `BootType` for OS
/// containers) may be added as new isolation modes land. Matches hcsshim's
/// `internal/hcs/schema2/guest_os.go` (`type GuestOs struct { HostName string }`).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct GuestOs {
    /// `NetBIOS`-format hostname observed from inside the container. Must be
    /// 1..=15 ASCII alphanumeric-or-hyphen characters, starting with a letter,
    /// no underscores, no dots. Longer values are silently truncated by some
    /// HCS builds and rejected by others — callers should pre-truncate to 15.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_name: Option<String>,
}

/// Layered storage document for a container.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct Storage {
    /// Parent layers, ordered deepest-first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub layers: Vec<Layer>,
    /// Scratch-directory path for the writable layer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// A single read-only parent layer entry.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct Layer {
    /// GUID of the layer.
    pub id: String,
    /// Absolute path to the layer's backing storage.
    pub path: String,
}

/// Container networking attachment.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct ContainerNetworking {
    /// Whether to allow DNS lookups for unqualified names.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_unqualified_dns_query: Option<bool>,
    /// DNS search-suffix list.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dns_search_list: Vec<String>,
    /// HCN namespace id the container is attached to. HCS's
    /// `Container.Networking.Namespace` is a **single** GUID string
    /// (`hcsshim/internal/hcs/schema2/networking.go`: `Namespace string`), not
    /// an array — serializing an array yields a `0xC037010D Invalid JSON
    /// document` rejection from `HcsCreateComputeSystem`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    /// Name of another container whose network namespace is shared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_shared_container_name: Option<String>,
}

/// Host-directory-to-container bind mount.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct MappedDirectory {
    /// Host path being mounted in.
    pub host_path: String,
    /// Path inside the container the host path is mounted at.
    pub container_path: String,
    /// When true, the mount is presented read-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_only: Option<bool>,
}

/// Host named-pipe projected into the container.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct MappedPipe {
    /// Host path of the pipe (for example `\\.\pipe\foo`).
    pub host_path: String,
    /// Pipe name as seen from inside the container.
    pub container_pipe_name: String,
}

/// Container CPU constraints.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct ContainerProcessor {
    /// Number of virtual processors exposed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u32>,
    /// CPU maximum in hundredths of a percent (1 to 10,000).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maximum: Option<u32>,
    /// Relative CPU weight (0 to 10,000).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight: Option<u32>,
}

/// Container memory constraints.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct ContainerMemory {
    /// Memory limit in MiB.
    #[serde(rename = "SizeInMB", default, skip_serializing_if = "Option::is_none")]
    pub size_in_mb: Option<u64>,
}

// ---------------------------------------------------------------------------
// VirtualMachine (Hyper-V isolated)
// ---------------------------------------------------------------------------

/// Virtual-machine body for a Hyper-V-isolated compute system.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct VirtualMachine {
    /// Chipset / firmware configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chipset: Option<Chipset>,
    /// VM compute topology (vCPUs, memory).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compute_topology: Option<Topology>,
    /// Attached devices (SCSI, `VirtualSMB`, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub devices: Option<Devices>,
    /// Guest-state (VHDX) configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guest_state: Option<GuestState>,
    /// Optional path where HCS should persist runtime state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_state_file_path: Option<String>,
}

/// Chipset / firmware block of a virtual machine.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct Chipset {
    /// UEFI-firmware configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uefi: Option<Uefi>,
}

/// UEFI firmware configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct Uefi {
    /// Boot target selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boot_this: Option<UefiBootEntry>,
}

/// Single UEFI boot entry.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct UefiBootEntry {
    /// Device class to boot from, for example `VmbFs` or `ScsiDrive`.
    pub device_type: String,
    /// Device path or image name, when applicable.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub device_path: String,
    /// Disk ordinal on the parent SCSI controller, when booting from SCSI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_number: Option<u32>,
}

/// Compute-topology block (vCPU + memory).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct Topology {
    /// Memory sizing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<TopologyMemory>,
    /// Processor sizing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub processor: Option<TopologyProcessor>,
}

/// VM memory sizing.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct TopologyMemory {
    /// RAM assigned to the VM, in MiB.
    #[serde(rename = "SizeInMB")]
    pub size_in_mb: u64,
}

/// VM processor sizing.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct TopologyProcessor {
    /// Number of vCPUs.
    pub count: u32,
}

/// VM device attachments.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct Devices {
    /// SCSI controllers keyed by their controller id (string-encoded index).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub scsi: BTreeMap<String, ScsiController>,
    /// `VirtualSMB` shares keyed by share name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub virtual_smb: BTreeMap<String, VirtualSmbShare>,
    /// GPU-PV (paravirtualized GPU) device assignment for Hyper-V-isolated
    /// containers. Populated only when the workload requests a GPU; otherwise
    /// omitted so HCS does not attach any host adapter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu: Option<GpuAssignment>,
}

/// GPU-PV assignment block attached under [`Devices::gpu`].
///
/// Mirrors the `GpuAssignment` schema HCS accepts on a `VirtualMachine`
/// document. The valid `assignment_mode` values are:
///
/// - [`GpuAssignmentMode::Default`] — HCS picks the host's default adapter
///   set (typically the discrete GPU, if any). `assignment_request` should
///   be empty in this mode.
/// - [`GpuAssignmentMode::List`] — explicit list of host adapter LUIDs in
///   `assignment_request`; HCS attaches only those adapters.
/// - [`GpuAssignmentMode::Disabled`] — no GPU is attached. Equivalent to
///   omitting the block, but explicit.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub struct GpuAssignment {
    /// Which GPUs to attach: `Default`, `List`, or `Disabled`.
    pub assignment_mode: GpuAssignmentMode,
    /// When `assignment_mode == List`, the host adapter LUIDs to attach.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assignment_request: Vec<GpuAssignmentRequest>,
    /// Allow vendor-extensions in the guest (vGPU paravirtualization).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_vendor_extension: Option<bool>,
}

/// Selection mode for [`GpuAssignment::assignment_mode`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum GpuAssignmentMode {
    /// Let HCS pick the host's default adapter set.
    Default,
    /// Use the LUIDs in [`GpuAssignment::assignment_request`].
    List,
    /// Attach no GPU.
    Disabled,
}

/// One host adapter LUID assigned to the VM.
///
/// LUIDs come from `IDXGIAdapter::GetDesc().AdapterLuid` on the host; the
/// `HighPart` (`u32`) and `LowPart` (`i32`) fields preserve the exact sign /
/// width Microsoft's `LUID` struct uses so the round-trip back to a host
/// handle is bit-exact.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub struct GpuAssignmentRequest {
    /// LUID as a `0x<hi>:0x<lo>` hex string per the HCS schema. HCS keys
    /// adapters by this string in its event payloads.
    pub virtual_machine_id_string: String,
    /// Adapter LUID high part.
    pub adapter_luid_high_part: u32,
    /// Adapter LUID low part.
    pub adapter_luid_low_part: i32,
}

/// A SCSI controller with its attachment map.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct ScsiController {
    /// SCSI attachments keyed by LUN (string-encoded index).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attachments: BTreeMap<String, ScsiAttachment>,
}

/// A single SCSI attachment (virtual disk or ISO).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct ScsiAttachment {
    /// Absolute host path of the VHDX / ISO.
    pub path: String,
    /// Attachment type — `VirtualDisk` or `Iso`.
    #[serde(rename = "Type")]
    pub r#type: String,
    /// When true, the attachment is presented read-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_only: Option<bool>,
}

/// `VirtualSMB` share projected into the VM.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct VirtualSmbShare {
    /// Share name exposed to the guest.
    pub name: String,
    /// Host path backing the share.
    pub path: String,
    /// Raw HCS share flags (see hcsshim for bit definitions).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flags: Option<u32>,
    /// Optional list of files within `path` that are allowed to be accessed
    /// from inside the guest. Empty means all files are accessible.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_files: Vec<String>,
    /// Named option flags for this share. Preferred over the legacy `flags`
    /// bitmask; see [`VirtualSmbShareOptions`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<VirtualSmbShareOptions>,
}

/// Named options for a [`VirtualSmbShare`], replacing the legacy raw `Flags`
/// bitmask. Matches hcsshim's
/// `internal/hcs/schema2/virtual_smb_share_options.go`. Each field defaults to
/// `false` and is omitted from the wire JSON when unset.
///
/// The `"os"` share for a UVM boot dir uses
/// `{ReadOnly:true, ShareRead:true, CacheIo:true, PseudoOplocks:true,
/// TakeBackupPrivilege:true}` — `DefaultVSMBOptions(true)` in hcsshim's
/// `internal/uvm/vsmb.go`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
#[allow(clippy::struct_excessive_bools)] // mirrors hcsshim's wire schema (17 named flags)
pub struct VirtualSmbShareOptions {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub read_only: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub share_read: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cache_io: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub no_oplocks: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub take_backup_privilege: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub use_share_root_identity: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub no_directmap: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub no_locks: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub no_dirnotify: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub vm_shared_memory: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub restrict_file_access: bool,
    #[serde(
        default,
        skip_serializing_if = "std::ops::Not::not",
        rename = "ForceLevelIIOplocks"
    )]
    pub force_level_ii_oplocks: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reparse_base_layer: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pseudo_oplocks: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub non_cache_io: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pseudo_dirnotify: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub single_file_mapping: bool,
}

/// Guest-state block — path to the VHDX holding persisted guest state.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct GuestState {
    /// Absolute path to the guest-state file (`.vmgs`).
    pub guest_state_file_path: String,
}

// ---------------------------------------------------------------------------
// Process parameters
// ---------------------------------------------------------------------------

/// Parameters passed to `HcsCreateProcess`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct ProcessParameters {
    /// Command line to execute inside the container.
    pub command_line: String,
    /// Working directory for the new process.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub working_directory: String,
    /// Environment variables for the new process.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub environment: BTreeMap<String, String>,
    /// When true, HCS emulates a console (PTY-like) for the process.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emulate_console: Option<bool>,
    /// Request an stdin pipe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub create_std_in_pipe: Option<bool>,
    /// Request an stdout pipe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub create_std_out_pipe: Option<bool>,
    /// Request an stderr pipe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub create_std_err_pipe: Option<bool>,
    /// Initial console size (honored when `emulate_console` is true).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub console_size: Option<ConsoleSize>,
    /// User identity the process should run as, when supported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
}

/// Console (PTY) size in character cells.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct ConsoleSize {
    /// Height in rows.
    pub height: u16,
    /// Width in columns.
    pub width: u16,
}

/// Process-status document returned by `HcsGetProcessProperties`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct ProcessStatus {
    /// PID of the process, when still running.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process_id: Option<u32>,
    /// Exit code once the process has terminated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<u32>,
    /// HRESULT of the last wait, when relevant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_wait_result: Option<i32>,
}

// ---------------------------------------------------------------------------
// Statistics property (GetProperties → Statistics)
// ---------------------------------------------------------------------------

/// Top-level `Statistics` property document returned by HCS.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct Statistics {
    /// ISO-8601 timestamp at which the sample was taken.
    #[serde(default)]
    pub timestamp: Option<String>,
    /// ISO-8601 timestamp at which the container first started.
    #[serde(default)]
    pub container_start_time: Option<String>,
    /// Container uptime measured in 100-nanosecond ticks.
    #[serde(default, rename = "Uptime100ns")]
    pub uptime_100ns: u64,
    /// CPU counters.
    #[serde(default)]
    pub processor: Option<ProcessorStats>,
    /// Memory counters.
    #[serde(default)]
    pub memory: Option<MemoryStats>,
    /// Storage I/O counters.
    #[serde(default)]
    pub storage: Option<StorageStats>,
}

/// Processor counters from `Statistics`.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct ProcessorStats {
    /// Total runtime across user + kernel, in 100-ns ticks.
    #[serde(default, rename = "TotalRuntime100ns")]
    pub total_runtime_100ns: u64,
    /// Time spent in user mode, in 100-ns ticks.
    #[serde(default, rename = "RuntimeUser100ns")]
    pub runtime_user_100ns: u64,
    /// Time spent in kernel mode, in 100-ns ticks.
    #[serde(default, rename = "RuntimeKernel100ns")]
    pub runtime_kernel_100ns: u64,
}

/// Memory counters from `Statistics`.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct MemoryStats {
    /// Committed bytes currently in use.
    #[serde(default)]
    pub memory_usage_commit_bytes: u64,
    /// Highest committed-byte watermark observed.
    #[serde(default)]
    pub memory_usage_commit_peak_bytes: u64,
    /// Private working-set bytes.
    #[serde(default)]
    pub memory_usage_private_working_set_bytes: u64,
}

/// Storage I/O counters from `Statistics`.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct StorageStats {
    /// Normalized read-operation count.
    #[serde(default)]
    pub read_count_normalized: u64,
    /// Bytes read.
    #[serde(default)]
    pub read_size_bytes: u64,
    /// Normalized write-operation count.
    #[serde(default)]
    pub write_count_normalized: u64,
    /// Bytes written.
    #[serde(default)]
    pub write_size_bytes: u64,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::{
        ComputeSystem, Container, ContainerMemory, ContainerProcessor, Layer, SchemaVersion,
        Statistics, Storage,
    };

    #[test]
    fn schema_version_default_is_v2_1() {
        let v = SchemaVersion::default();
        assert_eq!(v.major, 2);
        assert_eq!(v.minor, 1);
    }

    #[test]
    fn compute_system_json_round_trip_container() {
        let doc = ComputeSystem {
            owner: "zlayer".to_string(),
            schema_version: SchemaVersion::default(),
            hosting_system_id: String::new(),
            container: Some(Container {
                storage: Some(Storage {
                    layers: vec![Layer {
                        id: "0f2c0c2a-1111-2222-3333-444455556666".to_string(),
                        path: r"C:\ProgramData\zlayer\layers\base".to_string(),
                    }],
                    path: Some(r"C:\ProgramData\zlayer\scratch\abc".to_string()),
                }),
                networking: None,
                mapped_directories: Vec::new(),
                mapped_pipes: Vec::new(),
                guest_os: Some(GuestOs {
                    host_name: Some("test-host".to_string()),
                }),
                processor: Some(ContainerProcessor {
                    count: Some(2),
                    maximum: None,
                    weight: None,
                }),
                memory: Some(ContainerMemory {
                    size_in_mb: Some(1024),
                }),
            }),
            virtual_machine: None,
        };

        let json = serde_json::to_string(&doc).expect("serialize");
        // Sanity-check that we're producing PascalCase keys as HCS expects.
        assert!(json.contains("\"Owner\":\"zlayer\""));
        assert!(json.contains("\"SchemaVersion\":{\"Major\":2,\"Minor\":1}"));
        assert!(json.contains("\"GuestOs\":{\"HostName\":\"test-host\"}"));
        assert!(json.contains("\"SizeInMB\":1024"));

        let back: ComputeSystem = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.owner, "zlayer");
        assert_eq!(back.schema_version, SchemaVersion { major: 2, minor: 1 });
        let container = back.container.expect("container present");
        let storage = container.storage.expect("storage present");
        assert_eq!(storage.layers.len(), 1);
        assert_eq!(storage.layers[0].id, "0f2c0c2a-1111-2222-3333-444455556666");
        assert_eq!(
            storage.path.as_deref(),
            Some(r"C:\ProgramData\zlayer\scratch\abc"),
        );
        assert_eq!(
            container.guest_os.and_then(|g| g.host_name).as_deref(),
            Some("test-host"),
        );
        assert_eq!(container.processor.and_then(|p| p.count), Some(2));
        assert_eq!(container.memory.and_then(|m| m.size_in_mb), Some(1024));
    }

    #[test]
    fn statistics_parses_sample_json() {
        let payload = r#"{"Timestamp":"2026-04-21T12:34:56Z","Uptime100ns":2960000000,"Processor":{"TotalRuntime100ns":1234567,"RuntimeUser100ns":900000,"RuntimeKernel100ns":334567},"Memory":{"MemoryUsageCommitBytes":268435456,"MemoryUsageCommitPeakBytes":314572800,"MemoryUsagePrivateWorkingSetBytes":201326592},"Storage":{"ReadCountNormalized":42,"ReadSizeBytes":1048576,"WriteCountNormalized":13,"WriteSizeBytes":262144}}"#;

        let stats: Statistics = serde_json::from_str(payload).expect("parse statistics");

        assert_eq!(stats.timestamp.as_deref(), Some("2026-04-21T12:34:56Z"));
        assert_eq!(stats.uptime_100ns, 2_960_000_000);

        let cpu = stats.processor.expect("processor");
        assert_eq!(cpu.total_runtime_100ns, 1_234_567);
        assert_eq!(cpu.runtime_user_100ns, 900_000);
        assert_eq!(cpu.runtime_kernel_100ns, 334_567);

        let mem = stats.memory.expect("memory");
        assert_eq!(mem.memory_usage_commit_bytes, 268_435_456);
        assert_eq!(mem.memory_usage_commit_peak_bytes, 314_572_800);
        assert_eq!(mem.memory_usage_private_working_set_bytes, 201_326_592);

        let storage = stats.storage.expect("storage");
        assert_eq!(storage.read_count_normalized, 42);
        assert_eq!(storage.read_size_bytes, 1_048_576);
        assert_eq!(storage.write_count_normalized, 13);
        assert_eq!(storage.write_size_bytes, 262_144);
    }
}
