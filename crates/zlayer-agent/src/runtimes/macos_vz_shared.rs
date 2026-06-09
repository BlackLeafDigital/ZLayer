//! Guest-agnostic helpers shared by the macOS Apple-Virtualization runtimes.
//!
//! Both the native-macOS guest runtime
//! ([`crate::runtimes::macos_vz::VzRuntime`]) and the Linux-guest runtime
//! ([`crate::runtimes::macos_vz_linux::VzLinuxRuntime`]) drive Apple's
//! `Virtualization.framework`. The pieces that are independent of the guest OS
//! — the serial-queue Send/Sync bridge, the live-VM handle, the lifecycle
//! op runner, VM-state reads, DHCP-lease parsing, APFS clonefile, and ephemeral
//! SSH keypair generation — live here so both runtimes share one implementation.
//!
//! `VZVirtualMachine` is **not** thread-safe: every call to a live VM happens on
//! a single serial [`DispatchQueue`]. The [`QueuePinned`] wrapper asserts
//! `Send`/`Sync` for handles that are only ever touched on that queue.

#![allow(unsafe_code)]

use std::net::IpAddr;
use std::path::Path;
use std::sync::Arc;

use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchRetained};
use objc2::rc::Retained;
use objc2_foundation::{NSError, NSString, NSURL};
use objc2_virtualization::{
    VZVirtualMachine, VZVirtualMachineConfiguration, VZVirtualMachineState,
};
use zlayer_registry::ImageConfig;
use zlayer_spec::ServiceSpec;

// ---------------------------------------------------------------------------
// Send/Sync bridge for objc / dispatch handles
// ---------------------------------------------------------------------------

/// Assert `Send`/`Sync` for a value that is only ever touched on a single
/// serial dispatch queue. `VZVirtualMachine` and its config objects are not
/// thread-safe, but we serialize **all** access through one queue, so moving
/// the handle between the queue's worker threads is sound.
pub(crate) struct QueuePinned<T>(pub(crate) T);
// SAFETY: every access to the wrapped handle is funnelled through a single
// serial `DispatchQueue`, so there is never concurrent access even though the
// underlying Obj-C object is not itself `Send`/`Sync`.
unsafe impl<T> Send for QueuePinned<T> {}
unsafe impl<T> Sync for QueuePinned<T> {}

/// A live virtual machine plus the serial queue all its operations run on.
pub(crate) struct LiveVm {
    pub(crate) queue: DispatchRetained<DispatchQueue>,
    pub(crate) vm: Arc<QueuePinned<Retained<VZVirtualMachine>>>,
}

// ---------------------------------------------------------------------------
// Guest-agnostic VM-configuration helpers
// ---------------------------------------------------------------------------

/// A file-URL `NSURL` for a path.
pub(crate) fn file_url(path: &Path) -> Retained<NSURL> {
    let s = NSString::from_str(&path.to_string_lossy());
    NSURL::fileURLWithPath(&s)
}

/// Parse a memory string like "512Mi" / "2Gi" / "8589934592" into MiB.
#[allow(clippy::cast_possible_truncation)]
pub(crate) fn parse_memory_to_mib(s: &str) -> Option<u32> {
    let s = s.trim();
    if let Some(num) = s.strip_suffix("Gi") {
        num.parse::<u32>().ok().map(|v| v * 1024)
    } else if let Some(num) = s.strip_suffix("Mi") {
        num.parse::<u32>().ok()
    } else if let Some(num) = s.strip_suffix("Ki") {
        num.parse::<u32>().ok().map(|v| v / 1024)
    } else {
        s.parse::<u64>().ok().map(|v| (v / (1024 * 1024)) as u32)
    }
}

/// Clamp a requested vCPU count to `[1, host_cores]`.
#[allow(clippy::cast_possible_truncation)]
fn safe_vcpu_count(requested: u32) -> u32 {
    let host_cores = num_cpus::get() as u32;
    requested.clamp(1, host_cores.max(1))
}

/// Clamp a requested memory-in-MiB value to the framework's allowed range,
/// returning bytes (the unit `VZVirtualMachineConfiguration.setMemorySize`
/// expects).
pub(crate) fn clamp_memory_bytes(req_mib: u32) -> u64 {
    // SAFETY: these are pure class-method queries with no side effects.
    let min = unsafe { VZVirtualMachineConfiguration::minimumAllowedMemorySize() };
    let max = unsafe { VZVirtualMachineConfiguration::maximumAllowedMemorySize() };
    let requested = u64::from(req_mib) * 1024 * 1024;
    requested.clamp(min, max)
}

