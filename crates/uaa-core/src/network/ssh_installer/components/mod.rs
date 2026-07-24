// file: crates/uaa-core/src/network/ssh_installer/components/mod.rs
// version: 1.0.0
// guid: 71d9b4e1-e69e-443b-abec-52a53cd5c94a
// last-edited: 2026-07-23

//! Authoring-time component types for the profile-system conversion.
//!
//! Each component here is a self-contained tagged-enum spec type (mirroring
//! [`ApplicationSpec`](crate::network::ssh_installer::config::ApplicationSpec))
//! plus its per-variant partial(s). None of these are wired onto
//! [`InstallationConfig`](crate::network::ssh_installer::config::InstallationConfig)
//! yet — wiring each component in is a later, per-component task.

pub mod disk_layout;
