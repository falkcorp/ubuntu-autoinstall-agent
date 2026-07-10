<!-- file: docs/agent-tasks/control/TASK-01-control-crate-registry.md -->
<!-- version: 1.0.0 -->
<!-- guid: 7f72816c-8da6-4d0a-93e5-5e5989520be1 -->
<!-- last-edited: 2026-07-10 -->

# TASK-01 — Create the uaa-control crate: listeners + socket activation, embedded CRDB migrations, snapshot+WAL degraded mode, follower stubs (ws2-control)

**Priority:** P1 · **Effort:** L · **Recommended subagent:** Opus-class · rust-service subagent · **Why:** registry system-of-record data-integrity semantics (WAL event_id dedup, snapshot atomicity, fail-closed matrix) — this is the irreversible-stakes foundation every other control/install-plane/pki task builds on. · **Depends on:** core-proto CP-02 (global wave-3 gated: waves 1–2 merged — CP-01 workspace conversion + CP-02 uaa-proto/workspace-deps — and this worktree rebased onto that state)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/control-control-crate-registry" -b agent/control-control-crate-registry origin/main
cd "$REPO/.worktrees/control-control-crate-registry"
git rebase origin/main
```

(Protocol also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

## Goal

Create the NEW crate `crates/uaa-control` (spec component C3, topology row `uaa-control`): the central daemon that owns the registry (CockroachDB system-of-record per spec Decision 4), the four listeners (:25000 legacy machine plane via **systemd socket activation** per Decision 24, plus :7443 gRPC mTLS, :7444 enrollment JSON, :8443 operator — the three TLS listeners land as bind-and-health scaffolds here, routes filled by followers), the embedded schema migrations (the FULL spec SQL, verbatim, below), the snapshot+WAL degraded-mode layer (Decision 4 repairs a/b/c), and — critically — the complete set of **stub module files that follower tasks fill EXCLUSIVELY** (the de-collision pattern: exactly one filling task per stub file).

Purely additive: `crates/uaa-control` is a new directory; the root `Cargo.toml` uses `members = ["crates/*"]` (CP-01) so NO existing file changes. All deps come from `[workspace.dependencies]` (populated by CP-02) via `workspace = true` — if a dep you need is missing from the workspace table, STOP and report (do not edit the root Cargo.toml; that is a collision).

Reuse — do not invent parallels:
- **`uuid` crate (v4)** for WAL `event_id` minting (workspace dep — verify: `grep -n "^uuid" Cargo.toml`). Do NOT hand-roll UUIDs.
- **tmp+rename atomic-write idiom** exactly as the Python ground truth does it (`save_registry` in `scripts/autoinstall-agent.py` — verify: `grep -n "os.replace(tmp" scripts/autoinstall-agent.py`). Do NOT write in place.
- **`tokio-postgres` + rustls** for CRDB (spec Decision 5); **NEVER sqlx**.

## Background (verify before editing)

- Spec: `docs/specs/constellation-design.md` — Decisions 4 (CRDB + degraded mode), 5 (tokio-postgres), 24 (socket activation), C3 component section, and the normative `CREATE TABLE` schema block. The schema below is copied verbatim from the spec; the spec wins if they ever diverge.
- CRDB itself already runs on the fleet; database/user creation is Bucket-3 (human). **Cargo tests must NOT require a live CockroachDB** — everything DB-shaped sits behind traits with in-memory mocks; `PgHealth`/real connections are constructed only at runtime.
- Degraded-mode semantics (spec Decision 4, spelled here and again in Step 5): detection = 2s connect timeout / 5s query timeout; reads served from `/var/lib/uaa/registry-snapshot.json`; mutations fail CLOSED (typed error → HTTP 503) EXCEPT telemetry ingestion (webhook/checkin/install events) which appends to `/var/lib/uaa/wal.jsonl`; every WAL entry carries an `event_id` UUID minted at ingest; replay = `INSERT ... ON CONFLICT (event_id) DO NOTHING`, entry marked consumed ONLY after its CRDB txn commits; WAL-wins over snapshot (strictly newer); total quorum loss explicitly OUT of scope (Non-goals).
- Edge semantics: missing snapshot file in degraded mode → serve an EMPTY registry + loud `tracing::error!` (never panic); corrupt WAL line → copy to `wal.quarantine.jsonl`, skip it, keep replaying the rest (never abandon the tail); crash between commit and consumed-mark → safe, dedup makes re-replay a no-op.
- `main.rs` stays thin; all logic in `src/lib.rs` + modules so `cargo test --lib --offline` exercises it.

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
  grep -n "CREATE TABLE machines" docs/specs/constellation-design.md          # expect: 1 hit (~line 279; schema block is normative)
  grep -n "ON CONFLICT (event_id) DO NOTHING" docs/specs/constellation-design.md  # expect: 2 hits (~lines 93, 356)
  grep -n "registry-snapshot.json" docs/specs/constellation-design.md         # expect: 2 hits (~lines 89, 352)
  grep -n "socket activation" docs/specs/constellation-design.md              # expect: 3+ hits (Decision 24, topology row)
  grep -n "^uuid" Cargo.toml                                                  # expect: 1 hit (workspace dep for event_id)
  grep -n "os.replace(tmp" scripts/autoinstall-agent.py                       # expect: 3 hits (the tmp+rename idiom to mirror)
  test -d crates/uaa-proto && echo OK                                         # expect: OK (wave gate: CP-02 merged; absent = STOP, you are too early)
  ```

