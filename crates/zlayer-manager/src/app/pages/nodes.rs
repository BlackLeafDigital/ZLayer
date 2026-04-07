//! Nodes management page
//!
//! Displays cluster nodes from Raft state with status, role, resource usage,
//! and overlay IP. Also shows aggregate stats and a join-token generator for
//! the leader.

use leptos::prelude::*;

use crate::app::server_fns::{get_nodes, Node};

/// Format bytes into a human-readable string (e.g. "16.0 GB").
fn format_bytes(bytes: u64) -> String {
    #[allow(clippy::cast_precision_loss)]
    let b = bytes as f64;
    if b >= 1_073_741_824.0 {
        format!("{:.1} GB", b / 1_073_741_824.0)
    } else if b >= 1_048_576.0 {
        format!("{:.1} MB", b / 1_048_576.0)
    } else if b >= 1024.0 {
        format!("{:.1} KB", b / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

/// Truncate a node ID for display (e.g. "12345678" -> "1234...5678").
fn truncate_id(id: &str) -> String {
    if id.len() > 10 {
        format!("{}...{}", &id[..4], &id[id.len() - 4..])
    } else {
        id.to_string()
    }
}

/// Badge for node role (leader / voter / learner).
#[component]
fn RoleBadge(role: String) -> impl IntoView {
    let class = match role.as_str() {
        "leader" => "badge badge-primary badge-sm",
        "voter" => "badge badge-info badge-sm",
        _ => "badge badge-ghost badge-sm",
    };
    view! { <span class=class>{role}</span> }
}

/// Badge for node status (ready / draining / dead).
#[component]
fn StatusBadge(status: String) -> impl IntoView {
    let class = match status.as_str() {
        "ready" => "badge badge-success badge-sm",
        "draining" => "badge badge-warning badge-sm",
        "dead" => "badge badge-error badge-sm",
        _ => "badge badge-ghost badge-sm",
    };
    view! { <span class=class>{status}</span> }
}

/// Stats row with Total Nodes, Healthy Nodes, Leader, and Cluster Status.
#[component]
fn NodeStatsRow(nodes: Vec<Node>) -> impl IntoView {
    let total = nodes.len();
    let healthy = nodes.iter().filter(|n| n.status == "ready").count();
    let leader_node = nodes.iter().find(|n| n.is_leader);
    let leader_label = leader_node.map_or_else(|| "None".to_string(), |n| format!("Node {}", n.id));
    let cluster_healthy = healthy == total && total > 0;

    view! {
        <div class="stats shadow bg-base-200 w-full">
            <div class="stat">
                <div class="stat-figure text-primary">
                    <svg xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" stroke-width="1.5" stroke="currentColor" class="w-8 h-8">
                        <path stroke-linecap="round" stroke-linejoin="round" d="M5.25 14.25h13.5m-13.5 0a3 3 0 01-3-3m3 3a3 3 0 100 6h13.5a3 3 0 100-6m-16.5-3a3 3 0 013-3h13.5a3 3 0 013 3m-19.5 0a4.5 4.5 0 01.9-2.7L5.737 5.1a3.375 3.375 0 012.7-1.35h7.126c1.062 0 2.062.5 2.7 1.35l2.587 3.45a4.5 4.5 0 01.9 2.7m0 0a3 3 0 01-3 3m0 3h.008v.008h-.008v-.008zm0-6h.008v.008h-.008v-.008zm-3 6h.008v.008h-.008v-.008zm0-6h.008v.008h-.008v-.008z" />
                    </svg>
                </div>
                <div class="stat-title">"Total Nodes"</div>
                <div class="stat-value text-primary">{total}</div>
                <div class="stat-desc">{format!("{healthy} healthy")}</div>
            </div>
            <div class="stat">
                <div class="stat-figure text-success">
                    <svg xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" stroke-width="1.5" stroke="currentColor" class="w-8 h-8">
                        <path stroke-linecap="round" stroke-linejoin="round" d="M9 12.75L11.25 15 15 9.75M21 12a9 9 0 11-18 0 9 9 0 0118 0z" />
                    </svg>
                </div>
                <div class="stat-title">"Healthy"</div>
                <div class="stat-value text-success">{healthy}</div>
                <div class="stat-desc">"nodes responding"</div>
            </div>
            <div class="stat">
                <div class="stat-title">"Leader"</div>
                <div class="stat-value text-lg">{leader_label}</div>
                <div class="stat-desc">"Raft consensus leader"</div>
            </div>
            <div class="stat">
                <div class="stat-title">"Cluster"</div>
                <div class="stat-value text-lg">
                    {if cluster_healthy {
                        view! { <span class="text-success">"Healthy"</span> }.into_any()
                    } else if total == 0 {
                        view! { <span class="text-base-content/50">"No Nodes"</span> }.into_any()
                    } else {
                        view! { <span class="text-warning">"Degraded"</span> }.into_any()
                    }}
                </div>
                <div class="stat-desc">{format!("{total} node cluster")}</div>
            </div>
        </div>
    }
}

/// Table of cluster nodes.
#[component]
fn NodesTable(nodes: Vec<Node>) -> impl IntoView {
    if nodes.is_empty() {
        return view! {
            <div class="card bg-base-200 shadow">
                <div class="card-body items-center text-center">
                    <p class="text-base-content/50">"No nodes in the cluster. Start a runtime with Raft enabled to see nodes here."</p>
                </div>
            </div>
        }
        .into_any();
    }

    view! {
        <div class="card bg-base-200 shadow">
            <div class="card-body">
                <h2 class="card-title">"Cluster Nodes"</h2>
                <div class="overflow-x-auto">
                    <table class="table table-zebra w-full">
                        <thead>
                            <tr>
                                <th>"Node ID"</th>
                                <th>"Address"</th>
                                <th>"Role"</th>
                                <th>"Overlay IP"</th>
                                <th>"Status"</th>
                                <th>"CPU"</th>
                                <th>"Memory"</th>
                            </tr>
                        </thead>
                        <tbody>
                            {nodes
                                .into_iter()
                                .map(|node| {
                                    let full_id = node.id.clone();
                                    let short_id = truncate_id(&node.id);
                                    let address = if node.address.is_empty() {
                                        "-".to_string()
                                    } else {
                                        node.address.clone()
                                    };
                                    let overlay = if node.overlay_ip.is_empty() {
                                        "-".to_string()
                                    } else {
                                        node.overlay_ip.clone()
                                    };
                                    let cpu_display = if node.cpu_total > 0.0 {
                                        format!("{:.1} / {:.0} cores", node.cpu_used, node.cpu_total)
                                    } else {
                                        "-".to_string()
                                    };
                                    let mem_display = if node.memory_total > 0 {
                                        format!("{} / {}", format_bytes(node.memory_used), format_bytes(node.memory_total))
                                    } else {
                                        "-".to_string()
                                    };
                                    view! {
                                        <tr class="hover">
                                            <td class="font-mono text-sm" title=full_id>{short_id}</td>
                                            <td class="font-mono text-sm">{address}</td>
                                            <td><RoleBadge role=node.role /></td>
                                            <td class="font-mono text-sm">{overlay}</td>
                                            <td><StatusBadge status=node.status /></td>
                                            <td class="text-sm">{cpu_display}</td>
                                            <td class="text-sm">{mem_display}</td>
                                        </tr>
                                    }
                                })
                                .collect::<Vec<_>>()}
                        </tbody>
                    </table>
                </div>
            </div>
        </div>
    }
    .into_any()
}

/// Join token generation card (only relevant when this node is the leader).
#[component]
fn JoinTokenCard(has_leader: bool) -> impl IntoView {
    if !has_leader {
        return view! { <div></div> }.into_any();
    }

    let (token_visible, set_token_visible) = signal(false);
    let (token_text, set_token_text) = signal(String::new());
    let (token_error, set_token_error) = signal(Option::<String>::None);

    let generate = move |_| {
        set_token_visible.set(false);
        set_token_error.set(None);
        set_token_text.set(String::new());

        // For now, show instructions since the join token API requires admin auth
        // and the manager may not have admin credentials configured.
        set_token_text.set("Run on the leader node:\n  zlayer node gen-token".to_string());
        set_token_visible.set(true);
    };

    view! {
        <div class="card bg-base-200 shadow">
            <div class="card-body">
                <div class="flex justify-between items-center">
                    <h2 class="card-title text-lg">"Add Nodes"</h2>
                    <button class="btn btn-primary btn-sm" on:click=generate>
                        "Generate Join Token"
                    </button>
                </div>
                <p class="text-sm text-base-content/70">
                    "Generate a join token so new worker nodes can join this cluster."
                </p>
                {move || {
                    if let Some(err) = token_error.get() {
                        view! {
                            <div class="alert alert-error mt-2">
                                <span>{err}</span>
                            </div>
                        }.into_any()
                    } else if token_visible.get() {
                        let text = token_text.get();
                        view! {
                            <div class="mt-2 p-3 bg-base-300 rounded-lg">
                                <pre class="text-sm font-mono whitespace-pre-wrap break-all">{text}</pre>
                            </div>
                        }.into_any()
                    } else {
                        view! { <div></div> }.into_any()
                    }
                }}
            </div>
        </div>
    }
    .into_any()
}

/// Nodes page component
#[component]
pub fn Nodes() -> impl IntoView {
    let nodes = Resource::new(|| (), |()| get_nodes());

    view! {
        <div class="container mx-auto p-6 space-y-6">
            <h1 class="text-3xl font-bold">"Nodes"</h1>

            <Suspense fallback=move || view! {
                <div class="flex flex-col gap-4">
                    <div class="skeleton h-24 w-full"></div>
                    <div class="skeleton h-64 w-full"></div>
                </div>
            }>
                {move || {
                    nodes
                        .get()
                        .map(|result| {
                            match result {
                                Ok(node_list) => {
                                    let has_leader = node_list.iter().any(|n| n.is_leader);
                                    let stats_nodes = node_list.clone();
                                    view! {
                                        <div class="space-y-6">
                                            <NodeStatsRow nodes=stats_nodes />
                                            <NodesTable nodes=node_list />
                                            <JoinTokenCard has_leader=has_leader />
                                        </div>
                                    }
                                    .into_any()
                                }
                                Err(e) => {
                                    view! {
                                        <div class="alert alert-error">
                                            <span>{format!("Failed to load nodes: {e}")}</span>
                                        </div>
                                    }
                                    .into_any()
                                }
                            }
                        })
                }}
            </Suspense>
        </div>
    }
}
