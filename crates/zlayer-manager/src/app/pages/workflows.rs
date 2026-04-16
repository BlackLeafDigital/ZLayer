//! `/workflows` — named sequences of steps (master-detail layout).
//!
//! Left column: workflow list. Right column: the currently selected
//! workflow with its steps, a Run button, and recent runs. Runs expand
//! inline to show per-step progress using DaisyUI's `steps steps-vertical`
//! visualisation, with `step-success` / `step-error` / `step-neutral`
//! coloring driven by the daemon's `StepResult.status` values (`"ok" |
//! "failed" | "skipped"`).
//!
//! Action types: a workflow step is one of `RunTask`, `BuildProject`,
//! `DeployProject`, or `ApplySync`. The create modal renders a
//! discriminated-union form for each step (select variant + id), and on
//! submit serialises every step back into a `Vec<WireWorkflowStep>` for
//! the daemon's `POST /api/v1/workflows` endpoint.
//!
//! The daemon currently exposes create / delete / run / list-runs but NOT
//! an update/patch endpoint (see `router.rs::build_workflow_routes`), so
//! this page intentionally omits an "Edit" action — same pattern as
//! Tasks. To change a workflow, delete it and recreate it.

use leptos::ev::SubmitEvent;
use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::app::server_fns::{
    manager_create_workflow, manager_delete_workflow, manager_list_workflow_runs,
    manager_list_workflows, manager_run_workflow,
};
use crate::app::util::errors::format_server_error;
use crate::wire::workflows::{
    WireStepResult, WireWorkflow, WireWorkflowAction, WireWorkflowRun, WireWorkflowSpec,
    WireWorkflowStep,
};

/// Workflows management page with master-detail layout.
#[component]
#[allow(clippy::too_many_lines)] // view macro DSL + master-detail + modals
pub fn Workflows() -> impl IntoView {
    let workflows = Resource::new(|| (), |()| async move { manager_list_workflows().await });
    let (new_open, set_new_open) = signal(false);
    let (error, set_error) = signal(None::<String>);

    // Which workflow is currently selected. Stored as the id so it
    // survives list refetches without pointing at a stale clone.
    let selected_workflow: RwSignal<Option<String>> = RwSignal::new(None);

    let delete_target = RwSignal::new(None::<WireWorkflow>);

    // Shown after a successful synchronous run: the terminal run plus its
    // per-step results. Rendering modal vs. inline is driven by the
    // signal's `Some(_)` state.
    let latest_run = RwSignal::new(None::<WireWorkflowRun>);

    let refetch = move || workflows.refetch();

    view! {
        <div class="p-6 space-y-4">
            <div class="flex justify-between items-center">
                <h1 class="text-2xl font-bold">"Workflows"</h1>
                <button
                    class="btn btn-primary btn-sm"
                    on:click=move |_| set_new_open.set(true)
                >
                    "New workflow"
                </button>
            </div>

            {move || {
                error
                    .get()
                    .map(|msg| {
                        view! {
                            <div class="alert alert-error">
                                <span>{msg}</span>
                                <button
                                    class="btn btn-ghost btn-xs"
                                    on:click=move |_| set_error.set(None)
                                >
                                    "Dismiss"
                                </button>
                            </div>
                        }
                    })
            }}

            <div class="grid grid-cols-1 lg:grid-cols-3 gap-4">
                // Left column: workflow list
                <div class="lg:col-span-1 card bg-base-200">
                    <div class="card-body p-0 overflow-x-auto">
                        <Suspense fallback=move || {
                            view! {
                                <div class="p-6 flex justify-center">
                                    <span class="loading loading-spinner loading-md"></span>
                                </div>
                            }
                        }>
                            {move || {
                                workflows.get()
                                    .map(|res| match res {
                                        Err(e) => {
                                            view! {
                                                <div class="p-4 text-error">
                                                    {format!(
                                                        "Failed to load workflows: {}",
                                                        format_server_error(&e)
                                                    )}
                                                </div>
                                            }
                                                .into_any()
                                        }
                                        Ok(list) => render_workflow_table(
                                            &list,
                                            selected_workflow,
                                            delete_target,
                                        )
                                            .into_any(),
                                    })
                            }}
                        </Suspense>
                    </div>
                </div>

                // Right column: selected workflow detail
                <div class="lg:col-span-2">
                    <WorkflowDetail
                        selected=selected_workflow
                        workflows=workflows
                        set_error=set_error
                        latest_run=latest_run
                    />
                </div>
            </div>

            <NewWorkflowModal
                open=new_open
                set_open=set_new_open
                set_error=set_error
                on_success=move |id: String| {
                    selected_workflow.set(Some(id));
                    refetch();
                }
            />

            <DeleteWorkflowModal
                target=delete_target
                selected=selected_workflow
                set_error=set_error
                on_success=move || {
                    refetch();
                }
            />

            <RunResultModal
                target=latest_run
            />
        </div>
    }
}

/// Render the workflow list table. Clicking a row selects that workflow.
fn render_workflow_table(
    list: &[WireWorkflow],
    selected_workflow: RwSignal<Option<String>>,
    delete_target: RwSignal<Option<WireWorkflow>>,
) -> impl IntoView {
    if list.is_empty() {
        return view! { <div class="p-4 opacity-70">"(no workflows)"</div> }.into_any();
    }

    view! {
        <table class="table table-zebra table-sm">
            <thead>
                <tr>
                    <th>"Name"</th>
                    <th>"Steps"</th>
                    <th>"Updated"</th>
                    <th class="text-right">"Actions"</th>
                </tr>
            </thead>
            <tbody>
                {list
                    .iter()
                    .map(|w| render_workflow_row(w, selected_workflow, delete_target))
                    .collect_view()}
            </tbody>
        </table>
    }
    .into_any()
}

