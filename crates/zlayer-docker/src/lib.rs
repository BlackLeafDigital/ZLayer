//! Docker CLI, Compose, and API socket compatibility layer for `ZLayer`.

pub mod cli;
pub mod compose;

#[cfg(unix)]
pub mod socket;

pub use cli::{handle_docker_command, DockerCommands};
pub use compose::{parse_compose, ComposeFile};

#[derive(Debug, thiserror::Error)]
pub enum DockerError {
    #[error("compose parse error: {0}")]
    ComposeParse(String),
    #[error("conversion error: {0}")]
    Conversion(String),
    #[error("unsupported compose feature: {0}")]
    Unsupported(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Yaml(#[from] serde_yaml::Error),
    #[error(transparent)]
    Spec(#[from] zlayer_spec::SpecError),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

pub type Result<T> = std::result::Result<T, DockerError>;
