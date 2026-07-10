<!-- file: docs/constellation/00-ROADMAP.md -->
<!-- version: 1.0.0 -->
<!-- guid: 7df2b381-5df5-41f5-b824-5e6c7214c1be -->
<!-- last-edited: 2026-07-10 -->

# uaa Constellation — Roadmap (2026-07-10)

Findings produced by a 3-scout grep-verified evidence sweep of the repo + server docs,
adversarially reviewed by a 3-lens design-judge panel (correctness / ops-rollback /
simplicity-scope); every claim cites file:line in the linked docs. Ranking = impact ×
effort. Sequencing: foundation → parity → security → features → retire.

Detail docs (full detail lives there, not here):

| Doc | Scope | Items |
|-----|-------|-------|
| [../specs/constellation-design.md](../specs/constellation-design.md) | architecture, 25 locked decisions, data model, components | 25 decisions |
| [../specs/constellation-plan.md](../specs/constellation-plan.md) | taskboard: waves, collision matrix, tiers, protocol | 42 tasks / 9 waves |
| [../agent-tasks/BREAKDOWN-2026-07-10.md](../agent-tasks/BREAKDOWN-2026-07-10.md) | bucket sort + fan-out strategy | 42 + B2/B3 |

## Headline conclusions

1. **The serving plane is greenfield.** No Rust HTTP server exists (grep-verified);
   everything on :25000 is the ~731-line Python mirror. Parity (IP-01..04) is the
   pivotal risk-retirement step — everything else hangs off its fixture suite.
2. **The workspace conversion (CP-01) is the one task that collides with everything.**
   It runs alone in wave 1, Opus-class, behavior-frozen (311 tests before == after),
   and pre-creates every stub + CLI variant that lets waves 2–7 parallelize.
3. **Reuse is high:** render/verify/place/power/config import as-is behind the
   `CommandExecutor` seam; the new services are mostly new glue, not rewrites.
4. **The judges materially changed the design:** dnsmasq hostsdir (not conf.d), serialized
   audit chain, quiesce-import-swap cutover with export-first rollback, service-cert
   bootstrap, CA-backup ship-gate, ordered SAGA. See spec verdict markers (⚡/🔒).

## Rank 0 — Do immediately (P0; waves 1–4)

| # | Item | IDs | Why now |
|---|------|-----|---------|
| 1 | Workspace + proto foundation | CP-01..03, CP-06, TG-04 | unblocks all 10 workstreams |
| 2 | Registry + machine-plane parity | CT-01..02, IP-01..04 | replaces the unversioned Python SoR; M2 gate |
| 3 | Discovery inbox + approve SAGA + reinstall | PX-03, CT-05, CT-06 | the P0 operator features |
| 4 | Config templating + verify sweep reuse | (in CT-06/IP briefs via render.rs/verify.rs) | P0, nearly free through reuse |
| 5 | Signed audit log | CT-04 | P0; serialized chain per Decision 21 |

## Rank 1 — Next (P1; waves 3–8)

| # | Item | IDs | Notes |
|---|------|-----|-------|
| 6 | **Enrollment PKI + mTLS + CRL** | PK-01..04 | gated on the CA-backup ceremony (Bucket 3) |
| 7 | **Self-update channel** | CP-05, WB-04 | manifest-revert-first rollback; daemons stage-only |
| 8 | **uaa-web placement + ISO self-service** | WB-01..03, TP-01, TP-03 | detached builds only |
| 9 | **LUKS FIDO2 + Tang rotation guard** | LK-01..03 | t=2-of-3 cold-start guard fail-closed |
| 10 | **Power finish** | RP-02..03 | mock-validated only; hardware validation deferred |
| 11 | **E2E VM gate** | TG-03 | M5 — THE gate before any hardware |
| 12 | Registry backup/restore | CT-02 export + Bucket-3 runbook | pairs with cutover rollback |

## Rank 2 — Later (P2/P3)

- **DNS record management** — PX-04 (P3, mechanical).
- **Health-dashboard aggregation, config-mgmt UI** — Bucket 2 (design first).
- **arm64/RPi agent + template variant** — Bucket 2 (boot chain undesigned).
- **Out-of-band audit witness** — Bucket 2 (threat-model boundary recorded in spec).
- **Cutover + retirement** — M6/W9 (TP-05) — operational gate, ≥2-week rollback window.

## Deferred-work verdicts

| Item | Verdict |
|------|---------|
| GraphQL browser API | **Defer** — JSON+OpenAPI locked; owner open to revisit (Decision 3) |
| No-SPA htmx console | **Kill** — owner locks the Node SPA (Decision 19, objection recorded) |
| Static-only discovery (drop mDNS) | **Kill** — owner locks mDNS (Decision 11, objection recorded) |
| SQLite registry | **Kill** — CRDB locked (Decision 4) |
| uaa-power / uaa-luks daemons | **Kill** — library + subcommand suffice (Decisions 14/15) |
| BSR publish | **Defer** — optional Bucket 3; builds never need it (Decision 18) |

## What was explicitly validated (don't re-fix)

- Install-ops package fully shipped (20/20 tasks, 311 tests, PRs #27–#50 era) — webhook
  flip widening, /api/health, /api/uaa-configs, --inject-from, /dashboard all live in
  the Python mirror today.
- Static musl build proven (`uaa-amd64` artifact + ldd verification in CI).
- Golden render pipeline (byte-for-byte fixtures) and the 19-check verify sweep.
- QEMU+swtpm harness stages 0–7 (LUKS + rpool/bpool + multi-user asserts).
