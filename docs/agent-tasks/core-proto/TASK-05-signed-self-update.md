<!-- file: docs/agent-tasks/core-proto/TASK-05-signed-self-update.md -->
<!-- version: 1.0.0 -->
<!-- guid: 98cb8825-4939-4c2c-9c41-498149c3d8b6 -->
<!-- last-edited: 2026-07-10 -->

# TASK-05 — update.rs: manifest model, dual-pubkey ed25519 verify, min_version floor, stage/apply modes, hold pin, prev-swap rollback (ws1-core)

**Priority:** P1 · **Effort:** M · **Recommended subagent:** Sonnet-class · rust-security subagent · **Why:** security-critical verify ordering (fail-closed at every step per spec C7) · **Depends on:** TASK-02 (wave-3 gated: CP-02 MERGED — `ed25519-dalek` and `semver` must exist in `[workspace.dependencies]`)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/core-proto-signed-self-update" -b agent/core-proto-signed-self-update origin/main
cd "$REPO/.worktrees/core-proto-signed-self-update"
git rebase origin/main
```

(Protocol also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

## Goal

Fill the `crates/uaa-core/src/update.rs` stub with the signed self-update library per spec C7 and Decisions 9/10 (`docs/specs/constellation-design.md`): `Manifest { binaries: Vec<BinaryEntry>, min_version: semver::Version }`, `BinaryEntry { name, version, target, sha256, sig, url }` (same shape as `proto/uaa/update/v1/update.proto` — the manifest travels as JSON with a detached `.sig`), verification against **two** embedded ed25519 pubkeys (`&[VerifyingKey; 2]`, current + next — Decision 10 rotation repair), a `min_version` floor (a replayed old-but-signed manifest can NEVER downgrade — Decision 9c), `ApplyMode::TimerAuto` (fleet agents/CLI) vs `ApplyMode::StageOnly` (the three server daemons check-and-STAGE; apply only on explicit operator `--apply` — Decision 9a), a hold pin (marker file suppresses TimerAuto), and prev-swap rollback (`<bin>.prev` kept beside the binary). Verify order is LOCKED and fail-closed at EVERY step (spec C7): manifest sig → version newer AND ≥ min_version → download → sha256 → artifact sig → write `<bin>.new` → atomic rename (current → `.prev`, `.new` → current) → restart is the CALLER's job. Reuse `sha2` (already a dep) for sha256 and `ed25519-dalek` for signatures; reuse `AutoInstallError::{ConfigError, ValidationError, NetworkError}` — no new error enum. Purely additive: only update.rs.

## Background (verify before editing)

- Library only — NO timer, NO systemd unit, NO live HTTP in tests. `self_update()` takes a fetcher seam so unit tests inject bytes: `async fn self_update(current: &BinaryIdentity, manifest_url: &str, pubkeys: &[VerifyingKey; 2], mode: ApplyMode, fetch: &dyn Fetcher) -> Result<UpdateOutcome>` where `Fetcher` is a tiny `#[async_trait]` (`async fn fetch(&self, url: &str) -> Result<Vec<u8>>`) — mirror the `CommandExecutor` mock idiom (verify: `grep -n "pub trait CommandExecutor" crates/uaa-core/src/network/executor.rs`); do NOT add a mocking crate.
- `BinaryIdentity { name: String, version: semver::Version, target: String, install_path: PathBuf }`. `UpdateOutcome`: `UpToDate`, `Held`, `Staged(PathBuf)`, `Applied { new_version, prev: PathBuf }`.
- Dual-pubkey rule: a signature is valid if it verifies under EITHER slot (this is how rotation stages through the channel before the old key retires). BOTH failing = hard error. Signature bytes travel hex- or base64-encoded in JSON — pick base64, document it on the field.
- min_version floor semantics (spell twice — here and in tests): candidate entry must satisfy `entry.version > current.version` AND `entry.version >= manifest.min_version` AND `current.version >= manifest.min_version` is NOT required (an old binary below the floor must still be allowed to climb); a manifest whose best entry is `<= current` → `UpToDate` (not an error); a manifest offering a version LOWER than current → `UpToDate` (downgrade unrepresentable — rollback is manifest-revert + prev-swap, never a forward "update" to an older version).
- Hold pin: `install_path.with_extension("hold")` exists → `TimerAuto` returns `Held` BEFORE any fetch (zero fetcher calls, assert in test); `StageOnly` ignores the hold (staging is harmless; apply is manual anyway).
- Atomicity: stage writes `<install_path>.new` (0755) + fsync; apply renames current → `<install_path>.prev` then `.new` → current (`std::fs::rename`, same filesystem — atomic). A crash between the two renames leaves `.prev` + `.new` present and current missing — document recovery (re-run apply: it detects orphan `.new` and completes) in the module docs.
- No secret material anywhere: tests generate throwaway keypairs (`ed25519_dalek::SigningKey::generate`) — the real update PRIVATE key lives offline in the operator password manager (Decision 10), never in this repo.

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

