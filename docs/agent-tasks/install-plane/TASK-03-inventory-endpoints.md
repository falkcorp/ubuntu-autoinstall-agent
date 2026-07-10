<!-- file: docs/agent-tasks/install-plane/TASK-03-inventory-endpoints.md -->
<!-- version: 1.0.1 -->
<!-- guid: 3af2b91f-e2ff-49f0-85a5-d2c613177920 -->
<!-- last-edited: 2026-07-10 -->

# TASK-03 — approve/deregister/flip/certs + yubikey + tang endpoint parity incl. 403-unapproved rules (ws3-parity)

**Priority:** P1 · **Effort:** M · **Recommended subagent:** Sonnet-class · rust-http subagent · **Why:** many endpoints, each mechanically mirrored from Python with exact status codes · **Depends on:** none within this workstream (wave-4 gated: `control/TASK-01` (CT-01) MERGED — it creates `crates/uaa-control` incl. the `machine_plane/inventory.rs` stub and the Registry trait)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/install-plane-inventory-endpoints" -b agent/install-plane-inventory-endpoints origin/main
cd "$REPO/.worktrees/install-plane-inventory-endpoints"
git rebase origin/main
```

(Protocol also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

Wave gate check: `test -f crates/uaa-control/src/machine_plane/inventory.rs` — missing = wave-4 gate NOT satisfied; STOP and report.

## Goal

Fill `crates/uaa-control/src/machine_plane/inventory.rs` (CT-01 stub — exactly-one-filler rule) with parity handlers for the operator/inventory GET+POST endpoints of `scripts/autoinstall-agent.py` (spec Decision 12). Every JSON response via the CT-01 `send_json` equivalent.

Per-endpoint parity table (anchors below; all paths regex-matched like Python):

| Method | Path | Python | Statuses | Semantics |
|---|---|---|---|---|
| GET | `/api/certs/<hostname>?ip=&mac=` | `:286-312` | 403, 500, 200 | lookup by normalized `mac` param, fallback hostname scan; not registered → `403 {"ok":false,"error":"Not registered. Run register-len-server.sh first."}`; status≠approved → `403 {"ok":false,"error":"Pending approval. Status: <s>."}`; cert-gen error → `500 {"ok":false,"error":<stderr>}`; else `200 {"ok":true,"certs":{"ca.crt":b64,"node.crt":b64,"node.key":b64}}` |
| GET | `/api/flip/<hostname>?target=` | `:315-329` | 403, 404, 200 | target default `boot-local-disk`; target=`custom-autoinstall` requires an APPROVED registry entry for that hostname else `403 {"ok":false,"error":"Flip to reinstall requires approved status"}`; flip result → `200/404 {"ok":<b>,"message":<m>}` (missing iPXE file = 404 here, unlike the webhook's swallow) |
| GET | `/api/approve/<mac>` | `:332-344` | 404, 200 | unknown → `404 {"ok":false,"error":"MAC not registered"}`; else status=approved + approved_at, `200 {"ok":true,"message":"Approved <mac>","entry":<row>}` |
| GET | `/api/deregister/<mac>` | `:347-359` | 404, 200 | unknown → 404 same error; else delete row, `200 {"ok":true,"message":"Deregistered <mac> (<hostname>)"}` |
| GET | `/api/registry` | `:401-403` | 200 | whole machine map (empty map = `200 {}`) |
| GET | `/api/events` | `:406-412` | 200 | last 50 events; ANY read error → `200 []` (never 500) |
| GET | `/api/yubikeys` | `:419-424` | 200 | map with `gpg_pubkey` STRIPPED from every entry |
| GET | `/api/yubikeys/ssh-keys` | `:427-431` | 200 | `{"keys":[...]}` — ssh_pubkey of APPROVED entries only |
| GET | `/api/yubikeys/approve/<fp>` | `:434-446` | 404, 200 | fp uppercased; unknown → 404; else approved + approved_at, entry echoed WITHOUT gpg_pubkey |
| GET | `/api/yubikeys/<FP>/pubkey` | `:449-465` | 404, 403, 200 | fp regex `[A-F0-9]+`; unknown or no gpg key → 404; not approved → `403 {"ok":false,"error":"YubiKey not approved"}`; else 200 raw armored block, `Content-Type: application/pgp-keys` |
| GET | `/api/yubikeys/revoke/<fp>` | `:468-480` | 404, 200 | unknown → 404; else status=revoked + revoked_at |
| GET | `/api/tang/servers` | `:487-490` | 200 | whole tang map |
| POST | `/api/yubikeys/register` | `:662-689` | 400, 200 | fp uppercased+space-stripped; empty → `400 {"ok":false,"error":"fingerprint required"}`; upsert preserving status/registered_at; 200 with approve-URL message |
| POST | `/api/tang/checkin` | `:692-709` | 200 | upsert by hostname with last_seen; `200 {"ok":true}` |
| any | anything unmatched | `:558,711` | 404 | `404 {"error":"not found"}` |

REUSE — do not invent parallels:
- **`CommandExecutor`** (`src/network/executor.rs` — verify: `grep -n "pub trait CommandExecutor" src/network/executor.rs`) for the `cockroach cert create-node` shell-out (skeleton shared_state: preserved via CommandExecutor, LocalClient at runtime; MOCK in tests — no cockroach binary needed). Command shape mirrors Python `:240-244`: `cockroach cert create-node <ip> <hostname> <hostname>.jf.local localhost 127.0.0.1 --certs-dir=<tmpdir> --ca-key=<ca_key>` after copying `ca.crt` into a fresh tempdir; read back + base64 `ca.crt`/`node.crt`/`node.key`.
- CT-01 Registry trait for machines/yubikeys/tang stores (mock in tests; no live CRDB).
- iPXE flip: mirror `flip_ipxe` (`:170-177`) — regex `set menu-default \S+` replacement; file located per `find_ipxe_file_by_hostname` (`:103-113`). (IP-02 ports the same 8-line helper for the webhook path; files are disjoint, coordinator deduplicates in review if both land.)

## Background (verify before editing)

- Design spec: `docs/specs/constellation-design.md` Decision 12 (hard-404 unknown single resources; empty-200 collections — `/api/events` returning `200 []` on error is normative) and the machines/yubikeys/tang tables in the CRDB schema section.
- Legacy note: these CA paths are the COCKROACH CA (`/var/lib/cockroach-autoinstall/.cockroach-ca/`, Python `:41-42`) serving node-cert issuance for the DB cluster — this is parity-frozen legacy behavior. It is NOT the install CA of Decision 6 (PKI workstream) and the two must never be conflated; paths come from CT-01 config, tests use tempdirs.
- Edge semantics spelled out: approve of an already-approved MAC is a normal 200 re-approve (Python has no guard); deregister removes only the registry row (iPXE/cloud-init files untouched); yubikey fingerprint case: approve/revoke/register uppercase the input, the pubkey route only MATCHES `[A-F0-9]+` (lowercase fp in URL → falls through to 404 not-found); `/api/flip` with a non-`custom-autoinstall` target needs NO registration at all (any hostname with an iPXE file flips to local-disk).

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
  grep -n "status.*approved" scripts/autoinstall-agent.py | head -5     # expect: hits ~302 (certs gate), ~322 (flip gate), ~339 (approve write)
  grep -n "custom-autoinstall" scripts/autoinstall-agent.py | head -3   # expect: hits ~319 (flip target gate)
  grep -n "cockroach\", \"cert\", \"create-node" scripts/autoinstall-agent.py  # expect: 1 hit ~240
  grep -n "application/pgp-keys" scripts/autoinstall-agent.py           # expect: 1 hit ~461
  grep -n "pub trait CommandExecutor" src/network/executor.rs           # expect: 1 hit ~11 (mapped: crates/uaa-core/src/network/executor.rs)
  # Parity-table route anchors (each grep verified against today's main; comment hits alongside route hits are expected):
  grep -nE '/api/(deregister|registry|events)' scripts/autoinstall-agent.py
  # expect: 3 hits — deregister re.match ~347, registry ~401, events ~406
  grep -n '/api/approve/' scripts/autoinstall-agent.py
  # expect: 3 hits — docstring ~13, the route re.match ~332, register-reply message ~595
  grep -nE '/api/(yubikeys|tang)' scripts/autoinstall-agent.py
  # expect: ~17 hits (routes + adjacent comments) — yubikeys list ~419, ssh-keys ~427,
  #   approve ~434, <fp>/pubkey ~449, revoke ~468, tang/servers ~487,
  #   POST yubikeys/register ~662, POST tang/checkin ~693
  grep -n '"error": "not found"' scripts/autoinstall-agent.py
  # expect: 2 hits ~558 (GET fallthrough) and ~711 (POST fallthrough)
  grep -nE 'def (flip_ipxe|find_ipxe_file_by_hostname)' scripts/autoinstall-agent.py
  # expect: 2 hits — find_ipxe_file_by_hostname ~103, flip_ipxe ~170 (the REUSE helpers above)
  grep -n 'cockroach-ca' scripts/autoinstall-agent.py
  # expect: 2 hits ~41-42 — CA_CRT/CA_KEY under /var/lib/cockroach-autoinstall/.cockroach-ca/
  ```