## Step-by-step

1. Run the ⛔ START HERE block, then every anchor grep above. Any zero-hit / missing-dir result → STOP and report.

2. **Crate skeleton.** Create `crates/uaa-control/Cargo.toml` (`name = "uaa-control"`, `[lib]` + `[[bin]] name = "uaa-control"`), deps ALL `workspace = true` (tokio, axum, tonic, prost, tower-http, rustls, tokio-rustls, tokio-postgres, serde, serde_json, uuid, tracing, clap, sha2, uaa-proto, uaa-core). `src/main.rs` = thin clap entry (subcommands `serve` (default), plus stubs `import`, `export`, `audit` that print "not yet implemented — see control TASK-02/TASK-04" and exit 1). `src/lib.rs` declares every module below. Every new file gets the mandatory 4-line `// file:/version:/guid:/last-edited:` header with a fresh uuid4.

3. **Embedded migrations.** `crates/uaa-control/migrations/0001_init.sql` containing EXACTLY this schema (verbatim from the spec data-model section — 10 `CREATE TABLE` statements; each wrapped `CREATE TABLE IF NOT EXISTS` is FORBIDDEN, use plain `CREATE TABLE`; versioning is handled by the migrations table):

   ```sql
   CREATE TABLE machines (
     mac            STRING PRIMARY KEY,            -- normalized aa:bb:cc:dd:ee:ff
     hostname       STRING NOT NULL,
     ip             STRING,
     type           STRING NOT NULL DEFAULT 'lenovo',
     status         STRING NOT NULL DEFAULT 'pending',  -- pending|approved|revoked
     boot_target    STRING NOT NULL DEFAULT 'local-disk',
                    -- authoritative next-boot intent (Decision 13):
                    -- local-disk|custom-autoinstall|pxe-disabled|pxe-grub
     tpm_ek         STRING,                        -- sha256 of TPM EK pub, bound at first checkin
     registered_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
     approved_at    TIMESTAMPTZ,
     last_seen      TIMESTAMPTZ,
     last_ip        STRING,
     installed_at   TIMESTAMPTZ,                   -- parity: persist install completion
     last_install_status STRING,                   -- success|failed|in-progress
     updated_at     TIMESTAMPTZ NOT NULL DEFAULT now()
   );
   CREATE TABLE install_history (
     event_id UUID PRIMARY KEY,                    -- minted at INGEST (WAL-replay dedup key)
     mac STRING NOT NULL REFERENCES machines (mac),
     started_at TIMESTAMPTZ, finished_at TIMESTAMPTZ,
     status STRING NOT NULL, detail JSONB
   );
   CREATE TABLE enrollments (
     spki_fingerprint STRING PRIMARY KEY,           -- sha256 of CSR public key
     mac STRING REFERENCES machines (mac),
     csr_pem STRING NOT NULL,
     state STRING NOT NULL DEFAULT 'pending',       -- pending|approved|issued|rejected|revoked|superseded
     cert_pem STRING, requested_at TIMESTAMPTZ NOT NULL DEFAULT now(), decided_by STRING
   );
   CREATE TABLE yubikeys (                          -- extends today's GPG/SSH registry
     fingerprint STRING PRIMARY KEY, gpg_pubkey STRING, ssh_pubkey STRING,
     comment STRING, serial STRING, status STRING NOT NULL DEFAULT 'pending',
     registered_at TIMESTAMPTZ NOT NULL DEFAULT now()
   );
   CREATE TABLE luks_credentials (                  -- NEW: FIDO2 keyslot tracking
     id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
     mac STRING NOT NULL REFERENCES machines (mac),
     yubikey_serial STRING NOT NULL,
     role STRING NOT NULL,                          -- primary|backup1|backup2
     luks_keyslot INT, enrolled_at TIMESTAMPTZ, revoked_at TIMESTAMPTZ
   );
   CREATE TABLE tang_servers (
     hostname STRING PRIMARY KEY, ip STRING, tang_url STRING,
     adv_keys JSONB, last_seen TIMESTAMPTZ
   );
   CREATE TABLE discovered_macs (                   -- uaa-pxe inbox
     mac STRING PRIMARY KEY, first_seen TIMESTAMPTZ, last_seen TIMESTAMPTZ,
     arch_hint STRING, vendor_class STRING, dismissed BOOL NOT NULL DEFAULT false
   );
   CREATE TABLE audit_events (
     seq INT8 PRIMARY KEY DEFAULT unique_rowid(),
     at TIMESTAMPTZ NOT NULL DEFAULT now(),
     actor STRING NOT NULL, role STRING NOT NULL,   -- github login / 'system'
     action STRING NOT NULL, target STRING, outcome STRING NOT NULL,
     detail JSONB, prev_hash BYTES NOT NULL, hash BYTES NOT NULL
     -- append serialized via SELECT tip FOR UPDATE in the recording txn (Decision 21);
     -- genesis prev_hash = 32 zero bytes
   );
   CREATE TABLE audit_checkpoints (
     day DATE PRIMARY KEY, tip_seq INT8 NOT NULL, tip_hash BYTES NOT NULL,
     signature BYTES NOT NULL                       -- ed25519, on-server audit key
   );
   CREATE TABLE saga_log (
     saga_id UUID PRIMARY KEY, kind STRING NOT NULL,
     state STRING NOT NULL,  -- running|done|compensating|compensated|compensation_pending
     steps JSONB NOT NULL, started_at TIMESTAMPTZ, finished_at TIMESTAMPTZ
   );
   ```

   `src/db/migrations.rs`: `include_str!` the file, `pub fn migration_sql() -> &'static str`, and `pub async fn apply(client) -> Result<()>` that creates a `schema_migrations (version INT8 PRIMARY KEY, applied_at TIMESTAMPTZ)` table and applies 0001 iff absent (runtime-only path; unit tests assert the SQL TEXT, never connect).

