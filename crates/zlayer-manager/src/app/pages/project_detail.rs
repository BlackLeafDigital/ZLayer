//! `/projects/:id` — project detail view.
//!
//! Nested route off the `/projects` list page. The `:id` segment is
//! extracted via `use_params_map`; a `Resource` fetches the project on
//! mount (and refetches when the id changes).
//!
//! Four tabs expose the project's functionality: **Source** (git URL /
//! branch + manual pull), **Build** (build kind / path / deploy-spec /
//! auto-deploy / polling), **Credentials** (attach or detach a registry
//! or git credential reference), and **Envs & Deployments** (linked
//! deployments + webhook URL & rotate).

use leptos::ev::SubmitEvent;
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::use_params_map;

use crate::app::components::forms::{ConfirmDeleteModal, TabBar};
use crate::app::server_fns::{
    manager_get_project, manager_get_project_webhook, manager_link_project_deployment,
    manager_list_credentials, manager_list_project_deployments, manager_pull_project,
    manager_rotate_project_webhook, manager_unlink_project_deployment, manager_update_project,
};
use crate::app::util::errors::format_server_error;
use crate::wire::projects::{
    WireBuildKind, WireProject, WireProjectCredential, WireProjectSpec, WirePullResult,
    WireWebhookInfo,
};

/// Project detail page. Mounted at `/projects/:id`.
#[component]
pub fn ProjectDetail() -> impl IntoView {
    let params = use_params_map();
    // Reactive signal scoped to the current `:id`. When the user navigates
    // between projects we reseed this and the `Resource` below refires.
    let id = Memo::new(move |_| params.read().get("id").unwrap_or_default());

    let project = Resource::new(
        move || id.get(),
        |pid| async move {
            if pid.is_empty() {
                return Err(ServerFnError::new("missing project id"));
            }
            manager_get_project(pid).await
        },
    );

    let selected_tab = RwSignal::new("source".to_string());
    let (page_error, set_page_error) = signal(None::<String>);

    let refetch_project = move || project.refetch();

    view! {
        <div class="p-6 space-y-4">
            <Suspense fallback=move || view! {
                <div class="flex justify-center p-6">
                    <span class="loading loading-spinner loading-md"></span>
                </div>
            }>
                {move || {
                    project
                        .get()
                        .map(|res| match res {
                            Err(e) => view! {
                                <div class="alert alert-warning">
                                    <span>
                                        {format!(
                                            "Project not found or load failed: {}",
                                            format_server_error(&e),
                                        )}
                                    </span>
                                </div>
                            }
                            .into_any(),
                            Ok(p) => {
                                let project_signal = RwSignal::new(p);
                                view! {
                                    <ProjectDetailBody
                                        project=project_signal
                                        selected_tab=selected_tab
                                        set_page_error=set_page_error
                                        refetch_project=refetch_project
                                    />
                                }
                                .into_any()
                            }
                        })
                }}
            </Suspense>

            {move || {
                page_error
                    .get()
                    .map(|msg| {
                        view! {
                            <div class="alert alert-error">
                                <span>{msg}</span>
                                <button
                                    class="btn btn-ghost btn-xs"
                                    on:click=move |_| set_page_error.set(None)
                                >
                                    "Dismiss"
                                </button>
                            </div>
                        }
                    })
            }}
        </div>
    }
}

#[component]
fn ProjectDetailBody(
    project: RwSignal<WireProject>,
    selected_tab: RwSignal<String>,
    set_page_error: WriteSignal<Option<String>>,
    refetch_project: impl Fn() + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let tabs = vec![
        ("source".to_string(), "Source".to_string()),
        ("build".to_string(), "Build".to_string()),
        ("credentials".to_string(), "Credentials".to_string()),
        ("envs".to_string(), "Envs & Deployments".to_string()),
    ];

    view! {
        <div class="flex items-baseline gap-3">
            <h1 class="text-2xl font-bold">{move || project.with(|p| p.name.clone())}</h1>
            <span class="opacity-60 text-sm font-mono">
                {move || project.with(|p| p.id.clone())}
            </span>
        </div>
        <div class="text-sm opacity-70">
            "Updated " {move || project.with(|p| p.updated_at.clone())}
        </div>

        <TabBar tabs=tabs selected=selected_tab />

        {move || {
            let tab = selected_tab.get();
            match tab.as_str() {
                "source" => view! {
                    <SourceTab
                        project=project
                        set_page_error=set_page_error
                        refetch_project=refetch_project
                    />
                }
                .into_any(),
                "build" => view! {
                    <BuildTab
                        project=project
                        set_page_error=set_page_error
                        refetch_project=refetch_project
                    />
                }
                .into_any(),
                "credentials" => view! {
                    <CredentialsTab
                        project=project
                        set_page_error=set_page_error
                        refetch_project=refetch_project
                    />
                }
                .into_any(),
                "envs" => view! {
                    <EnvsTab
                        project=project
                        set_page_error=set_page_error
                    />
                }
                .into_any(),
                _ => view! { <div></div> }.into_any(),
            }
        }}
    }
}

