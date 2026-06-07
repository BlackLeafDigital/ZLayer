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
    use std::process::{Child, ChildStdin, Command, Stdio};
    use std::sync::atomic::{AtomicI32, Ordering};
    use std::sync::{mpsc, Mutex, OnceLock};
    use std::thread;

    use zlayer_vzagent::proto::{self, Msg};

    // The in-guest WireGuard overlay bring-up (`Msg::OverlayConfig`). Lives in
    // its own module but uses this module's `Error`/`err`/`Result` helpers.
    #[path = "overlay.rs"]
    mod overlay;

    /// vsock address-family constant (Linux `AF_VSOCK`).
    const AF_VSOCK: libc::c_int = 40;
    /// "any CID" wildcard for binding a vsock listener.
    const VMADDR_CID_ANY: libc::c_uint = 0xFFFF_FFFF;

    /// PID of the currently-running primary workload, shared with the
    /// `Signal`/teardown paths. `0` means "no workload running".
    static WORKLOAD_PID: AtomicI32 = AtomicI32::new(0);

    /// Write half of the current workload's stdin pipe, shared with the
    /// `Stdin`/`StdinEof` handlers. `None` means "no workload running" or its
    /// stdin was never piped. Taking the value (`StdinEof`) drops the
    /// [`ChildStdin`], which closes the pipe so the workload sees EOF.
    static WORKLOAD_STDIN: Mutex<Option<ChildStdin>> = Mutex::new(None);

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

    /// True if the last OS error was `EEXIST`.
    fn is_eexist() -> bool {
        std::io::Error::last_os_error().raw_os_error() == Some(libc::EEXIST)
    }

    /// Create a character device node at `path` with the given `major`/`minor`,
    /// tolerating `EEXIST` (devtmpfs may have already populated it). Permission
    /// bits default to `0o666` (matching runc's standard `/dev` nodes).
    fn mknod_char(path: &str, major: u32, minor: u32, mode: libc::mode_t) -> Result<()> {
        let c_path = CString::new(path).map_err(|_| err("mknod: nul in path"))?;
        // SAFETY: `dev` is a valid device number from `makedev`; `c_path` is a
        // live NUL-terminated string; result is checked below.
        let dev = libc::makedev(major, minor);
        let rc = unsafe { libc::mknod(c_path.as_ptr(), libc::S_IFCHR | mode, dev) };
        if rc != 0 && !is_eexist() {
            return Err(errno(&format!("mknod {path} ({major},{minor})")));
        }
        // `mknod(2)` applies the process umask to `mode` (e.g. umask 022 turns a
        // requested 0666 into 0644), which would leave /dev/null et al. not
        // world-writable and break non-root workloads (`/dev/null: Permission
        // denied`). And on EEXIST the pre-existing node may have the wrong mode.
        // Force the exact mode unconditionally with chmod (umask-immune).
        // SAFETY: live NUL-terminated path; result checked.
        let rc = unsafe { libc::chmod(c_path.as_ptr(), mode) };
        if rc != 0 {
            return Err(errno(&format!("chmod {path}")));
        }
        Ok(())
    }

    /// `symlink(target, link)` tolerating `EEXIST` idempotently.
    fn symlink_idempotent(target: &str, link: &str) -> Result<()> {
        match std::os::unix::fs::symlink(target, link) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
            Err(e) => Err(err(format!("symlink {link} -> {target}: {e}"))),
        }
    }

    /// Provision the container's `/dev` exactly as a real OCI runtime (runc /
    /// containerd) does, **after** the pivot so the path resolves inside the
    /// container root.
    ///
    /// The pivot moved the boot devtmpfs to `/dev`; we mount a fresh `tmpfs`
    /// over it so the workload sees a clean, runc-shaped `/dev` (the devtmpfs
    /// stays mounted underneath, shadowed — harmless). Then we create the
    /// standard device nodes, the `/dev/fd`+std{in,out,err} symlinks (these are
    /// what fix bash process-substitution / `/dev/fd/63` in images like
    /// `postgres:*-alpine`), plus `/dev/pts` and `/dev/shm`.
    ///
    /// Individual node failures are logged and tolerated (mirroring
    /// `bring_up_network`), EXCEPT the `/dev/fd` symlink which is load-bearing
    /// for many entrypoints and is logged loudly on failure.
    fn setup_container_dev() -> Result<()> {
        // Fresh tmpfs over /dev: a clean, writable, runc-shaped device dir.
        // `nosuid` + `mode=0755` mirror runc's default /dev mount options.
        if let Err(e) = mount(
            "tmpfs",
            "/dev",
            "tmpfs",
            libc::MS_NOSUID | libc::MS_STRICTATIME,
            Some("mode=0755,size=65536k"),
        ) {
            // If the fresh mount fails we fall back to whatever /dev already
            // exists (the moved-in devtmpfs); node/symlink creation below is
            // still attempted and is idempotent.
            eprintln!("zlayer-vzagent: warning: tmpfs /dev mount failed, using existing /dev: {e}");
        }

        // Standard device nodes (mirrors runc's defaults). 0o666 except the
        // tty/console pair which runc also creates 0o666.
        let nodes: &[(&str, u32, u32)] = &[
            ("/dev/null", 1, 3),
            ("/dev/zero", 1, 5),
            ("/dev/full", 1, 7),
            ("/dev/random", 1, 8),
            ("/dev/urandom", 1, 9),
            ("/dev/tty", 5, 0),
            ("/dev/console", 5, 1),
            ("/dev/ptmx", 5, 2),
        ];
        for &(path, major, minor) in nodes {
            if let Err(e) = mknod_char(path, major, minor, 0o666) {
                eprintln!("zlayer-vzagent: warning: could not create {path}: {e}");
            }
        }

        // /dev/pts (devpts) — required for the /dev/ptmx multiplexor and for
        // exec/attach pseudo-terminals. `ptmxmode=0666` makes the bind-mounted
        // ptmx usable; `gid=5` matches the conventional `tty` group.
        if let Err(e) = mkdir_p("/dev/pts") {
            eprintln!("zlayer-vzagent: warning: mkdir /dev/pts: {e}");
        }
        if let Err(e) = mount(
            "devpts",
            "/dev/pts",
            "devpts",
            libc::MS_NOSUID | libc::MS_NOEXEC,
            Some("newinstance,ptmxmode=0666,mode=0620,gid=5"),
        ) {
            eprintln!("zlayer-vzagent: warning: mount /dev/pts: {e}");
        }

        // /dev/shm (tmpfs) — POSIX shared memory; many images (postgres
        // included) expect it to exist and be writable.
        if let Err(e) = mkdir_p("/dev/shm") {
            eprintln!("zlayer-vzagent: warning: mkdir /dev/shm: {e}");
        }
        if let Err(e) = mount(
            "tmpfs",
            "/dev/shm",
            "tmpfs",
            libc::MS_NOSUID | libc::MS_NODEV,
            Some("mode=1777,size=65536k"),
        ) {
            eprintln!("zlayer-vzagent: warning: mount /dev/shm: {e}");
        }

        // /proc must be mounted in the container root for the /dev/fd family of
        // symlinks to resolve. The pivot moves it in; verify and (re)mount if
        // it's somehow absent.
        if !Path::new("/proc/self/fd").exists() {
            if let Err(e) = mkdir_p("/proc") {
                eprintln!("zlayer-vzagent: warning: mkdir /proc: {e}");
            }
            if let Err(e) = mount("proc", "/proc", "proc", 0, None) {
                eprintln!("zlayer-vzagent: warning: mount /proc in container root: {e}");
            }
        }

        // The /dev/fd family — runc always creates these. `/dev/fd` in
        // particular is load-bearing for bash process substitution (the
        // `/dev/fd/63` failure in postgres:*-alpine's initdb), so failing to
        // create it is logged loudly.
        if let Err(e) = symlink_idempotent("/proc/self/fd", "/dev/fd") {
            eprintln!("zlayer-vzagent: ERROR: failed to create /dev/fd symlink (process substitution and /dev/fd/N will break): {e}");
        }
        for (target, link) in [
            ("/proc/self/fd/0", "/dev/stdin"),
            ("/proc/self/fd/1", "/dev/stdout"),
            ("/proc/self/fd/2", "/dev/stderr"),
        ] {
            if let Err(e) = symlink_idempotent(target, link) {
                eprintln!("zlayer-vzagent: warning: could not create {link} symlink: {e}");
            }
        }
        Ok(())
    }

    /// Ensure the container root has a usable `/etc/resolv.conf` after the pivot.
    ///
    /// The pre-pivot udhcpc lease script writes into the *initramfs* `/etc`,
    /// which the pivot swaps away (hence the benign
    /// "can't create /etc/resolv.conf: nonexistent directory" warning). The
    /// authoritative DNS for the workload comes from
    /// [`Msg::OverlayConfig`](crate::proto::Msg::OverlayConfig)
    /// (`overlay::apply_overlay_config` → `write_resolv_conf`, which writes this
    /// same post-pivot path). Until/unless that arrives, the workload still
    /// needs the file to exist so libc resolvers don't error out — so we create
    /// `/etc` and seed a sane default (`1.1.1.1` / `8.8.8.8`, matching the VZ
    /// NAT's upstreams) only when no resolv.conf is present yet. The overlay
    /// handler appends to (never clobbers) whatever is here.
    fn ensure_container_resolv_conf() -> Result<()> {
        mkdir_p("/etc")?;
        if Path::new("/etc/resolv.conf").exists() {
            return Ok(());
        }
        fs::write(
            "/etc/resolv.conf",
            "nameserver 1.1.1.1\nnameserver 8.8.8.8\n",
        )
        .map_err(|e| err(format!("seed /etc/resolv.conf: {e}")))
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

    /// Mount a host-shared virtiofs directory (addressed by its device `tag`)
    /// at `target` inside the **container root**.
    ///
    /// By the time the host sends [`Msg::Mount`] the agent has already
    /// `pivot_into_newroot`ed (see [`run`]), so the current `/` *is* the
    /// container root and `target` resolves against it directly — exactly the
    /// filesystem the workload runs in. The mount technique mirrors the rootfs
    /// mount in [`assemble_container_root`]: the virtiofs share is addressed by
    /// its tag as the mount source, fstype `"virtiofs"`, and `MS_RDONLY` is
    /// folded into the flags when a read-only mount is requested (the same way
    /// the rootfs lower layer is mounted read-only). The target directory is
    /// created first (`mkdir -p`).
    fn mount_share(tag: &str, target: &str, readonly: bool) -> Result<()> {
        mkdir_p(target)?;
        let flags = if readonly { libc::MS_RDONLY } else { 0 };
        mount(tag, target, "virtiofs", flags, None)
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

        // The virtio-net device is probed asynchronously during boot, so it may
        // not exist the instant PID 1 runs, and its name is not guaranteed to be
        // `eth0`. Discover the first non-loopback interface, waiting briefly for
        // it to appear.
        let Some(iface) = wait_for_eth(std::time::Duration::from_secs(5)) else {
            eprintln!(
                "zlayer-vzagent: warning: no ethernet interface appeared; \
                 continuing without networking"
            );
            return Ok(());
        };

        if let Err(e) = set_link_up(&iface) {
            // Not fatal: the workload may not need networking, and the host
            // surfaces the warning. Log and continue.
            eprintln!("zlayer-vzagent: warning: failed to set {iface} up: {e}");
            return Ok(());
        }

        if interface_has_ipv4(&iface) {
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
            cmd.args(["-i", iface.as_str(), "-f", "-q", "-n", "-t", "5", "-T", "2"]);
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

    /// Wait up to `timeout` for the guest's *ethernet* NIC (the virtio-net
    /// device) to appear in `/sys/class/net`, returning its name. virtio-net is
    /// probed asynchronously, so the NIC may not exist the instant PID 1 runs.
    ///
    /// We must NOT just grab the first non-loopback interface: the kernel
    /// auto-creates an IPv6-in-IPv4 tunnel `sit0` (`ARPHRD_SIT`) as soon as the
    /// `sit` module is present, and directory order is unspecified, so a naive
    /// scan can pick `sit0` and run DHCP on the wrong (useless) interface while
    /// the real `eth0` stays down. Filter on the interface's `type` sysfs file
    /// being `ARPHRD_ETHER` (`1`), which selects virtio-net and excludes `lo`
    /// (`772`), `sit0` (`776`), and other tunnels.
    fn wait_for_eth(timeout: std::time::Duration) -> Option<String> {
        const ARPHRD_ETHER: &str = "1";
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if let Ok(entries) = fs::read_dir("/sys/class/net") {
                for e in entries.flatten() {
                    let name = e.file_name().to_string_lossy().into_owned();
                    if name == "lo" {
                        continue;
                    }
                    // Only consider real ethernet links (virtio-net is ETHER).
                    let type_path = format!("/sys/class/net/{name}/type");
                    // First ethernet device wins; anything else (tunnels, read
                    // errors) is skipped by falling through to the next entry.
                    if let Ok(t) = fs::read_to_string(&type_path) {
                        if t.trim() == ARPHRD_ETHER {
                            return Some(name);
                        }
                    }
                }
            }
            if std::time::Instant::now() >= deadline {
                return None;
            }
            thread::sleep(std::time::Duration::from_millis(100));
        }
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
        if unsafe {
            libc::ioctl(
                owned.as_raw_fd(),
                libc::SIOCGIFFLAGS as libc::Ioctl,
                &mut ifr,
            )
        } < 0
        {
            return Err(errno(&format!("SIOCGIFFLAGS {ifname}")));
        }
        // SAFETY: ifr_flags union field is valid after the get ioctl above.
        unsafe {
            ifr.ifr_ifru.ifru_flags |= (libc::IFF_UP | libc::IFF_RUNNING) as libc::c_short;
        }
        // SAFETY: ioctl with the updated ifreq on a valid fd.
        if unsafe { libc::ioctl(owned.as_raw_fd(), libc::SIOCSIFFLAGS as libc::Ioctl, &ifr) } < 0 {
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
        unsafe {
            libc::ioctl(
                owned.as_raw_fd(),
                libc::SIOCGIFADDR as libc::Ioctl,
                &mut ifr,
            ) == 0
        }
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

        // Make all mount propagation private so the root move below is allowed.
        mount("", "/", "", libc::MS_REC | libc::MS_PRIVATE, None)?;

        // We are PID 1 running from the initramfs, which is a rootfs/ramfs.
        // `pivot_root(2)` CANNOT move the initial rootfs and returns EINVAL on
        // it, so the kernel-documented technique for the initramfs→real-root
        // transition is `switch_root` semantics: move the new-root mount on top
        // of "/" and `chroot` into it (exactly what busybox `switch_root` does).
        let c_newroot = CString::new(new_root).map_err(|_| err("nul in new_root"))?;
        // SAFETY: chdir into the overlay mount (`/newroot`).
        if unsafe { libc::chdir(c_newroot.as_ptr()) } != 0 {
            return Err(errno("chdir(/newroot)"));
        }
        // Move the overlay mount (now the cwd) onto "/". `.` resolves to the
        // overlay mount itself, which is required for MS_MOVE onto the old root.
        mount(".", "/", "", libc::MS_MOVE, None)?;
        let c_dot = CString::new(".").unwrap();
        // SAFETY: chroot into the relocated root ("." is now "/").
        if unsafe { libc::chroot(c_dot.as_ptr()) } != 0 {
            return Err(errno("chroot(.)"));
        }
        let c_root = CString::new("/").unwrap();
        // SAFETY: chdir to "/" inside the new root.
        if unsafe { libc::chdir(c_root.as_ptr()) } != 0 {
            return Err(errno("chdir(/) after chroot"));
        }
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

    impl AsRawFd for VsockStream {
        fn as_raw_fd(&self) -> RawFd {
            self.fd.as_raw_fd()
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

    /// Fallback `PATH` applied to spawned children when neither the image nor
    /// the spec supplied one.
    ///
    /// `Command` resolves the *program* (`argv[0]`) via the agent's own libc
    /// search, but the program itself — most importantly `/bin/sh` running a
    /// `sh -c "..."` workload — relies on the **child's** `$PATH` to locate any
    /// external command it invokes (`sleep`, `ls`, …). PID 1 is exec'd by the
    /// kernel with an empty environment, and the host only forwards the spec's
    /// env, so without this default a workload like `sh -c "sleep 1 && exit 0"`
    /// fails to find `sleep` and the shell exits **127** ("command not found")
    /// with no stdout — exactly the bug this constant fixes. The value matches
    /// Docker's built-in default and the other `ZLayer` runtimes
    /// (`wasm.rs`'s `build_env_vars`).
    const DEFAULT_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

    /// Apply the host-provided environment to `cmd`, clearing any inherited
    /// (PID 1) environment first and guaranteeing a usable `PATH`.
    ///
    /// The agent is PID 1 with an effectively empty environment, so we start
    /// from `env_clear()` and layer only what the host sent. If the host's env
    /// carries no `PATH` (a bare image with no `ENV PATH`), we inject
    /// [`DEFAULT_PATH`] so the workload's own command lookups succeed; an
    /// explicit `PATH` from the spec/image always wins.
    fn apply_env(cmd: &mut Command, env: &[(String, String)]) {
        cmd.env_clear();
        let mut saw_path = false;
        for (k, v) in env {
            if k == "PATH" {
                saw_path = true;
            }
            cmd.env(k, v);
        }
        if !saw_path {
            cmd.env("PATH", DEFAULT_PATH);
        }
    }

    /// Resolve the workload's working directory.
    ///
    /// Docker `WorkingDir` semantics: an empty string means "no explicit
    /// workdir" (use the root), and a configured directory is *created* if it
    /// does not exist rather than aborting the spawn. We therefore ignore an
    /// empty/whitespace value, and for a real absolute path best-effort
    /// `mkdir -p` it before handing it to `Command::current_dir`. A failure to
    /// create it is non-fatal: we simply leave the cwd unset so the workload
    /// still starts from `/` instead of failing with `ENOENT` (which the host
    /// would otherwise see as a spawn error, never a clean run).
    fn resolve_cwd(cwd: Option<&str>) -> Option<String> {
        let dir = cwd.map(str::trim).filter(|d| !d.is_empty())?;
        // Best-effort create so a baked-in WORKDIR that isn't materialised in
        // the image layers still works (matching Docker).
        let _ = fs::create_dir_all(dir);
        if Path::new(dir).is_dir() {
            Some(dir.to_string())
        } else {
            None
        }
    }

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
        apply_env(&mut cmd, env);
        if let Some(dir) = resolve_cwd(cwd) {
            cmd.current_dir(dir);
        }
        // Pipe stdin so the host can stream interactive input (`-it`) via
        // `Msg::Stdin`/`Msg::StdinEof`; the write half is retained in
        // [`WORKLOAD_STDIN`] by the `Run` handler.
        cmd.stdin(Stdio::piped());
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

    /// Write a chunk of host-supplied stdin to the running workload.
    ///
    /// Robust if stdin was never piped (no workload, or its stdin handle was
    /// already taken by [`StdinEof`](Msg::StdinEof)): such a write is a no-op,
    /// never a panic. A write error (e.g. the workload closed its stdin / exited)
    /// is surfaced to the caller, and the now-broken handle is dropped so we stop
    /// trying.
    fn write_workload_stdin(bytes: &[u8]) -> Result<()> {
        let mut guard = WORKLOAD_STDIN
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(stdin) = guard.as_mut() else {
            // No stdin to write to (no workload, or EOF already sent). Silently
            // ignore: the host may race a stdin chunk against workload exit.
            return Ok(());
        };
        if let Err(e) = stdin.write_all(bytes) {
            // The pipe is broken (workload closed stdin / exited). Drop the
            // handle so subsequent chunks are quietly ignored, and report once.
            *guard = None;
            return Err(err(format!("write workload stdin: {e}")));
        }
        Ok(())
    }

    /// Close the workload's stdin so it observes EOF, by taking and dropping the
    /// retained [`ChildStdin`]. Idempotent and safe if stdin was never piped.
    fn close_workload_stdin() {
        let mut guard = WORKLOAD_STDIN
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Dropping the ChildStdin closes the write end of the pipe -> EOF.
        *guard = None;
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
        apply_env(&mut cmd, env);
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
    // Port forwarding (vsock <-> guest-local TCP tunnel)
    // ----------------------------------------------------------------------

    /// Copy bytes from `src` to `dst` until EOF or error, then half-close the
    /// write side of `dst` so the peer observes EOF. Best-effort: any IO error
    /// simply ends the copy (the other direction's thread will then also wind
    /// down once its peer closes).
    fn pump<R: Read, W: Write + AsRawFd>(mut src: R, mut dst: W) {
        let mut buf = [0u8; 16 * 1024];
        loop {
            match src.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if dst.write_all(&buf[..n]).is_err() {
                        break;
                    }
                }
                // Retry interrupted reads; any other error ends the copy.
                Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
        let _ = dst.flush();
        // Half-close the write direction so the peer sees EOF promptly even if
        // the reverse copy is still draining. SHUT_WR on the destination fd.
        // SAFETY: `dst` owns a live fd for the duration of this call.
        unsafe {
            libc::shutdown(dst.as_raw_fd(), libc::SHUT_WR);
        }
    }

    /// Turn a connection that opened with [`Msg::Forward`] into a transparent
    /// byte tunnel between the vsock connection and `127.0.0.1:<port>` inside the
    /// guest.
    ///
    /// Opens a guest-local TCP connection, then bidirectionally copies raw bytes
    /// (one thread per direction) until either side reaches EOF/error. No proto
    /// framing is exchanged after the `Forward` frame — this is a transparent
    /// L4 tunnel. If the TCP connect fails, a single `Msg::Error` frame is
    /// written back over the still-framed vsock connection before it closes.
    fn serve_forward(conn: VsockStream, port: u16) -> Result<()> {
        let tcp = match std::net::TcpStream::connect(("127.0.0.1", port)) {
            Ok(s) => s,
            Err(e) => {
                // The connection has not been switched to raw mode yet, so we can
                // still report the failure as a framed protocol error.
                let mut w = conn;
                let _ = proto::write_frame(
                    &mut w,
                    &Msg::Error {
                        message: format!("forward connect 127.0.0.1:{port}: {e}"),
                    },
                );
                return Ok(());
            }
        };
        // Independent owned handles for each direction. The vsock side clones the
        // fd; the TCP side clones the TcpStream.
        let vsock_read = conn.try_clone()?;
        let vsock_write = conn;
        let tcp_read = tcp.try_clone().map_err(Error::from)?;
        let tcp_write = tcp;

        // vsock -> tcp on this thread, tcp -> vsock on a helper thread.
        let h = thread::spawn(move || {
            pump(tcp_read, vsock_write);
        });
        pump(vsock_read, tcp_write);
        let _ = h.join();
        Ok(())
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

        // Peek the FIRST frame before standing up the framed writer thread: a
        // `Forward` connection is a transparent L4 tunnel with NO proto framing
        // after this point, so it must NOT share the framed writer machinery.
        match proto::read_frame(&mut reader) {
            Ok(Msg::Forward { port }) => {
                // `reader` and `conn` are independent dup'd handles of the same
                // vsock fd; hand the original `conn` to the splicer (it reuses it
                // for both directions via its own try_clone). Drop our reader dup
                // so only the splice owns live copies.
                drop(reader);
                serve_forward(conn, port)
            }
            Ok(first) => {
                // A normal control connection: process the already-read first
                // frame, then continue the framed loop below.
                serve_framed(reader, conn, Some(first))
            }
            Err(proto::ProtoError::Io(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                // Host closed before sending anything: nothing to do.
                Ok(())
            }
            Err(e) => {
                eprintln!("zlayer-vzagent: control conn read_frame error: {e}");
                Err(Error::from(e))
            }
        }
    }

    /// The framed control loop (Run/Exec/Signal). `first` is the already-read
    /// leading frame (if any); subsequent frames are read from `reader`. All
    /// outbound frames go through a single serialized writer thread over `writer`.
    fn serve_framed(
        mut reader: VsockStream,
        writer: VsockStream,
        mut first: Option<Msg>,
    ) -> Result<()> {
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
            let msg = match first.take() {
                Some(m) => m,
                None => match proto::read_frame(&mut reader) {
                    Ok(m) => m,
                    Err(proto::ProtoError::Io(e))
                        if e.kind() == std::io::ErrorKind::UnexpectedEof =>
                    {
                        // Host closed the control connection cleanly.
                        break;
                    }
                    Err(e) => {
                        let _ = tx.send(Msg::Error {
                            message: format!("read_frame: {e}"),
                        });
                        break;
                    }
                },
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
                            // Retain the workload's stdin write half so the
                            // `Stdin`/`StdinEof` handlers below can feed it. The
                            // child's stdin was piped in `spawn_workload`.
                            {
                                let mut guard = WORKLOAD_STDIN
                                    .lock()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                                *guard = child.stdin.take();
                            }
                            let _ = tx.send(Msg::Started { pid });
                            let (h_out, h_err) = stream_child_output(&mut child, &tx);
                            // Reap + report exit on a DEDICATED thread so this
                            // control loop keeps reading frames while the
                            // workload runs — interactive `-it` stdin
                            // (`Msg::Stdin`/`Msg::StdinEof`), `Signal`, and
                            // `Mount` all arrive on THIS connection after `Run`
                            // and must be serviced concurrently rather than
                            // blocking until the workload exits.
                            let exit_tx = tx.clone();
                            thread::spawn(move || {
                                // Hand reaping to the dedicated reaper thread: do
                                // NOT call child.wait() (that would race the
                                // reaper for the child's status). We only kept
                                // `child` to own the stdio pipes; forget it so
                                // its Drop never touches the pid the reaper now
                                // owns.
                                let raw_status = wait_via_reaper(&status_rx);
                                std::mem::forget(child);
                                // Ensure all buffered output has been pumped.
                                let _ = h_out.join();
                                let _ = h_err.join();
                                WORKLOAD_PID.store(0, Ordering::SeqCst);
                                // The workload is gone; drop its stdin handle so
                                // late stdin chunks become no-ops.
                                close_workload_stdin();
                                if let Some(s) = raw_status {
                                    let _ = exit_tx.send(Msg::Exited {
                                        code: status_to_code(s),
                                    });
                                } else {
                                    let _ = exit_tx.send(Msg::Error {
                                        message: "wait: reaper channel closed".into(),
                                    });
                                    let _ = exit_tx.send(Msg::Exited { code: -1 });
                                }
                            });
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
                Msg::Mount {
                    tag,
                    target,
                    readonly,
                } => {
                    // The agent has already pivoted into the container root, so
                    // `target` resolves against the workload's filesystem.
                    // Failure is reported but never aborts the agent.
                    if let Err(e) = mount_share(&tag, &target, readonly) {
                        let _ = tx.send(Msg::Error {
                            message: format!("Mount {tag} -> {target} failed: {e}"),
                        });
                    }
                }
                Msg::Stdin(bytes) => {
                    if let Err(e) = write_workload_stdin(&bytes) {
                        let _ = tx.send(Msg::Error {
                            message: format!("Stdin failed: {e}"),
                        });
                    }
                }
                Msg::StdinEof => {
                    close_workload_stdin();
                }
                Msg::OverlayConfig {
                    overlay_ip,
                    prefix_len,
                    private_key,
                    listen_port,
                    peers,
                    dns_server,
                    dns_domain,
                } => {
                    // Stand up the in-guest kernel WireGuard overlay
                    // (`zl-overlay0`) and join the mesh. Like `bring_up_network`,
                    // a failure here is reported to the host but never aborts the
                    // agent: a workload that doesn't use the overlay is
                    // unaffected, and PID 1 must stay alive.
                    if let Err(e) = overlay::apply_overlay_config(
                        &overlay_ip,
                        prefix_len,
                        &private_key,
                        listen_port,
                        &peers,
                        dns_server.as_deref(),
                        dns_domain.as_deref(),
                    ) {
                        eprintln!("zlayer-vzagent: overlay bring-up failed: {e}");
                        let _ = tx.send(Msg::Error {
                            message: format!("OverlayConfig failed: {e}"),
                        });
                    } else {
                        eprintln!(
                            "zlayer-vzagent: overlay {} up on {} ({} peer(s))",
                            overlay_ip,
                            overlay::OVERLAY_IFNAME,
                            peers.len()
                        );
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

        // Bring up the guest NIC + DHCP concurrently with assembling the
        // container root, to overlap the DHCP round-trip with the virtiofs /
        // overlay mounts. It MUST finish before `pivot_into_newroot`: it relies
        // on the initramfs busybox/udhcpc and writes into the initramfs `/etc`,
        // both of which the pivot swaps away — so we join it before pivoting.
        // `bring_up_network` only reads `/sys/class/net` and operates on the NIC
        // + sockets, which don't overlap the `/newroot` mounts `assemble_*` does.
        let net_thread = thread::Builder::new()
            .name("vzagent-net".into())
            .spawn(|| {
                if let Err(e) = bring_up_network() {
                    eprintln!("zlayer-vzagent: network bring-up failed: {e}");
                }
            })
            .map_err(|e| err(format!("spawn net thread: {e}")))?;
        assemble_container_root()?;
        // Join before pivot (see above). A panicked net thread is non-fatal —
        // networking is best-effort, exactly as the inline path was.
        let _ = net_thread.join();
        pivot_into_newroot()?;

        // Now that `/` is the container root, give the workload the minimal
        // runtime environment a real OCI runtime provides: a runc-shaped `/dev`
        // (standard nodes + the `/dev/fd` family of symlinks + pts/shm) and a
        // usable `/etc/resolv.conf`. Best-effort: a malformed `/dev` would only
        // hurt the workload, so failures are logged, not fatal.
        if let Err(e) = setup_container_dev() {
            eprintln!("zlayer-vzagent: warning: container /dev setup failed: {e}");
        }
        if let Err(e) = ensure_container_resolv_conf() {
            eprintln!("zlayer-vzagent: warning: ensuring /etc/resolv.conf failed: {e}");
        }

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

    #[cfg(test)]
    mod tests {
        use super::{apply_env, resolve_cwd, status_to_code, DEFAULT_PATH};
        use std::process::Command;

        /// Read back the environment a `Command` would hand its child. There is
        /// no public getter, so we observe behaviour through `get_envs` (which
        /// reflects `env_clear` + `env` calls in order).
        fn collected_env(cmd: &Command) -> Vec<(String, String)> {
            cmd.get_envs()
                .filter_map(|(k, v)| {
                    Some((
                        k.to_string_lossy().into_owned(),
                        v?.to_string_lossy().into_owned(),
                    ))
                })
                .collect()
        }

        #[test]
        fn apply_env_injects_default_path_when_absent() {
            let mut cmd = Command::new("true");
            apply_env(&mut cmd, &[("FOO".into(), "bar".into())]);
            let env = collected_env(&cmd);
            assert!(
                env.contains(&("FOO".to_string(), "bar".to_string())),
                "spec env must be forwarded"
            );
            assert!(
                env.contains(&("PATH".to_string(), DEFAULT_PATH.to_string())),
                "a default PATH must be injected when the spec carries none (this is the 127 fix); got {env:?}"
            );
        }

        #[test]
        fn apply_env_preserves_explicit_path() {
            let mut cmd = Command::new("true");
            apply_env(
                &mut cmd,
                &[
                    ("PATH".into(), "/opt/custom/bin".into()),
                    ("A".into(), "1".into()),
                ],
            );
            let env = collected_env(&cmd);
            assert!(
                env.contains(&("PATH".to_string(), "/opt/custom/bin".to_string())),
                "an explicit PATH from the spec/image must win over the default"
            );
            assert!(
                !env.contains(&("PATH".to_string(), DEFAULT_PATH.to_string())),
                "the default PATH must not be appended when an explicit one exists"
            );
        }

        #[test]
        fn resolve_cwd_ignores_empty_and_whitespace() {
            // Docker WorkingDir="" means "no explicit workdir": must be None so
            // the spawn never fails with ENOENT on an empty current_dir.
            assert_eq!(resolve_cwd(None), None);
            assert_eq!(resolve_cwd(Some("")), None);
            assert_eq!(resolve_cwd(Some("   ")), None);
        }

        #[test]
        fn resolve_cwd_creates_and_keeps_real_dir() {
            let base = std::env::temp_dir().join(format!("vzagent-cwd-{}", std::process::id()));
            let dir = base.join("nested/work");
            let dir_str = dir.to_string_lossy().into_owned();
            let _ = std::fs::remove_dir_all(&base);
            // Directory does not exist yet — resolve_cwd must create it (Docker
            // WORKDIR semantics) and return it.
            assert_eq!(resolve_cwd(Some(&dir_str)), Some(dir_str.clone()));
            assert!(
                dir.is_dir(),
                "resolve_cwd must mkdir -p a configured workdir"
            );
            let _ = std::fs::remove_dir_all(&base);
        }

        #[test]
        fn status_to_code_maps_exit_and_signal() {
            // A clean exit 0 must round-trip to 0 (the exit-0 regression).
            let exited_0 = 0; // WIFEXITED, WEXITSTATUS == 0
            assert_eq!(status_to_code(exited_0), 0);
            // exit(42): WEXITSTATUS is in the high byte on Linux wait status.
            let exited_42 = 42 << 8;
            assert_eq!(status_to_code(exited_42), 42);
            // exit(127): the "command not found" code must pass through, not be
            // remapped — so the host sees the true shell failure.
            let exited_127 = 127 << 8;
            assert_eq!(status_to_code(exited_127), 127);
            // Killed by SIGKILL (9): low 7 bits hold the signal number; mapped
            // to 128 + signum.
            let signaled_9 = 9;
            assert_eq!(status_to_code(signaled_9), 128 + 9);
        }
    }
}
