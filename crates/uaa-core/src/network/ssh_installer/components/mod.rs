// file: crates/uaa-core/src/network/ssh_installer/components/mod.rs
// version: 1.0.0
// guid: 472b5bd7-86fb-4323-bde7-93b3ea691c89
// last-edited: 2026-07-23

//! Profile-system authoring-time components.
//!
//! Each module here defines a self-contained, closed-enum component type
//! (variant-select "union-by-kind") intended to be composed onto profile
//! authoring types. Modules are registered alphabetically.

pub mod firmware_quirks;
