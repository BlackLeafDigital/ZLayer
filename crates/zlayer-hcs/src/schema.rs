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
    /// GUID of the layer. Serializes as `Id` (NOT `ID`) to byte-match
    /// hcsshim's `internal/hcs/schema2/layer.go` (`Id string json:"Id,omitempty"`).
    /// The strict inbox GCS unmarshaller (hcsshim #2714) rejects `ID`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    /// Absolute path to the layer's backing storage. hcsshim marks this
    /// `Path,omitempty`; omitted when empty so we never emit a field hcsshim
    /// would have dropped.
    #[serde(default, skip_serializing_if = "String::is_empty")]
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
    ///
    /// TYPE MISMATCH (flagged): hcsshim's
    /// `internal/hcs/schema2/container_networking.go` declares this as a
    /// **comma-joined `string`** (`DnsSearchList string json:"DnsSearchList,omitempty"`),
    /// NOT a JSON array. As a `Vec<String>` this serializes to `["a","b"]`,
    /// which the strict inbox GCS unmarshaller (hcsshim #2714) would reject
    /// with `HCS_E_INVALID_JSON`. It is kept as a `Vec<String>` only because
    /// the two `zlayer-agent` call sites construct it with `Vec::new()` and
    /// changing the type would break a crate this audit must not edit. The
    /// `skip_serializing_if = "Vec::is_empty"` below guarantees the field is
    /// OMITTED entirely whenever it is empty (which is always, today), so no
    /// JSON array is ever emitted on the wire. If a non-empty search list is
    /// ever needed, this MUST be migrated to a comma-joined `String` first.
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
    /// Stop (rather than reboot) the VM when the guest issues a reset. hcsshim
    /// sets this `true` for every utility VM (`create_wcow.go` /
    /// `create_lcow.go`) so a guest-side bugcheck/reset surfaces as a clean
    /// stop instead of an endless reboot loop. Omitted when `false`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stop_on_reset: bool,
    /// Offline registry edits HCS applies to the guest's hives before first
    /// boot. hcsshim populates this for WCOW UVMs (e.g. the `gns`
    /// `EnableCompartmentNamespace` networking workaround). Omitted when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registry_changes: Option<RegistryChanges>,
    /// Chipset / firmware configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chipset: Option<Chipset>,
    /// VM compute topology (vCPUs, memory).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compute_topology: Option<Topology>,
    /// Attached devices (SCSI, `VirtualSMB`, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub devices: Option<Devices>,
    /// Bugcheck / saved-state capture paths. When the guest OS bugchecks, HCS
    /// snapshots VM state to these host-side files. hcsshim sets these whenever
    /// a dump directory is configured (`create_wcow.go`); we use them to
    /// diagnose early-boot guest crashes. Omitted when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub debug_options: Option<DebugOptions>,
    /// Guest-state (VHDX) configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guest_state: Option<GuestState>,
    /// Optional path where HCS should persist runtime state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_state_file_path: Option<String>,
}

/// Offline guest-registry edits applied before first boot. Mirrors hcsshim
/// hcsschema `RegistryChanges` (`internal/hcs/schema2/registry_changes.go`).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct RegistryChanges {
    /// Registry values to add (or overwrite) in the guest hives.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub add_values: Vec<RegistryValue>,
}

/// A single registry value to write. Mirrors hcsshim hcsschema `RegistryValue`.
/// Exactly one of `string_value` / `binary_value` / `d_word_value` should be
/// set depending on `r#type`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct RegistryValue {
    /// The hive + subkey the value lives under.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<RegistryKey>,
    /// Value name.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// Value type — `DWord`, `String`, etc.
    #[serde(rename = "Type", default, skip_serializing_if = "Option::is_none")]
    pub r#type: Option<RegistryValueType>,
    /// `REG_SZ` / `REG_EXPAND_SZ` payload. For `REG_EXPAND_SZ`, embed
    /// environment variable references like `%SystemRoot%` literally.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub string_value: String,
    /// `REG_BINARY` payload, base64-encoded per HCS schema.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub binary_value: String,
    /// `REG_DWORD` payload. Hcsshim's schema types this as `int32`.
    #[serde(
        rename = "DWordValue",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub d_word_value: Option<i32>,
}

