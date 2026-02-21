//! Dashboard page
//!
//! Main dashboard showing system overview, active deployments, and services.

use leptos::prelude::*;
use leptos_router::components::A;

use crate::app::server_fns::{
    get_deployments, get_overlay_status, get_services, get_system_stats, Deployment, Service,
};

/// Format uptime seconds into human-readable string
fn format_uptime(secs: u64) -> String {
    let days = secs / 86400;
    let hours = (secs % 86400) / 3600;
    let minutes = (secs % 3600) / 60;

    if days > 0 {
        format!("{days}d {hours}h {minutes}m")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{minutes}m")
    }
}

/// Dashboard page component
#[component]
#[allow(clippy::too_many_lines)]
pub fn Dashboard() -> impl IntoView {
    let stats = Resource::new(|| (), |()| get_system_stats());
    let deployments = Resource::new(|| (), |()| get_deployments());
    let overlay = Resource::new(|| (), |()| get_overlay_status());

    view! {
        <div class="space-y-6">
            <h1 class="text-2xl font-bold">"Dashboard"</h1>

            // === Row 1: Stats Cards ===
            <Suspense fallback=move || view! { <div class="skeleton h-24 w-full"></div> }>
                {move || {
                    stats
                        .get()
                        .map(|result| {
                            match result {
                                Ok(s) => {
                                    view! {
                                        <div class="stats shadow bg-base-200 w-full">
                                            <div class="stat">
                                                <div class="stat-figure text-primary">
                                                    <svg xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" stroke-width="1.5" stroke="currentColor" class="w-8 h-8">
                                                        <path stroke-linecap="round" stroke-linejoin="round" d="M20.25 7.5l-.625 10.632a2.25 2.25 0 01-2.247 2.118H6.622a2.25 2.25 0 01-2.247-2.118L3.75 7.5M10 11.25h4M3.375 7.5h17.25c.621 0 1.125-.504 1.125-1.125v-1.5c0-.621-.504-1.125-1.125-1.125H3.375c-.621 0-1.125.504-1.125 1.125v1.5c0 .621.504 1.125 1.125 1.125z" />
                                                    </svg>
                                                </div>
                                                <div class="stat-title">"Deployments"</div>
                                                <div class="stat-value text-primary">{s.total_deployments}</div>
                                                <div class="stat-desc">{format!("{} active", s.active_deployments)}</div>
                                            </div>
                                            <div class="stat">
                                                <div class="stat-figure text-success">
                                                    <svg xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" stroke-width="1.5" stroke="currentColor" class="w-8 h-8">
                                                        <path stroke-linecap="round" stroke-linejoin="round" d="M9 12.75L11.25 15 15 9.75M21 12a9 9 0 11-18 0 9 9 0 0118 0z" />
                                                    </svg>
                                                </div>
                                                <div class="stat-title">"Active"</div>
                                                <div class="stat-value text-success">{s.active_deployments}</div>
                                                <div class="stat-desc">"services running"</div>
                                            </div>
                                            <div class="stat">
                                                <div class="stat-figure text-info">
                                                    <svg xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" stroke-width="1.5" stroke="currentColor" class="w-8 h-8">
                                                        <path stroke-linecap="round" stroke-linejoin="round" d="M9 17.25v1.007a3 3 0 01-.879 2.122L7.5 21h9l-.621-.621A3 3 0 0115 18.257V17.25m6-12V15a2.25 2.25 0 01-2.25 2.25H5.25A2.25 2.25 0 013 15V5.25m18 0A2.25 2.25 0 0018.75 3H5.25A2.25 2.25 0 003 5.25m18 0V12a2.25 2.25 0 01-2.25 2.25H5.25A2.25 2.25 0 013 12V5.25" />
                                                    </svg>
                                                </div>
                                                <div class="stat-title">"Version"</div>
                                                <div class="stat-value text-info text-lg">{s.version.clone()}</div>
                                                <div class="stat-desc">"mac-sandbox runtime"</div>
                                            </div>
                                            <div class="stat">
                                                <div class="stat-title">"Uptime"</div>
                                                <div class="stat-value text-lg">{format_uptime(s.uptime_seconds)}</div>
                                                <div class="stat-desc">"since last restart"</div>
                                            </div>
                                        </div>
                                    }
                                        .into_any()
                                }
                                Err(e) => {
                                    view! {
                                        <div class="alert alert-error">
                                            <span>{format!("Failed to load stats: {e}")}</span>
                                        </div>
                                    }
                                        .into_any()
                                }
                            }
                        })
                }}
            </Suspense>

            // === Row 2: Deployments + Network (side by side on desktop) ===
            <div class="grid grid-cols-1 lg:grid-cols-3 gap-6">
                // Active Deployments (takes 2/3)
                <div class="lg:col-span-2">
                    <div class="card bg-base-200 shadow">
                        <div class="card-body">
                            <div class="flex justify-between items-center">
                                <h2 class="card-title">"Active Deployments"</h2>
                                <A href="/deployments" attr:class="btn btn-ghost btn-sm">"View All"</A>
                            </div>
                            <Suspense fallback=move || view! {
                                <div class="flex justify-center p-4">
                                    <span class="loading loading-spinner"></span>
                                </div>
                            }>
                                {move || {
                                    deployments
                                        .get()
                                        .map(|result| {
                                            match result {
                                                Ok(deps) => render_deployment_list(deps).into_any(),
                                                Err(e) => {
                                                    view! {
                                                        <div class="alert alert-error alert-sm">
                                                            <span>{e.to_string()}</span>
                                                        </div>
                                                    }
                                                        .into_any()
                                                }
                                            }
                                        })
                                }}
                            </Suspense>
                        </div>
                    </div>
                </div>

                // Network Status (1/3)
                <div>
                    <div class="card bg-base-200 shadow h-full">
                        <div class="card-body">
                            <h2 class="card-title">"Network"</h2>
                            <Suspense fallback=move || view! {
                                <div class="flex justify-center p-4">
                                    <span class="loading loading-spinner"></span>
                                </div>
                            }>
                                {move || {
                                    overlay
                                        .get()
                                        .map(|result| {
                                            match result {
                                                Ok(net_status) => {
                                                    view! {
                                                        <div class="space-y-3">
                                                            <div class="flex justify-between">
                                                                <span class="text-sm text-base-content/70">"Interface"</span>
                                                                <span class="font-mono text-sm">{net_status.interface}</span>
                                                            </div>
                                                            <div class="flex justify-between">
                                                                <span class="text-sm text-base-content/70">"Node IP"</span>
                                                                <span class="font-mono text-sm">{net_status.node_ip}</span>
                                                            </div>
                                                            <div class="flex justify-between">
                                                                <span class="text-sm text-base-content/70">"CIDR"</span>
                                                                <span class="font-mono text-sm">{net_status.cidr}</span>
                                                            </div>
                                                            <div class="divider my-1"></div>
                                                            <div class="flex justify-between items-center">
                                                                <span class="text-sm text-base-content/70">"Peers"</span>
                                                                <span class="font-semibold">{format!("{}/{}", net_status.healthy_peers, net_status.total_peers)}</span>
                                                            </div>
                                                            <div class="flex justify-between items-center">
                                                                <span class="text-sm text-base-content/70">"Role"</span>
                                                                {if net_status.is_leader {
                                                                    view! { <span class="badge badge-primary badge-sm">"Leader"</span> }.into_any()
                                                                } else {
                                                                    view! { <span class="badge badge-ghost badge-sm">"Follower"</span> }.into_any()
                                                                }}
                                                            </div>
                                                            <A href="/overlay" attr:class="btn btn-ghost btn-sm btn-block mt-2">"View Overlay Details"</A>
                                                        </div>
                                                    }
                                                        .into_any()
                                                }
                                                Err(_) => {
                                                    view! {
                                                        <div class="text-center py-4">
                                                            <div class="badge badge-ghost">"Overlay inactive"</div>
                                                            <p class="text-xs text-base-content/50 mt-2">"No overlay network configured"</p>
                                                        </div>
                                                    }
                                                        .into_any()
                                                }
                                            }
                                        })
                                }}
                            </Suspense>
                        </div>
                    </div>
                </div>
            </div>

            // === Row 3: Services for each deployment ===
            <Suspense fallback=|| ()>
                {move || {
                    deployments
                        .get()
                        .map(|result| {
                            if let Ok(deps) = result {
                                let running: Vec<_> = deps
                                    .into_iter()
                                    .filter(|d| d.status == "running")
                                    .collect();
                                if running.is_empty() {
                                    view! { <div></div> }.into_any()
                                } else {
                                    view! {
                                        <div class="card bg-base-200 shadow">
                                            <div class="card-body">
                                                <h2 class="card-title">"Running Services"</h2>
                                                <div class="space-y-4">
                                                    {running
                                                        .into_iter()
                                                        .map(|dep| {
                                                            let dep_name = dep.name.clone();
                                                            view! { <DeploymentServices name=dep_name /> }
                                                        })
                                                        .collect::<Vec<_>>()}
                                                </div>
                                            </div>
                                        </div>
                                    }
                                        .into_any()
                                }
                            } else {
                                view! { <div></div> }.into_any()
                            }
                        })
                }}
            </Suspense>
        </div>
    }
}