fn render_workflow_row(
    workflow: &WireWorkflow,
    selected_workflow: RwSignal<Option<String>>,
    delete_target: RwSignal<Option<WireWorkflow>>,
) -> impl IntoView {
    let select_id = workflow.id.clone();
    let row_id = workflow.id.clone();
    let delete_workflow = workflow.clone();
    let name = workflow.name.clone();
    let steps_count = workflow.steps.len();
    let updated_at = workflow.updated_at.clone();

    let row_class = move || {
        if selected_workflow.with(|s| s.as_deref().is_some_and(|id| id == row_id.as_str())) {
            "cursor-pointer bg-primary/10"
        } else {
            "cursor-pointer hover:bg-base-300"
        }
    };

    view! {
        <tr
            class=row_class
            on:click=move |_| selected_workflow.set(Some(select_id.clone()))
        >
            <td class="font-mono text-sm">{name}</td>
            <td>
                <span class="badge badge-outline">{steps_count.to_string()}</span>
            </td>
            <td class="text-xs opacity-70">{updated_at}</td>
            <td class="text-right">
                <button
                    class="btn btn-xs btn-error"
                    on:click=move |ev| {
                        ev.stop_propagation();
                        delete_target.set(Some(delete_workflow.clone()));
                    }
                >
                    "Delete"
                </button>
            </td>
        </tr>
    }
}

/// DaisyUI badge class for a workflow action type — semantic colors only.
fn action_badge_class(action: &WireWorkflowAction) -> &'static str {
    match action {
        WireWorkflowAction::RunTask { .. } => "badge badge-primary badge-sm",
        WireWorkflowAction::BuildProject { .. } => "badge badge-secondary badge-sm",
        WireWorkflowAction::DeployProject { .. } => "badge badge-accent badge-sm",
        WireWorkflowAction::ApplySync { .. } => "badge badge-info badge-sm",
    }
}

/// DaisyUI step-class for a [`WireStepResult.status`] string. The `step`
/// base class is added by the caller so this returns the modifier only.
fn step_status_class(status: &str) -> &'static str {
    match status {
        "ok" => "step step-success",
        "failed" => "step step-error",
        "skipped" => "step step-neutral",
        _ => "step",
    }
}

/// DaisyUI badge class for the overall run status.
fn run_status_badge_class(status: &str) -> &'static str {
    match status {
        "completed" => "badge badge-success",
        "failed" => "badge badge-error",
        "running" => "badge badge-primary",
        "pending" => "badge badge-ghost",
        _ => "badge badge-neutral",
    }
}

/// Right-hand detail panel for the currently selected workflow.
#[component]
#[allow(clippy::too_many_lines)]
fn WorkflowDetail(
    selected: RwSignal<Option<String>>,
    workflows: Resource<Result<Vec<WireWorkflow>, ServerFnError>>,
    set_error: WriteSignal<Option<String>>,
    latest_run: RwSignal<Option<WireWorkflowRun>>,
) -> impl IntoView {
    let runs: RwSignal<Vec<WireWorkflowRun>> = RwSignal::new(Vec::new());
    let (runs_loading, set_runs_loading) = signal(false);
    let (run_in_flight, set_run_in_flight) = signal(false);
    // Which run's step-results panel is expanded. Stored as the run id so
    // it survives refetches.
    let expanded_run = RwSignal::new(None::<String>);

    // Re-fetch runs every time the selection flips.
    Effect::new(move |_| {
        let Some(id) = selected.get() else {
            runs.set(Vec::new());
            expanded_run.set(None);
            return;
        };
        set_runs_loading.set(true);
        expanded_run.set(None);
        let id_cloned = id.clone();
        spawn_local(async move {
            match manager_list_workflow_runs(id_cloned).await {
                Ok(list) => runs.set(list),
                Err(e) => {
                    runs.set(Vec::new());
                    set_error.set(Some(format!(
                        "Load runs failed: {}",
                        format_server_error(&e)
                    )));
                }
            }
            set_runs_loading.set(false);
        });
    });

    // Run button handler.
    let on_run = move |_| {
        let Some(id) = selected.get() else {
            return;
        };
        if run_in_flight.get() {
            return;
        }
        set_run_in_flight.set(true);
        let id_cloned = id.clone();
        spawn_local(async move {
            let result = manager_run_workflow(id_cloned.clone()).await;
            set_run_in_flight.set(false);
            match result {
                Ok(run) => {
                    // Surface the finished run in the run-result modal and
                    // prepend it to the history list.
                    let run_for_history = run.clone();
                    latest_run.set(Some(run));
                    runs.update(|list| list.insert(0, run_for_history));
                }
                Err(e) => set_error.set(Some(format!("Run failed: {}", format_server_error(&e)))),
            }
        });
    };

    view! {
        {move || {
            let Some(workflow_id) = selected.get() else {
                return view! {
                    <div class="card bg-base-200">
                        <div class="card-body">
                            <p class="opacity-70">
                                "Select a workflow on the left to view details."
                            </p>
                        </div>
                    </div>
                }
                    .into_any();
            };

            let workflow = workflows
                .get()
                .and_then(Result::ok)
                .and_then(|list| list.into_iter().find(|w| w.id == workflow_id));

            let Some(workflow) = workflow else {
                return view! {
                    <div class="card bg-base-200">
                        <div class="card-body">
                            <p class="opacity-70">"Loading workflow…"</p>
                        </div>
                    </div>
                }
                    .into_any();
            };

            let workflow_name = workflow.name.clone();
            let scope_label = workflow
                .project_id
                .as_deref()
                .map_or_else(|| "(global)".to_string(), |p| format!("project {p}"));
            let workflow_steps = workflow.steps.clone();

            let running_badge = move || {
                if run_in_flight.get() {
                    view! {
                        <span class="badge badge-primary gap-1">
                            <span class="loading loading-spinner loading-xs"></span>
                            "Running…"
                        </span>
                    }
                        .into_any()
                } else {
                    ().into_any()
                }
            };

            view! {
                <div class="space-y-4">
                    // Header card
                    <div class="card bg-base-200">
                        <div class="card-body">
                            <div class="flex flex-wrap items-center justify-between gap-2">
                                <div>
                                    <h2 class="card-title font-mono">{workflow_name}</h2>
                                    <div class="flex gap-2 items-center mt-1 text-xs opacity-70">
                                        <span class="badge badge-outline badge-sm">
                                            {format!("{} steps", workflow_steps.len())}
                                        </span>
                                        <span>{scope_label}</span>
                                    </div>
                                </div>
                                <div class="flex gap-2 items-center">
                                    {running_badge}
                                    <button
                                        class="btn btn-primary btn-sm"
                                        disabled=move || run_in_flight.get()
                                        on:click=on_run
                                    >
                                        {move || if run_in_flight.get() {
                                            "Running…"
                                        } else {
                                            "Run"
                                        }}
                                    </button>
                                </div>
                            </div>

                            <div class="mt-4">
                                <div class="label-text mb-2 opacity-70">"Steps"</div>
                                {render_step_list(&workflow_steps)}
                            </div>
                        </div>
                    </div>

                    // Recent runs
                    <div class="card bg-base-200">
                        <div class="card-body">
                            <h3 class="card-title text-base">"Recent runs"</h3>
                            {move || {
                                if runs_loading.get() {
                                    view! {
                                        <div class="p-4 flex justify-center">
                                            <span class="loading loading-spinner loading-sm"></span>
                                        </div>
                                    }
                                        .into_any()
                                } else {
                                    render_runs_panel(&runs.get(), expanded_run).into_any()
                                }
                            }}
                        </div>
                    </div>
                </div>
            }
                .into_any()
        }}
    }
}

