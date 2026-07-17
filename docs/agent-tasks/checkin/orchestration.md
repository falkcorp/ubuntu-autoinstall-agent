<!-- file: docs/agent-tasks/checkin/orchestration.md -->
<!-- version: 1.0.0 -->
<!-- guid: f00cc012-59b0-4947-86be-8dfe49009cd6 -->
<!-- last-edited: 2026-07-16 -->

# Orchestration — checkin workstream

Read the package-level [`../ORCHESTRATION.md`](../ORCHESTRATION.md) first. This file only adds the workstream-specific wave order. **Wave numbers are GLOBAL across the deploy-system package** — this workstream's tasks interleave with the other four.

## Waves (respect `Depends on:`)

```mermaid
flowchart LR
    subgraph Wave1
      K1[TASK-01 app_status reporter]
    end
    subgraph Wave4
      K2[TASK-02 machine-plane ingest]
    end
    subgraph Wave5
      K3[TASK-03 read-time staleness]
    end
    K1 --> K2
    K2 --> K3
```

- **Wave 1**: TASK-01 is a standalone client module mirroring `luks_sync`; it depends on nothing and touches no shared file.
- **Wave 4**: TASK-02 adds the ingest route and the additive `MachineRow` fields. It edits `db/mod.rs` — coordinate with the registry workstream's TASK-01/04 per the package collision table.
- **Wave 5**: TASK-03 reads TASK-02's `last_app_status_at`. Pure, read-time, no job.

## Coordinator protocol (verbatim)

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
> files); 3) file-copy cherry-pick fallback — re-apply the task’s file states onto a
> fresh branch from HEAD; 4) mark `rebase_blocked`, stop the lane, escalate to a human.
>
> **A wave MUST NOT start** while any of: the previous wave has an unmerged PR; any
> sibling worktree is un-rebased; the gate is red on `origin/main`; or a
> `rebase_blocked` marker is unresolved.

## Run it

```bash
# from docs/agent-tasks/checkin/
./run.sh                 # print task list + set up worktrees
./run.sh 01            # wave 1 — independent
./run.sh 02            # wave 4 (after 01 merged)
./run.sh 03            # wave 5 (after 02 merged)
```

After each wave: gate each worktree with `cargo test --lib --offline && cargo build --offline`, push/PR/merge as coordinator, then rebase every remaining sibling worktree onto `origin/main` before starting the next wave.
