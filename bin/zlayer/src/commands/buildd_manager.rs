//! macOS build-sidecar lifecycle manager.
//!
//! On macOS the host cannot run `buildah` natively, so a Linux Dockerfile
//! build is routed to a `zlayer-buildd` (Go gRPC/buildah server) running
//! inside a VZ-Linux container. This module owns the lifecycle of that
//! container plus the host-side plumbing the build needs:
//!
//! 1. **Ensure running** — probe the sidecar's `Health` RPC on
//!    `127.0.0.1:8099`; if it is not `SERVING`, (re)start the buildd
//!    container (the public `ghcr.io/blackleafdigital/zlayer/buildd:arm64`
//!    image by default, overridable via `$ZLAYER_BUILDD_IMAGE`, falling back
//!    to the local Phase-A `zlayer-buildd-v2:arm64` import when the
//!    configured/ghcr image can't be resolved)
//!    with the gRPC port published to host loopback, the persistent
//!    storage volume, the shared mTLS directory, and a **build-context
//!    bind mount** so build inputs are visible to the in-guest buildah.
//!
//! 2. **Stage context** — copy a build context tree into a unique subdir
//!    of the shared context dir so the in-guest buildah sees it at
//!    `/context/<id>`.
//!
//! 3. **Export** — after the build, `buildah push <tag>
//!    oci-archive:/context/<id>.tar:<tag>` *inside* the buildd container
//!    (via `zlayer exec`) so the image lands as an OCI archive in the
//!    shared dir, ready for `zlayer import` on the host.
//!
//! The whole module is `cfg(target_os = "macos")` — every other host
//! either has native buildah (Linux) or builds Windows images (HCS).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use tracing::{info, warn};

/// Container/deployment name for the managed sidecar.
pub const BUILDD_NAME: &str = "zlayer-buildd";
/// Published, anonymously-pullable OCI image the sidecar runs by default.
///
/// This lives in the same ghcr namespace as `ZLayer`'s other OCI images
/// (`ghcr.io/blackleafdigital/zlayer/*`). When the package is public any macOS
/// host pulls it without credentials; while it is org-`internal` a host needs a
/// ghcr read credential (`zlayer credential registry add --registry ghcr.io`).
/// Either way the [`BUILDD_IMAGE_LOCAL`] fallback keeps a dev Mac working if the
/// remote can't be resolved. The CI workflow `.forgejo/workflows/
/// buildd-image.yml` rebuilds + republishes it.
const BUILDD_IMAGE_GHCR: &str = "ghcr.io/blackleafdigital/zlayer/buildd:arm64";
/// Local dev fallback: the image imported into the local store in Phase A on
/// this build Mac. Used only when the configured/ghcr image can't be resolved
/// (e.g. offline dev with the ghcr image not yet pulled).
const BUILDD_IMAGE_LOCAL: &str = "zlayer-buildd-v2:arm64";
/// Env var to override the buildd image (full ref). Takes precedence over the
/// ghcr default; empty/unset falls back to [`BUILDD_IMAGE_GHCR`].
const BUILDD_IMAGE_ENV: &str = "ZLAYER_BUILDD_IMAGE";

/// The buildd image to start: `$ZLAYER_BUILDD_IMAGE` if set (and non-empty),
/// otherwise the published ghcr default.
fn buildd_image() -> String {
    match std::env::var(BUILDD_IMAGE_ENV) {
        Ok(v) if !v.trim().is_empty() => v.trim().to_string(),
        _ => BUILDD_IMAGE_GHCR.to_string(),
    }
}
/// gRPC bind/publish address on host loopback.
const BUILDD_HOST_ADDR: &str = "127.0.0.1:8099";
/// In-guest gRPC bind address (published to `BUILDD_HOST_ADDR`).
const BUILDD_GUEST_BIND: &str = "0.0.0.0:8099";
/// In-guest mount point for the shared build-context dir.
const GUEST_CONTEXT_DIR: &str = "/context";
/// In-guest mount point for the mTLS material.
const GUEST_TLS_DIR: &str = "/tls";

