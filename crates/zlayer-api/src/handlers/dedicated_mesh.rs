//! Cross-node peer distribution for [`OverlayMode::Dedicated`] service overlays.
//!
//! A `Dedicated` service stands up its own per-service `WireGuard` transport on
//! every node that hosts it. Those per-service devices are isolated from the
//! cluster (`Global`) transport, so the global join-broadcast does NOT make them
//! peer with each other. This module ties the pieces together:
//!
//! 1. **Publish** the local node's per-service endpoint into Raft
//!    ([`Request::SetServiceOverlayEndpoint`]) so every replica converges on the
//!    `(node, service) -> endpoint` mapping.
//! 2. **Learn** every other hosting node's published endpoint from Raft and add
//!    each as a service-scoped `WireGuard` peer
//!    ([`OverlayManager::add_service_peer`]).
//! 3. **Notify** the other hosting nodes (fire-and-forget HTTP POST to their
//!    `/api/v1/internal/add-peer` with the service fields populated) so they add
//!    THIS node as a peer immediately, rather than waiting for their next
//!    deploy/restore reconcile.
//!
//! Teardown ([`remove_dedicated_service_endpoint`]) is the inverse: it removes
//! the endpoint from Raft and broadcasts a scoped removal so the other nodes
//! drop this node as a service peer.
//!
//! Everything here is **best-effort / non-fatal** — a transient failure to
//! publish, learn, or notify never fails the deploy. The deploy/restore loop
//! re-runs [`distribute_dedicated_service`] on every pass, which doubles as the
//! reconcile backstop that self-heals dropped peers (see step 4 in the task).

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;
use tracing::{info, warn};

use zlayer_agent::OverlayManager;
use zlayer_scheduler::raft::{Request, ServiceOverlayEndpoint};
use zlayer_scheduler::RaftCoordinator;
use zlayer_types::overlay::OverlayMode;
use zlayer_types::overlayd::ServiceOverlayInfo;

use crate::handlers::internal::INTERNAL_AUTH_HEADER;
use zlayer_types::api::internal::InternalAddPeerRequest;

/// Keepalive used for service-scoped peers, matching the global add-peer path
/// (`internal::add_peer_internal` uses 25s) so dedicated transports behave
/// consistently with the cluster transport behind NAT.
const SERVICE_PEER_KEEPALIVE: Duration = Duration::from_secs(25);

