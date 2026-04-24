//! Inbound firewall-rule management for the overlay + API + Raft ports.
//!
//! On Windows this module installs three inbound-allow rules in Windows
//! Defender Firewall via the `INetFwPolicy2` COM API:
//!
//! - `ZLayer Overlay (UDP)` — the Wintun/boringtun listen port
//! - `ZLayer API (TCP)`     — the daemon HTTP/gRPC port
//! - `ZLayer Raft (TCP)`    — the scheduler Raft port
//!
//! Rules are scoped to the **Private + Domain** profiles only. Public profile
//! is intentionally excluded so laptops on untrusted networks (coffee-shop
//! Wi-Fi, airport, etc.) do not start accepting inbound cluster traffic.
//!
//! [`ensure_overlay_rules`] is idempotent: if a rule with the same name
//! already exists it is left in place rather than duplicated.
//!
//! On non-Windows targets both functions are no-ops that return `Ok(())`.
//! Linux nodes are expected to manage their own `iptables`/`nftables` or
//! `firewalld` state out-of-band, and macOS has its own model (`pfctl` /
//! Application Firewall) that isn't in scope for this phase.

use thiserror::Error;

/// Errors produced while installing or removing Windows firewall rules.
#[derive(Error, Debug)]
pub enum FirewallError {
    /// A COM call failed. Includes the underlying `HRESULT` message.
    #[error("Windows COM call failed: {0}")]
    Com(String),

    /// `CoInitializeEx` returned a failure status.
    #[error("CoInitializeEx failed: {0}")]
    ComInit(String),

    /// The `INetFwPolicy2` interface could not be instantiated.
    #[error("INetFwPolicy2 not available: {0}")]
    PolicyUnavailable(String),

    /// Adding a firewall rule failed. Includes the rule name.
    #[error("Failed to add firewall rule '{name}': {reason}")]
    AddRule {
        /// Display name of the rule that could not be created.
        name: String,
        /// Underlying error message from the Windows API.
        reason: String,
    },

    /// Removing a firewall rule failed. Includes the rule name.
    #[error("Failed to remove firewall rule '{name}': {reason}")]
    RemoveRule {
        /// Display name of the rule that could not be removed.
        name: String,
        /// Underlying error message from the Windows API.
        reason: String,
    },

    /// A string could not be converted to the `BSTR` / wide-string form
    /// required by the Windows COM API.
    #[error("String conversion failed: {0}")]
    StringConversion(String),
}

#[cfg(windows)]
mod windows;

/// Display name of the inbound overlay (`WireGuard` UDP) firewall rule.
pub const OVERLAY_RULE_NAME: &str = "ZLayer Overlay (UDP)";

/// Display name of the inbound API (HTTP/gRPC TCP) firewall rule.
pub const API_RULE_NAME: &str = "ZLayer API (TCP)";

/// Display name of the inbound Raft (TCP) firewall rule.
pub const RAFT_RULE_NAME: &str = "ZLayer Raft (TCP)";

/// All three rule names that this module manages, in the order they are
/// installed / removed.
pub const MANAGED_RULE_NAMES: &[&str] = &[OVERLAY_RULE_NAME, API_RULE_NAME, RAFT_RULE_NAME];

/// Ensure the three inbound allow-rules exist in Windows Defender Firewall
/// for the overlay UDP, API TCP, and Raft TCP ports.
///
/// Idempotent: if a rule with the expected name already exists it is left
/// untouched. Rules are scoped to the Private + Domain profiles only.
///
/// On non-Windows targets this is a no-op that returns `Ok(())`.
///
/// # Arguments
///
/// * `wg_port`   — UDP inbound port for the overlay (boringtun)
/// * `api_port`  — TCP inbound port for the daemon API
/// * `raft_port` — TCP inbound port for the Raft scheduler
///
/// # Errors
///
/// Returns a [`FirewallError`] if COM initialization fails, the
/// `INetFwPolicy2` service is unavailable, or the Windows Firewall API
/// rejects a rule creation (typically because the process lacks
/// administrator privileges). On non-Windows targets this cannot fail.
pub fn ensure_overlay_rules(
    wg_port: u16,
    api_port: u16,
    raft_port: u16,
) -> Result<(), FirewallError> {
    #[cfg(windows)]
    {
        self::windows::ensure_overlay_rules(wg_port, api_port, raft_port)
    }
    #[cfg(not(windows))]
    {
        let _ = (wg_port, api_port, raft_port);
        Ok(())
    }
}

/// Remove any ZLayer-managed inbound firewall rules that this module would
/// otherwise install.
///
/// Safe to call when the rules do not exist — missing rules are treated as
/// a successful no-op. On non-Windows targets this is a no-op that returns
/// `Ok(())`.
///
/// # Errors
///
/// Returns a [`FirewallError`] if COM initialization fails, the
/// `INetFwPolicy2` service is unavailable, or the Windows Firewall API
/// rejects the remove call. "Rule not found" is not treated as an error.
/// On non-Windows targets this cannot fail.
pub fn remove_overlay_rules() -> Result<(), FirewallError> {
    #[cfg(windows)]
    {
        self::windows::remove_overlay_rules()
    }
    #[cfg(not(windows))]
    {
        Ok(())
    }
}
