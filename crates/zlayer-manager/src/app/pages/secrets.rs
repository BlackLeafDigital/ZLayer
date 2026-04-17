//! `/secrets` — environment-scoped secrets management.
//!
//! Matches the Users + Variables CRUD idiom:
//!
//! - Env tab filter at the top (`all` + one tab per environment).
//! - Masked value column with per-row reveal (auto-hides after 10s).
//! - Three modals for single-secret create/edit/delete and a fourth for
//!   `.env` bulk import with a client-side preview.
//!
//! The daemon identifies a secret by `(scope, name)` — there is no server
//! UUID, so the UI uses `(environment_id, name)` as the row key. The
//! environment tab always flows through to the server fn as the
//! `environment` query param.

use leptos::ev::SubmitEvent;
use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::app::components::forms::TabBar;
use crate::app::server_fns::{
    manager_bulk_import_secrets, manager_create_secret, manager_delete_secret,
    manager_list_environments, manager_list_secrets, manager_reveal_secret, manager_update_secret,
};
use crate::wire::secrets::{BulkImportResult, WireEnvironment, WireSecret};

/// The "all" sentinel value used as the default env tab key. It never
/// collides with a real env id (env ids are UUIDs).
const ALL_ENVS: &str = "all";

/// Convert an env tab value to the `environment` query param — `None`
/// means "list the default scope / don't filter".
fn tab_to_env(tab: &str) -> Option<String> {
    if tab == ALL_ENVS || tab.is_empty() {
        None
    } else {
        Some(tab.to_string())
    }
}

/// Top-level Secrets page component.
#[component]
#[allow(clippy::too_many_lines)] // view DSL + four modals inline.
pub fn Secrets() -> impl IntoView {
    let env_tab: RwSignal<String> = RwSignal::new(ALL_ENVS.to_string());
    let envs = Resource::new(|| (), |()| async move { manager_list_environments().await });
    let secrets = Resource::new(
        move || env_tab.get(),
        |tab| async move { manager_list_secrets(tab_to_env(&tab)).await },
    );

    let (new_open, set_new_open) = signal(false);
    let (bulk_open, set_bulk_open) = signal(false);
    let (error, set_error) = signal(None::<String>);
    let edit_target = RwSignal::new(None::<WireSecret>);
    let delete_target = RwSignal::new(None::<WireSecret>);

    let refetch = move || secrets.refetch();

    // Build the tabs view reactively off the env list.
    let tabs_view = move || {
        envs.get().map(|res| match res {
            Err(e) => view! {
                <div class="alert alert-warning text-sm">
                    <span>{format!("Failed to load environments: {e}")}</span>
                </div>
            }
            .into_any(),
            Ok(env_list) => {
                let mut tabs: Vec<(String, String)> =
                    vec![(ALL_ENVS.to_string(), "All".to_string())];
                for env in &env_list {
                    let label = match env.project_id.as_deref() {
                        Some(pid) if !pid.is_empty() => format!("{} ({})", env.name, pid),
                        _ => env.name.clone(),
                    };
                    tabs.push((env.id.clone(), label));
                }
                view! { <TabBar tabs=tabs selected=env_tab /> }.into_any()
            }
        })
    };

    view! {
        <div class="p-6 space-y-4">
            <div class="flex justify-between items-center">
                <h1 class="text-2xl font-bold">"Secrets"</h1>
                <div class="flex gap-2">
                    <button
                        class="btn btn-ghost btn-sm"
                        on:click=move |_| set_bulk_open.set(true)
                    >
                        "Bulk import .env"
                    </button>
                    <button
                        class="btn btn-primary btn-sm"
                        on:click=move |_| set_new_open.set(true)
                    >
                        "New secret"
                    </button>
                </div>
            </div>

            {tabs_view}

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

            <div class="card bg-base-200">
                <div class="card-body p-0 overflow-x-auto">
                    <Suspense fallback=move || view! {
                        <div class="p-6 flex justify-center">
                            <span class="loading loading-spinner loading-md"></span>
                        </div>
                    }>
                        {move || {
                            secrets.get().map(|res| match res {
                                Err(e) => view! {
                                    <div class="p-4 text-error">
                                        {format!("Failed to load secrets: {e}")}
                                    </div>
                                }
                                .into_any(),
                                Ok(list) => render_table(
                                    list,
                                    env_tab,
                                    edit_target,
                                    delete_target,
                                    set_error,
                                )
                                .into_any(),
                            })
                        }}
                    </Suspense>
                </div>
            </div>

            <NewSecretModal
                open=new_open
                set_open=set_new_open
                env_tab=env_tab
                envs_resource=envs
                set_error=set_error
                on_success=move || { refetch(); }
            />

            <EditSecretModal
                target=edit_target
                env_tab=env_tab
                set_error=set_error
                on_success=move || { refetch(); }
            />

            <DeleteSecretModal
                target=delete_target
                env_tab=env_tab
                set_error=set_error
                on_success=move || { refetch(); }
            />

            <BulkImportModal
                open=bulk_open
                set_open=set_bulk_open
                env_tab=env_tab
                envs_resource=envs
                set_error=set_error
                on_success=move || { refetch(); }
            />
        </div>
    }
}