// ========================================================================
// Source tab — git URL (read-only), branch (editable), pull button.
// ========================================================================

#[component]
fn SourceTab(
    project: RwSignal<WireProject>,
    set_page_error: WriteSignal<Option<String>>,
    refetch_project: impl Fn() + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let (branch, set_branch) = signal(String::new());
    let (pull_result, set_pull_result) = signal(None::<WirePullResult>);
    let (last_pulled_sha, set_last_pulled_sha) = signal(None::<String>);
    let (saving, set_saving) = signal(false);
    let (pulling, set_pulling) = signal(false);

    // Seed the branch input from the project every time it changes.
    Effect::new(move |_| {
        project.with(|p| {
            set_branch.set(p.git_branch.clone().unwrap_or_else(|| "main".to_string()));
        });
    });

    let save_branch = move |ev: SubmitEvent| {
        ev.prevent_default();
        if saving.get() {
            return;
        }
        let branch_val = branch.get().trim().to_string();
        if branch_val.is_empty() {
            set_page_error.set(Some("Branch cannot be empty.".to_string()));
            return;
        }
        let id = project.with(|p| p.id.clone());
        let spec = WireProjectSpec {
            git_branch: Some(branch_val),
            ..Default::default()
        };
        set_saving.set(true);
        spawn_local(async move {
            let result = manager_update_project(id, spec).await;
            set_saving.set(false);
            match result {
                Ok(updated) => {
                    project.set(updated);
                    refetch_project();
                }
                Err(e) => set_page_error.set(Some(format!(
                    "Save branch failed: {}",
                    format_server_error(&e)
                ))),
            }
        });
    };

    let pull_now = move |_| {
        if pulling.get() {
            return;
        }
        let id = project.with(|p| p.id.clone());
        set_pulling.set(true);
        set_pull_result.set(None);
        spawn_local(async move {
            let result = manager_pull_project(id).await;
            set_pulling.set(false);
            match result {
                Ok(r) => {
                    set_last_pulled_sha.set(Some(r.sha.clone()));
                    set_pull_result.set(Some(r));
                }
                Err(e) => {
                    set_page_error.set(Some(format!("Pull failed: {}", format_server_error(&e))));
                }
            }
        });
    };

    view! {
        <div class="card bg-base-200 mt-4">
            <div class="card-body space-y-3">
                <h2 class="card-title text-lg">"Source"</h2>

                <div class="form-control">
                    <label class="label">
                        <span class="label-text">"Git URL"</span>
                        <span class="label-text-alt opacity-60">"read-only"</span>
                    </label>
                    <input
                        type="text"
                        class="input input-bordered w-full font-mono text-sm"
                        readonly=true
                        prop:value=move || project.with(|p| p.git_url.clone().unwrap_or_default())
                    />
                </div>

                <form on:submit=save_branch class="form-control">
                    <label class="label">
                        <span class="label-text">"Branch"</span>
                    </label>
                    <div class="join w-full">
                        <input
                            type="text"
                            class="input input-bordered join-item flex-1"
                            prop:value=branch
                            on:input=move |ev| set_branch.set(event_target_value(&ev))
                        />
                        <button
                            type="submit"
                            class="btn btn-primary join-item"
                            disabled=move || saving.get()
                        >
                            {move || if saving.get() { "Saving…" } else { "Save" }}
                        </button>
                    </div>
                </form>

                <div class="flex items-center gap-3">
                    <button
                        type="button"
                        class="btn btn-accent btn-sm"
                        on:click=pull_now
                        disabled=move || pulling.get()
                    >
                        {move || if pulling.get() { "Pulling…" } else { "Pull now" }}
                    </button>
                    <span class="text-sm opacity-70">
                        "Clones or fast-forwards the project's working copy."
                    </span>
                </div>

                {move || {
                    pull_result
                        .get()
                        .map(|r| {
                            view! {
                                <div class="alert alert-success text-sm">
                                    <span>
                                        {format!(
                                            "Pulled {} @ {} → {}",
                                            r.git_url,
                                            r.branch,
                                            &r.sha[..r.sha.len().min(12)],
                                        )}
                                    </span>
                                </div>
                            }
                        })
                }}

                <div class="form-control">
                    <label class="label">
                        <span class="label-text">"Last pulled SHA"</span>
                    </label>
                    <input
                        type="text"
                        class="input input-bordered w-full font-mono text-sm"
                        readonly=true
                        prop:value=move || {
                            last_pulled_sha
                                .get()
                                .map_or_else(
                                    || "(none)".to_string(),
                                    |s| s[..s.len().min(40)].to_string(),
                                )
                        }
                    />
                </div>
            </div>
        </div>
    }
}

