//! Service-level container lifecycle management

use crate::container_supervisor::{ContainerSupervisor, SupervisedState, SupervisorEvent};
use crate::cron_scheduler::CronScheduler;
use crate::dependency::{
    DependencyConditionChecker, DependencyGraph, DependencyWaiter, WaitResult,
};
use crate::error::{AgentError, Result};
use crate::health::{HealthCallback, HealthChecker, HealthMonitor, HealthState};
use crate::init::InitOrchestrator;
use crate::job::{JobExecution, JobExecutionId, JobExecutor, JobTrigger};
use crate::overlay_manager::OverlayManager;
use crate::proxy_manager::ProxyManager;
use crate::runtime::{Container, ContainerId, ContainerState, Runtime};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{RwLock, Semaphore};
use zlayer_observability::logs::LogEntry;
use zlayer_overlay::DnsServer;
use zlayer_proxy::{StreamRegistry, StreamService};
use zlayer_spec::{DependsSpec, HealthCheck, Protocol, PullPolicy, ResourceType, ServiceSpec};

/// Service instance manages a single service's containers
pub struct ServiceInstance {
    pub service_name: String,
    pub spec: ServiceSpec,
    runtime: Arc<dyn Runtime + Send + Sync>,
    containers: tokio::sync::RwLock<std::collections::HashMap<ContainerId, Container>>,
    /// Overlay network manager for container networking (optional, not needed for Docker runtime)
    overlay_manager: Option<Arc<RwLock<OverlayManager>>>,
    /// Proxy manager for updating backend health (optional)
    proxy_manager: Option<Arc<ProxyManager>>,
    /// DNS server for service discovery (optional)
    dns_server: Option<Arc<DnsServer>>,
    /// Shared health states map so callbacks can update ServiceManager-level health
    health_states: Option<Arc<RwLock<HashMap<String, HealthState>>>>,
    /// Most recently observed image digest after a successful pull. Used by
    /// `upsert_service` to detect drift on `:latest`/`Newer` redeploys without
    /// requiring callers to track digest state externally. Wrapped in a
    /// `RwLock` so `&self` methods (`scale_to`) can update it.
    last_pulled_digest: tokio::sync::RwLock<Option<String>>,
    /// Local cluster node id used when constructing new `ContainerId`s during
    /// scale-up. `0` in single-node deployments or when the cluster handle is
    /// not yet wired. Populated by `ServiceManager` from `Cluster::node_id()`
    /// at instance construction time.
    node_id: u64,
}

impl ServiceInstance {
    /// Create a new service instance
    pub fn new(
        service_name: String,
        spec: ServiceSpec,
        runtime: Arc<dyn Runtime + Send + Sync>,
        overlay_manager: Option<Arc<RwLock<OverlayManager>>>,
    ) -> Self {
        Self {
            service_name,
            spec,
            runtime,
            containers: tokio::sync::RwLock::new(std::collections::HashMap::new()),
            overlay_manager,
            proxy_manager: None,
            dns_server: None,
            health_states: None,
            last_pulled_digest: tokio::sync::RwLock::new(None),
            node_id: 0,
        }
    }

    /// Create a new service instance with proxy manager for health-aware load balancing
    pub fn with_proxy(
        service_name: String,
        spec: ServiceSpec,
        runtime: Arc<dyn Runtime + Send + Sync>,
        overlay_manager: Option<Arc<RwLock<OverlayManager>>>,
        proxy_manager: Arc<ProxyManager>,
    ) -> Self {
        Self {
            service_name,
            spec,
            runtime,
            containers: tokio::sync::RwLock::new(std::collections::HashMap::new()),
            overlay_manager,
            proxy_manager: Some(proxy_manager),
            dns_server: None,
            health_states: None,
            last_pulled_digest: tokio::sync::RwLock::new(None),
            node_id: 0,
        }
    }

    /// Set the local cluster node id. Used by `ServiceManager` to thread
    /// `Cluster::node_id()` down to container construction so new
    /// `ContainerId`s carry the owning node identity. Defaults to `0` (the
    /// single-node sentinel) when unset.
    pub fn set_node_id(&mut self, node_id: u64) {
        self.node_id = node_id;
    }

    /// Derive the replica group role for a 1-based `replica_idx`.
    ///
    /// When `spec.replica_groups` is unset, returns `"default"` (the implicit
    /// single-group case). Otherwise walks groups in declaration order,
    /// accumulating each group's `count` until `replica_idx` falls within the
    /// current group's range, and returns that group's `role`.
    ///
    /// Replicas beyond the declared total fall back to `"default"`.
    #[must_use]
    pub fn role_for_replica(&self, replica_idx: u32) -> String {
        let Some(groups) = self.spec.replica_groups.as_ref() else {
            return "default".to_string();
        };
        let mut cumulative = 0u32;
        for group in groups {
            cumulative = cumulative.saturating_add(group.count);
            if replica_idx <= cumulative {
                return group.role.clone();
            }
        }
        "default".to_string()
    }

    /// Builder method to add DNS server for service discovery
    #[must_use]
    pub fn with_dns(mut self, dns_server: Arc<DnsServer>) -> Self {
        self.dns_server = Some(dns_server);
        self
    }

    /// Set the DNS server for service discovery
    pub fn set_dns_server(&mut self, dns_server: Arc<DnsServer>) {
        self.dns_server = Some(dns_server);
    }

    /// Set the proxy manager for health-aware load balancing
    pub fn set_proxy_manager(&mut self, proxy_manager: Arc<ProxyManager>) {
        self.proxy_manager = Some(proxy_manager);
    }

    /// Set the shared health states map so health callbacks can bridge state back to `ServiceManager`
    pub fn set_health_states(&mut self, states: Arc<RwLock<HashMap<String, HealthState>>>) {
        self.health_states = Some(states);
    }

    /// Get the last observed image digest (after the most recent successful
    /// pull). Returns `None` when no pull has happened yet, when the runtime
    /// does not expose digests, or when no matching `ImageInfo` was found.
    pub async fn last_pulled_digest(&self) -> Option<String> {
        self.last_pulled_digest.read().await.clone()
    }

    /// Pull the service image using the spec's pull policy (literal Docker /
    /// Kubernetes semantics — no silent auto-upgrade of `IfNotPresent` to
    /// `Newer` for `:latest` tags) and refresh the cached digest from
    /// `Runtime::list_images` when the runtime exposes it. Returns the digest
    /// observed after the pull, when known.
    ///
    /// For `Never`, the runtime is still called so it can load the image
    /// config from the local cache (without any remote round-trip); only the
    /// remote digest refresh is skipped. Without this call the bundle builder
    /// has no image entrypoint/cmd and falls back to `/bin/sh`.
    async fn pull_and_refresh_digest(&self) -> Result<Option<String>> {
        let image_str = self.spec.image.name.to_string();
        let policy = self.spec.image.pull_policy;

        self.runtime
            .pull_image_with_policy(&image_str, policy, None)
            .await
            .map_err(|e| AgentError::PullFailed {
                image: self.spec.image.name.to_string(),
                reason: e.to_string(),
            })?;

        // Best-effort: try to discover the resolved digest via list_images.
        // Runtimes that don't support introspection (Unsupported) leave the
        // cached digest unchanged; drift detection then falls back to "always
        // recreate on PullPolicy::Always, never recreate on PullPolicy::Newer
        // when no digests are known".
        let new_digest = match self.runtime.list_images().await {
            Ok(images) => images
                .into_iter()
                .find(|info| info.reference == image_str)
                .and_then(|info| info.digest),
            Err(e) => {
                tracing::debug!(
                    image = %image_str,
                    error = %e,
                    "list_images unavailable; cannot record post-pull digest"
                );
                None
            }
        };

        if let Some(ref digest) = new_digest {
            *self.last_pulled_digest.write().await = Some(digest.clone());
        }

        Ok(new_digest)
    }

