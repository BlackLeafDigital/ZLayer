//! `ZLayer` Manager Application Shell
//!
//! Leptos 0.8 application with routing for the management interface.

pub mod auth_guard;
pub mod components;
pub mod pages;
pub mod server_fns;
pub mod util;

use leptos::prelude::*;
use leptos_meta::{provide_meta_context, HashedStylesheet, MetaTags, Title};
use leptos_router::{
    components::{Route, Router, Routes},
    path,
};

use auth_guard::AuthGuard;
use components::{Navbar, Sidebar};
use pages::{
    Audit, Bootstrap, Builds, Dashboard, Deployments, Git, Groups, Jobs, Login, Networking,
    Networks, Nodes, Notifiers, Overlay, Permissions, ProjectDetail, Projects, Proxy, Secrets,
    Settings, SshTunnels, Tasks, Users, Variables, Workflows,
};

/// Shell component providing HTML structure for both SSR and hydration.
pub fn shell(options: LeptosOptions) -> impl IntoView {
    view! {
        <!DOCTYPE html>
        <html lang="en" data-theme="dark">
            <head>
                <meta charset="utf-8" />
                <meta name="viewport" content="width=device-width, initial-scale=1" />
                <link rel="icon" type="image/png" href="/assets/zlayer_logo.png" />
                <HashedStylesheet id="leptos" options=options.clone() />
                <AutoReload options=options.clone() />
                <HydrationScripts options />
                <MetaTags />
            </head>
            <body class="min-h-screen bg-base-100">
                <App />
            </body>
        </html>
    }
}

/// Main application component with router setup.
#[component]
pub fn App() -> impl IntoView {
    provide_meta_context();

    view! {
        <Title text="ZLayer Manager" />

        <Router>
            <div class="drawer lg:drawer-open">
                <input id="sidebar-drawer" type="checkbox" class="drawer-toggle" />

                <div class="drawer-content flex flex-col min-h-screen">
                    <Navbar />
                    <main class="flex-1 p-6 bg-base-100">
                        <Routes fallback=|| "Page not found.">
                            <Route path=path!("/login") view=Login />
                            <Route path=path!("/bootstrap") view=Bootstrap />
                            <Route path=path!("/") view=|| view! { <AuthGuard><Dashboard /></AuthGuard> } />
                            <Route path=path!("/nodes") view=|| view! { <AuthGuard><Nodes /></AuthGuard> } />
                            <Route path=path!("/deployments") view=|| view! { <AuthGuard><Deployments /></AuthGuard> } />
                            <Route path=path!("/networking") view=|| view! { <AuthGuard><Networking /></AuthGuard> } />
                            <Route path=path!("/networks") view=|| view! { <AuthGuard><Networks /></AuthGuard> } />
                            <Route path=path!("/overlay") view=|| view! { <AuthGuard><Overlay /></AuthGuard> } />
                            <Route path=path!("/proxy") view=|| view! { <AuthGuard><Proxy /></AuthGuard> } />
                            <Route path=path!("/ssh-tunnels") view=|| view! { <AuthGuard><SshTunnels /></AuthGuard> } />
                            <Route path=path!("/git") view=|| view! { <AuthGuard><Git /></AuthGuard> } />
                            <Route path=path!("/builds") view=|| view! { <AuthGuard><Builds /></AuthGuard> } />
                            <Route path=path!("/jobs") view=|| view! { <AuthGuard><Jobs /></AuthGuard> } />
                            <Route path=path!("/users") view=|| view! { <AuthGuard><Users /></AuthGuard> } />
                            <Route path=path!("/groups") view=|| view! { <AuthGuard><Groups /></AuthGuard> } />
                            <Route path=path!("/variables") view=|| view! { <AuthGuard><Variables /></AuthGuard> } />
                            <Route path=path!("/secrets") view=|| view! { <AuthGuard><Secrets /></AuthGuard> } />
                            <Route path=path!("/tasks") view=|| view! { <AuthGuard><Tasks /></AuthGuard> } />
                            <Route path=path!("/workflows") view=|| view! { <AuthGuard><Workflows /></AuthGuard> } />
                            <Route path=path!("/notifiers") view=|| view! { <AuthGuard><Notifiers /></AuthGuard> } />
                            <Route path=path!("/permissions") view=|| view! { <AuthGuard><Permissions /></AuthGuard> } />
                            <Route path=path!("/projects") view=|| view! { <AuthGuard><Projects /></AuthGuard> } />
                            <Route path=path!("/projects/:id") view=|| view! { <AuthGuard><ProjectDetail /></AuthGuard> } />
                            <Route path=path!("/audit") view=|| view! { <AuthGuard><Audit /></AuthGuard> } />
                            <Route path=path!("/settings") view=|| view! { <AuthGuard><Settings /></AuthGuard> } />
                        </Routes>
                    </main>
                </div>

                <div class="drawer-side z-40">
                    <label for="sidebar-drawer" aria-label="close sidebar" class="drawer-overlay"></label>
                    <Sidebar />
                </div>
            </div>
        </Router>
    }
}
