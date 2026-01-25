//! Cron scheduler - triggers jobs on time-based schedules
//!
//! This module provides the CronScheduler which manages scheduled job executions.
//! Jobs are triggered based on cron expressions (e.g., "0 0 * * * * *" for hourly).

use crate::error::{AgentError, Result};
use crate::job::{JobExecutionId, JobExecutor, JobTrigger};
use chrono::{DateTime, Utc};
use cron::Schedule;
use spec::ServiceSpec;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

/// A registered cron job
struct CronJob {
    /// Job name
    name: String,
    /// Service specification for the job
    spec: ServiceSpec,
    /// Parsed cron schedule
    schedule: Schedule,
    /// Last time this job was run
    last_run: Option<Instant>,
    /// Next scheduled run time
    next_run: Option<DateTime<Utc>>,
    /// Whether this job is enabled
    enabled: bool,
}

/// Public info about a cron job (for external visibility)
#[derive(Debug, Clone)]
pub struct CronJobInfo {
    /// Job name
    pub name: String,
    /// Cron schedule expression
    pub schedule_expr: String,
    /// Last run time (as UTC datetime)
    pub last_run: Option<DateTime<Utc>>,
    /// Next scheduled run time
    pub next_run: Option<DateTime<Utc>>,
    /// Whether this job is enabled
    pub enabled: bool,
}

/// Cron scheduler manages time-based job triggers
pub struct CronScheduler {
    /// Registered cron jobs
    jobs: RwLock<HashMap<String, CronJob>>,
    /// Job executor for running jobs
    job_executor: Arc<JobExecutor>,
    /// Running state flag
    running: AtomicBool,
    /// Shutdown signal
    shutdown: Arc<tokio::sync::Notify>,
}

impl CronScheduler {
    /// Create a new cron scheduler
    ///
    /// # Arguments
    /// * `job_executor` - The job executor to use for running triggered jobs
    pub fn new(job_executor: Arc<JobExecutor>) -> Self {
        Self {
            jobs: RwLock::new(HashMap::new()),
            job_executor,
            running: AtomicBool::new(false),
            shutdown: Arc::new(tokio::sync::Notify::new()),
        }
    }

    /// Register a cron job
    ///
    /// # Arguments
    /// * `name` - Unique name for this cron job
    /// * `spec` - Service specification (must have rtype: cron and schedule field)
    ///
    /// # Errors
    /// Returns error if spec has no schedule or if schedule is invalid
    pub async fn register(&self, name: &str, spec: &ServiceSpec) -> Result<()> {
        let schedule_str = spec.schedule.as_ref().ok_or_else(|| {
            AgentError::InvalidSpec("Cron job missing schedule field".to_string())
        })?;

        let schedule = Schedule::from_str(schedule_str).map_err(|e| {
            AgentError::InvalidSpec(format!("Invalid cron schedule '{}': {}", schedule_str, e))
        })?;

        let next_run = schedule.upcoming(Utc).next();

        let job = CronJob {
            name: name.to_string(),
            spec: spec.clone(),
            schedule,
            last_run: None,
            next_run,
            enabled: true,
        };

        {
            let mut jobs = self.jobs.write().await;
            jobs.insert(name.to_string(), job);
        }

        info!(
            job = %name,
            schedule = %schedule_str,
            next_run = ?next_run,
            "Registered cron job"
        );

        Ok(())
    }

    /// Unregister a cron job
    ///
    /// # Arguments
    /// * `name` - Name of the cron job to unregister
    pub async fn unregister(&self, name: &str) {
        let mut jobs = self.jobs.write().await;
        if jobs.remove(name).is_some() {
            info!(job = %name, "Unregistered cron job");
        } else {
            warn!(job = %name, "Attempted to unregister non-existent cron job");
        }
    }

    /// Enable or disable a cron job
    ///
    /// When enabled, recalculates the next run time.
    pub async fn set_enabled(&self, name: &str, enabled: bool) {
        let mut jobs = self.jobs.write().await;
        if let Some(job) = jobs.get_mut(name) {
            job.enabled = enabled;
            if enabled {
                // Recalculate next run when re-enabled
                job.next_run = job.schedule.upcoming(Utc).next();
            }
            info!(
                job = %name,
                enabled = enabled,
                next_run = ?job.next_run,
                "Updated cron job enabled state"
            );
        }
    }

    /// Get info about a specific cron job
    pub async fn get_job_info(&self, name: &str) -> Option<CronJobInfo> {
        let jobs = self.jobs.read().await;
        jobs.get(name).map(|j| CronJobInfo {
            name: j.name.clone(),
            schedule_expr: j.spec.schedule.clone().unwrap_or_default(),
            last_run: j.last_run.map(|_| {
                // Convert Instant to approximate DateTime
                // Note: Instant doesn't have a direct conversion, so we approximate
                // based on current time minus elapsed duration
                Utc::now()
            }),
            next_run: j.next_run,
            enabled: j.enabled,
        })
    }

