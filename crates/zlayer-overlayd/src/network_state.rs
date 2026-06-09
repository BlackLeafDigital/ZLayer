//! Persistent marker for host-level networks `ZLayer` creates.
//!
//! Some network objects `ZLayer` provisions live at the **host** level, not the
//! daemon-process level — most notably the Windows HCN overlay network, which
//! the HCS runtime creates and HCN keeps alive until an explicit
//! `HcnDeleteNetwork`. Such objects must:
//!
//!   * be **reused** across daemon restarts / binary updates / reinstalls
//!     (look them up by their recorded id instead of blindly recreating), and
//!   * be torn down **only** on a full uninstall (`daemon uninstall --purge`),
//!     never on a routine stop/restart.
//!
//! This module is the on-disk record that makes that lifecycle possible. It is
//! intentionally backend-agnostic (pure `serde` + `std::fs`) so the same marker
//! file can track HCN networks on Windows, bridges on Linux, etc. The file
//! lives at [`zlayer_paths::ZLayerDirs::agent_network_state`]
//! (`{data_dir}/agent_network.json`).

use std::path::Path;

use serde::{Deserialize, Serialize};

/// Schema version for [`NetworkState`]. Bump on a breaking layout change.
const CURRENT_VERSION: u32 = 1;

/// `owner` value for the node's single shared base overlay network.
pub const OWNER_BASE: &str = "base";

/// Build the `owner` value for a dedicated per-service network.
#[must_use]
pub fn owner_for_service(service: &str) -> String {
    format!("service:{service}")
}

/// One host-level network `ZLayer` is responsible for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedNetwork {
    /// Logical owner: [`OWNER_BASE`] for the node's shared overlay, or
    /// `service:<name>` (see [`owner_for_service`]) for a dedicated per-service
    /// network. Used as the upsert key.
    pub owner: String,
    /// Backend-specific kind, e.g. `"hcn-internal"`. Lets a reader (and the
    /// uninstall path) know which API to use to delete the object.
    pub kind: String,
    /// Human-readable network name (e.g. `"zlayer-overlay"`).
    pub name: String,
    /// Host-addressable id — for HCN this is the network GUID string.
    pub id: String,
    /// CIDR the network was created with (informational / diagnostics).
    pub subnet: String,
    /// Dedicated-overlay `WireGuard` listen port (per-service transports only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wg_port: Option<u16>,
    /// Dedicated-overlay `WireGuard` private key, base64 (per-service only).
    /// Persisted so the device identity survives overlayd restarts (no
    /// per-service republish loop exists, so a stable key avoids a re-peer storm).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wg_private_key: Option<String>,
    /// Dedicated-overlay public key, base64.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wg_public_key: Option<String>,
    /// Dedicated-overlay interface name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interface: Option<String>,
}

/// The full marker file: every host-level network this node manages.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkState {
    /// On-disk schema version.
    #[serde(default = "default_version")]
    pub version: u32,
    /// Managed networks, keyed in practice by [`ManagedNetwork::owner`].
    #[serde(default)]
    pub networks: Vec<ManagedNetwork>,
}

fn default_version() -> u32 {
    CURRENT_VERSION
}

impl Default for NetworkState {
    fn default() -> Self {
        Self {
            version: CURRENT_VERSION,
            networks: Vec::new(),
        }
    }
}

impl NetworkState {
    /// Load the marker file. A missing or unparseable file yields an empty
    /// state rather than an error — the marker is a best-effort cache that the
    /// live-host enumeration paths can always rebuild.
    #[must_use]
    pub fn load(path: &Path) -> Self {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    /// Persist the marker file, creating the parent directory if needed. Writes
    /// to a sibling temp file and renames so a crash mid-write can't leave a
    /// truncated marker.
    ///
    /// # Errors
    ///
    /// Returns any I/O error from creating the directory, writing, or renaming.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, &json)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Look up a managed network by owner.
    #[must_use]
    pub fn get(&self, owner: &str) -> Option<&ManagedNetwork> {
        self.networks.iter().find(|n| n.owner == owner)
    }

    /// Insert or replace the entry for `net.owner`.
    pub fn upsert(&mut self, net: ManagedNetwork) {
        if let Some(existing) = self.networks.iter_mut().find(|n| n.owner == net.owner) {
            *existing = net;
        } else {
            self.networks.push(net);
        }
    }

    /// Remove and return the entry for `owner`, if present.
    pub fn remove(&mut self, owner: &str) -> Option<ManagedNetwork> {
        self.networks
            .iter()
            .position(|n| n.owner == owner)
            .map(|pos| self.networks.remove(pos))
    }
}

/// Width of the dedicated-overlay listen-port band scanned by
/// [`DedicatedPortAllocator`]. Ports are handed out from `base+1 ..= base+MAX`,
/// so a default base of `51820` yields the range `51821..=52076` — 256 distinct
/// per-service `WireGuard` transports, comfortably more than any single node is
/// expected to host while staying well clear of the ephemeral range.
pub const DEDICATED_PORT_BAND: u16 = 256;

/// Deterministic allocator for dedicated-overlay `WireGuard` listen ports.
///
/// Each per-service [`OverlayMode::Dedicated`] overlay needs its own UDP listen
/// port distinct from the node's shared base-overlay port. This allocator hands
/// out the lowest free port in the band `base+1 ..= base+`[`DEDICATED_PORT_BAND`]
/// by scanning ascending — no RNG, fully reproducible across restarts.
///
/// On startup, callers rehydrate the in-use set from the marker (the persisted
/// [`ManagedNetwork::wg_port`] of each dedicated service) via [`Self::reserve`]
/// so a service re-binds the exact port it had before.
///
/// [`OverlayMode::Dedicated`]: # "consumed by a later task"
#[derive(Debug, Clone)]
pub struct DedicatedPortAllocator {
    base: u16,
    used: std::collections::BTreeSet<u16>,
}

impl DedicatedPortAllocator {
    /// Build an allocator over `base+1 ..= base+`[`DEDICATED_PORT_BAND`], seeding
    /// the in-use set from `in_use` (e.g. ports already recorded in the marker).
    ///
    /// Ports in `in_use` that fall outside the band are kept in the used set —
    /// they never collide with [`allocate`](Self::allocate) results and reserving
    /// an out-of-band port is harmless — but they can never be re-allocated.
    pub fn new(base: u16, in_use: impl IntoIterator<Item = u16>) -> Self {
        Self {
            base,
            used: in_use.into_iter().collect(),
        }
    }

