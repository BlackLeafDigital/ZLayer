#![cfg(target_os = "windows")]
//! Windows-gated end-to-end tests for the HCS (Host Compute Service)
//! container runtime.
//!
//! These tests require a real Windows 10/11 or Windows Server host with
//! HCS available, outbound network access to `mcr.microsoft.com`, and the
//! ability to pull and run Windows nanoserver containers. They are
//! `#[ignore]` by default — run with:
//!
//! ```powershell
//! cargo test --package zlayer-agent --test windows_hcs_e2e -- --ignored --nocapture
//! ```
//!
//! # Why this file exists
//!
//! `HcsRuntime` is the Windows-native container runtime. It is feature
//! complete on paper but, without this file, has no automated end-to-end
//! coverage exercising the full
//! `pull_image → create_container → start_container → poll → exec → stop →
//! remove` lifecycle against a real Microsoft Container Registry image.
//! Test 1 pins the registry pull path; Test 2 is the high-value lifecycle
//! check; Test 3 proves that the synchronous exec path round-trips a
//! non-zero exit code through HCS.
#![allow(
    clippy::too_many_lines,
    clippy::used_underscore_binding,
    clippy::doc_markdown
)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use zlayer_agent::runtimes::hcs::{HcsConfig, HcsRuntime, IsolationMode};
use zlayer_agent::{ContainerId, ContainerState, Runtime};
use zlayer_spec::{DeploymentSpec, ServiceSpec};

/// MCR nanoserver tag known to be available for ltsc2022. Chosen because
/// it is small (~120 MB compressed) and exits-on-empty-entrypoint friendly.
const TEST_IMAGE: &str = "mcr.microsoft.com/windows/nanoserver:ltsc2022";

/// Generate a unique short suffix so parallel CI shards do not collide on
/// the per-test storage root or HCS system id.
#[allow(clippy::cast_possible_truncation)]
fn unique_name(prefix: &str) -> String {
    let pid = std::process::id();
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
        % 1_000_000;
    format!("{prefix}-{pid}-{ts}")
}

/// Allocate a per-test storage root under the OS temp dir. We deliberately
/// avoid `tempfile::TempDir` so that a teardown failure leaves the directory
/// visible for post-mortem on CI — the explicit `remove_dir_all` calls in
/// each test perform the cleanup.
fn fresh_storage_root(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("zlayer-hcs-e2e-{}", unique_name(tag)))
}

/// Build the test-local `HcsConfig`. `slice_cidr` is intentionally left at
/// `None`: the runtime warns and proceeds without overlay networking, which
/// is fine for these tests — none of them rely on container-to-container
/// or container-to-internet traffic.
fn test_config(tag: &str) -> (HcsConfig, PathBuf) {
    let storage_root = fresh_storage_root(tag);
    let cfg = HcsConfig {
        storage_root: storage_root.clone(),
        default_isolation: IsolationMode::Process,
        default_scratch_size_gb: 20,
        ..HcsConfig::default()
    };
    (cfg, storage_root)
}

/// Parse a single-service `DeploymentSpec` YAML and pop the named service
/// out. Mirrors the helper pattern used in `macos_sandbox_e2e.rs`.
fn spec_from_yaml(yaml: &str, service: &str) -> ServiceSpec {
    serde_yaml::from_str::<DeploymentSpec>(yaml)
        .expect("test YAML must parse into DeploymentSpec")
        .services
        .remove(service)
        .unwrap_or_else(|| panic!("test YAML is missing service {service:?}"))
}

/// Build a short-lived spec that exits with code 0 immediately.
fn echo_spec() -> ServiceSpec {
    let yaml = format!(
        r#"
version: v1
deployment: hcs-e2e
services:
  echo:
    rtype: service
    image:
      name: {TEST_IMAGE}
    command:
      entrypoint: ["cmd", "/c", "echo", "hello from hcs"]
    endpoints:
      - name: dummy
        protocol: tcp
        port: 8080
    scale:
      mode: fixed
      replicas: 1
"#
    );
    spec_from_yaml(&yaml, "echo")
}

