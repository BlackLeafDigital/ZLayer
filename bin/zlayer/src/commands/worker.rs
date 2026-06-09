//! `zlayer worker` subcommand — runs this host as a worker-tier worker
//! node. Spins up a lightweight agent + the worker gRPC client; no API
//! server, no raft, no scheduler.

use anyhow::{Context, Result};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tracing::info;

use zlayer_agent::worker_client::{WorkerClientImpl, WorkerStatusProvider};
use zlayer_types::cluster::{WorkerContainerStatus, WorkerProfile, WorkerResourceUsage};

/// Read a bootstrap token from a file (single line, trimmed).
fn read_token_file(path: &str) -> Result<String> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("reading worker token file {path}"))?;
    Ok(raw.trim().to_string())
}

/// Build a `WorkerProfile` from local system facts (uname, /proc/cpuinfo,
/// /proc/meminfo). On Windows/macOS we fall back to `num_cpus` and conservative
/// defaults — the real production builds detect more.
fn detect_local_profile(
    labels: std::collections::HashMap<String, String>,
    api_addr: SocketAddr,
) -> WorkerProfile {
    let os = std::env::consts::OS.to_string();
    let arch = std::env::consts::ARCH.to_string();
    let cpu_total = u32::try_from(num_cpus::get()).unwrap_or(1);
    let memory_total_bytes = sys_memory_total().unwrap_or(0);
    WorkerProfile {
        api_addr,
        os,
        arch,
        labels,
        cpu_total,
        memory_total_bytes,
    }
}

#[cfg(target_os = "linux")]
fn sys_memory_total() -> Option<u64> {
    let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
    for line in meminfo.lines() {
        if let Some(rest) = line.strip_prefix("MemTotal:") {
            let kb: u64 = rest.split_whitespace().next()?.parse().ok()?;
            return Some(kb * 1024);
        }
    }
    None
}

#[cfg(not(target_os = "linux"))]
fn sys_memory_total() -> Option<u64> {
    None
}

/// Status provider that reports an empty container list and zero resource
/// usage. Real production wires this to the agent's container supervisor;
/// for the worker MVP we keep it minimal so the worker registers and stays
/// alive.
#[derive(Debug)]
struct EmptyStatusProvider;

#[async_trait::async_trait]
impl WorkerStatusProvider for EmptyStatusProvider {
    async fn snapshot_containers(&self) -> Vec<WorkerContainerStatus> {
        vec![]
    }
    async fn snapshot_resources(&self) -> WorkerResourceUsage {
        WorkerResourceUsage {
            cpu_used: 0.0,
            memory_used_bytes: 0,
            gpu_used: 0,
        }
    }
}

/// Parse `--labels "k=v,k=v"` into a `HashMap`.
fn parse_labels(raw: &str) -> Result<std::collections::HashMap<String, String>> {
    let mut out = std::collections::HashMap::new();
    if raw.is_empty() {
        return Ok(out);
    }
    for entry in raw.split(',') {
        let (k, v) = entry
            .split_once('=')
            .with_context(|| format!("expected k=v in --labels, got: {entry}"))?;
        out.insert(k.trim().to_string(), v.trim().to_string());
    }
    Ok(out)
}

/// Run the `zlayer worker` subcommand. Blocks until ctrl-c.
pub async fn run(
    servers: Vec<String>,
    token: Option<String>,
    token_file: Option<String>,
    labels: String,
    identity_dir: PathBuf,
    api_addr: SocketAddr,
) -> Result<()> {
    let token = match (token, token_file) {
        (Some(t), _) => Some(t),
        (None, Some(path)) => Some(read_token_file(&path)?),
        (None, None) => None,
    };

    if servers.is_empty() {
        anyhow::bail!("at least one --server must be specified");
    }
    if !identity_dir.exists() {
        std::fs::create_dir_all(&identity_dir)
            .with_context(|| format!("create identity dir {}", identity_dir.display()))?;
    }

    let labels_map = parse_labels(&labels)?;
    let profile = detect_local_profile(labels_map, api_addr);

    let status: Arc<dyn WorkerStatusProvider> = Arc::new(EmptyStatusProvider);
    let (client, mut assignments_rx, mut commands_rx) =
        WorkerClientImpl::new(servers, token, profile, identity_dir, status);

    let _tasks = client.start();
    info!("worker started; awaiting assignments");

    // Drain the channels until ctrl-c. Real production wires assignments_rx
    // into the agent's container supervisor; commands_rx into a per-command
    // dispatcher (drain/evict/restart). For the MVP we log + ignore.
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            info!("worker shutting down on ctrl-c");
        }
        () = async {
            loop {
                tokio::select! {
                    Some(ev) = assignments_rx.recv() => {
                        info!(event = ?ev, "received assignment");
                    }
                    Some(cmd) = commands_rx.recv() => {
                        info!(command = ?cmd, "received command");
                    }
                    else => {
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                }
            }
        } => {}
    }

    Ok(())
}
