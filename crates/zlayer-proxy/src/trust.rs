//! Trusted-proxy predicate.
//!
//! Combines user-configured trusted-proxy CIDRs with an optional Cloudflare IP
//! cache into a single `is_trusted(peer_ip) -> bool` predicate. The
//! reverse-proxy service uses this to decide whether to honor
//! `CF-Connecting-IP` / `X-Forwarded-For` request headers from a given peer.

use std::net::IpAddr;
use std::sync::Arc;

use ipnet::IpNet;

use crate::cf_ip_list::CloudflareIpCache;

/// A list of trusted upstream proxies, expressed as a set of user CIDRs plus
/// an optional Cloudflare IP cache.
#[derive(Clone)]
pub struct TrustedProxyList {
    user_cidrs: Vec<IpNet>,
    cf_cache: Option<Arc<CloudflareIpCache>>,
}

impl TrustedProxyList {
    /// Build a trusted-proxy list from user CIDRs and an optional Cloudflare
    /// IP cache.
    #[must_use]
    pub fn new(user_cidrs: Vec<IpNet>, cf_cache: Option<Arc<CloudflareIpCache>>) -> Self {
        Self {
            user_cidrs,
            cf_cache,
        }
    }

    /// Shortcut: localhost-only, no CF. Safe default for origins that are NOT
    /// behind any upstream proxy.
    ///
    /// # Panics
    ///
    /// This function contains `.expect()` calls on CIDR parsing, but in
    /// practice it never panics: the CIDR strings (`127.0.0.0/8` and
    /// `::1/128`) are hardcoded constants that are guaranteed to be valid
    /// `IpNet` values. The expects exist only to satisfy the `FromStr`
    /// signature.
    #[must_use]
    pub fn localhost_only() -> Self {
        Self::new(
            vec![
                "127.0.0.0/8"
                    .parse()
                    .expect("hardcoded loopback CIDR is valid"),
                "::1/128"
                    .parse()
                    .expect("hardcoded IPv6 loopback CIDR is valid"),
            ],
            None,
        )
    }

    /// True if `peer` is in `user_cidrs` OR in the CF cache (when present).
    #[must_use]
    pub fn is_trusted(&self, peer: IpAddr) -> bool {
        if self.user_cidrs.iter().any(|net| net.contains(&peer)) {
            return true;
        }

        if let Some(cache) = &self.cf_cache {
            if cache.contains(peer) {
                return true;
            }
        }

        false
    }

    /// Returns the configured user CIDRs (for diagnostics / inspection).
    #[must_use]
    pub fn user_cidrs(&self) -> &[IpNet] {
        &self.user_cidrs
    }

    /// True if a CF cache is attached.
    #[must_use]
    pub fn has_cloudflare_trust(&self) -> bool {
        self.cf_cache.is_some()
    }
}

impl std::fmt::Debug for TrustedProxyList {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TrustedProxyList")
            .field("user_cidrs", &self.user_cidrs)
            .field("cf_cache_attached", &self.cf_cache.is_some())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn localhost_only_trusts_127_0_0_1() {
        let list = TrustedProxyList::localhost_only();
        assert!(list.is_trusted("127.0.0.1".parse().unwrap()));
    }

    #[test]
    fn localhost_only_rejects_public_ip() {
        let list = TrustedProxyList::localhost_only();
        assert!(!list.is_trusted("8.8.8.8".parse().unwrap()));
    }

    #[test]
    fn user_cidr_trusts_in_range_rejects_out_of_range() {
        let list = TrustedProxyList::new(vec!["10.0.0.0/24".parse().unwrap()], None);
        assert!(list.is_trusted("10.0.0.5".parse().unwrap()));
        assert!(!list.is_trusted("10.0.1.5".parse().unwrap()));
    }

    #[test]
    fn cf_cache_consulted_when_attached() {
        let cache = CloudflareIpCache::new_with_fallback();
        let list = TrustedProxyList::new(vec![], Some(cache));
        assert!(list.is_trusted("104.16.0.1".parse().unwrap()));
    }

    #[test]
    fn empty_list_rejects_everything() {
        let list = TrustedProxyList::new(vec![], None);
        assert!(!list.is_trusted("1.2.3.4".parse().unwrap()));
    }
}