    /// Scale to the desired number of replicas
    ///
    /// This method uses short-lived locks to avoid blocking concurrent operations.
    /// I/O operations (pull, create, start, stop, remove) are performed without
    /// holding the containers lock to allow other operations to proceed.
    ///
    /// # Errors
    /// Returns an error if image pull, container creation, or container lifecycle operations fail.
    #[allow(clippy::too_many_lines, clippy::cast_possible_truncation)]
    pub async fn scale_to(&self, replicas: u32) -> Result<()> {
        // Phase 1: Determine current state (short read lock)
        let current_replicas = { self.containers.read().await.len() as u32 }; // Lock released here

        // Phase 1b: Pull image up front so a redeploy on `:latest` (which lands
        // here with replicas == current_replicas in the steady state) actually
        // refreshes the cached digest. We skip the call only when scaling
        // strictly down (no new containers needed). For `Never` the runtime
        // still needs to load the image config from the local cache so the
        // bundle builder gets entrypoint/cmd/env — without it the container
        // falls back to `/bin/sh` and exits instantly. `pull_and_refresh_digest`
        // itself handles the Never case (no remote round-trip, cache-only).
        if replicas >= current_replicas {
            let _ = self.pull_and_refresh_digest().await?;
        }

        // Phase 2: Scale up - create new containers (no lock held during I/O)
        //
        // Compute (role, replica_index) tuples for each new replica. When
        // `spec.replica_groups` is set, expand groups in declaration order so
        // each created replica maps to its declared `(role, intra_group_index)`.
        // Otherwise fall back to the implicit single "default" group. The
        // `local_node_id` is captured once so every new `ContainerId` carries
        // the owning node identity for cross-node disambiguation.
        let local_node_id = self.node_id;
        if replicas > current_replicas {
            let replica_specs: Vec<(String, u32)> =
                if let Some(groups) = self.spec.replica_groups.as_ref() {
                    let mut specs: Vec<(String, u32)> = Vec::new();
                    for group in groups {
                        for idx in 0..group.count {
                            specs.push((group.role.clone(), idx + 1));
                        }
                    }
                    specs
                        .into_iter()
                        .skip(current_replicas as usize)
                        .take((replicas - current_replicas) as usize)
                        .collect()
                } else {
                    (current_replicas..replicas)
                        .map(|i| ("default".to_string(), i + 1))
                        .collect()
                };

            for (role, replica_idx) in replica_specs {
                let id = ContainerId::with_role_and_node(
                    self.service_name.clone(),
                    replica_idx,
                    role,
                    local_node_id,
                );

                // Create container (no lock needed - I/O operation)
                //
                // RouteToPeer must propagate unchanged: the scheduler uses it
                // to re-place the workload on a capable peer, and wrapping it
                // in `CreateFailed` would hide the signal and mark the service
                // dead instead of rescheduling it. All other errors are
                // normalised to `CreateFailed` for upstream handling.
                self.runtime
                    .create_container(&id, &self.spec)
                    .await
                    .map_err(|e| match e {
                        AgentError::RouteToPeer { .. } => e,
                        other => AgentError::CreateFailed {
                            id: id.to_string(),
                            reason: other.to_string(),
                        },
                    })?;

                // Run init actions with error policy enforcement (no lock needed)
                let init_orchestrator = InitOrchestrator::with_error_policy(
                    id.clone(),
                    self.spec.init.clone(),
                    self.spec.errors.clone(),
                );
                init_orchestrator.run().await?;

                // Start container (no lock needed - I/O operation)
                self.runtime
                    .start_container(&id)
                    .await
                    .map_err(|e| AgentError::StartFailed {
                        id: id.to_string(),
                        reason: e.to_string(),
                    })?;

                // Get container PID with retries (may not be immediately available)
                let mut container_pid = None;
                for attempt in 1..=5u32 {
                    match self.runtime.get_container_pid(&id).await {
                        Ok(Some(pid)) => {
                            container_pid = Some(pid);
                            break;
                        }
                        Ok(None) if attempt < 5 => {
                            tracing::debug!(container = %id, attempt, "PID not available yet, retrying");
                            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                        }
                        Ok(None) => {
                            tracing::warn!(container = %id, "Container PID unavailable after 5 attempts");
                        }
                        Err(e) => {
                            tracing::warn!(container = %id, attempt, error = %e, "Failed to get PID");
                            if attempt < 5 {
                                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                            }
                        }
                    }
                }

                // Verify the container is still running before attempting
                // overlay attach. If the init process crashed during start
                // (bad image, missing libs, failed mount), the PID above is
                // now dead and every `ip link set ... netns {pid}` will
                // return a cryptic RTNETLINK error. Surface the real cause
                // from the container's log tail instead of the cascade.
                if container_pid.is_some() {
                    let alive = match self.runtime.container_state(&id).await {
                        Ok(
                            ContainerState::Running
                            | ContainerState::Pending
                            | ContainerState::Initializing,
                        ) => true,
                        Ok(state) => {
                            tracing::warn!(
                                container = %id,
                                ?state,
                                "container exited before overlay attach could run"
                            );
                            false
                        }
                        Err(e) => {
                            // State query failed — don't block the attach on
                            // it. The overlay manager's own cleanup-on-error
                            // path now handles the dead-PID case cleanly.
                            tracing::warn!(
                                container = %id,
                                error = %e,
                                "container state query failed before overlay attach, proceeding"
                            );
                            true
                        }
                    };
                    if !alive {
                        let log_tail = self.runtime.container_logs(&id, 40).await.ok().map_or_else(
                            || "  <log read failed>".to_string(),
                            |entries| {
                                if entries.is_empty() {
                                    "  <no log output>".to_string()
                                } else {
                                    entries
                                        .into_iter()
                                        .map(|e| format!("  {}", e.message))
                                        .collect::<Vec<_>>()
                                        .join("\n")
                                }
                            },
                        );
                        return Err(AgentError::StartFailed {
                            id: id.to_string(),
                            reason: format!("container exited during startup:\n{log_tail}"),
                        });
                    }
                }

                // Attach to overlay network if manager is available.
                //
                // Linux uses the container PID to enter the netns and attach a
                // veth. Windows has no PID-addressable netns — the HCN namespace
                // GUID (obtained from `get_container_namespace_id`) is used
                // instead, and the endpoint's IP has already been populated by
                // `EndpointAttachment::create_overlay` during container creation.
                // We simply register that IP with the slice allocator so host
                // accounting stays in sync.
                let overlay_ip = if let Some(overlay) = &self.overlay_manager {
                    let overlay_guard = overlay.read().await;
                    #[cfg(target_os = "windows")]
                    let attach_result: Option<std::net::IpAddr> = {
                        // On Windows the overlay attach (HCN endpoint + per-container
                        // namespace creation, via overlayd) already happened inside
                        // `HcsRuntime::create_container`. Here we only need the IP it
                        // assigned so we can register DNS for service discovery.
                        let _ = (container_pid, &overlay_guard); // unused on Windows
                        match self.runtime.get_container_ip(&id).await {
                            Ok(Some(ip)) => Some(ip),
                            Ok(None) => {
                                tracing::debug!(
                                    container = %id,
                                    "no overlay IP recorded for container (overlay attach skipped at create time)"
                                );
                                None
                            }
                            Err(e) => {
                                tracing::warn!(
                                    container = %id,
                                    error = %e,
                                    "failed to fetch container overlay IP"
                                );
                                None
                            }
                        }
                    };
                    #[cfg(not(target_os = "windows"))]
                    let attach_result: Option<std::net::IpAddr> = {
                        if let Some(pid) = container_pid {
                            match overlay_guard
                                .attach_container(pid, &self.service_name, true)
                                .await
                            {
                                Ok(ip) => Some(ip),
                                Err(e) => {
                                    tracing::warn!(
                                        container = %id,
                                        error = %e,
                                        "failed to attach container to overlay network"
                                    );
                                    None
                                }
                            }
                        } else {
                            // No PID available (e.g. WASM runtime) - skip overlay attachment
                            tracing::debug!(
                                container = %id,
                                "skipping overlay attachment - no PID available"
                            );
                            None
                        }
                    };

                    if let Some(ip) = attach_result {
                        tracing::info!(
                            container = %id,
                            overlay_ip = %ip,
                            "attached container to overlay network"
                        );

                        // Register DNS for service discovery
                        if let Some(dns) = &self.dns_server {
                            // Register service-level hostname: {service}.service.local
                            let service_hostname = format!("{}.service.local", self.service_name);

                            // Register replica-specific hostname: {replica}.{service}.service.local
                            let replica_hostname =
                                format!("{}.{}.service.local", id.replica, self.service_name);

                            match dns.add_record(&service_hostname, ip).await {
                                Ok(()) => tracing::debug!(
                                    hostname = %service_hostname,
                                    ip = %ip,
                                    "registered DNS for service"
                                ),
                                Err(e) => tracing::warn!(
                                    hostname = %service_hostname,
                                    error = %e,
                                    "failed to register DNS for service"
                                ),
                            }

                            // Also register replica-specific entry
                            if let Err(e) = dns.add_record(&replica_hostname, ip).await {
                                tracing::warn!(
                                    hostname = %replica_hostname,
                                    error = %e,
                                    "failed to register replica DNS"
                                );
                            } else {
                                tracing::debug!(
                                    hostname = %replica_hostname,
                                    ip = %ip,
                                    "registered DNS for replica"
                                );
                            }

                            // Per-role DNS: register `{role}.{service}.service.local` when
                            // this container belongs to a non-default replica group. Lets
                            // intra-cluster clients reach a specific group (e.g.
                            // `read.db.service.local` for the read replicas of a postgres
                            // service with primary+read replica groups).
                            if id.role != "default" {
                                let role_hostname =
                                    format!("{}.{}.service.local", id.role, self.service_name);
                                match dns.add_record(&role_hostname, ip).await {
                                    Ok(()) => tracing::debug!(
                                        hostname = %role_hostname,
                                        ip = %ip,
                                        role = %id.role,
                                        "registered DNS for replica group role"
                                    ),
                                    Err(e) => tracing::warn!(
                                        hostname = %role_hostname,
                                        error = %e,
                                        "failed to register role DNS"
                                    ),
                                }
                            }
                        }

                        Some(ip)
                    } else {
                        None
                    }
                } else {
                    None
                };

                // If overlay failed, try the container runtime's own IP as fallback
                let effective_ip = if overlay_ip.is_none() {
                    match self.runtime.get_container_ip(&id).await {
                        Ok(Some(ip)) => {
                            tracing::info!(
                                container = %id,
                                ip = %ip,
                                "using runtime container IP for proxy (overlay unavailable)"
                            );
                            Some(ip)
                        }
                        Ok(None) => {
                            tracing::warn!(
                                container = %id,
                                "no container IP available from runtime, proxy routing will be unavailable"
                            );
                            None
                        }
                        Err(e) => {
                            tracing::warn!(
                                container = %id,
                                error = %e,
                                "failed to get container IP from runtime"
                            );
                            None
                        }
                    }
                } else {
                    overlay_ip
                };

                tracing::info!(
                    container = %id,
                    service = %self.service_name,
                    overlay_ip = ?overlay_ip,
                    effective_ip = ?effective_ip,
                    "Container IP resolution complete"
                );

                // Query port override from the runtime.
                // On macOS sandbox, each container is assigned a unique port since
                // all processes share the host network (no network namespaces).
                // The runtime passes the port to the process via the PORT env var.
                let port_override = match self.runtime.get_container_port_override(&id).await {
                    Ok(Some(port)) => {
                        tracing::info!(
                            container = %id,
                            port = port,
                            "runtime assigned dynamic port override for this container"
                        );
                        Some(port)
                    }
                    Ok(None) => None,
                    Err(e) => {
                        tracing::warn!(
                            container = %id,
                            error = %e,
                            "failed to query port override from runtime, using spec port"
                        );
                        None
                    }
                };

                // Start health monitoring and store handle (no lock needed during start)
                let health_monitor_handle = {
                    let mut check = self.spec.health.check.clone();

                    // Resolve Tcp { port: 0 } ("use first endpoint") to the actual
                    // port the container is listening on. With mac-sandbox, each
                    // replica gets a unique assigned port via port_override.
                    if let HealthCheck::Tcp { ref mut port } = check {
                        if *port == 0 {
                            *port = port_override.unwrap_or_else(|| {
                                self.spec
                                    .endpoints
                                    .iter()
                                    .find(|ep| {
                                        matches!(
                                            ep.protocol,
                                            Protocol::Http | Protocol::Https | Protocol::Websocket
                                        )
                                    })
                                    .map_or(8080, zlayer_spec::EndpointSpec::target_port)
                            });
                        }
                    }

                    let start_grace = self
                        .spec
                        .health
                        .start_grace
                        .unwrap_or(Duration::from_secs(5));
                    let check_timeout = self.spec.health.timeout.unwrap_or(Duration::from_secs(5));
                    let interval = self.spec.health.interval.unwrap_or(Duration::from_secs(10));
                    let retries = self.spec.health.retries;

                    let checker = HealthChecker::new(check, effective_ip);
                    let mut monitor = HealthMonitor::new(id.clone(), checker, interval, retries)
                        .with_start_grace(start_grace)
                        .with_check_timeout(check_timeout);

                    // Build the optional proxy backend handle. This is only present
                    // when both a proxy manager AND a reachable overlay IP exist; in
                    // degraded-overlay / no-proxy deployments it stays None and the
                    // callback below skips all proxy work while STILL bridging health
                    // state back into ServiceManager.
                    let proxy_backend: Option<(Arc<ProxyManager>, SocketAddr)> =
                        if let (Some(proxy), Some(ip)) = (&self.proxy_manager, effective_ip) {
                            let proxy = Arc::clone(proxy);
                            // Get the container's target port, using the runtime override if
                            // present. On macOS sandbox, port_override gives each replica a
                            // unique port so the proxy can distinguish backends sharing
                            // 127.0.0.1.
                            let port = port_override.unwrap_or_else(|| {
                                self.spec
                                    .endpoints
                                    .iter()
                                    .find(|ep| {
                                        matches!(
                                            ep.protocol,
                                            Protocol::Http | Protocol::Https | Protocol::Websocket
                                        )
                                    })
                                    .map_or(8080, zlayer_spec::EndpointSpec::target_port)
                            });

                            let backend_addr = SocketAddr::new(ip, port);

                            // Register backend with load balancer so proxy can route to it.
                            // This must happen before the health callback is created, because
                            // update_backend_health only updates *existing* backends.
                            proxy.add_backend(&self.service_name, backend_addr).await;

                            Some((proxy, backend_addr))
                        } else {
                            None
                        };

                    // The health bridge is ALWAYS attached, independent of proxy/IP
                    // availability. stabilization::wait_for_stabilization only treats a
                    // service as ready when health_states[name] == Healthy, so this write
                    // must happen even when the overlay is degraded and no proxy backend
                    // exists — otherwise the service stays healthy=false forever and
                    // stabilization times out.
                    let health_states_opt = self.health_states.clone();
                    let svc_name_for_states = self.service_name.clone();
                    let svc_name_for_proxy = self.service_name.clone();
                    let svc_name_for_log = self.service_name.clone();

                    let health_callback: HealthCallback =
                        Arc::new(move |container_id: ContainerId, is_healthy: bool| {
                            tracing::info!(
                                container = %container_id,
                                service = %svc_name_for_log,
                                healthy = is_healthy,
                                has_proxy_backend = proxy_backend.is_some(),
                                "health status changed"
                            );

                            // Always bridge health state back to ServiceManager's
                            // health_states map (unconditional — no proxy/IP required).
                            if let Some(ref health_states) = health_states_opt {
                                let states = Arc::clone(health_states);
                                let svc = svc_name_for_states.clone();
                                tokio::spawn(async move {
                                    let state = if is_healthy {
                                        HealthState::Healthy
                                    } else {
                                        HealthState::Unhealthy {
                                            failures: 0,
                                            reason: "health check failed".into(),
                                        }
                                    };
                                    states.write().await.insert(svc, state);
                                });
                            }

                            // Update proxy backend health only when a proxy backend was
                            // registered (proxy manager + reachable overlay IP present).
                            if let Some((proxy, backend_addr)) = proxy_backend.clone() {
                                let svc = svc_name_for_proxy.clone();
                                tokio::spawn(async move {
                                    proxy
                                        .update_backend_health(&svc, backend_addr, is_healthy)
                                        .await;
                                });
                            }
                        });

                    monitor = monitor.with_callback(health_callback);

                    monitor.start()
                };

                // Update state (short write lock)
                {
                    let mut containers = self.containers.write().await;
                    containers.insert(
                        id.clone(),
                        Container {
                            id: id.clone(),
                            image: self.spec.image.name.to_string(),
                            state: ContainerState::Running,
                            pid: None,
                            task: None,
                            overlay_ip: effective_ip,
                            health_monitor: Some(health_monitor_handle),
                            port_override,
                        },
                    );
                } // Lock released here
            }
        }

        // Phase 3: Scale down - remove containers (short write lock per removal)
        //
        // Containers were created with `with_role_and_node(role, local_node_id)`
        // on scale-up, so we must reconstruct the same identity on scale-down
        // — the role is derived from `replica_groups` via `role_for_replica`
        // and the node id is the local cluster node. Mismatched ids would miss
        // the live entry in `self.containers` and leak the container.
        if replicas < current_replicas {
            for i in replicas..current_replicas {
                let replica_idx = i + 1;
                let id = ContainerId::with_role_and_node(
                    self.service_name.clone(),
                    replica_idx,
                    self.role_for_replica(replica_idx),
                    local_node_id,
                );

                // Remove from state first and get the container to abort health monitor (short write lock)
                let removed_container = {
                    let mut containers = self.containers.write().await;
                    containers.remove(&id)
                }; // Lock released here

                // Then perform cleanup (no lock held - I/O operations)
                if let Some(container) = removed_container {
                    // Abort the health monitor task if it exists
                    if let Some(handle) = container.health_monitor {
                        handle.abort();
                    }

                    // Remove DNS records for this container
                    if let Some(dns) = &self.dns_server {
                        // Remove replica-specific DNS entry
                        let replica_hostname =
                            format!("{}.{}.service.local", id.replica, self.service_name);
                        if let Err(e) = dns.remove_record(&replica_hostname).await {
                            tracing::warn!(
                                hostname = %replica_hostname,
                                error = %e,
                                "failed to remove replica DNS record"
                            );
                        } else {
                            tracing::debug!(
                                hostname = %replica_hostname,
                                "removed replica DNS record"
                            );
                        }

                        // Remove per-role DNS entry if this was a non-default group.
                        // Note: this is best-effort and removes the record even if
                        // other replicas in the same role still need it — the DNS
                        // server's add/remove API is single-record so we can't keep
                        // it alive for siblings. P2.3-bis (round-robin per-role)
                        // can fix this later via a per-role refcount; for now the
                        // service-level hostname keeps cluster-internal clients
                        // working even when the role-specific record briefly
                        // disappears.
                        if id.role != "default" {
                            let role_hostname =
                                format!("{}.{}.service.local", id.role, self.service_name);
                            if let Err(e) = dns.remove_record(&role_hostname).await {
                                tracing::warn!(
                                    hostname = %role_hostname,
                                    error = %e,
                                    "failed to remove role DNS record"
                                );
                            } else {
                                tracing::debug!(
                                    hostname = %role_hostname,
                                    "removed role DNS record"
                                );
                            }
                        }

                        // Note: We don't remove the service-level hostname here because
                        // other replicas may still be using it. The service-level record
                        // should be cleaned up when the entire service is removed.
                    }

                    // Detach from overlay network if manager available.
                    //
                    // Done BEFORE stop_container because:
                    //   - The container init process must still be in
                    //     /proc to look up its PID via `get_container_pid`.
                    //   - `OverlayManager::detach_container` deletes host-side
                    //     veth interfaces by name (`veth-<pid>-*`) and
                    //     releases the allocated overlay IPs back to the
                    //     per-node slice. Without this the IPs leak across
                    //     container churn and the slice exhausts.
                    //
                    // Best-effort: failures are logged but never abort the
                    // scale-down. The periodic orphan sweep
                    // (`start_periodic_orphan_sweep`) catches anything we
                    // missed.
                    if let Some(overlay) = &self.overlay_manager {
                        match self.runtime.get_container_pid(&id).await {
                            Ok(Some(pid)) => {
                                let overlay_guard = overlay.read().await;
                                if let Err(e) = overlay_guard.detach_container(pid).await {
                                    tracing::warn!(
                                        container = %id,
                                        pid,
                                        error = %e,
                                        "overlay detach_container failed; relying on orphan sweep"
                                    );
                                }
                            }
                            Ok(None) => {
                                tracing::debug!(
                                    container = %id,
                                    "no PID available for overlay detach (already exited or non-Linux runtime)"
                                );
                            }
                            Err(e) => {
                                tracing::warn!(
                                    container = %id,
                                    error = %e,
                                    "failed to query container PID for overlay detach"
                                );
                            }
                        }
                    }

                    // Stop container
                    self.runtime
                        .stop_container(&id, Duration::from_secs(30))
                        .await?;

                    // Sync volumes to S3 before removal (no-op if not configured)
                    if let Err(e) = self.runtime.sync_container_volumes(&id).await {
                        tracing::warn!(
                            container = %id,
                            error = %e,
                            "failed to sync volumes before removal"
                        );
                    }

                    // Remove container
                    self.runtime.remove_container(&id).await?;
                }
            }
        }

        Ok(())
    }

    /// Get current number of replicas
    pub async fn replica_count(&self) -> usize {
        self.containers.read().await.len()
    }

    /// Get all container IDs
    pub async fn container_ids(&self) -> Vec<ContainerId> {
        self.containers.read().await.keys().cloned().collect()
    }

    /// Get per-container info (id, image, state, pid, overlay IP) for every
    /// live container in this instance.
    ///
    /// Surfaces the REAL image reference each container was created from and its
    /// REAL lifecycle state (lowercased via [`ContainerState::as_str`]) so the
    /// API/`ps` no longer reports a hardcoded `"running"` with no image.
    pub async fn container_infos(&self) -> Vec<ContainerInfo> {
        self.containers
            .read()
            .await
            .values()
            .map(|c| ContainerInfo {
                id: c.id.clone(),
                image: c.image.clone(),
                state: c.state.as_str().to_string(),
                pid: c.pid,
                overlay_ip: c.overlay_ip.map(|ip| ip.to_string()),
            })
            .collect()
    }

