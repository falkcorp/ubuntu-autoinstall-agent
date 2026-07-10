<!-- file: docs/agent-tasks/tooling-port/TASK-01-iso-remaster.md -->
<!-- version: 1.0.0 -->
<!-- guid: 4e4e2bf2-b8f6-4e91-b8c3-b61fd296f69e -->
<!-- last-edited: 2026-07-10 -->

# TASK-01 — `uaa iso remaster`: port make-ssh-ready-iso.sh (xorriso extract/patch/repack, idempotent cmdline tokens, --autoinstall opt-in) (ws9-tooling)

**Priority:** P1 · **Effort:** L · **Recommended subagent:** Sonnet-class · rust-port subagent · **Why:** faithful port of a battle-tested pipeline incl. El Torito preservation · **Depends on:** CP-01 (wave-2 gated: `core-proto/TASK-01` workspace conversion MERGED and this worktree rebased — the stub file this task fills does not exist before then)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/tooling-port-iso-remaster" -b agent/tooling-port-iso-remaster origin/main
cd "$REPO/.worktrees/tooling-port-iso-remaster"
git rebase origin/main
```

(Protocol also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

## Goal

Fill the CP-01-created stub `crates/uaa-core/src/iso/remaster.rs` with a faithful Rust port of `scripts/make-ssh-ready-iso.sh` (v1.2.0, 135 lines), and replace the `todo!()` body of the pre-wired `uaa iso remaster` CLI dispatch (created by CP-01 in `crates/uaa/src/`). The shell script stays in place UNCHANGED — it is deleted only by TASK-05 after the M6 cutover gate (spec Goals: "Retire the ported shell tools … only after their Rust replacement passes its gate"; spec Decision 17 places iso tooling in `crates/uaa-core`). Purely additive: no existing module, test, or golden fixture is modified.

REUSE — do not invent parallels:

- **`CommandExecutor`** trait (pre-move: `src/network/executor.rs` — verify: `grep -n "pub trait CommandExecutor" src/network/executor.rs`; post-CP-01: `crates/uaa-core/src/network/executor.rs`). EVERY `xorriso` invocation goes through `executor.execute_with_output(...)` so tests inject a mock. Do NOT call `std::process::Command` anywhere in this change.
- **Mock idiom:** mirror `MockExecutor` (`src/autoinstall/verify.rs` — verify: `grep -n "struct MockExecutor" src/autoinstall/verify.rs`, a HashMap command→response mock) inside `#[cfg(test)]`, extended with a `Vec<String>` of recorded commands. Do NOT add a mocking crate.
- **`AutoInstallError::ConfigError` / `AutoInstallError::SystemError`** (`src/error.rs`) for all error paths. No new error enum.

## Background (verify before editing)

- Ground truth is `scripts/make-ssh-ready-iso.sh`. Its behavior, in full (port ALL of it):
  - Inputs: `[--autoinstall] [--on-done poweroff|reboot|shell] <input.iso> [output.iso]`; env equivalents `UAA_AUTOINSTALL=1`, `UAA_ON_DONE`, seed dir override `UAA_SEED_DIR` (default `installer-image/nocloud` relative to the repo). Invalid `--on-done` value = hard error.
  - Input may be a regular `.iso` FILE or a BLOCK DEVICE: an existing block device is addressed as `stdio:<path>`; an input already starting with `stdio:` passes through; anything else missing = error. Output path starting with `stdio:` or `/dev/` = REFUSED (never write output to a device).
  - Preflight: `xorriso` must exist on PATH (probe via the executor, e.g. `command -v xorriso`); `<seed>/user-data` AND `<seed>/meta-data` must exist.
  - Extract `/boot/grub/grub.cfg` (missing = hard error) and optionally `/boot/grub/loopback.cfg` (missing = fine, remember `have_loopback`) via `xorriso -osirrox on -indev <dev> -extract <iso-path> <local-path>`.
  - Patch 1 (`patch_cfg`): on every `linux`/`linuxefi` line booting `/casper/vmlinuz`, insert ` ds=nocloud\;s=/cdrom/nocloud/ autoinstall=0` right after the vmlinuz path. The `;` MUST stay backslash-escaped for GRUB. IDEMPOTENT: if the text already contains `ds=nocloud`, skip (log "already patched", not an error).
  - Patch 2 (`patch_autoinstall`, ONLY when autoinstall opted in): insert ` uaa.autoinstall` (plus ` uaa.on_done=<action>` when set) after the vmlinuz path. INDEPENDENTLY idempotent: skip if `uaa.autoinstall` already present — re-running with `--autoinstall` on an already-SSH-ready ISO only adds the token; running twice adds nothing.
  - Repack preserving El Torito boot: `xorriso -indev <dev> -outdev <out> -boot_image any replay -compliance no_emul_toc -map <patched grub.cfg> /boot/grub/grub.cfg -map <seed-dir> /nocloud` (+ loopback map iff extracted). The `-boot_image any replay` + `no_emul_toc` pair is load-bearing — a repack without them produces an unbootable stick.
