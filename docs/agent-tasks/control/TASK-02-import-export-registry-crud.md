<!-- file: docs/agent-tasks/control/TASK-02-import-export-registry-crud.md -->
<!-- version: 1.0.1 -->
<!-- guid: 6ca924a9-7ef4-4694-841c-280743bfd44e -->
<!-- last-edited: 2026-07-10 -->

# TASK-02 — Fill registry CRUD + `import --from` (insert-if-absent) + `export --to-json` + luks/tang store surface (ws2-control)

**Priority:** P1 · **Effort:** M · **Recommended subagent:** Sonnet-class · rust-service subagent · **Why:** cutover-critical merge semantics pinned by spec Decisions 16/22 — a wrong upsert de-approves live hosts during a rollback-retry cycle. · **Depends on:** TASK-01 (wave-4 gated: CT-01 merged — the `db/registry.rs` and `import_export.rs` stubs must exist on origin/main)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/control-import-export-registry-crud" -b agent/control-import-export-registry-crud origin/main
cd "$REPO/.worktrees/control-import-export-registry-crud"
git rebase origin/main
```

(Protocol also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

## Goal

Fill the two CT-01 stubs you own EXCLUSIVELY — `crates/uaa-control/src/db/registry.rs` and `crates/uaa-control/src/import_export.rs` — with (a) the typed registry store (`RegistryStore` trait: machines/yubikeys/tang_servers/luks_credentials CRUD, a `PgRegistryStore` tokio-postgres impl, and an always-compiled `MemRegistryStore` other control tasks reuse in THEIR tests), (b) `uaa-control import --from <dir>` with the Decision-22 pinned semantics — **`INSERT ... ON CONFLICT (mac) DO NOTHING`; never clobber a CRDB row that is newer than the JSON source** — and (c) `uaa-control export --to-json <dir>` (Decision 16 rollback: re-hydrate the Python-shaped JSON from CRDB).

Purely additive to those two files (plus wiring the two already-stubbed clap subcommands in `main.rs`). Reuse — do not invent parallels:
- **Row types from CT-01** (`crates/uaa-control/src/db/mod.rs` — verify: `grep -n "pub struct MachineRow" crates/uaa-control/src/db/mod.rs` at execution time). Do NOT define new row structs.
- **`write_snapshot` tmp+rename** from CT-01's `db/store.rs` for the snapshot-after-mutation contract. Do NOT write a second atomic-write helper.
- **Python ground truth** `scripts/autoinstall-agent.py` for the JSON shapes (greps below).

## Background (verify before editing)

- Ground-truth JSON files (the import source, `/var/log/cockroach-autoinstall/` at cutover time): `registry.json` = dict keyed by normalized mac → `{hostname, type, status, registered_at, approved_at?, last_seen?, last_ip?, tpm_ek?, ...}`; `yubikey-registry.json` = dict keyed by fingerprint → `{gpg_pubkey, ssh_pubkey, comment, serial, status, registered_at, approved_at?}`; `tang-registry.json` = dict keyed by hostname → `{ip, tang_url, adv_keys, last_seen}`. Timestamps are UNIX INTEGER SECONDS in the JSON, `TIMESTAMPTZ` in CRDB — convert on import, convert BACK to ints on export (round-trip stable).
- Edge semantics (spell twice — here and Step 3): missing `status` in a JSON entry → `pending`; missing `registered_at` → now; unknown extra JSON keys → ignored (log at debug, never error); a JSON file absent from the dir → skipped with a loud warning, the other files still import; empty dict → 0 inserts, exit 0.
- Insert-if-absent is per PRIMARY KEY: machines `ON CONFLICT (mac) DO NOTHING`, yubikeys `ON CONFLICT (fingerprint) DO NOTHING`, tang `ON CONFLICT (hostname) DO NOTHING`. NEVER `DO UPDATE` — both judges showed all-column upserts de-approve live hosts and null bound TPM EKs on rollback-retry (spec Decision 22).
- **Cargo tests must NOT require live CockroachDB**: `PgRegistryStore`'s SQL strings are `pub(crate) const` and asserted textually; behavioral tests run against `MemRegistryStore` + tempdir JSON fixtures.
- Export is server-local, human-run; it OVERWRITES the target JSON files (that is its purpose — rollback re-hydration) via tmp+rename.

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
  grep -n "^REGISTRY_FILE\|^YUBIKEY_REGISTRY_FILE\|^TANG_REGISTRY_FILE" scripts/autoinstall-agent.py  # expect: 3 hits (lines ~37-39, the three JSON paths)
  grep -n "def normalize_mac" scripts/autoinstall-agent.py            # expect: 1 hit (~line 73; mirror this normalization on import)
  grep -n "os.replace(tmp" scripts/autoinstall-agent.py               # expect: 3 hits (tmp+rename idiom the export mirrors)
  grep -n "ON CONFLICT (mac) DO NOTHING" docs/specs/constellation-design.md  # expect: 1 hit (Decision 22, normative)
  grep -n "export --to-json" docs/specs/constellation-design.md       # expect: 2+ hits (Decision 16 rollback)
  test -f crates/uaa-control/src/db/registry.rs && echo OK            # expect: OK (wave gate: CT-01 merged; missing = STOP, too early)
  grep -n "import\|export\|todo!" crates/uaa-control/src/main.rs      # WAVE GATE (file exists only after CT-01 merges): expect hits on the two stub CLI arms this task wires; missing/zero hits = upstream not merged, STOP and report
  grep -n "fn write_snapshot" crates/uaa-control/src/db/store.rs      # WAVE GATE (file exists only after CT-01 merges): expect 1 hit — the tmp+rename idiom the export copies; missing/zero hits = upstream not merged, STOP and report
  ```

