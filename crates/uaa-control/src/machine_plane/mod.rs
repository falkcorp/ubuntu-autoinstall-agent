// file: crates/uaa-control/src/machine_plane/mod.rs
// version: 1.4.0
// guid: eee7b5c0-24d0-4406-b5f6-68d5bac52cae
// last-edited: 2026-07-19

//! Legacy machine plane (`:25000`) — exact Python parity (spec Decision 12).
//!
//! CT-01 owns THIS file. It declares the four follower submodules and the top-level
//! [`router`], which merges each follower's router (disjoint edits — one line per
//! task, no shared bodies):
//!   * `seeds`     — IP-01 (seed placement / boot-target intent)
//!   * `lifecycle` — IP-02 (checkin / install-event ingest → WAL when degraded)
//!   * `inventory` — IP-03 (machine listing / discovery inbox)
//!   * `dashboard` — IP-04 (status dashboard HTML)
//!
//! `staleness` (DS-CHK-03) is a standalone, read-time-only helper — it has no
//! route and is not merged into [`router`].

pub mod dashboard;
pub mod inventory;
pub mod lifecycle;
pub mod seeds;
pub mod staleness;

use axum::{routing::get, Json, Router};
use serde_json::json;

/// The `:25000` router. Followers merge their submodule routers here (see module doc).
pub fn router() -> Router {
    Router::new()
        .route(
            "/healthz",
            get(|| async {
                Json(json!({ "service": "uaa-control", "listener": "machine-plane" }))
            }),
        )
        .merge(seeds::router()) // IP-01
        .merge(lifecycle::router()) // IP-02
        .merge(inventory::router()) // IP-03
        .merge(dashboard::router()) // IP-04
        .merge(crate::discovered::ingest_router()) // DHCP discovery inbox ingest
}
