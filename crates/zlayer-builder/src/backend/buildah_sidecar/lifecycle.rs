//! Sidecar process lifecycle: spawn, handshake, mTLS dialing, teardown.
//!
//! The lifecycle manager keeps at most one live `zlayer-buildd` instance
//! alive per `BuildahSidecarBackend`. On the first call to
//! [`SidecarLifecycle::ensure`] we either:
//!
//! * dial a pre-existing remote sidecar named by `SidecarConfig::addr`,
//!   or
//! * discover the local `zlayer-buildd` binary, spawn it bound to
//!   `127.0.0.1:0`, read its `LISTENING host:port` handshake from
//!   stdout, and dial the reported address.
//!
//! The connected [`Channel`] is stored alongside the spawned `Child`
//! (when applicable). The `Child` is held in an `Arc<ChildHolder>` so
//! its `Drop` impl is what actually tears the process down (SIGTERM,
//! 5s grace, then SIGKILL). Cloning a [`LiveSidecar`] therefore shares
//! the same process — teardown happens when the last clone is dropped.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Endpoint, Identity};

use crate::backend::buildah_sidecar::discover::{self, Discovery};
use crate::backend::buildah_sidecar::proto::build_service_client::BuildServiceClient;
use crate::backend::buildah_sidecar::tls::{ensure_tls_material, TlsMaterial};
use crate::error::{BuildError, Result};

/// Prefix the sidecar prints to stdout exactly once on startup.
const HANDSHAKE_PREFIX: &str = "LISTENING ";

/// How long to wait for `LISTENING host:port` after spawning the sidecar.
const SPAWN_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);

/// How long to wait for SIGTERM to take effect before escalating to SIGKILL.
const SIGTERM_GRACE: Duration = Duration::from_secs(5);

/// Holds the `Child` so its `Drop` impl performs graceful teardown.
///
/// Kept private to this module — outside callers interact with
/// [`LiveSidecar`], which wraps this holder in an `Arc` so multiple
/// clones share one process and only the last drop terminates it.
#[derive(Debug)]
struct ChildHolder {
    child: std::sync::Mutex<Option<Child>>,
}

impl ChildHolder {
    fn new(child: Child) -> Self {
        Self {
            child: std::sync::Mutex::new(Some(child)),
        }
    }
}

impl Drop for ChildHolder {
    fn drop(&mut self) {
        // Grab the inner Child. If it was already taken by manual
        // shutdown, nothing to do.
        let mut guard = match self.child.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        let Some(mut child) = guard.take() else {
            return;
        };

        // Try graceful shutdown via SIGTERM on Unix.
        #[cfg(unix)]
        {
            use nix::sys::signal::{kill, Signal};
            use nix::unistd::Pid;

            // Child::id() returns u32 (the PID). Real PIDs always fit
            // in a non-negative i32 (PID_MAX is well below i32::MAX),
            // so an overflow here would indicate either kernel
            // misbehavior or a wrap that already invalidates the
            // signal target — fall through to try_wait / kill in that
            // case instead of panicking.
            if let Ok(raw) = i32::try_from(child.id()) {
                let pid = Pid::from_raw(raw);
                let _ = kill(pid, Signal::SIGTERM);
            }
        }

        // On Windows there is no SIGTERM; fall straight through to
        // try_wait + kill below.
        let deadline = Instant::now() + SIGTERM_GRACE;
        loop {
            match child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) => {
                    if Instant::now() >= deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(_) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return;
                }
            }
        }
    }
}

/// A live `zlayer-buildd` plus the gRPC channel pointing at it.
///
/// The held `Arc<ChildHolder>` is the lifetime anchor for the spawned
/// process; when all `LiveSidecar` clones are dropped the underlying
/// `Child` is SIGTERM'd (5s grace) and then SIGKILL'd. For
/// remote-sidecar mode (`SidecarConfig::addr = Some(_)`) the child
/// field is `None` because the operator owns lifecycle.
#[derive(Debug, Clone)]
pub struct LiveSidecar {
    /// The `host:port` we connected to. For locally-spawned sidecars
    /// this is always `127.0.0.1:<auto-port>`.
    pub addr: String,
    /// Paths to the mTLS material in use.
    pub tls: TlsMaterial,
    /// The binary we spawned (empty `PathBuf` when `addr` was supplied
    /// and we did not spawn anything).
    pub binary: PathBuf,
    /// Connected channel ready to issue RPCs.
    pub channel: Channel,
    /// Holds the spawned child (when present); `Drop` performs
    /// teardown. `None` when we dialed a remote sidecar.
    _child: Option<Arc<ChildHolder>>,
}

impl LiveSidecar {
    /// Build a fresh `BuildServiceClient` over the shared channel.
    #[must_use]
    pub fn client(&self) -> BuildServiceClient<Channel> {
        BuildServiceClient::new(self.channel.clone())
    }
}

/// Lifecycle manager owned by `BuildahSidecarBackend`.
///
/// Holds at most one `LiveSidecar` and lazily produces it on the first
/// call to [`Self::ensure`]. Cheap to clone (the inner `Arc` shares the
/// underlying state), so the backend can pass `&self` references into
/// async build pipelines without ceremony.
#[derive(Debug)]
pub struct SidecarLifecycle {
    config: Arc<zlayer_types::builder::SidecarConfig>,
    state: Mutex<Option<LiveSidecar>>,
}

