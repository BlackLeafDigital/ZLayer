//! Sanity check: a single node bootstraps and reports itself as leader.

mod common;

use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn single_node_bootstrap_becomes_leader() {
    let port = common::allocate_port();
    let node = common::spawn_node(1, port).await;

    node.coordinator.bootstrap().await.expect("bootstrap");

    let leader = common::wait_for_leader(&[&node], Duration::from_secs(5)).await;
    assert_eq!(leader, 1, "node 1 should be its own leader after bootstrap");

    common::shutdown_node(node).await;
}
