//! `/notifiers` — CRUD + test for notification channels.
//!
//! All authenticated users can browse the list; only admins can mutate
//! (the daemon enforces admin-only on every mutation). Each row exposes
//! a **Test** button which drives `POST /api/v1/notifiers/{id}/test` and
//! surfaces a DaisyUI `alert-success`/`alert-error` depending on the
//! daemon's `NotifierTestResult.success` flag. Upstream failures come
//! back as HTTP 200 + `success: false`, matching the
//! Slack/Discord/Webhook convention, and are shown as errors in the UI.
//!
//! The new/edit modals render a discriminated-union form: the user picks
//! a **kind** (slack/discord/webhook/smtp) and the form shows only the
//! variant-specific fields. Switching variants clears the other
//! variant's inputs so we never accidentally submit stale state. The
//! final payload is built via a `match` on the selected variant.

use leptos::ev::SubmitEvent;
use leptos::prelude::*;
use leptos::task::spawn_local;
use std::collections::HashMap;

use crate::app::server_fns::{
    manager_create_notifier, manager_delete_notifier, manager_list_notifiers,
    manager_test_notifier, manager_update_notifier,
};
use crate::app::util::errors::format_server_error;
use crate::wire::notifiers::{NotifierTestResult, WireNotifier, WireNotifierConfig};

/// Modal mode — a New modal with empty fields vs. an Edit modal
/// pre-seeded from an existing notifier. The `Edit` variant boxes its
/// payload so the enum stays small enough for clippy's
/// `large_enum_variant` lint (and so an unused `New` doesn't carry a
/// couple-hundred-byte padding).
#[derive(Clone)]
enum FormMode {
    /// Empty form, create a new notifier on submit.
    New,
    /// Pre-seeded form; update the provided notifier on submit.
    Edit(Box<WireNotifier>),
}

/// Notifiers management page.
#[component]
#[allow(clippy::too_many_lines)] // view! DSL + multiple modals
pub fn Notifiers() -> impl IntoView {
    let list = Resource::new(|| (), |()| async move { manager_list_notifiers().await });
    let (error, set_error) = signal(None::<String>);

    // RwSignal<Option<…>>: None = closed, Some = open with target notifier.
    let form_mode = RwSignal::new(None::<FormMode>);
    let test_target = RwSignal::new(None::<WireNotifier>);
    let delete_target = RwSignal::new(None::<WireNotifier>);

    let refetch = move || list.refetch();

    view! {
        <div class="p-6 space-y-4">
            <div class="flex justify-between items-center">
                <h1 class="text-2xl font-bold">"Notifiers"</h1>
                <button
                    class="btn btn-primary btn-sm"
                    on:click=move |_| form_mode.set(Some(FormMode::New))
                >
                    "New notifier"
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
                                            {format!("Failed to load notifiers: {}", format_server_error(&e))}
                                        </div>
                                    }
                                    .into_any(),
                                    Ok(items) => render_table(
                                        items,
                                        form_mode,
                                        test_target,
                                        delete_target,
                                        set_error,
                                        refetch,
                                    )
                                    .into_any(),
                                })
                        }}
                    </Suspense>
                </div>
            </div>

            <NotifierFormModal
                target=form_mode
                set_error=set_error
                on_success=move || {
                    refetch();
                }
            />

            <TestNotifierModal
                target=test_target
            />

            <DeleteNotifierModal
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
    list: Vec<WireNotifier>,
    form_mode: RwSignal<Option<FormMode>>,
    test_target: RwSignal<Option<WireNotifier>>,
    delete_target: RwSignal<Option<WireNotifier>>,
    set_error: WriteSignal<Option<String>>,
    refetch: impl Fn() + Copy + Send + Sync + 'static,
) -> impl IntoView {
    if list.is_empty() {
        return view! { <div class="p-4 opacity-70">"(no notifiers)"</div> }.into_any();
    }

    view! {
        <table class="table table-zebra">
            <thead>
                <tr>
                    <th>"Name"</th>
                    <th>"Type"</th>
                    <th>"Enabled"</th>
                    <th>"Updated"</th>
                    <th class="text-right">"Actions"</th>
                </tr>
            </thead>
            <tbody>
                {list
                    .into_iter()
                    .map(|n| render_row(&n, form_mode, test_target, delete_target, set_error, refetch))
                    .collect_view()}
            </tbody>
        </table>
    }
    .into_any()
}

