use anyhow::{bail, Context, Result};
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tracing::{info, warn};
use zlayer_types::api::cluster::{ClusterJoinClaims, SignedClusterJoinToken, SIGNED_TOKEN_V_WAVE3};

#[cfg(not(unix))]
use crate::ui::consent::ConsentMode;

// =============================================================================
// Node management types
// =============================================================================

// NodeConfig and its helpers live in `crate::daemon` so they can be shared
// with the daemon init path on every platform (Windows included). Re-export
// them here so the rest of this module keeps using the short name.
pub(crate) use crate::daemon::{
    current_timestamp, detect_local_ip, generate_node_id, load_node_config, save_node_config,
    NodeConfig,
};

/// Join token payload
///
/// Wave 6 (v0.13.0): the `auth_secret` field is no longer part of the type.
/// Plaintext tokens are no longer accepted server-side; this struct survives
/// only as a parser for the modern (Ed25519 + HS256) token formats' shared
/// metadata (endpoint/CIDR/pubkey). Extra fields encountered during decode
/// (e.g. `auth_secret` in a stale v0.11.x-minted plaintext token) are
/// tolerated by `serde_json` and silently dropped — the token still won't
/// authenticate because the server-side acceptance gate is gone.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ClusterJoinToken {
    /// Leader's API endpoint
    api_endpoint: String,
    /// Leader's Raft endpoint
    raft_endpoint: String,
    /// Leader's overlay public key
    leader_wg_pubkey: String,
    /// Overlay network CIDR
    overlay_cidr: String,
    /// Token creation timestamp
    created_at: String,
}

/// Join request sent to the leader
#[derive(Debug, Clone, Serialize, Deserialize)]
struct NodeJoinRequest {
    /// Join token for authentication
    token: String,
    /// Joining node's advertise address
    advertise_addr: String,
    /// Joining node's overlay port
    overlay_port: u16,
    /// Joining node's Raft port
    raft_port: u16,
    /// Joining node's overlay public key
    wg_public_key: String,
    /// Node mode (full, replicate)
    mode: String,
    /// Services to replicate (if mode is replicate)
    services: Option<Vec<String>>,
    /// Operating system of the joining agent (detected via `std::env::consts::OS`)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    os: Option<zlayer_spec::OsKind>,
    /// CPU architecture of the joining agent (detected via `std::env::consts::ARCH`)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    arch: Option<zlayer_spec::ArchKind>,
    /// Joiner's 32-byte X25519 pubkey for sealed-box DEK wrapping (Phase 1+).
    /// `None` only when the local keypair could not be created (extremely
    /// unusual — the leader will then treat this node as not eligible to
    /// host replicated-secret ciphertext).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    secrets_pubkey: Option<[u8; 32]>,
}

/// Join response from the leader
#[derive(Debug, Clone, Serialize, Deserialize)]
struct NodeJoinResponse {
    /// Assigned node ID
    node_id: String,
    /// Assigned Raft node ID
    raft_node_id: u64,
    /// Assigned overlay IP
    overlay_ip: String,
    /// Per-node slice CIDR assigned by the leader (e.g. "10.200.42.0/28").
    /// Empty string if the leader is not slice-aware yet.
    #[serde(default)]
    slice_cidr: String,
    /// Existing peers in the cluster
    peers: Vec<PeerNode>,
    /// Node JWT minted by the leader for this joiner — `roles: ["node"]`
    /// with the assigned `node_id` baked in. Used to authenticate inter-node
    /// calls separately from any user identity. `None` when the leader is a
    /// legacy build without secrets capability.
    #[serde(default)]
    node_jwt: Option<String>,
    /// Sealed-box-wrapped copy of the cluster DEK addressed to the joiner's
    /// `secrets_pubkey`. The daemon unwraps this on startup with the on-disk
    /// node X25519 private key. `None` on legacy responses or when the
    /// joiner did not provide a `secrets_pubkey`.
    #[serde(default)]
    wrapped_dek: Option<Vec<u8>>,
    /// Cluster DEK generation that `wrapped_dek` was sealed under. Lets the
    /// daemon detect rotation drift. `None` when `wrapped_dek` is `None`.
    #[serde(default)]
    dek_generation: Option<u64>,
}

/// Peer node information
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PeerNode {
    node_id: String,
    raft_node_id: u64,
    advertise_addr: String,
    overlay_port: u16,
    raft_port: u16,
    wg_public_key: String,
    overlay_ip: String,
}

/// Node status for listing
#[derive(Debug, Clone, Serialize, Deserialize)]
struct NodeStatus {
    id: String,
    address: String,
    status: String,
    mode: String,
    services: Vec<String>,
    is_leader: bool,
    /// Server-computed Raft role for this node: `"leader"`, `"voter"`, or
    /// `"learner"`. Defaulted so old daemons that don't yet return this
    /// field still deserialize cleanly (empty string then renders as
    /// "Unknown" in the table).
    #[serde(default)]
    role: String,
}

// =============================================================================
// Helper functions
// =============================================================================

/// Generate a secure random token.
///
/// Wave 6 (v0.13.0): runtime callers no longer mint random join secrets
/// from the CLI — the daemon owns `{data_dir}/join_secret` (HS256) and
/// the Ed25519 signer (`cluster_signing.key`). Kept for the unit tests
/// in `#[cfg(test)] mod tests` that round-trip the parser.
#[cfg(test)]
fn generate_secure_token() -> String {
    use rand::Rng;
    let mut rng = rand::rng();
    let bytes: [u8; 32] = rng.random();
    base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, bytes)
}

/// Load node config, or auto-initialize as single-node leader if none exists.
pub(crate) async fn load_or_init_node_config(data_dir: &Path) -> Result<NodeConfig> {
    if let Ok(cfg) = load_node_config(data_dir).await {
        Ok(cfg)
    } else {
        let advertise_addr = detect_local_ip();
        println!(
            "Node not yet initialized. Auto-initializing with advertise address {advertise_addr}..."
        );
        handle_node_init(
            advertise_addr,
            3669,
            9000,
            zlayer_core::DEFAULT_WG_PORT,
            data_dir.to_path_buf(),
            "10.200.0.0/16".to_string(),
            // Auto-init from `load_or_init_node_config` has no CLI args to read
            // `--install-wsl` from. `ConsentMode::Ask` honours `ZLAYER_INSTALL_WSL`
            // if set and otherwise falls back to the interactive prompt, matching
            // the top-level CLI default. The parameter is `#[cfg(not(unix))]` only
            // — Unix builds don't see it.
            #[cfg(not(unix))]
            ConsentMode::Ask,
        )
        .await?;
        load_node_config(data_dir)
            .await
            .context("Failed to load node config after auto-initialization")
    }
}

/// Generate a join token for the cluster.
///
/// Wave 6 (v0.13.0): used only by the round-trip test harness. The runtime
/// CLI no longer mints plaintext tokens — `handle_node_generate_join_token`
/// emits Ed25519 + HS256 only. This helper survives so the existing parser
/// tests in `#[cfg(test)] mod tests` can keep verifying that
/// `parse_cluster_join_token` still round-trips a self-encoded payload.
#[cfg(test)]
fn generate_join_token_data(
    advertise_addr: &str,
    api_port: u16,
    raft_port: u16,
    wg_public_key: &str,
    overlay_cidr: &str,
) -> Result<String> {
    let token_data = ClusterJoinToken {
        api_endpoint: format!("{advertise_addr}:{api_port}"),
        raft_endpoint: format!("{advertise_addr}:{raft_port}"),
        leader_wg_pubkey: wg_public_key.to_string(),
        overlay_cidr: overlay_cidr.to_string(),
        created_at: current_timestamp(),
    };

    let json = serde_json::to_string(&token_data).context("Failed to serialize join token")?;

    Ok(base64::Engine::encode(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD,
        json.as_bytes(),
    ))
}

/// Parse a join token
fn parse_cluster_join_token(token: &str) -> Result<ClusterJoinToken> {
    let decoded = base64::Engine::decode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, token)
        .or_else(|_| base64::Engine::decode(&base64::engine::general_purpose::STANDARD, token))
        .context("Invalid join token: not valid base64")?;

    let token_data: ClusterJoinToken =
        serde_json::from_slice(&decoded).context("Invalid join token: not valid JSON")?;

    Ok(token_data)
}

/// Mint a Wave-3 signed cluster join token.
///
/// Produces the base64 url-safe-no-pad encoding of a `SignedClusterJoinToken`
/// envelope:
///   1. Canonically serialize `claims` to JSON bytes (declaration order).
///   2. Sign with the cluster's `ClusterSigner`.
///   3. Wrap claims + sig + kid + v=1 in an envelope.
///   4. JSON-serialize the envelope and base64url-no-pad encode it.
///
/// The output is safe to copy-paste into chat/email/wiki: a single ASCII
/// string, no line wraps, ~340 chars.
///
/// Wired into `handle_node_generate_join_token` (Wave 3.3); the server-side
/// validator (Wave 3.4) and join-CLI (Wave 4) hold the matching parse/verify
/// helpers below.
pub fn mint_signed_cluster_join_token(
    claims: &ClusterJoinClaims,
    signer: &zlayer_secrets::ClusterSigner,
    ca_context: Option<(&zlayer_secrets::ClusterCa, &str, std::time::Duration)>,
) -> Result<String> {
    let claims_bytes =
        serde_json::to_vec(claims).context("serializing cluster join claims for signing")?;
    let sig_bytes = signer.sign(&claims_bytes);

    // v=2 emit when caller supplies a CA + cluster_domain + cert
    // validity. Otherwise emit v=1 (wire-compatible: `ca_chain: None`
    // is skipped by serde).
    let (v, ca_chain) = if let Some((ca, cluster_domain, validity)) = ca_context {
        let cert = ca
            .issue_ca_cert(
                signer.key_id(),
                signer.public_key_b64(),
                cluster_domain.to_string(),
                validity,
            )
            .context("issuing CaCert for v=2 token mint")?;
        (zlayer_types::api::cluster::SIGNED_TOKEN_V_WAVE9, Some(cert))
    } else {
        (SIGNED_TOKEN_V_WAVE3, None)
    };

    let envelope = SignedClusterJoinToken {
        v,
        kid: signer.key_id(),
        claims: claims.clone(),
        sig: URL_SAFE_NO_PAD.encode(sig_bytes),
        ca_chain,
    };
    let envelope_bytes =
        serde_json::to_vec(&envelope).context("serializing signed cluster join token envelope")?;
    Ok(URL_SAFE_NO_PAD.encode(envelope_bytes))
}

/// Parse a Wave-3 signed cluster join token from its base64 envelope form.
///
/// Does NOT verify the signature — that's `verify_signed_cluster_join_token`.
/// Returns `Err` with an actionable message if the input isn't a well-formed
/// envelope (wrong base64, wrong JSON shape, unsupported version).
///
/// `#[allow(dead_code)]` because Wave 3.2 only ships the helper; the
/// server-side validator (Agent 3.4) and CLI join path (Wave 4) call it.
#[allow(dead_code)]
pub fn parse_signed_cluster_join_token(s: &str) -> Result<SignedClusterJoinToken> {
    let s = s.trim();
    let bytes = URL_SAFE_NO_PAD
        .decode(s)
        .or_else(|_| STANDARD.decode(s))
        .with_context(|| {
            format!(
                "decoding signed cluster join token base64 (len={})",
                s.len()
            )
        })?;
    let envelope: SignedClusterJoinToken = serde_json::from_slice(&bytes)
        .context("parsing signed cluster join token envelope JSON")?;
    if envelope.v != SIGNED_TOKEN_V_WAVE3 {
        bail!(
            "unsupported signed cluster join token version: got v={}, expected v={}",
            envelope.v,
            SIGNED_TOKEN_V_WAVE3,
        );
    }
    Ok(envelope)
}

/// Verify a parsed signed cluster join token against a verifying key.
///
/// Checks:
///   1. The `kid` in the envelope matches the verifying key's id.
///   2. The signature decodes from base64.
///   3. The signature validates against `serde_json::to_vec(&claims)` bytes.
///   4. The `exp` timestamp is in the future (RFC3339 parse).
///
/// Replay protection (`(kid, iat, iss)` tuple deduplication) is the caller's
/// responsibility — this helper is a pure cryptographic check.
///
/// `#[allow(dead_code)]` because Wave 3.2 only ships the helper; Wave 3.4
/// will call it from the server-side validate path.
#[allow(dead_code)]
pub fn verify_signed_cluster_join_token<'a>(
    token: &'a SignedClusterJoinToken,
    verifying_key: &ed25519_dalek::VerifyingKey,
    expected_kid: &str,
) -> Result<&'a ClusterJoinClaims> {
    if token.kid != expected_kid {
        bail!(
            "kid mismatch: token says {}, expected {}",
            token.kid,
            expected_kid
        );
    }
    let sig_bytes_vec = URL_SAFE_NO_PAD
        .decode(&token.sig)
        .or_else(|_| STANDARD.decode(&token.sig))
        .context("decoding signed cluster join token signature base64")?;
    let sig_array: [u8; 64] = sig_bytes_vec.as_slice().try_into().map_err(|_| {
        anyhow::anyhow!(
            "signature has wrong length: expected 64, got {}",
            sig_bytes_vec.len()
        )
    })?;
    let sig = ed25519_dalek::Signature::from_bytes(&sig_array);
    let claims_bytes = serde_json::to_vec(&token.claims)
        .context("re-serializing claims for signature verification")?;
    verifying_key
        .verify_strict(&claims_bytes, &sig)
        .context("Ed25519 signature verification failed")?;
    let now = chrono::Utc::now();
    let exp = chrono::DateTime::parse_from_rfc3339(&token.claims.exp)
        .with_context(|| format!("parsing token exp timestamp: {}", token.claims.exp))?
        .with_timezone(&chrono::Utc);
    if now >= exp {
        bail!(
            "signed cluster join token expired at {} (now {})",
            token.claims.exp,
            now.to_rfc3339()
        );
    }
    Ok(&token.claims)
}

/// Write `bytes` to `path`, creating the file with restrictive permissions
/// (Unix mode 0600 — owner read/write only) **before** any data is written.
///
/// On non-Unix targets the file is written with the default platform
/// permissions. Windows ACL hardening is handled at the data-directory
/// level by the installer (`%ProgramData%\ZLayer` inherits SYSTEM +
/// Administrators write, Users read), so per-file ACL plumbing is not
/// duplicated here.
///
/// This helper is the canonical way to persist secrets-bearing files
/// (node JWTs, wrapped DEKs, etc.) from the CLI side. It mirrors the
/// open-with-mode-0600 pattern used by
/// [`zlayer_secrets::load_or_generate_node_keypair`] so the on-disk
/// permission story is consistent across all node-secret material.
fn write_secure(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    // Make sure the parent directory exists. `secrets_dir` may have been
    // created by `load_or_generate_node_keypair` already, but callers are
    // free to use this helper for files that live elsewhere too.
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    #[cfg(unix)]
    {
        use std::fs::OpenOptions;
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(bytes)?;

        // Belt-and-suspenders: explicitly re-set perms in case a pre-existing
        // file at this path overrode the open-time mode (mode_t is only
        // honored on creation, not on truncation of an existing file).
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }

    #[cfg(not(unix))]
    {
        std::fs::write(path, bytes)?;
    }

    Ok(())
}

/// File names for the on-disk secrets join artifacts under
/// [`zlayer_paths::ZLayerDirs::secrets()`] (`{data_dir}/secrets/`).
///
/// **Stable on-disk contract** consumed by the daemon-side bootstrap (Task #18).
///
/// Layout:
/// - `node_secrets.key` — raw 32-byte X25519 private key (mode 0600).
///   Written by `zlayer_secrets::load_or_generate_node_keypair` on the
///   first call from the CLI; the daemon loads the same file at startup.
/// - `node.jwt` — UTF-8 JWT minted by the leader, `roles: ["node"]`,
///   `node_id` set (mode 0600). Used to authenticate inter-node API calls
///   separately from user identity.
/// - `wrapped_dek.bin` — raw bytes of the sealed-box-wrapped cluster DEK
///   addressed to `node_secrets.key`'s pubkey (mode 0600). Daemon unwraps
///   with the X25519 private key on startup and holds the DEK in zeroized
///   memory.
/// - `dek_generation` — decimal-string DEK generation (e.g. `"3"`), used
///   for rotation drift detection (mode 0600).
const NODE_JWT_FILE: &str = "node.jwt";
const WRAPPED_DEK_FILE: &str = "wrapped_dek.bin";
const DEK_GENERATION_FILE: &str = "dek_generation";

/// Persist the secrets join material returned by the leader under
/// `secrets_dir` (which must already exist — typically created by
/// `load_or_generate_node_keypair`).
///
/// All three pieces (`jwt`, `wrapped_dek`, `dek_generation`) are written
/// atomically from the caller's perspective: any partial state is the
/// result of a process kill mid-write, in which case the daemon-side
/// loader treats a missing or short-read file as "no secrets capability"
/// and falls back to local-only secrets (matching the legacy-leader path).
///
/// Returns `Ok(())` on a clean write of all three files, otherwise an
/// `anyhow::Error` annotated with the path that failed.
fn persist_secrets_join_material(
    secrets_dir: &Path,
    jwt: &str,
    wrapped_dek: &[u8],
    dek_generation: u64,
) -> Result<()> {
    let jwt_path = secrets_dir.join(NODE_JWT_FILE);
    let dek_path = secrets_dir.join(WRAPPED_DEK_FILE);
    let gen_path = secrets_dir.join(DEK_GENERATION_FILE);

    write_secure(&jwt_path, jwt.as_bytes())
        .with_context(|| format!("write node JWT to {}", jwt_path.display()))?;
    write_secure(&dek_path, wrapped_dek)
        .with_context(|| format!("write wrapped DEK to {}", dek_path.display()))?;
    write_secure(&gen_path, dek_generation.to_string().as_bytes())
        .with_context(|| format!("write DEK generation to {}", gen_path.display()))?;

    info!(
        jwt_path = %jwt_path.display(),
        dek_path = %dek_path.display(),
        dek_generation,
        "persisted secrets join material"
    );

    Ok(())
}

// =============================================================================
// Command handlers
// =============================================================================

use crate::cli::NodeCommands;

