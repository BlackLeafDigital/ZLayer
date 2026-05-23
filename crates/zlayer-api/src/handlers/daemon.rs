//! Daemon-level introspection endpoints.
//!
//! Exposes the process-wide capability survey computed by
//! [`zlayer_agent::capability::DaemonCapabilities`]. Useful for UIs and
//! operators that want to know whether the running daemon is root, nested
//! in a container, has `CAP_NET_ADMIN`, etc.

use axum::Json;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use zlayer_agent::capability::{DaemonCapabilities, DaemonMode};

/// Coarse classification of the daemon's effective execution environment.
///
/// Mirror of [`zlayer_agent::capability::DaemonMode`]. The agent's enum lives
/// in a crate that doesn't depend on `utoipa`, so we duplicate the variants
/// here (kept in sync via the [`From`] impl below) and tag this one with
/// [`ToSchema`] for `OpenAPI` generation.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DaemonModeDto {
    /// Host-level execution: all caps, can write cgroup root, can create overlay.
    Full,
    /// Inside a container: scoped to a sub-cgroup; some caps may be present.
    NestedAdaptive,
    /// Missing privileges required for any meaningful container creation.
    Degraded,
}

impl From<DaemonMode> for DaemonModeDto {
    fn from(m: DaemonMode) -> Self {
        match m {
            DaemonMode::Full => Self::Full,
            DaemonMode::NestedAdaptive => Self::NestedAdaptive,
            DaemonMode::Degraded => Self::Degraded,
        }
    }
}

/// Response body for `GET /api/v1/daemon/capabilities`.
///
/// Snapshot of the daemon's runtime environment: privilege level, cgroup
/// scoping, network-related kernel capabilities. The fields mirror
/// [`DaemonCapabilities`] field-for-field; values are memoised at process
/// start and do not change at runtime.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct DaemonCapabilitiesResponse {
    /// Coarse classification derived from the boolean fields below.
    pub mode: DaemonModeDto,
    /// `true` if the process is running as uid 0.
    pub is_root: bool,
    /// `true` if the process appears to be inside a container (non-root
    /// cgroup v2 path).
    pub is_nested: bool,
    /// The cgroup v2 path of the current process, if any.
    pub cgroup_parent: Option<String>,
    /// `true` if the cgroup root's `cgroup.subtree_control` has the owner-
    /// write bit set.
    pub can_write_cgroup_root: bool,
    /// `true` if `CAP_NET_ADMIN` is present in the process's bounding set.
    pub has_cap_net_admin: bool,
    /// `true` if `/dev/net/tun` can be opened r/w without
    /// EACCES/EPERM/ENOENT/ENXIO.
    pub tun_device_available: bool,
}

impl From<&DaemonCapabilities> for DaemonCapabilitiesResponse {
    fn from(c: &DaemonCapabilities) -> Self {
        Self {
            mode: c.effective_mode.into(),
            is_root: c.is_root,
            is_nested: c.is_nested,
            cgroup_parent: c.cgroup_parent.clone(),
            can_write_cgroup_root: c.can_write_cgroup_root,
            has_cap_net_admin: c.has_cap_net_admin,
            tun_device_available: c.tun_device_available,
        }
    }
}

/// Get the daemon's runtime capability survey.
///
/// Returns the process-wide memoised [`DaemonCapabilities`] snapshot. The
/// handler is intentionally state-free — `DaemonCapabilities::get()` is a
/// process-wide accessor backed by a `OnceLock`, so it does not need to
/// pull from `AppState` and tests don't have to construct one to exercise
/// the endpoint.
///
/// If the survey has not been seeded yet (i.e. the daemon's startup task
/// hasn't run), the first call here triggers the probe lazily.
#[utoipa::path(
    get,
    path = "/api/v1/daemon/capabilities",
    responses(
        (status = 200, description = "Daemon capability survey", body = DaemonCapabilitiesResponse),
    ),
    tag = "Daemon"
)]
pub async fn get_daemon_capabilities() -> Json<DaemonCapabilitiesResponse> {
    Json(DaemonCapabilitiesResponse::from(DaemonCapabilities::get()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The handler returns the memoised survey and the JSON body has the
    /// expected field names. We do not assert specific values because they
    /// depend on the test process's environment (root-ness, cgroup layout,
    /// `/dev/net/tun` availability, etc.).
    #[tokio::test]
    async fn test_get_daemon_capabilities_returns_expected_fields() {
        let Json(response) = get_daemon_capabilities().await;
        let value = serde_json::to_value(&response).expect("response serialises");
        let obj = value.as_object().expect("response is a JSON object");

        for field in [
            "mode",
            "is_root",
            "is_nested",
            "cgroup_parent",
            "can_write_cgroup_root",
            "has_cap_net_admin",
            "tun_device_available",
        ] {
            assert!(
                obj.contains_key(field),
                "missing field `{field}` in response"
            );
        }

        // `mode` must serialise as one of the three snake_case variants.
        let mode = obj
            .get("mode")
            .and_then(|m| m.as_str())
            .expect("mode is a string");
        assert!(
            matches!(mode, "full" | "nested_adaptive" | "degraded"),
            "unexpected mode variant: {mode}"
        );
    }

    /// The `DaemonModeDto` <-> `DaemonMode` mirror must serialise identically
    /// so external consumers can't tell the two enums apart on the wire.
    #[test]
    fn test_daemon_mode_dto_serialises_like_agent_enum() {
        for (agent, dto) in [
            (DaemonMode::Full, DaemonModeDto::Full),
            (DaemonMode::NestedAdaptive, DaemonModeDto::NestedAdaptive),
            (DaemonMode::Degraded, DaemonModeDto::Degraded),
        ] {
            let agent_json = serde_json::to_string(&agent).unwrap();
            let dto_json = serde_json::to_string(&dto).unwrap();
            assert_eq!(
                agent_json, dto_json,
                "DaemonModeDto::{dto:?} must serialise identically to DaemonMode::{agent:?}"
            );
            // And the `From` impl preserves the variant.
            let converted: DaemonModeDto = agent.into();
            assert_eq!(converted, dto);
        }
    }
}
