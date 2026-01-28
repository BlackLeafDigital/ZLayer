//! ZLayer Leptos UI Application
//!
//! This application supports SSR (Server-Side Rendering) with client-side hydration.
//! - Initial HTML is rendered on the server for fast first paint and SEO
//! - WASM bundle hydrates the HTML for full client-side interactivity
//! - Reactive signals work on the client after hydration completes

pub mod components;
pub mod pages;
pub mod server_fns;

use leptos::prelude::*;
use leptos_router::components::{Route, Router, Routes};
use leptos_router::*;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::JsCast;

use components::{Footer, Navbar};
use pages::{DocsPage, HomePage, PlaygroundPage};

// =============================================================================
// SVG Icons Module - Inline SVGs for SSR compatibility
// =============================================================================

/// Inline SVG icons using Iconify-style paths for SSR compatibility
pub mod icons {
    use leptos::prelude::*;

    /// Container/Box icon for ZLayer logo
    #[component]
    pub fn ContainerIcon(#[prop(default = "32")] size: &'static str) -> impl IntoView {
        view! {
            <svg
                xmlns="http://www.w3.org/2000/svg"
                width=size
                height=size
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="2"
                stroke-linecap="round"
                stroke-linejoin="round"
                class="icon icon-container"
            >
                // Box base
                <path d="M21 16V8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16z"/>
                <polyline points="3.27 6.96 12 12.01 20.73 6.96"/>
                <line x1="12" y1="22.08" x2="12" y2="12"/>
            </svg>
        }
    }

    /// Layers icon
    #[component]
    pub fn LayersIcon(#[prop(default = "24")] size: &'static str) -> impl IntoView {
        view! {
            <svg
                xmlns="http://www.w3.org/2000/svg"
                width=size
                height=size
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="2"
                stroke-linecap="round"
                stroke-linejoin="round"
                class="icon icon-layers"
            >
                <polygon points="12 2 2 7 12 12 22 7 12 2"/>
                <polyline points="2 17 12 22 22 17"/>
                <polyline points="2 12 12 17 22 12"/>
            </svg>
        }
    }

    /// Book/Documentation icon
    #[component]
    pub fn BookIcon(#[prop(default = "18")] size: &'static str) -> impl IntoView {
        view! {
            <svg
                xmlns="http://www.w3.org/2000/svg"
                width=size
                height=size
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="2"
                stroke-linecap="round"
                stroke-linejoin="round"
                class="icon icon-book"
            >
                <path d="M4 19.5A2.5 2.5 0 0 1 6.5 17H20"/>
                <path d="M6.5 2H20v20H6.5A2.5 2.5 0 0 1 4 19.5v-15A2.5 2.5 0 0 1 6.5 2z"/>
            </svg>
        }
    }

    /// Play/Terminal icon
    #[component]
    pub fn TerminalIcon(#[prop(default = "18")] size: &'static str) -> impl IntoView {
        view! {
            <svg
                xmlns="http://www.w3.org/2000/svg"
                width=size
                height=size
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="2"
                stroke-linecap="round"
                stroke-linejoin="round"
                class="icon icon-terminal"
            >
                <polyline points="4 17 10 11 4 5"/>
                <line x1="12" y1="19" x2="20" y2="19"/>
            </svg>
        }
    }

    /// GitHub icon
    #[component]
    pub fn GithubIcon(#[prop(default = "18")] size: &'static str) -> impl IntoView {
        view! {
            <svg
                xmlns="http://www.w3.org/2000/svg"
                width=size
                height=size
                viewBox="0 0 24 24"
                fill="currentColor"
                class="icon icon-github"
            >
                <path d="M12 0c-6.626 0-12 5.373-12 12 0 5.302 3.438 9.8 8.207 11.387.599.111.793-.261.793-.577v-2.234c-3.338.726-4.033-1.416-4.033-1.416-.546-1.387-1.333-1.756-1.333-1.756-1.089-.745.083-.729.083-.729 1.205.084 1.839 1.237 1.839 1.237 1.07 1.834 2.807 1.304 3.492.997.107-.775.418-1.305.762-1.604-2.665-.305-5.467-1.334-5.467-5.931 0-1.311.469-2.381 1.236-3.221-.124-.303-.535-1.524.117-3.176 0 0 1.008-.322 3.301 1.23.957-.266 1.983-.399 3.003-.404 1.02.005 2.047.138 3.006.404 2.291-1.552 3.297-1.23 3.297-1.23.653 1.653.242 2.874.118 3.176.77.84 1.235 1.911 1.235 3.221 0 4.609-2.807 5.624-5.479 5.921.43.372.823 1.102.823 2.222v3.293c0 .319.192.694.801.576 4.765-1.589 8.199-6.086 8.199-11.386 0-6.627-5.373-12-12-12z"/>
            </svg>
        }
    }