/// Publish + mesh the local node's `Dedicated` per-service overlay across the
/// cluster.
///
/// No-op for non-`Dedicated` modes or when the dedicated device did not report
/// a `wg_public_key` (`Shared` mode, or a server that hasn't switched Dedicated
/// setup over yet). All work is best-effort: failures are logged at WARN and
/// never propagated, because the deploy path treats overlay distribution as
/// non-fatal and the next deploy/restore pass re-reconciles.
#[allow(clippy::too_many_lines)]
pub async fn distribute_dedicated_service(
    raft: &Arc<RaftCoordinator>,
    overlay: &Arc<RwLock<OverlayManager>>,
    internal_token: &str,
    advertise_addr: &str,
    service: &str,
    info: &ServiceOverlayInfo,
) {
    if info.mode != OverlayMode::Dedicated {
        return;
    }

    // Dedicated devices that didn't report their crypto identity can't be
    // meshed; bail (Shared-shaped response, or legacy overlayd).
    let (Some(wg_public_key), Some(wg_port), Some(overlay_ip), Some(subnet)) = (
        info.wg_public_key.as_deref(),
        info.wg_port,
        info.overlay_ip,
        info.subnet.as_deref(),
    ) else {
        warn!(
            service = %service,
            "dedicated service overlay reported no WireGuard identity; skipping cross-node mesh"
        );
        return;
    };

    let local_node_id = raft.node_id();
    let local_endpoint = format!("{advertise_addr}:{wg_port}");

    // (a) Publish our endpoint into Raft so every node learns about us.
    let endpoint = ServiceOverlayEndpoint {
        node_id: local_node_id,
        service: service.to_string(),
        wg_public_key: wg_public_key.to_string(),
        endpoint: local_endpoint.clone(),
        overlay_ip: overlay_ip.to_string(),
        subnet: subnet.to_string(),
    };
    match raft
        .propose(Request::SetServiceOverlayEndpoint { endpoint })
        .await
    {
        Ok(_) => info!(
            service = %service,
            node_id = local_node_id,
            endpoint = %local_endpoint,
            "published dedicated service overlay endpoint to raft"
        ),
        Err(e) => warn!(
            service = %service,
            node_id = local_node_id,
            error = %e,
            "failed to publish dedicated service overlay endpoint to raft (non-fatal); \
             will retry on next deploy/restore"
        ),
    }

    let cluster_state = raft.read_state().await;

    // (b) Learn the existing peers from Raft and add each as a service-scoped
    //     WireGuard peer on our dedicated device.
    {
        let guard = overlay.read().await;
        for ep in cluster_state.service_overlay_endpoints_for(service) {
            if ep.node_id == local_node_id {
                continue;
            }
            let parsed: std::net::SocketAddr = match ep.endpoint.parse() {
                Ok(sa) => sa,
                Err(e) => {
                    warn!(
                        service = %service,
                        peer_node = ep.node_id,
                        endpoint = %ep.endpoint,
                        error = %e,
                        "skipping service peer with unparseable endpoint"
                    );
                    continue;
                }
            };
            let peer_info = zlayer_overlay::PeerInfo::new(
                ep.wg_public_key.clone(),
                parsed,
                &ep.subnet,
                SERVICE_PEER_KEEPALIVE,
            );
            if let Err(e) = guard
                .add_service_peer(service, &peer_info, &ep.subnet)
                .await
            {
                warn!(
                    service = %service,
                    peer_node = ep.node_id,
                    error = %e,
                    "failed to add dedicated service peer (non-fatal); will retry on next \
                     deploy/restore"
                );
            } else {
                info!(
                    service = %service,
                    peer_node = ep.node_id,
                    endpoint = %ep.endpoint,
                    "added dedicated service peer from raft endpoint"
                );
            }
        }
    }

    // (c) Notify the other hosting nodes so THEY add THIS node as a service
    //     peer immediately (fire-and-forget, mirroring the cluster-join
    //     broadcast in `cluster.rs`).
    let targets = hosting_node_api_urls(&cluster_state, service, local_node_id);
    if !targets.is_empty() {
        let add_request = InternalAddPeerRequest {
            wg_public_key: wg_public_key.to_string(),
            overlay_ip: overlay_ip.to_string(),
            endpoint: local_endpoint,
            service: Some(service.to_string()),
            service_subnet: Some(subnet.to_string()),
        };
        let token = internal_token.to_string();
        let service = service.to_string();
        tokio::spawn(async move {
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap_or_default();
            for base_url in &targets {
                let url = format!("{base_url}/api/v1/internal/add-peer");
                match client
                    .post(&url)
                    .header(INTERNAL_AUTH_HEADER, &token)
                    .json(&add_request)
                    .send()
                    .await
                {
                    Ok(resp) if resp.status().is_success() => {
                        info!(service = %service, peer_target = %base_url, "broadcast dedicated service peer to hosting node");
                    }
                    Ok(resp) => {
                        warn!(service = %service, peer_target = %base_url, status = %resp.status(), "failed to broadcast dedicated service peer to hosting node");
                    }
                    Err(e) => {
                        warn!(service = %service, peer_target = %base_url, error = %e, "failed to reach hosting node for dedicated service peer broadcast");
                    }
                }
            }
        });
    }
}

