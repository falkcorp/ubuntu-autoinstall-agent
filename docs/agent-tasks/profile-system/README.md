<!-- file: docs/agent-tasks/profile-system/README.md -->
<!-- version: 1.0.0 -->
<!-- guid: 56c4d14f-5a7c-421f-859d-f8136b5d080c -->
<!-- last-edited: 2026-07-23 -->

# Agent Task Package — Profile-System Conversion

Turn the provisioning stack's **standalone action-modules + one monolithic
`InstallationConfig`** into a **composable component-profile system** built on top
of the existing `HostGroup`/`HostProfile`/merge registry — so we stop rebuilding
one-off installers every time a host needs a change. Covers **len-serv** (amd64 /
Cockroach / PlainLuks), **U1** (ZFS-native / clevis D2-B), and **rpi-serv**
(arm64 / Tang).

- **Design spec:** [`../../specs/profile-system-design.md`](../../specs/profile-system-design.md)
- **Current state:** [`../../specs/profile-system-current-state.md`](../../specs/profile-system-current-state.md)
- Authored by a 47-agent investigate→design→plan workflow (45 candidate components across 8 areas; 3-lens adversarial judge panel; every brief cold-verified by a Haiku agent).

Each `TASK-NN-*.md` is a **self-contained brief** a single agent runs end to end
(own git worktree → branch → PR → `gh pr merge --rebase`). Model tier is chosen to
**minimise token cost**: 4 Haiku, 24 Sonnet, 3 Opus.

## Recommended subagent roster

| Tier | Use for | Tasks |
|------|---------|-------|
| **Haiku-class** | mechanical: one new types-only module / enum, no cross-cutting logic | TASK-02, TASK-05, TASK-07, TASK-09 |
| **Sonnet-class** | a self-contained component + its wiring + tests | the 24 component / wiring / migration briefs |
| **Opus-class** | cross-cutting seams, provenance/rollback, multi-module refactor | TASK-15, TASK-18, TASK-29 |

## Component model (the target)

