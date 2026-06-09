//! Worker-side gRPC client for `ZLayer`'s worker tier.
//!
//! Lifecycle:
//! 1. On startup: load persisted mTLS identity (cert + key) if present;
//!    otherwise read the bootstrap token, generate a fresh EC P-256 keypair,
//!    build a PKCS#10 CSR, call `Register`, persist the signed cert + key
//!    + ca chain under `<data_dir>/worker/identity/`.
//! 2. Background loops (each spawned as its own `JoinHandle`):
//!    - `WatchAssignments`: server-streaming; receive `AssignmentEvent`s and
//!      forward them via `assignment_tx` to the agent's executor.
//!    - `ReportStatus`: bidi-streaming; tick at `(next_ttl_secs - jitter)`
//!      sending a `StatusReport` snapshot; receive `StatusAck` and update
//!      `next_ttl_secs`.
//!    - `WatchCommands`: server-streaming; receive `CommandEvent`s and
//!      forward via `command_tx`.
//! 3. On disconnect: exponential backoff capped at 60s; on reconnect send a
//!    full snapshot (`StatusReport.full_snapshot = true`).
//!
//! Implements `zlayer_scheduler::cluster::WorkerClient` so a `WorkerTierCluster`
//! in worker mode can route Cluster trait calls through this.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use tokio::sync::{mpsc, RwLock};
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;
use tonic::transport::{Certificate, ClientTlsConfig, Endpoint, Identity};
use tonic::Request;
use zlayer_cluster_rpc::proto;

/// Errors produced by the worker client.
#[derive(Debug, thiserror::Error)]
pub enum WorkerClientError {
    /// No configured control-plane servers.
    #[error("no control-plane servers configured")]
    NoServers,
    /// We have neither a bootstrap token nor a persisted identity, so we
    /// cannot authenticate against the control plane.
    #[error("no bootstrap token and no persisted identity available")]
    NoCredentials,
    /// Failed to build a tonic `Endpoint` from a configured server URL.
    #[error("invalid endpoint {endpoint:?}: {source}")]
    InvalidEndpoint {
        endpoint: String,
        #[source]
        source: tonic::transport::Error,
    },
    /// TLS configuration failed (e.g. malformed PEM).
    #[error("tls config error: {0}")]
    Tls(tonic::transport::Error),
    /// gRPC transport error (connection refused, peer closed, etc.).
    #[error("transport error: {0}")]
    Transport(tonic::transport::Error),
    /// A gRPC call returned a `tonic::Status` error.
    #[error("grpc status: {0}")]
    Status(tonic::Status),
    /// Crypto / CSR generation failed.
    #[error("rcgen error: {0}")]
    Rcgen(rcgen::Error),
    /// Persisted identity I/O failed.
    #[error("identity io error: {0}")]
    Io(std::io::Error),
}

impl From<tonic::Status> for WorkerClientError {
    fn from(s: tonic::Status) -> Self {
        Self::Status(s)
    }
}

impl From<std::io::Error> for WorkerClientError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<rcgen::Error> for WorkerClientError {
    fn from(e: rcgen::Error) -> Self {
        Self::Rcgen(e)
    }
}

/// Worker mTLS identity persisted to disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerIdentity {
    /// PEM-encoded worker certificate (issued by the control plane).
    pub cert_pem: String,
    /// PEM-encoded private key (EC P-256). Persisted with mode 0600.
    pub key_pem: String,
    /// PEM-encoded CA chain back to the cluster root.
    pub ca_chain_pem: String,
}

/// Status snapshot provider for the worker. The agent's `ServiceManager`
/// implements this so the worker client can report current container state
/// + resource usage on each heartbeat tick.
#[async_trait]
pub trait WorkerStatusProvider: Send + Sync + std::fmt::Debug {
    /// Snapshot the worker's current container states for `ReportStatus`.
    async fn snapshot_containers(&self) -> Vec<zlayer_types::cluster::WorkerContainerStatus>;
    /// Snapshot the worker's current resource usage.
    async fn snapshot_resources(&self) -> zlayer_types::cluster::WorkerResourceUsage;
}

