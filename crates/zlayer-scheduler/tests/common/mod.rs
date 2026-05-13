//! Shared multi-node Raft test harness.
//!
//! Each test brings up N in-process `RaftService` instances on loopback ports,
//! drives them via their public APIs, and asserts replicated state.

#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

use tempfile::TempDir;
use zlayer_scheduler::raft::{NodeId, RaftConfig};
use zlayer_scheduler::{RaftCoordinator, RaftService};

pub struct NodeHandle {
    pub node_id: NodeId,
    pub api_addr: String,
    pub coordinator: Arc<RaftCoordinator>,
    pub server_task: tokio::task::JoinHandle<()>,
    pub _tempdir: TempDir,
}

/// Spawn an in-process Raft node on the given loopback port.
///
/// The caller is responsible for calling `bootstrap()` on the first node
/// and `add_learner` / `change_membership` for subsequent peers.
pub async fn spawn_node(node_id: NodeId, port: u16) -> NodeHandle {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let api_addr = format!("127.0.0.1:{port}");
    let config = RaftConfig {
        node_id,
        address: api_addr.clone(),
        raft_port: port,
        data_dir: tempdir.path().to_path_buf(),
        ..RaftConfig::default()
    };

    let coordinator = Arc::new(
        RaftCoordinator::new(config)
            .await
            .expect("RaftCoordinator::new"),
    );

    let service = RaftService::new(Arc::clone(&coordinator));
    let addr: std::net::SocketAddr = api_addr.parse().expect("addr");
    let server_task = tokio::spawn(async move {
        let _ = service.run(addr).await;
    });

    tokio::time::sleep(Duration::from_millis(200)).await;

    NodeHandle {
        node_id,
        api_addr,
        coordinator,
        server_task,
        _tempdir: tempdir,
    }
}

/// Allocate a free loopback port via the kernel's "bind to port 0" trick.
pub fn allocate_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind 0");
    let port = listener.local_addr().expect("local_addr").port();
    drop(listener);
    port
}

/// Poll node metrics until exactly one node reports a leader and all peers agree.
pub async fn wait_for_leader(nodes: &[&NodeHandle], timeout: Duration) -> NodeId {
    let start = std::time::Instant::now();
    loop {
        let mut leaders: Vec<Option<NodeId>> = Vec::with_capacity(nodes.len());
        for n in nodes {
            let metrics = n.coordinator.raft_clone().metrics().borrow().clone();
            leaders.push(metrics.current_leader);
        }
        if let Some(Some(id)) = leaders.first().copied() {
            if leaders.iter().all(|l| *l == Some(id)) {
                return id;
            }
        }
        assert!(
            start.elapsed() <= timeout,
            "no consensus leader within {timeout:?}; leaders = {leaders:?}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Wait until every node reports `last_applied >= min_index`.
pub async fn wait_for_apply(nodes: &[&NodeHandle], min_index: u64, timeout: Duration) {
    let start = std::time::Instant::now();
    loop {
        let mut applied: Vec<u64> = Vec::with_capacity(nodes.len());
        for n in nodes {
            let metrics = n.coordinator.raft_clone().metrics().borrow().clone();
            applied.push(metrics.last_applied.map_or(0, |li| li.index));
        }
        if applied.iter().all(|i| *i >= min_index) {
            return;
        }
        assert!(
            start.elapsed() <= timeout,
            "apply target {min_index} not reached within {timeout:?}; applied = {applied:?}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

pub async fn shutdown_node(handle: NodeHandle) {
    handle.server_task.abort();
    let _ = handle.server_task.await;
}
