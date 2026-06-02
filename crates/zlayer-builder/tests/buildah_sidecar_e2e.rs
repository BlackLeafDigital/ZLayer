//! End-to-end tests for the buildah-sidecar backend.
//!
//! All tests in this file are `#[ignore]`-gated. They require:
//!   - `buildah` installed on the host;
//!   - the `zlayer-buildd` Go binary built (`cd bin/zlayer-buildd && make build`),
//!     OR `ZLAYER_BUILDD_BIN` set to an absolute path.
//!
//! Run manually:
//! ```bash
//! cargo test --test buildah_sidecar_e2e -- --ignored --nocapture
//! ```
//!
//! Or, with explicit sidecar binary discovery:
//! ```bash
//! cd bin/zlayer-buildd && make build && cd ../..
//! ZLAYER_BUILDD_BIN=$(pwd)/bin/zlayer-buildd/zlayer-buildd \
//!     cargo test --test buildah_sidecar_e2e -- --ignored --nocapture
//! ```

#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use zlayer_builder::backend::buildah_sidecar::BuildahSidecarBackend;
use zlayer_builder::backend::{BuildBackend, BuildahBackend};
use zlayer_builder::builder::BuildOptions;
use zlayer_builder::dockerfile::Dockerfile;
use zlayer_builder::tui::BuildEvent;
use zlayer_types::builder::SidecarConfig;

/// Returns `true` if `buildah --version` succeeds; otherwise prints a skip
/// reason and returns `false`.
fn require_buildah_or_skip() -> bool {
    match std::process::Command::new("buildah")
        .arg("--version")
        .output()
    {
        Ok(o) if o.status.success() => true,
        _ => {
            eprintln!("skipping: buildah not available on PATH");
            false
        }
    }
}

/// Locate the `zlayer-buildd` sidecar binary. Honors `ZLAYER_BUILDD_BIN` and
/// falls back to `bin/zlayer-buildd/zlayer-buildd` in the repo root. If the
/// binary is not found, prints a skip reason and returns `None`.
///
/// When the repo-root fallback succeeds, exports `ZLAYER_BUILDD_BIN` so the
/// sidecar's own discovery logic finds the same path.
fn require_buildd_or_skip() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("ZLAYER_BUILDD_BIN") {
        let path = PathBuf::from(&p);
        if path.is_file() {
            return Some(path);
        }
        eprintln!(
            "skipping: ZLAYER_BUILDD_BIN={} is not a file",
            path.display()
        );
        return None;
    }

    // Repo root is two levels up from CARGO_MANIFEST_DIR
    // (crates/zlayer-builder -> crates -> repo-root).
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("CARGO_MANIFEST_DIR has at least 2 parents");
    let candidate = repo_root.join("bin/zlayer-buildd/zlayer-buildd");
    if candidate.is_file() {
        // SAFETY: setting an env var is unsafe in newer Rust because it can
        // race with other threads. These tests are gated by `#[ignore]` and
        // run sequentially via `--test-threads=1` in CI, and at this point
        // no async work has started, so the call is safe in practice.
        #[allow(unsafe_code)]
        unsafe {
            std::env::set_var("ZLAYER_BUILDD_BIN", &candidate);
        }
        return Some(candidate);
    }
    eprintln!(
        "skipping: zlayer-buildd binary not found at {} (build with `cd bin/zlayer-buildd && make build`)",
        candidate.display()
    );
    None
}

fn write_dockerfile(dir: &Path, contents: &str) -> PathBuf {
    let path = dir.join("Dockerfile");
    std::fs::write(&path, contents).expect("write Dockerfile");
    path
}

