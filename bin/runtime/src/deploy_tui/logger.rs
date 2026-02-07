//! Plain logger for deploy events in CI/non-interactive environments
//!
//! This module provides a simple line-by-line logger for deployment progress,
//! suitable for terminals, CI pipelines, and log files. It follows the same
//! pattern as the build `PlainLogger` in `zlayer-builder`.

use super::DeployEvent;
use zlayer_tui::palette::ansi;
use zlayer_tui::widgets::scrollable_pane::LogLevel;

/// Plain-text deployment logger for non-interactive output
///
/// Processes `DeployEvent`s and prints structured, human-readable output
/// with optional ANSI color support.
///
/// # Example
///
/// ```no_run
/// use crate::deploy_tui::{PlainDeployLogger, DeployEvent};
///
/// let logger = PlainDeployLogger::new();
///
/// logger.handle_event(&DeployEvent::ShutdownStarted);
/// // Output: Shutting down...
/// ```
#[derive(Debug, Clone)]
pub struct PlainDeployLogger {
    /// Whether to use ANSI color codes in output
    color: bool,
}

impl Default for PlainDeployLogger {
    fn default() -> Self {
        Self::new()
    }
}

impl PlainDeployLogger {
    /// Create a new plain deploy logger with auto-detected color support
    pub fn new() -> Self {
        Self {
            color: zlayer_tui::logger::detect_color_support(),
        }
    }

    /// Create a new plain deploy logger with explicit color setting
    pub fn with_color(color: bool) -> Self {
        Self { color }
    }

    /// Apply ANSI color codes if color is enabled
    fn colorize(&self, text: &str, color: &str) -> String {
        zlayer_tui::logger::colorize(text, color, self.color)
    }

