//! `/permissions` — admin-only permission grants management.
//!
//! Flat list (not a matrix) with three filters — subject kind, subject id,
//! resource kind — plus a Grant modal with cascading user/group pickers and
//! a Revoke confirm. The daemon's filter surface is narrow (exactly one of
//! `?user=<id>` or `?group=<id>`), so the page fetches the union of all
//! per-user and per-group lists on mount and filters client-side when the
//! filter bar changes.
//!
//! Design rationale: a full subject×resource matrix UI doesn't scale past
//! a handful of rows. A flat filtered list + targeted "Grant new
//! permission" modal is the shape that works for real deployments.
//!
//! Admin gate: the page component short-circuits to an access-denied
//! banner when `CurrentUser::is_admin()` is false, matching the Users
//! page pattern. The daemon additionally enforces admin-only on every
//! mutation (see `grant_permission` / `revoke_permission` in
//! `crates/zlayer-api/src/handlers/permissions.rs`).

use leptos::ev::SubmitEvent;
use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::app::auth_guard::CurrentUser;
use crate::app::server_fns::{
    manager_grant_permission, manager_list_groups, manager_list_permissions_for_subject,
    manager_list_users, manager_revoke_permission, ManagerUserView,
};
use crate::app::util::errors::format_server_error;
use crate::wire::groups::WireUserGroup;
use crate::wire::permissions::{WireGrantPermissionRequest, WirePermission};

/// Subject kind filter — the wire values are `"user"` / `"group"`, with a
/// virtual "all" option in the UI for "don't narrow by kind".
const SUBJECT_KIND_ALL: &str = "all";
const SUBJECT_KIND_USER: &str = "user";
const SUBJECT_KIND_GROUP: &str = "group";

/// Common resource kinds the daemon exposes today. These match the
/// `resource_kind` strings used by `PermissionStorage::check` across the
/// codebase (`deployment`, `project`, `secret`, `task`, `workflow`,
/// `notifier`, `variable`, `environment`). "all" is the virtual
/// don't-filter option and is not sent on grant.
const RESOURCE_KINDS: &[&str] = &[
    "deployment",
    "project",
    "secret",
    "task",
    "workflow",
    "notifier",
    "variable",
    "environment",
];

/// Permission levels accepted by the daemon's `PermissionLevel` enum in
/// snake_case form. `"none"` is valid schema-wise but meaningless as a
/// grant, so the form omits it.
const PERMISSION_LEVELS: &[&str] = &["read", "execute", "write"];

