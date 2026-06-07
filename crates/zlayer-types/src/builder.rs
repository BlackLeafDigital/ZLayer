//! Shared types for build-backend dispatch and sidecar wire protocol.
//!
//! The `BuilderBackendKind` discriminator selects between the in-process buildah
//! CLI shellout, the buildah-sidecar gRPC client, the macOS sandbox builder,
//! and the Windows HCS builder. Other crates import this discriminator rather
//! than redefining their own enum, per the workspace's types-first rule.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Selects which build backend handles a given build.
///
/// `BuildahCli` is the legacy in-process shellout to the `buildah` binary.
/// `BuildahSidecar` talks to a Go sidecar (`zlayer-buildd`) over gRPC+mTLS.
/// `Sandbox` is the macOS-native sandbox-exec builder.
/// `Hcs` is the Windows-native HCS builder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BuilderBackendKind {
    BuildahCli,
    BuildahSidecar,
    Sandbox,
    Hcs,
}

impl BuilderBackendKind {
    /// Stable string identifier used in CLI flags, env vars, and logs.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BuildahCli => "buildah-cli",
            Self::BuildahSidecar => "buildah-sidecar",
            Self::Sandbox => "sandbox",
            Self::Hcs => "hcs",
        }
    }
}

impl fmt::Display for BuilderBackendKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for BuilderBackendKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "buildah-cli" | "buildah" | "cli" => Ok(Self::BuildahCli),
            "buildah-sidecar" | "sidecar" | "buildd" => Ok(Self::BuildahSidecar),
            "sandbox" | "macos-sandbox" => Ok(Self::Sandbox),
            "hcs" | "windows-hcs" => Ok(Self::Hcs),
            other => Err(format!(
                "unknown builder backend kind: {other:?} (expected one of: \
                 buildah-cli, buildah-sidecar, sandbox, hcs)"
            )),
        }
    }
}

/// Transport configuration for connecting to a `zlayer-buildd` sidecar.
///
/// Default operation: the daemon spawns its own local sidecar bound to
/// `127.0.0.1:<auto-port>` with mTLS material under
/// `${ZLAYER_DATA_DIR}/buildd/`. A remote builder (LAN build host) is selected
/// by setting `addr` to a `host:port` reachable by the caller.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SidecarConfig {
    /// `host:port` to dial. `None` means "spawn a local sidecar and use the
    /// auto-allocated 127.0.0.1 port."
    #[serde(skip_serializing_if = "Option::is_none")]
    pub addr: Option<String>,

    /// Directory containing the mTLS material (`ca.pem`, `cert.pem`, `key.pem`).
    /// `None` means use the default under `${ZLAYER_DATA_DIR}/buildd/`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tls_dir: Option<std::path::PathBuf>,

    /// Idle-shutdown timeout in seconds for an auto-spawned local sidecar.
    /// Ignored when `addr` points at a remote sidecar (lifecycle is the
    /// remote operator's responsibility).
    #[serde(default = "SidecarConfig::default_idle_secs")]
    pub idle_secs: u64,

    /// Storage `GraphRoot` to pass to the sidecar. `None` → derive
    /// `${ZLAYER_DATA_DIR}/buildd/storage/graph`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage_graph_root: Option<std::path::PathBuf>,

    /// Storage `RunRoot`. `None` → `${ZLAYER_DATA_DIR}/buildd/storage/run`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage_run_root: Option<std::path::PathBuf>,

    /// Storage driver name. `None` → `vfs` for rootless safety. Operators
    /// running as root may switch to `overlay` for performance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage_driver: Option<String>,

    /// Build-context mount translation for a remote (cross-host) sidecar.
    ///
    /// When the sidecar runs in a different mount namespace than the client
    /// (e.g. a `zlayer-buildd` inside a VZ-Linux container on a macOS host),
    /// the build context must be shared in via a bind / virtiofs mount and
    /// the `context_dir` sent on the wire must be the path *the sidecar*
    /// sees, not the host path the client computed.
    ///
    /// `Some((host_prefix, guest_prefix))` rewrites any `context_dir` that
    /// starts with `host_prefix` so the prefix becomes `guest_prefix`. The
    /// dockerfile paths are translated the same way. `None` (the default,
    /// same-host sidecar) passes paths through unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_mount: Option<(std::path::PathBuf, std::path::PathBuf)>,
}