/// Worker-side gRPC client. Construct via [`WorkerClientImpl::new`], then
/// spawn the background loops with [`WorkerClientImpl::start`].
pub struct WorkerClientImpl {
    inner: Arc<WorkerClientState>,
}

impl std::fmt::Debug for WorkerClientImpl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkerClientImpl")
            .field("node_id", &self.inner.node_id.load(Ordering::SeqCst))
            .finish_non_exhaustive()
    }
}

struct WorkerClientState {
    /// Endpoints of all known control-plane nodes; tried in order.
    servers: RwLock<Vec<String>>,
    /// Bootstrap token (used until we have a signed cert).
    token: RwLock<Option<String>>,
    /// Persisted identity (loaded once we have one).
    identity: RwLock<Option<WorkerIdentity>>,
    /// Most recently assigned node id.
    node_id: AtomicU64,
    /// Most recent leader address (set after Register / on reconnect).
    leader_addr: RwLock<Option<SocketAddr>>,
    /// Snapshot of cluster peers, populated by `WatchAssignments` / `Register`.
    peers: RwLock<Vec<zlayer_scheduler::cluster::NodeRecord>>,
    /// Worker profile (os, arch, labels, etc.).
    profile: zlayer_types::cluster::WorkerProfile,
    /// Identity persistence directory.
    identity_dir: PathBuf,
    /// Outbound assignment channel — the agent's executor pulls from here.
    assignment_tx: mpsc::UnboundedSender<proto::AssignmentEvent>,
    /// Outbound command channel.
    command_tx: mpsc::UnboundedSender<proto::CommandEvent>,
    /// Current adaptive TTL (seconds). Updated on every `StatusAck`.
    current_ttl_secs: AtomicU32,
    /// Highest revision we've received on `WatchAssignments`. Used to
    /// resume on reconnect.
    last_seen_revision: AtomicU64,
    /// Set when we should send a full snapshot on the next `ReportStatus`
    /// tick (after `Register` or any reconnect).
    full_snapshot_pending: AtomicBool,
    /// Snapshot of containers + resources used in `StatusReport`.
    status_provider: Arc<dyn WorkerStatusProvider>,
}

impl WorkerClientImpl {
    /// Create a fresh worker client. Returns the client plus two receivers:
    /// `assignment_rx` and `command_rx`, which the agent's executor drains
    /// to apply control-plane decisions locally.
    ///
    /// Call [`start`](Self::start) to spawn the background loops.
    #[allow(clippy::needless_pass_by_value)]
    pub fn new(
        servers: Vec<String>,
        token: Option<String>,
        profile: zlayer_types::cluster::WorkerProfile,
        identity_dir: PathBuf,
        status_provider: Arc<dyn WorkerStatusProvider>,
    ) -> (
        Self,
        mpsc::UnboundedReceiver<proto::AssignmentEvent>,
        mpsc::UnboundedReceiver<proto::CommandEvent>,
    ) {
        let (assignment_tx, assignment_rx) = mpsc::unbounded_channel();
        let (command_tx, command_rx) = mpsc::unbounded_channel();

        // Try to load a previously-persisted identity. Best-effort: any error
        // means we'll have to re-register using the bootstrap token.
        let identity = load_identity(&identity_dir).ok().flatten();

        let inner = Arc::new(WorkerClientState {
            servers: RwLock::new(servers),
            token: RwLock::new(token),
            identity: RwLock::new(identity),
            node_id: AtomicU64::new(0),
            leader_addr: RwLock::new(None),
            peers: RwLock::new(Vec::new()),
            profile,
            identity_dir,
            assignment_tx,
            command_tx,
            current_ttl_secs: AtomicU32::new(30),
            last_seen_revision: AtomicU64::new(0),
            full_snapshot_pending: AtomicBool::new(true),
            status_provider,
        });

        (Self { inner }, assignment_rx, command_rx)
    }

    /// Start the background reconnect loop. Spawns one supervisor task that
    /// in turn fans out `WatchAssignments`, `ReportStatus`, and
    /// `WatchCommands` per connected session.
    #[must_use]
    pub fn start(&self) -> tokio::task::JoinSet<()> {
        let mut set = tokio::task::JoinSet::new();
        let state = Arc::clone(&self.inner);
        set.spawn(run_loop(state));
        set
    }
}

