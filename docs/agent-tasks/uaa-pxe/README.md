<!-- file: docs/agent-tasks/uaa-pxe/README.md -->
<!-- version: 1.0.0 -->
<!-- guid: 6ac831ef-89c4-47c6-ab92-e46b4de84cdd -->
<!-- last-edited: 2026-07-10 -->

# Workstream — uaa-pxe (dnsmasq boot-config projection, PXE health, discovery inbox, DNS)

Build the NEW `crates/uaa-pxe` daemon (`:7446` gRPC mTLS, spec `docs/specs/constellation-design.md` §C5 / Decision 13): per-host boot config projected into `dhcp-hostsdir`/`dhcp-optsdir` files — NOT `/etc/dnsmasq.d` conf files, which dnsmasq only reads at startup so a test-then-reload write there silently no-ops — gated by `dnsmasq --test` before reload and verified after; dnsmasq/tftpd-hpa health with a TFTP self-probe; a passive discovery inbox following the dnsmasq journal; and optional per-host DNS A/PTR. From ws6-pxe.

**Execution mode:** SERIAL WAVES — PX-01 creates the crate, then PX-02/03/04 fill disjoint stubs in parallel — trigger: 3 disjoint filler tasks in local wave 2 (meets the ≥3 parallel threshold; local wave 1 is a single crate-creation task).

| Task | Src id | Title | Priority | Effort | Tier | Wave |
|------|--------|-------|----------|--------|------|------|
| TASK-01 | ws6-pxe | uaa-pxe crate: dhcp-hostsdir/optsdir projection (NOT conf.d), dnsmasq --test → reload → post-verify, SetupPxe/SetBootTarget | P1 | L | Sonnet-class | 6 |
| TASK-02 | ws6-pxe | Health: dnsmasq/tftpd unit state + TFTP self-probe + boot-target consistency verification | P2 | S | Sonnet-class | 7 |
| TASK-03 | ws6-pxe | Discovery inbox: journald dnsmasq follow → unknown-MAC extraction → UpsertDiscoveredMac → StreamDiscoveredMacs | P1 | M | Sonnet-class | 7 |
| TASK-04 | ws6-pxe | Optional DNS A/PTR per approved host via dedicated dnsmasq hosts file (same test-then-reload gate) | P3 | S | Haiku-class | 7 |

Waves are GLOBAL across the constellation plan (this workstream owns wave 6's `PX-01` alongside `WB-01`, and three of wave 7's six slots).

## Ground rules

- Rust only, exclusively inside `crates/uaa-pxe/**` (TASK-01 creates the crate + the three stubs; TASK-02/03/04 each fill EXACTLY ONE stub file plus its RPC arm in `main.rs`). Purely additive; root `Cargo.toml` is never edited (CP-01's `crates/*` members glob).
- Build + test gate for every task in this workstream:
  ```bash
  cargo test --lib --offline && cargo build --offline
  # Expected: all tests pass (baseline 311 + everything earlier waves added + this task's new tests; 0 failed)
  cargo clippy --offline -- -D warnings
  # Expected: no warnings
  ```
- **Verify every file:line anchor with `grep` before editing** — this workstream runs in global waves 6–7, after the CP-01 path move (`src/**` → `crates/uaa-core/src/**`) and dozens of merges; the grep hits are authoritative, line numbers are not. Zero hits at both the old and mapped path = STOP and report.
- File headers MANDATORY: new files get a fresh 4-line `// file: / // version: / // guid: / // last-edited:` header (uuid4 via `uuidgen | tr 'A-F' 'a-f'`); every edited file gets version bumped + `last-edited` updated, guid preserved.
- HARD RULES (operation contract, restated in every brief):
  - ALL dnsmasq/systemctl/journalctl interaction goes through the `CommandExecutor` mock seam — no live dnsmasq in tests, no `std::process::Command` anywhere in the crate.
  - NEVER reload on `dnsmasq --test` failure; per-host config goes ONLY into hostsdir/optsdir (SIGHUP-re-read), never conf.d.
  - NO hardware actions; NEVER wipe, write to, or deploy on 172.16.2.30 ("the server") or len-serv-003; NEVER power on unimatrixone.
  - No real secret in any file — `REPLACE_AT_PLACE_TIME` placeholders stay placeholders.
  - Workers stay in their worktree and NEVER push/PR/merge — the coordinator owns all git.

## Collision / wave note

From the plan collision matrix, the only row touching this workstream is the stub-pattern row: "stub-pattern (… uaa-pxe stubs by PX-01) — serialize: dependency-ordered (stub wave precedes fill wave); each stub file has EXACTLY ONE filling task." Concretely:

| Shared file | Creator (wave 6) | Exclusive filler (wave 7) |
|---|---|---|
| `crates/uaa-pxe/src/health.rs` | TASK-01 | TASK-02 |
| `crates/uaa-pxe/src/discovery_inbox.rs` | TASK-01 | TASK-03 |
| `crates/uaa-pxe/src/dns.rs` | TASK-01 | TASK-04 |

`crates/uaa-pxe/src/main.rs` is touched by all three wave-7 tasks (each replaces exactly one distinct `unimplemented` RPC arm) — trivially mergeable, but the coordinator rebases each sibling after every merge per protocol. TASK-01 itself is wave-6 gated on CP-02 (uaa-proto + workspace deps) and PK-03 (`crates/uaa-core/src/tls.rs`) being merged; its wave-6 peer `WB-01` is a disjoint new crate (`crates/uaa-web`), so both run concurrently.

Execution mode (from the skeleton, verbatim): "SERIAL WAVES — PX-01 creates the crate, then PX-02/03/04 fill disjoint stubs in parallel".

See [ORCHESTRATION.md](../ORCHESTRATION.md) for the coordinator + worker protocol.
