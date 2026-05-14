use std::path::{Path, PathBuf};

/// Centralized filesystem path resolution for ZLayer.
///
/// All ZLayer crates should use this instead of hardcoding paths.
pub struct ZLayerDirs {
    data_dir: PathBuf,
}

impl ZLayerDirs {
    /// Create from an explicit data directory.
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
        }
    }

    /// Create using the platform default data directory.
    pub fn system_default() -> Self {
        Self::new(Self::default_data_dir())
    }

    // -- Platform defaults (associated functions) ----------------------------

    /// Platform-aware default data directory.
    ///
    /// - `$ZLAYER_DATA_DIR` (if set and non-empty) overrides every other source.
    /// - macOS: `~/.zlayer`
    /// - Linux (root): `/var/lib/zlayer`
    /// - Linux (user): `~/.zlayer`
    /// - Windows: `%ProgramData%\ZLayer` (system) or `C:\ProgramData\ZLayer`
    ///   fallback. HCS-backed nodes run as SYSTEM so the system-wide
    ///   `ProgramData` location is the right default.
    pub fn default_data_dir() -> PathBuf {
        if let Some(env_dir) = std::env::var_os("ZLAYER_DATA_DIR") {
            if !env_dir.is_empty() {
                return PathBuf::from(env_dir);
            }
        }
        platform_default_data_dir()
    }

    /// Detect the data directory of an existing installation.
    ///
    /// On Linux, if not root, checks whether `/var/lib/zlayer/daemon.json`
    /// exists (indicating a system-level install) and returns
    /// `/var/lib/zlayer` if so. On Windows, probes `%ProgramData%\ZLayer`
    /// for a `daemon.json` marker in case the caller lacks the env var but
    /// a prior system install is present. Otherwise falls back to
    /// [`default_data_dir`].
    pub fn detect_data_dir() -> PathBuf {
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            if !is_root() {
                let system_data = PathBuf::from("/var/lib/zlayer");
                if system_data.join("daemon.json").exists() {
                    return system_data;
                }
            }
        }
        #[cfg(target_os = "windows")]
        {
            let system_data = windows_program_data_root();
            if system_data.join("daemon.json").exists() {
                return system_data;
            }
        }
        Self::default_data_dir()
    }

    /// Default runtime directory.
    ///
    /// - Linux: `/var/run/zlayer`
    /// - macOS: `{default_data_dir}/run`
    /// - Windows: `{default_data_dir}\run` (i.e. `%ProgramData%\ZLayer\run`)
    pub fn default_run_dir() -> PathBuf {
        Self::default_run_dir_for(&Self::default_data_dir())
    }

    /// Data-dir-aware default run directory.
    ///
    /// Returns the platform's system default (e.g. `/var/run/zlayer` on Linux)
    /// when `data_dir` matches [`Self::default_data_dir`]; otherwise returns
    /// `{data_dir}/run`. This preserves the FHS layout for stock installs while
    /// letting `--data-dir /tmp/foo` get a fully isolated runtime directory.
    pub fn default_run_dir_for(data_dir: &Path) -> PathBuf {
        let system_default = platform_default_data_dir();
        if data_dir == system_default.as_path() {
            #[cfg(not(any(target_os = "macos", target_os = "windows")))]
            {
                return PathBuf::from("/var/run/zlayer");
            }
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            {
                return system_default.join("run");
            }
        }
        data_dir.join("run")
    }

    /// Default log directory.
    ///
    /// - Linux: `/var/log/zlayer`
    /// - macOS: `{default_data_dir}/logs`
    /// - Windows: `{default_data_dir}\logs` (i.e. `%ProgramData%\ZLayer\logs`)
    pub fn default_log_dir() -> PathBuf {
        Self::default_log_dir_for(&Self::default_data_dir())
    }

    /// Data-dir-aware default log directory.
    ///
    /// Returns the platform's system default (e.g. `/var/log/zlayer` on Linux)
    /// when `data_dir` matches [`Self::default_data_dir`]; otherwise returns
    /// `{data_dir}/logs`. This preserves the FHS layout for stock installs
    /// while letting `--data-dir /tmp/foo` get a fully isolated log directory.
    pub fn default_log_dir_for(data_dir: &Path) -> PathBuf {
        let system_default = platform_default_data_dir();
        if data_dir == system_default.as_path() {
            #[cfg(not(any(target_os = "macos", target_os = "windows")))]
            {
                return PathBuf::from("/var/log/zlayer");
            }
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            {
                return system_default.join("logs");
            }
        }
        data_dir.join("logs")
    }

    /// Default Unix socket path.
    ///
    /// - Linux: `/var/run/zlayer.sock`
    /// - macOS: `{default_data_dir}/run/zlayer.sock`
    /// - Windows: `tcp://127.0.0.1:3669`
    pub fn default_socket_path() -> String {
        Self::default_socket_path_for(&Self::default_data_dir())
    }

    /// Data-dir-aware default daemon socket path.
    ///
    /// On Windows always returns `tcp://127.0.0.1:3669` regardless of
    /// `data_dir` (the daemon listens on TCP loopback, not a filesystem
    /// socket). On Unix, returns the platform's system default when
    /// `data_dir` matches [`Self::default_data_dir`]; otherwise returns
    /// `{data_dir}/run/zlayer.sock`. Stock installs keep their FHS-style
    /// path while `--data-dir /tmp/foo` gets an isolated socket.
    pub fn default_socket_path_for(data_dir: &Path) -> String {
        #[cfg(target_os = "windows")]
        {
            let _ = data_dir;
            "tcp://127.0.0.1:3669".to_string()
        }
        #[cfg(not(target_os = "windows"))]
        {
            let system_default = platform_default_data_dir();
            if data_dir == system_default.as_path() {
                #[cfg(target_os = "macos")]
                {
                    return system_default
                        .join("run")
                        .join("zlayer.sock")
                        .to_string_lossy()
                        .into_owned();
                }
                #[cfg(not(target_os = "macos"))]
                {
                    return "/var/run/zlayer.sock".to_string();
                }
            }
            let natural = data_dir
                .join("run")
                .join("zlayer.sock")
                .to_string_lossy()
                .into_owned();
            if natural.len() <= SUN_PATH_MAX {
                natural
            } else {
                socket_safe_fallback(data_dir, "daemon")
            }
        }
    }

    /// Default Docker-compatible API socket path.
    ///
    /// - Linux (root): `/var/run/zlayer/docker.sock`
    /// - Linux (user, `XDG_RUNTIME_DIR` set): `{XDG_RUNTIME_DIR}/zlayer/docker.sock`
    /// - Linux (user, no `XDG_RUNTIME_DIR`): `{default_data_dir}/run/docker.sock`
    /// - macOS: `{default_data_dir}/run/docker.sock`
    /// - Windows: `\\.\pipe\zlayer-docker`
    pub fn default_docker_socket_path() -> String {
        #[cfg(target_os = "windows")]
        {
            r"\\.\pipe\zlayer-docker".to_string()
        }
        #[cfg(not(target_os = "windows"))]
        {
            #[cfg(target_os = "macos")]
            {
                let path = Self::default_data_dir()
                    .join("run")
                    .join("docker.sock")
                    .to_string_lossy()
                    .into_owned();
                if path.len() <= SUN_PATH_MAX {
                    path
                } else {
                    socket_safe_fallback(&Self::default_data_dir(), "docker")
                }
            }
            #[cfg(not(target_os = "macos"))]
            {
                if is_root() {
                    "/var/run/zlayer/docker.sock".to_string()
                } else if let Some(xdg) = std::env::var_os("XDG_RUNTIME_DIR") {
                    let xdg_path = PathBuf::from(&xdg);
                    let mut p = xdg_path.clone();
                    p.push("zlayer");
                    p.push("docker.sock");
                    let path = p.to_string_lossy().into_owned();
                    if path.len() <= SUN_PATH_MAX {
                        path
                    } else {
                        socket_safe_fallback(&xdg_path, "docker")
                    }
                } else {
                    let path = Self::default_data_dir()
                        .join("run")
                        .join("docker.sock")
                        .to_string_lossy()
                        .into_owned();
                    if path.len() <= SUN_PATH_MAX {
                        path
                    } else {
                        socket_safe_fallback(&Self::default_data_dir(), "docker")
                    }
                }
            }
        }
    }

    /// Preferred system directory for the `zlayer` binary.
    ///
    /// Tries `/usr/local/bin` first (standard FHS, writable on most systems).
    /// Falls back to `{data_dir}/bin` (`/var/lib/zlayer/bin` on Linux as root)
    /// which is always writable since `ZLayer` owns that directory.
    ///
    /// On macOS and Windows, returns `/usr/local/bin` or the data-dir `bin`
    /// subdirectory respectively.
    pub fn default_binary_dir() -> PathBuf {
        // Probe /usr/local/bin writability — metadata mode bits lie on overlayfs
        #[cfg(unix)]
        {
            let probe = PathBuf::from("/usr/local/bin/.zlayer_write_probe");
            if std::fs::write(&probe, b"").is_ok() {
                let _ = std::fs::remove_file(&probe);
                return PathBuf::from("/usr/local/bin");
            }
        }
        // Fallback: our own bin dir (always writable)
        let dirs = Self::system_default();
        let bin_dir = dirs.bin();
        let _ = std::fs::create_dir_all(&bin_dir);
        bin_dir
    }

    // -- Core subdirectories -------------------------------------------------

    /// Root data directory.
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// Container state directory (`{data}/containers`).
    pub fn containers(&self) -> PathBuf {
        self.data_dir.join("containers")
    }

    /// Unpacked image rootfs directory (`{data}/rootfs`).
    pub fn rootfs(&self) -> PathBuf {
        self.data_dir.join("rootfs")
    }

    /// OCI bundle directory (`{data}/bundles`).
    pub fn bundles(&self) -> PathBuf {
        self.data_dir.join("bundles")
    }

    /// Image/blob cache directory (`{data}/cache`).
    pub fn cache(&self) -> PathBuf {
        self.data_dir.join("cache")
    }

    /// Named volumes directory (`{data}/volumes`).
    pub fn volumes(&self) -> PathBuf {
        self.data_dir.join("volumes")
    }

    /// Project git clones directory (`{data}/projects`). Persistent state —
    /// per-project working copies live at `{data}/projects/{project_id}`.
    pub fn projects(&self) -> PathBuf {
        self.data_dir.join("projects")
    }

    /// WASM module cache directory (`{data}/wasm`).
    pub fn wasm(&self) -> PathBuf {
        self.data_dir.join("wasm")
    }

    /// AOT-compiled WASM cache directory (`{data}/wasm/compiled`).
    pub fn wasm_compiled(&self) -> PathBuf {
        self.data_dir.join("wasm").join("compiled")
    }

    /// Encrypted secrets store directory (`{data}/secrets`).
    pub fn secrets(&self) -> PathBuf {
        self.data_dir.join("secrets")
    }

    /// TLS certificate storage directory (`{data}/certs`).
    pub fn certs(&self) -> PathBuf {
        self.data_dir.join("certs")
    }

    /// Raft consensus data directory (`{data}/raft`).
    pub fn raft(&self) -> PathBuf {
        self.data_dir.join("raft")
    }

    /// Admin password file path (`{data}/admin_password`).
    pub fn admin_password(&self) -> PathBuf {
        self.data_dir.join("admin_password")
    }

    /// Path to the persisted local-admin bearer token file.
    ///
    /// On Linux/macOS this file is informational — the daemon's UDS middleware
    /// already injects the bearer into UDS-originated requests. On Windows the
    /// `DaemonClient` reads this file on connect to authenticate against the
    /// loopback TCP listener (which has no socket-path-based local-admin
    /// bypass).
    ///
    /// Default: `<data_dir>/admin_bearer.token`
    ///
    /// On Windows this resolves under `%ProgramData%\ZLayer` so the file
    /// inherits the parent ACL (SYSTEM + Administrators write, Users read),
    /// which is adequate for the local-admin bearer.
    #[must_use]
    pub fn admin_bearer_path(&self) -> PathBuf {
        self.data_dir.join("admin_bearer.token")
    }

    /// Daemon metadata file path (`{data}/daemon.json`).
    pub fn daemon_json(&self) -> PathBuf {
        self.data_dir.join("daemon.json")
    }

    /// Path to the agent's local IPAM (per-node slice allocator) state file.
    pub fn agent_ipam_state(&self) -> PathBuf {
        self.data_dir.join("agent_ipam.json")
    }

    /// Logs subdirectory under data_dir (`{data}/logs`).
    /// Used on macOS where logs live under the user data dir.
    pub fn logs(&self) -> PathBuf {
        self.data_dir.join("logs")
    }

    // -- macOS sandbox / builder paths ---------------------------------------

    /// macOS VM state directory (`{data}/vms`).
    pub fn vms(&self) -> PathBuf {
        self.data_dir.join("vms")
    }

    /// OCI image storage directory (`{data}/images`).
    pub fn images(&self) -> PathBuf {
        self.data_dir.join("images")
    }

    /// Local binary directory (`{data}/bin`).
    pub fn bin(&self) -> PathBuf {
        self.data_dir.join("bin")
    }

    /// Toolchain download cache directory (`{data}/toolchain-cache`).
    pub fn toolchain_cache(&self) -> PathBuf {
        self.data_dir.join("toolchain-cache")
    }

    /// Temporary build directory (`{data}/tmp`).
    pub fn tmp(&self) -> PathBuf {
        self.data_dir.join("tmp")
    }

    /// Create a uniquely-named scratch directory under `{data}/tmp`.
    ///
    /// Returns a [`zlayer_types::Scratch`] RAII guard — the directory is
    /// removed when the guard is dropped. Use this instead of
    /// `tempfile::tempdir()` so scratch data lives on the configured data
    /// filesystem rather than `/tmp`, which is tmpfs (RAM-backed) on most
    /// modern Linux distros and risks OOM for large scratch data
    /// (build contexts, image tarballs, layer staging, etc.).
    ///
    /// # Errors
    ///
    /// Returns the underlying filesystem error if `{data}/tmp` can't be
    /// created or the unique subdirectory can't be allocated.
    pub fn scratch_dir(&self, prefix: &str) -> std::io::Result<zlayer_types::Scratch> {
        std::fs::create_dir_all(self.tmp())?;
        let td = tempfile::Builder::new()
            .prefix(prefix)
            .tempdir_in(self.tmp())?;
        Ok(zlayer_types::Scratch::from_tempdir(td))
    }

    /// Create a uniquely-named scratch file under `{data}/tmp`.
    ///
    /// Returns a [`zlayer_types::ScratchFile`] RAII guard. Same rationale
    /// as [`Self::scratch_dir`].
    ///
    /// # Errors
    ///
    /// Returns the underlying filesystem error if `{data}/tmp` can't be
    /// created or the unique file can't be allocated.
    pub fn scratch_file(&self, prefix: &str) -> std::io::Result<zlayer_types::ScratchFile> {
        std::fs::create_dir_all(self.tmp())?;
        let nf = tempfile::Builder::new()
            .prefix(prefix)
            .tempfile_in(self.tmp())?;
        Ok(zlayer_types::ScratchFile::from_named(nf))
    }

    /// Data-dir-aware WireGuard UAPI socket directory.
    ///
    /// When `data_dir == Self::default_data_dir()`, returns
    /// `/var/run/wireguard` (FHS default — also where `wg(8)` looks).
    /// Otherwise returns `{data_dir}/run/wireguard` so an isolated
    /// install (e.g. `--data-dir /tmp/foo`) does not collide with a
    /// system install on the same host.
    ///
    /// macOS / Windows: always returns `{data_dir}/run/wireguard`
    /// since the FHS path doesn't apply.
    pub fn wireguard(&self) -> PathBuf {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        let natural = self.data_dir.join("run").join("wireguard");
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        let natural = if self.data_dir == Self::default_data_dir() {
            PathBuf::from("/var/run/wireguard")
        } else {
            self.data_dir.join("run").join("wireguard")
        };
        // 1 byte separator + IFNAMSIZ (15) + ".sock" (5) = 21
        const WG_DIR_MAX: usize = SUN_PATH_MAX - 21;
        if natural.to_string_lossy().len() <= WG_DIR_MAX {
            natural
        } else {
            PathBuf::from(format!(
                "/tmp/zlayer-wg-{:016x}",
                hash_for_socket(&self.data_dir, "wg")
            ))
        }
    }
}

