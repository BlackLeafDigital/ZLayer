//! Docker `Healthcheck` -> `ZLayer` `HealthSpec`.
//!
//! Docker encodes the probe argv plus its cadence as nanosecond integers in a
//! single block. `ZLayer`'s [`HealthSpec`] uses [`std::time::Duration`], so the
//! translator does:
//! - argv decoding into [`HealthCheck::Command`]
//! - nanosecond -> [`Duration`] conversion
//! - the `["NONE"]` opt-out (Docker's "disable any image-baked healthcheck")
//!   maps to `Option::None`, telling the daemon not to install a probe.

use std::time::Duration;

use zlayer_types::spec::{HealthCheck, HealthSpec};

use crate::socket::types::container_create::HealthcheckBody;

/// Default retry count when Docker omits the field, matching `ZLayer`'s
/// `HealthSpec` default.
const DEFAULT_RETRIES: u32 = 3;

/// Translate a [`HealthcheckBody`] into a [`HealthSpec`].
///
/// Returns `None` when:
/// - the input itself is `None`
/// - `Test` is empty (Docker's "inherit image healthcheck" sentinel — we
///   leave it to the runtime to surface the image-baked probe)
/// - `Test == ["NONE"]` (Docker's explicit "disable" sentinel)
#[must_use]
pub fn translate(body: Option<&HealthcheckBody>) -> Option<HealthSpec> {
    let body = body?;
    if body.test.is_empty() {
        return None;
    }
    if body.test.len() == 1 && body.test[0] == "NONE" {
        return None;
    }

    let check = command_from_test(&body.test)?;

    let retries = body
        .retries
        .filter(|r| *r >= 0)
        .and_then(|r| u32::try_from(r).ok())
        .unwrap_or(DEFAULT_RETRIES);

    Some(HealthSpec {
        start_grace: nanos_to_duration(body.start_period),
        interval: nanos_to_duration(body.interval),
        timeout: nanos_to_duration(body.timeout),
        retries,
        check,
    })
}

/// Decode a Docker `Test` array into a [`HealthCheck::Command`].
///
/// Recognised forms:
/// - `["CMD-SHELL", "<cmd-line>"]` — single shell-line probe; the line is
///   stored as-is and is expected to be re-shelled by the runtime.
/// - `["CMD", "<exe>", "<arg1>", ...]` — argv form; tokens are space-joined
///   to keep [`HealthCheck::Command::command`]'s `String` shape.
/// - Any other non-empty form is treated as a bare argv vector and joined
///   the same way (matches Docker's permissive parsing).
fn command_from_test(test: &[String]) -> Option<HealthCheck> {
    match test.first().map(String::as_str) {
        // CMD-SHELL and CMD share the same translation path: take the rest
        // of the argv after the directive token and space-join it. Docker's
        // own runtime distinguishes them at probe-execution time (CMD-SHELL
        // wraps the line in `sh -c`, CMD execs argv directly), but at the
        // ZLayer translation layer the result is a single command string.
        Some("CMD-SHELL" | "CMD") if test.len() >= 2 => Some(HealthCheck::Command {
            command: test[1..].join(" "),
        }),
        Some(_) => Some(HealthCheck::Command {
            command: test.join(" "),
        }),
        None => None,
    }
}

fn nanos_to_duration(nanos: Option<i64>) -> Option<Duration> {
    nanos
        .filter(|n| *n > 0)
        .and_then(|n| u64::try_from(n).ok())
        .map(Duration::from_nanos)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn none_input_returns_none() {
        assert!(translate(None).is_none());
    }

    #[test]
    fn empty_test_array_returns_none() {
        let body = HealthcheckBody::default();
        assert!(translate(Some(&body)).is_none());
    }

    #[test]
    fn explicit_none_disables_probe() {
        let body = HealthcheckBody {
            test: vec!["NONE".to_string()],
            ..HealthcheckBody::default()
        };
        assert!(translate(Some(&body)).is_none());
    }

    #[test]
    fn cmd_shell_form_translates() {
        let body = HealthcheckBody {
            test: vec![
                "CMD-SHELL".to_string(),
                "curl -f http://localhost/ || exit 1".to_string(),
            ],
            interval: Some(30_000_000_000),
            timeout: Some(5_000_000_000),
            retries: Some(4),
            start_period: Some(10_000_000_000),
            ..HealthcheckBody::default()
        };
        let spec = translate(Some(&body)).unwrap();
        assert_eq!(spec.retries, 4);
        assert_eq!(spec.interval, Some(Duration::from_secs(30)));
        assert_eq!(spec.timeout, Some(Duration::from_secs(5)));
        assert_eq!(spec.start_grace, Some(Duration::from_secs(10)));
        match spec.check {
            HealthCheck::Command { command } => {
                assert_eq!(command, "curl -f http://localhost/ || exit 1");
            }
            other => panic!("expected Command, got {other:?}"),
        }
    }

    #[test]
    fn cmd_argv_form_joins_with_spaces() {
        let body = HealthcheckBody {
            test: vec![
                "CMD".to_string(),
                "pg_isready".to_string(),
                "-U".to_string(),
                "postgres".to_string(),
            ],
            ..HealthcheckBody::default()
        };
        let spec = translate(Some(&body)).unwrap();
        match spec.check {
            HealthCheck::Command { command } => assert_eq!(command, "pg_isready -U postgres"),
            other => panic!("expected Command, got {other:?}"),
        }
    }

    #[test]
    fn missing_retries_defaults_to_three() {
        let body = HealthcheckBody {
            test: vec!["CMD".to_string(), "true".to_string()],
            ..HealthcheckBody::default()
        };
        assert_eq!(translate(Some(&body)).unwrap().retries, DEFAULT_RETRIES);
    }

    #[test]
    fn negative_durations_are_dropped() {
        let body = HealthcheckBody {
            test: vec!["CMD".to_string(), "true".to_string()],
            interval: Some(-1),
            timeout: Some(0),
            ..HealthcheckBody::default()
        };
        let spec = translate(Some(&body)).unwrap();
        assert!(spec.interval.is_none());
        assert!(spec.timeout.is_none());
    }

    #[test]
    fn bare_argv_without_cmd_prefix_still_parses() {
        // Some clients omit the leading "CMD"; Docker accepts the bare argv.
        let body = HealthcheckBody {
            test: vec!["pg_isready".to_string()],
            ..HealthcheckBody::default()
        };
        let spec = translate(Some(&body)).unwrap();
        match spec.check {
            HealthCheck::Command { command } => assert_eq!(command, "pg_isready"),
            other => panic!("expected Command, got {other:?}"),
        }
    }
}
