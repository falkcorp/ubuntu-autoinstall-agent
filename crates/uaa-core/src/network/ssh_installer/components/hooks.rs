// file: crates/uaa-core/src/network/ssh_installer/components/hooks.rs
// version: 1.1.0
// guid: 8574566e-c12f-4516-b66c-bffce50bb35c
// last-edited: 2026-07-23

//! `hooks` authoring types — arbitrary host-specific commands at named phase
//! points (like cloud-init late-commands).
//!
//! Types only: nothing here is wired into the installer or executed. This is
//! the Phase-0 authoring seam described in `docs/agent-tasks/profile-system/README.md`
//! — every type here is reachable but referenced by zero committed host.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A named point in the installer's phase sequence, one variant per
/// `run_phase!` label in `installer.rs` (see `SshInstaller::install`).
///
/// Deliberately NOT [`super::super::installer::PhaseSelection`] — that type's
/// `selected` field is private to its module and cannot key a map.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Phase {
    /// installer.rs:295 — "Phase 0: Setup variables"
    SetupVariables,
    /// installer.rs:299 — "Phase 1: Package installation"
    PackageInstall,
    /// installer.rs:303 — "Phase 2: Disk preparation"
    DiskPreparation,
    /// installer.rs:308 — "Phase 3: ZFS creation"
    ZfsCreation,
    /// installer.rs:323 — "Phase 4: Base system"
    BaseSystem,
    /// installer.rs:328 — "Phase 5: System configuration"
    SystemConfiguration,
    /// installer.rs:337 — "Phase 6: Final setup"
    FinalSetup,
}

/// A single hook command.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HookStep {
    /// The command to run, stored as-is (no validation).
    pub run: String,
    /// `true` — runs inside the target chroot. `false` — runs on the live
    /// ISO/host.
    pub chroot: bool,
}

/// Arbitrary host-specific commands keyed by the [`Phase`] they run
/// immediately before/after.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Hooks {
    /// Steps run immediately before the given phase.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub pre_phase: BTreeMap<Phase, Vec<HookStep>>,
    /// Steps run immediately after the given phase.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub post_phase: BTreeMap<Phase, Vec<HookStep>>,
}

impl Hooks {
    /// `true` when neither `pre_phase` nor `post_phase` carries any steps —
    /// the `skip_serializing_if` predicate for a `hooks` field wired onto
    /// [`InstallationConfig`](super::super::config::InstallationConfig), so an
    /// unhooked host omits the key entirely (same byte-identical discipline as
    /// `StorageMode::is_default`).
    pub fn is_empty(&self) -> bool {
        self.pre_phase.is_empty() && self.post_phase.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_empty_true_for_default() {
        assert!(Hooks::default().is_empty());
    }

    #[test]
    fn is_empty_false_when_pre_phase_populated() {
        let mut hooks = Hooks::default();
        hooks.pre_phase.insert(
            Phase::DiskPreparation,
            vec![HookStep {
                run: "echo pre-disk".to_string(),
                chroot: false,
            }],
        );
        assert!(!hooks.is_empty());
    }

    #[test]
    fn default_hooks_serializes_with_both_maps_omitted() {
        let hooks = Hooks::default();
        let json = serde_json::to_string(&hooks).expect("serialize");
        assert_eq!(json, "{}");
    }

    #[test]
    fn pre_phase_only_omits_post_phase() {
        let mut hooks = Hooks::default();
        hooks.pre_phase.insert(
            Phase::DiskPreparation,
            vec![HookStep {
                run: "echo pre-disk".to_string(),
                chroot: false,
            }],
        );
        let json = serde_json::to_value(&hooks).expect("serialize");
        let obj = json.as_object().expect("object");
        assert!(obj.contains_key("pre_phase"));
        assert!(!obj.contains_key("post_phase"));
    }

    #[test]
    fn both_populated_round_trips_without_loss() {
        let mut hooks = Hooks::default();
        hooks.pre_phase.insert(
            Phase::SetupVariables,
            vec![HookStep {
                run: "echo pre-setup".to_string(),
                chroot: false,
            }],
        );
        hooks.post_phase.insert(
            Phase::FinalSetup,
            vec![HookStep {
                run: "echo post-final".to_string(),
                chroot: true,
            }],
        );

        let json = serde_json::to_string(&hooks).expect("serialize");
        let round_tripped: Hooks = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(hooks, round_tripped);
    }
}
