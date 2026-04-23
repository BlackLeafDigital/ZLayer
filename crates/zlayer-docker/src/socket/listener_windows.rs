//! Named-pipe transport for the Docker Engine API server on Windows.

use std::path::Path;

use anyhow::{Context, Result};
use axum::Router;
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper::Request;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder as ConnBuilder;
use tokio::net::windows::named_pipe::ServerOptions;
use tower::ServiceExt;

pub async fn serve_on(pipe_path: &Path, app: Router) -> Result<()> {
    // The docker CLI / DOCKER_HOST accepts pipe paths of the form
    // `\\.\pipe\<name>`. Pass the raw path straight to
    // `ServerOptions::create`.
    let pipe_name = pipe_path
        .to_str()
        .context("Windows pipe path must be valid UTF-8")?;
    tracing::info!("Docker API named-pipe listening on {pipe_name}");

    // A named-pipe "server" is single-shot: each `create` instance
    // accepts one connection, then you must create another. We loop
    // and spawn per-connection tasks.
    loop {
        let server = ServerOptions::new()
            .first_pipe_instance(false)
            .create(pipe_name)
            .with_context(|| format!("failed to create named-pipe {pipe_name}"))?;
        server
            .connect()
            .await
            .context("named-pipe connect failed")?;
        let app_clone = app.clone();
        tokio::spawn(async move {
            let io = TokioIo::new(server);
            let service = service_fn(move |req: Request<Incoming>| {
                let router = app_clone.clone();
                async move {
                    let response = router
                        .oneshot(req)
                        .await
                        .map_err(|e| std::io::Error::other(format!("{e}")))?;
                    Ok::<_, std::io::Error>(response)
                }
            });
            if let Err(e) = ConnBuilder::new(TokioExecutor::new())
                .serve_connection(io, service)
                .await
            {
                tracing::warn!("named-pipe connection error: {e}");
            }
        });
    }
}
