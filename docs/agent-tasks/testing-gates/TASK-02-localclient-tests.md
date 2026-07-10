<!-- file: docs/agent-tasks/testing-gates/TASK-02-localclient-tests.md -->
<!-- version: 1.0.0 -->
<!-- guid: 0a8ea1d4-e43e-417e-a562-5375769ddfa2 -->
<!-- last-edited: 2026-07-09 -->

# TASK-02 — Unit tests for LocalClient / local install flow (CommandExecutor seam) — today 0 tests exercise LocalClient (todo:local-tests)

**Priority:** P2 · **Effort:** M · **Recommended subagent:** Sonnet-class · rust-test subagent · **Why:** test-only; must use harmless commands (LocalClient runs real bash) · **Depends on:** none

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/testing-gates-localclient-tests" -b agent/testing-gates-localclient-tests origin/main
cd "$REPO/.worktrees/testing-gates-localclient-tests"
git rebase origin/main
```

(Protocol is also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is
authoritative for this task.)

## Goal

Add a `#[cfg(test)] mod tests` to `src/network/local.rs` that exercises the real
`LocalClient` — `connect`, `execute`, `execute_with_output`,
`execute_with_error_collection`, `check_silent`, `upload_file`/`download_file`,
`Default` — with HARMLESS commands only (`echo`, `true`, `false`, `cp` on tempfiles).
REUSE, do not reinvent: the existing mock seam for *callers* is the `CommandExecutor`
trait (`src/network/executor.rs:11` — `LocalClient`'s trait impl lives at
`executor.rs:89`); at least one test must go through that trait (call via
`&mut dyn CommandExecutor` or a `Box<dyn CommandExecutor>` holding a `LocalClient`) so
the trait wiring `SshInstaller` depends on is pinned, mirroring how `MockExecutor`
(`src/autoinstall/verify.rs`) and `RecordingMock` (`src/autoinstall/place.rs`) already
implement it for fakes. Do NOT add a mock inside `LocalClient`, do NOT create a new
executor trait or helper — the design spec LOCKED "real harmless commands; the seam
stays at CommandExecutor". Use `#[tokio::test]` exactly like the existing async tests
(e.g. `src/network/download.rs`).

## Background (verify before editing)

- `LocalClient` (`src/network/local.rs`) runs every command as real `bash -c` via
  `std::process::Command`. `LocalInstall`/`uaa install` (no `--remote`) routes the ENTIRE
  installer through it (`SshInstaller` wires `Box::new(LocalClient::new())`), yet the
  file has ZERO tests today — no `#[cfg(test)]` module, nothing under `tests/` references
  it. The 237 lib tests fake SSH via `CommandExecutor` mocks but never touch the local
  runner.
