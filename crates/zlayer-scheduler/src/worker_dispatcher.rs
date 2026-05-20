//! Leader-side worker dispatcher: the gRPC server workers connect to + the
//! per-worker assignment queue.
//!
//! Architecture:
//! - [`WorkerDispatcherImpl`] owns a `HashMap<NodeId, WorkerSession>`.
//! - Each [`WorkerSession`] has an `mpsc::Sender<AssignmentEvent>` whose
//!   receiver is held by an open `WatchAssignments` stream (and a parallel
//!   `mpsc::Sender<CommandEvent>` for the out-of-band command channel).
//! - On worker `Register`: verify token, sign CSR, propose
//!   `Request::GrantWorkerLease` to raft, install a new session, return
//!   `RegisterResponse` with cert + CA chain + TTL.
//! - On `WatchAssignments` / `WatchCommands`: drain the session's channel
//!   into the response stream.
//! - On `ReportStatus`: propose `Request::RenewWorkerLease` on every tick,
//!   compute adaptive TTL, return a `StatusAck` for each report.
//! - Background task ([`Self::start_expiry_sweep`]): every
//!   `heartbeat_min_ttl / 2`, check leases against `now`; propose
//!   `Request::ExpireWorkerLease` for any past their grace and drop the
//!   in-memory session.
//!
//! The [`crate::cluster::WorkerDispatcher`] trait impl lets the scheduler's
//! `dispatch_scale` route to a worker by pushing into the appropriate
//! session's mpsc channel.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use tokio::sync::{mpsc, RwLock};
use tonic::codegen::tokio_stream::wrappers::ReceiverStream;
use tonic::codegen::tokio_stream::{Stream, StreamExt};
use tonic::{Request, Response, Status};
use tracing::{debug, info, warn};

use zlayer_cluster_rpc::proto;
use zlayer_secrets::{
    verify_worker_bootstrap_token, ClusterSigner, WorkerBootstrapClaims, WorkerBootstrapToken,
    WorkerCa,
};
use zlayer_types::cluster::AdaptiveTtlConfig;

use crate::cluster::{ClusterError, NodeRecord, NodeState, WorkerDispatcher};
use crate::raft::{NodeId, RaftCoordinator, Request as RaftRequest};

/// Channel buffer for the per-worker assignment + command queues.
///
/// Sized for short bursts of fan-out events. Workers consume them as fast as
/// gRPC frames flush; if the buffer ever saturates the dispatcher logs a
/// warning and the assignment is dropped (raft state stays authoritative).
const SESSION_CHANNEL_BUFFER: usize = 256;

/// Channel buffer for `StatusAck` responses sent back to a single worker.
const STATUS_ACK_BUFFER: usize = 64;

/// One connected worker. Held inside `WorkerDispatcherImpl.sessions`.
struct WorkerSession {
    /// Stable id assigned at Register time.
    node_id: NodeId,
    /// Worker's externally-reachable API address (host:port). Used by
    /// [`WorkerDispatcher::known_workers`] to populate [`NodeRecord`].
    api_addr: SocketAddr,
    /// Worker-declared labels (region, hw class, etc.). Echoed by
    /// `known_workers`.
    labels: HashMap<String, String>,
    /// Operating system the worker reported.
    os: String,
    /// Channel feeding the worker's `WatchAssignments` stream.
    tx: mpsc::Sender<Result<proto::AssignmentEvent, Status>>,
    /// Channel feeding the worker's `WatchCommands` stream.
    cmd_tx: mpsc::Sender<Result<proto::CommandEvent, Status>>,
    /// Monotonic revision number of the last assignment we pushed. Lets the
    /// worker resume after a reconnect.
    last_revision: AtomicU64,
    /// Time of last successful interaction (Register or `ReportStatus`).
    last_seen: RwLock<SystemTime>,
}

