//! macOS Apple-Virtualization (VZ) runtime
//!
//! Implements the [`Runtime`] trait using Apple's `Virtualization.framework`
//! (via the `objc2-virtualization` bindings) to run **ephemeral native-macOS
//! guest VMs** — the GitHub-runner / Tart model. This is distinct from, and
//! coexists with:
//!   - [`crate::runtimes::macos_sandbox::SandboxRuntime`] (Seatbelt, the macOS
//!     primary), and
//!   - [`crate::runtimes::macos_vm::VmRuntime`] (libkrun, Linux guests).
//!
//! Under the composite runtime the VZ delegate is preferred automatically for
//! images whose manifest is marked `com.zlayer.runtime=vz` (a VZ base bundle
//! from `zlayer vz build-base`); it can also be forced per-service with the
//! label `com.zlayer.isolation = "vz"` (or opted out with `"sandbox"`). Ordinary
//! Linux/Seatbelt images never route here. See `docs/macos-vz-runtime.md`.
//!
//! ## Model
//!
//! Each "container" is a full macOS guest VM cloned from a base image bundle:
//!
//! ```text
//! macOS Host
//!   +-- ZLayer VzRuntime
//!        +-- VZVirtualMachine (per container, on its own serial dispatch queue)
//!             +-- VZMacPlatformConfiguration (hardware model + fresh machine id + aux)
//!             +-- VZMacOSBootLoader (native macOS guest)
//!             +-- virtio block (disk.img, APFS clonefile CoW of the base)
//!             +-- virtio-net + NAT (per-VM random MAC -> dhcpd lease -> guest IP)
//!             +-- serial -> console.log
//!             +-- the entrypoint runs over SSH once the guest's sshd is up
//! ```
//!
//! ## Hard constraints
//!
//! - `VZVirtualMachine` is **not thread-safe**: every call happens on one
//!   serial [`DispatchQueue`] per VM. Async framework methods deliver their
//!   completion on that queue; we bridge those `block2` completion blocks to a
//!   std channel so the async runtime can await them.
//! - At most **two macOS VMs may run per host** (an Apple licensing limit). A
//!   process-wide [`AtomicU32`] gate enforces this with an RAII guard so an
//!   early `?` return can't leak the count.
//! - Booting a VM requires the `com.apple.security.virtualization` entitlement
//!   and code-signing. Where that is unavailable (CI), the VM-boot integration
//!   tests are `#[ignore]`d — the full code path is still compiled.
//!
//! ## Directory layout
//!
//! ```text
//! {data_dir}/vz/
//!   images/{sanitized_image}/        -- base bundle (shared, read-only)
//!     disk.img                       -- base macOS disk
//!     hardware-model.bin             -- VZMacHardwareModel dataRepresentation
//!     aux.img                        -- base auxiliary storage
//!   {service}-{replica}/             -- per-container state
//!     disk.img                       -- APFS clonefile CoW of the base disk
//!     aux.img                        -- per-VM auxiliary storage
//!     machine-id.bin                 -- fresh VZMacMachineIdentifier
//!     config.json                    -- serialized ServiceSpec
//!     console.log                    -- guest serial console
//!     ssh_key / ssh_key.pub          -- ephemeral keypair for exec
//! ```

#![allow(unsafe_code)]

use crate::cgroups_stats::ContainerStats;
use crate::error::{AgentError, Result};
use crate::runtime::{ContainerId, ContainerState, Runtime};
use std::collections::HashMap;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use zlayer_observability::logs::{LogEntry, LogSource, LogStream};
use zlayer_spec::{RegistryAuth, ServiceSpec};

use dispatch2::DispatchQueue;
use objc2::rc::Retained;
use objc2::AnyThread;
use objc2_foundation::{NSArray, NSData, NSError, NSFileHandle, NSString};

use crate::runtimes::macos_vz_shared::{
    clamp_cpu_count, clamp_memory_bytes, clone_or_copy, current_guest_ip, file_url,
    generate_ssh_keypair, read_vm_state, resolve_entrypoint, run_vm_lifecycle, spec_memory_mib,
    spec_vcpus, LiveVm, QueuePinned, VmLifecycleOp,
};
use objc2_virtualization::{
    VZBootLoader, VZDiskImageStorageDeviceAttachment, VZFileHandleSerialPortAttachment,
    VZMACAddress, VZMacAuxiliaryStorage, VZMacGraphicsDeviceConfiguration,
    VZMacGraphicsDisplayConfiguration, VZMacHardwareModel, VZMacMachineIdentifier,
    VZMacOSBootLoader, VZMacPlatformConfiguration, VZNATNetworkDeviceAttachment,
    VZStorageDeviceConfiguration, VZVirtioBlockDeviceConfiguration,
    VZVirtioConsoleDeviceSerialPortConfiguration, VZVirtioNetworkDeviceConfiguration,
    VZVirtualMachine, VZVirtualMachineConfiguration, VZVirtualMachineState,
};

/// The Apple-imposed ceiling on concurrently-running macOS guest VMs per host.
const MAX_MACOS_VMS: u32 = 2;

/// Width/height/DPI for the (required-to-boot) graphics device.
const DISPLAY_WIDTH: isize = 1920;
const DISPLAY_HEIGHT: isize = 1200;
const DISPLAY_DPI: isize = 80;

// ---------------------------------------------------------------------------
// 2-VM concurrency gate
// ---------------------------------------------------------------------------