// ========================================================================
// Build tab — build kind, build path, deploy spec, auto-deploy, polling.
// ========================================================================

#[component]
#[allow(clippy::too_many_lines)]
fn BuildTab(
    project: RwSignal<WireProject>,
    set_page_error: WriteSignal<Option<String>>,
    refetch_project: impl Fn() + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let (build_kind, set_build_kind) = signal(String::new());
    let (build_path, set_build_path) = signal(String::new());
    let (deploy_spec_path, set_deploy_spec_path) = signal(String::new());
    let (auto_deploy, set_auto_deploy) = signal(false);
    let (poll_secs, set_poll_secs) = signal(0_u64);
    let (saving, set_saving) = signal(false);

    // Seed from the current project. Any later project update (e.g. after
    // a successful save) re-runs this effect because we read the reactive
    // signal, keeping the form in sync.
    Effect::new(move |_| {
        project.with(|p| {
            set_build_kind.set(
                p.build_kind
                    .map(|k| k.as_str().to_string())
                    .unwrap_or_default(),
            );
            set_build_path.set(p.build_path.clone().unwrap_or_default());
            set_deploy_spec_path.set(p.deploy_spec_path.clone().unwrap_or_default());
            set_auto_deploy.set(p.auto_deploy);
            set_poll_secs.set(p.poll_interval_secs.unwrap_or(0));
        });
    });

    let save = move |ev: SubmitEvent| {
        ev.prevent_default();
        if saving.get() {
            return;
        }

        let kind_val = build_kind.get();
        let kind_parsed = if kind_val.is_empty() {
            None
        } else {
            WireBuildKind::parse_wire(&kind_val)
        };
        let build_path_val = build_path.get().trim().to_string();
        let deploy_spec_val = deploy_spec_path.get().trim().to_string();
        let poll_val = poll_secs.get();

        let id = project.with(|p| p.id.clone());
        let spec = WireProjectSpec {
            build_kind: kind_parsed,
            // The daemon accepts empty string = clear; we send the trimmed
            // string either way so clearing works from the UI.
            build_path: Some(build_path_val),
            deploy_spec_path: Some(deploy_spec_val),
            auto_deploy: Some(auto_deploy.get()),
            poll_interval_secs: if poll_val == 0 { None } else { Some(poll_val) },
            ..Default::default()
        };

        set_saving.set(true);
        spawn_local(async move {
            let result = manager_update_project(id, spec).await;
            set_saving.set(false);
            match result {
                Ok(updated) => {
                    project.set(updated);
                    refetch_project();
                }
                Err(e) => set_page_error.set(Some(format!(
                    "Save build settings failed: {}",
                    format_server_error(&e)
                ))),
            }
        });
    };

    view! {
        <div class="card bg-base-200 mt-4">
            <div class="card-body space-y-3">
                <h2 class="card-title text-lg">"Build"</h2>

                <form on:submit=save class="space-y-3">
                    <div class="form-control">
                        <label class="label">
                            <span class="label-text">"Build kind"</span>
                        </label>
                        <div class="join">
                            {["", "dockerfile", "compose", "zimagefile", "spec"]
                                .iter()
                                .map(|k| {
                                    let k_str = (*k).to_string();
                                    let label = if k.is_empty() { "none".to_string() } else { k_str.clone() };
                                    let k_cmp = k_str.clone();
                                    view! {
                                        <button
                                            type="button"
                                            class="btn join-item btn-sm"
                                            class:btn-primary=move || build_kind.get() == k_cmp
                                            on:click={
                                                let k_click = k_str.clone();
                                                move |_| set_build_kind.set(k_click.clone())
                                            }
                                        >
                                            {label}
                                        </button>
                                    }
                                })
                                .collect_view()}
                        </div>
                    </div>

                    <div class="form-control">
                        <label class="label">
                            <span class="label-text">"Build path"</span>
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
                        <label class="label">
                            <span class="label-text">"Deploy spec path"</span>
                            <span class="label-text-alt opacity-70">"Path to DeploymentSpec YAML"</span>
                        </label>
                        <input
                            type="text"
                            class="input input-bordered w-full font-mono text-sm"
                            placeholder="./deployment.yaml"
                            prop:value=deploy_spec_path
                            on:input=move |ev| set_deploy_spec_path.set(event_target_value(&ev))
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
                                    if v == 0 { "disabled".to_string() } else { format!("{v}s") }
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

                    <div class="flex justify-end">
                        <button
                            type="submit"
                            class="btn btn-primary btn-sm"
                            disabled=move || saving.get()
                        >
                            {move || if saving.get() { "Saving…" } else { "Save build settings" }}
                        </button>
                    </div>
                </form>
            </div>
        </div>
    }
}

