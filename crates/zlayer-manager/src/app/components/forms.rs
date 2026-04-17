//! Shared form + modal primitives used by Manager pages.
//!
//! These components wrap DaisyUI semantic classes only — they never pick
//! hex colors. The goal is to keep page components focused on their data
//! logic, not on modal plumbing.

use std::collections::HashMap;

use leptos::ev::SubmitEvent;
use leptos::prelude::*;

/// A confirm/cancel modal. Shown when `open()` is true.
///
/// The caller owns the "submitting" flag — useful for disabling the
/// confirm button while the async mutation is in-flight.
///
/// `title` and `message` accept either a plain `&str`/`String` or a
/// reactive `Signal<String>` via `#[prop(into)]`. Callers that need to
/// change the copy based on a selected item (e.g. "Delete 'foo'?")
/// pass `Signal::derive(|| ...)` so the rendered text stays in sync
/// with the reactive state; static copy just uses a string literal.
#[component]
pub fn ConfirmDeleteModal(
    /// Controls visibility. Modal renders as `modal modal-open` when true.
    open: ReadSignal<bool>,
    /// Setter; Cancel clears this and the modal closes.
    set_open: WriteSignal<bool>,
    /// Bold title shown at the top of the dialog.
    #[prop(into)]
    title: Signal<String>,
    /// Body copy — typically "Delete foo? This cannot be undone."
    #[prop(into)]
    message: Signal<String>,
    /// Override the confirm-button label. Defaults to "Delete".
    #[prop(optional, into)]
    confirm_label: Option<String>,
    /// Fired when the user clicks the confirm button.
    on_confirm: Callback<()>,
    /// While true, the confirm button is disabled and shows a spinner.
    submitting: ReadSignal<bool>,
) -> impl IntoView {
    let confirm_label = confirm_label.unwrap_or_else(|| "Delete".to_string());
    let label_submit = confirm_label.clone();
    let label_idle = confirm_label;

    view! {
        <dialog class=move || if open.get() { "modal modal-open" } else { "modal" }>
            <div class="modal-box">
                <h3 class="font-bold text-lg mb-3">{move || title.get()}</h3>
                <p class="text-sm opacity-80">{move || message.get()}</p>
                <div class="modal-action">
                    <button
                        type="button"
                        class="btn btn-ghost"
                        on:click=move |_| set_open.set(false)
                    >
                        "Cancel"
                    </button>
                    <button
                        type="button"
                        class="btn btn-error"
                        disabled=move || submitting.get()
                        on:click=move |_| on_confirm.run(())
                    >
                        {
                            let s = label_submit.clone();
                            let i = label_idle.clone();
                            move || if submitting.get() {
                                format!("{s}…")
                            } else {
                                i.clone()
                            }
                        }
                    </button>
                </div>
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

/// A single-field text input modal. Used for renames, short-label edits,
/// etc. On submit, calls `on_save` with the trimmed non-empty value.
#[component]
pub fn TextFieldModal(
    /// Visibility control.
    open: ReadSignal<bool>,
    /// Setter; Cancel clears this.
    set_open: WriteSignal<bool>,
    /// Modal title.
    #[prop(into)]
    title: String,
    /// Label for the single input field.
    #[prop(into)]
    field_label: String,
    /// Optional pre-filled value when opening the modal.
    #[prop(optional)]
    initial_value: Option<String>,
    /// Called with the final value when the user submits.
    on_save: Callback<String>,
    /// In-flight mutation flag — disables the submit button.
    submitting: ReadSignal<bool>,
    /// Optional error banner above the form — typically the server's
    /// humanised error message.
    error: ReadSignal<Option<String>>,
) -> impl IntoView {
    let initial = initial_value.unwrap_or_default();
    let (value, set_value) = signal(initial.clone());

    // When the modal re-opens we want the initial value to re-seed; a
    // `RwSignal` effect accomplishes that by reacting to `open` flipping
    // true.
    Effect::new({
        let initial = initial.clone();
        move |_| {
            if open.get() {
                set_value.set(initial.clone());
            }
        }
    });

    let on_submit = move |ev: SubmitEvent| {
        ev.prevent_default();
        if submitting.get() {
            return;
        }
        let v = value.get();
        if v.trim().is_empty() {
            return;
        }
        on_save.run(v);
    };

    view! {
        <dialog class=move || if open.get() { "modal modal-open" } else { "modal" }>
            <div class="modal-box">
                <h3 class="font-bold text-lg mb-3">{title}</h3>
                {move || {
                    error
                        .get()
                        .map(|msg| view! { <div class="alert alert-error text-sm py-2"><span>{msg}</span></div> })
                }}
                <form on:submit=on_submit class="space-y-3 mt-2">
                    <div class="form-control">
                        <label class="label">
                            <span class="label-text">{field_label}</span>
                        </label>
                        <input
                            type="text"
                            class="input input-bordered w-full"
                            required
                            prop:value=value
                            on:input=move |ev| set_value.set(event_target_value(&ev))
                        />
                    </div>
                    <div class="modal-action">
                        <button
                            type="button"
                            class="btn btn-ghost"
                            on:click=move |_| set_open.set(false)
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

/// Labelled form-field wrapper with optional hint + error text.
///
/// The `children` slot is the actual input/select/textarea — this just
/// handles the DaisyUI `form-control` + `label` shell and the below-input
/// hint/error line.
#[component]
pub fn FormField(
    /// Field label text.
    #[prop(into)]
    label: String,
    /// Optional hint shown in `label-text-alt` style.
    #[prop(optional, into)]
    hint: Option<String>,
    /// Optional error message; when present, styles the hint line as
    /// `text-error`.
    #[prop(optional, into)]
    error: Option<String>,
    /// The actual input element(s).
    children: Children,
) -> impl IntoView {
    let hint_view = hint.map(|h| view! { <span class="label-text-alt opacity-70">{h}</span> });
    let error_view = error.map(|e| {
        view! {
            <label class="label">
                <span class="label-text-alt text-error">{e}</span>
            </label>
        }
    });

    view! {
        <div class="form-control">
            <label class="label">
                <span class="label-text">{label}</span>
                {hint_view}
            </label>
            {children()}
            {error_view}
        </div>
    }
}

/// A horizontal tab bar. Each tab is `(value, label)`; clicking one writes
/// that value into `selected`.
///
/// When `badge_counts` is provided, a small count badge is appended to tabs
/// whose key matches an entry in the map.
#[component]
#[allow(clippy::implicit_hasher)] // Leptos component macros don't carry through `BuildHasher` generics.
pub fn TabBar(
    /// (value, label) pairs — value is the stable key, label is the
    /// human-readable text.
    tabs: Vec<(String, String)>,
    /// Currently selected value.
    selected: RwSignal<String>,
    /// Optional per-tab badge counts.
    #[prop(optional)]
    badge_counts: Option<HashMap<String, usize>>,
) -> impl IntoView {
    let badges = badge_counts.unwrap_or_default();
    let rendered = tabs
        .into_iter()
        .map(|(value, label)| {
            let value_for_click = value.clone();
            let value_for_class = value.clone();
            let value_for_badge = value.clone();
            let count = badges.get(&value).copied();
            view! {
                <a
                    role="tab"
                    class="tab"
                    class:tab-active=move || selected.get() == value_for_class
                    on:click={
                        let v = value_for_click.clone();
                        move |_| selected.set(v.clone())
                    }
                >
                    {label}
                    {count.map(|n| {
                        let _ = &value_for_badge;
                        view! { <span class="badge badge-sm ml-2">{n}</span> }
                    })}
                </a>
            }
        })
        .collect_view();

    view! {
        <div role="tablist" class="tabs tabs-boxed">
            {rendered}
        </div>
    }
}

/// Prev/Next pagination with a page indicator. `page` is zero-indexed.
#[component]
pub fn Pagination(
    /// Mutable current page (zero-indexed).
    page: RwSignal<usize>,
    /// Total page count; when zero, the component renders nothing.
    total_pages: Signal<usize>,
    /// When true, both buttons are disabled regardless of page.
    disabled: Signal<bool>,
) -> impl IntoView {
    view! {
        {move || {
            let total = total_pages.get();
            if total == 0 {
                return view! { <span></span> }.into_any();
            }
            let current = page.get();
            let on_first = current == 0;
            let on_last = current + 1 >= total;
            let is_disabled = disabled.get();
            view! {
                <div class="flex items-center gap-2">
                    <div class="btn-group">
                        <button
                            class="btn btn-sm"
                            disabled=is_disabled || on_first
                            on:click=move |_| {
                                let c = page.get();
                                if c > 0 {
                                    page.set(c - 1);
                                }
                            }
                        >
                            "Prev"
                        </button>
                        <button
                            class="btn btn-sm"
                            disabled=is_disabled || on_last
                            on:click=move |_| {
                                let c = page.get();
                                if c + 1 < total_pages.get_untracked() {
                                    page.set(c + 1);
                                }
                            }
                        >
                            "Next"
                        </button>
                    </div>
                    <span class="text-sm opacity-70">
                        {format!("{} of {}", current + 1, total)}
                    </span>
                </div>
            }
            .into_any()
        }}
    }
}
