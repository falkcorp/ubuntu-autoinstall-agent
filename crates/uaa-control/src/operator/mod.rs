// file: crates/uaa-control/src/operator/mod.rs
// version: 1.1.0
// guid: c78abe95-9fa9-4dcf-9e4c-14f07d8fb509
// last-edited: 2026-07-12

//! Operator plane (`:8443`) — JSON API + SPA hosting.
//!
//! First vertical slice of CT-07 (full OpenAPI/utoipa + auth land later, per
//! `handlers.rs`'s module doc): [`handlers::router`]'s `/api/*` routes are
//! merged ahead of [`web_ui::router`]'s catch-all SPA fallback, so API paths
//! are matched first and every other path serves the embedded `web/dist`
//! (client-side routing via `react-router-dom` owns everything else).

pub mod api_types;
pub mod handlers;
pub mod web_ui;

use axum::Router;

/// The `:8443` router: real/stubbed JSON API, then the SPA for everything else.
pub fn router() -> Router {
    handlers::router().merge(web_ui::router())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_router_builds_standalone() {
        let _ = router();
    }
}
