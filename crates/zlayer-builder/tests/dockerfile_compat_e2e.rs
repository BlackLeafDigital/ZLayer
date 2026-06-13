#![cfg(target_os = "linux")]
//! Dockerfile-compatibility end-to-end tests.
//!
//! Builds the fixture Dockerfiles under `tests/fixtures/dockerfiles/`
//! through the **`zlayer-buildd` sidecar backend** — the production-preferred
//! Linux path (`detect_backend` only falls back to the buildah CLI shellout
//! when the sidecar binary is missing) — and verifies the RUN semantics by
//! reading marker files out of the built images. One additional smoke test
//! runs through the CLI-shellout fallback, since that path ships too.
//!
//! The fixtures cover the constructs that real-world Dockerfiles lean on:
//!
//! * `arg-conditional` — multi-stage + ARG-driven `RUN if/else` + staged
//!   artifact copy (the shape that broke the zbrain worker builds).
//! * `cache-mount` — `RUN --mount=type=cache,...,sharing=locked`.
//! * `multi-stage-args` — pre-FROM ARG in `FROM` lines, stage-local ARG
//!   scoping, `COPY --from`.
//! * `metadata` — ENV expansion in RUN, WORKDIR, LABEL, EXPOSE,
//!   HEALTHCHECK, ENTRYPOINT+CMD.
//!
//! # Running
//!
//! Requires buildah, the `zlayer-buildd` Go sidecar built
//! (`cd bin/zlayer-buildd && make build`, or `ZLAYER_BUILDD_BIN` set), and
//! network access (pulls the GHCR-mirrored alpine test image). The tests
//! share containers-storage, so run single-threaded:
//!
//! ```bash
//! cargo test --package zlayer-builder --test dockerfile_compat_e2e -- \
//!   --ignored --nocapture --test-threads=1
//! ```

use std::path::{Path, PathBuf};
use std::process::Command;

use zlayer_builder::ImageBuilder;
use zlayer_types::builder::BuilderBackendKind;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/dockerfiles")
        .join(name)
}

fn unique_tag(name: &str) -> String {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    format!("zlayer-compat-e2e/{name}:{ts}")
}

/// Locate the `zlayer-buildd` sidecar binary and export `ZLAYER_BUILDD_BIN`
/// so the backend's discovery resolves the same path. Mirrors
/// `buildah_sidecar_e2e.rs`, but PANICS instead of skipping: this suite's
/// whole point is the sidecar path, and a silent skip would read as green
/// coverage that never ran.
fn require_buildd() {
    if let Ok(p) = std::env::var("ZLAYER_BUILDD_BIN") {
        assert!(
            Path::new(&p).is_file(),
            "ZLAYER_BUILDD_BIN={p} is not a file"
        );
        return;
    }
    // Repo root is two levels up from CARGO_MANIFEST_DIR
    // (crates/zlayer-builder -> crates -> repo-root).
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("CARGO_MANIFEST_DIR has at least 2 parents");
    let candidate = repo_root.join("bin/zlayer-buildd/zlayer-buildd");
    assert!(
        candidate.is_file(),
        "zlayer-buildd sidecar binary not found at {} — build it with \
         `cd bin/zlayer-buildd && make build` or set ZLAYER_BUILDD_BIN",
        candidate.display()
    );
    // SAFETY: setting an env var is unsafe in newer Rust because it can race
    // with other threads. These tests are `#[ignore]`-gated and run with
    // `--test-threads=1`, and no async work has started yet.
    #[allow(unsafe_code)]
    unsafe {
        std::env::set_var("ZLAYER_BUILDD_BIN", &candidate);
    }
}

/// Build one fixture context through the given backend.
async fn build_fixture(
    name: &str,
    tag: &str,
    build_args: &[(&str, &str)],
    backend: BuilderBackendKind,
) {
    if backend == BuilderBackendKind::BuildahSidecar {
        require_buildd();
    }
    let ctx = fixture(name);
    let mut builder = ImageBuilder::new(&ctx)
        .await
        .unwrap_or_else(|e| panic!("ImageBuilder::new({name}): {e}"))
        .dockerfile(ctx.join("Dockerfile"))
        .tag(tag.to_string())
        .with_backend_override(Some(backend))
        // `--net=host` on every RUN: the fixtures don't need an isolated
        // netns, and this sidesteps the buildah 1.44 ↔ netavark 1.17.2
        // network-options skew (docs/known-issues-linux-build.md) that
        // otherwise kills the first RUN on affected hosts.
        .with_host_network(true);
    for (k, v) in build_args {
        builder = builder.build_arg(*k, *v);
    }
    match builder.build().await {
        Ok(r) => {
            eprintln!("built {name} -> {tag} via {} ({r:?})", backend.as_str());
        }
        Err(e) => panic!(
            "building fixture {name} via {} failed: {e:#}",
            backend.as_str()
        ),
    }
}

