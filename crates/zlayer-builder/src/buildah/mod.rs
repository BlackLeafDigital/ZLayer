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
#[cfg(unix)]
pub use install::buildd as buildd_install;
#[cfg(unix)]
pub use install::buildd::{ensure_buildd_sidecar, InstallOutcome as SidecarInstallOutcome};
pub use install::{
    current_platform, install_instructions, is_platform_supported, BuildahInstallation,
    BuildahInstaller, InstallError,
};

use crate::backend::ImageOs;
use crate::dockerfile::{
    AddInstruction, CopyInstruction, EnvInstruction, ExposeInstruction, HealthcheckInstruction,
    Instruction, RunInstruction, RunNetwork, ShellOrExec,
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
        Self::run_shell_custom_with_net(container, shell, command, None)
    }

    /// Run a command in the container (shell form) using an explicit shell,
    /// optionally pinning the network mode.
    ///
    /// `buildah run [--net=<mode>] <container> -- <shell...> <command>`
    ///
    /// See [`Self::run_exec_with_net`] for the meaning of `net`.
    #[must_use]
    pub fn run_shell_custom_with_net(
        container: &str,
        shell: impl IntoIterator<Item = impl AsRef<str>>,
        command: &str,
        net: Option<RunNetwork>,
    ) -> Self {
        let mut cmd = Self::new("run");
        if let Some(mode) = net {
            match mode {
                RunNetwork::Host => cmd = cmd.arg("--net=host"),
                RunNetwork::None => cmd = cmd.arg("--net=none"),
                RunNetwork::Default => {}
            }
        }
        cmd = cmd.arg(container).arg("--");
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
        Self::run_exec_with_net(container, args, None)
    }

    /// Run a command in the container (exec form), optionally pinning the
    /// network mode.
    ///
    /// `buildah run [--net=<mode>] <container> -- <args...>`
    ///
    /// The `--net=<mode>` flag MUST precede the container ID — buildah parses
    /// flags up to the first positional and then treats the rest as the
    /// command. When `net` is `None`, no `--net` flag is emitted and buildah
    /// uses its default rootless networking. Use `Some(RunNetwork::Host)`
    /// when the translator-level `host_network` override is on, so the
    /// emitted `buildah run` bypasses CNI/netavark just like the dedicated
    /// `RUN` instruction path does.
    #[must_use]
    pub fn run_exec_with_net(container: &str, args: &[String], net: Option<RunNetwork>) -> Self {
        let mut cmd = Self::new("run");
        if let Some(mode) = net {
            match mode {
                RunNetwork::Host => cmd = cmd.arg("--net=host"),
                RunNetwork::None => cmd = cmd.arg("--net=none"),
                RunNetwork::Default => {}
            }
        }
        cmd = cmd.arg(container).arg("--");
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

        // Add transient --env=K=V flags BEFORE the container ID. Sort by key
        // for deterministic ordering (HashMap iteration is non-deterministic).
        // These are scoped to this single `buildah run` invocation and are
        // intentionally NOT persisted into the image config.
        let mut env_keys: Vec<&String> = run.env.keys().collect();
        env_keys.sort();
        for key in env_keys {
            if let Some(value) = run.env.get(key) {
                cmd = cmd.arg(format!("--env={key}={value}"));
            }
        }

        // Add --net=<value> BEFORE the container ID when a network mode is set.
        // `RunNetwork::Default` is omitted (buildah's default is `private`),
        // matching Docker's BuildKit semantics where `--network` is only
        // emitted when the user opts out of the default. `--net` is the
        // canonical buildah spelling per the man page (both `--net` and
        // `--network` work, but `--net` is shorter and matches buildah's
        // own help output).
        if let Some(net) = run.network {
            match net {
                RunNetwork::Host => {
                    cmd = cmd.arg("--net=host");
                }
                RunNetwork::None => {
                    cmd = cmd.arg("--net=none");
                }
                RunNetwork::Default => {}
            }
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
    /// When `true`, every emitted `buildah run` for a `RUN` instruction will
    /// carry `--net=host` regardless of any per-instruction `network` value.
    /// Mirrors Docker's `docker build --network=host` flag and bypasses
    /// buildah's CNI/netavark plumbing entirely (the container shares the
    /// host's network namespace).
    host_network: bool,
}

impl DockerfileTranslator {
    /// Create a new translator for a given target OS, with no `SHELL` override.
    #[must_use]
    pub fn new(target_os: ImageOs) -> Self {
        Self {
            target_os,
            shell_override: None,
            host_network: false,
        }
    }

    /// Force every translated `RUN` instruction to use host networking.
    ///
    /// When `on` is `true`, the translator overrides any per-instruction
    /// `network` value (including `None`) with [`RunNetwork::Host`] before
    /// emitting the buildah command. This mirrors the effect of Docker's
    /// `docker build --network=host` flag and bypasses buildah's CNI /
    /// netavark plumbing entirely. When `on` is `false` (the default),
    /// per-instruction `network` values are passed through unchanged.
    #[must_use]
    pub fn with_host_network(mut self, on: bool) -> Self {
        self.host_network = on;
        self
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
                // Apply the translator-level host_network override: when set,
                // every emitted RUN gets `--net=host` regardless of any
                // per-instruction network value. We clone only when we
                // actually need to mutate so the no-override path stays
                // allocation-free.
                let effective_run: std::borrow::Cow<'_, RunInstruction> = if self.host_network {
                    let mut owned = run.clone();
                    owned.network = Some(RunNetwork::Host);
                    std::borrow::Cow::Owned(owned)
                } else {
                    std::borrow::Cow::Borrowed(run)
                };
                let run_ref: &RunInstruction = &effective_run;

                // Route through run_with_mounts_shell whenever mounts, env,
                // or a network mode are present, since the simple factories
                // don't emit those pre-container flags.
                let needs_pre_container_flags = !run_ref.mounts.is_empty()
                    || !run_ref.env.is_empty()
                    || run_ref.network.is_some();

                if needs_pre_container_flags {
                    vec![BuildahCommand::run_with_mounts_shell(
                        container, run_ref, &shell,
                    )]
                } else {
                    match &run_ref.command {
                        ShellOrExec::Shell(s) => {
                            vec![BuildahCommand::run_shell_custom(container, &shell, s)]
                        }
                        ShellOrExec::Exec(args) => vec![BuildahCommand::run_exec(container, args)],
                    }
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
        // Mirror `Instruction::Run`: when the translator was constructed with
        // `with_host_network(true)`, every emitted `buildah run` MUST carry
        // `--net=host` so the build bypasses buildah's rootless CNI/netavark
        // plumbing entirely. Without this, a WORKDIR — which runs `mkdir -p`
        // through `buildah run` — gets routed through netavark even though
        // the user opted out, and dies on the first instruction of the first
        // stage when the host's netavark config is broken.
        let net = self.host_network.then_some(RunNetwork::Host);
        match self.target_os {
            ImageOs::Linux => {
                vec![
                    BuildahCommand::run_exec_with_net(
                        container,
                        &["mkdir".to_string(), "-p".to_string(), dir.to_string()],
                        net,
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
                    BuildahCommand::run_shell_custom_with_net(
                        container,
                        WINDOWS_DEFAULT_SHELL,
                        &guarded,
                        net,
                    ),
                    BuildahCommand::config_workdir(container, dir),
                ]
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Linux-package-manager → Chocolatey translation helpers
//
// These were originally housed in `crate::windows_builder` and are used to
// rewrite Linux package-manager invocations (`apt-get install`, `apk add`,
// `yum/dnf install`) inside `RUN` instructions into a Chocolatey
// (`choco install -y …`) equivalent when the target OS is Windows. Moving
// them here lets both the production `HcsBackend` and the in-process
// `WindowsBuilder` test harness flow through one translator instead of
// duplicating the logic.
//
// The free helpers below are kept `pub(crate)` so the existing
// `windows_builder` test module (and any other in-crate caller) can keep
// exercising them directly without going through `DockerfileTranslator`.
// ---------------------------------------------------------------------------

/// Which Linux package manager an install sub-command was issued against.
//
// The whole apt→choco helper surface is gated on `windows || test` so
// non-Windows production builds don't warn about dead code — every call
// site lives in either the Windows-only `windows_builder` production
// path or a `#[cfg(test)]` unit test.
#[cfg(any(target_os = "windows", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DetectedPmKind {
    /// `apt-get install -y …` or `apt install -y …`.
    Apt,
    /// `apk add [--no-cache] …`.
    Apk,
    /// `yum install -y …` or `dnf install -y …`.
    YumOrDnf,
}

/// One sub-command parsed out of a shell-form RUN string.
#[cfg(any(target_os = "windows", test))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ShellSubcommand {
    /// The literal text of a non-install sub-command, kept verbatim.
    Verbatim(String),
    /// The literal text of an `apt-get update` / `apk update` /
    /// `dnf check-update` style sync command. Surfaced as a distinct
    /// variant so we can elide it (no Chocolatey equivalent) instead of
    /// passing it through verbatim and breaking the shell.
    PackageManagerSync,
    /// A detected install invocation: kind + the package list.
    Install {
        kind: DetectedPmKind,
        packages: Vec<String>,
    },
}

/// Detect whether a single shell sub-command is an `apt-get install`,
/// `apk add`, `yum install`, or `dnf install` invocation. Returns
/// `Some((kind, packages))` if so. Flag-only arguments (starting with
/// `-`) are stripped; bare positional args are treated as package names.
#[cfg(any(target_os = "windows", test))]
pub(crate) fn detect_install_in_subcommand(
    subcommand: &str,
) -> Option<(DetectedPmKind, Vec<String>)> {
    let tokens: Vec<&str> = subcommand.split_whitespace().collect();
    if tokens.is_empty() {
        return None;
    }
    // Drop `sudo` if present so `sudo apt-get install -y curl` is
    // recognised the same as the bareword form.
    let (kind, after_verb_idx) = match tokens[0] {
        "sudo" if tokens.len() >= 2 => detect_pm_verb(&tokens[1..]).map(|(k, n)| (k, n + 1))?,
        _ => detect_pm_verb(&tokens)?,
    };
    let args = &tokens[after_verb_idx..];
    let mut packages = Vec::new();
    for arg in args {
        if arg.starts_with('-') {
            continue;
        }
        packages.push((*arg).to_string());
    }
    if packages.is_empty() {
        return None;
    }
    Some((kind, packages))
}

/// Recognise the `<pm> <verb>` prefix of a sub-command. Returns
/// `(kind, tokens_consumed)` on success — the caller then walks
/// `tokens[tokens_consumed..]` for the package list.
#[cfg(any(target_os = "windows", test))]
fn detect_pm_verb(tokens: &[&str]) -> Option<(DetectedPmKind, usize)> {
    match (tokens.first().copied(), tokens.get(1).copied()) {
        (Some("apt-get" | "apt"), Some("install")) => Some((DetectedPmKind::Apt, 2)),
        (Some("apk"), Some("add")) => Some((DetectedPmKind::Apk, 2)),
        (Some("yum" | "dnf"), Some("install")) => Some((DetectedPmKind::YumOrDnf, 2)),
        _ => None,
    }
}

/// Recognise package-manager-sync invocations (`apt-get update`,
/// `apk update`, `dnf check-update`, etc.) so we can elide them in the
/// rewritten command — Chocolatey resolves package metadata on every
/// install and has no separate sync step.
#[cfg(any(target_os = "windows", test))]
pub(crate) fn is_package_manager_sync(subcommand: &str) -> bool {
    let tokens: Vec<&str> = subcommand.split_whitespace().collect();
    let stripped: &[&str] = if tokens.first().copied() == Some("sudo") {
        &tokens[1..]
    } else {
        &tokens
    };
    matches!(
        (stripped.first().copied(), stripped.get(1).copied()),
        (Some("apt-get" | "apt" | "apk"), Some("update"))
            | (
                Some("yum" | "dnf"),
                Some("check-update" | "update" | "makecache")
            )
    )
}

/// Split a shell-form RUN command on `&&` and `;` boundaries, preserving
/// each sub-command's text. Quoted regions are NOT honoured (Docker
/// shell-form is itself a string passed to `cmd /c` which doesn't
/// preserve nested shell quoting either; matching that lenient
/// behaviour avoids a regex dep and keeps the implementation simple).
#[cfg(any(target_os = "windows", test))]
pub(crate) fn split_shell_subcommands(raw: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '&' if chars.peek() == Some(&'&') => {
                chars.next();
                if !current.trim().is_empty() {
                    out.push(current.trim().to_string());
                }
                current.clear();
            }
            ';' => {
                if !current.trim().is_empty() {
                    out.push(current.trim().to_string());
                }
                current.clear();
            }
            other => current.push(other),
        }
    }
    if !current.trim().is_empty() {
        out.push(current.trim().to_string());
    }
    out
}

/// Re-join a list of [`ShellSubcommand`]s back into a single
/// `cmd /c`-compatible string, eliding sync sub-commands and rewriting
/// install sub-commands as `choco install -y …`.
#[cfg(any(target_os = "windows", test))]
pub(crate) fn rejoin_subcommands(parts: &[ShellSubcommand]) -> String {
    let mut emitted: Vec<String> = Vec::new();
    for part in parts {
        match part {
            ShellSubcommand::Verbatim(s) => emitted.push(s.clone()),
            ShellSubcommand::PackageManagerSync => {
                // Eliding: no equivalent in Chocolatey.
            }
            ShellSubcommand::Install { packages, .. } => {
                if packages.is_empty() {
                    continue;
                }
                let mut joined = String::from("choco install -y");
                for pkg in packages {
                    joined.push(' ');
                    joined.push_str(pkg);
                }
                emitted.push(joined);
            }
        }
    }
    emitted.join(" && ")
}

/// Wrap a shell command body in `cmd /c "…"` so HCS's `CreateProcess`
/// invokes the Windows command interpreter. Embedded double quotes are
/// backslash-escaped per the NT `CommandLineToArgvW` convention. An
/// empty body still produces a well-formed (no-op) command.
#[cfg(any(target_os = "windows", test))]
pub(crate) fn wrap_in_cmd(body: &str) -> String {
    if body.is_empty() {
        return "cmd /c \"\"".to_string();
    }
    let escaped = body.replace('"', "\\\"");
    format!("cmd /c \"{escaped}\"")
}

/// Return `true` if `linux_pkg` is the Linux name of the language toolchain
/// indicated by `toolchain_language`. Used to drop packages that are
/// already provisioned directly into the rootfs via
/// `crate::windows_toolchain`, so we don't re-install (a possibly
/// conflicting) Chocolatey copy on top.
///
/// Match is case-insensitive on the toolchain language and exact on the
/// package name (lower-cased). The mapping mirrors what users typically
/// write in Linux Dockerfiles for each language:
///
/// | toolchain language | matching Linux package names      |
/// |--------------------|-----------------------------------|
/// | `go`               | `golang`, `go`                    |
/// | `node`             | `nodejs`, `node`                  |
/// | `python`           | `python3`, `python`               |
/// | `rust`             | `rust`, `rustc`, `cargo`          |
/// | `deno`             | `deno`                            |
/// | `bun`              | `bun`                             |
#[cfg(any(target_os = "windows", test))]
fn package_matches_toolchain(linux_pkg: &str, toolchain_language: &str) -> bool {
    let pkg = linux_pkg.to_ascii_lowercase();
    match toolchain_language.to_ascii_lowercase().as_str() {
        "go" => matches!(pkg.as_str(), "golang" | "go"),
        "node" => matches!(pkg.as_str(), "nodejs" | "node"),
        "python" => matches!(pkg.as_str(), "python3" | "python"),
        "rust" => matches!(pkg.as_str(), "rust" | "rustc" | "cargo"),
        "deno" => pkg == "deno",
        "bun" => pkg == "bun",
        _ => false,
    }
}

#[cfg(any(target_os = "windows", test))]
impl DockerfileTranslator {
    /// Translate a `RUN` command (shell- or exec-form) for the
    /// translator's target OS.
    ///
    /// - **Linux**: returns the command verbatim — `(joined_string, [])` —
    ///   because Linux RUNs are handed straight to `/bin/sh -c` by the
    ///   buildah backend and need no rewriting.
    /// - **Windows**: exec-form is passed through (joined with spaces);
    ///   shell-form is forwarded to [`translate_shell_command`] which
    ///   detects apt/apk/yum/dnf invocations and rewrites them as
    ///   `choco install -y …` against the Chocolatey package map for
    ///   `source_distro`.
    ///
    /// If `provisioned_toolchain_language` is `Some(lang)`, any package
    /// matching that language ([`package_matches_toolchain`]) is **dropped**
    /// from the install list — the toolchain is already on `PATH` via
    /// `crate::windows_toolchain` and re-installing via Chocolatey would
    /// either fail or shadow the provisioned binary.
    ///
    /// Returns `(translated_command, skipped_packages)` where
    /// `skipped_packages` enumerates Linux package names whose Chocolatey
    /// mapping is the `__skip__` sentinel (no Windows equivalent — the
    /// caller typically logs them for the user).
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::BuildError::ChocoResolutionFailed`] if any
    /// Linux package detected in the install list has no mapping in the
    /// Chocolatey package map for `source_distro`. Returns whatever error
    /// `resolve_chocolatey_packages` surfaces for cache setup failures.
    pub(crate) async fn translate_run_command(
        &self,
        cmd: &ShellOrExec,
        source_distro: &str,
        provisioned_toolchain_language: Option<&str>,
    ) -> Result<(String, Vec<String>), crate::error::BuildError> {
        match self.target_os {
            ImageOs::Linux => match cmd {
                ShellOrExec::Shell(s) => Ok((s.clone(), Vec::new())),
                ShellOrExec::Exec(args) => Ok((args.join(" "), Vec::new())),
            },
            ImageOs::Windows => match cmd {
                ShellOrExec::Exec(args) => Ok((args.join(" "), Vec::new())),
                ShellOrExec::Shell(raw) => {
                    self.translate_shell_command(raw, source_distro, provisioned_toolchain_language)
                        .await
                }
            },
        }
    }

    /// Translate a shell-form RUN command. Behaviour matches
    /// [`Self::translate_run_command`] — Linux returns the input verbatim,
    /// Windows rewrites Linux package-manager invocations to Chocolatey.
    ///
    /// See [`Self::translate_run_command`] for the meaning of
    /// `provisioned_toolchain_language`.
    ///
    /// # Errors
    ///
    /// See [`Self::translate_run_command`].
    pub(crate) async fn translate_shell_command(
        &self,
        raw: &str,
        source_distro: &str,
        provisioned_toolchain_language: Option<&str>,
    ) -> Result<(String, Vec<String>), crate::error::BuildError> {
        if matches!(self.target_os, ImageOs::Linux) {
            return Ok((raw.to_string(), Vec::new()));
        }

        let subcommands = split_shell_subcommands(raw);
        if subcommands.is_empty() {
            // Empty RUN — defer to the underlying shell which will be a
            // no-op. `cmd /c` accepts an empty argument list and exits 0.
            return Ok((wrap_in_cmd(""), Vec::new()));
        }

        let mut classified: Vec<ShellSubcommand> = Vec::with_capacity(subcommands.len());
        let mut all_packages: Vec<String> = Vec::new();
        for sub in &subcommands {
            if is_package_manager_sync(sub) {
                classified.push(ShellSubcommand::PackageManagerSync);
                continue;
            }
            if let Some((kind, mut packages)) = detect_install_in_subcommand(sub) {
                // Drop packages already covered by the provisioned
                // toolchain — re-installing via Chocolatey would either
                // fail (version conflict) or shadow the provisioned
                // binary that the rest of the build relies on.
                if let Some(lang) = provisioned_toolchain_language {
                    packages.retain(|p| !package_matches_toolchain(p, lang));
                }
                if packages.is_empty() {
                    // Every package in this sub-command was the
                    // toolchain; nothing left to install, so elide the
                    // sub-command entirely.
                    continue;
                }
                all_packages.extend(packages.iter().cloned());
                classified.push(ShellSubcommand::Install { kind, packages });
                continue;
            }
            classified.push(ShellSubcommand::Verbatim(sub.clone()));
        }

        if all_packages.is_empty() {
            // No install was detected — pass the original shell command
            // through `cmd /c` unchanged. We re-join from the classified
            // parts so an `apt-get update`-only RUN still elides correctly.
            let rejoined = rejoin_subcommands(&classified);
            return Ok((wrap_in_cmd(&rejoined), Vec::new()));
        }

        // Bulk-resolve every package across every install sub-command in
        // one go so duplicate shard fetches are coalesced inside the
        // resolver.
        let resolved = crate::windows_image_resolver::resolve_chocolatey_packages(
            &all_packages,
            source_distro,
        )
        .await?;

        let mut lookup: HashMap<String, (Option<String>, bool)> = HashMap::new();
        for (linux, choco, skipped) in resolved {
            lookup.insert(linux, (choco, skipped));
        }

        let mut skipped_out: Vec<String> = Vec::new();
        for part in &mut classified {
            if let ShellSubcommand::Install { kind: _, packages } = part {
                let mut rewritten: Vec<String> = Vec::new();
                for pkg in packages.iter() {
                    match lookup.get(pkg) {
                        Some((Some(choco), false)) => rewritten.push(choco.clone()),
                        Some((_, true)) => skipped_out.push(pkg.clone()),
                        Some((None, false)) | None => {
                            // No mapping — surface as an error so the user
                            // gets a precise diagnostic instead of a
                            // silently-broken image.
                            return Err(crate::error::BuildError::ChocoResolutionFailed {
                                package: pkg.clone(),
                                source_distro: source_distro.to_string(),
                            });
                        }
                    }
                }
                *packages = rewritten;
            }
        }

        Ok((wrap_in_cmd(&rejoin_subcommands(&classified)), skipped_out))
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
            env: HashMap::new(),
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
            env: HashMap::new(),
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
            env: HashMap::new(),
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
            env: HashMap::new(),
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
            env: HashMap::new(),
        };

        let cmd = BuildahCommand::run_with_mounts("container-1", &run);

        // Should have mount, container, --, and then the exec args
        assert!(cmd.args.contains(&"--".to_string()));
        assert!(cmd.args.contains(&"pip".to_string()));
        assert!(cmd.args.contains(&"install".to_string()));
    }

    #[test]
    fn test_run_with_mounts_emits_env_flags_sorted() {
        // RunInstruction with `env` must produce `--env=K=V` args BEFORE the
        // container ID, sorted by key for determinism. Env is intentionally
        // scoped to this single buildah-run invocation (not baked into the
        // image config via `buildah config --env`).
        let mut env = HashMap::new();
        env.insert("B".to_string(), "2".to_string());
        env.insert("A".to_string(), "1".to_string());

        let run = RunInstruction {
            command: ShellOrExec::Shell("env".to_string()),
            mounts: vec![],
            network: None,
            security: None,
            env,
        };

        let cmd = BuildahCommand::run_with_mounts("container-1", &run);

        // Confirm both --env flags appear, in sorted (A then B) order.
        let env_positions: Vec<(usize, &String)> = cmd
            .args
            .iter()
            .enumerate()
            .filter(|(_, a)| a.starts_with("--env="))
            .collect();
        assert_eq!(
            env_positions.len(),
            2,
            "expected 2 --env args, got {env_positions:?}"
        );
        assert_eq!(env_positions[0].1, "--env=A=1");
        assert_eq!(env_positions[1].1, "--env=B=2");

        // Both --env flags must come BEFORE the container ID.
        let container_idx = cmd
            .args
            .iter()
            .position(|a| a == "container-1")
            .expect("container ID present");
        for (idx, _) in &env_positions {
            assert!(
                *idx < container_idx,
                "--env at {idx} must precede container ID at {container_idx}"
            );
        }

        // And BEFORE the `--` separator.
        let sep_idx = cmd
            .args
            .iter()
            .position(|a| a == "--")
            .expect("-- separator present");
        for (idx, _) in &env_positions {
            assert!(
                *idx < sep_idx,
                "--env at {idx} must precede `--` at {sep_idx}"
            );
        }
    }

    #[test]
    fn test_translator_routes_env_only_run_through_mounts_path() {
        // A RunInstruction with env but no mounts must still flow through
        // run_with_mounts_shell so the --env= flags get emitted. The plain
        // run_shell / run_exec factories don't know about env.
        let mut env = HashMap::new();
        env.insert("FOO".to_string(), "bar".to_string());

        let run = RunInstruction {
            command: ShellOrExec::Shell("echo $FOO".to_string()),
            mounts: vec![],
            network: None,
            security: None,
            env,
        };

        let cmds = DockerfileTranslator::new(ImageOs::Linux)
            .translate("container-1", &Instruction::Run(run));
        assert_eq!(cmds.len(), 1);

        let cmd = &cmds[0];
        assert!(
            cmd.args.iter().any(|a| a == "--env=FOO=bar"),
            "expected --env=FOO=bar in args: {:?}",
            cmd.args
        );
    }

    // ---------------------------------------------------------------------
    // Host-network plumbing
    //
    // These tests pin the `--host-network` CLI flag's end-to-end behavior:
    // a `RunInstruction` with `network: Some(RunNetwork::Host)` MUST emit
    // `--net=host` BEFORE the container ID on the buildah command, and the
    // translator-level `with_host_network(true)` MUST force-set host
    // networking on every RUN regardless of any per-instruction value.
    // ---------------------------------------------------------------------

    #[test]
    fn test_run_with_mounts_emits_net_host_before_container() {
        let run = RunInstruction {
            command: ShellOrExec::Shell("apt-get update".to_string()),
            mounts: vec![],
            network: Some(crate::dockerfile::RunNetwork::Host),
            security: None,
            env: HashMap::new(),
        };

        let cmd = BuildahCommand::run_with_mounts("container-1", &run);

        let net_idx = cmd
            .args
            .iter()
            .position(|a| a == "--net=host")
            .expect("expected --net=host arg");
        let container_idx = cmd
            .args
            .iter()
            .position(|a| a == "container-1")
            .expect("container id present");
        let sep_idx = cmd
            .args
            .iter()
            .position(|a| a == "--")
            .expect("-- separator present");
        assert!(
            net_idx < container_idx,
            "--net=host (idx {net_idx}) must precede container ID (idx {container_idx})"
        );
        assert!(
            net_idx < sep_idx,
            "--net=host (idx {net_idx}) must precede `--` (idx {sep_idx})"
        );
    }

    #[test]
    fn test_run_with_mounts_emits_net_none() {
        let run = RunInstruction {
            command: ShellOrExec::Shell("hostname".to_string()),
            mounts: vec![],
            network: Some(crate::dockerfile::RunNetwork::None),
            security: None,
            env: HashMap::new(),
        };

        let cmd = BuildahCommand::run_with_mounts("container-1", &run);
        assert!(
            cmd.args.iter().any(|a| a == "--net=none"),
            "expected --net=none in args, got: {:?}",
            cmd.args
        );
    }

    #[test]
    fn test_run_with_mounts_default_network_omits_net_flag() {
        let run = RunInstruction {
            command: ShellOrExec::Shell("true".to_string()),
            mounts: vec![],
            network: Some(crate::dockerfile::RunNetwork::Default),
            security: None,
            env: HashMap::new(),
        };

        let cmd = BuildahCommand::run_with_mounts("container-1", &run);
        assert!(
            !cmd.args.iter().any(|a| a.starts_with("--net")),
            "RunNetwork::Default must NOT emit any --net flag, got: {:?}",
            cmd.args
        );
    }

    #[test]
    fn test_run_with_mounts_no_network_field_omits_net_flag() {
        let run = RunInstruction {
            command: ShellOrExec::Shell("true".to_string()),
            mounts: vec![],
            network: None,
            security: None,
            env: HashMap::new(),
        };

        let cmd = BuildahCommand::run_with_mounts("container-1", &run);
        assert!(
            !cmd.args.iter().any(|a| a.starts_with("--net")),
            "network=None must NOT emit any --net flag, got: {:?}",
            cmd.args
        );
    }

    #[test]
    fn test_translator_host_network_forces_net_host_on_run_with_none_network() {
        // Load-bearing assertion for the `zlayer --host-network` CLI flag:
        // host_network=true must emit --net=host even when the Dockerfile
        // RUN does NOT specify a per-instruction --network.
        let run = RunInstruction {
            command: ShellOrExec::Shell("apt-get update".to_string()),
            mounts: vec![],
            network: None,
            security: None,
            env: HashMap::new(),
        };

        let cmds = DockerfileTranslator::new(ImageOs::Linux)
            .with_host_network(true)
            .translate("c1", &Instruction::Run(run));

        assert_eq!(cmds.len(), 1, "expected exactly one buildah command");
        let cmd = &cmds[0];
        assert!(
            cmd.args.iter().any(|a| a == "--net=host"),
            "expected --net=host in args (host_network=true should force it even when run.network is None), got: {:?}",
            cmd.args
        );
    }

    #[test]
    fn test_translator_host_network_overrides_per_instruction_network_none() {
        // `RUN --network=none` + CLI `--host-network` -> host wins.
        let run = RunInstruction {
            command: ShellOrExec::Shell("apt-get install -y curl".to_string()),
            mounts: vec![],
            network: Some(crate::dockerfile::RunNetwork::None),
            security: None,
            env: HashMap::new(),
        };

        let cmds = DockerfileTranslator::new(ImageOs::Linux)
            .with_host_network(true)
            .translate("c1", &Instruction::Run(run));

        assert_eq!(cmds.len(), 1);
        let cmd = &cmds[0];
        assert!(
            cmd.args.iter().any(|a| a == "--net=host"),
            "host_network=true must override RunNetwork::None, got: {:?}",
            cmd.args
        );
        assert!(
            !cmd.args.iter().any(|a| a == "--net=none"),
            "host_network=true must REPLACE (not append to) RunNetwork::None, got: {:?}",
            cmd.args
        );
    }

    #[test]
    fn test_translator_host_network_routes_bare_run_through_mounts_path() {
        // Bare RUN (no mounts, env, or network) must still flow through
        // run_with_mounts_shell when host_network=true — the simple
        // factories don't emit --net=host.
        let run = RunInstruction {
            command: ShellOrExec::Shell("echo hi".to_string()),
            mounts: vec![],
            network: None,
            security: None,
            env: HashMap::new(),
        };

        let cmds = DockerfileTranslator::new(ImageOs::Linux)
            .with_host_network(true)
            .translate("c1", &Instruction::Run(run));

        assert_eq!(cmds.len(), 1);
        let cmd = &cmds[0];
        assert!(
            cmd.args.iter().any(|a| a == "--net=host"),
            "bare RUN with host_network=true must emit --net=host, got: {:?}",
            cmd.args
        );
    }

    #[test]
    fn test_translator_host_network_routes_env_only_run_with_net_host() {
        // env-only RUN + host_network=true must produce BOTH --env=... and
        // --net=host, both before the container ID.
        let mut env = HashMap::new();
        env.insert("FOO".to_string(), "bar".to_string());

        let run = RunInstruction {
            command: ShellOrExec::Shell("echo $FOO".to_string()),
            mounts: vec![],
            network: None,
            security: None,
            env,
        };

        let cmds = DockerfileTranslator::new(ImageOs::Linux)
            .with_host_network(true)
            .translate("c1", &Instruction::Run(run));

        assert_eq!(cmds.len(), 1);
        let cmd = &cmds[0];

        let env_idx = cmd
            .args
            .iter()
            .position(|a| a == "--env=FOO=bar")
            .expect("--env=FOO=bar present");
        let net_idx = cmd
            .args
            .iter()
            .position(|a| a == "--net=host")
            .expect("--net=host present");
        let container_idx = cmd
            .args
            .iter()
            .position(|a| a == "c1")
            .expect("container id present");
        assert!(env_idx < container_idx);
        assert!(net_idx < container_idx);
    }

    #[test]
    fn test_translator_host_network_routes_mount_only_run_with_net_host() {
        use crate::dockerfile::{CacheSharing, RunMount};

        let run = RunInstruction {
            command: ShellOrExec::Shell("npm install".to_string()),
            mounts: vec![RunMount::Cache {
                target: "/root/.npm".to_string(),
                id: Some("npm-cache".to_string()),
                sharing: CacheSharing::Shared,
                readonly: false,
            }],
            network: None,
            security: None,
            env: HashMap::new(),
        };

        let cmds = DockerfileTranslator::new(ImageOs::Linux)
            .with_host_network(true)
            .translate("c1", &Instruction::Run(run));

        assert_eq!(cmds.len(), 1);
        let cmd = &cmds[0];
        assert!(
            cmd.args.iter().any(|a| a.starts_with("--mount=")),
            "--mount must be present"
        );
        assert!(
            cmd.args.iter().any(|a| a == "--net=host"),
            "--net=host must be present alongside --mount when host_network=true"
        );
    }

    #[test]
    fn test_translator_host_network_default_off_does_not_emit_net_flag() {
        // Sanity: default translator (host_network not set) must not emit
        // --net=... on a vanilla RUN. We're only adding a flag, never
        // silently changing existing behavior.
        let run = RunInstruction {
            command: ShellOrExec::Shell("true".to_string()),
            mounts: vec![],
            network: None,
            security: None,
            env: HashMap::new(),
        };

        let cmds =
            DockerfileTranslator::new(ImageOs::Linux).translate("c1", &Instruction::Run(run));
        assert_eq!(cmds.len(), 1);
        let cmd = &cmds[0];
        assert!(
            !cmd.args.iter().any(|a| a.starts_with("--net")),
            "default translator (host_network=false) must NOT emit --net flag, got: {:?}",
            cmd.args
        );
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
    fn test_translator_workdir_host_network_linux_emits_net_host() {
        // Regression: when the translator is constructed with
        // `with_host_network(true)` (i.e. the user passed `--host-network`),
        // WORKDIR's `mkdir -p` MUST carry `--net=host` just like every
        // RUN does. Before the fix, only `Instruction::Run` consulted
        // `self.host_network`, so a `WORKDIR /app` would route through
        // buildah's default rootless networking → netavark → die on the
        // very first instruction of stage 1 if the host's netavark
        // config was broken.
        let mut t = DockerfileTranslator::new(ImageOs::Linux).with_host_network(true);
        let cmds = t.translate("c1", &Instruction::Workdir("/app".to_string()));
        assert_eq!(cmds.len(), 2);
        assert_eq!(
            cmds[0].args,
            vec!["run", "--net=host", "c1", "--", "mkdir", "-p", "/app"],
            "WORKDIR mkdir with host_network=true must emit --net=host BEFORE the container ID",
        );
        // The `config --workingdir` metadata write is unaffected by
        // host_network — `buildah config` doesn't run a container.
        assert_eq!(cmds[1].args, vec!["config", "--workingdir", "/app", "c1"]);
    }

    #[test]
    fn test_translator_workdir_no_host_network_omits_net_flag() {
        // Pin the default-path behavior: without `--host-network`,
        // WORKDIR's mkdir must NOT carry any `--net` flag. Mirrors the
        // existing `test_translator_workdir_linux` but makes the
        // host_network=false branch explicit so a future refactor that
        // accidentally always emits `--net=host` is caught immediately.
        let mut t = DockerfileTranslator::new(ImageOs::Linux).with_host_network(false);
        let cmds = t.translate("c1", &Instruction::Workdir("/app".to_string()));
        assert_eq!(cmds.len(), 2);
        assert!(
            !cmds[0].args.iter().any(|a| a.starts_with("--net")),
            "WORKDIR with host_network=false must NOT emit any --net flag, got: {:?}",
            cmds[0].args
        );
        assert_eq!(cmds[0].args, vec!["run", "c1", "--", "mkdir", "-p", "/app"]);
    }

    #[test]
    fn test_translator_workdir_host_network_windows_emits_net_host() {
        // Symmetric coverage for the Windows guarded-mkdir path. Windows
        // builds dispatch through the HCS backend in practice (not buildah),
        // so this is belt-and-suspenders: if someone ever wires the buildah
        // translator into a Windows build, host_network must still be
        // honored on the guarded `if not exist ... mkdir` invocation.
        let mut t = DockerfileTranslator::new(ImageOs::Windows).with_host_network(true);
        let cmds = t.translate("c1", &Instruction::Workdir("C:\\app".to_string()));
        assert_eq!(cmds.len(), 2);
        let net_idx = cmds[0]
            .args
            .iter()
            .position(|a| a == "--net=host")
            .expect("expected --net=host on Windows WORKDIR with host_network=true");
        let container_idx = cmds[0]
            .args
            .iter()
            .position(|a| a == "c1")
            .expect("container ID present");
        let sep_idx = cmds[0]
            .args
            .iter()
            .position(|a| a == "--")
            .expect("`--` separator present");
        assert!(
            net_idx < container_idx && container_idx < sep_idx,
            "argument order must be: run --net=host <container> -- ... (got {:?})",
            cmds[0].args
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
            env: HashMap::new(),
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

    // -----------------------------------------------------------------
    // apt → choco translation (moved from `windows_builder::tests`)
    // -----------------------------------------------------------------

    use crate::windows_image_resolver::{ChocoMapMetadata, ChocoMapShard};

    /// Tests that mutate `XDG_CACHE_HOME` / `LOCALAPPDATA` must run
    /// serially or the resolver picks up another test's fixture dir
    /// because `cargo test` runs unit tests in parallel by default.
    static CACHE_ENV_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Write a single-shard package map under `cache_root` so the
    /// resolver's disk cache hits without going through the network.
    fn write_shard_fixture(
        cache_root: &std::path::Path,
        distro: &str,
        shard: &str,
        mappings: &[(&str, &str)],
    ) {
        let fixture = ChocoMapShard {
            metadata: ChocoMapMetadata {
                generated_at: "2026-05-21T00:00:00Z".to_string(),
                source: "chocolatey.org".to_string(),
                distro: distro.to_string(),
                shard: shard.to_string(),
                total_mappings: mappings.len() as u64,
            },
            mappings: mappings
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
        };
        let shard_dir = cache_root.join("package-maps-choco-v1").join(distro);
        std::fs::create_dir_all(&shard_dir).unwrap();
        std::fs::write(
            shard_dir.join(format!("{shard}.json")),
            serde_json::to_string(&fixture).unwrap(),
        )
        .unwrap();
    }

    /// Override the platform cache dir for the duration of a test by
    /// pointing `XDG_CACHE_HOME` (Linux/macOS) and `LOCALAPPDATA`
    /// (Windows) at a fresh tempdir. Returns the mutex guard (so the
    /// env is held exclusively for the test's lifetime), the tempdir
    /// (kept alive until drop), and the cache root.
    fn redirect_cache_dir() -> (
        std::sync::MutexGuard<'static, ()>,
        tempfile::TempDir,
        std::path::PathBuf,
    ) {
        // `lock()` may report poisoning if a prior test panicked while
        // holding it; the env vars themselves are safe to re-set so we
        // unwrap-or-into-inner to keep the test useful even after a
        // sibling failure.
        let guard = CACHE_ENV_GUARD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = tempfile::tempdir().unwrap();
        let cache_root = tmp.path().to_path_buf();
        // `dirs::cache_dir()` ignores `XDG_CACHE_HOME` on macOS/Windows, so set
        // the explicit override the resolver honors on every platform. The
        // XDG/LOCALAPPDATA sets are kept for any other code paths that read the
        // platform cache dir directly.
        std::env::set_var("ZLAYER_PACKAGE_MAP_CACHE_DIR", &cache_root);
        std::env::set_var("XDG_CACHE_HOME", &cache_root);
        std::env::set_var("LOCALAPPDATA", &cache_root);
        (guard, tmp, cache_root)
    }

    fn block_on<F: std::future::Future>(fut: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
            .block_on(fut)
    }

    #[test]
    fn detect_apt_install_in_run() {
        let parts = split_shell_subcommands("apt-get update && apt-get install -y curl git");
        assert_eq!(parts.len(), 2);
        assert!(is_package_manager_sync(&parts[0]));
        let detected = detect_install_in_subcommand(&parts[1])
            .expect("install sub-command must be recognised");
        assert_eq!(detected.0, DetectedPmKind::Apt);
        assert_eq!(detected.1, vec!["curl".to_string(), "git".to_string()]);
    }

    #[test]
    fn detect_yum_install_in_run() {
        let detected = detect_install_in_subcommand("yum install -y httpd")
            .expect("yum install -y httpd must be recognised");
        assert_eq!(detected.0, DetectedPmKind::YumOrDnf);
        assert_eq!(detected.1, vec!["httpd".to_string()]);

        let detected = detect_install_in_subcommand("dnf install -y nginx php-fpm")
            .expect("dnf install -y must be recognised");
        assert_eq!(detected.0, DetectedPmKind::YumOrDnf);
        assert_eq!(detected.1, vec!["nginx".to_string(), "php-fpm".to_string()]);
    }

    #[test]
    fn detect_apk_install_in_run() {
        let detected = detect_install_in_subcommand("apk add --no-cache nodejs npm")
            .expect("apk add must be recognised");
        assert_eq!(detected.0, DetectedPmKind::Apk);
        assert_eq!(detected.1, vec!["nodejs".to_string(), "npm".to_string()]);
    }

    #[test]
    fn detect_no_install_returns_none() {
        assert!(detect_install_in_subcommand("echo hello").is_none());
        assert!(detect_install_in_subcommand("ls /tmp").is_none());
        assert!(detect_install_in_subcommand("apt-getinstall -y curl").is_none());
        assert!(detect_install_in_subcommand("apt-get install -y").is_none());
        let parts = split_shell_subcommands("echo hello && ls /tmp");
        assert_eq!(parts.len(), 2);
        for p in &parts {
            assert!(detect_install_in_subcommand(p).is_none());
            assert!(!is_package_manager_sync(p));
        }
    }

    #[test]
    fn split_shell_subcommands_honours_and_and_semicolon() {
        let parts = split_shell_subcommands("a && b ; c");
        assert_eq!(
            parts,
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
    }

    #[test]
    fn split_shell_subcommands_drops_empty_segments() {
        let parts = split_shell_subcommands(" && a && ; b ;");
        assert_eq!(parts, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn is_package_manager_sync_matches_common_variants() {
        assert!(is_package_manager_sync("apt-get update"));
        assert!(is_package_manager_sync("apt update"));
        assert!(is_package_manager_sync("apk update"));
        assert!(is_package_manager_sync("yum check-update"));
        assert!(is_package_manager_sync("dnf makecache"));
        assert!(is_package_manager_sync("sudo apt-get update"));
        assert!(!is_package_manager_sync("apt-get install -y curl"));
        assert!(!is_package_manager_sync("echo hello"));
    }

    #[test]
    fn rejoin_emits_choco_install_for_install_subcommand() {
        let parts = vec![
            ShellSubcommand::Verbatim("echo before".to_string()),
            ShellSubcommand::PackageManagerSync,
            ShellSubcommand::Install {
                kind: DetectedPmKind::Apt,
                packages: vec!["curl".to_string(), "git".to_string()],
            },
            ShellSubcommand::Verbatim("echo after".to_string()),
        ];
        let out = rejoin_subcommands(&parts);
        assert_eq!(
            out,
            "echo before && choco install -y curl git && echo after"
        );
    }

    #[test]
    fn wrap_in_cmd_escapes_embedded_quotes() {
        let wrapped = wrap_in_cmd(r#"echo "hello""#);
        assert!(wrapped.starts_with("cmd /c \""));
        assert!(wrapped.contains(r#"\"hello\""#));
        assert!(wrapped.ends_with('"'));
    }

    #[test]
    fn translate_run_apt_to_choco_with_in_memory_shard() {
        // Build the shape the resolver writes to disk so the cache-hit
        // path runs without any network.
        let (_guard, _tmp, cache_root) = redirect_cache_dir();
        write_shard_fixture(
            &cache_root,
            "debian-12",
            "c",
            &[("curl", "curl"), ("linux-headers-generic", "__skip__")],
        );
        write_shard_fixture(
            &cache_root,
            "debian-12",
            "l",
            &[("linux-headers-generic", "__skip__")],
        );

        let translator = DockerfileTranslator::new(ImageOs::Windows);
        let (rewritten, skipped) = block_on(translator.translate_shell_command(
            "apt-get install -y curl linux-headers-generic",
            "debian-12",
            None,
        ))
        .expect("translate succeeds when every package resolves");
        assert!(
            rewritten.contains("choco install -y curl"),
            "rewritten command must include curl: {rewritten}"
        );
        assert!(
            !rewritten.contains("linux-headers-generic"),
            "skipped package must NOT appear in rewritten command: {rewritten}"
        );
        assert_eq!(skipped, vec!["linux-headers-generic".to_string()]);
    }

    // -----------------------------------------------------------------
    // Provisioned-toolchain skip behaviour (new in the move to
    // `DockerfileTranslator`).
    // -----------------------------------------------------------------

    #[test]
    fn translate_shell_command_skips_provisioned_toolchain() {
        // With a Go toolchain provisioned, an `apt-get install -y golang
        // git` must drop `golang` from the choco install and only emit
        // `git`. We seed the `g` shard with a `git → git` mapping so the
        // resolver hits without network.
        let (_guard, _tmp, cache_root) = redirect_cache_dir();
        write_shard_fixture(&cache_root, "debian-12", "g", &[("git", "git")]);

        let translator = DockerfileTranslator::new(ImageOs::Windows);
        let (rewritten, skipped) = block_on(translator.translate_shell_command(
            "apt-get install -y golang git",
            "debian-12",
            Some("go"),
        ))
        .expect("translate succeeds when remaining package resolves");
        assert!(
            !rewritten.contains("golang"),
            "provisioned-toolchain package must NOT appear in choco install: {rewritten}"
        );
        assert!(
            rewritten.contains("choco install -y git"),
            "non-toolchain package must still be installed: {rewritten}"
        );
        assert!(
            skipped.is_empty(),
            "toolchain drops are not reported as resolver-skipped: {skipped:?}"
        );
    }

    #[test]
    fn translate_shell_command_keeps_unrelated_pkg_with_toolchain() {
        // With a Go toolchain provisioned, an `apt-get install -y curl`
        // must still produce `choco install -y curl` — the toolchain
        // skip is only for packages that match the language.
        let (_guard, _tmp, cache_root) = redirect_cache_dir();
        write_shard_fixture(&cache_root, "debian-12", "c", &[("curl", "curl")]);

        let translator = DockerfileTranslator::new(ImageOs::Windows);
        let (rewritten, skipped) = block_on(translator.translate_shell_command(
            "apt-get install -y curl",
            "debian-12",
            Some("go"),
        ))
        .expect("translate succeeds");
        assert!(
            rewritten.contains("choco install -y curl"),
            "curl must still be installed: {rewritten}"
        );
        assert!(skipped.is_empty(), "no resolver-skipped: {skipped:?}");
    }

    #[test]
    fn translate_run_command_linux_is_passthrough() {
        // On Linux the translator must pass shell- and exec-form RUNs
        // through verbatim; no apt→choco rewriting happens because the
        // Linux backend hands the command straight to `/bin/sh -c`.
        let translator = DockerfileTranslator::new(ImageOs::Linux);
        let (shell_out, skipped) = block_on(translator.translate_run_command(
            &ShellOrExec::Shell("apt-get install -y curl".to_string()),
            "debian-12",
            None,
        ))
        .expect("Linux passthrough never fails");
        assert_eq!(shell_out, "apt-get install -y curl");
        assert!(skipped.is_empty());

        let (exec_out, skipped) = block_on(translator.translate_run_command(
            &ShellOrExec::Exec(vec!["echo".to_string(), "hi".to_string()]),
            "debian-12",
            None,
        ))
        .expect("Linux passthrough never fails");
        assert_eq!(exec_out, "echo hi");
        assert!(skipped.is_empty());
    }

    #[test]
    fn translate_run_command_windows_exec_is_passthrough() {
        // Exec-form on Windows joins with spaces and skips choco
        // detection — the caller has already chosen the absolute path
        // to the binary they want to invoke.
        let translator = DockerfileTranslator::new(ImageOs::Windows);
        let (out, skipped) = block_on(translator.translate_run_command(
            &ShellOrExec::Exec(vec![
                "C:\\app\\bin\\srv.exe".to_string(),
                "--port".to_string(),
                "80".to_string(),
            ]),
            "debian-12",
            None,
        ))
        .expect("exec-form passthrough never fails");
        assert_eq!(out, "C:\\app\\bin\\srv.exe --port 80");
        assert!(skipped.is_empty());
    }

    #[test]
    fn translate_shell_command_no_toolchain_installs_all() {
        // With no toolchain provisioned, `apt-get install -y golang git`
        // installs both packages via Chocolatey. `golang` resolves to
        // `golang` in the seeded shard (Chocolatey actually ships a
        // `golang` package, but the precise mapping is irrelevant here —
        // what matters is that it isn't dropped).
        let (_guard, _tmp, cache_root) = redirect_cache_dir();
        write_shard_fixture(
            &cache_root,
            "debian-12",
            "g",
            &[("golang", "golang"), ("git", "git")],
        );

        let translator = DockerfileTranslator::new(ImageOs::Windows);
        let (rewritten, skipped) = block_on(translator.translate_shell_command(
            "apt-get install -y golang git",
            "debian-12",
            None,
        ))
        .expect("translate succeeds");
        assert!(
            rewritten.contains("choco install -y golang git"),
            "both packages must be installed: {rewritten}"
        );
        assert!(skipped.is_empty(), "no resolver-skipped: {skipped:?}");
    }
}