/// A registry key location (hive + subkey path). Mirrors hcsshim
/// hcsschema `RegistryKey`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct RegistryKey {
    /// Which guest hive the subkey lives in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hive: Option<RegistryHive>,
    /// Subkey path relative to the hive root.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
}

/// Guest registry hive. Mirrors hcsshim hcsschema `RegistryHive`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RegistryHive {
    /// `HKLM\System`.
    System,
    /// `HKLM\Software`.
    Software,
    /// `HKLM\Security`.
    Security,
    /// `HKLM\Sam`.
    Sam,
}

/// Guest registry value type. Mirrors hcsshim hcsschema `RegistryValueType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RegistryValueType {
    /// `REG_NONE`.
    None,
    /// `REG_SZ`.
    String,
    /// `REG_EXPAND_SZ`.
    ExpandedString,
    /// `REG_MULTI_SZ`.
    MultiString,
    /// `REG_BINARY`.
    Binary,
    /// `REG_DWORD`.
    #[serde(rename = "DWord")]
    DWord,
    /// `REG_QWORD`.
    #[serde(rename = "QWord")]
    QWord,
}

/// Chipset / firmware block of a virtual machine.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct Chipset {
    /// UEFI-firmware configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uefi: Option<Uefi>,
}

/// UEFI firmware configuration. Mirrors hcsshim
/// `internal/hcs/schema2/uefi.go`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct Uefi {
    /// Tells the firmware to expose a UEFI-debugger transport. Distinct
    /// from Windows kernel debug (that's a BCD setting). Wire field name
    /// `EnableDebugger`, `omitempty` Go-side — we drop when false.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub enable_debugger: bool,
    /// One of the hcsshim Secure Boot template tokens (e.g. `"Skip"`,
    /// `"MicrosoftWindows"`). Empty means "leave HCS default". Required
    /// `"Skip"` value when the BCD has been patched to enable Windows
    /// kernel debug — debug-mode kernels fail Secure Boot.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub apply_secure_boot_template: String,
    /// Specific Secure Boot template GUID, when overriding the default.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub secure_boot_template_id: String,
    /// Boot target selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boot_this: Option<UefiBootEntry>,
    /// Redirect firmware POST output to a host-visible channel. Values
    /// observed in hcsshim: `"Default"` (no redirect), `"ComPort1"` /
    /// `"ComPort2"` (route to a `Devices.ComPorts[N]` named pipe). Setting
    /// `"ComPort1"` paired with `Devices.ComPorts["0"].NamedPipe` gives
    /// the host a readable stream of UEFI boot diagnostics without any
    /// BCD patching of the guest VHDX.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub console: String,
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
    /// Enable memory overcommit. Hcsshim sets this on every WCOW UVM
    /// (`internal/uvm/create_wcow.go` `prepareCommonConfigDoc`). Without it
    /// Hyper-V's memory manager may refuse the reservation or pick a mode
    /// that the in-guest GCS init path doesn't tolerate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_overcommit: Option<bool>,
    /// Hint to enable dynamic memory hot-add. Paired with `AllowOvercommit`
    /// in hcsshim's reference doc.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enable_hot_hint: Option<bool>,
}

/// VM processor sizing.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct TopologyProcessor {
    /// Number of vCPUs.
    pub count: u32,
}

