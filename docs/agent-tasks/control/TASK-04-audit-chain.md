<!-- file: docs/agent-tasks/control/TASK-04-audit-chain.md -->
<!-- version: 1.0.0 -->
<!-- guid: 58993045-2c12-4995-93f5-18fb95fba2c3 -->
<!-- last-edited: 2026-07-10 -->

# TASK-04 — Fill audit.rs: hash-chained audit log with FOR-UPDATE-serialized append, zero genesis, daily signed checkpoints, backfill cmd (ws2-control)

**Priority:** P1 · **Effort:** M · **Recommended subagent:** Sonnet-class · rust-service subagent · **Why:** concurrency-correctness — the chain must never fork (spec Decision 21 repair); a forked chain is a silently worthless audit log. · **Depends on:** TASK-01 (wave-4 gated: CT-01 merged — the `audit.rs` stub must exist on origin/main)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/control-audit-chain" -b agent/control-audit-chain origin/main
cd "$REPO/.worktrees/control-audit-chain"
git rebase origin/main
```

(Protocol also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

## Goal

Fill the CT-01 stub `crates/uaa-control/src/audit.rs` (your EXCLUSIVE file) with the spec Decision-21 audit chain: `audit::record(...)` appending a hash-chained event **inside the same transaction as the mutation it records**, with the `prev_hash` read under `SELECT ... FOR UPDATE` on the chain tip (concurrent handlers must never fork the chain), genesis `prev_hash` = 32 zero bytes, a `verify_chain` walker, a daily ed25519-signed checkpoint row, and the `uaa-control audit backfill` CLI command (the Decision-8 emergency-hatch companion: records an out-of-band `cockroach sql` mutation after the fact).

Purely additive to `audit.rs` (+ wiring the `audit` clap arm in `main.rs`). Reuse — do not invent parallels:
- **`sha2`** (workspace dep — verify: `grep -n "^sha2" Cargo.toml`) for event hashes; **`ed25519-dalek`** (workspace dep via CP-02 — verify: `grep -n "ed25519-dalek" Cargo.toml`) for checkpoint signatures. Do NOT add crypto crates.
- **`AuditEventRow` / `AuditCheckpointRow`** from CT-01's `db/mod.rs` (verify: `grep -n "AuditEventRow" crates/uaa-control/src/db/mod.rs` at execution time). Do NOT redefine.
- The threat model is STATED, not extended: the chain + on-server audit key defends against a rogue operator WITHOUT server root; a server-root adversary defeats it; out-of-band checkpoint witnessing is recorded P2 hardening, NOT built here (spec Decision 21b).

## Background (verify before editing)

- Spec: Decision 21 (+ repairs a/b), the `audit_events`/`audit_checkpoints` schema (already embedded verbatim by CT-01 — do not touch the migration file), C3 "Audit" paragraph, Decision 8 repair (backfill).
- Hash definition (make it a documented, tested constant of the module): `hash = SHA-256(prev_hash || canonical_bytes(event))` where `canonical_bytes` = the JSON serialization of `(at, actor, role, action, target, outcome, detail)` with sorted keys — deterministic, re-computable by `verify_chain`. Genesis: the FIRST event's `prev_hash` is exactly `[0u8; 32]`.
- Serialization rule (spell twice — here and Step 3): the append happens in the SAME CRDB txn as the mutation it records, and `prev_hash` comes from `SELECT hash FROM audit_events ORDER BY seq DESC LIMIT 1 FOR UPDATE` (tip lock) — two concurrent mutations serialize on that lock; the store trait must make this shape unavoidable, and the in-memory test double must emulate the lock with a Mutex held across read-tip→insert.
- **Cargo tests must NOT require live CockroachDB**: define `pub trait AuditStore: Send + Sync { async fn append_in_txn(&self, mutation: …, event: NewAuditEvent) -> Result<AuditEventRow>; async fn list_events(&self, from_seq: i64) -> Result<Vec<AuditEventRow>>; async fn tip(&self) -> Result<Option<(i64, Vec<u8>)>>; async fn insert_checkpoint(&self, AuditCheckpointRow) -> Result<()>; }` with `PgAuditStore` (SQL consts asserted textually — the tip query MUST contain `FOR UPDATE`) and `MemAuditStore` (Mutex-serialized, used by tests and exported for sibling tasks).
- Checkpoint: `pub async fn daily_checkpoint(store, signing_key, day) -> Result<AuditCheckpointRow>` — signs `day || tip_seq || tip_hash` with the on-server ed25519 audit key (generated at first start, 0600, path from config; tests use a tempdir key). Edge: an empty chain (no events yet) → checkpoint refused with a typed error, never a signed empty tip.
- Backfill: `uaa-control audit backfill --actor <github-login> --action <str> --target <str> --outcome <str> --detail <json>` → records a normal chained event with `role='system'` and `action` prefixed `backfill:` — it goes through the SAME serialized append (no side door).

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
  grep -n "FOR UPDATE" docs/specs/constellation-design.md          # expect: 3 hits (~lines 227, 336, 473 — the serialization rule, normative)
  grep -n "32 zero bytes" docs/specs/constellation-design.md       # expect: 2 hits (genesis definition)
  grep -n "audit backfill" docs/specs/constellation-design.md      # expect: 1 hit (~line 127)
  grep -n "^sha2" Cargo.toml                                       # expect: 1 hit
  grep -n "ed25519-dalek" Cargo.toml                               # expect: 1+ hits (workspace dep added by CP-02)
  test -f crates/uaa-control/src/audit.rs && echo OK               # expect: OK (wave gate: CT-01 merged; missing = STOP, too early)
  ```

