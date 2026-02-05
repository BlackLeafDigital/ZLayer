//! ZImagefile - YAML-based image build format
//!
//! This module provides types, parsing, and conversion for ZImagefiles,
//! an alternative to Dockerfiles using YAML syntax.

mod converter;
mod parser;
pub mod types;

pub use converter::zimage_to_dockerfile;
pub use parser::parse_zimagefile;
pub use types::*;
