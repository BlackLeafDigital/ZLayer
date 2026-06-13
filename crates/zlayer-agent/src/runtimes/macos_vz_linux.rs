//! macOS Apple-Virtualization (VZ) **Linux-guest** runtime
//!
//! Implements the [`Runtime`] trait using Apple's `Virtualization.framework`
//! to run **Linux** guests on macOS. This is the first-party VM path on macOS
//! — unlike the libkrun [`crate::runtimes::macos_vm::VmRuntime`] it loads no
//! external dylib (VZ ships with the OS) and unlike the native-macOS-guest
//! [`crate::runtimes::macos_vz::VzRuntime`] it boots a Linux kernel + initramfs
//! rather than a macOS bundle.
//!
//! ## Routing
//!
//! Under [`crate::runtimes::composite::CompositeRuntime`] this runtime is the
//! **default** path for Linux images on macOS. It is also selected explicitly
//! via the `com.zlayer.isolation = "vz-linux"` label or for images whose
//! manifest carries the `com.zlayer.runtime = "vz-linux"` marker. The libkrun
//! runtime remains reachable through `com.zlayer.isolation = "vm"`.
//!
//! ## Phase status
//!
//! **Phases 2–4 (boot + run a workload):** image pulling (OCI rootfs
//! extraction) is implemented, container records are created, and the runtime
//! boots a real Linux guest — a generic platform + [`VZLinuxBootLoader`] over a
//! kernel `Image` + `initramfs.cpio.gz`, headless, serial console wired to
//! `console.log`.
//!
//! **Phase 3 (virtiofs rootfs):** `build_config_linux` attaches a
//! `VZVirtioFileSystemDeviceConfiguration` (tag `rootfs`) that shares the
//! extracted image rootfs into the guest **read-only**; the in-guest agent
//! overlays a tmpfs upper and `pivot_root`s onto it.
//!
//! **Phase 4 (vsock workload):** a `VZVirtioSocketDeviceConfiguration` gives the
//! host a control channel to the guest PID1 agent. After the VM reaches Running,
//! the host connects to `AF_VSOCK` port `proto::CONTROL_PORT` (on the VM's
//! serial queue, bridging the connected fd back via a completion block), sends a
//! `proto::Msg::Run`, and drains `Stdout`/`Stderr`/`Started`/`Exited` frames —
//! capturing logs and the real exit code. `wait_container` returns the actual
//! workload code, `container_state` reflects the agent's terminal state, and the
//! log accessors surface captured stdout/stderr.
//!
//! **Phase 5 (exec):** [`exec`](VzLinuxRuntime::exec) opens a SECOND vsock
//! connection to the same `proto::CONTROL_PORT`, sends a `proto::Msg::Exec`
//! (argv + the container spec's env), and drains `Stdout`/`Stderr`/`Exited`
//! frames into buffers, returning `(exit_code, stdout, stderr)`. The guest
//! agent enters the running workload's PID namespace (`setns` on
//! `/proc/{pid}/ns/pid`) so the exec'd process shares the container view. The
//! streaming `exec_stream` uses the trait default over this buffered `exec`.
//!
//! **Phase 7 (lifecycle polish):** [`stop_container`](VzLinuxRuntime::stop_container)
//! is graceful — it delivers `SIGTERM` to the workload over a fresh vsock
//! control connection and waits up to the caller's timeout for the agent's
//! `Exited` frame before force-stopping the VM; [`kill_container`](VzLinuxRuntime::kill_container)
//! maps the requested signal name to its Linux number and delivers it for real
//! (defaulting to `SIGKILL`); [`remove_container`](VzLinuxRuntime::remove_container)
//! ensures the VM is stopped and aborts every spawned task; both stop and remove
//! are idempotent. [`get_container_stats`](VzLinuxRuntime::get_container_stats)
//! reports the configured allocation (VZ exposes no live metrics) with a
//! non-zero `memory_bytes` estimate, and [`pause_container`](VzLinuxRuntime::pause_container)/
//! [`unpause_container`](VzLinuxRuntime::unpause_container) use VZ's native
//! pause/resume. No method returns an "unimplemented" sentinel any longer.
//!
//! Guest kernel artifacts are resolved by [`VzLinuxRuntime::ensure_linux_kernel`]
//! from the `ZLAYER_VZ_LINUX_KERNEL` / `ZLAYER_VZ_LINUX_INITRD` dev-override env
//! vars, else by pulling the published bundle image
//! `ghcr.io/blackleafdigital/zlayer/vz-linux:arm64` through the normal image
//! machinery (digest-compared against, and installed into, the on-disk
//! `{data_dir}/vz/linux/kernel/` cache), else by serving that cache directly
//! when the registry is unreachable.
//!
//! ## Directory layout
//!
//! ```text
//! {data_dir}/vz/linux/
//!   images/{sanitized_image}/rootfs/   -- extracted OCI image layers (shared)
//!   kernel/                            -- guest kernel cache (Image + initramfs.cpio.gz + manifest.json + digest)
//!   cache/                             -- blob/registry cache
//!   {service}-{replica}/               -- per-container state
//!     rootfs/                          -- per-container rootfs (clone of base)
//!     console.log                      -- guest serial console
//!     config.json                      -- serialized ServiceSpec
//! ```

#![allow(unsafe_code)]

use crate::cgroups_stats::ContainerStats;
use crate::error::{AgentError, Result};
use crate::runtime::{
    ContainerId, ContainerState, LogChannel, LogChunk, LogsStream, LogsStreamOptions,
    OverlayAttachKind, Runtime, StatsSample, StatsStream,
};
use crate::runtimes::macos_vz_shared::{
    self, clamp_cpu_count, clamp_memory_bytes, clone_or_copy, file_url, read_vm_state,
    run_vm_lifecycle, spec_memory_mib, spec_vcpus, LiveVm, QueuePinned, VmLifecycleOp,
};
use std::collections::HashMap;
use std::io::Write as _;
use std::net::IpAddr;
use std::os::unix::io::FromRawFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use zlayer_observability::logs::{LogEntry, LogSource, LogStream};
use zlayer_spec::{RegistryAuth, ServiceSpec};
use zlayer_vzagent::proto;

use block2::RcBlock;
use dispatch2::DispatchQueue;
use objc2::rc::Retained;
use objc2::{AnyThread, ClassType};
use objc2_foundation::{NSArray, NSError, NSFileHandle, NSString};
use objc2_virtualization::{
    VZBootLoader, VZDirectoryShare, VZDirectorySharingDeviceConfiguration,
    VZEntropyDeviceConfiguration, VZFileHandleSerialPortAttachment, VZGenericPlatformConfiguration,
    VZLinuxBootLoader, VZMACAddress, VZNATNetworkDeviceAttachment, VZNetworkDeviceConfiguration,
    VZPlatformConfiguration, VZSharedDirectory, VZSingleDirectoryShare,
    VZSocketDeviceConfiguration, VZVirtioConsoleDeviceSerialPortConfiguration,
    VZVirtioEntropyDeviceConfiguration, VZVirtioFileSystemDeviceConfiguration,
    VZVirtioNetworkDeviceConfiguration, VZVirtioSocketConnection, VZVirtioSocketDevice,
    VZVirtioSocketDeviceConfiguration, VZVirtualMachine, VZVirtualMachineConfiguration,
    VZVirtualMachineState,
};

// ---------------------------------------------------------------------------
// Container record
// ---------------------------------------------------------------------------

/// The Linux kernel + initramfs artifacts a guest boots from.
///
/// Resolved by [`ensure_linux_kernel`]: either from the
/// `ZLAYER_VZ_LINUX_KERNEL` / `_INITRD` dev-override env vars or from the
/// on-disk cache.
#[derive(Clone, Debug)]
pub(crate) struct LinuxKernel {
    /// Path to the kernel `Image` (raw arm64 Linux kernel).
    pub(crate) image: PathBuf,
    /// Path to `initramfs.cpio.gz`.
    pub(crate) initramfs: PathBuf,
}

/// Captured result of the in-guest workload, shared between the blocking vsock
/// drain task and the runtime's async accessors.
///
/// The drain task owns the only writer side: it appends to `logs` as
/// `Stdout`/`Stderr` frames arrive, records the workload `pid` on `Started`,
/// and writes the terminal `state` (`Exited` / `Failed`) plus `exit_code` once
/// the connection produces an `Exited`/`Error` frame (or closes). All three
/// fields sit behind a `std::sync::Mutex` so the blocking task can update them
/// without an async runtime, while [`wait_container`](VzLinuxRuntime::wait_container),
/// `container_state`, and the log accessors read them from async context.
#[derive(Default)]
pub(crate) struct RunOutcome {
    /// Captured stdout/stderr log entries, in arrival order.
    logs: Mutex<Vec<LogEntry>>,
    /// PID of the workload reported by the guest's `Started` frame.
    pid: Mutex<Option<i32>>,
    /// The workload's exit code, set once `Exited` arrives.
    exit_code: Mutex<Option<i32>>,
    /// Terminal container state (`Exited`/`Failed`) once the workload finishes;
    /// `None` while the workload is still running.
    terminal: Mutex<Option<ContainerState>>,
}

/// Carried state for the `logs_stream` follow loop
/// ([`VzLinuxRuntime::logs_stream`]). Threaded through `stream::unfold` so each
/// poll can re-read the growing [`RunOutcome::logs`] buffer without borrowing
/// the runtime. `next` is the offset into that workload buffer already emitted;
/// `done` latches once the workload is terminal and the buffer is fully drained.
struct LogFollowState {
    containers: Arc<RwLock<HashMap<String, VzLinuxContainer>>>,
    dir_name: String,
    next: usize,
    done: bool,
}

/// Per-container state for a VZ Linux guest.
///
/// `kernel` is populated by [`start_container`](VzLinuxRuntime::start_container)
/// once [`ensure_linux_kernel`] resolves the guest kernel + initramfs.
pub(crate) struct VzLinuxContainer {
    /// Current container state.
    state: ContainerState,
    /// Per-container state directory.
    state_dir: PathBuf,
    /// Per-container rootfs directory (clone/reference of the image rootfs).
    #[allow(dead_code)]
    rootfs_dir: PathBuf,
    /// The shared image rootfs directory shared into the guest over virtiofs
    /// (read-only lower; the guest overlays a tmpfs upper).
    image_rootfs_dir: PathBuf,
    /// Resolved kernel `Image` + `initramfs.cpio.gz`, set at start.
    kernel: Option<LinuxKernel>,
    /// Guest serial console log.
    console_log: PathBuf,
    /// The VM's locally-administered MAC, used to find its DHCP lease (and
    /// pinned on the virtio-net device so the NAT DHCP lease resolves by MAC).
    mac: String,
    /// The original service spec.
    spec: ServiceSpec,
    /// The OCI image config (`Entrypoint`/`Cmd`/`Env`/`WorkingDir`/`User`) read
    /// from the `image-config.json` sidecar written at pull time. Merged with the
    /// spec by [`build_run_message`] so an image's default command/env/workdir/
    /// user actually take effect. `None` when the sidecar is absent (e.g. an old
    /// pull predating the sidecar) — the spec-only path then applies.
    image_config: Option<zlayer_registry::ImageConfig>,
    /// vCPUs the VM is configured with (stats reporting).
    vcpus: u32,
    /// RAM (MiB) the VM is configured with (stats reporting).
    #[allow(dead_code)]
    ram_mib: u32,
    /// When the VM was started.
    #[allow(dead_code)]
    started_at: Option<Instant>,
    /// The live VM + its queue, present only while running.
    live: Option<macos_vz_shared::LiveVm>,
    /// Captured workload output + exit code, written by the vsock drain task.
    outcome: Arc<RunOutcome>,
    /// Handle to the blocking vsock drain task, joined/aborted on stop/remove.
    drain_task: Option<JoinHandle<()>>,
    /// Handle to the `delete_on_exit` watcher task (spawned only when the spec's
    /// `lifecycle.delete_on_exit` is set). It waits for the workload to reach a
    /// terminal state, then tears the VM down and removes the container record +
    /// state dir. Aborted on `stop`/`remove` so an explicit teardown wins the
    /// race and the watcher can't double-free.
    cleanup_task: Option<JoinHandle<()>>,
    /// Handle to the periodic wall-clock resync task. Every
    /// [`SETTIME_RESYNC_INTERVAL`] it pushes a [`Msg::SetTime`](proto::Msg::SetTime)
    /// to the guest so a long-lived VM's clock doesn't drift after host sleeps
    /// (a VZ guest has no RTC and never resyncs on its own). Tied to VM lifetime:
    /// aborted on `stop`/`remove` (same as the drain/cleanup tasks) so it exits
    /// cleanly when the VM stops and never outlives the live VM it pushes to.
    settime_task: Option<JoinHandle<()>>,
    /// Host loopback forwarders for the container's published/exposed ports.
    /// Each binds `127.0.0.1:<container_port>` and tunnels every connection over
    /// a fresh vsock `Forward` to the guest agent. Tracked so `stop`/`remove`
    /// can abort them and so `port_mappings_container` can report the bindings.
    port_forwards: Vec<PortForward>,
    /// Sender half of the interactive-stdin channel, present only while the VM
    /// is running. The receiver half is owned by the `run_and_drain` blocking
    /// drain task, which forwards each chunk to the guest as a `Msg::Stdin`
    /// frame. [`Runtime::write_stdin`] sends host terminal bytes here;
    /// [`Runtime::close_stdin`] drops it so the drain task's `recv()` errs and
    /// it emits a final `Msg::StdinEof`. `None` before `start_container` wires
    /// the channel (and after `close_stdin` clears it).
    stdin_tx: Option<std::sync::mpsc::Sender<Vec<u8>>>,
    /// The container's overlay (`WireGuard`) address inside the cross-node mesh,
    /// set once `attach_overlay` has pushed a `Msg::OverlayConfig` into the guest
    /// and the in-guest `zl-overlay0` interface is up. `get_container_ip` prefers
    /// this over the NAT lease so service-mesh routing/DNS uses the overlay IP.
    /// `None` until the overlay is attached (or when the runtime has no overlay).
    overlay_ip: Option<IpAddr>,
}

/// A spawned host-port forwarder for a published container port.
///
/// VZ NAT establishes no usable guest lease on this macOS, so there is no guest
/// IP to dial. Instead each forwarder binds a host loopback TCP listener on
/// `127.0.0.1:container_port` and, for every accepted connection, opens a NEW
/// vsock connection to the guest agent ([`proto::CONTROL_PORT`]), writes a
/// [`proto::Msg::Forward`] frame, and then transparently pipes raw bytes between
/// the host TCP connection and the vsock connection. The guest agent connects to
/// `127.0.0.1:container_port` inside the guest and splices the bytes (see
/// `zlayer-vzagent`'s `serve_forward`). The host port equals the container port,
/// so `127.0.0.1:<container_port>` on the host reaches the same service the
/// workload listens on inside the guest.
struct PortForward {
    /// The container port forwarded; also the host loopback port bound.
    container_port: u16,
    /// Transport protocol (currently only TCP forwards are spawned).
    protocol: zlayer_spec::PortProtocol,
    /// The listener+tunnel task; aborted on teardown.
    task: JoinHandle<()>,
}

// ---------------------------------------------------------------------------
// VzLinuxRuntime
// ---------------------------------------------------------------------------

/// macOS Apple-Virtualization runtime for **Linux** guests.
pub struct VzLinuxRuntime {
    /// Base data directory (`{data_dir}/vz/linux` lives under this).
    data_dir: PathBuf,
    /// Directory for VM logs.
    #[allow(dead_code)]
    log_dir: PathBuf,
    /// Active containers keyed by `"{service}-{replica}"`.
    containers: Arc<RwLock<HashMap<String, VzLinuxContainer>>>,
    /// Pulled image rootfs paths keyed by sanitized image name.
    image_rootfs: Arc<RwLock<HashMap<String, PathBuf>>>,
}

impl std::fmt::Debug for VzLinuxRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VzLinuxRuntime")
            .field("data_dir", &self.data_dir)
            .field("log_dir", &self.log_dir)
            .finish_non_exhaustive()
    }
}