/// Admin-only permissions page.
#[component]
#[allow(clippy::too_many_lines)] // view macro DSL + filter bar + table + modals
pub fn Permissions() -> impl IntoView {
    let current = use_context::<CurrentUser>();
    let is_admin = current.as_ref().is_some_and(CurrentUser::is_admin);

    if !is_admin {
        return view! {
            <div class="p-6">
                <div class="alert alert-warning max-w-xl">
                    <div>
                        <h3 class="font-bold">"Access denied"</h3>
                        <p class="text-sm opacity-80">
                            "Admin role required to manage permissions."
                        </p>
                    </div>
                </div>
            </div>
        }
        .into_any();
    }

    // Users + groups feed the grant modal's pickers AND the display-name
    // column of the main table. A single Resource for each.
    let users = Resource::new(|| (), |()| async move { manager_list_users().await });
    let groups = Resource::new(|| (), |()| async move { manager_list_groups().await });

    // Permissions: loaded by fanning out across every user and every group,
    // concatenating the results. See the module-level comment for why —
    // the daemon's filter surface only accepts ONE subject per query.
    //
    // `Resource::new` requires a `PartialEq` source, and `ManagerUserView`
    // isn't `PartialEq`; using the `(Vec<id>, Vec<id>)` projection as the
    // source re-fires the fetch when users or groups are added/removed
    // while avoiding an extra derive on the shared `ManagerUserView` type.
    let subject_ids = Signal::derive(move || {
        let uids: Vec<String> = users
            .get()
            .and_then(Result::ok)
            .unwrap_or_default()
            .into_iter()
            .map(|u| u.id)
            .collect();
        let gids: Vec<String> = groups
            .get()
            .and_then(Result::ok)
            .unwrap_or_default()
            .into_iter()
            .map(|g| g.id)
            .collect();
        (uids, gids)
    });
    let permissions = Resource::new(
        move || subject_ids.get(),
        move |(uids, gids)| async move { load_all_permissions(uids, gids).await },
    );

    // Filter state. `RwSignal` because multiple event handlers both read
    // and write (dropdowns + reset button).
    let filter_kind = RwSignal::new(SUBJECT_KIND_ALL.to_string());
    let filter_subject_id = RwSignal::new(String::new());
    let filter_resource_kind = RwSignal::new(String::new()); // "" == all

    // Modal state.
    let grant_open = RwSignal::new(false);
    let revoke_target = RwSignal::new(None::<WirePermission>);

    // Error banner.
    let (error, set_error) = signal(None::<String>);

    let refetch = move || {
        users.refetch();
        groups.refetch();
        permissions.refetch();
    };

    view! {
        <div class="p-6 space-y-4">
            <div class="flex justify-between items-center">
                <h1 class="text-2xl font-bold">"Permissions"</h1>
                <button
                    class="btn btn-primary btn-sm"
                    on:click=move |_| grant_open.set(true)
                >
                    "Grant new permission"
                </button>
            </div>

            {move || {
                error.get().map(|msg| {
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

            <FilterBar
                filter_kind=filter_kind
                filter_subject_id=filter_subject_id
                filter_resource_kind=filter_resource_kind
            />

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
                            // Require all three resources loaded before we
                            // can render — users/groups supply display names.
                            match (permissions.get(), users.get(), groups.get()) {
                                (Some(Ok(perms)), Some(Ok(users_list)), Some(Ok(groups_list))) => {
                                    let filtered = apply_filters(
                                        &perms,
                                        filter_kind.get().as_str(),
                                        filter_subject_id.get().trim(),
                                        filter_resource_kind.get().trim(),
                                    );
                                    render_table(
                                            filtered,
                                            &users_list,
                                            &groups_list,
                                            revoke_target,
                                        )
                                        .into_any()
                                }
                                (Some(Err(e)), _, _) => {
                                    view! {
                                        <div class="p-4 text-error">
                                            {format!(
                                                "Failed to load permissions: {}",
                                                format_server_error(&e),
                                            )}
                                        </div>
                                    }
                                        .into_any()
                                }
                                (_, Some(Err(e)), _) => {
                                    view! {
                                        <div class="p-4 text-error">
                                            {format!(
                                                "Failed to load users: {}",
                                                format_server_error(&e),
                                            )}
                                        </div>
                                    }
                                        .into_any()
                                }
                                (_, _, Some(Err(e))) => {
                                    view! {
                                        <div class="p-4 text-error">
                                            {format!(
                                                "Failed to load groups: {}",
                                                format_server_error(&e),
                                            )}
                                        </div>
                                    }
                                        .into_any()
                                }
                                _ => {
                                    view! {
                                        <div class="p-6 flex justify-center">
                                            <span class="loading loading-spinner loading-md"></span>
                                        </div>
                                    }
                                        .into_any()
                                }
                            }
                        }}
                    </Suspense>
                </div>
            </div>

            <GrantPermissionModal
                open=grant_open
                users=users
                groups=groups
                set_error=set_error
                on_success=move || {
                    refetch();
                }
            />

            <RevokePermissionModal
                target=revoke_target
                set_error=set_error
                on_success=move || {
                    permissions.refetch();
                }
            />
        </div>
    }
    .into_any()
}

/// Load every permission from the daemon by fanning out across users and
/// groups. The daemon only accepts one subject per call, so we paginate
/// over the full subject population on the client.
///
/// Compiles identically under SSR and hydrate; the server fn call
/// handles the transport split internally. An empty pair of input
/// vectors short-circuits to `Ok(Vec::new())` so the page renders a
/// proper "no data" state rather than racing the users/groups loads.
async fn load_all_permissions(
    user_ids: Vec<String>,
    group_ids: Vec<String>,
) -> Result<Vec<WirePermission>, ServerFnError> {
    let mut out: Vec<WirePermission> = Vec::new();
    for uid in user_ids {
        let perms =
            manager_list_permissions_for_subject(SUBJECT_KIND_USER.to_string(), uid).await?;
        out.extend(perms);
    }
    for gid in group_ids {
        let perms =
            manager_list_permissions_for_subject(SUBJECT_KIND_GROUP.to_string(), gid).await?;
        out.extend(perms);
    }
    Ok(out)
}

/// Apply all three filters client-side. All filters compose (logical AND);
/// an empty string or `SUBJECT_KIND_ALL` on subject_kind means "don't
/// narrow on this axis".
fn apply_filters(
    all: &[WirePermission],
    kind: &str,
    subject_id: &str,
    resource_kind: &str,
) -> Vec<WirePermission> {
    all.iter()
        .filter(|p| {
            (kind == SUBJECT_KIND_ALL || p.subject_kind == kind)
                && (subject_id.is_empty() || p.subject_id.contains(subject_id))
                && (resource_kind.is_empty() || p.resource_kind == resource_kind)
        })
        .cloned()
        .collect()
}

/// Human-readable subject label for the grant table: "user: alice@x.com"
/// or "group: admins". Falls back to the raw id if no matching record is
/// found (e.g. because the user was deleted after the grant was created).
fn subject_label(
    p: &WirePermission,
    users: &[ManagerUserView],
    groups: &[WireUserGroup],
) -> String {
    match p.subject_kind.as_str() {
        "user" => users.iter().find(|u| u.id == p.subject_id).map_or_else(
            || format!("user: {}", p.subject_id),
            |u| format!("user: {}", u.email),
        ),
        "group" => groups.iter().find(|g| g.id == p.subject_id).map_or_else(
            || format!("group: {}", p.subject_id),
            |g| format!("group: {}", g.name),
        ),
        other => format!("{}: {}", other, p.subject_id),
    }
}

/// Resource label: `"kind"` for wildcards, `"kind:id"` for specific
/// resources. Kept terse so the column stays narrow.
fn resource_label(p: &WirePermission) -> String {
    match &p.resource_id {
        Some(id) => format!("{}:{id}", p.resource_kind),
        None => p.resource_kind.clone(),
    }
}

/// DaisyUI badge class for a permission level — colored by severity so
/// high-privilege grants pop visually in a long list. "none" and any
/// unknown value collapse to the neutral ghost style.
fn level_badge_class(level: &str) -> &'static str {
    match level {
        "read" => "badge badge-info",
        "execute" => "badge badge-warning",
        "write" => "badge badge-error",
        _ => "badge badge-ghost",
    }
}

/// DaisyUI badge class for a subject kind.
fn subject_badge_class(kind: &str) -> &'static str {
    match kind {
        "user" => "badge badge-primary",
        "group" => "badge badge-secondary",
        _ => "badge badge-ghost",
    }
}

