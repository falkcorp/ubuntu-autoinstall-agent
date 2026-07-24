// file: crates/uaa-core/src/network/ssh_installer/components/mod.rs
// version: 1.0.0
// guid: 71d9b4e1-e69e-443b-abec-52a53cd5c94a
// last-edited: 2026-07-23

//! Authoring-time component types for the profile-system conversion (Phase-0 seam).
//!
//! Each submodule is a self-contained AUTHORING component — a tagged-enum spec
//! type (mirroring
//! [`ApplicationSpec`](crate::network::ssh_installer::config::ApplicationSpec))
//! plus its per-variant partial(s) — created additively by its own wave-1 brief.
//! None of these are wired onto
//! [`InstallationConfig`](crate::network::ssh_installer::config::InstallationConfig)
//! yet; wiring each component in is a later, per-component task. See
//! `docs/agent-tasks/profile-system/README.md`. Modules registered alphabetically.

pub mod disk_layout;
pub mod firmware_quirks;
pub mod hooks;
