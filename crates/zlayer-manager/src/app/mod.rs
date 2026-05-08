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

use components::ProtectedShell;
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
///
/// `/login` and `/bootstrap` render bare (no sidebar / navbar). All other
/// routes wrap their content in [`ProtectedShell`], which combines an auth
/// guard with the persistent drawer + sidebar + navbar chrome.
#[component]
pub fn App() -> impl IntoView {
    provide_meta_context();

    view! {
        <Title text="ZLayer Manager" />

        <Router>
            <Routes fallback=|| "Page not found.">
                <Route path=path!("/login") view=Login />
                <Route path=path!("/bootstrap") view=Bootstrap />
                <Route path=path!("/") view=|| view! { <ProtectedShell><Dashboard /></ProtectedShell> } />
                <Route path=path!("/nodes") view=|| view! { <ProtectedShell><Nodes /></ProtectedShell> } />
                <Route path=path!("/deployments") view=|| view! { <ProtectedShell><Deployments /></ProtectedShell> } />
                <Route path=path!("/networking") view=|| view! { <ProtectedShell><Networking /></ProtectedShell> } />
                <Route path=path!("/networks") view=|| view! { <ProtectedShell><Networks /></ProtectedShell> } />
                <Route path=path!("/overlay") view=|| view! { <ProtectedShell><Overlay /></ProtectedShell> } />
                <Route path=path!("/proxy") view=|| view! { <ProtectedShell><Proxy /></ProtectedShell> } />
                <Route path=path!("/ssh-tunnels") view=|| view! { <ProtectedShell><SshTunnels /></ProtectedShell> } />
                <Route path=path!("/git") view=|| view! { <ProtectedShell><Git /></ProtectedShell> } />
                <Route path=path!("/builds") view=|| view! { <ProtectedShell><Builds /></ProtectedShell> } />
                <Route path=path!("/jobs") view=|| view! { <ProtectedShell><Jobs /></ProtectedShell> } />
                <Route path=path!("/users") view=|| view! { <ProtectedShell><Users /></ProtectedShell> } />
                <Route path=path!("/groups") view=|| view! { <ProtectedShell><Groups /></ProtectedShell> } />
                <Route path=path!("/variables") view=|| view! { <ProtectedShell><Variables /></ProtectedShell> } />
                <Route path=path!("/secrets") view=|| view! { <ProtectedShell><Secrets /></ProtectedShell> } />
                <Route path=path!("/tasks") view=|| view! { <ProtectedShell><Tasks /></ProtectedShell> } />
                <Route path=path!("/workflows") view=|| view! { <ProtectedShell><Workflows /></ProtectedShell> } />
                <Route path=path!("/notifiers") view=|| view! { <ProtectedShell><Notifiers /></ProtectedShell> } />
                <Route path=path!("/permissions") view=|| view! { <ProtectedShell><Permissions /></ProtectedShell> } />
                <Route path=path!("/projects") view=|| view! { <ProtectedShell><Projects /></ProtectedShell> } />
                <Route path=path!("/projects/:id") view=|| view! { <ProtectedShell><ProjectDetail /></ProtectedShell> } />
                <Route path=path!("/audit") view=|| view! { <ProtectedShell><Audit /></ProtectedShell> } />
                <Route path=path!("/settings") view=|| view! { <ProtectedShell><Settings /></ProtectedShell> } />
            </Routes>
        </Router>
    }
}
