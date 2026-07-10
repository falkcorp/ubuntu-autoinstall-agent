<!-- file: docs/agent-tasks/control/TASK-08-spa-scaffold.md -->
<!-- version: 1.0.0 -->
<!-- guid: c2c6c056-b0ca-4cb7-abd7-09183665fde0 -->
<!-- last-edited: 2026-07-10 -->

# TASK-08 — Scaffold the React+Vite+TS SPA: machines list, pending approvals, discovery inbox, audit view (ws2-control)

**Priority:** P2 · **Effort:** L · **Recommended subagent:** Sonnet-class · web-frontend subagent · **Why:** greenfield SPA scaffold against the typed JSON API; no Rust — self-contained under `web/` with its own npm build gate. · **Depends on:** none (global wave-2 slot; no shared files with any other task — `web/` and `spa-build.yml` are brand new; runs first in this workstream, before CT-01)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/control-spa-scaffold" -b agent/control-spa-scaffold origin/main
cd "$REPO/.worktrees/control-spa-scaffold"
git rebase origin/main
```

(Protocol also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

## Goal

Create `web/` — a React + Vite + TypeScript SPA (spec Decision 19, owner-locked) with four pages against the operator JSON API that CT-07 will serve at :8443: **Machines** (list + status + boot_target + per-layer `consistent` flag), **Pending approvals** (machine approvals AND enrollment CSRs — SPKI fingerprint + claimed MAC/hostname + discovery-inbox correlation, per spec C6), **Discovery inbox** (unknown PXE MACs, dismiss action), **Audit view** (chained events + verify status). Plus `.github/workflows/spa-build.yml` CI.

NO Rust changes anywhere. Non-negotiables from Decision 19 + the CT-07 contract:
- **Committed lockfile** (`web/package-lock.json`) — CI uses `npm ci`.
- **`web/dist` is CI-built, NEVER hand-edited and never committed** — gitignore `web/dist/*` but COMMIT `web/dist/.gitkeep` (an ignore-exception) so CT-07's `rust-embed #[folder]` compiles before the first real build.
- Typed API client only — every page consumes `src/api/client.ts` + `src/api/types.ts`; the TS types mirror the operator API DTO names (MachineRow, EnrollmentRow, DiscoveredMacRow, AuditEventRow) so CT-07's `api_types.rs` and this file stay name-aligned.
- Auth is a plain redirect to `/auth/login` on 401 (GitHub OAuth lives server-side, CT-03) — NO client-side auth logic, no tokens in localStorage.

## Background (verify before editing)

- Spec: Decision 19 (React+Vite+TS via rust-embed; dist CI-built), Decision 3 (JSON+OpenAPI), C3 operator plane (route shapes), C6 (approval queue contents). The API paths to code against (normative, matching CT-07's table): `GET /api/machines`, `GET /api/machines/{mac}`, `POST /api/machines/{mac}/approve`, `POST /api/machines/{mac}/reinstall`, `GET /api/enrollments`, `POST /api/enrollments/{fp}/approve`, `POST /api/enrollments/{fp}/reject`, `GET /api/discovered`, `POST /api/discovered/{mac}/dismiss`, `GET /api/audit`, `GET /api/audit/verify`.
- Edge semantics (spell twice — here and Step 4): 401 response → `window.location = "/auth/login"`; 403 → inline "insufficient role" banner (no redirect loop); empty list → an explicit empty state ("no machines yet"), never a blank page; fetch/network error → retry-able error card. Mutations (approve/reinstall/dismiss) show a confirm dialog BEFORE the POST — reinstall's dialog carries the cooldown warning text.
- There is NO backend during this task (CT-07 lands three waves later): `vite.config.ts` dev-proxies `/api` + `/auth` to `http://localhost:8443` for future local dev; the build must succeed with zero network (`npm ci` from the committed lockfile is the only fetch, done once by CI).
- Node/npm exist on the runner and dev machines; pin `"engines": { "node": ">=20" }`.
- Page → API → states matrix (normative for Step 4):

  | Page | API helpers used | Mutations (confirm dialog) | Must render |
  |---|---|---|---|
  | Machines | `listMachines` | `approveMachine`, `reinstallMachine` (cooldown warning text) | loading / error / empty / table |
  | Approvals | `listMachines` (pending), `listEnrollments`, `listDiscovered` (correlation) | `approveMachine`, `approveEnrollment`, `rejectEnrollment` | loading / error / two empty states |
  | Discovery | `listDiscovered` | `dismissDiscovered` | loading / error / empty / table |
  | Audit | `listAudit`, `verifyAudit` | — (read-only) | loading / error / empty / verify banner |

- **Re-verify these anchors before editing** — line numbers drift; zero hits at both
  old and mapped path = STOP and report:
  ```bash
  ls web 2>/dev/null | wc -l                                    # expect: 0 (dir absent — this task creates it; nonzero = STOP, already applied?)
  grep -n "rust-embed" docs/specs/constellation-design.md       # expect: 2 hits (~lines 210, 457 — the dist embedding contract)
  grep -n "never hand-edited" docs/specs/constellation-design.md # expect: 1 hit (~line 215, Decision 19)
  grep -n "actions/checkout" .github/workflows/musl-build.yml   # expect: 1+ hits (mirror its checkout/cache idioms in spa-build.yml)
  ```

## Step-by-step

1. Run the ⛔ START HERE block, then every anchor grep above. `ls web | wc -l` nonzero → STOP (idempotency section applies).

2. **Scaffold.** `web/package.json` (react, react-dom, react-router-dom, typescript, vite, @vitejs/plugin-react, @types/*; scripts: `dev`, `build` = `tsc --noEmit && vite build`, `typecheck`), `web/tsconfig.json` (strict), `web/vite.config.ts` (react plugin + `/api`,`/auth` proxy → `http://localhost:8443`), `web/index.html`, `web/.gitignore` containing exactly `dist/*` and `!dist/.gitkeep`, `web/dist/.gitkeep` (empty, committed), `web/package-lock.json` (run `npm install` once to generate; commit it). Every source file gets the mandatory 4-line `<!-- file: … -->`-style header adapted to its comment syntax (`//` for ts/tsx, `<!-- -->` for html).
3. **API layer.** `web/src/api/types.ts` — interfaces `MachineRow`, `EnrollmentRow`, `DiscoveredMacRow`, `AuditEventRow`, `ApiError` mirroring the CRDB schema fields (mac, hostname, status, boot_target, tpm_ek, last_seen…; spki_fingerprint, state…; first_seen, dismissed…; seq, actor, action, outcome…). `web/src/api/client.ts` — one `apiFetch<T>(path, init?)` wrapper implementing the pinned edges: 401 → redirect `/auth/login`; 403 → throw `ForbiddenError`; !ok → throw `ApiError` with the body message; typed helpers `listMachines()`, `approveMachine(mac)`, `reinstallMachine(mac, confirm)`, `listEnrollments()`, `approveEnrollment(fp)`, `rejectEnrollment(fp)`, `listDiscovered()`, `dismissDiscovered(mac)`, `listAudit()`, `verifyAudit()`.
4. **Pages + shell.** `web/src/main.tsx` + `App.tsx` (router + nav shell with the four links), `pages/Machines.tsx` (table: hostname, mac, status badge, boot_target, consistent flag, last_seen; row actions approve/reinstall with confirm dialogs — reinstall dialog includes the cooldown warning), `pages/Approvals.tsx` (two sections: pending machines, pending CSRs showing SPKI fp + claimed MAC/hostname + matching discovery-inbox row when the MAC appears in `listDiscovered()`), `pages/Discovery.tsx` (inbox table + dismiss), `pages/Audit.tsx` (event table + a verify banner from `verifyAudit()`). EVERY page renders the three non-happy states: loading, typed error card (retry button), explicit empty state. Repeat the edge law: 401 redirects, 403 banners, empty ≠ blank.
5. **CI.** `.github/workflows/spa-build.yml` (new file, nothing else in `.github/` touched): trigger `pull_request` + `push` on `main` filtered to `web/**`; steps mirroring `musl-build.yml`'s checkout idiom (`actions/checkout@v4` with `submodules: recursive`), `actions/setup-node@v4` with `node-version: 20` + `cache: npm` + `cache-dependency-path: web/package-lock.json`, then `cd web && npm ci && npm run build`; upload `web/dist` as artifact `spa-dist`.
6. Bump the file header (version + last-edited) on every file you touch; new files get a fresh 4-line header with a new uuid4 (`uuidgen | tr 'A-F' 'a-f'`).

## How to test

```bash
cargo test --lib --offline && cargo build --offline
# Expected: all tests pass (baseline 311, unchanged — this task adds no Rust), build clean
cargo clippy --offline -- -D warnings
# Expected: no warnings (no Rust touched)
cd web && npm ci
# Expected: clean install from the committed lockfile, exit 0
cd web && npm run build
# Expected: tsc --noEmit clean + vite build writes web/dist/index.html + assets, exit 0
cd web && npx tsc --noEmit
# Expected: 0 type errors (strict mode)
git status --porcelain web/dist | grep -v ".gitkeep"
# Expected: no COMMITTED dist artifacts staged — dist/* stays ignored (build output present locally is fine)
git ls-files web/dist
# Expected: exactly one line — web/dist/.gitkeep
```

## Acceptance criteria

- [ ] `web/` builds from a cold checkout: `cd web && npm ci && npm run build` exits 0 (Expected: dist/index.html produced).
- [ ] Lockfile committed: `test -f web/package-lock.json` → exists and is tracked (`git ls-files web/package-lock.json` → 1 line).
- [ ] dist hygiene: `git ls-files web/dist` → EXACTLY `web/dist/.gitkeep`; `grep -n '!dist/.gitkeep' web/.gitignore` → 1 hit (Decision 19: dist never committed, placeholder embeddable).
- [ ] Four pages + typed client: `ls web/src/pages/Machines.tsx web/src/pages/Approvals.tsx web/src/pages/Discovery.tsx web/src/pages/Audit.tsx web/src/api/client.ts web/src/api/types.ts` → all exist; `grep -c "apiFetch" web/src/api/client.ts` ≥ 10 (all helpers go through the one wrapper).
- [ ] Pinned edges implemented: `grep -n "auth/login" web/src/api/client.ts` → 1+ hits (401 redirect); `grep -rn "empty" web/src/pages/ | head -4` → hits (explicit empty states); confirm dialogs on mutations (`grep -rn "confirm" web/src/pages/Machines.tsx` → 1+ hits).
- [ ] Anti-over-suppression: N/A
- [ ] CI present and Rust-free: `grep -n "npm ci" .github/workflows/spa-build.yml` → 1 hit; `git diff origin/main --stat` touches ONLY `web/**` and `.github/workflows/spa-build.yml`.
- [ ] `cargo test --lib --offline && cargo build --offline` exits 0; clippy clean (proves zero Rust impact).
- [ ] File headers bumped on every changed file (`git diff origin/main --stat` files each show a version bump; every new file carries a fresh 4-line header with its own uuid4).

## Commit message

```
feat(control): React+Vite+TS SPA scaffold — machines, approvals, discovery inbox, audit view + spa-build CI (ws2-control)

New web/ per spec Decision 19: strict-TS Vite app with a single typed
apiFetch client (401 -> /auth/login redirect, 403 banner, typed errors) and
four pages (machines w/ approve+reinstall confirm dialogs, pending machine
approvals + enrollment CSRs with discovery correlation, discovery inbox
dismiss, audit chain + verify banner), each with loading/error/empty states.
Committed package-lock.json; dist/* gitignored with a committed
dist/.gitkeep so CT-07's rust-embed folder compiles pre-build;
spa-build.yml runs npm ci && npm run build on web/** changes.
No Rust files touched.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

If `grep -n '"name"' web/package.json` hits (i.e. `web/package.json` exists), already applied — run the acceptance checks instead of re-applying. Rollback = revert the single commit; it removes `web/` and `spa-build.yml` entirely, and nothing else references them until CT-07 lands (CT-07's wave gate checks for `web/dist/.gitkeep`, so a rollback here correctly blocks CT-07 rather than breaking it).