/// Hyper-V virtual serial port → host-side named pipe. Mirrors hcsshim
/// `internal/hcs/schema2/com_port.go`.
///
/// Used to capture early-boot UEFI / firmware / (with BCD patch) kernel
/// debug output that has no other channel — the in-guest GCS doesn't
/// expose this and the standard event log is post-bugcheck-only.
/// `OptimizeForDebugger=true` flips the emulated UART into KD-friendly
/// framing (low latency, no buffering).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct ComPort {
    /// Host named-pipe path the COM port is bridged to (e.g.
    /// `\\.\pipe\zlayer-uvm-<guid>-com1`). Empty means "disconnected"
    /// per hcsshim's docstring.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub named_pipe: String,
    /// When true, emulate the UART in low-latency / no-buffering mode
    /// suitable for windbg attach. Defaults false.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub optimize_for_debugger: bool,
}

/// VM device attachments.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct Devices {
    /// Virtual serial ports keyed by port index ("0" = COM1, "1" = COM2).
    /// Empty when no COM redirect is wanted.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub com_ports: BTreeMap<String, ComPort>,
    /// SCSI controllers keyed by their controller id (string-encoded index).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub scsi: BTreeMap<String, ScsiController>,
    /// `VirtualSMB` device block. Wraps the list of [`VirtualSmbShare`]
    /// entries; serializes as `{"Shares": [...]}` on the wire.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub virtual_smb: Option<VirtualSmb>,
    /// GPU-PV (paravirtualized GPU) device assignment for Hyper-V-isolated
    /// containers. Populated only when the workload requests a GPU; otherwise
    /// omitted so HCS does not attach any host adapter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu: Option<GpuAssignment>,
    /// `HvSocket` device configuration. Required for the in-guest GCS to
    /// accept the host's hvsock connection (the `DefaultBindSecurityDescriptor`
    /// authorizes SYSTEM/admin binds). Omitted when not set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hv_socket: Option<HvSocket2>,
    /// Guest crash-dump reporting. When set, HCS writes a Windows kernel dump
    /// to the configured host path on a guest bugcheck (subject to the guest
    /// reaching crashdump init). Mirrors hcsshim
    /// `Devices.GuestCrashReporting`. Omitted when not set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guest_crash_reporting: Option<GuestCrashReporting>,
}

/// Bugcheck / saved-state capture options for a `VirtualMachine`. Mirrors
/// hcsshim hcsschema `DebugOptions` (`internal/hcs/schema2/debug_options.go`).
///
/// `BugcheckNoCrashdumpSavedStateFileName` is the load-bearing one for
/// early-boot crashes: when the guest bugchecks before crashdump
/// infrastructure is ready, HCS can still snapshot raw VM state to this file.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct DebugOptions {
    /// Host path for the saved-state (`.vmrs`) file written when the guest
    /// crashes and a crashdump WAS produced.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub bugcheck_saved_state_file_name: String,
    /// Host path for the saved-state file written when the guest crashes but
    /// could not produce a crashdump (typical of early-boot failures).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub bugcheck_no_crashdump_saved_state_file_name: String,
}

/// Guest crash-reporting device block. Mirrors hcsshim hcsschema
/// `GuestCrashReporting` (`internal/hcs/schema2/guest_crash_reporting.go`).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct GuestCrashReporting {
    /// Windows kernel-dump settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub windows_crash_settings: Option<WindowsCrashReporting>,
}

/// Windows kernel crash-dump settings. Mirrors hcsshim hcsschema
/// `WindowsCrashReporting` (`internal/hcs/schema2/windows_crash_reporting.go`).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct WindowsCrashReporting {
    /// Host path for the kernel dump (`.dmp`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub dump_file_name: String,
    /// Maximum dump size in bytes (0 = unlimited).
    #[serde(default, skip_serializing_if = "is_zero_i64")]
    pub max_dump_size: i64,
    /// Dump type — e.g. `"Full"`, `"Mini"`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub dump_type: String,
}