    /// Sun icon for light theme
    #[component]
    pub fn SunIcon() -> impl IntoView {
        view! {
            <svg
                xmlns="http://www.w3.org/2000/svg"
                width="18"
                height="18"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="2"
                stroke-linecap="round"
                stroke-linejoin="round"
                class="icon icon-sun"
            >
                <circle cx="12" cy="12" r="5"/>
                <line x1="12" y1="1" x2="12" y2="3"/>
                <line x1="12" y1="21" x2="12" y2="23"/>
                <line x1="4.22" y1="4.22" x2="5.64" y2="5.64"/>
                <line x1="18.36" y1="18.36" x2="19.78" y2="19.78"/>
                <line x1="1" y1="12" x2="3" y2="12"/>
                <line x1="21" y1="12" x2="23" y2="12"/>
                <line x1="4.22" y1="19.78" x2="5.64" y2="18.36"/>
                <line x1="18.36" y1="5.64" x2="19.78" y2="4.22"/>
            </svg>
        }
    }

    /// Moon icon for dark theme
    #[component]
    pub fn MoonIcon() -> impl IntoView {
        view! {
            <svg
                xmlns="http://www.w3.org/2000/svg"
                width="18"
                height="18"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="2"
                stroke-linecap="round"
                stroke-linejoin="round"
                class="icon icon-moon"
            >
                <path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z"/>
            </svg>
        }
    }

    /// Monitor icon for system theme
    #[component]
    pub fn MonitorIcon() -> impl IntoView {
        view! {
            <svg
                xmlns="http://www.w3.org/2000/svg"
                width="18"
                height="18"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="2"
                stroke-linecap="round"
                stroke-linejoin="round"
                class="icon icon-monitor"
            >
                <rect x="2" y="3" width="20" height="14" rx="2" ry="2"/>
                <line x1="8" y1="21" x2="16" y2="21"/>
                <line x1="12" y1="17" x2="12" y2="21"/>
            </svg>
        }
    }

    /// Rocket icon for getting started
    #[component]
    pub fn RocketIcon(#[prop(default = "24")] size: &'static str) -> impl IntoView {
        view! {
            <svg
                xmlns="http://www.w3.org/2000/svg"
                width=size
                height=size
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="2"
                stroke-linecap="round"
                stroke-linejoin="round"
                class="icon icon-rocket"
            >
                <path d="M4.5 16.5c-1.5 1.26-2 5-2 5s3.74-.5 5-2c.71-.84.7-2.13-.09-2.91a2.18 2.18 0 0 0-2.91-.09z"/>
                <path d="m12 15-3-3a22 22 0 0 1 2-3.95A12.88 12.88 0 0 1 22 2c0 2.72-.78 7.5-6 11a22.35 22.35 0 0 1-4 2z"/>
                <path d="M9 12H4s.55-3.03 2-4c1.62-1.08 5 0 5 0"/>
                <path d="M12 15v5s3.03-.55 4-2c1.08-1.62 0-5 0-5"/>
            </svg>
        }
    }

    /// Zap/Lightning icon for features
    #[component]
    pub fn ZapIcon(#[prop(default = "24")] size: &'static str) -> impl IntoView {
        view! {
            <svg
                xmlns="http://www.w3.org/2000/svg"
                width=size
                height=size
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="2"
                stroke-linecap="round"
                stroke-linejoin="round"
                class="icon icon-zap"
            >
                <polygon points="13 2 3 14 12 14 11 22 21 10 12 10 13 2"/>
            </svg>
        }
    }

    /// Shield icon for security
    #[component]
    pub fn ShieldIcon(#[prop(default = "24")] size: &'static str) -> impl IntoView {
        view! {
            <svg
                xmlns="http://www.w3.org/2000/svg"
                width=size
                height=size
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="2"
                stroke-linecap="round"
                stroke-linejoin="round"
                class="icon icon-shield"
            >
                <path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z"/>
            </svg>
        }
    }

    /// Network icon for overlay networks
    #[component]
    pub fn NetworkIcon(#[prop(default = "24")] size: &'static str) -> impl IntoView {
        view! {
            <svg
                xmlns="http://www.w3.org/2000/svg"
                width=size
                height=size
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="2"
                stroke-linecap="round"
                stroke-linejoin="round"
                class="icon icon-network"
            >
                <circle cx="12" cy="12" r="2"/>
                <circle cx="12" cy="5" r="2"/>
                <circle cx="19" cy="12" r="2"/>
                <circle cx="12" cy="19" r="2"/>
                <circle cx="5" cy="12" r="2"/>
                <line x1="12" y1="7" x2="12" y2="10"/>
                <line x1="17" y1="12" x2="14" y2="12"/>
                <line x1="12" y1="14" x2="12" y2="17"/>
                <line x1="10" y1="12" x2="7" y2="12"/>
            </svg>
        }
    }

