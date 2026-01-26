//! Dockerfile instruction types
//!
//! This module defines the internal representation of Dockerfile instructions,
//! providing a clean, typed interface for working with parsed Dockerfiles.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Represents a command that can be in shell form or exec form
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShellOrExec {
    /// Shell form: `CMD command param1 param2`
    /// Executed as `/bin/sh -c "command param1 param2"`
    Shell(String),

    /// Exec form: `CMD ["executable", "param1", "param2"]`
    /// Executed directly without shell interpretation
    Exec(Vec<String>),
}

impl ShellOrExec {
    /// Returns true if this is the shell form
    pub fn is_shell(&self) -> bool {
        matches!(self, Self::Shell(_))
    }

    /// Returns true if this is the exec form
    pub fn is_exec(&self) -> bool {
        matches!(self, Self::Exec(_))
    }

    /// Convert to a vector of strings suitable for execution
    pub fn to_exec_args(&self, shell: &[String]) -> Vec<String> {
        match self {
            Self::Shell(cmd) => {
                let mut args = shell.to_vec();
                args.push(cmd.clone());
                args
            }
            Self::Exec(args) => args.clone(),
        }
    }
}

impl Default for ShellOrExec {
    fn default() -> Self {
        Self::Exec(Vec::new())
    }
}

/// A single Dockerfile instruction
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Instruction {
    /// RUN instruction - executes a command and commits the result as a new layer
    Run(RunInstruction),

    /// COPY instruction - copies files from build context or previous stage
    Copy(CopyInstruction),

    /// ADD instruction - like COPY but with URL support and auto-extraction
    Add(AddInstruction),

    /// ENV instruction - sets environment variables
    Env(EnvInstruction),

    /// WORKDIR instruction - sets the working directory
    Workdir(String),

    /// EXPOSE instruction - documents which ports the container listens on
    Expose(ExposeInstruction),

    /// LABEL instruction - adds metadata to the image
    Label(HashMap<String, String>),

    /// USER instruction - sets the user for subsequent instructions
    User(String),

    /// ENTRYPOINT instruction - configures container as executable
    Entrypoint(ShellOrExec),

    /// CMD instruction - provides defaults for container execution
    Cmd(ShellOrExec),

    /// VOLUME instruction - creates mount points
    Volume(Vec<String>),

    /// SHELL instruction - changes the default shell
    Shell(Vec<String>),

    /// ARG instruction - defines build-time variables
    Arg(ArgInstruction),

    /// STOPSIGNAL instruction - sets the signal for stopping the container
    Stopsignal(String),

    /// HEALTHCHECK instruction - defines how to check container health
    Healthcheck(HealthcheckInstruction),

    /// ONBUILD instruction - adds trigger for downstream builds
    Onbuild(Box<Instruction>),
}

impl Instruction {
    /// Returns the instruction name as it would appear in a Dockerfile
    pub fn name(&self) -> &'static str {
        match self {
            Self::Run(_) => "RUN",
            Self::Copy(_) => "COPY",
            Self::Add(_) => "ADD",
            Self::Env(_) => "ENV",
            Self::Workdir(_) => "WORKDIR",
            Self::Expose(_) => "EXPOSE",
            Self::Label(_) => "LABEL",
            Self::User(_) => "USER",
            Self::Entrypoint(_) => "ENTRYPOINT",
            Self::Cmd(_) => "CMD",
            Self::Volume(_) => "VOLUME",
            Self::Shell(_) => "SHELL",
            Self::Arg(_) => "ARG",
            Self::Stopsignal(_) => "STOPSIGNAL",
            Self::Healthcheck(_) => "HEALTHCHECK",
            Self::Onbuild(_) => "ONBUILD",
        }
    }

    /// Returns true if this instruction creates a new layer
    pub fn creates_layer(&self) -> bool {
        matches!(self, Self::Run(_) | Self::Copy(_) | Self::Add(_))
    }
}

/// RUN instruction with optional BuildKit features
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunInstruction {
    /// The command to run
    pub command: ShellOrExec,

    /// Optional mount specifications (BuildKit)
    pub mounts: Vec<RunMount>,

    /// Optional network mode (BuildKit)
    pub network: Option<RunNetwork>,

    /// Optional security mode (BuildKit)
    pub security: Option<RunSecurity>,
}

