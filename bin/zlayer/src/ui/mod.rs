//! User-interface helpers for the `zlayer` CLI.
//!
//! This module hosts small, focused helpers that prompt the user for
//! confirmation or otherwise interact with stdin/stdout outside of the
//! main interactive TUI (which lives in `crate::app`). Keep each helper
//! single-purpose and unit-testable with dependency-injected readers/writers
//! so we do not need a real terminal to cover behaviour.

pub mod consent;