    /// List all registered cron jobs
    pub async fn list_jobs(&self) -> Vec<CronJobInfo> {
        let jobs = self.jobs.read().await;
        jobs.values()
            .map(|j| CronJobInfo {
                name: j.name.clone(),
                schedule_expr: j.spec.schedule.clone().unwrap_or_default(),
                last_run: j.last_run.map(|_| Utc::now()), // Approximate
                next_run: j.next_run,
                enabled: j.enabled,
            })
            .collect()
    }

    /// Run the scheduler loop
    ///
    /// This method runs forever, checking every second for jobs that need to be triggered.
    /// Use `shutdown()` to stop the loop gracefully.
    pub async fn run_loop(&self) {
        if self
            .running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            warn!("Cron scheduler is already running");
            return;
        }

        let check_interval = Duration::from_secs(1);
        let mut interval = tokio::time::interval(check_interval);

        info!("Cron scheduler started");

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    self.check_and_trigger().await;
                }
                _ = self.shutdown.notified() => {
                    info!("Cron scheduler received shutdown signal");
                    break;
                }
            }
        }

        self.running.store(false, Ordering::SeqCst);
        info!("Cron scheduler stopped");
    }

    /// Check all jobs and trigger those that are due
    async fn check_and_trigger(&self) {
        let now = Utc::now();
        let mut jobs_to_trigger: Vec<(String, ServiceSpec)> = Vec::new();

        // First pass: find jobs that need to be triggered
        {
            let jobs = self.jobs.read().await;
            for (name, job) in jobs.iter() {
                if !job.enabled {
                    continue;
                }

                if let Some(next_run) = job.next_run {
                    if next_run <= now {
                        debug!(
                            job = %name,
                            scheduled_time = %next_run,
                            current_time = %now,
                            "Job is due for execution"
                        );
                        jobs_to_trigger.push((name.clone(), job.spec.clone()));
                    }
                }
            }
        }

        // Second pass: trigger jobs and update their state
        for (name, spec) in jobs_to_trigger {
            match self
                .job_executor
                .trigger(&name, &spec, JobTrigger::Scheduler)
                .await
            {
                Ok(exec_id) => {
                    info!(
                        job = %name,
                        execution_id = %exec_id,
                        "Cron job triggered"
                    );

                    // Update job state
                    let mut jobs = self.jobs.write().await;
                    if let Some(job) = jobs.get_mut(&name) {
                        job.last_run = Some(Instant::now());
                        job.next_run = job.schedule.upcoming(Utc).next();
                        debug!(
                            job = %name,
                            next_run = ?job.next_run,
                            "Updated cron job next run time"
                        );
                    }
                }
                Err(e) => {
                    error!(
                        job = %name,
                        error = %e,
                        "Failed to trigger cron job"
                    );
                }
            }
        }
    }

    /// Manually trigger a cron job (outside of its schedule)
    ///
    /// # Arguments
    /// * `name` - Name of the cron job to trigger
    ///
    /// # Returns
    /// The execution ID of the triggered job
    ///
    /// # Errors
    /// Returns error if the job is not found
    pub async fn trigger_now(&self, name: &str) -> Result<JobExecutionId> {
        let jobs = self.jobs.read().await;
        let job = jobs.get(name).ok_or_else(|| AgentError::NotFound {
            container: name.to_string(),
            reason: "cron job not found".to_string(),
        })?;

        info!(job = %name, "Manually triggering cron job");

        self.job_executor
            .trigger(name, &job.spec, JobTrigger::Cli)
            .await
    }

    /// Signal the scheduler to shut down
    pub fn shutdown(&self) {
        info!("Signaling cron scheduler shutdown");
        self.shutdown.notify_one();
    }

    /// Check if the scheduler is currently running
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }

    /// Get the number of registered jobs
    pub async fn job_count(&self) -> usize {
        let jobs = self.jobs.read().await;
        jobs.len()
    }

    /// Get the number of enabled jobs
    pub async fn enabled_job_count(&self) -> usize {
        let jobs = self.jobs.read().await;
        jobs.values().filter(|j| j.enabled).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::{MockRuntime, Runtime};
    use spec::DeploymentSpec;

    fn mock_cron_spec(schedule: &str) -> ServiceSpec {
        let yaml = format!(
            r#"
version: v1
deployment: test
services:
  cleanup:
    rtype: cron
    schedule: "{}"
    image:
      name: cleanup:latest
"#,
            schedule
        );

        serde_yaml::from_str::<DeploymentSpec>(&yaml)
            .unwrap()
            .services
            .remove("cleanup")
            .unwrap()
    }

    #[tokio::test]
    async fn test_cron_scheduler_register() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let executor = Arc::new(JobExecutor::new(runtime));
        let scheduler = CronScheduler::new(executor);

        // Valid cron expression (every minute)
        let spec = mock_cron_spec("0 * * * * * *");
        scheduler.register("cleanup", &spec).await.unwrap();

        assert_eq!(scheduler.job_count().await, 1);

        let info = scheduler.get_job_info("cleanup").await;
        assert!(info.is_some());
        let info = info.unwrap();
        assert_eq!(info.name, "cleanup");
        assert!(info.enabled);
        assert!(info.next_run.is_some());
    }

    #[tokio::test]
    async fn test_cron_scheduler_invalid_schedule() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let executor = Arc::new(JobExecutor::new(runtime));
        let scheduler = CronScheduler::new(executor);

        // Create a spec manually with invalid schedule
        let mut spec = mock_cron_spec("0 * * * * * *");
        spec.schedule = Some("not a valid cron".to_string());

        let result = scheduler.register("bad", &spec).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_cron_scheduler_missing_schedule() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let executor = Arc::new(JobExecutor::new(runtime));
        let scheduler = CronScheduler::new(executor);

        // Create a spec without schedule
        let mut spec = mock_cron_spec("0 * * * * * *");
        spec.schedule = None;

        let result = scheduler.register("missing", &spec).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_cron_scheduler_unregister() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let executor = Arc::new(JobExecutor::new(runtime));
        let scheduler = CronScheduler::new(executor);

        let spec = mock_cron_spec("0 * * * * * *");
        scheduler.register("cleanup", &spec).await.unwrap();
        assert_eq!(scheduler.job_count().await, 1);

        scheduler.unregister("cleanup").await;
        assert_eq!(scheduler.job_count().await, 0);
    }

    #[tokio::test]
    async fn test_cron_scheduler_enable_disable() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let executor = Arc::new(JobExecutor::new(runtime));
        let scheduler = CronScheduler::new(executor);

        let spec = mock_cron_spec("0 * * * * * *");
        scheduler.register("cleanup", &spec).await.unwrap();

        assert_eq!(scheduler.enabled_job_count().await, 1);

        scheduler.set_enabled("cleanup", false).await;
        assert_eq!(scheduler.enabled_job_count().await, 0);

        let info = scheduler.get_job_info("cleanup").await.unwrap();
        assert!(!info.enabled);

        scheduler.set_enabled("cleanup", true).await;
        assert_eq!(scheduler.enabled_job_count().await, 1);
    }

    #[tokio::test]
    async fn test_cron_scheduler_list_jobs() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let executor = Arc::new(JobExecutor::new(runtime));
        let scheduler = CronScheduler::new(executor);

        let spec1 = mock_cron_spec("0 * * * * * *");
        let spec2 = mock_cron_spec("0 0 * * * * *");

        scheduler.register("job1", &spec1).await.unwrap();
        scheduler.register("job2", &spec2).await.unwrap();

        let jobs = scheduler.list_jobs().await;
        assert_eq!(jobs.len(), 2);

        let names: Vec<_> = jobs.iter().map(|j| j.name.as_str()).collect();
        assert!(names.contains(&"job1"));
        assert!(names.contains(&"job2"));
    }

    #[tokio::test]
    async fn test_cron_scheduler_trigger_now() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let executor = Arc::new(JobExecutor::new(runtime));
        let scheduler = CronScheduler::new(executor.clone());

        let spec = mock_cron_spec("0 * * * * * *");
        scheduler.register("cleanup", &spec).await.unwrap();

        // Manually trigger
        let exec_id = scheduler.trigger_now("cleanup").await.unwrap();
        assert!(!exec_id.0.is_empty());

        // Verify execution was created
        tokio::time::sleep(Duration::from_millis(50)).await;
        let execution = executor.get_execution(&exec_id).await;
        assert!(execution.is_some());
    }

    #[tokio::test]
    async fn test_cron_scheduler_trigger_now_not_found() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let executor = Arc::new(JobExecutor::new(runtime));
        let scheduler = CronScheduler::new(executor);

        let result = scheduler.trigger_now("nonexistent").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_cron_job_info() {
        let info = CronJobInfo {
            name: "test".to_string(),
            schedule_expr: "0 * * * * * *".to_string(),
            last_run: Some(Utc::now()),
            next_run: Some(Utc::now()),
            enabled: true,
        };

        assert_eq!(info.name, "test");
        assert!(info.enabled);
    }

    #[tokio::test]
    async fn test_cron_scheduler_shutdown() {
        let runtime: Arc<dyn Runtime + Send + Sync> = Arc::new(MockRuntime::new());
        let executor = Arc::new(JobExecutor::new(runtime));
        let scheduler = Arc::new(CronScheduler::new(executor));

        assert!(!scheduler.is_running());

        // Start scheduler in background
        let scheduler_clone = scheduler.clone();
        let handle = tokio::spawn(async move {
            scheduler_clone.run_loop().await;
        });

        // Give it time to start
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(scheduler.is_running());

        // Shutdown
        scheduler.shutdown();

        // Wait for it to stop
        tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("Scheduler should stop within timeout")
            .expect("Scheduler task should complete without error");

        assert!(!scheduler.is_running());
    }
}
