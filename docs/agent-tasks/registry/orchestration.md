<!-- file: docs/agent-tasks/registry/orchestration.md -->
<!-- version: 1.0.0 -->
<!-- guid: a68c9f87-6e46-470e-89cc-dd79570d09cb -->
<!-- last-edited: 2026-07-16 -->

# Orchestration — registry workstream

Read the package-level [`../ORCHESTRATION.md`](../ORCHESTRATION.md) first. This file only adds the workstream-specific wave order. **Wave numbers are GLOBAL across the deploy-system package** — this workstream's tasks interleave with the other four.

## Waves (respect `Depends on:`)

```mermaid
flowchart LR
    subgraph Wave1
      R1[TASK-01 rows + scaffold]
    end
    subgraph Wave2
      R2[TASK-02 ProfileStore + read_snapshot_strict]
    end
    subgraph Wave3
      R3[TASK-03 allocate + rebind]
    end
    subgraph Wave4
      R4[TASK-04 content_hash + versions]
    end
    subgraph Wave5
      R5[TASK-05 drift scan + accept/revert]
    end
    R1 --> R2
    R2 --> R3
    R2 --> R4
    R4 --> R5
```

- **Wave 1**: TASK-01 creates the row types, the `SnapshotDoc` collections, and the empty `profiles/` stubs every sibling fills.
- **Wave 2**: TASK-02 fills `store.rs` and adds `read_snapshot_strict`. It also edits `db/store.rs`, which TASK-01 touched — hence the serialization.
- **Wave 3**: TASK-03 fills the allocation stubs in `profiles/store.rs` (shared with TASK-02) and `alloc.rs`.
- **Wave 4**: TASK-04 fills `drift.rs`'s hashing half and re-edits `db/mod.rs` (shared with TASK-01).
- **Wave 5**: TASK-05 fills the rest of `drift.rs` (shared with TASK-04).

This track starts first in the global wave order: TASK-03's fail-closed allocation is the design's core safety property and needs the most soak time.

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
# from docs/agent-tasks/registry/
./run.sh                 # print task list + set up worktrees
./run.sh 01            # wave 1
./run.sh 02            # wave 2 (after 01 merged)
./run.sh 03            # wave 3 — Opus-class, review-critical
./run.sh 04            # wave 4
./run.sh 05            # wave 5 — Opus-class, review-critical
```

After each wave: gate each worktree with `cargo test --lib --offline && cargo build --offline`, push/PR/merge as coordinator, then rebase every remaining sibling worktree onto `origin/main` before starting the next wave.
