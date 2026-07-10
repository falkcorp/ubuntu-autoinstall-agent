<!-- file: docs/agent-tasks/boot-prod/README.md -->
<!-- version: 1.0.0 -->
<!-- guid: 8d9f6d52-899c-47b1-89fd-8ba4c1322a5e -->
<!-- last-edited: 2026-07-09 -->

# Workstream — boot productionization (UEFI boot order + RESET recovery partition)

Make a Path B (`src/network/ssh_installer/`) install boot correctly and recover locally: (1) set
the UEFI BootOrder from inside the target chroot right after `update-grub` — network entries
first, ubuntu second — mirroring the proven `set_boot_order` logic from
`installer-image/nocloud/uaa-usb-bootstrap.sh`; (2) populate the already-created-but-abandoned
RESET p2 partition with a bootable copy of the SSH-ready recovery ISO, the debootstrap base
tarball, and a GRUB loopback "reset/recover" menu entry. TASK-02 is specified by
`docs/specs/reset-partition-design.md` and `docs/specs/reset-partition-plan.md`; TASK-01 is
self-contained in its brief (it cites `docs/specs/installer-robustness-design.md` only in
passing for the shared `system_setup.rs` surface). The DESTRUCTIVE reset flow itself lives in
the recovery environment behind a literal typed `nuke it` gate — this workstream only STAGES
artifacts and a menu entry; nothing here wipes anything.

**Execution mode:** SERIAL WAVES (coordinator-driven) — trigger: `system_setup.rs` is shared by
3 tasks (installer-robustness/TASK-01, installer-robustness/TASK-04, boot-prod/TASK-01) and
`installer.rs` by 6 tasks (installer-robustness/TASK-01/05/07, phase-rerun/TASK-01/02,
boot-prod/TASK-02) per the global collision matrix — both boot-prod tasks collide
cross-workstream and are wave-serialized after their colliders merge.

| Task | Src id | Title | Priority | Effort | Tier | Wave |
|------|--------|-------|----------|--------|------|------|
| TASK-01 | todo:efibootmgr | efibootmgr in chroot post-update-grub: BootOrder = network #1, ubuntu #2 (non-fatal on legacy BIOS) | P1 | M | Sonnet-class | 3 |
| TASK-02 | todo:RESET-partition | Populate RESET p2: remastered USB ISO copy + debootstrap tarball + GRUB loopback recover entry gated on typing nuke-it | P2 | L | Sonnet-class | 6 |

## Wave table

Waves are GLOBAL (shared across all install-ops workstreams); boot-prod occupies waves 3 and 6.

| Wave | Tasks | Prereq | Parallel-safe because |
|---|---|---|---|
| 3 | TASK-01 | waves 1–2 merged + siblings rebased (installer-robustness/TASK-01 in wave 1 and installer-robustness/TASK-04 in wave 2 both edit `system_setup.rs`) | only boot-prod task in wave 3; its sole file `system_setup.rs` collides only with already-merged waves; disjoint from wave-3 siblings (install-server/TASK-03 → `scripts/autoinstall-agent.py`, installer-robustness/TASK-07 → `installer.rs`/`commands.rs`) |
| 6 | TASK-02 | wave 5 merged + siblings rebased (all `installer.rs` colliders — installer-robustness/TASK-01/05/07, phase-rerun/TASK-01/02 — and the `mod.rs` collider installer-robustness/TASK-01 are merged by end of wave 5) | sole task in global wave 6 |

## Ground rules

- Rust only, in the files each brief names; every change is additive (a new step after
  `update-grub`; a new module + two small wiring hunks). No drive-by refactors — six other
  tasks rebase across these files.
- Build + test gate for every task in this workstream:
  ```bash
  cargo test --lib --offline    # Expected: >=237 passed; 0 failed (237 is the pre-package baseline)
  cargo build --offline         # Expected: exit 0
  cargo clippy --offline        # Expected: no new warnings
  ```
- **Verify every file:line anchor with `grep` before editing** — line numbers in each brief are
  a starting point, not a guarantee. Zero hits = STOP and report.
- File headers are MANDATORY: bump `version` + `last-edited` on every touched file, keep
  existing guids; new files get a fresh guid.
- HARD RULES (full list in each brief): NEVER wipe/reimage/touch 172.16.2.30 ("the server") or
  len-serv-003 — these tasks are code-only, validated in VM/QEMU (`testing-gates/TASK-01`
  harness) before any hardware run. `disk_device` is READ from the live target, never guessed
  from `/dev/sd*` conventions. Workers stay in their worktree and NEVER push/PR/merge — the
  coordinator owns all git.
- Nothing in this workstream is destructive at install time: TASK-01's boot-order step is
  non-fatal by design (legacy-BIOS hosts skip it), and TASK-02 only stages files + a menu
  entry — the wipe gate (`nuke it`) executes exclusively inside the recovery environment.

## Collision / wave note

Exact shared files from the global collision matrix:

| Shared file | Tasks that touch it | Resolution |
|---|---|---|
| `src/network/ssh_installer/system_setup.rs` | installer-robustness/TASK-01, installer-robustness/TASK-04, **boot-prod/TASK-01** | serialize: wave1=IR-T01, wave2=IR-T04, wave3=BP-T01 |
| `src/network/ssh_installer/installer.rs` | installer-robustness/TASK-01, installer-robustness/TASK-05, installer-robustness/TASK-07, phase-rerun/TASK-01, phase-rerun/TASK-02, **boot-prod/TASK-02** | serialize: wave1=IR-T01, wave2=IR-T05, wave3=IR-T07, wave4=PR-T01, wave5=PR-T02, wave6=BP-T02 |
| `src/network/ssh_installer/mod.rs` | installer-robustness/TASK-01, **boot-prod/TASK-02** | serialize: wave1=IR-T01, wave6=BP-T02 |

TASK-01 and TASK-02 do not collide with each other (disjoint file sets), but each collides
cross-workstream, so neither may start before its listed prereq waves merge and all open
sibling worktrees rebase. TASK-02 is deliberately LAST (wave 6) so its `installer.rs`/`mod.rs`
hunks rebase onto everything else, and so the `partition_path` helper from
installer-robustness/TASK-01 (wave 1) is already merged when TASK-02 mounts p2.

See [ORCHESTRATION.md](../ORCHESTRATION.md) (one level up) for the coordinator + worker protocol.