/// Global buildah flags selecting the storage the given backend commits
/// into. The sidecar uses its own rootless-safe per-user store at
/// `${ZLAYER_DATA_DIR}/buildd/storage/{graph,run}` with the vfs driver
/// (`backend/buildah_sidecar/lifecycle.rs::resolve_storage_spec`); the CLI
/// shellout uses the host's default containers-storage.
fn storage_flags(backend: BuilderBackendKind) -> Vec<String> {
    if backend != BuilderBackendKind::BuildahSidecar {
        return Vec::new();
    }
    let storage = zlayer_paths::ZLayerDirs::system_default()
        .buildd()
        .join("storage");
    vec![
        "--storage-driver".into(),
        "vfs".into(),
        "--root".into(),
        storage.join("graph").to_string_lossy().into_owned(),
        "--runroot".into(),
        storage.join("run").to_string_lossy().into_owned(),
    ]
}

/// `buildah` invocation against the given backend's storage.
fn buildah_cmd(storage: &[String]) -> Command {
    let mut cmd = Command::new("buildah");
    cmd.args(storage);
    cmd
}

/// Read a file out of a built image via `buildah from` + `buildah run`.
///
/// This is pure VERIFICATION, not the path under test: the buildah CLI
/// reads back the containers-storage the backend committed into (selected
/// via `storage_flags`). chroot isolation keeps the helper working as
/// root (CI) and rootless alike.
fn read_file_in_image(storage: &[String], tag: &str, path: &str) -> String {
    let from = buildah_cmd(storage)
        .args(["from", "--pull=never", tag])
        .output()
        .expect("buildah from");
    assert!(
        from.status.success(),
        "buildah from {tag}: {}",
        String::from_utf8_lossy(&from.stderr)
    );
    let cid = String::from_utf8_lossy(&from.stdout).trim().to_string();

    let run = buildah_cmd(storage)
        .env("BUILDAH_ISOLATION", "chroot")
        .args(["run", &cid, "--", "cat", path])
        .output()
        .expect("buildah run");
    let content = String::from_utf8_lossy(&run.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&run.stderr).into_owned();

    let _ = buildah_cmd(storage).args(["rm", &cid]).output();

    assert!(
        run.status.success(),
        "reading {path} from {tag} failed: {stderr}"
    );
    content
}

/// Remove a test image (best effort).
fn rmi(storage: &[String], tag: &str) {
    let _ = buildah_cmd(storage).args(["rmi", "-f", tag]).output();
}

#[tokio::test]
#[ignore = "live e2e: needs buildah + zlayer-buildd + network (run with --ignored --test-threads=1)"]
async fn arg_conditional_default_branch() {
    let tag = unique_tag("arg-conditional-default");
    build_fixture(
        "arg-conditional",
        &tag,
        &[("APP_NAME", "zworker")],
        BuilderBackendKind::BuildahSidecar,
    )
    .await;
    let storage = storage_flags(BuilderBackendKind::BuildahSidecar);
    let artifact = read_file_in_image(&storage, &tag, "/usr/local/bin/zapp-artifact");
    assert_eq!(artifact.trim(), "built zworker default");
    rmi(&storage, &tag);
}

#[tokio::test]
#[ignore = "live e2e: needs buildah + zlayer-buildd + network (run with --ignored --test-threads=1)"]
async fn arg_conditional_feature_branch() {
    let tag = unique_tag("arg-conditional-features");
    build_fixture(
        "arg-conditional",
        &tag,
        &[("APP_NAME", "zworker-tts"), ("APP_FEATURES", "tts,gpu")],
        BuilderBackendKind::BuildahSidecar,
    )
    .await;
    let storage = storage_flags(BuilderBackendKind::BuildahSidecar);
    let artifact = read_file_in_image(&storage, &tag, "/usr/local/bin/zapp-artifact");
    assert_eq!(artifact.trim(), "built zworker-tts with features tts,gpu");
    rmi(&storage, &tag);
}

