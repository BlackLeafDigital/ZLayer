//! WSL2 delegate runtime that executes Linux containers inside a configurable
//! WSL2 distro via shell-out to `zlayer runtime <verb>`.
//!
//! Used by [`super::composite::CompositeRuntime`] on Windows hosts to handle
//! Linux-image services alongside HCS-managed Windows-image services. There is
//! **no in-distro daemon**, **no HTTP server**, and **no new crate**: every
//! [`Runtime`] trait method maps to one or more `wsl.exe -d <distro> -- ...`
//! invocations, where `<distro>` and the in-distro `zlayer` binary path are
//! both resolved from [`Wsl2DelegateConfig`] at construction time rather than
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
//! * **Config-driven distro + runtime binary.** [`Wsl2DelegateConfig`] carries
//!   `distro`, `runtime_binary` (optional — defaults to
//!   `/usr/local/bin/zlayer`), `bundle_root`, `log_root`, and
//!   `oci_state_root`. [`Wsl2DelegateRuntime::try_new`] preserves the old
//!   `Ok(None)` "no WSL" contract; the explicit
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
//! the form `zlayer runtime <subcommand> failed (status <code>): <stderr>` so the
//! user sees both the command that was run and the distro's stderr.
//! Configuration errors (missing zlayer binary, bad path override) surface as
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
use oci_spec::runtime::{
    LinuxDevice, LinuxDeviceBuilder, LinuxDeviceType, Mount, MountBuilder, Spec,
};
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;
use tokio::sync::RwLock;
use tokio_stream::wrappers::ReceiverStream;
use zlayer_observability::logs::{LogEntry, LogSource, LogStream};
use zlayer_registry::{CompressionType, ImageConfig};
use zlayer_spec::{GpuSpec, PullPolicy, RegistryAuth, ServiceSpec};

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

/// Default in-distro path to the `zlayer` binary that exposes the
/// runc-compatible `runtime <verb>` surface. Installed by
/// `zlayer_wsl::setup::install_binary` at distro provisioning time.
const DEFAULT_RUNTIME_BINARY: &str = "/usr/local/bin/zlayer";

/// Default in-distro state root for `zlayer runtime`. Matches the
/// `RuntimeGlobal::state_root` default in `bin/zlayer/src/cli.rs` so an
/// operator can poke at containers from a shell inside the distro without
/// having to pass `--state-root` explicitly.
const DEFAULT_OCI_STATE_ROOT: &str = "/var/lib/zlayer/oci/state";

/// Configuration for [`Wsl2DelegateRuntime`].
///
/// Lets callers override the distro name and the in-distro `zlayer` runtime
/// binary location so this delegate isn't locked to the hardcoded `zlayer`
/// distro or `/usr/local/bin/zlayer`. `try_new` validates each field by
/// shelling out to the distro before returning a runtime, so a misconfigured
/// `runtime_binary` or a missing distro fails fast with an actionable error.
#[derive(Clone, Debug)]
pub struct Wsl2DelegateConfig {
    /// Name of the WSL2 distro to dispatch container operations into.
    /// Defaults to [`zlayer_wsl::distro::DISTRO_NAME`] (`"zlayer"`).
    pub distro: String,
    /// Absolute in-distro path to the `zlayer` binary that exposes the
    /// `runtime <verb>` surface. When `None`, defaults to
    /// [`DEFAULT_RUNTIME_BINARY`] (`/usr/local/bin/zlayer`) — the location
    /// `zlayer_wsl::setup::install_binary` writes to. Explicit overrides are
    /// honoured verbatim and verified at `try_new` time by running
    /// `<binary> runtime --help` inside the distro.
    pub runtime_binary: Option<String>,
    /// In-distro directory where per-container OCI bundles are materialized
    /// under `<bundle_root>/<container-slug>/`. Defaults to
    /// [`DEFAULT_BUNDLE_ROOT`] (`/var/lib/zlayer/bundles`).
    pub bundle_root: String,
    /// In-distro directory where the per-container log files written by
    /// `zlayer runtime create --log <path>` live; each container gets
    /// `<log_root>/<slug>.youki.log`. Defaults to [`DEFAULT_LOG_ROOT`]
    /// (`/var/lib/zlayer/logs`).
    pub log_root: String,
    /// In-distro state root threaded into every `zlayer runtime --state-root <p>
    /// <verb>` invocation. Defaults to [`DEFAULT_OCI_STATE_ROOT`]
    /// (`/var/lib/zlayer/oci/state`) so it matches the
    /// `RuntimeGlobal::state_root` default in `bin/zlayer/src/cli.rs` and
    /// shells inside the distro can
    /// drop the flag.
    pub oci_state_root: PathBuf,
}

