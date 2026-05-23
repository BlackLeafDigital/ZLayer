//! Daemon capability survey.
//!
//! Probes the runtime environment of the zlayer daemon (root vs. non-root,
//! host vs. nested in a container, cgroup v2 path, `CAP_NET_ADMIN`, presence
//! of `/dev/net/tun`, and writability of the cgroup root) and derives a coarse
//! [`DaemonMode`] from those signals.
//!
//! All probes are intentionally cheap and non-destructive — a handful of
//! syscalls, no allocations of kernel resources (no TUN interfaces, no cgroup
//! writes). The struct is safe to construct multiple times.
//!
//! Non-Linux targets report a fixed degraded survey since the kernel features
//! these probes target are Linux-only.

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

/// Coarse classification of the daemon's effective execution environment.
///
/// Derived from the boolean fields on [`DaemonCapabilities`].
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DaemonMode {
    /// Host-level execution: all caps, can write cgroup root, can create overlay.
    Full,
    /// Inside a container: scoped to a sub-cgroup; some caps may be present.
    NestedAdaptive,
    /// Missing privileges required for any meaningful container creation.
    Degraded,
}

/// Snapshot of the daemon's effective capabilities and execution environment.
///
/// Construct via [`DaemonCapabilities::probe`]. Cheap to call repeatedly.
///
/// The struct intentionally exposes independent capability bits as separate
/// booleans rather than collapsing them into an enum — each bit corresponds to
/// an orthogonal kernel feature (cgroup write, `CAP_NET_ADMIN`, TUN access,
/// root-ness) and downstream code wants to inspect them independently when
/// deciding what to gate.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonCapabilities {
    /// `true` if the process is running as uid 0.
    pub is_root: bool,
    /// `true` if the process appears to be inside a container (non-root cgroup
    /// v2 path).
    pub is_nested: bool,
    /// The cgroup v2 path of the current process, if any (e.g.
    /// `/system.slice/zlayer.service`). `None` on the cgroup root, on
    /// cgroup-v1-only hosts, on non-Linux, or on read errors.
    pub cgroup_parent: Option<String>,
    /// `true` if the cgroup root's `cgroup.subtree_control` has the
    /// owner-write bit set. Coarse, non-destructive signal — does not
    /// guarantee an actual write will succeed.
    pub can_write_cgroup_root: bool,
    /// `true` if `CAP_NET_ADMIN` is present in the process's bounding set
    /// (Linux only).
    pub has_cap_net_admin: bool,
    /// `true` if `/dev/net/tun` can be opened r/w in non-blocking mode without
    /// EACCES/EPERM/ENOENT/ENXIO. The fd is dropped immediately.
    pub tun_device_available: bool,
    /// Coarse classification derived from the above fields.
    pub effective_mode: DaemonMode,
}

/// Process-wide memoised capability survey. Seeded by the first call to
/// [`DaemonCapabilities::get`] or [`DaemonCapabilities::seed`].
static CAPS: OnceLock<DaemonCapabilities> = OnceLock::new();

