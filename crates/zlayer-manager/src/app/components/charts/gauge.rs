//! Gauge/Progress component
//!
//! Simple circular gauge using DaisyUI radial-progress.

use leptos::prelude::*;

/// Circular gauge/progress component
///
/// Displays a percentage value as a radial progress indicator.
/// Uses DaisyUI's radial-progress component.
#[component]
pub fn Gauge(
    /// Progress value (0-100)
    value: f64,
    /// Label displayed below the gauge
    label: String,
    /// Optional CSS color (defaults based on value: green > 66, yellow > 33, red otherwise)
    #[prop(optional)]
    color: Option<String>,
) -> impl IntoView {
    let clamped = value.clamp(0.0, 100.0);
    // Safe: clamped is 0.0-100.0, so rounded is 0-100 which fits in i32
    #[allow(clippy::cast_possible_truncation)]
    let rounded = clamped.round() as i32;

    let color_value = color.unwrap_or_else(|| {
        if clamped >= 66.0 {
            "oklch(var(--su))".to_string() // success green
        } else if clamped >= 33.0 {
            "oklch(var(--wa))".to_string() // warning yellow
        } else {
            "oklch(var(--er))".to_string() // error red
        }
    });

    let style = format!("--value:{rounded}; --size:6rem; color:{color_value};");

    view! {
        <div class="flex flex-col items-center gap-2">
            <div
                class="radial-progress"
                style=style
                role="progressbar"
                aria-valuenow=rounded
                aria-valuemin=0
                aria-valuemax=100
            >
                {format!("{rounded}%")}
            </div>
            <span class="text-sm font-medium">{label}</span>
        </div>
    }
}
