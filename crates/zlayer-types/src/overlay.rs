//! Overlay-network configuration types shared across `ZLayer` crates.
//!
//! Mode status:
//! - [`OverlayMode::Shared`] is fully implemented (single cluster `WireGuard`
//!   interface carrying multiple service subnets via multi-CIDR `AllowedIPs`,
//!   per-node Linux bridges per service).
//! - [`OverlayMode::Dedicated`] is implemented: each service gets its own
//!   per-service `WireGuard` transport with an isolated crypto context.
//! - [`OverlayMode::Auto`] is the default. It resolves to `Shared` — there is
//!   no telemetry heuristic yet (bandwidth budgets, NIC capabilities, per-node
//!   interface caps) to pick `Dedicated` automatically. A future round will
//!   wire a real heuristic.
//!
//! Every consumer of `OverlayMode` MUST go through [`OverlayMode::resolve`]
//! before acting on the value, so the resolution surface is uniform.

use serde::{Deserialize, Serialize};

/// How the daemon places a service's overlay attachment.
///
/// See module docs for the implementation status of each variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum OverlayMode {
    /// Daemon picks. Resolves to [`OverlayMode::Shared`] — there is no
    /// telemetry heuristic yet to pick [`OverlayMode::Dedicated`] on its own.
    #[default]
    Auto,
    /// Single cluster `WireGuard` interface carries every service subnet via
    /// multi-CIDR `AllowedIPs`; each service has a per-node Linux bridge
    /// for container attachment. Lowest interface count; one shared crypto
    /// context (bandwidth ceiling shared across all service traffic).
    Shared,
    /// Per-service `WireGuard` transport with its own isolated crypto context,
    /// for services that need their own bandwidth ceiling. Implemented.
    Dedicated,
}

impl OverlayMode {
    /// Resolve to the actually-implemented mode. `Auto` resolves to `Shared`
    /// (no telemetry heuristic yet to choose `Dedicated` automatically);
    /// `Shared` and `Dedicated` resolve to themselves. Every code path that
    /// consumes an `OverlayMode` value MUST go through this funnel so the
    /// resolution surface is uniform.
    #[must_use]
    pub fn resolve(self) -> OverlayMode {
        match self {
            OverlayMode::Auto | OverlayMode::Shared => OverlayMode::Shared,
            OverlayMode::Dedicated => OverlayMode::Dedicated,
        }
    }
}

/// Per-service overlay configuration, populated from the service spec.
///
/// `parent` names another overlay this one should nest under. In v0.51 only
/// `None` / `Some("cluster")` is honored; any other value triggers a warn-
/// and-fallback (treated as `None`). Future rounds may allow service-of-
/// service nesting.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OverlayConfig {
    #[serde(default)]
    pub mode: OverlayMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
}

impl OverlayConfig {
    /// Returns the resolved parent name for v0.51: `None` if the parent is
    /// `None` or `Some("cluster")`; logs a warning and falls back to `None`
    /// for any other value.
    #[must_use]
    pub fn resolved_parent_v0_51(&self) -> Option<&str> {
        match self.parent.as_deref() {
            None | Some("cluster") => None,
            Some(other) => {
                tracing::warn!(
                    parent = other,
                    "OverlayConfig.parent only supports `cluster` (or unset) in v0.51; \
                     service-of-service nesting is reserved for a future round. \
                     Falling back to cluster parent.",
                );
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_mode_default_is_auto() {
        assert_eq!(OverlayMode::default(), OverlayMode::Auto);
    }

    #[test]
    fn overlay_mode_resolve() {
        assert_eq!(OverlayMode::Auto.resolve(), OverlayMode::Shared);
        assert_eq!(OverlayMode::Shared.resolve(), OverlayMode::Shared);
        assert_eq!(OverlayMode::Dedicated.resolve(), OverlayMode::Dedicated);
    }

    #[test]
    fn overlay_mode_serde_lowercase() {
        assert_eq!(
            serde_json::to_string(&OverlayMode::Auto).unwrap(),
            "\"auto\""
        );
        assert_eq!(
            serde_json::to_string(&OverlayMode::Shared).unwrap(),
            "\"shared\""
        );
        assert_eq!(
            serde_json::to_string(&OverlayMode::Dedicated).unwrap(),
            "\"dedicated\""
        );
        assert_eq!(
            serde_json::from_str::<OverlayMode>("\"shared\"").unwrap(),
            OverlayMode::Shared,
        );
    }

    #[test]
    fn overlay_config_default_is_auto_no_parent() {
        let cfg = OverlayConfig::default();
        assert_eq!(cfg.mode, OverlayMode::Auto);
        assert_eq!(cfg.parent, None);
        assert_eq!(cfg.resolved_parent_v0_51(), None);
    }

    #[test]
    fn overlay_config_cluster_parent_is_none() {
        let cfg = OverlayConfig {
            mode: OverlayMode::Shared,
            parent: Some("cluster".to_string()),
        };
        assert_eq!(cfg.resolved_parent_v0_51(), None);
    }

    #[test]
    fn overlay_config_other_parent_warns_and_returns_none() {
        let cfg = OverlayConfig {
            mode: OverlayMode::Shared,
            parent: Some("svc-other".to_string()),
        };
        assert_eq!(cfg.resolved_parent_v0_51(), None);
    }
}
