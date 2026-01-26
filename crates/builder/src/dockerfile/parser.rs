//! Dockerfile parser
//!
//! This module provides functionality to parse Dockerfiles into a structured representation
//! using the `dockerfile-parser` crate as the parsing backend.

use std::collections::HashMap;
use std::path::Path;

use dockerfile_parser::{Dockerfile as RawDockerfile, Instruction as RawInstruction};
use serde::{Deserialize, Serialize};

use crate::error::{BuildError, Result};

use super::instruction::{
    AddInstruction, ArgInstruction, CopyInstruction, EnvInstruction, ExposeInstruction,
    ExposeProtocol, HealthcheckInstruction, Instruction, RunInstruction, ShellOrExec,
};

/// A reference to a Docker image
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImageRef {
    /// A registry image reference
    Registry {
        /// The full image name (e.g., "docker.io/library/alpine")
        image: String,
        /// Optional tag (e.g., "3.18")
        tag: Option<String>,
        /// Optional digest (e.g., "sha256:...")
        digest: Option<String>,
    },
    /// A reference to another stage in a multi-stage build
    Stage(String),
    /// The special "scratch" base image
    Scratch,
}

impl ImageRef {
    /// Parse an image reference string
    pub fn parse(s: &str) -> Self {
        let s = s.trim();

        // Handle scratch special case
        if s.eq_ignore_ascii_case("scratch") {
            return Self::Scratch;
        }

        // Parse image@digest or image:tag
        if let Some((image, digest)) = s.rsplit_once('@') {
            return Self::Registry {
                image: image.to_string(),
                tag: None,
                digest: Some(digest.to_string()),
            };
        }

        // Check for tag (but be careful with ports like localhost:5000/image)
        let colon_count = s.matches(':').count();
        if colon_count > 0 {
            if let Some((prefix, suffix)) = s.rsplit_once(':') {
                // If suffix doesn't contain '/', it's a tag
                if !suffix.contains('/') {
                    return Self::Registry {
                        image: prefix.to_string(),
                        tag: Some(suffix.to_string()),
                        digest: None,
                    };
                }
            }
        }

        // No tag or digest
        Self::Registry {
            image: s.to_string(),
            tag: None,
            digest: None,
        }
    }

    /// Convert to a full image string
    pub fn to_string_ref(&self) -> String {
        match self {
            Self::Registry { image, tag, digest } => {
                let mut s = image.clone();
                if let Some(t) = tag {
                    s.push(':');
                    s.push_str(t);
                }
                if let Some(d) = digest {
                    s.push('@');
                    s.push_str(d);
                }
                s
            }
            Self::Stage(name) => name.clone(),
            Self::Scratch => "scratch".to_string(),
        }
    }

    /// Returns true if this is a reference to a build stage
    pub fn is_stage(&self) -> bool {
        matches!(self, Self::Stage(_))
    }

    /// Returns true if this is the scratch base
    pub fn is_scratch(&self) -> bool {
        matches!(self, Self::Scratch)
    }
}

/// A single stage in a multi-stage Dockerfile
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Stage {
    /// Stage index (0-based)
    pub index: usize,

    /// Optional stage name (from `AS name`)
    pub name: Option<String>,

    /// The base image for this stage
    pub base_image: ImageRef,

    /// Optional platform specification (e.g., "linux/amd64")
    pub platform: Option<String>,

    /// Instructions in this stage (excluding the FROM)
    pub instructions: Vec<Instruction>,
}

impl Stage {
    /// Returns the stage identifier (name if present, otherwise index as string)
    pub fn identifier(&self) -> String {
        self.name.clone().unwrap_or_else(|| self.index.to_string())
    }

    /// Returns true if this stage matches the given name or index
    pub fn matches(&self, name_or_index: &str) -> bool {
        if let Some(ref name) = self.name {
            if name == name_or_index {
                return true;
            }
        }

        if let Ok(idx) = name_or_index.parse::<usize>() {
            return idx == self.index;
        }

        false
    }
}

