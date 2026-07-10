<!-- file: docs/agent-tasks/installer-robustness/TASK-08-path-a-b-split-doc.md -->
<!-- version: 1.0.0 -->
<!-- guid: adf8c680-4013-42de-ac20-770ac71311ba -->
<!-- last-edited: 2026-07-09 -->

# TASK-08 — Document the Path A (subiquity render pipeline) vs Path B (ssh_installer) split + guardrails (todo:boot-layout-PathA)

**Priority:** P3 · **Effort:** S · **Recommended subagent:** Haiku-class · docs subagent · **Why:** docs-only; Path A is still live (render/place/verify golden tests) so removal is NOT tasked · **Depends on:** none

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/installer-robustness-path-a-b-split-doc" -b agent/installer-robustness-path-a-b-split-doc origin/main
cd "$REPO/.worktrees/installer-robustness-path-a-b-split-doc"
git rebase origin/main
```

(Protocol is also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

## Goal

Write a NEW document `docs/architecture-path-split.md` that explains the repo's two live installer code paths — **Path A** (`src/autoinstall/`: render-user-data → place → verify, driving subiquity/curtin) and **Path B** (`src/network/ssh_installer/`: the PROVEN direct ZFS-on-LUKS installer) — when to use each, and the guardrails for future work; plus add a short pointer section to `README.md`. **DOCS ONLY — this task changes zero lines of Rust, YAML, or shell, and it must NOT plan, suggest, or schedule Path A's removal** (Path A's golden tests and verify checks are live and load-bearing).

Reuse the facts below and the repo's own doc conventions: the 4-line HTML-comment file header used by every `.md` file (see any file under `docs/`), and README's existing `##` section style. Do not invent new terminology — "Path A" and "Path B" are the established names (used in `docs/specs/installer-robustness-design.md` and `docs/specs/installer-robustness-plan.md`; this is the plan's TASK-08 step).

## Background (verify before editing)

Facts to state in the document (each backed by an anchor grep below — re-verify, then cite paths/symbols, not line numbers):