    /// Get read access to the containers map
    ///
    /// This allows callers to access container overlay IPs and other metadata
    /// without copying the entire map.
    pub fn containers(
        &self,
    ) -> &tokio::sync::RwLock<std::collections::HashMap<ContainerId, Container>> {
        &self.containers
    }

    /// Check if this service instance has an overlay manager configured
    pub fn has_overlay_manager(&self) -> bool {
        self.overlay_manager.is_some()
    }

    /// Check if this service instance has a proxy manager configured
    pub fn has_proxy_manager(&self) -> bool {
        self.proxy_manager.is_some()
    }

    /// Check if this service instance has a DNS server configured
    pub fn has_dns_server(&self) -> bool {
        self.dns_server.is_some()
    }
}

/// Per-container summary surfaced to callers (API / `ps`).
///
/// Carries the REAL image reference and lifecycle state of a single live
/// container, replacing the previous id-only view that forced the API to
/// fabricate a hardcoded `"running"` state with no image.
#[derive(Debug, Clone)]
pub struct ContainerInfo {
    /// Container identity.
    pub id: ContainerId,
    /// Image reference the container was created from (canonical form).
    pub image: String,
    /// Lowercased lifecycle state (e.g. `"running"`, `"exited"`).
    pub state: String,
    /// Process ID, when the container is running.
    pub pid: Option<u32>,
    /// Overlay IP rendered as a string, when assigned.
    pub overlay_ip: Option<String>,
}

/// Service manager for multiple services
pub struct ServiceManager {
    runtime: Arc<dyn Runtime + Send + Sync>,
    services: tokio::sync::RwLock<std::collections::HashMap<String, ServiceInstance>>,
    scale_semaphore: Arc<Semaphore>,
    /// Overlay network manager for container networking
    overlay_manager: Option<Arc<RwLock<OverlayManager>>>,
    /// Stream registry for L4 proxy route registration (TCP/UDP)
    stream_registry: Option<Arc<StreamRegistry>>,
    /// Proxy manager for health-aware load balancing (hyper-based proxy)
    proxy_manager: Option<Arc<ProxyManager>>,
    /// DNS server for service discovery
    dns_server: Option<Arc<DnsServer>>,
    /// Deployment name (used for generating hostnames)
    deployment_name: Option<String>,
    /// Health states for dependency condition checking
    health_states: Arc<RwLock<HashMap<String, HealthState>>>,
    /// Job executor for run-to-completion workloads
    job_executor: Option<Arc<JobExecutor>>,
    /// Cron scheduler for time-based job triggers
    cron_scheduler: Option<Arc<CronScheduler>>,
    /// Container supervisor for crash/panic policy enforcement
    container_supervisor: Option<Arc<ContainerSupervisor>>,
    /// Cluster membership + dispatch handle. When `None`, scale operations
    /// run purely local (single-node mode). When `Some`, `scale_service`
    /// routes through the cluster (leader dispatches to peers; followers
    /// forward to the leader).
    cluster: Option<Arc<dyn zlayer_scheduler::cluster::Cluster>>,
}

// ---------------------------------------------------------------------------
// ServiceManagerBuilder
// ---------------------------------------------------------------------------

/// Builder for constructing a [`ServiceManager`] with optional subsystems.
///
/// Prefer using `ServiceManager::builder(runtime)` to start building.
///
/// # Example
///
/// ```ignore
/// let manager = ServiceManager::builder(runtime)
///     .overlay_manager(om)
///     .proxy_manager(proxy)
///     .deployment_name("prod")
///     .build();
/// ```
pub struct ServiceManagerBuilder {
    runtime: Arc<dyn Runtime + Send + Sync>,
    overlay_manager: Option<Arc<RwLock<OverlayManager>>>,
    proxy_manager: Option<Arc<ProxyManager>>,
    stream_registry: Option<Arc<StreamRegistry>>,
    dns_server: Option<Arc<DnsServer>>,
    deployment_name: Option<String>,
    job_executor: Option<Arc<JobExecutor>>,
    cron_scheduler: Option<Arc<CronScheduler>>,
    container_supervisor: Option<Arc<ContainerSupervisor>>,
    cluster: Option<Arc<dyn zlayer_scheduler::cluster::Cluster>>,
}

impl ServiceManagerBuilder {
    /// Create a new builder with the required runtime.
    pub fn new(runtime: Arc<dyn Runtime + Send + Sync>) -> Self {
        Self {
            runtime,
            overlay_manager: None,
            proxy_manager: None,
            stream_registry: None,
            dns_server: None,
            deployment_name: None,
            job_executor: None,
            cron_scheduler: None,
            container_supervisor: None,
            cluster: None,
        }
    }

    /// Set the overlay network manager for container networking.
    #[must_use]
    pub fn overlay_manager(mut self, om: Arc<RwLock<OverlayManager>>) -> Self {
        self.overlay_manager = Some(om);
        self
    }

    /// Set the proxy manager for health-aware load balancing.
    #[must_use]
    pub fn proxy_manager(mut self, pm: Arc<ProxyManager>) -> Self {
        self.proxy_manager = Some(pm);
        self
    }

    /// Set the stream registry for TCP/UDP L4 proxy route registration.
    #[must_use]
    pub fn stream_registry(mut self, sr: Arc<StreamRegistry>) -> Self {
        self.stream_registry = Some(sr);
        self
    }

    /// Set the DNS server for service discovery.
    #[must_use]
    pub fn dns_server(mut self, dns: Arc<DnsServer>) -> Self {
        self.dns_server = Some(dns);
        self
    }

    /// Set the deployment name (used for hostname generation).
    #[must_use]
    pub fn deployment_name(mut self, name: impl Into<String>) -> Self {
        self.deployment_name = Some(name.into());
        self
    }

    /// Set the job executor for run-to-completion workloads.
    #[must_use]
    pub fn job_executor(mut self, je: Arc<JobExecutor>) -> Self {
        self.job_executor = Some(je);
        self
    }

    /// Set the cron scheduler for time-based job triggers.
    #[must_use]
    pub fn cron_scheduler(mut self, cs: Arc<CronScheduler>) -> Self {
        self.cron_scheduler = Some(cs);
        self
    }

    /// Set the container supervisor for crash/panic policy enforcement.
    #[must_use]
    pub fn container_supervisor(mut self, cs: Arc<ContainerSupervisor>) -> Self {
        self.container_supervisor = Some(cs);
        self
    }

    /// Set the cluster membership + dispatch handle. When set,
    /// [`ServiceManager::scale_service`] will route through the cluster
    /// (leader dispatches to peers; followers forward to the leader).
    /// When unset (the default), scale operations remain local-only.
    #[must_use]
    pub fn cluster(mut self, cluster: Arc<dyn zlayer_scheduler::cluster::Cluster>) -> Self {
        self.cluster = Some(cluster);
        self
    }

    /// Consume the builder and produce a fully-wired [`ServiceManager`].
    ///
    /// Logs warnings for missing recommended subsystems (proxy,
    /// `stream_registry`, `container_supervisor`, `deployment_name`).
    pub fn build(self) -> ServiceManager {
        if self.proxy_manager.is_none() {
            tracing::warn!("ServiceManager built without proxy_manager");
        }
        if self.stream_registry.is_none() {
            tracing::warn!("ServiceManager built without stream_registry");
        }
        if self.container_supervisor.is_none() {
            tracing::warn!("ServiceManager built without container_supervisor");
        }
        if self.deployment_name.is_none() {
            tracing::warn!("ServiceManager built without deployment_name");
        }

        ServiceManager {
            runtime: self.runtime,
            services: tokio::sync::RwLock::new(std::collections::HashMap::new()),
            scale_semaphore: Arc::new(Semaphore::new(10)),
            overlay_manager: self.overlay_manager,
            stream_registry: self.stream_registry,
            proxy_manager: self.proxy_manager,
            dns_server: self.dns_server,
            deployment_name: self.deployment_name,
            health_states: Arc::new(RwLock::new(HashMap::new())),
            job_executor: self.job_executor,
            cron_scheduler: self.cron_scheduler,
            container_supervisor: self.container_supervisor,
            cluster: self.cluster,
        }
    }
}

impl ServiceManager {
    /// Create a [`ServiceManagerBuilder`] for constructing a `ServiceManager`.
    ///
    /// This is the preferred way to construct a `ServiceManager` since v0.2.0.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let manager = ServiceManager::builder(runtime)
    ///     .overlay_manager(om)
    ///     .proxy_manager(proxy)
    ///     .build();
    /// ```
    pub fn builder(runtime: Arc<dyn Runtime + Send + Sync>) -> ServiceManagerBuilder {
        ServiceManagerBuilder::new(runtime)
    }

    /// Create a new service manager
    #[deprecated(since = "0.2.0", note = "use ServiceManager::builder() instead")]
    pub fn new(runtime: Arc<dyn Runtime + Send + Sync>) -> Self {
        Self {
            runtime,
            services: tokio::sync::RwLock::new(std::collections::HashMap::new()),
            scale_semaphore: Arc::new(Semaphore::new(10)), // Max 10 concurrent scaling operations
            overlay_manager: None,
            stream_registry: None,
            proxy_manager: None,
            dns_server: None,
            deployment_name: None,
            health_states: Arc::new(RwLock::new(HashMap::new())),
            job_executor: None,
            cron_scheduler: None,
            container_supervisor: None,
            cluster: None,
        }
    }

    /// Create a service manager with overlay network support
    #[deprecated(since = "0.2.0", note = "use ServiceManager::builder() instead")]
    pub fn with_overlay(
        runtime: Arc<dyn Runtime + Send + Sync>,
        overlay_manager: Arc<RwLock<OverlayManager>>,
    ) -> Self {
        Self {
            runtime,
            services: tokio::sync::RwLock::new(std::collections::HashMap::new()),
            scale_semaphore: Arc::new(Semaphore::new(10)),
            overlay_manager: Some(overlay_manager),
            stream_registry: None,
            proxy_manager: None,
            dns_server: None,
            deployment_name: None,
            health_states: Arc::new(RwLock::new(HashMap::new())),
            job_executor: None,
            cron_scheduler: None,
            container_supervisor: None,
            cluster: None,
        }
    }

    /// Create a fully-configured service manager with overlay and proxy support
    #[deprecated(since = "0.2.0", note = "use ServiceManager::builder() instead")]
    pub fn with_full_config(
        runtime: Arc<dyn Runtime + Send + Sync>,
        overlay_manager: Arc<RwLock<OverlayManager>>,
        deployment_name: String,
    ) -> Self {
        Self {
            runtime,
            services: tokio::sync::RwLock::new(std::collections::HashMap::new()),
            scale_semaphore: Arc::new(Semaphore::new(10)),
            overlay_manager: Some(overlay_manager),
            stream_registry: None,
            proxy_manager: None,
            dns_server: None,
            deployment_name: Some(deployment_name),
            health_states: Arc::new(RwLock::new(HashMap::new())),
            job_executor: None,
            cron_scheduler: None,
            container_supervisor: None,
            cluster: None,
        }
    }

    /// Get the health states map for external monitoring
    pub fn health_states(&self) -> Arc<RwLock<HashMap<String, HealthState>>> {
        Arc::clone(&self.health_states)
    }

    /// Update health state for a service
    pub async fn update_health_state(&self, service_name: &str, state: HealthState) {
        let mut states = self.health_states.write().await;
        states.insert(service_name.to_string(), state);
    }

    /// Set the deployment name (used for generating hostnames)
    #[deprecated(since = "0.2.0", note = "use ServiceManager::builder() instead")]
    pub fn set_deployment_name(&mut self, name: String) {
        self.deployment_name = Some(name);
    }

    /// Set the stream registry for L4 proxy integration (TCP/UDP)
    #[deprecated(since = "0.2.0", note = "use ServiceManager::builder() instead")]
    pub fn set_stream_registry(&mut self, registry: Arc<StreamRegistry>) {
        self.stream_registry = Some(registry);
    }

    /// Builder pattern: add stream registry for L4 proxy integration
    #[deprecated(since = "0.2.0", note = "use ServiceManager::builder() instead")]
    #[must_use]
    pub fn with_stream_registry(mut self, registry: Arc<StreamRegistry>) -> Self {
        self.stream_registry = Some(registry);
        self
    }

    /// Get the stream registry (if configured)
    pub fn stream_registry(&self) -> Option<&Arc<StreamRegistry>> {
        self.stream_registry.as_ref()
    }

    /// Set the overlay manager for container networking
    #[deprecated(since = "0.2.0", note = "use ServiceManager::builder() instead")]
    pub fn set_overlay_manager(&mut self, manager: Arc<RwLock<OverlayManager>>) {
        self.overlay_manager = Some(manager);
    }

    /// Set the proxy manager for health-aware load balancing
    #[deprecated(since = "0.2.0", note = "use ServiceManager::builder() instead")]
    pub fn set_proxy_manager(&mut self, proxy: Arc<ProxyManager>) {
        self.proxy_manager = Some(proxy);
    }

    /// Builder pattern: add proxy manager for health-aware load balancing
    #[deprecated(since = "0.2.0", note = "use ServiceManager::builder() instead")]
    #[must_use]
    pub fn with_proxy_manager(mut self, proxy: Arc<ProxyManager>) -> Self {
        self.proxy_manager = Some(proxy);
        self
    }

    /// Get the proxy manager (if configured)
    pub fn proxy_manager(&self) -> Option<&Arc<ProxyManager>> {
        self.proxy_manager.as_ref()
    }

    /// Set the DNS server for service discovery
    #[deprecated(since = "0.2.0", note = "use ServiceManager::builder() instead")]
    pub fn set_dns_server(&mut self, dns: Arc<DnsServer>) {
        self.dns_server = Some(dns);
    }

    /// Builder pattern: add DNS server for service discovery
    #[deprecated(since = "0.2.0", note = "use ServiceManager::builder() instead")]
    #[must_use]
    pub fn with_dns_server(mut self, dns: Arc<DnsServer>) -> Self {
        self.dns_server = Some(dns);
        self
    }

    /// Get the DNS server (if configured)
    pub fn dns_server(&self) -> Option<&Arc<DnsServer>> {
        self.dns_server.as_ref()
    }

    /// Set the job executor for run-to-completion workloads
    #[deprecated(since = "0.2.0", note = "use ServiceManager::builder() instead")]
    pub fn set_job_executor(&mut self, executor: Arc<JobExecutor>) {
        self.job_executor = Some(executor);
    }

    /// Set the cron scheduler for time-based job triggers
    #[deprecated(since = "0.2.0", note = "use ServiceManager::builder() instead")]
    pub fn set_cron_scheduler(&mut self, scheduler: Arc<CronScheduler>) {
        self.cron_scheduler = Some(scheduler);
    }

    /// Builder pattern: add job executor
    #[deprecated(since = "0.2.0", note = "use ServiceManager::builder() instead")]
    #[must_use]
    pub fn with_job_executor(mut self, executor: Arc<JobExecutor>) -> Self {
        self.job_executor = Some(executor);
        self
    }

    /// Builder pattern: add cron scheduler
    #[deprecated(since = "0.2.0", note = "use ServiceManager::builder() instead")]
    #[must_use]
    pub fn with_cron_scheduler(mut self, scheduler: Arc<CronScheduler>) -> Self {
        self.cron_scheduler = Some(scheduler);
        self
    }

