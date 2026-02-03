//! Nodes page
use leptos::prelude::*;

use crate::app::server_fns::{get_nodes, Node};

/// Render nodes table from data
fn render_nodes_table(nodes: Vec<Node>) -> impl IntoView {
    view! {
        <div class="overflow-x-auto">
            <table class="table table-zebra w-full">
                <thead>
                    <tr>
                        <th>"ID"</th>
                        <th>"Name"</th>
                        <th>"Status"</th>
                        <th>"IP Address"</th>
                        <th>"CPU %"</th>
                        <th>"Memory %"</th>
                        <th>"Actions"</th>
                    </tr>
                </thead>
                <tbody>
                    {nodes.into_iter().map(|node| {
                        let status_class = match node.status.as_str() {
                            "healthy" => "badge badge-success",
                            "unhealthy" => "badge badge-error",
                            _ => "badge badge-ghost",
                        };
                        let id_short = node.id[..8.min(node.id.len())].to_string();
                        view! {
                            <tr>
                                <td class="font-mono text-sm">{id_short}</td>
                                <td>{node.name}</td>
                                <td><span class=status_class>{node.status}</span></td>
                                <td class="font-mono">{node.ip_address}</td>
                                <td>{format!("{:.1}%", node.cpu_percent)}</td>
                                <td>{format!("{:.1}%", node.memory_percent)}</td>
                                <td>
                                    <button class="btn btn-sm btn-ghost">"Edit"</button>
                                    <button class="btn btn-sm btn-ghost text-error">"Remove"</button>
                                </td>
                            </tr>
                        }
                    }).collect::<Vec<_>>()}
                </tbody>
            </table>
        </div>
    }
}

#[component]
pub fn Nodes() -> impl IntoView {
    let nodes = Resource::new(|| (), |()| get_nodes());

    view! {
        <div class="container mx-auto p-6">
            <div class="flex justify-between items-center mb-6">
                <h1 class="text-3xl font-bold">"Nodes"</h1>
                <button class="btn btn-primary">"Add Node"</button>
            </div>
            <Suspense fallback=move || view! { <div class="flex justify-center p-8"><span class="loading loading-spinner loading-lg"></span></div> }>
                <ErrorBoundary fallback=|errors| view! { <div class="alert alert-error"><span>{move || format!("Error: {:?}", errors.get())}</span></div> }>
                    {move || nodes.get().map(|result| match result {
                        Ok(node_list) => {
                            if node_list.is_empty() {
                                view! { <div class="alert alert-info"><span>"No nodes found. Add your first node!"</span></div> }.into_any()
                            } else {
                                render_nodes_table(node_list).into_any()
                            }
                        }
                        Err(e) => view! { <div class="alert alert-error">{e.to_string()}</div> }.into_any(),
                    })}
                </ErrorBoundary>
            </Suspense>
        </div>
    }
}