4. **Shared row types.** `src/db/mod.rs`: serde structs mirroring every table (`MachineRow`, `InstallEvent`, `EnrollmentRow`, `YubikeyRow`, `LuksCredentialRow`, `TangServerRow`, `DiscoveredMacRow`, `AuditEventRow`, `AuditCheckpointRow`, `SagaRow`) — pre-declared HERE so no two follower tasks ever add the same type. `status`/`state`/`boot_target` are typed enums with `#[serde(rename_all = "kebab-case")]` where the spec uses kebab values; unknown incoming strings deserialize to a spelled-out `Unknown(String)` variant (never a hard error — parity data is dirty).

5. **Degraded-mode layer.** `src/db/store.rs`:
   - `pub trait DbHealth: Send + Sync { async fn healthy(&self) -> bool; }` — real impl wraps tokio-postgres with the 2s connect / 5s query timeouts; tests use `MockHealth(bool)`.
   - `pub struct StatePaths { pub snapshot: PathBuf, pub wal: PathBuf, pub wal_consumed: PathBuf, pub quarantine: PathBuf }` with `Default` = `/var/lib/uaa/{registry-snapshot.json,wal.jsonl,wal.consumed,wal.quarantine.jsonl}`; ALWAYS constructed from config so tests point at a tempdir.
   - `pub fn write_snapshot(paths, &SnapshotDoc) -> Result<()>` — serialize to `<snapshot>.tmp`, set mode 0600, `std::fs::rename` (atomic). Called after EVERY successful mutation (followers call it; the contract is documented on the fn).
   - `pub fn read_snapshot(paths) -> SnapshotDoc` — missing/corrupt file → `SnapshotDoc::default()` (empty) + `tracing::error!` (degraded reads must never panic).
   - `pub struct WalEntry { pub event_id: uuid::Uuid, pub kind: String, pub payload: serde_json::Value, pub at: String }`; `pub fn wal_append(paths, kind, payload) -> Result<uuid::Uuid>` — mints `event_id` at ingest, appends one JSON line, 0600.
   - `pub async fn wal_replay(paths, apply: &mut dyn WalApply) -> Result<ReplayReport>` where `pub trait WalApply { async fn apply(&mut self, &WalEntry) -> Result<()>; }` — the real impl runs `INSERT ... ON CONFLICT (event_id) DO NOTHING` in a txn; an entry is recorded in `wal.consumed` ONLY after `apply` returns Ok (commit). Corrupt line → append raw line to quarantine, continue. Repeat: dedup is by `event_id`; re-running replay after a crash re-applies nothing.
   - `pub enum StoreError { Degraded, ... }` — `Degraded` is the fail-closed mutation error the HTTP layers map to 503.

