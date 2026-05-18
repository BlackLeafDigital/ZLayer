//! `Tonic`-generated gRPC types and service for the `ZLayer` worker tier.
//!
//! Worker nodes use these client stubs to talk to the control plane; control
//! plane nodes use the server traits to handle worker registration,
//! assignment dispatch, and status reporting.

#[allow(clippy::pedantic, clippy::all, missing_docs, unused_qualifications)]
pub mod proto {
    tonic::include_proto!("zlayer.cluster.v1");
}

pub use proto::*;

pub mod convert;
pub use convert::ConvertError;
