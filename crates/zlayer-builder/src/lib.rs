//! ZLayer Builder - Dockerfile parsing and buildah command generation
//!
//! This crate provides functionality for parsing Dockerfiles and converting them
//! into buildah commands for container image building. It is designed to be used
//! as part of the ZLayer container orchestration platform.
//!
//! # Architecture
//!
//! The crate is organized into several modules:
//!
//! - [`dockerfile`]: Types and parsing for Dockerfile content
//! - [`buildah`]: Command generation and execution for buildah
//! - [`builder`]: High-level [`ImageBuilder`] API for orchestrating builds
//! - [`tui`]: Terminal UI for build progress visualization
//! - [`templates`]: Runtime templates for common development environments
//! - [`error`]: Error types for the builder subsystem
//!
//! # Quick Start with ImageBuilder
//!
//! The recommended way to build images is using the [`ImageBuilder`] API:
//!
//! ```no_run
//! use zlayer_builder::{ImageBuilder, Runtime};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // Build from a Dockerfile
//!     let image = ImageBuilder::new("./my-app").await?
//!         .tag("myapp:latest")
//!         .tag("myapp:v1.0.0")
//!         .build()
//!         .await?;
//!
//!     println!("Built image: {}", image.image_id);
//!     Ok(())
//! }
//! ```
//!
//! # Using Runtime Templates
//!
//! Build images without writing a Dockerfile using runtime templates:
//!
//! ```no_run
//! use zlayer_builder::{ImageBuilder, Runtime};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // Auto-detect runtime from project files, or specify explicitly
//!     let image = ImageBuilder::new("./my-node-app").await?
//!         .runtime(Runtime::Node20)
//!         .tag("myapp:latest")
//!         .build()
//!         .await?;
//!
//!     Ok(())
//! }
//! ```
//!
//! # Low-Level API
//!
//! For more control, you can use the low-level Dockerfile parsing and
//! buildah command generation APIs directly:
//!
//! ```no_run
//! use zlayer_builder::{Dockerfile, BuildahCommand, BuildahExecutor};
//!
//! # async fn example() -> Result<(), zlayer_builder::BuildError> {
//! // Parse a Dockerfile
//! let dockerfile = Dockerfile::parse(r#"
//!     FROM alpine:3.18
//!     RUN apk add --no-cache curl
//!     COPY . /app
//!     WORKDIR /app
//!     CMD ["./app"]
//! "#)?;
//!
//! // Get the final stage
//! let stage = dockerfile.final_stage().unwrap();
//!
//! // Create buildah commands for each instruction
//! let executor = BuildahExecutor::new()?;
//!
//! // Create a working container from the base image
//! let from_cmd = BuildahCommand::from_image(&stage.base_image.to_string_ref());
//! let output = executor.execute_checked(&from_cmd).await?;
//! let container_id = output.stdout.trim();
//!
//! // Execute each instruction
//! for instruction in &stage.instructions {
//!     let cmds = BuildahCommand::from_instruction(container_id, instruction);
//!     for cmd in cmds {
//!         executor.execute_checked(&cmd).await?;
//!     }
//! }
//!
//! // Commit the container to create an image
//! let commit_cmd = BuildahCommand::commit(container_id, "myimage:latest");
//! executor.execute_checked(&commit_cmd).await?;
//!
//! // Clean up the working container
//! let rm_cmd = BuildahCommand::rm(container_id);
//! executor.execute(&rm_cmd).await?;
//!
//! # Ok(())
//! # }
//! ```
//!
//! # Features
//!
//! ## ImageBuilder (High-Level API)
//!
//! The [`ImageBuilder`] provides a fluent API for:
//!
//! - Building from Dockerfiles or runtime templates
//! - Multi-stage builds with target stage selection
//! - Build arguments (ARG values)
//! - Image tagging and registry pushing
//! - TUI progress updates via event channels
//!
//! ## Dockerfile Parsing
//!
//! The crate supports parsing standard Dockerfiles with:
//!
//! - Multi-stage builds with named stages
//! - All standard Dockerfile instructions (FROM, RUN, COPY, ADD, ENV, etc.)
//! - ARG/ENV variable expansion with default values
//! - Global ARGs (before first FROM)
//!
//! ## Buildah Integration
//!
//! Commands are generated for buildah, a daemon-less container builder:
//!
//! - Container creation from images or scratch
//! - Running commands (shell and exec form)
//! - Copying files (including from other stages)
//! - Configuration (env, workdir, entrypoint, cmd, labels, etc.)
//! - Committing containers to images
//! - Image tagging and pushing
//!
//! ## Runtime Templates
//!
//! Pre-built templates for common development environments:
//!
//! - Node.js 20/22 (Alpine-based, production optimized)
//! - Python 3.12/3.13 (Slim Debian-based)
//! - Rust (Static musl binary)
//! - Go (Static binary)
//! - Deno and Bun
//!
//! ## Variable Expansion
//!
//! Full support for Dockerfile variable syntax:
//!
//! - `$VAR` and `${VAR}` - Simple variable reference
//! - `${VAR:-default}` - Default if unset or empty
//! - `${VAR:+alternate}` - Alternate if set and non-empty
//! - `${VAR-default}` - Default only if unset
//! - `${VAR+alternate}` - Alternate if set (including empty)

