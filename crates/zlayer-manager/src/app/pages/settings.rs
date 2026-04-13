//! Settings page
//!
//! Manages instances (remote ZLayer API connections), secrets,
//! and danger-zone operations (cluster reset).

use crate::app::server_fns::{
    add_new_instance, create_secret, delete_secret, get_instances, get_secrets,
    remove_existing_instance, reset_cluster, switch_instance, test_instance_connection,
    update_existing_instance, SecretInfo,
};
use leptos::prelude::*;

#[component]
fn InstancesCard() -> impl IntoView {
    let (refresh, set_refresh) = signal(0u32);
    let instances = Resource::new(move || refresh.get(), |_| get_instances());

    // Modal state
    let (show_modal, set_show_modal) = signal(false);
    let (editing_index, set_editing_index) = signal(Option::<usize>::None);
    let (inst_name, set_inst_name) = signal(String::new());
    let (inst_url, set_inst_url) = signal(String::new());
    let (inst_token, set_inst_token) = signal(String::new());
    let (modal_error, set_modal_error) = signal(Option::<String>::None);

    // Test connection state
    let (test_result, set_test_result) = signal(Option::<Result<String, String>>::None);
    let (testing, set_testing) = signal(false);

    // Delete confirmation state
    let (confirm_delete_index, set_confirm_delete_index) = signal(Option::<usize>::None);

    let open_add_modal = move |_| {
        set_editing_index.set(None);
        set_inst_name.set(String::new());
        set_inst_url.set(String::new());
        set_inst_token.set(String::new());
        set_modal_error.set(None);
        set_test_result.set(None);
        set_show_modal.set(true);
    };

    let close_modal = move |_: leptos::ev::MouseEvent| {
        set_show_modal.set(false);
        set_modal_error.set(None);
        set_test_result.set(None);
    };

    let test_action = Action::new(move |input: &(String, Option<String>)| {
        let (url, token) = input.clone();
        async move {
            set_testing.set(true);
            set_test_result.set(None);
            let result = test_instance_connection(url, token).await;
            match result {
                Ok(msg) => set_test_result.set(Some(Ok(msg))),
                Err(e) => set_test_result.set(Some(Err(e.to_string()))),
            }
            set_testing.set(false);
        }
    });

    let save_action = Action::new(
        move |input: &(Option<usize>, String, String, Option<String>)| {
            let (index, name, url, token) = input.clone();
            async move {
                let result = match index {
                    Some(idx) => update_existing_instance(idx, name, url, token).await,
                    None => add_new_instance(name, url, token).await,
                };
                match result {
                    Ok(()) => {
                        set_show_modal.set(false);
                        set_modal_error.set(None);
                        set_test_result.set(None);
                        set_refresh.update(|n| *n += 1);
                    }
                    Err(e) => {
                        set_modal_error.set(Some(e.to_string()));
                    }
                }
            }
        },
    );

    let switch_action = Action::new(move |index: &usize| {
        let index = *index;
        async move {
            let _ = switch_instance(index).await;
            set_refresh.update(|n| *n += 1);
        }
    });

    let delete_action = Action::new(move |index: &usize| {
        let index = *index;
        async move {
            let _ = remove_existing_instance(index).await;
            set_confirm_delete_index.set(None);
            set_refresh.update(|n| *n += 1);
        }
    });

    view! {
        <div class="card bg-base-200 shadow-xl">
            <div class="card-body">
                <div class="flex justify-between items-center">
                    <h2 class="card-title">"Instances"</h2>
                    <button class="btn btn-primary btn-sm" on:click=open_add_modal>
                        "Add Instance"
                    </button>
                </div>
                <p class="text-base-content/70 text-sm">
                    "Manage connections to ZLayer API instances."
                </p>

                <Suspense fallback=move || {
                    view! { <span class="loading loading-spinner"></span> }
                }>
                    <ErrorBoundary fallback=|errors| {
                        view! {
                            <div class="alert alert-error">
                                {move || format!("{:?}", errors.get())}
                            </div>
                        }
                    }>
                        {move || {
                            instances
                                .get()
                                .map(|result| {
                                    match result {
                                        Ok(instance_list) => {
                                            if instance_list.is_empty() {
                                                view! {
                                                    <div class="alert alert-info">
                                                        "No instances configured. Add one to get started."
                                                    </div>
                                                }
                                                    .into_any()
                                            } else {
                                                view! {
                                                    <div class="overflow-x-auto">
                                                        <table class="table table-zebra w-full">
                                                            <thead>
                                                                <tr>
                                                                    <th>"Status"</th>
                                                                    <th>"Name"</th>
                                                                    <th>"URL"</th>
                                                                    <th>"Token"</th>
                                                                    <th>"Actions"</th>
                                                                </tr>
                                                            </thead>
                                                            <tbody>
                                                                {instance_list
                                                                    .into_iter()
                                                                    .enumerate()
                                                                    .map(|(i, inst)| {
                                                                        let name = inst.name.clone();
                                                                        let url = inst.url.clone();
                                                                        let is_active = inst.is_active;
                                                                        let has_token = inst.has_token;
                                                                        let edit_name = name.clone();
                                                                        let edit_url = url.clone();
                                                                        view! {
                                                                            <tr class=if is_active {
                                                                                "bg-success/10"
                                                                            } else {
                                                                                ""
                                                                            }>
                                                                                <td>
                                                                                    {if is_active {
                                                                                        view! {
                                                                                            <span class="badge badge-success badge-sm">"Active"</span>
                                                                                        }
                                                                                            .into_any()
                                                                                    } else {
                                                                                        view! {
                                                                                            <span class="badge badge-ghost badge-sm">"Inactive"</span>
                                                                                        }
                                                                                            .into_any()
                                                                                    }}
                                                                                </td>
                                                                                <td class="font-semibold">{name}</td>
                                                                                <td class="font-mono text-sm">{url}</td>
                                                                                <td>
                                                                                    {if has_token {
                                                                                        view! {
                                                                                            <span class="badge badge-info badge-sm">
                                                                                                "Set"
                                                                                            </span>
                                                                                        }
                                                                                            .into_any()
                                                                                    } else {
                                                                                        view! {
                                                                                            <span class="badge badge-ghost badge-sm">
                                                                                                "None"
                                                                                            </span>
                                                                                        }
                                                                                            .into_any()
                                                                                    }}
                                                                                </td>
                                                                                <td>
                                                                                    <div class="flex gap-1">
                                                                                        {if is_active {
                                                                                            None
                                                                                        } else {
                                                                                            Some(view! {
                                                                                                <button
                                                                                                    class="btn btn-success btn-xs"
                                                                                                    on:click=move |_| {
                                                                                                        switch_action.dispatch(i);
                                                                                                    }
                                                                                                >
                                                                                                    "Connect"
                                                                                                </button>
                                                                                            })
                                                                                        }}
                                                                                        <button
                                                                                            class="btn btn-ghost btn-xs"
                                                                                            on:click=move |_| {
                                                                                                set_editing_index.set(Some(i));
                                                                                                set_inst_name.set(edit_name.clone());
                                                                                                set_inst_url.set(edit_url.clone());
                                                                                                set_inst_token.set(String::new());
                                                                                                set_modal_error.set(None);
                                                                                                set_test_result.set(None);
                                                                                                set_show_modal.set(true);
                                                                                            }
                                                                                        >
                                                                                            "Edit"
                                                                                        </button>
                                                                                        <button
                                                                                            class="btn btn-ghost btn-xs text-error"
                                                                                            on:click=move |_| {
                                                                                                set_confirm_delete_index.set(Some(i));
                                                                                            }
                                                                                        >
                                                                                            "Delete"
                                                                                        </button>
                                                                                    </div>
                                                                                </td>
                                                                            </tr>
                                                                        }
                                                                    })
                                                                    .collect::<Vec<_>>()}
                                                            </tbody>
                                                        </table>
                                                    </div>
                                                }
                                                    .into_any()
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
        </div>

        // Add/Edit Instance Modal
        <div class=move || {
            if show_modal.get() { "modal modal-open" } else { "modal" }
        }>
            <div class="modal-box">
                <h3 class="font-bold text-lg">
                    {move || {
                        if editing_index.get().is_some() {
                            "Edit Instance"
                        } else {
                            "Add Instance"
                        }
                    }}
                </h3>

                {move || {
                    modal_error
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
                        <span class="label-text">"Name"</span>
                    </label>
                    <input
                        type="text"
                        class="input input-bordered w-full"
                        placeholder="e.g. Production"
                        prop:value=move || inst_name.get()
                        on:input=move |ev| set_inst_name.set(event_target_value(&ev))
                    />
                </div>
                <div class="form-control w-full mt-2">
                    <label class="label">
                        <span class="label-text">"URL"</span>
                    </label>
                    <input
                        type="url"
                        class="input input-bordered w-full"
                        placeholder="https://api.example.com:3669"
                        prop:value=move || inst_url.get()
                        on:input=move |ev| set_inst_url.set(event_target_value(&ev))
                    />
                </div>
                <div class="form-control w-full mt-2">
                    <label class="label">
                        <span class="label-text">"Token (optional)"</span>
                    </label>
                    <input
                        type="password"
                        class="input input-bordered w-full"
                        placeholder="Leave blank to keep existing token"
                        prop:value=move || inst_token.get()
                        on:input=move |ev| set_inst_token.set(event_target_value(&ev))
                    />
                </div>

                // Test connection result
                {move || {
                    test_result
                        .get()
                        .map(|result| match result {
                            Ok(msg) => {
                                view! {
                                    <div class="alert alert-success mt-4">
                                        <span>{msg}</span>
                                    </div>
                                }
                                    .into_any()
                            }
                            Err(msg) => {
                                view! {
                                    <div class="alert alert-error mt-4">
                                        <span>{msg}</span>
                                    </div>
                                }
                                    .into_any()
                            }
                        })
                }}

                <div class="modal-action">
                    <button
                        class="btn btn-outline btn-sm"
                        prop:disabled=move || inst_url.get().trim().is_empty() || testing.get()
                        on:click=move |_| {
                            let token = if inst_token.get().trim().is_empty() {
                                None
                            } else {
                                Some(inst_token.get())
                            };
                            test_action.dispatch((inst_url.get(), token));
                        }
                    >
                        {move || if testing.get() { "Testing..." } else { "Test Connection" }}
                    </button>
                    <button class="btn" on:click=close_modal>
                        "Cancel"
                    </button>
                    <button
                        class="btn btn-primary"
                        prop:disabled=move || {
                            inst_name.get().trim().is_empty()
                                || inst_url.get().trim().is_empty()
                        }
                        on:click=move |_| {
                            let token = if inst_token.get().trim().is_empty() {
                                None
                            } else {
                                Some(inst_token.get())
                            };
                            save_action
                                .dispatch((
                                    editing_index.get(),
                                    inst_name.get(),
                                    inst_url.get(),
                                    token,
                                ));
                        }
                    >
                        "Save"
                    </button>
                </div>
            </div>
            <div class="modal-backdrop" on:click=close_modal></div>
        </div>

        // Delete Confirmation Modal
        <div class=move || {
            if confirm_delete_index.get().is_some() {
                "modal modal-open"
            } else {
                "modal"
            }
        }>
            <div class="modal-box">
                <h3 class="font-bold text-lg text-error">"Delete Instance"</h3>
                <p class="py-4">
                    "Are you sure you want to remove this instance? This action cannot be undone."
                </p>
                <div class="modal-action">
                    <button
                        class="btn"
                        on:click=move |_| set_confirm_delete_index.set(None)
                    >
                        "Cancel"
                    </button>
                    <button
                        class="btn btn-error"
                        on:click=move |_| {
                            if let Some(idx) = confirm_delete_index.get() {
                                delete_action.dispatch(idx);
                            }
                        }
                    >
                        "Delete"
                    </button>
                </div>
            </div>
            <div
                class="modal-backdrop"
                on:click=move |_| set_confirm_delete_index.set(None)
            ></div>
        </div>
    }
}

fn format_timestamp(ts: i64) -> String {
    let secs_per_day = 86400i64;
    let days_since_epoch = ts / secs_per_day;
    let years = 1970 + (days_since_epoch / 365);
    let day_of_year = days_since_epoch % 365;
    let month = (day_of_year / 30) + 1;
    let day = (day_of_year % 30) + 1;
    format!("{}-{:02}-{:02}", years, month.min(12), day.min(28))
}

fn render_secrets_table(
    secrets: Vec<SecretInfo>,
    on_delete: impl Fn(String) + 'static + Clone,
) -> impl IntoView {
    view! {
        <div class="overflow-x-auto">
            <table class="table table-zebra w-full">
                <thead>
                    <tr>
                        <th>"Name"</th>
                        <th>"Created"</th>
                        <th>"Updated"</th>
                        <th>"Version"</th>
                        <th>"Actions"</th>
                    </tr>
                </thead>
                <tbody>
                    {secrets
                        .into_iter()
                        .map(|secret| {
                            let name = secret.name.clone();
                            let name_for_delete = name.clone();
                            let on_delete = on_delete.clone();
                            view! {
                                <tr>
                                    <td class="font-mono">{name}</td>
                                    <td>{format_timestamp(secret.created_at)}</td>
                                    <td>{format_timestamp(secret.updated_at)}</td>
                                    <td>{secret.version}</td>
                                    <td>
                                        <button
                                            class="btn btn-ghost btn-xs text-error"
                                            on:click=move |_| on_delete(name_for_delete.clone())
                                        >
                                            "Delete"
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

#[component]
fn SecretsCard() -> impl IntoView {
    let (refresh, set_refresh) = signal(0u32);
    let secrets = Resource::new(move || refresh.get(), |_| get_secrets());

    // Add Secret modal state
    let (show_add_modal, set_show_add_modal) = signal(false);
    let (secret_name, set_secret_name) = signal(String::new());
    let (secret_value, set_secret_value) = signal(String::new());
    let (error_msg, set_error_msg) = signal(Option::<String>::None);

    let delete_action = Action::new(move |name: &String| {
        let name = name.clone();
        async move {
            let result = delete_secret(name).await;
            if result.is_ok() {
                set_refresh.update(|n| *n += 1);
            }
            result
        }
    });

    let create_action = Action::new(move |input: &(String, String)| {
        let (name, value) = input.clone();
        async move {
            let result = create_secret(name, value).await;
            match result {
                Ok(_) => {
                    set_show_add_modal.set(false);
                    set_secret_name.set(String::new());
                    set_secret_value.set(String::new());
                    set_error_msg.set(None);
                    set_refresh.update(|n| *n += 1);
                }
                Err(e) => {
                    set_error_msg.set(Some(e.to_string()));
                }
            }
        }
    });

    view! {
        <div class="card bg-base-200 shadow-xl">
            <div class="card-body">
                <div class="flex justify-between items-center">
                    <h2 class="card-title">"Secrets"</h2>
                    <button
                        class="btn btn-primary btn-sm"
                        on:click=move |_| {
                            set_error_msg.set(None);
                            set_secret_name.set(String::new());
                            set_secret_value.set(String::new());
                            set_show_add_modal.set(true);
                        }
                    >
                        "Add Secret"
                    </button>
                </div>
                <Suspense fallback=move || {
                    view! { <span class="loading loading-spinner"></span> }
                }>
                    <ErrorBoundary fallback=|errors| {
                        view! {
                            <div class="alert alert-error">
                                {move || format!("{:?}", errors.get())}
                            </div>
                        }
                    }>
                        {move || {
                            secrets
                                .get()
                                .map(|result| {
                                    match result {
                                        Ok(secret_list) => {
                                            if secret_list.is_empty() {
                                                view! {
                                                    <div class="alert alert-info">"No secrets found."</div>
                                                }
                                                    .into_any()
                                            } else {
                                                let on_delete = move |name: String| {
                                                    delete_action.dispatch(name);
                                                };
                                                render_secrets_table(secret_list, on_delete).into_any()
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
        </div>

        // Add Secret Modal
        <div class=move || {
            if show_add_modal.get() { "modal modal-open" } else { "modal" }
        }>
            <div class="modal-box">
                <h3 class="font-bold text-lg">"Add Secret"</h3>

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
                        <span class="label-text">"Secret Name"</span>
                    </label>
                    <input
                        type="text"
                        class="input input-bordered w-full"
                        placeholder="e.g. DATABASE_URL"
                        prop:value=move || secret_name.get()
                        on:input=move |ev| set_secret_name.set(event_target_value(&ev))
                    />
                </div>
                <div class="form-control w-full mt-2">
                    <label class="label">
                        <span class="label-text">"Secret Value"</span>
                    </label>
                    <input
                        type="password"
                        class="input input-bordered w-full"
                        placeholder="Enter secret value"
                        prop:value=move || secret_value.get()
                        on:input=move |ev| set_secret_value.set(event_target_value(&ev))
                    />
                </div>

                <div class="modal-action">
                    <button class="btn" on:click=move |_| set_show_add_modal.set(false)>
                        "Cancel"
                    </button>
                    <button
                        class="btn btn-primary"
                        prop:disabled=move || {
                            secret_name.get().trim().is_empty()
                                || secret_value.get().trim().is_empty()
                        }
                        on:click=move |_| {
                            create_action
                                .dispatch((secret_name.get(), secret_value.get()));
                        }
                    >
                        "Create"
                    </button>
                </div>
            </div>
            <div class="modal-backdrop" on:click=move |_| set_show_add_modal.set(false)></div>
        </div>
    }
}

#[component]
fn DangerZoneCard() -> impl IntoView {
    let (show_reset_modal, set_show_reset_modal) = signal(false);
    let (confirm_input, set_confirm_input) = signal(String::new());
    let (reset_result, set_reset_result) = signal(Option::<Result<String, String>>::None);
    let (resetting, set_resetting) = signal(false);

    let reset_action = Action::new(move |(): &()| async move {
        set_resetting.set(true);
        let result = reset_cluster().await;
        match result {
            Ok(msg) => {
                set_reset_result.set(Some(Ok(msg)));
                set_show_reset_modal.set(false);
                set_confirm_input.set(String::new());
            }
            Err(e) => {
                set_reset_result.set(Some(Err(e.to_string())));
                set_show_reset_modal.set(false);
                set_confirm_input.set(String::new());
            }
        }
        set_resetting.set(false);
    });

    view! {
        <div class="card bg-error/10 border border-error shadow-xl">
            <div class="card-body">
                <h2 class="card-title text-error">"Danger Zone"</h2>
                <p class="text-base-content/70">
                    "These actions are irreversible. Please proceed with caution."
                </p>

                {move || {
                    reset_result
                        .get()
                        .map(|result| match result {
                            Ok(msg) => {
                                view! {
                                    <div class="alert alert-success mt-2">
                                        <span>{msg}</span>
                                    </div>
                                }
                                    .into_any()
                            }
                            Err(msg) => {
                                view! {
                                    <div class="alert alert-error mt-2">
                                        <span>{msg}</span>
                                    </div>
                                }
                                    .into_any()
                            }
                        })
                }}

                <div class="card-actions justify-end mt-4">
                    <button
                        class="btn btn-error btn-outline"
                        on:click=move |_| {
                            set_confirm_input.set(String::new());
                            set_show_reset_modal.set(true);
                        }
                    >
                        "Reset Cluster"
                    </button>
                </div>
            </div>
        </div>

        // Reset Cluster Confirmation Modal
        <div class=move || {
            if show_reset_modal.get() { "modal modal-open" } else { "modal" }
        }>
            <div class="modal-box">
                <h3 class="font-bold text-lg text-error">"Reset Cluster"</h3>
                <div class="py-4">
                    <div class="alert alert-warning mb-4">
                        <span>
                            "This will delete ALL deployments, secrets, and reset the daemon. This action cannot be undone."
                        </span>
                    </div>
                    <p class="text-base-content/70 mb-2">
                        "Type " <span class="font-bold font-mono">"RESET"</span>
                        " below to confirm."
                    </p>
                    <input
                        type="text"
                        class="input input-bordered input-error w-full"
                        placeholder="Type RESET to confirm"
                        prop:value=move || confirm_input.get()
                        on:input=move |ev| set_confirm_input.set(event_target_value(&ev))
                    />
                </div>
                <div class="modal-action">
                    <button
                        class="btn"
                        on:click=move |_| {
                            set_show_reset_modal.set(false);
                            set_confirm_input.set(String::new());
                        }
                    >
                        "Cancel"
                    </button>
                    <button
                        class="btn btn-error"
                        prop:disabled=move || confirm_input.get() != "RESET" || resetting.get()
                        on:click=move |_| {
                            reset_action.dispatch(());
                        }
                    >
                        {move || if resetting.get() { "Resetting..." } else { "Reset Cluster" }}
                    </button>
                </div>
            </div>
            <div
                class="modal-backdrop"
                on:click=move |_| {
                    set_show_reset_modal.set(false);
                    set_confirm_input.set(String::new());
                }
            ></div>
        </div>
    }
}

#[component]
pub fn Settings() -> impl IntoView {
    view! {
        <div class="container mx-auto p-6 space-y-6">
            <h1 class="text-3xl font-bold mb-6">"Settings"</h1>
            <InstancesCard />
            <SecretsCard />
            <DangerZoneCard />
        </div>
    }
}
