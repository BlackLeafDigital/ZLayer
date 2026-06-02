//! Windows Service Control Manager (SCM) entry point for `zlayer-overlayd`.
//!
//! When `zlayer-overlayd --service` is invoked (normally by SCM spawning the
//! binary registered via `zlayer daemon install`), `main()` hands control to
//! [`run_as_overlayd_service`] instead of the normal foreground path. From
//! there we:
//!
//!   1. Stash the parsed `(data_dir, socket)` in a process-global static so the
//!      FFI service entry point (which cannot carry closures) can read them.
//!   2. Register an SCM control handler that accepts `Stop`/`Shutdown` and
//!      fires a oneshot wired into [`crate::run_overlayd`]'s external shutdown.
//!   3. Report `ServiceState::Running` to SCM (accepting `STOP | SHUTDOWN`).
//!   4. Build our own tokio runtime (the SCM entry point is sync) and block on
//!      [`crate::run_overlayd`], passing the oneshot receiver so the daemon
//!      tears the overlay adapter down cleanly when SCM signals a stop.
//!   5. Report `ServiceState::Stopped` with an exit code reflecting whether
//!      the daemon shut down cleanly.
//!
//! This module compiles on Windows only; it mirrors the main daemon's
//! `bin/zlayer/src/daemon_service.rs::imp` implementation.

#![cfg(windows)]

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use tracing::{error, info, warn};
use windows_service::service::{
    ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::service_dispatcher;

/// SCM service name overlayd registers under. Must match
/// `daemon_service::service_name("zlayer-overlayd")` in `bin/zlayer` so
/// `sc query ZLayerDaemon-Overlayd` and `zlayer daemon install` agree.
const SERVICE_NAME: &str = "ZLayerDaemon-Overlayd";

// SCM entry points can only carry process-global state through statics —
// `ffi_service_main` is called on a thread the dispatcher owns and we can't
// plumb closures into it. Stash the args the CLI parsed in a
// `Mutex<Option<..>>` so `my_service_main` can pick them up.
struct ServiceArgs {
    data_dir: PathBuf,
    socket: PathBuf,
}

static SERVICE_ARGS: Mutex<Option<ServiceArgs>> = Mutex::new(None);

/// Entry point called from `main()` when `zlayer-overlayd --service` is
/// invoked. Blocks until SCM tells the service to stop (via `Stop` or
/// `Shutdown` control).
///
/// # Errors
///
/// Returns an error if the service dispatcher fails to start (e.g. the binary
/// was not launched by SCM, in which case `service_dispatcher::start` returns a
/// `WinError` for `ERROR_FAILED_SERVICE_CONTROLLER_CONNECT`).
pub fn run_as_overlayd_service(data_dir: PathBuf, socket: PathBuf) -> Result<()> {
    {
        let mut slot = SERVICE_ARGS
            .lock()
            .expect("SERVICE_ARGS mutex poisoned (overlayd service startup)");
        *slot = Some(ServiceArgs { data_dir, socket });
    }

    info!(service = %SERVICE_NAME, "Starting overlayd Windows Service dispatcher");

    // Hand control to SCM. Blocks on the current thread until the service
    // stops.
    service_dispatcher::start(SERVICE_NAME, ffi_service_main).with_context(|| {
        format!(
            "Failed to start Windows Service dispatcher for '{SERVICE_NAME}'. \
             This binary must be launched by the Service Control Manager; \
             run `zlayer daemon install` then `sc start {SERVICE_NAME}` instead \
             of invoking `zlayer-overlayd --service` directly."
        )
    })?;

    Ok(())
}

// Generates the FFI entry point the SCM actually calls.
windows_service::define_windows_service!(ffi_service_main, my_service_main);

/// Service main body. Runs on a thread spawned by the dispatcher.
fn my_service_main(_args: Vec<OsString>) {
    if let Err(e) = run_service() {
        error!(error = %e, "overlayd Windows Service exited with error");
    }
}

fn run_service() -> Result<()> {
    // Pull the serve args stashed by `run_as_overlayd_service`.
    let ServiceArgs { data_dir, socket } = SERVICE_ARGS
        .lock()
        .expect("SERVICE_ARGS mutex poisoned (overlayd service main)")
        .take()
        .context(
            "overlayd Windows Service entry point invoked without stashed arguments. \
             This is a BUG: `run_as_overlayd_service` must populate SERVICE_ARGS \
             before calling `service_dispatcher::start`.",
        )?;

    // Oneshot used to signal the serve loop to shut down. The SCM control
    // handler fires `tx` on Stop/Shutdown; `run_overlayd` awaits `rx` as its
    // external shutdown. Stash the sender in a Mutex so the (Fn) control
    // handler can `take()` it on the first Stop/Shutdown control.
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let shutdown_tx = Arc::new(Mutex::new(Some(shutdown_tx)));

    // Register the SCM event handler. Must happen *before* we report Running,
    // otherwise SCM silently considers the service unresponsive.
    let handler_tx = Arc::clone(&shutdown_tx);
    let event_handler = move |control_event| -> ServiceControlHandlerResult {
        match control_event {
            ServiceControl::Stop | ServiceControl::Shutdown => {
                info!(?control_event, "SCM control received, signaling shutdown");
                // Ignore send errors: if the receiver was dropped, the daemon
                // has already exited and we just report back.
                if let Some(tx) = handler_tx
                    .lock()
                    .expect("overlayd shutdown_tx mutex poisoned")
                    .take()
                {
                    let _ = tx.send(());
                }
                ServiceControlHandlerResult::NoError
            }
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            _ => ServiceControlHandlerResult::NotImplemented,
        }
    };

    let status_handle = service_control_handler::register(SERVICE_NAME, event_handler)
        .context("Failed to register SCM control handler")?;

    // Report Running to SCM. We accept Stop and Shutdown.
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
        service = %SERVICE_NAME,
        "overlayd Windows Service reported Running to SCM, starting daemon"
    );

    // Build our own tokio runtime and block on the serve loop. The runtime
    // tears down when `block_on` returns, which happens when either the serve
    // loop completes (shutdown signaled by SCM or an in-band request) or it
    // returns an early error.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("zlayer-overlayd-service-worker")
        .build()
        .context("Failed to create tokio runtime for overlayd Windows Service")?;

    let serve_result = runtime.block_on(crate::run_overlayd(data_dir, socket, shutdown_rx));

    // Report final state to SCM. Clean exits map to Win32(0); failures surface
    // the error as Win32(1) so `sc query` and the event log both reflect the
    // abnormal termination.
    let (exit_code, clean) = match &serve_result {
        Ok(()) => {
            info!("overlayd Windows Service daemon exited cleanly");
            (ServiceExitCode::Win32(0), true)
        }
        Err(e) => {
            warn!(error = %e, "overlayd Windows Service daemon exited with error");
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

    info!(clean, "overlayd Windows Service reported Stopped to SCM");

    serve_result
}
