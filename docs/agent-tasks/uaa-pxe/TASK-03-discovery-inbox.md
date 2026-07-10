<!-- file: docs/agent-tasks/uaa-pxe/TASK-03-discovery-inbox.md -->
<!-- version: 1.0.0 -->
<!-- guid: e7e1c406-2257-4581-ade2-7640f6c99339 -->
<!-- last-edited: 2026-07-10 -->

# TASK-03 — Discovery inbox: journald dnsmasq follow → unknown-MAC extraction → UpsertDiscoveredMac → StreamDiscoveredMacs (ws6-pxe)

**Priority:** P1 · **Effort:** M · **Recommended subagent:** Sonnet-class · rust-services subagent · **Why:** P0 feature; log-parsing + stream plumbing with dedup/last-seen semantics. · **Depends on:** TASK-01 (wave-7 gated: PX-01 merged to `origin/main` — this task fills the `discovery_inbox.rs` stub PX-01 created; PX-02/04 are parallel-safe, disjoint stub files)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/uaa-pxe-discovery-inbox" -b agent/uaa-pxe-discovery-inbox origin/main
cd "$REPO/.worktrees/uaa-pxe-discovery-inbox"
git rebase origin/main
```

(Protocol also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

## Goal

Fill `crates/uaa-pxe/src/discovery_inbox.rs` (the PX-01 stub — this task is its EXCLUSIVE filler) with passive discovery (spec §C5): follow the dnsmasq journal via `journalctl -u dnsmasq -f -o json` through the executor seam, extract MACs from DHCP/proxy-DHCP request lines, drop MACs already known to the registry, dedup by MAC with `last_seen` update, forward each new/updated entry via `ControlService.UpsertDiscoveredMac` (spec proto §control.v1), and serve `PxeService.StreamDiscoveredMacs` to the SPA queue. There is NO passive discovery anywhere today (verified below) — purely additive.

REUSE — do not invent parallels for any of these:

- **`CommandExecutor`** trait (`crates/uaa-core/src/network/executor.rs`; pre-move `src/network/executor.rs` — verify: `grep -n "pub trait CommandExecutor" crates/uaa-core/src/network/executor.rs`) for the journalctl invocation. NO `std::process::Command`.
- **`normalize_mac`** from `crates/uaa-pxe/src/boot_config.rs` (PX-01) for every extracted MAC. Do NOT write a second MAC normalizer.
- **`uaa-proto`** (CP-02) for `UpsertDiscoveredMacRequest`, `DiscoveredMac`, `StreamDiscoveredMacsRequest`. Do NOT re-declare proto types.
- **Mock idiom:** the recording `MockExecutor` pattern from `crates/uaa-pxe/src/boot_config.rs` tests; the control client goes behind a local `trait ControlClient` implemented by the tonic client in prod and by a recording mock in tests. No mocking crate, no live gRPC in tests.

**Path map:** after CP-01 (wave 1) merges, `src/**` lives at `crates/uaa-core/src/**` and the CLI at `crates/uaa/src/**`. The greps below cite pre-move paths (verifiable on today's main); at execution time run them at the old path, then the mapped path. Zero hits at BOTH = STOP and report.

## Background (verify before editing)

- The server's dnsmasq runs with `log-dhcp` on (`unimatrixone-pxe-boot-status.md` ~line 588), so DHCP/proxy-DHCP requests land in the journal. `journalctl -o json` emits one JSON object per line; the log text is in the `MESSAGE` field.
- Lines to extract from (case-insensitive scan of `MESSAGE`): `DHCPDISCOVER(<iface>) <mac>`, `DHCPREQUEST(<iface>) <ip> <mac>`, and proxy-DHCP `... available DHCP subnet ... <mac>` / `PXE(<iface>) <mac> ...` shapes. Extraction rule (robust to dnsmasq phrasing drift): regex-scan the whole `MESSAGE` for the FIRST token matching `([0-9a-fA-F]{2}:){5}[0-9a-fA-F]{2}`, then `normalize_mac` it. A `MESSAGE` with no MAC token → skip silently (most journal lines are not DHCP events).
- **Streaming through a Result<String> seam:** `execute_with_output` returns the full output once, so a literal `-f` blocking follow cannot be consumed through it. Production loop: poll `journalctl -u dnsmasq -o json --show-cursor --after-cursor '<cursor>'` (first pass `--since -5min` instead of `--after-cursor`), parse the trailing `-- cursor: s=...` line to advance, sleep `poll_interval` (PxeConfig field, default 5s), repeat. Structure the code so the PURE part — `pub fn parse_journal_batch(batch: &str, known: &HashSet<String>, inbox: &mut Inbox) -> Vec<DiscoveredMacEvent>` — is fully tested against canned journal JSON; the loop is a thin driver. (Keep the doc-comment noting `-f -o json` is the semantic intent; the cursor poll is its executor-seam realization.)
- Edge semantics (here AND in acceptance): MAC already in the registry-known set → dropped BEFORE the inbox (unknown-MAC extraction only; the known set arrives via `ListMachines` through the `ControlClient` trait, refreshed each poll cycle, and a refresh FAILURE keeps the last snapshot — discovery must not die with control). MAC already in the inbox → NOT a new event; update `last_seen` (and `last_ip` if present) in place and DO forward an upsert (control owns persistence) but do NOT emit a duplicate stream item unless `last_seen` advanced ≥ `stream_requeue_secs` (PxeConfig, default 300). Malformed JSON line → skip + `tracing::debug`, never a crash. Upsert RPC failure → keep the entry, retry next cycle (at-least-once; control dedups by MAC).

**HARD RULES (non-negotiable):**
- NO hardware actions. Validate ONLY in-repo (`cargo`) and, where a brief says so, the QEMU+swtpm harness (`scripts/vm-validate.sh`). Code that COULD touch hardware is written and unit-tested against mock executors only.
- NEVER wipe, write to, or deploy on 172.16.2.30 ("the server") or len-serv-003.
- `disk_device` is read from the live target at runtime, never guessed or hardcoded.
- ipmitool runs via `ssh 172.16.2.30`, never on macOS.
- NEVER power on unimatrixone (U1).
- No real secret in any file: `REPLACE_AT_PLACE_TIME` placeholders stay placeholders.
- Stay inside your worktree; never `git push`, `gh pr`, or merge — report done and stop.

- **Re-verify these anchors before editing** — line numbers drift; zero hits at both old and mapped path = STOP and report:
  ```bash
  grep -rn "DHCPDISCOVER\|DiscoveredMac" src/ scripts/autoinstall-agent.py
  # expect: 0 hits (no passive discovery exists today; mapped: also 0 in crates/uaa-core/src/ — proto-generated code in crates/uaa-proto does not count)
  grep -n "log-dhcp" unimatrixone-pxe-boot-status.md
  # expect: 1+ hits (~line 588 — dnsmasq journal carries DHCP lines)
  grep -n "journalctl -u dnsmasq" unimatrixone-pxe-boot-status.md
  # expect: 1+ hits (~line 162 — the follow idiom used operationally)
  grep -n "TASK-03" crates/uaa-pxe/src/discovery_inbox.rs
  # expect: 1 hit (the PX-01 stub; absent file = PX-01 not merged, STOP)
  grep -n "pub fn normalize_mac" crates/uaa-pxe/src/boot_config.rs
  # expect: 1 hit (PX-01 artifact you reuse)
  grep -n "rpc StreamDiscoveredMacs\|rpc UpsertDiscoveredMac" proto/uaa/pxe/v1/pxe.proto proto/uaa/control/v1/control.proto
  # expect: 2 hits total (1 per file)
  grep -n "pub trait CommandExecutor" src/network/executor.rs
  # expect: 1 hit (mapped: crates/uaa-core/src/network/executor.rs)
  ```

## Step-by-step

1. Run the ⛔ START HERE block, then the anchor greps above. Any STOP condition → report and stop.
2. In `crates/uaa-pxe/src/discovery_inbox.rs`, define `pub struct DiscoveredEntry { pub mac: String, pub last_ip: Option<String>, pub first_seen: SystemTime, pub last_seen: SystemTime }` and `pub struct Inbox { entries: HashMap<String, DiscoveredEntry>, tx: tokio::sync::broadcast::Sender<DiscoveredMacEvent> }` (broadcast channel feeds `StreamDiscoveredMacs`; capacity from PxeConfig, default 256).
3. `pub fn extract_mac(message: &str) -> Option<String>` — the regex scan + `normalize_mac` from Background; `pub fn parse_journal_batch(...)` — split lines, `serde_json` each, `extract_mac(MESSAGE)`, drop known MACs, upsert into `Inbox` with the dedup/last_seen/requeue semantics from Background, return the events needing upstream forwarding.
4. Define `#[async_trait] pub trait ControlClient { async fn upsert_discovered_mac(&mut self, req: UpsertDiscoveredMacRequest) -> Result<(), tonic::Status>; async fn list_known_macs(&mut self) -> Result<HashSet<String>, tonic::Status>; }` + the prod impl wrapping the tonic `ControlServiceClient` (mTLS channel to `:7443` via `uaa_core::tls`).
5. `pub async fn run_follow_loop(executor, control, inbox, cfg)` — the cursor-poll driver from Background: refresh known set (failure → keep last snapshot + `tracing::warn`), fetch batch, `parse_journal_batch`, forward each returned event via `control.upsert_discovered_mac` (failure → retain, retry next cycle), publish stream items on the broadcast channel, advance cursor, sleep.
6. Implement the `StreamDiscoveredMacs` RPC arm in `main.rs` (replace the PX-01 `unimplemented` stub): on subscribe, first replay the current inbox snapshot, then live items from a `broadcast::Receiver` (map `RecvError::Lagged` to a skip + warn, not a stream kill). Spawn `run_follow_loop` from `main` alongside the server.
7. Unit tests (`#[cfg(test)]`, canned `journalctl -o json` fixture strings, recording MockExecutor + mock `ControlClient`): `test_extract_mac_from_dhcpdiscover` (a realistic `MESSAGE` with `DHCPDISCOVER(enp8s0f0) ac:1f:6b:40:fc:e2` → normalized MAC); `test_extract_mac_none_on_plain_line` (no MAC token → None); `test_known_mac_dropped` (MAC in known set → no inbox entry, no upsert recorded); `test_unknown_mac_flows_end_to_end` (**anti-over-suppression:** an unknown MAC survives the known-filter AND the dedup — exactly one upsert recorded, one stream event, inbox entry present); `test_dedup_updates_last_seen_no_duplicate_stream` (same MAC twice within requeue window → 1 stream event, 2 upserts, `last_seen` advanced); `test_malformed_json_line_skipped` (garbage line among valid ones → valid ones still processed); `test_upsert_failure_retries_next_cycle` (mock control errors once → entry retained, second cycle records the upsert).
8. Bump the file header (version + last-edited) on every file you touch; new files get a fresh 4-line header with a new uuid4 (`uuidgen | tr 'A-F' 'a-f'`).

## How to test

```bash
cargo test --lib --offline && cargo build --offline
# Expected: all tests pass (baseline 311 + earlier waves + your 7 new tests; 0 failed), build clean
cargo clippy --offline -- -D warnings
# Expected: no warnings
grep -rn "std::process::Command" crates/uaa-pxe/src/discovery_inbox.rs
# Expected: 0 hits
grep -n "journalctl -u dnsmasq" crates/uaa-pxe/src/discovery_inbox.rs
# Expected: 1+ hits (the executor-mediated invocation)
```

## Acceptance criteria

- [ ] Journal follow is executor-mediated: `grep -n "journalctl -u dnsmasq" crates/uaa-pxe/src/discovery_inbox.rs` → hits; `grep -rn "std::process::Command" crates/uaa-pxe/src/discovery_inbox.rs` → 0 hits.
- [ ] MAC handling reuses PX-01: `grep -n "normalize_mac" crates/uaa-pxe/src/discovery_inbox.rs` → ≥1 hit (no second normalizer defined: `grep -c "fn normalize_mac" crates/uaa-pxe/src/discovery_inbox.rs` → 0).
- [ ] Known-MAC filter + dedup proven: `test_known_mac_dropped` and `test_dedup_updates_last_seen_no_duplicate_stream` pass.
- [ ] **Anti-over-suppression:** `test_unknown_mac_flows_end_to_end` passes — the known-filter + dedup guards do not swallow a genuinely new MAC (1 upsert, 1 stream event asserted).
- [ ] Resilience proven: `test_malformed_json_line_skipped` and `test_upsert_failure_retries_next_cycle` pass (no crash paths, at-least-once forwarding).
- [ ] Only assigned files changed: `git diff origin/main --stat` touches `crates/uaa-pxe/src/discovery_inbox.rs`, `crates/uaa-pxe/src/main.rs`, and headers only.
- [ ] `cargo test --lib --offline && cargo build --offline` exits 0; clippy clean.
- [ ] File headers bumped on every changed file (`git diff origin/main --stat` files each show a version bump; guids unchanged).

## Commit message

```
feat(pxe): discovery inbox — dnsmasq journal follow, unknown-MAC upsert, SPA stream (ws6-pxe)

Fills the PX-01 discovery_inbox.rs stub: cursor-polled `journalctl -u
dnsmasq -o json` through the CommandExecutor seam, regex MAC extraction +
boot_config::normalize_mac, registry-known filter (snapshot kept on control
outage), per-MAC dedup with last_seen update, at-least-once
ControlService.UpsertDiscoveredMac forwarding, and broadcast-backed
StreamDiscoveredMacs with snapshot replay. 7 unit tests against canned
journal JSON + mock control client.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

Additive (stub-fill): if `grep -n "pub fn parse_journal_batch" crates/uaa-pxe/src/discovery_inbox.rs` hits, the task is already applied — run the Acceptance criteria checks instead of re-applying. Rollback = revert the single commit; `discovery_inbox.rs` returns to the PX-01 stub and the stream RPC arm to `unimplemented` — `boot_config.rs`, `health.rs`, `dns.rs`, and the registry are untouched (the inbox is in-memory only; no persisted state to unwind).
