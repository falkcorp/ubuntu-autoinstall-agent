<!-- file: docs/agent-tasks/registry/TASK-01-snapshot-rows-scaffold.md -->
<!-- version: 1.0.0 -->
<!-- guid: 8433ecc7-0a7b-4687-88cf-bc5e8607fceb -->
<!-- last-edited: 2026-07-16 -->

# TASK-01 — Profile row types + `SnapshotDoc` collections + `profiles/` module scaffold (DS-REG-01)

**Priority:** P1 · **Effort:** M · **Recommended subagent:** Sonnet-class · rust-control subagent · **Why:** shapes the four row types every sibling task depends on, in `db/mod.rs` — the crate's declared single home for row types. · **Depends on:** none (wave 1)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/registry-snapshot-rows-scaffold" -b agent/registry-snapshot-rows-scaffold origin/main
cd "$REPO/.worktrees/registry-snapshot-rows-scaffold"
git rebase origin/main
```

(Protocol also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

## Goal

Add four row types to `crates/uaa-control/src/db/mod.rs`, four matching `#[serde(default)]` collections to `SnapshotDoc` in `crates/uaa-control/src/db/store.rs`, and scaffold the `crates/uaa-control/src/profiles/` module (`mod.rs` + empty `store.rs`/`alloc.rs`/`drift.rs` stubs) so sibling tasks fill **disjoint** files and never collide on `mod.rs`.

**This task ships types and empty stubs only.** No store impl (DS-REG-02), no allocation (DS-REG-03), no hashing (DS-REG-04).

> **⚠ There is NO SQL and NO migration in this task or anywhere in this package.** `uaa-control` has **no database connection in production** — verified: `tokio_postgres` appears in no wiring file, `default_state()` builds `FileRegistry(StatePaths)` + `Mem*Store`, and `db::migrations::apply` has no caller. Profiles persist in the `StatePaths` **JSON snapshot** (spec `deploy-system-design.md` D4). If you find yourself writing `CREATE TABLE`, `SQL_*`, or editing `migrations.rs`, you have the wrong design — STOP and report.

REUSE — do not invent parallels:

- **Row types live in `db/mod.rs`**, per that file's own module doc. Verify: `grep -n "pre-declared HERE" crates/uaa-control/src/db/mod.rs`. Do NOT put them in `profiles/`.
- **`serde_bytes_hex`** for the `content_hash: Vec<u8>` fields — verify: `grep -n "mod serde_bytes_hex" crates/uaa-control/src/db/mod.rs`. Do NOT invent a second hex codec.
- **Mirror `MachineRow`'s field conventions** — verify: `grep -n "pub struct MachineRow" crates/uaa-control/src/db/mod.rs`. Timestamps are `Option<String>`, **never** `chrono::DateTime`; this is deliberate (avoids tokio-postgres feature flags).
- `uuid` (v4) and `serde_json` are already workspace deps. Do NOT add anything to `Cargo.toml`.

## Background (verify before editing)

- `SnapshotDoc` currently has six `#[serde(default)]` collections. **`#[serde(default)]` on every field is what makes adding collections backward-compatible with snapshot files already on disk** — a running server's existing snapshot must keep parsing. Omit it and you break production state on deploy.
- Field shapes come from the spec's Data model section (`deploy-system-design.md` § Data model) — copy them exactly. Key points a weak model will otherwise get wrong:
  - `HostGroupRow.name` is the hostname prefix **and is immutable** — allocations key on `id: Uuid`, **never** on `name` (spec D2: keying on the mutable name orphans every allocation on rename).
  - `HostnameAllocationRow`'s key is `(group_id, identity)` — carries `released_at` (soft release) and `rebound_to` (tombstone).
  - `HostProfileRow.identity` is the MAC (spec D-A/A1), stored `normalize_mac`'d.