/// Top-level dispatcher for node subcommands
#[allow(clippy::too_many_lines)]
pub(crate) async fn handle_node(
    node_cmd: &NodeCommands,
    cli_data_dir: &std::path::Path,
) -> Result<()> {
    match node_cmd {
        NodeCommands::Init {
            advertise_addr,
            api_port,
            raft_port,
            overlay_port,
            data_dir,
            overlay_cidr,
            install_wsl,
        } => {
            let resolved_dir = data_dir
                .clone()
                .unwrap_or_else(|| cli_data_dir.to_path_buf());
            // The Windows (and exotic-target) branch of `handle_node_init` takes an
            // extra `ConsentMode` argument for the WSL2 auto-install consent flow.
            // Unix ignores the flag entirely, so the field is only threaded through
            // on non-Unix targets.
            #[cfg(not(unix))]
            let consent = install_wsl.mode();
            #[cfg(unix)]
            let _ = install_wsl; // silence unused-field warning on Unix.
            handle_node_init(
                advertise_addr.clone(),
                *api_port,
                *raft_port,
                *overlay_port,
                resolved_dir,
                overlay_cidr.clone(),
                #[cfg(not(unix))]
                consent,
            )
            .await
        }
        NodeCommands::Join {
            leader_addr,
            token,
            advertise_addr,
            mode,
            services,
            api_port,
            raft_port,
            overlay_port,
            install_wsl,
        } => {
            #[cfg(not(unix))]
            let consent = install_wsl.mode();
            #[cfg(unix)]
            let _ = install_wsl;
            handle_node_join(
                leader_addr.clone(),
                token.clone(),
                advertise_addr.clone(),
                mode.clone(),
                services.clone(),
                *api_port,
                *raft_port,
                *overlay_port,
                cli_data_dir.to_path_buf(),
                #[cfg(not(unix))]
                consent,
            )
            .await
        }
        NodeCommands::List { output } => handle_node_list(output.clone(), cli_data_dir).await,
        NodeCommands::Status { node_id } => handle_node_status(node_id.clone(), cli_data_dir).await,
        NodeCommands::Remove { node_id, force } => {
            handle_node_remove(node_id.clone(), *force, cli_data_dir).await
        }
        NodeCommands::SetMode {
            node_id,
            mode,
            services,
        } => {
            handle_node_set_mode(
                node_id.clone(),
                mode.clone(),
                services.clone(),
                cli_data_dir,
            )
            .await
        }
        NodeCommands::Label { node_id, label } => {
            handle_node_label(node_id.clone(), label.clone(), cli_data_dir).await
        }
        NodeCommands::ForceLeader { api_addr } => handle_node_force_leader(api_addr.clone()).await,
        NodeCommands::RotateSigningKey { grace } => handle_node_rotate_signing_key(*grace).await,
        NodeCommands::Upgrade {
            version,
            cooldown_secs,
            strict,
            yes,
            skip_leader,
        } => {
            handle_node_upgrade(
                cli_data_dir.to_path_buf(),
                version.clone(),
                *cooldown_secs,
                *strict,
                *yes,
                *skip_leader,
            )
            .await
        }
        NodeCommands::GenerateJoinToken {
            deployment,
            api,
            service,
            data_dir,
            ttl,
        } => {
            let resolved_dir = data_dir
                .clone()
                .unwrap_or_else(|| cli_data_dir.to_path_buf());
            handle_node_generate_join_token(
                deployment.clone(),
                api.clone(),
                service.clone(),
                resolved_dir,
                *ttl,
            )
            .await
        }
    }
}

/// Initialize this node as cluster leader (Windows).
///
/// Mirrors the Unix implementation below, substituting the privileged-setup
/// steps for their Windows analogues:
///
/// - **Admin check** via [`zlayer_paths::is_root()`] (backed by `IsUserAnAdmin`
///   on Windows). Cluster bootstrap needs admin for firewall rules, Wintun
///   adapter creation, and `wsl --install`.
/// - **WSL2 backend bootstrap** via the G-6 consent flow. If the user declines
///   we print a note and continue — the cluster still comes up, but Linux
///   container dispatch has to go through a remote peer.
/// - **Overlay + Raft bootstrap** are fully cross-platform so they reuse the
///   Unix logic verbatim.
/// - **GPU detection** calls [`zlayer_agent::detect_gpus`] which now has a real
///   Windows branch (H-4).
/// - **Firewall rules** are installed via [`zlayer_overlay::firewall::ensure_overlay_rules`]
///   (H-6). No-ops on non-Windows.
/// - **Configs persistence** writes `node_config.json` + `overlay_bootstrap.json`
///   under `data_dir`, which resolves to `%ProgramData%\ZLayer\` on Windows
///   via `ZLayerDirs::default_data_dir()`.
#[cfg(windows)]
#[allow(clippy::too_many_lines)]
pub(crate) async fn handle_node_init(
    advertise_addr: String,
    api_port: u16,
    raft_port: u16,
    overlay_port: u16,
    data_dir: PathBuf,
    overlay_cidr: String,
    install_wsl: ConsentMode,
) -> Result<()> {
    use zlayer_overlay::OverlayTransport;

    println!("Initializing ZLayer node as cluster leader...");

    // 1. Admin check — Windows analogue of the Unix root check. Cluster bootstrap
    //    writes firewall rules and (on the `wsl` feature) drives an elevated
    //    `wsl --install`. Both require administrator privileges.
    if !zlayer_paths::is_root() {
        anyhow::bail!(
            "`zlayer node init` on Windows requires running as Administrator. \
             Right-click PowerShell/cmd → Run as Administrator, then re-run this command."
        );
    }

    // 2. Create data directory.
    tokio::fs::create_dir_all(&data_dir)
        .await
        .with_context(|| format!("Failed to create data directory: {}", data_dir.display()))?;
    info!(path = %data_dir.display(), "Created data directory");

    // 3. Check if already initialized (idempotent).
    let config_path = data_dir.join("node_config.json");
    if config_path.exists() {
        let existing = crate::daemon::load_or_init_node_config(&data_dir).await?;
        println!();
        println!("Node already initialized.");
        println!("Node ID:        {}", existing.node_id);
        println!("Raft Node ID:   {}", existing.raft_node_id);
        println!(
            "API Server:     http://{}:{}",
            existing.advertise_addr, existing.api_port
        );
        println!(
            "Raft Address:   {}:{}",
            existing.advertise_addr, existing.raft_port
        );
        println!("Use 'zlayer node status' for full details.");
        return Ok(());
    }

    // 4. WSL2 bootstrap (G-6 consent flow). The `wsl` Cargo feature gates the
    //    entire WSL crate; without it the backend is simply unavailable on this
    //    build and we skip the step with a warning. When present, a user refusal
    //    (`--install-wsl no` or an interactive "n") gracefully degrades: the node
    //    still bootstraps, but local Linux container dispatch is offline and
    //    Linux workloads must be routed through a remote peer.
    #[cfg(feature = "wsl")]
    {
        use zlayer_wsl::errors::WslError;
        use zlayer_wsl::setup::ensure_wsl_backend_ready_with_consent;

        use crate::ui::consent::{wsl2_install_consent, ConsentDecision};

        println!("  Checking WSL2 backend (Linux container support)...");
        let consent_mode = install_wsl;
        let consent_closure = move || match wsl2_install_consent(consent_mode)? {
            ConsentDecision::Granted => Ok(true),
            ConsentDecision::Refused => Ok(false),
        };

        match ensure_wsl_backend_ready_with_consent(consent_closure).await {
            Ok(_) => {
                info!("WSL2 backend is ready");
                println!("  WSL2 backend: ready");
            }
            Err(err) => {
                if let Some(wsl_err) = err.downcast_ref::<WslError>() {
                    match wsl_err {
                        WslError::InstallRefused => {
                            warn!("WSL2 auto-install declined; continuing without local Linux container support");
                            println!(
                                "  WSL2 auto-install declined. The node will bootstrap without a local Linux runtime.\n\
                                 Windows containers (HCS) will still run locally; Linux workloads will be routed\n\
                                 to remote peers. Re-run with `--install-wsl yes` or install WSL2 manually\n\
                                 (`wsl.exe --install --no-distribution`) to enable local Linux containers."
                            );
                        }
                        WslError::RebootRequired => {
                            anyhow::bail!(
                                "WSL2 install completed but Windows requires a reboot. \
                                 Please reboot and re-run `zlayer node init`."
                            );
                        }
                        _ => {
                            warn!(error = %err, "WSL2 setup failed; continuing without local Linux container support");
                            println!(
                                "  WSL2 setup failed: {err}.\n  The node will bootstrap without a local Linux runtime."
                            );
                        }
                    }
                } else {
                    warn!(error = %err, "WSL2 setup failed with an unexpected error; continuing");
                    println!("  WSL2 setup failed: {err}.\n  The node will bootstrap without a local Linux runtime.");
                }
            }
        }
    }
    #[cfg(not(feature = "wsl"))]
    {
        let _ = install_wsl;
        warn!(
            "`wsl` feature not enabled in this build; skipping WSL2 backend bootstrap. \
             Local Linux container dispatch will be unavailable until WSL2 support is compiled in."
        );
    }

    // 5. Generate node ID.
    let node_id = generate_node_id();
    let raft_node_id: u64 = 1; // First node is always ID 1
    info!(node_id = %node_id, raft_node_id = raft_node_id, "Generated node ID");

    // 6. Generate overlay keypair.
    println!("  Generating overlay keypair...");
    let (private_key, public_key) = OverlayTransport::generate_keys()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to generate overlay keys: {e}"))?;
    info!("Generated overlay keypair");

    // 7. Save node config.
    let node_config = NodeConfig {
        node_id: node_id.clone(),
        raft_node_id,
        advertise_addr: advertise_addr.clone(),
        api_port,
        raft_port,
        overlay_port,
        overlay_cidr: overlay_cidr.clone(),
        wireguard_private_key: private_key,
        wireguard_public_key: public_key.clone(),
        is_leader: true,
        created_at: current_timestamp(),
    };
    save_node_config(&data_dir, &node_config).await?;

    // 8. Detect GPUs (H-4 — real Windows GPU enumeration via NVML + WMI).
    println!("  Detecting GPUs...");
    let detected_gpus = zlayer_agent::detect_gpus();
    if detected_gpus.is_empty() {
        info!("No GPUs detected on this node");
    } else {
        info!(
            gpu_count = detected_gpus.len(),
            "Detected GPUs on this node"
        );
        for gpu in &detected_gpus {
            info!(
                pci_bus_id = %gpu.pci_bus_id,
                vendor = %gpu.vendor,
                model = %gpu.model,
                memory_mb = gpu.memory_mb,
                device_path = %gpu.device_path,
                "Detected GPU"
            );
        }
    }

    // 9. Initialize Raft as leader (bootstrap single-node cluster) — cross-platform.
    println!("  Starting Raft consensus...");
    let raft_config = zlayer_scheduler::RaftConfig {
        node_id: raft_node_id,
        address: format!("{advertise_addr}:{raft_port}"),
        raft_port,
        data_dir: data_dir.join("raft"),
        ..Default::default()
    };

    let raft = zlayer_scheduler::RaftCoordinator::new(raft_config)
        .await
        .context("Failed to create Raft coordinator")?;

    raft.bootstrap()
        .await
        .context("Failed to bootstrap Raft cluster")?;
    info!("Raft cluster bootstrapped");

    // Register the leader node in the Raft state machine. See the Unix body for
    // the background on why we register with placeholder overlay metadata that
    // the daemon later overwrites from the persisted bootstrap state.
    let leader_overlay_ip = "10.200.0.1".to_string();
    let leader_slice_cidr = String::new();
    raft.propose(zlayer_scheduler::Request::RegisterNode {
        node_id: raft_node_id,
        address: format!("{advertise_addr}:{raft_port}"),
        wg_public_key: public_key.clone(),
        overlay_ip: leader_overlay_ip,
        overlay_port,
        advertise_addr: advertise_addr.clone(),
        api_port,
        cpu_total: 0.0,
        memory_total: 0,
        disk_total: 0,
        gpus: vec![],
        mode: "full".to_string(),
        os: zlayer_spec::OsKind::from_rust_os(std::env::consts::OS),
        arch: zlayer_spec::ArchKind::from_rust_arch(std::env::consts::ARCH),
        slice_cidr: leader_slice_cidr,
    })
    .await
    .context("Failed to register leader in Raft state")?;

    // 10. Install Windows Firewall rules (H-6). No-op on non-Windows targets.
    if let Err(err) =
        zlayer_overlay::firewall::ensure_overlay_rules(overlay_port, api_port, raft_port)
    {
        warn!(
            error = %err,
            "Failed to install Windows firewall rules for overlay/API/Raft ports. \
             Inbound cluster traffic may be blocked until rules are added manually."
        );
        println!(
            "  Warning: failed to install Windows firewall rules: {err}\n\
             Inbound cluster traffic may be blocked until rules are added manually."
        );
    } else {
        info!("Installed Windows firewall rules for overlay/API/Raft ports");
        println!(
            "  Firewall: rules installed for overlay/API/Raft ports (Private + Domain profiles)"
        );
    }

    // 11. Print success message.
    //
    // Wave 6 (v0.13.0): see the Unix init body above — plaintext join
    // tokens are no longer minted at init time. Operator runs
    // `zlayer node generate-join-token` after starting the daemon.
    println!();
    println!("Node initialized successfully!");
    println!();
    println!("Node ID:        {node_id}");
    println!("Raft Node ID:   {raft_node_id}");
    println!("API Server:     http://{advertise_addr}:{api_port}");
    println!("Raft Address:   {advertise_addr}:{raft_port}");
    println!("Overlay Port:   {overlay_port}");
    println!("Overlay CIDR:   {overlay_cidr}");
    println!("WG Public Key:  {public_key}");
    if detected_gpus.is_empty() {
        println!("GPUs:           None detected");
    } else {
        println!("GPUs:           {} detected", detected_gpus.len());
        for gpu in &detected_gpus {
            println!(
                "  - {} ({}, {} MB VRAM) at {}",
                gpu.model, gpu.vendor, gpu.memory_mb, gpu.pci_bus_id
            );
        }
    }
    println!();
    println!("Next steps:");
    println!("  1. Start the API server:        zlayer serve --daemon");
    println!("  2. Mint a signed join token:    zlayer node generate-join-token");
    println!("  3. Run that command's output on other nodes:");
    println!("       zlayer node join {advertise_addr}:{api_port} --token <TOKEN> --advertise-addr <NODE_IP>");
    println!();
    println!("Note: The API server starts automatically when deploying with 'zlayer deploy' or 'zlayer up'.");

    Ok(())
}

/// `node init` fallback for exotic targets that are neither Unix nor Windows.
///
/// We cannot build the overlay + agent runtime for these targets, so the
/// command bails with an actionable error. The main two supported platforms
/// (Unix — Linux/macOS — and Windows) each have a full implementation above.
#[cfg(all(not(unix), not(windows)))]
pub(crate) async fn handle_node_init(
    _advertise_addr: String,
    _api_port: u16,
    _raft_port: u16,
    _overlay_port: u16,
    _data_dir: PathBuf,
    _overlay_cidr: String,
    _install_wsl: ConsentMode,
) -> Result<()> {
    anyhow::bail!(
        "'node init' is only supported on Linux, macOS, and Windows. \
         This build targets an exotic platform where the overlay + agent runtime is unavailable."
    )
}

