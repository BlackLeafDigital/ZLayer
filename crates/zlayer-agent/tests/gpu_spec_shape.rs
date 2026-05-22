//! Cross-runtime GPU spec-shape integration tests.
//!
//! Given a single `ServiceSpec` carrying a `GpuSpec`, this file walks each
//! reachable production spec-emission path and asserts the resulting
//! device/mount/env shape. No real hardware required — CDI fixtures and
//! parser-only assertions cover every code path.
//!
//! # Surface-visibility audit (Phase 5.E)
//!
//! - **Linux OCI bundle** (`BundleBuilder`): `pub` —
//!   [`zlayer_agent::BundleBuilder::build_spec_only`] is the public entry
//!   point and is exercised here on `target_os = "linux"`.
//! - **Windows HCS doc** (`build_compute_system_doc` /
//!   `build_virtual_machine_doc`): crate-private. Cross-runtime assertion
//!   from an integration test is not possible without widening the public
//!   API surface; the equivalent unit tests live alongside the impl in
//!   `crates/zlayer-agent/src/runtimes/hcs.rs` (Phase 5.C). This file
//!   covers the spec-parsing half so that any regression on
//!   `ServiceSpec::isolation` + `resources.gpu` parsing is caught here too.
//! - **WSL2 mount injection** (`apply_wsl_gpu_to_spec` + `WslGpuHostProbe`):
//!   crate-private (`pub(crate)`). Equivalent unit tests live alongside
//!   the impl in `crates/zlayer-agent/src/runtimes/wsl2_delegate.rs` (Phase
//!   5.D). Same rationale as HCS.
//!
//! The `portable` module below is the value-add cross-cutting layer: a
//! single fixture template documenting every supported GPU spec shape, so
//! that schema regressions in one backend's wiring don't silently slip
//! past the other two.

#![allow(clippy::doc_markdown)]

use zlayer_spec::{DeploymentSpec, GpuSharingMode, GpuSpec, IsolationMode, ServiceSpec};

/// Build a `ServiceSpec` from a minimal deployment-spec YAML body. Centralizes
/// the boilerplate so every test case below shows only the GPU-relevant
/// fields.
fn service_spec_from_resources_yaml(resources_block: &str) -> ServiceSpec {
    let yaml = format!(
        "
version: v1
deployment: gpu-spec-shape-test
services:
  gpu-svc:
    rtype: service
    image:
      name: test:latest
{resources_block}
"
    );
    serde_yaml::from_str::<DeploymentSpec>(&yaml)
        .unwrap_or_else(|e| panic!("invalid test YAML: {e}\n--- YAML ---\n{yaml}"))
        .services
        .remove("gpu-svc")
        .expect("gpu-svc service present in fixture")
}

/// Single source-of-truth fixture: one NVIDIA GPU, default sharing,
/// distributed coordination off. Every runtime-specific test starts from
/// this shape and overlays its own knobs.
#[cfg(target_os = "linux")]
fn gpu_test_service_spec() -> ServiceSpec {
    service_spec_from_resources_yaml(
        "    resources:
      gpu:
        count: 1
        vendor: nvidia",
    )
}

// ---------------------------------------------------------------------------
// Linux OCI bundle: BundleBuilder is the public entry point.
// ---------------------------------------------------------------------------
#[cfg(target_os = "linux")]
mod linux {
    use super::{gpu_test_service_spec, service_spec_from_resources_yaml};
    use std::sync::Arc;
    use zlayer_agent::cdi::CdiRegistry;
    use zlayer_agent::{BundleBuilder, ContainerId};

