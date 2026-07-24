<!-- file: changelog.d/ps-merge-13.md -->
<!-- version: 1.0.0 -->
<!-- guid: 3b8a1f6d-2c4e-4a9b-9d1e-7f0c5a2e8b31 -->
<!-- last-edited: 2026-07-24 -->

### Changed

#### Profile merge resolves nested components and flattens through `lower()` (PS-MERGE-13)

`profile::merge::merge` keeps its `(InstallationConfig, Provenance)` signature
but no longer hand-builds the flat config. It now resolves both tiers into one
RESOLVED `InstallationConfigPartial` — flat fields plus the nested components
(`disk_layout`, `unlock_policy`, `network`, `base_image`, `firmware_quirks`) —
and flattens it through the pure `lower()` (PS-LOWER-12), so the authoring→wire
mapping lives in exactly one place.

Component resolution is additive: a host that authors no component keys resolves
to today's output byte-for-byte (the len-serv `PlainLuks` fleet and U1 are
unchanged, proven by the M2 struct-equality gate). Variant-select components
(`disk_layout`, `applications`, `firmware_quirks`) whole-replace by kind — a
same-kind `disk_layout` allows a single-field partial override — while
field-components (`unlock_policy`, `network`, `base_image`) merge each leaf
independently, host winning per leaf. Provenance now carries additive
component-path keys (e.g. `unlock-policy.tang.threshold`, `disk-layout`)
alongside the unchanged flat keys.
