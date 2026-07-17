<!-- file: docs/agent-tasks/operator-api/TASK-04-spa-screens.md -->
<!-- version: 1.0.0 -->
<!-- guid: 1041576a-0707-438d-982a-4f79b40f4032 -->
<!-- last-edited: 2026-07-16 -->

# TASK-04 — SPA: profile + drift screens, staleness rendering (DS-OPS-04)

**Priority:** P3 · **Effort:** M · **Recommended subagent:** Haiku-class · frontend subagent · **Why:** mechanical — follows the existing page/client patterns (Machines, Approvals, Audit) exactly. · **Depends on:** DS-OPS-02 (needs `/api/drift`)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/operator-api-spa-screens" -b agent/operator-api-spa-screens origin/main
cd "$REPO/.worktrees/operator-api-spa-screens"
git rebase origin/main
```

(Protocol also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

**Wave gate:** DS-OPS-02 must be merged. If `grep -n '"/api/drift"' crates/uaa-control/src/operator/handlers.rs` returns 0 hits, the gate is not met: STOP and report.

## Goal

Add two SPA screens — **Profiles** (groups, members, allocations) and **Drift** (review queue with accept/revert) — and render application health with its **staleness**.

REUSE — do not invent parallels:

- **`apiFetch`** — verify: `grep -n "export async function apiFetch" web/src/api/client.ts`. Every call goes through it; it already handles the `ApiErrorBody` shape on non-2xx. Do NOT hand-roll `fetch`.
- **The existing page pattern** — verify: `ls web/src/pages/` (Machines, Approvals, Audit, Discovery, Login). Mirror the closest one; do not introduce a new state-management or styling approach.
- **`web/src/api/types.ts`** — add DTOs **name-aligned with `api_types.rs`** (that file's header states the two sides are deliberately kept name-identical). Do not rename fields on the way through.
- **The repo's UI standard**: a real layout — collapsible/pinnable sidebar + header/banner — already exists from the operator SPA redesign. New screens live **inside** it; do not ship a bare unstyled page.

## Background (verify before editing)

- **No new server route is needed.** The SPA is `rust_embed`-served with an index fallback — verify: `grep -n "pub fn router" crates/uaa-control/src/operator/web_ui.rs`. New screens are **client-side routes only**.
- **⚠ Three renderings are normative, not cosmetic.** Get these wrong and the UI actively misleads:
  1. **Revert must not read as "fleet fixed".** Revert restores **intent, not the machine** — the deployed host stays as drifted as it was (spec D11). DS-OPS-02's revert response carries a `note` saying so: **display it**. Do not summarise it away.
  2. **`Stale` is not `healthy` and not `unhealthy`.** DS-CHK-03's `Freshness` is `Fresh | Stale | NeverReported`. `Stale` means *we don't know*. A machine that reported `active: true` an hour ago must **not** render green — that is the exact bug DS-CHK-03 exists to fix. Render three visually distinct states, and show "last reported at T" rather than only a health dot.
  3. **`NeverReported` ≠ `Stale`.** A machine that never reported is a different fact from one that stopped.
- **Vocabulary:** `MachineRow` already carries `consistent: boolean` — *"True when every provisioning layer for this machine agrees; false = drift"*. Reuse that word; do not add a synonym.
- Edge semantics (spelled out here AND in acceptance):
  - **Empty drift list** → an explicit "no drift detected" empty state. NOT a spinner, NOT a blank panel, NOT an error.
  - **Empty group list** → an empty state pointing at how to create one.
  - **A 403 from a mutating call** (a Viewer clicking accept) → a readable message, not a stack trace. Ideally the control is disabled for non-Operators.
  - **A machine with zero applications and a recent report** → renders `Fresh`, "0 applications". "No applications" ≠ "no news".

**HARD RULES (non-negotiable):**
- NO hardware actions. NEVER wipe/write/deploy on 172.16.2.30 or len-serv-003. NEVER power on unimatrixone.
- No real secret in any file; no token, no MAC-derived secret in client code.
- Purely additive: do NOT modify existing pages, `apiFetch`, or the layout shell.
- Do NOT add a state-management library or a UI framework — match what is there.
- Stay inside your worktree; never `git push`, `gh pr`, or merge — report done and stop.

- **Re-verify these anchors before editing** — line numbers drift; zero hits = STOP and report:
  ```bash
  grep -n '"/api/drift"' crates/uaa-control/src/operator/handlers.rs
  # expect: >=1 hit — DS-OPS-02 merged (0 = wave gate not met, STOP)
  grep -n "export async function apiFetch" web/src/api/client.ts
  # expect: 1 hit (~line 41) — the ONLY call path
  grep -n "consistent" web/src/api/types.ts
  # expect: 1 hit — the existing drift vocabulary to reuse
  grep -n "pub fn router" crates/uaa-control/src/operator/web_ui.rs
  # expect: 1 hit — the SPA fallback; proof no new server route is needed
  ```
  Also read one existing page end to end before writing: `web/src/pages/Machines.tsx`.

## Step-by-step

1. In `web/src/api/types.ts`, add interfaces name-aligned with `api_types.rs`: `HostGroupView`, `HostProfileView`, `AllocationView`, `DriftView`, `ReviewResultView`, and the `Freshness` union (`"fresh" | "stale" | "never_reported"`). Bump the file header.
2. In `web/src/api/client.ts`, add typed wrappers via `apiFetch`, mirroring the existing `getMachines`/`approveMachine` style: `getGroups`, `getGroupProfiles`, `getAllocations`, `getDrift`, `acceptDrift`, `revertDrift`.
3. Add `web/src/pages/Profiles.tsx` — groups list → members + allocations (index, hostname, identity, `released_at`/`rebound_to` if set).
4. Add `web/src/pages/Drift.tsx` — the review queue: object, stored vs actual hash, `seen_count`, and Accept / Revert buttons. **After a revert, display the response's `note` verbatim.**
5. Render application health wherever machines are shown, using DS-CHK-03's three states + "last reported at T". Three visually distinct states; `Stale` must not look like `Fresh`.
6. Register both pages in the existing router/nav, inside the existing sidebar+header layout.
7. Add tests if and only if the repo already has a frontend test harness — verify: `ls web/src/**/*.test.tsx 2>/dev/null; grep -n '"test"' web/package.json`. **If there is no harness, do NOT add one**; say so in your report and rely on the build + the manual checks below.
8. Bump headers on every file you touch; keep existing guids.

**Anti-over-suppression:** the empty states are the guard. `Drift.tsx` with no drift must render an explicit "no drift detected" — not a spinner and not a blank panel, either of which makes a healthy fleet look broken or the page look hung. Same for an empty group list. Verify both manually (below).

## How to test

```bash
cd web && npm ci && npm run build
# Expected: exit 0, no TypeScript errors.
npm run lint 2>/dev/null || echo "no lint script — skip"
# Expected: clean, or the skip message.

