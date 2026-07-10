<!-- file: docs/agent-tasks/luks-keys/TASK-03-luks-registry-sync.md -->
<!-- version: 1.0.0 -->
<!-- guid: 6a77a205-74d9-4467-8768-6b37f0024ac9 -->
<!-- last-edited: 2026-07-10 -->

# TASK-03 — Report enrolled FIDO2 credentials to control (luks_credentials table) — client sync module (ws7-luks)

**Priority:** P2 · **Effort:** S · **Recommended subagent:** Haiku-class · rust-client subagent · **Why:** mechanical read-and-serialize POST of local state. · **Depends on:** TASK-01 + CT-02 (wave-5 gated: LK-01 merged — it defines the local state file this module reads — AND `control/TASK-02` (CT-02) merged — it creates the `luks_credentials` endpoints this module posts to)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/luks-keys-luks-registry-sync" -b agent/luks-keys-luks-registry-sync origin/main
cd "$REPO/.worktrees/luks-keys-luks-registry-sync"
git rebase origin/main
```

(Protocol also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

## Goal

Fill the CP-01-created stub `crates/uaa-core/src/luks_sync.rs`: a small client module that reads the local LUKS credential state file written by LK-01 (`enroll_fido2`) and LK-02 (`revoke_fido2`), serializes it, and POSTs it to uaa-control's `luks_credentials` endpoint so the registry table (spec `docs/specs/constellation-design.md` §data model, `CREATE TABLE luks_credentials`, and §C8 "Registry sync to `luks_credentials`") mirrors reality. Purely additive; the 3-credential-per-host model is `PLAN-zfs-luks-multikey.md`'s.

**YubiKeys are for LUKS disk unlock, NOT auth.** This module reports keyslot bookkeeping to the registry; it grants nothing and authenticates nothing (spec Decision 14).

REUSE — do not invent parallels:

- **`LuksCredentialRecord` + `CredentialRole`** from `crates/uaa-core/src/luks_keys.rs` (LK-01) — verify: `grep -n "pub struct LuksCredentialRecord" crates/uaa-core/src/luks_keys.rs`. Do NOT define a second credential struct; the sync payload embeds this one.
- **HTTP client idiom:** mirror `call_flip_api` (`src/autoinstall/place.rs` — verify: `grep -n "pub async fn call_flip_api" src/autoinstall/place.rs`; `reqwest` + `serde_json::Value` body inspection with an `ok` bool). `reqwest` is already a dependency — do NOT add `ureq`/`hyper`/anything new to `Cargo.toml`.
- **`AutoInstallError::ConfigError` / `SystemError`** (`src/error.rs`) for all error paths.

## Background (verify before editing)

- The registry side (CT-02, `crates/uaa-control/src/db/registry.rs` + `import_export.rs`) exposes luks_credentials/tang endpoints on the control machine plane; the sync URL is caller-supplied (the CLI resolves control's address — this module never hardcodes `172.16.2.30`).
- Local state file (LK-01 contract, default path the CLI passes: `/var/lib/uaa/luks-credentials.json`): a JSON ARRAY of `LuksCredentialRecord` — fields `yubikey_serial`, `role` (`"primary"|"backup1"|"backup2"`), `luks_keyslot` (nullable int), `enrolled_at` (RFC3339 string), `revoked_at` (nullable RFC3339 string).
- The registry table keys credentials by host `mac` (spec: `mac STRING NOT NULL REFERENCES machines (mac)`), so the payload wraps the records with the reporting host's MAC.
- Edge semantics (spelled out here AND in acceptance):
  - **Missing state file** → NOT an error: sync sends an EMPTY records list (a freshly-installed host has nothing enrolled yet; control learning "zero credentials" is correct data). Distinguish it in the return value (`records_sent: 0`).
  - **Malformed JSON in the state file** → hard `SystemError` naming the path — never "send what parsed"; a half-report would let control silently drop revocations.
  - **Non-2xx or `ok:false` response** → `SystemError` including status + body message; the local file is NEVER modified by sync (read-only module — no write, no tmp file, nothing).
  - **Empty/invalid MAC** (not 6 colon-separated hex pairs) → `ConfigError` before any HTTP call.
- Testing: unit tests exercise payload construction and state-file reading only; the HTTP POST function is a thin seam kept separate so it needs no live server. No live CockroachDB, no network, in any test.

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
  grep -n "luks_credentials" docs/specs/constellation-design.md
  # expect: 2+ hits (lines 315 + 535 today — table DDL + C8 registry-sync sentence;
  #         this spec lands with the planning package: if absent on main, run the grep
  #         in the plan worktree .worktrees/plan-constellation/docs/specs/)
  grep -n "pub struct LuksCredentialRecord" crates/uaa-core/src/luks_keys.rs
  # expect: 1 hit — LK-01 merged (0 hits = wave gate not met, STOP)
  grep -n "pub async fn call_flip_api" src/autoinstall/place.rs
  # expect: 1 hit (line 214 today; post-CP-01: crates/uaa-core/src/autoinstall/place.rs)
  grep -n "luks_sync" crates/uaa-core/src/lib.rs
  # expect: 1 hit — CP-01 declared the stub module
  ```