    /// Hammer icon for builder
    #[component]
    pub fn HammerIcon(#[prop(default = "24")] size: &'static str) -> impl IntoView {
        view! {
            <svg
                xmlns="http://www.w3.org/2000/svg"
                width=size
                height=size
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="2"
                stroke-linecap="round"
                stroke-linejoin="round"
                class="icon icon-hammer"
            >
                <path d="m15 12-8.5 8.5c-.83.83-2.17.83-3 0 0 0 0 0 0 0a2.12 2.12 0 0 1 0-3L12 9"/>
                <path d="M17.64 15 22 10.64"/>
                <path d="m20.91 11.7-1.25-1.25c-.6-.6-.93-1.4-.93-2.25v-.86L16.01 4.6a5.56 5.56 0 0 0-3.94-1.64H9l.92.82A6.18 6.18 0 0 1 12 8.4v1.56l2 2h2.47l2.26 1.91"/>
            </svg>
        }
    }

    /// Arrow right icon
    #[component]
    pub fn ArrowRightIcon(#[prop(default = "18")] size: &'static str) -> impl IntoView {
        view! {
            <svg
                xmlns="http://www.w3.org/2000/svg"
                width=size
                height=size
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="2"
                stroke-linecap="round"
                stroke-linejoin="round"
                class="icon icon-arrow-right"
            >
                <line x1="5" y1="12" x2="19" y2="12"/>
                <polyline points="12 5 19 12 12 19"/>
            </svg>
        }
    }
}

// =============================================================================
// Theme Management
// =============================================================================

/// Theme options for the application
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Theme {
    Light,
    Dark,
    #[default]
    System,
}

impl Theme {
    /// Get the next theme in the cycle
    pub fn next(self) -> Self {
        match self {
            Theme::Light => Theme::Dark,
            Theme::Dark => Theme::System,
            Theme::System => Theme::Light,
        }
    }

    /// Get display name for the theme
    pub fn display_name(self) -> &'static str {
        match self {
            Theme::Light => "Light",
            Theme::Dark => "Dark",
            Theme::System => "System",
        }
    }

    /// Convert to string for storage
    pub fn to_storage_string(self) -> &'static str {
        match self {
            Theme::Light => "light",
            Theme::Dark => "dark",
            Theme::System => "system",
        }
    }

    /// Parse from storage string
    pub fn from_storage_string(s: &str) -> Self {
        match s {
            "light" => Theme::Light,
            "dark" => Theme::Dark,
            _ => Theme::System,
        }
    }
}

// =============================================================================
// Main Application Component
// =============================================================================

/// Main application component
#[component]
pub fn App() -> impl IntoView {
    // Create theme signal and provide it as context
    let theme = RwSignal::new(Theme::System);
    provide_context(theme);

    // Initialize theme from localStorage on client side
    #[cfg(target_arch = "wasm32")]
    {
        use leptos::task::spawn_local;
        spawn_local(async move {
            if let Some(window) = web_sys::window() {
                // Read stored theme preference
                if let Ok(Some(storage)) = window.local_storage() {
                    if let Ok(Some(stored)) = storage.get_item("zlayer-theme") {
                        theme.set(Theme::from_storage_string(&stored));
                    }
                }

                // Apply initial theme to document
                if let Some(document) = window.document() {
                    if let Some(html) = document.document_element() {
                        let current_theme = theme.get();
                        let theme_class = match current_theme {
                            Theme::Light => "light",
                            Theme::Dark => "dark",
                            Theme::System => {
                                // Check system preference
                                if let Ok(Some(mq)) =
                                    window.match_media("(prefers-color-scheme: dark)")
                                {
                                    if mq.matches() {
                                        "dark"
                                    } else {
                                        "light"
                                    }
                                } else {
                                    "light"
                                }
                            }
                        };
                        let _ = html.set_attribute("data-theme", theme_class);
                    }
                }
            }
        });
    }

    view! {
        <Router>
            <Routes fallback=|| view! { <NotFound/> }>
                <Route path=StaticSegment("") view=HomePage/>
                <Route path=StaticSegment("docs") view=DocsPage/>
                <Route path=(StaticSegment("docs"), WildcardSegment("any")) view=DocsPage/>
                <Route path=StaticSegment("playground") view=PlaygroundPage/>
            </Routes>
        </Router>
    }
}

/// 404 Not Found page
#[component]
fn NotFound() -> impl IntoView {
    view! {
        <div class="page-layout">
            <Navbar/>
            <main class="main-content not-found-page">
                <div class="container">
                    <h1>"404 - Page Not Found"</h1>
                    <p>"The page you're looking for doesn't exist."</p>
                    <a href="/" class="btn btn-primary">"Go Home"</a>
                </div>
            </main>
            <Footer/>
        </div>
    }
}

// =============================================================================
// CSS Styles
// =============================================================================

/// Generate CSS for the application
pub fn app_css() -> &'static str {
    include_str!("styles.css")
}