// ========================================================================
// Credentials tab — attach/detach registry + git credential references.
// ========================================================================

#[component]
#[allow(clippy::too_many_lines)]
fn CredentialsTab(
    project: RwSignal<WireProject>,
    set_page_error: WriteSignal<Option<String>>,
    refetch_project: impl Fn() + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let credentials = Resource::new(
        || (),
        |()| async move { manager_list_credentials(None).await },
    );

    let (attach_open, set_attach_open) = signal(false);
    let (attach_kind, set_attach_kind) = signal("registry".to_string());
    let (attach_selected, set_attach_selected) = signal(String::new());
    let (detaching, set_detaching) = signal(false);

    let all_creds = Memo::new(move |_| credentials.get().and_then(Result::ok).unwrap_or_default());

    let attached_rows = Memo::new(move |_| {
        let (reg_id, git_id) = project.with(|p| {
            (
                p.registry_credential_id.clone(),
                p.git_credential_id.clone(),
            )
        });
        let creds = all_creds.get();
        let mut out: Vec<WireProjectCredential> = Vec::new();
        if let Some(rid) = reg_id {
            if let Some(hit) = creds.iter().find(|c| c.kind == "registry" && c.id == rid) {
                out.push(hit.clone());
            } else {
                out.push(WireProjectCredential {
                    id: rid,
                    kind: "registry".to_string(),
                    name: "(missing)".to_string(),
                    sub_kind: "unknown".to_string(),
                });
            }
        }
        if let Some(gid) = git_id {
            if let Some(hit) = creds.iter().find(|c| c.kind == "git" && c.id == gid) {
                out.push(hit.clone());
            } else {
                out.push(WireProjectCredential {
                    id: gid,
                    kind: "git".to_string(),
                    name: "(missing)".to_string(),
                    sub_kind: "unknown".to_string(),
                });
            }
        }
        out
    });

    let detach_target = RwSignal::new(None::<WireProjectCredential>);

    let attach = move |ev: SubmitEvent| {
        ev.prevent_default();
        let kind = attach_kind.get();
        let cred_id = attach_selected.get();
        if cred_id.is_empty() {
            set_page_error.set(Some("Select a credential first.".to_string()));
            return;
        }
        let id = project.with(|p| p.id.clone());
        let spec = if kind == "registry" {
            WireProjectSpec {
                registry_credential_id: Some(cred_id),
                ..Default::default()
            }
        } else {
            WireProjectSpec {
                git_credential_id: Some(cred_id),
                ..Default::default()
            }
        };
        spawn_local(async move {
            match manager_update_project(id, spec).await {
                Ok(updated) => {
                    project.set(updated);
                    refetch_project();
                    set_attach_open.set(false);
                    set_attach_selected.set(String::new());
                }
                Err(e) => set_page_error.set(Some(format!(
                    "Attach credential failed: {}",
                    format_server_error(&e)
                ))),
            }
        });
    };

    let confirm_detach = Callback::new(move |()| {
        let Some(target_cred) = detach_target.get() else {
            return;
        };
        let id = project.with(|p| p.id.clone());
        // Send an empty string to clear — the daemon treats empty strings
        // on these fields as "clear the reference" (see
        // `update_project` in handlers/projects.rs).
        let spec = if target_cred.kind == "registry" {
            WireProjectSpec {
                registry_credential_id: Some(String::new()),
                ..Default::default()
            }
        } else {
            WireProjectSpec {
                git_credential_id: Some(String::new()),
                ..Default::default()
            }
        };
        set_detaching.set(true);
        spawn_local(async move {
            let result = manager_update_project(id, spec).await;
            set_detaching.set(false);
            match result {
                Ok(updated) => {
                    project.set(updated);
                    refetch_project();
                    detach_target.set(None);
                }
                Err(e) => set_page_error.set(Some(format!(
                    "Detach credential failed: {}",
                    format_server_error(&e)
                ))),
            }
        });
    });

    let (detach_open_read, set_detach_open) = signal(false);
    Effect::new(move |_| {
        let want_open = detach_target.with(Option::is_some);
        if detach_open_read.get_untracked() != want_open {
            set_detach_open.set(want_open);
        }
    });
    Effect::new(move |_| {
        if !detach_open_read.get() && detach_target.with_untracked(Option::is_some) {
            detach_target.set(None);
        }
    });
    let detach_title = Signal::derive(move || {
        detach_target.get().map_or_else(
            || "Detach credential".to_string(),
            |c| format!("Detach {} credential", c.kind),
        )
    });
    let detach_message = Signal::derive(move || {
        detach_target
            .get()
            .map(|c| {
                format!(
                    "Detach the {} credential '{}' from this project? Builds and pulls that rely on it will start failing.",
                    c.kind, c.name
                )
            })
            .unwrap_or_default()
    });

    view! {
        <div class="card bg-base-200 mt-4">
            <div class="card-body space-y-3">
                <div class="flex justify-between items-center">
                    <h2 class="card-title text-lg">"Credentials"</h2>
                    <button
                        type="button"
                        class="btn btn-primary btn-sm"
                        on:click=move |_| set_attach_open.set(true)
                    >
                        "Attach credential"
                    </button>
                </div>

                <Suspense fallback=move || view! {
                    <div class="p-6 flex justify-center">
                        <span class="loading loading-spinner loading-md"></span>
                    </div>
                }>
                    {move || {
                        credentials.get().map(|res| {
                            if let Err(e) = res {
                                view! {
                                    <div class="p-4 text-error">
                                        {format!(
                                            "Failed to load credentials: {}",
                                            format_server_error(&e),
                                        )}
                                    </div>
                                }
                                .into_any()
                            } else {
                                let rows = attached_rows.get();
                                render_attached_credentials(rows, detach_target).into_any()
                            }
                        })
                    }}
                </Suspense>
            </div>
        </div>

        <dialog class=move || if attach_open.get() { "modal modal-open" } else { "modal" }>
            <div class="modal-box">
                <h3 class="font-bold text-lg mb-3">"Attach credential"</h3>
                <form on:submit=attach class="space-y-3">
                    <div class="form-control">
                        <label class="label">
                            <span class="label-text">"Credential kind"</span>
                        </label>
                        <div class="join">
                            <button
                                type="button"
                                class="btn join-item btn-sm"
                                class:btn-primary=move || attach_kind.get() == "registry"
                                on:click=move |_| set_attach_kind.set("registry".to_string())
                            >
                                "Registry"
                            </button>
                            <button
                                type="button"
                                class="btn join-item btn-sm"
                                class:btn-primary=move || attach_kind.get() == "git"
                                on:click=move |_| set_attach_kind.set("git".to_string())
                            >
                                "Git"
                            </button>
                        </div>
                    </div>

                    <div class="form-control">
                        <label class="label">
                            <span class="label-text">"Pick credential"</span>
                        </label>
                        {move || {
                            let kind = attach_kind.get();
                            let options = all_creds
                                .get()
                                .into_iter()
                                .filter(|c| c.kind == kind)
                                .collect::<Vec<_>>();
                            view! {
                                <select
                                    class="select select-bordered w-full"
                                    prop:value=attach_selected
                                    on:change=move |ev| set_attach_selected.set(event_target_value(&ev))
                                >
                                    <option value="">"(choose one)"</option>
                                    {options
                                        .into_iter()
                                        .map(|c| {
                                            view! {
                                                <option value=c.id.clone()>
                                                    {format!("{} [{}]", c.name, c.sub_kind)}
                                                </option>
                                            }
                                        })
                                        .collect_view()}
                                </select>
                            }
                        }}
                    </div>

                    <div class="modal-action">
                        <button
                            type="button"
                            class="btn btn-ghost"
                            on:click=move |_| set_attach_open.set(false)
                        >
                            "Cancel"
                        </button>
                        <button type="submit" class="btn btn-primary">
                            "Attach"
                        </button>
                    </div>
                </form>
            </div>
            <form
                method="dialog"
                class="modal-backdrop"
                on:submit=move |_| set_attach_open.set(false)
            >
                <button>"close"</button>
            </form>
        </dialog>

        <ConfirmDeleteModal
            open=detach_open_read
            set_open=set_detach_open
            title=detach_title
            message=detach_message
            confirm_label="Detach".to_string()
            on_confirm=confirm_detach
            submitting=detaching
        />
    }
}

