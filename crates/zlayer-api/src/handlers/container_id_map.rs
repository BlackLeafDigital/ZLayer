//! Bidirectional map between Docker-shaped 64-char hex container IDs and
//! `ZLayer`'s native `ContainerId { service, replica }`.
//!
//! Hex form is SHA-256(daemon_uuid:service:replica) lowercased. The map is
//! populated as containers are created and consulted by the Docker API
//! compatibility layer.

use std::collections::HashMap;
use std::sync::RwLock;

use sha2::{Digest, Sha256};
use zlayer_agent::runtime::ContainerId;

/// Default Docker truncation length for short container IDs.
pub const DEFAULT_SHORT_ID_LEN: usize = 12;

/// Label key stamped onto every ZLayer-created container so the daemon can
/// distinguish ZLayer-managed containers from foreign ones during
/// reconciliation. The value is the 64-char lowercase hex returned by
/// [`compute_hex`].
pub const ZLAYER_CONTAINER_ID_LABEL: &str = "com.zlayer.container_id";

/// Compute the deterministic 64-char lowercase hex ID for a container.
///
/// The hash input is `format!("{daemon_uuid}:{service}:{replica}")`.
#[must_use]
pub fn compute_hex(daemon_uuid: &str, cid: &ContainerId) -> String {
    let mut hasher = Sha256::new();
    hasher.update(format!("{}:{}:{}", daemon_uuid, cid.service, cid.replica).as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Truncate a 64-char hex container ID to its short form.
///
/// Returns the first `n` characters. If `n` exceeds the string length, the
/// full string is returned. Docker's default short form uses
/// [`DEFAULT_SHORT_ID_LEN`] (12) characters.
#[must_use]
pub fn short_id(hex: &str, n: usize) -> String {
    hex.chars().take(n).collect()
}

/// Bidirectional map between hex container IDs and `ContainerId`.
///
/// Backed by two `RwLock<HashMap>`s for independent direction lookups.
#[derive(Debug)]
pub struct ContainerIdMap {
    daemon_uuid: String,
    hex_to_cid: RwLock<HashMap<String, ContainerId>>,
    cid_to_hex: RwLock<HashMap<ContainerId, String>>,
}

impl ContainerIdMap {
    /// Create a new empty map seeded with the given daemon UUID.
    #[must_use]
    pub fn new(daemon_uuid: String) -> Self {
        Self {
            daemon_uuid,
            hex_to_cid: RwLock::new(HashMap::new()),
            cid_to_hex: RwLock::new(HashMap::new()),
        }
    }

    /// The daemon UUID this map was constructed with.
    #[must_use]
    pub fn daemon_uuid(&self) -> &str {
        &self.daemon_uuid
    }

    /// Register `cid` under `daemon_uuid`, returning its hex ID.
    ///
    /// Idempotent: if the container is already registered with a hex, that
    /// existing hex is returned and no rehash occurs.
    ///
    /// # Panics
    ///
    /// Panics if either internal `RwLock` is poisoned.
    pub fn register(&self, daemon_uuid: &str, cid: &ContainerId) -> String {
        // Fast path: already registered.
        {
            let g = self.cid_to_hex.read().expect("ContainerIdMap poisoned");
            if let Some(existing) = g.get(cid) {
                return existing.clone();
            }
        }

        let hex = compute_hex(daemon_uuid, cid);

        // Re-check under write locks, then insert.
        let mut cid_w = self.cid_to_hex.write().expect("ContainerIdMap poisoned");
        if let Some(existing) = cid_w.get(cid) {
            return existing.clone();
        }
        let mut hex_w = self.hex_to_cid.write().expect("ContainerIdMap poisoned");
        cid_w.insert(cid.clone(), hex.clone());
        hex_w.insert(hex.clone(), cid.clone());
        hex
    }

    /// Look up a `ContainerId` by its hex ID.
    ///
    /// # Panics
    ///
    /// Panics if the internal `RwLock` is poisoned.
    #[must_use]
    pub fn lookup_hex(&self, hex: &str) -> Option<ContainerId> {
        let g = self.hex_to_cid.read().expect("ContainerIdMap poisoned");
        g.get(hex).cloned()
    }

    /// Look up the hex ID for a `ContainerId`.
    ///
    /// # Panics
    ///
    /// Panics if the internal `RwLock` is poisoned.
    #[must_use]
    pub fn lookup_container(&self, cid: &ContainerId) -> Option<String> {
        let g = self.cid_to_hex.read().expect("ContainerIdMap poisoned");
        g.get(cid).cloned()
    }

    /// Remove the entry for `hex` from both directions. No-op if absent.
    ///
    /// # Panics
    ///
    /// Panics if either internal `RwLock` is poisoned.
    pub fn unregister_by_hex(&self, hex: &str) {
        let mut hex_w = self.hex_to_cid.write().expect("ContainerIdMap poisoned");
        if let Some(cid) = hex_w.remove(hex) {
            let mut cid_w = self.cid_to_hex.write().expect("ContainerIdMap poisoned");
            cid_w.remove(&cid);
        }
    }

    /// Remove the entry for `cid` from both directions. No-op if absent.
    ///
    /// # Panics
    ///
    /// Panics if either internal `RwLock` is poisoned.
    pub fn unregister_by_container(&self, cid: &ContainerId) {
        let mut cid_w = self.cid_to_hex.write().expect("ContainerIdMap poisoned");
        if let Some(hex) = cid_w.remove(cid) {
            let mut hex_w = self.hex_to_cid.write().expect("ContainerIdMap poisoned");
            hex_w.remove(&hex);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cid(service: &str, replica: u32) -> ContainerId {
        ContainerId::new(service.to_string(), replica)
    }

    #[test]
    fn compute_hex_is_64_lowercase_hex() {
        let h = compute_hex("daemon-uuid-1", &cid("svc", 0));
        assert_eq!(h.len(), 64, "hex must be exactly 64 chars");
        assert!(
            h.chars().all(|c| c.is_ascii_hexdigit()),
            "hex must contain only ASCII hex digits"
        );
        assert!(
            h.chars().all(|c| !c.is_ascii_uppercase()),
            "hex must be lowercase"
        );
    }

    #[test]
    fn compute_hex_is_deterministic() {
        let a = compute_hex("daemon-uuid-1", &cid("svc", 3));
        let b = compute_hex("daemon-uuid-1", &cid("svc", 3));
        assert_eq!(a, b);
    }

    #[test]
    fn compute_hex_differs_per_replica() {
        let a = compute_hex("daemon-uuid-1", &cid("svc", 0));
        let b = compute_hex("daemon-uuid-1", &cid("svc", 1));
        assert_ne!(a, b);
    }

    #[test]
    fn compute_hex_differs_per_daemon_uuid() {
        let a = compute_hex("daemon-A", &cid("svc", 0));
        let b = compute_hex("daemon-B", &cid("svc", 0));
        assert_ne!(
            a, b,
            "same container under different daemon UUIDs must hash differently"
        );
    }

    #[test]
    fn register_and_lookup_round_trip() {
        let m = ContainerIdMap::new("daemon-uuid-1".to_string());
        let c = cid("svc", 7);
        let hex = m.register("daemon-uuid-1", &c);
        assert_eq!(hex.len(), 64);
        assert_eq!(m.lookup_hex(&hex), Some(c.clone()));
        assert_eq!(m.lookup_container(&c), Some(hex));
    }

    #[test]
    fn register_returns_existing_hex_for_known_container() {
        let m = ContainerIdMap::new("daemon-uuid-1".to_string());
        let c = cid("svc", 1);
        let a = m.register("daemon-uuid-1", &c);
        let b = m.register("daemon-uuid-1", &c);
        assert_eq!(a, b, "register must be idempotent for known containers");
    }

    #[test]
    fn unregister_by_hex_clears_both_directions() {
        let m = ContainerIdMap::new("daemon-uuid-1".to_string());
        let c = cid("svc", 0);
        let hex = m.register("daemon-uuid-1", &c);
        m.unregister_by_hex(&hex);
        assert_eq!(m.lookup_hex(&hex), None);
        assert_eq!(m.lookup_container(&c), None);
    }

    #[test]
    fn unregister_by_container_clears_both_directions() {
        let m = ContainerIdMap::new("daemon-uuid-1".to_string());
        let c = cid("svc", 0);
        let hex = m.register("daemon-uuid-1", &c);
        m.unregister_by_container(&c);
        assert_eq!(m.lookup_hex(&hex), None);
        assert_eq!(m.lookup_container(&c), None);
    }

    #[test]
    fn short_id_truncates_to_12_default() {
        let hex = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        assert_eq!(short_id(hex, DEFAULT_SHORT_ID_LEN), "0123456789ab");
        assert_eq!(short_id(hex, DEFAULT_SHORT_ID_LEN).len(), 12);
    }

    #[test]
    fn short_id_returns_empty_for_n_zero() {
        let hex = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        assert_eq!(short_id(hex, 0), "");
    }

    /// Pin the exact string the Docker label uses. Reconciliation logic
    /// (and any external consumer that filters Docker `--filter label=...`)
    /// depends on this exact key, so a typo or rename here is a wire-format
    /// breakage and should fail loudly in CI.
    #[test]
    fn zlayer_container_id_label_is_stable() {
        assert_eq!(ZLAYER_CONTAINER_ID_LABEL, "com.zlayer.container_id");
    }

    #[test]
    fn short_id_clamps_to_string_length() {
        let hex = "abcdef";
        // n exceeds length: full string returned, no panic.
        let out = short_id(hex, 100);
        assert_eq!(out, "abcdef");
        assert_eq!(out.len(), 6);
    }
}