    /// Lowest port in the band, i.e. `base + 1` (saturating).
    fn band_start(&self) -> u16 {
        self.base.saturating_add(1)
    }

    /// Highest port in the band, i.e. `base + `[`DEDICATED_PORT_BAND`] (saturating).
    fn band_end(&self) -> u16 {
        self.base.saturating_add(DEDICATED_PORT_BAND)
    }

    /// Allocate the lowest free port in the band, recording it as used.
    ///
    /// # Errors
    ///
    /// Returns [`OverlaydError::Other`] if every port in the band is taken.
    pub fn allocate(&mut self) -> crate::error::Result<u16> {
        for port in self.band_start()..=self.band_end() {
            if !self.used.contains(&port) {
                self.used.insert(port);
                return Ok(port);
            }
        }
        Err(crate::error::OverlaydError::Other(format!(
            "dedicated-overlay port band exhausted ({}..={}, {} ports)",
            self.band_start(),
            self.band_end(),
            DEDICATED_PORT_BAND
        )))
    }

    /// Free a previously allocated port so it can be handed out again.
    pub fn release(&mut self, port: u16) {
        self.used.remove(&port);
    }

    /// Mark a specific port used without scanning — used to rehydrate the
    /// allocator from persisted marker state so a service re-binds its port.
    pub fn reserve(&mut self, port: u16) {
        self.used.insert(port);
    }

