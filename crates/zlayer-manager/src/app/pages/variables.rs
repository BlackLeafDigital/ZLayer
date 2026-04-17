//! `/variables` — workspace variables (plaintext key/value).
//!
//! All authenticated users can browse; only admins can mutate (the backend
//! enforces on every request). Matches the CRUD pattern used by the Users
//! page.

use leptos::ev::SubmitEvent;
use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::app::server_fns::{
    manager_create_variable, manager_delete_variable, manager_list_variables,
    manager_update_variable,
};
use crate::wire::variables::WireVariable;

/// Variables management page.
#[component]
#[allow(clippy::too_many_lines)] // view macro DSL + multiple modals
pub fn Variables() -> impl IntoView {
    let vars = Resource::new(|| (), |()| async move { manager_list_variables().await });
    let (new_open, set_new_open) = signal(false);
    let (error, set_error) = signal(None::<String>);
    let edit_target = RwSignal::new(None::<WireVariable>);
    let delete_target = RwSignal::new(None::<WireVariable>);

    let refetch = move || vars.refetch();

    view! {
        <div class="p-6 space-y-4">
            <div class="flex justify-between items-center">
                <h1 class="text-2xl font-bold">"Variables"</h1>
                <button
                    class="btn btn-primary btn-sm"
                    on:click=move |_| set_new_open.set(true)
                >
                    "New variable"
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

            <div class="card bg-base-200">
                <div class="card-body p-0 overflow-x-auto">
                    <Suspense fallback=move || {
                        view! {
                            <div class="p-6 flex justify-center">
                                <span class="loading loading-spinner loading-md"></span>
                            </div>
                        }
                    }>
                        {move || {
                            vars.get()
                                .map(|res| match res {
                                    Err(e) => {
                                        view! {
                                            <div class="p-4 text-error">
                                                {format!("Failed to load variables: {e}")}
                                            </div>
                                        }
                                            .into_any()
                                    }
                                    Ok(list) => render_table(list, edit_target, delete_target).into_any(),
                                })
                        }}
                    </Suspense>
                </div>
            </div>

            <NewVariableModal
                open=new_open
                set_open=set_new_open
                set_error=set_error
                on_success=move || {
                    refetch();
                }
            />

            <EditVariableModal
                target=edit_target
                set_error=set_error
                on_success=move || {
                    refetch();
                }
            />

            <DeleteVariableModal
                target=delete_target
                set_error=set_error
                on_success=move || {
                    refetch();
                }
            />
        </div>
    }
}

fn render_table(
    list: Vec<WireVariable>,
    edit_target: RwSignal<Option<WireVariable>>,
    delete_target: RwSignal<Option<WireVariable>>,
) -> impl IntoView {
    if list.is_empty() {
        return view! { <div class="p-4 opacity-70">"(no variables)"</div> }.into_any();
    }

    view! {
        <table class="table table-zebra">
            <thead>
                <tr>
                    <th>"Name"</th>
                    <th>"Value"</th>
                    <th>"Scope"</th>
                    <th>"Updated"</th>
                    <th class="text-right">"Actions"</th>
                </tr>
            </thead>
            <tbody>
                {list
                    .into_iter()
                    .map(|v| render_row(&v, edit_target, delete_target))
                    .collect_view()}
            </tbody>
        </table>
    }
    .into_any()
}

fn render_row(
    var: &WireVariable,
    edit_target: RwSignal<Option<WireVariable>>,
    delete_target: RwSignal<Option<WireVariable>>,
) -> impl IntoView {
    let scope_badge = var.scope.as_ref().map(|s| {
        view! { <span class="badge badge-outline">{s.clone()}</span> }
    });

    let edit_var = var.clone();
    let delete_var = var.clone();
    let tooltip_value = var.value.clone();

    view! {
        <tr>
            <td class="font-mono text-sm">{var.name.clone()}</td>
            <td class="font-mono text-xs max-w-xs">
                <div class="truncate" title=tooltip_value>{var.value.clone()}</div>
            </td>
            <td>{scope_badge}</td>
            <td class="text-xs opacity-70">{var.updated_at.clone()}</td>
            <td class="text-right space-x-1">
                <button
                    class="btn btn-xs btn-ghost"
                    on:click=move |_| edit_target.set(Some(edit_var.clone()))
                >
                    "Edit"
                </button>
                <button
                    class="btn btn-xs btn-error"
                    on:click=move |_| delete_target.set(Some(delete_var.clone()))
                >
                    "Delete"
                </button>
            </td>
        </tr>
    }
}

