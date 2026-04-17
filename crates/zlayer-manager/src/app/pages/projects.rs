//! `/projects` — list of projects with create + navigate-to-detail.
//!
//! This is the landing page. Detail view (source, build, credentials,
//! environments & deployments) lives in `project_detail.rs` behind the
//! nested route `/projects/:id`.
//!
//! All authenticated users can browse the list; only admins can mutate
//! (the daemon enforces admin-only on every mutation). Row clicks
//! anywhere except the Delete button navigate to the detail view.

use leptos::ev::SubmitEvent;
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::use_navigate;
use leptos_router::NavigateOptions;

use crate::app::components::forms::ConfirmDeleteModal;
use crate::app::server_fns::{
    manager_create_project, manager_delete_project, manager_list_projects,
};
use crate::app::util::errors::format_server_error;
use crate::wire::projects::{WireBuildKind, WireProject, WireProjectSpec};

/// Projects landing page.
#[component]
#[allow(clippy::too_many_lines)] // view! DSL + inline modal
pub fn Projects() -> impl IntoView {
    let list = Resource::new(|| (), |()| async move { manager_list_projects().await });
    let (error, set_error) = signal(None::<String>);
    let (new_open, set_new_open) = signal(false);
    let delete_target = RwSignal::new(None::<WireProject>);

    let refetch = move || list.refetch();

    view! {
        <div class="p-6 space-y-4">
            <div class="flex justify-between items-center">
                <h1 class="text-2xl font-bold">"Projects"</h1>
                <button
                    class="btn btn-primary btn-sm"
                    on:click=move |_| set_new_open.set(true)
                >
                    "New project"
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
                            list.get()
                                .map(|res| match res {
                                    Err(e) => view! {
                                        <div class="p-4 text-error">
                                            {format!(
                                                "Failed to load projects: {}",
                                                format_server_error(&e),
                                            )}
                                        </div>
                                    }
                                        .into_any(),
                                    Ok(items) => render_table(items, delete_target).into_any(),
                                })
                        }}
                    </Suspense>
                </div>
            </div>

            <NewProjectModal
                open=new_open
                set_open=set_new_open
                set_error=set_error
                on_success=move || {
                    refetch();
                }
            />

            <DeleteProjectView
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
    list: Vec<WireProject>,
    delete_target: RwSignal<Option<WireProject>>,
) -> impl IntoView {
    if list.is_empty() {
        return view! { <div class="p-4 opacity-70">"(no projects)"</div> }.into_any();
    }

    view! {
        <table class="table table-zebra">
            <thead>
                <tr>
                    <th>"Name"</th>
                    <th>"Git URL"</th>
                    <th>"Branch"</th>
                    <th>"Auto-deploy"</th>
                    <th>"Updated"</th>
                    <th class="text-right">"Actions"</th>
                </tr>
            </thead>
            <tbody>
                {list.into_iter().map(|p| render_row(&p, delete_target)).collect_view()}
            </tbody>
        </table>
    }
    .into_any()
}

fn render_row(p: &WireProject, delete_target: RwSignal<Option<WireProject>>) -> impl IntoView {
    let id_for_click = p.id.clone();
    let id_for_open = p.id.clone();
    let delete_proj = p.clone();
    let branch = p.git_branch.clone().unwrap_or_else(|| "main".to_string());
    let git_url = p.git_url.clone().unwrap_or_default();
    let git_url_for_tooltip = git_url.clone();
    let auto_deploy_badge = if p.auto_deploy {
        view! { <span class="badge badge-success badge-sm">"on"</span> }.into_any()
    } else {
        view! { <span class="badge badge-ghost badge-sm">"off"</span> }.into_any()
    };

    view! {
        <tr class="hover cursor-pointer" on:click={
            let id = id_for_click.clone();
            move |_| {
                let navigate = use_navigate();
                navigate(&format!("/projects/{id}"), NavigateOptions::default());
            }
        }>
            <td class="font-medium">{p.name.clone()}</td>
            <td class="font-mono text-xs max-w-xs">
                <div class="truncate" title=git_url_for_tooltip>{git_url}</div>
            </td>
            <td>
                <span class="badge badge-outline">{branch}</span>
            </td>
            <td>{auto_deploy_badge}</td>
            <td class="text-xs opacity-70">{p.updated_at.clone()}</td>
            <td class="text-right space-x-1" on:click=|ev| ev.stop_propagation()>
                <button
                    class="btn btn-xs btn-ghost"
                    on:click=move |_| {
                        let navigate = use_navigate();
                        navigate(
                            &format!("/projects/{id_for_open}"),
                            NavigateOptions::default(),
                        );
                    }
                >
                    "Detail"
                </button>
                <button
                    class="btn btn-xs btn-error"
                    on:click=move |_| delete_target.set(Some(delete_proj.clone()))
                >
                    "Delete"
                </button>
            </td>
        </tr>
    }
}