- **arch (classifier)** — Declares CPU/boot architecture (amd64|arm64). A CLASSIFIER, not a payload component: it gates which variants of other components are legal (arm64 forbids x86-only quirks; today's dracut/tpm2/grub chroot phases are x86-oriented). Resolves the implicit amd64 assumption baked throughout system_setup.rs into one explicit field and is the single fact that makes an rpi host EXPRESSIBLE (not yet bootable — see open questions).
- **role (classifier)** — Declares what the host IS (install-target | tang-server). A CLASSIFIER: gates whether the full disk/unlock/app pipeline runs (install-target) or a minimal path (tang-server). Gives rpi/tang a first-class representation the current InstallationConfig entirely lacks — it modeled Tang only as a CONSUMER, never a server.
- **disk-layout** — Owns whole-disk strategy: partition geometry, encryption container topology, pool/vdev shape, RESET-partition opt-in. Generalizes StorageMode + the two parallel appliers (disk_ops.rs PlainLuks vs disk_native.rs+layout.rs NativeKeystore) into one addressable variant-selected component. This is the ONE component that genuinely needs variant-select whole-replace semantics and therefore the lowering seam.
- **unlock-policy** — Owns the orthogonal factors that unlock the root/keystore LUKS at boot: Tang SSS, TPM2+PIN cryptenroll keyslot, TPM2-peer clevis SSS share (D2-B), FIDO2 intent. Replaces six flat fields plus the storage_mode-conditioned include_tpm2_peer bool with one cohesive AUTHORING object.
- **network** — Interface, addressing (static/DHCP), routing, DNS, netplan renderer. Replaces the magic network_address=="dhcp" string sentinel with a typed addressing enum, making an inconsistent renderer/address combination unrepresentable.
- **base-image** — Target rootfs bootstrap: Ubuntu release + mirror, initramfs generator, arch-aware mirror/agent selection. Folds debootstrap_release/mirror/initramfs_type into one AUTHORING unit; surfaces the hardcoded old-releases fallback as a field.
- **firmware-quirks** — Board/BIOS workarounds that today are sniffed or applied unconditionally. Turns each undeclared assumption into an explicit declared per-host quirk. NOTE: serial-console and nvme-cant-boot are DELIBERATELY EXCLUDED from this list — see decisions (serial-console is an arch-implied lower()-injected default that never appears in a host's serialized vec; nvme-cant-boot is already owned by DiskRole::System).
- **hooks** — Arbitrary host-specific commands at named phase points (like cloud-init late-commands). Fills the total gap: today every host-specific behavior needs a NEW hardcoded installer branch. The ONE data-driven extension slot that ends the rebuild-one-off-per-change pattern.
- **applications** — Post-install workloads. Already the exemplar variant-select component (closed ApplicationSpec enum, deny_unknown_fields, by-kind union, CockroachSpecPartial field-override). The model every variant-select component copies. Needs one new variant for rpi and to remove the LENSERV_MEMBER_IPS hardcode.

## Migration phases (staged, reversible; len-serv byte-identical until waves 7–10)

- **Phase 0 — Land component authoring types + lower() + validate_resolved() behind an unused seam** — Define every component sub-struct/enum + per-variant partial + the NEW Phase enum + the NEW Arch enum (in ssh_installer::config) + pure lower() + validate_resolved(), all reachable but referenced by ZERO committed host. Add a NON-tautological gate: compare lower() of a HAND-AUTHORED component profile for each of the 5 committed hosts against that host's currently-committed/placed InstallationConfig (struct equality) — NOT a lower(raise(committed))==committed round-trip, which would be tautological. Make it a merge-blocking CI check. _(risk: Minimal — additive types, no host uses them, no serialized artifact changes.)_
- **Phase 1 — New axes + one EARLY installer-consumption vertical slice (proves the model)** — Add arch/role/firmware-quirks/hooks as additive skip-if-empty fields whose DEFAULTS reproduce today byte-for-byte. Serial-console is handled as an ARCH-IMPLIED default that lower() injects unconditionally for amd64 install-targets — it NEVER appears in a host's serialized firmware_quirks vec, so the empty-vec byte-identical claim holds (see decisions). Replace disk_device.starts_with('/dev/md') IMSM sniffing with component-driven behavior (IMSM being dropped, so this is a removal). CRUCIALLY, to prove the payoff is not all deferred: refactor ONE installer decision now — derive CockroachSpec advertise/join membership from the group allocation roster (retiring LENSERV_MEMBER_IPS, applications.rs:26) — so at least one live one-off hardcode is genuinely deleted in the first working phase, gated by 'derived value == constant output for current fleet'. _(risk: Low — defaults reproduce current behavior; rollback-safe additive fields. The cockroach-membership slice is guarded by an equality check against the retired constant.)_
- **Phase 2 — Deploy component-aware CONTROL binary fleet-wide (rollback-safety prerequisite)** — Ship and deploy the control-plane binary that KNOWS every new component key, and add a stored schema_version on HostGroupRow/HostProfileRow that the control binary refuses to serve/roll-back below. This is the expand step of expand-then-migrate: because InstallationConfigPartial is deny_unknown_fields, a blob carrying a component key parsed by an OLDER binary fail-closes group-wide — so NO authoring blob may gain a component key until every control binary in the fleet recognizes it. This phase adds recognition + the version gate; it migrates zero blobs. _(risk: Low — binary knows the keys but no blob uses them yet; version gate is fail-loud, not fail-silent-wrong.)_
- **Phase 3 — Migrate U1 (unimatrixone) to component authoring** — Re-author the unimatrixone group/profile using disk-layout=zfs-native-keystore + unlock-policy(tang t=2 + tpm2-peer). U1 is the proving ground: a STANDALONE single-host group, so a control rollback below its schema_version fail-closes only U1, not a fleet. No len-serv byte-identical constraint applies here. Placeholder-survival test for the keystore luks_key + tpm2 fields must pass before ship. _(risk: Medium — must re-pass the D2-B clevis/tang VM gate before U1 hardware; blast radius contained to one host.)_
- **Phase 4 — Author rpi-serv group (EXPRESSIBILITY + validation only, NOT a bootable install)** — Author an rpi-serv group: arch=arm64, role=tang-server, base-image=ubuntu-arm64, applications=[tang-server], firmware-quirks[watchdog-staggered]. Proves ONE interface expresses all three host classes at the authoring/validation layer. Does NOT touch len-serv or U1. EXPLICIT scope: this delivers expressibility + validate_resolved() coverage, NOT a proven-bootable arm64 tang-server install — that needs a per-phase x86-assumption audit (open question). _(risk: Low to the existing fleet (net-new isolated group). Honest medium on the arm64 provisioning gap, which is out of scope here.)_
- **Phase 5 — Migrate the SHARED len-serv group, one component at a time, DOUBLE-gated** — Convert the len-serv group to component authoring in order network -> base-image -> unlock-policy -> disk-layout (riskiest last). len-serv is a SINGLE SHARED HostGroupProfile serving all 3 CockroachDB nodes, so this is the highest-blast-radius phase — it runs ONLY after Phase 2's fleet-wide component-aware control binary + schema_version gate are deployed and verified, with a documented operational rule: 'no control-plane rollback below schema_version X once len-serv defaults are migrated.' EACH component step must pass BOTH (a) the Phase-0 hand-authored-lower()==committed equality gate as a merge-blocking check on that specific PR, and (b) a placeholder-survival test for any secret-bearing field it touches (unlock-policy.tpm2_pin, disk-layout luks_key). _(risk: Medium-high — group-wide fail-closed-on-rollback is neutralized by Phase 2's version gate; byte-identical drift on the hardcoded GPT geometry is caught by the equality gate per step.)_
- **Phase 6 — Installer consumes typed resolved fields; retire dead paths** — Refactor installer modules to read the resolved config through the formalized struct{runner:&mut dyn CommandExecutor}+async fn(&config) trait shape, module by module. Retire the legacy autoinstall/render.rs template (a live second source of truth for len-serv) and the dead image/deployer.rs+customizer.rs+TargetConfig path (which owns the legacy Architecture enum). Flip only after the whole fleet's INSTALLER binary is component-aware. _(risk: Highest — this is the one phase that changes what a module reads and can threaten the rollback-parse guarantee; keep lower()->flat InstallationConfig as the fallback wire format until every installer binary is upgraded.)_

## Task board (dependency-wave order)

| Task | Id | Title | Priority | Effort | **Agent** | Wave | Deps |
|------|----|-------|----------|--------|-----------|------|------|
| [TASK-01](TASK-01-ps-app-09.md) | PS-APP-09 | add TangServer variant to ApplicationSpec | P1 | M | **Sonnet-class** | 1 | — |
| [TASK-02](TASK-02-ps-arch-07.md) | PS-ARCH-07 | Arch classifier enum (NEW, in ssh_installer::config) | P1 | S | **Haiku-class** | 1 | — |
| [TASK-03](TASK-03-ps-disk-01.md) | PS-DISK-01 | disk-layout component types + per-variant partials | P1 | M | **Sonnet-class** | 1 | — |
| [TASK-04](TASK-04-ps-hook-06.md) | PS-HOOK-06 | hooks types: new Phase enum + Hooks + HookStep | P1 | M | **Sonnet-class** | 1 | — |
| [TASK-05](TASK-05-ps-img-04.md) | PS-IMG-04 | base-image authoring sub-struct (BaseImagePartial) | P1 | S | **Haiku-class** | 1 | — |
| [TASK-06](TASK-06-ps-imsm-17.md) | PS-IMSM-17 | remove /dev/md IMSM sniffing (all call sites + tests) | P1 | M | **Sonnet-class** | 1 | — |
| [TASK-07](TASK-07-ps-net-03.md) | PS-NET-03 | network authoring sub-struct + Addressing enum | P1 | S | **Haiku-class** | 1 | — |
| [TASK-08](TASK-08-ps-quirk-05.md) | PS-QUIRK-05 | firmware-quirks closed enum + Vec type | P1 | M | **Sonnet-class** | 1 | — |
| [TASK-09](TASK-09-ps-role-08.md) | PS-ROLE-08 | HostRole classifier enum | P1 | S | **Haiku-class** | 1 | — |
| [TASK-10](TASK-10-ps-unlock-02.md) | PS-UNLOCK-02 | unlock-policy authoring sub-struct (UnlockPolicyPartial) + TangServer PartialEq | P1 | M | **Sonnet-class** | 1 | — |
| [TASK-11](TASK-11-ps-vmgate-19.md) | PS-VMGATE-19 | make VM-gate readiness probe role/application-driven | P1 | M | **Sonnet-class** | 1 | — |
| [TASK-12](TASK-12-ps-wire-axes-10.md) | PS-WIRE-AXES-10 | wire arch/role/firmware_quirks/hooks onto InstallationConfig (additive, byte-identical) | P1 | M | **Sonnet-class** | 2 | 2,9,8,4 |
| [TASK-13](TASK-13-ps-wire-partial-11.md) | PS-WIRE-PARTIAL-11 | wire component sub-structs onto InstallationConfigPartial (additive) | P1 | M | **Sonnet-class** | 2 | 3,10,7,5,8,4,2,9 |
| [TASK-14](TASK-14-ps-lower-12.md) | PS-LOWER-12 | lower(): pure total authoring->flat-wire bridge | P1 | L | **Sonnet-class** | 3 | 12,13,1 |
| [TASK-15](TASK-15-ps-schema-20.md) | PS-SCHEMA-20 | schema_version row gate + component-aware control binary (expand step) | P1 | L | **Opus-class** | 3 | 13 |
| [TASK-16](TASK-16-ps-serial-18.md) | PS-SERIAL-18 | serial-console as arch-gated installer default (serialization-safe) | P1 | M | **Sonnet-class** | 3 | 12 |
| [TASK-17](TASK-17-ps-validate-14.md) | PS-VALIDATE-14 | validate_resolved(&InstallationConfig) composition-legality sibling | P1 | M | **Sonnet-class** | 3 | 12 |
| [TASK-18](TASK-18-ps-merge-13.md) | PS-MERGE-13 | merge(): component resolvers + component-path provenance (additive) | P1 | L | **Opus-class** | 4 | 13,14 |
| [TASK-19](TASK-19-ps-cockroach-16.md) | PS-COCKROACH-16 | derive cockroach advertise/join from group roster; retire LENSERV_MEMBER_IPS | P1 | M | **Sonnet-class** | 5 | 18 |
| [TASK-20](TASK-20-ps-gate-15.md) | PS-GATE-15 | merge-blocking equality gate: parse->merge == committed for 5 hosts | P1 | M | **Sonnet-class** | 5 | 18 |
| [TASK-21](TASK-21-ps-pipeline-21.md) | PS-PIPELINE-21 | wire validate_resolved into resolve path + prove component fixture resolves | P1 | M | **Sonnet-class** | 5 | 18,15,17 |
| [TASK-22](TASK-22-ps-placeholder-22.md) | PS-PLACEHOLDER-22 | placeholder-survival test harness (parse->merge) | P1 | M | **Sonnet-class** | 5 | 18 |
| [TASK-23](TASK-23-ps-mig-rpi-24.md) | PS-MIG-RPI-24 | author rpi-serv group (expressibility + validation only) | P2 | M | **Sonnet-class** | 6 | 17,1,12,18,24 |
| [TASK-24](TASK-24-ps-mig-u1-23.md) | PS-MIG-U1-23 | migrate unimatrixone (U1) to component authoring | P2 | M | **Sonnet-class** | 6 | 21,20,22,17 |
| [TASK-25](TASK-25-ps-mig-len-net-25.md) | PS-MIG-LEN-NET-25 | migrate len-serv group: network component (step 1, lowest risk) | P2 | S | **Sonnet-class** | 7 | 24 |
| [TASK-26](TASK-26-ps-mig-len-img-26.md) | PS-MIG-LEN-IMG-26 | migrate len-serv group: base-image component (step 2) | P2 | S | **Sonnet-class** | 8 | 25 |
| [TASK-27](TASK-27-ps-mig-len-unlock-27.md) | PS-MIG-LEN-UNLOCK-27 | migrate len-serv group: unlock-policy component (step 3, secret-bearing) | P2 | M | **Sonnet-class** | 9 | 26 |
| [TASK-28](TASK-28-ps-mig-len-disk-28.md) | PS-MIG-LEN-DISK-28 | migrate len-serv group: disk-layout component (step 4, riskiest, last) | P2 | M | **Sonnet-class** | 10 | 27 |
| [TASK-29](TASK-29-ps-installer-29.md) | PS-INSTALLER-29 | installer modules consume typed resolved fields (per-module, flat fallback) | P3 | L | **Opus-class** | 11 | 28,24 |
| [TASK-30](TASK-30-ps-retire-30.md) | PS-RETIRE-30 | retire legacy autoinstall/render.rs subiquity template path | P3 | M | **Sonnet-class** | 11 | 28 |
| [TASK-31](TASK-31-ps-retire-31.md) | PS-RETIRE-31 | remove dead image/deployer + TargetConfig + legacy Architecture enum + cascade | P3 | M | **Sonnet-class** | 12 | 30 |

## Execution waves (parallelism)

- **Wave 1** (TASK-03, TASK-10, TASK-07, TASK-05, TASK-08, TASK-04, TASK-02, TASK-09, TASK-01, TASK-06, TASK-11): Phase 0 additive authoring types, each in its own NEW module file so parallel agents never collide; every one is unused by any committed host and changes zero serialized artifact. IMSM removal is an independent behavior-deletion. VMGATE-19 is a standalone shell-script change with no Rust dependency, so it runs earliest. Nothing here alters len-serv output. The only shared-file touches are append-only mod.rs declarations (documented per brief) and a one-word PartialEq derive on TangServer (UNLOCK-02).
- **Wave 2** (TASK-12, TASK-13): Wire the wave-1 types onto the two structs. AXES adds arch/role/firmware_quirks/hooks as additive skip-if-default fields on the wire InstallationConfig (defaults reproduce today byte-for-byte). PARTIAL adds the four component sub-structs as nested Option<..Partial> fields on InstallationConfigPartial and extends its manual PartialEq. Different files (config.rs vs profile/mod.rs) so they run in parallel; each depends on its respective wave-1 type modules.
- **Wave 3** (TASK-14, TASK-17, TASK-15, TASK-16): Seam part A + independents. lower() is the pure authoring->wire bridge (needs the wired structs only). validate_resolved() is a new sibling needing only the wired wire-type. SCHEMA adds the schema_version floor on the control rows (needs the wired partial to deserialize component keys). SERIAL-18 is now arch-gated per the serialization-safety fix, so it depends only on the wired arch field (WIRE-AXES-10), not on lower(). All four depend only on wave 2 and touch disjoint files.
- **Wave 4** (TASK-18): Seam part B. merge() gains component resolvers and internally calls lower() to keep its (InstallationConfig, Provenance) signature stable, so it depends on LOWER-12 (wave 3) plus the wired partial. It is the one genuinely novel cross-cutting piece (variant-select vs field-component resolution) and lands alone to isolate its blast radius.
- **Wave 5** (TASK-20, TASK-19, TASK-21, TASK-22): Everything that exercises the full merge->lower path. GATE proves parse->merge equals the 5 committed configs. COCKROACH derives membership from the roster and deletes a live one-off. PIPELINE wires validate_resolved into resolve_from_registry and proves a component fixture resolves (merge already lowered, so no lower() insertion). PLACEHOLDER builds the secret-survival harness. All depend on MERGE-13; PIPELINE also on SCHEMA-20+VALIDATE-14.
- **Wave 6** (TASK-24, TASK-23): Lowest-blast-radius migrations. U1 is a standalone single-host group (rollback fail-closes only U1) and re-proves the D2-B VM gate; it is explicitly allowed to change its own placed artifact. rpi-serv is a net-new isolated group delivering expressibility + validate_resolved coverage only. Neither touches the shared len-serv group. Both depend on the control pipeline + gates being live.
- **Wave 7** (TASK-25): First and lowest-risk step of the shared len-serv group migration: the network axis only (no secret). Sequential head of the len-serv chain; depends on the U1 migration having established the concrete component-in-defaults pattern and the schema_version field.
- **Wave 8** (TASK-26): Second len-serv step: base-image axis. Depends on NET-25 so the equality gate can bisect any drift to exactly one newly-componentized axis.
- **Wave 9** (TASK-27): Third len-serv step: unlock-policy axis, the first secret-bearing one (tpm2_pin). Double-gated (equality + placeholder-survival). Depends on IMG-26.
- **Wave 10** (TASK-28): Fourth and riskiest len-serv step: disk-layout axis. After this the whole len-serv group is component-authored. Double-gated (equality + placeholder-survival for luks_key). Depends on UNLOCK-27.
- **Wave 11** (TASK-29, TASK-30): Phase 6 cleanup once every host is migrated. INSTALLER-29 makes the installer modules read the typed axes (and finally consume the inert-until-now sizes/reset_enabled/tpm2_clevis_peer), highest risk, per-module with flat fallback. RETIRE-30 removes the dead render.rs/HostSpec second source of truth. Both depend on LEN-DISK-28; they touch mostly disjoint files.
- **Wave 12** (TASK-31): Final removal of the dead image/deployer + TargetConfig + legacy Architecture enum and its ImageBuilder/ImageManager cascade. Depends on RETIRE-30 (which introduces no new arch users) so that ssh_installer::config::Arch is provably the only architecture concept left.

## Ground rules (every task)

- **Worktree, never `main`.** Each task = its own `.worktrees/ps-<slug>` + `agent/ps-<slug>` branch + PR + `gh pr merge <n> --rebase`.
- **Re-verify grep anchors before editing** — line numbers in a brief are a starting point, not a guarantee. Zero-hit = STOP and report.
- **File-version headers MANDATORY** on every changed file (bump version + last-edited).
- **Gate before PR:** `cargo test -p uaa-core -p uaa-control` (touched crates) + `cargo clippy --workspace --all-targets`.
- **len-serv PlainLuks stays byte-identical** until its explicit migration briefs (waves 7–10), double-gated by the parse→merge equality gate (PS-GATE-15).
- Additive first: waves 1–5 add component types + the merge/lower seam behind defaults that reproduce today's behavior exactly; no host changes until wave 6.

## Same-file collision → wave rule

Wave 1 briefs each create a **new** module file (zero code-file overlap) — safe to fan out in parallel. Wiring briefs (wave 2) and the seam (waves 3–4) touch shared structs (`config.rs`, `profile/{mod,merge}.rs`) and are **serialized** by the dependency edges above. Host-migration briefs (waves 6–10) touch **disjoint** YAML/group files but share the equality-gate harness, so they run **strictly in the listed order** (each bisects against the previous). `CHANGELOG.md`/`todo.d`/`changelog.d` are fragment-based — no cross-task collision.
