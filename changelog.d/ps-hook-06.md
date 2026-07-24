### Added

#### hooks authoring component: Phase enum + Hooks + HookStep (PS-HOOK-06)

New `crates/uaa-core/src/network/ssh_installer/components/hooks.rs` module,
part of wave 1 of the [Profile-System conversion](../docs/agent-tasks/profile-system/README.md).

- `Phase` — one variant per `run_phase!` label in `installer.rs`
  (`SetupVariables` .. `FinalSetup`), `Ord`/`Hash`-derived so it can key a
  `BTreeMap`. Deliberately separate from the existing `PhaseSelection`, whose
  fields are private and unusable as a map key.
- `HookStep { run, chroot }` — a single arbitrary command, run on the live
  ISO/host or inside the target chroot.
- `Hooks { pre_phase, post_phase }` — `BTreeMap<Phase, Vec<HookStep>>` pairs
  keyed by the phase they run immediately before/after, both
  `skip_serializing_if` empty.

Types only — nothing installer-side consumes these yet. No host's serialized
config or installer behavior changes.
