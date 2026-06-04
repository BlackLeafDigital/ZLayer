//! Workspace-level integration tests for the three node allocation modes
//! advertised in the README (Shared, Dedicated, Exclusive).
//!
//! These tests exercise [`zlayer_scheduler::place_service_replicas`] directly
//! against an in-memory fixture (no Raft, no daemon) and assert that the
//! placement decisions match the contract documented in the README:
//!
//! | Mode      | Description                                                     |
//! |-----------|-----------------------------------------------------------------|
//! | shared    | Containers bin-packed onto nodes with available capacity        |
//! | dedicated | Each replica gets its own node (1:1 mapping)                    |
//! | exclusive | Service has nodes exclusively to itself (no other services)     |

use std::collections::HashSet;

use zlayer_scheduler::{
    place_service_replicas, NodeResources, NodeState, PlacementDecision, PlacementState,
};
use zlayer_spec::{
    GroupAffinity, HealthCheck, HealthSpec, ImageSpec, NodeMode, PullPolicy, ResourcesSpec,
    ServiceSpec,
};

/// Build a healthy node with the requested CPU/memory capacity.
fn make_node(id: u64, cpu_total: f64, memory_total: u64) -> NodeState {
    NodeState::new(id, format!("10.0.0.{id}:8000"))
        .with_resources(NodeResources::new(cpu_total, memory_total))
}

/// Build a minimal `ServiceSpec` matching the in-tree test helper in
/// `placement.rs` (line ~1048). Mirrors that pattern to keep this integration
/// test in sync with the unit-test fixture.
fn make_spec(node_mode: NodeMode, cpu_per_replica: Option<f64>) -> ServiceSpec {
    let resources = ResourcesSpec {
        cpu: cpu_per_replica,
        ..ResourcesSpec::default()
    };

    ServiceSpec {
        image: ImageSpec {
            name: "test:latest".parse().expect("valid image reference"),
            pull_policy: PullPolicy::IfNotPresent,
        },
        resources,
        health: HealthSpec {
            start_grace: None,
            interval: None,
            timeout: None,
            retries: 3,
            check: HealthCheck::Tcp { port: 8080 },
        },
        node_mode,
        ..ServiceSpec::default()
    }
}

/// Like [`make_spec`] but in `Shared` mode with an explicit placement
/// `affinity` (the opt-in spread/pack/pin knob).
fn make_spec_affinity(cpu_per_replica: Option<f64>, affinity: GroupAffinity) -> ServiceSpec {
    ServiceSpec {
        affinity: Some(affinity),
        ..make_spec(NodeMode::Shared, cpu_per_replica)
    }
}

const GIB: u64 = 1024 * 1024 * 1024;

/// README contract: in `shared` mode, replicas are *bin-packed* onto nodes
/// with available capacity. With three identical empty nodes and a workload
/// whose per-replica footprint fits easily on a single node, the scheduler
/// should collapse all replicas onto a small number of nodes rather than
/// fanning out one-per-node.
#[test]
fn shared_mode_bin_packs_onto_one_node_when_possible() {
    let mut nodes = vec![
        make_node(1, 10.0, 16 * GIB),
        make_node(2, 10.0, 16 * GIB),
        make_node(3, 10.0, 16 * GIB),
    ];
    let mut placements = PlacementState::new();
    let spec = make_spec(NodeMode::Shared, Some(1.0));

    let decisions = place_service_replicas("api", &spec, 5, &mut nodes, &mut placements);

    assert_eq!(decisions.len(), 5, "expected 5 placement decisions");
    assert!(
        decisions.iter().all(PlacementDecision::is_success),
        "every replica should be placed, got: {decisions:?}"
    );

    let distinct_nodes: HashSet<u64> = decisions.iter().filter_map(|d| d.node_id).collect();

    // Bin-packing must NOT degenerate to one-replica-per-node.
    assert!(
        distinct_nodes.len() < 5,
        "shared mode should bin-pack; spread {} replicas across {} distinct nodes",
        decisions.len(),
        distinct_nodes.len()
    );

    // With three empty, identical nodes, today's scheduler ties on utilization
    // and concentrates onto one node. Allow up to two nodes for a small amount
    // of slack against future tie-breaking heuristics, but anything beyond that
    // is a regression in bin-packing.
    assert!(
        distinct_nodes.len() <= 2,
        "shared mode should concentrate replicas on at most 2 nodes when capacity is ample, \
         got {} distinct nodes: {:?}",
        distinct_nodes.len(),
        distinct_nodes
    );
}

