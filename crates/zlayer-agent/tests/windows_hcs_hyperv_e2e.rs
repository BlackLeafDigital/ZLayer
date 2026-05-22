#![cfg(target_os = "windows")]
//! Windows-gated end-to-end tests for the HCS (Host Compute Service)
//! container runtime under **Hyper-V isolation**.
//!
//! These tests mirror `tests/windows_hcs_e2e.rs` (the W2 process-isolation
//! suite) but pin `isolation: hyperv` on every `ServiceSpec`. They require a
//! real Windows 10/11 or Windows Server host with the **Hyper-V** feature
//! AND the **Windows Containers** feature enabled (the latter installs the
//! UVM boot files under
//! `%ProgramData%\Microsoft\Windows\Hyper-V\Containers\` that
//! `zlayer_agent::windows::uvm::Uvm::create` probes for), plus outbound
//! network access to `mcr.microsoft.com` to pull the nanoserver layers.
//!
//! They are `#[ignore]` by default — run with:
//!
//! ```powershell
//! cargo test --package zlayer-agent --test windows_hcs_hyperv_e2e -- --ignored --nocapture
//! ```
//!
//! # Why this file exists
//!
//! W3 wires Hyper-V isolation end-to-end through the same `Runtime` trait
//! surface that process isolation already used. The compute-system doc
//! switches from a populated `Container` body to a populated
//! `VirtualMachine` body backed by a per-container UVM scratch VHDX. Without
//! this file, that Hyper-V code path has no automated coverage of the full
//! `pull_image → create_container → start_container → poll → exec → stop →
//! remove` lifecycle on a real Hyper-V host. Test 1 pins the single-container
//! lifecycle, test 2 proves `exec` round-trips through the UVM, and test 3
//! exercises two independent UVMs in parallel to make sure scratch VHDX
//! allocation and teardown do not collide.
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
/// The same tag also boots fine inside the Hyper-V UVM that the Windows
/// Containers feature ships, so we use it for both isolation modes.
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
/// (and the scratch VHDX inside it) visible for post-mortem on CI — the
/// explicit `remove_dir_all` calls in each test perform the cleanup.
fn fresh_storage_root(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("zlayer-hcs-hyperv-e2e-{}", unique_name(tag)))
}

/// Build the test-local `HcsConfig`. `default_isolation` is pinned to
/// `Hyperv` so even a spec that forgets to set `isolation: hyperv` would
/// still resolve to Hyper-V — but every spec in this file sets it
/// explicitly too, defense in depth.
///
/// `slice_cidr` is intentionally left at `None`: the runtime warns and
/// proceeds without overlay networking, which is fine for these tests —
/// none of them rely on container-to-container or container-to-internet
/// traffic.
fn test_config(tag: &str) -> (HcsConfig, PathBuf) {
    let storage_root = fresh_storage_root(tag);
    let cfg = HcsConfig {
        storage_root: storage_root.clone(),
        default_isolation: IsolationMode::Hyperv,
        default_scratch_size_gb: 20,
        ..HcsConfig::default()
    };
    (cfg, storage_root)
}

/// Construct a fresh `HcsRuntime` wrapped in `Arc` together with the
/// per-test storage root the caller is responsible for cleaning up. We do
/// this in a single helper because all three tests share the exact same
/// setup boilerplate — extracting it keeps each test focused on its
/// assertion shape.
async fn make_runtime(tag: &str) -> (Arc<HcsRuntime>, PathBuf) {
    let (cfg, storage_root) = test_config(tag);
    let runtime = HcsRuntime::new(cfg)
        .await
        .expect("HcsRuntime::new must succeed on a Windows host with HCS available");
    (Arc::new(runtime), storage_root)
}

/// Parse a single-service `DeploymentSpec` YAML and pop the named service
/// out. Mirrors the helper pattern used in `windows_hcs_e2e.rs`.
fn spec_from_yaml(yaml: &str, service: &str) -> ServiceSpec {
    serde_yaml::from_str::<DeploymentSpec>(yaml)
        .expect("test YAML must parse into DeploymentSpec")
        .services
        .remove(service)
        .unwrap_or_else(|| panic!("test YAML is missing service {service:?}"))
}

