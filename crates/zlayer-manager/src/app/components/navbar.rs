//! Navbar component
//!
//! Top navigation bar with logo/title and theme toggle placeholder.

use leptos::prelude::*;

/// Navbar component for ZLayer Manager
///
/// Displays the application title and a theme toggle button placeholder.
#[component]
pub fn Navbar() -> impl IntoView {
    view! {
        <div class="navbar bg-base-100 shadow-sm">
            <div class="flex-1">
                <a class="btn btn-ghost text-xl">ZLayer Manager</a>
            </div>
            <div class="flex-none">
                <button class="btn btn-square btn-ghost" title="Toggle theme">
                    <svg
                        xmlns="http://www.w3.org/2000/svg"
                        fill="none"
                        viewBox="0 0 24 24"
                        stroke-width="1.5"
                        stroke="currentColor"
                        class="w-6 h-6"
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