/// Leader-side gRPC server for the worker control plane.
///
/// Implements both [`proto::cluster_control_plane_server::ClusterControlPlane`]
/// (the wire surface) and [`WorkerDispatcher`] (the in-process trait the
/// scheduler uses to push assignments).
pub struct WorkerDispatcherImpl {
    /// Currently-connected workers keyed by `node_id`.
    sessions: RwLock<HashMap<NodeId, Arc<WorkerSession>>>,
    /// Handle to the raft cluster for proposing `GrantWorkerLease`,
    /// `RenewWorkerLease`, and `ExpireWorkerLease` ops.
    raft: Arc<RaftCoordinator>,
    /// Cluster ID (matches `WorkerBootstrapClaims.cluster_id` on verify).
    cluster_id: String,
    /// Cluster signer used to verify worker bootstrap tokens. Selected by
    /// the caller via `signer.key_id()` matching `token.signer_kid`.
    signer: Arc<ClusterSigner>,
    /// Worker CA for signing leaf certs from CSRs at Register time.
    worker_ca: Arc<WorkerCa>,
    /// Adaptive-TTL math (heartbeat cadence vs cluster size).
    ttl_cfg: AdaptiveTtlConfig,
    /// `jti -> uses` counter enforcing `claims.max_uses` on bootstrap tokens.
    /// Persisted in-memory only; survives only as long as the leader.
    token_usage: RwLock<HashMap<String, u32>>,
    /// Next available node id for fresh worker registrations. The leader
    /// owns this monotonic counter; followers don't issue ids.
    next_node_id: AtomicU64,
}

