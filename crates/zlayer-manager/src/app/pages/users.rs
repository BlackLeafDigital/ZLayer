//! `/users` — admin user management.
//!
//! Lists all users and lets admins create, toggle role/active, reset
//! password, and delete them. Non-admins see an access-denied banner
//! because the page is mounted without a route-level role gate in this
//! phase (the backend still enforces admin-only on every mutation).

use leptos::ev::SubmitEvent;
use leptos::prelude::*;
use leptos::task::spawn_local;

use crate::app::auth_guard::CurrentUser;
use crate::app::server_fns::{
    manager_create_user, manager_delete_user, manager_list_users, manager_set_user_password,
    manager_update_user, ManagerCreateUserRequest, ManagerSetPasswordRequest,
    ManagerUpdateUserRequest, ManagerUserView,
};

/// Admin-only users page.
#[component]
#[allow(clippy::too_many_lines)] // view macro DSL + multiple modals
pub fn Users() -> impl IntoView {
    let current = use_context::<CurrentUser>();
    let is_admin = current.as_ref().is_some_and(CurrentUser::is_admin);

    if !is_admin {
        return view! {
            <div class="p-6">
                <div class="alert alert-warning max-w-xl">
                    <div>
                        <h3 class="font-bold">"Access denied"</h3>
                        <p class="text-sm opacity-80">
                            "Admin role required to manage users."
                        </p>
                    </div>
                </div>
            </div>
        }
        .into_any();
    }

    let users = Resource::new(|| (), |()| async move { manager_list_users().await });
    let (new_user_open, set_new_user_open) = signal(false);
    let (error, set_error) = signal(None::<String>);

    // `RwSignal<Option<T>>` lets downstream closures reach for `with` /
    // `get` / `set` without juggling a read/write pair; that's nicer for
    // "modal target" state that's read in one place and written in many.
    let password_target = RwSignal::new(None::<ManagerUserView>);
    let delete_target = RwSignal::new(None::<ManagerUserView>);

    let refetch = move || users.refetch();

    view! {
        <div class="p-6 space-y-4">
            <div class="flex justify-between items-center">
                <h1 class="text-2xl font-bold">"Users"</h1>
                <button
                    class="btn btn-primary btn-sm"
                    on:click=move |_| set_new_user_open.set(true)
                >
                    "New user"
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
                            users
                                .get()
                                .map(|res| match res {
                                    Err(e) => {
                                        view! {
                                            <div class="p-4 text-error">
                                                {format!("Failed to load users: {e}")}
                                            </div>
                                        }
                                            .into_any()
                                    }
                                    Ok(list) => {
                                        render_table(
                                                list,
                                                current.as_ref(),
                                                password_target,
                                                delete_target,
                                                set_error,
                                                refetch,
                                            )
                                            .into_any()
                                    }
                                })
                        }}
                    </Suspense>
                </div>
            </div>

            <NewUserModal
                open=new_user_open
                set_open=set_new_user_open
                set_error=set_error
                on_success=move || {
                    refetch();
                }
            />

            <PasswordModal
                target=password_target
                set_error=set_error
                on_success=move || {
                    refetch();
                }
            />

            <DeleteModal
                target=delete_target
                set_error=set_error
                on_success=move || {
                    refetch();
                }
            />
        </div>
    }
    .into_any()
}

fn render_table(
    list: Vec<ManagerUserView>,
    current: Option<&CurrentUser>,
    password_target: RwSignal<Option<ManagerUserView>>,
    delete_target: RwSignal<Option<ManagerUserView>>,
    set_error: WriteSignal<Option<String>>,
    refetch: impl Fn() + Copy + Send + Sync + 'static,
) -> impl IntoView {
    if list.is_empty() {
        return view! { <div class="p-4 opacity-70">"(no users)"</div> }.into_any();
    }

    let current_id = current.map(|c| c.0.id.clone());

    view! {
        <table class="table table-zebra">
            <thead>
                <tr>
                    <th>"Email"</th>
                    <th>"Display name"</th>
                    <th>"Role"</th>
                    <th>"Active"</th>
                    <th>"Last login"</th>
                    <th class="text-right">"Actions"</th>
                </tr>
            </thead>
            <tbody>
                {list
                    .into_iter()
                    .map(|u| {
                        render_row(
                            &u,
                            current_id.as_deref(),
                            password_target,
                            delete_target,
                            set_error,
                            refetch,
                        )
                    })
                    .collect_view()}
            </tbody>
        </table>
    }
    .into_any()
}

