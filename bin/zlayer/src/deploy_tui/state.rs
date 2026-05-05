//! Deploy TUI state management
//!
//! This module tracks the entire deployment lifecycle state, processing
//! `DeployEvent`s into a structured form suitable for TUI rendering.
//! It follows the same state-tracking pattern as `BuildState` in
//! `zlayer-builder::tui::app`.

use zlayer_tui::widgets::scrollable_pane::LogEntry;

use super::{DeployEvent, InfraPhase, ServiceHealth, ServicePlan};

/// Current phase of the overall deployment
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeployPhase {
    /// Initial setup before infrastructure starts
    Initializing,
    /// Infrastructure and services are being deployed
    Deploying,
    /// Waiting for services to reach target replica counts
    Stabilizing,
    /// All services deployed and running
    Running,
    /// Graceful shutdown in progress
    ShuttingDown,
    /// Deployment fully stopped
    Complete,
}

/// Status of an infrastructure phase
#[derive(Debug, Clone)]
pub enum PhaseStatus {
    /// Not yet started
    Pending,
    /// Currently initializing
    InProgress,
    /// Successfully completed
    Complete,
    /// Failed with an error message (reserved for future fatal infra failures)
    #[allow(dead_code)]
    Failed(String),
    /// Skipped with a reason (inner String read by rendering widgets)
    #[allow(dead_code)]
    Skipped(String),
}

impl PhaseStatus {
    /// Whether this phase has finished (complete, failed, or skipped)
    #[allow(dead_code)]
    pub fn is_done(&self) -> bool {
        matches!(
            self,
            PhaseStatus::Complete | PhaseStatus::Failed(_) | PhaseStatus::Skipped(_)
        )
    }
}

/// Deployment state of a single service
#[derive(Debug, Clone)]
pub struct ServiceState {
    /// Service name
    pub name: String,
    /// Current deployment phase
    pub phase: ServiceDeployPhase,
    /// Active sub-phase (in-flight pre-scale step), if any
    pub sub_phase: ServiceSubPhase,
    /// Target replica count
    pub target_replicas: u32,
    /// Current running replica count
    pub current_replicas: u32,
    /// Health status
    pub health: ServiceHealth,
    /// Re-deploy outcome (if any) — `Skipped` when service is up-to-date,
    /// `Recreating` when digest drift was detected and the service is being
    /// rebuilt with a new image.
    pub redeploy: RedeployState,
}

/// Deployment phase of a single service
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceDeployPhase {
    /// Not yet started
    Pending,
    /// Being registered with the service manager
    Registering,
    /// Scaling to target replicas
    Scaling,
    /// Running at target replicas
    Running,
    /// Deployment failed
    Failed(String),
    /// Shutdown in progress
    Stopping,
    /// Fully stopped
    Stopped,
}

/// In-flight sub-phase for a service during the pre-scale lifecycle.
///
/// These are emitted as `*Started` events from the daemon and cleared when
/// the corresponding `*Created`/`*Configured`/registered event arrives. The
/// TUI uses them to show a "progressing" spinner on the row while the work
/// is in flight, and reverts to the regular phase indicator on completion.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ServiceSubPhase {
    /// No sub-phase in flight
    #[default]
    Idle,
    /// Service registration is in progress
    Registering,
    /// Overlay network is being configured
    OverlaySetup,
    /// Proxy is being configured
    ProxySetup,
}

/// Per-service re-deploy outcome tracking.
///
/// This is set when the daemon emits `service_up_to_date` (no digest drift,
/// service skipped) or `service_recreating` (digest drift detected, service
/// being rebuilt). It is purely informational for the TUI and does not
/// affect the main `ServiceDeployPhase` machine.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum RedeployState {
    /// No re-deploy decision yet (fresh deploy or pre-decision)
    #[default]
    None,
    /// Service was up-to-date — nothing to do. Holds the digest currently
    /// running so the TUI can show `up-to-date (sha256:abc...)`.
    UpToDate {
        /// Image digest that is currently running
        digest: String,
    },
    /// Service is being recreated with a new image. Holds both digests so the
    /// TUI can render `sha256:abc... -> sha256:def...  recreating`.
    Recreating {
        /// Previous image digest
        old_digest: String,
        /// New image digest
        new_digest: String,
    },
}