/// Opt-in `affinity: spread` on a `Shared`-mode service must distribute
/// same-service replicas across distinct nodes even though they would all fit
/// on one node (the `cluster_scaling` e2e contract). This is same-service
/// anti-affinity, not a `node_mode` change.
#[test]
fn shared_mode_spread_affinity_distributes_across_nodes() {
    let mut nodes = vec![
        make_node(1, 10.0, 16 * GIB),
        make_node(2, 10.0, 16 * GIB),
        make_node(3, 10.0, 16 * GIB),
    ];
    let mut placements = PlacementState::new();
    let spec = make_spec_affinity(Some(1.0), GroupAffinity::Spread);

    let decisions = place_service_replicas("web", &spec, 3, &mut nodes, &mut placements);

    assert_eq!(decisions.len(), 3);
    assert!(
        decisions.iter().all(PlacementDecision::is_success),
        "every replica should be placed, got: {decisions:?}"
    );
    let distinct: HashSet<u64> = decisions.iter().filter_map(|d| d.node_id).collect();
    assert_eq!(
        distinct.len(),
        3,
        "spread affinity must put 3 replicas on 3 distinct nodes, got {distinct:?}"
    );
}

/// Explicit `affinity: pack` preserves the historical concentrate behavior
/// (same as the default `None`): replicas collapse onto few nodes.
#[test]
fn shared_mode_pack_affinity_concentrates() {
    let mut nodes = vec![
        make_node(1, 10.0, 16 * GIB),
        make_node(2, 10.0, 16 * GIB),
        make_node(3, 10.0, 16 * GIB),
    ];
    let mut placements = PlacementState::new();
    let spec = make_spec_affinity(Some(1.0), GroupAffinity::Pack);

    let decisions = place_service_replicas("web", &spec, 3, &mut nodes, &mut placements);

    let distinct: HashSet<u64> = decisions.iter().filter_map(|d| d.node_id).collect();
    assert!(
        distinct.len() <= 2,
        "pack affinity should concentrate, got {distinct:?}"
    );
}

/// Regression for the `cluster_scaling` e2e: a Spread service that requests
/// ZERO cpu/mem (like the nginx fixture), on nodes at DIFFERENT utilization
/// (like a real cluster). Utilization never moves between replicas (zero
/// request) and is never tied (uneven), so ranking by utilization-first — with
/// anti-affinity only as a tie-break — would pile all three replicas onto the
/// single emptiest node. Spread must still distribute across all nodes.
#[test]
fn spread_affinity_distributes_with_zero_request_and_uneven_utilization() {
    let mut n1 = make_node(1, 8.0, 16 * GIB);
    let mut n2 = make_node(2, 8.0, 16 * GIB);
    let mut n3 = make_node(3, 8.0, 16 * GIB);
    // Distinct utilizations so there is never a utilization tie.
    n1.resources.cpu_used = 1.0; // 12.5%
    n2.resources.cpu_used = 2.0; // 25.0%
    n3.resources.cpu_used = 3.0; // 37.5%
    let mut nodes = vec![n1, n2, n3];
    let mut placements = PlacementState::new();
    // `None` cpu request => zero per-replica footprint, exactly like the
    // nginx-v1-3r.yaml fixture (no `resources:` block).
    let spec = make_spec_affinity(None, GroupAffinity::Spread);

    let decisions = place_service_replicas("web", &spec, 3, &mut nodes, &mut placements);

    assert!(
        decisions.iter().all(PlacementDecision::is_success),
        "every replica should be placed, got: {decisions:?}"
    );
    let distinct: HashSet<u64> = decisions.iter().filter_map(|d| d.node_id).collect();
    assert_eq!(
        distinct.len(),
        3,
        "spread must distribute across all 3 nodes despite zero requests and \
         uneven utilization, got {distinct:?}"
    );
}

/// Mirror of the leader fan-out in
/// `RaftCluster::dispatch_scale_distributed`: place the replicas, then group
/// the decisions into a `node_id -> count` map (one `dispatch_scale` call per
/// node). This is the exact step the `cluster_scaling` bug skipped — the leader
/// dispatched every replica to itself instead of grouping per node.
///
/// Contract proven here:
///   1. `affinity: spread` yields >= 2 distinct target nodes.
///   2. The per-node counts sum to the total replica count (no replica lost or
///      double-dispatched).
///   3. `affinity: pack` (and the default `None`) concentrate onto <= 2 nodes.
fn group_per_node(decisions: &[PlacementDecision]) -> std::collections::HashMap<u64, u32> {
    let mut per_node: std::collections::HashMap<u64, u32> = std::collections::HashMap::new();
    for d in decisions {
        if let Some(node_id) = d.node_id {
            *per_node.entry(node_id).or_insert(0) += 1;
        }
    }
    per_node
}