/// Build a short-lived spec that exits with code 0 immediately, pinned to
/// Hyper-V isolation. `isolation: hyperv` is `serde(rename_all = "kebab-case")`
/// for [`zlayer_spec::IsolationMode::Hyperv`].
fn echo_spec_hyperv() -> ServiceSpec {
    let yaml = format!(
        r#"
version: v1
deployment: hcs-hyperv-e2e
services:
  echo:
    rtype: service
    isolation: hyperv
    image:
      name: {TEST_IMAGE}
    command:
      entrypoint: ["cmd", "/c", "echo", "hello from hyperv"]
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

/// Build a long-lived spec used for the exec round-trip test, also pinned
/// to Hyper-V. `ping -n 60` keeps the container alive ~60 seconds — plenty
/// for `cmd /c hostname` to round-trip through the UVM.
fn long_lived_spec_hyperv() -> ServiceSpec {
    let yaml = format!(
        r#"
version: v1
deployment: hcs-hyperv-e2e
services:
  longlived:
    rtype: service
    isolation: hyperv
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
        "timed out after {budget:?} waiting for {expected:?}; last observed = {last:?}"
    ))
}

/// Best-effort directory removal that swallows errors. Used in teardown
/// where we must not mask the original assertion failure. The per-container
/// scratch VHDX inside is dropped by `Uvm::Drop` when the runtime tears
/// down its `ContainerEntry`, so by the time we get here only an empty
/// (or near-empty) directory should remain.
fn rm_storage_root(root: &PathBuf) {
    let _ = std::fs::remove_dir_all(root);
}

/// Test 1: end-to-end smoke for the Hyper-V isolation path. Pull the image,
/// create a Hyper-V-isolated container with a short-lived entrypoint, start
/// it, poll for exit, and remove it. Asserts the captured exit code is 0
/// because `cmd /c echo` exits cleanly once the line is printed.
///
/// Reaching `Exited { code: 0 }` here implies the UVM scratch VHDX was
/// allocated, the `VirtualMachine` compute-system doc was accepted by HCS,
/// the UVM booted, the container started inside it, and the UVM tore down
/// cleanly on `remove_container` (otherwise `Uvm::Drop` would log a leak).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires Windows host with Hyper-V feature + Windows Containers feature enabled, plus nanoserver:ltsc2022 layers"]
async fn windows_hcs_hyperv_smoke_create_start_stop_remove() {
    let outcome = tokio::time::timeout(Duration::from_secs(900), async {
        let (runtime, storage_root) = make_runtime("smoke").await;

        runtime
            .pull_image(TEST_IMAGE)
            .await
            .expect("pull_image must succeed before create_container");

        let id = ContainerId::new(unique_name("smoke"), 1);
        let spec = echo_spec_hyperv();

        let create_result = runtime.create_container(&id, &spec).await;

        let runtime_for_body = runtime.clone();
        let id_for_body = id.clone();
        let body = async move {
            create_result.expect("create_container must succeed under Hyper-V isolation");
            runtime_for_body
                .start_container(&id_for_body)
                .await
                .expect("start_container must succeed under Hyper-V isolation");

            let final_state = wait_for_state(
                runtime_for_body.as_ref(),
                &id_for_body,
                ContainerState::Exited { code: 0 },
                Duration::from_secs(180),
            )
            .await
            .expect("Hyper-V container must reach Exited within 180s");

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

    outcome
        .expect("windows_hcs_hyperv_smoke_create_start_stop_remove exceeded the 900s outer budget");
}

/// Test 2: `exec` round-trips through the Hyper-V UVM into the container.
/// `cmd /c hostname` is a safe builtin available in nanoserver. We assert
/// exit code 0 and non-empty stdout — the HCS exec path returns
/// authoritative exit codes from `HcsGetProcessProperties`, and `hostname`
/// always prints at least the container's NetBIOS name + newline, so a
/// non-empty stdout means the exec actually ran inside the UVM (vs.
/// shorting out at the runtime boundary).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires Windows host with Hyper-V feature + Windows Containers feature enabled, plus nanoserver:ltsc2022 layers"]
async fn windows_hcs_hyperv_exec_inside_container() {
    let outcome = tokio::time::timeout(Duration::from_secs(900), async {
        let (runtime, storage_root) = make_runtime("exec").await;

        runtime
            .pull_image(TEST_IMAGE)
            .await
            .expect("pull_image must succeed before create_container");

        let id = ContainerId::new(unique_name("exec"), 1);
        let spec = long_lived_spec_hyperv();

        let create_result = runtime.create_container(&id, &spec).await;

        let runtime_for_body = runtime.clone();
        let id_for_body = id.clone();
        let body = async move {
            create_result.expect("create_container must succeed under Hyper-V isolation");
            runtime_for_body
                .start_container(&id_for_body)
                .await
                .expect("start_container must succeed under Hyper-V isolation");

            wait_for_state(
                runtime_for_body.as_ref(),
                &id_for_body,
                ContainerState::Running,
                Duration::from_secs(120),
            )
            .await
            .expect("Hyper-V container must reach Running within 120s");

            let cmd: Vec<String> = vec!["cmd".into(), "/c".into(), "hostname".into()];
            let (exit_code, stdout, _stderr) = runtime_for_body
                .exec(&id_for_body, &cmd)
                .await
                .expect("exec must succeed against a Running Hyper-V container");

            assert_eq!(
                exit_code, 0,
                "`cmd /c hostname` must exit with code 0 inside the UVM",
            );
            assert!(
                !stdout.is_empty(),
                "`cmd /c hostname` must print at least the container hostname to stdout",
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

    outcome.expect("windows_hcs_hyperv_exec_inside_container exceeded the 900s outer budget");
}

/// Test 3: two independent Hyper-V containers run concurrently inside the
/// same daemon. Each container gets its own UVM with its own scratch VHDX,
/// and tearing one down must not affect the other. This catches scratch
/// VHDX path collisions, UVM ID collisions, and any cross-container
/// resource sharing that would silently corrupt one side.
///
/// We create both, start both in parallel, wait for both to reach
/// `Running`, then stop and remove them independently and verify each
/// finishes its full lifecycle without interfering with the other.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires Windows host with Hyper-V feature + Windows Containers feature enabled, plus nanoserver:ltsc2022 layers"]
async fn windows_hcs_hyperv_concurrent_pair() {
    let outcome = tokio::time::timeout(Duration::from_secs(1200), async {
        let (runtime, storage_root) = make_runtime("pair").await;

        runtime
            .pull_image(TEST_IMAGE)
            .await
            .expect("pull_image must succeed before create_container");

        let id_a = ContainerId::new(unique_name("pair-a"), 1);
        let id_b = ContainerId::new(unique_name("pair-b"), 1);
        let spec_a = long_lived_spec_hyperv();
        let spec_b = long_lived_spec_hyperv();

        // Create both containers serially — `create_container` mutates the
        // shared `containers` map under a write lock, so back-to-back
        // serial creates are the correct shape (not concurrent).
        let create_a = runtime.create_container(&id_a, &spec_a).await;
        let create_b = runtime.create_container(&id_b, &spec_b).await;

        let runtime_for_body = runtime.clone();
        let id_a_for_body = id_a.clone();
        let id_other_for_body = id_b.clone();
        let body = async move {
            create_a.expect("create_container(a) must succeed under Hyper-V isolation");
            create_b.expect("create_container(b) must succeed under Hyper-V isolation");

            // Start both UVMs in parallel — this is the part that would
            // collide if scratch VHDX paths or UVM IDs were shared.
            let (start_a, start_b) = tokio::join!(
                runtime_for_body.start_container(&id_a_for_body),
                runtime_for_body.start_container(&id_other_for_body),
            );
            start_a.expect("start_container(a) must succeed");
            start_b.expect("start_container(b) must succeed");

            // Wait for both to reach Running in parallel.
            let (wait_a, wait_b) = tokio::join!(
                wait_for_state(
                    runtime_for_body.as_ref(),
                    &id_a_for_body,
                    ContainerState::Running,
                    Duration::from_secs(180),
                ),
                wait_for_state(
                    runtime_for_body.as_ref(),
                    &id_other_for_body,
                    ContainerState::Running,
                    Duration::from_secs(180),
                ),
            );
            wait_a.expect("container a must reach Running within 180s");
            wait_b.expect("container b must reach Running within 180s");

            // Stop and remove `a` first, then verify `b` is still Running.
            // This is the critical assertion — tearing down one UVM must
            // not affect the other.
            runtime_for_body
                .stop_container(&id_a_for_body, Duration::from_secs(10))
                .await
                .expect("stop_container(a) must succeed independently of b");
            runtime_for_body
                .remove_container(&id_a_for_body)
                .await
                .expect("remove_container(a) must succeed independently of b");

            let state_b = runtime_for_body
                .container_state(&id_other_for_body)
                .await
                .expect("container_state(b) must still be queryable after a is gone");
            assert_eq!(
                state_b,
                ContainerState::Running,
                "tearing down container a must not affect container b",
            );

            // Now tear down `b`.
            runtime_for_body
                .stop_container(&id_other_for_body, Duration::from_secs(10))
                .await
                .expect("stop_container(b) must succeed");
            runtime_for_body
                .remove_container(&id_other_for_body)
                .await
                .expect("remove_container(b) must succeed");
        };

        let body_outcome = std::panic::AssertUnwindSafe(body)
            .catch_unwind_async()
            .await;

        // Best-effort cleanup in case the body panicked partway through.
        let _ = runtime.stop_container(&id_a, Duration::from_secs(5)).await;
        let _ = runtime.remove_container(&id_a).await;
        let _ = runtime.stop_container(&id_b, Duration::from_secs(5)).await;
        let _ = runtime.remove_container(&id_b).await;
        rm_storage_root(&storage_root);

        if let Err(p) = body_outcome {
            std::panic::resume_unwind(p);
        }
    })
    .await;

    outcome.expect("windows_hcs_hyperv_concurrent_pair exceeded the 1200s outer budget");
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
