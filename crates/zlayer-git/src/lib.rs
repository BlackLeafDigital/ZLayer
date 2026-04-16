//! Git operations for `ZLayer`'s project system.
//!
//! # Implementation approach
//!
//! This crate is backed by [`gix`](https://docs.rs/gix) (gitoxide), a pure-Rust
//! git implementation. Every operation is performed in-process without shelling
//! out to the `git` CLI or linking `libgit2`. The blocking gix APIs are dispatched
//! through [`tokio::task::spawn_blocking`] so the operations remain usable from
//! `async` callers while still running on a runtime worker that is allowed to
//! block.
//!
//! # Authentication
//!
//! * **Anonymous** — no credentials are configured; the remote must accept
//!   unauthenticated reads.
//! * **HTTPS + Personal Access Token** — a [`gix::remote::Connection`] is
//!   installed with a custom credential closure (via
//!   [`gix::remote::Connection::with_credentials`]). When the transport asks
//!   for credentials the closure returns the configured username/token. The
//!   token never touches the filesystem and never appears on a command line.
//! * **SSH key** — the private key is materialised in a private
//!   [`tempfile::TempDir`] with mode `0600` on Unix. The `GIT_SSH_COMMAND`
//!   environment variable is set for the duration of the operation so that
//!   the SSH transport (which `gix` delegates to the system `ssh` binary)
//!   picks up the key. Host-key checking is disabled because `ZLayer` manages
//!   peer trust separately; callers that need strict host verification should
//!   layer that in. An internal mutex serialises SSH-authenticated operations
//!   to prevent concurrent callers from clobbering each other's environment.
//!
//! All temporary files are cleaned up when the [`tempfile::TempDir`] guard is
//! dropped at the end of each operation.

pub mod sync;

use std::path::Path;

use anyhow::{anyhow, Result};

/// Credentials for a git operation.
///
/// Use [`GitAuth::anonymous`] for public repositories, [`GitAuth::pat`] for
/// HTTPS URLs with a personal access token, or [`GitAuth::ssh_key`] for SSH
/// URLs with a private key.
#[derive(Debug, Clone, Default)]
pub struct GitAuth {
    /// Username for HTTPS PAT authentication. Many providers accept any
    /// non-empty placeholder (e.g. `"x-access-token"`, `"oauth2"`).
    pub username: Option<String>,
    /// Password or personal access token for HTTPS authentication.
    pub password_or_token: Option<String>,
    /// PEM-encoded SSH private key content. When set, SSH auth is used.
    pub ssh_key: Option<String>,
}

impl GitAuth {
    /// No credentials. Only works against publicly readable remotes.
    #[must_use]
    pub fn anonymous() -> Self {
        Self::default()
    }

    /// HTTPS authentication with a username and personal access token.
    #[must_use]
    pub fn pat(username: &str, token: &str) -> Self {
        Self {
            username: Some(username.to_string()),
            password_or_token: Some(token.to_string()),
            ssh_key: None,
        }
    }

    /// SSH authentication using the given PEM-encoded private key.
    #[must_use]
    pub fn ssh_key(key_pem: &str) -> Self {
        Self {
            username: None,
            password_or_token: None,
            ssh_key: Some(key_pem.to_string()),
        }
    }

