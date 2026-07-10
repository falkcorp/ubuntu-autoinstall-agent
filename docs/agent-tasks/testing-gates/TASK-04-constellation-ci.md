<!-- file: docs/agent-tasks/testing-gates/TASK-04-constellation-ci.md -->
<!-- version: 1.0.0 -->
<!-- guid: 34d05142-bdd4-4c4f-a110-182ba7ebf3c8 -->
<!-- last-edited: 2026-07-10 -->

# TASK-04 — Add constellation-ci.yml: workspace clippy + test + SPA build check on PRs (ws10-gates)

**Priority:** P2 · **Effort:** S · **Recommended subagent:** Haiku-class · ci-yaml subagent · **Why:** mechanical CI yml mirroring existing workflow patterns. · **Depends on:** none inside this workstream (wave-2 gated: CP-01 workspace conversion MERGED to `origin/main` first — the workflow's `--workspace` flags target the post-CP-01 layout, though they are also valid against a single crate)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/testing-gates-constellation-ci" -b agent/testing-gates-constellation-ci origin/main
cd "$REPO/.worktrees/testing-gates-constellation-ci"
git rebase origin/main
```

(Protocol also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

## Goal

Create ONE new file `.github/workflows/constellation-ci.yml` that gates PRs and pushes to `main` with: (1) workspace-wide `cargo clippy -- -D warnings`, (2) `cargo test --lib --offline` across the workspace, and (3) the SPA build (`cd web && npm ci && npm run build`) — the SPA step guarded so it runs when `web/` exists (CT-08 lands it in the same global wave) and prints an explicit skip line when it does not. Mirror the proven patterns of `.github/workflows/musl-build.yml` exactly: pinned-SHA actions, `submodules: recursive` checkout (the `.standards/` submodule), and the cargo registry/git/target cache. From the design spec (`docs/specs/constellation-design.md`, Scope: "Rust (new cargo workspace, new service binaries), protobuf, Node SPA (build tooling only)") — this workflow is the PR-time floor under every constellation wave; the musl matrix (CP-06) stays a separate workflow.

REUSE — do not invent parallels:

- **Pinned action SHAs from `.github/workflows/musl-build.yml`** (verify: greps below): `actions/checkout@9c091bb21b7c1c1d1991bb908d89e4e9dddfe3e0 # v7.0.0` and `actions/cache@0057852bfaa89a56745cba8c7296529d2fc39830 # v4.3.0`. Copy them verbatim — do NOT float to a tag or a newer SHA.
- **The cache stanza** (paths `~/.cargo/registry`, `~/.cargo/git`, `target`; key on `hashFiles('Cargo.lock')`) — same shape, new key prefix `constellation-`.

Purely additive: one new file. No edits to any existing workflow, script, or crate.

## Background (verify before editing)

- Today `.github/workflows/` contains `ci.yml`, `musl-build.yml`, `release.yml`, `reusable-ci.yml`, `reusable-release.yml`, `sync-receiver.yml`, `workflow-scripts-tests.yml` — no `constellation-ci.yml`. The file you create must not collide with or edit any of them.
- `musl-build.yml` is the style ground truth: 4-line `# file:` header, `on: push(main)/pull_request(main)/workflow_dispatch`, `permissions: contents: read`, `runs-on: ubuntu-latest`, pinned SHAs, recursive-submodule checkout.
- `--offline` needs a populated registry: run `cargo fetch` (online, cache-backed) BEFORE the offline test step. Clippy compiles everything anyway, so order the steps fetch → clippy → offline test.
- `web/` does NOT exist on today's main (CT-08 creates it, global wave 2). The workflow must be green both before and after CT-08 merges — hence the directory guard with an explicit skip echo (never a silent no-op).
- `ubuntu-latest` runners ship Node 20+; no `setup-node` step is needed for `npm ci && npm run build`.
- Edge semantics of the guard, spelled out: `web/` absent → the step echoes `SPA build skipped: web/ not present yet` and exits 0 (job stays green); `web/` present → `npm ci && npm run build` run and their failure FAILS the job (no `|| true` anywhere).
- **Re-verify these anchors before editing** — line numbers drift; zero hits at both
  old and mapped path = STOP and report:
  ```bash
  grep -n "cargo build --release --target" .github/workflows/musl-build.yml
  # expect: 1 hit (line ~59) — the workflow you mirror is intact
  grep -n "actions/checkout@9c091bb21b7c1c1d1991bb908d89e4e9dddfe3e0" .github/workflows/musl-build.yml
  # expect: 1 hit — pinned checkout SHA to copy verbatim
  grep -n "actions/cache@0057852bfaa89a56745cba8c7296529d2fc39830" .github/workflows/musl-build.yml
  # expect: 1 hit — pinned cache SHA to copy verbatim
  grep -n "submodules: recursive" .github/workflows/musl-build.yml
  # expect: 1 hit — checkout option to copy
  ls .github/workflows/constellation-ci.yml 2>/dev/null | wc -l
  # expect: 0 — the file does not exist yet
  ```

## Step-by-step

1. Run the ⛔ START HERE block, then every anchor grep above. Any zero-hit grep → STOP and report.

2. **Create `.github/workflows/constellation-ci.yml`** with a fresh 4-line header (`# file: .github/workflows/constellation-ci.yml`, `# version: 1.0.0`, a NEW guid from `uuidgen | tr 'A-F' 'a-f'`, `# last-edited: 2026-07-10`) and this exact structure:

   ```yaml
   name: Constellation CI

   on:
     push:
       branches: [main]
     pull_request:
       branches: [main]
     workflow_dispatch: {}

   permissions:
     contents: read

   jobs:
     workspace-checks:
       name: Workspace clippy + tests + SPA build
       runs-on: ubuntu-latest
       steps:
         - name: Checkout code
           uses: actions/checkout@9c091bb21b7c1c1d1991bb908d89e4e9dddfe3e0 # v7.0.0
           with:
             submodules: recursive

         - name: Cache cargo
           uses: actions/cache@0057852bfaa89a56745cba8c7296529d2fc39830 # v4.3.0
           with:
             path: |
               ~/.cargo/registry
               ~/.cargo/git
               target
             key: constellation-${{ runner.os }}-${{ hashFiles('Cargo.lock') }}

         - name: Fetch dependencies
           run: cargo fetch

         - name: Clippy (workspace, deny warnings)
           run: cargo clippy --workspace --all-targets -- -D warnings

         - name: Unit tests (workspace, offline)
           run: cargo test --workspace --lib --offline

         - name: SPA build (web/)
           run: |
             if [ -d web ]; then
               cd web && npm ci && npm run build
             else
               echo "SPA build skipped: web/ not present yet"
             fi
   ```

   Copy the SHAs character-for-character from `musl-build.yml`. Do not add `|| true`, `continue-on-error`, or any other failure-swallowing to any step.

3. **Prove both guard branches locally** (the SPA step's `run` block is plain bash):
   ```bash
   D=$(mktemp -d); cd "$D"
   bash -c 'if [ -d web ]; then echo BUILD; else echo "SPA build skipped: web/ not present yet"; fi'
   mkdir web
   bash -c 'if [ -d web ]; then echo BUILD; else echo "SPA build skipped: web/ not present yet"; fi'
   cd -
   ```
   First run prints the skip line; second prints `BUILD`. Record both outputs in your report.

4. Bump the file header (version + last-edited) on every file you touch; new files get a fresh 4-line header with a new uuid4 (`uuidgen | tr 'A-F' 'a-f'`).

## How to test

```bash
cargo test --lib --offline && cargo build --offline
# Expected: all tests pass (baseline 311 — this task adds no Rust code), build clean
cargo clippy --offline -- -D warnings
# Expected: no warnings (no Rust touched)
python3 -c "import yaml; yaml.safe_load(open('.github/workflows/constellation-ci.yml')); print('YAML OK')"
# Expected: YAML OK  (if PyYAML is unavailable on this box, note that and rely on the greps below)
grep -c "uses: actions/" .github/workflows/constellation-ci.yml
# Expected: 2 (checkout + cache, both pinned)
grep -n "@main\|@master\|@v[0-9]" .github/workflows/constellation-ci.yml | grep -v "#"
# Expected: 0 hits (no floating action refs — SHAs only)
git diff origin/main --stat -- .github/workflows/
# Expected: exactly one file listed: constellation-ci.yml
```

## Acceptance criteria

- [ ] File exists with all three checks: `grep -n "cargo clippy --workspace --all-targets -- -D warnings" .github/workflows/constellation-ci.yml` → 1 hit; `grep -n "cargo test --workspace --lib --offline" .github/workflows/constellation-ci.yml` → 1 hit; `grep -n "npm ci && npm run build" .github/workflows/constellation-ci.yml` → 1 hit.
- [ ] Pinned SHAs copied verbatim: `grep -n "actions/checkout@9c091bb21b7c1c1d1991bb908d89e4e9dddfe3e0" .github/workflows/constellation-ci.yml` → 1 hit; `grep -n "actions/cache@0057852bfaa89a56745cba8c7296529d2fc39830" .github/workflows/constellation-ci.yml` → 1 hit; recursive submodules: `grep -n "submodules: recursive" .github/workflows/constellation-ci.yml` → 1 hit.
- [ ] `cargo fetch` precedes the offline test step: `grep -n "cargo fetch\|--offline" .github/workflows/constellation-ci.yml` shows `cargo fetch` on an earlier line than `cargo test --workspace --lib --offline`.
- [ ] No failure-swallowing: `grep -n "continue-on-error\||| true" .github/workflows/constellation-ci.yml` → 0 hits.
- [ ] Anti-over-suppression: the `[ -d web ]` guard does not suppress the build when `web/` exists — Step 3's two-branch shell proof executed with outputs recorded (`BUILD` printed when the dir exists, the named skip line when not), and `grep -n "SPA build skipped: web/ not present yet" .github/workflows/constellation-ci.yml` → 1 hit (skip is loud, never silent).
- [ ] Nothing else changed: `git diff origin/main --stat` lists only `.github/workflows/constellation-ci.yml`.
- [ ] `cargo test --lib --offline && cargo build --offline` exits 0; clippy clean.
- [ ] File headers bumped on every changed file (`git diff origin/main --stat` files each show a version bump; guids unchanged — here, the one new file carries a fresh 4-line header).

## Commit message

```
ci(testing): add constellation-ci.yml workspace clippy+test+SPA gate (ws10-gates)

New .github/workflows/constellation-ci.yml mirroring musl-build.yml
patterns (pinned checkout/cache SHAs, recursive submodules, cargo cache):
cargo fetch, workspace clippy -D warnings, cargo test --workspace --lib
--offline, and the web/ SPA build (npm ci && npm run build) guarded by a
loud directory check so the job is green both before and after CT-08
lands web/. No existing workflow touched.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

Additive. If `grep -n "cargo clippy --workspace --all-targets -- -D warnings" .github/workflows/constellation-ci.yml` hits, the task is already applied — run the Acceptance criteria checks instead of re-applying. Rollback = revert the single commit; it removes only the new workflow file — every existing workflow (`ci.yml`, `musl-build.yml`, `release.yml`, the reusables), all crates, and all scripts stay untouched.
