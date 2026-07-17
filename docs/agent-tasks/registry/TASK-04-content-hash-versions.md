<!-- file: docs/agent-tasks/registry/TASK-04-content-hash-versions.md -->
<!-- version: 1.0.0 -->
<!-- guid: 6d295d55-5353-41d8-b54d-b17b3ef5ec67 -->
<!-- last-edited: 2026-07-16 -->

# TASK-04 — `content_hash` (explicit canonicalization) + `profile_versions` writes (DS-REG-04)

**Priority:** P1 · **Effort:** M · **Recommended subagent:** Sonnet-class · rust-control subagent · **Why:** hash determinism rests on unpinned `serde_json` assumptions; a naive implementation ships a test that passes without testing anything. · **Depends on:** DS-REG-02 (fills `profiles/drift.rs`, uses `ProfileStore`)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/registry-content-hash-versions" -b agent/registry-content-hash-versions origin/main
cd "$REPO/.worktrees/registry-content-hash-versions"
git rebase origin/main
```

(Protocol also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

**Wave gate:** DS-REG-02 must be merged. If `grep -n "pub trait ProfileStore" crates/uaa-control/src/profiles/store.rs` returns 0 hits, the gate is not met: STOP and report.

## Goal

Fill `crates/uaa-control/src/profiles/drift.rs` with `content_hash`, and make every profile write append a `ProfileVersionRow`.

**This task ships hashing + version capture only.** Drift *detection* and accept/revert are DS-REG-05.

REUSE — do not invent parallels:

- **`ProfileVersionRow`** from `db/mod.rs` (DS-REG-01) — import, never redefine.
- **`ProfileStore::put_version`** (DS-REG-02) — the write path.
- **`serde_bytes_hex`** — verify: `grep -n "mod serde_bytes_hex" crates/uaa-control/src/db/mod.rs`. Already used by the `content_hash` fields.
- **`sha2::Sha256`** — already a dependency (verify: `grep -n "sha2" crates/uaa-control/Cargo.toml`); `audit.rs` uses it. Do NOT add a crate.
- **Read `audit.rs`'s `canonical_bytes` for the IDEA** — verify: `grep -n "fn canonical_bytes" crates/uaa-control/src/audit.rs`. It sorts keys into a `BTreeMap` so ordering never depends on a serde feature. You are doing the same thing for an **arbitrary nested body** — a different function, same principle. Do NOT call `audit.rs`'s (it hashes an *event*, not an object body — spec D10).

## Background (verify before editing)

- **⚠ Do NOT lean on `serde_json`'s internal ordering — and do not write the test that a naive implementation would pass.** Today `serde_json` has `preserve_order` **off**, so `Value::Object` is a `BTreeMap` and keys re-sort on parse. That makes a naive `SHA-256(serde_json::to_vec(body))` *look* deterministic — **and makes the obvious test vacuous**: it passes whether or not you canonicalize, so it guards nothing. Two unpinned assumptions break it later: (a) any dependency enabling `preserve_order` (a global feature-unification hazard — `audit.rs` explicitly defends only its **top level** against this), and (b) a float entering a body (`1.0` vs `1` round-trip differently).
  Therefore `content_hash` **recursively sorts keys into a `BTreeMap` itself** and **rejects float values outright**, and its test feeds a **deliberately shuffled-key** input constructed so it would fail if canonicalization were removed.
- **`profile_versions` captures the body on EVERY write** — not only on change. DS-REG-05's revert restores "the newest version whose body still hashes to its own stored `content_hash`", which only works if version N was captured **before** an out-of-band edit could overwrite it. A version row written only on *detected drift* is too late: the drifted write already destroyed the good body (spec D11).
- Edge semantics (spelled out here AND in acceptance):
  - **Key order in the input body** → must not change the hash. This is the entire property.
  - **A float anywhere in the body** → hard `Err` naming the path to the offending value. Never hash it silently; `1.0` and `1` are the same JSON number and different bytes, so a silent hash is a hash that changes for no reason.
  - **An empty body `{}`** → legal; hashes to a stable value. NOT an error.
  - **Nested objects/arrays** → canonicalized recursively. Array *order* is significant and preserved; only object **keys** are sorted.
  - **Version numbering** → monotonic per `object_id`, starting at 1. A gap is never created; a duplicate `(object_id, version)` is an `Err`.

**HARD RULES (non-negotiable):**
- **NO SQL, NO migration** — no DB connection exists in production (spec D4).
- NO hardware actions. NEVER wipe/write/deploy on 172.16.2.30 or len-serv-003. NEVER power on unimatrixone.
- Do NOT call `audit.rs`'s `canonical_bytes` or `event_hash` — they hash an *event*; this hashes an *object body*. Conflating them misstates what the audit chain proves (spec D10).
- Do NOT add a dependency.
- Stay inside your worktree; never `git push`, `gh pr`, or merge — report done and stop.

- **Re-verify these anchors before editing** — line numbers drift; zero hits = STOP and report:
  ```bash
  grep -n "pub trait ProfileStore" crates/uaa-control/src/profiles/store.rs
  # expect: 1 hit — DS-REG-02 merged (0 hits = wave gate not met, STOP)
  grep -n "fn canonical_bytes" crates/uaa-control/src/audit.rs
  # expect: 1 hit (~line 108) — the sorted-BTreeMap IDEA to mirror (do NOT call it)
  grep -n "pub struct ProfileVersionRow" crates/uaa-control/src/db/mod.rs
  # expect: 1 hit — the row you append
  grep -n "mod serde_bytes_hex" crates/uaa-control/src/db/mod.rs
  # expect: 1 hit — the hex codec on content_hash fields
  grep -n "preserve_order" Cargo.toml crates/uaa-control/Cargo.toml
  # expect: 0 hits — PROOF the feature is off today, which is exactly why the
  #         naive test is vacuous and you must canonicalize explicitly
  ```

## Step-by-step

1. Open `crates/uaa-control/src/profiles/drift.rs` (DS-REG-02's stub). Keep its guid; bump its version.
2. Implement:
   ```rust
   /// Canonicalize a body: recursively sort object keys into a BTreeMap, preserve
   /// array order, reject floats. Deliberately does NOT rely on serde_json's
   /// internal ordering — preserve_order is off today, but that is a global
   /// feature-unification hazard, and a test that relies on it guards nothing.
   pub fn canonical_body_bytes(body: &serde_json::Value) -> Result<Vec<u8>>;

   /// SHA-256 over canonical_body_bytes.
   pub fn content_hash(body: &serde_json::Value) -> Result<[u8; 32]>;
   ```
3. Add a version-capture helper that callers use on **every** write:
   ```rust
   /// Append the body as the next version for this object. Called on EVERY write,
   /// not only on change — DS-REG-05's revert needs version N to exist BEFORE an
   /// out-of-band edit can overwrite it.
   pub async fn capture_version(
       store: &dyn ProfileStore, object_kind: &str, object_id: Uuid,
       body: &serde_json::Value, actor: &str,
   ) -> Result<ProfileVersionRow>;
   ```
4. Wire `capture_version` into `put_group` / `put_profile` in `profiles/store.rs`, and set each row's `content_hash` from `content_hash(body)` on write.
5. Keep purely additive — do not modify `audit.rs`, `read_snapshot`, or `RegistryStore`.
6. Add tests in `drift.rs`'s `mod tests`:
   - **`test_content_hash_is_canonical`** — build two `serde_json::Value`s with the **same keys inserted in different order** (construct them via `serde_json::from_str` of two differently-ordered JSON texts, and include a **nested** object whose keys also differ in order) ⇒ **equal hashes**. This must be written so it would FAIL if `canonical_body_bytes` were replaced by `serde_json::to_vec`. Add a comment saying so — otherwise a future reader "simplifies" it back.
   - `test_content_hash_rejects_float` — a body containing `1.5` ⇒ `Err` naming the path.
   - `test_content_hash_empty_body_is_stable` — `{}` hashes the same twice, no error.
   - `test_content_hash_array_order_is_significant` — `[1,2]` and `[2,1]` hash **differently** (only object keys sort, not arrays).
   - `test_capture_version_on_every_write` — two `put_group` calls with the **same** body ⇒ **two** version rows (v1, v2). Capture is unconditional; this is what makes revert possible.
   - `test_capture_version_is_monotonic` — versions are 1, 2, 3 with no gaps.
   - `test_version_body_hash_matches_stored` — every captured row's `content_hash` equals `content_hash(row.body)` (the self-consistency property DS-REG-05's revert selects on).
7. Bump headers on every file you touch; keep existing guids.

**Anti-over-suppression:** `content_hash`'s float rejection is a guard that can over-block. `test_content_hash_empty_body_is_stable` and `test_content_hash_is_canonical` are the happy-path proofs that ordinary bodies (the fleet's are all String/bool/integer) still hash cleanly — without them an over-strict validator would reject every real profile.

## How to test

```bash
cargo test --lib --offline
# Expected: 634+ passed, 0 failed (baseline + DS-REG-01/02's tests + your 7).
cargo build --offline
# Expected: exit 0.
cargo clippy --offline -- -D warnings
# Expected: no warnings.
```

## Acceptance criteria

- [ ] `cargo test --lib --offline` exits 0 — verify: `cargo test --lib --offline 2>&1 | grep -E "^test result"`
- [ ] `cargo build --offline` exits 0 — verify: `cargo build --offline && echo BUILD_OK`
- [ ] **Canonicalization is explicit, not inherited** — verify: `grep -c "BTreeMap" crates/uaa-control/src/profiles/drift.rs` returns ≥1, and `cargo test --lib --offline test_content_hash_is_canonical`
- [ ] The canonical test is non-vacuous — verify: the test's body contains **nested** differently-ordered objects and a comment stating it must fail if canonicalization is removed: `grep -c "would FAIL if" crates/uaa-control/src/profiles/drift.rs` returns ≥1
- [ ] Floats rejected — verify: `cargo test --lib --offline test_content_hash_rejects_float`
- [ ] **Version captured on every write, not only on change** — verify: `cargo test --lib --offline test_capture_version_on_every_write`
- [ ] Self-consistency holds — verify: `cargo test --lib --offline test_version_body_hash_matches_stored`
- [ ] Anti-over-suppression: ordinary bodies still hash — verify: `cargo test --lib --offline test_content_hash_empty_body_is_stable`
- [ ] `audit.rs` untouched and not called — verify: `git diff origin/main --name-only | grep -c "audit.rs"` returns **0**, and `grep -c "canonical_bytes\|event_hash" crates/uaa-control/src/profiles/drift.rs` returns **0**
- [ ] No SQL, no migration — verify: `git diff origin/main | grep -c "CREATE TABLE\|SQL_"` returns **0**
- [ ] `cargo clippy --offline -- -D warnings` clean
- [ ] File headers bumped — verify: `git diff origin/main --name-only | xargs -I{} grep -l "last-edited: 2026-07" {}`

## Commit message

```
feat(control): content_hash with explicit canonicalization + version capture (DS-REG-04)