#[async_trait]
impl zlayer_scheduler::cluster::WorkerClient for WorkerClientImpl {
    async fn current_leader_addr(&self) -> Option<SocketAddr> {
        *self.inner.leader_addr.read().await
    }

    async fn known_peers(&self) -> Vec<zlayer_scheduler::cluster::NodeRecord> {
        self.inner.peers.read().await.clone()
    }

    fn assigned_node_id(&self) -> u64 {
        self.inner.node_id.load(Ordering::SeqCst)
    }
}

// ----------------------------------------------------------------------------
// Background loops
// ----------------------------------------------------------------------------

/// Top-level supervisor: keep trying to (re)establish a session forever.
async fn run_loop(state: Arc<WorkerClientState>) {
    let mut backoff = Duration::from_secs(1);
    let mut server_idx: usize = 0;
    loop {
        match connect_and_run(&state, &mut server_idx).await {
            Ok(()) => {
                tracing::info!("worker session ended cleanly; reconnecting");
                backoff = Duration::from_secs(1);
            }
            Err(WorkerClientError::NoServers) => {
                // Fatal-ish: we can't do anything until somebody hands us a
                // server list. Sleep long-ish and check again.
                tracing::warn!("no control-plane servers configured; sleeping 30s");
                tokio::time::sleep(Duration::from_secs(30)).await;
            }
            Err(WorkerClientError::NoCredentials) => {
                tracing::error!(
                    "no bootstrap token and no persisted identity; cannot register; sleeping 30s"
                );
                tokio::time::sleep(Duration::from_secs(30)).await;
            }
            Err(e) => {
                tracing::warn!(error = %e, "worker session ended; reconnecting after backoff");
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(60));
            }
        }
        // On reconnect, the next ReportStatus must be a full snapshot.
        state.full_snapshot_pending.store(true, Ordering::SeqCst);
    }
}

/// One session: connect, optionally Register, then fan out the three streams.
/// Returns when any of them fails or the channel breaks.
async fn connect_and_run(
    state: &Arc<WorkerClientState>,
    server_idx: &mut usize,
) -> Result<(), WorkerClientError> {
    let endpoint_url = {
        let servers = state.servers.read().await;
        if servers.is_empty() {
            return Err(WorkerClientError::NoServers);
        }
        let idx = *server_idx % servers.len();
        *server_idx = server_idx.wrapping_add(1);
        servers[idx].clone()
    };

    // 1. Build the channel (mTLS if we have an identity, otherwise plain).
    let channel = build_channel(state, &endpoint_url).await?;

    // 2. Update leader_addr best-effort from the endpoint URL.
    if let Some(addr) = parse_addr_from_url(&endpoint_url) {
        *state.leader_addr.write().await = Some(addr);
    }

    let mut client =
        proto::cluster_control_plane_client::ClusterControlPlaneClient::new(channel.clone());

    // 3. If we don't have a signed identity, register first.
    if state.identity.read().await.is_none() {
        register(state, &mut client).await?;
        // Rebuild the channel using the new identity so subsequent streams
        // run over mTLS.
        let channel = build_channel(state, &endpoint_url).await?;
        client = proto::cluster_control_plane_client::ClusterControlPlaneClient::new(channel);
    }

    let node_id = state.node_id.load(Ordering::SeqCst);
    if node_id == 0 {
        return Err(WorkerClientError::Status(
            tonic::Status::failed_precondition("register did not assign node_id"),
        ));
    }

    // 4. Fan out the three streams. First failure cancels the session.
    let assignments_state = Arc::clone(state);
    let mut assignments_client = client.clone();
    let assignments_task = tokio::spawn(async move {
        run_watch_assignments(&assignments_state, &mut assignments_client, node_id).await
    });

    let status_state = Arc::clone(state);
    let mut status_client = client.clone();
    let status_task =
        tokio::spawn(
            async move { run_report_status(&status_state, &mut status_client, node_id).await },
        );

    let commands_state = Arc::clone(state);
    let mut commands_client = client;
    let commands_task = tokio::spawn(async move {
        run_watch_commands(&commands_state, &mut commands_client, node_id).await
    });

    // Wait for any of them to terminate.
    let result = tokio::select! {
        r = assignments_task => unwrap_join(r),
        r = status_task => unwrap_join(r),
        r = commands_task => unwrap_join(r),
    };
    result
}

