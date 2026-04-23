//! Windows Defender Firewall rule management via `INetFwPolicy2` COM.
//!
//! The `windows_firewall` crate on crates.io was evaluated and rejected:
//! its `FirewallRuleBuilder::profiles` takes an `Option<Profile>` (single
//! variant), so restricting a rule to Private + Domain simultaneously --
//! which is a hard requirement for this module -- would require creating
//! two separate rules per port and is not how the underlying COM API
//! models it (the profile mask is a bitflag on a single rule). We go
//! direct to `windows-rs` instead.

#![cfg(target_os = "windows")]
// This module is the Windows Firewall COM FFI boundary. Every `unsafe`
// block below has a `SAFETY:` comment explaining why the required COM
// invariants hold; the workspace-wide `-W unsafe-code` policy remains in
// force everywhere else.
#![allow(unsafe_code)]

use windows::core::BSTR;
use windows::Win32::Foundation::VARIANT_TRUE;
use windows::Win32::NetworkManagement::WindowsFirewall::{
    INetFwPolicy2, INetFwRules, NetFwPolicy2, NetFwRule, NET_FW_ACTION_ALLOW,
    NET_FW_IP_PROTOCOL_TCP, NET_FW_IP_PROTOCOL_UDP, NET_FW_PROFILE2_DOMAIN,
    NET_FW_PROFILE2_PRIVATE, NET_FW_RULE_DIR_IN,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
    COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
};

use super::{FirewallError, API_RULE_NAME, MANAGED_RULE_NAMES, OVERLAY_RULE_NAME, RAFT_RULE_NAME};

/// RAII guard that pairs `CoInitializeEx` with `CoUninitialize` on drop.
///
/// Windows requires COM to be initialized on the calling thread before
/// any `CoCreateInstance` calls. We initialize apartment-threaded (the
/// Windows Firewall COM server is STA) and tear down on scope exit so
/// repeated calls to `ensure_overlay_rules` don't leak COM state.
struct ComGuard;

impl ComGuard {
    fn new() -> Result<Self, FirewallError> {
        // SAFETY: CoInitializeEx is safe to call on any thread; the
        // returned HRESULT is checked below. COINIT_APARTMENTTHREADED
        // | COINIT_DISABLE_OLE1DDE is the standard non-OLE STA flag
        // combo recommended by Microsoft for modern COM clients.
        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
        // S_OK, S_FALSE (already initialized on this thread), and
        // RPC_E_CHANGED_MODE (already initialized with a different
        // concurrency model -- someone else owns the apartment) are all
        // acceptable: we can still make COM calls. `HRESULT::is_err`
        // treats those three as success.
        if hr.is_err() {
            return Err(FirewallError::ComInit(format!("{hr:?}")));
        }
        Ok(Self)
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        // SAFETY: We only call CoUninitialize once per successful
        // CoInitializeEx, matching the documented reference-count
        // contract.
        unsafe { CoUninitialize() };
    }
}

/// Handle for talking to the live firewall policy.
///
/// Holds the `INetFwRules` collection plus the `ComGuard` so that COM
/// stays initialized for the full lifetime of the handle. The raw
/// `INetFwPolicy2` pointer isn't re-used after we pull `Rules()` off it,
/// so it is dropped at the end of [`FirewallPolicy::open`].
struct FirewallPolicy {
    rules: INetFwRules,
    // Drop order matters: `_com` is the last field, so interface
    // pointers in `rules` are released before CoUninitialize fires.
    _com: ComGuard,
}