impl RunInstruction {
    /// Create a new RUN instruction from a shell command
    pub fn shell(cmd: impl Into<String>) -> Self {
        Self {
            command: ShellOrExec::Shell(cmd.into()),
            mounts: Vec::new(),
            network: None,
            security: None,
        }
    }

    /// Create a new RUN instruction from exec form
    pub fn exec(args: Vec<String>) -> Self {
        Self {
            command: ShellOrExec::Exec(args),
            mounts: Vec::new(),
            network: None,
            security: None,
        }
    }
}

/// Mount types for RUN --mount
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunMount {
    /// Bind mount from build context or another stage
    Bind {
        target: String,
        source: Option<String>,
        from: Option<String>,
        readonly: bool,
    },
    /// Cache mount for build caches (e.g., package managers)
    Cache {
        target: String,
        id: Option<String>,
        sharing: CacheSharing,
        readonly: bool,
    },
    /// Tmpfs mount
    Tmpfs { target: String, size: Option<u64> },
    /// Secret mount
    Secret {
        target: String,
        id: String,
        required: bool,
    },
    /// SSH mount for SSH agent forwarding
    Ssh {
        target: String,
        id: Option<String>,
        required: bool,
    },
}

/// Cache sharing mode for RUN --mount=type=cache
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CacheSharing {
    /// Only one build can access at a time
    #[default]
    Locked,
    /// Multiple builds can access, last write wins
    Shared,
    /// Each build gets a private copy
    Private,
}

/// Network mode for RUN --network
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunNetwork {
    /// Use default network
    Default,
    /// No network access
    None,
    /// Use host network
    Host,
}

/// Security mode for RUN --security
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunSecurity {
    /// Default security
    Sandbox,
    /// Insecure mode (privileged)
    Insecure,
}

/// COPY instruction
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopyInstruction {
    /// Source paths (relative to context or stage)
    pub sources: Vec<String>,

    /// Destination path in the image
    pub destination: String,

    /// Source stage for multi-stage builds (--from)
    pub from: Option<String>,

    /// Change ownership (--chown)
    pub chown: Option<String>,

    /// Change permissions (--chmod)
    pub chmod: Option<String>,

    /// Create hardlink instead of copying (--link)
    pub link: bool,

    /// Exclude patterns (--exclude)
    pub exclude: Vec<String>,
}

impl CopyInstruction {
    /// Create a new COPY instruction
    pub fn new(sources: Vec<String>, destination: String) -> Self {
        Self {
            sources,
            destination,
            from: None,
            chown: None,
            chmod: None,
            link: false,
            exclude: Vec::new(),
        }
    }

    /// Set the source stage
    pub fn from_stage(mut self, stage: impl Into<String>) -> Self {
        self.from = Some(stage.into());
        self
    }

    /// Set ownership
    pub fn chown(mut self, owner: impl Into<String>) -> Self {
        self.chown = Some(owner.into());
        self
    }
}

/// ADD instruction (similar to COPY but with URL and archive support)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddInstruction {
    /// Source paths or URLs
    pub sources: Vec<String>,

    /// Destination path in the image
    pub destination: String,

    /// Change ownership (--chown)
    pub chown: Option<String>,

    /// Change permissions (--chmod)
    pub chmod: Option<String>,

    /// Create hardlink instead of copying (--link)
    pub link: bool,

    /// Checksum to verify remote URLs (--checksum)
    pub checksum: Option<String>,

    /// Keep UID/GID from archive (--keep-git-dir)
    pub keep_git_dir: bool,
}

impl AddInstruction {
    /// Create a new ADD instruction
    pub fn new(sources: Vec<String>, destination: String) -> Self {
        Self {
            sources,
            destination,
            chown: None,
            chmod: None,
            link: false,
            checksum: None,
            keep_git_dir: false,
        }
    }
}

/// ENV instruction
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvInstruction {
    /// Environment variables to set
    pub vars: HashMap<String, String>,
}

impl EnvInstruction {
    /// Create a new ENV instruction with a single variable
    pub fn new(key: impl Into<String>, value: impl Into<String>) -> Self {
        let mut vars = HashMap::new();
        vars.insert(key.into(), value.into());
        Self { vars }
    }