- Edge semantics (spelled out here AND in acceptance):
  - **An existing snapshot file with no profile collections** → parses fine; the four new collections default to empty vecs. NOT an error.
  - **`content_hash` on a freshly-constructed row** → an empty `Vec<u8>` is legal at this task's scope (DS-REG-04 computes real hashes). Do NOT invent a placeholder hash value.
  - **`index` is a plain `i64` field name** — legal in Rust. (It is a reserved keyword in CockroachDB, which is irrelevant here because there is no SQL.)

**HARD RULES (non-negotiable):**
- NO hardware actions. NEVER wipe/write/deploy on 172.16.2.30 or len-serv-003. NEVER power on unimatrixone.
- No real secret in any file; `REPLACE_AT_PLACE_TIME` stays a placeholder.
- Do NOT edit `crates/uaa-control/src/db/registry.rs` — `RegistryStore` is a 14-method trait whose `RecordingRegistry` in `saga.rs` hand-forwards every method; growing it breaks a file outside this scope (spec D5).
- Stay inside your worktree; never `git push`, `gh pr`, or merge — report done and stop.

- **Re-verify these anchors before editing** — line numbers drift; zero hits = STOP and report:
  ```bash
  grep -n "pub struct MachineRow" crates/uaa-control/src/db/mod.rs
  # expect: 1 hit (~line 193) — the row-convention reference to mirror
  grep -n "mod serde_bytes_hex" crates/uaa-control/src/db/mod.rs
  # expect: 1 hit (~line 319) — the hex codec to reuse for content_hash
  grep -n "pub struct SnapshotDoc" crates/uaa-control/src/db/store.rs
  # expect: 1 hit (~line 99) — add four collections here
  grep -n "pub trait SagaStore: Send + Sync" crates/uaa-control/src/saga.rs
  # expect: 1 hit (~line 241) — the separate-trait+module precedent this scaffold follows
  grep -n "^pub mod \|^mod " crates/uaa-control/src/lib.rs
  # expect: several hits — add `pub mod profiles;` alongside them, mirroring the style
  ```

## Step-by-step

1. In `crates/uaa-control/src/db/mod.rs`, add `HostGroupRow`, `HostProfileRow`, `HostnameAllocationRow`, `ProfileVersionRow` exactly as the spec's Data model defines them. Derive `Debug, Clone, Serialize, Deserialize`. Use `#[serde(with = "serde_bytes_hex")]` on every `content_hash: Vec<u8>`.
2. In `crates/uaa-control/src/db/store.rs`, add to `SnapshotDoc`, each with `#[serde(default)]`:
   `host_groups: Vec<HostGroupRow>`, `host_profiles: Vec<HostProfileRow>`, `hostname_allocations: Vec<HostnameAllocationRow>`, `profile_versions: Vec<ProfileVersionRow>`.
3. Create `crates/uaa-control/src/profiles/mod.rs` with a fresh 4-line header (new uuid4 via `uuidgen | tr '[:upper:]' '[:lower:]'`) declaring `pub mod store; pub mod alloc; pub mod drift;`, plus a module doc stating: profiles persist in the StatePaths snapshot, NOT CockroachDB, and why (spec D4).
4. Create `store.rs`, `alloc.rs`, `drift.rs` as stubs — each with its own fresh header and a `// Filled by DS-REG-0N.` comment. **Empty stubs, no logic.** They must compile.
5. Declare `pub mod profiles;` in `crates/uaa-control/src/lib.rs`, mirroring the existing module lines.
6. Keep the change purely additive — do not modify `MachineRow`, `RegistryStore`, `registry.rs`, `migrations.rs`, or any existing `SnapshotDoc` field.
7. Add tests in `db/store.rs`'s existing test module:
   - `test_snapshot_without_profile_collections_still_parses` — a JSON snapshot containing only the six original collections round-trips; the four new ones are empty. **This is the backward-compatibility guard for production state.**
   - `test_snapshot_roundtrips_profile_collections` — a doc with one of each row survives `write_snapshot` → `read_snapshot`.
   - `test_content_hash_serializes_as_hex` — a `HostGroupRow` with `content_hash: vec![0xde, 0xad]` serializes to `"dead"`.
