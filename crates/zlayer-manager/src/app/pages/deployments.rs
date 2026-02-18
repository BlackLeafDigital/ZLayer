//! Deployments page
//!
//! Displays deployment list with `DaisyUI` table styling.
//! Supports creating, editing (read-only view), and deleting deployments.

use leptos::prelude::*;

use crate::app::server_fns::{
    create_deployment, delete_deployment, get_deployments, get_service_logs, get_services,
    Deployment, Service,
};

/// Get the CSS class for a deployment status badge
fn deployment_status_class(status: &str) -> &'static str {
    match status {
        "running" => "badge badge-success",
        "pending" | "deploying" => "badge badge-warning",
        "failed" => "badge badge-error",
        _ => "badge badge-ghost",
    }
}

/// Get the CSS class for a service status badge
fn service_status_class(status: &str) -> &'static str {
    match status {
        "running" => "badge badge-success badge-sm",
        "pending" | "deploying" => "badge badge-warning badge-sm",
        "failed" | "error" => "badge badge-error badge-sm",
        "stopped" => "badge badge-ghost badge-sm",
        _ => "badge badge-ghost badge-sm",
    }
}

/// Render deployments table from data
fn render_deployments_table(
    deployments: Vec<Deployment>,
    on_delete: impl Fn(String) + 'static + Clone,
    on_view: impl Fn(Deployment) + 'static + Clone,
) -> impl IntoView {
    view! {
        <div class="overflow-x-auto">
            <table class="table table-zebra w-full">
                <thead>
                    <tr>
                        <th>"Name"</th>
                        <th>"Status"</th>
                        <th>"Replicas"</th>
                        <th>"Created"</th>
                        <th>"Actions"</th>
                    </tr>
                </thead>
                <tbody>
                    {deployments
                        .into_iter()
                        .map(|d| {
                            let status_class = deployment_status_class(&d.status);
                            let name_for_delete = d.name.clone();
                            let dep_for_view = d.clone();
                            let on_delete = on_delete.clone();
                            let on_view = on_view.clone();
                            view! {
                                <tr>
                                    <td class="font-medium">{d.name}</td>
                                    <td>
                                        <span class=status_class>{d.status}</span>
                                    </td>
                                    <td>{format!("{}/{}", d.replicas, d.target_replicas)}</td>
                                    <td>{d.created_at}</td>
                                    <td>
                                        <button
                                            class="btn btn-ghost btn-xs"
                                            on:click=move |_| on_view(dep_for_view.clone())
                                        >
                                            "View"
                                        </button>
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

/// Deployments page component
///
/// Displays a table of deployments with "New Deployment" button,
/// delete confirmation, and a view modal for deployment details.
#[component]
pub fn Deployments() -> impl IntoView {
    let (refresh, set_refresh) = signal(0u32);
    let deployments = Resource::new(move || refresh.get(), |_| get_deployments());

    // Modal states
    let (show_create_modal, set_show_create_modal) = signal(false);
    let (show_delete_modal, set_show_delete_modal) = signal(false);
    let (show_view_modal, set_show_view_modal) = signal(false);
    let (delete_target, set_delete_target) = signal(String::new());
    let (yaml_input, set_yaml_input) = signal(String::new());
    let (error_msg, set_error_msg) = signal(Option::<String>::None);

    // View modal state
    let (view_deployment, set_view_deployment) = signal(Option::<Deployment>::None);
    let (view_services, set_view_services) = signal(Vec::<Service>::new());
    let (view_services_loading, set_view_services_loading) = signal(false);
    let (view_services_error, set_view_services_error) = signal(Option::<String>::None);

    // Logs state
    let (logs_service_name, set_logs_service_name) = signal(Option::<String>::None);
    let (logs_content, set_logs_content) = signal(String::new());
    let (logs_loading, set_logs_loading) = signal(false);

    // Create deployment action
    let create_action = Action::new(move |yaml: &String| {
        let yaml = yaml.clone();
        async move {
            let result = create_deployment(yaml).await;
            match result {
                Ok(()) => {
                    set_show_create_modal.set(false);
                    set_yaml_input.set(String::new());
                    set_error_msg.set(None);
                    set_refresh.update(|n| *n += 1);
                }
                Err(e) => {
                    set_error_msg.set(Some(e.to_string()));
                }
            }
        }
    });

    // Delete deployment action
    let delete_action = Action::new(move |name: &String| {
        let name = name.clone();
        async move {
            let result = delete_deployment(name).await;
            if result.is_ok() {
                set_show_delete_modal.set(false);
                set_delete_target.set(String::new());
                set_refresh.update(|n| *n += 1);
            }
        }
    });

    // Open view modal and fetch services
    let open_view_action = Action::new(move |dep: &Deployment| {
        let dep = dep.clone();
        let dep_name = dep.name.clone();
        async move {
            set_view_deployment.set(Some(dep));
            set_view_services.set(Vec::new());
            set_view_services_error.set(None);
            set_view_services_loading.set(true);
            set_logs_service_name.set(None);
            set_logs_content.set(String::new());
            set_show_view_modal.set(true);

            match get_services(dep_name).await {
                Ok(services) => {
                    set_view_services.set(services);
                }
                Err(e) => {
                    set_view_services_error.set(Some(e.to_string()));
                }
            }
            set_view_services_loading.set(false);
        }
    });

    // Fetch logs for a service
    let fetch_logs_action = Action::new(move |input: &(String, String)| {
        let (deployment_name, service_name) = input.clone();
        let svc_display = service_name.clone();
        async move {
            // Toggle off if already viewing this service's logs
            if logs_service_name.get().as_deref() == Some(&svc_display) {
                set_logs_service_name.set(None);
                set_logs_content.set(String::new());
                return;
            }

            set_logs_service_name.set(Some(svc_display));
            set_logs_content.set(String::new());
            set_logs_loading.set(true);

            match get_service_logs(deployment_name, service_name, Some(100)).await {
                Ok(logs) => {
                    set_logs_content.set(if logs.is_empty() {
                        "No logs available.".to_string()
                    } else {
                        logs
                    });
                }
                Err(e) => {
                    set_logs_content.set(format!("Error fetching logs: {e}"));
                }
            }
            set_logs_loading.set(false);
        }
    });

    view! {
        <div class="p-6">
            <div class="flex justify-between items-center mb-6">
                <h1 class="text-3xl font-bold">"Deployments"</h1>
                <button
                    class="btn btn-primary"
                    on:click=move |_| {
                        set_error_msg.set(None);
                        set_yaml_input.set(String::new());
                        set_show_create_modal.set(true);
                    }
                >
                    "New Deployment"
                </button>
            </div>

            <Suspense fallback=move || {
                view! {
                    <div class="flex justify-center p-8">
                        <span class="loading loading-spinner loading-lg"></span>
                    </div>
                }
            }>
                <ErrorBoundary fallback=|errors| {
                    view! {
                        <div class="alert alert-error">
                            <span>{move || format!("Error: {:?}", errors.get())}</span>
                        </div>
                    }
                }>
                    {move || {
                        deployments
                            .get()
                            .map(|result| {
                                match result {
                                    Ok(deps) => {
                                        if deps.is_empty() {
                                            view! {
                                                <div class="alert alert-info">
                                                    <span>"No deployments found. Create your first deployment!"</span>
                                                </div>
                                            }
                                                .into_any()
                                        } else {
                                            let on_delete = move |name: String| {
                                                set_delete_target.set(name);
                                                set_show_delete_modal.set(true);
                                            };
                                            let on_view = move |dep: Deployment| {
                                                open_view_action.dispatch(dep);
                                            };
                                            render_deployments_table(deps, on_delete, on_view)
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

            // Create Deployment Modal
            <div class=move || {
                if show_create_modal.get() { "modal modal-open" } else { "modal" }
            }>
                <div class="modal-box w-11/12 max-w-3xl">
                    <h3 class="font-bold text-lg">"New Deployment"</h3>
                    <p class="py-2 text-base-content/70">
                        "Paste your deployment YAML specification below."
                    </p>

                    {move || {
                        error_msg
                            .get()
                            .map(|msg| {
                                view! {
                                    <div class="alert alert-error mb-4">
                                        <span>{msg}</span>
                                    </div>
                                }
                            })
                    }}

                    <div class="form-control w-full">
                        <label class="label">
                            <span class="label-text">"Deployment YAML"</span>
                        </label>
                        <textarea
                            class="textarea textarea-bordered w-full h-64 font-mono text-sm"
                            placeholder="deployment: my-app\nservices:\n  web:\n    image: nginx:latest\n    ports:\n      - 80:80"
                            prop:value=move || yaml_input.get()
                            on:input=move |ev| {
                                set_yaml_input
                                    .set(event_target_value(&ev));
                            }
                        ></textarea>
                    </div>

                    <div class="modal-action">
                        <button
                            class="btn"
                            on:click=move |_| set_show_create_modal.set(false)
                        >
                            "Cancel"
                        </button>
                        <button
                            class="btn btn-primary"
                            prop:disabled=move || yaml_input.get().trim().is_empty()
                            on:click=move |_| {
                                create_action.dispatch(yaml_input.get());
                            }
                        >
                            "Deploy"
                        </button>
                    </div>
                </div>
                <div
                    class="modal-backdrop"
                    on:click=move |_| set_show_create_modal.set(false)
                ></div>
            </div>

            // Delete Confirmation Modal
            <div class=move || {
                if show_delete_modal.get() { "modal modal-open" } else { "modal" }
            }>
                <div class="modal-box">
                    <h3 class="font-bold text-lg text-error">"Delete Deployment"</h3>
                    <p class="py-4">
                        "Are you sure you want to delete deployment "
                        <span class="font-bold font-mono">{move || delete_target.get()}</span>
                        "? This action cannot be undone."
                    </p>
                    <div class="modal-action">
                        <button
                            class="btn"
                            on:click=move |_| set_show_delete_modal.set(false)
                        >
                            "Cancel"
                        </button>
                        <button
                            class="btn btn-error"
                            on:click=move |_| {
                                delete_action.dispatch(delete_target.get());
                            }
                        >
                            "Delete"
                        </button>
                    </div>
                </div>
                <div
                    class="modal-backdrop"
                    on:click=move |_| set_show_delete_modal.set(false)
                ></div>
            </div>

            // View Deployment Modal
            <div class=move || {
                if show_view_modal.get() { "modal modal-open" } else { "modal" }
            }>
                <div class="modal-box w-11/12 max-w-4xl">
                    // Deployment header
                    {move || {
                        view_deployment.get().map(|dep| {
                            let status_class = deployment_status_class(&dep.status);
                            view! {
                                <div>
                                    <h3 class="font-bold text-lg">
                                        "Deployment: "
                                        <span class="font-mono">{dep.name}</span>
                                    </h3>
                                    <div class="mt-3 grid grid-cols-2 md:grid-cols-4 gap-4">
                                        <div class="stat bg-base-200 rounded-lg p-3">
                                            <div class="stat-title text-xs">"Status"</div>
                                            <div class="stat-value text-sm">
                                                <span class=status_class>{dep.status}</span>
                                            </div>
                                        </div>
                                        <div class="stat bg-base-200 rounded-lg p-3">
                                            <div class="stat-title text-xs">"Replicas"</div>
                                            <div class="stat-value text-sm">
                                                {format!("{}/{}", dep.replicas, dep.target_replicas)}
                                            </div>
                                        </div>
                                        <div class="stat bg-base-200 rounded-lg p-3">
                                            <div class="stat-title text-xs">"Created"</div>
                                            {
                                                let created_title = dep.created_at.clone();
                                                let created_display = dep.created_at.clone();
                                                view! {
                                                    <div class="stat-value text-sm truncate" title=created_title>
                                                        {created_display}
                                                    </div>
                                                }
                                            }
                                        </div>
                                        <div class="stat bg-base-200 rounded-lg p-3">
                                            <div class="stat-title text-xs">"Updated"</div>
                                            {
                                                let updated_title = dep.updated_at.clone();
                                                let updated_display = dep.updated_at.clone();
                                                view! {
                                                    <div class="stat-value text-sm truncate" title=updated_title>
                                                        {updated_display}
                                                    </div>
                                                }
                                            }
                                        </div>
                                    </div>
                                </div>
                            }
                        })
                    }}

                    // Services section
                    <div class="mt-6">
                        <h4 class="font-semibold text-md mb-2">"Services"</h4>

                        // Loading spinner
                        {move || {
                            if view_services_loading.get() {
                                Some(view! {
                                    <div class="flex justify-center p-6">
                                        <span class="loading loading-spinner loading-md"></span>
                                        <span class="ml-2 text-base-content/70">"Loading services..."</span>
                                    </div>
                                })
                            } else {
                                None
                            }
                        }}

                        // Error message
                        {move || {
                            view_services_error.get().map(|msg| {
                                view! {
                                    <div class="alert alert-error mb-4">
                                        <span>{msg}</span>
                                    </div>
                                }
                            })
                        }}

                        // Services table
                        {move || {
                            let services = view_services.get();
                            let loading = view_services_loading.get();
                            if loading || services.is_empty() {
                                if !loading && view_services_error.get().is_none() {
                                    Some(view! {
                                        <div class="text-base-content/50 text-sm py-2">
                                            "No services found for this deployment."
                                        </div>
                                    }.into_any())
                                } else {
                                    None
                                }
                            } else {
                                Some(view! {
                                    <div class="overflow-x-auto">
                                        <table class="table table-zebra table-sm w-full">
                                            <thead>
                                                <tr>
                                                    <th>"Name"</th>
                                                    <th>"Status"</th>
                                                    <th>"Replicas"</th>
                                                    <th>"Actions"</th>
                                                </tr>
                                            </thead>
                                            <tbody>
                                                {services
                                                    .into_iter()
                                                    .map(|svc| {
                                                        let svc_status_class = service_status_class(&svc.status);
                                                        let svc_name_for_logs = svc.name.clone();
                                                        let dep_name_for_logs = svc.deployment.clone();
                                                        let svc_name_check = svc.name.clone();
                                                        view! {
                                                            <tr>
                                                                <td class="font-mono text-sm">{svc.name}</td>
                                                                <td>
                                                                    <span class=svc_status_class>{svc.status}</span>
                                                                </td>
                                                                <td>{format!("{}/{}", svc.replicas, svc.desired_replicas)}</td>
                                                                <td>
                                                                    <button
                                                                        class="btn btn-ghost btn-xs"
                                                                        on:click=move |_| {
                                                                            fetch_logs_action
                                                                                .dispatch((
                                                                                    dep_name_for_logs.clone(),
                                                                                    svc_name_for_logs.clone(),
                                                                                ));
                                                                        }
                                                                    >
                                                                        {move || {
                                                                            if logs_service_name.get().as_deref()
                                                                                == Some(svc_name_check.as_str())
                                                                            {
                                                                                "Hide Logs"
                                                                            } else {
                                                                                "View Logs"
                                                                            }
                                                                        }}
                                                                    </button>
                                                                </td>
                                                            </tr>
                                                        }
                                                    })
                                                    .collect::<Vec<_>>()}
                                            </tbody>
                                        </table>
                                    </div>
                                }.into_any())
                            }
                        }}
                    </div>

                    // Logs section (shown when a service's logs are requested)
                    {move || {
                        logs_service_name.get().map(|svc_name| {
                            view! {
                                <div class="mt-4">
                                    <h4 class="font-semibold text-md mb-2">
                                        "Logs: "
                                        <span class="font-mono">{svc_name}</span>
                                    </h4>
                                    {move || {
                                        if logs_loading.get() {
                                            view! {
                                                <div class="flex justify-center p-4">
                                                    <span class="loading loading-spinner loading-sm"></span>
                                                    <span class="ml-2 text-base-content/70 text-sm">
                                                        "Loading logs..."
                                                    </span>
                                                </div>
                                            }
                                                .into_any()
                                        } else {
                                            view! {
                                                <pre class="bg-base-300 rounded-lg p-4 overflow-auto max-h-64 text-xs font-mono whitespace-pre-wrap">
                                                    {move || logs_content.get()}
                                                </pre>
                                            }
                                                .into_any()
                                        }
                                    }}
                                </div>
                            }
                        })
                    }}

                    <div class="modal-action">
                        <button
                            class="btn"
                            on:click=move |_| {
                                set_show_view_modal.set(false);
                                set_view_deployment.set(None);
                                set_view_services.set(Vec::new());
                                set_logs_service_name.set(None);
                                set_logs_content.set(String::new());
                            }
                        >
                            "Close"
                        </button>
                    </div>
                </div>
                <div
                    class="modal-backdrop"
                    on:click=move |_| {
                        set_show_view_modal.set(false);
                        set_view_deployment.set(None);
                        set_view_services.set(Vec::new());
                        set_logs_service_name.set(None);
                        set_logs_content.set(String::new());
                    }
                ></div>
            </div>
        </div>
    }
}
