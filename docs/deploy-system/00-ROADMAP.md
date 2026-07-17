<!-- file: docs/deploy-system/00-ROADMAP.md -->
<!-- version: 1.1.0 -->
<!-- guid: 5fc13d4f-0001-499e-81d1-cdbca2a67871 -->
<!-- last-edited: 2026-07-16 -->

# Deployment System — Roadmap (2026-07-16)

Findings were produced by five read-only repo scouts over the native installer pipeline, the `uaa-control` persistence layer, the audit/operator surface, the retiring curtin path, and the machine plane — every anchor grep-verified on `main` @ `82e4082`. The design was then adjudicated by a three-lens design-judge panel (correctness / ops-rollback / simplicity-scope), which **overturned the v1 persistence architecture**. Ranking below is impact × effort. Baseline: `cargo test --lib --offline` = **634 passed**.

Detail docs (full detail lives there, not here):

| Doc | Scope | Items |
|-----|-------|-------|
| [`../specs/deploy-system-design.md`](../specs/deploy-system-design.md) | Design spec v2.1 — 19 locked decisions, data model, components, rollback | D1–D19 |
| [`../specs/deploy-system-plan.md`](../specs/deploy-system-plan.md) | Taskboard — collision matrix, 6 global waves, model assignments | DS-*-NN |
| [`../agent-tasks/BREAKDOWN-2026-07-16.md`](../agent-tasks/BREAKDOWN-2026-07-16.md) | 3-bucket sort + fan-out strategy | 20 briefs |

## Headline conclusions

1. **`uaa-control` has no database connection in production, and the v1 design did not know it.** `tokio_postgres` appears in no wiring file; `default_state()` — the only reachable production state builder — constructs `FileRegistry(StatePaths)` + `MemEnrollmentStore` + `MemAuditStore`; `db::migrations::apply` has **no caller**. v1's four CockroachDB tables would have been created by nobody and read by nobody, and profiles would have vanished on every restart. Profiles now persist in the `StatePaths` snapshot (**D4**). This single finding removed a migration, a SQL const family, and an entire unscoped dependency from the plan.
2. **The fleet-rename bug is reachable through three doors, not one.** The operator's stated fear was sort-order re-derivation. Two more were found: `read_snapshot` **fails open to an empty doc** on a missing/corrupt file, so an allocator reading through it re-allocates every index from 1 (**D8**); and keying allocations on the mutable group *name* orphans every binding on rename (**D2**). All three are closed.
3. **A dead NIC permanently bricks an index** without a re-bind operation — the machine returns as `len-serv-004` and index 002 can never exist again. Hardware replacement is a normal event; `rebind` is the one audited exception to append-only (**D18**).
4. **The QEMU gate that authorizes hardware currently passes on broken machines.** `vm-validate.sh` accepts `systemctl is-system-running` returning `degraded` as PASS — and `degraded` is returned *precisely when units have FAILED*. Pre-existing, P0, independent of everything else (**DS-APP-04**).
5. **"No news" reads as healthy, forever.** Application health is written only on check-in and nothing flips a status on absence — a dead box renders green indefinitely. Fixed at read time, with no background job (**C6**).

## Rank 0 — Do immediately (high impact, low effort)

| # | Item | IDs | Why now |
|---|------|-----|---------|
| 1 | **Fix the VM gate accepting `degraded` as PASS** | DS-APP-04 | The gate guarding every install can pass on a machine with failed units. Haiku-class, ~20 lines, blocks nothing, depends on nothing. Dispatch ahead of wave 1. |

## Rank 1 — Next (high impact, medium effort)

| # | Item | IDs | Notes |
|---|------|-----|-------|
| 2 | **Profile schema + merge + validation** | DS-PRF-01/02/03 | Pure `uaa-core`, no I/O. Milestone M1. Gated on DS-APP-01 for `ApplicationSpec`. |
| 3 | **Snapshot store + allocate-once + rebind** | DS-REG-01/02/03 | Milestone M2. **DS-REG-03 is the package's core safety task** (Opus-class): every allocation read is fail-closed, or the fleet renames itself. Starts first in the global wave order for maximum soak time. |
| 4 | **Cockroach end-to-end, VM-proven** | DS-APP-01/02/03/05 | Milestone M3 — the original ask: the native pipeline cannot stand up a Cockroach node today. Also deletes a plain-HTTP fetch-and-exec of an ungoverned, self-deleting script from first boot. Gated on 1 (the gate must be able to fail before it can prove anything). |
| 5 | **`config place --from-registry`** | DS-OPS-03 | Milestone M4 — the **only** behavior-changing task. Dry-run default on, `.bak` before every overwrite, all-or-nothing resolution. Gated on 2 and 3. Opus-class, review-critical. |

## Rank 2 — Later (medium impact or high effort)

