//! `/audit` — admin-only, read-only audit log browser.
//!
//! Renders a filterable, paginated table of audit entries served by the
//! daemon's `GET /api/v1/audit`. The daemon response is a bare
//! `Vec<AuditEntry>` (no `{entries, total}` envelope), so pagination is
//! client-side: each fetch asks the daemon for `PER_PAGE + 1` rows at
//! `offset = page * PER_PAGE`, and the extra row (if present) is peeled
//! off and used as the signal "there is a next page". This keeps the
//! Prev/Next controls correct even when the total count is unknown.
//!
//! Filter bar fields:
//!   - user id (free-form substring, applied daemon-side as `user=`)
//!   - resource kind (free-form string — the daemon accepts any; the
//!     Manager doesn't constrain to a dropdown because new kinds get
//!     added without a client upgrade)
//!   - since / until (HTML `datetime-local` inputs; the browser emits
//!     `YYYY-MM-DDTHH:MM`, we append `:00Z` to turn them into RFC-3339
//!     UTC before sending — the daemon parses with `chrono::DateTime<Utc>`
//!     which requires a timezone suffix).
//!
//! The daemon's `limit` parameter caps the number of rows returned per
//! request. There is no `offset` parameter on the daemon's side yet, so
//! page > 0 fetches happen by over-sampling and client-side slicing: we
//! request `(page + 1) * PER_PAGE + 1` rows and drop the first
//! `page * PER_PAGE`. Since the daemon orders newest-first on a stable
//! time key, this is deterministic. The over-fetch caveat means deep
//! pagination through very large result sets is inefficient — but the
//! common admin flow is page 0-2 after a narrowing filter, so this is
//! acceptable until the daemon grows an explicit offset param.
//!
//! The Details column uses a `<details>/<summary>` disclosure so the JSON
//! payload is collapsed by default; tapping the summary reveals a
//! pretty-printed block.
//!
//! Admin gate: non-admin sessions see an `alert-error` "Admin access
//! required" banner and no server calls are issued. The backend
//! independently enforces `admin` role via `actor.require_admin()?` on
//! every request.

use leptos::ev::SubmitEvent;
use leptos::prelude::*;

use crate::app::auth_guard::CurrentUser;
use crate::app::components::forms::Pagination;
use crate::app::server_fns::manager_list_audit;
use crate::wire::audit::{WireAuditEntry, WireAuditFilter};

/// Page size for the audit table. A compromise: big enough that admins
/// don't page through single-digit results after an unrestrictive filter,
/// small enough that each fetch stays snappy even over a remote daemon.
const PER_PAGE: usize = 50;

