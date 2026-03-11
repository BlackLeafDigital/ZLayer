//! IP address allocation for overlay networks
//!
//! Manages allocation and tracking of overlay IP addresses within a CIDR range.
//! Supports both IPv4 and IPv6 (dual-stack) networks.

use crate::error::{OverlayError, Result};
use ipnet::IpNet;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::net::{IpAddr, Ipv6Addr};
use std::path::Path;

/// IP allocator for overlay network addresses
///
/// Tracks allocated IP addresses and provides next-available allocation
/// from a configured CIDR range. Supports both IPv4 and IPv6 networks.
#[derive(Debug, Clone)]
pub struct IpAllocator {
    /// Network CIDR range (IPv4 or IPv6)
    network: IpNet,
    /// Set of allocated IP addresses
    allocated: HashSet<IpAddr>,
}

/// Persistent state for IP allocator
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpAllocatorState {
    /// CIDR string
    pub cidr: String,
    /// List of allocated IPs (serializes as strings, backward-compatible)
    pub allocated: Vec<IpAddr>,
}

/// Increment an IPv6 address by a u128 offset from a base address.
///
/// Returns `None` if the result would overflow.
fn ipv6_add(base: Ipv6Addr, offset: u128) -> Option<Ipv6Addr> {
    let base_u128 = u128::from(base);
    base_u128.checked_add(offset).map(Ipv6Addr::from)
}

/// Compute the number of host addresses for a given address family and prefix length.
///
/// For IPv4: `2^(32 - prefix) - 2` (excludes network and broadcast).
/// For IPv6: `2^(128 - prefix) - 1` (excludes the network address).
///
/// Returns `None` if the result overflows u128 (only for /0 edge cases).
fn host_count(is_ipv6: bool, prefix_len: u8) -> u128 {
    if is_ipv6 {
        let bits = 128 - u32::from(prefix_len);
        if bits == 128 {
            // /0 network — saturate
            u128::MAX
        } else if bits == 0 {
            // /128 — single host, no usable addresses (it IS the network address)
            0
        } else {
            // 2^bits - 1 (skip network address)
            (1u128 << bits) - 1
        }
    } else {
        let bits = 32 - u32::from(prefix_len);
        if bits <= 1 {
            // /31 or /32 — no usable hosts in classical networking
            0
        } else {
            // 2^bits - 2 (skip network and broadcast)
            (1u128 << bits) - 2
        }
    }
}

impl IpAllocator {
    /// Create a new IP allocator for the given CIDR range
    ///
    /// Supports both IPv4 (e.g., "10.200.0.0/16") and IPv6 (e.g., `fd00::/48`).
    ///
    /// # Arguments
    /// * `cidr` - Network CIDR notation
    ///
    /// # Errors
    ///
    /// Returns `OverlayError::InvalidCidr` if the CIDR string cannot be parsed.
    ///
    /// # Example
    /// ```
    /// use zlayer_overlay_zql::allocator::IpAllocator;
    ///
    /// let v4 = IpAllocator::new("10.200.0.0/16").unwrap();
    /// let v6 = IpAllocator::new("fd00::/48").unwrap();
    /// ```
    pub fn new(cidr: &str) -> Result<Self> {
        let network: IpNet = cidr
            .parse()
            .map_err(|e| OverlayError::InvalidCidr(format!("{cidr}: {e}")))?;

        Ok(Self {
            network,
            allocated: HashSet::new(),
        })
    }

    /// Create an allocator from persisted state
    ///
    /// # Errors
    ///
    /// Returns an error if the CIDR is invalid or any IP is out of range.
    pub fn from_state(state: IpAllocatorState) -> Result<Self> {
        let mut allocator = Self::new(&state.cidr)?;
        for ip in state.allocated {
            allocator.mark_allocated(ip)?;
        }
        Ok(allocator)
    }

    /// Get the current state for persistence
    #[must_use]
    pub fn to_state(&self) -> IpAllocatorState {
        IpAllocatorState {
            cidr: self.network.to_string(),
            allocated: self.allocated.iter().copied().collect(),
        }
    }

    /// Load allocator state from a file
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read or the state is invalid.
    pub async fn load(path: &Path) -> Result<Self> {
        let contents = tokio::fs::read_to_string(path).await?;
        let state: IpAllocatorState = serde_json::from_str(&contents)?;
        Self::from_state(state)
    }

