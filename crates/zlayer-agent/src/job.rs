//! Job execution engine - run-to-completion container lifecycle
//!
//! This module provides the JobExecutor which handles run-to-completion
//! workloads (jobs and cron jobs). Unlike services which run indefinitely,
//! jobs run to completion and track their exit status.

use crate::error::{AgentError, Result};
use crate::init::InitOrchestrator;
use crate::runtime::{ContainerId, Runtime};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};
use uuid::Uuid;
use zlayer_spec::ServiceSpec;

/// Unique identifier for a job execution
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct JobExecutionId(pub String);

impl JobExecutionId {
    /// Create a new random execution ID
    pub fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }
}

impl Default for JobExecutionId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for JobExecutionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Status of a job execution
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobStatus {
    /// Job is queued, waiting to start
    Pending,
    /// Init steps are running
    Initializing,
    /// Main container is running
    Running,
    /// Job completed successfully
    Completed { exit_code: i32, duration: Duration },
    /// Job failed
    Failed {
        reason: String,
        exit_code: Option<i32>,
    },
    /// Job was cancelled
    Cancelled,
}

impl std::fmt::Display for JobStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JobStatus::Pending => write!(f, "pending"),
            JobStatus::Initializing => write!(f, "initializing"),
            JobStatus::Running => write!(f, "running"),
            JobStatus::Completed { exit_code, .. } => write!(f, "completed({})", exit_code),
            JobStatus::Failed { exit_code, .. } => {
                if let Some(code) = exit_code {
                    write!(f, "failed({})", code)
                } else {
                    write!(f, "failed")
                }
            }
            JobStatus::Cancelled => write!(f, "cancelled"),
        }
    }
}

/// How the job was triggered
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobTrigger {
    /// Triggered via HTTP endpoint
    Endpoint { remote_addr: Option<String> },
    /// Triggered via CLI
    Cli,
    /// Triggered by cron scheduler
    Scheduler,
    /// Triggered by internal system (dependency, etc.)
    Internal { reason: String },
}

impl std::fmt::Display for JobTrigger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JobTrigger::Endpoint { remote_addr } => {
                if let Some(addr) = remote_addr {
                    write!(f, "endpoint({})", addr)
                } else {
                    write!(f, "endpoint")
                }
            }
            JobTrigger::Cli => write!(f, "cli"),
            JobTrigger::Scheduler => write!(f, "scheduler"),
            JobTrigger::Internal { reason } => write!(f, "internal({})", reason),
        }
    }
}

/// A single job execution record
#[derive(Debug, Clone)]
pub struct JobExecution {
    pub id: JobExecutionId,
    pub job_name: String,
    pub status: JobStatus,
    pub started_at: Option<Instant>,
    pub completed_at: Option<Instant>,
    pub container_id: Option<ContainerId>,
    /// Captured stdout/stderr (limited to last N bytes)
    pub logs: Option<String>,
    /// Trigger source (endpoint, cli, scheduler, etc.)
    pub trigger: JobTrigger,
}

/// Configuration for the job executor
#[derive(Debug, Clone)]
pub struct JobExecutorConfig {
    /// Maximum concurrent job executions per job name
    pub max_concurrent: usize,
    /// How long to retain completed job records
    pub retention: Duration,
    /// Maximum log size to capture (in bytes)
    pub max_log_size: usize,
}

impl Default for JobExecutorConfig {
    fn default() -> Self {
        Self {
            max_concurrent: 10,
            retention: Duration::from_secs(3600), // 1 hour
            max_log_size: 1024 * 1024,            // 1 MB
        }
    }
}

/// Job executor handles run-to-completion workloads
pub struct JobExecutor {
    runtime: Arc<dyn Runtime + Send + Sync>,
    /// Active and recent job executions
    executions: Arc<RwLock<HashMap<JobExecutionId, JobExecution>>>,
    /// Job specs (for jobs that need to be stored)
    job_specs: Arc<RwLock<HashMap<String, ServiceSpec>>>,
    /// Configuration
    config: JobExecutorConfig,
    /// Shutdown flag
    shutdown: AtomicBool,
}

impl JobExecutor {
    /// Create a new job executor with default configuration
    pub fn new(runtime: Arc<dyn Runtime + Send + Sync>) -> Self {
        Self::with_config(runtime, JobExecutorConfig::default())
    }

