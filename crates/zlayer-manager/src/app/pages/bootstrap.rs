//! `/bootstrap` — first-run setup creating the initial admin user.
//!
//! Reachable only when the backend has no users yet (the auth guard routes
//! unauthenticated visitors here instead of `/login` on fresh installs).

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::use_navigate;
use leptos_router::NavigateOptions;

use crate::app::server_fns::{manager_bootstrap, ManagerBootstrapRequest};
use crate::app::util::errors::humanize_error;

#[component]
#[allow(clippy::too_many_lines)] // view macro DSL; form has multiple fields
pub fn Bootstrap() -> impl IntoView {
    let (email, set_email) = signal(String::new());
    let (display_name, set_display_name) = signal(String::new());
    let (password, set_password) = signal(String::new());
    let (confirm, set_confirm) = signal(String::new());
    let (error, set_error) = signal(None::<String>);
    let (submitting, set_submitting) = signal(false);

    // Password strength indicator (client-side only — real strength is
    // enforced server-side via the Argon2id hash + future policy).
    let strength = Memo::new(move |_| password_strength(&password.get()));

    let on_submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        if submitting.get() {
            return;
        }

        let email_val = email.get();
        let password_val = password.get();
        let confirm_val = confirm.get();
        let display_name_val = display_name.get();

        if email_val.trim().is_empty() || password_val.is_empty() {
            set_error.set(Some("Email and password are required.".to_string()));
            return;
        }
        if password_val != confirm_val {
            set_error.set(Some("Passwords do not match.".to_string()));
            return;
        }
        if password_val.len() < 8 {
            set_error.set(Some("Password must be at least 8 characters.".to_string()));
            return;
        }

        set_error.set(None);
        set_submitting.set(true);

        spawn_local(async move {
            let display_name_opt = if display_name_val.trim().is_empty() {
                None
            } else {
                Some(display_name_val)
            };

            let result = manager_bootstrap(ManagerBootstrapRequest {
                email: email_val,
                password: password_val,
                display_name: display_name_opt,
            })
            .await;

            set_submitting.set(false);
            match result {
                Ok(_resp) => {
                    let navigate = use_navigate();
                    navigate("/", NavigateOptions::default());
                }
                Err(e) => {
                    set_error.set(Some(humanize_error(&e.to_string())));
                }
            }
        });
    };

    view! {
        <div class="min-h-screen flex items-center justify-center bg-base-100 p-4">
            <div class="card bg-base-200 w-full max-w-md shadow-xl">
                <div class="card-body">
                    <h1 class="card-title text-2xl justify-center">"Welcome to ZLayer"</h1>
                    <p class="text-sm opacity-70 text-center">
                        "Create the first admin account to get started."
                    </p>

                    <form on:submit=on_submit class="mt-4 space-y-3">
                        <div class="form-control">
                            <label class="label" for="bs-email">
                                <span class="label-text">"Email"</span>
                            </label>
                            <input
                                id="bs-email"
                                type="email"
                                class="input input-bordered w-full"
                                autocomplete="email"
                                required
                                prop:value=email
                                on:input=move |ev| set_email.set(event_target_value(&ev))
                            />
                        </div>

                        <div class="form-control">
                            <label class="label" for="bs-display-name">
                                <span class="label-text">
                                    "Display name "
                                    <span class="opacity-60">"(optional)"</span>
                                </span>
                            </label>
                            <input
                                id="bs-display-name"
                                type="text"
                                class="input input-bordered w-full"
                                autocomplete="name"
                                prop:value=display_name
                                on:input=move |ev| set_display_name.set(event_target_value(&ev))
                            />
                        </div>

                        <div class="form-control">
                            <label class="label" for="bs-password">
                                <span class="label-text">"Password"</span>
                            </label>
                            <input
                                id="bs-password"
                                type="password"
                                class="input input-bordered w-full"
                                autocomplete="new-password"
                                required
                                minlength="8"
                                prop:value=password
                                on:input=move |ev| set_password.set(event_target_value(&ev))
                            />
                            <StrengthMeter strength />
                        </div>

                        <div class="form-control">
                            <label class="label" for="bs-confirm">
                                <span class="label-text">"Confirm password"</span>
                            </label>
                            <input
                                id="bs-confirm"
                                type="password"
                                class="input input-bordered w-full"
                                autocomplete="new-password"
                                required
                                prop:value=confirm
                                on:input=move |ev| set_confirm.set(event_target_value(&ev))
                            />
                        </div>

                        {move || {
                            error.get().map(|msg| view! {
                                <div class="alert alert-error text-sm py-2">
                                    <span>{msg}</span>
                                </div>
                            })
                        }}

                        <button
                            type="submit"
                            class="btn btn-primary w-full"
                            disabled=move || submitting.get()
                        >
                            {move || if submitting.get() { "Creating account…" } else { "Create admin account" }}
                        </button>
                    </form>
                </div>
            </div>
        </div>
    }
}

/// Password-strength classes, simple heuristic (length + character classes).
/// Strictly advisory — the server owns the actual policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Strength {
    Empty,
    Weak,
    Fair,
    Strong,
}

fn password_strength(pw: &str) -> Strength {
    if pw.is_empty() {
        return Strength::Empty;
    }
    let has_lower = pw.chars().any(|c| c.is_ascii_lowercase());
    let has_upper = pw.chars().any(|c| c.is_ascii_uppercase());
    let has_digit = pw.chars().any(|c| c.is_ascii_digit());
    let has_symbol = pw.chars().any(|c| !c.is_ascii_alphanumeric());
    let class_count = [has_lower, has_upper, has_digit, has_symbol]
        .into_iter()
        .filter(|&b| b)
        .count();

    match (pw.len(), class_count) {
        (l, _) if l < 8 => Strength::Weak,
        (l, c) if l >= 12 && c >= 3 => Strength::Strong,
        (_, c) if c >= 2 => Strength::Fair,
        _ => Strength::Weak,
    }
}

#[component]
fn StrengthMeter(strength: Memo<Strength>) -> impl IntoView {
    view! {
        <div class="mt-1">
            {move || match strength.get() {
                Strength::Empty => view! { <span class="text-xs opacity-40">"Choose a strong password."</span> }.into_any(),
                Strength::Weak => view! {
                    <progress class="progress progress-error w-full h-1" value="25" max="100"></progress>
                    <span class="text-xs text-error">"Weak"</span>
                }.into_any(),
                Strength::Fair => view! {
                    <progress class="progress progress-warning w-full h-1" value="55" max="100"></progress>
                    <span class="text-xs text-warning">"Fair"</span>
                }.into_any(),
                Strength::Strong => view! {
                    <progress class="progress progress-success w-full h-1" value="90" max="100"></progress>
                    <span class="text-xs text-success">"Strong"</span>
                }.into_any(),
            }}
        </div>
    }
}
