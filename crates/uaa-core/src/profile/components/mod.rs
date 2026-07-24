// file: crates/uaa-core/src/profile/components/mod.rs
// version: 1.0.0
// guid: d1e4c5f2-8a1b-4e2c-a3d7-f1b6e9c2a5d8
// last-edited: 2026-07-23

//! Authoring-time component sub-structs for the profile-system conversion
//! (PS wave 1).
//!
//! Each submodule defines a `*Partial` authoring type mirroring one logical
//! group of [`InstallationConfig`](crate::network::ssh_installer::config::InstallationConfig)
//! fields, for eventual use as a single field on `InstallationConfigPartial`
//! (wired by a future brief — see `unlock_policy` for the pattern). These
//! modules declare no wiring or merge/lower logic of their own. Registered
//! alphabetically.

pub mod base_image;
pub mod network;
pub mod unlock_policy;