    /// Minimal NVIDIA CDI fixture matching what `nvidia-ctk cdi generate`
    /// would emit on a real host with one GPU. Mirrors the unit-test
    /// fixture in `bundle.rs` so the cross-runtime integration view sees
    /// the same canonical shape.
    fn nvidia_cdi_fixture() -> &'static str {
        r#"{
            "cdiVersion": "0.6.0",
            "kind": "nvidia.com/gpu",
            "devices": [{
                "name": "0",
                "containerEdits": {
                    "deviceNodes": [
                        {"path": "/dev/nvidia0", "type": "c", "major": 195, "minor": 0}
                    ],
                    "env": ["NVIDIA_VISIBLE_DEVICES=0"],
                    "hooks": {
                        "createContainer": [{
                            "path": "/usr/bin/nvidia-container-runtime-hook",
                            "args": ["nvidia-container-runtime-hook", "prestart"]
                        }]
                    }
                }
            }]
        }"#
    }

    /// Cross-runtime assertion: feed the canonical fixture spec into the
    /// Linux bundle builder, point its CDI registry at an in-memory NVIDIA
    /// spec, and verify the resulting OCI bundle has the device node, env
    /// var, and create-container hook merged in.
    #[tokio::test]
    async fn gpu_spec_produces_cdi_merged_oci_bundle() {
        let dir = tempfile::tempdir().expect("tempdir for CDI fixture");
        std::fs::write(dir.path().join("nvidia.json"), nvidia_cdi_fixture())
            .expect("write CDI fixture");
        let registry = Arc::new(CdiRegistry::discover_from(&[dir.path()]));

        let id = ContainerId::new("gpu-svc".to_string(), 0);
        let spec = gpu_test_service_spec();
        let builder = BundleBuilder::new(dir.path().join("bundle")).with_cdi_registry(registry);

        let oci_spec = builder
            .build_spec_only(&id, &spec, &std::collections::HashMap::new())
            .await
            .expect("build_spec_only must succeed with CDI fixture present");

        // 1. linux.devices contains /dev/nvidia0.
        let linux = oci_spec.linux().as_ref().expect("linux config present");
        let devices = linux.devices().as_ref().expect("linux.devices present");
        assert!(
            devices
                .iter()
                .any(|d| d.path() == std::path::Path::new("/dev/nvidia0")),
            "expected /dev/nvidia0 from CDI fixture; got {:?}",
            devices
                .iter()
                .map(oci_spec::runtime::LinuxDevice::path)
                .collect::<Vec<_>>()
        );

        // 2. process.env contains NVIDIA_VISIBLE_DEVICES=0.
        let process = oci_spec.process().as_ref().expect("process present");
        let env = process.env().as_ref().expect("process.env present");
        assert!(
            env.iter().any(|e| e == "NVIDIA_VISIBLE_DEVICES=0"),
            "expected NVIDIA_VISIBLE_DEVICES=0 in env; got {env:?}"
        );

        // 3. hooks.createContainer contains the nvidia-container-runtime-hook.
        let hooks = oci_spec.hooks().as_ref().expect("hooks present");
        let create_container = hooks
            .create_container()
            .as_ref()
            .expect("createContainer hooks present");
        assert!(
            create_container
                .iter()
                .any(|h| h.path()
                    == &std::path::PathBuf::from("/usr/bin/nvidia-container-runtime-hook")),
            "expected nvidia-container-runtime-hook in createContainer; got {:?}",
            create_container
                .iter()
                .map(|h| h.path().clone())
                .collect::<Vec<_>>()
        );
    }

    /// Even without a CDI registry installed, the bundle builder must
    /// inject the baked-in NVIDIA env vars derived from the spec. This is
    /// the lazy-discover fallback path that lets production hosts without
    /// CDI specs still get `NVIDIA_VISIBLE_DEVICES` set.
    #[tokio::test]
    async fn gpu_spec_baked_in_env_without_cdi_registry() {
        let dir = tempfile::tempdir().expect("tempdir for bundle");
        let id = ContainerId::new("gpu-svc".to_string(), 0);
        let spec = gpu_test_service_spec();
        // No `.with_cdi_registry(...)` — exercises the lazy-discover path
        // where an empty registry triggers baked-in defaults.
        let builder = BundleBuilder::new(dir.path().join("bundle"));

        let oci_spec = builder
            .build_spec_only(&id, &spec, &std::collections::HashMap::new())
            .await
            .expect("build_spec_only must succeed without CDI registry");

        let process = oci_spec.process().as_ref().expect("process present");
        let env = process.env().as_ref().expect("process.env present");
        assert!(
            env.iter().any(|e| e == "NVIDIA_VISIBLE_DEVICES=0"),
            "expected baked-in NVIDIA_VISIBLE_DEVICES=0; got {env:?}"
        );
        assert!(
            env.iter().any(|e| e == "CUDA_VISIBLE_DEVICES=0"),
            "expected baked-in CUDA_VISIBLE_DEVICES=0; got {env:?}"
        );
    }

    /// AMD GPU spec must yield ROCm-style env vars even when CDI is not
    /// in play (no `amd.com/gpu` CDI kind exists in mainline yet).
    #[tokio::test]
    async fn amd_gpu_spec_yields_rocm_env_vars() {
        let dir = tempfile::tempdir().expect("tempdir for bundle");
        let spec = service_spec_from_resources_yaml(
            "    resources:
      gpu:
        count: 2
        vendor: amd",
        );
        let id = ContainerId::new("gpu-svc".to_string(), 0);
        let builder = BundleBuilder::new(dir.path().join("bundle"));

        let oci_spec = builder
            .build_spec_only(&id, &spec, &std::collections::HashMap::new())
            .await
            .expect("build_spec_only must succeed for AMD GPU spec");

        let process = oci_spec.process().as_ref().expect("process present");
        let env = process.env().as_ref().expect("process.env present");
        assert!(
            env.iter().any(|e| e == "ROCR_VISIBLE_DEVICES=0,1"),
            "expected ROCR_VISIBLE_DEVICES=0,1; got {env:?}"
        );
        assert!(
            env.iter().any(|e| e == "HIP_VISIBLE_DEVICES=0,1"),
            "expected HIP_VISIBLE_DEVICES=0,1; got {env:?}"
        );
    }

    /// Distributed coordination must inject the rank/world env vars that
    /// PyTorch/Horovod/DeepSpeed expect for multi-GPU jobs.
    #[tokio::test]
    async fn distributed_gpu_spec_injects_coordination_env() {
        let dir = tempfile::tempdir().expect("tempdir for bundle");
        let spec = service_spec_from_resources_yaml(
            "    resources:
      gpu:
        count: 1
        vendor: nvidia
        distributed:
          backend: nccl
          master_port: 29501",
        );
        let id = ContainerId::new("gpu-svc".to_string(), 0);
        let builder = BundleBuilder::new(dir.path().join("bundle"));

        let oci_spec = builder
            .build_spec_only(&id, &spec, &std::collections::HashMap::new())
            .await
            .expect("build_spec_only must succeed with distributed config");

        let process = oci_spec.process().as_ref().expect("process present");
        let env = process.env().as_ref().expect("process.env present");
        for expected in [
            "MASTER_PORT=29501",
            "MASTER_ADDR=gpu-svc",
            "WORLD_SIZE=1",
            "RANK=0",
            "LOCAL_RANK=0",
            "NCCL_SOCKET_IFNAME=eth0",
        ] {
            assert!(
                env.iter().any(|e| e == expected),
                "expected {expected} in env; got {env:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Windows HCS: build_compute_system_doc and build_virtual_machine_doc are
// crate-private. The cross-runtime spec-shape coverage lives next to the impl
// in `crates/zlayer-agent/src/runtimes/hcs.rs` (Phase 5.C); this module's
// purpose is to keep the placeholder visible so future API widening lights up
// a real integration assertion here.
// ---------------------------------------------------------------------------
#[cfg(target_os = "windows")]
mod windows_hcs {
    #[test]
    #[ignore = "integration: HCS doc builders are crate-private; covered by \
                in-crate unit tests in crates/zlayer-agent/src/runtimes/hcs.rs"]
    fn gpu_spec_with_hyperv_isolation_populates_assignment() {}

    #[test]
    #[ignore = "integration: HCS doc builders are crate-private; covered by \
                in-crate unit tests in crates/zlayer-agent/src/runtimes/hcs.rs"]
    fn gpu_spec_with_process_isolation_returns_unsupported() {}
}

// ---------------------------------------------------------------------------
// WSL2 delegate: apply_wsl_gpu_to_spec and WslGpuHostProbe are crate-private
// (`pub(crate)`). Cross-runtime spec-shape coverage lives next to the impl in
// `crates/zlayer-agent/src/runtimes/wsl2_delegate.rs` (Phase 5.D). Same
// placeholder rationale as windows_hcs above.
// ---------------------------------------------------------------------------
#[cfg(all(target_os = "windows", feature = "wsl"))]
mod wsl2 {
    #[test]
    #[ignore = "integration: apply_wsl_gpu_to_spec is crate-private; covered \
                by in-crate unit tests in crates/zlayer-agent/src/runtimes/wsl2_delegate.rs"]
    fn gpu_spec_injects_dxg_into_wsl_bundle() {}

    #[test]
    #[ignore = "integration: apply_wsl_gpu_to_spec is crate-private; covered \
                by in-crate unit tests in crates/zlayer-agent/src/runtimes/wsl2_delegate.rs"]
    fn gpu_spec_returns_error_when_dxg_missing() {}
}

// ---------------------------------------------------------------------------
// Cross-platform: portable GPU spec shape parsing. These tests run on every
// host (Linux CI, macOS CI, Windows CI) and pin the contract that every
// runtime backend reads from. Any future drift in `GpuSpec`'s serde shape
// gets caught here exactly once, no matter which OS the developer is on.
// ---------------------------------------------------------------------------
mod portable {
    use super::{service_spec_from_resources_yaml, IsolationMode};
    use zlayer_spec::{GpuSharingMode, SchedulingPolicy};

    /// Minimal positive case: one GPU, no advanced knobs, defaults apply.
    #[test]
    fn service_spec_yaml_with_gpu_block_parses() {
        let spec = service_spec_from_resources_yaml(
            "    resources:
      gpu:
        count: 1
        vendor: nvidia",
        );
        let gpu = spec.resources.gpu.expect("gpu block parsed");
        assert_eq!(gpu.count, 1);
        assert_eq!(gpu.vendor, "nvidia");
        assert!(gpu.mode.is_none());
        assert!(gpu.model.is_none());
        assert!(gpu.scheduling.is_none());
        assert!(gpu.distributed.is_none());
        assert!(gpu.sharing.is_none());
    }

    /// Negative case: a service spec with no `resources.gpu` block must
    /// surface as `None`, not a zero-count default. Catches accidental
    /// `#[serde(default)]` regressions on `ResourcesSpec::gpu`.
    #[test]
    fn service_spec_without_gpu_has_none() {
        let spec = service_spec_from_resources_yaml("");
        assert!(
            spec.resources.gpu.is_none(),
            "expected no gpu block; got {:?}",
            spec.resources.gpu
        );
    }

    /// Multi-GPU count for AMD; verifies that vendor strings other than
    /// nvidia round-trip without being silently rewritten.
    #[test]
    fn amd_multi_gpu_count_parses() {
        let spec = service_spec_from_resources_yaml(
            "    resources:
      gpu:
        count: 4
        vendor: amd",
        );
        let gpu = spec.resources.gpu.expect("gpu block parsed");
        assert_eq!(gpu.count, 4);
        assert_eq!(gpu.vendor, "amd");
    }

    /// Intel + explicit model substring (used by the scheduler to pin to a
    /// specific SKU like "Arc A770").
    #[test]
    fn intel_gpu_with_model_pinning_parses() {
        let spec = service_spec_from_resources_yaml(
            "    resources:
      gpu:
        count: 1
        vendor: intel
        model: A770",
        );
        let gpu = spec.resources.gpu.expect("gpu block parsed");
        assert_eq!(gpu.vendor, "intel");
        assert_eq!(gpu.model.as_deref(), Some("A770"));
    }

    /// Apple Silicon with `mode: native` (Metal/MPS via Seatbelt sandbox).
    /// This case validates that the macOS-only `mode` field is serde-visible
    /// on every host so the spec validator can reject it on the wrong
    /// platform with a clear error.
    #[test]
    fn apple_gpu_with_native_mode_parses() {
        let spec = service_spec_from_resources_yaml(
            "    resources:
      gpu:
        count: 1
        vendor: apple
        mode: native",
        );
        let gpu = spec.resources.gpu.expect("gpu block parsed");
        assert_eq!(gpu.vendor, "apple");
        assert_eq!(gpu.mode.as_deref(), Some("native"));
    }

    /// Every `SchedulingPolicy` variant round-trips through YAML (kebab-case
    /// per the `#[serde(rename_all = "kebab-case")]` derive).
    #[test]
    fn gpu_scheduling_policy_kebab_case_round_trip() {
        for (yaml_token, expected) in [
            ("best-effort", SchedulingPolicy::BestEffort),
            ("gang", SchedulingPolicy::Gang),
            ("spread", SchedulingPolicy::Spread),
        ] {
            let spec = service_spec_from_resources_yaml(&format!(
                "    resources:
      gpu:
        count: 1
        vendor: nvidia
        scheduling: {yaml_token}"
            ));
            let gpu = spec.resources.gpu.expect("gpu block parsed");
            assert_eq!(
                gpu.scheduling,
                Some(expected),
                "scheduling token '{yaml_token}' did not round-trip"
            );
        }
    }

    /// Every `GpuSharingMode` variant round-trips through YAML.
    #[test]
    fn gpu_sharing_mode_kebab_case_round_trip() {
        for (yaml_token, expected) in [
            ("exclusive", GpuSharingMode::Exclusive),
            ("mps", GpuSharingMode::Mps),
            ("time-slice", GpuSharingMode::TimeSlice),
        ] {
            let spec = service_spec_from_resources_yaml(&format!(
                "    resources:
      gpu:
        count: 1
        vendor: nvidia
        sharing: {yaml_token}"
            ));
            let gpu = spec.resources.gpu.expect("gpu block parsed");
            assert_eq!(
                gpu.sharing,
                Some(expected),
                "sharing token '{yaml_token}' did not round-trip"
            );
        }
    }

    /// `DistributedConfig` block parses with explicit backend + port; this
    /// is the field set the Linux bundle builder reads to emit MASTER_ADDR /
    /// MASTER_PORT / WORLD_SIZE env vars.
    #[test]
    fn distributed_block_parses_explicit_fields() {
        let spec = service_spec_from_resources_yaml(
            "    resources:
      gpu:
        count: 8
        vendor: nvidia
        distributed:
          backend: nccl
          master_port: 29500",
        );
        let gpu = spec.resources.gpu.expect("gpu block parsed");
        let dist = gpu.distributed.expect("distributed block parsed");
        assert_eq!(dist.backend, "nccl");
        assert_eq!(dist.master_port, 29500);
    }

    /// `DistributedConfig` defaults: omitting the inner fields must still
    /// produce a usable shape. This pins the serde defaults so a future
    /// renamer doesn't silently drop them.
    #[test]
    fn distributed_block_uses_defaults_when_inner_fields_omitted() {
        let spec = service_spec_from_resources_yaml(
            "    resources:
      gpu:
        count: 2
        vendor: nvidia
        distributed: {}",
        );
        let gpu = spec.resources.gpu.expect("gpu block parsed");
        let dist = gpu.distributed.expect("distributed block parsed");
        assert_eq!(dist.backend, "nccl");
        assert_eq!(dist.master_port, 29500);
    }

    /// `IsolationMode` rides on `ServiceSpec` itself (not under `resources`)
    /// and is consumed by the Windows HCS runtime to decide between
    /// `Process` and `Hyperv` container documents. Test the three variants
    /// so any drift in the runtime selection is caught at parse time.
    #[test]
    fn isolation_mode_kebab_case_round_trip() {
        let cases = [
            ("auto", IsolationMode::Auto),
            ("process", IsolationMode::Process),
            ("hyperv", IsolationMode::Hyperv),
        ];
        for (yaml_token, expected) in cases {
            let yaml = format!(
                "
version: v1
deployment: gpu-spec-shape-test
services:
  gpu-svc:
    rtype: service
    image:
      name: test:latest
    isolation: {yaml_token}
    resources:
      gpu:
        count: 1
        vendor: nvidia
"
            );
            let mut deployment: zlayer_spec::DeploymentSpec =
                serde_yaml::from_str(&yaml).expect("yaml parses");
            let svc = deployment.services.remove("gpu-svc").expect("service");
            assert_eq!(
                svc.isolation,
                Some(expected),
                "isolation token '{yaml_token}' did not round-trip"
            );
            assert!(
                svc.resources.gpu.is_some(),
                "gpu must coexist with isolation"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Build-time-only sanity: confirm the public types this file depends on are
// reachable through `zlayer_spec` re-exports. The compiler enforces this at
// build time — these aliases exist purely to surface a clear diagnostic if a
// future refactor moves any of them.
// ---------------------------------------------------------------------------
#[allow(dead_code)]
fn _public_type_visibility_anchor() {
    let _: Option<GpuSpec> = None;
    let _: Option<IsolationMode> = None;
    let _: Option<GpuSharingMode> = None;
    let _: Option<ServiceSpec> = None;
}