/// A parsed Dockerfile
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dockerfile {
    /// Global ARG instructions that appear before the first FROM
    pub global_args: Vec<ArgInstruction>,

    /// Build stages
    pub stages: Vec<Stage>,
}

impl Dockerfile {
    /// Parse a Dockerfile from a string
    pub fn parse(content: &str) -> Result<Self> {
        let raw = RawDockerfile::parse(content).map_err(|e| BuildError::DockerfileParse {
            message: e.to_string(),
            line: 1,
        })?;

        Self::from_raw(raw)
    }

    /// Parse a Dockerfile from a file
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self> {
        let content =
            std::fs::read_to_string(path.as_ref()).map_err(|e| BuildError::ContextRead {
                path: path.as_ref().to_path_buf(),
                source: e,
            })?;

        Self::parse(&content)
    }

    /// Convert from the raw dockerfile-parser types to our internal representation
    fn from_raw(raw: RawDockerfile) -> Result<Self> {
        let mut global_args = Vec::new();
        let mut stages = Vec::new();
        let mut current_stage: Option<Stage> = None;
        let mut stage_index = 0;

        for instruction in raw.instructions {
            match &instruction {
                RawInstruction::From(from) => {
                    // Save previous stage if any
                    if let Some(stage) = current_stage.take() {
                        stages.push(stage);
                    }

                    // Parse base image
                    let base_image = ImageRef::parse(&from.image.content);

                    // Get alias (stage name) - the field is `alias` not `image_alias`
                    let name = from.alias.as_ref().map(|a| a.content.clone());

                    // Get platform flag
                    let platform = from
                        .flags
                        .iter()
                        .find(|f| f.name.content.as_str() == "platform")
                        .map(|f| f.value.to_string());

                    current_stage = Some(Stage {
                        index: stage_index,
                        name,
                        base_image,
                        platform,
                        instructions: Vec::new(),
                    });

                    stage_index += 1;
                }

                RawInstruction::Arg(arg) => {
                    let arg_inst = ArgInstruction {
                        name: arg.name.to_string(),
                        default: arg.value.as_ref().map(|v| v.to_string()),
                    };

                    if current_stage.is_none() {
                        global_args.push(arg_inst);
                    } else if let Some(ref mut stage) = current_stage {
                        stage.instructions.push(Instruction::Arg(arg_inst));
                    }
                }

                _ => {
                    if let Some(ref mut stage) = current_stage {
                        if let Some(inst) = Self::convert_instruction(&instruction)? {
                            stage.instructions.push(inst);
                        }
                    }
                }
            }
        }

        // Don't forget the last stage
        if let Some(stage) = current_stage {
            stages.push(stage);
        }

        // Resolve stage references in COPY --from
        // (This is currently a no-op as stage references are already correct,
        // but kept for future validation/resolution logic)
        let _stage_names: HashMap<String, usize> = stages
            .iter()
            .filter_map(|s| s.name.as_ref().map(|n| (n.clone(), s.index)))
            .collect();
        let _num_stages = stages.len();

        Ok(Self {
            global_args,
            stages,
        })
    }