/// Human message shown inside the `ConfirmDeleteModal` when revoking a
/// specific grant. We stay explicit about subject/resource/level so the
/// admin doesn't accidentally revoke the wrong row.
fn format_revoke_message(p: &WirePermission) -> String {
    let resource = resource_label(p);
    format!(
        "Revoke {} grant on {} from {}:{}? This cannot be undone.",
        p.level, resource, p.subject_kind, p.subject_id,
    )
}

#[component]
fn FilterBar(
    filter_kind: RwSignal<String>,
    filter_subject_id: RwSignal<String>,
    filter_resource_kind: RwSignal<String>,
) -> impl IntoView {
    view! {
        <div class="card bg-base-200">
            <div class="card-body p-4 flex-row flex-wrap gap-3 items-end">
                <div class="form-control">
                    <label class="label py-0">
                        <span class="label-text">"Subject kind"</span>
                    </label>
                    <select
                        class="select select-bordered select-sm"
                        on:change=move |ev| filter_kind.set(event_target_value(&ev))
                    >
                        <option
                            value=SUBJECT_KIND_ALL
                            selected=move || filter_kind.get() == SUBJECT_KIND_ALL
                        >
                            "All"
                        </option>
                        <option
                            value=SUBJECT_KIND_USER
                            selected=move || filter_kind.get() == SUBJECT_KIND_USER
                        >
                            "User"
                        </option>
                        <option
                            value=SUBJECT_KIND_GROUP
                            selected=move || filter_kind.get() == SUBJECT_KIND_GROUP
                        >
                            "Group"
                        </option>
                    </select>
                </div>
                <div class="form-control">
                    <label class="label py-0">
                        <span class="label-text">"Subject id contains"</span>
                    </label>
                    <input
                        type="text"
                        class="input input-bordered input-sm"
                        placeholder="partial id"
                        prop:value=move || filter_subject_id.get()
                        on:input=move |ev| filter_subject_id.set(event_target_value(&ev))
                    />
                </div>
                <div class="form-control">
                    <label class="label py-0">
                        <span class="label-text">"Resource kind"</span>
                    </label>
                    <select
                        class="select select-bordered select-sm"
                        on:change=move |ev| filter_resource_kind.set(event_target_value(&ev))
                    >
                        <option value="" selected=move || filter_resource_kind.get().is_empty()>
                            "All"
                        </option>
                        {RESOURCE_KINDS
                            .iter()
                            .map(|kind| {
                                let kind = (*kind).to_string();
                                let kind_sel = kind.clone();
                                let kind_label = kind.clone();
                                view! {
                                    <option
                                        value=kind.clone()
                                        selected=move || filter_resource_kind.get() == kind_sel
                                    >
                                        {kind_label}
                                    </option>
                                }
                            })
                            .collect_view()}
                    </select>
                </div>
                <button
                    class="btn btn-ghost btn-sm"
                    on:click=move |_| {
                        filter_kind.set(SUBJECT_KIND_ALL.to_string());
                        filter_subject_id.set(String::new());
                        filter_resource_kind.set(String::new());
                    }
                >
                    "Reset filters"
                </button>
            </div>
        </div>
    }
}