cd .. && cargo build --offline
# Expected: exit 0 — the SPA is rust_embed'd, so a build break shows up here too.
cargo test --lib --offline
# Expected: 634+ passed — unchanged; this task touches no Rust logic.
```

Manual checks (no live server needed — use the dev server against a stub, or reason from the code and say so in your report):
- Drift page with an empty list ⇒ "no drift detected" empty state, not a spinner.
- A machine with `last_app_status_at` 2 hours old ⇒ renders **Stale**, not green.

## Acceptance criteria

- [ ] `npm run build` exits 0 — verify: `cd web && npm run build && echo BUILD_OK`
- [ ] `cargo build --offline` exits 0 (SPA is embedded) — verify: `cargo build --offline && echo BUILD_OK`
- [ ] `cargo test --lib --offline` still green — verify: `cargo test --lib --offline 2>&1 | grep -E "^test result"`
- [ ] Both pages exist — verify: `ls web/src/pages/Profiles.tsx web/src/pages/Drift.tsx`
- [ ] All calls go through `apiFetch` — verify: `grep -c "fetch(" web/src/pages/Profiles.tsx web/src/pages/Drift.tsx` returns **0** for both
- [ ] **Three distinct freshness states are rendered** — verify: `grep -c "stale\|never_reported\|fresh" web/src/pages/*.tsx` returns ≥3
- [ ] **The revert note is displayed, not summarised** — verify: `grep -ci "note" web/src/pages/Drift.tsx` returns ≥1
- [ ] Anti-over-suppression: explicit empty states — verify: `grep -ci "no drift" web/src/pages/Drift.tsx` returns ≥1
- [ ] No new dependency — verify: `git diff origin/main --name-only | grep -c "package.json\|package-lock.json"` returns **0**
- [ ] Existing pages untouched — verify: `git diff origin/main --name-only -- web/src/pages/ | grep -c "Machines.tsx\|Approvals.tsx\|Audit.tsx\|Login.tsx\|Discovery.tsx"` returns **0**
- [ ] Type names align with `api_types.rs` — verify: `grep -c "HostGroupView\|DriftView" web/src/api/types.ts` returns ≥2
- [ ] File headers bumped — verify: `git diff origin/main --name-only | xargs -I{} grep -l "last-edited: 2026-07" {}`

## Commit message

```
feat(web): add Profiles and Drift screens with staleness rendering (DS-OPS-04)

Two client-side screens inside the existing sidebar+header layout — no new
server route is needed, since the SPA is rust_embed'd with an index fallback.
All calls go through the existing apiFetch; DTO names stay aligned with
api_types.rs.

Three renderings are normative, not cosmetic:
- Revert displays DS-OPS-02's note verbatim: revert restores INTENT, not the
  machine. The deployed host stays as drifted as it was.
- Stale renders distinctly from Fresh. A machine that reported active:true an
  hour ago is Stale, not green — the bug DS-CHK-03 exists to fix.
- NeverReported is distinct from Stale: never reported is a different fact
  from stopped reporting.

Empty states are explicit ("no drift detected") rather than a spinner, so a
healthy fleet does not look broken.

Co-Authored-By: Claude <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

**Polarity: additive.** If `test -f web/src/pages/Drift.tsx` succeeds, this task is already applied — run the acceptance checks instead of re-applying. Rollback = revert the single commit; existing pages, `apiFetch`, and the layout are untouched, the API routes remain (just without a UI), and no data or schema is affected. No sibling shares these files.
