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
use zlayer_observability::logs::LogEntry;
use zlayer_spec::{OsKind, PullPolicy, RegistryAuth, ServiceSpec};

use crate::cgroups_stats::ContainerStats;
use crate::error::{AgentError, Result};
use crate::runtime::{
    ContainerId, ContainerInspectDetails, ContainerState, ExecEventStream, ImageInfo, PruneResult,
    Runtime, WaitCondition, WaitOutcome,
};

/// Which underlying runtime a given container was dispatched to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DispatchTarget {
    Primary,
    Delegate,
}

/// Routes each container to either the primary runtime or an optional delegate.
///
/// See the module-level documentation for the dispatch rules.
pub struct CompositeRuntime {
    primary: Arc<dyn Runtime>,
    delegate: Option<Arc<dyn Runtime>>,
    /// Per-container dispatch cache. Populated on `create_container`, removed
    /// on `remove_container`.
    dispatch: Arc<RwLock<HashMap<ContainerId, DispatchTarget>>>,
    /// Image-OS cache consulted when a spec has no explicit `platform`.
    /// Populated by [`CompositeRuntime::record_image_os`], which is driven
    /// from [`zlayer_registry::fetch_image_os`] during `pull_image*`.
    image_os: Arc<RwLock<HashMap<String, OsKind>>>,
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
            dispatch: Arc::new(RwLock::new(HashMap::new())),
            image_os: Arc::new(RwLock::new(HashMap::new())),
        }
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
    async fn select_for(&self, service: &str, spec: &ServiceSpec) -> Result<DispatchTarget> {
        if let Some(platform) = &spec.platform {
            let target = match platform.os {
                OsKind::Windows | OsKind::Macos => DispatchTarget::Primary,
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
                    if self.delegate.is_some() {
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
        }
    }
}

#[async_trait]
impl Runtime for CompositeRuntime {
    async fn pull_image(&self, image: &str) -> Result<()> {
        self.primary.pull_image(image).await?;
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

        // Inspect the OCI manifest's `config.os` so `select_for(spec)` can
        // dispatch correctly when `spec.platform` is `None`. Non-fatal: any
        // failure here just means dispatch falls through to primary.
        let os_result = zlayer_registry::fetch_image_os(image, None).await;
        self.apply_image_os_inspection(image, os_result).await;

        Ok(())
    }

    async fn pull_image_with_policy(
        &self,
        image: &str,
        policy: PullPolicy,
        auth: Option<&RegistryAuth>,
    ) -> Result<()> {
        self.primary
            .pull_image_with_policy(image, policy, auth)
            .await?;
        if let Some(delegate) = &self.delegate {
            if let Err(e) = delegate.pull_image_with_policy(image, policy, auth).await {
                tracing::debug!(
                    image,
                    error = %e,
                    "delegate runtime failed to pull image (likely wrong OS); continuing with primary result",
                );
            }
        }

        let os_result = zlayer_registry::fetch_image_os(image, auth).await;
        self.apply_image_os_inspection(image, os_result).await;

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
        let rt = self.lookup(id).await?;
        rt.container_logs(id, tail).await
    }

    async fn exec(&self, id: &ContainerId, cmd: &[String]) -> Result<(i32, String, String)> {
        let rt = self.lookup(id).await?;
        rt.exec(id, cmd).await
    }

    async fn exec_stream(&self, id: &ContainerId, cmd: &[String]) -> Result<ExecEventStream> {
        let rt = self.lookup(id).await?;
        rt.exec_stream(id, cmd).await
    }

    async fn get_container_stats(&self, id: &ContainerId) -> Result<ContainerStats> {
        let rt = self.lookup(id).await?;
        rt.get_container_stats(id).await
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
        let rt = self.lookup(id).await?;
        rt.get_logs(id).await
    }

    async fn get_container_pid(&self, id: &ContainerId) -> Result<Option<u32>> {
        let rt = self.lookup(id).await?;
        rt.get_container_pid(id).await
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
        let mut out = self.primary.list_images().await?;
        if let Some(delegate) = &self.delegate {
            match delegate.list_images().await {
                Ok(extra) => out.extend(extra),
                Err(e) => tracing::warn!(
                    error = %e,
                    "delegate runtime list_images failed; returning primary results only",
                ),
            }
        }
        Ok(out)
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
        pull_image_error: Option<String>,
    }

    impl MockRuntime {
        fn new(role: Role, calls: CallLog) -> Self {
            Self {
                role,
                calls,
                list_images_response: Vec::new(),
                pull_image_error: None,
            }
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
        async fn pull_image(&self, _image: &str) -> Result<()> {
            self.record("pull_image", None);
            if let Some(msg) = &self.pull_image_error {
                return Err(AgentError::Internal(msg.clone()));
            }
            Ok(())
        }

        async fn pull_image_with_policy(
            &self,
            _image: &str,
            _policy: PullPolicy,
            _auth: Option<&RegistryAuth>,
        ) -> Result<()> {
            self.record("pull_image_with_policy", None);
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
            Ok(Vec::new())
        }

        async fn exec(&self, id: &ContainerId, _cmd: &[String]) -> Result<(i32, String, String)> {
            self.record("exec", Some(id));
            Ok((0, String::new(), String::new()))
        }

        async fn get_container_stats(&self, id: &ContainerId) -> Result<ContainerStats> {
            self.record("get_container_stats", Some(id));
            Ok(ContainerStats {
                cpu_usage_usec: 0,
                memory_bytes: 0,
                memory_limit: 0,
                timestamp: std::time::Instant::now(),
            })
        }

        async fn wait_container(&self, id: &ContainerId) -> Result<i32> {
            self.record("wait_container", Some(id));
            Ok(0)
        }

        async fn get_logs(&self, id: &ContainerId) -> Result<Vec<LogEntry>> {
            self.record("get_logs", Some(id));
            Ok(Vec::new())
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
        ContainerId {
            service: service.to_string(),
            replica,
        }
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

        rt.pull_image_with_policy(image, PullPolicy::IfNotPresent, None)
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