/// RAII guard for the running-VM count. Acquired before a VM starts and
/// released on drop, so an early `?` return never leaks the count.
struct VmSlotGuard {
    count: Arc<AtomicU32>,
}

impl VmSlotGuard {
    /// Try to reserve one of the [`MAX_MACOS_VMS`] slots. Returns `None` when
    /// the host is already at the limit.
    fn acquire(count: &Arc<AtomicU32>) -> Option<Self> {
        loop {
            let cur = count.load(Ordering::SeqCst);
            if cur >= MAX_MACOS_VMS {
                return None;
            }
            if count
                .compare_exchange(cur, cur + 1, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                return Some(Self {
                    count: Arc::clone(count),
                });
            }
        }
    }
}

impl Drop for VmSlotGuard {
    fn drop(&mut self) {
        self.count.fetch_sub(1, Ordering::SeqCst);
    }
}

// ---------------------------------------------------------------------------
// Container record
// ---------------------------------------------------------------------------

pub(crate) struct VzContainer {
    /// Current container state.
    state: ContainerState,
    /// Per-container state directory.
    state_dir: PathBuf,
    /// Copy-on-write (`clonefile`) disk image path.
    disk_path: PathBuf,
    /// Per-VM auxiliary storage path.
    aux_path: PathBuf,
    /// Fresh machine-identifier data path.
    machine_id_path: PathBuf,
    /// Base bundle this container was cloned from.
    bundle_dir: PathBuf,
    /// Guest serial console log.
    console_log: PathBuf,
    /// Ephemeral private key path (exec over SSH).
    ssh_key_path: PathBuf,
    /// The VM's locally-administered MAC, used to find its DHCP lease.
    mac: String,
    /// The original service spec.
    spec: ServiceSpec,
    /// vCPUs / RAM the VM was configured with (stats reporting).
    vcpus: u32,
    ram_mib: u32,
    /// When the VM was started.
    started_at: Option<Instant>,
    /// The live VM + its queue, present only while running. Holds the 2-VM slot
    /// guard so the count is released when the container is stopped/removed.
    live: Option<LiveVm>,
    slot: Option<VmSlotGuard>,
}

// ---------------------------------------------------------------------------
// VzRuntime
// ---------------------------------------------------------------------------

/// macOS Apple-Virtualization runtime (native macOS guests). Opt-in only.
pub struct VzRuntime {
    /// Base directory for VZ state + base image bundles (`{data_dir}/vz`).
    vz_dir: PathBuf,
    /// Active containers keyed by `"{service}-{replica}"`.
    containers: Arc<RwLock<HashMap<String, VzContainer>>>,
    /// Process-wide running-VM counter (the 2-VM gate).
    vm_count: Arc<AtomicU32>,
}

impl std::fmt::Debug for VzRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VzRuntime")
            .field("vz_dir", &self.vz_dir)
            .field("running_vms", &self.vm_count.load(Ordering::SeqCst))
            .finish_non_exhaustive()
    }
}

impl VzRuntime {
    /// Create a new VZ runtime, provisioning its directory hierarchy.
    ///
    /// # Errors
    /// Returns an error if the required directories cannot be created or if
    /// `Virtualization.framework` reports it is unsupported on this host.
    pub fn new(_auth_ctx: Option<crate::runtime::ContainerAuthContext>) -> Result<Self> {
        let data_dir = zlayer_paths::ZLayerDirs::default_data_dir();
        let log_dir = zlayer_paths::ZLayerDirs::default_log_dir();
        let vz_dir = data_dir.join("vz");

        for d in [&vz_dir, &vz_dir.join("images"), &log_dir] {
            std::fs::create_dir_all(d).map_err(|e| {
                AgentError::Configuration(format!("Failed to create {}: {e}", d.display()))
            })?;
        }

        // Auto-detect the host. `VZVirtualMachine::isSupported()` reflects
        // hardware/OS capability (true on Apple Silicon running a supported
        // macOS); the `com.apple.security.virtualization` entitlement is
        // enforced later, at VM *creation*. We never hard-fail construction so
        // the daemon can still start and route non-VZ work elsewhere.
        let supported = unsafe { VZVirtualMachine::isSupported() };
        let apple_silicon = cfg!(target_arch = "aarch64");
        if supported {
            tracing::info!(
                vz_dir = %vz_dir.display(),
                apple_silicon,
                "macOS Apple-Virtualization (VZ) runtime ready (opt in with --runtime mac-vz \
                 or the com.zlayer.isolation=vz label)"
            );
        } else {
            tracing::warn!(
                apple_silicon,
                "Virtualization.framework reports UNSUPPORTED on this host — VZ-isolated \
                 services will fail to start. On Apple Silicon this is almost always a \
                 missing entitlement: sign the `zlayer` binary with `scripts/sign-vz.sh` \
                 (or build via `make build`, which auto-signs on macOS)."
            );
        }

        Ok(Self {
            vz_dir,
            containers: Arc::new(RwLock::new(HashMap::new())),
            vm_count: Arc::new(AtomicU32::new(0)),
        })
    }

    /// `"{service}-{replica}"` — the per-container directory / map key.
    fn container_dir_name(id: &ContainerId) -> String {
        format!("{}-{}", id.service, id.replica)
    }

    /// `{vz_dir}/{service}-{replica}/`.
    fn vm_dir(&self, id: &ContainerId) -> PathBuf {
        self.vz_dir.join(Self::container_dir_name(id))
    }

    /// `{vz_dir}/images/` — shared base image bundles.
    fn images_dir(&self) -> PathBuf {
        self.vz_dir.join("images")
    }

