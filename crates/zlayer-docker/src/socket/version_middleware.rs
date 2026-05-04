//! Docker API version-prefix stripping middleware.
//!
//! Docker Engine clients send requests as `/v1.43/_ping`, `/v1.40/info`, etc.
//! This middleware strips a leading `/v\d+\.\d+` segment from the request URI
//! before the router dispatches, so handlers can be registered at the
//! version-less path (`/_ping`, `/info`) and still serve all clients.

use axum::{
    extract::Request,
    http::uri::{PathAndQuery, Uri},
    middleware::Next,
    response::Response,
};

/// Strip a leading `/v<major>.<minor>` segment from the request URI.
///
/// `/v1.43/_ping` -> `/_ping`
/// `/v1.40/containers/json` -> `/containers/json`
/// `/_ping` -> `/_ping` (no-op)
pub async fn strip_version(mut req: Request, next: Next) -> Response {
    let path = req.uri().path();
    if let Some(rest) = strip_version_path(path) {
        let new_path = if rest.is_empty() { "/" } else { rest };
        let query = req.uri().query();
        let new_pq: PathAndQuery = match query {
            Some(q) => format!("{new_path}?{q}")
                .parse()
                .unwrap_or_else(|_| PathAndQuery::from_static("/")),
            None => new_path
                .parse()
                .unwrap_or_else(|_| PathAndQuery::from_static("/")),
        };
        let mut parts = req.uri().clone().into_parts();
        parts.path_and_query = Some(new_pq);
        if let Ok(new_uri) = Uri::from_parts(parts) {
            *req.uri_mut() = new_uri;
        }
    }
    next.run(req).await
}

/// Return `Some(rest)` when `path` begins with `/v<digits>.<digits>` and
/// `rest` is the remainder of the path (including the leading `/`, or
/// empty when the version segment was the only segment). Returns `None`
/// for any other shape (no leading `/v`, non-numeric segments, three or
/// more dotted segments, missing major or minor).
fn strip_version_path(path: &str) -> Option<&str> {
    let stripped = path.strip_prefix("/v")?;
    let (ver, rest) = match stripped.find('/') {
        Some(slash) => (&stripped[..slash], &stripped[slash..]),
        None => (stripped, ""),
    };
    let mut parts = ver.split('.');
    let major = parts.next()?;
    let minor = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    if major.is_empty() || !major.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if minor.is_empty() || !minor.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(rest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_simple() {
        assert_eq!(strip_version_path("/v1.43/_ping"), Some("/_ping"));
    }

    #[test]
    fn strips_with_subpath() {
        assert_eq!(
            strip_version_path("/v1.40/containers/json"),
            Some("/containers/json")
        );
    }

    #[test]
    fn strips_root() {
        assert_eq!(strip_version_path("/v1.43"), Some(""));
    }

    #[test]
    fn rejects_unversioned() {
        assert_eq!(strip_version_path("/_ping"), None);
    }

    #[test]
    fn rejects_letters() {
        assert_eq!(strip_version_path("/vfoo/bar"), None);
    }

    #[test]
    fn rejects_three_segs() {
        assert_eq!(strip_version_path("/v1.2.3/x"), None);
    }

    #[test]
    fn rejects_no_minor() {
        assert_eq!(strip_version_path("/v1/x"), None);
    }

    #[test]
    fn rejects_empty_major() {
        assert_eq!(strip_version_path("/v.5/x"), None);
    }

    #[test]
    fn rejects_empty_minor() {
        assert_eq!(strip_version_path("/v1./x"), None);
    }

    #[test]
    fn rejects_negative_major() {
        assert_eq!(strip_version_path("/v-1.0/x"), None);
    }

    #[test]
    fn strips_multidigit() {
        assert_eq!(strip_version_path("/v12.345/x"), Some("/x"));
    }
}
