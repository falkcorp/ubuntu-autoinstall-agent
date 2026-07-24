// file: crates/uaa-core/tests/component_equality_gate.rs
// version: 1.1.0
// guid: 3c9e2f1a-6b4d-4e0a-9f2b-7d1c5a8e6b02
// last-edited: 2026-07-24

//! Merge-blocking equality gate: hand-authored component fixture ->
//! `merge()` -> committed `InstallationConfig`, for all 5 committed hosts
//! (PS-GATE-15).
//!
//! Each `crates/uaa-core/tests/fixtures/components/<host>.yaml` hand-authors
//! an equivalent `HostGroupProfile` defaults blob + `HostProfile` overrides
//! blob (deserialized here via `serde_yaml::from_str` — the "raise" step,
//! NOT a function). Running the pair through
//! `uaa_core::profile::merge::merge(&group, &host)` (which internally lowers
//! via `uaa_core::profile::lower::lower`) must reproduce the SAME
//! `InstallationConfig` as the host's committed
//! `examples/configs/install/<host>.yaml`.
//!
//! This is deliberately NOT `lower(raise(committed)) == committed`
//! (tautological): the fixtures are independently hand-authored — mostly as
//! group-shared defaults + a thin per-host override, mirroring how a real
//! profile author would write them — not derived from the committed file by
//! any code path this crate owns. A drift here means `merge`/`lower` no
//! longer reproduce the fleet's actual behavior, not that a fixture is
//! stale.
//!
//! **Equality method:** `InstallationConfig` has no `PartialEq` (`TangServer`
//! blocks it, deliberately — see `config.rs`), so equality is proven via
//! canonical serialization, the SAME comparison the M2 gate
//! (`test_resolved_equals_committed_by_struct_equality` at
//! `crates/uaa/src/cli/config.rs`) uses: both the merged config and the
//! parsed committed config are re-serialized through the same `serde_yaml`
//! serializer, which eliminates the committed file's comments and any
//! omitted-default noise. Equal canonical YAML == equal structured value.
//!
//! **Scope note (deliberate, not an oversight):** this gate compares
//! `InstallationConfig` struct fields ONLY. Disk sizes (`esp_size`,
//! `reset_size`, `bpool_size`) and `reset_enabled` are NOT wire fields at
//! all (see `lower.rs`'s "Dropped fields" doc) — `InstallationConfig` has no
//! such fields to compare. So "the installer reproduces today's
//! unconditional RESET staging" is guaranteed by `disk_ops.rs` being
//! untouched (inert until PS-INSTALLER-29 wires the authored geometry
//! through), NOT by any assertion in this file. Do NOT attempt a
//! partition-geometry byte test here — it cannot pass without premature
//! installer wiring that is explicitly out of scope for this wave.

use uaa_core::network::InstallationConfig;
use uaa_core::profile::merge::merge;
use uaa_core::profile::validate::validate_resolved;
use uaa_core::profile::{HostGroupProfile, HostProfile};

/// The whole fixture file: a `HostGroupProfile` defaults blob plus a
/// `HostProfile` overrides blob, exactly what `merge()` takes.
#[derive(Debug, serde::Deserialize)]
struct ComponentFixture {
    group: HostGroupProfile,
    host: HostProfile,
}

/// The 5 committed hosts this gate covers — every fleet install config that
/// exists today (`len-serv-001/002/003`, `unimatrixone`, `vm-test`).
const GATED_HOSTS: [&str; 5] = [
    "len-serv-001",
    "len-serv-002",
    "len-serv-003",
    "unimatrixone",
    "vm-test",
];

fn fixture_path(host: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(format!(
        "{}/tests/fixtures/components/{host}.yaml",
        env!("CARGO_MANIFEST_DIR")
    ))
}

fn committed_path(host: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(format!(
        "{}/../../examples/configs/install/{host}.yaml",
        env!("CARGO_MANIFEST_DIR")
    ))
}

