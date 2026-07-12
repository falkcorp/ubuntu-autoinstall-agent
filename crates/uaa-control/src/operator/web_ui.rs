// file: crates/uaa-control/src/operator/web_ui.rs
// version: 1.0.0
// guid: 3b8f6a2d-2c9e-4a5d-9f1b-7e6d5c4b3a2f
// last-edited: 2026-07-12

//! Operator SPA hosting (CT-07 / CT-08 pairing): serves `web/dist` — built by
//! `crates/uaa-control/build.rs` before this crate compiles — embedded into
//! the binary via [`rust_embed`]. No files are copied at deploy time; the
//! running `uaa-control` binary IS the deploy artifact for the SPA too.
//!
//! Any request path that doesn't match an embedded asset falls back to
//! `index.html` (client-side routing via `react-router-dom` — the SPA owns
//! `/machines`, `/approvals`, `/discovery`, `/audit`; the server has no
//! matching routes for those paths and must not 404 them).

use axum::{
    http::{header, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "../../web/dist"]
struct WebAssets;

/// Hand-rolled extension -> MIME map instead of pulling in a `mime_guess`
/// dependency — the Vite build only ever emits these few types.
fn content_type_for(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("json") => "application/json; charset=utf-8",
        Some("ico") => "image/x-icon",
        Some("png") => "image/png",
        Some("woff2") => "font/woff2",
        Some("map") => "application/json; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn serve_embedded(path: &str) -> Option<Response> {
    let asset = WebAssets::get(path)?;
    Some(
        (
            StatusCode::OK,
            [(header::CONTENT_TYPE, content_type_for(path))],
            asset.data,
        )
            .into_response(),
    )
}

async fn handle_asset(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };

    if let Some(resp) = serve_embedded(path) {
        return resp;
    }
    // SPA fallback: an unmatched path is a client-side route, not a 404 —
    // serve index.html and let react-router-dom take it from there.
    match serve_embedded("index.html") {
        Some(resp) => resp,
        None => (
            StatusCode::NOT_FOUND,
            "web/dist not embedded (empty build?)",
        )
            .into_response(),
    }
}

/// The SPA-hosting sub-router. Mounted as the operator plane's fallback (API
/// routes are matched first; anything else falls through to here) so it
/// never shadows `/api/*`.
pub fn router() -> Router {
    Router::new().fallback(get(handle_asset))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_router_builds_standalone() {
        let _ = router();
    }

    #[test]
    fn test_content_type_matches_vite_output_extensions() {
        assert_eq!(content_type_for("index.html"), "text/html; charset=utf-8");
        assert_eq!(
            content_type_for("assets/index-abc123.js"),
            "text/javascript; charset=utf-8"
        );
        assert_eq!(
            content_type_for("assets/index-abc123.css"),
            "text/css; charset=utf-8"
        );
        assert_eq!(content_type_for("unknown.bin"), "application/octet-stream");
    }

    #[tokio::test]
    async fn test_unmatched_path_falls_back_to_index_html() {
        // Whatever web/dist currently contains (built or the .gitkeep-only
        // placeholder), an unmatched path must never 404 while index.html
        // itself is embedded and reachable — proves the fallback wiring, not
        // the build output.
        if serve_embedded("index.html").is_none() {
            // web/dist wasn't built in this test environment (e.g.
            // UAA_SKIP_WEB_BUILD with an empty placeholder) — nothing to
            // assert against; skip rather than false-fail.
            return;
        }
        let resp = handle_asset(Uri::from_static("/machines")).await;
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers()
                .get(header::CONTENT_TYPE)
                .unwrap()
                .to_str()
                .unwrap(),
            "text/html; charset=utf-8"
        );
    }
}