fn render_attached_credentials(
    rows: Vec<WireProjectCredential>,
    detach_target: RwSignal<Option<WireProjectCredential>>,
) -> impl IntoView {
    if rows.is_empty() {
        return view! {
            <div class="p-4 opacity-70">"No credentials attached to this project."</div>
        }
        .into_any();
    }
    view! {
        <div class="overflow-x-auto">
            <table class="table table-zebra">
                <thead>
                    <tr>
                        <th>"Kind"</th>
                        <th>"Name"</th>
                        <th>"Sub-kind"</th>
                        <th class="text-right">"Actions"</th>
                    </tr>
                </thead>
                <tbody>
                    {rows
                        .into_iter()
                        .map(|c| {
                            let c_for_detach = c.clone();
                            view! {
                                <tr>
                                    <td>
                                        <span class="badge badge-outline">{c.kind.clone()}</span>
                                    </td>
                                    <td>{c.name.clone()}</td>
                                    <td>
                                        <span class="badge badge-sm">{c.sub_kind.clone()}</span>
                                    </td>
                                    <td class="text-right">
                                        <button
                                            class="btn btn-xs btn-error"
                                            on:click=move |_| detach_target.set(Some(c_for_detach.clone()))
                                        >
                                            "Detach"
                                        </button>
                                    </td>
                                </tr>
                            }
                        })
                        .collect_view()}
                </tbody>
            </table>
        </div>
    }
    .into_any()
}

