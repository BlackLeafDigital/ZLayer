//! Streaming helpers for Docker Engine API socket emulation.
//!
//! Houses the wire-format encoders and stream-multiplex utilities used by
//! endpoints that produce framed byte streams (e.g.
//! `/containers/{id}/logs?follow=true` and `/containers/{id}/attach`).

pub mod hijack;
pub mod log_frame;
