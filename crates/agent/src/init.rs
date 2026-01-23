//! Init action orchestration

use crate::error::{AgentError, Result};
use crate::runtime::ContainerId;
use init_actions::InitAction;
use spec::InitSpec;
use std::time::Duration;

/// Orchestrates init actions for a container
pub struct InitOrchestrator {
    id: ContainerId,
    spec: InitSpec,
}

impl InitOrchestrator {
    /// Create a new init orchestrator
    pub fn new(id: ContainerId, spec: InitSpec) -> Self {
        Self { id, spec }
    }

    /// Run all init steps
    pub async fn run(&self) -> Result<()> {
        let _start_grace = std::time::Instant::now();

        for step in &self.spec.steps {
            let step_start = std::time::Instant::now();

            // Parse action
            let action = init_actions::from_spec(
                &step.uses,
                &step.with,
                Duration::from_secs(30), // default
            )
            .map_err(|e| AgentError::InitActionFailed {
                id: self.id.to_string(),
                reason: e.to_string(),
            })?;

            // Execute with timeout
            let timeout = step.timeout.unwrap_or(Duration::from_secs(300));
            let result = tokio::time::timeout(timeout, self.execute_step(&action, step)).await;

            match result {
                Ok(Ok(())) => {
                    // Step succeeded
                }
                Ok(Err(e)) => {
                    // Action failed
                    return match step.on_failure {
                        spec::FailureAction::Fail => {
                            Err(AgentError::InitActionFailed {
                                id: self.id.to_string(),
                                reason: format!("step '{}' failed: {}", step.id, e),
                            })
                        }
                        spec::FailureAction::Warn => {
                            eprintln!("WARNING: Init step '{}' failed: {}", step.id, e);
                            Ok(())
                        }
                        spec::FailureAction::Continue => {
                            // Continue to next step
                            Ok(())
                        }
                    };
                }
                Err(_) => {
                    // Timeout
                    return match step.on_failure {
                        spec::FailureAction::Fail => {
                            Err(AgentError::Timeout { timeout })
                        }
                        spec::FailureAction::Warn => {
                            eprintln!("WARNING: Init step '{}' timed out", step.id);
                            Ok(())
                        }
                        spec::FailureAction::Continue => {
                            // Continue to next step
                            Ok(())
                        }
                    };
                }
            }

            // Handle retries if specified
            if let Some(retry_count) = step.retry {
                // For now, retries are handled by the action itself
                // A more sophisticated implementation would retry the entire step
                let _ = retry_count;
            }
        }

        Ok(())
    }

    async fn execute_step(&self, action: &InitAction, _step: &spec::InitStep) -> Result<()> {
        match action {
            InitAction::WaitTcp(a) => a.execute().await.map_err(|e| AgentError::InitActionFailed {
                id: self.id.to_string(),
                reason: e.to_string(),
            }),
            InitAction::WaitHttp(a) => a.execute().await.map_err(|e| AgentError::InitActionFailed {
                id: self.id.to_string(),
                reason: e.to_string(),
            }),
            InitAction::Run(a) => a.execute().await.map_err(|e| AgentError::InitActionFailed {
                id: self.id.to_string(),
                reason: e.to_string(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_init_orchestrator_success() {
        use spec::*;

        let id = ContainerId {
            service: "test".to_string(),
            replica: 1,
        };

        let spec = InitSpec { steps: vec![] };

        let orchestrator = InitOrchestrator::new(id, spec);
        orchestrator.run().await.unwrap();
    }
}
