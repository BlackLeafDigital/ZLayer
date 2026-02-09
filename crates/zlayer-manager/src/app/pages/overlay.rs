//! Overlay Network page
use crate::app::server_fns::{
    get_dns_status, get_ip_allocation, get_overlay_peers, get_overlay_status, OverlayPeer,
};
use leptos::prelude::*;

/// Truncate public key for display
fn truncate_key(key: &str) -> String {
    if key.len() > 16 {
        format!("{}...{}", &key[..8], &key[key.len() - 4..])
    } else {
        key.to_string()
    }
}

/// SAFETY: allocated_count won't exceed u32::MAX in practice (max IP allocations in a subnet)
#[allow(clippy::cast_possible_truncation)]
#[component]
fn IpAllocationCard() -> impl IntoView {
    let ip_alloc = Resource::new(|| (), |()| get_ip_allocation());

    view! {
        <div class="card bg-base-200 shadow-md">
            <div class="card-body">
                <h2 class="card-title text-lg">"IP Allocation"</h2>
                <Suspense fallback=move || view! { <span class="loading loading-spinner"></span> }>
                    {move || ip_alloc.get().map(|result| match result {
                        Ok(alloc) => {
                            // SAFETY: allocated_count won't exceed u32::MAX in practice
                            #[allow(clippy::cast_possible_truncation)]
                            let allocated_u32 = alloc.allocated_count as u32;
                            view! {
                                <div class="space-y-2">
                                    <div class="flex justify-between">
                                        <span class="opacity-70">"Subnet"</span>
                                        <span class="font-mono">{alloc.cidr.clone()}</span>
                                    </div>
                                    <div class="flex justify-between">
                                        <span class="opacity-70">"Allocated"</span>
                                        <span>{format!("{} / {}", alloc.allocated_count, alloc.total_ips)}</span>
                                    </div>
                                    <div class="flex justify-between">
                                        <span class="opacity-70">"Available"</span>
                                        <span class="text-success">{alloc.available_count}</span>
                                    </div>
                                    <progress class="progress progress-primary w-full"
                                        value=allocated_u32
                                        max=alloc.total_ips>
                                    </progress>
                                </div>
                            }.into_any()
                        }
                        Err(e) => view! { <div class="text-error">{e.to_string()}</div> }.into_any(),
                    })}
                </Suspense>
            </div>
        </div>
    }
}

#[component]
fn DnsDiscoveryCard() -> impl IntoView {
    let dns = Resource::new(|| (), |()| get_dns_status());

    view! {
        <div class="card bg-base-200 shadow-md">
            <div class="card-body">
                <h2 class="card-title text-lg">"DNS Discovery"</h2>
                <Suspense fallback=move || view! { <span class="loading loading-spinner"></span> }>
                    {move || dns.get().map(|result| match result {
                        Ok(d) => view! {
                            <div class="space-y-2">
                                <div class="flex justify-between">
                                    <span class="opacity-70">"Status"</span>
                                    {if d.enabled {
                                        view! { <span class="badge badge-success">"Enabled"</span> }.into_any()
                                    } else {
                                        view! { <span class="badge badge-ghost">"Disabled"</span> }.into_any()
                                    }}
                                </div>
                                <div class="flex justify-between">
                                    <span class="opacity-70">"Domain"</span>
                                    <span class="font-mono">{d.zone.clone().unwrap_or_else(|| "-".to_string())}</span>
                                </div>
                                <div class="flex justify-between">
                                    <span class="opacity-70">"Records"</span>
                                    <span>{d.service_count}</span>
                                </div>
                            </div>
                        }.into_any(),
                        Err(e) => view! { <div class="text-error">{e.to_string()}</div> }.into_any(),
                    })}
                </Suspense>
            </div>
        </div>
    }
}