/// In-guest containers/storage **graph** root. This deliberately lives on the
/// guest's own root filesystem (a RAM-backed tmpfs overlay), NOT on a
/// virtiofs share: macOS's virtiofs does not honor `CAP_DAC_OVERRIDE`, so
/// buildah's vfs layer-apply (`mkdir .pivot_root` inside mode-0555 dirs)
/// fails with EACCES on a virtiofs graph. tmpfs honors `DAC_OVERRIDE` and is
/// sized at ~half the guest RAM — hence the generous [`BUILDD_MEMORY`].
const GUEST_GRAPH_ROOT: &str = "/var/lib/buildd-graph";
/// In-guest containers/storage **run** root (also on tmpfs root fs).
const GUEST_RUN_ROOT: &str = "/var/lib/buildd-run";
/// Guest RAM for the sidecar. The vfs graph is tmpfs-backed off the guest
/// root fs (≈half the RAM), so this also bounds the max build-storage
/// footprint. vfs is space-hungry (a full copy per layer), so a multi-stage
/// golang build (base + build-base + module cache + per-step rw layers + build
/// cache) needs tens of GB — hence 48Gi (→ ≈24 GB usable tmpfs graph). The host
/// is 128 GiB and the build VM is transient. `Gi`/`Mi` suffix is required by
/// the spec's memory validator.
const BUILDD_MEMORY: &str = "48Gi";

/// How long to wait for the sidecar to report `Health = SERVING`.
const HEALTH_TIMEOUT: Duration = Duration::from_secs(60);

/// Resolved handle to a live, context-mounted build sidecar.
#[derive(Debug, Clone)]
pub struct BuilddHandle {
    /// `host:port` the host dials over mTLS.
    pub addr: String,
    /// Host directory holding the mTLS material (`ca/cert/key.pem`).
    pub tls_dir: PathBuf,
    /// Host root of the shared build-context dir (bind-mounted into the
    /// guest at [`GUEST_CONTEXT_DIR`]).
    pub context_host_root: PathBuf,
    /// In-guest mount point of the shared context dir.
    pub context_guest_root: PathBuf,
}

impl BuilddHandle {
    /// The `(host_prefix, guest_prefix)` pair fed to
    /// `SidecarConfig::context_mount` so the backend rewrites context paths
    /// to what the in-guest buildah sees.
    #[must_use]
    pub fn context_mount(&self) -> (PathBuf, PathBuf) {
        (
            self.context_host_root.clone(),
            self.context_guest_root.clone(),
        )
    }
}

/// Resolve the shared buildd dir (`~/.zlayer/buildd`), where the Phase-A
/// mTLS material + storage already live and where we add `context/`.
fn buildd_dir() -> PathBuf {
    zlayer_paths::ZLayerDirs::system_default().buildd()
}