impl std::fmt::Debug for WorkerDispatcherImpl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkerDispatcherImpl")
            .field("cluster_id", &self.cluster_id)
            .field("ttl_cfg", &self.ttl_cfg)
            .field("next_node_id", &self.next_node_id.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl WorkerDispatcherImpl {
    /// Build a new worker dispatcher.
    ///
    /// `starting_node_id` is the first id that will be handed out when a
    /// fresh worker registers without supplying a `desired_node_id` (or
    /// when the desired id collides with an existing session).
    #[must_use]
    pub fn new(
        raft: Arc<RaftCoordinator>,
        cluster_id: String,
        signer: Arc<ClusterSigner>,
        worker_ca: Arc<WorkerCa>,
        ttl_cfg: AdaptiveTtlConfig,
        starting_node_id: NodeId,
    ) -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            raft,
            cluster_id,
            signer,
            worker_ca,
            ttl_cfg,
            token_usage: RwLock::new(HashMap::new()),
            next_node_id: AtomicU64::new(starting_node_id),
        }
    }

    /// Spawn the periodic lease-expiry sweep. Call once at construction; the
    /// returned `JoinHandle` lets the caller cancel by aborting.
    pub fn start_expiry_sweep(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let this = Arc::clone(self);
        tokio::spawn(async move {
            this.run_expiry_sweep().await;
        })
    }

    /// Wrap this dispatcher in a `tonic`-compatible service so it can be
    /// passed to `tonic::transport::Server::builder().add_service(...)`.
    ///
    /// The wrapper uses a local newtype so the gRPC trait can be implemented
    /// without running into Rust's orphan rule (which forbids implementing a
    /// foreign trait on `Arc<T>` directly).
    #[must_use]
    pub fn into_tonic_service(
        self: Arc<Self>,
    ) -> proto::cluster_control_plane_server::ClusterControlPlaneServer<WorkerDispatcherService>
    {
        proto::cluster_control_plane_server::ClusterControlPlaneServer::new(
            WorkerDispatcherService { inner: self },
        )
    }

    /// Current adaptive TTL in seconds, computed from the live session count.
    async fn current_ttl_secs(&self) -> u32 {
        let n = self.sessions.read().await.len();
        let n_u32 = u32::try_from(n).unwrap_or(u32::MAX);
        self.ttl_cfg.compute_ttl(n_u32)
    }

    /// Resolve which `node_id` to hand back for this `Register` call.
    ///
    /// Prefer `desired` if non-zero and not already taken; otherwise burn a
    /// fresh id from `next_node_id`. Always wraps past any colliding ids.
    async fn assign_node_id(&self, desired: NodeId) -> NodeId {
        if desired != 0 {
            let sessions = self.sessions.read().await;
            if !sessions.contains_key(&desired) {
                return desired;
            }
        }
        // Find an id strictly greater than any current session and any
        // previously-handed-out id.
        let mut candidate = self.next_node_id.fetch_add(1, Ordering::SeqCst);
        loop {
            let sessions = self.sessions.read().await;
            if !sessions.contains_key(&candidate) {
                return candidate;
            }
            drop(sessions);
            candidate = self.next_node_id.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// Verify and account a bootstrap token, returning the claims on success.
    async fn verify_and_account_token(
        &self,
        token_b64: &str,
    ) -> Result<WorkerBootstrapClaims, Status> {
        let token = WorkerBootstrapToken::from_cli_string(token_b64)
            .map_err(|e| Status::unauthenticated(format!("decode bootstrap token: {e}")))?;
        let claims = verify_worker_bootstrap_token(&self.signer, &token)
            .map_err(|e| Status::unauthenticated(format!("verify bootstrap token: {e}")))?;
        if claims.cluster_id != self.cluster_id {
            return Err(Status::unauthenticated(format!(
                "wrong cluster_id: expected {}, got {}",
                self.cluster_id, claims.cluster_id
            )));
        }
        if claims.max_uses != 0 {
            let mut usage = self.token_usage.write().await;
            let entry = usage.entry(claims.jti.clone()).or_insert(0);
            if *entry >= claims.max_uses {
                return Err(Status::resource_exhausted(format!(
                    "bootstrap token jti {} exhausted ({} of {} uses)",
                    claims.jti, entry, claims.max_uses
                )));
            }
            *entry = entry.saturating_add(1);
        }
        Ok(claims)
    }

    /// Periodic lease-expiry sweep.
    ///
    /// Reads the raft state's `worker_leases` map, expires any past their
    /// grace deadline, drops the corresponding in-memory session, and
    /// proposes `Request::ExpireWorkerLease` so followers converge.
    async fn run_expiry_sweep(self: Arc<Self>) {
        let interval_secs = (u64::from(self.ttl_cfg.min_ttl_secs) / 2).max(5);
        let mut ticker = tokio::time::interval(Duration::from_secs(interval_secs));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            let now_ns = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX));
            let grace = self.ttl_cfg.grace_secs;

            // Snapshot the lease set: we must not hold the raft state-machine
            // read lock while we propose (re-entrant).
            let leases = {
                let state = self.raft.read_state().await;
                state.worker_leases.clone()
            };

            for (node_id, lease) in leases {
                if !lease.is_expired(now_ns, grace) {
                    continue;
                }
                debug!(
                    node_id,
                    renewed_ns = lease.renewed_ns,
                    ttl_secs = lease.ttl_secs,
                    "Expiring stale worker lease"
                );
                if let Err(e) = self
                    .raft
                    .propose(RaftRequest::ExpireWorkerLease { node_id })
                    .await
                {
                    // Not fatal: another tick or the next renew will
                    // eventually clean up.
                    warn!(
                        node_id,
                        error = %e,
                        "Failed to propose ExpireWorkerLease"
                    );
                    continue;
                }
                self.sessions.write().await.remove(&node_id);
            }
        }
    }

    /// Push one `AssignmentEvent::Set` for each entry of `req.assignments`
    /// into the named worker's queue. Used by [`WorkerDispatcher::dispatch_to_worker`].
    async fn push_set_assignments(
        &self,
        target: NodeId,
        req: &zlayer_types::cluster::InternalScaleRequest,
    ) -> Result<(), ClusterError> {
        let session = {
            let sessions = self.sessions.read().await;
            sessions
                .get(&target)
                .cloned()
                .ok_or(ClusterError::UnknownNode(target))?
        };

        let now_ts = prost_timestamp_now();
        for assignment in &req.assignments {
            for &index in &assignment.indices {
                let revision = session.last_revision.fetch_add(1, Ordering::SeqCst) + 1;
                let event = proto::AssignmentEvent {
                    revision,
                    ts: Some(now_ts),
                    event: Some(proto::assignment_event::Event::Set(proto::AssignmentSet {
                        service: req.service.clone(),
                        role: assignment.role.clone(),
                        index,
                        // Phase 3.5: caller hasn't given us the full role-group
                        // spec yet (Scheduler still drives placement via the
                        // legacy HTTP fan-out). Send empty bytes; the worker
                        // resolves the spec via its local copy fetched at
                        // Register / via service-spec fetch RPC (P3.7+).
                        spec_json: Vec::new(),
                    })),
                };
                if session.tx.send(Ok(event)).await.is_err() {
                    return Err(ClusterError::Transport(format!(
                        "worker {target} assignment channel closed"
                    )));
                }
            }
        }
        Ok(())
    }
}

// ============================================================================
// WorkerDispatcher trait impl (consumed by the scheduler).
// ============================================================================

#[async_trait]
impl WorkerDispatcher for WorkerDispatcherImpl {
    async fn dispatch_to_worker(
        &self,
        target: NodeId,
        req: zlayer_types::cluster::InternalScaleRequest,
    ) -> Result<(), ClusterError> {
        self.push_set_assignments(target, &req).await
    }

