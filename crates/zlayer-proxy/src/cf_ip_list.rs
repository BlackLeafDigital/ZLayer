//! Cloudflare edge IP range cache.
//!
//! The proxy needs to know which inbound TCP peer IPs are Cloudflare edge
//! nodes so it can honor `CF-Connecting-IP` headers as the real client IP.
//! Cloudflare publishes its edge ranges at:
//!
//! * <https://www.cloudflare.com/ips-v4>
//! * <https://www.cloudflare.com/ips-v6>
//!
//! (newline-delimited CIDRs). This module caches those ranges and exposes a
//! fast `contains(ip)` check. It includes a baked-in fallback list for
//! boot-time fetch failures and an auto-refresh background task.

use std::net::IpAddr;
use std::str::FromStr;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use ipnet::IpNet;
use tokio::time::MissedTickBehavior;

/// Baked-in IPv4 Cloudflare edge ranges, accurate as of 2026.
///
/// Source: <https://www.cloudflare.com/ips-v4>
const FALLBACK_IPV4: &[&str] = &[
    "173.245.48.0/20",
    "103.21.244.0/22",
    "103.22.200.0/22",
    "103.31.4.0/22",
    "141.101.64.0/18",
    "108.162.192.0/18",
    "190.93.240.0/20",
    "188.114.96.0/20",
    "197.234.240.0/22",
    "198.41.128.0/17",
    "162.158.0.0/15",
    "104.16.0.0/13",
    "104.24.0.0/14",
    "172.64.0.0/13",
    "131.0.72.0/22",
];

/// Baked-in IPv6 Cloudflare edge ranges, accurate as of 2026.
///
/// Source: <https://www.cloudflare.com/ips-v6>
const FALLBACK_IPV6: &[&str] = &[
    "2400:cb00::/32",
    "2606:4700::/32",
    "2803:f800::/32",
    "2405:b500::/32",
    "2405:8100::/32",
    "2a06:98c0::/29",
    "2c0f:f248::/32",
];

const CF_IPV4_URL: &str = "https://www.cloudflare.com/ips-v4";
const CF_IPV6_URL: &str = "https://www.cloudflare.com/ips-v6";

/// In-memory cache of Cloudflare edge CIDR ranges with a fast `contains` check.
#[derive(Debug)]
pub struct CloudflareIpCache {
    ranges: RwLock<Vec<IpNet>>,
}

impl CloudflareIpCache {
    /// Build a cache populated from the hardcoded fallback list. No network.
    #[must_use]
    pub fn new_with_fallback() -> Arc<Self> {
        let ranges = parse_fallback_all();
        Arc::new(Self {
            ranges: RwLock::new(ranges),
        })
    }

    /// GET the two upstream CF URLs, parse CIDRs, and return a new cache.
    ///
    /// Falls back to the baked-in list on any error (network failure, DNS,
    /// timeout, non-200 status, parse error for both lists). If one of the
    /// two fetches succeeds and the other fails, the successful half is
    /// merged with the fallback for the failing half.
    #[must_use]
    pub async fn fetch_from_upstream() -> Arc<Self> {
        let ranges = fetch_ranges_from_upstream().await;
        Arc::new(Self {
            ranges: RwLock::new(ranges),
        })
    }

    /// Spawn a tokio task that refreshes the cache immediately, then every
    /// `interval`. The returned `Arc` is shared with the refresh task, which
    /// lives for the duration of the process.
    #[must_use]
    pub fn spawn_auto_refresh(interval: Duration) -> Arc<Self> {
        let cache = Self::new_with_fallback();
        let task_cache = Arc::clone(&cache);

        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

            // First tick yields immediately, triggering the initial fetch.
            loop {
                ticker.tick().await;
                let new_ranges = fetch_ranges_from_upstream().await;
                task_cache.replace(new_ranges);
            }
        });

        cache
    }

    /// Returns true if `ip` is within any cached CF range.
    #[must_use]
    pub fn contains(&self, ip: IpAddr) -> bool {
        let guard = match self.ranges.read() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.iter().any(|net| net.contains(&ip))
    }

    /// Replace the cached ranges atomically. Primarily used by the refresh task.
    pub fn replace(&self, new_ranges: Vec<IpNet>) {
        let mut guard = match self.ranges.write() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        *guard = new_ranges;
    }

    /// Snapshot of the current cached ranges (clones).
    #[must_use]
    pub fn ranges(&self) -> Vec<IpNet> {
        let guard = match self.ranges.read() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.clone()
    }
}

/// Parse the baked-in IPv4 fallback list.
fn parse_fallback_ipv4() -> Vec<IpNet> {
    parse_cidr_list(FALLBACK_IPV4, "fallback-ipv4")
}

/// Parse the baked-in IPv6 fallback list.
fn parse_fallback_ipv6() -> Vec<IpNet> {
    parse_cidr_list(FALLBACK_IPV6, "fallback-ipv6")
}

/// Parse both baked-in fallback lists concatenated.
fn parse_fallback_all() -> Vec<IpNet> {
    let mut ranges = parse_fallback_ipv4();
    ranges.extend(parse_fallback_ipv6());
    ranges
}