/// Ensure a context-mounted `zlayer-buildd` is up and serving, starting (or
/// restarting) the VZ container as needed.
///
/// Reuses the verified `zlayer run -d` path (Phase A) by shelling out to the
/// current `zlayer` executable, then polls the sidecar's `Health` RPC until
/// it reports `SERVING` (or `HEALTH_TIMEOUT` elapses).
pub async fn ensure_buildd() -> Result<BuilddHandle> {
    let base = buildd_dir();
    let tls_dir = base.clone();
    let context_host_root = base.join("context");

    std::fs::create_dir_all(&context_host_root)
        .with_context(|| format!("creating shared context dir {}", context_host_root.display()))?;

    let handle = BuilddHandle {
        addr: BUILDD_HOST_ADDR.to_string(),
        tls_dir: tls_dir.clone(),
        context_host_root: context_host_root.clone(),
        context_guest_root: PathBuf::from(GUEST_CONTEXT_DIR),
    };

    // A sidecar started before this binary learned about the context mount
    // (e.g. the Phase-A container) is alive but has no `/context`. Detect
    // that and force a recreate so builds can actually see their inputs.
    let serving = probe_health(&handle).await;
    if serving && guest_has_context_mount().await {
        info!(addr = %handle.addr, "reusing running zlayer-buildd (context mount present)");
        ensure_dev_fuse().await;
        return Ok(handle);
    }

    if serving {
        info!("zlayer-buildd is running but lacks the /context mount; recreating");
    } else {
        info!("zlayer-buildd not serving; starting it");
    }

    // Tear down any existing (mount-less or dead) container before starting a
    // fresh one with the context bind mount.
    remove_existing().await;

    // Start with the configured/ghcr image; if that container never reaches
    // SERVING (e.g. the public image can't be pulled in an offline dev env),
    // fall back to the local Phase-A import so this dev Mac keeps working.
    let primary = buildd_image();
    let used_image = match start_and_wait(&handle, &tls_dir, &context_host_root, &primary).await {
        Ok(()) => primary,
        Err(e) if primary != BUILDD_IMAGE_LOCAL => {
            warn!(
                image = %primary,
                fallback = %BUILDD_IMAGE_LOCAL,
                "buildd image failed to come up ({e:#}); retrying with local fallback"
            );
            remove_existing().await;
            start_and_wait(&handle, &tls_dir, &context_host_root, BUILDD_IMAGE_LOCAL)
                .await
                .with_context(|| {
                    format!(
                        "starting zlayer-buildd with fallback image {BUILDD_IMAGE_LOCAL} \
                         (primary {primary} also failed)"
                    )
                })?;
            BUILDD_IMAGE_LOCAL.to_string()
        }
        Err(e) => {
            return Err(e).with_context(|| format!("starting zlayer-buildd from {primary}"));
        }
    };
    info!(image = %used_image, "zlayer-buildd container up");

    // buildah overlay-mounts the build-context dir via fuse-overlayfs, which
    // needs `/dev/fuse`. VZ-Linux containers don't get one by default, so
    // create the device node now. The guest kernel has the `fuse` module
    // loaded (visible in /proc/filesystems); only the node is missing.
    ensure_dev_fuse().await;

    info!(addr = %handle.addr, "zlayer-buildd ready (context-mounted)");
    Ok(handle)
}

/// Path to the running `zlayer` executable (so we shell out to the same
/// build/run code path that's exercising us).
fn zlayer_exe() -> Result<PathBuf> {
    std::env::current_exe().context("resolving current zlayer executable path")
}

/// Start the container from `image` and wait for `Health = SERVING`. On any
/// failure (run failed, or never reached SERVING) the partial container is
/// torn down so the caller can cleanly retry with a different image.
async fn start_and_wait(
    handle: &BuilddHandle,
    tls_dir: &Path,
    context_dir: &Path,
    image: &str,
) -> Result<()> {
    start_container(tls_dir, context_dir, image)
        .await
        .with_context(|| format!("`zlayer run -d` for buildd image {image}"))?;
    if let Err(e) = wait_for_health(handle).await {
        remove_existing().await;
        return Err(e);
    }
    Ok(())
}

/// `zlayer run -d --name zlayer-buildd --memory 8Gi -p ... -v tls -v context
/// <image> -- <buildd flags>`. Mirrors the verified Phase-A command, plus
/// the `/context` bind mount, generous RAM, and a tmpfs-backed graph root
/// (see [`GUEST_GRAPH_ROOT`] for why the graph must not be on virtiofs).
async fn start_container(tls_dir: &Path, context_dir: &Path, image: &str) -> Result<()> {
    let exe = zlayer_exe()?;

    let tls_mount = format!("{}:{GUEST_TLS_DIR}", tls_dir.display());
    let context_mount = format!("{}:{GUEST_CONTEXT_DIR}", context_dir.display());
    let port_pub = format!("{BUILDD_HOST_ADDR}:8099");

    let status = tokio::process::Command::new(&exe)
        .arg("run")
        .arg("-d")
        .arg("--name")
        .arg(BUILDD_NAME)
        .arg("--memory")
        .arg(BUILDD_MEMORY)
        .arg("-p")
        .arg(&port_pub)
        .arg("-v")
        .arg(&tls_mount)
        .arg("-v")
        .arg(&context_mount)
        .arg(image)
        .arg("--")
        .arg("--bind")
        .arg(BUILDD_GUEST_BIND)
        .arg("--tls-ca")
        .arg(format!("{GUEST_TLS_DIR}/ca.pem"))
        .arg("--tls-cert")
        .arg(format!("{GUEST_TLS_DIR}/cert.pem"))
        .arg("--tls-key")
        .arg(format!("{GUEST_TLS_DIR}/key.pem"))
        .arg("--storage-root")
        .arg(GUEST_GRAPH_ROOT)
        .arg("--storage-runroot")
        .arg(GUEST_RUN_ROOT)
        .arg("--storage-driver")
        .arg("vfs")
        .arg("--idle-secs")
        .arg("1800")
        .status()
        .await
        .context("spawning `zlayer run -d` for the buildd sidecar")?;

    if !status.success() {
        bail!("`zlayer run -d {BUILDD_NAME}` exited with {status}");
    }
    Ok(())
}