/// Admin-only audit log page.
#[component]
#[allow(clippy::too_many_lines)] // view macro DSL + filter bar + table + pagination
pub fn Audit() -> impl IntoView {
    let current = use_context::<CurrentUser>();
    let is_admin = current.as_ref().is_some_and(CurrentUser::is_admin);

    if !is_admin {
        return view! {
            <div class="p-6">
                <div class="alert alert-error max-w-xl">
                    <div>
                        <h3 class="font-bold">"Admin access required"</h3>
                        <p class="text-sm opacity-80">
                            "The audit log is restricted to administrators."
                        </p>
                    </div>
                </div>
            </div>
        }
        .into_any();
    }

    // Filter form state. Each input binds to its own `RwSignal<String>`
    // and the user presses "Apply" to commit the filter into `applied` —
    // that way typing in the user-id box doesn't fire N daemon calls.
    let user_id_input: RwSignal<String> = RwSignal::new(String::new());
    let resource_kind_input: RwSignal<String> = RwSignal::new(String::new());
    let since_input: RwSignal<String> = RwSignal::new(String::new());
    let until_input: RwSignal<String> = RwSignal::new(String::new());

    let applied_user_id: RwSignal<String> = RwSignal::new(String::new());
    let applied_resource_kind: RwSignal<String> = RwSignal::new(String::new());
    // Stored as pre-rendered RFC-3339 strings (the conversion from
    // `datetime-local` happens at "Apply" time).
    let applied_since: RwSignal<String> = RwSignal::new(String::new());
    let applied_until: RwSignal<String> = RwSignal::new(String::new());

    let page: RwSignal<usize> = RwSignal::new(0);
    let (error, set_error) = signal(None::<String>);

    // Derived Resource: keyed on the applied filter tuple + page so it
    // refetches whenever Apply is clicked or the user pages forward /
    // backward. A filter change resets `page` to 0 (see the on-apply
    // handler); the refetch happens via the Resource's source signal.
    //
    // The "+1 probe row" trick: we ask for PER_PAGE + 1 rows so we can
    // tell whether a next page exists. After slicing off the probe row
    // we still expose at most PER_PAGE rows to the table.
    let audit_resource: Resource<Result<Vec<WireAuditEntry>, ServerFnError>> = Resource::new(
        move || {
            (
                applied_user_id.get(),
                applied_resource_kind.get(),
                applied_since.get(),
                applied_until.get(),
                page.get(),
            )
        },
        move |(uid, rk, since, until, p)| async move {
            // Daemon doesn't yet support offset; request enough rows to
            // cover page 0..p and slice client-side.
            let probe = PER_PAGE
                .saturating_mul(p.saturating_add(1))
                .saturating_add(1);
            let filter = WireAuditFilter {
                user_id: if uid.is_empty() { None } else { Some(uid) },
                resource_kind: if rk.is_empty() { None } else { Some(rk) },
                since: if since.is_empty() { None } else { Some(since) },
                until: if until.is_empty() { None } else { Some(until) },
                limit: Some(probe),
            };
            manager_list_audit(filter).await
        },
    );

    // Apply the current form values into `applied_*` and reset paging.
    let on_apply = move |ev: SubmitEvent| {
        ev.prevent_default();

        applied_user_id.set(user_id_input.get().trim().to_string());
        applied_resource_kind.set(resource_kind_input.get().trim().to_string());

        // datetime-local emits `YYYY-MM-DDTHH:MM` with no zone; tack on
        // `:00Z` so the daemon can parse via `DateTime<Utc>`. If the user
        // cleared the field we propagate an empty string.
        let since_raw = since_input.get();
        applied_since.set(datetime_local_to_rfc3339(&since_raw));
        let until_raw = until_input.get();
        applied_until.set(datetime_local_to_rfc3339(&until_raw));

        page.set(0);
    };

    // Clear every filter field and pagination.
    let on_clear = move |_| {
        user_id_input.set(String::new());
        resource_kind_input.set(String::new());
        since_input.set(String::new());
        until_input.set(String::new());
        applied_user_id.set(String::new());
        applied_resource_kind.set(String::new());
        applied_since.set(String::new());
        applied_until.set(String::new());
        page.set(0);
    };

    view! {
        <div class="p-6 space-y-4">
            <div class="flex justify-between items-center">
                <h1 class="text-2xl font-bold">"Audit log"</h1>
                <div class="text-sm opacity-70">
                    "Read-only. Admin access required."
                </div>
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

            // Filter bar.
            <form
                on:submit=on_apply
                class="card bg-base-200"
            >
                <div class="card-body p-4">
                    <div class="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-3">
                        <div class="form-control">
                            <label class="label py-1">
                                <span class="label-text">"User id"</span>
                            </label>
                            <input
                                type="text"
                                class="input input-bordered input-sm font-mono"
                                placeholder="uuid"
                                prop:value=user_id_input
                                on:input=move |ev| user_id_input.set(event_target_value(&ev))
                            />
                        </div>
                        <div class="form-control">
                            <label class="label py-1">
                                <span class="label-text">"Resource kind"</span>
                            </label>
                            <input
                                type="text"
                                class="input input-bordered input-sm"
                                placeholder="deployment, user, …"
                                prop:value=resource_kind_input
                                on:input=move |ev| {
                                    resource_kind_input.set(event_target_value(&ev));
                                }
                            />
                        </div>
                        <div class="form-control">
                            <label class="label py-1">
                                <span class="label-text">"Since (UTC)"</span>
                            </label>
                            <input
                                type="datetime-local"
                                class="input input-bordered input-sm"
                                prop:value=since_input
                                on:input=move |ev| since_input.set(event_target_value(&ev))
                            />
                        </div>
                        <div class="form-control">
                            <label class="label py-1">
                                <span class="label-text">"Until (UTC)"</span>
                            </label>
                            <input
                                type="datetime-local"
                                class="input input-bordered input-sm"
                                prop:value=until_input
                                on:input=move |ev| until_input.set(event_target_value(&ev))
                            />
                        </div>
                    </div>
                    <div class="flex justify-end gap-2 mt-2">
                        <button
                            type="button"
                            class="btn btn-ghost btn-sm"
                            on:click=on_clear
                        >
                            "Clear"
                        </button>
                        <button type="submit" class="btn btn-primary btn-sm">
                            "Apply"
                        </button>
                    </div>
                </div>
            </form>

            // Results table.
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
                            audit_resource
                                .get()
                                .map(|res| match res {
                                    Err(e) => {
                                        let msg = format!("Failed to load audit entries: {e}");
                                        // Surface the error in the alert bar too.
                                        set_error.set(Some(msg.clone()));
                                        view! {
                                            <div class="p-4 text-error">{msg}</div>
                                        }
                                            .into_any()
                                    }
                                    Ok(rows) => render_rows_slice(&rows, page.get()).into_any(),
                                })
                        }}
                    </Suspense>
                </div>
            </div>

            // Pagination. Uses the shared `Pagination` component, which
            // expects a concrete `total_pages` — we don't have one, so we
            // lift `has_next` into a reactive "total_pages = current + 1
            // or current + 2" count to drive the Prev/Next enable/disable
            // edges correctly.
            {move || {
                let current = page.get();
                let has_next = audit_resource
                    .get()
                    .and_then(Result::ok)
                    .is_some_and(|rows| page_has_next(&rows, current));
                let total_pages_fake = Signal::derive(move || {
                    if has_next { current + 2 } else { current + 1 }
                });
                let disabled = Signal::derive(move || audit_resource.get().is_none());
                view! {
                    <div class="flex justify-end">
                        <Pagination
                            page=page
                            total_pages=total_pages_fake
                            disabled=disabled
                        />
                    </div>
                }
            }}
        </div>
    }
    .into_any()
}

