//! Shared helpers for resolving deployment/service names from CLI args.
//!
//! Most user-facing commands (`logs`, `restart`, `exec`, ...) accept a service
//! name and an optional `--deployment` flag. This module centralises the logic
//! for turning those inputs into a concrete `(deployment, service)` pair:
//!
//! * unambiguous match → return it directly,
//! * multiple matches on a TTY → interactive prompt,
//! * multiple matches off a TTY (or zero matches) → actionable error listing
//!   the candidates.
//!
//! Used by `logs`, `stop`, and other deployment/service-scoped commands.

use std::io::IsTerminal;

use anyhow::{anyhow, bail, Result};
use dialoguer::Select;
use tracing::debug;

use crate::daemon_client::DaemonClient;

/// A candidate (deployment, service) pair matched during resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceMatch {
    pub deployment: String,
    pub service: String,
}

/// Resolve a service name to a (deployment, service) pair.
///
/// Behavior:
/// - If `deployment_hint` is `Some(d)`: verifies `service` exists in `d`;
///   errors with the list of services in `d` otherwise.
/// - If `deployment_hint` is `None`: scans all deployments for services
///   matching `service_name`.
///   - 0 matches: error listing all available services across all deployments.
///   - 1 match: returns it directly.
///   - N matches: if stdout is a TTY, shows a `dialoguer::Select` prompt
///     listing each `{deployment}/{service}`; if non-TTY, errors with the
///     list and tells the user to pass `--deployment`.
pub async fn resolve_service(
    client: &DaemonClient,
    service_name: &str,
    deployment_hint: Option<&str>,
) -> Result<ServiceMatch> {
    if let Some(deployment) = deployment_hint {
        let services = client
            .list_services(deployment)
            .await
            .map_err(|e| anyhow!("Failed to list services in deployment '{deployment}': {e}"))?;

        let names: Vec<String> = services
            .iter()
            .filter_map(|s| s["name"].as_str().map(str::to_string))
            .collect();

        if names.iter().any(|n| n == service_name) {
            return Ok(ServiceMatch {
                deployment: deployment.to_string(),
                service: service_name.to_string(),
            });
        }

        let available = format_options(&names);
        bail!("Service '{service_name}' not found in deployment '{deployment}'.\n  Available: {available}");
    }

    let (matches, all_known) = scan_all_deployments(client, service_name).await?;
    let is_tty = std::io::stdout().is_terminal();

    match select_from_candidates(service_name, matches.clone(), is_tty, &all_known)? {
        Some(chosen) => Ok(chosen),
        None => prompt_for_match(&matches),
    }
}

/// Resolve a deployment name only (no service). Useful for deployment-scoped
/// commands with no service arg (e.g. `zlayer stop`).
///
/// - `Some(name)`: returns `name` (no existence check — caller's API call
///   will 404 if wrong).
/// - `None`:
///   - 0 deployments: error "no deployments found".
///   - 1 deployment: return it.
///   - N deployments: TTY prompt via `dialoguer::Select`; non-TTY errors with
///     the list.
pub async fn resolve_deployment(client: &DaemonClient, hint: Option<&str>) -> Result<String> {
    if let Some(name) = hint {
        return Ok(name.to_string());
    }

    let deployments = client
        .list_deployments()
        .await
        .map_err(|e| anyhow!("Failed to list deployments: {e}"))?;

    let names: Vec<String> = deployments
        .iter()
        .filter_map(|d| {
            d["name"]
                .as_str()
                .or_else(|| d["deployment"].as_str())
                .map(str::to_string)
        })
        .collect();

    match names.len() {
        0 => bail!("No deployments found. Deploy something first."),
        1 => Ok(names.into_iter().next().expect("len == 1")),
        _ => {
            if std::io::stdout().is_terminal() {
                let idx = Select::new()
                    .with_prompt("Multiple deployments — pick one")
                    .items(&names)
                    .interact()
                    .map_err(|e| anyhow!("Interactive prompt failed: {e}"))?;
                Ok(names.into_iter().nth(idx).expect("idx in range"))
            } else {
                let count = names.len();
                let available = format_options(&names);
                bail!("Multiple deployments found ({count}). Specify one with --deployment.\n  Available: {available}\n  Hint: pass --deployment <name> to disambiguate.");
            }
        }
    }
}

/// Pure selection helper — given match candidates, decide what to do.
///
/// Returns `Ok(Some(chosen))` for direct pick, `Ok(None)` for "needs prompt",
/// or `Err(...)` for zero / needs-disambiguation-but-non-TTY.
pub(crate) fn select_from_candidates(
    service_name: &str,
    candidates: Vec<ServiceMatch>,
    is_tty: bool,
    all_known_services: &[ServiceMatch],
) -> Result<Option<ServiceMatch>> {
    match candidates.len() {
        0 => {
            let labels: Vec<String> = all_known_services
                .iter()
                .map(|m| format!("{}/{}", m.deployment, m.service))
                .collect();
            let available = format_options(&labels);
            bail!("Service '{service_name}' not found in any deployment.\n  Available: {available}\n  Hint: pass --deployment <name> to disambiguate.");
        }
        1 => Ok(Some(candidates.into_iter().next().expect("len == 1"))),
        _ => {
            if is_tty {
                Ok(None)
            } else {
                let count = candidates.len();
                let labels: Vec<String> = candidates
                    .iter()
                    .map(|m| format!("{}/{}", m.deployment, m.service))
                    .collect();
                let matches_str = format_options(&labels);
                bail!("Service '{service_name}' is ambiguous — {count} matches across deployments.\n  Matches: {matches_str}\n  Hint: pass --deployment <name> to disambiguate.");
            }
        }
    }
}

