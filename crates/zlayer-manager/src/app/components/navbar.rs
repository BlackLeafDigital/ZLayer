//! Navbar component
//!
//! Top navigation bar with hamburger menu (mobile) and theme info.

use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::app::server_fns::{get_runtime_name, manager_logout, manager_me};

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
#[allow(clippy::too_many_lines)] // view macro DSL; matches Sidebar's pattern
pub fn Navbar() -> impl IntoView {
    let runtime = Resource::new(|| (), |()| get_runtime_name());
    let me = Resource::new(|| (), |()| async move { manager_me().await });

    let on_logout = move |_| {
        spawn_local(async move {
            let _ = manager_logout().await;
            if let Some(w) = web_sys::window() {
                let _ = w.location().set_href("/login");
            }
        });
    };

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
                <Suspense fallback=|| ().into_any()>
                    {move || {
                        match me.get() {
                            Some(Ok(resp)) => {
                                if let Some(user) = resp.user {
                                    view! {
                                        <div class="dropdown dropdown-end">
                                            <div
                                                tabindex="0"
                                                role="button"
                                                class="btn btn-ghost btn-sm gap-2"
                                            >
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
                                                        d="M15.75 6a3.75 3.75 0 11-7.5 0 3.75 3.75 0 017.5 0zM4.501 20.118a7.5 7.5 0 0114.998 0A17.933 17.933 0 0112 21.75c-2.676 0-5.216-.584-7.499-1.632z"
                                                    />
                                                </svg>
                                                <span class="hidden sm:inline">
                                                    {user.display_name.clone()}
                                                </span>
                                            </div>
                                            <ul
                                                tabindex="0"
                                                class="dropdown-content menu bg-base-200 rounded-box z-40 w-56 p-2 shadow"
                                            >
                                                <li class="menu-title">
                                                    <span class="text-xs">{user.email.clone()}</span>
                                                </li>
                                                <li>
                                                    <button on:click=on_logout>
                                                        <svg
                                                            xmlns="http://www.w3.org/2000/svg"
                                                            fill="none"
                                                            viewBox="0 0 24 24"
                                                            stroke-width="1.5"
                                                            stroke="currentColor"
                                                            class="w-4 h-4"
                                                        >
                                                            <path
                                                                stroke-linecap="round"
                                                                stroke-linejoin="round"
                                                                d="M15.75 9V5.25A2.25 2.25 0 0013.5 3h-6a2.25 2.25 0 00-2.25 2.25v13.5A2.25 2.25 0 007.5 21h6a2.25 2.25 0 002.25-2.25V15M12 9l-3 3m0 0l3 3m-3-3h12.75"
                                                            />
                                                        </svg>
                                                        "Sign out"
                                                    </button>
                                                </li>
                                            </ul>
                                        </div>
                                    }
                                        .into_any()
                                } else {
                                    ().into_any()
                                }
                            }
                            _ => ().into_any(),
                        }
                    }}
                </Suspense>
            </div>
        </div>
    }
}
