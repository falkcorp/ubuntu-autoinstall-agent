// file: crates/uaa-control/src/profiles/alloc.rs
// version: 0.2.0
// guid: 04fc940a-9b96-44dc-a724-55b6c8069818
// last-edited: 2026-07-17

//! Pure allocation arithmetic for DS-REG-03 — the testable half of
//! `allocate_index`/`rebind`, split out so it can be exercised over plain
//! slices with no store, no filesystem, and no mocks.
//!
//! None of these functions read a snapshot. The fail-CLOSED read
//! ([`read_snapshot_strict`][crate::db::store::read_snapshot_strict]) lives in
//! [`SnapshotProfileStore`][crate::profiles::store::SnapshotProfileStore]; this
//! module only computes over a doc/slice a caller has ALREADY read strictly.

use std::collections::BTreeSet;

use anyhow::{anyhow, Result};

use crate::db::store::SnapshotDoc;
use crate::db::HostnameAllocationRow;

/// Next index for a group: `max(existing) + 1`, starting at 1. **Never** "lowest
/// free" — indices are never reused, so a released or rebound-away row is still
/// counted. The caller passes EVERY row for the group (active, released, and
/// tombstoned), so a machine added later with a numerically lower MAC gets the
/// next index rather than reshuffling an existing binding (spec D18 / the
/// operator's original bug).
pub fn next_index(existing: &[HostnameAllocationRow]) -> i64 {
    existing
        .iter()
        .map(|a| a.index)
        .max()
        .map(|max| max + 1)
        .unwrap_or(1)
}

/// Render a hostname from a group's `hostname_pattern` + `name` + `index`.
/// Supports `{name}` and a Rust-style `{index}` / `{index:0<width>}` (e.g.
/// `"{name}-{index:03}"` -> `"len-serv-001"`). An unterminated, absent, or
/// unsupported `{index...}` placeholder is an `Err`, never a silently wrong
/// name.
pub fn render_hostname(pattern: &str, name: &str, index: i64) -> Result<String> {
    let with_name = pattern.replace("{name}", name);

    let start = with_name
        .find("{index")
        .ok_or_else(|| anyhow!("hostname pattern {pattern:?} has no {{index}} placeholder"))?;
    let rel_end = with_name[start..]
        .find('}')
        .ok_or_else(|| anyhow!("hostname pattern {pattern:?} has an unterminated {{index placeholder"))?;
    let token = &with_name[start..=start + rel_end];
    // Strip the leading "{index" and trailing "}" -> "" or ":03".
    let spec = &token[6..token.len() - 1];

    let rendered_index = if spec.is_empty() {
        index.to_string()
    } else if let Some(width) = spec.strip_prefix(":0") {
        let width: usize = width
            .parse()
            .map_err(|_| anyhow!("unsupported index width in hostname pattern {pattern:?}"))?;
        format!("{index:0width$}")
    } else {
        return Err(anyhow!(
            "unsupported index format {spec:?} in hostname pattern {pattern:?}"
        ));
    };

    Ok(with_name.replacen(token, &rendered_index, 1))
}

/// Every materialized hostname across ALL groups' allocations, plus every
/// `hostname_override` — the global-uniqueness input (spec D2). `hostname_pattern`
/// is free-form, so two groups can render the same name from different prefixes;
/// uniqueness is therefore checked against this whole set, never per-group.
/// Released and rebound-away rows are included: their names are still spoken for
/// (a released machine keeps its name; a rebound row shares its name with the new
/// identity's row, which the set dedups).
pub fn taken_hostnames(doc: &SnapshotDoc) -> BTreeSet<String> {
    let mut taken = BTreeSet::new();
    for alloc in &doc.hostname_allocations {
        taken.insert(alloc.hostname.clone());
    }
    for profile in &doc.host_profiles {
        if let Some(hostname) = &profile.hostname_override {
            taken.insert(hostname.clone());
        }
    }
    taken
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn alloc_row(group_id: Uuid, identity: &str, index: i64, hostname: &str) -> HostnameAllocationRow {
        HostnameAllocationRow {
            group_id,
            identity: identity.to_string(),
            index,
            hostname: hostname.to_string(),
            allocated_at: None,
            released_at: None,
            rebound_to: None,
        }
    }

    /// An empty snapshot built by an explicit struct literal (deliberately not
    /// the fail-open default constructor, which the allocation-path grep gate
    /// forbids anywhere in this file) — `taken_hostnames` needs a doc to read.
    fn empty_doc() -> SnapshotDoc {
        SnapshotDoc {
            machines: vec![],
            enrollments: vec![],
            yubikeys: vec![],
            luks_credentials: vec![],
            tang_servers: vec![],
            discovered_macs: vec![],
            host_groups: vec![],
            host_profiles: vec![],
            hostname_allocations: vec![],
            profile_versions: vec![],
        }
    }

    #[test]
    fn test_next_index_starts_at_one_when_empty() {
        assert_eq!(next_index(&[]), 1);
    }

    #[test]
    fn test_next_index_is_max_plus_one_never_lowest_free() {
        let g = Uuid::new_v4();
        // Indices 1 and 3 present (2 was released and its row removed from this
        // slice would still count if present) — a gap must NOT be filled.
        let rows = vec![
            alloc_row(g, "a", 1, "len-001"),
            alloc_row(g, "c", 3, "len-003"),
        ];
        assert_eq!(next_index(&rows), 4, "must be max+1, never the free slot 2");
    }

    #[test]
    fn test_next_index_counts_released_rows() {
        let g = Uuid::new_v4();
        let mut released = alloc_row(g, "b", 2, "len-002");
        released.released_at = Some("2026-01-01".into());
        let rows = vec![alloc_row(g, "a", 1, "len-001"), released];
        assert_eq!(next_index(&rows), 3, "a released index is still counted");
    }

    #[test]
    fn test_render_hostname_zero_pads() {
        assert_eq!(
            render_hostname("{name}-{index:03}", "len-serv", 1).unwrap(),
            "len-serv-001"
        );
    }

    #[test]
    fn test_render_hostname_different_prefix_same_result() {
        // The spec's collision example: two patterns render the same hostname.
        assert_eq!(
            render_hostname("{name}-serv-{index:03}", "len", 1).unwrap(),
            "len-serv-001"
        );
    }

    #[test]
    fn test_render_hostname_bare_index() {
        assert_eq!(
            render_hostname("{name}-{index}", "host", 42).unwrap(),
            "host-42"
        );
    }

    #[test]
    fn test_render_hostname_missing_placeholder_errors() {
        assert!(render_hostname("{name}-static", "len", 1).is_err());
    }

    #[test]
    fn test_taken_hostnames_unions_allocations_and_overrides() {
        let g = Uuid::new_v4();
        let mut doc = empty_doc();
        doc.hostname_allocations.push(alloc_row(g, "a", 1, "len-001"));
        doc.host_profiles.push(crate::db::HostProfileRow {
            id: Uuid::new_v4(),
            group_id: g,
            identity: "b".into(),
            hostname_override: Some("special-box".into()),
            overrides: serde_json::json!({}),
            applications: serde_json::json!([]),
            content_hash: vec![],
            version: 1,
            created_at: None,
            updated_at: None,
        });

        let taken = taken_hostnames(&doc);
        assert!(taken.contains("len-001"));
        assert!(taken.contains("special-box"));
    }
}
