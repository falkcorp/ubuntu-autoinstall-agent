// file: crates/uaa-control/src/db/registry.rs
// version: 1.0.0
// guid: 02d40065-96da-4469-b679-b8bfd4f0b8b3
// last-edited: 2026-07-10

//! Registry CRUD against CockroachDB (the `RegistryStore` trait + tokio-postgres impl).
//!
//! STUB — Filled exclusively by control TASK-02 (CT-02). Row types live in
//! `db::mod`; this module adds the query/mutation methods and calls
//! `db::store::write_snapshot` after every successful mutation.