/// Render the workflow's step list as a vertical DaisyUI `steps` rail.
/// The rail uses a per-step `data-content` marker (the 1-indexed step
/// number) and shows name + action badge + referenced id below the step
/// label.
fn render_step_list(steps: &[WireWorkflowStep]) -> impl IntoView {
    if steps.is_empty() {
        return view! {
            <div class="opacity-70 text-sm">"(no steps)"</div>
        }
        .into_any();
    }

    let rows = steps
        .iter()
        .enumerate()
        .map(|(idx, step)| {
            let num = (idx + 1).to_string();
            let step_name = step.name.clone();
            let action_label = step.action.label();
            let action_target = step.action.target_id().to_string();
            let action_cls = action_badge_class(&step.action);
            let on_fail = step.on_failure.clone();
            view! {
                <li class="step step-primary" data-content=num>
                    <div class="text-left">
                        <div class="font-mono text-sm">{step_name}</div>
                        <div class="flex gap-2 items-center mt-1">
                            <span class=action_cls>{action_label}</span>
                            <span class="font-mono text-xs opacity-70">{action_target}</span>
                        </div>
                        {on_fail.map(|handler| view! {
                            <div class="text-xs opacity-60 mt-1">
                                {"on_failure: "} {handler}
                            </div>
                        })}
                    </div>
                </li>
            }
        })
        .collect_view();

    view! {
        <ul class="steps steps-vertical">{rows}</ul>
    }
    .into_any()
}

/// Render the "Recent runs" panel: a compact list of run cards with a
/// status badge, start timestamp, duration, and an expandable per-step
/// breakdown (click to toggle).
fn render_runs_panel(
    runs: &[WireWorkflowRun],
    expanded_run: RwSignal<Option<String>>,
) -> impl IntoView {
    if runs.is_empty() {
        return view! {
            <div class="p-2 opacity-70 text-sm">"(no runs yet)"</div>
        }
        .into_any();
    }

    let rows = runs
        .iter()
        .map(|run| render_run_row(run, expanded_run))
        .collect_view();

    view! {
        <div class="space-y-2">{rows}</div>
    }
    .into_any()
}

fn render_run_row(run: &WireWorkflowRun, expanded_run: RwSignal<Option<String>>) -> impl IntoView {
    let short_id: String = run.id.chars().take(8).collect();
    let full_id = run.id.clone();
    let full_id_for_click = run.id.clone();
    let badge_cls = run_status_badge_class(&run.status);
    let status_label = run.status.clone();
    let started_at = run.started_at.clone();
    let duration = compute_duration(&run.started_at, run.finished_at.as_deref());
    let results = run.step_results.clone();
    let expanded_id_a = full_id.clone();
    let expanded_id_b = full_id.clone();

    // Two closures because neither the reactive expression in the
    // `{move || …}` block nor the arrow glyph selector below can share a
    // single `FnOnce`-ish closure — `move` captures the `String` by value.
    let is_expanded = move || expanded_run.with(|e| e.as_deref() == Some(expanded_id_a.as_str()));
    let expanded_for_row =
        move || expanded_run.with(|e| e.as_deref() == Some(expanded_id_b.as_str()));

    let on_toggle = move |_| {
        let id = full_id_for_click.clone();
        expanded_run.update(|cur| {
            if cur.as_deref() == Some(id.as_str()) {
                *cur = None;
            } else {
                *cur = Some(id);
            }
        });
    };

    view! {
        <div class="bg-base-100 rounded-lg">
            <button
                type="button"
                class="w-full flex flex-wrap items-center justify-between gap-2 p-3 hover:bg-base-300 rounded-lg text-left"
                on:click=on_toggle
            >
                <div class="flex items-center gap-3">
                    <span class=badge_cls>{status_label}</span>
                    <span class="font-mono text-xs opacity-80" title=full_id>{short_id}</span>
                </div>
                <div class="flex items-center gap-3 text-xs opacity-70">
                    <span>{started_at}</span>
                    <span>{duration}</span>
                    <span>
                        {move || if expanded_for_row() { "▼" } else { "▶" }}
                    </span>
                </div>
            </button>
            {move || if is_expanded() {
                view! {
                    <div class="p-3 pt-0">
                        {render_step_results(&results)}
                    </div>
                }
                    .into_any()
            } else {
                ().into_any()
            }}
        </div>
    }
}

