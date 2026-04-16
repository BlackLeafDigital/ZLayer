//! Cookie helpers shared between auth handlers and CSRF middleware.
//!
//! Standard production cookies: `zlayer_session` is the `HttpOnly` signed JWT
//! session cookie; `zlayer_csrf` is the JS-readable CSRF double-submit token.

use axum_extra::extract::cookie::{Cookie, SameSite};
use rand::RngCore;
use time::Duration as TimeDuration;

/// Name of the `HttpOnly` session cookie carrying the JWT.
pub const SESSION_COOKIE: &str = "zlayer_session";

/// Name of the JS-readable CSRF double-submit cookie.
pub const CSRF_COOKIE: &str = "zlayer_csrf";

/// Name of the request header holding the CSRF token echoed by the client.
pub const CSRF_HEADER: &str = "x-csrf-token";

/// Default session TTL (24 hours). Mirrors the JWT `exp`.
pub const SESSION_TTL: TimeDuration = TimeDuration::hours(24);

/// Generate a fresh CSRF token (URL-safe base64, 32 random bytes → 43 chars).
#[must_use]
pub fn generate_csrf_token() -> String {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Build the `HttpOnly` session cookie. The caller decides `secure` based on
/// whether the API server is behind TLS — production should always pass `true`.
#[must_use]
pub fn session_cookie<'c>(jwt: String, secure: bool) -> Cookie<'c> {
    Cookie::build((SESSION_COOKIE, jwt))
        .path("/")
        .http_only(true)
        .secure(secure)
        .same_site(SameSite::Lax)
        .max_age(SESSION_TTL)
        .build()
}

/// Build a cookie that clears the session on the client.
#[must_use]
pub fn clear_session_cookie<'c>() -> Cookie<'c> {
    Cookie::build((SESSION_COOKIE, ""))
        .path("/")
        .http_only(true)
        .max_age(TimeDuration::seconds(0))
        .build()
}

/// Build the JS-readable CSRF double-submit cookie. NOT `HttpOnly` — the
/// browser-side client reads this and echoes it back in the `X-CSRF-Token`
/// header on mutating requests.
#[must_use]
pub fn csrf_cookie<'c>(token: String, secure: bool) -> Cookie<'c> {
    Cookie::build((CSRF_COOKIE, token))
        .path("/")
        .http_only(false)
        .secure(secure)
        .same_site(SameSite::Lax)
        .max_age(SESSION_TTL)
        .build()
}

/// Build a cookie that clears the CSRF token on the client.
#[must_use]
pub fn clear_csrf_cookie<'c>() -> Cookie<'c> {
    Cookie::build((CSRF_COOKIE, ""))
        .path("/")
        .max_age(TimeDuration::seconds(0))
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csrf_token_is_unique_and_correct_length() {
        let a = generate_csrf_token();
        let b = generate_csrf_token();
        assert_ne!(a, b);
        // 32 bytes → 43 base64url chars (no padding)
        assert_eq!(a.len(), 43);
        assert_eq!(b.len(), 43);
    }

    #[test]
    fn session_cookie_is_httponly_samesite_lax() {
        let c = session_cookie("jwt".into(), true);
        assert_eq!(c.name(), SESSION_COOKIE);
        assert_eq!(c.value(), "jwt");
        assert_eq!(c.http_only(), Some(true));
        assert_eq!(c.secure(), Some(true));
        assert_eq!(c.same_site(), Some(SameSite::Lax));
    }

    #[test]
    fn csrf_cookie_is_not_httponly() {
        let c = csrf_cookie("token".into(), true);
        assert_eq!(c.name(), CSRF_COOKIE);
        assert_eq!(c.http_only(), Some(false));
    }

    #[test]
    fn clear_cookies_have_zero_max_age() {
        assert_eq!(
            clear_session_cookie().max_age(),
            Some(TimeDuration::seconds(0))
        );
        assert_eq!(
            clear_csrf_cookie().max_age(),
            Some(TimeDuration::seconds(0))
        );
    }
}
