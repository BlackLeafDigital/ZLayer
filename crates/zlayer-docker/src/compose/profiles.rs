//! Docker Compose profiles filter.
//!
//! Compose lets a service declare a list of profiles via `profiles: [<name>, ...]`.
//! A service that has no profiles is always part of the active set; a service that
//! does declare profiles is only active when at least one of its profiles is
//! currently activated.
//!
//! Active profiles come from two sources:
//!
//! * `--profile <name>` CLI flags (repeatable).
//! * The `COMPOSE_PROFILES` environment variable (comma-separated).
//!
//! The two sources are merged into a single set; entries are trimmed of
//! surrounding whitespace and empty entries are dropped.

use std::collections::{HashMap, HashSet};

use super::types::ComposeService;

/// Environment variable Compose reads to discover active profiles.
const COMPOSE_PROFILES_ENV: &str = "COMPOSE_PROFILES";

/// Compute the set of active profiles from CLI flags and environment.
///
/// CLI flags are taken verbatim (each entry in `cli_flags` is treated as one
/// profile name; trimmed and dropped if empty). The `COMPOSE_PROFILES`
/// environment variable, if present, is split on commas and each piece is
/// trimmed and dropped if empty.
///
/// The two inputs are unioned into a single [`HashSet`].
#[must_use]
#[allow(clippy::implicit_hasher)]
pub fn select_active_profiles(
    cli_flags: &[String],
    env: &HashMap<String, String>,
) -> HashSet<String> {
    let mut active: HashSet<String> = HashSet::new();

    for flag in cli_flags {
        let trimmed = flag.trim();
        if !trimmed.is_empty() {
            active.insert(trimmed.to_string());
        }
    }

    if let Some(raw) = env.get(COMPOSE_PROFILES_ENV) {
        for part in raw.split(',') {
            let trimmed = part.trim();
            if !trimmed.is_empty() {
                active.insert(trimmed.to_string());
            }
        }
    }

    active
}

/// Return `true` iff a service with the given declared profiles should be
/// active given the currently active profile set.
///
/// Rules:
/// * A service with no declared profiles is **always** active.
/// * A service with declared profiles is active **only** if at least one of
///   its profiles is in `active`.
#[must_use]
#[allow(clippy::implicit_hasher)]
pub fn service_is_active(service_profiles: &[String], active: &HashSet<String>) -> bool {
    if service_profiles.is_empty() {
        return true;
    }
    service_profiles.iter().any(|p| active.contains(p))
}

