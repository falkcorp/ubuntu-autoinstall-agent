<!-- file: docs/agent-tasks/remote-power/orchestration.md -->
<!-- version: 1.1.0 -->
<!-- guid: b91e9a4c-20d2-46b3-806f-761e133d7ec5 -->
<!-- last-edited: 2026-07-10 -->

# remote-power — orchestration

`remote-power/TASK-01` sits in **global wave 5** of the install-ops plan (MERGED); its only ordering constraints were cross-workstream file collisions, not logical dependencies (`Depends on: none`). The constellation continuation adds **TASK-02 (RP-02)** and **TASK-03 (RP-03)** in **constellation global wave 3** — see the continuation section below.

## Wave order for this workstream

| Global wave | This WS runs | Must be MERGED first (cross-workstream, collision-driven) |
|---|---|---|
| 1 | — | `installer-robustness/TASK-02` (shares `src/cli/commands.rs`) |
| 2 | — | `installer-robustness/TASK-03` (shares `src/cli/commands.rs`) |
| 3 | — | `installer-robustness/TASK-07` (shares `src/cli/commands.rs`) |
| 4 | — | `phase-rerun/TASK-01` (shares `src/cli/args.rs`, `src/main.rs`, `src/cli/commands.rs`) |
| 5 | **TASK-01** (power subcommand + IPMI-via-server) | — runs alongside `phase-rerun/TASK-02` (disjoint files: ssh_installer vs power/CLI wiring) |

Dispatch rule: the coordinator dispatches TASK-01 only when every colliding wave-≤4 task above is merged to `origin/main` and the gate is green on `main`; the worker's `git rebase origin/main` in the brief's ⛔ START HERE block then picks up the final shape of the three shared CLI files.

## Constellation continuation — wave order (GLOBAL constellation numbering)

Execution mode: PARALLEL DISPATCH — RP-02/RP-03 fill disjoint CP-01 stubs — trigger: 2 tasks in one wave, zero shared files.

| Constellation wave | This WS runs | Must be MERGED first (cross-workstream) |
|---|---|---|
| 1 | — | `core-proto/TASK-01` (CP-01, workspace transform — creates the stub files `crates/uaa-core/src/power/{dash,amt_wol}.rs` and pre-wires `power/mod.rs` dispatch) |
| 2 | — | `core-proto/TASK-03` (CP-03, fleet config — `depends_on` of both RP tasks; finalizes registry/deny-list shape) |
| 3 | **TASK-02** (RP-02, AMD DASH) ∥ **TASK-03** (RP-03, Intel AMT + WoL) | — dispatched together: disjoint single-file stub fills |

Dispatch rule: the coordinator dispatches TASK-02 and TASK-03 concurrently only when CP-01 AND CP-03 are merged to `origin/main` and the gate (baseline 311 tests) is green on `main`. Neither task edits `power/mod.rs`, the CLI, or the other's file, so no serialization between them is needed; each brief's acceptance gates a single-file diff.

```mermaid
flowchart LR
  subgraph CW1[Constellation wave 1]
    CP01[core-proto/TASK-01 CP-01<br/>workspace + power stubs]
  end
  subgraph CW2[Constellation wave 2]
    CP03[core-proto/TASK-03 CP-03<br/>fleet config + deny-list]
  end
  subgraph CW3[Constellation wave 3]
    RP02[remote-power/TASK-02 RP-02<br/>dash.rs — AMD DASH]
    RP03[remote-power/TASK-03 RP-03<br/>amt_wol.rs — AMT + WoL]
  end
  CP01 --> CP03
  CP03 --> RP02
  CP03 --> RP03
```

No edge between `RP02` and `RP03` — they are parallel-safe (disjoint files).

## Coordinator / worker protocol

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

## Dependency graph

Edges mean "waits for the upstream task's MERGE" (file collisions from the operation matrix). Nodes outside Wave5 belong to other workstreams and are shown only because they gate this one. No edge between `RP01` and `PR02` — they are parallel-safe (disjoint files).

```mermaid
flowchart LR
  subgraph Wave1
    IR02[installer-robustness/TASK-02<br/>commands.rs]
  end
  subgraph Wave2
    IR03[installer-robustness/TASK-03<br/>commands.rs]
  end
  subgraph Wave3
    IR07[installer-robustness/TASK-07<br/>commands.rs]
  end
  subgraph Wave4
    PR01[phase-rerun/TASK-01<br/>args.rs + main.rs + commands.rs]
  end
  subgraph Wave5
    RP01[remote-power/TASK-01<br/>power subcommand + IPMI-via-server]
    PR02[phase-rerun/TASK-02<br/>ssh_installer — disjoint]
  end
  IR02 --> IR03
  IR03 --> IR07
  IR07 --> PR01
  PR01 --> RP01
```
