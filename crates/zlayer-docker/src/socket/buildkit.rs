//! Docker `BuildKit` session endpoints — stub responses.
//!
//! Modern Docker clients (`buildx`, `docker build` with `DOCKER_BUILDKIT=1`)
//! upgrade their `POST /build` request into a `BuildKit` gRPC session over
//! `POST /session`. They also probe `GET /grpc` when negotiating gRPC-over-
//! HTTP transports. `ZLayer`'s image builder is buildah-based and does NOT
//! speak the `BuildKit` gRPC protocol, so we cannot honour the upgrade.
//!
//! Rather than letting these requests hit a 404 (which produces opaque
//! "connection reset" errors in the Docker CLI), each route below answers
//! with a Docker-shape `501 Not Implemented` body explaining what is
//! missing and how to opt out (`DOCKER_BUILDKIT=0`, or use the legacy
//! `/build` endpoint).
//!
//! # Implementing for real
//!
//! A real `BuildKit` shim would have to:
//!  1. Accept the `Upgrade: h2c` (or websocket) handshake on `POST /session`.
//!  2. Speak the `moby.buildkit.v1.Control` and `moby.buildkit.v1.frontend`
//!     gRPC services (see `github.com/moby/buildkit/api/services`).
//!  3. Translate `BuildKit`'s LLB graph into `ZLayer`'s buildah pipeline (or
//!     embed `buildkitd` as a sidecar and proxy to it).
//!
//! That is a substantial undertaking; until then these stubs keep clients
//! from hanging on a phantom upgrade.

use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;

use super::system::error_response;

/// `BuildKit` route table.
///
/// Mounted by [`super::router`] at the daemon root so requests like
/// `POST /session` resolve here. Stateless: every handler short-circuits
/// to a 501 response.
pub(crate) fn routes() -> Router {
    Router::new()
        .route("/session", post(session_stub))
        .route("/grpc", get(grpc_stub))
}

/// Message body shared by both stubs.
const BUILDKIT_MESSAGE: &str =
    "BuildKit is not supported by the ZLayer daemon. Set DOCKER_BUILDKIT=0 \
     in your environment to fall back to the legacy builder, or use \
     `zlayer build` directly.";

/// `POST /session` — `BuildKit` session upgrade. Always 501.
///
/// Docker clients send this when `DOCKER_BUILDKIT=1`. Returning 501 makes
/// `docker buildx` print a clear error rather than silently hanging on the
/// gRPC handshake.
async fn session_stub() -> Response {
    error_response(StatusCode::NOT_IMPLEMENTED, BUILDKIT_MESSAGE)
}

/// `GET /grpc` — gRPC-over-HTTP probe. Always 501.
///
/// Some clients probe this path to decide whether the daemon supports
/// `BuildKit`'s gRPC transport. Answering 501 (rather than 404) keeps the
/// error path consistent with the rest of the unsupported `BuildKit` surface.
async fn grpc_stub() -> Response {
    error_response(StatusCode::NOT_IMPLEMENTED, BUILDKIT_MESSAGE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use tower::ServiceExt;

    /// `POST /session` returns the documented 501 + Docker-shape body.
    #[tokio::test]
    async fn session_returns_501_with_docker_message() {
        let resp = routes()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/session")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_IMPLEMENTED);
        let bytes = to_bytes(resp.into_body(), 4096).await.expect("body");
        let v: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        let msg = v["message"].as_str().expect("message field");
        assert!(msg.contains("BuildKit"), "message must mention BuildKit");
        assert!(
            msg.contains("DOCKER_BUILDKIT=0"),
            "message must suggest the documented opt-out"
        );
        assert_eq!(v.as_object().expect("object").len(), 1);
    }

    /// `GET /grpc` returns the documented 501 + Docker-shape body.
    #[tokio::test]
    async fn grpc_returns_501_with_docker_message() {
        let resp = routes()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/grpc")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_IMPLEMENTED);
        let bytes = to_bytes(resp.into_body(), 4096).await.expect("body");
        let v: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        assert!(v["message"].is_string());
    }
}