    /// Base bundle directory for an image reference.
    fn bundle_dir(&self, image: &str) -> PathBuf {
        self.images_dir().join(sanitize_image_name(image))
    }
}

// ---------------------------------------------------------------------------
// Pure helpers (unit-tested below)
// ---------------------------------------------------------------------------

/// Sanitize an image reference for use as a filesystem directory name.
fn sanitize_image_name(image: &str) -> String {
    image.replace(['/', ':', '@'], "_")
}

// ---------------------------------------------------------------------------
// objc helpers
// ---------------------------------------------------------------------------

/// Human-readable message from an `*mut NSError` (null => generic).
pub(crate) fn ns_error_message(err: *mut NSError) -> String {
    if err.is_null() {
        return "unknown VZ error".to_string();
    }
    // SAFETY: caller passes the framework-provided non-null error pointer; we
    // only read its localizedDescription, we do not take ownership.
    let desc = unsafe { (*err).localizedDescription() };
    desc.to_string()
}

// ---------------------------------------------------------------------------
// VM configuration + lifecycle (all VZ calls happen on the per-VM queue)
// ---------------------------------------------------------------------------

/// Plain (`Send`) inputs for building a VM configuration on the queue.
#[derive(Clone)]
pub(crate) struct VmBuildInputs {
    pub(crate) bundle_dir: PathBuf,
    pub(crate) disk_path: PathBuf,
    pub(crate) aux_path: PathBuf,
    pub(crate) machine_id_path: PathBuf,
    pub(crate) console_log: PathBuf,
    pub(crate) mac: String,
    pub(crate) cpu_count: usize,
    pub(crate) memory_bytes: u64,
}

impl VzContainer {
    /// Build a [`VZVirtualMachineConfiguration`]. MUST be called on the VM's
    /// dispatch queue. Returns an error string on any FFI/validation failure.
    pub(crate) fn build_configuration(
        inputs: &VmBuildInputs,
    ) -> std::result::Result<Retained<VZVirtualMachineConfiguration>, String> {
        unsafe {
            let config = VZVirtualMachineConfiguration::new();
            config.setCPUCount(inputs.cpu_count);
            config.setMemorySize(inputs.memory_bytes);

            // --- platform: hardware model + fresh machine id + aux storage ---
            let hw_bytes = std::fs::read(inputs.bundle_dir.join("hardware-model.bin"))
                .map_err(|e| format!("read hardware-model.bin: {e}"))?;
            let hw_data = NSData::with_bytes(&hw_bytes);
            let hardware_model = VZMacHardwareModel::initWithDataRepresentation(
                VZMacHardwareModel::alloc(),
                &hw_data,
            )
            .ok_or_else(|| "invalid hardware model data".to_string())?;
            if !hardware_model.isSupported() {
                return Err("base image hardware model is not supported on this host".to_string());
            }

            let machine_id = if let Ok(bytes) = std::fs::read(&inputs.machine_id_path) {
                let data = NSData::with_bytes(&bytes);
                VZMacMachineIdentifier::initWithDataRepresentation(
                    VZMacMachineIdentifier::alloc(),
                    &data,
                )
                .unwrap_or_else(|| VZMacMachineIdentifier::new())
            } else {
                VZMacMachineIdentifier::new()
            };

            let aux_url = file_url(&inputs.aux_path);
            let aux = VZMacAuxiliaryStorage::initWithURL(VZMacAuxiliaryStorage::alloc(), &aux_url);

            let platform = VZMacPlatformConfiguration::new();
            platform.setHardwareModel(&hardware_model);
            platform.setMachineIdentifier(&machine_id);
            platform.setAuxiliaryStorage(Some(&aux));
            config.setPlatform(&platform);

            // --- boot loader: native macOS guest ---
            let boot: Retained<VZBootLoader> = Retained::into_super(VZMacOSBootLoader::new());
            config.setBootLoader(Some(&boot));

            // --- graphics (required to boot a macOS guest) ---
            let display = VZMacGraphicsDisplayConfiguration::initWithWidthInPixels_heightInPixels_pixelsPerInch(
                VZMacGraphicsDisplayConfiguration::alloc(),
                DISPLAY_WIDTH,
                DISPLAY_HEIGHT,
                DISPLAY_DPI,
            );
            let graphics = VZMacGraphicsDeviceConfiguration::new();
            let displays = NSArray::from_retained_slice(&[display]);
            graphics.setDisplays(&displays);
            let graphics_arr = NSArray::from_retained_slice(&[Retained::into_super(graphics)]);
            config.setGraphicsDevices(&graphics_arr);

            // --- storage: virtio block over the CoW disk image ---
            let disk_url = file_url(&inputs.disk_path);
            let attachment = VZDiskImageStorageDeviceAttachment::initWithURL_readOnly_error(
                VZDiskImageStorageDeviceAttachment::alloc(),
                &disk_url,
                false,
            )
            .map_err(|e| format!("disk attachment: {}", e.localizedDescription()))?;
            let block = VZVirtioBlockDeviceConfiguration::initWithAttachment(
                VZVirtioBlockDeviceConfiguration::alloc(),
                &attachment,
            );
            let storage: Retained<VZStorageDeviceConfiguration> = Retained::into_super(block);
            let storage_arr = NSArray::from_retained_slice(&[storage]);
            config.setStorageDevices(&storage_arr);

            // --- network: virtio + NAT, with the per-VM MAC ---
            let net = VZVirtioNetworkDeviceConfiguration::new();
            let nat = VZNATNetworkDeviceAttachment::new();
            net.setAttachment(Some(&nat));
            let mac_str = NSString::from_str(&inputs.mac);
            if let Some(mac) = VZMACAddress::initWithString(VZMACAddress::alloc(), &mac_str) {
                net.setMACAddress(&mac);
            }
            let net_super = Retained::into_super(net);
            let net_arr = NSArray::from_retained_slice(&[net_super]);
            config.setNetworkDevices(&net_arr);

            // --- serial console -> console.log ---
            // Truncate/create the log and hand its fd to the guest's serial port.
            let _ = std::fs::File::create(&inputs.console_log);
            let log_str = NSString::from_str(&inputs.console_log.to_string_lossy());
            if let Some(write_fh) = NSFileHandle::fileHandleForWritingAtPath(&log_str) {
                let serial = VZVirtioConsoleDeviceSerialPortConfiguration::new();
                let attach = VZFileHandleSerialPortAttachment::initWithFileHandleForReading_fileHandleForWriting(
                    VZFileHandleSerialPortAttachment::alloc(),
                    None,
                    Some(&write_fh),
                );
                serial.setAttachment(Some(&attach));
                let serial_super = Retained::into_super(serial);
                let serial_arr = NSArray::from_retained_slice(&[serial_super]);
                config.setSerialPorts(&serial_arr);
            }

            // --- validate ---
            config
                .validateWithError()
                .map_err(|e| format!("configuration invalid: {}", e.localizedDescription()))?;

            Ok(config)
        }
    }
}

