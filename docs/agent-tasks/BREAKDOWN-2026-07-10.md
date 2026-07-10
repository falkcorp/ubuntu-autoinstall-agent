<!-- file: docs/agent-tasks/BREAKDOWN-2026-07-10.md -->
<!-- version: 1.0.0 -->
<!-- guid: ed40afe5-db72-4723-9280-302daf1fc0fe -->
<!-- last-edited: 2026-07-10 -->

# Agent-Task Breakdown & Fan-Out Plan — 2026-07-10 (constellation)

This document turns the approved constellation plan (`docs/specs/constellation-design.md`
+ `docs/specs/constellation-plan.md`) into **weak-model-proof agent briefs** plus a
cost/efficiency strategy for fanning them out. See [`ORCHESTRATION.md`](ORCHESTRATION.md)
(coordinator + workers, dependency waves). This breakdown EXTENDS the 2026-07-09
install-ops package (all 20 tasks shipped — see Archived).

## Method

Every task was verified against the current codebase (311 passing lib tests at plan
time), judge-panel-reviewed at the spec level, then sorted into three buckets. **Only
Bucket 1 becomes agent briefs** — forcing design-heavy or operational items into
weak-model briefs produces the opposite of excellent results.

---

## Bucket 1 — Authored as agent briefs (42 tasks, 10 workstreams)

### ⚠️ Same-file collision rule (drives wave ordering)

| Shared file | Tasks that touch it | Resolution |
|-------------|---------------------|------------|
| `Cargo.toml` (root) | CP-01, CP-02 | serialize: wave1=CP-01, wave2=CP-02 |
| `crates/uaa-core/src/power/mod.rs` | CP-01, CP-03 | serialize: wave1=CP-01, wave2=CP-03 |
| `crates/uaa-core/src/luks_keys.rs` | CP-01, LK-01, LK-02 | serialize: wave1=CP-01, wave2=LK-01, wave3=LK-02 |
| `crates/uaa-control/src/ca.rs` | CT-01, PK-01, PK-03 | serialize: wave3=CT-01, wave4=PK-01, wave5=PK-03 |
| stub-pattern (crate-skeleton task pre-creates every follower's file; one filler each) | CP-01→{CP-03..05, LK-01, TP-01..04, PK-02, RP-02/03}; CT-01→{CT-02..07, IP-01..04, PK-01}; WB-01→{WB-02..04}; PX-01→{PX-02..04} | serialize: stub wave strictly precedes fill wave |

The full 9-wave global table with Execution-mode stamps lives in
`docs/specs/constellation-plan.md` — it is the single projection of the computed matrix;
workstream READMEs repeat only their slice.

### Workstreams (tables per workstream live in each `<ws>/README.md`)

| Workstream | Tasks | Maps to | Waves |
|---|---|---|---|
| `core-proto/` | CP-01..06 | ws1 foundation | 1–3 |
| `control/` | CT-01..08 | ws2 uaa-control | 2–5 |
| `install-plane/` | IP-01..04 | ws3 Python parity | 4–5 |
| `pki/` | PK-01..04 | ws4 enrollment | 3–5 |
| `uaa-web/` | WB-01..04 | ws5 webroot owner | 6–7 |
| `uaa-pxe/` | PX-01..04 | ws6 dnsmasq/discovery | 6–7 |
| `luks-keys/` | LK-01..03 | ws7 FIDO2 LUKS | 2–5 |
| `remote-power/` (continuation) | RP-02..03 | ws8 power finish | 3 |
| `tooling-port/` | TP-01..05 | ws9 shell→Rust | 2–9 |
| `testing-gates/` (continuation) | TG-03..04 | ws10 gates | 2, 8 |

Execution mode: PARALLEL DISPATCH within each wave, SERIAL WAVES between waves
(coordinator-driven) — trigger: every wave's tasks touch disjoint files per the
collision matrix; no wave has ≥3 mechanically-similar same-pattern tasks, so
`/parallel-sweep` is not stamped anywhere. W1/W8/W9 are SINGLE-AGENT (strong model):
CP-01 (judgment transform), TG-03 (harness judgment), TP-05 (gated removal).

> Do-not-touch markers: no brief touches 172.16.2.30, len-serv-003, or any BMC; TP-05
> is ⛔ blocked on the operator-confirmed M6 cutover; unimatrixone is never powered on.

### Coordinator protocol (verbatim)

> **Coordinator owns git. Workers never push.** Each worker operates only inside its
> assigned worktree: edit, test, commit — then stop. Workers never run `git push`,
> `gh pr`, or any merge command. The coordinator runs the gate (`cargo test --lib
> --offline && cargo build --offline`) in each finished worktree, opens the PR, merges
> (rebase/FF unless the repo profile says otherwise), and then **rebases every open
> sibling worktree** before dispatching anything else.
>
> **Per-merge sibling-rebase loop:** after EVERY merge to `origin/main`: for each open
> sibling worktree, `git fetch origin && git rebase origin/main`. A sibling that skips a
> rebase is a future conflict.
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
|------|---------------------------|
| Advanced SPA pages (config-mgmt editing of non-secret fields, health-dashboard aggregation) | UX design session needed; the JSON API (CT-07) lands first |
| arm64/RPi agent + template variant (P2) | RPi boot chain (u-boot/UEFI path) undesigned |
| Out-of-band audit checkpoint witness (P2) | witness target (git repo vs second host) undecided — spec Decision 21 records the threat-model boundary |
| AMD DASH deep-dive if `dashcli` .deb unavailable on modern Ubuntu | research task carried from the install-ops DEFERRED list |

## Bucket 3 — NOT tasks: operational / prod-verification (no code deliverable)

M6 cutover runbook execution on the server (port audit :7443-:7446/:8081/:8443, quiesce
Python, `import`, unit swap, dual-serve :80+:8081, iPXE URL flips, drain, nginx location
removal) · CA generation + offline CA/update-key backup ceremony (M3 ship-gate) ·
GitHub OAuth app + org team setup (uaa-admins/uaa-operators) · CRDB `uaa` database/user
creation · optional BSR publish `buf.build/falkcorp/uaa`.

---

## Archived (already shipped)

| Workstream | Status | Note |
|------------|--------|------|
| `installer-robustness/`, `phase-rerun/`, `boot-prod/`, `install-server/`, `remote-power/` TASK-01, `testing-gates/` TASK-01..02 | ✅ all 20 shipped (2026-07-10, 311 tests) | residuals: DASH/AMT/WoL carried into RP-02/03; IPMI-log-redaction follow-up tracked in todo.md |

---

## Cost / efficiency strategy (fan-out)

- **Tier split:** 5 Haiku-class (mechanical) : 33 Sonnet-class : 4 Opus-class ⚠. The
  Sonnet share exceeds the house ~2/3 heuristic — expected for greenfield-service logic;
  deviation noted deliberately, do not re-tier downward.
- **Coordinator owns git/gh:** workers stay in their worktree and report done; only the
  coordinator merges + rebases siblings; ⚠ tasks get line-by-line review.
- **Waves respect the collision table** — never co-schedule two tasks touching the same
  file; the stub-pattern is what keeps waves 2–7 wide.
- **Cheapest-first within a workstream:** independent different-file tasks first;
  same-file tasks serialize after (LK, PK ca.rs).
- **Known CI noise:** none currently; each worker's gate is the local
  `cargo test --lib --offline && cargo build --offline` (offline — no registry access
  needed; `Cargo.lock` committed). SPA tasks add `cd web && npm ci && npm run build`.