/// Scan every deployment for services matching `service_name`. Also returns
/// the full known-service list (used for the zero-match error message).
async fn scan_all_deployments(
    client: &DaemonClient,
    service_name: &str,
) -> Result<(Vec<ServiceMatch>, Vec<ServiceMatch>)> {
    let deployments = client
        .list_deployments()
        .await
        .map_err(|e| anyhow!("Failed to list deployments: {e}"))?;

    let mut matches = Vec::new();
    let mut all_known = Vec::new();

    for d in &deployments {
        let Some(deployment_name) = d["name"]
            .as_str()
            .or_else(|| d["deployment"].as_str())
            .map(str::to_string)
        else {
            continue;
        };

        let services = match client.list_services(&deployment_name).await {
            Ok(s) => s,
            Err(e) => {
                debug!(deployment = %deployment_name, error = %e, "skipping deployment during resolution");
                continue;
            }
        };

        for s in services {
            let Some(svc_name) = s["name"].as_str().map(str::to_string) else {
                continue;
            };
            let candidate = ServiceMatch {
                deployment: deployment_name.clone(),
                service: svc_name.clone(),
            };
            if svc_name == service_name {
                matches.push(candidate.clone());
            }
            all_known.push(candidate);
        }
    }

    Ok((matches, all_known))
}

/// Show the interactive `dialoguer::Select` prompt to disambiguate matches.
fn prompt_for_match(candidates: &[ServiceMatch]) -> Result<ServiceMatch> {
    let labels: Vec<String> = candidates
        .iter()
        .map(|m| format!("{}/{}", m.deployment, m.service))
        .collect();
    let idx = Select::new()
        .with_prompt("Multiple services match — pick one")
        .items(&labels)
        .interact()
        .map_err(|e| anyhow!("Interactive prompt failed: {e}"))?;
    Ok(candidates[idx].clone())
}

/// Format a list of options for an error message: first 10, then "... and N more".
fn format_options(options: &[String]) -> String {
    if options.is_empty() {
        return "(none)".to_string();
    }
    if options.len() <= 10 {
        return options.join(", ");
    }
    let head: Vec<&str> = options.iter().take(10).map(String::as_str).collect();
    format!("{}, ... and {} more", head.join(", "), options.len() - 10)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_candidates(pairs: &[(&str, &str)]) -> Vec<ServiceMatch> {
        pairs
            .iter()
            .map(|(d, s)| ServiceMatch {
                deployment: (*d).to_string(),
                service: (*s).to_string(),
            })
            .collect()
    }

    #[test]
    fn zero_candidates_errors_with_listing() {
        let all = make_candidates(&[("dep-a", "api"), ("dep-b", "worker")]);
        let err =
            select_from_candidates("nope", Vec::new(), true, &all).expect_err("must error on zero");
        let msg = format!("{err}");
        assert!(
            msg.contains("nope"),
            "error should mention service name: {msg}"
        );
        assert!(
            msg.contains("dep-a/api"),
            "error should list known services: {msg}"
        );
        assert!(
            msg.contains("dep-b/worker"),
            "error should list known services: {msg}"
        );
    }

    #[test]
    fn zero_candidates_with_no_services_says_none() {
        let err =
            select_from_candidates("nope", Vec::new(), true, &[]).expect_err("must error on zero");
        let msg = format!("{err}");
        assert!(
            msg.contains("(none)"),
            "empty list should render as (none): {msg}"
        );
    }

    #[test]
    fn single_candidate_returns_directly() {
        let candidates = make_candidates(&[("dep-a", "api")]);
        let chosen = select_from_candidates("api", candidates.clone(), false, &candidates)
            .expect("single match must succeed off-tty too");
        assert_eq!(chosen, Some(candidates.into_iter().next().unwrap()));
    }

    #[test]
    fn multiple_candidates_on_tty_returns_none() {
        let candidates = make_candidates(&[("dep-a", "api"), ("dep-b", "api")]);
        let result = select_from_candidates("api", candidates.clone(), true, &candidates)
            .expect("ambiguous-on-tty must defer to caller");
        assert_eq!(result, None, "TTY path must return None so caller prompts");
    }

    #[test]
    fn multiple_candidates_off_tty_errors_with_hint() {
        let candidates = make_candidates(&[("dep-a", "api"), ("dep-b", "api")]);
        let err = select_from_candidates("api", candidates.clone(), false, &candidates)
            .expect_err("ambiguous-off-tty must error");
        let msg = format!("{err}");
        assert!(
            msg.contains("--deployment"),
            "error must mention --deployment: {msg}"
        );
        assert!(msg.contains("dep-a/api"), "error must list matches: {msg}");
        assert!(msg.contains("dep-b/api"), "error must list matches: {msg}");
    }

    #[test]
    fn format_options_truncates_after_ten() {
        let many: Vec<String> = (0..15).map(|i| format!("svc-{i}")).collect();
        let formatted = format_options(&many);
        assert!(
            formatted.contains("svc-0"),
            "first item present: {formatted}"
        );
        assert!(
            formatted.contains("svc-9"),
            "tenth item present: {formatted}"
        );
        assert!(
            !formatted.contains("svc-10"),
            "eleventh suppressed: {formatted}"
        );
        assert!(
            formatted.contains("and 5 more"),
            "tail summary present: {formatted}"
        );
    }

    #[test]
    fn format_options_lists_all_when_short() {
        let opts: Vec<String> = vec!["a".into(), "b".into(), "c".into()];
        assert_eq!(format_options(&opts), "a, b, c");
    }
}