/// Best-effort teardown of any existing `zlayer-buildd` deployment so a fresh
/// container can claim the name + port. Ignores "not found".
async fn remove_existing() {
    let Ok(exe) = zlayer_exe() else {
        return;
    };
    // `zlayer down <name>` tears down the whole single-service deployment
    // created by `zlayer run -d --name`.
    let out = tokio::process::Command::new(&exe)
        .arg("down")
        .arg(BUILDD_NAME)
        .output()
        .await;
    match out {
        Ok(o) if o.status.success() => {
            info!("removed existing {BUILDD_NAME} deployment");
        }
        Ok(o) => {
            let err = String::from_utf8_lossy(&o.stderr);
            // Not-found is the common, fine case.
            if !err.to_lowercase().contains("not found") && !err.trim().is_empty() {
                warn!("removing existing {BUILDD_NAME}: {}", err.trim());
            }
        }
        Err(e) => warn!("could not invoke deployment rm for {BUILDD_NAME}: {e}"),
    }
    // Give the daemon a beat to release the published port before re-bind.
    tokio::time::sleep(Duration::from_millis(500)).await;
}

/// Prune the buildd's containers/storage (working containers + dangling
/// images) so a reused sidecar doesn't accumulate vfs layers across builds
/// and exhaust the RAM-backed graph. Best-effort.
///
/// vfs keeps a full copy of every layer, so leftover intermediates from a
/// prior build can fill the tmpfs graph. We drop working containers and
/// untagged images but keep tagged base images as a warm cache.
pub async fn prune_build_storage(_handle: &BuilddHandle) {
    let Ok(exe) = zlayer_exe() else {
        return;
    };
    let script = format!(
        "buildah --storage-driver vfs --root {GUEST_GRAPH_ROOT} --runroot {GUEST_RUN_ROOT} rm --all >/dev/null 2>&1; \
         buildah --storage-driver vfs --root {GUEST_GRAPH_ROOT} --runroot {GUEST_RUN_ROOT} rmi --prune --force >/dev/null 2>&1; \
         true",
    );
    let _ = tokio::process::Command::new(&exe)
        .arg("exec")
        .arg(BUILDD_NAME)
        .arg("--")
        .arg("sh")
        .arg("-c")
        .arg(script)
        .output()
        .await;
}

/// Create `/dev/fuse` (char 10:229) inside the buildd container if missing,
/// so buildah's fuse-overlayfs context mount works. Idempotent + best-effort
/// (a pre-existing node makes `mknod` fail harmlessly).
async fn ensure_dev_fuse() {
    let Ok(exe) = zlayer_exe() else {
        return;
    };
    let out = tokio::process::Command::new(&exe)
        .arg("exec")
        .arg(BUILDD_NAME)
        .arg("--")
        .arg("sh")
        .arg("-c")
        .arg("test -e /dev/fuse || mknod -m 666 /dev/fuse c 10 229")
        .output()
        .await;
    match out {
        Ok(o) if o.status.success() => {}
        Ok(o) => warn!(
            "could not ensure /dev/fuse in {BUILDD_NAME}: {}",
            String::from_utf8_lossy(&o.stderr).trim()
        ),
        Err(e) => warn!("exec mknod /dev/fuse failed: {e}"),
    }
}

/// True if the running buildd container has the `/context` virtiofs mount.
async fn guest_has_context_mount() -> bool {
    let Ok(exe) = zlayer_exe() else {
        return false;
    };
    let out = tokio::process::Command::new(&exe)
        .arg("exec")
        .arg(BUILDD_NAME)
        .arg("--")
        .arg("sh")
        .arg("-c")
        .arg(format!("test -d {GUEST_CONTEXT_DIR} && grep -q {GUEST_CONTEXT_DIR} /proc/mounts"))
        .output()
        .await;
    matches!(out, Ok(o) if o.status.success())
}

