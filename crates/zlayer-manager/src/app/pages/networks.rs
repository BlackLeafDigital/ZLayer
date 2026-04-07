//! Networks management page
//!
//! Displays and manages network access-control policies via the ZLayer API.
//! Supports listing, viewing details, creating, and deleting networks.

use leptos::prelude::*;

use crate::app::server_fns::{
    create_network, delete_network, get_network, get_networks, NetworkAccessRuleItem,
    NetworkDetail, NetworkInfo, NetworkMemberItem,
};

/// Stats row showing aggregate network metrics
#[component]
fn NetworkStats(networks: Vec<NetworkInfo>) -> impl IntoView {
    let total_networks = networks.len();
    let total_members: usize = networks.iter().map(|n| n.member_count).sum();
    let total_rules: usize = networks.iter().map(|n| n.rule_count).sum();
    let active_policies = networks.iter().filter(|n| n.rule_count > 0).count();

    view! {
        <div class="stats shadow w-full mb-6">
            <div class="stat">
                <div class="stat-title">"Total Networks"</div>
                <div class="stat-value text-primary">{total_networks}</div>
            </div>
            <div class="stat">
                <div class="stat-title">"Total Members"</div>
                <div class="stat-value text-info">{total_members}</div>
            </div>
            <div class="stat">
                <div class="stat-title">"Total Rules"</div>
                <div class="stat-value text-secondary">{total_rules}</div>
            </div>
            <div class="stat">
                <div class="stat-title">"Active Policies"</div>
                <div class="stat-value text-success">{active_policies}</div>
            </div>
        </div>
    }
}

/// Render CIDR badges
fn render_cidr_badges(cidrs: &[String]) -> impl IntoView {
    cidrs
        .iter()
        .map(|cidr| {
            let cidr = cidr.clone();
            view! {
                <span class="badge badge-outline badge-sm font-mono mr-1 mb-1">{cidr}</span>
            }
        })
        .collect::<Vec<_>>()
}

/// Render members table in the detail modal
fn render_members_table(members: Vec<NetworkMemberItem>) -> impl IntoView {
    if members.is_empty() {
        return view! {
            <div class="text-sm opacity-50 italic">"No members defined."</div>
        }
        .into_any();
    }

    view! {
        <div class="overflow-x-auto">
            <table class="table table-zebra table-sm w-full">
                <thead>
                    <tr>
                        <th>"Name"</th>
                        <th>"Kind"</th>
                    </tr>
                </thead>
                <tbody>
                    {members.into_iter().map(|m| {
                        let kind_class = match m.kind.as_str() {
                            "user" => "badge badge-info badge-sm",
                            "group" => "badge badge-accent badge-sm",
                            "node" => "badge badge-warning badge-sm",
                            "cidr" => "badge badge-outline badge-sm",
                            _ => "badge badge-ghost badge-sm",
                        };
                        view! {
                            <tr>
                                <td class="font-medium">{m.name}</td>
                                <td><span class=kind_class>{m.kind}</span></td>
                            </tr>
                        }
                    }).collect::<Vec<_>>()}
                </tbody>
            </table>
        </div>
    }
    .into_any()
}

/// Render access rules table in the detail modal
fn render_access_rules_table(rules: Vec<NetworkAccessRuleItem>) -> impl IntoView {
    if rules.is_empty() {
        return view! {
            <div class="text-sm opacity-50 italic">"No access rules defined."</div>
        }
        .into_any();
    }

    view! {
        <div class="overflow-x-auto">
            <table class="table table-zebra table-sm w-full">
                <thead>
                    <tr>
                        <th>"Service"</th>
                        <th>"Deployment"</th>
                        <th>"Ports"</th>
                        <th>"Action"</th>
                    </tr>
                </thead>
                <tbody>
                    {rules.into_iter().map(|r| {
                        let action_class = match r.action.as_str() {
                            "allow" => "badge badge-success badge-sm",
                            "deny" => "badge badge-error badge-sm",
                            _ => "badge badge-ghost badge-sm",
                        };
                        view! {
                            <tr>
                                <td class="font-mono">{r.service}</td>
                                <td class="font-mono">{r.deployment}</td>
                                <td class="font-mono text-sm">{r.ports}</td>
                                <td><span class=action_class>{r.action}</span></td>
                            </tr>
                        }
                    }).collect::<Vec<_>>()}
                </tbody>
            </table>
        </div>
    }
    .into_any()
}

