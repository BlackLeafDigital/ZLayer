//! Serf/SWIM-style gossip pool for worker-tier overlay peer discovery.
//!
//! Wraps the `chitchat` crate (Quickwit's Phi-accrual + Versioned KV gossip).
//! Workers join the pool after registering with the control plane; the
//! gossip pool then handles pairwise `WireGuard` peer-key + endpoint discovery
//! without routing every update through the leader.
//!
//! Key shape: each worker writes one KV pair under
//! `worker:<node_id>` → JSON-encoded [`PeerInfo`]. Every other worker
//! subscribes to changes and updates its overlay topology accordingly.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use chitchat::transport::UdpTransport;
use chitchat::{
    spawn_chitchat, Chitchat, ChitchatConfig, ChitchatHandle, ChitchatId, FailureDetectorConfig,
};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, Mutex};
use tracing::{info, warn};

use crate::error::OverlayError;

/// Information one worker publishes about itself via the gossip pool.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PeerInfo {
    pub node_id: u64,
    /// `WireGuard` public key (base64-url-no-pad).
    pub wg_pubkey: String,
    /// `WireGuard` UDP endpoint (host:port).
    pub wg_endpoint: SocketAddr,
    /// Overlay IP assigned to this peer.
    pub overlay_ip: String,
    /// Optional labels (mirrors the labels declared during Register).
    #[serde(default)]
    pub labels: HashMap<String, String>,
}

/// Topology-change events emitted by the gossip pool.
#[derive(Debug, Clone)]
pub enum TopologyEvent {
    Joined(PeerInfo),
    Updated(PeerInfo),
    Left { node_id: u64 },
}

/// Configuration for the gossip pool.
#[derive(Debug, Clone)]
pub struct GossipConfig {
    /// This node's identity for the gossip protocol.
    pub node_id: u64,
    /// UDP bind address for chitchat (e.g. `0.0.0.0:7946` — Serf default).
    pub gossip_listen: SocketAddr,
    /// Seed addresses the control plane provided during Register.
    /// Each is a chitchat-protocol UDP endpoint of another pool member.
    pub seeds: Vec<SocketAddr>,
    /// Cluster identifier (chitchat partitions on this — keeps gossip
    /// from cross-talking between independent clusters in the same LAN).
    pub cluster_id: String,
    /// Self-info to publish into the gossip KV.
    pub self_info: PeerInfo,
}

/// Live handle to a running gossip pool.
pub struct GossipPool {
    // Keep the chitchat server task alive for the lifetime of the pool.
    _handle: ChitchatHandle,
    chitchat: Arc<Mutex<Chitchat>>,
    cluster_id: String,
    events_tx: broadcast::Sender<TopologyEvent>,
}

impl std::fmt::Debug for GossipPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GossipPool")
            .field("cluster_id", &self.cluster_id)
            .finish_non_exhaustive()
    }
}

impl GossipPool {
    /// Start a gossip pool. Spawns the chitchat background task + a watcher
    /// that publishes `TopologyEvent`s as peers join/leave/change.
    ///
    /// # Errors
    ///
    /// Returns `OverlayError::NetworkConfig` if `self_info` cannot be encoded
    /// or if the chitchat server fails to bind to `gossip_listen`.
    pub async fn start(config: GossipConfig) -> Result<Arc<Self>, OverlayError> {
        let chitchat_id = ChitchatId::new(
            format!("worker:{}", config.node_id),
            0,
            config.gossip_listen,
        );

        let cfg = ChitchatConfig {
            chitchat_id,
            cluster_id: config.cluster_id.clone(),
            gossip_interval: Duration::from_secs(1),
            listen_addr: config.gossip_listen,
            seed_nodes: config
                .seeds
                .iter()
                .map(std::string::ToString::to_string)
                .collect(),
            failure_detector_config: FailureDetectorConfig::default(),
            marked_for_deletion_grace_period: Duration::from_secs(60),
            catchup_callback: None,
            extra_liveness_predicate: None,
        };

        // Pre-stamp the chitchat KV with this node's self_info.
        let self_info_bytes = serde_json::to_vec(&config.self_info)
            .map_err(|e| OverlayError::NetworkConfig(format!("encode gossip self_info: {e}")))?;
        let self_info_b64 =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&self_info_bytes);

        let initial_kvs = vec![(format!("worker:{}", config.node_id), self_info_b64)];

        let handle = spawn_chitchat(cfg, initial_kvs, &UdpTransport)
            .await
            .map_err(|e| OverlayError::NetworkConfig(format!("spawn chitchat: {e}")))?;

        let chitchat = handle.chitchat();
        let (events_tx, _events_rx) = broadcast::channel(256);