/// One-shot `Health` probe over mTLS. Returns true only on `SERVING`.
async fn probe_health(handle: &BuilddHandle) -> bool {
    use zlayer_builder::backend::buildah_sidecar::proto::HealthRequest;
    use zlayer_builder::backend::buildah_sidecar::BuildahSidecarBackend;
    use zlayer_types::builder::SidecarConfig;

    let cfg = SidecarConfig {
        addr: Some(handle.addr.clone()),
        tls_dir: Some(handle.tls_dir.clone()),
        ..Default::default()
    };
    let backend = BuildahSidecarBackend::new(cfg);
    let Ok(live) = backend.lifecycle().ensure().await else {
        return false;
    };
    let mut client = live.client();
    match client.health(HealthRequest {}).await {
        Ok(resp) => resp.into_inner().status == 1, // HealthResponse.Status.SERVING
        Err(_) => false,
    }
}

/// Poll `Health` until `SERVING` or timeout.
async fn wait_for_health(handle: &BuilddHandle) -> Result<()> {
    let deadline = Instant::now() + HEALTH_TIMEOUT;
    loop {
        if probe_health(handle).await {
            return Ok(());
        }
        if Instant::now() >= deadline {
            bail!(
                "zlayer-buildd did not report Health=SERVING within {HEALTH_TIMEOUT:?} \
                 (check `zlayer logs --deployment {BUILDD_NAME} {BUILDD_NAME}`)"
            );
        }
        tokio::time::sleep(Duration::from_millis(750)).await;
    }
}

/// Stage `src_context` into a unique subdir of the shared context dir and
/// return `(host_dir, guest_dir)`.
///
/// The copy honors a `.dockerignore` at the context root when present
/// (best-effort prefix/suffix/exact matching); otherwise the whole tree is
/// copied. `.git` is always skipped.
pub fn stage_context(handle: &BuilddHandle, src_context: &Path) -> Result<(PathBuf, PathBuf)> {
    let id = format!("{}-{}", std::process::id(), now_nanos());
    let host_dir = handle.context_host_root.join(&id);
    let guest_dir = handle.context_guest_root.join(&id);

    if host_dir.exists() {
        std::fs::remove_dir_all(&host_dir).ok();
    }
    std::fs::create_dir_all(&host_dir)
        .with_context(|| format!("creating staged context dir {}", host_dir.display()))?;

    let ignore = load_dockerignore(src_context);
    copy_tree(src_context, src_context, &host_dir, &ignore)
        .with_context(|| format!("staging build context from {}", src_context.display()))?;

    Ok((host_dir, guest_dir))
}

/// Remove a previously staged context dir (best effort).
pub fn cleanup_staged(host_dir: &Path) {
    if let Err(e) = std::fs::remove_dir_all(host_dir) {
        warn!(dir = %host_dir.display(), "failed to clean staged context: {e}");
    }
}