/// Tear down the local node's `Dedicated` per-service endpoint across the
/// cluster: remove it from Raft and broadcast a scoped removal so the other
/// hosting nodes drop this node as a service peer.
///
/// The local per-service device's `wg_public_key` (which peers need to know
/// which peer to drop) is read back from the node's own published Raft
/// endpoint. If no endpoint was published (the service was never a meshed
/// `Dedicated` overlay on this node) this is a no-op — there is nothing to
/// remove. Best-effort / non-fatal throughout.
pub async fn remove_dedicated_service_endpoint(
    raft: &Arc<RaftCoordinator>,
    internal_token: &str,
    service: &str,
) {
    let local_node_id = raft.node_id();

    // Snapshot the hosting set + our own published pubkey BEFORE removing the
    // endpoint so the broadcast still reaches every peer and carries the right
    // key.
    let cluster_state = raft.read_state().await;
    let local_key = ServiceOverlayEndpoint::map_key(local_node_id, service);
    let Some(wg_public_key) = cluster_state
        .service_overlay_endpoints
        .get(&local_key)
        .map(|ep| ep.wg_public_key.clone())
    else {
        // Nothing published for (this node, service) — not a meshed Dedicated
        // overlay here; nothing to tear down.
        return;
    };
    let targets = hosting_node_api_urls(&cluster_state, service, local_node_id);

    match raft
        .propose(Request::RemoveServiceOverlayEndpoint {
            node_id: local_node_id,
            service: service.to_string(),
        })
        .await
    {
        Ok(_) => info!(
            service = %service,
            node_id = local_node_id,
            "removed dedicated service overlay endpoint from raft"
        ),
        Err(e) => warn!(
            service = %service,
            node_id = local_node_id,
            error = %e,
            "failed to remove dedicated service overlay endpoint from raft (non-fatal)"
        ),
    }

    if targets.is_empty() {
        return;
    }
    let remove_request = InternalRemovePeerRequest {
        wg_public_key,
        service: service.to_string(),
    };
    let token = internal_token.to_string();
    let service = service.to_string();
    tokio::spawn(async move {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap_or_default();
        for base_url in &targets {
            let url = format!("{base_url}/api/v1/internal/remove-peer");
            match client
                .post(&url)
                .header(INTERNAL_AUTH_HEADER, &token)
                .json(&remove_request)
                .send()
                .await
            {
                Ok(resp) if resp.status().is_success() => {
                    info!(service = %service, peer_target = %base_url, "broadcast dedicated service peer removal to hosting node");
                }
                Ok(resp) => {
                    warn!(service = %service, peer_target = %base_url, status = %resp.status(), "failed to broadcast dedicated service peer removal to hosting node");
                }
                Err(e) => {
                    warn!(service = %service, peer_target = %base_url, error = %e, "failed to reach hosting node for dedicated service peer removal");
                }
            }
        }
    });
}

/// Request body for `POST /api/v1/internal/remove-peer` — the scoped removal
/// analog of [`InternalAddPeerRequest`]. Kept local to `zlayer-api` (rather than
/// in `zlayer-types`) because, unlike add-peer, no out-of-crate caller needs to
/// describe this shape; the only producer is
/// [`remove_dedicated_service_endpoint`] and the only consumer is the internal
/// handler.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
pub struct InternalRemovePeerRequest {
    /// base64 `WireGuard` public key of the peer to remove.
    pub wg_public_key: String,
    /// Service whose dedicated overlay the peer belongs to.
    pub service: String,
}

/// Collect the `http://{advertise_addr}:{api_port}` base URLs of every node
/// hosting `service`, excluding `exclude_node` (the local node) and nodes with
/// no usable advertise address / API port.
fn hosting_node_api_urls(
    cluster_state: &zlayer_scheduler::raft::ClusterState,
    service: &str,
    exclude_node: u64,
) -> Vec<String> {
    let hosting: std::collections::HashSet<u64> =
        cluster_state.nodes_hosting(service).into_iter().collect();
    cluster_state
        .nodes
        .values()
        .filter(|n| n.node_id != exclude_node && hosting.contains(&n.node_id))
        .filter(|n| !n.advertise_addr.is_empty() && n.api_port > 0)
        .map(|n| format!("http://{}:{}", n.advertise_addr, n.api_port))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remove_peer_request_round_trips() {
        let req = InternalRemovePeerRequest {
            wg_public_key: "pubkey==".into(),
            service: "web".into(),
        };
        let json = serde_json::to_string(&req).unwrap();
        let back: InternalRemovePeerRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back.wg_public_key, "pubkey==");
        assert_eq!(back.service, "web");
    }
}