impl SidecarConfig {
    pub const DEFAULT_IDLE_SECS: u64 = 30;

    #[must_use]
    pub fn default_idle_secs() -> u64 {
        Self::DEFAULT_IDLE_SECS
    }
}

impl Default for SidecarConfig {
    fn default() -> Self {
        Self {
            addr: None,
            tls_dir: None,
            idle_secs: Self::DEFAULT_IDLE_SECS,
            storage_graph_root: None,
            storage_run_root: None,
            storage_driver: None,
            context_mount: None,
        }
    }
}

/// Wire-shaped request for the sidecar's `Build` RPC.
///
/// This mirrors the proto schema 1:1 so the Rust client can deserialize
/// without an intermediate translation type. Fields that are `Option` in
/// the proto become `Option` here; repeated fields become `Vec`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[allow(clippy::struct_excessive_bools)]
pub struct BuildSidecarRequest {
    pub context_dir: String,
    pub dockerfile_paths: Vec<String>,
    pub tags: Vec<String>,
    pub platforms: Vec<String>,
    pub build_args: std::collections::BTreeMap<String, String>,
    pub secrets: Vec<String>,
    pub ssh: Vec<String>,
    pub target_stage: Option<String>,
    pub host_network: bool,
    pub cache_from: Option<String>,
    pub cache_to: Option<String>,
    pub no_cache: bool,
    pub squash: bool,
    pub layers: bool,
    pub format: Option<String>,
    pub pull_policy: Option<String>,
}

/// Streamed event from the sidecar during a build.
///
/// Variants mirror the proto `BuildEvent.oneof event` arms and translate to
/// the zlayer-builder `BuildEvent` enum at the client boundary.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BuildSidecarEvent {
    StageStarted {
        index: u32,
        name: Option<String>,
        base_image: String,
    },
    StageFinished {
        index: u32,
    },
    InstructionStarted {
        stage: u32,
        index: u32,
        instruction: String,
    },
    InstructionFinished {
        stage: u32,
        index: u32,
        cached: bool,
    },
    Log {
        line: String,
        is_stderr: bool,
    },
    Warning {
        message: String,
    },
    Finished {
        image_id: String,
        manifest_ref: Option<String>,
    },
    Error {
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_kind_round_trips_through_as_str() {
        for kind in [
            BuilderBackendKind::BuildahCli,
            BuilderBackendKind::BuildahSidecar,
            BuilderBackendKind::Sandbox,
            BuilderBackendKind::Hcs,
        ] {
            assert_eq!(BuilderBackendKind::from_str(kind.as_str()).unwrap(), kind);
        }
    }

    #[test]
    fn backend_kind_accepts_aliases() {
        assert_eq!(
            BuilderBackendKind::from_str("buildah").unwrap(),
            BuilderBackendKind::BuildahCli
        );
        assert_eq!(
            BuilderBackendKind::from_str("sidecar").unwrap(),
            BuilderBackendKind::BuildahSidecar
        );
        assert_eq!(
            BuilderBackendKind::from_str("BUILDAH-SIDECAR").unwrap(),
            BuilderBackendKind::BuildahSidecar
        );
    }

    #[test]
    fn sidecar_config_idle_default() {
        let cfg = SidecarConfig::default();
        assert_eq!(cfg.idle_secs, 30);
        assert!(cfg.addr.is_none());
        assert!(cfg.tls_dir.is_none());
        assert!(cfg.storage_graph_root.is_none());
        assert!(cfg.storage_run_root.is_none());
        assert!(cfg.storage_driver.is_none());
    }
}
