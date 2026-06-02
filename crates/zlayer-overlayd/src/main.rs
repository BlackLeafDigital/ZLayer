//! `zlayer-overlayd` — the standalone `ZLayer` overlay daemon binary.
//!
//! Binds the IPC endpoint ([`transport::serve`]), wraps an [`OverlaydServer`]
//! in an `Arc<Mutex<_>>`, and runs each accepted connection's request loop
//! against it. A `Shutdown` request drops the overlay adapter and exits the
//! process gracefully.

use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use tokio::sync::Mutex;
use zlayer_overlayd::server::OverlaydServer;
use zlayer_overlayd::transport;
use zlayer_paths::ZLayerDirs;
use zlayer_types::overlayd::OverlaydFrame;

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
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
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
            tracing::info!("overlayd: exiting after graceful shutdown");
        }
    }

    Ok(())
}
