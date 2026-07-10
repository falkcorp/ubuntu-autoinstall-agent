<!-- file: docs/agent-tasks/uaa-web/TASK-03-iso-build-jobs.md -->
<!-- version: 1.0.0 -->
<!-- guid: d4e2ae9b-de7e-4c1f-a4e5-d885b835cce9 -->
<!-- last-edited: 2026-07-10 -->

# TASK-03 — Fill iso_jobs.rs: BuildIso/GetBuildJob/ListIsos as a detached job runner wrapping the uaa-core iso pipeline, never inline (ws5-web)

**Priority:** P2 · **Effort:** M · **Recommended subagent:** Sonnet-class · rust-service subagent · **Why:** job lifecycle + reuse of TP-01/TP-03 library entry points — no new pipeline logic, only orchestration around it. · **Depends on:** TASK-01 (wave-7 gated: WB-01 merged — the `crates/uaa-web/src/iso_jobs.rs` stub must exist on `origin/main`) + TP-01 and TP-03 already merged in waves 2–3 (`crates/uaa-core/src/iso/{remaster,image_build}.rs`)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/uaa-web-iso-build-jobs" -b agent/uaa-web-iso-build-jobs origin/main
cd "$REPO/.worktrees/uaa-web-iso-build-jobs"
git rebase origin/main
```

(Protocol also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

## Goal

Fill `crates/uaa-web/src/iso_jobs.rs` (the ONLY file this task edits) per spec §C4: `BuildIso` returns a job id immediately and runs the build as a **detached tokio job wrapping the tooling-port pipeline, never inline** in the RPC handler (spec: "`BuildIso` = detached tokio job wrapping the tooling-port pipeline, never inline"; topology table: "ISO build jobs (detached)"); `GetBuildJob` reports job state; `ListIsos` inventories `<webroot>/isos/`. Purely additive within the one stub file.

REUSE — do not invent parallels for any of these:

- **ISO pipeline entry points** from TP-01/TP-03: `crates/uaa-core/src/iso/remaster.rs` and `crates/uaa-core/src/iso/image_build.rs` (re-read their public signatures at execution time — `grep -n "pub " crates/uaa-core/src/iso/remaster.rs | head`). Do NOT shell out to `scripts/make-ssh-ready-iso.sh` or re-implement any xorriso/squashfs step; the RPC layer only ORCHESTRATES the library calls.
- **`CommandExecutor`** (`src/network/executor.rs` — the proven test seam) is what the TP pipeline already uses for xorriso/unsquashfs/mksquashfs; your job runner passes an executor through, so tests inject mocks and NO external tool ever runs in `cargo test`.
- **Request/response types** from `crates/uaa-proto` (`proto/uaa/web/v1/web.proto`, CP-02): `BuildIsoRequest/Response` ("detached job id"), `BuildJob`, `ListIsosRequest/Response`. Do NOT define parallel structs.

**HARD RULES (non-negotiable):**
- NO hardware actions. Validate ONLY in-repo (`cargo`) and, where a brief says so, the QEMU+swtpm harness (`scripts/vm-validate.sh`). Code that COULD touch hardware is written and unit-tested against mock executors only.
- NEVER wipe, write to, or deploy on 172.16.2.30 ("the server") or len-serv-003.
- `disk_device` is read from the live target at runtime, never guessed or hardcoded.
- ipmitool runs via `ssh 172.16.2.30`, never on macOS.
- NEVER power on unimatrixone (U1).
- No real secret in any file: `REPLACE_AT_PLACE_TIME` placeholders stay placeholders.
- Stay inside your worktree; never `git push`, `gh pr`, or merge — report done and stop.

## Background (verify before editing)

- WB-01's `grpc.rs` already delegates `BuildIso`/`GetBuildJob`/`ListIsos` to free functions in this stub — you replace the `unimplemented` bodies only; siblings TASK-02/04 own `placement.rs`/`publish.rs` in the same wave, so touching any other file is a merge conflict by construction.
- Edge semantics, spelled out (and repeated in acceptance):
  - `GetBuildJob` with an unknown job id → `Status::not_found` (hard 404 for an unknown single resource — same convention as Decision 12's parity rule).
  - `ListIsos` on an empty/missing `isos/` dir → `Ok` with an EMPTY list (empty-200 collection convention), never an error.
  - Job states: `Queued → Running → Succeeded | Failed` — monotonic, never backwards; `Failed` carries the error string; finished jobs keep their record for the daemon's lifetime (in-memory registry; persistence is out of scope v1).
  - A second `BuildIso` while one is `Running` is ACCEPTED and queued/run concurrently only if the output filenames differ; identical output path → `Status::already_exists` (two jobs writing one ISO path is a corruption path — fail-closed).
  - Build output lands in `<webroot>/isos/` via tmp+rename (the pipeline writes to a scratch path; the finalize step renames into `isos/` so :8081 readers never fetch a partial ISO).
- Testability seam: the job runner takes the pipeline as an injected async closure/trait (`JobRunner: Fn(BuildSpec) -> Future<Result<PathBuf>>`) whose PRODUCTION value calls the TP-01/TP-03 library; tests inject a stub that sleeps/succeeds/fails on command. No xorriso, no network, no live tools in `cargo test`.
- **Path map:** after CP-01 (wave 1) merges, `src/**` lives at `crates/uaa-core/src/**` and the CLI at `crates/uaa/src/**`. The greps below cite pre-move paths (verifiable on today's main); at execution time run them at the old path, then the mapped path. Zero hits at BOTH = STOP and report.
- **Re-verify these anchors before editing** — line numbers drift; zero hits at both old and mapped path = STOP and report:
  ```bash
  grep -n "detached" docs/specs/constellation-design.md          # expect: 2+ hits (detached-job lock, spec C4 + proto surface)
  grep -n "pub trait CommandExecutor" src/network/executor.rs    # expect: 1 hit (mapped: crates/uaa-core/src/network/executor.rs)
  # Post-merge greps (files exist only after waves 1-6 — run at execution time):
  grep -n "fn build_iso" crates/uaa-web/src/iso_jobs.rs          # expect: 1 hit (WB-01 stub you are filling)
  grep -n "pub " crates/uaa-core/src/iso/remaster.rs | head -5   # expect: hits — TP-01 entry points; re-read for exact signatures
  grep -n "pub " crates/uaa-core/src/iso/image_build.rs | head -5 # expect: hits — TP-03 entry points
  grep -n "rpc BuildIso" proto/uaa/web/v1/web.proto              # expect: 1 hit (CP-02)
  ```

## Step-by-step

1. Run the ⛔ START HERE block, then every anchor grep above. Any zero-hit grep (at old AND mapped path) → STOP and report.

2. **Job model** — in `iso_jobs.rs`:
   ```rust
   #[derive(Debug, Clone, PartialEq)]
   pub enum JobState { Queued, Running, Succeeded, Failed(String) }
   pub struct JobRecord { pub id: String /* uuid4 */, pub state: JobState,
       pub output: PathBuf, pub started_at: SystemTime, pub finished_at: Option<SystemTime> }
   /// Shared registry: Arc<Mutex<HashMap<String, JobRecord>>>. In-memory only (v1).
   pub struct JobRegistry { /* ... */ }
   ```
   State transitions happen ONLY through registry methods (`mark_running`, `mark_finished`) so the monotonic rule is enforced in one place.

3. **Runner seam** — `pub trait IsoBuildRunner: Send + Sync { async fn run(&self, spec: &BuildSpec) -> Result<PathBuf, String>; }` (or an `Arc<dyn Fn ...>` — match the codebase's async-trait idiom, check how `CommandExecutor` is declared). The production impl maps the request to the TP-01/TP-03 library calls (`iso::remaster::*` for remaster requests, `iso::image_build::*` for image builds) with a real `CommandExecutor`; tests inject a `MockRunner`.

4. **`build_iso`** — validate the request (source ISO name: reject path components with `/`, `..`, NUL → `invalid_argument`; compute the output path under `<webroot>/isos/`); if any live job (Queued/Running) targets the same output path → `Status::already_exists` (fail-closed duplicate guard). Otherwise insert `Queued`, `tokio::spawn` the detached task (mark Running → run the injected runner → tmp+rename the produced file into `isos/` → mark Succeeded/Failed), and return the job id IMMEDIATELY — the RPC never awaits the build.

5. **`get_build_job`** — look up by id; unknown → `Status::not_found`; known → map `JobRecord` to the proto `BuildJob` (state, output filename, timestamps, failure message if any).

6. **`list_isos`** — read `<webroot>/isos/` (missing dir → empty list, `Ok`); return name/size/mtime per `*.iso` entry, skipping `*.tmp.*` in-flight files.

7. **Unit tests** (`#[cfg(test)] mod tests`, tempdir webroot, `MockRunner` with a controllable oneshot so tests deterministically observe Queued/Running before completion):

   | Test | Asserts |
   |---|---|
   | `test_build_iso_returns_immediately` | **anti-over-suppression / happy path:** `build_iso` with a valid request + MockRunner returns a job id BEFORE the runner completes; after releasing the mock, polling the registry reaches `Succeeded` and the output file exists in `isos/` (the duplicate/validation guards do not block a legitimate build) |
   | `test_build_failure_recorded` | MockRunner returns `Err("boom")` → state `Failed("boom")`, `finished_at` set, no file in `isos/` |
   | `test_get_build_job_unknown_not_found` | random uuid → `Status::not_found` |
   | `test_duplicate_output_rejected` | second `build_iso` targeting the same output while the first is Running → `already_exists`; the FIRST job is unaffected and still completes |
   | `test_list_isos_empty_ok` | missing `isos/` dir → `Ok(vec![])` |
   | `test_list_isos_skips_tmp` | `a.iso` + `b.iso.tmp.123` in dir → only `a.iso` listed |
   | `test_no_inline_build` | the RPC future resolves while the MockRunner is still blocked (proves detachment) |