/// Parse a slice of string CIDRs into `IpNet` values, warning on any line
/// that fails to parse. The `source` label is included in warnings.
fn parse_cidr_list<S: AsRef<str>>(lines: &[S], source: &str) -> Vec<IpNet> {
    lines
        .iter()
        .filter_map(|line| {
            let trimmed = line.as_ref().trim();
            if trimmed.is_empty() {
                return None;
            }
            match IpNet::from_str(trimmed) {
                Ok(net) => Some(net),
                Err(err) => {
                    tracing::warn!(
                        source = %source,
                        line = %trimmed,
                        error = %err,
                        "failed to parse Cloudflare CIDR"
                    );
                    None
                }
            }
        })
        .collect()
}

/// Parse a newline-delimited CIDR body (as returned by cloudflare.com/ips-vN).
fn parse_cidr_body(body: &str, source: &str) -> Vec<IpNet> {
    let lines: Vec<&str> = body.lines().collect();
    parse_cidr_list(&lines, source)
}

/// Fetch a single upstream URL and parse the response into CIDRs.
async fn fetch_single(client: &reqwest::Client, url: &str) -> Result<Vec<IpNet>, reqwest::Error> {
    let body = client
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    Ok(parse_cidr_body(&body, url))
}

/// Perform the HTTP fetches for both IPv4 and IPv6 lists and merge them.
///
/// On total failure (both fetches error), returns the full baked-in fallback.
/// On partial failure, returns the successful half merged with the fallback
/// for the failing half.
async fn fetch_ranges_from_upstream() -> Vec<IpNet> {
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
    {
        Ok(c) => c,
        Err(err) => {
            tracing::error!(
                error = %err,
                "failed to build reqwest client for Cloudflare IP fetch; using fallback"
            );
            return parse_fallback_all();
        }
    };

    let (v4_result, v6_result) = tokio::join!(
        fetch_single(&client, CF_IPV4_URL),
        fetch_single(&client, CF_IPV6_URL),
    );

    match (v4_result, v6_result) {
        (Ok(v4), Ok(v6)) => {
            let mut all = v4;
            all.extend(v6);
            if all.is_empty() {
                tracing::warn!("Cloudflare upstream returned zero parseable CIDRs; using fallback");
                return parse_fallback_all();
            }
            all
        }
        (Ok(v4), Err(v6_err)) => {
            tracing::warn!(
                error = %v6_err,
                "failed to fetch Cloudflare IPv6 list; merging IPv4 fetch with fallback IPv6"
            );
            let mut all = v4;
            all.extend(parse_fallback_ipv6());
            all
        }
        (Err(v4_err), Ok(v6)) => {
            tracing::warn!(
                error = %v4_err,
                "failed to fetch Cloudflare IPv4 list; merging fallback IPv4 with IPv6 fetch"
            );
            let mut all = parse_fallback_ipv4();
            all.extend(v6);
            all
        }
        (Err(v4_err), Err(v6_err)) => {
            tracing::error!(
                ipv4_error = %v4_err,
                ipv6_error = %v6_err,
                "failed to fetch both Cloudflare IP lists; using baked-in fallback"
            );
            parse_fallback_all()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_list_is_non_empty() {
        let cache = CloudflareIpCache::new_with_fallback();
        let ranges = cache.ranges();

        let ipv4_count = ranges.iter().filter(|n| matches!(n, IpNet::V4(_))).count();
        let ipv6_count = ranges.iter().filter(|n| matches!(n, IpNet::V6(_))).count();

        assert!(
            ipv4_count >= 10,
            "expected >= 10 IPv4 ranges in fallback, got {ipv4_count}"
        );
        assert!(
            ipv6_count >= 5,
            "expected >= 5 IPv6 ranges in fallback, got {ipv6_count}"
        );
    }

    #[test]
    fn contains_known_cf_ipv4() {
        let cache = CloudflareIpCache::new_with_fallback();
        let ip: IpAddr = "104.16.0.1".parse().expect("valid ipv4");
        assert!(
            cache.contains(ip),
            "104.16.0.1 should be within 104.16.0.0/13"
        );
    }

    #[test]
    fn contains_known_cf_ipv6() {
        let cache = CloudflareIpCache::new_with_fallback();
        let ip: IpAddr = "2606:4700::1".parse().expect("valid ipv6");
        assert!(
            cache.contains(ip),
            "2606:4700::1 should be within 2606:4700::/32"
        );
    }

    #[test]
    fn rejects_non_cf_ipv4() {
        let cache = CloudflareIpCache::new_with_fallback();
        let ip: IpAddr = "8.8.8.8".parse().expect("valid ipv4");
        assert!(
            !cache.contains(ip),
            "8.8.8.8 (Google DNS) should not be in Cloudflare ranges"
        );
    }

    #[test]
    fn replace_swaps_ranges() {
        let cache = CloudflareIpCache::new_with_fallback();
        let ip: IpAddr = "104.16.0.1".parse().expect("valid ipv4");
        assert!(cache.contains(ip), "precondition: IP should be present");

        cache.replace(Vec::new());
        assert!(
            !cache.contains(ip),
            "after replace with empty vec, IP should no longer be contained"
        );
        assert!(
            cache.ranges().is_empty(),
            "ranges snapshot should be empty after replace"
        );
    }

    #[test]
    fn parse_cidr_body_skips_blank_and_bad_lines() {
        let body = "\n104.16.0.0/13\n\n   \nnot-a-cidr\n2606:4700::/32\n";
        let parsed = parse_cidr_body(body, "test");
        assert_eq!(parsed.len(), 2, "expected exactly 2 valid CIDRs parsed");
    }
}