    /// Whether `port` is currently recorded as in use.
    #[must_use]
    pub fn is_used(&self, port: u16) -> bool {
        self.used.contains(&port)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(owner: &str, id: &str) -> ManagedNetwork {
        ManagedNetwork {
            owner: owner.to_string(),
            kind: "hcn-internal".to_string(),
            name: "zlayer-overlay".to_string(),
            id: id.to_string(),
            subnet: "10.200.0.0/28".to_string(),
            wg_port: None,
            wg_private_key: None,
            wg_public_key: None,
            interface: None,
        }
    }

    #[test]
    fn upsert_replaces_same_owner_and_get_finds_it() {
        let mut st = NetworkState::default();
        st.upsert(sample(OWNER_BASE, "guid-1"));
        st.upsert(sample(OWNER_BASE, "guid-2")); // same owner -> replace
        assert_eq!(st.networks.len(), 1);
        assert_eq!(st.get(OWNER_BASE).unwrap().id, "guid-2");
    }

    #[test]
    fn distinct_owners_coexist_and_remove_targets_one() {
        let mut st = NetworkState::default();
        st.upsert(sample(OWNER_BASE, "base-guid"));
        st.upsert(sample(&owner_for_service("web"), "web-guid"));
        assert_eq!(st.networks.len(), 2);

        let removed = st.remove(OWNER_BASE).expect("base entry present");
        assert_eq!(removed.id, "base-guid");
        assert_eq!(st.networks.len(), 1);
        assert!(st.get(OWNER_BASE).is_none());
        assert_eq!(st.get(&owner_for_service("web")).unwrap().id, "web-guid");
        assert!(st.remove("service:nope").is_none());
    }

    #[test]
    fn save_then_load_roundtrips() {
        let dir = std::env::temp_dir().join(format!("zlayer-netstate-test-{}", std::process::id()));
        let path = dir.join("agent_network.json");
        let _ = std::fs::remove_dir_all(&dir);

        let mut st = NetworkState::default();
        st.upsert(sample(OWNER_BASE, "guid-rt"));
        st.save(&path).expect("save must succeed");

        let loaded = NetworkState::load(&path);
        assert_eq!(loaded.version, CURRENT_VERSION);
        assert_eq!(loaded.networks, st.networks);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_missing_file_is_empty_default() {
        let path = std::env::temp_dir().join("zlayer-netstate-does-not-exist-xyz.json");
        let _ = std::fs::remove_file(&path);
        let st = NetworkState::load(&path);
        assert_eq!(st.version, CURRENT_VERSION);
        assert!(st.networks.is_empty());
    }

    #[test]
    fn dedicated_fields_survive_save_load_roundtrip() {
        let dir = std::env::temp_dir().join(format!("zlayer-netstate-ded-{}", std::process::id()));
        let path = dir.join("agent_network.json");
        let _ = std::fs::remove_dir_all(&dir);

        let mut net = sample(&owner_for_service("web"), "ded-guid");
        net.wg_port = Some(51823);
        net.wg_private_key = Some("cHJpdmF0ZS1rZXktYjY0".to_string());
        net.wg_public_key = Some("cHVibGljLWtleS1iNjQ=".to_string());
        net.interface = Some("zl-web0".to_string());

        let mut st = NetworkState::default();
        st.upsert(net.clone());
        st.save(&path).expect("save must succeed");

        let loaded = NetworkState::load(&path);
        let got = loaded
            .get(&owner_for_service("web"))
            .expect("service entry present");
        assert_eq!(got.wg_port, Some(51823));
        assert_eq!(got.wg_private_key.as_deref(), Some("cHJpdmF0ZS1rZXktYjY0"));
        assert_eq!(got.wg_public_key.as_deref(), Some("cHVibGljLWtleS1iNjQ="));
        assert_eq!(got.interface.as_deref(), Some("zl-web0"));
        assert_eq!(got, &net);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn older_marker_without_dedicated_fields_still_loads() {
        // Hand-written marker JSON from before the dedicated-overlay fields
        // existed: it must deserialize with the new fields defaulting to None.
        let dir = std::env::temp_dir().join(format!("zlayer-netstate-bc-{}", std::process::id()));
        let path = dir.join("agent_network.json");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");

        let legacy = r#"{
            "version": 1,
            "networks": [
                {
                    "owner": "base",
                    "kind": "hcn-internal",
                    "name": "zlayer-overlay",
                    "id": "legacy-guid",
                    "subnet": "10.200.0.0/28"
                }
            ]
        }"#;
        std::fs::write(&path, legacy).expect("write legacy marker");

        let loaded = NetworkState::load(&path);
        let got = loaded.get(OWNER_BASE).expect("base entry present");
        assert_eq!(got.id, "legacy-guid");
        assert_eq!(got.wg_port, None);
        assert_eq!(got.wg_private_key, None);
        assert_eq!(got.wg_public_key, None);
        assert_eq!(got.interface, None);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn allocate_returns_distinct_ascending_ports() {
        let mut alloc = DedicatedPortAllocator::new(51820, std::iter::empty());
        let a = alloc.allocate().expect("port a");
        let b = alloc.allocate().expect("port b");
        let c = alloc.allocate().expect("port c");
        assert_eq!(a, 51821);
        assert_eq!(b, 51822);
        assert_eq!(c, 51823);
    }

    #[test]
    fn release_then_allocate_reuses_freed_port() {
        let mut alloc = DedicatedPortAllocator::new(51820, std::iter::empty());
        let a = alloc.allocate().expect("port a");
        let b = alloc.allocate().expect("port b");
        assert_eq!(a, 51821);
        assert_eq!(b, 51822);

        alloc.release(a);
        // Lowest free is now the released port again.
        let reused = alloc.allocate().expect("reused port");
        assert_eq!(reused, 51821);
    }

    #[test]
    fn reserved_port_is_skipped_by_allocate() {
        // Rehydrate as if 51821 was persisted in the marker for another service.
        let mut alloc = DedicatedPortAllocator::new(51820, [51821]);
        assert!(alloc.is_used(51821));
        let first = alloc.allocate().expect("first allocation");
        assert_eq!(first, 51822);

        // Explicit reserve mid-flight is also honored.
        alloc.reserve(51823);
        let next = alloc.allocate().expect("next allocation");
        assert_eq!(next, 51824);
    }

    #[test]
    fn band_exhaustion_errors() {
        // Pre-reserve every port in the band so allocate has nothing left.
        let base = 51820u16;
        let full: Vec<u16> = (base + 1..=base + DEDICATED_PORT_BAND).collect();
        let mut alloc = DedicatedPortAllocator::new(base, full);
        let err = alloc.allocate().expect_err("band must be exhausted");
        assert!(
            matches!(err, crate::error::OverlaydError::Other(ref m) if m.contains("exhausted")),
            "unexpected error: {err:?}"
        );
    }
}