    /// Handle a deploy event and print appropriate output
    pub fn handle_event(&self, event: &DeployEvent) {
        match event {
            DeployEvent::PlanReady {
                deployment_name,
                version,
                services,
            } => {
                let header = "=== Deployment Plan ===".to_string();
                println!("\n{}", self.colorize(&header, ansi::BOLD));
                println!("Deployment: {}", self.colorize(deployment_name, ansi::CYAN));
                println!("Version: {}", version);
                println!("Services: {}", services.len());
                println!();

                for svc in services {
                    println!("  Service: {}", self.colorize(&svc.name, ansi::BOLD));
                    println!("    Image: {}", svc.image);
                    println!("    Scale: {}", svc.scale_mode);
                    if !svc.endpoints.is_empty() {
                        println!("    Endpoints:");
                        for ep in &svc.endpoints {
                            println!("      - {}", ep);
                        }
                    }
                    println!();
                }
            }

            DeployEvent::InfraPhaseStarted { phase } => {
                let line = format!("-> Starting {}...", phase);
                println!("{}", self.colorize(&line, ansi::DIM));
            }

            DeployEvent::InfraPhaseComplete {
                phase,
                success,
                message,
            } => {
                if *success {
                    let check = self.colorize("v", ansi::GREEN);
                    let msg = message
                        .as_deref()
                        .map(|m| format!(" ({})", m))
                        .unwrap_or_default();
                    println!("  {} {}{}", check, phase, msg);
                } else {
                    let x_mark = self.colorize("x", ansi::RED);
                    let msg = message
                        .as_deref()
                        .map(|m| format!(": {}", m))
                        .unwrap_or_default();
                    let line = format!("  {} {} failed{}", x_mark, phase, msg);
                    println!("{}", self.colorize(&line, ansi::YELLOW));
                }
            }

            DeployEvent::ServiceDeployStarted { name } => {
                println!();
                let header = "=== Deploying Services ===".to_string();
                // Only print the header once (on first service). We use a simple
                // heuristic: the caller sends ServiceDeployStarted for each service,
                // so we just print the service name with an arrow.
                let line = format!("-> Deploying service: {}", name);
                // Suppress the header here; it's implied by the PlanReady event.
                let _ = header;
                println!("{}", self.colorize(&line, ansi::CYAN));
            }

            DeployEvent::ServiceRegistered { name } => {
                let check = self.colorize("v", ansi::GREEN);
                println!("  {} {} registered", check, name);
            }

            DeployEvent::ServiceScaling {
                name,
                target_replicas,
            } => {
                println!(
                    "  {} Scaling {} to {} replica(s)...",
                    self.colorize("->", ansi::DIM),
                    name,
                    target_replicas
                );
            }

            DeployEvent::ServiceReplicaUpdate {
                name,
                current,
                target,
            } => {
                // Only print when there's a meaningful change
                if current != target {
                    println!(
                        "  {} {} {}/{} replicas",
                        self.colorize("..", ansi::DIM),
                        name,
                        current,
                        target
                    );
                }
            }

            DeployEvent::ServiceDeployComplete { name, replicas } => {
                let check = self.colorize("v", ansi::GREEN);
                println!("  {} {} running ({} replicas)", check, name, replicas);
            }

            DeployEvent::ServiceDeployFailed { name, error } => {
                let x_mark = self.colorize("x", ansi::RED);
                let line = format!("  {} {} FAILED: {}", x_mark, name, error);
                eprintln!("{}", self.colorize(&line, ansi::RED));
            }

            DeployEvent::DeploymentRunning { services } => {
                println!();
                let header = "=== Deployment Summary ===".to_string();
                println!("{}", self.colorize(&header, ansi::BOLD));
                println!();

                if services.is_empty() {
                    println!("  No services running.");
                } else {
                    println!("  Running services:");
                    for (name, replicas) in services {
                        let check = self.colorize("v", ansi::GREEN);
                        println!("    {} {} ({} replicas)", check, name, replicas);
                    }
                }

                println!();
                println!(
                    "{}",
                    self.colorize("Services running. Press Ctrl+C to stop.", ansi::DIM)
                );
            }

            DeployEvent::StatusTick { .. } => {
                // Silenced in plain output - too noisy for logs/CI
            }

            DeployEvent::ShutdownStarted => {
                println!();
                println!("{}", self.colorize("Shutting down...", ansi::YELLOW));
            }

            DeployEvent::ServiceStopping { name } => {
                println!("  {} Stopping {}...", self.colorize("->", ansi::DIM), name);
            }

            DeployEvent::ServiceStopped { name } => {
                let check = self.colorize("v", ansi::GREEN);
                println!("  {} {} stopped", check, name);
            }

            DeployEvent::ShutdownComplete => {
                println!("{}", self.colorize("Shutdown complete.", ansi::GREEN));
            }

            DeployEvent::Log { level, message } => match level {
                LogLevel::Info => {
                    println!("{}", message);
                }
                LogLevel::Warn => {
                    let prefix = self.colorize("WARNING:", ansi::YELLOW);
                    println!("{} {}", prefix, message);
                }
                LogLevel::Error => {
                    let prefix = self.colorize("ERROR:", ansi::RED);
                    eprintln!("{} {}", prefix, message);
                }
            },
        }
    }