// ========================================================================
// Envs & Deployments tab — linked deployments + webhook URL / rotate.
// ========================================================================

#[component]
#[allow(clippy::too_many_lines)]
fn EnvsTab(
    project: RwSignal<WireProject>,
    set_page_error: WriteSignal<Option<String>>,
) -> impl IntoView {
    let project_id = Memo::new(move |_| project.with(|p| p.id.clone()));

    let deployments = Resource::new(
        move || project_id.get(),
        |pid| async move { manager_list_project_deployments(pid).await },
    );

    let webhook = Resource::new(
        move || project_id.get(),
        |pid| async move { manager_get_project_webhook(pid).await },
    );

    let (show_secret, set_show_secret) = signal(false);
    let (rotating, set_rotating) = signal(false);
    let (rotated_secret, set_rotated_secret) = signal(None::<WireWebhookInfo>);
    let rotate_confirm = RwSignal::new(false);

    let (link_open, set_link_open) = signal(false);
    let (link_value, set_link_value) = signal(String::new());
    let (linking, set_linking) = signal(false);

    let link = move |ev: SubmitEvent| {
        ev.prevent_default();
        if linking.get() {
            return;
        }
        let name = link_value.get().trim().to_string();
        if name.is_empty() {
            set_page_error.set(Some("Deployment name cannot be empty.".to_string()));
            return;
        }
        let id = project.with(|p| p.id.clone());
        set_linking.set(true);
        spawn_local(async move {
            let result = manager_link_project_deployment(id, name).await;
            set_linking.set(false);
            match result {
                Ok(()) => {
                    set_link_open.set(false);
                    set_link_value.set(String::new());
                    deployments.refetch();
                }
                Err(e) => set_page_error.set(Some(format!(
                    "Link deployment failed: {}",
                    format_server_error(&e)
                ))),
            }
        });
    };

    let unlink_target = RwSignal::new(None::<String>);

    let confirm_unlink = Callback::new(move |()| {
        let Some(name) = unlink_target.get() else {
            return;
        };
        let id = project.with(|p| p.id.clone());
        spawn_local(async move {
            match manager_unlink_project_deployment(id, name).await {
                Ok(()) => {
                    unlink_target.set(None);
                    deployments.refetch();
                }
                Err(e) => set_page_error.set(Some(format!(
                    "Unlink deployment failed: {}",
                    format_server_error(&e)
                ))),
            }
        });
    });

    let (unlink_open_read, set_unlink_open) = signal(false);
    Effect::new(move |_| {
        let want_open = unlink_target.with(Option::is_some);
        if unlink_open_read.get_untracked() != want_open {
            set_unlink_open.set(want_open);
        }
    });
    Effect::new(move |_| {
        if !unlink_open_read.get() && unlink_target.with_untracked(Option::is_some) {
            unlink_target.set(None);
        }
    });
    let unlink_title = Signal::derive(move || {
        unlink_target.get().map_or_else(
            || "Unlink deployment".to_string(),
            |n| format!("Unlink '{n}'"),
        )
    });
    let unlink_message = Signal::derive(move || {
        unlink_target
            .get()
            .map(|n| format!("Unlink deployment '{n}' from this project?"))
            .unwrap_or_default()
    });
    let (unlinking, _set_unlinking) = signal(false);

    let rotate = move |()| {
        if rotating.get() {
            return;
        }
        let id = project.with(|p| p.id.clone());
        set_rotating.set(true);
        spawn_local(async move {
            let result = manager_rotate_project_webhook(id).await;
            set_rotating.set(false);
            match result {
                Ok(info) => {
                    set_rotated_secret.set(Some(info));
                    webhook.refetch();
                    rotate_confirm.set(false);
                }
                Err(e) => set_page_error.set(Some(format!(
                    "Rotate webhook failed: {}",
                    format_server_error(&e)
                ))),
            }
        });
    };

    let (rotate_open_read, set_rotate_open) = signal(false);
    Effect::new(move |_| {
        let want_open = rotate_confirm.get();
        if rotate_open_read.get_untracked() != want_open {
            set_rotate_open.set(want_open);
        }
    });
    Effect::new(move |_| {
        if !rotate_open_read.get() && rotate_confirm.get_untracked() {
            rotate_confirm.set(false);
        }
    });

    view! {
        <div class="space-y-4 mt-4">
            // ---- Deployments card ----
            <div class="card bg-base-200">
                <div class="card-body space-y-3">
                    <div class="flex justify-between items-center">
                        <h2 class="card-title text-lg">"Linked deployments"</h2>
                        <button
                            type="button"
                            class="btn btn-primary btn-sm"
                            on:click=move |_| set_link_open.set(true)
                        >
                            "Link deployment"
                        </button>
                    </div>

                    <Suspense fallback=move || view! {
                        <div class="p-6 flex justify-center">
                            <span class="loading loading-spinner loading-md"></span>
                        </div>
                    }>
                        {move || {
                            deployments
                                .get()
                                .map(|res| match res {
                                    Err(e) => view! {
                                        <div class="p-4 text-error">
                                            {format!(
                                                "Failed to load deployments: {}",
                                                format_server_error(&e),
                                            )}
                                        </div>
                                    }
                                    .into_any(),
                                    Ok(names) => {
                                        if names.is_empty() {
                                            view! {
                                                <div class="opacity-70 text-sm">
                                                    "No deployments linked to this project yet."
                                                </div>
                                            }
                                            .into_any()
                                        } else {
                                            view! {
                                                <div class="overflow-x-auto">
                                                    <table class="table table-zebra">
                                                        <thead>
                                                            <tr>
                                                                <th>"Deployment name"</th>
                                                                <th class="text-right">"Actions"</th>
                                                            </tr>
                                                        </thead>
                                                        <tbody>
                                                            {names
                                                                .into_iter()
                                                                .map(|n| {
                                                                    let for_unlink = n.clone();
                                                                    view! {
                                                                        <tr>
                                                                            <td class="font-mono text-sm">{n}</td>
                                                                            <td class="text-right">
                                                                                <button
                                                                                    class="btn btn-xs btn-error"
                                                                                    on:click=move |_| unlink_target.set(Some(for_unlink.clone()))
                                                                                >
                                                                                    "Unlink"
                                                                                </button>
                                                                            </td>
                                                                        </tr>
                                                                    }
                                                                })
                                                                .collect_view()}
                                                        </tbody>
                                                    </table>
                                                </div>
                                            }
                                            .into_any()
                                        }
                                    }
                                })
                        }}
                    </Suspense>
                </div>
            </div>

            // ---- Webhook card ----
            <div class="card bg-base-200">
                <div class="card-body space-y-3">
                    <h2 class="card-title text-lg">"Webhook"</h2>

                    <Suspense fallback=move || view! {
                        <div class="p-6 flex justify-center">
                            <span class="loading loading-spinner loading-md"></span>
                        </div>
                    }>
                        {move || {
                            webhook
                                .get()
                                .map(|res| match res {
                                    Err(e) => view! {
                                        <div class="p-4 text-error">
                                            {format!(
                                                "Failed to load webhook info: {}",
                                                format_server_error(&e),
                                            )}
                                        </div>
                                    }
                                    .into_any(),
                                    Ok(info) => {
                                        let rotated = rotated_secret.get();
                                        let secret = rotated.as_ref().map_or_else(
                                            || info.secret.clone(),
                                            |r| r.secret.clone(),
                                        );
                                        let url = rotated.as_ref().map_or_else(
                                            || info.url.clone(),
                                            |r| r.url.clone(),
                                        );
                                        view! {
                                            <div class="form-control">
                                                <label class="label">
                                                    <span class="label-text">"URL (replace {provider} with github/gitea/forgejo/gitlab)"</span>
                                                </label>
                                                <input
                                                    type="text"
                                                    class="input input-bordered w-full font-mono text-sm"
                                                    readonly=true
                                                    prop:value=url
                                                />
                                            </div>

                                            <div class="form-control">
                                                <label class="label">
                                                    <span class="label-text">"Secret"</span>
                                                    <button
                                                        type="button"
                                                        class="btn btn-ghost btn-xs"
                                                        on:click=move |_| set_show_secret.update(|v| *v = !*v)
                                                    >
                                                        {move || if show_secret.get() { "Hide" } else { "Show" }}
                                                    </button>
                                                </label>
                                                <input
                                                    type="text"
                                                    class="input input-bordered w-full font-mono text-sm"
                                                    readonly=true
                                                    prop:value=move || {
                                                        if show_secret.get() {
                                                            secret.clone()
                                                        } else {
                                                            "•".repeat(secret.len().min(32))
                                                        }
                                                    }
                                                />
                                            </div>
                                        }
                                        .into_any()
                                    }
                                })
                        }}
                    </Suspense>

                    <div class="flex justify-end">
                        <button
                            type="button"
                            class="btn btn-warning btn-sm"
                            on:click=move |_| rotate_confirm.set(true)
                            disabled=move || rotating.get()
                        >
                            {move || if rotating.get() { "Rotating…" } else { "Rotate webhook secret" }}
                        </button>
                    </div>
                </div>
            </div>
        </div>

        // ---- Link deployment modal ----
        <dialog class=move || if link_open.get() { "modal modal-open" } else { "modal" }>
            <div class="modal-box">
                <h3 class="font-bold text-lg mb-3">"Link deployment"</h3>
                <form on:submit=link class="space-y-3">
                    <div class="form-control">
                        <label class="label">
                            <span class="label-text">"Deployment name"</span>
                        </label>
                        <input
                            type="text"
                            class="input input-bordered w-full"
                            required
                            prop:value=link_value
                            on:input=move |ev| set_link_value.set(event_target_value(&ev))
                        />
                    </div>
                    <div class="modal-action">
                        <button
                            type="button"
                            class="btn btn-ghost"
                            on:click=move |_| set_link_open.set(false)
                        >
                            "Cancel"
                        </button>
                        <button
                            type="submit"
                            class="btn btn-primary"
                            disabled=move || linking.get()
                        >
                            {move || if linking.get() { "Linking…" } else { "Link" }}
                        </button>
                    </div>
                </form>
            </div>
            <form method="dialog" class="modal-backdrop" on:submit=move |_| set_link_open.set(false)>
                <button>"close"</button>
            </form>
        </dialog>

        <ConfirmDeleteModal
            open=unlink_open_read
            set_open=set_unlink_open
            title=unlink_title
            message=unlink_message
            confirm_label="Unlink".to_string()
            on_confirm=confirm_unlink
            submitting=unlinking
        />

        // ---- Rotate webhook confirm ----
        <ConfirmDeleteModal
            open=rotate_open_read
            set_open=set_rotate_open
            title=Signal::derive(|| "Rotate webhook secret?".to_string())
            message=Signal::derive(|| {
                "Rotating the secret invalidates any git host webhook that's still using the old value. You'll need to update the remote configuration afterwards.".to_string()
            })
            confirm_label="Rotate".to_string()
            on_confirm=Callback::new(move |()| rotate(()))
            submitting=rotating
        />
    }
}
