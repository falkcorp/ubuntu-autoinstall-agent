<!-- file: docs/agent-tasks/tooling-port/TASK-05-retire-python-shell.md -->
<!-- version: 1.0.0 -->
<!-- guid: 6ea29c4b-7a75-47b9-9fa3-c33e11e0fcd3 -->
<!-- last-edited: 2026-07-10 -->

# TASK-05 — Retire scripts/autoinstall-agent.py + the four ported shell scripts (deletion, gated on operator-confirmed cutover) (ws9-tooling)

**Priority:** P3 · **Effort:** S · **Recommended subagent:** Haiku-class · mechanical-deletion subagent · **Why:** mechanical deletion with absence-checks; the gate is the entire risk · **Depends on:** TG-03 (wave-9 gated: `testing-gates/TASK-03` constellation e2e MERGED and green — plus the operator gate below)

**⛔ DO NOT DISPATCH UNTIL THE OPERATOR CONFIRMS: M6 CUTOVER COMPLETE + 2-WEEK ROLLBACK WINDOW ELAPSED (BUCKET-3 GATE). NO AUTOMATED SIGNAL SUBSTITUTES FOR THIS CONFIRMATION — THE COORDINATOR MUST HAVE THE OPERATOR'S EXPLICIT WRITTEN GO IN HAND.**