/// Clamp a requested vCPU count to the framework's allowed CPU range AND the
/// host core count.
pub(crate) fn clamp_cpu_count(req: u32) -> usize {
    // SAFETY: these are pure class-method queries with no side effects.
    let min = unsafe { VZVirtualMachineConfiguration::minimumAllowedCPUCount() };
    let max = unsafe { VZVirtualMachineConfiguration::maximumAllowedCPUCount() };
    let host_clamped = safe_vcpu_count(req) as usize;
    host_clamped.clamp(min, max)
}

/// vCPUs requested by a spec, defaulting to `default` (>=1).
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub(crate) fn spec_vcpus(spec: &ServiceSpec, default: u32) -> u32 {
    spec.resources
        .cpu
        .map(|c| c.ceil() as u32)
        .filter(|&c| c > 0)
        .unwrap_or(default)
}

/// Memory (MiB) requested by a spec, defaulting to `default` and never below
/// `floor` (a guest-specific minimum).
pub(crate) fn spec_memory_mib(spec: &ServiceSpec, default: u32, floor: u32) -> u32 {
    spec.resources
        .memory
        .as_deref()
        .and_then(parse_memory_to_mib)
        .unwrap_or(default)
        .max(floor)
}

/// Resolve the effective container command (`argv`) by merging the spec's
/// command overrides with the OCI **image config** (`Entrypoint`/`Cmd`), per
/// Docker/OCI semantics. Falls back to a no-op `true` only when neither the
/// spec nor the image supplies any command (so a bare, configless image still
/// "runs" rather than failing to exec).
///
/// Resolution rules (matching `docker run` / the OCI runtime spec):
/// - spec **entrypoint** set (non-empty):
///   - argv = spec entrypoint + (spec args if set, else the image's `Cmd`).
/// - spec **entrypoint** NOT set:
///   - spec args set (non-empty): argv = image `Entrypoint` (if any) + spec args.
///   - spec args NOT set:          argv = image `Entrypoint` + image `Cmd`.
/// - If the merged result is still empty, fall back to `["true"]`.
pub(crate) fn resolve_entrypoint(spec: &ServiceSpec, image: Option<&ImageConfig>) -> Vec<String> {
    let image_entrypoint = image.and_then(|c| c.entrypoint.clone()).unwrap_or_default();
    let image_cmd = image.and_then(|c| c.cmd.clone()).unwrap_or_default();

    let mut argv: Vec<String> = Vec::new();

    if let Some(ep) = spec.command.entrypoint.as_ref().filter(|e| !e.is_empty()) {
        // Spec overrides the entrypoint; args come from the spec if given, else
        // the image's default Cmd (Docker resets Cmd only when the IMAGE's
        // entrypoint changes — a spec entrypoint override keeps image Cmd).
        argv.extend(ep.iter().cloned());
        match spec.command.args.as_ref() {
            Some(cmd_args) => argv.extend(cmd_args.iter().cloned()),
            None => argv.extend(image_cmd),
        }
    } else if let Some(cmd_args) = spec.command.args.as_ref().filter(|a| !a.is_empty()) {
        // Spec supplies only args (== Docker CMD override): they are appended to
        // the image's Entrypoint (if the image has one).
        argv.extend(image_entrypoint);
        argv.extend(cmd_args.iter().cloned());
    } else {
        // No spec command at all: use the image defaults verbatim.
        argv.extend(image_entrypoint);
        argv.extend(image_cmd);
    }

    if argv.is_empty() {
        return vec!["true".to_string()];
    }
    argv
}

/// Merge the workload environment: the OCI image config's `Env` list provides
/// the base (`PATH`, image-specific vars postgres/forgejo rely on), with the
/// spec's environment layered ON TOP (spec/compose values win for the same key).
/// Returns `(KEY, VALUE)` pairs in a stable order: image vars first (in image
/// order, minus any the spec overrides), then spec vars (in spec order).
pub(crate) fn merge_env(spec: &ServiceSpec, image: Option<&ImageConfig>) -> Vec<(String, String)> {
    // Spec env keys win — collect them first so we can skip image vars they shadow.
    let spec_keys: std::collections::HashSet<&str> = spec.env.keys().map(String::as_str).collect();

    let mut out: Vec<(String, String)> = Vec::new();

    if let Some(img_env) = image.and_then(|c| c.env.as_ref()) {
        for entry in img_env {
            // Image env entries are `KEY=VALUE`; a value may itself contain `=`.
            let (key, value) = match entry.split_once('=') {
                Some((k, v)) => (k.to_string(), v.to_string()),
                None => (entry.clone(), String::new()),
            };
            if !spec_keys.contains(key.as_str()) {
                out.push((key, value));
            }
        }
    }

    for (k, v) in &spec.env {
        out.push((k.clone(), v.clone()));
    }

    out
}

