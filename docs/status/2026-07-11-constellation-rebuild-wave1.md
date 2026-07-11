<!-- file: docs/status/2026-07-11-constellation-rebuild-wave1.md -->
<!-- version: 1.0.0 -->
<!-- guid: 4f3787a8-7b27-45b6-b8d9-fe4c721d2396 -->
<!-- last-edited: 2026-07-11 -->

# Constellation Rebuild — Wave 1 Execution Status

## TL;DR

Executed the constellation planning package (`docs/agent-tasks/{core-proto,
control,install-plane,pki,luks-keys,remote-power,tooling-port,
testing-gates}/`, PR #51) as a coordinator running weak-model-proof task
briefs in parallel worktrees, gating and merging each. Waves 1–4 (18 tasks)
are fully complete and merged, plus 2 rounds of security hardening (6
findings, all fixed same-day). `main` is at a 5-crate cargo workspace with
560 passing tests, up from 311 at the start. Wave 5 (5 tasks) was
dispatched but hit the account session limit before finishing — 2 branches
have unverified WIP pushed, 3 have nothing to salvage and need re-dispatch.
See [`../executive-summaries/2026-07-11-constellation-rebuild-executive-summary.md`](../executive-summaries/2026-07-11-constellation-rebuild-executive-summary.md)
for the stakeholder-facing narrative version of this same work.

## Shipped this session

| PR | Area | What |
|----|------|------|
| #52 | core-proto ⚠ | CP-01: cargo workspace conversion; stub modules for all later tasks |
| #53 | testing-gates | TG-04: `constellation-ci.yml` workflow |
| #54 | core-proto | CP-06: musl CI matrix, build+verify every workspace binary |
| #55 | luks-keys | LK-01: `uaa luks` enroll/status (FIDO2+PIN, mock-tested) |
| #56 | security fix | Hardened `validate_luks_dev` against shell injection |
| #57 | core-proto | CP-02: `uaa-proto` crate (tonic/prost via protox) + workspace deps |
| #58 | core-proto | CP-03: `FleetConfig`, parameterize hardcoded fleet constants |
| #59 | tooling-port | TP-01: `uaa iso remaster` (xorriso port, mock-tested) |
| #60 | tooling-port ⚠ | TP-02: `uaa config place/--inject-from` (secret injection) |
| #61 | control | CT-08: React+Vite SPA scaffold |
| #62 | tooling-port | TP-04: `uaa vm-validate` (QEMU+swtpm harness port) |
| #63 | tooling-port | TP-03: `uaa image build` (squashfs overlay port) |
| #64 | core-proto | CP-04: mDNS + static-fallback discovery |
| #65 | core-proto | CP-05: signed self-update (dual-key ed25519) |
| #66 | security fix | Shell-quoted vm-validate command-builder paths + LUKS key |
| #67 | luks-keys ⚠ | LK-02: rotate/revoke + Tang t=2-of-3 cold-start guard |
| #68 | remote-power | RP-02: AMD DASH power path |
| #69 | control ⚠ | CT-01: `uaa-control` crate — registry SoR + degraded mode + wave-4 stubs |
| #70 | pki | PK-02: agent enroll client (pinned-CA poll loop, resume) |
| #71 | remote-power | RP-03: Intel AMT + Wake-on-LAN power paths |
| #72 | control/install-plane | CT-02/03/04/06, IP-02: registry CRUD, OAuth/RBAC, audit chain, reinstall, checkin/webhook parity + router wiring |
| #73 | security fix | Path-traversal (webhook file save) + OAuth state browser-binding |
| #74 | install-plane/control/pki | IP-01/03, CT-05, PK-01: seed/inventory parity, approve SAGA, install CA + EnrollService + wiring + 2 secfixes (argv smuggling, renewal identity-spoof) |
| #75 | docs | Prior execution-status doc + executive-summary CLAUDE.md note (superseded by this file + `docs/process/`) |

Test count: 311 → 560 (workspace `cargo test --lib --offline`), zero
failures at any merge point. `cargo clippy --offline --all-targets -- -D
warnings` clean throughout.

⚠ = review-critical (Opus-tier, line-reviewed by the coordinator before
merge): CP-01, TP-02, LK-02, CT-01.

## In flight

Five wave-5 tasks were dispatched; all five hit either the account session
limit or a mid-stream API stall before finishing.

| Task | Branch | State |
|------|--------|-------|
| IP-04 (parity fixtures + dashboard) | `agent/install-plane-parity-fixtures-dashboard` | Stalled twice before writing code. Worktree empty — **not started**. Known blocker for re-dispatch: the three merged sibling machine-plane routers (`seeds.rs`/`lifecycle.rs`/`inventory.rs`) each return `Router<()>` with state baked in from fixed production paths, so a cross-module HTTP-level fixture suite needs either a new `router_with_state(...)` injection seam or scoping the fixture suite to direct handler-level tests (the established per-module pattern) — document whichever is chosen. |
| CT-07 (operator API + OpenAPI) | `agent/control-operator-api-openapi` | Stalled before writing code. Worktree empty — **not started**. |
| LK-03 (luks registry sync) | `agent/luks-keys-luks-registry-sync` (pushed, `20fcbb1`) | One file (`crates/uaa-core/src/luks_sync.rs`) committed as `wip(luks)` right at the session-limit cutoff — **untested, needs a gate run** (`cargo test --lib --offline && cargo build --offline`, `cargo clippy --offline --all-targets -- -D warnings`), then normal review + PR + merge. |
| PK-03 (mTLS/CRL/service certs) | `agent/pki-mtls-crl-service-certs` | Hit the limit early (still reading rustls APIs). Worktree empty — **not started**. |
| PK-04 (CA cert in seed/ISO) | `agent/pki-ca-cert-seed-embedding` (pushed, `e018990`) | 5 files committed as `wip(pki)` right at the session-limit cutoff, including the golden-tested user-data template + 3 golden fixtures — **untested, needs a gate run including the byte-for-byte golden render tests**; verify the golden diff is exactly the intended CA-cert-placeholder addition and nothing spurious, then normal review + PR + merge. |

## Blocked / deferred

- **Wave 9 (`TP-05` — delete `scripts/autoinstall-agent.py` + 4 shell
  scripts)**: operator-gated. Blocked on the M6 cutover runbook (quiesce
  Python, import registry, swap the systemd unit, dual-serve boot paths,
  drain, remove nginx locations) plus a ≥2-week rollback window — both
  Bucket-3 (human/operational) items outside coordinator scope. Do not
  dispatch without explicit operator confirmation the cutover is complete.
- **Power CLI wiring**: `commands.rs`'s `power_command` still runs an
  IPMI-only pre-check before dispatch, so `uaa power <host> on` doesn't yet
  reach the DASH/AMT/WoL library paths from the CLI even though those
  functions exist and are tested (merged in #68/#71).
- **WoL MAC field**: `PowerHostEntry` has no dedicated MAC field; a
  WoL-mechanism fleet host currently keys its MAC into the `hostname` field
  (fail-closed via `validate_mac`, not the intended long-term shape).
- **`CT-02` `RegistryStore` gaps**: no `delete_machine`, no yubikey
  status-update methods. `IP-03` (merged in #74) worked around this with a
  narrower local trait; `reinstall.rs` (CT-06, #72) declared a similar
  local trait independently. Candidate follow-up: grow `RegistryStore` so
  both can unify into it.
- **`CT-05`/`CT-06` local trait duplication**: `saga.rs` and `reinstall.rs`
  independently declared similar-but-not-identical local `WebClient`/
  `PxeClient` traits (both written before real tonic clients existed from
  `uaa-web`/`uaa-pxe`) — unify once waves 6–7 land.
- **`registry_approve` non-atomic write**: the SAGA does a status write,
  then a boot-target write, then an audit record — three separate calls
  rather than one atomic multi-field write, leaving a narrow failure
  window between them. Candidate `CT-02` follow-up for a combined atomic
  write method.
- **Out-of-band audit-checkpoint witness** (P2 hardening, spec-recorded,
  not built) and the **arm64/RPi agent variant** (P2, undesigned) remain
  Bucket-2 items needing a design pass before they become task briefs.

## Next steps

1. Gate-check the two pushed WIP branches (`agent/luks-keys-luks-registry-sync`,
   `agent/pki-ca-cert-seed-embedding`); if green, PR + admin-rebase-merge
   each individually (mutually disjoint files).
2. Re-dispatch `IP-04`, `CT-07`, `PK-03` from scratch off the then-current
   `main` (prior worktrees are empty).
3. Assemble wave 5 the same way wave 4 was assembled (PRs #72, #74): copy
   disjoint files into one integration branch, union `Cargo.toml` deps, do
   the coordinator-owned wiring (dashboard router merge, `:8443` operator
   listener, PK-03's mTLS helpers into the listener TLS config), gate once,
   merge.
4. Continue waves 6 (`uaa-web` crate, WB-01..04) → 7 (`uaa-pxe` crate,
   PX-01..04) → 8 (`TG-03`, the constellation end-to-end VM gate — the hard
   gate before any hardware) in the same coordinator pattern.
5. Stop before wave 9 (`TP-05`) and get explicit operator confirmation
   before dispatching it.
