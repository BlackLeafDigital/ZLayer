//! `/login` — email + password sign-in page.

use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::use_navigate;
use leptos_router::NavigateOptions;

use crate::app::server_fns::{manager_login, ManagerLoginRequest};

#[component]
pub fn Login() -> impl IntoView {
    let (email, set_email) = signal(String::new());
    let (password, set_password) = signal(String::new());
    let (error, set_error) = signal(None::<String>);
    let (submitting, set_submitting) = signal(false);

    let on_submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        if submitting.get() {
            return;
        }
        let email_val = email.get();
        let password_val = password.get();
        if email_val.is_empty() || password_val.is_empty() {
            set_error.set(Some("Email and password are required.".to_string()));
            return;
        }
        set_error.set(None);
        set_submitting.set(true);

        spawn_local(async move {
            let result = manager_login(ManagerLoginRequest {
                email: email_val,
                password: password_val,
            })
            .await;

            set_submitting.set(false);
            match result {
                Ok(_resp) => {
                    // Session cookies were set by the server_fn; navigate home.
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
            <div class="card bg-base-200 w-full max-w-sm shadow-xl">
                <div class="card-body">
                    <h1 class="card-title text-2xl justify-center">"Sign in to ZLayer"</h1>
                    <p class="text-sm opacity-70 text-center">"Enter your email and password."</p>

                    <form on:submit=on_submit class="mt-4 space-y-3">
                        <div class="form-control">
                            <label class="label" for="login-email">
                                <span class="label-text">"Email"</span>
                            </label>
                            <input
                                id="login-email"
                                type="email"
                                class="input input-bordered w-full"
                                autocomplete="email"
                                required
                                prop:value=email
                                on:input=move |ev| set_email.set(event_target_value(&ev))
                            />
                        </div>

                        <div class="form-control">
                            <label class="label" for="login-password">
                                <span class="label-text">"Password"</span>
                            </label>
                            <input
                                id="login-password"
                                type="password"
                                class="input input-bordered w-full"
                                autocomplete="current-password"
                                required
                                prop:value=password
                                on:input=move |ev| set_password.set(event_target_value(&ev))
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
                            {move || if submitting.get() { "Signing in…" } else { "Sign in" }}
                        </button>
                    </form>
                </div>
            </div>
        </div>
    }
}

/// Strip internal error noise for display. Server errors already carry a
/// human-readable prefix like `"Login failed (401): Invalid credentials"`,
/// so we surface that directly; pure network errors get a softer wording.
fn humanize_error(raw: &str) -> String {
    if raw.contains("Login failed") {
        raw.to_string()
    } else if raw.to_ascii_lowercase().contains("network") {
        "Could not reach the server. Check your connection and try again.".to_string()
    } else {
        raw.to_string()
    }
}
