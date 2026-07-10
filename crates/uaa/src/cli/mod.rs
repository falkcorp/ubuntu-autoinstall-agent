// file: crates/uaa/src/cli/mod.rs
// version: 1.0.1
// guid: e5f6g7h8-i9j0-1234-5678-901234efghij

//! Command line interface for Ubuntu AutoInstall Agent

pub mod args;
pub mod commands;
pub mod config;
pub mod enroll;
pub mod image;
pub mod iso;
pub mod luks;
pub mod vm_validate;

pub use args::Cli;
pub use commands::*;
