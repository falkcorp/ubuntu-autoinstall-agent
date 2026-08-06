// file: crates/uaa-core/src/network/ssh_installer/mod.rs
// version: 1.9.0
// guid: sshmod01-2345-6789-abcd-ef0123456789
// last-edited: 2026-08-02

//! SSH-based Ubuntu installation with ZFS and LUKS
//!
//! This module provides a comprehensive SSH-based installation system
//! for Ubuntu with ZFS and LUKS encryption.

pub mod applications;
pub mod components;
pub mod config;
pub mod credentials;
pub mod disk_native;
pub mod disk_ops;
pub mod installer;
pub mod investigation;
pub mod layout;
pub mod packages;
pub mod partitions;
pub mod reset_partition;
pub mod status;
pub mod system_setup;
pub mod unlock_sss;
pub mod zfs_native;
pub mod zfs_ops;

pub use config::{InstallationConfig, SystemInfo};
pub use installer::SshInstaller;
