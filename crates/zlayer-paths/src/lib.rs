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
    /// - macOS: `~/.zlayer`
    /// - Linux (root): `/var/lib/zlayer`
    /// - Linux (user): `~/.zlayer`
    /// - Windows: `%LOCALAPPDATA%\ZLayer` or `C:\ProgramData\ZLayer`
    pub fn default_data_dir() -> PathBuf {
        #[cfg(target_os = "macos")]
        {
            home_dir_or_tmp().join(".zlayer")
        }
        #[cfg(target_os = "windows")]
        {
            if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
                PathBuf::from(local_app_data).join("ZLayer")
            } else {
                PathBuf::from(r"C:\ProgramData\ZLayer")
            }
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            if is_root() {
                PathBuf::from("/var/lib/zlayer")
            } else {
                home_dir_or_tmp().join(".zlayer")
            }
        }
    }

    /// Default runtime directory.
    ///
    /// - Linux: `/var/run/zlayer`
    /// - macOS: `{default_data_dir}/run`
    /// - Windows: `{default_data_dir}\run`
    pub fn default_run_dir() -> PathBuf {
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            PathBuf::from("/var/run/zlayer")
        }
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            Self::default_data_dir().join("run")
        }
    }

    /// Default log directory.
    ///
    /// - Linux: `/var/log/zlayer`
    /// - macOS: `{default_data_dir}/logs`
    /// - Windows: `{default_data_dir}\logs`
    pub fn default_log_dir() -> PathBuf {
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            PathBuf::from("/var/log/zlayer")
        }
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            Self::default_data_dir().join("logs")
        }
    }

    /// Default Unix socket path.
    ///
    /// - Linux: `/var/run/zlayer.sock`
    /// - macOS: `{default_data_dir}/run/zlayer.sock`
    /// - Windows: `tcp://127.0.0.1:3669`
    pub fn default_socket_path() -> String {
        #[cfg(target_os = "windows")]
        {
            "tcp://127.0.0.1:3669".to_string()
        }
        #[cfg(not(target_os = "windows"))]
        {
            #[cfg(target_os = "macos")]
            {
                Self::default_data_dir()
                    .join("run")
                    .join("zlayer.sock")
                    .to_string_lossy()
                    .into_owned()
            }
            #[cfg(not(target_os = "macos"))]
            {
                "/var/run/zlayer.sock".to_string()
            }
        }
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

    /// Daemon metadata file path (`{data}/daemon.json`).
    pub fn daemon_json(&self) -> PathBuf {
        self.data_dir.join("daemon.json")
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
}

// -- Internal helpers --------------------------------------------------------

fn home_dir_or_tmp() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn is_root() -> bool {
    #[cfg(unix)]
    {
        nix::unistd::geteuid().is_root()
    }
    #[cfg(not(unix))]
    {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let dirs = ZLayerDirs::system_default();
        assert_eq!(dirs.data_dir(), ZLayerDirs::default_data_dir().as_path());
    }
}
