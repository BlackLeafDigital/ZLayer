//! `/tasks` — named runnable scripts (master-detail layout).
//!
//! Left column: task list. Right column: the currently selected task with
//! a Run button, run history, and the output of the latest run.
//!
//! All authenticated users can browse; only admins can mutate or run (the
//! backend enforces on every request — we surface the resulting error as
//! an alert banner).
//!
//! The daemon currently exposes create / delete / run / list-runs but
//! NOT an update/patch endpoint, so this page intentionally omits an
//! "Edit" action. To change a task, delete it and recreate it under the
//! same name.

use leptos::ev::SubmitEvent;
use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::app::server_fns::{
    manager_create_task, manager_delete_task, manager_list_task_runs, manager_list_tasks,
    manager_run_task,
};
use crate::wire::tasks::{WireTask, WireTaskRun, WireTaskSpec};

/// Tasks management page with master-detail layout.
#[component]
#[allow(clippy::too_many_lines)] // view macro DSL + master-detail + modals
pub fn Tasks() -> impl IntoView {
    let tasks = Resource::new(|| (), |()| async move { manager_list_tasks().await });
    let (new_open, set_new_open) = signal(false);
    let (error, set_error) = signal(None::<String>);

    // Which task is currently selected. Stored as the task id so it
    // survives list refetches without pointing at a stale clone.
    let selected_task: RwSignal<Option<String>> = RwSignal::new(None);

    let delete_target = RwSignal::new(None::<WireTask>);

    let refetch = move || tasks.refetch();

    view! {
        <div class="p-6 space-y-4">
            <div class="flex justify-between items-center">
                <h1 class="text-2xl font-bold">"Tasks"</h1>
                <button
                    class="btn btn-primary btn-sm"
                    on:click=move |_| set_new_open.set(true)
                >
                    "New task"
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
                // Left column: task list
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
                                tasks.get()
                                    .map(|res| match res {
                                        Err(e) => {
                                            view! {
                                                <div class="p-4 text-error">
                                                    {format!("Failed to load tasks: {e}")}
                                                </div>
                                            }
                                                .into_any()
                                        }
                                        Ok(list) => render_task_table(
                                            &list,
                                            selected_task,
                                            delete_target,
                                        )
                                            .into_any(),
                                    })
                            }}
                        </Suspense>
                    </div>
                </div>

                // Right column: selected task detail
                <div class="lg:col-span-2">
                    <TaskDetail
                        selected=selected_task
                        tasks=tasks
                        set_error=set_error
                    />
                </div>
            </div>

            <NewTaskModal
                open=new_open
                set_open=set_new_open
                set_error=set_error
                on_success=move |id: String| {
                    selected_task.set(Some(id));
                    refetch();
                }
            />

            <DeleteTaskModal
                target=delete_target
                selected=selected_task
                set_error=set_error
                on_success=move || {
                    refetch();
                }
            />
        </div>
    }
}

/// Render the task list table. Clicking a row selects that task.
fn render_task_table(
    list: &[WireTask],
    selected_task: RwSignal<Option<String>>,
    delete_target: RwSignal<Option<WireTask>>,
) -> impl IntoView {
    if list.is_empty() {
        return view! { <div class="p-4 opacity-70">"(no tasks)"</div> }.into_any();
    }

    view! {
        <table class="table table-zebra table-sm">
            <thead>
                <tr>
                    <th>"Name"</th>
                    <th>"Kind"</th>
                    <th>"Updated"</th>
                    <th class="text-right">"Actions"</th>
                </tr>
            </thead>
            <tbody>
                {list
                    .iter()
                    .map(|t| render_task_row(t, selected_task, delete_target))
                    .collect_view()}
            </tbody>
        </table>
    }
    .into_any()
}

fn render_task_row(
    task: &WireTask,
    selected_task: RwSignal<Option<String>>,
    delete_target: RwSignal<Option<WireTask>>,
) -> impl IntoView {
    // Captured by individual on_click closures, so clone per handler.
    let select_id = task.id.clone();
    let row_id = task.id.clone();
    let delete_task = task.clone();
    let name = task.name.clone();
    let kind = task.kind.clone();
    let updated_at = task.updated_at.clone();

    // Row highlight when selected.
    let row_class = move || {
        if selected_task.with(|s| s.as_deref().is_some_and(|id| id == row_id.as_str())) {
            "cursor-pointer bg-primary/10"
        } else {
            "cursor-pointer hover:bg-base-300"
        }
    };

    view! {
        <tr
            class=row_class
            on:click=move |_| selected_task.set(Some(select_id.clone()))
        >
            <td class="font-mono text-sm">{name}</td>
            <td>
                <span class="badge badge-outline">{kind}</span>
            </td>
            <td class="text-xs opacity-70">{updated_at}</td>
            <td class="text-right">
                <button
                    class="btn btn-xs btn-error"
                    on:click=move |ev| {
                        // Don't also trigger the row-level selection.
                        ev.stop_propagation();
                        delete_target.set(Some(delete_task.clone()));
                    }
                >
                    "Delete"
                </button>
            </td>
        </tr>
    }
}

