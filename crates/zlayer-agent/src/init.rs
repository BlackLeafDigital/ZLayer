//! Init action orchestration

use crate::error::{AgentError, Result};
use crate::runtime::ContainerId;
use std::time::Duration;
use zlayer_init_actions::InitAction;
use zlayer_spec::{ErrorsSpec, InitFailureAction, InitSpec};

/// Default max retry attempts for init failure policies
const DEFAULT_MAX_INIT_RETRIES: u32 = 3;

/// Backoff configuration for exponential backoff
#[derive(Debug, Clone)]
pub struct BackoffConfig {
    /// Initial delay before first retry
    pub initial_delay: Duration,
    /// Maximum delay between retries
    pub max_delay: Duration,
    /// Multiplier for each subsequent retry
    pub multiplier: f64,
}

impl Default for BackoffConfig {
    fn default() -> Self {
        Self {
            initial_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(60),
            multiplier: 2.0,
        }
    }
}

impl BackoffConfig {
    /// Calculate the delay for a given retry attempt (0-indexed)
    pub fn delay_for_attempt(&self, attempt: u32) -> Duration {
        let delay_secs = self.initial_delay.as_secs_f64() * self.multiplier.powi(attempt as i32);
        let capped_secs = delay_secs.min(self.max_delay.as_secs_f64());
        Duration::from_secs_f64(capped_secs)
    }
}

/// Orchestrates init actions for a container
pub struct InitOrchestrator {
    id: ContainerId,
    spec: InitSpec,
    /// Error handling policy from the service spec
    error_policy: ErrorsSpec,
    /// Maximum retry attempts for init failure
    max_retries: u32,
    /// Backoff configuration for exponential backoff
    backoff_config: BackoffConfig,
}

impl InitOrchestrator {
    /// Create a new init orchestrator
    pub fn new(id: ContainerId, spec: InitSpec) -> Self {
        Self {
            id,
            spec,
            error_policy: ErrorsSpec::default(),
            max_retries: DEFAULT_MAX_INIT_RETRIES,
            backoff_config: BackoffConfig::default(),
        }
    }

    /// Create a new init orchestrator with error policy
    pub fn with_error_policy(id: ContainerId, spec: InitSpec, error_policy: ErrorsSpec) -> Self {
        Self {
            id,
            spec,
            error_policy,
            max_retries: DEFAULT_MAX_INIT_RETRIES,
            backoff_config: BackoffConfig::default(),
        }
    }

