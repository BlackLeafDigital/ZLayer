//! `/groups` — admin-only user group management (master-detail layout).
//!
//! Left column: groups table with New / Edit / Delete actions. Right
//! column: the currently selected group's member list with an
//! "Add member" picker (populated from `manager_list_users`, minus the
//! users already in the group) and a Remove action per row.
//!
//! Admin-gated at the component level: non-admins see an `alert-error`
//! "Admin access required" banner. The page never attempts any server
//! call when the gate fails, so a non-admin session doesn't leak
//! group/user metadata. The backend still enforces admin-only on every
//! mutation independently.
//!
//! The daemon's member-list endpoint returns just `user_id` values, so
//! we enrich each row client-side by joining against
//! `manager_list_users`. If a listed user id can't be found in the users
//! list (e.g. the user was deleted mid-session) the id is shown verbatim
//! and flagged as `(unknown)`.

use leptos::ev::SubmitEvent;
use leptos::prelude::*;
use leptos::task::spawn_local;
use std::collections::HashSet;

use crate::app::auth_guard::CurrentUser;
use crate::app::server_fns::{
    manager_add_group_member, manager_create_group, manager_delete_group,
    manager_list_group_members, manager_list_groups, manager_list_users,
    manager_remove_group_member, manager_update_group, ManagerUserView,
};
use crate::wire::groups::WireUserGroup;

/// Admin-only groups page with a master-detail layout.
#[component]
#[allow(clippy::too_many_lines)] // view macro DSL + master-detail + multiple modals
pub fn Groups() -> impl IntoView {
    let current = use_context::<CurrentUser>();
    let is_admin = current.as_ref().is_some_and(CurrentUser::is_admin);

    if !is_admin {
        return view! {
            <div class="p-6">
                <div class="alert alert-error max-w-xl">
                    <div>
                        <h3 class="font-bold">"Admin access required"</h3>
                        <p class="text-sm opacity-80">
                            "Group management is restricted to administrators."
                        </p>
                    </div>
                </div>
            </div>
        }
        .into_any();
    }

    let groups = Resource::new(|| (), |()| async move { manager_list_groups().await });
    let users = Resource::new(|| (), |()| async move { manager_list_users().await });

    let (new_open, set_new_open) = signal(false);
    let (error, set_error) = signal(None::<String>);

    // Selection stored as the group id so it survives list refetches.
    let selected_group: RwSignal<Option<String>> = RwSignal::new(None);
    let edit_target = RwSignal::new(None::<WireUserGroup>);
    let delete_target = RwSignal::new(None::<WireUserGroup>);

    let refetch_groups = move || groups.refetch();

    view! {
        <div class="p-6 space-y-4">
            <div class="flex justify-between items-center">
                <h1 class="text-2xl font-bold">"Groups"</h1>
                <button
                    class="btn btn-primary btn-sm"
                    on:click=move |_| set_new_open.set(true)
                >
                    "New group"
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

            <div class="grid grid-cols-1 lg:grid-cols-3 gap-4">
                // Left column: groups list.
                <div class="lg:col-span-1 card bg-base-200">
                    <div class="card-body p-0 overflow-x-auto">
                        <Suspense fallback=move || {
                            view! {
                                <div class="p-6 flex justify-center">
                                    <span class="loading loading-spinner loading-md"></span>
                                </div>
                            }
                        }>
                            {move || {
                                groups.get().map(|res| match res {
                                    Err(e) => view! {
                                        <div class="p-4 text-error">
                                            {format!("Failed to load groups: {e}")}
                                        </div>
                                    }
                                    .into_any(),
                                    Ok(list) => render_groups_table(
                                        &list,
                                        selected_group,
                                        edit_target,
                                        delete_target,
                                    )
                                    .into_any(),
                                })
                            }}
                        </Suspense>
                    </div>
                </div>

                // Right column: selected group detail + members.
                <div class="lg:col-span-2">
                    <GroupDetail
                        selected=selected_group
                        groups=groups
                        users=users
                        set_error=set_error
                    />
                </div>
            </div>

            <NewGroupModal
                open=new_open
                set_open=set_new_open
                set_error=set_error
                on_success=move |id: String| {
                    selected_group.set(Some(id));
                    refetch_groups();
                }
            />

            <EditGroupModal
                target=edit_target
                set_error=set_error
                on_success=move || {
                    refetch_groups();
                }
            />

            <DeleteGroupModal
                target=delete_target
                selected=selected_group
                set_error=set_error
                on_success=move || {
                    refetch_groups();
                }
            />
        </div>
    }
    .into_any()
}

