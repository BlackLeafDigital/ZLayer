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

use crate::dockerfile::{
    AddInstruction, CopyInstruction, EnvInstruction, ExposeInstruction, HealthcheckInstruction,
    Instruction, ShellOrExec,
};

use std::collections::HashMap;

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
    pub fn new(subcommand: &str) -> Self {
        Self {
            program: "buildah".to_string(),
            args: vec![subcommand.to_string()],
            env: HashMap::new(),
        }
    }

    /// Add an argument
    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// Add multiple arguments
    pub fn args(mut self, args: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    /// Add an optional argument (only added if value is Some)
    pub fn arg_opt(self, flag: &str, value: Option<impl Into<String>>) -> Self {
        if let Some(v) = value {
            self.arg(flag).arg(v)
        } else {
            self
        }
    }

    /// Add an environment variable for command execution
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }

    /// Convert to a command line string for display/logging
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
    pub fn from_image(image: &str) -> Self {
        Self::new("from").arg(image)
    }

    /// Create a new working container from an image with a specific name
    ///
    /// `buildah from --name <name> <image>`
    pub fn from_image_named(image: &str, name: &str) -> Self {
        Self::new("from").arg("--name").arg(name).arg(image)
    }

    /// Create a scratch container
    ///
    /// `buildah from scratch`
    pub fn from_scratch() -> Self {
        Self::new("from").arg("scratch")
    }

    /// Remove a working container
    ///
    /// `buildah rm <container>`
    pub fn rm(container: &str) -> Self {
        Self::new("rm").arg(container)
    }

    /// Commit a container to create an image
    ///
    /// `buildah commit <container> <image>`
    pub fn commit(container: &str, image_name: &str) -> Self {
        Self::new("commit").arg(container).arg(image_name)
    }

    /// Commit with additional options
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
    pub fn tag(image: &str, new_name: &str) -> Self {
        Self::new("tag").arg(image).arg(new_name)
    }

    /// Remove an image
    ///
    /// `buildah rmi <image>`
    pub fn rmi(image: &str) -> Self {
        Self::new("rmi").arg(image)
    }

    /// Push an image to a registry
    ///
    /// `buildah push <image>`
    pub fn push(image: &str) -> Self {
        Self::new("push").arg(image)
    }

    /// Push an image to a registry with options
    ///
    /// `buildah push [options] <image> [destination]`
    pub fn push_to(image: &str, destination: &str) -> Self {
        Self::new("push").arg(image).arg(destination)
    }

    /// Inspect an image or container
    ///
    /// `buildah inspect <name>`
    pub fn inspect(name: &str) -> Self {
        Self::new("inspect").arg(name)
    }

    /// Inspect an image or container with format
    ///
    /// `buildah inspect --format <format> <name>`
    pub fn inspect_format(name: &str, format: &str) -> Self {
        Self::new("inspect").arg("--format").arg(format).arg(name)
    }

    /// List images
    ///
    /// `buildah images`
    pub fn images() -> Self {
        Self::new("images")
    }

    /// List containers
    ///
    /// `buildah containers`
    pub fn containers() -> Self {
        Self::new("containers")
    }

    // =========================================================================
    // Run Commands
    // =========================================================================

    /// Run a command in the container (shell form)
    ///
    /// `buildah run <container> -- /bin/sh -c "<command>"`
    pub fn run_shell(container: &str, command: &str) -> Self {
        Self::new("run")
            .arg(container)
            .arg("--")
            .arg("/bin/sh")
            .arg("-c")
            .arg(command)
    }

    /// Run a command in the container (exec form)
    ///
    /// `buildah run <container> -- <args...>`
    pub fn run_exec(container: &str, args: &[String]) -> Self {
        let mut cmd = Self::new("run").arg(container).arg("--");
        for arg in args {
            cmd = cmd.arg(arg);
        }
        cmd
    }

    /// Run a command based on ShellOrExec
    pub fn run(container: &str, command: &ShellOrExec) -> Self {
        match command {
            ShellOrExec::Shell(s) => Self::run_shell(container, s),
            ShellOrExec::Exec(args) => Self::run_exec(container, args),
        }
    }

    // =========================================================================
    // Copy/Add Commands
    // =========================================================================

    /// Copy files into the container
    ///
    /// `buildah copy <container> <src...> <dest>`
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
    pub fn copy_from(container: &str, from: &str, sources: &[String], dest: &str) -> Self {
        let mut cmd = Self::new("copy").arg("--from").arg(from).arg(container);
        for src in sources {
            cmd = cmd.arg(src);
        }
        cmd.arg(dest)
    }

    /// Copy with all options from CopyInstruction
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
    pub fn add(container: &str, sources: &[String], dest: &str) -> Self {
        let mut cmd = Self::new("add").arg(container);
        for src in sources {
            cmd = cmd.arg(src);
        }
        cmd.arg(dest)
    }

    /// Add with all options from AddInstruction
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
    pub fn config_env(container: &str, key: &str, value: &str) -> Self {
        Self::new("config")
            .arg("--env")
            .arg(format!("{}={}", key, value))
            .arg(container)
    }

    /// Set multiple environment variables
    pub fn config_envs(container: &str, env: &EnvInstruction) -> Vec<Self> {
        env.vars
            .iter()
            .map(|(k, v)| Self::config_env(container, k, v))
            .collect()
    }

    /// Set the working directory
    ///
    /// `buildah config --workingdir <dir> <container>`
    pub fn config_workdir(container: &str, dir: &str) -> Self {
        Self::new("config")
            .arg("--workingdir")
            .arg(dir)
            .arg(container)
    }

    /// Expose a port
    ///
    /// `buildah config --port <port>/<proto> <container>`
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

    /// Set the entrypoint based on ShellOrExec
    pub fn config_entrypoint(container: &str, command: &ShellOrExec) -> Self {
        match command {
            ShellOrExec::Shell(s) => Self::config_entrypoint_shell(container, s),
            ShellOrExec::Exec(args) => Self::config_entrypoint_exec(container, args),
        }
    }

    /// Set the default command (shell form)
    pub fn config_cmd_shell(container: &str, command: &str) -> Self {
        Self::new("config")
            .arg("--cmd")
            .arg(format!("/bin/sh -c \"{}\"", escape_json_string(command)))
            .arg(container)
    }

    /// Set the default command (exec form)
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

    /// Set the default command based on ShellOrExec
    pub fn config_cmd(container: &str, command: &ShellOrExec) -> Self {
        match command {
            ShellOrExec::Shell(s) => Self::config_cmd_shell(container, s),
            ShellOrExec::Exec(args) => Self::config_cmd_exec(container, args),
        }
    }

    /// Set the user
    ///
    /// `buildah config --user <user> <container>`
    pub fn config_user(container: &str, user: &str) -> Self {
        Self::new("config").arg("--user").arg(user).arg(container)
    }

    /// Set a label
    ///
    /// `buildah config --label KEY=VALUE <container>`
    pub fn config_label(container: &str, key: &str, value: &str) -> Self {
        Self::new("config")
            .arg("--label")
            .arg(format!("{}={}", key, value))
            .arg(container)
    }

    /// Set multiple labels
    pub fn config_labels(container: &str, labels: &HashMap<String, String>) -> Vec<Self> {
        labels
            .iter()
            .map(|(k, v)| Self::config_label(container, k, v))
            .collect()
    }

    /// Set volumes
    ///
    /// `buildah config --volume <path> <container>`
    pub fn config_volume(container: &str, path: &str) -> Self {
        Self::new("config").arg("--volume").arg(path).arg(container)
    }

    /// Set the stop signal
    ///
    /// `buildah config --stop-signal <signal> <container>`
    pub fn config_stopsignal(container: &str, signal: &str) -> Self {
        Self::new("config")
            .arg("--stop-signal")
            .arg(signal)
            .arg(container)
    }

    /// Set the shell
    ///
    /// `buildah config --shell '["shell", "args"]' <container>`
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
                    ShellOrExec::Shell(s) => format!("CMD {}", s),
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
    // Convert Instruction to Commands
    // =========================================================================

    /// Convert a Dockerfile instruction to buildah command(s)
    ///
    /// Some instructions map to multiple buildah commands (e.g., multiple ENV vars)
    pub fn from_instruction(container: &str, instruction: &Instruction) -> Vec<Self> {
        match instruction {
            Instruction::Run(run) => {
                vec![Self::run(container, &run.command)]
            }

            Instruction::Copy(copy) => {
                vec![Self::copy_instruction(container, copy)]
            }

            Instruction::Add(add) => {
                vec![Self::add_instruction(container, add)]
            }

            Instruction::Env(env) => Self::config_envs(container, env),

            Instruction::Workdir(dir) => {
                vec![Self::config_workdir(container, dir)]
            }

            Instruction::Expose(expose) => {
                vec![Self::config_expose(container, expose)]
            }

            Instruction::Label(labels) => Self::config_labels(container, labels),

            Instruction::User(user) => {
                vec![Self::config_user(container, user)]
            }

            Instruction::Entrypoint(cmd) => {
                vec![Self::config_entrypoint(container, cmd)]
            }

            Instruction::Cmd(cmd) => {
                vec![Self::config_cmd(container, cmd)]
            }

            Instruction::Volume(paths) => paths
                .iter()
                .map(|p| Self::config_volume(container, p))
                .collect(),

            Instruction::Shell(shell) => {
                vec![Self::config_shell(container, shell)]
            }

            Instruction::Arg(_) => {
                // ARG is handled during variable expansion, not as a buildah command
                vec![]
            }

            Instruction::Stopsignal(signal) => {
                vec![Self::config_stopsignal(container, signal)]
            }

            Instruction::Healthcheck(hc) => {
                vec![Self::config_healthcheck(container, hc)]
            }

            Instruction::Onbuild(_) => {
                // ONBUILD would need special handling
                tracing::warn!("ONBUILD instruction not supported in buildah conversion");
                vec![]
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
            .any(|a| a.contains("[") && a.contains("/app")));
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
}