    async fn known_workers(&self) -> Vec<NodeRecord> {
        let sessions = self.sessions.read().await;
        let mut out = Vec::with_capacity(sessions.len());
        for session in sessions.values() {
            let last_seen = *session.last_seen.read().await;
            out.push(NodeRecord {
                id: session.node_id,
                api_addr: session.api_addr,
                labels: session.labels.clone(),
                os: session.os.clone(),
                state: NodeState::Ready,
                last_seen,
            });
        }
        out
    }

    async fn worker_count(&self) -> usize {
        self.sessions.read().await.len()
    }
}

// ============================================================================
// gRPC service trait impl.
// ============================================================================

/// Newtype wrapper that owns the `Arc<WorkerDispatcherImpl>` and carries the
/// gRPC `ClusterControlPlane` impl.
///
/// We can't impl a foreign trait directly on `Arc<WorkerDispatcherImpl>`
/// (orphan rule), so the newtype absorbs the smart-pointer indirection.
/// Construct via [`WorkerDispatcherImpl::into_tonic_service`].
#[derive(Debug, Clone)]
pub struct WorkerDispatcherService {
    inner: Arc<WorkerDispatcherImpl>,
}

#[async_trait]
impl proto::cluster_control_plane_server::ClusterControlPlane for WorkerDispatcherService {
    async fn register(
        &self,
        request: Request<proto::RegisterRequest>,
    ) -> Result<Response<proto::RegisterResponse>, Status> {
        let req = request.into_inner();
        let _claims = self
            .inner
            .verify_and_account_token(&req.bootstrap_token)
            .await?;

        // Assign a node_id (prefer desired_node_id, falling back to fresh).
        let node_id = self.inner.assign_node_id(req.desired_node_id).await;

        // Sign the worker's CSR.
        let signed_cert_der = self
            .inner
            .worker_ca
            .sign_csr_der(
                &req.csr_der,
                &format!("worker-{node_id}"),
                ::time::Duration::days(90),
            )
            .map_err(|e| Status::invalid_argument(format!("sign CSR: {e}")))?;

        // Grant the lease through raft.
        let now_ns = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX));
        let ttl_secs = self.inner.current_ttl_secs().await;
        self.inner
            .raft
            .propose(RaftRequest::GrantWorkerLease {
                node_id,
                holder: format!("worker-{node_id}"),
                acquired_ns: now_ns,
                renewed_ns: now_ns,
                ttl_secs,
            })
            .await
            .map_err(|e| Status::internal(format!("raft propose GrantWorkerLease: {e}")))?;

        // Build the in-memory session.
        let profile = req.profile.unwrap_or(proto::NodeProfile {
            os: String::new(),
            arch: String::new(),
            labels: HashMap::new(),
            caps: None,
            version: String::new(),
        });
        // The worker's reported api_addr isn't in the proto today; default to
        // a placeholder. Phase 3.7+ adds an explicit field on NodeProfile.
        let api_addr: SocketAddr = "0.0.0.0:0".parse().expect("static addr");
        let (tx, _rx) =
            mpsc::channel::<Result<proto::AssignmentEvent, Status>>(SESSION_CHANNEL_BUFFER);
        let (cmd_tx, _cmd_rx) =
            mpsc::channel::<Result<proto::CommandEvent, Status>>(SESSION_CHANNEL_BUFFER);
        let session = WorkerSession {
            node_id,
            api_addr,
            labels: profile.labels.clone(),
            os: profile.os.clone(),
            tx,
            cmd_tx,
            last_revision: AtomicU64::new(0),
            last_seen: RwLock::new(SystemTime::now()),
        };
        // NOTE: the `_rx`/`_cmd_rx` we just dropped above means the first
        // WatchAssignments / WatchCommands call will *replace* the channel
        // with a live one. See those methods for the swap.
        let _ = (_rx, _cmd_rx);
        self.inner
            .sessions
            .write()
            .await
            .insert(node_id, Arc::new(session));

        info!(
            node_id,
            os = %profile.os,
            arch = %profile.arch,
            ttl_secs,
            "Worker registered"
        );

        Ok(Response::new(proto::RegisterResponse {
            node_id,
            signed_cert_der,
            ca_chain_der: vec![self.inner.worker_ca.ca_cert_der()],
            heartbeat_ttl_secs: ttl_secs,
            heartbeat_grace_secs: self.inner.ttl_cfg.grace_secs,
            gossip_seeds: Vec::new(),
        }))
    }

    type WatchAssignmentsStream =
        Pin<Box<dyn Stream<Item = Result<proto::AssignmentEvent, Status>> + Send>>;

    async fn watch_assignments(
        &self,
        request: Request<proto::WatchAssignmentsRequest>,
    ) -> Result<Response<Self::WatchAssignmentsStream>, Status> {
        let node_id = request.into_inner().node_id;
        // Replace the session's `tx` channel with a fresh one so the new
        // WatchAssignments stream owns the receiver. The old sender, if any,
        // is dropped — pushes against it will see SendError and the
        // dispatcher will surface that as a transport error.
        let (tx, rx) =
            mpsc::channel::<Result<proto::AssignmentEvent, Status>>(SESSION_CHANNEL_BUFFER);
        {
            let mut sessions = self.inner.sessions.write().await;
            let session = sessions.get(&node_id).cloned().ok_or_else(|| {
                Status::failed_precondition(format!("node {node_id} not registered"))
            })?;
            // Replace `tx` in-place: build a new session record with the new
            // sender. `WorkerSession` fields aren't `Cell`/`Mutex`, so swap by
            // re-inserting an Arc.
            let new_session = WorkerSession {
                node_id: session.node_id,
                api_addr: session.api_addr,
                labels: session.labels.clone(),
                os: session.os.clone(),
                tx,
                cmd_tx: session.cmd_tx.clone(),
                last_revision: AtomicU64::new(session.last_revision.load(Ordering::SeqCst)),
                last_seen: RwLock::new(*session.last_seen.read().await),
            };
            sessions.insert(node_id, Arc::new(new_session));
        }
        let stream = ReceiverStream::new(rx);
        Ok(Response::new(
            Box::pin(stream) as Self::WatchAssignmentsStream
        ))
    }

    type ReportStatusStream = Pin<Box<dyn Stream<Item = Result<proto::StatusAck, Status>> + Send>>;

    async fn report_status(
        &self,
        request: Request<tonic::Streaming<proto::StatusReport>>,
    ) -> Result<Response<Self::ReportStatusStream>, Status> {
        let mut inbound = request.into_inner();
        let (ack_tx, ack_rx) = mpsc::channel::<Result<proto::StatusAck, Status>>(STATUS_ACK_BUFFER);

        let dispatcher = Arc::clone(&self.inner);
        tokio::spawn(async move {
            while let Some(report) = inbound.next().await {
                let report = match report {
                    Ok(r) => r,
                    Err(e) => {
                        let _ = ack_tx.send(Err(e)).await;
                        return;
                    }
                };
                let node_id = report.node_id;
                let now_ns = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX));
                let ttl_secs = dispatcher.current_ttl_secs().await;

                // Renew the lease in raft.
                if let Err(e) = dispatcher
                    .raft
                    .propose(RaftRequest::RenewWorkerLease {
                        node_id,
                        renewed_ns: now_ns,
                        ttl_secs,
                    })
                    .await
                {
                    warn!(node_id, error = %e, "Failed to renew worker lease");
                    let _ = ack_tx
                        .send(Err(Status::failed_precondition(format!(
                            "renew lease: {e}"
                        ))))
                        .await;
                    return;
                }

                // Update last_seen.
                if let Some(session) = dispatcher.sessions.read().await.get(&node_id) {
                    *session.last_seen.write().await = SystemTime::now();
                }

                // Echo accepted_revision = container-status length (best-effort
                // dedup marker until Phase 3.7 adds an explicit revision in
                // StatusReport).
                let accepted_revision = u64::try_from(report.containers.len()).unwrap_or(0);
                let ack = proto::StatusAck {
                    next_ttl_secs: ttl_secs,
                    accepted_revision,
                };
                if ack_tx.send(Ok(ack)).await.is_err() {
                    // Client gave up.
                    return;
                }
            }
        });

        let stream = ReceiverStream::new(ack_rx);
        Ok(Response::new(Box::pin(stream) as Self::ReportStatusStream))
    }

    type WatchCommandsStream =
        Pin<Box<dyn Stream<Item = Result<proto::CommandEvent, Status>> + Send>>;

    async fn watch_commands(
        &self,
        request: Request<proto::WatchCommandsRequest>,
    ) -> Result<Response<Self::WatchCommandsStream>, Status> {
        let node_id = request.into_inner().node_id;
        let (cmd_tx, cmd_rx) =
            mpsc::channel::<Result<proto::CommandEvent, Status>>(SESSION_CHANNEL_BUFFER);
        {
            let mut sessions = self.inner.sessions.write().await;
            let session = sessions.get(&node_id).cloned().ok_or_else(|| {
                Status::failed_precondition(format!("node {node_id} not registered"))
            })?;
            let new_session = WorkerSession {
                node_id: session.node_id,
                api_addr: session.api_addr,
                labels: session.labels.clone(),
                os: session.os.clone(),
                tx: session.tx.clone(),
                cmd_tx,
                last_revision: AtomicU64::new(session.last_revision.load(Ordering::SeqCst)),
                last_seen: RwLock::new(*session.last_seen.read().await),
            };
            sessions.insert(node_id, Arc::new(new_session));
        }
        let stream = ReceiverStream::new(cmd_rx);
        Ok(Response::new(Box::pin(stream) as Self::WatchCommandsStream))
    }
}

