// file: crates/uaa-control/src/machine_plane/staleness.rs
// version: 1.0.0
// guid: a170f9d1-5d14-4948-91a5-70b220dacf29
// last-edited: 2026-07-18

//! Read-time application-health staleness (DS-CHK-03).
//!
//! The machine plane writes `app_reports` / `last_app_status_at` only on
//! check-in (DS-CHK-02) — nothing flips a status on absence, because there is
//! no reaper and no TTL sweep. Left alone, a machine whose Cockroach died, or
//! whose NIC died, or which never booted after a reinstall, keeps its
//! last-known-good health in the snapshot forever and renders green.
//!
//! That is strictly worse than showing nothing: it actively asserts health
//! for a dead box. The fix is a pure, clock-injected classification computed
//! at READ time from [`crate::db::MachineRow::last_app_status_at`] — no new
//! persistence, no background job, and the ingest path stays fail-open.

use tracing::warn;

// `last_app_status_at` (like `last_seen`/`updated_at` elsewhere in this
// crate — see `machine_plane::lifecycle::now_epoch_string`) is a decimal
// Unix-epoch-seconds string, NOT RFC3339. `freshness()` must parse the same
// shape the ingest path actually writes.

/// A machine that reported `active: true` an hour ago is `Stale`, NOT
/// healthy. `Stale` means "we don't know", which is different from
/// unhealthy AND different from healthy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freshness {
    /// Reported within [`APP_STATUS_STALE_AFTER_SECS`] of `now`.
    Fresh,
    /// Reported, but longer ago than the threshold. Not the same as
    /// unhealthy — it means we don't know.
    Stale,
    /// `last_app_status_at` is `None`, or the timestamp could not be
    /// parsed. Also not unhealthy, and not healthy.
    NeverReported,
}

/// A small multiple of the expected report interval: long enough that a
/// slow host is not flagged, short enough that a dead one stops reading
/// green. Check-ins are expected roughly every few minutes; 900s (15
/// minutes) tolerates a handful of missed/delayed check-ins before the
/// dashboard stops trusting the last-known health.
pub const APP_STATUS_STALE_AFTER_SECS: i64 = 900;

/// Read-time only. `now_unix` is injected so this is testable without a
/// clock, and so no background job is needed.
///
/// `last_app_status_at` is a decimal Unix-epoch-seconds string — the same
/// shape `machine_plane::lifecycle::now_epoch_string` writes for
/// `last_seen`/`updated_at` — not RFC3339.
///
/// Edge semantics:
/// - `None` -> [`Freshness::NeverReported`] — never reported is a different
///   fact from stopped reporting.
/// - Unparseable timestamp -> [`Freshness::NeverReported`] plus a `warn` log
///   naming `mac` — a corrupt timestamp must not read as healthy.
/// - Timestamp in the future (clock skew) -> [`Freshness::Fresh`] — a skewed
///   clock is not a health signal, and erroring here would take a dashboard
///   read down.
pub fn freshness(last_app_status_at: Option<&str>, now_unix: i64, mac: &str) -> Freshness {
    let Some(raw) = last_app_status_at else {
        return Freshness::NeverReported;
    };

    let Ok(reported_at) = raw.trim().parse::<i64>() else {
        warn!(%mac, timestamp = raw, "staleness: unparseable last_app_status_at, treating as NeverReported");
        return Freshness::NeverReported;
    };

    if reported_at > now_unix {
        // Clock skew: a timestamp from the future is not evidence of
        // staleness, and must not be treated as an error.
        return Freshness::Fresh;
    }

    if now_unix - reported_at < APP_STATUS_STALE_AFTER_SECS {
        Freshness::Fresh
    } else {
        Freshness::Stale
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAC: &str = "aa:bb:cc:dd:ee:ff";

    #[test]
    fn test_recent_report_is_fresh() {
        let now = 1_700_000_000_i64;
        let ts = (now - 60).to_string();
        assert_eq!(freshness(Some(&ts), now, MAC), Freshness::Fresh);
    }

    /// This is the bug: a report of `active: true` from 2 hours ago must not
    /// read as healthy.
    #[test]
    fn test_old_healthy_report_is_stale_not_fresh() {
        let now = 1_700_000_000_i64;
        let ts = (now - 2 * 3600).to_string();
        assert_eq!(freshness(Some(&ts), now, MAC), Freshness::Stale);
    }

    #[test]
    fn test_never_reported_is_never_reported() {
        let now = 1_700_000_000_i64;
        assert_eq!(freshness(None, now, MAC), Freshness::NeverReported);
    }

    #[test]
    fn test_unparseable_timestamp_is_never_reported() {
        let now = 1_700_000_000_i64;
        assert_eq!(
            freshness(Some("not-a-timestamp"), now, MAC),
            Freshness::NeverReported
        );
    }

    /// `last_app_status_at` is a decimal Unix-epoch string (matching
    /// `last_seen`/`updated_at`'s convention), not RFC3339 — an RFC3339
    /// string must be rejected the same as any other garbage, never silently
    /// accepted as "close enough".
    #[test]
    fn test_rfc3339_timestamp_is_never_reported() {
        let now = 1_700_000_000_i64;
        let ts = chrono::DateTime::from_timestamp(now - 60, 0).unwrap().to_rfc3339();
        assert_eq!(freshness(Some(&ts), now, MAC), Freshness::NeverReported);
    }

    #[test]
    fn test_future_timestamp_is_fresh() {
        let now = 1_700_000_000_i64;
        let ts = (now + 3600).to_string();
        assert_eq!(freshness(Some(&ts), now, MAC), Freshness::Fresh);
    }

    /// "No applications" != "no news": a host running zero applications that
    /// reported a minute ago is healthy, not unknown.
    #[test]
    fn test_empty_reports_with_recent_timestamp_is_fresh() {
        let now = 1_700_000_000_i64;
        let ts = (now - 60).to_string();
        // `app_reports` itself is not an input to `freshness` — this test
        // documents the contract that emptiness must not be conflated with
        // staleness; the timestamp alone drives the result.
        assert_eq!(freshness(Some(&ts), now, MAC), Freshness::Fresh);
    }

    /// Pin the boundary explicitly so a future refactor cannot flip `<` to
    /// `<=` (or vice versa) unnoticed.
    #[test]
    fn test_boundary_exactly_at_threshold() {
        let now = 1_700_000_000_i64;
        let ts = (now - APP_STATUS_STALE_AFTER_SECS).to_string();
        assert_eq!(freshness(Some(&ts), now, MAC), Freshness::Stale);
    }
}
