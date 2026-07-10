<!-- file: docs/agent-tasks/uaa-pxe/TASK-01-pxe-crate-boot-config.md -->
<!-- version: 1.0.0 -->
<!-- guid: 6e61fc07-3918-4fd8-98ad-089f142029da -->
<!-- last-edited: 2026-07-10 -->

# TASK-01 — uaa-pxe crate: dhcp-hostsdir/optsdir projection (NOT conf.d), dnsmasq --test → reload → post-verify, SetupPxe/SetBootTarget (ws6-pxe)

**Priority:** P1 · **Effort:** L · **Recommended subagent:** Sonnet-class · rust-services subagent · **Why:** the SIGHUP-reload subtlety is the safety property (spec Decision 13 repair) — the projection mechanism must be impossible to get wrong. · **Depends on:** none inside this workstream (wave-6 gated: CP-02 [workspace deps + uaa-proto] and PK-03 [`crates/uaa-core/src/tls.rs` mTLS helpers] must be MERGED to `origin/main` first; waves 1–5 of the constellation plan are complete before this dispatches)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/uaa-pxe-pxe-crate-boot-config" -b agent/uaa-pxe-pxe-crate-boot-config origin/main
cd "$REPO/.worktrees/uaa-pxe-pxe-crate-boot-config"
git rebase origin/main
```

(Protocol also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

## Goal

Create the NEW crate `crates/uaa-pxe` (spec `docs/specs/constellation-design.md` §C5, Decision 13): a gRPC mTLS daemon on `:7446` implementing `PxeService.SetupPxe` and `PxeService.SetBootTarget` from `proto/uaa/pxe/v1/pxe.proto` (built by CP-02's `uaa-proto` crate). Per-host boot config is PROJECTED into `dhcp-hostsdir`/`dhcp-optsdir` files — never `/etc/dnsmasq.d/*.conf` — with a `dnsmasq --test` gate before reload and a post-reload verification. Also create the three headered STUB files `crates/uaa-pxe/src/health.rs`, `crates/uaa-pxe/src/discovery_inbox.rs`, `crates/uaa-pxe/src/dns.rs` that TASK-02/03/04 fill EXCLUSIVELY (one filler per stub — the de-collision pattern; put only the module doc-comment, the file header, and `// Filled by uaa-pxe/TASK-NN` in each).

REUSE — do not invent parallels for any of these:

- **`CommandExecutor`** trait (`crates/uaa-core/src/network/executor.rs` after CP-01; pre-move `src/network/executor.rs` — verify: `grep -n "pub trait CommandExecutor" crates/uaa-core/src/network/executor.rs`). ALL dnsmasq/systemctl interaction goes through `&mut dyn CommandExecutor` so tests inject a mock. Do NOT shell out via `std::process::Command` anywhere in this crate.
- **`crates/uaa-core/src/tls.rs`** (PK-03) for the mTLS server config (service cert `/var/lib/uaa/certs/uaa-pxe.{key,crt}` per spec Decision 23, CRL enforcement per Decision 25). Do NOT hand-roll rustls config.
- **`uaa-proto`** (CP-02) for `PxeService` server codegen. Do NOT re-declare proto types.
- **Mock idiom:** mirror `MockExecutor` (`crates/uaa-core/src/autoinstall/verify.rs`, pre-move `src/autoinstall/verify.rs` — a HashMap command→response mock) inside `#[cfg(test)]`, extended with a `Vec<String>` of recorded commands. Do NOT add a mocking crate.

**Path map:** after CP-01 (wave 1) merges, `src/**` lives at `crates/uaa-core/src/**` and the CLI at `crates/uaa/src/**`. The greps below cite pre-move paths (verifiable on today's main); at execution time run them at the old path, then the mapped path. Zero hits at BOTH = STOP and report.

## Background (verify before editing)

- **WHY hostsdir/optsdir and NOT conf.d (spec Decision 13, verbatim safety property):** dnsmasq re-reads `dhcp-hostsdir`/`dhcp-optsdir` directories on SIGHUP (i.e. `systemctl reload dnsmasq`), but `conf-dir` (`/etc/dnsmasq.d/*.conf`) files are read ONLY at startup. A per-host boot-config write into conf.d followed by test-then-reload SILENTLY NO-OPS until the next full restart — the host boots the stale target and the reinstall silently doesn't happen. This crate therefore writes ONLY into the configured hostsdir/optsdir paths, and verifies after reload rather than assuming the reload took.
- `machines.boot_target` is the ONE authoritative field (`local-disk | custom-autoinstall | pxe-disabled | pxe-grub`); this crate is a PROJECTION of it (spec Decision 13, schema in spec §Data model). uaa-control reconciles; this crate never decides targets, it applies them.
- The server's dnsmasq today is proxy-DHCP only with `log-dhcp` on (`/etc/dnsmasq.d/ubuntu-netboot.conf`), and TFTP is served by tftpd-hpa, NOT dnsmasq — ground truth in `unimatrixone-pxe-boot-status.md`. The daemon config (`PxeConfig`) must carry hostsdir/optsdir as PATHS (defaults `/etc/uaa/dnsmasq-hosts.d` and `/etc/uaa/dnsmasq-opts.d`) so tests use a tempdir; enabling those dirs in the live dnsmasq config (`dhcp-hostsdir=`/`dhcp-optsdir=` lines) is a documented DEPLOY step, not code in this task.
- Edge semantics (spelled here AND in acceptance): unknown/missing MAC in a request → typed InvalidArgument error, no file written, no reload. `local-disk` → REMOVE both per-host files (absence = default flow); removal of already-absent files is Ok (idempotent). `dnsmasq --test` failure → the just-written tmp files are discarded, the live files are untouched, NO reload happens, error returned. Reload failure → error, but files stay (they are valid; operator retries reload).

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
  grep -in "proxy.dhcp\|arch matches" unimatrixone-pxe-boot-status.md | head -5
  # expect: hits (dnsmasq proxy-DHCP + arch matching documented, ~lines 460/588/643)
  grep -n "dhcp-hostsdir" docs/specs/constellation-design.md
  # expect: 2+ hits (Decision 13 + §C5)
  grep -n "pub trait CommandExecutor" src/network/executor.rs
  # expect: 1 hit (mapped: crates/uaa-core/src/network/executor.rs)
  grep -n "async fn execute_with_output" src/network/executor.rs
  # expect: 1+ hits (mapped path as above)
  grep -n "struct MockExecutor" src/autoinstall/verify.rs
  # expect: 1 hit (mapped: crates/uaa-core/src/autoinstall/verify.rs)
  grep -n "rpc SetupPxe\|rpc SetBootTarget" proto/uaa/pxe/v1/pxe.proto
  # expect: 2 hits (CP-02 artifact; absent = CP-02 not merged, STOP)
  grep -rn "pub" crates/uaa-core/src/tls.rs | head -5
  # expect: hits (PK-03 artifact; absent = PK-03 not merged, STOP)
  ```

## Step-by-step

1. Run the ⛔ START HERE block, then the anchor greps above. Any STOP condition → report and stop.
2. **Create `crates/uaa-pxe/Cargo.toml`**: package `uaa-pxe`, binary target, deps ONLY as `workspace = true` references (tonic, prost, tokio, tokio-rustls, serde, tracing, thiserror — all pre-populated by CP-02; adding a version literal here is a defect) plus path deps `uaa-core`, `uaa-proto`. Root `Cargo.toml` needs NO edit (members glob `crates/*` from CP-01).
3. **Create `crates/uaa-pxe/src/main.rs`**: load `PxeConfig` (serde, from `/etc/uaa/uaa-pxe.yaml`, `#[serde(deny_unknown_fields)]`, fields `listen_addr` default `0.0.0.0:7446`, `hostsdir`, `optsdir`, `cert_path`, `key_path`, `ca_path`, `crl_path`); build the mTLS server via `uaa_core::tls` helpers; serve `PxeService`. `mod boot_config; mod health; mod discovery_inbox; mod dns;`. `Health`/`StreamDiscoveredMacs`/`SetDnsRecord` RPC arms return `tonic::Status::unimplemented("filled by uaa-pxe TASK-02/03/04")` for now.
4. **Create `crates/uaa-pxe/src/boot_config.rs`** — the whole projection lives here:
   - `pub fn normalize_mac(s: &str) -> Result<String, PxeError>` → lowercase `aa:bb:cc:dd:ee:ff`; accepts `-`/`:`/bare-hex input; anything else → `PxeError::InvalidMac`. `pub fn mac_file_stem(mac: &str) -> String` → `uaa-aabbccddeeff`.
   - `pub enum BootTarget { LocalDisk, CustomAutoinstall, PxeDisabled, PxeGrub }` with `FromStr` accepting exactly the four registry strings `local-disk|custom-autoinstall|pxe-disabled|pxe-grub` (anything else = error — the registry enum is closed, spec Decision 13).
   - `pub fn render_host_lines(mac: &str, target: BootTarget, req: &SetBootTargetRequest) -> Option<(String, String)>` returning `(hostsdir_content, optsdir_content)` — pure, fully unit-testable:
     - `LocalDisk` → `None` (projection = remove both files).
     - `PxeDisabled` → hostsdir `dhcp-host=<mac>,ignore\n`, optsdir empty string.
     - `CustomAutoinstall` → hostsdir `dhcp-host=<mac>,set:<stem>\n`; optsdir `tag:<stem>,option:bootfile-name,<boot_file from request>\n` (boot_file is the host-specific iPXE path uaa-web placed; empty boot_file → `PxeError::InvalidArgument`, surface via a `Result` wrapper).
     - `PxeGrub` → same shape with the request's grub netboot path (e.g. `grubnetx64.efi`).
   - `pub fn write_atomic(path: &Path, content: &str) -> Result<(), PxeError>` — tmp file in the SAME directory + `fs::rename` (atomic on same fs), mode 0644. ALL file writes in this crate go through it.
   - `pub async fn apply_boot_target(executor: &mut dyn CommandExecutor, cfg: &PxeConfig, mac: &str, target: BootTarget, req: &SetBootTargetRequest) -> Result<AppliedTarget, PxeError>` — the ONLY mutation path, in this exact order:
     1. normalize MAC (fail-closed before any I/O);
     2. render lines; for `LocalDisk` remove `<hostsdir>/<stem>.conf` and `<optsdir>/<stem>.conf` (missing = Ok), skip to step 5;
     3. write BOTH files via `write_atomic`;
     4. gate: `executor.execute_with_output("dnsmasq --test")` — on Err, DELETE the two just-written files (restore prior state: if a previous version existed, you overwrote it — so first copy any existing file aside in step 3 and restore it here), return `PxeError::ConfigRejected` with dnsmasq's stderr, and NEVER reload (`shared_state` rule: never reload on --test failure);
     5. reload: `executor.execute("systemctl reload dnsmasq")` (SIGHUP — this is exactly what re-reads hostsdir/optsdir; a restart is NOT used, it would drop in-flight PXE fetches);
     6. post-verify: re-read both files from disk and compare byte-for-byte to the rendered intent, and `executor.check_silent("systemctl is-active --quiet dnsmasq")` must be true; mismatch/inactive → `PxeError::VerifyFailed` (the richer boot-target consistency probe lands in TASK-02's `health.rs`).
   - `SetupPxe` = whole-host setup (validates request, calls `apply_boot_target`); `SetBootTarget` = target flip only; both RPC handlers map `PxeError` → `tonic::Status` (`InvalidMac`/`InvalidArgument` → `invalid_argument`, `ConfigRejected`/`VerifyFailed` → `failed_precondition`, I/O → `internal`).
5. **Create the three stubs** `src/health.rs`, `src/discovery_inbox.rs`, `src/dns.rs`: 4-line header each (fresh uuid4 via `uuidgen | tr 'A-F' 'a-f'`), one doc-comment naming its filler task, no code beyond an empty `#[allow(dead_code)]` marker if needed for clippy.
6. **Unit tests** (`#[cfg(test)]` in `boot_config.rs`, tempdir for hostsdir/optsdir, recording MockExecutor): `test_normalize_mac_forms` (3 input forms + reject garbage); `test_render_all_four_targets` (exact line content per target incl. `LocalDisk → None`); `test_apply_writes_then_tests_then_reloads` — recorded command order is exactly `dnsmasq --test` then `systemctl reload dnsmasq`, files exist with rendered content (anti-over-suppression: a valid target passes every gate and the reload REALLY happens); `test_apply_test_failure_no_reload_files_restored` — mock fails `dnsmasq --test`: zero reload commands recorded, pre-existing file content restored byte-for-byte; `test_local_disk_removes_files_idempotent` (second call with files absent still Ok, still reloads); `test_invalid_mac_no_io` (nothing written, zero commands recorded); `test_conf_d_never_referenced` — no rendered path or command string contains `dnsmasq.d`.
7. Bump the file header (version + last-edited) on every file you touch; new files get a fresh 4-line header with a new uuid4 (`uuidgen | tr 'A-F' 'a-f'`).

## How to test

```bash
cargo test --lib --offline && cargo build --offline
# Expected: all tests pass (baseline 311 + everything waves 1-5 added + your 7 new tests; 0 failed), build clean
cargo clippy --offline -- -D warnings
# Expected: no warnings
grep -rn "dnsmasq.d" crates/uaa-pxe/src/
# Expected: 0 hits in code paths (comments explaining WHY conf.d is banned are allowed — code strings are not)
grep -rn "std::process::Command" crates/uaa-pxe/src/
# Expected: 0 hits (all external commands via CommandExecutor)
grep -rn "systemctl restart dnsmasq" crates/uaa-pxe/src/
# Expected: 0 hits (reload/SIGHUP only)
```

## Acceptance criteria

- [ ] Crate + stubs exist: `test -f crates/uaa-pxe/src/boot_config.rs && test -f crates/uaa-pxe/src/health.rs && test -f crates/uaa-pxe/src/discovery_inbox.rs && test -f crates/uaa-pxe/src/dns.rs` succeeds; each stub names its filler task (`grep -l "TASK-0" crates/uaa-pxe/src/health.rs crates/uaa-pxe/src/discovery_inbox.rs crates/uaa-pxe/src/dns.rs` → 3 files).
- [ ] Projection targets hostsdir/optsdir only: `grep -rn "dnsmasq.d" crates/uaa-pxe/src/ | grep -v "^.*//"` → 0 hits; `grep -n "hostsdir\|optsdir" crates/uaa-pxe/src/boot_config.rs` → hits.
- [ ] Fail-closed gate proven: `grep -n "test_apply_test_failure_no_reload_files_restored" crates/uaa-pxe/src/boot_config.rs` → 1 hit and the test asserts zero reload commands recorded.
- [ ] **Anti-over-suppression:** `test_apply_writes_then_tests_then_reloads` passes — a valid target flows through the --test gate and the recorded reload command is present (the gate does not block the happy path).
- [ ] All four `BootTarget` values covered incl. `local-disk` removal: `grep -c "LocalDisk\|CustomAutoinstall\|PxeDisabled\|PxeGrub" crates/uaa-pxe/src/boot_config.rs` ≥ 8 (enum + render arms + tests).
- [ ] No direct process spawning: `grep -rn "std::process::Command" crates/uaa-pxe/src/` → 0 hits.
- [ ] `cargo test --lib --offline && cargo build --offline` exits 0; clippy clean.
- [ ] File headers bumped on every changed file (`git diff origin/main --stat` files each show a version bump; guids unchanged; new files carry fresh uuid4 headers).

## Commit message

```
feat(pxe): add uaa-pxe crate with hostsdir/optsdir boot-config projection (ws6-pxe)

New crates/uaa-pxe daemon (:7446 gRPC mTLS via uaa-core tls helpers):
SetupPxe/SetBootTarget project machines.boot_target into dhcp-hostsdir/
dhcp-optsdir files (NOT conf.d — SIGHUP re-reads only those dirs, spec
Decision 13), gated by dnsmasq --test before systemctl reload and verified
after. Atomic tmp+rename writes; all dnsmasq/systemctl calls through the
CommandExecutor mock seam. Stubs health.rs/discovery_inbox.rs/dns.rs for
TASK-02/03/04. 7 unit tests incl. fail-closed no-reload-on-test-failure.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

Additive: if `grep -n "pub async fn apply_boot_target" crates/uaa-pxe/src/boot_config.rs` hits, the task is already applied — run the Acceptance criteria checks instead of re-applying. Rollback = revert the single commit; it removes the whole `crates/uaa-pxe/` directory cleanly (root `Cargo.toml` was never edited thanks to the `crates/*` members glob), and no server, dnsmasq, or filesystem state exists outside the repo to unwind — the daemon is never deployed by this task.