/// Initialize this node as cluster leader
#[cfg(unix)]
#[allow(clippy::too_many_lines)]
pub(crate) async fn handle_node_init(
    advertise_addr: String,
    api_port: u16,
    raft_port: u16,
    overlay_port: u16,
    data_dir: PathBuf,
    overlay_cidr: String,
) -> Result<()> {
    use zlayer_overlay::OverlayTransport;

    println!("Initializing ZLayer node as cluster leader...");

    // 1. Create data directory
    tokio::fs::create_dir_all(&data_dir)
        .await
        .with_context(|| format!("Failed to create data directory: {}", data_dir.display()))?;
    info!(path = %data_dir.display(), "Created data directory");

    // 2. Check if already initialized -- use load_or_init_node_config()
    //    from daemon.rs for the load path.  If an existing config is found,
    //    we show its details and exit early (idempotent).
    let config_path = data_dir.join("node_config.json");
    if config_path.exists() {
        let existing = crate::daemon::load_or_init_node_config(&data_dir).await?;
        println!();
        println!("Node already initialized.");
        println!("Node ID:        {}", existing.node_id);
        println!("Raft Node ID:   {}", existing.raft_node_id);
        println!(
            "API Server:     http://{}:{}",
            existing.advertise_addr, existing.api_port
        );
        println!(
            "Raft Address:   {}:{}",
            existing.advertise_addr, existing.raft_port
        );
        println!("Use 'zlayer node status' for full details.");
        return Ok(());
    }

    // 3. Generate node ID
    let node_id = generate_node_id();
    let raft_node_id: u64 = 1; // First node is always ID 1
    info!(node_id = %node_id, raft_node_id = raft_node_id, "Generated node ID");

    // 4. Generate overlay keypair
    println!("  Generating overlay keypair...");
    let (private_key, public_key) = OverlayTransport::generate_keys()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to generate overlay keys: {e}"))?;
    info!("Generated overlay keypair");

    // 5. Save node config
    let node_config = NodeConfig {
        node_id: node_id.clone(),
        raft_node_id,
        advertise_addr: advertise_addr.clone(),
        api_port,
        raft_port,
        overlay_port,
        overlay_cidr: overlay_cidr.clone(),
        wireguard_private_key: private_key,
        wireguard_public_key: public_key.clone(),
        is_leader: true,
        created_at: current_timestamp(),
    };
    save_node_config(&data_dir, &node_config).await?;

    // 6. Detect GPUs on this node
    println!("  Detecting GPUs...");
    let detected_gpus = zlayer_agent::detect_gpus();
    if detected_gpus.is_empty() {
        info!("No GPUs detected on this node");
    } else {
        info!(
            gpu_count = detected_gpus.len(),
            "Detected GPUs on this node"
        );
        for gpu in &detected_gpus {
            info!(
                pci_bus_id = %gpu.pci_bus_id,
                vendor = %gpu.vendor,
                model = %gpu.model,
                memory_mb = gpu.memory_mb,
                device_path = %gpu.device_path,
                "Detected GPU"
            );
        }
    }

    // 7. Initialize Raft as leader (bootstrap single-node cluster)
    println!("  Starting Raft consensus...");
    let raft_config = zlayer_scheduler::RaftConfig {
        node_id: raft_node_id,
        address: format!("{advertise_addr}:{raft_port}"),
        raft_port,
        data_dir: data_dir.join("raft"),
        ..Default::default()
    };

    let raft = zlayer_scheduler::RaftCoordinator::new(raft_config)
        .await
        .context("Failed to create Raft coordinator")?;

    raft.bootstrap()
        .await
        .context("Failed to bootstrap Raft cluster")?;
    info!("Raft cluster bootstrapped");

    // Register the leader node in the Raft state machine with overlay metadata.
    // `handle_node_init` runs before the overlay bootstrap is persisted — the
    // daemon is responsible for computing the leader's `/28` slice on first
    // start (via `OverlayBootstrap::init_leader`), after which the daemon
    // re-registers the leader with the correct `slice_cidr`. Here we register
    // with the legacy hardcoded `10.200.0.1` overlay IP and an empty
    // `slice_cidr`; the daemon overwrites both once the bootstrap state
    // exists on disk.
    let leader_overlay_ip = "10.200.0.1".to_string();
    let leader_slice_cidr = String::new();
    raft.propose(zlayer_scheduler::Request::RegisterNode {
        node_id: raft_node_id,
        address: format!("{advertise_addr}:{raft_port}"),
        wg_public_key: public_key.clone(),
        overlay_ip: leader_overlay_ip,
        overlay_port,
        advertise_addr: advertise_addr.clone(),
        api_port,
        cpu_total: 0.0,
        memory_total: 0,
        disk_total: 0,
        gpus: vec![],
        mode: "full".to_string(),
        os: zlayer_spec::OsKind::from_rust_os(std::env::consts::OS),
        arch: zlayer_spec::ArchKind::from_rust_arch(std::env::consts::ARCH),
        slice_cidr: leader_slice_cidr,
    })
    .await
    .context("Failed to register leader in Raft state")?;

    // 8. Print success message.
    //
    // Wave 6 (v0.13.0): we no longer mint a plaintext join token inline.
    // The daemon owns the cluster `join_secret` (created on first `zlayer
    // serve` boot) and the Ed25519 signing key, so the join token must be
    // requested via `zlayer node generate-join-token` AFTER the daemon
    // starts. Printing a fake plaintext token here would be rejected by
    // every server post-v0.13.0.
    println!();
    println!("Node initialized successfully!");
    println!();
    println!("Node ID:        {node_id}");
    println!("Raft Node ID:   {raft_node_id}");
    println!("API Server:     http://{advertise_addr}:{api_port}");
    println!("Raft Address:   {advertise_addr}:{raft_port}");
    println!("Overlay Port:   {overlay_port}");
    println!("Overlay CIDR:   {overlay_cidr}");
    println!("WG Public Key:  {public_key}");
    if detected_gpus.is_empty() {
        println!("GPUs:           None detected");
    } else {
        println!("GPUs:           {} detected", detected_gpus.len());
        for gpu in &detected_gpus {
            println!(
                "  - {} ({}, {} MB VRAM) at {}",
                gpu.model, gpu.vendor, gpu.memory_mb, gpu.pci_bus_id
            );
        }
    }
    println!();
    println!("Next steps:");
    println!("  1. Start the API server:        zlayer serve --daemon");
    println!("  2. Mint a signed join token:    zlayer node generate-join-token");
    println!("  3. Run that command's output on other nodes:");
    println!("       zlayer node join {advertise_addr}:{api_port} --token <TOKEN> --advertise-addr <NODE_IP>");
    println!();
    println!("Note: The API server starts automatically when deploying with 'zlayer deploy' or 'zlayer up'.");

    Ok(())
}

/// Join an existing cluster as a worker node (Windows).
///
/// See the Unix body below for the canonical flow. The Windows version differs
/// only in the privileged-setup steps:
///
/// - **Admin check** via [`zlayer_paths::is_root()`] (H-5).
/// - **WSL2 bootstrap** via the G-6 consent flow. `InstallRefused` is a soft
///   failure: the node still joins, but without a local Linux runtime.
/// - **Wintun** interface creation is handled transparently by
///   [`zlayer_overlay::OverlayTransport`] on Windows — the call sites are
///   identical to the Unix code.
/// - **GPU detection** uses the Windows branch of `detect_gpus()` (H-4).
/// - **Firewall rules** are installed via [`zlayer_overlay::firewall::ensure_overlay_rules`]
///   (H-6). No-op on non-Windows.
/// - **Configs persistence** writes `node_config.json` + `overlay_bootstrap.json`
///   under `data_dir`, which resolves to `%ProgramData%\ZLayer\` on Windows.
#[cfg(windows)]
#[allow(clippy::too_many_lines)]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn handle_node_join(
    leader_addr: String,
    token: String,
    advertise_addr: String,
    mode: String,
    services: Option<Vec<String>>,
    api_port: u16,
    raft_port: u16,
    overlay_port: u16,
    data_dir_override: PathBuf,
    install_wsl: ConsentMode,
) -> Result<()> {
    use std::time::Duration;
    use zlayer_overlay::OverlayTransport;

    println!("Joining ZLayer cluster at {leader_addr}...");

    // 1. Admin check (H-5) — same reasoning as `handle_node_init`.
    if !zlayer_paths::is_root() {
        anyhow::bail!(
            "`zlayer node join` on Windows requires running as Administrator. \
             Right-click PowerShell/cmd → Run as Administrator, then re-run this command."
        );
    }

    // 2. WSL2 bootstrap (G-6). Mirrors the `handle_node_init` flow — a refusal
    //    degrades gracefully to "join without local Linux runtime" rather than
    //    hard-failing.
    #[cfg(feature = "wsl")]
    {
        use zlayer_wsl::errors::WslError;
        use zlayer_wsl::setup::ensure_wsl_backend_ready_with_consent;

        use crate::ui::consent::{wsl2_install_consent, ConsentDecision};

        println!("  Checking WSL2 backend (Linux container support)...");
        let consent_mode = install_wsl;
        let consent_closure = move || match wsl2_install_consent(consent_mode)? {
            ConsentDecision::Granted => Ok(true),
            ConsentDecision::Refused => Ok(false),
        };

        match ensure_wsl_backend_ready_with_consent(consent_closure).await {
            Ok(_) => {
                info!("WSL2 backend is ready");
                println!("  WSL2 backend: ready");
            }
            Err(err) => {
                if let Some(wsl_err) = err.downcast_ref::<WslError>() {
                    match wsl_err {
                        WslError::InstallRefused => {
                            warn!("WSL2 auto-install declined; joining without local Linux container support");
                            println!(
                                "  WSL2 auto-install declined. The node will join without a local Linux runtime.\n\
                                 Windows containers (HCS) will still run locally; Linux workloads will be routed\n\
                                 to remote peers. Re-run with `--install-wsl yes` or install WSL2 manually\n\
                                 (`wsl.exe --install --no-distribution`) to enable local Linux containers."
                            );
                        }
                        WslError::RebootRequired => {
                            anyhow::bail!(
                                "WSL2 install completed but Windows requires a reboot. \
                                 Please reboot and re-run `zlayer node join`."
                            );
                        }
                        _ => {
                            warn!(error = %err, "WSL2 setup failed; joining without local Linux container support");
                            println!(
                                "  WSL2 setup failed: {err}.\n  The node will join without a local Linux runtime."
                            );
                        }
                    }
                } else {
                    warn!(error = %err, "WSL2 setup failed with an unexpected error; continuing");
                    println!("  WSL2 setup failed: {err}.\n  The node will join without a local Linux runtime.");
                }
            }
        }
    }
    #[cfg(not(feature = "wsl"))]
    {
        let _ = install_wsl;
        warn!(
            "`wsl` feature not enabled in this build; skipping WSL2 backend bootstrap. \
             Local Linux container dispatch will be unavailable until WSL2 support is compiled in."
        );
    }

    // 3. Parse and validate the join token.
    let token_data = parse_cluster_join_token(&token).context("Invalid join token")?;

    info!(
        api_endpoint = %token_data.api_endpoint,
        overlay_cidr = %token_data.overlay_cidr,
        "Parsed join token"
    );

    // 4. Generate overlay keypair.
    println!("  Generating overlay keypair...");
    let (private_key, public_key) = OverlayTransport::generate_keys()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to generate overlay keys: {e}"))?;

    // 5. Determine data directory.
    let data_dir = data_dir_override;
    tokio::fs::create_dir_all(&data_dir)
        .await
        .context("Failed to create data directory")?;

    // 6. Check if already initialized.
    let config_path = data_dir.join("node_config.json");
    if config_path.exists() {
        anyhow::bail!(
            "Node already initialized. Configuration exists at {}. \
            Remove the config file to join a different cluster.",
            config_path.display()
        );
    }

    // 7. Load (or generate) the node X25519 secrets keypair and prepare the
    //    pubkey for the join request. The private key file is left on disk
    //    under `{secrets_dir}/node_secrets.key` for the daemon to load on
    //    startup (Task #18). We deliberately do not keep the private key in
    //    memory after this point — the join flow only needs the pubkey.
    let secrets_dir = zlayer_paths::ZLayerDirs::new(data_dir.clone()).secrets();
    let secrets_pubkey: Option<[u8; 32]> =
        match zlayer_secrets::load_or_generate_node_keypair(&secrets_dir) {
            Ok((_node_priv, node_pub)) => Some(*node_pub.as_bytes()),
            Err(e) => {
                warn!(
                    error = %e,
                    secrets_dir = %secrets_dir.display(),
                    "failed to load/generate node X25519 keypair; joining without \
                     replicated-secrets capability — node will not host secret \
                     ciphertext until the keypair is created and the join is retried"
                );
                None
            }
        };

    // 8. Send join request to the leader.
    println!("  Contacting leader at {}...", token_data.api_endpoint);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .context("Failed to create HTTP client")?;

    let join_request = NodeJoinRequest {
        token: token.clone(),
        advertise_addr: advertise_addr.clone(),
        overlay_port,
        raft_port,
        wg_public_key: public_key.clone(),
        mode: mode.clone(),
        services: services.clone(),
        os: zlayer_spec::OsKind::from_rust_os(std::env::consts::OS),
        arch: zlayer_spec::ArchKind::from_rust_arch(std::env::consts::ARCH),
        secrets_pubkey,
    };

    let response = client
        .post(format!(
            "http://{}/api/v1/cluster/join",
            token_data.api_endpoint
        ))
        .json(&join_request)
        .send()
        .await
        .context("Failed to send join request to leader")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("Join request failed: {status} - {body}");
    }

    let join_response: NodeJoinResponse = response
        .json()
        .await
        .context("Failed to parse join response")?;

    info!(
        node_id = %join_response.node_id,
        raft_node_id = join_response.raft_node_id,
        overlay_ip = %join_response.overlay_ip,
        "Received join response"
    );

    // 8a. Persist the secrets join material returned by the leader so the
    //     daemon can pick it up on startup (Task #18). Three pieces are
    //     written into `{secrets_dir}` (mode 0600 on Unix):
    //       - `node.jwt`         — node-scoped JWT for inter-node calls
    //       - `wrapped_dek.bin`  — sealed-box-wrapped cluster DEK
    //       - `dek_generation`   — decimal generation marker
    //     A legacy leader returns all three as `None`; in that case we log a
    //     warning and continue — the join still succeeds and Raft membership
    //     is not gated on the secrets material.
    if let (Some(jwt), Some(wrapped), Some(generation)) = (
        join_response.node_jwt.as_ref(),
        join_response.wrapped_dek.as_ref(),
        join_response.dek_generation,
    ) {
        persist_secrets_join_material(&secrets_dir, jwt, wrapped, generation)
            .context("failed to persist secrets join material")?;
    } else {
        warn!(
            "leader did not return secrets capability — node will run without \
             cluster-replicated secrets access; re-run join against a Phase-1+ \
             leader to enable replicated secrets"
        );
    }

    // 9. Save node configuration.
    let node_config = NodeConfig {
        node_id: join_response.node_id.clone(),
        raft_node_id: join_response.raft_node_id,
        advertise_addr: advertise_addr.clone(),
        api_port,
        raft_port,
        overlay_port,
        overlay_cidr: token_data.overlay_cidr.clone(),
        wireguard_private_key: private_key,
        wireguard_public_key: public_key.clone(),
        is_leader: false,
        created_at: current_timestamp(),
    };
    save_node_config(&data_dir, &node_config).await?;

    // 10. Configure overlay with peers.
    println!("  Configuring overlay network...");

    let our_overlay_ip: std::net::IpAddr = join_response
        .overlay_ip
        .parse()
        .context("Invalid overlay IP in join response")?;

    let our_slice_cidr: Option<zlayer_overlay::ipnet::IpNet> =
        if join_response.slice_cidr.is_empty() {
            None
        } else {
            Some(
                join_response
                    .slice_cidr
                    .parse()
                    .context("Invalid slice CIDR in join response")?,
            )
        };

    let overlay_prefix_len = token_data
        .overlay_cidr
        .split('/')
        .nth(1)
        .and_then(|s| s.parse::<u8>().ok())
        .unwrap_or(16);

    let overlay_ip_cidr = format!("{our_overlay_ip}/{overlay_prefix_len}");

    let mut overlay_peers: Vec<zlayer_overlay::PeerConfig> = Vec::new();
    for peer in &join_response.peers {
        if peer.wg_public_key.is_empty() {
            warn!(
                peer_id = %peer.node_id,
                "Peer has no WireGuard public key, skipping overlay setup for this peer"
            );
            continue;
        }
        let peer_overlay_ip: std::net::IpAddr = match peer.overlay_ip.parse() {
            Ok(ip) => ip,
            Err(e) => {
                warn!(
                    peer_id = %peer.node_id,
                    overlay_ip = %peer.overlay_ip,
                    error = %e,
                    "Failed to parse peer overlay IP, skipping"
                );
                continue;
            }
        };
        let endpoint = format!("{}:{}", peer.advertise_addr, peer.overlay_port);
        overlay_peers.push(zlayer_overlay::PeerConfig::new(
            peer.node_id.clone(),
            peer.wg_public_key.clone(),
            endpoint,
            peer_overlay_ip,
        ));
        info!(
            peer_id = %peer.node_id,
            peer_addr = %peer.advertise_addr,
            peer_overlay_ip = %peer_overlay_ip,
            "Configured overlay peer"
        );
    }

    // 11a. Persist overlay bootstrap state (same schema as Unix).
    let bootstrap_state = zlayer_overlay::BootstrapState {
        config: zlayer_overlay::BootstrapConfig {
            cidr: token_data.overlay_cidr.clone(),
            node_ip: our_overlay_ip,
            interface: zlayer_overlay::DEFAULT_INTERFACE_NAME.to_string(),
            port: overlay_port,
            private_key: node_config.wireguard_private_key.clone(),
            public_key: node_config.wireguard_public_key.clone(),
            is_leader: false,
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            slice_cidr: our_slice_cidr,
        },
        peers: overlay_peers.clone(),
        allocator_state: None,
        slice_allocator_state: None,
    };
    let bootstrap_path = data_dir.join("overlay_bootstrap.json");
    let bootstrap_json = serde_json::to_string_pretty(&bootstrap_state)
        .context("Failed to serialize overlay bootstrap state")?;
    tokio::fs::write(&bootstrap_path, bootstrap_json)
        .await
        .with_context(|| {
            format!(
                "Failed to write overlay bootstrap state to {}",
                bootstrap_path.display()
            )
        })?;
    info!(path = %bootstrap_path.display(), "Saved overlay bootstrap state");

    // 11. Create and configure the overlay transport (Wintun on Windows).
    #[allow(clippy::needless_update)]
    let overlay_config = zlayer_overlay::OverlayConfig {
        local_endpoint: std::net::SocketAddr::new(
            std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED),
            overlay_port,
        ),
        private_key: node_config.wireguard_private_key.clone(),
        public_key: node_config.wireguard_public_key.clone(),
        overlay_cidr: overlay_ip_cidr,
        peer_discovery_interval: Duration::from_secs(30),
        ..zlayer_overlay::OverlayConfig::default()
    };

    let peer_infos: Vec<zlayer_overlay::PeerInfo> = overlay_peers
        .iter()
        .filter_map(|p| match p.to_peer_info() {
            Ok(info) => Some(info),
            Err(e) => {
                warn!(peer = %p.node_id, error = %e, "Failed to convert peer info, skipping");
                None
            }
        })
        .collect();

    let interface_name = zlayer_overlay::DEFAULT_INTERFACE_NAME.to_string();
    let mut transport = zlayer_overlay::OverlayTransport::new(overlay_config, interface_name);

    match transport.create_interface().await {
        Ok(()) => match transport.configure(&peer_infos).await {
            Ok(()) => {
                info!(
                    overlay_ip = %our_overlay_ip,
                    peer_count = peer_infos.len(),
                    "Overlay network interface configured successfully"
                );
                println!(
                    "  Overlay network up: {} with {} peer(s)",
                    our_overlay_ip,
                    peer_infos.len()
                );
                drop(transport);
            }
            Err(e) => {
                warn!(
                    error = %e,
                    "Failed to configure overlay transport. \
                     The overlay bootstrap state has been saved and the daemon will \
                     retry on next start."
                );
                println!("  Warning: Overlay interface created but configuration failed: {e}");
            }
        },
        Err(e) => {
            warn!(
                error = %e,
                "Failed to create overlay (Wintun) network interface. \
                 Requires administrator privileges. The overlay bootstrap state has \
                 been saved and the daemon will create the interface on next start."
            );
            println!(
                "  Warning: Could not create overlay interface ({e}). \
                 The daemon will set it up on next start."
            );
        }
    }

    // 12. Detect GPUs (H-4) for the node-registration payload/info output.
    println!("  Detecting GPUs...");
    let detected_gpus = zlayer_agent::detect_gpus();
    if detected_gpus.is_empty() {
        info!("No GPUs detected on this node");
    } else {
        info!(
            gpu_count = detected_gpus.len(),
            "Detected GPUs on this node"
        );
        for gpu in &detected_gpus {
            info!(
                pci_bus_id = %gpu.pci_bus_id,
                vendor = %gpu.vendor,
                model = %gpu.model,
                memory_mb = gpu.memory_mb,
                device_path = %gpu.device_path,
                "Detected GPU"
            );
        }
    }

    // 13. Install Windows Firewall rules (H-6). No-op on non-Windows.
    if let Err(err) =
        zlayer_overlay::firewall::ensure_overlay_rules(overlay_port, api_port, raft_port)
    {
        warn!(
            error = %err,
            "Failed to install Windows firewall rules for overlay/API/Raft ports"
        );
        println!(
            "  Warning: failed to install Windows firewall rules: {err}\n\
             Inbound cluster traffic may be blocked until rules are added manually."
        );
    } else {
        info!("Installed Windows firewall rules for overlay/API/Raft ports");
        println!(
            "  Firewall: rules installed for overlay/API/Raft ports (Private + Domain profiles)"
        );
    }

    // 14. Print success message.
    println!();
    println!("Successfully joined cluster!");
    println!();
    println!("Node ID:        {}", join_response.node_id);
    println!("Raft Node ID:   {}", join_response.raft_node_id);
    println!("Overlay IP:     {}", join_response.overlay_ip);
    if !join_response.slice_cidr.is_empty() {
        println!("Slice CIDR:     {}", join_response.slice_cidr);
    }
    println!("Mode:           {mode}");
    if let Some(svcs) = services {
        println!("Services:       {}", svcs.join(", "));
    }
    println!("Peers:          {}", join_response.peers.len());
    if detected_gpus.is_empty() {
        println!("GPUs:           None detected");
    } else {
        println!("GPUs:           {} detected", detected_gpus.len());
        for gpu in &detected_gpus {
            println!(
                "  - {} ({}, {} MB VRAM) at {}",
                gpu.model, gpu.vendor, gpu.memory_mb, gpu.pci_bus_id
            );
        }
    }
    println!();
    println!("Run 'zlayer deploy' or 'zlayer up' with a spec file to start processing workloads.");

    Ok(())
}