// ---------------------------------------------------------------------------
// SSH exec
// ---------------------------------------------------------------------------

/// Run `cmd` inside the guest over SSH, returning `(exit_code, stdout, stderr)`.
/// Uses the system `ssh` with the container's ephemeral key. Requires the guest
/// IP (from the DHCP lease) and a reachable sshd.
async fn ssh_exec(
    key_path: &Path,
    ip: IpAddr,
    user: &str,
    cmd: &[String],
) -> Result<(i32, String, String)> {
    let mut command = tokio::process::Command::new("ssh");
    command
        .arg("-i")
        .arg(key_path)
        .args([
            "-o",
            "StrictHostKeyChecking=no",
            "-o",
            "UserKnownHostsFile=/dev/null",
            "-o",
            "ConnectTimeout=10",
            "-o",
            "BatchMode=yes",
        ])
        .arg(format!("{user}@{ip}"));
    if cmd.is_empty() {
        command.arg("true");
    } else {
        command.args(cmd);
    }
    let out = command
        .output()
        .await
        .map_err(|e| AgentError::Internal(format!("ssh {user}@{ip} spawn failed: {e}")))?;
    Ok((
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    ))
}

/// SSH login user for the guest. Tart/cirrus macOS images use `admin`; override
/// with the `com.zlayer.vz.user` label.
fn ssh_user(spec: &ServiceSpec) -> String {
    spec.labels
        .get("com.zlayer.vz.user")
        .cloned()
        .unwrap_or_else(|| "admin".to_string())
}

// ---------------------------------------------------------------------------
// Runtime trait implementation
// ---------------------------------------------------------------------------

#[async_trait::async_trait]
impl Runtime for VzRuntime {
    async fn pull_image(&self, image: &str) -> Result<()> {
        self.pull_image_with_policy(
            image,
            zlayer_spec::PullPolicy::IfNotPresent,
            None,
            zlayer_spec::SourcePolicy::default(),
        )
        .await
    }

