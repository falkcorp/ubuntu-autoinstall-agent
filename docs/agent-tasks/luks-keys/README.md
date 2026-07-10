<!-- file: docs/agent-tasks/luks-keys/README.md -->
<!-- version: 1.0.0 -->
<!-- guid: 2938392c-54b1-47af-8a57-4d3bf2a0b703 -->
<!-- last-edited: 2026-07-10 -->

# Workstream — LUKS key management (`uaa luks` + registry sync)

Build the FIDO2+PIN LUKS keyslot manager from spec Decision 14 / component C8 ([docs/specs/constellation-design.md](../../specs/constellation-design.md)): enroll/status (LK-01), rotate/revoke/rotate-tang with the fleet-aware Tang t=2-of-3 cold-start guard (LK-02), and the client module that reports enrolled credentials to uaa-control's `luks_credentials` table (LK-03). The 3-credential-per-host model (primary micro + 2 removable backups) is `PLAN-zfs-luks-multikey.md`'s. **YubiKeys are for LUKS disk unlock, NOT auth** — nothing in this workstream authenticates anything. From ws7-luks.

| Task | Src id | Title | Priority | Effort | Tier | Wave |
|------|--------|-------|----------|--------|------|------|
| TASK-01 | ws7-luks | `uaa luks` enroll (systemd-cryptenroll FIDO2+PIN wrapper) + status (luksDump fido2 token parse) | P1 | M | Sonnet-class | 2 |
| TASK-02 | ws7-luks | rotate/revoke/rotate-tang with fleet-aware Tang t=2-of-3 cold-start guard (fail-closed, typed-hostname override) | P1 | M | Opus-class | 3 |
| TASK-03 | ws7-luks | Report enrolled FIDO2 credentials to control (luks_credentials table) — client sync module | P2 | S | Haiku-class | 5 |

Waves are GLOBAL across the constellation plan (skeleton `.global_waves`): TASK-01 runs in wave 2 (after CP-01's workspace conversion creates the `luks_keys.rs`/`luks_sync.rs` stubs), TASK-02 in wave 3 (serialized behind TASK-01 — same file), TASK-03 in wave 5 (after CT-02 lands the `luks_credentials` endpoints).

## Ground rules

- Rust only, in exactly the files each brief names: `crates/uaa-core/src/luks_keys.rs` (TASK-01 fills the stub, TASK-02 extends it) and `crates/uaa-core/src/luks_sync.rs` (TASK-03). Purely additive — no existing function's signature changes.
- Build + test gate for every task in this workstream:
  ```bash
  cargo test --lib --offline && cargo build --offline
  # Expected: all pass (baseline 311 + this workstream's new tests), build clean
  cargo clippy --offline -- -D warnings
  # Expected: no warnings
  ```
- **Verify every file:line anchor with `grep` before editing** — these tasks run in waves 2–5, after the CP-01 workspace transform, so every `src/**` path in a brief maps to `crates/uaa-core/src/**`; the grep hits are authoritative, line numbers are not. Zero hits at both old and mapped path = STOP and report.
- File headers MANDATORY: stub files keep their CP-01 guid with a version bump; genuinely new files get a fresh 4-line header with a new uuid4.
- HARD RULES (operation contract, restated in every brief): NO hardware actions — all `systemd-cryptenroll`/`cryptsetup`/`clevis`/`curl` calls go through `CommandExecutor` mocks; NEVER wipe/write/deploy on 172.16.2.30 or len-serv-003; `disk_device` read from the live target, never guessed; no real secret anywhere (`REPLACE_AT_PLACE_TIME` stays a placeholder; test passphrases are obviously fake); workers stay in their worktree and never push/PR/merge.
- Safety properties owned here (do not "simplify" them away): enroll-new-THEN-revoke-old ordering; Tang t=2-of-3 quorum guard fail-closed with typed-exact-hostname override only; rotate-tang re-bind sweep completes on EVERY fleet host before old-key retirement (encoded as a state machine, not prose); luksDump parsing reuses `evaluate_fido2_keyslot`.

## Collision / wave note

From the skeleton collision matrix — `crates/uaa-core/src/luks_keys.rs` is shared three ways:

| Shared file | Colliding tasks | Resolution |
|---|---|---|
| `crates/uaa-core/src/luks_keys.rs` | CP-01 (stub), LK-01, LK-02 | serialize: wave1=CP-01 (stub), wave2=LK-01, wave3=LK-02 |

`crates/uaa-core/src/luks_sync.rs` has exactly one filler (LK-03) after its CP-01 stub — no intra-workstream collision; LK-03's gate is the logical dependency on CT-02 (endpoints) + LK-01 (state-file contract).

Execution mode: SERIAL WAVES — LK-01→LK-02 share luks_keys.rs (collision row); LK-03 parallel-safe after CT-02 — trigger: 3 tasks with a 3-way shared-file collision row on `luks_keys.rs` (below the ≥3-parallel threshold per wave; local waves are `[LK-01]` then `[LK-02, LK-03]`, with LK-03 additionally held for its global wave-5 prereq CT-02).

Link: See [ORCHESTRATION.md](../ORCHESTRATION.md) for the coordinator + worker protocol.
