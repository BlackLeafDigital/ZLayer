//! Sidebar navigation component

use leptos::prelude::*;
use leptos_router::components::A;

/// Sidebar component with navigation links
#[component]
pub fn Sidebar() -> impl IntoView {
    view! {
        <aside class="w-64 bg-base-200 min-h-screen">
            <ul class="menu p-4">
                <li>
                    <A href="/">
                        "Dashboard"
                    </A>
                </li>
                <li>
                    <A href="/nodes">
                        "Nodes"
                    </A>
                </li>
                <li>
                    <A href="/deployments">
                        "Deployments"
                    </A>
                </li>
                <li>
                    <A href="/networking">
                        "Networking"
                    </A>
                </li>
                <li>
                    <A href="/git">
                        "Git"
                    </A>
                </li>
                <li>
                    <A href="/builds">
                        "Builds"
                    </A>
                </li>
                <li>
                    <A href="/settings">
                        "Settings"
                    </A>
                </li>
            </ul>
        </aside>
    }
}
