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

    /// Generate a cache key for this instruction.
    ///
    /// The key uniquely identifies the instruction and its parameters,
    /// enabling cache hit detection when combined with the base layer digest.
    ///
    /// # Returns
    ///
    /// A 16-character hexadecimal string representing the hash of the instruction.
    ///
    /// # Example
    ///
    /// ```
    /// use zlayer_builder::dockerfile::Instruction;
    /// use zlayer_builder::dockerfile::RunInstruction;
    ///
    /// let run = Instruction::Run(RunInstruction::shell("echo hello"));
    /// let key = run.cache_key();
    /// assert_eq!(key.len(), 16);
    /// ```
    pub fn cache_key(&self) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();

        match self {
            Self::Run(run) => {
                "RUN".hash(&mut hasher);
                // Hash the command representation
                match &run.command {
                    ShellOrExec::Shell(s) => {
                        "shell".hash(&mut hasher);
                        s.hash(&mut hasher);
                    }
                    ShellOrExec::Exec(args) => {
                        "exec".hash(&mut hasher);
                        args.hash(&mut hasher);
                    }
                }
                // Include mounts in the hash - they affect layer content
                for mount in &run.mounts {
                    format!("{:?}", mount).hash(&mut hasher);
                }
                // Include network mode if set
                if let Some(network) = &run.network {
                    format!("{:?}", network).hash(&mut hasher);
                }
                // Include security mode if set
                if let Some(security) = &run.security {
                    format!("{:?}", security).hash(&mut hasher);
                }
            }
            Self::Copy(copy) => {
                "COPY".hash(&mut hasher);
                copy.sources.hash(&mut hasher);
                copy.destination.hash(&mut hasher);
                copy.from.hash(&mut hasher);
                copy.chown.hash(&mut hasher);
                copy.chmod.hash(&mut hasher);
                copy.link.hash(&mut hasher);
                copy.exclude.hash(&mut hasher);
            }
            Self::Add(add) => {
                "ADD".hash(&mut hasher);
                add.sources.hash(&mut hasher);
                add.destination.hash(&mut hasher);
                add.chown.hash(&mut hasher);
                add.chmod.hash(&mut hasher);
                add.link.hash(&mut hasher);
                add.checksum.hash(&mut hasher);
                add.keep_git_dir.hash(&mut hasher);
            }
            Self::Env(env) => {
                "ENV".hash(&mut hasher);
                // Sort keys for deterministic hashing
                let mut keys: Vec<_> = env.vars.keys().collect();
                keys.sort();
                for key in keys {
                    key.hash(&mut hasher);
                    env.vars.get(key).hash(&mut hasher);
                }
            }
            Self::Workdir(path) => {
                "WORKDIR".hash(&mut hasher);
                path.hash(&mut hasher);
            }
            Self::Expose(expose) => {
                "EXPOSE".hash(&mut hasher);
                expose.port.hash(&mut hasher);
                format!("{:?}", expose.protocol).hash(&mut hasher);
            }
            Self::Label(labels) => {
                "LABEL".hash(&mut hasher);
                // Sort keys for deterministic hashing
                let mut keys: Vec<_> = labels.keys().collect();
                keys.sort();
                for key in keys {
                    key.hash(&mut hasher);
                    labels.get(key).hash(&mut hasher);
                }
            }
            Self::User(user) => {
                "USER".hash(&mut hasher);
                user.hash(&mut hasher);
            }
            Self::Entrypoint(cmd) => {
                "ENTRYPOINT".hash(&mut hasher);
                match cmd {
                    ShellOrExec::Shell(s) => {
                        "shell".hash(&mut hasher);
                        s.hash(&mut hasher);
                    }
                    ShellOrExec::Exec(args) => {
                        "exec".hash(&mut hasher);
                        args.hash(&mut hasher);
                    }
                }
            }
            Self::Cmd(cmd) => {
                "CMD".hash(&mut hasher);
                match cmd {
                    ShellOrExec::Shell(s) => {
                        "shell".hash(&mut hasher);
                        s.hash(&mut hasher);
                    }
                    ShellOrExec::Exec(args) => {
                        "exec".hash(&mut hasher);
                        args.hash(&mut hasher);
                    }
                }
            }
            Self::Volume(paths) => {
                "VOLUME".hash(&mut hasher);
                paths.hash(&mut hasher);
            }
            Self::Shell(shell) => {
                "SHELL".hash(&mut hasher);
                shell.hash(&mut hasher);
            }
            Self::Arg(arg) => {
                "ARG".hash(&mut hasher);
                arg.name.hash(&mut hasher);
                arg.default.hash(&mut hasher);
            }
            Self::Stopsignal(signal) => {
                "STOPSIGNAL".hash(&mut hasher);
                signal.hash(&mut hasher);
            }
            Self::Healthcheck(health) => {
                "HEALTHCHECK".hash(&mut hasher);
                match health {
                    HealthcheckInstruction::None => {
                        "none".hash(&mut hasher);
                    }
                    HealthcheckInstruction::Check {
                        command,
                        interval,
                        timeout,
                        start_period,
                        start_interval,
                        retries,
                    } => {
                        "check".hash(&mut hasher);
                        match command {
                            ShellOrExec::Shell(s) => {
                                "shell".hash(&mut hasher);
                                s.hash(&mut hasher);
                            }
                            ShellOrExec::Exec(args) => {
                                "exec".hash(&mut hasher);
                                args.hash(&mut hasher);
                            }
                        }
                        interval.map(|d| d.as_nanos()).hash(&mut hasher);
                        timeout.map(|d| d.as_nanos()).hash(&mut hasher);
                        start_period.map(|d| d.as_nanos()).hash(&mut hasher);
                        start_interval.map(|d| d.as_nanos()).hash(&mut hasher);
                        retries.hash(&mut hasher);
                    }
                }
            }
            Self::Onbuild(inner) => {
                "ONBUILD".hash(&mut hasher);
                // Recursively hash the inner instruction
                inner.cache_key().hash(&mut hasher);
            }
        }

        format!("{:016x}", hasher.finish())
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