    /// Set the cluster handle for cluster-aware scaling.
    #[deprecated(since = "0.2.0", note = "use ServiceManager::builder() instead")]
    pub fn set_cluster(&mut self, cluster: Arc<dyn zlayer_scheduler::cluster::Cluster>) {
        self.cluster = Some(cluster);
    }

    /// Builder pattern: add a cluster handle for cluster-aware scaling.
    #[deprecated(since = "0.2.0", note = "use ServiceManager::builder() instead")]
    #[must_use]
    pub fn with_cluster(mut self, cluster: Arc<dyn zlayer_scheduler::cluster::Cluster>) -> Self {
        self.cluster = Some(cluster);
        self
    }

    /// Get the cluster handle (if configured).
    pub fn cluster(&self) -> Option<&Arc<dyn zlayer_scheduler::cluster::Cluster>> {
        self.cluster.as_ref()
    }

    /// Get the job executor (if configured)
    pub fn job_executor(&self) -> Option<&Arc<JobExecutor>> {
        self.job_executor.as_ref()
    }

    /// Get the cron scheduler (if configured)
    pub fn cron_scheduler(&self) -> Option<&Arc<CronScheduler>> {
        self.cron_scheduler.as_ref()
    }

    /// Set the container supervisor for crash/panic policy enforcement
    #[deprecated(since = "0.2.0", note = "use ServiceManager::builder() instead")]
    pub fn set_container_supervisor(&mut self, supervisor: Arc<ContainerSupervisor>) {
        self.container_supervisor = Some(supervisor);
    }

    /// Builder pattern: add container supervisor
    #[deprecated(since = "0.2.0", note = "use ServiceManager::builder() instead")]
    #[must_use]
    pub fn with_container_supervisor(mut self, supervisor: Arc<ContainerSupervisor>) -> Self {
        self.container_supervisor = Some(supervisor);
        self
    }

    /// Get the container supervisor (if configured)
    pub fn container_supervisor(&self) -> Option<&Arc<ContainerSupervisor>> {
        self.container_supervisor.as_ref()
    }

    /// Start the container supervisor background task
    ///
    /// This spawns a background task that monitors containers for crashes
    /// and enforces the `on_panic` error policy.
    ///
    /// # Errors
    /// Returns an error if no container supervisor is configured.
    ///
    /// # Returns
    /// A `JoinHandle` for the supervisor task.
    pub fn start_container_supervisor(&self) -> Result<tokio::task::JoinHandle<()>> {
        let supervisor = self.container_supervisor.as_ref().ok_or_else(|| {
            AgentError::Configuration("Container supervisor not configured".to_string())
        })?;

        let supervisor = Arc::clone(supervisor);
        Ok(tokio::spawn(async move {
            supervisor.run_loop().await;
        }))
    }

    /// Shutdown the container supervisor
    pub fn shutdown_container_supervisor(&self) {
        if let Some(supervisor) = &self.container_supervisor {
            supervisor.shutdown();
        }
    }

    /// Get the supervised state of a container
    pub async fn get_container_supervised_state(
        &self,
        container_id: &ContainerId,
    ) -> Option<SupervisedState> {
        if let Some(supervisor) = &self.container_supervisor {
            supervisor.get_state(container_id).await
        } else {
            None
        }
    }

    /// Get supervisor events receiver
    ///
    /// Note: This can only be called once; the receiver is moved to the caller.
    pub async fn take_supervisor_events(
        &self,
    ) -> Option<tokio::sync::mpsc::Receiver<SupervisorEvent>> {
        if let Some(supervisor) = &self.container_supervisor {
            supervisor.take_event_receiver().await
        } else {
            None
        }
    }

    // ==================== Dependency Orchestration ====================

    /// Deploy multiple services respecting their dependency order
    ///
    /// This method:
    /// 1. Builds a dependency graph from the services
    /// 2. Validates no cycles exist
    /// 3. Computes topological order (services with no deps first)
    /// 4. For each service in order, waits for dependencies then starts the service
    ///
    /// # Arguments
    /// * `services` - Map of service name to service specification
    ///
    /// # Errors
    /// - Returns `AgentError::InvalidSpec` if there are cyclic dependencies
    /// - Returns `AgentError::DependencyTimeout` if a dependency times out with `on_timeout`: fail
    pub async fn deploy_with_dependencies(
        &self,
        services: HashMap<String, ServiceSpec>,
    ) -> Result<()> {
        if services.is_empty() {
            return Ok(());
        }

        // Build dependency graph
        let graph = DependencyGraph::build(&services)?;

        tracing::info!(
            service_count = services.len(),
            "Starting deployment with dependency ordering"
        );

        // Get startup order
        let order = graph.startup_order();
        tracing::debug!(order = ?order, "Computed startup order");

        // Start services in dependency order
        for service_name in order {
            let service_spec = services
                .get(service_name)
                .ok_or_else(|| AgentError::Internal(format!("Service {service_name} not found")))?;

            // Wait for dependencies first
            if !service_spec.depends.is_empty() {
                tracing::info!(
                    service = %service_name,
                    dependency_count = service_spec.depends.len(),
                    "Waiting for dependencies"
                );
                self.wait_for_dependencies(service_name, &service_spec.depends)
                    .await?;
            }

            // Register and start service
            tracing::info!(service = %service_name, "Starting service");
            Box::pin(self.upsert_service(service_name.clone(), service_spec.clone())).await?;

            // Get the desired replica count from scale config
            let replicas = match &service_spec.scale {
                zlayer_spec::ScaleSpec::Fixed { replicas } => *replicas,
                zlayer_spec::ScaleSpec::Adaptive { min, .. } => *min, // Start with min replicas
                zlayer_spec::ScaleSpec::Manual => 1, // Default to 1 for manual scaling
            };
            self.scale_service(service_name, replicas).await?;

            // Mark service as started in health states (Unknown until health check runs)
            self.update_health_state(service_name, HealthState::Unknown)
                .await;

            tracing::info!(
                service = %service_name,
                replicas = replicas,
                "Service started"
            );
        }

        tracing::info!(service_count = services.len(), "Deployment complete");

        Ok(())
    }

    /// Wait for all dependencies of a service to be satisfied
    ///
    /// # Arguments
    /// * `service` - Name of the service waiting for dependencies
    /// * `deps` - Slice of dependency specifications
    ///
    /// # Errors
    /// Returns `AgentError::DependencyTimeout` if any dependency with `on_timeout`: fail times out
    async fn wait_for_dependencies(&self, service: &str, deps: &[DependsSpec]) -> Result<()> {
        let condition_checker = DependencyConditionChecker::new(
            Arc::clone(&self.runtime),
            Arc::clone(&self.health_states),
            None,
        );

        let waiter = DependencyWaiter::new(condition_checker);
        let results = waiter.wait_for_all(deps).await?;

        // Check results for failures
        for result in results {
            match result {
                WaitResult::TimedOutFail {
                    service: dep_service,
                    condition,
                    timeout,
                } => {
                    return Err(AgentError::DependencyTimeout {
                        service: service.to_string(),
                        dependency: dep_service,
                        condition: format!("{condition:?}"),
                        timeout,
                    });
                }
                WaitResult::TimedOutWarn {
                    service: dep_service,
                    condition,
                } => {
                    tracing::warn!(
                        service = %service,
                        dependency = %dep_service,
                        condition = ?condition,
                        "Dependency timed out but continuing"
                    );
                }
                WaitResult::TimedOutContinue | WaitResult::Satisfied => {
                    // Continue silently
                }
            }
        }

        Ok(())
    }

    /// Check if all dependencies for a service are currently satisfied
    ///
    /// This is a one-shot check (no waiting). Useful for pre-flight validation.
    ///
    /// # Errors
    /// Returns an error if a dependency check fails unexpectedly.
    pub async fn check_dependencies(&self, deps: &[DependsSpec]) -> Result<bool> {
        let condition_checker = DependencyConditionChecker::new(
            Arc::clone(&self.runtime),
            Arc::clone(&self.health_states),
            None,
        );

        for dep in deps {
            if !condition_checker.check(dep).await? {
                return Ok(false);
            }
        }

        Ok(true)
    }

    /// Add or update a workload (service, job, or cron)
    ///
    /// This method handles different resource types appropriately:
    /// - **Service**: Traditional long-running containers with scaling and health checks
    /// - **Job**: Run-to-completion workloads triggered on-demand (stores spec for later)
    /// - **Cron**: Scheduled run-to-completion workloads (registers with cron scheduler)
    ///
    /// # Errors
    /// Returns an error if service creation, scaling, or cron registration fails.
    #[allow(clippy::too_many_lines)]
    pub async fn upsert_service(&self, name: String, spec: ServiceSpec) -> Result<()> {
        match spec.rtype {
            ResourceType::Service => {
                // Long-running service: create/update instance
                let mut services = self.services.write().await;

                if let Some(instance) = services.get_mut(&name) {
                    // Update existing service. We need to:
                    //   1. Update the in-memory spec (so future scale-ups use the new image).
                    //   2. Recreate the local replicas when the image actually changed —
                    //      either a different image *reference* (e.g. tag bump
                    //      1.28 -> 1.29), which is a new image regardless of pull
                    //      policy, or, under Always/Newer, observed *digest* drift on
                    //      the same reference.
                    // The recreate is LOCAL (`scale_service_local`): `upsert_service`
                    // runs on whichever node owns the replicas (the leader for its
                    // own share, each worker via the `/internal/scale` handler). Using
                    // the cluster-routed `scale_service` here would bounce a worker's
                    // recreate back to the leader and re-enter dispatch. Cluster-wide
                    // distribution is the caller's job (orchestrate_deployment + the
                    // scale dispatch that carries this spec to every node).
                    let image_changed = instance.spec.image.name != spec.image.name;
                    instance.spec = spec.clone();
                    if let Some(dns) = &self.dns_server {
                        instance.set_dns_server(Arc::clone(dns));
                    }

                    let effective = spec.image.pull_policy;
                    let old_digest = instance.last_pulled_digest().await;
                    let current_replicas =
                        u32::try_from(instance.replica_count().await).unwrap_or(u32::MAX);
                    drop(services); // Release write lock before pull / scale (which take their own locks).

                    // A changed image reference always recreates. Same-reference
                    // refreshes are governed by pull policy + digest drift.
                    let mut should_recreate = image_changed;
                    let mut new_digest = old_digest.clone();

                    match effective {
                        PullPolicy::Never | PullPolicy::IfNotPresent => {
                            // No proactive pull. If the reference changed we still
                            // recreate below; the scale-up path pulls the (absent) new
                            // image per IfNotPresent. A same-reference redeploy under
                            // these policies is a genuine no-op.
                            tracing::debug!(
                                service = %name,
                                policy = ?effective,
                                image_changed,
                                "re-deploy under no-refresh pull policy"
                            );
                        }
                        PullPolicy::Always | PullPolicy::Newer => {
                            // Pull (this updates the cached digest as a side-effect).
                            // We need a read guard to keep the instance alive while
                            // calling its &self method.
                            let services_ro = self.services.read().await;
                            new_digest = if let Some(inst) = services_ro.get(&name) {
                                inst.pull_and_refresh_digest().await?
                            } else {
                                // The service vanished between our write-lock release
                                // and read-lock acquisition (race with remove_service).
                                // Treat this as a no-op; the caller will see the removal.
                                tracing::warn!(
                                    service = %name,
                                    "service removed during upsert; skipping drift recreate"
                                );
                                return Ok(());
                            };
                            drop(services_ro);

                            // Always forces a recreate. Newer recreates on digest
                            // drift. When digests are unknown (runtime doesn't expose
                            // them), we can't observe drift safely under Newer, so the
                            // reference check above is the only trigger.
                            should_recreate = should_recreate
                                || match effective {
                                    PullPolicy::Always => true,
                                    PullPolicy::Newer => match (&old_digest, &new_digest) {
                                        (Some(old), Some(new)) => old != new,
                                        _ => false,
                                    },
                                    _ => false,
                                };
                        }
                    }

                    if should_recreate && current_replicas > 0 {
                        tracing::info!(
                            service = %name,
                            policy = ?effective,
                            image_changed,
                            old_digest = ?old_digest,
                            new_digest = ?new_digest,
                            replicas = current_replicas,
                            "image changed; performing local rolling recreate"
                        );
                        self.scale_service_local(&name, 0).await?;
                        self.scale_service_local(&name, current_replicas).await?;
                        tracing::info!(
                            service = %name,
                            new_digest = ?new_digest,
                            "service recreated with refreshed image"
                        );
                    } else {
                        tracing::debug!(
                            service = %name,
                            policy = ?effective,
                            old_digest = ?old_digest,
                            new_digest = ?new_digest,
                            "service up to date; no recreate required"
                        );
                    }
                    return Ok(());
                }
                // Create new service with proxy manager for health-aware load balancing
                let overlay = self.overlay_manager.as_ref().map(Arc::clone);
                let mut instance = if let Some(proxy) = &self.proxy_manager {
                    ServiceInstance::with_proxy(
                        name.clone(),
                        spec,
                        self.runtime.clone(),
                        overlay,
                        Arc::clone(proxy),
                    )
                } else {
                    ServiceInstance::new(name.clone(), spec, self.runtime.clone(), overlay)
                };
                // Thread the local cluster node id so new `ContainerId`s carry
                // owning-node identity. Defaults to `0` in single-node mode.
                instance.set_node_id(self.cluster.as_ref().map_or(0, |c| c.node_id()));
                // Set DNS server if configured
                if let Some(dns) = &self.dns_server {
                    instance.set_dns_server(Arc::clone(dns));
                }
                // Wire shared health states so callbacks bridge back to ServiceManager
                instance.set_health_states(Arc::clone(&self.health_states));
                // Register HTTP routes via proxy manager
                if let Some(proxy) = &self.proxy_manager {
                    proxy.add_service(&name, &instance.spec).await;
                }
                // Register TCP/UDP endpoints in stream registry
                if let Some(stream_registry) = &self.stream_registry {
                    for endpoint in &instance.spec.endpoints {
                        let svc = StreamService::new(
                            name.clone(),
                            Vec::new(), // No backends yet; added on scale-up
                        );
                        match endpoint.protocol {
                            Protocol::Tcp => {
                                stream_registry.register_tcp(endpoint.port, svc);
                                tracing::debug!(
                                    service = %name,
                                    port = endpoint.port,
                                    "Registered TCP stream route"
                                );
                            }
                            Protocol::Udp => {
                                stream_registry.register_udp(endpoint.port, svc);
                                tracing::debug!(
                                    service = %name,
                                    port = endpoint.port,
                                    "Registered UDP stream route"
                                );
                            }
                            _ => {} // HTTP routes handled by proxy manager
                        }
                    }
                }
                services.insert(name, instance);
            }
            ResourceType::Job => {
                // Job: Just store the spec for later triggering
                // Jobs don't start containers immediately; they're triggered on-demand
                if let Some(executor) = &self.job_executor {
                    executor.register_job(&name, spec).await;
                    tracing::info!(job = %name, "Registered job spec");
                } else {
                    tracing::warn!(
                        job = %name,
                        "Job executor not configured, storing as service for reference"
                    );
                    // Fallback: store as service instance for reference
                    let mut services = self.services.write().await;
                    let overlay = self.overlay_manager.as_ref().map(Arc::clone);
                    let mut instance = if let Some(proxy) = &self.proxy_manager {
                        ServiceInstance::with_proxy(
                            name.clone(),
                            spec,
                            self.runtime.clone(),
                            overlay,
                            Arc::clone(proxy),
                        )
                    } else {
                        ServiceInstance::new(name.clone(), spec, self.runtime.clone(), overlay)
                    };
                    // Thread the local cluster node id (same as the Service
                    // branch above) so the fallback-as-service Job entry also
                    // carries owning-node identity.
                    instance.set_node_id(self.cluster.as_ref().map_or(0, |c| c.node_id()));
                    // Set DNS server if configured
                    if let Some(dns) = &self.dns_server {
                        instance.set_dns_server(Arc::clone(dns));
                    }
                    services.insert(name, instance);
                }
            }
            ResourceType::Cron => {
                // Cron: Register with the cron scheduler
                if let Some(scheduler) = &self.cron_scheduler {
                    scheduler.register(&name, &spec).await?;
                    tracing::info!(cron = %name, "Registered cron job with scheduler");
                } else {
                    return Err(AgentError::Configuration(format!(
                        "Cron scheduler not configured for cron job '{name}'"
                    )));
                }
            }
        }

        Ok(())
    }

