//! High-level `ConsensusNode` wrapper around `openraft::Raft`.
//!
//! Provides a builder pattern for constructing Raft nodes with the desired
//! storage and network backends, plus convenience methods for common operations.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::sync::Arc;

use openraft::impls::OneshotResponder;
use openraft::{BasicNode, Raft, RaftMetrics, RaftTypeConfig};
use tracing::info;

use crate::config::ConsensusConfig;
use crate::error::{ConsensusError, Result};
use crate::types::NodeId;

/// A high-level wrapper around `openraft::Raft`.
///
/// Provides ergonomic methods for common Raft operations:
/// proposing writes, linearizable reads, cluster membership changes,
/// and metrics observation.
///
/// Construct via [`ConsensusNodeBuilder`].
///
/// Note: this requires `C::Responder = OneshotResponder<C>` which is the
/// default when using `declare_raft_types!`. This enables the blocking
/// `client_write`, `add_learner`, and `change_membership` APIs.
pub struct ConsensusNode<C>
where
    C: RaftTypeConfig<NodeId = NodeId, Node = BasicNode, Responder = OneshotResponder<C>>,
{
    /// The inner openraft `Raft` instance.
    raft: Raft<C>,
    /// This node's ID.
    node_id: NodeId,
    /// This node's advertised address.
    address: String,
}