fn render_table(
    filtered: Vec<WirePermission>,
    users: &[ManagerUserView],
    groups: &[WireUserGroup],
    revoke_target: RwSignal<Option<WirePermission>>,
) -> impl IntoView {
    if filtered.is_empty() {
        return view! {
            <div class="p-4 opacity-70">"(no permissions match the current filters)"</div>
        }
        .into_any();
    }

    view! {
        <table class="table table-zebra">
            <thead>
                <tr>
                    <th>"Subject"</th>
                    <th>"Resource"</th>
                    <th>"Level"</th>
                    <th>"Created"</th>
                    <th class="text-right">"Actions"</th>
                </tr>
            </thead>
            <tbody>
                {filtered
                    .into_iter()
                    .map(|p| render_row(&p, users, groups, revoke_target))
                    .collect_view()}
            </tbody>
        </table>
    }
    .into_any()
}

fn render_row(
    p: &WirePermission,
    users: &[ManagerUserView],
    groups: &[WireUserGroup],
    revoke_target: RwSignal<Option<WirePermission>>,
) -> impl IntoView {
    let subj_class = subject_badge_class(&p.subject_kind);
    let subj_label = subject_label(p, users, groups);
    let res_label = resource_label(p);
    let lvl_class = level_badge_class(&p.level);
    let lvl = p.level.clone();
    let created = p.created_at.clone();
    let for_delete = p.clone();

    view! {
        <tr>
            <td>
                <span class=subj_class>{subj_label}</span>
            </td>
            <td class="font-mono text-xs">{res_label}</td>
            <td>
                <span class=lvl_class>{lvl}</span>
            </td>
            <td class="text-xs opacity-70">{created}</td>
            <td class="text-right">
                <button
                    class="btn btn-xs btn-error"
                    on:click=move |_| revoke_target.set(Some(for_delete.clone()))
                >
                    "Revoke"
                </button>
            </td>
        </tr>
    }
}