/// Resolve the workload working directory: the spec's workdir wins; otherwise
/// the image config's `WorkingDir` (when non-empty); otherwise `None` (the guest
/// agent defaults to `/`).
pub(crate) fn resolve_workdir(spec: &ServiceSpec, image: Option<&ImageConfig>) -> Option<String> {
    if let Some(w) = spec.command.workdir.as_ref().filter(|w| !w.is_empty()) {
        return Some(w.clone());
    }
    image
        .and_then(|c| c.working_dir.as_ref())
        .filter(|w| !w.is_empty())
        .cloned()
}

/// Resolve the workload `(uid, gid)`: the spec's `user` wins; otherwise the
/// image config's `User`. Both are parsed by [`parse_user`] which honours numeric
/// `uid` / `uid:gid` and falls back to `0:0` for names it cannot resolve on the
/// host (the guest carries its own `/etc/passwd`). A non-root NUMERIC image user
/// is therefore preserved; a NAME-form image user (e.g. `postgres`) currently
/// degrades to `0:0` — documented limitation, not a silent drop of the field.
pub(crate) fn resolve_user(spec: &ServiceSpec, image: Option<&ImageConfig>) -> (u32, u32) {
    let chosen = spec.user.as_deref().filter(|u| !u.is_empty()).or_else(|| {
        image
            .and_then(|c| c.user.as_deref())
            .filter(|u| !u.is_empty())
    });
    parse_user(chosen)
}

/// Parse a Docker-style user value (`"uid"` or `"uid:gid"`) into a `(uid, gid)`
/// pair, defaulting to `(0, 0)`. Non-numeric NAME forms are not resolvable on the
/// host (the guest owns its passwd db), so only numeric ids are honoured; a name
/// falls back to root.
pub(crate) fn parse_user(user: Option<&str>) -> (u32, u32) {
    let Some(user) = user else {
        return (0, 0);
    };
    let mut parts = user.splitn(2, ':');
    let uid = parts
        .next()
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(0);
    let gid = parts
        .next()
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(uid);
    (uid, gid)
}

// ---------------------------------------------------------------------------
// VM lifecycle (all VZ calls happen on the per-VM queue)
// ---------------------------------------------------------------------------

/// The async VZ lifecycle methods that can be driven on a live VM's queue.
#[derive(Clone, Copy)]
pub(crate) enum VmLifecycleOp {
    Start,
    Stop,
    Pause,
    Resume,
}

/// Human-readable message from an `*mut NSError` (null => generic).
fn ns_error_message(err: *mut NSError) -> String {
    if err.is_null() {
        return "unknown VZ error".to_string();
    }
    // SAFETY: caller passes the framework-provided non-null error pointer; we
    // only read its localizedDescription, we do not take ownership.
    let desc = unsafe { (*err).localizedDescription() };
    desc.to_string()
}

/// Run one async VZ lifecycle method (start/stop/pause/resume) on `live`'s
/// queue and block until its completion handler fires. Bridges the framework's
/// `block2` completion to a std channel.
pub(crate) fn run_vm_lifecycle(
    live: &LiveVm,
    op: VmLifecycleOp,
) -> std::result::Result<(), String> {
    let (tx, rx) = std::sync::mpsc::channel::<std::result::Result<(), String>>();
    let vm = Arc::clone(&live.vm);
    live.queue.exec_async(move || {
        let completion = RcBlock::new(move |err: *mut NSError| {
            let r = if err.is_null() {
                Ok(())
            } else {
                Err(ns_error_message(err))
            };
            let _ = tx.send(r);
        });
        // SAFETY: we are on the VM's serial queue, the only place these methods
        // may be invoked. The completion block is heap-allocated (RcBlock) and
        // retained by the framework until it fires.
        unsafe {
            match op {
                VmLifecycleOp::Start => vm.0.startWithCompletionHandler(&completion),
                VmLifecycleOp::Stop => vm.0.stopWithCompletionHandler(&completion),
                VmLifecycleOp::Pause => vm.0.pauseWithCompletionHandler(&completion),
                VmLifecycleOp::Resume => vm.0.resumeWithCompletionHandler(&completion),
            }
        }
    });
    // The completion fires asynchronously on the queue; block the caller for it.
    rx.recv()
        .unwrap_or_else(|_| Err("VZ completion channel closed".to_string()))
}

