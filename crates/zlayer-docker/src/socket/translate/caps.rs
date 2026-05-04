//! Docker `HostConfig.CapAdd` / `CapDrop` -> `ZLayer` capability lists.
//!
//! Returns two flat `Vec<String>`s rather than a single struct so the caller
//! can wire them into [`zlayer_types::api::containers::CreateContainerRequest`]'s
//! `cap_add` / `cap_drop` fields without intermediate massaging.
//!
//! Docker accepts capability names with or without the `CAP_` prefix
//! (e.g. both `NET_ADMIN` and `CAP_NET_ADMIN`). `ZLayer`'s downstream
//! validation is also tolerant, so we normalise to the unprefixed form on
//! the way through — this keeps wire-bug-for-wire-bug compat with tools
//! that emit one form vs the other.

use crate::socket::types::container_create::HostConfig;

/// Translate cap-add / cap-drop arrays into normalised `(add, drop)` lists.
#[must_use]
pub fn translate(hc: &HostConfig) -> (Vec<String>, Vec<String>) {
    let cap_add = hc.cap_add.iter().map(|c| normalize(c)).collect();
    let cap_drop = hc.cap_drop.iter().map(|c| normalize(c)).collect();
    (cap_add, cap_drop)
}

/// Strip the optional `CAP_` prefix and uppercase the result.
fn normalize(cap: &str) -> String {
    let trimmed = cap.trim();
    let stripped = trimmed
        .strip_prefix("CAP_")
        .or_else(|| trimmed.strip_prefix("cap_"))
        .unwrap_or(trimmed);
    stripped.to_ascii_uppercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passes_named_caps_through() {
        let hc = HostConfig {
            cap_add: vec!["NET_ADMIN".to_string(), "SYS_PTRACE".to_string()],
            cap_drop: vec!["MKNOD".to_string()],
            ..HostConfig::default()
        };
        let (add, drop) = translate(&hc);
        assert_eq!(add, vec!["NET_ADMIN".to_string(), "SYS_PTRACE".to_string()]);
        assert_eq!(drop, vec!["MKNOD".to_string()]);
    }

    #[test]
    fn strips_cap_prefix() {
        let hc = HostConfig {
            cap_add: vec!["CAP_NET_ADMIN".to_string()],
            cap_drop: vec!["CAP_MKNOD".to_string()],
            ..HostConfig::default()
        };
        let (add, drop) = translate(&hc);
        assert_eq!(add, vec!["NET_ADMIN".to_string()]);
        assert_eq!(drop, vec!["MKNOD".to_string()]);
    }

    #[test]
    fn lowercase_input_is_uppercased() {
        let hc = HostConfig {
            cap_add: vec!["net_admin".to_string(), "cap_sys_admin".to_string()],
            ..HostConfig::default()
        };
        let (add, _) = translate(&hc);
        assert_eq!(add, vec!["NET_ADMIN".to_string(), "SYS_ADMIN".to_string()]);
    }

    #[test]
    fn empty_lists_translate_to_empty() {
        let hc = HostConfig::default();
        let (add, drop) = translate(&hc);
        assert!(add.is_empty());
        assert!(drop.is_empty());
    }

    #[test]
    fn surrounding_whitespace_is_trimmed() {
        let hc = HostConfig {
            cap_add: vec!["  NET_ADMIN  ".to_string()],
            ..HostConfig::default()
        };
        let (add, _) = translate(&hc);
        assert_eq!(add, vec!["NET_ADMIN".to_string()]);
    }
}
