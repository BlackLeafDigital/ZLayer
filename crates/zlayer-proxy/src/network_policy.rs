//! Network policy access control for the reverse proxy.
//!
//! This module provides [`NetworkPolicyChecker`], which evaluates incoming
//! requests against [`NetworkPolicySpec`] access rules to determine whether
//! a source IP is allowed to reach a given service/port combination.

use std::net::IpAddr;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, warn};
use zlayer_spec::{AccessAction, AccessRule, NetworkPolicySpec};

/// Checks incoming requests against network access policies.
///
/// When a request arrives, the checker:
/// 1. Finds all networks whose CIDRs contain the source IP.
/// 2. If no networks match, access is allowed (default open).
/// 3. If any matching network has a deny rule for the target, access is denied.
/// 4. If any matching network has an allow rule for the target, access is allowed.
/// 5. If the source belongs to a network but no rules match, access is denied
///    (having a network policy implies explicit access control).
#[derive(Clone)]
pub struct NetworkPolicyChecker {
    policies: Arc<RwLock<Vec<NetworkPolicySpec>>>,
}

impl NetworkPolicyChecker {
    /// Create a new checker backed by the given shared policy list.
    pub fn new(policies: Arc<RwLock<Vec<NetworkPolicySpec>>>) -> Self {
        Self { policies }
    }

    /// Check if `source_ip` is allowed to access a target service on the given port.
    ///
    /// Returns `true` if access is allowed, `false` if denied.
    ///
    /// The `deployment` parameter exists for forward compatibility with
    /// per-deployment rules; pass `"*"` when the deployment is unknown.
    pub async fn check_access(
        &self,
        source_ip: IpAddr,
        service: &str,
        deployment: &str,
        port: u16,
    ) -> bool {
        let policies = self.policies.read().await;

        let matching_networks: Vec<&NetworkPolicySpec> = policies
            .iter()
            .filter(|p| ip_in_cidrs(source_ip, &p.cidrs))
            .collect();

        // No network policy governs this IP — allow by default.
        if matching_networks.is_empty() {
            return true;
        }

        // Phase 1: explicit deny takes priority.
        for network in &matching_networks {
            for rule in &network.access_rules {
                if rule_matches(rule, service, deployment, port)
                    && rule.action == AccessAction::Deny
                {
                    warn!(
                        source = %source_ip,
                        network = %network.name,
                        service = %service,
                        port = %port,
                        "Network policy denied access"
                    );
                    return false;
                }
            }
        }

        // Phase 2: explicit allow.
        for network in &matching_networks {
            for rule in &network.access_rules {
                if rule_matches(rule, service, deployment, port)
                    && rule.action == AccessAction::Allow
                {
                    debug!(
                        source = %source_ip,
                        network = %network.name,
                        service = %service,
                        port = %port,
                        "Network policy allowed access"
                    );
                    return true;
                }
            }
        }

        // Source is governed by a network policy but no rules matched — default deny.
        warn!(
            source = %source_ip,
            service = %service,
            port = %port,
            "Source in network policy but no matching rule; default deny"
        );
        false
    }
}

/// Returns `true` if `ip` falls within any of the given CIDR strings.
fn ip_in_cidrs(ip: IpAddr, cidrs: &[String]) -> bool {
    for cidr_str in cidrs {
        if let Some((net_str, prefix_str)) = cidr_str.split_once('/') {
            let Ok(net_addr) = net_str.parse::<IpAddr>() else {
                continue;
            };
            let Ok(prefix_len) = prefix_str.parse::<u32>() else {
                continue;
            };
            if cidr_contains(net_addr, prefix_len, ip) {
                return true;
            }
        }
    }
    false
}

/// Returns `true` if `addr` is within the CIDR `network/prefix_len`.
fn cidr_contains(network: IpAddr, prefix_len: u32, addr: IpAddr) -> bool {
    match (network, addr) {
        (IpAddr::V4(net), IpAddr::V4(ip)) => {
            let prefix_len = prefix_len.min(32);
            if prefix_len == 0 {
                return true;
            }
            let mask = u32::MAX.checked_shl(32 - prefix_len).unwrap_or(0);
            (u32::from(net) & mask) == (u32::from(ip) & mask)
        }
        (IpAddr::V6(net), IpAddr::V6(ip)) => {
            let prefix_len = prefix_len.min(128);
            if prefix_len == 0 {
                return true;
            }
            let mask = u128::MAX.checked_shl(128 - prefix_len).unwrap_or(0);
            (u128::from(net) & mask) == (u128::from(ip) & mask)
        }
        _ => false, // v4 vs v6 mismatch
    }
}