#[component]
#[allow(clippy::too_many_lines)] // DaisyUI form DSL + cascading pickers
fn GrantPermissionModal(
    open: RwSignal<bool>,
    users: Resource<Result<Vec<ManagerUserView>, ServerFnError>>,
    groups: Resource<Result<Vec<WireUserGroup>, ServerFnError>>,
    set_error: WriteSignal<Option<String>>,
    on_success: impl Fn() + Copy + Send + Sync + 'static,
) -> impl IntoView {
    // Local form state — reset every time the modal opens.
    let subject_kind = RwSignal::new(SUBJECT_KIND_USER.to_string());
    let subject_id = RwSignal::new(String::new());
    let resource_kind = RwSignal::new(RESOURCE_KINDS[0].to_string());
    let resource_id = RwSignal::new(String::new());
    let level = RwSignal::new(PERMISSION_LEVELS[0].to_string());
    let submitting = RwSignal::new(false);

    let reset = move || {
        subject_kind.set(SUBJECT_KIND_USER.to_string());
        subject_id.set(String::new());
        resource_kind.set(RESOURCE_KINDS[0].to_string());
        resource_id.set(String::new());
        level.set(PERMISSION_LEVELS[0].to_string());
    };

    // Re-seed the subject_id dropdown when the user flips kind.
    Effect::new(move |_| {
        let _ = subject_kind.get();
        subject_id.set(String::new());
    });

    let on_submit = move |ev: SubmitEvent| {
        ev.prevent_default();
        if submitting.get() {
            return;
        }
        let kind = subject_kind.get();
        let sid = subject_id.get();
        let rkind = resource_kind.get();
        let rid_raw = resource_id.get();
        let rid = if rid_raw.trim().is_empty() {
            None
        } else {
            Some(rid_raw.trim().to_string())
        };
        let lvl = level.get();

        if sid.trim().is_empty() {
            set_error.set(Some("Select a subject.".to_string()));
            return;
        }
        if rkind.trim().is_empty() {
            set_error.set(Some("Select a resource kind.".to_string()));
            return;
        }

        submitting.set(true);
        spawn_local(async move {
            let req = WireGrantPermissionRequest {
                subject_kind: kind,
                subject_id: sid,
                resource_kind: rkind,
                resource_id: rid,
                level: lvl,
            };
            let result = manager_grant_permission(req).await;
            submitting.set(false);
            match result {
                Ok(_) => {
                    reset();
                    open.set(false);
                    on_success();
                }
                Err(e) => {
                    set_error.set(Some(format!("Grant failed: {}", format_server_error(&e),)));
                }
            }
        });
    };

    view! {
        <dialog class=move || if open.get() { "modal modal-open" } else { "modal" }>
            <div class="modal-box max-w-xl">
                <h3 class="font-bold text-lg mb-3">"Grant permission"</h3>
                <form on:submit=on_submit class="space-y-3">
                    <div class="form-control">
                        <label class="label">
                            <span class="label-text">"Subject kind"</span>
                        </label>
                        <select
                            class="select select-bordered w-full"
                            on:change=move |ev| subject_kind.set(event_target_value(&ev))
                        >
                            <option
                                value=SUBJECT_KIND_USER
                                selected=move || subject_kind.get() == SUBJECT_KIND_USER
                            >
                                "User"
                            </option>
                            <option
                                value=SUBJECT_KIND_GROUP
                                selected=move || subject_kind.get() == SUBJECT_KIND_GROUP
                            >
                                "Group"
                            </option>
                        </select>
                    </div>

                    <div class="form-control">
                        <label class="label">
                            <span class="label-text">"Subject"</span>
                        </label>
                        {move || {
                            // Cascading picker: show users when kind=user,
                            // groups when kind=group. Uses the resources
                            // we already loaded for the main table.
                            let kind = subject_kind.get();
                            if kind == SUBJECT_KIND_USER {
                                render_user_picker(users, subject_id).into_any()
                            } else {
                                render_group_picker(groups, subject_id).into_any()
                            }
                        }}
                    </div>

                    <div class="form-control">
                        <label class="label">
                            <span class="label-text">"Resource kind"</span>
                        </label>
                        <select
                            class="select select-bordered w-full"
                            on:change=move |ev| resource_kind.set(event_target_value(&ev))
                        >
                            {RESOURCE_KINDS
                                .iter()
                                .map(|kind| {
                                    let kind = (*kind).to_string();
                                    let kind_sel = kind.clone();
                                    let kind_label = kind.clone();
                                    view! {
                                        <option
                                            value=kind.clone()
                                            selected=move || resource_kind.get() == kind_sel
                                        >
                                            {kind_label}
                                        </option>
                                    }
                                })
                                .collect_view()}
                        </select>
                    </div>

                    <div class="form-control">
                        <label class="label">
                            <span class="label-text">"Resource id (optional)"</span>
                            <span class="label-text-alt opacity-70">"Blank = all resources of that kind"</span>
                        </label>
                        <input
                            type="text"
                            class="input input-bordered w-full"
                            placeholder="(wildcard)"
                            prop:value=move || resource_id.get()
                            on:input=move |ev| resource_id.set(event_target_value(&ev))
                        />
                    </div>

                    <div class="form-control">
                        <label class="label">
                            <span class="label-text">"Level"</span>
                        </label>
                        <select
                            class="select select-bordered w-full"
                            on:change=move |ev| level.set(event_target_value(&ev))
                        >
                            {PERMISSION_LEVELS
                                .iter()
                                .map(|lvl| {
                                    let lvl = (*lvl).to_string();
                                    let lvl_sel = lvl.clone();
                                    let lvl_label = lvl.clone();
                                    view! {
                                        <option
                                            value=lvl.clone()
                                            selected=move || level.get() == lvl_sel
                                        >
                                            {lvl_label}
                                        </option>
                                    }
                                })
                                .collect_view()}
                        </select>
                    </div>

                    <div class="modal-action">
                        <button
                            type="button"
                            class="btn btn-ghost"
                            on:click=move |_| {
                                reset();
                                open.set(false);
                            }
                        >
                            "Cancel"
                        </button>
                        <button
                            type="submit"
                            class="btn btn-primary"
                            disabled=move || submitting.get()
                        >
                            {move || if submitting.get() { "Granting…" } else { "Grant" }}
                        </button>
                    </div>
                </form>
            </div>
            <form
                method="dialog"
                class="modal-backdrop"
                on:submit=move |_| open.set(false)
            >
                <button>"close"</button>
            </form>
        </dialog>
    }
}

