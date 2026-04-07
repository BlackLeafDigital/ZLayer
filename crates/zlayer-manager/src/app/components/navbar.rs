//! Navbar component
//!
//! Top navigation bar with hamburger menu (mobile) and theme info.

use leptos::prelude::*;

use crate::app::server_fns::get_runtime_name;

const THEME_STORAGE_KEY: &str = "zlm-theme";
const THEME_DARK: &str = "dark";
const THEME_LIGHT: &str = "zlayer";

/// Apply a theme to the `<html>` element and persist it to localStorage.
fn apply_theme(theme: &str) {
    if let Some(window) = web_sys::window() {
        if let Some(doc) = window.document() {
            if let Some(html) = doc.document_element() {
                let _ = html.set_attribute("data-theme", theme);
            }
        }
        if let Ok(Some(storage)) = window.local_storage() {
            let _ = storage.set_item(THEME_STORAGE_KEY, theme);
        }
    }
}

/// Read the saved theme from localStorage, falling back to `"dark"`.
fn read_saved_theme() -> String {
    web_sys::window()
        .and_then(|w| w.local_storage().ok().flatten())
        .and_then(|s| s.get_item(THEME_STORAGE_KEY).ok().flatten())
        .unwrap_or_else(|| THEME_DARK.to_string())
}

/// Navbar component for ZLayer Manager
#[component]
pub fn Navbar() -> impl IntoView {
    let runtime = Resource::new(|| (), |()| get_runtime_name());

    // true = dark theme, false = light ("zlayer") theme
    let is_dark = RwSignal::new(true);

    // On hydration, read the persisted theme and apply it.
    Effect::new(move |_| {
        let saved = read_saved_theme();
        let dark = saved == THEME_DARK;
        is_dark.set(dark);
        apply_theme(&saved);
    });

    let toggle_theme = move |_| {
        let new_dark = !is_dark.get_untracked();
        is_dark.set(new_dark);
        apply_theme(if new_dark { THEME_DARK } else { THEME_LIGHT });
    };

    view! {
        <div class="navbar bg-base-200 border-b border-base-300 sticky top-0 z-30">
            <div class="flex-none lg:hidden">
                <label for="sidebar-drawer" class="btn btn-square btn-ghost">
                    <svg xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" class="inline-block w-6 h-6 stroke-current">
                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 6h16M4 12h16M4 18h16"></path>
                    </svg>
                </label>
            </div>
            <div class="flex-1">
                <span class="text-lg font-semibold lg:hidden">"ZLayer"</span>
            </div>
            <div class="flex-none gap-2">
                <Suspense fallback=move || view! { <div class="badge badge-ghost badge-sm">"..."</div> }>
                    {move || {
                        runtime
                            .get()
                            .map(|result| {
                                let name = result.unwrap_or_else(|_| "auto".to_string());
                                view! { <div class="badge badge-ghost badge-sm">{name}</div> }
                            })
                    }}
                </Suspense>
                <button
                    class="btn btn-square btn-ghost btn-sm"
                    title="Toggle theme"
                    on:click=toggle_theme
                >
                    <Show
                        when=move || is_dark.get()
                        fallback=move || {
                            view! {
                                // Moon icon – shown in light mode, click to go dark
                                <svg
                                    xmlns="http://www.w3.org/2000/svg"
                                    fill="none"
                                    viewBox="0 0 24 24"
                                    stroke-width="1.5"
                                    stroke="currentColor"
                                    class="w-5 h-5"
                                >
                                    <path
                                        stroke-linecap="round"
                                        stroke-linejoin="round"
                                        d="M21.752 15.002A9.72 9.72 0 0118 15.75c-5.385 0-9.75-4.365-9.75-9.75 0-1.33.266-2.597.748-3.752A9.753 9.753 0 003 11.25C3 16.635 7.365 21 12.75 21a9.753 9.753 0 009.002-5.998z"
                                    />
                                </svg>
                            }
                        }
                    >
                        // Sun icon – shown in dark mode, click to go light
                        <svg
                            xmlns="http://www.w3.org/2000/svg"
                            fill="none"
                            viewBox="0 0 24 24"
                            stroke-width="1.5"
                            stroke="currentColor"
                            class="w-5 h-5"
                        >
                            <path
                                stroke-linecap="round"
                                stroke-linejoin="round"
                                d="M12 3v2.25m6.364.386l-1.591 1.591M21 12h-2.25m-.386 6.364l-1.591-1.591M12 18.75V21m-4.773-4.227l-1.591 1.591M5.25 12H3m4.227-4.773L5.636 5.636M15.75 12a3.75 3.75 0 11-7.5 0 3.75 3.75 0 017.5 0z"
                            />
                        </svg>
                    </Show>
                </button>
            </div>
        </div>
    }
}
