//! `build_image` implementation for the buildah-sidecar backend.
//!
//! Translates a [`BuildOptions`] + parsed [`Dockerfile`] into a
//! [`proto::BuildRequest`], opens the server-streamed `Build` RPC against
//! the sidecar gRPC service, and translates each [`proto::BuildEvent`] into
//! a [`BuildEvent`] for the TUI. The final [`BuiltImage`] is constructed
//! from the terminal `BuildFinished` event.

use std::path::Path;
use std::sync::mpsc::Sender;

use tokio_stream::StreamExt;
use tonic::Request;

use crate::backend::buildah_sidecar::proto;
use crate::builder::{BuildOptions, BuiltImage, PullBaseMode};
use crate::dockerfile::Dockerfile;
use crate::error::{BuildError, Result};
use crate::tui::BuildEvent;

use super::BuildahSidecarBackend;

impl BuildahSidecarBackend {
    /// Server-stream the build through the sidecar and assemble a
    /// [`BuiltImage`]. Wired up from the `BuildBackend::build_image` trait
    /// method on [`BuildahSidecarBackend`] in `mod.rs`.
    pub(super) async fn build_image_impl(
        &self,
        context: &Path,
        dockerfile: &Dockerfile,
        options: &BuildOptions,
        event_tx: Option<Sender<BuildEvent>>,
    ) -> Result<BuiltImage> {
        let started_at = std::time::Instant::now();
        let live = self.lifecycle.ensure().await?;
        let mut client = live.client();

        let request = build_request_from(context, dockerfile, options);
        let stream = client
            .build(Request::new(request))
            .await
            .map_err(|s| grpc_err(&s))?
            .into_inner();

        consume_build_stream(stream, event_tx, dockerfile, options, started_at).await
    }
}

