//! Service-level container lifecycle management

use crate::container_supervisor::{ContainerSupervisor, SupervisedState, SupervisorEvent};
use crate::cron_scheduler::CronScheduler;
use crate::dependency::{
    DependencyConditionChecker, DependencyGraph, DependencyWaiter, WaitResult,
};
use crate::error::{AgentError, Result};
use crate::health::{HealthChecker, HealthMonitor, HealthState};
use crate::init::InitOrchestrator;
use crate::job::{JobExecution, JobExecutionId, JobExecutor, JobTrigger};
use crate::overlay_manager::OverlayManager;
use crate::runtime::{Container, ContainerId, ContainerState, Runtime};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{RwLock, Semaphore};
use zlayer_proxy::{ResolvedService, ServiceRegistry};
use zlayer_spec::{DependsSpec, Protocol, ResourceType, ServiceSpec};

/// Service instance manages a single service's containers
pub struct ServiceInstance {
    pub service_name: String,
    pub spec: ServiceSpec,
    runtime: Arc<dyn Runtime + Send + Sync>,
    containers: tokio::sync::RwLock<std::collections::HashMap<ContainerId, Container>>,
}

impl ServiceInstance {
    /// Create a new service instance
    pub fn new(
        service_name: String,
        spec: ServiceSpec,
        runtime: Arc<dyn Runtime + Send + Sync>,
    ) -> Self {
        Self {
            service_name,
            spec,
            runtime,
            containers: tokio::sync::RwLock::new(std::collections::HashMap::new()),
        }
    }