## Step-by-step

1. Run the ⛔ START HERE block, then every anchor grep above. Any zero-hit / missing-file result → STOP and report.

2. **`db/registry.rs` — the store.**
   - `pub trait RegistryStore: Send + Sync` with: `get_machine(mac)`, `list_machines()`, `insert_machine_if_absent(MachineRow) -> Result<bool>` (true = inserted, false = pre-existing row untouched), `update_machine_status(mac, status, approved_at)`, `touch_last_seen(mac, ip)`, `set_boot_target(mac, BootTarget)`, `list_yubikeys()`, `insert_yubikey_if_absent(YubikeyRow) -> Result<bool>`, `upsert_tang_server(TangServerRow)` (tang IS a last-seen upsert — checkin semantics, matches the Python), `insert_tang_if_absent(TangServerRow) -> Result<bool>` (import path only), `insert_luks_credential(LuksCredentialRow)`, `list_luks_credentials(mac)`, `revoke_luks_credential(id)`.
   - `pub struct PgRegistryStore` implementing it over tokio-postgres; every SQL string a `pub(crate) const` (e.g. `SQL_INSERT_MACHINE_IF_ABSENT` ending in `ON CONFLICT (mac) DO NOTHING`).
   - `pub struct MemRegistryStore` (HashMaps behind a Mutex) implementing the same trait — always compiled (doc: "test/degraded support; used by sibling task tests"), insert-if-absent returns false and CHANGES NOTHING when the key exists.