- **Path A** lives in `src/autoinstall/` (`render.rs`, `place.rs`, `verify.rs`, `host_spec.rs`, `templates/`). It renders cloud-init/subiquity autoinstall user-data from host specs (`render-user-data` subcommand), places it on the netboot server (`place`), and verifies an installed host post-hoc (`verify`). The actual OS install is performed by subiquity/curtin from the rendered user-data — Path A never runs `cryptsetup`/`zpool` itself.
- **Path A's known defect (why Path B exists):** the subiquity/curtin storage path historically produced a LUKS+LVM+ext4 layout instead of the required ZFS-on-LUKS; `evaluate_no_lvm` in `src/autoinstall/verify.rs` is the standing regression guard ("LVM present — expected ZFS-on-LUKS"). Document this as the motivation for Path B, NOT as a removal justification.
- **Path A is still live:** golden-fixture tests (`GOLDEN_001/002/003` in `src/autoinstall/render.rs`, fixtures under `tests/fixtures/golden/`) run in the 237-test lib suite, and `verify`/`place` are used operationally.
- **Path B** lives in `src/network/ssh_installer/` (installer.rs, disk_ops.rs, zfs_ops.rs, system_setup.rs, packages.rs, config.rs): a 7-phase (0–6) direct installer that itself partitions, LUKS-formats, creates bpool/rpool, debootstraps, and configures GRUB/crypttab/dracut/Tang — over SSH (`ssh-install`), locally (`local-install`), or via the unified `install`. It is the PROVEN path: 7/7 phases succeeded on unimatrixone hardware 2026-07-09.
- **No curtin logic exists in Rust** (Path A only emits user-data that curtin consumes) — worth one line in the doc so readers don't hunt for it.
- **When to use which (the doc's core table/section):** installing or reinstalling a machine with ZFS-on-LUKS → Path B; rendering/placing netboot autoinstall seeds and post-install verification → Path A.
- **Guardrails to write down:** (1) new install-execution logic goes to Path B only; (2) Path A stays for render/place/verify — do not add storage-execution code to it; (3) Path A must not be removed while its golden tests and `verify` command are live; removal is explicitly NOT planned.
- README.md currently has a 3-line header comment (`file`/`version`/`guid`, version 1.0.0) and NO `last-edited` line — this task bumps the version, adds `last-edited`, and adds the pointer section.

- **Re-verify these anchors before editing** — cite symbols/paths in the doc, never bare line numbers:
  ```bash
  grep -rn 'curtin' --include='*.rs' src/
  # expect: 0 hits (mentions exist only in PLAN.md/PLAN-zfs-luks-multikey.md/todo.md/docs/netboot-autodeploy.md prose)
  ls src/autoinstall src/network/ssh_installer          # expect: both directories exist (Path A / Path B)
  grep -n 'fn evaluate_no_lvm' src/autoinstall/verify.rs   # expect: 1 hit — the LVM regression guard to cite
  grep -n 'GOLDEN_00' src/autoinstall/render.rs            # expect: 3 hits — golden tests still live
  grep -n '<!-- version:' README.md                        # expect: 1 hit near the top — header to bump
  test -f docs/architecture-path-split.md && echo EXISTS || echo NEW   # expect: NEW (see Idempotency if EXISTS)
  ```
  Zero hits on an anchor whose "expect" says ≥1 means STOP and report — do not guess.

- **HARD RULES (restated):**
  1. NEVER wipe/reimage/touch 172.16.2.30 ("the server") or len-serv-003 — this task is documentation only and runs nothing on any host.
  2. SECRETS: never paste a real `luks_key`/`root_password`/`tpm2_pin` into any doc; if showing config snippets, use the `REPLACE_AT_PLACE_TIME` placeholder convention.
  3. Stay in your worktree; NEVER push/PR/merge — the coordinator owns all git.

## Step-by-step

1. Run the anchor greps above from the worktree root.
2. Create `docs/architecture-path-split.md` with the mandatory 4-line header (generate a fresh guid with `uuidgen | tr 'A-Z' 'a-z'`):
   ```
   <!-- file: docs/architecture-path-split.md -->
   <!-- version: 1.0.0 -->
   <!-- guid: <fresh-uuid> -->
   <!-- last-edited: 2026-07-09 -->
   ```
3. Structure the document with these sections (≈1–2 pages total; every factual claim comes from the Background list — do not speculate beyond it):
   - `# Architecture: Path A vs Path B` — one-paragraph summary of the split.
   - `## Path A — subiquity render pipeline (src/autoinstall/)` — render-user-data → place → verify flow; subiquity/curtin performs the install; the LVM defect and the `evaluate_no_lvm` regression guard; golden-fixture tests keep it pinned.
   - `## Path B — direct ZFS-on-LUKS installer (src/network/ssh_installer/)` — 7 phases (0 vars, 1 packages, 2 disk prep, 3 ZFS pools, 4 debootstrap base, 5 system config incl. GRUB/crypttab/dracut/Tang, 6 final cleanup); SSH or local execution; proven 7/7 on unimatrixone hardware 2026-07-09.
   - `## When to use which` — a small table: task → path → subcommand(s).
   - `## Guardrails` — the three guardrails from Background, verbatim in spirit, including the explicit sentence: "Path A removal is NOT planned; its golden tests and `verify` are live."
4. Edit `README.md`: add a short section `## Architecture: Path A vs Path B` (place it after the existing `## Features` section) — 3–5 lines summarizing the split and linking `[docs/architecture-path-split.md](docs/architecture-path-split.md)`. Purely additive: do not rewrite, reorder, or delete any existing README content.
5. Update README.md's header comment: bump `<!-- version: 1.0.0 -->` to `<!-- version: 1.1.0 -->`, add `<!-- last-edited: 2026-07-09 -->` after the guid line, keep the existing guid.
6. Touch NOTHING else — no code files, no other docs, no todo.md edits.

## How to test

```bash
cargo test --lib --offline
# Expected: 237+ passed; 0 failed (docs-only change — suite unchanged)
cargo build --offline
# Expected: exit 0
```

## Acceptance criteria

- [ ] `test -f docs/architecture-path-split.md` succeeds and the file has the 4-line header (`grep -c '<!--' docs/architecture-path-split.md` ≥ 4).
- [ ] Both paths documented: `grep -n 'src/autoinstall\|src/network/ssh_installer' docs/architecture-path-split.md` returns ≥ 2 hits.
- [ ] The LVM defect + guard are cited: `grep -n 'evaluate_no_lvm' docs/architecture-path-split.md` returns ≥ 1 hit.
- [ ] Removal is explicitly NOT planned: `grep -in 'not planned' docs/architecture-path-split.md` returns ≥ 1 hit.
- [ ] README pointer present: `grep -n 'architecture-path-split.md' README.md` returns ≥ 1 hit.
- [ ] No code touched: `git diff --name-only origin/main` lists EXACTLY `docs/architecture-path-split.md` and `README.md`.
- [ ] Tests green: `cargo test --lib --offline` reports 237+ passed / 0 failed; `cargo build --offline` exits 0.
- [ ] File headers bumped: README.md shows `version: 1.1.0` and `last-edited: 2026-07-09` (`git diff origin/main -- README.md | grep -c 'version:'` → ≥1 (README header bumped in this diff) and the NEW docs/architecture-path-split.md carries a fresh 4-line header).
- [ ] Anti-over-suppression: N/A

## Commit message

```
docs(architecture): document Path A (subiquity render) vs Path B (ssh_installer) split + guardrails

Adds docs/architecture-path-split.md covering the render/place/verify pipeline
(Path A, incl. the LVM defect and its evaluate_no_lvm regression guard) versus
the proven 7-phase ZFS-on-LUKS installer (Path B), when to use which, and the
guardrail that Path A removal is not planned. README gains a pointer section.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

Already-done check (additive polarity — presence of the new doc + pointer):
```bash
test -f docs/architecture-path-split.md && echo EXISTS   # EXISTS → doc already written
grep -n 'architecture-path-split.md' README.md           # ≥1 hit → README pointer already present
```
If both hold, the task is already applied — run the acceptance checks instead of re-applying. Rollback: `git revert` the single commit deletes the doc and the README section/header bump; no code, data, or sibling task is affected.