    /// Update backend addresses via `ProxyManager` after scaling, applying
    /// per-endpoint `target_role` filtering.
    ///
    /// For each L7 endpoint of the service, this collects the subset of
    /// containers whose `ContainerId.role` matches `endpoint.target_role`
    /// (or all containers when `target_role` is `None`) and updates the
    /// proxy's backend pool for that specific endpoint via
    /// [`ProxyManager::update_endpoint_backends`].
    async fn update_proxy_backends(&self, instance: &ServiceInstance) {
        let Some(proxy) = &self.proxy_manager else {
            return;
        };
        for endpoint in &instance.spec.endpoints {
            // Only L7 endpoints flow through the proxy (HTTP/HTTPS/WS).
            if !matches!(
                endpoint.protocol,
                Protocol::Http | Protocol::Https | Protocol::Websocket
            ) {
                continue;
            }
            let addrs = self.collect_endpoint_backends(instance, endpoint).await;
            proxy
                .update_endpoint_backends(&instance.service_name, &endpoint.name, addrs)
                .await;
        }
    }

    /// Update backend addresses in the `StreamRegistry` for TCP/UDP endpoints after scaling
    ///
    /// For containers with a port override (macOS sandbox), the addresses already
    /// carry the runtime-assigned port. In that case, the container listens on the
    /// override port for all traffic, so we use the address port directly. For
    /// containers without a port override (Linux, VMs), we reconstruct addresses
    /// using the endpoint's declared port, since each container has its own IP
    /// and can bind any port independently.
    async fn update_stream_backends(&self, instance: &ServiceInstance) {
        let Some(stream_registry) = &self.stream_registry else {
            return;
        };

        for endpoint in &instance.spec.endpoints {
            match endpoint.protocol {
                Protocol::Tcp => {
                    let tcp_backends = self.collect_endpoint_backends(instance, endpoint).await;
                    let backend_count = tcp_backends.len();
                    stream_registry.update_tcp_backends(endpoint.port, tcp_backends);
                    tracing::debug!(
                        endpoint = %endpoint.name,
                        port = endpoint.port,
                        backend_count = backend_count,
                        target_role = ?endpoint.target_role,
                        "Updated TCP stream backends"
                    );
                }
                Protocol::Udp => {
                    let udp_backends = self.collect_endpoint_backends(instance, endpoint).await;
                    let backend_count = udp_backends.len();
                    stream_registry.update_udp_backends(endpoint.port, udp_backends);
                    tracing::debug!(
                        endpoint = %endpoint.name,
                        port = endpoint.port,
                        backend_count = backend_count,
                        target_role = ?endpoint.target_role,
                        "Updated UDP stream backends"
                    );
                }
                _ => {} // HTTP endpoints handled by update_proxy_backends
            }
        }
    }

    /// Scale a service. Cluster-aware: if this node has a `Cluster` handle
    /// and we're not the leader, forward to the leader; if leader, dispatch
    /// via the cluster's placement layer (Phase 1 sends to every peer that
    /// gets a share); else (single-node) just scale locally.
    ///
    /// # Errors
    /// Returns an error if scaling fails on any participating node.
    #[allow(clippy::cast_possible_truncation)]
    pub async fn scale_service(&self, name: &str, replicas: u32) -> Result<()> {
        use zlayer_scheduler::cluster::InternalScaleRequest;

        // Attach the current spec so every receiving node can register/update
        // the service before scaling. This is what propagates an image change
        // to worker containers and lets a fresh worker run a replica it has
        // never seen. `None` if the service isn't registered locally (the
        // receiver then falls back to its own cached spec).
        let spec = self
            .services
            .read()
            .await
            .get(name)
            .map(|inst| inst.spec.clone());
        let build_req = |replicas: u32| {
            let req = InternalScaleRequest::new(name, replicas);
            match spec.clone() {
                Some(s) => req.with_spec(s),
                None => req,
            }
        };

        if let Some(cluster) = &self.cluster {
            if !cluster.is_leader().await {
                // Follower: forward to the leader and let it dispatch.
                return cluster
                    .forward_scale(build_req(replicas))
                    .await
                    .map_err(|e| AgentError::CreateFailed {
                        id: name.to_string(),
                        reason: format!("cluster forward: {e}"),
                    });
            }

            // Leader path. For Phase 1 we keep the placement logic in the
            // scheduler layer (called externally); here we just send the
            // legacy `{service, replicas}` shape to every node and let the
            // scheduler fan it out. The scheduler-side wrapper handles the
            // actual per-node split — that lands in a follow-up.
            //
            // In Phase 1 single-cluster setups, leader dispatch reduces to
            // `dispatch_scale(self_node_id, req)` which short-circuits to
            // local. The full scheduler-driven fan-out wires up once
            // `Scheduler::scale_service_distributed` is exposed.
            return cluster
                .dispatch_scale(cluster.node_id(), build_req(replicas))
                .await
                .map_err(|e| AgentError::CreateFailed {
                    id: name.to_string(),
                    reason: format!("cluster dispatch: {e}"),
                });
        }

        // No cluster handle — single-node mode.
        self.scale_service_local(name, replicas).await
    }

    /// Local (single-node) scale: directly creates/destroys containers on
    /// this node only. Called by:
    ///   - `scale_service` in single-node mode (when `self.cluster` is None).
    ///   - The `/api/v1/internal/scale` handler (which the leader's
    ///     `Cluster::dispatch_scale` HTTP-POSTs to, bottoming out the
    ///     recursive loop on each receiving node).
    ///   - The cluster impls' `local_dispatch` closure (for the leader's own
    ///     share — short-circuited to avoid a localhost round-trip).
    ///
    /// # Errors
    /// Returns an error if the service is not found or scaling fails.
    #[allow(clippy::cast_possible_truncation)]
    pub async fn scale_service_local(&self, name: &str, replicas: u32) -> Result<()> {
        let _permit = self.scale_semaphore.acquire().await;

        let services = self.services.read().await;
        let instance = services.get(name).ok_or_else(|| AgentError::NotFound {
            container: name.to_string(),
            reason: "service not found".to_string(),
        })?;

        // Get current replica count before scaling
        let current_replicas = instance.replica_count().await as u32;

        // Perform the scaling operation
        instance.scale_to(replicas).await?;

        // After scaling, update proxy and stream backends for each endpoint.
        // Per-endpoint collection (rather than a single service-wide list)
        // is what makes `EndpointSpec.target_role` filtering possible:
        // each endpoint receives only the containers whose
        // `ContainerId.role` matches its declared role.
        if self.proxy_manager.is_some() {
            self.update_proxy_backends(instance).await;
        }
        if self.stream_registry.is_some() {
            self.update_stream_backends(instance).await;
        }

        // Register new containers with supervisor for crash monitoring.
        //
        // Container ids here must match what `ServiceInstance::scale_to`
        // constructed — same role (derived from `replica_groups`) and same
        // local node id. Otherwise supervise/unsupervise miss the live entry
        // and crash-restart bookkeeping leaks across scale events.
        let local_node_id = self.cluster.as_ref().map_or(0, |c| c.node_id());
        if let Some(supervisor) = &self.container_supervisor {
            // For scale-up, register new containers
            if replicas > current_replicas {
                for i in current_replicas..replicas {
                    let replica_idx = i + 1;
                    let container_id = ContainerId::with_role_and_node(
                        name.to_string(),
                        replica_idx,
                        instance.role_for_replica(replica_idx),
                        local_node_id,
                    );
                    supervisor.supervise(&container_id, &instance.spec).await;
                }
            }
            // For scale-down, unregister removed containers
            if replicas < current_replicas {
                for i in replicas..current_replicas {
                    let replica_idx = i + 1;
                    let container_id = ContainerId::with_role_and_node(
                        name.to_string(),
                        replica_idx,
                        instance.role_for_replica(replica_idx),
                        local_node_id,
                    );
                    supervisor.unsupervise(&container_id).await;
                }
            }
        }

        Ok(())
    }

    /// Collect backend addresses for a single endpoint of a service.
    ///
    /// This queries the service instance's containers for their overlay
    /// network IP addresses and constructs backend addresses using the
    /// endpoint's container target port.
    ///
    /// Containers are filtered by `endpoint.target_role`:
    /// - `None` (default): all containers of the service are eligible
    ///   (legacy behavior).
    /// - `Some(role)`: only containers whose `ContainerId.role` equals
    ///   `role` are included. Implements
    ///   [`zlayer_spec::EndpointSpec::target_role`].
    ///
    /// If a container has a `port_override` (e.g., macOS sandbox where all
    /// containers share the host network), that port is used instead of
    /// the spec-declared endpoint port. This allows multiple replicas on
    /// the same IP (`127.0.0.1`) to be distinguished by port.
    async fn collect_endpoint_backends(
        &self,
        instance: &ServiceInstance,
        endpoint: &zlayer_spec::EndpointSpec,
    ) -> Vec<SocketAddr> {
        let mut addrs = Vec::new();
        let endpoint_port = endpoint.target_port();
        let containers = instance.containers().read().await;

        for (container_id, container) in containers.iter() {
            // target_role filter: skip containers whose role doesn't match.
            if let Some(required_role) = endpoint.target_role.as_ref() {
                if container_id.role != *required_role {
                    continue;
                }
            }
            let Some(ip) = container.overlay_ip else {
                continue;
            };
            // Use the runtime-assigned port override if present (macOS
            // sandbox), otherwise fall back to the endpoint's declared
            // target port.
            let port = container.port_override.unwrap_or(endpoint_port);
            addrs.push(SocketAddr::new(ip, port));
        }

        // If we expected backends but found none, log a hint so operators
        // can debug. Distinguish "no containers" from "role filter
        // excluded everything" from "no overlay IPs".
        if addrs.is_empty() && !containers.is_empty() {
            tracing::warn!(
                service = %instance.service_name,
                endpoint = %endpoint.name,
                target_role = ?endpoint.target_role,
                container_count = containers.len(),
                "no backends collected for endpoint - either no matching role, no overlay IPs, or filtering excluded all"
            );
        }

        addrs
    }

    /// Get service replica count
    ///
    /// # Errors
    /// Returns an error if the service is not found.
    pub async fn service_replica_count(&self, name: &str) -> Result<usize> {
        let services = self.services.read().await;
        let instance = services.get(name).ok_or_else(|| AgentError::NotFound {
            container: name.to_string(),
            reason: "service not found".to_string(),
        })?;

        Ok(instance.replica_count().await)
    }

    /// Remove a workload (service, job, or cron)
    ///
    /// This method handles cleanup for different resource types:
    /// - **Service**: Unregisters proxy routes, supervisor, and removes from service map
    /// - **Job**: Unregisters from job executor
    /// - **Cron**: Unregisters from cron scheduler
    ///
    /// # Errors
    /// Returns an error if the service cannot be removed or scale-down fails.
    pub async fn remove_service(&self, name: &str) -> Result<()> {
        // Try to unregister from cron scheduler first
        if let Some(scheduler) = &self.cron_scheduler {
            scheduler.unregister(name).await;
        }

        // Try to unregister from job executor
        if let Some(executor) = &self.job_executor {
            executor.unregister_job(name).await;
        }

        // Unregister stream routes (TCP/UDP) from the stream registry
        if let Some(stream_registry) = &self.stream_registry {
            // Need to get the service spec to know which ports to unregister
            let services = self.services.read().await;
            if let Some(instance) = services.get(name) {
                for endpoint in &instance.spec.endpoints {
                    match endpoint.protocol {
                        Protocol::Tcp => {
                            let _ = stream_registry.unregister_tcp(endpoint.port);
                            tracing::debug!(
                                service = %name,
                                port = endpoint.port,
                                "Unregistered TCP stream route"
                            );
                        }
                        Protocol::Udp => {
                            let _ = stream_registry.unregister_udp(endpoint.port);
                            tracing::debug!(
                                service = %name,
                                port = endpoint.port,
                                "Unregistered UDP stream route"
                            );
                        }
                        _ => {} // HTTP routes handled above
                    }
                }
            }
            drop(services); // Release read lock
        }

        // Unregister containers from the supervisor
        if let Some(supervisor) = &self.container_supervisor {
            let containers = self.get_service_containers(name).await;
            for container_id in containers {
                supervisor.unsupervise(&container_id).await;
            }
            tracing::debug!(service = %name, "Unregistered containers from supervisor");
        }

        // Clean up DNS records for the service
        if let Some(dns) = &self.dns_server {
            // Remove the service-level DNS entry
            let service_hostname = format!("{name}.service.local");
            if let Err(e) = dns.remove_record(&service_hostname).await {
                tracing::warn!(
                    hostname = %service_hostname,
                    error = %e,
                    "failed to remove service DNS record"
                );
            } else {
                tracing::debug!(
                    hostname = %service_hostname,
                    "removed service DNS record"
                );
            }

            // Also remove any remaining replica-specific DNS entries
            let services = self.services.read().await;
            if let Some(instance) = services.get(name) {
                let containers = instance.containers().read().await;
                for (id, _) in containers.iter() {
                    let replica_hostname = format!("{}.{}.service.local", id.replica, name);
                    if let Err(e) = dns.remove_record(&replica_hostname).await {
                        tracing::warn!(
                            hostname = %replica_hostname,
                            error = %e,
                            "failed to remove replica DNS record during service removal"
                        );
                    }
                }
            }
            drop(services); // Release read lock before write lock
        }

        // Remove from services map (may or may not exist depending on rtype)
        let mut services = self.services.write().await;
        if services.remove(name).is_some() {
            tracing::debug!(service = %name, "Removed service from manager");
        }

        Ok(())
    }

    /// Introspect service infrastructure wiring.
    /// Returns (`has_overlay`, `has_proxy`, `has_dns`), or None if service not found.
    pub async fn service_infrastructure(&self, name: &str) -> Option<(bool, bool, bool)> {
        let services = self.services.read().await;
        services.get(name).map(|i| {
            (
                i.has_overlay_manager(),
                i.has_proxy_manager(),
                i.has_dns_server(),
            )
        })
    }