6. **Listeners + socket activation.** `src/listeners.rs`:
   - `pub fn sd_listen_fd() -> Option<std::os::unix::io::RawFd>` — returns fd 3 iff `LISTEN_PID` == this pid AND `LISTEN_FDS` >= 1 (manual sd_listen_fds; NO new crate). Unit-test the env parsing via an injectable `(pid, envs)` inner fn `parse_listen_fds(pid: u32, listen_pid: Option<&str>, listen_fds: Option<&str>) -> Option<RawFd>`.
   - `serve` wires: :25000 axum router from `machine_plane::router()` on the activated fd when present, else a plain bind (dev fallback, port from config); :7443/:7444/:8443 scaffolds each serving only `GET /healthz` → `200 {"service":"uaa-control","listener":"<name>"}` (TLS wiring arrives with PK-03/CT-07; bind plain for now, port 0 in tests).
   - Ship unit files `crates/uaa-control/systemd/uaa-control.socket` (`ListenStream=25000`) + `uaa-control.service` (docs artifacts; deploy is Bucket-3 human work — say so in a comment).

7. **Follower stubs** — create each file with ONLY its 4-line header, a module doc-comment naming its filling task, and (where a router is needed) an empty `pub fn router() -> axum::Router` returning routes for nothing. This table is normative; each stub has EXACTLY ONE filler:

   | Stub file (under `crates/uaa-control/src/`) | Filled exclusively by |
   |---|---|
   | `db/registry.rs` | control TASK-02 (CT-02) |
   | `import_export.rs` | control TASK-02 (CT-02) |
   | `auth.rs` | control TASK-03 (CT-03) |
   | `audit.rs` | control TASK-04 (CT-04) |
   | `saga.rs` | control TASK-05 (CT-05) |
   | `reinstall.rs` | control TASK-06 (CT-06) |
   | `operator/mod.rs`, `operator/handlers.rs`, `operator/api_types.rs` | control TASK-07 (CT-07) |
   | `ca.rs` | pki PK-01, then PK-03 (serialized — collision row) |
   | `enroll.rs` | pki PK-01 |
   | `machine_plane/seeds.rs` | install-plane IP-01 |
   | `machine_plane/lifecycle.rs` | install-plane IP-02 |
   | `machine_plane/inventory.rs` | install-plane IP-03 |
   | `machine_plane/dashboard.rs` | install-plane IP-04 |

   `machine_plane/mod.rs` is CT-01-owned (not a stub): it declares the four submodules and a `pub fn router()` that today serves only `/healthz` and will `merge()` the submodule routers as they land (each filler adds ONE `merge` line — disjoint edits documented in the mod doc-comment).

8. **Unit tests** (in-module `#[cfg(test)]`, tempdir-backed, NO network, NO CockroachDB):
   `test_migration_sql_has_all_ten_tables` (10 `CREATE TABLE` occurrences + the exact table names), `test_migration_sql_wal_dedup_comment` (contains `unique_rowid` and `prev_hash`), `test_snapshot_write_is_atomic_and_0600` (no `.tmp` left behind; perms 0o600), `test_snapshot_missing_reads_empty` (no panic, default doc), `test_wal_append_mints_event_id` (line parses back, uuid parses), `test_wal_replay_dedup` (replay twice against a recording `MockWalApply` → each event_id applied ONCE; consumed-mark written only after Ok), `test_wal_replay_quarantines_corrupt_line` (1 bad line among 3 → 2 applied, 1 quarantined), `test_wal_apply_failure_not_marked_consumed` (Err from apply → entry NOT in consumed; retry re-delivers it), `test_parse_listen_fds` (match/mismatch pid, 0 fds, missing envs), `test_mutation_degraded_fails_closed` (`MockHealth(false)` → `StoreError::Degraded`, snapshot untouched), and the anti-over-suppression test `test_mutation_healthy_passes_and_snapshots` (`MockHealth(true)` → mutation closure runs, snapshot rewritten, WAL untouched).