/// Render the per-step breakdown of a run as a DaisyUI `steps steps-vertical`
/// rail. Each step is colored by its result status and exposes its output
/// (or error) in a `mockup-code` block beneath the step label.
fn render_step_results(results: &[WireStepResult]) -> impl IntoView {
    if results.is_empty() {
        return view! {
            <div class="opacity-70 text-sm">"(no step results)"</div>
        }
        .into_any();
    }

    let rows = results
        .iter()
        .enumerate()
        .map(|(idx, r)| {
            let cls = step_status_class(&r.status);
            let num = (idx + 1).to_string();
            let step_name = r.step_name.clone();
            let status_copy = r.status.clone();
            let output = r.output.clone();
            view! {
                <li class=cls data-content=num>
                    <div class="text-left w-full">
                        <div class="font-mono text-sm">{step_name}</div>
                        <div class="text-xs opacity-70 mt-1">{status_copy}</div>
                        {output.filter(|s| !s.is_empty()).map(|text| view! {
                            <pre class="mockup-code text-xs overflow-x-auto mt-2">
                                <code>{text}</code>
                            </pre>
                        })}
                    </div>
                </li>
            }
        })
        .collect_view();

    view! {
        <ul class="steps steps-vertical">{rows}</ul>
    }
    .into_any()
}

/// Compute the duration between two RFC-3339 timestamps — same minimal
/// parser the tasks page uses (no chrono in hydrate/WASM).
fn compute_duration(started: &str, finished: Option<&str>) -> String {
    let Some(finished) = finished else {
        return "(running)".to_string();
    };
    let (Some(start_ms), Some(end_ms)) =
        (rfc3339_to_unix_ms(started), rfc3339_to_unix_ms(finished))
    else {
        return "?".to_string();
    };
    let millis = end_ms - start_ms;
    if millis < 0 {
        return "?".to_string();
    }
    #[allow(clippy::cast_precision_loss)]
    let secs = millis as f64 / 1000.0;
    if secs < 10.0 {
        format!("{secs:.3}s")
    } else if secs < 600.0 {
        format!("{secs:.1}s")
    } else {
        let total_secs = millis / 1000;
        let minutes = total_secs / 60;
        let leftover = total_secs - minutes * 60;
        format!("{minutes}m{leftover}s")
    }
}

/// Parse a subset of RFC-3339 into milliseconds since the unix epoch.
/// Mirrors the implementation in `tasks.rs`; kept local so the pages
/// don't take a cross-module dependency on a private helper.
fn rfc3339_to_unix_ms(s: &str) -> Option<i64> {
    let (date, rest) = s.split_once('T')?;
    let mut y_m_d = date.split('-');
    let year: i64 = y_m_d.next()?.parse().ok()?;
    let month: u32 = y_m_d.next()?.parse().ok()?;
    let day: u32 = y_m_d.next()?.parse().ok()?;

    let (time_part, offset_mins): (&str, i64) = if let Some(stripped) = rest.strip_suffix('Z') {
        (stripped, 0)
    } else if let Some(idx) = rest.rfind(['+', '-']) {
        let (t, tz) = rest.split_at(idx);
        let sign = if tz.starts_with('+') { 1 } else { -1 };
        let tz_body = &tz[1..];
        let mut hm = tz_body.split(':');
        let hh: i64 = hm.next()?.parse().ok()?;
        let mm: i64 = hm.next().unwrap_or("0").parse().ok()?;
        (t, sign * (hh * 60 + mm))
    } else {
        (rest, 0)
    };

    let mut hms_frac = time_part.split('.');
    let hms = hms_frac.next()?;
    let frac = hms_frac.next().unwrap_or("");

    let mut hms_parts = hms.split(':');
    let hour: u32 = hms_parts.next()?.parse().ok()?;
    let minute: u32 = hms_parts.next()?.parse().ok()?;
    let second: u32 = hms_parts.next()?.parse().ok()?;

    let days = ymd_to_unix_days(year, month, day)?;

    let secs_in_day = i64::from(hour) * 3600 + i64::from(minute) * 60 + i64::from(second);
    let utc_secs = days * 86_400 + secs_in_day - offset_mins * 60;

    let frac_ms: i64 = if frac.is_empty() {
        0
    } else {
        let mut buf = [b'0'; 3];
        for (i, c) in frac.bytes().take(3).enumerate() {
            buf[i] = c;
        }
        std::str::from_utf8(&buf).ok()?.parse().ok()?
    };

    Some(utc_secs * 1000 + frac_ms)
}

fn ymd_to_unix_days(year: i64, month: u32, day: u32) -> Option<i64> {
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let m = i64::from(month);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe - 719_468)
}

// ---------------------------------------------------------------------------
// New workflow modal — dynamic step list builder
// ---------------------------------------------------------------------------

