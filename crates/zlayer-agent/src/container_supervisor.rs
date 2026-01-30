//! Container Supervisor - Monitors running containers and enforces panic/crash policies
//!
//! This module implements the `on_panic` error policy from the V1 spec (Section 16):
//! - `Restart` (default): Restart container immediately on crash
//! - `Shutdown`: Stop the service and mark as failed
//! - `Isolate`: Remove from load balancer but keep running for debugging

use crate::error::{AgentError, Result};
use crate::runtime::{ContainerId, ContainerState, Runtime};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, Notify, RwLock};
use zlayer_spec::{PanicAction, ServiceSpec};

/// Type alias for the isolate callback function
pub type IsolateCallback = Arc<dyn Fn(&ContainerId) + Send + Sync>;

/// Default maximum restarts within the tracking window before entering CrashLoopBackOff
const DEFAULT_MAX_RESTARTS: u32 = 5;
/// Default time window for tracking restarts
const DEFAULT_RESTART_WINDOW: Duration = Duration::from_secs(300); // 5 minutes
/// Default poll interval for checking container health
const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(5);
/// Backoff delay when in CrashLoopBackOff state
const CRASH_LOOP_BACKOFF_DELAY: Duration = Duration::from_secs(30);

/// Container state as tracked by the supervisor
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SupervisedState {
    /// Container is running normally
    Running,
    /// Container is being restarted
    Restarting,
    /// Container is in crash loop backoff (too many restarts)
    CrashLoopBackOff,
    /// Container has been isolated (removed from LB, kept for debugging)
    Isolated,
    /// Container/service has been shut down due to repeated failures
    Shutdown,
    /// Container exited successfully (exit code 0)
    Completed,
}

/// Information about a supervised container
#[derive(Debug, Clone)]
pub struct SupervisedContainer {
    /// Container ID
    pub id: ContainerId,
    /// Service name
    pub service_name: String,
    /// Current supervisor state
    pub state: SupervisedState,
    /// Panic action policy
    pub panic_action: PanicAction,
    /// Timestamps of recent restarts (for tracking crash loops)
    pub restart_times: Vec<Instant>,
    /// Total restart count (lifetime)
    pub total_restarts: u32,
    /// Last known exit code (if crashed)
    pub last_exit_code: Option<i32>,
    /// When the container was first supervised
    pub supervised_since: Instant,
}

impl SupervisedContainer {
    /// Create a new supervised container entry
    pub fn new(id: ContainerId, service_name: String, panic_action: PanicAction) -> Self {
        Self {
            id,
            service_name,
            state: SupervisedState::Running,
            panic_action,
            restart_times: Vec::new(),
            total_restarts: 0,
            last_exit_code: None,
            supervised_since: Instant::now(),
        }
    }

    /// Record a restart and return whether we're in a crash loop
    pub fn record_restart(&mut self, window: Duration, max_restarts: u32) -> bool {
        let now = Instant::now();
        self.restart_times.push(now);
        self.total_restarts += 1;

        // Remove restarts outside the window
        self.restart_times
            .retain(|&t| now.duration_since(t) < window);

        // Check if we've exceeded the threshold
        self.restart_times.len() as u32 > max_restarts
    }

    /// Check if the container is in a state that should be monitored
    pub fn should_monitor(&self) -> bool {
        matches!(
            self.state,
            SupervisedState::Running | SupervisedState::CrashLoopBackOff
        )
    }
}

/// Events emitted by the container supervisor
#[derive(Debug, Clone)]
pub enum SupervisorEvent {
    /// Container crashed and is being restarted
    ContainerRestarted {
        id: ContainerId,
        service_name: String,
        exit_code: i32,
        restart_count: u32,
    },
    /// Container entered crash loop backoff state
    CrashLoopBackOff {
        id: ContainerId,
        service_name: String,
        restart_count: u32,
    },
    /// Container has been isolated (removed from LB)
    ContainerIsolated {
        id: ContainerId,
        service_name: String,
        exit_code: i32,
    },
    /// Service has been shut down due to failures
    ServiceShutdown {
        id: ContainerId,
        service_name: String,
        exit_code: i32,
    },
    /// Container exited successfully
    ContainerCompleted {
        id: ContainerId,
        service_name: String,
    },
}

