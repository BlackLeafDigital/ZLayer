//! Container events API DTOs.

use serde::Deserialize;
#[cfg(feature = "utoipa")]
use utoipa::IntoParams;

/// Query parameters for `GET /api/v1/events`.
#[derive(Debug, Clone, Default, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(IntoParams))]
#[cfg_attr(feature = "utoipa", into_params(parameter_in = Query))]
pub struct EventsQuery {
    /// Follow the event stream. Default: `true`. Reserved for parity with
    /// Docker-compat tooling; this endpoint is always streaming.
    #[serde(default)]
    pub follow: Option<bool>,
    /// Label filter in `k=v` form. Repeatable. An event passes only if all
    /// filters match (AND semantics).
    #[serde(default)]
    pub label: Vec<String>,
}