pub mod buildah;
pub mod builder;
pub mod dockerfile;
pub mod error;
pub mod templates;
pub mod tui;

// Re-export main types at crate root
pub use buildah::{
    current_platform,
    install_instructions,
    is_platform_supported,
    BuildahCommand,
    BuildahExecutor,
    // Installation types
    BuildahInstallation,
    BuildahInstaller,
    CommandOutput,
    InstallError,
};
pub use builder::{BuildOptions, BuiltImage, ImageBuilder, RegistryAuth};
pub use dockerfile::{
    expand_variables,
    // Instruction types
    AddInstruction,
    ArgInstruction,
    CopyInstruction,
    Dockerfile,
    EnvInstruction,
    ExposeInstruction,
    ExposeProtocol,
    HealthcheckInstruction,
    ImageRef,
    Instruction,
    RunInstruction,
    ShellOrExec,
    Stage,
    // Variable expansion
    VariableContext,
};
pub use error::{BuildError, Result};
pub use templates::{
    detect_runtime, detect_runtime_with_version, get_template, get_template_by_name,
    list_templates, resolve_runtime, Runtime, RuntimeInfo,
};
pub use tui::{BuildEvent, BuildTui, InstructionStatus, PlainLogger};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_and_convert_simple() {
        let dockerfile = Dockerfile::parse(
            r#"
FROM alpine:3.18
RUN echo "hello"
COPY . /app
WORKDIR /app
"#,
        )
        .unwrap();

        assert_eq!(dockerfile.stages.len(), 1);

        let stage = &dockerfile.stages[0];
        assert_eq!(stage.instructions.len(), 3);

        // Convert each instruction to buildah commands
        let container = "test-container";
        for instruction in &stage.instructions {
            let cmds = BuildahCommand::from_instruction(container, instruction);
            assert!(!cmds.is_empty() || matches!(instruction, Instruction::Arg(_)));
        }
    }

    #[test]
    fn test_parse_multistage_and_convert() {
        let dockerfile = Dockerfile::parse(
            r#"
FROM golang:1.21 AS builder
WORKDIR /src
COPY . .
RUN go build -o /app

FROM alpine:3.18
COPY --from=builder /app /app
ENTRYPOINT ["/app"]
"#,
        )
        .unwrap();

        assert_eq!(dockerfile.stages.len(), 2);

        // First stage (builder)
        let builder = &dockerfile.stages[0];
        assert_eq!(builder.name, Some("builder".to_string()));
        assert_eq!(builder.instructions.len(), 3);

        // Second stage (runtime)
        let runtime = &dockerfile.stages[1];
        assert!(runtime.name.is_none());
        assert_eq!(runtime.instructions.len(), 2);

        // Check COPY --from=builder is preserved
        if let Instruction::Copy(copy) = &runtime.instructions[0] {
            assert_eq!(copy.from, Some("builder".to_string()));
        } else {
            panic!("Expected COPY instruction");
        }
    }

    #[test]
    fn test_variable_expansion() {
        let mut ctx = VariableContext::new();
        ctx.add_arg("VERSION", Some("1.0".to_string()));
        ctx.set_env("HOME", "/app".to_string());

        assert_eq!(ctx.expand("$VERSION"), "1.0");
        assert_eq!(ctx.expand("$HOME"), "/app");
        assert_eq!(ctx.expand("${UNSET:-default}"), "default");
    }

    #[test]
    fn test_buildah_command_generation() {
        // Test various instruction conversions
        let container = "test";

        // RUN
        let run = Instruction::Run(RunInstruction {
            command: ShellOrExec::Shell("apt-get update".to_string()),
            mounts: vec![],
            network: None,
            security: None,
        });
        let cmds = BuildahCommand::from_instruction(container, &run);
        assert_eq!(cmds.len(), 1);
        assert!(cmds[0].args.contains(&"run".to_string()));

        // ENV
        let mut vars = std::collections::HashMap::new();
        vars.insert("PATH".to_string(), "/usr/local/bin".to_string());
        let env = Instruction::Env(EnvInstruction { vars });
        let cmds = BuildahCommand::from_instruction(container, &env);
        assert_eq!(cmds.len(), 1);
        assert!(cmds[0].args.contains(&"config".to_string()));
        assert!(cmds[0].args.contains(&"--env".to_string()));

        // WORKDIR
        let workdir = Instruction::Workdir("/app".to_string());
        let cmds = BuildahCommand::from_instruction(container, &workdir);
        assert_eq!(cmds.len(), 1);
        assert!(cmds[0].args.contains(&"--workingdir".to_string()));
    }
}
