//! Builds page
//!
//! Displays build history with management capabilities.
//! Supports triggering new builds and viewing build logs.

use leptos::prelude::*;

use crate::app::server_fns::{get_build_logs, get_builds, trigger_build, Build, BuildState};

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
fn render_builds_table(
    builds: Vec<Build>,
    on_view_logs: impl Fn(String) + 'static + Clone,
) -> impl IntoView {
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
                            let build_id = build.id.clone();
                            let on_view_logs = on_view_logs.clone();

                            // Show error info if build failed
                            let error_display = build.error.clone();

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
                                        {error_display.map(|err| {
                                            let title = err.clone();
                                            let display = if err.len() > 30 {
                                                format!("{}...", &err[..30])
                                            } else {
                                                err
                                            };
                                            view! {
                                                <span class="text-error text-xs ml-2" title=title>
                                                    {display}
                                                </span>
                                            }
                                        })}
                                    </td>
                                    <td>{build.started_at}</td>
                                    <td>{completed_display}</td>
                                    <td>
                                        <button
                                            class="btn btn-sm btn-ghost"
                                            on:click=move |_| on_view_logs(build_id.clone())
                                        >
                                            "View Logs"
                                        </button>
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
    let (refresh, set_refresh) = signal(0u32);
    let builds = Resource::new(move || refresh.get(), |_| get_builds());

    // Modal states
    let (show_trigger_modal, set_show_trigger_modal) = signal(false);
    let (show_logs_modal, set_show_logs_modal) = signal(false);
    let (logs_content, set_logs_content) = signal(String::new());
    let (logs_loading, set_logs_loading) = signal(false);
    let (error_msg, set_error_msg) = signal(Option::<String>::None);

    // Form fields
    let (context_path, set_context_path) = signal(String::new());
    let (tags, set_tags) = signal(String::new());
    let (runtime, set_runtime) = signal(String::new());

    // Trigger build action
    let trigger_action = Action::new(move |input: &(String, String, String)| {
        let (context_path, tags, runtime) = input.clone();
        async move {
            let result = trigger_build(context_path, tags, runtime).await;
            match result {
                Ok(_build_id) => {
                    set_show_trigger_modal.set(false);
                    set_context_path.set(String::new());
                    set_tags.set(String::new());
                    set_runtime.set(String::new());
                    set_error_msg.set(None);
                    set_refresh.update(|n| *n += 1);
                }
                Err(e) => {
                    set_error_msg.set(Some(e.to_string()));
                }
            }
        }
    });

    // View logs action
    let view_logs_action = Action::new(move |build_id: &String| {
        let build_id = build_id.clone();
        async move {
            set_logs_loading.set(true);
            set_logs_content.set(String::new());
            set_show_logs_modal.set(true);
            match get_build_logs(build_id).await {
                Ok(logs) => {
                    set_logs_content.set(if logs.is_empty() {
                        "No logs available.".to_string()
                    } else {
                        logs
                    });
                }
                Err(e) => {
                    set_logs_content.set(format!("Error fetching logs: {}", e));
                }
            }
            set_logs_loading.set(false);
        }
    });

    view! {
        <div class="container mx-auto p-6">
            <div class="flex justify-between items-center mb-6">
                <h1 class="text-3xl font-bold">"Builds"</h1>
                <button
                    class="btn btn-primary"
                    on:click=move |_| {
                        set_error_msg.set(None);
                        set_context_path.set(String::new());
                        set_tags.set(String::new());
                        set_runtime.set(String::new());
                        set_show_trigger_modal.set(true);
                    }
                >
                    "Trigger Build"
                </button>
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
                                            let on_view_logs = move |id: String| {
                                                view_logs_action.dispatch(id);
                                            };
                                            render_builds_table(build_list, on_view_logs).into_any()
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

            // Trigger Build Modal
            <div class=move || {
                if show_trigger_modal.get() { "modal modal-open" } else { "modal" }
            }>
                <div class="modal-box w-11/12 max-w-2xl">
                    <h3 class="font-bold text-lg">"Trigger Build"</h3>

                    {move || {
                        error_msg
                            .get()
                            .map(|msg| {
                                view! {
                                    <div class="alert alert-error mb-4">
                                        <span>{msg}</span>
                                    </div>
                                }
                            })
                    }}

                    <div class="form-control w-full mt-4">
                        <label class="label">
                            <span class="label-text">"Context Path" <span class="text-error">"*"</span></span>
                        </label>
                        <input
                            type="text"
                            class="input input-bordered w-full"
                            placeholder="/path/to/build/context"
                            prop:value=move || context_path.get()
                            on:input=move |ev| set_context_path.set(event_target_value(&ev))
                        />
                        <label class="label">
                            <span class="label-text-alt text-base-content/50">"Server-side path to the build context directory"</span>
                        </label>
                    </div>

                    <div class="form-control w-full mt-2">
                        <label class="label">
                            <span class="label-text">"Tags"</span>
                        </label>
                        <input
                            type="text"
                            class="input input-bordered w-full"
                            placeholder="myapp:latest, myapp:v1.0"
                            prop:value=move || tags.get()
                            on:input=move |ev| set_tags.set(event_target_value(&ev))
                        />
                        <label class="label">
                            <span class="label-text-alt text-base-content/50">"Comma-separated list of image tags"</span>
                        </label>
                    </div>

                    <div class="form-control w-full mt-2">
                        <label class="label">
                            <span class="label-text">"Runtime Template (optional)"</span>
                        </label>
                        <input
                            type="text"
                            class="input input-bordered w-full"
                            placeholder="e.g. node, python, rust"
                            prop:value=move || runtime.get()
                            on:input=move |ev| set_runtime.set(event_target_value(&ev))
                        />
                        <label class="label">
                            <span class="label-text-alt text-base-content/50">"Use a runtime template instead of a Dockerfile"</span>
                        </label>
                    </div>

                    <div class="modal-action">
                        <button
                            class="btn"
                            on:click=move |_| set_show_trigger_modal.set(false)
                        >
                            "Cancel"
                        </button>
                        <button
                            class="btn btn-primary"
                            prop:disabled=move || context_path.get().trim().is_empty()
                            on:click=move |_| {
                                trigger_action
                                    .dispatch((
                                        context_path.get(),
                                        tags.get(),
                                        runtime.get(),
                                    ));
                            }
                        >
                            "Start Build"
                        </button>
                    </div>
                </div>
                <div
                    class="modal-backdrop"
                    on:click=move |_| set_show_trigger_modal.set(false)
                ></div>
            </div>

            // View Logs Modal
            <div class=move || {
                if show_logs_modal.get() { "modal modal-open" } else { "modal" }
            }>
                <div class="modal-box w-11/12 max-w-4xl">
                    <h3 class="font-bold text-lg">"Build Logs"</h3>
                    <div class="mt-4">
                        {move || {
                            if logs_loading.get() {
                                view! {
                                    <div class="flex justify-center p-8">
                                        <span class="loading loading-spinner loading-lg"></span>
                                    </div>
                                }
                                    .into_any()
                            } else {
                                view! {
                                    <pre class="bg-base-300 rounded-lg p-4 overflow-auto max-h-96 text-sm font-mono whitespace-pre-wrap">
                                        {move || logs_content.get()}
                                    </pre>
                                }
                                    .into_any()
                            }
                        }}
                    </div>
                    <div class="modal-action">
                        <button
                            class="btn"
                            on:click=move |_| set_show_logs_modal.set(false)
                        >
                            "Close"
                        </button>
                    </div>
                </div>
                <div
                    class="modal-backdrop"
                    on:click=move |_| set_show_logs_modal.set(false)
                ></div>
            </div>
        </div>
    }
}
