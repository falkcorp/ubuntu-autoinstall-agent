<!-- file: docs/agent-tasks/operator-api/orchestration.md -->
<!-- version: 1.0.0 -->
<!-- guid: 113d660a-223d-4592-9deb-d4130ff07bc2 -->
<!-- last-edited: 2026-07-16 -->

# Orchestration — operator-api workstream

Read the package-level [`../ORCHESTRATION.md`](../ORCHESTRATION.md) first. This file only adds the workstream-specific wave order. **Wave numbers are GLOBAL across the deploy-system package** — this workstream's tasks interleave with the other four.

## Waves (respect `Depends on:`)

```mermaid
flowchart LR
    subgraph Wave4
      O1[TASK-01 /api/profiles routes]
    end
    subgraph Wave5
      O2[TASK-02 /api/drift routes]
    end
    subgraph Wave6
      O3[TASK-03 config place --from-registry]
      O4[TASK-04 SPA screens]
    end
    O1 --> O2
    O2 --> O4
```

- **Wave 4**: TASK-01 adds the profile route group and `AppState.profile_store`. Depends on DS-REG-02.
- **Wave 5**: TASK-02 also edits `handlers.rs`/`api_types.rs` — it MUST NOT start until TASK-01's PR is merged and its worktree is rebased. It also depends on DS-REG-05.
- **Wave 6**: TASK-03 (`config_place.rs`) and TASK-04 (`web/**`) touch disjoint files and run concurrently. TASK-03 additionally depends on DS-PRF-02 **and** DS-REG-03. **Execution mode for TASK-03: SINGLE-AGENT (strong model)** — judgment work with irreversible writes; never parallelized, never weak-tier.

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
# from docs/agent-tasks/operator-api/
./run.sh                 # print task list + set up worktrees
./run.sh 01            # wave 4
./run.sh 02            # wave 5 (after 01 merged + rebased)
./run.sh 03            # wave 6 — Opus-class, review-critical, alone
./run.sh 04            # wave 6 — parallel with 03 (disjoint files)
```

After each wave: gate each worktree with `cargo test --lib --offline && cargo build --offline`, push/PR/merge as coordinator, then rebase every remaining sibling worktree onto `origin/main` before starting the next wave.