/// Convenience: `ZLayerDirs::system_default().admin_bearer_path()`.
#[must_use]
pub fn default_admin_bearer_path() -> PathBuf {
    ZLayerDirs::system_default().admin_bearer_path()
}

// -- Internal helpers --------------------------------------------------------

/// Platform-default data directory, ignoring `$ZLAYER_DATA_DIR`.
///
/// Mirrors [`ZLayerDirs::default_data_dir`] but skips the env-var override so
/// `default_run_dir_for` / `default_log_dir_for` / `default_socket_path_for`
/// can compare a caller-supplied `data_dir` against the platform-hardcoded
/// default rather than against whatever the env var currently resolves to
/// (which would make both sides of the comparison identical when the env var
/// is set, falsely triggering the FHS branch).
pub(crate) fn platform_default_data_dir() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        home_dir_or_fallback().join(".zlayer")
    }
    #[cfg(target_os = "windows")]
    {
        windows_program_data_root()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        if is_root() {
            PathBuf::from("/var/lib/zlayer")
        } else {
            home_dir_or_fallback().join(".zlayer")
        }
    }
}

/// Max usable bytes in `sockaddr_un.sun_path` across our supported Unix
/// targets. Linux's `sun_path` is `char[108]`; macOS's is `char[104]`.
/// Pick the lower bound and reserve one byte for the trailing NUL so the
/// check is trivial on both platforms.
const SUN_PATH_MAX: usize = 103;