    fn kind(&self) -> AuthKind {
        if self.ssh_key.is_some() {
            AuthKind::Ssh
        } else if self.password_or_token.is_some() {
            AuthKind::Pat
        } else {
            AuthKind::Anonymous
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
enum AuthKind {
    Anonymous,
    Pat,
    Ssh,
}

/// Clone `url` at `branch` into `dest` and return the SHA of the new HEAD.
///
/// `dest` must not already exist (or must be empty); `gix` will refuse to
/// clone over a populated directory.
///
/// # Errors
///
/// Returns an error if the clone fails, the authentication setup fails, or
/// the resulting HEAD cannot be resolved.
pub async fn clone_repo(url: &str, branch: &str, auth: &GitAuth, dest: &Path) -> Result<String> {
    let url = url.to_string();
    let branch = branch.to_string();
    let auth = auth.clone();
    let dest = dest.to_path_buf();
    spawn_blocking_result(move || blocking::clone_repo(&url, &branch, &auth, &dest)).await
}

/// Fetch from `origin` for the repository at `repo_path`.
///
/// # Errors
///
/// Returns an error if the fetch fails or authentication setup fails.
pub async fn fetch(repo_path: &Path, auth: &GitAuth) -> Result<()> {
    let repo_path = repo_path.to_path_buf();
    let auth = auth.clone();
    spawn_blocking_result(move || blocking::fetch(&repo_path, &auth)).await
}

/// Fetch from `origin` then fast-forward merge `branch` from the remote.
/// Returns the resulting HEAD SHA.
///
/// # Errors
///
/// Returns an error if the merge is not a fast-forward, if the fetch or
/// checkout step fails, or if `branch` does not exist on the remote.
pub async fn pull_ff(repo_path: &Path, branch: &str, auth: &GitAuth) -> Result<String> {
    let repo_path = repo_path.to_path_buf();
    let branch = branch.to_string();
    let auth = auth.clone();
    spawn_blocking_result(move || blocking::pull_ff(&repo_path, &branch, &auth)).await
}

/// Resolve `spec` to a full 40-character SHA-1 (or 64-char SHA-256) object id.
///
/// # Errors
///
/// Returns an error if the revision cannot be resolved.
pub async fn rev_parse(repo_path: &Path, spec: &str) -> Result<String> {
    let repo_path = repo_path.to_path_buf();
    let spec = spec.to_string();
    spawn_blocking_result(move || blocking::rev_parse(&repo_path, &spec)).await
}

/// Current HEAD SHA of the repository at `repo_path`.
///
/// # Errors
///
/// Returns an error if HEAD cannot be resolved.
pub async fn current_sha(repo_path: &Path) -> Result<String> {
    let repo_path = repo_path.to_path_buf();
    spawn_blocking_result(move || blocking::current_sha(&repo_path)).await
}

/// Check out `target` (a branch name, tag, or SHA) in the repository at
/// `repo_path`. HEAD is detached at the resolved object.
///
/// # Errors
///
/// Returns an error if the target cannot be resolved or the worktree cannot
/// be updated.
pub async fn checkout(repo_path: &Path, target: &str) -> Result<()> {
    let repo_path = repo_path.to_path_buf();
    let target = target.to_string();
    spawn_blocking_result(move || blocking::checkout(&repo_path, &target)).await
}

/// Query the remote for the SHA of `branch` without cloning or fetching.
///
/// Returns `None` if the branch does not exist on the remote.
///
/// # Errors
///
/// Returns an error if the remote listing fails (e.g. bad URL, network
/// error, authentication failure).
pub async fn ls_remote(url: &str, branch: &str, auth: &GitAuth) -> Result<Option<String>> {
    let url = url.to_string();
    let branch = branch.to_string();
    let auth = auth.clone();
    spawn_blocking_result(move || blocking::ls_remote(&url, &branch, &auth)).await
}

/// Run `work` on the tokio blocking thread pool and flatten the nested
/// `Result` from the join handle and the task itself.
async fn spawn_blocking_result<T, F>(work: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T> + Send + 'static,
{
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| anyhow!("git blocking task join error: {e}"))?
}

mod blocking {
    //! Synchronous gix-backed implementations of the public async API.
    //!
    //! Everything in this module must be callable from a
    //! [`tokio::task::spawn_blocking`] closure — i.e. no `.await`, no
    //! non-`Send` handles escaping.

    use super::{AuthKind, GitAuth};
    use anyhow::{anyhow, bail, Context, Result};
    use gix::bstr::ByteSlice;
    use gix::progress::Discard;
    use gix::refs::transaction::{Change, LogChange, PreviousValue, RefEdit, RefLog};
    use gix::refs::Target;
    use std::path::Path;
    use std::sync::Mutex;
    use tempfile::TempDir;

    /// Environment variables applied for the duration of a git operation,
    /// holding any on-disk credential material alive via a [`TempDir`].
    ///
    /// The `ssh_key_dir` field is intentionally only used for its `Drop`
    /// impl — it keeps the on-disk SSH key alive for the duration of the
    /// operation, after which the temp directory and its contents vanish.
    struct AuthEnv {
        _ssh_key_dir: Option<TempDir>,
        ssh_command: Option<String>,
    }

    impl AuthEnv {
        fn build(auth: &GitAuth) -> Result<Self> {
            match auth.kind() {
                AuthKind::Anonymous | AuthKind::Pat => Ok(Self {
                    _ssh_key_dir: None,
                    ssh_command: None,
                }),
                AuthKind::Ssh => build_ssh_env(auth),
            }
        }
    }

    fn build_ssh_env(auth: &GitAuth) -> Result<AuthEnv> {
        let key = auth
            .ssh_key
            .as_ref()
            .ok_or_else(|| anyhow!("SSH auth requested but no key supplied"))?;

        let tmp = tempfile::Builder::new()
            .prefix("zlayer-git-ssh-")
            .tempdir()
            .context("creating tempdir for SSH key")?;
        let key_path = tmp.path().join("id_key");
        write_ssh_key(&key_path, key)?;

        // Disable host-key verification: ZLayer manages peer trust separately,
        // and we never want to hang on an interactive prompt.
        let ssh_cmd = format!(
            "ssh -i {} -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o IdentitiesOnly=yes -o BatchMode=yes",
            shell_quote(&key_path.to_string_lossy()),
        );

        Ok(AuthEnv {
            _ssh_key_dir: Some(tmp),
            ssh_command: Some(ssh_cmd),
        })
    }

    #[cfg(unix)]
    fn write_ssh_key(path: &Path, key_pem: &str) -> Result<()> {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;

        let mut file = std::fs::File::create(path)
            .with_context(|| format!("writing SSH key to {}", path.display()))?;
        let mut perms = file.metadata()?.permissions();
        perms.set_mode(0o600);
        std::fs::set_permissions(path, perms).context("setting SSH key file to 0600")?;
        file.write_all(key_pem.as_bytes())
            .context("writing SSH key contents")?;
        // Ensure a trailing newline — OpenSSH is picky about truncated keys.
        if !key_pem.ends_with('\n') {
            file.write_all(b"\n")
                .context("appending final newline to SSH key")?;
        }
        Ok(())
    }

    #[cfg(windows)]
    fn write_ssh_key(path: &Path, key_pem: &str) -> Result<()> {
        use std::io::Write;
        let mut file = std::fs::File::create(path)
            .with_context(|| format!("writing SSH key to {}", path.display()))?;
        file.write_all(key_pem.as_bytes())
            .context("writing SSH key contents")?;
        if !key_pem.ends_with('\n') {
            file.write_all(b"\n")
                .context("appending final newline to SSH key")?;
        }
        Ok(())
    }

    /// Very small shell-quoter for embedding an absolute path into
    /// `GIT_SSH_COMMAND`. We only need to defend against spaces and quotes
    /// because our paths come from [`TempDir::path`] and are under our control.
    fn shell_quote(s: &str) -> String {
        let mut out = String::with_capacity(s.len() + 2);
        out.push('"');
        for c in s.chars() {
            if c == '"' || c == '\\' {
                out.push('\\');
            }
            out.push(c);
        }
        out.push('"');
        out
    }

    /// Serialises SSH-authenticated operations so they don't clobber each
    /// other's `GIT_SSH_COMMAND` environment variable.
    static SSH_ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Guard that restores `GIT_SSH_COMMAND` (and friends) to their prior
    /// values when dropped.
    struct SshEnvGuard<'a> {
        _lock: std::sync::MutexGuard<'a, ()>,
        previous: Vec<(&'static str, Option<std::ffi::OsString>)>,
    }

    impl Drop for SshEnvGuard<'_> {
        fn drop(&mut self) {
            for (key, value) in self.previous.drain(..) {
                match value {
                    Some(v) => std::env::set_var(key, v),
                    None => std::env::remove_var(key),
                }
            }
        }
    }

    /// Acquire the SSH-env lock and set the requested environment variables,
    /// returning a guard that restores them on drop.
    fn apply_ssh_env(command: &str) -> SshEnvGuard<'static> {
        // Poisoning is benign: we're only storing a zero-sized unit. Recover
        // so we don't panic surprise callers.
        let lock = match SSH_ENV_LOCK.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        let previous = [
            ("GIT_SSH_COMMAND", std::env::var_os("GIT_SSH_COMMAND")),
            (
                "GIT_TERMINAL_PROMPT",
                std::env::var_os("GIT_TERMINAL_PROMPT"),
            ),
        ]
        .into_iter()
        .collect();
        std::env::set_var("GIT_SSH_COMMAND", command);
        std::env::set_var("GIT_TERMINAL_PROMPT", "0");
        SshEnvGuard {
            _lock: lock,
            previous,
        }
    }

    /// Run `f` with any credentials/env state from `auth` applied, then drop
    /// the state. The closure borrows no gix types beyond those it creates.
    ///
    /// The `env` argument is held by reference for the duration of `f` so
    /// that any [`TempDir`] it owns (e.g. the on-disk SSH key) lives at
    /// least until `f` returns.
    fn with_auth<T>(auth: &GitAuth, env: &AuthEnv, f: impl FnOnce() -> Result<T>) -> Result<T> {
        let ssh_guard = match (auth.kind(), env.ssh_command.as_deref()) {
            (AuthKind::Ssh, Some(cmd)) => Some(apply_ssh_env(cmd)),
            _ => None,
        };
        let out = f();
        // Explicitly dropped here so the `GIT_SSH_COMMAND` env var is
        // restored before we return to the caller. `env.ssh_key_dir` is kept
        // alive by the caller's borrow of `env` for the whole lifetime of
        // this function.
        drop(ssh_guard);
        out
    }

    /// Build a credential-helper closure suitable for installing on a
    /// [`gix::remote::Connection`] via `with_credentials`.
    ///
    /// For anonymous/SSH auth the closure returns `Ok(None)` so that gix
    /// falls back to its defaults (or, for SSH, just lets the transport
    /// handle auth via `GIT_SSH_COMMAND`).
    fn credentials_for(
        auth: &GitAuth,
    ) -> Option<
        impl FnMut(gix::credentials::helper::Action) -> gix::credentials::protocol::Result + 'static,
    > {
        use gix::credentials::helper::{Action, NextAction};
        use gix::credentials::protocol;
        use gix::sec::identity::Account;

        if auth.kind() != AuthKind::Pat {
            return None;
        }
        let username = auth
            .username
            .clone()
            .unwrap_or_else(|| "x-access-token".to_string());
        let password = auth.password_or_token.clone().unwrap_or_default();

        Some(move |action: Action| -> protocol::Result {
            match action {
                Action::Get(ctx) => Ok(Some(protocol::Outcome {
                    identity: Account {
                        username: username.clone(),
                        password: password.clone(),
                        oauth_refresh_token: None,
                    },
                    next: NextAction::from(ctx),
                })),
                Action::Store(_) | Action::Erase(_) => Ok(None),
            }
        })
    }

    /// Materialise a fresh in-memory [`gix::Repository`] opened at `path`.
    fn open_repo(path: &Path) -> Result<gix::Repository> {
        gix::open(path).with_context(|| format!("opening git repository at {}", path.display()))
    }

    /// Ensure `repo` has a committer identity available for reflog writes.
    ///
    /// gix 0.81 refuses to update references with `RefLog::AndReference` when
    /// the resolved config has no `user.name` / `user.email` (or their
    /// `committer.*` / `GIT_COMMITTER_*` equivalents). On a developer's laptop
    /// this is supplied by `~/.gitconfig`; on CI runners and inside fresh
    /// containers / rootless sandboxes it is not. When we detect a missing
    /// committer we install a generic in-memory fallback so every blocking
    /// operation that writes a reflog — `fetch`, `pull_ff`, `checkout` — works
    /// regardless of the host environment.
    ///
    /// This mutation is per-`Repository` only (no env vars, no process-global
    /// state) and takes effect immediately because `commit()` drops the
    /// cached `Personas`.
    fn ensure_committer(repo: &mut gix::Repository) -> Result<()> {
        if repo.committer().is_some() {
            return Ok(());
        }
        let mut snap = repo.config_snapshot_mut();
        snap.set_value(&gix::config::tree::User::NAME, "ZLayer")
            .context("installing fallback user.name for reflog writes")?;
        snap.set_value(&gix::config::tree::User::EMAIL, "zlayer@localhost")
            .context("installing fallback user.email for reflog writes")?;
        snap.commit()
            .context("committing fallback committer identity to repo config")?;
        Ok(())
    }

    pub(super) fn clone_repo(
        url: &str,
        branch: &str,
        auth: &GitAuth,
        dest: &Path,
    ) -> Result<String> {
        let env = AuthEnv::build(auth)?;
        with_auth(auth, &env, || {
            let mut prepare = gix::prepare_clone(url, dest)
                .with_context(|| format!("preparing clone of {url}"))?
                .with_ref_name(Some(branch))
                .with_context(|| format!("configuring branch {branch} for clone"))?;

            let auth = auth.clone();
            prepare = prepare.configure_connection(move |connection| {
                if let Some(cb) = credentials_for(&auth) {
                    connection.set_credentials(cb);
                }
                Ok(())
            });

            let (mut checkout, _) = prepare
                .fetch_then_checkout(Discard, &gix::interrupt::IS_INTERRUPTED)
                .with_context(|| format!("fetching from {url}"))?;
            let (repo, _) = checkout
                .main_worktree(Discard, &gix::interrupt::IS_INTERRUPTED)
                .with_context(|| format!("writing worktree into {}", dest.display()))?;
            let head = repo.head_id().context("resolving HEAD after clone")?;
            Ok(head.to_string())
        })
    }

    pub(super) fn fetch(repo_path: &Path, auth: &GitAuth) -> Result<()> {
        let env = AuthEnv::build(auth)?;
        with_auth(auth, &env, || {
            let mut repo = open_repo(repo_path)?;
            ensure_committer(&mut repo)?;
            let mut remote = repo
                .find_remote("origin")
                .context("locating 'origin' remote for fetch")?;
            remote.replace_refspecs(
                Some("+refs/heads/*:refs/remotes/origin/*"),
                gix::remote::Direction::Fetch,
            )?;

            let mut connection = remote
                .connect(gix::remote::Direction::Fetch)
                .context("connecting to remote 'origin' for fetch")?;
            if let Some(cb) = credentials_for(auth) {
                connection.set_credentials(cb);
            }

            let outcome = connection
                .prepare_fetch(Discard, gix::remote::ref_map::Options::default())
                .context("preparing fetch from 'origin'")?
                .receive(Discard, &gix::interrupt::IS_INTERRUPTED)
                .context("receiving pack from 'origin'")?;
            tracing::trace!(
                target: "zlayer_git",
                remote_refs = outcome.ref_map.remote_refs.len(),
                "fetch from origin complete",
            );
            Ok(())
        })
    }

    pub(super) fn pull_ff(repo_path: &Path, branch: &str, auth: &GitAuth) -> Result<String> {
        fetch(repo_path, auth)?;

        let mut repo = open_repo(repo_path)?;
        ensure_committer(&mut repo)?;
        let local_ref_name = format!("refs/heads/{branch}");
        let remote_ref_name = format!("refs/remotes/origin/{branch}");

        let local_id = repo
            .rev_parse_single(local_ref_name.as_str())
            .with_context(|| format!("resolving local branch {branch}"))?
            .detach();
        let remote_id = repo
            .rev_parse_single(remote_ref_name.as_str())
            .with_context(|| format!("resolving remote branch origin/{branch}"))?
            .detach();

        if local_id == remote_id {
            return Ok(local_id.to_string());
        }

        // Fast-forward check: the merge-base of local and remote must be the
        // local tip. Otherwise the histories have diverged and a true merge
        // would be required.
        let merge_base = repo
            .merge_base(local_id, remote_id)
            .with_context(|| format!("computing merge-base between {local_id} and {remote_id}"))?
            .detach();
        if merge_base != local_id {
            bail!(
                "pull_ff: local branch {branch} has diverged from origin/{branch}; \
                 merge-base {merge_base} != local tip {local_id}"
            );
        }

        // Advance the local branch to the remote tip, and update the worktree
        // if HEAD currently points at this branch.
        let full_local_ref: gix::refs::FullName = local_ref_name.as_str().try_into()?;
        repo.edit_reference(RefEdit {
            change: Change::Update {
                log: LogChange {
                    mode: RefLog::AndReference,
                    force_create_reflog: false,
                    message: format!("pull_ff: fast-forward to {remote_id}").into(),
                },
                expected: PreviousValue::MustExistAndMatch(Target::Object(local_id)),
                new: Target::Object(remote_id),
            },
            name: full_local_ref.clone(),
            deref: false,
        })?;

        let head_points_at_branch = repo
            .head()
            .context("reading HEAD after fast-forward")?
            .referent_name()
            .is_some_and(|n| n.as_bstr() == full_local_ref.as_bstr());
        if head_points_at_branch {
            rebuild_worktree(&repo, remote_id)?;
        }

        Ok(remote_id.to_string())
    }

    pub(super) fn rev_parse(repo_path: &Path, spec: &str) -> Result<String> {
        let repo = open_repo(repo_path)?;
        let id = repo
            .rev_parse_single(spec)
            .with_context(|| format!("rev-parse '{spec}'"))?;
        Ok(id.to_string())
    }

    pub(super) fn current_sha(repo_path: &Path) -> Result<String> {
        let repo = open_repo(repo_path)?;
        Ok(repo.head_id().context("resolving HEAD")?.to_string())
    }

    pub(super) fn checkout(repo_path: &Path, target: &str) -> Result<()> {
        let mut repo = open_repo(repo_path)?;
        ensure_committer(&mut repo)?;
        let id = repo
            .rev_parse_single(target)
            .with_context(|| format!("resolving checkout target '{target}'"))?
            .detach();
        rebuild_worktree(&repo, id)?;

        // Detach HEAD at the resolved object.
        let head_name: gix::refs::FullName = "HEAD".try_into()?;
        repo.edit_reference(RefEdit {
            change: Change::Update {
                log: LogChange {
                    mode: RefLog::AndReference,
                    force_create_reflog: false,
                    message: format!("checkout: moving to {id}").into(),
                },
                expected: PreviousValue::Any,
                new: Target::Object(id),
            },
            name: head_name,
            deref: false,
        })?;
        Ok(())
    }

    pub(super) fn ls_remote(url: &str, branch: &str, auth: &GitAuth) -> Result<Option<String>> {
        let env = AuthEnv::build(auth)?;
        with_auth(auth, &env, || {
            // gix has no "no-repo" remote listing path in 0.81, so stand up a
            // throwaway bare repository and use its remote subsystem. The
            // TempDir is cleaned up automatically when this scope returns.
            let scratch = tempfile::Builder::new()
                .prefix("zlayer-git-lsremote-")
                .tempdir()
                .context("creating tempdir for ls-remote scratch repo")?;
            let repo = gix::init_bare(scratch.path()).with_context(|| {
                format!(
                    "initialising scratch bare repo at {}",
                    scratch.path().display()
                )
            })?;

            let mut remote = repo
                .remote_at(url.to_string())
                .with_context(|| format!("constructing remote for {url}"))?;
            let refspec = format!("+refs/heads/{branch}:refs/remotes/ls/{branch}");
            remote.replace_refspecs(Some(refspec.as_str()), gix::remote::Direction::Fetch)?;

            let mut connection = remote
                .connect(gix::remote::Direction::Fetch)
                .with_context(|| format!("connecting to {url}"))?;
            if let Some(cb) = credentials_for(auth) {
                connection.set_credentials(cb);
            }

            let (ref_map, _handshake) = connection
                .ref_map(Discard, gix::remote::ref_map::Options::default())
                .with_context(|| format!("listing refs from {url}"))?;

            let wanted_full = format!("refs/heads/{branch}");
            for r in &ref_map.remote_refs {
                match r {
                    gix::protocol::handshake::Ref::Direct {
                        full_ref_name,
                        object,
                    } if full_ref_name.as_bstr() == wanted_full.as_bytes().as_bstr() => {
                        return Ok(Some(object.to_string()));
                    }
                    gix::protocol::handshake::Ref::Peeled {
                        full_ref_name, tag, ..
                    } if full_ref_name.as_bstr() == wanted_full.as_bytes().as_bstr() => {
                        return Ok(Some(tag.to_string()));
                    }
                    gix::protocol::handshake::Ref::Symbolic {
                        full_ref_name,
                        object,
                        ..
                    } if full_ref_name.as_bstr() == wanted_full.as_bytes().as_bstr() => {
                        return Ok(Some(object.to_string()));
                    }
                    _ => {}
                }
            }
            Ok(None)
        })
    }

    /// Write the tree of `commit_id` into the repository's worktree, updating
    /// the on-disk index in the process. Used by both [`pull_ff`] and
    /// [`checkout`].
    ///
    /// This mimics the "2-way" checkout that `git checkout` performs: it
    /// writes all entries from the target tree, and it also deletes files
    /// from the worktree that existed in the previous index but are not
    /// present in the target tree. Without the second step, old files
    /// would linger and diverge from the checked-out revision.
    fn rebuild_worktree(repo: &gix::Repository, commit_id: gix::hash::ObjectId) -> Result<()> {
        let workdir = repo
            .workdir()
            .ok_or_else(|| anyhow!("repository at {} has no worktree", repo.path().display()))?
            .to_path_buf();

        // Resolve the commit's root tree id.
        let object = repo
            .find_object(commit_id)
            .with_context(|| format!("locating object {commit_id}"))?;
        let tree_id = object
            .peel_to_kind(gix::object::Kind::Tree)
            .with_context(|| format!("peeling {commit_id} to a tree"))?
            .id;

        // Gather the set of paths that were tracked before we switch. We
        // use these to delete worktree entries that are *not* present in
        // the new tree — otherwise a checkout backwards in history would
        // leave behind files added by later commits.
        let previous_paths: std::collections::BTreeSet<Vec<u8>> = match repo.try_index() {
            Ok(Some(index)) => index
                .entries()
                .iter()
                .map(|e| e.path(&index).to_vec())
                .collect(),
            Ok(None) | Err(_) => std::collections::BTreeSet::new(),
        };

        let mut index = repo
            .index_from_tree(&tree_id)
            .with_context(|| format!("materialising index from tree {tree_id}"))?;
        let new_paths: std::collections::BTreeSet<Vec<u8>> = index
            .entries()
            .iter()
            .map(|e| e.path(&index).to_vec())
            .collect();

        let mut opts = repo
            .checkout_options(gix::worktree::stack::state::attributes::Source::IdMapping)
            .context("building checkout options")?;
        opts.destination_is_initially_empty = false;
        opts.overwrite_existing = true;

        let files = Discard;
        let bytes = Discard;
        let odb = repo
            .objects
            .clone()
            .into_arc()
            .context("sharing object database across threads for checkout")?;
        gix::worktree::state::checkout(
            &mut index,
            &workdir,
            odb,
            &files,
            &bytes,
            &gix::interrupt::IS_INTERRUPTED,
            opts,
        )
        .context("writing worktree from tree")?;
        index
            .write(gix::index::write::Options::default())
            .context("writing updated index to disk")?;

        // Delete worktree files that existed previously but are absent from
        // the new tree, then prune newly-empty directories.
        for relative in previous_paths.difference(&new_paths) {
            // Silently skip non-UTF-8 paths rather than failing the whole
            // checkout; on Windows and Linux the worktree is UTF-8 in practice.
            let Ok(rel_path) = std::str::from_utf8(relative) else {
                continue;
            };
            let absolute = workdir.join(rel_path);
            match std::fs::remove_file(&absolute) {
                Ok(()) => tracing::trace!(
                    target: "zlayer_git",
                    path = %absolute.display(),
                    "removed stale worktree file",
                ),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => {
                    return Err(anyhow!(
                        "removing stale worktree file {}: {err}",
                        absolute.display()
                    ));
                }
            }
            // Attempt to clean up any directories that became empty as a
            // result. `remove_dir` fails loudly if non-empty, which we
            // silently tolerate.
            if let Some(mut parent) = absolute.parent() {
                while parent != workdir && parent.starts_with(&workdir) {
                    if std::fs::remove_dir(parent).is_err() {
                        break;
                    }
                    match parent.parent() {
                        Some(p) => parent = p,
                        None => break,
                    }
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command as StdCommand;
    use tempfile::TempDir;

    /// Create a bare repository and a working clone with an initial commit
    /// on `main`, then push. Returns `(bare_dir, first_clone_dir)`.
    fn init_remote_with_commit() -> (TempDir, TempDir) {
        let bare = tempfile::Builder::new()
            .prefix("zlayer-git-bare-")
            .tempdir()
            .expect("bare tempdir");
        let status = StdCommand::new("git")
            .args(["init", "--bare", "--initial-branch=main"])
            .arg(bare.path())
            .status()
            .expect("git init --bare");
        assert!(status.success(), "git init --bare failed");

        let work = tempfile::Builder::new()
            .prefix("zlayer-git-work-")
            .tempdir()
            .expect("work tempdir");
        run_sync(
            StdCommand::new("git")
                .args(["init", "--initial-branch=main"])
                .arg(work.path()),
            "git init",
        );
        configure_identity(work.path());
        std::fs::write(work.path().join("hello.txt"), "hello, world\n").unwrap();
        run_sync(
            StdCommand::new("git")
                .arg("-C")
                .arg(work.path())
                .args(["add", "hello.txt"]),
            "git add",
        );
        run_sync(
            StdCommand::new("git")
                .arg("-C")
                .arg(work.path())
                .args(["commit", "-m", "initial"]),
            "git commit",
        );
        run_sync(
            StdCommand::new("git")
                .arg("-C")
                .arg(work.path())
                .args(["remote", "add", "origin"])
                .arg(bare.path()),
            "git remote add",
        );
        run_sync(
            StdCommand::new("git")
                .arg("-C")
                .arg(work.path())
                .args(["push", "-u", "origin", "main"]),
            "git push",
        );

        (bare, work)
    }

    fn configure_identity(repo: &Path) {
        run_sync(
            StdCommand::new("git").arg("-C").arg(repo).args([
                "config",
                "user.email",
                "test@zlayer.dev",
            ]),
            "git config user.email",
        );
        run_sync(
            StdCommand::new("git")
                .arg("-C")
                .arg(repo)
                .args(["config", "user.name", "ZLayer Test"]),
            "git config user.name",
        );
        run_sync(
            StdCommand::new("git")
                .arg("-C")
                .arg(repo)
                .args(["config", "commit.gpgsign", "false"]),
            "git config commit.gpgsign",
        );
    }

    fn run_sync(cmd: &mut StdCommand, label: &str) {
        let output = cmd.output().expect(label);
        assert!(
            output.status.success(),
            "{label} failed: stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[tokio::test]
    async fn clone_pull_and_sha_roundtrip() {
        let (bare, first) = init_remote_with_commit();
        let bare_url = bare.path().to_string_lossy().into_owned();

        // --- Clone into a second workspace ------------------------------------
        let second_root = tempfile::Builder::new()
            .prefix("zlayer-git-second-")
            .tempdir()
            .unwrap();
        let second = second_root.path().join("repo");
        let clone_sha = clone_repo(&bare_url, "main", &GitAuth::anonymous(), &second)
            .await
            .expect("clone");
        assert_eq!(clone_sha.len(), 40, "expected SHA-1 hex: {clone_sha}");
        assert!(second.join("hello.txt").exists(), "cloned file is missing");
        // Defense-in-depth: if the library's `ensure_committer` fallback ever
        // regresses, a failing test here points at the library and not at a
        // missing fixture identity.
        configure_identity(&second);

        // `rev_parse` and `current_sha` must agree.
        let parsed = rev_parse(&second, "HEAD").await.expect("rev_parse HEAD");
        let head = current_sha(&second).await.expect("current_sha");
        assert_eq!(parsed, head);
        assert_eq!(parsed, clone_sha);

        // --- Add a new commit in the first clone and push --------------------
        configure_identity(first.path());
        std::fs::write(first.path().join("second.txt"), "second\n").unwrap();
        run_sync(
            StdCommand::new("git")
                .arg("-C")
                .arg(first.path())
                .args(["add", "second.txt"]),
            "git add",
        );
        run_sync(
            StdCommand::new("git")
                .arg("-C")
                .arg(first.path())
                .args(["commit", "-m", "second"]),
            "git commit",
        );
        run_sync(
            StdCommand::new("git")
                .arg("-C")
                .arg(first.path())
                .args(["push", "origin", "main"]),
            "git push",
        );

        // --- Fast-forward pull in the second clone ---------------------------
        let new_sha = pull_ff(&second, "main", &GitAuth::anonymous())
            .await
            .expect("pull_ff");
        assert_ne!(new_sha, clone_sha, "pull_ff did not advance HEAD");
        assert!(
            second.join("second.txt").exists(),
            "pull_ff did not bring in new file",
        );
        assert_eq!(new_sha, current_sha(&second).await.expect("current_sha"));
    }

    #[tokio::test]
    async fn checkout_by_sha_detaches_head() {
        let (bare, first) = init_remote_with_commit();
        let bare_url = bare.path().to_string_lossy().into_owned();

        // Make a second commit so we have two SHAs to flip between.
        configure_identity(first.path());
        let first_sha = {
            let out = StdCommand::new("git")
                .arg("-C")
                .arg(first.path())
                .args(["rev-parse", "HEAD"])
                .output()
                .unwrap();
            String::from_utf8(out.stdout).unwrap().trim().to_string()
        };
        std::fs::write(first.path().join("extra.txt"), "extra\n").unwrap();
        run_sync(
            StdCommand::new("git")
                .arg("-C")
                .arg(first.path())
                .args(["add", "extra.txt"]),
            "git add",
        );
        run_sync(
            StdCommand::new("git")
                .arg("-C")
                .arg(first.path())
                .args(["commit", "-m", "extra"]),
            "git commit",
        );
        run_sync(
            StdCommand::new("git")
                .arg("-C")
                .arg(first.path())
                .args(["push", "origin", "main"]),
            "git push",
        );

        let target = tempfile::Builder::new()
            .prefix("zlayer-git-checkout-")
            .tempdir()
            .unwrap();
        let repo = target.path().join("repo");
        let tip = clone_repo(&bare_url, "main", &GitAuth::anonymous(), &repo)
            .await
            .expect("clone");
        assert_ne!(tip, first_sha);
        // Defense-in-depth: regression here points at the library's
        // `ensure_committer` fallback rather than a missing fixture identity.
        configure_identity(&repo);

        checkout(&repo, &first_sha).await.expect("checkout sha");
        assert_eq!(
            current_sha(&repo)
                .await
                .expect("current_sha after checkout"),
            first_sha,
        );
        assert!(!repo.join("extra.txt").exists());
    }

    #[tokio::test]
    async fn ls_remote_against_local_bare() {
        let (bare, _first) = init_remote_with_commit();
        let bare_url = bare.path().to_string_lossy().into_owned();

        let head_sha = ls_remote(&bare_url, "main", &GitAuth::anonymous())
            .await
            .expect("ls_remote main")
            .expect("main branch must exist on the bare remote");
        assert_eq!(head_sha.len(), 40, "expected SHA-1 hex: {head_sha}");

        let missing = ls_remote(&bare_url, "does-not-exist", &GitAuth::anonymous())
            .await
            .expect("ls_remote missing");
        assert!(missing.is_none(), "expected None for missing branch");
    }

    #[tokio::test]
    async fn gix_native_clone_reads_gix_init_bare() {
        // Initialise a bare repository purely through gix (no CLI), plant a
        // commit in it via a sibling non-bare repo created by the system
        // `git`, then clone it back out with zlayer-git's gix-backed API.
        let bare = tempfile::Builder::new()
            .prefix("zlayer-git-native-bare-")
            .tempdir()
            .unwrap();
        gix::init_bare(bare.path()).expect("gix::init_bare");

        // Seed the bare repo with a commit via the system git CLI.
        let seed = tempfile::Builder::new()
            .prefix("zlayer-git-native-seed-")
            .tempdir()
            .unwrap();
        run_sync(
            StdCommand::new("git")
                .args(["init", "--initial-branch=main"])
                .arg(seed.path()),
            "git init seed",
        );
        configure_identity(seed.path());
        std::fs::write(seed.path().join("gix.txt"), "gix\n").unwrap();
        run_sync(
            StdCommand::new("git")
                .arg("-C")
                .arg(seed.path())
                .args(["add", "gix.txt"]),
            "git add",
        );
        run_sync(
            StdCommand::new("git")
                .arg("-C")
                .arg(seed.path())
                .args(["commit", "-m", "native"]),
            "git commit",
        );
        run_sync(
            StdCommand::new("git")
                .arg("-C")
                .arg(seed.path())
                .arg("remote")
                .arg("add")
                .arg("origin")
                .arg(bare.path()),
            "git remote add",
        );
        run_sync(
            StdCommand::new("git")
                .arg("-C")
                .arg(seed.path())
                .args(["push", "-u", "origin", "main"]),
            "git push",
        );

        // Now clone the gix-initialised bare repo through our gix-backed API.
        let dest_root = tempfile::Builder::new()
            .prefix("zlayer-git-native-dest-")
            .tempdir()
            .unwrap();
        let dest = dest_root.path().join("clone");
        let url = bare.path().to_string_lossy().into_owned();
        let sha = clone_repo(&url, "main", &GitAuth::anonymous(), &dest)
            .await
            .expect("gix-native clone");
        assert_eq!(sha.len(), 40);
        assert!(dest.join("gix.txt").exists());
    }
}
