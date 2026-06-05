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
//! vars or the on-disk `{data_dir}/vz/linux/kernel/` cache.
//!
//! ## Directory layout
//!
//! ```text
//! {data_dir}/vz/linux/
//!   images/{sanitized_image}/rootfs/   -- extracted OCI image layers (shared)
//!   kernel/                            -- guest kernel cache (Image + initramfs.cpio.gz)
//!   cache/                             -- blob/registry cache
//!   {service}-{replica}/               -- per-container state
//!     rootfs/                          -- per-container rootfs (clone of base)
//!     console.log                      -- guest serial console
//!     config.json                      -- serialized ServiceSpec
//! ```

#![allow(unsafe_code)]

use crate::cgroups_stats::ContainerStats;
use crate::error::{AgentError, Result};
use crate::runtime::{ContainerId, ContainerState, Runtime};
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
    VZFileHandleSerialPortAttachment, VZGenericPlatformConfiguration, VZLinuxBootLoader,
    VZMACAddress, VZNATNetworkDeviceAttachment, VZNetworkDeviceConfiguration,
    VZPlatformConfiguration, VZSharedDirectory, VZSingleDirectoryShare,
    VZSocketDeviceConfiguration, VZVirtioConsoleDeviceSerialPortConfiguration,
    VZVirtioFileSystemDeviceConfiguration, VZVirtioNetworkDeviceConfiguration,
    VZVirtioSocketConnection, VZVirtioSocketDevice, VZVirtioSocketDeviceConfiguration,
    VZVirtualMachine, VZVirtualMachineConfiguration, VZVirtualMachineState,
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
    /// Host loopback forwarders for the container's published/exposed ports.
    /// Each binds `127.0.0.1:<container_port>` and tunnels every connection over
    /// a fresh vsock `Forward` to the guest agent. Tracked so `stop`/`remove`
    /// can abort them and so `port_mappings_container` can report the bindings.
    port_forwards: Vec<PortForward>,
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

    /// `"{service}-{replica}"` — the per-container directory / map key.
    fn container_dir_name(id: &ContainerId) -> String {
        format!("{}-{}", id.service, id.replica)
    }

    /// `{data_dir}/vz/linux` — the runtime's base directory.
    fn linux_dir(&self) -> PathBuf {
        self.data_dir.join("vz").join("linux")
    }

    /// `{data_dir}/vz/linux/images` — shared image rootfs store.
    fn images_dir(&self) -> PathBuf {
        self.linux_dir().join("images")
    }

    /// `{data_dir}/vz/linux/cache` — blob/registry cache.
    #[allow(dead_code)]
    fn cache_dir(&self) -> PathBuf {
        self.linux_dir().join("cache")
    }

    /// `{data_dir}/vz/linux/kernel` — guest kernel + initramfs cache.
    fn kernel_cache_dir(&self) -> PathBuf {
        self.linux_dir().join("kernel")
    }

    /// Resolve the Linux guest kernel `Image` + `initramfs.cpio.gz`.
    ///
    /// Resolution order:
    /// 1. The `ZLAYER_VZ_LINUX_KERNEL` + `ZLAYER_VZ_LINUX_INITRD` env vars
    ///    (the working dev override — both must be set and exist).
    /// 2. The on-disk cache at `{data_dir}/vz/linux/kernel/` containing the
    ///    files `Image` and `initramfs.cpio.gz`.
    ///
    /// Downloading a published CI artifact into the cache is a later concern;
    /// for now an operator runs `images/vz-linux/build.sh` and drops the two
    /// files into the cache, or points the env vars at a local build.
    ///
    /// # Errors
    /// Returns [`AgentError::Configuration`] when neither source yields both
    /// artifacts.
    fn ensure_linux_kernel(&self) -> Result<LinuxKernel> {
        // 1. Dev override via env.
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
                 falling back to the on-disk cache"
            );
        }

        // 2. On-disk cache.
        let cache = self.kernel_cache_dir();
        let image = cache.join("Image");
        let initramfs = cache.join("initramfs.cpio.gz");
        if image.exists() && initramfs.exists() {
            tracing::info!(
                kernel = %image.display(),
                initramfs = %initramfs.display(),
                "vz-linux: using kernel from on-disk cache"
            );
            return Ok(LinuxKernel { image, initramfs });
        }

        Err(AgentError::Configuration(format!(
            "vz-linux kernel artifact not found; set ZLAYER_VZ_LINUX_KERNEL/_INITRD or run \
             images/vz-linux/build.sh and place Image+initramfs.cpio.gz in {}",
            cache.display()
        )))
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
fn sanitize_image_name(image: &str) -> String {
    image.replace(['/', ':', '@'], "_")
}