/// Configure a rootless-friendly environment for the CLI `BuildahBackend`
/// half of the parity test:
///
/// 1. Write a custom `storage.conf` pointing at a per-test writable
///    storage tree (vfs driver, paths under the returned tempdir).
/// 2. Set `BUILDAH_ISOLATION=chroot` so the CLI's `buildah run` uses the
///    chroot path — same as the sidecar — instead of trying to spawn
///    `crun` / `runc`, which fails on hosts where `/run/user/<uid>/crun`
///    or kernel-rootless OCI runtimes are unreliable.
///
/// Returns a `TempDir` guard that, when dropped, cleans up the storage
/// tree. The env vars are intentionally NOT restored on drop — the test
/// is the only consumer in this process, and resetting after the test
/// adds complexity with no benefit.
fn with_rootless_cli_env() -> tempfile::TempDir {
    let tmp = tempfile::Builder::new()
        .prefix("zlayer-cli-storage-")
        .tempdir()
        .expect("storage tmpdir");
    let graph_root = tmp.path().join("graph");
    let run_root = tmp.path().join("run");
    std::fs::create_dir_all(&graph_root).expect("graph dir");
    std::fs::create_dir_all(&run_root).expect("run dir");

    let conf_path = tmp.path().join("storage.conf");
    let conf = format!(
        r#"[storage]
driver = "vfs"
graphroot = {graph:?}
runroot = {run:?}

[storage.options]
pull_options = {{ enable_partial_images = "false" }}
"#,
        graph = graph_root.to_string_lossy(),
        run = run_root.to_string_lossy(),
    );
    std::fs::write(&conf_path, conf).expect("write storage.conf");

    // SAFETY: this test file runs serially because every test mutates
    // process-wide env (ZLAYER_BUILDD_BIN, CONTAINERS_STORAGE_CONF,
    // BUILDAH_ISOLATION). The TempDir guard owns the storage lifetime.
    #[allow(unsafe_code)]
    unsafe {
        std::env::set_var("CONTAINERS_STORAGE_CONF", &conf_path);
        std::env::set_var("BUILDAH_ISOLATION", "chroot");
    }
    tmp
}

