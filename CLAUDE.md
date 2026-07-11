<!-- file: CLAUDE.md -->
<!-- version: 1.1.0 -->
<!-- guid: b3c4d5e6-f7a8-4b9c-0d1e-2f3a4b5c6d7e -->
<!-- last-edited: 2026-07-10 -->

# ubuntu-autoinstall-agent

Comprehensive Ubuntu Server auto-installer with ZFS encryption and error recovery.

## Coding Standards

Org-wide coding standards are in the `.standards/` git submodule (cloned from `https://github.com/falkcorp/.github`).
Always clone with `git clone --recurse-submodules` so these are available.

Key files:
- **File headers (MANDATORY):** `.standards/instructions/file-headers.md`
- **Commit format:** `.standards/instructions/commit-messages.md`

## Documentation Conventions

**Executive summaries** (org-wide pattern, originated in `audiobook-organizer`):
`docs/status/YYYY-MM-DD-<slug>-executive-summary.md`. Plain-language,
non-technical/stakeholder-facing prose — explain what a bug/feature was, why
it mattered, and the fix in terms a non-engineer can follow; no unexplained
jargon. Structure: 4-line header; `# Executive Summary: <Title>`;
`**Shipped:**` line with PR link(s)/range + count; `**Related doc:**` link to
the technical/operational companion doc (kept separate — e.g.
`docs/constellation/`, `docs/specs/`, `docs/plans/`); one-paragraph scope
intro; `## Executive Summary` as bullets, one per theme (bold lead phrase,
plain explanation, why it matters, PR numbers as evidence); for
security-relevant or data-loss-risk work, add a `## Highest-risk items`
callout naming those items explicitly in plain language.

Keep the plain-language executive summary and the technical/operational
status doc as **separate files**, cross-linked — never merge engineering
detail into the executive summary or vice versa.
