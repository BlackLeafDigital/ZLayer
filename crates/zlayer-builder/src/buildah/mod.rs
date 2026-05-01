//! Buildah command generation and execution
//!
//! This module provides types and functions for converting Dockerfile instructions
//! into buildah commands and executing them.
//!
//! # Installation
//!
//! The [`BuildahInstaller`] type can be used to find existing buildah installations
//! or provide helpful installation instructions when buildah is not found.
//!
//! ```no_run
//! use zlayer_builder::buildah::{BuildahInstaller, BuildahExecutor};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! // Ensure buildah is available
//! let installer = BuildahInstaller::new();
//! let installation = installer.ensure().await?;
//!
//! // Create executor using found installation
//! let executor = BuildahExecutor::with_path(installation.path);
//! # Ok(())
//! # }
//! ```

mod executor;
mod install;

pub use executor::*;
pub use install::{
    current_platform, install_instructions, is_platform_supported, BuildahInstallation,
    BuildahInstaller, InstallError,
};

use crate::backend::ImageOs;
use crate::dockerfile::{
    AddInstruction, CopyInstruction, EnvInstruction, ExposeInstruction, HealthcheckInstruction,
    Instruction, RunInstruction, ShellOrExec,
};

use std::collections::HashMap;

/// Default shell used for `RUN <cmd>` (shell form) on Linux targets.
///
/// Matches the historical default used by Docker / buildah and keeps the
/// generated buildah command byte-identical to what we emitted before the
/// OS-aware translator landed.
const LINUX_DEFAULT_SHELL: &[&str] = &["/bin/sh", "-c"];

/// Default shell used for `RUN <cmd>` (shell form) on Windows targets.
///
/// Matches Docker's Windows default (`cmd /S /C`) used when no `SHELL`
/// instruction has overridden it.
const WINDOWS_DEFAULT_SHELL: &[&str] = &["cmd.exe", "/S", "/C"];

/// Return the default shell-form prefix for an OS when no `SHELL` instruction
/// has been seen.
fn default_shell_for(os: ImageOs) -> Vec<String> {
    let raw: &[&str] = match os {
        ImageOs::Linux => LINUX_DEFAULT_SHELL,
        ImageOs::Windows => WINDOWS_DEFAULT_SHELL,
    };
    raw.iter().map(|s| (*s).to_string()).collect()
}

/// A buildah command ready for execution
#[derive(Debug, Clone)]
pub struct BuildahCommand {
    /// The program to execute (typically "buildah")
    pub program: String,

    /// Command arguments
    pub args: Vec<String>,

    /// Optional environment variables for the command
    pub env: HashMap<String, String>,
}

impl BuildahCommand {
    /// Create a new buildah command
    #[must_use]
    pub fn new(subcommand: &str) -> Self {
        Self {
            program: "buildah".to_string(),
            args: vec![subcommand.to_string()],
            env: HashMap::new(),
        }
    }

    /// Add an argument
    #[must_use]
    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// Add multiple arguments
    #[must_use]
    pub fn args(mut self, args: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    /// Add an optional argument (only added if value is Some)
    #[must_use]
    pub fn arg_opt(self, flag: &str, value: Option<impl Into<String>>) -> Self {
        if let Some(v) = value {
            self.arg(flag).arg(v)
        } else {
            self
        }
    }

    /// Add an environment variable for command execution
    #[must_use]
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }

    /// Convert to a command line string for display/logging
    #[must_use]
    pub fn to_command_string(&self) -> String {
        let mut parts = vec![self.program.clone()];
        parts.extend(self.args.iter().map(|a| {
            if a.contains(' ') || a.contains('"') {
                format!("\"{}\"", a.replace('"', "\\\""))
            } else {
                a.clone()
            }
        }));
        parts.join(" ")
    }

    // =========================================================================
    // Container Lifecycle Commands
    // =========================================================================

    /// Create a new working container from an image
    ///
    /// `buildah from <image>`
    #[must_use]
    pub fn from_image(image: &str) -> Self {
        Self::new("from").arg(image)
    }

    /// Create a new working container from an image with a specific name
    ///
    /// `buildah from --name <name> <image>`
    #[must_use]
    pub fn from_image_named(image: &str, name: &str) -> Self {
        Self::new("from").arg("--name").arg(name).arg(image)
    }

    /// Create a scratch container
    ///
    /// `buildah from scratch`
    #[must_use]
    pub fn from_scratch() -> Self {
        Self::new("from").arg("scratch")
    }

    /// Remove a working container
    ///
    /// `buildah rm <container>`
    #[must_use]
    pub fn rm(container: &str) -> Self {
        Self::new("rm").arg(container)
    }

    /// Commit a container to create an image
    ///
    /// `buildah commit <container> <image>`
    #[must_use]
    pub fn commit(container: &str, image_name: &str) -> Self {
        Self::new("commit").arg(container).arg(image_name)
    }

    /// Commit with additional options
    #[must_use]
    pub fn commit_with_opts(
        container: &str,
        image_name: &str,
        format: Option<&str>,
        squash: bool,
    ) -> Self {
        let mut cmd = Self::new("commit");

        if let Some(fmt) = format {
            cmd = cmd.arg("--format").arg(fmt);
        }

        if squash {
            cmd = cmd.arg("--squash");
        }

        cmd.arg(container).arg(image_name)
    }

    /// Tag an image with a new name
    ///
    /// `buildah tag <image> <new-name>`
    #[must_use]
    pub fn tag(image: &str, new_name: &str) -> Self {
        Self::new("tag").arg(image).arg(new_name)
    }

    /// Remove an image
    ///
    /// `buildah rmi <image>`
    #[must_use]
    pub fn rmi(image: &str) -> Self {
        Self::new("rmi").arg(image)
    }

    /// Pull an image from a registry into local storage.
    ///
    /// `buildah pull [--policy <policy>] <image>`
    ///
    /// `policy` controls when the registry is consulted: `"newer"` only pulls
    /// when the upstream is newer than the local copy, `"always"` forces a
    /// fresh pull, and `"never"` (or `"missing"`) is useful for offline builds.
    /// When `policy` is `None`, buildah's default policy applies.
    #[must_use]
    pub fn pull(image: &str, policy: Option<&str>) -> Self {
        let mut cmd = Self::new("pull");
        if let Some(p) = policy {
            cmd = cmd.arg("--policy").arg(p);
        }
        cmd.arg(image)
    }

    /// Push an image to a registry
    ///
    /// `buildah push <image>`
    #[must_use]
    pub fn push(image: &str) -> Self {
        Self::new("push").arg(image)
    }

    /// Push an image to a registry with options
    ///
    /// `buildah push [options] <image> [destination]`
    #[must_use]
    pub fn push_to(image: &str, destination: &str) -> Self {
        Self::new("push").arg(image).arg(destination)
    }