## Step-by-step

1. Run the ⛔ START HERE block, then every anchor grep above. Any zero-hit / missing-file result → STOP and report.

2. **Canonical hashing.** `pub fn event_hash(prev_hash: &[u8; 32], event: &NewAuditEvent) -> [u8; 32]` — SHA-256 over `prev_hash || canonical_json(event)` (serde_json with sorted map keys — serialize into a `BTreeMap` first so ordering is deterministic). `pub const GENESIS_PREV_HASH: [u8; 32] = [0u8; 32];`
3. **Serialized append.** `pub async fn record(store: &dyn AuditStore, actor, role, action, target, outcome, detail) -> Result<AuditEventRow>` — inside the store's txn: lock+read tip (`FOR UPDATE`), `prev_hash` = tip hash or `GENESIS_PREV_HASH`, compute `hash`, insert. Repeat the law: the tip read and the insert are one critical section; `MemAuditStore` holds its Mutex across both; `PgAuditStore`'s `SQL_SELECT_TIP` const literally contains `FOR UPDATE` (textually tested).
4. **Verification.** `pub fn verify_chain(events: &[AuditEventRow]) -> Result<(), ChainDefect>` — walks seq order, recomputes every hash, checks genesis, returns a typed defect naming the first bad seq (`{ seq, kind: BadPrevHash | BadHash | BadGenesis }`).
5. **Daily checkpoint + backfill.** Implement `daily_checkpoint` (Step-list in Background; empty chain → typed refusal) and `load_or_create_audit_key(state_dir)` (0600). Wire `main.rs`'s `audit` arm: `audit verify` (walk + print), `audit checkpoint` (sign today), `audit backfill --actor … --action … --target … --outcome … --detail …` (chained append with `role='system'`, action prefixed `backfill:`).
6. **Unit tests** (`MemAuditStore`, tempdir key, no network): `test_genesis_prev_hash_is_zero`, `test_append_links_prev_hash` (3 events, each prev == predecessor hash), `test_concurrent_appends_never_fork` (spawn 16 tokio tasks appending concurrently → 16 events, strictly linear chain, `verify_chain` Ok — THE Decision-21 test), `test_verify_detects_tamper` (mutate one mid-chain `detail` → `BadHash` at that seq), `test_verify_detects_reorder` (swap two rows → defect), `test_checkpoint_signs_tip` (signature verifies with the pubkey over `day||tip_seq||tip_hash`), `test_checkpoint_empty_chain_refused`, `test_backfill_goes_through_chain` (backfill event has valid links + `backfill:` prefix), `test_pg_tip_sql_has_for_update` (`SQL_SELECT_TIP` contains `FOR UPDATE`), and the anti-over-suppression test `test_record_happy_path_returns_row` (a plain single append succeeds, verifies, and its fields round-trip intact — the serialization lock does not deadlock or block the ordinary path).
7. Bump the file header (version + last-edited) on every file you touch; new files get a fresh 4-line header with a new uuid4 (`uuidgen | tr 'A-F' 'a-f'`).

