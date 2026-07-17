<!-- file: docs/agent-tasks/registry/TASK-05-drift-scan-accept-revert.md -->
<!-- version: 1.0.0 -->
<!-- guid: e312903f-9af6-48ee-942e-ae1b406fd807 -->
<!-- last-edited: 2026-07-16 -->

# TASK-05 — Drift scan + accept/revert (last-good-version semantics) ⚠ review-critical (DS-REG-05)

**Priority:** P1 · **Effort:** L · **Recommended subagent:** **Opus-class** · rust-control subagent · **Why:** revert semantics are subtle — the obvious implementation ("restore N−1") destroys the evidence it exists to preserve and can silently discard a legitimate change. Never downgrade this tier. · **Depends on:** DS-REG-04 (needs `content_hash` + `capture_version`)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/registry-drift-scan-accept-revert" -b agent/registry-drift-scan-accept-revert origin/main
cd "$REPO/.worktrees/registry-drift-scan-accept-revert"
git rebase origin/main
```

(Protocol also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

**Wave gate:** DS-REG-04 must be merged. If `grep -n "pub fn content_hash" crates/uaa-control/src/profiles/drift.rs` returns 0 hits, the gate is not met: STOP and report.

## Goal

Add drift **detection**, a **periodic scan**, and the **accept** / **revert** review actions to `crates/uaa-control/src/profiles/drift.rs`.

> ## ⚠ Revert is NOT "restore version N−1". Read this twice.
>
> The obvious implementation is wrong in two different ways, and both are silent:
>
> - **If you revert to N−1 when the live row is still labeled N**, you **discard the last legitimate change** along with the drift. The operator asked to undo tampering; you undid their own last edit too.
> - **If `profile_versions` held only *prior* versions**, version N's good body would **never have been captured** — the out-of-band edit overwrote the sole copy, and revert could not reconstruct it at any price.
>
> DS-REG-04 fixed the second by capturing a version on **every** write. This task must get the first right:
>
> **Revert = restore the newest version whose `body` still hashes to its own stored `content_hash`** — the last provably-untampered version. Never a blind `N−1`.
>
> And before **either** review action, **capture the drifted body as its own version row** (`source: "drift"`), so accept and revert both preserve the evidence. v1 of the spec rejected in-place mutation for "destroying the drift evidence" and then specified a revert that destroyed it just as thoroughly.

REUSE — do not invent parallels:

- **`content_hash` / `capture_version`** from DS-REG-04 — import, never reimplement.
- **`AuditStore::append_in_txn`** — verify: `grep -n "async fn append_in_txn" crates/uaa-control/src/audit.rs`. **NOT `record()`.** `record()` passes a no-op mutation and must never be used for something that also changes state; `append_in_txn` commits the mutation and its audit row atomically.
- **`ProfileStore`** (DS-REG-02) for reads/writes; **`read_snapshot_strict`** for any scan read.
- **`crate::db::store::guarded_mutation`** for the accept/revert writes.

## Background (verify before editing)

- **Drift** = `stored.content_hash != content_hash(stored.body)`. It means the body was changed by something that did not go through the API (which would have recomputed the hash) — e.g. a hand-edited snapshot file.
- **⚠ Detection must be scheduled, not incidental.** A read-triggered check only notices drift in objects somebody happens to read; drift in a profile nobody reads is never surfaced. Implement `scan_drift(store) -> Result<Vec<DriftReport>>` walking **every** group and profile, intended to be called periodically. (The caller/scheduler is out of scope for this task — expose the function and unit-test it.)
- **⚠ Repeat drift must not thrash.** An out-of-band editor and the revert button will otherwise append versions forever, one per scan. Report a repeat on the same `object_id` **once with a count**, not once per scan. Track it in memory keyed by `(object_id, content_hash)` — no new persistence.
- **Revert restores INTENT, not the machine.** v1 has no re-render, so revert changes a stored row and **leaves the deployed machine exactly as drifted as it was**. Every user-facing string this task emits must say so — the operator must not read "reverted" as "fleet fixed". Re-deploying is a separate explicit action.
- **The threat model is inherited and bounded** (spec D9, `audit.rs` Decision 21b verbatim): this defends against an editor who cannot also rewrite the stored `content_hash`. Since the hash lives beside the body, anyone who can edit one can edit both — so drift detection's real yield is **accident and mistake detection**, not defense. Do not write a doc comment or log line claiming more.
- Edge semantics (spelled out here AND in acceptance):
  - **No versions exist for a drifted object** → revert is `Err` naming the object. Never invent a body, never fall back to the drifted one.
  - **Every stored version is itself inconsistent** (all hashes mismatch) → revert is `Err`. Never pick "the least bad".
  - **Accept** → capture the drifted body as a version, then write a new version adopting it with a freshly computed hash. Forward-only; nothing is destroyed.
  - **Object is not drifted** → accept/revert are `Err` naming the object ("no drift to review"). Never a silent no-op — an operator clicking revert on a clean object must learn nothing happened.

**HARD RULES (non-negotiable):**
- **NO SQL, NO migration** — no DB connection exists in production (spec D4).
- NO hardware actions. NEVER wipe/write/deploy on 172.16.2.30 or len-serv-003. NEVER power on unimatrixone.
- Do NOT use `read_snapshot` (fail-open) on any path here; use `read_snapshot_strict`.
- Do NOT claim resistance to a server-root adversary anywhere in code, comments, or log strings.
- Stay inside your worktree; never `git push`, `gh pr`, or merge — report done and stop.

- **Re-verify these anchors before editing** — line numbers drift; zero hits = STOP and report:
  ```bash
  grep -n "pub fn content_hash" crates/uaa-control/src/profiles/drift.rs
  # expect: 1 hit — DS-REG-04 merged (0 hits = wave gate not met, STOP)
  grep -n "pub async fn capture_version" crates/uaa-control/src/profiles/drift.rs
  # expect: 1 hit — capture-on-every-write, which makes revert possible
  grep -n "async fn append_in_txn" crates/uaa-control/src/audit.rs
  # expect: 1 hit (~line 157) — the audited-mutation path (NOT record())
  grep -n "server-root adversary defeats it" crates/uaa-control/src/audit.rs
  # expect: 1 hit (~line 35) — the inherited bound you must not overclaim past
  grep -n "pub fn read_snapshot_strict" crates/uaa-control/src/db/store.rs
  # expect: 1 hit — the only read you may use
  grep -n "pub async fn guarded_mutation" crates/uaa-control/src/db/store.rs
  # expect: 1 hit (~line 327) — wrap accept/revert writes in this
  grep -n "pub trait ProfileStore" crates/uaa-control/src/profiles/store.rs
  # expect: 1 hit — the store you read/write through (0 hits = DS-REG-02 not merged, STOP)
  ```

## Step-by-step

1. Open `crates/uaa-control/src/profiles/drift.rs`. Keep its guid; bump its version.
2. Implement:
   ```rust
   pub struct DriftReport { pub object_kind: String, pub object_id: Uuid,
                            pub stored_hash: Vec<u8>, pub actual_hash: Vec<u8>, pub seen_count: u32 }

   /// True iff the stored hash disagrees with the body's actual hash.
   pub fn is_drifted(stored_hash: &[u8], body: &serde_json::Value) -> Result<bool>;

   /// Walk EVERY group and profile. Scheduled, not read-triggered: drift in an
   /// object nobody reads is otherwise never surfaced.
   pub async fn scan_drift(store: &dyn ProfileStore) -> Result<Vec<DriftReport>>;

   /// The newest version whose body still hashes to its own stored content_hash
   /// — the last provably-untampered version. NOT version N-1.
   pub fn last_good_version(versions: &[ProfileVersionRow]) -> Result<&ProfileVersionRow>;

   /// Adopt the current (drifted) body as intended. Captures it as evidence first.
   pub async fn accept_drift(store: &dyn ProfileStore, audit: &dyn AuditStore,
                             object_id: Uuid, actor: &str) -> Result<ProfileVersionRow>;

   /// Restore last_good_version. Captures the drifted body as evidence first.
   /// Restores INTENT, not the machine — the deployed host stays as drifted as
   /// it was; re-deploying is a separate operator action.
   pub async fn revert_drift(store: &dyn ProfileStore, audit: &dyn AuditStore,
                             object_id: Uuid, actor: &str) -> Result<ProfileVersionRow>;
   ```
3. Both review actions: capture the drifted body (`source: "drift"`) **first**, then write the new version via `append_in_txn` with the caller-supplied actor.
4. Keep purely additive — do not modify `audit.rs`, `read_snapshot`, `RegistryStore`, or DS-REG-04's functions.
5. Add tests in `drift.rs`'s `mod tests` (`MemProfileStore` + `MemAuditStore`):
   - `test_drift_detected_on_out_of_band_edit` — mutate a stored body without recomputing its hash ⇒ `is_drifted` true, and `scan_drift` reports it.
   - **`test_scan_finds_drift_in_unread_object`** — an object never fetched individually is still reported. (Proves detection is scheduled, not read-triggered.)
   - **`test_revert_restores_last_good_not_n_minus_1`** — v1 good, v2 good (a legitimate change), then v2's body is tampered out-of-band ⇒ revert restores **v2's** body, **not v1's**. *This is the whole task: a blind N−1 would silently discard the legitimate v2 change.*
   - **`test_revert_captures_drifted_body_first`** — after revert, a version row with `source: "drift"` holds the tampered body. The evidence survives.
   - `test_accept_captures_drifted_body_and_adopts` — accept records the evidence and then adopts the body with a fresh, correct hash.
   - `test_revert_errors_when_no_good_version` — every version inconsistent ⇒ `Err`; never "least bad".
   - `test_review_on_clean_object_errors` — accept/revert on a non-drifted object ⇒ `Err`, not a silent no-op.
   - `test_repeat_drift_reported_once_with_count` — the same drift across three scans ⇒ one report, `seen_count: 3`.
   - `test_review_actions_are_audited` — `MemAuditStore` recorded an event with the caller's actor for both accept and revert.
   - **`test_clean_object_is_not_reported`** — a fleet-shaped set of un-drifted objects ⇒ `scan_drift` returns empty.
6. Bump the header; keep the guid.

**Anti-over-suppression:** drift detection is a filter and can over-report. `test_clean_object_is_not_reported` is the happy-path guard — a scan over untouched objects must return **empty**, not flag everything. Without it, a hash bug that reports every object as drifted would look like a working detector while making the review queue useless.

## How to test

```bash
cargo test --lib --offline
# Expected: 634+ passed, 0 failed (baseline + DS-REG-01/02/04's tests + your 10).
cargo build --offline
# Expected: exit 0.
cargo clippy --offline -- -D warnings
# Expected: no warnings.
```

## Acceptance criteria

- [ ] `cargo test --lib --offline` exits 0 — verify: `cargo test --lib --offline 2>&1 | grep -E "^test result"`
- [ ] `cargo build --offline` exits 0 — verify: `cargo build --offline && echo BUILD_OK`
- [ ] **Revert restores last-good, not N−1** — verify: `cargo test --lib --offline test_revert_restores_last_good_not_n_minus_1`
- [ ] **Evidence is preserved by both actions** — verify: `cargo test --lib --offline test_revert_captures_drifted_body_first test_accept_captures_drifted_body_and_adopts`
- [ ] Detection is scheduled — verify: `cargo test --lib --offline test_scan_finds_drift_in_unread_object`
- [ ] No thrash — verify: `cargo test --lib --offline test_repeat_drift_reported_once_with_count`
- [ ] Anti-over-suppression: clean objects are not reported — verify: `cargo test --lib --offline test_clean_object_is_not_reported`
- [ ] Audited via `append_in_txn`, never `record` — verify: `grep -c "record(" crates/uaa-control/src/profiles/drift.rs` returns **0**, and `cargo test --lib --offline test_review_actions_are_audited` passes
- [ ] No fail-open read — verify: `grep -c "read_snapshot(" crates/uaa-control/src/profiles/drift.rs` returns **0**
- [ ] No overclaim — verify: `grep -ci "tamper-proof\|cannot be tampered\|prevents tampering" crates/uaa-control/src/profiles/drift.rs` returns **0**
- [ ] "Restores intent, not the machine" is stated in the revert doc comment — verify: `grep -c "not the machine" crates/uaa-control/src/profiles/drift.rs` returns ≥1 (exact phrase, case-sensitive — a loose `-i "intent"` would also match "intentional" and pass without the wording actually being there)
- [ ] `cargo clippy --offline -- -D warnings` clean
- [ ] File header bumped — verify: `grep -n "last-edited: 2026-07" crates/uaa-control/src/profiles/drift.rs`

## Coordinator review checklist (⚠ review-critical — line-by-line before merge)

- [ ] `revert_drift` selects via `last_good_version` (newest self-consistent), **never** an index-arithmetic `N−1`.
- [ ] Both accept and revert capture the drifted body **before** writing anything.
- [ ] `append_in_txn` with a real actor — no `record()`, no placeholder actor string.
- [ ] `read_snapshot_strict` only; no fail-open read, no `unwrap_or_default`.
- [ ] No comment or log claims defense against a root-level adversary.

## Commit message

```
feat(control): drift scan with last-good-version revert (DS-REG-05)

