//! WSL2 delegate runtime that executes Linux containers inside a configurable
//! WSL2 distro via shell-out to `youki`.
//!
//! Used by [`super::composite::CompositeRuntime`] on Windows hosts to handle
//! Linux-image services alongside HCS-managed Windows-image services. There is
//! **no in-distro daemon**, **no HTTP server**, and **no new crate**: every
//! [`Runtime`] trait method maps to one or more `wsl.exe -d <distro> -- ...`
//! invocations, where `<distro>` and the `youki` binary path are both
//! resolved from [`Wsl2DelegateConfig`] at construction time rather than
//! hardcoded.
//!
//! # Scope
//!
//! Phase F-2 wired the dispatch path. Phase G-2 wired a real Windows-side
//! OCI bundle-write path. Phase G-3/G-4/G-5 adds:
//!
//! * **Real exec streaming.** [`Runtime::exec_stream`] now spawns `wsl.exe`
//!   with piped stdout/stderr and emits [`ExecEvent::Stdout`] /
//!   [`ExecEvent::Stderr`] line-by-line via `BufReader::lines()`, followed
//!   by a terminal [`ExecEvent::Exit`] once the child reaps.
//! * **Real youki log path.** `youki create` is invoked with
//!   `--log <log_root>/<slug>.youki.log`, and [`Runtime::container_logs`]
//!   tails the same path — so logs actually make it out of the distro
//!   instead of landing in a fabricated file youki never writes to.
//! * **Config-driven distro + youki path.** [`Wsl2DelegateConfig`] carries
//!   `distro`, `youki_path` (optional — resolved via `which youki` when
//!   unset), `bundle_root`, and `log_root`. [`Wsl2DelegateRuntime::try_new`]
//!   preserves the old `Ok(None)` "no WSL" contract; the explicit
//!   [`Wsl2DelegateRuntime::try_new_with_config`] surface returns hard
//!   errors for misconfigurations so operators catch typos early.
//!
//! Phase J-3 adds per-container Linux network namespaces inside the distro:
//! [`Wsl2DelegateRuntime::setup_container_netns`] runs before `youki start`
//! to program a veth pair + overlay IP + default route, and
//! [`Wsl2DelegateRuntime::teardown_container_netns`] cleans it up on
//! `remove_container`. Failures degrade to host networking and
//! [`Runtime::get_container_ip`] hides the recorded IP to match reality.
//!
//! # Error mapping
//!
//! Every shell-out failure maps to [`AgentError::Network`] with a message of
//! the form `youki <subcommand> failed (status <code>): <stderr>` so the user
//! sees both the command that was run and the distro's stderr. Configuration
//! errors (missing youki, bad path override) surface as
//! [`AgentError::Configuration`] from `try_new_with_config` instead.

#![cfg(all(target_os = "windows", feature = "wsl"))]

use std::collections::HashMap;
use std::io::Read;
use std::net::IpAddr;
use std::path::PathBuf;
use std::process::{Output, Stdio};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;
use tokio::sync::RwLock;
use tokio_stream::wrappers::ReceiverStream;
use zlayer_observability::logs::{LogEntry, LogSource, LogStream};
use zlayer_registry::{CompressionType, ImageConfig};
use zlayer_spec::{PullPolicy, RegistryAuth, ServiceSpec};

use crate::bundle::BundleBuilder;
use crate::cgroups_stats::ContainerStats;
use crate::error::{AgentError, Result};
use crate::overlay_manager::make_interface_name;
use crate::runtime::{
    validate_signal, ContainerId, ContainerInspectDetails, ContainerState, ExecEvent,
    ExecEventStream, ImageInfo, PruneResult, Runtime,
};

/// Default in-distro path under which per-container OCI bundles are rooted.
const DEFAULT_BUNDLE_ROOT: &str = "/var/lib/zlayer/bundles";

/// Default in-distro directory where per-container youki log files are written.
/// `create_container` passes `--log <dir>/<slug>.youki.log` to `youki create`,
/// and `container_logs` reads back from the same file.
const DEFAULT_LOG_ROOT: &str = "/var/lib/zlayer/logs";

/// Maximum time we'll poll [`Runtime::wait_container`] before giving up. Youki
/// exposes no blocking `wait` subcommand today, so we poll `state` and bail
/// after a day's worth of polling — plenty for batch jobs, cheap to rerun.
const WAIT_POLL_CAP: Duration = Duration::from_secs(24 * 60 * 60);

/// Polling interval used by [`Runtime::wait_container`] and
/// [`Runtime::stop_container`] while waiting for a container to stop.
const WAIT_POLL_INTERVAL: Duration = Duration::from_millis(500);

/// Configuration for [`Wsl2DelegateRuntime`].
///
/// Lets callers override the distro name and the in-distro `youki` binary
/// location so this delegate isn't locked to the hardcoded `zlayer` distro or
/// the `$PATH`-resolved `youki` binary. `try_new` validates each field by
/// shelling out to the distro before returning a runtime, so a misconfigured
/// `youki_path` or a missing distro fails fast with an actionable error.
#[derive(Clone, Debug)]
pub struct Wsl2DelegateConfig {
    /// Name of the WSL2 distro to dispatch container operations into.
    /// Defaults to [`zlayer_wsl::distro::DISTRO_NAME`] (`"zlayer"`).
    pub distro: String,
    /// Absolute in-distro path to the `youki` binary. When `None`, `try_new`
    /// runs `wsl.exe -d <distro> -- which youki` to resolve a value and uses
    /// `youki` (bare name, relying on `$PATH`) if resolution fails — so both
    /// "installed on `$PATH`" and "installed at a custom prefix" layouts are
    /// supported.
    pub youki_path: Option<String>,
    /// In-distro directory where per-container OCI bundles are materialized
    /// under `<bundle_root>/<container-slug>/`. Defaults to
    /// [`DEFAULT_BUNDLE_ROOT`] (`/var/lib/zlayer/bundles`).
    pub bundle_root: String,
    /// In-distro directory where youki writes its per-container log files;
    /// each container gets `<log_root>/<slug>.youki.log`. Defaults to
    /// [`DEFAULT_LOG_ROOT`] (`/var/lib/zlayer/logs`).
    pub log_root: String,
}

impl Default for Wsl2DelegateConfig {
    fn default() -> Self {
        Self {
            distro: zlayer_wsl::distro::DISTRO_NAME.to_string(),
            youki_path: None,
            bundle_root: DEFAULT_BUNDLE_ROOT.to_string(),
            log_root: DEFAULT_LOG_ROOT.to_string(),
        }
    }
}

/// Cached per-image data gathered on the Windows host and reused by
/// [`Wsl2DelegateRuntime::create_container`] to populate the rootfs in the
/// WSL2 distro.
#[derive(Clone, Debug)]
struct CachedImage {
    /// Raw (still-compressed) layer blobs plus their OCI media types, in
    /// application order (base first). We keep them compressed here and
    /// decompress just-in-time on the tar-streaming path so re-pulls stay
    /// free and RAM is only held for the duration of `create_container`.
    layers: Vec<(Vec<u8>, String)>,
    /// Image configuration (entrypoint/cmd/env/workdir/user) so we can feed
    /// it to [`BundleBuilder::with_image_config`] when rendering `config.json`.
    config: ImageConfig,
}

/// Per-container netns lifecycle state, tracked so `start_container` /
/// `remove_container` know whether to tear the netns down and so
/// [`Runtime::get_container_ip`] can hide the recorded IP on fallback.
///
/// Populated by [`Wsl2DelegateRuntime::setup_container_netns`] at start time
/// and consulted by [`Wsl2DelegateRuntime::teardown_container_netns`] at
/// remove time. Entries only exist for containers that had an overlay IP
/// recorded; containers attached in host-network mode never appear here.
#[derive(Debug, Clone)]
enum NetnsState {
    /// netns + veth + IP + default route were all programmed successfully.
    /// `host_iface` is the host-end veth name (kept so teardown can delete
    /// the peer even after the netns is gone). `ip` is the overlay IP
    /// that was stamped on the container end — kept for tracing so logs
    /// in `teardown_container_netns` and debugging introspection can show
    /// what the workload actually bound to.
    Configured {
        host_iface: String,
        #[allow(dead_code)]
        ip: IpAddr,
    },
    /// Setup failed. The container is running but with host networking;
    /// `get_container_ip` will return `None` to reflect the actual reality
    /// the workload sees.
    HostFallback,
}

/// Hook for injecting a custom `wsl.exe` runner in unit tests. Production
/// uses [`zlayer_wsl::distro::wsl_exec`] via the default trait method.
///
/// Kept as a trait object behind `Arc` so [`Wsl2DelegateRuntime`] stays
/// `Send + Sync + 'static`. All method bodies in tests push the `(cmd, args)`
/// pair into a log and return a canned `Output`.
#[async_trait]
pub trait WslRunner: Send + Sync + 'static {
    /// Run `cmd args...` inside the WSL2 helper distro and return the
    /// buffered `Output`. Implementations that fail to invoke `wsl.exe`
    /// return an error — the caller translates to [`AgentError::Network`].
    async fn run(&self, cmd: &str, args: &[&str]) -> anyhow::Result<Output>;
}

/// Default runner: hands off directly to [`zlayer_wsl::distro::wsl_exec`].
struct DefaultWslRunner;

#[async_trait]
impl WslRunner for DefaultWslRunner {
    async fn run(&self, cmd: &str, args: &[&str]) -> anyhow::Result<Output> {
        zlayer_wsl::distro::wsl_exec(cmd, args).await
    }
}

