//! Settings page
//!
//! Manages general settings (saved to localStorage) and secrets
//! (saved via the ZLayer API).

use crate::app::server_fns::{
    create_secret, delete_secret, get_secrets, reset_cluster, SecretInfo,
};
use leptos::prelude::*;

#[component]
fn GeneralSettingsCard() -> impl IntoView {
    let (cluster_name, set_cluster_name) = signal("zlayer-production".to_string());
    let (api_url, set_api_url) = signal("https://api.zlayer.io".to_string());
    let (saved, set_saved) = signal(false);

    view! {
        <div class="card bg-base-200 shadow-xl">
            <div class="card-body">
                <h2 class="card-title">"General Settings"</h2>
                <div class="form-control w-full max-w-md">
                    <label class="label">
                        <span class="label-text">"Cluster Name"</span>
                    </label>
                    <input
                        type="text"
                        placeholder="my-cluster"
                        class="input input-bordered w-full"
                        prop:value=move || cluster_name.get()
                        on:input=move |ev| {
                            set_cluster_name.set(event_target_value(&ev));
                            set_saved.set(false);
                        }
                    />
                </div>
                <div class="form-control w-full max-w-md">
                    <label class="label">
                        <span class="label-text">"API URL"</span>
                    </label>
                    <input
                        type="url"
                        placeholder="https://api.example.com"
                        class="input input-bordered w-full"
                        prop:value=move || api_url.get()
                        on:input=move |ev| {
                            set_api_url.set(event_target_value(&ev));
                            set_saved.set(false);
                        }
                    />
                </div>
                <div class="card-actions justify-end mt-4 items-center">
                    {move || {
                        saved
                            .get()
                            .then(|| {
                                view! {
                                    <span class="text-success text-sm">"Settings saved"</span>
                                }
                            })
                    }}
                    <button
                        class="btn btn-primary"
                        on:click=move |_| {
                            #[cfg(target_arch = "wasm32")]
                            {
                                if let Some(storage) = web_sys::window()
                                    .and_then(|w| w.local_storage().ok().flatten())
                                {
                                    let _ = storage
                                        .set_item("zlayer_cluster_name", &cluster_name.get());
                                    let _ = storage.set_item("zlayer_api_url", &api_url.get());
                                }
                            }
                            set_saved.set(true);
                        }
                    >
                        "Save Changes"
                    </button>
                </div>
            </div>
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
            <GeneralSettingsCard />
            <SecretsCard />
            <DangerZoneCard />
        </div>
    }
}