impl VzLinuxRuntime {
    /// Create a new VZ Linux-guest runtime, provisioning its directory
    /// hierarchy.
    ///
    /// VZ is a first-party framework (no dylib to load), so construction always
    /// succeeds on macOS as long as the directories can be created. The
    /// `com.apple.security.virtualization` entitlement is enforced later, at VM
    /// boot, not here.
    ///
    /// # Errors
    /// Returns an error only if the required directories cannot be created.
    pub fn new(_auth: Option<crate::runtime::ContainerAuthContext>) -> Result<Self> {
        let data_dir = zlayer_paths::ZLayerDirs::default_data_dir();
        let log_dir = zlayer_paths::ZLayerDirs::default_log_dir();
        let linux_dir = data_dir.join("vz").join("linux");

        for d in [
            &linux_dir,
            &linux_dir.join("images"),
            &linux_dir.join("cache"),
            &log_dir,
        ] {
            std::fs::create_dir_all(d).map_err(|e| {
                AgentError::Configuration(format!("Failed to create {}: {e}", d.display()))
            })?;
        }

        tracing::info!(
            vz_linux_dir = %linux_dir.display(),
            "macOS VZ Linux-guest runtime ready (default Linux path on macOS; \
             VM boot lands in a later phase)"
        );

        Ok(Self {
            data_dir,
            log_dir,
            containers: Arc::new(RwLock::new(HashMap::new())),
            image_rootfs: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    /// Resolve OCI pull auth for `image`.
    ///
    /// When the spec supplies auth, honor it via [`zlayer_registry::spec_auth_to_oci`]
    /// (inline / daemon-resolved credentials). When it does NOT, fall back to the
    /// hostname-based [`zlayer_core::AuthResolver`] whose default
    /// [`zlayer_core::AuthConfig`] reads `~/.docker/config.json` (`DockerConfig`) —
    /// matching youki's behavior so `zlayer login`-written credentials are picked
    /// up here too instead of degrading to anonymous. The resolver is built
    /// per-pull (cheap) so freshly-written docker-config creds are seen
    /// immediately by an already-running daemon.
    fn resolve_pull_auth(
        auth: Option<&RegistryAuth>,
        image: &str,
    ) -> zlayer_registry::RegistryAuth {
        match auth {
            Some(a) => zlayer_registry::spec_auth_to_oci(Some(a)),
            None => {
                zlayer_core::AuthResolver::new(zlayer_core::AuthConfig::default()).resolve(image)
            }
        }
    }

    /// `"{service}-{replica}"` — the per-container directory / map key.
    fn container_dir_name(id: &ContainerId) -> String {
        format!("{}-{}", id.service, id.replica)
    }

    /// A stable, nonzero pseudo-PID for a VZ container.
    ///
    /// A VM has no host-visible PID, but the overlay-attach contract wants a
    /// `Some(pid)` so PID-gated bookkeeping runs (see [`get_container_pid`]). We
    /// hash the container id into the upper-but-still-positive-`i32` range
    /// (`[2^30, 2^31)`, bit 30 set) so it can never collide with a real low
    /// host PID (`pid_max` is a few million), is guaranteed nonzero, AND still
    /// fits in a signed 32-bit integer. The latter matters because Docker /
    /// docker-compat clients (e.g. the `ZArcRunner` SDK) deserialize `pid` as an
    /// `int32`: a value with bit 31 set overflows that and breaks the client.
    ///
    /// [`get_container_pid`]: Runtime::get_container_pid
    fn pseudo_pid(id: &ContainerId) -> u32 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        id.service.hash(&mut h);
        id.replica.hash(&mut h);
        // Take the low 30 bits (always fits in u32, so try_from can't fail),
        // then set bit 30 so it lands in [2^30, 2^31): high enough to never
        // collide with a real host PID, nonzero, and within positive i32.
        u32::try_from(h.finish() & 0x3fff_ffff).unwrap_or(0) | 0x4000_0000
    }

    /// `{data_dir}/vz/linux` — the runtime's base directory.
    fn linux_dir(&self) -> PathBuf {
        self.data_dir.join("vz").join("linux")
    }

    /// `{data_dir}/vz/linux/images` — shared image rootfs store.
    fn images_dir(&self) -> PathBuf {
        self.linux_dir().join("images")
    }

    /// Find an already-extracted, populated rootfs for `image`, trying the
    /// literal key first and then every candidate spelling.
    ///
    /// The store key is always the literal user ref (see [`sanitize_image_name`]),
    /// but an image may have been extracted under an equivalent spelling on an
    /// earlier pull (e.g. `docker.io/library/alpine:latest` vs `alpine:latest`).
    /// Rather than rewrite keys to a canonical form (which would silently bind
    /// bare names to docker.io), we probe each candidate spelling at lookup
    /// time via [`zlayer_types::image_ref_candidates`] and return the first
    /// directory whose `rootfs` is populated. Returns `None` when nothing is
    /// present (so the caller pulls).
    fn resolve_existing_rootfs(&self, image: &str) -> Option<PathBuf> {
        let images_dir = self.images_dir();
        // 1. Literal user ref, exactly as typed.
        let literal = images_dir.join(sanitize_image_name(image)).join("rootfs");
        if rootfs_is_populated(&literal) {
            return Some(literal);
        }
        // 2. Candidate spellings (name + reference recombined into `name:ref`).
        for (name, reference) in zlayer_types::image_ref_candidates(image) {
            let spelling = format!("{name}:{reference}");
            let rootfs = images_dir
                .join(sanitize_image_name(&spelling))
                .join("rootfs");
            if rootfs_is_populated(&rootfs) {
                return Some(rootfs);
            }
        }
        None
    }

    /// `{data_dir}/vz/linux/cache` — blob/registry cache.
    #[allow(dead_code)]
    fn cache_dir(&self) -> PathBuf {
        self.linux_dir().join("cache")
    }

    /// Open the daemon-wide local OCI registry at `{data_dir}/registry` — the
    /// store `zlayer import` / `zlayer build` write into. Opened lazily (just
    /// loads `index.json`) so the runtime constructor can stay synchronous.
    ///
    /// Returns `None` (with a warning) if the registry cannot be opened, so a
    /// missing local store degrades to the cache/S3/origin chain rather than
    /// hard-failing the pull.
    async fn open_local_registry(&self) -> Option<std::sync::Arc<zlayer_registry::LocalRegistry>> {
        let registry_path = self.data_dir.join("registry");
        match zlayer_registry::LocalRegistry::new(registry_path).await {
            Ok(reg) => Some(std::sync::Arc::new(reg)),
            Err(e) => {
                tracing::warn!(error = %e, "vz-linux: failed to open local registry; \
                                            imported/built images will be invisible");
                None
            }
        }
    }

    /// Backfill the `image-config.json` sidecar for an ALREADY-present image.
    ///
    /// `pull_image_with_policy` writes the sidecar on a fresh pull, but an
    /// `IfNotPresent`/`Never` pull short-circuits when the rootfs already exists
    /// — so an image extracted before this fix shipped (or by a path that didn't
    /// write the sidecar) would never gain it, and `create_container` would fall
    /// back to running `/bin/true`. This ensures the sidecar exists for the
    /// resolved `rootfs` (its parent dir): if already present, it's a no-op;
    /// otherwise it fetches the OCI config (the config blob is typically already
    /// in the local `blobs.redb` from the original pull, so this is offline) and
    /// writes it. Best-effort — failures are logged inside the writer.
    async fn ensure_image_config_sidecar(
        &self,
        image: &str,
        rootfs: &std::path::Path,
        auth: Option<&RegistryAuth>,
        source: zlayer_spec::SourcePolicy,
    ) {
        let Some(image_dir) = rootfs.parent() else {
            return;
        };
        if image_dir.join("image-config.json").exists() {
            return;
        }
        let cache_path = self.images_dir().join("blobs.redb");
        let Ok(blob_cache) = zlayer_registry::CacheType::persistent_at(&cache_path)
            .build()
            .await
        else {
            tracing::debug!(image = %image, "vz-linux: sidecar backfill skipped (blob cache open failed)");
            return;
        };
        let mut puller =
            zlayer_registry::ImagePuller::from_env_for_runtime(blob_cache, source).await;
        if let Some(reg) = self.open_local_registry().await {
            puller = puller.with_local_registry(reg);
        }
        let pull_auth = Self::resolve_pull_auth(auth, image);
        write_image_config_sidecar(&puller, image, &pull_auth, image_dir).await;
    }

    /// `{data_dir}/vz/linux/kernel` — guest kernel + initramfs cache.
    fn kernel_cache_dir(&self) -> PathBuf {
        self.linux_dir().join("kernel")
    }

    /// Resolve the Linux guest kernel `Image` + `initramfs.cpio.gz`.
    ///
    /// Resolution order:
    /// 1. The `ZLAYER_VZ_LINUX_KERNEL` + `ZLAYER_VZ_LINUX_INITRD` env vars
    ///    (the working dev override — both must be set and exist). Checked FIRST
    ///    and short-circuits before any network so a local build is always
    ///    honored.
    /// 2. A normally-pulled OCI image, [`VZ_LINUX_BUNDLE_IMAGE`] (rolling
    ///    `arm64`, `FROM scratch`, members `vz-linux/Image`,
    ///    `vz-linux/initramfs.cpio.gz`, `vz-linux/manifest.json`). Pulled through
    ///    the SAME image machinery as every other image
    ///    ([`zlayer_registry::ImagePuller`] over the persistent `blobs.redb` blob
    ///    cache, with the default hostname-based [`zlayer_core::AuthResolver`] —
    ///    anonymous GHCR works once the package is public). The pulled manifest
    ///    digest (read back from the blob cache under
    ///    [`zlayer_registry::manifest_digest_cache_key`]) is compared against a
    ///    `digest` file in [`kernel_cache_dir`](Self::kernel_cache_dir): on a
    ///    mismatch (or when the cached artifacts are missing) the new layers are
    ///    unpacked and `Image` + `initramfs.cpio.gz` are atomically installed
    ///    into the cache (write-to-`.tmp` + rename), `manifest.json` is copied
    ///    alongside, and the new digest is recorded; on a match the existing
    ///    cache is reused as-is.
    /// 3. On a pull FAILURE (offline / 401 / registry down) the existing on-disk
    ///    cache is served with a warning when present (it may be stale); only
    ///    when NO cache exists does this fail, naming the env-override and
    ///    `images/vz-linux/build.sh` escape hatches plus the GHCR ref.
    ///
    /// # Errors
    /// Returns [`AgentError::Configuration`] when neither the env override, a
    /// successful pull/refresh, nor a pre-existing cache yields both artifacts.
    async fn ensure_linux_kernel(&self) -> Result<LinuxKernel> {
        // 1. Dev override via env. Checked FIRST and unchanged: a local build
        //    always wins and we never touch the network.
        let env_kernel = std::env::var_os("ZLAYER_VZ_LINUX_KERNEL").map(PathBuf::from);
        let env_initrd = std::env::var_os("ZLAYER_VZ_LINUX_INITRD").map(PathBuf::from);
        if let (Some(image), Some(initramfs)) = (env_kernel, env_initrd) {
            if image.exists() && initramfs.exists() {
                tracing::info!(
                    kernel = %image.display(),
                    initramfs = %initramfs.display(),
                    "vz-linux: using kernel from ZLAYER_VZ_LINUX_KERNEL/_INITRD override"
                );
                return Ok(LinuxKernel { image, initramfs });
            }
            tracing::warn!(
                "vz-linux: ZLAYER_VZ_LINUX_KERNEL/_INITRD set but a path does not exist; \
                 falling back to the pulled bundle / on-disk cache"
            );
        }

        let cache = self.kernel_cache_dir();
        let image = cache.join("Image");
        let initramfs = cache.join("initramfs.cpio.gz");

        // 2. Pull/refresh the published bundle through the normal image machinery.
        match self.pull_kernel_bundle(&cache).await {
            Ok(kernel) => return Ok(kernel),
            Err(e) => {
                // 3. Pull failed: serve a pre-existing cache (possibly stale) if
                //    present, otherwise fall through to the clear error below.
                if image.exists() && initramfs.exists() {
                    tracing::warn!(
                        kernel = %image.display(),
                        initramfs = %initramfs.display(),
                        error = %e,
                        bundle = VZ_LINUX_BUNDLE_IMAGE,
                        "vz-linux: registry unreachable; using cached kernel bundle \
                         (which may be stale)"
                    );
                    return Ok(LinuxKernel { image, initramfs });
                }
                tracing::warn!(
                    error = %e,
                    bundle = VZ_LINUX_BUNDLE_IMAGE,
                    "vz-linux: kernel-bundle pull failed and no cache present"
                );
            }
        }

        Err(AgentError::Configuration(format!(
            "vz-linux kernel artifact not found; the bundle image {bundle} could not be pulled \
             (the GHCR package must be public) — set ZLAYER_VZ_LINUX_KERNEL/_INITRD or run \
             images/vz-linux/build.sh and place Image+initramfs.cpio.gz in {cache}",
            bundle = VZ_LINUX_BUNDLE_IMAGE,
            cache = cache.display(),
        )))
    }

    /// Pull (or refresh) [`VZ_LINUX_BUNDLE_IMAGE`] and install its
    /// `Image` + `initramfs.cpio.gz` (+ `manifest.json`) into `cache_dir`.
    ///
    /// Pulls through the persistent `blobs.redb` blob cache with the default
    /// hostname-based auth resolver (anonymous GHCR), reads the resulting
    /// manifest digest back out of the blob cache, and — only when that digest
    /// differs from the recorded one or the cached files are missing — unpacks
    /// the layer(s) to a temp dir and atomically installs the two artifacts plus
    /// the manifest. A digest match is a no-op that reuses the cache as-is.
    ///
    /// # Errors
    /// Returns [`AgentError::PullFailed`] when the registry pull fails (so the
    /// caller can fall back to a stale cache), and [`AgentError::Configuration`]
    /// when the pull succeeds but the extracted bundle is missing an artifact.
    // Sequential pull/digest-check/unpack/atomic-install pipeline; the stages
    // share a pile of intermediate paths and the cache-hit early return, so
    // extracting helpers would obscure the flow more than it shortens it.
    #[allow(clippy::too_many_lines)]
    async fn pull_kernel_bundle(&self, cache_dir: &std::path::Path) -> Result<LinuxKernel> {
        let image = VZ_LINUX_BUNDLE_IMAGE;

        tokio::fs::create_dir_all(cache_dir)
            .await
            .map_err(|e| AgentError::PullFailed {
                image: image.to_string(),
                reason: format!("create kernel cache dir: {e}"),
            })?;

        // Persistent blob cache — same convention this runtime uses for every
        // other image pull (`{data_dir}/vz/linux/images/blobs.redb`). Held as a
        // cloneable Arc so we can read the manifest digest back out after the
        // pull (`from_env_for_runtime` takes its own clone of the Arc).
        let cache_path = self.images_dir().join("blobs.redb");
        let blob_cache = zlayer_registry::CacheType::persistent_at(&cache_path)
            .build()
            .await
            .map_err(|e| AgentError::PullFailed {
                image: image.to_string(),
                reason: format!("open blob cache: {e}"),
            })?;

        // Default hostname-based auth (DockerConfig → ~/.docker/config.json,
        // degrading to anonymous): the published GHCR package is public.
        let pull_auth = Self::resolve_pull_auth(None, image);

        // `Newer` so a rolling tag is revalidated against the origin each boot.
        let mut puller = zlayer_registry::ImagePuller::from_env_for_runtime(
            Arc::clone(&blob_cache),
            zlayer_spec::SourcePolicy::default(),
        )
        .await;
        if let Some(reg) = self.open_local_registry().await {
            puller = puller.with_local_registry(reg);
        }

        let layers =
            puller
                .pull_image(image, &pull_auth)
                .await
                .map_err(|e| AgentError::PullFailed {
                    image: image.to_string(),
                    reason: format!("pull kernel bundle layers: {e}"),
                })?;

        // The registry digest is cached under `manifest_digest_cache_key` by the
        // pull (same convention youki's `list_images` reads). Read it back so we
        // can decide whether the on-disk cache is already current.
        let digest_key = zlayer_registry::manifest_digest_cache_key(image);
        let remote_digest = blob_cache
            .get(&digest_key)
            .await
            .ok()
            .flatten()
            .and_then(|bytes| String::from_utf8(bytes).ok());

        let kernel_path = cache_dir.join("Image");
        let initrd_path = cache_dir.join("initramfs.cpio.gz");
        let digest_path = cache_dir.join("digest");

        let cached_digest = tokio::fs::read_to_string(&digest_path)
            .await
            .ok()
            .map(|s| s.trim().to_string());

        let need_extract = kernel_bundle_needs_extract(
            remote_digest.as_deref(),
            cached_digest.as_deref(),
            kernel_path.exists(),
            initrd_path.exists(),
        );

        if !need_extract {
            tracing::info!(
                kernel = %kernel_path.display(),
                initramfs = %initrd_path.display(),
                digest = remote_digest.as_deref().unwrap_or("<unknown>"),
                "vz-linux: kernel bundle already current; using on-disk cache"
            );
            return Ok(LinuxKernel {
                image: kernel_path,
                initramfs: initrd_path,
            });
        }

        tracing::info!(
            image = %image,
            layer_count = layers.len(),
            digest = remote_digest.as_deref().unwrap_or("<unknown>"),
            "vz-linux: extracting kernel bundle layers"
        );

        // Unpack into a scratch dir under the cache so the atomic renames stay on
        // the same filesystem. Cleared first to avoid stale members.
        let staging = cache_dir.join(".extract.tmp");
        let _ = tokio::fs::remove_dir_all(&staging).await;
        let mut unpacker = zlayer_registry::LayerUnpacker::new(staging.clone());
        unpacker
            .unpack_layers(&layers)
            .await
            .map_err(|e| AgentError::PullFailed {
                image: image.to_string(),
                reason: format!("extract kernel bundle: {e}"),
            })?;

        let src_kernel = staging.join("vz-linux").join("Image");
        let src_initrd = staging.join("vz-linux").join("initramfs.cpio.gz");
        let src_manifest = staging.join("vz-linux").join("manifest.json");

        if !src_kernel.exists() || !src_initrd.exists() {
            let _ = tokio::fs::remove_dir_all(&staging).await;
            return Err(AgentError::Configuration(format!(
                "vz-linux bundle image {image} is missing vz-linux/Image or \
                 vz-linux/initramfs.cpio.gz after extraction"
            )));
        }

        // Atomically install each artifact: copy to a `.tmp` sibling, then rename
        // over the live name (rename within a dir is atomic on the same fs).
        install_atomically(&src_kernel, &kernel_path).await?;
        install_atomically(&src_initrd, &initrd_path).await?;
        if src_manifest.exists() {
            // manifest.json is informational; a copy failure must not abort boot.
            let manifest_path = cache_dir.join("manifest.json");
            if let Err(e) = install_atomically(&src_manifest, &manifest_path).await {
                tracing::debug!(error = %e, "vz-linux: failed to install bundle manifest.json (non-fatal)");
            }
        }

        // Record the digest LAST so a crash mid-install never marks the cache
        // current. Skipped when the registry gave us no digest (e.g. an
        // anonymous pull served purely from cache) — the files are installed
        // either way; the next boot just re-checks.
        if let Some(digest) = remote_digest.as_deref() {
            let tmp_digest = cache_dir.join("digest.tmp");
            tokio::fs::write(&tmp_digest, digest.as_bytes())
                .await
                .map_err(|e| AgentError::PullFailed {
                    image: image.to_string(),
                    reason: format!("write kernel digest: {e}"),
                })?;
            tokio::fs::rename(&tmp_digest, &digest_path)
                .await
                .map_err(|e| AgentError::PullFailed {
                    image: image.to_string(),
                    reason: format!("install kernel digest: {e}"),
                })?;
        }

        let _ = tokio::fs::remove_dir_all(&staging).await;

        tracing::info!(
            kernel = %kernel_path.display(),
            initramfs = %initrd_path.display(),
            digest = remote_digest.as_deref().unwrap_or("<unknown>"),
            "vz-linux: kernel bundle pulled and installed"
        );
        Ok(LinuxKernel {
            image: kernel_path,
            initramfs: initrd_path,
        })
    }

    /// `{data_dir}/vz/linux/{service}-{replica}/` — per-container state.
    fn vm_dir(&self, id: &ContainerId) -> PathBuf {
        self.linux_dir().join(Self::container_dir_name(id))
    }

    /// Read the tail of a container's serial console log as [`LogEntry`]s.
    /// Returns an empty Vec when the container or its log is absent.
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

    /// Return up to `tail` of a container's captured logs.
    ///
    /// Merges two sources, in order:
    /// 1. The guest **serial console** (`console.log`) — kernel + boot output,
    ///    read via [`read_console`](Self::read_console). Useful for diagnosing a
    ///    guest that never reached the vsock agent.
    /// 2. The **workload** stdout/stderr captured by the vsock drain task in the
    ///    container's [`RunOutcome::logs`] buffer — these are the
    ///    `docker run ... echo hello`-style lines callers actually want.
    ///
    /// The `tail` cap is applied to the merged sequence so a caller asking for
    /// the last N lines still gets the most recent workload output.
    async fn captured_logs(&self, id: &ContainerId, tail: usize) -> Result<Vec<LogEntry>> {
        let dir_name = Self::container_dir_name(id);
        let mut merged = self.read_console(id, tail).await.unwrap_or_default();
        let outcome = {
            let guard = self.containers.read().await;
            guard.get(&dir_name).map(|c| Arc::clone(&c.outcome))
        };
        if let Some(outcome) = outcome {
            if let Ok(buf) = outcome.logs.lock() {
                merged.extend(buf.iter().cloned());
            }
        }
        let start = merged.len().saturating_sub(tail);
        Ok(merged.split_off(start))
    }

    /// Spawn a host loopback forwarder per **distinct published TCP container
    /// port** the spec declares, tracking each on the container record for
    /// teardown.
    ///
    /// Every forwarder binds `127.0.0.1:<container_port>` and tunnels each
    /// accepted connection over a fresh vsock `Forward` to the guest agent (which
    /// dials `127.0.0.1:<container_port>` inside the guest). The host port is the
    /// container port: there is no separate "host port" to pin because the guest
    /// has no routable NAT IP — loopback-on-host *is* the reachability path.
    ///
    /// UDP mappings are skipped (only TCP tunnels are spawned).
    async fn spawn_port_forwards(&self, dir_name: &str) {
        // Collect distinct TCP container ports from the spec's port mappings.
        let (ports, live): (Vec<(u16, zlayer_spec::PortProtocol)>, Option<LiveVm>) = {
            let guard = self.containers.read().await;
            let Some(c) = guard.get(dir_name) else {
                return;
            };
            let mut seen = std::collections::HashSet::new();
            let ports = c
                .spec
                .port_mappings
                .iter()
                .filter(|m| m.protocol == zlayer_spec::PortProtocol::Tcp)
                .filter_map(|m| {
                    if seen.insert(m.container_port) {
                        Some((m.container_port, m.protocol))
                    } else {
                        None
                    }
                })
                .collect();
            let live = c.live.as_ref().map(|l| LiveVm {
                queue: l.queue.clone(),
                vm: Arc::clone(&l.vm),
            });
            (ports, live)
        };
        let Some(live) = live else {
            return;
        };
        if ports.is_empty() {
            return;
        }
        let mut forwards = Vec::with_capacity(ports.len());
        for (container_port, protocol) in ports {
            let containers = Arc::clone(&self.containers);
            let dir = dir_name.to_string();
            let live_for_task = LiveVm {
                queue: live.queue.clone(),
                vm: Arc::clone(&live.vm),
            };
            let task = tokio::spawn(async move {
                run_port_forward(containers, dir, live_for_task, container_port).await;
            });
            forwards.push(PortForward {
                container_port,
                protocol,
                task,
            });
        }
        let mut guard = self.containers.write().await;
        if let Some(c) = guard.get_mut(dir_name) {
            c.port_forwards = forwards;
        } else {
            // Container vanished between snapshot and write: don't leak tasks.
            for f in forwards {
                f.task.abort();
            }
        }
    }
}

/// Run one host loopback forwarder that tunnels `127.0.0.1:<container_port>`
/// (on the host) into the guest over vsock.
///
/// Binds the host loopback listener, then for every accepted connection opens a
/// **fresh** vsock connection to the guest agent, writes a
/// [`proto::Msg::Forward`] frame naming `container_port`, and bidirectionally
/// copies raw bytes between the accepted host TCP connection and the vsock
/// connection (the guest splices it to `127.0.0.1:<container_port>` inside the
/// guest). Exits when the container disappears or its VM is torn down.
/// Best-effort: bind/accept/tunnel errors are logged, not fatal.
async fn run_port_forward(
    containers: Arc<RwLock<HashMap<String, VzLinuxContainer>>>,
    dir_name: String,
    live: LiveVm,
    container_port: u16,
) {
    use std::net::{Ipv4Addr, SocketAddr};
    use tokio::net::TcpListener;

    let bind_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), container_port);
    let listener = match TcpListener::bind(bind_addr).await {
        Ok(l) => l,
        Err(e) => {
            tracing::warn!(
                container = %dir_name,
                container_port,
                error = %e,
                "vz-linux: failed to bind loopback forwarder"
            );
            return;
        }
    };
    tracing::info!(
        container = %dir_name,
        %bind_addr,
        container_port,
        "vz-linux: loopback->vsock forwarder up"
    );

    loop {
        // Stop if the container/VM went away.
        {
            let guard = containers.read().await;
            if guard.get(&dir_name).is_none_or(|c| c.live.is_none()) {
                return;
            }
        }
        let accept = tokio::time::timeout(Duration::from_secs(1), listener.accept()).await;
        let (inbound, _peer) = match accept {
            Ok(Ok(conn)) => conn,
            Ok(Err(e)) => {
                tracing::debug!(container = %dir_name, error = %e, "vz-linux: forward accept error");
                continue;
            }
            // Timeout: loop back to re-check liveness.
            Err(_) => continue,
        };

        // Convert the accepted tokio connection into a blocking std stream so the
        // whole tunnel runs on a blocking thread alongside the blocking
        // `connect_vsock`/`UnixStream` IO.
        let inbound_std = match inbound.into_std() {
            Ok(s) => s,
            Err(e) => {
                tracing::debug!(container = %dir_name, error = %e, "vz-linux: into_std failed");
                continue;
            }
        };
        let live_for_conn = LiveVm {
            queue: live.queue.clone(),
            vm: Arc::clone(&live.vm),
        };
        let dir = dir_name.clone();
        tokio::task::spawn_blocking(move || {
            // The tokio stream was non-blocking; restore blocking mode for the
            // synchronous copy loops below.
            if let Err(e) = inbound_std.set_nonblocking(false) {
                tracing::debug!(container = %dir, error = %e, "vz-linux: set_blocking failed");
                return;
            }
            tunnel_connection_over_vsock(&live_for_conn, inbound_std, container_port, &dir);
        });
    }
}

/// Open a fresh vsock connection to the guest agent, send a
/// [`proto::Msg::Forward`] frame for `container_port`, and bidirectionally pipe
/// raw bytes between the host TCP connection and the vsock connection until
/// either side reaches EOF/error. Runs on a blocking thread.
fn tunnel_connection_over_vsock(
    live: &LiveVm,
    inbound: std::net::TcpStream,
    container_port: u16,
    dir_name: &str,
) {
    let mut vsock = match connect_vsock(live) {
        Ok(s) => s,
        Err(e) => {
            tracing::debug!(container = %dir_name, error = %e, "vz-linux: forward vsock connect failed");
            return;
        }
    };
    // Frame the guest into transparent-tunnel mode for this connection.
    if let Err(e) = proto::write_frame(
        &mut vsock,
        &proto::Msg::Forward {
            port: container_port,
        },
    ) {
        tracing::debug!(container = %dir_name, error = %e, "vz-linux: forward send Forward failed");
        return;
    }

    // Independent owned halves for each direction. Both an `UnixStream` and a
    // `TcpStream` can be cloned to split read/write across threads.
    let (mut vsock_r, mut vsock_w) = match vsock.try_clone() {
        Ok(clone) => (vsock, clone),
        Err(e) => {
            tracing::debug!(container = %dir_name, error = %e, "vz-linux: vsock clone failed");
            return;
        }
    };
    let (mut tcp_r, mut tcp_w) = match inbound.try_clone() {
        Ok(clone) => (inbound, clone),
        Err(e) => {
            tracing::debug!(container = %dir_name, error = %e, "vz-linux: tcp clone failed");
            return;
        }
    };

    // host -> guest on a helper thread; guest -> host on this thread.
    let h = std::thread::spawn(move || {
        let _ = std::io::copy(&mut tcp_r, &mut vsock_w);
        // Half-close the vsock write side so the guest's splice sees EOF.
        let _ = vsock_w.shutdown(std::net::Shutdown::Write);
    });
    let _ = std::io::copy(&mut vsock_r, &mut tcp_w);
    let _ = tcp_w.shutdown(std::net::Shutdown::Write);
    let _ = h.join();
}

// ---------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------

/// Sanitize an image reference for use as a filesystem directory name.
///
/// Keyed on the **literal** user reference — separators (`/ : @`) replaced
/// with `_`, and NOTHING else. NO canonicalization: `alpine:latest` becomes
/// `alpine_latest`, NOT `docker.io_library_alpine_latest`; `ghcr.io/o/r:1`
/// becomes `ghcr.io_o_r_1`. `ZLayer` never silently rewrites a bare name onto
/// docker.io, so the store key is exactly what the user typed.
///
/// The cross-spelling problem this used to "solve" by canonicalizing (a pull
/// stored under `docker.io_library_alpine_latest` while the deploy looked up
/// `alpine_latest` → empty share → guest `/bin/sh` ENOENT) is instead handled
/// at LOOKUP time by [`VzLinuxRuntime::resolve_existing_rootfs`], which tries
/// every candidate spelling without rewriting the key.
fn sanitize_image_name(image: &str) -> String {
    image.replace(['/', ':', '@'], "_")
}

/// Whether an extracted-image rootfs directory exists AND has at least one
/// entry. A bare-existing-but-empty `…/rootfs` is the residue of a pull that
/// created the directory and then failed (or was interrupted) before extracting
/// any layers; treating it as "present" would let an `IfNotPresent` pull
/// short-circuit forever and would share an empty filesystem into the guest
/// (no `/bin/sh`). So "present" requires non-empty AND the completion marker
/// written after a fully successful unpack — "non-empty" alone let a rootfs
/// truncated mid-extraction (observed: 1.3 MB of a 227 MB buildd image, no
/// `usr/`, container crash-looping on a missing entrypoint) be cached and
/// reused forever. A marker-less rootfs re-pulls; the layers are normally
/// still in the local blob cache, so the heal is a re-extract, not a
/// re-download.
fn rootfs_is_populated(rootfs_dir: &std::path::Path) -> bool {
    let non_empty = std::fs::read_dir(rootfs_dir).is_ok_and(|mut entries| entries.next().is_some());
    non_empty && rootfs_complete_marker(rootfs_dir).is_some_and(|m| m.exists())
}

/// Path of the unpack-completion marker for a given `…/<image>/rootfs` dir:
/// `…/<image>/rootfs.complete` (sibling, so it never ships into the guest).
/// `None` only for a rootless path (no parent), which never occurs for real
/// image dirs.
fn rootfs_complete_marker(rootfs_dir: &std::path::Path) -> Option<std::path::PathBuf> {
    rootfs_dir.parent().map(|p| p.join("rootfs.complete"))
}

/// Decide whether the VZ-Linux kernel bundle must be (re-)extracted into the
/// on-disk cache, given the freshly-pulled manifest `remote_digest`, the
/// `cached_digest` recorded alongside the last install, and whether the cached
/// `Image` / `initramfs.cpio.gz` files are present.
///
/// Pure (paths-resolved booleans + digest strings in, decision out) so the
/// refresh policy is unit-testable without a registry or filesystem:
/// - Missing either artifact ⇒ extract (the cache is incomplete).
/// - Both present AND the remote digest equals the recorded one ⇒ reuse.
/// - A differing digest ⇒ extract (the rolling tag moved).
/// - An UNKNOWN remote digest (anonymous/offline pull served from cache with no
///   digest sidecar) with both files present ⇒ reuse (don't churn what we have).
fn kernel_bundle_needs_extract(
    remote_digest: Option<&str>,
    cached_digest: Option<&str>,
    kernel_present: bool,
    initrd_present: bool,
) -> bool {
    if !kernel_present || !initrd_present {
        return true;
    }
    match (remote_digest, cached_digest) {
        // Remote digest known: extract only when it differs from the record
        // (a missing record counts as different).
        (Some(remote), cached) => cached != Some(remote),
        // No remote digest available but both files exist: reuse the cache.
        (None, _) => false,
    }
}

/// Atomically install `src` at `dst` by copying to a `dst.tmp` sibling and then
/// renaming over `dst` (a rename within one directory on the same filesystem is
/// atomic). Used to publish the kernel/initramfs into the cache without ever
/// exposing a half-written file to a concurrent boot.
async fn install_atomically(src: &std::path::Path, dst: &std::path::Path) -> Result<()> {
    let tmp = match dst.file_name() {
        Some(name) => {
            let mut t = name.to_os_string();
            t.push(".tmp");
            dst.with_file_name(t)
        }
        None => dst.with_extension("tmp"),
    };
    tokio::fs::copy(src, &tmp)
        .await
        .map_err(|e| AgentError::PullFailed {
            image: VZ_LINUX_BUNDLE_IMAGE.to_string(),
            reason: format!("stage {}: {e}", dst.display()),
        })?;
    tokio::fs::rename(&tmp, dst)
        .await
        .map_err(|e| AgentError::PullFailed {
            image: VZ_LINUX_BUNDLE_IMAGE.to_string(),
            reason: format!("install {}: {e}", dst.display()),
        })?;
    Ok(())
}

