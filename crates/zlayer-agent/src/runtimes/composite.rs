//! Composite runtime that dispatches per-container to a primary + optional delegate.
//!
//! The [`CompositeRuntime`] owns a *primary* runtime (the node-native runtime —
//! e.g. `HcsRuntime` on Windows, `YoukiRuntime` on Linux, Docker elsewhere) and
//! an optional *delegate* runtime used for foreign-OS workloads (e.g. a WSL2
//! delegate on Windows that runs Linux containers). Each call is routed based
//! on the container's identity:
//!
//! * **[`Runtime::create_container`]** consults
//!   [`ServiceSpec::platform`](zlayer_spec::ServiceSpec) first; when the
//!   spec's OS targets the delegate we route there, otherwise primary. When
//!   `platform` is `None`, a secondary **image-OS cache** (populated by
//!   [`CompositeRuntime::record_image_os`] from OCI manifest inspection at
//!   pull time) is consulted. If both are unknown we fall through to the
//!   primary. **Strict policy (H-7):** if either source identifies the
//!   workload as Linux and this node has no delegate configured, dispatch
//!   returns [`AgentError::RouteToPeer`] so the scheduler can re-place the
//!   workload on a Linux peer — the old permissive "fall through to primary"
//!   behavior is gone.
//! * All subsequent per-container operations (start/stop/remove/logs/exec/…)
//!   look up the container in an internal **dispatch cache** that records
//!   which runtime created it. This guarantees the same runtime sees the
//!   container for its whole lifecycle, even after daemon restarts within
//!   the same process.
//! * Cross-cutting image operations (`pull_image`, `pull_image_with_policy`,
//!   `list_images`, `prune_images`) fan out to both runtimes — we cannot know
//!   in advance which runtime will execute a pulled image, and merged image
//!   listings give users a single coherent view. `remove_image` / `tag_image`
//!   try primary first and fall back to delegate.
//!
//! The dispatch cache is populated on `create_container` and cleared on
//! `remove_container`. Looking up an unknown id yields
//! [`AgentError::NotFound`], which surfaces as a clean 404 at the API layer
//! rather than silently forwarding to the wrong runtime.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::RwLock;
use zlayer_observability::logs::{LogEntry, LogStream};
use zlayer_spec::{OsKind, PullPolicy, RegistryAuth, ServiceSpec};

use crate::cgroups_stats::ContainerStats;
use crate::error::{AgentError, Result};
use crate::runtime::{
    ContainerId, ContainerInspectDetails, ContainerState, ExecEventStream, ImageInfo, LogChannel,
    LogChunk, LogsStream, LogsStreamOptions, OverlayAttachKind, PruneResult, Runtime, StatsSample,
    StatsStream, WaitCondition, WaitOutcome,
};

/// Which underlying runtime a given container was dispatched to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DispatchTarget {
    Primary,
    Delegate,
    /// The Apple-Virtualization (VZ) delegate (macOS only). Selected
    /// automatically for `com.zlayer.runtime=vz` base bundles, or per-service
    /// via the `com.zlayer.isolation=vz` label.
    Vz,
    /// The Apple-Virtualization **Linux-guest** delegate (macOS only). The
    /// default Linux path on macOS: selected for Linux images, the
    /// `com.zlayer.runtime=vz-linux` marker, or the
    /// `com.zlayer.isolation=vz-linux` label.
    VzLinux,
}

/// Routes each container to either the primary runtime or an optional delegate.
///
/// See the module-level documentation for the dispatch rules.
pub struct CompositeRuntime {
    primary: Arc<dyn Runtime>,
    delegate: Option<Arc<dyn Runtime>>,
    /// Opt-in Apple-Virtualization delegate (macOS). Selected only when a
    /// service carries `com.zlayer.isolation=vz`.
    vz: Option<Arc<dyn Runtime>>,
    /// Apple-Virtualization Linux-guest delegate (macOS). When present, it is
    /// the default runtime for Linux images on this node; libkrun
    /// (`delegate`) is then reachable only via `com.zlayer.isolation=vm`.
    vz_linux: Option<Arc<dyn Runtime>>,
    /// Per-container dispatch cache. Populated on `create_container`, removed
    /// on `remove_container`.
    dispatch: Arc<RwLock<HashMap<ContainerId, DispatchTarget>>>,
    /// Image-OS cache consulted when a spec has no explicit `platform`.
    /// Populated by [`CompositeRuntime::record_image_os`], which is driven
    /// from [`zlayer_registry::fetch_image_os`] during `pull_image*`.
    image_os: Arc<RwLock<HashMap<String, OsKind>>>,
    /// Image runtime-marker cache (the `com.zlayer.runtime` manifest
    /// annotation, e.g. `"vz"`). Populated from
    /// [`zlayer_registry::fetch_image_runtime_marker`] during `pull_image*` so
    /// `select_for` can auto-detect a VZ base bundle and prefer the VZ runtime
    /// for it without requiring a per-service label.
    image_runtime: Arc<RwLock<HashMap<String, String>>>,
    /// Filesystem paths of the persistent blob caches that the runtimes pull
    /// into, tried IN ORDER for image-OS / runtime-marker inspection. Typically:
    ///
    /// 1. the VZ-Linux runtime's `{data_dir}/vz/linux/images/blobs.redb` (the
    ///    delegate that actually runs the Linux workload), and
    /// 2. the primary Sandbox runtime's `{data_dir}/images/blobs.redb`.
    ///
    /// Both stores matter because `pull_image` pulls into BOTH (primary first,
    /// then VZ-Linux), and either pull may short-circuit under
    /// `PullPolicy::IfNotPresent` when its rootfs already exists — leaving the
    /// manifest/config in only ONE of the two caches. Inspection therefore
    /// probes them in order and stops at the first store that resolves the OS,
    /// LOCAL-ONLY via [`zlayer_registry::fetch_image_os_in_cache_only`] — so an
    /// already-pulled Linux image is detected as Linux (and routed to VZ-Linux)
    /// with NO network call, even under a Docker Hub rate-limit. For the OS
    /// dispatch path there is intentionally **no** network fallback: a local
    /// miss yields "OS unknown" and dispatch uses its safe macOS default rather
    /// than risking a 429 (see [`CompositeRuntime::inspect_image_os`]).
    os_inspect_cache_paths: Vec<std::path::PathBuf>,
}

impl CompositeRuntime {
    /// Construct a new composite runtime.
    ///
    /// `primary` handles containers whose platform matches the host node.
    /// `delegate`, when present, handles foreign-OS containers (currently:
    /// Linux containers on a Windows host via the WSL2 delegate runtime).
    #[must_use]
    pub fn new(primary: Arc<dyn Runtime>, delegate: Option<Arc<dyn Runtime>>) -> Self {
        Self {
            primary,
            delegate,
            vz: None,
            vz_linux: None,
            dispatch: Arc::new(RwLock::new(HashMap::new())),
            image_os: Arc::new(RwLock::new(HashMap::new())),
            image_runtime: Arc::new(RwLock::new(HashMap::new())),
            os_inspect_cache_paths: Vec::new(),
        }
    }

    /// Point image-OS / runtime-marker inspection at a single persistent blob
    /// cache the runtimes pull into, so the OS of an already-pulled image
    /// resolves from the LOCAL config blob with no network round-trip.
    ///
    /// Convenience wrapper over [`CompositeRuntime::with_os_inspect_cache_paths`]
    /// for callers that only have one store. `path` is the on-disk blob-cache
    /// file (e.g. the VZ-Linux runtime's `{data_dir}/vz/linux/images/blobs.redb`).
    #[must_use]
    pub fn with_os_inspect_cache_path(self, path: Option<std::path::PathBuf>) -> Self {
        self.with_os_inspect_cache_paths(path.into_iter().collect())
    }

    /// Point image-OS / runtime-marker inspection at an ORDERED list of
    /// persistent blob caches the runtimes pull into.
    ///
    /// Inspection probes each store LOCAL-ONLY (no network) in order and stops
    /// at the first that resolves the image's OS / marker. This matters because
    /// `pull_image` pulls into BOTH the VZ-Linux store and the primary Sandbox
    /// store, and either pull may short-circuit under `PullPolicy::IfNotPresent`
    /// when its rootfs already exists — leaving the manifest/config in only ONE
    /// of the two caches. Probing both (VZ-Linux first, then primary) is what
    /// lets a locally-cached Linux image route to VZ-Linux under a Docker Hub
    /// rate-limit (see [`zlayer_registry::fetch_image_os_in_cache_only`]).
    #[must_use]
    pub fn with_os_inspect_cache_paths(mut self, paths: Vec<std::path::PathBuf>) -> Self {
        self.os_inspect_cache_paths = paths;
        self
    }

    /// Resolve `image`'s OS for **dispatch**, probing each configured local blob
    /// cache in order, **LOCAL-ONLY — never a network call**.
    ///
    /// This is the dispatch-population path: it runs inside `pull_image*` purely
    /// to fill the image-OS cache that [`CompositeRuntime::select_for`] consults.
    /// It MUST NOT touch the wire. The image's layers have already been pulled
    /// and extracted by the time we get here, and the runtimes that did the pull
    /// (VZ-Linux / Sandbox) wrote the manifest+config into the very blob caches
    /// `os_inspect_cache_paths` points at — so the OS is knowable with zero
    /// network round-trips.
    ///
    /// The old code fell back to a network inspection (`fetch_image_os`) when no
    /// local cache resolved the OS. That network call was reachable on a Docker
    /// Hub 429, and a failed inspection left the cache empty → a cached Linux
    /// image (e.g. `alpine`) fell through to the Seatbelt sandbox (`Primary`),
    /// which cannot exec a Linux ELF (exit 127). The network fallback is gone:
    /// a registry rate-limit can no longer break dispatch here. A genuine local
    /// miss simply returns `Ok(None)` (dispatch then uses its safe macOS
    /// fallthrough), and it never errors or blocks.
    async fn inspect_image_os(
        &self,
        image: &str,
    ) -> std::result::Result<Option<OsKind>, zlayer_registry::RegistryError> {
        for path in &self.os_inspect_cache_paths {
            match zlayer_registry::CacheType::persistent_at(path)
                .build()
                .await
            {
                Ok(cache) => {
                    match zlayer_registry::fetch_image_os_in_cache_only(image, cache, None).await {
                        Ok(Some(os)) => return Ok(Some(os)),
                        Ok(None) => {
                            tracing::trace!(
                                image,
                                cache = %path.display(),
                                "image OS not resolvable from this local cache; trying next",
                            );
                        }
                        Err(e) => return Err(e),
                    }
                }
                Err(e) => {
                    tracing::debug!(
                        image,
                        cache = %path.display(),
                        error = %e,
                        "failed to open OS-inspect blob cache; trying next",
                    );
                }
            }
        }
        // No local cache resolved it. We deliberately do NOT fall back to a
        // network inspection: a Docker Hub 429 must never reach this
        // dispatch-population path (see the doc comment above). A clean local
        // miss is `Ok(None)` — dispatch falls through to its safe macOS default.
        Ok(None)
    }

    /// Resolve `image`'s `com.zlayer.runtime` marker, probing each configured
    /// local blob cache in order (no network per cache) before any network call.
    async fn inspect_image_runtime_marker(
        &self,
        image: &str,
        auth: Option<&RegistryAuth>,
    ) -> std::result::Result<Option<String>, zlayer_registry::RegistryError> {
        for path in &self.os_inspect_cache_paths {
            match zlayer_registry::CacheType::persistent_at(path)
                .build()
                .await
            {
                Ok(cache) => {
                    match zlayer_registry::fetch_image_runtime_marker_in_cache_only(
                        image, cache, None,
                    )
                    .await
                    {
                        Ok(Some(marker)) => return Ok(Some(marker)),
                        Ok(None) => {
                            tracing::trace!(
                                image,
                                cache = %path.display(),
                                "runtime marker not resolvable from this local cache; trying next",
                            );
                        }
                        Err(e) => return Err(e),
                    }
                }
                Err(e) => {
                    tracing::debug!(
                        image,
                        cache = %path.display(),
                        error = %e,
                        "failed to open marker-inspect blob cache; trying next",
                    );
                }
            }
        }
        zlayer_registry::fetch_image_runtime_marker(image, auth).await
    }

    /// Attach an opt-in Apple-Virtualization delegate. Services labelled
    /// `com.zlayer.isolation=vz` route to it; everything else is unaffected.
    #[must_use]
    pub fn with_vz_delegate(mut self, vz: Option<Arc<dyn Runtime>>) -> Self {
        self.vz = vz;
        self
    }

    /// Attach the Apple-Virtualization Linux-guest delegate. When present it
    /// becomes the **default** runtime for Linux images on this node (libkrun
    /// is then reachable only via the explicit `com.zlayer.isolation=vm`
    /// label); when `None`, Linux dispatch falls back to the libkrun delegate
    /// or `RouteToPeer` as before.
    #[must_use]
    pub fn with_vz_linux_delegate(mut self, vz_linux: Option<Arc<dyn Runtime>>) -> Self {
        self.vz_linux = vz_linux;
        self
    }

    /// Access the primary runtime (for introspection / tests).
    #[must_use]
    pub fn primary(&self) -> &Arc<dyn Runtime> {
        &self.primary
    }

    /// Access the delegate runtime, if one is configured.
    #[must_use]
    pub fn delegate(&self) -> Option<&Arc<dyn Runtime>> {
        self.delegate.as_ref()
    }

    /// Record that `image` is known to target operating system `os`.
    ///
    /// Wired from [`zlayer_registry::fetch_image_os`] during `pull_image*`
    /// (see [`CompositeRuntime::apply_image_os_inspection`]) so that specs
    /// without an explicit `platform` still dispatch correctly.
    pub(crate) async fn record_image_os(&self, image: &str, os: OsKind) {
        self.image_os.write().await.insert(image.to_string(), os);
    }

    /// Record an image's `com.zlayer.runtime` marker (e.g. `"vz"`), used by
    /// [`CompositeRuntime::select_for`] to auto-detect a runtime-specific bundle.
    pub(crate) async fn record_image_runtime(&self, image: &str, marker: String) {
        self.image_runtime
            .write()
            .await
            .insert(image.to_string(), marker);
    }