    /// Save allocator state to a file
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be written or serialization fails.
    pub async fn save(&self, path: &Path) -> Result<()> {
        let state = self.to_state();
        let contents = serde_json::to_string_pretty(&state)?;
        tokio::fs::write(path, contents).await?;
        Ok(())
    }

    /// Allocate the next available IP address
    ///
    /// For IPv4, skips the network and broadcast addresses.
    /// For IPv6, skips the network address.
    ///
    /// Returns `None` if all addresses in the CIDR range are allocated.
    ///
    /// # Example
    /// ```
    /// use zlayer_overlay_zql::allocator::IpAllocator;
    ///
    /// let mut allocator = IpAllocator::new("10.200.0.0/24").unwrap();
    /// let ip = allocator.allocate().unwrap();
    /// assert_eq!(ip.to_string(), "10.200.0.1");
    /// ```
    pub fn allocate(&mut self) -> Option<IpAddr> {
        match self.network {
            IpNet::V4(v4net) => {
                // IPv4: iterate hosts() which skips network and broadcast
                for ip in v4net.hosts() {
                    let addr = IpAddr::V4(ip);
                    if !self.allocated.contains(&addr) {
                        self.allocated.insert(addr);
                        return Some(addr);
                    }
                }
                None
            }
            IpNet::V6(v6net) => {
                // IPv6: counter-based allocation starting from base+1
                // We skip the network address itself (offset 0) and allocate from offset 1.
                let base = v6net.network();
                let total = host_count(true, v6net.prefix_len());

                for offset in 1..=total {
                    if let Some(candidate) = ipv6_add(base, offset) {
                        let addr = IpAddr::V6(candidate);
                        if !self.allocated.contains(&addr) {
                            self.allocated.insert(addr);
                            return Some(addr);
                        }
                    } else {
                        break;
                    }
                }
                None
            }
        }
    }

    /// Allocate a specific IP address
    ///
    /// # Errors
    ///
    /// Returns an error if the IP is already allocated or not in the CIDR range.
    pub fn allocate_specific(&mut self, ip: IpAddr) -> Result<()> {
        if !self.network.contains(&ip) {
            return Err(OverlayError::IpNotInRange(ip, self.network.to_string()));
        }

        if self.allocated.contains(&ip) {
            return Err(OverlayError::IpAlreadyAllocated(ip));
        }

        self.allocated.insert(ip);
        Ok(())
    }

    /// Allocate the first usable IP in the range (typically for the leader)
    ///
    /// # Example
    /// ```
    /// use zlayer_overlay_zql::allocator::IpAllocator;
    ///
    /// let mut allocator = IpAllocator::new("10.200.0.0/24").unwrap();
    /// let ip = allocator.allocate_first().unwrap();
    /// assert_eq!(ip.to_string(), "10.200.0.1");
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error if no IPs are available or the first IP is already allocated.
    pub fn allocate_first(&mut self) -> Result<IpAddr> {
        let first_ip = self.first_host().ok_or(OverlayError::NoAvailableIps)?;

        if self.allocated.contains(&first_ip) {
            return Err(OverlayError::IpAlreadyAllocated(first_ip));
        }

        self.allocated.insert(first_ip);
        Ok(first_ip)
    }

    /// Get the first usable host address in the network.
    ///
    /// For IPv4: first host from `hosts()` (skips network address).
    /// For IPv6: network address + 1 (skips the network address).
    fn first_host(&self) -> Option<IpAddr> {
        match self.network {
            IpNet::V4(v4net) => v4net.hosts().next().map(IpAddr::V4),
            IpNet::V6(v6net) => {
                let base = v6net.network();
                ipv6_add(base, 1).map(IpAddr::V6)
            }
        }
    }

    /// Mark an IP address as allocated (for restoring state)
    ///
    /// # Errors
    ///
    /// Returns an error if the IP is not in the CIDR range.
    pub fn mark_allocated(&mut self, ip: IpAddr) -> Result<()> {
        if !self.network.contains(&ip) {
            return Err(OverlayError::IpNotInRange(ip, self.network.to_string()));
        }
        self.allocated.insert(ip);
        Ok(())
    }

    /// Release an IP address back to the pool
    ///
    /// Returns `true` if the IP was released, `false` if it wasn't allocated.
    pub fn release(&mut self, ip: IpAddr) -> bool {
        self.allocated.remove(&ip)
    }

    /// Check if an IP address is allocated
    #[must_use]
    pub fn is_allocated(&self, ip: IpAddr) -> bool {
        self.allocated.contains(&ip)
    }