3. **`import_export.rs` — import.** `pub async fn import_from(dir: &Path, store: &dyn RegistryStore) -> Result<ImportReport>`: read the three JSON files by their ground-truth names (`registry.json`, `yubikey-registry.json`, `tang-registry.json`); normalize MACs exactly like the Python `normalize_mac` (lowercase, `-`/`.` → `:`); unix ints → timestamps; missing `status` → `pending`, missing `registered_at` → now, unknown keys ignored (debug log); call the `*_if_absent` methods ONLY — a pre-existing row is counted `skipped`, never touched. `ImportReport { inserted: {machines, yubikeys, tang}, skipped: {…}, files_missing: Vec<String> }`; absent file → warn + continue; malformed JSON file → hard error naming the file (fail-closed: a half-parsed registry must not half-import).
4. **`import_export.rs` — export.** `pub async fn export_to_json(dir: &Path, store: &dyn RegistryStore) -> Result<ExportReport>`: write the three files in the exact Python shape (dict keyed by mac/fingerprint/hostname, timestamps back to unix ints, `None` fields omitted), each via tmp+rename (mirror CT-01's `write_snapshot` idiom), `indent=2`-equivalent pretty JSON. Round-trip law (tested): `import(export(state))` inserts 0.
5. **Wire the CLI**: replace the two `main.rs` stub arms — `uaa-control import --from <dir>` and `uaa-control export --to-json <dir>` — to construct a `PgRegistryStore` from config and call the fns above, printing the report; keep the arms thin (logic stays in `import_export.rs`).
6. **Unit tests** (tempdir fixtures, `MemRegistryStore`, no network): `test_import_inserts_fresh_rows` (3-file fixture → counts match), `test_import_is_insert_if_absent` (pre-seed `MemRegistryStore` with mac X `status=approved`, fixture says X `status=pending` → after import X is STILL approved, skipped=1 — the Decision-22 no-clobber law), `test_import_twice_idempotent` (second run inserts 0), `test_import_missing_file_warns_and_continues`, `test_import_malformed_json_fails_closed` (nothing inserted from the bad file), `test_import_normalizes_macs` (`AA-BB-CC-DD-EE-FF` key → `aa:bb:cc:dd:ee:ff` row), `test_export_python_shape` (dict-keyed, unix-int timestamps, no nulls), `test_export_import_round_trip_inserts_zero`, `test_sql_const_pins_on_conflict` (`SQL_INSERT_MACHINE_IF_ABSENT` contains `ON CONFLICT (mac) DO NOTHING` and contains NO `DO UPDATE`), and the anti-over-suppression test `test_import_absent_rows_actually_insert` (a fixture row whose mac is NOT pre-seeded really lands, fields intact — the no-clobber guard doesn't block fresh inserts).
7. Bump the file header (version + last-edited) on every file you touch; new files get a fresh 4-line header with a new uuid4 (`uuidgen | tr 'A-F' 'a-f'`).

## How to test

```bash
cargo test --lib --offline && cargo build --offline
# Expected: all tests pass (baseline 311 + prior control tests + the ~10 new tests), build clean
cargo clippy --offline -- -D warnings
# Expected: no warnings
grep -rn "DO UPDATE" crates/uaa-control/src/db/registry.rs crates/uaa-control/src/import_export.rs
# Expected: 0 hits (insert-if-absent only — Decision 22)
grep -n "ON CONFLICT (mac) DO NOTHING" crates/uaa-control/src/db/registry.rs
# Expected: 1+ hits (the pinned SQL const)
```

## Acceptance criteria

- [ ] Only the assigned files (+ the two `main.rs` CLI arms) changed: `git diff origin/main --stat` shows `db/registry.rs`, `import_export.rs`, `main.rs`, nothing else under `crates/uaa-control/src/`.
- [ ] No-clobber law proven: `test_import_is_insert_if_absent` passes (pre-existing approved row survives an import that says pending); `grep -rn "DO UPDATE" crates/uaa-control/src/db/registry.rs crates/uaa-control/src/import_export.rs` → 0 hits.
- [ ] Idempotency law proven: `test_import_twice_idempotent` and `test_export_import_round_trip_inserts_zero` pass.
- [ ] Python-shape fidelity: `test_export_python_shape` passes (dict-keyed, unix-int timestamps); `test_import_normalizes_macs` passes.
- [ ] **Anti-over-suppression:** `test_import_absent_rows_actually_insert` passes — fresh rows still insert through the no-clobber guard.
- [ ] `MemRegistryStore` is exported for sibling tests: `grep -n "pub struct MemRegistryStore" crates/uaa-control/src/db/registry.rs` → 1 hit.
- [ ] `cargo test --lib --offline && cargo build --offline` exits 0; clippy clean; no test opens a network connection.
- [ ] File headers bumped on every changed file (`git diff origin/main --stat` files each show a version bump; guids unchanged).

## Commit message

```
feat(control): registry CRUD store + import --from / export --to-json with pinned insert-if-absent semantics (ws2-control)

Fills the CT-01 stubs db/registry.rs (RegistryStore trait, PgRegistryStore
with const SQL pinning ON CONFLICT (mac|fingerprint|hostname) DO NOTHING,
MemRegistryStore for sibling tests) and import_export.rs (Decision-22 import:
never clobbers a newer CRDB row, mac normalization + unix-int timestamp
conversion mirroring autoinstall-agent.py; Decision-16 export: Python-shaped
JSON re-hydration via tmp+rename; round-trip imports zero). CLI arms wired.
All tests on MemRegistryStore + tempdir fixtures — no live CockroachDB.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

If `grep -n "pub trait RegistryStore" crates/uaa-control/src/db/registry.rs` hits, already applied — run the acceptance checks instead of re-applying. Rollback = revert the single commit; the two files return to CT-01's header-only stubs and the `main.rs` arms return to their not-yet-implemented messages; no data or server state exists to unwind.