/// Serde `skip_serializing_if` predicate for the `MaxDumpSize` field — omit it
/// from the wire payload when zero (HCS treats absent as unlimited).
#[allow(clippy::trivially_copy_pass_by_ref)] // serde requires `&T`
fn is_zero_i64(v: &i64) -> bool {
    *v == 0
}

/// Per-service `HvSocket` ACL entry. Mirrors hcsshim hcsschema `HvSocketServiceConfig`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct HvSocketServiceConfig {
    /// Security descriptor authorizing binds for this service GUID.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub bind_security_descriptor: String,
    /// Security descriptor authorizing connects for this service GUID.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub connect_security_descriptor: String,
    /// Allow wildcard binds for this service.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub allow_wildcard_binds: bool,
    /// Disable this service entry.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub disabled: bool,
}

/// System-wide `HvSocket` config for a VM. Mirrors hcsshim hcsschema `HvSocketSystemConfig`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct HvSocketSystemConfig {
    /// Default security descriptor authorizing binds when no per-service entry matches.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub default_bind_security_descriptor: String,
    /// Default security descriptor authorizing connects when no per-service entry matches.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub default_connect_security_descriptor: String,
    /// Per-service ACL entries keyed by service GUID.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub service_table: BTreeMap<String, HvSocketServiceConfig>,
}

/// `HvSocket` device block on a `VirtualMachine`. Mirrors hcsshim hcsschema `HvSocket2`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct HvSocket2 {
    /// System-wide `HvSocket` configuration for the VM.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hv_socket_config: Option<HvSocketSystemConfig>,
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

/// `Devices.VirtualSmb` body — wraps a list of [`VirtualSmbShare`] entries.
/// Matches hcsshim `internal/hcs/schema2/virtual_smb.go`:
/// `type VirtualSmb struct { Shares []VirtualSmbShare; DirectFileMappingInMB int64 }`.
///
/// Earlier iterations modeled this as a `BTreeMap<String, VirtualSmbShare>`
/// keyed by share name, which serializes as a JSON object and is rejected by
/// `HcsCreateComputeSystem` with `0xC037010D` (`HCS_E_INVALID_JSON`, generic
/// `Construct` failure with no specific field cited in the error envelope).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct VirtualSmb {
    /// Ordered list of shares projected into the VM. The `"os"` boot-files
    /// share goes first by hcsshim convention; parent-layer shares follow.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shares: Vec<VirtualSmbShare>,
    /// Optional hint (in MiB) to enable direct file mapping for VSMB; omitted
    /// when unset so HCS uses its default.
    #[serde(
        rename = "DirectFileMappingInMB",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub direct_file_mapping_in_mb: Option<i64>,
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
// Modify-setting request
// ---------------------------------------------------------------------------

/// Hot-modification request for a running compute system. Submitted to
/// `HcsModifyComputeSystem` (host-side) or the GCS `ModifySettings` RPC
/// (guest-side). Matches hcsshim's
/// `internal/hcs/schema2/modify_setting_request.go`.
///
/// `resource_path` is a slash-separated path into the compute-system tree
/// (e.g. `"VirtualMachine/Devices/VirtualSmb/Shares"`,
/// `"VirtualMachine/Devices/Scsi/0/Attachments/1"`,
/// `"Container/Networks"`).
///
/// `request_type` is one of `Add` / `Remove` / `Update`.
///
/// `settings` is the new value for the resource — its schema depends on the
/// `resource_path`. We keep it as `serde_json::Value` for flexibility;
/// helpers in [`crate::system`] build common shapes typed.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ModifySettingRequest {
    /// Slash-separated path identifying the resource to modify.
    pub resource_path: String,
    /// Whether to add, remove, or update the resource.
    pub request_type: ModifyRequestType,
    /// New value for the resource. Schema depends on `resource_path`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<serde_json::Value>,
    /// Optional guest-side request bundled along — e.g. `CombineLayers`
    /// instructions that need both a host-side hot-add AND a guest RPC.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guest_request: Option<serde_json::Value>,
}

