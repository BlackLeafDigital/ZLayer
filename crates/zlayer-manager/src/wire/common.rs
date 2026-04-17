//! Cross-page wire helpers.
//!
//! Hosts shared primitives that don't belong to a single page. Currently
//! empty; each page-specific wire module lives alongside this one.

use serde::{Deserialize, Serialize};

/// Generic `{ message }` error payload — matches the shape the daemon uses
/// for most 4xx responses when it can't be more specific.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ErrorBody {
    /// Human-readable error message.
    pub message: String,
}