/// Full deployment state tracked by the TUI
///
/// This is the single source of truth for the deploy TUI renderer.
/// Events are applied via `apply_event` and the widgets read from
/// the public fields.
pub struct DeployState {
    /// Name of the deployment
    pub deployment_name: String,
    /// Spec version string
    pub version: String,
    /// Current overall deployment phase
    pub phase: DeployPhase,
    /// Infrastructure phase statuses (ordered)
    pub infra_phases: Vec<(InfraPhase, PhaseStatus)>,
    /// Per-service deployment states
    pub services: Vec<ServiceState>,
    /// Service plans from the `PlanReady` event
    pub service_plans: Vec<ServicePlan>,
    /// Log entries for the log pane
    pub log_entries: Vec<LogEntry>,
    /// Current scroll offset in the log pane
    pub log_scroll_offset: usize,
    /// Running service summary from `DeploymentRunning` event
    pub running_services: Vec<(String, u32)>,
    /// Active image pulls (image -> in-flight). When a pull completes, the
    /// digest is captured here for display next to the image reference.
    pub image_pulls: Vec<ImagePullStatus>,
}

/// Per-image pull progress tracker.
///
/// The TUI renders these as a top-of-screen "Pulling images" line when one or
/// more images are still pulling. Once a pull completes, the digest is stored
/// (truncated by the renderer) and the entry stays around so the user can
/// confirm what was pulled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImagePullStatus {
    /// Image reference (e.g. `nginx:latest`)
    pub image: String,
    /// Resolved digest, populated on completion
    pub digest: Option<String>,
    /// Whether the pull is still in flight
    pub in_flight: bool,
}

impl DeployState {
    /// Create a new deploy state with all 6 infrastructure phases pending
    pub fn new() -> Self {
        Self {
            deployment_name: String::new(),
            version: String::new(),
            phase: DeployPhase::Initializing,
            infra_phases: vec![
                (InfraPhase::Runtime, PhaseStatus::Pending),
                (InfraPhase::Overlay, PhaseStatus::Pending),
                (InfraPhase::Dns, PhaseStatus::Pending),
                (InfraPhase::Proxy, PhaseStatus::Pending),
                (InfraPhase::Supervisor, PhaseStatus::Pending),
                (InfraPhase::Api, PhaseStatus::Pending),
            ],
            services: Vec::new(),
            service_plans: Vec::new(),
            log_entries: Vec::new(),
            log_scroll_offset: 0,
            running_services: Vec::new(),
            image_pulls: Vec::new(),
        }
    }