impl SidecarLifecycle {
    /// Construct a manager bound to `config`. No filesystem or network
    /// I/O happens here.
    #[must_use]
    pub fn new(config: Arc<zlayer_types::builder::SidecarConfig>) -> Self {
        Self {
            config,
            state: Mutex::new(None),
        }
    }

    /// Return the cached [`LiveSidecar`], spawning + dialing on first
    /// call.
    ///
    /// # Errors
    ///
    /// Returns whatever the underlying spawn / handshake / dial flow
    /// produced — typically [`BuildError::NotSupported`] when the
    /// binary is missing or the handshake times out.
    pub async fn ensure(&self) -> Result<LiveSidecar> {
        let mut guard = self.state.lock().await;

        if let Some(existing) = guard.as_ref() {
            return Ok(existing.clone());
        }

        let live = self.spawn_and_dial().await?;
        *guard = Some(live.clone());
        Ok(live)
    }

    /// Drop the cached [`LiveSidecar`] so the next call to
    /// [`Self::ensure`] performs a fresh spawn + dial. Tears down the
    /// previous child via its `Drop` impl once the last outstanding
    /// clone is released.
    pub async fn drop_connection(&self) {
        let mut guard = self.state.lock().await;
        *guard = None;
    }

    async fn spawn_and_dial(&self) -> Result<LiveSidecar> {
        let tls_dir = self.config.tls_dir.clone().unwrap_or_else(default_tls_dir);

        let tls = ensure_tls_material(&tls_dir)?;

        // Remote-sidecar branch: caller pre-configured a reachable
        // address, so we never spawn.
        if let Some(addr) = self.config.addr.clone() {
            let channel = dial_mtls(&addr, &tls).await?;
            return Ok(LiveSidecar {
                addr,
                tls,
                binary: PathBuf::new(),
                channel,
                _child: None,
            });
        }

        // Local spawn branch.
        let Discovery { binary, tried } = discover::discover_default()?;
        tracing::info!(?binary, ?tried, "spawning zlayer-buildd");

        let mut cmd = Command::new(&binary);
        cmd.arg("--bind").arg("127.0.0.1:0");
        cmd.arg("--tls-ca").arg(&tls.ca_pem);
        cmd.arg("--tls-cert").arg(&tls.cert_pem);
        cmd.arg("--tls-key").arg(&tls.key_pem);
        cmd.arg("--idle-secs")
            .arg(self.config.idle_secs.to_string());
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let mut child = cmd.spawn().map_err(|e| BuildError::NotSupported {
            operation: format!("spawning zlayer-buildd at {}: {e}", binary.display()),
        })?;

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| BuildError::NotSupported {
                operation: "zlayer-buildd: missing stdout pipe".into(),
            })?;

        // Read the handshake line on a dedicated blocking thread. We
        // can't park the async runtime on a synchronous
        // `read_line` against a `std::process::Child`'s stdout, and
        // the runtime-free thread keeps the code portable across
        // tokio current_thread / multi_thread schedulers.
        let (tx, rx) = std::sync::mpsc::channel::<Result<String>>();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            let send_result = match reader.read_line(&mut line) {
                Ok(0) => tx.send(Err(BuildError::NotSupported {
                    operation: "zlayer-buildd exited before printing LISTENING".into(),
                })),
                Ok(_) => {
                    let trimmed = line
                        .trim_end_matches('\n')
                        .trim_end_matches('\r')
                        .to_string();
                    if let Some(addr) = trimmed.strip_prefix(HANDSHAKE_PREFIX) {
                        tx.send(Ok(addr.to_string()))
                    } else {
                        tx.send(Err(BuildError::NotSupported {
                            operation: format!("zlayer-buildd handshake malformed: {trimmed:?}"),
                        }))
                    }
                }
                Err(e) => tx.send(Err(BuildError::NotSupported {
                    operation: format!("reading zlayer-buildd stdout: {e}"),
                })),
            };
            // If the receiver has already gone away (e.g. timeout
            // killed the child), there's nothing we can do.
            let _ = send_result;
        });

        let addr_string = match rx.recv_timeout(SPAWN_HANDSHAKE_TIMEOUT) {
            Ok(Ok(addr)) => addr,
            Ok(Err(e)) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(e);
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(BuildError::NotSupported {
                    operation: format!(
                        "zlayer-buildd did not print LISTENING within {SPAWN_HANDSHAKE_TIMEOUT:?}"
                    ),
                });
            }
        };

        let channel = match dial_mtls(&addr_string, &tls).await {
            Ok(ch) => ch,
            Err(e) => {
                // Dial failed — make sure we don't leak the orphaned
                // child since we haven't yet wrapped it in the
                // `ChildHolder` Drop guard.
                let _ = child.kill();
                let _ = child.wait();
                return Err(e);
            }
        };

        Ok(LiveSidecar {
            addr: addr_string,
            tls,
            binary,
            channel,
            _child: Some(Arc::new(ChildHolder::new(child))),
        })
    }
}