/// Right-hand detail panel for the currently selected task.
#[component]
#[allow(clippy::too_many_lines)]
fn TaskDetail(
    selected: RwSignal<Option<String>>,
    tasks: Resource<Result<Vec<WireTask>, ServerFnError>>,
    set_error: WriteSignal<Option<String>>,
) -> impl IntoView {
    // Per-task run history. Prepended to when a Run completes; refreshed
    // from the daemon whenever the selected task changes.
    let runs: RwSignal<Vec<WireTaskRun>> = RwSignal::new(Vec::new());
    let (runs_loading, set_runs_loading) = signal(false);
    let (run_in_flight, set_run_in_flight) = signal(false);

    // Re-fetch runs every time the selection flips.
    Effect::new(move |_| {
        let Some(id) = selected.get() else {
            runs.set(Vec::new());
            return;
        };
        set_runs_loading.set(true);
        let id_cloned = id.clone();
        spawn_local(async move {
            match manager_list_task_runs(id_cloned).await {
                Ok(list) => runs.set(list),
                Err(e) => {
                    runs.set(Vec::new());
                    set_error.set(Some(format!("Load runs failed: {e}")));
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
            let result = manager_run_task(id_cloned.clone()).await;
            set_run_in_flight.set(false);
            match result {
                Ok(run) => {
                    // Prepend the new run so it shows up first in the
                    // history table and drives the "latest output"
                    // panel below.
                    runs.update(|list| list.insert(0, run));
                }
                Err(e) => set_error.set(Some(format!("Run failed: {e}"))),
            }
        });
    };

    view! {
        {move || {
            // When nothing is selected, show the empty-state hint.
            let Some(task_id) = selected.get() else {
                return view! {
                    <div class="card bg-base-200">
                        <div class="card-body">
                            <p class="opacity-70">
                                "Select a task on the left to view details."
                            </p>
                        </div>
                    </div>
                }
                    .into_any();
            };

            // Look up the selected task in the current Resource snapshot.
            let task = tasks
                .get()
                .and_then(Result::ok)
                .and_then(|list| list.into_iter().find(|t| t.id == task_id));

            let Some(task) = task else {
                return view! {
                    <div class="card bg-base-200">
                        <div class="card-body">
                            <p class="opacity-70">"Loading task…"</p>
                        </div>
                    </div>
                }
                    .into_any();
            };

            let task_name = task.name.clone();
            let task_body = task.body.clone();
            let task_kind = task.kind.clone();
            let scope_label = task
                .project_id
                .as_deref()
                .map_or_else(|| "(global)".to_string(), |p| format!("project {p}"));

            let is_running_badge = move || {
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
                                    <h2 class="card-title font-mono">{task_name}</h2>
                                    <div class="flex gap-2 items-center mt-1 text-xs opacity-70">
                                        <span class="badge badge-outline badge-sm">
                                            {task_kind}
                                        </span>
                                        <span>{scope_label}</span>
                                    </div>
                                </div>
                                <div class="flex gap-2 items-center">
                                    {is_running_badge}
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
                                <div class="label-text mb-1 opacity-70">"Script body"</div>
                                <pre class="mockup-code text-xs overflow-x-auto">
                                    <code>{task_body}</code>
                                </pre>
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
                                    render_runs_table(&runs.get()).into_any()
                                }
                            }}
                        </div>
                    </div>

                    // Latest run output
                    <div class="card bg-base-200">
                        <div class="card-body">
                            <h3 class="card-title text-base">"Latest output"</h3>
                            {move || render_latest_output(&runs.get())}
                        </div>
                    </div>
                </div>
            }
                .into_any()
        }}
    }
}