/// Render deployment list for the dashboard card
fn render_deployment_list(deployments: Vec<Deployment>) -> impl IntoView {
    if deployments.is_empty() {
        return view! {
            <div class="text-center py-6">
                <p class="text-base-content/50">"No deployments yet"</p>
                <A href="/deployments" attr:class="btn btn-primary btn-sm mt-3">"Create Deployment"</A>
            </div>
        }
        .into_any();
    }

    view! {
        <div class="overflow-x-auto">
            <table class="table table-sm">
                <thead>
                    <tr>
                        <th>"Name"</th>
                        <th>"Status"</th>
                        <th>"Replicas"</th>
                        <th>"Created"</th>
                    </tr>
                </thead>
                <tbody>
                    {deployments
                        .into_iter()
                        .map(|d| {
                            let status_class = match d.status.as_str() {
                                "running" => "badge badge-success badge-sm",
                                "pending" | "deploying" => "badge badge-warning badge-sm",
                                "failed" | "error" => "badge badge-error badge-sm",
                                _ => "badge badge-ghost badge-sm",
                            };
                            view! {
                                <tr class="hover">
                                    <td>
                                        <A href="/deployments" attr:class="font-medium hover:text-primary">
                                            {d.name}
                                        </A>
                                    </td>
                                    <td><span class=status_class>{d.status}</span></td>
                                    <td class="font-mono text-sm">{format!("{}/{}", d.replicas, d.target_replicas)}</td>
                                    <td class="text-xs text-base-content/60">{d.created_at}</td>
                                </tr>
                            }
                        })
                        .collect::<Vec<_>>()}
                </tbody>
            </table>
        </div>
    }
    .into_any()
}

