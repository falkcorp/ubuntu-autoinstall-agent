<!-- file: docs/agent-tasks/uaa-pxe/TASK-04-dns-records.md -->
<!-- version: 1.0.0 -->
<!-- guid: b7f38bce-1898-453d-a2b5-7c77a7bdb37d -->
<!-- last-edited: 2026-07-10 -->

# TASK-04 — Optional DNS A/PTR per approved host via dedicated dnsmasq hosts file (same test-then-reload gate) (ws6-pxe)

**Priority:** P3 · **Effort:** S · **Recommended subagent:** Haiku-class · rust-services subagent · **Why:** mechanical mirror of the PX-01 file-projection pattern. · **Depends on:** TASK-01 (wave-7 gated: PX-01 merged to `origin/main` — this task fills the `dns.rs` stub PX-01 created; PX-02/03 are parallel-safe, disjoint stub files)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/uaa-pxe-dns-records" -b agent/uaa-pxe-dns-records origin/main
cd "$REPO/.worktrees/uaa-pxe-dns-records"
git rebase origin/main
```

(Protocol also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

## Goal

Fill `crates/uaa-pxe/src/dns.rs` (the PX-01 stub — this task is its EXCLUSIVE filler) with the P2 optional-DNS feature (spec §C5: "DNS A/PTR (P2) uses a dedicated hosts file, same gate"; proto `rpc SetDnsRecord(SetDnsRecordRequest) returns (Ack)`): per approved host, write an `<ip> <hostname>` entry into ONE dedicated dnsmasq additional-hosts file, then run the EXACT test-then-reload-then-verify gate PX-01 built. dnsmasq re-reads `addn-hosts` files on SIGHUP (same property that makes hostsdir/optsdir safe — and the same reason conf.d is banned), and serves both the A record and the reverse PTR from that single hosts-file line.

REUSE — do not invent parallels for any of these:

- **The PX-01 gate**: `write_atomic` and the test→reload→verify sequence from `crates/uaa-pxe/src/boot_config.rs` (verify: `grep -n "pub fn write_atomic" crates/uaa-pxe/src/boot_config.rs`). If PX-01 exposed the gate as a reusable helper (e.g. a `test_then_reload`-shaped fn), CALL it; if it is inlined in `apply_boot_target`, extract it into a shared `pub(crate) async fn gate_and_reload(executor, ...)` in `boot_config.rs` (a minimal, mechanical extraction — behavior identical, PX-01's tests must stay green) and call it from both sites. Do NOT copy-paste a second gate.
- **`CommandExecutor`** trait (`crates/uaa-core/src/network/executor.rs`; pre-move `src/network/executor.rs`) for `dnsmasq --test` / `systemctl reload dnsmasq`. NO `std::process::Command`.
- **Mock idiom:** the recording `MockExecutor` from `boot_config.rs` tests. No mocking crate.

**Path map:** after CP-01 (wave 1) merges, `src/**` lives at `crates/uaa-core/src/**` and the CLI at `crates/uaa/src/**`. The greps below cite pre-move paths (verifiable on today's main); at execution time run them at the old path, then the mapped path. Zero hits at BOTH = STOP and report.

## Background (verify before editing)

- One file, whole-file rewrite: `PxeConfig` gains `dns_hosts_file` (default `/etc/uaa/dnsmasq-uaa.hosts`). `SetDnsRecord` upserts/removes ONE `<ip> <hostname>` line keyed by hostname, then rewrites the WHOLE file (sorted by hostname, one line each) via `write_atomic` — no in-place patching. Enabling `addn-hosts=/etc/uaa/dnsmasq-uaa.hosts` in the live dnsmasq config is a documented DEPLOY step, not code.
- PTR comes for free: dnsmasq synthesizes the reverse (`in-addr.arpa` / `ip6.arpa`) record from the same hosts-file line — there is NO separate PTR write path, and the brief must not invent one. One line therefore IS "A/PTR per approved host" from the spec topology row.
- "Per approved host" is enforced upstream: uaa-control only calls `SetDnsRecord` for `status = approved` machines (approve-SAGA / reconciliation). This crate does NOT check approval state — it has no registry access by design; do not add a registry client here.
- Edge semantics (here AND in acceptance): request with `remove: true` for an absent hostname → Ok, no-op, NO reload (nothing changed). Upsert with identical existing line → Ok, no-op, NO reload. Invalid IPv4/IPv6 (parse via `std::net::IpAddr`) or hostname not matching `^[a-z0-9]([a-z0-9-]*[a-z0-9])?(\.[a-z0-9]([a-z0-9-]*[a-z0-9])?)*$` → typed InvalidArgument, file untouched, no reload. Same hostname new IP → replace the line (one line per hostname). `dnsmasq --test` failure → restore the previous file content, no reload, error (identical fail-closed shape as PX-01).

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
  grep -n "A/PTR" docs/specs/constellation-design.md
  # expect: 2+ hits (topology row, proto comment, §C5)
  grep -n "TASK-04" crates/uaa-pxe/src/dns.rs
  # expect: 1 hit (the PX-01 stub; absent file = PX-01 not merged, STOP)
  grep -n "pub fn write_atomic" crates/uaa-pxe/src/boot_config.rs
  # expect: 1 hit (PX-01 artifact you reuse)
  grep -n "rpc SetDnsRecord" proto/uaa/pxe/v1/pxe.proto
  # expect: 1 hit
  grep -n "pub trait CommandExecutor" src/network/executor.rs
  # expect: 1 hit (mapped: crates/uaa-core/src/network/executor.rs)
  ```

## Step-by-step

1. Run the ⛔ START HERE block, then the anchor greps above. Any STOP condition → report and stop.
2. In `crates/uaa-pxe/src/dns.rs`: `pub fn validate_record(ip: &str, hostname: &str) -> Result<(IpAddr, String), PxeError>` (rules from Background, hostname lowercased); `pub fn render_hosts_file(records: &BTreeMap<String, IpAddr>) -> String` (sorted `<ip> <hostname>\n` lines — pure, testable); `pub fn parse_hosts_file(content: &str) -> BTreeMap<String, IpAddr>` (tolerant: skip blank/comment lines).
3. `pub async fn apply_dns_record(executor: &mut dyn CommandExecutor, cfg: &PxeConfig, ip: &str, hostname: &str, remove: bool) -> Result<DnsApplied, PxeError>`: validate (fail-closed before I/O) → read+parse current file (missing file = empty map) → mutate map → if unchanged, return `DnsApplied::NoChange` WITHOUT touching disk or the executor → else `write_atomic` the re-rendered file → run the shared PX-01 gate (test → reload → verify: re-read file matches render AND `systemctl is-active --quiet dnsmasq`); on `--test` failure restore prior content, no reload.
4. Implement the `SetDnsRecord` RPC arm in `main.rs` (replace the PX-01 `unimplemented` stub): map `PxeError` → `tonic::Status` exactly as `boot_config.rs` does.
5. Unit tests (`#[cfg(test)]`, tempdir + recording MockExecutor): `test_validate_rejects_bad_ip_and_hostname` (garbage IP, `-leading` hostname → Err, zero commands recorded); `test_render_parse_roundtrip` (2 records → render → parse → equal map, sorted output); `test_upsert_writes_tests_reloads` (**anti-over-suppression:** a valid new record passes validation + no-change guard and the recorded commands are exactly `dnsmasq --test` then `systemctl reload dnsmasq`, file contains `<ip> <hostname>`); `test_noop_upsert_no_reload` (identical record twice → second call records zero commands); `test_remove_absent_noop` (remove unknown hostname → Ok, zero commands); `test_test_failure_restores_no_reload` (mock fails `--test` → prior file bytes restored, zero reload commands); `test_same_hostname_new_ip_replaces` (one line per hostname).
6. Bump the file header (version + last-edited) on every file you touch; new files get a fresh 4-line header with a new uuid4 (`uuidgen | tr 'A-F' 'a-f'`).

## How to test

```bash
cargo test --lib --offline && cargo build --offline
# Expected: all tests pass (baseline 311 + earlier waves + your 7 new tests; 0 failed), build clean
cargo clippy --offline -- -D warnings
# Expected: no warnings
grep -rn "std::process::Command" crates/uaa-pxe/src/dns.rs
# Expected: 0 hits
grep -c "dnsmasq --test" crates/uaa-pxe/src/dns.rs
# Expected: 0 (the gate is CALLED from boot_config.rs, not duplicated — literal appears only in boot_config.rs)
grep -rn "in-addr.arpa\|ptr-record" crates/uaa-pxe/src/dns.rs
# Expected: 0 hits in code (dnsmasq synthesizes PTR from the hosts line; no separate PTR path exists)
```

## Acceptance criteria

- [ ] Single shared gate: `grep -rn "dnsmasq --test" crates/uaa-pxe/src/ | grep -v boot_config.rs | grep -v "^.*://"` → 0 code hits outside `boot_config.rs` (dns.rs calls the shared helper); `grep -n "write_atomic" crates/uaa-pxe/src/dns.rs` → ≥1 hit.
- [ ] Fail-closed + no-op semantics proven: `test_test_failure_restores_no_reload`, `test_noop_upsert_no_reload`, `test_remove_absent_noop` all pass with zero-command assertions.
- [ ] **Anti-over-suppression:** `test_upsert_writes_tests_reloads` passes — a legitimate record clears validation and the no-change guard, and the reload really happens.
- [ ] One line per hostname: `test_same_hostname_new_ip_replaces` passes.
- [ ] Only assigned files changed: `git diff origin/main --stat` touches `crates/uaa-pxe/src/dns.rs`, `crates/uaa-pxe/src/main.rs`, at most a mechanical gate-extraction in `crates/uaa-pxe/src/boot_config.rs` (its existing tests still green), and headers.
- [ ] `cargo test --lib --offline && cargo build --offline` exits 0; clippy clean.
- [ ] File headers bumped on every changed file (`git diff origin/main --stat` files each show a version bump; guids unchanged).

## Commit message

```
feat(pxe): optional DNS A/PTR via dedicated addn-hosts file with shared reload gate (ws6-pxe)

Fills the PX-01 dns.rs stub: SetDnsRecord upserts/removes one
"<ip> <hostname>" line in a dedicated dnsmasq additional-hosts file
(whole-file atomic rewrite, sorted, one line per hostname) and reuses the
boot_config test->reload->verify gate — dnsmasq re-reads addn-hosts on
SIGHUP, same Decision-13 property as hostsdir/optsdir. No-op upserts and
absent-removes skip the reload entirely. 7 unit tests, mock executor only.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

Additive (stub-fill): if `grep -n "pub async fn apply_dns_record" crates/uaa-pxe/src/dns.rs` hits, the task is already applied — run the Acceptance criteria checks instead of re-applying. Rollback = revert the single commit; `dns.rs` returns to the PX-01 stub, the `SetDnsRecord` arm to `unimplemented`, and any gate-extraction hunk in `boot_config.rs` reverts with it (PX-01 behavior unchanged either way) — no hosts file exists anywhere outside tests to clean up.
