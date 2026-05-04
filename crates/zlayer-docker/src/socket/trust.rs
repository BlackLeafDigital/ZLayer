//! Docker Content Trust (Notary) — intentionally empty.
//!
//! Docker Content Trust (DCT, formerly "Notary v1") is implemented entirely
//! **client-side**: when `DOCKER_CONTENT_TRUST=1` is set, the Docker CLI
//! signs and verifies image tags against a Notary server *before* it ever
//! talks to `dockerd`. The daemon itself has no `/trust` endpoint to
//! emulate — there is nothing on the wire for `ZLayer` to stub.
//!
//! This module exists so the route registration in [`super::router`]
//! reads as an exhaustive accounting of unsupported Docker subsystems
//! (`buildkit`, `swarm`, `manifest`, `trust`). It deliberately exposes
//! **no** routes; calling [`routes`] yields an empty `Router`.
//!
//! # If a client breaks
//!
//! If a future Docker release adds a server-side trust endpoint, add it
//! to [`routes`] returning a Docker-shape 501 via
//! [`super::system::error_response`]. The client opt-out is to unset
//! `DOCKER_CONTENT_TRUST` (or set it to `0`).

use axum::Router;

/// Trust route table.
///
/// Returns an empty `Router` because Docker Content Trust runs in the CLI,
/// not the daemon. Mounted by [`super::router`] for symmetry with the
/// other unsupported-subsystem stubs.
pub(crate) fn routes() -> Router {
    Router::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    /// The trust router exposes zero endpoints — any path returns 404.
    /// This pins the contract: if a future change accidentally adds a
    /// route here, this test must be updated explicitly so the change is
    /// reviewed.
    #[tokio::test]
    async fn trust_router_exposes_no_routes() {
        let resp = routes()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/trust")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