(Spec Decision 16: rollback within the window is export-first re-enable of the Python unit — deleting these files before the window closes destroys that path. Spec Migration §6: "delete Python + each ported shell script, each deletion gated on its replacement's gate".)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/tooling-port-retire-python-shell" -b agent/tooling-port-retire-python-shell origin/main
cd "$REPO/.worktrees/tooling-port-retire-python-shell"
git rebase origin/main
```

(Protocol also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

## Goal

DELETE exactly five files, nothing else (spec Goals: "Retire the ported shell tools … only after their Rust replacement passes its gate"; spec Files-modified table: "retired at M6, each gated"):

1. `scripts/autoinstall-agent.py` — replaced by uaa-control's :25000 parity plane (IP-01..04, spec Decision 12)
2. `scripts/make-ssh-ready-iso.sh` — replaced by `uaa iso remaster` (TASK-01)
3. `scripts/deploy-usb-configs.sh` — replaced by `uaa config place` (TASK-02)
4. `scripts/build-installer-image.sh` — replaced by `uaa image build` (TASK-03)
5. `scripts/vm-validate.sh` — replaced by `uaa vm-validate` (TASK-04), proven by TG-03

This is the ONLY removal-polarity task in the tooling-port workstream. Scope is the five deletions plus reference hygiene (Step 4) — no code changes, no doc rewrites beyond dangling-reference fixes.

## Background (verify before editing)

- Each replacement must already be merged AND its gate green: TP-01..04 merged (waves 2–3), IP-01..04 merged (waves 4–5), TG-03 merged and passing (wave 8). If ANY replacement grep in the execution-time block below misses → STOP and report.
- The Python service on the server (`autoinstall-agent.service`) is an OPERATIONS artifact — this task deletes only REPO files; it never touches the server (hard rules below). Decommissioning the server unit is the operator's M6 runbook step, not yours.
- `scripts/vm-validate.sh` was the authoritative gate until TG-03 (TASK-04 shared_state); after TG-03, `uaa vm-validate` + `scripts/vm-validate-constellation.sh` own that role — which is why TG-03 is the hard dependency.

**HARD RULES (non-negotiable):**
- NO hardware actions. Validate ONLY in-repo (`cargo`) and, where a brief says so,
  the QEMU+swtpm harness (`scripts/vm-validate.sh`). Code that COULD touch hardware
  is written and unit-tested against mock executors only.
- NEVER wipe, write to, or deploy on 172.16.2.30 ("the server") or len-serv-003.
- `disk_device` is read from the live target at runtime, never guessed or hardcoded.
- ipmitool runs via `ssh 172.16.2.30`, never on macOS.
- NEVER power on unimatrixone (U1).
- No real secret in any file: `REPLACE_AT_PLACE_TIME` placeholders stay placeholders.
- Stay inside your worktree; never `git push`, `gh pr`, or merge — report done and stop.

- **Re-verify these anchors before editing** — line numbers drift; zero hits at both
  old and mapped path = STOP and report:
  ```bash
  ls scripts/autoinstall-agent.py scripts/make-ssh-ready-iso.sh scripts/deploy-usb-configs.sh scripts/build-installer-image.sh scripts/vm-validate.sh | wc -l
  # expect: 5 (all five exist today; fewer = partially applied, jump to Idempotency)
  ```
- Execution-time checks (the replacements exist only after waves 2–8):
  ```bash
  grep -n "todo!" crates/uaa-core/src/iso/remaster.rs crates/uaa-core/src/config_place.rs crates/uaa-core/src/iso/image_build.rs crates/uaa-core/src/vm_validate.rs
  # expect: 0 hits total — all four ports filled; any hit = STOP, replacement not merged
  test -f scripts/vm-validate-constellation.sh && echo TG03-OK
  # expect: TG03-OK (TG-03 merged; absent = STOP)
  ```

## Step-by-step

1. CONFIRM WITH THE COORDINATOR that the operator's written M6-cutover + 2-week-window confirmation exists. If you cannot see that confirmation quoted in your dispatch instructions, STOP and report — do not proceed on inference.
2. Run the ⛔ START HERE block, then every check above (5 files present, 0 `todo!` hits, TG03-OK).
3. Delete the five files:
   ```bash
   git rm scripts/autoinstall-agent.py scripts/make-ssh-ready-iso.sh scripts/deploy-usb-configs.sh scripts/build-installer-image.sh scripts/vm-validate.sh
   ```
4. Reference hygiene — find every EXECUTABLE reference (CI, scripts, build files) to the five names:
   ```bash
   grep -rn "autoinstall-agent\.py\|make-ssh-ready-iso\.sh\|deploy-usb-configs\.sh\|build-installer-image\.sh\|vm-validate\.sh" \
     .github/ scripts/ Makefile* Justfile* 2>/dev/null
   ```
   - A hit in `.github/workflows/` or a surviving script = FIX it in this commit (point it at the `uaa` replacement, bumping that file's header) — a deleted-file invocation in CI is a broken build.
   - Hits in `docs/**` or `docs/agent-tasks/**` are HISTORICAL RECORD — leave them (do not rewrite plan/brief history). Exception: `docs/vm-validation.md` — if it still instructs running `scripts/vm-validate.sh`, add a one-line deprecation pointer to `uaa vm-validate` at the top (bump version, KEEP guid).
   - Rust source/comment mentions (e.g. the marker strings `marker build-installer-image.sh:72` inside `vm_validate.rs`'s report format): LEAVE UNCHANGED — the report format is frozen (TASK-04) and the string is a label, not a path lookup.
5. Bump the file header (version + last-edited) on every file you touch; new files get a fresh 4-line header with a new uuid4 (`uuidgen | tr 'A-F' 'a-f'`). (For this task that is only files edited in Step 4, if any.)

## How to test

```bash
cargo test --lib --offline && cargo build --offline
# Expected: all tests pass (baseline 311 + all constellation-wave tests), build clean
cargo clippy --offline -- -D warnings
# Expected: no warnings
ls scripts/autoinstall-agent.py scripts/make-ssh-ready-iso.sh scripts/deploy-usb-configs.sh scripts/build-installer-image.sh scripts/vm-validate.sh 2>&1 | grep -c "No such file"
# Expected: 5
grep -rn "autoinstall-agent\.py\|make-ssh-ready-iso\.sh\|deploy-usb-configs\.sh\|build-installer-image\.sh\|vm-validate\.sh" .github/ scripts/ Makefile* Justfile* 2>/dev/null
# Expected: 0 hits (no executable reference to a deleted file)
git diff origin/main --stat
# Expected: exactly 5 deletions + only the reference-hygiene edits from Step 4
```

## Acceptance criteria

- [ ] All five files gone: `ls scripts/autoinstall-agent.py scripts/make-ssh-ready-iso.sh scripts/deploy-usb-configs.sh scripts/build-installer-image.sh scripts/vm-validate.sh 2>&1 | grep -c "No such file"` → 5.
- [ ] No executable dangling reference: the `.github/`+`scripts/`+`Makefile*`+`Justfile*` grep in How-to-test → 0 hits.
- [ ] Nothing else deleted or rewritten: `git diff origin/main --stat` shows the 5 removals + only Step-4 hygiene edits; `docs/**` history untouched except the optional `docs/vm-validation.md` pointer.
- [ ] Anti-over-suppression: N/A
- [ ] `cargo test --lib --offline && cargo build --offline` exits 0; clippy clean.
- [ ] File headers bumped on every changed file (`git diff origin/main --stat` files each show a version bump; guids unchanged — deletions excepted).

## Commit message

```
chore(scripts): retire autoinstall-agent.py + four ported shell scripts (ws9-tooling)

M6 cutover confirmed by the operator and the 2-week rollback window elapsed
(Bucket-3 gate). Deletes scripts/autoinstall-agent.py (replaced by the
uaa-control :25000 parity plane), make-ssh-ready-iso.sh (uaa iso remaster),
deploy-usb-configs.sh (uaa config place), build-installer-image.sh
(uaa image build), vm-validate.sh (uaa vm-validate, proven by the TG-03
constellation e2e gate). Executable references fixed; historical docs left
as record.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

Removal polarity — check for ABSENCE: if `! test -f scripts/autoinstall-agent.py && ! test -f scripts/make-ssh-ready-iso.sh && ! test -f scripts/deploy-usb-configs.sh && ! test -f scripts/build-installer-image.sh && ! test -f scripts/vm-validate.sh` AND `grep -rn "make-ssh-ready-iso\.sh\|deploy-usb-configs\.sh\|build-installer-image\.sh" .github/ scripts/ 2>/dev/null` returns 0 hits, already done — run the acceptance checks instead of re-applying. Rollback = `git revert` the single commit to restore all five files exactly as they were (they are plain repo files; no server, service, or data state is involved in either direction).
