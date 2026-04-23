//! End-to-end integration tests for `zlayer-hcs`.
//!
//! The whole file is `#[cfg(windows)]`-gated to mirror the crate root. On
//! Linux `cargo test` still compiles the integration-test target, but every
//! item below is compiled out and the resulting test binary is empty.
//!
//! Tests that touch live HCS require Administrator (or membership of the
//! Hyper-V Administrators group). Those are gated through [`is_elevated`] at
//! runtime and skip with a `println!` when the caller isn't elevated, so the
//! suite stays green on developer machines and unprivileged CI runners.

#![cfg(windows)]
// Mirrors the `src/lib.rs` policy: this is an HCS FFI integration test that
// calls `OpenProcessToken` / `GetTokenInformation` directly to drive the
// elevation probe, so `unsafe` + the pointer-family lints are allowed here
// for the same reason they're allowed at the crate root.
#![allow(unsafe_code, clippy::borrow_as_ptr, clippy::items_after_statements)]

use zlayer_hcs::enumerate::{list as enum_list, list_by_owner, EnumerateQuery};
use zlayer_hcs::error::HcsError;
use zlayer_hcs::process::ComputeProcess;
use zlayer_hcs::schema::{
    Chipset, ComputeSystem as CsDoc, SchemaVersion, Statistics, Topology, TopologyMemory,
    TopologyProcessor, Uefi, UefiBootEntry, VirtualMachine,
};
use zlayer_hcs::system::ComputeSystem as System;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Best-effort elevation check. Returns `false` (safe default: skip) if the
/// token query itself fails — we never want a broken helper to falsely
/// enable destructive live-HCS tests.
fn is_elevated() -> bool {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Security::{
        GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    let mut token = HANDLE::default();
    unsafe {
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut elev = TOKEN_ELEVATION::default();
        let mut len = 0u32;
        let size = u32::try_from(std::mem::size_of::<TOKEN_ELEVATION>()).unwrap_or(0);
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some(std::ptr::from_mut(&mut elev).cast()),
            size,
            &mut len,
        );
        if ok.is_err() {
            return false;
        }
        elev.TokenIsElevated != 0
    }
}

fn skip_if_not_elevated(test_name: &str) -> bool {
    if !is_elevated() {
        println!("skipping {test_name} — requires Administrator");
        return true;
    }
    false
}

/// Build an empty, minimally viable Hyper-V VM compute-system document.
/// Mirrors the HCS "empty VM" completion sample — 256 MiB RAM, 1 vCPU, UEFI,
/// no attached disks.
fn empty_vm_document(owner: &str) -> CsDoc {
    CsDoc {
        owner: owner.to_string(),
        schema_version: SchemaVersion::default(),
        hosting_system_id: String::new(),
        container: None,
        virtual_machine: Some(VirtualMachine {
            chipset: Some(Chipset {
                uefi: Some(Uefi {
                    boot_this: Some(UefiBootEntry {
                        device_type: "VmbFs".to_string(),
                        device_path: String::new(),
                        disk_number: None,
                    }),
                }),
            }),
            compute_topology: Some(Topology {
                memory: Some(TopologyMemory { size_in_mb: 256 }),
                processor: Some(TopologyProcessor { count: 1 }),
            }),
            devices: None,
            guest_state: None,
            runtime_state_file_path: None,
        }),
        should_terminate_on_last_handle_closed: Some(true),
    }
}

// ---------------------------------------------------------------------------
// 1. Schema round-trip — no HCS involvement.
// ---------------------------------------------------------------------------

#[test]
fn schema_round_trip() {
    let doc = empty_vm_document("zlayer-test");

    let json = serde_json::to_string(&doc).expect("serialize");
    // Sanity-check the PascalCase wire format HCS expects.
    assert!(json.contains("\"Owner\":\"zlayer-test\""));
    assert!(json.contains("\"SchemaVersion\":{\"Major\":2,\"Minor\":1}"));
    assert!(json.contains("\"VirtualMachine\""));
    assert!(json.contains("\"SizeInMB\":256"));
    assert!(json.contains("\"Count\":1"));

    let back: CsDoc = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back.owner, "zlayer-test");
    assert_eq!(back.schema_version, SchemaVersion { major: 2, minor: 1 });
    assert!(back.container.is_none());
    assert_eq!(back.should_terminate_on_last_handle_closed, Some(true));

    let vm = back.virtual_machine.expect("virtual machine present");
    let topology = vm.compute_topology.expect("topology present");
    assert_eq!(topology.memory.expect("memory").size_in_mb, 256);
    assert_eq!(topology.processor.expect("processor").count, 1);

    let chipset = vm.chipset.expect("chipset present");
    let uefi = chipset.uefi.expect("uefi present");
    let boot = uefi.boot_this.expect("boot_this present");
    assert_eq!(boot.device_type, "VmbFs");
}