    /// List all services
    pub async fn list_services(&self) -> Vec<String> {
        self.services.read().await.keys().cloned().collect()
    }

    /// Get logs for a service, aggregated from all container replicas.
    ///
    /// # Arguments
    /// * `service_name` - Name of the service to fetch logs for
    /// * `tail` - Number of lines to return per container (0 = all)
    /// * `instance` - Optional specific instance (container ID suffix like "1", "2")
    ///
    /// # Errors
    /// Returns an error if the service or instance is not found.
    ///
    /// # Returns
    /// Structured log entries from all (or specific) container replicas. Each
    /// entry has its `service` and `deployment` fields populated when available.
    pub async fn get_service_logs(
        &self,
        service_name: &str,
        tail: usize,
        instance: Option<&str>,
    ) -> Result<Vec<LogEntry>> {
        let container_ids = self.get_service_containers(service_name).await;

        if container_ids.is_empty() {
            return Err(AgentError::NotFound {
                container: service_name.to_string(),
                reason: "no containers found for service".to_string(),
            });
        }

        // If a specific instance is requested, filter to just that one
        let target_ids: Vec<&ContainerId> = if let Some(inst) = instance {
            if let Ok(replica_num) = inst.parse::<u32>() {
                container_ids
                    .iter()
                    .filter(|id| id.replica == replica_num)
                    .collect()
            } else {
                // Try matching by full container ID string suffix
                container_ids
                    .iter()
                    .filter(|id| id.to_string().contains(inst))
                    .collect()
            }
        } else {
            container_ids.iter().collect()
        };

        if target_ids.is_empty() {
            return Err(AgentError::NotFound {
                container: format!("{}/{}", service_name, instance.unwrap_or("?")),
                reason: "instance not found".to_string(),
            });
        }

        let mut all_entries: Vec<LogEntry> = Vec::new();

        for id in &target_ids {
            match self.runtime.container_logs(id, tail).await {
                Ok(mut entries) => {
                    // Populate service and deployment metadata on each entry
                    for entry in &mut entries {
                        if entry.service.is_none() {
                            entry.service = Some(service_name.to_string());
                        }
                        if entry.deployment.is_none() {
                            entry.deployment.clone_from(&self.deployment_name);
                        }
                    }
                    all_entries.extend(entries);
                }
                Err(e) => {
                    tracing::warn!(
                        service = service_name,
                        container = %id,
                        error = %e,
                        "Failed to read container logs"
                    );
                }
            }
        }

        Ok(all_entries)
    }

    /// Get all container IDs for a specific service
    ///
    /// Returns an empty vector if the service doesn't exist.
    ///
    /// # Arguments
    /// * `service_name` - Name of the service to query
    ///
    /// # Returns
    /// Vector of `ContainerIds` for all replicas of the service
    pub async fn get_service_containers(&self, service_name: &str) -> Vec<ContainerId> {
        let services = self.services.read().await;
        if let Some(instance) = services.get(service_name) {
            instance.container_ids().await
        } else {
            Vec::new()
        }
    }

    /// Get per-container info (id, image, real state, pid, overlay IP) for a
    /// specific service.
    ///
    /// Unlike [`get_service_containers`](Self::get_service_containers) (which
    /// returns ids only), this surfaces the REAL image reference and lifecycle
    /// state of each live container so the API/`ps` can report them accurately.
    ///
    /// Returns an empty vector if the service doesn't exist.
    pub async fn get_service_container_infos(&self, service_name: &str) -> Vec<ContainerInfo> {
        let services = self.services.read().await;
        if let Some(instance) = services.get(service_name) {
            instance.container_infos().await
        } else {
            Vec::new()
        }
    }

    /// Execute a command inside a running container for a service
    ///
    /// Picks a specific replica if provided, otherwise uses the first available container.
    ///
    /// # Arguments
    /// * `service_name` - Name of the service
    /// * `replica` - Optional replica number to target
    /// * `cmd` - Command and arguments to execute
    ///
    /// # Errors
    /// Returns an error if the service or replica is not found, or if exec fails.
    ///
    /// # Panics
    /// Panics if no replica is specified and the container list is unexpectedly empty
    /// after the emptiness check (should not happen in practice).
    ///
    /// # Returns
    /// Tuple of (`exit_code`, stdout, stderr)
    pub async fn exec_in_container(
        &self,
        service_name: &str,
        replica: Option<u32>,
        cmd: &[String],
    ) -> Result<(i32, String, String)> {
        let container_ids = self.get_service_containers(service_name).await;

        if container_ids.is_empty() {
            return Err(AgentError::NotFound {
                container: service_name.to_string(),
                reason: "no containers found for service".to_string(),
            });
        }

        // Pick the target container
        let target = if let Some(rep) = replica {
            container_ids
                .into_iter()
                .find(|cid| cid.replica == rep)
                .ok_or_else(|| AgentError::NotFound {
                    container: format!("{service_name}-rep-{rep}"),
                    reason: format!("replica {rep} not found for service"),
                })?
        } else {
            // Use the first container (lowest replica number)
            container_ids.into_iter().next().unwrap()
        };

        self.runtime.exec(&target, cmd).await
    }

    // ==================== Job Management ====================

    /// Trigger a job execution
    ///
    /// # Arguments
    /// * `name` - Name of the registered job
    /// * `trigger` - How the job was triggered (endpoint, cli, etc.)
    ///
    /// # Returns
    /// The execution ID for tracking the job
    ///
    /// # Errors
    /// - Returns error if job executor is not configured
    /// - Returns error if the job is not registered
    pub async fn trigger_job(&self, name: &str, trigger: JobTrigger) -> Result<JobExecutionId> {
        let executor = self
            .job_executor
            .as_ref()
            .ok_or_else(|| AgentError::Configuration("Job executor not configured".to_string()))?;

        let spec = executor
            .get_job_spec(name)
            .await
            .ok_or_else(|| AgentError::NotFound {
                container: name.to_string(),
                reason: "job not registered".to_string(),
            })?;

        executor.trigger(name, &spec, trigger).await
    }

    /// Get the status of a job execution
    ///
    /// # Arguments
    /// * `id` - The execution ID returned from `trigger_job`
    ///
    /// # Returns
    /// The job execution details, or None if not found
    pub async fn get_job_execution(&self, id: &JobExecutionId) -> Option<JobExecution> {
        if let Some(executor) = &self.job_executor {
            executor.get_execution(id).await
        } else {
            None
        }
    }

    /// List all executions for a specific job
    ///
    /// # Arguments
    /// * `name` - Name of the job
    ///
    /// # Returns
    /// Vector of job executions for the specified job
    pub async fn list_job_executions(&self, name: &str) -> Vec<JobExecution> {
        if let Some(executor) = &self.job_executor {
            executor.list_executions(name).await
        } else {
            Vec::new()
        }
    }

    /// Cancel a running job execution
    ///
    /// # Arguments
    /// * `id` - The execution ID to cancel
    ///
    /// # Errors
    /// Returns error if job executor is not configured or if cancellation fails
    pub async fn cancel_job(&self, id: &JobExecutionId) -> Result<()> {
        let executor = self
            .job_executor
            .as_ref()
            .ok_or_else(|| AgentError::Configuration("Job executor not configured".to_string()))?;

        executor.cancel(id).await
    }

    // ==================== Cron Management ====================

    /// Manually trigger a cron job (outside of its schedule)
    ///
    /// # Arguments
    /// * `name` - Name of the cron job
    ///
    /// # Returns
    /// The execution ID for tracking the triggered job
    ///
    /// # Errors
    /// Returns error if cron scheduler is not configured or job not found
    pub async fn trigger_cron(&self, name: &str) -> Result<JobExecutionId> {
        let scheduler = self.cron_scheduler.as_ref().ok_or_else(|| {
            AgentError::Configuration("Cron scheduler not configured".to_string())
        })?;

        scheduler.trigger_now(name).await
    }

    /// Enable or disable a cron job
    ///
    /// # Arguments
    /// * `name` - Name of the cron job
    /// * `enabled` - Whether to enable or disable the job
    pub async fn set_cron_enabled(&self, name: &str, enabled: bool) {
        if let Some(scheduler) = &self.cron_scheduler {
            scheduler.set_enabled(name, enabled).await;
        }
    }

    /// List all registered cron jobs
    pub async fn list_cron_jobs(&self) -> Vec<crate::cron_scheduler::CronJobInfo> {
        if let Some(scheduler) = &self.cron_scheduler {
            scheduler.list_jobs().await
        } else {
            Vec::new()
        }
    }

    /// Start the cron scheduler background task
    ///
    /// This spawns a background task that checks for due cron jobs every second.
    /// Returns a `JoinHandle` that can be used to wait for the scheduler to stop.
    ///
    /// # Errors
    /// Returns error if cron scheduler is not configured
    pub fn start_cron_scheduler(&self) -> Result<tokio::task::JoinHandle<()>> {
        let scheduler = self.cron_scheduler.as_ref().ok_or_else(|| {
            AgentError::Configuration("Cron scheduler not configured".to_string())
        })?;

        let scheduler: Arc<CronScheduler> = Arc::clone(scheduler);
        Ok(tokio::spawn(async move {
            scheduler.run_loop().await;
        }))
    }

    /// Shutdown the cron scheduler
    pub fn shutdown_cron(&self) {
        if let Some(scheduler) = &self.cron_scheduler {
            scheduler.shutdown();
        }
    }
}

#[cfg(test)]
#[allow(deprecated)]
mod tests {
    use super::*;
    use crate::runtime::MockRuntime;

    #[tokio::test]
    async fn test_service_manager() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let manager = ServiceManager::new(runtime);

        // Add service
        let spec = mock_spec();
        Box::pin(manager.upsert_service("test".to_string(), spec))
            .await
            .unwrap();

        // Scale up
        manager.scale_service("test", 3).await.unwrap();

        // Check count
        let count = manager.service_replica_count("test").await.unwrap();
        assert_eq!(count, 3);