impl RunMount {
    /// Convert the mount specification to a buildah `--mount` argument string.
    ///
    /// Returns a string in the format `type=<type>,<option>=<value>,...`
    /// suitable for use with `buildah run --mount=<result>`.
    ///
    /// # Examples
    ///
    /// ```
    /// use zlayer_builder::dockerfile::{RunMount, CacheSharing};
    ///
    /// let cache_mount = RunMount::Cache {
    ///     target: "/var/cache/apt".to_string(),
    ///     id: Some("apt-cache".to_string()),
    ///     sharing: CacheSharing::Shared,
    ///     readonly: false,
    /// };
    /// assert_eq!(
    ///     cache_mount.to_buildah_arg(),
    ///     "type=cache,target=/var/cache/apt,id=apt-cache,sharing=shared"
    /// );
    /// ```
    pub fn to_buildah_arg(&self) -> String {
        match self {
            Self::Bind {
                target,
                source,
                from,
                readonly,
            } => {
                let mut parts = vec![format!("type=bind,target={}", target)];
                if let Some(src) = source {
                    parts.push(format!("source={}", src));
                }
                if let Some(from_stage) = from {
                    parts.push(format!("from={}", from_stage));
                }
                if *readonly {
                    parts.push("ro".to_string());
                }
                parts.join(",")
            }
            Self::Cache {
                target,
                id,
                sharing,
                readonly,
            } => {
                let mut parts = vec![format!("type=cache,target={}", target)];
                if let Some(cache_id) = id {
                    parts.push(format!("id={}", cache_id));
                }
                // Only add sharing if not the default (locked)
                if *sharing != CacheSharing::Locked {
                    parts.push(format!("sharing={}", sharing.as_str()));
                }
                if *readonly {
                    parts.push("ro".to_string());
                }
                parts.join(",")
            }
            Self::Tmpfs { target, size } => {
                let mut parts = vec![format!("type=tmpfs,target={}", target)];
                if let Some(sz) = size {
                    parts.push(format!("tmpfs-size={}", sz));
                }
                parts.join(",")
            }
            Self::Secret {
                target,
                id,
                required,
            } => {
                let mut parts = vec![format!("type=secret,id={}", id)];
                // Only add target if it's not empty (some secrets use default paths)
                if !target.is_empty() {
                    parts.push(format!("target={}", target));
                }
                if *required {
                    parts.push("required".to_string());
                }
                parts.join(",")
            }
            Self::Ssh {
                target,
                id,
                required,
            } => {
                let mut parts = vec!["type=ssh".to_string()];
                if let Some(ssh_id) = id {
                    parts.push(format!("id={}", ssh_id));
                }
                if !target.is_empty() {
                    parts.push(format!("target={}", target));
                }
                if *required {
                    parts.push("required".to_string());
                }
                parts.join(",")
            }
        }
    }
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

impl CacheSharing {
    /// Returns the string representation for buildah mount arguments
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Locked => "locked",
            Self::Shared => "shared",
            Self::Private => "private",
        }
    }
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

    #[test]
    fn test_cache_sharing_as_str() {
        assert_eq!(CacheSharing::Locked.as_str(), "locked");
        assert_eq!(CacheSharing::Shared.as_str(), "shared");
        assert_eq!(CacheSharing::Private.as_str(), "private");
    }

    #[test]
    fn test_run_mount_cache_to_buildah_arg() {
        let mount = RunMount::Cache {
            target: "/var/cache/apt".to_string(),
            id: Some("apt-cache".to_string()),
            sharing: CacheSharing::Shared,
            readonly: false,
        };
        assert_eq!(
            mount.to_buildah_arg(),
            "type=cache,target=/var/cache/apt,id=apt-cache,sharing=shared"
        );

        // Test with default sharing (locked) - should not include sharing
        let mount_locked = RunMount::Cache {
            target: "/cache".to_string(),
            id: None,
            sharing: CacheSharing::Locked,
            readonly: false,
        };
        assert_eq!(mount_locked.to_buildah_arg(), "type=cache,target=/cache");

        // Test readonly
        let mount_ro = RunMount::Cache {
            target: "/cache".to_string(),
            id: Some("mycache".to_string()),
            sharing: CacheSharing::Locked,
            readonly: true,
        };
        assert_eq!(
            mount_ro.to_buildah_arg(),
            "type=cache,target=/cache,id=mycache,ro"
        );
    }

    #[test]
    fn test_run_mount_bind_to_buildah_arg() {
        let mount = RunMount::Bind {
            target: "/app".to_string(),
            source: Some("/src".to_string()),
            from: Some("builder".to_string()),
            readonly: true,
        };
        assert_eq!(
            mount.to_buildah_arg(),
            "type=bind,target=/app,source=/src,from=builder,ro"
        );

        // Minimal bind mount
        let mount_minimal = RunMount::Bind {
            target: "/app".to_string(),
            source: None,
            from: None,
            readonly: false,
        };
        assert_eq!(mount_minimal.to_buildah_arg(), "type=bind,target=/app");
    }

    #[test]
    fn test_run_mount_tmpfs_to_buildah_arg() {
        let mount = RunMount::Tmpfs {
            target: "/tmp".to_string(),
            size: Some(1048576),
        };
        assert_eq!(
            mount.to_buildah_arg(),
            "type=tmpfs,target=/tmp,tmpfs-size=1048576"
        );

        let mount_no_size = RunMount::Tmpfs {
            target: "/tmp".to_string(),
            size: None,
        };
        assert_eq!(mount_no_size.to_buildah_arg(), "type=tmpfs,target=/tmp");
    }

    #[test]
    fn test_run_mount_secret_to_buildah_arg() {
        let mount = RunMount::Secret {
            target: "/run/secrets/mysecret".to_string(),
            id: "mysecret".to_string(),
            required: true,
        };
        assert_eq!(
            mount.to_buildah_arg(),
            "type=secret,id=mysecret,target=/run/secrets/mysecret,required"
        );

        // Without target (uses default)
        let mount_no_target = RunMount::Secret {
            target: String::new(),
            id: "mysecret".to_string(),
            required: false,
        };
        assert_eq!(mount_no_target.to_buildah_arg(), "type=secret,id=mysecret");
    }

    #[test]
    fn test_run_mount_ssh_to_buildah_arg() {
        let mount = RunMount::Ssh {
            target: "/root/.ssh".to_string(),
            id: Some("default".to_string()),
            required: true,
        };
        assert_eq!(
            mount.to_buildah_arg(),
            "type=ssh,id=default,target=/root/.ssh,required"
        );

        // Minimal SSH mount
        let mount_minimal = RunMount::Ssh {
            target: String::new(),
            id: None,
            required: false,
        };
        assert_eq!(mount_minimal.to_buildah_arg(), "type=ssh");
    }

    #[test]
    fn test_cache_key_length() {
        // All cache keys should be 16 hex characters
        let run = Instruction::Run(RunInstruction::shell("echo hello"));
        assert_eq!(run.cache_key().len(), 16);

        let copy = Instruction::Copy(CopyInstruction::new(vec!["src".into()], "/app".into()));
        assert_eq!(copy.cache_key().len(), 16);

        let workdir = Instruction::Workdir("/app".into());
        assert_eq!(workdir.cache_key().len(), 16);
    }

    #[test]
    fn test_cache_key_deterministic() {
        // Same instruction should produce the same cache key
        let run1 = Instruction::Run(RunInstruction::shell("apt-get update"));
        let run2 = Instruction::Run(RunInstruction::shell("apt-get update"));
        assert_eq!(run1.cache_key(), run2.cache_key());

        let copy1 = Instruction::Copy(CopyInstruction::new(vec!["src".into()], "/app".into()));
        let copy2 = Instruction::Copy(CopyInstruction::new(vec!["src".into()], "/app".into()));
        assert_eq!(copy1.cache_key(), copy2.cache_key());
    }

    #[test]
    fn test_cache_key_different_for_different_instructions() {
        // Different instructions should produce different cache keys
        let run = Instruction::Run(RunInstruction::shell("echo hello"));
        let workdir = Instruction::Workdir("echo hello".into());
        assert_ne!(run.cache_key(), workdir.cache_key());

        // Different commands should produce different cache keys
        let run1 = Instruction::Run(RunInstruction::shell("apt-get update"));
        let run2 = Instruction::Run(RunInstruction::shell("apt-get upgrade"));
        assert_ne!(run1.cache_key(), run2.cache_key());

        // Same sources, different destinations
        let copy1 = Instruction::Copy(CopyInstruction::new(vec!["src".into()], "/app".into()));
        let copy2 = Instruction::Copy(CopyInstruction::new(vec!["src".into()], "/opt".into()));
        assert_ne!(copy1.cache_key(), copy2.cache_key());
    }

    #[test]
    fn test_cache_key_shell_vs_exec() {
        // Shell form vs exec form should produce different keys even with similar commands
        let shell = Instruction::Run(RunInstruction::shell("echo hello"));
        let exec = Instruction::Run(RunInstruction::exec(vec![
            "echo".to_string(),
            "hello".to_string(),
        ]));
        assert_ne!(shell.cache_key(), exec.cache_key());
    }

    #[test]
    fn test_cache_key_with_mounts() {
        // RUN with mounts should differ from RUN without mounts
        let run_no_mount = Instruction::Run(RunInstruction::shell("apt-get install -y curl"));

        let mut run_with_mount = RunInstruction::shell("apt-get install -y curl");
        run_with_mount.mounts = vec![RunMount::Cache {
            target: "/var/cache/apt".to_string(),
            id: Some("apt-cache".to_string()),
            sharing: CacheSharing::Shared,
            readonly: false,
        }];
        let run_mounted = Instruction::Run(run_with_mount);

        assert_ne!(run_no_mount.cache_key(), run_mounted.cache_key());
    }

    #[test]
    fn test_cache_key_env_ordering() {
        // ENV with same variables in different insertion order should have same key
        // (because we sort keys before hashing)
        let mut vars1 = HashMap::new();
        vars1.insert("A".to_string(), "1".to_string());
        vars1.insert("B".to_string(), "2".to_string());
        let env1 = Instruction::Env(EnvInstruction::from_vars(vars1));

        let mut vars2 = HashMap::new();
        vars2.insert("B".to_string(), "2".to_string());
        vars2.insert("A".to_string(), "1".to_string());
        let env2 = Instruction::Env(EnvInstruction::from_vars(vars2));

        assert_eq!(env1.cache_key(), env2.cache_key());
    }

    #[test]
    fn test_cache_key_onbuild() {
        // ONBUILD instructions should incorporate the inner instruction's cache key
        let inner = RunInstruction::shell("echo hello");
        let onbuild = Instruction::Onbuild(Box::new(Instruction::Run(inner.clone())));

        // The ONBUILD key should be different from the inner RUN key
        let run_key = Instruction::Run(inner).cache_key();
        let onbuild_key = onbuild.cache_key();
        assert_ne!(run_key, onbuild_key);
    }

    #[test]
    fn test_cache_key_copy_with_options() {
        // COPY with --from should differ from COPY without
        let copy_simple = Instruction::Copy(CopyInstruction::new(
            vec!["binary".into()],
            "/app/binary".into(),
        ));

        let copy_from = Instruction::Copy(
            CopyInstruction::new(vec!["binary".into()], "/app/binary".into()).from_stage("builder"),
        );

        assert_ne!(copy_simple.cache_key(), copy_from.cache_key());
    }
}