/// `Runtime` implementation that shells out to `youki` inside the `zlayer`
/// WSL2 distro.
///
/// Construct via [`Wsl2DelegateRuntime::try_new`], which returns `Ok(None)`
/// when WSL2 (or the helper distro) is not available — callers should treat
/// that as "no Linux-container support on this node" rather than an error.
pub struct Wsl2DelegateRuntime {
    /// Resolved runtime configuration: distro name, in-distro `youki` binary
    /// path, bundle root, log root. Fully populated by
    /// [`Wsl2DelegateRuntime::try_new`] — in particular `youki_path` is the
    /// final absolute path (resolved via `which youki` when the caller left
    /// it `None`) or the literal string `"youki"` if everything else failed
    /// and we're relying on `$PATH` inside the distro.
    config: ResolvedConfig,
    /// Per-container cache of the Linux PID youki reports after `start`.
    /// Populated lazily; used by [`Runtime::get_container_pid`].
    pids: Arc<RwLock<HashMap<ContainerId, u32>>>,
    /// Per-container cache of the assigned overlay IP, set by an external
    /// caller (the `OverlayManager` / agent's create flow) via
    /// [`Wsl2DelegateRuntime::record_container_ip`]. Returned by
    /// [`Runtime::get_container_ip`] when the netns setup succeeds.
    ips: Arc<RwLock<HashMap<ContainerId, IpAddr>>>,
    /// Per-container record of the in-distro bundle directory that was
    /// materialized by `create_container`. Used by `remove_container` to
    /// clean up after a container goes away, and kept alive so
    /// `start_container` doesn't have to re-derive the path.
    bundle_roots: Arc<RwLock<HashMap<ContainerId, String>>>,
    /// Per-image cache of pulled layer blobs + config, populated by
    /// [`Wsl2DelegateRuntime::pull_image_with_policy`] and consumed by
    /// [`Wsl2DelegateRuntime::create_container`] when it streams the rootfs
    /// into the WSL2 distro. `ServiceSpec::image.name` is the cache key.
    image_cache: Arc<RwLock<HashMap<String, CachedImage>>>,
    /// Optional per-container gateway (the host-end veth IP inside the
    /// distro) supplied by [`Wsl2DelegateRuntime::record_container_gateway`].
    /// When absent, `setup_container_netns` skips the default-route step
    /// and logs a warning.
    gateways: Arc<RwLock<HashMap<ContainerId, IpAddr>>>,
    /// Per-container netns lifecycle state. Populated by
    /// [`Wsl2DelegateRuntime::setup_container_netns`] and cleared by
    /// [`Wsl2DelegateRuntime::teardown_container_netns`].
    netns: Arc<RwLock<HashMap<ContainerId, NetnsState>>>,
    /// Runner used to execute in-distro commands via the J-3 netns setup
    /// path. Swapped in tests for a recording fake; defaults to
    /// [`DefaultWslRunner`]. Note: the non-netns code paths (G-2/G-3/G-4/G-5)
    /// use the [`wsl_exec_in`] free helper directly via `self.wsl()` /
    /// `self.youki()` so they can target the configured distro name; the
    /// runner only covers the netns commands (`ip netns`, `ip link`, …).
    runner: Arc<dyn WslRunner>,
}

/// Internal helper that replaces every `Option` in [`Wsl2DelegateConfig`] with
/// a concrete value. Populated by [`Wsl2DelegateRuntime::try_new`] once every
/// field has been validated against the live distro.
#[derive(Clone, Debug)]
struct ResolvedConfig {
    distro: String,
    youki_path: String,
    bundle_root: String,
    log_root: String,
}