## How to test

```bash
cargo test --lib --offline && cargo build --offline
# Expected: all tests pass (baseline 311 + prior control tests + the ~10 new tests), build clean
cargo clippy --offline -- -D warnings
# Expected: no warnings
grep -n "FOR UPDATE" crates/uaa-control/src/audit.rs
# Expected: 1+ hits (the pinned tip-lock SQL const)
cargo test --lib --offline test_concurrent_appends_never_fork
# Expected: 1 passed (the fork-resistance test specifically)
grep -n "GENESIS_PREV_HASH" crates/uaa-control/src/audit.rs | head -2
# Expected: 1+ hits (const definition [0u8; 32] + its use in record)
```

## Acceptance criteria

- [ ] Only `audit.rs` (+ the `main.rs` audit arm) changed: `git diff origin/main --stat` shows no other `crates/uaa-control/src/` file.
- [ ] Fork-resistance proven: `test_concurrent_appends_never_fork` passes (16 concurrent appends → one linear verified chain).
- [ ] Genesis + tamper detection proven: `test_genesis_prev_hash_is_zero`, `test_verify_detects_tamper`, `test_verify_detects_reorder` pass.
- [ ] Serialization pinned in SQL: `grep -n "FOR UPDATE" crates/uaa-control/src/audit.rs` → 1+ hits and `test_pg_tip_sql_has_for_update` passes.
- [ ] Checkpoint + backfill: `test_checkpoint_signs_tip`, `test_checkpoint_empty_chain_refused`, `test_backfill_goes_through_chain` pass; `grep -n "audit backfill\|backfill" crates/uaa-control/src/main.rs` → 1+ hits (CLI arm wired).
- [ ] **Anti-over-suppression:** `test_record_happy_path_returns_row` passes — the tip lock does not block ordinary appends.
- [ ] `cargo test --lib --offline && cargo build --offline` exits 0; clippy clean; no test opens a network connection.
- [ ] File headers bumped on every changed file (`git diff origin/main --stat` files each show a version bump; guids unchanged).

## Commit message

```
feat(control): hash-chained audit log — FOR-UPDATE-serialized append, zero genesis, signed daily checkpoints, backfill cmd (ws2-control)

Fills the CT-01 audit.rs stub per spec Decision 21: sha256 chain over
canonical event bytes with prev_hash read under SELECT ... FOR UPDATE inside
the recording txn (MemAuditStore emulates the lock; PgAuditStore pins it in a
textually-tested SQL const), genesis = 32 zero bytes, verify_chain with typed
defects, ed25519 daily checkpoints (empty chain refused), and the
`uaa-control audit backfill` hatch companion that records through the same
serialized path. Concurrency test proves 16 parallel appends never fork.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

If `grep -n "GENESIS_PREV_HASH" crates/uaa-control/src/audit.rs` hits, already applied — run the acceptance checks instead of re-applying. Rollback = revert the single commit; `audit.rs` returns to CT-01's header-only stub and the `audit` CLI arm to its stub message; no chain data exists anywhere until the daemon runs against a real DB.