    /// Apply a deploy event to update state
    ///
    /// This is the core state machine. Each event variant maps to one or
    /// more field mutations and potentially a phase transition.
    #[allow(clippy::too_many_lines)]
    pub fn apply_event(&mut self, event: &DeployEvent) {
        match event {
            DeployEvent::PlanReady {
                deployment_name,
                version,
                services,
            } => {
                self.deployment_name.clone_from(deployment_name);
                self.version.clone_from(version);
                self.service_plans.clone_from(services);
                // Seed `services` from the plan so the Services panel shows
                // one Pending row per planned service immediately. This is
                // race-free -- `PlanReady` is emitted by the CLI itself
                // before submitting to the daemon, so it always lands.
                //
                // Without this, the panel relies on the daemon's `started`
                // SSE event for seeding (see commands/deploy.rs). The
                // broadcast subscribe happens AFTER the daemon spawns the
                // orchestrator -- which fires `started` synchronously
                // before yielding -- so the CLI usually misses it and the
                // panel sits at "0/0 deployed" forever.
                for plan in services {
                    if !self.services.iter().any(|s| s.name == plan.name) {
                        self.services.push(ServiceState {
                            name: plan.name.clone(),
                            phase: ServiceDeployPhase::Pending,
                            sub_phase: ServiceSubPhase::Idle,
                            target_replicas: 0,
                            current_replicas: 0,
                            health: ServiceHealth::Unknown,
                            redeploy: RedeployState::None,
                        });
                    }
                }
                self.phase = DeployPhase::Deploying;
            }

            DeployEvent::InfraPhaseStarted { phase } => {
                if let Some(entry) = self.find_infra_phase_mut(*phase) {
                    *entry = PhaseStatus::InProgress;
                }
            }

            DeployEvent::InfraPhaseComplete {
                phase,
                success,
                message,
            } => {
                if let Some(entry) = self.find_infra_phase_mut(*phase) {
                    if *success {
                        *entry = PhaseStatus::Complete;
                    } else {
                        // Non-fatal infra failures are treated as "skipped" since
                        // the deployment continues without this phase.
                        let msg = message
                            .clone()
                            .unwrap_or_else(|| "unknown error".to_string());
                        *entry = PhaseStatus::Skipped(msg);
                    }
                }
            }

            DeployEvent::ServiceDeployStarted { name } => {
                // Add service if it doesn't already exist
                if !self.services.iter().any(|s| s.name == *name) {
                    self.services.push(ServiceState {
                        name: name.clone(),
                        phase: ServiceDeployPhase::Pending,
                        sub_phase: ServiceSubPhase::Idle,
                        target_replicas: 0,
                        current_replicas: 0,
                        health: ServiceHealth::Unknown,
                        redeploy: RedeployState::None,
                    });
                }
            }

            DeployEvent::ServiceRegistrationStarted { name } => {
                self.ensure_service(name);
                if let Some(svc) = self.find_service_mut(name) {
                    svc.sub_phase = ServiceSubPhase::Registering;
                }
            }

            DeployEvent::ServiceRegistered { name } => {
                if let Some(svc) = self.find_service_mut(name) {
                    svc.phase = ServiceDeployPhase::Registering;
                    // Clear the registration sub-phase now that it's done
                    if svc.sub_phase == ServiceSubPhase::Registering {
                        svc.sub_phase = ServiceSubPhase::Idle;
                    }
                }
            }

            DeployEvent::ServiceOverlaySetupStarted { name } => {
                self.ensure_service(name);
                if let Some(svc) = self.find_service_mut(name) {
                    svc.sub_phase = ServiceSubPhase::OverlaySetup;
                }
            }

            DeployEvent::ServiceProxySetupStarted { name } => {
                self.ensure_service(name);
                if let Some(svc) = self.find_service_mut(name) {
                    svc.sub_phase = ServiceSubPhase::ProxySetup;
                }
            }

            DeployEvent::ServiceScaling {
                name,
                target_replicas,
            } => {
                if let Some(svc) = self.find_service_mut(name) {
                    svc.phase = ServiceDeployPhase::Scaling;
                    svc.sub_phase = ServiceSubPhase::Idle;
                    svc.target_replicas = *target_replicas;
                }
                // Transition to Stabilizing when scaling starts during Deploying phase
                if self.phase == DeployPhase::Deploying {
                    self.phase = DeployPhase::Stabilizing;
                }
            }

            DeployEvent::ImagePullStarted { image } => {
                if let Some(existing) = self.image_pulls.iter_mut().find(|p| p.image == *image) {
                    existing.in_flight = true;
                } else {
                    self.image_pulls.push(ImagePullStatus {
                        image: image.clone(),
                        digest: None,
                        in_flight: true,
                    });
                }
            }

            DeployEvent::ImagePullComplete { image, digest } => {
                if let Some(existing) = self.image_pulls.iter_mut().find(|p| p.image == *image) {
                    existing.digest = Some(digest.clone());
                    existing.in_flight = false;
                } else {
                    self.image_pulls.push(ImagePullStatus {
                        image: image.clone(),
                        digest: Some(digest.clone()),
                        in_flight: false,
                    });
                }
            }

            DeployEvent::ServiceUpToDate { name, digest } => {
                self.ensure_service(name);
                if let Some(svc) = self.find_service_mut(name) {
                    svc.redeploy = RedeployState::UpToDate {
                        digest: digest.clone(),
                    };
                    // Treat up-to-date as already-running so the deployed
                    // counter and visual state line up with reality.
                    svc.phase = ServiceDeployPhase::Running;
                    svc.sub_phase = ServiceSubPhase::Idle;
                    svc.health = ServiceHealth::Healthy;
                    if svc.target_replicas == 0 {
                        svc.target_replicas = svc.current_replicas;
                    }
                }
            }

            DeployEvent::ServiceRecreating {
                name,
                old_digest,
                new_digest,
            } => {
                self.ensure_service(name);
                if let Some(svc) = self.find_service_mut(name) {
                    svc.redeploy = RedeployState::Recreating {
                        old_digest: old_digest.clone(),
                        new_digest: new_digest.clone(),
                    };
                }
            }

            DeployEvent::ServiceReplicaUpdate {
                name,
                current,
                target,
            } => {
                if let Some(svc) = self.find_service_mut(name) {
                    svc.current_replicas = *current;
                    svc.target_replicas = *target;
                    // Set health to Degraded when some but not all replicas are running
                    if *current > 0 && *current < *target {
                        svc.health = ServiceHealth::Degraded;
                    }
                }
            }

            DeployEvent::ServiceDeployComplete { name, replicas } => {
                if let Some(svc) = self.find_service_mut(name) {
                    svc.phase = ServiceDeployPhase::Running;
                    svc.sub_phase = ServiceSubPhase::Idle;
                    svc.current_replicas = *replicas;
                    svc.target_replicas = *replicas;
                    svc.health = ServiceHealth::Healthy;
                }
            }

            DeployEvent::ServiceDeployFailed { name, error } => {
                if let Some(svc) = self.find_service_mut(name) {
                    svc.phase = ServiceDeployPhase::Failed(error.clone());
                    svc.sub_phase = ServiceSubPhase::Idle;
                    svc.health = ServiceHealth::Unhealthy;
                }
            }

            DeployEvent::DeploymentRunning { services } => {
                self.phase = DeployPhase::Running;
                self.running_services.clone_from(services);
            }

            DeployEvent::StatusTick { services } => {
                for status in services {
                    if let Some(svc) = self.find_service_mut(&status.name) {
                        svc.current_replicas = status.replicas_running;
                        svc.target_replicas = status.replicas_target;
                        svc.health = status.health;
                    }
                }
            }

            DeployEvent::ShutdownStarted => {
                self.phase = DeployPhase::ShuttingDown;
            }

            DeployEvent::ServiceStopping { name } => {
                if let Some(svc) = self.find_service_mut(name) {
                    svc.phase = ServiceDeployPhase::Stopping;
                }
            }

            DeployEvent::ServiceStopped { name } => {
                if let Some(svc) = self.find_service_mut(name) {
                    svc.phase = ServiceDeployPhase::Stopped;
                    svc.current_replicas = 0;
                }
            }

            DeployEvent::ShutdownComplete => {
                self.phase = DeployPhase::Complete;
            }

            DeployEvent::Log { level, message } => {
                self.log_entries.push(LogEntry {
                    level: *level,
                    message: message.clone(),
                });
                // Auto-scroll to bottom when new log arrives
                self.scroll_to_bottom();
            }
        }
    }