fn unwrap_join(
    r: Result<Result<(), WorkerClientError>, tokio::task::JoinError>,
) -> Result<(), WorkerClientError> {
    match r {
        Ok(inner) => inner,
        Err(e) => Err(WorkerClientError::Status(tonic::Status::internal(format!(
            "task join error: {e}"
        )))),
    }
}

// ----------------------------------------------------------------------------
// Stream loops
// ----------------------------------------------------------------------------

async fn run_watch_assignments(
    state: &Arc<WorkerClientState>,
    client: &mut proto::cluster_control_plane_client::ClusterControlPlaneClient<
        tonic::transport::Channel,
    >,
    node_id: u64,
) -> Result<(), WorkerClientError> {
    let req = proto::WatchAssignmentsRequest {
        node_id,
        last_seen_revision: state.last_seen_revision.load(Ordering::SeqCst),
    };
    let resp = client.watch_assignments(Request::new(req)).await?;
    let mut stream = resp.into_inner();
    while let Some(event) = stream.next().await {
        match event {
            Ok(ev) => {
                if ev.revision > state.last_seen_revision.load(Ordering::SeqCst) {
                    state
                        .last_seen_revision
                        .store(ev.revision, Ordering::SeqCst);
                }
                if state.assignment_tx.send(ev).is_err() {
                    tracing::warn!("assignment receiver dropped; exiting watch loop");
                    return Ok(());
                }
            }
            Err(status) => {
                return Err(WorkerClientError::Status(status));
            }
        }
    }
    Ok(())
}

async fn run_watch_commands(
    state: &Arc<WorkerClientState>,
    client: &mut proto::cluster_control_plane_client::ClusterControlPlaneClient<
        tonic::transport::Channel,
    >,
    node_id: u64,
) -> Result<(), WorkerClientError> {
    let req = proto::WatchCommandsRequest { node_id };
    let resp = client.watch_commands(Request::new(req)).await?;
    let mut stream = resp.into_inner();
    while let Some(event) = stream.next().await {
        match event {
            Ok(ev) => {
                if state.command_tx.send(ev).is_err() {
                    tracing::warn!("command receiver dropped; exiting watch loop");
                    return Ok(());
                }
            }
            Err(status) => {
                return Err(WorkerClientError::Status(status));
            }
        }
    }
    Ok(())
}

async fn run_report_status(
    state: &Arc<WorkerClientState>,
    client: &mut proto::cluster_control_plane_client::ClusterControlPlaneClient<
        tonic::transport::Channel,
    >,
    node_id: u64,
) -> Result<(), WorkerClientError> {
    // Bidirectional: spawn a producer task that pushes StatusReports on
    // an adaptive tick into an mpsc; the receiver-stream wrapper feeds them
    // into tonic. The bidi response stream is drained inline.
    let (tx, rx) = mpsc::channel::<proto::StatusReport>(8);
    let outbound = ReceiverStream::new(rx);

    // Spawn the producer.
    let prod_state = Arc::clone(state);
    let producer = tokio::spawn(async move {
        produce_status_reports(prod_state, tx, node_id).await;
    });

    let resp = client.report_status(Request::new(outbound)).await?;
    let mut acks = resp.into_inner();
    while let Some(ack) = acks.next().await {
        match ack {
            Ok(a) => {
                if a.next_ttl_secs > 0 {
                    state
                        .current_ttl_secs
                        .store(a.next_ttl_secs, Ordering::SeqCst);
                }
            }
            Err(status) => {
                producer.abort();
                return Err(WorkerClientError::Status(status));
            }
        }
    }
    producer.abort();
    Ok(())
}