#[component]
fn NewProjectModal(
    open: ReadSignal<bool>,
    set_open: WriteSignal<bool>,
    set_error: WriteSignal<Option<String>>,
    on_success: impl Fn() + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let (name, set_name) = signal(String::new());
    let (git_url, set_git_url) = signal(String::new());
    let (git_branch, set_git_branch) = signal("main".to_string());
    let (build_kind, set_build_kind) = signal(String::new());
    let (build_path, set_build_path) = signal(String::new());
    let (auto_deploy, set_auto_deploy) = signal(false);
    let (poll_secs, set_poll_secs) = signal(0_u64);
    let (submitting, set_submitting) = signal(false);

    let reset = move || {
        set_name.set(String::new());
        set_git_url.set(String::new());
        set_git_branch.set("main".to_string());
        set_build_kind.set(String::new());
        set_build_path.set(String::new());
        set_auto_deploy.set(false);
        set_poll_secs.set(0);
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

        let build_kind_val = build_kind.get();
        let build_kind_parsed = if build_kind_val.is_empty() {
            None
        } else {
            WireBuildKind::parse_wire(&build_kind_val)
        };

        let git_url_val = git_url.get().trim().to_string();
        let git_branch_val = git_branch.get().trim().to_string();
        let build_path_val = build_path.get().trim().to_string();
        let poll_val = poll_secs.get();
        let auto_deploy_val = auto_deploy.get();

        let spec = WireProjectSpec {
            name: Some(name_val),
            description: None,
            git_url: (!git_url_val.is_empty()).then_some(git_url_val),
            git_branch: (!git_branch_val.is_empty()).then_some(git_branch_val),
            git_credential_id: None,
            build_kind: build_kind_parsed,
            build_path: (!build_path_val.is_empty()).then_some(build_path_val),
            deploy_spec_path: None,
            registry_credential_id: None,
            default_environment_id: None,
            auto_deploy: Some(auto_deploy_val),
            poll_interval_secs: if poll_val == 0 { None } else { Some(poll_val) },
        };

        set_submitting.set(true);
        spawn_local(async move {
            let result = manager_create_project(spec).await;
            set_submitting.set(false);
            match result {
                Ok(_) => {
                    reset();
                    set_open.set(false);
                    on_success();
                }
                Err(e) => {
                    set_error.set(Some(format!("Create failed: {}", format_server_error(&e))));
                }
            }
        });
    };

    view! {
        <dialog class=move || if open.get() { "modal modal-open" } else { "modal" }>
            <div class="modal-box max-w-2xl">
                <h3 class="font-bold text-lg mb-3">"New project"</h3>
                <form on:submit=on_submit class="space-y-3">
                    <div class="form-control">
                        <label class="label">
                            <span class="label-text">"Name"</span>
                        </label>
                        <input
                            type="text"
                            class="input input-bordered w-full"
                            required
                            placeholder="my-app"
                            prop:value=name
                            on:input=move |ev| set_name.set(event_target_value(&ev))
                        />
                    </div>
                    <div class="form-control">
                        <label class="label">
                            <span class="label-text">"Git URL "<span class="opacity-60">"(optional)"</span></span>
                        </label>
                        <input
                            type="text"
                            class="input input-bordered w-full font-mono text-sm"
                            placeholder="https://github.com/user/repo"
                            prop:value=git_url
                            on:input=move |ev| set_git_url.set(event_target_value(&ev))
                        />
                    </div>
                    <div class="grid grid-cols-2 gap-3">
                        <div class="form-control">
                            <label class="label">
                                <span class="label-text">"Git branch"</span>
                            </label>
                            <input
                                type="text"
                                class="input input-bordered w-full"
                                prop:value=git_branch
                                on:input=move |ev| set_git_branch.set(event_target_value(&ev))
                            />
                        </div>
                        <div class="form-control">
                            <label class="label">
                                <span class="label-text">"Build kind"</span>
                            </label>
                            <select
                                class="select select-bordered w-full"
                                prop:value=build_kind
                                on:change=move |ev| set_build_kind.set(event_target_value(&ev))
                            >
                                <option value="">"(none)"</option>
                                <option value="dockerfile">"dockerfile"</option>
                                <option value="compose">"compose"</option>
                                <option value="zimagefile">"zimagefile"</option>
                                <option value="spec">"spec"</option>
                            </select>
                        </div>
                    </div>
                    <div class="form-control">
                        <label class="label">
                            <span class="label-text">"Build path "<span class="opacity-60">"(optional)"</span></span>
                            <span class="label-text-alt opacity-70">"Relative path in repo"</span>
                        </label>
                        <input
                            type="text"
                            class="input input-bordered w-full font-mono text-sm"
                            placeholder="./Dockerfile"
                            prop:value=build_path
                            on:input=move |ev| set_build_path.set(event_target_value(&ev))
                        />
                    </div>
                    <div class="form-control">
                        <label class="label cursor-pointer">
                            <span class="label-text">"Auto-deploy on new commits"</span>
                            <input
                                type="checkbox"
                                class="toggle toggle-primary"
                                prop:checked=auto_deploy
                                on:change=move |ev| set_auto_deploy.set(event_target_checked(&ev))
                            />
                        </label>
                    </div>
                    <div class="form-control">
                        <label class="label">
                            <span class="label-text">"Poll interval (seconds)"</span>
                            <span class="label-text-alt opacity-70">
                                {move || {
                                    let v = poll_secs.get();
                                    if v == 0 {
                                        "disabled".to_string()
                                    } else {
                                        format!("{v}s")
                                    }
                                }}
                            </span>
                        </label>
                        <input
                            type="range"
                            min="0"
                            max="3600"
                            step="30"
                            class="range range-sm"
                            prop:value=move || poll_secs.get().to_string()
                            on:input=move |ev| {
                                let raw = event_target_value(&ev);
                                if let Ok(v) = raw.parse::<u64>() {
                                    set_poll_secs.set(v);
                                }
                            }
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
fn DeleteProjectView(
    target: RwSignal<Option<WireProject>>,
    set_error: WriteSignal<Option<String>>,
    on_success: impl Fn() + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let (submitting, set_submitting) = signal(false);
    // The shared `ConfirmDeleteModal` expects a `ReadSignal<bool>` +
    // `WriteSignal<bool>` pair; we mirror it onto `target.is_some()` so
    // both sides stay in sync.
    let (open_read, set_open) = signal(false);
    Effect::new(move |_| {
        let want_open = target.with(Option::is_some);
        if open_read.get_untracked() != want_open {
            set_open.set(want_open);
        }
    });
    Effect::new(move |_| {
        if !open_read.get() && target.with_untracked(Option::is_some) {
            // User hit Cancel — sync the target back to None.
            target.set(None);
        }
    });

    let title = Signal::derive(move || {
        target.get().map_or_else(
            || "Delete project".to_string(),
            |p| format!("Delete project '{}'?", p.name),
        )
    });
    let message = Signal::derive(move || {
        target
            .get()
            .map(|p| {
                format!(
                    "Delete project '{}'? This cascade-removes its deployment links and cannot be undone.",
                    p.name
                )
            })
            .unwrap_or_default()
    });

    let confirm = Callback::new(move |()| {
        let Some(p) = target.get() else {
            return;
        };
        set_submitting.set(true);
        spawn_local(async move {
            let result = manager_delete_project(p.id.clone()).await;
            set_submitting.set(false);
            match result {
                Ok(()) => {
                    target.set(None);
                    on_success();
                }
                Err(e) => {
                    set_error.set(Some(format!("Delete failed: {}", format_server_error(&e))));
                }
            }
        });
    });

    view! {
        <ConfirmDeleteModal
            open=open_read
            set_open=set_open
            title=title
            message=message
            on_confirm=confirm
            submitting=submitting
        />
    }
}