#[allow(clippy::too_many_lines)] // DaisyUI table row markup is naturally verbose
fn render_row(
    user: &ManagerUserView,
    current_id: Option<&str>,
    password_target: RwSignal<Option<ManagerUserView>>,
    delete_target: RwSignal<Option<ManagerUserView>>,
    set_error: WriteSignal<Option<String>>,
    refetch: impl Fn() + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let is_self = current_id == Some(user.id.as_str());
    let role_badge_class = if user.role == "admin" {
        "badge badge-primary"
    } else {
        "badge badge-ghost"
    };
    let active_badge_class = if user.is_active {
        "badge badge-success"
    } else {
        "badge badge-error"
    };
    let last_login = user
        .last_login_at
        .clone()
        .unwrap_or_else(|| "—".to_string());

    let toggle_role_user = user.clone();
    let toggle_role = move |_| {
        let next_role = if toggle_role_user.role == "admin" {
            "user"
        } else {
            "admin"
        };
        let id = toggle_role_user.id.clone();
        let next = next_role.to_string();
        spawn_local(async move {
            let result = manager_update_user(
                id,
                ManagerUpdateUserRequest {
                    display_name: None,
                    role: Some(next),
                    is_active: None,
                },
            )
            .await;
            match result {
                Ok(_) => {
                    refetch();
                }
                Err(e) => set_error.set(Some(format!("Update failed: {e}"))),
            }
        });
    };

    let toggle_active_user = user.clone();
    let toggle_active = move |_| {
        let next = !toggle_active_user.is_active;
        let id = toggle_active_user.id.clone();
        spawn_local(async move {
            let result = manager_update_user(
                id,
                ManagerUpdateUserRequest {
                    display_name: None,
                    role: None,
                    is_active: Some(next),
                },
            )
            .await;
            match result {
                Ok(_) => {
                    refetch();
                }
                Err(e) => set_error.set(Some(format!("Update failed: {e}"))),
            }
        });
    };

    let password_user = user.clone();
    let delete_user_val = user.clone();

    let toggle_role_title = if is_self {
        "Cannot change your own role"
    } else {
        "Toggle role"
    };
    let active_label = if user.is_active { "Disable" } else { "Enable" };
    let active_text = if user.is_active { "yes" } else { "no" };

    view! {
        <tr>
            <td class="font-mono text-xs">{user.email.clone()}</td>
            <td>{user.display_name.clone()}</td>
            <td>
                <span class=role_badge_class>{user.role.clone()}</span>
            </td>
            <td>
                <span class=active_badge_class>{active_text}</span>
            </td>
            <td class="text-xs opacity-70">{last_login}</td>
            <td class="text-right space-x-1">
                <button
                    class="btn btn-xs btn-ghost"
                    disabled=is_self
                    title=toggle_role_title
                    on:click=toggle_role
                >
                    "Toggle role"
                </button>
                <button
                    class="btn btn-xs btn-ghost"
                    disabled=is_self
                    on:click=toggle_active
                >
                    {active_label}
                </button>
                <button
                    class="btn btn-xs btn-ghost"
                    on:click=move |_| password_target.set(Some(password_user.clone()))
                >
                    "Reset password"
                </button>
                <button
                    class="btn btn-xs btn-error"
                    disabled=is_self
                    on:click=move |_| delete_target.set(Some(delete_user_val.clone()))
                >
                    "Delete"
                </button>
            </td>
        </tr>
    }
}

