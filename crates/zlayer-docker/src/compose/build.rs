//! `docker compose build` planning + execution.
//!
//! Walks a [`super::ComposeFile`] (already loaded by
//! [`crate::cli::compose_cmd::load_project`]) and produces a [`BuildPlan`]
//! per service that ships a `build:` directive. The plan captures every
//! field the [`zlayer_builder::ImageBuilder`] needs (context, dockerfile,
//! args, target, `cache_from`, `no_cache`, tags) plus the canonical
//! `<project>-<service>:latest` tag emitted by
//! [`super::convert::build_image_tag`] so that `compose up` and
//! `compose build` agree on the image name.
//!
//! The planner is deliberately a pure function over [`super::ComposeFile`]
//! and the resolved working directory — no daemon traffic, no buildah —
//! so it can be unit-tested without spinning up a runtime.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::convert::build_image_tag;
use super::types::{ComposeBuild, ComposeFile};

/// Default Dockerfile name used by Compose when the long form omits
/// `dockerfile:`. Matches `docker buildx`.
const DEFAULT_DOCKERFILE: &str = "Dockerfile";

/// One service's build directive resolved into the concrete inputs the
/// builder backend needs. Produced by [`plan_builds`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildPlan {
    /// Compose project name (used for tagging).
    pub project: String,
    /// Compose service name within the project.
    pub service: String,
    /// Absolute build context directory.
    pub context: PathBuf,
    /// Absolute Dockerfile path. When the source long form supplies
    /// `dockerfile_inline`, this is `None`.
    pub dockerfile: Option<PathBuf>,
    /// Inline Dockerfile body (long-form `dockerfile_inline`). Mutually
    /// exclusive with `dockerfile`.
    pub dockerfile_inline: Option<String>,
    /// Build-time `ARG`s, key = name, value = value.
    pub args: HashMap<String, String>,
    /// Multi-stage `target:`.
    pub target: Option<String>,
    /// `--cache-from` images for the buildah cache layer.
    pub cache_from: Vec<String>,
    /// `--network` mode used during `RUN` instructions.
    pub network: Option<String>,
    /// `--shm-size` value (string, e.g. `"256m"`) — interpreted by the
    /// underlying buildah backend.
    pub shm_size: Option<String>,
    /// Labels applied to the produced image (`org.opencontainers.image.*`,
    /// user-supplied, ...).
    pub labels: HashMap<String, String>,
    /// Final tags to apply. Always begins with the canonical
    /// `<project>-<service>:latest`; user-supplied `tags:` from the
    /// long-form build directive are appended afterwards.
    pub tags: Vec<String>,
    /// Honour `compose build --no-cache`.
    pub no_cache: bool,
    /// Honour `compose build --pull` (force a fresh base-image pull).
    pub pull: bool,
}

impl BuildPlan {
    /// Canonical primary tag for this build (always
    /// `<project>-<service>:latest`).
    #[must_use]
    pub fn primary_tag(&self) -> String {
        build_image_tag(&self.project, &self.service)
    }
}

/// Errors surfaced when a `build:` directive cannot be turned into a
/// concrete [`BuildPlan`].
#[derive(Debug, thiserror::Error)]
pub enum BuildPlanError {
    /// The service has no `build:` directive (the caller asked us to plan
    /// a service that compose says is image-only).
    #[error("service '{0}' has no `build:` directive")]
    NoBuildDirective(String),
    /// Long-form build with neither `context:` nor `dockerfile_inline:`.
    /// Mirrored from [`crate::compose::convert`] so callers can detect the
    /// same failure shape regardless of whether they go through
    /// `compose_to_deployment` first.
    #[error(
        "service '{0}' has a `build:` directive with neither `context` nor `dockerfile_inline`"
    )]
    MissingContext(String),
    /// Caller named a service that is not in the compose project.
    #[error("service '{0}' not found in compose project")]
    UnknownService(String),
}

/// Build a list of [`BuildPlan`]s, one per service in `compose` that has a
/// `build:` directive. Services with only an `image:` are skipped silently
/// (matching `docker compose build`'s behaviour).
///
/// `services_filter`, when set, restricts the output to the named services
/// (and errors out if any name is unknown). When `None`, every buildable
/// service is included.
///
/// `no_cache` and `pull` thread the corresponding `compose build` flags
/// onto every produced plan.
///
/// `working_dir` is used to resolve relative `context:` paths. It is
/// typically the project working directory captured in
/// [`crate::cli::compose_cmd::LoadedProject::working_dir`].
///
/// # Errors
///
/// Returns [`BuildPlanError::UnknownService`] when `services_filter`
/// references a service that does not exist, or
/// [`BuildPlanError::MissingContext`] when a long-form `build:` has neither
/// `context` nor `dockerfile_inline`.
pub fn plan_builds(
    project: &str,
    compose: &ComposeFile,
    working_dir: &Path,
    services_filter: Option<&[String]>,
    no_cache: bool,
    pull: bool,
) -> Result<Vec<BuildPlan>, BuildPlanError> {
    if let Some(filter) = services_filter {
        for name in filter {
            if !compose.services.contains_key(name) {
                return Err(BuildPlanError::UnknownService(name.clone()));
            }
        }
    }

    let mut plans = Vec::new();
    // Stable iteration order: BTreeMap-style sort so test output and
    // human-facing logs are deterministic.
    let mut names: Vec<&String> = compose.services.keys().collect();
    names.sort();
    for svc_name in names {
        if let Some(filter) = services_filter {
            if !filter.iter().any(|s| s == svc_name) {
                continue;
            }
        }
        let svc = &compose.services[svc_name];
        let Some(build) = svc.build.as_ref() else {
            continue;
        };
        plans.push(plan_one(
            project,
            svc_name,
            build,
            working_dir,
            no_cache,
            pull,
        )?);
    }
    Ok(plans)
}

