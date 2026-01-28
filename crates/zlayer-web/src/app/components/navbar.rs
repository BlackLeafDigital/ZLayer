//! Navigation bar component

use leptos::prelude::*;
use leptos_router::components::A;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::JsCast;

use crate::app::icons::{BookIcon, ContainerIcon, GithubIcon, MoonIcon, MonitorIcon, SunIcon, TerminalIcon};
use crate::app::Theme;

/// Theme toggle button component
#[component]
fn ThemeToggle() -> impl IntoView {
    let theme = use_context::<RwSignal<Theme>>().expect("Theme context should be provided");

    let cycle_theme = move |_| {
        let new_theme = theme.get().next();
        theme.set(new_theme);

        // Store in localStorage (client-side only)
        #[cfg(target_arch = "wasm32")]
        {
            if let Some(window) = web_sys::window() {
                if let Ok(Some(storage)) = window.local_storage() {
                    let _ = storage.set_item("zlayer-theme", new_theme.to_storage_string());
                }
            }
        }

        // Apply theme to document
        #[cfg(target_arch = "wasm32")]
        {
            if let Some(window) = web_sys::window() {
                if let Some(document) = window.document() {
                    if let Some(html) = document.document_element() {
                        let theme_class = match new_theme {
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
        }
    };

    view! {
        <button
            class="theme-toggle"
            on:click=cycle_theme
            title=move || format!("Theme: {}", theme.get().display_name())
        >
            {move || match theme.get() {
                Theme::Light => view! { <SunIcon/> }.into_any(),
                Theme::Dark => view! { <MoonIcon/> }.into_any(),
                Theme::System => view! { <MonitorIcon/> }.into_any(),
            }}
        </button>
    }
}

/// Main navigation bar component
#[component]
pub fn Navbar() -> impl IntoView {
    view! {
        <nav class="navbar">
            <div class="container navbar-content">
                <A href="/" attr:class="navbar-logo">
                    <ContainerIcon size="28"/>
                    <span>"ZLayer"</span>
                </A>

                <div class="navbar-nav">
                    <A href="/docs" attr:class="nav-link">
                        <BookIcon size="16"/>
                        <span>"Docs"</span>
                    </A>
                    <A href="/playground" attr:class="nav-link">
                        <TerminalIcon size="16"/>
                        <span>"Playground"</span>
                    </A>
                </div>

                <div class="navbar-actions">
                    <ThemeToggle/>
                    <a
                        href="https://github.com/zachhandley/ZLayer"
                        target="_blank"
                        rel="noopener noreferrer"
                        class="github-link"
                    >
                        <GithubIcon size="18"/>
                        <span>"GitHub"</span>
                    </a>
                </div>
            </div>
        </nav>
    }
}