/// Read a VM's current state on its queue.
pub(crate) fn read_vm_state(live: &LiveVm) -> VZVirtualMachineState {
    let (tx, rx) = std::sync::mpsc::channel::<isize>();
    let vm = Arc::clone(&live.vm);
    live.queue.exec_sync(move || {
        // SAFETY: on the VM's serial queue.
        let s = unsafe { vm.0.state() };
        let _ = tx.send(s.0);
    });
    VZVirtualMachineState(rx.recv().unwrap_or(VZVirtualMachineState::Error.0))
}

// ---------------------------------------------------------------------------
// DHCP lease parsing
// ---------------------------------------------------------------------------

/// Find the IPv4 lease for `mac` in macOS's `dhcpd_leases` file contents.
///
/// The file is a sequence of `{ ... }` stanzas with `hw_address=1,<mac>` and
/// `ip_address=<ip>` lines. The host's NAT DHCP server writes a lease here once
/// the guest requests an address. MAC matching is case-insensitive and tolerant
/// of zero-padding differences (`0a:1:...` vs `0a:01:...`).
pub(crate) fn parse_dhcpd_lease_ip(contents: &str, mac: &str) -> Option<IpAddr> {
    let want = normalize_mac(mac);
    let mut cur_ip: Option<&str> = None;
    let mut cur_mac: Option<String> = None;
    for raw in contents.lines() {
        let line = raw.trim();
        if line == "{" {
            cur_ip = None;
            cur_mac = None;
        } else if let Some(ip) = line.strip_prefix("ip_address=") {
            cur_ip = Some(ip.trim());
        } else if let Some(hw) = line.strip_prefix("hw_address=") {
            // `hw_address=1,0a:1b:2c:3d:4e:5f` — strip the leading `1,` type tag.
            let mac_part = hw.rsplit(',').next().unwrap_or(hw).trim();
            cur_mac = Some(normalize_mac(mac_part));
        } else if line == "}" && cur_mac.as_deref() == Some(want.as_str()) {
            if let Some(ip) = cur_ip.and_then(|s| s.parse::<IpAddr>().ok()) {
                return Some(ip);
            }
        }
    }
    None
}

/// Canonicalize a MAC: lowercase, no zero-padding (`0a:01` -> `a:1`), so two
/// spellings of the same address compare equal.
fn normalize_mac(mac: &str) -> String {
    mac.split(':')
        .map(|octet| {
            let trimmed = octet.trim_start_matches('0');
            let v = if trimmed.is_empty() { "0" } else { trimmed };
            v.to_lowercase()
        })
        .collect::<Vec<_>>()
        .join(":")
}

/// `pub(crate)` test-only view of [`normalize_mac`] so sibling runtimes can
/// compare two MAC spellings for logical equality (case/zero-padding-
/// insensitive) without duplicating the canonicaliser.
#[cfg(test)]
pub(crate) fn normalize_mac_for_test(mac: &str) -> String {
    normalize_mac(mac)
}

/// Read the host's `dhcpd_leases` and return the current IP for `mac`.
pub(crate) async fn current_guest_ip(mac: &str) -> Option<IpAddr> {
    let contents = tokio::fs::read_to_string("/var/db/dhcpd_leases")
        .await
        .ok()?;
    parse_dhcpd_lease_ip(&contents, mac)
}

// ---------------------------------------------------------------------------
// Filesystem / SSH-key helpers
// ---------------------------------------------------------------------------

/// APFS `clonefile` copy-on-write from `src` to `dst`, falling back to a byte
/// copy on any failure (e.g. non-APFS volumes).
pub(crate) fn clone_or_copy(src: &Path, dst: &Path) -> std::io::Result<()> {
    if dst.exists() {
        return Ok(());
    }
    let src_c = std::ffi::CString::new(src.as_os_str().as_encoded_bytes())?;
    let dst_c = std::ffi::CString::new(dst.as_os_str().as_encoded_bytes())?;
    // SAFETY: both paths are valid NUL-terminated C strings; clonefile takes
    // two `const char *` and a flags int.
    let rc = unsafe { libc::clonefile(src_c.as_ptr(), dst_c.as_ptr(), 0) };
    if rc == 0 {
        return Ok(());
    }
    std::fs::copy(src, dst).map(|_| ())
}