fn render_runs_table(runs: &[WireTaskRun]) -> impl IntoView {
    if runs.is_empty() {
        return view! {
            <div class="p-2 opacity-70 text-sm">"(no runs yet)"</div>
        }
        .into_any();
    }

    view! {
        <div class="overflow-x-auto">
            <table class="table table-sm table-zebra">
                <thead>
                    <tr>
                        <th>"Run id"</th>
                        <th>"Started"</th>
                        <th>"Duration"</th>
                        <th>"Exit code"</th>
                    </tr>
                </thead>
                <tbody>
                    {runs.iter().map(render_run_row).collect_view()}
                </tbody>
            </table>
        </div>
    }
    .into_any()
}

fn render_run_row(run: &WireTaskRun) -> impl IntoView {
    let short_id: String = run.id.chars().take(8).collect();
    let duration = compute_duration(&run.started_at, run.finished_at.as_deref());
    let exit_badge = match run.exit_code {
        Some(0) => view! {
            <span class="badge badge-success badge-sm">"0"</span>
        }
        .into_any(),
        Some(code) => view! {
            <span class="badge badge-error badge-sm">{code.to_string()}</span>
        }
        .into_any(),
        None => view! {
            <span class="badge badge-warning badge-sm">"?"</span>
        }
        .into_any(),
    };
    let full_id = run.id.clone();
    let started_at = run.started_at.clone();

    view! {
        <tr>
            <td class="font-mono text-xs" title=full_id>{short_id}</td>
            <td class="text-xs">{started_at}</td>
            <td class="text-xs">{duration}</td>
            <td>{exit_badge}</td>
        </tr>
    }
}

/// Compute the duration between two RFC-3339 timestamps.
///
/// Returns a short human string like `"1.234s"` or `"(running)"` when
/// the finished timestamp is missing. If either timestamp fails to
/// parse we return `"?"` rather than bailing — this is diagnostic
/// fluff, not correctness-critical.
///
/// Implemented with a minimal RFC-3339 parser instead of pulling
/// `chrono` into the WASM build (the `secrets.rs` page avoids chrono
/// for the same reason).
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
    #[allow(clippy::cast_precision_loss)] // millis is bounded by wall-clock run times
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
///
/// Accepts the shapes the daemon emits (see `chrono::DateTime<Utc>` default
/// serialization):
///
/// - `YYYY-MM-DDTHH:MM:SSZ`
/// - `YYYY-MM-DDTHH:MM:SS.fff...Z`
/// - `YYYY-MM-DDTHH:MM:SS+00:00` / `-00:00` (offset honored to the minute)
///
/// Returns `None` on any parse error so the caller can fall back to a
/// placeholder rather than propagating a hard error into the UI.
fn rfc3339_to_unix_ms(s: &str) -> Option<i64> {
    // Split date & time on 'T'.
    let (date, rest) = s.split_once('T')?;
    let mut y_m_d = date.split('-');
    let year: i64 = y_m_d.next()?.parse().ok()?;
    let month: u32 = y_m_d.next()?.parse().ok()?;
    let day: u32 = y_m_d.next()?.parse().ok()?;

    // Detect and strip the timezone suffix. We only need sign + offset
    // in minutes; the offset is added back at the end.
    let (time_part, offset_mins): (&str, i64) = if let Some(stripped) = rest.strip_suffix('Z') {
        (stripped, 0)
    } else if let Some(idx) = rest.rfind(['+', '-']) {
        // Must come after the 'T' portion; the offset marker won't appear
        // in the time itself (colons separate H:M:S).
        let (t, tz) = rest.split_at(idx);
        // tz is like "+HH:MM" or "-HH:MM"
        let sign = if tz.starts_with('+') { 1 } else { -1 };
        let tz_body = &tz[1..];
        let mut hm = tz_body.split(':');
        let hh: i64 = hm.next()?.parse().ok()?;
        let mm: i64 = hm.next().unwrap_or("0").parse().ok()?;
        (t, sign * (hh * 60 + mm))
    } else {
        // No timezone suffix at all — treat as UTC, matching the
        // daemon's default (which always appends `Z`).
        (rest, 0)
    };

    // Time: HH:MM:SS(.fff...)
    let mut hms_frac = time_part.split('.');
    let hms = hms_frac.next()?;
    let frac = hms_frac.next().unwrap_or("");

    let mut hms_parts = hms.split(':');
    let hour: u32 = hms_parts.next()?.parse().ok()?;
    let minute: u32 = hms_parts.next()?.parse().ok()?;
    let second: u32 = hms_parts.next()?.parse().ok()?;

    // Convert date → days since 1970-01-01 via civil-from-days.
    let days = ymd_to_unix_days(year, month, day)?;

    let secs_in_day = i64::from(hour) * 3600 + i64::from(minute) * 60 + i64::from(second);
    let utc_secs = days * 86_400 + secs_in_day - offset_mins * 60;

    // Fractional seconds: take up to 3 digits (ms), pad with zeros.
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

/// Convert a proleptic-Gregorian (year, month, day) to days-since-
/// 1970-01-01. Lifted from the civil-from-days algorithm (Hinnant).
///
/// Stays in `i64` the whole way to avoid lossy casts between i64/u64;
/// all intermediate values are bounded to small positive numbers (era
/// is the 400-year chunk, `yoe` is 0..400, `doy` is 0..366) so the
/// signed arithmetic never overflows for any realistic wall-clock
/// timestamp.
fn ymd_to_unix_days(year: i64, month: u32, day: u32) -> Option<i64> {
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // 0..=399, always non-negative
    let m = i64::from(month);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe - 719_468)
}