    /// Number of infrastructure phases that have completed (success or failure)
    #[allow(dead_code)]
    pub fn infra_complete_count(&self) -> usize {
        self.infra_phases
            .iter()
            .filter(|(_, status)| status.is_done())
            .count()
    }

    /// Number of services that have reached the Running phase
    pub fn services_deployed_count(&self) -> usize {
        self.services
            .iter()
            .filter(|s| s.phase == ServiceDeployPhase::Running)
            .count()
    }

    /// Scroll the log pane up by `amount` lines
    pub fn scroll_up(&mut self, amount: usize) {
        self.log_scroll_offset = self.log_scroll_offset.saturating_sub(amount);
    }

    /// Scroll the log pane down by `amount` lines
    pub fn scroll_down(&mut self, amount: usize) {
        let max = self.log_entries.len();
        self.log_scroll_offset = (self.log_scroll_offset + amount).min(max);
    }

    /// Scroll the log pane to the bottom
    pub fn scroll_to_bottom(&mut self) {
        self.log_scroll_offset = self.log_entries.len();
    }

    /// Find a mutable reference to an infra phase status entry
    fn find_infra_phase_mut(&mut self, target: InfraPhase) -> Option<&mut PhaseStatus> {
        self.infra_phases
            .iter_mut()
            .find(|(phase, _)| *phase == target)
            .map(|(_, status)| status)
    }