8. Bump the file header (version + last-edited) on every file you touch; new files get a fresh 4-line header with a new uuid4 (`uuidgen | tr 'A-F' 'a-f'`).

## How to test

```bash
cargo test --lib --offline && cargo build --offline
# Expected: all tests pass (wave-6 count + the 7 tests above), build clean
cargo clippy --offline -- -D warnings
# Expected: no warnings
cargo test --offline -p uaa-web iso_jobs
# Expected: 7 passed; 0 failed
grep -rn "xorriso\|mksquashfs\|unsquashfs" crates/uaa-web/src/iso_jobs.rs
# Expected: 0 hits (tool invocation lives in uaa-core's pipeline, not here)
git diff origin/main --stat
# Expected: ONLY crates/uaa-web/src/iso_jobs.rs (+ its header bump)
```

## Acceptance criteria

- [ ] Detached, never inline: `test_build_iso_returns_immediately` and `test_no_inline_build` pass; `grep -n "tokio::spawn" crates/uaa-web/src/iso_jobs.rs` → ≥1 hit inside `build_iso`'s path.
- [ ] Reuse, not reimplementation: `grep -rn "iso::remaster\|iso::image_build" crates/uaa-web/src/iso_jobs.rs` → ≥1 hit (production runner calls TP-01/TP-03 libraries); `grep -rn "xorriso\|mksquashfs" crates/uaa-web/src/iso_jobs.rs` → 0 hits.
- [ ] Edge semantics proven: `test_get_build_job_unknown_not_found` (hard 404), `test_list_isos_empty_ok` (empty-200 collection), `test_build_failure_recorded` all pass.
- [ ] **Anti-over-suppression:** `test_duplicate_output_rejected` ALSO asserts the first (legitimate) job still completes `Succeeded` — the duplicate guard does not suppress the happy path (paired with `test_build_iso_returns_immediately`).
- [ ] Single-file scope: `git diff origin/main --stat` lists only `crates/uaa-web/src/iso_jobs.rs`.
- [ ] `cargo test --lib --offline && cargo build --offline` exits 0; clippy clean.
- [ ] File headers bumped on every changed file (`git diff origin/main --stat` files each show a version bump; guids unchanged).

## Commit message

```
feat(web): fill ISO build-job RPCs — detached runner over the uaa-core pipeline (ws5-web)

crates/uaa-web/src/iso_jobs.rs: BuildIso spawns a detached tokio job (RPC
returns the job id immediately, never builds inline) wrapping the TP-01/TP-03
iso library entry points through an injected runner seam so tests mock the
pipeline — no xorriso/squashfs in this file. In-memory JobRegistry with
monotonic Queued->Running->Succeeded|Failed, hard-404 unknown job, empty-200
ListIsos, tmp-skipping inventory, and a fail-closed same-output duplicate
guard proven not to block legitimate builds. 7 unit tests.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

Idempotency (additive — check for the NEW thing's presence): if `grep -n "JobRegistry" crates/uaa-web/src/iso_jobs.rs` hits (the stub had only `unimplemented`), the task is already applied — run the acceptance checks instead of re-applying. Rollback = revert the single commit; the three RPCs return `Unimplemented` again, `grpc.rs`, `placement.rs`, `publish.rs`, and the uaa-core iso pipeline stay untouched, and no jobs/ISOs persist anywhere (in-memory registry, tempdir tests).