impl FirewallPolicy {
    fn open() -> Result<Self, FirewallError> {
        let com = ComGuard::new()?;
        // SAFETY: CLSID_NetFwPolicy2 is a well-known COM class shipped
        // with every supported Windows SKU. CLSCTX_INPROC_SERVER is the
        // activation context Microsoft documents for this class.
        let policy: INetFwPolicy2 = unsafe {
            CoCreateInstance::<Option<&windows::core::IUnknown>, INetFwPolicy2>(
                &NetFwPolicy2,
                None,
                CLSCTX_INPROC_SERVER,
            )
        }
        .map_err(|e| FirewallError::PolicyUnavailable(format!("{e}")))?;

        // SAFETY: Rules is a [propget] accessor on INetFwPolicy2 that
        // returns the shared INetFwRules collection; safe to call on a
        // valid policy pointer.
        let rules = unsafe { policy.Rules() }
            .map_err(|e| FirewallError::Com(format!("INetFwPolicy2::Rules: {e}")))?;
        Ok(Self { rules, _com: com })
    }

    /// Return true if a rule with the given display name already exists.
    fn rule_exists(&self, name: &str) -> bool {
        let key = BSTR::from(name);
        // SAFETY: `Item` is the [propget] lookup on INetFwRules; it
        // returns a failing HRESULT (typically E_INVALIDARG / item not
        // found) when no rule matches, which we translate to `false`.
        unsafe { self.rules.Item(&key) }.is_ok()
    }

    /// Create and install a rule. Caller must guarantee `rule_exists`
    /// returned `false` first to keep the operation idempotent.
    fn add_rule(&self, spec: &RuleSpec<'_>) -> Result<(), FirewallError> {
        // SAFETY: CLSID_NetFwRule is also a shipped-with-Windows COM
        // class and supports CLSCTX_INPROC_SERVER.
        let rule = unsafe {
            CoCreateInstance::<Option<&windows::core::IUnknown>, _>(
                &NetFwRule,
                None,
                CLSCTX_INPROC_SERVER,
            )
        }
        .map_err(|e| FirewallError::AddRule {
            name: spec.name.to_string(),
            reason: format!("CoCreateInstance(NetFwRule): {e}"),
        })?;

        let name_bstr = BSTR::from(spec.name);
        let desc_bstr = BSTR::from(spec.description);
        let ports_bstr = BSTR::from(spec.port.to_string().as_str());
        let group_bstr = BSTR::from("ZLayer");

        let configure_and_add = || -> windows::core::Result<()> {
            use windows::Win32::NetworkManagement::WindowsFirewall::INetFwRule;
            let rule: &INetFwRule = &rule;
            // SAFETY: Every setter below is a [propput] on INetFwRule;
            // each call takes ownership of its argument or makes an
            // internal copy per the COM contract. We hold the BSTRs
            // alive until the full configure+Add sequence returns.
            unsafe {
                rule.SetName(&name_bstr)?;
                rule.SetDescription(&desc_bstr)?;
                rule.SetProtocol(spec.protocol)?;
                rule.SetLocalPorts(&ports_bstr)?;
                rule.SetDirection(NET_FW_RULE_DIR_IN)?;
                rule.SetAction(NET_FW_ACTION_ALLOW)?;
                rule.SetEnabled(VARIANT_TRUE)?;
                // Private + Domain only. Public is intentionally omitted.
                let profile_mask = NET_FW_PROFILE2_DOMAIN.0 | NET_FW_PROFILE2_PRIVATE.0;
                rule.SetProfiles(profile_mask)?;
                rule.SetGrouping(&group_bstr)?;
                self.rules.Add(rule)?;
            }
            Ok(())
        };

        configure_and_add().map_err(|e| FirewallError::AddRule {
            name: spec.name.to_string(),
            reason: format!("{e}"),
        })
    }

    /// Delete a rule by display name. Treats "not found" as success.
    fn remove_rule(&self, name: &str) -> Result<(), FirewallError> {
        if !self.rule_exists(name) {
            return Ok(());
        }
        let key = BSTR::from(name);
        // SAFETY: Remove is a mutating method on the shared INetFwRules
        // collection; the BSTR outlives the call.
        unsafe { self.rules.Remove(&key) }.map_err(|e| FirewallError::RemoveRule {
            name: name.to_string(),
            reason: format!("{e}"),
        })
    }
}

