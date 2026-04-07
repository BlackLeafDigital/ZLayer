//! Reverse Proxy management page
//!
//! Displays proxy routes, TLS certificates, and stream proxies
//! with health status and backend details.

use crate::app::server_fns::{
    get_proxy_routes, get_stream_proxies, get_tls_certificates, ProxyBackendInfo, ProxyRouteInfo,
    StreamProxyInfo, TlsCertificateInfo,
};
use leptos::prelude::*;

/// Compute a health summary string like "2/3 healthy"
fn health_summary(backends: &[ProxyBackendInfo]) -> String {
    let healthy = backends.iter().filter(|b| b.healthy).count();
    let total = backends.len();
    format!("{healthy}/{total} healthy")
}

/// Check if a certificate expires within the given number of days.
///
/// Returns `true` if the expiry date is parseable and within `days` days from
/// `2026-04-03` (the current date at time of writing). Returns `false` if the
/// date is missing or unparseable.
fn expires_within_days(expires_at: Option<&String>, days: i64) -> bool {
    let Some(exp) = expires_at else {
        return false;
    };
    // Expected format: "YYYY-MM-DD" or ISO 8601 prefix
    let date_part = &exp[..10.min(exp.len())];
    let parts: Vec<&str> = date_part.split('-').collect();
    if parts.len() < 3 {
        return false;
    }
    let Ok(year) = parts[0].parse::<i64>() else {
        return false;
    };
    let Ok(month) = parts[1].parse::<i64>() else {
        return false;
    };
    let Ok(day) = parts[2].parse::<i64>() else {
        return false;
    };
    // Rough days-since-epoch calculation (good enough for threshold checks)
    let exp_days = year * 365 + month * 30 + day;
    // 2026-04-03
    let now_days = 2026 * 365 + 4 * 30 + 3;
    let diff = exp_days - now_days;
    diff >= 0 && diff < days
}

/// Stats row showing aggregate proxy metrics
#[component]
fn ProxyStats(
    routes: Vec<ProxyRouteInfo>,
    certs: Vec<TlsCertificateInfo>,
    streams: Vec<StreamProxyInfo>,
) -> impl IntoView {
    let total_routes = routes.len();

    let (healthy_backends, total_backends) = routes.iter().fold((0usize, 0usize), |(h, t), r| {
        let healthy = r.backends.iter().filter(|b| b.healthy).count();
        (h + healthy, t + r.backends.len())
    });

    let total_certs = certs.len();
    let active_streams = streams.len();

    view! {
        <div class="stats shadow w-full mb-6">
            <div class="stat">
                <div class="stat-title">"Total Routes"</div>
                <div class="stat-value text-primary">{total_routes}</div>
            </div>
            <div class="stat">
                <div class="stat-title">"Backends"</div>
                <div class="stat-value text-success">{format!("{healthy_backends} / {total_backends}")}</div>
                <div class="stat-desc">"Healthy / Total"</div>
            </div>
            <div class="stat">
                <div class="stat-title">"TLS Certificates"</div>
                <div class="stat-value text-info">{total_certs}</div>
            </div>
            <div class="stat">
                <div class="stat-title">"Active Streams"</div>
                <div class="stat-value text-secondary">{active_streams}</div>
            </div>
        </div>
    }
}

/// Render backend detail rows for a route
fn render_backend_rows(backends: Vec<ProxyBackendInfo>) -> impl IntoView {
    backends
        .into_iter()
        .map(|b| {
            let health_class = if b.healthy {
                "badge badge-success badge-sm"
            } else {
                "badge badge-error badge-sm"
            };
            let health_text = if b.healthy { "healthy" } else { "unhealthy" };
            view! {
                <tr class="bg-base-300/30">
                    <td class="pl-12 font-mono text-sm" colspan="2">{b.address}</td>
                    <td><span class=health_class>{health_text}</span></td>
                    <td class="text-sm opacity-70">{format!("{} conn", b.active_connections)}</td>
                </tr>
            }
        })
        .collect::<Vec<_>>()
}