/// `node join` fallback for exotic targets that are neither Unix nor Windows.
#[cfg(all(not(unix), not(windows)))]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn handle_node_join(
    _leader_addr: String,
    _token: String,
    _advertise_addr: String,
    _mode: String,
    _services: Option<Vec<String>>,
    _api_port: u16,
    _raft_port: u16,
    _overlay_port: u16,
    _data_dir_override: PathBuf,
    _install_wsl: ConsentMode,
) -> Result<()> {
    anyhow::bail!(
        "'node join' is only supported on Linux, macOS, and Windows. \
         This build targets an exotic platform where the overlay + agent runtime is unavailable."
    )
}

/// Join an existing cluster as a worker node
#[cfg(unix)]
#[allow(clippy::too_many_lines)]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn handle_node_join(
    leader_addr: String,
    token: String,
    advertise_addr: String,
    mode: String,
    services: Option<Vec<String>>,
    api_port: u16,
    raft_port: u16,
    overlay_port: u16,
    data_dir_override: PathBuf,
) -> Result<()> {
    use std::time::Duration;
    use zlayer_overlay::OverlayTransport;

    println!("Joining ZLayer cluster at {leader_addr}...");

    // 1. Parse and validate the join token
    let token_data = parse_cluster_join_token(&token).context("Invalid join token")?;

    info!(
        api_endpoint = %token_data.api_endpoint,
        overlay_cidr = %token_data.overlay_cidr,
        "Parsed join token"
    );

    // 2. Generate overlay keypair for this node
    println!("  Generating overlay keypair...");
    let (private_key, public_key) = OverlayTransport::generate_keys()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to generate overlay keys: {e}"))?;

    // 3. Determine data directory
    let data_dir = data_dir_override;
    tokio::fs::create_dir_all(&data_dir)
        .await
        .context("Failed to create data directory")?;

    // 4. Check if already initialized
    let config_path = data_dir.join("node_config.json");
    if config_path.exists() {
        anyhow::bail!(
            "Node already initialized. Configuration exists at {}. \
            Remove the config file to join a different cluster.",
            config_path.display()
        );
    }

    // 5. Load (or generate) the node X25519 secrets keypair and prepare the
    //    pubkey for the join request. The private key file is left on disk
    //    under `{secrets_dir}/node_secrets.key` for the daemon to load on
    //    startup (Task #18). We deliberately do not keep the private key in
    //    memory after this point — the join flow only needs the pubkey.
    let secrets_dir = zlayer_paths::ZLayerDirs::new(data_dir.clone()).secrets();
    let secrets_pubkey: Option<[u8; 32]> =
        match zlayer_secrets::load_or_generate_node_keypair(&secrets_dir) {
            Ok((_node_priv, node_pub)) => Some(*node_pub.as_bytes()),
            Err(e) => {
                warn!(
                    error = %e,
                    secrets_dir = %secrets_dir.display(),
                    "failed to load/generate node X25519 keypair; joining without \
                     replicated-secrets capability — node will not host secret \
                     ciphertext until the keypair is created and the join is retried"
                );
                None
            }
        };

    // 6. Send join request to the leader
    println!("  Contacting leader at {}...", token_data.api_endpoint);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .context("Failed to create HTTP client")?;

    let join_request = NodeJoinRequest {
        token: token.clone(),
        advertise_addr: advertise_addr.clone(),
        overlay_port,
        raft_port,
        wg_public_key: public_key.clone(),
        mode: mode.clone(),
        services: services.clone(),
        os: zlayer_spec::OsKind::from_rust_os(std::env::consts::OS),
        arch: zlayer_spec::ArchKind::from_rust_arch(std::env::consts::ARCH),
        secrets_pubkey,
    };

    let response = client
        .post(format!(
            "http://{}/api/v1/cluster/join",
            token_data.api_endpoint
        ))
        .json(&join_request)
        .send()
        .await
        .context("Failed to send join request to leader")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("Join request failed: {status} - {body}");
    }

    let join_response: NodeJoinResponse = response
        .json()
        .await
        .context("Failed to parse join response")?;

    info!(
        node_id = %join_response.node_id,
        raft_node_id = join_response.raft_node_id,
        overlay_ip = %join_response.overlay_ip,
        "Received join response"
    );

    // 6a. Persist the secrets join material returned by the leader so the
    //     daemon can pick it up on startup (Task #18). Three pieces are
    //     written into `{secrets_dir}` (mode 0600 on Unix):
    //       - `node.jwt`         — node-scoped JWT for inter-node calls
    //       - `wrapped_dek.bin`  — sealed-box-wrapped cluster DEK
    //       - `dek_generation`   — decimal generation marker
    //     A legacy leader returns all three as `None`; in that case we log a
    //     warning and continue — the join still succeeds and Raft membership
    //     is not gated on the secrets material.
    if let (Some(jwt), Some(wrapped), Some(generation)) = (
        join_response.node_jwt.as_ref(),
        join_response.wrapped_dek.as_ref(),
        join_response.dek_generation,
    ) {
        persist_secrets_join_material(&secrets_dir, jwt, wrapped, generation)
            .context("failed to persist secrets join material")?;
    } else {
        warn!(
            "leader did not return secrets capability — node will run without \
             cluster-replicated secrets access; re-run join against a Phase-1+ \
             leader to enable replicated secrets"
        );
    }

    // 7. Save node configuration
    let node_config = NodeConfig {
        node_id: join_response.node_id.clone(),
        raft_node_id: join_response.raft_node_id,
        advertise_addr: advertise_addr.clone(),
        api_port,
        raft_port,
        overlay_port,
        overlay_cidr: token_data.overlay_cidr.clone(),
        wireguard_private_key: private_key,
        wireguard_public_key: public_key.clone(),
        is_leader: false,
        created_at: current_timestamp(),
    };
    save_node_config(&data_dir, &node_config).await?;

    // 8. Configure overlay with peers
    println!("  Configuring overlay network...");

    // Parse the overlay IP assigned by the leader
    let our_overlay_ip: std::net::IpAddr = join_response
        .overlay_ip
        .parse()
        .context("Invalid overlay IP in join response")?;

    // Parse the per-node slice CIDR assigned by the leader. Empty string
    // preserves pre-slice-aware behavior (allocator ranges over the full
    // cluster CIDR). Non-empty strings are required to parse; a malformed
    // slice from the leader is a hard error because the daemon would later
    // misconfigure overlay routing.
    let our_slice_cidr: Option<zlayer_overlay::ipnet::IpNet> =
        if join_response.slice_cidr.is_empty() {
            None
        } else {
            Some(
                join_response
                    .slice_cidr
                    .parse()
                    .context("Invalid slice CIDR in join response")?,
            )
        };

    // Extract the prefix length from the overlay CIDR (e.g. "10.200.0.0/16" -> 16)
    let overlay_prefix_len = token_data
        .overlay_cidr
        .split('/')
        .nth(1)
        .and_then(|s| s.parse::<u8>().ok())
        .unwrap_or(16);

    // Build the overlay IP with the network prefix for proper routing
    // e.g. "10.200.0.5/16" so all overlay peers are routable
    let overlay_ip_cidr = format!("{our_overlay_ip}/{overlay_prefix_len}");

    // Convert join response peers into zlayer_overlay PeerConfig entries
    let mut overlay_peers: Vec<zlayer_overlay::PeerConfig> = Vec::new();
    for peer in &join_response.peers {
        if peer.wg_public_key.is_empty() {
            warn!(
                peer_id = %peer.node_id,
                "Peer has no WireGuard public key, skipping overlay setup for this peer"
            );
            continue;
        }
        let peer_overlay_ip: std::net::IpAddr = match peer.overlay_ip.parse() {
            Ok(ip) => ip,
            Err(e) => {
                warn!(
                    peer_id = %peer.node_id,
                    overlay_ip = %peer.overlay_ip,
                    error = %e,
                    "Failed to parse peer overlay IP, skipping"
                );
                continue;
            }
        };
        let endpoint = format!("{}:{}", peer.advertise_addr, peer.overlay_port);
        overlay_peers.push(zlayer_overlay::PeerConfig::new(
            peer.node_id.clone(),
            peer.wg_public_key.clone(),
            endpoint,
            peer_overlay_ip,
        ));
        info!(
            peer_id = %peer.node_id,
            peer_addr = %peer.advertise_addr,
            peer_overlay_ip = %peer_overlay_ip,
            "Configured overlay peer"
        );
    }

    // Persist overlay bootstrap state so the daemon can reload it later
    // via OverlayBootstrap::load()
    let bootstrap_state = zlayer_overlay::BootstrapState {
        config: zlayer_overlay::BootstrapConfig {
            cidr: token_data.overlay_cidr.clone(),
            node_ip: our_overlay_ip,
            interface: zlayer_overlay::DEFAULT_INTERFACE_NAME.to_string(),
            port: overlay_port,
            private_key: node_config.wireguard_private_key.clone(),
            public_key: node_config.wireguard_public_key.clone(),
            is_leader: false,
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            // Slice CIDR advertised by the leader (T8). Populated when the
            // leader is slice-aware; `None` for back-compat with pre-slice
            // leaders, in which case the agent allocator ranges over the
            // full cluster CIDR (legacy behavior).
            slice_cidr: our_slice_cidr,
        },
        peers: overlay_peers.clone(),
        allocator_state: None,
        slice_allocator_state: None,
    };
    let bootstrap_path = data_dir.join("overlay_bootstrap.json");
    let bootstrap_json = serde_json::to_string_pretty(&bootstrap_state)
        .context("Failed to serialize overlay bootstrap state")?;
    tokio::fs::write(&bootstrap_path, bootstrap_json)
        .await
        .with_context(|| {
            format!(
                "Failed to write overlay bootstrap state to {}",
                bootstrap_path.display()
            )
        })?;
    info!(path = %bootstrap_path.display(), "Saved overlay bootstrap state");

    // Create and configure the overlay transport (boringtun TUN device)
    // This gives the joining node immediate overlay connectivity.
    // The TUN device will be cleaned up when this process exits; the daemon
    // will re-create it from the persisted bootstrap state on next start.
    #[allow(clippy::needless_update)]
    let overlay_config = zlayer_overlay::OverlayConfig {
        local_endpoint: std::net::SocketAddr::new(
            std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED),
            overlay_port,
        ),
        private_key: node_config.wireguard_private_key.clone(),
        public_key: node_config.wireguard_public_key.clone(),
        overlay_cidr: overlay_ip_cidr,
        peer_discovery_interval: Duration::from_secs(30),
        ..zlayer_overlay::OverlayConfig::default()
    };

    // Convert PeerConfig to PeerInfo for the transport layer
    let peer_infos: Vec<zlayer_overlay::PeerInfo> = overlay_peers
        .iter()
        .filter_map(|p| match p.to_peer_info() {
            Ok(info) => Some(info),
            Err(e) => {
                warn!(peer = %p.node_id, error = %e, "Failed to convert peer info, skipping");
                None
            }
        })
        .collect();

    let interface_name = zlayer_overlay::DEFAULT_INTERFACE_NAME.to_string();
    let mut transport = zlayer_overlay::OverlayTransport::new(overlay_config, interface_name);

    match transport.create_interface().await {
        Ok(()) => {
            match transport.configure(&peer_infos).await {
                Ok(()) => {
                    info!(
                        overlay_ip = %our_overlay_ip,
                        peer_count = peer_infos.len(),
                        "Overlay network interface configured successfully"
                    );
                    println!(
                        "  Overlay network up: {} with {} peer(s)",
                        our_overlay_ip,
                        peer_infos.len()
                    );
                    // Keep the transport alive until we finish printing the
                    // success message below; it will be cleaned up on exit.
                    // The daemon re-creates the overlay from persisted state.
                    drop(transport);
                }
                Err(e) => {
                    warn!(
                        error = %e,
                        "Failed to configure overlay transport. \
                         The overlay bootstrap state has been saved and the daemon will \
                         retry on next start."
                    );
                    println!("  Warning: Overlay interface created but configuration failed: {e}");
                }
            }
        }
        Err(e) => {
            // Non-fatal: the overlay state is persisted, the daemon will set
            // it up when it starts. This commonly fails in unprivileged
            // environments or CI.
            warn!(
                error = %e,
                "Failed to create overlay network interface. \
                 Requires CAP_NET_ADMIN capability. The overlay bootstrap state has \
                 been saved and the daemon will create the interface on next start."
            );
            println!(
                "  Warning: Could not create overlay interface ({e}). \
                 The daemon will set it up on next start."
            );
        }
    }

    // TODO: Existing peers also need to add this new node as a peer. Currently
    // requires periodic reconciliation from Raft state or an explicit push
    // notification from the leader after processing the join request.

    // 8. Detect GPUs on this node
    println!("  Detecting GPUs...");
    let detected_gpus = zlayer_agent::detect_gpus();
    if detected_gpus.is_empty() {
        info!("No GPUs detected on this node");
    } else {
        info!(
            gpu_count = detected_gpus.len(),
            "Detected GPUs on this node"
        );
        for gpu in &detected_gpus {
            info!(
                pci_bus_id = %gpu.pci_bus_id,
                vendor = %gpu.vendor,
                model = %gpu.model,
                memory_mb = gpu.memory_mb,
                device_path = %gpu.device_path,
                "Detected GPU"
            );
        }
    }

    // 9. Print success message
    println!();
    println!("Successfully joined cluster!");
    println!();
    println!("Node ID:        {}", join_response.node_id);
    println!("Raft Node ID:   {}", join_response.raft_node_id);
    println!("Overlay IP:     {}", join_response.overlay_ip);
    if !join_response.slice_cidr.is_empty() {
        println!("Slice CIDR:     {}", join_response.slice_cidr);
    }
    println!("Mode:           {mode}");
    if let Some(svcs) = services {
        println!("Services:       {}", svcs.join(", "));
    }
    println!("Peers:          {}", join_response.peers.len());
    if detected_gpus.is_empty() {
        println!("GPUs:           None detected");
    } else {
        println!("GPUs:           {} detected", detected_gpus.len());
        for gpu in &detected_gpus {
            println!(
                "  - {} ({}, {} MB VRAM) at {}",
                gpu.model, gpu.vendor, gpu.memory_mb, gpu.pci_bus_id
            );
        }
    }
    println!();
    println!("Run 'zlayer deploy' or 'zlayer up' with a spec file to start processing workloads.");

    Ok(())
}

/// List all nodes in the cluster
pub(crate) async fn handle_node_list(output: String, cli_data_dir: &std::path::Path) -> Result<()> {
    use std::time::Duration;

    // Try to load local node config to get API endpoint
    let data_dir = cli_data_dir.to_path_buf();
    let node_config = load_or_init_node_config(&data_dir).await?;

    let api_endpoint = format!("{}:{}", node_config.advertise_addr, node_config.api_port);

    // Fetch node list from API
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .context("Failed to create HTTP client")?;

    let response = client
        .get(format!("http://{api_endpoint}/api/v1/cluster/nodes"))
        .send()
        .await;

    let nodes: Vec<NodeStatus> = match response {
        Ok(resp) if resp.status().is_success() => {
            resp.json().await.unwrap_or_else(|_| {
                // Return at least the local node if we can't parse
                vec![NodeStatus {
                    id: node_config.node_id.clone(),
                    address: format!("{}:{}", node_config.advertise_addr, node_config.api_port),
                    status: "unknown".to_string(),
                    mode: if node_config.is_leader {
                        "leader".to_string()
                    } else {
                        "worker".to_string()
                    },
                    services: vec![],
                    is_leader: node_config.is_leader,
                    role: if node_config.is_leader {
                        "leader".to_string()
                    } else {
                        "learner".to_string()
                    },
                }]
            })
        }
        _ => {
            // API not available, show local node info
            warn!("Could not connect to cluster API. Showing local node only.");
            vec![NodeStatus {
                id: node_config.node_id.clone(),
                address: format!("{}:{}", node_config.advertise_addr, node_config.api_port),
                status: "local".to_string(),
                mode: if node_config.is_leader {
                    "leader".to_string()
                } else {
                    "worker".to_string()
                },
                services: vec![],
                is_leader: node_config.is_leader,
                role: if node_config.is_leader {
                    "leader".to_string()
                } else {
                    "learner".to_string()
                },
            }]
        }
    };

    if output == "json" {
        println!("{}", serde_json::to_string_pretty(&nodes)?);
    } else {
        // Table format
        println!(
            "{:<36} {:<20} {:<10} {:<10} {:<20} SERVICES",
            "NODE ID", "ADDRESS", "STATUS", "MODE", "ROLE"
        );
        println!("{}", "-".repeat(114));

        for node in nodes {
            let services = if node.services.is_empty() {
                "-".to_string()
            } else {
                let s = node.services.join(", ");
                if s.len() > 20 {
                    format!("{}...", &s[..17])
                } else {
                    s
                }
            };

            // Map the server-provided Raft role string to a friendly label.
            // README §400 ("Adding Worker Nodes") uses "Worker" for the
            // non-leader/learner case, so we follow that convention here.
            let base_role = match node.role.as_str() {
                "leader" => "Leader",
                "voter" => "Voter",
                "learner" => "Worker",
                "" => "Unknown",
                other => other,
            };

            // Decorate the role with the node's lifecycle status when it's
            // not in the healthy "ready" / "local" state so operators
            // immediately see drains and dead nodes.
            let role_display = match node.status.as_str() {
                "draining" => format!("{base_role} (draining)"),
                "dead" => format!("{base_role} [DEAD]"),
                _ => base_role.to_string(),
            };

            println!(
                "{:<36} {:<20} {:<10} {:<10} {:<20} {}",
                node.id, node.address, node.status, node.mode, role_display, services
            );
        }
    }

    Ok(())
}

