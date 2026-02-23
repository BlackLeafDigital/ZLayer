//! Nodes page
//!
//! The node management API is not yet implemented. This page shows a
//! "Coming Soon" message describing the planned functionality.

use leptos::prelude::*;

/// Nodes page component
///
/// Displays a Coming Soon state since the node management API is still
/// a placeholder (returns empty lists). Once scheduler integration is
/// complete, this page will show a live node table.
#[component]
pub fn Nodes() -> impl IntoView {
    view! {
        <div class="container mx-auto p-6">
            <div class="flex justify-between items-center mb-6">
                <h1 class="text-3xl font-bold">"Nodes"</h1>
            </div>

            <div class="card bg-base-200 shadow-xl">
                <div class="card-body items-center text-center">
                    <h2 class="card-title text-2xl mb-4">"Coming Soon"</h2>
                    <p class="text-base-content/70 max-w-lg">
                        "Node management is under active development. When complete, this page will allow you to:"
                    </p>
                    <ul class="text-left mt-4 space-y-2 text-base-content/70">
                        <li class="flex items-center gap-2">
                            <span class="badge badge-ghost badge-sm">"1"</span>
                            "View all cluster nodes with real-time health status"
                        </li>
                        <li class="flex items-center gap-2">
                            <span class="badge badge-ghost badge-sm">"2"</span>
                            "Monitor CPU, memory, and container metrics per node"
                        </li>
                        <li class="flex items-center gap-2">
                            <span class="badge badge-ghost badge-sm">"3"</span>
                            "Generate join tokens to add new worker nodes"
                        </li>
                        <li class="flex items-center gap-2">
                            <span class="badge badge-ghost badge-sm">"4"</span>
                            "Manage node labels for scheduling and placement"
                        </li>
                        <li class="flex items-center gap-2">
                            <span class="badge badge-ghost badge-sm">"5"</span>
                            "Drain and remove nodes from the cluster"
                        </li>
                    </ul>
                </div>
            </div>
        </div>
    }
}
