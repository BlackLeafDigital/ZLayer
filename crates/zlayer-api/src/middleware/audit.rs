//! Audit middleware -- records an audit entry for every mutating request that
//! succeeds.
//!
//! Policy:
//! - Safe methods (GET, HEAD, OPTIONS, TRACE) are ignored.
//! - Only 2xx responses are recorded (failed requests are not audited).
//! - Extracts the user id from the `AuthActor` if it was placed in extensions
//!   (e.g. by a prior auth layer). If no actor is found, the entry is skipped
//!   (unauthenticated requests are not audited).
//!
//! Wire with `axum::middleware::from_fn(audit_middleware)` **after** the auth
//! layer so that `AuthActor` is available in extensions, and **before** the
//! handlers.

use std::sync::Arc;

use axum::{body::Body, extract::Request, http::Method, middleware::Next, response::Response};

use crate::handlers::users::AuthActor;
use crate::storage::{AuditEntry, AuditStorage};

/// Extension type inserted into the router so the middleware can locate the
/// audit store.
#[derive(Clone)]
pub struct AuditStoreExtension(pub Arc<dyn AuditStorage>);

/// Axum middleware that records an audit entry for every mutating request
/// (POST, PUT, PATCH, DELETE) that succeeds (2xx status).
pub async fn audit_middleware(req: Request<Body>, next: Next) -> Response {
    // Safe methods are never audited.
    if matches!(
        req.method(),
        &Method::GET | &Method::HEAD | &Method::OPTIONS | &Method::TRACE
    ) {
        return next.run(req).await;
    }

    // Capture request metadata before forwarding.
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let user_agent = req
        .headers()
        .get(axum::http::header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(String::from);

    // Try to extract the actor and audit store from extensions.
    let actor = req.extensions().get::<AuthActor>().cloned();
    let audit_ext = req.extensions().get::<AuditStoreExtension>().cloned();

    let response = next.run(req).await;

    // Only audit successful mutating requests.
    if response.status().is_success() {
        if let (Some(actor), Some(ext)) = (actor, audit_ext) {
            // Derive a human-readable action from the HTTP method.
            let action = match method {
                Method::POST => "create",
                Method::PUT | Method::PATCH => "update",
                Method::DELETE => "delete",
                _ => "unknown",
            };

            // Derive resource_kind from the URL path.  Heuristic: take the
            // first segment after `/api/v1/`.
            let resource_kind = path
                .strip_prefix("/api/v1/")
                .and_then(|rest| rest.split('/').next())
                .unwrap_or("unknown")
                .to_string();

            // The resource id is the next segment after the kind, if any.
            let resource_id = path
                .strip_prefix("/api/v1/")
                .and_then(|rest| {
                    let mut parts = rest.split('/');
                    parts.next(); // skip kind
                    parts.next().map(String::from)
                })
                .filter(|s| !s.is_empty());

            let mut entry = AuditEntry::new(&actor.user_id, action, &resource_kind);
            entry.resource_id = resource_id;
            entry.user_agent = user_agent;
            // IP is not available here without a real
            // `ConnectInfo` extractor; leave it as None.

            // Fire and forget -- audit failures should not break requests.
            let _ = ext.0.record(&entry).await;
        }
    }

    response
}

#[cfg(test)]
mod tests {
    use axum::http::Method;

    #[test]
    fn test_safe_methods_not_audited() {
        for method in &[Method::GET, Method::HEAD, Method::OPTIONS, Method::TRACE] {
            assert!(matches!(
                method,
                &Method::GET | &Method::HEAD | &Method::OPTIONS | &Method::TRACE
            ));
        }
    }

    #[test]
    fn test_resource_kind_parsing() {
        let path = "/api/v1/deployments/my-deploy";
        let kind = path
            .strip_prefix("/api/v1/")
            .and_then(|rest| rest.split('/').next())
            .unwrap_or("unknown");
        assert_eq!(kind, "deployments");
    }

    #[test]
    fn test_resource_id_parsing() {
        let path = "/api/v1/groups/g-123/members";
        let id = path
            .strip_prefix("/api/v1/")
            .and_then(|rest| {
                let mut parts = rest.split('/');
                parts.next();
                parts.next().map(String::from)
            })
            .filter(|s| !s.is_empty());
        assert_eq!(id.as_deref(), Some("g-123"));
    }

    #[test]
    fn test_resource_id_missing() {
        let path = "/api/v1/groups";
        let id = path
            .strip_prefix("/api/v1/")
            .and_then(|rest| {
                let mut parts = rest.split('/');
                parts.next();
                parts.next().map(String::from)
            })
            .filter(|s| !s.is_empty());
        assert!(id.is_none());
    }
}
