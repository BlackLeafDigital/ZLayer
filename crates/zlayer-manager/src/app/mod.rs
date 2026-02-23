//! `ZLayer` Manager Application Shell
//!
//! Leptos 0.8 application with routing for the management interface.

pub mod components;
pub mod pages;
pub mod server_fns;

use leptos::prelude::*;
use leptos_meta::{provide_meta_context, MetaTags, Stylesheet, Title};
use leptos_router::{
    components::{Route, Router, Routes},
    path,
};

use components::{Navbar, Sidebar};
use pages::{
    Builds, Dashboard, Deployments, Git, Jobs, Networking, Nodes, Overlay, Settings, SshTunnels,
};

/// Shell component providing HTML structure for both SSR and hydration.
pub fn shell(options: LeptosOptions) -> impl IntoView {
    view! {
        <!DOCTYPE html>
        <html lang="en" data-theme="dark">
            <head>
                <meta charset="utf-8" />
                <meta name="viewport" content="width=device-width, initial-scale=1" />
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
        <Stylesheet id="leptos" href="/pkg/zlayer-manager.css" />
        <Title text="ZLayer Manager" />

        <Router>
            <div class="drawer lg:drawer-open">
                <input id="sidebar-drawer" type="checkbox" class="drawer-toggle" />

                <div class="drawer-content flex flex-col min-h-screen">
                    <Navbar />
                    <main class="flex-1 p-6 bg-base-100">
                        <Routes fallback=|| "Page not found.">
                            <Route path=path!("/") view=Dashboard />
                            <Route path=path!("/nodes") view=Nodes />
                            <Route path=path!("/deployments") view=Deployments />
                            <Route path=path!("/networking") view=Networking />
                            <Route path=path!("/overlay") view=Overlay />
                            <Route path=path!("/ssh-tunnels") view=SshTunnels />
                            <Route path=path!("/git") view=Git />
                            <Route path=path!("/builds") view=Builds />
                            <Route path=path!("/jobs") view=Jobs />
                            <Route path=path!("/settings") view=Settings />
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