/// Translate a [`BuildOptions`] + parsed [`Dockerfile`] into a
/// [`proto::BuildRequest`]. Every field on the proto schema is set
/// explicitly so the wire payload is deterministic.
fn build_request_from(
    context: &Path,
    _dockerfile: &Dockerfile,
    options: &BuildOptions,
) -> proto::BuildRequest {
    let dockerfile_path = options.dockerfile.clone().map_or_else(
        || context.join("Dockerfile").to_string_lossy().into_owned(),
        |p| p.to_string_lossy().into_owned(),
    );

    let platforms = options
        .platform
        .as_deref()
        .map(|s| {
            s.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let cache_from = options.cache_from.clone().unwrap_or_default();
    let cache_to = options.cache_to.clone().unwrap_or_default();

    // Merge `pipeline_vars` into `build_args` so `${VAR}` references in the
    // request (e.g. LTSC line, version) reach the sidecar as ARG bindings.
    let mut build_args = std::collections::HashMap::<String, String>::new();
    for (k, v) in &options.build_args {
        build_args.insert(k.clone(), v.clone());
    }
    for (k, v) in &options.pipeline_vars {
        build_args.insert(k.clone(), v.clone());
    }

    let pull_policy = pull_policy_str(options.pull).to_string();
    let format = options.format.clone().unwrap_or_default();
    let target_stage = options.target.clone().unwrap_or_default();

    proto::BuildRequest {
        request_id: String::new(),
        context_dir: context.to_string_lossy().into_owned(),
        dockerfile_paths: vec![dockerfile_path],
        tags: options.tags.clone(),
        platforms,
        build_args,
        secrets: Vec::new(),
        ssh: Vec::new(),
        target_stage,
        host_network: options.host_network,
        cache_from,
        cache_to,
        no_cache: options.no_cache,
        squash: options.squash,
        layers: options.layers,
        format,
        pull_policy,
        labels: Vec::new(),
        annotations: Vec::new(),
        add_hosts: Vec::new(),
        envs: Vec::new(),
        shm_size: String::new(),
        ulimits: Vec::new(),
        volumes: Vec::new(),
        source_date_epoch: 0,
        rewrite_timestamp: false,
    }
}

/// Map [`PullBaseMode`] to the string the sidecar / buildah expect for the
/// `--pull` flag.
///
/// `PullBaseMode::Newer` is documented as "only pull if the registry has a
/// newer version", which is exactly `buildah --pull=ifnewer`.
fn pull_policy_str(mode: PullBaseMode) -> &'static str {
    match mode {
        PullBaseMode::Never => "never",
        PullBaseMode::Always => "always",
        PullBaseMode::Newer => "ifnewer",
    }
}

/// Consume the streamed `BuildEvent`s from the sidecar, fan out
/// [`BuildEvent`]s to the optional TUI sender, and assemble the final
/// [`BuiltImage`] from the terminal `BuildFinished` event.
async fn consume_build_stream(
    mut stream: tonic::Streaming<proto::BuildEvent>,
    event_tx: Option<Sender<BuildEvent>>,
    dockerfile: &Dockerfile,
    options: &BuildOptions,
    started_at: std::time::Instant,
) -> Result<BuiltImage> {
    let total_stages = dockerfile.stages.len();
    let total_instructions: usize = dockerfile.stages.iter().map(|s| s.instructions.len()).sum();

    if let Some(tx) = &event_tx {
        let _ = tx.send(BuildEvent::BuildStarted {
            total_stages,
            total_instructions,
        });
    }

    let mut final_image_id: Option<String> = None;
    let mut final_manifest_ref: Option<String> = None;
    let mut final_error: Option<String> = None;

    while let Some(message) = stream.next().await {
        let event = message.map_err(|s| grpc_err(&s))?;
        let Some(ev) = event.event else {
            continue;
        };
        dispatch_event(
            ev,
            event_tx.as_ref(),
            &mut final_image_id,
            &mut final_manifest_ref,
            &mut final_error,
        );
    }

    if let Some(err) = final_error {
        if let Some(tx) = &event_tx {
            let _ = tx.send(BuildEvent::BuildFailed { error: err.clone() });
        }
        return Err(BuildError::BuildahExecution {
            command: "buildah-sidecar build".to_string(),
            exit_code: 1,
            stderr: err,
        });
    }

    let image_id = final_image_id.ok_or_else(|| BuildError::BuildahExecution {
        command: "buildah-sidecar build".to_string(),
        exit_code: 1,
        stderr: "sidecar stream ended without Finished or Error event".to_string(),
    })?;

    if let Some(tx) = &event_tx {
        let _ = tx.send(BuildEvent::BuildComplete {
            image_id: image_id.clone(),
        });
    }

    Ok(built_image_from(
        image_id,
        final_manifest_ref.as_deref(),
        options,
        started_at,
    ))
}

/// Dispatch a single sidecar event: forward the corresponding [`BuildEvent`]
/// to the TUI (if any) and update the terminal-state slots for the caller.
fn dispatch_event(
    ev: proto::build_event::Event,
    event_tx: Option<&Sender<BuildEvent>>,
    final_image_id: &mut Option<String>,
    final_manifest_ref: &mut Option<String>,
    final_error: &mut Option<String>,
) {
    match ev {
        proto::build_event::Event::StageStarted(s) => {
            if let Some(tx) = event_tx {
                let _ = tx.send(BuildEvent::StageStarted {
                    index: s.index as usize,
                    name: if s.name.is_empty() {
                        None
                    } else {
                        Some(s.name)
                    },
                    base_image: s.base_image,
                });
            }
        }
        proto::build_event::Event::StageFinished(s) => {
            if let Some(tx) = event_tx {
                let _ = tx.send(BuildEvent::StageComplete {
                    index: s.index as usize,
                });
            }
        }
        proto::build_event::Event::InstructionStarted(i) => {
            if let Some(tx) = event_tx {
                let _ = tx.send(BuildEvent::InstructionStarted {
                    stage: i.stage as usize,
                    index: i.index as usize,
                    instruction: i.instruction,
                });
            }
        }
        proto::build_event::Event::InstructionFinished(i) => {
            if let Some(tx) = event_tx {
                let _ = tx.send(BuildEvent::InstructionComplete {
                    stage: i.stage as usize,
                    index: i.index as usize,
                    cached: i.cached,
                });
            }
        }
        proto::build_event::Event::Log(line) => {
            if let Some(tx) = event_tx {
                let _ = tx.send(BuildEvent::Output {
                    line: line.line,
                    is_stderr: line.is_stderr,
                });
            }
        }
        proto::build_event::Event::Warning(w) => {
            if let Some(tx) = event_tx {
                let _ = tx.send(BuildEvent::Output {
                    line: format!("warning: {}", w.message),
                    is_stderr: true,
                });
            }
        }
        proto::build_event::Event::Finished(f) => {
            *final_image_id = Some(f.image_id);
            *final_manifest_ref = if f.manifest_ref.is_empty() {
                None
            } else {
                Some(f.manifest_ref)
            };
        }
        proto::build_event::Event::Error(e) => {
            *final_error = Some(if e.kind.is_empty() {
                e.message
            } else {
                format!("{}: {}", e.kind, e.message)
            });
        }
    }
}

/// Construct a [`BuiltImage`] from the terminal sidecar event.
///
/// The sidecar's `BuildFinished` carries only the image ID and (optionally)
/// the canonical `name@sha256:...` reference. `layer_count` and `size` are
/// not reported by the current schema; an `Inspect` RPC would be needed to
/// surface them. Those fields are zeroed for now — callers that need them
/// can follow up via `crate::backend::BuildahSidecarBackend::lifecycle`
/// and the `Inspect` RPC.
fn built_image_from(
    image_id: String,
    manifest_ref: Option<&str>,
    options: &BuildOptions,
    started_at: std::time::Instant,
) -> BuiltImage {
    let is_manifest =
        manifest_ref.is_some() && options.platform.as_deref().is_some_and(|s| s.contains(','));

    BuiltImage {
        image_id,
        tags: options.tags.clone(),
        layer_count: 0,
        size: 0,
        build_time_ms: u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX),
        is_manifest,
    }
}