/// A per-step draft carrying per-field signals. The form maintains a
/// `RwSignal<Vec<StepDraft>>`; `build_steps` converts that into a
/// `Vec<WireWorkflowStep>` at submit time.
///
/// Each draft carries ALL variant-specific id signals so the user can
/// switch step-type without losing values on the other variants. `kind`
/// is one of `"run_task" | "build_project" | "deploy_project" |
/// "apply_sync"` — same snake-case discriminator the daemon's `serde(tag
/// = "type")` emits.
#[derive(Clone, Copy)]
struct StepDraft {
    /// Stable internal id used as the React-style list key so reorders
    /// don't force Leptos to re-mount inputs (which would lose focus).
    key: u64,
    name: RwSignal<String>,
    kind: RwSignal<String>,
    task_id: RwSignal<String>,
    project_id: RwSignal<String>,
    sync_id: RwSignal<String>,
    on_failure: RwSignal<String>,
}

impl StepDraft {
    fn new(key: u64) -> Self {
        Self {
            key,
            name: RwSignal::new(String::new()),
            kind: RwSignal::new("run_task".to_string()),
            task_id: RwSignal::new(String::new()),
            project_id: RwSignal::new(String::new()),
            sync_id: RwSignal::new(String::new()),
            on_failure: RwSignal::new(String::new()),
        }
    }

    /// Build a `WireWorkflowStep` from the current signal values, or
    /// `Err` with a user-facing validation message.
    fn build(&self, index: usize) -> Result<WireWorkflowStep, String> {
        let name = self.name.get().trim().to_string();
        if name.is_empty() {
            return Err(format!("Step {} is missing a name.", index + 1));
        }
        let kind = self.kind.get();
        let action = match kind.as_str() {
            "run_task" => {
                let id = self.task_id.get().trim().to_string();
                if id.is_empty() {
                    return Err(format!("Step {}: task id is required.", index + 1));
                }
                WireWorkflowAction::RunTask { task_id: id }
            }
            "build_project" => {
                let id = self.project_id.get().trim().to_string();
                if id.is_empty() {
                    return Err(format!("Step {}: project id is required.", index + 1));
                }
                WireWorkflowAction::BuildProject { project_id: id }
            }
            "deploy_project" => {
                let id = self.project_id.get().trim().to_string();
                if id.is_empty() {
                    return Err(format!("Step {}: project id is required.", index + 1));
                }
                WireWorkflowAction::DeployProject { project_id: id }
            }
            "apply_sync" => {
                let id = self.sync_id.get().trim().to_string();
                if id.is_empty() {
                    return Err(format!("Step {}: sync id is required.", index + 1));
                }
                WireWorkflowAction::ApplySync { sync_id: id }
            }
            other => {
                return Err(format!(
                    "Step {}: unknown action type '{other}'.",
                    index + 1
                ));
            }
        };
        let on_failure = {
            let v = self.on_failure.get().trim().to_string();
            if v.is_empty() {
                None
            } else {
                Some(v)
            }
        };
        Ok(WireWorkflowStep {
            name,
            action,
            on_failure,
        })
    }
}

#[component]
#[allow(clippy::too_many_lines)]
fn NewWorkflowModal(
    open: ReadSignal<bool>,
    set_open: WriteSignal<bool>,
    set_error: WriteSignal<Option<String>>,
    on_success: impl Fn(String) + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let (name, set_name) = signal(String::new());
    let (project_id, set_project_id) = signal(String::new());
    let (submitting, set_submitting) = signal(false);
    let (form_error, set_form_error) = signal(None::<String>);

    // Stable key counter for StepDraft entries — lets the view track
    // which draft a given input belongs to across reorders.
    let next_key = RwSignal::new(1_u64);
    let steps: RwSignal<Vec<StepDraft>> = RwSignal::new(Vec::new());

    let allocate_key = move || {
        let k = next_key.get();
        next_key.set(k + 1);
        k
    };

    let reset_form = move || {
        set_name.set(String::new());
        set_project_id.set(String::new());
        set_form_error.set(None);
        steps.set(vec![StepDraft::new(allocate_key())]);
        next_key.set(2);
    };

    // Seed with a single empty step whenever the modal opens.
    Effect::new(move |_| {
        if open.get() && steps.with(Vec::is_empty) {
            steps.set(vec![StepDraft::new(allocate_key())]);
        }
    });

    let add_step = move |_| {
        let key = allocate_key();
        steps.update(|list| list.push(StepDraft::new(key)));
    };

    let close = move || {
        set_open.set(false);
        set_form_error.set(None);
    };

    let on_submit = move |ev: SubmitEvent| {
        ev.prevent_default();
        if submitting.get() {
            return;
        }
        let name_val = name.get().trim().to_string();
        if name_val.is_empty() {
            set_form_error.set(Some("Name is required.".into()));
            return;
        }
        let drafts = steps.get();
        if drafts.is_empty() {
            set_form_error.set(Some("At least one step is required.".into()));
            return;
        }
        let mut built = Vec::with_capacity(drafts.len());
        for (idx, draft) in drafts.iter().enumerate() {
            match draft.build(idx) {
                Ok(step) => built.push(step),
                Err(msg) => {
                    set_form_error.set(Some(msg));
                    return;
                }
            }
        }

        let project_val = project_id.get().trim().to_string();
        let project_opt = if project_val.is_empty() {
            None
        } else {
            Some(project_val)
        };

        let spec = WireWorkflowSpec {
            name: name_val,
            steps: built,
            project_id: project_opt,
        };

        set_submitting.set(true);
        spawn_local(async move {
            let result = manager_create_workflow(spec).await;
            set_submitting.set(false);
            match result {
                Ok(wf) => {
                    reset_form();
                    set_open.set(false);
                    on_success(wf.id);
                }
                Err(e) => {
                    set_error.set(Some(format!("Create failed: {}", format_server_error(&e))));
                }
            }
        });
    };

    view! {
        <dialog class=move || if open.get() { "modal modal-open" } else { "modal" }>
            <div class="modal-box max-w-3xl">
                <h3 class="font-bold text-lg mb-3">"New workflow"</h3>

                {move || form_error.get().map(|msg| view! {
                    <div class="alert alert-error text-sm py-2 mb-2"><span>{msg}</span></div>
                })}

                <form on:submit=on_submit class="space-y-3">
                    <div class="form-control">
                        <label class="label">
                            <span class="label-text">"Name"</span>
                        </label>
                        <input
                            type="text"
                            class="input input-bordered w-full font-mono"
                            required
                            placeholder="deploy-pipeline"
                            prop:value=name
                            on:input=move |ev| set_name.set(event_target_value(&ev))
                        />
                    </div>

                    <div class="form-control">
                        <label class="label">
                            <span class="label-text">
                                "Project id "<span class="opacity-60">"(optional)"</span>
                            </span>
                            <span class="label-text-alt opacity-70">"Blank = global workflow"</span>
                        </label>
                        <input
                            type="text"
                            class="input input-bordered w-full"
                            placeholder="(global)"
                            prop:value=project_id
                            on:input=move |ev| set_project_id.set(event_target_value(&ev))
                        />
                    </div>

                    <div class="divider">"Steps"</div>

                    {render_step_editors(steps)}

                    <div class="flex justify-start">
                        <button
                            type="button"
                            class="btn btn-sm btn-ghost"
                            on:click=add_step
                        >
                            "+ Add step"
                        </button>
                    </div>

                    <div class="modal-action">
                        <button
                            type="button"
                            class="btn btn-ghost"
                            on:click=move |_| close()
                        >
                            "Cancel"
                        </button>
                        <button
                            type="submit"
                            class="btn btn-primary"
                            disabled=move || submitting.get()
                        >
                            {move || if submitting.get() { "Creating…" } else { "Create" }}
                        </button>
                    </div>
                </form>
            </div>
            <form
                method="dialog"
                class="modal-backdrop"
                on:submit=move |_| close()
            >
                <button>"close"</button>
            </form>
        </dialog>
    }
}

