//! Windows Service event loop (I-1).
//!
//! When `zlayer serve --service` is invoked (normally by the Windows Service
//! Control Manager spawning the binary registered via I-2's `sc create`), the
//! binary hands control to [`run_as_windows_service`] instead of executing the
//! normal foreground `serve()` path. From there we:
//!
//!   1. Register an SCM control handler that accepts `Stop` / `Shutdown`
//!      control codes and flips a `tokio::sync::watch` channel.
//!   2. Report `ServiceState::Running` to SCM with `STOP | SHUTDOWN` controls.
//!   3. Build our own tokio runtime (the service entry point is sync) and
//!      block on [`crate::commands::serve::serve_with_external_shutdown`],
//!      passing the watch receiver so the daemon tears down cleanly when SCM
//!      signals a stop.
//!   4. Report `ServiceState::Stopped` with an exit code reflecting whether
//!      the daemon shut down cleanly.
//!
//! Foreground `zlayer serve` (no `--service` flag) is unchanged — it takes
//! the normal path in `main()` and `serve()` runs with the standard Ctrl+C /
//! SIGTERM shutdown handlers.
//!
//! This module is compiled on Windows only; the stub in `daemon_service.rs`
//! (via the `cfg(not(windows))` variant below) lets callers reference the
//! entry point from cross-platform code and surface a clear error on other
//! OSes.

#[cfg(windows)]
#[allow(unused_imports)]
pub use self::imp::{run_as_windows_service, SERVICE_NAME};

#[cfg(not(windows))]
#[allow(unused_imports)]
pub use self::stub::{run_as_windows_service, SERVICE_NAME};

// -----------------------------------------------------------------------
// Non-Windows stub
// -----------------------------------------------------------------------
#[cfg(not(windows))]
mod stub {
    use anyhow::{bail, Result};

    /// Name of the service registered with SCM. Kept in sync with the
    /// Windows implementation so the rest of the codebase can reference this
    /// constant from cross-platform code.
    #[allow(dead_code)]
    pub const SERVICE_NAME: &str = "ZLayerDaemon";

    /// Called when `--service` is set on a non-Windows target. Always errors
    /// out with a helpful message — the flag is only meaningful when SCM
    /// spawns the binary.
    ///
    /// Kept around for API parity with the Windows implementation so
    /// cross-platform callers can link against the same symbol; the actual
    /// `run_service_entry` in `main.rs` short-circuits on non-Windows and
    /// never calls this.
    #[allow(dead_code, clippy::needless_pass_by_value, clippy::too_many_arguments)]
    pub fn run_as_windows_service(
        _bind: String,
        _jwt_secret: Option<String>,
        _no_swagger: bool,
        _socket_path: String,
        _host_network: bool,
        _data_dir: std::path::PathBuf,
        _deployment_name: String,
        _wg_port: Option<u16>,
        _dns_port: Option<u16>,
    ) -> Result<()> {
        bail!("--service is not supported on this platform (Windows only)");
    }
}

// -----------------------------------------------------------------------
// Windows implementation
// -----------------------------------------------------------------------
#[cfg(windows)]
mod imp {
    use std::ffi::OsString;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use anyhow::{Context, Result};
    use tracing::{error, info, warn};
    use windows_service::service::{
        ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
        ServiceType,
    };
    use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
    use windows_service::service_dispatcher;

    /// Name under which SCM knows the daemon service. I-2 (`daemon install`)
    /// registers this exact name via `sc create`, and I-3..I-5 (`stop`,
    /// `uninstall`, `status`) look it up by the same string.
    pub const SERVICE_NAME: &str = "ZLayerDaemon";

    // SCM entry points can only carry process-global state through statics —
    // `ffi_service_main` is called on a thread the dispatcher owns and we
    // can't plumb closures into it. Stash the args the CLI parsed in a
    // Mutex<Option<..>> so `my_service_main` can pick them up.
    struct ServiceArgs {
        bind: String,
        jwt_secret: Option<String>,
        no_swagger: bool,
        socket_path: String,
        host_network: bool,
        data_dir: PathBuf,
        deployment_name: String,
        wg_port: Option<u16>,
        dns_port: Option<u16>,
    }

    static SERVICE_ARGS: Mutex<Option<ServiceArgs>> = Mutex::new(None);

    /// Entry point called from `main()` when `zlayer serve --service` is
    /// invoked. Blocks until SCM tells the service to stop (via `Stop` or
    /// `Shutdown` control). Returns the exit code the service reported to SCM.
    ///
    /// # Errors
    ///
    /// Returns an error if the service dispatcher fails to start (e.g. the
    /// binary was not launched by SCM, in which case `service_dispatcher::start`
    /// returns a `WinError` for `ERROR_FAILED_SERVICE_CONTROLLER_CONNECT`).
    #[allow(clippy::needless_pass_by_value, clippy::too_many_arguments)]
    pub fn run_as_windows_service(
        bind: String,
        jwt_secret: Option<String>,
        no_swagger: bool,
        socket_path: String,
        host_network: bool,
        data_dir: PathBuf,
        deployment_name: String,
        wg_port: Option<u16>,
        dns_port: Option<u16>,
    ) -> Result<()> {
        // Stash the serve arguments so `my_service_main` can read them when
        // SCM invokes the dispatched entry point.
        {
            let mut slot = SERVICE_ARGS
                .lock()
                .expect("SERVICE_ARGS mutex poisoned (zlayer service startup)");
            *slot = Some(ServiceArgs {
                bind,
                jwt_secret,
                no_swagger,
                socket_path,
                host_network,
                data_dir,
                deployment_name,
                wg_port,
                dns_port,
            });
        }

        info!(
            service = SERVICE_NAME,
            "Starting Windows Service dispatcher"
        );

        // Hand control to SCM. Blocks on the current thread until the
        // service stops.
        service_dispatcher::start(SERVICE_NAME, ffi_service_main).with_context(|| {
            format!(
                "Failed to start Windows Service dispatcher for '{SERVICE_NAME}'. \
                 This binary must be launched by the Service Control Manager; \
                 run `zlayer daemon install` then `sc start {SERVICE_NAME}` instead \
                 of invoking `zlayer serve --service` directly."
            )
        })?;

        Ok(())
    }