/// Plan a single service's build. Pulled out so test cases can drive it
/// directly without going through [`plan_builds`]'s service iteration.
fn plan_one(
    project: &str,
    svc_name: &str,
    build: &ComposeBuild,
    working_dir: &Path,
    no_cache: bool,
    pull: bool,
) -> Result<BuildPlan, BuildPlanError> {
    let primary_tag = build_image_tag(project, svc_name);
    let mut tags = vec![primary_tag.clone()];

    let plan = match build {
        ComposeBuild::Simple(ctx) => {
            let trimmed = ctx.trim();
            if trimmed.is_empty() {
                return Err(BuildPlanError::MissingContext(svc_name.to_string()));
            }
            let context = resolve_path(working_dir, trimmed);
            let dockerfile = Some(context.join(DEFAULT_DOCKERFILE));
            BuildPlan {
                project: project.to_string(),
                service: svc_name.to_string(),
                context,
                dockerfile,
                dockerfile_inline: None,
                args: HashMap::new(),
                target: None,
                cache_from: Vec::new(),
                network: None,
                shm_size: None,
                labels: HashMap::new(),
                tags,
                no_cache,
                pull,
            }
        }
        ComposeBuild::Full(cfg) => {
            let has_context = cfg.context.as_deref().is_some_and(|c| !c.trim().is_empty());
            let has_inline = cfg
                .dockerfile_inline
                .as_deref()
                .is_some_and(|s| !s.trim().is_empty());
            if !has_context && !has_inline {
                return Err(BuildPlanError::MissingContext(svc_name.to_string()));
            }
            let context = match cfg
                .context
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                Some(c) => resolve_path(working_dir, c),
                None => working_dir.to_path_buf(),
            };
            // Long form: `dockerfile:` is a relative path inside the context
            // (per Compose spec). When `dockerfile_inline:` is present we
            // skip the on-disk Dockerfile entirely.
            let (dockerfile, dockerfile_inline) = if has_inline {
                (None, cfg.dockerfile_inline.clone())
            } else {
                let df_name = cfg.dockerfile.as_deref().unwrap_or(DEFAULT_DOCKERFILE);
                (Some(context.join(df_name)), None)
            };
            // Append user-supplied tags so the primary `<project>-<service>:latest`
            // tag stays first (downstream code uses tags[0] as the image id).
            tags.extend(cfg.tags.iter().cloned());
            let shm_size = cfg
                .shm_size
                .as_ref()
                .map(super::types::StringOrNumber::as_string);
            BuildPlan {
                project: project.to_string(),
                service: svc_name.to_string(),
                context,
                dockerfile,
                dockerfile_inline,
                args: cfg.args.clone(),
                target: cfg.target.clone(),
                cache_from: cfg.cache_from.clone(),
                network: cfg.network.clone(),
                shm_size,
                labels: cfg.labels.clone(),
                tags,
                no_cache,
                pull,
            }
        }
    };

    Ok(plan)
}