/// Show detailed status of a node
pub(crate) async fn handle_node_status(
    node_id: Option<String>,
    cli_data_dir: &std::path::Path,
) -> Result<()> {
    let data_dir = cli_data_dir.to_path_buf();
    let node_config = load_or_init_node_config(&data_dir).await?;

    // If no node_id specified, show this node
    let target_id = node_id.unwrap_or(node_config.node_id.clone());

    if target_id == node_config.node_id {
        // Show local node detailed status
        println!("Node Status");
        println!("{}", "=".repeat(50));
        println!();
        println!("Node ID:            {}", node_config.node_id);
        println!("Raft Node ID:       {}", node_config.raft_node_id);
        println!(
            "Role:               {}",
            if node_config.is_leader {
                "Leader"
            } else {
                "Worker"
            }
        );
        println!("Created At:         {}", node_config.created_at);
        println!();
        println!("Network Configuration:");
        println!("  Advertise Address: {}", node_config.advertise_addr);
        println!("  API Port:          {}", node_config.api_port);
        println!("  Raft Port:         {}", node_config.raft_port);
        println!("  Overlay Port:      {}", node_config.overlay_port);
        println!("  Overlay CIDR:      {}", node_config.overlay_cidr);
        println!();
        println!("Overlay:");
        println!("  Public Key:        {}", node_config.wireguard_public_key);
        println!();
        println!("Endpoints:");
        println!(
            "  API:   http://{}:{}",
            node_config.advertise_addr, node_config.api_port
        );
        println!(
            "  Raft:  {}:{}",
            node_config.advertise_addr, node_config.raft_port
        );
        println!(
            "  WG:    {}:{}/udp",
            node_config.advertise_addr, node_config.overlay_port
        );
    } else {
        // Fetch remote node status via API
        use std::time::Duration;
        let api_endpoint = format!("{}:{}", node_config.advertise_addr, node_config.api_port);

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .context("Failed to create HTTP client")?;

        let response = client
            .get(format!(
                "http://{api_endpoint}/api/v1/cluster/nodes/{target_id}"
            ))
            .send()
            .await
            .context("Failed to fetch node status")?;

        if !response.status().is_success() {
            anyhow::bail!("Node '{target_id}' not found in cluster");
        }

        let status: NodeStatus = response
            .json()
            .await
            .context("Failed to parse node status")?;

        println!("Node Status");
        println!("{}", "=".repeat(50));
        println!();
        println!("Node ID:    {}", status.id);
        println!("Address:    {}", status.address);
        println!("Status:     {}", status.status);
        println!("Mode:       {}", status.mode);
        println!(
            "Leader:     {}",
            if status.is_leader { "Yes" } else { "No" }
        );
        if !status.services.is_empty() {
            println!("Services:   {}", status.services.join(", "));
        }
    }

    Ok(())
}

/// Remove a node from the cluster
pub(crate) async fn handle_node_remove(
    node_id: String,
    force: bool,
    cli_data_dir: &std::path::Path,
) -> Result<()> {
    use std::time::Duration;

    let data_dir = cli_data_dir.to_path_buf();
    let node_config = load_or_init_node_config(&data_dir).await?;

    // Cannot remove yourself
    if node_id == node_config.node_id {
        anyhow::bail!(
            "Cannot remove the current node. To leave the cluster, stop the node and remove its data directory."
        );
    }

    let api_endpoint = format!("{}:{}", node_config.advertise_addr, node_config.api_port);

    println!("Removing node '{node_id}' from cluster...");
    if force {
        warn!("Force removal enabled - services will not be migrated");
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .context("Failed to create HTTP client")?;

    let mut url = format!("http://{api_endpoint}/api/v1/cluster/nodes/{node_id}");
    if force {
        url.push_str("?force=true");
    }

    let response = client
        .delete(&url)
        .send()
        .await
        .context("Failed to send remove request")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("Failed to remove node: {status} - {body}");
    }

    println!("Node '{node_id}' removed successfully.");
    if !force {
        println!("Services have been migrated to other nodes.");
    }

    Ok(())
}

/// Body of `POST /api/v1/cluster/upgrade`.
#[derive(Debug, Clone, Serialize)]
struct ClusterUpgradeRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    cooldown_secs: u64,
    strict: bool,
}

/// One entry in the `errors` field of the upgrade response.
#[derive(Debug, Clone, Deserialize)]
struct ClusterUpgradeError {
    node_id: String,
    message: String,
}

/// Response body for `POST /api/v1/cluster/upgrade`.
#[derive(Debug, Clone, Deserialize)]
struct ClusterUpgradeResponse {
    #[serde(default)]
    upgraded: Vec<String>,
    #[serde(default)]
    skipped: Vec<String>,
    #[serde(default)]
    errors: Vec<ClusterUpgradeError>,
}

/// Roll a new zlayer version across every node in the cluster.
///
/// The local daemon's API endpoint is contacted first. If it answers with
/// 421 Misdirected Request, the request is re-issued once against the leader
/// address advertised in the `X-Leader-Addr` response header. The leader
/// orchestrates the rolling follower walk internally and blocks until done,
/// so the HTTP client uses a generous 30-minute timeout.
///
/// After the follower walk returns, the leader is upgraded last via
/// `POST /api/v1/cluster/upgrade-self` (unless `skip_leader` is true, or
/// `strict` mode short-circuited because of follower errors). The leader's
/// daemon exits with code 75 and the OS supervisor (launchd / systemd)
/// respawns it on the new binary; Raft holds a brief re-election.
#[allow(clippy::too_many_lines)]
pub(crate) async fn handle_node_upgrade(
    data_dir: PathBuf,
    version: Option<String>,
    cooldown_secs: u64,
    strict: bool,
    yes: bool,
    skip_leader: bool,
) -> Result<()> {
    use reqwest::StatusCode;
    use std::io::{self, Write};
    use std::time::Duration;

    // 1. Resolve the local daemon's API endpoint from node_config.json.
    let node_config = load_or_init_node_config(&data_dir).await?;
    let local_api_endpoint = format!("{}:{}", node_config.advertise_addr, node_config.api_port);

    // 2. Confirmation prompt (skipped with `-y`).
    if !yes {
        let version_msg = match version.as_deref() {
            Some(v) => format!(" {v}"),
            None => String::new(),
        };
        let leader_msg = if skip_leader {
            " (followers only)"
        } else {
            " (followers, then the leader)"
        };
        print!(
            "This will roll zlayer{version_msg} across every node in the cluster{leader_msg}. Continue? [y/N] "
        );
        io::stdout().flush().ok();
        let mut input = String::new();
        io::stdin()
            .read_line(&mut input)
            .context("Failed to read confirmation")?;
        let answer = input.trim().to_ascii_lowercase();
        if answer != "y" && answer != "yes" {
            println!("Aborted.");
            return Ok(());
        }
    }

    // 3. Long-running HTTP client — rolling upgrades can take many minutes.
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(1800))
        .build()
        .context("Failed to create HTTP client")?;

    let body = ClusterUpgradeRequest {
        version: version.clone(),
        cooldown_secs,
        strict,
    };

    // 4. First attempt: hit the local daemon.
    let url = format!("http://{local_api_endpoint}/api/v1/cluster/upgrade");
    info!(%url, "Sending cluster upgrade request to local daemon");
    let response = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .context("Failed to send cluster upgrade request")?;

    // 5. Handle 421 by retrying against the leader exactly once. Track
    //    which addr we reached the leader on, since that's where we'll
    //    send the `upgrade-self` follow-up.
    let (response, leader_endpoint) = if response.status() == StatusCode::MISDIRECTED_REQUEST {
        let leader_addr = response
            .headers()
            .get("X-Leader-Addr")
            .and_then(|v| v.to_str().ok())
            .map(std::string::ToString::to_string);
        match leader_addr {
            Some(addr) => {
                let leader_url = format!("http://{addr}/api/v1/cluster/upgrade");
                info!("Local daemon is not the leader. Retrying via leader at {addr}...");
                let resp = client
                    .post(&leader_url)
                    .json(&body)
                    .send()
                    .await
                    .context("Failed to send cluster upgrade request to leader")?;
                (resp, addr)
            }
            None => {
                anyhow::bail!(
                    "Local daemon is not the leader and could not determine leader address. \
                         Run this command on the leader node."
                );
            }
        }
    } else {
        (response, local_api_endpoint.clone())
    };

    // 6. Bail on any non-2xx.
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("Upgrade request failed: {status} - {body}");
    }

    // 7. Parse the upgrade result and report it.
    let result: ClusterUpgradeResponse = response
        .json()
        .await
        .context("Failed to parse cluster upgrade response")?;

    if result.upgraded.is_empty() {
        println!("✓ Upgraded 0 follower(s).");
    } else {
        println!(
            "✓ Upgraded {} follower(s): {}",
            result.upgraded.len(),
            result.upgraded.join(", ")
        );
    }
    if !result.skipped.is_empty() {
        println!("- Skipped: {}", result.skipped.join(", "));
    }
    if !result.errors.is_empty() {
        println!("- Errors:");
        for err in &result.errors {
            println!("    node {}: {}", err.node_id, err.message);
        }
    }

    // 8. Decide whether to also upgrade the leader.
    let leader_upgrade_attempted = if skip_leader {
        info!("Skipping leader upgrade (--skip-leader). Run 'zlayer self-update --restart' on the leader manually.");
        println!();
        println!(
            "Note: --skip-leader set. The leader was NOT auto-upgraded. \
             Run 'zlayer self-update --restart' on the leader manually."
        );
        false
    } else if strict && !result.errors.is_empty() {
        warn!("Follower upgrades had errors; skipping leader upgrade.");
        println!();
        println!(
            "- Leader skipped: follower upgrades had errors and --strict was set. \
             Run 'zlayer self-update --restart' on the leader after resolving the failures."
        );
        false
    } else {
        upgrade_leader_self(&client, &leader_endpoint, version.as_deref()).await?;
        true
    };

    // 9. Final summary.
    println!();
    if leader_upgrade_attempted {
        println!("Rolling upgrade complete. Followers + leader requested.");
    } else {
        println!("Rolling upgrade complete. Followers upgraded; leader NOT auto-upgraded.");
    }

    if strict && !result.errors.is_empty() {
        let summary = result
            .errors
            .iter()
            .map(|e| format!("{}: {}", e.node_id, e.message))
            .collect::<Vec<_>>()
            .join("; ");
        anyhow::bail!("Rolling upgrade failed in strict mode: {summary}");
    }

    Ok(())
}

/// GET `<addr>/health/ready` and return true on any 2xx response.
async fn health_ready_ok(client: &reqwest::Client, addr: &str) -> bool {
    let url = format!("http://{addr}/health/ready");
    match client
        .get(&url)
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
    {
        Ok(resp) => resp.status().is_success(),
        Err(_) => false,
    }
}