/// Check whether a single access rule matches the given target.
fn rule_matches(rule: &AccessRule, service: &str, deployment: &str, port: u16) -> bool {
    let service_match = rule.service == "*" || rule.service == service;
    let deployment_match = rule.deployment == "*" || rule.deployment == deployment;
    let port_match = rule
        .ports
        .as_ref()
        .is_none_or(|ports| ports.contains(&port));
    service_match && deployment_match && port_match
}

#[cfg(test)]
mod tests {
    use super::*;
    use zlayer_spec::{AccessAction, AccessRule, NetworkPolicySpec};

    fn make_policy(name: &str, cidrs: Vec<&str>, rules: Vec<AccessRule>) -> NetworkPolicySpec {
        NetworkPolicySpec {
            name: name.to_string(),
            cidrs: cidrs.into_iter().map(String::from).collect(),
            access_rules: rules,
            ..Default::default()
        }
    }

    fn allow_rule(service: &str, deployment: &str, ports: Option<Vec<u16>>) -> AccessRule {
        AccessRule {
            service: service.to_string(),
            deployment: deployment.to_string(),
            ports,
            action: AccessAction::Allow,
        }
    }

    fn deny_rule(service: &str, deployment: &str, ports: Option<Vec<u16>>) -> AccessRule {
        AccessRule {
            service: service.to_string(),
            deployment: deployment.to_string(),
            ports,
            action: AccessAction::Deny,
        }
    }

    #[tokio::test]
    async fn test_no_matching_network_allows() {
        let policies = Arc::new(RwLock::new(vec![make_policy(
            "corp",
            vec!["10.0.0.0/8"],
            vec![allow_rule("api", "*", None)],
        )]));
        let checker = NetworkPolicyChecker::new(policies);

        // 192.168.1.1 is not in 10.0.0.0/8 — should allow.
        assert!(
            checker
                .check_access("192.168.1.1".parse().unwrap(), "api", "*", 8080)
                .await
        );
    }

    #[tokio::test]
    async fn test_matching_allow_rule() {
        let policies = Arc::new(RwLock::new(vec![make_policy(
            "corp",
            vec!["10.0.0.0/8"],
            vec![allow_rule("api", "*", None)],
        )]));
        let checker = NetworkPolicyChecker::new(policies);

        // 10.1.2.3 is in 10.0.0.0/8 and rule allows "api" — should allow.
        assert!(
            checker
                .check_access("10.1.2.3".parse().unwrap(), "api", "*", 8080)
                .await
        );
    }

    #[tokio::test]
    async fn test_matching_deny_rule() {
        let policies = Arc::new(RwLock::new(vec![make_policy(
            "restricted",
            vec!["10.0.0.0/8"],
            vec![deny_rule("admin", "*", None)],
        )]));
        let checker = NetworkPolicyChecker::new(policies);

        // 10.1.2.3 is in 10.0.0.0/8 and rule denies "admin" — should deny.
        assert!(
            !checker
                .check_access("10.1.2.3".parse().unwrap(), "admin", "*", 443)
                .await
        );
    }

    #[tokio::test]
    async fn test_deny_takes_priority_over_allow() {
        let policies = Arc::new(RwLock::new(vec![make_policy(
            "mixed",
            vec!["10.0.0.0/8"],
            vec![
                allow_rule("api", "*", None),
                deny_rule("api", "*", Some(vec![9090])),
            ],
        )]));
        let checker = NetworkPolicyChecker::new(policies);

        // Port 8080 is allowed (no deny matches port 8080).
        assert!(
            checker
                .check_access("10.1.2.3".parse().unwrap(), "api", "*", 8080)
                .await
        );

        // Port 9090 is denied (deny rule matches).
        assert!(
            !checker
                .check_access("10.1.2.3".parse().unwrap(), "api", "*", 9090)
                .await
        );
    }

