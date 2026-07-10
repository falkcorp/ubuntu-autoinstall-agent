<!-- file: docs/agent-tasks/README.md -->
<!-- version: 1.0.0 -->
<!-- guid: f97394a1-66ba-4684-9aee-b99879edb817 -->
<!-- last-edited: 2026-07-09 -->

# Agent tasks — install-ops planning package (2026-07-09)

Master index for the post-USB-bootstrap operation: 6 workstreams, 20 task briefs,
6 dependency waves. Protocol: [ORCHESTRATION.md](ORCHESTRATION.md). Hardware-blocked
work is listed (not tasked) in [DEFERRED.md](DEFERRED.md). Baseline at planning time:
main @ 8540976, `cargo test --lib --offline` = **237 passed**.

Every workstream folder has `README.md` (task + wave tables), `orchestration.md`
(protocol + mermaid), and one self-contained `TASK-NN-<slug>.md` per task. Design
specs + implementation plans live in `docs/specs/<slug>-{design,plan}.md` and are
cited from each workstream README.

## Workstreams

| Workstream | Spec | Tasks | Focus |
|---|---|---|---|
| [installer-robustness](installer-robustness/) | [design](../specs/installer-robustness-design.md) · [plan](../specs/installer-robustness-plan.md) | 8 | partition-suffix helper (⚠ wide collision), detect fns, netplan, LUKS keyfile (security), schema, curtin, Path A/B doc |
| [phase-rerun](phase-rerun/) | [design](../specs/phase-selective-rerun-design.md) · [plan](../specs/phase-selective-rerun-plan.md) | 2 | `--phases`/`--from-phase` + non-destructive mount-existing-target (⚠ wipe-adjacent) |
| [boot-prod](boot-prod/) | [design](../specs/reset-partition-design.md) · [plan](../specs/reset-partition-plan.md) | 2 | efibootmgr boot order in chroot; RESET partition populate + `nuke it` recovery entry |
| [install-server](install-server/) | [design](../specs/install-server-design.md) · [plan](../specs/install-server-plan.md) | 5 | webhook flip on `success`, health/list endpoints, secret-injection placement (server-local, NO HTTP write API), dashboard |
| [testing-gates](testing-gates/) | [design](../specs/qemu-validation-design.md) · [plan](../specs/qemu-validation-plan.md) | 2 | QEMU+swtpm VM gate (THE gate before hardware), LocalClient tests |
| [remote-power](remote-power/) | [design](../specs/remote-power-design.md) · [plan](../specs/remote-power-plan.md) | 1 | `uaa power` dispatch + IPMI-via-server path |

## Master task table

| Task | Src todo item | Priority | Effort | Tier | Wave |
|---|---|---|---|---|---|
| installer-robustness/TASK-01 partition-suffix-helper | partition-suffix `{}p1..p4` | P1 | L | Opus ⚠ | 1 |
| installer-robustness/TASK-02 detect-primary-disk-json | detect_primary_disk fragile | P2 | M | Sonnet | 1 |
| installer-robustness/TASK-03 detect-network-config-parse | detect_network_config hardcoded | P2 | M | Sonnet | 2 |
| installer-robustness/TASK-04 netplan-renderer-dhcp | renderer configurable | P2 | M | Sonnet | 2 |
| installer-robustness/TASK-05 luks-keyfile | LUKS passphrase in env | P1 | M | Opus ⚠ | 2 |
| installer-robustness/TASK-06 config-schema-hardening | config schema completeness | P3 | S | Haiku | 1 |
| installer-robustness/TASK-07 curtin-in-target | curtin in-target compat | P3 | M | Sonnet | 3 |
| installer-robustness/TASK-08 path-a-b-split-doc | broken /boot layout (Path A disposition) | P3 | S | Haiku | 1 |
| phase-rerun/TASK-01 phase-spec-cli | phase-selective re-run | P1 | L | Opus ⚠ | 4 |
| phase-rerun/TASK-02 mount-existing-target | phase-selective re-run | P1 | L | Opus ⚠ | 5 |
| boot-prod/TASK-01 efibootmgr-chroot | efibootmgr boot order | P1 | M | Sonnet | 3 |
| boot-prod/TASK-02 reset-partition-populate | RESET partition (p2) | P2 | L | Sonnet | 6 |
| install-server/TASK-01 webhook-flip-success | USB report → boot-order fix | P1 | S | Haiku | 1 |
| install-server/TASK-02 serve-agent-binary | USB agent serving | P1 | S | Haiku | 2 |
| install-server/TASK-03 list-configs-endpoint | install-server extras | P2 | S | Haiku | 3 |
| install-server/TASK-04 secret-injection-placement | place-time secrets | P2 | M | Sonnet | 1 |
| install-server/TASK-05 status-dashboard | install-server extras | P3 | M | Sonnet | 4 |
| testing-gates/TASK-01 qemu-swtpm-harness | QEMU+swtpm gate | P1 | L | Sonnet | 2 |
| testing-gates/TASK-02 localclient-tests | no local-flow tests | P2 | M | Sonnet | 1 |
| remote-power/TASK-01 power-subcommand-ipmi | wire remote power | P2 | M | Sonnet | 5 |