// ---------------------------------------------------------------------------
// 2. Empty-VM lifecycle — create → terminate → enumerate.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
#[ignore = "requires physical host with Hyper-V role enabled"]
async fn empty_vm_create_and_terminate() {
    if skip_if_not_elevated("empty_vm_create_and_terminate") {
        return;
    }

    let id = "zlayer-test-integration";
    let doc = empty_vm_document("zlayer-test");
    let json = serde_json::to_string(&doc).expect("serialize");

    let system = match System::create(id, &json).await {
        Ok(sys) => sys,
        Err(e) => {
            // Hyper-V not installed / stack unavailable → skip, not fail.
            println!("skipping empty_vm_create_and_terminate — create failed: {e:?}");
            return;
        }
    };

    // Terminate — accept any outcome (system may already be gone if HCS
    // torn-down-on-last-handle behaves unexpectedly).
    if let Err(e) = system.terminate("{}").await {
        println!("terminate returned: {e:?}");
    }
    drop(system);

    // After drop + terminate the system should not be listed any more.
    let listed = enum_list(&EnumerateQuery::default())
        .await
        .expect("enumerate");
    assert!(
        !listed.iter().any(|s| s.id == id),
        "test compute system still listed after terminate: {listed:?}"
    );
}

// ---------------------------------------------------------------------------
// 3. Enumerate self — owner that cannot possibly exist.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn enumerate_self_empty() {
    if skip_if_not_elevated("enumerate_self_empty") {
        return;
    }

    let result = list_by_owner("zlayer-test-this-owner-should-not-exist").await;
    match result {
        Ok(list) => assert!(list.is_empty(), "expected empty, got {list:?}"),
        Err(e) => {
            // Some Windows SKUs without the container feature installed fail
            // the enumerate call outright; skip with a note instead of
            // failing the suite.
            println!("enumerate_self_empty skipped — enumerate failed: {e:?}");
        }
    }
}

// ---------------------------------------------------------------------------
// 4. Statistics fixture parse — no HCS involvement.
// ---------------------------------------------------------------------------

#[test]
fn parse_statistics_fixture() {
    // Duplicated from `schema::tests::statistics_parses_sample_json` so this
    // integration binary exercises the public API surface it can see.
    let payload = r#"{
        "Timestamp":"2026-04-21T12:34:56Z",
        "Uptime100ns":2960000000,
        "Processor":{"TotalRuntime100ns":1234567,"RuntimeUser100ns":900000,"RuntimeKernel100ns":334567},
        "Memory":{"MemoryUsageCommitBytes":268435456,"MemoryUsageCommitPeakBytes":314572800,"MemoryUsagePrivateWorkingSetBytes":201326592},
        "Storage":{"ReadCountNormalized":42,"ReadSizeBytes":1048576,"WriteCountNormalized":13,"WriteSizeBytes":262144}
    }"#;

    let stats: Statistics = serde_json::from_str(payload).expect("parse statistics");

    assert_eq!(stats.timestamp.as_deref(), Some("2026-04-21T12:34:56Z"));
    assert_eq!(stats.uptime_100ns, 2_960_000_000);

    let cpu = stats.processor.expect("processor");
    assert_eq!(cpu.total_runtime_100ns, 1_234_567);
    assert_eq!(cpu.runtime_user_100ns, 900_000);
    assert_eq!(cpu.runtime_kernel_100ns, 334_567);

    let mem = stats.memory.expect("memory");
    assert_eq!(mem.memory_usage_commit_bytes, 268_435_456);
    assert_eq!(mem.memory_usage_commit_peak_bytes, 314_572_800);
    assert_eq!(mem.memory_usage_private_working_set_bytes, 201_326_592);

    let storage = stats.storage.expect("storage");
    assert_eq!(storage.read_count_normalized, 42);
    assert_eq!(storage.read_size_bytes, 1_048_576);
    assert_eq!(storage.write_count_normalized, 13);
    assert_eq!(storage.write_size_bytes, 262_144);
}

// ---------------------------------------------------------------------------
// 5. Opening a process on a null system with a bogus pid errors cleanly.
// ---------------------------------------------------------------------------

#[test]
fn open_nonexistent_process_errors_cleanly() {
    if skip_if_not_elevated("open_nonexistent_process_errors_cleanly") {
        return;
    }

    use windows::Win32::System::HostComputeSystem::HCS_SYSTEM;

    // Zeroed HCS_SYSTEM is the documented null handle per windows-rs 0.62.
    let null_system: HCS_SYSTEM = HCS_SYSTEM::default();

    // pid 999999, access 0 — we don't care which error variant comes back,
    // only that the call returns an `Err` rather than panicking or returning
    // a bogus handle. Expected variants include NotFound, AccessDenied, or
    // Other, depending on host configuration.
    let result = ComputeProcess::open(null_system, 999_999, 0);
    match result {
        Ok(_) => panic!("expected an error opening a non-existent process on a null system"),
        Err(e) => {
            // Any of these are acceptable — we just want a classified error.
            assert!(
                matches!(
                    e,
                    HcsError::NotFound { .. }
                        | HcsError::AccessDenied { .. }
                        | HcsError::Other { .. }
                        | HcsError::InvalidSchema { .. }
                ),
                "unexpected error variant: {e:?}"
            );
        }
    }
}