        // Background task: poll chitchat state every ~1s, diff against last
        // snapshot, emit TopologyEvents.
        let chitchat_for_watcher = chitchat.clone();
        let events_for_watcher = events_tx.clone();
        let cluster_for_watcher = config.cluster_id.clone();
        tokio::spawn(async move {
            let mut last_snapshot: HashMap<u64, PeerInfo> = HashMap::new();
            let mut tick = tokio::time::interval(Duration::from_secs(1));
            loop {
                tick.tick().await;
                let chitchat_guard = chitchat_for_watcher.lock().await;
                let current = collect_peers(&chitchat_guard);
                drop(chitchat_guard);

                let mut next_snapshot = HashMap::new();
                for peer in current {
                    next_snapshot.insert(peer.node_id, peer.clone());
                    match last_snapshot.get(&peer.node_id) {
                        None => {
                            let _ = events_for_watcher.send(TopologyEvent::Joined(peer));
                        }
                        Some(prev) if prev != &peer => {
                            let _ = events_for_watcher.send(TopologyEvent::Updated(peer));
                        }
                        _ => {}
                    }
                }
                for id in last_snapshot.keys() {
                    if !next_snapshot.contains_key(id) {
                        let _ = events_for_watcher.send(TopologyEvent::Left { node_id: *id });
                    }
                }
                last_snapshot = next_snapshot;

                tracing::trace!(cluster = %cluster_for_watcher, "gossip watcher tick");
            }
        });

        info!(
            cluster_id = %config.cluster_id,
            node_id = config.node_id,
            seeds = ?config.seeds,
            "gossip pool started"
        );

        Ok(Arc::new(Self {
            _handle: handle,
            chitchat,
            cluster_id: config.cluster_id,
            events_tx,
        }))
    }

    /// Snapshot of all currently-known peers (excluding self).
    pub async fn peers(&self) -> Vec<PeerInfo> {
        let chitchat = self.chitchat.lock().await;
        collect_peers(&chitchat)
    }

    /// Subscribe to topology change events. Each subscriber gets a fresh
    /// `broadcast::Receiver` — late-joiners do NOT replay history.
    #[must_use]
    pub fn subscribe_updates(&self) -> broadcast::Receiver<TopologyEvent> {
        self.events_tx.subscribe()
    }

    /// Re-publish this node's `PeerInfo` (e.g. after a `WireGuard` key rotation).
    ///
    /// # Errors
    ///
    /// Returns `OverlayError::NetworkConfig` if `info` cannot be JSON-encoded.
    pub async fn announce_self(&self, info: &PeerInfo) -> Result<(), OverlayError> {
        let bytes = serde_json::to_vec(info)
            .map_err(|e| OverlayError::NetworkConfig(format!("encode self_info: {e}")))?;
        let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&bytes);
        let key = format!("worker:{}", info.node_id);

        let mut chitchat = self.chitchat.lock().await;
        chitchat.self_node_state().set(key, b64);
        Ok(())
    }

    /// Cluster id this pool belongs to.
    #[must_use]
    pub fn cluster_id(&self) -> &str {
        &self.cluster_id
    }
}

/// Collect peer info from chitchat's current node-state map. Skips self
/// (we don't emit Joined/Left for the local node).
fn collect_peers(chitchat: &Chitchat) -> Vec<PeerInfo> {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;

    let mut out = Vec::new();
    let self_id = chitchat.self_chitchat_id().clone();

    for (chitchat_id, node_state) in chitchat.node_states() {
        if chitchat_id == &self_id {
            continue;
        }
        for (key, value) in node_state.key_values() {
            if let Some(node_id_str) = key.strip_prefix("worker:") {
                if let Ok(node_id) = node_id_str.parse::<u64>() {
                    match URL_SAFE_NO_PAD.decode(value) {
                        Ok(bytes) => {
                            if let Ok(info) = serde_json::from_slice::<PeerInfo>(&bytes) {
                                out.push(info);
                            }
                        }
                        Err(e) => {
                            warn!(
                                ?chitchat_id,
                                key,
                                node_id,
                                error = %e,
                                "decode peer info failed"
                            );
                        }
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_self_info(node_id: u64) -> PeerInfo {
        PeerInfo {
            node_id,
            wg_pubkey: "test-key".into(),
            wg_endpoint: "127.0.0.1:51820".parse().unwrap(),
            overlay_ip: "10.0.0.1".into(),
            labels: HashMap::default(),
        }
    }

    #[tokio::test]
    async fn gossip_pool_starts_with_self_only() {
        let config = GossipConfig {
            node_id: 42,
            gossip_listen: "127.0.0.1:0".parse().unwrap(),
            seeds: vec![],
            cluster_id: "test-cluster".into(),
            self_info: make_self_info(42),
        };
        let pool = GossipPool::start(config).await.expect("start");
        // self is excluded from peers()
        let peers = pool.peers().await;
        assert!(peers.is_empty(), "expected no peers, got: {peers:?}");
        assert_eq!(pool.cluster_id(), "test-cluster");
    }
}