/// Render the ordered list of step editors. Each editor exposes the
/// name + kind selector + variant-specific id input, plus optional
/// `on_failure` task id. A row also carries move-up / move-down /
/// remove buttons; the kind selector uses a plain DaisyUI `<select>` so
/// keyboard navigation stays native.
fn render_step_editors(steps: RwSignal<Vec<StepDraft>>) -> impl IntoView {
    view! {
        <div class="space-y-3">
            {move || {
                let list = steps.get();
                let len = list.len();
                list.into_iter()
                    .enumerate()
                    .map(|(idx, draft)| render_step_editor(steps, idx, len, draft))
                    .collect_view()
            }}
        </div>
    }
}

#[allow(clippy::too_many_lines)]
fn render_step_editor(
    steps: RwSignal<Vec<StepDraft>>,
    idx: usize,
    total: usize,
    draft: StepDraft,
) -> impl IntoView {
    let draft_key = draft.key;
    let name_sig = draft.name;
    let kind_sig = draft.kind;
    let task_id_sig = draft.task_id;
    let project_id_sig = draft.project_id;
    let sync_id_sig = draft.sync_id;
    let on_failure_sig = draft.on_failure;

    let can_move_up = idx > 0;
    let can_move_down = idx + 1 < total;

    let move_up = move |_| {
        if idx == 0 {
            return;
        }
        steps.update(|list| {
            list.swap(idx - 1, idx);
        });
    };
    let move_down = move |_| {
        steps.update(|list| {
            if idx + 1 < list.len() {
                list.swap(idx, idx + 1);
            }
        });
    };
    let remove = move |_| {
        steps.update(|list| {
            list.retain(|d| d.key != draft_key);
        });
    };

    view! {
        <div class="card bg-base-300 shadow-sm">
            <div class="card-body p-3 space-y-2">
                <div class="flex items-center justify-between gap-2">
                    <span class="badge badge-neutral">{format!("Step {}", idx + 1)}</span>
                    <div class="flex gap-1">
                        <button
                            type="button"
                            class="btn btn-xs btn-ghost"
                            disabled=!can_move_up
                            on:click=move_up
                            title="Move up"
                        >
                            "↑"
                        </button>
                        <button
                            type="button"
                            class="btn btn-xs btn-ghost"
                            disabled=!can_move_down
                            on:click=move_down
                            title="Move down"
                        >
                            "↓"
                        </button>
                        <button
                            type="button"
                            class="btn btn-xs btn-error"
                            on:click=remove
                            title="Remove step"
                        >
                            "Remove"
                        </button>
                    </div>
                </div>

                <div class="grid grid-cols-1 md:grid-cols-2 gap-2">
                    <div class="form-control">
                        <label class="label py-1">
                            <span class="label-text text-xs">"Step name"</span>
                        </label>
                        <input
                            type="text"
                            class="input input-bordered input-sm w-full font-mono"
                            required
                            placeholder="build"
                            prop:value=move || name_sig.get()
                            on:input=move |ev| name_sig.set(event_target_value(&ev))
                        />
                    </div>

                    <div class="form-control">
                        <label class="label py-1">
                            <span class="label-text text-xs">"Action"</span>
                        </label>
                        <select
                            class="select select-bordered select-sm w-full"
                            prop:value=move || kind_sig.get()
                            on:change=move |ev| kind_sig.set(event_target_value(&ev))
                        >
                            <option value="run_task">"Run task"</option>
                            <option value="build_project">"Build project"</option>
                            <option value="deploy_project">"Deploy project"</option>
                            <option value="apply_sync">"Apply sync"</option>
                        </select>
                    </div>
                </div>

                // Variant-specific id field.
                {move || {
                    let kind = kind_sig.get();
                    match kind.as_str() {
                        "run_task" => view! {
                            <div class="form-control">
                                <label class="label py-1">
                                    <span class="label-text text-xs">"Task id"</span>
                                </label>
                                <input
                                    type="text"
                                    class="input input-bordered input-sm w-full font-mono"
                                    required
                                    placeholder="task uuid"
                                    prop:value=move || task_id_sig.get()
                                    on:input=move |ev| task_id_sig.set(event_target_value(&ev))
                                />
                            </div>
                        }
                        .into_any(),
                        "build_project" | "deploy_project" => view! {
                            <div class="form-control">
                                <label class="label py-1">
                                    <span class="label-text text-xs">"Project id"</span>
                                </label>
                                <input
                                    type="text"
                                    class="input input-bordered input-sm w-full font-mono"
                                    required
                                    placeholder="project uuid"
                                    prop:value=move || project_id_sig.get()
                                    on:input=move |ev| project_id_sig.set(event_target_value(&ev))
                                />
                            </div>
                        }
                        .into_any(),
                        "apply_sync" => view! {
                            <div class="form-control">
                                <label class="label py-1">
                                    <span class="label-text text-xs">"Sync id"</span>
                                </label>
                                <input
                                    type="text"
                                    class="input input-bordered input-sm w-full font-mono"
                                    required
                                    placeholder="sync uuid"
                                    prop:value=move || sync_id_sig.get()
                                    on:input=move |ev| sync_id_sig.set(event_target_value(&ev))
                                />
                            </div>
                        }
                        .into_any(),
                        _ => view! { <div></div> }.into_any(),
                    }
                }}

                <div class="form-control">
                    <label class="label py-1">
                        <span class="label-text text-xs">
                            "on_failure task id "
                            <span class="opacity-60">"(optional)"</span>
                        </span>
                        <span class="label-text-alt opacity-70 text-xs">
                            "Runs when this step fails"
                        </span>
                    </label>
                    <input
                        type="text"
                        class="input input-bordered input-sm w-full font-mono"
                        placeholder="task uuid"
                        prop:value=move || on_failure_sig.get()
                        on:input=move |ev| on_failure_sig.set(event_target_value(&ev))
                    />
                </div>
            </div>
        </div>
    }
}