    /// Create a new job executor with custom configuration
    pub fn with_config(runtime: Arc<dyn Runtime + Send + Sync>, config: JobExecutorConfig) -> Self {
        Self {
            runtime,
            executions: Arc::new(RwLock::new(HashMap::new())),
            job_specs: Arc::new(RwLock::new(HashMap::new())),
            config,
            shutdown: AtomicBool::new(false),
        }
    }

    /// Register a job spec (for later triggering)
    pub async fn register_job(&self, name: &str, spec: ServiceSpec) {
        let mut specs = self.job_specs.write().await;
        specs.insert(name.to_string(), spec);
        info!(job = %name, "Registered job spec");
    }

    /// Unregister a job spec
    pub async fn unregister_job(&self, name: &str) {
        let mut specs = self.job_specs.write().await;
        specs.remove(name);
        info!(job = %name, "Unregistered job spec");
    }

    /// Get a registered job spec
    pub async fn get_job_spec(&self, name: &str) -> Option<ServiceSpec> {
        let specs = self.job_specs.read().await;
        specs.get(name).cloned()
    }

    /// Trigger a job execution
    pub async fn trigger(
        &self,
        job_name: &str,
        spec: &ServiceSpec,
        trigger: JobTrigger,
    ) -> Result<JobExecutionId> {
        if self.shutdown.load(Ordering::Relaxed) {
            return Err(AgentError::Internal("Job executor is shutting down".into()));
        }

        let exec_id = JobExecutionId::new();

        info!(
            job = %job_name,
            execution_id = %exec_id,
            trigger = %trigger,
            "Triggering job execution"
        );

        // Create execution record
        let execution = JobExecution {
            id: exec_id.clone(),
            job_name: job_name.to_string(),
            status: JobStatus::Pending,
            started_at: None,
            completed_at: None,
            container_id: None,
            logs: None,
            trigger,
        };

        // Store execution record
        {
            let mut executions = self.executions.write().await;
            executions.insert(exec_id.clone(), execution);
        }

        // Spawn the job execution task
        let runtime = self.runtime.clone();
        let spec = spec.clone();
        let exec_id_clone = exec_id.clone();
        let executions = self.executions.clone();
        let job_name = job_name.to_string();
        let max_log_size = self.config.max_log_size;

        tokio::spawn(async move {
            Self::run_job(
                runtime,
                executions,
                exec_id_clone,
                &job_name,
                spec,
                max_log_size,
            )
            .await;
        });

        Ok(exec_id)
    }