async fn produce_status_reports(
    state: Arc<WorkerClientState>,
    tx: mpsc::Sender<proto::StatusReport>,
    node_id: u64,
) {
    loop {
        // Drift slightly: tick at (ttl - jitter) where jitter is up to 25% of ttl
        // (min 1s, max 5s) so we stay comfortably inside the grace window.
        let ttl = state.current_ttl_secs.load(Ordering::SeqCst).max(1);
        let jitter = (ttl / 4).clamp(1, 5);
        let interval = u64::from(ttl.saturating_sub(jitter)).max(1);
        tokio::time::sleep(Duration::from_secs(interval)).await;

        // Build a snapshot.
        let containers = state.status_provider.snapshot_containers().await;
        let resources = state.status_provider.snapshot_resources().await;
        let full = state.full_snapshot_pending.swap(false, Ordering::SeqCst);

        let report = proto::StatusReport {
            node_id,
            ts: Some(now_proto_timestamp()),
            containers: containers.into_iter().map(Into::into).collect(),
            resources: Some(resources.into()),
            full_snapshot: full,
        };

        if tx.send(report).await.is_err() {
            // Tonic closed the stream; producer task should exit.
            return;
        }
    }
}

fn now_proto_timestamp() -> prost_types::Timestamp {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => prost_types::Timestamp {
            seconds: i64::try_from(d.as_secs()).unwrap_or(i64::MAX),
            nanos: i32::try_from(d.subsec_nanos()).unwrap_or(0),
        },
        Err(_) => prost_types::Timestamp {
            seconds: 0,
            nanos: 0,
        },
    }
}

// ----------------------------------------------------------------------------
// Register
// ----------------------------------------------------------------------------

async fn register(
    state: &Arc<WorkerClientState>,
    client: &mut proto::cluster_control_plane_client::ClusterControlPlaneClient<
        tonic::transport::Channel,
    >,
) -> Result<(), WorkerClientError> {
    // We need a bootstrap token for the very first registration.
    let token = state
        .token
        .read()
        .await
        .clone()
        .ok_or(WorkerClientError::NoCredentials)?;

    // Generate a fresh EC P-256 keypair and matching CSR. The key never
    // leaves this process unencrypted on the wire — the CSR carries only
    // the public half.
    let key_pair = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)?;
    let key_pem = key_pair.serialize_pem();

    let params = rcgen::CertificateParams::default();
    let csr = params.serialize_request(&key_pair)?;
    let csr_der: Vec<u8> = csr.der().as_ref().to_vec();

    let req = proto::RegisterRequest {
        bootstrap_token: token,
        desired_node_id: 0,
        profile: Some(state.profile.clone().into()),
        csr_der,
    };

    let resp = client.register(Request::new(req)).await?.into_inner();

    if resp.node_id == 0 {
        return Err(WorkerClientError::Status(
            tonic::Status::failed_precondition("control plane returned node_id=0"),
        ));
    }
    state.node_id.store(resp.node_id, Ordering::SeqCst);
    if resp.heartbeat_ttl_secs > 0 {
        state
            .current_ttl_secs
            .store(resp.heartbeat_ttl_secs, Ordering::SeqCst);
    }

    // Persist identity. The control plane returns DER-encoded certs; turn
    // them into PEM blocks for tonic's `Identity::from_pem`.
    let cert_pem = der_to_pem(&resp.signed_cert_der, "CERTIFICATE");
    let ca_chain_pem = resp
        .ca_chain_der
        .iter()
        .map(|d| der_to_pem(d, "CERTIFICATE"))
        .collect::<String>();
    let identity = WorkerIdentity {
        cert_pem,
        key_pem,
        ca_chain_pem,
    };

    persist_identity(&state.identity_dir, &identity)?;
    *state.identity.write().await = Some(identity);

    tracing::info!(
        node_id = resp.node_id,
        ttl_secs = resp.heartbeat_ttl_secs,
        "worker registered with control plane"
    );

    Ok(())
}

// ----------------------------------------------------------------------------
// Channel construction
// ----------------------------------------------------------------------------