Tier policy: cheapest viable — Haiku for mechanical single-file edits, Sonnet for
moderate multi-file logic, **Opus reserved for the ⚠ review-critical four**: the
wide-collision partition-suffix transform, the LUKS-keyfile security change, and both
wipe-adjacent phase-rerun tasks. Never downgrade a ⚠ task.

## ⚠️ Same-file collision matrix (computed from the briefs' exact-file lists)

| Shared file | Tasks that touch it | Resolution |
|---|---|---|
| scripts/autoinstall-agent.py | IS-01, IS-02, IS-03, IS-05 | serialize: wave1=IS-01, wave2=IS-02, wave3=IS-03, wave4=IS-05 |
| src/cli/args.rs | PR-01, RP-01 | serialize: wave4=PR-01, wave5=RP-01 |
| src/cli/commands.rs | IR-02, IR-03, IR-07, PR-01, RP-01 | serialize: wave1=IR-02, wave2=IR-03, wave3=IR-07, wave4=PR-01, wave5=RP-01 |
| src/main.rs | PR-01, RP-01 | serialize: wave4=PR-01, wave5=RP-01 |
| src/network/ssh_installer/config.rs | IR-04, IR-06 | serialize: wave1=IR-06, wave2=IR-04 |
| src/network/ssh_installer/disk_ops.rs | IR-01, IR-05, PR-02 | serialize: wave1=IR-01, wave2=IR-05, wave5=PR-02 |
| src/network/ssh_installer/installer.rs | IR-01, IR-05, IR-07, PR-01, PR-02, BP-02 | serialize: wave1=IR-01, wave2=IR-05, wave3=IR-07, wave4=PR-01, wave5=PR-02, wave6=BP-02 |
| src/network/ssh_installer/mod.rs | IR-01, BP-02 | serialize: wave1=IR-01, wave6=BP-02 |
| src/network/ssh_installer/system_setup.rs | IR-01, IR-04, BP-01 | serialize: wave1=IR-01, wave2=IR-04, wave3=BP-01 |
| src/network/ssh_installer/zfs_ops.rs | IR-01, PR-02 | serialize: wave1=IR-01, wave5=PR-02 |

(IR = installer-robustness, PR = phase-rerun, BP = boot-prod, IS = install-server,
TG = testing-gates, RP = remote-power.)

## Global wave table

| Wave | Tasks | Prereq | Parallel-safe because |
|---|---|---|---|
| 1 | IR-01, IR-02, IR-06, IR-08, IS-01, IS-04, TG-02 | none | disjoint file sets (see collision matrix) |
| 2 | IR-03, IR-04, IR-05, IS-02, TG-01 | wave 1 merged + siblings rebased | each shares files only with wave-1 tasks; TG-01 depends on IR-01 (virtio /dev/vda) |
| 3 | IR-07, BP-01, IS-03 | wave 2 merged + siblings rebased | shares commands.rs/installer.rs/system_setup.rs/py only with earlier waves |
| 4 | PR-01, IS-05 | wave 3 merged + siblings rebased | PR-01 shares args/main/commands/installer with earlier waves only |
| 5 | PR-02, RP-01 | wave 4 merged (PR-02 depends on PR-01) | disjoint: PR-02={installer,zfs_ops,disk_ops}, RP-01={args,main,commands,power/,lib} — no overlap |
| 6 | BP-02 | wave 5 merged + siblings rebased | shares installer.rs/mod.rs with nearly everything — runs last, alone |

Execution mode per wave: **PARALLEL DISPATCH within the wave, SERIAL WAVES between
waves (coordinator-driven)** — trigger: the collision matrix above (10 shared-file
rows). ⚠ Opus tasks (IR-01, IR-05, PR-01, PR-02) are **SINGLE-AGENT (strong model)**
dispatches — never batched with a weak-tier sweep. Wave 1's Haiku trio
(IR-06, IR-08, IS-01) is `/parallel-sweep`-eligible (3 mechanically simple tasks,
disjoint files, per-worktree gate).

## Ground rules (bind every task; each brief restates its relevant subset)

- Gate: `cargo test --lib --offline` (≥237 passed) + `cargo build --offline` before
  any "done" report; clippy for code briefs.
- **Verify every file:line anchor with the brief's `grep` block before editing** —
  line numbers drift; grep is authoritative. Zero hits ⇒ STOP and report.
- File version headers bumped on every touched file (keep guids).
- Workers never push/PR/merge; the coordinator owns git (see ORCHESTRATION.md).
- NO destructive actions against any live host; VM/QEMU validation only
  (testing-gates/TASK-01 gates all hardware work).
- `scripts/autoinstall-agent.py` edits are repo-mirror-only; a human deploys.
- No real secrets in git — `REPLACE_AT_PLACE_TIME` stays a placeholder everywhere.
