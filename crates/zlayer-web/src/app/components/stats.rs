//! Stats UI components for displaying metrics and service information
//!
//! Leptos components take owned values by design for reactivity, so we allow
//! `needless_pass_by_value` warnings throughout this module.

#![allow(clippy::needless_pass_by_value, clippy::must_use_candidate)]

use leptos::prelude::*;

use crate::app::server_fns::ServiceStats;

/// A card displaying a single metric value with an optional icon
///
/// # Props
/// - `value`: The metric value to display prominently
/// - `label`: A descriptive label shown below the value
/// - `icon`: Optional CSS class for an icon (e.g., "icon-cpu", "icon-memory")
#[component]
pub fn StatsCard(
    /// The metric value to display
    value: String,
    /// Descriptive label for the metric
    label: String,
    /// Optional CSS class for an icon
    #[prop(optional)]
    icon: Option<String>,
) -> impl IntoView {
    view! {
        <div class="stats-card">
            {icon.map(|icon_class| {
                view! { <span class={format!("stats-card-icon {icon_class}")}></span> }
            })}
            <span class="stats-card-value">{value}</span>
            <span class="stats-card-label">{label}</span>
        </div>
    }
}

/// A progress bar for displaying resource usage (CPU, memory, etc.)
///
/// # Props
/// - `label`: The resource name (e.g., "CPU", "Memory")
/// - `value`: Usage percentage from 0.0 to 100.0
/// - `color`: Optional custom color for the progress bar
#[component]
pub fn ResourceMeter(
    /// The resource label (e.g., "CPU", "Memory")
    label: String,
    /// Usage percentage (0.0 to 100.0)
    value: f64,
    /// Optional custom color for the progress bar fill
    #[prop(optional)]
    color: Option<String>,
) -> impl IntoView {
    // Clamp value to valid percentage range
    let clamped_value = value.clamp(0.0, 100.0);
    let percentage_text = format!("{clamped_value:.1}%");
    let width_style = format!("width: {clamped_value}%");

    // Determine color class based on value thresholds if no custom color provided
    let color_class = color.unwrap_or_else(|| {
        if clamped_value >= 90.0 {
            "resource-meter-fill--critical".to_string()
        } else if clamped_value >= 70.0 {
            "resource-meter-fill--warning".to_string()
        } else {
            "resource-meter-fill--normal".to_string()
        }
    });

    view! {
        <div class="resource-meter">
            <div class="resource-meter-header">
                <span class="resource-meter-label">{label}</span>
                <span class="resource-meter-percentage">{percentage_text}</span>
            </div>
            <div class="resource-meter-bar">
                <div
                    class={format!("resource-meter-fill {color_class}")}
                    style={width_style}
                ></div>
            </div>
        </div>
    }
}

/// A status badge indicating health state
///
/// # Props
/// - `status`: Health status string ("healthy", "unhealthy", "degraded", "unknown", "checking")
#[component]
pub fn HealthBadge(
    /// Health status: "healthy", "unhealthy", "degraded", "unknown", or "checking"
    status: String,
) -> impl IntoView {
    let status_lower = status.to_lowercase();
    let modifier_class = match status_lower.as_str() {
        "healthy" => "health-badge--healthy",
        "unhealthy" => "health-badge--unhealthy",
        "degraded" => "health-badge--degraded",
        "checking" => "health-badge--checking",
        _ => "health-badge--unknown",
    };

    let display_text = match status_lower.as_str() {
        "healthy" => "Healthy",
        "unhealthy" => "Unhealthy",
        "degraded" => "Degraded",
        "checking" => "Checking",
        "unknown" => "Unknown",
        _ => &status,
    };

    view! {
        <span class={format!("health-badge {modifier_class}")}>
            {display_text.to_string()}
        </span>
    }
}

/// A table row displaying service statistics
///
/// # Props
/// - `stats`: ServiceStats containing name, replicas, CPU%, memory%, and health status
#[component]
pub fn ServiceRow(
    /// Service statistics to display
    stats: ServiceStats,
) -> impl IntoView {
    let cpu_text = format!("{:.1}%", stats.cpu_percent);
    let memory_text = format!("{:.1}%", stats.memory_percent);
    let replicas_text = format!("{}", stats.replicas);

    view! {
        <tr class="service-row">
            <td class="service-row-name">{stats.name}</td>
            <td class="service-row-replicas">{replicas_text}</td>
            <td class="service-row-cpu">{cpu_text}</td>
            <td class="service-row-memory">{memory_text}</td>
            <td class="service-row-health">
                <HealthBadge status={stats.health_status} />
            </td>
        </tr>
    }
}

/// A complete services table with header and rows
///
/// # Props
/// - `services`: Vector of ServiceStats to display
#[component]
pub fn ServicesTable(
    /// List of services to display
    services: Vec<ServiceStats>,
) -> impl IntoView {
    view! {
        <table class="services-table">
            <thead>
                <tr class="services-table-header">
                    <th>"Service"</th>
                    <th>"Replicas"</th>
                    <th>"CPU"</th>
                    <th>"Memory"</th>
                    <th>"Health"</th>
                </tr>
            </thead>
            <tbody>
                {services.into_iter().map(|service| {
                    view! { <ServiceRow stats={service} /> }
                }).collect::<Vec<_>>()}
            </tbody>
        </table>
    }
}