/// Configuration for the container supervisor
#[derive(Debug, Clone)]
pub struct SupervisorConfig {
    /// Maximum restarts within the window before crash loop backoff
    pub max_restarts: u32,
    /// Time window for tracking restarts
    pub restart_window: Duration,
    /// How often to poll container state
    pub poll_interval: Duration,
}

impl Default for SupervisorConfig {
    fn default() -> Self {
        Self {
            max_restarts: DEFAULT_MAX_RESTARTS,
            restart_window: DEFAULT_RESTART_WINDOW,
            poll_interval: DEFAULT_POLL_INTERVAL,
        }
    }
}

/// Container supervisor that monitors running containers and enforces error policies
pub struct ContainerSupervisor {
    /// Container runtime
    runtime: Arc<dyn Runtime + Send + Sync>,
    /// Supervised containers
    containers: Arc<RwLock<HashMap<ContainerId, SupervisedContainer>>>,
    /// Configuration
    config: SupervisorConfig,
    /// Event sender for supervisor events
    event_tx: mpsc::Sender<SupervisorEvent>,
    /// Event receiver for supervisor events
    event_rx: Arc<RwLock<mpsc::Receiver<SupervisorEvent>>>,
    /// Shutdown flag
    running: Arc<AtomicBool>,
    /// Shutdown notification
    shutdown: Arc<Notify>,
    /// Callback for removing container from load balancer (isolation)
    on_isolate: Option<IsolateCallback>,
}

impl ContainerSupervisor {
    /// Create a new container supervisor
    pub fn new(runtime: Arc<dyn Runtime + Send + Sync>) -> Self {
        Self::with_config(runtime, SupervisorConfig::default())
    }

    /// Create a new container supervisor with custom configuration
    pub fn with_config(runtime: Arc<dyn Runtime + Send + Sync>, config: SupervisorConfig) -> Self {
        let (event_tx, event_rx) = mpsc::channel(100);

        Self {
            runtime,
            containers: Arc::new(RwLock::new(HashMap::new())),
            config,
            event_tx,
            event_rx: Arc::new(RwLock::new(event_rx)),
            running: Arc::new(AtomicBool::new(false)),
            shutdown: Arc::new(Notify::new()),
            on_isolate: None,
        }
    }

    /// Set the callback for container isolation (removing from load balancer)
    pub fn set_isolate_callback<F>(&mut self, callback: F)
    where
        F: Fn(&ContainerId) + Send + Sync + 'static,
    {
        self.on_isolate = Some(Arc::new(callback));
    }

    /// Register a container for supervision
    ///
    /// # Arguments
    /// * `container_id` - The container ID to supervise
    /// * `spec` - The service specification (for error policies)
    pub async fn supervise(&self, container_id: &ContainerId, spec: &ServiceSpec) {
        let supervised = SupervisedContainer::new(
            container_id.clone(),
            container_id.service.clone(),
            spec.errors.on_panic.action,
        );

        let mut containers = self.containers.write().await;
        containers.insert(container_id.clone(), supervised);

        tracing::info!(
            container = %container_id,
            panic_action = ?spec.errors.on_panic.action,
            "Container registered for supervision"
        );
    }

    /// Remove a container from supervision
    pub async fn unsupervise(&self, container_id: &ContainerId) {
        let mut containers = self.containers.write().await;
        if containers.remove(container_id).is_some() {
            tracing::debug!(container = %container_id, "Container removed from supervision");
        }
    }

    /// Get the current state of a supervised container
    pub async fn get_state(&self, container_id: &ContainerId) -> Option<SupervisedState> {
        let containers = self.containers.read().await;
        containers.get(container_id).map(|c| c.state.clone())
    }

    /// Get information about a supervised container
    pub async fn get_container_info(
        &self,
        container_id: &ContainerId,
    ) -> Option<SupervisedContainer> {
        let containers = self.containers.read().await;
        containers.get(container_id).cloned()
    }

    /// Get all supervised containers
    pub async fn list_supervised(&self) -> Vec<SupervisedContainer> {
        let containers = self.containers.read().await;
        containers.values().cloned().collect()
    }

