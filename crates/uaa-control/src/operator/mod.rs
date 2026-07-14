// file: crates/uaa-control/src/operator/mod.rs
// version: 1.2.1
// guid: c78abe95-9fa9-4dcf-9e4c-14f07d8fb509
// last-edited: 2026-07-14

//! Operator plane (`:15000`) — JSON API + SPA hosting.
//!
//! First vertical slice of CT-07 (full OpenAPI/utoipa lands later, per
//! `handlers.rs`'s module doc): [`handlers::router`]'s `/api/*` routes are
//! merged ahead of [`web_ui::router`]'s catch-all SPA fallback, so API paths
//! are matched first and every other path serves the embedded `web/dist`
//! (client-side routing via `react-router-dom` owns everything else).
//!
//! Auth (CT-03, `crate::auth`) is now wired here: [`handlers::router`] builds
//! its own `Arc<crate::auth::AuthState>` + `Arc<crate::auth::BootstrapTokenState>`
//! internally and layers them as `Extension`s over its whole sub-router — see
//! that module's doc for exactly which routes require which
//! [`crate::auth::Role`].

pub mod api_types;
pub mod handlers;
pub mod web_ui;

use axum::Router;

/// The `:15000` router: real/stubbed JSON API, then the SPA for everything else.
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
