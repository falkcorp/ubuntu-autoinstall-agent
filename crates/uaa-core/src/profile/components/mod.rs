// file: crates/uaa-core/src/profile/components/mod.rs
// version: 1.0.0
// guid: 8c5e3d9a-1f7b-4e8c-9b2d-6a7c8d9e0a1b
// last-edited: 2026-07-23

//! Authoring-time component types for profile configuration.
//!
//! This module re-exports component types used in profile authoring, allowing
//! per-host and per-group overrides without restating unchanged fields.

pub mod network;
pub mod unlock_policy;
