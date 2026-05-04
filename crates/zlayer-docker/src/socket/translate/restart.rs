//! Docker `HostConfig.RestartPolicy` -> `ZLayer` `ContainerRestartPolicy`.
//!
//! Docker emits a `{ Name, MaximumRetryCount }` pair where `Name` is one of
//! `""`, `"no"`, `"always"`, `"unless-stopped"`, or `"on-failure"`. The empty
//! string and `"no"` both translate to [`ContainerRestartKind::No`] (Docker's
//! documented default).

use zlayer_types::spec::{ContainerRestartKind, ContainerRestartPolicy};

use crate::socket::types::container_create::RestartPolicy;

/// Translate a Docker [`RestartPolicy`] into `ZLayer`'s
/// [`ContainerRestartPolicy`].
///
/// Returns `None` when the input is itself `None` (no policy on the request)
/// or when the `Name` field is missing — Docker's documented behaviour is to
/// treat that as "no policy supplied", which lets us preserve the existing
/// `CreateContainerRequest::restart_policy = None` semantics.
#[must_use]
pub fn translate(policy: Option<&RestartPolicy>) -> Option<ContainerRestartPolicy> {
    let policy = policy?;
    let name = policy.name.as_deref()?;
    let kind = match name {
        "always" => ContainerRestartKind::Always,
        "unless-stopped" => ContainerRestartKind::UnlessStopped,
        "on-failure" => ContainerRestartKind::OnFailure,
        // `""`, `"no"`, and any unknown name all clamp to `No`. Empty/`no`
        // are Docker's documented default; unknown names follow the
        // daemon's "be permissive on input" stance rather than rejecting
        // the request.
        _ => ContainerRestartKind::No,
    };

    let max_attempts = if matches!(kind, ContainerRestartKind::OnFailure) {
        policy
            .maximum_retry_count
            .filter(|n| *n > 0)
            .and_then(|n| u32::try_from(n).ok())
    } else {
        None
    };

    Some(ContainerRestartPolicy {
        kind,
        max_attempts,
        delay: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn none_input_returns_none() {
        assert!(translate(None).is_none());
    }

    #[test]
    fn missing_name_returns_none() {
        let p = RestartPolicy {
            name: None,
            maximum_retry_count: None,
        };
        assert!(translate(Some(&p)).is_none());
    }

    #[test]
    fn empty_name_translates_to_no() {
        let p = RestartPolicy {
            name: Some(String::new()),
            maximum_retry_count: None,
        };
        let out = translate(Some(&p)).unwrap();
        assert_eq!(out.kind, ContainerRestartKind::No);
        assert!(out.max_attempts.is_none());
    }

    #[test]
    fn no_name_translates_to_no() {
        let p = RestartPolicy {
            name: Some("no".to_string()),
            maximum_retry_count: None,
        };
        assert_eq!(translate(Some(&p)).unwrap().kind, ContainerRestartKind::No);
    }

    #[test]
    fn always_translates() {
        let p = RestartPolicy {
            name: Some("always".to_string()),
            maximum_retry_count: None,
        };
        assert_eq!(
            translate(Some(&p)).unwrap().kind,
            ContainerRestartKind::Always
        );
    }

    #[test]
    fn unless_stopped_translates() {
        let p = RestartPolicy {
            name: Some("unless-stopped".to_string()),
            maximum_retry_count: None,
        };
        assert_eq!(
            translate(Some(&p)).unwrap().kind,
            ContainerRestartKind::UnlessStopped
        );
    }

    #[test]
    fn on_failure_carries_max_attempts() {
        let p = RestartPolicy {
            name: Some("on-failure".to_string()),
            maximum_retry_count: Some(5),
        };
        let out = translate(Some(&p)).unwrap();
        assert_eq!(out.kind, ContainerRestartKind::OnFailure);
        assert_eq!(out.max_attempts, Some(5));
    }

    #[test]
    fn on_failure_with_zero_max_attempts_is_none() {
        // Docker's docs treat `0` as "retry forever"; we encode that as
        // `max_attempts: None` to match `ContainerRestartPolicy`'s shape.
        let p = RestartPolicy {
            name: Some("on-failure".to_string()),
            maximum_retry_count: Some(0),
        };
        let out = translate(Some(&p)).unwrap();
        assert_eq!(out.kind, ContainerRestartKind::OnFailure);
        assert!(out.max_attempts.is_none());
    }

    #[test]
    fn unknown_name_falls_back_to_no() {
        let p = RestartPolicy {
            name: Some("not-a-real-policy".to_string()),
            maximum_retry_count: Some(7),
        };
        let out = translate(Some(&p)).unwrap();
        assert_eq!(out.kind, ContainerRestartKind::No);
        assert!(out.max_attempts.is_none());
    }
}
