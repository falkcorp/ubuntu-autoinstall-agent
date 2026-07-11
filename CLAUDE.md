<!-- file: CLAUDE.md -->
<!-- version: 1.2.0 -->
<!-- guid: b3c4d5e6-f7a8-4b9c-0d1e-2f3a4b5c6d7e -->
<!-- last-edited: 2026-07-11 -->

# ubuntu-autoinstall-agent

Comprehensive Ubuntu Server auto-installer with ZFS encryption and error recovery.

## Coding Standards

Org-wide coding standards are in the `.standards/` git submodule (cloned from `https://github.com/falkcorp/.github`).
Always clone with `git clone --recurse-submodules` so these are available.

Key files:
- **File headers (MANDATORY):** `.standards/instructions/file-headers.md`
- **Commit format:** `.standards/instructions/commit-messages.md`

## Documentation Conventions

Org-wide pattern, originated in `audiobook-organizer` and adopted here in
full:

- **Executive summaries** — `docs/executive-summaries/`, convention in
  [`docs/process/executive-summaries.md`](docs/process/executive-summaries.md).
  Polished, stakeholder-facing narrative; plain language, no jargon, no
  file paths in the body.
- **Status reports** — `docs/status/`, convention in
  [`docs/process/status-reports.md`](docs/process/status-reports.md).
  Terse, internal/operational; file:line references and jargon are fine.

The two are not mutually exclusive — a large execution wave usually
produces both, cross-linked. Read both convention docs before writing
either; do not improvise the structure.
