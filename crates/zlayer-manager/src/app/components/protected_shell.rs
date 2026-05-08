//! `ProtectedShell` — combined auth gate + drawer layout for authenticated routes.
//!
//! Wraps every authenticated route in two responsibilities:
//!
//! 1. Calls [`AuthGuard`] so the user is redirected to `/login` (or
//!    `/bootstrap`) when they aren't signed in.
//! 2. Renders the persistent app chrome — drawer, Navbar, Sidebar — around
//!    the route's content.
//!
//! Login and bootstrap routes do NOT use this component; they render bare so
//! the sidebar / navbar are not visible to logged-out users.

use leptos::prelude::*;

use crate::app::auth_guard::AuthGuard;
use crate::app::components::{Navbar, Sidebar};

/// Wrap an authenticated route in [`AuthGuard`] + the drawer layout.
///
/// Usage:
///
/// ```ignore
/// <Route path=path!("/dashboard")
///        view=|| view! { <ProtectedShell><Dashboard /></ProtectedShell> } />
/// ```
#[component]
pub fn ProtectedShell(children: ChildrenFn) -> impl IntoView {
    let children = StoredValue::new(children);
    view! {
        <AuthGuard>
            <div class="drawer lg:drawer-open">
                <input id="sidebar-drawer" type="checkbox" class="drawer-toggle" />

                <div class="drawer-content flex flex-col min-h-screen">
                    <Navbar />
                    <main class="flex-1 p-6 bg-base-100">
                        {move || children.with_value(|c| c())}
                    </main>
                </div>

                <div class="drawer-side z-40">
                    <label for="sidebar-drawer" aria-label="close sidebar" class="drawer-overlay"></label>
                    <Sidebar />
                </div>
            </div>
        </AuthGuard>
    }
}
