//! REST API handlers

pub mod auth;
pub mod build;
pub mod cluster;
pub mod containers;
pub mod cron;
pub mod deployments;
pub mod health;
pub mod images;
pub mod internal;
pub mod jobs;
pub mod networks;
pub mod nodes;
pub mod overlay;
pub mod proxy;
pub mod secrets;
pub mod services;
pub mod storage;
pub mod tunnels;

pub use auth::*;
pub use build::*;
pub use cluster::*;
pub use containers::ContainerApiState;
pub use cron::*;
pub use deployments::*;
pub use health::*;
pub use internal::*;
pub use jobs::*;
pub use networks::*;
pub use nodes::*;
pub use overlay::*;
pub use proxy::*;
pub use secrets::*;
pub use services::*;
pub use storage::*;
pub use tunnels::*;