    /// Inspect an image or container
    ///
    /// `buildah inspect <name>`
    #[must_use]
    pub fn inspect(name: &str) -> Self {
        Self::new("inspect").arg(name)
    }

    /// Inspect an image or container with format
    ///
    /// `buildah inspect --format <format> <name>`
    #[must_use]
    pub fn inspect_format(name: &str, format: &str) -> Self {
        Self::new("inspect").arg("--format").arg(format).arg(name)
    }

    /// List images
    ///
    /// `buildah images`
    #[must_use]
    pub fn images() -> Self {
        Self::new("images")
    }

    /// List containers
    ///
    /// `buildah containers`
    #[must_use]
    pub fn containers() -> Self {
        Self::new("containers")
    }

    // =========================================================================
    // Run Commands
    // =========================================================================

    /// Run a command in the container (shell form) using the Linux default shell.
    ///
    /// `buildah run <container> -- /bin/sh -c "<command>"`
    ///
    /// For OS-aware translation (Windows targets, or honoring a `SHELL`
    /// override), prefer [`Self::run_shell_custom`] or the stateful
    /// [`DockerfileTranslator`].
    #[must_use]
    pub fn run_shell(container: &str, command: &str) -> Self {
        Self::run_shell_custom(container, LINUX_DEFAULT_SHELL, command)
    }

    /// Run a command in the container (shell form) using an explicit shell.
    ///
    /// `buildah run <container> -- <shell...> <command>`
    ///
    /// The `shell` slice is emitted verbatim before the command argument, e.g.
    /// `["cmd.exe", "/S", "/C"]` for Windows or `["/bin/bash", "-lc"]` for a
    /// bash-login override.
    #[must_use]
    pub fn run_shell_custom(
        container: &str,
        shell: impl IntoIterator<Item = impl AsRef<str>>,
        command: &str,
    ) -> Self {
        let mut cmd = Self::new("run").arg(container).arg("--");
        for s in shell {
            cmd = cmd.arg(s.as_ref().to_string());
        }
        cmd.arg(command)
    }

    /// Run a command in the container (shell form) using the OS default shell.
    ///
    /// Linux → `/bin/sh -c`, Windows → `cmd.exe /S /C`.
    #[must_use]
    pub fn run_shell_for_os(container: &str, command: &str, os: ImageOs) -> Self {
        let shell = default_shell_for(os);
        Self::run_shell_custom(container, &shell, command)
    }

    /// Run a command in the container (exec form)
    ///
    /// `buildah run <container> -- <args...>`
    #[must_use]
    pub fn run_exec(container: &str, args: &[String]) -> Self {
        let mut cmd = Self::new("run").arg(container).arg("--");
        for arg in args {
            cmd = cmd.arg(arg);
        }
        cmd
    }

    /// Run a command based on `ShellOrExec`
    #[must_use]
    pub fn run(container: &str, command: &ShellOrExec) -> Self {
        match command {
            ShellOrExec::Shell(s) => Self::run_shell(container, s),
            ShellOrExec::Exec(args) => Self::run_exec(container, args),
        }
    }

    /// Run a command with mount specifications from a `RunInstruction`.
    ///
    /// Buildah requires `--mount` arguments to appear BEFORE the container ID:
    /// `buildah run [--mount=...] <container> -- <command>`
    ///
    /// This method properly orders the arguments to ensure mounts are applied.
    ///
    /// Uses the Linux default shell (`/bin/sh -c`) for shell-form commands.
    /// For OS-aware translation use [`Self::run_with_mounts_shell`].
    #[must_use]
    pub fn run_with_mounts(container: &str, run: &RunInstruction) -> Self {
        Self::run_with_mounts_shell(container, run, LINUX_DEFAULT_SHELL)
    }