    /// Internal: Run a job to completion
    async fn run_job(
        runtime: Arc<dyn Runtime + Send + Sync>,
        executions: Arc<RwLock<HashMap<JobExecutionId, JobExecution>>>,
        exec_id: JobExecutionId,
        job_name: &str,
        spec: ServiceSpec,
        max_log_size: usize,
    ) {
        let started = Instant::now();

        // Update status to Initializing
        Self::update_status(&executions, &exec_id, |exec| {
            exec.status = JobStatus::Initializing;
            exec.started_at = Some(started);
        })
        .await;

        // Create container ID for this execution
        // Use a unique replica number based on execution ID hash
        let replica = exec_id.0.chars().take(8).collect::<String>();
        let replica_num = u32::from_str_radix(&replica, 16).unwrap_or(0) % 10000;
        let container_id = ContainerId {
            service: format!("job-{}", job_name),
            replica: replica_num,
        };

        // Store container ID
        Self::update_status(&executions, &exec_id, |exec| {
            exec.container_id = Some(container_id.clone());
        })
        .await;

        debug!(
            job = %job_name,
            execution_id = %exec_id,
            container_id = %container_id,
            "Creating job container"
        );

        // Pull image
        if let Err(e) = runtime
            .pull_image_with_policy(&spec.image.name, spec.image.pull_policy)
            .await
        {
            error!(
                job = %job_name,
                execution_id = %exec_id,
                error = %e,
                "Image pull failed"
            );
            Self::update_status(&executions, &exec_id, |exec| {
                exec.status = JobStatus::Failed {
                    reason: format!("Image pull failed: {}", e),
                    exit_code: None,
                };
                exec.completed_at = Some(Instant::now());
            })
            .await;
            return;
        }

        // Create container
        if let Err(e) = runtime.create_container(&container_id, &spec).await {
            let error_msg = e.to_string();
            error!(
                job = %job_name,
                execution_id = %exec_id,
                error = %error_msg,
                "Container create failed"
            );
            Self::update_status(&executions, &exec_id, |exec| {
                exec.status = JobStatus::Failed {
                    reason: format!("Container create failed: {}", error_msg),
                    exit_code: None,
                };
                exec.completed_at = Some(Instant::now());
            })
            .await;
            return;
        }

        // Run init steps
        let init_orchestrator = InitOrchestrator::new(container_id.clone(), spec.init.clone());
        if let Err(e) = init_orchestrator.run().await {
            let error_msg = e.to_string();
            error!(
                job = %job_name,
                execution_id = %exec_id,
                error = %error_msg,
                "Init failed"
            );
            Self::update_status(&executions, &exec_id, |exec| {
                exec.status = JobStatus::Failed {
                    reason: format!("Init failed: {}", error_msg),
                    exit_code: None,
                };
                exec.completed_at = Some(Instant::now());
            })
            .await;
            // Cleanup container
            let _ = runtime.remove_container(&container_id).await;
            return;
        }

        // Update status to Running
        Self::update_status(&executions, &exec_id, |exec| {
            exec.status = JobStatus::Running;
        })
        .await;

        debug!(
            job = %job_name,
            execution_id = %exec_id,
            "Starting job container"
        );

        // Start container
        if let Err(e) = runtime.start_container(&container_id).await {
            let error_msg = e.to_string();
            error!(
                job = %job_name,
                execution_id = %exec_id,
                error = %error_msg,
                "Container start failed"
            );
            Self::update_status(&executions, &exec_id, |exec| {
                exec.status = JobStatus::Failed {
                    reason: format!("Container start failed: {}", error_msg),
                    exit_code: None,
                };
                exec.completed_at = Some(Instant::now());
            })
            .await;
            let _ = runtime.remove_container(&container_id).await;
            return;
        }

        // Wait for container to exit using the runtime's wait_container method
        let exit_code = runtime.wait_container(&container_id).await;
        let duration = started.elapsed();

        // Collect logs before cleanup using the runtime's get_logs method
        let logs = match runtime.get_logs(&container_id).await {
            Ok(log_lines) => Some(log_lines.join("\n")),
            Err(e) => {
                // Fallback to container_logs if get_logs fails
                match runtime.container_logs(&container_id, max_log_size).await {
                    Ok(log_content) => Some(log_content),
                    Err(e2) => {
                        warn!(
                            job = %job_name,
                            execution_id = %exec_id,
                            error = %e,
                            fallback_error = %e2,
                            "Failed to collect logs"
                        );
                        None
                    }
                }
            }
        };

        // Update final status
        Self::update_status(&executions, &exec_id, |exec| {
            exec.logs = logs;
            exec.completed_at = Some(Instant::now());

            match exit_code {
                Ok(code) => {
                    if code == 0 {
                        info!(
                            job = exec.job_name,
                            execution_id = %exec.id,
                            duration_ms = duration.as_millis(),
                            "Job completed successfully"
                        );
                        exec.status = JobStatus::Completed {
                            exit_code: code,
                            duration,
                        };
                    } else {
                        warn!(
                            job = exec.job_name,
                            execution_id = %exec.id,
                            exit_code = code,
                            duration_ms = duration.as_millis(),
                            "Job failed with non-zero exit code"
                        );
                        exec.status = JobStatus::Failed {
                            reason: format!("Non-zero exit code: {}", code),
                            exit_code: Some(code),
                        };
                    }
                }
                Err(err) => {
                    error!(
                        job = exec.job_name,
                        execution_id = %exec.id,
                        error = %err,
                        "Job execution error"
                    );
                    exec.status = JobStatus::Failed {
                        reason: err.to_string(),
                        exit_code: None,
                    };
                }
            }
        })
        .await;

        // Cleanup container
        if let Err(e) = runtime.remove_container(&container_id).await {
            warn!(
                job = %job_name,
                execution_id = %exec_id,
                error = %e,
                "Failed to remove job container"
            );
        }
    }

    async fn update_status<F>(
        executions: &RwLock<HashMap<JobExecutionId, JobExecution>>,
        exec_id: &JobExecutionId,
        f: F,
    ) where
        F: FnOnce(&mut JobExecution),
    {
        let mut execs = executions.write().await;
        if let Some(exec) = execs.get_mut(exec_id) {
            f(exec);
        }
    }

    /// Get the status of a job execution
    pub async fn get_execution(&self, exec_id: &JobExecutionId) -> Option<JobExecution> {
        let executions = self.executions.read().await;
        executions.get(exec_id).cloned()
    }

    /// List all executions for a job
    pub async fn list_executions(&self, job_name: &str) -> Vec<JobExecution> {
        let executions = self.executions.read().await;
        executions
            .values()
            .filter(|e| e.job_name == job_name)
            .cloned()
            .collect()
    }

