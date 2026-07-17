// file: crates/uaa-control/src/profiles/mod.rs
// version: 0.1.0
// guid: 10bb9c0d-98ab-48f9-ba74-9f89d1d6443e
// last-edited: 2026-07-17

//! Host group / profile management (spec `deploy-system-design.md`, DS-REG-01..04).
//!
//! Profiles persist in the `StatePaths` JSON snapshot (`db/store.rs`'s
//! `SnapshotDoc.host_groups` / `host_profiles` / `hostname_allocations` /
//! `profile_versions`), **NOT** in CockroachDB: `uaa-control` has no database
//! connection in production (spec D4) — `tokio_postgres` appears in no wiring
//! file, `default_state()` builds `FileRegistry(StatePaths)` + `Mem*Store`, and
//! `db::migrations::apply` has no caller. There is no SQL and no migration for
//! this module or any of its siblings.
//!
//! Row types live in `crate::db` per that module's own convention; this module
//! only adds behavior, mirroring `saga.rs`'s separate-trait+module precedent
//! rather than growing `RegistryStore` (`db/registry.rs`).

pub mod store;
pub mod alloc;
pub mod drift;