    /// Get the event receiver for consuming supervisor events
    ///
    /// Note: This takes ownership of the receiver. Only one consumer can receive events.
    pub async fn take_event_receiver(&self) -> Option<mpsc::Receiver<SupervisorEvent>> {
        let mut rx_guard = self.event_rx.write().await;
        // Create a dummy channel and swap
        let (_, dummy_rx) = mpsc::channel(1);
        let old_rx = std::mem::replace(&mut *rx_guard, dummy_rx);
        Some(old_rx)
    }

    /// Start the supervision loop
    ///
    /// This runs in the background, periodically checking container health
    /// and taking action based on the error policies.
    pub async fn run_loop(&self) {
        self.running.store(true, Ordering::SeqCst);

        tracing::info!(
            poll_interval_ms = self.config.poll_interval.as_millis(),
            "Container supervisor started"
        );

        loop {
            tokio::select! {
                _ = self.shutdown.notified() => {
                    tracing::info!("Container supervisor shutting down");
                    break;
                }
                _ = tokio::time::sleep(self.config.poll_interval) => {
                    if let Err(e) = self.check_all_containers().await {
                        tracing::error!(error = %e, "Error during container health check");
                    }
                }
            }
        }

        self.running.store(false, Ordering::SeqCst);
    }

    /// Signal the supervisor to stop
    pub fn shutdown(&self) {
        self.shutdown.notify_one();
    }

    /// Check if the supervisor is currently running
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Check all supervised containers
    async fn check_all_containers(&self) -> Result<()> {
        let containers_to_check: Vec<_> = {
            let containers = self.containers.read().await;
            containers
                .iter()
                .filter(|(_, c)| c.should_monitor())
                .map(|(id, c)| (id.clone(), c.panic_action))
                .collect()
        };

        for (container_id, panic_action) in containers_to_check {
            self.check_container(&container_id, panic_action).await?;
        }

        Ok(())
    }

    /// Check a single container's health
    async fn check_container(
        &self,
        container_id: &ContainerId,
        panic_action: PanicAction,
    ) -> Result<()> {
        let state = self.runtime.container_state(container_id).await?;

        match state {
            ContainerState::Running => {
                // Container is healthy, nothing to do
            }
            ContainerState::Exited { code } => {
                // Container has exited, handle based on exit code
                self.handle_container_exit(container_id, code, panic_action)
                    .await?;
            }
            ContainerState::Failed { reason } => {
                // Container failed, treat as crash with exit code -1
                tracing::warn!(
                    container = %container_id,
                    reason = %reason,
                    "Container reported as failed"
                );
                self.handle_container_exit(container_id, -1, panic_action)
                    .await?;
            }
            ContainerState::Pending | ContainerState::Initializing | ContainerState::Stopping => {
                // Container is in a transitional state, skip for now
            }
        }

        Ok(())
    }

    /// Handle a container exit (crash)
    async fn handle_container_exit(
        &self,
        container_id: &ContainerId,
        exit_code: i32,
        panic_action: PanicAction,
    ) -> Result<()> {
        // Get container info and update state
        let (service_name, _should_restart, in_crash_loop) = {
            let mut containers = self.containers.write().await;
            let container = match containers.get_mut(container_id) {
                Some(c) => c,
                None => return Ok(()), // Container was unregistered
            };

            container.last_exit_code = Some(exit_code);

            // Check if this is a successful exit
            if exit_code == 0 {
                container.state = SupervisedState::Completed;
                let _ = self
                    .event_tx
                    .send(SupervisorEvent::ContainerCompleted {
                        id: container_id.clone(),
                        service_name: container.service_name.clone(),
                    })
                    .await;
                return Ok(());
            }

            let service_name = container.service_name.clone();
            let in_crash_loop =
                container.record_restart(self.config.restart_window, self.config.max_restarts);

            let should_restart = match panic_action {
                PanicAction::Restart => !in_crash_loop,
                PanicAction::Shutdown => false,
                PanicAction::Isolate => false,
            };

            if in_crash_loop && matches!(panic_action, PanicAction::Restart) {
                container.state = SupervisedState::CrashLoopBackOff;
            } else if should_restart {
                container.state = SupervisedState::Restarting;
            }

            (service_name, should_restart, in_crash_loop)
        };

        // Handle based on policy
        match panic_action {
            PanicAction::Restart => {
                if in_crash_loop {
                    self.handle_crash_loop_backoff(container_id, &service_name)
                        .await?;
                } else {
                    self.restart_container(container_id, &service_name, exit_code)
                        .await?;
                }
            }
            PanicAction::Shutdown => {
                self.shutdown_container(container_id, &service_name, exit_code)
                    .await?;
            }
            PanicAction::Isolate => {
                self.isolate_container(container_id, &service_name, exit_code)
                    .await?;
            }
        }

        Ok(())
    }