/// Pull the OCI image config and persist its runtime defaults
/// (Entrypoint/Cmd/Env/WorkingDir/User) as an `image-config.json` sidecar in
/// `image_dir` (the parent of the extracted `rootfs`).
///
/// `create_container` later reads this sidecar (via
/// [`read_image_config_sidecar`]) so [`build_run_message`] can apply the image's
/// DEFAULT command/env without a network round-trip. WITHOUT this the VZ guest
/// only ever saw the spec's command and fell back to `/bin/true` for
/// image-default images (postgres/forgejo), exiting instantly with no output.
///
/// Best-effort: a config-pull/serialize/write failure is logged, not fatal — the
/// layers are already extracted; the cost is only that the image defaults won't
/// apply on run (the spec command, if any, still does).
async fn write_image_config_sidecar(
    puller: &zlayer_registry::ImagePuller,
    image: &str,
    auth: &zlayer_registry::RegistryAuth,
    image_dir: &std::path::Path,
) {
    let config = match puller.pull_image_config(image, auth).await {
        Ok(c) => c,
        Err(e) => {
            tracing::debug!(
                image = %image,
                error = %e,
                "vz-linux: failed to fetch OCI config blob; the image-default \
                 command/env will not apply (dispatch uses its macOS default OS hint)",
            );
            return;
        }
    };
    let json = match serde_json::to_string_pretty(&config) {
        Ok(j) => j,
        Err(e) => {
            tracing::warn!(image = %image, error = %e, "vz-linux: failed to serialize image config sidecar");
            return;
        }
    };
    let sidecar = image_dir.join("image-config.json");
    if let Err(e) = tokio::fs::write(&sidecar, json).await {
        tracing::warn!(
            image = %image,
            error = %e,
            "vz-linux: failed to write image-config.json sidecar; image-default command/env will not apply on run",
        );
    }
}

/// Read the `image-config.json` sidecar that [`write_image_config_sidecar`]
/// writes next to a pulled image's rootfs, given that `…/rootfs` directory.
///
/// The sidecar lives at `{image_dir}/image-config.json` (the parent of the
/// `rootfs` dir). Returns the parsed [`zlayer_registry::ImageConfig`] so
/// [`build_run_message`] can apply the image's default Entrypoint/Cmd/Env/
/// WorkingDir/User. Returns `None` when the sidecar is absent (e.g. a pull that
/// predates the sidecar) or unparseable — the caller then runs the spec-only
/// path. A missing/unparseable sidecar is logged at debug, not fatal.
fn read_image_config_sidecar(rootfs_dir: &std::path::Path) -> Option<zlayer_registry::ImageConfig> {
    let sidecar = rootfs_dir.parent()?.join("image-config.json");
    let bytes = std::fs::read(&sidecar).ok()?;
    match serde_json::from_slice::<zlayer_registry::ImageConfig>(&bytes) {
        Ok(cfg) => Some(cfg),
        Err(e) => {
            tracing::debug!(
                sidecar = %sidecar.display(),
                error = %e,
                "vz-linux: failed to parse image-config.json sidecar; \
                 image-default command/env will not apply",
            );
            None
        }
    }
}

/// OCI image holding the published VZ-Linux guest kernel bundle.
///
/// CI publishes this rolling `arm64` image (FROM scratch) with exactly three
/// members under `/vz-linux/`: `Image` (the raw arm64 Linux kernel),
/// `initramfs.cpio.gz`, and `manifest.json`. The daemon pulls it through the
/// SAME image machinery the sandbox/youki runtimes use for every other image.
///
/// Used as a LITERAL reference everywhere (stored/looked up under exactly this
/// string per the codebase's no-canonicalization rule — see
/// [`sanitize_image_name`] / [`resolve_existing_rootfs`]).
const VZ_LINUX_BUNDLE_IMAGE: &str = "ghcr.io/blackleafdigital/zlayer/vz-linux:arm64";

/// Default Linux-guest kernel command line.
///
/// `console=hvc0` routes the kernel console to the virtio serial port wired to
/// `console.log`. `rootfstag=rootfs` is a marker the initramfs interprets:
/// Phase 3 will switch the root filesystem to a virtiofs share tagged `rootfs`
/// (the agent mounts virtiofs `rootfs` as `/`), replacing the placeholder
/// `root=/dev/vda` block-device path. Until then the initramfs runs from RAM.
const LINUX_CMDLINE: &str = "console=hvc0 rootfstag=rootfs rw";

/// Build the Linux-guest kernel command line, appending the host wall-clock
/// epoch as `zlayer.boottime=<unix_secs>` so the guest can seed its clock at
/// boot.
///
/// A VZ Linux guest has no RTC and starts at epoch 0 (1970), which breaks TLS
/// cert validity windows, build timestamps, and anything time-aware. The
/// in-guest `zlayer-vzagent` reads `zlayer.boottime=` from `/proc/cmdline` and
/// `settimeofday`s before serving. If the host clock is somehow unavailable
/// (a pre-1970 system clock — `duration_since(UNIX_EPOCH)` errs), we fall back
/// to the static [`LINUX_CMDLINE`] and let the guest start at epoch (a later
/// [`Msg::SetTime`](proto::Msg::SetTime) resync corrects it).
///
/// Factored out of [`build_config_linux`] (which constructs queue-affine VZ
/// objects and can't run off-macOS) so the cmdline string is unit-testable.
fn build_linux_cmdline(now: std::time::SystemTime) -> String {
    now.duration_since(std::time::UNIX_EPOCH).map_or_else(
        |_| LINUX_CMDLINE.to_string(),
        |d| format!("{LINUX_CMDLINE} zlayer.boottime={}", d.as_secs()),
    )
}

/// Virtiofs share tag the guest agent mounts as the container rootfs lower.
/// MUST match the `rootfstag=` value in [`LINUX_CMDLINE`] and the tag the
/// in-guest `zlayer-vzagent` mounts.
const VIRTIOFS_ROOTFS_TAG: &str = "rootfs";

/// How long to keep retrying the vsock connect to the guest agent. The agent
/// has to finish mounting virtiofs + DHCP before it `listen`s, so the first
/// connects will fail; we retry until the deadline.
const VSOCK_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Pause between vsock connect attempts.
const VSOCK_CONNECT_RETRY_INTERVAL: Duration = Duration::from_millis(250);

/// How often the per-VM background task re-pushes the host wall clock to the
/// guest via [`Msg::SetTime`](proto::Msg::SetTime). Apple Virtualization guests
/// have no RTC and never resync after a host sleep, so a long-lived VM drifts;
/// a periodic resync keeps x509 validity windows and time-aware workloads sane.
const SETTIME_RESYNC_INTERVAL: Duration = Duration::from_secs(300);

/// Round-trip a MAC string through `VZMACAddress` (parse then re-stringify),
/// returning the framework's canonical spelling or `None` when the string is
/// not a valid MAC.
///
/// This is exactly the parse `build_config_linux` performs to pin the virtio-net
/// device's address — exercising it here proves the per-container `mac` we
/// persist survives the `initWithString` -> `string()` round-trip the host-side
/// DHCP-lease lookup (keyed by that same MAC) depends on. Constructing a
/// `VZMACAddress` is a plain Obj-C allocation; it needs no VM / entitlement.
#[cfg(test)]
pub(crate) fn mac_roundtrip(mac: &str) -> Option<String> {
    let mac_str = NSString::from_str(mac);
    // SAFETY: `initWithString` is a pure Obj-C initializer over an NSString; it
    // allocates a value object and touches no VM / queue-affine state.
    let parsed = unsafe { VZMACAddress::initWithString(VZMACAddress::alloc(), &mac_str) }?;
    Some(unsafe { parsed.string() }.to_string())
}

/// The honest "not yet implemented" sentinel previously returned by the
/// VM-lifecycle methods before they were implemented (stop/remove/state/stats,
/// kill, pause/resume all now have real implementations). Retained for the
/// regression test that documents the sentinel's shape; `#[cfg(test)]` so it
/// is not dead code in a normal build.
#[cfg(test)]
fn not_yet(op: &str) -> AgentError {
    AgentError::Configuration(format!(
        "vz-linux: {op} not implemented until a later phase"
    ))
}

// ---------------------------------------------------------------------------
// VM configuration (all VZ calls happen on the per-VM queue)
// ---------------------------------------------------------------------------

/// A single host->guest bind mount exported over its own virtiofs share.
///
/// Derived from a [`zlayer_spec::StorageSpec::Bind`] entry on the container's
/// spec: `host` is the host source directory, `tag` is the unique virtiofs
/// device tag (`zlmnt{i}`), `target` is the in-guest mount point, and
/// `readonly` is the share's access mode. The host attaches one
/// `VZVirtioFileSystemDeviceConfiguration` per bind (in addition to the rootfs
/// share); the guest agent receives a matching [`proto::Msg::Mount`] frame
/// (same `tag`/`target`/`readonly`) and mounts the virtiofs share at `target`.
#[derive(Clone, Debug)]
pub(crate) struct BindMount {
    /// Host source directory shared into the guest.
    pub(crate) host: PathBuf,
    /// Unique virtiofs device tag (`zlmnt0`, `zlmnt1`, ...).
    pub(crate) tag: String,
    /// In-guest mount point the agent mounts this share at.
    pub(crate) target: String,
    /// Whether the share (and the guest mount) is read-only.
    pub(crate) readonly: bool,
}

/// Plain (`Send`) inputs for building a Linux-guest VM configuration on the
/// VM's serial dispatch queue.
#[derive(Clone)]
pub(crate) struct LinuxVmBuildInputs {
    /// Kernel `Image` + `initramfs.cpio.gz`.
    pub(crate) kernel: LinuxKernel,
    /// Guest serial console log (the kernel `console=hvc0` is wired here).
    pub(crate) console_log: PathBuf,
    /// The shared image rootfs directory exported into the guest over virtiofs
    /// (tag `rootfs`, read-only lower).
    pub(crate) image_rootfs_dir: PathBuf,
    /// vCPUs, already clamped to the framework + host range.
    pub(crate) cpu_count: usize,
    /// RAM in bytes, already clamped to the framework's allowed range.
    pub(crate) memory_bytes: u64,
    /// The VM's locally-administered MAC (string form, e.g. `"0a:1b:2c:.."`),
    /// round-tripped into a `VZMACAddress` so the host's `dhcpd_leases` lookup
    /// by MAC resolves the NAT IP the guest acquires.
    pub(crate) mac: String,
    /// Host bind mounts, one extra virtiofs share each (tag `zlmnt{i}`), in
    /// addition to the rootfs share. The matching [`proto::Msg::Mount`] frames
    /// are sent to the guest agent after `Run`.
    pub(crate) bind_mounts: Vec<BindMount>,
}

/// Build a [`VZVirtualMachineConfiguration`] for a **Linux** guest.
///
/// MUST be called on the VM's serial dispatch queue (every VZ object touched
/// here is queue-affine). Returns an error string on any FFI/validation
/// failure. Mirrors `macos_vz::VzContainer::build_configuration` but boots a
/// Linux kernel + initramfs via [`VZLinuxBootLoader`] on a generic platform —
/// no graphics, no aux storage, no machine identifier.
pub(crate) fn build_config_linux(
    inputs: &LinuxVmBuildInputs,
) -> std::result::Result<Retained<VZVirtualMachineConfiguration>, String> {
    // SAFETY: caller guarantees we run on the VM's serial dispatch queue, the
    // only place these queue-affine VZ objects may be constructed/mutated.
    unsafe {
        let config = VZVirtualMachineConfiguration::new();
        config.setCPUCount(inputs.cpu_count);
        config.setMemorySize(inputs.memory_bytes);

        // --- platform: generic (Linux), no machine id / aux storage ---
        let platform = VZGenericPlatformConfiguration::new();
        let platform_super: &VZPlatformConfiguration = platform.as_super();
        config.setPlatform(platform_super);

        // --- boot loader: Linux kernel Image + initramfs ---
        let kernel_url = file_url(&inputs.kernel.image);
        let linux_boot =
            VZLinuxBootLoader::initWithKernelURL(VZLinuxBootLoader::alloc(), &kernel_url);
        let initrd_url = file_url(&inputs.kernel.initramfs);
        linux_boot.setInitialRamdiskURL(Some(&initrd_url));
        // Inject the host wall-clock epoch so the guest can set its clock at
        // boot. A VZ Linux guest has no RTC and starts at epoch 0 (1970), which
        // breaks TLS cert validity, build timestamps, and anything time-aware.
        // The vzagent reads `zlayer.boottime=<unix_secs>` from /proc/cmdline and
        // `settimeofday`s before serving. `build_linux_cmdline` falls back to a
        // static cmdline if the clock is somehow unavailable.
        let cmdline_str = build_linux_cmdline(std::time::SystemTime::now());
        let cmdline = NSString::from_str(&cmdline_str);
        linux_boot.setCommandLine(&cmdline);
        let boot: Retained<VZBootLoader> = Retained::into_super(linux_boot);
        config.setBootLoader(Some(&boot));

        // --- serial console -> console.log (kernel console=hvc0) ---
        // Truncate/create the log and hand its fd to the guest's serial port.
        let _ = std::fs::File::create(&inputs.console_log);
        let log_str = NSString::from_str(&inputs.console_log.to_string_lossy());
        if let Some(write_fh) = NSFileHandle::fileHandleForWritingAtPath(&log_str) {
            let serial = VZVirtioConsoleDeviceSerialPortConfiguration::new();
            let attach =
                VZFileHandleSerialPortAttachment::initWithFileHandleForReading_fileHandleForWriting(
                    VZFileHandleSerialPortAttachment::alloc(),
                    None,
                    Some(&write_fh),
                );
            serial.setAttachment(Some(&attach));
            let serial_super = Retained::into_super(serial);
            let serial_arr = NSArray::from_retained_slice(&[serial_super]);
            config.setSerialPorts(&serial_arr);
        }

        // --- virtiofs: share the image rootfs read-only as tag `rootfs` ---
        // The in-guest agent mounts this share (tag `VIRTIOFS_ROOTFS_TAG`),
        // overlays a tmpfs upper, and `pivot_root`s onto it. Read-only lower is
        // correct: the per-container writable layer lives in the guest tmpfs, so
        // the shared image rootfs is never mutated and can be shared across
        // replicas.
        // Collect every directory-sharing device: the rootfs share first, then
        // one virtiofs device per host bind mount (tag `zlmnt{i}`).
        let mut fs_devices: Vec<Retained<VZDirectorySharingDeviceConfiguration>> = Vec::new();

        let tag = NSString::from_str(VIRTIOFS_ROOTFS_TAG);
        let fs = VZVirtioFileSystemDeviceConfiguration::initWithTag(
            VZVirtioFileSystemDeviceConfiguration::alloc(),
            &tag,
        );
        let rootfs_url = file_url(&inputs.image_rootfs_dir);
        let dir =
            VZSharedDirectory::initWithURL_readOnly(VZSharedDirectory::alloc(), &rootfs_url, true);
        let share =
            VZSingleDirectoryShare::initWithDirectory(VZSingleDirectoryShare::alloc(), &dir);
        // VZSingleDirectoryShare -> VZDirectoryShare upcast for setShare.
        let share_super: Retained<VZDirectoryShare> = Retained::into_super(share);
        fs.setShare(Some(&share_super));
        // VZVirtioFileSystemDeviceConfiguration -> VZDirectorySharingDeviceConfiguration.
        let fs_super: Retained<VZDirectorySharingDeviceConfiguration> = Retained::into_super(fs);
        fs_devices.push(fs_super);

        // --- virtiofs: one extra share per host bind mount ---
        // Each bind mount gets its own `VZVirtioFileSystemDeviceConfiguration`
        // tagged `zlmnt{i}` (matching the tag the guest agent receives in the
        // `Msg::Mount` frame) pointing at the host source dir, with the share's
        // `readOnly` taken from the mount. Mirrors the rootfs share's object
        // shape exactly, just per-bind tag/url/readonly.
        for bind in &inputs.bind_mounts {
            let bind_tag = NSString::from_str(&bind.tag);
            let bind_fs = VZVirtioFileSystemDeviceConfiguration::initWithTag(
                VZVirtioFileSystemDeviceConfiguration::alloc(),
                &bind_tag,
            );
            let bind_url = file_url(&bind.host);
            let bind_dir = VZSharedDirectory::initWithURL_readOnly(
                VZSharedDirectory::alloc(),
                &bind_url,
                bind.readonly,
            );
            let bind_share = VZSingleDirectoryShare::initWithDirectory(
                VZSingleDirectoryShare::alloc(),
                &bind_dir,
            );
            let bind_share_super: Retained<VZDirectoryShare> = Retained::into_super(bind_share);
            bind_fs.setShare(Some(&bind_share_super));
            let bind_fs_super: Retained<VZDirectorySharingDeviceConfiguration> =
                Retained::into_super(bind_fs);
            fs_devices.push(bind_fs_super);
        }

        let fs_arr = NSArray::from_retained_slice(&fs_devices);
        config.setDirectorySharingDevices(&fs_arr);

        // --- network: virtio-net + NAT, with the per-VM MAC ---
        // VZ's NAT attachment runs a userspace DHCP server that hands the guest a
        // `192.168.64.x` lease (the in-guest agent brings up eth0 via DHCP). We
        // pin the device's MAC to the container's recorded MAC so the host-side
        // `dhcpd_leases` lookup (keyed by MAC) resolves that NAT IP. The NAT
        // subnet is directly host-reachable, so `guest_ip:port` works with no
        // proxy. Mirrors `macos_vz::build_configuration`'s network device.
        let net = VZVirtioNetworkDeviceConfiguration::new();
        let nat = VZNATNetworkDeviceAttachment::new();
        net.setAttachment(Some(&nat));
        let mac_str = NSString::from_str(&inputs.mac);
        if let Some(mac) = VZMACAddress::initWithString(VZMACAddress::alloc(), &mac_str) {
            net.setMACAddress(&mac);
        }
        let net_super: Retained<VZNetworkDeviceConfiguration> = Retained::into_super(net);
        let net_arr = NSArray::from_retained_slice(&[net_super]);
        config.setNetworkDevices(&net_arr);

        // --- entropy: virtio-rng so the guest CSPRNG seeds promptly ---
        // A VZ guest has no hardware RNG; without a virtio entropy device the
        // kernel CSPRNG never seeds, so anything reading it (Go's crypto/rand,
        // TLS handshakes, sshd) blocks ~60s+ at startup ("crypto/rand: blocked
        // for 60 seconds waiting to read random data from the kernel"). Needs
        // CONFIG_HW_RANDOM_VIRTIO in the guest kernel.
        // SAFETY: standard VZ device construction on the config queue (already
        // inside this fn's `unsafe` block); the array is a valid NSArray of
        // entropy device configs.
        let entropy = VZVirtioEntropyDeviceConfiguration::new();
        let entropy_super: Retained<VZEntropyDeviceConfiguration> = Retained::into_super(entropy);
        let entropy_arr = NSArray::from_retained_slice(&[entropy_super]);
        config.setEntropyDevices(&entropy_arr);

        // --- vsock: virtio socket device for the host<->agent control channel ---
        // The guest agent listens on AF_VSOCK port `proto::CONTROL_PORT`; the
        // host connects to it after boot to drive the workload.
        let vsock = VZVirtioSocketDeviceConfiguration::new();
        let vsock_super: Retained<VZSocketDeviceConfiguration> = Retained::into_super(vsock);
        let vsock_arr = NSArray::from_retained_slice(&[vsock_super]);
        config.setSocketDevices(&vsock_arr);

        // NO graphics device (Linux headless). NO VZMacPlatform / aux / machine
        // id (those are macOS-guest only).

        // --- validate ---
        config
            .validateWithError()
            .map_err(|e| format!("configuration invalid: {}", e.localizedDescription()))?;

        Ok(config)
    }
}

// ---------------------------------------------------------------------------
// vsock connect + workload Run/drain (Phase 4)
// ---------------------------------------------------------------------------

/// Build the `Run` control message by merging a [`ServiceSpec`] with the OCI
/// **image config** (`image`), per Docker/OCI semantics.
///
/// - `argv` is the effective entrypoint+cmd resolved by
///   [`macos_vz_shared::resolve_entrypoint`]: spec overrides win, otherwise the
///   image's `Entrypoint`+`Cmd` are used (falling back to `["true"]` only when
///   neither side supplies a command). This is the fix for images like
///   `postgres:16-alpine`/`forgejo` that rely on their image ENTRYPOINT/CMD —
///   previously they ran `/bin/true` and exited instantly.
/// - `env` merges the image config's `Env` (PATH etc.) UNDER the spec's env
///   (spec/compose values win for the same key) via
///   [`macos_vz_shared::merge_env`].
/// - `cwd` is the spec workdir, falling back to the image's `WorkingDir`.
/// - `uid`/`gid` come from the spec's `user`, falling back to the image's
///   `User` (numeric `uid`/`uid:gid` honoured; a NAME form degrades to `0:0`).
pub(crate) fn build_run_message(
    spec: &ServiceSpec,
    image: Option<&zlayer_registry::ImageConfig>,
) -> proto::Msg {
    let argv = macos_vz_shared::resolve_entrypoint(spec, image);
    let env = macos_vz_shared::merge_env(spec, image);
    let cwd = macos_vz_shared::resolve_workdir(spec, image);
    let (uid, gid) = macos_vz_shared::resolve_user(spec, image);
    proto::Msg::Run {
        argv,
        env,
        cwd,
        uid,
        gid,
    }
}

/// Derive the host bind mounts from a [`ServiceSpec`]'s storage list.
///
/// Walks `spec.storage` for [`zlayer_spec::StorageSpec::Bind`] entries and maps
/// each to a [`BindMount`] with a sequential virtiofs tag `zlmnt{i}` (i in
/// declaration order). The same `tag`/`target`/`readonly` triple is used both
/// to attach the per-bind virtiofs device in [`build_config_linux`] and to emit
/// the matching [`proto::Msg::Mount`] frame the guest agent mounts — so this is
/// the single source of truth that keeps the two sides in lockstep. Non-`Bind`
/// storage kinds (named/anonymous/tmpfs/s3) are not host-dir virtiofs shares
/// and are skipped here.
pub(crate) fn derive_bind_mounts(spec: &ServiceSpec) -> Vec<BindMount> {
    spec.storage
        .iter()
        .filter_map(|s| match s {
            zlayer_spec::StorageSpec::Bind {
                source,
                target,
                readonly,
            } => Some((source.clone(), target.clone(), *readonly)),
            _ => None,
        })
        .enumerate()
        .map(|(i, (source, target, readonly))| BindMount {
            host: PathBuf::from(source),
            tag: format!("zlmnt{i}"),
            target,
            readonly,
        })
        .collect()
}

/// Connect to the guest agent's vsock control port, returning an owned,
/// blocking [`UnixStream`] over the connected socket fd.
///
/// VZ socket objects are queue-affine, so the `connectToPort` call is issued on
/// the VM's serial queue; its completion block bridges the connected fd (or an
/// error string) back over a `std::mpsc` channel. The block `dup`s the fd
/// because the `VZVirtioSocketConnection` object closes its fd when it is
/// released after the block returns — the `dup` gives us an independent owner
/// that outlives the connection object.
///
/// The agent only `listen`s once it has finished mounting virtiofs and brought
/// up DHCP, so early connects fail; we retry every
/// [`VSOCK_CONNECT_RETRY_INTERVAL`] until [`VSOCK_CONNECT_TIMEOUT`].
fn connect_vsock(live: &LiveVm) -> std::result::Result<UnixStream, String> {
    let deadline = Instant::now() + VSOCK_CONNECT_TIMEOUT;
    let mut last_err = "vsock connect never attempted".to_string();
    loop {
        if Instant::now() >= deadline {
            return Err(format!("vsock connect timed out: {last_err}"));
        }

        let (tx, rx) =
            std::sync::mpsc::channel::<std::result::Result<std::os::unix::io::RawFd, String>>();
        let vm = Arc::clone(&live.vm);
        live.queue.exec_async(move || {
            // The completion block fires on this same serial queue once the
            // connect resolves. It must own everything it touches.
            let block = RcBlock::new(
                move |conn: *mut VZVirtioSocketConnection, err: *mut NSError| {
                    if !err.is_null() {
                        // SAFETY: framework hands us a valid non-null NSError we
                        // only read; we do not take ownership.
                        let msg = unsafe { (*err).localizedDescription() }.to_string();
                        let _ = tx.send(Err(msg));
                        return;
                    }
                    if conn.is_null() {
                        let _ = tx.send(Err("vsock connect: null connection".to_string()));
                        return;
                    }
                    // SAFETY: non-null connected VZVirtioSocketConnection. Its
                    // `fileDescriptor` is a live SOCK_STREAM fd owned by the
                    // connection object; we `dup` it so our copy survives the
                    // connection object being released after this block returns.
                    let fd = unsafe { (*conn).fileDescriptor() };
                    let owned = unsafe { libc::dup(fd) };
                    if owned < 0 {
                        let e = std::io::Error::last_os_error();
                        let _ = tx.send(Err(format!("dup(vsock fd): {e}")));
                        return;
                    }
                    let _ = tx.send(Ok(owned));
                },
            );
            // SAFETY: we are on the VM's serial queue — the only place its
            // socket devices may be touched. Index 0 is the single vsock device
            // configured in `build_config_linux`; downcast to the concrete
            // virtio socket device exposing `connectToPort`.
            unsafe {
                let devices = vm.0.socketDevices();
                if devices.count() == 0 {
                    let _ = block; // drop the unused completion block
                    return;
                }
                let dev = devices.objectAtIndex(0);
                if let Ok(vsock) = dev.downcast::<VZVirtioSocketDevice>() {
                    vsock.connectToPort_completionHandler(proto::CONTROL_PORT, &block);
                } else {
                    // Not a virtio socket device; block dropped unused.
                }
            }
        });

        match rx.recv() {
            Ok(Ok(fd)) => {
                // SAFETY: `fd` is an owned, connected SOCK_STREAM fd from `dup`;
                // UnixStream takes ownership and closes it on drop. A connected
                // AF_VSOCK stream fd is read/write-compatible with UnixStream.
                return Ok(unsafe { UnixStream::from_raw_fd(fd) });
            }
            Ok(Err(e)) => last_err = e,
            Err(_) => last_err = "vsock connect channel closed".to_string(),
        }
        std::thread::sleep(VSOCK_CONNECT_RETRY_INTERVAL);
    }
}

