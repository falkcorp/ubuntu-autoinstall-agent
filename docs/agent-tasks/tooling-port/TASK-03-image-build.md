<!-- file: docs/agent-tasks/tooling-port/TASK-03-image-build.md -->
<!-- version: 1.0.0 -->
<!-- guid: c6986bb7-91ef-423b-913d-3e8d017cee35 -->
<!-- last-edited: 2026-07-10 -->

# TASK-03 — `uaa image build`: port build-installer-image.sh (unsquashfs overlay, agent install, subiquity mask, mksquashfs zstd) (ws9-tooling)

**Priority:** P2 · **Effort:** M · **Recommended subagent:** Sonnet-class · rust-port subagent · **Why:** squashfs pipeline port with VERIFY-ON-VM markers preserved · **Depends on:** TASK-01 (wave-3 gated: TP-01 MERGED — both tasks live under `crates/uaa-core/src/iso/` and share `iso/mod.rs`; rebase before starting)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/tooling-port-image-build" -b agent/tooling-port-image-build origin/main
cd "$REPO/.worktrees/tooling-port-image-build"
git rebase origin/main
```

(Protocol also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

## Goal

Fill the CP-01-created stub `crates/uaa-core/src/iso/image_build.rs` with a faithful Rust port of `scripts/build-installer-image.sh` (v1.0.0, 97 lines) — overlay the Ubuntu live-server squashfs with the static `uaa` agent + boot automation, mask the stock installer autostart, and repack with `mksquashfs -comp zstd` — and replace the `todo!()` of the pre-wired `uaa image build` CLI dispatch (CP-01, `crates/uaa/src/`). The two **VERIFY-ON-VM** markers in the script are load-bearing documentation consumed by `scripts/vm-validate.sh` stage 3 (which greps them by file:line topic) — they MUST be preserved verbatim as doc comments AND as runtime `WARN`-level log lines. Shell script stays UNCHANGED — TASK-05 deletes it after the M6 gate (spec Goals; Decision 17 puts iso tooling in `crates/uaa-core`; uaa-web's ISO jobs shell out to THIS pipeline per spec C4 — "tooling-port pipeline, never inline"). Purely additive.

REUSE — do not invent parallels:

- **`CommandExecutor`** trait (pre-move: `src/network/executor.rs` — verify: `grep -n "pub trait CommandExecutor" src/network/executor.rs`; post-CP-01: `crates/uaa-core/src/network/executor.rs`). EVERY external command (`unsquashfs`, `mksquashfs`, `install`, `ln`, `mkdir`, `id -u`, `du`) goes through it so tests mock them. NO `std::process::Command` in this change.
- **Mock idiom:** mirror `MockExecutor` (`src/autoinstall/verify.rs` — verify: `grep -n "struct MockExecutor" src/autoinstall/verify.rs`) + a recorded-commands `Vec<String>`. No mocking crate.
- **`AutoInstallError::ConfigError` / `AutoInstallError::SystemError`** (`src/error.rs`). No new error enum.

## Background (verify before editing)

Ground truth is `scripts/build-installer-image.sh`. Port ALL of it:

- Inputs (all required): `--src-squashfs <path>` (must exist), `--agent <musl uaa binary>` (must exist), `--out <path>`. Overlay assets come from `installer-image/` (repo-relative): `uaa-autoinstall.sh`, `uaa-autoinstall.service`.
- Preflight: must run as root (`id -u` == 0 via the executor); `unsquashfs` must be on PATH. All preflight failures are fail-closed BEFORE any unpack command.
- Pipeline (each step one executor command, work dir = `tempfile::TempDir` cleaned on drop):
  1. `unsquashfs -d <work>/squashfs-root <src-squashfs>`;
  2. inject: `install -m 0755 <agent> <root>/usr/local/bin/uaa`; `install -m 0755 installer-image/uaa-autoinstall.sh <root>/usr/local/bin/uaa-autoinstall.sh`; `install -m 0644 installer-image/uaa-autoinstall.service <root>/etc/systemd/system/uaa-autoinstall.service`;
  3. enable: `mkdir -p <root>/etc/systemd/system/multi-user.target.wants` + `ln -sf ../uaa-autoinstall.service <root>/etc/systemd/system/multi-user.target.wants/uaa-autoinstall.service`;
  4. **VERIFY-ON-VM marker 1** (stock-installer autostart unit name on 26.04 live-server): mask ALL THREE candidate units — `subiquity-server.service`, `serial-subiquity@.service`, `snap.subiquity.subiquity-server.service` — via `ln -sf /dev/null <root>/etc/systemd/system/<unit>`; masking an absent unit is a tolerated no-op (`|| true` semantics: a failed `ln` here is logged, not fatal);
  5. **VERIFY-ON-VM marker 2** (live-rootfs install tools): for each of `debootstrap sgdisk zpool cryptsetup dracut clevis`, check existence at `<root>/usr/sbin/`, `<root>/sbin/`, `<root>/usr/bin/`; a missing tool emits `WARN: '<tool>' not found in live rootfs — bake it into the overlay` (WARN only, never fatal — flag loudly rather than silently ship, but the decision belongs to the VM gate);
  6. `rm -f <out>` then `mksquashfs <root> <out> -comp zstd -no-progress`; report the output size (`du -h`).
- The tool list and the three unit names are shared vocabulary with `scripts/vm-validate.sh` stage 3 and its `==== VERIFY-ON-VM REPORT ====` — keep them EXACTLY as spelled (TP-04 asserts the same strings).

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
`crates/uaa-core/src/**` and the CLI at `crates/uaa/src/**`. The greps above cite
pre-move paths (verifiable on today's main); at execution time run them at the old
path, then the mapped path. Zero hits at BOTH = STOP and report.

- **Re-verify these anchors before editing** — line numbers drift; zero hits at both
  old and mapped path = STOP and report:
  ```bash
  grep -n "VERIFY-ON-VM\|subiquity" scripts/build-installer-image.sh | head -5
  # expect: hits (marker 1 ~lines 72-77, marker 2 ~line 81)
  grep -n "mksquashfs\|zstd" scripts/build-installer-image.sh
  # expect: hits (~line 95, -comp zstd -no-progress)
  grep -n "debootstrap sgdisk zpool cryptsetup dracut clevis" scripts/build-installer-image.sh
  # expect: 1 hit (the tool-check list, ~line 86)
  ls installer-image/uaa-autoinstall.sh installer-image/uaa-autoinstall.service
  # expect: both listed (overlay assets)
  grep -n "pub trait CommandExecutor" src/network/executor.rs
  # expect: 1 hit (then at crates/uaa-core/src/network/executor.rs)
  ```
- Execution-time checks (crates/ exists only after CP-01; TP-01 merged means iso/remaster.rs is already filled):
  ```bash
  test -f crates/uaa-core/src/iso/image_build.rs && grep -n "todo!" crates/uaa-core/src/iso/image_build.rs
  # expect: headered stub exists; absent = STOP, CP-01 not merged
  grep -n "todo!" crates/uaa-core/src/iso/remaster.rs
  # expect: 0 hits — TP-01 merged; if it still shows todo!, STOP (wave gate not satisfied)
  grep -rn "image" crates/uaa/src/ | grep -i "build\|todo" | head -5
  # expect: the pre-wired `uaa image build` variant + todo!() arm to replace
  ```

## Step-by-step

1. Run the ⛔ START HERE block, then every anchor grep above. Any zero-hit result at both paths → STOP and report.
2. In `crates/uaa-core/src/iso/image_build.rs` define `pub struct ImageBuildOptions { pub src_squashfs: PathBuf, pub agent_bin: PathBuf, pub out: PathBuf, pub overlay_dir: PathBuf /* default installer-image/ */ }` and the constants the VM gate greps for:
   ```rust
   /// VERIFY-ON-VM: the exact stock-installer autostart unit on 26.04 live-server
   /// is unconfirmed; all three candidates are masked. vm-validate stage 3
   /// resolves this marker.
   pub const MASK_UNITS: [&str; 3] = ["subiquity-server.service", "serial-subiquity@.service", "snap.subiquity.subiquity-server.service"];
   /// VERIFY-ON-VM: these must exist in the live rootfs or be baked in.
   pub const REQUIRED_LIVE_TOOLS: [&str; 6] = ["debootstrap", "sgdisk", "zpool", "cryptsetup", "dracut", "clevis"];
   ```
3. Add `pub async fn image_build(executor: &mut dyn CommandExecutor, opts: &ImageBuildOptions) -> Result<ImageBuildReport>` implementing steps 1–6 from Background in order. `ImageBuildReport { out: PathBuf, missing_tools: Vec<String>, masked_units: Vec<String> }`. Preflight (root check via `id -u`, `unsquashfs` on PATH, all three input paths + both overlay assets exist) BEFORE any pipeline command — the mock records zero pipeline commands on preflight failure. Missing tools: WARN log with the exact script wording, collected into `missing_tools`, NEVER an `Err`.
4. Emit the two VERIFY-ON-VM markers as `tracing::warn!`/log lines during the mask and tool-check steps (containing the literal string `VERIFY-ON-VM`) so operator logs carry the same flags the script printed.
5. Replace the `todo!()` in the pre-wired `uaa image build` CLI module in `crates/uaa/src/`: flags `--src-squashfs`, `--agent`, `--out` (all required), wire a local executor (same one TP-01 used — `grep -rn "impl CommandExecutor" crates/uaa-core/src/ | head`), print the script's `==>` progress lines and the final `Done: <out>` size line.
6. Unit tests (recording MockExecutor; no real squashfs-tools ever):

   | Test | Asserts |
   |---|---|
   | `test_preflight_fails_closed` | non-root `id -u` mock (`"1000"`) → `Err`, zero pipeline commands recorded; missing `--agent` file → same |
   | `test_pipeline_command_order` | happy path: recorded commands appear in order unsquashfs → 3×install → mkdir/ln enable → 3×mask ln → tool checks → `rm -f` → mksquashfs |
   | `test_masks_all_three_units` | each `MASK_UNITS` name appears in exactly one recorded `ln -sf /dev/null` command; a failing mask `ln` does NOT abort the build |
   | `test_mksquashfs_zstd` | final command contains `mksquashfs`, `-comp zstd`, `-no-progress`, and the `--out` path |
   | `test_missing_tool_warns_not_fails` | mock reports `debootstrap` absent at all three prefixes → `Ok`, `missing_tools == ["debootstrap"]` |
   | `test_verify_on_vm_markers_present` | `MASK_UNITS`/`REQUIRED_LIVE_TOOLS` doc text and the emitted log strings contain the literal `VERIFY-ON-VM` |
   | `test_agent_installed_0755` | **anti-over-suppression:** happy path records `install -m 0755 <agent> .../usr/local/bin/uaa` and the build reaches mksquashfs (preflight guards do not block a valid build) |

7. Bump the file header (version + last-edited) on every file you touch; new files get a fresh 4-line header with a new uuid4 (`uuidgen | tr 'A-F' 'a-f'`).

## How to test

```bash
cargo test --lib --offline && cargo build --offline
# Expected: all tests pass (baseline 311 + your new tests), build clean
cargo clippy --offline -- -D warnings
# Expected: no warnings
cargo test --lib --offline image_build
# Expected: the 7 tests above all pass
grep -rn "std::process::Command" crates/uaa-core/src/iso/image_build.rs
# Expected: 0 hits (executor-only)
grep -c "VERIFY-ON-VM" crates/uaa-core/src/iso/image_build.rs
# Expected: >= 2 (both markers preserved)
git diff origin/main -- scripts/build-installer-image.sh
# Expected: empty (script untouched — TASK-05 owns its deletion)
```

## Acceptance criteria

- [ ] Port complete: `grep -n "pub async fn image_build" crates/uaa-core/src/iso/image_build.rs` → 1 hit; `grep -n "todo!" crates/uaa-core/src/iso/image_build.rs` → 0 hits; the `image build` CLI arm no longer contains `todo!`.
- [ ] Markers preserved verbatim: `grep -c "VERIFY-ON-VM" crates/uaa-core/src/iso/image_build.rs` ≥ 2; `grep -n "subiquity-server.service" crates/uaa-core/src/iso/image_build.rs` → ≥1 hit; all six tool names present (`grep -n "debootstrap" crates/uaa-core/src/iso/image_build.rs` → ≥1 hit).
- [ ] zstd repack ported: `test_mksquashfs_zstd` passes.
- [ ] Missing tools warn-not-fail: `test_missing_tool_warns_not_fails` passes; mask of absent unit tolerated: `test_masks_all_three_units` passes.
- [ ] **Anti-over-suppression:** `test_agent_installed_0755` passes — a valid build goes through every preflight guard to the final mksquashfs.
- [ ] `scripts/build-installer-image.sh` unchanged: `git diff origin/main -- scripts/build-installer-image.sh` empty.
- [ ] `cargo test --lib --offline && cargo build --offline` exits 0; clippy clean.
- [ ] File headers bumped on every changed file (`git diff origin/main --stat` files each show a version bump; guids unchanged).

## Commit message

```
feat(iso): port build-installer-image.sh to uaa image build (ws9-tooling)

Fills crates/uaa-core/src/iso/image_build.rs: unsquashfs overlay of the
live-server squashfs, static agent + uaa-autoinstall.{sh,service} injection,
multi-user.target.wants enable, masking of all three subiquity autostart
candidates, live-rootfs tool check (warn-not-fail), mksquashfs -comp zstd
repack. Both VERIFY-ON-VM markers preserved as constants + doc comments +
WARN logs so vm-validate stage 3 keeps resolving them. All commands via
CommandExecutor; 7 unit tests against a recording mock. Shell script
untouched (retired in TASK-05 after the M6 gate).

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

If `grep -n "pub async fn image_build" crates/uaa-core/src/iso/image_build.rs` hits (and `grep -n "todo!"` in the same file shows 0), already applied — run the acceptance checks instead of re-applying. Rollback = revert the single commit; `scripts/build-installer-image.sh`, `installer-image/*`, `iso/remaster.rs` (TP-01's work), and all golden fixtures stay untouched (the stub returns to its CP-01 `todo!()` form).