    /// Apply a manifest runtime-marker inspection to the cache. Mirrors
    /// [`CompositeRuntime::apply_image_os_inspection`]'s non-fatal contract:
    /// only a present marker updates the cache; absence or error leaves it
    /// untouched (dispatch falls through to the OS/platform rules).
    async fn apply_image_runtime_inspection(
        &self,
        image: &str,
        result: std::result::Result<Option<String>, zlayer_registry::RegistryError>,
    ) {
        match result {
            Ok(Some(marker)) => {
                tracing::debug!(image, marker, "cached image runtime marker for dispatch");
                self.record_image_runtime(image, marker).await;
            }
            Ok(None) => {}
            Err(e) => {
                tracing::trace!(
                    image,
                    error = %e,
                    "failed to inspect image runtime marker — dispatch unaffected",
                );
            }
        }
    }

    /// Apply the result of a manifest OS inspection to the image-OS cache.
    ///
    /// Factored out of [`Runtime::pull_image`] and
    /// [`Runtime::pull_image_with_policy`] so the cache-update policy can be
    /// unit-tested without depending on a live registry. The three branches
    /// mirror the contract of [`zlayer_registry::fetch_image_os`]:
    ///
    /// * `Ok(Some(os))` — populate the cache so future `create_container`
    ///   calls without an explicit `spec.platform` dispatch to the right
    ///   runtime.
    /// * `Ok(None)` — the config blob had no (or an unrecognized) `os`
    ///   field. Leave the cache untouched; dispatch falls through to primary.
    /// * `Err(_)` — transient or registry error. Log at warn and leave the
    ///   cache untouched. We never fail the overall `pull_image*` call on
    ///   inspection failure: the primary runtime's own pull already
    ///   succeeded, and the safe fall-through is "primary".
    async fn apply_image_os_inspection(
        &self,
        image: &str,
        result: std::result::Result<Option<OsKind>, zlayer_registry::RegistryError>,
    ) {
        match result {
            Ok(Some(os)) => {
                self.record_image_os(image, os).await;
                tracing::debug!(image, ?os, "cached image OS for dispatch");
            }
            Ok(None) => {
                tracing::trace!(
                    image,
                    "image manifest has no OS field — dispatch will fall through to primary",
                );
            }
            Err(e) => {
                tracing::warn!(
                    image,
                    error = %e,
                    "failed to inspect image manifest OS — dispatch will fall through to primary",
                );
            }
        }
    }

    /// Decide which runtime should handle a `create_container` call for `spec`.
    ///
    /// The `service` argument is the originating service name, used to build an
    /// actionable [`AgentError::RouteToPeer`] when a Linux workload lands on
    /// this node without a local delegate so the scheduler can re-place it on
    /// a capable peer.
    ///
    /// Policy (H-7): Linux workloads are never silently routed to the primary
    /// on nodes without a delegate. The old "permissive" fall-through (primary
    /// handles everything) returned an `Unsupported` error only when
    /// `spec.platform` was explicitly set, but fell through to primary for
    /// specs without a platform — producing cryptic downstream errors when the
    /// image-OS cache said `Linux`. We now return `RouteToPeer` in both cases.
    ///
    /// Routing precedence, locally-known OS only (NO network call ever happens
    /// here — the image-OS cache is filled local-only at pull time):
    /// 1. explicit `com.zlayer.isolation` label,
    /// 2. `com.zlayer.runtime` manifest marker (`vz` / `vz-linux`),
    /// 3. `spec.platform.os`,
    /// 4. the image-OS cache: `Linux` -> `VzLinux` (when present), `Macos` /
    ///    `Windows` -> `Primary`,
    /// 5. FINAL fallthrough — OS genuinely unknown: on a macOS host (proxied by
    ///    the presence of a `vz_linux` delegate) default to `VzLinux`, because
    ///    almost every registry image is Linux and the Seatbelt sandbox cannot
    ///    exec a Linux ELF. A macOS-native rootfs never reaches this branch: it
    ///    resolves `os == Macos` at step 4 and routes to `Primary`.
    async fn select_for(&self, service: &str, spec: &ServiceSpec) -> Result<DispatchTarget> {
        // Explicit per-service isolation label wins over everything below.
        //   `com.zlayer.isolation=vz`               -> VZ (native-macOS guest VM)
        //   `com.zlayer.isolation=vz-linux`         -> VZ Linux-guest VM
        //   `com.zlayer.isolation=vm|libkrun`       -> libkrun delegate (force VM)
        //   `com.zlayer.isolation=sandbox|seatbelt` -> Seatbelt sandbox (primary),
        //                                              opting OUT of VZ auto-detect.
        if let Some(label) = spec.labels.get("com.zlayer.isolation") {
            if self.vz.is_some() && label.eq_ignore_ascii_case("vz") {
                return Ok(DispatchTarget::Vz);
            }
            if self.vz_linux.is_some() && label.eq_ignore_ascii_case("vz-linux") {
                return Ok(DispatchTarget::VzLinux);
            }
            if label.eq_ignore_ascii_case("vm") || label.eq_ignore_ascii_case("libkrun") {
                // Force the libkrun delegate. If no delegate exists the
                // platform/image-OS rules below produce the appropriate
                // `RouteToPeer`, so only short-circuit when one is present.
                if self.delegate.is_some() {
                    return Ok(DispatchTarget::Delegate);
                }
            }
            if label.eq_ignore_ascii_case("sandbox") || label.eq_ignore_ascii_case("seatbelt") {
                return Ok(DispatchTarget::Primary);
            }
        }

        // Auto-detect a VZ base bundle: when the image's manifest carries
        // `com.zlayer.runtime=vz` (stamped by `zlayer vz build-base`), prefer the
        // VZ runtime — it is the only runtime that can boot such a bundle. This
        // is the "prefer VZ by default" behaviour: it only fires for genuine VZ
        // bundles, so Seatbelt-rootfs and Linux images are unaffected.
        if self.vz.is_some()
            && self
                .image_runtime
                .read()
                .await
                .get(&spec.image.name.to_string())
                .is_some_and(|m| m.eq_ignore_ascii_case(zlayer_registry::ZLAYER_RUNTIME_VZ))
        {
            return Ok(DispatchTarget::Vz);
        }

        // Auto-detect a VZ Linux-guest image: when the manifest carries
        // `com.zlayer.runtime=vz-linux`, prefer the VZ Linux runtime.
        if self.vz_linux.is_some()
            && self
                .image_runtime
                .read()
                .await
                .get(&spec.image.name.to_string())
                .is_some_and(|m| m.eq_ignore_ascii_case(zlayer_registry::ZLAYER_RUNTIME_LINUX_VZ))
        {
            return Ok(DispatchTarget::VzLinux);
        }

        if let Some(platform) = &spec.platform {
            let target = match platform.os {
                OsKind::Windows | OsKind::Macos => DispatchTarget::Primary,
                // On macOS the VZ Linux-guest runtime is the default Linux path;
                // only fall back to the libkrun delegate when it is absent.
                OsKind::Linux if self.vz_linux.is_some() => DispatchTarget::VzLinux,
                OsKind::Linux => DispatchTarget::Delegate,
            };
            if matches!(target, DispatchTarget::Delegate) && self.delegate.is_none() {
                return Err(AgentError::RouteToPeer {
                    service: service.to_string(),
                    required_os: OsKind::Linux.as_oci_str().to_string(),
                    reason: "spec.platform.os = linux but this node has no WSL2 delegate \
                            configured; enable `--install-wsl yes` on this node or add a Linux \
                            peer to the cluster"
                        .to_string(),
                });
            }
            return Ok(target);
        }

        if let Some(os) = self
            .image_os
            .read()
            .await
            .get(&spec.image.name.to_string())
            .copied()
        {
            return match os {
                OsKind::Linux => {
                    if self.vz_linux.is_some() {
                        // VZ Linux-guest is the default Linux path on macOS.
                        Ok(DispatchTarget::VzLinux)
                    } else if self.delegate.is_some() {
                        Ok(DispatchTarget::Delegate)
                    } else {
                        // No delegate and the image manifest says Linux —
                        // refuse at the composite layer so the scheduler can
                        // re-place on a Linux peer instead of the primary
                        // failing with a cryptic HCS error.
                        Err(AgentError::RouteToPeer {
                            service: service.to_string(),
                            required_os: OsKind::Linux.as_oci_str().to_string(),
                            reason: format!(
                                "image '{}' manifest reports os=linux but this node has no WSL2 \
                                 delegate configured; enable `--install-wsl yes` on this node or \
                                 add a Linux peer to the cluster",
                                spec.image.name
                            ),
                        })
                    }
                }
                OsKind::Windows | OsKind::Macos => Ok(DispatchTarget::Primary),
            };
        }

        // OS genuinely unknown (no isolation label, no runtime marker, no
        // `spec.platform`, no image-OS cache hit). On a macOS host with a
        // VZ-Linux delegate, default to VZ-Linux: the overwhelming majority of
        // images pulled from public registries are Linux, and the Seatbelt
        // sandbox (the primary) cannot exec a Linux ELF — sending an unknown
        // image there is the exit-127 failure this fix exists to prevent. The
        // user is fine with VZ-Linux as the default; the only hard rule is that
        // a macOS-native rootfs must never go to the Linux VM, and that is
        // already guaranteed above by the `image_os == Macos -> Primary` branch
        // (a native bundle resolves its OS locally and never reaches here).
        //
        // The `vz_linux` delegate is only ever attached on a macOS host, so its
        // presence is a sufficient proxy for "macOS host" — non-macOS hosts
        // (Windows HCS, Linux) keep the historical primary fallthrough.
        if self.vz_linux.is_some() {
            return Ok(DispatchTarget::VzLinux);
        }

        Ok(DispatchTarget::Primary)
    }

    /// Look up an existing dispatch decision for `id`, or return `NotFound`.
    async fn lookup(&self, id: &ContainerId) -> Result<Arc<dyn Runtime>> {
        let target =
            self.dispatch
                .read()
                .await
                .get(id)
                .copied()
                .ok_or_else(|| AgentError::NotFound {
                    container: id.to_string(),
                    reason: "no dispatch record in CompositeRuntime".to_string(),
                })?;
        Ok(self.runtime_for(target).clone())
    }

    /// Resolve a [`DispatchTarget`] to the concrete runtime reference.
    ///
    /// Unwrapping the delegate is safe because [`Self::select_for`] returns
    /// `Err` whenever a delegate would be required but is missing, so a
    /// `DispatchTarget::Delegate` can never end up in the dispatch map
    /// without a delegate being present.
    fn runtime_for(&self, t: DispatchTarget) -> &Arc<dyn Runtime> {
        match t {
            DispatchTarget::Primary => &self.primary,
            DispatchTarget::Delegate => self
                .delegate
                .as_ref()
                .expect("delegate target requires delegate to exist"),
            // `select_for` only returns `Vz` when a vz delegate is present;
            // fall back to primary defensively.
            DispatchTarget::Vz => self.vz.as_ref().unwrap_or(&self.primary),
            // `select_for` only returns `VzLinux` when a vz-linux delegate is
            // present; fall back to primary defensively.
            DispatchTarget::VzLinux => self.vz_linux.as_ref().unwrap_or(&self.primary),
        }
    }

    /// Build the ordered list of backends to try for a per-container read
    /// (logs / stats), owning backend first.
    ///
    /// The container's dispatch record (recorded at `create_container`) names
    /// the runtime that actually ran it, so we try that one first. The other
    /// configured backends follow as a defensive fallback for the case where
    /// the owning backend can answer container lifecycle calls but not a
    /// particular read (e.g. the macOS `SandboxRuntime` primary implements
    /// `container_logs`/`get_container_stats` but not the *streaming*
    /// `logs_stream`/`stats_stream`, so it returns `Unsupported` for the
    /// latter). Returns `NotFound` when the id was never dispatched.
    async fn read_backends(
        &self,
        id: &ContainerId,
    ) -> Result<Vec<(&'static str, Arc<dyn Runtime>)>> {
        let owner =
            self.dispatch
                .read()
                .await
                .get(id)
                .copied()
                .ok_or_else(|| AgentError::NotFound {
                    container: id.to_string(),
                    reason: "no dispatch record in CompositeRuntime".to_string(),
                })?;

        // Owning backend first, then every other configured backend (de-duped
        // against the owner) so a read the owner can't serve can still be
        // satisfied elsewhere instead of 500-ing.
        let all: [(DispatchTarget, Option<&Arc<dyn Runtime>>); 4] = [
            (DispatchTarget::Primary, Some(&self.primary)),
            (DispatchTarget::Delegate, self.delegate.as_ref()),
            (DispatchTarget::Vz, self.vz.as_ref()),
            (DispatchTarget::VzLinux, self.vz_linux.as_ref()),
        ];

        let label_for = |t: DispatchTarget| match t {
            DispatchTarget::Primary => "primary",
            DispatchTarget::Delegate => "delegate",
            DispatchTarget::Vz => "vz",
            DispatchTarget::VzLinux => "vz_linux",
        };

        let mut out: Vec<(&'static str, Arc<dyn Runtime>)> =
            vec![(label_for(owner), self.runtime_for(owner).clone())];
        for (target, rt) in all {
            if target != owner {
                if let Some(rt) = rt {
                    out.push((label_for(target), rt.clone()));
                }
            }
        }
        Ok(out)
    }
}

/// Accumulates per-backend errors while a read fans out across the
/// owner-first fallback chain, so the *final* error reflects the right HTTP
/// status.
///
/// Every backend in the chain is tried; a backend that does not own the
/// container returns [`AgentError::NotFound`] (a *skip*, not authoritative),
/// while a backend that owns it but cannot serve this particular read returns
/// some other error (notably the `Unsupported` default for an unimplemented
/// streaming read) — a *soft miss* we fall back from. The distinction matters
/// for the final error: if **every** backend returned `NotFound`, the container
/// genuinely does not exist here and we surface `NotFound` (→ 404); if any
/// backend produced a non-`NotFound` error, that is the more informative
/// failure to report (→ 500) once no backend could serve the read.
#[derive(Default)]
struct ReadMissAccumulator {
    /// The most recent non-`NotFound` error, if any backend produced one.
    soft_err: Option<AgentError>,
    /// The most recent `NotFound`, used only when *no* soft error occurred.
    not_found: Option<AgentError>,
}

impl ReadMissAccumulator {
    fn record(&mut self, e: AgentError) {
        if matches!(e, AgentError::NotFound { .. }) {
            self.not_found = Some(e);
        } else {
            self.soft_err = Some(e);
        }
    }