fn load_fixture(host: &str) -> ComponentFixture {
    let path = fixture_path(host);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("reading component fixture {}: {e}", path.display()));
    serde_yaml::from_str(&text)
        .unwrap_or_else(|e| panic!("parsing component fixture {}: {e}", path.display()))
}

fn load_committed(host: &str) -> InstallationConfig {
    let path = committed_path(host);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("reading committed config {}: {e}", path.display()));
    serde_yaml::from_str(&text)
        .unwrap_or_else(|e| panic!("parsing committed config {}: {e}", path.display()))
}

/// THE GATE: for every one of the 5 committed hosts, the hand-authored
/// component fixture must merge (and lower) to exactly the committed
/// `InstallationConfig` — canonical-YAML equality, per the module doc.
#[test]
fn test_component_fixtures_merge_to_committed_configs_for_all_5_hosts() {
    for host in GATED_HOSTS {
        let fixture = load_fixture(host);
        let (resolved, _provenance) = merge(&fixture.group, &fixture.host)
            .unwrap_or_else(|e| panic!("{host}: merge() failed: {e}"));
        let committed = load_committed(host);

        assert_eq!(
            serde_yaml::to_string(&resolved).unwrap(),
            serde_yaml::to_string(&committed).unwrap(),
            "{host}: component-fixture merge()+lower() output must equal the \
             committed InstallationConfig (canonical YAML form)"
        );
    }
}

/// `unimatrixone`'s fixture deliberately authors
/// `unlock_policy.tpm2_clevis_peer` (an authoring/validate-only nested
/// leaf — see `profile/components/unlock_policy.rs`'s module doc). Assert it
/// is accepted by the authoring schema but does NOT leak into the lowered
/// wire config: `InstallationConfig` has no such field, and every other
/// unlock leaf in the fixture is a flat override, so this specifically
/// exercises `lower()`'s per-leaf component-vs-flat fallback for
/// unimatrixone rather than a fully-nested `unlock_policy`.
#[test]
fn test_unimatrixone_tpm2_clevis_peer_is_authored_but_never_lowered() {
    let fixture = load_fixture("unimatrixone");

    // Sanity: the fixture actually authors the leaf under test — a fixture
    // edit that silently dropped it would make the rest of this test
    // vacuously true.
    let authored_peer = fixture
        .host
        .overrides
        .unlock_policy
        .as_ref()
        .and_then(|u| u.tpm2_clevis_peer);
    assert_eq!(
        authored_peer,
        Some(true),
        "fixture must author unlock_policy.tpm2_clevis_peer for this test to be meaningful"
    );

    let (resolved, _provenance) =
        merge(&fixture.group, &fixture.host).expect("unimatrixone fixture must merge");

    // Struct-shape proof: `InstallationConfig` has no `tpm2_clevis_peer`
    // field at all, so a round-trip through its own serializer can never
    // contain the key — belt-and-suspenders alongside the type-level
    // guarantee.
    let json = serde_json::to_value(&resolved).unwrap();
    assert!(
        !json.as_object().unwrap().contains_key("tpm2_clevis_peer"),
        "tpm2_clevis_peer must never appear in the lowered wire config"
    );
}

/// PS-MIG-U1-23 gate 2: `validate_resolved` must accept unimatrixone's
/// component-fixture merge output. The existing
/// `test_component_authored_fixture_resolves_through_registry`
/// (`crates/uaa-control/src/profiles/resolve.rs`) only exercises a synthetic
/// `comp-grp` fixture through `validate_resolved` — this is the first
/// assertion that the REAL unimatrixone fixture (NativeKeystore, D2-B) is
/// itself validate-clean, not just struct-equal to the committed file.
#[test]
fn test_unimatrixone_merge_output_passes_validate_resolved() {
    let fixture = load_fixture("unimatrixone");
    let (resolved, _provenance) =
        merge(&fixture.group, &fixture.host).expect("unimatrixone fixture must merge");
    validate_resolved(&resolved).expect("unimatrixone merge()+lower() output must be validate-clean");
}