fn render_peers_table(peers: Vec<OverlayPeer>) -> impl IntoView {
    view! {
        <div class="overflow-x-auto">
            <table class="table table-zebra w-full">
                <thead>
                    <tr>
                        <th>"IP"</th>
                        <th>"Public Key"</th>
                        <th>"Status"</th>
                        <th>"Last Handshake"</th>
                        <th>"Ping"</th>
                    </tr>
                </thead>
                <tbody>
                    {peers.into_iter().map(|peer| {
                        let status_class = if peer.healthy { "badge badge-success" } else { "badge badge-error" };
                        let status_text = if peer.healthy { "Connected" } else { "Disconnected" };
                        let truncated_key = truncate_key(&peer.public_key);
                        let handshake = peer.last_handshake_secs.map_or_else(|| "-".to_string(), |s| format!("{s}s ago"));
                        let ping = peer.last_ping_ms.map_or_else(|| "-".to_string(), |ms| format!("{ms}ms"));
                        view! {
                            <tr>
                                <td class="font-mono">{peer.overlay_ip.clone().unwrap_or_else(|| "-".to_string())}</td>
                                <td class="font-mono text-sm" title=peer.public_key.clone()>{truncated_key}</td>
                                <td><span class=status_class>{status_text}</span></td>
                                <td class="opacity-70">{handshake}</td>
                                <td class="opacity-70">{ping}</td>
                            </tr>
                        }
                    }).collect::<Vec<_>>()}
                </tbody>
            </table>
        </div>
    }
}

#[component]
fn PeersCard() -> impl IntoView {
    let peers = Resource::new(|| (), |()| get_overlay_peers());

    view! {
        <div class="card bg-base-200 shadow-md">
            <div class="card-body">
                <h2 class="card-title">"Mesh Peers"</h2>
                <Suspense fallback=move || view! { <span class="loading loading-spinner loading-lg"></span> }>
                    <ErrorBoundary fallback=|errors| view! { <div class="alert alert-error">{move || format!("{:?}", errors.get())}</div> }>
                        {move || peers.get().map(|result| match result {
                            Ok(peer_list) => {
                                if peer_list.is_empty() {
                                    view! { <div class="alert alert-info">"No peers found."</div> }.into_any()
                                } else {
                                    render_peers_table(peer_list).into_any()
                                }
                            }
                            Err(e) => view! { <div class="alert alert-error">{e.to_string()}</div> }.into_any(),
                        })}
                    </ErrorBoundary>
                </Suspense>
            </div>
        </div>
    }
}

#[component]
pub fn Overlay() -> impl IntoView {
    let status = Resource::new(|| (), |()| get_overlay_status());

    view! {
        <div class="p-6">
            <h1 class="text-3xl font-bold mb-6">"Overlay Network"</h1>

            // Overlay Mesh Status Card
            <div class="card bg-base-200 shadow-md mb-6">
                <div class="card-body">
                    <h2 class="card-title">"Overlay Mesh Status"</h2>
                    <Suspense fallback=move || view! { <span class="loading loading-spinner"></span> }>
                        {move || status.get().map(|result| match result {
                            Ok(s) => view! {
                                <div class="flex items-center gap-4">
                                    <div class="badge badge-success badge-lg">"Active"</div>
                                    <span class="text-sm opacity-70">
                                        {format!("{} / {} peers connected", s.healthy_peers, s.total_peers)}
                                    </span>
                                    <span class="font-mono text-sm">{s.node_ip}</span>
                                </div>
                            }.into_any(),
                            Err(e) => view! {
                                <div class="flex items-center gap-4">
                                    <div class="badge badge-error badge-lg">"Unavailable"</div>
                                    <span class="text-sm opacity-70">{e.to_string()}</span>
                                </div>
                            }.into_any(),
                        })}
                    </Suspense>
                </div>
            </div>

            // Info Cards Row
            <div class="grid grid-cols-1 md:grid-cols-2 gap-4 mb-6">
                <IpAllocationCard />
                <DnsDiscoveryCard />
            </div>

            <PeersCard />
        </div>
    }
}
