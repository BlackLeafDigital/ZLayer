//! Per-project git poller.
//!
//! Periodically checks the remote for new commits on projects that have
//! `poll_interval_secs` set.  When new commits are detected the local
//! working copy is fast-forward pulled and, if `auto_deploy` is true, the
//! intent to rebuild/redeploy is logged.  The actual build+deploy trigger
//! integration requires composing with the build system which is wired in
//! a later phase -- for now the log message is the intended behaviour.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tracing::{debug, info, warn};

use crate::storage::{ProjectStorage, StoredProject};

/// Per-project git poller that runs as a background task inside the daemon.
pub struct GitPoller {
    /// Project storage backend.
    project_store: Arc<dyn ProjectStorage>,
    /// Optional credential store for resolving `git_credential_id` refs.
    git_creds: Option<
        Arc<zlayer_secrets::GitCredentialStore<Arc<zlayer_secrets::PersistentSecretsStore>>>,
    >,
    /// Root directory under which each project's working copy lives
    /// (`{clone_root}/{project_id}`).
    clone_root: PathBuf,
}

impl GitPoller {
    /// Create a new poller.
    #[must_use]
    pub fn new(
        project_store: Arc<dyn ProjectStorage>,
        git_creds: Option<
            Arc<zlayer_secrets::GitCredentialStore<Arc<zlayer_secrets::PersistentSecretsStore>>>,
        >,
        clone_root: PathBuf,
    ) -> Self {
        Self {
            project_store,
            git_creds,
            clone_root,
        }
    }