/// Append a captured workload line to the in-memory log buffer **and** mirror it
/// into `console.log`, so both `captured_logs` readers and on-disk diagnostics
/// see the workload's output.
fn capture_chunk(
    outcome: &RunOutcome,
    console_log: &std::path::Path,
    dir_name: &str,
    stream: LogStream,
    bytes: &[u8],
) {
    let message = String::from_utf8_lossy(bytes).into_owned();
    // Mirror to console.log (best-effort).
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(console_log)
    {
        let _ = f.write_all(bytes);
    }
    if let Ok(mut buf) = outcome.logs.lock() {
        buf.push(LogEntry {
            timestamp: chrono::Utc::now(),
            stream,
            source: LogSource::Container(dir_name.to_string()),
            message,
            service: None,
            deployment: None,
        });
    }
}

/// Connect to the guest agent, send the `Run` message, then drain frames until
/// the workload exits (or the connection ends), recording stdout/stderr, the
/// pid, and the final exit code/state on the shared [`RunOutcome`].
///
/// Runs on a blocking thread (NOT the VZ queue): all frame IO is synchronous
/// `UnixStream` read/write via the `proto` framing helpers.
#[allow(clippy::too_many_lines)]
fn run_and_drain(
    live: &LiveVm,
    run_msg: &proto::Msg,
    bind_mounts: &[BindMount],
    stdin_rx: Option<std::sync::mpsc::Receiver<Vec<u8>>>,
    outcome: &RunOutcome,
    console_log: &std::path::Path,
    dir_name: &str,
) {
    let mut stream = match connect_vsock(live) {
        Ok(s) => s,
        Err(e) => {
            if let Ok(mut t) = outcome.terminal.lock() {
                *t = Some(ContainerState::Failed {
                    reason: format!("vsock connect failed: {e}"),
                });
            }
            tracing::warn!(container = %dir_name, error = %e, "vz-linux: vsock connect failed");
            return;
        }
    };

    if let Err(e) = proto::write_frame(&mut stream, run_msg) {
        if let Ok(mut t) = outcome.terminal.lock() {
            *t = Some(ContainerState::Failed {
                reason: format!("send Run failed: {e}"),
            });
        }
        tracing::warn!(container = %dir_name, error = %e, "vz-linux: failed to send Run");
        return;
    }

    // Immediately after `Run`, ask the guest agent to mount each host bind mount
    // (one virtiofs share per `tag`, attached in `build_config_linux`) at its
    // `target`. The tag/target/readonly triple is the SAME one used to attach
    // the share, so the guest can pair the frame to its device. A failed Mount
    // frame is logged (best-effort) but does not abort the workload drain — the
    // workload may not need the mount, and the guest surfaces a hard mount error
    // over its own `Error`/`Stderr` channel.
    for bind in bind_mounts {
        if let Err(e) = proto::write_frame(
            &mut stream,
            &proto::Msg::Mount {
                tag: bind.tag.clone(),
                target: bind.target.clone(),
                readonly: bind.readonly,
            },
        ) {
            tracing::warn!(
                container = %dir_name,
                tag = %bind.tag,
                target = %bind.target,
                error = %e,
                "vz-linux: failed to send Mount frame"
            );
        }
    }

    // STDIN seam: when a stdin source is wired (the interactive `-it`
    // CLI->daemon->vsock pipeline passes `Some(rx)` from `start_container`),
    // forward each chunk to the guest as a `Msg::Stdin` frame and a final
    // `Msg::StdinEof` once the channel closes, on a helper thread so it runs
    // concurrently with the drain loop below. `close_stdin` (and container
    // teardown) drop the sender half, which ends `rx.recv()` and emits the
    // EOF. Non-interactive callers may still pass `None`, in which case this
    // spawns nothing. A second writer half is cloned so the drain loop keeps
    // reading on `stream`.
    // Held for the function's duration (detached): the helper thread exits on
    // its own when the channel closes or the workload's drain returns and the
    // stream is dropped. Named with a leading underscore so it stays bound to
    // end-of-scope (a bare `_` would drop and join it immediately).
    let _stdin_handle = stdin_rx.and_then(|rx| match stream.try_clone() {
        Ok(mut stdin_w) => Some(std::thread::spawn(move || {
            while let Ok(chunk) = rx.recv() {
                if proto::write_frame(&mut stdin_w, &proto::Msg::Stdin(chunk)).is_err() {
                    return;
                }
            }
            let _ = proto::write_frame(&mut stdin_w, &proto::Msg::StdinEof);
        })),
        Err(e) => {
            tracing::debug!(error = %e, "vz-linux: stdin stream clone failed; stdin disabled");
            None
        }
    });

    loop {
        match proto::read_frame(&mut stream) {
            Ok(proto::Msg::Stdout(b)) => {
                capture_chunk(outcome, console_log, dir_name, LogStream::Stdout, &b);
            }
            Ok(proto::Msg::Stderr(b)) => {
                capture_chunk(outcome, console_log, dir_name, LogStream::Stderr, &b);
            }
            Ok(proto::Msg::Started { pid }) => {
                if let Ok(mut p) = outcome.pid.lock() {
                    *p = Some(pid);
                }
                tracing::debug!(container = %dir_name, pid, "vz-linux: workload started");
            }
            Ok(proto::Msg::Exited { code }) => {
                record_exit(outcome, code);
                tracing::debug!(container = %dir_name, code, "vz-linux: workload exited");
                return;
            }
            Ok(proto::Msg::Error { message }) => {
                if let Ok(mut t) = outcome.terminal.lock() {
                    *t = Some(ContainerState::Failed {
                        reason: message.clone(),
                    });
                }
                tracing::warn!(container = %dir_name, error = %message, "vz-linux: agent error");
                return;
            }
            // Host-only message variants are never sent guest->host; ignore.
            Ok(other) => {
                tracing::trace!(container = %dir_name, tag = other.tag(), "vz-linux: unexpected frame");
            }
            Err(e) => {
                // The stream closed (workload gone) or a transport error. If we
                // never saw an explicit Exited, record a clean-ish terminal so
                // waiters don't hang; preserve any already-recorded outcome.
                let already = outcome
                    .terminal
                    .lock()
                    .ok()
                    .and_then(|g| g.clone())
                    .is_some();
                if !already {
                    if let Ok(mut t) = outcome.terminal.lock() {
                        *t = Some(ContainerState::Exited { code: 0 });
                    }
                    if let Ok(mut c) = outcome.exit_code.lock() {
                        c.get_or_insert(0);
                    }
                }
                tracing::debug!(container = %dir_name, error = %e, "vz-linux: drain stream ended");
                return;
            }
        }
    }
}

/// Tear a VZ-Linux container down completely and free its resources: remove the
/// record from the shared `containers` map (taking ownership of its VM + every
/// spawned task), abort the drain + port-forward tasks, force the VM down +
/// release it (closing the vsock device + its fds), and delete the per-container
/// state directory (rootfs/overlay + console.log + config.json).
///
/// This is the shared teardown used by both the `delete_on_exit` watcher
/// ([`watch_for_auto_remove`]) and (logically) [`VzLinuxRuntime::remove_container`].
/// Removing an unknown container is a no-op. The `cleanup_task` handle is NOT
/// aborted here (a watcher calling this would be aborting itself); callers that
/// hold the watcher handle abort it separately.
async fn teardown_container(
    containers: &Arc<RwLock<HashMap<String, VzLinuxContainer>>>,
    dir_name: &str,
) {
    let (state_dir, live, drain, forwards) = {
        let mut guard = containers.write().await;
        match guard.remove(dir_name) {
            Some(c) => (Some(c.state_dir), c.live, c.drain_task, c.port_forwards),
            None => return,
        }
    };
    // Abort the drain task FIRST so its blocking vsock read can't race the VM
    // teardown below.
    if let Some(drain) = drain {
        drain.abort();
    }
    // Drop network state: stop all host loopback forwarders.
    for f in forwards {
        f.task.abort();
    }
    // Ensure the VM is fully stopped + released before deleting its state dir.
    if let Some(live) = live {
        let _ = tokio::task::spawn_blocking(move || {
            let r = run_vm_lifecycle(&live, VmLifecycleOp::Stop);
            drop(live);
            r
        })
        .await;
    }
    if let Some(dir) = state_dir {
        let _ = tokio::fs::remove_dir_all(dir).await;
    }
}