    /// Restart a crashed container
    async fn restart_container(
        &self,
        container_id: &ContainerId,
        service_name: &str,
        exit_code: i32,
    ) -> Result<()> {
        let restart_count = {
            let containers = self.containers.read().await;
            containers
                .get(container_id)
                .map(|c| c.total_restarts)
                .unwrap_or(0)
        };

        tracing::info!(
            container = %container_id,
            service = %service_name,
            exit_code = exit_code,
            restart_count = restart_count,
            "Restarting crashed container"
        );

        // Start the container
        self.runtime
            .start_container(container_id)
            .await
            .map_err(|e| AgentError::StartFailed {
                id: container_id.to_string(),
                reason: e.to_string(),
            })?;

        // Update state
        {
            let mut containers = self.containers.write().await;
            if let Some(container) = containers.get_mut(container_id) {
                container.state = SupervisedState::Running;
            }
        }

        // Send event
        let _ = self
            .event_tx
            .send(SupervisorEvent::ContainerRestarted {
                id: container_id.clone(),
                service_name: service_name.to_string(),
                exit_code,
                restart_count,
            })
            .await;

        Ok(())
    }

    /// Handle a container that's in crash loop backoff
    async fn handle_crash_loop_backoff(
        &self,
        container_id: &ContainerId,
        service_name: &str,
    ) -> Result<()> {
        let restart_count = {
            let containers = self.containers.read().await;
            containers
                .get(container_id)
                .map(|c| c.total_restarts)
                .unwrap_or(0)
        };

        tracing::warn!(
            container = %container_id,
            service = %service_name,
            restart_count = restart_count,
            backoff_delay_secs = CRASH_LOOP_BACKOFF_DELAY.as_secs(),
            "Container in CrashLoopBackOff, delaying restart"
        );

        // Send event
        let _ = self
            .event_tx
            .send(SupervisorEvent::CrashLoopBackOff {
                id: container_id.clone(),
                service_name: service_name.to_string(),
                restart_count,
            })
            .await;

        // Schedule a delayed restart
        let runtime = Arc::clone(&self.runtime);
        let container_id = container_id.clone();
        let containers = Arc::clone(&self.containers);

        tokio::spawn(async move {
            tokio::time::sleep(CRASH_LOOP_BACKOFF_DELAY).await;

            // Try to restart
            if let Err(e) = runtime.start_container(&container_id).await {
                tracing::error!(
                    container = %container_id,
                    error = %e,
                    "Failed to restart container after CrashLoopBackOff delay"
                );
                return;
            }

            // Update state
            let mut containers_guard = containers.write().await;
            if let Some(container) = containers_guard.get_mut(&container_id) {
                container.state = SupervisedState::Running;
            }
        });

        Ok(())
    }

    /// Shutdown a container/service due to failures
    async fn shutdown_container(
        &self,
        container_id: &ContainerId,
        service_name: &str,
        exit_code: i32,
    ) -> Result<()> {
        tracing::warn!(
            container = %container_id,
            service = %service_name,
            exit_code = exit_code,
            "Shutting down service due to panic policy"
        );

        // Update state
        {
            let mut containers = self.containers.write().await;
            if let Some(container) = containers.get_mut(container_id) {
                container.state = SupervisedState::Shutdown;
            }
        }

        // Send event
        let _ = self
            .event_tx
            .send(SupervisorEvent::ServiceShutdown {
                id: container_id.clone(),
                service_name: service_name.to_string(),
                exit_code,
            })
            .await;

        Ok(())
    }