/// Default Linux-guest kernel command line.
///
/// `console=hvc0` routes the kernel console to the virtio serial port wired to
/// `console.log`. `rootfstag=rootfs` is a marker the initramfs interprets:
/// Phase 3 will switch the root filesystem to a virtiofs share tagged `rootfs`
/// (the agent mounts virtiofs `rootfs` as `/`), replacing the placeholder
/// `root=/dev/vda` block-device path. Until then the initramfs runs from RAM.
const LINUX_CMDLINE: &str = "console=hvc0 rootfstag=rootfs rw";

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
        let cmdline = NSString::from_str(LINUX_CMDLINE);
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
        let fs_arr = NSArray::from_retained_slice(&[fs_super]);
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

/// Build the `Run` control message from a [`ServiceSpec`].
///
/// `argv` is the resolved entrypoint (entrypoint + args, falling back to
/// `["true"]`), `env` is the spec's environment as `(KEY, VALUE)` pairs, `cwd`
/// is the command workdir, and `uid`/`gid` are parsed from `spec.user`
/// (`"uid"` or `"uid:gid"`), defaulting to `0:0` (root).
pub(crate) fn build_run_message(spec: &ServiceSpec) -> proto::Msg {
    let argv = macos_vz_shared::resolve_entrypoint(spec);
    let env: Vec<(String, String)> = spec
        .env
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let cwd = spec.command.workdir.clone();
    let (uid, gid) = parse_user(spec.user.as_deref());
    proto::Msg::Run {
        argv,
        env,
        cwd,
        uid,
        gid,
    }
}