## Step-by-step

1. Run the ⛔ START HERE block, the wave-gate check, and the anchor greps. Any STOP condition → report.
2. Add `pub async fn generate_certs(executor: &dyn CommandExecutor, ca_crt: &Path, ca_key: &Path, hostname: &str, ip: &str) -> Result<BTreeMap<String,String>, String>` mirroring Python `:236-254`: tempdir, copy ca.crt in, run the exact `cockroach cert create-node ...` argv through the executor, nonzero exit → `Err(stderr)`, else base64 the three files. Tests inject a mock that WRITES fake `node.crt`/`node.key` into the certs-dir it is pointed at.
3. Implement `/api/certs/<hostname>` per the table, in Python's order: mac-param lookup → hostname-scan fallback → 403 unregistered → 403 unapproved → generate → 500 on Err → 200.
4. Implement `/api/flip/<hostname>`: parse `target` (default `boot-local-disk`); the `custom-autoinstall` approved-status gate FIRST (403); then flip via the mirrored `flip_ipxe`; `200/404 {"ok","message"}`.
5. Implement approve/deregister/registry/events per the table (Registry trait; events read errors → `200 []`).
6. Implement the six yubikey routes + two tang routes per the table. `gpg_pubkey` stripping applies to the LIST and the approve echo, never to the pubkey route body.
7. Add the machine-plane catch-all `404 {"error":"not found"}` if CT-01's router stub does not already provide it (grep first: `grep -rn "not found" crates/uaa-control/src/machine_plane/`).
8. Register all routes in the CT-01 machine_plane router.
9. Unit tests (mock Registry + mock executor + tempdir iPXE dir; no live CRDB, no cockroach binary, no network):
   - `test_certs_unregistered_403` / `test_certs_unapproved_403` — exact error strings from the table.
   - `test_certs_approved_issues` — anti-over-suppression: approved host + mock executor writing fake certs → 200 with 3 base64 entries and the recorded argv containing `cert create-node` and `<hostname>.jf.local`.
   - `test_flip_install_requires_approved` — `target=custom-autoinstall` unapproved → 403; approved → 200 and file rewritten (guard does not block the sanctioned path).
   - `test_flip_local_disk_no_registration_needed` — unregistered hostname with an iPXE file flips to `boot-local-disk` → 200.
   - `test_flip_missing_file_404`.
   - `test_approve_unknown_404` / `test_approve_sets_status`.
   - `test_yubikey_listing_strips_gpg` — listed entries lack `gpg_pubkey`; `/pubkey` route still returns the armored block for an approved fp; unapproved fp → 403.
   - `test_events_read_error_empty_200`.