/// Routes table component
#[component]
fn RoutesCard(routes: Vec<ProxyRouteInfo>) -> impl IntoView {
    view! {
        <div class="card bg-base-200 shadow-md mb-6">
            <div class="card-body">
                <h2 class="card-title">"Routes"</h2>
                {if routes.is_empty() {
                    view! { <div class="alert alert-info">"No proxy routes configured."</div> }.into_any()
                } else {
                    view! {
                        <div class="overflow-x-auto">
                            <table class="table table-zebra w-full">
                                <thead>
                                    <tr>
                                        <th>"Host"</th>
                                        <th>"Path Prefix"</th>
                                        <th>"Strip Prefix"</th>
                                        <th>"Backends"</th>
                                    </tr>
                                </thead>
                                <tbody>
                                    {routes.into_iter().map(|route| {
                                        let host = route.host.clone().unwrap_or_else(|| "*".to_string());
                                        let strip_badge = if route.strip_prefix { "badge badge-warning badge-sm" } else { "badge badge-ghost badge-sm" };
                                        let strip_text = if route.strip_prefix { "yes" } else { "no" };
                                        let summary = health_summary(&route.backends);
                                        let count = route.backends.len();
                                        let backend_rows = render_backend_rows(route.backends);
                                        view! {
                                            <tr>
                                                <td class="font-mono">{host}</td>
                                                <td class="font-mono">{route.path_prefix}</td>
                                                <td><span class=strip_badge>{strip_text}</span></td>
                                                <td>
                                                    <span class="text-sm">{format!("{count} backends")}</span>
                                                    " "
                                                    <span class="text-xs opacity-70">{format!("({summary})")}</span>
                                                </td>
                                            </tr>
                                            {backend_rows}
                                        }
                                    }).collect::<Vec<_>>()}
                                </tbody>
                            </table>
                        </div>
                    }.into_any()
                }}
            </div>
        </div>
    }
}

/// TLS certificates card
#[component]
fn TlsCertificatesCard(certs: Vec<TlsCertificateInfo>) -> impl IntoView {
    view! {
        <div class="card bg-base-200 shadow-md mb-6">
            <div class="card-body">
                <h2 class="card-title">"TLS Certificates"</h2>
                {if certs.is_empty() {
                    view! { <div class="alert alert-info">"No TLS certificates configured."</div> }.into_any()
                } else {
                    view! {
                        <div class="overflow-x-auto">
                            <table class="table table-zebra w-full">
                                <thead>
                                    <tr>
                                        <th>"Domain"</th>
                                        <th>"Issuer"</th>
                                        <th>"Expires"</th>
                                        <th>"Auto-Renew"</th>
                                    </tr>
                                </thead>
                                <tbody>
                                    {certs.into_iter().map(|cert| {
                                        let expiring_soon = expires_within_days(cert.expires_at.as_ref(), 7);
                                        let expires_display = cert.expires_at.clone().unwrap_or_else(|| "-".to_string());
                                        let renew_class = if cert.auto_renew { "badge badge-success badge-sm" } else { "badge badge-ghost badge-sm" };
                                        let renew_text = if cert.auto_renew { "enabled" } else { "disabled" };
                                        view! {
                                            <tr>
                                                <td class="font-mono">{cert.domain}</td>
                                                <td>{cert.issuer.unwrap_or_else(|| "-".to_string())}</td>
                                                <td>
                                                    <span class=if expiring_soon { "text-warning font-semibold" } else { "" }>
                                                        {expires_display}
                                                    </span>
                                                    {if expiring_soon {
                                                        view! { <span class="badge badge-warning badge-sm ml-2">"expiring soon"</span> }.into_any()
                                                    } else {
                                                        view! { <span></span> }.into_any()
                                                    }}
                                                </td>
                                                <td><span class=renew_class>{renew_text}</span></td>
                                            </tr>
                                        }
                                    }).collect::<Vec<_>>()}
                                </tbody>
                            </table>
                        </div>
                    }.into_any()
                }}
            </div>
        </div>
    }
}

