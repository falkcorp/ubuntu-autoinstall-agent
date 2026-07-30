<!-- file: changelog.d/applications-five-new-variants.md -->
<!-- version: 1.0.0 -->
<!-- guid: 9f4b7d23-6c18-4a05-b3e9-7d21c8f40a96 -->
<!-- last-edited: 2026-07-30 -->

### Added

#### Five new composable `ApplicationSpec` variants

`ApplicationSpec` gains `cockroach-rollout-agent`, `prometheus-node-exporter`,
`canonical-livepatch`, `report-status` and `zsh`, each with an applier in
`ApplicationInstaller` (Phase 5, fail-closed). They are tier-agnostic: any
group or host profile can attach any of them, unioned by kind with the host
winning whole-entry. This closes most of the `app-specs` TODO — a rebuilt host
now returns configured rather than needing manual post-install steps.
`landscape-client` is deliberately NOT modelled.

Unit shapes and script contents are transcriptions of what is actually running
on len-serv-001/002 (read 2026-07-30), not invented. Two behaviors worth
knowing:

- Secret-bearing fields (`canonical-livepatch.key`,
  `cockroach-rollout-agent.database-url`) author as `REPLACE_AT_PLACE_TIME`.
  Livepatch installs the snap but SKIPS `canonical-livepatch enable` while the
  placeholder is present, rather than running `enable REPLACE_AT_PLACE_TIME`
  and failing the whole install for what is an authoring state.
- `cockroach-rollout-agent` defaults to `enabled: false`, matching the fleet's
  real state (installed but disabled on len-serv-003 as of 2026-07-28).

#### `ApplicationSpec::kind()` — single source of truth for wire tags

The `cockroach`/`tang-server` strings were duplicated across
`applications.rs::reject_duplicates`, `profile/merge.rs::app_kind` and
`scripts/vm-validate.sh`, with nothing tying them together; adding a variant
meant remembering three edits, and a missed one silently broke
duplicate-rejection or merge-by-kind with no compile error. Both Rust call
sites now delegate to one exhaustive match, so a new variant without a tag is
a compile error, and `kind_tags_match_serde` pins the strings against actual
serialization.

### Fixed

#### Cockroach flags reproduced len-serv-003's drift instead of the fleet form

The installer hardcoded `--store=/var/lib/cockroach/data` and derived
`--listen-addr`/`--sql-addr` from the host IP — which is precisely
len-serv-003's drifted configuration, and why
`cockroach sql --host=127.0.0.1:36257` is refused there but works on
len-serv-001/002. Redeploying 003 would have faithfully recreated the outlier
the pre-wipe inventory says to standardize away from.

`--listen-addr` and `--sql-addr` are now port-only and `--store` comes from a
new `CockroachSpec::store` field defaulting to the 001/002 value
(`path=/var/lib/cockroach/cockroach-data,attrs=ssd,size=.5`). `--advertise-addr`
deliberately stays `IP:port` — it is the address peers dial, and a port-only
advertise breaks cluster join; a test now asserts that asymmetry explicitly.
The data directory is derived from the `--store` value via `store_directory`,
which handles both the bare-path and `path=…,attrs=…` forms; without it
`mkdir -p` would create a directory literally named `path=...,attrs=ssd,size=.5`.

#### VM gate probed only the first application

`scripts/vm-validate.sh` stage 6 read `applications[0].kind` and stopped, so on
a host with several applications — the entire point of composable specs — it
proved one and silently reported PASS for the rest. It now loops over every
declared kind, with readiness probes added for the five new variants. Unknown
kinds still fail closed.

### Changed

`examples/configs/install/len-serv-003-native-keystore.yaml` attaches the six
applications at the HOST tier. Deliberately not the len-serv group: 001/002 are
in-service CockroachDB nodes under the byte-identical-until-wave-7-10 rule
enforced by the parse→merge equality gate, and group-tier authoring would
change their resolved artifacts too.

A new `every_committed_install_config_parses` test deserializes every committed
`examples/configs/install/*.yaml` into `InstallationConfig`, so a typo'd key or
unknown `kind` fails at test time instead of at the moment a machine is being
rebuilt.
