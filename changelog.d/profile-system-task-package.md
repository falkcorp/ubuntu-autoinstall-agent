### Added

#### Profile-System conversion plan + 31-task agent package

A full investigation → design → tiered task breakdown for converting the
provisioning stack from standalone installer action-modules + one monolithic
`InstallationConfig` into a **composable component-profile system** built on the
existing `HostGroup`/`HostProfile`/merge registry.

- `docs/specs/profile-system-current-state.md` — how provisioning works today
  (SSH installer, the partial profile/registry layer, the legacy/dead pipelines).
- `docs/specs/profile-system-design.md` — the 9-component model (arch, role,
  disk-layout, unlock-policy, network, base-image, firmware-quirks, hooks,
  applications) + a 7-phase reversible migration that keeps the len-serv
  PlainLuks path byte-identical until an explicit double-gated migration.
- `docs/agent-tasks/profile-system/` — 31 self-contained `TASK-NN` briefs in
  12 dependency waves, each tagged with a recommended model tier (4 Haiku /
  24 Sonnet / 3 Opus) to minimise token cost, plus a README task-board.

Authored by a 47-agent orchestrated workflow (8 investigation scouts, opus
synthesis, a 3-lens adversarial judge panel, and per-brief Haiku cold-verify).
Planning only — no installer behavior changes in this commit.