/// Render the slice of `rows` corresponding to `page`. `rows` is the
/// over-fetch result (up to `(page + 1) * PER_PAGE + 1` rows); we drop
/// the first `page * PER_PAGE` rows and then take at most `PER_PAGE`.
fn render_rows_slice(rows: &[WireAuditEntry], page: usize) -> AnyView {
    let start = page.saturating_mul(PER_PAGE);
    if rows.len() <= start {
        return view! {
            <div class="p-4 opacity-70">"(no entries)"</div>
        }
        .into_any();
    }
    let page_rows: Vec<&WireAuditEntry> = rows.iter().skip(start).take(PER_PAGE).collect();
    if page_rows.is_empty() {
        return view! {
            <div class="p-4 opacity-70">"(no entries)"</div>
        }
        .into_any();
    }

    view! {
        <table class="table table-zebra table-sm">
            <thead>
                <tr>
                    <th>"Time"</th>
                    <th>"User"</th>
                    <th>"Action"</th>
                    <th>"Resource"</th>
                    <th>"IP"</th>
                    <th>"Details"</th>
                </tr>
            </thead>
            <tbody>
                {page_rows.iter().map(|e| render_entry_row(e)).collect_view()}
            </tbody>
        </table>
    }
    .into_any()
}

/// Does the over-fetched `rows` buffer contain at least one row past the
/// end of `page`? Used to drive the Prev/Next enabled state.
fn page_has_next(rows: &[WireAuditEntry], page: usize) -> bool {
    let end = page.saturating_add(1).saturating_mul(PER_PAGE);
    rows.len() > end
}

fn render_entry_row(entry: &WireAuditEntry) -> impl IntoView {
    let time_human = format_timestamp(&entry.created_at);
    let created_title = entry.created_at.clone();
    let user_short = short_id(&entry.user_id);
    let user_full = entry.user_id.clone();
    let action = entry.action.clone();
    let resource = match entry.resource_id.as_deref() {
        Some(id) => format!("{}:{}", entry.resource_kind, id),
        None => entry.resource_kind.clone(),
    };
    let ip = entry.ip.clone().unwrap_or_else(|| "—".to_string());
    let details_view = details_disclosure(entry);

    view! {
        <tr>
            <td class="text-xs whitespace-nowrap" title=created_title>{time_human}</td>
            <td class="font-mono text-xs" title=user_full>{user_short}</td>
            <td>
                <span class="badge badge-outline badge-sm">{action}</span>
            </td>
            <td class="font-mono text-xs">{resource}</td>
            <td class="font-mono text-xs">{ip}</td>
            <td>{details_view}</td>
        </tr>
    }
}

