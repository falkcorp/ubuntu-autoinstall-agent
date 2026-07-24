// file: crates/uaa-core/src/profile/components/mod.rs
// version: 1.0.0
// guid: 11e6587c-6071-414e-8da4-8562584644fc
// last-edited: 2026-07-23

//! Authoring-time sub-structs for `InstallationConfigPartial` fields that
//! are themselves multi-field groups (PS-UNLOCK-02 and siblings).
//!
//! Each submodule here defines a `*Partial` type mirroring one logical
//! group of [`InstallationConfig`](crate::network::ssh_installer::config::InstallationConfig)
//! fields, for eventual use as a single field on `InstallationConfigPartial`
//! (wired by a future brief — see `unlock_policy` for the pattern). This
//! module declares no wiring or merge/lower logic of its own.

pub mod unlock_policy;