**Path map:** after CP-01 (wave 1) merges, `src/**` lives at `crates/uaa-core/src/**` and the CLI at `crates/uaa/src/**`. The greps below cite pre-move paths (verifiable on today's main); at execution time run them at the old path, then the mapped path. Zero hits at BOTH = STOP and report.

- **Re-verify these anchors before editing** — line numbers drift; zero hits at both old and mapped path = STOP and report:
  ```bash
  grep -n "^ring\|^sha2" Cargo.toml   # expect: 2 hits (sha256 available; after CP-01 the same two lines live in [workspace.dependencies] of the ROOT Cargo.toml — grep the root either way)
  grep -n "ed25519-dalek" Cargo.toml  # expect: 1 hit (CP-02 added it; 0 hits = CP-02 not merged, STOP)
  grep -n "//! .*update\|filled exclusively" crates/uaa-core/src/update.rs   # expect: 1+ hits (the CP-01 stub you fill; no old path exists)
  grep -n "min_version" docs/specs/constellation-design.md   # expect: 2+ hits (the floor is normative)
  ```

## Step-by-step

1. Run the ⛔ START HERE block, then the anchor greps.
2. Fill `crates/uaa-core/src/update.rs` (keep the CP-01 header, bump to 1.1.0) with the types from Background plus serde derives on `Manifest`/`BinaryEntry` (JSON; `version`/`min_version` via `semver` serde feature or string + parse — string+parse is fine, fail-closed `ValidationError` on garbage).
3. Implement the verification pipeline as SMALL PURE FUNCTIONS in this exact order, each fail-closed (`Err` stops everything; no step may be skipped or reordered):
   1. `verify_manifest_sig(manifest_bytes, sig, pubkeys) -> Result<Manifest>` — base64-decode sig, try slot 0 then slot 1, both fail → `ValidationError("manifest signature invalid under both update keys")`; only THEN parse JSON.
   2. `select_entry(manifest, current) -> Result<Option<&BinaryEntry>>` — match `name` AND `target`; apply the min_version floor + newer-only rules from Background; `Ok(None)` = up to date.
   3. `verify_artifact(bytes, entry, pubkeys) -> Result<()>` — sha256 hex compare FIRST (`sha2`), then artifact sig (same dual-slot rule). Mismatch names which check failed, never writes anything.
   4. `stage(bytes, install_path) -> Result<PathBuf>` — write `<path>.new`, 0755, fsync.
   5. `apply(install_path) -> Result<PathBuf>` — the two renames from Background; returns the `.prev` path; also completes an orphaned `.new` (crash recovery).
4. Compose `pub async fn self_update(...)` per Background: hold-check (TimerAuto only, BEFORE any fetch) → fetch `manifest_url` and `manifest_url + ".sig"` → steps 1–4 → `StageOnly` stops at `Staged`, `TimerAuto` continues to `apply` → `Applied`.
5. `#[cfg(test)]` tests with a recording `MockFetcher` (HashMap url→bytes + `Vec<String>` of fetched urls) and throwaway keypairs; helper `fn signed_manifest(entries, min_version, key) -> (Vec<u8>, Vec<u8>)`:
   | Test | Asserts |
   |---|---|
   | `test_bad_manifest_sig_rejected` | tampered manifest bytes → `Err`; fetcher recorded ONLY the 2 manifest fetches (no artifact download after a failed sig) |
   | `test_second_slot_key_accepted` | manifest signed by slot-1 ("next") key verifies (rotation path) |
   | `test_older_or_equal_version_is_uptodate` | entry version == current and < current → `Ok(UpToDate)`, zero artifact fetches |
   | `test_min_version_floor_blocks_replay` | current 1.2.0, replayed manifest offering 1.1.0 with min_version 1.0.0 → `UpToDate` (downgrade unrepresentable); and a manifest with min_version 2.0.0 offering 1.3.0 → `Err`/no-apply (entry below its own floor) |
   | `test_sha256_mismatch_rejected` | corrupt artifact bytes → `Err` naming sha256; `<bin>.new` NOT created |
   | `test_bad_artifact_sig_rejected` | good sha, wrong-key artifact sig → `Err`; `<bin>.new` NOT created |
   | `test_hold_pin_suppresses_timer` | `.hold` file present + TimerAuto → `Held`, fetcher recorded ZERO calls; StageOnly with same hold still stages |
   | `test_stage_only_never_renames` | StageOnly happy path → `Staged`, `<bin>.new` exists, original binary bytes UNCHANGED |
   | `test_timer_auto_applies_with_prev` | **anti-over-suppression / happy path:** fully valid manifest+artifact (slot-0 key), TimerAuto, tempdir binary → `Applied`; current file has the new bytes, `.prev` has the old bytes, `.new` is gone (the guard stack does not block a legitimate update) |
6. Bump the file header (version + last-edited) on every file you touch; new files get a fresh 4-line header with a new uuid4 (`uuidgen | tr 'A-F' 'a-f'`).

## How to test

```bash
cargo test --lib --offline && cargo build --offline
# Expected: all tests pass (baseline 311 + prior-wave additions + your 9 update tests), build clean
cargo clippy --offline -- -D warnings
# Expected: no warnings
cargo test --lib --offline -p uaa-core update
# Expected: 9 passed; 0 failed (all against MockFetcher + tempdirs, no network)
grep -rn "reqwest\|hyper" crates/uaa-core/src/update.rs
# Expected: 0 hits (fetching goes through the Fetcher seam only)
```

## Acceptance criteria

- [ ] Types + pipeline present: `grep -n "pub struct Manifest\|pub struct BinaryEntry\|enum ApplyMode\|pub async fn self_update" crates/uaa-core/src/update.rs` → 4 hits; verify order encoded as the five named fns (`grep -c "fn verify_manifest_sig\|fn select_entry\|fn verify_artifact\|fn stage\|fn apply" crates/uaa-core/src/update.rs` → 5).
- [ ] Fail-closed ordering proven: `test_bad_manifest_sig_rejected` asserts no artifact fetch after a bad sig; `test_sha256_mismatch_rejected` and `test_bad_artifact_sig_rejected` assert `<bin>.new` absent.
- [ ] Dual-pubkey rotation proven: `test_second_slot_key_accepted` green; downgrade unrepresentable: `test_min_version_floor_blocks_replay` green.
- [ ] Mode split proven: `test_stage_only_never_renames` and `test_hold_pin_suppresses_timer` green (hold checked before ANY fetch — zero recorded fetcher calls).
- [ ] **Anti-over-suppression:** `test_timer_auto_applies_with_prev` passes — a fully valid update sails through every guard and lands atomically with `.prev` rollback material in place.
- [ ] No key material committed: `grep -rn "PRIVATE KEY\|SigningKey::from" crates/uaa-core/src/update.rs | grep -v "cfg(test)\|mod tests\|generate"` → 0 hits.
- [ ] `cargo test --lib --offline && cargo build --offline` exits 0; clippy clean.
- [ ] File headers bumped on every changed file (`git diff origin/main --stat` files each show a version bump; guids unchanged).

## Commit message

```
feat(update): signed self-update library — dual-key ed25519, min_version floor, stage/apply, hold, prev-swap (ws1-core)

Fills the CP-01 update.rs stub per spec C7 + Decisions 9/10: manifest/entry
model (JSON + detached .sig), verification pipeline fail-closed at every step
(manifest sig -> newer-and->=min_version -> download -> sha256 -> artifact sig
-> .new -> atomic rename), signatures valid under either of two embedded
pubkeys (rotation), min_version floor makes replayed-manifest downgrades
unrepresentable, ApplyMode::TimerAuto (agents/CLI) vs StageOnly (daemons,
operator --apply), .hold pin suppresses the timer before any fetch, .prev kept
for rollback. Fetching behind a mockable Fetcher seam; 9 tests with throwaway
keypairs incl. the full happy-path apply.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

Additive polarity: if `grep -n "pub async fn self_update" crates/uaa-core/src/update.rs` hits, the task is already applied — run the Acceptance criteria checks instead of re-applying. Rollback = revert the single commit; update.rs returns to the CP-01 stub — nothing calls it yet (WB-04 serves manifests and the daemons wire timers in later waves), no binary on any host is touched by merging library code.
