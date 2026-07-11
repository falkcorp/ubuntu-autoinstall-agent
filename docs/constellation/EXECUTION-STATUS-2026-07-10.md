<!-- file: docs/constellation/EXECUTION-STATUS-2026-07-10.md -->
<!-- version: 1.0.0 -->
<!-- guid: e5f2a1c4-8b3d-4e6f-9a1b-2c3d4e5f6a7b -->
<!-- last-edited: 2026-07-10 -->

# Constellation Execution Status — 2026-07-10

Status snapshot of the autonomous coordinator run executing the
`docs/agent-tasks/{core-proto,control,install-plane,pki,uaa-web,uaa-pxe,
luks-keys,remote-power,tooling-port,testing-gates}/` planning package
(PR #51). Written per user request to preserve state and minimize token
spend on mechanical bookkeeping while sub-agent capacity is limited.

## Executive summary

23 PRs (#52–#74) merged to `main`, taking the workspace from a single crate
(311 tests) to a 5-crate cargo workspace (`uaa`, `uaa-core`, `uaa-proto`,
`uaa-control`, plus scaffolding for `uaa-web`/`uaa-pxe`) with **560 passing
tests**, clippy-clean throughout. This delivers, in order:

1. **Foundation (wave 1–2, 10 tasks)**: the workspace conversion itself
   (⚠ reviewed), the protobuf/gRPC crate, fleet configuration
   parameterization, mDNS discovery, signed self-update, and the CI/musl
   build matrix — the scaffolding every other service depends on.
2. **Tooling parity (wave 2, 4 tasks)**: Rust ports of the shell-based ISO
   remastering, secret-injection config placement (⚠ reviewed — handles
   real credentials), image building, and the QEMU+swtpm VM validation
   harness — replacing brittle bash with tested, mockable Rust.
3. **Power management (wave 3, 2 tasks)**: AMD DASH and Intel AMT/Wake-on-LAN
   remote power paths, completing the power story alongside the pre-existing
   IPMI path.
4. **LUKS key management (wave 3, 2 tasks)**: FIDO2 YubiKey enrollment and
   safety-critical key rotation with a Tang t=2-of-3 cold-start quorum guard
   (⚠ reviewed — this is disk-encryption keyslot management, where a bug
   means data loss).
5. **The registry crate (wave 3, 1 task, ⚠ reviewed)**: `uaa-control`, the
   new service holding the CockroachDB-backed system of record, with a
   crash-safe snapshot+WAL degraded-mode layer so the fleet keeps functioning
   during a database outage.
6. **Control-plane business logic (wave 4, 9 tasks)**: registry CRUD,
   GitHub OAuth + RBAC, a tamper-evident hash-chained audit log, the
   multi-step approve/reinstall sagas, and full parity with the legacy
   Python install-plane server's HTTP endpoints (seed serving, checkin,
   webhooks, machine inventory) plus the new install-CA and certificate
   enrollment service.
7. **Two rounds of security hardening**: three vulnerabilities were caught
   by automated commit review and fixed in-line — a shell command-injection
   bug in LUKS device paths, a path-traversal bug that let a malicious
   webhook write files outside the intended directory, an OAuth login-CSRF
   gap, and an argv flag-smuggling bug in a `cockroach cert` shell-out. All
   four are fixed, tested, and merged.

**Why this matters**: the fleet's install/provisioning system today is a
single Python HTTP server with no authentication, no audit trail, and no
tested error handling. This work replaces it with a typed, tested,
security-reviewed Rust service — while every change so far has been
additive (the legacy Python server is untouched and still running; nothing
in this batch has been cut over).

## Completed and merged (main @ c7916e6, 560 tests, clippy clean)

| # | PR | Task | What it does |
|---|----|------|---------------|
| 1 | #52 | CP-01 ⚠ | Cargo workspace conversion; stub modules for all later tasks |
| 2 | #53 | TG-04 | `constellation-ci.yml`: workspace clippy+test+SPA build |
| 3 | #54 | CP-06 | musl CI matrix: build+verify every workspace binary |
| 4 | #55 | LK-01 | `uaa luks` enroll/status (FIDO2+PIN, mock-tested) |
| 5 | #56 | fix | Hardened `validate_luks_dev` against shell injection |
| 6 | #57 | CP-02 | `uaa-proto` crate (tonic/prost via protox) + workspace deps |
| 7 | #58 | CP-03 | `FleetConfig`: parameterize hardcoded fleet constants |
| 8 | #59 | TP-01 | `uaa iso remaster` (xorriso port, mock-tested) |
| 9 | #60 | TP-02 ⚠ | `uaa config place/--inject-from` (secret injection) |
| 10 | #61 | CT-08 | React+Vite SPA scaffold |
| 11 | #62 | TP-04 | `uaa vm-validate` (QEMU+swtpm harness port) |
| 12 | #63 | TP-03 | `uaa image build` (squashfs overlay port) |
| 13 | #64 | CP-04 | mDNS + static-fallback discovery |
| 14 | #65 | CP-05 | Signed self-update (dual-key ed25519) |
| 15 | #66 | fix | Shell-quoted vm-validate paths + LUKS key (injection) |
| 16 | #67 | LK-02 ⚠ | LUKS rotate/revoke + Tang t=2-of-3 guard |
| 17 | #68 | RP-02 | AMD DASH power path |
| 18 | #69 | CT-01 ⚠ | `uaa-control` crate: registry SoR + degraded mode |
| 19 | #70 | PK-02 | Agent enroll client (pinned-CA poll loop) |
| 20 | #71 | RP-03 | Intel AMT + Wake-on-LAN power paths |
| 21 | #72 | CT-02/03/04/06, IP-02 | Registry CRUD, OAuth/RBAC, audit chain, reinstall, checkin/webhook parity |
| 22 | #73 | fix | Path-traversal (webhook) + OAuth state browser-binding |
| 23 | #74 | IP-01/03, CT-05, PK-01 | Seed/inventory parity, approve SAGA, install CA + EnrollService |

**Wave 1–4 are fully complete (18/18 tasks).** Two rounds of security fixes
(4 findings, all resolved) are merged alongside them.

## In progress — NOT merged, needs verification

Five wave-5 tasks were dispatched; all five hit either the account session
limit or a mid-stream API stall before finishing. State:

| Task | Branch | Status |
|------|--------|--------|
| **IP-04** parity fixtures + dashboard | `agent/install-plane-parity-fixtures-dashboard` | Stalled twice, no code written. Worktree empty, nothing to salvage. **Not started.** |
| **CT-07** operator API + OpenAPI | `agent/control-operator-api-openapi` | Stalled before writing code. Worktree empty. **Not started.** |
| **LK-03** LUKS registry sync | `agent/luks-keys-luks-registry-sync` (pushed, commit `20fcbb1`) | Hit session limit right at commit time. One file (`crates/uaa-core/src/luks_sync.rs`) has **uncommitted-then-salvaged** work — pushed as-is, **UNTESTED**, marked `wip(luks)`. Needs: gate run (`cargo test --lib --offline && cargo build --offline`, `cargo clippy --offline --all-targets -- -D warnings`), review, then normal PR+merge. |
| **PK-03** mTLS/CRL/service certs | `agent/pki-mtls-crl-service-certs` | Hit session limit early (still reading rustls APIs). Worktree empty, nothing to salvage. **Not started.** |
| **PK-04** CA cert in seed/ISO | `agent/pki-ca-cert-seed-embedding` (pushed, commit `e018990`) | Hit session limit right before commit — the worker had asked the advisor for a final check before declaring done, implying it believed the work was complete. 5 files changed (template + 3 golden fixtures + `installer-image/nocloud/user-data`), pushed as-is, **UNTESTED**, marked `wip(pki)`. Needs: gate run **including the byte-for-byte golden render tests** (this touches the golden-tested template — verify `cargo test --lib --offline` passes and the golden diff is exactly the intended CA-cert-placeholder addition, nothing spurious), review, then normal PR+merge. |

**Action needed on resume:** run the gate on the two pushed WIP branches
first (`LK-03`, `PK-04`) — they may well be complete and just needed a test
run. Then re-dispatch `IP-04`, `CT-07`, `PK-03` from scratch (nothing to
salvage). A known design note for IP-04's re-dispatch: the three merged
sibling machine-plane routers (`seeds.rs`/`lifecycle.rs`/`inventory.rs`)
each return `Router<()>` with state baked in from fixed production paths,
so a cross-module HTTP-level fixture suite can't inject a tempdir/mock into
the *merged* router without a new `router_with_state(...)` seam — either
add that seam or scope IP-04's fixture suite to direct handler-level tests
(the established per-module pattern) and document the gap.

## Not yet dispatched (remaining scope)

- **Wave 5 remainder integration**: once IP-04/CT-07/PK-03 land (plus
  gating LK-03/PK-04), assemble wave 5 the same way wave 4 was assembled —
  copy disjoint files into one integration branch, union `Cargo.toml` deps,
  do the coordinator-owned wiring (dashboard router merge, `:8443` operator
  listener, PK-03's mTLS helpers into the listener TLS config), gate once,
  merge.
- **Wave 6**: `uaa-web` crate (WB-01..04) — the webroot-owning service:
  static boot-artifact serving, seed/iPXE placement RPCs with the
  placeholder secret-gate, detached ISO build jobs, agent binary publish +
  update manifest signing.
- **Wave 7**: `uaa-pxe` crate (PX-01..04) — dnsmasq boot-config projection
  (via `dhcp-hostsdir`, not `conf.d`, per the spec's SIGHUP-safety finding),
  health checks, the unmanaged-MAC discovery inbox, optional DNS A/PTR.
- **Wave 8**: `TG-03` — the constellation end-to-end VM gate (enroll →
  approve → cert → install → verify sweep, fully inside QEMU+swtpm). This
  is the hard gate before any hardware is touched, per the plan's locked
  rules.
- **Wave 9 (operator-gated, do not dispatch without explicit confirmation)**:
  `TP-05` — deletion of `scripts/autoinstall-agent.py` and the four ported
  shell scripts. Blocked on: the M6 cutover runbook (quiesce Python, import
  registry, swap the systemd unit, dual-serve boot paths, drain, remove
  nginx locations) plus a ≥2-week rollback window, both of which are
  Bucket-3 (human/operational) items outside this coordinator's scope.
- **Deferred follow-ups** (flagged during execution, not yet actioned):
  - Power CLI wiring: `commands.rs`'s `power_command` still runs an
    IPMI-only pre-check before dispatch, so `uaa power <host> on` doesn't
    yet reach the DASH/AMT/WoL paths from the CLI even though the library
    functions exist and are tested.
  - WoL has no dedicated MAC field on `PowerHostEntry`; a WoL-mechanism
    fleet host must currently key its MAC into the `hostname` field
    (fail-closed via `validate_mac`, but not the intended long-term shape).
  - CT-02's `RegistryStore` trait lacks `delete_machine` and yubikey
    status-update methods; `IP-03` worked around this with a narrower local
    trait — CT-02 may need a follow-up to grow those columns/methods so
    `inventory.rs` and `reinstall.rs`'s local traits can be unified into it.
  - `CT-05` (SAGA) and `CT-06` (reinstall) independently declared similar
    but not identical local `WebClient`/`PxeClient` traits (both written
    before real tonic clients existed) — should be unified once `uaa-web`/
    `uaa-pxe` land in waves 6–7.
  - `registry_approve` in the SAGA does two separate registry writes
    (status, then boot_target) plus an audit record rather than one atomic
    multi-field write — a narrow failure window exists between them; may
    need a CT-02 follow-up for an atomic combined-write method.
  - Out-of-band audit-checkpoint witness (P2 hardening, spec-recorded, not
    built) and the arm64/RPi agent variant (P2, undesigned) remain
    Bucket-2 items needing a design pass before they can become briefs.

## How to resume

1. `git fetch origin && git checkout main && git pull --ff-only` (should
   land on `c7916e6` or later).
2. Gate-check the two pushed WIP branches
   (`agent/luks-keys-luks-registry-sync`, `agent/pki-ca-cert-seed-embedding`)
   in their existing worktrees or fresh ones; if green, PR + admin-rebase-merge
   each individually (they're mutually disjoint files).
3. Re-dispatch `IP-04`, `CT-07`, `PK-03` as fresh sub-agent tasks off the
   then-current `main` (their prior worktrees are empty and can be reused
   or recreated).
4. Assemble + wire + gate + merge wave 5 as one integration PR (the
   established pattern from PRs #72 and #74).
5. Continue with waves 6 → 7 → 8 in the same coordinator pattern: dispatch
   disjoint-file tasks in parallel, gate each, merge (or assemble a batch
   integration branch when several tasks share a merge/wiring point),
   never edit `main` directly.
6. Stop before wave 9 (`TP-05`) and get explicit operator confirmation that
   the M6 cutover is complete before dispatching it.
