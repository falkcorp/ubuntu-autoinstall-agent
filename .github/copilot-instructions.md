<!-- file: .github/copilot-instructions.md -->
<!-- version: 3.0.0 -->
<!-- guid: 4d5e6f7a-8b9c-0d1e-2f3a-4b5c6d7e8f9a -->
<!-- last-edited: 2026-06-13 -->

# ubuntu-autoinstall-agent — Additional Context

Org-wide coding standards (file headers, language rules, commit format) are at
**https://github.com/falkcorp/.github** and apply automatically to this repo.

For full project context: **CLAUDE.md** at the repo root.

## Project overview

Comprehensive Ubuntu Server auto-installer with ZFS encryption and error recovery. Language: Rust.

## Key directories

| Path | Purpose |
|------|---------|
| `src/` | Rust source code |
| `tests/` | Integration tests |
| `scripts/` | Build and utility scripts |
| `examples/` | Example configurations |

## Critical constraints

- Build on Linux only — `build-on-linux.sh` for native, `build-cross-platform.sh` for cross-compile
- ZFS-related code requires careful handling of encryption keys and recovery paths