## Step-by-step

1. Run the ⛔ START HERE block, then the anchor greps. `LuksCredentialRecord` absent → LK-01 not merged → STOP and report (wave gate).

2. **Replace the stub body of `crates/uaa-core/src/luks_sync.rs`** (keep the CP-01 header; bump version + last-edited, keep guid) with the payload types:

   ```rust
   use crate::luks_keys::LuksCredentialRecord;

   /// What one host reports to control's luks_credentials endpoint.
   #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
   pub struct LuksSyncPayload {
       pub mac: String,                            // lowercase aa:bb:cc:dd:ee:ff
       pub records: Vec<LuksCredentialRecord>,
   }

   #[derive(Debug, Clone, PartialEq)]
   pub struct LuksSyncOutcome { pub records_sent: usize, pub message: String }
   ```

3. **Add the reader + payload builder (pure, unit-testable):**

   ```rust
   /// Normalize + validate a MAC: lowercase, must be 6 colon-separated hex
   /// pairs; anything else => ConfigError. (Checked BEFORE any file/HTTP work.)
   pub fn normalize_mac(mac: &str) -> crate::error::Result<String>;

   /// Read the LK-01 state file. Missing file => Ok(vec![]) — NOT an error
   /// (fresh host, zero credentials is correct data). Present-but-malformed
   /// JSON => Err(SystemError naming the path) — never a partial report.
   pub fn read_local_state(state_path: &std::path::Path)
       -> crate::error::Result<Vec<LuksCredentialRecord>>;

   pub fn build_payload(mac: &str, records: Vec<LuksCredentialRecord>)
       -> crate::error::Result<LuksSyncPayload>;   // uses normalize_mac
   ```

4. **Add the POST seam,** mirroring `call_flip_api`'s response handling (status + JSON `ok`/`message` body):

   ```rust
   /// POST the payload as JSON to `<control_url>/luks-credentials` (CT-02's
   /// endpoint; control_url is caller-supplied — never hardcoded here).
   /// 2xx with ok:true => Ok(LuksSyncOutcome); anything else => SystemError
   /// with status + body message. Read-only w.r.t. local state: this module
   /// never writes any file.
   pub async fn post_sync(control_url: &str, payload: &LuksSyncPayload)
       -> crate::error::Result<LuksSyncOutcome>;

   /// Convenience: read + build + post. All validation errors fire before
   /// any HTTP call.
   pub async fn sync_credentials(control_url: &str, mac: &str,
                                 state_path: &std::path::Path)
       -> crate::error::Result<LuksSyncOutcome>;
   ```

   Use `reqwest::Client::new().post(url).json(payload).send().await` (same crate/features `call_flip_api` already relies on). Keep `post_sync` thin — all logic that needs tests lives in Steps 3's pure functions.