/// Trigger `POST /api/v1/cluster/upgrade-self` against the leader and wait
/// for the daemon to drop and come back. Returns Ok on success; surfaces a
/// hard error only if the POST fails to deliver — drop/comeback timeouts
/// are logged as warnings (the supervisor may still finish the respawn
/// asynchronously).
async fn upgrade_leader_self(
    client: &reqwest::Client,
    leader_endpoint: &str,
    version: Option<&str>,
) -> Result<()> {
    use std::time::{Duration, Instant};

    info!(%leader_endpoint, "Followers upgraded. Scheduling leader self-upgrade...");
    println!();
    println!("Scheduling leader self-upgrade at {leader_endpoint}...");

    let leader_url = format!("http://{leader_endpoint}/api/v1/cluster/upgrade-self");
    let resp = client
        .post(&leader_url)
        .json(&serde_json::json!({ "version": version }))
        .send()
        .await
        .context("posting /cluster/upgrade-self")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("Leader self-upgrade request failed: {status} - {body}");
    }

    // The leader daemon will now exit code 75. Wait for /health/ready to flap.
    info!("Waiting for leader to drop /health/ready (signal restart)...");
    println!("Waiting for leader to drop /health/ready...");
    let drop_deadline = Instant::now() + Duration::from_secs(60);
    let mut dropped = false;
    while Instant::now() < drop_deadline {
        if !health_ready_ok(client, leader_endpoint).await {
            dropped = true;
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    if !dropped {
        warn!("Leader's /health/ready did not drop within 60s — upgrade may not have started");
    }

    info!("Waiting for leader to come back...");
    println!("Waiting for leader to come back...");
    let back_deadline = Instant::now() + Duration::from_secs(180);
    let mut back = false;
    while Instant::now() < back_deadline {
        if health_ready_ok(client, leader_endpoint).await {
            back = true;
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    if back {
        println!("✓ Leader is back and serving /health/ready.");
    } else {
        warn!(
            "Leader did not return to /health/ready within 180s. The supervisor may still be respawning it — check manually."
        );
        println!(
            "- Leader did not return to /health/ready within 180s. Check the supervisor / logs."
        );
    }

    Ok(())
}

/// Set node resource mode
pub(crate) async fn handle_node_set_mode(
    node_id: String,
    mode: String,
    services: Option<Vec<String>>,
    cli_data_dir: &std::path::Path,
) -> Result<()> {
    use std::time::Duration;

    // Validate mode
    let valid_modes = ["full", "dedicated", "replicate"];
    if !valid_modes.contains(&mode.as_str()) {
        anyhow::bail!(
            "Invalid mode '{}'. Valid modes are: {}",
            mode,
            valid_modes.join(", ")
        );
    }

    // Validate mode-service combination
    if (mode == "dedicated" || mode == "replicate") && services.is_none() {
        anyhow::bail!("Mode '{mode}' requires --services to be specified");
    }

    let data_dir = cli_data_dir.to_path_buf();
    let node_config = load_or_init_node_config(&data_dir).await?;

    let api_endpoint = format!("{}:{}", node_config.advertise_addr, node_config.api_port);

    println!("Setting mode for node '{node_id}'...");

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .context("Failed to create HTTP client")?;

    #[allow(clippy::items_after_statements)]
    #[derive(Serialize)]
    struct SetModeRequest {
        mode: String,
        services: Option<Vec<String>>,
    }

    let request = SetModeRequest {
        mode: mode.clone(),
        services: services.clone(),
    };

    let response = client
        .put(format!(
            "http://{api_endpoint}/api/v1/cluster/nodes/{node_id}/mode"
        ))
        .json(&request)
        .send()
        .await
        .context("Failed to send set-mode request")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("Failed to set node mode: {status} - {body}");
    }

    println!("Node '{node_id}' mode set to '{mode}'.");
    if let Some(svcs) = services {
        println!("Services: {}", svcs.join(", "));
    }

    Ok(())
}

/// Add label to a node
pub(crate) async fn handle_node_label(
    node_id: String,
    label: String,
    cli_data_dir: &std::path::Path,
) -> Result<()> {
    use std::time::Duration;

    // Parse label
    let parts: Vec<&str> = label.splitn(2, '=').collect();
    if parts.len() != 2 {
        anyhow::bail!("Invalid label format '{label}'. Expected key=value format.");
    }
    let (key, value) = (parts[0], parts[1]);

    // Validate label key
    if key.is_empty() {
        anyhow::bail!("Label key cannot be empty");
    }

    let data_dir = cli_data_dir.to_path_buf();
    let node_config = load_or_init_node_config(&data_dir).await?;

    let api_endpoint = format!("{}:{}", node_config.advertise_addr, node_config.api_port);

    println!("Adding label to node '{node_id}'...");

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .context("Failed to create HTTP client")?;

    #[allow(clippy::items_after_statements)]
    #[derive(Serialize)]
    struct AddLabelRequest {
        key: String,
        value: String,
    }

    let request = AddLabelRequest {
        key: key.to_string(),
        value: value.to_string(),
    };

    let response = client
        .post(format!(
            "http://{api_endpoint}/api/v1/cluster/nodes/{node_id}/labels"
        ))
        .json(&request)
        .send()
        .await
        .context("Failed to send label request")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("Failed to add label: {status} - {body}");
    }

    println!("Label '{key}={value}' added to node '{node_id}'.");

    Ok(())
}

/// Force this node to become cluster leader (disaster recovery).
///
/// Sends a POST to the local API server's force-leader endpoint. Use this when
/// the original leader is permanently lost and the surviving learner needs to
/// take over the cluster.
pub(crate) async fn handle_node_force_leader(api_addr: String) -> Result<()> {
    use std::time::Duration;

    println!("WARNING: Force-leader will make this node the new cluster leader.");
    println!("         Only use when the original leader is permanently lost.\n");

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .context("Failed to create HTTP client")?;

    let url = format!("http://{api_addr}/api/v1/cluster/force-leader");
    let resp = client
        .post(&url)
        .json(&serde_json::json!({"confirm": "CONFIRM_FORCE_LEADER"}))
        .send()
        .await
        .context("Failed to connect to API server")?;

    if resp.status().is_success() {
        let body: serde_json::Value = resp
            .json()
            .await
            .context("Failed to parse force-leader response")?;
        println!("Force-leader initiated successfully.");
        if let Some(nodes) = body.get("preserved_nodes") {
            println!(
                "  Preserved {} nodes, {} services",
                nodes,
                body.get("preserved_services")
                    .unwrap_or(&serde_json::Value::Null)
            );
        }
        println!("\nRestart the daemon to complete recovery:");
        println!("  systemctl restart zlayer");
    } else {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("Force-leader failed: {status} - {body}");
    }

    Ok(())
}

/// Rotate the cluster's Ed25519 signing keypair via the local daemon.
///
/// Sends `POST /api/v1/cluster/rotate-signing-key`. The daemon owns the
/// leader-vs-worker decision: workers forward to the current Raft leader,
/// the leader rotates in place. The previously active key is moved into a
/// grace window controlled by `--grace` (default 7d on the server when
/// omitted) so in-flight join tokens minted under the prior key continue
/// to validate until the grace expires.
///
/// Wave 5B.3 wires this handler; the server-side endpoint shipped in
/// Wave 5B.4 (`crates/zlayer-api/src/handlers/cluster.rs`). The same handler
/// services both the dedicated `zlayer cluster rotate-signing-key` invocation
/// and the alias `zlayer node rotate-signing-key`.
pub(crate) async fn handle_node_rotate_signing_key(
    grace: Option<humantime::Duration>,
) -> Result<()> {
    use zlayer_client::DaemonClient;
    use zlayer_types::api::cluster::RotateSigningKeyRequest;

    let req = RotateSigningKeyRequest {
        // Humantime's `Display` impl matches the server-side parser, so a
        // round-trip through `format_duration` keeps the on-wire string in
        // the exact shape the daemon expects.
        grace: grace.map(|d| humantime::format_duration(*d).to_string()),
    };

    let client = DaemonClient::connect()
        .await
        .context("Failed to connect to local zlayer daemon")?;
    let resp = client
        .cluster_rotate_signing_key(&req)
        .await
        .context("calling /api/v1/cluster/rotate-signing-key")?;

    println!("Cluster signing key rotated.");
    println!("  New active kid:        {}", resp.kid);
    println!("  New public key (b64):  {}", resp.public_key_b64);
    println!("  Previous kid (grace):  {}", resp.previous_kid);
    println!("  Grace expires at:      {}", resp.previous_grace_until);
    println!();
    println!("In-flight tokens signed under the previous key will continue to validate");
    println!("until the grace expiration above. After that, they are rejected.");
    Ok(())
}

/// Handler for `zlayer cluster revoke-token <token-or-hash> [--reason ...]`.
///
/// Builds a [`RevokeTokenRequest`] and forwards to the daemon. The daemon
/// hashes the input to its canonical form, proposes a
/// `SecretsRaftOp::RevokeToken`, and the entry replicates to every node
/// via Raft. On success this prints the canonical hash and pruning
/// timestamp so the operator can correlate with subsequent
/// `list-revocations` output.
pub(crate) async fn handle_cluster_revoke_token(
    token_or_hash: String,
    reason: Option<String>,
) -> Result<()> {
    use zlayer_client::DaemonClient;
    use zlayer_types::api::cluster::RevokeTokenRequest;

    let req = RevokeTokenRequest {
        token_or_hash,
        reason,
    };

    let client = DaemonClient::connect()
        .await
        .context("Failed to connect to local zlayer daemon")?;
    let resp = client
        .cluster_revoke_token(&req)
        .await
        .context("calling /api/v1/cluster/revoke-token")?;

    println!("Revoked join token");
    println!("  token_hash: {}", resp.token_hash);
    println!("  expires_at: {}", resp.expires_at);
    println!(
        "Propagated through Raft — every node will reject this hash until \
         the entry is pruned at the listed expiry."
    );
    Ok(())
}

/// Handler for `zlayer cluster list-revocations`.
///
/// Reads the daemon's view of the Raft-replicated revocation list.
/// Sorted soonest-to-prune first by the server; this just renders the
/// list. Empty output ("(no revocations)") indicates the list is clean
/// — either no revocations have ever been issued, or every prior one
/// has already auto-pruned past its natural expiry.
pub(crate) async fn handle_cluster_list_revocations() -> Result<()> {
    use zlayer_client::DaemonClient;

    let client = DaemonClient::connect()
        .await
        .context("Failed to connect to local zlayer daemon")?;
    let resp = client
        .cluster_list_revocations()
        .await
        .context("calling /api/v1/cluster/revocations")?;

    if resp.revocations.is_empty() {
        println!("(no revocations)");
    } else {
        println!(
            "Active token revocations ({} total):",
            resp.revocations.len()
        );
        for entry in &resp.revocations {
            println!("  {}  expires_at={}", entry.token_hash, entry.expires_at);
        }
    }
    Ok(())
}

/// Handler for `zlayer cluster trust-bundle export [--out FILE]`.
pub(crate) async fn handle_cluster_trust_bundle_export(
    out: Option<std::path::PathBuf>,
) -> anyhow::Result<()> {
    use zlayer_client::DaemonClient;

    let client = DaemonClient::connect()
        .await
        .context("Failed to connect to local zlayer daemon")?;
    let bundle = client
        .cluster_export_trust_bundle()
        .await
        .context("fetching /api/v1/cluster/trust-bundle")?;
    let json = serde_json::to_string_pretty(&bundle).context("serializing TrustBundle to JSON")?;
    if let Some(path) = out {
        tokio::fs::write(&path, &json)
            .await
            .with_context(|| format!("writing trust bundle to {}", path.display()))?;
        println!("Wrote trust bundle to {}", path.display());
        println!("  cluster_domain: {}", bundle.cluster_domain);
        println!("  ca_kid:         {}", bundle.ca_kid);
    } else {
        println!("{json}");
    }
    Ok(())
}

/// Handler for `zlayer cluster trust-bundle import <SOURCE> [--source-url URL]`.
pub(crate) async fn handle_cluster_trust_bundle_import(
    source: String,
    source_url: Option<String>,
) -> anyhow::Result<()> {
    use zlayer_client::DaemonClient;

    // Decide whether SOURCE is a URL or a file path. URLs MUST be https://
    // to mitigate trivial MitM of the trust-bundle fetch — the bundle is
    // the very thing that bootstraps trust.
    let (bundle_json, derived_source_url): (String, Option<String>) =
        if source.starts_with("https://") {
            let resp = reqwest::get(&source)
                .await
                .with_context(|| format!("fetching trust bundle from {source}"))?;
            let status = resp.status();
            if !status.is_success() {
                anyhow::bail!("trust bundle URL returned HTTP {status}");
            }
            let text = resp
                .text()
                .await
                .context("reading trust bundle response body")?;
            (text, Some(source.clone()))
        } else if source.starts_with("http://") {
            anyhow::bail!(
            "refusing to fetch trust bundle over plaintext http://; use https:// or a local file"
        );
        } else {
            let path = std::path::Path::new(&source);
            let text = tokio::fs::read_to_string(path)
                .await
                .with_context(|| format!("reading trust bundle file {}", path.display()))?;
            (text, source_url.clone())
        };

    let bundle: zlayer_types::api::cluster::TrustBundle = serde_json::from_str(&bundle_json)
        .with_context(|| format!("parsing TrustBundle JSON from {source}"))?;

    let client = DaemonClient::connect()
        .await
        .context("Failed to connect to local zlayer daemon")?;
    let req = zlayer_types::api::cluster::ImportTrustBundleRequest {
        bundle,
        source_url: derived_source_url.or(source_url),
    };
    let resp = client
        .cluster_import_trust_bundle(&req)
        .await
        .context("POST /api/v1/cluster/trust-imports")?;
    println!("Imported trust bundle");
    println!("  cluster_domain: {}", resp.cluster_domain);
    println!("  ca_kid:         {}", resp.ca_kid);
    println!(
        "Replicated through Raft \u{2014} every node will now accept tokens minted by this cluster"
    );
    Ok(())
}

/// Handler for `zlayer cluster trust-bundle list`.
pub(crate) async fn handle_cluster_trust_bundle_list() -> anyhow::Result<()> {
    use zlayer_client::DaemonClient;

    let client = DaemonClient::connect()
        .await
        .context("Failed to connect to local zlayer daemon")?;
    let resp = client
        .cluster_list_trust_bundles()
        .await
        .context("GET /api/v1/cluster/trust-bundles")?;
    if resp.bundles.is_empty() {
        println!("(no trusted foreign-cluster bundles)");
    } else {
        println!("Trusted foreign-cluster bundles ({}):", resp.bundles.len());
        for b in &resp.bundles {
            println!("  cluster_domain: {}", b.cluster_domain);
            println!("    ca_kid:        {}", b.ca_kid);
            println!("    generated_at:  {}", b.generated_at);
            if let Some(src) = &b.source_url {
                println!("    source_url:    {src}");
            }
        }
    }
    Ok(())
}

/// Handler for `zlayer cluster trust-bundle remove <cluster_domain>`.
pub(crate) async fn handle_cluster_trust_bundle_remove(
    cluster_domain: String,
) -> anyhow::Result<()> {
    use zlayer_client::DaemonClient;

    let client = DaemonClient::connect()
        .await
        .context("Failed to connect to local zlayer daemon")?;
    client
        .cluster_remove_trust_bundle(&cluster_domain)
        .await
        .with_context(|| format!("DELETE /api/v1/cluster/trust-imports/{cluster_domain}"))?;
    println!("Removed trust bundle for cluster_domain: {cluster_domain}");
    Ok(())
}

/// Try to discover the deployment name from a `.zlayer.yml` / `*.zlayer.yml` spec
/// in the current working directory.  Returns `None` if nothing is found.
fn try_discover_deployment_name() -> Option<String> {
    let spec_path = crate::util::discover_spec_path(None).ok()?;
    let spec = crate::util::parse_spec(&spec_path).ok()?;
    Some(spec.deployment)
}

/// Build a `ClusterJoinToken` from this node's on-disk state.
///
/// Reads `node_config.json` (via [`load_or_init_node_config`]) and the
/// `join_secret` file from `data_dir`. Returns an actionable error when the
/// join secret is missing — that file is owned by the daemon and only
/// materializes after `zlayer serve` runs at least once.
///
/// Returns a tuple of (`ClusterJoinToken`, `join_secret`). The token carries
/// the public cluster metadata (endpoint, CIDR, leader pubkey, timestamp);
/// the `join_secret` is the HMAC root used to mint the HS256 JWT alongside
/// the Ed25519-signed token. Wave 6 (v0.13.0) split these because the
/// plaintext format that used to bake the secret INTO the token body was
/// removed — the caller threads `join_secret` directly into
/// `mint_hs256_cluster_join_token` and does not embed it in any output.
///
/// Pulled out as a pure helper so unit tests can construct a token from a
/// scratch data directory and round-trip it through
/// [`parse_cluster_join_token`] without going through the `println!`-bearing
/// CLI handler.
async fn build_cluster_join_token_from_disk(
    data_dir: &Path,
    api_override: Option<String>,
) -> Result<(ClusterJoinToken, String)> {
    let node_config = load_or_init_node_config(data_dir).await?;

    // The API endpoint embedded in the token is a bare `host:port`. When the
    // caller passes an `--api` override it wins verbatim — that mirrors the
    // documented behavior of the `--api` CLI flag.
    let api_endpoint = api_override
        .unwrap_or_else(|| format!("{}:{}", node_config.advertise_addr, node_config.api_port));

    let raft_endpoint = format!("{}:{}", node_config.advertise_addr, node_config.raft_port);

    // The join secret is persisted by the daemon at `data_dir/join_secret`
    // (see `bin/zlayer/src/commands/serve.rs`). If it doesn't exist the user
    // has not yet started the daemon on this node, so there is no cluster
    // secret to mint an HS256 token against — and silently generating one
    // here would diverge from whatever the daemon mints on first boot,
    // breaking every join attempt. Surface a precise, actionable error.
    let join_secret_path = data_dir.join("join_secret");
    let join_secret = match tokio::fs::read_to_string(&join_secret_path).await {
        Ok(s) => s.trim().to_string(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            anyhow::bail!(
                "join secret not found at {}; start the daemon (zlayer serve) at least once on this node before generating a join token",
                join_secret_path.display()
            );
        }
        Err(e) => {
            return Err(e).with_context(|| {
                format!(
                    "Failed to read join secret at {}",
                    join_secret_path.display()
                )
            });
        }
    };

    if join_secret.is_empty() {
        anyhow::bail!(
            "join secret file at {} is empty; remove it and restart the daemon to regenerate it",
            join_secret_path.display()
        );
    }

    Ok((
        ClusterJoinToken {
            api_endpoint,
            raft_endpoint,
            leader_wg_pubkey: node_config.wireguard_public_key,
            overlay_cidr: node_config.overlay_cidr,
            created_at: current_timestamp(),
        },
        join_secret,
    ))
}

/// Handle node generate-join-token command.
///
/// Emits the two modern formats unconditionally:
///   1. **Signed (recommended)** — Wave-3 Ed25519-signed envelope
///      (`SignedClusterJoinToken`). Carries an `exp` claim (`now() + ttl`)
///      and is verified by the leader's `ClusterSigner` public key.
///   2. **HS256 (modern alternative)** — HS256 JWT over an HMAC key derived
///      from the cluster's `join_secret` (matches the
///      `JoinTokenClaims` shape consumed by the leader's
///      `validate_join_token_signed` in `zlayer-api`). Single-use via the
///      embedded `jti`; the same `--ttl` controls its `exp`.
///
/// Wave 6 (v0.13.0): the legacy plaintext token format (`ClusterJoinToken`
/// base64 of JSON with an embedded `auth_secret`) is no longer emitted —
/// servers reject it outright. The `--legacy-plaintext` opt-in flag was
/// also removed in the same wave.
///
/// `deployment` and `service` are no longer load-bearing — cluster joining is
/// per-cluster, not per-deployment — but they are preserved as informational
/// header output so existing invocations keep working without surprising the
/// user. `api` continues to act as an override for the API endpoint embedded
/// in the token; when omitted, the value is reconstructed from the on-disk
/// `node_config.json`. `ttl` controls both the signed envelope's `exp` claim
/// and the HS256 token's `exp`.
pub(crate) async fn handle_node_generate_join_token(
    deployment: Option<String>,
    api: Option<String>,
    service: Option<String>,
    data_dir: PathBuf,
    ttl: humantime::Duration,
) -> Result<()> {
    // --- Resolve the deployment name (informational only) ---
    let deployment_name = match deployment {
        Some(d) => Some(d),
        None => {
            // Try to discover from spec files in the current directory. Unlike
            // the previous implementation we do not fall back to "default" --
            // the deployment field is purely informational now, so leaving it
            // absent is cleaner than fabricating a meaningless placeholder.
            if let Some(name) = try_discover_deployment_name() {
                info!(deployment = %name, "Auto-discovered deployment name from spec");
                Some(name)
            } else {
                None
            }
        }
    };

    // --- Build the underlying token data (also resolves the HMAC root for
    //     the HS256 path). `build_cluster_join_token_from_disk` loads the
    //     cluster's `join_secret` -- that same secret is the HMAC root for
    //     the HS256 token below, so both modern formats describe the same
    //     cluster in lockstep with the Ed25519-signed envelope.
    let (token_data, join_secret) = build_cluster_join_token_from_disk(&data_dir, api).await?;

    // --- Build the Wave-3 signed token ---
    //
    // Reuse the builder's resolved endpoint/CIDR/leader-WG fields so the
    // two tokens describe the same cluster in lockstep. Load (or generate
    // first-time) the signing key from the same on-disk path the daemon uses
    // in `serve.rs`. We deliberately call `load_or_generate` here instead of
    // talking to the daemon: this CLI is typically run on the leader host
    // where the on-disk key is canonical, and it works even when the daemon
    // is offline.
    let signer =
        zlayer_secrets::ClusterSigner::load_or_generate(&data_dir.join("cluster_signing.key"))
            .await
            .context("loading cluster signing key for signed-token mint")?;

    // `iss` is the issuing node's UUID. We don't auto-init the node config
    // here: if it's missing, the builder above would already have failed,
    // so by this point we know `load_node_config` succeeds.
    let node_config = load_node_config(&data_dir).await?;

    let now = chrono::Utc::now();
    let exp = now
        + chrono::Duration::from_std(ttl.into())
            .context("converting --ttl into chrono::Duration (overflow — try a shorter ttl)")?;
    let claims = ClusterJoinClaims {
        api_endpoint: token_data.api_endpoint.clone(),
        raft_endpoint: token_data.raft_endpoint.clone(),
        leader_wg_pubkey: token_data.leader_wg_pubkey.clone(),
        overlay_cidr: token_data.overlay_cidr.clone(),
        exp: exp.to_rfc3339(),
        iat: now.to_rfc3339(),
        iss: node_config.node_id.clone(),
    };
    let signed_token = mint_signed_cluster_join_token(&claims, &signer, None)
        .context("minting signed cluster join token")?;

    // --- Build the HS256 JWT (modern alternative) ---
    //
    // The leader's `validate_join_token_signed` (in
    // `crates/zlayer-api/src/handlers/cluster.rs`) accepts HS256 JWTs whose
    // claims are `{ iss: "zlayer-cluster", aud: "cluster-join", jti, exp,
    // iat }`, signed with `SHA256("zlayer-join-token-v1\0" || join_secret)`
    // as the HMAC key. We mirror that derivation here so the CLI doesn't
    // need a daemon round-trip.
    //
    // `chrono::DateTime::timestamp()` returns `i64` (Unix seconds, signed
    // to allow pre-epoch dates). Both values come from `Utc::now()` plus a
    // non-negative `chrono::Duration`, so they're always >= 0 here, but we
    // round-trip through `u64::try_from` to keep clippy::cast_sign_loss
    // happy and to be defensive about a future code change to `now`'s
    // source.
    let iat_u64 = u64::try_from(now.timestamp())
        .context("Unix timestamp for HS256 `iat` claim was negative; system clock is pre-1970")?;
    let exp_u64 = u64::try_from(exp.timestamp()).context(
        "Unix timestamp for HS256 `exp` claim was negative; --ttl underflowed the epoch",
    )?;
    let hs256_token = mint_hs256_cluster_join_token(&join_secret, iat_u64, exp_u64)
        .context("minting HS256 cluster join token")?;

    // --- Output ---
    println!("Join Token Generated");
    println!("====================\n");
    if let Some(d) = &deployment_name {
        println!("Deployment (informational): {d}");
    }
    println!("API: {}", token_data.api_endpoint);
    println!("Raft: {}", token_data.raft_endpoint);
    if let Some(svc) = &service {
        println!("Service (informational): {svc}");
    }

    println!(
        "\nSigned token (recommended, expires {}):",
        exp.to_rfc3339()
    );
    println!("  kid: {}", signer.key_id());
    println!("  {signed_token}");
    println!();
    println!("HS256 token (modern alternative):");
    println!("  {hs256_token}");

    println!("\nUsage:");
    println!("  zlayer node join <leader-addr> --token {signed_token}");

    Ok(())
}

/// Mint an HS256 cluster-join JWT matching the shape accepted by the leader's
/// `validate_join_token_signed` validator.
///
/// Claims layout (must stay in lockstep with `JoinTokenClaims` in
/// `crates/zlayer-api/src/handlers/cluster.rs`):
/// * `iss` = `"zlayer-cluster"`
/// * `aud` = `"cluster-join"`
/// * `jti` = freshly minted UUID v4 (single-use, recorded in the leader's
///   `used_jtis` after first redemption)
/// * `iat`, `exp` = Unix seconds
///
/// HMAC key derivation matches `derive_join_hmac_key`:
/// `SHA256("zlayer-join-token-v1\0" || join_secret)`. Keeping the derivation
/// inline (rather than reaching into `zlayer-api` for a public helper)
/// avoids pulling the API crate's full transitive graph into the CLI just
/// for token minting; the two implementations are guarded by the unit tests
/// below and the leader-side validation tests in `cluster.rs`.
fn mint_hs256_cluster_join_token(
    join_secret: &str,
    iat_secs: u64,
    exp_secs: u64,
) -> Result<String> {
    use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
    use sha2::{Digest, Sha256};

    #[derive(Serialize)]
    struct Hs256JoinClaims {
        iss: String,
        aud: String,
        jti: String,
        exp: u64,
        iat: u64,
    }

    let hmac_key = {
        let mut hasher = Sha256::new();
        hasher.update(b"zlayer-join-token-v1\0");
        hasher.update(join_secret.as_bytes());
        let out: [u8; 32] = hasher.finalize().into();
        out
    };

    let claims = Hs256JoinClaims {
        iss: "zlayer-cluster".to_string(),
        aud: "cluster-join".to_string(),
        jti: uuid::Uuid::new_v4().to_string(),
        exp: exp_secs,
        iat: iat_secs,
    };

    encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(&hmac_key),
    )
    .context("encoding HS256 cluster-join JWT")
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use crate::cli::{Cli, Commands, NodeCommands};
    use clap::Parser;
    use std::path::PathBuf;
    use zlayer_paths::ZLayerDirs;

    #[test]
    fn test_cli_node_init_command() {
        let cli = Cli::try_parse_from(["zlayer", "node", "init", "--advertise-addr", "10.0.0.1"])
            .unwrap();

        match cli.command {
            Some(Commands::Node(NodeCommands::Init {
                advertise_addr,
                api_port,
                raft_port,
                overlay_port,
                data_dir,
                overlay_cidr,
                ..
            })) => {
                assert_eq!(advertise_addr, "10.0.0.1");
                assert_eq!(api_port, 3669);
                assert_eq!(raft_port, 9000);
                assert_eq!(overlay_port, zlayer_core::DEFAULT_WG_PORT);
                assert!(data_dir.is_none()); // resolved at runtime
                assert_eq!(overlay_cidr, "10.200.0.0/16");
            }
            _ => panic!("Expected Node Init command"),
        }
    }

    #[test]
    fn test_cli_node_init_command_all_options() {
        let cli = Cli::try_parse_from([
            "zlayer",
            "node",
            "init",
            "--advertise-addr",
            "192.168.1.100",
            "--api-port",
            "9090",
            "--raft-port",
            "9001",
            "--overlay-port",
            "51821",
            "--data-dir",
            "/custom/data",
            "--overlay-cidr",
            "10.100.0.0/16",
        ])
        .unwrap();

        match cli.command {
            Some(Commands::Node(NodeCommands::Init {
                advertise_addr,
                api_port,
                raft_port,
                overlay_port,
                data_dir,
                overlay_cidr,
                ..
            })) => {
                assert_eq!(advertise_addr, "192.168.1.100");
                assert_eq!(api_port, 9090);
                assert_eq!(raft_port, 9001);
                assert_eq!(overlay_port, 51821);
                assert_eq!(data_dir, Some(PathBuf::from("/custom/data")));
                assert_eq!(overlay_cidr, "10.100.0.0/16");
            }
            _ => panic!("Expected Node Init command"),
        }
    }

    #[test]
    fn test_cli_node_join_command() {
        let cli = Cli::try_parse_from([
            "zlayer",
            "node",
            "join",
            "10.0.0.1:3669",
            "--token",
            "abc123",
            "--advertise-addr",
            "10.0.0.2",
        ])
        .unwrap();

        match cli.command {
            Some(Commands::Node(NodeCommands::Join {
                leader_addr,
                token,
                advertise_addr,
                mode,
                services,
                ..
            })) => {
                assert_eq!(leader_addr, "10.0.0.1:3669");
                assert_eq!(token, "abc123");
                assert_eq!(advertise_addr, "10.0.0.2");
                assert_eq!(mode, "full");
                assert!(services.is_none());
            }
            _ => panic!("Expected Node Join command"),
        }
    }

    #[test]
    fn test_cli_node_join_command_with_services() {
        let cli = Cli::try_parse_from([
            "zlayer",
            "node",
            "join",
            "10.0.0.1:3669",
            "--token",
            "abc123",
            "--advertise-addr",
            "10.0.0.2",
            "--mode",
            "replicate",
            "--services",
            "api",
            "--services",
            "web",
        ])
        .unwrap();

        match cli.command {
            Some(Commands::Node(NodeCommands::Join { mode, services, .. })) => {
                assert_eq!(mode, "replicate");
                assert_eq!(services, Some(vec!["api".to_string(), "web".to_string()]));
            }
            _ => panic!("Expected Node Join command"),
        }
    }

    #[test]
    fn test_cli_node_generate_join_token_default_ttl_is_24h() {
        // Wave 3.3: `--ttl` defaults to `24h` (humantime), so omitting the flag
        // should parse out a Duration equal to 24 hours.
        let cli = Cli::try_parse_from(["zlayer", "node", "generate-join-token"]).unwrap();

        match cli.command {
            Some(Commands::Node(NodeCommands::GenerateJoinToken { ttl, .. })) => {
                let expected = std::time::Duration::from_secs(24 * 60 * 60);
                assert_eq!(
                    std::time::Duration::from(ttl),
                    expected,
                    "default --ttl must be 24h"
                );
            }
            _ => panic!("Expected Node GenerateJoinToken command"),
        }
    }

    #[test]
    fn test_cli_node_generate_join_token_custom_ttl_parses() {
        // Wave 3.3: `--ttl 1h` should propagate end-to-end into the parsed
        // value (later mapped onto the signed token's `exp` claim).
        let cli =
            Cli::try_parse_from(["zlayer", "node", "generate-join-token", "--ttl", "1h"]).unwrap();

        match cli.command {
            Some(Commands::Node(NodeCommands::GenerateJoinToken { ttl, .. })) => {
                assert_eq!(
                    std::time::Duration::from(ttl),
                    std::time::Duration::from_secs(3600),
                    "--ttl 1h must parse to one hour"
                );
            }
            _ => panic!("Expected Node GenerateJoinToken command"),
        }
    }

    #[test]
    fn generate_join_token_rejects_legacy_plaintext_flag() {
        // Wave 6 (v0.13.0): the `--legacy-plaintext` opt-in (Wave 4.1) is
        // gone. Clap must hard-reject it as an unknown flag so operators
        // calling old playbooks see a clear failure instead of silently
        // missing the deprecated emergency-rollback form they expected.
        let result = Cli::try_parse_from([
            "zlayer",
            "node",
            "generate-join-token",
            "--legacy-plaintext",
        ]);
        assert!(
            result.is_err(),
            "expected --legacy-plaintext to be rejected after Wave 6 removal"
        );
    }

    #[test]
    fn test_cli_node_list_command() {
        let cli = Cli::try_parse_from(["zlayer", "node", "list"]).unwrap();

        match cli.command {
            Some(Commands::Node(NodeCommands::List { output })) => {
                assert_eq!(output, "table");
            }
            _ => panic!("Expected Node List command"),
        }
    }

    #[test]
    fn test_cli_node_list_command_json() {
        let cli = Cli::try_parse_from(["zlayer", "node", "list", "--output", "json"]).unwrap();

        match cli.command {
            Some(Commands::Node(NodeCommands::List { output })) => {
                assert_eq!(output, "json");
            }
            _ => panic!("Expected Node List command"),
        }
    }

    #[test]
    fn test_cli_node_status_command() {
        let cli = Cli::try_parse_from(["zlayer", "node", "status"]).unwrap();

        match cli.command {
            Some(Commands::Node(NodeCommands::Status { node_id })) => {
                assert!(node_id.is_none());
            }
            _ => panic!("Expected Node Status command"),
        }
    }

    #[test]
    fn test_cli_node_status_command_with_id() {
        let cli = Cli::try_parse_from(["zlayer", "node", "status", "node-abc-123"]).unwrap();

        match cli.command {
            Some(Commands::Node(NodeCommands::Status { node_id })) => {
                assert_eq!(node_id, Some("node-abc-123".to_string()));
            }
            _ => panic!("Expected Node Status command"),
        }
    }

    #[test]
    fn test_cli_node_remove_command() {
        let cli = Cli::try_parse_from(["zlayer", "node", "remove", "node-123"]).unwrap();

        match cli.command {
            Some(Commands::Node(NodeCommands::Remove { node_id, force })) => {
                assert_eq!(node_id, "node-123");
                assert!(!force);
            }
            _ => panic!("Expected Node Remove command"),
        }
    }

    #[test]
    fn test_cli_node_remove_command_force() {
        let cli = Cli::try_parse_from(["zlayer", "node", "remove", "--force", "node-123"]).unwrap();

        match cli.command {
            Some(Commands::Node(NodeCommands::Remove { node_id, force })) => {
                assert_eq!(node_id, "node-123");
                assert!(force);
            }
            _ => panic!("Expected Node Remove command"),
        }
    }

    #[test]
    fn test_cli_node_set_mode_command() {
        let cli = Cli::try_parse_from([
            "zlayer",
            "node",
            "set-mode",
            "node-123",
            "--mode",
            "dedicated",
            "--services",
            "api",
        ])
        .unwrap();

        match cli.command {
            Some(Commands::Node(NodeCommands::SetMode {
                node_id,
                mode,
                services,
            })) => {
                assert_eq!(node_id, "node-123");
                assert_eq!(mode, "dedicated");
                assert_eq!(services, Some(vec!["api".to_string()]));
            }
            _ => panic!("Expected Node SetMode command"),
        }
    }

    #[test]
    fn test_cli_node_label_command() {
        let cli = Cli::try_parse_from([
            "zlayer",
            "node",
            "label",
            "node-123",
            "environment=production",
        ])
        .unwrap();

        match cli.command {
            Some(Commands::Node(NodeCommands::Label { node_id, label })) => {
                assert_eq!(node_id, "node-123");
                assert_eq!(label, "environment=production");
            }
            _ => panic!("Expected Node Label command"),
        }
    }

    #[test]
    fn test_generate_secure_token() {
        let token1 = super::generate_secure_token();
        let token2 = super::generate_secure_token();

        // Tokens should be different
        assert_ne!(token1, token2);

        // Tokens should be base64 encoded (43 chars for 32 bytes URL-safe no padding)
        assert_eq!(token1.len(), 43);
        assert_eq!(token2.len(), 43);
    }

    #[test]
    fn test_generate_node_id() {
        let id1 = super::generate_node_id();
        let id2 = super::generate_node_id();

        // IDs should be different
        assert_ne!(id1, id2);

        // IDs should be valid UUIDs (36 chars with dashes)
        assert_eq!(id1.len(), 36);
        assert_eq!(id2.len(), 36);
    }

    #[test]
    fn test_join_token_roundtrip() {
        let token = super::generate_join_token_data(
            "192.168.1.1",
            3669,
            9000,
            "test-public-key",
            "10.200.0.0/16",
        )
        .unwrap();

        let parsed = super::parse_cluster_join_token(&token).unwrap();

        assert_eq!(parsed.api_endpoint, "192.168.1.1:3669");
        assert_eq!(parsed.raft_endpoint, "192.168.1.1:9000");
        assert_eq!(parsed.leader_wg_pubkey, "test-public-key");
        assert_eq!(parsed.overlay_cidr, "10.200.0.0/16");
        assert!(!parsed.created_at.is_empty());
    }

    #[test]
    fn test_parse_invalid_join_token() {
        // Invalid base64
        let result = super::parse_cluster_join_token("not-valid-base64!!!");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("base64"));

        // Valid base64 but invalid JSON
        let invalid_json = base64::Engine::encode(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD,
            "not json",
        );
        let result = super::parse_cluster_join_token(&invalid_json);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("JSON"));
    }

    /// On a non-admin Windows shell, `handle_node_init` should refuse to do any
    /// privileged work and surface an actionable "Run as Administrator" error
    /// **before** touching the filesystem, network, or subprocess APIs. The
    /// test executes only when the current process is not elevated so it never
    /// interferes with a genuine init on a developer machine.
    ///
    /// Also validates that the `#[cfg(windows)]` body compiles with the new
    /// `ConsentMode` parameter — a regression guard for the H-1 wiring.
    #[cfg(windows)]
    #[tokio::test]
    async fn handle_node_init_non_admin_errors_early() {
        // Skip if the test process happens to be running elevated — the
        // admin-check would then fall through and attempt real side effects.
        if zlayer_paths::is_root() {
            eprintln!("test skipped: running as Administrator");
            return;
        }
        let tmp = ZLayerDirs::system_default()
            .scratch_dir("handle-node-init-non-admin-errors-early-")
            .expect("tempdir");
        let err = super::handle_node_init(
            "127.0.0.1".to_string(),
            3669,
            9000,
            zlayer_core::DEFAULT_WG_PORT,
            tmp.path().to_path_buf(),
            "10.200.0.0/16".to_string(),
            crate::ui::consent::ConsentMode::No,
        )
        .await
        .expect_err("non-admin init must error");

        let msg = format!("{err:#}");
        assert!(
            msg.contains("Administrator"),
            "error should cite Administrator: {msg}"
        );
        // Filesystem must be untouched — no partial state written.
        assert!(
            !tmp.path().join("node_config.json").exists(),
            "node_config.json must not be written before admin check"
        );
    }

    /// Companion to `handle_node_init_non_admin_errors_early` for the join
    /// path. Validates the same admin precedence guard on H-2.
    #[cfg(windows)]
    #[tokio::test]
    async fn handle_node_join_non_admin_errors_early() {
        if zlayer_paths::is_root() {
            eprintln!("test skipped: running as Administrator");
            return;
        }
        let tmp = ZLayerDirs::system_default()
            .scratch_dir("handle-node-join-non-admin-errors-early-")
            .expect("tempdir");
        let err = super::handle_node_join(
            "127.0.0.1:3669".to_string(),
            "dummy-token".to_string(),
            "127.0.0.1".to_string(),
            "full".to_string(),
            None,
            3669,
            9000,
            zlayer_core::DEFAULT_WG_PORT,
            tmp.path().to_path_buf(),
            crate::ui::consent::ConsentMode::No,
        )
        .await
        .expect_err("non-admin join must error");

        let msg = format!("{err:#}");
        assert!(
            msg.contains("Administrator"),
            "error should cite Administrator: {msg}"
        );
        assert!(
            !tmp.path().join("node_config.json").exists(),
            "node_config.json must not be written before admin check"
        );
    }

    /// `write_secure` must round-trip exactly the bytes it is given. This is
    /// the contract relied on by `persist_secrets_join_material` for the
    /// node JWT (UTF-8) and the wrapped DEK (binary).
    #[test]
    fn write_secure_round_trip() {
        let tmp = ZLayerDirs::system_default()
            .scratch_dir("write-secure-round-trip-")
            .expect("tempdir");
        let path = tmp.path().join("secret.bin");

        // Use a payload with explicit non-UTF8 bytes (0xFF, 0x00) to prove
        // the helper does not corrupt binary content.
        let payload: &[u8] = &[0x00, 0x01, 0xFE, 0xFF, b'h', b'i'];
        super::write_secure(&path, payload).expect("write_secure");

        let read_back = std::fs::read(&path).expect("read back");
        assert_eq!(read_back, payload);
    }

    /// On Unix, `write_secure` must create the file with mode 0600 so that
    /// a misconfigured umask cannot widen permissions. This is the same
    /// invariant that `zlayer_secrets::load_or_generate_node_keypair`
    /// enforces for `node_secrets.key`.
    #[cfg(unix)]
    #[test]
    fn write_secure_sets_mode_0600_on_unix() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = ZLayerDirs::system_default()
            .scratch_dir("write-secure-sets-mode-0600-on-unix-")
            .expect("tempdir");
        let path = tmp.path().join("secret.bin");

        super::write_secure(&path, b"sensitive").expect("write_secure");

        let mode = std::fs::metadata(&path).expect("stat").permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "expected mode 0600, got {mode:o}");
    }

    /// `handle_node_generate_join_token` must mint a token that survives a
    /// round trip through `parse_cluster_join_token` (the same parser
    /// `zlayer node join` runs). This is a regression test for a bug where
    /// the generator produced a 3-field `{deployment, api_endpoint, service}`
    /// JSON blob instead of the 6-field `ClusterJoinToken` the parser
    /// expects — every join attempt failed with
    /// "Invalid join token: not valid JSON: missing field `raft_endpoint`".
    #[tokio::test]
    async fn cluster_join_token_built_from_disk_round_trips_through_parse() {
        use base64::Engine;

        let tmp = ZLayerDirs::system_default()
            .scratch_dir("cluster-join-token-round-trip-")
            .expect("tempdir");
        let data_dir = tmp.path().to_path_buf();

        // Seed a minimal node_config.json that mirrors the shape
        // `save_node_config` writes -- pretty-printed JSON with every field
        // populated. We use values that are easy to recognize in the
        // assertions below.
        let node_config = super::NodeConfig {
            node_id: "node-test-uuid".to_string(),
            raft_node_id: 1,
            advertise_addr: "10.0.0.7".to_string(),
            api_port: 3669,
            raft_port: 9000,
            overlay_port: zlayer_core::DEFAULT_WG_PORT,
            overlay_cidr: "10.200.0.0/16".to_string(),
            wireguard_private_key: "test-priv-key-base64".to_string(),
            wireguard_public_key: "test-pub-key-base64".to_string(),
            is_leader: true,
            created_at: super::current_timestamp(),
        };
        super::save_node_config(&data_dir, &node_config)
            .await
            .expect("save_node_config");

        // Seed the join secret the way the daemon does (plain text, no
        // trailing newline -- matches `commands/serve.rs:~1429`).
        let join_secret = "deadbeefcafef00d".to_string();
        std::fs::write(data_dir.join("join_secret"), &join_secret).expect("write join_secret");

        // Build the token directly via the pure helper and assert each field
        // matches what we seeded. Wave 6 (v0.13.0): the helper now returns
        // (token, join_secret) — the secret is no longer baked into the
        // token body but is still threaded out separately to feed the
        // HS256 mint path.
        let (built, built_secret) = super::build_cluster_join_token_from_disk(&data_dir, None)
            .await
            .expect("build_cluster_join_token_from_disk");

        assert_eq!(built.api_endpoint, "10.0.0.7:3669");
        assert_eq!(built.raft_endpoint, "10.0.0.7:9000");
        assert_eq!(built.leader_wg_pubkey, "test-pub-key-base64");
        assert_eq!(built.overlay_cidr, "10.200.0.0/16");
        assert_eq!(built_secret, join_secret);
        assert!(!built.created_at.is_empty(), "created_at must be populated");

        // Encode the same way the handler does and re-parse it through the
        // exact function `node join` calls. This is the load-bearing
        // assertion -- if `parse_cluster_join_token` rejects the output
        // (e.g. because a required field is missing) the bug is back.
        let json = serde_json::to_string(&built).expect("serialize token");
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json.as_bytes());
        let parsed = super::parse_cluster_join_token(&encoded)
            .expect("parse_cluster_join_token must accept the freshly minted token");

        assert_eq!(parsed.api_endpoint, built.api_endpoint);
        assert_eq!(parsed.raft_endpoint, built.raft_endpoint);
        assert_eq!(parsed.leader_wg_pubkey, built.leader_wg_pubkey);
        assert_eq!(parsed.overlay_cidr, built.overlay_cidr);
        assert_eq!(parsed.created_at, built.created_at);

        // The `api` override must win verbatim when supplied -- mirrors the
        // documented behaviour of the `--api` CLI flag.
        let (override_built, _) = super::build_cluster_join_token_from_disk(
            &data_dir,
            Some("api.example.com:8443".to_string()),
        )
        .await
        .expect("build_cluster_join_token_from_disk with override");
        assert_eq!(override_built.api_endpoint, "api.example.com:8443");
        // Raft endpoint is still sourced from node_config -- the override
        // applies only to the API surface.
        assert_eq!(override_built.raft_endpoint, "10.0.0.7:9000");
    }

    /// Missing `join_secret` file must produce an actionable error rather than
    /// silently fabricating a fresh secret (which would never match what the
    /// daemon eventually mints on first boot).
    #[tokio::test]
    async fn cluster_join_token_errors_when_join_secret_missing() {
        let tmp = ZLayerDirs::system_default()
            .scratch_dir("cluster-join-token-missing-secret-")
            .expect("tempdir");
        let data_dir = tmp.path().to_path_buf();

        let node_config = super::NodeConfig {
            node_id: "node-test-uuid".to_string(),
            raft_node_id: 1,
            advertise_addr: "10.0.0.7".to_string(),
            api_port: 3669,
            raft_port: 9000,
            overlay_port: zlayer_core::DEFAULT_WG_PORT,
            overlay_cidr: "10.200.0.0/16".to_string(),
            wireguard_private_key: "test-priv-key-base64".to_string(),
            wireguard_public_key: "test-pub-key-base64".to_string(),
            is_leader: true,
            created_at: super::current_timestamp(),
        };
        super::save_node_config(&data_dir, &node_config)
            .await
            .expect("save_node_config");

        let err = super::build_cluster_join_token_from_disk(&data_dir, None)
            .await
            .expect_err("must fail when join_secret is absent");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("join secret not found"),
            "error must mention the missing secret: {msg}"
        );
        assert!(
            msg.contains("zlayer serve"),
            "error must point the user at the daemon: {msg}"
        );
    }

    /// `write_secure` must overwrite an existing file's content **and** ensure
    /// its mode is reset to 0600 — covering the case where a pre-existing file
    /// at the path was created with a more permissive mode (`mode_t` passed
    /// to `OpenOptions::mode` is only honored on file creation, not truncation).
    #[cfg(unix)]
    #[test]
    fn write_secure_overwrite_resets_mode_0600_on_unix() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = ZLayerDirs::system_default()
            .scratch_dir("write-secure-overwrite-resets-mode-0600-on-unix-")
            .expect("tempdir");
        let path = tmp.path().join("secret.bin");

        // Pre-create with a wide-open mode and unrelated content.
        std::fs::write(&path, b"old").expect("seed file");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))
            .expect("seed perms");

        super::write_secure(&path, b"new").expect("write_secure overwrite");

        // Content was replaced.
        assert_eq!(std::fs::read(&path).expect("read"), b"new");

        // Mode was tightened back down to 0600 by the belt-and-suspenders
        // `set_permissions` call inside `write_secure`.
        let mode = std::fs::metadata(&path).expect("stat").permissions().mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "overwrite must reset mode to 0600, got {mode:o}"
        );
    }
}

