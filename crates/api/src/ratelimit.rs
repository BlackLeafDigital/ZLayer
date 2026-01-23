//! Rate limiting middleware for the ZLayer API
//!
//! Provides per-IP rate limiting using the governor crate.

use axum::{
    body::Body,
    extract::ConnectInfo,
    http::Request,
    middleware::Next,
    response::{IntoResponse, Response},
};
use governor::{clock::DefaultClock, state::InMemoryState, Quota, RateLimiter};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::num::NonZeroU32;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::warn;

use crate::config::RateLimitConfig;
use crate::error::ApiError;

/// Global rate limiter (per-instance, not per-IP)
/// Uses the default direct rate limiter type from governor
pub type GlobalLimiter = RateLimiter<
    governor::state::NotKeyed,
    InMemoryState,
    DefaultClock,
    governor::middleware::NoOpMiddleware,
>;

/// Create a global rate limiter
pub fn create_global_limiter(config: &RateLimitConfig) -> Arc<GlobalLimiter> {
    let rps = NonZeroU32::new(config.requests_per_second).unwrap_or(NonZeroU32::new(100).unwrap());
    let burst = NonZeroU32::new(config.burst_size).unwrap_or(NonZeroU32::new(50).unwrap());

    let quota = Quota::per_second(rps).allow_burst(burst);

    Arc::new(RateLimiter::direct(quota))
}

/// Rate limit state for middleware
#[derive(Clone)]
pub struct RateLimitState {
    pub limiter: Arc<GlobalLimiter>,
    pub enabled: bool,
}

impl RateLimitState {
    pub fn new(config: &RateLimitConfig) -> Self {
        Self {
            limiter: create_global_limiter(config),
            enabled: config.enabled,
        }
    }

    pub fn disabled() -> Self {
        Self {
            limiter: Arc::new(RateLimiter::direct(
                Quota::per_second(NonZeroU32::new(u32::MAX).unwrap()),
            )),
            enabled: false,
        }
    }
}

/// Rate limiting middleware
pub async fn rate_limit_middleware(request: Request<Body>, next: Next) -> Response {
    // Get rate limit state from extensions
    let state = request.extensions().get::<RateLimitState>().cloned();

    if let Some(state) = state {
        if state.enabled {
            // Check rate limit
            match state.limiter.check() {
                Ok(_) => {
                    // Request allowed
                }
                Err(_) => {
                    warn!("Rate limit exceeded");
                    return ApiError::RateLimited.into_response();
                }
            }
        }
    }

    next.run(request).await
}

/// Per-IP rate limiter using a concurrent hash map
pub struct IpRateLimiter {
    limiters: RwLock<HashMap<std::net::IpAddr, Arc<GlobalLimiter>>>,
    config: RateLimitConfig,
}

impl IpRateLimiter {
    pub fn new(config: RateLimitConfig) -> Self {
        Self {
            limiters: RwLock::new(HashMap::new()),
            config,
        }
    }

    pub async fn check(&self, ip: std::net::IpAddr) -> bool {
        if !self.config.enabled {
            return true;
        }

        // Get or create limiter for this IP
        let limiter = {
            let read_guard = self.limiters.read().await;
            if let Some(limiter) = read_guard.get(&ip) {
                limiter.clone()
            } else {
                drop(read_guard);

                let mut write_guard = self.limiters.write().await;
                // Double-check after acquiring write lock
                if let Some(limiter) = write_guard.get(&ip) {
                    limiter.clone()
                } else {
                    let limiter = create_global_limiter(&self.config);
                    write_guard.insert(ip, limiter.clone());
                    limiter
                }
            }
        };

        limiter.check().is_ok()
    }

    /// Clean up old entries (call periodically)
    pub async fn cleanup(&self) {
        let mut write_guard = self.limiters.write().await;
        // In a real implementation, you'd track last access time
        // and remove entries that haven't been used recently.
        // For now, we just limit the size.
        if write_guard.len() > 10000 {
            write_guard.clear();
        }
    }
}

/// Per-IP rate limiting middleware
pub async fn ip_rate_limit_middleware(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    request: Request<Body>,
    next: Next,
) -> Response {
    // Get IP limiter from extensions
    let limiter = request.extensions().get::<Arc<IpRateLimiter>>().cloned();

    if let Some(limiter) = limiter {
        if !limiter.check(addr.ip()).await {
            warn!(ip = %addr.ip(), "Rate limit exceeded for IP");
            return ApiError::RateLimited.into_response();
        }
    }

    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_limiter() {
        let config = RateLimitConfig {
            enabled: true,
            requests_per_second: 10,
            burst_size: 5,
        };

        let limiter = create_global_limiter(&config);

        // Should allow burst
        for _ in 0..5 {
            assert!(limiter.check().is_ok());
        }
    }

    #[test]
    fn test_rate_limit_state_disabled() {
        let state = RateLimitState::disabled();
        assert!(!state.enabled);
    }

    #[tokio::test]
    async fn test_ip_rate_limiter() {
        let config = RateLimitConfig {
            enabled: true,
            requests_per_second: 2,
            burst_size: 2,
        };

        let limiter = IpRateLimiter::new(config);
        let ip: std::net::IpAddr = "127.0.0.1".parse().unwrap();

        // Should allow initial burst
        assert!(limiter.check(ip).await);
        assert!(limiter.check(ip).await);
    }

    #[tokio::test]
    async fn test_ip_rate_limiter_disabled() {
        let config = RateLimitConfig {
            enabled: false,
            requests_per_second: 1,
            burst_size: 1,
        };

        let limiter = IpRateLimiter::new(config);
        let ip: std::net::IpAddr = "127.0.0.1".parse().unwrap();

        // Should always allow when disabled
        for _ in 0..100 {
            assert!(limiter.check(ip).await);
        }
    }
}