impl std::fmt::Debug for Wsl2DelegateRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Wsl2DelegateRuntime")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl Wsl2DelegateRuntime {
    /// Create a [`Wsl2DelegateRuntime`] if WSL2 is available on this host.
    ///
    /// Returns `Ok(None)` when WSL2 is not installed or the helper distro
    /// cannot be set up — callers should treat this as "no Linux-container
    /// support on this node" rather than an error. The probe is best-effort;
    /// transient WSL failures log a warning and return `Ok(None)` so daemon
    /// boot proceeds with HCS-only.
    ///
    /// # Errors
    ///
    /// Never bubbles an error up to the caller — the whole point of
    /// `try_new` returning `Option` is that absence-of-WSL2 is a normal
    /// state on Windows hosts. The `Result` return type is kept to leave
    /// room for future fatal-config errors (e.g. malformed environment).
    pub async fn try_new() -> Result<Option<Self>> {
        Self::try_new_with_config(Wsl2DelegateConfig::default()).await
    }

    /// Create a [`Wsl2DelegateRuntime`] using a caller-supplied config.
    ///
    /// Same best-effort contract as [`Self::try_new`] — returns `Ok(None)` if
    /// WSL2 or the requested distro is unavailable — but lets the caller pin
    /// a non-default distro name, youki path, bundle root, or log root.
    ///
    /// `youki_path` resolution rules:
    /// 1. If `config.youki_path` is `Some(path)`, verify that `path` exists
    ///    inside the distro (`wsl.exe -d <distro> -- test -x <path>`). On
    ///    success, use it verbatim. On failure, return a clear error.
    /// 2. If `config.youki_path` is `None`, run
    ///    `wsl.exe -d <distro> -- which youki`. On success, use the resolved
    ///    absolute path. On failure, return an actionable error telling the
    ///    user how to install youki or configure a path override.
    ///
    /// # Errors
    ///
    /// Returns `Err(AgentError::Configuration(_))` when `youki_path` is
    /// explicitly set but does not exist / is not executable inside the
    /// distro, or when `youki` is not on `$PATH` inside the distro and no
    /// explicit path was provided — both are misconfigurations that warrant a
    /// hard fail rather than silently disabling Linux support.
    pub async fn try_new_with_config(config: Wsl2DelegateConfig) -> Result<Option<Self>> {
        // 1. Detect WSL2. Any failure of the detect call is treated as
        //    "no WSL" rather than a hard error.
        let status = match zlayer_wsl::detect::detect_wsl().await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "WSL2 detection failed; Linux container support disabled"
                );
                return Ok(None);
            }
        };
        if !status.wsl2_available {
            tracing::info!(
                wsl_installed = status.wsl_installed,
                "WSL2 not available; Linux container support disabled on this node"
            );
            return Ok(None);
        }

        // 2. Bootstrap the helper distro if missing. Any failure here is
        //    also best-effort — we log and return Ok(None). This step only
        //    runs when the caller is using the default distro (the setup
        //    module only knows how to bootstrap the `zlayer` distro); for
        //    custom distros we assume the operator has already provisioned
        //    it out-of-band.
        if config.distro == zlayer_wsl::distro::DISTRO_NAME {
            if let Err(e) = zlayer_wsl::setup::ensure_wsl_backend_ready().await {
                tracing::warn!(
                    error = %e,
                    "failed to bootstrap WSL2 zlayer distro; Linux container support disabled"
                );
                return Ok(None);
            }
        }

        // 3. Resolve the youki path. When the caller supplied one, verify it
        //    exists inside the distro; otherwise run `which youki` and fail
        //    loudly if it's not on `$PATH` — a silent fallback to a bare
        //    `youki` would hide misconfigurations.
        let youki_path = match config.youki_path.as_deref() {
            Some(explicit) => {
                let probe = wsl_exec_in(&config.distro, "test", &["-x", explicit]).await;
                match probe {
                    Ok(out) if out.status.success() => explicit.to_string(),
                    Ok(out) => {
                        return Err(AgentError::Configuration(format!(
                            "configured youki_path '{explicit}' is not executable in WSL2 \
                             distro '{}' (status {:?}): {}",
                            config.distro,
                            out.status.code(),
                            String::from_utf8_lossy(&out.stderr).trim(),
                        )));
                    }
                    Err(e) => {
                        return Err(AgentError::Configuration(format!(
                            "failed to verify configured youki_path '{explicit}' in WSL2 \
                             distro '{}': {e}",
                            config.distro,
                        )));
                    }
                }
            }
            None => match wsl_exec_in(&config.distro, "which", &["youki"]).await {
                Ok(out) if out.status.success() => {
                    let resolved = String::from_utf8_lossy(&out.stdout).trim().to_string();
                    if resolved.is_empty() {
                        return Err(AgentError::Configuration(format!(
                            "youki not found in WSL2 distro '{}'; install it or \
                             configure runtime.wsl2.youki_path",
                            config.distro,
                        )));
                    }
                    resolved
                }
                Ok(out) => {
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    return Err(AgentError::Configuration(format!(
                        "youki not found in WSL2 distro '{}' (status {:?}: {}); \
                         install it or configure runtime.wsl2.youki_path",
                        config.distro,
                        out.status.code(),
                        stderr.trim(),
                    )));
                }
                Err(e) => {
                    return Err(AgentError::Configuration(format!(
                        "probing for youki in WSL2 distro '{}' failed: {e}; \
                         install it or configure runtime.wsl2.youki_path",
                        config.distro,
                    )));
                }
            },
        };

        // 4. Ensure the log directory exists inside the distro so that
        //    `youki create --log <log_root>/<slug>.youki.log` does not fail
        //    on first use. `mkdir -p` is idempotent and cheap.
        if let Err(e) = wsl_exec_in(&config.distro, "mkdir", &["-p", &config.log_root]).await {
            tracing::warn!(
                distro = %config.distro,
                log_root = %config.log_root,
                error = %e,
                "failed to pre-create youki log root; container_logs may be empty until youki creates it"
            );
        }

        Ok(Some(Self {
            config: ResolvedConfig {
                distro: config.distro,
                youki_path,
                bundle_root: config.bundle_root,
                log_root: config.log_root,
            },
            pids: Arc::new(RwLock::new(HashMap::new())),
            ips: Arc::new(RwLock::new(HashMap::new())),
            bundle_roots: Arc::new(RwLock::new(HashMap::new())),
            image_cache: Arc::new(RwLock::new(HashMap::new())),
            gateways: Arc::new(RwLock::new(HashMap::new())),
            netns: Arc::new(RwLock::new(HashMap::new())),
            runner: Arc::new(DefaultWslRunner),
        }))
    }

    /// Record an overlay IP that has been allocated for the given container.
    ///
    /// Called by the agent's overlay-attach flow before `create_container`,
    /// so [`Runtime::get_container_ip`] can later return the right value and
    /// so [`Self::setup_container_netns`] knows which IP to stamp inside the
    /// distro's network namespace before `youki start`.
    pub async fn record_container_ip(&self, id: &ContainerId, ip: IpAddr) {
        self.ips.write().await.insert(id.clone(), ip);
    }

    /// Record the overlay gateway IP (the host-end veth inside the distro)
    /// that the container should use for its default route.
    ///
    /// Called alongside [`Self::record_container_ip`] when the caller knows
    /// both endpoints of the veth pair ahead of start. If the gateway is
    /// unknown at start time, [`Self::setup_container_netns`] logs a warning
    /// and skips the default-route step but still programs the IP, so
    /// overlay-local traffic continues to work.
    pub async fn record_container_gateway(&self, id: &ContainerId, gateway: IpAddr) {
        self.gateways.write().await.insert(id.clone(), gateway);
    }

    /// In-distro directory for the given container's OCI bundle.
    fn bundle_dir(&self, id: &ContainerId) -> String {
        format!("{}/{}", self.config.bundle_root, id_slug(id))
    }

    /// In-distro path where youki writes the given container's log file. Kept
    /// in sync with the `--log` argument passed to `youki create`.
    fn log_path(&self, id: &ContainerId) -> String {
        format!("{}/{}.youki.log", self.config.log_root, id_slug(id))
    }

    /// Run a youki subcommand inside the configured distro. Thin wrapper
    /// around [`wsl_exec_in`] that prepends the resolved youki binary path so
    /// callers don't have to repeat the distro name + binary name in every
    /// method.
    async fn youki(&self, args: &[&str]) -> Result<Output> {
        wsl_exec_in(&self.config.distro, &self.config.youki_path, args).await
    }

    /// Run an arbitrary binary inside the configured distro. Used for the
    /// handful of call sites that need `mkdir`, `rm`, `tail`, etc. rather
    /// than youki itself.
    async fn wsl(&self, cmd: &str, args: &[&str]) -> Result<Output> {
        wsl_exec_in(&self.config.distro, cmd, args).await
    }

    /// Execute `cmd args...` inside the distro via the configured [`WslRunner`].
    ///
    /// Separate from [`Self::wsl`] so unit tests for the J-3 netns plumbing
    /// can swap in a recording runner without having to stub the whole
    /// `wsl.exe` surface. Production code paths (G-2/G-3/G-4/G-5) continue
    /// to go through [`Self::wsl`] / [`Self::youki`] so they honour the
    /// configured distro + youki path.
    async fn wsl_run(&self, cmd: &str, args: &[&str]) -> Result<Output> {
        self.runner.run(cmd, args).await.map_err(|e| {
            AgentError::Network(format!("wsl.exe -d {} -- {cmd}: {e}", self.config.distro))
        })
    }

    /// Program the container's network namespace, veth pair, overlay IP and
    /// default route inside the distro. Called from `start_container` before
    /// `youki start` so the container-init process sees the overlay IP on
    /// its `eth0`-equivalent interface at the very first instruction.
    ///
    /// Only meaningful for containers that had an overlay IP recorded via
    /// [`Self::record_container_ip`]. On any step failure we log a warning,
    /// mark the container as [`NetnsState::HostFallback`], and return `Ok`
    /// — the container can still boot on host networking, which is the
    /// documented fallback for this codepath.
    async fn setup_container_netns(&self, id: &ContainerId) {
        let Some(ip) = self.ips.read().await.get(id).copied() else {
            // No recorded IP -> nothing to program; container gets default
            // (host) networking from youki. Not a failure.
            return;
        };
        let slug = id_slug(id);
        let gateway = self.gateways.read().await.get(id).copied();

        // Generate IFNAMSIZ-safe names. `make_interface_name` guarantees
        // <= 15 chars by hashing long inputs.
        let host_iface = make_interface_name(&[&slug], "h");
        let cont_iface = make_interface_name(&[&slug], "c");
        let final_iface = "eth0".to_string();

        match self
            .setup_container_netns_inner(&slug, ip, gateway, &host_iface, &cont_iface, &final_iface)
            .await
        {
            Ok(()) => {
                self.netns.write().await.insert(
                    id.clone(),
                    NetnsState::Configured {
                        host_iface: host_iface.clone(),
                        ip,
                    },
                );
                tracing::info!(
                    container = %id,
                    %ip,
                    host_iface = %host_iface,
                    "WSL2 container netns configured"
                );
            }
            Err(e) => {
                tracing::warn!(
                    container = %id,
                    %ip,
                    error = %e,
                    "WSL2 container netns setup failed; falling back to host networking \
                     (container will boot but without overlay IP)"
                );
                // Best-effort cleanup of any partial state left behind.
                let _ = self.wsl_run("ip", &["link", "delete", &host_iface]).await;
                let _ = self.wsl_run("ip", &["netns", "delete", &slug]).await;
                self.netns
                    .write()
                    .await
                    .insert(id.clone(), NetnsState::HostFallback);
            }
        }
    }

    /// The fallible body of [`Self::setup_container_netns`]. Factored out so
    /// the outer method can uniformly translate failures into the
    /// host-network fallback.
    ///
    /// Clippy is correct that this is long, but every step is a distinct
    /// `wsl.exe` invocation with its own error message; factoring each
    /// step into its own helper would just push the per-step `match`
    /// arms one level deeper without improving readability.
    #[allow(clippy::too_many_lines)]
    async fn setup_container_netns_inner(
        &self,
        slug: &str,
        ip: IpAddr,
        gateway: Option<IpAddr>,
        host_iface: &str,
        cont_iface: &str,
        final_iface: &str,
    ) -> Result<()> {
        // 1. Create the netns. Idempotent: "File exists" means it already
        //    exists from a prior attempt and is fine to reuse.
        let mk_ns = self.wsl_run("ip", &["netns", "add", slug]).await?;
        if !mk_ns.status.success() {
            let stderr = String::from_utf8_lossy(&mk_ns.stderr);
            if !stderr.to_ascii_lowercase().contains("file exists") {
                return Err(AgentError::Network(format!(
                    "ip netns add {slug} failed (status {:?}): {}",
                    mk_ns.status.code(),
                    stderr.trim()
                )));
            }
        }

        // 2. Create the veth pair in the host netns. If a stale one exists
        //    from a previous crashed run, delete it first so our create is
        //    clean.
        let _ = self.wsl_run("ip", &["link", "delete", host_iface]).await;
        let mk_veth = self
            .wsl_run(
                "ip",
                &[
                    "link", "add", host_iface, "type", "veth", "peer", "name", cont_iface,
                ],
            )
            .await?;
        if !mk_veth.status.success() {
            let stderr = String::from_utf8_lossy(&mk_veth.stderr);
            return Err(AgentError::Network(format!(
                "ip link add veth {host_iface}/{cont_iface} failed (status {:?}): {}",
                mk_veth.status.code(),
                stderr.trim()
            )));
        }

        // 3. Move the container end into the new netns.
        let move_end = self
            .wsl_run("ip", &["link", "set", cont_iface, "netns", slug])
            .await?;
        if !move_end.status.success() {
            let stderr = String::from_utf8_lossy(&move_end.stderr);
            return Err(AgentError::Network(format!(
                "ip link set {cont_iface} netns {slug} failed (status {:?}): {}",
                move_end.status.code(),
                stderr.trim()
            )));
        }

        // 4. Rename the container end inside the netns to the final name
        //    (`eth0`) the workload expects.
        let rename = self
            .wsl_run(
                "ip",
                &[
                    "netns",
                    "exec",
                    slug,
                    "ip",
                    "link",
                    "set",
                    cont_iface,
                    "name",
                    final_iface,
                ],
            )
            .await?;
        if !rename.status.success() {
            let stderr = String::from_utf8_lossy(&rename.stderr);
            return Err(AgentError::Network(format!(
                "ip link rename {cont_iface} -> {final_iface} (in {slug}) failed (status {:?}): {}",
                rename.status.code(),
                stderr.trim()
            )));
        }

        // 5. Assign the overlay IP on the container end and bring it up.
        let cidr = match ip {
            IpAddr::V4(_) => format!("{ip}/24"),
            IpAddr::V6(_) => format!("{ip}/64"),
        };
        let addr = self
            .wsl_run(
                "ip",
                &[
                    "netns",
                    "exec",
                    slug,
                    "ip",
                    "addr",
                    "add",
                    &cidr,
                    "dev",
                    final_iface,
                ],
            )
            .await?;
        if !addr.status.success() {
            let stderr = String::from_utf8_lossy(&addr.stderr);
            return Err(AgentError::Network(format!(
                "ip addr add {cidr} dev {final_iface} (in {slug}) failed (status {:?}): {}",
                addr.status.code(),
                stderr.trim()
            )));
        }

        let up = self
            .wsl_run(
                "ip",
                &[
                    "netns",
                    "exec",
                    slug,
                    "ip",
                    "link",
                    "set",
                    final_iface,
                    "up",
                ],
            )
            .await?;
        if !up.status.success() {
            let stderr = String::from_utf8_lossy(&up.stderr);
            return Err(AgentError::Network(format!(
                "ip link set {final_iface} up (in {slug}) failed (status {:?}): {}",
                up.status.code(),
                stderr.trim()
            )));
        }

        // 5b. Bring up loopback so intra-container localhost traffic works.
        //     Best-effort: don't fail the attach on this.
        let _ = self
            .wsl_run(
                "ip",
                &["netns", "exec", slug, "ip", "link", "set", "lo", "up"],
            )
            .await;

        // 6. Bring up the host end of the veth pair.
        let host_up = self
            .wsl_run("ip", &["link", "set", host_iface, "up"])
            .await?;
        if !host_up.status.success() {
            let stderr = String::from_utf8_lossy(&host_up.stderr);
            return Err(AgentError::Network(format!(
                "ip link set {host_iface} up failed (status {:?}): {}",
                host_up.status.code(),
                stderr.trim()
            )));
        }

        // 7. Add a default route inside the netns if we know the gateway.
        //    Skipping this is a degraded but usable outcome (overlay-local
        //    traffic still works) so we warn rather than fail.
        if let Some(gw) = gateway {
            let route = self
                .wsl_run(
                    "ip",
                    &[
                        "netns",
                        "exec",
                        slug,
                        "ip",
                        "route",
                        "add",
                        "default",
                        "via",
                        &gw.to_string(),
                    ],
                )
                .await?;
            if !route.status.success() {
                let stderr = String::from_utf8_lossy(&route.stderr);
                return Err(AgentError::Network(format!(
                    "ip route add default via {gw} (in {slug}) failed (status {:?}): {}",
                    route.status.code(),
                    stderr.trim()
                )));
            }
        } else {
            tracing::warn!(
                container = %slug,
                "no overlay gateway recorded; skipping default-route setup. \
                 Container will have overlay-local connectivity only."
            );
        }

        Ok(())
    }

    /// Tear down the per-container netns + veth pair.
    ///
    /// Called from [`Runtime::remove_container`]. Infallible: every `ip`
    /// invocation is best-effort because the user has already asked for the
    /// container to be gone, and a partial teardown is more useful than a
    /// hard error that leaves the cleanup half-done.
    async fn teardown_container_netns(&self, id: &ContainerId) {
        let state = self.netns.write().await.remove(id);
        let Some(state) = state else {
            return;
        };
        let slug = id_slug(id);
        match state {
            NetnsState::Configured { host_iface, .. } => {
                // Delete the host-end veth first (this also removes the
                // peer that was moved into the netns).
                let _ = self.wsl_run("ip", &["link", "delete", &host_iface]).await;
                let _ = self.wsl_run("ip", &["netns", "delete", &slug]).await;
            }
            NetnsState::HostFallback => {
                // Nothing to tear down; the container never got a netns.
            }
        }
    }

    /// Query `youki state` for the given container and parse its JSON.
    async fn query_state(&self, id: &ContainerId) -> Result<YoukiState> {
        let slug = id_slug(id);
        let output = self.youki(&["state", &slug]).await?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // Youki emits a consistent "not found" message when the
            // container id is unknown; translate to NotFound so callers get
            // the conventional 404 surface from the API layer.
            if stderr.to_ascii_lowercase().contains("does not exist")
                || stderr.to_ascii_lowercase().contains("not found")
            {
                return Err(AgentError::NotFound {
                    container: id.to_string(),
                    reason: format!("youki state reports container unknown: {stderr}"),
                });
            }
            return Err(youki_error("state", &output));
        }
        parse_youki_state(&output)
    }
}

// ---------------------------------------------------------------------------
// Runtime impl
// ---------------------------------------------------------------------------

#[async_trait]
impl Runtime for Wsl2DelegateRuntime {
    async fn pull_image(&self, image: &str) -> Result<()> {
        self.pull_image_with_policy(image, PullPolicy::IfNotPresent, None)
            .await
    }