content_hash recursively sorts object keys into a BTreeMap and rejects floats
rather than leaning on serde_json's internal ordering. preserve_order is off
today, which makes a naive SHA-256(to_vec(body)) LOOK deterministic — and makes
the obvious test vacuous, since it passes whether or not you canonicalize. Two
unpinned assumptions break it later: any dependency enabling preserve_order (a
global feature-unification hazard) and a float entering a body.

profile_versions is captured on EVERY write, not only on change: DS-REG-05's
revert restores the newest version whose body still hashes to its own stored
hash, which requires version N to exist before an out-of-band edit can
overwrite it.

Deliberately does not call audit.rs's canonical_bytes — that hashes an event
(was the log tampered with), this hashes an object body (did this object
change out-of-band). Conflating them would misstate what the chain proves.

Co-Authored-By: Claude <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

**Polarity: additive** (fills an empty stub). If `grep -n "pub fn content_hash" crates/uaa-control/src/profiles/drift.rs` hits, this task is already applied — run the acceptance checks instead of re-applying. Rollback = revert the single commit, restoring the empty stub; nothing detects drift yet (DS-REG-05), and no profile data can exist in production because nothing writes profiles until DS-OPS-01. **Shared files:** DS-REG-05 (wave 5) fills the rest of `drift.rs` and must rebase after this merges; and this task edits `profiles/store.rs` (wiring `capture_version` into `put_group`/`put_profile`), which DS-REG-02 (wave 2) and DS-REG-03 (wave 3) also own — all three are in different waves. This task does **not** edit `db/mod.rs`; it only imports `ProfileVersionRow` from it. See the collision table in `../BREAKDOWN-2026-07-16.md`.
