// file: crates/uaa-core/src/network/ssh_installer/components/mod.rs
// version: 1.0.0
// guid: 3b7e0a1a-6f3c-4e2b-9a8d-2c1f5e0d4b6a
// last-edited: 2026-07-23

//! Component-profile authoring types (Phase-0 seam).
//!
//! Each submodule is a self-contained AUTHORING component, created
//! additively by its own wave-1 brief. Nothing here is wired into the
//! installer yet — see `docs/agent-tasks/profile-system/README.md`.

pub mod hooks;
