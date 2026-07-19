<!-- file: docs/status/2026-07-19-profile-registry-deploy-system-handoff.md -->
<!-- version: 1.0.0 -->
<!-- guid: 4a9f1c72-8e3d-4b25-9f6a-0d7c2b18e934 -->
<!-- last-edited: 2026-07-19 -->

# Profile-registry / deploy-system execution — handoff for reconciliation (2026-07-19)

Operational status report. Terse, file:line-anchored. **Purpose:** two Claude
instances have been working this repo concurrently and tripping over each other
(both touched the deploy + registry). This captures **instance A's** (the
profile-registry / deploy-system instance's) complete state so **instance B**
(the ARP/DHCP-tracking instance — see
[`2026-07-18-registry-deploy-and-arp-dhcp-tracking.md`](2026-07-18-registry-deploy-and-arp-dhcp-tracking.md))
can reconcile. The two workstreams are mostly disjoint in code; the overlap is
the **deploy, the registry, and `docs/status/`**.

## TL;DR of the collision

- **Instance A (me):** built + merged the deploy-system feature plan (profile
  registry, drift, operator API, SPA, `config place --from-registry`, reify +
  backfill + shadow-registration). All on `main`.
- **Instance B (you):** ARP/DHCP device tracking / discovery-inbox / future
  `uaa-pxe` crate (machine registry, not profile registry). You also **already
  deployed my DS-OPS-05 to the server** (`main @ 15be746`) — the deploy I was
  blocked on. Thank you; that resolved my pending step.
- **Nothing is in code conflict on `main` right now.** The risk is duplicated
  *actions* (deploy, backfill) and duplicated *docs*, not merge conflicts.

## What instance A completed (all merged to `main`)