/// Stream proxies card
#[component]
fn StreamProxiesCard(streams: Vec<StreamProxyInfo>) -> impl IntoView {
    view! {
        <div class="card bg-base-200 shadow-md mb-6">
            <div class="card-body">
                <h2 class="card-title">"Stream Proxies"</h2>
                {if streams.is_empty() {
                    view! { <div class="alert alert-info">"No stream proxies configured."</div> }.into_any()
                } else {
                    view! {
                        <div class="overflow-x-auto">
                            <table class="table table-zebra w-full">
                                <thead>
                                    <tr>
                                        <th>"Protocol"</th>
                                        <th>"Listen Port"</th>
                                        <th>"Backends"</th>
                                    </tr>
                                </thead>
                                <tbody>
                                    {streams.into_iter().map(|stream| {
                                        let proto_class = match stream.protocol.as_str() {
                                            "tcp" => "badge badge-info badge-sm",
                                            "udp" => "badge badge-accent badge-sm",
                                            _ => "badge badge-ghost badge-sm",
                                        };
                                        let summary = health_summary(&stream.backends);
                                        let count = stream.backends.len();
                                        view! {
                                            <tr>
                                                <td><span class=proto_class>{stream.protocol.to_uppercase()}</span></td>
                                                <td class="font-mono">{stream.listen_port}</td>
                                                <td>
                                                    <span class="text-sm">{format!("{count} backends")}</span>
                                                    " "
                                                    <span class="text-xs opacity-70">{format!("({summary})")}</span>
                                                </td>
                                            </tr>
                                        }
                                    }).collect::<Vec<_>>()}
                                </tbody>
                            </table>
                        </div>
                    }.into_any()
                }}
            </div>
        </div>
    }
}

/// Reverse Proxy management page
#[component]
pub fn Proxy() -> impl IntoView {
    let routes = Resource::new(|| (), |()| get_proxy_routes());
    let certs = Resource::new(|| (), |()| get_tls_certificates());
    let streams = Resource::new(|| (), |()| get_stream_proxies());

    view! {
        <div class="p-6">
            <h1 class="text-3xl font-bold mb-6">"Reverse Proxy"</h1>

            <Suspense fallback=move || view! {
                <div class="flex justify-center p-8">
                    <span class="loading loading-spinner loading-lg"></span>
                </div>
            }>
                <ErrorBoundary fallback=|errors| view! {
                    <div class="alert alert-error">
                        <span>{move || format!("Error: {:?}", errors.get())}</span>
                    </div>
                }>
                    {move || {
                        let routes_data = routes.get();
                        let certs_data = certs.get();
                        let streams_data = streams.get();

                        match (routes_data, certs_data, streams_data) {
                            (Some(Ok(r)), Some(Ok(c)), Some(Ok(s))) => {
                                view! {
                                    <ProxyStats routes=r.clone() certs=c.clone() streams=s.clone() />
                                    <RoutesCard routes=r />
                                    <TlsCertificatesCard certs=c />
                                    <StreamProxiesCard streams=s />
                                }.into_any()
                            }
                            (Some(Err(e)), _, _) | (_, Some(Err(e)), _) | (_, _, Some(Err(e))) => {
                                view! {
                                    <div class="alert alert-error">{e.to_string()}</div>
                                }.into_any()
                            }
                            _ => {
                                view! {
                                    <div class="flex justify-center p-8">
                                        <span class="loading loading-spinner loading-lg"></span>
                                    </div>
                                }.into_any()
                            }
                        }
                    }}
                </ErrorBoundary>
            </Suspense>
        </div>
    }
}