/// Export a built image (looked up by `tag` in the buildd's containers
/// storage) to an OCI archive inside the shared context dir, returning the
/// **host** path of the produced tar.
///
/// Runs `buildah push <tag> oci-archive:/context/<file>:<tag>` via `zlayer
/// exec` inside the running buildd container — no skopeo needed, no new
/// proto RPC.
pub async fn export_image(handle: &BuilddHandle, tag: &str) -> Result<PathBuf> {
    let file = format!("export-{}-{}.tar", std::process::id(), now_nanos());
    let host_path = handle.context_host_root.join(&file);
    let guest_path = format!("{GUEST_CONTEXT_DIR}/{file}");
    let dest = format!("oci-archive:{guest_path}:{tag}");

    // The push must read from the same containers/storage the buildd server
    // built into, so pass the identical `--storage-*` flags.
    let exe = zlayer_exe()?;
    let out = tokio::process::Command::new(&exe)
        .arg("exec")
        .arg(BUILDD_NAME)
        .arg("--")
        .arg("buildah")
        .arg("--storage-driver")
        .arg("vfs")
        .arg("--root")
        .arg(GUEST_GRAPH_ROOT)
        .arg("--runroot")
        .arg(GUEST_RUN_ROOT)
        .arg("push")
        .arg(tag)
        .arg(&dest)
        .output()
        .await
        .context("invoking `buildah push` inside zlayer-buildd")?;

    if !out.status.success() {
        bail!(
            "buildah push {tag} -> {dest} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }

    if !host_path.exists() {
        bail!(
            "export tar not found at {} after buildah push (guest path {})",
            host_path.display(),
            guest_path
        );
    }
    Ok(host_path)
}

// ----------------------------------------------------------------------
// helpers
// ----------------------------------------------------------------------

fn now_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

/// Minimal `.dockerignore` patterns (prefix `dir/`, suffix `*.ext`, exact).
struct DockerIgnore {
    patterns: Vec<String>,
}

fn load_dockerignore(ctx: &Path) -> DockerIgnore {
    let mut patterns = Vec::new();
    if let Ok(s) = std::fs::read_to_string(ctx.join(".dockerignore")) {
        for line in s.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with('!') {
                continue;
            }
            patterns.push(line.trim_start_matches("./").to_string());
        }
    }
    DockerIgnore { patterns }
}

fn is_ignored(ignore: &DockerIgnore, rel: &Path) -> bool {
    let rel_str = rel.to_string_lossy().replace('\\', "/");
    for pat in &ignore.patterns {
        let pat = pat.trim_end_matches('/');
        if rel_str == pat
            || rel_str.starts_with(&format!("{pat}/"))
            || pat.strip_prefix('*').is_some_and(|suf| rel_str.ends_with(suf))
        {
            return true;
        }
    }
    false
}

/// Recursively copy `dir` into `dst_root`, preserving structure relative to
/// `ctx_root`, skipping `.git` and `.dockerignore`d paths.
fn copy_tree(ctx_root: &Path, dir: &Path, dst_root: &Path, ignore: &DockerIgnore) -> Result<()> {
    for entry in std::fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        let file_name = entry.file_name();
        if file_name == ".git" {
            continue;
        }
        let rel = path
            .strip_prefix(ctx_root)
            .unwrap_or(&path)
            .to_path_buf();
        if is_ignored(ignore, &rel) {
            continue;
        }
        let dst = dst_root.join(&rel);
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            std::fs::create_dir_all(&dst)
                .with_context(|| format!("mkdir {}", dst.display()))?;
            copy_tree(ctx_root, &path, dst_root, ignore)?;
        } else if file_type.is_symlink() {
            // Copy the link target's bytes (deref) — buildah contexts are
            // file trees; a dangling symlink would just fail the build later.
            if let Ok(target) = std::fs::read_link(&path) {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::symlink;
                    if let Some(parent) = dst.parent() {
                        std::fs::create_dir_all(parent).ok();
                    }
                    let _ = std::fs::remove_file(&dst);
                    symlink(&target, &dst)
                        .with_context(|| format!("symlink {}", dst.display()))?;
                }
            }
        } else {
            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent).ok();
            }
            std::fs::copy(&path, &dst)
                .with_context(|| format!("copy {} -> {}", path.display(), dst.display()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dockerignore_matches_prefix_suffix_exact() {
        let ig = DockerIgnore {
            patterns: vec!["target".into(), "*.log".into(), "secret.txt".into()],
        };
        assert!(is_ignored(&ig, Path::new("target")));
        assert!(is_ignored(&ig, Path::new("target/debug/foo")));
        assert!(is_ignored(&ig, Path::new("build.log")));
        assert!(is_ignored(&ig, Path::new("secret.txt")));
        assert!(!is_ignored(&ig, Path::new("src/main.rs")));
    }

    #[test]
    fn context_mount_pair_roundtrips() {
        let h = BuilddHandle {
            addr: "127.0.0.1:8099".into(),
            tls_dir: PathBuf::from("/tls"),
            context_host_root: PathBuf::from("/host/ctx"),
            context_guest_root: PathBuf::from("/context"),
        };
        let (host, guest) = h.context_mount();
        assert_eq!(host, PathBuf::from("/host/ctx"));
        assert_eq!(guest, PathBuf::from("/context"));
    }
}