// =============================================================================
// Signed cluster join token tests (Wave 3.2)
// =============================================================================

#[cfg(test)]
mod signed_token_tests {
    use super::{
        mint_signed_cluster_join_token, parse_signed_cluster_join_token,
        verify_signed_cluster_join_token, ClusterJoinClaims, SignedClusterJoinToken,
        SIGNED_TOKEN_V_WAVE3, STANDARD, URL_SAFE_NO_PAD,
    };
    use base64::Engine as _;
    use zlayer_types::api::cluster::SIGNED_TOKEN_V_WAVE9;

    fn sample_claims() -> ClusterJoinClaims {
        sample_claims_with_exp(chrono::Utc::now() + chrono::Duration::hours(1))
    }

    fn sample_claims_with_exp(exp: chrono::DateTime<chrono::Utc>) -> ClusterJoinClaims {
        ClusterJoinClaims {
            api_endpoint: "https://leader.example.com:3669".to_string(),
            raft_endpoint: "10.0.0.1:9000".to_string(),
            leader_wg_pubkey: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=".to_string(),
            overlay_cidr: "10.200.0.0/16".to_string(),
            exp: exp.to_rfc3339(),
            iat: chrono::Utc::now().to_rfc3339(),
            iss: "node-uuid-aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".to_string(),
        }
    }