- API asymmetry to pin (read the file — this is the load-bearing semantic):
  `execute` and `execute_with_output` return `Err(AutoInstallError::ProcessError {
  command, exit_code, stderr })` on nonzero exit (with `exit_code: Some(n)`, and the
  `stderr` field falling back to captured stdout when the process's stderr is empty),
  while `execute_with_error_collection` returns `Ok((exit_code, stdout, stderr))` EVEN on
  nonzero exit — it collects, it does not fail. `check_silent` maps exit status to
  `Ok(true)`/`Ok(false)`. `connect` is a no-op `Ok(())`. `upload_file`/`download_file`
  shell out to `cp '<src>' '<dst>'`.
- **HARD RULES (restated):** this task is TEST-ONLY. NO installer logic, NO root, NO
  destructive commands — every command a test runs must be side-effect-free (`echo`,
  `true`, `false`, `cp` between tempfiles created by the test). Never touch 172.16.2.30
  or len-serv-003 (nothing here talks to any host). No secrets of any kind. Stay in your
  worktree; never push/PR/merge — the coordinator owns all git.
- **grep -c caveat for acceptance checks:** `grep -c` on a pattern with zero matches
  prints `0` AND exits nonzero, which aborts `set -e` scripts — use the
  `grep -c ... || true` idiom (or expect `1+` with `grep -n`) in every count-style check.

- **Re-verify these anchors before editing** — line numbers drift, they are a starting
  point only; an unexpected result = STOP and report:

  ```bash
  grep -n "pub trait CommandExecutor" src/network/executor.rs
  # expect: 1 hit at line 11
  grep -n "impl CommandExecutor for" src/network/executor.rs src/autoinstall/place.rs src/autoinstall/verify.rs
  # expect: 4 hits: executor.rs:45 (SshClient), executor.rs:89 (LocalClient), place.rs:404 (RecordingMock), verify.rs:547 (MockExecutor)
  grep -n "pub struct LocalClient" src/network/local.rs
  # expect: 1 hit at line 12
  grep -c "cfg(test)" src/network/local.rs
  # expect: outputs 0 (grep exits nonzero on zero count) — i.e. no test module yet; if it
  # outputs 1+, see Idempotency below
  grep -n "struct MockExecutor" src/autoinstall/verify.rs
  # expect: 1 hit at line 527
  grep -n "struct RecordingMock" src/autoinstall/place.rs
  # expect: 1 hit at line 380
  grep -n "LocalClient" src/network/ssh_installer/installer.rs
  # expect: 4 hits: lines 9 (doc), 17 (use), 38 and 81 (Box::new(LocalClient::new()))
  grep -n "tokio::test" src/network/download.rs
  # expect: 2+ hits (~lines 217, 225) — copy this async-test attribute shape
  ```

## Step-by-step

1. Run the ⛔ START HERE block and the anchor re-verify block above.
2. Open `src/network/local.rs`. Read the four execute-family methods end to end and note
   the exact error/ok semantics described in Background — the tests must encode them,
   not what seems "sensible".
3. Append (purely additive — do NOT modify any existing function, signature, log line,
   or import above the module) a `#[cfg(test)] mod tests` block at the end of the file:

   ```rust
   #[cfg(test)]
   mod tests {
       use super::*;
       use crate::network::executor::CommandExecutor;
       // All commands are side-effect-free: echo/true/false/cp on tempfiles. NO
       // installer logic, NO root, NO destructive commands.
   ```

   with these tests (all `#[tokio::test]` except the sync `Default` check):
   - `test_connect_is_noop_ok` — `LocalClient::new().connect("ignored-host",
     "ignored-user").await` → `Ok(())`.
   - `test_execute_true_succeeds` — `execute("true")` → `Ok(())`.
   - `test_execute_false_returns_process_error` — `execute("false")` → `Err`; match the
     error as `AutoInstallError::ProcessError { exit_code, .. }` and assert
     `exit_code == Some(1)`. Edge semantics: nonzero exit MUST be an `Err`, and the exit
     code MUST be preserved (fail-closed — a future refactor that swallows the code
     breaks this test).
   - `test_execute_with_output_captures_stdout` — `execute_with_output("echo hello")` →
     `Ok(s)` with `s == "hello\n"` (trailing newline included — assert the exact string,
     not `contains`).
   - `test_execute_with_output_failure_prefers_stderr` — a failing command that writes to
     stderr, e.g. `echo oops >&2; exit 3` → `Err(ProcessError { exit_code: Some(3),
     stderr, .. })` with `stderr` containing `oops`. Also cover the fallback: a failing
     command with EMPTY stderr but stdout text (e.g. `echo out-only; exit 4`) puts the
     stdout text in the error's `stderr` field — that fallback is intentional, do not
     "fix" it.
   - `test_execute_with_error_collection_nonzero_is_ok_tuple` —
     `execute_with_error_collection("echo so; echo se >&2; exit 5", "desc")` →
     `Ok((5, stdout, stderr))` with `stdout` containing `so` and `stderr` containing
     `se`. Edge semantics stated twice on purpose: this method returns `Ok` EVEN on
     nonzero exit — asserting `Err` here would be wrong.
   - `test_check_silent_true_false` — `check_silent("true")` → `Ok(true)`;
     `check_silent("false")` → `Ok(false)` (an `Ok(false)`, NOT an `Err`).
   - `test_upload_download_copy_tempfiles` — create a `tempfile::TempDir` (`tempfile`
     is ALREADY in Cargo.toml — verify with `grep -n "^tempfile" Cargo.toml`, expect
     `tempfile = "3.8"`; do NOT add a new dependency), write a small file,
     `upload_file(src, dst)` then `download_file(dst, back)`, assert byte-identical
     content.
   - `test_trait_object_execute_matches_inherent` — put a `LocalClient` behind
     `Box<dyn CommandExecutor>` and call the trait's `execute`/`execute_with_output` with
     `true` / `echo via-trait`; assert same results as the inherent calls (pins the
     `executor.rs:89` impl that `SshInstaller` depends on).
   - `test_default_matches_new` — `#[test]` (sync): `LocalClient::default()` constructs
     (both `new()` and `default()` yield a usable client; the `host` field is private —
     just prove construction + one `check_silent("true")` in a follow-up async test if
     needed, or assert via behavior not fields).
4. Do NOT test `collect_debug_info` (it shells out to `journalctl`/`dmesg`/`zpool` —
   environment-dependent and slow) and do NOT test `disconnect` beyond, at most, calling
   it for coverage; both are out of scope. NO other file may change.
5. Bump the header of `src/network/local.rs`: `// version: 1.0.0` → `// version: 1.1.0`,
   and add `// last-edited: 2026-07-09` (the file currently lacks the last-edited line —
   add it after the guid line, keep the existing `// guid:` unchanged).
6. Run the gates below; fix warnings until clippy is clean.

## How to test

```bash
cargo test --lib --offline
# Expected: 247+ passed; 0 failed (baseline 237 + the ~10 new LocalClient tests)
cargo test --lib --offline network::local
# Expected: 10+ passed; 0 failed (the new module in isolation)
cargo build --offline
# Expected: exit 0
cargo clippy --offline
# Expected: exit 0, no new warnings
grep -c "cfg(test)" src/network/local.rs || true
# Expected: 1 (module added; the || true guards the zero-match nonzero exit)
```

## Acceptance criteria

- [ ] Tests green: `cargo test --lib --offline` → 247+ passed, 0 failed; `cargo build --offline` exits 0; `cargo clippy --offline` clean.
- [ ] Test module present: `grep -c "cfg(test)" src/network/local.rs || true` outputs `1` (was `0`).
- [ ] Fail-path pinned: `grep -n "test_execute_false_returns_process_error" src/network/local.rs` → 1 hit, and the test asserts `exit_code == Some(1)` inside a `ProcessError` match (`grep -n "Some(1)" src/network/local.rs` → 1+ hit).
- [ ] Collection asymmetry pinned: `grep -n "test_execute_with_error_collection_nonzero_is_ok_tuple" src/network/local.rs` → 1 hit (nonzero exit yields `Ok((exit, stdout, stderr))`, never `Err`).
- [ ] Trait seam exercised: `grep -n "dyn CommandExecutor" src/network/local.rs` → 1+ hit (a test drives `LocalClient` through the `CommandExecutor` trait object).
- [ ] Harmless commands only: `grep -En "rm |mkfs|cryptsetup|sgdisk|zpool|dd |sudo" src/network/local.rs || true` prints NO hits inside the new `mod tests` (the pre-existing `collect_debug_info` production strings are the only allowed matches outside it).
- [ ] Purely additive: `git diff origin/main --stat` touches ONLY `src/network/local.rs`, and `git diff origin/main -- src/network/local.rs` shows no changes to existing functions (header lines + appended test module only).
- [ ] File headers bumped: `grep -n "last-edited: 2026-07-09" src/network/local.rs` → 1 hit; `grep -n "version: 1.1.0" src/network/local.rs` → 1 hit; guid unchanged.
- [ ] Anti-over-suppression: N/A (no filter/guard/veto/skip path is added — test-only change; the happy-path tests `test_execute_true_succeeds` / `test_execute_with_output_captures_stdout` cover success behavior regardless).

## Commit message

```
test(network): add LocalClient unit tests via harmless real commands

Adds a #[cfg(test)] module to src/network/local.rs (previously 0 tests):
connect no-op, execute true/false exit codes, stdout capture, stderr-preferred
ProcessError text, execute_with_error_collection Ok-on-nonzero asymmetry,
check_silent, upload/download tempfile round-trip, Default, and one test
driving LocalClient through the CommandExecutor trait object. Commands are
side-effect-free (echo/true/false/cp); no production code changes.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

Idempotency (additive — check for the NEW thing's presence):
`grep -n "test_execute_with_error_collection_nonzero_is_ok_tuple" src/network/local.rs`
— if this hits (and `grep -c "cfg(test)" src/network/local.rs || true` outputs `1+`),
the test module already exists; run the acceptance checks instead of re-applying.
Rollback: `git revert` of the single commit returns `src/network/local.rs` to its
untested state; the module is `#[cfg(test)]`-only, so shipping binaries are identical
either way and no sibling task is affected.