impl DaemonCapabilities {
    /// Returns the process-wide capability snapshot, probing on first call.
    ///
    /// Subsequent calls return the same memoised instance — capabilities of a
    /// running daemon do not change at runtime, so re-probing would be wasted
    /// syscalls and could create the illusion that the daemon's behaviour can
    /// shift mid-flight.
    pub fn get() -> &'static Self {
        CAPS.get_or_init(Self::probe)
    }

    /// Eagerly seed the memoised survey with an explicit probe result.
    ///
    /// Useful at daemon startup to force the probe to happen at a known point
    /// (so the banner log appears in the expected place). Returns the stored
    /// instance — if the cache was already seeded, the existing value wins
    /// and the passed-in `caps` is dropped (probe is pure, so this is fine).
    ///
    /// # Panics
    ///
    /// In practice this never panics — `OnceLock::set` either stores the
    /// value or rejects it because the cell is already filled, and in both
    /// cases the subsequent `get()` returns `Some`. The `expect` exists only
    /// to satisfy the type system.
    pub fn seed(caps: Self) -> &'static Self {
        let _ = CAPS.set(caps);
        CAPS.get()
            .expect("CAPS is filled after set or was already filled")
    }

    /// Probe the running daemon's effective capabilities.
    ///
    /// Cheap — a handful of syscalls and no resource allocation. Prefer
    /// [`DaemonCapabilities::get`] when you want the process-wide memoised
    /// value; call this directly only when you intentionally want a fresh
    /// snapshot (e.g. tests).
    #[must_use]
    pub fn probe() -> Self {
        let is_root = zlayer_paths::is_root();
        let cgroup_parent = current_cgroup_v2_path();
        let is_nested = cgroup_parent.is_some();
        let can_write_cgroup_root = probe_can_write_cgroup_root();
        let has_cap_net_admin = probe_has_cap_net_admin();
        let tun_device_available = probe_tun_device_available();

        let effective_mode = if !is_nested && can_write_cgroup_root {
            DaemonMode::Full
        } else if (is_nested && cgroup_parent.is_some()) || can_write_cgroup_root {
            DaemonMode::NestedAdaptive
        } else {
            DaemonMode::Degraded
        };

        Self {
            is_root,
            is_nested,
            cgroup_parent,
            can_write_cgroup_root,
            has_cap_net_admin,
            tun_device_available,
            effective_mode,
        }
    }
}

/// Pure parser for the contents of `/proc/self/cgroup`.
///
/// Finds the cgroup-v2 line (prefix `0::`) and returns the path suffix with
/// surrounding whitespace trimmed. Returns `None` when:
/// - the input has no `0::` line (cgroup-v1-only host), or
/// - the v2 path is exactly `/` (host root — bare-metal, no enclosing cgroup), or
/// - the input is empty.
#[cfg(target_os = "linux")]
fn parse_cgroup_v2_line(content: &str) -> Option<String> {
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("0::") {
            let trimmed = rest.trim();
            if trimmed.is_empty() || trimmed == "/" {
                return None;
            }
            return Some(trimmed.to_string());
        }
    }
    None
}

/// Returns the current process's cgroup-v2 path, if any.
///
/// On Linux reads `/proc/self/cgroup` and delegates to `parse_cgroup_v2_line`.
/// On non-Linux always returns `None`. Returns `None` on any read error or
/// when the process is at the cgroup-v2 root (bare-metal case).
#[cfg(target_os = "linux")]
#[must_use]
pub fn current_cgroup_v2_path() -> Option<String> {
    let content = std::fs::read_to_string("/proc/self/cgroup").ok()?;
    parse_cgroup_v2_line(&content)
}

#[cfg(not(target_os = "linux"))]
#[must_use]
pub fn current_cgroup_v2_path() -> Option<String> {
    None
}

#[cfg(target_os = "linux")]
fn probe_can_write_cgroup_root() -> bool {
    use std::os::unix::fs::PermissionsExt;

    std::fs::metadata("/sys/fs/cgroup/cgroup.subtree_control")
        .ok()
        .is_some_and(|m| m.permissions().mode() & 0o200 != 0)
}

#[cfg(not(target_os = "linux"))]
fn probe_can_write_cgroup_root() -> bool {
    false
}

#[cfg(target_os = "linux")]
fn probe_has_cap_net_admin() -> bool {
    // The `libc` crate does not expose capability constants. CAP_NET_ADMIN is
    // defined as 12 in `<linux/capability.h>` and is part of the stable Linux
    // ABI.
    const CAP_NET_ADMIN: libc::c_ulong = 12;

    // SAFETY: PR_CAPBSET_READ is a read-only prctl that returns 1 if the
    // capability is in the bounding set, 0 if not, and -1 on error. No
    // pointer arguments are passed and the kernel does not retain state.
    #[allow(unsafe_code)]
    let rc = unsafe { libc::prctl(libc::PR_CAPBSET_READ, CAP_NET_ADMIN, 0, 0, 0) };
    rc == 1
}