/// Render the groups list table. Clicking a row selects that group.
fn render_groups_table(
    list: &[WireUserGroup],
    selected_group: RwSignal<Option<String>>,
    edit_target: RwSignal<Option<WireUserGroup>>,
    delete_target: RwSignal<Option<WireUserGroup>>,
) -> impl IntoView {
    if list.is_empty() {
        return view! { <div class="p-4 opacity-70">"(no groups)"</div> }.into_any();
    }

    view! {
        <table class="table table-zebra table-sm">
            <thead>
                <tr>
                    <th>"Name"</th>
                    <th>"Description"</th>
                    <th class="text-right">"Actions"</th>
                </tr>
            </thead>
            <tbody>
                {list
                    .iter()
                    .map(|g| render_group_row(g, selected_group, edit_target, delete_target))
                    .collect_view()}
            </tbody>
        </table>
    }
    .into_any()
}

fn render_group_row(
    group: &WireUserGroup,
    selected_group: RwSignal<Option<String>>,
    edit_target: RwSignal<Option<WireUserGroup>>,
    delete_target: RwSignal<Option<WireUserGroup>>,
) -> impl IntoView {
    let select_id = group.id.clone();
    let row_id = group.id.clone();
    let edit_group = group.clone();
    let delete_group = group.clone();
    let name = group.name.clone();
    let description = group.description.clone().unwrap_or_else(|| "—".to_string());

    let row_class = move || {
        if selected_group.with(|s| s.as_deref().is_some_and(|id| id == row_id.as_str())) {
            "cursor-pointer bg-primary/10"
        } else {
            "cursor-pointer hover:bg-base-300"
        }
    };

    view! {
        <tr class=row_class on:click=move |_| selected_group.set(Some(select_id.clone()))>
            <td class="font-mono text-sm">{name}</td>
            <td class="text-xs opacity-70">{description}</td>
            <td class="text-right space-x-1">
                <button
                    class="btn btn-xs btn-ghost"
                    on:click=move |ev| {
                        ev.stop_propagation();
                        edit_target.set(Some(edit_group.clone()));
                    }
                >
                    "Edit"
                </button>
                <button
                    class="btn btn-xs btn-error"
                    on:click=move |ev| {
                        ev.stop_propagation();
                        delete_target.set(Some(delete_group.clone()));
                    }
                >
                    "Delete"
                </button>
            </td>
        </tr>
    }
}