/// Render the subject-id picker as a <select> over loaded users. Falls
/// back to a free-form text input while users are loading or if the call
/// errored — admins can still type in an id directly.
fn render_user_picker(
    users: Resource<Result<Vec<ManagerUserView>, ServerFnError>>,
    subject_id: RwSignal<String>,
) -> impl IntoView {
    view! {
        {move || {
            match users.get() {
                Some(Ok(list)) => {
                    view! {
                        <select
                            class="select select-bordered w-full"
                            on:change=move |ev| subject_id.set(event_target_value(&ev))
                        >
                            <option value="" selected=move || subject_id.get().is_empty()>
                                "-- select user --"
                            </option>
                            {list
                                .into_iter()
                                .map(|u| {
                                    let id = u.id.clone();
                                    let id_sel = id.clone();
                                    let label = format!("{} ({})", u.email, u.id);
                                    view! {
                                        <option
                                            value=id.clone()
                                            selected=move || subject_id.get() == id_sel
                                        >
                                            {label}
                                        </option>
                                    }
                                })
                                .collect_view()}
                        </select>
                    }
                        .into_any()
                }
                _ => {
                    view! {
                        <input
                            type="text"
                            class="input input-bordered w-full"
                            placeholder="user id"
                            prop:value=move || subject_id.get()
                            on:input=move |ev| subject_id.set(event_target_value(&ev))
                        />
                    }
                        .into_any()
                }
            }
        }}
    }
}

