<!-- file: docs/agent-tasks/BREAKDOWN-2026-07-16.md -->
<!-- version: 1.1.0 -->
<!-- guid: 9b248d1c-6ba8-4580-b375-6619606eb533 -->
<!-- last-edited: 2026-07-16 -->

# Agent-Task Breakdown & Fan-Out Plan — 2026-07-16 (deploy-system)

Turns [`../specs/deploy-system-plan.md`](../specs/deploy-system-plan.md) into **weak-model-proof agent briefs** plus a cost/efficiency strategy for fanning them out. See [`ORCHESTRATION.md`](ORCHESTRATION.md) (coordinator + workers, dependency waves) — this package shares that protocol with the install-ops and constellation packages.

Design spec: [`../specs/deploy-system-design.md`](../specs/deploy-system-design.md) v2.1 (decision IDs `D1`–`D19`).

## Method

Every task was verified against the current codebase by five read-only scouts, then the design was adjudicated by a three-lens design-judge panel (correctness / ops-rollback / simplicity-scope) **which overturned the v1 persistence architecture**. Tasks were then sorted into three buckets. **Only Bucket 1 becomes agent briefs** — forcing design-heavy or prod-verification items into weak-model briefs produces the opposite of "excellent results."

**The single most important inherited fact, and the reason Bucket 2 is as large as it is:** `uaa-control` has **no CockroachDB connection in production**. Verified three ways — `tokio_postgres` appears in no wiring file; `default_state()` (the only reachable prod state builder, since `operator::router()` takes no arguments) constructs `FileRegistry(StatePaths)` + `MemEnrollmentStore` + `MemAuditStore`; and `db::migrations::apply` has no caller. Profiles therefore persist in the `StatePaths` snapshot (spec D4), and **no Bucket-1 task writes SQL or a migration.**

---

## Bucket 1 — Authored as agent briefs (localized / mechanical / well-specced)

### ⚠️ Same-file collision rule (drives wave ordering)

Computed from every task's Exact-files list — never eyeballed. Full table with resolutions: [`../specs/deploy-system-plan.md`](../specs/deploy-system-plan.md) § collision table. Summary of the 10 collision rows:

| Shared file | Tasks that touch it | Resolution |
|---|---|---|
| `crates/uaa-core/src/network/ssh_installer/installer.rs` | DS-APP-02, DS-APP-03 | serialize: wave2=DS-APP-02, wave3=DS-APP-03 |
| `crates/uaa-core/src/network/ssh_installer/applications.rs` | DS-APP-02, DS-APP-03 | serialize: wave2=DS-APP-02, wave3=DS-APP-03 |
| `crates/uaa-control/src/db/mod.rs` | DS-REG-01, **DS-CHK-02** | serialize: wave1=DS-REG-01, wave4=DS-CHK-02 |
| `crates/uaa-control/src/db/store.rs` | DS-REG-01, DS-REG-02 | serialize: wave1=DS-REG-01, wave2=DS-REG-02 |
| `crates/uaa-control/src/profiles/store.rs` | DS-REG-02, DS-REG-03, **DS-REG-04** | serialize: wave2=DS-REG-02, wave3=DS-REG-03, wave4=DS-REG-04 |
| `crates/uaa-control/src/profiles/drift.rs` | DS-REG-04, DS-REG-05 | serialize: wave4=DS-REG-04, wave5=DS-REG-05 |
| `crates/uaa-core/src/profile/mod.rs` | DS-PRF-01, DS-PRF-02, DS-PRF-03 | scaffold-first: wave2=DS-PRF-01 creates mod.rs + stubs; wave3 fills **disjoint** stubs |
| `crates/uaa-control/src/operator/handlers.rs` | DS-OPS-01, DS-OPS-02 | serialize: wave4=DS-OPS-01, wave5=DS-OPS-02 |
| `crates/uaa-control/src/operator/api_types.rs` | DS-OPS-01, DS-OPS-02 | serialize: wave4=DS-OPS-01, wave5=DS-OPS-02 |
| `scripts/vm-validate.sh` | DS-APP-04, DS-APP-05 | serialize: wave2=DS-APP-04, wave5=DS-APP-05 |