/// Detail modal content for a network
#[component]
fn NetworkDetailContent(detail: NetworkDetail) -> impl IntoView {
    let cidrs = detail.cidrs.clone();
    let members = detail.members;
    let rules = detail.access_rules;

    view! {
        <div>
            // Description
            {detail.description.map(|desc| view! {
                <p class="text-sm opacity-70 mb-4">{desc}</p>
            })}

            // CIDRs section
            <div class="mb-4">
                <h4 class="font-semibold mb-2">"CIDRs"</h4>
                {if cidrs.is_empty() {
                    view! {
                        <div class="text-sm opacity-50 italic">"No CIDRs defined."</div>
                    }.into_any()
                } else {
                    view! {
                        <div class="flex flex-wrap gap-1">
                            {render_cidr_badges(&cidrs)}
                        </div>
                    }.into_any()
                }}
            </div>

            <div class="divider"></div>

            // Members section
            <div class="mb-4">
                <h4 class="font-semibold mb-2">"Members"</h4>
                {render_members_table(members)}
            </div>

            <div class="divider"></div>

            // Access Rules section
            <div>
                <h4 class="font-semibold mb-2">"Access Rules"</h4>
                {render_access_rules_table(rules)}
            </div>
        </div>
    }
}

/// Render the networks table
fn render_networks_table(
    networks: Vec<NetworkInfo>,
    on_view: impl Fn(String) + 'static + Clone,
    on_delete: impl Fn(String) + 'static + Clone,
) -> impl IntoView {
    view! {
        <div class="overflow-x-auto">
            <table class="table table-zebra w-full">
                <thead>
                    <tr>
                        <th>"Name"</th>
                        <th>"Description"</th>
                        <th>"CIDRs"</th>
                        <th>"Members"</th>
                        <th>"Rules"</th>
                        <th>"Actions"</th>
                    </tr>
                </thead>
                <tbody>
                    {networks.into_iter().map(|net| {
                        let view_name = net.name.clone();
                        let delete_name = net.name.clone();
                        let on_view = on_view.clone();
                        let on_delete = on_delete.clone();
                        let desc = net.description.clone().unwrap_or_else(|| "-".to_string());
                        view! {
                            <tr>
                                <td class="font-medium">{net.name}</td>
                                <td class="text-sm opacity-70">{desc}</td>
                                <td>{net.cidr_count}</td>
                                <td>{net.member_count}</td>
                                <td>{net.rule_count}</td>
                                <td>
                                    <div class="flex gap-1">
                                        <button
                                            class="btn btn-sm btn-ghost text-info"
                                            on:click=move |_| on_view(view_name.clone())
                                        >
                                            "View"
                                        </button>
                                        <button
                                            class="btn btn-sm btn-ghost text-error"
                                            on:click=move |_| on_delete(delete_name.clone())
                                        >
                                            "Delete"
                                        </button>
                                    </div>
                                </td>
                            </tr>
                        }
                    }).collect::<Vec<_>>()}
                </tbody>
            </table>
        </div>
    }
}