/// Render the subject-id picker as a <select> over loaded groups. Falls
/// back to a free-form text input while groups are loading or if the
/// call errored.
fn render_group_picker(
    groups: Resource<Result<Vec<WireUserGroup>, ServerFnError>>,
    subject_id: RwSignal<String>,
) -> impl IntoView {
    view! {
        {move || {
            match groups.get() {
                Some(Ok(list)) => {
                    view! {
                        <select
                            class="select select-bordered w-full"
                            on:change=move |ev| subject_id.set(event_target_value(&ev))
                        >
                            <option value="" selected=move || subject_id.get().is_empty()>
                                "-- select group --"
                            </option>
                            {list
                                .into_iter()
                                .map(|g| {
                                    let id = g.id.clone();
                                    let id_sel = id.clone();
                                    let label = format!("{} ({})", g.name, g.id);
                                    view! {
                                        <option
                                            value=id.clone()
                                            selected=move || subject_id.get() == id_sel
                                        >
                                            {label}
                                        </option>
                                    }
                                })
                                .collect_view()}
                        </select>
                    }
                        .into_any()
                }
                _ => {
                    view! {
                        <input
                            type="text"
                            class="input input-bordered w-full"
                            placeholder="group id"
                            prop:value=move || subject_id.get()
                            on:input=move |ev| subject_id.set(event_target_value(&ev))
                        />
                    }
                        .into_any()
                }
            }
        }}
    }
}