    /// Convert a raw instruction to our internal representation
    fn convert_instruction(raw: &RawInstruction) -> Result<Option<Instruction>> {
        let instruction = match raw {
            RawInstruction::From(_) => {
                return Ok(None);
            }

            RawInstruction::Run(run) => {
                let command = match &run.expr {
                    dockerfile_parser::ShellOrExecExpr::Shell(s) => {
                        ShellOrExec::Shell(s.to_string())
                    }
                    dockerfile_parser::ShellOrExecExpr::Exec(args) => {
                        ShellOrExec::Exec(args.elements.iter().map(|s| s.content.clone()).collect())
                    }
                };

                Instruction::Run(RunInstruction {
                    command,
                    mounts: Vec::new(),
                    network: None,
                    security: None,
                })
            }

            RawInstruction::Copy(copy) => {
                let from = copy
                    .flags
                    .iter()
                    .find(|f| f.name.content.as_str() == "from")
                    .map(|f| f.value.to_string());

                let chown = copy
                    .flags
                    .iter()
                    .find(|f| f.name.content.as_str() == "chown")
                    .map(|f| f.value.to_string());

                let chmod = copy
                    .flags
                    .iter()
                    .find(|f| f.name.content.as_str() == "chmod")
                    .map(|f| f.value.to_string());

                let link = copy.flags.iter().any(|f| f.name.content.as_str() == "link");

                // Get all paths
                let all_paths: Vec<String> = copy.sources.iter().map(|s| s.to_string()).collect();

                if all_paths.is_empty() {
                    return Err(BuildError::InvalidInstruction {
                        instruction: "COPY".to_string(),
                        reason: "COPY requires at least one source and a destination".to_string(),
                    });
                }

                let (sources, dest) = all_paths.split_at(all_paths.len().saturating_sub(1));
                let destination = dest.first().cloned().unwrap_or_default();

                Instruction::Copy(CopyInstruction {
                    sources: sources.to_vec(),
                    destination,
                    from,
                    chown,
                    chmod,
                    link,
                    exclude: Vec::new(),
                })
            }

            RawInstruction::Entrypoint(ep) => {
                let command = match &ep.expr {
                    dockerfile_parser::ShellOrExecExpr::Shell(s) => {
                        ShellOrExec::Shell(s.to_string())
                    }
                    dockerfile_parser::ShellOrExecExpr::Exec(args) => {
                        ShellOrExec::Exec(args.elements.iter().map(|s| s.content.clone()).collect())
                    }
                };
                Instruction::Entrypoint(command)
            }

            RawInstruction::Cmd(cmd) => {
                let command = match &cmd.expr {
                    dockerfile_parser::ShellOrExecExpr::Shell(s) => {
                        ShellOrExec::Shell(s.to_string())
                    }
                    dockerfile_parser::ShellOrExecExpr::Exec(args) => {
                        ShellOrExec::Exec(args.elements.iter().map(|s| s.content.clone()).collect())
                    }
                };
                Instruction::Cmd(command)
            }

            RawInstruction::Env(env) => {
                let mut vars = HashMap::new();
                for var in &env.vars {
                    vars.insert(var.key.to_string(), var.value.to_string());
                }
                Instruction::Env(EnvInstruction { vars })
            }

            RawInstruction::Label(label) => {
                let mut labels = HashMap::new();
                for l in &label.labels {
                    labels.insert(l.name.to_string(), l.value.to_string());
                }
                Instruction::Label(labels)
            }

            RawInstruction::Arg(arg) => Instruction::Arg(ArgInstruction {
                name: arg.name.to_string(),
                default: arg.value.as_ref().map(|v| v.to_string()),
            }),

            RawInstruction::Misc(misc) => {
                let instruction_upper = misc.instruction.content.to_uppercase();
                match instruction_upper.as_str() {
                    "WORKDIR" => Instruction::Workdir(misc.arguments.to_string()),

                    "USER" => Instruction::User(misc.arguments.to_string()),

                    "VOLUME" => {
                        let args = misc.arguments.to_string();
                        let volumes = if args.trim().starts_with('[') {
                            serde_json::from_str(&args).unwrap_or_else(|_| vec![args])
                        } else {
                            args.split_whitespace().map(String::from).collect()
                        };
                        Instruction::Volume(volumes)
                    }

                    "EXPOSE" => {
                        let args = misc.arguments.to_string();
                        let (port_str, protocol) = if let Some((p, proto)) = args.split_once('/') {
                            let proto = match proto.to_lowercase().as_str() {
                                "udp" => ExposeProtocol::Udp,
                                _ => ExposeProtocol::Tcp,
                            };
                            (p, proto)
                        } else {
                            (args.as_str(), ExposeProtocol::Tcp)
                        };

                        let port: u16 = port_str.trim().parse().map_err(|_| {
                            BuildError::InvalidInstruction {
                                instruction: "EXPOSE".to_string(),
                                reason: format!("Invalid port number: {}", port_str),
                            }
                        })?;

                        Instruction::Expose(ExposeInstruction { port, protocol })
                    }

                    "SHELL" => {
                        let args = misc.arguments.to_string();
                        let shell: Vec<String> = serde_json::from_str(&args).map_err(|_| {
                            BuildError::InvalidInstruction {
                                instruction: "SHELL".to_string(),
                                reason: "SHELL requires a JSON array".to_string(),
                            }
                        })?;
                        Instruction::Shell(shell)
                    }

                    "STOPSIGNAL" => Instruction::Stopsignal(misc.arguments.to_string()),

                    "HEALTHCHECK" => {
                        let args = misc.arguments.to_string().trim().to_string();
                        if args.eq_ignore_ascii_case("NONE") {
                            Instruction::Healthcheck(HealthcheckInstruction::None)
                        } else {
                            let command = if let Some(stripped) = args.strip_prefix("CMD ") {
                                ShellOrExec::Shell(stripped.to_string())
                            } else {
                                ShellOrExec::Shell(args)
                            };
                            Instruction::Healthcheck(HealthcheckInstruction::cmd(command))
                        }
                    }

                    "ONBUILD" => {
                        tracing::warn!("ONBUILD instruction parsing not fully implemented");
                        return Ok(None);
                    }

                    "MAINTAINER" => {
                        let mut labels = HashMap::new();
                        labels.insert("maintainer".to_string(), misc.arguments.to_string());
                        Instruction::Label(labels)
                    }

                    "ADD" => {
                        let args = misc.arguments.to_string();
                        let parts: Vec<String> =
                            args.split_whitespace().map(String::from).collect();

                        if parts.len() < 2 {
                            return Err(BuildError::InvalidInstruction {
                                instruction: "ADD".to_string(),
                                reason: "ADD requires at least one source and a destination"
                                    .to_string(),
                            });
                        }

                        let (sources, dest) = parts.split_at(parts.len() - 1);
                        let destination = dest.first().cloned().unwrap_or_default();

                        Instruction::Add(AddInstruction {
                            sources: sources.to_vec(),
                            destination,
                            chown: None,
                            chmod: None,
                            link: false,
                            checksum: None,
                            keep_git_dir: false,
                        })
                    }

                    other => {
                        tracing::warn!("Unknown Dockerfile instruction: {}", other);
                        return Ok(None);
                    }
                }
            }
        };

        Ok(Some(instruction))
    }

