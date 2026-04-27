//! OIDC / SSO API DTOs.

use serde::{Deserialize, Serialize};

use crate::api::auth::UserView;

/// Query params on the callback URL — the provider appends `?code=...&state=...`
/// on success, or `?error=...&error_description=...` on user denial.
#[derive(Debug, Deserialize)]
pub struct CallbackParams {
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub error_description: Option<String>,
}

/// Callback response body (returned as JSON unless the operator prefers a
/// redirect; initial integration keeps JSON so the Manager UI can show a
/// welcome screen post-exchange).
#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct OidcCallbackResponse {
    pub user: UserView,
    pub csrf_token: String,
    pub provider: String,
}