    async fn pull_image_with_policy(
        &self,
        image: &str,
        policy: PullPolicy,
        auth: Option<&RegistryAuth>,
    ) -> Result<()> {
        // Fast path: already cached and the policy allows reuse.
        if matches!(policy, PullPolicy::IfNotPresent | PullPolicy::Never)
            && self.image_cache.read().await.contains_key(image)
        {
            if matches!(policy, PullPolicy::Never) {
                return Ok(());
            }
            tracing::debug!(image, "image cache hit; skipping re-pull");
            return Ok(());
        }
        if matches!(policy, PullPolicy::Never) {
            return Err(AgentError::PullFailed {
                image: image.to_string(),
                reason: "PullPolicy::Never but image is not in the WSL2 delegate cache".to_string(),
            });
        }

        // Pulls on the Windows host go through `zlayer_registry::ImagePuller`
        // because the WSL2 distro is *not* a registry: it has no credential
        // store, no reusable blob cache across daemon restarts, and — more
        // importantly — upstream `youki` has no real `pull` subcommand. Doing
        // the HTTP work here keeps a single code path for all runtimes and
        // gives us a `Vec<(blob, media_type)>` we can later stream directly
        // into the distro at `create_container` time.
        let registry_auth = match auth {
            Some(a) => zlayer_registry::RegistryAuth::Basic(a.username.clone(), a.password.clone()),
            None => zlayer_registry::RegistryAuth::Anonymous,
        };
        let cache = zlayer_registry::BlobCache::new().map_err(|e| AgentError::PullFailed {
            image: image.to_string(),
            reason: format!("failed to create blob cache: {e}"),
        })?;
        // The WSL2 delegate *always* runs linux/amd64 containers inside the
        // helper distro, even though the host is Windows. Pin the puller's
        // platform selector so multi-platform image indexes resolve to the
        // Linux manifest — otherwise `oci-client` would pick the Windows
        // variant (matching the host) and every pull would silently fail.
        let cache_arc: Arc<Box<dyn zlayer_registry::BlobCacheBackend>> = Arc::new(Box::new(cache));
        let puller = zlayer_registry::ImagePuller::with_platform(
            cache_arc,
            zlayer_spec::TargetPlatform::new(
                zlayer_spec::OsKind::Linux,
                zlayer_spec::ArchKind::Amd64,
            ),
        );

        let force_refresh = matches!(policy, PullPolicy::Always);
        let layers = puller
            .pull_image_with_policy(image, &registry_auth, force_refresh)
            .await
            .map_err(|e| AgentError::PullFailed {
                image: image.to_string(),
                reason: format!("registry pull failed: {e}"),
            })?;

        let config = puller
            .pull_image_config(image, &registry_auth)
            .await
            .map_err(|e| AgentError::PullFailed {
                image: image.to_string(),
                reason: format!("image config fetch failed: {e}"),
            })?;

        self.image_cache
            .write()
            .await
            .insert(image.to_string(), CachedImage { layers, config });

        Ok(())
    }

    async fn create_container(&self, id: &ContainerId, spec: &ServiceSpec) -> Result<()> {
        let bundle_dir = self.bundle_dir(id);
        let rootfs_dir = format!("{bundle_dir}/rootfs");
        let slug = id_slug(id);

        // 1. Make sure the bundle + rootfs directories exist. Idempotent.
        let mkdir = self.wsl("mkdir", &["-p", &rootfs_dir]).await?;
        if !mkdir.status.success() {
            return Err(AgentError::CreateFailed {
                id: id.to_string(),
                reason: format!(
                    "mkdir -p {rootfs_dir} failed (status {:?}): {}",
                    mkdir.status.code(),
                    String::from_utf8_lossy(&mkdir.stderr).trim()
                ),
            });
        }

        // 2. Fetch or reuse the pulled image data. When the composite layer
        //    has already pulled this image the cache hits immediately; when
        //    the caller skipped the pull step (e.g. direct `create_container`
        //    in a test) we fall through to the same pull path used by
        //    `pull_image_with_policy`.
        let cached = match self.image_cache.read().await.get(&spec.image.name).cloned() {
            Some(c) => c,
            None => {
                self.pull_image_with_policy(&spec.image.name, spec.image.pull_policy, None)
                    .await?;
                self.image_cache
                    .read()
                    .await
                    .get(&spec.image.name)
                    .cloned()
                    .ok_or_else(|| AgentError::CreateFailed {
                        id: id.to_string(),
                        reason: format!(
                            "image {} missing from WSL2 delegate cache after pull",
                            spec.image.name
                        ),
                    })?
            }
        };

        // 3. Extract every layer into <bundle_dir>/rootfs inside the distro.
        //    Decompression happens on the Windows host (synchronous, fast,
        //    already in RAM) so the tar stream we hand to `wsl.exe -- tar`
        //    is always a plain tarball — no gzip/zstd tooling required
        //    inside the distro. `--no-same-owner` keeps things working when
        //    WSL exposes a mismatched uid map.
        let tar_sh_cmd = format!("cd {rootfs_dir} && tar -xf - --no-same-owner");
        for (i, (data, media_type)) in cached.layers.iter().enumerate() {
            let tar_bytes =
                decompress_layer(data, media_type).map_err(|e| AgentError::CreateFailed {
                    id: id.to_string(),
                    reason: format!(
                        "failed to decompress layer {i} ({media_type}) of {}: {e}",
                        spec.image.name
                    ),
                })?;

            wsl_stdin_pipe(
                &["-d", &self.config.distro, "--", "sh", "-c", &tar_sh_cmd],
                &tar_bytes,
            )
            .await
            .map_err(|e| AgentError::CreateFailed {
                id: id.to_string(),
                reason: format!(
                    "streaming layer {i} ({media_type}) of {} into WSL2 rootfs failed: {e}",
                    spec.image.name
                ),
            })?;
        }

        // 4. Render the OCI runtime spec on the host using the cross-platform
        //    `BundleBuilder::build_spec_only` entry point (G-1). The bundle
        //    path passed to `BundleBuilder::new` is purely informational here
        //    — `build_spec_only` never touches the filesystem, so the
        //    Windows-style path is fine even though it'll never exist.
        let builder = BundleBuilder::new(PathBuf::from(&bundle_dir))
            .with_image_config(cached.config.clone())
            .with_hostname(slug.clone());
        let oci_spec = builder
            .build_spec_only(id, spec, &HashMap::new())
            .await
            .map_err(|e| AgentError::CreateFailed {
                id: id.to_string(),
                reason: format!("failed to build OCI spec on Windows host: {e}"),
            })?;

        let config_json =
            serde_json::to_string_pretty(&oci_spec).map_err(|e| AgentError::CreateFailed {
                id: id.to_string(),
                reason: format!("failed to serialize OCI spec to JSON: {e}"),
            })?;

        // 5. Stream the rendered `config.json` into the distro via `tee`.
        //    `tee` buffers the write inside the distro so WSL's stdin
        //    handling does not truncate partial UTF-8 at the filesystem
        //    boundary; redirecting stdout to /dev/null avoids echoing the
        //    payload back over the pipe.
        let config_path = format!("{bundle_dir}/config.json");
        let tee_sh_cmd = format!("tee {config_path} > /dev/null");
        wsl_stdin_pipe(
            &["-d", &self.config.distro, "--", "sh", "-c", &tee_sh_cmd],
            config_json.as_bytes(),
        )
        .await
        .map_err(|e| AgentError::CreateFailed {
            id: id.to_string(),
            reason: format!("failed to write config.json into WSL2 bundle: {e}"),
        })?;

        // 6. Hand the bundle to youki. `--log <path>` points at the per-
        //    container log file inside the distro so `container_logs` has
        //    something to tail — without this flag youki just writes to
        //    stderr which we can't easily read back later. A non-zero exit
        //    here rolls back the bundle directory so a retry sees a clean
        //    slate.
        let log_path = self.log_path(id);
        let create = self
            .youki(&["--log", &log_path, "create", "--bundle", &bundle_dir, &slug])
            .await?;
        if !create.status.success() {
            let stderr = String::from_utf8_lossy(&create.stderr).trim().to_string();
            // Best-effort cleanup: we don't care if it succeeds.
            let _ = self.wsl("rm", &["-rf", &bundle_dir]).await;
            return Err(AgentError::CreateFailed {
                id: id.to_string(),
                reason: format!(
                    "youki create failed (status {:?}): {stderr}",
                    create.status.code(),
                ),
            });
        }

        // 7. Record the bundle path so cleanup knows where to look later.
        self.bundle_roots
            .write()
            .await
            .insert(id.clone(), bundle_dir);
        Ok(())
    }

    async fn start_container(&self, id: &ContainerId) -> Result<()> {
        // J-3: program the container's Linux netns, veth pair, overlay IP
        // and default route inside the distro before youki actually starts
        // the init process. Failures here degrade to host networking with a
        // warning rather than aborting the start — documented fallback.
        self.setup_container_netns(id).await;

        let slug = id_slug(id);
        let output = self.youki(&["start", &slug]).await?;
        if !output.status.success() {
            // youki itself failed; clean up the netns we just configured so
            // we don't leak it on a start that never took effect.
            self.teardown_container_netns(id).await;
            return Err(AgentError::StartFailed {
                id: id.to_string(),
                reason: format!(
                    "youki start failed (status {:?}): {}",
                    output.status.code(),
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
            });
        }
        // Best-effort: cache the PID for later get_container_pid calls.
        if let Ok(state) = self.query_state(id).await {
            if let Some(pid) = state.pid {
                self.pids.write().await.insert(id.clone(), pid);
            }
        }
        Ok(())
    }

    async fn stop_container(&self, id: &ContainerId, timeout: Duration) -> Result<()> {
        let slug = id_slug(id);
        // SIGTERM first.
        let term = self.youki(&["kill", "--all", &slug, "SIGTERM"]).await?;
        if !term.status.success() {
            // If the container is already stopped youki returns nonzero;
            // treat that as success for stop semantics rather than erroring.
            let stderr = String::from_utf8_lossy(&term.stderr).to_ascii_lowercase();
            if !(stderr.contains("stopped") || stderr.contains("not running")) {
                return Err(youki_error("kill", &term));
            }
        }

        // Poll `state` until status == "stopped" or timeout expires.
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            match self.query_state(id).await {
                Ok(state) if state.is_stopped() => return Ok(()),
                Ok(_) => {}
                Err(AgentError::NotFound { .. }) => return Ok(()),
                Err(e) => return Err(e),
            }
            if tokio::time::Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(WAIT_POLL_INTERVAL).await;
        }

        // Escalate to SIGKILL.
        let kill = self.youki(&["kill", "--all", &slug, "SIGKILL"]).await?;
        if !kill.status.success() {
            let stderr = String::from_utf8_lossy(&kill.stderr).to_ascii_lowercase();
            if !(stderr.contains("stopped") || stderr.contains("not running")) {
                return Err(youki_error("kill", &kill));
            }
        }
        Ok(())
    }

