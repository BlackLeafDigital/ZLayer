//! README:384 — a 2-node cluster (1 voter + 1 learner) elects the bootstrapper
//! as leader and replicates entries to the learner.

mod common;

use std::time::Duration;

use openraft::BasicNode;
use zlayer_scheduler::raft::{Request, ServiceState};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_node_cluster_with_one_voter_one_learner() {
    let port1 = common::allocate_port();
    let port2 = common::allocate_port();

    let node1 = common::spawn_node(1, port1).await;
    let node2 = common::spawn_node(2, port2).await;

    node1.coordinator.bootstrap().await.expect("bootstrap");

    let leader = common::wait_for_leader(&[&node1], Duration::from_secs(5)).await;
    assert_eq!(leader, 1, "first node up should win leadership");

    let basic2 = BasicNode {
        addr: node2.api_addr.clone(),
    };
    node1
        .coordinator
        .raft_clone()
        .add_learner(2, basic2, true)
        .await
        .expect("add_learner");

    let metrics = node1.coordinator.raft_clone().metrics().borrow().clone();
    let membership = metrics.membership_config.membership();
    let voters: Vec<_> = membership.voter_ids().collect();
    let learners: Vec<_> = membership.learner_ids().collect();
    assert_eq!(voters.len(), 1, "expected 1 voter, got {voters:?}");
    assert_eq!(learners.len(), 1, "expected 1 learner, got {learners:?}");
    assert_eq!(voters, vec![1]);
    assert_eq!(learners, vec![2]);

    for i in 0..5u32 {
        node1
            .coordinator
            .propose(Request::UpdateServiceState {
                service_name: format!("svc-{i}"),
                state: ServiceState::default(),
            })
            .await
            .expect("propose");
    }

    common::wait_for_apply(&[&node1, &node2], 5, Duration::from_secs(5)).await;

    common::shutdown_node(node1).await;
    common::shutdown_node(node2).await;
}