/// FNV-1a hash of `data_dir`'s bytes plus a label. Dependency-free and
/// deterministic across processes (unlike `DefaultHasher` which is
/// keyed per-process). The daemon and any CLI client passing the same
/// `data_dir` resolve to byte-identical paths.
fn hash_for_socket(data_dir: &Path, label: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in data_dir.to_string_lossy().as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    for b in label.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Short, deterministic UDS path for callers whose natural
/// `{data_dir}/run/...` path would overflow `sun_path`.
///
/// **Scope guardrail:** `/tmp` is acceptable here ONLY because a UDS
/// endpoint file is just an inode (kernel metadata, no payload). Daemon
/// state — databases, logs, blob caches, image rootfs, anything the
/// daemon reads back later — must NEVER use `/tmp`, even as a fallback.
/// Most modern Linux distros mount `/tmp` as tmpfs (RAM-backed) and
/// silently landing state there courts OOM under load. Sockets are the
/// narrow exception because the file itself stores ~256 bytes of inode
/// metadata.
fn socket_safe_fallback(data_dir: &Path, label: &str) -> String {
    format!(
        "/tmp/zlayer-{label}-{:016x}.sock",
        hash_for_socket(data_dir, label)
    )
}

#[cfg(not(target_os = "windows"))]
fn home_dir_or_fallback() -> PathBuf {
    // Falls back to the FHS system data dir rather than /tmp because /tmp is
    // tmpfs (RAM-backed) on most modern Linux distros and silently landing
    // daemon state there courts OOM.
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/var/lib/zlayer"))
}

/// Resolve the Windows system-wide ZLayer data root.
///
/// Uses `%ProgramData%` (typically `C:\ProgramData`) when present, falling
/// back to the literal `C:\ProgramData\ZLayer` path when the env var is
/// missing (as can happen under a stripped-down service account).
#[cfg(target_os = "windows")]
fn windows_program_data_root() -> PathBuf {
    if let Some(program_data) = std::env::var_os("PROGRAMDATA") {
        let mut p = PathBuf::from(program_data);
        p.push("ZLayer");
        p
    } else {
        PathBuf::from(r"C:\ProgramData\ZLayer")
    }
}

/// Returns `true` when the current process is running with superuser /
/// Administrator privileges.
///
/// - Unix: true when the effective UID is `0`.
/// - Windows: true when the current process token is a member of the
///   built-in Administrators group (checked via `IsUserAnAdmin`).
/// - Other targets: always returns `false`.
#[cfg(unix)]
#[must_use]
pub fn is_root() -> bool {
    // SAFETY: `geteuid` is always safe to call and is thread-safe.
    unsafe { libc::geteuid() == 0 }
}

/// Returns `true` when the current process is running with superuser /
/// Administrator privileges.
#[cfg(windows)]
#[must_use]
pub fn is_root() -> bool {
    use windows::Win32::UI::Shell::IsUserAnAdmin;
    // SAFETY: `IsUserAnAdmin` has no preconditions and returns a BOOL.
    unsafe { IsUserAnAdmin().as_bool() }
}

/// Fallback for non-unix, non-windows targets.
#[cfg(not(any(unix, windows)))]
#[must_use]
pub fn is_root() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    // Tests below mutate environment variables (`PROGRAMDATA` on Windows,
    // `ZLAYER_DATA_DIR` on Linux/macOS) to exercise platform-default and
    // env-override path resolution. Cargo runs tests concurrently, so
    // readers (`system_default`, `default_admin_bearer_path`, the
    // `default_*_for` helpers) must serialize against the mutators or
    // they race and observe a mix of pre- and post-mutation env state.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn subdirectories_are_relative_to_data_dir() {
        let dirs = ZLayerDirs::new("/test/data");
        assert_eq!(dirs.containers(), PathBuf::from("/test/data/containers"));
        assert_eq!(dirs.rootfs(), PathBuf::from("/test/data/rootfs"));
        assert_eq!(dirs.bundles(), PathBuf::from("/test/data/bundles"));
        assert_eq!(dirs.cache(), PathBuf::from("/test/data/cache"));
        assert_eq!(dirs.volumes(), PathBuf::from("/test/data/volumes"));
        assert_eq!(dirs.wasm(), PathBuf::from("/test/data/wasm"));
        assert_eq!(
            dirs.wasm_compiled(),
            PathBuf::from("/test/data/wasm/compiled")
        );
        assert_eq!(dirs.secrets(), PathBuf::from("/test/data/secrets"));
        assert_eq!(dirs.certs(), PathBuf::from("/test/data/certs"));
        assert_eq!(dirs.raft(), PathBuf::from("/test/data/raft"));
        assert_eq!(
            dirs.admin_password(),
            PathBuf::from("/test/data/admin_password")
        );
        assert_eq!(dirs.daemon_json(), PathBuf::from("/test/data/daemon.json"));
        assert_eq!(dirs.logs(), PathBuf::from("/test/data/logs"));
        assert_eq!(dirs.vms(), PathBuf::from("/test/data/vms"));
        assert_eq!(dirs.images(), PathBuf::from("/test/data/images"));
        assert_eq!(dirs.bin(), PathBuf::from("/test/data/bin"));
        assert_eq!(
            dirs.toolchain_cache(),
            PathBuf::from("/test/data/toolchain-cache")
        );
        assert_eq!(dirs.tmp(), PathBuf::from("/test/data/tmp"));
    }

    #[test]
    fn system_default_uses_default_data_dir() {
        let _env_guard = ENV_LOCK.lock().unwrap();
        let dirs = ZLayerDirs::system_default();
        assert_eq!(dirs.data_dir(), ZLayerDirs::default_data_dir().as_path());
    }

    #[test]
    fn admin_bearer_path_is_under_data_dir() {
        let dirs = ZLayerDirs::new(PathBuf::from("/var/lib/zlayer-test"));
        assert_eq!(
            dirs.admin_bearer_path(),
            PathBuf::from("/var/lib/zlayer-test/admin_bearer.token")
        );
    }

    #[test]
    fn default_admin_bearer_path_matches_system_default() {
        let _env_guard = ENV_LOCK.lock().unwrap();
        assert_eq!(
            default_admin_bearer_path(),
            ZLayerDirs::system_default().admin_bearer_path()
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_default_data_dir_uses_program_data() {
        let _env_guard = ENV_LOCK.lock().unwrap();
        let prev = std::env::var_os("PROGRAMDATA");
        std::env::set_var("PROGRAMDATA", r"C:\TestProgramData");

        let data = ZLayerDirs::default_data_dir();
        assert_eq!(data, PathBuf::from(r"C:\TestProgramData\ZLayer"));

        // Sub-paths should live under the ProgramData root.
        let dirs = ZLayerDirs::system_default();
        assert_eq!(dirs.certs(), data.join("certs"));
        assert_eq!(dirs.secrets(), data.join("secrets"));
        assert_eq!(dirs.logs(), data.join("logs"));

        // Run/log helpers should also honour the ProgramData root.
        assert_eq!(ZLayerDirs::default_run_dir(), data.join("run"));
        assert_eq!(ZLayerDirs::default_log_dir(), data.join("logs"));

        // Socket path on Windows is a TCP loopback endpoint, not a filesystem
        // path.
        assert_eq!(ZLayerDirs::default_socket_path(), "tcp://127.0.0.1:3669");

        match prev {
            Some(v) => std::env::set_var("PROGRAMDATA", v),
            None => std::env::remove_var("PROGRAMDATA"),
        }
    }

    #[test]
    fn default_log_dir_for_returns_system_path_when_data_dir_is_default() {
        let _env_guard = ENV_LOCK.lock().unwrap();
        let system_default = ZLayerDirs::default_data_dir();
        let result = ZLayerDirs::default_log_dir_for(&system_default);
        assert_eq!(result, ZLayerDirs::default_log_dir());
    }

    #[test]
    fn default_log_dir_for_returns_data_subdir_when_data_dir_overridden() {
        let _env_guard = ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().expect("create tempdir");
        let custom = tmp.path().to_path_buf();
        let result = ZLayerDirs::default_log_dir_for(&custom);
        assert_eq!(result, custom.join("logs"));
        // Sanity: must NOT be the system default path on Linux/macOS/Windows.
        assert_ne!(result, ZLayerDirs::default_log_dir());
    }

    #[test]
    fn default_run_dir_for_returns_system_path_when_data_dir_is_default() {
        let _env_guard = ENV_LOCK.lock().unwrap();
        let system_default = ZLayerDirs::default_data_dir();
        let result = ZLayerDirs::default_run_dir_for(&system_default);
        assert_eq!(result, ZLayerDirs::default_run_dir());
    }

    #[test]
    fn default_run_dir_for_returns_data_subdir_when_data_dir_overridden() {
        let _env_guard = ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().expect("create tempdir");
        let custom = tmp.path().to_path_buf();
        let result = ZLayerDirs::default_run_dir_for(&custom);
        assert_eq!(result, custom.join("run"));
        assert_ne!(result, ZLayerDirs::default_run_dir());
    }

    #[test]
    fn default_socket_path_for_returns_system_path_when_data_dir_is_default() {
        let _env_guard = ENV_LOCK.lock().unwrap();
        let system_default = ZLayerDirs::default_data_dir();
        let result = ZLayerDirs::default_socket_path_for(&system_default);
        assert_eq!(result, ZLayerDirs::default_socket_path());
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn default_socket_path_for_returns_data_subdir_when_data_dir_overridden() {
        let _env_guard = ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().expect("create tempdir");
        let custom = tmp.path().to_path_buf();
        let result = ZLayerDirs::default_socket_path_for(&custom);
        let expected = custom
            .join("run")
            .join("zlayer.sock")
            .to_string_lossy()
            .into_owned();
        assert_eq!(result, expected);
        assert_ne!(result, ZLayerDirs::default_socket_path());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn default_socket_path_for_always_tcp_on_windows() {
        let _env_guard = ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().expect("create tempdir");
        let custom = tmp.path().to_path_buf();
        // On Windows the daemon listens on TCP loopback regardless of data_dir.
        assert_eq!(
            ZLayerDirs::default_socket_path_for(&custom),
            "tcp://127.0.0.1:3669"
        );
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn default_socket_path_uses_data_subdir_when_env_var_overrides() {
        let _env_guard = ENV_LOCK.lock().unwrap();
        let prev = std::env::var_os("ZLAYER_DATA_DIR");
        let tmp = tempfile::tempdir().expect("create tempdir");
        let custom = tmp.path().to_path_buf();
        std::env::set_var("ZLAYER_DATA_DIR", &custom);

        let result = ZLayerDirs::default_socket_path();
        let expected = custom
            .join("run")
            .join("zlayer.sock")
            .to_string_lossy()
            .into_owned();
        assert_eq!(result, expected);

        match prev {
            Some(v) => std::env::set_var("ZLAYER_DATA_DIR", v),
            None => std::env::remove_var("ZLAYER_DATA_DIR"),
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn default_socket_path_uses_data_subdir_when_env_var_overrides() {
        let _env_guard = ENV_LOCK.lock().unwrap();
        // On Windows the socket is always TCP loopback regardless of
        // `ZLAYER_DATA_DIR`.
        let prev = std::env::var_os("ZLAYER_DATA_DIR");
        let tmp = tempfile::tempdir().expect("create tempdir");
        let custom = tmp.path().to_path_buf();
        std::env::set_var("ZLAYER_DATA_DIR", &custom);

        assert_eq!(ZLayerDirs::default_socket_path(), "tcp://127.0.0.1:3669");

        match prev {
            Some(v) => std::env::set_var("ZLAYER_DATA_DIR", v),
            None => std::env::remove_var("ZLAYER_DATA_DIR"),
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_default_data_dir_fallback_when_env_missing() {
        let _env_guard = ENV_LOCK.lock().unwrap();
        let prev = std::env::var_os("PROGRAMDATA");
        std::env::remove_var("PROGRAMDATA");

        let data = ZLayerDirs::default_data_dir();
        assert_eq!(data, PathBuf::from(r"C:\ProgramData\ZLayer"));

        if let Some(v) = prev {
            std::env::set_var("PROGRAMDATA", v);
        }
    }

    #[test]
    fn default_docker_socket_path_not_empty() {
        let result = ZLayerDirs::default_docker_socket_path();
        assert!(!result.is_empty());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn default_docker_socket_path_platform_shape() {
        let result = ZLayerDirs::default_docker_socket_path();
        assert!(result.starts_with(r"\\.\pipe"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn default_docker_socket_path_platform_shape() {
        let result = ZLayerDirs::default_docker_socket_path();
        assert!(result.ends_with("/docker.sock"));
    }

    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    #[test]
    fn default_docker_socket_path_platform_shape() {
        let result = ZLayerDirs::default_docker_socket_path();
        assert!(result.ends_with("/docker.sock"));
    }

    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    #[test]
    fn wireguard_returns_fhs_path_on_default_data_dir() {
        let dirs = ZLayerDirs::system_default();
        assert_eq!(dirs.wireguard(), PathBuf::from("/var/run/wireguard"));
    }

    #[test]
    fn wireguard_returns_data_subdir_when_overridden() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let custom = tmp.path().to_path_buf();
        let dirs = ZLayerDirs::new(&custom);
        let result = dirs.wireguard();
        assert_eq!(result, custom.join("run").join("wireguard"));
        // Sanity: must NOT be the FHS path on Linux when the data dir is custom.
        #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
        assert_ne!(result, PathBuf::from("/var/run/wireguard"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn wireguard_always_returns_data_subdir_on_macos() {
        // On macOS there is no FHS convention; even the system-default data
        // dir gets `{data_dir}/run/wireguard`.
        let dirs = ZLayerDirs::system_default();
        let expected = ZLayerDirs::default_data_dir().join("run").join("wireguard");
        assert_eq!(dirs.wireguard(), expected);
    }

    #[test]
    fn scratch_dir_under_data_tmp() {
        let parent = tempfile::tempdir().expect("parent");
        let dirs = ZLayerDirs::new(parent.path());
        let s = dirs.scratch_dir("zlayer-test-").expect("scratch_dir");
        assert!(s.path().starts_with(dirs.tmp()));
        assert!(s.path().is_dir());
        let kept = s.path().to_path_buf();
        drop(s);
        assert!(!kept.exists());
    }

    #[test]
    fn scratch_file_under_data_tmp() {
        let parent = tempfile::tempdir().expect("parent");
        let dirs = ZLayerDirs::new(parent.path());
        let f = dirs.scratch_file("zlayer-test-").expect("scratch_file");
        assert!(f.path().starts_with(dirs.tmp()));
        assert!(f.path().is_file());
        let kept = f.path().to_path_buf();
        drop(f);
        assert!(!kept.exists());
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn default_socket_path_for_falls_back_when_path_too_long() {
        let deep = PathBuf::from(
            "/var/lib/forgejo-runner/workdir/9dbc274201705d7d/hostexecutor/target/zlayer-e2e/cluster_3node/node1/data",
        );
        let result = ZLayerDirs::default_socket_path_for(&deep);
        assert!(
            result.len() <= SUN_PATH_MAX,
            "fallback path overflows sun_path: len={} path={}",
            result.len(),
            result,
        );
        // Determinism.
        assert_eq!(result, ZLayerDirs::default_socket_path_for(&deep));
        // Distinct from another data_dir.
        let other = deep.parent().unwrap().to_path_buf();
        assert_ne!(result, ZLayerDirs::default_socket_path_for(&other));
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn default_socket_path_for_keeps_natural_path_when_short() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let short = tmp.path().to_path_buf();
        assert!(short.to_string_lossy().len() < 80);
        let result = ZLayerDirs::default_socket_path_for(&short);
        assert!(result.ends_with("/run/zlayer.sock"));
        assert!(result.starts_with(&*short.to_string_lossy()));
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    #[test]
    fn wireguard_dir_falls_back_when_path_too_long() {
        let deep = PathBuf::from(
            "/var/lib/forgejo-runner/workdir/9dbc274201705d7d/hostexecutor/target/zlayer-e2e/cluster_3node/node1/data",
        );
        let dirs = ZLayerDirs::new(&deep);
        let wg = dirs.wireguard();
        // 21 = "/" + IFNAMSIZ(15) + ".sock"(5)
        assert!(
            wg.to_string_lossy().len() + 21 <= SUN_PATH_MAX,
            "wireguard dir + ifname overflows: dir={}",
            wg.display(),
        );
    }
}