Single-writer files (no collision): `config.rs` (DS-APP-01), `config_place.rs` (DS-OPS-03), `machine_plane/lifecycle.rs` (DS-CHK-02), `app_status.rs` (DS-CHK-01), `vm-test.yaml` (DS-APP-05), `profile/merge.rs` (DS-PRF-02), `profile/validate.rs` (DS-PRF-03), `profiles/alloc.rs` (DS-REG-03).

### WS-1 — `applications` (backend / installer) · maps to DS-APP-01..05

| Task | Src id | Title | Tier | Why tier | Wave |
|---|---|---|---|---|---|
| T01 | DS-APP-01 | `ApplicationSpec` + defaulted `applications` field (+ `PartialEq`) | **Sonnet-class** | touches `InstallationConfig`'s serde contract; a missing default breaks 5 YAMLs and 4 exhaustive literals | 1 |
| T02 | DS-APP-02 | `ApplicationInstaller` + Phase-5 wiring (fail-closed scaffold) | **Sonnet-class** | inserts a fallible step into the proven 7/7 flow, reached from two call sites | 2 |
| T03 | DS-APP-03 | Cockroach install step (port `setup_cockroachdb.sh`) | **Sonnet-class** | ports an out-of-git script; systemd + cert fetch + join derivation | 3 |
| T04 | DS-APP-04 | Fix `vm-validate.sh` accepting `degraded` as PASS | **Haiku-class** | two-line shell-gate fix with an exact before/after | 2 |
| T05 | DS-APP-05 | VM gate: Cockroach readiness + `vm-test.yaml` spec | **Sonnet-class** | gate semantics; `is-active` is insufficient, readiness required | 5 |

Execution mode: SERIAL WAVES (coordinator-driven) — trigger: DS-APP-03 shares `applications.rs` + `installer.rs` with DS-APP-02 (collision rows 1–2).

> **DS-APP-04 is P0 and independent of everything else in this package.** `vm-validate.sh` currently accepts `systemctl is-system-running` returning `degraded` as PASS — and `degraded` is returned *precisely when one or more units have FAILED*. This is a **pre-existing bug in the gate that guards every install**, not new work. It can be dispatched immediately, ahead of wave 1, and should be.

### WS-2 — `registry` (backend / persistence) · maps to DS-REG-01..05

| Task | Src id | Title | Tier | Why tier | Wave |
|---|---|---|---|---|---|
| T01 | DS-REG-01 | Snapshot row types + `SnapshotDoc` collections + `profiles/` scaffold | **Sonnet-class** | shapes 4 row types every sibling depends on; `db/mod.rs` is the crate's declared single home for rows | 1 |
| T02 | DS-REG-02 | `ProfileStore` + `SnapshotProfileStore` + `read_snapshot_strict` | **Sonnet-class** | new trait + twins mirroring `SagaStore`; introduces the fail-closed read | 2 |
| T03 ⚠ | DS-REG-03 | `allocate_index` (fail-closed insert-if-absent) + `rebind` | **Opus-class** | irreversible: a wrong read here renames the entire fleet | 3 |
| T04 | DS-REG-04 | `content_hash` (explicit canonicalization) + `profile_versions` | **Sonnet-class** | hash determinism rests on unpinned serde_json assumptions | 4 |
| T05 ⚠ | DS-REG-05 | Drift scan + accept/revert (last-good-version) | **Opus-class** | revert semantics are subtle and destroy evidence if wrong | 5 |

Execution mode: SERIAL WAVES (coordinator-driven) — trigger: DS-REG-02 shares `db/store.rs` with DS-REG-01 (collision row 4).

### WS-3 — `profiles` (backend / pure) · maps to DS-PRF-01..03