    /// Get a stage by name or index
    pub fn get_stage(&self, name_or_index: &str) -> Option<&Stage> {
        self.stages.iter().find(|s| s.matches(name_or_index))
    }

    /// Get the final stage (last one in the Dockerfile)
    pub fn final_stage(&self) -> Option<&Stage> {
        self.stages.last()
    }

    /// Get all stage names/identifiers
    pub fn stage_names(&self) -> Vec<String> {
        self.stages.iter().map(|s| s.identifier()).collect()
    }

    /// Check if a stage exists
    pub fn has_stage(&self, name_or_index: &str) -> bool {
        self.get_stage(name_or_index).is_some()
    }

    /// Returns the number of stages
    pub fn stage_count(&self) -> usize {
        self.stages.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_image_ref_parse_simple() {
        let img = ImageRef::parse("alpine");
        assert!(matches!(
            img,
            ImageRef::Registry {
                ref image,
                tag: None,
                digest: None
            } if image == "alpine"
        ));
    }

    #[test]
    fn test_image_ref_parse_with_tag() {
        let img = ImageRef::parse("alpine:3.18");
        assert!(matches!(
            img,
            ImageRef::Registry {
                ref image,
                tag: Some(ref t),
                digest: None
            } if image == "alpine" && t == "3.18"
        ));
    }

    #[test]
    fn test_image_ref_parse_with_digest() {
        let img = ImageRef::parse("alpine@sha256:abc123");
        assert!(matches!(
            img,
            ImageRef::Registry {
                ref image,
                tag: None,
                digest: Some(ref d)
            } if image == "alpine" && d == "sha256:abc123"
        ));
    }

    #[test]
    fn test_image_ref_parse_scratch() {
        let img = ImageRef::parse("scratch");
        assert!(matches!(img, ImageRef::Scratch));

        let img = ImageRef::parse("SCRATCH");
        assert!(matches!(img, ImageRef::Scratch));
    }

    #[test]
    fn test_image_ref_parse_registry_with_port() {
        let img = ImageRef::parse("localhost:5000/myimage:latest");
        assert!(matches!(
            img,
            ImageRef::Registry {
                ref image,
                tag: Some(ref t),
                ..
            } if image == "localhost:5000/myimage" && t == "latest"
        ));
    }

    #[test]
    fn test_parse_simple_dockerfile() {
        let content = r#"
FROM alpine:3.18
RUN apk add --no-cache curl
COPY . /app
WORKDIR /app
CMD ["./app"]
"#;

        let dockerfile = Dockerfile::parse(content).unwrap();
        assert_eq!(dockerfile.stages.len(), 1);

        let stage = &dockerfile.stages[0];
        assert_eq!(stage.index, 0);
        assert!(stage.name.is_none());
        assert_eq!(stage.instructions.len(), 4);
    }

    #[test]
    fn test_parse_multistage_dockerfile() {
        let content = r#"
FROM golang:1.21 AS builder
WORKDIR /src
COPY . .
RUN go build -o /app

FROM alpine:3.18
COPY --from=builder /app /app
CMD ["/app"]
"#;

        let dockerfile = Dockerfile::parse(content).unwrap();
        assert_eq!(dockerfile.stages.len(), 2);

        let builder = &dockerfile.stages[0];
        assert_eq!(builder.name, Some("builder".to_string()));

        let runtime = &dockerfile.stages[1];
        assert!(runtime.name.is_none());

        let copy = runtime
            .instructions
            .iter()
            .find(|i| matches!(i, Instruction::Copy(_)));
        assert!(copy.is_some());
        if let Some(Instruction::Copy(c)) = copy {
            assert_eq!(c.from, Some("builder".to_string()));
        }
    }

    #[test]
    fn test_parse_global_args() {
        let content = r#"
ARG BASE_IMAGE=alpine:3.18
FROM ${BASE_IMAGE}
RUN echo "hello"
"#;

        let dockerfile = Dockerfile::parse(content).unwrap();
        assert_eq!(dockerfile.global_args.len(), 1);
        assert_eq!(dockerfile.global_args[0].name, "BASE_IMAGE");
        assert_eq!(
            dockerfile.global_args[0].default,
            Some("alpine:3.18".to_string())
        );
    }

    #[test]
    fn test_get_stage_by_name() {
        let content = r#"
FROM alpine:3.18 AS base
RUN echo "base"

FROM base AS builder
RUN echo "builder"
"#;

        let dockerfile = Dockerfile::parse(content).unwrap();

        let base = dockerfile.get_stage("base");
        assert!(base.is_some());
        assert_eq!(base.unwrap().index, 0);

        let builder = dockerfile.get_stage("builder");
        assert!(builder.is_some());
        assert_eq!(builder.unwrap().index, 1);

        let stage_0 = dockerfile.get_stage("0");
        assert!(stage_0.is_some());
        assert_eq!(stage_0.unwrap().name, Some("base".to_string()));
    }

    #[test]
    fn test_final_stage() {
        let content = r#"
FROM alpine:3.18 AS builder
RUN echo "builder"

FROM scratch
COPY --from=builder /app /app
"#;

        let dockerfile = Dockerfile::parse(content).unwrap();
        let final_stage = dockerfile.final_stage().unwrap();

        assert_eq!(final_stage.index, 1);
        assert!(matches!(final_stage.base_image, ImageRef::Scratch));
    }

    #[test]
    fn test_parse_env_instruction() {
        let content = r#"
FROM alpine
ENV FOO=bar BAZ=qux
"#;

        let dockerfile = Dockerfile::parse(content).unwrap();
        let stage = &dockerfile.stages[0];

        let env = stage
            .instructions
            .iter()
            .find(|i| matches!(i, Instruction::Env(_)));
        assert!(env.is_some());

        if let Some(Instruction::Env(e)) = env {
            assert_eq!(e.vars.get("FOO"), Some(&"bar".to_string()));
            assert_eq!(e.vars.get("BAZ"), Some(&"qux".to_string()));
        }
    }
}