impl<C> ConsensusNode<C>
where
    C: RaftTypeConfig<NodeId = NodeId, Node = BasicNode, Responder = OneshotResponder<C>>,
{
    /// Create a `ConsensusNode` directly from an already-constructed `Raft` instance.
    #[must_use]
    pub fn from_raft(raft: Raft<C>, node_id: NodeId, address: String) -> Self {
        Self {
            raft,
            node_id,
            address,
        }
    }

    /// This node's ID.
    #[must_use]
    pub fn node_id(&self) -> NodeId {
        self.node_id
    }

    /// This node's address.
    #[must_use]
    pub fn address(&self) -> &str {
        &self.address
    }

    /// Get a reference to the inner `Raft` instance for advanced usage.
    #[must_use]
    pub fn raft(&self) -> &Raft<C> {
        &self.raft
    }

    /// Get a clone of the inner `Raft` instance (cheap, Arc-based).
    #[must_use]
    pub fn raft_clone(&self) -> Raft<C> {
        self.raft.clone()
    }

    /// Propose a client write to the Raft cluster.
    ///
    /// This node must be the leader. The write is replicated to a quorum
    /// before the response is returned.
    ///
    /// # Errors
    /// Returns `ConsensusError::Write` if the write fails (e.g., not the leader).
    pub async fn propose(&self, request: C::D) -> Result<C::R> {
        let result = self
            .raft
            .client_write(request)
            .await
            .map_err(|e| ConsensusError::Write(e.to_string()))?;

        Ok(result.data)
    }

    /// Ensure linearizable reads by confirming this node is still the leader.
    ///
    /// Call this before reading from the state machine to guarantee that the
    /// data is not stale. Implements the "leader lease read" pattern.
    ///
    /// # Errors
    /// Returns `ConsensusError::Write` if the linearizable check fails.
    pub async fn ensure_linearizable(&self) -> Result<()> {
        self.raft
            .ensure_linearizable()
            .await
            .map_err(|e| ConsensusError::Write(format!("linearizable check failed: {e}")))?;
        Ok(())
    }

    /// Check if this node is the current leader.
    #[must_use]
    pub fn is_leader(&self) -> bool {
        let metrics = self.raft.metrics().borrow().clone();
        metrics.current_leader == Some(self.node_id)
    }

    /// Get the current leader's node ID, if known.
    #[must_use]
    pub fn leader_id(&self) -> Option<NodeId> {
        self.raft.metrics().borrow().current_leader
    }

    /// Returns the set of current voter node IDs.
    #[must_use]
    pub fn voter_ids(&self) -> BTreeSet<NodeId> {
        let metrics = self.raft.metrics().borrow().clone();
        metrics.membership_config.membership().voter_ids().collect()
    }

    /// Returns the number of current voters.
    #[must_use]
    pub fn voter_count(&self) -> usize {
        self.voter_ids().len()
    }

    /// Returns the set of current learner node IDs (non-voters).
    #[must_use]
    pub fn learner_ids(&self) -> BTreeSet<NodeId> {
        let metrics = self.raft.metrics().borrow().clone();
        metrics
            .membership_config
            .membership()
            .learner_ids()
            .collect()
    }

    /// Returns all member node IDs (voters + learners).
    #[must_use]
    pub fn all_member_ids(&self) -> BTreeSet<NodeId> {
        let metrics = self.raft.metrics().borrow().clone();
        let membership = metrics.membership_config.membership();
        let mut ids: BTreeSet<NodeId> = membership.voter_ids().collect();
        ids.extend(membership.learner_ids());
        ids
    }

    /// Bootstrap a new single-node cluster.
    ///
    /// This must only be called once, on the first node, when creating a new cluster.
    ///
    /// # Errors
    /// Returns `ConsensusError::Init` if bootstrap initialization fails.
    pub async fn bootstrap(&self) -> Result<()> {
        let mut members = BTreeMap::new();
        members.insert(
            self.node_id,
            BasicNode {
                addr: self.address.clone(),
            },
        );

        self.raft
            .initialize(members)
            .await
            .map_err(|e| ConsensusError::Init(format!("bootstrap failed: {e}")))?;

        info!(node_id = self.node_id, "Bootstrapped single-node cluster");
        Ok(())
    }

    /// Add a learner to the cluster.
    ///
    /// A learner receives log entries but does not vote. Use this to
    /// pre-sync a node before promoting it to a voter.
    ///
    /// If `blocking` is true, waits until the learner has caught up with the log.
    ///
    /// # Errors
    /// Returns `ConsensusError::Membership` if the learner cannot be added.
    pub async fn add_learner(
        &self,
        node_id: NodeId,
        address: String,
        blocking: bool,
    ) -> Result<()> {
        let node = BasicNode { addr: address };

        self.raft
            .add_learner(node_id, node, blocking)
            .await
            .map_err(|e| {
                ConsensusError::Membership(format!("add_learner({node_id}) failed: {e}"))
            })?;

        info!(node_id, "Added learner");
        Ok(())
    }

    /// Change the cluster membership (promote learners to voters, or remove members).
    ///
    /// Pass the complete set of voter node IDs. Nodes not in the set will be
    /// demoted or removed.
    ///
    /// If `retain` is true, nodes not in `voter_ids` are kept as learners
    /// rather than removed entirely.
    ///
    /// # Errors
    /// Returns `ConsensusError::Membership` if the membership change fails.
    pub async fn change_membership(&self, voter_ids: BTreeSet<NodeId>, retain: bool) -> Result<()> {
        self.raft
            .change_membership(voter_ids, retain)
            .await
            .map_err(|e| ConsensusError::Membership(format!("change_membership failed: {e}")))?;

        info!("Membership change committed");
        Ok(())
    }

    /// Convenience: add a node as learner then promote it to voter.
    ///
    /// This performs the full two-step process:
    /// 1. Add as learner (blocking, waits for log sync)
    /// 2. Change membership to include the new voter
    ///
    /// # Errors
    /// Returns a `ConsensusError` if either the learner addition or membership change fails.
    pub async fn add_voter(&self, node_id: NodeId, address: String) -> Result<()> {
        // Step 1: add as learner
        self.add_learner(node_id, address, true).await?;

        // Step 2: collect current voters and add the new one
        let metrics = self.raft.metrics().borrow().clone();
        let mut voter_ids = BTreeSet::new();

        if let Some(membership) = metrics
            .membership_config
            .membership()
            .get_joint_config()
            .last()
        {
            for id in membership {
                voter_ids.insert(*id);
            }
        }
        voter_ids.insert(self.node_id);
        voter_ids.insert(node_id);

        self.change_membership(voter_ids, false).await?;

        info!(node_id, "Promoted learner to voter");
        Ok(())
    }

    /// Get current Raft metrics (leader, term, log indices, etc.).
    #[must_use]
    pub fn metrics(&self) -> RaftMetrics<NodeId, BasicNode> {
        self.raft.metrics().borrow().clone()
    }

    /// Gracefully shut down the Raft node.
    ///
    /// # Errors
    /// Returns `ConsensusError::Fatal` if shutdown fails.
    pub async fn shutdown(&self) -> Result<()> {
        self.raft
            .shutdown()
            .await
            .map_err(|e| ConsensusError::Fatal(format!("shutdown failed: {e}")))?;

        info!(node_id = self.node_id, "Raft node shut down");
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

/// Builder for constructing a `ConsensusNode`.
///
/// # Example
///
/// ```ignore
/// use zlayer_consensus_zql::ConsensusNodeBuilder;
///
/// let node = ConsensusNodeBuilder::new(1, "127.0.0.1:9000".into())
///     .with_config(ConsensusConfig::default())
///     .build_with(log_store, state_machine, network)
///     .await?;
/// ```
pub struct ConsensusNodeBuilder {
    node_id: NodeId,
    address: String,
    config: ConsensusConfig,
}

impl ConsensusNodeBuilder {
    /// Create a new builder for the given node ID and address.
    #[must_use]
    pub fn new(node_id: NodeId, address: String) -> Self {
        Self {
            node_id,
            address,
            config: ConsensusConfig::default(),
        }
    }

    /// Set the consensus configuration.
    #[must_use]
    pub fn with_config(mut self, config: ConsensusConfig) -> Self {
        self.config = config;
        self
    }

    /// Build the `ConsensusNode` with the provided storage and network implementations.
    ///
    /// This is the most flexible build method -- you provide all three components.
    ///
    /// # Errors
    /// Returns `ConsensusError::Fatal` if the Raft instance fails to start, or
    /// a configuration error if the consensus config is invalid.
    pub async fn build_with<C, LS, SM, N>(
        self,
        log_store: LS,
        state_machine: SM,
        network: N,
    ) -> Result<ConsensusNode<C>>
    where
        C: RaftTypeConfig<NodeId = NodeId, Node = BasicNode, Responder = OneshotResponder<C>>,
        LS: openraft::storage::RaftLogStorage<C>,
        SM: openraft::storage::RaftStateMachine<C>,
        N: openraft::network::RaftNetworkFactory<C>,
    {
        let raft_config = Arc::new(self.config.to_openraft_config()?);

        let raft = Raft::new(self.node_id, raft_config, network, log_store, state_machine)
            .await
            .map_err(|e| ConsensusError::Fatal(format!("Failed to create Raft: {e}")))?;

        info!(
            node_id = self.node_id,
            address = %self.address,
            "Created ConsensusNode"
        );

        Ok(ConsensusNode {
            raft,
            node_id: self.node_id,
            address: self.address,
        })
    }
}