| Task | Src id | Title | Tier | Why tier | Wave |
|---|---|---|---|---|---|
| T01 | DS-PRF-01 | `profile/` scaffold: types + stubs | **Sonnet-class** | defines the partial types (incl. the `Option<Option<String>>` trap) every sibling fills against | 2 |
| T02 | DS-PRF-02 | `merge()` + provenance + 10-required-field fail-closed | **Sonnet-class** | the fail-closed scope is a correctness trap (defaults must win) | 3 |
| T03 | DS-PRF-03 | Validation: global hostname uniqueness, immutability, standalone | **Sonnet-class** | prefix uniqueness is necessary-not-sufficient; the real invariant is global | 3 |

Execution mode: SERIAL WAVES (coordinator-driven) — trigger: DS-PRF-02/03 fill stubs created by DS-PRF-01 (collision row 7, resolved scaffold-first).

### WS-4 — `checkin` (backend / telemetry) · maps to DS-CHK-01..03

| Task | Src id | Title | Tier | Why tier | Wave |
|---|---|---|---|---|---|
| T01 | DS-CHK-01 | `app_status.rs` client reporter | **Haiku-class** | mechanical mirror of `luks_sync`'s payload→post_sync→ok:bool shape | 1 |
| T02 | DS-CHK-02 | Machine-plane ingest + snapshot field | **Sonnet-class** | fail-open ingest path; must not extend `MachineStatus` | 4 |
| T03 | DS-CHK-03 | Read-time staleness (`Stale` ≠ healthy) | **Sonnet-class** | the "no news = healthy forever" failure mode | 5 |

Execution mode: SERIAL WAVES (coordinator-driven) — trigger: DS-CHK-02/03 depend on DS-CHK-01's payload type.

### WS-5 — `operator-api` (backend + frontend) · maps to DS-OPS-01..04

| Task | Src id | Title | Tier | Why tier | Wave |
|---|---|---|---|---|---|
| T01 | DS-OPS-01 | `/api/profiles` route group + DTOs | **Sonnet-class** | mirrors `build_router`'s role-grouping; auth wiring is load-bearing | 4 |
| T02 | DS-OPS-02 | `/api/drift` review routes | **Sonnet-class** | mutations must use `append_in_txn`, never `record()` | 5 |
| T03 ⚠ | DS-OPS-03 | `config place --from-registry` (dry-run default, `.bak`) | **Opus-class** | the ONLY behavior-changing task; mass-overwrites the webroot | 6 |
| T04 | DS-OPS-04 | SPA: profile + drift screens, staleness rendering | **Haiku-class** | follows existing page patterns (Machines/Approvals/Audit) | 6 |

Execution mode: SINGLE-AGENT (strong model) for DS-OPS-03 — trigger: judgment work, irreversible writes to `/var/www/html/cloud-init/**`. Never parallelized, never weak-tier.

### Coordinator protocol (verbatim)

> **Coordinator owns git. Workers never push.** Each worker operates only inside its
> assigned worktree: edit, test, commit — then stop. Workers never run `git push`,
> `gh pr`, or any merge command. The coordinator runs the gate (`cargo test --lib --offline && cargo build --offline`) in each
> finished worktree, opens the PR, merges (rebase/FF unless the repo profile says
> otherwise), and then **rebases every open sibling worktree** before dispatching
> anything else.
>
> **Per-merge sibling-rebase loop:** after EVERY merge to `origin/main`:
> for each open sibling worktree, `git fetch origin && git rebase
> origin/main`. A sibling that skips a rebase is a future conflict.
>
> **Conflict escalation ladder** (in order, never skip a rung): 1) clean rebase;
> 2) conflict-resolver subagent (Sonnet-class, only when the conflict spans 1–3 small
> files); 3) file-copy cherry-pick fallback — re-apply the task's file states onto a
> fresh branch from HEAD; 4) mark `rebase_blocked`, stop the lane, escalate to a human.
>
> **A wave MUST NOT start** while any of: the previous wave has an unmerged PR; any
> sibling worktree is un-rebased; the gate is red on `origin/main`; or a
> `rebase_blocked` marker is unresolved.

---

## Bucket 2 — NOT briefs: needs brainstorm/design first