/// Build a long-lived spec used for the exec round-trip test. `ping -n 60`
/// keeps the container alive ~60 seconds (one ping per second, default
/// interval), which is plenty for a single `cmd /c exit 7` exec round-trip
/// to complete. `timeout.exe` is not present in nanoserver's PATH, so we
/// use `ping` against loopback instead — the classic Windows sleep trick.
fn long_lived_spec() -> ServiceSpec {
    let yaml = format!(
        r#"
version: v1
deployment: hcs-e2e
services:
  longlived:
    rtype: service
    image:
      name: {TEST_IMAGE}
    command:
      entrypoint: ["cmd", "/c", "ping", "-n", "60", "127.0.0.1"]
    endpoints:
      - name: dummy
        protocol: tcp
        port: 8080
    scale:
      mode: fixed
      replicas: 1
"#
    );
    spec_from_yaml(&yaml, "longlived")
}

/// Poll `container_state` until it matches the expected variant or the
/// budget is exhausted. For `Exited` we match the variant (not the code)
/// because the caller asserts on the code separately. `Running` is matched
/// exactly. Returns the last observed state on success so callers can
/// inspect the exit code.
async fn wait_for_state(
    runtime: &dyn Runtime,
    id: &ContainerId,
    expected: ContainerState,
    budget: Duration,
) -> Result<ContainerState, String> {
    let start = std::time::Instant::now();
    let poll = Duration::from_millis(200);
    let mut last: Option<ContainerState> = None;

    while start.elapsed() < budget {
        match runtime.container_state(id).await {
            Ok(state) => {
                let matches = match (&state, &expected) {
                    (ContainerState::Exited { .. }, ContainerState::Exited { .. }) => true,
                    (a, b) => a == b,
                };
                if matches {
                    return Ok(state);
                }
                last = Some(state);
            }
            Err(e) => return Err(format!("container_state error: {e}")),
        }
        tokio::time::sleep(poll).await;
    }

    Err(format!(
        "timed out after {:?} waiting for {expected:?}; last observed = {last:?}",
        budget
    ))
}

/// Best-effort directory removal that swallows errors. Used in teardown
/// where we must not mask the original assertion failure.
fn rm_storage_root(root: &PathBuf) {
    let _ = std::fs::remove_dir_all(root);
}

/// Test 1: image pull pins the on-disk image cache against the public MCR
/// nanoserver layer chain. If this passes, the registry client, blob cache,
/// and HCS layer unpacker are all wired correctly on the host.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires Windows host with HCS + outbound access to mcr.microsoft.com"]
async fn test_pull_nanoserver_image() {
    let outcome = tokio::time::timeout(Duration::from_secs(600), async {
        let (cfg, storage_root) = test_config("pull");
        let runtime = HcsRuntime::new(cfg)
            .await
            .expect("HcsRuntime::new must succeed on a Windows host with HCS available");

        let pull_result = runtime.pull_image(TEST_IMAGE).await;

        let assertion = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            assert!(
                pull_result.is_ok(),
                "pull_image({TEST_IMAGE}) must succeed; got {pull_result:?}",
            );
        }));

        rm_storage_root(&storage_root);

        if let Err(p) = assertion {
            std::panic::resume_unwind(p);
        }
    })
    .await;

    outcome.expect("test_pull_nanoserver_image exceeded the 600s outer budget");
}

/// Test 2: high-value end-to-end lifecycle. Pull the image, create a
/// container with a short-lived entrypoint, start it, poll for exit, and
/// remove it. Asserts the captured exit code is 0 because `cmd /c echo`
/// exits cleanly once the line is printed.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires Windows host with HCS + outbound access to mcr.microsoft.com"]
async fn test_run_short_lived_container_lifecycle() {
    let outcome = tokio::time::timeout(Duration::from_secs(600), async {
        let (cfg, storage_root) = test_config("life");
        let runtime: Arc<HcsRuntime> = Arc::new(
            HcsRuntime::new(cfg)
                .await
                .expect("HcsRuntime::new must succeed"),
        );

        runtime
            .pull_image(TEST_IMAGE)
            .await
            .expect("pull_image must succeed before create_container");

        let id = ContainerId::new(unique_name("life"), 1);
        let spec = echo_spec();

        let create_result = runtime.create_container(&id, &spec).await;

        let runtime_for_body = runtime.clone();
        let id_for_body = id.clone();
        let body = async move {
            create_result.expect("create_container must succeed");
            runtime_for_body
                .start_container(&id_for_body)
                .await
                .expect("start_container must succeed");

            let final_state = wait_for_state(
                runtime_for_body.as_ref(),
                &id_for_body,
                ContainerState::Exited { code: 0 },
                Duration::from_secs(60),
            )
            .await
            .expect("container must reach Exited within 60s");

            match final_state {
                ContainerState::Exited { code } => {
                    assert_eq!(code, 0, "echo entrypoint must exit with code 0");
                }
                other => panic!("expected Exited, got {other:?}"),
            }
        };

        let body_outcome = std::panic::AssertUnwindSafe(body)
            .catch_unwind_async()
            .await;

        let _ = runtime.remove_container(&id).await;
        rm_storage_root(&storage_root);

        if let Err(p) = body_outcome {
            std::panic::resume_unwind(p);
        }
    })
    .await;

    outcome.expect("test_run_short_lived_container_lifecycle exceeded the 600s outer budget");
}