/// Convert a tonic `Status` into a typed `BuildError` so the surrounding
/// code can match on the variant rather than parsing strings.
fn grpc_err(status: &tonic::Status) -> BuildError {
    BuildError::BuildahExecution {
        command: format!("buildah-sidecar rpc ({:?})", status.code()),
        exit_code: 1,
        stderr: status.message().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::PullBaseMode;
    use std::path::Path;

    fn empty_dockerfile() -> Dockerfile {
        // A trivial single-stage Dockerfile so `parse` succeeds and we get
        // a `Dockerfile` value cheaply for tests that don't care about
        // its contents.
        Dockerfile::parse("FROM scratch\n").expect("trivial Dockerfile must parse")
    }

    #[test]
    fn build_request_from_minimal_options() {
        let context = Path::new("/tmp/ctx");
        let df = empty_dockerfile();
        let options = BuildOptions {
            tags: vec!["test/img:latest".into()],
            ..BuildOptions::default()
        };

        let req = build_request_from(context, &df, &options);
        assert_eq!(req.context_dir, "/tmp/ctx");
        assert_eq!(req.tags, vec!["test/img:latest".to_string()]);
        assert!(req.platforms.is_empty());
        assert_eq!(
            req.dockerfile_paths,
            vec!["/tmp/ctx/Dockerfile".to_string()]
        );
        assert_eq!(req.pull_policy, "ifnewer"); // PullBaseMode default = Newer
        assert!(!req.no_cache);
        assert!(!req.squash);
        assert!(req.layers); // default in BuildOptions::default()
        assert_eq!(req.target_stage, "");
        assert_eq!(req.format, "");
        assert_eq!(req.cache_from, "");
        assert_eq!(req.cache_to, "");
    }

    #[test]
    fn build_request_explicit_dockerfile_overrides_context_join() {
        let context = Path::new("/tmp/ctx");
        let df = empty_dockerfile();
        let options = BuildOptions {
            dockerfile: Some(Path::new("/custom/Dockerfile.web").into()),
            ..BuildOptions::default()
        };
        let req = build_request_from(context, &df, &options);
        assert_eq!(
            req.dockerfile_paths,
            vec!["/custom/Dockerfile.web".to_string()]
        );
    }

    #[test]
    fn build_request_splits_multi_platform_string() {
        let context = Path::new("/tmp/ctx");
        let df = empty_dockerfile();
        let options = BuildOptions {
            platform: Some(" linux/amd64 , linux/arm64 ".to_string()),
            ..BuildOptions::default()
        };
        let req = build_request_from(context, &df, &options);
        assert_eq!(
            req.platforms,
            vec!["linux/amd64".to_string(), "linux/arm64".to_string()]
        );
    }

    #[test]
    fn build_request_merges_pipeline_vars_into_build_args() {
        let context = Path::new("/tmp/ctx");
        let df = empty_dockerfile();
        let mut build_args = std::collections::HashMap::new();
        build_args.insert("FOO".to_string(), "1".to_string());
        let mut pipeline_vars = std::collections::HashMap::new();
        pipeline_vars.insert("LTSC".to_string(), "ltsc2025".to_string());
        let options = BuildOptions {
            build_args,
            pipeline_vars,
            ..BuildOptions::default()
        };
        let req = build_request_from(context, &df, &options);
        assert_eq!(req.build_args.get("FOO"), Some(&"1".to_string()));
        assert_eq!(req.build_args.get("LTSC"), Some(&"ltsc2025".to_string()));
    }

    #[test]
    fn pull_policy_translations() {
        assert_eq!(pull_policy_str(PullBaseMode::Never), "never");
        assert_eq!(pull_policy_str(PullBaseMode::Always), "always");
        assert_eq!(pull_policy_str(PullBaseMode::Newer), "ifnewer");
    }

    #[test]
    fn built_image_carries_tags_and_id() {
        let started = std::time::Instant::now();
        let options = BuildOptions {
            tags: vec!["a:b".into(), "c:d".into()],
            ..BuildOptions::default()
        };
        let img = built_image_from("sha256:abc".to_string(), None, &options, started);
        assert_eq!(img.image_id, "sha256:abc");
        assert_eq!(img.tags, vec!["a:b".to_string(), "c:d".to_string()]);
        assert!(!img.is_manifest);
    }

    #[test]
    fn built_image_flags_manifest_for_multi_arch_with_manifest_ref() {
        let started = std::time::Instant::now();
        let options = BuildOptions {
            tags: vec!["a:b".into()],
            platform: Some("linux/amd64,linux/arm64".to_string()),
            ..BuildOptions::default()
        };
        let img = built_image_from(
            "sha256:abc".to_string(),
            Some("registry/img@sha256:abc"),
            &options,
            started,
        );
        assert!(img.is_manifest);
    }

    #[test]
    fn grpc_err_carries_status_message() {
        let status = tonic::Status::internal("boom");
        let err = grpc_err(&status);
        match err {
            BuildError::BuildahExecution {
                command, stderr, ..
            } => {
                assert!(command.contains("Internal"));
                assert_eq!(stderr, "boom");
            }
            other => panic!("unexpected variant: {other:?}"),
        }
    }
}
