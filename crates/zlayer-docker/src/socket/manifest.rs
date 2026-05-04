//! Docker manifest distribution endpoints — stub responses.
//!
//! `GET /distribution/{name}/json` is used by `docker manifest inspect`
//! to fetch a manifest list (multi-arch index) directly from a registry,
//! tunnelled through the daemon's authentication. The daemon performs the
//! registry handshake on the client's behalf and returns the merged
//! manifest descriptor.
//!
//! `ZLayer`'s registry client (`zlayer-registry`) supports pulling images
//! by digest and tag, but it does NOT yet expose a manifest-inspection
//! endpoint that mirrors Docker's `DistributionInspect` response shape.
//! Until that is implemented, this stub returns a Docker-shape `501 Not
//! Implemented` so `docker manifest inspect` prints a clear error rather
//! than receiving a 404 and complaining about an unreachable daemon.
//!
//! # Implementing for real
//!
//! A real implementation would have to:
//!  1. Resolve the registry from the image reference (`{name}` is a
//!     possibly-prefixed reference like `docker.io/library/alpine:3.18`).
//!  2. Authenticate using the `X-Registry-Auth` header (already decoded by
//!     `super::auth`).
//!  3. Issue a manifest fetch with `Accept: application/vnd.oci.image.\
//!     index.v1+json, application/vnd.docker.distribution.manifest.list.v2+json`.
//!  4. Return a [`types::DistributionInspect`]-shape body containing
//!     `Descriptor` and `Platforms` fields.
//!
//! See the Docker API reference, section "Distribution," for the exact
//! response shape: <https://docs.docker.com/engine/api/v1.43/#tag/Distribution>.

use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::get;
use axum::Router;

use super::system::error_response;

/// Manifest distribution route table. Stateless.
///
/// Axum requires catch-all parameters to live at the end of the path, but
/// Docker's reference includes a trailing `/json` segment after the
/// (slash-bearing) image name. We work around that by registering a
/// catch-all `/distribution/{*tail}` and validating the `/json` suffix in
/// the handler — anything else falls through to a 404 from the outer
/// router, which is the correct shape for an unknown distribution path.
pub(crate) fn routes() -> Router {
    Router::new().route("/distribution/{*tail}", get(distribution_inspect_stub))
}

/// Message returned for any manifest-inspection request.
const MANIFEST_MESSAGE: &str =
    "Manifest inspection (`docker manifest inspect`) is not yet supported \
     by the ZLayer daemon. Use `skopeo inspect docker://<image>` or query \
     the registry directly until ZLayer's registry client exposes this \
     endpoint.";

/// `GET /distribution/{name}/json` — Inspect a remote image manifest.
///
/// The route is registered as `/distribution/{*tail}` because Docker
/// references frequently include slashes (`library/alpine`,
/// `ghcr.io/foo/bar:tag`) and axum only permits catch-all parameters at
/// the end of a path. We accept the catch-all and require the trailing
/// `/json` here; anything else (e.g. a registry-API path that would need
/// proxying) returns 404 so it surfaces clearly rather than silently
/// 501-ing.
async fn distribution_inspect_stub(Path(tail): Path<String>) -> Response {
    if tail.ends_with("/json") {
        error_response(StatusCode::NOT_IMPLEMENTED, MANIFEST_MESSAGE)
    } else {
        error_response(
            StatusCode::NOT_FOUND,
            "unknown /distribution path; only `/distribution/{name}/json` is recognised",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use tower::ServiceExt;

    /// `GET /distribution/.../json` returns 501 + Docker-shape message.
    /// Hits a few realistic image reference shapes (single-segment,
    /// namespaced, registry-qualified) to make sure the wildcard route
    /// matches all of them.
    #[tokio::test]
    async fn distribution_inspect_returns_501() {
        for path in [
            "/distribution/alpine/json",
            "/distribution/library/alpine/json",
            "/distribution/ghcr.io/foo/bar/json",
        ] {
            let resp = routes()
                .oneshot(
                    Request::builder()
                        .method("GET")
                        .uri(path)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::NOT_IMPLEMENTED, "path = {path}");
            let bytes = to_bytes(resp.into_body(), 4096).await.expect("body");
            let v: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
            let msg = v["message"].as_str().expect("message");
            assert!(
                msg.contains("Manifest"),
                "{path}: message must mention Manifest"
            );
            assert_eq!(v.as_object().expect("object").len(), 1);
        }
    }
}