        // List services
        let services = manager.list_services().await;
        assert_eq!(services, vec!["test".to_string()]);
    }

    #[tokio::test]
    async fn test_service_manager_basic_lifecycle() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let manager = ServiceManager::new(runtime);

        // Add service with HTTP endpoint
        let spec = mock_spec();
        Box::pin(manager.upsert_service("api".to_string(), spec))
            .await
            .unwrap();

        // Scale up
        manager.scale_service("api", 2).await.unwrap();

        // Check count
        let count = manager.service_replica_count("api").await.unwrap();
        assert_eq!(count, 2);

        // Remove service
        manager.remove_service("api").await.unwrap();

        // Verify service is gone
        let services = manager.list_services().await;
        assert!(!services.contains(&"api".to_string()));
    }

    #[tokio::test]
    async fn test_service_manager_with_full_config() {
        use tokio::sync::RwLock;

        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());

        // Create a mock overlay manager (skip actual network setup)
        let overlay_manager = Arc::new(RwLock::new(
            OverlayManager::new("test-deployment".to_string(), "test".to_string())
                .await
                .unwrap(),
        ));

        let manager =
            ServiceManager::with_full_config(runtime, overlay_manager, "prod".to_string());

        // Add service
        let spec = mock_spec();
        Box::pin(manager.upsert_service("web".to_string(), spec))
            .await
            .unwrap();

        // Verify service is registered
        let services = manager.list_services().await;
        assert!(services.contains(&"web".to_string()));
    }

    #[test]
    fn test_container_state_as_str() {
        use crate::runtime::ContainerState;
        assert_eq!(ContainerState::Pending.as_str(), "pending");
        assert_eq!(ContainerState::Initializing.as_str(), "initializing");
        assert_eq!(ContainerState::Running.as_str(), "running");
        assert_eq!(ContainerState::Stopping.as_str(), "stopping");
        assert_eq!(ContainerState::Exited { code: 0 }.as_str(), "exited");
        assert_eq!(
            ContainerState::Failed {
                reason: "boom".to_string()
            }
            .as_str(),
            "failed"
        );
        // Display delegates to as_str.
        assert_eq!(ContainerState::Running.to_string(), "running");
    }

    /// A container created from image X must report image X and its real
    /// lifecycle state through the new `container_infos` accessor, replacing
    /// the previously hardcoded `"running"` / empty-image behavior.
    #[tokio::test]
    async fn test_container_infos_surfaces_image_and_state() {
        use crate::runtime::{Container, ContainerState};

        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let manager = ServiceManager::new(runtime);

        let spec = mock_spec(); // image name = "test:latest"
        let image = spec.image.name.to_string();
        Box::pin(manager.upsert_service("web".to_string(), spec))
            .await
            .unwrap();

        // Inject containers directly with distinct states.
        {
            let services = manager.services.read().await;
            let instance = services.get("web").unwrap();
            let mut containers = instance.containers().write().await;

            let running_id = ContainerId::new("web", 0);
            containers.insert(
                running_id.clone(),
                Container {
                    id: running_id,
                    image: image.clone(),
                    state: ContainerState::Running,
                    pid: Some(4242),
                    task: None,
                    overlay_ip: None,
                    health_monitor: None,
                    port_override: None,
                },
            );

            let exited_id = ContainerId::new("web", 1);
            containers.insert(
                exited_id.clone(),
                Container {
                    id: exited_id,
                    image: image.clone(),
                    state: ContainerState::Exited { code: 1 },
                    pid: None,
                    task: None,
                    overlay_ip: None,
                    health_monitor: None,
                    port_override: None,
                },
            );
        }

        let mut infos = manager.get_service_container_infos("web").await;
        infos.sort_by_key(|i| i.id.replica);
        assert_eq!(infos.len(), 2);

        // Every container reports the real image it was created from.
        assert!(infos.iter().all(|i| i.image == image));
        assert!(infos.iter().all(|i| i.image == "test:latest"));

        // Real per-container state is surfaced (not a hardcoded "running").
        assert_eq!(infos[0].state, "running");
        assert_eq!(infos[0].pid, Some(4242));
        assert_eq!(infos[1].state, "exited");
        assert_eq!(infos[1].pid, None);

        // Unknown service yields an empty list.
        assert!(manager
            .get_service_container_infos("missing")
            .await
            .is_empty());
    }

    /// Bug 2 (`cluster_upgrade`): a changed image *reference* (tag bump) under
    /// `if_not_present` must still recreate the local replicas. Previously the
    /// recreate only fired on digest drift under `Always`/`Newer`, so a tag
    /// change was silently ignored and containers stayed on the old image.
    #[tokio::test]
    async fn upsert_recreates_local_replicas_on_image_reference_change() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let manager = ServiceManager::new(runtime);

        // Deploy v1 with the e2e's pull policy (if_not_present) and scale up.
        let mut spec = mock_spec();
        spec.image.name = "docker.io/library/nginx:1.28-alpine".parse().unwrap();
        spec.image.pull_policy = zlayer_spec::PullPolicy::IfNotPresent;
        Box::pin(manager.upsert_service("web".to_string(), spec.clone()))
            .await
            .unwrap();
        manager.scale_service_local("web", 2).await.unwrap();

        let v1: Vec<String> = manager
            .get_service_container_infos("web")
            .await
            .into_iter()
            .map(|i| i.image)
            .collect();
        assert_eq!(v1.len(), 2);
        assert!(
            v1.iter().all(|img| img.contains("1.28")),
            "expected v1 images, got {v1:?}"
        );

        // Upgrade to v2 under the SAME if_not_present policy.
        let mut spec_v2 = spec;
        spec_v2.image.name = "docker.io/library/nginx:1.29-alpine".parse().unwrap();
        Box::pin(manager.upsert_service("web".to_string(), spec_v2))
            .await
            .unwrap();

        let v2: Vec<String> = manager
            .get_service_container_infos("web")
            .await
            .into_iter()
            .map(|i| i.image)
            .collect();
        assert_eq!(v2.len(), 2, "replica count preserved across upgrade");
        assert!(
            v2.iter().all(|img| img.contains("1.29")),
            "containers must be recreated on the new image, got {v2:?}"
        );
    }

    fn mock_spec() -> ServiceSpec {
        serde_yaml::from_str::<zlayer_spec::DeploymentSpec>(
            r"
version: v1
deployment: test
services:
  test:
    rtype: service
    image:
      name: test:latest
    endpoints:
      - name: http
        protocol: http
        port: 8080
    scale:
      mode: fixed
      replicas: 1
",
        )
        .unwrap()
        .services
        .remove("test")
        .unwrap()
    }

    /// Helper to create a `ServiceSpec` with dependencies
    fn mock_spec_with_deps(deps: Vec<DependsSpec>) -> ServiceSpec {
        let mut spec = mock_spec();
        spec.depends = deps;
        spec
    }

    /// Helper to create a `DependsSpec`
    fn dep(
        service: &str,
        condition: zlayer_spec::DependencyCondition,
        timeout_ms: u64,
        on_timeout: zlayer_spec::TimeoutAction,
    ) -> DependsSpec {
        DependsSpec {
            service: service.to_string(),
            condition,
            timeout: Some(Duration::from_millis(timeout_ms)),
            on_timeout,
        }
    }

    #[tokio::test]
    async fn test_deploy_with_dependencies_no_deps() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let manager = ServiceManager::new(runtime);

        // Services with no dependencies
        let mut services = HashMap::new();
        services.insert("a".to_string(), mock_spec());
        services.insert("b".to_string(), mock_spec());

        // Should deploy both without issue
        Box::pin(manager.deploy_with_dependencies(services))
            .await
            .unwrap();

        // Both services should be registered
        let service_list = manager.list_services().await;
        assert_eq!(service_list.len(), 2);
    }

    #[tokio::test]
    async fn test_deploy_with_dependencies_linear() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let manager = ServiceManager::new(runtime);

        // A -> B -> C (A depends on B, B depends on C)
        // All use "started" condition which is satisfied when container is running
        let mut services = HashMap::new();
        services.insert("c".to_string(), mock_spec());
        services.insert(
            "b".to_string(),
            mock_spec_with_deps(vec![dep(
                "c",
                zlayer_spec::DependencyCondition::Started,
                5000,
                zlayer_spec::TimeoutAction::Fail,
            )]),
        );
        services.insert(
            "a".to_string(),
            mock_spec_with_deps(vec![dep(
                "b",
                zlayer_spec::DependencyCondition::Started,
                5000,
                zlayer_spec::TimeoutAction::Fail,
            )]),
        );

        // Should deploy in order: c, b, a
        Box::pin(manager.deploy_with_dependencies(services))
            .await
            .unwrap();

        // All services should be registered
        let service_list = manager.list_services().await;
        assert_eq!(service_list.len(), 3);
    }

    #[tokio::test]
    async fn test_deploy_with_dependencies_cycle_detection() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let manager = ServiceManager::new(runtime);

        // A -> B -> A (cycle)
        let mut services = HashMap::new();
        services.insert(
            "a".to_string(),
            mock_spec_with_deps(vec![dep(
                "b",
                zlayer_spec::DependencyCondition::Started,
                5000,
                zlayer_spec::TimeoutAction::Fail,
            )]),
        );
        services.insert(
            "b".to_string(),
            mock_spec_with_deps(vec![dep(
                "a",
                zlayer_spec::DependencyCondition::Started,
                5000,
                zlayer_spec::TimeoutAction::Fail,
            )]),
        );

        // Should fail with cycle detection
        let result = Box::pin(manager.deploy_with_dependencies(services)).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Cyclic dependency"));
    }

    #[tokio::test]
    async fn test_deploy_with_dependencies_timeout_continue() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let manager = ServiceManager::new(runtime);

        // A depends on B (healthy), but B never becomes healthy
        // Using continue action, so it should proceed anyway
        let mut services = HashMap::new();
        services.insert("b".to_string(), mock_spec());
        services.insert(
            "a".to_string(),
            mock_spec_with_deps(vec![dep(
                "b",
                zlayer_spec::DependencyCondition::Healthy, // B won't pass healthy check
                100,                                       // Short timeout
                zlayer_spec::TimeoutAction::Continue,      // But continue anyway
            )]),
        );

        // Should deploy both despite timeout
        Box::pin(manager.deploy_with_dependencies(services))
            .await
            .unwrap();

        let service_list = manager.list_services().await;
        assert_eq!(service_list.len(), 2);
    }

    #[tokio::test]
    async fn test_deploy_with_dependencies_timeout_warn() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let manager = ServiceManager::new(runtime);

        // A depends on B (healthy), but B never becomes healthy
        // Using warn action, so it should proceed with a warning
        let mut services = HashMap::new();
        services.insert("b".to_string(), mock_spec());
        services.insert(
            "a".to_string(),
            mock_spec_with_deps(vec![dep(
                "b",
                zlayer_spec::DependencyCondition::Healthy,
                100,
                zlayer_spec::TimeoutAction::Warn,
            )]),
        );

        // Should deploy both despite timeout (with warning)
        Box::pin(manager.deploy_with_dependencies(services))
            .await
            .unwrap();

        let service_list = manager.list_services().await;
        assert_eq!(service_list.len(), 2);
    }

    #[tokio::test]
    async fn test_deploy_with_dependencies_timeout_fail() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let manager = ServiceManager::new(runtime);

        // A depends on B (healthy), but B never becomes healthy
        // Using fail action, so deployment should fail
        let mut services = HashMap::new();
        services.insert("b".to_string(), mock_spec());
        services.insert(
            "a".to_string(),
            mock_spec_with_deps(vec![dep(
                "b",
                zlayer_spec::DependencyCondition::Healthy,
                100,
                zlayer_spec::TimeoutAction::Fail,
            )]),
        );

        // Should fail after B is started but doesn't become healthy
        let result = Box::pin(manager.deploy_with_dependencies(services)).await;
        assert!(result.is_err());

        // B should be started (it has no deps), but A should fail
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Dependency timeout"));
    }

    #[tokio::test]
    async fn test_check_dependencies_all_satisfied() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let manager = ServiceManager::new(runtime);

        // Mark a service as healthy
        manager
            .update_health_state("db", HealthState::Healthy)
            .await;

        let deps = vec![DependsSpec {
            service: "db".to_string(),
            condition: zlayer_spec::DependencyCondition::Healthy,
            timeout: Some(Duration::from_secs(60)),
            on_timeout: zlayer_spec::TimeoutAction::Fail,
        }];

        let satisfied = manager.check_dependencies(&deps).await.unwrap();
        assert!(satisfied);
    }

    #[tokio::test]
    async fn test_check_dependencies_not_satisfied() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let manager = ServiceManager::new(runtime);

        // Service not healthy (no state set = Unknown)
        let deps = vec![DependsSpec {
            service: "db".to_string(),
            condition: zlayer_spec::DependencyCondition::Healthy,
            timeout: Some(Duration::from_secs(60)),
            on_timeout: zlayer_spec::TimeoutAction::Fail,
        }];

        let satisfied = manager.check_dependencies(&deps).await.unwrap();
        assert!(!satisfied);
    }

    #[tokio::test]
    async fn test_health_state_tracking() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let manager = ServiceManager::new(runtime);

        // Update health states
        manager
            .update_health_state("db", HealthState::Healthy)
            .await;
        manager
            .update_health_state("cache", HealthState::Unknown)
            .await;

        // Verify states
        let states = manager.health_states();
        let states_read = states.read().await;

        assert!(matches!(states_read.get("db"), Some(HealthState::Healthy)));
        assert!(matches!(
            states_read.get("cache"),
            Some(HealthState::Unknown)
        ));
    }

    /// Regression test for the stabilization timeout that blocked the raft-e2e
    /// `cluster_scaling` / `cluster_upgrade` suites.
    ///
    /// Previously the callback that bridges a container's health result into the
    /// `ServiceManager` `health_states` map was only attached when BOTH a proxy
    /// manager AND a reachable overlay IP existed. In degraded-overlay / no-proxy
    /// deployments that `if let` was false, so `health_states` was never written,
    /// the service stayed `healthy=false` forever, and stabilization timed out
    /// even though the container was running and its health check passing.
    ///
    /// This test drives the real `scale_to` create path with:
    ///   * NO `proxy_manager` (so `proxy_backend` resolves to None), and
    ///   * a `Command { command: "true" }` health check (always passes host-side),
    /// then asserts the shared `health_states` map receives `Healthy` for the
    /// service — proving the bridge fires unconditionally.
    #[tokio::test]
    async fn test_health_states_bridge_fires_without_proxy() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());

        // Service spec with a host-side command health check that always passes.
        // Zero start-grace + a short interval keep the test fast.
        let mut spec = mock_spec();
        spec.health = zlayer_spec::HealthSpec {
            start_grace: Some(Duration::from_millis(0)),
            interval: Some(Duration::from_millis(50)),
            timeout: Some(Duration::from_secs(5)),
            retries: 1,
            check: HealthCheck::Command {
                command: "true".to_string(),
            },
        };

        // Build a ServiceInstance with NO proxy_manager and NO overlay_manager,
        // then wire in the shared health_states map exactly as ServiceManager does.
        let mut instance =
            ServiceInstance::new("web".to_string(), spec, Arc::clone(&runtime), None);
        let health_states: Arc<RwLock<HashMap<String, HealthState>>> =
            Arc::new(RwLock::new(HashMap::new()));
        instance.set_health_states(Arc::clone(&health_states));

        // Drive the real create path (no proxy, MockRuntime IP present but proxy
        // absent => proxy_backend is None, hitting the previously-broken branch).
        instance.scale_to(1).await.unwrap();

        // Poll for the bridged Healthy state (the monitor checks asynchronously
        // after its start grace). Bounded so a regression fails fast.
        let mut bridged = false;
        for _ in 0..100 {
            if matches!(
                health_states.read().await.get("web"),
                Some(HealthState::Healthy)
            ) {
                bridged = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        assert!(
            bridged,
            "health_states must receive Healthy for the service even without a \
             proxy or overlay IP; the bridge regressed and stabilization would time out"
        );
    }

    // ==================== Job/Cron Integration Tests ====================

    fn mock_job_spec() -> ServiceSpec {
        serde_yaml::from_str::<zlayer_spec::DeploymentSpec>(
            r"
version: v1
deployment: test
services:
  backup:
    rtype: job
    image:
      name: backup:latest
",
        )
        .unwrap()
        .services
        .remove("backup")
        .unwrap()
    }

    fn mock_cron_spec() -> ServiceSpec {
        serde_yaml::from_str::<zlayer_spec::DeploymentSpec>(
            r#"
version: v1
deployment: test
services:
  cleanup:
    rtype: cron
    schedule: "0 0 * * * * *"
    image:
      name: cleanup:latest
"#,
        )
        .unwrap()
        .services
        .remove("cleanup")
        .unwrap()
    }

    #[tokio::test]
    async fn test_service_manager_with_job_executor() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let job_executor = Arc::new(JobExecutor::new(runtime.clone()));

        let manager = ServiceManager::new(runtime).with_job_executor(job_executor);

        // Register job
        let job_spec = mock_job_spec();
        Box::pin(manager.upsert_service("backup".to_string(), job_spec))
            .await
            .unwrap();

        // Trigger job
        let exec_id = manager
            .trigger_job("backup", JobTrigger::Cli)
            .await
            .unwrap();

        // Give job time to start
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Check execution exists
        let execution = manager.get_job_execution(&exec_id).await;
        assert!(execution.is_some());
        assert_eq!(execution.unwrap().job_name, "backup");
    }

    #[tokio::test]
    async fn test_service_manager_with_cron_scheduler() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let job_executor = Arc::new(JobExecutor::new(runtime.clone()));
        let cron_scheduler = Arc::new(CronScheduler::new(job_executor));

        let manager = ServiceManager::new(runtime).with_cron_scheduler(cron_scheduler);

        // Register cron job
        let cron_spec = mock_cron_spec();
        Box::pin(manager.upsert_service("cleanup".to_string(), cron_spec))
            .await
            .unwrap();

        // List cron jobs
        let cron_jobs = manager.list_cron_jobs().await;
        assert_eq!(cron_jobs.len(), 1);
        assert_eq!(cron_jobs[0].name, "cleanup");
        assert!(cron_jobs[0].enabled);
    }

    #[tokio::test]
    async fn test_service_manager_trigger_cron() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let job_executor = Arc::new(JobExecutor::new(runtime.clone()));
        let cron_scheduler = Arc::new(CronScheduler::new(job_executor.clone()));

        let manager = ServiceManager::new(runtime)
            .with_job_executor(job_executor)
            .with_cron_scheduler(cron_scheduler);

        // Register cron job
        let cron_spec = mock_cron_spec();
        Box::pin(manager.upsert_service("cleanup".to_string(), cron_spec))
            .await
            .unwrap();

        // Manually trigger the cron job
        let exec_id = manager.trigger_cron("cleanup").await.unwrap();
        assert!(!exec_id.0.is_empty());
    }

    #[tokio::test]
    async fn test_service_manager_enable_disable_cron() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let job_executor = Arc::new(JobExecutor::new(runtime.clone()));
        let cron_scheduler = Arc::new(CronScheduler::new(job_executor));

        let manager = ServiceManager::new(runtime).with_cron_scheduler(cron_scheduler);

        // Register cron job
        let cron_spec = mock_cron_spec();
        Box::pin(manager.upsert_service("cleanup".to_string(), cron_spec))
            .await
            .unwrap();

        // Initially enabled
        let cron_jobs = manager.list_cron_jobs().await;
        assert!(cron_jobs[0].enabled);

        // Disable
        manager.set_cron_enabled("cleanup", false).await;
        let cron_jobs = manager.list_cron_jobs().await;
        assert!(!cron_jobs[0].enabled);

        // Re-enable
        manager.set_cron_enabled("cleanup", true).await;
        let cron_jobs = manager.list_cron_jobs().await;
        assert!(cron_jobs[0].enabled);
    }

    #[tokio::test]
    async fn test_service_manager_remove_cleans_up_job() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let job_executor = Arc::new(JobExecutor::new(runtime.clone()));

        let manager = ServiceManager::new(runtime).with_job_executor(job_executor.clone());

        // Register job
        let job_spec = mock_job_spec();
        Box::pin(manager.upsert_service("backup".to_string(), job_spec))
            .await
            .unwrap();

        // Verify job is registered
        let spec = job_executor.get_job_spec("backup").await;
        assert!(spec.is_some());

        // Remove job
        manager.remove_service("backup").await.unwrap();

        // Verify job is unregistered
        let spec = job_executor.get_job_spec("backup").await;
        assert!(spec.is_none());
    }

    #[tokio::test]
    async fn test_service_manager_remove_cleans_up_cron() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let job_executor = Arc::new(JobExecutor::new(runtime.clone()));
        let cron_scheduler = Arc::new(CronScheduler::new(job_executor));

        let manager = ServiceManager::new(runtime).with_cron_scheduler(cron_scheduler.clone());

        // Register cron job
        let cron_spec = mock_cron_spec();
        Box::pin(manager.upsert_service("cleanup".to_string(), cron_spec))
            .await
            .unwrap();

        // Verify cron job is registered
        assert_eq!(cron_scheduler.job_count().await, 1);

        // Remove cron job
        manager.remove_service("cleanup").await.unwrap();

        // Verify cron job is unregistered
        assert_eq!(cron_scheduler.job_count().await, 0);
    }

    #[tokio::test]
    async fn test_service_manager_job_without_executor() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let manager = ServiceManager::new(runtime);

        // Try to trigger job without executor configured
        let result = manager.trigger_job("nonexistent", JobTrigger::Cli).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not configured"));
    }

    #[tokio::test]
    async fn test_service_manager_cron_without_scheduler() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let manager = ServiceManager::new(runtime);

        // Try to register cron job without scheduler configured
        let cron_spec = mock_cron_spec();
        let result = Box::pin(manager.upsert_service("cleanup".to_string(), cron_spec)).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not configured"));
    }

    #[tokio::test]
    async fn test_service_manager_list_job_executions() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let job_executor = Arc::new(JobExecutor::new(runtime.clone()));

        let manager = ServiceManager::new(runtime).with_job_executor(job_executor);

        // Register job
        let job_spec = mock_job_spec();
        Box::pin(manager.upsert_service("backup".to_string(), job_spec))
            .await
            .unwrap();

        // Trigger job twice
        manager
            .trigger_job("backup", JobTrigger::Cli)
            .await
            .unwrap();
        manager
            .trigger_job("backup", JobTrigger::Scheduler)
            .await
            .unwrap();

        // Give jobs time to start
        tokio::time::sleep(Duration::from_millis(50)).await;

        // List executions
        let executions = manager.list_job_executions("backup").await;
        assert_eq!(executions.len(), 2);
    }

    // ==================== Container Supervisor Integration Tests ====================

    #[tokio::test]
    async fn test_service_manager_with_supervisor() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let supervisor = Arc::new(ContainerSupervisor::new(runtime.clone()));

        let manager = ServiceManager::new(runtime).with_container_supervisor(supervisor.clone());

        // Add service
        let spec = mock_spec();
        Box::pin(manager.upsert_service("api".to_string(), spec))
            .await
            .unwrap();

        // Scale up - containers should be registered with supervisor
        manager.scale_service("api", 2).await.unwrap();

        // Verify containers are supervised
        assert_eq!(supervisor.supervised_count().await, 2);

        // Scale down - containers should be unregistered
        manager.scale_service("api", 1).await.unwrap();
        assert_eq!(supervisor.supervised_count().await, 1);

        // Remove service - remaining containers should be unregistered
        manager.remove_service("api").await.unwrap();
        assert_eq!(supervisor.supervised_count().await, 0);
    }

    #[tokio::test]
    async fn test_service_manager_supervisor_state() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let supervisor = Arc::new(ContainerSupervisor::new(runtime.clone()));

        let manager = ServiceManager::new(runtime).with_container_supervisor(supervisor);

        // Add and scale service
        let spec = mock_spec();
        Box::pin(manager.upsert_service("web".to_string(), spec))
            .await
            .unwrap();
        manager.scale_service("web", 1).await.unwrap();

        // Check supervised state
        let container_id = ContainerId::new("web".to_string(), 1);
        let state = manager.get_container_supervised_state(&container_id).await;
        assert_eq!(state, Some(SupervisedState::Running));
    }

    #[tokio::test]
    async fn test_service_manager_start_supervisor() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let supervisor = Arc::new(ContainerSupervisor::new(runtime.clone()));

        let manager = ServiceManager::new(runtime).with_container_supervisor(supervisor.clone());

        // Start the supervisor
        let handle = manager.start_container_supervisor().unwrap();

        // Give it time to start
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(supervisor.is_running());

        // Shutdown
        manager.shutdown_container_supervisor();

        // Wait for it to stop
        tokio::time::timeout(Duration::from_secs(1), handle)
            .await
            .unwrap()
            .unwrap();

        assert!(!supervisor.is_running());
    }

    #[tokio::test]
    async fn test_service_manager_supervisor_not_configured() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let manager = ServiceManager::new(runtime);

        // Try to start supervisor without configuring it
        let result = manager.start_container_supervisor();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not configured"));
    }

    // ==================== Stream Registry Integration Tests ====================

    fn mock_tcp_spec() -> ServiceSpec {
        serde_yaml::from_str::<zlayer_spec::DeploymentSpec>(
            r"
version: v1
deployment: test
services:
  database:
    rtype: service
    image:
      name: postgres:latest
    endpoints:
      - name: postgresql
        protocol: tcp
        port: 5432
    scale:
      mode: fixed
      replicas: 1
",
        )
        .unwrap()
        .services
        .remove("database")
        .unwrap()
    }

    fn mock_udp_spec() -> ServiceSpec {
        serde_yaml::from_str::<zlayer_spec::DeploymentSpec>(
            r"
version: v1
deployment: test
services:
  dns:
    rtype: service
    image:
      name: dns:latest
    endpoints:
      - name: dns
        protocol: udp
        port: 53
    scale:
      mode: fixed
      replicas: 1
",
        )
        .unwrap()
        .services
        .remove("dns")
        .unwrap()
    }

    fn mock_mixed_spec() -> ServiceSpec {
        serde_yaml::from_str::<zlayer_spec::DeploymentSpec>(
            r"
version: v1
deployment: test
services:
  mixed:
    rtype: service
    image:
      name: mixed:latest
    endpoints:
      - name: http
        protocol: http
        port: 8080
      - name: grpc
        protocol: tcp
        port: 9000
      - name: metrics
        protocol: udp
        port: 8125
    scale:
      mode: fixed
      replicas: 1
",
        )
        .unwrap()
        .services
        .remove("mixed")
        .unwrap()
    }

    #[tokio::test]
    async fn test_service_manager_with_stream_registry_tcp() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let stream_registry = Arc::new(StreamRegistry::new());

        let mut manager = ServiceManager::new(runtime);
        manager.set_stream_registry(stream_registry.clone());
        manager.set_deployment_name("test".to_string());

        // Add TCP-only service
        let spec = mock_tcp_spec();
        Box::pin(manager.upsert_service("database".to_string(), spec))
            .await
            .unwrap();

        // Verify TCP route was registered
        assert_eq!(stream_registry.tcp_count(), 1);
        assert!(stream_registry.tcp_ports().contains(&5432));

        // Remove service and verify cleanup
        manager.remove_service("database").await.unwrap();
        assert_eq!(stream_registry.tcp_count(), 0);
    }

    #[tokio::test]
    async fn test_service_manager_with_stream_registry_udp() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let stream_registry = Arc::new(StreamRegistry::new());

        let mut manager = ServiceManager::new(runtime);
        manager.set_stream_registry(stream_registry.clone());
        manager.set_deployment_name("test".to_string());

        // Add UDP-only service
        let spec = mock_udp_spec();
        Box::pin(manager.upsert_service("dns".to_string(), spec))
            .await
            .unwrap();

        // Verify UDP route was registered
        assert_eq!(stream_registry.udp_count(), 1);
        assert!(stream_registry.udp_ports().contains(&53));

        // Remove service and verify cleanup
        manager.remove_service("dns").await.unwrap();
        assert_eq!(stream_registry.udp_count(), 0);
    }

    #[tokio::test]
    async fn test_service_manager_with_stream_registry_mixed() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let stream_registry = Arc::new(StreamRegistry::new());

        let mut manager = ServiceManager::new(runtime);
        manager.set_stream_registry(stream_registry.clone());
        manager.set_deployment_name("test".to_string());

        // Add mixed service (HTTP + TCP + UDP)
        let spec = mock_mixed_spec();
        Box::pin(manager.upsert_service("mixed".to_string(), spec))
            .await
            .unwrap();

        // Verify stream routes were registered
        assert_eq!(stream_registry.tcp_count(), 1); // TCP: 9000
        assert_eq!(stream_registry.udp_count(), 1); // UDP: 8125

        assert!(stream_registry.tcp_ports().contains(&9000));
        assert!(stream_registry.udp_ports().contains(&8125));

        // Remove service and verify stream cleanup
        manager.remove_service("mixed").await.unwrap();
        assert_eq!(stream_registry.tcp_count(), 0);
        assert_eq!(stream_registry.udp_count(), 0);
    }

    #[tokio::test]
    async fn test_service_manager_stream_registry_builder() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let stream_registry = Arc::new(StreamRegistry::new());

        // Test builder pattern
        let manager = ServiceManager::new(runtime).with_stream_registry(stream_registry.clone());

        // Verify stream registry is accessible
        assert!(manager.stream_registry().is_some());
    }

    #[tokio::test]
    async fn test_tcp_service_without_stream_registry() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());

        // Manager without stream registry
        let mut manager = ServiceManager::new(runtime);
        manager.set_deployment_name("test".to_string());

        // Add TCP service - should log warning but not fail
        let spec = mock_tcp_spec();
        Box::pin(manager.upsert_service("database".to_string(), spec))
            .await
            .unwrap();

        // No stream registry to check, but service should be tracked
        let services = manager.list_services().await;
        assert!(services.contains(&"database".to_string()));
    }

    /// Verify `collect_endpoint_backends` filters containers by
    /// `EndpointSpec.target_role`.
    ///
    /// Given two replica groups (`primary` × 1, `read` × 2) and two
    /// endpoints — one with `target_role: primary` and one with
    /// `target_role: read` — each endpoint should receive only the
    /// matching containers' overlay addresses. The legacy no-filter
    /// endpoint (`target_role: None`) should receive all of them.
    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn test_collect_endpoint_backends_respects_target_role() {
        use crate::runtime::Container;
        use std::collections::HashMap as StdHashMap;
        use std::net::{IpAddr, Ipv4Addr};
        use zlayer_spec::{
            EndpointSpec, ExposeType, GroupAffinity, Protocol, ReplicaGroup, ScaleSpec,
        };

        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let manager = ServiceManager::new(runtime.clone());

        // Build a spec with replica_groups and three endpoints:
        // - "write" targets role "primary"
        // - "read" targets role "read"
        // - "any" has no target_role (legacy)
        let mut spec = mock_spec();
        spec.replica_groups = Some(vec![
            ReplicaGroup {
                role: "primary".to_string(),
                count: 1,
                image: None,
                env: StdHashMap::new(),
                command: None,
                resources: None,
                affinity: GroupAffinity::default(),
            },
            ReplicaGroup {
                role: "read".to_string(),
                count: 2,
                image: None,
                env: StdHashMap::new(),
                command: None,
                resources: None,
                affinity: GroupAffinity::default(),
            },
        ]);
        spec.scale = ScaleSpec::Fixed { replicas: 3 };
        spec.endpoints = vec![
            EndpointSpec {
                name: "write".to_string(),
                protocol: Protocol::Tcp,
                port: 5432,
                target_port: Some(5432),
                path: None,
                host: None,
                expose: ExposeType::Internal,
                stream: None,
                tunnel: None,
                target_role: Some("primary".to_string()),
            },
            EndpointSpec {
                name: "read".to_string(),
                protocol: Protocol::Tcp,
                port: 5433,
                target_port: Some(5432),
                path: None,
                host: None,
                expose: ExposeType::Internal,
                stream: None,
                tunnel: None,
                target_role: Some("read".to_string()),
            },
            EndpointSpec {
                name: "any".to_string(),
                protocol: Protocol::Tcp,
                port: 5434,
                target_port: Some(5432),
                path: None,
                host: None,
                expose: ExposeType::Internal,
                stream: None,
                tunnel: None,
                target_role: None,
            },
        ];

        let instance = ServiceInstance::new(
            "postgres".to_string(),
            spec.clone(),
            runtime,
            None, // overlay_manager — not exercised by this test
        );

        // Inject three containers directly: one primary, two read replicas.
        let cid_primary = ContainerId::with_role_and_node("postgres", 1, "primary", 0);
        let cid_first_read = ContainerId::with_role_and_node("postgres", 2, "read", 0);
        let cid_second_read = ContainerId::with_role_and_node("postgres", 3, "read", 0);

        let ip_primary = IpAddr::V4(Ipv4Addr::new(10, 200, 0, 1));
        let ip_first_read = IpAddr::V4(Ipv4Addr::new(10, 200, 0, 2));
        let ip_second_read = IpAddr::V4(Ipv4Addr::new(10, 200, 0, 3));

        {
            let mut containers = instance.containers().write().await;
            containers.insert(
                cid_primary.clone(),
                Container {
                    id: cid_primary,
                    image: spec.image.name.to_string(),
                    state: crate::runtime::ContainerState::Running,
                    pid: None,
                    task: None,
                    overlay_ip: Some(ip_primary),
                    health_monitor: None,
                    port_override: None,
                },
            );
            containers.insert(
                cid_first_read.clone(),
                Container {
                    id: cid_first_read,
                    image: spec.image.name.to_string(),
                    state: crate::runtime::ContainerState::Running,
                    pid: None,
                    task: None,
                    overlay_ip: Some(ip_first_read),
                    health_monitor: None,
                    port_override: None,
                },
            );
            containers.insert(
                cid_second_read.clone(),
                Container {
                    id: cid_second_read,
                    image: spec.image.name.to_string(),
                    state: crate::runtime::ContainerState::Running,
                    pid: None,
                    task: None,
                    overlay_ip: Some(ip_second_read),
                    health_monitor: None,
                    port_override: None,
                },
            );
        }

        let write_ep = &spec.endpoints[0];
        let read_ep = &spec.endpoints[1];
        let any_ep = &spec.endpoints[2];

        let write_backends = manager.collect_endpoint_backends(&instance, write_ep).await;
        let read_backends = manager.collect_endpoint_backends(&instance, read_ep).await;
        let any_backends = manager.collect_endpoint_backends(&instance, any_ep).await;

        // write endpoint -> only the primary container
        assert_eq!(write_backends.len(), 1, "write should match only primary");
        assert!(
            write_backends.iter().any(|a| a.ip() == ip_primary),
            "write backends missing primary IP: {write_backends:?}"
        );

        // read endpoint -> both read containers, no primary
        assert_eq!(
            read_backends.len(),
            2,
            "read should match both read replicas"
        );
        assert!(read_backends.iter().any(|a| a.ip() == ip_first_read));
        assert!(read_backends.iter().any(|a| a.ip() == ip_second_read));
        assert!(
            !read_backends.iter().any(|a| a.ip() == ip_primary),
            "read backends must not contain primary: {read_backends:?}"
        );

        // legacy endpoint (target_role = None) -> every container
        assert_eq!(
            any_backends.len(),
            3,
            "any-role endpoint should see all containers"
        );
    }
}