/// `delete_on_exit` watcher: poll the container's shared [`RunOutcome`] until the
/// workload reaches a terminal state (an `Exited`/`Failed`/`Error` frame, or the
/// drain stream ending), then [`teardown_container`] it — the VZ-Linux analog of
/// Docker `--rm`. Spawned by [`start_container`](VzLinuxRuntime::start_container)
/// only when the spec's `lifecycle.delete_on_exit` is set.
///
/// The VM does not power itself off when the workload exits, so without this the
/// VM + its tasks + state dir would leak until an explicit `remove`. The watcher
/// is aborted by `stop`/`remove` (which take the handle out of the record and
/// `abort()` it) so an explicit teardown wins the race and the watcher can never
/// double-free a record another path already removed.
async fn watch_for_auto_remove(
    containers: Arc<RwLock<HashMap<String, VzLinuxContainer>>>,
    dir_name: String,
) {
    loop {
        // The container vanished (explicit remove won the race): nothing to do.
        let outcome = {
            let guard = containers.read().await;
            match guard.get(&dir_name) {
                Some(c) => Arc::clone(&c.outcome),
                None => return,
            }
        };
        let terminal = outcome
            .terminal
            .lock()
            .ok()
            .and_then(|g| g.clone())
            .is_some();
        let exited = outcome.exit_code.lock().ok().and_then(|g| *g).is_some();
        if terminal || exited {
            tracing::info!(
                container = %dir_name,
                "vz-linux: delete_on_exit — workload terminal, removing container"
            );
            teardown_container(&containers, &dir_name).await;
            return;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Connect a SECOND vsock control connection to the guest agent, send an
/// `Exec` message, then drain frames until the exec'd process exits, returning
/// `(exit_code, stdout, stderr)`.
///
/// This mirrors [`run_and_drain`] but for the one-shot `exec` path: instead of
/// writing captured output to the shared [`RunOutcome`], it accumulates
/// `Stdout`/`Stderr` bytes into local buffers and returns the captured
/// `Exited` code. A `Msg::Error` frame (the guest reporting that the exec
/// spawn failed) is surfaced as an `Err`.
///
/// Runs on a blocking thread (NOT the VZ queue): the `connectToPort` call is
/// internally hopped onto the VM's serial queue by [`connect_vsock`], but all
/// frame IO here is synchronous `UnixStream` read/write.
///
/// The guest accepts control connections on the same [`proto::CONTROL_PORT`]
/// and dispatches `Msg::Exec` by entering the running workload's PID namespace
/// (`setns` on `/proc/{pid}/ns/pid`); the mount/net namespaces are already
/// shared, so the exec'd process sees the container filesystem. See
/// `zlayer-vzagent`'s `spawn_exec`.
fn exec_and_collect(
    live: &LiveVm,
    exec_msg: &proto::Msg,
) -> std::result::Result<(i32, String, String), String> {
    let mut stream = connect_vsock(live)?;
    proto::write_frame(&mut stream, exec_msg).map_err(|e| format!("send Exec failed: {e}"))?;

    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    loop {
        match proto::read_frame(&mut stream) {
            Ok(proto::Msg::Stdout(b)) => stdout.extend_from_slice(&b),
            Ok(proto::Msg::Stderr(b)) => stderr.extend_from_slice(&b),
            Ok(proto::Msg::Exited { code }) => {
                return Ok((
                    code,
                    String::from_utf8_lossy(&stdout).into_owned(),
                    String::from_utf8_lossy(&stderr).into_owned(),
                ));
            }
            Ok(proto::Msg::Error { message }) => {
                return Err(format!("guest exec error: {message}"));
            }
            // `Started` carries no host-side state for exec, and host-only
            // variants are never sent guest->host; ignore both.
            Ok(_) => {}
            Err(proto::ProtoError::Io(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                // The connection closed before an explicit `Exited`. Surface
                // whatever output we captured with a conventional "killed"
                // code rather than hanging or inventing a success.
                return Ok((
                    -1,
                    String::from_utf8_lossy(&stdout).into_owned(),
                    String::from_utf8_lossy(&stderr).into_owned(),
                ));
            }
            Err(e) => return Err(format!("exec drain failed: {e}")),
        }
    }
}

/// Parse a Docker `--user` value into the wire fields for [`proto::Msg::Exec`].
///
/// Accepts:
/// * `""` / `None` → `(None, None, None)` (keep the guest's identity / root).
/// * `uid` (numeric) → `(Some(uid), None, None)`; the guest mirrors gid = uid.
/// * `uid:gid` (both numeric) → `(Some(uid), Some(gid), None)`.
/// * `name` or `name:group` containing a non-numeric component → deferred to
///   the guest via `user = Some(raw)`, which resolves it against the
///   container's `/etc/passwd` / `/etc/group` after pivot. We still surface any
///   numeric half so a `1000:git` style request keeps the numeric uid.
fn parse_exec_user(user: Option<&str>) -> (Option<u32>, Option<u32>, Option<String>) {
    let Some(raw) = user.map(str::trim).filter(|u| !u.is_empty()) else {
        return (None, None, None);
    };
    let (u, g) = match raw.split_once(':') {
        Some((u, g)) => (u, Some(g)),
        None => (raw, None),
    };
    let uid = u.parse::<u32>().ok();
    let gid = g.and_then(|g| g.parse::<u32>().ok());
    // If either half is a non-numeric name, hand the raw string to the guest so
    // it can resolve names against the container's passwd/group databases.
    let needs_name_resolution = uid.is_none() || (g.is_some() && gid.is_none());
    let user_name = if needs_name_resolution {
        Some(raw.to_string())
    } else {
        None
    };
    (uid, gid, user_name)
}

/// Map a canonical POSIX signal name (as produced by
/// [`crate::runtime::validate_signal`], e.g. `"SIGTERM"`) to its numeric value
/// on Linux (the guest is always Linux, so the Linux numbering is authoritative
/// regardless of the macOS host's own `libc` constants). Unknown names fall back
/// to `SIGKILL` (9) so a kill is always forceful rather than silently dropped.
///
/// Only the signals [`crate::runtime::validate_signal`] accepts are mapped; the
/// numbers are the Linux/glibc `asm-generic` values, which match every common
/// Linux architecture (arm64/x86-64) the guest runs on.
pub(crate) fn signal_number(name: &str) -> i32 {
    match name.trim().to_ascii_uppercase().as_str() {
        "SIGHUP" => 1,
        "SIGINT" => 2,
        "SIGUSR1" => 10,
        "SIGUSR2" => 12,
        "SIGTERM" => 15,
        // SIGKILL and anything unrecognised: force-kill.
        _ => 9,
    }
}

/// Open a fresh vsock control connection to the guest agent and send a single
/// [`proto::Msg::Signal`] frame, returning once the frame is written.
///
/// Used by graceful [`stop_container`](VzLinuxRuntime::stop_container) (SIGTERM)
/// and [`kill_container`](VzLinuxRuntime::kill_container) (mapped signal). Runs
/// on a blocking thread: [`connect_vsock`] hops the connect onto the VM's serial
/// queue, but the frame write is synchronous `UnixStream` IO. The stream is
/// dropped (closing its fd) as soon as the frame is sent — the agent's primary
/// `Run` connection is the one that reports the resulting `Exited`.
fn signal_agent(live: &LiveVm, signum: i32) -> std::result::Result<(), String> {
    let mut stream = connect_vsock(live)?;
    proto::write_frame(&mut stream, &proto::Msg::Signal { signum })
        .map_err(|e| format!("send Signal({signum}) failed: {e}"))?;
    // Drop `stream` here: the connection's only purpose was to deliver the
    // signal; the workload's exit is observed on the primary drain connection.
    Ok(())
}

/// Open a fresh vsock control connection and send a single
/// [`proto::Msg::OverlayConfig`] frame so the guest brings up its in-guest
/// `WireGuard` overlay interface (`zl-overlay0`).
///
/// Unlike [`signal_agent`] (which can write-and-drop because a missed signal is
/// retried), this keeps the connection open until the guest has read AND
/// processed the frame: after writing, we half-close our write end and read to
/// EOF. The guest's control loop reads the `OverlayConfig`, applies it, then its
/// next read sees our EOF, breaks, and closes — which unblocks our read. This
/// avoids racing the connection close against the guest's accept/read: a bare
/// write+drop sent immediately after `Run` can drop the queued frame if the
/// guest hasn't accepted the connection yet. A read timeout bounds the wait so a
/// misbehaving guest can't wedge the (blocking) caller task. Any reply frame the
/// guest sends back (e.g. a `Msg::Error` on apply failure) is drained here.
fn push_overlay_agent(live: &LiveVm, msg: &proto::Msg) -> std::result::Result<bool, String> {
    use std::io::Read as _;
    let mut stream = connect_vsock(live)?;
    proto::write_frame(&mut stream, msg).map_err(|e| format!("send OverlayConfig failed: {e}"))?;
    // Bound the post-write wait; the guest typically closes within a few hundred
    // ms (read frame -> apply -> next read hits EOF -> close). Returns whether we
    // observed the guest close the connection (EOF) — i.e. it actually consumed
    // the frame — vs. a timeout/reset (the frame may not have been processed).
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = stream.shutdown(std::net::Shutdown::Write);
    let mut sink = [0u8; 256];
    let mut saw_eof = false;
    loop {
        match stream.read(&mut sink) {
            Ok(0) => {
                // Guest closed: it finished reading + applying.
                saw_eof = true;
                break;
            }
            Ok(_) => {}      // drain any reply frame bytes, keep reading
            Err(_) => break, // timeout / reset: don't wedge the caller
        }
    }
    Ok(saw_eof)
}

/// Send a [`Msg::SetTime`](proto::Msg::SetTime) to a live guest agent, seeding
/// the guest wall clock to the host's current epoch seconds.
///
/// Mirrors [`push_overlay_agent`]: opens a fresh vsock control connection to the
/// guest, writes the single frame, then drains until EOF (bounded by a 5s read
/// timeout) so we can report whether the guest actually consumed the frame
/// (`Ok(true)`) vs. a timeout/reset (`Ok(false)`).
///
/// Fire-and-forget by design — a VZ guest has no RTC and never resyncs its
/// clock after the host sleeps, so the host re-pushes the wall clock on resume
/// and on a periodic tick. Callers treat any failure as warn-level and keep
/// going; the next tick retries.
///
/// **Stale-guest safety:** an old guest binary built before tag 14 existed runs
/// a [`proto::decode`] whose range check rejected tag 14
/// ([`proto::ProtoError::UnknownTag(14)`](proto::ProtoError::UnknownTag)). On
/// that path the guest logs the unknown tag and closes the connection without
/// applying anything — it never crashes. Host-side we only *write* the frame
/// here, so the worst case is the guest dropping it and us observing a reset
/// (`Ok(false)`) or a write error (returned as `Err`, logged warn by the
/// caller). Either way the host is never wedged or aborted by an out-of-range
/// tag at the guest.
fn push_settime(live: &LiveVm) -> std::result::Result<bool, String> {
    use std::io::Read as _;
    let unix_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| {
            // `as` on a u64 -> i64: epoch seconds won't overflow i64 until the
            // year ~292 billion, so this is lossless for any real wall clock.
            i64::try_from(d.as_secs()).unwrap_or(i64::MAX)
        })
        .map_err(|e| format!("host clock before UNIX_EPOCH: {e}"))?;
    let msg = proto::Msg::SetTime { unix_secs };
    let mut stream = connect_vsock(live)?;
    proto::write_frame(&mut stream, &msg).map_err(|e| format!("send SetTime failed: {e}"))?;
    // Bound the post-write wait like `push_overlay_agent`: the guest reads the
    // frame, applies `settimeofday`, then its next read hits EOF and it closes.
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = stream.shutdown(std::net::Shutdown::Write);
    let mut sink = [0u8; 256];
    let mut saw_eof = false;
    loop {
        match stream.read(&mut sink) {
            Ok(0) => {
                saw_eof = true;
                break;
            }
            Ok(_) => {}      // drain any reply bytes, keep reading
            Err(_) => break, // timeout / reset: don't wedge the caller
        }
    }
    Ok(saw_eof)
}

/// Derive the VZ NAT gateway for a guest from its leased address: the `.1` of
/// its IPv4 `/24`. VZ NAT is a `192.168.64.0/24` with the host at `.1`, but
/// deriving it from the lease keeps us correct if the subnet ever differs. IPv6
/// (not used by VZ NAT) falls back to the standard gateway string.
fn nat_gateway_for(ip: IpAddr) -> String {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            format!("{}.{}.{}.1", o[0], o[1], o[2])
        }
        IpAddr::V6(_) => "192.168.64.1".to_string(),
    }
}

/// Rewrite the host portion of a `host:port` `WireGuard` endpoint to `gateway`,
/// preserving the port. An empty endpoint (a roaming peer) is left as-is, and an
/// unparseable endpoint is returned unchanged.
fn rewrite_endpoint_host(endpoint: &str, gateway: &str) -> String {
    if endpoint.is_empty() {
        return String::new();
    }
    match endpoint.rsplit_once(':') {
        Some((_host, port)) => format!("{gateway}:{port}"),
        None => endpoint.to_string(),
    }
}

/// Resolve the authoritative terminal [`ContainerState`] from a workload's
/// captured outcome, given an optional `fallback` for when nothing terminal has
/// been recorded yet (e.g. the live VM's reconciled state, or a forced-stop
/// default).
///
/// This is the single mapping from a drained [`RunOutcome`] to the state that
/// `container_state` / Docker `inspect`'s `State.ExitCode` reads. It is the
/// reconciliation point for the `wait` vs `state` split: `wait_container` reads
/// `RunOutcome::exit_code` directly, so this MUST prefer that same captured code
/// rather than a stale record state or a fabricated `0`.
///
/// Precedence:
/// 1. A recorded `terminal` state (`Exited`/`Failed` written by the drain task
///    on an `Exited`/`Error` frame) — the richest signal, kept verbatim.
/// 2. A captured `exit_code` (the real workload code `wait_container` returns),
///    surfaced as `Exited { code }`. This covers the race where the drain task
///    set `exit_code` but `container_state` was consulted before `terminal` was
///    written, or where only the code is known.
/// 3. The caller's `fallback` (the live-VM reconciliation, or `Running` /
///    `Exited { code: 0 }` defaults). Only used when the workload's outcome is
///    entirely unknown — never to override a captured non-zero code.
///
/// Without step 2 the workload's real exit code (e.g. `42`) was dropped on the
/// state path: `wait` returned `42` from `exit_code` while `container_state`
/// fabricated `Exited { code: 0 }`.
/// Record a workload's real exit `code` onto the shared [`RunOutcome`] when the
/// drain task reads an `Exited { code }` frame.
///
/// Writes BOTH the captured `exit_code` (what [`wait_container`](VzLinuxRuntime::wait_container)
/// returns) and the richer `terminal` state (what [`container_state`](VzLinuxRuntime::container_state)
/// reads) so the wait and state paths stay in lockstep on the same code. Kept as
/// a tiny pure helper so the "frame -> stored outcome" mapping is unit-testable
/// without a live VM.
fn record_exit(outcome: &RunOutcome, code: i32) {
    if let Ok(mut c) = outcome.exit_code.lock() {
        *c = Some(code);
    }
    if let Ok(mut t) = outcome.terminal.lock() {
        *t = Some(ContainerState::Exited { code });
    }
}

fn outcome_to_state(
    exit_code: Option<i32>,
    terminal: Option<ContainerState>,
    fallback: ContainerState,
) -> ContainerState {
    if let Some(terminal) = terminal {
        return terminal;
    }
    if let Some(code) = exit_code {
        return ContainerState::Exited { code };
    }
    fallback
}

// ---------------------------------------------------------------------------
// Runtime trait implementation
// ---------------------------------------------------------------------------

#[async_trait::async_trait]
impl Runtime for VzLinuxRuntime {
    async fn pull_image(&self, image: &str) -> Result<()> {
        self.pull_image_with_policy(
            image,
            zlayer_spec::PullPolicy::IfNotPresent,
            None,
            zlayer_spec::SourcePolicy::default(),
        )
        .await
    }

    #[allow(clippy::too_many_lines)]
    async fn pull_image_with_policy(
        &self,
        image: &str,
        policy: zlayer_spec::PullPolicy,
        auth: Option<&RegistryAuth>,
        source: zlayer_spec::SourcePolicy,
    ) -> Result<()> {
        // The store key is always the LITERAL user ref (no canonicalization).
        let safe_name = sanitize_image_name(image);
        let image_dir = self.images_dir().join(&safe_name);
        let rootfs_dir = image_dir.join("rootfs");

        match policy {
            zlayer_spec::PullPolicy::Always | zlayer_spec::PullPolicy::Newer => {
                // Always re-pull; drift detection happens at the service layer.
            }
            zlayer_spec::PullPolicy::IfNotPresent => {
                // "Present" requires a NON-EMPTY rootfs (a bare-existing-but-empty
                // dir is the residue of a failed pull and must NOT short-circuit,
                // or the empty share is shared into the guest forever — no
                // `/bin/sh`). Probe the literal key AND candidate spellings so an
                // image extracted under an equivalent spelling is reused instead
                // of needlessly re-pulled, WITHOUT rewriting the key.
                if let Some(existing) = self.resolve_existing_rootfs(image) {
                    tracing::debug!(
                        image = %image,
                        rootfs = %existing.display(),
                        "vz-linux image already present; skipping pull"
                    );
                    self.ensure_image_config_sidecar(image, &existing, auth, source)
                        .await;
                    self.image_rootfs.write().await.insert(safe_name, existing);
                    return Ok(());
                }
            }
            zlayer_spec::PullPolicy::Never => {
                if let Some(existing) = self.resolve_existing_rootfs(image) {
                    self.ensure_image_config_sidecar(image, &existing, auth, source)
                        .await;
                    self.image_rootfs.write().await.insert(safe_name, existing);
                    return Ok(());
                }
                return Err(AgentError::PullFailed {
                    image: image.to_string(),
                    reason: "image not present (or its rootfs is empty) and pull policy is Never"
                        .to_string(),
                });
            }
        }

        tracing::info!(image = %image, "pulling image for vz-linux runtime");

        tokio::fs::create_dir_all(&rootfs_dir)
            .await
            .map_err(|e| AgentError::PullFailed {
                image: image.to_string(),
                reason: format!("create rootfs dir: {e}"),
            })?;

        let cache_path = self.images_dir().join("blobs.redb");
        let blob_cache = zlayer_registry::CacheType::persistent_at(&cache_path)
            .build()
            .await
            .map_err(|e| AgentError::PullFailed {
                image: image.to_string(),
                reason: format!("open blob cache: {e}"),
            })?;

        // Resolve auth from the caller (the daemon/composite resolves per-registry
        // credentials — inline spec creds or, via the credential store, by host —
        // and passes them down). When NONE was supplied, fall back to the
        // hostname-based AuthResolver (DockerConfig → ~/.docker/config.json),
        // matching youki — so `zlayer login`-written creds are honored here too.
        let pull_auth = Self::resolve_pull_auth(auth, image);

        // Central constructor: wires the local persistent blob cache through the
        // full source chain — the shared S3 tier (when ZLAYER_S3_BUCKET is
        // configured) and the last-resort default registry (when
        // ZLAYER_DEFAULT_REGISTRY is set) — AND applies the per-image source
        // policy. The daemon-wide local OCI registry (`{data_dir}/registry`,
        // written by `zlayer import` / `zlayer build`) is chained in so
        // imported/built images resolve here without a remote pull.
        let mut puller =
            zlayer_registry::ImagePuller::from_env_for_runtime(blob_cache, source).await;
        if let Some(reg) = self.open_local_registry().await {
            puller = puller.with_local_registry(reg);
        }

        let layers =
            puller
                .pull_image(image, &pull_auth)
                .await
                .map_err(|e| AgentError::PullFailed {
                    image: image.to_string(),
                    reason: format!("pull image layers: {e}"),
                })?;

        // Persist the OCI image CONFIG blob into the same `blobs.redb` while we
        // still have the network. `pull_image` above caches the manifest + layers
        // but NOT the config blob, and the config's `os` field is what the
        // composite's LOCAL-ONLY dispatch inspection
        // (`fetch_image_os_in_cache_only`) reads to route Linux images here on a
        // later `create_container` with NO network. Without this, a Docker Hub
        // 429 on the redundant dispatch-time re-inspection used to leave the OS
        // unknown and fall the image through to the Seatbelt sandbox (exit 127).
        // Non-fatal: the layers are already extracted, so a config-blob miss only
        // costs us the local OS hint (dispatch still has its VZ-Linux default).
        write_image_config_sidecar(&puller, image, &pull_auth, &image_dir).await;

        tracing::info!(
            image = %image,
            layer_count = layers.len(),
            "extracting layers to vz-linux image rootfs"
        );

        let mut unpacker = zlayer_registry::LayerUnpacker::new(rootfs_dir.clone());
        if let Err(e) = unpacker.unpack_layers(&layers).await {
            // LOUD failure + no poisoned cache: a partial rootfs left on disk
            // used to satisfy the populated-rootfs probe forever, so every
            // later create shared a truncated filesystem into the guest.
            tracing::error!(
                image = %image,
                rootfs = %rootfs_dir.display(),
                error = %e,
                "vz-linux: rootfs extraction failed; removing partial rootfs"
            );
            let _ = tokio::fs::remove_dir_all(&rootfs_dir).await;
            if let Some(marker) = rootfs_complete_marker(&rootfs_dir) {
                let _ = tokio::fs::remove_file(&marker).await;
            }
            return Err(AgentError::PullFailed {
                image: image.to_string(),
                reason: format!("extract rootfs: {e}"),
            });
        }

        // Marker AFTER a fully-successful unpack; its absence is what makes
        // `rootfs_is_populated` reject interrupted/truncated extractions.
        if let Some(marker) = rootfs_complete_marker(&rootfs_dir) {
            if let Err(e) = tokio::fs::write(&marker, b"").await {
                tracing::warn!(
                    marker = %marker.display(),
                    error = %e,
                    "vz-linux: could not write rootfs completion marker; \
                     the image will re-extract on next use"
                );
            }
        }

        self.image_rootfs
            .write()
            .await
            .insert(safe_name, rootfs_dir.clone());

        tracing::info!(
            image = %image,
            rootfs = %rootfs_dir.display(),
            "vz-linux image pulled successfully"
        );
        Ok(())
    }

    async fn create_container(&self, id: &ContainerId, spec: &ServiceSpec) -> Result<()> {
        let dir_name = Self::container_dir_name(id);
        let state_dir = self.vm_dir(id);

        // Locate the base image rootfs. `spec.image.name` is an `ImageRef` whose
        // `to_string()` preserves the user's raw bytes (`alpine:latest`). The
        // store is keyed by that LITERAL ref (no canonicalization), so:
        //   1. the in-memory `image_rootfs` map (populated by the pull) under the
        //      literal key, then
        //   2. `resolve_existing_rootfs`, which probes the literal key AND every
        //      candidate spelling on disk — so an image extracted under an
        //      equivalent spelling (e.g. by an explicit
        //      `pull docker.io/library/alpine:latest`) is still found for a
        //      deploy of `alpine:latest`, WITHOUT rewriting the key onto docker.io.
        let image_name = spec.image.name.to_string();
        let safe_image = sanitize_image_name(&image_name);
        let image_rootfs = {
            let images = self.image_rootfs.read().await;
            images.get(&safe_image).cloned()
        }
        .or_else(|| self.resolve_existing_rootfs(&image_name))
        .unwrap_or_else(|| self.images_dir().join(&safe_image).join("rootfs"));

        if !image_rootfs.exists() {
            return Err(AgentError::CreateFailed {
                id: dir_name,
                reason: format!(
                    "image rootfs not found at {}; pull the image first",
                    image_rootfs.display()
                ),
            });
        }

        // Guard against an EMPTY rootfs share. `pull_image_with_policy` creates
        // the `…/rootfs` dir BEFORE extracting layers, and the composite swallows
        // a per-backend pull error (e.g. a Docker Hub 429) as non-fatal — so a
        // failed pull can leave the directory present-but-empty. Under
        // `PullPolicy::IfNotPresent` that empty dir then short-circuits every
        // subsequent pull, and sharing it into the guest pivot_roots onto an
        // empty filesystem where `/bin/sh` does not exist ("No such file or
        // directory"). Refuse here with an actionable error instead of booting a
        // doomed guest, so the caller re-pulls (or the operator clears the dir).
        if !rootfs_is_populated(&image_rootfs) {
            return Err(AgentError::CreateFailed {
                id: dir_name,
                reason: format!(
                    "image rootfs at {} is empty (a previous pull was interrupted or failed); \
                     re-pull the image — remove the empty directory if the re-pull keeps \
                     short-circuiting under PullPolicy::IfNotPresent",
                    image_rootfs.display()
                ),
            });
        }

        // Read the OCI image config (Entrypoint/Cmd/Env/WorkingDir/User) from the
        // `image-config.json` sidecar written next to this rootfs at pull time.
        // This is what lets `build_run_message` apply an image's DEFAULT command
        // (postgres/forgejo etc.) instead of falling back to `/bin/true`.
        let image_config = read_image_config_sidecar(&image_rootfs);
        if image_config.is_none() {
            tracing::warn!(
                container = %dir_name,
                image = %image_name,
                "vz-linux: no image-config.json sidecar found for image; the image's \
                 default ENTRYPOINT/CMD/ENV/WORKDIR/USER will NOT apply (re-pull the \
                 image to generate the sidecar). The spec command still runs.",
            );
        }

        std::fs::create_dir_all(&state_dir).map_err(|e| AgentError::CreateFailed {
            id: dir_name.clone(),
            reason: format!("create state dir: {e}"),
        })?;

        let rootfs_dir = state_dir.join("rootfs");
        let console_log = state_dir.join("console.log");

        // Reference the shared image rootfs via an APFS clone of the directory
        // entry. A later phase wires virtiofs to share this into the guest.
        if let Err(e) = clone_or_copy(&image_rootfs, &rootfs_dir) {
            tracing::debug!(
                container = %dir_name,
                error = %e,
                "vz-linux: clonefile of image rootfs failed; per-container rootfs deferred to a later phase"
            );
        }

        // Per-VM locally-administered MAC.
        let mac = unsafe {
            VZMACAddress::randomLocallyAdministeredAddress()
                .string()
                .to_string()
        };

        // Persist the spec for diagnostics.
        if let Ok(json) = serde_json::to_string_pretty(spec) {
            std::fs::write(state_dir.join("config.json"), json).ok();
        }

        // Linux guests are lean: default 512 MiB, floor 128 MiB (kernel + init).
        let vcpus = spec_vcpus(spec, 2);
        let ram_mib = spec_memory_mib(spec, 512, 128);

        let container = VzLinuxContainer {
            state: ContainerState::Pending,
            state_dir,
            rootfs_dir,
            image_rootfs_dir: image_rootfs,
            kernel: None,
            console_log,
            mac,
            spec: spec.clone(),
            image_config,
            vcpus,
            ram_mib,
            started_at: None,
            live: None,
            outcome: Arc::new(RunOutcome::default()),
            drain_task: None,
            cleanup_task: None,
            settime_task: None,
            port_forwards: Vec::new(),
            stdin_tx: None,
            overlay_ip: None,
        };

        self.containers.write().await.insert(dir_name, container);
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    async fn start_container(&self, id: &ContainerId) -> Result<()> {
        let dir_name = Self::container_dir_name(id);

        // Resolve the guest kernel + initramfs (env override, then a normal pull
        // of the published bundle image, falling back to the on-disk cache).
        let kernel = self.ensure_linux_kernel().await?;

        // Snapshot the Send build inputs without holding the lock across the
        // blocking queue work.
        let inputs = {
            let guard = self.containers.read().await;
            let c = guard.get(&dir_name).ok_or_else(|| AgentError::NotFound {
                container: dir_name.clone(),
                reason: "not created".to_string(),
            })?;
            LinuxVmBuildInputs {
                kernel: kernel.clone(),
                console_log: c.console_log.clone(),
                image_rootfs_dir: c.image_rootfs_dir.clone(),
                cpu_count: clamp_cpu_count(c.vcpus),
                memory_bytes: clamp_memory_bytes(c.ram_mib),
                mac: c.mac.clone(),
                bind_mounts: derive_bind_mounts(&c.spec),
            }
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

        // Build the config + VM + start it on a dedicated serial queue. All of
        // this is blocking Obj-C work, so run it on a blocking thread. Linux
        // guests are uncapped (no 2-VM macOS licensing limit).
        let dir_for_task = dir_name.clone();
        let live = tokio::task::spawn_blocking(move || -> std::result::Result<LiveVm, String> {
            let queue = DispatchQueue::new(&format!("com.zlayer.vz-linux.{dir_for_task}"), None);

            // Build the config + create the VM on the queue.
            let (tx, rx) = std::sync::mpsc::channel::<
                std::result::Result<QueuePinned<Retained<VZVirtualMachine>>, String>,
            >();
            let inputs_q = inputs.clone();
            let queue_for_vm = queue.clone();
            queue.exec_sync(move || {
                let built = build_config_linux(&inputs_q).map(|config| {
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

        // A queue-handle clone of the live VM for the drain task to drive the
        // vsock connect on the VM's serial queue (VZ objects are queue-bound).
        let live_for_drain = LiveVm {
            queue: live.queue.clone(),
            vm: Arc::clone(&live.vm),
        };
        // A second queue-handle clone for the periodic wall-clock resync task,
        // taken here before `live` is moved into the container record below.
        let live_for_settime = LiveVm {
            queue: live.queue.clone(),
            vm: Arc::clone(&live.vm),
        };

        // Build the `Run` message + bind-mount list + capture handles, then
        // record the running VM. The bind mounts are derived from the SAME spec
        // storage that produced the per-bind virtiofs shares in
        // `build_config_linux`, so the `Msg::Mount` frames sent below name tags
        // the guest has matching virtiofs devices for.
        let (run_msg, bind_mounts, outcome, console_log) = {
            let mut guard = self.containers.write().await;
            let c = guard
                .get_mut(&dir_name)
                .ok_or_else(|| AgentError::NotFound {
                    container: dir_name.clone(),
                    reason: "vanished during start".to_string(),
                })?;
            c.kernel = Some(kernel);
            c.live = Some(live);
            c.state = ContainerState::Running;
            c.started_at = Some(Instant::now());
            (
                build_run_message(&c.spec, c.image_config.as_ref()),
                derive_bind_mounts(&c.spec),
                Arc::clone(&c.outcome),
                c.console_log.clone(),
            )
        };

        // Interactive `-it` stdin channel: the sender half is stored in the
        // container record so `write_stdin` can push host terminal bytes; the
        // receiver half is handed to the blocking drain task, which forwards
        // each chunk as a `Msg::Stdin` frame. Dropping the sender (via
        // `close_stdin` or container teardown) makes the drain task's `recv()`
        // err and emit a final `Msg::StdinEof`. We wire it unconditionally —
        // no interactivity flag is plumbed to this runtime yet, and an unread
        // channel is harmless (the drain task just blocks on `recv()` and the
        // guest sees no stdin frames until bytes arrive).
        let (stdin_tx, stdin_rx) = std::sync::mpsc::channel::<Vec<u8>>();

        // Spawn the blocking vsock drain task: connect to the guest agent,
        // send `Run`, emit one `Mount` frame per bind mount, forward stdin
        // chunks, then stream stdout/stderr/exit into the shared outcome.
        let dir_for_drain = dir_name.clone();
        let drain = tokio::task::spawn_blocking(move || {
            run_and_drain(
                &live_for_drain,
                &run_msg,
                &bind_mounts,
                Some(stdin_rx),
                &outcome,
                &console_log,
                &dir_for_drain,
            );
        });
        {
            let mut guard = self.containers.write().await;
            if let Some(c) = guard.get_mut(&dir_name) {
                c.drain_task = Some(drain);
                c.stdin_tx = Some(stdin_tx);
            }
        }

        // Spawn the periodic wall-clock resync task. A VZ guest has no RTC and
        // never resyncs after a host sleep, so a long-lived VM's clock drifts
        // (breaking x509 validity windows + time-aware workloads). Every
        // `SETTIME_RESYNC_INTERVAL` we push the host's current epoch as a
        // `Msg::SetTime`. The vsock connect + frame write are blocking, so each
        // tick runs them via `spawn_blocking`. The handle is tied to VM lifetime:
        // `stop`/`remove` abort it (alongside the drain/cleanup tasks), so it
        // exits cleanly when the VM stops and never pushes to a dead VM.
        let dir_for_settime = dir_name.clone();
        let settime = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(SETTIME_RESYNC_INTERVAL);
            // Skip the immediate tick `interval` fires at t=0: the guest already
            // seeded its clock from `zlayer.boottime=` at boot, so the first
            // resync should wait a full interval.
            ticker.tick().await;
            loop {
                ticker.tick().await;
                let live = LiveVm {
                    queue: live_for_settime.queue.clone(),
                    vm: Arc::clone(&live_for_settime.vm),
                };
                match tokio::task::spawn_blocking(move || push_settime(&live)).await {
                    Ok(Ok(acked)) => tracing::debug!(
                        container = %dir_for_settime,
                        guest_acked = acked,
                        "vz-linux: periodic wall-clock resync pushed to guest"
                    ),
                    Ok(Err(e)) => tracing::warn!(
                        container = %dir_for_settime,
                        error = %e,
                        "vz-linux: periodic wall-clock resync failed; retrying next tick"
                    ),
                    Err(e) => tracing::warn!(
                        container = %dir_for_settime,
                        error = %e,
                        "vz-linux: wall-clock resync task panicked; retrying next tick"
                    ),
                }
            }
        });
        {
            let mut guard = self.containers.write().await;
            if let Some(c) = guard.get_mut(&dir_name) {
                c.settime_task = Some(settime);
            } else {
                // Container vanished between the drain spawn and here: don't leak
                // the resync task.
                settime.abort();
            }
        }

        // `--rm` / delete_on_exit: when the spec asks for it, spawn a watcher
        // that tears the container down (VM + tasks + state dir) once the
        // workload reaches a terminal state. The VM does not power itself off on
        // workload exit, so this is what frees the resources. The handle is
        // tracked so an explicit `stop`/`remove` can abort it and win the race.
        let delete_on_exit = {
            let guard = self.containers.read().await;
            guard
                .get(&dir_name)
                .is_some_and(|c| c.spec.lifecycle.delete_on_exit)
        };
        if delete_on_exit {
            let containers = Arc::clone(&self.containers);
            let dir_for_cleanup = dir_name.clone();
            let cleanup =
                tokio::spawn(
                    async move { watch_for_auto_remove(containers, dir_for_cleanup).await },
                );
            let mut guard = self.containers.write().await;
            if let Some(c) = guard.get_mut(&dir_name) {
                c.cleanup_task = Some(cleanup);
            } else {
                // Container vanished between the drain spawn and here: don't leak
                // the watcher task.
                cleanup.abort();
            }
        }

        // Stand up host loopback forwarders for the spec's published container
        // ports. VZ NAT establishes no usable guest lease on this macOS, so the
        // guest has no routable IP; instead each forwarder binds
        // `127.0.0.1:<container_port>` on the host and tunnels every accepted
        // connection over a fresh vsock `Forward` into the guest agent, which
        // splices it to `127.0.0.1:<container_port>` inside the guest. This is
        // the host->guest reachability path (replacing the broken NAT-IP route).
        self.spawn_port_forwards(&dir_name).await;

        Ok(())
    }

    async fn stop_container(&self, id: &ContainerId, timeout: Duration) -> Result<()> {
        let dir_name = Self::container_dir_name(id);

        // Snapshot what we need WITHOUT yet tearing the container down: a
        // queue-handle clone of the live VM (to deliver SIGTERM + read state),
        // the shared outcome (to observe the workload's terminal state), and
        // whether the workload is still running. Stopping an already-stopped
        // (or already-removed) container is a no-op success — idempotent.
        let (live_for_signal, outcome, already_terminal) = {
            let guard = self.containers.read().await;
            let Some(c) = guard.get(&dir_name) else {
                // Removing/stopping an unknown container is Ok (idempotent).
                return Ok(());
            };
            let live = c.live.as_ref().map(|l| LiveVm {
                queue: l.queue.clone(),
                vm: Arc::clone(&l.vm),
            });
            let terminal = c.outcome.terminal.lock().ok().and_then(|g| g.clone());
            (live, Arc::clone(&c.outcome), terminal)
        };

        // Graceful phase: if the VM is live and the workload has NOT already
        // exited, ask the guest agent to SIGTERM the workload, then wait up to
        // `timeout` for it to reach a terminal state. A live agent connection is
        // required; if the signal can't be delivered we fall straight through to
        // the forced VM stop below.
        if let Some(live) = &live_for_signal {
            if already_terminal.is_none() {
                let live_sig = LiveVm {
                    queue: live.queue.clone(),
                    vm: Arc::clone(&live.vm),
                };
                let signalled = tokio::task::spawn_blocking(move || {
                    signal_agent(&live_sig, signal_number("SIGTERM"))
                })
                .await;
                match signalled {
                    Ok(Ok(())) => {
                        // Poll the shared outcome for the agent's `Exited`/`Failed`
                        // frame up to the caller's grace period.
                        let deadline = Instant::now() + timeout;
                        while Instant::now() < deadline {
                            if outcome
                                .terminal
                                .lock()
                                .ok()
                                .and_then(|g| g.clone())
                                .is_some()
                            {
                                break;
                            }
                            tokio::time::sleep(Duration::from_millis(100)).await;
                        }
                    }
                    Ok(Err(e)) => {
                        tracing::debug!(
                            container = %dir_name,
                            error = %e,
                            "vz-linux: graceful SIGTERM not delivered; forcing VM stop"
                        );
                    }
                    Err(e) => {
                        tracing::debug!(
                            container = %dir_name,
                            error = %e,
                            "vz-linux: SIGTERM task panicked; forcing VM stop"
                        );
                    }
                }
            }
        }

        // Teardown phase: take the live VM + tasks out of the record, set the
        // final state, abort the drain + port-forward + cleanup + resync tasks,
        // then force the VM down and release it.
        let (live, drain, cleanup, settime) = {
            let mut guard = self.containers.write().await;
            let Some(c) = guard.get_mut(&dir_name) else {
                // Vanished mid-stop: nothing more to do.
                return Ok(());
            };
            // Preserve the workload's real exit code where the drain task already
            // recorded one (via either `terminal` or the `exit_code` the wait
            // path reads); otherwise mark a forced stop as a clean exit. Routing
            // through `outcome_to_state` keeps the stored `c.state` in lockstep
            // with what `wait_container` reports.
            let recorded = c.outcome.terminal.lock().ok().and_then(|g| g.clone());
            let recorded_code = c.outcome.exit_code.lock().ok().and_then(|g| *g);
            c.state = outcome_to_state(recorded_code, recorded, ContainerState::Exited { code: 0 });
            // Drop network state: abort every host loopback forwarder.
            for f in std::mem::take(&mut c.port_forwards) {
                f.task.abort();
            }
            (
                c.live.take(),
                c.drain_task.take(),
                c.cleanup_task.take(),
                c.settime_task.take(),
            )
        };
        // Abort the periodic wall-clock resync task so it can't push to the VM
        // we're about to tear down.
        if let Some(settime) = settime {
            settime.abort();
        }
        // Abort the delete_on_exit watcher so an explicit stop wins the race and
        // the watcher can't race this teardown / double-free the record.
        if let Some(cleanup) = cleanup {
            cleanup.abort();
        }
        // The drain task blocks on vsock IO; aborting drops its future. The fd
        // it owns is closed when the VM stops, so the blocking read unblocks.
        if let Some(drain) = drain {
            drain.abort();
        }
        if let Some(live) = live {
            // Force the VM down (a no-op-ish if the guest already powered off
            // after SIGTERM) and release it, closing the vsock device + its fds.
            let _ = tokio::task::spawn_blocking(move || {
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
        // Remove the record outright, taking ownership of its VM + every spawned
        // task. Removing an unknown container is Ok (idempotent) — this matches
        // docker_runtime_test::test_remove_nonexistent_container.
        let (state_dir, live, drain, cleanup, settime, forwards) = {
            let mut guard = self.containers.write().await;
            match guard.remove(&dir_name) {
                Some(c) => (
                    Some(c.state_dir),
                    c.live,
                    c.drain_task,
                    c.cleanup_task,
                    c.settime_task,
                    c.port_forwards,
                ),
                None => return Ok(()),
            }
        };
        // Abort the periodic wall-clock resync task so it can't race this
        // explicit removal / push to a torn-down VM.
        if let Some(settime) = settime {
            settime.abort();
        }
        // Abort the delete_on_exit watcher (if any) so it can't race this
        // explicit removal.
        if let Some(cleanup) = cleanup {
            cleanup.abort();
        }
        // Abort the drain task FIRST so its blocking vsock read can't race the
        // VM teardown below.
        if let Some(drain) = drain {
            drain.abort();
        }
        // Drop network state: stop all host loopback forwarders.
        for f in forwards {
            f.task.abort();
        }
        // Ensure the VM is fully stopped + released (closing its vsock device and
        // fds) before we delete its state directory. A `Stop` on an
        // already-stopped VM is harmless; errors are non-fatal for removal.
        if let Some(live) = live {
            let _ = tokio::task::spawn_blocking(move || {
                let r = run_vm_lifecycle(&live, VmLifecycleOp::Stop);
                drop(live);
                r
            })
            .await;
        }
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
        // The workload's captured outcome is authoritative: it exited (with its
        // real code) even if the VM lingers as a running VM. The drain task
        // records the code on `RunOutcome::exit_code` (what `wait_container`
        // returns) and a richer `terminal` state on the `Exited`/`Error` frame.
        // Read BOTH so the state path uses the same captured exit code as the
        // wait path — otherwise the real code (e.g. 42) is dropped here and the
        // VM-`Stopped` reconcile below fabricates `Exited { code: 0 }`.
        let captured_code = c.outcome.exit_code.lock().ok().and_then(|g| *g);
        let terminal = c.outcome.terminal.lock().ok().and_then(|g| g.clone());
        if terminal.is_some() || captured_code.is_some() {
            // A captured outcome wins outright; the VM may still be tearing down.
            return Ok(outcome_to_state(captured_code, terminal, c.state.clone()));
        }
        // No captured workload outcome yet: reconcile with the live VM's state.
        if let Some(live) = &c.live {
            let reconciled = match read_vm_state(live) {
                VZVirtualMachineState::Running | VZVirtualMachineState::Paused => {
                    ContainerState::Running
                }
                VZVirtualMachineState::Error => ContainerState::Failed {
                    reason: "VM entered error state".to_string(),
                },
                // The guest powered off before the agent reported an exit: keep
                // the last recorded exit state where we have one, else a clean 0.
                VZVirtualMachineState::Stopped => match &c.state {
                    s @ ContainerState::Exited { .. } => s.clone(),
                    _ => ContainerState::Exited { code: 0 },
                },
                _ => c.state.clone(),
            };
            return Ok(reconciled);
        }
        Ok(c.state.clone())
    }

    async fn container_logs(&self, id: &ContainerId, tail: usize) -> Result<Vec<LogEntry>> {
        self.captured_logs(id, tail).await
    }

    async fn exec(&self, id: &ContainerId, cmd: &[String]) -> Result<(i32, String, String)> {
        let opts = crate::runtime::ExecOptions {
            command: cmd.to_vec(),
            ..Default::default()
        };
        self.exec_with_opts(id, &opts).await
    }

    async fn exec_with_opts(
        &self,
        id: &ContainerId,
        opts: &crate::runtime::ExecOptions,
    ) -> Result<(i32, String, String)> {
        let dir_name = Self::container_dir_name(id);

        if opts.command.is_empty() {
            return Err(AgentError::Configuration(
                "vz-linux: exec requires a non-empty command".to_string(),
            ));
        }

        // Snapshot a queue-handle clone of the live VM (so we can drive the
        // vsock connect on its serial queue) plus the container spec's env to
        // inherit into the exec'd process. The VM must be running with a live
        // agent connection, or there is nothing to exec into.
        let (live, mut env) = {
            let guard = self.containers.read().await;
            let c = guard.get(&dir_name).ok_or_else(|| AgentError::NotFound {
                container: dir_name.clone(),
                reason: "not found".to_string(),
            })?;
            let live = c.live.as_ref().ok_or_else(|| {
                AgentError::Configuration(format!(
                    "vz-linux: container {dir_name} is not running (no live VM / agent connection)"
                ))
            })?;
            // If the workload already exited there is no PID namespace to enter.
            if c.outcome
                .terminal
                .lock()
                .ok()
                .and_then(|g| g.clone())
                .is_some()
            {
                return Err(AgentError::Configuration(format!(
                    "vz-linux: container {dir_name} workload has exited; nothing to exec into"
                )));
            }
            let env: Vec<(String, String)> = c
                .spec
                .env
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            (
                LiveVm {
                    queue: live.queue.clone(),
                    vm: Arc::clone(&live.vm),
                },
                env,
            )
        };

        // Merge Docker `-e KEY=VALUE` overrides on top of the container env
        // (later entries win, so an override replaces an inherited value).
        for kv in &opts.env {
            if let Some((k, v)) = kv.split_once('=') {
                if let Some(slot) = env.iter_mut().find(|(ek, _)| ek == k) {
                    slot.1 = v.to_string();
                } else {
                    env.push((k.to_string(), v.to_string()));
                }
            }
        }

        // Resolve `--user` into a numeric uid/gid where possible, deferring a
        // pure NAME (`git`) to the guest, which can read the container's
        // /etc/passwd after pivot.
        let (uid, gid, user) = parse_exec_user(opts.user.as_deref());

        let exec_msg = proto::Msg::Exec {
            argv: opts.command.clone(),
            env,
            cwd: opts.working_dir.clone(),
            uid,
            gid,
            user,
        };

        // The connect + frame IO is blocking (synchronous `UnixStream`), and
        // `connect_vsock` has its own timeout so a dead guest can't hang us.
        let dir_for_task = dir_name.clone();
        tokio::task::spawn_blocking(move || exec_and_collect(&live, &exec_msg))
            .await
            .map_err(|e| {
                AgentError::Configuration(format!(
                    "vz-linux: exec task for {dir_for_task} panicked: {e}"
                ))
            })?
            .map_err(|e| AgentError::Configuration(format!("vz-linux: exec failed: {e}")))
    }

    async fn get_container_stats(&self, id: &ContainerId) -> Result<ContainerStats> {
        let dir_name = Self::container_dir_name(id);
        let guard = self.containers.read().await;
        let c = guard.get(&dir_name).ok_or_else(|| AgentError::NotFound {
            container: dir_name.clone(),
            reason: "not found".to_string(),
        })?;
        // VZ (Virtualization.framework) exposes NO per-VM live resource metrics
        // — there is no API to read a guest's actual RSS or CPU time. So we
        // report the configured allocation as the limit and a coarse,
        // deliberately NON-ZERO usage estimate (a quarter of the RAM the VM was
        // booted with). The estimate matters: `docker_runtime_test::
        // test_container_stats` asserts `memory_bytes > 0` for a running
        // container, and `macos_vz.rs` reporting `memory_bytes = 0` would fail
        // that assertion. A guest kernel + init + workload always resident in at
        // least a fraction of its RAM, so a quarter is a sane floor rather than
        // an invented exact figure.
        let memory_limit = u64::from(c.ram_mib) * 1024 * 1024;
        let memory_bytes = (memory_limit / 4).max(1);
        Ok(ContainerStats {
            // No live CPU accounting available from VZ.
            cpu_usage_usec: 0,
            memory_bytes,
            memory_limit,
            timestamp: Instant::now(),
        })
    }

    async fn stats_stream(&self, id: &ContainerId) -> Result<StatsStream> {
        use futures_util::stream;

        // VZ exposes no live per-VM metrics, so we emit a single deterministic
        // sample built from the configured allocation (matching
        // `get_container_stats`). Previously this fell through to the
        // `Unsupported` default, 500-ing `GET /containers/{id}/stats`. The
        // sample carries a NON-ZERO `mem_used_bytes` so the Docker-compat
        // one-shot stats body satisfies `memory_bytes > 0`.
        let dir_name = Self::container_dir_name(id);
        let (vcpus, ram_mib) = {
            let guard = self.containers.read().await;
            let c = guard.get(&dir_name).ok_or_else(|| AgentError::NotFound {
                container: dir_name.clone(),
                reason: "not found".to_string(),
            })?;
            (c.vcpus, c.ram_mib)
        };
        let mem_limit_bytes = u64::from(ram_mib) * 1024 * 1024;
        let mem_used_bytes = (mem_limit_bytes / 4).max(1);
        let sample = StatsSample {
            cpu_total_ns: 0,
            cpu_system_ns: 0,
            online_cpus: vcpus,
            mem_used_bytes,
            mem_limit_bytes,
            net_rx_bytes: 0,
            net_tx_bytes: 0,
            blkio_read_bytes: 0,
            blkio_write_bytes: 0,
            pids_current: 0,
            pids_limit: None,
            timestamp: chrono::Utc::now(),
        };
        Ok(Box::pin(stream::iter(vec![Ok(sample)])))
    }

    async fn wait_container(&self, id: &ContainerId) -> Result<i32> {
        let dir_name = Self::container_dir_name(id);
        loop {
            {
                let guard = self.containers.read().await;
                let Some(c) = guard.get(&dir_name) else {
                    // Removed out from under us: treat as a clean exit.
                    return Ok(0);
                };
                // The drain task records the real exit code on `Exited`.
                if let Some(code) = c.outcome.exit_code.lock().ok().and_then(|g| *g) {
                    return Ok(code);
                }
                // An `Error` frame (or a failed VM) terminates without a code.
                if let Some(ContainerState::Failed { .. }) =
                    c.outcome.terminal.lock().ok().and_then(|g| g.clone())
                {
                    return Ok(-1);
                }
                // If the VM itself stopped before the agent reported an exit,
                // fall back to a clean exit so callers don't hang forever.
                if let Some(live) = &c.live {
                    if read_vm_state(live) == VZVirtualMachineState::Stopped {
                        return Ok(0);
                    }
                } else {
                    return Ok(0);
                }
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    async fn get_logs(&self, id: &ContainerId) -> Result<Vec<LogEntry>> {
        self.captured_logs(id, 1000).await
    }

    #[allow(clippy::too_many_lines)]
    async fn logs_stream(&self, id: &ContainerId, opts: LogsStreamOptions) -> Result<LogsStream> {
        use futures_util::{stream, StreamExt as _};

        // VZ-Linux captures the workload's stdout/stderr (plus the guest serial
        // console) into an append-only buffer drained over vsock; there is no
        // live tail device. Both the guest `console.log` and the
        // [`RunOutcome::logs`] workload buffer only ever GROW, so we can follow
        // them by re-merging and emitting the lines past a running offset.
        //
        // Honour the per-channel filters when either is explicitly requested;
        // Docker's default (neither set) means "both".
        let want_stdout = opts.stdout || !opts.stderr;
        let want_stderr = opts.stderr || !opts.stdout;
        let with_ts = opts.timestamps;

        // Convert a `LogEntry` into a `LogChunk`, applying the channel filter.
        let to_chunk = move |e: LogEntry| -> Option<Result<LogChunk>> {
            let channel = match e.stream {
                LogStream::Stdout => LogChannel::Stdout,
                LogStream::Stderr => LogChannel::Stderr,
            };
            let keep = match channel {
                LogChannel::Stdout => want_stdout,
                LogChannel::Stderr => want_stderr,
                LogChannel::Stdin => false,
            };
            if !keep {
                return None;
            }
            // Re-attach the newline the line-splitter stripped so a consumer
            // that concatenates chunks reconstructs the original output.
            let mut bytes = e.message.into_bytes();
            bytes.push(b'\n');
            Some(Ok(LogChunk {
                stream: channel,
                bytes: bytes::Bytes::from(bytes),
                timestamp: with_ts.then_some(e.timestamp),
            }))
        };

        // ---- Non-follow: one-shot snapshot honouring `tail`. ----
        if !opts.follow {
            let tail = opts
                .tail
                .map_or(1000, |n| usize::try_from(n).unwrap_or(1000));
            let entries = self.captured_logs(id, tail).await?;
            let chunks: Vec<Result<LogChunk>> = entries.into_iter().filter_map(to_chunk).collect();
            return Ok(Box::pin(stream::iter(chunks)));
        }

        // ---- Follow: emit the initial `tail` snapshot, then poll the
        // append-only merged buffer for new lines until the workload reaches a
        // terminal state (and one final drain after, to catch the lines that
        // arrived alongside the `Exited` frame). This is what the foreground
        // `zlayer run` path relies on: without a real follow the stream ended
        // mid-boot and the run "completed" before `echo` ever ran. ----

        // Snapshot the full merged log once to seed the initial emit. The merged
        // snapshot is `console.log` (kernel/boot serial output) ++ the
        // `RunOutcome::logs` workload buffer captured SO FAR. We emit the last
        // `tail` of it, then the follow loop tracks ONLY the workload buffer's
        // growth.
        //
        // The follow offset MUST be seeded from the workload buffer's length at
        // snapshot time — NOT the merged total. The two live in different index
        // spaces (the merged list interleaves the console, which the follow loop
        // does not re-read); seeding the follow cursor with the merged total
        // makes `next` (e.g. 12, counting boot console lines) overshoot the
        // workload buffer's real length (e.g. 0), so the follow loop never emits
        // the workload's stdout (`HELLO_FROM_GUEST` / `uname -m`) even though it
        // arrives. Seed from the buffer's own length so we resume exactly where
        // the snapshot left off and emit each subsequent workload line once.
        let dir_name = Self::container_dir_name(id);
        // Read the workload-buffer length BEFORE the merged snapshot so a line
        // that appears between the two reads is re-emitted by the follow loop
        // rather than dropped (at worst a duplicate, never a gap).
        let workload_seen = {
            let guard = self.containers.read().await;
            guard
                .get(&dir_name)
                .and_then(|c| c.outcome.logs.lock().ok().map(|b| b.len()))
                .unwrap_or(0)
        };
        let full = self.captured_logs(id, usize::MAX).await?;
        let total = full.len();
        let tail = opts
            .tail
            .map_or(total, |n| usize::try_from(n).unwrap_or(total));
        let seed_start = total.saturating_sub(tail);
        let initial: Vec<Result<LogChunk>> = full
            .into_iter()
            .skip(seed_start)
            .filter_map(to_chunk)
            .collect();

        // The follow loop closes over a clone of the containers map and the
        // container id so it can re-read the growing workload buffer without
        // borrowing `self`. State: (next offset into `RunOutcome::logs`, whether
        // we've done the final post-terminal drain).
        let containers = Arc::clone(&self.containers);

        let seed_state = LogFollowState {
            containers,
            dir_name,
            next: workload_seen,
            done: false,
        };

        let follow = stream::unfold(seed_state, move |mut st| {
            // `to_chunk` is a `Copy` closure (captures only flags), so it is
            // freely usable in each iteration without an explicit clone.
            async move {
                loop {
                    if st.done {
                        return None;
                    }

                    // Re-read the workload buffer + terminal flag. The console
                    // (kernel/boot) output is captured up front in the initial
                    // snapshot; live follow tracks the vsock workload buffer,
                    // which is the stdout/stderr callers actually want.
                    let (entries, terminal): (Vec<LogEntry>, bool) = {
                        let guard = st.containers.read().await;
                        match guard.get(&st.dir_name) {
                            Some(c) => {
                                let entries =
                                    c.outcome.logs.lock().map(|b| b.clone()).unwrap_or_default();
                                let terminal = c
                                    .outcome
                                    .terminal
                                    .lock()
                                    .ok()
                                    .and_then(|t| t.clone())
                                    .is_some()
                                    || c.outcome.exit_code.lock().ok().and_then(|g| *g).is_some();
                                (entries, terminal)
                            }
                            // Container removed (e.g. `--rm` teardown): end the
                            // stream cleanly.
                            None => return None,
                        }
                    };

                    if st.next < entries.len() {
                        // Emit the next not-yet-seen line.
                        let entry = entries[st.next].clone();
                        st.next += 1;
                        if let Some(chunk) = to_chunk(entry) {
                            return Some((chunk, st));
                        }
                        // Filtered out (wrong channel): keep scanning without
                        // sleeping.
                        continue;
                    }

                    // No new lines. If the workload is terminal AND we've
                    // drained everything, do one last pass next iteration then
                    // stop.
                    if terminal {
                        st.done = true;
                        return None;
                    }

                    // Still running, nothing new yet — poll again shortly.
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        });

        Ok(Box::pin(stream::iter(initial).chain(follow)))
    }

    async fn get_container_pid(&self, id: &ContainerId) -> Result<Option<u32>> {
        // A VM has no host-visible container PID. Per the overlay-attach contract
        // we report a stable, nonzero *pseudo*-PID derived from the container id
        // (`use the containerID as the process ID`) so PID-gated bookkeeping —
        // the service layer's liveness check and overlay-attach gate — proceeds
        // instead of short-circuiting at "PID unavailable". The pseudo-PID is
        // never used as a real host PID: VZ overlay attach goes through
        // `push_overlay_config` (`OverlayAttachKind::InGuestVsock`), not
        // netns-by-PID. Returns `None` when the VM is not live (mirrors a stopped
        // container having no PID).
        let dir_name = Self::container_dir_name(id);
        let guard = self.containers.read().await;
        match guard.get(&dir_name) {
            Some(c) if c.live.is_some() => Ok(Some(Self::pseudo_pid(id))),
            _ => Ok(None),
        }
    }

    fn overlay_attach_kind(&self) -> OverlayAttachKind {
        // A VZ guest is a VM with no host netns/PID: the host can't plumb a veth
        // by PID. The service layer instead asks overlayd for a guest-managed
        // config and pushes it into the guest over vsock (`push_overlay_config`).
        OverlayAttachKind::InGuestVsock
    }

    async fn push_overlay_config(
        &self,
        id: &ContainerId,
        config: &zlayer_types::overlayd::GuestOverlayConfig,
    ) -> Result<()> {
        let dir_name = Self::container_dir_name(id);

        // Snapshot a queue-handle clone of the live VM + the per-VM MAC (the
        // container must be running to receive the config).
        let (live, mac) = {
            let guard = self.containers.read().await;
            let c = guard.get(&dir_name).ok_or_else(|| AgentError::NotFound {
                container: dir_name.clone(),
                reason: "not found".to_string(),
            })?;
            let live = c.live.as_ref().map(|l| LiveVm {
                queue: l.queue.clone(),
                vm: Arc::clone(&l.vm),
            });
            (live, c.mac.clone())
        };
        let Some(live) = live else {
            return Err(AgentError::Network(format!(
                "cannot push overlay config to {dir_name}: VM is not live"
            )));
        };

        // The overlayd-advertised peer endpoint is the NODE's *overlay* IP, which
        // is only reachable THROUGH the overlay (circular) — a VZ guest behind NAT
        // can't use it as a WireGuard underlay. The only host address the guest
        // can reach is its NAT gateway. Resolve it from the guest's lease (the
        // `.1` of its NAT /24), falling back to the VZ default `192.168.64.1`, and
        // rewrite each peer's endpoint host to it (keeping the port). Roaming
        // peers (empty endpoint) are left untouched.
        let gateway = macos_vz_shared::current_guest_ip(&mac)
            .await
            .map_or_else(|| "192.168.64.1".to_string(), nat_gateway_for);

        // Build the wire message from the host-allocated overlay identity.
        let msg = proto::Msg::OverlayConfig {
            overlay_ip: config.overlay_ip.to_string(),
            prefix_len: config.prefix_len,
            private_key: config.private_key.clone(),
            listen_port: config.listen_port,
            peers: config
                .peers
                .iter()
                .map(|p| proto::WgPeer {
                    public_key: p.public_key.clone(),
                    endpoint: rewrite_endpoint_host(&p.endpoint, &gateway),
                    allowed_ips: p.allowed_ips.clone(),
                    persistent_keepalive_secs: p.persistent_keepalive_secs,
                })
                .collect(),
            dns_server: config.dns_server.map(|ip| ip.to_string()),
            dns_domain: config.dns_domain.clone(),
        };

        // The vsock connect + frame write are synchronous; run them off-runtime.
        let saw_eof = tokio::task::spawn_blocking(move || push_overlay_agent(&live, &msg))
            .await
            .map_err(|e| AgentError::Network(format!("overlay push task join failed: {e}")))?
            .map_err(AgentError::Network)?;
        tracing::info!(
            container = %dir_name,
            guest_acked = saw_eof,
            "pushed OverlayConfig to guest over vsock"
        );

        // Record the overlay IP so `get_container_ip` reports it (mesh routing /
        // DNS prefer the overlay address over the NAT lease).
        if let Some(c) = self.containers.write().await.get_mut(&dir_name) {
            c.overlay_ip = Some(config.overlay_ip);
        }
        Ok(())
    }

    async fn get_container_ip(&self, id: &ContainerId) -> Result<Option<IpAddr>> {
        use std::net::Ipv4Addr;
        let dir_name = Self::container_dir_name(id);
        // Resolution order for a live VZ-Linux guest:
        //   1. The overlay (`WireGuard`) IP, once `attach_overlay` has brought up
        //      `zl-overlay0` in the guest — this is the cross-node mesh address
        //      callers should prefer for service routing/DNS.
        //   2. The guest's NAT lease (`192.168.64.x`). The host's NAT DHCP server
        //      writes the lease to `/var/db/dhcpd_leases` keyed by the per-VM MAC
        //      once the guest's virtio-net device DHCPs (the in-guest agent runs
        //      udhcpc); we resolve it via `current_guest_ip`. The NAT subnet is
        //      directly host-reachable, so `guest_ip:port` works with no proxy.
        //   3. Loopback fallback — for environments where no lease appears, each
        //      published port is still reachable through the host loopback
        //      forwarder that tunnels over vsock (see `PortForward`).
        // `Ok(None)` when not running or for an unknown container.
        let mac = {
            let guard = self.containers.read().await;
            match guard.get(&dir_name) {
                Some(c) if c.live.is_some() => {
                    if let Some(ip) = c.overlay_ip {
                        return Ok(Some(ip));
                    }
                    c.mac.clone()
                }
                _ => return Ok(None),
            }
        };
        // Lock released before the (async) lease read.
        if let Some(ip) = macos_vz_shared::current_guest_ip(&mac).await {
            return Ok(Some(ip));
        }
        Ok(Some(IpAddr::V4(Ipv4Addr::LOCALHOST)))
    }

    async fn port_mappings_container(
        &self,
        id: &ContainerId,
    ) -> Result<Vec<crate::runtime::PortMappingEntry>> {
        let dir_name = Self::container_dir_name(id);

        // Report each spawned loopback forwarder as a binding on
        // `127.0.0.1:<container_port>` (host_port == container_port). The host
        // port equals the container port because reachability is via the host
        // loopback forwarder that tunnels over vsock — there is no separate
        // NAT-IP route or distinct host port to advertise.
        let forwarded: Vec<(u16, zlayer_spec::PortProtocol)> = {
            let guard = self.containers.read().await;
            let c = guard.get(&dir_name).ok_or_else(|| AgentError::NotFound {
                container: dir_name.clone(),
                reason: "not found".to_string(),
            })?;
            c.port_forwards
                .iter()
                .map(|f| (f.container_port, f.protocol))
                .collect()
        };

        let out = forwarded
            .into_iter()
            .map(
                |(container_port, protocol)| crate::runtime::PortMappingEntry {
                    container_port,
                    protocol: protocol.as_str().to_string(),
                    host_ip: Some("127.0.0.1".to_string()),
                    host_port: Some(container_port),
                },
            )
            .collect();
        Ok(out)
    }

    async fn kill_container(&self, id: &ContainerId, signal: Option<&str>) -> Result<()> {
        let dir_name = Self::container_dir_name(id);
        // Validate + canonicalise the signal name (defaulting to SIGKILL), then
        // map it to its Linux signal number for the guest agent.
        let canonical = crate::runtime::validate_signal(signal.unwrap_or("SIGKILL"))?;
        let signum = signal_number(&canonical);

        // Snapshot a queue-handle clone of the live VM + whether the workload is
        // still running. Killing an unknown container errors (NotFound) like the
        // other accessors.
        let (live, running) = {
            let guard = self.containers.read().await;
            let c = guard.get(&dir_name).ok_or_else(|| AgentError::NotFound {
                container: dir_name.clone(),
                reason: "not found".to_string(),
            })?;
            let live = c.live.as_ref().map(|l| LiveVm {
                queue: l.queue.clone(),
                vm: Arc::clone(&l.vm),
            });
            let running = c
                .outcome
                .terminal
                .lock()
                .ok()
                .and_then(|g| g.clone())
                .is_none();
            (live, running)
        };

        // If there is a live agent and the workload hasn't already exited, send
        // the real signal to the guest. A terminating signal (SIGKILL/SIGTERM/…)
        // will cause the workload to exit, which the drain task records; we then
        // tear the VM down via `stop_container` so no VM/tasks/fds leak.
        match live {
            Some(live) if running => {
                let deliver =
                    tokio::task::spawn_blocking(move || signal_agent(&live, signum)).await;
                match deliver {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => tracing::debug!(
                        container = %dir_name,
                        error = %e,
                        "vz-linux: kill signal not delivered; forcing VM stop"
                    ),
                    Err(e) => tracing::debug!(
                        container = %dir_name,
                        error = %e,
                        "vz-linux: kill signal task panicked; forcing VM stop"
                    ),
                }
            }
            // No live agent (already exited / never started): nothing to signal,
            // fall through to the forced teardown so the VM + tasks are released.
            _ => {}
        }

        // Tear the VM + tasks down (idempotent). `stop_container` re-snapshots
        // and short-circuits if the workload already reached a terminal state.
        self.stop_container(id, Duration::from_secs(0)).await
    }

    async fn write_stdin(&self, id: &ContainerId, data: &[u8]) -> Result<()> {
        let dir_name = Self::container_dir_name(id);
        let guard = self.containers.read().await;
        let c = guard.get(&dir_name).ok_or_else(|| AgentError::NotFound {
            container: dir_name.clone(),
            reason: "not found".to_string(),
        })?;
        // No sender means stdin was already closed (Ctrl-D) or the workload has
        // exited and the drain task dropped the receiver. Surface a clear error
        // rather than silently dropping the bytes.
        let tx = c.stdin_tx.as_ref().ok_or_else(|| {
            AgentError::Internal(format!("stdin is closed for container {dir_name}"))
        })?;
        tx.send(data.to_vec()).map_err(|_| {
            AgentError::Internal(format!(
                "stdin receiver gone for container {dir_name} (workload exited?)"
            ))
        })?;
        Ok(())
    }

    async fn close_stdin(&self, id: &ContainerId) -> Result<()> {
        let dir_name = Self::container_dir_name(id);
        let mut guard = self.containers.write().await;
        let c = guard
            .get_mut(&dir_name)
            .ok_or_else(|| AgentError::NotFound {
                container: dir_name.clone(),
                reason: "not found".to_string(),
            })?;
        // Dropping the sender makes the drain task's `recv()` return `Err`, which
        // is exactly how `run_and_drain` decides to emit the final
        // `Msg::StdinEof`. Idempotent: clearing an already-`None` sender is fine.
        c.stdin_tx = None;
        Ok(())
    }

    async fn list_images(&self) -> Result<Vec<crate::runtime::ImageInfo>> {
        let images = self.image_rootfs.read().await;
        Ok(images
            .keys()
            .map(|reference| crate::runtime::ImageInfo {
                reference: reference.clone(),
                digest: None,
                size_bytes: None,
            })
            .collect())
    }

    async fn pause_container(&self, id: &ContainerId) -> Result<()> {
        // VZ supports live pause/resume (mirrors `macos_vz::VzRuntime`). Clone the
        // live VM's queue + Arc handles for the transient op rather than moving
        // the `LiveVm` out of the record.
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
        // A second queue-handle clone so we can re-seed the guest clock after the
        // resume without moving `live` out of the resume op.
        let live_for_settime = LiveVm {
            queue: live.queue.clone(),
            vm: Arc::clone(&live.vm),
        };
        tokio::task::spawn_blocking(move || run_vm_lifecycle(&live, VmLifecycleOp::Resume))
            .await
            .map_err(|e| AgentError::Internal(format!("resume task: {e}")))?
            .map_err(|e| AgentError::Internal(format!("resume: {e}")))?;

        // Best-effort wall-clock resync right after resume. A paused/resumed VZ
        // guest's clock froze while paused (and never resyncs on its own), so
        // the host re-seeds it immediately rather than waiting for the next
        // periodic tick. Failures are warn-level — the periodic task retries.
        match tokio::task::spawn_blocking(move || push_settime(&live_for_settime)).await {
            Ok(Ok(acked)) => tracing::debug!(
                container = %dir_name,
                guest_acked = acked,
                "vz-linux: pushed wall-clock resync to guest after resume"
            ),
            Ok(Err(e)) => tracing::warn!(
                container = %dir_name,
                error = %e,
                "vz-linux: post-resume wall-clock resync failed; periodic task will retry"
            ),
            Err(e) => tracing::warn!(
                container = %dir_name,
                error = %e,
                "vz-linux: post-resume wall-clock resync task panicked; periodic task will retry"
            ),
        }
        Ok(())
    }
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
            sanitize_image_name("docker.io/library/alpine:3.19"),
            "docker.io_library_alpine_3.19"
        );
        // Digest refs: both `@` and `:` collapse to `_`.
        assert_eq!(
            sanitize_image_name("ghcr.io/o/r@sha256:abc"),
            "ghcr.io_o_r_sha256_abc"
        );
    }

    /// The store key is the LITERAL user ref — NO canonicalization, NO silent
    /// docker.io injection. `alpine:latest` keys `alpine_latest`, NOT
    /// `docker.io_library_alpine_latest`. A bare name and its docker.io-qualified
    /// form are DIFFERENT strings → DIFFERENT keys, exactly as the user requires.
    /// (Cross-spelling reuse of an already-extracted rootfs is handled at lookup
    /// time by `resolve_existing_rootfs`, not by rewriting the key.)
    #[test]
    fn sanitize_image_name_is_literal_no_docker_hub_injection() {
        assert_eq!(sanitize_image_name("alpine"), "alpine");
        assert_eq!(sanitize_image_name("alpine:latest"), "alpine_latest");
        assert_eq!(
            sanitize_image_name("library/alpine:latest"),
            "library_alpine_latest"
        );
        assert_eq!(
            sanitize_image_name("docker.io/library/alpine:latest"),
            "docker.io_library_alpine_latest"
        );
        // Bare and qualified are DIFFERENT keys.
        assert_ne!(
            sanitize_image_name("alpine:latest"),
            sanitize_image_name("docker.io/library/alpine:latest"),
        );
    }

    /// A `ServiceSpec` built from the raw shorthand (`image: alpine:latest`)
    /// keys the LITERAL `alpine_latest` directory via the SAME
    /// `spec.image.name.to_string()` -> `sanitize_image_name` chain that both
    /// `pull_image_with_policy` and `create_container` use — so pull and create
    /// agree on the key with no canonicalization.
    #[test]
    fn service_spec_shorthand_keys_literal_image_dir() {
        let spec = ServiceSpec::minimal("svc", "alpine:latest");
        assert_eq!(
            sanitize_image_name(&spec.image.name.to_string()),
            "alpine_latest",
        );
    }

    /// `resolve_existing_rootfs` reuses an already-extracted rootfs across
    /// equivalent spellings WITHOUT rewriting the literal key: a rootfs
    /// extracted under the bare `alpine_latest` is found when later referenced by
    /// the fully-qualified `docker.io/library/alpine:latest` (the safe
    /// qualified->bare direction, via `image_ref_candidates`' prefix-stripping).
    /// It NEVER injects docker.io for a bare lookup.
    #[test]
    fn resolve_existing_rootfs_finds_cross_spelling() {
        let tmp = std::env::temp_dir().join(format!(
            "zlayer-vz-resolve-test-{}-{}",
            std::process::id(),
            "alpine"
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        let rt = VzLinuxRuntime {
            data_dir: tmp.clone(),
            log_dir: tmp.join("logs"),
            containers: Arc::new(RwLock::new(HashMap::new())),
            image_rootfs: Arc::new(RwLock::new(HashMap::new())),
        };
        // Extract a rootfs under the bare literal key `alpine_latest`.
        let bare_rootfs = rt.images_dir().join("alpine_latest").join("rootfs");
        std::fs::create_dir_all(bare_rootfs.join("bin")).unwrap();
        std::fs::write(bare_rootfs.join("bin").join("sh"), b"#!/bin/sh\n").unwrap();

        // Found directly by the bare ref.
        assert_eq!(
            rt.resolve_existing_rootfs("alpine:latest"),
            Some(bare_rootfs.clone())
        );
        // Found by the fully-qualified ref via prefix-stripping candidates.
        assert_eq!(
            rt.resolve_existing_rootfs("docker.io/library/alpine:latest"),
            Some(bare_rootfs)
        );
        // A different image is NOT matched.
        assert_eq!(rt.resolve_existing_rootfs("nginx:latest"), None);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn rootfs_is_populated_distinguishes_empty_from_nonempty() {
        let tmp =
            std::env::temp_dir().join(format!("zlayer-vzlinux-rootfs-test-{}", std::process::id()));
        // Image-dir layout: <image>/rootfs + sibling <image>/rootfs.complete.
        let empty = tmp.join("img-empty").join("rootfs");
        let full = tmp.join("img-full").join("rootfs");
        let truncated = tmp.join("img-truncated").join("rootfs");
        std::fs::create_dir_all(&empty).unwrap();
        std::fs::create_dir_all(full.join("bin")).unwrap();
        std::fs::write(full.join("bin").join("sh"), b"#!/bin/sh\n").unwrap();
        std::fs::write(rootfs_complete_marker(&full).unwrap(), b"").unwrap();
        // Non-empty but NO completion marker: the truncated-unpack residue
        // that used to be cached and reused forever.
        std::fs::create_dir_all(truncated.join("etc")).unwrap();
        std::fs::write(truncated.join("etc").join("hostname"), b"x\n").unwrap();

        // Missing dir, empty dir, marker-less dir -> not populated;
        // non-empty dir WITH marker -> populated.
        assert!(!rootfs_is_populated(&tmp.join("does-not-exist")));
        assert!(!rootfs_is_populated(&empty));
        assert!(!rootfs_is_populated(&truncated));
        assert!(rootfs_is_populated(&full));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn spec_defaults_are_sane() {
        // Linux-guest defaults: 2 vCPU, 512 MiB floor 128.
        let spec = ServiceSpec::minimal("svc", "docker.io/library/alpine:3.19");
        assert_eq!(spec_vcpus(&spec, 2), 2);
        assert!(spec_memory_mib(&spec, 512, 128) >= 128);
    }

    #[test]
    fn linux_cmdline_routes_console_and_marks_virtiofs_root() {
        // console=hvc0 wires the kernel console to console.log; rootfstag=rootfs
        // is the virtiofs marker the initramfs interprets (Phase 3).
        assert!(LINUX_CMDLINE.contains("console=hvc0"));
        assert!(LINUX_CMDLINE.contains("rootfstag=rootfs"));
        assert!(!LINUX_CMDLINE.contains("root=/dev/vda"));
    }

    #[test]
    fn linux_cmdline_injects_recent_boottime() {
        // The guest has no RTC and seeds its clock from `zlayer.boottime=` at
        // boot, so the built cmdline must carry the host's current epoch. Build
        // it from `now` and assert the embedded value is a fresh epoch.
        let now = std::time::SystemTime::now();
        let now_secs = now
            .duration_since(std::time::UNIX_EPOCH)
            .expect("host clock is after UNIX_EPOCH")
            .as_secs();

        let cmdline = build_linux_cmdline(now);
        // The static base must still be present (the boottime is appended).
        assert!(
            cmdline.contains("console=hvc0"),
            "cmdline lost its static base: {cmdline}"
        );

        // Parse the `zlayer.boottime=<unix_secs>` token's value.
        let token = cmdline
            .split_whitespace()
            .find_map(|t| t.strip_prefix("zlayer.boottime="))
            .unwrap_or_else(|| panic!("cmdline missing zlayer.boottime=: {cmdline}"));
        let boottime: u64 = token
            .parse()
            .unwrap_or_else(|e| panic!("zlayer.boottime not an integer ({token:?}): {e}"));

        // It must equal the epoch we passed in (within a tight window — the
        // helper reads no clock of its own, but allow a small slack for the
        // `SystemTime::now()` captured above vs. this assert).
        let drift = boottime.abs_diff(now_secs);
        assert!(
            drift <= 300,
            "boottime {boottime} drifted {drift}s from now {now_secs} (>300s)"
        );
    }

    #[test]
    fn linux_cmdline_falls_back_when_clock_pre_epoch() {
        // A system clock before 1970 makes `duration_since(UNIX_EPOCH)` err; the
        // helper must then fall back to the static cmdline (no boottime token)
        // rather than panic — the guest stays at epoch and a later `Msg::SetTime`
        // resync corrects it.
        let pre_epoch = std::time::UNIX_EPOCH - Duration::from_secs(1);
        let cmdline = build_linux_cmdline(pre_epoch);
        assert_eq!(cmdline, LINUX_CMDLINE);
        assert!(!cmdline.contains("zlayer.boottime="));
    }

    #[test]
    fn not_yet_sentinel_is_configuration_error() {
        let err = not_yet("exec");
        match err {
            AgentError::Configuration(msg) => {
                assert!(msg.contains("vz-linux"));
                assert!(msg.contains("exec"));
                assert!(msg.contains("later phase"));
            }
            other => panic!("expected Configuration sentinel, got {other:?}"),
        }
    }

    #[test]
    fn parse_user_handles_uid_gid_forms() {
        use macos_vz_shared::parse_user;
        assert_eq!(parse_user(None), (0, 0));
        assert_eq!(parse_user(Some("")), (0, 0));
        // uid only -> gid defaults to uid.
        assert_eq!(parse_user(Some("1000")), (1000, 1000));
        assert_eq!(parse_user(Some("1000:2000")), (1000, 2000));
        // Non-numeric names aren't host-resolvable -> fall back to root.
        assert_eq!(parse_user(Some("alice")), (0, 0));
        assert_eq!(parse_user(Some("alice:staff")), (0, 0));
    }

    #[test]
    fn parse_exec_user_defers_names_to_guest() {
        // Empty / none -> keep guest identity.
        assert_eq!(parse_exec_user(None), (None, None, None));
        assert_eq!(parse_exec_user(Some("")), (None, None, None));
        assert_eq!(parse_exec_user(Some("  ")), (None, None, None));
        // Numeric uid only -> numeric, no name.
        assert_eq!(parse_exec_user(Some("1000")), (Some(1000), None, None));
        // Numeric uid:gid -> both numeric, no name.
        assert_eq!(
            parse_exec_user(Some("1000:2000")),
            (Some(1000), Some(2000), None)
        );
        // Pure NAME -> deferred to guest verbatim, no numeric halves.
        assert_eq!(
            parse_exec_user(Some("git")),
            (None, None, Some("git".to_string()))
        );
        // name:group -> deferred (both names need /etc/passwd + /etc/group).
        assert_eq!(
            parse_exec_user(Some("git:git")),
            (None, None, Some("git:git".to_string()))
        );
        // numeric uid : NAME group -> uid kept numeric, raw deferred so the
        // guest resolves the group name.
        assert_eq!(
            parse_exec_user(Some("1000:staff")),
            (Some(1000), None, Some("1000:staff".to_string()))
        );
    }

    #[test]
    fn signal_number_maps_canonical_names_and_defaults_to_sigkill() {
        // The exact canonical names `validate_signal` emits map to their Linux
        // numbers (the guest is always Linux).
        assert_eq!(signal_number("SIGHUP"), 1);
        assert_eq!(signal_number("SIGINT"), 2);
        assert_eq!(signal_number("SIGKILL"), 9);
        assert_eq!(signal_number("SIGUSR1"), 10);
        assert_eq!(signal_number("SIGUSR2"), 12);
        assert_eq!(signal_number("SIGTERM"), 15);
        // Case-insensitive / surrounding whitespace are tolerated.
        assert_eq!(signal_number(" sigterm "), 15);
        // Anything unrecognised falls back to SIGKILL so a kill is never a no-op.
        assert_eq!(signal_number("SIGWINCH"), 9);
        assert_eq!(signal_number(""), 9);
    }

    #[test]
    fn signal_number_roundtrips_validate_signal_output() {
        // Every signal `validate_signal` accepts must map to a non-zero, sane
        // number — kill_container feeds validate_signal's canonical output here.
        for s in [
            "SIGKILL", "SIGTERM", "SIGINT", "SIGHUP", "SIGUSR1", "SIGUSR2",
        ] {
            let canonical = crate::runtime::validate_signal(s).expect("known signal");
            let n = signal_number(&canonical);
            assert!(n > 0, "{s} mapped to non-positive {n}");
        }
    }

    #[test]
    fn build_run_message_carries_entrypoint_env_and_user() {
        let mut spec = ServiceSpec::minimal("svc", "docker.io/library/alpine:latest");
        spec.command.entrypoint = Some(vec!["echo".to_string()]);
        spec.command.args = Some(vec!["hello".to_string()]);
        spec.command.workdir = Some("/app".to_string());
        spec.env.insert("FOO".to_string(), "bar".to_string());
        spec.user = Some("1000:1000".to_string());

        match build_run_message(&spec, None) {
            proto::Msg::Run {
                argv,
                env,
                cwd,
                uid,
                gid,
            } => {
                assert_eq!(argv, vec!["echo".to_string(), "hello".to_string()]);
                assert_eq!(cwd, Some("/app".to_string()));
                assert_eq!(uid, 1000);
                assert_eq!(gid, 1000);
                assert!(env.contains(&("FOO".to_string(), "bar".to_string())));
            }
            other => panic!("expected Msg::Run, got {other:?}"),
        }
    }

    #[test]
    fn derive_bind_mounts_maps_only_binds_with_sequential_tags() {
        // Only `StorageSpec::Bind` entries become host->guest virtiofs shares,
        // tagged `zlmnt{i}` in declaration order. Other storage kinds
        // (tmpfs/named/anonymous/s3) are NOT host-dir shares and are skipped, so
        // tag indices count only the binds, keeping host shares <-> Mount frames
        // in lockstep with `build_config_linux`.
        let mut spec = ServiceSpec::minimal("svc", "docker.io/library/alpine:latest");
        spec.storage = vec![
            zlayer_spec::StorageSpec::Bind {
                source: "/host/a".to_string(),
                target: "/work".to_string(),
                readonly: false,
            },
            zlayer_spec::StorageSpec::Tmpfs {
                target: "/tmp".to_string(),
                size: None,
                mode: None,
            },
            zlayer_spec::StorageSpec::Bind {
                source: "/host/b".to_string(),
                target: "/data".to_string(),
                readonly: true,
            },
        ];

        let binds = derive_bind_mounts(&spec);
        assert_eq!(binds.len(), 2, "only the two Bind entries map to shares");

        assert_eq!(binds[0].host, PathBuf::from("/host/a"));
        assert_eq!(binds[0].tag, "zlmnt0");
        assert_eq!(binds[0].target, "/work");
        assert!(!binds[0].readonly);

        // The tmpfs entry between the binds must NOT consume a tag index.
        assert_eq!(binds[1].host, PathBuf::from("/host/b"));
        assert_eq!(binds[1].tag, "zlmnt1");
        assert_eq!(binds[1].target, "/data");
        assert!(binds[1].readonly);
    }

    #[test]
    fn derive_bind_mounts_empty_when_no_binds() {
        // A spec with no Bind storage yields no extra virtiofs shares (only the
        // rootfs share is attached by build_config_linux).
        let spec = ServiceSpec::minimal("svc", "docker.io/library/alpine:latest");
        assert!(derive_bind_mounts(&spec).is_empty());
    }

    #[test]
    fn capture_chunk_records_stream_into_outcome_and_console() {
        // Regression for the "empty logs" symptom: a workload `Stdout`/`Stderr`
        // frame drained by `run_and_drain` must land in `RunOutcome::logs` (what
        // `captured_logs`/`get_logs` read) AND be mirrored to console.log.
        let outcome = RunOutcome::default();
        let tmp = std::env::temp_dir().join(format!(
            "vzl-capture-{}-{}.log",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let _ = std::fs::remove_file(&tmp);

        capture_chunk(&outcome, &tmp, "svc-0", LogStream::Stdout, b"HELLO\n");
        capture_chunk(&outcome, &tmp, "svc-0", LogStream::Stderr, b"oops\n");

        let logs = outcome.logs.lock().expect("logs lock");
        assert_eq!(logs.len(), 2, "both chunks must be captured");
        assert_eq!(logs[0].stream, LogStream::Stdout);
        assert!(logs[0].message.contains("HELLO"));
        assert_eq!(logs[1].stream, LogStream::Stderr);
        assert!(logs[1].message.contains("oops"));

        let on_disk = std::fs::read_to_string(&tmp).unwrap_or_default();
        assert!(
            on_disk.contains("HELLO") && on_disk.contains("oops"),
            "captured output must also be mirrored to console.log; got {on_disk:?}"
        );
        let _ = std::fs::remove_file(&tmp);
    }

    /// Regression for the "exit code dropped on the state path" bug: a workload
    /// that exits non-zero (e.g. `sh -c "exit 42"`) had `wait_container` report
    /// 42 (from `RunOutcome::exit_code`) while `container_state` returned
    /// `Exited { code: 0 }` — the captured code was never surfaced on the state
    /// path. This drives the exact capture -> stored-outcome -> state mapping
    /// `run_and_drain`/`container_state` use, with no live VM.
    #[test]
    fn captured_exit_code_persists_into_container_state() {
        // 1. Simulate the workload exiting non-zero exactly as the `Exited`
        //    frame handler in `run_and_drain` does.
        let outcome = RunOutcome::default();
        record_exit(&outcome, 42);

        // The wait path reads `exit_code` directly: it already saw 42.
        assert_eq!(
            outcome.exit_code.lock().expect("exit_code lock").as_ref(),
            Some(&42),
            "wait path: captured exit code must be the real workload code"
        );

        // The state path (`container_state`) maps the captured outcome to a
        // `ContainerState`. With the bug it dropped the code and returned 0.
        let terminal = outcome.terminal.lock().expect("terminal lock").clone();
        let captured = outcome
            .exit_code
            .lock()
            .expect("exit_code lock")
            .as_ref()
            .copied();
        let state = outcome_to_state(captured, terminal, ContainerState::Running);
        assert_eq!(
            state,
            ContainerState::Exited { code: 42 },
            "state path must surface the captured non-zero exit code, not 0"
        );

        // 2. The success case is unaffected: exit 0 -> Exited { code: 0 }.
        let outcome_ok = RunOutcome::default();
        record_exit(&outcome_ok, 0);
        let terminal_ok = outcome_ok.terminal.lock().expect("terminal lock").clone();
        let captured_ok = outcome_ok
            .exit_code
            .lock()
            .expect("exit_code lock")
            .as_ref()
            .copied();
        assert_eq!(
            outcome_to_state(captured_ok, terminal_ok, ContainerState::Running),
            ContainerState::Exited { code: 0 },
            "exit 0 must map to Exited {{ code: 0 }}"
        );

        // 3. A signal death (e.g. SIGKILL -> 137) maps the same way the wait path
        //    sees it: the guest sends `Exited { code: 128 + signum }`.
        let outcome_sig = RunOutcome::default();
        record_exit(&outcome_sig, 137);
        let terminal_sig = outcome_sig.terminal.lock().expect("terminal lock").clone();
        let captured_sig = outcome_sig
            .exit_code
            .lock()
            .expect("exit_code lock")
            .as_ref()
            .copied();
        assert_eq!(
            outcome_to_state(captured_sig, terminal_sig, ContainerState::Running),
            ContainerState::Exited { code: 137 },
            "SIGKILL death must surface 137 on the state path"
        );
    }

    /// `outcome_to_state` precedence: a captured `exit_code` must win over the
    /// caller's fallback (the live-VM reconcile / forced-stop default), a richer
    /// `terminal` state wins outright, and the fallback is used only when the
    /// workload's outcome is wholly unknown.
    #[test]
    fn outcome_to_state_prefers_captured_outcome_over_fallback() {
        // Captured code alone (the race window: `exit_code` set before
        // `terminal`) — must NOT fall through to a fabricated 0 / Running.
        assert_eq!(
            outcome_to_state(Some(42), None, ContainerState::Running),
            ContainerState::Exited { code: 42 },
        );
        assert_eq!(
            outcome_to_state(Some(7), None, ContainerState::Exited { code: 0 }),
            ContainerState::Exited { code: 7 },
            "a captured code must override a forced-stop `Exited {{ code: 0 }}` fallback"
        );

        // A recorded terminal state wins verbatim, even alongside a code.
        let failed = ContainerState::Failed {
            reason: "agent error".to_string(),
        };
        assert_eq!(
            outcome_to_state(Some(0), Some(failed.clone()), ContainerState::Running),
            failed,
        );

        // Nothing captured -> the fallback is used (e.g. the VM is still up).
        assert_eq!(
            outcome_to_state(None, None, ContainerState::Running),
            ContainerState::Running,
        );
    }

    #[test]
    fn build_run_message_defaults_to_root_and_true() {
        // A bare spec: no entrypoint/args -> `true`, no user -> root.
        let spec = ServiceSpec::minimal("svc", "docker.io/library/alpine:latest");
        match build_run_message(&spec, None) {
            proto::Msg::Run { argv, uid, gid, .. } => {
                assert_eq!(argv, vec!["true".to_string()]);
                assert_eq!((uid, gid), (0, 0));
            }
            other => panic!("expected Msg::Run, got {other:?}"),
        }
    }

    #[test]
    fn build_run_message_applies_image_entrypoint_and_env() {
        // The core fix: a spec with NO command picks up the image's default
        // Entrypoint+Cmd and Env (the postgres/forgejo case) instead of `true`.
        let spec = ServiceSpec::minimal("svc", "docker.io/library/postgres:16-alpine");
        let image = zlayer_registry::ImageConfig {
            entrypoint: Some(vec!["docker-entrypoint.sh".to_string()]),
            cmd: Some(vec!["postgres".to_string()]),
            env: Some(vec!["PATH=/usr/local/bin:/usr/bin".to_string()]),
            working_dir: Some("/var/lib/postgresql".to_string()),
            user: Some("70:70".to_string()),
            ..Default::default()
        };
        match build_run_message(&spec, Some(&image)) {
            proto::Msg::Run {
                argv,
                env,
                cwd,
                uid,
                gid,
            } => {
                assert_eq!(
                    argv,
                    vec!["docker-entrypoint.sh".to_string(), "postgres".to_string()]
                );
                assert!(env.contains(&("PATH".to_string(), "/usr/local/bin:/usr/bin".to_string())));
                assert_eq!(cwd, Some("/var/lib/postgresql".to_string()));
                assert_eq!((uid, gid), (70, 70));
            }
            other => panic!("expected Msg::Run, got {other:?}"),
        }
    }

    #[test]
    fn virtiofs_tag_matches_cmdline_rootfstag() {
        // The virtiofs share tag the host exports MUST equal the `rootfstag=`
        // the guest agent reads from the kernel command line, or the guest can't
        // find the share.
        assert!(LINUX_CMDLINE.contains(&format!("rootfstag={VIRTIOFS_ROOTFS_TAG}")));
    }

    #[tokio::test]
    async fn ensure_linux_kernel_errors_clearly_when_absent() {
        // With neither the env override, a successful pull, nor a populated
        // cache, the error must name both escape hatches, the cache directory,
        // and the GHCR bundle ref (the new pull-based path).
        let rt = VzLinuxRuntime::new(None).expect("construct VzLinuxRuntime");
        // Guard against a populated cache or env on the dev box: only assert the
        // error shape when resolution actually fails. (A reachable network with a
        // public bundle would also succeed; in that case skip — we only assert
        // the failure wording.)
        if std::env::var_os("ZLAYER_VZ_LINUX_KERNEL").is_some()
            && std::env::var_os("ZLAYER_VZ_LINUX_INITRD").is_some()
        {
            return;
        }
        let cache = rt.kernel_cache_dir();
        if cache.join("Image").exists() && cache.join("initramfs.cpio.gz").exists() {
            return;
        }
        match rt.ensure_linux_kernel().await {
            Err(AgentError::Configuration(msg)) => {
                assert!(msg.contains("ZLAYER_VZ_LINUX_KERNEL/_INITRD"));
                assert!(msg.contains("Image+initramfs.cpio.gz"));
                assert!(msg.contains(VZ_LINUX_BUNDLE_IMAGE));
                assert!(msg.contains("must be public"));
            }
            // A reachable, public bundle pulled successfully — nothing to assert.
            Ok(_) => {}
            other => panic!("expected Configuration error or Ok, got {other:?}"),
        }
    }

    #[test]
    fn kernel_bundle_needs_extract_decision_matrix() {
        // Missing either artifact always forces an extract, regardless of digest.
        assert!(kernel_bundle_needs_extract(
            Some("sha256:a"),
            Some("sha256:a"),
            false,
            true
        ));
        assert!(kernel_bundle_needs_extract(
            Some("sha256:a"),
            Some("sha256:a"),
            true,
            false
        ));
        assert!(kernel_bundle_needs_extract(None, None, false, false));

        // Both present + matching digest ⇒ reuse.
        assert!(!kernel_bundle_needs_extract(
            Some("sha256:a"),
            Some("sha256:a"),
            true,
            true
        ));

        // Both present + differing (or missing) recorded digest ⇒ extract.
        assert!(kernel_bundle_needs_extract(
            Some("sha256:b"),
            Some("sha256:a"),
            true,
            true
        ));
        assert!(kernel_bundle_needs_extract(
            Some("sha256:b"),
            None,
            true,
            true
        ));

        // Unknown remote digest but both files present ⇒ reuse (don't churn).
        assert!(!kernel_bundle_needs_extract(
            None,
            Some("sha256:a"),
            true,
            true
        ));
        assert!(!kernel_bundle_needs_extract(None, None, true, true));
    }

    #[test]
    fn mac_roundtrip_preserves_address_and_rejects_garbage() {
        // A real locally-administered MAC must parse and re-stringify to the
        // SAME logical address (case-insensitively): this is the exact parse
        // `build_config_linux` performs to pin the virtio-net MAC, and the
        // host-side DHCP-lease lookup is keyed by that string. Compare via the
        // shared MAC normaliser so framework canonicalisation (case/padding)
        // doesn't make a correct round-trip look like a mismatch.
        let original = unsafe {
            VZMACAddress::randomLocallyAdministeredAddress()
                .string()
                .to_string()
        };
        let round = mac_roundtrip(&original).expect("valid MAC must round-trip");
        assert_eq!(
            macos_vz_shared::normalize_mac_for_test(&round),
            macos_vz_shared::normalize_mac_for_test(&original),
            "round-tripped MAC must match the original (modulo case/padding)"
        );
        // A fixed, known-good MAC also round-trips to the same address.
        let fixed = mac_roundtrip("0a:1b:2c:3d:4e:5f").expect("fixed MAC must parse");
        assert_eq!(
            macos_vz_shared::normalize_mac_for_test(&fixed),
            "a:1b:2c:3d:4e:5f"
        );
        // Garbage is rejected (None), so we never pin a bogus MAC.
        assert!(mac_roundtrip("not-a-mac").is_none());
        assert!(mac_roundtrip("").is_none());
    }

    /// Boot a real Linux guest to the serial console and assert it reaches
    /// `Running` and writes to `console.log`.
    ///
    /// `#[ignore]`d: requires the `com.apple.security.virtualization` entitlement
    /// (a code-signed binary) AND a guest kernel supplied via
    /// `ZLAYER_VZ_LINUX_KERNEL` (raw arm64 `Image`) + `ZLAYER_VZ_LINUX_INITRD`
    /// (`initramfs.cpio.gz`). For Phase 2 the initramfs alone boots and prints a
    /// banner; the real rootfs (virtiofs) is wired in Phase 3. Run locally on an
    /// entitled build with:
    /// `ZLAYER_VZ_LINUX_KERNEL=/path/Image ZLAYER_VZ_LINUX_INITRD=/path/initramfs.cpio.gz \
    ///  cargo test -p zlayer-agent vz_linux_boots_to_console -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "needs the virtualization entitlement + a guest kernel (ZLAYER_VZ_LINUX_KERNEL/_INITRD)"]
    async fn vz_linux_boots_to_console() {
        let (Some(_kernel), Some(_initrd)) = (
            std::env::var_os("ZLAYER_VZ_LINUX_KERNEL"),
            std::env::var_os("ZLAYER_VZ_LINUX_INITRD"),
        ) else {
            eprintln!("ZLAYER_VZ_LINUX_KERNEL/_INITRD unset — skipping");
            return;
        };
        assert!(
            unsafe { VZVirtualMachine::isSupported() },
            "Virtualization.framework not supported (missing entitlement?)"
        );

        let rt = VzLinuxRuntime::new(None).expect("construct VzLinuxRuntime");
        let id = ContainerId::new("vzlinuxtest", 1);
        let spec = ServiceSpec::minimal("vzlinuxtest", "docker.io/library/alpine:3.19");

        // The guest shares the image rootfs over virtiofs, so the image must be
        // pulled before create_container (which records the rootfs path).
        rt.pull_image("docker.io/library/alpine:3.19")
            .await
            .expect("pull alpine");
        rt.create_container(&id, &spec).await.expect("create");
        rt.start_container(&id).await.expect("start");

        // The VM should report Running.
        let state = rt.container_state(&id).await.expect("state");
        assert_eq!(state, ContainerState::Running, "VM should be Running");

        // The guest serial console should produce output within the timeout.
        let mut wrote = false;
        for _ in 0..60 {
            let logs = rt.container_logs(&id, 200).await.unwrap_or_default();
            if logs.iter().any(|l| !l.message.trim().is_empty()) {
                wrote = true;
                break;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        assert!(wrote, "guest never wrote to the serial console");

        rt.stop_container(&id, Duration::from_secs(10))
            .await
            .expect("stop");
        rt.remove_container(&id).await.expect("remove");
    }

    /// Full Phase 3+4 path: pull alpine, share its rootfs over virtiofs, run
    /// `echo hello` through the in-guest vsock agent, and assert the captured
    /// logs contain `hello` and the workload exits 0.
    ///
    /// `#[ignore]`d: needs a code-signed (entitled) binary, the guest kernel +
    /// initramfs (with the `zlayer-vzagent` PID1 baked in) via
    /// `ZLAYER_VZ_LINUX_KERNEL` / `_INITRD`, and network access to pull alpine.
    /// Run locally with:
    /// `ZLAYER_VZ_LINUX_KERNEL=/path/Image ZLAYER_VZ_LINUX_INITRD=/path/initramfs.cpio.gz \
    ///  cargo test -p zlayer-agent vz_linux_run_echo -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "needs the virtualization entitlement + vzagent kernel/initrd + alpine pull"]
    async fn vz_linux_run_echo() {
        let (Some(_kernel), Some(_initrd)) = (
            std::env::var_os("ZLAYER_VZ_LINUX_KERNEL"),
            std::env::var_os("ZLAYER_VZ_LINUX_INITRD"),
        ) else {
            eprintln!("ZLAYER_VZ_LINUX_KERNEL/_INITRD unset — skipping");
            return;
        };
        assert!(
            unsafe { VZVirtualMachine::isSupported() },
            "Virtualization.framework not supported (missing entitlement?)"
        );

        let rt = VzLinuxRuntime::new(None).expect("construct VzLinuxRuntime");
        let id = ContainerId::new("vzlinuxecho", 1);
        let mut spec = ServiceSpec::minimal("vzlinuxecho", "docker.io/library/alpine:latest");
        spec.command.entrypoint = Some(vec!["echo".to_string(), "hello".to_string()]);

        rt.pull_image("docker.io/library/alpine:latest")
            .await
            .expect("pull alpine");
        rt.create_container(&id, &spec).await.expect("create");
        rt.start_container(&id).await.expect("start");

        // Wait for the workload to exit (the drain task records the code).
        let code = tokio::time::timeout(Duration::from_secs(120), rt.wait_container(&id))
            .await
            .expect("wait_container timed out")
            .expect("wait_container");
        assert_eq!(code, 0, "echo should exit 0");

        let logs = rt.get_logs(&id).await.expect("logs");
        assert!(
            logs.iter().any(|l| l.message.contains("hello")),
            "captured logs should contain 'hello', got: {:?}",
            logs.iter().map(|l| &l.message).collect::<Vec<_>>()
        );

        rt.stop_container(&id, Duration::from_secs(10))
            .await
            .expect("stop");
        rt.remove_container(&id).await.expect("remove");
    }

    /// Asserts the real exit code propagates: `sh -c "exit 7"` -> 7.
    ///
    /// `#[ignore]`d for the same reasons as [`vz_linux_run_echo`].
    #[tokio::test]
    #[ignore = "needs the virtualization entitlement + vzagent kernel/initrd + alpine pull"]
    async fn vz_linux_run_exit_code() {
        let (Some(_kernel), Some(_initrd)) = (
            std::env::var_os("ZLAYER_VZ_LINUX_KERNEL"),
            std::env::var_os("ZLAYER_VZ_LINUX_INITRD"),
        ) else {
            eprintln!("ZLAYER_VZ_LINUX_KERNEL/_INITRD unset — skipping");
            return;
        };
        assert!(
            unsafe { VZVirtualMachine::isSupported() },
            "Virtualization.framework not supported (missing entitlement?)"
        );

        let rt = VzLinuxRuntime::new(None).expect("construct VzLinuxRuntime");
        let id = ContainerId::new("vzlinuxexit", 1);
        let mut spec = ServiceSpec::minimal("vzlinuxexit", "docker.io/library/alpine:latest");
        spec.command.entrypoint = Some(vec![
            "sh".to_string(),
            "-c".to_string(),
            "exit 7".to_string(),
        ]);

        rt.pull_image("docker.io/library/alpine:latest")
            .await
            .expect("pull alpine");
        rt.create_container(&id, &spec).await.expect("create");
        rt.start_container(&id).await.expect("start");

        let code = tokio::time::timeout(Duration::from_secs(120), rt.wait_container(&id))
            .await
            .expect("wait_container timed out")
            .expect("wait_container");
        assert_eq!(code, 7, "sh -c 'exit 7' should propagate exit code 7");

        rt.stop_container(&id, Duration::from_secs(10))
            .await
            .expect("stop");
        rt.remove_container(&id).await.expect("remove");
    }

    /// Phase 5: `exec` into a running guest over a SECOND vsock connection.
    /// Start a long-running container (`sleep 600`), then `exec ["echo","hi"]`
    /// and assert it returns `(0, contains "hi", "")`.
    ///
    /// `#[ignore]`d: needs a code-signed (entitled) binary, the guest kernel +
    /// initramfs (with the `zlayer-vzagent` PID1 baked in) via
    /// `ZLAYER_VZ_LINUX_KERNEL` / `_INITRD`, and network access to pull alpine.
    /// Run locally with:
    /// `ZLAYER_VZ_LINUX_KERNEL=/path/Image ZLAYER_VZ_LINUX_INITRD=/path/initramfs.cpio.gz \
    ///  cargo test -p zlayer-agent vz_linux_exec_echo -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "needs the virtualization entitlement + vzagent kernel/initrd + alpine pull"]
    async fn vz_linux_exec_echo() {
        let (Some(_kernel), Some(_initrd)) = (
            std::env::var_os("ZLAYER_VZ_LINUX_KERNEL"),
            std::env::var_os("ZLAYER_VZ_LINUX_INITRD"),
        ) else {
            eprintln!("ZLAYER_VZ_LINUX_KERNEL/_INITRD unset — skipping");
            return;
        };
        assert!(
            unsafe { VZVirtualMachine::isSupported() },
            "Virtualization.framework not supported (missing entitlement?)"
        );

        let rt = VzLinuxRuntime::new(None).expect("construct VzLinuxRuntime");
        let id = ContainerId::new("vzlinuxexec", 1);
        // A long-running primary workload so a live agent + PID namespace exist
        // to exec into.
        let mut spec = ServiceSpec::minimal("vzlinuxexec", "docker.io/library/alpine:latest");
        spec.command.entrypoint = Some(vec!["sleep".to_string(), "600".to_string()]);

        rt.pull_image("docker.io/library/alpine:latest")
            .await
            .expect("pull alpine");
        rt.create_container(&id, &spec).await.expect("create");
        rt.start_container(&id).await.expect("start");

        // Give the guest time to boot + start the primary workload before exec.
        tokio::time::sleep(Duration::from_secs(15)).await;
        assert_eq!(
            rt.container_state(&id).await.expect("state"),
            ContainerState::Running,
            "container should be running before exec"
        );

        let (code, stdout, stderr) = tokio::time::timeout(
            Duration::from_secs(60),
            rt.exec(&id, &["echo".to_string(), "hi".to_string()]),
        )
        .await
        .expect("exec timed out")
        .expect("exec");

        assert_eq!(code, 0, "echo should exit 0");
        assert!(
            stdout.contains("hi"),
            "exec stdout should contain 'hi', got: {stdout:?}"
        );
        assert_eq!(stderr, "", "exec stderr should be empty, got: {stderr:?}");

        rt.stop_container(&id, Duration::from_secs(10))
            .await
            .expect("stop");
        rt.remove_container(&id).await.expect("remove");
    }

    /// Phase 5: `exec` propagates the real exit code: `sh -c "exit 42"` -> 42.
    ///
    /// `#[ignore]`d for the same reasons as [`vz_linux_exec_echo`].
    #[tokio::test]
    #[ignore = "needs the virtualization entitlement + vzagent kernel/initrd + alpine pull"]
    async fn vz_linux_exec_exit_code() {
        let (Some(_kernel), Some(_initrd)) = (
            std::env::var_os("ZLAYER_VZ_LINUX_KERNEL"),
            std::env::var_os("ZLAYER_VZ_LINUX_INITRD"),
        ) else {
            eprintln!("ZLAYER_VZ_LINUX_KERNEL/_INITRD unset — skipping");
            return;
        };
        assert!(
            unsafe { VZVirtualMachine::isSupported() },
            "Virtualization.framework not supported (missing entitlement?)"
        );

        let rt = VzLinuxRuntime::new(None).expect("construct VzLinuxRuntime");
        let id = ContainerId::new("vzlinuxexeccode", 1);
        let mut spec = ServiceSpec::minimal("vzlinuxexeccode", "docker.io/library/alpine:latest");
        spec.command.entrypoint = Some(vec!["sleep".to_string(), "600".to_string()]);

        rt.pull_image("docker.io/library/alpine:latest")
            .await
            .expect("pull alpine");
        rt.create_container(&id, &spec).await.expect("create");
        rt.start_container(&id).await.expect("start");

        tokio::time::sleep(Duration::from_secs(15)).await;
        assert_eq!(
            rt.container_state(&id).await.expect("state"),
            ContainerState::Running,
            "container should be running before exec"
        );

        let (code, _stdout, _stderr) = tokio::time::timeout(
            Duration::from_secs(60),
            rt.exec(
                &id,
                &["sh".to_string(), "-c".to_string(), "exit 42".to_string()],
            ),
        )
        .await
        .expect("exec timed out")
        .expect("exec");

        assert_eq!(code, 42, "sh -c 'exit 42' should propagate exit code 42");

        rt.stop_container(&id, Duration::from_secs(10))
            .await
            .expect("stop");
        rt.remove_container(&id).await.expect("remove");
    }

    /// Port forwarding: a published container port is reachable from the host
    /// via the vsock loopback forwarder (NOT a guest NAT IP — VZ NAT yields no
    /// usable lease on this macOS). Start an alpine container running a tiny TCP
    /// server on :8080, declare :8080 in the spec so the forwarder binds
    /// `127.0.0.1:8080` on the host, assert `get_container_ip` reports loopback,
    /// assert `port_mappings_container` reports the loopback binding, then
    /// connect to `127.0.0.1:8080` and round-trip the server's banner — which
    /// only succeeds if the host TCP -> vsock `Forward` -> guest 127.0.0.1:8080
    /// tunnel works end to end.
    ///
    /// `#[ignore]`d: needs a code-signed (entitled) binary, the guest kernel +
    /// initramfs (with `zlayer-vzagent` PID1) via `ZLAYER_VZ_LINUX_KERNEL` /
    /// `_INITRD`, and network access to pull alpine. Run locally with:
    /// `ZLAYER_VZ_LINUX_KERNEL=/path/Image ZLAYER_VZ_LINUX_INITRD=/path/initramfs.cpio.gz \
    ///  cargo test -p zlayer-agent vz_linux_guest_gets_ip_and_port_reachable -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "needs the virtualization entitlement + vzagent kernel/initrd + alpine pull"]
    async fn vz_linux_guest_gets_ip_and_port_reachable() {
        use std::io::Read as _;
        use std::net::{IpAddr as StdIpAddr, Ipv4Addr, TcpStream};

        let (Some(_kernel), Some(_initrd)) = (
            std::env::var_os("ZLAYER_VZ_LINUX_KERNEL"),
            std::env::var_os("ZLAYER_VZ_LINUX_INITRD"),
        ) else {
            eprintln!("ZLAYER_VZ_LINUX_KERNEL/_INITRD unset — skipping");
            return;
        };
        assert!(
            unsafe { VZVirtualMachine::isSupported() },
            "Virtualization.framework not supported (missing entitlement?)"
        );

        let rt = VzLinuxRuntime::new(None).expect("construct VzLinuxRuntime");
        let id = ContainerId::new("vzlinuxnet", 1);
        let mut spec = ServiceSpec::minimal("vzlinuxnet", "docker.io/library/alpine:latest");
        // A persistent listener: serve a fixed banner on :8080 forever (busybox
        // `nc -l -p` exits after one connection, so loop it).
        spec.command.entrypoint = Some(vec![
            "sh".to_string(),
            "-c".to_string(),
            "while true; do printf 'HTTP/1.1 200 OK\\r\\n\\r\\nhi' | nc -l -p 8080; done"
                .to_string(),
        ]);
        // Publish :8080 the same way the forwarder reads it (spec.port_mappings).
        // The forwarder binds `127.0.0.1:8080` on the host (host_port ==
        // container_port) and tunnels each connection over vsock to the guest.
        spec.port_mappings = vec![zlayer_spec::PortMapping {
            host_port: Some(8080),
            container_port: 8080,
            protocol: zlayer_spec::PortProtocol::Tcp,
            host_ip: "127.0.0.1".to_string(),
        }];

        rt.pull_image("docker.io/library/alpine:latest")
            .await
            .expect("pull alpine");
        rt.create_container(&id, &spec).await.expect("create");
        rt.start_container(&id).await.expect("start");

        // Reachability is via host loopback (the vsock forwarder), so
        // `get_container_ip` reports 127.0.0.1 while the VM is live.
        let mut ip = None;
        for _ in 0..60 {
            if let Ok(Some(addr)) = rt.get_container_ip(&id).await {
                ip = Some(addr);
                break;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        let ip = ip.expect("get_container_ip never returned an address");
        assert_eq!(
            ip,
            StdIpAddr::V4(Ipv4Addr::LOCALHOST),
            "vz-linux reachability is via host loopback, not a guest NAT IP"
        );

        // `port_mappings_container` must report the loopback binding for :8080.
        let maps = rt.port_mappings_container(&id).await.expect("port maps");
        assert!(
            maps.iter().any(|m| m.container_port == 8080
                && m.host_port == Some(8080)
                && m.host_ip.as_deref() == Some("127.0.0.1")),
            "port_mappings_container should report 127.0.0.1:8080, got: {maps:?}"
        );

        // The in-guest listener may take a moment to bind; retry the connect to
        // the HOST loopback `127.0.0.1:8080` (the forwarder) and assert we read
        // the banner the guest server emits — proving the vsock tunnel works.
        let mut ok = false;
        for _ in 0..30 {
            let connect = tokio::task::spawn_blocking(move || {
                let target = std::net::SocketAddr::new(StdIpAddr::V4(Ipv4Addr::LOCALHOST), 8080);
                let mut s = TcpStream::connect_timeout(&target, Duration::from_secs(2)).ok()?;
                s.set_read_timeout(Some(Duration::from_secs(2))).ok();
                let mut buf = [0u8; 64];
                let n = s.read(&mut buf).ok()?;
                Some(String::from_utf8_lossy(&buf[..n]).into_owned())
            })
            .await
            .expect("connect task");
            if let Some(resp) = connect {
                if resp.contains("200") || resp.contains("hi") {
                    ok = true;
                    break;
                }
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        assert!(
            ok,
            "127.0.0.1:8080 (vsock forwarder) was not reachable / did not serve the banner"
        );

        rt.stop_container(&id, Duration::from_secs(10))
            .await
            .expect("stop");
        rt.remove_container(&id).await.expect("remove");
    }

    /// Phase 7: the full lifecycle through every hardened method — create →
    /// start → logs → exec → stop → remove — asserting the state transitions and
    /// that the container is gone after `remove`.
    ///
    /// Runs a long-lived `sleep 600` primary so a live agent + PID namespace
    /// exist to exec into and to gracefully SIGTERM on stop. After `stop` the
    /// state must be terminal (`Exited`); after `remove` the record is gone, so
    /// `container_state` must error.
    ///
    /// `#[ignore]`d: needs a code-signed (entitled) binary, the guest kernel +
    /// initramfs (with `zlayer-vzagent` PID1) via `ZLAYER_VZ_LINUX_KERNEL` /
    /// `_INITRD`, and network access to pull alpine. Run locally with:
    /// `ZLAYER_VZ_LINUX_KERNEL=/path/Image ZLAYER_VZ_LINUX_INITRD=/path/initramfs.cpio.gz \
    ///  cargo test -p zlayer-agent vz_linux_full_lifecycle -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "needs the virtualization entitlement + vzagent kernel/initrd + alpine pull"]
    async fn vz_linux_full_lifecycle() {
        let (Some(_kernel), Some(_initrd)) = (
            std::env::var_os("ZLAYER_VZ_LINUX_KERNEL"),
            std::env::var_os("ZLAYER_VZ_LINUX_INITRD"),
        ) else {
            eprintln!("ZLAYER_VZ_LINUX_KERNEL/_INITRD unset — skipping");
            return;
        };
        assert!(
            unsafe { VZVirtualMachine::isSupported() },
            "Virtualization.framework not supported (missing entitlement?)"
        );

        let rt = VzLinuxRuntime::new(None).expect("construct VzLinuxRuntime");
        let id = ContainerId::new("vzlinuxlifecycle", 1);
        let mut spec = ServiceSpec::minimal("vzlinuxlifecycle", "docker.io/library/alpine:latest");
        // A long-running primary so the agent stays connected for exec + a
        // graceful SIGTERM on stop. Echo a banner first so logs have content.
        spec.command.entrypoint = Some(vec![
            "sh".to_string(),
            "-c".to_string(),
            "echo lifecycle-up; sleep 600".to_string(),
        ]);

        rt.pull_image("docker.io/library/alpine:latest")
            .await
            .expect("pull alpine");

        // create -> Pending (no VM yet).
        rt.create_container(&id, &spec).await.expect("create");
        assert_eq!(
            rt.container_state(&id).await.expect("state after create"),
            ContainerState::Pending,
            "state should be Pending before start"
        );

        // start -> Running.
        rt.start_container(&id).await.expect("start");
        assert_eq!(
            rt.container_state(&id).await.expect("state after start"),
            ContainerState::Running,
            "state should be Running after start"
        );

        // logs: the banner should appear within a boot timeout.
        let mut saw_banner = false;
        for _ in 0..60 {
            let logs = rt.container_logs(&id, 200).await.unwrap_or_default();
            if logs.iter().any(|l| l.message.contains("lifecycle-up")) {
                saw_banner = true;
                break;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        assert!(saw_banner, "primary banner never appeared in logs");

        // stats: VZ has no live metrics, but memory_bytes/limit must be > 0.
        let stats = rt.get_container_stats(&id).await.expect("stats");
        assert!(stats.memory_bytes > 0, "memory_bytes must be > 0");
        assert!(stats.memory_limit > 0, "memory_limit must be > 0");

        // exec into the running PID namespace.
        let (code, stdout, _stderr) = tokio::time::timeout(
            Duration::from_secs(60),
            rt.exec(&id, &["echo".to_string(), "hi".to_string()]),
        )
        .await
        .expect("exec timed out")
        .expect("exec");
        assert_eq!(code, 0, "exec echo should exit 0");
        assert!(stdout.contains("hi"), "exec stdout should contain 'hi'");

        // stop: graceful SIGTERM, then forced stop. State must be terminal.
        rt.stop_container(&id, Duration::from_secs(10))
            .await
            .expect("stop");
        let stopped = rt.container_state(&id).await.expect("state after stop");
        assert!(
            matches!(stopped, ContainerState::Exited { .. }),
            "state after stop should be Exited, got: {stopped:?}"
        );

        // remove: the record is gone afterward, so state must error.
        rt.remove_container(&id).await.expect("remove");
        assert!(
            rt.container_state(&id).await.is_err(),
            "container_state must error after remove (record gone)"
        );
        // remove is idempotent.
        rt.remove_container(&id)
            .await
            .expect("second remove is Ok (idempotent)");
    }
}