- **Drift detection + review** — content fingerprint, scheduled scan, accept/revert with last-good-version semantics (DS-REG-04/05). Milestone M5a. Real yield is *accident* detection, not defense (D9) — the hash sits beside the body.
- **Application check-in + staleness** — client reporter, ingest, read-time freshness (DS-CHK-01/02/03). Milestone M5b. DS-CHK-01 is independent and can run in wave 1 opportunistically.
- **Operator API + SPA** — profile/drift routes and screens (DS-OPS-01/02/04). Milestone M5b. The SPA is `rust_embed`-served with an index fallback, so screens need no new server route.

## Deferred-work verdicts

| Item | Verdict |
|------|---------|
| CockroachDB-backed profile store | **Defer** — needs DB connection plumbing (DSN, pool, migration invocation, connect-failure behavior) that `main.rs`/`listeners::serve` do not have. Its own operation. Would also finally give the unused `PgAuditStore` a purpose and make the audit chain survive restart. |
| Keepalived / shared-VIP | **Defer** — forces the continuous-convergence decision this spec defers: unlike CockroachDB, Keepalived does **not** tolerate a stale peer list. |
| HAProxy | **Defer** — cheap once Keepalived's sibling resolution exists; premature before it. |
| `ResolvedSibling` sibling abstraction | **Kill (for v1)** — `compute_join` already takes bare IPs and already filters self. v1 called it "the load-bearing abstraction"; it was load-bearing for a *non-goal*. Reintroduce with Keepalived. |
| Standalone-group "second host" warning | **Kill** — `vm-test` and `unimatrixone` are 2 of 5 machines and both legitimately live there. It would fire on the fleet's normal state from day one. |
| EK-bound machine identity | **Defer** — **nothing on the native install path posts a TPM EK**; only the retiring curtin template does. The `/api/checkin` 403 mismatch alarm is unreachable for exactly the machines this spec provisions, so claiming it would claim a defense that does not exist. |
| IP allocation from the index | **Defer** — the fleet's addressing is arithmetic (`.92`/`.94`/`.96` = index × 2), which is tempting; it collides with DHCP and NIC replacement and needs its own decision. |
| Continuous convergence / re-render | **Defer, with a stated gap** — a 4th node joining leaves nodes 1–3 with stale rendered configs and **no detector** (drift watches profile objects, not rendered outputs). Tolerable at 5 machines because `--join` always lists the seed first, so the member union stays connected. |
| Retiring the curtin path | **Defer** — `autoinstall::` is NOT dead code; `uaa place`/`verify`/`render-user-data` are live CLI subcommands. Retirement removes three subcommands. |
| rpi-serv-001/002/003 | **Kill (for this operation)** — ARM64 Tang/tunnel replicas that never go through `uaa install`. |

## What was explicitly validated (don't re-fix)

- **Adding `applications` is genuinely behavior-neutral today.** `place_configs` never re-serializes — it does line-based textual injection on the source YAML and writes that text verbatim (`config_place.rs`). All five committed configs simply lack the key, and `#[serde(default)]` supplies `vec![]`. Verified by the ops lens.
- **The content hash is not over-built.** The simplicity lens tried hardest to fold it into the audit chain's `detail` field and could not: the audit store is `MemAuditStore` in production and does not survive restart, so it cannot be a revert source. A content hash is literally what the operator asked for.
- **The Application axis survives.** The simplicity lens argued to collapse it into a group field; it fails because `unimatrixone` shares the Lenovos' hardware/secrets model but runs no Cockroach, so class and workload vary independently — collapsing forces a `has_cockroach: bool`, and HAProxy/Keepalived make it a third and fourth boolean.
- **`serde_json`'s `preserve_order` is off today**, so a naive hash *looks* deterministic — which is exactly why the naive test is **vacuous** and `content_hash` canonicalizes explicitly (DS-REG-04).
- **A pre-existing duplicate guid** exists on `main` between `docs/vm-validation.md` and `docs/agent-tasks/testing-gates/TASK-03-constellation-e2e-vm.md`. Unrelated to this package; noted so a future header lint does not blame it on the deploy-system docs.

## Recorded deviation — the commit trailer

This package's briefs use a **model-agnostic** trailer:

```
Co-Authored-By: Claude <noreply@anthropic.com>
```

The 62 briefs in the install-ops and constellation packages use `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`, and `.claude/plan-op.local.md` still declares that value. The deviation is **deliberate, not drift**: those packages were planned *and* executed under one model, whereas these briefs are dispatched to **Haiku- and Sonnet-class** agents, so a Fable-5 trailer would attest to work a different model did. A brief-verifier flagged the inconsistency (advisory, non-fatal — the trailer is present and well-formed).

If the operator prefers package-wide consistency over per-model accuracy, update `.claude/plan-op.local.md`'s `commit_trailer` and the 20 briefs together — do not change one without the other.
