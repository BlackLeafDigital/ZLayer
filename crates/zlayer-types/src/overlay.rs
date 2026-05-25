//! Overlay-network configuration types shared across `ZLayer` crates.
//!
//! Status as of v0.51:
//! - [`OverlayMode::Shared`] is fully implemented (single cluster `WireGuard`
//!   interface carrying multiple service subnets via multi-CIDR `AllowedIPs`,
//!   per-node Linux bridges per service).
//! - [`OverlayMode::Auto`] is the default. In v0.51 it always resolves to
//!   `Shared` because we have no telemetry inputs (bandwidth budgets, NIC
//!   capabilities, per-node interface caps) to choose otherwise. Future
//!   rounds will wire a real heuristic.
//! - [`OverlayMode::Dedicated`] is reserved for a future round that restores
//!   per-service `WireGuard` TUNs as a bandwidth opt-out. Currently warns and
//!   falls back to `Shared`.
//!
//! Every consumer of `OverlayMode` MUST go through [`OverlayMode::resolve_v0_51`]
//! before acting on the value, so the warn-and-fallback surface is uniform.

use serde::{Deserialize, Serialize};

/// How the daemon places a service's overlay attachment.
///
/// See module docs for the v0.51 implementation status of each variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum OverlayMode {
    /// Daemon picks. In v0.51 always resolves to [`OverlayMode::Shared`]
    /// (no telemetry inputs yet to choose otherwise).
    #[default]
    Auto,
    /// Single cluster `WireGuard` interface carries every service subnet via
    /// multi-CIDR `AllowedIPs`; each service has a per-node Linux bridge
    /// for container attachment. Lowest interface count; one shared crypto
    /// context (bandwidth ceiling shared across all service traffic).
    Shared,
    /// Per-service `WireGuard` TUN with its own crypto context. Reserved for
    /// a future bandwidth opt-out — **NOT implemented in v0.51**, currently
    /// warns and falls back to `Shared`.
    Dedicated,
}

impl OverlayMode {
    /// Resolve to the actually-implemented mode for v0.51, logging a warning
    /// if the requested mode is not yet wired up. Every code path that
    /// consumes an `OverlayMode` value MUST go through this funnel so we
    /// get a uniform warning surface for future-reserved variants.
    #[must_use]
    pub fn resolve_v0_51(self) -> OverlayMode {
        match self {
            OverlayMode::Auto | OverlayMode::Shared => OverlayMode::Shared,
            OverlayMode::Dedicated => {
                tracing::warn!(
                    "OverlayMode::Dedicated is reserved for a future round; \
                     falling back to OverlayMode::Shared. Per-service `WireGuard` \
                     TUNs will be restored when bandwidth telemetry lands.",
                );
                OverlayMode::Shared
            }
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
    fn overlay_mode_resolve_v0_51_collapses_to_shared() {
        assert_eq!(OverlayMode::Auto.resolve_v0_51(), OverlayMode::Shared);
        assert_eq!(OverlayMode::Shared.resolve_v0_51(), OverlayMode::Shared);
        assert_eq!(OverlayMode::Dedicated.resolve_v0_51(), OverlayMode::Shared);
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