9. Bump the file header (version + last-edited) on every file you touch; new files get a fresh 4-line header with a new uuid4 (`uuidgen | tr 'A-F' 'a-f'`).

## How to test

```bash
cargo test --lib --offline && cargo build --offline
# Expected: all tests pass (baseline 311 + the ~11 new uaa-control tests), build clean
cargo clippy --offline -- -D warnings
# Expected: no warnings
grep -c "CREATE TABLE" crates/uaa-control/migrations/0001_init.sql
# Expected: 10
for f in db/registry.rs import_export.rs auth.rs audit.rs saga.rs reinstall.rs ca.rs enroll.rs operator/mod.rs machine_plane/seeds.rs machine_plane/lifecycle.rs machine_plane/inventory.rs machine_plane/dashboard.rs; do test -f "crates/uaa-control/src/$f" || echo "MISSING $f"; done
# Expected: no output (all stubs present)
grep -rn "sqlx" crates/uaa-control/
# Expected: 0 hits (Decision 5: tokio-postgres only)
grep -rn "cockroach\|26257" crates/uaa-control/src/ | grep -v "//"
# Expected: 0 hits outside comments (no live-DB coordinates baked into tests)
```

## Acceptance criteria

- [ ] Crate + workspace-glob pickup: `cargo build --offline` builds `uaa-control` with NO edit to the root `Cargo.toml` (`git diff origin/main -- Cargo.toml` is empty).
- [ ] Schema verbatim: `grep -c "CREATE TABLE" crates/uaa-control/migrations/0001_init.sql` → 10; `grep -n "boot_target    STRING NOT NULL DEFAULT 'local-disk'" crates/uaa-control/migrations/0001_init.sql` → 1 hit.
- [ ] All 13 stub files from the Step-7 table exist, each naming its filler task: `grep -rln "Filled exclusively by\|filled exclusively" crates/uaa-control/src/ | wc -l` → ≥13.
- [ ] WAL dedup proven: `grep -n "test_wal_replay_dedup\|test_wal_apply_failure_not_marked_consumed" crates/uaa-control/src/db/store.rs` → 2 hits, both pass in the suite.
- [ ] Snapshot atomicity proven: `test_snapshot_write_is_atomic_and_0600` passes (0600 + no `.tmp` residue).
- [ ] Socket activation parse: `grep -n "fn parse_listen_fds" crates/uaa-control/src/listeners.rs` → 1 hit + its test passes; unit files exist (`ls crates/uaa-control/systemd/uaa-control.socket crates/uaa-control/systemd/uaa-control.service`).
- [ ] Fail-closed matrix: `test_mutation_degraded_fails_closed` passes.
- [ ] **Anti-over-suppression:** `test_mutation_healthy_passes_and_snapshots` passes — a healthy store still mutates + snapshots (the degraded guard does not block the happy path).
- [ ] No live-DB requirement: `cargo test --lib --offline` passes on a machine with no CockroachDB and no network.
- [ ] `cargo test --lib --offline && cargo build --offline` exits 0; clippy clean.
- [ ] File headers bumped on every changed file (`git diff origin/main --stat` files each show a version bump; guids unchanged; new files have fresh uuid4 headers).

## Commit message

```
feat(control): scaffold uaa-control crate — listeners, socket activation, embedded CRDB schema, snapshot+WAL degraded mode, follower stubs (ws2-control)

New crates/uaa-control: thin main + lib, 0001_init.sql embedding the full
normative spec schema (10 tables), sd_listen_fds socket activation for the
:25000 legacy plane (Decision 24) with dev fallback, health scaffolds for
:7443/:7444/:8443, and the Decision-4 degraded layer: 0600 tmp+rename
registry snapshot, wal.jsonl with ingest-minted event_id UUIDs,
ON CONFLICT (event_id) DO NOTHING replay marked consumed only after commit,
quarantine for corrupt lines, mutations fail-closed (503) when unhealthy.
13 stub modules pre-declared, one exclusive filler each (CT-02..07, PK-01/03,
IP-01..04). No live CockroachDB in tests — trait seams + tempdir throughout.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

If `test -d crates/uaa-control && grep -n "fn parse_listen_fds" crates/uaa-control/src/listeners.rs` hits, already applied — run the acceptance checks instead of re-applying. Rollback = revert the single commit; it removes the whole `crates/uaa-control/` directory cleanly (the workspace glob means nothing else references it), no DB, server, or config state exists to unwind.