/// Operation kind for a [`ModifySettingRequest`].
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum ModifyRequestType {
    /// Attach / hot-add the resource described by `settings`.
    #[default]
    Add,
    /// Detach / hot-remove the resource identified by `resource_path`.
    Remove,
    /// Update an existing resource's settings in place.
    Update,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::{
        ComputeSystem, Container, ContainerMemory, ContainerProcessor, GuestOs, Layer,
        ModifyRequestType, ModifySettingRequest, SchemaVersion, Statistics, Storage,
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
            should_terminate_on_last_handle_closed: None,
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
    fn empty_container_omits_all_optional_fields() {
        // A Container whose optionals are all None/empty must serialize with
        // NO null, empty-string, empty-array, or zero-valued fields — mimicking
        // hcsshim's `,omitempty`. The strict inbox GCS unmarshaller
        // (hcsshim #2714) tears down the VM on any unexpected/empty field.
        let doc = ComputeSystem {
            owner: "zlayer".to_string(),
            schema_version: SchemaVersion::default(),
            hosting_system_id: String::new(),
            container: Some(Container::default()),
            virtual_machine: None,
            should_terminate_on_last_handle_closed: None,
        };
        let json = serde_json::to_string(&doc).expect("serialize");

        // The empty Container body must collapse to `{}` — every field omitted.
        assert!(
            json.contains("\"Container\":{}"),
            "empty Container must serialize to {{}}, got: {json}",
        );
        // No null, no empty string, no empty array anywhere.
        assert!(!json.contains("null"), "no null fields: {json}");
        assert!(!json.contains("[]"), "no empty arrays: {json}");
        assert!(!json.contains("\"\""), "no empty strings: {json}");
        // Omitted optionals — none of these keys should appear.
        for key in [
            "GuestOs",
            "Storage",
            "Networking",
            "MappedDirectories",
            "MappedPipes",
            "Processor",
            "Memory",
            "HostingSystemId",
            "VirtualMachine",
            "ShouldTerminateOnLastHandleClosed",
        ] {
            assert!(
                !json.contains(&format!("\"{key}\"")),
                "key {key} must be omitted, got: {json}",
            );
        }
    }

    #[test]
    fn layer_serializes_id_not_uppercase_id() {
        // CRITICAL: hcsshim's Layer.Id is `json:"Id,omitempty"` (NOT `ID`).
        let layer = Layer {
            id: "0f2c0c2a-1111-2222-3333-444455556666".to_string(),
            path: r"C:\layers\base".to_string(),
        };
        let json = serde_json::to_string(&layer).expect("serialize layer");
        assert!(json.contains("\"Id\":"), "must emit `Id`: {json}");
        assert!(!json.contains("\"ID\":"), "must NOT emit `ID`: {json}");
        assert!(json.contains("\"Path\":"), "must emit `Path`: {json}");

        // Empty layer must omit both fields (omitempty parity).
        let empty = serde_json::to_string(&Layer::default()).expect("serialize");
        assert_eq!(
            empty, "{}",
            "empty Layer must serialize to {{}}, got: {empty}"
        );
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

    #[test]
    fn modify_setting_request_round_trip() {
        let req = ModifySettingRequest {
            resource_path: "VirtualMachine/Devices/VirtualSmb/Shares".to_string(),
            request_type: ModifyRequestType::Add,
            settings: Some(serde_json::json!({"Name": "layer1", "Path": "C:\\foo"})),
            guest_request: None,
        };
        let s = serde_json::to_string(&req).unwrap();
        assert!(s.contains("\"ResourcePath\""));
        assert!(s.contains("\"RequestType\":\"Add\""));
        assert!(s.contains("\"Settings\""));
        assert!(!s.contains("\"GuestRequest\""));
        let _back: ModifySettingRequest = serde_json::from_str(&s).unwrap();
    }
}