fn render_table(
    list: Vec<WireSecret>,
    env_tab: RwSignal<String>,
    edit_target: RwSignal<Option<WireSecret>>,
    delete_target: RwSignal<Option<WireSecret>>,
    set_error: WriteSignal<Option<String>>,
) -> impl IntoView {
    if list.is_empty() {
        return view! { <div class="p-4 opacity-70">"(no secrets in this scope)"</div> }.into_any();
    }

    view! {
        <table class="table table-zebra">
            <thead>
                <tr>
                    <th>"Name"</th>
                    <th>"Value"</th>
                    <th>"Version"</th>
                    <th>"Updated"</th>
                    <th class="text-right">"Actions"</th>
                </tr>
            </thead>
            <tbody>
                {list
                    .into_iter()
                    .map(|s| render_row(&s, env_tab, edit_target, delete_target, set_error))
                    .collect_view()}
            </tbody>
        </table>
    }
    .into_any()
}

fn render_row(
    secret: &WireSecret,
    env_tab: RwSignal<String>,
    edit_target: RwSignal<Option<WireSecret>>,
    delete_target: RwSignal<Option<WireSecret>>,
    set_error: WriteSignal<Option<String>>,
) -> impl IntoView {
    let edit_secret = secret.clone();
    let delete_secret = secret.clone();
    let name_for_reveal = secret.name.clone();
    let updated = format_unix(secret.updated_at);

    // Per-row reveal state. `None` = masked, `Some(value)` = revealed.
    let revealed: RwSignal<Option<String>> = RwSignal::new(None);
    let (revealing, set_revealing) = signal(false);

    // `value_cell` is a reactive closure that re-runs on every
    // `revealed` change, so its on:click handlers must be `FnMut`-safe.
    // We clone the captured `name` into each handler fresh, which keeps
    // the outer closure cloneable and re-entrant.
    let value_cell = move || match revealed.get() {
        None => {
            let name_for_click = name_for_reveal.clone();
            view! {
                <div class="flex items-center gap-2">
                    <span class="font-mono text-xs opacity-60">"••••••••"</span>
                    <button
                        class="btn btn-xs btn-ghost"
                        disabled=move || revealing.get()
                        on:click=move |_| {
                            if revealing.get() || revealed.get().is_some() {
                                return;
                            }
                            set_revealing.set(true);
                            let env = tab_to_env(&env_tab.get());
                            let name = name_for_click.clone();
                            spawn_local(async move {
                                match manager_reveal_secret(name, env).await {
                                    Ok(v) => {
                                        revealed.set(Some(v));
                                        set_revealing.set(false);
                                        schedule_auto_hide(revealed);
                                    }
                                    Err(e) => {
                                        set_revealing.set(false);
                                        set_error.set(Some(format!("Reveal failed: {e}")));
                                    }
                                }
                            });
                        }
                    >
                        {move || if revealing.get() { "Revealing…" } else { "Reveal" }}
                    </button>
                </div>
            }
            .into_any()
        }
        Some(v) => view! {
            <div class="flex items-center gap-2">
                <code class="font-mono text-xs break-all">{v}</code>
                <button
                    class="btn btn-xs btn-ghost"
                    on:click=move |_| revealed.set(None)
                >
                    "Hide"
                </button>
                <span class="text-xs opacity-60">"(auto-hides in 10s)"</span>
            </div>
        }
        .into_any(),
    };

    view! {
        <tr>
            <td class="font-mono text-sm">{secret.name.clone()}</td>
            <td class="max-w-md">{value_cell}</td>
            <td class="text-xs opacity-70">{secret.version}</td>
            <td class="text-xs opacity-70">{updated}</td>
            <td class="text-right space-x-1">
                <button
                    class="btn btn-xs btn-ghost"
                    on:click=move |_| edit_target.set(Some(edit_secret.clone()))
                >
                    "Edit"
                </button>
                <button
                    class="btn btn-xs btn-error"
                    on:click=move |_| delete_target.set(Some(delete_secret.clone()))
                >
                    "Delete"
                </button>
            </td>
        </tr>
    }
}