#[component]
fn NewUserModal(
    open: ReadSignal<bool>,
    set_open: WriteSignal<bool>,
    set_error: WriteSignal<Option<String>>,
    on_success: impl Fn() + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let (email, set_email) = signal(String::new());
    let (display_name, set_display_name) = signal(String::new());
    let (password, set_password) = signal(String::new());
    let (role, set_role) = signal(String::from("user"));
    let (submitting, set_submitting) = signal(false);

    let reset = move || {
        set_email.set(String::new());
        set_display_name.set(String::new());
        set_password.set(String::new());
        set_role.set(String::from("user"));
    };

    let on_submit = move |ev: SubmitEvent| {
        ev.prevent_default();
        if submitting.get() {
            return;
        }
        let email_val = email.get();
        let password_val = password.get();
        let display_name_val = display_name.get();
        let role_val = role.get();

        if email_val.trim().is_empty() || password_val.is_empty() {
            set_error.set(Some("Email and password are required.".to_string()));
            return;
        }

        set_submitting.set(true);

        spawn_local(async move {
            let display_name_opt = if display_name_val.trim().is_empty() {
                None
            } else {
                Some(display_name_val)
            };
            let result = manager_create_user(ManagerCreateUserRequest {
                email: email_val,
                password: password_val,
                display_name: display_name_opt,
                role: role_val,
            })
            .await;

            set_submitting.set(false);

            match result {
                Ok(_) => {
                    reset();
                    set_open.set(false);
                    on_success();
                }
                Err(e) => {
                    set_error.set(Some(format!("Create failed: {e}")));
                }
            }
        });
    };

    view! {
        <dialog class=move || if open.get() { "modal modal-open" } else { "modal" }>
            <div class="modal-box">
                <h3 class="font-bold text-lg mb-3">"New user"</h3>
                <form on:submit=on_submit class="space-y-3">
                    <div class="form-control">
                        <label class="label">
                            <span class="label-text">"Email"</span>
                        </label>
                        <input
                            type="email"
                            class="input input-bordered w-full"
                            required
                            prop:value=email
                            on:input=move |ev| set_email.set(event_target_value(&ev))
                        />
                    </div>
                    <div class="form-control">
                        <label class="label">
                            <span class="label-text">"Display name"</span>
                        </label>
                        <input
                            type="text"
                            class="input input-bordered w-full"
                            prop:value=display_name
                            on:input=move |ev| set_display_name.set(event_target_value(&ev))
                        />
                    </div>
                    <div class="form-control">
                        <label class="label">
                            <span class="label-text">"Password"</span>
                        </label>
                        <input
                            type="password"
                            class="input input-bordered w-full"
                            required
                            minlength="8"
                            prop:value=password
                            on:input=move |ev| set_password.set(event_target_value(&ev))
                        />
                    </div>
                    <div class="form-control">
                        <label class="label">
                            <span class="label-text">"Role"</span>
                        </label>
                        <select
                            class="select select-bordered w-full"
                            on:change=move |ev| set_role.set(event_target_value(&ev))
                        >
                            <option value="user" selected=move || role.get() == "user">
                                "User"
                            </option>
                            <option value="admin" selected=move || role.get() == "admin">
                                "Admin"
                            </option>
                        </select>
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
fn PasswordModal(
    target: RwSignal<Option<ManagerUserView>>,
    set_error: WriteSignal<Option<String>>,
    on_success: impl Fn() + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let (new_password, set_new_password) = signal(String::new());
    let (submitting, set_submitting) = signal(false);

    let close = move || {
        target.set(None);
        set_new_password.set(String::new());
    };

    let on_submit = move |ev: SubmitEvent| {
        ev.prevent_default();
        let Some(user) = target.get() else {
            return;
        };
        let pw = new_password.get();
        if pw.len() < 8 {
            set_error.set(Some("Password must be at least 8 characters.".to_string()));
            return;
        }
        set_submitting.set(true);
        spawn_local(async move {
            let result = manager_set_user_password(
                user.id.clone(),
                ManagerSetPasswordRequest {
                    new_password: pw,
                    current_password: None,
                },
            )
            .await;
            set_submitting.set(false);
            match result {
                Ok(()) => {
                    close();
                    on_success();
                }
                Err(e) => set_error.set(Some(format!("Password update failed: {e}"))),
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
                        target
                            .get()
                            .map(|u| format!("Reset password for {}", u.email))
                            .unwrap_or_default()
                    }}
                </h3>
                <form on:submit=on_submit class="space-y-3">
                    <div class="form-control">
                        <label class="label">
                            <span class="label-text">"New password"</span>
                        </label>
                        <input
                            type="password"
                            class="input input-bordered w-full"
                            required
                            minlength="8"
                            prop:value=new_password
                            on:input=move |ev| set_new_password.set(event_target_value(&ev))
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
                            {move || if submitting.get() { "Updating…" } else { "Update" }}
                        </button>
                    </div>
                </form>
            </div>
        </dialog>
    }
}

#[component]
fn DeleteModal(
    target: RwSignal<Option<ManagerUserView>>,
    set_error: WriteSignal<Option<String>>,
    on_success: impl Fn() + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let (submitting, set_submitting) = signal(false);

    let close = move || target.set(None);

    let confirm = move |_| {
        let Some(user) = target.get() else {
            return;
        };
        set_submitting.set(true);
        spawn_local(async move {
            let result = manager_delete_user(user.id.clone()).await;
            set_submitting.set(false);
            match result {
                Ok(()) => {
                    close();
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
                <h3 class="font-bold text-lg mb-3">"Delete user"</h3>
                <p>
                    {move || {
                        target
                            .get()
                            .map(|u| {
                                format!(
                                    "Delete {} <{}>? This cannot be undone.",
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
                        {move || if submitting.get() { "Deleting…" } else { "Delete" }}
                    </button>
                </div>
            </div>
        </dialog>
    }
}