    /// Isolate a container (remove from LB but keep for debugging)
    async fn isolate_container(
        &self,
        container_id: &ContainerId,
        service_name: &str,
        exit_code: i32,
    ) -> Result<()> {
        tracing::info!(
            container = %container_id,
            service = %service_name,
            exit_code = exit_code,
            "Isolating container (removed from load balancer for debugging)"
        );

        // Call the isolate callback if set
        if let Some(ref callback) = self.on_isolate {
            callback(container_id);
        }

        // Update state
        {
            let mut containers = self.containers.write().await;
            if let Some(container) = containers.get_mut(container_id) {
                container.state = SupervisedState::Isolated;
            }
        }

        // Send event
        let _ = self
            .event_tx
            .send(SupervisorEvent::ContainerIsolated {
                id: container_id.clone(),
                service_name: service_name.to_string(),
                exit_code,
            })
            .await;

        Ok(())
    }

    /// Get the number of supervised containers
    pub async fn supervised_count(&self) -> usize {
        self.containers.read().await.len()
    }

    /// Get the number of containers in a specific state
    pub async fn count_by_state(&self, state: SupervisedState) -> usize {
        self.containers
            .read()
            .await
            .values()
            .filter(|c| c.state == state)
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::MockRuntime;

    fn mock_container_id(service: &str, replica: u32) -> ContainerId {
        ContainerId {
            service: service.to_string(),
            replica,
        }
    }

    fn mock_service_spec(panic_action: PanicAction) -> ServiceSpec {
        let mut spec: ServiceSpec = serde_yaml::from_str::<zlayer_spec::DeploymentSpec>(
            r#"
version: v1
deployment: test
services:
  test:
    rtype: service
    image:
      name: test:latest
"#,
        )
        .unwrap()
        .services
        .remove("test")
        .unwrap();

        spec.errors.on_panic.action = panic_action;
        spec
    }

    #[tokio::test]
    async fn test_supervisor_creation() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let supervisor = ContainerSupervisor::new(runtime);

        assert!(!supervisor.is_running());
        assert_eq!(supervisor.supervised_count().await, 0);
    }

    #[tokio::test]
    async fn test_supervisor_with_config() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let config = SupervisorConfig {
            max_restarts: 10,
            restart_window: Duration::from_secs(600),
            poll_interval: Duration::from_secs(1),
        };