- Do the patching IN MEMORY with pure functions on `&str` (not sed): pure functions are what make this unit-testable. Only extract/repack shell out (via the executor).

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

**Path map:** after CP-01 (wave 1) merges, `src/**` lives at
`crates/uaa-core/src/**` and the CLI at `crates/uaa/src/**`. The greps below cite
pre-move paths (verifiable on today's main); at execution time run them at the old
path, then the mapped path. Zero hits at BOTH = STOP and report.

- **Re-verify these anchors before editing** — line numbers drift; zero hits at both
  old and mapped path = STOP and report:
  ```bash
  grep -n "replay" scripts/make-ssh-ready-iso.sh
  # expect: 1+ hits (the -boot_image any replay repack, ~line 129)
  grep -n "patch_cfg\|already" scripts/make-ssh-ready-iso.sh | head -4
  # expect: hits (idempotent patch guard, ~lines 96-99)
  grep -n "pub trait CommandExecutor" src/network/executor.rs
  # expect: 1 hit (then at crates/uaa-core/src/network/executor.rs)
  grep -n "struct MockExecutor" src/autoinstall/verify.rs
  # expect: 1 hit (then at crates/uaa-core/src/autoinstall/verify.rs)
  ls installer-image/nocloud/user-data installer-image/nocloud/meta-data
  # expect: both listed (default seed dir contents)
  ```
- Execution-time checks (crates/ exists only after CP-01 — not resolvable on today's main):
  ```bash
  test -f crates/uaa-core/src/iso/remaster.rs && grep -n "todo!" crates/uaa-core/src/iso/remaster.rs
  # expect: file exists with a headered stub; if the stub is absent STOP — CP-01 not merged
  grep -rn "remaster" crates/uaa/src/ | head -5
  # expect: the pre-wired `uaa iso remaster` variant + todo!() dispatch arm to replace
  ```

## Step-by-step

1. Run the ⛔ START HERE block, then every anchor grep above. Any zero-hit result at both paths → STOP and report.
2. In `crates/uaa-core/src/iso/remaster.rs`, define the options struct and pure patch functions:
   - `pub struct RemasterOptions { pub input: String, pub output: Option<String>, pub seed_dir: PathBuf, pub autoinstall: bool, pub on_done: Option<OnDone> }` with `pub enum OnDone { Poweroff, Reboot, Shell }` (clap `ValueEnum` on the CLI side — invalid values unrepresentable, mirroring the script's hard error).
   - `pub fn resolve_input_dev(input: &str, is_block_device: bool) -> Result<String>` — `stdio:` passthrough; block device → prefix `stdio:`; plain file → as-is; missing → `ConfigError`.
   - `pub fn validate_output_path(out: &str) -> Result<()>` — `stdio:*` or `/dev/*` → `ConfigError` ("refusing to write output to a device").
   - `pub fn default_output_for(input: &str) -> String` — `<input minus .iso>-ssh-ready.iso`.
   - `pub fn patch_kernel_cmdline(cfg: &str) -> (String, bool)` — regex `(linux(efi)?[[:space:]]+/casper/vmlinuz)` → `$1 ds=nocloud\;s=/cdrom/nocloud/ autoinstall=0`; returns `(unchanged, false)` when `ds=nocloud` already present. Use the `regex` crate if it is already a workspace dependency (check `Cargo.toml`); otherwise a hand-rolled line scanner is fine — behavior over mechanism.
   - `pub fn patch_autoinstall_tokens(cfg: &str, on_done: Option<OnDone>) -> (String, bool)` — same insertion point, tokens `uaa.autoinstall` (+ ` uaa.on_done=<poweroff|reboot|shell>`); `(unchanged, false)` when `uaa.autoinstall` already present. Idempotency of the two patches is INDEPENDENT — spell this in a doc comment.
3. Add the orchestrator `pub async fn remaster(executor: &mut dyn CommandExecutor, opts: &RemasterOptions) -> Result<String>` (returns the output path):
   - preflight: xorriso on PATH (executor probe), seed `user-data`+`meta-data` exist, output-path refusal — all BEFORE any extract command (fail-closed: the mock records zero xorriso commands on these paths);
   - extract grub.cfg (hard error if xorriso fails) and loopback.cfg (tolerated failure → `have_loopback=false`) into a tempdir (`tempfile::TempDir`, cleaned on drop — mirrors the script's `trap rm -rf`);
   - apply patch 1 to both cfgs, patch 2 only when `opts.autoinstall`;
   - repack with `-boot_image any replay -compliance no_emul_toc` and the `-map` list (grub.cfg, seed dir → `/nocloud`, loopback iff present) in one executor command.
4. Replace the `todo!()` in the CP-01 pre-wired `iso remaster` CLI module in `crates/uaa/src/` (locate with the execution-time grep above): parse flags/env exactly as the script (`--autoinstall`/`UAA_AUTOINSTALL`, `--on-done`/`UAA_ON_DONE`, `UAA_SEED_DIR`), construct a real `SshClient`-free LOCAL executor the codebase already provides for local runs (find it: `grep -rn "impl CommandExecutor" crates/uaa-core/src/ | head`), call `remaster`, print `DONE: <out>` + the dd hint line the script prints. Touch nothing else in the CLI file.
5. Unit tests in `#[cfg(test)] mod tests` (recording MockExecutor; no real xorriso ever):

   | Test | Asserts |
   |---|---|
   | `test_patch_cmdline_inserts_tokens` | a `linux /casper/vmlinuz ---` and a `linuxefi /casper/vmlinuz ---` line both gain ` ds=nocloud\;s=/cdrom/nocloud/ autoinstall=0` immediately after the vmlinuz path; returns `true` |
   | `test_patch_cmdline_idempotent` | patching its own output returns the identical string and `false` |
   | `test_patch_autoinstall_independent_idempotency` | on an already-`ds=nocloud` cfg, autoinstall patch still adds `uaa.autoinstall`; second application changes nothing; `on_done: Some(Poweroff)` adds `uaa.on_done=poweroff` |
   | `test_semicolon_stays_escaped` | patched output contains the literal `ds=nocloud\;s=` (backslash present) |
   | `test_resolve_input_and_output_guards` | `stdio:/dev/sdc` passthrough; block-device flag → `stdio:` prefix; output `/dev/sdc` → `Err`; output `stdio:x` → `Err` |
   | `test_remaster_fails_closed_before_xorriso` | missing seed file → `Err`, mock recorded 0 commands |
   | `test_remaster_repack_preserves_el_torito` | happy path (mock answers the extract + repack commands): the final recorded command contains `-boot_image any replay`, `-compliance no_emul_toc`, `-map`, and `/nocloud` |
   | `test_remaster_no_loopback_tolerated` | mock fails the loopback extract only → `Ok`, and the repack command contains no `loopback.cfg` map |

6. Bump the file header (version + last-edited) on every file you touch; new files get a fresh 4-line header with a new uuid4 (`uuidgen | tr 'A-F' 'a-f'`).

## How to test

```bash
cargo test --lib --offline && cargo build --offline
# Expected: all tests pass (baseline 311 + your new tests), build clean
cargo clippy --offline -- -D warnings
# Expected: no warnings
cargo test --lib --offline remaster
# Expected: the 8 tests above all pass
grep -rn "std::process::Command" crates/uaa-core/src/iso/remaster.rs
# Expected: 0 hits (executor-only)
git diff origin/main -- scripts/make-ssh-ready-iso.sh
# Expected: empty (script untouched — TASK-05 owns its deletion)
```

## Acceptance criteria

- [ ] `grep -n "pub async fn remaster" crates/uaa-core/src/iso/remaster.rs` → 1 hit; `grep -n "todo!" crates/uaa-core/src/iso/remaster.rs` → 0 hits; the `iso remaster` CLI arm no longer contains `todo!` (`grep -rn "remaster" crates/uaa/src/ | grep todo!` → 0 hits).
- [ ] El Torito preserved: `grep -n "boot_image any replay" crates/uaa-core/src/iso/remaster.rs` → ≥1 hit and `test_remaster_repack_preserves_el_torito` passes.
- [ ] Both idempotency guards ported: `test_patch_cmdline_idempotent` and `test_patch_autoinstall_independent_idempotency` pass.
- [ ] Device-output refusal + block-device input ported: `test_resolve_input_and_output_guards` passes.
- [ ] **Anti-over-suppression:** `test_remaster_repack_preserves_el_torito` (happy path through all preflight guards produces the full repack command) passes.
- [ ] `scripts/make-ssh-ready-iso.sh` unchanged: `git diff origin/main -- scripts/make-ssh-ready-iso.sh` empty.
- [ ] `cargo test --lib --offline && cargo build --offline` exits 0; clippy clean.
- [ ] File headers bumped on every changed file (`git diff origin/main --stat` files each show a version bump; guids unchanged).

## Commit message

```
feat(iso): port make-ssh-ready-iso.sh to uaa iso remaster (ws9-tooling)

Fills the CP-01 stub crates/uaa-core/src/iso/remaster.rs with the xorriso
extract/patch/repack pipeline: in-memory GRUB cmdline patching (ds=nocloud
NoCloud seed, escaped semicolon, independent opt-in uaa.autoinstall /
uaa.on_done tokens, both idempotent), block-device stdio: input handling,
device-output refusal, and El Torito-preserving repack (-boot_image any
replay -compliance no_emul_toc). All xorriso calls via CommandExecutor;
8 unit tests against a recording mock. Shell script untouched (retired in
TASK-05 after the M6 gate).

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

If `grep -n "pub async fn remaster" crates/uaa-core/src/iso/remaster.rs` hits (and `grep -n "todo!"` in the same file shows 0), already applied — run the acceptance checks instead of re-applying. Rollback = revert the single commit; `scripts/make-ssh-ready-iso.sh`, the seed dir, all golden fixtures, and every other crate stay untouched (the stub file returns to its CP-01 `todo!()` form).