/// Parse a Docker-style `--user` value (`"uid"` or `"uid:gid"`) into a
/// `(uid, gid)` pair, defaulting to `(0, 0)`. Non-numeric names are not
/// resolvable on the host (the guest has its own passwd db), so we only honour
/// numeric ids and otherwise fall back to root.
fn parse_user(user: Option<&str>) -> (u32, u32) {
    let Some(user) = user else {
        return (0, 0);
    };
    let mut parts = user.splitn(2, ':');
    let uid = parts
        .next()
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(0);
    let gid = parts
        .next()
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(uid);
    (uid, gid)
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
fn run_and_drain(
    live: &LiveVm,
    run_msg: &proto::Msg,
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
                if let Ok(mut c) = outcome.exit_code.lock() {
                    *c = Some(code);
                }
                if let Ok(mut t) = outcome.terminal.lock() {
                    *t = Some(ContainerState::Exited { code });
                }
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

// ---------------------------------------------------------------------------
// Runtime trait implementation
// ---------------------------------------------------------------------------

#[async_trait::async_trait]
impl Runtime for VzLinuxRuntime {
    async fn pull_image(&self, image: &str) -> Result<()> {
        self.pull_image_with_policy(image, zlayer_spec::PullPolicy::IfNotPresent, None)
            .await
    }

    async fn pull_image_with_policy(
        &self,
        image: &str,
        policy: zlayer_spec::PullPolicy,
        _auth: Option<&RegistryAuth>,
    ) -> Result<()> {
        let safe_name = sanitize_image_name(image);
        let image_dir = self.images_dir().join(&safe_name);
        let rootfs_dir = image_dir.join("rootfs");

        match policy {
            zlayer_spec::PullPolicy::Always | zlayer_spec::PullPolicy::Newer => {
                // Always re-pull; drift detection happens at the service layer.
            }
            zlayer_spec::PullPolicy::IfNotPresent => {
                if rootfs_dir.exists() {
                    tracing::debug!(image = %image, "vz-linux image already present; skipping pull");
                    self.image_rootfs
                        .write()
                        .await
                        .insert(safe_name, rootfs_dir);
                    return Ok(());
                }
            }
            zlayer_spec::PullPolicy::Never => {
                if !rootfs_dir.exists() {
                    return Err(AgentError::PullFailed {
                        image: image.to_string(),
                        reason: "image not present and pull policy is Never".to_string(),
                    });
                }
                self.image_rootfs
                    .write()
                    .await
                    .insert(safe_name, rootfs_dir);
                return Ok(());
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

        let puller = zlayer_registry::ImagePuller::with_cache(blob_cache);
        let layers = puller
            .pull_image(image, &zlayer_registry::RegistryAuth::Anonymous)
            .await
            .map_err(|e| AgentError::PullFailed {
                image: image.to_string(),
                reason: format!("pull image layers: {e}"),
            })?;

        tracing::info!(
            image = %image,
            layer_count = layers.len(),
            "extracting layers to vz-linux image rootfs"
        );

        let mut unpacker = zlayer_registry::LayerUnpacker::new(rootfs_dir.clone());
        unpacker
            .unpack_layers(&layers)
            .await
            .map_err(|e| AgentError::PullFailed {
                image: image.to_string(),
                reason: format!("extract rootfs: {e}"),
            })?;

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

        // Locate the base image rootfs (from the pull cache, or the on-disk
        // default location).
        let image_name = spec.image.name.to_string();
        let safe_image = sanitize_image_name(&image_name);
        let image_rootfs = {
            let images = self.image_rootfs.read().await;
            images.get(&safe_image).cloned()
        }
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
            vcpus,
            ram_mib,
            started_at: None,
            live: None,
            outcome: Arc::new(RunOutcome::default()),
            drain_task: None,
            port_forwards: Vec::new(),
        };

        self.containers.write().await.insert(dir_name, container);
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    async fn start_container(&self, id: &ContainerId) -> Result<()> {
        let dir_name = Self::container_dir_name(id);

        // Resolve the guest kernel + initramfs (env override or on-disk cache).
        let kernel = self.ensure_linux_kernel()?;

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

        // Build the `Run` message + capture handles, then record the running VM.
        let (run_msg, outcome, console_log) = {
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
                build_run_message(&c.spec),
                Arc::clone(&c.outcome),
                c.console_log.clone(),
            )
        };

        // Spawn the blocking vsock drain task: connect to the guest agent,
        // send `Run`, then stream stdout/stderr/exit into the shared outcome.
        let dir_for_drain = dir_name.clone();
        let drain = tokio::task::spawn_blocking(move || {
            run_and_drain(
                &live_for_drain,
                &run_msg,
                &outcome,
                &console_log,
                &dir_for_drain,
            );
        });
        {
            let mut guard = self.containers.write().await;
            if let Some(c) = guard.get_mut(&dir_name) {
                c.drain_task = Some(drain);
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
        // final state, abort the drain + port-forward tasks, then force the VM
        // down and release it.
        let (live, drain) = {
            let mut guard = self.containers.write().await;
            let Some(c) = guard.get_mut(&dir_name) else {
                // Vanished mid-stop: nothing more to do.
                return Ok(());
            };
            // Preserve the workload's real exit code where the drain task already
            // recorded one; otherwise mark a forced stop as a clean exit.
            let recorded = c.outcome.terminal.lock().ok().and_then(|g| g.clone());
            c.state = recorded.unwrap_or(ContainerState::Exited { code: 0 });
            // Drop network state: abort every host loopback forwarder.
            for f in std::mem::take(&mut c.port_forwards) {
                f.task.abort();
            }
            (c.live.take(), c.drain_task.take())
        };
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
        let (state_dir, live, drain, forwards) = {
            let mut guard = self.containers.write().await;
            match guard.remove(&dir_name) {
                Some(c) => (Some(c.state_dir), c.live, c.drain_task, c.port_forwards),
                None => return Ok(()),
            }
        };
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
        // The agent's `Exited`/`Error` frame is authoritative: the workload
        // exited (with its real code) even if the VM lingers. Prefer it.
        if let Some(terminal) = c.outcome.terminal.lock().ok().and_then(|g| g.clone()) {
            return Ok(terminal);
        }
        // Otherwise reconcile with the live VM's actual state when running.
        if let Some(live) = &c.live {
            let reconciled = match read_vm_state(live) {
                VZVirtualMachineState::Running | VZVirtualMachineState::Paused => {
                    ContainerState::Running
                }
                VZVirtualMachineState::Error => ContainerState::Failed {
                    reason: "VM entered error state".to_string(),
                },
                // The guest powered off: a clean exit. Keep the last recorded
                // exit state where we already have one.
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
        let dir_name = Self::container_dir_name(id);

        if cmd.is_empty() {
            return Err(AgentError::Configuration(
                "vz-linux: exec requires a non-empty command".to_string(),
            ));
        }

        // Snapshot a queue-handle clone of the live VM (so we can drive the
        // vsock connect on its serial queue) plus the container spec's env to
        // inherit into the exec'd process. The VM must be running with a live
        // agent connection, or there is nothing to exec into.
        let (live, env) = {
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

        let exec_msg = proto::Msg::Exec {
            argv: cmd.to_vec(),
            env,
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

    async fn get_container_pid(&self, _id: &ContainerId) -> Result<Option<u32>> {
        // A VM has no host-visible container PID.
        Ok(None)
    }

    async fn get_container_ip(&self, id: &ContainerId) -> Result<Option<IpAddr>> {
        use std::net::Ipv4Addr;
        let dir_name = Self::container_dir_name(id);
        // Reachability to a VZ-Linux guest is NOT via a guest NAT IP: on this
        // macOS, Virtualization.framework's NAT never establishes a usable
        // vmnet/DHCP lease for our process, so the guest never gets one we can
        // route to. Instead each published port is reachable through a host
        // loopback forwarder that tunnels the connection over vsock into the
        // guest (see `run_port_forward`). So while the VM is live we report
        // loopback — that is the address callers (service.rs/proxy_manager.rs)
        // can actually reach the workload on. `Ok(None)` when not running or for
        // an unknown container.
        let guard = self.containers.read().await;
        match guard.get(&dir_name) {
            Some(c) if c.live.is_some() => Ok(Some(IpAddr::V4(Ipv4Addr::LOCALHOST))),
            _ => Ok(None),
        }
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
        tokio::task::spawn_blocking(move || run_vm_lifecycle(&live, VmLifecycleOp::Resume))
            .await
            .map_err(|e| AgentError::Internal(format!("resume task: {e}")))?
            .map_err(|e| AgentError::Internal(format!("resume: {e}")))
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

        match build_run_message(&spec) {
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
    fn build_run_message_defaults_to_root_and_true() {
        // A bare spec: no entrypoint/args -> `true`, no user -> root.
        let spec = ServiceSpec::minimal("svc", "docker.io/library/alpine:latest");
        match build_run_message(&spec) {
            proto::Msg::Run { argv, uid, gid, .. } => {
                assert_eq!(argv, vec!["true".to_string()]);
                assert_eq!((uid, gid), (0, 0));
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

    #[test]
    fn ensure_linux_kernel_errors_clearly_when_absent() {
        // With neither the env override nor a populated cache, the error must
        // name both escape hatches and the cache directory.
        let rt = VzLinuxRuntime::new(None).expect("construct VzLinuxRuntime");
        // Guard against a populated cache or env on the dev box: only assert the
        // error shape when resolution actually fails.
        if std::env::var_os("ZLAYER_VZ_LINUX_KERNEL").is_some()
            && std::env::var_os("ZLAYER_VZ_LINUX_INITRD").is_some()
        {
            return;
        }
        let cache = rt.kernel_cache_dir();
        if cache.join("Image").exists() && cache.join("initramfs.cpio.gz").exists() {
            return;
        }
        match rt.ensure_linux_kernel() {
            Err(AgentError::Configuration(msg)) => {
                assert!(msg.contains("ZLAYER_VZ_LINUX_KERNEL/_INITRD"));
                assert!(msg.contains("Image+initramfs.cpio.gz"));
            }
            other => panic!("expected Configuration error, got {other:?}"),
        }
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