        let supervisor = ContainerSupervisor::with_config(runtime, config);
        assert_eq!(supervisor.config.max_restarts, 10);
        assert_eq!(supervisor.config.restart_window, Duration::from_secs(600));
    }

    #[tokio::test]
    async fn test_supervise_container() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let supervisor = ContainerSupervisor::new(runtime);

        let container_id = mock_container_id("api", 1);
        let spec = mock_service_spec(PanicAction::Restart);

        supervisor.supervise(&container_id, &spec).await;

        assert_eq!(supervisor.supervised_count().await, 1);

        let state = supervisor.get_state(&container_id).await;
        assert_eq!(state, Some(SupervisedState::Running));
    }

    #[tokio::test]
    async fn test_unsupervise_container() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let supervisor = ContainerSupervisor::new(runtime);

        let container_id = mock_container_id("api", 1);
        let spec = mock_service_spec(PanicAction::Restart);

        supervisor.supervise(&container_id, &spec).await;
        assert_eq!(supervisor.supervised_count().await, 1);

        supervisor.unsupervise(&container_id).await;
        assert_eq!(supervisor.supervised_count().await, 0);
    }

    #[tokio::test]
    async fn test_list_supervised() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let supervisor = ContainerSupervisor::new(runtime);

        let spec = mock_service_spec(PanicAction::Restart);

        supervisor
            .supervise(&mock_container_id("api", 1), &spec)
            .await;
        supervisor
            .supervise(&mock_container_id("api", 2), &spec)
            .await;
        supervisor
            .supervise(&mock_container_id("web", 1), &spec)
            .await;

        let containers = supervisor.list_supervised().await;
        assert_eq!(containers.len(), 3);
    }

    #[tokio::test]
    async fn test_supervised_container_record_restart() {
        let mut container = SupervisedContainer::new(
            mock_container_id("api", 1),
            "api".to_string(),
            PanicAction::Restart,
        );

        // First few restarts should not trigger crash loop
        for _ in 0..5 {
            let in_loop = container.record_restart(Duration::from_secs(300), 5);
            assert!(!in_loop);
        }

        // 6th restart should trigger crash loop
        let in_loop = container.record_restart(Duration::from_secs(300), 5);
        assert!(in_loop);
    }

    #[tokio::test]
    async fn test_supervised_container_restart_window() {
        let mut container = SupervisedContainer::new(
            mock_container_id("api", 1),
            "api".to_string(),
            PanicAction::Restart,
        );

        // Record 5 restarts
        for _ in 0..5 {
            container.record_restart(Duration::from_millis(100), 5);
        }

        // Wait for window to expire
        tokio::time::sleep(Duration::from_millis(150)).await;

        // This restart should not trigger crash loop because old ones expired
        let in_loop = container.record_restart(Duration::from_millis(100), 5);
        assert!(!in_loop);
    }

    #[tokio::test]
    async fn test_get_container_info() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let supervisor = ContainerSupervisor::new(runtime);

        let container_id = mock_container_id("api", 1);
        let spec = mock_service_spec(PanicAction::Isolate);

        supervisor.supervise(&container_id, &spec).await;

        let info = supervisor.get_container_info(&container_id).await;
        assert!(info.is_some());

        let info = info.unwrap();
        assert_eq!(info.id, container_id);
        assert_eq!(info.service_name, "api");
        assert_eq!(info.panic_action, PanicAction::Isolate);
        assert_eq!(info.state, SupervisedState::Running);
        assert_eq!(info.total_restarts, 0);
    }

    #[tokio::test]
    async fn test_count_by_state() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let supervisor = ContainerSupervisor::new(runtime);

        let spec = mock_service_spec(PanicAction::Restart);

        supervisor
            .supervise(&mock_container_id("api", 1), &spec)
            .await;
        supervisor
            .supervise(&mock_container_id("api", 2), &spec)
            .await;

        assert_eq!(supervisor.count_by_state(SupervisedState::Running).await, 2);
        assert_eq!(
            supervisor
                .count_by_state(SupervisedState::CrashLoopBackOff)
                .await,
            0
        );
    }

    #[test]
    fn test_supervisor_config_default() {
        let config = SupervisorConfig::default();

        assert_eq!(config.max_restarts, DEFAULT_MAX_RESTARTS);
        assert_eq!(config.restart_window, DEFAULT_RESTART_WINDOW);
        assert_eq!(config.poll_interval, DEFAULT_POLL_INTERVAL);
    }

    #[test]
    fn test_supervised_state_should_monitor() {
        // These states should be monitored
        let container = SupervisedContainer {
            state: SupervisedState::Running,
            ..SupervisedContainer::new(
                mock_container_id("api", 1),
                "api".to_string(),
                PanicAction::Restart,
            )
        };
        assert!(container.should_monitor());

        let container = SupervisedContainer {
            state: SupervisedState::CrashLoopBackOff,
            ..SupervisedContainer::new(
                mock_container_id("api", 1),
                "api".to_string(),
                PanicAction::Restart,
            )
        };
        assert!(container.should_monitor());

        // These states should not be monitored
        let container = SupervisedContainer {
            state: SupervisedState::Shutdown,
            ..SupervisedContainer::new(
                mock_container_id("api", 1),
                "api".to_string(),
                PanicAction::Restart,
            )
        };
        assert!(!container.should_monitor());

        let container = SupervisedContainer {
            state: SupervisedState::Isolated,
            ..SupervisedContainer::new(
                mock_container_id("api", 1),
                "api".to_string(),
                PanicAction::Restart,
            )
        };
        assert!(!container.should_monitor());

        let container = SupervisedContainer {
            state: SupervisedState::Completed,
            ..SupervisedContainer::new(
                mock_container_id("api", 1),
                "api".to_string(),
                PanicAction::Restart,
            )
        };
        assert!(!container.should_monitor());
    }
}
