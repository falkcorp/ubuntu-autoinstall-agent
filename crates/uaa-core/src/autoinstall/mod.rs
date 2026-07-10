// file: crates/uaa-core/src/autoinstall/mod.rs
// version: 1.2.1
// guid: a0b1c2d3-e4f5-6a7b-8c9d-0e1f2a3b4c5d
// last-edited: 2026-07-10

//! Render subiquity autoinstall `user-data` from a template + per-host spec.
//!
//! This is the pivot away from the imperative ZFS installer: instead of driving
//! an install command-by-command, the tool generates the proven, hand-verified
//! len-serv-003 `user-data` parameterized per host, which the native Ubuntu
//! installer (subiquity) then consumes.

pub mod host_spec;
pub mod place;
pub mod render;
pub mod verify;

pub use host_spec::HostSpec;
pub use place::{place_and_drive, FlipResult, PlaceOpts, PlaceReport};
pub use render::{default_template, render_user_data};
pub use verify::{verify_host, CheckResult, VerifyReport};