    /// Check if an IP address is within the CIDR range
    #[must_use]
    pub fn contains(&self, ip: IpAddr) -> bool {
        self.network.contains(&ip)
    }

    /// Get the number of allocated addresses
    #[must_use]
    pub fn allocated_count(&self) -> usize {
        self.allocated.len()
    }

    /// Get the total number of usable addresses in the range
    ///
    /// For IPv6 networks with large host spaces, this saturates at `u32::MAX`.
    #[must_use]
    #[allow(clippy::cast_possible_truncation)]
    pub fn total_hosts(&self) -> u32 {
        let is_v6 = matches!(self.network, IpNet::V6(_));
        let count = host_count(is_v6, self.network.prefix_len());
        // Saturate to u32::MAX for enormous IPv6 subnets
        if count > u128::from(u32::MAX) {
            u32::MAX
        } else {
            count as u32
        }
    }

    /// Get the number of available addresses
    #[must_use]
    #[allow(clippy::cast_possible_truncation)]
    pub fn available_count(&self) -> u32 {
        self.total_hosts()
            .saturating_sub(self.allocated.len() as u32)
    }

    /// Get the CIDR string
    #[must_use]
    pub fn cidr(&self) -> String {
        self.network.to_string()
    }

    /// Get the network address
    #[must_use]
    pub fn network_addr(&self) -> IpAddr {
        self.network.network()
    }

    /// Get the broadcast address
    ///
    /// For IPv6, returns the last address in the range (all host bits set to 1).
    #[must_use]
    pub fn broadcast_addr(&self) -> IpAddr {
        self.network.broadcast()
    }

    /// Get the prefix length
    #[must_use]
    pub fn prefix_len(&self) -> u8 {
        self.network.prefix_len()
    }

    /// Get the host prefix length (32 for IPv4, 128 for IPv6)
    #[must_use]
    pub fn host_prefix_len(&self) -> u8 {
        self.network.max_prefix_len()
    }

    /// Get all allocated IPs
    #[must_use]
    pub fn allocated_ips(&self) -> Vec<IpAddr> {
        self.allocated.iter().copied().collect()
    }
}