/// Right-hand detail panel for the currently selected group.
///
/// Loads members via `manager_list_group_members`, joins them against
/// the users resource to render Name/Email/Actions, and offers an
/// "Add member" picker populated from users-minus-current-members.
#[component]
#[allow(clippy::too_many_lines)]
fn GroupDetail(
    selected: RwSignal<Option<String>>,
    groups: Resource<Result<Vec<WireUserGroup>, ServerFnError>>,
    users: Resource<Result<Vec<ManagerUserView>, ServerFnError>>,
    set_error: WriteSignal<Option<String>>,
) -> impl IntoView {
    // Member user_ids for the selected group. Refreshed on selection
    // change and on add/remove actions.
    let members: RwSignal<Vec<String>> = RwSignal::new(Vec::new());
    let (members_loading, set_members_loading) = signal(false);
    let (adding, set_adding) = signal(false);
    // The user id currently selected in the "add member" dropdown.
    let (add_pick, set_add_pick) = signal(String::new());

    let remove_target = RwSignal::new(None::<ManagerUserView>);

    // Re-fetch members when the selection flips.
    Effect::new(move |_| {
        let Some(id) = selected.get() else {
            members.set(Vec::new());
            return;
        };
        set_members_loading.set(true);
        let id_cloned = id.clone();
        spawn_local(async move {
            match manager_list_group_members(id_cloned).await {
                Ok(list) => members.set(list),
                Err(e) => {
                    members.set(Vec::new());
                    set_error.set(Some(format!("Load members failed: {e}")));
                }
            }
            set_members_loading.set(false);
        });
    });

    // Refresh the member list after mutations.
    let refresh_members = move || {
        let Some(id) = selected.get() else {
            return;
        };
        set_members_loading.set(true);
        spawn_local(async move {
            match manager_list_group_members(id).await {
                Ok(list) => members.set(list),
                Err(e) => set_error.set(Some(format!("Refresh members failed: {e}"))),
            }
            set_members_loading.set(false);
        });
    };

    // Add the currently picked user to the group.
    let on_add_member = move |ev: SubmitEvent| {
        ev.prevent_default();
        let Some(gid) = selected.get() else {
            return;
        };
        let uid = add_pick.get();
        if uid.trim().is_empty() {
            return;
        }
        if adding.get() {
            return;
        }
        set_adding.set(true);
        let gid_cloned = gid.clone();
        let uid_cloned = uid.clone();
        spawn_local(async move {
            let result = manager_add_group_member(gid_cloned, uid_cloned).await;
            set_adding.set(false);
            match result {
                Ok(()) => {
                    set_add_pick.set(String::new());
                    refresh_members();
                }
                Err(e) => set_error.set(Some(format!("Add member failed: {e}"))),
            }
        });
    };

    view! {
        {move || {
            let Some(group_id) = selected.get() else {
                return view! {
                    <div class="card bg-base-200">
                        <div class="card-body">
                            <p class="opacity-70">
                                "Select a group on the left to manage its members."
                            </p>
                        </div>
                    </div>
                }
                .into_any();
            };

            // Look up the group from the current snapshot; if absent, show
            // a loading placeholder (first paint race).
            let group = groups
                .get()
                .and_then(Result::ok)
                .and_then(|list| list.into_iter().find(|g| g.id == group_id));

            let Some(group) = group else {
                return view! {
                    <div class="card bg-base-200">
                        <div class="card-body">
                            <p class="opacity-70">"Loading group…"</p>
                        </div>
                    </div>
                }
                .into_any();
            };

            let group_name = group.name.clone();
            let description = group
                .description
                .clone()
                .unwrap_or_else(|| "(no description)".to_string());
            let created_at = group.created_at.clone();

            view! {
                <div class="space-y-4">
                    // Header
                    <div class="card bg-base-200">
                        <div class="card-body">
                            <h2 class="card-title font-mono">{group_name}</h2>
                            <p class="text-sm opacity-80">{description}</p>
                            <div class="text-xs opacity-60 mt-1">
                                {format!("Created {created_at}")}
                            </div>
                        </div>
                    </div>

                    // Add member picker
                    <div class="card bg-base-200">
                        <div class="card-body">
                            <h3 class="card-title text-base">"Add member"</h3>
                            {move || {
                                let users_opt = users.get();
                                match users_opt {
                                    None => view! {
                                        <div class="flex justify-center p-2">
                                            <span class="loading loading-spinner loading-sm"></span>
                                        </div>
                                    }
                                    .into_any(),
                                    Some(Err(e)) => view! {
                                        <div class="text-error text-sm">
                                            {format!("Failed to load users: {e}")}
                                        </div>
                                    }
                                    .into_any(),
                                    Some(Ok(all_users)) => {
                                        let current_members: HashSet<String> =
                                            members.get().into_iter().collect();
                                        let available: Vec<ManagerUserView> = all_users
                                            .into_iter()
                                            .filter(|u| !current_members.contains(&u.id))
                                            .collect();
                                        if available.is_empty() {
                                            return view! {
                                                <div class="text-xs opacity-70">
                                                    "All known users are already members."
                                                </div>
                                            }
                                            .into_any();
                                        }
                                        view! {
                                            <form
                                                on:submit=on_add_member
                                                class="flex flex-wrap items-end gap-2"
                                            >
                                                <select
                                                    class="select select-bordered select-sm flex-1 min-w-[12rem]"
                                                    required
                                                    on:change=move |ev| {
                                                        set_add_pick.set(event_target_value(&ev));
                                                    }
                                                >
                                                    <option value="" selected=move || add_pick.get().is_empty()>
                                                        "Select a user…"
                                                    </option>
                                                    {available
                                                        .into_iter()
                                                        .map(|u| {
                                                            let uid = u.id.clone();
                                                            let uid_for_selected = uid.clone();
                                                            let label = format!(
                                                                "{} <{}>",
                                                                u.display_name,
                                                                u.email,
                                                            );
                                                            view! {
                                                                <option
                                                                    value=uid
                                                                    selected=move || {
                                                                        add_pick.get() == uid_for_selected
                                                                    }
                                                                >
                                                                    {label}
                                                                </option>
                                                            }
                                                        })
                                                        .collect_view()}
                                                </select>
                                                <button
                                                    type="submit"
                                                    class="btn btn-primary btn-sm"
                                                    disabled=move || {
                                                        adding.get() || add_pick.get().trim().is_empty()
                                                    }
                                                >
                                                    {move || if adding.get() {
                                                        "Adding…"
                                                    } else {
                                                        "Add"
                                                    }}
                                                </button>
                                            </form>
                                        }
                                        .into_any()
                                    }
                                }
                            }}
                        </div>
                    </div>

                    // Members list
                    <div class="card bg-base-200">
                        <div class="card-body">
                            <h3 class="card-title text-base">"Members"</h3>
                            {move || {
                                if members_loading.get() {
                                    return view! {
                                        <div class="p-4 flex justify-center">
                                            <span class="loading loading-spinner loading-sm"></span>
                                        </div>
                                    }
                                    .into_any();
                                }
                                let ids = members.get();
                                if ids.is_empty() {
                                    return view! {
                                        <div class="p-2 opacity-70 text-sm">
                                            "(no members yet)"
                                        </div>
                                    }
                                    .into_any();
                                }
                                let all_users = users
                                    .get()
                                    .and_then(Result::ok)
                                    .unwrap_or_default();
                                render_members_table(&ids, &all_users, remove_target).into_any()
                            }}
                        </div>
                    </div>
                </div>
            }
            .into_any()
        }}

        <RemoveMemberModal
            target=remove_target
            selected=selected
            set_error=set_error
            on_success=move || {
                refresh_members();
            }
        />
    }
}