    /// Start the polling loop.  Spawns a single tokio task that wakes up
    /// at a fixed base interval (10 s), lists projects with polling enabled,
    /// and for each eligible project checks whether the remote has new
    /// commits.  Returns the [`tokio::task::JoinHandle`] so the caller can
    /// abort it on shutdown.
    #[must_use]
    pub fn start(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            // Base tick: every 10 seconds we evaluate which projects are due.
            let mut tick = tokio::time::interval(Duration::from_secs(10));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            // Track the last poll time per project so we honour their
            // individual `poll_interval_secs`.
            let mut last_poll: std::collections::HashMap<String, tokio::time::Instant> =
                std::collections::HashMap::new();

            loop {
                tick.tick().await;
                self.tick_once(&mut last_poll).await;
            }
        })
    }

    /// Run one tick: list all projects, check which are due for a poll, and
    /// poll them.
    async fn tick_once(
        &self,
        last_poll: &mut std::collections::HashMap<String, tokio::time::Instant>,
    ) {
        let projects = match self.project_store.list().await {
            Ok(p) => p,
            Err(e) => {
                warn!(error = %e, "git poller: failed to list projects");
                return;
            }
        };

        let now = tokio::time::Instant::now();

        for project in &projects {
            let Some(interval_secs) = project.poll_interval_secs else {
                continue;
            };
            if interval_secs == 0 {
                continue;
            }

            let due = match last_poll.get(&project.id) {
                Some(last) => now.duration_since(*last) >= Duration::from_secs(interval_secs),
                None => true, // first time: poll immediately
            };

            if !due {
                continue;
            }

            last_poll.insert(project.id.clone(), now);

            match self.poll_project(project).await {
                Ok(Some(sha)) => {
                    info!(
                        project_id = %project.id,
                        project_name = %project.name,
                        sha = %sha,
                        auto_deploy = project.auto_deploy,
                        "git poller: new commit detected",
                    );
                    if project.auto_deploy {
                        info!(
                            project_id = %project.id,
                            project_name = %project.name,
                            sha = %sha,
                            "git poller: would trigger build+deploy \
                             (not yet wired to build system)",
                        );
                    }
                }
                Ok(None) => {
                    debug!(
                        project_id = %project.id,
                        project_name = %project.name,
                        "git poller: up to date",
                    );
                }
                Err(e) => {
                    warn!(
                        project_id = %project.id,
                        project_name = %project.name,
                        error = %e,
                        "git poller: poll failed",
                    );
                }
            }
        }
    }

    /// Poll a single project.  Returns `Ok(Some(sha))` when new commits
    /// were detected and the local working copy was advanced, `Ok(None)`
    /// when already up-to-date, or `Err` on failure.
    async fn poll_project(&self, project: &StoredProject) -> anyhow::Result<Option<String>> {
        let git_url = project
            .git_url
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("project has no git_url"))?;
        let branch = project.git_branch.as_deref().unwrap_or("main");

        let auth = self.resolve_auth(project).await?;

        // Check the remote SHA without fetching the full history.
        let remote_sha = zlayer_git::ls_remote(git_url, branch, &auth).await?;
        let Some(remote_sha) = remote_sha else {
            anyhow::bail!("branch '{branch}' not found on remote {git_url}");
        };

        let dest = self.clone_root.join(&project.id);

        // If the working copy does not exist yet, clone it.
        if !dest.exists() {
            if let Some(parent) = dest.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            let sha = zlayer_git::clone_repo(git_url, branch, &auth, &dest).await?;
            return Ok(Some(sha));
        }

        // Compare local HEAD with the remote.
        let local_sha = zlayer_git::current_sha(&dest).await?;
        if local_sha == remote_sha {
            return Ok(None);
        }

        // New commits -- fast-forward pull.
        let sha = zlayer_git::pull_ff(&dest, branch, &auth).await?;
        Ok(Some(sha))
    }

    /// Resolve git auth for a project, mirroring the logic in the pull
    /// endpoint.
    async fn resolve_auth(&self, project: &StoredProject) -> anyhow::Result<zlayer_git::GitAuth> {
        match (&project.git_credential_id, &self.git_creds) {
            (Some(cred_id), Some(git_store)) => {
                let cred = git_store
                    .get(cred_id)
                    .await
                    .map_err(|e| anyhow::anyhow!("credential lookup: {e}"))?
                    .ok_or_else(|| anyhow::anyhow!("git credential {cred_id} not found"))?;
                let value = git_store
                    .get_value(cred_id)
                    .await
                    .map_err(|e| anyhow::anyhow!("credential value: {e}"))?;
                Ok(match cred.kind {
                    zlayer_secrets::GitCredentialKind::Pat => {
                        zlayer_git::GitAuth::pat(&cred.name, value.expose())
                    }
                    zlayer_secrets::GitCredentialKind::SshKey => {
                        zlayer_git::GitAuth::ssh_key(value.expose())
                    }
                })
            }
            _ => Ok(zlayer_git::GitAuth::anonymous()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::InMemoryProjectStore;

    #[tokio::test]
    async fn poller_skips_projects_without_interval() {
        let store = Arc::new(InMemoryProjectStore::new());
        let mut project = crate::storage::StoredProject::new("no-poll");
        project.poll_interval_secs = None;
        store.store(&project).await.unwrap();

        let poller = Arc::new(GitPoller::new(
            store,
            None,
            std::env::temp_dir().join("zlayer-poller-test"),
        ));

        let mut last_poll = std::collections::HashMap::new();
        // Should not panic or error -- it simply skips the project.
        poller.tick_once(&mut last_poll).await;
        assert!(last_poll.is_empty());
    }

    #[tokio::test]
    async fn poller_tracks_due_projects() {
        let store = Arc::new(InMemoryProjectStore::new());
        let mut project = crate::storage::StoredProject::new("polled");
        project.poll_interval_secs = Some(300);
        // No git_url so poll_project will fail, but tick_once should
        // still record the attempt time.
        let pid = project.id.clone();
        store.store(&project).await.unwrap();

        let poller = Arc::new(GitPoller::new(
            store,
            None,
            std::env::temp_dir().join("zlayer-poller-test-2"),
        ));

        let mut last_poll = std::collections::HashMap::new();
        poller.tick_once(&mut last_poll).await;
        assert!(last_poll.contains_key(&pid));
    }
}