/// Render the details disclosure cell. When `details` is absent, shows a
/// greyed-out em-dash; when present, renders a `<details>/<summary>` pair
/// with the pretty-printed JSON inside.
fn details_disclosure(entry: &WireAuditEntry) -> AnyView {
    let Some(ref details) = entry.details else {
        return view! {
            <span class="opacity-50">"—"</span>
        }
        .into_any();
    };

    let pretty = serde_json::to_string_pretty(details).unwrap_or_else(|_| format!("{details:?}"));
    let ua = entry.user_agent.clone();
    let summary_text = describe_details(details);

    // If we also have a user-agent we stash it in the expanded block
    // since it's conceptually metadata about the actor.
    let ua_view = ua.map(|ua| {
        view! {
            <div class="text-xs opacity-70 mt-1">
                {format!("User-agent: {ua}")}
            </div>
        }
    });

    view! {
        <details class="collapse collapse-arrow">
            <summary class="collapse-title text-xs min-h-0 p-1 cursor-pointer">
                {summary_text}
            </summary>
            <div class="collapse-content p-1">
                <pre class="mockup-code text-xs overflow-x-auto">
                    <code>{pretty}</code>
                </pre>
                {ua_view}
            </div>
        </details>
    }
    .into_any()
}

/// Short, human-ish summary of a details JSON value for use as the
/// disclosure summary. Arrays & objects get a size hint; primitives
/// render inline.
fn describe_details(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => "null".to_string(),
        serde_json::Value::Bool(b) => format!("{b}"),
        serde_json::Value::Number(n) => format!("{n}"),
        serde_json::Value::String(s) => {
            // Keep the string summary readable at a glance.
            if s.len() > 40 {
                format!("\"{}…\"", &s[..40])
            } else {
                format!("\"{s}\"")
            }
        }
        serde_json::Value::Array(arr) => format!("array ({} items)", arr.len()),
        serde_json::Value::Object(obj) => {
            let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
            keys.sort_unstable();
            let preview: String = keys.iter().take(3).copied().collect::<Vec<_>>().join(", ");
            if keys.len() <= 3 {
                format!("{{{preview}}}")
            } else {
                format!("{{{preview}, +{}}}", keys.len() - 3)
            }
        }
    }
}

/// Truncate a UUID-ish id to its first 8 characters; non-UUID ids come
/// through intact when shorter than 8 chars.
fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

/// Convert an HTML `datetime-local` value (`YYYY-MM-DDTHH:MM`) to an
/// RFC-3339 UTC string (`YYYY-MM-DDTHH:MM:00Z`) the daemon will accept.
///
/// An empty input returns an empty string so the caller can treat "no
/// filter" uniformly as "empty `applied_*` signal".
///
/// We deliberately do not include seconds in the input widget (browsers
/// omit them unless `step` is set) so we always append `:00Z`. If the
/// browser DID emit seconds (e.g. `YYYY-MM-DDTHH:MM:SS`) we accept that
/// too and only add the `Z`.
fn datetime_local_to_rfc3339(raw: &str) -> String {
    if raw.is_empty() {
        return String::new();
    }
    // Count colons in the time portion to detect whether seconds are
    // present. The `T` separator is at position 10 for a well-formed
    // date.
    let t_pos = raw.find('T');
    let time_part = t_pos.map_or("", |i| &raw[i + 1..]);
    let colons = time_part.bytes().filter(|b| *b == b':').count();
    if colons >= 2 {
        // Already has seconds; just append Z if missing.
        if raw.ends_with('Z') {
            raw.to_string()
        } else {
            format!("{raw}Z")
        }
    } else {
        format!("{raw}:00Z")
    }
}