/// Test 3: `exec` round-trips a non-zero exit code through HCS. Stdout and
/// stderr are unchecked because the HCS exec path currently returns empty
/// placeholders for both — see `crates/zlayer-agent/src/runtimes/hcs.rs:1311`.
/// The exit code, however, comes directly from `HcsGetProcessProperties` and
/// is the authoritative status signal for callers that use exec as a health
/// probe.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires Windows host with HCS + outbound access to mcr.microsoft.com"]
async fn test_exec_into_running_container() {
    let outcome = tokio::time::timeout(Duration::from_secs(600), async {
        let (cfg, storage_root) = test_config("exec");
        let runtime: Arc<HcsRuntime> = Arc::new(
            HcsRuntime::new(cfg)
                .await
                .expect("HcsRuntime::new must succeed"),
        );

        runtime
            .pull_image(TEST_IMAGE)
            .await
            .expect("pull_image must succeed before create_container");

        let id = ContainerId::new(unique_name("exec"), 1);
        let spec = long_lived_spec();

        let create_result = runtime.create_container(&id, &spec).await;

        let runtime_for_body = runtime.clone();
        let id_for_body = id.clone();
        let body = async move {
            create_result.expect("create_container must succeed");
            runtime_for_body
                .start_container(&id_for_body)
                .await
                .expect("start_container must succeed");

            wait_for_state(
                runtime_for_body.as_ref(),
                &id_for_body,
                ContainerState::Running,
                Duration::from_secs(30),
            )
            .await
            .expect("container must reach Running within 30s");

            let cmd: Vec<String> = vec!["cmd".into(), "/c".into(), "exit".into(), "7".into()];
            let (exit_code, _stdout, _stderr) = runtime_for_body
                .exec(&id_for_body, &cmd)
                .await
                .expect("exec must succeed against a Running container");

            assert_eq!(
                exit_code, 7,
                "exec must round-trip the child's exit code through HCS",
            );
        };

        let body_outcome = std::panic::AssertUnwindSafe(body)
            .catch_unwind_async()
            .await;

        let _ = runtime.stop_container(&id, Duration::from_secs(5)).await;
        let _ = runtime.remove_container(&id).await;
        rm_storage_root(&storage_root);

        if let Err(p) = body_outcome {
            std::panic::resume_unwind(p);
        }
    })
    .await;

    outcome.expect("test_exec_into_running_container exceeded the 600s outer budget");
}

/// Local async-aware `catch_unwind` adapter. `std::panic::catch_unwind`
/// cannot be wrapped around an `async` block directly because the future
/// must be polled, not just called once. This helper polls the wrapped
/// future and catches any panic raised inside `poll`, converting it into
/// the same `Err` shape `catch_unwind` produces for sync code.
trait CatchUnwindAsyncExt: std::future::Future + Sized {
    async fn catch_unwind_async(
        self,
    ) -> Result<Self::Output, Box<dyn std::any::Any + Send + 'static>>;
}

impl<F> CatchUnwindAsyncExt for std::panic::AssertUnwindSafe<F>
where
    F: std::future::Future,
{
    async fn catch_unwind_async(
        self,
    ) -> Result<F::Output, Box<dyn std::any::Any + Send + 'static>> {
        use std::future::Future;
        use std::pin::Pin;
        use std::task::Poll;

        let mut inner = Box::pin(self.0);
        std::future::poll_fn(move |cx| {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                Pin::new(&mut inner).as_mut().poll(cx)
            }));
            match result {
                Ok(Poll::Pending) => Poll::Pending,
                Ok(Poll::Ready(v)) => Poll::Ready(Ok(v)),
                Err(panic) => Poll::Ready(Err(panic)),
            }
        })
        .await
    }
}