// ---------------------------------------------------------------------------
// Delete modal
// ---------------------------------------------------------------------------

#[component]
fn DeleteWorkflowModal(
    target: RwSignal<Option<WireWorkflow>>,
    selected: RwSignal<Option<String>>,
    set_error: WriteSignal<Option<String>>,
    on_success: impl Fn() + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let (submitting, set_submitting) = signal(false);

    let close = move || target.set(None);

    let confirm = move |_| {
        let Some(workflow) = target.get() else {
            return;
        };
        set_submitting.set(true);
        spawn_local(async move {
            let result = manager_delete_workflow(workflow.id.clone()).await;
            set_submitting.set(false);
            match result {
                Ok(()) => {
                    if selected.with(|s| s.as_deref() == Some(workflow.id.as_str())) {
                        selected.set(None);
                    }
                    close();
                    on_success();
                }
                Err(e) => {
                    set_error.set(Some(format!("Delete failed: {}", format_server_error(&e))));
                }
            }
        });
    };

    view! {
        <dialog class=move || {
            if target.with(Option::is_some) {
                "modal modal-open"
            } else {
                "modal"
            }
        }>
            <div class="modal-box">
                <h3 class="font-bold text-lg mb-3">"Delete workflow"</h3>
                <p>
                    {move || {
                        target
                            .get()
                            .map(|w| format!(
                                "Delete workflow '{}'? This cannot be undone — the run history will also be removed.",
                                w.name
                            ))
                            .unwrap_or_default()
                    }}
                </p>
                <div class="modal-action">
                    <button
                        type="button"
                        class="btn btn-ghost"
                        on:click=move |_| close()
                    >
                        "Cancel"
                    </button>
                    <button
                        type="button"
                        class="btn btn-error"
                        on:click=confirm
                        disabled=move || submitting.get()
                    >
                        {move || if submitting.get() { "Deleting…" } else { "Delete" }}
                    </button>
                </div>
            </div>
        </dialog>
    }
}

// ---------------------------------------------------------------------------
// Run-result modal — shows the finished run's per-step breakdown
// ---------------------------------------------------------------------------