/// Networks management page component
#[component]
#[allow(clippy::too_many_lines)]
pub fn Networks() -> impl IntoView {
    let (refresh, set_refresh) = signal(0u32);
    let networks = Resource::new(move || refresh.get(), |_| get_networks());

    // Modal states
    let (show_create_modal, set_show_create_modal) = signal(false);
    let (show_delete_modal, set_show_delete_modal) = signal(false);
    let (show_detail_modal, set_show_detail_modal) = signal(false);
    let (delete_target, set_delete_target) = signal(String::new());
    let (error_msg, set_error_msg) = signal(Option::<String>::None);
    let (detail_data, set_detail_data) = signal(Option::<NetworkDetail>::None);
    let (detail_loading, set_detail_loading) = signal(false);

    // Form fields
    let (net_name, set_net_name) = signal(String::new());
    let (net_description, set_net_description) = signal(String::new());
    let (net_cidrs, set_net_cidrs) = signal(String::new());

    // Create network action
    let create_action = Action::new(move |input: &(String, String, String)| {
        let (name, description, cidrs) = input.clone();
        async move {
            let result = create_network(name, description, cidrs).await;
            match result {
                Ok(_) => {
                    set_show_create_modal.set(false);
                    set_net_name.set(String::new());
                    set_net_description.set(String::new());
                    set_net_cidrs.set(String::new());
                    set_error_msg.set(None);
                    set_refresh.update(|n| *n += 1);
                }
                Err(e) => {
                    set_error_msg.set(Some(e.to_string()));
                }
            }
        }
    });

    // Delete network action
    let delete_action = Action::new(move |name: &String| {
        let name = name.clone();
        async move {
            let result = delete_network(name).await;
            if result.is_ok() {
                set_show_delete_modal.set(false);
                set_delete_target.set(String::new());
                set_refresh.update(|n| *n += 1);
            }
        }
    });

    // View network detail action
    let view_action = Action::new(move |name: &String| {
        let name = name.clone();
        async move {
            set_detail_loading.set(true);
            set_show_detail_modal.set(true);
            let result = get_network(name).await;
            match result {
                Ok(detail) => {
                    set_detail_data.set(Some(detail));
                }
                Err(e) => {
                    set_error_msg.set(Some(e.to_string()));
                    set_show_detail_modal.set(false);
                }
            }
            set_detail_loading.set(false);
        }
    });

    view! {
        <div class="container mx-auto p-6">
            <div class="flex justify-between items-center mb-6">
                <h1 class="text-3xl font-bold">"Networks"</h1>
                <button
                    class="btn btn-primary"
                    on:click=move |_| {
                        set_error_msg.set(None);
                        set_net_name.set(String::new());
                        set_net_description.set(String::new());
                        set_net_cidrs.set(String::new());
                        set_show_create_modal.set(true);
                    }
                >
                    "Create Network"
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
                        networks
                            .get()
                            .map(|result| {
                                match result {
                                    Ok(network_list) => {
                                        if network_list.is_empty() {
                                            view! {
                                                <div class="alert alert-info">
                                                    <span>"No networks configured. Create your first network."</span>
                                                </div>
                                            }.into_any()
                                        } else {
                                            let stats_list = network_list.clone();
                                            let on_view = move |name: String| {
                                                view_action.dispatch(name);
                                            };
                                            let on_delete = move |name: String| {
                                                set_delete_target.set(name);
                                                set_show_delete_modal.set(true);
                                            };
                                            view! {
                                                <NetworkStats networks=stats_list />
                                                {render_networks_table(network_list, on_view, on_delete)}
                                            }.into_any()
                                        }
                                    }
                                    Err(e) => {
                                        view! {
                                            <div class="alert alert-error">{e.to_string()}</div>
                                        }.into_any()
                                    }
                                }
                            })
                    }}
                </ErrorBoundary>
            </Suspense>

            // Create Network Modal
            <div class=move || {
                if show_create_modal.get() { "modal modal-open" } else { "modal" }
            }>
                <div class="modal-box">
                    <h3 class="font-bold text-lg">"Create Network"</h3>

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
                            <span class="label-text">"Network Name" <span class="text-error">"*"</span></span>
                        </label>
                        <input
                            type="text"
                            class="input input-bordered w-full"
                            placeholder="e.g. corp-vpn"
                            prop:value=move || net_name.get()
                            on:input=move |ev| set_net_name.set(event_target_value(&ev))
                        />
                    </div>

                    <div class="form-control w-full mt-2">
                        <label class="label">
                            <span class="label-text">"Description"</span>
                        </label>
                        <textarea
                            class="textarea textarea-bordered w-full"
                            placeholder="e.g. Corporate VPN network"
                            rows="2"
                            prop:value=move || net_description.get()
                            on:input=move |ev| set_net_description.set(event_target_value(&ev))
                        ></textarea>
                    </div>

                    <div class="form-control w-full mt-2">
                        <label class="label">
                            <span class="label-text">"CIDRs"</span>
                        </label>
                        <textarea
                            class="textarea textarea-bordered w-full font-mono"
                            placeholder="e.g. 10.200.0.0/16, 192.168.1.0/24"
                            rows="3"
                            prop:value=move || net_cidrs.get()
                            on:input=move |ev| set_net_cidrs.set(event_target_value(&ev))
                        ></textarea>
                        <label class="label">
                            <span class="label-text-alt text-base-content/50">
                                "Comma-separated or one per line."
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
                            prop:disabled=move || net_name.get().trim().is_empty()
                            on:click=move |_| {
                                create_action.dispatch((
                                    net_name.get(),
                                    net_description.get(),
                                    net_cidrs.get(),
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

            // Detail Modal
            <div class=move || {
                if show_detail_modal.get() { "modal modal-open" } else { "modal" }
            }>
                <div class="modal-box max-w-3xl">
                    {move || {
                        if detail_loading.get() {
                            view! {
                                <div class="flex justify-center p-8">
                                    <span class="loading loading-spinner loading-lg"></span>
                                </div>
                            }.into_any()
                        } else if let Some(detail) = detail_data.get() {
                            let title = detail.name.clone();
                            view! {
                                <h3 class="font-bold text-lg mb-4">{title}</h3>
                                <NetworkDetailContent detail=detail />
                                <div class="modal-action">
                                    <button
                                        class="btn"
                                        on:click=move |_| {
                                            set_show_detail_modal.set(false);
                                            set_detail_data.set(None);
                                        }
                                    >
                                        "Close"
                                    </button>
                                </div>
                            }.into_any()
                        } else {
                            view! {
                                <div class="alert alert-warning">"No data available."</div>
                                <div class="modal-action">
                                    <button
                                        class="btn"
                                        on:click=move |_| set_show_detail_modal.set(false)
                                    >
                                        "Close"
                                    </button>
                                </div>
                            }.into_any()
                        }
                    }}
                </div>
                <div
                    class="modal-backdrop"
                    on:click=move |_| {
                        set_show_detail_modal.set(false);
                        set_detail_data.set(None);
                    }
                ></div>
            </div>

            // Delete Confirmation Modal
            <div class=move || {
                if show_delete_modal.get() { "modal modal-open" } else { "modal" }
            }>
                <div class="modal-box">
                    <h3 class="font-bold text-lg text-error">"Delete Network"</h3>
                    <p class="py-4">
                        "Are you sure you want to delete this network? All members and access rules will be removed."
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