The 20-brief deploy-system plan (`docs/specs/deploy-system-plan.md`),
`docs/agent-tasks/**`. First 14 landed in prior sessions (#97–#110). This
session landed the last 6 + a follow-on:

| PR | Task | Area |
|---|---|---|
| #111 | DS-APP-05 vm-gate cockroach readiness | `scripts/vm-validate.sh`, `examples/configs/install/vm-test.yaml` |
| #112 | DS-CHK-03 read-time staleness | `crates/uaa-control/src/machine_plane/staleness.rs` |
| #113 | DS-REG-05 drift scan/accept/revert | `crates/uaa-control/src/profiles/drift.rs`, `store.rs` |
| #114 | DS-OPS-02 drift review routes | `crates/uaa-control/src/operator/handlers.rs`, `api_types.rs` |
| #116 | DS-OPS-04 SPA Drift+Profiles | `web/src/pages/{Drift,Profiles}.tsx`, `web/src/api/{client,types}.ts`, `App.tsx` |
| #117 | DS-OPS-03 `config place --from-registry` | `crates/uaa-core/src/config_place.rs`, `network/ssh_installer/config.rs`, `crates/uaa/src/cli/config.rs`, `crates/uaa-control/src/profiles/{resolve,convert}.rs` |
| #118 | DS-OPS-05 reify + backfill + shadow-registration | `crates/uaa-control/src/profiles/reify.rs`, `crates/uaa/src/cli/config.rs`, `lib.rs`, `profiles/mod.rs` |

`main @ 15be746` (my last), then `3bb6461` (instance B's status doc) on top.
Baseline: `cargo test --lib --offline` = **779 passed / 0 failed**; build +
`clippy --all-targets -D warnings` clean; frontend `tsc --noEmit && vite build`
clean.

## Deploy state (current, verified read-only 2026-07-19)

- Server **172.16.2.30** (hostname `unimatrixzero`), repo
  `/home/jdfalk/ubuntu-autoinstall-agent`, HEAD **`15be746`** — DS-OPS-05 IS
  deployed (uaa-control rebuilt 2026-07-18 20:36, service active/healthy on
  `:25000` machine plane + `:15000` operator TLS + `:15001` grpc). New
  `/api/drift` route returns 401 (deployed + auth-gated).
- **Deploy history this round:** instance A deployed `4bb9708` at 19:25;
  instance B advanced to `15be746` at 20:36. **Do not double-deploy** — HEAD is
  current with `main` except for B's `3bb6461` docs-only commit.
- Deploy tool: `scripts/server-deploy.sh` (no-arg = pull+build+stage+restart,
  bare command only — the compound `merge && pull` and even `checkout && pull`
  trip the auto-mode classifier; run each git step separately).

## THE key open action: the registry is still EMPTY

`/var/lib/uaa/registry-snapshot.json` = **549 bytes, dated Jul-14** — the
initial empty snapshot. **`uaa config backfill` has NOT been run by either
instance.** Until it does, the Profiles SPA page is empty and
`config place --from-registry` resolves nothing.

- Backfill writes `/var/lib/uaa` (uaa-owned `0600`), so it must run **as the
  `uaa` user** — and `sudo -u uaa` is **not** in the deploy NOPASSWD sudoers, so
  neither instance can run it non-interactively. **This needs the operator to
  run it (password) or a sudoers grant.**
- ⚠ CLI-binary ambiguity to resolve first: instance B's doc says the CLI is
  `target/release/uaa`, but on the server only `/usr/local/bin/ubuntu-autoinstall-agent`
  exists (and it *does* carry the `config` subcommands — `config place --help`
  worked pre-DS-OPS-05). Confirm which binary carries `config backfill` before
  running: `<cli> config backfill --help`.

## Pending sequence A was about to execute (hand this to whoever owns it)

1. **Backfill** (as `uaa`): `<cli> config backfill --src /home/jdfalk/ubuntu-autoinstall-agent/examples/configs/install` — idempotent, non-destructive, seeds the 4 committed fleet hosts (len-serv-001/002/003 indexed, unimatrixone standalone).
2. **Dry-run**: `<cli> config place --from-registry` (dry-run defaults ON, writes nothing) — prints resolved-vs-committed. NOTE: expect **cosmetic** text diffs (raw commented file vs serialized output); the real faithfulness proof is the unit round-trip (M2 gate), not a live zero-diff.
3. **Operator-gated flip** (Bucket 3, needs explicit user "yes" — NOT auto): `<cli> config place --from-registry --no-dry-run` (writes `.bak` before each overwrite). Never run against real hosts without the operator's go.

## Correctness invariants to preserve (if instance B edits these files)

- **Shadow noop** (`crates/uaa/src/cli/config.rs`): `config place --register`
  commits placement first (`place_configs(&base)?` returns before the registry
  is touched), then `shadow_register_placed` swallows every error (no `?`, no
  unwrap) — a registry-write failure must NEVER fail/alter a placement.
- **Reify round-trip** (`crates/uaa-control/src/profiles/reify.rs`):
  `register_from_config` → `resolve_from_registry` must round-trip exactly; the
  M2 gate (`test_resolved_equals_committed_by_struct_equality`) calls the real
  reify. Indexed hosts allocate in hostname order (guarded, loud fail).
- **Place safety** (`crates/uaa-core/src/config_place.rs`): `--dry-run`
  defaults ON, `.bak` before overwrite, `REPLACE_AT_PLACE_TIME` hard-gate,
  all-or-nothing resolution.
- **Registry**: `allocate_index` allocate-once/idempotent; drift revert
  restores the newest version whose body still hashes to its stored
  `content_hash` (never blind N-1); mutations append-only, never `record()`.

## Overlap / collision map with instance B

- **Disjoint in code:** B's machine registry (`MachineRow`, `machine_plane/seeds.rs`, discovery inbox, `uaa-pxe`) vs A's profile registry (`profiles/*`). Both persist into the same `/var/lib/uaa` snapshot but different row types (B's own doc §"The two registries").
- **Shared, watch these:** `docs/status/` (two docs now — this one + B's); the deploy mechanism; `crates/uaa-control/src/operator/handlers.rs` (A added drift routes + moved the row→profile converters to `profiles/convert.rs`; B's `/api/discovered` stub at `:786` is untouched by A).
- **Latent future conflict:** `agent/tooling-port-config-place-inject` (`cad242c`, ws9 — ports `deploy-usb-configs.sh` to `config place --inject-from`) touches `config_place.rs`, which A rewrote for DS-OPS-03/05. If that branch is revived it will need a rebase onto A's version.

## Documented non-blocking follow-ups (from A's line-reviews)

1. DS-OPS-03: a registry host outside the hardcoded 4-entry `mac_for_host`
   (`config_place.rs`) resolves but is **refused at placement loudly** — the
   "registry host not in KNOWN_HOSTS is placeable" edge isn't delivered; real
   fix threads the allocation identity (MAC) to place. Prod scope = 4 hosts.
2. DS-OPS-02: `review_error_response` maps a store-I/O failure to HTTP 400 not
   503 (substring match; drift has no typed errors) — typed-error cleanup.
3. DS-REG-05: `MemProfileStore::inject_{group,profile}_raw` are ungated
   `pub fn` (test-double helpers) — should be `#[cfg(test)]`-gated.

## Worktree / branch hygiene

- A removed all 6 of its wave worktrees + the stray `app-spec-partialeq-remediation`.
- Remaining worktrees (leave alone): `ubuntu-autoinstall-agent-zfs-luks-multikey`
  (feature branch), `.worktrees/luks-keys-luks-registry-sync` (`20fcbb1`, LK-03
  WIP — not A's, has uncommitted work).