/// Helper function to get the first usable IP from a CIDR
///
/// Supports both IPv4 and IPv6 CIDR notation.
///
/// # Errors
///
/// Returns an error if the CIDR is invalid or has no usable hosts.
pub fn first_ip_from_cidr(cidr: &str) -> Result<IpAddr> {
    let network: IpNet = cidr
        .parse()
        .map_err(|e| OverlayError::InvalidCidr(format!("{cidr}: {e}")))?;

    match network {
        IpNet::V4(v4net) => v4net
            .hosts()
            .next()
            .map(IpAddr::V4)
            .ok_or(OverlayError::NoAvailableIps),
        IpNet::V6(v6net) => {
            let base = v6net.network();
            ipv6_add(base, 1)
                .map(IpAddr::V6)
                .ok_or(OverlayError::NoAvailableIps)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    /// Increment an IPv4 address by a u32 offset from a base address.
    ///
    /// Returns `None` if the result would overflow.
    fn ipv4_add(base: Ipv4Addr, offset: u32) -> Option<Ipv4Addr> {
        let base_u32 = u32::from(base);
        base_u32.checked_add(offset).map(Ipv4Addr::from)
    }

    // ========================
    // IPv4 Tests (existing, updated for IpAddr)
    // ========================

    #[test]
    fn test_allocator_new() {
        let allocator = IpAllocator::new("10.200.0.0/24").unwrap();
        assert_eq!(allocator.cidr(), "10.200.0.0/24");
        assert_eq!(allocator.allocated_count(), 0);
    }

    #[test]
    fn test_allocator_invalid_cidr() {
        let result = IpAllocator::new("invalid");
        assert!(result.is_err());
    }

    #[test]
    fn test_allocate_sequential() {
        let mut allocator = IpAllocator::new("10.200.0.0/30").unwrap();

        // /30 has 2 usable hosts (excluding network and broadcast)
        let ip1 = allocator.allocate().unwrap();
        let ip2 = allocator.allocate().unwrap();

        assert_eq!(ip1.to_string(), "10.200.0.1");
        assert_eq!(ip2.to_string(), "10.200.0.2");

        // Should be exhausted
        assert!(allocator.allocate().is_none());
    }

    #[test]
    fn test_allocate_first() {
        let mut allocator = IpAllocator::new("10.200.0.0/24").unwrap();

        let first = allocator.allocate_first().unwrap();
        assert_eq!(first.to_string(), "10.200.0.1");

        // Can't allocate first again
        assert!(allocator.allocate_first().is_err());
    }

    #[test]
    fn test_allocate_specific() {
        let mut allocator = IpAllocator::new("10.200.0.0/24").unwrap();

        let specific_ip: IpAddr = "10.200.0.50".parse().unwrap();
        allocator.allocate_specific(specific_ip).unwrap();

        assert!(allocator.is_allocated(specific_ip));

        // Can't allocate same IP again
        assert!(allocator.allocate_specific(specific_ip).is_err());
    }

    #[test]
    fn test_allocate_specific_out_of_range() {
        let mut allocator = IpAllocator::new("10.200.0.0/24").unwrap();

        let out_of_range: IpAddr = "192.168.1.1".parse().unwrap();
        assert!(allocator.allocate_specific(out_of_range).is_err());
    }

    #[test]
    fn test_release() {
        let mut allocator = IpAllocator::new("10.200.0.0/24").unwrap();

        let ip = allocator.allocate().unwrap();
        assert!(allocator.is_allocated(ip));

        assert!(allocator.release(ip));
        assert!(!allocator.is_allocated(ip));

        // Can allocate same IP again
        let ip2 = allocator.allocate().unwrap();
        assert_eq!(ip, ip2);
    }

    #[test]
    fn test_mark_allocated() {
        let mut allocator = IpAllocator::new("10.200.0.0/24").unwrap();

        let ip: IpAddr = "10.200.0.100".parse().unwrap();
        allocator.mark_allocated(ip).unwrap();

        assert!(allocator.is_allocated(ip));
    }

    #[test]
    fn test_contains() {
        let allocator = IpAllocator::new("10.200.0.0/24").unwrap();

        assert!(allocator.contains("10.200.0.50".parse().unwrap()));
        assert!(!allocator.contains("10.201.0.50".parse().unwrap()));
    }

    #[test]
    fn test_total_hosts() {
        // /24 has 254 usable hosts
        let allocator = IpAllocator::new("10.200.0.0/24").unwrap();
        assert_eq!(allocator.total_hosts(), 254);

        // /30 has 2 usable hosts
        let allocator = IpAllocator::new("10.200.0.0/30").unwrap();
        assert_eq!(allocator.total_hosts(), 2);
    }

    #[test]
    fn test_available_count() {
        let mut allocator = IpAllocator::new("10.200.0.0/30").unwrap();

        assert_eq!(allocator.available_count(), 2);

        allocator.allocate();
        assert_eq!(allocator.available_count(), 1);

        allocator.allocate();
        assert_eq!(allocator.available_count(), 0);
    }

    #[test]
    fn test_state_roundtrip() {
        let mut allocator = IpAllocator::new("10.200.0.0/24").unwrap();
        allocator.allocate();
        allocator.allocate();

        let state = allocator.to_state();
        let restored = IpAllocator::from_state(state).unwrap();

        assert_eq!(allocator.cidr(), restored.cidr());
        assert_eq!(allocator.allocated_count(), restored.allocated_count());
    }

    #[test]
    fn test_first_ip_from_cidr() {
        let ip = first_ip_from_cidr("10.200.0.0/24").unwrap();
        assert_eq!(ip.to_string(), "10.200.0.1");
    }

    #[test]
    fn test_network_addr_v4() {
        let allocator = IpAllocator::new("10.200.0.0/24").unwrap();
        assert_eq!(
            allocator.network_addr(),
            IpAddr::V4("10.200.0.0".parse().unwrap())
        );
    }

    #[test]
    fn test_broadcast_addr_v4() {
        let allocator = IpAllocator::new("10.200.0.0/24").unwrap();
        assert_eq!(
            allocator.broadcast_addr(),
            IpAddr::V4("10.200.0.255".parse().unwrap())
        );
    }

    #[test]
    fn test_host_prefix_len_v4() {
        let allocator = IpAllocator::new("10.200.0.0/24").unwrap();
        assert_eq!(allocator.host_prefix_len(), 32);
    }

    // ========================
    // IPv6 Tests
    // ========================

    #[test]
    fn test_allocator_new_v6() {
        let allocator = IpAllocator::new("fd00::/48").unwrap();
        assert_eq!(allocator.cidr(), "fd00::/48");
        assert_eq!(allocator.allocated_count(), 0);
    }

    #[test]
    fn test_allocate_sequential_v6() {
        let mut allocator = IpAllocator::new("fd00::/126").unwrap();

        // /126 has 3 usable hosts (4 addresses total, minus the network address)
        let ip1 = allocator.allocate().unwrap();
        let ip2 = allocator.allocate().unwrap();
        let ip3 = allocator.allocate().unwrap();

        assert_eq!(ip1.to_string(), "fd00::1");
        assert_eq!(ip2.to_string(), "fd00::2");
        assert_eq!(ip3.to_string(), "fd00::3");

        // Should be exhausted
        assert!(allocator.allocate().is_none());
    }

    #[test]
    fn test_allocate_first_v6() {
        let mut allocator = IpAllocator::new("fd00::/48").unwrap();

        let first = allocator.allocate_first().unwrap();
        assert_eq!(first.to_string(), "fd00::1");

        // Can't allocate first again
        assert!(allocator.allocate_first().is_err());
    }

    #[test]
    fn test_allocate_specific_v6() {
        let mut allocator = IpAllocator::new("fd00::/48").unwrap();

        let specific_ip: IpAddr = "fd00::beef".parse().unwrap();
        allocator.allocate_specific(specific_ip).unwrap();

        assert!(allocator.is_allocated(specific_ip));

        // Can't allocate same IP again
        assert!(allocator.allocate_specific(specific_ip).is_err());
    }

    #[test]
    fn test_allocate_specific_out_of_range_v6() {
        let mut allocator = IpAllocator::new("fd00::/48").unwrap();

        let out_of_range: IpAddr = "fe80::1".parse().unwrap();
        assert!(allocator.allocate_specific(out_of_range).is_err());
    }

    #[test]
    fn test_release_v6() {
        let mut allocator = IpAllocator::new("fd00::/48").unwrap();

        let ip = allocator.allocate().unwrap();
        assert!(allocator.is_allocated(ip));

        assert!(allocator.release(ip));
        assert!(!allocator.is_allocated(ip));

        // Can allocate same IP again
        let ip2 = allocator.allocate().unwrap();
        assert_eq!(ip, ip2);
    }

    #[test]
    fn test_mark_allocated_v6() {
        let mut allocator = IpAllocator::new("fd00::/48").unwrap();

        let ip: IpAddr = "fd00::ff".parse().unwrap();
        allocator.mark_allocated(ip).unwrap();

        assert!(allocator.is_allocated(ip));
    }

    #[test]
    fn test_contains_v6() {
        let allocator = IpAllocator::new("fd00::/48").unwrap();

        assert!(allocator.contains("fd00::50".parse().unwrap()));
        assert!(!allocator.contains("fe80::1".parse().unwrap()));
    }

    #[test]
    fn test_total_hosts_v6_small() {
        // /126 has 3 usable hosts (skip network addr)
        let allocator = IpAllocator::new("fd00::/126").unwrap();
        assert_eq!(allocator.total_hosts(), 3);

        // /127 has 1 usable host
        let allocator = IpAllocator::new("fd00::/127").unwrap();
        assert_eq!(allocator.total_hosts(), 1);
    }

    #[test]
    fn test_total_hosts_v6_large() {
        // /48 has 2^80 - 1 usable hosts, which saturates to u32::MAX
        let allocator = IpAllocator::new("fd00::/48").unwrap();
        assert_eq!(allocator.total_hosts(), u32::MAX);
    }

    #[test]
    fn test_available_count_v6() {
        let mut allocator = IpAllocator::new("fd00::/126").unwrap();

        assert_eq!(allocator.available_count(), 3);

        allocator.allocate();
        assert_eq!(allocator.available_count(), 2);

        allocator.allocate();
        assert_eq!(allocator.available_count(), 1);

        allocator.allocate();
        assert_eq!(allocator.available_count(), 0);
    }

    #[test]
    fn test_state_roundtrip_v6() {
        let mut allocator = IpAllocator::new("fd00::/48").unwrap();
        allocator.allocate();
        allocator.allocate();

        let state = allocator.to_state();

        // Verify IpAddr serializes as strings (backward-compatible)
        let json = serde_json::to_string_pretty(&state).unwrap();
        assert!(json.contains("fd00::1"));
        assert!(json.contains("fd00::2"));

        let restored = IpAllocator::from_state(state).unwrap();

        assert_eq!(allocator.cidr(), restored.cidr());
        assert_eq!(allocator.allocated_count(), restored.allocated_count());
    }

    #[test]
    fn test_first_ip_from_cidr_v6() {
        let ip = first_ip_from_cidr("fd00::/48").unwrap();
        assert_eq!(ip.to_string(), "fd00::1");
    }

    #[test]
    fn test_network_addr_v6() {
        let allocator = IpAllocator::new("fd00::/48").unwrap();
        assert_eq!(
            allocator.network_addr(),
            IpAddr::V6("fd00::".parse().unwrap())
        );
    }

    #[test]
    fn test_broadcast_addr_v6() {
        let allocator = IpAllocator::new("fd00::/126").unwrap();
        assert_eq!(
            allocator.broadcast_addr(),
            IpAddr::V6("fd00::3".parse().unwrap())
        );
    }

    #[test]
    fn test_host_prefix_len_v6() {
        let allocator = IpAllocator::new("fd00::/48").unwrap();
        assert_eq!(allocator.host_prefix_len(), 128);
    }

    // ========================
    // Cross-protocol tests
    // ========================

    #[test]
    fn test_v4_and_v6_allocators_independent() {
        let mut v4 = IpAllocator::new("10.200.0.0/30").unwrap();
        let mut v6 = IpAllocator::new("fd00::/126").unwrap();

        let v4_ip = v4.allocate().unwrap();
        let v6_ip = v6.allocate().unwrap();

        assert!(v4_ip.is_ipv4());
        assert!(v6_ip.is_ipv6());
        assert_eq!(v4_ip.to_string(), "10.200.0.1");
        assert_eq!(v6_ip.to_string(), "fd00::1");
    }

    #[test]
    fn test_ipv6_does_not_contain_ipv4() {
        let allocator = IpAllocator::new("fd00::/48").unwrap();
        assert!(!allocator.contains("10.200.0.1".parse().unwrap()));
    }

    #[test]
    fn test_ipv4_does_not_contain_ipv6() {
        let allocator = IpAllocator::new("10.200.0.0/24").unwrap();
        assert!(!allocator.contains("fd00::1".parse().unwrap()));
    }

    #[test]
    fn test_allocate_specific_wrong_family() {
        let mut v4_alloc = IpAllocator::new("10.200.0.0/24").unwrap();
        let v6_ip: IpAddr = "fd00::1".parse().unwrap();
        assert!(v4_alloc.allocate_specific(v6_ip).is_err());

        let mut v6_alloc = IpAllocator::new("fd00::/48").unwrap();
        let v4_ip: IpAddr = "10.200.0.1".parse().unwrap();
        assert!(v6_alloc.allocate_specific(v4_ip).is_err());
    }

    // ========================
    // Helper function tests
    // ========================

    #[test]
    fn test_ipv4_add() {
        let base: Ipv4Addr = "10.0.0.0".parse().unwrap();
        assert_eq!(ipv4_add(base, 1), Some("10.0.0.1".parse().unwrap()));
        assert_eq!(ipv4_add(base, 256), Some("10.0.1.0".parse().unwrap()));
    }

    #[test]
    fn test_ipv4_add_overflow() {
        let base: Ipv4Addr = "255.255.255.255".parse().unwrap();
        assert_eq!(ipv4_add(base, 1), None);
    }

    #[test]
    fn test_ipv6_add() {
        let base: Ipv6Addr = "fd00::".parse().unwrap();
        assert_eq!(ipv6_add(base, 1), Some("fd00::1".parse().unwrap()));
        assert_eq!(ipv6_add(base, 0xffff), Some("fd00::ffff".parse().unwrap()));
    }

    #[test]
    fn test_ipv6_add_overflow() {
        let base: Ipv6Addr = "ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff".parse().unwrap();
        assert_eq!(ipv6_add(base, 1), None);
    }

    #[test]
    fn test_host_count_v4() {
        assert_eq!(host_count(false, 24), 254); // 2^8 - 2
        assert_eq!(host_count(false, 30), 2); // 2^2 - 2
        assert_eq!(host_count(false, 16), 65534); // 2^16 - 2
        assert_eq!(host_count(false, 31), 0); // /31 — no classical hosts
        assert_eq!(host_count(false, 32), 0); // /32 — single address
    }

    #[test]
    fn test_host_count_v6() {
        assert_eq!(host_count(true, 126), 3); // 2^2 - 1
        assert_eq!(host_count(true, 127), 1); // 2^1 - 1
        assert_eq!(host_count(true, 128), 0); // /128 — single address (is network addr)
        assert_eq!(host_count(true, 64), (1u128 << 64) - 1); // 2^64 - 1
    }
}
