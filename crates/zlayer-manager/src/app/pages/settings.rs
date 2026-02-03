//! Settings page
use crate::app::server_fns::{delete_secret, get_secrets, SecretInfo};
use leptos::prelude::*;

#[component]
fn GeneralSettingsCard() -> impl IntoView {
    view! {
        <div class="card bg-base-200 shadow-xl">
            <div class="card-body">
                <h2 class="card-title">"General Settings"</h2>
                <div class="form-control w-full max-w-md">
                    <label class="label"><span class="label-text">"Cluster Name"</span></label>
                    <input type="text" placeholder="my-cluster" value="zlayer-production" class="input input-bordered w-full" />
                </div>
                <div class="form-control w-full max-w-md">
                    <label class="label"><span class="label-text">"API URL"</span></label>
                    <input type="url" placeholder="https://api.example.com" value="https://api.zlayer.io" class="input input-bordered w-full" />
                </div>
                <div class="card-actions justify-end mt-4">
                    <button class="btn btn-primary">"Save Changes"</button>
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
                    {secrets.into_iter().map(|secret| {
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
                                    <button class="btn btn-ghost btn-xs text-error"
                                        on:click=move |_| on_delete(name_for_delete.clone())>
                                        "Delete"
                                    </button>
                                </td>
                            </tr>
                        }
                    }).collect::<Vec<_>>()}
                </tbody>
            </table>
        </div>
    }
}

#[component]
fn SecretsCard() -> impl IntoView {
    let (refresh, set_refresh) = signal(0u32);
    let secrets = Resource::new(move || refresh.get(), |_| get_secrets());

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

    view! {
        <div class="card bg-base-200 shadow-xl">
            <div class="card-body">
                <div class="flex justify-between items-center">
                    <h2 class="card-title">"Secrets"</h2>
                    <button class="btn btn-primary btn-sm">"Add Secret"</button>
                </div>
                <Suspense fallback=move || view! { <span class="loading loading-spinner"></span> }>
                    <ErrorBoundary fallback=|errors| view! { <div class="alert alert-error">{move || format!("{:?}", errors.get())}</div> }>
                        {move || secrets.get().map(|result| match result {
                            Ok(secret_list) => {
                                if secret_list.is_empty() {
                                    view! { <div class="alert alert-info">"No secrets found."</div> }.into_any()
                                } else {
                                    let on_delete = move |name: String| { delete_action.dispatch(name); };
                                    render_secrets_table(secret_list, on_delete).into_any()
                                }
                            }
                            Err(e) => view! { <div class="alert alert-error">{e.to_string()}</div> }.into_any(),
                        })}
                    </ErrorBoundary>
                </Suspense>
            </div>
        </div>
    }
}

#[component]
fn DangerZoneCard() -> impl IntoView {
    view! {
        <div class="card bg-error/10 border border-error shadow-xl">
            <div class="card-body">
                <h2 class="card-title text-error">"Danger Zone"</h2>
                <p class="text-base-content/70">"These actions are irreversible. Please proceed with caution."</p>
                <div class="card-actions justify-end mt-4">
                    <button class="btn btn-error btn-outline">"Reset Cluster"</button>
                </div>
            </div>
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
