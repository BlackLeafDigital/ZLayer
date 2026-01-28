//! IP address allocation for overlay networks
//!
//! Manages allocation and tracking of overlay IP addresses within a CIDR range.

use crate::error::{OverlayError, Result};
use ipnet::Ipv4Net;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::net::Ipv4Addr;
use std::path::Path;

/// IP allocator for overlay network addresses
///
/// Tracks allocated IP addresses and provides next-available allocation
/// from a configured CIDR range.
#[derive(Debug, Clone)]
pub struct IpAllocator {
    /// Network CIDR range
    network: Ipv4Net,
    /// Set of allocated IP addresses
    allocated: HashSet<Ipv4Addr>,
}

/// Persistent state for IP allocator
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpAllocatorState {
    /// CIDR string
    pub cidr: String,
    /// List of allocated IPs
    pub allocated: Vec<Ipv4Addr>,
}

impl IpAllocator {
    /// Create a new IP allocator for the given CIDR range
    ///
    /// # Arguments
    /// * `cidr` - Network CIDR notation (e.g., "10.200.0.0/16")
    ///
    /// # Example
    /// ```
    /// use zlayer_overlay::allocator::IpAllocator;
    ///
    /// let allocator = IpAllocator::new("10.200.0.0/16").unwrap();
    /// ```
    pub fn new(cidr: &str) -> Result<Self> {
        let network: Ipv4Net = cidr
            .parse()
            .map_err(|e| OverlayError::InvalidCidr(format!("{}: {}", cidr, e)))?;

        Ok(Self {
            network,
            allocated: HashSet::new(),
        })
    }

    /// Create an allocator from persisted state
    pub fn from_state(state: IpAllocatorState) -> Result<Self> {
        let mut allocator = Self::new(&state.cidr)?;
        for ip in state.allocated {
            allocator.mark_allocated(ip)?;
        }
        Ok(allocator)
    }

    /// Get the current state for persistence
    pub fn to_state(&self) -> IpAllocatorState {
        IpAllocatorState {
            cidr: self.network.to_string(),
            allocated: self.allocated.iter().copied().collect(),
        }
    }

    /// Load allocator state from a file
    pub async fn load(path: &Path) -> Result<Self> {
        let contents = tokio::fs::read_to_string(path).await?;
        let state: IpAllocatorState = serde_json::from_str(&contents)?;
        Self::from_state(state)
    }

    /// Save allocator state to a file
    pub async fn save(&self, path: &Path) -> Result<()> {
        let state = self.to_state();
        let contents = serde_json::to_string_pretty(&state)?;
        tokio::fs::write(path, contents).await?;
        Ok(())
    }

    /// Allocate the next available IP address
    ///
    /// Returns `None` if all addresses in the CIDR range are allocated.
    ///
    /// # Example
    /// ```
    /// use zlayer_overlay::allocator::IpAllocator;
    ///
    /// let mut allocator = IpAllocator::new("10.200.0.0/24").unwrap();
    /// let ip = allocator.allocate().unwrap();
    /// assert_eq!(ip.to_string(), "10.200.0.1");
    /// ```
    pub fn allocate(&mut self) -> Option<Ipv4Addr> {
        // Skip network address and broadcast address
        for ip in self.network.hosts() {
            if !self.allocated.contains(&ip) {
                self.allocated.insert(ip);
                return Some(ip);
            }
        }
        None
    }

    /// Allocate a specific IP address
    ///
    /// Returns an error if the IP is already allocated or not in the CIDR range.
    pub fn allocate_specific(&mut self, ip: Ipv4Addr) -> Result<()> {
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
    /// use zlayer_overlay::allocator::IpAllocator;
    ///
    /// let mut allocator = IpAllocator::new("10.200.0.0/24").unwrap();
    /// let ip = allocator.allocate_first().unwrap();
    /// assert_eq!(ip.to_string(), "10.200.0.1");
    /// ```
    pub fn allocate_first(&mut self) -> Result<Ipv4Addr> {
        let first_ip = self
            .network
            .hosts()
            .next()
            .ok_or(OverlayError::NoAvailableIps)?;

        if self.allocated.contains(&first_ip) {
            return Err(OverlayError::IpAlreadyAllocated(first_ip));
        }

        self.allocated.insert(first_ip);
        Ok(first_ip)
    }

    /// Mark an IP address as allocated (for restoring state)
    ///
    /// Returns an error if the IP is not in the CIDR range.
    pub fn mark_allocated(&mut self, ip: Ipv4Addr) -> Result<()> {
        if !self.network.contains(&ip) {
            return Err(OverlayError::IpNotInRange(ip, self.network.to_string()));
        }
        self.allocated.insert(ip);
        Ok(())
    }

    /// Release an IP address back to the pool
    ///
    /// Returns `true` if the IP was released, `false` if it wasn't allocated.
    pub fn release(&mut self, ip: Ipv4Addr) -> bool {
        self.allocated.remove(&ip)
    }

    /// Check if an IP address is allocated
    pub fn is_allocated(&self, ip: Ipv4Addr) -> bool {
        self.allocated.contains(&ip)
    }

    /// Check if an IP address is within the CIDR range
    pub fn contains(&self, ip: Ipv4Addr) -> bool {
        self.network.contains(&ip)
    }

    /// Get the number of allocated addresses
    pub fn allocated_count(&self) -> usize {
        self.allocated.len()
    }

    /// Get the total number of usable addresses in the range
    pub fn total_hosts(&self) -> u32 {
        self.network.hosts().count() as u32
    }

    /// Get the number of available addresses
    pub fn available_count(&self) -> u32 {
        self.total_hosts()
            .saturating_sub(self.allocated.len() as u32)
    }

    /// Get the CIDR string
    pub fn cidr(&self) -> String {
        self.network.to_string()
    }

    /// Get the network address
    pub fn network_addr(&self) -> Ipv4Addr {
        self.network.network()
    }

    /// Get the broadcast address
    pub fn broadcast_addr(&self) -> Ipv4Addr {
        self.network.broadcast()
    }

    /// Get the prefix length
    pub fn prefix_len(&self) -> u8 {
        self.network.prefix_len()
    }

    /// Get all allocated IPs
    pub fn allocated_ips(&self) -> Vec<Ipv4Addr> {
        self.allocated.iter().copied().collect()
    }
}

/// Helper function to get the first usable IP from a CIDR
pub fn first_ip_from_cidr(cidr: &str) -> Result<Ipv4Addr> {
    let network: Ipv4Net = cidr
        .parse()
        .map_err(|e| OverlayError::InvalidCidr(format!("{}: {}", cidr, e)))?;

    network.hosts().next().ok_or(OverlayError::NoAvailableIps)
}

#[cfg(test)]
mod tests {
    use super::*;

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

        let specific_ip: Ipv4Addr = "10.200.0.50".parse().unwrap();
        allocator.allocate_specific(specific_ip).unwrap();

        assert!(allocator.is_allocated(specific_ip));

        // Can't allocate same IP again
        assert!(allocator.allocate_specific(specific_ip).is_err());
    }

    #[test]
    fn test_allocate_specific_out_of_range() {
        let mut allocator = IpAllocator::new("10.200.0.0/24").unwrap();

        let out_of_range: Ipv4Addr = "192.168.1.1".parse().unwrap();
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

        let ip: Ipv4Addr = "10.200.0.100".parse().unwrap();
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
}