    /// Run a command with mount specifications, using an explicit shell for
    /// shell-form commands.
    ///
    /// Exec-form commands ignore `shell` and are emitted verbatim, matching
    /// Docker/Buildah semantics.
    #[must_use]
    pub fn run_with_mounts_shell(
        container: &str,
        run: &RunInstruction,
        shell: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> Self {
        let mut cmd = Self::new("run");

        // Add --mount arguments BEFORE the container ID
        for mount in &run.mounts {
            cmd = cmd.arg(format!("--mount={}", mount.to_buildah_arg()));
        }

        // Now add container and the command
        cmd = cmd.arg(container).arg("--");

        match &run.command {
            ShellOrExec::Shell(s) => {
                for part in shell {
                    cmd = cmd.arg(part.as_ref().to_string());
                }
                cmd.arg(s)
            }
            ShellOrExec::Exec(args) => {
                for arg in args {
                    cmd = cmd.arg(arg);
                }
                cmd
            }
        }
    }

    // =========================================================================
    // Copy/Add Commands
    // =========================================================================

    /// Copy files into the container
    ///
    /// `buildah copy <container> <src...> <dest>`
    #[must_use]
    pub fn copy(container: &str, sources: &[String], dest: &str) -> Self {
        let mut cmd = Self::new("copy").arg(container);
        for src in sources {
            cmd = cmd.arg(src);
        }
        cmd.arg(dest)
    }

    /// Copy files from another container/image
    ///
    /// `buildah copy --from=<source> <container> <src...> <dest>`
    #[must_use]
    pub fn copy_from(container: &str, from: &str, sources: &[String], dest: &str) -> Self {
        let mut cmd = Self::new("copy").arg("--from").arg(from).arg(container);
        for src in sources {
            cmd = cmd.arg(src);
        }
        cmd.arg(dest)
    }

    /// Copy with all options from `CopyInstruction`
    #[must_use]
    pub fn copy_instruction(container: &str, copy: &CopyInstruction) -> Self {
        let mut cmd = Self::new("copy");

        if let Some(ref from) = copy.from {
            cmd = cmd.arg("--from").arg(from);
        }

        if let Some(ref chown) = copy.chown {
            cmd = cmd.arg("--chown").arg(chown);
        }

        if let Some(ref chmod) = copy.chmod {
            cmd = cmd.arg("--chmod").arg(chmod);
        }

        cmd = cmd.arg(container);

        for src in &copy.sources {
            cmd = cmd.arg(src);
        }

        cmd.arg(&copy.destination)
    }

    /// Add files (like copy but with URL support and extraction)
    #[must_use]
    pub fn add(container: &str, sources: &[String], dest: &str) -> Self {
        let mut cmd = Self::new("add").arg(container);
        for src in sources {
            cmd = cmd.arg(src);
        }
        cmd.arg(dest)
    }

    /// Add with all options from `AddInstruction`
    #[must_use]
    pub fn add_instruction(container: &str, add: &AddInstruction) -> Self {
        let mut cmd = Self::new("add");

        if let Some(ref chown) = add.chown {
            cmd = cmd.arg("--chown").arg(chown);
        }

        if let Some(ref chmod) = add.chmod {
            cmd = cmd.arg("--chmod").arg(chmod);
        }

        cmd = cmd.arg(container);

        for src in &add.sources {
            cmd = cmd.arg(src);
        }

        cmd.arg(&add.destination)
    }

    // =========================================================================
    // Config Commands
    // =========================================================================

    /// Set an environment variable
    ///
    /// `buildah config --env KEY=VALUE <container>`
    #[must_use]
    pub fn config_env(container: &str, key: &str, value: &str) -> Self {
        Self::new("config")
            .arg("--env")
            .arg(format!("{key}={value}"))
            .arg(container)
    }

    /// Set multiple environment variables
    #[must_use]
    pub fn config_envs(container: &str, env: &EnvInstruction) -> Vec<Self> {
        env.vars
            .iter()
            .map(|(k, v)| Self::config_env(container, k, v))
            .collect()
    }

    /// Set the working directory
    ///
    /// `buildah config --workingdir <dir> <container>`
    #[must_use]
    pub fn config_workdir(container: &str, dir: &str) -> Self {
        Self::new("config")
            .arg("--workingdir")
            .arg(dir)
            .arg(container)
    }

    /// Expose a port
    ///
    /// `buildah config --port <port>/<proto> <container>`
    #[must_use]
    pub fn config_expose(container: &str, expose: &ExposeInstruction) -> Self {
        let port_spec = format!(
            "{}/{}",
            expose.port,
            match expose.protocol {
                crate::dockerfile::ExposeProtocol::Tcp => "tcp",
                crate::dockerfile::ExposeProtocol::Udp => "udp",
            }
        );
        Self::new("config")
            .arg("--port")
            .arg(port_spec)
            .arg(container)
    }

    /// Set the entrypoint (shell form)
    ///
    /// `buildah config --entrypoint '<command>' <container>`
    #[must_use]
    pub fn config_entrypoint_shell(container: &str, command: &str) -> Self {
        Self::new("config")
            .arg("--entrypoint")
            .arg(format!(
                "[\"/bin/sh\", \"-c\", \"{}\"]",
                escape_json_string(command)
            ))
            .arg(container)
    }

    /// Set the entrypoint (exec form)
    ///
    /// `buildah config --entrypoint '["exe", "arg1"]' <container>`
    #[must_use]
    pub fn config_entrypoint_exec(container: &str, args: &[String]) -> Self {
        let json_array = format!(
            "[{}]",
            args.iter()
                .map(|a| format!("\"{}\"", escape_json_string(a)))
                .collect::<Vec<_>>()
                .join(", ")
        );
        Self::new("config")
            .arg("--entrypoint")
            .arg(json_array)
            .arg(container)
    }

    /// Set the entrypoint based on `ShellOrExec`
    #[must_use]
    pub fn config_entrypoint(container: &str, command: &ShellOrExec) -> Self {
        match command {
            ShellOrExec::Shell(s) => Self::config_entrypoint_shell(container, s),
            ShellOrExec::Exec(args) => Self::config_entrypoint_exec(container, args),
        }
    }

    /// Set the default command (shell form)
    #[must_use]
    pub fn config_cmd_shell(container: &str, command: &str) -> Self {
        Self::new("config")
            .arg("--cmd")
            .arg(format!("/bin/sh -c \"{}\"", escape_json_string(command)))
            .arg(container)
    }

    /// Set the default command (exec form)
    #[must_use]
    pub fn config_cmd_exec(container: &str, args: &[String]) -> Self {
        let json_array = format!(
            "[{}]",
            args.iter()
                .map(|a| format!("\"{}\"", escape_json_string(a)))
                .collect::<Vec<_>>()
                .join(", ")
        );
        Self::new("config")
            .arg("--cmd")
            .arg(json_array)
            .arg(container)
    }

    /// Set the default command based on `ShellOrExec`
    #[must_use]
    pub fn config_cmd(container: &str, command: &ShellOrExec) -> Self {
        match command {
            ShellOrExec::Shell(s) => Self::config_cmd_shell(container, s),
            ShellOrExec::Exec(args) => Self::config_cmd_exec(container, args),
        }
    }

    /// Set the user
    ///
    /// `buildah config --user <user> <container>`
    #[must_use]
    pub fn config_user(container: &str, user: &str) -> Self {
        Self::new("config").arg("--user").arg(user).arg(container)
    }

    /// Set a label
    ///
    /// `buildah config --label KEY=VALUE <container>`
    #[must_use]
    pub fn config_label(container: &str, key: &str, value: &str) -> Self {
        Self::new("config")
            .arg("--label")
            .arg(format!("{key}={value}"))
            .arg(container)
    }

    /// Set multiple labels
    #[must_use]
    pub fn config_labels(container: &str, labels: &HashMap<String, String>) -> Vec<Self> {
        labels
            .iter()
            .map(|(k, v)| Self::config_label(container, k, v))
            .collect()
    }

    /// Set volumes
    ///
    /// `buildah config --volume <path> <container>`
    #[must_use]
    pub fn config_volume(container: &str, path: &str) -> Self {
        Self::new("config").arg("--volume").arg(path).arg(container)
    }

    /// Set the stop signal
    ///
    /// `buildah config --stop-signal <signal> <container>`
    #[must_use]
    pub fn config_stopsignal(container: &str, signal: &str) -> Self {
        Self::new("config")
            .arg("--stop-signal")
            .arg(signal)
            .arg(container)
    }

    /// Set the shell
    ///
    /// `buildah config --shell '["shell", "args"]' <container>`
    #[must_use]
    pub fn config_shell(container: &str, shell: &[String]) -> Self {
        let json_array = format!(
            "[{}]",
            shell
                .iter()
                .map(|a| format!("\"{}\"", escape_json_string(a)))
                .collect::<Vec<_>>()
                .join(", ")
        );
        Self::new("config")
            .arg("--shell")
            .arg(json_array)
            .arg(container)
    }

    /// Set healthcheck
    #[must_use]
    pub fn config_healthcheck(container: &str, healthcheck: &HealthcheckInstruction) -> Self {
        match healthcheck {
            HealthcheckInstruction::None => Self::new("config")
                .arg("--healthcheck")
                .arg("NONE")
                .arg(container),
            HealthcheckInstruction::Check {
                command,
                interval,
                timeout,
                start_period,
                retries,
                ..
            } => {
                let mut cmd = Self::new("config");

                let cmd_str = match command {
                    ShellOrExec::Shell(s) => format!("CMD {s}"),
                    ShellOrExec::Exec(args) => {
                        format!(
                            "CMD [{}]",
                            args.iter()
                                .map(|a| format!("\"{}\"", escape_json_string(a)))
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    }
                };

                cmd = cmd.arg("--healthcheck").arg(cmd_str);

                if let Some(i) = interval {
                    cmd = cmd
                        .arg("--healthcheck-interval")
                        .arg(format!("{}s", i.as_secs()));
                }

                if let Some(t) = timeout {
                    cmd = cmd
                        .arg("--healthcheck-timeout")
                        .arg(format!("{}s", t.as_secs()));
                }

                if let Some(sp) = start_period {
                    cmd = cmd
                        .arg("--healthcheck-start-period")
                        .arg(format!("{}s", sp.as_secs()));
                }

                if let Some(r) = retries {
                    cmd = cmd.arg("--healthcheck-retries").arg(r.to_string());
                }

                cmd.arg(container)
            }
        }
    }

    // =========================================================================
    // Manifest Commands
    // =========================================================================

    /// Create a new manifest list.
    ///
    /// `buildah manifest create <name>`
    #[must_use]
    pub fn manifest_create(name: &str) -> Self {
        Self::new("manifest").arg("create").arg(name)
    }

    /// Add an image to a manifest list.
    ///
    /// `buildah manifest add <list> <image>`
    #[must_use]
    pub fn manifest_add(list: &str, image: &str) -> Self {
        Self::new("manifest").arg("add").arg(list).arg(image)
    }

    /// Push a manifest list and all referenced images.
    ///
    /// `buildah manifest push --all <list> <destination>`
    #[must_use]
    pub fn manifest_push(list: &str, destination: &str) -> Self {
        Self::new("manifest")
            .arg("push")
            .arg("--all")
            .arg(list)
            .arg(destination)
    }

    /// Remove a manifest list.
    ///
    /// `buildah manifest rm <list>`
    #[must_use]
    pub fn manifest_rm(list: &str) -> Self {
        Self::new("manifest").arg("rm").arg(list)
    }

    // =========================================================================
    // Convert Instruction to Commands
    // =========================================================================

    /// Convert a Dockerfile instruction to buildah command(s) using the Linux
    /// default shell (`/bin/sh -c`) and POSIX `mkdir -p` semantics.
    ///
    /// This is a convenience wrapper around [`DockerfileTranslator`] with
    /// `target_os = ImageOs::Linux` and no `SHELL` override. It preserves the
    /// historical byte-for-byte behavior for every call site that existed
    /// before OS-aware translation landed in Phase L-3.
    ///
    /// For Windows targets or when a Dockerfile-level `SHELL` instruction
    /// needs to be honored across subsequent `RUN`s, construct a
    /// [`DockerfileTranslator`] explicitly and call
    /// [`DockerfileTranslator::translate`].
    ///
    /// Some instructions map to multiple buildah commands (e.g., multiple
    /// ENV vars, or WORKDIR emitting both `mkdir` and `config --workingdir`).
    #[must_use]
    pub fn from_instruction(container: &str, instruction: &Instruction) -> Vec<Self> {
        DockerfileTranslator::new(ImageOs::Linux).translate(container, instruction)
    }
}

/// Stateful translator from [`Instruction`] to [`BuildahCommand`] sequences.
///
/// Tracks the target OS and the most recent `SHELL` instruction so that
/// shell-form `RUN` / `CMD` / `ENTRYPOINT` use the correct shell for the
/// target platform:
///
/// - **Linux, no SHELL override** — `RUN cmd` → `buildah run -- /bin/sh -c "cmd"`
/// - **Windows, no SHELL override** — `RUN cmd` → `buildah run -- cmd.exe /S /C "cmd"`
/// - **Any OS with `SHELL ["pwsh", "-Command"]`** — subsequent `RUN cmd` uses
///   `buildah run -- pwsh -Command "cmd"`
///
/// The translator is stateful because Dockerfile `SHELL` instructions persist
/// across subsequent `RUN`/`CMD`/`ENTRYPOINT` translations until another
/// `SHELL` replaces them. Callers that translate a multi-instruction stage
/// should reuse a single translator instance across the full instruction
/// stream.
///
/// This translator is designed to be shared between the buildah backend and
/// the Phase L-4 HCS (Windows host compute service) backend, so neither needs
/// to re-implement the shell-form / workdir branching.
#[derive(Debug, Clone)]
pub struct DockerfileTranslator {
    target_os: ImageOs,
    /// Most recent `SHELL` instruction override, if any. When `None` the
    /// translator falls back to [`default_shell_for`] for the target OS.
    shell_override: Option<Vec<String>>,
}

impl DockerfileTranslator {
    /// Create a new translator for a given target OS, with no `SHELL` override.
    #[must_use]
    pub fn new(target_os: ImageOs) -> Self {
        Self {
            target_os,
            shell_override: None,
        }
    }

    /// Return the target OS this translator emits commands for.
    #[must_use]
    pub fn target_os(&self) -> ImageOs {
        self.target_os
    }

    /// Return the current shell-form prefix: the `SHELL` override if one was
    /// applied, else the OS default (`/bin/sh -c` on Linux, `cmd.exe /S /C` on
    /// Windows).
    #[must_use]
    pub fn active_shell(&self) -> Vec<String> {
        match &self.shell_override {
            Some(s) => s.clone(),
            None => default_shell_for(self.target_os),
        }
    }

    /// Replace the translator's `SHELL` override, matching the effect of a
    /// Dockerfile `SHELL ["…"]` instruction on subsequent RUN/CMD/ENTRYPOINT
    /// shell-form commands.
    pub fn set_shell_override(&mut self, shell: Vec<String>) {
        self.shell_override = Some(shell);
    }

    /// Translate a single instruction into zero or more [`BuildahCommand`]s.
    ///
    /// Stateful: `SHELL` instructions update the translator's shell override,
    /// so subsequent `RUN` / `CMD` / `ENTRYPOINT` shell-form translations pick
    /// up the new shell. `WORKDIR` emits an OS-appropriate pre-mkdir followed
    /// by `buildah config --workingdir`.
    #[allow(clippy::too_many_lines)]
    pub fn translate(&mut self, container: &str, instruction: &Instruction) -> Vec<BuildahCommand> {
        match instruction {
            Instruction::Run(run) => {
                let shell = self.active_shell();
                if run.mounts.is_empty() {
                    match &run.command {
                        ShellOrExec::Shell(s) => {
                            vec![BuildahCommand::run_shell_custom(container, &shell, s)]
                        }
                        ShellOrExec::Exec(args) => vec![BuildahCommand::run_exec(container, args)],
                    }
                } else {
                    vec![BuildahCommand::run_with_mounts_shell(
                        container, run, &shell,
                    )]
                }
            }

            Instruction::Copy(copy) => {
                vec![BuildahCommand::copy_instruction(container, copy)]
            }

            Instruction::Add(add) => {
                vec![BuildahCommand::add_instruction(container, add)]
            }

            Instruction::Env(env) => BuildahCommand::config_envs(container, env),

            Instruction::Workdir(dir) => self.translate_workdir(container, dir),

            Instruction::Expose(expose) => {
                vec![BuildahCommand::config_expose(container, expose)]
            }

            Instruction::Label(labels) => BuildahCommand::config_labels(container, labels),

            Instruction::User(user) => {
                vec![BuildahCommand::config_user(container, user)]
            }

            Instruction::Entrypoint(cmd) => {
                vec![BuildahCommand::config_entrypoint(container, cmd)]
            }

            Instruction::Cmd(cmd) => {
                vec![BuildahCommand::config_cmd(container, cmd)]
            }

            Instruction::Volume(paths) => paths
                .iter()
                .map(|p| BuildahCommand::config_volume(container, p))
                .collect(),

            Instruction::Shell(shell) => {
                // SHELL instruction: update the translator's shell override
                // AND emit the metadata config --shell so committed images
                // record the user-declared shell. Both matter: the override
                // changes how we translate subsequent RUN/CMD/ENTRYPOINT
                // shell-form instructions in THIS build; the metadata is what
                // tools like `docker inspect` show on the resulting image.
                self.set_shell_override(shell.clone());
                vec![BuildahCommand::config_shell(container, shell)]
            }

            Instruction::Arg(_) => {
                // ARG is handled during variable expansion, not as a buildah command
                vec![]
            }

            Instruction::Stopsignal(signal) => {
                vec![BuildahCommand::config_stopsignal(container, signal)]
            }

            Instruction::Healthcheck(hc) => {
                vec![BuildahCommand::config_healthcheck(container, hc)]
            }

            Instruction::Onbuild(_) => {
                // ONBUILD would need special handling
                tracing::warn!("ONBUILD instruction not supported in buildah conversion");
                vec![]
            }
        }
    }

    /// Emit the commands needed to realise a `WORKDIR <dir>` instruction for
    /// the target OS.
    ///
    /// # Linux
    ///
    /// Emits `mkdir -p <dir>` followed by `buildah config --workingdir`. This
    /// matches Docker's WORKDIR semantics: the directory must exist in the
    /// rootfs before a process can `chdir()` into it, and `buildah config
    /// --workingdir` alone is metadata-only.
    ///
    /// # Windows
    ///
    /// Emits `cmd /S /C "if not exist <dir> mkdir <dir>"` before the
    /// metadata write. Windows `mkdir` (unlike `mkdir -p`) errors out when the
    /// directory exists, so we guard with `if not exist` to stay idempotent
    /// across repeated WORKDIR instructions in the same Dockerfile.
    fn translate_workdir(&self, container: &str, dir: &str) -> Vec<BuildahCommand> {
        match self.target_os {
            ImageOs::Linux => {
                vec![
                    BuildahCommand::run_exec(
                        container,
                        &["mkdir".to_string(), "-p".to_string(), dir.to_string()],
                    ),
                    BuildahCommand::config_workdir(container, dir),
                ]
            }
            ImageOs::Windows => {
                // Quote the path so paths with spaces (e.g. `C:\Program Files\app`)
                // survive cmd.exe parsing. `cmd /S /C` strips the outer quotes
                // before executing, so double-quote the full command and escape
                // any inner quotes.
                let guarded = format!(r#"if not exist "{dir}" mkdir "{dir}""#);
                vec![
                    BuildahCommand::run_shell_custom(container, WINDOWS_DEFAULT_SHELL, &guarded),
                    BuildahCommand::config_workdir(container, dir),
                ]
            }
        }
    }
}

/// Escape a string for use in JSON
fn escape_json_string(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dockerfile::RunInstruction;

    #[test]
    fn test_from_image() {
        let cmd = BuildahCommand::from_image("alpine:3.18");
        assert_eq!(cmd.program, "buildah");
        assert_eq!(cmd.args, vec!["from", "alpine:3.18"]);
    }

    #[test]
    fn test_pull_no_policy() {
        let cmd = BuildahCommand::pull("ghcr.io/astral-sh/uv:0.5.0", None);
        assert_eq!(cmd.program, "buildah");
        assert_eq!(cmd.args, vec!["pull", "ghcr.io/astral-sh/uv:0.5.0"]);
    }

    #[test]
    fn test_pull_with_policy() {
        let cmd = BuildahCommand::pull("ghcr.io/astral-sh/uv:0.5.0", Some("newer"));
        assert_eq!(
            cmd.args,
            vec!["pull", "--policy", "newer", "ghcr.io/astral-sh/uv:0.5.0"]
        );
    }

    #[test]
    fn test_run_shell() {
        let cmd = BuildahCommand::run_shell("container-1", "apt-get update");
        assert_eq!(
            cmd.args,
            vec![
                "run",
                "container-1",
                "--",
                "/bin/sh",
                "-c",
                "apt-get update"
            ]
        );
    }

    #[test]
    fn test_run_exec() {
        let args = vec!["echo".to_string(), "hello".to_string()];
        let cmd = BuildahCommand::run_exec("container-1", &args);
        assert_eq!(cmd.args, vec!["run", "container-1", "--", "echo", "hello"]);
    }

    #[test]
    fn test_copy() {
        let sources = vec!["src/".to_string(), "Cargo.toml".to_string()];
        let cmd = BuildahCommand::copy("container-1", &sources, "/app/");
        assert_eq!(
            cmd.args,
            vec!["copy", "container-1", "src/", "Cargo.toml", "/app/"]
        );
    }

    #[test]
    fn test_copy_from() {
        let sources = vec!["/app".to_string()];
        let cmd = BuildahCommand::copy_from("container-1", "builder", &sources, "/app");
        assert_eq!(
            cmd.args,
            vec!["copy", "--from", "builder", "container-1", "/app", "/app"]
        );
    }

    #[test]
    fn test_copy_from_external_image_reference_is_preserved() {
        // `COPY --from=ghcr.io/astral-sh/uv:0.5.0 /uv /usr/local/bin/uv`
        // — when the buildah backend hands an external image reference to
        // the translator, the registry-qualified ref must reach buildah
        // verbatim. Buildah resolves it against the local image store; the
        // backend is responsible for pulling the image first.
        use crate::dockerfile::CopyInstruction;

        let copy = CopyInstruction {
            sources: vec!["/uv".to_string()],
            destination: "/usr/local/bin/uv".to_string(),
            from: Some("ghcr.io/astral-sh/uv:0.5.0".to_string()),
            chown: None,
            chmod: None,
            link: false,
            exclude: Vec::new(),
        };
        let instruction = Instruction::Copy(copy);
        let cmds = BuildahCommand::from_instruction("container-1", &instruction);

        assert_eq!(
            cmds.len(),
            1,
            "COPY translates to a single buildah copy command"
        );
        assert_eq!(
            cmds[0].args,
            vec![
                "copy",
                "--from",
                "ghcr.io/astral-sh/uv:0.5.0",
                "container-1",
                "/uv",
                "/usr/local/bin/uv",
            ],
            "external image reference must be passed through to buildah unchanged",
        );
    }

    #[test]
    fn test_config_env() {
        let cmd = BuildahCommand::config_env("container-1", "PATH", "/usr/local/bin");
        assert_eq!(
            cmd.args,
            vec!["config", "--env", "PATH=/usr/local/bin", "container-1"]
        );
    }

    #[test]
    fn test_config_workdir() {
        let cmd = BuildahCommand::config_workdir("container-1", "/app");
        assert_eq!(
            cmd.args,
            vec!["config", "--workingdir", "/app", "container-1"]
        );
    }

    #[test]
    fn test_config_entrypoint_exec() {
        let args = vec!["/app".to_string(), "--config".to_string()];
        let cmd = BuildahCommand::config_entrypoint_exec("container-1", &args);
        assert!(cmd.args.contains(&"--entrypoint".to_string()));
        assert!(cmd
            .args
            .iter()
            .any(|a| a.contains('[') && a.contains("/app")));
    }

    #[test]
    fn test_commit() {
        let cmd = BuildahCommand::commit("container-1", "myimage:latest");
        assert_eq!(cmd.args, vec!["commit", "container-1", "myimage:latest"]);
    }

    #[test]
    fn test_to_command_string() {
        let cmd = BuildahCommand::config_env("container-1", "VAR", "value with spaces");
        let s = cmd.to_command_string();
        assert!(s.starts_with("buildah config"));
        assert!(s.contains("VAR=value with spaces"));
    }

    #[test]
    fn test_from_instruction_run() {
        let instruction = Instruction::Run(RunInstruction {
            command: ShellOrExec::Shell("echo hello".to_string()),
            mounts: vec![],
            network: None,
            security: None,
        });

        let cmds = BuildahCommand::from_instruction("container-1", &instruction);
        assert_eq!(cmds.len(), 1);
        assert!(cmds[0].args.contains(&"run".to_string()));
    }

    #[test]
    fn test_from_instruction_workdir_creates_and_configures() {
        // WORKDIR must both create the dir in the rootfs (like Docker) AND
        // update image metadata. Emitting only `config --workingdir` leaves
        // containers chdir-ing to a missing directory at init time.
        let instruction = Instruction::Workdir("/workspace".to_string());
        let cmds = BuildahCommand::from_instruction("container-1", &instruction);

        assert_eq!(cmds.len(), 2, "WORKDIR should emit mkdir + config");

        let run_args = &cmds[0].args;
        assert_eq!(run_args[0], "run");
        assert_eq!(run_args[1], "container-1");
        assert_eq!(run_args[2], "--");
        assert_eq!(run_args[3], "mkdir");
        assert_eq!(run_args[4], "-p");
        assert_eq!(run_args[5], "/workspace");

        assert_eq!(
            cmds[1].args,
            vec!["config", "--workingdir", "/workspace", "container-1"]
        );
    }

    #[test]
    fn test_from_instruction_env_multiple() {
        let mut vars = HashMap::new();
        vars.insert("FOO".to_string(), "bar".to_string());
        vars.insert("BAZ".to_string(), "qux".to_string());

        let instruction = Instruction::Env(EnvInstruction { vars });
        let cmds = BuildahCommand::from_instruction("container-1", &instruction);

        // Should produce two config commands (one per env var)
        assert_eq!(cmds.len(), 2);
        for cmd in &cmds {
            assert!(cmd.args.contains(&"config".to_string()));
            assert!(cmd.args.contains(&"--env".to_string()));
        }
    }

    #[test]
    fn test_escape_json_string() {
        assert_eq!(escape_json_string("hello"), "hello");
        assert_eq!(escape_json_string("hello \"world\""), "hello \\\"world\\\"");
        assert_eq!(escape_json_string("line1\nline2"), "line1\\nline2");
    }

    #[test]
    fn test_run_with_mounts_cache() {
        use crate::dockerfile::{CacheSharing, RunMount};

        let run = RunInstruction {
            command: ShellOrExec::Shell("apt-get update".to_string()),
            mounts: vec![RunMount::Cache {
                target: "/var/cache/apt".to_string(),
                id: Some("apt-cache".to_string()),
                sharing: CacheSharing::Shared,
                readonly: false,
            }],
            network: None,
            security: None,
        };

        let cmd = BuildahCommand::run_with_mounts("container-1", &run);

        // Verify --mount comes BEFORE container ID
        let mount_idx = cmd
            .args
            .iter()
            .position(|a| a.starts_with("--mount="))
            .expect("should have --mount arg");
        let container_idx = cmd
            .args
            .iter()
            .position(|a| a == "container-1")
            .expect("should have container id");

        assert!(
            mount_idx < container_idx,
            "--mount should come before container ID"
        );

        // Verify mount argument content
        assert!(cmd.args[mount_idx].contains("type=cache"));
        assert!(cmd.args[mount_idx].contains("target=/var/cache/apt"));
        assert!(cmd.args[mount_idx].contains("id=apt-cache"));
        assert!(cmd.args[mount_idx].contains("sharing=shared"));
    }

    #[test]
    fn test_run_with_multiple_mounts() {
        use crate::dockerfile::{CacheSharing, RunMount};

        let run = RunInstruction {
            command: ShellOrExec::Shell("cargo build".to_string()),
            mounts: vec![
                RunMount::Cache {
                    target: "/usr/local/cargo/registry".to_string(),
                    id: Some("cargo-registry".to_string()),
                    sharing: CacheSharing::Shared,
                    readonly: false,
                },
                RunMount::Cache {
                    target: "/app/target".to_string(),
                    id: Some("cargo-target".to_string()),
                    sharing: CacheSharing::Locked,
                    readonly: false,
                },
            ],
            network: None,
            security: None,
        };

        let cmd = BuildahCommand::run_with_mounts("container-1", &run);

        // Count --mount arguments
        let mount_count = cmd
            .args
            .iter()
            .filter(|a| a.starts_with("--mount="))
            .count();
        assert_eq!(mount_count, 2, "should have 2 mount arguments");

        // Verify all mounts come before container ID
        let container_idx = cmd
            .args
            .iter()
            .position(|a| a == "container-1")
            .expect("should have container id");

        for (idx, arg) in cmd.args.iter().enumerate() {
            if arg.starts_with("--mount=") {
                assert!(
                    idx < container_idx,
                    "--mount at index {idx} should come before container ID at {container_idx}",
                );
            }
        }
    }

    #[test]
    fn test_from_instruction_run_with_mounts() {
        use crate::dockerfile::{CacheSharing, RunMount};

        let instruction = Instruction::Run(RunInstruction {
            command: ShellOrExec::Shell("npm install".to_string()),
            mounts: vec![RunMount::Cache {
                target: "/root/.npm".to_string(),
                id: Some("npm-cache".to_string()),
                sharing: CacheSharing::Shared,
                readonly: false,
            }],
            network: None,
            security: None,
        });

        let cmds = BuildahCommand::from_instruction("container-1", &instruction);
        assert_eq!(cmds.len(), 1);

        let cmd = &cmds[0];
        assert!(
            cmd.args.iter().any(|a| a.starts_with("--mount=")),
            "should include --mount argument"
        );
    }

    #[test]
    fn test_run_with_mounts_exec_form() {
        use crate::dockerfile::{CacheSharing, RunMount};

        let run = RunInstruction {
            command: ShellOrExec::Exec(vec![
                "pip".to_string(),
                "install".to_string(),
                "-r".to_string(),
                "requirements.txt".to_string(),
            ]),
            mounts: vec![RunMount::Cache {
                target: "/root/.cache/pip".to_string(),
                id: Some("pip-cache".to_string()),
                sharing: CacheSharing::Shared,
                readonly: false,
            }],
            network: None,
            security: None,
        };

        let cmd = BuildahCommand::run_with_mounts("container-1", &run);

        // Should have mount, container, --, and then the exec args
        assert!(cmd.args.contains(&"--".to_string()));
        assert!(cmd.args.contains(&"pip".to_string()));
        assert!(cmd.args.contains(&"install".to_string()));
    }

    #[test]
    fn test_manifest_create() {
        let cmd = BuildahCommand::manifest_create("myapp:latest");
        assert_eq!(cmd.program, "buildah");
        assert_eq!(cmd.args, vec!["manifest", "create", "myapp:latest"]);
    }

    #[test]
    fn test_manifest_add() {
        let cmd = BuildahCommand::manifest_add("myapp:latest", "myapp-amd64:latest");
        assert_eq!(
            cmd.args,
            vec!["manifest", "add", "myapp:latest", "myapp-amd64:latest"]
        );
    }

    #[test]
    fn test_manifest_push() {
        let cmd =
            BuildahCommand::manifest_push("myapp:latest", "docker://registry.example.com/myapp");
        assert_eq!(
            cmd.args,
            vec![
                "manifest",
                "push",
                "--all",
                "myapp:latest",
                "docker://registry.example.com/myapp"
            ]
        );
    }

    #[test]
    fn test_manifest_rm() {
        let cmd = BuildahCommand::manifest_rm("myapp:latest");
        assert_eq!(cmd.args, vec!["manifest", "rm", "myapp:latest"]);
    }

    // -------------------------------------------------------------------------
    // L-3: OS-aware translation tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_run_shell_for_os_linux() {
        let cmd = BuildahCommand::run_shell_for_os("c1", "echo hello", ImageOs::Linux);
        assert_eq!(
            cmd.args,
            vec!["run", "c1", "--", "/bin/sh", "-c", "echo hello"]
        );
    }

    #[test]
    fn test_run_shell_for_os_windows() {
        let cmd = BuildahCommand::run_shell_for_os("c1", "echo hello", ImageOs::Windows);
        assert_eq!(
            cmd.args,
            vec!["run", "c1", "--", "cmd.exe", "/S", "/C", "echo hello"]
        );
    }

    #[test]
    fn test_run_shell_custom_powershell() {
        let shell = ["powershell", "-Command"];
        let cmd = BuildahCommand::run_shell_custom("c1", shell, "Get-Process");
        assert_eq!(
            cmd.args,
            vec!["run", "c1", "--", "powershell", "-Command", "Get-Process"]
        );
    }

    #[test]
    fn test_translator_linux_run_shell_default() {
        let mut t = DockerfileTranslator::new(ImageOs::Linux);
        let instr = Instruction::Run(RunInstruction::shell("apt-get update"));
        let cmds = t.translate("c1", &instr);
        assert_eq!(cmds.len(), 1);
        assert_eq!(
            cmds[0].args,
            vec!["run", "c1", "--", "/bin/sh", "-c", "apt-get update"]
        );
    }

    #[test]
    fn test_translator_windows_run_shell_default() {
        let mut t = DockerfileTranslator::new(ImageOs::Windows);
        let instr = Instruction::Run(RunInstruction::shell("dir C:\\"));
        let cmds = t.translate("c1", &instr);
        assert_eq!(cmds.len(), 1);
        assert_eq!(
            cmds[0].args,
            vec!["run", "c1", "--", "cmd.exe", "/S", "/C", "dir C:\\"]
        );
    }

    #[test]
    fn test_translator_shell_override_linux_bash() {
        // SHELL ["/bin/bash", "-lc"] then RUN cmd → uses bash
        let mut t = DockerfileTranslator::new(ImageOs::Linux);

        let shell_instr = Instruction::Shell(vec!["/bin/bash".to_string(), "-lc".to_string()]);
        let shell_cmds = t.translate("c1", &shell_instr);
        // SHELL emits the metadata config --shell
        assert_eq!(shell_cmds.len(), 1);
        assert!(shell_cmds[0].args.contains(&"--shell".to_string()));

        let run_instr = Instruction::Run(RunInstruction::shell("set -e; echo $SHELL"));
        let run_cmds = t.translate("c1", &run_instr);
        assert_eq!(run_cmds.len(), 1);
        assert_eq!(
            run_cmds[0].args,
            vec!["run", "c1", "--", "/bin/bash", "-lc", "set -e; echo $SHELL"]
        );
    }

    #[test]
    fn test_translator_shell_override_windows_powershell() {
        // SHELL ["powershell", "-Command"] then RUN cmd on Windows → uses
        // powershell, not the default cmd.exe /S /C.
        let mut t = DockerfileTranslator::new(ImageOs::Windows);

        let shell_instr =
            Instruction::Shell(vec!["powershell".to_string(), "-Command".to_string()]);
        t.translate("c1", &shell_instr);

        let run_instr = Instruction::Run(RunInstruction::shell("Get-Process"));
        let run_cmds = t.translate("c1", &run_instr);
        assert_eq!(run_cmds.len(), 1);
        assert_eq!(
            run_cmds[0].args,
            vec!["run", "c1", "--", "powershell", "-Command", "Get-Process"]
        );
    }

    #[test]
    fn test_translator_shell_override_persists_across_runs() {
        // Two RUNs after a single SHELL should both use the overridden shell.
        let mut t = DockerfileTranslator::new(ImageOs::Linux);
        t.translate(
            "c1",
            &Instruction::Shell(vec!["/bin/bash".to_string(), "-c".to_string()]),
        );

        for _ in 0..2 {
            let cmds = t.translate("c1", &Instruction::Run(RunInstruction::shell("echo hi")));
            assert_eq!(
                cmds[0].args,
                vec!["run", "c1", "--", "/bin/bash", "-c", "echo hi"]
            );
        }
    }

    #[test]
    fn test_translator_exec_form_ignores_shell_override() {
        // Exec-form RUN must not be wrapped with the shell prefix, even
        // when a SHELL override is active — matches Docker semantics.
        let mut t = DockerfileTranslator::new(ImageOs::Windows);
        t.translate(
            "c1",
            &Instruction::Shell(vec!["powershell".to_string(), "-Command".to_string()]),
        );

        let run = Instruction::Run(RunInstruction::exec(vec![
            "myapp.exe".to_string(),
            "--flag".to_string(),
        ]));
        let cmds = t.translate("c1", &run);
        assert_eq!(cmds[0].args, vec!["run", "c1", "--", "myapp.exe", "--flag"]);
    }

    #[test]
    fn test_translator_workdir_linux() {
        let mut t = DockerfileTranslator::new(ImageOs::Linux);
        let cmds = t.translate("c1", &Instruction::Workdir("/app".to_string()));
        assert_eq!(cmds.len(), 2);
        assert_eq!(cmds[0].args, vec!["run", "c1", "--", "mkdir", "-p", "/app"]);
        assert_eq!(cmds[1].args, vec!["config", "--workingdir", "/app", "c1"]);
    }

    #[test]
    fn test_translator_workdir_windows() {
        let mut t = DockerfileTranslator::new(ImageOs::Windows);
        let cmds = t.translate("c1", &Instruction::Workdir("C:\\app".to_string()));
        assert_eq!(cmds.len(), 2);
        // Pre-mkdir guarded with `if not exist` so a repeated WORKDIR in the
        // same Dockerfile doesn't cause Windows mkdir to error on existence.
        assert_eq!(
            cmds[0].args,
            vec![
                "run",
                "c1",
                "--",
                "cmd.exe",
                "/S",
                "/C",
                r#"if not exist "C:\app" mkdir "C:\app""#
            ]
        );
        assert_eq!(
            cmds[1].args,
            vec!["config", "--workingdir", "C:\\app", "c1"]
        );
    }

    #[test]
    fn test_translator_workdir_windows_path_with_spaces() {
        // Paths containing spaces (e.g. `C:\Program Files\app`) must be quoted
        // for cmd.exe, which is why the mkdir command we emit wraps the path
        // in double-quotes.
        let mut t = DockerfileTranslator::new(ImageOs::Windows);
        let cmds = t.translate(
            "c1",
            &Instruction::Workdir("C:\\Program Files\\app".to_string()),
        );
        assert_eq!(cmds.len(), 2);
        let mkdir_cmd = &cmds[0].args[6];
        assert_eq!(
            mkdir_cmd,
            r#"if not exist "C:\Program Files\app" mkdir "C:\Program Files\app""#
        );
    }

    #[test]
    fn test_from_instruction_preserves_linux_byte_identical_output() {
        // Backward-compat guarantee: the legacy `from_instruction` entrypoint
        // must emit the exact same byte-for-byte commands as it did before
        // the translator refactor. The existing Linux callers (buildah backend)
        // rely on this.
        let run = Instruction::Run(RunInstruction::shell("echo hello"));
        let legacy = BuildahCommand::from_instruction("c1", &run);
        let via_translator = DockerfileTranslator::new(ImageOs::Linux).translate("c1", &run);
        assert_eq!(legacy.len(), via_translator.len());
        for (a, b) in legacy.iter().zip(via_translator.iter()) {
            assert_eq!(a.args, b.args);
            assert_eq!(a.program, b.program);
        }

        // WORKDIR must still emit mkdir -p + config --workingdir
        let workdir = Instruction::Workdir("/workspace".to_string());
        let legacy = BuildahCommand::from_instruction("c1", &workdir);
        assert_eq!(legacy.len(), 2);
        assert_eq!(
            legacy[0].args,
            vec!["run", "c1", "--", "mkdir", "-p", "/workspace"]
        );
        assert_eq!(
            legacy[1].args,
            vec!["config", "--workingdir", "/workspace", "c1"]
        );
    }

    #[test]
    fn test_translator_active_shell_reflects_override() {
        let mut t = DockerfileTranslator::new(ImageOs::Linux);
        assert_eq!(t.active_shell(), vec!["/bin/sh", "-c"]);

        t.set_shell_override(vec!["/bin/bash".to_string(), "-lc".to_string()]);
        assert_eq!(t.active_shell(), vec!["/bin/bash", "-lc"]);
    }

    #[test]
    fn test_translator_target_os_accessor() {
        assert_eq!(
            DockerfileTranslator::new(ImageOs::Linux).target_os(),
            ImageOs::Linux
        );
        assert_eq!(
            DockerfileTranslator::new(ImageOs::Windows).target_os(),
            ImageOs::Windows
        );
    }

    #[test]
    fn test_translator_windows_run_with_mounts_uses_cmd_exe() {
        use crate::dockerfile::{CacheSharing, RunMount};

        let mut t = DockerfileTranslator::new(ImageOs::Windows);
        let run = RunInstruction {
            command: ShellOrExec::Shell("echo cached".to_string()),
            mounts: vec![RunMount::Cache {
                target: "C:\\cache".to_string(),
                id: Some("win-cache".to_string()),
                sharing: CacheSharing::Shared,
                readonly: false,
            }],
            network: None,
            security: None,
        };

        let cmds = t.translate("c1", &Instruction::Run(run));
        assert_eq!(cmds.len(), 1);

        // --mount= must precede container ID
        let mount_idx = cmds[0]
            .args
            .iter()
            .position(|a| a.starts_with("--mount="))
            .expect("mount arg present");
        let container_idx = cmds[0]
            .args
            .iter()
            .position(|a| a == "c1")
            .expect("container ID present");
        assert!(mount_idx < container_idx);

        // Shell form must use cmd.exe /S /C, not /bin/sh -c
        assert!(cmds[0].args.iter().any(|a| a == "cmd.exe"));
        assert!(cmds[0].args.iter().any(|a| a == "/S"));
        assert!(cmds[0].args.iter().any(|a| a == "/C"));
        assert!(!cmds[0].args.iter().any(|a| a == "/bin/sh"));
    }
}