/// Resolve `path` against `working_dir` if relative; otherwise return a
/// clean absolute copy. `Path::join` handles both cases — when `path` is
/// already absolute, the result equals `path`.
fn resolve_path(working_dir: &Path, path: &str) -> PathBuf {
    let p = Path::new(path);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        working_dir.join(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compose::parse_compose;

    fn compose_from(yaml: &str) -> ComposeFile {
        parse_compose(yaml).unwrap()
    }

    #[test]
    fn plan_skips_image_only_services() {
        let yaml = r"
services:
  api:
    image: nginx:1.25
  worker:
    build: ./worker
";
        let compose = compose_from(yaml);
        let working = PathBuf::from("/tmp/proj");
        let plans = plan_builds("p", &compose, &working, None, false, false).unwrap();
        assert_eq!(plans.len(), 1, "image-only services must be skipped");
        assert_eq!(plans[0].service, "worker");
        assert_eq!(plans[0].primary_tag(), "p-worker:latest");
    }

    #[test]
    fn plan_short_form_resolves_context_and_default_dockerfile() {
        let yaml = r"
services:
  app:
    build: ./app
";
        let compose = compose_from(yaml);
        let working = PathBuf::from("/tmp/proj");
        let plans = plan_builds("p", &compose, &working, None, false, false).unwrap();
        let plan = &plans[0];
        assert_eq!(plan.context, PathBuf::from("/tmp/proj/app"));
        assert_eq!(
            plan.dockerfile,
            Some(PathBuf::from("/tmp/proj/app/Dockerfile")),
        );
        assert!(plan.dockerfile_inline.is_none());
        assert_eq!(plan.tags, vec!["p-app:latest".to_string()]);
        assert!(!plan.no_cache);
    }

    #[test]
    fn plan_long_form_round_trips_every_field() {
        let yaml = r#"
services:
  worker:
    build:
      context: ./worker
      dockerfile: Dockerfile.prod
      args:
        VERSION: "1.2.3"
      target: runtime
      cache_from:
        - "ghcr.io/foo/worker:cache"
      network: host
      shm_size: "256m"
      labels:
        tier: backend
      tags:
        - "ghcr.io/foo/worker:v1"
"#;
        let compose = compose_from(yaml);
        let working = PathBuf::from("/tmp/proj");
        let plans = plan_builds("p", &compose, &working, None, true, true).unwrap();
        let plan = &plans[0];
        assert_eq!(plan.context, PathBuf::from("/tmp/proj/worker"));
        assert_eq!(
            plan.dockerfile,
            Some(PathBuf::from("/tmp/proj/worker/Dockerfile.prod")),
        );
        assert_eq!(plan.args.get("VERSION").map(String::as_str), Some("1.2.3"),);
        assert_eq!(plan.target.as_deref(), Some("runtime"));
        assert_eq!(
            plan.cache_from,
            vec!["ghcr.io/foo/worker:cache".to_string()]
        );
        assert_eq!(plan.network.as_deref(), Some("host"));
        assert_eq!(plan.shm_size.as_deref(), Some("256m"));
        assert_eq!(plan.labels.get("tier").map(String::as_str), Some("backend"));
        // Primary tag stays first; user tags follow.
        assert_eq!(plan.tags[0], "p-worker:latest");
        assert!(plan.tags.contains(&"ghcr.io/foo/worker:v1".to_string()));
        assert!(plan.no_cache);
        assert!(plan.pull);
    }

    #[test]
    fn plan_dockerfile_inline_skips_on_disk_path() {
        let yaml = "
services:
  inl:
    build:
      dockerfile_inline: |
        FROM alpine
        RUN echo hi
";
        let compose = compose_from(yaml);
        let working = PathBuf::from("/tmp/proj");
        let plans = plan_builds("p", &compose, &working, None, false, false).unwrap();
        let plan = &plans[0];
        assert!(plan.dockerfile.is_none());
        let inline = plan
            .dockerfile_inline
            .as_deref()
            .expect("inline body present");
        assert!(inline.contains("FROM alpine"));
        // Falls back to the working dir when no context is supplied.
        assert_eq!(plan.context, PathBuf::from("/tmp/proj"));
    }

    #[test]
    fn plan_missing_context_errors() {
        let yaml = "
services:
  bad:
    build:
      args:
        FOO: bar
";
        let compose = compose_from(yaml);
        let working = PathBuf::from("/tmp/proj");
        let err = plan_builds("p", &compose, &working, None, false, false)
            .expect_err("missing context+inline must error");
        assert!(matches!(err, BuildPlanError::MissingContext(name) if name == "bad"));
    }

    #[test]
    fn plan_unknown_service_filter_errors() {
        let yaml = "
services:
  app:
    build: .
";
        let compose = compose_from(yaml);
        let working = PathBuf::from("/tmp/proj");
        let filter = vec!["nope".to_string()];
        let err = plan_builds("p", &compose, &working, Some(&filter), false, false)
            .expect_err("unknown service in filter must error");
        assert!(matches!(err, BuildPlanError::UnknownService(name) if name == "nope"));
    }

    #[test]
    fn plan_filter_only_returns_matching_services() {
        let yaml = "
services:
  alpha:
    build: ./a
  beta:
    build: ./b
  gamma:
    build: ./c
";
        let compose = compose_from(yaml);
        let working = PathBuf::from("/tmp/proj");
        let filter = vec!["beta".to_string(), "alpha".to_string()];
        let plans = plan_builds("p", &compose, &working, Some(&filter), false, false).unwrap();
        let services: Vec<&str> = plans.iter().map(|p| p.service.as_str()).collect();
        // Output is sorted for determinism, regardless of filter order.
        assert_eq!(services, vec!["alpha", "beta"]);
    }

    #[test]
    fn plan_absolute_context_kept_as_is() {
        let yaml = "
services:
  abs:
    build: /opt/proj/abs-ctx
";
        let compose = compose_from(yaml);
        let working = PathBuf::from("/tmp/proj");
        let plans = plan_builds("p", &compose, &working, None, false, false).unwrap();
        assert_eq!(plans[0].context, PathBuf::from("/opt/proj/abs-ctx"));
    }
}
