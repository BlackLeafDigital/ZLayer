//! Jobs & Cron page
//!
//! Displays cron job schedules with enable/disable toggles and manual triggers.
//! Shows recent job executions with status badges and log viewing.

use leptos::prelude::*;

use crate::app::server_fns::{
    disable_cron_job, enable_cron_job, get_cron_jobs, list_job_executions, trigger_cron_job,
    trigger_job, CronJob, JobExecution,
};

/// Get the CSS class for a job execution status badge
fn execution_status_badge(status: &str) -> &'static str {
    match status {
        "completed" => "badge badge-success",
        "failed" | "cancelled" => "badge badge-error",
        "running" => "badge badge-warning",
        _ => "badge badge-ghost",
    }
}

/// Format an optional duration in milliseconds for display
#[allow(clippy::cast_precision_loss)]
fn format_duration_ms(ms: Option<u64>) -> String {
    match ms {
        Some(d) if d < 1_000 => format!("{d}ms"),
        Some(d) if d < 60_000 => format!("{:.1}s", d as f64 / 1_000.0),
        Some(d) => format!("{:.1}m", d as f64 / 60_000.0),
        None => "-".to_string(),
    }
}

/// Render the cron jobs table
fn render_cron_table(
    cron_jobs: Vec<CronJob>,
    on_trigger: impl Fn(String) + 'static + Clone,
    on_toggle: impl Fn((String, bool)) + 'static + Clone,
) -> impl IntoView {
    view! {
        <div class="overflow-x-auto">
            <table class="table table-zebra w-full">
                <thead>
                    <tr>
                        <th>"Name"</th>
                        <th>"Schedule"</th>
                        <th>"Status"</th>
                        <th>"Last Run"</th>
                        <th>"Next Run"</th>
                        <th>"Actions"</th>
                    </tr>
                </thead>
                <tbody>
                    {cron_jobs
                        .into_iter()
                        .map(|job| {
                            let name_trigger = job.name.clone();
                            let name_toggle = job.name.clone();
                            let on_trigger = on_trigger.clone();
                            let on_toggle = on_toggle.clone();
                            let is_enabled = job.enabled;
                            let status_class = if job.enabled {
                                "badge badge-success"
                            } else {
                                "badge badge-ghost"
                            };
                            let status_text = if job.enabled { "Enabled" } else { "Disabled" };
                            let last_run = job
                                .last_run
                                .clone()
                                .unwrap_or_else(|| "-".to_string());
                            let next_run = job
                                .next_run
                                .clone()
                                .unwrap_or_else(|| "-".to_string());
                            view! {
                                <tr>
                                    <td class="font-medium font-mono">{job.name}</td>
                                    <td class="font-mono text-sm">{job.schedule}</td>
                                    <td>
                                        <span class=status_class>{status_text}</span>
                                    </td>
                                    <td class="text-sm">{last_run}</td>
                                    <td class="text-sm">{next_run}</td>
                                    <td class="flex gap-1">
                                        <button
                                            class="btn btn-sm btn-ghost"
                                            on:click=move |_| on_trigger(name_trigger.clone())
                                        >
                                            "Trigger"
                                        </button>
                                        <button
                                            class=move || {
                                                if is_enabled {
                                                    "btn btn-sm btn-ghost text-warning"
                                                } else {
                                                    "btn btn-sm btn-ghost text-success"
                                                }
                                            }
                                            on:click=move |_| {
                                                on_toggle((name_toggle.clone(), is_enabled));
                                            }
                                        >
                                            {move || if is_enabled { "Disable" } else { "Enable" }}
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

/// Render job executions table
fn render_executions_table(executions: Vec<JobExecution>) -> impl IntoView {
    view! {
        <div class="overflow-x-auto">
            <table class="table table-zebra w-full">
                <thead>
                    <tr>
                        <th>"Execution ID"</th>
                        <th>"Job Name"</th>
                        <th>"Status"</th>
                        <th>"Trigger"</th>
                        <th>"Started"</th>
                        <th>"Duration"</th>
                    </tr>
                </thead>
                <tbody>
                    {executions
                        .into_iter()
                        .map(|exec| {
                            let status_class = execution_status_badge(&exec.status);
                            let started = exec
                                .started_at
                                .clone()
                                .unwrap_or_else(|| "-".to_string());
                            let duration = format_duration_ms(exec.duration_ms);
                            let error_display = exec.error.clone();
                            view! {
                                <tr>
                                    <td class="font-mono text-sm">
                                        {if exec.id.len() > 12 {
                                            exec.id[..12].to_string()
                                        } else {
                                            exec.id
                                        }}
                                    </td>
                                    <td class="font-medium">{exec.job_name}</td>
                                    <td>
                                        <span class=status_class>{exec.status}</span>
                                        {error_display
                                            .map(|err| {
                                                let title = err.clone();
                                                let display = if err.len() > 30 {
                                                    format!("{}...", &err[..30])
                                                } else {
                                                    err
                                                };
                                                view! {
                                                    <span
                                                        class="text-error text-xs ml-2"
                                                        title=title
                                                    >
                                                        {display}
                                                    </span>
                                                }
                                            })}
                                    </td>
                                    <td>
                                        <span class="badge badge-sm badge-outline">{exec.trigger}</span>
                                    </td>
                                    <td class="text-sm">{started}</td>
                                    <td class="text-sm">{duration}</td>
                                </tr>
                            }
                        })
                        .collect::<Vec<_>>()}
                </tbody>
            </table>
        </div>
    }
}

/// Jobs & Cron page component
#[component]
#[allow(clippy::too_many_lines)]
pub fn Jobs() -> impl IntoView {
    let (refresh, set_refresh) = signal(0u32);
    let cron_jobs = Resource::new(move || refresh.get(), |_| get_cron_jobs());
    // Load all recent executions (empty string = all jobs)
    let executions = Resource::new(
        move || refresh.get(),
        |_| list_job_executions(String::new(), Some(50)),
    );

    // Modal states
    let (show_trigger_modal, set_show_trigger_modal) = signal(false);
    let (error_msg, set_error_msg) = signal(Option::<String>::None);
    let (success_msg, set_success_msg) = signal(Option::<String>::None);

    // Form field for manual job trigger
    let (job_name_input, set_job_name_input) = signal(String::new());

    // Trigger a manual job
    let trigger_action = Action::new(move |name: &String| {
        let name = name.clone();
        async move {
            match trigger_job(name.clone()).await {
                Ok(exec_id) => {
                    set_show_trigger_modal.set(false);
                    set_job_name_input.set(String::new());
                    set_error_msg.set(None);
                    set_success_msg.set(Some(format!(
                        "Job '{name}' triggered (execution: {exec_id})"
                    )));
                    set_refresh.update(|n| *n += 1);
                }
                Err(e) => {
                    set_error_msg.set(Some(e.to_string()));
                }
            }
        }
    });

    // Trigger a cron job manually
    let trigger_cron_action = Action::new(move |name: &String| {
        let name = name.clone();
        async move {
            match trigger_cron_job(name.clone()).await {
                Ok(exec_id) => {
                    set_success_msg.set(Some(format!(
                        "Cron job '{name}' triggered (execution: {exec_id})"
                    )));
                    set_refresh.update(|n| *n += 1);
                }
                Err(e) => {
                    set_success_msg.set(None);
                    set_error_msg.set(Some(format!("Failed to trigger '{name}': {e}")));
                }
            }
        }
    });

    // Toggle cron job enabled/disabled
    let toggle_cron_action = Action::new(move |input: &(String, bool)| {
        let (name, currently_enabled) = input.clone();
        async move {
            let result = if currently_enabled {
                disable_cron_job(name.clone()).await
            } else {
                enable_cron_job(name.clone()).await
            };
            match result {
                Ok(()) => {
                    let action = if currently_enabled {
                        "disabled"
                    } else {
                        "enabled"
                    };
                    set_success_msg.set(Some(format!("Cron job '{name}' {action}")));
                    set_error_msg.set(None);
                    set_refresh.update(|n| *n += 1);
                }
                Err(e) => {
                    set_success_msg.set(None);
                    set_error_msg.set(Some(e.to_string()));
                }
            }
        }
    });

    view! {
        <div class="container mx-auto p-6">
            <div class="flex justify-between items-center mb-6">
                <h1 class="text-3xl font-bold">"Jobs & Cron"</h1>
                <button
                    class="btn btn-primary"
                    on:click=move |_| {
                        set_error_msg.set(None);
                        set_job_name_input.set(String::new());
                        set_show_trigger_modal.set(true);
                    }
                >
                    "Trigger Job"
                </button>
            </div>

            // Success / Error alerts
            {move || {
                success_msg
                    .get()
                    .map(|msg| {
                        view! {
                            <div class="alert alert-success mb-4">
                                <span>{msg}</span>
                                <button
                                    class="btn btn-ghost btn-sm"
                                    on:click=move |_| set_success_msg.set(None)
                                >
                                    "Dismiss"
                                </button>
                            </div>
                        }
                    })
            }}

            // ================================================================
            // Cron Jobs Section
            // ================================================================
            <div class="card bg-base-200 shadow-lg mb-8">
                <div class="card-body">
                    <h2 class="card-title">"Cron Jobs"</h2>
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
                                cron_jobs
                                    .get()
                                    .map(|result| {
                                        match result {
                                            Ok(jobs) => {
                                                if jobs.is_empty() {
                                                    view! {
                                                        <div class="alert alert-info">
                                                            <span>
                                                                "No cron jobs configured."
                                                            </span>
                                                        </div>
                                                    }
                                                        .into_any()
                                                } else {
                                                    let on_trigger = move |name: String| {
                                                        trigger_cron_action.dispatch(name);
                                                    };
                                                    let on_toggle = move |input: (String, bool)| {
                                                        toggle_cron_action.dispatch(input);
                                                    };
                                                    render_cron_table(jobs, on_trigger, on_toggle)
                                                        .into_any()
                                                }
                                            }
                                            Err(e) => {
                                                view! {
                                                    <div class="alert alert-error">
                                                        {e.to_string()}
                                                    </div>
                                                }
                                                    .into_any()
                                            }
                                        }
                                    })
                            }}
                        </ErrorBoundary>
                    </Suspense>
                </div>
            </div>

            // ================================================================
            // Job Executions Section
            // ================================================================
            <div class="card bg-base-200 shadow-lg">
                <div class="card-body">
                    <h2 class="card-title">"Recent Executions"</h2>
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
                                executions
                                    .get()
                                    .map(|result| {
                                        match result {
                                            Ok(exec_list) => {
                                                if exec_list.is_empty() {
                                                    view! {
                                                        <div class="alert alert-info">
                                                            <span>
                                                                "No job executions found."
                                                            </span>
                                                        </div>
                                                    }
                                                        .into_any()
                                                } else {
                                                    render_executions_table(exec_list).into_any()
                                                }
                                            }
                                            Err(e) => {
                                                view! {
                                                    <div class="alert alert-error">
                                                        {e.to_string()}
                                                    </div>
                                                }
                                                    .into_any()
                                            }
                                        }
                                    })
                            }}
                        </ErrorBoundary>
                    </Suspense>
                </div>
            </div>

            // ================================================================
            // Trigger Job Modal
            // ================================================================
            <div class=move || {
                if show_trigger_modal.get() { "modal modal-open" } else { "modal" }
            }>
                <div class="modal-box">
                    <h3 class="font-bold text-lg">"Trigger Job"</h3>

                    {move || {
                        error_msg
                            .get()
                            .map(|msg| {
                                view! {
                                    <div class="alert alert-error mb-4 mt-2">
                                        <span>{msg}</span>
                                    </div>
                                }
                            })
                    }}

                    <div class="form-control w-full mt-4">
                        <label class="label">
                            <span class="label-text">
                                "Job Name" <span class="text-error">"*"</span>
                            </span>
                        </label>
                        <input
                            type="text"
                            class="input input-bordered w-full"
                            placeholder="e.g. backup, sync, cleanup"
                            prop:value=move || job_name_input.get()
                            on:input=move |ev| set_job_name_input.set(event_target_value(&ev))
                        />
                        <label class="label">
                            <span class="label-text-alt text-base-content/50">
                                "Name of the job to trigger manually"
                            </span>
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
                            prop:disabled=move || job_name_input.get().trim().is_empty()
                            on:click=move |_| {
                                trigger_action.dispatch(job_name_input.get());
                            }
                        >
                            "Trigger"
                        </button>
                    </div>
                </div>
                <div
                    class="modal-backdrop"
                    on:click=move |_| set_show_trigger_modal.set(false)
                ></div>
            </div>
        </div>
    }
}