fn render_latest_output(runs: &[WireTaskRun]) -> AnyView {
    let Some(latest) = runs.first() else {
        return view! {
            <div class="p-2 opacity-70 text-sm">
                "Run the task to capture stdout / stderr here."
            </div>
        }
        .into_any();
    };

    let stdout = latest.stdout.clone();
    let stderr = latest.stderr.clone();
    let has_stdout = !stdout.is_empty();
    let has_stderr = !stderr.is_empty();

    view! {
        <div class="space-y-3">
            {if has_stdout {
                view! {
                    <div>
                        <div class="label-text mb-1 opacity-70">"stdout"</div>
                        <pre class="mockup-code text-xs overflow-x-auto">
                            <code>{stdout}</code>
                        </pre>
                    </div>
                }
                    .into_any()
            } else {
                view! {
                    <div class="text-xs opacity-60">"(stdout empty)"</div>
                }
                    .into_any()
            }}
            {if has_stderr {
                view! {
                    <div>
                        <div class="label-text mb-1 opacity-70">"stderr"</div>
                        <pre class="mockup-code text-xs overflow-x-auto bg-error/20">
                            <code>{stderr}</code>
                        </pre>
                    </div>
                }
                    .into_any()
            } else {
                ().into_any()
            }}
        </div>
    }
    .into_any()
}

#[component]
fn NewTaskModal(
    open: ReadSignal<bool>,
    set_open: WriteSignal<bool>,
    set_error: WriteSignal<Option<String>>,
    on_success: impl Fn(String) + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let (name, set_name) = signal(String::new());
    let (body, set_body) = signal(String::new());
    let (project_id, set_project_id) = signal(String::new());
    let (submitting, set_submitting) = signal(false);

    let reset = move || {
        set_name.set(String::new());
        set_body.set(String::new());
        set_project_id.set(String::new());
    };

    let on_submit = move |ev: SubmitEvent| {
        ev.prevent_default();
        if submitting.get() {
            return;
        }

        let name_val = name.get().trim().to_string();
        let body_val = body.get();
        let project_val = project_id.get();

        if name_val.is_empty() {
            set_error.set(Some("Name is required.".to_string()));
            return;
        }
        if body_val.trim().is_empty() {
            set_error.set(Some("Script body is required.".to_string()));
            return;
        }

        let project_opt = if project_val.trim().is_empty() {
            None
        } else {
            Some(project_val.trim().to_string())
        };

        let spec = WireTaskSpec {
            name: name_val,
            kind: "bash".to_string(),
            body: body_val,
            project_id: project_opt,
        };

        set_submitting.set(true);
        spawn_local(async move {
            let result = manager_create_task(spec).await;
            set_submitting.set(false);
            match result {
                Ok(task) => {
                    reset();
                    set_open.set(false);
                    on_success(task.id);
                }
                Err(e) => set_error.set(Some(format!("Create failed: {e}"))),
            }
        });
    };

    view! {
        <dialog class=move || if open.get() { "modal modal-open" } else { "modal" }>
            <div class="modal-box max-w-2xl">
                <h3 class="font-bold text-lg mb-3">"New task"</h3>
                <form on:submit=on_submit class="space-y-3">
                    <div class="form-control">
                        <label class="label">
                            <span class="label-text">"Name"</span>
                        </label>
                        <input
                            type="text"
                            class="input input-bordered w-full font-mono"
                            required
                            placeholder="run-migrations"
                            prop:value=name
                            on:input=move |ev| set_name.set(event_target_value(&ev))
                        />
                    </div>
                    <div class="form-control">
                        <label class="label">
                            <span class="label-text">"Script body (bash)"</span>
                        </label>
                        <textarea
                            class="textarea textarea-bordered w-full font-mono"
                            rows="12"
                            required
                            placeholder="#!/usr/bin/env bash\nset -euo pipefail\necho hello"
                            prop:value=body
                            on:input=move |ev| set_body.set(event_target_value(&ev))
                        ></textarea>
                    </div>
                    <div class="form-control">
                        <label class="label">
                            <span class="label-text">
                                "Project id "<span class="opacity-60">"(optional)"</span>
                            </span>
                            <span class="label-text-alt opacity-70">"Blank = global task"</span>
                        </label>
                        <input
                            type="text"
                            class="input input-bordered w-full"
                            placeholder="(global)"
                            prop:value=project_id
                            on:input=move |ev| set_project_id.set(event_target_value(&ev))
                        />
                    </div>

                    <div class="modal-action">
                        <button
                            type="button"
                            class="btn btn-ghost"
                            on:click=move |_| {
                                reset();
                                set_open.set(false);
                            }
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
                on:submit=move |_| set_open.set(false)
            >
                <button>"close"</button>
            </form>
        </dialog>
    }
}