    async fn pull_image_with_policy(
        &self,
        image: &str,
        policy: zlayer_spec::PullPolicy,
        _auth: Option<&RegistryAuth>,
        _source: zlayer_spec::SourcePolicy,
    ) -> Result<()> {
        let bundle = self.bundle_dir(image);
        let have_bundle = bundle.join("disk.img").exists()
            && bundle.join("hardware-model.bin").exists()
            && bundle.join("aux.img").exists();

        match policy {
            zlayer_spec::PullPolicy::IfNotPresent | zlayer_spec::PullPolicy::Never
                if have_bundle =>
            {
                return Ok(());
            }
            zlayer_spec::PullPolicy::Never => {
                return Err(AgentError::PullFailed {
                    image: image.to_string(),
                    reason: "base bundle not present and pull policy is Never".to_string(),
                });
            }
            _ => {}
        }

        // A locally-staged bundle directory: nothing to fetch.
        if Path::new(image).is_dir() {
            let src = PathBuf::from(image);
            if src != bundle {
                std::fs::create_dir_all(&bundle).ok();
                for f in ["disk.img", "hardware-model.bin", "aux.img"] {
                    if src.join(f).exists() {
                        std::fs::copy(src.join(f), bundle.join(f)).map_err(|e| {
                            AgentError::PullFailed {
                                image: image.to_string(),
                                reason: format!("staging {f}: {e}"),
                            }
                        })?;
                    }
                }
            }
            return Ok(());
        }

        // OCI artifact: pull + unpack a Tart-style bundle into the images dir.
        std::fs::create_dir_all(&bundle).ok();
        let cache_path = self.images_dir().join("blobs.redb");
        let cache = zlayer_registry::CacheType::persistent_at(&cache_path)
            .build()
            .await
            .map_err(|e| AgentError::PullFailed {
                image: image.to_string(),
                reason: format!("open blob cache: {e}"),
            })?;
        let puller = zlayer_registry::ImagePuller::with_cache(cache);
        let layers = puller
            .pull_image(image, &zlayer_registry::RegistryAuth::Anonymous)
            .await
            .map_err(|e| AgentError::PullFailed {
                image: image.to_string(),
                reason: format!("pull layers: {e}"),
            })?;
        let mut unpacker = zlayer_registry::LayerUnpacker::new(bundle.clone());
        unpacker
            .unpack_layers(&layers)
            .await
            .map_err(|e| AgentError::PullFailed {
                image: image.to_string(),
                reason: format!("unpack bundle: {e}"),
            })?;

        if !(bundle.join("disk.img").exists()
            && bundle.join("hardware-model.bin").exists()
            && bundle.join("aux.img").exists())
        {
            return Err(AgentError::PullFailed {
                image: image.to_string(),
                reason: "pulled artifact is not a macOS VM bundle (need disk.img, \
                         hardware-model.bin, aux.img)"
                    .to_string(),
            });
        }
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    async fn create_container(&self, id: &ContainerId, spec: &ServiceSpec) -> Result<()> {
        let dir_name = Self::container_dir_name(id);
        let state_dir = self.vm_dir(id);
        let bundle = self.bundle_dir(&spec.image.name.to_string());

        if !bundle.join("disk.img").exists() {
            return Err(AgentError::CreateFailed {
                id: dir_name,
                reason: format!(
                    "base bundle {} missing; pull the image first",
                    bundle.display()
                ),
            });
        }

        std::fs::create_dir_all(&state_dir).map_err(|e| AgentError::CreateFailed {
            id: dir_name.clone(),
            reason: format!("create state dir: {e}"),
        })?;

        let disk_path = state_dir.join("disk.img");
        let aux_path = state_dir.join("aux.img");
        let machine_id_path = state_dir.join("machine-id.bin");
        let console_log = state_dir.join("console.log");
        let ssh_key_path = state_dir.join("ssh_key");

        // CoW-clone the base disk (APFS clonefile), falling back to a full copy.
        clone_or_copy(&bundle.join("disk.img"), &disk_path).map_err(|e| {
            AgentError::CreateFailed {
                id: dir_name.clone(),
                reason: format!("clone disk: {e}"),
            }
        })?;
        // Per-VM aux: copy the base (a fresh aux is created at start if needed).
        if bundle.join("aux.img").exists() && !aux_path.exists() {
            std::fs::copy(bundle.join("aux.img"), &aux_path).map_err(|e| {
                AgentError::CreateFailed {
                    id: dir_name.clone(),
                    reason: format!("copy aux: {e}"),
                }
            })?;
        }

        // Fresh machine identifier per VM.
        let machine_id_data = unsafe {
            let id = VZMacMachineIdentifier::new();
            id.dataRepresentation().to_vec()
        };
        std::fs::write(&machine_id_path, &machine_id_data).ok();

        // Per-VM locally-administered MAC.
        let mac = unsafe {
            VZMACAddress::randomLocallyAdministeredAddress()
                .string()
                .to_string()
        };

        // Ephemeral SSH keypair for exec.
        generate_ssh_keypair(&ssh_key_path).await;

        // Persist the spec for diagnostics.
        if let Ok(json) = serde_json::to_string_pretty(spec) {
            std::fs::write(state_dir.join("config.json"), json).ok();
        }

        // macOS guests need substantial RAM: default 4096 MiB, floor 2048 MiB.
        let vcpus = spec_vcpus(spec, 2);
        let ram_mib = spec_memory_mib(spec, 4096, 2048);

        let container = VzContainer {
            state: ContainerState::Pending,
            state_dir,
            disk_path,
            aux_path,
            machine_id_path,
            bundle_dir: bundle,
            console_log,
            ssh_key_path,
            mac,
            spec: spec.clone(),
            vcpus,
            ram_mib,
            started_at: None,
            live: None,
            slot: None,
        };

        self.containers.write().await.insert(dir_name, container);
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    async fn start_container(&self, id: &ContainerId) -> Result<()> {
        let dir_name = Self::container_dir_name(id);

        // Snapshot the build inputs (Send) without holding the lock across the
        // blocking queue work.
        let (inputs, spec, ssh_key_path, mac, console_log) = {
            let guard = self.containers.read().await;
            let c = guard.get(&dir_name).ok_or_else(|| AgentError::NotFound {
                container: dir_name.clone(),
                reason: "not created".to_string(),
            })?;
            (
                VmBuildInputs {
                    bundle_dir: c.bundle_dir.clone(),
                    disk_path: c.disk_path.clone(),
                    aux_path: c.aux_path.clone(),
                    machine_id_path: c.machine_id_path.clone(),
                    console_log: c.console_log.clone(),
                    mac: c.mac.clone(),
                    cpu_count: clamp_cpu_count(c.vcpus),
                    memory_bytes: clamp_memory_bytes(c.ram_mib),
                },
                c.spec.clone(),
                c.ssh_key_path.clone(),
                c.mac.clone(),
                c.console_log.clone(),
            )
        };

        if !unsafe { VZVirtualMachine::isSupported() } {
            return Err(AgentError::StartFailed {
                id: dir_name,
                reason: "Virtualization.framework is unavailable. On Apple Silicon, grant the \
                         com.apple.security.virtualization entitlement by signing the binary \
                         with `scripts/sign-vz.sh` (or build via `make build`, which auto-signs \
                         on macOS)."
                    .to_string(),
            });
        }

        // Reserve a VM slot (RAII): released if anything below fails.
        let slot = VmSlotGuard::acquire(&self.vm_count).ok_or_else(|| AgentError::StartFailed {
            id: dir_name.clone(),
            reason: format!("host already running the maximum of {MAX_MACOS_VMS} macOS VMs"),
        })?;

        // Build the config + VM + start it on a dedicated serial queue. All of
        // this is blocking Obj-C work, so run it on a blocking thread.
        let dir_for_task = dir_name.clone();
        let live = tokio::task::spawn_blocking(move || -> std::result::Result<LiveVm, String> {
            let queue = DispatchQueue::new(&format!("com.zlayer.vz.{dir_for_task}"), None);

            // Build the config + create the VM on the queue.
            let (tx, rx) = std::sync::mpsc::channel::<
                std::result::Result<QueuePinned<Retained<VZVirtualMachine>>, String>,
            >();
            let inputs_q = inputs.clone();
            let queue_for_vm = queue.clone();
            queue.exec_sync(move || {
                let built = VzContainer::build_configuration(&inputs_q).map(|config| {
                    // SAFETY: created on this serial queue, the only place it is used.
                    let vm = unsafe {
                        VZVirtualMachine::initWithConfiguration_queue(
                            VZVirtualMachine::alloc(),
                            &config,
                            &queue_for_vm,
                        )
                    };
                    QueuePinned(vm)
                });
                let _ = tx.send(built);
            });
            let pinned = rx
                .recv()
                .unwrap_or_else(|_| Err("VM build channel closed".to_string()))?;

            let live = LiveVm {
                queue,
                vm: Arc::new(pinned),
            };
            run_vm_lifecycle(&live, VmLifecycleOp::Start)?;
            Ok(live)
        })
        .await
        .map_err(|e| AgentError::StartFailed {
            id: dir_name.clone(),
            reason: format!("VM start task panicked: {e}"),
        })?
        .map_err(|e| AgentError::StartFailed {
            id: dir_name.clone(),
            reason: e,
        })?;

        // Record the running VM (move the slot guard in so it lives with the VM).
        {
            let mut guard = self.containers.write().await;
            if let Some(c) = guard.get_mut(&dir_name) {
                c.live = Some(live);
                c.slot = Some(slot);
                c.state = ContainerState::Running;
                c.started_at = Some(Instant::now());
            }
        }

        // Best-effort: wait for sshd, then run the entrypoint in the guest,
        // appending output to the console log. Failures here don't fail start —
        // the VM is up; the entrypoint is the workload.
        let containers = Arc::clone(&self.containers);
        // macOS-guest VMs run a macOS bundle, not an OCI image, so there is no
        // image config to merge — pass `None` (spec command / `true` fallback).
        let entry = resolve_entrypoint(&spec, None);
        let user = ssh_user(&spec);
        let dir_bg = dir_name.clone();
        tokio::spawn(async move {
            let ip = wait_for_guest_ip(&containers, &dir_bg, &mac, Duration::from_secs(180)).await;
            let Some(ip) = ip else {
                tracing::warn!(container = %dir_bg, "VZ guest never acquired a DHCP lease");
                return;
            };
            // Poll sshd, then run the entrypoint.
            for _ in 0..60u32 {
                match ssh_exec(&ssh_key_path, ip, &user, &["true".to_string()]).await {
                    Ok((0, _, _)) => break,
                    _ => tokio::time::sleep(Duration::from_secs(2)).await,
                }
            }
            match ssh_exec(&ssh_key_path, ip, &user, &entry).await {
                Ok((code, out, err)) => {
                    use std::io::Write as _;
                    if let Ok(mut f) = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&console_log)
                    {
                        let _ = writeln!(f, "[entrypoint exit={code}]\n{out}{err}");
                    }
                }
                Err(e) => {
                    tracing::warn!(container = %dir_bg, error = %e, "VZ entrypoint exec failed");
                }
            }
        });

        Ok(())
    }

    async fn stop_container(&self, id: &ContainerId, _timeout: Duration) -> Result<()> {
        let dir_name = Self::container_dir_name(id);
        let live = {
            let mut guard = self.containers.write().await;
            let c = guard
                .get_mut(&dir_name)
                .ok_or_else(|| AgentError::NotFound {
                    container: dir_name.clone(),
                    reason: "not found".to_string(),
                })?;
            c.state = ContainerState::Exited { code: 0 };
            // Drop the slot guard (releases the 2-VM count) and take the VM out.
            c.slot = None;
            c.live.take()
        };
        if let Some(live) = live {
            let _ = tokio::task::spawn_blocking(move || {
                // requestStop is the graceful path; fall back to forced stop.
                let r = run_vm_lifecycle(&live, VmLifecycleOp::Stop);
                drop(live);
                r
            })
            .await;
        }
        Ok(())
    }

    async fn remove_container(&self, id: &ContainerId) -> Result<()> {
        let dir_name = Self::container_dir_name(id);
        // Ensure stopped first.
        let _ = self.stop_container(id, Duration::from_secs(5)).await;
        let state_dir = {
            let mut guard = self.containers.write().await;
            guard.remove(&dir_name).map(|c| c.state_dir)
        };
        if let Some(dir) = state_dir {
            let _ = tokio::fs::remove_dir_all(dir).await;
        }
        Ok(())
    }

    async fn container_state(&self, id: &ContainerId) -> Result<ContainerState> {
        let dir_name = Self::container_dir_name(id);
        let guard = self.containers.read().await;
        let c = guard.get(&dir_name).ok_or_else(|| AgentError::NotFound {
            container: dir_name.clone(),
            reason: "not found".to_string(),
        })?;
        // Reconcile with the live VM's actual state when running.
        if let Some(live) = &c.live {
            let state = read_vm_state(live);
            let reconciled = match state {
                VZVirtualMachineState::Running | VZVirtualMachineState::Paused => {
                    ContainerState::Running
                }
                VZVirtualMachineState::Error => ContainerState::Failed {
                    reason: "VM entered error state".to_string(),
                },
                VZVirtualMachineState::Stopped => ContainerState::Exited { code: 0 },
                _ => c.state.clone(),
            };
            return Ok(reconciled);
        }
        Ok(c.state.clone())
    }

    async fn container_logs(&self, id: &ContainerId, tail: usize) -> Result<Vec<LogEntry>> {
        self.read_console(id, tail).await
    }

    async fn exec(&self, id: &ContainerId, cmd: &[String]) -> Result<(i32, String, String)> {
        let dir_name = Self::container_dir_name(id);
        let (key, mac, user) = {
            let guard = self.containers.read().await;
            let c = guard.get(&dir_name).ok_or_else(|| AgentError::NotFound {
                container: dir_name.clone(),
                reason: "not found".to_string(),
            })?;
            (c.ssh_key_path.clone(), c.mac.clone(), ssh_user(&c.spec))
        };
        let ip = current_guest_ip(&mac).await.ok_or_else(|| {
            AgentError::Internal(format!("exec {dir_name}: guest has no DHCP lease yet"))
        })?;
        ssh_exec(&key, ip, &user, cmd).await
    }

    async fn get_container_stats(&self, id: &ContainerId) -> Result<ContainerStats> {
        let dir_name = Self::container_dir_name(id);
        let guard = self.containers.read().await;
        let c = guard.get(&dir_name).ok_or_else(|| AgentError::NotFound {
            container: dir_name.clone(),
            reason: "not found".to_string(),
        })?;
        // VZ exposes no per-VM live metrics; report the configured allocation.
        Ok(ContainerStats {
            cpu_usage_usec: 0,
            memory_bytes: 0,
            memory_limit: u64::from(c.ram_mib) * 1024 * 1024,
            timestamp: Instant::now(),
        })
    }

    async fn wait_container(&self, id: &ContainerId) -> Result<i32> {
        let dir_name = Self::container_dir_name(id);
        loop {
            {
                let guard = self.containers.read().await;
                match guard.get(&dir_name) {
                    None => return Ok(0),
                    Some(c) => match &c.live {
                        None => return Ok(0),
                        Some(live) => {
                            if read_vm_state(live) == VZVirtualMachineState::Stopped {
                                return Ok(0);
                            }
                        }
                    },
                }
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }

    async fn get_logs(&self, id: &ContainerId) -> Result<Vec<LogEntry>> {
        self.read_console(id, 1000).await
    }

    async fn get_container_pid(&self, _id: &ContainerId) -> Result<Option<u32>> {
        // A VM has no host-visible container PID.
        Ok(None)
    }

    async fn get_container_ip(&self, id: &ContainerId) -> Result<Option<IpAddr>> {
        let dir_name = Self::container_dir_name(id);
        let mac = {
            let guard = self.containers.read().await;
            guard.get(&dir_name).map(|c| c.mac.clone())
        };
        match mac {
            Some(mac) => Ok(current_guest_ip(&mac).await),
            None => Ok(None),
        }
    }

    async fn kill_container(&self, id: &ContainerId, signal: Option<&str>) -> Result<()> {
        // VZ has no signal delivery; SIGKILL/SIGTERM map to a forced stop.
        let sig = signal.unwrap_or("SIGKILL");
        let _ = crate::runtime::validate_signal(sig)?;
        self.stop_container(id, Duration::from_secs(0)).await
    }

    async fn list_images(&self) -> Result<Vec<crate::runtime::ImageInfo>> {
        let mut out = Vec::new();
        if let Ok(rd) = std::fs::read_dir(self.images_dir()) {
            for entry in rd.flatten() {
                if entry.path().join("disk.img").exists() {
                    out.push(crate::runtime::ImageInfo {
                        reference: entry.file_name().to_string_lossy().into_owned(),
                        digest: None,
                        size_bytes: std::fs::metadata(entry.path().join("disk.img"))
                            .map(|m| m.len())
                            .ok(),
                    });
                }
            }
        }
        Ok(out)
    }

    async fn pause_container(&self, id: &ContainerId) -> Result<()> {
        let dir_name = Self::container_dir_name(id);
        let live = {
            let guard = self.containers.read().await;
            // We cannot move the LiveVm out for a transient op; instead clone the
            // Arc handles needed for the queue dispatch.
            guard.get(&dir_name).and_then(|c| {
                c.live.as_ref().map(|l| LiveVm {
                    queue: l.queue.clone(),
                    vm: Arc::clone(&l.vm),
                })
            })
        };
        let Some(live) = live else {
            return Err(AgentError::NotFound {
                container: dir_name,
                reason: "not running".to_string(),
            });
        };
        tokio::task::spawn_blocking(move || run_vm_lifecycle(&live, VmLifecycleOp::Pause))
            .await
            .map_err(|e| AgentError::Internal(format!("pause task: {e}")))?
            .map_err(|e| AgentError::Internal(format!("pause: {e}")))
    }

    async fn unpause_container(&self, id: &ContainerId) -> Result<()> {
        let dir_name = Self::container_dir_name(id);
        let live = {
            let guard = self.containers.read().await;
            guard.get(&dir_name).and_then(|c| {
                c.live.as_ref().map(|l| LiveVm {
                    queue: l.queue.clone(),
                    vm: Arc::clone(&l.vm),
                })
            })
        };
        let Some(live) = live else {
            return Err(AgentError::NotFound {
                container: dir_name,
                reason: "not running".to_string(),
            });
        };
        tokio::task::spawn_blocking(move || run_vm_lifecycle(&live, VmLifecycleOp::Resume))
            .await
            .map_err(|e| AgentError::Internal(format!("resume task: {e}")))?
            .map_err(|e| AgentError::Internal(format!("resume: {e}")))
    }
}

impl VzRuntime {
    /// Read the tail of a container's serial console log as [`LogEntry`]s.
    async fn read_console(&self, id: &ContainerId, tail: usize) -> Result<Vec<LogEntry>> {
        let dir_name = Self::container_dir_name(id);
        let log = {
            let guard = self.containers.read().await;
            guard.get(&dir_name).map(|c| c.console_log.clone())
        };
        let Some(log) = log else {
            return Ok(Vec::new());
        };
        let content = tokio::fs::read_to_string(&log).await.unwrap_or_default();
        let lines: Vec<&str> = content.lines().collect();
        let start = lines.len().saturating_sub(tail);
        Ok(lines[start..]
            .iter()
            .map(|l| LogEntry {
                timestamp: chrono::Utc::now(),
                stream: LogStream::Stdout,
                source: LogSource::Container(dir_name.clone()),
                message: (*l).to_string(),
                service: None,
                deployment: None,
            })
            .collect())
    }
}

// ---------------------------------------------------------------------------
// DHCP polling
// ---------------------------------------------------------------------------

/// Poll for the guest's DHCP lease up to `timeout`, recording it on the
/// container record when found.
async fn wait_for_guest_ip(
    containers: &Arc<RwLock<HashMap<String, VzContainer>>>,
    dir_name: &str,
    mac: &str,
    timeout: Duration,
) -> Option<IpAddr> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        // Stop polling if the container disappeared or stopped.
        {
            let guard = containers.read().await;
            if guard.get(dir_name).is_none_or(|c| c.live.is_none()) {
                return None;
            }
        }
        if let Some(ip) = current_guest_ip(mac).await {
            return Some(ip);
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_image_name_replaces_separators() {
        assert_eq!(
            sanitize_image_name("ghcr.io/cirruslabs/macos-sonoma:latest"),
            "ghcr.io_cirruslabs_macos-sonoma_latest"
        );
    }

    // NOTE: `parse_memory_to_mib` / `safe_vcpu_count` moved to
    // `macos_vz_shared` (where they are now exercised); the guest-agnostic
    // config helpers are tested there.

    #[test]
    fn vm_slot_gate_caps_at_two() {
        let count = Arc::new(AtomicU32::new(0));
        let a = VmSlotGuard::acquire(&count).unwrap();
        let b = VmSlotGuard::acquire(&count).unwrap();
        assert!(
            VmSlotGuard::acquire(&count).is_none(),
            "third VM must be refused"
        );
        drop(a);
        let _c = VmSlotGuard::acquire(&count).expect("slot freed on drop");
        drop(b);
    }

    #[test]
    fn resolve_entrypoint_falls_back_to_true() {
        let spec = ServiceSpec::minimal("svc", "ghcr.io/x/macos:latest");
        assert_eq!(resolve_entrypoint(&spec, None), vec!["true".to_string()]);
    }

    /// Full create -> start -> exec -> stop against a real macOS guest.
    ///
    /// `#[ignore]`d: it requires the `com.apple.security.virtualization`
    /// entitlement (a code-signed binary) AND a base bundle directory supplied
    /// via `ZLAYER_VZ_TEST_BUNDLE` (containing `disk.img`, `hardware-model.bin`,
    /// `aux.img`). Run locally on an entitled build with:
    /// `ZLAYER_VZ_TEST_BUNDLE=/path/to/bundle cargo test -p zlayer-agent \
    ///  vz_boot_exec_stop -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "needs the virtualization entitlement + a base macOS bundle"]
    async fn vz_boot_exec_stop() {
        let Ok(bundle) = std::env::var("ZLAYER_VZ_TEST_BUNDLE") else {
            eprintln!("ZLAYER_VZ_TEST_BUNDLE unset — skipping");
            return;
        };
        assert!(
            unsafe { VZVirtualMachine::isSupported() },
            "Virtualization.framework not supported (missing entitlement?)"
        );

        let rt = VzRuntime::new(None).expect("construct VzRuntime");
        let id = ContainerId::new("vztest", 1);
        let spec = ServiceSpec::minimal("vztest", bundle.as_str());

        rt.create_container(&id, &spec).await.expect("create");
        rt.start_container(&id).await.expect("start");

        // Wait for an IP, then exec.
        let mut ip = None;
        for _ in 0..90 {
            if let Ok(Some(addr)) = rt.get_container_ip(&id).await {
                ip = Some(addr);
                break;
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        assert!(ip.is_some(), "guest never acquired a DHCP lease");

        let (code, out, _err) = rt
            .exec(&id, &["echo".to_string(), "hello".to_string()])
            .await
            .expect("exec");
        assert_eq!(code, 0);
        assert!(out.contains("hello"));

        rt.stop_container(&id, Duration::from_secs(10))
            .await
            .expect("stop");
        rt.remove_container(&id).await.expect("remove");
    }
}
