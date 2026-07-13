// file: crates/uaa-control/src/listeners.rs
// version: 1.2.0
// guid: 4275dc4f-c0cb-479b-9f5c-5a444ed312f7
// last-edited: 2026-07-13

//! Listener wiring + systemd socket activation (spec Decision 24).
//!
//! uaa-control serves four planes:
//!   * `:25000` — legacy machine plane (exact Python parity), taken over via systemd
//!     socket activation so a self-update restart never drops the listening socket;
//!   * `:7443` — gRPC mTLS (services + enrolled agents);
//!   * `:7444` — enrollment JSON (install-CA-pinned);
//!   * `:15001` — operator JSON API (first CT-07 slice, see `operator::handlers`'s
//!     module doc for what's real vs. stubbed) + SPA hosting (`operator::web_ui`).
//!     Deliberately NOT `:8443` — that's a common alt-HTTPS port other services
//!     reuse; a high, less-contested port avoids collisions on shared hosts.
//!
//! `:7443`/`:7444` remain bind-and-health scaffolds: each serves only `GET /healthz`;
//! routes and TLS termination arrive with follower tasks (PK-03). `:15001` now serves
//! its real router. Ports bind plain for now (TLS is a runtime concern; tests bind :0).

use std::os::unix::io::RawFd;

use axum::{routing::get, Json, Router};
use serde_json::json;

/// systemd passes activated sockets starting at fd 3 (SD_LISTEN_FDS_START).
const SD_LISTEN_FDS_START: RawFd = 3;

/// Default plane ports (dev fallback binds these when not socket-activated).
#[derive(Debug, Clone)]
pub struct ServeConfig {
    pub machine_plane_port: u16,
    pub grpc_port: u16,
    pub enroll_port: u16,
    pub operator_port: u16,
}

impl Default for ServeConfig {
    fn default() -> Self {
        Self {
            machine_plane_port: 25000,
            grpc_port: 7443,
            enroll_port: 7444,
            operator_port: 15001,
        }
    }
}

/// Return the socket-activated listen fd (3) iff systemd handed us exactly this
/// process's sockets. Reads `LISTEN_PID` / `LISTEN_FDS` from the environment; the pure
/// parsing lives in [`parse_listen_fds`] so it is unit-testable without real env/pid.
pub fn sd_listen_fd() -> Option<RawFd> {
    let listen_pid = std::env::var("LISTEN_PID").ok();
    let listen_fds = std::env::var("LISTEN_FDS").ok();
    parse_listen_fds(
        std::process::id(),
        listen_pid.as_deref(),
        listen_fds.as_deref(),
    )
}

/// Pure sd_listen_fds check: returns `Some(3)` iff `LISTEN_PID` names *this* pid AND
/// `LISTEN_FDS` is >= 1. Any mismatch, unparseable value, or missing var → `None`
/// (dev fallback: bind the port directly). Injectable for tests.
pub fn parse_listen_fds(
    pid: u32,
    listen_pid: Option<&str>,
    listen_fds: Option<&str>,
) -> Option<RawFd> {
    let listen_pid: u32 = listen_pid?.trim().parse().ok()?;
    if listen_pid != pid {
        return None;
    }
    let count: i32 = listen_fds?.trim().parse().ok()?;
    if count >= 1 {
        Some(SD_LISTEN_FDS_START)
    } else {
        None
    }
}

/// A minimal `GET /healthz` router used by the TLS scaffolds until their real routes
/// land. Responds `200 {"service":"uaa-control","listener":"<name>"}`.
pub fn health_router(listener: &'static str) -> Router {
    Router::new().route(
        "/healthz",
        get(move || async move { Json(json!({ "service": "uaa-control", "listener": listener })) }),
    )
}

/// Bind and serve all four planes.
///
/// `:25000` uses the socket-activated fd when present (Decision 24), else a plain bind
/// on `config.machine_plane_port` (dev fallback). `:7443`/`:7444` are health scaffolds
/// (TLS wired later by PK-03); `:15001` serves the real operator router. Runtime-only;
/// unit tests exercise [`parse_listen_fds`] and the routers, never this bind loop.
pub async fn serve(config: ServeConfig) -> anyhow::Result<()> {
    let machine_listener = match sd_listen_fd() {
        Some(fd) => {
            tracing::info!(fd, "machine plane :25000 via systemd socket activation");
            listener_from_fd(fd)?
        }
        None => {
            let addr = format!("0.0.0.0:{}", config.machine_plane_port);
            tracing::info!(%addr, "machine plane binding directly (no socket activation)");
            tokio::net::TcpListener::bind(&addr).await?
        }
    };

    let grpc = bind(config.grpc_port).await?;
    let enroll = bind(config.enroll_port).await?;
    let operator = bind(config.operator_port).await?;

    let machine = tokio::spawn(async move {
        // ConnectInfo::<SocketAddr> is required: the /autoinstall/* seed handlers
        // (IP-01) read the client's TCP peer address for `ip neigh` MAC resolution
        // (Python parity). Without into_make_service_with_connect_info they 500.
        axum::serve(
            machine_listener,
            crate::machine_plane::router()
                .into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
    });
    let grpc = tokio::spawn(async move { axum::serve(grpc, health_router("grpc")).await });
    let enroll = tokio::spawn(async move { axum::serve(enroll, health_router("enroll")).await });
    let operator =
        tokio::spawn(async move { axum::serve(operator, crate::operator::router()).await });

    // Any listener exiting is fatal; surface the first error.
    let (m, g, e, o) = tokio::try_join!(machine, grpc, enroll, operator)?;
    m?;
    g?;
    e?;
    o?;
    Ok(())
}

async fn bind(port: u16) -> anyhow::Result<tokio::net::TcpListener> {
    Ok(tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await?)
}

/// Adopt an inherited raw fd (from systemd) as a tokio TCP listener.
fn listener_from_fd(fd: RawFd) -> anyhow::Result<tokio::net::TcpListener> {
    use std::os::unix::io::FromRawFd;
    // SAFETY: systemd guarantees fd 3 is an open, listening AF_INET socket for this
    // process when LISTEN_PID/LISTEN_FDS validate (checked in parse_listen_fds).
    let std_listener = unsafe { std::net::TcpListener::from_raw_fd(fd) };
    std_listener.set_nonblocking(true)?;
    Ok(tokio::net::TcpListener::from_std(std_listener)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_listen_fds() {
        // Matching pid + one fd → adopt fd 3.
        assert_eq!(parse_listen_fds(42, Some("42"), Some("1")), Some(3));
        // Matching pid + several fds → still starts at fd 3.
        assert_eq!(parse_listen_fds(42, Some("42"), Some("3")), Some(3));
        // pid mismatch → None (sockets are for a different process).
        assert_eq!(parse_listen_fds(42, Some("99"), Some("1")), None);
        // Zero fds → None.
        assert_eq!(parse_listen_fds(42, Some("42"), Some("0")), None);
        // Missing LISTEN_PID → None.
        assert_eq!(parse_listen_fds(42, None, Some("1")), None);
        // Missing LISTEN_FDS → None.
        assert_eq!(parse_listen_fds(42, Some("42"), None), None);
        // Unparseable values → None (never panic).
        assert_eq!(parse_listen_fds(42, Some("not-a-pid"), Some("1")), None);
        assert_eq!(parse_listen_fds(42, Some("42"), Some("xyz")), None);
    }
}