/// Component showing services for a specific deployment
#[component]
fn DeploymentServices(name: String) -> impl IntoView {
    let dep_name = name.clone();
    let services = Resource::new(move || dep_name.clone(), get_services);

    view! {
        <div>
            <h3 class="font-semibold text-sm text-base-content/70 mb-2">{name}</h3>
            <Suspense fallback=move || view! {
                <span class="loading loading-dots loading-xs"></span>
            }>
                {move || {
                    services
                        .get()
                        .map(|result| {
                            match result {
                                Ok(svcs) => render_services(svcs).into_any(),
                                Err(_) => view! { <span class="text-xs text-error">"Failed to load"</span> }.into_any(),
                            }
                        })
                }}
            </Suspense>
        </div>
    }
}

/// Render service badges inline
fn render_services(services: Vec<Service>) -> impl IntoView {
    if services.is_empty() {
        return view! { <span class="text-xs text-base-content/40">"No services"</span> }
            .into_any();
    }

    view! {
        <div class="flex flex-wrap gap-2">
            {services
                .into_iter()
                .map(|s| {
                    let status_class = match s.status.as_str() {
                        "running" => "badge badge-success gap-1",
                        "pending" => "badge badge-warning gap-1",
                        _ => "badge badge-ghost gap-1",
                    };
                    view! {
                        <div class=status_class>
                            <span>{s.name}</span>
                            <span class="font-mono text-xs">{format!("{}/{}", s.replicas, s.desired_replicas)}</span>
                        </div>
                    }
                })
                .collect::<Vec<_>>()}
        </div>
    }
    .into_any()
}