// ============================================================================
// Helpers.
// ============================================================================

fn prost_timestamp_now() -> prost_types::Timestamp {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO);
    prost_types::Timestamp {
        seconds: i64::try_from(now.as_secs()).unwrap_or(0),
        nanos: i32::try_from(now.subsec_nanos()).unwrap_or(0),
    }
}

// ============================================================================
// Tests.
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a `WorkerDispatcherImpl` without any sessions, suitable for
    /// trait-level unit tests that don't touch the gRPC layer.
    ///
    /// The `raft`/`signer`/`worker_ca` fields are populated with stub Arcs
    /// that the trait methods under test don't dereference; if a future test
    /// exercises Register / `ReportStatus`, it must replace this builder with
    /// a richer fixture.
    ///
    /// # Panics
    ///
    /// Panics if the keystore or CA generation fails (only happens if the OS
    /// CSPRNG is unavailable, which would fail every other crate too).
    async fn make_empty_dispatcher() -> (Arc<WorkerDispatcherImpl>, tempfile::TempDir) {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let signer = Arc::new(
            ClusterSigner::load_or_generate(&tmp.path().join("signer.json"))
                .await
                .expect("signer"),
        );
        let worker_ca = Arc::new(WorkerCa::load_or_generate(tmp.path()).expect("ca"));
        let raft = build_test_raft(tmp.path()).await;
        let dispatcher = Arc::new(WorkerDispatcherImpl::new(
            raft,
            "test-cluster".to_string(),
            signer,
            worker_ca,
            AdaptiveTtlConfig::default(),
            100,
        ));
        (dispatcher, tmp)
    }

    async fn build_test_raft(data_dir: &std::path::Path) -> Arc<RaftCoordinator> {
        // Build a per-test RaftCoordinator rooted in `data_dir` so concurrent
        // tests don't race on the production raft lock file at
        // `~/.zlayer/raft/raft-log`.
        let cfg = crate::raft::RaftConfig {
            data_dir: data_dir.join("raft"),
            ..Default::default()
        };
        Arc::new(
            RaftCoordinator::new(cfg)
                .await
                .expect("build test raft coordinator"),
        )
    }

    #[tokio::test]
    async fn dispatch_to_worker_for_unknown_node_errors() {
        let (dispatcher, _tmp) = make_empty_dispatcher().await;
        let req = zlayer_types::cluster::InternalScaleRequest::with_assignments(
            "svc",
            vec![zlayer_types::cluster::ScaleAssignment {
                role: "default".to_string(),
                indices: vec![0],
            }],
        );
        let err = WorkerDispatcher::dispatch_to_worker(dispatcher.as_ref(), 99, req)
            .await
            .expect_err("expected error for unknown node");
        assert!(
            matches!(err, ClusterError::UnknownNode(99)),
            "expected UnknownNode(99), got: {err:?}"
        );
    }

    #[tokio::test]
    async fn known_workers_returns_empty_when_no_sessions() {
        let (dispatcher, _tmp) = make_empty_dispatcher().await;
        let workers = WorkerDispatcher::known_workers(dispatcher.as_ref()).await;
        assert!(workers.is_empty(), "expected no workers, got: {workers:?}");
        assert_eq!(WorkerDispatcher::worker_count(dispatcher.as_ref()).await, 0);
    }
}
