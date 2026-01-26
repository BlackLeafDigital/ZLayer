//! Dockerfile parsing and representation
//!
//! This module provides types and functions for parsing Dockerfiles into a structured
//! representation that can be used for analysis, validation, and conversion to buildah commands.

mod instruction;
mod parser;
mod variable;

pub use instruction::*;
pub use parser::*;
pub use variable::*;