| Item | Why it needs design first |
|---|---|
| **CockroachDB-backed profile store** | Needs DB connection plumbing (DSN/config, pool, migration invocation, connect-failure behavior) that `main.rs`/`listeners::serve` **do not have** — the crate's own module doc says so. It is its own operation, not a task. It would also finally give `PgAuditStore` (built, unused) a purpose, and make the audit chain survive restart. Blocked on: deciding fail-open vs fail-closed on an unreachable cluster, and the bootstrap circularity (CRDB runs on the machines this system deploys). |
| **Keepalived / shared-VIP application** | Needs the deferred sibling abstraction (D17), a VRRP priority rule, **and** a convergence story — unlike CockroachDB, Keepalived does *not* tolerate a stale peer list, so it forces the continuous-convergence decision this spec defers. |
| **HAProxy application** | No backend-selection or health-check semantics decided. Cheap once Keepalived's sibling resolution exists; premature before it. |
| **Continuous convergence / re-render on group change** | The known gap named in Non-goals: a 4th node joining leaves nodes 1–3 with stale rendered configs and **no detector** (drift watches profile objects, not rendered outputs). Needs a decision on whether rendered outputs get their own fingerprint. |
| **EK-bound machine identity** | The natural upgrade from D-A/A1, but **nothing on the native install path posts a TPM EK** — only the retiring curtin template does. Requires wiring EK capture into the native installer *and* deciding whether enrollment mandates it. |
| **IP allocation from the index** | The fleet's addressing is arithmetic (`.92`/`.94`/`.96` = index × 2), so it is tempting. Collides with DHCP and NIC replacement; needs its own decision. |
| **rpi-serv-001/002/003 onboarding** | ARM64 Tang/tunnel replicas that never go through `uaa install`. Bringing them in scope is a separate operation. |

---

## Bucket 3 — NOT tasks: operational / prod-verification (no code deliverable)

- **Deploying `uaa-control` to 172.16.2.30** — a human deploy step; the repo's hard rule keeps server writes out of agent scope.
- **Flipping `--from-registry` in production** (M4 cutover) — operator-gated, after DS-OPS-03's dry-run diff is reviewed by a human.
- **Backing up the snapshot file** before the first profile write — one `cp`; an operator runbook line, not a task.
- **Retiring the curtin path** — removes three live CLI subcommands (`uaa place`/`verify`/`render-user-data`); operator-gated, out of scope (Non-goals).
- **Verifying a real Cockroach node joins the live cluster** — hardware; only after the QEMU gate (DS-APP-05) is green.

---

## Cost / efficiency strategy (fan-out)

- **Tier split:** 3 Haiku-class : 14 Sonnet-class : 3 Opus-class across 20 Bucket-1 tasks. Weighted to Sonnet because most tasks integrate with an existing seam (`SagaStore`, `ResetPartitionStager`, `build_router`, `luks_sync`) rather than mirror one mechanically. The three Opus-class tasks are the irreversible ones and must never be downgraded.
- **No `/parallel-sweep` anywhere in this package.** No wave contains ≥3 mechanically-similar tasks, and no single change touches ≥20 similar callsites — both `/parallel-sweep` floors are unmet. Waves run as concurrent single-agent dispatches, coordinator-merged.
- **Coordinator owns git/gh:** workers stay in their worktree and report done; only the coordinator merges + rebases siblings.
- **Waves respect the collision table** — never co-schedule two tasks touching the same file.
- **Cheapest-first within a wave:** independent different-file tasks are wave 1; same-file tasks serialize after. The `registry` track starts first (DS-REG-01 in wave 1) because DS-REG-03's fail-closed allocation is the design's core safety property and needs maximum soak time.
- **Dispatch DS-APP-04 immediately, ahead of wave 1.** It fixes a pre-existing bug in the gate that guards every install, is Haiku-class, and blocks nothing.
- **Known CI noise:** none identified at planning time; each worker's gate is `cargo test --lib --offline && cargo build --offline` in its own worktree, not full CI.