/// Human-friendlier rendering of the daemon's RFC-3339 timestamp. Drops
/// the fractional-seconds suffix and the trailing `Z` so the column
/// stays narrow; full timestamp goes in the `title=` tooltip.
fn format_timestamp(rfc3339: &str) -> String {
    // Drop anything after the fractional dot (if any) and the trailing
    // `Z`. Example: "2026-04-16T00:00:00.123Z" → "2026-04-16 00:00:00".
    let mut out = rfc3339.trim_end_matches('Z').to_string();
    if let Some(dot) = out.find('.') {
        out.truncate(dot);
    }
    out = out.replacen('T', " ", 1);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn datetime_local_no_seconds_gets_colon_00_z() {
        assert_eq!(
            datetime_local_to_rfc3339("2026-04-16T12:34"),
            "2026-04-16T12:34:00Z"
        );
    }

    #[test]
    fn datetime_local_with_seconds_just_adds_z() {
        assert_eq!(
            datetime_local_to_rfc3339("2026-04-16T12:34:56"),
            "2026-04-16T12:34:56Z"
        );
    }

    #[test]
    fn datetime_local_already_z_is_passthrough() {
        assert_eq!(
            datetime_local_to_rfc3339("2026-04-16T12:34:56Z"),
            "2026-04-16T12:34:56Z"
        );
    }

    #[test]
    fn datetime_local_empty_stays_empty() {
        assert_eq!(datetime_local_to_rfc3339(""), "");
    }

    #[test]
    fn format_timestamp_drops_fractional_and_z() {
        assert_eq!(
            format_timestamp("2026-04-16T12:00:00.123Z"),
            "2026-04-16 12:00:00"
        );
    }

    #[test]
    fn format_timestamp_no_fractional() {
        assert_eq!(
            format_timestamp("2026-04-16T12:00:00Z"),
            "2026-04-16 12:00:00"
        );
    }

    #[test]
    fn short_id_truncates() {
        assert_eq!(short_id("abcdef0123456789"), "abcdef01");
    }

    #[test]
    fn short_id_keeps_short_ids_intact() {
        assert_eq!(short_id("root"), "root");
    }

    #[test]
    fn page_has_next_detects_probe_row() {
        // PER_PAGE + 1 rows on page 0 means page 1 exists.
        let rows: Vec<WireAuditEntry> = (0..=PER_PAGE)
            .map(|i| WireAuditEntry {
                id: format!("e-{i}"),
                user_id: "u".to_string(),
                action: "a".to_string(),
                resource_kind: "r".to_string(),
                resource_id: None,
                details: None,
                ip: None,
                user_agent: None,
                created_at: "2026-04-16T00:00:00Z".to_string(),
            })
            .collect();
        assert!(page_has_next(&rows, 0));
    }

    #[test]
    fn page_has_next_false_when_exactly_full() {
        let rows: Vec<WireAuditEntry> = (0..PER_PAGE)
            .map(|i| WireAuditEntry {
                id: format!("e-{i}"),
                user_id: "u".to_string(),
                action: "a".to_string(),
                resource_kind: "r".to_string(),
                resource_id: None,
                details: None,
                ip: None,
                user_agent: None,
                created_at: "2026-04-16T00:00:00Z".to_string(),
            })
            .collect();
        assert!(!page_has_next(&rows, 0));
    }

    #[test]
    fn describe_details_object_preview() {
        let v = serde_json::json!({ "zebra": 1, "alpha": 2, "beta": 3 });
        assert_eq!(describe_details(&v), "{alpha, beta, zebra}");
    }

    #[test]
    fn describe_details_object_overflow() {
        let v = serde_json::json!({ "a": 1, "b": 2, "c": 3, "d": 4, "e": 5 });
        assert_eq!(describe_details(&v), "{a, b, c, +2}");
    }

    #[test]
    fn describe_details_array_and_primitives() {
        assert_eq!(
            describe_details(&serde_json::json!([1, 2, 3])),
            "array (3 items)"
        );
        assert_eq!(describe_details(&serde_json::json!(null)), "null");
        assert_eq!(describe_details(&serde_json::json!(42)), "42");
        assert_eq!(describe_details(&serde_json::json!("ok")), "\"ok\"");
    }
}