impl Default for Wsl2DelegateConfig {
    fn default() -> Self {
        Self {
            distro: zlayer_wsl::distro::DISTRO_NAME.to_string(),
            runtime_binary: Some(DEFAULT_RUNTIME_BINARY.to_string()),
            bundle_root: DEFAULT_BUNDLE_ROOT.to_string(),
            log_root: DEFAULT_LOG_ROOT.to_string(),
            oci_state_root: PathBuf::from(DEFAULT_OCI_STATE_ROOT),
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

/// `Runtime` implementation that shells out to `zlayer runtime <verb>` inside
/// the `zlayer` WSL2 distro.
///
/// Construct via [`Wsl2DelegateRuntime::try_new`], which returns `Ok(None)`
/// when WSL2 (or the helper distro) is not available — callers should treat
/// that as "no Linux-container support on this node" rather than an error.
pub struct Wsl2DelegateRuntime {
    /// Resolved runtime configuration: distro name, in-distro `zlayer` binary
    /// path, bundle root, log root, OCI state root. Fully populated by
    /// [`Wsl2DelegateRuntime::try_new`] — in particular `runtime_binary` is
    /// the final absolute path (defaulted to [`DEFAULT_RUNTIME_BINARY`] when
    /// the caller left it `None`).
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
    /// `self.zlayer_runtime()` so they can target the configured distro name;
    /// the runner only covers the netns commands (`ip netns`, `ip link`, …).
    runner: Arc<dyn WslRunner>,
}

/// Internal helper that replaces every `Option` in [`Wsl2DelegateConfig`] with
/// a concrete value. Populated by [`Wsl2DelegateRuntime::try_new`] once every
/// field has been validated against the live distro.
#[derive(Clone, Debug)]
struct ResolvedConfig {
    distro: String,
    runtime_binary: String,
    bundle_root: String,
    log_root: String,
    oci_state_root: PathBuf,
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
    /// a non-default distro name, runtime binary, bundle root, log root, or
    /// OCI state root.
    ///
    /// `runtime_binary` resolution rules:
    /// 1. If `config.runtime_binary` is `Some(path)`, use that absolute path.
    /// 2. If `config.runtime_binary` is `None`, use
    ///    [`DEFAULT_RUNTIME_BINARY`] (`/usr/local/bin/zlayer`) — the location
    ///    `zlayer_wsl::setup::install_binary` writes to.
    ///
    /// The resolved binary is then sanity-checked by running
    /// `<binary> runtime --help` inside the distro; failure surfaces as a
    /// hard [`AgentError::Configuration`] so a stale Windows-arch binary or
    /// a build without the `youki-runtime` feature is caught at boot rather
    /// than at first dispatch.
    ///
    /// # Errors
    ///
    /// Returns `Err(AgentError::Configuration(_))` when the resolved
    /// `runtime_binary` does not expose the `zlayer runtime` subcommand
    /// surface inside the distro — a misconfiguration that warrants a hard
    /// fail rather than silently disabling Linux support.
    #[allow(clippy::too_many_lines)]
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

        // 3. Resolve the in-distro `zlayer` runtime binary. Either honour the
        //    caller's override or fall back to `/usr/local/bin/zlayer`, then
        //    sanity-check by running `<binary> runtime --help` so a stale
        //    Windows-arch binary or one built without the `youki-runtime`
        //    feature is caught up front rather than at first dispatch.
        let runtime_binary = config
            .runtime_binary
            .clone()
            .unwrap_or_else(|| DEFAULT_RUNTIME_BINARY.to_string());
        match wsl_exec_in(&config.distro, &runtime_binary, &["runtime", "--help"]).await {
            Ok(out) if out.status.success() => {}
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                return Err(AgentError::Configuration(format!(
                    "zlayer binary at '{runtime_binary}' in distro '{}' does not expose \
                     the `runtime` subcommand (status {:?}): {}",
                    config.distro,
                    out.status.code(),
                    stderr.trim(),
                )));
            }
            Err(e) => {
                return Err(AgentError::Configuration(format!(
                    "zlayer binary at '{runtime_binary}' in distro '{}' does not expose \
                     the `runtime` subcommand: {e}",
                    config.distro,
                )));
            }
        }

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
                runtime_binary,
                bundle_root: config.bundle_root,
                log_root: config.log_root,
                oci_state_root: config.oci_state_root,
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

    /// Run a `zlayer runtime <verb>` subcommand inside the configured distro.
    /// Thin wrapper around [`wsl_exec_in`] that prepends the resolved
    /// runtime binary path plus the canonical `runtime --state-root <root>`
    /// prefix so callers don't have to repeat any of that boilerplate.
    async fn zlayer_runtime(&self, args: &[&str]) -> Result<Output> {
        // Hold the borrow on an owned String so the &str produced by
        // `to_string_lossy` outlives the &[&str] we hand to `wsl_exec_in`.
        let state_root_owned = self.config.oci_state_root.to_string_lossy().into_owned();
        let mut full_args: Vec<&str> = Vec::with_capacity(args.len() + 3);
        full_args.push("runtime");
        full_args.push("--state-root");
        full_args.push(state_root_owned.as_str());
        full_args.extend(args.iter().copied());
        wsl_exec_in(&self.config.distro, &self.config.runtime_binary, &full_args).await
    }

    /// Run an arbitrary binary inside the configured distro. Used for the
    /// handful of call sites that need `mkdir`, `rm`, `tail`, etc. rather
    /// than the `zlayer runtime` surface itself.
    async fn wsl(&self, cmd: &str, args: &[&str]) -> Result<Output> {
        wsl_exec_in(&self.config.distro, cmd, args).await
    }

    /// Execute `cmd args...` inside the distro via the configured [`WslRunner`].
    ///
    /// Separate from [`Self::wsl`] so unit tests for the J-3 netns plumbing
    /// can swap in a recording runner without having to stub the whole
    /// `wsl.exe` surface. Production code paths (G-2/G-3/G-4/G-5) continue
    /// to go through [`Self::wsl`] / [`Self::zlayer_runtime`] so they honour
    /// the configured distro + runtime binary.
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

    /// Query `zlayer runtime state` for the given container and parse its JSON.
    async fn query_state(&self, id: &ContainerId) -> Result<YoukiState> {
        let slug = id_slug(id);
        let output = self.zlayer_runtime(&["state", &slug]).await?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // The runtime emits a consistent "not found" message when the
            // container id is unknown; translate to NotFound so callers get
            // the conventional 404 surface from the API layer.
            if stderr.to_ascii_lowercase().contains("does not exist")
                || stderr.to_ascii_lowercase().contains("not found")
            {
                return Err(AgentError::NotFound {
                    container: id.to_string(),
                    reason: format!("zlayer runtime state reports container unknown: {stderr}"),
                });
            }
            return Err(zlayer_runtime_error("state", &output));
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
        self.pull_image_with_policy(
            image,
            PullPolicy::IfNotPresent,
            None,
            zlayer_spec::SourcePolicy::default(),
        )
        .await
    }

    async fn pull_image_with_policy(
        &self,
        image: &str,
        policy: PullPolicy,
        auth: Option<&RegistryAuth>,
        _source: zlayer_spec::SourcePolicy,
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
            // Honor ~/.docker/config.json (AuthConfig default = DockerConfig) so
            // `zlayer login` creds / Docker Hub auth apply instead of anonymous.
            None => {
                zlayer_core::AuthResolver::new(zlayer_core::AuthConfig::default()).resolve(image)
            }
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

        let layers = puller
            .pull_image_with_policy(image, &registry_auth, policy)
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

    #[allow(clippy::too_many_lines)]
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
        let image_name = spec.image.name.to_string();
        let cached = if let Some(c) = self.image_cache.read().await.get(&image_name).cloned() {
            c
        } else {
            self.pull_image_with_policy(
                &image_name,
                spec.image.pull_policy,
                None,
                spec.image.source_policy.unwrap_or_default(),
            )
            .await?;
            self.image_cache
                .read()
                .await
                .get(&image_name)
                .cloned()
                .ok_or_else(|| AgentError::CreateFailed {
                    id: id.to_string(),
                    reason: format!(
                        "image {} missing from WSL2 delegate cache after pull",
                        spec.image.name
                    ),
                })?
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
        let mut oci_spec = builder
            .build_spec_only(id, spec, &HashMap::new())
            .await
            .map_err(|e| AgentError::CreateFailed {
                id: id.to_string(),
                reason: format!("failed to build OCI spec on Windows host: {e}"),
            })?;

        // Phase 5.D: wire `/dev/dxg` + WSLg lib mounts into the bundle when
        // the service spec requests a GPU. No-op for CPU-only workloads.
        // A missing `/dev/dxg` is a hard error here; we don't want a silent
        // CPU fallback for users who explicitly asked for GPU.
        let gpu_probe = DefaultWslGpuHostProbe { runtime: self };
        apply_wsl_gpu_to_spec(&mut oci_spec, spec, &gpu_probe).await?;

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

        // 6. Hand the bundle to `zlayer runtime create`. `--log <path>` points
        //    at the per-container log file inside the distro so `container_logs`
        //    has something to tail — without this flag the runtime just
        //    writes to stderr which we can't easily read back later. A
        //    non-zero exit here rolls back the bundle directory so a retry
        //    sees a clean slate.
        let log_path = self.log_path(id);
        let create = self
            .zlayer_runtime(&["create", &slug, "--bundle", &bundle_dir, "--log", &log_path])
            .await?;
        if !create.status.success() {
            let stderr = String::from_utf8_lossy(&create.stderr).trim().to_string();
            // Best-effort cleanup: we don't care if it succeeds.
            let _ = self.wsl("rm", &["-rf", &bundle_dir]).await;
            return Err(AgentError::CreateFailed {
                id: id.to_string(),
                reason: format!(
                    "zlayer runtime create failed (status {:?}): {stderr}",
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
        let output = self.zlayer_runtime(&["start", &slug]).await?;
        if !output.status.success() {
            // zlayer runtime start failed; clean up the netns we just configured
            // so we don't leak it on a start that never took effect.
            self.teardown_container_netns(id).await;
            return Err(AgentError::StartFailed {
                id: id.to_string(),
                reason: format!(
                    "zlayer runtime start failed (status {:?}): {}",
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
        let term = self
            .zlayer_runtime(&["kill", "--all", &slug, "SIGTERM"])
            .await?;
        if !term.status.success() {
            // If the container is already stopped the runtime returns
            // nonzero; treat that as success for stop semantics rather than
            // erroring.
            let stderr = String::from_utf8_lossy(&term.stderr).to_ascii_lowercase();
            if !(stderr.contains("stopped") || stderr.contains("not running")) {
                return Err(zlayer_runtime_error("kill", &term));
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
        let kill = self
            .zlayer_runtime(&["kill", "--all", &slug, "SIGKILL"])
            .await?;
        if !kill.status.success() {
            let stderr = String::from_utf8_lossy(&kill.stderr).to_ascii_lowercase();
            if !(stderr.contains("stopped") || stderr.contains("not running")) {
                return Err(zlayer_runtime_error("kill", &kill));
            }
        }
        Ok(())
    }

    async fn remove_container(&self, id: &ContainerId) -> Result<()> {
        let slug = id_slug(id);
        let output = self.zlayer_runtime(&["delete", &slug]).await?;
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
            Err(zlayer_runtime_error("delete", &output))
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
            return Err(zlayer_runtime_error("tail", &output));
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
        let output = self.zlayer_runtime(&args).await?;
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        let exit = output.status.code().unwrap_or(-1);
        Ok((exit, stdout, stderr))
    }

    async fn get_container_stats(&self, id: &ContainerId) -> Result<ContainerStats> {
        let slug = id_slug(id);
        let output = self.zlayer_runtime(&["events", &slug, "--stats"]).await;
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
        let output = self.zlayer_runtime(&["kill", &slug, &canonical]).await?;
        if output.status.success() {
            Ok(())
        } else {
            Err(zlayer_runtime_error("kill", &output))
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
    /// Spawns `wsl.exe -d <distro> -- <runtime_binary> oci --state-root
    /// <root> exec <slug> -- <cmd...>` with piped stdout and stderr, then
    /// drives two `BufReader::lines()` loops on background tasks. Each line
    /// becomes one [`ExecEvent::Stdout`] / [`ExecEvent::Stderr`]; once both
    /// readers reach EOF and the child process exits, a final
    /// [`ExecEvent::Exit`] is emitted and the stream closes. Errors surfaced
    /// before the child is spawned (empty cmd, `wsl.exe` not launchable)
    /// flow through the outer `Result`; post-spawn errors are logged and
    /// the stream closes with `ExecEvent::Exit(-1)`.
    async fn exec_stream(&self, id: &ContainerId, cmd: &[String]) -> Result<ExecEventStream> {
        if cmd.is_empty() {
            return Err(AgentError::InvalidSpec(
                "exec command must not be empty".to_string(),
            ));
        }

        let slug = id_slug(id);
        // Build the `wsl.exe` argv:
        //   `-d <distro> -- <runtime_binary> runtime --state-root <root>
        //    exec <slug> -- <user_cmd...>`
        // Everything is owned strings because we hand them off to a background
        // task below and can't rely on the &[&str] borrow outliving the spawn.
        let state_root = self.config.oci_state_root.to_string_lossy().into_owned();
        let mut argv: Vec<String> = Vec::with_capacity(10 + cmd.len());
        argv.push("-d".to_string());
        argv.push(self.config.distro.clone());
        argv.push("--".to_string());
        argv.push(self.config.runtime_binary.clone());
        argv.push("runtime".to_string());
        argv.push("--state-root".to_string());
        argv.push(state_root);
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

/// Build a conventional `AgentError::Network` describing a nonzero
/// `zlayer runtime <subcommand>` exit.
///
/// Format: `zlayer runtime <subcommand> failed (status <code>): <stderr>`.
/// Never swallows stderr so the user sees both the command and the distro's
/// own diagnostic output.
fn zlayer_runtime_error(subcommand: &str, output: &Output) -> AgentError {
    let status = output.status.code();
    let stderr = String::from_utf8_lossy(&output.stderr);
    AgentError::Network(format!(
        "zlayer runtime {subcommand} failed (status {status:?}): {}",
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

/// Parse the JSON payload emitted by `zlayer runtime state <id>`.
fn parse_youki_state(output: &Output) -> Result<YoukiState> {
    let stdout = std::str::from_utf8(&output.stdout).map_err(|e| {
        AgentError::Internal(format!("zlayer runtime state: stdout not utf-8: {e}"))
    })?;
    serde_json::from_str::<YoukiState>(stdout.trim()).map_err(|e| {
        AgentError::Internal(format!(
            "zlayer runtime state: failed to parse JSON: {e} (raw: {:?})",
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
// WSL2 GPU exposure (Phase 5.D)
//
// When a service spec carries `resources.gpu`, the youki bundle running inside
// the WSL2 distro needs three things mirrored from how WSLg exposes GPU to a
// regular user shell:
//
//   1. `/dev/dxg`            — the WSL DirectX kernel interface, the actual
//                              entry point apps talk to for GPU work.
//   2. `/usr/lib/wsl/`       — the WSLg shim library tree (`libdxcore.so`,
//                              `libd3d12.so`, `libdxguid.so`).
//   3. `/usr/lib/wsl/drivers/` (NVIDIA-only) — the NVIDIA WDDM driver shim that
//                              CUDA libraries resolve through.
//
// On the host (Windows) we can't touch any of these directly; everything
// lives inside the WSL2 distro. So the probe trait below shells out to
// `wsl.exe -d <distro> -- ...` to stat `/dev/dxg` and check the lib trees.
// The pure mutation helper [`inject_wsl_gpu_mounts`] takes the probe's
// answers as inputs so it can be unit-tested without any real WSL2 host.
// ---------------------------------------------------------------------------

/// Result of probing the WSL2 distro for GPU readiness.
///
/// Populated by an impl of [`WslGpuHostProbe`] before
/// [`inject_wsl_gpu_mounts`] runs so the mutation helper stays purely
/// in-memory and testable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WslGpuHostState {
    /// `(major, minor)` for `/dev/dxg` inside the distro, or `None` if the
    /// device node is absent. `None` is a hard error when GPU was requested:
    /// silently falling back to CPU is surprising.
    pub dxg_devno: Option<(i64, i64)>,
    /// Whether `/usr/lib/wsl` is present inside the distro. When false the
    /// shim libraries are missing and GPU work will fail at link time even
    /// with `/dev/dxg` mounted.
    pub wsl_lib_present: bool,
    /// Whether `/usr/lib/wsl/drivers` is present inside the distro. This is
    /// the NVIDIA-specific driver shim path; absent on AMD/Intel hosts.
    pub wsl_drivers_present: bool,
}

/// Probe trait. Lets unit tests substitute a stub that doesn't need a real
/// WSL2 host with `/dev/dxg` exposed.
#[async_trait]
pub(crate) trait WslGpuHostProbe: Send + Sync {
    async fn probe(&self) -> Result<WslGpuHostState>;
}

/// Production probe that shells out via [`Wsl2DelegateRuntime::wsl`] to read
/// the actual state of the configured distro.
struct DefaultWslGpuHostProbe<'a> {
    runtime: &'a Wsl2DelegateRuntime,
}

#[async_trait]
impl WslGpuHostProbe for DefaultWslGpuHostProbe<'_> {
    async fn probe(&self) -> Result<WslGpuHostState> {
        // `stat -c '%t %T' /dev/dxg` prints major/minor as hex. We swallow
        // stat's nonzero exit (file missing) and report devno=None, which
        // `inject_wsl_gpu_mounts` translates into `WslGpuUnavailable`.
        let dxg_devno = match self.runtime.wsl("stat", &["-c", "%t %T", "/dev/dxg"]).await {
            Ok(output) if output.status.success() => {
                let raw = String::from_utf8_lossy(&output.stdout);
                parse_stat_hex_devno(raw.trim())
            }
            _ => None,
        };

        // `test -d` returns 0 when the directory exists. Both -d probes are
        // best-effort: a failure to spawn `wsl.exe` is the same as the path
        // being absent — we'd fail later anyway.
        let wsl_lib_present = self
            .runtime
            .wsl("test", &["-d", "/usr/lib/wsl"])
            .await
            .map(|o| o.status.success())
            .unwrap_or(false);
        let wsl_drivers_present = self
            .runtime
            .wsl("test", &["-d", "/usr/lib/wsl/drivers"])
            .await
            .map(|o| o.status.success())
            .unwrap_or(false);

        Ok(WslGpuHostState {
            dxg_devno,
            wsl_lib_present,
            wsl_drivers_present,
        })
    }
}

/// Parse the output of `stat -c '%t %T' <path>`: two whitespace-separated
/// hexadecimal numbers (major, then minor). Returns `None` on any parse
/// failure so callers treat malformed stat output as "device missing".
fn parse_stat_hex_devno(s: &str) -> Option<(i64, i64)> {
    let mut parts = s.split_ascii_whitespace();
    let major = i64::from_str_radix(parts.next()?, 16).ok()?;
    let minor = i64::from_str_radix(parts.next()?, 16).ok()?;
    Some((major, minor))
}

/// Inject `/dev/dxg` + the `WSLg` shim mounts into a bundle's mounts/devices
/// and prepend the `WSLg` lib paths to `LD_LIBRARY_PATH`.
///
/// Returns `Err(AgentError::WslGpuUnavailable)` when GPU was requested but
/// the host cannot deliver it (no `/dev/dxg`). Idempotent w.r.t. existing
/// user-supplied `/dev/dxg` entries: if the spec's device list already names
/// `/dev/dxg`, the helper leaves that entry alone instead of duplicating.
pub(crate) fn inject_wsl_gpu_mounts(
    mounts: &mut Vec<Mount>,
    env: &mut Vec<String>,
    devices: &mut Vec<LinuxDevice>,
    _gpu_spec: &GpuSpec,
    host: &WslGpuHostState,
) -> Result<()> {
    let (major, minor) = host
        .dxg_devno
        .ok_or_else(|| AgentError::WslGpuUnavailable {
            reason: "/dev/dxg is not exposed by the WSL2 kernel inside the configured distro; \
                 enable WSL2 GPU support (Windows 11 + a recent WSL kernel) or drop \
                 `resources.gpu` from the service spec"
                .to_string(),
        })?;

    let has_dxg_mount = mounts
        .iter()
        .any(|m| m.destination().as_path() == std::path::Path::new("/dev/dxg"));
    if !has_dxg_mount {
        let mount = MountBuilder::default()
            .destination("/dev/dxg".to_string())
            .source("/dev/dxg".to_string())
            .typ("bind".to_string())
            .options(vec!["bind".to_string(), "rw".to_string()])
            .build()
            .map_err(|e| AgentError::WslGpuUnavailable {
                reason: format!("failed to build /dev/dxg bind mount: {e}"),
            })?;
        mounts.push(mount);
    }

    let has_dxg_device = devices
        .iter()
        .any(|d| d.path().as_path() == std::path::Path::new("/dev/dxg"));
    if !has_dxg_device {
        let device = LinuxDeviceBuilder::default()
            .path("/dev/dxg")
            .typ(LinuxDeviceType::C)
            .major(major)
            .minor(minor)
            .file_mode(0o666u32)
            .uid(0u32)
            .gid(0u32)
            .build()
            .map_err(|e| AgentError::WslGpuUnavailable {
                reason: format!("failed to build /dev/dxg device node: {e}"),
            })?;
        devices.push(device);
    }

    // /usr/lib/wsl read-only shim mount.
    if host.wsl_lib_present {
        let has_wsl_lib_mount = mounts
            .iter()
            .any(|m| m.destination().as_path() == std::path::Path::new("/usr/lib/wsl"));
        if !has_wsl_lib_mount {
            let mount = MountBuilder::default()
                .destination("/usr/lib/wsl".to_string())
                .source("/usr/lib/wsl".to_string())
                .typ("bind".to_string())
                .options(vec!["bind".to_string(), "ro".to_string()])
                .build()
                .map_err(|e| AgentError::WslGpuUnavailable {
                    reason: format!("failed to build /usr/lib/wsl bind mount: {e}"),
                })?;
            mounts.push(mount);
        }
    } else {
        tracing::warn!(
            "WSL2 GPU: /usr/lib/wsl missing on host; container will see /dev/dxg but no \
             WSLg shim libraries (libdxcore.so etc.). GPU workloads will fail at dlopen."
        );
    }

    // Prepend WSLg lib paths to LD_LIBRARY_PATH (drivers ahead of lib so the
    // NVIDIA driver shim wins lookup ordering). Preserve any existing value.
    let mut new_prefix: Vec<&str> = Vec::new();
    if host.wsl_drivers_present {
        new_prefix.push("/usr/lib/wsl/drivers");
    }
    if host.wsl_lib_present {
        new_prefix.push("/usr/lib/wsl/lib");
    }
    if !new_prefix.is_empty() {
        let prefix_joined = new_prefix.join(":");
        if let Some(entry) = env.iter_mut().find(|e| e.starts_with("LD_LIBRARY_PATH=")) {
            let existing = entry.split_once('=').map_or("", |(_, v)| v).to_string();
            *entry = if existing.is_empty() {
                format!("LD_LIBRARY_PATH={prefix_joined}")
            } else {
                format!("LD_LIBRARY_PATH={prefix_joined}:{existing}")
            };
        } else {
            env.push(format!("LD_LIBRARY_PATH={prefix_joined}"));
        }
    }

    Ok(())
}

/// Mutates the already-built [`Spec`] in place to inject the WSL2 GPU
/// mounts/devices/env when `resources.gpu` is set. No-op when the spec
/// doesn't request a GPU.
async fn apply_wsl_gpu_to_spec(
    oci_spec: &mut Spec,
    service: &ServiceSpec,
    probe: &dyn WslGpuHostProbe,
) -> Result<()> {
    let Some(gpu_spec) = service.resources.gpu.as_ref() else {
        return Ok(());
    };

    let host = probe.probe().await?;

    // Pull the three slices we need to mutate out of the Spec via the getset
    // accessors. Each is wrapped in Option<Vec<...>>; we materialise empty
    // vecs as needed so the helper can push without further branching.
    let mut mounts = oci_spec.mounts().clone().unwrap_or_default();
    let mut env = oci_spec
        .process()
        .as_ref()
        .and_then(|p| p.env().clone())
        .unwrap_or_default();
    let mut devices = oci_spec
        .linux()
        .as_ref()
        .and_then(|l| l.devices().clone())
        .unwrap_or_default();

    inject_wsl_gpu_mounts(&mut mounts, &mut env, &mut devices, gpu_spec, &host)?;

    oci_spec.set_mounts(Some(mounts));
    if let Some(process) = oci_spec.process_mut().as_mut() {
        process.set_env(Some(env));
    }
    if let Some(linux) = oci_spec.linux_mut().as_mut() {
        linux.set_devices(Some(devices));
    }

    Ok(())
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
        ContainerId::new(service, replica)
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
            runtime_binary: DEFAULT_RUNTIME_BINARY.to_string(),
            bundle_root: DEFAULT_BUNDLE_ROOT.to_string(),
            log_root: DEFAULT_LOG_ROOT.to_string(),
            oci_state_root: PathBuf::from(DEFAULT_OCI_STATE_ROOT),
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

    // `#[ignore]`: this test originally piggy-backed on the side effect that
    // `wsl.exe -d zlayer -- mkdir -p …` fails fast on hosts that lack the
    // `zlayer` distro, which kept the call inside `create_container` and
    // satisfied the "not Unsupported" assertion in ~100 ms. Once a real
    // `zlayer` distro exists (production hosts AND any CI runner that has
    // run `setup_distro` once) the mkdir succeeds and execution proceeds
    // through the live registry pull → `wsl_stdin_pipe` tar-extract →
    // `zlayer runtime create` chain, none of which have inline timeouts.
    // The result is a multi-minute hang on a test whose only purpose was
    // a one-liner regression guard against re-adding the old
    // `AgentError::Unsupported` stub.
    //
    // The positive end-to-end case is covered by
    // `crates/zlayer-agent/tests/composite_dispatch_e2e.rs::composite_dispatches_linux_spec_to_wsl2`,
    // which exercises the same dispatch path against a real distro with
    // proper test infrastructure. The stub-regression contract is also
    // statically enforced — `create_container`'s body never constructs
    // `AgentError::Unsupported` — so leaving this `#[ignore]`'d does not
    // weaken the workspace's protection against the regression.
    //
    // The proper unwind here is workstream B8 (mock `ImagePullerLike` +
    // route `create_container`'s wsl calls through the existing
    // `WslRunner` trait) so the test can exercise the dispatch path
    // without any live I/O. Until then: ignored.
    #[ignore = "hangs on hosts with a real `zlayer` WSL distro; see comment + B8"]
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
        // Test-only fixture image: our own GHCR-hosted retag of alpine:3.19.
        // Avoids docker.io rate limits + the public-internet dependency that
        // makes this test flake on hosts where the `zlayer` WSL distro exists
        // (mkdir succeeds → pull fires → 60s+ hang under constrained
        // networking). The pull still hits the wire — see
        // [`crate::runtimes::wsl2_delegate::ImagePullerLike`] (B8) for the
        // proper trait-injected stub that retires this footgun entirely.
        let yaml = r"
version: v1
deployment: wsl2-g2-test
services:
  svc:
    rtype: service
    image:
      name: ghcr.io/blackleafdigital/zlayer/test-fixtures:latest
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
            runtime_binary: DEFAULT_RUNTIME_BINARY.to_string(),
            bundle_root: "/custom/bundles".to_string(),
            log_root: "/custom/logs".to_string(),
            oci_state_root: PathBuf::from(DEFAULT_OCI_STATE_ROOT),
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

    /// [`Wsl2DelegateConfig::default`] must reproduce the documented
    /// defaults so an operator upgrading without overriding any field
    /// silently keeps the same layout. Post-`zlayer runtime` migration the
    /// runtime binary defaults to `/usr/local/bin/zlayer` (rather than
    /// `which youki`) and `oci_state_root` is `/var/lib/zlayer/oci/state`.
    #[test]
    fn default_config_matches_previous_hardcoded_values() {
        let cfg = Wsl2DelegateConfig::default();
        assert_eq!(cfg.distro, zlayer_wsl::distro::DISTRO_NAME);
        assert_eq!(cfg.runtime_binary.as_deref(), Some(DEFAULT_RUNTIME_BINARY));
        assert_eq!(cfg.bundle_root, DEFAULT_BUNDLE_ROOT);
        assert_eq!(cfg.log_root, DEFAULT_LOG_ROOT);
        assert_eq!(cfg.oci_state_root, PathBuf::from(DEFAULT_OCI_STATE_ROOT));
    }

    /// A runtime built from a custom [`Wsl2DelegateConfig`] must propagate
    /// the chosen distro name + runtime binary path into every derived
    /// field so subsequent `wsl.exe` invocations target the right distro.
    /// We can't actually drive `wsl.exe` from unit tests, but we *can*
    /// assert that the runtime's `bundle_dir` / `log_path` / `config`
    /// carry the custom values.
    #[test]
    fn custom_config_propagates_into_runtime_fields() {
        let runtime = test_runtime(ResolvedConfig {
            distro: "ubuntu-lts".to_string(),
            runtime_binary: "/opt/zlayer/bin/zlayer".to_string(),
            bundle_root: "/srv/zlayer/bundles".to_string(),
            log_root: "/srv/zlayer/logs".to_string(),
            oci_state_root: PathBuf::from("/srv/zlayer/oci-state"),
        });
        let id = cid("api", 0);

        assert_eq!(runtime.config.distro, "ubuntu-lts");
        assert_eq!(runtime.config.runtime_binary, "/opt/zlayer/bin/zlayer");
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

    /// `zlayer_runtime` must prefix every argv with `runtime --state-root <root>`
    /// and delegate to the configured runtime binary. We can't actually
    /// run `wsl.exe` from a unit test, but we can spawn the helper and
    /// assert the borrow/argv plumbing compiles + holds the expected
    /// shape by reaching into `wsl_exec_in` indirectly via a fake distro
    /// that will trivially fail on the test host. Instead, lock down the
    /// shape with a pure construction test: assemble the argv the same
    /// way `zlayer_runtime` does and verify it matches expectations.
    #[test]
    fn zlayer_runtime_prefixes_args_with_state_root() {
        let cfg = ResolvedConfig {
            distro: "zlayer".to_string(),
            runtime_binary: DEFAULT_RUNTIME_BINARY.to_string(),
            bundle_root: DEFAULT_BUNDLE_ROOT.to_string(),
            log_root: DEFAULT_LOG_ROOT.to_string(),
            oci_state_root: PathBuf::from("/var/lib/zlayer/oci/state"),
        };

        // Mirror the argv-building logic from `zlayer_runtime`.
        let state_root_owned = cfg.oci_state_root.to_string_lossy().into_owned();
        let user_args: &[&str] = &["create", "id", "--bundle", "/b", "--log", "/l"];
        let mut full_args: Vec<&str> = Vec::with_capacity(user_args.len() + 3);
        full_args.push("runtime");
        full_args.push("--state-root");
        full_args.push(state_root_owned.as_str());
        full_args.extend(user_args.iter().copied());

        assert_eq!(
            full_args,
            vec![
                "runtime",
                "--state-root",
                "/var/lib/zlayer/oci/state",
                "create",
                "id",
                "--bundle",
                "/b",
                "--log",
                "/l",
            ],
            "zlayer_runtime must prefix `runtime --state-root <root>` to its argv",
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

    // -------------------------------------------------------------------
    // Phase 5.D: WSL2 GPU exposure tests.
    //
    // These exercise `inject_wsl_gpu_mounts` against a stub host state so we
    // don't need a real `/dev/dxg` on the Windows CI runner. They cover
    // the four spec'd cases plus the GPU-not-requested no-op path.
    // -------------------------------------------------------------------

    fn test_gpu_spec() -> GpuSpec {
        GpuSpec {
            count: 1,
            vendor: "nvidia".to_string(),
            mode: None,
            model: None,
            scheduling: None,
            distributed: None,
            sharing: None,
            mps_pipe_dir: None,
            mps_log_dir: None,
            time_slice_index: None,
            time_slicing_config_path: None,
        }
    }

    fn happy_host_state() -> WslGpuHostState {
        WslGpuHostState {
            dxg_devno: Some((10, 117)),
            wsl_lib_present: true,
            wsl_drivers_present: true,
        }
    }

    #[test]
    fn inject_wsl_gpu_mounts_adds_dxg_mount() {
        let mut mounts: Vec<Mount> = Vec::new();
        let mut env: Vec<String> = Vec::new();
        let mut devices: Vec<LinuxDevice> = Vec::new();
        let gpu = test_gpu_spec();
        let host = happy_host_state();

        inject_wsl_gpu_mounts(&mut mounts, &mut env, &mut devices, &gpu, &host)
            .expect("inject must succeed on happy host");

        let dxg_mounts: Vec<_> = mounts
            .iter()
            .filter(|m| m.destination().as_path() == std::path::Path::new("/dev/dxg"))
            .collect();
        assert_eq!(
            dxg_mounts.len(),
            1,
            "expected exactly one /dev/dxg mount, got {}: {:?}",
            dxg_mounts.len(),
            mounts
        );
        assert_eq!(
            dxg_mounts[0]
                .source()
                .as_ref()
                .map(std::path::PathBuf::as_path),
            Some(std::path::Path::new("/dev/dxg"))
        );
        assert_eq!(dxg_mounts[0].typ().as_deref(), Some("bind"));

        // Device cgroup entry must be present too with the probed major/minor.
        let dxg_devs: Vec<_> = devices
            .iter()
            .filter(|d| d.path().as_path() == std::path::Path::new("/dev/dxg"))
            .collect();
        assert_eq!(dxg_devs.len(), 1);
        assert_eq!(dxg_devs[0].major(), 10);
        assert_eq!(dxg_devs[0].minor(), 117);
        assert_eq!(dxg_devs[0].typ(), LinuxDeviceType::C);
    }

    #[test]
    fn inject_wsl_gpu_mounts_respects_existing_user_mount() {
        // User already declared a /dev/dxg bind mount via spec.devices; the
        // helper must not double-mount.
        let pre_existing = MountBuilder::default()
            .destination("/dev/dxg".to_string())
            .source("/dev/dxg".to_string())
            .typ("bind".to_string())
            .options(vec!["bind".to_string(), "rw".to_string()])
            .build()
            .unwrap();
        let mut mounts = vec![pre_existing];

        let pre_existing_dev = LinuxDeviceBuilder::default()
            .path("/dev/dxg")
            .typ(LinuxDeviceType::C)
            .major(10)
            .minor(117)
            .file_mode(0o666u32)
            .uid(0u32)
            .gid(0u32)
            .build()
            .unwrap();
        let mut devices = vec![pre_existing_dev];

        let mut env: Vec<String> = Vec::new();
        let gpu = test_gpu_spec();
        let host = happy_host_state();

        inject_wsl_gpu_mounts(&mut mounts, &mut env, &mut devices, &gpu, &host)
            .expect("inject must succeed on happy host");

        assert_eq!(
            mounts
                .iter()
                .filter(|m| m.destination().as_path() == std::path::Path::new("/dev/dxg"))
                .count(),
            1,
            "expected no duplicate /dev/dxg mount, mounts: {mounts:?}"
        );
        assert_eq!(
            devices
                .iter()
                .filter(|d| d.path().as_path() == std::path::Path::new("/dev/dxg"))
                .count(),
            1,
            "expected no duplicate /dev/dxg device, devices: {devices:?}"
        );
    }

    #[test]
    fn inject_wsl_gpu_mounts_prepends_ld_library_path() {
        let mut mounts: Vec<Mount> = Vec::new();
        let mut env: Vec<String> = vec!["LD_LIBRARY_PATH=/foo".to_string()];
        let mut devices: Vec<LinuxDevice> = Vec::new();
        let gpu = test_gpu_spec();
        let host = happy_host_state();

        inject_wsl_gpu_mounts(&mut mounts, &mut env, &mut devices, &gpu, &host)
            .expect("inject must succeed on happy host");

        let ld_entries: Vec<_> = env
            .iter()
            .filter(|e| e.starts_with("LD_LIBRARY_PATH="))
            .collect();
        assert_eq!(
            ld_entries.len(),
            1,
            "exactly one LD_LIBRARY_PATH entry: {env:?}"
        );
        // Drivers ahead of lib (NVIDIA shim wins lookup), original /foo preserved.
        assert_eq!(
            ld_entries[0], "LD_LIBRARY_PATH=/usr/lib/wsl/drivers:/usr/lib/wsl/lib:/foo",
            "unexpected LD_LIBRARY_PATH: {env:?}"
        );
    }

    #[test]
    fn inject_wsl_gpu_mounts_appends_ld_library_path_when_absent() {
        // No existing LD_LIBRARY_PATH — the helper should add one rather than
        // silently dropping the prefix.
        let mut mounts: Vec<Mount> = Vec::new();
        let mut env: Vec<String> = vec!["PATH=/bin".to_string()];
        let mut devices: Vec<LinuxDevice> = Vec::new();
        let gpu = test_gpu_spec();
        let host = happy_host_state();

        inject_wsl_gpu_mounts(&mut mounts, &mut env, &mut devices, &gpu, &host).unwrap();

        assert!(env.contains(&"PATH=/bin".to_string()));
        assert!(env
            .iter()
            .any(|e| e == "LD_LIBRARY_PATH=/usr/lib/wsl/drivers:/usr/lib/wsl/lib"));
    }

    #[test]
    fn inject_wsl_gpu_mounts_fails_when_dxg_missing() {
        let mut mounts: Vec<Mount> = Vec::new();
        let mut env: Vec<String> = Vec::new();
        let mut devices: Vec<LinuxDevice> = Vec::new();
        let gpu = test_gpu_spec();
        let host = WslGpuHostState {
            dxg_devno: None,
            wsl_lib_present: true,
            wsl_drivers_present: true,
        };

        let err = inject_wsl_gpu_mounts(&mut mounts, &mut env, &mut devices, &gpu, &host)
            .expect_err("expected WslGpuUnavailable when /dev/dxg missing");
        assert!(
            matches!(err, AgentError::WslGpuUnavailable { .. }),
            "expected WslGpuUnavailable, got: {err:?}"
        );
        assert!(mounts.is_empty(), "no mount should have been pushed");
        assert!(devices.is_empty(), "no device should have been pushed");
    }

    #[tokio::test]
    async fn apply_wsl_gpu_to_spec_no_op_when_gpu_not_requested() {
        struct ExplodingProbe;
        #[async_trait]
        impl WslGpuHostProbe for ExplodingProbe {
            async fn probe(&self) -> Result<WslGpuHostState> {
                panic!("probe must not run when no GPU is requested");
            }
        }

        // Build a minimal Spec via the OCI default; the apply function should
        // leave it untouched when the service spec doesn't request a GPU.
        let mut oci_spec = Spec::default();
        let mounts_before = oci_spec.mounts().clone();

        // ServiceSpec has no Default impl in zlayer-types; parse a minimal
        // fixture from YAML the same way the spec crate's own tests do.
        let yaml = r"
version: v1
deployment: gpu-noop-test
services:
  cpu-only:
    rtype: service
    image:
      name: alpine:latest
";
        let deployment: zlayer_spec::DeploymentSpec =
            serde_yaml::from_str(yaml).expect("parse fixture deployment");
        let service = deployment
            .services
            .get("cpu-only")
            .cloned()
            .expect("cpu-only service");
        assert!(
            service.resources.gpu.is_none(),
            "fixture must not request GPU"
        );

        apply_wsl_gpu_to_spec(&mut oci_spec, &service, &ExplodingProbe)
            .await
            .expect("no-op should succeed");

        assert_eq!(oci_spec.mounts(), &mounts_before);
        let has_dxg = oci_spec.mounts().as_ref().is_some_and(|ms| {
            ms.iter()
                .any(|m| m.destination().as_path() == std::path::Path::new("/dev/dxg"))
        });
        assert!(
            !has_dxg,
            "no /dev/dxg mount should appear for CPU-only spec"
        );
    }

    #[test]
    fn parse_stat_hex_devno_handles_typical_wsl_output() {
        // `stat -c '%t %T' /dev/dxg` on a real WSL2 distro returns e.g. "a 75".
        assert_eq!(parse_stat_hex_devno("a 75"), Some((10, 117)));
        assert_eq!(parse_stat_hex_devno("  a   75  "), Some((10, 117)));
        assert_eq!(parse_stat_hex_devno(""), None);
        assert_eq!(parse_stat_hex_devno("nothex zz"), None);
        assert_eq!(parse_stat_hex_devno("a"), None);
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