fn render_members_table(
    ids: &[String],
    all_users: &[ManagerUserView],
    remove_target: RwSignal<Option<ManagerUserView>>,
) -> impl IntoView {
    // Build a detached Vec of rows so we can collect_view without holding
    // references across a spawn_local.
    let rows: Vec<ManagerUserView> = ids
        .iter()
        .map(|id| {
            all_users
                .iter()
                .find(|u| &u.id == id)
                .cloned()
                .unwrap_or_else(|| ManagerUserView {
                    id: id.clone(),
                    email: "(unknown)".to_string(),
                    display_name: "(unknown user)".to_string(),
                    role: String::new(),
                    is_active: false,
                    last_login_at: None,
                })
        })
        .collect();

    view! {
        <div class="overflow-x-auto">
            <table class="table table-sm">
                <thead>
                    <tr>
                        <th>"User"</th>
                        <th>"Email"</th>
                        <th class="text-right">"Actions"</th>
                    </tr>
                </thead>
                <tbody>
                    {rows.iter().map(|u| render_member_row(u, remove_target)).collect_view()}
                </tbody>
            </table>
        </div>
    }
}

fn render_member_row(
    user: &ManagerUserView,
    remove_target: RwSignal<Option<ManagerUserView>>,
) -> impl IntoView {
    let display_name = user.display_name.clone();
    let email = user.email.clone();
    let row_user = user.clone();

    view! {
        <tr>
            <td>{display_name}</td>
            <td class="font-mono text-xs">{email}</td>
            <td class="text-right">
                <button
                    class="btn btn-xs btn-error"
                    on:click=move |_| remove_target.set(Some(row_user.clone()))
                >
                    "Remove"
                </button>
            </td>
        </tr>
    }
}