/// The CLI-shellout fallback (hosts without the sidecar binary) must handle
/// the same ARG-driven conditional shape.
#[tokio::test]
#[ignore = "live e2e: needs buildah + network (run with --ignored --test-threads=1)"]
async fn arg_conditional_via_cli_fallback() {
    let tag = unique_tag("arg-conditional-cli");
    build_fixture(
        "arg-conditional",
        &tag,
        &[("APP_NAME", "zworker"), ("APP_FEATURES", "cli-path")],
        BuilderBackendKind::BuildahCli,
    )
    .await;
    let storage = storage_flags(BuilderBackendKind::BuildahCli);
    let artifact = read_file_in_image(&storage, &tag, "/usr/local/bin/zapp-artifact");
    assert_eq!(artifact.trim(), "built zworker with features cli-path");
    rmi(&storage, &tag);
}

#[tokio::test]
#[ignore = "live e2e: needs buildah + zlayer-buildd + network (run with --ignored --test-threads=1)"]
async fn cache_mount_run() {
    let tag = unique_tag("cache-mount");
    build_fixture("cache-mount", &tag, &[], BuilderBackendKind::BuildahSidecar).await;
    let storage = storage_flags(BuilderBackendKind::BuildahSidecar);
    // The marker written FROM the cache mount must be committed...
    let runs = read_file_in_image(&storage, &tag, "/cache-runs.txt");
    let count: u64 = runs
        .trim()
        .parse()
        .unwrap_or_else(|e| panic!("cache-runs.txt not a number ({e}): {runs:?}"));
    assert!(count >= 1, "cache mount RUN left no stamp count: {runs:?}");

    // ...while the cache directory itself must NOT be part of the image.
    let from = buildah_cmd(&storage)
        .args(["from", "--pull=never", &tag])
        .output()
        .expect("buildah from");
    assert!(from.status.success());
    let cid = String::from_utf8_lossy(&from.stdout).trim().to_string();
    let probe = buildah_cmd(&storage)
        .env("BUILDAH_ISOLATION", "chroot")
        .args([
            "run",
            &cid,
            "--",
            "sh",
            "-c",
            "test ! -e /var/cache/compat-e2e/stamp",
        ])
        .output()
        .expect("buildah run");
    let _ = buildah_cmd(&storage).args(["rm", &cid]).output();
    assert!(
        probe.status.success(),
        "cache mount contents leaked into the committed image"
    );
    rmi(&storage, &tag);
}

#[tokio::test]
#[ignore = "live e2e: needs buildah + zlayer-buildd + network (run with --ignored --test-threads=1)"]
async fn multi_stage_arg_scoping() {
    let tag = unique_tag("multi-stage-args");
    build_fixture(
        "multi-stage-args",
        &tag,
        &[("MESSAGE", "hello-from-producer")],
        BuilderBackendKind::BuildahSidecar,
    )
    .await;
    let storage = storage_flags(BuilderBackendKind::BuildahSidecar);
    let final_txt = read_file_in_image(&storage, &tag, "/final.txt");
    assert_eq!(final_txt.trim(), "hello-from-producer");
    rmi(&storage, &tag);
}

#[tokio::test]
#[ignore = "live e2e: needs buildah + zlayer-buildd + network (run with --ignored --test-threads=1)"]
async fn metadata_instructions() {
    let tag = unique_tag("metadata");
    build_fixture("metadata", &tag, &[], BuilderBackendKind::BuildahSidecar).await;
    let storage = storage_flags(BuilderBackendKind::BuildahSidecar);

    // ENV must have expanded inside RUN.
    let env_txt = read_file_in_image(&storage, &tag, "/app/env.txt");
    assert_eq!(env_txt.trim(), "production:8080");

    // WORKDIR must have been the cwd of the RUN.
    let workdir_txt = read_file_in_image(&storage, &tag, "/app/workdir.txt");
    assert_eq!(workdir_txt.trim(), "/app/data");

    // Image config must carry the metadata instructions.
    let inspect = buildah_cmd(&storage)
        .args(["inspect", "--type", "image", &tag])
        .output()
        .expect("buildah inspect");
    assert!(inspect.status.success());
    let doc: serde_json::Value = serde_json::from_slice(&inspect.stdout).expect("inspect json");
    let config = &doc["OCIv1"]["config"];
    let labels = &config["Labels"];
    assert_eq!(
        labels["com.zlayer.test"], "dockerfile-compat",
        "LABEL missing from image config: {labels}"
    );
    let exposed = config["ExposedPorts"]
        .as_object()
        .map(|m| m.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    assert!(
        exposed.iter().any(|p| p.starts_with("8080")),
        "EXPOSE 8080 missing from image config: {exposed:?}"
    );
    let entrypoint = config["Entrypoint"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
        .unwrap_or_default();
    assert_eq!(entrypoint, ["/bin/sh", "-c"], "ENTRYPOINT mismatch");
    rmi(&storage, &tag);
}
