<!-- file: docs/agent-tasks/core-proto/TASK-06-musl-matrix-ci.md -->
<!-- version: 1.0.0 -->
<!-- guid: 708bcf72-706e-4fcd-b594-37be1df9490e -->
<!-- last-edited: 2026-07-10 -->

# TASK-06 — musl-build.yml: build every workspace binary, static-verify each, artifact per binary (ws1-core)

**Priority:** P2 · **Effort:** S · **Recommended subagent:** Haiku-class · ci-yaml subagent · **Why:** mechanical CI yml extension mirroring the existing single-binary job · **Depends on:** TASK-01 (wave-2 gated: CP-01 MERGED — the workflow must build the WORKSPACE, which only exists after the conversion)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/core-proto-musl-matrix-ci" -b agent/core-proto-musl-matrix-ci origin/main
cd "$REPO/.worktrees/core-proto-musl-matrix-ci"
git rebase origin/main
```

(Protocol also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

## Goal

Extend `.github/workflows/musl-build.yml` from the single-binary job to a per-binary workspace build (spec "Files modified": "`.github/workflows/musl-build.yml` — per-binary matrix"; spec topology: all four constellation binaries are `x86_64-unknown-linux-musl` static): build the whole workspace, static-verify EVERY produced binary, and upload one artifact per binary. The workflow must be **forward-compatible**: `uaa-control`/`uaa-web`/`uaa-pxe` do not exist yet at your wave (they land in waves 3–6) — their upload steps use `if-no-files-found: ignore` so this file is edited exactly ONCE and new crates get artifacts automatically as they appear. The existing `uaa-amd64` artifact name is FROZEN (the USB bootstrap and the deploy runbook reference it — see the workflow's own header comment) and keeps `if-no-files-found: error`. Purely additive edits: keep the existing triggers, permissions, checkout-with-submodules, musl toolchain, rustup target, and cargo cache steps byte-compatible; you change only the build, verify, and upload steps.

## Background (verify before editing)

- Today's job: single `cargo build --release --target x86_64-unknown-linux-musl`, one `ldd`-based static check of `target/x86_64-unknown-linux-musl/release/ubuntu-autoinstall-agent`, one `upload-artifact` named `uaa-amd64`.
- After CP-01 the binary is built from `crates/uaa` but the `[[bin]]` name — and therefore the artifact path — is UNCHANGED: `target/x86_64-unknown-linux-musl/release/ubuntu-autoinstall-agent`.
- `--workspace` on a build includes every member; members without a bin target (uaa-core, uaa-proto) produce no executables — the verify loop must therefore DISCOVER binaries rather than assume a list, then the upload steps map known names to artifacts.
- Edge semantics: a binary that exists but is NOT static must FAIL the job (that is the entire point of this workflow — the installer environment has no glibc); a binary that does not exist yet must NOT fail anything except the frozen `uaa-amd64` (its absence means the workspace conversion broke the bin name → loud failure is correct).
- Pin new action steps to the same commit-pinned action versions already used in this file (`actions/upload-artifact@ea165f8d...` etc.) — copy the existing pins, do not float tags.

- **Re-verify these anchors before editing** — line numbers drift; zero hits at both old and mapped path = STOP and report:
  ```bash
  grep -n "uaa-amd64" .github/workflows/musl-build.yml                          # expect: 1+ hits (artifact name + header comments)
  grep -n "cargo build --release --target x86_64-unknown-linux-musl" .github/workflows/musl-build.yml   # expect: 1 hit (the build step you extend)
  grep -n "not a dynamic executable" .github/workflows/musl-build.yml           # expect: 1 hit (the static check you generalize)
  grep -n 'members = \["crates/\*"\]' Cargo.toml                                # expect: 1 hit (CP-01 merged; 0 hits = STOP, you are before wave 1)
  ```

## Step-by-step

1. Run the ⛔ START HERE block, then the anchor greps.
2. In `.github/workflows/musl-build.yml`, change the build step's `run:` to `cargo build --release --target x86_64-unknown-linux-musl --workspace` (keep the `CC_x86_64_unknown_linux_musl: musl-gcc` env line).
3. Replace the single-binary verify step with a discovery loop (same step name style):
   ```yaml
   - name: Verify every workspace binary is static
     run: |
       set -euo pipefail
       REL=target/x86_64-unknown-linux-musl/release
       FOUND=0
       for BIN in "$REL"/*; do
         [ -f "$BIN" ] && [ -x "$BIN" ] || continue
         case "$BIN" in *.d|*.so|*.rlib) continue;; esac
         FOUND=$((FOUND+1)); file "$BIN"
         if ldd "$BIN" 2>&1 | grep -vq "not a dynamic executable\|statically linked"; then
           echo "ERROR: $BIN is not static"; ldd "$BIN"; exit 1
         fi
       done
       [ "$FOUND" -ge 1 ] || { echo "ERROR: no binaries produced"; exit 1; }
       echo "verified $FOUND static binaries"
   ```
4. Keep the existing `Upload artifact (uaa-amd64)` step EXACTLY as is (`name: uaa-amd64`, path `.../ubuntu-autoinstall-agent`, `if-no-files-found: error`). After it, append one upload step per future daemon, all with `if-no-files-found: ignore` and the SAME commit-pinned upload-artifact action:
   ```yaml
   - name: Upload artifact (uaa-control-amd64)
     uses: actions/upload-artifact@<same pinned sha as the existing step>
     with:
       name: uaa-control-amd64
       path: target/x86_64-unknown-linux-musl/release/uaa-control
       if-no-files-found: ignore
   ```
   (Repeat for `uaa-web-amd64` → `.../uaa-web` and `uaa-pxe-amd64` → `.../uaa-pxe`.)
5. Update the workflow's header comment block: bump `# version:` to 1.1.0, `last-edited` line if present (add `# last-edited: 2026-07-10` if the header lacks it), keep the guid, and extend the DEPLOY note with one line: `# Later constellation binaries (uaa-control/uaa-web/uaa-pxe) upload as <name>-amd64 automatically once their crates exist.`
6. Validate the YAML parses (see How to test) — a broken workflow file fails every future PR, which is this task's only real risk.
7. Bump the file header (version + last-edited) on every file you touch; new files get a fresh 4-line header with a new uuid4 (`uuidgen | tr 'A-F' 'a-f'`).

## How to test

```bash
cargo test --lib --offline && cargo build --offline
# Expected: all tests pass (baseline 311 + prior-wave additions — this task changes no Rust, count unchanged from your rebase point), build clean
cargo clippy --offline -- -D warnings
# Expected: no warnings
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/musl-build.yml')); print('yaml ok')"
# Expected: yaml ok
command -v actionlint >/dev/null && actionlint .github/workflows/musl-build.yml || echo "actionlint not installed — skipped"
# Expected: no findings (or the skip line if actionlint is absent)
grep -c "if-no-files-found: ignore" .github/workflows/musl-build.yml
# Expected: 3
```

## Acceptance criteria

- [ ] Workspace build: `grep -n "cargo build --release --target x86_64-unknown-linux-musl --workspace" .github/workflows/musl-build.yml` → 1 hit.
- [ ] Per-binary static verification: `grep -n "verified .* static binaries\|is not static" .github/workflows/musl-build.yml` → 2 hits (loop success + failure lines), and the loop fails the job on any non-static binary (exit 1 path present).
- [ ] Frozen agent artifact: `grep -n "name: uaa-amd64" .github/workflows/musl-build.yml` → 1 hit with `if-no-files-found: error` still on that step.
- [ ] Forward-compatible artifacts: `grep -c "uaa-control-amd64\|uaa-web-amd64\|uaa-pxe-amd64" .github/workflows/musl-build.yml` → 3, each step with `if-no-files-found: ignore`; all upload steps use the SAME pinned action sha (`grep -c "actions/upload-artifact@" .github/workflows/musl-build.yml` → 4, one distinct sha).
- [ ] YAML valid: the python3 yaml.safe_load command above exits 0.
- [ ] Anti-over-suppression: N/A
- [ ] `cargo test --lib --offline && cargo build --offline` exits 0; clippy clean.
- [ ] File headers bumped on every changed file (`git diff origin/main --stat` shows only `.github/workflows/musl-build.yml`, its `# version:` bumped, guid unchanged).

## Commit message

```
ci(musl): build every workspace binary, static-verify each, artifact per binary (ws1-core)

musl-build.yml builds the workspace (--workspace), discovers every produced
executable under target/x86_64-unknown-linux-musl/release and fails on any
non-static one, keeps the frozen uaa-amd64 artifact (if-no-files-found: error —
the USB bootstrap depends on it), and adds forward-compatible per-binary
uploads for uaa-control/uaa-web/uaa-pxe with if-no-files-found: ignore so the
workflow is edited exactly once and later crates get artifacts automatically.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

Additive polarity: if `grep -n "uaa-control-amd64" .github/workflows/musl-build.yml` hits, the task is already applied — run the Acceptance criteria checks instead of re-applying. Rollback = revert the single commit; the workflow returns to the single-binary job, the frozen `uaa-amd64` artifact and every Rust source file stay untouched (this task changes no code).