    /// Resolve the accumulated misses into the final error for a read where no
    /// backend succeeded. Prefers a soft error (more informative → 500) over a
    /// bare `NotFound`; falls back to a synthesised `Unsupported` only if
    /// nothing was recorded at all (an empty backend list, which cannot happen
    /// in practice since the owner is always present).
    fn into_error(self, what: &str) -> AgentError {
        self.soft_err
            .or(self.not_found)
            .unwrap_or_else(|| AgentError::Unsupported(format!("no backend could serve {what}")))
    }
}

/// Build a bounded one-shot [`LogsStream`] from a captured-log snapshot.
///
/// Used by [`CompositeRuntime::logs_stream`] when no backend offers a native
/// log stream but one can produce a `container_logs` snapshot (e.g. the macOS
/// `SandboxRuntime`). Mirrors the VZ-Linux runtime's own snapshot-to-stream
/// translation so the wire shape is identical regardless of which backend
/// served the data: honour the per-channel filters and re-attach the newline
/// the line-splitter stripped.
fn one_shot_logs_stream(entries: Vec<LogEntry>, opts: &LogsStreamOptions) -> LogsStream {
    use futures_util::stream;

    // Docker's default (neither stdout nor stderr explicitly requested) means
    // "both"; equivalently, keep stdout unless stderr was the *only* channel
    // requested, and vice-versa.
    let want_stdout = opts.stdout || !opts.stderr;
    let want_stderr = opts.stderr || !opts.stdout;
    let timestamps = opts.timestamps;

    let chunks: Vec<Result<LogChunk>> = entries
        .into_iter()
        .filter_map(|e| {
            let channel = match e.stream {
                LogStream::Stdout => LogChannel::Stdout,
                LogStream::Stderr => LogChannel::Stderr,
            };
            let keep = match channel {
                LogChannel::Stdout => want_stdout,
                LogChannel::Stderr => want_stderr,
                LogChannel::Stdin => false,
            };
            if !keep {
                return None;
            }
            let mut bytes = e.message.into_bytes();
            bytes.push(b'\n');
            Some(Ok(LogChunk {
                stream: channel,
                bytes: bytes::Bytes::from(bytes),
                timestamp: timestamps.then_some(e.timestamp),
            }))
        })
        .collect();

    Box::pin(stream::iter(chunks))
}

/// Build a bounded one-shot [`StatsStream`] from a single [`ContainerStats`]
/// snapshot.
///
/// Used by [`CompositeRuntime::stats_stream`] when no backend offers a native
/// stats stream but one can produce a `get_container_stats` snapshot. The
/// [`ContainerStats`] CPU figure is microseconds; [`StatsSample::cpu_total_ns`]
/// is nanoseconds, so we scale. `online_cpus` is unknown from this coarse
/// snapshot (the non-streaming API does not carry it) and is reported as `1`
/// so the Docker-compat CPU-percent math has a sane divisor.
fn one_shot_stats_stream(stats: &ContainerStats) -> StatsStream {
    use futures_util::stream;

    let sample = StatsSample {
        cpu_total_ns: stats.cpu_usage_usec.saturating_mul(1_000),
        cpu_system_ns: 0,
        online_cpus: 1,
        mem_used_bytes: stats.memory_bytes,
        mem_limit_bytes: stats.memory_limit,
        net_rx_bytes: 0,
        net_tx_bytes: 0,
        blkio_read_bytes: 0,
        blkio_write_bytes: 0,
        pids_current: 0,
        pids_limit: None,
        timestamp: chrono::Utc::now(),
    };
    Box::pin(stream::iter(vec![Ok(sample)]))
}

#[async_trait]
impl Runtime for CompositeRuntime {
    async fn pull_image(&self, image: &str) -> Result<()> {
        // Primary pull. `WrongPlatform` here means the image's OCI config
        // reports an OS the primary cannot service (e.g. a Linux image on the
        // Windows HCS runtime). That is a *soft* failure: the delegate's pull
        // below owns the image, so we log and continue rather than failing
        // the whole composite call. Any other error is a real pull failure
        // and must bubble.
        if let Err(e) = self.primary.pull_image(image).await {
            if matches!(e, AgentError::WrongPlatform { .. }) {
                tracing::debug!(
                    image,
                    error = %e,
                    "primary runtime cannot service image (wrong platform); delegating",
                );
            } else {
                return Err(e);
            }
        }
        if let Some(delegate) = &self.delegate {
            if let Err(e) = delegate.pull_image(image).await {
                // Foreign-OS images will reliably fail one of the two pulls
                // (primary can't store a Linux image's config on Windows, or
                // vice versa). That's expected — the successful side owns the
                // layers we'll actually use — so we keep this at debug.
                tracing::debug!(
                    image,
                    error = %e,
                    "delegate runtime failed to pull image (likely wrong OS); continuing with primary result",
                );
            }
        }
        // VZ + VZ-Linux delegates (macOS). The VZ-Linux runtime is the default
        // execution path for Linux images on macOS and owns its OWN image store
        // (`image_rootfs`); if we never pull into it, the image is absent both
        // when `create_container` dispatches there AND from `list_images` /
        // `inspect_image` (which is what `docker pull` verifies). Pulling here
        // makes the image actually present where it runs and listable. Errors
        // are non-fatal for the same wrong-OS reason as the delegate above.
        for (label, rt) in [
            self.vz.as_ref().map(|r| ("vz", r)),
            self.vz_linux.as_ref().map(|r| ("vz_linux", r)),
        ]
        .into_iter()
        .flatten()
        {
            if let Err(e) = rt.pull_image(image).await {
                tracing::debug!(
                    image,
                    runtime = label,
                    error = %e,
                    "vz delegate failed to pull image (likely wrong OS); continuing",
                );
            }
        }

        // Inspect the OCI manifest's `config.os` so `select_for(spec)` can
        // dispatch correctly when `spec.platform` is `None`. Non-fatal: any
        // failure here just means dispatch falls through to primary.
        let os_result = self.inspect_image_os(image).await;
        self.apply_image_os_inspection(image, os_result).await;
        let marker_result = self.inspect_image_runtime_marker(image, None).await;
        self.apply_image_runtime_inspection(image, marker_result)
            .await;

        Ok(())
    }

    async fn pull_image_with_policy(
        &self,
        image: &str,
        policy: PullPolicy,
        auth: Option<&RegistryAuth>,
        source: zlayer_spec::SourcePolicy,
    ) -> Result<()> {
        // See `pull_image` above for the `WrongPlatform` soft-skip rationale.
        if let Err(e) = self
            .primary
            .pull_image_with_policy(image, policy, auth, source)
            .await
        {
            if matches!(e, AgentError::WrongPlatform { .. }) {
                tracing::debug!(
                    image,
                    error = %e,
                    "primary runtime cannot service image (wrong platform); delegating",
                );
            } else {
                return Err(e);
            }
        }
        if let Some(delegate) = &self.delegate {
            if let Err(e) = delegate
                .pull_image_with_policy(image, policy, auth, source)
                .await
            {
                tracing::debug!(
                    image,
                    error = %e,
                    "delegate runtime failed to pull image (likely wrong OS); continuing with primary result",
                );
            }
        }
        // See `pull_image` above: the VZ-Linux runtime owns its own image store
        // and is the default Linux execution path on macOS, so pull into it (and
        // the opt-in VZ delegate) too. Non-fatal per-backend errors.
        for (label, rt) in [
            self.vz.as_ref().map(|r| ("vz", r)),
            self.vz_linux.as_ref().map(|r| ("vz_linux", r)),
        ]
        .into_iter()
        .flatten()
        {
            if let Err(e) = rt.pull_image_with_policy(image, policy, auth, source).await {
                tracing::debug!(
                    image,
                    runtime = label,
                    error = %e,
                    "vz delegate failed to pull image (likely wrong OS); continuing",
                );
            }
        }

        let os_result = self.inspect_image_os(image).await;
        self.apply_image_os_inspection(image, os_result).await;
        let marker_result = self.inspect_image_runtime_marker(image, auth).await;
        self.apply_image_runtime_inspection(image, marker_result)
            .await;

        Ok(())
    }

    async fn create_container(&self, id: &ContainerId, spec: &ServiceSpec) -> Result<()> {
        let target = self.select_for(&id.service, spec).await?;
        {
            let mut dispatch = self.dispatch.write().await;
            dispatch.insert(id.clone(), target);
        }
        let rt = self.runtime_for(target).clone();
        match rt.create_container(id, spec).await {
            Ok(()) => Ok(()),
            Err(e) => {
                // Roll back the cache insert on failure so subsequent lookups
                // don't find a dangling entry.
                self.dispatch.write().await.remove(id);
                Err(e)
            }
        }
    }

    async fn start_container(&self, id: &ContainerId) -> Result<()> {
        let rt = self.lookup(id).await?;
        rt.start_container(id).await
    }

    async fn stop_container(&self, id: &ContainerId, timeout: Duration) -> Result<()> {
        let rt = self.lookup(id).await?;
        rt.stop_container(id, timeout).await
    }

    async fn remove_container(&self, id: &ContainerId) -> Result<()> {
        let rt = self.lookup(id).await?;
        let res = rt.remove_container(id).await;
        self.dispatch.write().await.remove(id);
        res
    }

    async fn container_state(&self, id: &ContainerId) -> Result<ContainerState> {
        let rt = self.lookup(id).await?;
        rt.container_state(id).await
    }

    async fn container_logs(&self, id: &ContainerId, tail: usize) -> Result<Vec<LogEntry>> {
        let backends = self.read_backends(id).await?;
        let mut misses = ReadMissAccumulator::default();
        for (label, rt) in backends {
            match rt.container_logs(id, tail).await {
                Ok(logs) => return Ok(logs),
                Err(e) => {
                    tracing::warn!(
                        container = %id,
                        runtime = label,
                        error = %e,
                        "composite container_logs: backend could not serve logs; trying next backend",
                    );
                    misses.record(e);
                }
            }
        }
        Err(misses.into_error("container_logs"))
    }

    async fn exec(&self, id: &ContainerId, cmd: &[String]) -> Result<(i32, String, String)> {
        let rt = self.lookup(id).await?;
        rt.exec(id, cmd).await
    }

    async fn exec_with_opts(
        &self,
        id: &ContainerId,
        opts: &crate::runtime::ExecOptions,
    ) -> Result<(i32, String, String)> {
        // Forward to the resolved backend's `exec_with_opts` so Docker exec
        // options (`--user`, `-w`, `-e`) reach the runtime that actually owns
        // the container. Without this override the trait default would call
        // `self.exec(opts.command)` and silently drop user/cwd/env.
        let rt = self.lookup(id).await?;
        rt.exec_with_opts(id, opts).await
    }

    async fn exec_stream(&self, id: &ContainerId, cmd: &[String]) -> Result<ExecEventStream> {
        let rt = self.lookup(id).await?;
        rt.exec_stream(id, cmd).await
    }

    async fn get_container_stats(&self, id: &ContainerId) -> Result<ContainerStats> {
        let backends = self.read_backends(id).await?;
        let mut misses = ReadMissAccumulator::default();
        for (label, rt) in backends {
            match rt.get_container_stats(id).await {
                Ok(stats) => return Ok(stats),
                Err(e) => {
                    tracing::warn!(
                        container = %id,
                        runtime = label,
                        error = %e,
                        "composite get_container_stats: backend could not serve stats; \
                         trying next backend",
                    );
                    misses.record(e);
                }
            }
        }
        Err(misses.into_error("get_container_stats"))
    }

    async fn wait_container(&self, id: &ContainerId) -> Result<i32> {
        let rt = self.lookup(id).await?;
        rt.wait_container(id).await
    }

    async fn wait_outcome(&self, id: &ContainerId) -> Result<WaitOutcome> {
        let rt = self.lookup(id).await?;
        rt.wait_outcome(id).await
    }

    async fn wait_outcome_with_condition(
        &self,
        id: &ContainerId,
        condition: WaitCondition,
    ) -> Result<WaitOutcome> {
        let rt = self.lookup(id).await?;
        rt.wait_outcome_with_condition(id, condition).await
    }

    async fn rename_container(&self, id: &ContainerId, new_name: &str) -> Result<()> {
        let rt = self.lookup(id).await?;
        rt.rename_container(id, new_name).await
    }

    async fn get_logs(&self, id: &ContainerId) -> Result<Vec<LogEntry>> {
        let backends = self.read_backends(id).await?;
        let mut misses = ReadMissAccumulator::default();
        for (label, rt) in backends {
            match rt.get_logs(id).await {
                Ok(logs) => return Ok(logs),
                Err(e) => {
                    tracing::warn!(
                        container = %id,
                        runtime = label,
                        error = %e,
                        "composite get_logs: backend could not serve logs; trying next backend",
                    );
                    misses.record(e);
                }
            }
        }
        Err(misses.into_error("get_logs"))
    }

    async fn logs_stream(&self, id: &ContainerId, opts: LogsStreamOptions) -> Result<LogsStream> {
        // Route to the backend that actually created the container. The default
        // trait impl returns `Unsupported`, which surfaced as a swallowed 500 on
        // `GET /containers/{id}/logs` whenever the owning backend did not
        // implement streaming (e.g. the macOS `SandboxRuntime` primary, which
        // implements `container_logs` but not `logs_stream`).
        //
        // Try each backend's `logs_stream` (owner first); on a soft miss
        // (`Unsupported`/error that is not `NotFound`) fall back to the same
        // backend's non-streaming `container_logs` and SYNTHESISE a one-shot
        // stream from it. Only a genuine `NotFound` propagates (→ 404).
        let backends = self.read_backends(id).await?;
        let mut misses = ReadMissAccumulator::default();
        for (label, rt) in &backends {
            match rt.logs_stream(id, opts.clone()).await {
                Ok(stream) => return Ok(stream),
                Err(e) => {
                    tracing::warn!(
                        container = %id,
                        runtime = label,
                        error = %e,
                        "composite logs_stream: backend has no native log stream; \
                         falling back to a one-shot snapshot",
                    );
                    misses.record(e);
                }
            }
        }

        // No backend offered a native stream. Synthesise one from whichever
        // backend can produce a captured-log snapshot (`container_logs`).
        let tail = opts
            .tail
            .map_or(1000, |n| usize::try_from(n).unwrap_or(1000));
        for (label, rt) in &backends {
            match rt.container_logs(id, tail).await {
                Ok(entries) => {
                    return Ok(one_shot_logs_stream(entries, &opts));
                }
                Err(e) => {
                    tracing::warn!(
                        container = %id,
                        runtime = label,
                        error = %e,
                        "composite logs_stream: backend snapshot fallback failed; trying next",
                    );
                    misses.record(e);
                }
            }
        }
        Err(misses.into_error("container logs"))
    }