    /// Process a stream of events, printing each one
    ///
    /// This is useful for draining events from a channel receiver.
    pub fn process_events<I>(&self, events: I)
    where
        I: IntoIterator<Item = DeployEvent>,
    {
        for event in events {
            self.handle_event(&event);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deploy_tui::{InfraPhase, ServiceHealth, ServicePlan, ServiceStatus};

    #[test]
    fn test_plain_deploy_logger_creation() {
        let logger = PlainDeployLogger::new();
        // Just verify it doesn't panic
        let _ = format!("{:?}", logger);
    }

    #[test]
    fn test_with_color() {
        let logger = PlainDeployLogger::with_color(true);
        assert!(logger.color);

        let no_color = PlainDeployLogger::with_color(false);
        assert!(!no_color.color);
    }

    #[test]
    fn test_colorize_enabled() {
        let logger = PlainDeployLogger::with_color(true);
        let result = logger.colorize("test", ansi::GREEN);
        assert!(result.contains("\x1b[32m"));
        assert!(result.contains("\x1b[0m"));
        assert!(result.contains("test"));
    }

    #[test]
    fn test_colorize_disabled() {
        let logger = PlainDeployLogger::with_color(false);
        let result = logger.colorize("test", ansi::GREEN);
        assert_eq!(result, "test");
        assert!(!result.contains("\x1b["));
    }

    #[test]
    fn test_default() {
        let logger = PlainDeployLogger::default();
        let _ = format!("{:?}", logger);
    }

    #[test]
    fn test_handle_all_events_no_panic() {
        let logger = PlainDeployLogger::with_color(false);

        logger.handle_event(&DeployEvent::PlanReady {
            deployment_name: "test-app".to_string(),
            version: "v1".to_string(),
            services: vec![ServicePlan {
                name: "web".to_string(),
                image: "nginx:latest".to_string(),
                scale_mode: "fixed(2)".to_string(),
                endpoints: vec!["http:80 (public)".to_string()],
            }],
        });

        logger.handle_event(&DeployEvent::InfraPhaseStarted {
            phase: InfraPhase::Runtime,
        });

        logger.handle_event(&DeployEvent::InfraPhaseComplete {
            phase: InfraPhase::Runtime,
            success: true,
            message: Some("libcontainer".to_string()),
        });

        logger.handle_event(&DeployEvent::InfraPhaseComplete {
            phase: InfraPhase::Overlay,
            success: false,
            message: Some("not available".to_string()),
        });

        logger.handle_event(&DeployEvent::ServiceDeployStarted {
            name: "web".to_string(),
        });

        logger.handle_event(&DeployEvent::ServiceRegistered {
            name: "web".to_string(),
        });

        logger.handle_event(&DeployEvent::ServiceScaling {
            name: "web".to_string(),
            target_replicas: 3,
        });

        logger.handle_event(&DeployEvent::ServiceReplicaUpdate {
            name: "web".to_string(),
            current: 1,
            target: 3,
        });

        logger.handle_event(&DeployEvent::ServiceDeployComplete {
            name: "web".to_string(),
            replicas: 3,
        });

        logger.handle_event(&DeployEvent::ServiceDeployFailed {
            name: "broken".to_string(),
            error: "image not found".to_string(),
        });

        logger.handle_event(&DeployEvent::DeploymentRunning {
            services: vec![("web".to_string(), 3)],
        });

        logger.handle_event(&DeployEvent::StatusTick {
            services: vec![ServiceStatus {
                name: "web".to_string(),
                replicas_running: 3,
                replicas_target: 3,
                health: ServiceHealth::Healthy,
            }],
        });

        logger.handle_event(&DeployEvent::ShutdownStarted);

        logger.handle_event(&DeployEvent::ServiceStopping {
            name: "web".to_string(),
        });

        logger.handle_event(&DeployEvent::ServiceStopped {
            name: "web".to_string(),
        });

        logger.handle_event(&DeployEvent::ShutdownComplete);

        logger.handle_event(&DeployEvent::Log {
            level: LogLevel::Info,
            message: "info message".to_string(),
        });

        logger.handle_event(&DeployEvent::Log {
            level: LogLevel::Warn,
            message: "warn message".to_string(),
        });

        logger.handle_event(&DeployEvent::Log {
            level: LogLevel::Error,
            message: "error message".to_string(),
        });
    }

    #[test]
    fn test_status_tick_is_silent() {
        // StatusTick should produce no output (silenced for plain mode).
        // We can't easily capture stdout in a unit test, but we verify it
        // doesn't panic and the match arm exists.
        let logger = PlainDeployLogger::with_color(false);
        logger.handle_event(&DeployEvent::StatusTick {
            services: vec![ServiceStatus {
                name: "api".to_string(),
                replicas_running: 2,
                replicas_target: 2,
                health: ServiceHealth::Healthy,
            }],
        });
    }

    #[test]
    fn test_process_events() {
        let logger = PlainDeployLogger::with_color(false);
        let events = vec![
            DeployEvent::InfraPhaseStarted {
                phase: InfraPhase::Runtime,
            },
            DeployEvent::InfraPhaseComplete {
                phase: InfraPhase::Runtime,
                success: true,
                message: None,
            },
            DeployEvent::ShutdownComplete,
        ];
        logger.process_events(events);
    }

    #[test]
    fn test_deployment_running_empty() {
        let logger = PlainDeployLogger::with_color(false);
        logger.handle_event(&DeployEvent::DeploymentRunning { services: vec![] });
    }
}