    /// List all executions (across all jobs)
    pub async fn list_all_executions(&self) -> Vec<JobExecution> {
        let executions = self.executions.read().await;
        executions.values().cloned().collect()
    }

    /// Cancel a running job execution
    pub async fn cancel(&self, exec_id: &JobExecutionId) -> Result<()> {
        let mut executions = self.executions.write().await;
        if let Some(execution) = executions.get_mut(exec_id) {
            if matches!(
                execution.status,
                JobStatus::Pending | JobStatus::Initializing | JobStatus::Running
            ) {
                if let Some(ref container_id) = execution.container_id {
                    self.runtime
                        .stop_container(container_id, Duration::from_secs(10))
                        .await?;
                    self.runtime.remove_container(container_id).await?;
                }
                execution.status = JobStatus::Cancelled;
                execution.completed_at = Some(Instant::now());
                info!(
                    job = %execution.job_name,
                    execution_id = %exec_id,
                    "Job execution cancelled"
                );
            }
        }
        Ok(())
    }

    /// Clean up old execution records
    pub async fn cleanup_old_executions(&self) {
        let now = Instant::now();
        let mut executions = self.executions.write().await;
        let before_count = executions.len();
        executions.retain(|_, exec| match exec.completed_at {
            Some(completed) => now.duration_since(completed) < self.config.retention,
            None => true, // Keep running executions
        });
        let removed = before_count - executions.len();
        if removed > 0 {
            debug!(removed = removed, "Cleaned up old job execution records");
        }
    }

    /// Signal shutdown
    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::Relaxed);
    }

    /// Check if executor is shutting down
    pub fn is_shutting_down(&self) -> bool {
        self.shutdown.load(Ordering::Relaxed)
    }

    /// Get the number of active (non-completed) executions
    pub async fn active_execution_count(&self) -> usize {
        let executions = self.executions.read().await;
        executions
            .values()
            .filter(|e| {
                matches!(
                    e.status,
                    JobStatus::Pending | JobStatus::Initializing | JobStatus::Running
                )
            })
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::MockRuntime;

    fn mock_job_spec() -> ServiceSpec {
        use zlayer_spec::*;
        serde_yaml::from_str::<DeploymentSpec>(
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

    #[tokio::test]
    async fn test_job_execution_id() {
        let id1 = JobExecutionId::new();
        let id2 = JobExecutionId::new();
        assert_ne!(id1, id2);
        assert!(!id1.0.is_empty());
    }

    #[tokio::test]
    async fn test_job_executor_trigger() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let executor = JobExecutor::new(runtime);

        let spec = mock_job_spec();
        let exec_id = executor
            .trigger("backup", &spec, JobTrigger::Cli)
            .await
            .unwrap();

        // Give the job a moment to start
        tokio::time::sleep(Duration::from_millis(50)).await;

        let execution = executor.get_execution(&exec_id).await;
        assert!(execution.is_some());

        let exec = execution.unwrap();
        assert_eq!(exec.job_name, "backup");
        assert!(matches!(exec.trigger, JobTrigger::Cli));
    }

    #[tokio::test]
    async fn test_job_executor_list_executions() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let executor = JobExecutor::new(runtime);

        let spec = mock_job_spec();

        // Trigger multiple executions
        executor
            .trigger("backup", &spec, JobTrigger::Cli)
            .await
            .unwrap();
        executor
            .trigger("backup", &spec, JobTrigger::Scheduler)
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(50)).await;

        let executions = executor.list_executions("backup").await;
        assert_eq!(executions.len(), 2);
    }

    #[tokio::test]
    async fn test_job_executor_register_spec() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let executor = JobExecutor::new(runtime);

        let spec = mock_job_spec();
        executor.register_job("backup", spec.clone()).await;

        let retrieved = executor.get_job_spec("backup").await;
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().image.name, spec.image.name);
    }

    #[tokio::test]
    async fn test_job_status_display() {
        assert_eq!(format!("{}", JobStatus::Pending), "pending");
        assert_eq!(format!("{}", JobStatus::Running), "running");
        assert_eq!(
            format!(
                "{}",
                JobStatus::Completed {
                    exit_code: 0,
                    duration: Duration::from_secs(10)
                }
            ),
            "completed(0)"
        );
        assert_eq!(
            format!(
                "{}",
                JobStatus::Failed {
                    reason: "error".into(),
                    exit_code: Some(1)
                }
            ),
            "failed(1)"
        );
    }
}