#[component]
fn NewGroupModal(
    open: ReadSignal<bool>,
    set_open: WriteSignal<bool>,
    set_error: WriteSignal<Option<String>>,
    on_success: impl Fn(String) + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let (name, set_name) = signal(String::new());
    let (description, set_description) = signal(String::new());
    let (submitting, set_submitting) = signal(false);

    let reset = move || {
        set_name.set(String::new());
        set_description.set(String::new());
    };

    let on_submit = move |ev: SubmitEvent| {
        ev.prevent_default();
        if submitting.get() {
            return;
        }
        let name_val = name.get().trim().to_string();
        if name_val.is_empty() {
            set_error.set(Some("Group name is required.".to_string()));
            return;
        }
        let desc_val = description.get().trim().to_string();
        let desc_opt = if desc_val.is_empty() {
            None
        } else {
            Some(desc_val)
        };

        set_submitting.set(true);
        spawn_local(async move {
            let result = manager_create_group(name_val, desc_opt).await;
            set_submitting.set(false);
            match result {
                Ok(group) => {
                    reset();
                    set_open.set(false);
                    on_success(group.id);
                }
                Err(e) => set_error.set(Some(format!("Create failed: {e}"))),
            }
        });
    };

    view! {
        <dialog class=move || if open.get() { "modal modal-open" } else { "modal" }>
            <div class="modal-box">
                <h3 class="font-bold text-lg mb-3">"New group"</h3>
                <form on:submit=on_submit class="space-y-3">
                    <div class="form-control">
                        <label class="label">
                            <span class="label-text">"Name"</span>
                        </label>
                        <input
                            type="text"
                            class="input input-bordered w-full font-mono"
                            required
                            placeholder="developers"
                            prop:value=name
                            on:input=move |ev| set_name.set(event_target_value(&ev))
                        />
                    </div>
                    <div class="form-control">
                        <label class="label">
                            <span class="label-text">
                                "Description "
                                <span class="opacity-60">"(optional)"</span>
                            </span>
                        </label>
                        <input
                            type="text"
                            class="input input-bordered w-full"
                            placeholder="Team description"
                            prop:value=description
                            on:input=move |ev| set_description.set(event_target_value(&ev))
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
fn EditGroupModal(
    target: RwSignal<Option<WireUserGroup>>,
    set_error: WriteSignal<Option<String>>,
    on_success: impl Fn() + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let (name, set_name) = signal(String::new());
    let (description, set_description) = signal(String::new());
    let (submitting, set_submitting) = signal(false);

    // Seed the form whenever a new target is selected.
    Effect::new(move |_| {
        if let Some(g) = target.get() {
            set_name.set(g.name);
            set_description.set(g.description.unwrap_or_default());
        }
    });

    let close = move || target.set(None);

    let on_submit = move |ev: SubmitEvent| {
        ev.prevent_default();
        let Some(group) = target.get() else {
            return;
        };
        if submitting.get() {
            return;
        }
        let name_val = name.get().trim().to_string();
        if name_val.is_empty() {
            set_error.set(Some("Group name is required.".to_string()));
            return;
        }
        let desc_val = description.get().trim().to_string();

        // Only send fields that changed (keeps the PATCH minimal).
        let name_patch = if name_val == group.name {
            None
        } else {
            Some(name_val)
        };
        let existing_desc = group.description.clone().unwrap_or_default();
        let desc_patch = if desc_val == existing_desc {
            None
        } else {
            Some(desc_val)
        };

        if name_patch.is_none() && desc_patch.is_none() {
            // No-op edit — just close.
            close();
            return;
        }

        let id = group.id.clone();
        set_submitting.set(true);
        spawn_local(async move {
            let result = manager_update_group(id, name_patch, desc_patch).await;
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
                <h3 class="font-bold text-lg mb-3">"Edit group"</h3>
                <form on:submit=on_submit class="space-y-3">
                    <div class="form-control">
                        <label class="label">
                            <span class="label-text">"Name"</span>
                        </label>
                        <input
                            type="text"
                            class="input input-bordered w-full font-mono"
                            required
                            prop:value=name
                            on:input=move |ev| set_name.set(event_target_value(&ev))
                        />
                    </div>
                    <div class="form-control">
                        <label class="label">
                            <span class="label-text">"Description"</span>
                        </label>
                        <input
                            type="text"
                            class="input input-bordered w-full"
                            prop:value=description
                            on:input=move |ev| set_description.set(event_target_value(&ev))
                        />
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
fn DeleteGroupModal(
    target: RwSignal<Option<WireUserGroup>>,
    selected: RwSignal<Option<String>>,
    set_error: WriteSignal<Option<String>>,
    on_success: impl Fn() + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let (submitting, set_submitting) = signal(false);

    let close = move || target.set(None);

    let confirm = move |_| {
        let Some(group) = target.get() else {
            return;
        };
        if submitting.get() {
            return;
        }
        set_submitting.set(true);
        spawn_local(async move {
            let result = manager_delete_group(group.id.clone()).await;
            set_submitting.set(false);
            match result {
                Ok(()) => {
                    if selected.with(|s| s.as_deref() == Some(group.id.as_str())) {
                        selected.set(None);
                    }
                    target.set(None);
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
                <h3 class="font-bold text-lg mb-3">
                    {move || {
                        target.get().map_or_else(
                            || "Delete group".to_string(),
                            |g| format!("Delete group '{}'", g.name),
                        )
                    }}
                </h3>
                <p class="text-sm opacity-80">
                    {move || {
                        target
                            .get()
                            .map(|g| {
                                format!(
                                    "Delete group '{}'? All membership rows will be removed. This cannot be undone.",
                                    g.name,
                                )
                            })
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

#[component]
fn RemoveMemberModal(
    target: RwSignal<Option<ManagerUserView>>,
    selected: RwSignal<Option<String>>,
    set_error: WriteSignal<Option<String>>,
    on_success: impl Fn() + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let (submitting, set_submitting) = signal(false);

    let close = move || target.set(None);

    let confirm = move |_| {
        let Some(user) = target.get() else {
            return;
        };
        let Some(group_id) = selected.get() else {
            return;
        };
        if submitting.get() {
            return;
        }
        set_submitting.set(true);
        let uid = user.id.clone();
        spawn_local(async move {
            let result = manager_remove_group_member(group_id, uid).await;
            set_submitting.set(false);
            match result {
                Ok(()) => {
                    target.set(None);
                    on_success();
                }
                Err(e) => set_error.set(Some(format!("Remove failed: {e}"))),
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
                        target.get().map_or_else(
                            || "Remove member".to_string(),
                            |u| format!("Remove {}", u.display_name),
                        )
                    }}
                </h3>
                <p class="text-sm opacity-80">
                    {move || {
                        target
                            .get()
                            .map(|u| {
                                format!(
                                    "Remove {} <{}> from this group?",
                                    u.display_name,
                                    u.email,
                                )
                            })
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
                        {move || if submitting.get() { "Removing…" } else { "Remove" }}
                    </button>
                </div>
            </div>
        </dialog>
    }
}