#[cfg(not(target_os = "linux"))]
fn probe_has_cap_net_admin() -> bool {
    false
}

#[cfg(target_os = "linux")]
fn probe_tun_device_available() -> bool {
    use std::os::unix::fs::OpenOptionsExt;

    // Opening /dev/net/tun without any ioctls is benign and does not allocate
    // a TUN interface. The fd is dropped immediately when this scope ends.
    // Any open error — missing device, no perms, kernel module not loaded,
    // FD exhaustion — means we can't actually use TUN. Treat as unavailable.
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open("/dev/net/tun")
        .is_ok()
}

#[cfg(not(target_os = "linux"))]
fn probe_tun_device_available() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_does_not_panic_and_returns_valid_struct() {
        let caps = DaemonCapabilities::probe();
        // Sanity: the mode must be derivable from the boolean fields. Re-run
        // the derivation and compare.
        let expected = if !caps.is_nested && caps.can_write_cgroup_root {
            DaemonMode::Full
        } else if (caps.is_nested && caps.cgroup_parent.is_some()) || caps.can_write_cgroup_root {
            DaemonMode::NestedAdaptive
        } else {
            DaemonMode::Degraded
        };
        assert_eq!(caps.effective_mode, expected);

        // is_nested must agree with cgroup_parent.is_some().
        assert_eq!(caps.is_nested, caps.cgroup_parent.is_some());
    }

    #[test]
    fn serializes_round_trip_via_serde_json() {
        let caps = DaemonCapabilities::probe();
        let json = serde_json::to_string(&caps).expect("serialize");
        let parsed: DaemonCapabilities = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.is_root, caps.is_root);
        assert_eq!(parsed.is_nested, caps.is_nested);
        assert_eq!(parsed.cgroup_parent, caps.cgroup_parent);
        assert_eq!(parsed.can_write_cgroup_root, caps.can_write_cgroup_root);
        assert_eq!(parsed.has_cap_net_admin, caps.has_cap_net_admin);
        assert_eq!(parsed.tun_device_available, caps.tun_device_available);
        assert_eq!(parsed.effective_mode, caps.effective_mode);
    }

    #[test]
    fn daemon_mode_serde_uses_snake_case() {
        assert_eq!(
            serde_json::to_string(&DaemonMode::Full).unwrap(),
            "\"full\""
        );
        assert_eq!(
            serde_json::to_string(&DaemonMode::NestedAdaptive).unwrap(),
            "\"nested_adaptive\""
        );
        assert_eq!(
            serde_json::to_string(&DaemonMode::Degraded).unwrap(),
            "\"degraded\""
        );
    }

    #[cfg(target_os = "linux")]
    mod cgroup_parser {
        use super::super::parse_cgroup_v2_line;

        #[test]
        fn parse_cgroup_v2_root_returns_none() {
            assert_eq!(parse_cgroup_v2_line("0::/\n"), None);
        }

        #[test]
        fn parse_cgroup_v2_path_returns_some() {
            assert_eq!(
                parse_cgroup_v2_line("0::/system.slice/forgejo-runner.service\n"),
                Some("/system.slice/forgejo-runner.service".to_string())
            );
        }

        #[test]
        fn parse_cgroup_v2_hybrid_finds_v2_line() {
            let input = "12:devices:/user.slice\n11:memory:/user.slice\n0::/foo\n";
            assert_eq!(parse_cgroup_v2_line(input), Some("/foo".to_string()));
        }

        #[test]
        fn parse_cgroup_v2_no_newline() {
            assert_eq!(parse_cgroup_v2_line("0::/bar"), Some("/bar".to_string()));
        }

        #[test]
        fn parse_cgroup_v2_missing_returns_none() {
            assert_eq!(parse_cgroup_v2_line(""), None);
        }
    }
}