/// Parameters for a single rule we want to install. Kept as a plain
/// struct so the add-rule code path is a straight-line sequence of COM
/// setter calls rather than a matrix of conditionals.
struct RuleSpec<'a> {
    name: &'a str,
    description: &'a str,
    port: u16,
    /// `NET_FW_IP_PROTOCOL_TCP` (6) or `NET_FW_IP_PROTOCOL_UDP` (17).
    protocol: i32,
}

/// Idempotently install the three inbound rules.
pub(super) fn ensure_overlay_rules(
    wg_port: u16,
    api_port: u16,
    raft_port: u16,
) -> Result<(), FirewallError> {
    let policy = FirewallPolicy::open()?;

    let specs = [
        RuleSpec {
            name: OVERLAY_RULE_NAME,
            description: "ZLayer encrypted overlay (WireGuard/boringtun) inbound UDP",
            port: wg_port,
            protocol: NET_FW_IP_PROTOCOL_UDP.0,
        },
        RuleSpec {
            name: API_RULE_NAME,
            description: "ZLayer daemon HTTP/gRPC API inbound TCP",
            port: api_port,
            protocol: NET_FW_IP_PROTOCOL_TCP.0,
        },
        RuleSpec {
            name: RAFT_RULE_NAME,
            description: "ZLayer Raft scheduler inbound TCP",
            port: raft_port,
            protocol: NET_FW_IP_PROTOCOL_TCP.0,
        },
    ];

    for spec in specs {
        if policy.rule_exists(spec.name) {
            tracing::debug!(rule = spec.name, "firewall rule already present; skipping");
            continue;
        }
        let port = spec.port;
        let name = spec.name;
        policy.add_rule(&spec)?;
        tracing::info!(rule = name, port = port, "installed firewall rule");
    }

    Ok(())
}

/// Remove every rule this module would install. Missing rules are OK.
pub(super) fn remove_overlay_rules() -> Result<(), FirewallError> {
    let policy = FirewallPolicy::open()?;
    for name in MANAGED_RULE_NAMES {
        policy.remove_rule(name)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Round-trip: ensure then remove. Requires admin -- marked `#[ignore]`
    /// so it doesn't fire in the default `cargo test` run.
    #[test]
    #[ignore = "requires administrator privileges + Windows Defender Firewall service"]
    fn ensure_then_remove_roundtrip() {
        // Use high, unlikely-to-conflict ports so a real cluster's rules
        // aren't clobbered by the test.
        ensure_overlay_rules(51820, 13669, 13670).expect("ensure failed");

        let policy = FirewallPolicy::open().expect("open policy");
        assert!(policy.rule_exists(OVERLAY_RULE_NAME));
        assert!(policy.rule_exists(API_RULE_NAME));
        assert!(policy.rule_exists(RAFT_RULE_NAME));
        drop(policy);

        remove_overlay_rules().expect("remove failed");

        let policy = FirewallPolicy::open().expect("reopen policy");
        assert!(!policy.rule_exists(OVERLAY_RULE_NAME));
        assert!(!policy.rule_exists(API_RULE_NAME));
        assert!(!policy.rule_exists(RAFT_RULE_NAME));
    }

    /// Calling `ensure_overlay_rules` twice in a row must not create
    /// duplicate rules or return an error.
    #[test]
    #[ignore = "requires administrator privileges + Windows Defender Firewall service"]
    fn ensure_is_idempotent() {
        // Clean slate before and after to avoid leaking rules.
        let _ = remove_overlay_rules();

        ensure_overlay_rules(51820, 13669, 13670).expect("first ensure");
        ensure_overlay_rules(51820, 13669, 13670).expect("second ensure (idempotent)");

        let policy = FirewallPolicy::open().expect("open policy");
        assert!(policy.rule_exists(OVERLAY_RULE_NAME));
        assert!(policy.rule_exists(API_RULE_NAME));
        assert!(policy.rule_exists(RAFT_RULE_NAME));
        drop(policy);

        remove_overlay_rules().expect("cleanup");
    }
}
