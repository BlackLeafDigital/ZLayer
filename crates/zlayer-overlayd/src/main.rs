//! `zlayer-overlayd` — the standalone `ZLayer` overlay daemon binary.
//!
//! Binds the IPC endpoint ([`transport::serve`]), wraps an [`OverlaydServer`]
//! in an `Arc<Mutex<_>>`, and runs each accepted connection's request loop
//! against it. A `Shutdown` request drops the overlay adapter and exits the
//! process gracefully.
//!
//! On Windows, when launched by the Service Control Manager with `--service`,
//! control is handed to the SCM dispatcher in [`mod@service`] which registers a
//! control handler (Stop/Shutdown → graceful exit), reports `Running`/`Stopped`,
//! and drives [`run_overlayd`] on a runtime it builds itself. The plain
//! foreground path uses the same [`run_overlayd`] with a `Ctrl-C`-wired
//! shutdown so dev runs exit cleanly too.

use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use tokio::sync::Mutex;
use zlayer_overlayd::server::OverlaydServer;
use zlayer_overlayd::transport;
use zlayer_paths::ZLayerDirs;
use zlayer_types::overlayd::OverlaydFrame;

#[cfg(windows)]
mod service;

/// Standalone `ZLayer` overlay daemon.
#[derive(Debug, Parser)]
#[command(name = "zlayer-overlayd", about, version)]
struct Args {
    /// Root data directory (HCN markers, IPAM state, UAPI sockets live under it).
    #[arg(long, value_name = "PATH")]
    data_dir: Option<PathBuf>,

    /// IPC endpoint to listen on (Unix socket path, or `\\.\pipe\...` on
    /// Windows). Defaults to a data-dir-aware path.
    #[arg(long, value_name = "PATH")]
    socket: Option<PathBuf>,

    /// Run under the Windows Service Control Manager (SCM). Set automatically
    /// by `zlayer daemon install` when registering the overlayd service; not
    /// meant to be passed by hand. Windows-only; ignored elsewhere.
    #[arg(long, hide = true)]
    #[cfg_attr(not(windows), allow(dead_code))]
    service: bool,
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();

    let data_dir = args.data_dir.unwrap_or_else(ZLayerDirs::default_data_dir);
    let socket = args
        .socket
        .unwrap_or_else(|| PathBuf::from(ZLayerDirs::default_overlayd_socket_path_for(&data_dir)));

    tracing::info!(
        data_dir = %data_dir.display(),
        socket = %socket.display(),
        "starting zlayer-overlayd"
    );

    // Windows SCM mode: hand control to the dispatcher, which owns its own
    // tokio runtime and drives `run_overlayd` for us. Blocks until SCM stops
    // the service.
    #[cfg(windows)]
    if args.service {
        return service::run_as_overlayd_service(data_dir, socket);
    }

    // Foreground / dev mode: build a runtime and drive `run_overlayd` directly,
    // wiring Ctrl-C (and SIGTERM on Unix) to the external shutdown so the
    // process exits cleanly when interrupted.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("zlayer-overlayd-worker")
        .build()?;

    runtime.block_on(async move {
        let (external_tx, external_rx) = tokio::sync::oneshot::channel::<()>();
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                tracing::info!("overlayd: Ctrl-C received; signaling shutdown");
                let _ = external_tx.send(());
            }
        });
        run_overlayd(data_dir, socket, external_rx).await
    })
}

/// Run the overlayd serve loop until either an in-band `Shutdown` IPC request
/// is observed, the `external_shutdown` oneshot fires, or the accept loop
/// returns. Shared by the foreground path and the Windows SCM dispatcher.
///
/// `external_shutdown` is fired by the caller's signal handler (Ctrl-C in
/// dev, an SCM Stop/Shutdown control under the service) and joins the same
/// `select!` as the existing in-band shutdown, so both routes tear the
/// adapter down gracefully.
///
/// # Errors
///
/// Returns an error if binding the IPC endpoint or the accept loop fails.
async fn run_overlayd(
    data_dir: PathBuf,
    socket: PathBuf,
    external_shutdown: tokio::sync::oneshot::Receiver<()>,
) -> anyhow::Result<()> {
    // Scope the overlay's UAPI sockets to this data dir so a test daemon's
    // sockets don't collide with a host-wide install.
    let dirs = ZLayerDirs::new(data_dir.clone());
    let server = OverlaydServer::new(data_dir).with_uapi_sock_dir(dirs.wireguard());
    let server = Arc::new(Mutex::new(server));

    // A oneshot fired when a connection handler observes a Shutdown request, so
    // `serve`'s accept loop can be aborted and the process exit gracefully.
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let shutdown_tx = Arc::new(Mutex::new(Some(shutdown_tx)));

    let serve = {
        let server = Arc::clone(&server);
        let shutdown_tx = Arc::clone(&shutdown_tx);
        transport::serve(&socket, move |mut conn| {
            let server = Arc::clone(&server);
            let shutdown_tx = Arc::clone(&shutdown_tx);
            async move {
                loop {
                    let frame = match conn.recv().await {
                        Ok(f) => f,
                        Err(zlayer_overlayd::OverlaydError::Closed) => break,
                        Err(e) => {
                            tracing::warn!(error = %e, "overlayd: recv failed; closing connection");
                            break;
                        }
                    };
                    let OverlaydFrame::Request { id, request } = frame else {
                        tracing::warn!("overlayd: received non-Request frame; ignoring");
                        continue;
                    };

                    let (response, shutdown) = {
                        let mut guard = server.lock().await;
                        let response = guard.handle(request).await;
                        (response, guard.shutdown_requested())
                    };

                    if let Err(e) = conn.send(&OverlaydFrame::Response { id, response }).await {
                        tracing::warn!(error = %e, "overlayd: send failed; closing connection");
                        break;
                    }

                    if shutdown {
                        tracing::info!("overlayd: shutdown requested; signaling exit");
                        if let Some(tx) = shutdown_tx.lock().await.take() {
                            let _ = tx.send(());
                        }
                        break;
                    }
                }
            }
        })
    };

    tokio::select! {
        res = serve => {
            res?;
        }
        _ = shutdown_rx => {
            tracing::info!("overlayd: exiting after graceful shutdown (in-band request)");
        }
        _ = external_shutdown => {
            tracing::info!("overlayd: exiting after graceful shutdown (external signal)");
        }
    }

    Ok(())
}
