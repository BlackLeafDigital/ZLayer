//! `zlayer-vzagent` — the in-guest PID1 agent for the macOS Apple-Virtualization
//! (VZ) Linux runtime.
//!
//! Inside the initramfs this binary is installed as `/init`, so the Linux
//! kernel execs it as **PID 1**. It:
//!
//! 1. mounts the core pseudo-filesystems (`/proc`, `/sys`, devtmpfs on `/dev`,
//!    tmpfs on `/run` and `/tmp`),
//! 2. mounts the host-shared OCI rootfs over **virtiofs** (tag `rootfs`) and
//!    layers a writable tmpfs overlay on top,
//! 3. brings up networking (kernel `ip=dhcp` and/or `udhcpc` on `eth0`),
//! 4. `pivot_root`s onto the assembled container root while **staying PID 1**,
//! 5. opens an `AF_VSOCK` listener on [`proto::CONTROL_PORT`] and speaks the
//!    [`zlayer_vzagent::proto`] protocol to run/exec/signal the workload and
//!    stream its stdout/stderr + exit code back to the host.
//!
//! The real body is Linux-only. On any other platform (notably the macOS host
//! that reuses the `proto` library) a stub `main` is compiled so the crate
//! still builds.

// The PID1 agent needs raw syscalls (mount, pivot_root, AF_VSOCK via libc,
// fork/exec, waitpid, signals). All unsafe blocks are localized and documented;
// this allow is scoped to the Linux binary only via the module gate below.
#![cfg_attr(target_os = "linux", allow(unsafe_code))]

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("zlayer-vzagent runs only inside the Linux guest");
    std::process::exit(1);
}

#[cfg(target_os = "linux")]
fn main() {
    if let Err(e) = linux::run() {
        // PID 1 has nowhere meaningful to return an error to, but a clear
        // message on the console aids guest-boot debugging. We deliberately do
        // not exit(0): if PID 1 exits the kernel panics, which is the correct
        // failure mode (the host observes the VM stopping).
        eprintln!("zlayer-vzagent fatal: {e:#}");
        // Give the console a moment to flush, then halt loudly.
        linux::sync_and_halt();
    }
}

