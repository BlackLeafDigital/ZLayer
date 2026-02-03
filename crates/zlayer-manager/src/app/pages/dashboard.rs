//! Dashboard page
//!
//! Main dashboard view displaying system stats overview.

use leptos::prelude::*;

use crate::app::components::StatsCard;
use crate::app::server_fns::get_system_stats;

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
///
/// Displays overview stats for nodes, deployments, containers, and services.
#[component]
pub fn Dashboard() -> impl IntoView {
    let stats = Resource::new(|| (), |()| get_system_stats());

    view! {
        <div class="p-6">
            <h1 class="text-3xl font-bold mb-6">"Dashboard"</h1>
            <Suspense fallback=move || {
                view! {
                    <div class="flex justify-center p-8">
                        <span class="loading loading-spinner loading-lg"></span>
                    </div>
                }
            }>
                <ErrorBoundary fallback=|errors| {
                    view! {
                        <div class="alert alert-error">
                            <span>{move || format!("Error: {:?}", errors.get())}</span>
                        </div>
                    }
                }>
                    {move || {
                        stats
                            .get()
                            .map(|result| {
                                match result {
                                    Ok(s) => {
                                        view! {
                                            <div class="stats stats-vertical lg:stats-horizontal shadow w-full">
                                                <StatsCard
                                                    title="Nodes".to_string()
                                                    value=s.total_nodes.to_string()
                                                />
                                                <StatsCard
                                                    title="Deployments".to_string()
                                                    value=s.total_deployments.to_string()
                                                />
                                                <StatsCard
                                                    title="Active".to_string()
                                                    value=s.active_deployments.to_string()
                                                />
                                                <StatsCard
                                                    title="Uptime".to_string()
                                                    value=format_uptime(s.uptime_seconds)
                                                />
                                            </div>
                                        }
                                            .into_any()
                                    }
                                    Err(e) => {
                                        view! {
                                            <div class="alert alert-error">{e.to_string()}</div>
                                        }
                                            .into_any()
                                    }
                                }
                            })
                    }}
                </ErrorBoundary>
            </Suspense>
        </div>
    }
}
