//! Builds page
//!
//! Displays build history with management capabilities.

use leptos::prelude::*;

use crate::app::server_fns::{get_builds, Build, BuildState};

/// Get the CSS class for a build status badge
fn status_badge_class(status: BuildState) -> &'static str {
    match status {
        BuildState::Complete => "badge badge-success",
        BuildState::Failed => "badge badge-error",
        BuildState::Running => "badge badge-warning",
        BuildState::Pending => "badge badge-ghost",
    }
}

/// Get display text for a build status
fn status_display_text(status: BuildState) -> &'static str {
    match status {
        BuildState::Complete => "Complete",
        BuildState::Failed => "Failed",
        BuildState::Running => "Running",
        BuildState::Pending => "Pending",
    }
}

/// Render builds table from data
fn render_builds_table(builds: Vec<Build>) -> impl IntoView {
    view! {
        <div class="overflow-x-auto">
            <table class="table table-zebra w-full">
                <thead>
                    <tr>
                        <th>"ID"</th>
                        <th>"Image"</th>
                        <th>"Status"</th>
                        <th>"Started"</th>
                        <th>"Completed"</th>
                        <th>"Actions"</th>
                    </tr>
                </thead>
                <tbody>
                    {builds
                        .into_iter()
                        .map(|build| {
                            let status_class = status_badge_class(build.status);
                            let status_text = status_display_text(build.status);
                            let image_display = build.image_id.clone().map_or_else(
                                || "-".to_string(),
                                |id| {
                                    if id.len() > 12 {
                                        id[..12].to_string()
                                    } else {
                                        id
                                    }
                                },
                            );
                            let completed_display = build
                                .completed_at
                                .clone()
                                .unwrap_or_else(|| "-".to_string());
                            view! {
                                <tr>
                                    <td class="font-mono text-sm">
                                        {if build.id.len() > 8 {
                                            build.id[..8].to_string()
                                        } else {
                                            build.id.clone()
                                        }}
                                    </td>
                                    <td class="font-mono">{image_display}</td>
                                    <td>
                                        <span class=status_class>{status_text}</span>
                                    </td>
                                    <td>{build.started_at}</td>
                                    <td>{completed_display}</td>
                                    <td>
                                        <button class="btn btn-sm btn-ghost">"View Logs"</button>
                                        <button class="btn btn-sm btn-ghost">"Rebuild"</button>
                                        <button class="btn btn-sm btn-ghost text-error">"Cancel"</button>
                                    </td>
                                </tr>
                            }
                        })
                        .collect::<Vec<_>>()}
                </tbody>
            </table>
        </div>
    }
}

/// Builds page component
#[component]
pub fn Builds() -> impl IntoView {
    let builds = Resource::new(|| (), |()| get_builds());

    view! {
        <div class="container mx-auto p-6">
            <div class="flex justify-between items-center mb-6">
                <h1 class="text-3xl font-bold">"Builds"</h1>
                <button class="btn btn-primary">"Trigger Build"</button>
            </div>

            <Suspense fallback=move || {
                view! {
                    <div class="flex justify-center p-8">
                        <span class="loading loading-spinner loading-lg"></span>
                    </div>
                }
            }>
                <ErrorBoundary fallback=|errors| {
                    view! {
                        <div class="alert alert-error">
                            <span>{move || format!("Error: {:?}", errors.get())}</span>
                        </div>
                    }
                }>
                    {move || {
                        builds
                            .get()
                            .map(|result| {
                                match result {
                                    Ok(build_list) => {
                                        if build_list.is_empty() {
                                            view! {
                                                <div class="alert alert-info">
                                                    <span>"No builds found. Trigger your first build!"</span>
                                                </div>
                                            }
                                                .into_any()
                                        } else {
                                            render_builds_table(build_list).into_any()
                                        }
                                    }
                                    Err(e) => {
                                        view! {
                                            <div class="alert alert-error">{e.to_string()}</div>
                                        }
                                            .into_any()
                                    }
                                }
                            })
                    }}
                </ErrorBoundary>
            </Suspense>
        </div>
    }
}