    /// Scale to the desired number of replicas
    ///
    /// This method uses short-lived locks to avoid blocking concurrent operations.
    /// I/O operations (pull, create, start, stop, remove) are performed without
    /// holding the containers lock to allow other operations to proceed.
    pub async fn scale_to(&self, replicas: u32) -> Result<()> {
        // Phase 1: Determine current state (short read lock)
        let current_replicas = { self.containers.read().await.len() as u32 }; // Lock released here

        // Phase 2: Scale up - create new containers (no lock held during I/O)
        if replicas > current_replicas {
            // Pull image ONCE before creating any replicas (cached layers are reused)
            self.runtime
                .pull_image_with_policy(&self.spec.image.name, self.spec.image.pull_policy)
                .await
                .map_err(|e| AgentError::PullFailed {
                    image: self.spec.image.name.clone(),
                    reason: e.to_string(),
                })?;

            for i in current_replicas..replicas {
                let id = ContainerId {
                    service: self.service_name.clone(),
                    replica: i + 1,
                };

                // Create container (no lock needed - I/O operation)
                self.runtime
                    .create_container(&id, &self.spec)
                    .await
                    .map_err(|e| AgentError::CreateFailed {
                        id: id.to_string(),
                        reason: e.to_string(),
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

                // Start health monitoring (no lock needed)
                {
                    let check = self.spec.health.check.clone();
                    let interval = self.spec.health.interval.unwrap_or(Duration::from_secs(10));
                    let retries = self.spec.health.retries;

                    let checker = HealthChecker::new(check);
                    let monitor = HealthMonitor::new(id.clone(), checker, interval, retries);
                    // TODO: store monitor handle
                    let _monitor = monitor;
                }

                // Update state (short write lock)
                {
                    let mut containers = self.containers.write().await;
                    containers.insert(
                        id.clone(),
                        Container {
                            id: id.clone(),
                            state: ContainerState::Running,
                            pid: None,
                            task: None,
                        },
                    );
                } // Lock released here
            }
        }

        // Phase 3: Scale down - remove containers (short write lock per removal)
        if replicas < current_replicas {
            for i in replicas..current_replicas {
                let id = ContainerId {
                    service: self.service_name.clone(),
                    replica: i + 1,
                };

                // Remove from state first (short write lock)
                let existed = {
                    let mut containers = self.containers.write().await;
                    containers.remove(&id).is_some()
                }; // Lock released here

                // Then perform cleanup (no lock held - I/O operations)
                if existed {
                    // Stop container
                    self.runtime
                        .stop_container(&id, Duration::from_secs(30))
                        .await?;

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
}

/// Service manager for multiple services
pub struct ServiceManager {
    runtime: Arc<dyn Runtime + Send + Sync>,
    services: tokio::sync::RwLock<std::collections::HashMap<String, ServiceInstance>>,
    scale_semaphore: Arc<Semaphore>,
    /// Overlay network manager for container networking
    overlay_manager: Option<Arc<RwLock<OverlayManager>>>,
    /// Service registry for Pingora proxy route registration
    service_registry: Option<Arc<ServiceRegistry>>,
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
}

impl ServiceManager {
    /// Create a new service manager
    pub fn new(runtime: Arc<dyn Runtime + Send + Sync>) -> Self {
        Self {
            runtime,
            services: tokio::sync::RwLock::new(std::collections::HashMap::new()),
            scale_semaphore: Arc::new(Semaphore::new(10)), // Max 10 concurrent scaling operations
            overlay_manager: None,
            service_registry: None,
            deployment_name: None,
            health_states: Arc::new(RwLock::new(HashMap::new())),
            job_executor: None,
            cron_scheduler: None,
            container_supervisor: None,
        }
    }

    /// Create a service manager with overlay network support
    pub fn with_overlay(
        runtime: Arc<dyn Runtime + Send + Sync>,
        overlay_manager: Arc<RwLock<OverlayManager>>,
    ) -> Self {
        Self {
            runtime,
            services: tokio::sync::RwLock::new(std::collections::HashMap::new()),
            scale_semaphore: Arc::new(Semaphore::new(10)),
            overlay_manager: Some(overlay_manager),
            service_registry: None,
            deployment_name: None,
            health_states: Arc::new(RwLock::new(HashMap::new())),
            job_executor: None,
            cron_scheduler: None,
            container_supervisor: None,
        }
    }

    /// Create a service manager with Pingora proxy support
    pub fn with_proxy(
        runtime: Arc<dyn Runtime + Send + Sync>,
        service_registry: Arc<ServiceRegistry>,
    ) -> Self {
        Self {
            runtime,
            services: tokio::sync::RwLock::new(std::collections::HashMap::new()),
            scale_semaphore: Arc::new(Semaphore::new(10)),
            overlay_manager: None,
            service_registry: Some(service_registry),
            deployment_name: None,
            health_states: Arc::new(RwLock::new(HashMap::new())),
            job_executor: None,
            cron_scheduler: None,
            container_supervisor: None,
        }
    }

    /// Create a fully-configured service manager with overlay and proxy support
    pub fn with_full_config(
        runtime: Arc<dyn Runtime + Send + Sync>,
        overlay_manager: Arc<RwLock<OverlayManager>>,
        service_registry: Arc<ServiceRegistry>,
        deployment_name: String,
    ) -> Self {
        Self {
            runtime,
            services: tokio::sync::RwLock::new(std::collections::HashMap::new()),
            scale_semaphore: Arc::new(Semaphore::new(10)),
            overlay_manager: Some(overlay_manager),
            service_registry: Some(service_registry),
            deployment_name: Some(deployment_name),
            health_states: Arc::new(RwLock::new(HashMap::new())),
            job_executor: None,
            cron_scheduler: None,
            container_supervisor: None,
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
    pub fn set_deployment_name(&mut self, name: String) {
        self.deployment_name = Some(name);
    }

    /// Set the service registry for proxy integration
    pub fn set_service_registry(&mut self, registry: Arc<ServiceRegistry>) {
        self.service_registry = Some(registry);
    }

    /// Set the overlay manager for container networking
    pub fn set_overlay_manager(&mut self, manager: Arc<RwLock<OverlayManager>>) {
        self.overlay_manager = Some(manager);
    }

    /// Set the job executor for run-to-completion workloads
    pub fn set_job_executor(&mut self, executor: Arc<JobExecutor>) {
        self.job_executor = Some(executor);
    }

    /// Set the cron scheduler for time-based job triggers
    pub fn set_cron_scheduler(&mut self, scheduler: Arc<CronScheduler>) {
        self.cron_scheduler = Some(scheduler);
    }

    /// Builder pattern: add job executor
    pub fn with_job_executor(mut self, executor: Arc<JobExecutor>) -> Self {
        self.job_executor = Some(executor);
        self
    }

    /// Builder pattern: add cron scheduler
    pub fn with_cron_scheduler(mut self, scheduler: Arc<CronScheduler>) -> Self {
        self.cron_scheduler = Some(scheduler);
        self
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
    pub fn set_container_supervisor(&mut self, supervisor: Arc<ContainerSupervisor>) {
        self.container_supervisor = Some(supervisor);
    }

    /// Builder pattern: add container supervisor
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
    /// # Returns
    /// A JoinHandle for the supervisor task, or an error if no supervisor is configured
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
    /// - Returns `AgentError::DependencyTimeout` if a dependency times out with on_timeout: fail
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
            let service_spec = services.get(service_name).ok_or_else(|| {
                AgentError::Internal(format!("Service {} not found", service_name))
            })?;

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
            self.upsert_service(service_name.clone(), service_spec.clone())
                .await?;

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
    /// Returns `AgentError::DependencyTimeout` if any dependency with on_timeout: fail times out
    async fn wait_for_dependencies(&self, service: &str, deps: &[DependsSpec]) -> Result<()> {
        let condition_checker = DependencyConditionChecker::new(
            Arc::clone(&self.runtime),
            Arc::clone(&self.health_states),
            self.service_registry.clone(),
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
                        condition: format!("{:?}", condition),
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
                WaitResult::TimedOutContinue => {
                    // Silently continue
                }
                WaitResult::Satisfied => {
                    // Good, dependency is satisfied
                }
            }
        }

        Ok(())
    }

    /// Check if all dependencies for a service are currently satisfied
    ///
    /// This is a one-shot check (no waiting). Useful for pre-flight validation.
    pub async fn check_dependencies(&self, deps: &[DependsSpec]) -> Result<bool> {
        let condition_checker = DependencyConditionChecker::new(
            Arc::clone(&self.runtime),
            Arc::clone(&self.health_states),
            self.service_registry.clone(),
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
    pub async fn upsert_service(&self, name: String, spec: ServiceSpec) -> Result<()> {
        match spec.rtype {
            ResourceType::Service => {
                // Long-running service: register routes and create/update instance
                if let Some(registry) = &self.service_registry {
                    self.register_service_routes(registry, &name, &spec);
                }

                let mut services = self.services.write().await;

                if let Some(instance) = services.get_mut(&name) {
                    // Update existing service
                    instance.spec = spec;
                } else {
                    // Create new service
                    let instance = ServiceInstance::new(name.clone(), spec, self.runtime.clone());
                    services.insert(name, instance);
                }
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
                    let instance = ServiceInstance::new(name.clone(), spec, self.runtime.clone());
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
                        "Cron scheduler not configured for cron job '{}'",
                        name
                    )));
                }
            }
        }

        Ok(())
    }

    /// Register service routes with the Pingora proxy ServiceRegistry
    fn register_service_routes(
        &self,
        registry: &ServiceRegistry,
        service_name: &str,
        spec: &ServiceSpec,
    ) {
        let deployment = self.deployment_name.as_deref().unwrap_or("default");

        for endpoint in &spec.endpoints {
            // Only register HTTP-compatible endpoints (http, https, websocket)
            let use_tls = match endpoint.protocol {
                Protocol::Http | Protocol::Websocket => false,
                Protocol::Https => true,
                Protocol::Tcp | Protocol::Udp => continue, // Skip non-HTTP protocols
            };

            let resolved = ResolvedService {
                name: service_name.to_string(),
                backends: vec![], // Backends are populated on scale operations
                use_tls,
                sni_hostname: format!("{}.{}.service", endpoint.name, service_name),
            };

            // Generate the host pattern for this endpoint
            // Format: {endpoint_name}.{service_name}.{deployment}
            let host = format!("{}.{}.{}", endpoint.name, service_name, deployment);

            // Register with optional path prefix
            registry.register(&host, endpoint.path.as_deref(), resolved.clone());

            // Also register a simpler host pattern for convenience
            // Format: {service_name}.{deployment} (matches first endpoint)
            if endpoint.name == "http" || endpoint.name == "main" || endpoint.name == "web" {
                let simple_host = format!("{}.{}", service_name, deployment);
                registry.register(&simple_host, endpoint.path.as_deref(), resolved);
            }

            tracing::debug!(
                service = %service_name,
                endpoint = %endpoint.name,
                host = %host,
                path = ?endpoint.path,
                "Registered proxy route"
            );
        }
    }

    /// Update backend addresses in the proxy ServiceRegistry after scaling
    fn update_proxy_backends(&self, service_name: &str, addrs: Vec<SocketAddr>) {
        if let Some(registry) = &self.service_registry {
            registry.update_backends(service_name, addrs);
            tracing::debug!(
                service = %service_name,
                "Updated proxy backends"
            );
        }
    }

    /// Scale a service to desired replica count
    pub async fn scale_service(&self, name: &str, replicas: u32) -> Result<()> {
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

        // After scaling, update proxy backends with new container addresses
        // Note: In a real implementation, we would get actual container IPs
        // from the overlay network or container runtime. For now, we construct
        // backend addresses based on the endpoint port and localhost (for same-node).
        // TODO: Get actual container addresses from overlay_manager or runtime
        if self.service_registry.is_some() {
            let addrs = self.collect_backend_addrs(instance, replicas).await;
            self.update_proxy_backends(name, addrs);
        }

        // Register new containers with supervisor for crash monitoring
        if let Some(supervisor) = &self.container_supervisor {
            // For scale-up, register new containers
            if replicas > current_replicas {
                for i in current_replicas..replicas {
                    let container_id = ContainerId {
                        service: name.to_string(),
                        replica: i + 1,
                    };
                    supervisor.supervise(&container_id, &instance.spec).await;
                }
            }
            // For scale-down, unregister removed containers
            if replicas < current_replicas {
                for i in replicas..current_replicas {
                    let container_id = ContainerId {
                        service: name.to_string(),
                        replica: i + 1,
                    };
                    supervisor.unsupervise(&container_id).await;
                }
            }
        }

        Ok(())
    }

    /// Collect backend addresses for a service's containers
    ///
    /// In a full implementation, this would query the overlay network manager
    /// or container runtime for actual container IP addresses. For now, we
    /// use localhost with the service's endpoint port.
    async fn collect_backend_addrs(
        &self,
        instance: &ServiceInstance,
        replicas: u32,
    ) -> Vec<SocketAddr> {
        let mut addrs = Vec::with_capacity(replicas as usize);

        // Get the primary endpoint port (first HTTP endpoint)
        let port = instance
            .spec
            .endpoints
            .iter()
            .find(|ep| {
                matches!(
                    ep.protocol,
                    Protocol::Http | Protocol::Https | Protocol::Websocket
                )
            })
            .map(|ep| ep.port)
            .unwrap_or(8080);

        // For each replica, generate a backend address
        // TODO: Replace with actual container IPs from overlay network
        // Currently using localhost with port offset per replica for testing
        for i in 0..replicas {
            // In production, this would be the container's overlay IP
            // For now, use localhost with port + replica offset
            let addr: SocketAddr = format!("127.0.0.1:{}", port + (i as u16))
                .parse()
                .unwrap_or_else(|_| SocketAddr::from(([127, 0, 0, 1], port)));
            addrs.push(addr);
        }

        addrs
    }

    /// Get service replica count
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
    pub async fn remove_service(&self, name: &str) -> Result<()> {
        // Try to unregister from cron scheduler first
        if let Some(scheduler) = &self.cron_scheduler {
            scheduler.unregister(name).await;
        }

        // Try to unregister from job executor
        if let Some(executor) = &self.job_executor {
            executor.unregister_job(name).await;
        }

        // Unregister routes from the proxy service registry
        if let Some(registry) = &self.service_registry {
            registry.unregister_service(name);
            tracing::debug!(service = %name, "Unregistered proxy routes");
        }

        // Unregister containers from the supervisor
        if let Some(supervisor) = &self.container_supervisor {
            let containers = self.get_service_containers(name).await;
            for container_id in containers {
                supervisor.unsupervise(&container_id).await;
            }
            tracing::debug!(service = %name, "Unregistered containers from supervisor");
        }

        // Remove from services map (may or may not exist depending on rtype)
        let mut services = self.services.write().await;
        if services.remove(name).is_some() {
            tracing::debug!(service = %name, "Removed service from manager");
        }

        Ok(())
    }

    /// List all services
    pub async fn list_services(&self) -> Vec<String> {
        self.services.read().await.keys().cloned().collect()
    }

    /// Get all container IDs for a specific service
    ///
    /// Returns an empty vector if the service doesn't exist.
    ///
    /// # Arguments
    /// * `service_name` - Name of the service to query
    ///
    /// # Returns
    /// Vector of ContainerIds for all replicas of the service
    pub async fn get_service_containers(&self, service_name: &str) -> Vec<ContainerId> {
        let services = self.services.read().await;
        if let Some(instance) = services.get(service_name) {
            instance.container_ids().await
        } else {
            Vec::new()
        }
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
    /// Returns a JoinHandle that can be used to wait for the scheduler to stop.
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
mod tests {
    use super::*;
    use crate::runtime::MockRuntime;

    #[tokio::test]
    async fn test_service_manager() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let manager = ServiceManager::new(runtime);

        // Add service
        let spec = mock_spec();
        manager
            .upsert_service("test".to_string(), spec)
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
    async fn test_service_manager_with_proxy() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let registry = Arc::new(ServiceRegistry::new());
        let mut manager = ServiceManager::with_proxy(runtime, registry.clone());
        manager.set_deployment_name("mydeployment".to_string());

        // Add service with HTTP endpoint
        let spec = mock_spec();
        manager
            .upsert_service("api".to_string(), spec)
            .await
            .unwrap();

        // Verify routes were registered
        assert_eq!(registry.route_count(), 2); // http.api.mydeployment + api.mydeployment

        // Scale up and verify backends are updated
        manager.scale_service("api", 2).await.unwrap();

        // Verify we can resolve the route
        let resolved = registry.resolve("http.api.mydeployment", "/").unwrap();
        assert_eq!(resolved.name, "api");
        assert_eq!(resolved.backends.len(), 2);

        // Remove service and verify routes are unregistered
        manager.remove_service("api").await.unwrap();
        assert_eq!(registry.route_count(), 0);
    }

    #[tokio::test]
    async fn test_service_manager_with_full_config() {
        use tokio::sync::RwLock;

        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let registry = Arc::new(ServiceRegistry::new());

        // Create a mock overlay manager (skip actual network setup)
        let overlay_manager = Arc::new(RwLock::new(
            OverlayManager::new("test-deployment".to_string())
                .await
                .unwrap(),
        ));

        let manager = ServiceManager::with_full_config(
            runtime,
            overlay_manager,
            registry.clone(),
            "prod".to_string(),
        );

        // Add service
        let spec = mock_spec();
        manager
            .upsert_service("web".to_string(), spec)
            .await
            .unwrap();

        // Verify routes were registered with correct deployment name
        let resolved = registry.resolve("http.web.prod", "/").unwrap();
        assert_eq!(resolved.name, "web");

        // Also verify simple host pattern
        let resolved_simple = registry.resolve("web.prod", "/").unwrap();
        assert_eq!(resolved_simple.name, "web");
    }

    fn mock_spec() -> ServiceSpec {
        serde_yaml::from_str::<zlayer_spec::DeploymentSpec>(
            r#"
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
"#,
        )
        .unwrap()
        .services
        .remove("test")
        .unwrap()
    }

    /// Helper to create a ServiceSpec with dependencies
    fn mock_spec_with_deps(deps: Vec<DependsSpec>) -> ServiceSpec {
        let mut spec = mock_spec();
        spec.depends = deps;
        spec
    }

    /// Helper to create a DependsSpec
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
        manager.deploy_with_dependencies(services).await.unwrap();

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
        manager.deploy_with_dependencies(services).await.unwrap();

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
        let result = manager.deploy_with_dependencies(services).await;
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
        manager.deploy_with_dependencies(services).await.unwrap();

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
        manager.deploy_with_dependencies(services).await.unwrap();

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
        let result = manager.deploy_with_dependencies(services).await;
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

    // ==================== Job/Cron Integration Tests ====================

    fn mock_job_spec() -> ServiceSpec {
        serde_yaml::from_str::<zlayer_spec::DeploymentSpec>(
            r#"
version: v1
deployment: test
services:
  backup:
    rtype: job
    image:
      name: backup:latest
"#,
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
        manager
            .upsert_service("backup".to_string(), job_spec)
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
        manager
            .upsert_service("cleanup".to_string(), cron_spec)
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
        manager
            .upsert_service("cleanup".to_string(), cron_spec)
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
        manager
            .upsert_service("cleanup".to_string(), cron_spec)
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
        manager
            .upsert_service("backup".to_string(), job_spec)
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
        manager
            .upsert_service("cleanup".to_string(), cron_spec)
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
        let result = manager
            .upsert_service("cleanup".to_string(), cron_spec)
            .await;
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
        manager
            .upsert_service("backup".to_string(), job_spec)
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
        manager
            .upsert_service("api".to_string(), spec)
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
        manager
            .upsert_service("web".to_string(), spec)
            .await
            .unwrap();
        manager.scale_service("web", 1).await.unwrap();

        // Check supervised state
        let container_id = ContainerId {
            service: "web".to_string(),
            replica: 1,
        };
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
}