5. **Unit tests** — `#[cfg(test)] mod tests` at the bottom (tempdir state files; NO network, NO CockroachDB):

   | Test | Asserts |
   |---|---|
   | `test_normalize_mac` | `"AA:BB:CC:DD:EE:F0"` → `"aa:bb:cc:dd:ee:f0"`; `"aabbcc"`, `""`, `"aa:bb:cc:dd:ee"` → Err |
   | `test_read_missing_state_is_empty` | nonexistent path → `Ok(vec![])` |
   | `test_read_malformed_state_errors` | file containing `"{not json"` → Err naming the path |
   | `test_read_roundtrip` | write 2 records (1 with `revoked_at: Some`), read → identical structs |
   | `test_build_payload_bad_mac_no_io` | invalid mac → Err (fires before any file read — pass a path that does NOT exist and assert the Err is the MAC ConfigError, not a file error) |
   | `test_payload_serializes_roles_lowercase` | serde_json of a payload contains `"role":"backup1"` and the mac lowercase |
   | `test_build_payload_happy` | **anti-over-suppression:** valid mac + 3 records (primary/backup1/backup2) → payload with `records.len()==3` — validation does not drop legitimate records |

6. Bump the file header (version + last-edited) on every file you touch; new files get a fresh 4-line header with a new uuid4 (`uuidgen | tr 'A-F' 'a-f'`). (Here: `crates/uaa-core/src/luks_sync.rs` keeps its CP-01 guid, version bumped.)

## How to test

```bash
cargo test --lib --offline && cargo build --offline
# Expected: all tests pass (baseline 311 + earlier waves' tests + your 7 new tests), build clean
cargo clippy --offline -- -D warnings
# Expected: no warnings
cargo test --lib --offline luks_sync
# Expected: 7 passed; 0 failed
grep -rn "172.16.2.30" crates/uaa-core/src/luks_sync.rs
# Expected: 0 hits (control URL is caller-supplied, never hardcoded)
grep -rn "fs::write\|File::create\|rename" crates/uaa-core/src/luks_sync.rs | grep -v "cfg(test)\|mod tests"
# Expected: 0 hits outside tests (module is read-only w.r.t. local state)
```

## Acceptance criteria

- [ ] Module filled: `grep -n "pub async fn sync_credentials\|pub fn read_local_state\|pub fn build_payload\|pub fn normalize_mac" crates/uaa-core/src/luks_sync.rs` → 4 hits; `grep -n "todo!" crates/uaa-core/src/luks_sync.rs` → 0 hits.
- [ ] Reuse honored: `grep -n "use crate::luks_keys::LuksCredentialRecord" crates/uaa-core/src/luks_sync.rs` → 1 hit (no second credential struct: `grep -c "struct.*Credential" crates/uaa-core/src/luks_sync.rs` → 0 beyond the payload/outcome types).
- [ ] Missing-file vs malformed-file split proven: `test_read_missing_state_is_empty` (Ok empty) and `test_read_malformed_state_errors` (hard Err) both pass.
- [ ] Fail-before-IO proven: `test_build_payload_bad_mac_no_io` passes (MAC ConfigError, not a file error, on a nonexistent path).
- [ ] Read-only module: grep for non-test `fs::write|File::create|rename` in `crates/uaa-core/src/luks_sync.rs` → 0 hits.
- [ ] Anti-over-suppression: `test_build_payload_happy` passes — 3 legitimate records survive validation intact.
- [ ] `cargo test --lib --offline && cargo build --offline` exits 0; clippy clean (`cargo clippy --offline -- -D warnings`).
- [ ] File headers bumped on every changed file (`git diff origin/main --stat` files each show a version bump; guids unchanged).

## Commit message

```
feat(luks): add luks_sync client module reporting FIDO2 credentials to control (ws7-luks)

Fills the CP-01 stub crates/uaa-core/src/luks_sync.rs: reads the LK-01
local state file (missing file => empty report, malformed => hard error,
never partial), wraps records with a validated lowercase MAC, and POSTs
JSON to control's luks_credentials endpoint (CT-02) mirroring the
call_flip_api reqwest idiom. Read-only w.r.t. local state; control URL
caller-supplied. LUKS keyslot bookkeeping only — never auth. 7 unit
tests, no network/CRDB required.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

Additive (stub → fill): if `grep -n "pub async fn sync_credentials" crates/uaa-core/src/luks_sync.rs` hits, the task is already applied — run the Acceptance checks instead of re-applying. Rollback = revert the single commit; the CP-01 stub returns, `luks_keys.rs` and its state-file contract are untouched (this module only reads them), and no registry row or host state exists to unwind — sync only runs when invoked.
