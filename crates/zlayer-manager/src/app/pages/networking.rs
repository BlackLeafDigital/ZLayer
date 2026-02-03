//! Networking page
//!
//! Main networking overview displaying overlay status, tunnels, and connected nodes.

use leptos::prelude::*;
use leptos_router::components::A;

use crate::app::components::StatsCard;

/// Networking overview page component
///
/// Displays networking stats and links to overlay and SSH tunnels subpages.
#[component]
pub fn Networking() -> impl IntoView {
    // Mock data for stats
    let overlay_status = "Active";
    let active_tunnels = 5;
    let connected_nodes = 3;

    view! {
        <div class="container mx-auto p-6">
            <h1 class="text-3xl font-bold mb-6">"Networking"</h1>

            // Stats overview
            <div class="stats stats-vertical lg:stats-horizontal shadow w-full mb-8">
                <StatsCard
                    title="Overlay Status".to_string()
                    value=overlay_status.to_string()
                    icon=r#"<svg xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" class="inline-block w-8 h-8 stroke-current"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M13 10V3L4 14h7v7l9-11h-7z"></path></svg>"#.to_string()
                />
                <StatsCard
                    title="Active Tunnels".to_string()
                    value=active_tunnels.to_string()
                    icon=r#"<svg xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" class="inline-block w-8 h-8 stroke-current"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 6V4m0 2a2 2 0 100 4m0-4a2 2 0 110 4m-6 8a2 2 0 100-4m0 4a2 2 0 110-4m0 4v2m0-6V4m6 6v10m6-2a2 2 0 100-4m0 4a2 2 0 110-4m0 4v2m0-6V4"></path></svg>"#.to_string()
                />
                <StatsCard
                    title="Connected Nodes".to_string()
                    value=connected_nodes.to_string()
                    icon=r#"<svg xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" class="inline-block w-8 h-8 stroke-current"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M5 12h14M5 12a2 2 0 01-2-2V6a2 2 0 012-2h14a2 2 0 012 2v4a2 2 0 01-2 2M5 12a2 2 0 00-2 2v4a2 2 0 002 2h14a2 2 0 002-2v-4a2 2 0 00-2-2m-2-4h.01M17 16h.01"></path></svg>"#.to_string()
                />
            </div>

            // Navigation cards
            <div class="grid grid-cols-1 md:grid-cols-2 gap-6">
                // Overlay card
                <div class="card bg-base-100 shadow-xl">
                    <div class="card-body">
                        <h2 class="card-title">"Overlay Network"</h2>
                        <p>"Manage the WireGuard-based overlay network, peer connections, and routing."</p>
                        <div class="card-actions justify-end">
                            <A href="/overlay" attr:class="btn btn-primary">
                                "Manage Overlay"
                            </A>
                        </div>
                    </div>
                </div>

                // SSH Tunnels card
                <div class="card bg-base-100 shadow-xl">
                    <div class="card-body">
                        <h2 class="card-title">"SSH Tunnels"</h2>
                        <p>"Configure and monitor SSH tunnels for secure remote access and port forwarding."</p>
                        <div class="card-actions justify-end">
                            <A href="/ssh-tunnels" attr:class="btn btn-primary">
                                "Manage Tunnels"
                            </A>
                        </div>
                    </div>
                </div>
            </div>
        </div>
    }
}