/// Filter a `services` map down to those active under the given profile set.
///
/// The returned map borrows from the input and contains exactly the entries
/// for which [`service_is_active`] returns `true`.
#[must_use]
#[allow(clippy::implicit_hasher)]
pub fn filter_services_by_profile<'a>(
    services: &'a HashMap<String, ComposeService>,
    active: &HashSet<String>,
) -> HashMap<&'a String, &'a ComposeService> {
    services
        .iter()
        .filter(|(_, svc)| service_is_active(&svc.profiles, active))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal `ComposeService` carrying only `image:` and the
    /// supplied `profiles:` list. Implemented via YAML deserialisation so
    /// the helper stays in sync with the (large, evolving) `ComposeService`
    /// field set without enumerating every default at the call site.
    fn make_service(profiles: &[&str]) -> ComposeService {
        let yaml = if profiles.is_empty() {
            "image: alpine\n".to_string()
        } else {
            let list = profiles
                .iter()
                .map(|p| format!("  - {p}"))
                .collect::<Vec<_>>()
                .join("\n");
            format!("image: alpine\nprofiles:\n{list}\n")
        };
        serde_yaml::from_str(&yaml).expect("ComposeService YAML parses")
    }

    #[test]
    fn env_single_profile() {
        let mut env = HashMap::new();
        env.insert("COMPOSE_PROFILES".to_string(), "dev".to_string());

        let active = select_active_profiles(&[], &env);

        assert_eq!(active.len(), 1);
        assert!(active.contains("dev"));
    }

    #[test]
    fn env_comma_separated_with_whitespace() {
        let mut env = HashMap::new();
        env.insert(
            "COMPOSE_PROFILES".to_string(),
            "dev, test ,  staging".to_string(),
        );

        let active = select_active_profiles(&[], &env);

        assert_eq!(active.len(), 3);
        assert!(active.contains("dev"));
        assert!(active.contains("test"));
        assert!(active.contains("staging"));
    }

    #[test]
    fn env_empty_and_blank_entries_dropped() {
        let mut env = HashMap::new();
        env.insert("COMPOSE_PROFILES".to_string(), ",, ,".to_string());

        let active = select_active_profiles(&[], &env);

        assert!(active.is_empty());
    }

    #[test]
    fn env_unset_yields_empty_active_set() {
        let env = HashMap::new();

        let active = select_active_profiles(&[], &env);

        assert!(active.is_empty());
    }

    #[test]
    fn cli_flags_merge_with_env_and_dedupe() {
        let mut env = HashMap::new();
        env.insert("COMPOSE_PROFILES".to_string(), "dev,prod".to_string());

        let cli = vec![
            "debug".to_string(),
            "dev".to_string(), // duplicate of env value
            "  staging  ".to_string(),
            String::new(), // dropped
        ];

        let active = select_active_profiles(&cli, &env);

        assert_eq!(active.len(), 4);
        assert!(active.contains("dev"));
        assert!(active.contains("prod"));
        assert!(active.contains("debug"));
        assert!(active.contains("staging"));
    }

    #[test]
    fn service_with_no_profiles_is_always_active() {
        let svc_profiles: Vec<String> = Vec::new();

        // Empty active set
        assert!(service_is_active(&svc_profiles, &HashSet::new()));

        // Non-empty active set
        let mut active = HashSet::new();
        active.insert("dev".to_string());
        assert!(service_is_active(&svc_profiles, &active));
    }

    #[test]
    fn service_with_profiles_requires_match() {
        let svc_profiles = vec!["debug".to_string(), "tools".to_string()];

        // No active profiles: not active.
        assert!(!service_is_active(&svc_profiles, &HashSet::new()));

        // Active set with no overlap: not active.
        let mut other = HashSet::new();
        other.insert("prod".to_string());
        assert!(!service_is_active(&svc_profiles, &other));

        // Active set overlaps on one profile: active.
        let mut overlap = HashSet::new();
        overlap.insert("tools".to_string());
        assert!(service_is_active(&svc_profiles, &overlap));
    }

    #[test]
    fn filter_services_round_trip() {
        let mut services: HashMap<String, ComposeService> = HashMap::new();
        services.insert("web".to_string(), make_service(&[]));
        services.insert("db".to_string(), make_service(&[]));
        services.insert("debug".to_string(), make_service(&["debug"]));
        services.insert("seed".to_string(), make_service(&["tools", "seed"]));
        services.insert("ml".to_string(), make_service(&["gpu"]));

        // Empty active set: only the always-on services survive.
        let filtered = filter_services_by_profile(&services, &HashSet::new());
        assert_eq!(filtered.len(), 2);
        assert!(filtered.contains_key(&"web".to_string()));
        assert!(filtered.contains_key(&"db".to_string()));

        // Activate "debug": web, db, and debug should be present.
        let mut active = HashSet::new();
        active.insert("debug".to_string());
        let filtered = filter_services_by_profile(&services, &active);
        assert_eq!(filtered.len(), 3);
        assert!(filtered.contains_key(&"web".to_string()));
        assert!(filtered.contains_key(&"db".to_string()));
        assert!(filtered.contains_key(&"debug".to_string()));
        assert!(!filtered.contains_key(&"seed".to_string()));
        assert!(!filtered.contains_key(&"ml".to_string()));

        // Activate "seed" (matches one of seed's two profiles).
        let mut active = HashSet::new();
        active.insert("seed".to_string());
        let filtered = filter_services_by_profile(&services, &active);
        assert_eq!(filtered.len(), 3);
        assert!(filtered.contains_key(&"seed".to_string()));

        // Activate every profile: every service is present.
        let mut active = HashSet::new();
        active.insert("debug".to_string());
        active.insert("tools".to_string());
        active.insert("gpu".to_string());
        let filtered = filter_services_by_profile(&services, &active);
        assert_eq!(filtered.len(), services.len());
    }

    #[test]
    fn filter_borrows_from_source_map() {
        // Sanity check that the returned map references the original services
        // (i.e. the returned value's lifetime is tied to the input map).
        let mut services: HashMap<String, ComposeService> = HashMap::new();
        services.insert("web".to_string(), make_service(&[]));

        let filtered = filter_services_by_profile(&services, &HashSet::new());
        let (name, svc) = filtered.iter().next().expect("one service");
        assert_eq!(*name, &"web".to_string());
        assert_eq!(svc.image.as_deref(), Some("alpine"));
    }
}