#[test]
fn distributed_dispatch_spread_splits_across_at_least_two_nodes() {
    let mut nodes = vec![
        make_node(1, 10.0, 16 * GIB),
        make_node(2, 10.0, 16 * GIB),
        make_node(3, 10.0, 16 * GIB),
    ];
    let mut placements = PlacementState::new();
    let spec = make_spec_affinity(Some(1.0), GroupAffinity::Spread);

    let decisions = place_service_replicas("web", &spec, 3, &mut nodes, &mut placements);
    let per_node = group_per_node(&decisions);

    // (1) spread => at least two distinct nodes get a share. This is the exact
    // assertion the e2e `cluster_scaling` suite makes
    // (`distinct={'1'}` failure before the fix).
    assert!(
        per_node.len() >= 2,
        "spread must fan out across >= 2 nodes; got per-node {per_node:?}"
    );
    // (2) the dispatched counts sum to the requested replica total.
    let total: u32 = per_node.values().sum();
    assert_eq!(
        total, 3,
        "sum of per-node dispatch counts must equal total replicas; got {per_node:?}"
    );
}

#[test]
fn distributed_dispatch_pack_concentrates_but_preserves_total() {
    let mut nodes = vec![
        make_node(1, 10.0, 16 * GIB),
        make_node(2, 10.0, 16 * GIB),
        make_node(3, 10.0, 16 * GIB),
    ];
    let mut placements = PlacementState::new();
    // Default affinity is Pack (None). Use an explicit Pack to be unambiguous.
    let spec = make_spec_affinity(Some(1.0), GroupAffinity::Pack);

    let decisions = place_service_replicas("web", &spec, 3, &mut nodes, &mut placements);
    let per_node = group_per_node(&decisions);

    // (3) pack concentrates onto few nodes...
    assert!(
        per_node.len() <= 2,
        "pack must concentrate onto <= 2 nodes; got per-node {per_node:?}"
    );
    // ...without ever dropping a replica from the fan-out.
    let total: u32 = per_node.values().sum();
    assert_eq!(
        total, 3,
        "sum of per-node dispatch counts must equal total replicas; got {per_node:?}"
    );
}

/// Capacity always wins over affinity: a `pack`/default service whose
/// per-replica CPU footprint exceeds a single node's headroom is forced to
/// spread, mirroring the user's "2 CPU replica on a 2-CPU node" example —
/// the second replica cannot co-locate and must land elsewhere.
#[test]
fn capacity_forces_spread_even_when_packing() {
    // Two nodes, 2 vCPU each. A replica needs 2 vCPU, so only one fits per node.
    let mut nodes = vec![make_node(1, 2.0, 6 * GIB), make_node(2, 2.0, 6 * GIB)];
    let mut placements = PlacementState::new();
    // Default affinity (None => Pack/concentrate). Capacity must still spread.
    let spec = make_spec(NodeMode::Shared, Some(2.0));

    let decisions = place_service_replicas("heavy", &spec, 2, &mut nodes, &mut placements);

    assert_eq!(decisions.len(), 2);
    assert!(
        decisions.iter().all(PlacementDecision::is_success),
        "both replicas should place across the two nodes, got: {decisions:?}"
    );
    let distinct: HashSet<u64> = decisions.iter().filter_map(|d| d.node_id).collect();
    assert_eq!(
        distinct.len(),
        2,
        "a 2-vCPU replica cannot share a 2-vCPU node; placement must spread, got {distinct:?}"
    );

    // A third replica has nowhere to fit — both nodes are full.
    let third = place_service_replicas("heavy", &spec, 1, &mut nodes, &mut placements);
    assert!(
        !third[0].is_success(),
        "no capacity remains for a third 2-vCPU replica, expected pending: {third:?}"
    );
}

/// `affinity: pin("id=2")` binds every replica to the named node.
#[test]
fn shared_mode_pin_affinity_binds_to_node() {
    let mut nodes = vec![
        make_node(1, 10.0, 16 * GIB),
        make_node(2, 10.0, 16 * GIB),
        make_node(3, 10.0, 16 * GIB),
    ];
    let mut placements = PlacementState::new();
    let spec = make_spec_affinity(Some(1.0), GroupAffinity::Pin("id=2".to_string()));

    let decisions = place_service_replicas("pinned", &spec, 3, &mut nodes, &mut placements);

    assert_eq!(decisions.len(), 3);
    assert!(
        decisions.iter().all(PlacementDecision::is_success),
        "every replica should be placed on the pinned node, got: {decisions:?}"
    );
    assert!(
        decisions.iter().all(|d| d.node_id == Some(2)),
        "pin must bind all replicas to node 2, got {:?}",
        decisions.iter().map(|d| d.node_id).collect::<Vec<_>>()
    );
}

