//! Auth guard wrapping protected routes.
//!
//! On mount the guard calls [`manager_me`]. Three outcomes:
//!
//! 1. `user: Some(_)` — the user is signed in; render children and expose
//!    the user via Leptos context so pages + navbar can read it.
//! 2. `user: None, needs_bootstrap: true` — the backend has no users yet;
//!    redirect to `/bootstrap`.
//! 3. `user: None, needs_bootstrap: false` — not signed in; redirect to
//!    `/login`.
//!
//! While the server call is in flight, a lightweight spinner is rendered.
//! Load failures (daemon down, etc.) render an error banner with a retry
//! button rather than silently redirecting — masking a real outage as
//! "please log in" would be misleading.

use leptos::prelude::*;
use leptos_router::hooks::use_navigate;
use leptos_router::NavigateOptions;

use crate::app::server_fns::{manager_me, ManagerUserView};

/// Context carrying the currently signed-in user.
///
/// Pages that want to gate admin UI (e.g. the Users page link in the
/// sidebar) call `expect_context::<CurrentUser>()` to read it.
#[derive(Debug, Clone)]
pub struct CurrentUser(pub ManagerUserView);

impl CurrentUser {
    /// True iff the signed-in user has the "admin" role.
    #[must_use]
    pub fn is_admin(&self) -> bool {
        self.0.role == "admin"
    }
}

/// Wrap protected routes.
///
/// Usage (planned wiring in a later task):
///
/// ```ignore
/// view! {
///     <AuthGuard>
///         <Dashboard />
///     </AuthGuard>
/// }
/// ```
#[component]
pub fn AuthGuard(children: ChildrenFn) -> impl IntoView {
    let me = Resource::new(|| (), |()| async move { manager_me().await });
    let children = StoredValue::new(children);

    view! {
        <Suspense fallback=move || view! { <AuthGuardLoading /> }>
            {move || {
                me.get()
                    .map(|result| match result {
                        Ok(resp) => match resp.user {
                            Some(user) => {
                                provide_context(CurrentUser(user));
                                children.with_value(|c| c()).into_any()
                            }
                            None if resp.needs_bootstrap => {
                                redirect_to("/bootstrap");
                                view! { <AuthGuardLoading /> }.into_any()
                            }
                            None => {
                                redirect_to("/login");
                                view! { <AuthGuardLoading /> }.into_any()
                            }
                        },
                        Err(e) => view! {
                            <AuthGuardError error=e.to_string() resource=me />
                        }
                        .into_any(),
                    })
            }}
        </Suspense>
    }
}

/// Loading state while `manager_me` is pending.
#[component]
fn AuthGuardLoading() -> impl IntoView {
    view! {
        <div class="flex items-center justify-center min-h-[60vh]">
            <span class="loading loading-spinner loading-lg text-primary"></span>
        </div>
    }
}

/// Error state shown when the auth probe itself fails (daemon unreachable,
/// network hiccup). Gives the user an explicit retry rather than silently
/// redirecting, which would mask real outages.
#[component]
fn AuthGuardError(
    error: String,
    resource: Resource<Result<crate::app::server_fns::ManagerMeResponse, ServerFnError>>,
) -> impl IntoView {
    view! {
        <div class="flex items-center justify-center min-h-[60vh]">
            <div class="alert alert-error max-w-md">
                <div>
                    <h3 class="font-bold">"Authentication check failed"</h3>
                    <p class="text-sm opacity-80">{error}</p>
                    <button
                        class="btn btn-sm btn-outline mt-3"
                        on:click=move |_| resource.refetch()
                    >
                        "Retry"
                    </button>
                </div>
            </div>
        </div>
    }
}

/// Navigate to `path` via the Leptos router. On SSR this is a no-op (the
/// server-rendered HTML for the "not authenticated" branch will be replaced
/// by the client-side navigate on hydration).
fn redirect_to(path: &'static str) {
    let navigate = use_navigate();
    navigate(path, NavigateOptions::default());
}