/// Construct a fresh sidecar backend with a dedicated TLS material directory.
///
/// The TLS dir is created via `tempfile` then `keep()`'d so it outlives the
/// `TempDir` guard — the sidecar's mTLS material must survive for the duration
/// of the build call. This intentionally leaks the directory (test-only).
fn make_backend() -> BuildahSidecarBackend {
    let tls_dir = tempfile::Builder::new()
        .prefix("zlayer-sidecar-tls-")
        .tempdir()
        .expect("create TLS tmpdir")
        .keep();
    BuildahSidecarBackend::new(SidecarConfig {
        addr: None,
        tls_dir: Some(tls_dir),
        idle_secs: 5,
        ..Default::default()
    })
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires buildah + zlayer-buildd binaries on PATH and network access"]
async fn sidecar_builds_minimal_dockerfile() {
    if !require_buildah_or_skip() {
        return;
    }
    if require_buildd_or_skip().is_none() {
        return;
    }

    let tmp = tempfile::tempdir().expect("ctx tmpdir");
    let dockerfile_path = write_dockerfile(
        tmp.path(),
        r#"FROM docker.io/library/busybox:latest
CMD ["echo","sidecar-e2e"]
"#,
    );
    let dockerfile = Dockerfile::from_file(&dockerfile_path).expect("parse Dockerfile");

    let backend = make_backend();
    let options = BuildOptions {
        tags: vec!["zlayer/test-sidecar-e2e:latest".to_string()],
        dockerfile: Some(dockerfile_path),
        ..BuildOptions::default()
    };

    let (tx, rx) = mpsc::channel::<BuildEvent>();
    let built = tokio::time::timeout(
        Duration::from_secs(180),
        backend.build_image(tmp.path(), &dockerfile, &options, Some(tx)),
    )
    .await
    .expect("build_image timed out after 180s")
    .expect("build_image must succeed");

    assert!(!built.image_id.is_empty(), "image_id must be non-empty");
    let id_lower = built.image_id.to_ascii_lowercase();
    let valid_shape = id_lower.starts_with("sha256:")
        || id_lower
            .strip_prefix("sha256:")
            .unwrap_or(&id_lower)
            .chars()
            .all(|c| c.is_ascii_hexdigit());
    assert!(valid_shape, "unexpected image_id shape: {}", built.image_id);

    // Drain events and assert we saw at least BuildStarted and BuildComplete.
    let events: Vec<_> = rx.try_iter().collect();
    let saw_started = events
        .iter()
        .any(|e| matches!(e, BuildEvent::BuildStarted { .. }));
    let saw_complete = events
        .iter()
        .any(|e| matches!(e, BuildEvent::BuildComplete { .. }));
    assert!(
        saw_started,
        "did not observe BuildStarted event (got {} events)",
        events.len()
    );
    assert!(
        saw_complete,
        "did not observe BuildComplete event (got {} events)",
        events.len()
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires buildah + zlayer-buildd binaries on PATH and network access"]
async fn sidecar_and_cli_produce_equivalent_image() {
    if !require_buildah_or_skip() {
        return;
    }
    if require_buildd_or_skip().is_none() {
        return;
    }

    let tmp = tempfile::tempdir().expect("ctx tmpdir");
    let dockerfile_path = write_dockerfile(
        tmp.path(),
        r#"FROM docker.io/library/busybox:latest
RUN sh -c 'echo "parity test" > /parity'
"#,
    );
    let dockerfile = Dockerfile::from_file(&dockerfile_path).expect("parse Dockerfile");

    // Sidecar build.
    let sidecar = make_backend();
    let sidecar_opts = BuildOptions {
        tags: vec!["zlayer/parity-sidecar:latest".to_string()],
        dockerfile: Some(dockerfile_path.clone()),
        ..BuildOptions::default()
    };
    let sidecar_image = tokio::time::timeout(
        Duration::from_secs(240),
        sidecar.build_image(tmp.path(), &dockerfile, &sidecar_opts, None),
    )
    .await
    .expect("sidecar build timed out")
    .expect("sidecar build failed");

    // CLI build — point containers/storage at a writable per-test tree
    // and pin `BUILDAH_ISOLATION=chroot` so the CLI uses the same
    // isolation backend as the sidecar (the parity test must compare
    // like-for-like).
    let _cli_env = with_rootless_cli_env();
    let cli = BuildahBackend::new().await.expect("init BuildahBackend");
    let cli_opts = BuildOptions {
        tags: vec!["zlayer/parity-cli:latest".to_string()],
        dockerfile: Some(dockerfile_path),
        ..BuildOptions::default()
    };
    let cli_image = tokio::time::timeout(
        Duration::from_secs(240),
        cli.build_image(tmp.path(), &dockerfile, &cli_opts, None),
    )
    .await
    .expect("CLI build timed out")
    .expect("CLI build failed");

    // We deliberately do NOT compare image_id digests: identical Dockerfiles
    // produce different IDs across runs because buildah stamps each layer
    // with the build timestamp. We assert "both produced a non-empty ID and
    // the tag set matches the request."
    assert!(
        !sidecar_image.image_id.is_empty(),
        "sidecar image_id must be non-empty"
    );
    assert!(
        !cli_image.image_id.is_empty(),
        "CLI image_id must be non-empty"
    );
    assert_eq!(
        sidecar_image.tags.len(),
        1,
        "sidecar tags: {:?}",
        sidecar_image.tags
    );
    assert_eq!(cli_image.tags.len(), 1, "CLI tags: {:?}", cli_image.tags);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires buildah + zlayer-buildd binaries on PATH and network access"]
async fn sidecar_propagates_build_failure() {
    if !require_buildah_or_skip() {
        return;
    }
    if require_buildd_or_skip().is_none() {
        return;
    }

    let tmp = tempfile::tempdir().expect("ctx tmpdir");
    let dockerfile_path = write_dockerfile(
        tmp.path(),
        r"FROM docker.io/library/busybox:latest
RUN false
",
    );
    let dockerfile = Dockerfile::from_file(&dockerfile_path).expect("parse Dockerfile");

    let backend = make_backend();
    let options = BuildOptions {
        tags: vec!["zlayer/test-sidecar-failure:latest".to_string()],
        dockerfile: Some(dockerfile_path),
        ..BuildOptions::default()
    };

    let (tx, rx) = mpsc::channel::<BuildEvent>();
    let result = tokio::time::timeout(
        Duration::from_secs(120),
        backend.build_image(tmp.path(), &dockerfile, &options, Some(tx)),
    )
    .await
    .expect("expected RUN false build to complete (with error) within 120s");

    assert!(
        result.is_err(),
        "expected build_image to return Err for `RUN false`, got: {:?}",
        result.as_ref().map(|b| &b.image_id)
    );

    let events: Vec<_> = rx.try_iter().collect();
    let saw_failure = events
        .iter()
        .any(|e| matches!(e, BuildEvent::BuildFailed { .. }));
    assert!(
        saw_failure,
        "did not observe BuildFailed event (got {} events: {:?})",
        events.len(),
        events
            .iter()
            .map(std::mem::discriminant)
            .collect::<Vec<_>>()
    );
}