    /// Create a new ENV instruction with multiple variables
    pub fn from_vars(vars: HashMap<String, String>) -> Self {
        Self { vars }
    }
}

/// EXPOSE instruction
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExposeInstruction {
    /// Port to expose
    pub port: u16,

    /// Protocol (tcp or udp)
    pub protocol: ExposeProtocol,
}

impl ExposeInstruction {
    /// Create a new TCP EXPOSE instruction
    pub fn tcp(port: u16) -> Self {
        Self {
            port,
            protocol: ExposeProtocol::Tcp,
        }
    }

    /// Create a new UDP EXPOSE instruction
    pub fn udp(port: u16) -> Self {
        Self {
            port,
            protocol: ExposeProtocol::Udp,
        }
    }
}

/// Protocol for EXPOSE instruction
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ExposeProtocol {
    #[default]
    Tcp,
    Udp,
}

/// ARG instruction
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArgInstruction {
    /// Argument name
    pub name: String,

    /// Default value (if any)
    pub default: Option<String>,
}

impl ArgInstruction {
    /// Create a new ARG instruction
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            default: None,
        }
    }

    /// Create a new ARG instruction with a default value
    pub fn with_default(name: impl Into<String>, default: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            default: Some(default.into()),
        }
    }
}

/// HEALTHCHECK instruction
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HealthcheckInstruction {
    /// Disable healthcheck inherited from base image
    None,

    /// Configure healthcheck
    Check {
        /// Command to run for health check
        command: ShellOrExec,

        /// Interval between checks
        interval: Option<std::time::Duration>,

        /// Timeout for each check
        timeout: Option<std::time::Duration>,

        /// Start period before first check
        start_period: Option<std::time::Duration>,

        /// Start interval during start period
        start_interval: Option<std::time::Duration>,

        /// Number of retries before unhealthy
        retries: Option<u32>,
    },
}

impl HealthcheckInstruction {
    /// Create a new CMD-style healthcheck
    pub fn cmd(command: ShellOrExec) -> Self {
        Self::Check {
            command,
            interval: None,
            timeout: None,
            start_period: None,
            start_interval: None,
            retries: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shell_or_exec() {
        let shell = ShellOrExec::Shell("echo hello".to_string());
        assert!(shell.is_shell());
        assert!(!shell.is_exec());

        let exec = ShellOrExec::Exec(vec!["echo".to_string(), "hello".to_string()]);
        assert!(exec.is_exec());
        assert!(!exec.is_shell());
    }

    #[test]
    fn test_shell_to_exec_args() {
        let shell = ShellOrExec::Shell("echo hello".to_string());
        let default_shell = vec!["/bin/sh".to_string(), "-c".to_string()];
        let args = shell.to_exec_args(&default_shell);
        assert_eq!(
            args,
            vec!["/bin/sh", "-c", "echo hello"]
                .into_iter()
                .map(String::from)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_instruction_names() {
        assert_eq!(
            Instruction::Run(RunInstruction::shell("test")).name(),
            "RUN"
        );
        assert_eq!(
            Instruction::Copy(CopyInstruction::new(vec![], ".".into())).name(),
            "COPY"
        );
        assert_eq!(Instruction::Workdir("/app".into()).name(), "WORKDIR");
    }

    #[test]
    fn test_creates_layer() {
        assert!(Instruction::Run(RunInstruction::shell("test")).creates_layer());
        assert!(Instruction::Copy(CopyInstruction::new(vec![], ".".into())).creates_layer());
        assert!(!Instruction::Env(EnvInstruction::new("KEY", "value")).creates_layer());
        assert!(!Instruction::Workdir("/app".into()).creates_layer());
    }

    #[test]
    fn test_copy_instruction_builder() {
        let copy = CopyInstruction::new(vec!["src".into()], "/app".into())
            .from_stage("builder")
            .chown("1000:1000");

        assert_eq!(copy.from, Some("builder".to_string()));
        assert_eq!(copy.chown, Some("1000:1000".to_string()));
    }

    #[test]
    fn test_arg_instruction() {
        let arg = ArgInstruction::new("VERSION");
        assert_eq!(arg.name, "VERSION");
        assert!(arg.default.is_none());

        let arg_with_default = ArgInstruction::with_default("VERSION", "1.0");
        assert_eq!(arg_with_default.default, Some("1.0".to_string()));
    }
}
