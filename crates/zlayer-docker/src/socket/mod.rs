//! Docker Engine API socket emulation.
//!
//! Provides a Docker-compatible HTTP API over a Unix domain socket on
//! Linux/macOS or a Windows named pipe, allowing tools like the VS Code
//! Docker extension, CI systems, and other Docker-aware tools to interact
//! with `ZLayer`.

pub mod auth;
mod build;
mod buildkit;
mod containers;
mod images;
#[cfg(unix)]
mod listener_unix;
#[cfg(windows)]
mod listener_windows;
mod manifest;
mod networks;
pub mod streaming;
mod swarm;
mod system;
mod translate;
mod trust;
mod types;
mod version_middleware;
mod volumes;

use std::path::Path;
use std::sync::Arc;

use axum::Router;
use zlayer_client::DaemonClient;

/// Shared state passed to Docker API socket handlers.
///
/// Carries a process-wide [`DaemonClient`] so each handler can dispatch
/// to the local daemon without re-opening the Unix socket per request.
#[derive(Clone)]
pub(crate) struct SocketState {
    pub(crate) client: Arc<DaemonClient>,
}

/// Start the Docker API socket server.
///
/// Dispatches to the Unix-domain-socket transport on Linux/macOS or
/// the named-pipe transport on Windows.
///
/// # Errors
///
/// Returns an error if the socket/pipe cannot be bound, the daemon
/// cannot be contacted, or the server fails.
pub async fn serve(socket_path: &Path, daemon_socket: &Path) -> anyhow::Result<()> {
    // This Docker API server runs INSIDE the daemon process (spawned by `serve`
    // when `--docker-socket` is set), so it must connect to the LOCAL daemon's
    // own API socket (`daemon_socket`) — and must NEVER use the auto-starting
    // `DaemonClient::connect()`. Auto-start spawns a SECOND `zlayer serve`
    // process that races the daemon we are part of for the API port/socket;
    // under launchd the freshly-bootstrapped job then fails to stay loaded
    // ("Daemon failed to start within 45s / no job loaded"). Instead, wait for
    // the in-process daemon to finish coming up. Its API/health only answers
    // after `init_daemon` (including deployment restoration), which can take a
    // while, so retry `try_connect_to` (which never auto-starts) on a generous
    // deadline. On Windows the daemon listens on TCP loopback, so the explicit
    // socket path is ignored there.
    let client = wait_for_local_daemon(daemon_socket).await?;
    let state = SocketState {
        client: Arc::new(client),
    };
    let app = router(state);

    #[cfg(unix)]
    {
        listener_unix::serve_on(socket_path, app).await
    }
    #[cfg(windows)]
    {
        listener_windows::serve_on(socket_path, app).await
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (socket_path, app);
        anyhow::bail!("platform not supported for Docker API server");
    }
}

/// Wait for the LOCAL daemon (the process this Docker API server runs inside)
/// to finish starting and become reachable on its API socket.
///
/// Uses [`DaemonClient::try_connect_to`] — which probes an already-running
/// daemon on the given socket and NEVER auto-starts one — in a bounded retry
/// loop. This deliberately avoids [`DaemonClient::connect`]: auto-starting from
/// here would fork a second `zlayer serve` that competes for the API
/// port/socket and breaks the launchd install. The deadline is generous because
/// the daemon's API only answers after full initialisation (overlay, storage,
/// and deployment restoration), which can take tens of seconds on a host with
/// restored deployments. On Windows the daemon listens on TCP loopback, so
/// `daemon_socket` is ignored and the default probe is used.
async fn wait_for_local_daemon(daemon_socket: &Path) -> anyhow::Result<DaemonClient> {
    use std::time::Duration;

    let poll = Duration::from_millis(250);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(300);
    loop {
        #[cfg(unix)]
        let probe = DaemonClient::try_connect_to(daemon_socket).await;
        #[cfg(not(unix))]
        let probe = {
            let _ = daemon_socket;
            DaemonClient::try_connect().await
        };
        match probe {
            Ok(Some(client)) => return Ok(client),
            Ok(None) | Err(_) if tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(poll).await;
            }
            Ok(None) => {
                anyhow::bail!(
                    "local daemon did not become reachable within 300s; \
                     Docker API socket server giving up"
                );
            }
            Err(e) => return Err(e.context("probing the local daemon for the Docker API server")),
        }
    }
}

/// Apply Docker's default-registry normalization to an image reference for the
/// Docker-compat socket path.
///
/// Real Docker resolves a bare name to the Docker Hub `library` namespace
/// (`alpine` → `docker.io/library/alpine`, `alpine:latest` →
/// `docker.io/library/alpine:latest`, `user/img` → `docker.io/user/img`).
/// The native paths (`/api/v1`, `zlayer build`) deliberately REFUSE unqualified
/// names — no silent Hub pulls — but a Docker *client* talking to our compat
/// socket expects Docker's behaviour. So we canonicalize unqualified references
/// here, upstream of the daemon's strict guard (`zlayer-registry`/`client.rs`),
/// while leaving already-qualified refs (`ghcr.io/...`, `localhost:5000/...`,
/// `…@sha256:…`) untouched. If parsing fails, fall back to the original string
/// (the daemon will surface a clear error).
pub(super) fn normalize_compat_image(image: &str) -> String {
    if !zlayer_types::image_str_is_unqualified(image) {
        return image.to_owned();
    }
    image
        .parse::<zlayer_types::ImageReference>()
        .map_or_else(|_| image.to_owned(), |r| r.whole())
}

/// Build the Docker Engine API router.
///
/// The first seven `merge` calls register real, state-bearing handler
/// modules (`/containers`, `/images`, `/build`, `/volumes`, `/networks`,
/// `/system`, and `/swarm` -- the last bridges to `ZLayer`'s native
/// cluster primitives). The trailing three (`buildkit`, `manifest`,
/// `trust`) register stub routers that answer with a Docker-shape 501 /
/// 503 so Docker clients see a clean error instead of a 404. Each stub
/// module documents what a real implementation would need.
fn router(state: SocketState) -> Router {
    let inner = Router::new()
        .merge(containers::routes(state.clone()))
        .merge(images::routes(state.clone()))
        .merge(build::routes(state.clone()))
        .merge(volumes::routes(state.clone()))
        .merge(networks::routes(state.clone()))
        .merge(system::routes(state.clone()))
        .merge(buildkit::routes())
        .merge(swarm::routes(state))
        .merge(manifest::routes())
        .merge(trust::routes());

    // Strip the Docker `/v1.NN` version prefix BEFORE routing. A plain
    // `inner.layer(strip)` does NOT work for version-prefixed paths: in axum
    // 0.8 `Router::layer` middleware only runs for requests that already match
    // a route, but `/v1.47/containers/json` matches nothing until it is
    // stripped — a chicken-and-egg that left every real Docker client (bollard,
    // the `docker` CLI) getting a 404. Wrapping the merged router as the
    // FALLBACK of an outer router and layering the strip there makes it run for
    // EVERY request, rewrite the URI, then dispatch into `inner`, which now
    // matches the version-less route.
    Router::new()
        .fallback_service(inner)
        .layer(axum::middleware::from_fn(version_middleware::strip_version))
}