#[cfg(target_os = "linux")]
mod linux {
    // This module is a thin shim over raw Linux syscalls and libc structs, so
    // a handful of pedantic lints fire on conversions that are correct by
    // construction at this boundary:
    //   * cast_possible_wrap / cast_possible_truncation / cast_sign_loss:
    //     `ifreq` flag fields are `c_short`/`c_char`, vsock ports are `u16`,
    //     PIDs round-trip through `u32`↔`i32`, and `read`/`write` return
    //     `isize` that is non-negative after the error check. Every such cast
    //     targets the exact width the kernel ABI demands.
    //   * unnecessary_wraps: a couple of bring-up helpers return `Result` for
    //     uniform `?`-chaining in `run()` even though they currently only fail
    //     softly (logging + continue).
    //   * too_many_lines: the protocol dispatch loop is one cohesive state
    //     machine; splitting it would hurt readability more than it helps.
    #![allow(
        clippy::cast_possible_wrap,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::unnecessary_wraps,
        clippy::too_many_lines
    )]

    use std::collections::HashMap;
    use std::ffi::{CStr, CString};
    use std::fs;
    use std::io::{Read, Write};
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
    use std::path::Path;
    use std::process::{Child, Command, Stdio};
    use std::sync::atomic::{AtomicI32, Ordering};
    use std::sync::{mpsc, Mutex, OnceLock};
    use std::thread;

    use zlayer_vzagent::proto::{self, Msg};

    /// vsock address-family constant (Linux `AF_VSOCK`).
    const AF_VSOCK: libc::c_int = 40;
    /// "any CID" wildcard for binding a vsock listener.
    const VMADDR_CID_ANY: libc::c_uint = 0xFFFF_FFFF;

    /// PID of the currently-running primary workload, shared with the
    /// `Signal`/teardown paths. `0` means "no workload running".
    static WORKLOAD_PID: AtomicI32 = AtomicI32::new(0);

    type Result<T> = std::result::Result<T, Error>;

    // ----------------------------------------------------------------------
    // Child reaping (single-reaper + per-child status delivery)
    // ----------------------------------------------------------------------
    //
    // As PID 1 we MUST reap every child — including re-parented orphans — or
    // they pile up as zombies. But each connection handler also needs the exit
    // status of ITS specific child. We therefore use exactly ONE place that
    // ever calls `waitpid`: a dedicated reaper thread that loops on a blocking
    // `waitpid(-1, ...)`. Nothing else (no `Child::wait`, no per-handler
    // `waitpid`) competes for child statuses, so there is no ECHILD race.
    //
    // Delivery to the right handler goes through a shared [`Reaper`] registry:
    //   * Before reaping anyone, a handler can register a `(pid -> Sender)`
    //     waiter. When the reaper reaps `pid`, it hands the status to that
    //     waiter.
    //   * A child can exit (and be reaped) *before* its handler manages to
    //     register — a short-lived `Run`/`Exec` is the classic case. To close
    //     that race the reaper buffers the status of any reaped pid that has no
    //     waiter yet into `pending`. When the handler finally registers, it
    //     first checks `pending` and takes the already-delivered status.
    //   * A reaped pid that is a true orphan (re-parented grandchild) has no
    //     waiter and never will. Its status sits briefly in `pending`; to keep
    //     that map from growing without bound it is bounded and pruned (see
    //     `Reaper::deliver`). The zombie itself is already gone — we reaped it.

    /// Shared child-reaping registry. One instance lives in [`REAPER`].
    struct Reaper {
        inner: Mutex<ReaperInner>,
    }

    /// Mutable state behind the reaper's lock.
    struct ReaperInner {
        /// Handlers waiting for a specific child's exit status, keyed by pid.
        waiters: HashMap<i32, mpsc::Sender<i32>>,
        /// Raw `waitpid` statuses reaped before their handler registered.
        /// Keyed by pid; consumed by [`Reaper::register`].
        pending: HashMap<i32, libc::c_int>,
    }

    /// Process-global reaper. Initialized once in [`run`] before any child is
    /// spawned, so the reaper thread is always present to drain zombies.
    static REAPER: OnceLock<Reaper> = OnceLock::new();

    /// Hard cap on buffered orphan statuses. Orphans (re-parented grandchildren
    /// with no waiter) leave a status entry in `pending`; this bounds that map
    /// so a workload that forks many short-lived grandchildren cannot grow it
    /// without limit. The cap is generous — real per-connection children always
    /// register and consume their entry promptly.
    const MAX_PENDING_STATUSES: usize = 1024;

    impl Reaper {
        /// Access the process-global reaper, initializing its state on first use.
        fn global() -> &'static Reaper {
            REAPER.get_or_init(|| Reaper {
                inner: Mutex::new(ReaperInner {
                    waiters: HashMap::new(),
                    pending: HashMap::new(),
                }),
            })
        }

        /// Register interest in `pid`'s exit status and return the receiver the
        /// reaper will deliver the raw status to. If the child has *already*
        /// been reaped (status sitting in `pending`), the status is delivered
        /// on the returned receiver immediately so the caller never blocks
        /// forever on a race.
        fn register(&self, pid: i32) -> mpsc::Receiver<i32> {
            let (tx, rx) = mpsc::channel();
            let mut inner = self
                .inner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(status) = inner.pending.remove(&pid) {
                // Already reaped before we got here: deliver immediately.
                let _ = tx.send(status);
            } else {
                inner.waiters.insert(pid, tx);
            }
            rx
        }

        /// Deliver a reaped `(pid, status)` to its waiter, or buffer it if the
        /// handler has not registered yet. Called only by the reaper thread.
        fn deliver(&self, pid: i32, status: libc::c_int) {
            let mut inner = self
                .inner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(tx) = inner.waiters.remove(&pid) {
                // Waiter present: hand off (it may have dropped the rx if the
                // connection died; that's fine, send just errors and we move
                // on — the zombie is already reaped).
                let _ = tx.send(status);
            } else {
                // No waiter yet. Buffer for the handler's upcoming register(),
                // unless this is an unbounded flood of orphan grandchildren.
                if inner.pending.len() < MAX_PENDING_STATUSES {
                    inner.pending.insert(pid, status);
                }
                // If over the cap, we simply drop the status: such a pid is an
                // orphan no handler will ever wait on, and the zombie is
                // already reaped, so dropping the (now meaningless) status is
                // safe.
            }
        }
    }

    /// Translate a raw `waitpid` status into the host-facing exit code:
    /// the process exit code, or `128 + signal` for signal termination.
    fn status_to_code(status: libc::c_int) -> i32 {
        // SAFETY: the W* accessors are pure macros over an integer status with
        // no memory access; always sound.
        if libc::WIFEXITED(status) {
            libc::WEXITSTATUS(status)
        } else if libc::WIFSIGNALED(status) {
            128 + libc::WTERMSIG(status)
        } else {
            // Stopped/continued statuses are filtered out before delivery (we
            // do not pass WUNTRACED/WCONTINUED), so this is unreachable in
            // practice; report -1 defensively.
            -1
        }
    }

    /// The dedicated reaper thread body: block on `waitpid(-1, ...)` forever,
    /// dispatching every reaped child to [`Reaper::deliver`]. This is the ONLY
    /// caller of `waitpid` in the agent, which is what makes per-child status
    /// delivery race-free.
    fn reaper_loop() -> ! {
        let reaper = Reaper::global();
        loop {
            let mut status: libc::c_int = 0;
            // Blocking wait for ANY child (workloads, exec'd processes, and
            // re-parented orphans alike). No WNOHANG: when there are no
            // children we want to sleep in the kernel, not spin.
            // SAFETY: waitpid with a valid status out-pointer and a literal
            // flag set; -1 means "any child".
            let pid = unsafe { libc::waitpid(-1, &raw mut status, 0) };
            if pid > 0 {
                reaper.deliver(pid, status);
                continue;
            }
            // pid <= 0 here means either EINTR (retry) or ECHILD (no children
            // right now). For ECHILD a tight loop would spin-wait, so back off
            // briefly; a new child will arrive via a future Run/Exec. EINTR is
            // also covered by the short sleep.
            let e = std::io::Error::last_os_error();
            if e.raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    /// Agent error type. Kept simple and message-oriented because everything
    /// ultimately funnels to a console line or a `proto::Msg::Error` frame.
    #[derive(Debug)]
    pub struct Error(String);

    impl std::fmt::Display for Error {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(&self.0)
        }
    }

    impl std::error::Error for Error {}

    impl From<std::io::Error> for Error {
        fn from(e: std::io::Error) -> Self {
            Error(e.to_string())
        }
    }

    impl From<proto::ProtoError> for Error {
        fn from(e: proto::ProtoError) -> Self {
            Error(e.to_string())
        }
    }

    /// Build an [`Error`] from a static or formatted context string.
    fn err(msg: impl Into<String>) -> Error {
        Error(msg.into())
    }

    /// Convert the current `errno` into an [`Error`] tagged with `ctx`.
    fn errno(ctx: &str) -> Error {
        Error(format!("{ctx}: {}", std::io::Error::last_os_error()))
    }

    // ----------------------------------------------------------------------
    // Mount helpers
    // ----------------------------------------------------------------------

    /// Thin wrapper over the `mount(2)` syscall.
    fn mount(
        source: &str,
        target: &str,
        fstype: &str,
        flags: libc::c_ulong,
        data: Option<&str>,
    ) -> Result<()> {
        let c_source = CString::new(source).map_err(|_| err("mount: nul in source"))?;
        let c_target = CString::new(target).map_err(|_| err("mount: nul in target"))?;
        let c_fstype = CString::new(fstype).map_err(|_| err("mount: nul in fstype"))?;
        let c_data = match data {
            Some(d) => Some(CString::new(d).map_err(|_| err("mount: nul in data"))?),
            None => None,
        };
        // SAFETY: all pointers are valid NUL-terminated C strings for the
        // duration of the call; `data` is either null or a live CString.
        let rc = unsafe {
            libc::mount(
                c_source.as_ptr(),
                c_target.as_ptr(),
                c_fstype.as_ptr(),
                flags,
                c_data
                    .as_ref()
                    .map_or(std::ptr::null(), |d| d.as_ptr().cast::<libc::c_void>()),
            )
        };
        if rc != 0 {
            return Err(errno(&format!("mount {source} -> {target} ({fstype})")));
        }
        Ok(())
    }

    /// `mkdir -p` for a single path, tolerating "already exists".
    fn mkdir_p(path: &str) -> Result<()> {
        match fs::create_dir_all(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
            Err(e) => Err(err(format!("mkdir {path}: {e}"))),
        }
    }

    /// Mount the core pseudo-filesystems every Linux userspace expects.
    fn mount_core_filesystems() -> Result<()> {
        // /proc and /sys: virtual kernel interfaces.
        mkdir_p("/proc")?;
        mount("proc", "/proc", "proc", 0, None)?;
        mkdir_p("/sys")?;
        mount("sysfs", "/sys", "sysfs", 0, None)?;
        // devtmpfs auto-populates device nodes (CONFIG_DEVTMPFS_MOUNT may also
        // do this; mounting explicitly is idempotent-enough that we tolerate
        // EBUSY).
        mkdir_p("/dev")?;
        if let Err(e) = mount("devtmpfs", "/dev", "devtmpfs", 0, None) {
            // If the kernel already auto-mounted it (DEVTMPFS_MOUNT=y) the
            // explicit mount returns EBUSY — that's fine.
            if !is_ebusy() {
                return Err(e);
            }
        }
        // /dev/pts for pseudo-terminals (exec/attach later relies on it).
        mkdir_p("/dev/pts")?;
        let _ = mount("devpts", "/dev/pts", "devpts", 0, Some("mode=0620,gid=5"));
        // tmpfs scratch areas.
        mkdir_p("/run")?;
        mount("tmpfs", "/run", "tmpfs", 0, Some("mode=0755"))?;
        mkdir_p("/tmp")?;
        mount("tmpfs", "/tmp", "tmpfs", 0, Some("mode=1777"))?;
        Ok(())
    }

    /// True if the last OS error was `EBUSY`.
    fn is_ebusy() -> bool {
        std::io::Error::last_os_error().raw_os_error() == Some(libc::EBUSY)
    }

    /// Mount the host-shared OCI rootfs (virtiofs tag `rootfs`) read-only at
    /// `/lower`, then build a writable overlay at `/newroot`:
    /// `lowerdir=/lower, upperdir=/run/overlay/upper, workdir=/run/overlay/work`.
    fn assemble_container_root() -> Result<()> {
        mkdir_p("/lower")?;
        // virtiofs is exposed by Apple VZ's directory-share device; the kernel
        // surfaces it through FUSE (CONFIG_VIRTIO_FS + CONFIG_FUSE_FS). The
        // share tag agreed with the host is "rootfs". Mount read-only so the
        // immutable image layers can never be mutated in place.
        mount("rootfs", "/lower", "virtiofs", libc::MS_RDONLY, None)?;

        // Overlay upper + work dirs live on a tmpfs so container writes are
        // ephemeral (matching a fresh container layer). The host owns
        // persistence via separate volume mounts when needed.
        mkdir_p("/run/overlay")?;
        mount("tmpfs", "/run/overlay", "tmpfs", 0, Some("mode=0755"))?;
        mkdir_p("/run/overlay/upper")?;
        mkdir_p("/run/overlay/work")?;
        mkdir_p("/newroot")?;

        mount(
            "overlay",
            "/newroot",
            "overlay",
            0,
            Some("lowerdir=/lower,upperdir=/run/overlay/upper,workdir=/run/overlay/work"),
        )?;
        Ok(())
    }

    // ----------------------------------------------------------------------
    // Networking
    // ----------------------------------------------------------------------

    /// Bring up `eth0` and acquire a DHCP lease from the VZ NAT.
    ///
    /// The kernel may already have configured the interface via the
    /// `ip=dhcp`/`IP_PNP` boot path; we detect that and skip. Otherwise we run
    /// busybox `udhcpc` (shipped in the initramfs) defensively.
    fn bring_up_network() -> Result<()> {
        // Ensure loopback is up — many runtimes assume 127.0.0.1 works.
        let _ = set_link_up("lo");

        if let Err(e) = set_link_up("eth0") {
            // Not fatal: the workload may not need networking, and the host
            // surfaces the warning. Log and continue.
            eprintln!("zlayer-vzagent: warning: failed to set eth0 up: {e}");
            return Ok(());
        }

        if interface_has_ipv4("eth0") {
            // Kernel IP_PNP already gave us a lease; nothing to do.
            return Ok(());
        }

        // Run udhcpc in one-shot mode (-q quits after obtaining a lease, -f
        // foreground, -n give-up-if-no-lease) with a short timeout so a missing
        // DHCP server doesn't wedge boot.
        let candidates = ["/sbin/udhcpc", "/usr/sbin/udhcpc", "/bin/busybox"];
        for path in candidates {
            if !Path::new(path).exists() {
                continue;
            }
            let mut cmd = Command::new(path);
            if path.ends_with("busybox") {
                cmd.arg("udhcpc");
            }
            cmd.args(["-i", "eth0", "-f", "-q", "-n", "-t", "5", "-T", "2"]);
            match cmd.status() {
                Ok(status) if status.success() => return Ok(()),
                Ok(status) => {
                    eprintln!("zlayer-vzagent: udhcpc exited {status}; continuing");
                    return Ok(());
                }
                Err(e) => {
                    eprintln!("zlayer-vzagent: failed to spawn {path}: {e}");
                }
            }
        }
        eprintln!("zlayer-vzagent: no udhcpc/busybox found; relying on kernel ip=dhcp");
        Ok(())
    }

    /// Set a network interface administratively up via an ioctl on a dgram
    /// socket (`SIOCSIFFLAGS` | `IFF_UP`).
    fn set_link_up(ifname: &str) -> Result<()> {
        // SAFETY: socket(2) with valid args; fd is checked and wrapped in
        // OwnedFd so it is always closed.
        let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0) };
        if fd < 0 {
            return Err(errno("socket(AF_INET)"));
        }
        // SAFETY: fd is a valid, owned socket descriptor.
        let owned = unsafe { OwnedFd::from_raw_fd(fd) };

        let mut ifr: libc::ifreq = unsafe { std::mem::zeroed() };
        let name_bytes = ifname.as_bytes();
        if name_bytes.len() >= ifr.ifr_name.len() {
            return Err(err("interface name too long"));
        }
        for (dst, &b) in ifr.ifr_name.iter_mut().zip(name_bytes) {
            *dst = b as libc::c_char;
        }

        // Read current flags.
        // SAFETY: ioctl with a properly-initialized ifreq on a valid fd.
        if unsafe { libc::ioctl(owned.as_raw_fd(), libc::SIOCGIFFLAGS, &mut ifr) } < 0 {
            return Err(errno(&format!("SIOCGIFFLAGS {ifname}")));
        }
        // SAFETY: ifr_flags union field is valid after the get ioctl above.
        unsafe {
            ifr.ifr_ifru.ifru_flags |= (libc::IFF_UP | libc::IFF_RUNNING) as libc::c_short;
        }
        // SAFETY: ioctl with the updated ifreq on a valid fd.
        if unsafe { libc::ioctl(owned.as_raw_fd(), libc::SIOCSIFFLAGS, &ifr) } < 0 {
            return Err(errno(&format!("SIOCSIFFLAGS {ifname}")));
        }
        Ok(())
    }

    /// Check whether `ifname` already has an IPv4 address (kernel `IP_PNP` lease).
    fn interface_has_ipv4(ifname: &str) -> bool {
        let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0) };
        if fd < 0 {
            return false;
        }
        let owned = unsafe { OwnedFd::from_raw_fd(fd) };
        let mut ifr: libc::ifreq = unsafe { std::mem::zeroed() };
        let name_bytes = ifname.as_bytes();
        if name_bytes.len() >= ifr.ifr_name.len() {
            return false;
        }
        for (dst, &b) in ifr.ifr_name.iter_mut().zip(name_bytes) {
            *dst = b as libc::c_char;
        }
        // SIOCGIFADDR succeeds only if an address is assigned.
        // SAFETY: valid fd + initialized ifreq.
        unsafe { libc::ioctl(owned.as_raw_fd(), libc::SIOCGIFADDR, &mut ifr) == 0 }
    }

    // ----------------------------------------------------------------------
    // pivot_root
    // ----------------------------------------------------------------------

    /// `pivot_root` onto `/newroot` while staying PID 1.
    ///
    /// We carry the already-mounted pseudo-filesystems across by moving them
    /// into the new root, then detach the old root. The agent process itself
    /// stays resident as PID 1 — it does **not** re-exec — so the vsock control
    /// loop continues uninterrupted and the workload is launched as a child
    /// inside the new root.
    fn pivot_into_newroot() -> Result<()> {
        let new_root = "/newroot";
        // Move the core mounts into the new root so they survive the pivot.
        for (src, dst) in [
            ("/proc", "/newroot/proc"),
            ("/sys", "/newroot/sys"),
            ("/dev", "/newroot/dev"),
            ("/run", "/newroot/run"),
            ("/tmp", "/newroot/tmp"),
        ] {
            mkdir_p(dst)?;
            // MS_MOVE relocates an existing mount; ignore ENOENT/EINVAL for
            // mounts that may not exist in a minimal config.
            if let Err(e) = mount(src, dst, "", libc::MS_MOVE, None) {
                eprintln!("zlayer-vzagent: warning: could not move {src} -> {dst}: {e}");
            }
        }

        // Make sure new_root is a mount point and the propagation is private so
        // pivot_root succeeds.
        mount("", "/", "", libc::MS_REC | libc::MS_PRIVATE, None)?;

        // Bind-mount newroot onto itself so it is guaranteed to be a mount
        // point (required by pivot_root).
        mount(new_root, new_root, "", libc::MS_BIND | libc::MS_REC, None)?;

        let put_old = "/newroot/.oldroot";
        mkdir_p(put_old)?;

        let c_new = CString::new(new_root).map_err(|_| err("nul in new_root"))?;
        let c_old = CString::new(put_old).map_err(|_| err("nul in put_old"))?;
        // SAFETY: SYS_pivot_root with two valid C-string paths.
        let rc = unsafe { libc::syscall(libc::SYS_pivot_root, c_new.as_ptr(), c_old.as_ptr()) };
        if rc != 0 {
            return Err(errno("pivot_root"));
        }

        // chdir into the new root and detach the old one.
        let c_root = CString::new("/").unwrap();
        // SAFETY: chdir to "/" (now the new root) — valid path.
        if unsafe { libc::chdir(c_root.as_ptr()) } != 0 {
            return Err(errno("chdir(/) after pivot"));
        }
        let c_oldroot = CString::new("/.oldroot").unwrap();
        // Lazy-detach the old root; it disappears once unused.
        // SAFETY: umount2 with a valid path and MNT_DETACH flag.
        if unsafe { libc::umount2(c_oldroot.as_ptr(), libc::MNT_DETACH) } != 0 {
            eprintln!(
                "zlayer-vzagent: warning: umount old root: {}",
                std::io::Error::last_os_error()
            );
        }
        let _ = fs::remove_dir("/.oldroot");
        Ok(())
    }

    // ----------------------------------------------------------------------
    // vsock listener
    // ----------------------------------------------------------------------

    /// Bind an `AF_VSOCK` listener on `(VMADDR_CID_ANY, port)` and return the
    /// listening socket fd.
    fn vsock_listen(port: u32) -> Result<OwnedFd> {
        // SAFETY: socket(2) with AF_VSOCK; result checked.
        let fd = unsafe { libc::socket(AF_VSOCK, libc::SOCK_STREAM, 0) };
        if fd < 0 {
            return Err(errno("socket(AF_VSOCK)"));
        }
        // SAFETY: fd is a valid owned descriptor.
        let owned = unsafe { OwnedFd::from_raw_fd(fd) };

        let mut addr: libc::sockaddr_vm = unsafe { std::mem::zeroed() };
        addr.svm_family = AF_VSOCK as libc::sa_family_t;
        addr.svm_port = port;
        addr.svm_cid = VMADDR_CID_ANY;

        // SAFETY: bind with a correctly-sized sockaddr_vm on a valid fd.
        let rc = unsafe {
            libc::bind(
                owned.as_raw_fd(),
                std::ptr::addr_of!(addr).cast::<libc::sockaddr>(),
                std::mem::size_of::<libc::sockaddr_vm>() as libc::socklen_t,
            )
        };
        if rc != 0 {
            return Err(errno("bind(AF_VSOCK)"));
        }
        // SAFETY: listen on a bound, valid fd.
        if unsafe { libc::listen(owned.as_raw_fd(), 8) } != 0 {
            return Err(errno("listen(AF_VSOCK)"));
        }
        Ok(owned)
    }

    /// Accept a single vsock connection, returning the connected fd.
    fn vsock_accept(listener: RawFd) -> Result<OwnedFd> {
        // SAFETY: accept on a valid listening fd; we pass null addr/len since
        // we don't need the peer address.
        let fd = unsafe { libc::accept(listener, std::ptr::null_mut(), std::ptr::null_mut()) };
        if fd < 0 {
            return Err(errno("accept(AF_VSOCK)"));
        }
        // SAFETY: fd is a valid owned descriptor.
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }

    /// A blocking duplex wrapper around a vsock fd, usable as `Read`/`Write`.
    struct VsockStream {
        fd: OwnedFd,
    }

    impl VsockStream {
        fn new(fd: OwnedFd) -> Self {
            Self { fd }
        }
        /// Duplicate the underlying fd into an independent owned handle so a
        /// reader and writer half can be split across threads.
        fn try_clone(&self) -> Result<VsockStream> {
            let dup = self.fd.try_clone().map_err(Error::from)?;
            Ok(VsockStream { fd: dup })
        }
    }

    impl Read for VsockStream {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            // SAFETY: read into a valid buffer slice on a valid fd.
            let n = unsafe {
                libc::read(
                    self.fd.as_raw_fd(),
                    buf.as_mut_ptr().cast::<libc::c_void>(),
                    buf.len(),
                )
            };
            if n < 0 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(n as usize)
            }
        }
    }

    impl Write for VsockStream {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            // SAFETY: write from a valid buffer slice on a valid fd.
            let n = unsafe {
                libc::write(
                    self.fd.as_raw_fd(),
                    buf.as_ptr().cast::<libc::c_void>(),
                    buf.len(),
                )
            };
            if n < 0 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(n as usize)
            }
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    // ----------------------------------------------------------------------
    // Workload execution
    // ----------------------------------------------------------------------

    /// Spawn the workload entrypoint described by a `Run` message, with stdio
    /// piped back to the host. Sets uid/gid/cwd/env. Returns the [`Child`].
    fn spawn_workload(
        argv: &[String],
        env: &[(String, String)],
        cwd: Option<&str>,
        uid: u32,
        gid: u32,
    ) -> Result<Child> {
        let program = argv.first().ok_or_else(|| err("Run: empty argv"))?;
        let mut cmd = Command::new(program);
        cmd.args(&argv[1..]);
        cmd.env_clear();
        for (k, v) in env {
            cmd.env(k, v);
        }
        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        // Drop privileges before exec. We use a pre-exec hook so the
        // setgid/setuid run in the child just before execve, in the correct
        // order (group first, then user).
        let want_user = uid;
        let want_group = gid;
        // SAFETY: the closure runs in the forked child between fork and exec.
        // It only calls async-signal-safe libc functions (setgid/setuid).
        unsafe {
            use std::os::unix::process::CommandExt;
            cmd.pre_exec(move || {
                if libc::setgid(want_group) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::setuid(want_user) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }

        cmd.spawn()
            .map_err(|e| err(format!("spawn {program}: {e}")))
    }

    /// Pump a child's stdout/stderr into framed `Stdout`/`Stderr` messages on
    /// the shared writer until EOF.
    fn stream_child_output(
        child: &mut Child,
        out_tx: &mpsc::Sender<Msg>,
    ) -> (thread::JoinHandle<()>, thread::JoinHandle<()>) {
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        let tx_out = out_tx.clone();
        let h_out = thread::spawn(move || {
            if let Some(mut s) = stdout {
                let mut buf = [0u8; 8192];
                loop {
                    match s.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            if tx_out.send(Msg::Stdout(buf[..n].to_vec())).is_err() {
                                break;
                            }
                        }
                    }
                }
            }
        });

        let tx_err = out_tx.clone();
        let h_err = thread::spawn(move || {
            if let Some(mut s) = stderr {
                let mut buf = [0u8; 8192];
                loop {
                    match s.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            if tx_err.send(Msg::Stderr(buf[..n].to_vec())).is_err() {
                                break;
                            }
                        }
                    }
                }
            }
        });

        (h_out, h_err)
    }

    /// Deliver a signal to the running workload (and its process group).
    fn signal_workload(signum: i32) -> Result<()> {
        let pid = WORKLOAD_PID.load(Ordering::SeqCst);
        if pid <= 0 {
            return Err(err("Signal: no workload running"));
        }
        // SAFETY: kill(2) with a captured pid and a signal number.
        if unsafe { libc::kill(pid, signum) } != 0 {
            return Err(errno(&format!("kill({pid}, {signum})")));
        }
        Ok(())
    }

    /// Spawn an additional process (the `exec` path) inside the workload's
    /// mount/PID namespaces via `nsenter`-style `setns`, streaming its output.
    ///
    /// Because the agent stays PID 1 in the same mount namespace as the
    /// workload root (we `pivot_root`ed, we did not unshare a fresh mount ns),
    /// the exec'd process already shares the container filesystem. We attach it
    /// to the workload's PID namespace if one exists so tools like `ps` see the
    /// container view.
    fn spawn_exec(argv: &[String], env: &[(String, String)]) -> Result<Child> {
        let program = argv.first().ok_or_else(|| err("Exec: empty argv"))?;
        let mut cmd = Command::new(program);
        cmd.args(&argv[1..]);
        cmd.env_clear();
        for (k, v) in env {
            cmd.env(k, v);
        }
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let pid = WORKLOAD_PID.load(Ordering::SeqCst);
        if pid > 0 {
            // Enter the workload's PID namespace so child PIDs are visible in
            // the container's PID space. The mount/net namespaces are already
            // shared (single-namespace guest), so only PID needs setns.
            let ns_path = format!("/proc/{pid}/ns/pid");
            if let Ok(file) = fs::File::open(&ns_path) {
                let raw = file.as_raw_fd();
                // SAFETY: setns with a valid open ns fd and CLONE_NEWPID; runs
                // before fork/exec in the parent, affecting children only after
                // the next fork.
                let rc = unsafe { libc::setns(raw, libc::CLONE_NEWPID) };
                if rc != 0 {
                    eprintln!(
                        "zlayer-vzagent: warning: setns(pid) for exec failed: {}",
                        std::io::Error::last_os_error()
                    );
                }
                // Keep `file` alive until after the spawn so the fd stays open.
                let child = cmd
                    .spawn()
                    .map_err(|e| err(format!("exec spawn {program}: {e}")))?;
                drop(file);
                return Ok(child);
            }
        }
        cmd.spawn()
            .map_err(|e| err(format!("exec spawn {program}: {e}")))
    }

    // ----------------------------------------------------------------------
    // Protocol loop
    // ----------------------------------------------------------------------

    /// Block until the dedicated reaper thread delivers this child's raw
    /// `waitpid` status on `rx`. Returns `None` only if the reaper somehow
    /// dropped the sender without delivering (should not happen in practice —
    /// the reaper either delivers or the pid is buffered in `pending` and
    /// delivered at `register` time).
    fn wait_via_reaper(rx: &mpsc::Receiver<libc::c_int>) -> Option<libc::c_int> {
        rx.recv().ok()
    }

    /// Drive the control connection: read messages from the host, dispatch
    /// run/exec/signal, and stream output + exit codes back.
    fn serve_connection(conn: VsockStream) -> Result<()> {
        let mut reader = conn.try_clone()?;
        let writer = conn;

        // All outbound frames (stdout/stderr/started/exited/error) are
        // serialized through a single writer thread to avoid interleaving
        // partial frames from the output-pump threads.
        let (tx, rx) = mpsc::channel::<Msg>();
        let mut writer_stream = writer;
        let writer_handle = thread::spawn(move || {
            while let Ok(msg) = rx.recv() {
                if proto::write_frame(&mut writer_stream, &msg).is_err() {
                    break;
                }
            }
        });

        loop {
            let msg = match proto::read_frame(&mut reader) {
                Ok(m) => m,
                Err(proto::ProtoError::Io(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    // Host closed the control connection cleanly.
                    break;
                }
                Err(e) => {
                    let _ = tx.send(Msg::Error {
                        message: format!("read_frame: {e}"),
                    });
                    break;
                }
            };

            match msg {
                Msg::Run {
                    argv,
                    env,
                    cwd,
                    uid,
                    gid,
                } => {
                    match spawn_workload(&argv, &env, cwd.as_deref(), uid, gid) {
                        Ok(mut child) => {
                            let pid = child.id() as i32;
                            // Register with the reaper FIRST so its status is
                            // captured even if the child exits immediately.
                            let status_rx = Reaper::global().register(pid);
                            WORKLOAD_PID.store(pid, Ordering::SeqCst);
                            let _ = tx.send(Msg::Started { pid });
                            let (h_out, h_err) = stream_child_output(&mut child, &tx);
                            // Hand reaping to the dedicated reaper thread: do
                            // NOT call child.wait() (that would race the reaper
                            // for the child's status). We only kept `child` to
                            // own the stdio pipes; forget it so its Drop never
                            // touches the pid the reaper now owns.
                            let raw_status = wait_via_reaper(&status_rx);
                            std::mem::forget(child);
                            // Ensure all buffered output has been pumped.
                            let _ = h_out.join();
                            let _ = h_err.join();
                            WORKLOAD_PID.store(0, Ordering::SeqCst);
                            if let Some(s) = raw_status {
                                let _ = tx.send(Msg::Exited {
                                    code: status_to_code(s),
                                });
                            } else {
                                let _ = tx.send(Msg::Error {
                                    message: "wait: reaper channel closed".into(),
                                });
                                let _ = tx.send(Msg::Exited { code: -1 });
                            }
                        }
                        Err(e) => {
                            let _ = tx.send(Msg::Error {
                                message: format!("Run failed: {e}"),
                            });
                        }
                    }
                }
                Msg::Exec { argv, env } => match spawn_exec(&argv, &env) {
                    Ok(mut child) => {
                        let pid = child.id() as i32;
                        let status_rx = Reaper::global().register(pid);
                        let _ = tx.send(Msg::Started { pid });
                        let (h_out, h_err) = stream_child_output(&mut child, &tx);
                        let raw_status = wait_via_reaper(&status_rx);
                        std::mem::forget(child);
                        let _ = h_out.join();
                        let _ = h_err.join();
                        let code = raw_status.map_or(-1, status_to_code);
                        let _ = tx.send(Msg::Exited { code });
                    }
                    Err(e) => {
                        let _ = tx.send(Msg::Error {
                            message: format!("Exec failed: {e}"),
                        });
                    }
                },
                Msg::Signal { signum } => {
                    if let Err(e) = signal_workload(signum) {
                        let _ = tx.send(Msg::Error {
                            message: format!("Signal failed: {e}"),
                        });
                    }
                }
                // Guest→host-only messages are never expected from the host;
                // reply with an error rather than silently dropping them.
                other => {
                    let _ = tx.send(Msg::Error {
                        message: format!("unexpected host message: {:?}", other.tag()),
                    });
                }
            }
        }

        drop(tx);
        let _ = writer_handle.join();
        Ok(())
    }

    // ----------------------------------------------------------------------
    // Entry point
    // ----------------------------------------------------------------------

    /// Full PID1 bring-up sequence followed by the control loop.
    pub fn run() -> Result<()> {
        // Reassure ourselves we're actually PID 1.
        // SAFETY: getpid is always safe.
        let me = unsafe { libc::getpid() };
        if me != 1 {
            eprintln!("zlayer-vzagent: warning: not PID 1 (pid={me}); continuing anyway");
        }

        mount_core_filesystems()?;
        assemble_container_root()?;
        bring_up_network()?;
        pivot_into_newroot()?;

        // Initialize the global reaper registry and start the SINGLE dedicated
        // reaper thread before any child can be spawned. As PID 1 we are the
        // reaper of last resort for every re-parented orphan; this thread is
        // the only `waitpid` caller in the process, so per-child status
        // delivery to connection handlers is race-free.
        let _ = Reaper::global();
        thread::Builder::new()
            .name("vzagent-reaper".into())
            .spawn(reaper_loop)
            .map_err(|e| err(format!("spawn reaper thread: {e}")))?;

        let listener = vsock_listen(proto::CONTROL_PORT)?;
        eprintln!(
            "zlayer-vzagent: listening on vsock (CID_ANY, port {})",
            proto::CONTROL_PORT
        );

        // Accept host control connections in a loop and serve each on its OWN
        // thread, so a long-running `Run` workload does not block accepting (or
        // serving) a subsequent `Exec` — `docker exec` into a running container
        // must not wait for the container to exit. Handler threads are
        // panic-isolated via `serve_connection`'s catch_unwind wrapper so a
        // bug in one connection can never take down PID 1.
        loop {
            let conn_fd = match vsock_accept(listener.as_raw_fd()) {
                Ok(fd) => fd,
                Err(e) => {
                    eprintln!("zlayer-vzagent: accept error: {e}; retrying");
                    thread::sleep(std::time::Duration::from_millis(100));
                    continue;
                }
            };
            let stream = VsockStream::new(conn_fd);
            // Pre-dup a fallback handle: `thread::Builder::spawn` consumes the
            // closure (and thus `stream`) even on failure, so we keep an
            // independent owned handle for the degraded inline path. The dup is
            // dropped immediately if the thread spawns successfully.
            let fallback = stream.try_clone();
            let spawned = thread::Builder::new()
                .name("vzagent-conn".into())
                .spawn(move || serve_connection_guarded(stream));
            match (spawned, fallback) {
                (Ok(_handle), _) => {}
                (Err(e), Ok(fallback_stream)) => {
                    // Thread spawn failed (e.g. resource exhaustion). Serve
                    // inline as a degraded fallback rather than dropping the
                    // connection; still guarded so a panic cannot kill PID 1.
                    eprintln!(
                        "zlayer-vzagent: failed to spawn handler thread: {e}; serving inline"
                    );
                    serve_connection_guarded(fallback_stream);
                }
                (Err(e), Err(dup_err)) => {
                    // Could neither spawn a thread nor dup the fd for inline
                    // service. Drop the connection (the host will reconnect)
                    // rather than risk PID 1; the original `stream` was already
                    // consumed by the failed spawn.
                    eprintln!(
                        "zlayer-vzagent: failed to spawn handler ({e}) and dup fd ({dup_err}); dropping connection"
                    );
                }
            }
        }
    }

    /// Run [`serve_connection`] with panic isolation. A connection handler runs
    /// on its own thread; this catches any panic (or surfaced error) so a bug
    /// while serving one connection can never unwind into — and thus terminate
    /// — PID 1.
    fn serve_connection_guarded(stream: VsockStream) {
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| serve_connection(stream)));
        match result {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                eprintln!("zlayer-vzagent: connection ended with error: {e}");
            }
            Err(_) => {
                eprintln!("zlayer-vzagent: connection handler panicked; recovered");
            }
        }
    }

    /// Flush filesystems and halt. Used when `run()` returns an error so PID 1
    /// does not simply exit (which would panic the kernel without context).
    pub fn sync_and_halt() -> ! {
        // SAFETY: sync(2) takes no arguments and is always safe.
        unsafe { libc::sync() };
        // Give the console output a chance to drain.
        thread::sleep(std::time::Duration::from_secs(1));
        // reboot(RB_POWER_OFF) cleanly stops the VM so the host sees it exit.
        // SAFETY: reboot syscall with the power-off magic; this never returns
        // on success.
        unsafe {
            libc::reboot(libc::RB_POWER_OFF);
        }
        // If reboot somehow returns, loop forever rather than exit PID 1.
        loop {
            thread::sleep(std::time::Duration::from_secs(3600));
        }
    }

    // Silence unused-import lints for items only used on some libc versions.
    #[allow(dead_code)]
    fn _assert_cstr(_: &CStr) {}
}