async fn build_channel(
    state: &Arc<WorkerClientState>,
    endpoint_url: &str,
) -> Result<tonic::transport::Channel, WorkerClientError> {
    let endpoint = Endpoint::from_shared(endpoint_url.to_string()).map_err(|e| {
        WorkerClientError::InvalidEndpoint {
            endpoint: endpoint_url.to_string(),
            source: e,
        }
    })?;

    let endpoint = if let Some(identity) = state.identity.read().await.clone() {
        let tls = ClientTlsConfig::new()
            .ca_certificate(Certificate::from_pem(identity.ca_chain_pem.as_bytes()))
            .identity(Identity::from_pem(
                identity.cert_pem.as_bytes(),
                identity.key_pem.as_bytes(),
            ));
        endpoint.tls_config(tls).map_err(WorkerClientError::Tls)?
    } else {
        endpoint
    };

    endpoint
        .connect()
        .await
        .map_err(WorkerClientError::Transport)
}

fn parse_addr_from_url(url: &str) -> Option<SocketAddr> {
    // Strip "http://" / "https://" prefix and parse the host:port portion.
    let trimmed = url
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/');
    trimmed.parse().ok()
}

// ----------------------------------------------------------------------------
// Identity persistence
// ----------------------------------------------------------------------------

fn identity_paths(dir: &Path) -> (PathBuf, PathBuf, PathBuf) {
    (
        dir.join("cert.pem"),
        dir.join("key.pem"),
        dir.join("ca.pem"),
    )
}

fn load_identity(dir: &Path) -> Result<Option<WorkerIdentity>, WorkerClientError> {
    let (cert_path, key_path, ca_path) = identity_paths(dir);
    if !cert_path.exists() || !key_path.exists() || !ca_path.exists() {
        return Ok(None);
    }
    let cert_pem = std::fs::read_to_string(&cert_path)?;
    let key_pem = std::fs::read_to_string(&key_path)?;
    let ca_chain_pem = std::fs::read_to_string(&ca_path)?;
    Ok(Some(WorkerIdentity {
        cert_pem,
        key_pem,
        ca_chain_pem,
    }))
}

fn persist_identity(dir: &Path, identity: &WorkerIdentity) -> Result<(), WorkerClientError> {
    std::fs::create_dir_all(dir)?;
    let (cert_path, key_path, ca_path) = identity_paths(dir);
    write_mode_0600(&cert_path, identity.cert_pem.as_bytes())?;
    write_mode_0600(&key_path, identity.key_pem.as_bytes())?;
    write_mode_0600(&ca_path, identity.ca_chain_pem.as_bytes())?;
    Ok(())
}

fn write_mode_0600(path: &Path, bytes: &[u8]) -> Result<(), WorkerClientError> {
    std::fs::write(path, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path)?.permissions();
        perms.set_mode(0o600);
        std::fs::set_permissions(path, perms)?;
    }
    Ok(())
}

fn der_to_pem(der: &[u8], label: &str) -> String {
    use std::fmt::Write;
    let b64 = base64_encode(der);
    let mut out = String::with_capacity(b64.len() + 64);
    let _ = writeln!(out, "-----BEGIN {label}-----");
    for chunk in b64.as_bytes().chunks(64) {
        out.push_str(std::str::from_utf8(chunk).expect("base64 is ascii"));
        out.push('\n');
    }
    let _ = writeln!(out, "-----END {label}-----");
    out
}

/// Minimal RFC 4648 base64 encoder. Avoids pulling in another crate just
/// for this single use site.
fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    let mut i = 0;
    while i + 3 <= input.len() {
        let b0 = input[i];
        let b1 = input[i + 1];
        let b2 = input[i + 2];
        out.push(TABLE[(b0 >> 2) as usize] as char);
        out.push(TABLE[(((b0 & 0x03) << 4) | (b1 >> 4)) as usize] as char);
        out.push(TABLE[(((b1 & 0x0f) << 2) | (b2 >> 6)) as usize] as char);
        out.push(TABLE[(b2 & 0x3f) as usize] as char);
        i += 3;
    }
    match input.len() - i {
        0 => {}
        1 => {
            let b0 = input[i];
            out.push(TABLE[(b0 >> 2) as usize] as char);
            out.push(TABLE[((b0 & 0x03) << 4) as usize] as char);
            out.push('=');
            out.push('=');
        }
        2 => {
            let b0 = input[i];
            let b1 = input[i + 1];
            out.push(TABLE[(b0 >> 2) as usize] as char);
            out.push(TABLE[(((b0 & 0x03) << 4) | (b1 >> 4)) as usize] as char);
            out.push(TABLE[((b1 & 0x0f) << 2) as usize] as char);
            out.push('=');
        }
        _ => unreachable!(),
    }
    out
}