10. Bump the file header (version + last-edited) on every file you touch; new files get a fresh 4-line header with a new uuid4 (`uuidgen | tr 'A-F' 'a-f'`).

## How to test

```bash
cargo test --lib --offline && cargo build --offline
# Expected: all tests pass (baseline 311 + your new tests), build clean
cargo clippy --offline -- -D warnings
# Expected: no warnings
cargo test -p uaa-control --offline
# Expected: all uaa-control tests pass incl. the 10 tests above; 0 failed
grep -c '"ok"' crates/uaa-control/src/machine_plane/inventory.rs
# Expected: ≥10 (the ok/error body convention is followed per endpoint)
```

## Acceptance criteria

- [ ] 403-unapproved rules proven: `test_certs_unregistered_403`, `test_certs_unapproved_403`, `test_flip_install_requires_approved` all pass with the exact Python error strings (`grep -n "Pending approval. Status:" crates/uaa-control/src/machine_plane/inventory.rs` → 1 hit).
- [ ] Anti-over-suppression: `test_certs_approved_issues` and the approved half of `test_flip_install_requires_approved` pass — approved hosts get certs and install flips through every guard.
- [ ] cockroach shell-out through the seam only: `grep -rn "process::Command\|Command::new" crates/uaa-control/src/machine_plane/inventory.rs` → 0 hits; `grep -n "create-node" crates/uaa-control/src/machine_plane/inventory.rs` → ≥1 hit.
- [ ] gpg_pubkey never leaks in listings: `test_yubikey_listing_strips_gpg` passes.
- [ ] Collections vs single resources: `test_events_read_error_empty_200` and `test_approve_unknown_404` pass (Decision 12 conventions).
- [ ] `cargo test --lib --offline && cargo build --offline` exits 0; clippy clean.
- [ ] File headers bumped on every changed file (`git diff origin/main --stat` files each show a version bump; guids unchanged).

## Commit message

```
feat(control): approve/deregister/flip/certs + yubikey + tang parity (ws3-parity)

Fills crates/uaa-control/src/machine_plane/inventory.rs (CT-01 stub): 15 endpoints
mirrored from scripts/autoinstall-agent.py with exact status codes — certs 403
unregistered/unapproved gates + cockroach cert create-node via CommandExecutor,
flip with the custom-autoinstall approved gate, approve/deregister/registry/events,
yubikey registry (gpg_pubkey stripped in listings, pgp-keys pubkey route, approve/
revoke), tang registry, catch-all 404. 10 tests: mock registry + mock executor +
tempdir iPXE dir.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

If `grep -n "Pending approval. Status:" crates/uaa-control/src/machine_plane/inventory.rs` hits, already applied — run the acceptance checks instead of re-applying. Rollback = revert the single commit; the CT-01 stub returns to empty, sibling machine_plane files (seeds.rs, lifecycle.rs) and the Registry layer stay untouched.