#[component]
fn RunResultModal(target: RwSignal<Option<WireWorkflowRun>>) -> impl IntoView {
    let close = move || target.set(None);

    view! {
        <dialog class=move || {
            if target.with(Option::is_some) {
                "modal modal-open"
            } else {
                "modal"
            }
        }>
            <div class="modal-box max-w-3xl">
                {move || {
                    let Some(run) = target.get() else {
                        return ().into_any();
                    };
                    let badge_cls = run_status_badge_class(&run.status);
                    let status_label = run.status.clone();
                    let run_id = run.id.clone();
                    let short_id: String = run_id.chars().take(12).collect();
                    let started_at = run.started_at.clone();
                    let duration = compute_duration(
                        &run.started_at,
                        run.finished_at.as_deref(),
                    );
                    let results = run.step_results.clone();
                    view! {
                        <div class="space-y-3">
                            <div class="flex items-center justify-between gap-2 flex-wrap">
                                <h3 class="font-bold text-lg">"Workflow run"</h3>
                                <span class=badge_cls>{status_label}</span>
                            </div>
                            <div class="flex flex-wrap gap-3 text-xs opacity-70">
                                <span class="font-mono" title=run_id>{short_id}</span>
                                <span>{"Started: "} {started_at}</span>
                                <span>{"Duration: "} {duration}</span>
                            </div>
                            <div class="divider my-1"></div>
                            {render_step_results(&results)}
                        </div>
                    }
                        .into_any()
                }}
                <div class="modal-action">
                    <button
                        type="button"
                        class="btn btn-ghost"
                        on:click=move |_| close()
                    >
                        "Close"
                    </button>
                </div>
            </div>
            <form
                method="dialog"
                class="modal-backdrop"
                on:submit=move |_| close()
            >
                <button>"close"</button>
            </form>
        </dialog>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_badge_returns_semantic_classes_only() {
        let run = WireWorkflowAction::RunTask {
            task_id: "t".into(),
        };
        assert_eq!(action_badge_class(&run), "badge badge-primary badge-sm");
        let build = WireWorkflowAction::BuildProject {
            project_id: "p".into(),
        };
        assert_eq!(action_badge_class(&build), "badge badge-secondary badge-sm");
    }

    #[test]
    fn step_status_class_maps_known_statuses() {
        assert_eq!(step_status_class("ok"), "step step-success");
        assert_eq!(step_status_class("failed"), "step step-error");
        assert_eq!(step_status_class("skipped"), "step step-neutral");
        assert_eq!(step_status_class("weird"), "step");
    }

    #[test]
    fn run_status_badge_maps_known_statuses() {
        assert_eq!(run_status_badge_class("completed"), "badge badge-success");
        assert_eq!(run_status_badge_class("failed"), "badge badge-error");
        assert_eq!(run_status_badge_class("running"), "badge badge-primary");
        assert_eq!(run_status_badge_class("pending"), "badge badge-ghost");
        assert_eq!(run_status_badge_class("other"), "badge badge-neutral");
    }

    #[test]
    fn rfc3339_parser_matches_tasks_page() {
        let a = rfc3339_to_unix_ms("2026-04-16T00:00:00Z").unwrap();
        let b = rfc3339_to_unix_ms("2026-04-16T00:00:01Z").unwrap();
        assert_eq!(b - a, 1000);
    }

    #[test]
    fn compute_duration_running_is_labelled() {
        assert_eq!(
            compute_duration("2026-04-16T00:00:00Z", None),
            "(running)".to_string()
        );
    }

    #[test]
    fn compute_duration_under_ten_seconds_shows_millis() {
        assert_eq!(
            compute_duration("2026-04-16T00:00:00Z", Some("2026-04-16T00:00:01.234Z")),
            "1.234s".to_string()
        );
    }

    #[test]
    fn compute_duration_negative_returns_placeholder() {
        assert_eq!(
            compute_duration("2026-04-16T00:00:10Z", Some("2026-04-16T00:00:00Z")),
            "?".to_string()
        );
    }

    // `StepDraft` holds Leptos `RwSignal`s, which require a reactive
    // `Owner` to allocate. Every `StepDraft` test therefore wraps its
    // body in `Owner::new().with(|| …)` so the sandboxed-arena runtime
    // has an active scope.
    fn with_owner<T>(fun: impl FnOnce() -> T) -> T {
        leptos::reactive::owner::Owner::new().with(fun)
    }

    #[test]
    fn step_draft_builds_run_task() {
        with_owner(|| {
            let d = StepDraft::new(1);
            d.name.set("build".into());
            d.kind.set("run_task".into());
            d.task_id.set("task-uuid".into());
            let step = d.build(0).expect("valid draft");
            assert_eq!(step.name, "build");
            match step.action {
                WireWorkflowAction::RunTask { task_id } => assert_eq!(task_id, "task-uuid"),
                other => panic!("expected RunTask, got {other:?}"),
            }
            assert!(step.on_failure.is_none());
        });
    }

    #[test]
    fn step_draft_builds_build_project_with_on_failure() {
        with_owner(|| {
            let d = StepDraft::new(2);
            d.name.set("build".into());
            d.kind.set("build_project".into());
            d.project_id.set("proj-1".into());
            d.on_failure.set("cleanup-task".into());
            let step = d.build(0).expect("valid draft");
            assert_eq!(step.on_failure.as_deref(), Some("cleanup-task"));
            match step.action {
                WireWorkflowAction::BuildProject { project_id } => {
                    assert_eq!(project_id, "proj-1");
                }
                other => panic!("expected BuildProject, got {other:?}"),
            }
        });
    }

    #[test]
    fn step_draft_rejects_missing_name() {
        with_owner(|| {
            let d = StepDraft::new(3);
            d.kind.set("run_task".into());
            d.task_id.set("t1".into());
            let err = d.build(0).expect_err("missing name must reject");
            assert!(err.contains("Step 1"));
            assert!(err.contains("name"));
        });
    }

    #[test]
    fn step_draft_rejects_missing_target_id_per_variant() {
        with_owner(|| {
            for kind in &["run_task", "build_project", "deploy_project", "apply_sync"] {
                let d = StepDraft::new(4);
                d.name.set("step".into());
                d.kind.set((*kind).to_string());
                let err = d.build(0).expect_err("missing id must reject");
                assert!(err.contains("Step 1"), "kind={kind}: {err}");
            }
        });
    }

    #[test]
    fn step_draft_unknown_kind_is_rejected() {
        with_owner(|| {
            let d = StepDraft::new(5);
            d.name.set("x".into());
            d.kind.set("bogus".into());
            let err = d.build(0).expect_err("unknown kind must reject");
            assert!(err.contains("bogus"));
        });
    }

    #[test]
    fn step_draft_apply_sync_builds_correctly() {
        with_owner(|| {
            let d = StepDraft::new(6);
            d.name.set("apply".into());
            d.kind.set("apply_sync".into());
            d.sync_id.set("s1".into());
            let step = d.build(0).expect("valid draft");
            match step.action {
                WireWorkflowAction::ApplySync { sync_id } => assert_eq!(sync_id, "s1"),
                other => panic!("expected ApplySync, got {other:?}"),
            }
        });
    }
}
