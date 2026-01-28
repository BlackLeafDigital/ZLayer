//! Reusable UI components for `ZLayer` web interface

mod code_editor;
mod footer;
mod navbar;
pub mod stats;

pub use code_editor::{CodeBlock, CodeEditor};
pub use footer::Footer;
pub use navbar::Navbar;
pub use stats::{HealthBadge, ResourceMeter, ServiceRow, ServicesTable, StatsCard};