// ----------------------------------------------------------------------------
// Tests
// ----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use zlayer_scheduler::cluster::WorkerClient as _;

    #[derive(Debug)]
    struct DummyStatusProvider;

    #[async_trait]
    impl WorkerStatusProvider for DummyStatusProvider {
        async fn snapshot_containers(&self) -> Vec<zlayer_types::cluster::WorkerContainerStatus> {
            Vec::new()
        }
        async fn snapshot_resources(&self) -> zlayer_types::cluster::WorkerResourceUsage {
            zlayer_types::cluster::WorkerResourceUsage {
                cpu_used: 0.0,
                memory_used_bytes: 0,
                gpu_used: 0,
            }
        }
    }

    fn dummy_profile() -> zlayer_types::cluster::WorkerProfile {
        zlayer_types::cluster::WorkerProfile {
            api_addr: "127.0.0.1:3669".parse().unwrap(),
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            labels: HashMap::new(),
            cpu_total: 4,
            memory_total_bytes: 8_000_000_000,
        }
    }

    #[tokio::test]
    async fn worker_client_starts_empty_with_no_servers() {
        let dir = tempfile::tempdir().unwrap();
        let (client, _assignments, _commands) = WorkerClientImpl::new(
            Vec::new(),
            None,
            dummy_profile(),
            dir.path().to_path_buf(),
            Arc::new(DummyStatusProvider),
        );
        assert_eq!(client.assigned_node_id(), 0);
        assert!(client.known_peers().await.is_empty());
        assert!(client.current_leader_addr().await.is_none());
    }

    #[test]
    fn worker_identity_persists_to_disk() {
        let dir = tempfile::tempdir().unwrap();
        let identity = WorkerIdentity {
            cert_pem: "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n".to_string(),
            key_pem: "-----BEGIN PRIVATE KEY-----\nBBBB\n-----END PRIVATE KEY-----\n".to_string(),
            ca_chain_pem: "-----BEGIN CERTIFICATE-----\nCCCC\n-----END CERTIFICATE-----\n"
                .to_string(),
        };
        persist_identity(dir.path(), &identity).expect("persist");
        let loaded = load_identity(dir.path()).expect("load").expect("present");
        assert_eq!(loaded, identity);

        // Verify mode 0600 on Unix.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let (cert, key, ca) = identity_paths(dir.path());
            for p in [cert, key, ca] {
                let meta = std::fs::metadata(&p).unwrap();
                assert_eq!(meta.permissions().mode() & 0o777, 0o600, "{p:?}");
            }
        }
    }

    #[test]
    fn base64_roundtrip_basic() {
        // Spot-check standard test vectors from RFC 4648.
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn der_to_pem_wraps_with_label() {
        let pem = der_to_pem(&[0x30, 0x82, 0x01, 0x00], "CERTIFICATE");
        assert!(pem.starts_with("-----BEGIN CERTIFICATE-----\n"));
        assert!(pem.trim_end().ends_with("-----END CERTIFICATE-----"));
    }

    #[test]
    fn parse_addr_from_url_handles_http_prefix() {
        assert_eq!(
            parse_addr_from_url("http://127.0.0.1:3669"),
            Some("127.0.0.1:3669".parse().unwrap())
        );
        assert_eq!(
            parse_addr_from_url("https://10.0.0.1:443/"),
            Some("10.0.0.1:443".parse().unwrap())
        );
        assert_eq!(parse_addr_from_url("not-a-url"), None);
    }
}