    async fn stats_stream(&self, id: &ContainerId) -> Result<StatsStream> {
        // Same rationale as `logs_stream`: forward to the owning backend so
        // `GET /containers/{id}/stats` reaches the runtime that ran the
        // container instead of hitting the `Unsupported` default (→ swallowed
        // 500). On a soft miss, fall back to the non-streaming
        // `get_container_stats` and synthesise a one-shot sample.
        let backends = self.read_backends(id).await?;
        let mut misses = ReadMissAccumulator::default();
        for (label, rt) in &backends {
            match rt.stats_stream(id).await {
                Ok(stream) => return Ok(stream),
                Err(e) => {
                    tracing::warn!(
                        container = %id,
                        runtime = label,
                        error = %e,
                        "composite stats_stream: backend has no native stats stream; \
                         falling back to a one-shot sample",
                    );
                    misses.record(e);
                }
            }
        }

        for (label, rt) in &backends {
            match rt.get_container_stats(id).await {
                Ok(stats) => return Ok(one_shot_stats_stream(&stats)),
                Err(e) => {
                    tracing::warn!(
                        container = %id,
                        runtime = label,
                        error = %e,
                        "composite stats_stream: backend sample fallback failed; trying next",
                    );
                    misses.record(e);
                }
            }
        }
        Err(misses.into_error("container stats"))
    }

    async fn get_container_pid(&self, id: &ContainerId) -> Result<Option<u32>> {
        let rt = self.lookup(id).await?;
        rt.get_container_pid(id).await
    }

    fn overlay_attach_kind(&self) -> OverlayAttachKind {
        // Linux workloads on macOS execute in the VZ-Linux delegate, which joins
        // the overlay from inside the guest (`InGuestVsock`). Defer to it when
        // present so the service layer takes the guest-managed attach path and
        // calls `push_overlay_config` (routed per-container below); otherwise use
        // the primary's kind. Non-VZ containers route to a runtime whose
        // `push_overlay_config` is unsupported and degrade gracefully.
        self.vz_linux.as_ref().map_or_else(
            || self.primary.overlay_attach_kind(),
            |vz| vz.overlay_attach_kind(),
        )
    }

    async fn push_overlay_config(
        &self,
        id: &ContainerId,
        config: &zlayer_types::overlayd::GuestOverlayConfig,
    ) -> Result<()> {
        let rt = self.lookup(id).await?;
        rt.push_overlay_config(id, config).await
    }

    async fn get_container_ip(&self, id: &ContainerId) -> Result<Option<IpAddr>> {
        let rt = self.lookup(id).await?;
        rt.get_container_ip(id).await
    }

    async fn get_container_port_override(&self, id: &ContainerId) -> Result<Option<u16>> {
        let rt = self.lookup(id).await?;
        rt.get_container_port_override(id).await
    }

    #[cfg(target_os = "windows")]
    async fn get_container_namespace_id(
        &self,
        id: &ContainerId,
    ) -> Result<Option<windows::core::GUID>> {
        let rt = self.lookup(id).await?;
        rt.get_container_namespace_id(id).await
    }

    async fn sync_container_volumes(&self, id: &ContainerId) -> Result<()> {
        let rt = self.lookup(id).await?;
        rt.sync_container_volumes(id).await
    }

    async fn list_images(&self) -> Result<Vec<ImageInfo>> {
        // Fan out over every configured runtime and merge their image lists.
        // Crucially, a *single* backend's failure must not fail the whole
        // call: on macOS the `primary` (SandboxRuntime) does not implement
        // `list_images` at all (it returns `Unsupported`), yet pulled Linux
        // images live in the `vz_linux` delegate's store. Propagating the
        // primary's error via `?` here used to surface as a 500 on
        // `GET /images/json` (and, via the inspect fallback, broke every
        // `docker pull` verification). Tolerate per-backend errors the same
        // way we already tolerate the delegate's, and include the VZ +
        // VZ-Linux delegates so their images are actually listable.
        let mut out: Vec<ImageInfo> = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut any_ok = false;
        let mut last_err: Option<AgentError> = None;

        for (label, rt) in [
            Some(("primary", &self.primary)),
            self.delegate.as_ref().map(|d| ("delegate", d)),
            self.vz.as_ref().map(|d| ("vz", d)),
            self.vz_linux.as_ref().map(|d| ("vz_linux", d)),
        ]
        .into_iter()
        .flatten()
        {
            match rt.list_images().await {
                Ok(images) => {
                    any_ok = true;
                    for img in images {
                        // De-dup by reference so an image registered in more
                        // than one backend isn't reported twice.
                        if seen.insert(img.reference.clone()) {
                            out.push(img);
                        }
                    }
                }
                Err(e) => {
                    tracing::debug!(
                        runtime = label,
                        error = %e,
                        "composite list_images: backend returned an error; skipping it",
                    );
                    last_err = Some(e);
                }
            }
        }

        // Only fail if *every* backend errored. With at least one success we
        // return the merged (possibly empty) list — an empty image set is a
        // valid response, not an error.
        if any_ok {
            Ok(out)
        } else {
            Err(last_err.unwrap_or_else(|| {
                AgentError::Unsupported("no runtime implements list_images".into())
            }))
        }
    }

    async fn remove_image(&self, image: &str, force: bool) -> Result<()> {
        match self.primary.remove_image(image, force).await {
            Ok(()) => Ok(()),
            Err(primary_err) => {
                if let Some(delegate) = &self.delegate {
                    match delegate.remove_image(image, force).await {
                        Ok(()) => Ok(()),
                        Err(delegate_err) => {
                            tracing::debug!(
                                image,
                                %delegate_err,
                                "delegate remove_image also failed; returning primary error",
                            );
                            Err(primary_err)
                        }
                    }
                } else {
                    Err(primary_err)
                }
            }
        }
    }

    async fn prune_images(&self) -> Result<PruneResult> {
        let mut result = self.primary.prune_images().await?;
        if let Some(delegate) = &self.delegate {
            match delegate.prune_images().await {
                Ok(extra) => {
                    result.deleted.extend(extra.deleted);
                    result.space_reclaimed =
                        result.space_reclaimed.saturating_add(extra.space_reclaimed);
                }
                Err(e) => tracing::warn!(
                    error = %e,
                    "delegate runtime prune_images failed; returning primary result only",
                ),
            }
        }
        Ok(result)
    }

    async fn kill_container(&self, id: &ContainerId, signal: Option<&str>) -> Result<()> {
        let rt = self.lookup(id).await?;
        rt.kill_container(id, signal).await
    }

    async fn tag_image(&self, source: &str, target: &str) -> Result<()> {
        match self.primary.tag_image(source, target).await {
            Ok(()) => Ok(()),
            Err(primary_err) => {
                if let Some(delegate) = &self.delegate {
                    match delegate.tag_image(source, target).await {
                        Ok(()) => Ok(()),
                        Err(delegate_err) => {
                            tracing::debug!(
                                source,
                                target,
                                %delegate_err,
                                "delegate tag_image also failed; returning primary error",
                            );
                            Err(primary_err)
                        }
                    }
                } else {
                    Err(primary_err)
                }
            }
        }
    }