/// Schedule the per-row `revealed` signal to auto-clear in 10 seconds.
///
/// Hydrate: uses `gloo_timers::callback::Timeout` which is safe in WASM.
/// SSR: no-op (server-rendered tables never have reveal state anyway — the
/// user has to click a button which fires after hydration).
fn schedule_auto_hide(revealed: RwSignal<Option<String>>) {
    #[cfg(feature = "hydrate")]
    {
        gloo_timers::callback::Timeout::new(10_000, move || {
            revealed.set(None);
        })
        .forget();
    }
    #[cfg(not(feature = "hydrate"))]
    {
        // SSR — nothing to do. Silence unused warning.
        let _ = revealed;
    }
}

/// Render a unix timestamp as a short `YYYY-MM-DD HH:MM` UTC label.
///
/// Non-positive timestamps render as `"-"`; malformed / overflowing
/// values fall back to the same. Uses chrono (already in the manager's
/// deps) rather than a hand-rolled Gregorian calc.
fn format_unix(ts: i64) -> String {
    if ts <= 0 {
        return "-".to_string();
    }
    chrono::DateTime::<chrono::Utc>::from_timestamp(ts, 0).map_or_else(
        || "-".to_string(),
        |dt| dt.format("%Y-%m-%d %H:%M").to_string(),
    )
}

// ---------------------------------------------------------------------------
// Modals
// ---------------------------------------------------------------------------