#[component]
fn NewVariableModal(
    open: ReadSignal<bool>,
    set_open: WriteSignal<bool>,
    set_error: WriteSignal<Option<String>>,
    on_success: impl Fn() + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let (name, set_name) = signal(String::new());
    let (value, set_value) = signal(String::new());
    let (scope, set_scope) = signal(String::new());
    let (submitting, set_submitting) = signal(false);

    let reset = move || {
        set_name.set(String::new());
        set_value.set(String::new());
        set_scope.set(String::new());
    };

    let on_submit = move |ev: SubmitEvent| {
        ev.prevent_default();
        if submitting.get() {
            return;
        }

        let name_val = name.get().trim().to_string();
        let value_val = value.get();
        let scope_val = scope.get();

        if name_val.is_empty() {
            set_error.set(Some("Name is required.".to_string()));
            return;
        }

        set_submitting.set(true);

        spawn_local(async move {
            let scope_opt = if scope_val.trim().is_empty() {
                None
            } else {
                Some(scope_val.trim().to_string())
            };
            let result = manager_create_variable(name_val, value_val, scope_opt).await;
            set_submitting.set(false);
            match result {
                Ok(_) => {
                    reset();
                    set_open.set(false);
                    on_success();
                }
                Err(e) => {
                    set_error.set(Some(format!("Create failed: {e}")));
                }
            }
        });
    };

    view! {
        <dialog class=move || if open.get() { "modal modal-open" } else { "modal" }>
            <div class="modal-box">
                <h3 class="font-bold text-lg mb-3">"New variable"</h3>
                <form on:submit=on_submit class="space-y-3">
                    <div class="form-control">
                        <label class="label">
                            <span class="label-text">"Name"</span>
                        </label>
                        <input
                            type="text"
                            class="input input-bordered w-full font-mono"
                            required
                            placeholder="APP_VERSION"
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
                            prop:value=value
                            on:input=move |ev| set_value.set(event_target_value(&ev))
                        ></textarea>
                    </div>
                    <div class="form-control">
                        <label class="label">
                            <span class="label-text">"Scope "<span class="opacity-60">"(optional)"</span></span>
                            <span class="label-text-alt opacity-70">"Project id; blank = global"</span>
                        </label>
                        <input
                            type="text"
                            class="input input-bordered w-full"
                            placeholder="(global)"
                            prop:value=scope
                            on:input=move |ev| set_scope.set(event_target_value(&ev))
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
fn EditVariableModal(
    target: RwSignal<Option<WireVariable>>,
    set_error: WriteSignal<Option<String>>,
    on_success: impl Fn() + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let (value, set_value) = signal(String::new());
    let (submitting, set_submitting) = signal(false);

    // Seed the value field whenever a new target is opened.
    Effect::new(move |_| {
        if let Some(v) = target.get() {
            set_value.set(v.value);
        }
    });

    let close = move || {
        target.set(None);
        set_value.set(String::new());
    };

    let on_submit = move |ev: SubmitEvent| {
        ev.prevent_default();
        let Some(var) = target.get() else {
            return;
        };
        if submitting.get() {
            return;
        }
        let new_value = value.get();

        set_submitting.set(true);
        spawn_local(async move {
            let result = manager_update_variable(var.id.clone(), None, Some(new_value)).await;
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
                    {move || {
                        target
                            .get()
                            .map(|v| format!("Edit variable: {}", v.name))
                            .unwrap_or_default()
                    }}
                </h3>
                <form on:submit=on_submit class="space-y-3">
                    <div class="form-control">
                        <label class="label">
                            <span class="label-text">"Value"</span>
                        </label>
                        <textarea
                            class="textarea textarea-bordered w-full font-mono h-32"
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
fn DeleteVariableModal(
    target: RwSignal<Option<WireVariable>>,
    set_error: WriteSignal<Option<String>>,
    on_success: impl Fn() + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let (submitting, set_submitting) = signal(false);

    let close = move || target.set(None);

    let confirm = move |_| {
        let Some(var) = target.get() else {
            return;
        };
        set_submitting.set(true);
        spawn_local(async move {
            let result = manager_delete_variable(var.id.clone()).await;
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
                <h3 class="font-bold text-lg mb-3">"Delete variable"</h3>
                <p>
                    {move || {
                        target
                            .get()
                            .map(|v| format!("Delete variable '{}'? This cannot be undone.", v.name))
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
