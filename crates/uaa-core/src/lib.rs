// file: crates/uaa-core/src/lib.rs
// version: 1.3.0
// guid: d82472d1-7f0f-4eb4-b0a3-6e1547103eb4
// last-edited: 2026-07-10

//! # Ubuntu AutoInstall Agent
//!
//! Automated Ubuntu server deployment with golden images and LUKS encryption.
//! This system provides zero manual intervention deployment using VM-based golden
//! images that can be deployed via SSH or netboot.

pub mod autoinstall;
pub mod config;
pub mod config_place;
pub mod discovery;
pub mod error;
pub mod fleet;
pub mod image;
pub mod iso;
pub mod logging;
pub mod luks_keys;
pub mod luks_sync;
pub mod network;
pub mod pki;
pub mod power;
pub mod security;
pub mod tls;
pub mod update;
pub mod utils;
pub mod vm_validate;

pub use error::{AutoInstallError, Result};