#[component]
fn NewSecretModal(
    open: ReadSignal<bool>,
    set_open: WriteSignal<bool>,
    env_tab: RwSignal<String>,
    envs_resource: Resource<Result<Vec<WireEnvironment>, ServerFnError>>,
    set_error: WriteSignal<Option<String>>,
    on_success: impl Fn() + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let (name, set_name) = signal(String::new());
    let (value, set_value) = signal(String::new());
    let (env_choice, set_env_choice) = signal(String::new());
    let (submitting, set_submitting) = signal(false);

    // Seed the env selector from the current tab when the modal opens.
    Effect::new(move |_| {
        if open.get() {
            let tab = env_tab.get();
            set_env_choice.set(if tab == ALL_ENVS { String::new() } else { tab });
        }
    });

    let reset = move || {
        set_name.set(String::new());
        set_value.set(String::new());
        set_env_choice.set(String::new());
    };

    let on_submit = move |ev: SubmitEvent| {
        ev.prevent_default();
        if submitting.get() {
            return;
        }
        let name_val = name.get().trim().to_string();
        if name_val.is_empty() {
            set_error.set(Some("Name is required.".to_string()));
            return;
        }
        let value_val = value.get();
        let env_val = env_choice.get();
        let env_opt = if env_val.is_empty() {
            None
        } else {
            Some(env_val)
        };

        set_submitting.set(true);
        spawn_local(async move {
            let result = manager_create_secret(name_val, value_val, env_opt).await;
            set_submitting.set(false);
            match result {
                Ok(_) => {
                    reset();
                    set_open.set(false);
                    on_success();
                }
                Err(e) => set_error.set(Some(format!("Create failed: {e}"))),
            }
        });
    };

    let env_options = move || {
        envs_resource.get().and_then(Result::ok).map(|envs| {
            envs.into_iter()
                .map(|e| {
                    let label = match e.project_id.as_deref() {
                        Some(pid) if !pid.is_empty() => format!("{} ({})", e.name, pid),
                        _ => e.name.clone(),
                    };
                    view! { <option value=e.id>{label}</option> }
                })
                .collect_view()
        })
    };

    view! {
        <dialog class=move || if open.get() { "modal modal-open" } else { "modal" }>
            <div class="modal-box">
                <h3 class="font-bold text-lg mb-3">"New secret"</h3>
                <form on:submit=on_submit class="space-y-3">
                    <div class="form-control">
                        <label class="label">
                            <span class="label-text">"Name"</span>
                        </label>
                        <input
                            type="text"
                            class="input input-bordered w-full font-mono"
                            required
                            placeholder="DATABASE_URL"
                            autocomplete="off"
                            spellcheck="false"
                            prop:value=name
                            on:input=move |ev| set_name.set(event_target_value(&ev))
                        />
                    </div>
                    <div class="form-control">
                        <label class="label">
                            <span class="label-text">"Value"</span>
                        </label>
                        <textarea
                            class="textarea textarea-bordered w-full font-mono h-24"
                            autocomplete="off"
                            spellcheck="false"
                            prop:value=value
                            on:input=move |ev| set_value.set(event_target_value(&ev))
                        ></textarea>
                    </div>
                    <div class="form-control">
                        <label class="label">
                            <span class="label-text">"Environment"</span>
                            <span class="label-text-alt opacity-70">"blank = default scope"</span>
                        </label>
                        <select
                            class="select select-bordered w-full"
                            prop:value=env_choice
                            on:change=move |ev| set_env_choice.set(event_target_value(&ev))
                        >
                            <option value="">"(default / global scope)"</option>
                            {env_options}
                        </select>
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
fn EditSecretModal(
    target: RwSignal<Option<WireSecret>>,
    env_tab: RwSignal<String>,
    set_error: WriteSignal<Option<String>>,
    on_success: impl Fn() + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let (value, set_value) = signal(String::new());
    let (submitting, set_submitting) = signal(false);

    // Clear the value field every time the modal reopens. The daemon
    // never lets us preview the current value here (reveal is a separate
    // admin call), so we just ask the user to type the new value fresh.
    Effect::new(move |_| {
        if target.get().is_some() {
            set_value.set(String::new());
        }
    });

    let close = move || {
        target.set(None);
        set_value.set(String::new());
    };

    let on_submit = move |ev: SubmitEvent| {
        ev.prevent_default();
        let Some(secret) = target.get() else {
            return;
        };
        if submitting.get() {
            return;
        }
        let new_value = value.get();
        if new_value.is_empty() {
            set_error.set(Some("Value is required.".to_string()));
            return;
        }
        let env = tab_to_env(&env_tab.get());

        set_submitting.set(true);
        spawn_local(async move {
            let result = manager_update_secret(secret.name.clone(), new_value, env).await;
            set_submitting.set(false);
            match result {
                Ok(_) => {
                    close();
                    on_success();
                }
                Err(e) => set_error.set(Some(format!("Update failed: {e}"))),
            }
        });
    };

    view! {
        <dialog class=move || {
            if target.with(Option::is_some) { "modal modal-open" } else { "modal" }
        }>
            <div class="modal-box">
                <h3 class="font-bold text-lg mb-3">
                    {move || target.get().map(|s| format!("Edit secret: {}", s.name)).unwrap_or_default()}
                </h3>
                <p class="text-xs opacity-70 mb-2">
                    "Name and environment are immutable. Enter a new value to overwrite."
                </p>
                <form on:submit=on_submit class="space-y-3">
                    <div class="form-control">
                        <label class="label">
                            <span class="label-text">"New value"</span>
                        </label>
                        <textarea
                            class="textarea textarea-bordered w-full font-mono h-32"
                            autocomplete="off"
                            spellcheck="false"
                            prop:value=value
                            on:input=move |ev| set_value.set(event_target_value(&ev))
                        ></textarea>
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
                            {move || if submitting.get() { "Saving…" } else { "Save" }}
                        </button>
                    </div>
                </form>
            </div>
        </dialog>
    }
}

#[component]
fn DeleteSecretModal(
    target: RwSignal<Option<WireSecret>>,
    env_tab: RwSignal<String>,
    set_error: WriteSignal<Option<String>>,
    on_success: impl Fn() + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let (submitting, set_submitting) = signal(false);

    let close = move || target.set(None);

    let confirm = move |_| {
        let Some(secret) = target.get() else {
            return;
        };
        let env = tab_to_env(&env_tab.get());
        set_submitting.set(true);
        spawn_local(async move {
            let result = manager_delete_secret(secret.name.clone(), env).await;
            set_submitting.set(false);
            match result {
                Ok(()) => {
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
                <h3 class="font-bold text-lg mb-3">"Delete secret"</h3>
                <p>
                    {move || {
                        target
                            .get()
                            .map(|s| format!("Delete secret '{}'? This cannot be undone.", s.name))
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

/// Client-side parse of a `.env`-style blob.
///
/// Rules mirror the daemon's bulk-import parser so what the user previews
/// matches what the server ingests:
///
/// - Blank lines and `#`-prefixed comments are skipped.
/// - A leading `export ` is stripped (bash-style `.env` files).
/// - Values are split on the FIRST `=` only.
/// - Keys and values are trimmed; a matching pair of surrounding single
///   or double quotes is stripped from the value.
///
/// Returns `(entries, skipped_count)`. `skipped_count` includes blank +
/// comment + malformed lines so the preview can show "M skipped".
pub fn parse_dotenv(raw: &str) -> (Vec<(String, String)>, usize) {
    let mut entries = Vec::new();
    let mut skipped = 0usize;

    for line_raw in raw.lines() {
        let line = line_raw.trim();
        if line.is_empty() || line.starts_with('#') {
            skipped += 1;
            continue;
        }
        let stripped = line.strip_prefix("export ").unwrap_or(line);
        let Some((k_raw, v_raw)) = stripped.split_once('=') else {
            skipped += 1;
            continue;
        };
        let key = k_raw.trim();
        if key.is_empty() {
            skipped += 1;
            continue;
        }
        let value = strip_quotes(v_raw.trim());
        entries.push((key.to_string(), value.to_string()));
    }

    (entries, skipped)
}

fn strip_quotes(v: &str) -> &str {
    let b = v.as_bytes();
    if b.len() >= 2 {
        let first = b[0];
        let last = b[b.len() - 1];
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return &v[1..v.len() - 1];
        }
    }
    v
}

#[component]
fn BulkImportModal(
    open: ReadSignal<bool>,
    set_open: WriteSignal<bool>,
    env_tab: RwSignal<String>,
    envs_resource: Resource<Result<Vec<WireEnvironment>, ServerFnError>>,
    set_error: WriteSignal<Option<String>>,
    on_success: impl Fn() + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let (body_text, set_body_text) = signal(String::new());
    let (env_choice, set_env_choice) = signal(String::new());
    let (submitting, set_submitting) = signal(false);
    let (result, set_result) = signal(None::<BulkImportResult>);

    // Seed env choice from the active tab, but only when the tab
    // identifies a real env. Bulk import requires a real env id.
    Effect::new(move |_| {
        if open.get() {
            let tab = env_tab.get();
            if tab == ALL_ENVS {
                set_env_choice.set(String::new());
            } else {
                set_env_choice.set(tab);
            }
            set_result.set(None);
        }
    });

    let close = move || {
        set_open.set(false);
        set_body_text.set(String::new());
        set_result.set(None);
    };

    let env_options = move || {
        envs_resource.get().and_then(Result::ok).map(|envs| {
            envs.into_iter()
                .map(|e| {
                    let label = match e.project_id.as_deref() {
                        Some(pid) if !pid.is_empty() => format!("{} ({})", e.name, pid),
                        _ => e.name.clone(),
                    };
                    view! { <option value=e.id>{label}</option> }
                })
                .collect_view()
        })
    };

    // Reactive parse preview for the currently-typed body.
    let parse_summary = move || {
        let body = body_text.get();
        let (entries, skipped) = parse_dotenv(&body);
        format!("{} pairs parsed, {skipped} skipped", entries.len())
    };

    let on_submit = move |ev: SubmitEvent| {
        ev.prevent_default();
        if submitting.get() {
            return;
        }
        let env = env_choice.get();
        if env.is_empty() {
            set_error.set(Some(
                "Choose an environment — bulk import requires one.".to_string(),
            ));
            return;
        }
        let body = body_text.get();
        let (entries, _skipped) = parse_dotenv(&body);
        if entries.is_empty() {
            set_error.set(Some(
                "Nothing to import — no key=value pairs found.".to_string(),
            ));
            return;
        }

        set_submitting.set(true);
        spawn_local(async move {
            let res = manager_bulk_import_secrets(Some(env), entries).await;
            set_submitting.set(false);
            match res {
                Ok(r) => {
                    set_result.set(Some(r));
                    on_success();
                }
                Err(e) => set_error.set(Some(format!("Bulk import failed: {e}"))),
            }
        });
    };

    let result_view = move || {
        result.get().map(|r| {
            let errors: Vec<String> = r.errors.clone();
            view! {
                <div class="alert alert-info flex-col items-start mt-3">
                    <div>
                        {format!("Created: {} · Updated: {} · Errors: {}", r.created, r.updated, errors.len())}
                    </div>
                    {(!errors.is_empty()).then(|| view! {
                        <ul class="mt-2 list-disc list-inside text-xs">
                            {errors.into_iter()
                                .map(|e| view! { <li>{e}</li> })
                                .collect_view()}
                        </ul>
                    })}
                </div>
            }
        })
    };

    view! {
        <dialog class=move || if open.get() { "modal modal-open" } else { "modal" }>
            <div class="modal-box max-w-2xl">
                <h3 class="font-bold text-lg mb-3">"Bulk import secrets from .env"</h3>
                <p class="text-xs opacity-70 mb-2">
                    "Paste a .env file. Blank lines, lines starting with # and lines without '=' are skipped. Each remaining KEY=value pair is upserted into the chosen environment."
                </p>
                <form on:submit=on_submit class="space-y-3">
                    <div class="form-control">
                        <label class="label">
                            <span class="label-text">"Environment"</span>
                            <span class="label-text-alt opacity-70">"required"</span>
                        </label>
                        <select
                            class="select select-bordered w-full"
                            required
                            prop:value=env_choice
                            on:change=move |ev| set_env_choice.set(event_target_value(&ev))
                        >
                            <option value="">"(choose environment)"</option>
                            {env_options}
                        </select>
                    </div>

                    <div class="form-control">
                        <label class="label">
                            <span class="label-text">".env content"</span>
                            <span class="label-text-alt opacity-70">{parse_summary}</span>
                        </label>
                        <textarea
                            class="textarea textarea-bordered w-full font-mono text-xs h-64"
                            placeholder="KEY=value
# comment
export OTHER=\"value with spaces\""
                            autocomplete="off"
                            spellcheck="false"
                            prop:value=body_text
                            on:input=move |ev| set_body_text.set(event_target_value(&ev))
                        ></textarea>
                    </div>

                    {result_view}

                    <div class="modal-action">
                        <button
                            type="button"
                            class="btn btn-ghost"
                            on:click=move |_| close()
                        >
                            "Close"
                        </button>
                        <button
                            type="submit"
                            class="btn btn-primary"
                            disabled=move || submitting.get()
                        >
                            {move || if submitting.get() { "Importing…" } else { "Import" }}
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_to_env_converts_all_to_none() {
        assert_eq!(tab_to_env(ALL_ENVS), None);
        assert_eq!(tab_to_env(""), None);
        assert_eq!(tab_to_env("abc-123"), Some("abc-123".to_string()));
    }

    #[test]
    fn parse_dotenv_strips_comments_and_blanks() {
        let input = "
# comment
FOO=bar

# another
BAZ=qux
";
        let (entries, skipped) = parse_dotenv(input);
        assert_eq!(
            entries,
            vec![
                ("FOO".to_string(), "bar".to_string()),
                ("BAZ".to_string(), "qux".to_string()),
            ]
        );
        // 2 blanks + 2 comments = 4 skipped.
        assert_eq!(skipped, 4);
    }

    #[test]
    fn parse_dotenv_strips_export_prefix() {
        let (entries, _) = parse_dotenv("export DATABASE_URL=postgres://x");
        assert_eq!(
            entries,
            vec![("DATABASE_URL".to_string(), "postgres://x".to_string())]
        );
    }

    #[test]
    fn parse_dotenv_splits_on_first_equals() {
        let (entries, _) = parse_dotenv("A=1=2=3");
        assert_eq!(entries, vec![("A".to_string(), "1=2=3".to_string())]);
    }

    #[test]
    fn parse_dotenv_strips_matching_quotes() {
        let (entries, _) = parse_dotenv("A=\"hello world\"\nB='also quoted'");
        assert_eq!(
            entries,
            vec![
                ("A".to_string(), "hello world".to_string()),
                ("B".to_string(), "also quoted".to_string()),
            ]
        );
    }

    #[test]
    fn parse_dotenv_counts_malformed_as_skipped() {
        let (entries, skipped) = parse_dotenv("no_equals_here\n=no_key\nREAL=ok");
        assert_eq!(entries, vec![("REAL".to_string(), "ok".to_string())]);
        assert_eq!(skipped, 2);
    }

    #[test]
    fn format_unix_negative_or_zero_is_dash() {
        assert_eq!(format_unix(0), "-");
        assert_eq!(format_unix(-5), "-");
    }

    #[test]
    fn format_unix_renders_known_timestamp() {
        // 1_776_297_600 == 2026-04-16T00:00:00Z (verified with
        // `date -d '2026-04-16T00:00:00Z' +%s`).
        let s = format_unix(1_776_297_600);
        assert_eq!(s, "2026-04-16 00:00");
    }
}
