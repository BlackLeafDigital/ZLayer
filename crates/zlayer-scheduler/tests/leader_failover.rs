//! README:385 failover claim: when the Raft leader dies, the surviving
//! nodes must elect a new leader and continue to accept proposals.
//!
//! Test sequence:
//!   1. Bring up a 3-node cluster (node 1 bootstraps, nodes 2 & 3 added
//!      as learners then promoted to voters).
//!   2. Wait for the initial leader to emerge.
//!   3. Propose one log entry through that leader.
//!   4. Kill the leader (abort its server task).
//!   5. Assert the two survivors elect a *different* leader.
//!   6. Propose another entry through the new leader and wait for it to
//!      replicate so we know writes are still possible post-failover.

mod common;

use std::collections::BTreeSet;
use std::time::Duration;

use openraft::BasicNode;
use zlayer_scheduler::raft::Request;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn killing_leader_elects_new_leader() {
    // 1. Allocate three loopback ports and spawn three nodes.
    let port1 = common::allocate_port();
    let port2 = common::allocate_port();
    let port3 = common::allocate_port();

    let node1 = common::spawn_node(1, port1).await;
    let node2 = common::spawn_node(2, port2).await;
    let node3 = common::spawn_node(3, port3).await;

    // 2. Bootstrap node 1, then add 2 and 3 as learners (blocking sync).
    node1.coordinator.bootstrap().await.expect("bootstrap");

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

    // 3. Promote both new nodes to voters (retain=false: drop any stragglers).
    let voters: BTreeSet<u64> = BTreeSet::from([1u64, 2, 3]);
    node1
        .coordinator
        .raft_clone()
        .change_membership(voters, false)
        .await
        .expect("change_membership to {1,2,3}");

    // 4. Wait for the cluster to converge on a single leader.
    let initial_leader =
        common::wait_for_leader(&[&node1, &node2, &node3], Duration::from_secs(5)).await;

    // 5. Propose one entry through the actual leader so we have a known
    //    log point to verify replication after failover.
    let leader_handle = match initial_leader {
        1 => &node1,
        2 => &node2,
        3 => &node3,
        other => panic!("unexpected leader id {other}"),
    };
    leader_handle
        .coordinator
        .propose(Request::UpdateNodeStatus {
            node_id: 1,
            status: "pre-failover".into(),
        })
        .await
        .expect("pre-failover propose");

    // 6. Kill the leader: take ownership of its handle, abort the
    //    server task, drop the tempdir. The remaining two nodes must
    //    elect a new leader.
    let (leader_to_kill, survivors_a, survivors_b) = match initial_leader {
        1 => (node1, node2, node3),
        2 => (node2, node1, node3),
        3 => (node3, node1, node2),
        _ => unreachable!(),
    };
    common::shutdown_node(leader_to_kill).await;

    // 7. Wait for the survivors to agree on a new leader (NOT the dead one).
    //
    // `common::wait_for_leader` returns as soon as both nodes agree on
    // any `Some(id)` — but immediately after the kill, the survivors
    // still have the dead node cached as leader. Poll until they
    // converge to a *new* leader id different from `initial_leader`.
    let new_leader_deadline = std::time::Instant::now() + Duration::from_secs(15);
    let new_leader = loop {
        let candidate =
            common::wait_for_leader(&[&survivors_a, &survivors_b], Duration::from_secs(10)).await;
        if candidate != initial_leader {
            break candidate;
        }
        assert!(
            std::time::Instant::now() < new_leader_deadline,
            "survivors never elected a new leader; still reporting dead node {initial_leader}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    assert_ne!(
        new_leader, initial_leader,
        "expected a different leader after killing node {initial_leader}, but cluster still reports {new_leader}"
    );

    // 8. Propose through the freshly elected leader.
    let new_leader_handle = if survivors_a.node_id == new_leader {
        &survivors_a
    } else {
        &survivors_b
    };
    new_leader_handle
        .coordinator
        .propose(Request::UpdateNodeStatus {
            node_id: 1,
            status: "post-failover".into(),
        })
        .await
        .expect("post-failover propose");

    // 9. Both survivors should apply at least 4 entries (bootstrap +
    //    two membership changes + at least one user propose). Use a
    //    generous bound that accounts for membership-change entries.
    common::wait_for_apply(&[&survivors_a, &survivors_b], 4, Duration::from_secs(10)).await;

    // 10. Cleanup remaining handles.
    common::shutdown_node(survivors_a).await;
    common::shutdown_node(survivors_b).await;
}
