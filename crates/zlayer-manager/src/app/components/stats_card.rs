//! Stats card component
//!
//! Simple stat display using `DaisyUI` stat classes.

use leptos::prelude::*;

/// Stats card component
///
/// Displays a stat with title, value, and optional icon using DaisyUI styling.
#[component]
pub fn StatsCard(
    /// Stat title (e.g., "Total Downloads")
    title: String,
    /// Stat value to display (e.g., "31K")
    value: String,
    /// Optional icon (rendered as inner HTML, e.g., SVG string)
    #[prop(optional)]
    icon: Option<String>,
) -> impl IntoView {
    view! {
        <div class="stat">
            {icon.map(|i| view! {
                    <div class="stat-figure text-primary" inner_html=i></div>
                })}
            <div class="stat-title">{title}</div>
            <div class="stat-value">{value}</div>
        </div>
    }
}