/// Return a DaisyUI badge class colored by notifier kind. Uses semantic
/// classes only (no hand-picked colors).
fn kind_badge_class(kind: &str) -> &'static str {
    match kind {
        "slack" => "badge badge-primary",
        "discord" => "badge badge-secondary",
        "webhook" => "badge badge-accent",
        "smtp" => "badge badge-info",
        _ => "badge badge-ghost",
    }
}

fn render_row(
    n: &WireNotifier,
    form_mode: RwSignal<Option<FormMode>>,
    test_target: RwSignal<Option<WireNotifier>>,
    delete_target: RwSignal<Option<WireNotifier>>,
    set_error: WriteSignal<Option<String>>,
    refetch: impl Fn() + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let kind_class = kind_badge_class(&n.kind);
    let kind_label = n.kind.clone();

    // Toggle-enabled closure: PATCH with enabled flipped.
    let toggle_notifier = n.clone();
    let toggle_enabled = move |_| {
        let id = toggle_notifier.id.clone();
        let next = !toggle_notifier.enabled;
        spawn_local(async move {
            let result = manager_update_notifier(id, None, Some(next), None).await;
            match result {
                Ok(_) => refetch(),
                Err(e) => {
                    set_error.set(Some(format!("Update failed: {}", format_server_error(&e))));
                }
            }
        });
    };
    let enabled_flag = n.enabled;
    let enabled_badge = if enabled_flag {
        "badge badge-success"
    } else {
        "badge badge-ghost"
    };
    let enabled_text = if enabled_flag { "yes" } else { "no" };

    let edit_notifier = n.clone();
    let test_notifier = n.clone();
    let delete_notifier = n.clone();

    view! {
        <tr>
            <td class="font-mono text-sm">{n.name.clone()}</td>
            <td><span class=kind_class>{kind_label}</span></td>
            <td>
                <button
                    class="btn btn-xs btn-ghost p-0"
                    title="Toggle enabled"
                    on:click=toggle_enabled
                >
                    <span class=enabled_badge>{enabled_text}</span>
                </button>
            </td>
            <td class="text-xs opacity-70">{n.updated_at.clone()}</td>
            <td class="text-right space-x-1">
                <button
                    class="btn btn-xs btn-ghost"
                    on:click=move |_| test_target.set(Some(test_notifier.clone()))
                >
                    "Test"
                </button>
                <button
                    class="btn btn-xs btn-ghost"
                    on:click=move |_| form_mode.set(Some(FormMode::Edit(Box::new(edit_notifier.clone()))))
                >
                    "Edit"
                </button>
                <button
                    class="btn btn-xs btn-error"
                    on:click=move |_| delete_target.set(Some(delete_notifier.clone()))
                >
                    "Delete"
                </button>
            </td>
        </tr>
    }
}

// ---- Shared create/edit modal -------------------------------------------