/// Confirmation modal for revoking a permission. Matches the pattern used by
/// every other page (notifiers / users / tasks) — inline so we can show
/// per-target detail in the message text.
#[component]
fn RevokePermissionModal(
    target: RwSignal<Option<WirePermission>>,
    set_error: WriteSignal<Option<String>>,
    on_success: impl Fn() + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let (submitting, set_submitting) = signal(false);

    let close = move || target.set(None);

    let confirm = move |_| {
        let Some(p) = target.get() else {
            return;
        };
        set_submitting.set(true);
        spawn_local(async move {
            let result = manager_revoke_permission(p.id.clone()).await;
            set_submitting.set(false);
            match result {
                Ok(()) => {
                    close();
                    on_success();
                }
                Err(e) => {
                    set_error.set(Some(format!("Revoke failed: {}", format_server_error(&e))));
                }
            }
        });
    };

    view! {
        <dialog class=move || {
            if target.with(Option::is_some) { "modal modal-open" } else { "modal" }
        }>
            <div class="modal-box">
                <h3 class="font-bold text-lg mb-3">"Revoke permission"</h3>
                <p class="text-sm opacity-80">
                    {move || {
                        target
                            .get()
                            .map(|p| format_revoke_message(&p))
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
                        {move || if submitting.get() { "Revoking…" } else { "Revoke" }}
                    </button>
                </div>
            </div>
        </dialog>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_perm(
        id: &str,
        subject_kind: &str,
        subject_id: &str,
        resource_kind: &str,
        resource_id: Option<&str>,
        level: &str,
    ) -> WirePermission {
        WirePermission {
            id: id.to_string(),
            subject_kind: subject_kind.to_string(),
            subject_id: subject_id.to_string(),
            resource_kind: resource_kind.to_string(),
            resource_id: resource_id.map(String::from),
            level: level.to_string(),
            created_at: "2026-04-16T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn apply_filters_all_passes_through() {
        let all = vec![
            make_perm("1", "user", "u1", "deployment", Some("d1"), "read"),
            make_perm("2", "group", "g1", "project", None, "write"),
        ];
        let out = apply_filters(&all, SUBJECT_KIND_ALL, "", "");
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn apply_filters_narrows_by_subject_kind() {
        let all = vec![
            make_perm("1", "user", "u1", "deployment", None, "read"),
            make_perm("2", "group", "g1", "deployment", None, "read"),
        ];
        let users_only = apply_filters(&all, "user", "", "");
        assert_eq!(users_only.len(), 1);
        assert_eq!(users_only[0].subject_kind, "user");
    }

    #[test]
    fn apply_filters_narrows_by_subject_id_substring() {
        let all = vec![
            make_perm("1", "user", "alice-123", "deployment", None, "read"),
            make_perm("2", "user", "bob-456", "deployment", None, "read"),
        ];
        let out = apply_filters(&all, SUBJECT_KIND_ALL, "alice", "");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].subject_id, "alice-123");
    }

    #[test]
    fn apply_filters_narrows_by_resource_kind() {
        let all = vec![
            make_perm("1", "user", "u1", "deployment", None, "read"),
            make_perm("2", "user", "u1", "secret", None, "read"),
        ];
        let out = apply_filters(&all, SUBJECT_KIND_ALL, "", "secret");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].resource_kind, "secret");
    }

    #[test]
    fn resource_label_wildcard_vs_specific() {
        let wc = make_perm("1", "user", "u1", "deployment", None, "read");
        assert_eq!(resource_label(&wc), "deployment");
        let spec = make_perm("2", "user", "u1", "deployment", Some("d1"), "read");
        assert_eq!(resource_label(&spec), "deployment:d1");
    }

    #[test]
    fn subject_label_joins_email() {
        let users = vec![ManagerUserView {
            id: "u1".into(),
            email: "alice@example.com".into(),
            display_name: "Alice".into(),
            role: "user".into(),
            is_active: true,
            last_login_at: None,
        }];
        let groups: Vec<WireUserGroup> = Vec::new();
        let p = make_perm("p1", "user", "u1", "deployment", None, "read");
        assert_eq!(
            subject_label(&p, &users, &groups),
            "user: alice@example.com"
        );
    }

    #[test]
    fn subject_label_joins_group_name() {
        let users: Vec<ManagerUserView> = Vec::new();
        let groups = vec![WireUserGroup {
            id: "g1".into(),
            name: "admins".into(),
            description: None,
            created_at: "2026-04-16T00:00:00Z".into(),
            updated_at: "2026-04-16T00:00:00Z".into(),
        }];
        let p = make_perm("p1", "group", "g1", "deployment", None, "read");
        assert_eq!(subject_label(&p, &users, &groups), "group: admins");
    }

    #[test]
    fn subject_label_falls_back_to_id() {
        let p = make_perm("p1", "user", "u-missing", "deployment", None, "read");
        assert_eq!(
            subject_label(&p, &[], &[]),
            "user: u-missing",
            "unknown user should fall back to the raw id, not panic",
        );
    }

    #[test]
    fn level_badge_class_is_stable_for_known_levels() {
        assert_eq!(level_badge_class("read"), "badge badge-info");
        assert_eq!(level_badge_class("execute"), "badge badge-warning");
        assert_eq!(level_badge_class("write"), "badge badge-error");
        assert_eq!(level_badge_class("none"), "badge badge-ghost");
        assert_eq!(
            level_badge_class("unknown-future-level"),
            "badge badge-ghost"
        );
    }

    #[test]
    fn wire_permission_round_trips_wildcard() {
        let json = r#"{
            "id": "p1",
            "subject_kind": "user",
            "subject_id": "u1",
            "resource_kind": "deployment",
            "resource_id": null,
            "level": "write",
            "created_at": "2026-04-16T00:00:00Z"
        }"#;
        let p: WirePermission = serde_json::from_str(json).unwrap();
        assert_eq!(p.id, "p1");
        assert!(p.resource_id.is_none());
        assert_eq!(p.level, "write");
    }
}
