//! Deployments page
//!
//! Displays deployment list with `DaisyUI` table styling.

use leptos::prelude::*;

use crate::app::server_fns::{get_deployments, Deployment};

/// Render deployments table from data
fn render_deployments_table(deployments: Vec<Deployment>) -> impl IntoView {
    view! {
        <div class="overflow-x-auto">
            <table class="table table-zebra w-full">
                <thead>
                    <tr>
                        <th>"Name"</th>
                        <th>"Status"</th>
                        <th>"Replicas"</th>
                        <th>"Created"</th>
                        <th>"Actions"</th>
                    </tr>
                </thead>
                <tbody>
                    {deployments
                        .into_iter()
                        .map(|d| {
                            let status_class = match d.status.as_str() {
                                "running" => "badge badge-success",
                                "pending" => "badge badge-warning",
                                _ => "badge badge-ghost",
                            };
                            view! {
                                <tr>
                                    <td class="font-medium">{d.name}</td>
                                    <td>
                                        <span class=status_class>{d.status}</span>
                                    </td>
                                    <td>{format!("{}/{}", d.replicas, d.target_replicas)}</td>
                                    <td>{d.created_at}</td>
                                    <td>
                                        <button class="btn btn-ghost btn-xs">"Edit"</button>
                                        <button class="btn btn-ghost btn-xs text-error">"Delete"</button>
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

/// Deployments page component
///
/// Displays a table of deployments with a "New Deployment" button.
#[component]
pub fn Deployments() -> impl IntoView {
    let deployments = Resource::new(|| (), |()| get_deployments());

    view! {
        <div class="p-6">
            <div class="flex justify-between items-center mb-6">
                <h1 class="text-3xl font-bold">"Deployments"</h1>
                <button class="btn btn-primary">"New Deployment"</button>
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
                        deployments
                            .get()
                            .map(|result| {
                                match result {
                                    Ok(deps) => {
                                        if deps.is_empty() {
                                            view! {
                                                <div class="alert alert-info">
                                                    <span>"No deployments found. Create your first deployment!"</span>
                                                </div>
                                            }
                                                .into_any()
                                        } else {
                                            render_deployments_table(deps).into_any()
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