    #[test]
    fn mint_then_parse_round_trip() {
        let signer = zlayer_secrets::ClusterSigner::generate();
        let claims = sample_claims();
        let token_str = mint_signed_cluster_join_token(&claims, &signer, None).unwrap();
        let parsed = parse_signed_cluster_join_token(&token_str).unwrap();
        assert_eq!(parsed.v, SIGNED_TOKEN_V_WAVE3);
        assert_eq!(parsed.kid, signer.key_id());
        assert_eq!(parsed.claims.api_endpoint, claims.api_endpoint);
        assert_eq!(parsed.claims.raft_endpoint, claims.raft_endpoint);
        assert_eq!(parsed.claims.leader_wg_pubkey, claims.leader_wg_pubkey);
        assert_eq!(parsed.claims.overlay_cidr, claims.overlay_cidr);
        assert_eq!(parsed.claims.exp, claims.exp);
        assert_eq!(parsed.claims.iat, claims.iat);
        assert_eq!(parsed.claims.iss, claims.iss);
    }

    #[test]
    fn minted_token_is_single_line_ascii() {
        let signer = zlayer_secrets::ClusterSigner::generate();
        let claims = sample_claims();
        let token_str = mint_signed_cluster_join_token(&claims, &signer, None).unwrap();
        assert!(
            token_str
                .chars()
                .all(|c| c.is_ascii() && !c.is_whitespace()),
            "minted token must be pure ASCII with no whitespace"
        );
    }

    #[test]
    fn verify_accepts_freshly_minted_token() {
        let signer = zlayer_secrets::ClusterSigner::generate();
        let claims = sample_claims_with_exp(chrono::Utc::now() + chrono::Duration::hours(1));
        let token_str = mint_signed_cluster_join_token(&claims, &signer, None).unwrap();
        let parsed = parse_signed_cluster_join_token(&token_str).unwrap();
        let verified =
            verify_signed_cluster_join_token(&parsed, &signer.verifying_key(), &signer.key_id())
                .expect("freshly-minted token must verify against its signer");
        assert_eq!(verified.api_endpoint, claims.api_endpoint);
        assert_eq!(verified.iss, claims.iss);
    }

    #[test]
    fn verify_rejects_when_kid_mismatches() {
        let signer = zlayer_secrets::ClusterSigner::generate();
        let claims = sample_claims();
        let token_str = mint_signed_cluster_join_token(&claims, &signer, None).unwrap();
        let parsed = parse_signed_cluster_join_token(&token_str).unwrap();

        let err = verify_signed_cluster_join_token(&parsed, &signer.verifying_key(), "deadbeef")
            .expect_err("kid mismatch must fail verification");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("kid mismatch"),
            "error must mention kid mismatch: {msg}"
        );
    }

    #[test]
    fn verify_rejects_when_signature_tampered() {
        let signer = zlayer_secrets::ClusterSigner::generate();
        let claims = sample_claims();
        let token_str = mint_signed_cluster_join_token(&claims, &signer, None).unwrap();
        let mut parsed = parse_signed_cluster_join_token(&token_str).unwrap();

        // Flip a byte in the signature (the signature is base64 of 64 bytes).
        let mut sig_bytes = URL_SAFE_NO_PAD.decode(&parsed.sig).unwrap();
        sig_bytes[0] ^= 0x01;
        parsed.sig = URL_SAFE_NO_PAD.encode(&sig_bytes);

        let err =
            verify_signed_cluster_join_token(&parsed, &signer.verifying_key(), &signer.key_id())
                .expect_err("tampered signature must fail verification");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("Ed25519 signature verification failed"),
            "error must mention signature failure: {msg}"
        );
    }

    #[test]
    fn verify_rejects_when_claims_tampered() {
        let signer = zlayer_secrets::ClusterSigner::generate();
        let claims = sample_claims();
        let token_str = mint_signed_cluster_join_token(&claims, &signer, None).unwrap();
        let mut parsed = parse_signed_cluster_join_token(&token_str).unwrap();

        // Modify the api_endpoint after minting — signature should no longer
        // be valid because it was computed over the original claims bytes.
        parsed.claims.api_endpoint = "https://attacker.example.com:3669".to_string();

        let err =
            verify_signed_cluster_join_token(&parsed, &signer.verifying_key(), &signer.key_id())
                .expect_err("tampered claims must fail verification");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("Ed25519 signature verification failed"),
            "error must mention signature failure: {msg}"
        );
    }

    #[test]
    fn verify_rejects_when_expired() {
        let signer = zlayer_secrets::ClusterSigner::generate();
        let claims = sample_claims_with_exp(chrono::Utc::now() - chrono::Duration::hours(1));
        let token_str = mint_signed_cluster_join_token(&claims, &signer, None).unwrap();
        let parsed = parse_signed_cluster_join_token(&token_str).unwrap();

        let err =
            verify_signed_cluster_join_token(&parsed, &signer.verifying_key(), &signer.key_id())
                .expect_err("expired token must fail verification");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("expired"),
            "error must mention expiration: {msg}"
        );
    }

    #[test]
    fn parse_rejects_unsupported_version() {
        let signer = zlayer_secrets::ClusterSigner::generate();
        let claims = sample_claims();
        // Hand-craft an envelope with v=99.
        let claims_bytes = serde_json::to_vec(&claims).unwrap();
        let sig_bytes = signer.sign(&claims_bytes);
        let envelope = SignedClusterJoinToken {
            v: 99,
            kid: signer.key_id(),
            claims: claims.clone(),
            sig: URL_SAFE_NO_PAD.encode(sig_bytes),
            ca_chain: None,
        };
        let envelope_bytes = serde_json::to_vec(&envelope).unwrap();
        let token_str = URL_SAFE_NO_PAD.encode(envelope_bytes);

        let err = parse_signed_cluster_join_token(&token_str).expect_err("v=99 must be rejected");
        let msg = format!("{err:#}");
        assert!(msg.contains("version"), "error must mention version: {msg}");
    }

    #[test]
    fn parse_rejects_garbage_base64() {
        let err = parse_signed_cluster_join_token("not-valid-base64!!!!")
            .expect_err("garbage must be rejected");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("base64"),
            "error must mention base64 decoding: {msg}"
        );
    }

    #[test]
    fn mint_propagates_ttl_into_exp_claim() {
        // Wave 3.3: when the CLI handler builds claims as
        // `exp = now() + ttl`, round-tripping through mint/parse should
        // preserve the gap. We exercise that directly with a 1h ttl.
        let signer = zlayer_secrets::ClusterSigner::generate();
        let ttl = chrono::Duration::hours(1);
        let now = chrono::Utc::now();
        let exp = now + ttl;
        let claims = ClusterJoinClaims {
            api_endpoint: "https://leader.example.com:3669".to_string(),
            raft_endpoint: "10.0.0.1:9000".to_string(),
            leader_wg_pubkey: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=".to_string(),
            overlay_cidr: "10.200.0.0/16".to_string(),
            exp: exp.to_rfc3339(),
            iat: now.to_rfc3339(),
            iss: "node-uuid-aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".to_string(),
        };
        let token_str = mint_signed_cluster_join_token(&claims, &signer, None).unwrap();
        let parsed = parse_signed_cluster_join_token(&token_str).unwrap();

        let parsed_iat = chrono::DateTime::parse_from_rfc3339(&parsed.claims.iat).unwrap();
        let parsed_exp = chrono::DateTime::parse_from_rfc3339(&parsed.claims.exp).unwrap();
        let gap = parsed_exp - parsed_iat;
        assert_eq!(
            gap, ttl,
            "exp - iat must equal the ttl that was used to build the claims"
        );
    }

    #[test]
    fn parse_accepts_standard_base64_too() {
        // Mint a token, then re-encode the envelope using STANDARD base64
        // instead of URL_SAFE_NO_PAD to confirm parse handles both.
        let signer = zlayer_secrets::ClusterSigner::generate();
        let claims = sample_claims();
        let token_str = mint_signed_cluster_join_token(&claims, &signer, None).unwrap();
        // Decode with URL_SAFE_NO_PAD, re-encode with STANDARD.
        let envelope_bytes = URL_SAFE_NO_PAD.decode(token_str.as_bytes()).unwrap();
        let standard_token = STANDARD.encode(&envelope_bytes);
        let parsed = parse_signed_cluster_join_token(&standard_token)
            .expect("STANDARD-encoded envelope must parse");
        assert_eq!(parsed.kid, signer.key_id());
    }

    /// Wave 9D-iii: when no CA context is supplied, the mint MUST
    /// emit a v=1 envelope with `ca_chain: None` — wire-compatible
    /// with every pre-Wave-9 validator.
    #[test]
    fn mint_signed_cluster_join_token_v1_when_ca_context_absent() {
        let signer = zlayer_secrets::ClusterSigner::generate();
        let claims = ClusterJoinClaims {
            api_endpoint: "http://127.0.0.1:3669".into(),
            raft_endpoint: "127.0.0.1:3670".into(),
            leader_wg_pubkey: "ignored-pubkey".into(),
            overlay_cidr: "10.42.0.0/16".into(),
            exp: (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339(),
            iat: chrono::Utc::now().to_rfc3339(),
            iss: "test-node".into(),
        };
        let token = mint_signed_cluster_join_token(&claims, &signer, None).unwrap();
        let bytes = URL_SAFE_NO_PAD.decode(&token).unwrap();
        let parsed: SignedClusterJoinToken = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(parsed.v, SIGNED_TOKEN_V_WAVE3);
        assert!(parsed.ca_chain.is_none(), "v=1 mints must omit ca_chain");
    }

    /// Wave 9D-iii: when a CA + `cluster_domain` + cert-validity are
    /// supplied, the mint MUST emit a v=2 envelope whose `ca_chain`
    /// is a fresh `CaCert` signed by the CA, with `active_kid` /
    /// `active_pubkey_b64` matching the signer.
    #[tokio::test]
    async fn mint_signed_cluster_join_token_v2_when_ca_context_present() {
        let dir = tempfile::tempdir().unwrap();
        let signer = zlayer_secrets::ClusterSigner::generate();
        let ca = zlayer_secrets::ClusterCa::load_or_generate(&dir.path().join("ca.key"))
            .await
            .unwrap();
        let claims = ClusterJoinClaims {
            api_endpoint: "http://127.0.0.1:3669".into(),
            raft_endpoint: "127.0.0.1:3670".into(),
            leader_wg_pubkey: "ignored-pubkey".into(),
            overlay_cidr: "10.42.0.0/16".into(),
            exp: (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339(),
            iat: chrono::Utc::now().to_rfc3339(),
            iss: "test-node".into(),
        };
        let token = mint_signed_cluster_join_token(
            &claims,
            &signer,
            Some((
                &ca,
                "test-cluster-domain",
                std::time::Duration::from_secs(3600),
            )),
        )
        .unwrap();
        let bytes = URL_SAFE_NO_PAD.decode(&token).unwrap();
        let parsed: SignedClusterJoinToken = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(parsed.v, SIGNED_TOKEN_V_WAVE9);
        let cert = parsed.ca_chain.expect("v=2 mints must include ca_chain");
        assert_eq!(cert.cluster_domain, "test-cluster-domain");
        assert_eq!(cert.active_kid, signer.key_id());

        // CA-signed cert must verify under its own CA pubkey.
        zlayer_secrets::ClusterCa::verify_ca_cert(&ca.ca_public_key_b64(), &cert).unwrap();
    }
}
