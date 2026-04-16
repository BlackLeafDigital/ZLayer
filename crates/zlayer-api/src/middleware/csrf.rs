//! CSRF middleware — double-submit token validation for cookie-authed sessions.
//!
//! Policy:
//! - Safe methods (GET, HEAD, OPTIONS, TRACE) always pass.
//! - Bearer-authed requests (`Authorization: Bearer …` header present) pass —
//!   those are API clients, not browser sessions, and don't need double-submit.
//! - Cookie-authed mutating requests must carry an `X-CSRF-Token` header that
//!   matches the `zlayer_csrf` cookie exactly (constant-time compare).
//! - Requests without any auth (no cookie + no bearer) pass — they'll be
//!   rejected downstream by the usual auth extractor, not here.

use axum::{
    body::Body,
    extract::Request,
    http::{header::AUTHORIZATION, HeaderMap, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use axum_extra::extract::cookie::CookieJar;
use subtle::ConstantTimeEq;

use super::cookies::{CSRF_COOKIE, CSRF_HEADER, SESSION_COOKIE};

/// Middleware function — wire with `axum::middleware::from_fn(csrf_middleware)`.
pub async fn csrf_middleware(req: Request<Body>, next: Next) -> Response {
    let jar = CookieJar::from_headers(req.headers());
    if !requires_csrf_check(req.method(), req.headers(), &jar) {
        return next.run(req).await;
    }

    let cookie_token = jar.get(CSRF_COOKIE).map(|c| c.value().to_string());
    let header_token = req
        .headers()
        .get(CSRF_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    match (cookie_token, header_token) {
        (Some(cookie), Some(header)) if tokens_match(&cookie, &header) => next.run(req).await,
        _ => (
            StatusCode::FORBIDDEN,
            "CSRF token missing or does not match session cookie",
        )
            .into_response(),
    }
}

/// Decide whether this request needs CSRF validation.
///
/// Exported for unit testing. Returns `true` only when the request is
/// mutating AND carries a session cookie AND does NOT carry a bearer token.
pub(crate) fn requires_csrf_check(method: &Method, headers: &HeaderMap, jar: &CookieJar) -> bool {
    if matches!(
        method,
        &Method::GET | &Method::HEAD | &Method::OPTIONS | &Method::TRACE
    ) {
        return false;
    }

    let has_bearer = headers
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|h| h.starts_with("Bearer "));
    if has_bearer {
        return false;
    }

    jar.get(SESSION_COOKIE).is_some()
}

/// Constant-time token comparison — avoids timing leaks even though these
/// tokens are not long-term secrets.
fn tokens_match(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.as_bytes().ct_eq(b.as_bytes()).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderName, HeaderValue};

    fn headers_with(entries: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in entries {
            let name = HeaderName::from_bytes(k.as_bytes()).unwrap();
            h.insert(name, HeaderValue::from_str(v).unwrap());
        }
        h
    }

    fn jar_with_cookies(pairs: &[(&str, &str)]) -> CookieJar {
        let cookie_header = pairs
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("; ");
        let mut headers = HeaderMap::new();
        if !cookie_header.is_empty() {
            headers.insert(
                axum::http::header::COOKIE,
                HeaderValue::from_str(&cookie_header).unwrap(),
            );
        }
        CookieJar::from_headers(&headers)
    }

    #[test]
    fn safe_methods_skip_csrf() {
        let jar = jar_with_cookies(&[(SESSION_COOKIE, "jwt")]);
        assert!(!requires_csrf_check(&Method::GET, &HeaderMap::new(), &jar));
        assert!(!requires_csrf_check(&Method::HEAD, &HeaderMap::new(), &jar));
        assert!(!requires_csrf_check(
            &Method::OPTIONS,
            &HeaderMap::new(),
            &jar
        ));
    }

    #[test]
    fn bearer_auth_skips_csrf() {
        let headers = headers_with(&[("authorization", "Bearer abc")]);
        let jar = jar_with_cookies(&[(SESSION_COOKIE, "jwt")]);
        assert!(!requires_csrf_check(&Method::POST, &headers, &jar));
    }

    #[test]
    fn no_session_cookie_no_csrf_check() {
        let jar = jar_with_cookies(&[]);
        assert!(!requires_csrf_check(&Method::POST, &HeaderMap::new(), &jar));
    }

    #[test]
    fn cookie_authed_post_requires_csrf() {
        let jar = jar_with_cookies(&[(SESSION_COOKIE, "jwt")]);
        assert!(requires_csrf_check(&Method::POST, &HeaderMap::new(), &jar));
        assert!(requires_csrf_check(&Method::PATCH, &HeaderMap::new(), &jar));
        assert!(requires_csrf_check(
            &Method::DELETE,
            &HeaderMap::new(),
            &jar
        ));
    }

    #[test]
    fn constant_time_compare_ok() {
        assert!(tokens_match("same", "same"));
        assert!(!tokens_match("a", "ab"));
        assert!(!tokens_match("abc", "abd"));
    }
}
