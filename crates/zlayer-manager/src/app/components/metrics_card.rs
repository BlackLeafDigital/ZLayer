//! Metrics card component
//!
//! Displays a single metric with label, value, and optional change indicator.

use leptos::prelude::*;

/// Metric display card component
///
/// Shows a metric with its label, value, and optional change indicator.
/// Positive changes are shown in green, negative in red.
#[component]
pub fn MetricsCard(
    /// Metric label (e.g., "Total Users")
    label: String,
    /// Metric value to display (e.g., "1,234")
    value: String,
    /// Optional percentage change (e.g., Some(12.5) for +12.5%, Some(-5.0) for -5%)
    change: Option<f64>,
    /// Optional subtitle text
    #[prop(optional)]
    subtitle: Option<String>,
    /// Optional CSS class for custom styling
    #[prop(optional)]
    class: Option<String>,
) -> impl IntoView {
    let change_class = match change {
        Some(c) if c > 0.0 => "metrics-card-change metrics-card-change-positive",
        Some(c) if c < 0.0 => "metrics-card-change metrics-card-change-negative",
        _ => "metrics-card-change metrics-card-change-neutral",
    };

    let change_text = match change {
        Some(c) if c > 0.0 => format!("+{c:.1}%"),
        Some(c) => format!("{c:.1}%"),
        None => String::new(),
    };

    let base_class = class.unwrap_or_else(|| String::from("metrics-card"));

    view! {
        <div class=base_class>
            <div class="metrics-card-header">
                <span class="metrics-card-label">{label}</span>
                {change.map(|_| view! {
                        <span class=change_class>{change_text}</span>
                    })}
            </div>
            <div class="metrics-card-value">{value}</div>
            {subtitle.map(|s| view! {
                        <div class="metrics-card-subtitle">{s}</div>
                    })}
        </div>
    }
}
