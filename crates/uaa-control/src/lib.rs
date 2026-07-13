// file: crates/uaa-control/src/lib.rs
// version: 1.0.1
// guid: 377e6bf2-0687-480d-a7f4-7bd21c525206
// last-edited: 2026-07-13

//! uaa-control — the constellation's central daemon (spec component C3).
//!
//! Owns the registry system-of-record (CockroachDB, Decision 4/5), the four listeners
//! (`:25000` legacy machine plane via systemd socket activation, plus `:7443` gRPC
//! mTLS, `:7444` enrollment JSON, `:15001` operator), the embedded schema migrations,
//! and the snapshot+WAL degraded-mode layer.
//!
//! This crate is scaffolded by CT-01. Most feature surface lands via follower tasks
//! that each own a DISJOINT set of stub modules (the de-collision pattern — one filling
//! task per stub file). CT-01 provides: [`db`] (row types, migrations, degraded store),
//! [`listeners`] (socket activation + health scaffolds), and [`machine_plane`] (the
//! `:25000` router that fillers merge into). Everything DB-shaped sits behind traits
//! with in-memory mocks, so `cargo test --lib --offline` needs NO live CockroachDB.

// CT-01-owned modules (implemented here).
pub mod db;
pub mod listeners;
pub mod machine_plane;

// Follower-owned stub modules (one exclusive filler each — see each file's header).
pub mod audit; // CT-04
pub mod auth; // CT-03
pub mod ca; // PK-01, then PK-03 (serialized)
pub mod enroll; // PK-01
pub mod import_export; // CT-02
pub mod operator; // CT-07
pub mod reinstall; // CT-06
pub mod saga; // CT-05
