//! Dashboard page component
//!
//! Displays system statistics, resource usage, and service information.
//! Data is fetched from server functions and auto-refreshes every 5 seconds.

use leptos::prelude::*;

use crate::app::components::{Footer, Navbar, ResourceMeter, ServicesTable, StatsCard};
use crate::app::server_fns::{get_all_service_stats, get_system_stats};

/// Format uptime seconds into a human-readable string
fn format_uptime(seconds: u64) -> String {
    let days = seconds / 86400;
    let hours = (seconds % 86400) / 3600;
    let minutes = (seconds % 3600) / 60;
    let secs = seconds % 60;

    if days > 0 {
        format!("{days}d {hours}h {minutes}m")
    } else if hours > 0 {
        format!("{hours}h {minutes}m {secs}s")
    } else if minutes > 0 {
        format!("{minutes}m {secs}s")
    } else {
        format!("{secs}s")
    }
}

/// Dashboard page component
#[component]
#[allow(clippy::too_many_lines)]
pub fn DashboardPage() -> impl IntoView {
    // Refresh trigger - incremented to force resource refetch
    let refresh_trigger = RwSignal::new(0_u32);

    // Fetch system stats with refresh support
    let system_stats = Resource::new(
        move || refresh_trigger.get(),
        |_| async move { get_system_stats().await.ok() },
    );

    // Fetch service stats with refresh support
    let service_stats = Resource::new(
        move || refresh_trigger.get(),
        |_| async move { get_all_service_stats().await.ok() },
    );

    // Set up auto-refresh interval on client side only
    #[cfg(feature = "hydrate")]
    {
        use gloo_timers::callback::Interval;

        // Use spawn_local to set up the interval - it will live as long as the component
        leptos::task::spawn_local(async move {
            // The interval will run indefinitely. Since WASM is single-threaded,
            // when the component is removed from the DOM, the spawned task
            // effectively becomes orphaned but harmless (just updates a signal).
            let _interval = Interval::new(5000, move || {
                refresh_trigger.update(|n| *n = n.wrapping_add(1));
            });

            // Keep the interval alive by pending forever
            std::future::pending::<()>().await;
        });
    }

    view! {
        <div class="page-layout">
            <Navbar/>

            <main class="main-content dashboard-page">
                <div class="container">
                    <header class="dashboard-header">
                        <h1 class="dashboard-title">"Dashboard"</h1>
                        <p class="dashboard-subtitle">
                            "System overview and service monitoring"
                        </p>
                    </header>

                    <div class="dashboard-grid">
                        // System Overview Section
                        <section class="stats-section">
                            <h2 class="section-title">"System Overview"</h2>
                            <Suspense fallback=move || view! {
                                <div class="stats-loading">"Loading system stats..."</div>
                            }>
                                {move || {
                                    system_stats.get().map(|stats_opt| {
                                        match stats_opt {
                                            Some(stats) => view! {
                                                <div class="stats-cards-grid">
                                                    <StatsCard
                                                        value={stats.version.clone()}
                                                        label="Version".to_string()
                                                        icon="icon-version".to_string()
                                                    />
                                                    <StatsCard
                                                        value={format_uptime(stats.uptime_seconds)}
                                                        label="Uptime".to_string()
                                                        icon="icon-clock".to_string()
                                                    />
                                                    <StatsCard
                                                        value={format!("{}", stats.total_services)}
                                                        label="Services".to_string()
                                                        icon="icon-services".to_string()
                                                    />
                                                    <StatsCard
                                                        value={format!("{}", stats.running_containers)}
                                                        label="Containers".to_string()
                                                        icon="icon-container".to_string()
                                                    />
                                                </div>
                                            }.into_any(),
                                            None => view! {
                                                <div class="stats-error">"Failed to load system stats"</div>
                                            }.into_any()
                                        }
                                    })
                                }}
                            </Suspense>
                        </section>

                        // Resource Usage Section
                        <section class="resource-section">
                            <h2 class="section-title">"Resource Usage"</h2>
                            <Suspense fallback=move || view! {
                                <div class="stats-loading">"Loading resource data..."</div>
                            }>
                                {move || {
                                    system_stats.get().map(|stats_opt| {
                                        match stats_opt {
                                            Some(stats) => {
                                                #[allow(clippy::cast_precision_loss)]
                                                let memory_percent = if stats.total_memory_limit > 0 {
                                                    (stats.total_memory_bytes as f64 / stats.total_memory_limit as f64) * 100.0
                                                } else {
                                                    0.0
                                                };

                                                view! {
                                                    <div class="resource-meters">
                                                        <ResourceMeter
                                                            label="CPU Usage".to_string()
                                                            value={stats.total_cpu_percent}
                                                        />
                                                        <ResourceMeter
                                                            label="Memory Usage".to_string()
                                                            value={memory_percent}
                                                        />
                                                    </div>
                                                }.into_any()
                                            },
                                            None => view! {
                                                <div class="stats-error">"Failed to load resource data"</div>
                                            }.into_any()
                                        }
                                    })
                                }}
                            </Suspense>
                        </section>

                        // Services List Section
                        <section class="services-section">
                            <h2 class="section-title">"Services"</h2>
                            <Suspense fallback=move || view! {
                                <div class="stats-loading">"Loading services..."</div>
                            }>
                                {move || {
                                    service_stats.get().map(|services_opt| {
                                        match services_opt {
                                            Some(services) if !services.is_empty() => {
                                                view! {
                                                    <ServicesTable services={services} />
                                                }.into_any()
                                            },
                                            Some(_) => view! {
                                                <div class="services-empty">
                                                    <p>"No services registered yet."</p>
                                                    <p class="services-empty-hint">
                                                        "Deploy a service to see it appear here."
                                                    </p>
                                                </div>
                                            }.into_any(),
                                            None => view! {
                                                <div class="stats-error">"Failed to load services"</div>
                                            }.into_any()
                                        }
                                    })
                                }}
                            </Suspense>
                        </section>
                    </div>
                </div>
            </main>

            <Footer/>
        </div>
    }
}