Adds is_drifted, a scheduled scan_drift over EVERY object, and the accept /
revert review actions.

Revert restores the newest version whose body still hashes to its own stored
content_hash — NOT version N-1. A blind N-1 silently discards the last
legitimate change along with the drift; and had DS-REG-04 not captured a
version on every write, version N's good body would never have existed to
restore. Both actions capture the drifted body as evidence first, so accept
and revert preserve what happened rather than destroying it.

Detection is scheduled, not read-triggered: drift in an object nobody reads
would otherwise never surface. Repeat drift is reported once with a count so
an out-of-band editor and the revert button cannot thrash forever.

Revert restores INTENT, not the machine: v1 has no re-render, so the deployed
host stays as drifted as it was. Threat model is inherited verbatim from
Decision 21b and not extended — the hash lives beside the body, so this
detects accidents, not a root adversary.

Co-Authored-By: Claude <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge. This task is ⚠ review-critical: expect line-by-line review against the checklist above.

## Idempotency / Rollback

**Polarity: additive.** If `grep -n "pub async fn revert_drift" crates/uaa-control/src/profiles/drift.rs` hits, this task is already applied — run the acceptance checks instead of re-applying. Rollback = revert the single commit; `content_hash`/`capture_version` (DS-REG-04) survive, no review action can have run because nothing calls these until DS-OPS-02, and no data or schema is touched. DS-REG-04 also owns `drift.rs` — this task rebases after it merges; see the collision table in `../BREAKDOWN-2026-07-16.md`.