    async fn remove_container(&self, id: &ContainerId) -> Result<()> {
        let slug = id_slug(id);
        let output = self.youki(&["delete", &slug]).await?;
        // Tear down the per-container netns + veth before clearing caches.
        // Best-effort: we've already told youki to delete the container,
        // and leaving a netns leak is less bad than failing the remove.
        self.teardown_container_netns(id).await;
        // Clear caches regardless of delete outcome — the container entry
        // is gone from our perspective once we've called `delete`.
        self.pids.write().await.remove(id);
        self.ips.write().await.remove(id);
        self.gateways.write().await.remove(id);
        // Best-effort: wipe the bundle directory in the distro so a future
        // create with the same id starts from a clean rootfs.
        if let Some(bundle_dir) = self.bundle_roots.write().await.remove(id) {
            if let Err(e) = self.wsl("rm", &["-rf", &bundle_dir]).await {
                tracing::debug!(
                    container = %id,
                    bundle_dir = %bundle_dir,
                    error = %e,
                    "failed to remove WSL2 bundle dir; leaving for GC"
                );
            }
        }
        // Same best-effort cleanup for the per-container youki log file —
        // otherwise re-creating a container with the same id would tail
        // lines from the previous incarnation.
        let log_path = self.log_path(id);
        if let Err(e) = self.wsl("rm", &["-f", &log_path]).await {
            tracing::debug!(
                container = %id,
                log_path = %log_path,
                error = %e,
                "failed to remove youki log file; leaving for GC"
            );
        }
        if output.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
            if stderr.contains("does not exist") || stderr.contains("not found") {
                return Ok(());
            }
            Err(youki_error("delete", &output))
        }
    }

    async fn container_state(&self, id: &ContainerId) -> Result<ContainerState> {
        let state = self.query_state(id).await?;
        Ok(state.as_container_state())
    }

    async fn container_logs(&self, id: &ContainerId, tail: usize) -> Result<Vec<LogEntry>> {
        // Read from the per-container log file that `youki create --log
        // <path>` writes to inside the distro. Before G-4 this pointed at a
        // fabricated `/var/log/youki/<slug>.stdout.log` that youki never
        // touched — now it's the same path `create_container` just passed
        // as `--log`, so a tail actually returns something.
        let log_path = self.log_path(id);
        let tail_str = tail.to_string();
        let output = self.wsl("tail", &["-n", &tail_str, &log_path]).await?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
            // First-call-before-any-log-writes: treat missing file as an
            // empty log rather than an error so startup polling is quiet.
            if stderr.contains("no such file") || stderr.contains("cannot open") {
                return Ok(Vec::new());
            }
            return Err(youki_error("tail", &output));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let now = chrono::Utc::now();
        let source = LogSource::Container(id.to_string());
        let service = Some(id.service.clone());
        let entries = stdout
            .lines()
            .filter(|l| !l.is_empty())
            .map(|line| LogEntry {
                timestamp: now,
                stream: LogStream::Stdout,
                message: line.to_string(),
                source: source.clone(),
                service: service.clone(),
                deployment: None,
            })
            .collect();
        Ok(entries)
    }

    async fn exec(&self, id: &ContainerId, cmd: &[String]) -> Result<(i32, String, String)> {
        if cmd.is_empty() {
            return Err(AgentError::InvalidSpec(
                "exec command must not be empty".to_string(),
            ));
        }
        let slug = id_slug(id);
        let mut args: Vec<&str> = vec!["exec", &slug, "--"];
        args.extend(cmd.iter().map(String::as_str));
        let output = self.youki(&args).await?;
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        let exit = output.status.code().unwrap_or(-1);
        Ok((exit, stdout, stderr))
    }

    async fn get_container_stats(&self, id: &ContainerId) -> Result<ContainerStats> {
        let slug = id_slug(id);
        let output = self.youki(&["events", "--stats", &slug]).await;
        let now = std::time::Instant::now();
        match output {
            Ok(out) if out.status.success() => Ok(parse_youki_stats(&out, now)),
            _ => {
                // If events isn't supported or the container isn't running,
                // return a default so autoscaling / metrics don't blow up.
                Ok(ContainerStats {
                    cpu_usage_usec: 0,
                    memory_bytes: 0,
                    memory_limit: u64::MAX,
                    timestamp: now,
                })
            }
        }
    }

    async fn wait_container(&self, id: &ContainerId) -> Result<i32> {
        let start = tokio::time::Instant::now();
        loop {
            match self.query_state(id).await {
                Ok(state) if state.is_stopped() => {
                    return Ok(state.exit_code.unwrap_or(0));
                }
                Ok(_) => {}
                Err(e) => return Err(e),
            }
            if start.elapsed() >= WAIT_POLL_CAP {
                return Err(AgentError::Timeout {
                    timeout: WAIT_POLL_CAP,
                });
            }
            tokio::time::sleep(WAIT_POLL_INTERVAL).await;
        }
    }

    async fn get_logs(&self, id: &ContainerId) -> Result<Vec<LogEntry>> {
        self.container_logs(id, usize::MAX).await
    }

    async fn get_container_pid(&self, id: &ContainerId) -> Result<Option<u32>> {
        if let Some(pid) = self.pids.read().await.get(id).copied() {
            return Ok(Some(pid));
        }
        // Fallback: query youki state and update the cache.
        match self.query_state(id).await {
            Ok(state) => {
                if let Some(pid) = state.pid {
                    self.pids.write().await.insert(id.clone(), pid);
                    Ok(Some(pid))
                } else {
                    Ok(None)
                }
            }
            Err(AgentError::NotFound { .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }

    async fn get_container_ip(&self, id: &ContainerId) -> Result<Option<IpAddr>> {
        // If we tried to program the netns and fell back to host networking,
        // the recorded IP is not actually live inside the container. Hide it
        // so downstream consumers (DNS registration, service discovery)
        // don't advertise an IP the workload cannot bind to.
        if matches!(
            self.netns.read().await.get(id),
            Some(NetnsState::HostFallback)
        ) {
            return Ok(None);
        }
        Ok(self.ips.read().await.get(id).copied())
    }

    async fn list_images(&self) -> Result<Vec<ImageInfo>> {
        // Youki doesn't (yet) ship an image-list subcommand that we can rely
        // on. Be explicit about the unsupported surface rather than pretending
        // to return a successful empty list.
        Err(AgentError::Unsupported(
            "list_images is not supported by the WSL2 delegate runtime \
             (youki has no image registry; images are managed on the host)"
                .to_string(),
        ))
    }

    async fn remove_image(&self, _image: &str, _force: bool) -> Result<()> {
        Err(AgentError::Unsupported(
            "remove_image is not supported by the WSL2 delegate runtime".to_string(),
        ))
    }

    async fn prune_images(&self) -> Result<PruneResult> {
        Err(AgentError::Unsupported(
            "prune_images is not supported by the WSL2 delegate runtime".to_string(),
        ))
    }

    async fn kill_container(&self, id: &ContainerId, signal: Option<&str>) -> Result<()> {
        let canonical = validate_signal(signal.unwrap_or("SIGKILL"))?;
        let slug = id_slug(id);
        let output = self.youki(&["kill", &slug, &canonical]).await?;
        if output.status.success() {
            Ok(())
        } else {
            Err(youki_error("kill", &output))
        }
    }

    async fn tag_image(&self, _source: &str, _target: &str) -> Result<()> {
        Err(AgentError::Unsupported(
            "tag_image is not supported by the WSL2 delegate runtime".to_string(),
        ))
    }

    async fn inspect_detailed(&self, _id: &ContainerId) -> Result<ContainerInspectDetails> {
        Ok(ContainerInspectDetails::default())
    }

    /// Real line-by-line streaming over the `wsl.exe` boundary.
    ///
    /// Spawns `wsl.exe -d <distro> -- <youki> exec <slug> -- <cmd...>` with
    /// piped stdout and stderr, then drives two `BufReader::lines()` loops on
    /// background tasks. Each line becomes one [`ExecEvent::Stdout`] /
    /// [`ExecEvent::Stderr`]; once both readers reach EOF and the child
    /// process exits, a final [`ExecEvent::Exit`] is emitted and the stream
    /// closes. Errors surfaced before the child is spawned (empty cmd,
    /// `wsl.exe` not launchable) flow through the outer `Result`; post-spawn
    /// errors are logged and the stream closes with `ExecEvent::Exit(-1)`.
    async fn exec_stream(&self, id: &ContainerId, cmd: &[String]) -> Result<ExecEventStream> {
        if cmd.is_empty() {
            return Err(AgentError::InvalidSpec(
                "exec command must not be empty".to_string(),
            ));
        }

        let slug = id_slug(id);
        // Build the `wsl.exe` argv: `-d <distro> -- <youki_path> exec <slug>
        // -- <user_cmd...>`. Everything is owned strings because we hand
        // them off to a background task below and can't rely on the &[&str]
        // borrow outliving the spawn.
        let mut argv: Vec<String> = Vec::with_capacity(6 + cmd.len());
        argv.push("-d".to_string());
        argv.push(self.config.distro.clone());
        argv.push("--".to_string());
        argv.push(self.config.youki_path.clone());
        argv.push("exec".to_string());
        argv.push(slug);
        argv.push("--".to_string());
        argv.extend(cmd.iter().cloned());

        let mut child = tokio::process::Command::new("wsl.exe")
            .args(&argv)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| AgentError::Network(format!("wsl.exe spawn for exec_stream: {e}")))?;

        let stdout = child.stdout.take().ok_or_else(|| {
            AgentError::Internal("wsl.exe child did not expose a stdout handle".to_string())
        })?;
        let stderr = child.stderr.take().ok_or_else(|| {
            AgentError::Internal("wsl.exe child did not expose a stderr handle".to_string())
        })?;

        Ok(spawn_exec_event_stream(child, stdout, stderr))
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Execute `cmd` inside a specific WSL2 distro, converting any transport
/// error into [`AgentError::Network`]. Used by [`Wsl2DelegateRuntime`]'s
/// per-instance helpers so the distro name is threaded through configuration
/// rather than hardcoded against `zlayer_wsl::distro::DISTRO_NAME`.
///
/// This is the config-aware replacement for the old `wsl_exec_or` helper
/// (which wrapped [`zlayer_wsl::distro::wsl_exec`], a function hardcoded to
/// the `zlayer` distro).
async fn wsl_exec_in(distro: &str, cmd: &str, args: &[&str]) -> Result<Output> {
    let mut wsl_args: Vec<&str> = vec!["-d", distro, "--", cmd];
    wsl_args.extend_from_slice(args);
    tokio::process::Command::new("wsl.exe")
        .args(&wsl_args)
        .output()
        .await
        .map_err(|e| AgentError::Network(format!("wsl.exe -d {distro} -- {cmd}: {e}")))
}

/// Wire up the background tasks that turn a spawned `wsl.exe` child's
/// stdout + stderr pipes into an ordered [`ExecEventStream`].
///
/// Factored out of [`Wsl2DelegateRuntime::exec_stream`] so the task graph —
/// two line-reader tasks plus one waiter that sends the terminal
/// [`ExecEvent::Exit`] — is named and unit-testable. The returned stream
/// always terminates with exactly one `ExecEvent::Exit`.
fn spawn_exec_event_stream(
    mut child: tokio::process::Child,
    stdout: tokio::process::ChildStdout,
    stderr: tokio::process::ChildStderr,
) -> ExecEventStream {
    let (tx, rx) = mpsc::channel::<ExecEvent>(128);

    let tx_stdout = tx.clone();
    let stdout_task = tokio::spawn(async move {
        let mut reader = BufReader::new(stdout).lines();
        loop {
            match reader.next_line().await {
                Ok(Some(line)) => {
                    if tx_stdout.send(ExecEvent::Stdout(line)).await.is_err() {
                        break;
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    tracing::warn!(error = %e, "exec_stream: stdout read error");
                    break;
                }
            }
        }
    });

    let tx_stderr = tx.clone();
    let stderr_task = tokio::spawn(async move {
        let mut reader = BufReader::new(stderr).lines();
        loop {
            match reader.next_line().await {
                Ok(Some(line)) => {
                    if tx_stderr.send(ExecEvent::Stderr(line)).await.is_err() {
                        break;
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    tracing::warn!(error = %e, "exec_stream: stderr read error");
                    break;
                }
            }
        }
    });

    // Supervisor: wait for both readers to drain, reap the child, and emit
    // the terminal `Exit` event. Keeping `tx` alive in *this* task (and only
    // this task) is load-bearing — it's how the receiver side observes
    // stream termination after the readers have dropped their clones.
    tokio::spawn(async move {
        let _ = stdout_task.await;
        let _ = stderr_task.await;
        let exit_code = match child.wait().await {
            Ok(status) => status.code().unwrap_or(-1),
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "wsl.exe exec_stream child wait failed; reporting exit -1"
                );
                -1
            }
        };
        // `send` fails only when the receiver has been dropped — benign.
        let _ = tx.send(ExecEvent::Exit(exit_code)).await;
    });

    Box::pin(ReceiverStream::new(rx))
}

/// Spawn `wsl.exe` with the supplied argv and pipe `stdin_bytes` into its
/// standard input, returning an error if the process exits non-zero or
/// stdin cannot be written.
///
/// The existing [`zlayer_wsl::distro::wsl_exec`] helper uses `Command::output`
/// which does not expose stdin; we need real stdin streaming for
/// `tee config.json` + `tar -xf -`. Lives inside this module (rather than
/// as a cross-crate helper in `zlayer-wsl`) because it is only load-bearing
/// for the WSL2 delegate today.
async fn wsl_stdin_pipe(args: &[&str], stdin_bytes: &[u8]) -> std::io::Result<()> {
    let mut child = tokio::process::Command::new("wsl.exe")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(stdin_bytes).await?;
        stdin.shutdown().await?;
    } else {
        return Err(std::io::Error::other(
            "wsl.exe child did not expose a stdin handle",
        ));
    }

    let output = child.wait_with_output().await?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(std::io::Error::other(format!(
            "wsl.exe exited with {:?}: {}",
            output.status.code(),
            stderr.trim()
        )));
    }
    Ok(())
}

/// Decompress an OCI layer blob in-process so the caller can pipe the raw
/// tarball bytes through `wsl.exe -- tar -xf -`.
///
/// Uses [`CompressionType::detect`] — same logic as the Linux-side unpacker
/// in `zlayer-registry` — so every media type the agent has ever pulled is
/// handled consistently. Gzip and zstd decompression is performed with the
/// blocking `flate2::read::GzDecoder` / `zstd::stream::Decoder` APIs because
/// the input is fully buffered in memory and the whole flow runs under
/// `spawn_blocking`-equivalent time anyway (wsl.exe boot dominates).
fn decompress_layer(data: &[u8], media_type: &str) -> std::io::Result<Vec<u8>> {
    let compression = CompressionType::detect(media_type, data);
    match compression {
        CompressionType::None => Ok(data.to_vec()),
        CompressionType::Gzip => {
            let mut out = Vec::with_capacity(data.len());
            let mut decoder = flate2::read::GzDecoder::new(data);
            decoder.read_to_end(&mut out)?;
            Ok(out)
        }
        CompressionType::Zstd => {
            let mut out = Vec::with_capacity(data.len());
            let mut decoder = zstd::stream::Decoder::new(data)?;
            decoder.read_to_end(&mut out)?;
            Ok(out)
        }
    }
}

/// Build a conventional `AgentError::Network` describing a nonzero youki exit.
///
/// Format: `youki <subcommand> failed (status <code>): <stderr>`. Never
/// swallows stderr so the user sees both the command and the distro's own
/// diagnostic output.
fn youki_error(subcommand: &str, output: &Output) -> AgentError {
    let status = output.status.code();
    let stderr = String::from_utf8_lossy(&output.stderr);
    AgentError::Network(format!(
        "youki {subcommand} failed (status {status:?}): {}",
        stderr.trim()
    ))
}

/// Produce a youki-compatible slug for a [`ContainerId`].
///
/// `ContainerId::Display` emits `"{service}-rep-{replica}"`, which is already
/// alphanumeric-plus-dashes except for the degenerate case where a service
/// name contains characters youki rejects (whitespace, quotes, slashes). This
/// helper filters those so a malformed service spec can't break shell-out
/// argument quoting.
fn id_slug(id: &ContainerId) -> String {
    let raw = id.to_string();
    raw.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// Subset of `youki state` output we care about. Matches the runtime-spec
/// `State` document, which youki emits verbatim.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct YoukiState {
    /// `creating` | `created` | `running` | `stopped`.
    status: String,
    /// PID of the init process inside the container (absent for `creating`).
    #[serde(default)]
    pid: Option<u32>,
    /// Exit code — youki adds this on top of the spec when status is
    /// `stopped`. `None` while the container is still alive.
    #[serde(default)]
    exit_code: Option<i32>,
}

impl YoukiState {
    fn is_stopped(&self) -> bool {
        self.status.eq_ignore_ascii_case("stopped")
    }

    fn as_container_state(&self) -> ContainerState {
        match self.status.to_ascii_lowercase().as_str() {
            "creating" => ContainerState::Pending,
            "created" => ContainerState::Initializing,
            "running" => ContainerState::Running,
            "stopped" => ContainerState::Exited {
                code: self.exit_code.unwrap_or(0),
            },
            other => ContainerState::Failed {
                reason: format!("unknown youki state: {other}"),
            },
        }
    }
}

/// Parse the JSON payload emitted by `youki state <id>`.
fn parse_youki_state(output: &Output) -> Result<YoukiState> {
    let stdout = std::str::from_utf8(&output.stdout)
        .map_err(|e| AgentError::Internal(format!("youki state: stdout not utf-8: {e}")))?;
    serde_json::from_str::<YoukiState>(stdout.trim()).map_err(|e| {
        AgentError::Internal(format!(
            "youki state: failed to parse JSON: {e} (raw: {:?})",
            stdout.chars().take(256).collect::<String>()
        ))
    })
}

/// Best-effort parser for the JSON `youki events --stats <id>` payload.
///
/// Youki follows the `runc`-compatible shape: `{"cpu":{"usage":{"total":N}},
/// "memory":{"usage":{"usage":N,"limit":M}}}`. We tolerate missing fields by
/// defaulting to zero / `u64::MAX` so callers never see a malformed-payload
/// error for a metrics sample.
fn parse_youki_stats(output: &Output, timestamp: std::time::Instant) -> ContainerStats {
    let raw = String::from_utf8_lossy(&output.stdout);
    let v: serde_json::Value = match serde_json::from_str(raw.trim()) {
        Ok(v) => v,
        Err(_) => {
            return ContainerStats {
                cpu_usage_usec: 0,
                memory_bytes: 0,
                memory_limit: u64::MAX,
                timestamp,
            };
        }
    };
    // CPU total is in nanoseconds for runc-style stats; convert to usec.
    let cpu_ns = v
        .pointer("/cpu/usage/total")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let cpu_usage_usec = cpu_ns / 1_000;
    let memory_bytes = v
        .pointer("/memory/usage/usage")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let memory_limit = v
        .pointer("/memory/usage/limit")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(u64::MAX);
    ContainerStats {
        cpu_usage_usec,
        memory_bytes,
        memory_limit,
        timestamp,
    }
}

// ---------------------------------------------------------------------------
// Tests — pure-logic coverage only. Anything that shells out to wsl.exe is
// tested by the Windows-gated integration suite in Phase F-9.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;
    #[cfg(not(target_os = "windows"))]
    use std::os::unix::process::ExitStatusExt;
    #[cfg(target_os = "windows")]
    use std::os::windows::process::ExitStatusExt;
    use std::process::ExitStatus;
    use std::sync::Mutex as StdMutex;

    fn cid(service: &str, replica: u32) -> ContainerId {
        ContainerId {
            service: service.to_string(),
            replica,
        }
    }

    #[cfg(target_os = "windows")]
    fn make_exit_status(code: i32) -> ExitStatus {
        // On Windows, `ExitStatus::from_raw` takes the raw Windows process
        // exit code as `u32`; negative i32 values are reinterpreted
        // bit-for-bit, which is what we want for synthesising realistic
        // status payloads in tests.
        #[allow(clippy::cast_sign_loss)]
        let raw = code as u32;
        ExitStatus::from_raw(raw)
    }

    #[cfg(not(target_os = "windows"))]
    fn make_exit_status(code: i32) -> ExitStatus {
        // On Unix, `ExitStatus::from_raw` takes the raw wait-status `i32`.
        // Shift so the low byte encodes our synthesised code (Unix packs
        // the exit code in bits 8..15).
        ExitStatus::from_raw(code << 8)
    }

    fn fake_output(stdout: &str, code: i32) -> Output {
        Output {
            status: make_exit_status(code),
            stdout: stdout.as_bytes().to_vec(),
            stderr: Vec::new(),
        }
    }

    fn fake_output_err(stderr: &str, code: i32) -> Output {
        Output {
            status: make_exit_status(code),
            stdout: Vec::new(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    /// Shared call log: one entry per `wsl.exe` invocation, captured as
    /// `(cmd, args)` so tests can assert the exact command sequence.
    type CallLog = Arc<StdMutex<Vec<(String, Vec<String>)>>>;

    /// Recording fake [`WslRunner`] used to verify the sequence of commands
    /// emitted by `setup_container_netns` / `teardown_container_netns`.
    struct RecordingRunner {
        calls: CallLog,
        /// Per-(cmd, args-join) canned responses. If a call is not present
        /// in the map we return a success code with empty output, which is
        /// the right default for the happy path.
        responses: StdMutex<HashMap<String, Output>>,
    }

    impl RecordingRunner {
        fn new() -> Self {
            Self {
                calls: Arc::new(StdMutex::new(Vec::new())),
                responses: StdMutex::new(HashMap::new()),
            }
        }

        fn calls_handle(&self) -> CallLog {
            Arc::clone(&self.calls)
        }

        fn key(cmd: &str, args: &[&str]) -> String {
            let mut k = String::from(cmd);
            for a in args {
                k.push(' ');
                k.push_str(a);
            }
            k
        }

        /// Install a canned response for the exact `cmd args...` invocation.
        fn set_response(&self, cmd: &str, args: &[&str], output: Output) {
            self.responses
                .lock()
                .expect("responses mutex poisoned")
                .insert(Self::key(cmd, args), output);
        }
    }

    #[async_trait]
    impl WslRunner for RecordingRunner {
        async fn run(&self, cmd: &str, args: &[&str]) -> anyhow::Result<Output> {
            self.calls.lock().expect("calls mutex poisoned").push((
                cmd.to_string(),
                args.iter().map(|s| (*s).to_string()).collect(),
            ));
            let key = Self::key(cmd, args);
            if let Some(out) = self
                .responses
                .lock()
                .expect("responses mutex poisoned")
                .remove(&key)
            {
                Ok(out)
            } else {
                Ok(fake_output("", 0))
            }
        }
    }

    /// Default `ResolvedConfig` for tests that don't care about the exact
    /// distro name / paths.
    fn default_resolved_config() -> ResolvedConfig {
        ResolvedConfig {
            distro: zlayer_wsl::distro::DISTRO_NAME.to_string(),
            youki_path: "youki".to_string(),
            bundle_root: DEFAULT_BUNDLE_ROOT.to_string(),
            log_root: DEFAULT_LOG_ROOT.to_string(),
        }
    }

    /// Build a runtime with a fully resolved config + a caller-supplied
    /// runner. Used by the J-3 netns tests so the in-memory recording
    /// runner captures the emitted `ip` commands.
    fn test_runtime_with_runner(
        config: ResolvedConfig,
        runner: Arc<dyn WslRunner>,
    ) -> Wsl2DelegateRuntime {
        Wsl2DelegateRuntime {
            config,
            pids: Arc::new(RwLock::new(HashMap::new())),
            ips: Arc::new(RwLock::new(HashMap::new())),
            bundle_roots: Arc::new(RwLock::new(HashMap::new())),
            image_cache: Arc::new(RwLock::new(HashMap::new())),
            gateways: Arc::new(RwLock::new(HashMap::new())),
            netns: Arc::new(RwLock::new(HashMap::new())),
            runner,
        }
    }

    /// Shorthand for the G-2/G-4/G-5 unit tests that don't exercise the
    /// J-3 runner plumbing. Uses the default (production) runner — those
    /// tests don't actually dispatch `ip` commands.
    fn test_runtime(config: ResolvedConfig) -> Wsl2DelegateRuntime {
        test_runtime_with_runner(config, Arc::new(DefaultWslRunner))
    }

    /// Shorthand for the J-3 tests that want a default-config runtime with
    /// an injected recording runner.
    fn make_runtime(runner: Arc<dyn WslRunner>) -> Wsl2DelegateRuntime {
        test_runtime_with_runner(default_resolved_config(), runner)
    }

    #[test]
    fn id_slug_sanitizes_special_chars() {
        // Normal ContainerId formats produce an already-safe slug.
        assert_eq!(id_slug(&cid("web", 0)), "web-rep-0");

        // Weird service names get their offending characters replaced with
        // dashes so youki never sees a shell metacharacter.
        let weird = cid("my svc/with:quotes", 3);
        let slug = id_slug(&weird);
        assert!(
            slug.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "slug must be alnum/dash/underscore only: {slug}"
        );
        assert!(
            slug.contains("my-svc"),
            "slug should preserve alnum: {slug}"
        );
        assert!(slug.ends_with("-rep-3"), "slug should keep replica: {slug}");
    }

    #[test]
    fn parse_youki_state_running() {
        let raw = r#"{"ociVersion":"1.0.2","id":"web-rep-0","status":"running","pid":12345,"bundle":"/var/lib/zlayer/bundles/web-rep-0"}"#;
        let out = fake_output(raw, 0);
        let state = parse_youki_state(&out).expect("valid state JSON");
        assert_eq!(state.status, "running");
        assert_eq!(state.pid, Some(12345));
        assert_eq!(state.exit_code, None);
        assert!(!state.is_stopped());
        assert_eq!(state.as_container_state(), ContainerState::Running);
    }

    #[test]
    fn parse_youki_state_stopped() {
        let raw = r#"{"ociVersion":"1.0.2","id":"job-rep-0","status":"stopped","exitCode":42}"#;
        let out = fake_output(raw, 0);
        let state = parse_youki_state(&out).expect("valid state JSON");
        assert_eq!(state.status, "stopped");
        assert_eq!(state.exit_code, Some(42));
        assert!(state.is_stopped());
        assert_eq!(
            state.as_container_state(),
            ContainerState::Exited { code: 42 }
        );
    }

    #[test]
    fn parse_youki_state_creating_maps_to_pending() {
        let raw = r#"{"ociVersion":"1.0.2","id":"x","status":"creating"}"#;
        let out = fake_output(raw, 0);
        let state = parse_youki_state(&out).expect("valid state JSON");
        assert_eq!(state.as_container_state(), ContainerState::Pending);
    }

    #[test]
    fn parse_youki_state_unknown_maps_to_failed() {
        let raw = r#"{"ociVersion":"1.0.2","id":"x","status":"paused"}"#;
        let out = fake_output(raw, 0);
        let state = parse_youki_state(&out).expect("valid state JSON");
        assert!(matches!(
            state.as_container_state(),
            ContainerState::Failed { .. }
        ));
    }

    #[test]
    fn parse_youki_stats_handles_full_payload() {
        let raw = r#"{"cpu":{"usage":{"total":2500000000}},"memory":{"usage":{"usage":104857600,"limit":268435456}}}"#;
        let out = fake_output(raw, 0);
        let stats = parse_youki_stats(&out, std::time::Instant::now());
        // 2_500_000_000 ns / 1_000 = 2_500_000 usec.
        assert_eq!(stats.cpu_usage_usec, 2_500_000);
        assert_eq!(stats.memory_bytes, 104_857_600);
        assert_eq!(stats.memory_limit, 268_435_456);
    }

    #[test]
    fn parse_youki_stats_defaults_on_malformed() {
        let out = fake_output("not json", 0);
        let stats = parse_youki_stats(&out, std::time::Instant::now());
        assert_eq!(stats.cpu_usage_usec, 0);
        assert_eq!(stats.memory_bytes, 0);
        assert_eq!(stats.memory_limit, u64::MAX);
    }

    #[tokio::test]
    async fn record_container_ip_then_get_returns_it() {
        let runtime = make_runtime(Arc::new(RecordingRunner::new()));
        let id = cid("web", 2);
        let ip = IpAddr::V4(Ipv4Addr::new(10, 200, 0, 42));

        // Before recording, get returns None.
        assert_eq!(runtime.get_container_ip(&id).await.unwrap(), None);

        runtime.record_container_ip(&id, ip).await;
        assert_eq!(runtime.get_container_ip(&id).await.unwrap(), Some(ip));
    }

    #[test]
    fn decompress_layer_plain_tar_is_noop() {
        // Plain tar (no compression) should round-trip byte-for-byte. This
        // is the "raw" arm of `CompressionType::detect`, hit by magic-bytes
        // fallback when an OCI media type carries no compression suffix.
        let payload = b"ustar-tar-payload".to_vec();
        let out = decompress_layer(&payload, zlayer_registry::unpack::media_types::TAR)
            .expect("plain tar is a no-op");
        assert_eq!(out, payload);
    }

    #[test]
    fn decompress_layer_gzip_roundtrip() {
        use flate2::write::GzEncoder;
        use flate2::Compression;
        use std::io::Write as _;

        let original: &[u8] = b"hello-wsl2-linux-layer-bytes";
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(original).unwrap();
        let compressed = encoder.finish().unwrap();

        let out = decompress_layer(&compressed, zlayer_registry::unpack::media_types::TAR_GZIP)
            .expect("gzip decompression must succeed");
        assert_eq!(out, original);
    }

    #[test]
    fn decompress_layer_honors_magic_bytes_when_media_type_unknown() {
        // Even with an unrecognized media type, `CompressionType::detect`
        // falls back to magic-byte sniffing. This guards against partially
        // labelled blobs leaking into `create_container`'s rootfs path.
        use flate2::write::GzEncoder;
        use flate2::Compression;
        use std::io::Write as _;

        let original: &[u8] = b"magic-byte-fallback";
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(original).unwrap();
        let compressed = encoder.finish().unwrap();

        let out = decompress_layer(&compressed, "application/octet-stream")
            .expect("fallback-to-magic path must still decompress");
        assert_eq!(out, original);
    }

    #[tokio::test]
    async fn create_container_no_longer_returns_unsupported() {
        // The G-2 contract: `create_container` is wired end-to-end. Without
        // a live WSL2 distro or a cached image, the call must still fail
        // *cleanly* (CreateFailed / PullFailed / Network) rather than with
        // the old `AgentError::Unsupported` stub. This test codifies the
        // negative assertion so a future stub-regression fails loudly.
        use zlayer_spec::DeploymentSpec;

        let runtime = test_runtime(default_resolved_config());
        let id = cid("svc", 0);
        let yaml = r"
version: v1
deployment: wsl2-g2-test
services:
  svc:
    rtype: service
    image:
      name: docker.io/library/alpine:3.19
    endpoints:
      - name: http
        protocol: http
        port: 8080
";
        let spec = serde_yaml::from_str::<DeploymentSpec>(yaml)
            .expect("valid deployment yaml")
            .services
            .remove("svc")
            .expect("service 'svc' present");

        let err = runtime.create_container(&id, &spec).await.unwrap_err();
        assert!(
            !matches!(err, AgentError::Unsupported(_)),
            "create_container must not return Unsupported after G-2 (got {err:?})",
        );
    }

    // -----------------------------------------------------------------
    // G-3: Real exec streaming — parser-level test.
    //
    // Running the actual `wsl.exe` child is Windows-only and requires a
    // live `zlayer` distro with youki, so the e2e exec-streaming test
    // lives in `composite_dispatch_e2e.rs` (`#[ignore]`'d). Here we
    // exercise the helper that pumps pipe output into `ExecEvent`s
    // against an in-process child process (`cmd.exe`) whose output is
    // fully deterministic.
    // -----------------------------------------------------------------

    /// `spawn_exec_event_stream` must turn a real child's piped stdout
    /// into one `ExecEvent::Stdout` per line and close with exactly one
    /// `ExecEvent::Exit` carrying the child's exit code. We drive
    /// `cmd.exe /c echo a & echo b & exit 7` which is portable across
    /// every Windows host and produces two deterministic stdout lines.
    #[tokio::test]
    async fn exec_stream_pump_yields_line_events_then_exit() {
        use futures_util::stream::StreamExt as _;

        let mut child = tokio::process::Command::new("cmd.exe")
            .args(["/c", "echo a & echo b & exit 7"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("cmd.exe must be available on Windows test hosts");

        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");

        let mut stream = spawn_exec_event_stream(child, stdout, stderr);

        let mut stdout_lines: Vec<String> = Vec::new();
        let mut exit_code: Option<i32> = None;
        while let Some(ev) = stream.next().await {
            match ev {
                ExecEvent::Stdout(line) => stdout_lines.push(line.trim().to_string()),
                ExecEvent::Stderr(_) => {}
                ExecEvent::Exit(code) => {
                    exit_code = Some(code);
                    break;
                }
            }
        }

        assert_eq!(
            stdout_lines,
            vec!["a".to_string(), "b".to_string()],
            "stdout should have been split into one event per line",
        );
        assert_eq!(
            exit_code,
            Some(7),
            "terminal Exit event must carry the child's exit code",
        );
    }

    // -----------------------------------------------------------------
    // G-4: Youki log path plumbed through.
    // -----------------------------------------------------------------

    /// The log-path helper must compose the configured `log_root` with the
    /// slug derived from [`ContainerId`]. This is the exact string we hand
    /// to `youki create --log` and later tail from in `container_logs`, so
    /// a drift between the two would silently produce empty log responses.
    #[test]
    fn log_path_uses_configured_log_root() {
        let runtime = test_runtime(ResolvedConfig {
            distro: "zlayer".to_string(),
            youki_path: "youki".to_string(),
            bundle_root: "/custom/bundles".to_string(),
            log_root: "/custom/logs".to_string(),
        });
        let id = cid("web", 7);
        assert_eq!(
            runtime.log_path(&id),
            "/custom/logs/web-rep-7.youki.log",
            "log_path must be <log_root>/<slug>.youki.log",
        );
    }

    /// Sanity-check the default log root: tests that construct a runtime
    /// via [`default_resolved_config`] should see `/var/lib/zlayer/logs`,
    /// matching [`DEFAULT_LOG_ROOT`].
    #[test]
    fn log_path_uses_default_log_root_by_default() {
        let runtime = test_runtime(default_resolved_config());
        let id = cid("svc", 0);
        assert!(
            runtime.log_path(&id).starts_with(DEFAULT_LOG_ROOT),
            "default log_path should live under DEFAULT_LOG_ROOT ({DEFAULT_LOG_ROOT}), got {}",
            runtime.log_path(&id),
        );
    }

    // -----------------------------------------------------------------
    // G-5: Config-driven distro name + youki path discovery.
    // -----------------------------------------------------------------

    /// [`Wsl2DelegateConfig::default`] must reproduce the hardcoded
    /// constants this module used to carry — otherwise an operator
    /// upgrading from pre-G-5 would silently get a different layout.
    #[test]
    fn default_config_matches_previous_hardcoded_values() {
        let cfg = Wsl2DelegateConfig::default();
        assert_eq!(cfg.distro, zlayer_wsl::distro::DISTRO_NAME);
        assert_eq!(cfg.youki_path, None);
        assert_eq!(cfg.bundle_root, DEFAULT_BUNDLE_ROOT);
        assert_eq!(cfg.log_root, DEFAULT_LOG_ROOT);
    }

    /// A runtime built from a custom [`Wsl2DelegateConfig`] must propagate
    /// the chosen distro name + youki path into every derived field so
    /// subsequent `wsl.exe` invocations target the right distro. We can't
    /// actually drive `wsl.exe` from unit tests, but we *can* assert that
    /// the runtime's `bundle_dir` / `log_path` / `config` carry the custom
    /// values.
    #[test]
    fn custom_config_propagates_into_runtime_fields() {
        let runtime = test_runtime(ResolvedConfig {
            distro: "ubuntu-lts".to_string(),
            youki_path: "/opt/youki/bin/youki".to_string(),
            bundle_root: "/srv/zlayer/bundles".to_string(),
            log_root: "/srv/zlayer/logs".to_string(),
        });
        let id = cid("api", 0);

        assert_eq!(runtime.config.distro, "ubuntu-lts");
        assert_eq!(runtime.config.youki_path, "/opt/youki/bin/youki");
        assert_eq!(
            runtime.bundle_dir(&id),
            "/srv/zlayer/bundles/api-rep-0",
            "bundle_dir should use the configured bundle_root",
        );
        assert_eq!(
            runtime.log_path(&id),
            "/srv/zlayer/logs/api-rep-0.youki.log",
            "log_path should use the configured log_root",
        );
    }

    // -----------------------------------------------------------------
    // J-3: Per-container netns lifecycle.
    // -----------------------------------------------------------------

    /// J-3: a recorded IP + gateway should drive the full setup sequence —
    /// netns create, veth pair, move one end into the netns, rename it to
    /// `eth0`, assign the IP, bring up both ends, add the default route.
    #[tokio::test]
    async fn setup_container_netns_emits_full_sequence() {
        let runner = Arc::new(RecordingRunner::new());
        let calls = runner.calls_handle();
        let runtime = make_runtime(runner.clone());

        let id = cid("web", 0);
        let ip = IpAddr::V4(Ipv4Addr::new(10, 200, 0, 42));
        let gw = IpAddr::V4(Ipv4Addr::new(10, 200, 0, 1));
        runtime.record_container_ip(&id, ip).await;
        runtime.record_container_gateway(&id, gw).await;

        runtime.setup_container_netns(&id).await;

        let recorded = calls.lock().unwrap().clone();
        let joined: Vec<String> = recorded
            .iter()
            .map(|(c, a)| format!("{} {}", c, a.join(" ")))
            .collect();
        // Must include: netns add, veth pair create, move-into-netns, addr
        // add with correct CIDR, default-route add with correct gateway.
        let must_contain = [
            "ip netns add web-rep-0",
            "type veth peer name",
            "netns web-rep-0",
            "ip addr add 10.200.0.42/24 dev eth0",
            "ip route add default via 10.200.0.1",
        ];
        for needle in must_contain {
            assert!(
                joined.iter().any(|c| c.contains(needle)),
                "expected a command matching {needle:?} in {joined:?}"
            );
        }

        // State should reflect a successful configuration.
        let netns = runtime.netns.read().await;
        match netns.get(&id) {
            Some(NetnsState::Configured {
                host_iface,
                ip: got_ip,
            }) => {
                assert_eq!(*got_ip, ip);
                assert!(
                    host_iface.starts_with("zl-") && host_iface.len() <= 15,
                    "host_iface must be a zl- prefixed IFNAMSIZ-safe name, got {host_iface:?}"
                );
            }
            other => panic!("expected Configured, got {other:?}"),
        }
    }

    /// J-3: no recorded IP means host networking — setup is a no-op and the
    /// state map stays empty.
    #[tokio::test]
    async fn setup_container_netns_no_ip_is_noop() {
        let runner = Arc::new(RecordingRunner::new());
        let calls = runner.calls_handle();
        let runtime = make_runtime(runner.clone());

        let id = cid("web", 1);
        runtime.setup_container_netns(&id).await;

        assert!(
            calls.lock().unwrap().is_empty(),
            "no IP => no wsl.exe calls"
        );
        assert!(runtime.netns.read().await.get(&id).is_none());
    }

    /// J-3: when a mid-setup command fails, we log + fall back to host
    /// networking rather than erroring, and `get_container_ip` must hide the
    /// recorded IP so downstream DNS doesn't advertise it.
    #[tokio::test]
    async fn setup_container_netns_failure_falls_back_to_host_network() {
        let runner = Arc::new(RecordingRunner::new());
        let runtime = make_runtime(runner.clone());

        let id = cid("web", 7);
        let ip = IpAddr::V4(Ipv4Addr::new(10, 200, 0, 99));
        runtime.record_container_ip(&id, ip).await;

        // Make the veth create fail. The setup flow pre-deletes the host
        // iface before `link add`, so we only wire the failure on `add`.
        let slug = "web-rep-7";
        let host_iface = make_interface_name(&[slug], "h");
        let cont_iface = make_interface_name(&[slug], "c");
        runner.set_response(
            "ip",
            &[
                "link",
                "add",
                &host_iface,
                "type",
                "veth",
                "peer",
                "name",
                &cont_iface,
            ],
            fake_output_err("RTNETLINK answers: Operation not permitted", 2),
        );

        runtime.setup_container_netns(&id).await;

        assert!(matches!(
            runtime.netns.read().await.get(&id),
            Some(NetnsState::HostFallback)
        ));
        assert_eq!(runtime.get_container_ip(&id).await.unwrap(), None);
    }

    /// J-3: teardown deletes both the host veth and the netns after a
    /// successful configuration.
    #[tokio::test]
    async fn teardown_container_netns_deletes_veth_and_ns() {
        let runner = Arc::new(RecordingRunner::new());
        let calls = runner.calls_handle();
        let runtime = make_runtime(runner.clone());

        let id = cid("web", 0);
        let ip = IpAddr::V4(Ipv4Addr::new(10, 200, 0, 42));
        let gw = IpAddr::V4(Ipv4Addr::new(10, 200, 0, 1));
        runtime.record_container_ip(&id, ip).await;
        runtime.record_container_gateway(&id, gw).await;
        runtime.setup_container_netns(&id).await;

        // Reset the call log so we only inspect teardown commands.
        calls.lock().unwrap().clear();

        runtime.teardown_container_netns(&id).await;

        let recorded = calls.lock().unwrap().clone();
        let joined: Vec<String> = recorded
            .iter()
            .map(|(c, a)| format!("{} {}", c, a.join(" ")))
            .collect();
        assert!(
            joined.iter().any(|c| c.starts_with("ip link delete")),
            "teardown must delete the host veth, got {joined:?}"
        );
        assert!(
            joined.iter().any(|c| c == "ip netns delete web-rep-0"),
            "teardown must delete the netns, got {joined:?}"
        );
        assert!(runtime.netns.read().await.get(&id).is_none());
    }

    /// J-3: netns setup tolerates an already-existing namespace so a
    /// crash-restart cycle can re-enter the path without failing.
    #[tokio::test]
    async fn setup_container_netns_idempotent_on_existing_ns() {
        let runner = Arc::new(RecordingRunner::new());
        let runtime = make_runtime(runner.clone());

        let id = cid("web", 3);
        let ip = IpAddr::V4(Ipv4Addr::new(10, 200, 0, 42));
        runtime.record_container_ip(&id, ip).await;

        runner.set_response(
            "ip",
            &["netns", "add", "web-rep-3"],
            fake_output_err(
                "Cannot create namespace file \"/run/netns/web-rep-3\": File exists",
                1,
            ),
        );

        runtime.setup_container_netns(&id).await;

        assert!(matches!(
            runtime.netns.read().await.get(&id),
            Some(NetnsState::Configured { .. })
        ));
    }
}