8. Bump the header (`version` + `last-edited`) on every file you touch; keep existing guids.

**Anti-over-suppression:** `test_snapshot_without_profile_collections_still_parses` is the happy-path guard — it proves the new `#[serde(default)]` collections do not reject an existing on-disk snapshot. Without it, a missing default would silently break every running server's state on deploy.

## How to test

```bash
cargo test --lib --offline
# Expected: 634+ passed, 0 failed (634 baseline + your 3).
cargo build --offline
# Expected: exit 0.
cargo clippy --offline -- -D warnings
# Expected: no warnings.
```

## Acceptance criteria

- [ ] `cargo test --lib --offline` exits 0 — verify: `cargo test --lib --offline 2>&1 | grep -E "^test result"`
- [ ] `cargo build --offline` exits 0 — verify: `cargo build --offline && echo BUILD_OK`
- [ ] All four row types exist — verify: `grep -c "pub struct HostGroupRow\|pub struct HostProfileRow\|pub struct HostnameAllocationRow\|pub struct ProfileVersionRow" crates/uaa-control/src/db/mod.rs` returns 4
- [ ] All four collections carry `#[serde(default)]` — verify: `grep -A1 "pub host_groups\|pub host_profiles\|pub hostname_allocations\|pub profile_versions" crates/uaa-control/src/db/store.rs | grep -c "serde(default)"` returns 4 (attribute precedes each field; adjust the grep direction if your formatter differs — the requirement is that each of the four has it)
- [ ] **No SQL, no migration** — verify: `git diff origin/main --name-only | grep -c "migrations"` returns **0**, and `git diff origin/main | grep -c "CREATE TABLE\|SQL_"` returns **0**
- [ ] `RegistryStore` untouched — verify: `git diff origin/main --name-only | grep -c "db/registry.rs"` returns **0**
- [ ] Backward compatibility — verify: `cargo test --lib --offline test_snapshot_without_profile_collections_still_parses`
- [ ] Stubs compile and are empty — verify: `wc -l crates/uaa-control/src/profiles/store.rs` returns < 15
- [ ] `cargo clippy --offline -- -D warnings` clean
- [ ] File headers bumped on every changed file — verify: `git diff origin/main --name-only | xargs -I{} grep -l "last-edited: 2026-07" {}`

## Commit message

```
feat(control): add profile row types, snapshot collections, profiles scaffold (DS-REG-01)

Adds HostGroupRow/HostProfileRow/HostnameAllocationRow/ProfileVersionRow to
db/mod.rs (the crate's declared single home for row types) and four
#[serde(default)] collections to SnapshotDoc, so snapshot files already on
disk keep parsing unchanged.

Scaffolds crates/uaa-control/src/profiles/ with mod.rs plus empty
store/alloc/drift stubs, following saga.rs's separate-trait+module precedent
rather than growing RegistryStore — whose RecordingRegistry in saga.rs
hand-forwards every method and would break.

No SQL and no migration: uaa-control has no database connection in
production, so profiles persist in the StatePaths snapshot (spec D4).

Co-Authored-By: Claude <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

**Polarity: additive.** If `grep -n "pub struct HostnameAllocationRow" crates/uaa-control/src/db/mod.rs` hits, this task is already applied — run the acceptance checks instead of re-applying. Rollback = revert the single commit; existing snapshot files are unaffected (the new collections were additive and defaulted), no data is touched, and nothing reads the new types yet. Siblings that also edit `db/mod.rs` (DS-REG-04) or `db/store.rs` (DS-REG-02) must rebase after this merges — see the collision table in `../BREAKDOWN-2026-07-16.md`.