/// README contract: in `dedicated` mode, each replica gets its own node
/// (1:1 mapping). With N replicas and N nodes the placement must assign
/// every node exactly once.
#[test]
fn dedicated_mode_assigns_one_replica_per_node() {
    let mut nodes = vec![
        make_node(1, 16.0, 32 * GIB),
        make_node(2, 16.0, 32 * GIB),
        make_node(3, 16.0, 32 * GIB),
    ];
    let mut placements = PlacementState::new();
    let spec = make_spec(NodeMode::Dedicated, Some(1.0));

    let decisions = place_service_replicas("api", &spec, 3, &mut nodes, &mut placements);

    assert_eq!(decisions.len(), 3);
    assert!(
        decisions.iter().all(PlacementDecision::is_success),
        "every replica should be placed, got: {decisions:?}"
    );

    let assigned: Vec<u64> = decisions.iter().filter_map(|d| d.node_id).collect();
    let distinct: HashSet<u64> = assigned.iter().copied().collect();
    assert_eq!(
        distinct.len(),
        assigned.len(),
        "dedicated mode must place each replica on a distinct node; got {assigned:?}"
    );
    assert_eq!(distinct.len(), 3, "all three nodes should be used");
}

/// README contract: in `exclusive` mode, a service has nodes exclusively to
/// itself — no other services share those nodes. We verify this from the
/// authoritative side (the side the scheduler actually enforces): an
/// exclusive placement refuses to land on nodes already occupied by a prior
/// service. Service A (exclusive) claims two nodes; a second exclusive
/// service B is then forced onto the one remaining empty node.
#[test]
fn exclusive_mode_blocks_other_services() {
    let mut nodes = vec![
        make_node(1, 16.0, 32 * GIB),
        make_node(2, 16.0, 32 * GIB),
        make_node(3, 16.0, 32 * GIB),
    ];
    let mut placements = PlacementState::new();

    // Service A: exclusive, 2 replicas — claims 2 of the 3 nodes.
    let spec_a = make_spec(NodeMode::Exclusive, Some(1.0));
    let decisions_a = place_service_replicas("svc-a", &spec_a, 2, &mut nodes, &mut placements);

    assert_eq!(decisions_a.len(), 2);
    assert!(
        decisions_a.iter().all(PlacementDecision::is_success),
        "exclusive service A failed to place all replicas: {decisions_a:?}"
    );
    let claimed: HashSet<u64> = decisions_a.iter().filter_map(|d| d.node_id).collect();
    assert_eq!(
        claimed.len(),
        2,
        "exclusive service A must occupy 2 distinct nodes, got {claimed:?}"
    );

    // The one node A didn't take.
    let free_node: u64 = [1u64, 2, 3]
        .into_iter()
        .find(|id| !claimed.contains(id))
        .expect("exactly one node should remain unclaimed");

    // Service B: also exclusive, 1 replica. The placement contract says no
    // other service may co-tenant with A on A's nodes; therefore B must land
    // on the single remaining empty node.
    let spec_b = make_spec(NodeMode::Exclusive, Some(1.0));
    let decisions_b = place_service_replicas("svc-b", &spec_b, 1, &mut nodes, &mut placements);

    assert_eq!(decisions_b.len(), 1);
    let decision = &decisions_b[0];
    assert!(
        decision.is_success(),
        "exclusive service B should fit on the one remaining empty node, got: {decision:?}"
    );
    assert_eq!(
        decision.node_id,
        Some(free_node),
        "exclusive service B must land on the unclaimed node {free_node}, got {:?}",
        decision.node_id
    );

    // And a second replica of B has nowhere to go: every node now hosts
    // exactly one exclusive service, so further exclusive placements pend.
    let decisions_b2 = place_service_replicas("svc-b2", &spec_b, 1, &mut nodes, &mut placements);
    assert_eq!(decisions_b2.len(), 1);
    assert!(
        !decisions_b2[0].is_success(),
        "no exclusive nodes should remain; expected pending placement, got: {:?}",
        decisions_b2[0]
    );
}