#[component]
fn DeleteTaskModal(
    target: RwSignal<Option<WireTask>>,
    selected: RwSignal<Option<String>>,
    set_error: WriteSignal<Option<String>>,
    on_success: impl Fn() + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let (submitting, set_submitting) = signal(false);

    let close = move || target.set(None);

    let confirm = move |_| {
        let Some(task) = target.get() else {
            return;
        };
        set_submitting.set(true);
        spawn_local(async move {
            let result = manager_delete_task(task.id.clone()).await;
            set_submitting.set(false);
            match result {
                Ok(()) => {
                    // If the deleted task was currently selected, clear the
                    // detail pane.
                    if selected.with(|s| s.as_deref() == Some(task.id.as_str())) {
                        selected.set(None);
                    }
                    close();
                    on_success();
                }
                Err(e) => set_error.set(Some(format!("Delete failed: {e}"))),
            }
        });
    };

    view! {
        <dialog class=move || {
            if target.with(Option::is_some) { "modal modal-open" } else { "modal" }
        }>
            <div class="modal-box">
                <h3 class="font-bold text-lg mb-3">"Delete task"</h3>
                <p>
                    {move || {
                        target
                            .get()
                            .map(|t| format!("Delete task '{}'? This cannot be undone.", t.name))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_parser_basic_z() {
        // 2026-04-16T00:00:00Z → 2026-04-16 epoch = 1_776_470_400 seconds.
        // Verify against a second call that offsets by a known amount.
        let a = rfc3339_to_unix_ms("2026-04-16T00:00:00Z").unwrap();
        let b = rfc3339_to_unix_ms("2026-04-16T00:00:01Z").unwrap();
        assert_eq!(b - a, 1000);
    }

    #[test]
    fn rfc3339_parser_handles_fractional_seconds() {
        let a = rfc3339_to_unix_ms("2026-04-16T00:00:00Z").unwrap();
        let b = rfc3339_to_unix_ms("2026-04-16T00:00:00.250Z").unwrap();
        assert_eq!(b - a, 250);
    }

    #[test]
    fn rfc3339_parser_handles_offset() {
        // +01:00 means the UTC time is one hour earlier than the wall-clock.
        let utc = rfc3339_to_unix_ms("2026-04-16T00:00:00Z").unwrap();
        let offset = rfc3339_to_unix_ms("2026-04-16T01:00:00+01:00").unwrap();
        assert_eq!(utc, offset);
    }

    #[test]
    fn rfc3339_parser_rejects_garbage() {
        assert!(rfc3339_to_unix_ms("not-a-date").is_none());
        assert!(rfc3339_to_unix_ms("").is_none());
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
    fn compute_duration_multi_minute_uses_m_s_format() {
        assert_eq!(
            compute_duration("2026-04-16T00:00:00Z", Some("2026-04-16T00:15:30Z")),
            "15m30s".to_string()
        );
    }

    #[test]
    fn compute_duration_unparseable_returns_placeholder() {
        assert_eq!(
            compute_duration("garbage", Some("2026-04-16T00:00:00Z")),
            "?".to_string()
        );
    }

    #[test]
    fn compute_duration_negative_returns_placeholder() {
        // finished before started → nonsense, return placeholder rather than
        // showing a negative number in the UI.
        assert_eq!(
            compute_duration("2026-04-16T00:00:10Z", Some("2026-04-16T00:00:00Z")),
            "?".to_string()
        );
    }
}