    async fn inspect_detailed(&self, id: &ContainerId) -> Result<ContainerInspectDetails> {
        let rt = self.lookup(id).await?;
        rt.inspect_detailed(id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cgroups_stats::ContainerStats;
    use std::sync::Mutex as StdMutex;
    use zlayer_spec::{ArchKind, DeploymentSpec, TargetPlatform};

    /// Which runtime a mock represents. Only used for labelling invocation
    /// records in tests.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Role {
        Primary,
        Delegate,
        Vz,
        VzLinux,
    }

    /// One recorded invocation: (runtime role, method name, container id).
    type CallRecord = (Role, String, Option<ContainerId>);
    /// Shared, thread-safe log of every mock call made in a single test.
    type CallLog = Arc<StdMutex<Vec<CallRecord>>>;

    /// Mock runtime that records every method call it receives.
    ///
    /// This is intentionally minimal — just enough trait surface to exercise
    /// the composite's dispatch logic. Every recorded call includes the role
    /// (primary vs delegate), the method name, and the container id (or
    /// `None` for cross-cutting image operations).
    struct MockRuntime {
        role: Role,
        calls: CallLog,
        list_images_response: Vec<ImageInfo>,
        /// When set, `list_images` returns `AgentError::Unsupported(msg)`
        /// instead of `list_images_response`. Models a backend (e.g. the macOS
        /// `SandboxRuntime` primary) that does not implement image listing.
        list_images_error: Option<String>,
        pull_image_error: Option<String>,
        /// When set, both `pull_image` and `pull_image_with_policy` return a
        /// freshly-built [`AgentError::WrongPlatform`] using these fields
        /// (`expected`, `actual`). Takes precedence over `pull_image_error`
        /// so tests can simulate a wrong-platform soft skip end-to-end.
        pull_image_wrong_platform: Option<(&'static str, &'static str)>,
        /// When `true`, the *streaming* reads (`logs_stream` / `stats_stream`)
        /// return `AgentError::Unsupported`, modelling a backend (e.g. the macOS
        /// `SandboxRuntime` primary) that implements the snapshot reads
        /// (`container_logs` / `get_container_stats`) but not the streaming
        /// ones — exactly the case that used to surface as a swallowed 500.
        stream_unsupported: bool,
        /// When `true`, *every* per-container read (`container_logs`,
        /// `get_logs`, `get_container_stats`, `logs_stream`, `stats_stream`)
        /// returns `AgentError::NotFound`, modelling a backend that does not own
        /// the container at all. The composite must NOT mask this as success,
        /// and a genuine all-not-found must propagate as `NotFound` (404).
        reads_not_found: bool,
        /// Captured-log snapshot returned by `container_logs` / `get_logs`
        /// (unless `reads_not_found`). Lets a delegate model real workload
        /// output the composite's snapshot fallback should surface.
        logs_response: Vec<LogEntry>,
        /// When `true`, the snapshot `get_container_stats` returns
        /// `AgentError::Unsupported` (a soft miss), modelling a backend that
        /// owns the container but cannot report stats at all. Forces the
        /// composite to fall back to another backend.
        stats_snapshot_unsupported: bool,
    }

    impl MockRuntime {
        fn new(role: Role, calls: CallLog) -> Self {
            Self {
                role,
                calls,
                list_images_response: Vec::new(),
                list_images_error: None,
                pull_image_error: None,
                pull_image_wrong_platform: None,
                stream_unsupported: false,
                reads_not_found: false,
                logs_response: Vec::new(),
                stats_snapshot_unsupported: false,
            }
        }

        /// Streaming reads return `Unsupported`; snapshot reads still work.
        fn with_stream_unsupported(mut self) -> Self {
            self.stream_unsupported = true;
            self
        }

        /// Every per-container read returns `NotFound`.
        fn with_reads_not_found(mut self) -> Self {
            self.reads_not_found = true;
            self
        }

        /// Set the captured-log snapshot returned by the snapshot reads.
        fn with_logs(mut self, logs: Vec<LogEntry>) -> Self {
            self.logs_response = logs;
            self
        }

        /// Snapshot `get_container_stats` returns `Unsupported` (a soft miss).
        fn with_stats_snapshot_unsupported(mut self) -> Self {
            self.stats_snapshot_unsupported = true;
            self
        }

        fn build_wrong_platform_error(&self, image: &str) -> Option<AgentError> {
            self.pull_image_wrong_platform
                .map(|(expected, actual)| AgentError::WrongPlatform {
                    runtime: match self.role {
                        Role::Primary => "primary-mock".to_string(),
                        Role::Delegate => "delegate-mock".to_string(),
                        Role::Vz => "vz-mock".to_string(),
                        Role::VzLinux => "vz-linux-mock".to_string(),
                    },
                    expected: expected.to_string(),
                    actual: actual.to_string(),
                    image: image.to_string(),
                })
        }

        fn record(&self, method: &str, id: Option<&ContainerId>) {
            self.calls
                .lock()
                .expect("mock call-log mutex poisoned")
                .push((self.role, method.to_string(), id.cloned()));
        }
    }

    #[async_trait]
    impl Runtime for MockRuntime {
        async fn pull_image(&self, image: &str) -> Result<()> {
            self.record("pull_image", None);
            if let Some(err) = self.build_wrong_platform_error(image) {
                return Err(err);
            }
            if let Some(msg) = &self.pull_image_error {
                return Err(AgentError::Internal(msg.clone()));
            }
            Ok(())
        }

        async fn pull_image_with_policy(
            &self,
            image: &str,
            _policy: PullPolicy,
            _auth: Option<&RegistryAuth>,
            _source: zlayer_spec::SourcePolicy,
        ) -> Result<()> {
            self.record("pull_image_with_policy", None);
            if let Some(err) = self.build_wrong_platform_error(image) {
                return Err(err);
            }
            if let Some(msg) = &self.pull_image_error {
                return Err(AgentError::Internal(msg.clone()));
            }
            Ok(())
        }

        async fn create_container(&self, id: &ContainerId, _spec: &ServiceSpec) -> Result<()> {
            self.record("create_container", Some(id));
            Ok(())
        }

        async fn start_container(&self, id: &ContainerId) -> Result<()> {
            self.record("start_container", Some(id));
            Ok(())
        }

        async fn stop_container(&self, id: &ContainerId, _timeout: Duration) -> Result<()> {
            self.record("stop_container", Some(id));
            Ok(())
        }

        async fn remove_container(&self, id: &ContainerId) -> Result<()> {
            self.record("remove_container", Some(id));
            Ok(())
        }

        async fn container_state(&self, id: &ContainerId) -> Result<ContainerState> {
            self.record("container_state", Some(id));
            Ok(ContainerState::Running)
        }

        async fn container_logs(&self, id: &ContainerId, _tail: usize) -> Result<Vec<LogEntry>> {
            self.record("container_logs", Some(id));
            if self.reads_not_found {
                return Err(mock_not_found());
            }
            Ok(self.logs_response.clone())
        }

        async fn exec(&self, id: &ContainerId, _cmd: &[String]) -> Result<(i32, String, String)> {
            self.record("exec", Some(id));
            Ok((0, String::new(), String::new()))
        }

        async fn get_container_stats(&self, id: &ContainerId) -> Result<ContainerStats> {
            self.record("get_container_stats", Some(id));
            if self.reads_not_found {
                return Err(mock_not_found());
            }
            if self.stats_snapshot_unsupported {
                return Err(AgentError::Unsupported("mock has no snapshot stats".into()));
            }
            Ok(ContainerStats {
                cpu_usage_usec: 1_000,
                memory_bytes: 4096,
                memory_limit: 8192,
                timestamp: std::time::Instant::now(),
            })
        }

        async fn wait_container(&self, id: &ContainerId) -> Result<i32> {
            self.record("wait_container", Some(id));
            Ok(0)
        }

        async fn get_logs(&self, id: &ContainerId) -> Result<Vec<LogEntry>> {
            self.record("get_logs", Some(id));
            if self.reads_not_found {
                return Err(mock_not_found());
            }
            Ok(self.logs_response.clone())
        }

        async fn logs_stream(
            &self,
            id: &ContainerId,
            _opts: LogsStreamOptions,
        ) -> Result<LogsStream> {
            self.record("logs_stream", Some(id));
            if self.reads_not_found {
                return Err(mock_not_found());
            }
            if self.stream_unsupported {
                return Err(AgentError::Unsupported("mock has no log stream".into()));
            }
            // A backend that owns a native stream replays its captured logs.
            Ok(one_shot_logs_stream(
                self.logs_response.clone(),
                &LogsStreamOptions::default(),
            ))
        }

        async fn stats_stream(&self, id: &ContainerId) -> Result<StatsStream> {
            use futures_util::stream;
            self.record("stats_stream", Some(id));
            if self.reads_not_found {
                return Err(mock_not_found());
            }
            if self.stream_unsupported {
                return Err(AgentError::Unsupported("mock has no stats stream".into()));
            }
            Ok(Box::pin(stream::iter(vec![Ok(StatsSample {
                cpu_total_ns: 0,
                cpu_system_ns: 0,
                online_cpus: 1,
                mem_used_bytes: 4096,
                mem_limit_bytes: 8192,
                net_rx_bytes: 0,
                net_tx_bytes: 0,
                blkio_read_bytes: 0,
                blkio_write_bytes: 0,
                pids_current: 0,
                pids_limit: None,
                timestamp: chrono::Utc::now(),
            })])))
        }

        async fn get_container_pid(&self, id: &ContainerId) -> Result<Option<u32>> {
            self.record("get_container_pid", Some(id));
            Ok(None)
        }

        async fn get_container_ip(&self, id: &ContainerId) -> Result<Option<IpAddr>> {
            self.record("get_container_ip", Some(id));
            Ok(None)
        }

        async fn list_images(&self) -> Result<Vec<ImageInfo>> {
            self.record("list_images", None);
            if let Some(msg) = &self.list_images_error {
                return Err(AgentError::Unsupported(msg.clone()));
            }
            Ok(self.list_images_response.clone())
        }
    }

    /// Build a [`ServiceSpec`] (with the given image name) from the minimal
    /// inline YAML the existing runtime tests use, then optionally set a
    /// target platform on it.
    fn make_spec(image: &str, platform: Option<TargetPlatform>) -> ServiceSpec {
        let yaml = format!(
            r"
version: v1
deployment: test
services:
  test:
    rtype: service
    image:
      name: {image}
    endpoints:
      - name: http
        protocol: http
        port: 8080
"
        );
        let mut spec = serde_yaml::from_str::<DeploymentSpec>(&yaml)
            .expect("valid deployment yaml")
            .services
            .remove("test")
            .expect("service 'test' present");
        spec.platform = platform;
        spec
    }

    fn cid(service: &str, replica: u32) -> ContainerId {
        ContainerId::new(service.to_string(), replica)
    }

    fn make_composite(with_delegate: bool) -> (CompositeRuntime, CallLog) {
        let calls = Arc::new(StdMutex::new(Vec::new()));
        let primary = Arc::new(MockRuntime::new(Role::Primary, Arc::clone(&calls)));
        let delegate = if with_delegate {
            Some(Arc::new(MockRuntime::new(Role::Delegate, Arc::clone(&calls))) as Arc<dyn Runtime>)
        } else {
            None
        };
        (
            CompositeRuntime::new(primary as Arc<dyn Runtime>, delegate),
            calls,
        )
    }

    fn role_for(calls: &[CallRecord], method: &str) -> Option<Role> {
        calls
            .iter()
            .find(|(_, m, _)| m == method)
            .map(|(role, _, _)| *role)
    }

    /// The `NotFound` a `MockRuntime` returns when it does not own a container.
    fn mock_not_found() -> AgentError {
        AgentError::NotFound {
            container: "mock".to_string(),
            reason: "mock backend does not own this container".to_string(),
        }
    }

    #[tokio::test]
    async fn dispatch_windows_spec_goes_to_primary() {
        let (rt, calls) = make_composite(true);
        let id = cid("win-svc", 0);
        let spec = make_spec(
            "mcr.microsoft.com/windows/nanoserver:ltsc2022",
            Some(TargetPlatform::new(OsKind::Windows, ArchKind::Amd64)),
        );

        rt.create_container(&id, &spec).await.unwrap();
        rt.start_container(&id).await.unwrap();

        let calls = calls.lock().unwrap();
        assert_eq!(
            role_for(&calls, "create_container"),
            Some(Role::Primary),
            "create_container should hit primary for Windows spec"
        );
        assert_eq!(
            role_for(&calls, "start_container"),
            Some(Role::Primary),
            "start_container should hit primary for Windows spec"
        );
    }

    #[tokio::test]
    async fn dispatch_linux_spec_goes_to_delegate() {
        let (rt, calls) = make_composite(true);
        let id = cid("lin-svc", 0);
        let spec = make_spec(
            "docker.io/library/alpine:3.19",
            Some(TargetPlatform::new(OsKind::Linux, ArchKind::Amd64)),
        );

        rt.create_container(&id, &spec).await.unwrap();
        rt.start_container(&id).await.unwrap();

        let calls = calls.lock().unwrap();
        assert_eq!(
            role_for(&calls, "create_container"),
            Some(Role::Delegate),
            "create_container should hit delegate for Linux spec"
        );
        assert_eq!(
            role_for(&calls, "start_container"),
            Some(Role::Delegate),
            "start_container should hit delegate for Linux spec"
        );
    }

    #[tokio::test]
    async fn dispatch_linux_without_delegate_errors() {
        // H-7 policy: a Linux spec on a node without a delegate must return
        // `RouteToPeer` (not `Unsupported`, not a silent primary fall-through)
        // so the scheduler can re-place the workload on a capable peer.
        let (rt, _calls) = make_composite(false);
        let id = cid("lin-svc", 0);
        let spec = make_spec(
            "docker.io/library/alpine:3.19",
            Some(TargetPlatform::new(OsKind::Linux, ArchKind::Amd64)),
        );

        let err = rt.create_container(&id, &spec).await.unwrap_err();
        match err {
            AgentError::RouteToPeer {
                service,
                required_os,
                reason,
            } => {
                assert_eq!(service, "lin-svc");
                assert_eq!(required_os, "linux");
                assert!(
                    reason.contains("--install-wsl") && reason.contains("Linux peer"),
                    "reason must name both remediations, got: {reason}"
                );
            }
            other => panic!("expected RouteToPeer, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_linux_image_cache_without_delegate_routes_to_peer() {
        // H-7 policy: even when `spec.platform` is unset, a Linux image in the
        // OS cache must route to a peer instead of falling through to primary.
        // This is the old permissive-fallthrough path the comment at lines
        // 172-178 used to describe; the behavior is now strict.
        let (rt, _calls) = make_composite(false);
        let id = cid("svc", 0);
        let image = "docker.io/library/nginx:1.25";
        rt.record_image_os(image, OsKind::Linux).await;

        let spec = make_spec(image, None);
        let err = rt.create_container(&id, &spec).await.unwrap_err();
        match err {
            AgentError::RouteToPeer {
                service,
                required_os,
                reason,
            } => {
                assert_eq!(service, "svc");
                assert_eq!(required_os, "linux");
                assert!(
                    reason.contains(image),
                    "reason should mention the image name, got: {reason}"
                );
                assert!(
                    reason.contains("--install-wsl") && reason.contains("Linux peer"),
                    "reason must name both remediations, got: {reason}"
                );
            }
            other => panic!("expected RouteToPeer, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_macos_spec_goes_to_primary() {
        let (rt, calls) = make_composite(true);
        let id = cid("mac-svc", 0);
        let spec = make_spec(
            "ghcr.io/zlayer/macos:latest",
            Some(TargetPlatform::new(OsKind::Macos, ArchKind::Arm64)),
        );

        rt.create_container(&id, &spec).await.unwrap();

        let calls = calls.lock().unwrap();
        assert_eq!(
            role_for(&calls, "create_container"),
            Some(Role::Primary),
            "create_container should hit primary for Macos spec"
        );
    }

    #[tokio::test]
    async fn dispatch_no_platform_no_image_os_falls_through_to_primary() {
        let (rt, calls) = make_composite(true);
        let id = cid("svc", 0);
        let spec = make_spec("docker.io/library/nginx:1.25", None);

        rt.create_container(&id, &spec).await.unwrap();

        let calls = calls.lock().unwrap();
        assert_eq!(
            role_for(&calls, "create_container"),
            Some(Role::Primary),
            "fall-through should pick primary when both platform and image-OS cache are unknown"
        );
    }

    #[tokio::test]
    async fn dispatch_uses_image_os_cache_when_platform_missing() {
        let (rt, calls) = make_composite(true);
        let id = cid("svc", 0);
        let image = "docker.io/library/nginx:1.25";
        rt.record_image_os(image, OsKind::Linux).await;

        let spec = make_spec(image, None);
        rt.create_container(&id, &spec).await.unwrap();

        let calls = calls.lock().unwrap();
        assert_eq!(
            role_for(&calls, "create_container"),
            Some(Role::Delegate),
            "image-OS cache should route Linux images to the delegate"
        );
    }

    /// Composite with primary + delegate + an attached VZ delegate, all sharing
    /// one call log.
    fn make_composite_with_vz() -> (CompositeRuntime, CallLog) {
        let calls = Arc::new(StdMutex::new(Vec::new()));
        let primary = Arc::new(MockRuntime::new(Role::Primary, Arc::clone(&calls)));
        let delegate =
            Arc::new(MockRuntime::new(Role::Delegate, Arc::clone(&calls))) as Arc<dyn Runtime>;
        let vz = Arc::new(MockRuntime::new(Role::Vz, Arc::clone(&calls))) as Arc<dyn Runtime>;
        let rt = CompositeRuntime::new(primary as Arc<dyn Runtime>, Some(delegate))
            .with_vz_delegate(Some(vz));
        (rt, calls)
    }

    #[tokio::test]
    async fn dispatch_vz_bundle_annotation_auto_routes_to_vz() {
        let (rt, calls) = make_composite_with_vz();
        let id = cid("mac-svc", 0);
        let image = "ghcr.io/org/macos-vz:sequoia";
        // Simulate the manifest inspection having cached `com.zlayer.runtime=vz`.
        rt.record_image_runtime(image, "vz".to_string()).await;

        let spec = make_spec(image, None);
        rt.create_container(&id, &spec).await.unwrap();

        let calls = calls.lock().unwrap();
        assert_eq!(
            role_for(&calls, "create_container"),
            Some(Role::Vz),
            "a com.zlayer.runtime=vz bundle should auto-route to the VZ runtime"
        );
    }

    #[tokio::test]
    async fn dispatch_vz_label_forces_vz() {
        let (rt, calls) = make_composite_with_vz();
        let id = cid("mac-svc", 0);
        let mut spec = make_spec("ghcr.io/org/whatever:1", None);
        spec.labels
            .insert("com.zlayer.isolation".to_string(), "vz".to_string());

        rt.create_container(&id, &spec).await.unwrap();

        let calls = calls.lock().unwrap();
        assert_eq!(
            role_for(&calls, "create_container"),
            Some(Role::Vz),
            "an explicit com.zlayer.isolation=vz label should force the VZ runtime"
        );
    }

    #[tokio::test]
    async fn dispatch_sandbox_label_overrides_vz_bundle() {
        let (rt, calls) = make_composite_with_vz();
        let id = cid("mac-svc", 0);
        let image = "ghcr.io/org/macos-vz:sequoia";
        rt.record_image_runtime(image, "vz".to_string()).await;

        let mut spec = make_spec(image, None);
        spec.labels
            .insert("com.zlayer.isolation".to_string(), "sandbox".to_string());
        rt.create_container(&id, &spec).await.unwrap();

        let calls = calls.lock().unwrap();
        assert_eq!(
            role_for(&calls, "create_container"),
            Some(Role::Primary),
            "com.zlayer.isolation=sandbox should opt out of VZ auto-detect (force the sandbox)"
        );
    }

    /// Composite with primary + delegate (libkrun) + a VZ Linux-guest delegate,
    /// all sharing one call log. Mirrors `make_composite_with_vz`.
    fn make_composite_with_vz_linux() -> (CompositeRuntime, CallLog) {
        let calls = Arc::new(StdMutex::new(Vec::new()));
        let primary = Arc::new(MockRuntime::new(Role::Primary, Arc::clone(&calls)));
        let delegate =
            Arc::new(MockRuntime::new(Role::Delegate, Arc::clone(&calls))) as Arc<dyn Runtime>;
        let vz_linux =
            Arc::new(MockRuntime::new(Role::VzLinux, Arc::clone(&calls))) as Arc<dyn Runtime>;
        let rt = CompositeRuntime::new(primary as Arc<dyn Runtime>, Some(delegate))
            .with_vz_linux_delegate(Some(vz_linux));
        (rt, calls)
    }

    #[tokio::test]
    async fn dispatch_vz_linux_label_forces_vz_linux() {
        let (rt, calls) = make_composite_with_vz_linux();
        let id = cid("lin-svc", 0);
        let mut spec = make_spec("docker.io/library/alpine:3.19", None);
        spec.labels
            .insert("com.zlayer.isolation".to_string(), "vz-linux".to_string());

        rt.create_container(&id, &spec).await.unwrap();

        let calls = calls.lock().unwrap();
        assert_eq!(
            role_for(&calls, "create_container"),
            Some(Role::VzLinux),
            "com.zlayer.isolation=vz-linux must force the VZ Linux runtime"
        );
    }

    #[tokio::test]
    async fn dispatch_vz_linux_marker_auto_routes_to_vz_linux() {
        let (rt, calls) = make_composite_with_vz_linux();
        let id = cid("lin-svc", 0);
        let image = "ghcr.io/org/linux-vz:bookworm";
        rt.record_image_runtime(image, "vz-linux".to_string()).await;

        let spec = make_spec(image, None);
        rt.create_container(&id, &spec).await.unwrap();

        let calls = calls.lock().unwrap();
        assert_eq!(
            role_for(&calls, "create_container"),
            Some(Role::VzLinux),
            "a com.zlayer.runtime=vz-linux marker should auto-route to the VZ Linux runtime"
        );
    }

    #[tokio::test]
    async fn dispatch_linux_platform_with_vz_linux_routes_to_vz_linux() {
        let (rt, calls) = make_composite_with_vz_linux();
        let id = cid("lin-svc", 0);
        // platform.os = linux: with a VZ Linux delegate present this is the
        // default Linux path, NOT the libkrun delegate.
        let spec = make_spec(
            "docker.io/library/alpine:3.19",
            Some(TargetPlatform::new(OsKind::Linux, ArchKind::Arm64)),
        );

        rt.create_container(&id, &spec).await.unwrap();

        let calls = calls.lock().unwrap();
        assert_eq!(
            role_for(&calls, "create_container"),
            Some(Role::VzLinux),
            "a Linux platform spec must default to the VZ Linux runtime when present"
        );
    }

    #[tokio::test]
    async fn dispatch_linux_image_os_with_vz_linux_routes_to_vz_linux() {
        let (rt, calls) = make_composite_with_vz_linux();
        let id = cid("lin-svc", 0);
        let image = "docker.io/library/nginx:1.25";
        rt.record_image_os(image, OsKind::Linux).await;

        let spec = make_spec(image, None);
        rt.create_container(&id, &spec).await.unwrap();

        let calls = calls.lock().unwrap();
        assert_eq!(
            role_for(&calls, "create_container"),
            Some(Role::VzLinux),
            "a Linux image-OS cache hit must default to the VZ Linux runtime when present"
        );
    }

    #[tokio::test]
    async fn dispatch_macos_image_os_with_vz_linux_routes_to_primary() {
        // A macOS-native rootfs must NEVER go to the Linux VM. Even with a
        // VZ-Linux delegate present (the default Linux path), an image whose
        // locally-known OS is macOS routes to the primary (Seatbelt sandbox).
        let (rt, calls) = make_composite_with_vz_linux();
        let id = cid("mac-svc", 0);
        let image = "ghcr.io/zlayer/macos-native:latest";
        rt.record_image_os(image, OsKind::Macos).await;

        let spec = make_spec(image, None);
        rt.create_container(&id, &spec).await.unwrap();

        let calls = calls.lock().unwrap();
        assert_eq!(
            role_for(&calls, "create_container"),
            Some(Role::Primary),
            "image_os == Macos must route to primary even when VZ-Linux is the default",
        );
    }

    #[tokio::test]
    async fn dispatch_unknown_os_with_vz_linux_defaults_to_vz_linux() {
        // OS genuinely unknown (no isolation label, no runtime marker, no
        // platform, no image-OS cache hit) on a macOS host with a VZ-Linux
        // delegate: default to VZ-Linux. Sending an unknown (overwhelmingly
        // Linux) image to the Seatbelt sandbox is the exit-127 failure this fix
        // exists to prevent.
        let (rt, calls) = make_composite_with_vz_linux();
        let id = cid("svc", 0);
        let spec = make_spec("docker.io/library/whatever:latest", None);

        rt.create_container(&id, &spec).await.unwrap();

        let calls = calls.lock().unwrap();
        assert_eq!(
            role_for(&calls, "create_container"),
            Some(Role::VzLinux),
            "an unknown-OS image must default to VZ-Linux when the delegate is present",
        );
    }

    #[tokio::test]
    async fn dispatch_unknown_os_without_vz_linux_falls_through_to_primary() {
        // The unknown-OS default to VZ-Linux is keyed on the delegate's
        // presence (a proxy for "macOS host"). Without a VZ-Linux delegate the
        // historical primary fallthrough is preserved for non-macOS hosts.
        let (rt, calls) = make_composite(true);
        let id = cid("svc", 0);
        let spec = make_spec("docker.io/library/whatever:latest", None);

        rt.create_container(&id, &spec).await.unwrap();

        let calls = calls.lock().unwrap();
        assert_eq!(
            role_for(&calls, "create_container"),
            Some(Role::Primary),
            "without a VZ-Linux delegate an unknown-OS image keeps the primary fallthrough",
        );
    }

    /// Seed a persistent blob cache at `path` with a manifest + config blob for
    /// `image` whose config declares `os = linux`, mirroring what a real
    /// VZ-Linux pull writes to `{data_dir}/vz/linux/images/blobs.redb`.
    async fn seed_persistent_linux_cache(path: &std::path::Path, image: &str) {
        seed_persistent_cache_with_os(path, image, "linux").await;
    }

    /// Like [`seed_persistent_linux_cache`] but lets the test pick the config
    /// `os` value (e.g. `"darwin"` for a macOS-native bundle).
    async fn seed_persistent_cache_with_os(path: &std::path::Path, image: &str, os: &str) {
        let cache = zlayer_registry::CacheType::persistent_at(path)
            .build()
            .await
            .expect("open persistent blob cache");

        let config_json = serde_json::json!({
            "architecture": "arm64",
            "os": os,
            "config": {},
        });
        let config_bytes = serde_json::to_vec(&config_json).unwrap();
        let config_digest = zlayer_registry::compute_digest(&config_bytes);
        cache.put(&config_digest, &config_bytes).await.unwrap();

        let manifest = zlayer_registry::OciImageManifest {
            schema_version: 2,
            media_type: Some("application/vnd.oci.image.manifest.v1+json".to_string()),
            artifact_type: None,
            config: oci_client::manifest::OciDescriptor {
                media_type: "application/vnd.oci.image.config.v1+json".to_string(),
                digest: config_digest.clone(),
                size: i64::try_from(config_bytes.len()).unwrap(),
                urls: None,
                annotations: None,
            },
            layers: vec![],
            annotations: None,
            subject: None,
        };
        let manifest_bytes = serde_json::to_vec(&manifest).unwrap();
        let manifest_digest = zlayer_registry::compute_digest(&manifest_bytes);
        cache
            .put(&zlayer_registry::manifest_cache_key(image), &manifest_bytes)
            .await
            .unwrap();
        cache
            .put(
                &zlayer_registry::manifest_digest_cache_key(image),
                manifest_digest.as_bytes(),
            )
            .await
            .unwrap();
    }

    /// End-to-end of the macOS rate-limit routing fix: a Linux image whose OS
    /// lives ONLY in the local persistent blob cache (no network) must be
    /// inspected at `pull_image` time and then routed to the VZ-Linux runtime
    /// by `select_for` — exactly the path that breaks under a Docker Hub 429
    /// when inspection goes to the wire.
    #[tokio::test]
    async fn pull_then_dispatch_resolves_linux_os_from_local_cache_routes_to_vz_linux() {
        let tmp = tempfile::tempdir().unwrap();
        let cache_path = tmp.path().join("blobs.redb");
        let image = "docker.io/library/alpine:latest";
        seed_persistent_linux_cache(&cache_path, image).await;

        let (rt, calls) = make_composite_with_vz_linux();
        let rt = rt.with_os_inspect_cache_path(Some(cache_path));

        // pull_image drives the real local-first OS inspection; no network.
        rt.pull_image(image).await.unwrap();

        // The OS must now be cached as Linux purely from the local store.
        assert_eq!(
            rt.image_os.read().await.get(image).copied(),
            Some(OsKind::Linux),
            "pull_image must resolve Linux OS from the local persistent cache",
        );

        // And select_for must route the (platform-less) spec to VZ-Linux.
        let id = cid("lin-svc", 0);
        let spec = make_spec(image, None);
        rt.create_container(&id, &spec).await.unwrap();

        let calls = calls.lock().unwrap();
        assert_eq!(
            role_for(&calls, "create_container"),
            Some(Role::VzLinux),
            "a Linux image whose OS came from the local cache must route to VZ-Linux",
        );
    }

    /// LIVE BUG #1, end-to-end: the cache is seeded under the QUALIFIED ref
    /// (`docker.io/library/alpine:latest`, as the pull writes it) but the spec —
    /// and therefore every `pull_image` / `inspect_image_os` / `select_for`
    /// lookup — uses the BARE `alpine:latest`. With the canonical manifest-key
    /// normalization, the bare-ref inspect hits the qualified-seeded cache with
    /// NO network call, so the Linux image still routes to VZ-Linux.
    #[tokio::test]
    async fn bare_ref_spec_resolves_os_from_qualified_seeded_cache_routes_to_vz_linux() {
        let tmp = tempfile::tempdir().unwrap();
        let cache_path = tmp.path().join("blobs.redb");
        // Seed under the QUALIFIED ref, exactly as a real pull persists it.
        seed_persistent_linux_cache(&cache_path, "docker.io/library/alpine:latest").await;

        let (rt, calls) = make_composite_with_vz_linux();
        let rt = rt.with_os_inspect_cache_paths(vec![cache_path]);

        // Everything below uses the BARE ref, exactly as the live daemon does
        // (`ImageRef::Display` yields the user-original string).
        let bare = "alpine:latest";
        rt.pull_image(bare).await.unwrap();

        assert_eq!(
            rt.image_os.read().await.get(bare).copied(),
            Some(OsKind::Linux),
            "bare-ref inspect must resolve Linux from the qualified-seeded cache",
        );

        let id = cid("lin-svc", 0);
        let spec = make_spec(bare, None);
        rt.create_container(&id, &spec).await.unwrap();

        let calls = calls.lock().unwrap();
        assert_eq!(
            role_for(&calls, "create_container"),
            Some(Role::VzLinux),
            "bare-ref Linux image routes to VZ-Linux via the canonical-key cache hit",
        );
    }

    /// LIVE BUG #2 / multi-cache fallback: the manifest+config live ONLY in the
    /// SECOND configured cache (the primary Sandbox store), because the
    /// VZ-Linux pull short-circuited under `IfNotPresent`. Inspection must probe
    /// the empty first cache (no network), then resolve from the second — still
    /// with NO network — and route to VZ-Linux.
    #[tokio::test]
    async fn os_resolves_from_second_cache_when_first_is_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let empty_cache = tmp.path().join("vz-linux-blobs.redb");
        let primary_cache = tmp.path().join("primary-blobs.redb");
        // Create the first cache empty (so opening it succeeds but it misses).
        zlayer_registry::CacheType::persistent_at(&empty_cache)
            .build()
            .await
            .unwrap();
        // Only the SECOND cache has the image.
        seed_persistent_linux_cache(&primary_cache, "docker.io/library/alpine:latest").await;

        let (rt, calls) = make_composite_with_vz_linux();
        let rt = rt.with_os_inspect_cache_paths(vec![empty_cache, primary_cache]);

        let bare = "alpine:latest";
        rt.pull_image(bare).await.unwrap();

        assert_eq!(
            rt.image_os.read().await.get(bare).copied(),
            Some(OsKind::Linux),
            "OS must resolve from the second cache after the first misses (no network)",
        );

        let id = cid("lin-svc", 0);
        let spec = make_spec(bare, None);
        rt.create_container(&id, &spec).await.unwrap();

        let calls = calls.lock().unwrap();
        assert_eq!(role_for(&calls, "create_container"), Some(Role::VzLinux),);
    }

    /// The exact LIVE bug, simulated end-to-end: a `pull_image` whose network OS
    /// re-inspection WOULD 429 still leaves dispatch fully working, because the
    /// image's OS is resolved purely from the local persistent blob cache the
    /// runtime already populated during extract — with NO network call at all.
    ///
    /// We model the 429 by pointing `os_inspect_cache_paths` at a real seeded
    /// cache (so the local resolver succeeds) while using a synthetic
    /// `*.invalid` registry host: if the dispatch-population path ever reached
    /// the network it would fail to resolve, leaving the cache empty and routing
    /// the Linux image to the Seatbelt primary (exit 127). It must not — the
    /// local cache hit is authoritative and the image routes to VZ-Linux.
    #[tokio::test]
    async fn pull_with_network_429_still_dispatches_via_local_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let cache_path = tmp.path().join("blobs.redb");
        // The image ref uses a host that cannot be resolved on the wire; only
        // the LOCAL cache knows its OS.
        let image = "registry.invalid.example/library/alpine:latest";
        seed_persistent_linux_cache(&cache_path, image).await;

        let (rt, calls) = make_composite_with_vz_linux();
        let rt = rt.with_os_inspect_cache_path(Some(cache_path));

        // `pull_image` drives the dispatch-population inspection. Even though a
        // real registry inspection of `*.invalid` would fail (our stand-in for a
        // 429), the local-only path resolves Linux and the call succeeds.
        rt.pull_image(image).await.unwrap();
        assert_eq!(
            rt.image_os.read().await.get(image).copied(),
            Some(OsKind::Linux),
            "OS must be resolved from the local cache with no network call",
        );

        // And dispatch routes the Linux image to VZ-Linux, not the primary.
        let id = cid("lin-svc", 0);
        let spec = make_spec(image, None);
        rt.create_container(&id, &spec).await.unwrap();

        let calls = calls.lock().unwrap();
        assert_eq!(
            role_for(&calls, "create_container"),
            Some(Role::VzLinux),
            "a would-be-429 pull must still route the cached Linux image to VZ-Linux",
        );
    }

    /// Companion to the macOS-native dispatch guard, but driving the resolution
    /// through the real local-cache inspection at `pull_image` time: a bundle
    /// whose config declares `os = darwin` in the local cache must route to the
    /// primary, never the Linux VM.
    #[tokio::test]
    async fn pull_then_dispatch_resolves_macos_os_from_local_cache_routes_to_primary() {
        let tmp = tempfile::tempdir().unwrap();
        let cache_path = tmp.path().join("blobs.redb");
        let image = "ghcr.io/zlayer/macos-native:latest";
        seed_persistent_cache_with_os(&cache_path, image, "darwin").await;

        let (rt, calls) = make_composite_with_vz_linux();
        let rt = rt.with_os_inspect_cache_path(Some(cache_path));

        rt.pull_image(image).await.unwrap();
        assert_eq!(
            rt.image_os.read().await.get(image).copied(),
            Some(OsKind::Macos),
            "pull_image must resolve macOS OS from the local persistent cache",
        );

        let id = cid("mac-svc", 0);
        let spec = make_spec(image, None);
        rt.create_container(&id, &spec).await.unwrap();

        let calls = calls.lock().unwrap();
        assert_eq!(
            role_for(&calls, "create_container"),
            Some(Role::Primary),
            "a macOS-native rootfs must route to primary even with VZ-Linux as default",
        );
    }

    #[tokio::test]
    async fn dispatch_vm_label_forces_libkrun_delegate() {
        let (rt, calls) = make_composite_with_vz_linux();
        let id = cid("lin-svc", 0);
        // Even with a VZ Linux delegate as the default, an explicit
        // `com.zlayer.isolation=vm` label forces the libkrun delegate.
        let mut spec = make_spec(
            "docker.io/library/alpine:3.19",
            Some(TargetPlatform::new(OsKind::Linux, ArchKind::Arm64)),
        );
        spec.labels
            .insert("com.zlayer.isolation".to_string(), "vm".to_string());

        rt.create_container(&id, &spec).await.unwrap();

        let calls = calls.lock().unwrap();
        assert_eq!(
            role_for(&calls, "create_container"),
            Some(Role::Delegate),
            "com.zlayer.isolation=vm must force the libkrun delegate even when VZ Linux is default"
        );
    }

    #[tokio::test]
    async fn dispatch_unmarked_image_with_vz_delegate_falls_through_to_primary() {
        let (rt, calls) = make_composite_with_vz();
        let id = cid("mac-svc", 0);
        // No runtime marker, no platform, no image-OS cache: VZ must NOT capture
        // ordinary images just because the delegate exists.
        let spec = make_spec("ghcr.io/org/plain:1", None);
        rt.create_container(&id, &spec).await.unwrap();

        let calls = calls.lock().unwrap();
        assert_eq!(
            role_for(&calls, "create_container"),
            Some(Role::Primary),
            "an unmarked image must fall through to primary even when a VZ delegate is attached"
        );
    }

    #[tokio::test]
    async fn per_container_dispatch_cache_persists_through_start_stop() {
        let (rt, calls) = make_composite(true);
        let id = cid("win-svc", 0);
        let spec = make_spec(
            "mcr.microsoft.com/windows/nanoserver:ltsc2022",
            Some(TargetPlatform::new(OsKind::Windows, ArchKind::Amd64)),
        );

        rt.create_container(&id, &spec).await.unwrap();
        rt.start_container(&id).await.unwrap();
        rt.stop_container(&id, Duration::from_secs(1))
            .await
            .unwrap();
        rt.remove_container(&id).await.unwrap();

        let recorded = calls.lock().unwrap().clone();
        for method in [
            "create_container",
            "start_container",
            "stop_container",
            "remove_container",
        ] {
            assert_eq!(
                role_for(&recorded, method),
                Some(Role::Primary),
                "{method} should have dispatched to primary"
            );
        }

        // After remove, the dispatch cache entry should be gone.
        let after = rt
            .start_container(&id)
            .await
            .expect_err("lookup after remove should fail");
        assert!(
            matches!(after, AgentError::NotFound { .. }),
            "expected NotFound after remove, got {after:?}"
        );
    }

    #[tokio::test]
    async fn pull_image_calls_both_runtimes() {
        let (rt, calls) = make_composite(true);
        rt.pull_image("docker.io/library/alpine:3.19")
            .await
            .unwrap();

        let recorded = calls.lock().unwrap();
        let pull_calls: Vec<Role> = recorded
            .iter()
            .filter(|(_, m, _)| m == "pull_image")
            .map(|(r, _, _)| *r)
            .collect();
        assert!(
            pull_calls.contains(&Role::Primary),
            "primary should have been pulled: {pull_calls:?}",
        );
        assert!(
            pull_calls.contains(&Role::Delegate),
            "delegate should have been pulled: {pull_calls:?}",
        );
    }

    #[tokio::test]
    async fn pull_image_delegate_error_does_not_fail() {
        // Build the composite by hand so we can flip the delegate's
        // pull_image_error before wrapping it in an Arc<dyn Runtime>.
        let calls = Arc::new(StdMutex::new(Vec::new()));
        let primary = Arc::new(MockRuntime::new(Role::Primary, Arc::clone(&calls)));
        let mut delegate = MockRuntime::new(Role::Delegate, Arc::clone(&calls));
        delegate.pull_image_error = Some("simulated delegate pull failure".to_string());
        let rt = CompositeRuntime::new(
            primary as Arc<dyn Runtime>,
            Some(Arc::new(delegate) as Arc<dyn Runtime>),
        );

        // Top-level call must succeed despite the delegate error.
        rt.pull_image("docker.io/library/alpine:3.19")
            .await
            .unwrap();

        let recorded = calls.lock().unwrap();
        let pull_calls: Vec<Role> = recorded
            .iter()
            .filter(|(_, m, _)| m == "pull_image")
            .map(|(r, _, _)| *r)
            .collect();
        assert!(
            pull_calls.contains(&Role::Primary) && pull_calls.contains(&Role::Delegate),
            "both runtimes should have been called: {pull_calls:?}",
        );
    }

    #[tokio::test]
    async fn pull_image_primary_wrong_platform_does_not_fail() {
        // The HCS runtime returns `AgentError::WrongPlatform` when the image's
        // OCI config reports a non-Windows OS (calling `ProcessBaseImage` on a
        // Linux base layer is guaranteed to fail with 0x80070003). The
        // composite must treat that as a soft skip and let the delegate's
        // pull own the image — the overall pull must NOT fail.
        let calls = Arc::new(StdMutex::new(Vec::new()));
        let mut primary = MockRuntime::new(Role::Primary, Arc::clone(&calls));
        primary.pull_image_wrong_platform = Some(("windows", "linux"));
        let delegate = MockRuntime::new(Role::Delegate, Arc::clone(&calls));
        let rt = CompositeRuntime::new(
            Arc::new(primary) as Arc<dyn Runtime>,
            Some(Arc::new(delegate) as Arc<dyn Runtime>),
        );

        // Top-level call must succeed despite the primary's wrong-platform err.
        rt.pull_image("docker.io/library/alpine:3.19")
            .await
            .expect("composite pull must tolerate WrongPlatform from primary");

        let recorded = calls.lock().unwrap();
        let pull_calls: Vec<Role> = recorded
            .iter()
            .filter(|(_, m, _)| m == "pull_image")
            .map(|(r, _, _)| *r)
            .collect();
        assert!(
            pull_calls.contains(&Role::Primary) && pull_calls.contains(&Role::Delegate),
            "delegate must still be called when primary soft-skips: {pull_calls:?}",
        );
    }

    #[tokio::test]
    async fn pull_image_with_policy_primary_wrong_platform_does_not_fail() {
        // Same contract as `pull_image_primary_wrong_platform_does_not_fail`
        // but exercising the `pull_image_with_policy` entry point. The
        // policy/auth path is what the daemon's create-container hot loop
        // actually invokes, so it has to honour the same soft-skip rule.
        let calls = Arc::new(StdMutex::new(Vec::new()));
        let mut primary = MockRuntime::new(Role::Primary, Arc::clone(&calls));
        primary.pull_image_wrong_platform = Some(("windows", "linux"));
        let delegate = MockRuntime::new(Role::Delegate, Arc::clone(&calls));
        let rt = CompositeRuntime::new(
            Arc::new(primary) as Arc<dyn Runtime>,
            Some(Arc::new(delegate) as Arc<dyn Runtime>),
        );

        rt.pull_image_with_policy(
            "docker.io/library/alpine:3.19",
            PullPolicy::IfNotPresent,
            None,
            zlayer_spec::SourcePolicy::default(),
        )
        .await
        .expect("composite pull_image_with_policy must tolerate WrongPlatform from primary");

        let recorded = calls.lock().unwrap();
        let pull_calls: Vec<Role> = recorded
            .iter()
            .filter(|(_, m, _)| m == "pull_image_with_policy")
            .map(|(r, _, _)| *r)
            .collect();
        assert!(
            pull_calls.contains(&Role::Primary) && pull_calls.contains(&Role::Delegate),
            "delegate must still be called when primary soft-skips: {pull_calls:?}",
        );
    }

    #[tokio::test]
    async fn pull_image_primary_non_wrong_platform_error_still_fails() {
        // Sanity check: only `WrongPlatform` is soft-skipped. Any other error
        // from the primary must still bubble up so real pull failures aren't
        // silently swallowed.
        let calls = Arc::new(StdMutex::new(Vec::new()));
        let mut primary = MockRuntime::new(Role::Primary, Arc::clone(&calls));
        primary.pull_image_error = Some("simulated real failure".to_string());
        let delegate = MockRuntime::new(Role::Delegate, Arc::clone(&calls));
        let rt = CompositeRuntime::new(
            Arc::new(primary) as Arc<dyn Runtime>,
            Some(Arc::new(delegate) as Arc<dyn Runtime>),
        );

        let err = rt
            .pull_image("docker.io/library/alpine:3.19")
            .await
            .expect_err("real primary error must propagate");
        assert!(
            matches!(err, AgentError::Internal(_)),
            "expected Internal, got {err:?}",
        );
    }

    #[tokio::test]
    async fn list_images_merges_both() {
        // Hand-build so we can seed each mock's list_images_response.
        let calls = Arc::new(StdMutex::new(Vec::new()));
        let mut primary = MockRuntime::new(Role::Primary, Arc::clone(&calls));
        primary.list_images_response = vec![ImageInfo {
            reference: "primary/image:1".to_string(),
            digest: None,
            size_bytes: None,
        }];
        let mut delegate = MockRuntime::new(Role::Delegate, Arc::clone(&calls));
        delegate.list_images_response = vec![ImageInfo {
            reference: "delegate/image:1".to_string(),
            digest: None,
            size_bytes: None,
        }];
        let rt = CompositeRuntime::new(
            Arc::new(primary) as Arc<dyn Runtime>,
            Some(Arc::new(delegate) as Arc<dyn Runtime>),
        );

        let merged = rt.list_images().await.unwrap();
        let refs: Vec<&str> = merged.iter().map(|i| i.reference.as_str()).collect();
        assert!(
            refs.contains(&"primary/image:1") && refs.contains(&"delegate/image:1"),
            "merged list should contain both entries, got {refs:?}",
        );
    }

    /// Regression (macOS `GET /images/json` 500): when the *primary* runtime
    /// does not implement `list_images` (the `SandboxRuntime` returns
    /// `Unsupported`), the composite must NOT propagate that error. It must
    /// fall back to the other backends — in particular the VZ-Linux delegate
    /// that actually owns pulled Linux images — and return their list. Before
    /// the fix the composite used `self.primary.list_images().await?`, which
    /// surfaced as a 500 and (via the inspect fallback) broke `docker pull`.
    #[tokio::test]
    async fn list_images_tolerates_primary_unsupported_and_uses_vz_linux() {
        let calls = Arc::new(StdMutex::new(Vec::new()));
        let mut primary = MockRuntime::new(Role::Primary, Arc::clone(&calls));
        primary.list_images_error = Some("list_images is not supported".to_string());
        let mut vz_linux = MockRuntime::new(Role::VzLinux, Arc::clone(&calls));
        vz_linux.list_images_response = vec![ImageInfo {
            reference: "docker.io/library/alpine:latest".to_string(),
            digest: None,
            size_bytes: None,
        }];

        let rt = CompositeRuntime::new(Arc::new(primary) as Arc<dyn Runtime>, None)
            .with_vz_linux_delegate(Some(Arc::new(vz_linux) as Arc<dyn Runtime>));

        let images = rt
            .list_images()
            .await
            .expect("primary Unsupported must not fail the composite list_images");
        let refs: Vec<&str> = images.iter().map(|i| i.reference.as_str()).collect();
        assert_eq!(
            refs,
            vec!["docker.io/library/alpine:latest"],
            "should return the VZ-Linux delegate's images, got {refs:?}",
        );
    }

    /// When EVERY backend fails `list_images`, the composite surfaces an error
    /// (rather than silently returning an empty list, which would mask a total
    /// backend outage).
    #[tokio::test]
    async fn list_images_errors_only_when_all_backends_fail() {
        let calls = Arc::new(StdMutex::new(Vec::new()));
        let mut primary = MockRuntime::new(Role::Primary, Arc::clone(&calls));
        primary.list_images_error = Some("unsupported".to_string());
        let mut vz_linux = MockRuntime::new(Role::VzLinux, Arc::clone(&calls));
        vz_linux.list_images_error = Some("also unsupported".to_string());

        let rt = CompositeRuntime::new(Arc::new(primary) as Arc<dyn Runtime>, None)
            .with_vz_linux_delegate(Some(Arc::new(vz_linux) as Arc<dyn Runtime>));

        let err = rt.list_images().await.unwrap_err();
        assert!(
            matches!(err, AgentError::Unsupported(_)),
            "all-backends-fail should surface Unsupported, got {err:?}",
        );
    }

    // ----------------------------------------------------------------------
    // Per-container read routing (logs / stats).
    //
    // These guard the macOS Docker-compat `/logs` and `/stats` 500 fix: when
    // the owning backend cannot serve a particular read (the primary
    // `SandboxRuntime` implements snapshot reads but returns `Unsupported` for
    // the *streaming* ones, or a different backend owns the container), the
    // composite must route to / fall back across backends and return real data
    // instead of propagating `Unsupported` as a swallowed 500. Only a genuine
    // all-not-found is a 404.
    // ----------------------------------------------------------------------

    /// Build a `LogEntry` with the given stream + message for read tests.
    fn log_entry(stream: LogStream, message: &str) -> LogEntry {
        LogEntry {
            timestamp: chrono::Utc::now(),
            stream,
            source: zlayer_observability::logs::LogSource::Container("test".to_string()),
            message: message.to_string(),
            service: None,
            deployment: None,
        }
    }

    /// Drain a `LogsStream` into the concatenated UTF-8 body bytes.
    async fn drain_logs(stream: LogsStream) -> String {
        use futures_util::StreamExt as _;
        let mut out = Vec::new();
        let mut s = stream;
        while let Some(item) = s.next().await {
            out.extend_from_slice(&item.expect("log chunk ok").bytes);
        }
        String::from_utf8(out).expect("utf8 log body")
    }

    /// Collect a `StatsStream` into a Vec of samples.
    async fn drain_stats(stream: StatsStream) -> Vec<StatsSample> {
        use futures_util::StreamExt as _;
        let mut out = Vec::new();
        let mut s = stream;
        while let Some(item) = s.next().await {
            out.push(item.expect("stats sample ok"));
        }
        out
    }

    /// Build a composite whose primary models the macOS `SandboxRuntime`
    /// (snapshot reads work, streaming reads return `Unsupported`) and whose
    /// VZ-Linux delegate owns the container with working native streams.
    /// Returns (composite, call-log) with a container already dispatched to the
    /// chosen owner.
    async fn make_read_composite(owner: Role) -> (CompositeRuntime, ContainerId, CallLog) {
        let calls = Arc::new(StdMutex::new(Vec::new()));
        let logs = vec![
            log_entry(LogStream::Stdout, "hello stdout"),
            log_entry(LogStream::Stderr, "hello stderr"),
        ];
        let primary = MockRuntime::new(Role::Primary, Arc::clone(&calls))
            .with_stream_unsupported()
            .with_logs(logs.clone());
        let vz_linux = MockRuntime::new(Role::VzLinux, Arc::clone(&calls)).with_logs(logs);
        let rt = CompositeRuntime::new(Arc::new(primary) as Arc<dyn Runtime>, None)
            .with_vz_linux_delegate(Some(Arc::new(vz_linux) as Arc<dyn Runtime>));

        let id = cid("read-svc", 0);
        // Dispatch the container to the chosen owner without going through the
        // (platform-dependent) `select_for` path.
        let target = match owner {
            Role::Primary => DispatchTarget::Primary,
            Role::VzLinux => DispatchTarget::VzLinux,
            other => panic!("make_read_composite supports Primary/VzLinux, not {other:?}"),
        };
        rt.dispatch.write().await.insert(id.clone(), target);
        (rt, id, calls)
    }

    #[tokio::test]
    async fn logs_stream_falls_back_to_snapshot_when_owner_has_no_stream() {
        // Sole backend = primary (SandboxRuntime model): `logs_stream` is
        // Unsupported, but `container_logs` works. With no other backend the
        // composite must synthesise a stream from the snapshot rather than 500.
        let calls = Arc::new(StdMutex::new(Vec::new()));
        let logs = vec![
            log_entry(LogStream::Stdout, "hello stdout"),
            log_entry(LogStream::Stderr, "hello stderr"),
        ];
        let primary = MockRuntime::new(Role::Primary, Arc::clone(&calls))
            .with_stream_unsupported()
            .with_logs(logs);
        let rt = CompositeRuntime::new(Arc::new(primary) as Arc<dyn Runtime>, None);
        let id = cid("read-svc", 0);
        rt.dispatch
            .write()
            .await
            .insert(id.clone(), DispatchTarget::Primary);

        let stream = rt
            .logs_stream(&id, LogsStreamOptions::default())
            .await
            .expect("logs_stream must not 500 when snapshot reads work");
        let body = drain_logs(stream).await;
        assert!(
            body.contains("hello stdout") && body.contains("hello stderr"),
            "synthesised stream must carry the captured logs, got: {body:?}",
        );
    }

    #[tokio::test]
    async fn logs_stream_routes_to_delegate_owner_native_stream() {
        // Owner = VZ-Linux delegate with a working native stream; the primary's
        // streaming read is Unsupported but must not be consulted first.
        let (rt, id, calls) = make_read_composite(Role::VzLinux).await;
        let stream = rt
            .logs_stream(&id, LogsStreamOptions::default())
            .await
            .expect("delegate-owned logs_stream must succeed");
        let body = drain_logs(stream).await;
        assert!(body.contains("hello stdout"), "got: {body:?}");

        let log = calls.lock().expect("call-log mutex poisoned");
        assert_eq!(
            role_for(&log, "logs_stream"),
            Some(Role::VzLinux),
            "logs_stream must hit the owning delegate first, calls: {log:?}",
        );
    }

    #[tokio::test]
    async fn get_logs_falls_back_across_backends() {
        // Owner = primary; here snapshot `get_logs` works on primary directly,
        // so it should succeed on the owner without ever consulting the
        // delegate. (Soft-miss fallback is exercised by the stats test below.)
        let (rt, id, _calls) = make_read_composite(Role::Primary).await;
        let logs = rt.get_logs(&id).await.expect("get_logs must succeed");
        assert_eq!(logs.len(), 2, "owner snapshot logs should be returned");
    }

    #[tokio::test]
    async fn stats_stream_falls_back_to_snapshot_when_owner_has_no_stream() {
        // Sole backend = primary (SandboxRuntime model): `stats_stream` is
        // Unsupported but `get_container_stats` works. With no other backend
        // offering a native stream, the composite must synthesise a single
        // non-empty sample from the snapshot rather than 500.
        let calls = Arc::new(StdMutex::new(Vec::new()));
        let primary = MockRuntime::new(Role::Primary, Arc::clone(&calls)).with_stream_unsupported();
        let rt = CompositeRuntime::new(Arc::new(primary) as Arc<dyn Runtime>, None);
        let id = cid("read-svc", 0);
        rt.dispatch
            .write()
            .await
            .insert(id.clone(), DispatchTarget::Primary);

        let stream = rt
            .stats_stream(&id)
            .await
            .expect("stats_stream must not 500 when get_container_stats works");
        let samples = drain_stats(stream).await;
        assert_eq!(samples.len(), 1, "snapshot fallback yields one sample");
        assert!(
            samples[0].mem_used_bytes > 0,
            "synthesised sample must carry non-zero memory, got {:?}",
            samples[0],
        );
        assert_eq!(
            samples[0].cpu_total_ns, 1_000_000,
            "cpu microseconds must be scaled to nanoseconds in the synthesised sample",
        );
    }

    #[tokio::test]
    async fn get_container_stats_tolerates_owner_miss_and_uses_other_backend() {
        // Owner = primary whose snapshot `get_container_stats` returns
        // `Unsupported` (a soft miss); the delegate that follows in the fallback
        // chain serves it. The composite must NOT propagate the owner's
        // Unsupported as a 500.
        let calls = Arc::new(StdMutex::new(Vec::new()));
        let primary =
            MockRuntime::new(Role::Primary, Arc::clone(&calls)).with_stats_snapshot_unsupported();
        let vz_linux = MockRuntime::new(Role::VzLinux, Arc::clone(&calls));
        let rt = CompositeRuntime::new(Arc::new(primary) as Arc<dyn Runtime>, None)
            .with_vz_linux_delegate(Some(Arc::new(vz_linux) as Arc<dyn Runtime>));
        let id = cid("read-svc", 0);
        rt.dispatch
            .write()
            .await
            .insert(id.clone(), DispatchTarget::Primary);

        let stats = rt
            .get_container_stats(&id)
            .await
            .expect("owner Unsupported must fall back to the delegate, not 500");
        assert!(stats.memory_bytes > 0, "delegate stats should be returned");

        let log = calls.lock().expect("call-log mutex poisoned");
        assert!(
            log.iter()
                .any(|(role, method, _)| *role == Role::Primary && method == "get_container_stats"),
            "primary must have been tried first, calls: {log:?}",
        );
        assert!(
            log.iter()
                .any(|(role, method, _)| *role == Role::VzLinux && method == "get_container_stats"),
            "delegate must have served the fallback, calls: {log:?}",
        );
    }

    #[tokio::test]
    async fn reads_propagate_not_found_when_no_backend_owns_container() {
        // Every backend returns NotFound for the dispatched container: the
        // composite must surface NotFound (→ 404), NOT mask it as Unsupported
        // or empty success.
        let calls = Arc::new(StdMutex::new(Vec::new()));
        let primary = MockRuntime::new(Role::Primary, Arc::clone(&calls)).with_reads_not_found();
        let vz_linux = MockRuntime::new(Role::VzLinux, Arc::clone(&calls)).with_reads_not_found();
        let rt = CompositeRuntime::new(Arc::new(primary) as Arc<dyn Runtime>, None)
            .with_vz_linux_delegate(Some(Arc::new(vz_linux) as Arc<dyn Runtime>));
        let id = cid("read-svc", 0);
        rt.dispatch
            .write()
            .await
            .insert(id.clone(), DispatchTarget::Primary);

        // `LogsStream`/`StatsStream` are not `Debug`, so match instead of
        // `unwrap_err()`.
        match rt.logs_stream(&id, LogsStreamOptions::default()).await {
            Err(AgentError::NotFound { .. }) => {}
            other => panic!(
                "all-not-found logs_stream must be NotFound (404), got {:?}",
                other.err(),
            ),
        }
        match rt.stats_stream(&id).await {
            Err(AgentError::NotFound { .. }) => {}
            other => panic!(
                "all-not-found stats_stream must be NotFound (404), got {:?}",
                other.err(),
            ),
        }
        let cl_err = rt.container_logs(&id, 10).await.unwrap_err();
        assert!(
            matches!(cl_err, AgentError::NotFound { .. }),
            "all-not-found container_logs must be NotFound (404), got {cl_err:?}",
        );
    }

    #[tokio::test]
    async fn reads_on_undispatched_container_are_not_found() {
        // No dispatch record at all → NotFound (the id was never created here).
        let (rt, _calls) = make_composite(false);
        let id = cid("ghost", 0);
        match rt.logs_stream(&id, LogsStreamOptions::default()).await {
            Err(AgentError::NotFound { .. }) => {}
            other => panic!(
                "undispatched logs_stream must be NotFound, got {:?}",
                other.err()
            ),
        }
    }

    /// Regression: `pull_image` must fan out to the VZ-Linux delegate so the
    /// image lands in the store where Linux containers actually execute on
    /// macOS (and so it becomes listable/inspectable). Before the fix the
    /// composite only pulled into `primary` + `delegate`, leaving the
    /// VZ-Linux `image_rootfs` empty.
    #[tokio::test]
    async fn pull_image_fans_out_to_vz_linux() {
        let calls = Arc::new(StdMutex::new(Vec::new()));
        let primary = MockRuntime::new(Role::Primary, Arc::clone(&calls));
        let vz_linux = MockRuntime::new(Role::VzLinux, Arc::clone(&calls));

        let rt = CompositeRuntime::new(Arc::new(primary) as Arc<dyn Runtime>, None)
            .with_vz_linux_delegate(Some(Arc::new(vz_linux) as Arc<dyn Runtime>));

        rt.pull_image("docker.io/library/alpine:latest")
            .await
            .expect("pull should succeed");

        let log = calls.lock().expect("call-log mutex poisoned");
        assert!(
            log.iter()
                .any(|(role, method, _)| *role == Role::VzLinux && method == "pull_image"),
            "pull_image must reach the VZ-Linux delegate, recorded calls: {log:?}",
        );
    }

    #[tokio::test]
    async fn dispatch_lookup_unknown_container_errors() {
        let (rt, _calls) = make_composite(true);
        let id = cid("ghost", 0);

        let err = rt.start_container(&id).await.unwrap_err();
        assert!(
            matches!(err, AgentError::NotFound { .. }),
            "expected NotFound for unknown container, got {err:?}"
        );
    }

    /// Helper: read the internal image-OS cache for test assertions.
    async fn cached_os(rt: &CompositeRuntime, image: &str) -> Option<OsKind> {
        rt.image_os.read().await.get(image).copied()
    }

    #[tokio::test]
    async fn apply_image_os_inspection_populates_cache_on_ok_some() {
        // Contract: when `fetch_image_os` resolves to a recognized OS, the
        // cache is populated so subsequent `select_for` calls for specs
        // without `platform` dispatch correctly.
        let (rt, _calls) = make_composite(true);
        let image = "docker.io/library/alpine:3.19";

        rt.apply_image_os_inspection(image, Ok(Some(OsKind::Linux)))
            .await;

        assert_eq!(cached_os(&rt, image).await, Some(OsKind::Linux));
    }

    #[tokio::test]
    async fn apply_image_os_inspection_leaves_cache_untouched_on_ok_none() {
        // Contract: when the manifest carries no (or an unrecognized) `os`
        // field the cache is left alone. Dispatch will fall through to the
        // primary on `create_container`.
        let (rt, _calls) = make_composite(true);
        let image = "docker.io/library/nginx:1.25";

        rt.apply_image_os_inspection(image, Ok(None)).await;

        assert_eq!(cached_os(&rt, image).await, None);
    }

    #[tokio::test]
    async fn apply_image_os_inspection_leaves_cache_untouched_on_err() {
        // Contract: a registry error during inspection is non-fatal and must
        // not poison the cache. Dispatch falls through to primary on lookup.
        let (rt, _calls) = make_composite(true);
        let image = "docker.io/library/nginx:1.25";

        // Pre-seed the cache so we can assert the error path doesn't
        // overwrite or clear an existing entry.
        rt.record_image_os(image, OsKind::Linux).await;

        let err = zlayer_registry::RegistryError::NotFound {
            registry: "docker.io".to_string(),
            image: image.to_string(),
        };
        rt.apply_image_os_inspection(image, Err(err)).await;

        // Cache is still whatever it was before the failed inspection.
        assert_eq!(cached_os(&rt, image).await, Some(OsKind::Linux));
    }

    #[tokio::test]
    async fn pull_image_inspection_failure_does_not_fail_pull() {
        // End-to-end: even when the registry fetch fails (inevitable for the
        // synthetic image refs used in unit tests), `pull_image` still
        // returns `Ok`. The mock primary/delegate both succeed; the
        // inspection step logs and moves on. The cache must remain empty
        // because there was no successful inspection to record.
        let (rt, _calls) = make_composite(true);
        let image = "invalid.example.invalid/ghost:v1";

        rt.pull_image(image).await.unwrap();

        assert_eq!(
            cached_os(&rt, image).await,
            None,
            "failed inspection must not populate the image-OS cache"
        );
    }

    #[tokio::test]
    async fn pull_image_with_policy_inspection_failure_does_not_fail_pull() {
        // Same contract as `pull_image_inspection_failure_does_not_fail_pull`
        // but exercising the policy-aware entry point.
        let (rt, _calls) = make_composite(true);
        let image = "invalid.example.invalid/ghost:v1";

        rt.pull_image_with_policy(
            image,
            PullPolicy::IfNotPresent,
            None,
            zlayer_spec::SourcePolicy::default(),
        )
        .await
        .unwrap();

        assert_eq!(cached_os(&rt, image).await, None);
    }

    #[test]
    fn os_kind_from_oci_str_roundtrip() {
        // Guards the `as_oci_str` ↔ `from_oci_str` relationship used by the
        // inspection path. If a new variant is added to `OsKind` without
        // updating `from_oci_str` we want the miss here, not a silent
        // "dispatch to primary" regression in production.
        for os in [OsKind::Linux, OsKind::Windows, OsKind::Macos] {
            assert_eq!(OsKind::from_oci_str(os.as_oci_str()), Some(os));
        }
        assert_eq!(OsKind::from_oci_str(""), None);
        assert_eq!(OsKind::from_oci_str("freebsd"), None);
    }
}