    // Generates the FFI entry point the SCM actually calls.
    windows_service::define_windows_service!(ffi_service_main, my_service_main);

    /// Service main body. Runs on a thread spawned by the dispatcher.
    ///
    /// Building a tokio runtime here (rather than reusing `main()`'s) is
    /// intentional: the SCM entry point is sync and can only block by
    /// `block_on`-ing our own runtime. The two lifecycles are otherwise
    /// identical to foreground `zlayer serve`.
    fn my_service_main(_args: Vec<OsString>) {
        if let Err(e) = run_service() {
            error!(error = %e, "Windows Service exited with error");
        }
    }

    #[allow(clippy::too_many_lines)]
    fn run_service() -> Result<()> {
        // Pull the serve args stashed by `run_as_windows_service`.
        let args = SERVICE_ARGS
            .lock()
            .expect("SERVICE_ARGS mutex poisoned (zlayer service main)")
            .take()
            .context(
                "Windows Service entry point invoked without stashed arguments. \
                 This is a BUG: `run_as_windows_service` must populate SERVICE_ARGS \
                 before calling `service_dispatcher::start`.",
            )?;

        // Watch channel used to signal the serve loop to shut down. The SCM
        // control handler flips `tx` to `true` on Stop/Shutdown; the serve
        // loop awaits `rx` via `serve_with_external_shutdown`.
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let shutdown_tx = Arc::new(shutdown_tx);

        // Register the SCM event handler. Must happen *before* we report
        // Running, otherwise SCM silently considers the service unresponsive.
        let handler_tx = Arc::clone(&shutdown_tx);
        let event_handler = move |control_event| -> ServiceControlHandlerResult {
            match control_event {
                ServiceControl::Stop | ServiceControl::Shutdown => {
                    info!(?control_event, "SCM control received, signaling shutdown");
                    // Ignore send errors: if the receiver was dropped, the
                    // daemon has already exited and we just report back.
                    let _ = handler_tx.send(true);
                    ServiceControlHandlerResult::NoError
                }
                ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
                _ => ServiceControlHandlerResult::NotImplemented,
            }
        };

        let status_handle = service_control_handler::register(SERVICE_NAME, event_handler)
            .context("Failed to register SCM control handler")?;

        // Report Running to SCM. We accept Stop and Shutdown — PreShutdown is
        // mutually exclusive with Shutdown on Windows and we prefer the
        // Shutdown code path for simplicity.
        status_handle
            .set_service_status(ServiceStatus {
                service_type: ServiceType::OWN_PROCESS,
                current_state: ServiceState::Running,
                controls_accepted: ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
                exit_code: ServiceExitCode::Win32(0),
                checkpoint: 0,
                wait_hint: Duration::default(),
                process_id: None,
            })
            .context("Failed to report ServiceState::Running to SCM")?;

        info!(
            service = SERVICE_NAME,
            "Windows Service reported Running to SCM, starting daemon"
        );

        // Build our own tokio runtime and block on the serve loop. The
        // runtime tears down when `block_on` returns, which happens when
        // either the serve loop completes (shutdown signaled by SCM) or it
        // returns an early error.
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("zlayer-service-worker")
            .build()
            .context("Failed to create tokio runtime for Windows Service")?;

        let ServiceArgs {
            bind,
            jwt_secret,
            no_swagger,
            socket_path,
            host_network,
            data_dir,
            deployment_name,
            wg_port,
            dns_port,
        } = args;

        let serve_result = runtime.block_on(crate::commands::serve::serve_with_external_shutdown(
            &bind,
            jwt_secret,
            no_swagger,
            &socket_path,
            host_network,
            data_dir,
            deployment_name,
            wg_port,
            dns_port,
            Some(shutdown_rx),
        ));

        // Report final state to SCM. Clean exits map to Win32(0); failures
        // surface the error as Win32(1) so `sc query` and the event log both
        // reflect the abnormal termination.
        let (exit_code, clean) = match &serve_result {
            Ok(()) => {
                info!("Windows Service daemon exited cleanly");
                (ServiceExitCode::Win32(0), true)
            }
            Err(e) => {
                warn!(error = %e, "Windows Service daemon exited with error");
                (ServiceExitCode::Win32(1), false)
            }
        };

        status_handle
            .set_service_status(ServiceStatus {
                service_type: ServiceType::OWN_PROCESS,
                current_state: ServiceState::Stopped,
                controls_accepted: ServiceControlAccept::empty(),
                exit_code,
                checkpoint: 0,
                wait_hint: Duration::default(),
                process_id: None,
            })
            .context("Failed to report ServiceState::Stopped to SCM")?;

        info!(clean, "Windows Service reported Stopped to SCM");

        serve_result
    }
}
