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
}
