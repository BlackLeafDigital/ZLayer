//! Line chart component using leptos-chartistry
//!
//! Simple line chart for displaying x,y data points.

use leptos::prelude::*;
use leptos_chartistry::{AspectRatio, Chart, IntoEdge, Line, RotatedLabel, Series, TickLabels};

/// Data point for line chart
#[derive(Clone, PartialEq)]
struct DataPoint {
    x: f64,
    y: f64,
}

/// Line chart component
///
/// Displays a line chart with the given data points.
#[component]
pub fn LineChart(
    /// Vector of (x, y) data points
    data: Vec<(f64, f64)>,
    /// Chart title
    title: String,
    /// Optional X-axis label
    #[prop(optional)]
    x_label: Option<String>,
    /// Optional Y-axis label
    #[prop(optional)]
    y_label: Option<String>,
) -> impl IntoView {
    let points: Vec<DataPoint> = data.into_iter().map(|(x, y)| DataPoint { x, y }).collect();

    let data_signal = RwSignal::new(points);

    let mut bottom_edges: Vec<_> = vec![TickLabels::aligned_floats().into_edge()];
    if let Some(label) = x_label {
        bottom_edges.push(RotatedLabel::middle(label).into_edge());
    }

    let mut left_edges: Vec<_> = vec![TickLabels::aligned_floats().into_edge()];
    if let Some(label) = y_label {
        left_edges.push(RotatedLabel::middle(label).into_edge());
    }

    view! {
        <div class="line-chart">
            <h3 class="line-chart-title">{title}</h3>
            <Chart
                aspect_ratio=AspectRatio::from_outer_ratio(600.0, 300.0)
                bottom=bottom_edges
                left=left_edges
                series=Series::new(|d: &DataPoint| d.x)
                    .line(Line::new(|d: &DataPoint| d.y))
                data=data_signal
            />
        </div>
    }
}