    #[tokio::test]
    async fn test_network_but_no_matching_rule_denies() {
        let policies = Arc::new(RwLock::new(vec![make_policy(
            "corp",
            vec!["10.0.0.0/8"],
            vec![allow_rule("api", "*", None)],
        )]));
        let checker = NetworkPolicyChecker::new(policies);

        // 10.1.2.3 is in the network, but "frontend" has no matching rule — default deny.
        assert!(
            !checker
                .check_access("10.1.2.3".parse().unwrap(), "frontend", "*", 80)
                .await
        );
    }

    #[tokio::test]
    async fn test_wildcard_service_rule() {
        let policies = Arc::new(RwLock::new(vec![make_policy(
            "admin-net",
            vec!["172.16.0.0/12"],
            vec![allow_rule("*", "*", None)],
        )]));
        let checker = NetworkPolicyChecker::new(policies);

        // Wildcard service rule should match any service.
        assert!(
            checker
                .check_access("172.16.5.10".parse().unwrap(), "anything", "*", 443)
                .await
        );
    }

    #[tokio::test]
    async fn test_port_restriction() {
        let policies = Arc::new(RwLock::new(vec![make_policy(
            "web",
            vec!["10.200.0.0/16"],
            vec![allow_rule("api", "*", Some(vec![80, 443]))],
        )]));
        let checker = NetworkPolicyChecker::new(policies);

        // Port 443 — allowed.
        assert!(
            checker
                .check_access("10.200.1.1".parse().unwrap(), "api", "*", 443)
                .await
        );

        // Port 8080 — not in ports list, no matching rule — default deny.
        assert!(
            !checker
                .check_access("10.200.1.1".parse().unwrap(), "api", "*", 8080)
                .await
        );
    }

    #[tokio::test]
    async fn test_multiple_networks() {
        let policies = Arc::new(RwLock::new(vec![
            make_policy(
                "office",
                vec!["192.168.1.0/24"],
                vec![allow_rule("api", "*", None)],
            ),
            make_policy(
                "vpn",
                vec!["10.200.0.0/16"],
                vec![allow_rule("*", "*", None)],
            ),
        ]));
        let checker = NetworkPolicyChecker::new(policies);

        // Office user can reach "api" but not "admin".
        assert!(
            checker
                .check_access("192.168.1.50".parse().unwrap(), "api", "*", 80)
                .await
        );
        assert!(
            !checker
                .check_access("192.168.1.50".parse().unwrap(), "admin", "*", 80)
                .await
        );

        // VPN user can reach anything.
        assert!(
            checker
                .check_access("10.200.5.5".parse().unwrap(), "admin", "*", 80)
                .await
        );
    }

    #[tokio::test]
    async fn test_empty_policies_allows_all() {
        let policies = Arc::new(RwLock::new(Vec::new()));
        let checker = NetworkPolicyChecker::new(policies);

        assert!(
            checker
                .check_access("1.2.3.4".parse().unwrap(), "anything", "*", 80)
                .await
        );
    }

    #[test]
    fn test_ip_in_cidrs_v4() {
        let cidrs = vec!["10.0.0.0/8".to_string(), "192.168.1.0/24".to_string()];

        assert!(ip_in_cidrs("10.1.2.3".parse().unwrap(), &cidrs));
        assert!(ip_in_cidrs("192.168.1.100".parse().unwrap(), &cidrs));
        assert!(!ip_in_cidrs("172.16.0.1".parse().unwrap(), &cidrs));
    }

    #[test]
    fn test_ip_in_cidrs_v6() {
        let cidrs = vec!["fd00::/64".to_string()];

        assert!(ip_in_cidrs("fd00::1".parse().unwrap(), &cidrs));
        assert!(!ip_in_cidrs("fd01::1".parse().unwrap(), &cidrs));
    }

    #[test]
    fn test_ip_in_cidrs_empty() {
        assert!(!ip_in_cidrs("10.0.0.1".parse().unwrap(), &[]));
    }

    #[test]
    fn test_rule_matches_wildcards() {
        let rule = allow_rule("*", "*", None);
        assert!(rule_matches(&rule, "any-service", "any-deployment", 12345));
    }

    #[test]
    fn test_rule_matches_specific() {
        let rule = allow_rule("api", "prod", Some(vec![443]));

        assert!(rule_matches(&rule, "api", "prod", 443));
        assert!(!rule_matches(&rule, "api", "staging", 443));
        assert!(!rule_matches(&rule, "web", "prod", 443));
        assert!(!rule_matches(&rule, "api", "prod", 80));
    }
}