    /// Set the maximum retry attempts
    pub fn with_max_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = max_retries;
        self
    }

    /// Set the backoff configuration
    pub fn with_backoff_config(mut self, config: BackoffConfig) -> Self {
        self.backoff_config = config;
        self
    }

    /// Run all init steps with error policy enforcement
    ///
    /// The behavior on init failure depends on the `errors.on_init_failure` policy:
    /// - `Fail` (default): Stop container creation, return error immediately
    /// - `Restart`: Retry init steps (up to max_retries) with immediate retry
    /// - `Backoff`: Retry with exponential backoff (1s, 2s, 4s, 8s, max 60s)
    pub async fn run(&self) -> Result<()> {
        match self.error_policy.on_init_failure.action {
            InitFailureAction::Fail => {
                // Fail immediately on any error
                self.run_init_steps().await
            }
            InitFailureAction::Restart => {
                // Retry immediately on failure
                self.run_with_retries(false).await
            }
            InitFailureAction::Backoff => {
                // Retry with exponential backoff
                self.run_with_retries(true).await
            }
        }
    }

    /// Run init steps with retry logic
    async fn run_with_retries(&self, use_backoff: bool) -> Result<()> {
        let mut last_error = None;

        for attempt in 0..=self.max_retries {
            if attempt > 0 {
                let delay = if use_backoff {
                    self.backoff_config.delay_for_attempt(attempt - 1)
                } else {
                    Duration::from_millis(100) // Small delay for immediate retry
                };

                tracing::info!(
                    container = %self.id,
                    attempt = attempt,
                    max_retries = self.max_retries,
                    delay_ms = delay.as_millis(),
                    "Retrying init steps after failure"
                );

                tokio::time::sleep(delay).await;
            }

            match self.run_init_steps().await {
                Ok(()) => {
                    if attempt > 0 {
                        tracing::info!(
                            container = %self.id,
                            attempt = attempt,
                            "Init steps succeeded after retry"
                        );
                    }
                    return Ok(());
                }
                Err(e) => {
                    tracing::warn!(
                        container = %self.id,
                        attempt = attempt,
                        max_retries = self.max_retries,
                        error = %e,
                        "Init steps failed"
                    );
                    last_error = Some(e);
                }
            }
        }

        // All retries exhausted
        Err(last_error.unwrap_or_else(|| AgentError::InitActionFailed {
            id: self.id.to_string(),
            reason: "Init failed after all retries".to_string(),
        }))
    }

    /// Run all init steps once (no retry logic)
    async fn run_init_steps(&self) -> Result<()> {
        let _start_grace = std::time::Instant::now();

        for step in &self.spec.steps {
            let _step_start = std::time::Instant::now();

            // Parse action
            let action = zlayer_init_actions::from_spec(
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
                        zlayer_spec::FailureAction::Fail => Err(AgentError::InitActionFailed {
                            id: self.id.to_string(),
                            reason: format!("step '{}' failed: {}", step.id, e),
                        }),
                        zlayer_spec::FailureAction::Warn => {
                            tracing::warn!(
                                container = %self.id,
                                step = %step.id,
                                error = %e,
                                "Init step failed (continuing due to warn policy)"
                            );
                            continue; // Continue to next step
                        }
                        zlayer_spec::FailureAction::Continue => {
                            // Continue to next step silently
                            continue;
                        }
                    };
                }
                Err(_) => {
                    // Timeout
                    return match step.on_failure {
                        zlayer_spec::FailureAction::Fail => Err(AgentError::Timeout { timeout }),
                        zlayer_spec::FailureAction::Warn => {
                            tracing::warn!(
                                container = %self.id,
                                step = %step.id,
                                timeout_secs = timeout.as_secs(),
                                "Init step timed out (continuing due to warn policy)"
                            );
                            continue; // Continue to next step
                        }
                        zlayer_spec::FailureAction::Continue => {
                            // Continue to next step silently
                            continue;
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

    async fn execute_step(&self, action: &InitAction, _step: &zlayer_spec::InitStep) -> Result<()> {
        match action {
            InitAction::WaitTcp(a) => a.execute().await.map_err(|e| AgentError::InitActionFailed {
                id: self.id.to_string(),
                reason: e.to_string(),
            }),
            InitAction::WaitHttp(a) => {
                a.execute().await.map_err(|e| AgentError::InitActionFailed {
                    id: self.id.to_string(),
                    reason: e.to_string(),
                })
            }
            InitAction::Run(a) => a.execute().await.map_err(|e| AgentError::InitActionFailed {
                id: self.id.to_string(),
                reason: e.to_string(),
            }),
            #[cfg(feature = "s3")]
            InitAction::S3Push(a) => a.execute().await.map_err(|e| AgentError::InitActionFailed {
                id: self.id.to_string(),
                reason: e.to_string(),
            }),
            #[cfg(feature = "s3")]
            InitAction::S3Pull(a) => a.execute().await.map_err(|e| AgentError::InitActionFailed {
                id: self.id.to_string(),
                reason: e.to_string(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_container_id() -> ContainerId {
        ContainerId {
            service: "test".to_string(),
            replica: 1,
        }
    }

    #[tokio::test]
    async fn test_init_orchestrator_success() {
        let id = test_container_id();
        let spec = InitSpec { steps: vec![] };

        let orchestrator = InitOrchestrator::new(id, spec);
        orchestrator.run().await.unwrap();
    }

    #[tokio::test]
    async fn test_init_orchestrator_with_error_policy() {
        let id = test_container_id();
        let spec = InitSpec { steps: vec![] };
        let error_policy = ErrorsSpec::default();

        let orchestrator = InitOrchestrator::with_error_policy(id, spec, error_policy);
        orchestrator.run().await.unwrap();
    }

    #[test]
    fn test_backoff_config_default() {
        let config = BackoffConfig::default();
        assert_eq!(config.initial_delay, Duration::from_secs(1));
        assert_eq!(config.max_delay, Duration::from_secs(60));
        assert_eq!(config.multiplier, 2.0);
    }

    #[test]
    fn test_backoff_delay_calculation() {
        let config = BackoffConfig::default();

        // Attempt 0: 1s
        assert_eq!(config.delay_for_attempt(0), Duration::from_secs(1));
        // Attempt 1: 2s
        assert_eq!(config.delay_for_attempt(1), Duration::from_secs(2));
        // Attempt 2: 4s
        assert_eq!(config.delay_for_attempt(2), Duration::from_secs(4));
        // Attempt 3: 8s
        assert_eq!(config.delay_for_attempt(3), Duration::from_secs(8));
        // Attempt 4: 16s
        assert_eq!(config.delay_for_attempt(4), Duration::from_secs(16));
        // Attempt 5: 32s
        assert_eq!(config.delay_for_attempt(5), Duration::from_secs(32));
        // Attempt 6: would be 64s, but capped at 60s
        assert_eq!(config.delay_for_attempt(6), Duration::from_secs(60));
        // Attempt 7: still capped at 60s
        assert_eq!(config.delay_for_attempt(7), Duration::from_secs(60));
    }

    #[test]
    fn test_backoff_custom_config() {
        let config = BackoffConfig {
            initial_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(10),
            multiplier: 3.0,
        };

        // Attempt 0: 100ms
        assert_eq!(config.delay_for_attempt(0), Duration::from_millis(100));
        // Attempt 1: 300ms
        assert_eq!(config.delay_for_attempt(1), Duration::from_millis(300));
        // Attempt 2: 900ms
        assert_eq!(config.delay_for_attempt(2), Duration::from_millis(900));
        // Attempt 3: 2700ms
        assert_eq!(config.delay_for_attempt(3), Duration::from_millis(2700));
        // Attempt 4: 8100ms
        assert_eq!(config.delay_for_attempt(4), Duration::from_millis(8100));
        // Attempt 5: would be 24300ms, but capped at 10s
        assert_eq!(config.delay_for_attempt(5), Duration::from_secs(10));
    }

    #[test]
    fn test_orchestrator_builder_pattern() {
        let id = test_container_id();
        let spec = InitSpec { steps: vec![] };

        let orchestrator = InitOrchestrator::new(id.clone(), spec.clone())
            .with_max_retries(5)
            .with_backoff_config(BackoffConfig {
                initial_delay: Duration::from_millis(500),
                max_delay: Duration::from_secs(30),
                multiplier: 1.5,
            });

        assert_eq!(orchestrator.max_retries, 5);
        assert_eq!(
            orchestrator.backoff_config.initial_delay,
            Duration::from_millis(500)
        );
    }
}