    /// Find a mutable reference to a service state by name
    fn find_service_mut(&mut self, name: &str) -> Option<&mut ServiceState> {
        self.services.iter_mut().find(|s| s.name == name)
    }

    /// Ensure a service entry exists by name, creating a default `Pending`
    /// entry if not. This lets sub-phase events (`ServiceRegistrationStarted`
    /// etc.) work even when they arrive before the explicit
    /// `ServiceDeployStarted` (e.g. fast paths that skip the started event).
    fn ensure_service(&mut self, name: &str) {
        if !self.services.iter().any(|s| s.name == name) {
            self.services.push(ServiceState {
                name: name.to_string(),
                phase: ServiceDeployPhase::Pending,
                sub_phase: ServiceSubPhase::Idle,
                target_replicas: 0,
                current_replicas: 0,
                health: ServiceHealth::Unknown,
                redeploy: RedeployState::None,
            });
        }
    }
}

impl Default for DeployState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deploy_tui::{LogLevel, ServicePlan, ServiceStatus};

    #[test]
    fn test_new_state_has_all_infra_phases() {
        let state = DeployState::new();
        assert_eq!(state.infra_phases.len(), 6);
        assert_eq!(state.phase, DeployPhase::Initializing);
        for (_, status) in &state.infra_phases {
            assert!(matches!(status, PhaseStatus::Pending));
        }
    }

    #[test]
    fn test_default_matches_new() {
        let state = DeployState::default();
        assert_eq!(state.phase, DeployPhase::Initializing);
        assert_eq!(state.infra_phases.len(), 6);
    }

    #[test]
    fn test_plan_ready_sets_deploying() {
        let mut state = DeployState::new();
        state.apply_event(&DeployEvent::PlanReady {
            deployment_name: "my-app".to_string(),
            version: "v1.0".to_string(),
            services: vec![ServicePlan {
                name: "api".to_string(),
                image: "api:latest".to_string(),
                scale_mode: "fixed(2)".to_string(),
                endpoints: vec![],
            }],
        });

        assert_eq!(state.phase, DeployPhase::Deploying);
        assert_eq!(state.deployment_name, "my-app");
        assert_eq!(state.version, "v1.0");
        assert_eq!(state.service_plans.len(), 1);
    }

    #[test]
    fn test_infra_phase_lifecycle() {
        let mut state = DeployState::new();

        // Start Runtime phase
        state.apply_event(&DeployEvent::InfraPhaseStarted {
            phase: InfraPhase::Runtime,
        });
        assert!(matches!(state.infra_phases[0].1, PhaseStatus::InProgress));

        // Complete Runtime phase
        state.apply_event(&DeployEvent::InfraPhaseComplete {
            phase: InfraPhase::Runtime,
            success: true,
            message: Some("ok".to_string()),
        });
        assert!(matches!(state.infra_phases[0].1, PhaseStatus::Complete));

        // Non-fatal failure on Overlay phase (becomes Skipped)
        state.apply_event(&DeployEvent::InfraPhaseStarted {
            phase: InfraPhase::Overlay,
        });
        state.apply_event(&DeployEvent::InfraPhaseComplete {
            phase: InfraPhase::Overlay,
            success: false,
            message: Some("not available".to_string()),
        });
        assert!(matches!(
            state.infra_phases[1].1,
            PhaseStatus::Skipped(ref msg) if msg == "not available"
        ));

        assert_eq!(state.infra_complete_count(), 2);
    }

    #[test]
    fn test_service_deploy_lifecycle() {
        let mut state = DeployState::new();

        // Start service
        state.apply_event(&DeployEvent::ServiceDeployStarted {
            name: "web".to_string(),
        });
        assert_eq!(state.services.len(), 1);
        assert_eq!(state.services[0].phase, ServiceDeployPhase::Pending);

        // Register
        state.apply_event(&DeployEvent::ServiceRegistered {
            name: "web".to_string(),
        });
        assert_eq!(state.services[0].phase, ServiceDeployPhase::Registering);

        // Scale
        state.apply_event(&DeployEvent::ServiceScaling {
            name: "web".to_string(),
            target_replicas: 3,
        });
        assert_eq!(state.services[0].phase, ServiceDeployPhase::Scaling);
        assert_eq!(state.services[0].target_replicas, 3);

        // Replica update
        state.apply_event(&DeployEvent::ServiceReplicaUpdate {
            name: "web".to_string(),
            current: 2,
            target: 3,
        });
        assert_eq!(state.services[0].current_replicas, 2);

        // Complete
        state.apply_event(&DeployEvent::ServiceDeployComplete {
            name: "web".to_string(),
            replicas: 3,
        });
        assert_eq!(state.services[0].phase, ServiceDeployPhase::Running);
        assert_eq!(state.services[0].current_replicas, 3);
        assert_eq!(state.services[0].health, ServiceHealth::Healthy);

        assert_eq!(state.services_deployed_count(), 1);
    }

    #[test]
    fn test_service_deploy_failed() {
        let mut state = DeployState::new();

        state.apply_event(&DeployEvent::ServiceDeployStarted {
            name: "broken".to_string(),
        });
        state.apply_event(&DeployEvent::ServiceDeployFailed {
            name: "broken".to_string(),
            error: "image not found".to_string(),
        });

        assert_eq!(
            state.services[0].phase,
            ServiceDeployPhase::Failed("image not found".to_string())
        );
        assert_eq!(state.services[0].health, ServiceHealth::Unhealthy);
        assert_eq!(state.services_deployed_count(), 0);
    }

    #[test]
    fn test_duplicate_service_start_is_idempotent() {
        let mut state = DeployState::new();

        state.apply_event(&DeployEvent::ServiceDeployStarted {
            name: "web".to_string(),
        });
        state.apply_event(&DeployEvent::ServiceDeployStarted {
            name: "web".to_string(),
        });

        assert_eq!(state.services.len(), 1);
    }

    #[test]
    fn test_deployment_running() {
        let mut state = DeployState::new();

        state.apply_event(&DeployEvent::DeploymentRunning {
            services: vec![("web".to_string(), 3), ("api".to_string(), 2)],
        });

        assert_eq!(state.phase, DeployPhase::Running);
        assert_eq!(state.running_services.len(), 2);
    }

    #[test]
    fn test_status_tick_updates_health() {
        let mut state = DeployState::new();

        state.apply_event(&DeployEvent::ServiceDeployStarted {
            name: "api".to_string(),
        });
        state.apply_event(&DeployEvent::StatusTick {
            services: vec![ServiceStatus {
                name: "api".to_string(),
                replicas_running: 2,
                replicas_target: 3,
                health: ServiceHealth::Degraded,
            }],
        });

        assert_eq!(state.services[0].current_replicas, 2);
        assert_eq!(state.services[0].target_replicas, 3);
        assert_eq!(state.services[0].health, ServiceHealth::Degraded);
    }

    #[test]
    fn test_shutdown_lifecycle() {
        let mut state = DeployState::new();
        state.phase = DeployPhase::Running;

        state.apply_event(&DeployEvent::ServiceDeployStarted {
            name: "web".to_string(),
        });

        state.apply_event(&DeployEvent::ShutdownStarted);
        assert_eq!(state.phase, DeployPhase::ShuttingDown);

        state.apply_event(&DeployEvent::ServiceStopping {
            name: "web".to_string(),
        });
        assert_eq!(state.services[0].phase, ServiceDeployPhase::Stopping);

        state.apply_event(&DeployEvent::ServiceStopped {
            name: "web".to_string(),
        });
        assert_eq!(state.services[0].phase, ServiceDeployPhase::Stopped);
        assert_eq!(state.services[0].current_replicas, 0);

        state.apply_event(&DeployEvent::ShutdownComplete);
        assert_eq!(state.phase, DeployPhase::Complete);
    }

    #[test]
    fn test_log_entries_and_auto_scroll() {
        let mut state = DeployState::new();
        assert_eq!(state.log_entries.len(), 0);
        assert_eq!(state.log_scroll_offset, 0);

        state.apply_event(&DeployEvent::Log {
            level: LogLevel::Info,
            message: "first".to_string(),
        });
        assert_eq!(state.log_entries.len(), 1);
        assert_eq!(state.log_scroll_offset, 1);

        state.apply_event(&DeployEvent::Log {
            level: LogLevel::Warn,
            message: "second".to_string(),
        });
        assert_eq!(state.log_entries.len(), 2);
        assert_eq!(state.log_scroll_offset, 2);
    }

    #[test]
    fn test_scroll_up_and_down() {
        let mut state = DeployState::new();

        // Add some log entries
        for i in 0..20 {
            state.apply_event(&DeployEvent::Log {
                level: LogLevel::Info,
                message: format!("line {i}"),
            });
        }

        assert_eq!(state.log_scroll_offset, 20);

        state.scroll_up(5);
        assert_eq!(state.log_scroll_offset, 15);

        state.scroll_up(100);
        assert_eq!(state.log_scroll_offset, 0);

        state.scroll_down(10);
        assert_eq!(state.log_scroll_offset, 10);

        // Clamp to max
        state.scroll_down(100);
        assert_eq!(state.log_scroll_offset, 20);

        state.scroll_to_bottom();
        assert_eq!(state.log_scroll_offset, 20);
    }

    #[test]
    fn test_infra_complete_count_with_skipped() {
        let mut state = DeployState::new();
        state.infra_phases[2].1 = PhaseStatus::Skipped("not needed".to_string());
        state.infra_phases[0].1 = PhaseStatus::Complete;

        assert_eq!(state.infra_complete_count(), 2);
    }

    #[test]
    fn test_phase_status_is_done() {
        assert!(!PhaseStatus::Pending.is_done());
        assert!(!PhaseStatus::InProgress.is_done());
        assert!(PhaseStatus::Complete.is_done());
        assert!(PhaseStatus::Failed("err".to_string()).is_done());
        assert!(PhaseStatus::Skipped("skip".to_string()).is_done());
    }

    #[test]
    fn test_full_deploy_scenario() {
        let mut state = DeployState::new();

        // Plan
        state.apply_event(&DeployEvent::PlanReady {
            deployment_name: "prod".to_string(),
            version: "v2.1".to_string(),
            services: vec![
                ServicePlan {
                    name: "postgres".to_string(),
                    image: "postgres:16".to_string(),
                    scale_mode: "fixed(1)".to_string(),
                    endpoints: vec![],
                },
                ServicePlan {
                    name: "api".to_string(),
                    image: "api:v2.1".to_string(),
                    scale_mode: "fixed(3)".to_string(),
                    endpoints: vec!["http:8080 (public)".to_string()],
                },
            ],
        });
        assert_eq!(state.phase, DeployPhase::Deploying);

        // Infrastructure
        for phase in [
            InfraPhase::Runtime,
            InfraPhase::Overlay,
            InfraPhase::Dns,
            InfraPhase::Proxy,
            InfraPhase::Supervisor,
            InfraPhase::Api,
        ] {
            state.apply_event(&DeployEvent::InfraPhaseStarted { phase });
            state.apply_event(&DeployEvent::InfraPhaseComplete {
                phase,
                success: true,
                message: None,
            });
        }
        assert_eq!(state.infra_complete_count(), 6);

        // Services
        for name in ["postgres", "api"] {
            state.apply_event(&DeployEvent::ServiceDeployStarted {
                name: name.to_string(),
            });
            state.apply_event(&DeployEvent::ServiceRegistered {
                name: name.to_string(),
            });
        }

        state.apply_event(&DeployEvent::ServiceDeployComplete {
            name: "postgres".to_string(),
            replicas: 1,
        });
        state.apply_event(&DeployEvent::ServiceDeployComplete {
            name: "api".to_string(),
            replicas: 3,
        });

        assert_eq!(state.services_deployed_count(), 2);

        // Running
        state.apply_event(&DeployEvent::DeploymentRunning {
            services: vec![("postgres".to_string(), 1), ("api".to_string(), 3)],
        });
        assert_eq!(state.phase, DeployPhase::Running);

        // Shutdown
        state.apply_event(&DeployEvent::ShutdownStarted);
        state.apply_event(&DeployEvent::ShutdownComplete);
        assert_eq!(state.phase, DeployPhase::Complete);
    }

    #[test]
    fn test_sub_phase_lifecycle() {
        let mut state = DeployState::new();

        // Pre-register Started -> sub_phase becomes Registering
        state.apply_event(&DeployEvent::ServiceRegistrationStarted {
            name: "web".to_string(),
        });
        assert_eq!(state.services.len(), 1);
        assert_eq!(state.services[0].sub_phase, ServiceSubPhase::Registering);

        // Registered clears sub_phase and sets phase
        state.apply_event(&DeployEvent::ServiceRegistered {
            name: "web".to_string(),
        });
        assert_eq!(state.services[0].sub_phase, ServiceSubPhase::Idle);
        assert_eq!(state.services[0].phase, ServiceDeployPhase::Registering);

        // Overlay started -> sub_phase
        state.apply_event(&DeployEvent::ServiceOverlaySetupStarted {
            name: "web".to_string(),
        });
        assert_eq!(state.services[0].sub_phase, ServiceSubPhase::OverlaySetup);

        // Proxy started -> sub_phase (overrides overlay)
        state.apply_event(&DeployEvent::ServiceProxySetupStarted {
            name: "web".to_string(),
        });
        assert_eq!(state.services[0].sub_phase, ServiceSubPhase::ProxySetup);

        // Scaling clears sub_phase
        state.apply_event(&DeployEvent::ServiceScaling {
            name: "web".to_string(),
            target_replicas: 2,
        });
        assert_eq!(state.services[0].sub_phase, ServiceSubPhase::Idle);
    }

    #[test]
    fn test_image_pull_lifecycle() {
        let mut state = DeployState::new();

        state.apply_event(&DeployEvent::ImagePullStarted {
            image: "nginx:latest".to_string(),
        });
        assert_eq!(state.image_pulls.len(), 1);
        assert!(state.image_pulls[0].in_flight);
        assert!(state.image_pulls[0].digest.is_none());

        state.apply_event(&DeployEvent::ImagePullComplete {
            image: "nginx:latest".to_string(),
            digest: "sha256:abcdef0123456789".to_string(),
        });
        assert_eq!(state.image_pulls.len(), 1);
        assert!(!state.image_pulls[0].in_flight);
        assert_eq!(
            state.image_pulls[0].digest.as_deref(),
            Some("sha256:abcdef0123456789")
        );

        // A complete event for a never-started image still records it
        state.apply_event(&DeployEvent::ImagePullComplete {
            image: "alpine:3.20".to_string(),
            digest: "sha256:fedcba".to_string(),
        });
        assert_eq!(state.image_pulls.len(), 2);
    }

    #[test]
    fn test_service_up_to_date() {
        let mut state = DeployState::new();

        state.apply_event(&DeployEvent::ServiceUpToDate {
            name: "api".to_string(),
            digest: "sha256:abc".to_string(),
        });

        assert_eq!(state.services.len(), 1);
        assert_eq!(state.services[0].phase, ServiceDeployPhase::Running);
        assert_eq!(state.services[0].health, ServiceHealth::Healthy);
        match &state.services[0].redeploy {
            RedeployState::UpToDate { digest } => assert_eq!(digest, "sha256:abc"),
            other => panic!("expected UpToDate, got {other:?}"),
        }
    }

    #[test]
    fn test_service_recreating() {
        let mut state = DeployState::new();

        state.apply_event(&DeployEvent::ServiceRecreating {
            name: "api".to_string(),
            old_digest: "sha256:111".to_string(),
            new_digest: "sha256:222".to_string(),
        });

        assert_eq!(state.services.len(), 1);
        match &state.services[0].redeploy {
            RedeployState::Recreating {
                old_digest,
                new_digest,
            } => {
                assert_eq!(old_digest, "sha256:111");
                assert_eq!(new_digest, "sha256:222");
            }
            other => panic!("expected Recreating, got {other:?}"),
        }
    }
}
