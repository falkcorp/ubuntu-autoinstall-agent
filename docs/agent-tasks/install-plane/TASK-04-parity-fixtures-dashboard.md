<!-- file: docs/agent-tasks/install-plane/TASK-04-parity-fixtures-dashboard.md -->
<!-- version: 1.0.2 -->
<!-- guid: 80d07d48-eb7a-4a82-8e42-10732b07b9b0 -->
<!-- last-edited: 2026-07-10 -->

# TASK-04 — Recorded parity fixture suite + /dashboard + /api/health + /api/uaa-configs (ws3-parity)

**Priority:** P1 · **Effort:** M · **Recommended subagent:** Sonnet-class · rust-test subagent · **Why:** the parity gate itself; fixtures derived line-by-line from the Python handlers · **Depends on:** TASK-01, TASK-02, TASK-03 (wave-5 gated: IP-01/IP-02/IP-03 all MERGED — the fixture suite exercises their handlers end-to-end)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/install-plane-parity-fixtures-dashboard" -b agent/install-plane-parity-fixtures-dashboard origin/main
cd "$REPO/.worktrees/install-plane-parity-fixtures-dashboard"
git rebase origin/main
```

(Protocol also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

Wave gate check: `grep -n "resolve_cloud_init_dir" crates/uaa-control/src/machine_plane/seeds.rs && grep -n "webhook_should_flip" crates/uaa-control/src/machine_plane/lifecycle.rs && grep -n "Pending approval. Status:" crates/uaa-control/src/machine_plane/inventory.rs` — any 0-hit = a wave-4 sibling is unmerged; STOP and report.

## Goal

Two deliverables (spec Decision 12 + Migration step 2 — this fixture suite is the **M2 gate artifact**, skeleton shared_state):

1. Fill `crates/uaa-control/src/machine_plane/dashboard.rs` (CT-01 stub — exactly-one-filler rule) with the three read-only status endpoints:

| Method | Path | Python | Statuses | Semantics |
|---|---|---|---|---|
| GET | `/api/health` | `:362-372` | 200 | `{"status":"ok","registry_hosts":N,"registry_approved":N,"yubikeys":N,"tang_servers":N,"agent_binary":{path,present,size,mtime}}` — an ABSENT agent binary is the NORMAL case (`present:false`, size/mtime null, Python `:47-58`), never an error |
| GET | `/api/uaa-configs` | `:375-378` | 200 | `{"configs":[{hexmac,hostname,mtime,placeholder_free}],"count":N}` — METADATA ONLY, never file contents (`:204-233`): dir names must match `^[0-9a-f]{12}$`, hexmac dirs without `uaa.yaml` skipped, missing root → `{"configs":[],"count":0}`, `placeholder_free` = bytes do NOT contain `REPLACE_AT_PLACE_TIME` |
| GET | `/dashboard` | `:381-398` | 200 | display-only HTML (`Content-Type: text/html; charset=utf-8`): agent-binary table, registry table, placed-configs table (metadata only; `ready` column = `yes`/`PLACEHOLDER`), last-20 events table. EVERY interpolated value html-escaped (`:118-119`); NO `<script>`, NO `<form>`, NO external assets |

2. Create the recorded parity fixture suite under `crates/uaa-control/tests/parity/` — one integration-test harness (`crates/uaa-control/tests/parity.rs` driving `tests/parity/fixtures/*.json`) that spins the full machine-plane router (in-process, mock Registry + mock executor + tempdir webroot) and asserts, per fixture: method, path, request body → expected status code, `Content-Type`, and body (exact JSON for API routes; byte-empty checks for the seed-split cases). Fixtures are transcribed line-by-line from `scripts/autoinstall-agent.py` and MUST cover at minimum: the Decision-12 empty-200-missing-seed-file vs hard-404-uaa-config pair, unknown-neighbor 404, register 400/200, checkin 403-unknown / first-bind / mismatch-403, webhook flip + missing-iPXE swallow, certs 403/403/200, flip 403/404/200, approve/deregister 404/200, events empty-200, yubikey list-strip/approve/pubkey-403/revoke, tang list/checkin, catch-all 404 — ≥25 fixtures total (one per Python endpoint/branch pair).

REUSE — do not invent parallels:
- The three filled machine_plane modules (IP-01/02/03) and CT-01's router/state — the fixture harness calls the SAME router construction the binary uses (`tower::ServiceExt::oneshot` per request). Do NOT re-declare handlers in the test tree.
- Dashboard escaping: use a tiny local `esc()` helper (or the `html-escape` crate ONLY if it is already in the workspace `Cargo.lock` — check `grep -n "html-escape" Cargo.lock`; if absent, hand-roll the 5-entity escaper). Do NOT add new crates.io dependencies (builds are `--offline`).

## Background (verify before editing)

- Design spec: `docs/specs/constellation-design.md` Decision 12 ("The parity matrix in the implementation plan is normative and encodes this split per endpoint") and Testing table row 1 ("parity fixture suite ... status/body parity with Python handlers incl. empty-200-missing-seed-file vs 404-missing-uaa-config, 403 conventions, TPM bind").
- The dashboard is DISPLAY-ONLY (Python docstring `:116-117`): no mutation routes, no forms — this is a security property, restate in code comments.
- `placeholder_free` semantics spelled out: it reports whether a placed config still carries `REPLACE_AT_PLACE_TIME` — the dashboard shows `PLACEHOLDER` for not-yet-injected configs. Fixture configs use the literal placeholder string; NEVER put a real-looking secret in a fixture.
- `agent_binary` path comes from CT-01 config (Python constant `:33`); tests point it at a tempdir file / absent path.

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
  grep -n "html.escape" scripts/autoinstall-agent.py | head -3          # expect: 1 hit ~119 (esc() inside render_dashboard; handler at ~381-398)
  grep -n "def render_dashboard" scripts/autoinstall-agent.py           # expect: 1 hit ~115
  grep -n "def collect_uaa_configs" scripts/autoinstall-agent.py        # expect: 1 hit ~204
  grep -n "def agent_binary_status" scripts/autoinstall-agent.py        # expect: 1 hit ~47
  grep -n "placeholder_free" scripts/autoinstall-agent.py               # expect: hits ~231 (definition) and ~143 (dashboard ready column)
  grep -n 'if path == "/api/health"' scripts/autoinstall-agent.py       # expect: 1 hit ~362 (handler cited :362-372 in the Goal table)
  grep -n 'if path == "/api/uaa-configs"' scripts/autoinstall-agent.py  # expect: 1 hit ~375 (handler cited :375-378)
  grep -n 'if path == "/dashboard"' scripts/autoinstall-agent.py        # expect: 1 hit ~381 (handler cited :381-398)
  grep -n "UAA_BINARY_PATH" scripts/autoinstall-agent.py                # expect: 3 hits — constant ~33 plus uses ~370/~391
  grep -n "display-only status HTML" scripts/autoinstall-agent.py       # expect: 1 hit ~116 (the :116-117 display-only docstring)
  grep -n 'if path == "/autoinstall/uaa-config"' scripts/autoinstall-agent.py  # expect: 1 hit ~530 (handler whose hard-404 branch is cited py:544-548 for the Decision-12 fixture)
  grep -n "no uaa.yaml placed" scripts/autoinstall-agent.py             # expect: 1 hit ~545 (the exact hard-404 missing-uaa.yaml branch)
  ```

## Step-by-step

1. Run the ⛔ START HERE block, the wave-gate check, and the anchor greps. Any STOP condition → report.
2. In `dashboard.rs` add `pub fn agent_binary_status(path: &Path) -> serde_json::Value` mirroring `:47-58` (absent → `present:false`, null size/mtime; mtime formatted `%Y-%m-%dT%H:%M:%SZ` UTC).
3. Add `pub fn collect_uaa_configs(base: &Path, hex_to_hostname: &BTreeMap<String,String>) -> Vec<serde_json::Value>` mirroring `:204-233` exactly: sorted dir listing, `^[0-9a-f]{12}$` filter, skip dirs without a readable `uaa.yaml`, metadata-only output, missing base → empty vec.
4. Implement `/api/health` and `/api/uaa-configs` per the table (counts via the Registry trait).
5. Implement `/dashboard`: port `render_dashboard` (`:115-152`) structurally — same four tables, same column order, `esc()` on every value, `ready` = `yes`/`PLACEHOLDER`. Register all three routes in the CT-01 router.
6. Unit tests in `dashboard.rs`:
   - `test_agent_binary_absent_is_normal` — absent path → `present:false`, no error.
   - `test_uaa_configs_metadata_only_and_regex` — tempdir with a valid hexmac dir (uaa.yaml containing `REPLACE_AT_PLACE_TIME`), a hexmac dir without uaa.yaml, and a `README.md` entry → exactly one config, `placeholder_free:false`, response contains NO file contents.
   - `test_uaa_configs_placeholder_free_listed` — anti-over-suppression: a placeholder-free uaa.yaml in a valid hexmac dir IS listed with `placeholder_free:true` (the regex/skip guards do not hide legitimate configs).
   - `test_dashboard_escapes_and_is_inert` — registry hostname `<script>alert(1)</script>` renders escaped (`&lt;script&gt;`); body contains no `<script>` and no `<form>`.
7. Create `crates/uaa-control/tests/parity.rs` + `crates/uaa-control/tests/parity/fixtures/*.json`. Fixture schema (document it in a `tests/parity/README.md` with a fresh header): `{"name","setup":{"registry":{...},"webroot_files":{...},"neighbor_mac":...,"executor":{...}},"request":{"method","path","body"},"expect":{"status","content_type","body_json"|"body_empty"|"body_contains"}}`. The harness loads every fixture, builds router state from `setup`, fires the request, asserts `expect`. Name fixtures `NN-<endpoint>-<branch>.json` in Python source order.
8. Write the ≥25 fixtures enumerated in the Goal. Every status code and body string transcribed from `scripts/autoinstall-agent.py` — cite the Python line in each fixture's `"name"` (e.g. `"uaa-config missing file hard-404 (py:544-548)"`).
9. Bump the file header (version + last-edited) on every file you touch; new files get a fresh 4-line header with a new uuid4 (`uuidgen | tr 'A-F' 'a-f'`).

## How to test

```bash
cargo test --lib --offline && cargo build --offline
# Expected: all tests pass (baseline 311 + your new tests), build clean
cargo clippy --offline -- -D warnings
# Expected: no warnings
cargo test -p uaa-control --offline
# Expected: all pass incl. 4 dashboard unit tests + the parity harness; 0 failed
cargo test -p uaa-control --test parity --offline
# Expected: parity suite green; ≥25 fixtures executed (count printed or visible in test names)
ls crates/uaa-control/tests/parity/fixtures/ | wc -l
# Expected: ≥25
```

## Acceptance criteria

- [ ] Decision-12 split in the gate suite: `grep -rln "544-548\|hard-404" crates/uaa-control/tests/parity/fixtures/` → ≥1 fixture, and the paired empty-200 seed fixture exists (`grep -rln "empty" crates/uaa-control/tests/parity/fixtures/ | head -1` → hit).
- [ ] Fixture count: `ls crates/uaa-control/tests/parity/fixtures/*.json | wc -l` → ≥25, covering every endpoint group in the Goal list.
- [ ] Metadata-only enforced: `test_uaa_configs_metadata_only_and_regex` passes; `grep -rn "REPLACE_AT_PLACE_TIME" crates/uaa-control/tests/parity/fixtures/` hits ONLY as the placeholder literal in setup data (no real-looking secrets: `grep -rniE "password|BEGIN (RSA|EC|OPENSSH) PRIVATE" crates/uaa-control/tests/parity/fixtures/` → 0 hits).
- [ ] Anti-over-suppression: `test_uaa_configs_placeholder_free_listed` passes — legitimate placeholder-free configs are listed, not filtered.
- [ ] Dashboard inert + escaped: `test_dashboard_escapes_and_is_inert` passes; `grep -n "<form\|<script" crates/uaa-control/src/machine_plane/dashboard.rs` → 0 hits outside test-input strings.
- [ ] `cargo test --lib --offline && cargo build --offline` exits 0; clippy clean.
- [ ] File headers bumped on every changed file (`git diff origin/main --stat` files each show a version bump; guids unchanged).

## Commit message

```
feat(control): parity fixture suite (M2 gate) + dashboard/health/uaa-configs (ws3-parity)

Fills crates/uaa-control/src/machine_plane/dashboard.rs (CT-01 stub): /api/health
with absent-binary-is-normal, /api/uaa-configs metadata-only inventory (hexmac
regex, placeholder_free flag), display-only escaped /dashboard. Adds
crates/uaa-control/tests/parity/ — ≥25 recorded request/response fixtures
transcribed line-by-line from scripts/autoinstall-agent.py (empty-200 vs hard-404
seed split, 403 conventions, TPM bind, flip swallow) driven by one router-level
harness with mock registry + tempdir webroot.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

If `grep -n "collect_uaa_configs" crates/uaa-control/src/machine_plane/dashboard.rs` hits AND `test -d crates/uaa-control/tests/parity/fixtures`, already applied — run the acceptance checks instead of re-applying. Rollback = revert the single commit; the dashboard stub returns to empty and the tests/parity tree is removed — IP-01/02/03 handlers and the rest of the crate stay untouched.