/// Serialize a `HashMap<String, String>` into the `Key: Value\n…` textarea
/// representation used by the Webhook headers input.
fn headers_to_textarea(headers: &HashMap<String, String>) -> String {
    let mut entries: Vec<(&String, &String)> = headers.iter().collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));
    entries
        .into_iter()
        .map(|(k, v)| format!("{k}: {v}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Parse the `Key: Value\n…` textarea back into a `HashMap`. Empty lines
/// are skipped; lines without a colon cause an `Err` with a readable
/// message so the form can surface the failure instead of silently
/// swallowing the entry.
fn parse_headers_textarea(raw: &str) -> Result<HashMap<String, String>, String> {
    let mut out = HashMap::new();
    for (idx, line) in raw.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let (k, v) = line
            .split_once(':')
            .ok_or_else(|| format!("Line {} is missing a ':' separator", idx + 1))?;
        out.insert(k.trim().to_string(), v.trim().to_string());
    }
    Ok(out)
}

#[component]
#[allow(clippy::too_many_lines)]
fn NotifierFormModal(
    target: RwSignal<Option<FormMode>>,
    set_error: WriteSignal<Option<String>>,
    on_success: impl Fn() + Copy + Send + Sync + 'static,
) -> impl IntoView {
    // Common fields.
    let (name, set_name) = signal(String::new());
    let (enabled, set_enabled) = signal(true);
    let variant = RwSignal::new(String::from("slack"));

    // Per-variant field state. Initialised blank and re-seeded when a
    // target is opened.
    let (webhook_url, set_webhook_url) = signal(String::new());

    let (webhook_req_url, set_webhook_req_url) = signal(String::new());
    let (webhook_req_method, set_webhook_req_method) = signal(String::from("POST"));
    let (webhook_req_headers, set_webhook_req_headers) = signal(String::new());

    let (smtp_host, set_smtp_host) = signal(String::new());
    let (smtp_port, set_smtp_port) = signal(String::from("587"));
    let (smtp_username, set_smtp_username) = signal(String::new());
    let (smtp_password, set_smtp_password) = signal(String::new());
    let (smtp_from, set_smtp_from) = signal(String::new());
    let (smtp_to, set_smtp_to) = signal(String::new());

    let (submitting, set_submitting) = signal(false);
    let (form_error, set_form_error) = signal(None::<String>);

    let reset_all = move || {
        set_name.set(String::new());
        set_enabled.set(true);
        variant.set(String::from("slack"));
        set_webhook_url.set(String::new());
        set_webhook_req_url.set(String::new());
        set_webhook_req_method.set(String::from("POST"));
        set_webhook_req_headers.set(String::new());
        set_smtp_host.set(String::new());
        set_smtp_port.set(String::from("587"));
        set_smtp_username.set(String::new());
        set_smtp_password.set(String::new());
        set_smtp_from.set(String::new());
        set_smtp_to.set(String::new());
        set_form_error.set(None);
    };

    // Seed the form whenever the target changes. In Edit mode we read
    // the existing notifier; in New mode we reset to blanks.
    Effect::new(move |_| match target.get() {
        Some(FormMode::Edit(n)) => {
            set_name.set(n.name.clone());
            set_enabled.set(n.enabled);
            variant.set(n.kind.clone());
            // Reset non-relevant variant fields so switching back and
            // forth doesn't leak.
            set_webhook_url.set(String::new());
            set_webhook_req_url.set(String::new());
            set_webhook_req_method.set(String::from("POST"));
            set_webhook_req_headers.set(String::new());
            set_smtp_host.set(String::new());
            set_smtp_port.set(String::from("587"));
            set_smtp_username.set(String::new());
            set_smtp_password.set(String::new());
            set_smtp_from.set(String::new());
            set_smtp_to.set(String::new());
            set_form_error.set(None);
            match &n.config {
                WireNotifierConfig::Slack { webhook_url: url }
                | WireNotifierConfig::Discord { webhook_url: url } => {
                    set_webhook_url.set(url.clone());
                }
                WireNotifierConfig::Webhook {
                    url,
                    method,
                    headers,
                } => {
                    set_webhook_req_url.set(url.clone());
                    set_webhook_req_method
                        .set(method.clone().unwrap_or_else(|| String::from("POST")));
                    if let Some(h) = headers.as_ref() {
                        set_webhook_req_headers.set(headers_to_textarea(h));
                    }
                }
                WireNotifierConfig::Smtp {
                    host,
                    port,
                    username,
                    password,
                    from,
                    to,
                } => {
                    set_smtp_host.set(host.clone());
                    set_smtp_port.set(port.to_string());
                    set_smtp_username.set(username.clone());
                    set_smtp_password.set(password.clone());
                    set_smtp_from.set(from.clone());
                    set_smtp_to.set(to.join("\n"));
                }
            }
        }
        Some(FormMode::New) => {
            reset_all();
        }
        None => {}
    });

    // When the user switches variant, clear the *other* variants' inputs
    // so a blank-but-lingering value never slips into the payload.
    let variant_changed = move |ev: leptos::ev::Event| {
        let v = event_target_value(&ev);
        variant.set(v.clone());
        match v.as_str() {
            "slack" | "discord" => {
                set_webhook_req_url.set(String::new());
                set_webhook_req_method.set(String::from("POST"));
                set_webhook_req_headers.set(String::new());
                set_smtp_host.set(String::new());
                set_smtp_port.set(String::from("587"));
                set_smtp_username.set(String::new());
                set_smtp_password.set(String::new());
                set_smtp_from.set(String::new());
                set_smtp_to.set(String::new());
            }
            "webhook" => {
                set_webhook_url.set(String::new());
                set_smtp_host.set(String::new());
                set_smtp_port.set(String::from("587"));
                set_smtp_username.set(String::new());
                set_smtp_password.set(String::new());
                set_smtp_from.set(String::new());
                set_smtp_to.set(String::new());
            }
            "smtp" => {
                set_webhook_url.set(String::new());
                set_webhook_req_url.set(String::new());
                set_webhook_req_method.set(String::from("POST"));
                set_webhook_req_headers.set(String::new());
            }
            _ => {}
        }
    };

    let close = move || {
        target.set(None);
        reset_all();
    };

    // Build a `WireNotifierConfig` from the current form state. Returns
    // `Err(msg)` when the variant-specific validation fails so the form
    // can surface the problem inline rather than hitting the daemon.
    let build_config = move || -> Result<WireNotifierConfig, String> {
        match variant.get().as_str() {
            "slack" => {
                let url = webhook_url.get().trim().to_string();
                if url.is_empty() {
                    return Err("Slack webhook URL is required.".into());
                }
                Ok(WireNotifierConfig::Slack { webhook_url: url })
            }
            "discord" => {
                let url = webhook_url.get().trim().to_string();
                if url.is_empty() {
                    return Err("Discord webhook URL is required.".into());
                }
                Ok(WireNotifierConfig::Discord { webhook_url: url })
            }
            "webhook" => {
                let url = webhook_req_url.get().trim().to_string();
                if url.is_empty() {
                    return Err("Webhook URL is required.".into());
                }
                let method = {
                    let m = webhook_req_method.get();
                    let trimmed = m.trim();
                    if trimmed.is_empty() {
                        None
                    } else {
                        Some(trimmed.to_string())
                    }
                };
                let headers_raw = webhook_req_headers.get();
                let headers = if headers_raw.trim().is_empty() {
                    None
                } else {
                    Some(parse_headers_textarea(&headers_raw)?)
                };
                Ok(WireNotifierConfig::Webhook {
                    url,
                    method,
                    headers,
                })
            }
            "smtp" => {
                let host = smtp_host.get().trim().to_string();
                if host.is_empty() {
                    return Err("SMTP host is required.".into());
                }
                let port: u16 =
                    smtp_port.get().trim().parse().map_err(|_| {
                        "SMTP port must be a number between 1 and 65535.".to_string()
                    })?;
                let username = smtp_username.get().trim().to_string();
                if username.is_empty() {
                    return Err("SMTP username is required.".into());
                }
                let password = smtp_password.get();
                if password.is_empty() {
                    return Err("SMTP password is required.".into());
                }
                let from = smtp_from.get().trim().to_string();
                if from.is_empty() {
                    return Err("SMTP 'from' address is required.".into());
                }
                let to: Vec<String> = smtp_to
                    .get()
                    .lines()
                    .map(|l| l.trim().to_string())
                    .filter(|l| !l.is_empty())
                    .collect();
                if to.is_empty() {
                    return Err("At least one SMTP recipient is required.".into());
                }
                Ok(WireNotifierConfig::Smtp {
                    host,
                    port,
                    username,
                    password,
                    from,
                    to,
                })
            }
            other => Err(format!("Unknown notifier kind: {other}")),
        }
    };

    let on_submit = move |ev: SubmitEvent| {
        ev.prevent_default();
        if submitting.get() {
            return;
        }
        let name_val = name.get().trim().to_string();
        if name_val.is_empty() {
            set_form_error.set(Some("Name is required.".into()));
            return;
        }
        let config = match build_config() {
            Ok(cfg) => cfg,
            Err(msg) => {
                set_form_error.set(Some(msg));
                return;
            }
        };
        let enabled_val = enabled.get();
        let mode = target.get();
        set_submitting.set(true);
        spawn_local(async move {
            let result = match mode {
                Some(FormMode::Edit(existing)) => manager_update_notifier(
                    existing.id,
                    Some(name_val),
                    Some(enabled_val),
                    Some(config),
                )
                .await
                .map(|_| ()),
                _ => manager_create_notifier(name_val, enabled_val, config)
                    .await
                    .map(|_| ()),
            };
            set_submitting.set(false);
            match result {
                Ok(()) => {
                    target.set(None);
                    reset_all();
                    on_success();
                }
                Err(e) => {
                    set_error.set(Some(format!("Save failed: {}", format_server_error(&e))));
                }
            }
        });
    };

    let title = move || match target.get() {
        Some(FormMode::Edit(n)) => format!("Edit notifier: {}", n.name),
        _ => "New notifier".to_string(),
    };

    view! {
        <dialog class=move || {
            if target.with(Option::is_some) {
                "modal modal-open"
            } else {
                "modal"
            }
        }>
            <div class="modal-box max-w-2xl">
                <h3 class="font-bold text-lg mb-3">{title}</h3>

                {move || form_error.get().map(|msg| view! {
                    <div class="alert alert-error text-sm py-2 mb-2"><span>{msg}</span></div>
                })}

                <form on:submit=on_submit class="space-y-3">
                    <div class="form-control">
                        <label class="label"><span class="label-text">"Name"</span></label>
                        <input
                            type="text"
                            class="input input-bordered w-full"
                            required
                            placeholder="deploy-alerts"
                            prop:value=name
                            on:input=move |ev| set_name.set(event_target_value(&ev))
                        />
                    </div>

                    <div class="form-control">
                        <label class="label cursor-pointer justify-start gap-3">
                            <input
                                type="checkbox"
                                class="checkbox checkbox-primary"
                                prop:checked=enabled
                                on:change=move |ev| set_enabled.set(event_target_checked(&ev))
                            />
                            <span class="label-text">"Enabled"</span>
                        </label>
                    </div>

                    <div class="form-control">
                        <label class="label"><span class="label-text">"Type"</span></label>
                        <select
                            class="select select-bordered w-full"
                            prop:value=variant
                            on:change=variant_changed
                        >
                            <option value="slack">"Slack"</option>
                            <option value="discord">"Discord"</option>
                            <option value="webhook">"Webhook"</option>
                            <option value="smtp">"SMTP (email)"</option>
                        </select>
                    </div>

                    // Variant-specific fields.
                    {move || match variant.get().as_str() {
                        "slack" | "discord" => {
                            let label = if variant.get() == "discord" {
                                "Discord webhook URL"
                            } else {
                                "Slack webhook URL"
                            };
                            view! {
                                <div class="form-control">
                                    <label class="label">
                                        <span class="label-text">{label}</span>
                                    </label>
                                    <input
                                        type="url"
                                        class="input input-bordered w-full font-mono text-sm"
                                        placeholder="https://hooks.example.com/…"
                                        prop:value=webhook_url
                                        on:input=move |ev| set_webhook_url.set(event_target_value(&ev))
                                    />
                                </div>
                            }
                            .into_any()
                        }
                        "webhook" => view! {
                            <div class="space-y-3">
                                <div class="form-control">
                                    <label class="label"><span class="label-text">"Target URL"</span></label>
                                    <input
                                        type="url"
                                        class="input input-bordered w-full font-mono text-sm"
                                        placeholder="https://example.com/hook"
                                        prop:value=webhook_req_url
                                        on:input=move |ev| set_webhook_req_url.set(event_target_value(&ev))
                                    />
                                </div>
                                <div class="form-control">
                                    <label class="label"><span class="label-text">"HTTP method"</span></label>
                                    <select
                                        class="select select-bordered w-full"
                                        prop:value=webhook_req_method
                                        on:change=move |ev| set_webhook_req_method.set(event_target_value(&ev))
                                    >
                                        <option value="GET">"GET"</option>
                                        <option value="POST">"POST"</option>
                                        <option value="PUT">"PUT"</option>
                                        <option value="PATCH">"PATCH"</option>
                                    </select>
                                </div>
                                <div class="form-control">
                                    <label class="label">
                                        <span class="label-text">"Extra headers (optional)"</span>
                                        <span class="label-text-alt opacity-70">"One per line: Key: Value"</span>
                                    </label>
                                    <textarea
                                        class="textarea textarea-bordered w-full font-mono text-sm h-24"
                                        placeholder="Authorization: Bearer token123"
                                        prop:value=webhook_req_headers
                                        on:input=move |ev| set_webhook_req_headers.set(event_target_value(&ev))
                                    ></textarea>
                                </div>
                            </div>
                        }
                        .into_any(),
                        "smtp" => view! {
                            <div class="grid grid-cols-2 gap-3">
                                <div class="form-control">
                                    <label class="label"><span class="label-text">"SMTP host"</span></label>
                                    <input
                                        type="text"
                                        class="input input-bordered w-full"
                                        placeholder="smtp.example.com"
                                        prop:value=smtp_host
                                        on:input=move |ev| set_smtp_host.set(event_target_value(&ev))
                                    />
                                </div>
                                <div class="form-control">
                                    <label class="label"><span class="label-text">"Port"</span></label>
                                    <input
                                        type="number"
                                        min="1"
                                        max="65535"
                                        class="input input-bordered w-full"
                                        placeholder="587"
                                        prop:value=smtp_port
                                        on:input=move |ev| set_smtp_port.set(event_target_value(&ev))
                                    />
                                </div>
                                <div class="form-control">
                                    <label class="label"><span class="label-text">"Username"</span></label>
                                    <input
                                        type="text"
                                        class="input input-bordered w-full"
                                        prop:value=smtp_username
                                        on:input=move |ev| set_smtp_username.set(event_target_value(&ev))
                                    />
                                </div>
                                <div class="form-control">
                                    <label class="label"><span class="label-text">"Password"</span></label>
                                    <input
                                        type="password"
                                        class="input input-bordered w-full"
                                        prop:value=smtp_password
                                        on:input=move |ev| set_smtp_password.set(event_target_value(&ev))
                                    />
                                </div>
                                <div class="form-control col-span-2">
                                    <label class="label"><span class="label-text">"From address"</span></label>
                                    <input
                                        type="email"
                                        class="input input-bordered w-full"
                                        placeholder="alerts@example.com"
                                        prop:value=smtp_from
                                        on:input=move |ev| set_smtp_from.set(event_target_value(&ev))
                                    />
                                </div>
                                <div class="form-control col-span-2">
                                    <label class="label">
                                        <span class="label-text">"Recipients"</span>
                                        <span class="label-text-alt opacity-70">"One per line"</span>
                                    </label>
                                    <textarea
                                        class="textarea textarea-bordered w-full font-mono text-sm h-20"
                                        placeholder="admin@example.com"
                                        prop:value=smtp_to
                                        on:input=move |ev| set_smtp_to.set(event_target_value(&ev))
                                    ></textarea>
                                </div>
                            </div>
                        }
                        .into_any(),
                        _ => view! { <div></div> }.into_any(),
                    }}

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
            <form
                method="dialog"
                class="modal-backdrop"
                on:submit=move |_| close()
            >
                <button>"close"</button>
            </form>
        </dialog>
    }
}

// ---- Test modal ----------------------------------------------------------

#[component]
fn TestNotifierModal(target: RwSignal<Option<WireNotifier>>) -> impl IntoView {
    let (running, set_running) = signal(false);
    let result = RwSignal::new(None::<NotifierTestResult>);
    let error = RwSignal::new(None::<String>);

    // Kick off the test whenever a new target is opened.
    Effect::new(move |_| {
        if let Some(n) = target.get() {
            let id = n.id.clone();
            set_running.set(true);
            result.set(None);
            error.set(None);
            spawn_local(async move {
                match manager_test_notifier(id).await {
                    Ok(r) => result.set(Some(r)),
                    Err(e) => error.set(Some(format_server_error(&e))),
                }
                set_running.set(false);
            });
        }
    });

    let close = move || {
        target.set(None);
        result.set(None);
        error.set(None);
        set_running.set(false);
    };

    view! {
        <dialog class=move || {
            if target.with(Option::is_some) {
                "modal modal-open"
            } else {
                "modal"
            }
        }>
            <div class="modal-box">
                <h3 class="font-bold text-lg mb-3">
                    {move || {
                        target
                            .get()
                            .map(|n| format!("Test notifier: {}", n.name))
                            .unwrap_or_default()
                    }}
                </h3>

                {move || {
                    if running.get() {
                        view! {
                            <div class="flex items-center gap-3 p-2">
                                <span class="loading loading-spinner loading-md"></span>
                                <span>"Sending test notification…"</span>
                            </div>
                        }
                        .into_any()
                    } else if let Some(err) = error.get() {
                        view! {
                            <div class="alert alert-error">
                                <span>{err}</span>
                            </div>
                        }
                        .into_any()
                    } else if let Some(r) = result.get() {
                        let cls = if r.success { "alert alert-success" } else { "alert alert-error" };
                        view! {
                            <div class=cls>
                                <span>{r.message}</span>
                            </div>
                        }
                        .into_any()
                    } else {
                        view! { <div></div> }.into_any()
                    }
                }}

                <div class="modal-action">
                    <button
                        type="button"
                        class="btn btn-ghost"
                        on:click=move |_| close()
                    >
                        "Close"
                    </button>
                </div>
            </div>
            <form
                method="dialog"
                class="modal-backdrop"
                on:submit=move |_| close()
            >
                <button>"close"</button>
            </form>
        </dialog>
    }
}

// ---- Delete modal --------------------------------------------------------

#[component]
fn DeleteNotifierModal(
    target: RwSignal<Option<WireNotifier>>,
    set_error: WriteSignal<Option<String>>,
    on_success: impl Fn() + Copy + Send + Sync + 'static,
) -> impl IntoView {
    let (submitting, set_submitting) = signal(false);

    let close = move || target.set(None);

    let confirm = move |_| {
        let Some(n) = target.get() else {
            return;
        };
        set_submitting.set(true);
        spawn_local(async move {
            let result = manager_delete_notifier(n.id.clone()).await;
            set_submitting.set(false);
            match result {
                Ok(()) => {
                    close();
                    on_success();
                }
                Err(e) => {
                    set_error.set(Some(format!("Delete failed: {}", format_server_error(&e))));
                }
            }
        });
    };

    view! {
        <dialog class=move || {
            if target.with(Option::is_some) {
                "modal modal-open"
            } else {
                "modal"
            }
        }>
            <div class="modal-box">
                <h3 class="font-bold text-lg mb-3">"Delete notifier"</h3>
                <p>
                    {move || {
                        target
                            .get()
                            .map(|n| format!("Delete notifier '{}'? This cannot be undone.", n.name))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headers_round_trip() {
        let mut hm = HashMap::new();
        hm.insert("X-Token".to_string(), "abc".to_string());
        hm.insert("Authorization".to_string(), "Bearer 123".to_string());

        let text = headers_to_textarea(&hm);
        // Sorted alphabetically.
        assert!(text.contains("Authorization: Bearer 123"));
        assert!(text.contains("X-Token: abc"));

        let parsed = parse_headers_textarea(&text).unwrap();
        assert_eq!(parsed, hm);
    }

    #[test]
    fn headers_ignore_blank_lines() {
        let text = "\n  \n  Foo: bar  \n\n  Baz: qux \n";
        let parsed = parse_headers_textarea(text).unwrap();
        assert_eq!(parsed.get("Foo").unwrap(), "bar");
        assert_eq!(parsed.get("Baz").unwrap(), "qux");
        assert_eq!(parsed.len(), 2);
    }

    #[test]
    fn headers_reject_missing_colon() {
        let err = parse_headers_textarea("valid: ok\nnotvalid\n").unwrap_err();
        assert!(err.contains("Line 2"), "got: {err}");
    }

    #[test]
    fn kind_badge_all_variants() {
        assert_eq!(kind_badge_class("slack"), "badge badge-primary");
        assert_eq!(kind_badge_class("discord"), "badge badge-secondary");
        assert_eq!(kind_badge_class("webhook"), "badge badge-accent");
        assert_eq!(kind_badge_class("smtp"), "badge badge-info");
        assert_eq!(kind_badge_class("unknown"), "badge badge-ghost");
    }
}
