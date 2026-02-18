//! Git repositories page
//!
//! There is no git backend API yet. This page shows a "Coming Soon"
//! state and offers the ability to store repository URLs as secrets
//! (using a `git-repo:` prefix) for future GitOps integration.

use leptos::prelude::*;

use crate::app::server_fns::{create_secret, delete_secret, get_secrets, SecretInfo};

/// Git repo prefix used in secrets
const GIT_REPO_PREFIX: &str = "git-repo:";

/// Render saved git repo secrets as a simple table
fn render_repos_table(
    repos: Vec<SecretInfo>,
    on_delete: impl Fn(String) + 'static + Clone,
) -> impl IntoView {
    view! {
        <div class="overflow-x-auto mt-6">
            <h3 class="text-lg font-semibold mb-3">"Saved Repository URLs"</h3>
            <table class="table table-zebra w-full">
                <thead>
                    <tr>
                        <th>"Name"</th>
                        <th>"Added"</th>
                        <th>"Actions"</th>
                    </tr>
                </thead>
                <tbody>
                    {repos
                        .into_iter()
                        .map(|repo| {
                            let display_name = repo
                                .name
                                .strip_prefix(GIT_REPO_PREFIX)
                                .unwrap_or(&repo.name)
                                .to_string();
                            let secret_name = repo.name.clone();
                            let on_delete = on_delete.clone();
                            view! {
                                <tr>
                                    <td class="font-mono">{display_name}</td>
                                    <td>{format_timestamp(repo.created_at)}</td>
                                    <td>
                                        <button
                                            class="btn btn-ghost btn-xs text-error"
                                            on:click=move |_| on_delete(secret_name.clone())
                                        >
                                            "Remove"
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

fn format_timestamp(ts: i64) -> String {
    let secs_per_day = 86400i64;
    let days_since_epoch = ts / secs_per_day;
    let years = 1970 + (days_since_epoch / 365);
    let day_of_year = days_since_epoch % 365;
    let month = (day_of_year / 30) + 1;
    let day = (day_of_year % 30) + 1;
    format!("{}-{:02}-{:02}", years, month.min(12), day.min(28))
}

/// Git repositories page component
///
/// Displays a Coming Soon message with the option to pre-register
/// repository URLs as secrets for future GitOps support.
#[component]
pub fn Git() -> impl IntoView {
    let (refresh, set_refresh) = signal(0u32);
    let secrets = Resource::new(move || refresh.get(), |_| get_secrets());

    // Add repo modal state
    let (show_add_modal, set_show_add_modal) = signal(false);
    let (repo_name, set_repo_name) = signal(String::new());
    let (repo_url, set_repo_url) = signal(String::new());
    let (error_msg, set_error_msg) = signal(Option::<String>::None);

    // Create repo secret action
    let create_action = Action::new(move |input: &(String, String)| {
        let (name, url) = input.clone();
        let secret_name = format!("{}{}", GIT_REPO_PREFIX, name);
        async move {
            let result = create_secret(secret_name, url).await;
            match result {
                Ok(_) => {
                    set_show_add_modal.set(false);
                    set_repo_name.set(String::new());
                    set_repo_url.set(String::new());
                    set_error_msg.set(None);
                    set_refresh.update(|n| *n += 1);
                }
                Err(e) => {
                    set_error_msg.set(Some(e.to_string()));
                }
            }
        }
    });

    // Delete repo secret action
    let delete_action = Action::new(move |name: &String| {
        let name = name.clone();
        async move {
            let result = delete_secret(name).await;
            if result.is_ok() {
                set_refresh.update(|n| *n += 1);
            }
        }
    });

    view! {
        <div class="p-6">
            <div class="flex justify-between items-center mb-6">
                <h1 class="text-3xl font-bold">"Git Repositories"</h1>
                <button
                    class="btn btn-primary"
                    on:click=move |_| {
                        set_error_msg.set(None);
                        set_repo_name.set(String::new());
                        set_repo_url.set(String::new());
                        set_show_add_modal.set(true);
                    }
                >
                    "Add Repository"
                </button>
            </div>

            <div class="card bg-base-200 shadow-xl">
                <div class="card-body items-center text-center">
                    <h2 class="card-title text-2xl mb-2">"Git Integration Coming Soon"</h2>
                    <p class="text-base-content/70 max-w-lg">
                        "Configure repository connections for GitOps deployments. "
                        "In the meantime, you can register repository URLs below "
                        "to prepare for automatic deployments when this feature launches."
                    </p>
                </div>
            </div>

            // Show saved git repos from secrets
            <Suspense fallback=move || {
                view! {
                    <div class="flex justify-center p-4">
                        <span class="loading loading-spinner"></span>
                    </div>
                }
            }>
                <ErrorBoundary fallback=|errors| {
                    view! {
                        <div class="alert alert-error mt-4">
                            <span>{move || format!("Error: {:?}", errors.get())}</span>
                        </div>
                    }
                }>
                    {move || {
                        secrets
                            .get()
                            .map(|result| {
                                match result {
                                    Ok(all_secrets) => {
                                        let git_repos: Vec<SecretInfo> = all_secrets
                                            .into_iter()
                                            .filter(|s| s.name.starts_with(GIT_REPO_PREFIX))
                                            .collect();
                                        if git_repos.is_empty() {
                                            view! {
                                                <div class="alert alert-info mt-6">
                                                    <span>
                                                        "No repositories registered yet. Click \"Add Repository\" to get started."
                                                    </span>
                                                </div>
                                            }
                                                .into_any()
                                        } else {
                                            let on_delete = move |name: String| {
                                                delete_action.dispatch(name);
                                            };
                                            render_repos_table(git_repos, on_delete).into_any()
                                        }
                                    }
                                    Err(e) => {
                                        view! {
                                            <div class="alert alert-error mt-4">
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

            // Add Repository Modal
            <div class=move || {
                if show_add_modal.get() { "modal modal-open" } else { "modal" }
            }>
                <div class="modal-box">
                    <h3 class="font-bold text-lg">"Add Repository"</h3>
                    <p class="text-base-content/70 text-sm mt-1">
                        "Register a git repository URL for future GitOps integration."
                    </p>

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
                            <span class="label-text">"Repository Name" <span class="text-error">"*"</span></span>
                        </label>
                        <input
                            type="text"
                            class="input input-bordered w-full"
                            placeholder="e.g. frontend-app"
                            prop:value=move || repo_name.get()
                            on:input=move |ev| set_repo_name.set(event_target_value(&ev))
                        />
                    </div>
                    <div class="form-control w-full mt-2">
                        <label class="label">
                            <span class="label-text">"Repository URL" <span class="text-error">"*"</span></span>
                        </label>
                        <input
                            type="url"
                            class="input input-bordered w-full"
                            placeholder="https://github.com/org/repo.git"
                            prop:value=move || repo_url.get()
                            on:input=move |ev| set_repo_url.set(event_target_value(&ev))
                        />
                    </div>

                    <div class="modal-action">
                        <button class="btn" on:click=move |_| set_show_add_modal.set(false)>
                            "Cancel"
                        </button>
                        <button
                            class="btn btn-primary"
                            prop:disabled=move || {
                                repo_name.get().trim().is_empty()
                                    || repo_url.get().trim().is_empty()
                            }
                            on:click=move |_| {
                                create_action
                                    .dispatch((repo_name.get(), repo_url.get()));
                            }
                        >
                            "Add"
                        </button>
                    </div>
                </div>
                <div class="modal-backdrop" on:click=move |_| set_show_add_modal.set(false)></div>
            </div>
        </div>
    }
}
