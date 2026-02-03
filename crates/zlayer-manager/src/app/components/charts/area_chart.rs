//! Area chart component using leptos-chartistry
//!
//! Simple area/filled chart for displaying x,y data points with fill.

use leptos::prelude::*;
use leptos_chartistry::{AspectRatio, Chart, IntoEdge, Line, Series, TickLabels};

/// Data point for area chart
#[derive(Clone, PartialEq)]
struct DataPoint {
    x: f64,
    y: f64,
}

/// Area chart component
///
/// Displays an area chart with the given data points.
/// Note: leptos-chartistry Line does not support fill, so this renders as a styled line chart.
#[allow(clippy::needless_pass_by_value)]
#[component]
pub fn AreaChart(
    /// Vector of (x, y) data points
    data: Vec<(f64, f64)>,
    /// Chart title
    title: String,
    /// Optional fill color (CSS color string) - currently unused as library doesn't support fill
    #[prop(optional)]
    #[allow(unused_variables)]
    fill_color: Option<String>,
) -> impl IntoView {
    let points: Vec<DataPoint> = data.into_iter().map(|(x, y)| DataPoint { x, y }).collect();

    let data_signal = RwSignal::new(points);

    let bottom_edges: Vec<_> = vec![TickLabels::aligned_floats().into_edge()];
    let left_edges: Vec<_> = vec![TickLabels::aligned_floats().into_edge()];

    view! {
        <div class="area-chart">
            <h3 class="area-chart-title">{title}</h3>
            <Chart
                aspect_ratio=AspectRatio::from_outer_ratio(600.0, 300.0)
                bottom=bottom_edges
                left=left_edges
                series=Series::new(|d: &DataPoint| d.x)
                    .line(Line::new(|d: &DataPoint| d.y).with_width(2.0))
                data=data_signal
            />
        </div>
    }
}
