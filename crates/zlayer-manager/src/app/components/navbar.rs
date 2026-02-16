//! Navbar component
//!
//! Top navigation bar with hamburger menu (mobile) and theme info.

use leptos::prelude::*;

/// Navbar component for ZLayer Manager
#[component]
pub fn Navbar() -> impl IntoView {
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
                <div class="badge badge-ghost badge-sm">"mac-sandbox"</div>
                <button class="btn btn-square btn-ghost btn-sm" title="Toggle theme">
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
                </button>
            </div>
        </div>
    }
}