/// Generate an ed25519 keypair at `key_path` (+ `.pub`) via `ssh-keygen`.
pub(crate) async fn generate_ssh_keypair(key_path: &Path) {
    if key_path.exists() {
        return;
    }
    let _ = tokio::process::Command::new("ssh-keygen")
        .args(["-t", "ed25519", "-N", "", "-q", "-f"])
        .arg(key_path)
        .status()
        .await;
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_mac_strips_zero_padding_and_case() {
        assert_eq!(normalize_mac("0A:01:00:0F:5d:6E"), "a:1:0:f:5d:6e");
        assert_eq!(
            normalize_mac("0a:1:0:f:5d:6e"),
            normalize_mac("0A:01:00:0F:5D:6E")
        );
    }

    #[test]
    fn parse_memory_units() {
        assert_eq!(parse_memory_to_mib("512Mi"), Some(512));
        assert_eq!(parse_memory_to_mib("2Gi"), Some(2048));
        assert_eq!(parse_memory_to_mib("1048576Ki"), Some(1024));
        assert_eq!(
            parse_memory_to_mib(&(4u64 * 1024 * 1024 * 1024).to_string()),
            Some(4096)
        );
        assert_eq!(parse_memory_to_mib("garbage"), None);
    }

    #[test]
    fn safe_vcpu_clamps_to_at_least_one() {
        assert!(safe_vcpu_count(0) >= 1);
        assert!(safe_vcpu_count(1000) <= u32::try_from(num_cpus::get()).unwrap_or(u32::MAX));
    }

    #[test]
    fn spec_defaults_respect_floor_and_default() {
        let spec = ServiceSpec::minimal("svc", "docker.io/library/alpine:3.19");
        assert_eq!(spec_vcpus(&spec, 2), 2);
        assert_eq!(spec_vcpus(&spec, 4), 4);
        // No memory requested -> default, raised to floor.
        assert_eq!(spec_memory_mib(&spec, 512, 128), 512);
        assert_eq!(spec_memory_mib(&spec, 64, 128), 128);
    }

    #[test]
    fn resolve_entrypoint_falls_back_to_true() {
        let spec = ServiceSpec::minimal("svc", "docker.io/library/alpine:3.19");
        // No spec command AND no image config -> the no-op `true` fallback.
        assert_eq!(resolve_entrypoint(&spec, None), vec!["true".to_string()]);
    }

    fn cfg() -> ImageConfig {
        ImageConfig {
            entrypoint: Some(vec!["docker-entrypoint.sh".to_string()]),
            cmd: Some(vec!["postgres".to_string()]),
            env: Some(vec![
                "PATH=/usr/local/sbin:/usr/local/bin:/usr/bin".to_string(),
                "PG_MAJOR=16".to_string(),
            ]),
            working_dir: Some("/var/lib/postgresql".to_string()),
            user: Some("70:70".to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn resolve_entrypoint_uses_image_entrypoint_and_cmd() {
        // No spec command -> image Entrypoint + image Cmd (the postgres case).
        let spec = ServiceSpec::minimal("svc", "docker.io/library/postgres:16-alpine");
        assert_eq!(
            resolve_entrypoint(&spec, Some(&cfg())),
            vec!["docker-entrypoint.sh".to_string(), "postgres".to_string()]
        );
    }

    #[test]
    fn resolve_entrypoint_spec_args_append_to_image_entrypoint() {
        // Spec args only (Docker CMD override) -> image Entrypoint + spec args.
        let mut spec = ServiceSpec::minimal("svc", "img");
        spec.command.args = Some(vec!["-c".to_string(), "max_connections=200".to_string()]);
        assert_eq!(
            resolve_entrypoint(&spec, Some(&cfg())),
            vec![
                "docker-entrypoint.sh".to_string(),
                "-c".to_string(),
                "max_connections=200".to_string()
            ]
        );
    }

    #[test]
    fn resolve_entrypoint_spec_entrypoint_uses_image_cmd_when_no_args() {
        // Spec entrypoint override, no spec args -> spec entrypoint + image Cmd.
        let mut spec = ServiceSpec::minimal("svc", "img");
        spec.command.entrypoint = Some(vec!["/custom".to_string()]);
        assert_eq!(
            resolve_entrypoint(&spec, Some(&cfg())),
            vec!["/custom".to_string(), "postgres".to_string()]
        );
    }

    #[test]
    fn resolve_entrypoint_spec_entrypoint_and_args_override_both() {
        let mut spec = ServiceSpec::minimal("svc", "img");
        spec.command.entrypoint = Some(vec!["/bin/sh".to_string()]);
        spec.command.args = Some(vec!["-c".to_string(), "echo hi".to_string()]);
        assert_eq!(
            resolve_entrypoint(&spec, Some(&cfg())),
            vec![
                "/bin/sh".to_string(),
                "-c".to_string(),
                "echo hi".to_string()
            ]
        );
    }

    #[test]
    fn merge_env_layers_image_under_spec() {
        let mut spec = ServiceSpec::minimal("svc", "img");
        spec.env
            .insert("PG_MAJOR".to_string(), "override".to_string());
        spec.env
            .insert("POSTGRES_PASSWORD".to_string(), "x".to_string());
        let merged = merge_env(&spec, Some(&cfg()));
        // PATH from image survives (postgres needs it); PG_MAJOR is overridden by
        // the spec; the spec's POSTGRES_PASSWORD is present.
        assert!(merged.contains(&(
            "PATH".to_string(),
            "/usr/local/sbin:/usr/local/bin:/usr/bin".to_string()
        )));
        assert!(merged.contains(&("PG_MAJOR".to_string(), "override".to_string())));
        assert!(merged.contains(&("POSTGRES_PASSWORD".to_string(), "x".to_string())));
        // The image's PG_MAJOR=16 must NOT also be present (spec shadowed it).
        assert!(!merged.contains(&("PG_MAJOR".to_string(), "16".to_string())));
        // Exactly one PG_MAJOR entry.
        assert_eq!(merged.iter().filter(|(k, _)| k == "PG_MAJOR").count(), 1);
    }

    #[test]
    fn resolve_workdir_prefers_spec_then_image() {
        let mut spec = ServiceSpec::minimal("svc", "img");
        assert_eq!(
            resolve_workdir(&spec, Some(&cfg())),
            Some("/var/lib/postgresql".to_string())
        );
        spec.command.workdir = Some("/app".to_string());
        assert_eq!(
            resolve_workdir(&spec, Some(&cfg())),
            Some("/app".to_string())
        );
        // No spec workdir AND no image config -> None (guest defaults to /).
        let bare = ServiceSpec::minimal("svc", "img");
        assert_eq!(resolve_workdir(&bare, None), None);
    }

    #[test]
    fn resolve_user_numeric_from_image_and_spec_override() {
        let mut spec = ServiceSpec::minimal("svc", "img");
        // Numeric image user resolved.
        assert_eq!(resolve_user(&spec, Some(&cfg())), (70, 70));
        // Spec wins.
        spec.user = Some("1000:1001".to_string());
        assert_eq!(resolve_user(&spec, Some(&cfg())), (1000, 1001));
        // No user anywhere -> root.
        let bare = ServiceSpec::minimal("svc", "img");
        assert_eq!(resolve_user(&bare, None), (0, 0));
    }

    #[test]
    fn parse_user_name_falls_back_to_root() {
        // A NAME-form user (e.g. image User "postgres") degrades to root rather
        // than being silently dropped into an undefined uid.
        assert_eq!(parse_user(Some("postgres")), (0, 0));
        assert_eq!(parse_user(Some("1000")), (1000, 1000));
        assert_eq!(parse_user(Some("1000:2000")), (1000, 2000));
    }

    #[test]
    fn dhcpd_lease_lookup_matches_mac() {
        let leases = "\
{
\tname=guest-a
\tip_address=192.168.64.7
\thw_address=1,0a:1b:2c:3d:4e:5f
\tidentifier=1,0a:1b:2c:3d:4e:5f
\tlease=0x600
}
{
\tname=guest-b
\tip_address=192.168.64.9
\thw_address=1,aa:bb:cc:dd:ee:ff
}
";
        // zero-padding-insensitive match
        let ip = parse_dhcpd_lease_ip(leases, "0a:1b:2c:3d:4e:5f").unwrap();
        assert_eq!(ip.to_string(), "192.168.64.7");
        let ip2 = parse_dhcpd_lease_ip(leases, "AA:BB:CC:DD:EE:FF").unwrap();
        assert_eq!(ip2.to_string(), "192.168.64.9");
        assert!(parse_dhcpd_lease_ip(leases, "11:22:33:44:55:66").is_none());
    }
}