fn default_tls_dir() -> PathBuf {
    zlayer_paths::ZLayerDirs::system_default().buildd()
}

async fn dial_mtls(addr: &str, tls: &TlsMaterial) -> Result<Channel> {
    let ca = std::fs::read(&tls.ca_pem).map_err(BuildError::from)?;
    let cert = std::fs::read(&tls.cert_pem).map_err(BuildError::from)?;
    let key = std::fs::read(&tls.key_pem).map_err(BuildError::from)?;

    let identity = Identity::from_pem(&cert, &key);
    let ca_root = Certificate::from_pem(&ca);

    let tls_config = ClientTlsConfig::new()
        .ca_certificate(ca_root)
        .identity(identity)
        .domain_name("zlayer-buildd");

    let uri = format!("https://{addr}")
        .parse::<tonic::transport::Uri>()
        .map_err(|e| BuildError::NotSupported {
            operation: format!("invalid sidecar address {addr:?}: {e}"),
        })?;

    let endpoint = Endpoint::from(uri)
        .tls_config(tls_config)
        .map_err(|e| BuildError::NotSupported {
            operation: format!("sidecar TLS config: {e}"),
        })?
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(600));

    endpoint
        .connect()
        .await
        .map_err(|e| BuildError::NotSupported {
            operation: format!("dialing sidecar at {addr}: {e}"),
        })
}

#[cfg(test)]
#[allow(unsafe_code)]
mod tests {
    use super::*;
    use std::sync::Arc;

    // The env-lock guard *must* be held across the `.await` below so
    // that no other test races us on `PATH`/`ZLAYER_BUILDD_BIN`/
    // `ZLAYER_DATA_DIR` while we have them clobbered. The blocking
    // mutex is the right tool — switching to an async mutex would
    // make every other env-mutating sync test (in discover.rs etc.)
    // unusable.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn ensure_fails_cleanly_when_binary_missing() {
        // No env override, restricted PATH → discover_default must
        // fail, and ensure() must surface that as an error rather
        // than hanging.
        let _g = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let prev_path = std::env::var_os("PATH");
        let prev_buildd_bin = std::env::var_os("ZLAYER_BUILDD_BIN");
        let prev_data_dir = std::env::var_os("ZLAYER_DATA_DIR");

        let tmp = tempfile::tempdir().unwrap();
        // SAFETY: env mutation serialized by `TEST_ENV_LOCK`.
        unsafe {
            std::env::remove_var("ZLAYER_BUILDD_BIN");
            std::env::set_var("PATH", "/nonexistent-zlayer-test-dir");
            std::env::set_var("ZLAYER_DATA_DIR", tmp.path());
        }

        let cfg = Arc::new(zlayer_types::builder::SidecarConfig {
            addr: None,
            tls_dir: Some(tmp.path().to_path_buf()),
            idle_secs: 30,
        });
        let lifecycle = SidecarLifecycle::new(cfg);
        let result = lifecycle.ensure().await;

        // SAFETY: env mutation serialized by `TEST_ENV_LOCK`.
        unsafe {
            match prev_path {
                Some(v) => std::env::set_var("PATH", v),
                None => std::env::remove_var("PATH"),
            }
            match prev_buildd_bin {
                Some(v) => std::env::set_var("ZLAYER_BUILDD_BIN", v),
                None => std::env::remove_var("ZLAYER_BUILDD_BIN"),
            }
            match prev_data_dir {
                Some(v) => std::env::set_var("ZLAYER_DATA_DIR", v),
                None => std::env::remove_var("ZLAYER_DATA_DIR"),
            }
        }

        let err = result.expect_err("ensure() should fail when binary cannot be discovered");
        let msg = err.to_string();
        assert!(
            msg.contains("zlayer-buildd") || msg.contains("not found"),
            "error should mention the missing binary: {msg}"
        );
    }

    /// End-to-end spawn smoke. Builds the Go sidecar once with
    /// `make build` in `bin/zlayer-buildd`, then sets
    /// `ZLAYER_BUILDD_BIN` and runs this test with `--ignored`.
    #[tokio::test]
    #[ignore = "requires zlayer-buildd binary; gate with ZLAYER_BUILDD_BIN"]
    #[allow(clippy::await_holding_lock)]
    async fn spawn_smoke() {
        let _g = crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = tempfile::tempdir().unwrap();
        let cfg = Arc::new(zlayer_types::builder::SidecarConfig {
            addr: None,
            tls_dir: Some(tmp.path().to_path_buf()),
            idle_secs: 30,
        });
        let lifecycle = SidecarLifecycle::new(cfg);
        let live = lifecycle
            .ensure()
            .await
            .expect("sidecar should spawn and handshake");
        assert!(
            live.addr.starts_with("127.0.0.1:"),
            "expected loopback addr, got {}",
            live.addr
        );
        // Build a client; we don't issue an RPC because the proto
        // server-side handlers may not exist yet — the dial alone
        // proves we got past the handshake and TLS negotiation.
        let _client = live.client();
    }
}
