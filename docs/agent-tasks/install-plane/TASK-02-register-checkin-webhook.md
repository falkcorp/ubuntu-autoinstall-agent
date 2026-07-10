<!-- file: docs/agent-tasks/install-plane/TASK-02-register-checkin-webhook.md -->
<!-- version: 1.0.1 -->
<!-- guid: 6e2b6562-5dda-44da-9e63-1c52ab0ebca7 -->
<!-- last-edited: 2026-07-10 -->

# TASK-02 — register/checkin/webhook parity: TPM-EK first-bind + mismatch-403, auto-flip-on-success, events + install_history (ws3-parity)

**Priority:** P1 · **Effort:** L · **Recommended subagent:** Sonnet-class · rust-http subagent · **Why:** anti-spoof binding + flip tuple semantics must match Python exactly · **Depends on:** none within this workstream (wave-4 gated: `control/TASK-01` (CT-01) MERGED — it creates `crates/uaa-control` incl. the `machine_plane/lifecycle.rs` stub, the Registry trait, and the WAL/snapshot layer this task calls)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/install-plane-register-checkin-webhook" -b agent/install-plane-register-checkin-webhook origin/main
cd "$REPO/.worktrees/install-plane-register-checkin-webhook"
git rebase origin/main
```

(Protocol also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

Wave gate check: `test -f crates/uaa-control/src/machine_plane/lifecycle.rs` — missing = wave-4 gate NOT satisfied; STOP and report.

## Goal

Fill `crates/uaa-control/src/machine_plane/lifecycle.rs` (CT-01 stub — exactly-one-filler rule) with parity handlers for the lifecycle POST endpoints of `scripts/autoinstall-agent.py` (spec Decision 12: parity lives in uaa-control; Decision 4: telemetry ingestion is fail-OPEN under CRDB loss via the WAL). Every response is JSON via the CT-01 `send_json` equivalent; invalid JSON body → `400 {"error": "invalid json"}` (Python `:564-568`).

Per-endpoint parity table (transcribed from `scripts/autoinstall-agent.py`, anchors below):

| Method | Path | Python | Statuses | Body / semantics |
|---|---|---|---|---|
| POST | `/api/register` | `:571-596` | 400, 200 | missing mac or hostname → `400 {"ok":false,"error":"mac and hostname required"}`; else upsert keyed on normalized MAC — PRESERVE existing `status` (default `pending`), `registered_at` (default now-epoch), `tpm_ek` (default null); overwrite hostname/ip/type (type default `"lenovo"`); `200 {"ok":true,"status":<status>,"message":"Registered. Approve with: curl http://172.16.2.30:25000/api/approve/<mac>"}` |
| POST | `/api/checkin` | `:599-621` | 403, 200 | unknown MAC → `403 {"ok":false,"error":"Not registered"}`; `tpm_ek` present + registry EK empty → FIRST-BIND (persist EK); `tpm_ek` present + registry EK differs → `403 {"ok":false,"error":"TPM mismatch - MAC may be spoofed"}` with NO last_seen update; else update `last_seen` (now-epoch) + `last_ip`; `200 {"ok":true,"status":<s>,"approved":<s=="approved">}` |
| POST | `/api/webhook` | `:624-650` | 200 | append event to events log FIRST; if flip predicate (below) → attempt iPXE flip; a missing iPXE file or ANY flip error is logged and SWALLOWED (`:633-635`) — response is `200 {"ok":true}` regardless; decode `files[]` base64 to the files dir (per-file failures logged + swallowed) |
| POST | `/api/finalreport` | `:652-656` | 200 | log event, `200 {"ok":true}` |
| POST | `/api/hardware-info` | `:652-656` | 200 | same |
| POST | `/api/cloud-init` | `:652-656` | 200 | same |

Flip predicate = exact port of `webhook_should_flip` (`:154-168`): `name` non-empty AND `status ∈ {"finished","complete","success"}` (the WIDENED tuple, `:164`); when `event_type == "status_update"` (the Rust installer's shape) it flips ONLY on `status == "success"` OR `progress == 100`. Anything else (incl. `status="running"`, `"failed"`) never flips.

install_history persist (constellation addition, spec CRDB schema): a webhook whose predicate fires (final success) also records an install event through the CT-01 Registry trait — `event_id` UUID minted at ingest (WAL-replay dedup key, Decision 4a), `mac`/`name`, `status`, `finished_at`, and updates `machines.installed_at` + `last_install_status`. Under CRDB degradation this append goes to the WAL, never 503s (telemetry is fail-open).

REUSE — do not invent parallels:
- CT-01 Registry trait + WAL layer (`crates/uaa-control/src/` — grep at execution time: `grep -rn "trait.*Registry\|wal" crates/uaa-control/src/ | head -5`). Do NOT open a DB connection in handlers; all persistence through the trait so tests mock it.
- The iPXE flip helper: if IP-03 has not merged yet, implement `flip_ipxe` locally in `lifecycle.rs` mirroring Python `:170-177` (`re.sub(r"set menu-default \S+", ...)`, missing file → `(false, "No iPXE file found for <hostname>")`) — coordinate note: IP-03 also mirrors the flip regex for `/api/flip`; both live in machine_plane and the coordinator deduplicates in review if both land helpers (acceptable; files are disjoint).

## Background (verify before editing)

- Design spec: `docs/specs/constellation-design.md` Decision 12 (webhook flip tolerance for missing iPXE files — USB-only hosts — is normative), Decision 4 (WAL `event_id` dedup, fail-open telemetry).
- `shared_state` (skeleton): the Rust webhook SENDER posts `event_type: "status_update"` with `status=success` / `progress=100` (`src/network/ssh_installer/status.rs` — verify grep below). Your predicate must accept exactly that payload; breaking it strands every in-flight installer.
- Edge semantics spelled twice (here + tests): TPM mismatch does NOT update `last_seen`/`last_ip` (Python returns before those lines); a checkin WITHOUT `tpm_ek` never binds and never 403s on EK grounds; register of an EXISTING mac keeps its `approved` status (re-register must not de-approve); events log entries get `received_at` = now-epoch prepended (`:257-259`).
- MAC normalization: lowercase, `-`→`:`, `.`→`:` (Python `:72-73`). Apply on register AND checkin so `AA-BB-...` and `aa:bb:...` are one machine.

**HARD RULES (non-negotiable):**
- NO hardware actions. Validate ONLY in-repo (`cargo`) and, where a brief says so, the QEMU+swtpm harness (`scripts/vm-validate.sh`). Code that COULD touch hardware is written and unit-tested against mock executors only.
- NEVER wipe, write to, or deploy on 172.16.2.30 ("the server") or len-serv-003.
- `disk_device` is read from the live target at runtime, never guessed or hardcoded.
- ipmitool runs via `ssh 172.16.2.30`, never on macOS.
- NEVER power on unimatrixone (U1).
- No real secret in any file: `REPLACE_AT_PLACE_TIME` placeholders stay placeholders.
- Stay inside your worktree; never `git push`, `gh pr`, or merge — report done and stop.

**Path map:** after CP-01 (wave 1) merges, `src/**` lives at `crates/uaa-core/src/**` and the CLI at `crates/uaa/src/**`. The greps below cite pre-move paths (verifiable on today's main); at execution time run them at the old path, then the mapped path. Zero hits at BOTH = STOP and report.

- **Re-verify these anchors before editing** — line numbers drift; zero hits at both old and mapped path = STOP and report:
  ```bash
  grep -n "def webhook_should_flip" scripts/autoinstall-agent.py        # expect: 1 hit ~154
  grep -n 'finished", "complete", "success' scripts/autoinstall-agent.py # expect: 1 hit ~164 (widened tuple)
  grep -n "tpm_ek" scripts/autoinstall-agent.py                          # expect: hits ~601-613 (bind + mismatch-403)
  grep -n "No iPXE file found" scripts/autoinstall-agent.py              # expect: 1 hit ~173 (missing-file tolerance)
  grep -n "status_update" src/network/ssh_installer/status.rs            # expect: hits — the Rust sender payload you must stay compatible with
  grep -n '"error": "invalid json"' scripts/autoinstall-agent.py         # expect: 1 hit ~567 (the :564-568 invalid-json 400)
  grep -n '"/api/register"' scripts/autoinstall-agent.py                 # expect: 1 hit ~571 (start of the :571-596 register handler)
  grep -n "mac and hostname required" scripts/autoinstall-agent.py       # expect: 1 hit ~577 (register 400 body)
  grep -n '"/api/webhook"' scripts/autoinstall-agent.py                  # expect: 1 hit ~624 (start of the :624-650 webhook handler)
  grep -n '"/api/finalreport"' scripts/autoinstall-agent.py              # expect: 1 hit ~652 (the :652-656 log-only endpoints tuple)
  grep -n '{"endpoint": path' scripts/autoinstall-agent.py               # expect: 1 hit ~653 (log-only event shape)
  grep -n '"received_at": int(time.time())' scripts/autoinstall-agent.py # expect: 1 hit ~259 (events-log prepend, :257-259)
  grep -n "def normalize_mac" scripts/autoinstall-agent.py               # expect: 1 hit ~72 (:72-73 lowercase + -/. → :)
  grep -n "def find_ipxe_file_by_hostname" scripts/autoinstall-agent.py  # expect: 1 hit ~103 (:103-113 hostname→ipxe resolution)
  ```

## Step-by-step

1. Run the ⛔ START HERE block, the wave-gate check, and the anchor greps. Any STOP condition → report.
2. Add pure functions first (they carry the parity subtleties and get direct tests): `pub fn normalize_mac(mac: &str) -> String`, `pub fn webhook_should_flip(data: &serde_json::Value) -> bool` (exact `:154-168` port incl. `progress == 100` accepting JSON number 100), and (if not reusable from IP-03) `pub fn flip_ipxe_content(content: &str, target: &str) -> String` using regex `set menu-default \S+`.
3. Implement `/api/register` per the table: build the upsert entry preserving `status`/`registered_at`/`tpm_ek` from any existing row; persist via the Registry trait; return the exact message string with the normalized MAC interpolated.
4. Implement `/api/checkin` per the table, in Python's ORDER: lookup → 403 unknown → first-bind → mismatch-403 (return BEFORE touching last_seen) → update last_seen/last_ip → 200 with `approved` bool.
5. Implement `/api/webhook`: (a) append `{"received_at": <epoch>, ...payload}` to the events store (CT-01 seam; Python appends to `events.jsonl` `:257-259`); (b) evaluate `webhook_should_flip`; on true, flip the host's iPXE file to `boot-local-disk` — resolve by hostname the way Python `find_ipxe_file_by_hostname` (`:103-113`) does (registry hostname → `mac-<hexmac>.ipxe`, fallback content scan for `set hostname <name>`); swallow + log every flip failure; (c) record install_history via the Registry trait with a fresh `event_id` UUID; (d) decode `files[]` (`path` sanitized `/`→`_`, content base64) into the CT-01 files dir, swallowing per-file errors; (e) reply `200 {"ok":true}`.
6. Implement `/api/finalreport`, `/api/hardware-info`, `/api/cloud-init`: log event with `{"endpoint": <path>, ...payload}` (`:653`), reply `200 {"ok":true}`.
7. Register the routes in the CT-01 machine_plane router.
8. Unit tests (mock Registry + `tempfile::tempdir()` webroot for iPXE files; no live CRDB, no network):
   - `test_flip_predicate_matrix` — table-driven over: `finished`/`complete` (reporting.sh) → true; `status_update`+`success` → true; `status_update`+`running`+`progress:50` → false; `status_update`+`finished`+`progress:100` → true; `failed` → false; empty `name` → false.
   - `test_register_preserves_approval_and_ek` — pre-seed approved row with EK; re-register → still approved, EK intact, 200.
   - `test_register_missing_fields_400`.
   - `test_checkin_first_bind_then_mismatch_403` — bind EK on first checkin; second checkin with a different EK → 403 with `"TPM mismatch - MAC may be spoofed"` AND `last_seen` unchanged.
   - `test_checkin_matching_ek_ok` — anti-over-suppression: same EK on repeat checkin → 200, `approved` bool correct, last_seen updated (the mismatch guard does not block legitimate checkins).
   - `test_webhook_missing_ipxe_swallowed` — predicate-true payload, NO iPXE file in tempdir → response still `200 {"ok":true}`, install_history still recorded.
   - `test_webhook_flip_and_history` — anti-over-suppression happy path: predicate-true payload + iPXE file present → file rewritten to `set menu-default boot-local-disk`, 200, exactly one install_history record with a UUID event_id.
   - `test_invalid_json_400`.
9. Bump the file header (version + last-edited) on every file you touch; new files get a fresh 4-line header with a new uuid4 (`uuidgen | tr 'A-F' 'a-f'`).

## How to test

```bash
cargo test --lib --offline && cargo build --offline
# Expected: all tests pass (baseline 311 + your new tests), build clean
cargo clippy --offline -- -D warnings
# Expected: no warnings
cargo test -p uaa-control --offline
# Expected: all uaa-control tests pass incl. the 8 tests above; 0 failed
grep -c "test_" crates/uaa-control/src/machine_plane/lifecycle.rs
# Expected: ≥8
```

## Acceptance criteria

- [ ] Flip predicate parity: `grep -n "finished\|complete\|success" crates/uaa-control/src/machine_plane/lifecycle.rs | head -3` → hits (widened tuple present) and `test_flip_predicate_matrix` passes.
- [ ] TPM semantics proven: `test_checkin_first_bind_then_mismatch_403` passes and asserts `last_seen` unchanged on mismatch.
- [ ] Missing-iPXE tolerance: `test_webhook_missing_ipxe_swallowed` passes (200 despite flip failure).
- [ ] Anti-over-suppression: `test_checkin_matching_ek_ok` AND `test_webhook_flip_and_history` pass — legitimate checkins and real final-success flips go through every guard.
- [ ] install_history dedup key: `grep -n "event_id" crates/uaa-control/src/machine_plane/lifecycle.rs` → ≥1 hit (UUID minted at ingest).
- [ ] No direct DB/process access: `grep -rn "tokio_postgres\|process::Command" crates/uaa-control/src/machine_plane/lifecycle.rs` → 0 hits.
- [ ] `cargo test --lib --offline && cargo build --offline` exits 0; clippy clean.
- [ ] File headers bumped on every changed file (`git diff origin/main --stat` files each show a version bump; guids unchanged).

## Commit message

```
feat(control): register/checkin/webhook parity with TPM-EK bind + auto-flip (ws3-parity)

Fills crates/uaa-control/src/machine_plane/lifecycle.rs (CT-01 stub): /api/register
upsert preserving status/registered_at/tpm_ek, /api/checkin first-bind + mismatch-403
without last_seen update, /api/webhook with the exact webhook_should_flip port
(widened finished/complete/success tuple, status_update success-or-progress-100),
missing-iPXE tolerance, events + install_history (event_id UUID, WAL-safe) and
finalreport/hardware-info/cloud-init sinks. 8 tests: mock registry + tempdir webroot.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

If `grep -n "webhook_should_flip" crates/uaa-control/src/machine_plane/lifecycle.rs` hits, already applied — run the acceptance checks instead of re-applying. Rollback = revert the single commit; the CT-01 stub returns to empty, sibling machine_plane files (seeds.rs, inventory.rs) and the Registry layer stay untouched.
