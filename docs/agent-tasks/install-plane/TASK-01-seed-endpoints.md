<!-- file: docs/agent-tasks/install-plane/TASK-01-seed-endpoints.md -->
<!-- version: 1.0.1 -->
<!-- guid: 39e6984e-1ade-4f00-b796-86ab829541bc -->
<!-- last-edited: 2026-07-10 -->

# TASK-01 — /autoinstall/* seed-endpoint parity: ip-neigh MAC resolution, empty-200 vs hard-404 split (ws3-parity)

**Priority:** P1 · **Effort:** M · **Recommended subagent:** Sonnet-class · rust-http subagent · **Why:** parity subtleties are the whole task (spec Decision 12 normative split) · **Depends on:** none within this workstream (wave-4 gated: `control/TASK-01` (CT-01) MERGED — it creates the `crates/uaa-control` crate and the empty `machine_plane/seeds.rs` stub this task fills)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/install-plane-seed-endpoints" -b agent/install-plane-seed-endpoints origin/main
cd "$REPO/.worktrees/install-plane-seed-endpoints"
git rebase origin/main
```

(Protocol also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

Wave gate check (CT-01 merged?): `test -f crates/uaa-control/src/machine_plane/seeds.rs && grep -rn "machine_plane" crates/uaa-control/src/lib.rs` — 0 hits / missing file = the wave-4 gate is NOT satisfied; STOP and report.

## Goal

Fill `crates/uaa-control/src/machine_plane/seeds.rs` (stub created by CT-01 — this task is its EXACTLY-ONE filler per the collision matrix) with drop-in parity handlers for the five auto-resolved endpoints of `scripts/autoinstall-agent.py`:

| Method | Path | Python anchor | Status/body convention |
|---|---|---|---|
| GET | `/autoinstall/user-data` | `:496-519` | see split below |
| GET | `/autoinstall/meta-data` | `:496-519` | see split below |
| GET | `/autoinstall/vendor-data` | `:496-519` | see split below |
| GET | `/autoinstall/network-config` | `:496-519` | see split below |
| GET | `/autoinstall/uaa-config` | `:530-556` | missing `uaa.yaml` = **hard 404** |

Normative split (spec Decision 12, correctness-judge-confirmed): for the four seed files, when the client's MAC resolves and the `<hexmac>` dir EXISTS but the requested FILE is missing, return **empty 200** (`Content-Type: text/plain; charset=utf-8`, `Content-Length: 0`) — Python `:512` reads `b""` for a missing file. For `/autoinstall/uaa-config` the same condition is a **hard 404 with empty body** (`:544-548`): the USB bootstrap must fail loudly at fetch time, never receive an empty config. No neighbor-table entry for the client IP → 404 (empty body) for ALL five. Neighbor resolves but no `<hexmac>` dir → 404 (empty body) for ALL five. File present → 200 with the raw file bytes, `Content-Type: text/plain; charset=utf-8`.

MAC resolution mirrors `mac_from_neighbor_table` (`scripts/autoinstall-agent.py:186`): run `ip neigh show <client_ip>`, regex `lladdr ([0-9a-fA-F:]+)`, lowercase, strip to 12-hex `hexmac` (`:`/`-`/`.` removed). Purely additive: only `seeds.rs` (plus its registration line if CT-01's router stub requires one inside `machine_plane/mod.rs`) changes.

REUSE — do not invent parallels:
- **`CommandExecutor`** trait (`src/network/executor.rs` — verify: `grep -n "pub trait CommandExecutor" src/network/executor.rs`) as the seam for the `ip neigh show` shell-out. Handlers take `&dyn CommandExecutor` (or the Arc'd form CT-01's router state uses) so tests inject a mock. Do NOT call `std::process::Command` directly.
- CT-01's `machine_plane` router/state types (`crates/uaa-control/src/machine_plane/mod.rs` after the wave gate). Do NOT create a second axum Router or a second webroot-path config field.

## Background (verify before editing)

- Design spec: `docs/specs/constellation-design.md` — Decisions 12 (parity in uaa-control; seed READS straight from the webroot; the empty-200/hard-404 split is normative) and 4 (`:25000` read plane keeps serving under CRDB degradation — these handlers never require a live registry/DB: resolution is neighbor-table + filesystem only, exactly like Python).
- Webroot base is the Python `CLOUD_INIT_BASE = /var/www/html/cloud-init` (`scripts/autoinstall-agent.py:32`); in this crate it MUST come from CT-01's config/state (tests point it at a tempdir), never hardcoded.
- Edge semantics, spelled out: `ip neigh` command error/timeout/no-`lladdr`-match ⇒ treated as "no neighbor entry" ⇒ 404 (Python swallows every exception to `None`, `:193-194`). Empty client IP ⇒ 404. A hexmac dir that exists with NONE of the files ⇒ each seed file is empty-200, `uaa-config` is 404. Never log file contents — placed `uaa.yaml` holds real secrets at runtime.

**HARD RULES (non-negotiable):**
- NO hardware actions. Validate ONLY in-repo (`cargo`) and, where a brief says so, the QEMU+swtpm harness (`scripts/vm-validate.sh`). Code that COULD touch hardware is written and unit-tested against mock executors only.
- NEVER wipe, write to, or deploy on 172.16.2.30 ("the server") or len-serv-003.
- `disk_device` is read from the live target at runtime, never guessed or hardcoded.
- ipmitool runs via `ssh 172.16.2.30`, never on macOS.
- NEVER power on unimatrixone (U1).
- No real secret in any file: `REPLACE_AT_PLACE_TIME` placeholders stay placeholders.
- Stay inside your worktree; never `git push`, `gh pr`, or merge — report done and stop.

**Path map:** after CP-01 (wave 1) merges, `src/**` lives at `crates/uaa-core/src/**` and the CLI at `crates/uaa/src/**`. The greps above/below cite pre-move paths (verifiable on today's main); at execution time run them at the old path, then the mapped path. Zero hits at BOTH = STOP and report.

- **Re-verify these anchors before editing** — line numbers drift; zero hits at both old and mapped path = STOP and report:
  ```bash
  grep -n "send_response(200)" scripts/autoinstall-agent.py | head -5   # expect: hits incl. line ~514 (seed serve) and ~551 (uaa-config serve)
  grep -n "uaa-config" scripts/autoinstall-agent.py                     # expect: hits ~523-530 (handler) — hard-404 branch at ~544-548
  grep -n "def mac_from_neighbor_table" scripts/autoinstall-agent.py    # expect: 1 hit ~186
  grep -n "def resolve_cloud_init_dir" scripts/autoinstall-agent.py     # expect: 1 hit ~196
  grep -n "def mac_to_hex" scripts/autoinstall-agent.py                 # expect: 1 hit ~75 (body :76 — the strip-to-12-hex mirror for step 2)
  grep -n "^CLOUD_INIT_BASE" scripts/autoinstall-agent.py               # expect: 1 hit ~32 (webroot base constant)
  grep -n 'else b""' scripts/autoinstall-agent.py                       # expect: 1 hit ~512 (missing seed file read as empty bytes — the empty-200 anchor)
  grep -n "except Exception:" scripts/autoinstall-agent.py              # expect: 2 hits; the mac_from_neighbor_table swallow-to-None is ~193 (returns None ~194)
  grep -n "lladdr" scripts/autoinstall-agent.py                         # expect: 1 hit ~191 (the resolution regex)
  grep -n "pub trait CommandExecutor" src/network/executor.rs           # expect: 1 hit ~11 (mapped: crates/uaa-core/src/network/executor.rs)
  ```

## Step-by-step

1. Run the ⛔ START HERE block, the wave-gate check, and the anchor greps. Any STOP condition → report, do not improvise.
2. In `crates/uaa-control/src/machine_plane/seeds.rs`, add `pub fn mac_from_neighbor_output(out: &str) -> Option<String>` — pure function: regex `lladdr ([0-9a-fA-F:]+)`, lowercase the capture. Add `pub fn mac_to_hex(mac: &str) -> String` (strip `:`/`-`/`.`, lowercase) — mirror Python `:75-76`.
3. Add `pub async fn resolve_cloud_init_dir(executor: &dyn CommandExecutor, webroot: &Path, client_ip: &str) -> Option<(String, Option<PathBuf>)>` mirroring Python `:196-202`: run `ip neigh show <client_ip>` via the executor (any executor error ⇒ `None`), parse the MAC, return `(hexmac, Some(dir))` only when `webroot/<hexmac>` is a directory, `(hexmac, None)` when it is not, `None` when no MAC resolved.
4. Implement the four seed handlers on ONE parameterized route (filename ∈ {`user-data`,`meta-data`,`vendor-data`,`network-config`} — any other filename must NOT match this route): no-MAC or no-dir → 404 empty; dir present → read `dir/<filename>`; missing file → `200` empty body; present → `200` raw bytes. Content-Type `text/plain; charset=utf-8` on every 200.
5. Implement `/autoinstall/uaa-config`: same resolution; additionally missing `dir/uaa.yaml` → **404 empty** (never empty-200); present → 200 raw bytes, same Content-Type. Log lines mirror Python's redaction discipline: log client_ip + hexmac + DENIED reason only — NEVER body contents.
6. Register the five routes in CT-01's `machine_plane` router exactly where its stub marks handler registration (grep at execution time: `grep -n "seeds" crates/uaa-control/src/machine_plane/mod.rs`).
7. Unit tests (`#[cfg(test)]` in `seeds.rs`) with a recording `MockExecutor` (mirror the `MockExecutor` idiom — verify: `grep -n "struct MockExecutor" src/autoinstall/verify.rs`) + `tempfile::tempdir()` webroot; no network, no live CRDB:
   - `test_mac_parse_and_hex` — `"172.16.3.92 dev eth0 lladdr 6C:4B:90:BC:39:B3 REACHABLE"` → `6c:4b:90:bc:39:b3` → hex `6c4b90bc39b3`.
   - `test_no_neighbor_entry_404` — executor returns no `lladdr` → all five endpoints 404, empty body.
   - `test_no_hexmac_dir_404` — MAC resolves, dir absent → all five 404.
   - `test_missing_seed_file_empty_200` — dir exists, `user-data` absent → 200, body length 0, `text/plain; charset=utf-8`.
   - `test_missing_uaa_config_hard_404` — same dir state, `/autoinstall/uaa-config` → 404 (the Decision-12 split in one pair of tests).
   - `test_present_files_served` — anti-over-suppression: write `user-data` + `uaa.yaml` into the hexmac dir → both endpoints 200 with the exact bytes (the 404/empty guards do not block the happy path).
   - `test_executor_error_is_404` — executor `Err` → 404, no panic.
8. Bump the file header (version + last-edited) on every file you touch; new files get a fresh 4-line header with a new uuid4 (`uuidgen | tr 'A-F' 'a-f'`).

## How to test

```bash
cargo test --lib --offline && cargo build --offline
# Expected: all tests pass (baseline 311 + your new tests), build clean
cargo clippy --offline -- -D warnings
# Expected: no warnings
cargo test -p uaa-control --offline
# Expected: all uaa-control tests pass incl. the 7 tests above; 0 failed
grep -n "empty" crates/uaa-control/src/machine_plane/seeds.rs | head -3
# Expected: ≥1 hit (the empty-200 branch is explicit and commented)
```

## Acceptance criteria

- [ ] Decision-12 split proven by paired tests: `grep -n "test_missing_seed_file_empty_200\|test_missing_uaa_config_hard_404" crates/uaa-control/src/machine_plane/seeds.rs` → 2 hits and both pass.
- [ ] No direct process spawn: `grep -rn "process::Command\|Command::new" crates/uaa-control/src/machine_plane/seeds.rs` → 0 hits (executor seam only).
- [ ] Resolution parity: `grep -n "lladdr" crates/uaa-control/src/machine_plane/seeds.rs` → ≥1 hit (same regex family as Python `:191`).
- [ ] Anti-over-suppression: `test_present_files_served` passes — known MAC + present files still serve 200 with exact bytes through all guards.
- [ ] No secret/body logging: `grep -n "body\|contents" crates/uaa-control/src/machine_plane/seeds.rs | grep -i "log\|tracing\|info!\|warn!"` → 0 hits.
- [ ] `cargo test --lib --offline && cargo build --offline` exits 0; clippy clean.
- [ ] File headers bumped on every changed file (`git diff origin/main --stat` files each show a version bump; guids unchanged).

## Commit message

```
feat(control): /autoinstall/* seed-endpoint parity with empty-200 vs hard-404 split (ws3-parity)

Fills crates/uaa-control/src/machine_plane/seeds.rs (CT-01 stub): ip-neigh MAC
resolution through the CommandExecutor seam, four cloud-init seed endpoints
(missing FILE under an existing hexmac dir = empty 200) and /autoinstall/uaa-config
(same condition = hard 404, spec Decision 12). 7 tests: mock executor + tempdir
webroot, no network, no CRDB.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

If `grep -n "resolve_cloud_init_dir" crates/uaa-control/src/machine_plane/seeds.rs` hits, already applied — run the acceptance checks instead of re-applying. Rollback = revert the single commit; the CT-01 stub file returns to empty, all other machine_plane modules and the rest of the crate stay untouched.
