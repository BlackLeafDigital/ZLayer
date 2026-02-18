//! SSH Tunnels page
//!
//! Displays and manages SSH tunnels via the ZLayer tunnel API.
//! Supports creating, listing, and deleting tunnels.

use leptos::prelude::*;

use crate::app::server_fns::{create_tunnel, delete_tunnel, get_tunnels, TunnelInfo};

/// Format a Unix timestamp for display
fn format_unix_timestamp(ts: u64) -> String {
    let secs_per_day = 86400u64;
    let days_since_epoch = ts / secs_per_day;
    let years = 1970 + (days_since_epoch / 365);
    let day_of_year = days_since_epoch % 365;
    let month = (day_of_year / 30) + 1;
    let day = (day_of_year % 30) + 1;
    format!("{}-{:02}-{:02}", years, month.min(12), day.min(28))
}

/// Render tunnels table from data
fn render_tunnels_table(
    tunnels: Vec<TunnelInfo>,
    on_delete: impl Fn(String) + 'static + Clone,
) -> impl IntoView {
    view! {
        <div class="overflow-x-auto">
            <table class="table table-zebra w-full">
                <thead>
                    <tr>
                        <th>"Name"</th>
                        <th>"Status"</th>
                        <th>"Services"</th>
                        <th>"Created"</th>
                        <th>"Expires"</th>
                        <th>"Actions"</th>
                    </tr>
                </thead>
                <tbody>
                    {tunnels
                        .into_iter()
                        .map(|tunnel| {
                            let status_class = match tunnel.status.as_str() {
                                "active" => "badge badge-success",
                                "pending" => "badge badge-warning",
                                "expired" => "badge badge-error",
                                _ => "badge badge-ghost",
                            };
                            let services_display = if tunnel.services.is_empty() {
                                "All".to_string()
                            } else {
                                tunnel.services.join(", ")
                            };
                            let tunnel_id = tunnel.id.clone();
                            let on_delete = on_delete.clone();
                            view! {
                                <tr>
                                    <td class="font-medium">{tunnel.name}</td>
                                    <td>
                                        <span class=status_class>{tunnel.status}</span>
                                    </td>
                                    <td class="text-sm">{services_display}</td>
                                    <td>{format_unix_timestamp(tunnel.created_at)}</td>
                                    <td>{format_unix_timestamp(tunnel.expires_at)}</td>
                                    <td>
                                        <button
                                            class="btn btn-sm btn-ghost text-error"
                                            on:click=move |_| on_delete(tunnel_id.clone())
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

/// SSH Tunnels page component
#[component]
pub fn SshTunnels() -> impl IntoView {
    let (refresh, set_refresh) = signal(0u32);
    let tunnels = Resource::new(move || refresh.get(), |_| get_tunnels());

    // Modal states
    let (show_create_modal, set_show_create_modal) = signal(false);
    let (show_delete_modal, set_show_delete_modal) = signal(false);
    let (delete_target, set_delete_target) = signal(String::new());
    let (error_msg, set_error_msg) = signal(Option::<String>::None);

    // Form fields
    let (tunnel_name, set_tunnel_name) = signal(String::new());
    let (tunnel_services, set_tunnel_services) = signal(String::new());
    let (tunnel_ttl_hours, set_tunnel_ttl_hours) = signal("24".to_string());

    // Create tunnel action
    let create_action = Action::new(move |input: &(String, String, u64)| {
        let (name, services, ttl_hours) = input.clone();
        async move {
            let result = create_tunnel(name, services, ttl_hours).await;
            match result {
                Ok(_) => {
                    set_show_create_modal.set(false);
                    set_tunnel_name.set(String::new());
                    set_tunnel_services.set(String::new());
                    set_tunnel_ttl_hours.set("24".to_string());
                    set_error_msg.set(None);
                    set_refresh.update(|n| *n += 1);
                }
                Err(e) => {
                    set_error_msg.set(Some(e.to_string()));
                }
            }
        }
    });

    // Delete tunnel action
    let delete_action = Action::new(move |id: &String| {
        let id = id.clone();
        async move {
            let result = delete_tunnel(id).await;
            if result.is_ok() {
                set_show_delete_modal.set(false);
                set_delete_target.set(String::new());
                set_refresh.update(|n| *n += 1);
            }
        }
    });

    view! {
        <div class="container mx-auto p-6">
            <div class="flex justify-between items-center mb-6">
                <h1 class="text-3xl font-bold">"SSH Tunnels"</h1>
                <button
                    class="btn btn-primary"
                    on:click=move |_| {
                        set_error_msg.set(None);
                        set_tunnel_name.set(String::new());
                        set_tunnel_services.set(String::new());
                        set_tunnel_ttl_hours.set("24".to_string());
                        set_show_create_modal.set(true);
                    }
                >
                    "Create Tunnel"
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
                        tunnels
                            .get()
                            .map(|result| {
                                match result {
                                    Ok(tunnel_list) => {
                                        if tunnel_list.is_empty() {
                                            view! {
                                                <div class="alert alert-info">
                                                    <span>
                                                        "No tunnels configured. Create your first tunnel."
                                                    </span>
                                                </div>
                                            }
                                                .into_any()
                                        } else {
                                            let on_delete = move |id: String| {
                                                set_delete_target.set(id);
                                                set_show_delete_modal.set(true);
                                            };
                                            render_tunnels_table(tunnel_list, on_delete).into_any()
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

            // Create Tunnel Modal
            <div class=move || {
                if show_create_modal.get() { "modal modal-open" } else { "modal" }
            }>
                <div class="modal-box">
                    <h3 class="font-bold text-lg">"Create Tunnel"</h3>

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
                            <span class="label-text">"Tunnel Name" <span class="text-error">"*"</span></span>
                        </label>
                        <input
                            type="text"
                            class="input input-bordered w-full"
                            placeholder="e.g. database-tunnel"
                            prop:value=move || tunnel_name.get()
                            on:input=move |ev| set_tunnel_name.set(event_target_value(&ev))
                        />
                    </div>

                    <div class="form-control w-full mt-2">
                        <label class="label">
                            <span class="label-text">"Allowed Services"</span>
                        </label>
                        <input
                            type="text"
                            class="input input-bordered w-full"
                            placeholder="e.g. ssh, postgres (leave empty for all)"
                            prop:value=move || tunnel_services.get()
                            on:input=move |ev| set_tunnel_services.set(event_target_value(&ev))
                        />
                        <label class="label">
                            <span class="label-text-alt text-base-content/50">
                                "Comma-separated list of services. Leave empty to allow all."
                            </span>
                        </label>
                    </div>

                    <div class="form-control w-full mt-2">
                        <label class="label">
                            <span class="label-text">"TTL (hours)"</span>
                        </label>
                        <input
                            type="number"
                            class="input input-bordered w-full"
                            min="1"
                            max="8760"
                            prop:value=move || tunnel_ttl_hours.get()
                            on:input=move |ev| set_tunnel_ttl_hours.set(event_target_value(&ev))
                        />
                        <label class="label">
                            <span class="label-text-alt text-base-content/50">
                                "How long the tunnel token is valid (default: 24 hours)"
                            </span>
                        </label>
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
                            prop:disabled=move || tunnel_name.get().trim().is_empty()
                            on:click=move |_| {
                                let ttl: u64 = tunnel_ttl_hours
                                    .get()
                                    .parse()
                                    .unwrap_or(24);
                                create_action
                                    .dispatch((
                                        tunnel_name.get(),
                                        tunnel_services.get(),
                                        ttl,
                                    ));
                            }
                        >
                            "Create"
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
                    <h3 class="font-bold text-lg text-error">"Delete Tunnel"</h3>
                    <p class="py-4">
                        "Are you sure you want to delete this tunnel? The tunnel token will be revoked immediately."
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
        </div>
    }
}
