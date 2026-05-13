//! Multi-node Raft replication tests.
//!
//! Spins up three in-process nodes against the shared harness in
//! `tests/common/mod.rs`, drives membership changes through openraft's
//! public APIs, and asserts that proposals replicate to every voter.

mod common;

use std::collections::BTreeSet;
use std::time::Duration;

use openraft::BasicNode;
use zlayer_scheduler::raft::{NodeId, Request};

/// Bootstrap a 3-voter cluster and replicate 100 entries from the leader.
///
/// Walks the standard openraft join sequence — `add_learner` (blocking) for
/// each follower, then `change_membership` with the full voter set and
/// `retain = false` to land in a uniform 3-voter config — and confirms that
/// every node applies the resulting log up to index 100.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn three_node_cluster_replicates_100_entries() {
    let port1 = common::allocate_port();
    let port2 = common::allocate_port();
    let port3 = common::allocate_port();

    let node1 = common::spawn_node(1, port1).await;
    let node2 = common::spawn_node(2, port2).await;
    let node3 = common::spawn_node(3, port3).await;

    node1.coordinator.bootstrap().await.expect("bootstrap");

    let leader = common::wait_for_leader(&[&node1], Duration::from_secs(5)).await;
    assert_eq!(leader, 1, "node 1 should be leader after bootstrap");

    // Add nodes 2 and 3 as learners (blocking, so each catches up on log
    // entries before the membership change promotes them).
    node1
        .coordinator
        .raft_clone()
        .add_learner(
            2,
            BasicNode {
                addr: node2.api_addr.clone(),
            },
            true,
        )
        .await
        .expect("add_learner 2");
    node1
        .coordinator
        .raft_clone()
        .add_learner(
            3,
            BasicNode {
                addr: node3.api_addr.clone(),
            },
            true,
        )
        .await
        .expect("add_learner 3");

    // Promote {1, 2, 3} to voters. The `BTreeSet<NodeId>` argument goes
    // through `From<I: IntoIterator<Item = NodeId>> for ChangeMembers`,
    // landing as `ChangeMembers::ReplaceAllVoters`. `retain = false`
    // means anyone outside the set is removed entirely (none here).
    let voters: BTreeSet<NodeId> = [1u64, 2, 3].into_iter().collect();
    node1
        .coordinator
        .raft_clone()
        .change_membership(voters, false)
        .await
        .expect("change_membership");

    // Confirm the new uniform config landed.
    let metrics = node1.coordinator.raft_clone().metrics().borrow().clone();
    let voter_count = metrics.membership_config.membership().voter_ids().count();
    assert_eq!(voter_count, 3, "expected 3 voters after change_membership");

    // Replicate 100 entries through the leader.
    for i in 0..100u64 {
        node1
            .coordinator
            .propose(Request::UpdateNodeStatus {
                node_id: 1,
                status: format!("step-{i}"),
            })
            .await
            .expect("propose");
    }

    // The 100 proposals plus bootstrap/membership entries should put
    // last_applied at >= 100 on every voter. Higher timeout because each
    // propose round-trips through the leader serially.
    common::wait_for_apply(&[&node1, &node2, &node3], 100, Duration::from_secs(15)).await;

    common::shutdown_node(node1).await;
    common::shutdown_node(node2).await;
    common::shutdown_node(node3).await;
}

/// A freshly-added voter with empty state must catch up via snapshot.
///
/// Builds a 2-voter cluster, drives the leader past 50 log entries, forces
/// a snapshot via `Raft::trigger().snapshot()`, then introduces a third
/// node that joins as a learner. The leader's append-entries path can still
/// reach the empty learner (logs aren't purged on the trigger alone), but
/// the snapshot metadata must already be present for the install-snapshot
/// path to be available. We assert that node 3 reaches `last_applied >= 50`
/// after the join, which exercises whichever mechanism openraft selects.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn snapshot_install_to_lagging_follower() {
    let port1 = common::allocate_port();
    let port2 = common::allocate_port();

    let node1 = common::spawn_node(1, port1).await;
    let node2 = common::spawn_node(2, port2).await;

    node1.coordinator.bootstrap().await.expect("bootstrap");
    let _ = common::wait_for_leader(&[&node1], Duration::from_secs(5)).await;

    node1
        .coordinator
        .raft_clone()
        .add_learner(
            2,
            BasicNode {
                addr: node2.api_addr.clone(),
            },
            true,
        )
        .await
        .expect("add_learner 2");

    let voters: BTreeSet<NodeId> = [1u64, 2].into_iter().collect();
    node1
        .coordinator
        .raft_clone()
        .change_membership(voters, false)
        .await
        .expect("change_membership {1,2}");

    // Replicate 50 entries.
    for i in 0..50u64 {
        node1
            .coordinator
            .propose(Request::UpdateNodeStatus {
                node_id: 1,
                status: format!("pre-snap-{i}"),
            })
            .await
            .expect("propose pre-snapshot");
    }

    common::wait_for_apply(&[&node1, &node2], 50, Duration::from_secs(15)).await;

    // Force a snapshot. The default snapshot policy is
    // `LogsSinceLast(5000)`, which we can't override through the
    // current harness, so use the explicit trigger to build one.
    node1
        .coordinator
        .raft_clone()
        .trigger()
        .snapshot()
        .await
        .expect("trigger snapshot");

    // Wait for the snapshot metadata to land in the leader's metrics with
    // an index of at least 50.
    let snapshot_deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let metrics = node1.coordinator.raft_clone().metrics().borrow().clone();
        if let Some(snap) = metrics.snapshot {
            if snap.index >= 50 {
                break;
            }
        }
        assert!(
            std::time::Instant::now() < snapshot_deadline,
            "snapshot with index >= 50 did not appear on leader within 5s; \
             current snapshot = {:?}",
            node1.coordinator.raft_clone().metrics().borrow().snapshot
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // Bring up a fresh node 3 (empty tempdir, empty state) and add as
    // learner. The leader should backfill via append_entries or by
    // installing the snapshot we just built.
    let port3 = common::allocate_port();
    let node3 = common::spawn_node(3, port3).await;

    node1
        .coordinator
        .raft_clone()
        .add_learner(
            3,
            BasicNode {
                addr: node3.api_addr.clone(),
            },
            true,
        )
        .await
        .expect("add_learner 3");

    common::wait_for_apply(&[&node3], 50, Duration::from_secs(15)).await;

    common::shutdown_node(node1).await;
    common::shutdown_node(node2).await;
    common::shutdown_node(node3).await;
}
