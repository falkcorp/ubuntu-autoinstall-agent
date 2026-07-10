<!-- file: docs/agent-tasks/core-proto/TASK-01-workspace-conversion.md -->
<!-- version: 1.0.0 -->
<!-- guid: 948f6393-fc72-425e-a406-04c4d00230bf -->
<!-- last-edited: 2026-07-10 -->

# TASK-01 — Convert to a cargo workspace (uaa-core lib + uaa bin) with pre-declared stub modules and CLI variants (ws1-core)

**Priority:** P1 · **Effort:** L · **Recommended subagent:** Opus-class · rust-workspace-refactor subagent · **Why:** cross-cutting transform touching every path; wide-collision root task; behavior must be frozen (311 tests before == after) · **Depends on:** none (global wave 1 — this task runs ALONE; NOTHING else in the whole constellation plan may be dispatched until it is merged)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/core-proto-workspace-conversion" -b agent/core-proto-workspace-conversion origin/main
cd "$REPO/.worktrees/core-proto-workspace-conversion"
git rebase origin/main
```

(Protocol also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

## Goal

Convert the single-crate repo into the cargo workspace of spec Decision 17 (`docs/specs/constellation-design.md`): `crates/uaa-core` (library — everything under `src/` today EXCEPT `main.rs` and `cli/`), `crates/uaa` (CLI+agent: moved `main.rs` + `cli/`, with a small lib target so the cli unit tests stay inside `cargo test --lib`), root Cargo.toml becomes a virtual manifest with `members = ["crates/*"]` (glob — later crates NEVER edit the members list) and a `[workspace.dependencies]` table holding today's dependencies verbatim (CP-02 extends that table; you only move it). **Behavior is FROZEN:** `cargo test --lib --offline` reports the SAME 311 passing tests before and after; the three golden fixtures are byte-identical (moved, never regenerated); the release binary is still named `ubuntu-autoinstall-agent` (CI `musl-build.yml` and `scripts/build-musl.sh` reference that artifact path). This is a TRANSFORM (git mv + manifest surgery + mechanical import rewrites) — no logic changes anywhere. You ALSO pre-declare every future uaa-core stub module and every future `uaa` CLI subcommand variant (Step 6/7) — that pre-wiring is what de-collides waves 2–7 of the plan (skeleton stub-pattern collision row: each stub file gets EXACTLY ONE filling task later).

## Background (verify before editing)

- Today: one crate `ubuntu-autoinstall-agent`, no `[workspace]`, `src/lib.rs` declares 10 `pub mod` (autoinstall, cli, config, error, image, logging, network, power, security, utils) + `pub use error::{AutoInstallError, Result};`.
- `src/main.rs` dispatches via the fully-qualified idiom `ubuntu_autoinstall_agent::cli::args::Commands::<Variant>` (13 match arms); `src/cli/commands.rs` carries many `#[cfg(test)]` unit tests that MUST remain in `--lib` scope after the move (hence the `crates/uaa` lib target).
- Golden fixtures: `src/autoinstall/render.rs` does `include_str!("templates/len-serv.user-data.tmpl")` and `include_str!("../../tests/fixtures/golden/len-serv-00{1,2,3}.user-data")` — the relative depth `../../tests/` is preserved if fixtures move to `crates/uaa-core/tests/fixtures/golden/`.
- `tests/integration_test.rs` imports `ubuntu_autoinstall_agent::` — it moves to `crates/uaa-core/tests/` and its `use` lines change to `uaa_core::`.
- `tests/workflow_scripts/**` and `tests/integration/**` are Python — leave them at the repo root untouched.
- Missing/edge semantics: a stub module is an EMPTY compiling file (header + `//!` doc naming its filler) — no types, no `todo!()` at module level; a pre-wired CLI command IS `todo!()` at runtime (panics if invoked — acceptable, tests never invoke them, `--help` still lists them).

**HARD RULES (non-negotiable):**
- NO hardware actions. Validate ONLY in-repo (`cargo`) and, where a brief says so,
  the QEMU+swtpm harness (`scripts/vm-validate.sh`). Code that COULD touch hardware
  is written and unit-tested against mock executors only.
- NEVER wipe, write to, or deploy on 172.16.2.30 ("the server") or len-serv-003.
- `disk_device` is read from the live target at runtime, never guessed or hardcoded.
- ipmitool runs via `ssh 172.16.2.30`, never on macOS.
- NEVER power on unimatrixone (U1).
- No real secret in any file: `REPLACE_AT_PLACE_TIME` placeholders stay placeholders.
- Stay inside your worktree; never `git push`, `gh pr`, or merge — report done and stop.

- **Re-verify these anchors before editing** — line numbers drift; zero hits at both old and mapped path = STOP and report:
  ```bash
  grep -c "\[workspace\]" Cargo.toml                 # expect: 0
  grep -n "pub mod" src/lib.rs                       # expect: 10 hits
  grep -n "^name = " Cargo.toml                      # expect: 1 hit (ubuntu-autoinstall-agent)
  grep -n "include_str" src/autoinstall/render.rs    # expect: 4 hits (template + 3 goldens)
  grep -c "ubuntu_autoinstall_agent::cli::args::Commands::" src/main.rs   # expect: 13
  grep -n "ubuntu-autoinstall-agent" .github/workflows/musl-build.yml scripts/build-musl.sh | head -4  # expect: hits (frozen artifact path)
  ```

## Step-by-step

1. Run the ⛔ START HERE block, then the anchor greps. Record the baseline: `cargo test --lib --offline 2>&1 | grep "test result:"` — expect `311 passed; 0 failed`. Write the number down; it is the frozen contract.
2. **Root manifest → virtual workspace.** Rewrite `Cargo.toml` (bump its header comment if present; it currently has none — TOML manifests are exempt from the 4-line header rule) to exactly this shape, moving EVERY `[dependencies]`/`[dev-dependencies]` entry verbatim (same versions, same features) into `[workspace.dependencies]`:
   ```toml
   [workspace]
   resolver = "2"
   members = ["crates/*"]

   [workspace.package]
   version = "0.1.0"
   edition = "2021"
   license = "MIT"
   repository = "https://github.com/jdfalk/ubuntu-autoinstall-agent"

   [workspace.dependencies]
   clap = { version = "4.4", features = ["derive"] }
   # ... every existing dep + dev-dep moved verbatim (tokio, serde, serde_yaml,
   # serde_json, anyhow, thiserror, tracing, tracing-subscriber, reqwest, ssh2,
   # ring, sha2, tempfile, walkdir, dirs, nix, indicatif, uuid, async-trait,
   # futures, regex, chrono, libc, tokio-test) ...
   ```
3. **Move the tree with `git mv` (preserves rename detection):**
   ```bash
   mkdir -p crates/uaa-core crates/uaa/src
   git mv src crates/uaa-core/src
   git mv crates/uaa-core/src/main.rs crates/uaa/src/main.rs
   git mv crates/uaa-core/src/cli crates/uaa/src/cli
   mkdir -p crates/uaa-core/tests
   git mv tests/integration_test.rs crates/uaa-core/tests/integration_test.rs
   git mv tests/fixtures crates/uaa-core/tests/fixtures
   ```
   Do NOT move `tests/workflow_scripts/` or `tests/integration/` (Python, root-level CI concerns).
4. **`crates/uaa-core/Cargo.toml`** (new): `[package] name = "uaa-core"`, `version.workspace = true`, `edition.workspace = true`, `license.workspace = true`, `repository.workspace = true`, description from the old manifest; `[dependencies]` referencing every moved dep as `<name> = { workspace = true }`; `[dev-dependencies] tokio-test = { workspace = true }`, `tempfile = { workspace = true }`. The lib target is implicit (`src/lib.rs`), crate import name `uaa_core`.
5. **`crates/uaa/Cargo.toml`** (new): `[package] name = "uaa"` + the same `*.workspace = true` package fields; `[lib] path = "src/lib.rs"` (implicit is fine); `[[bin]] name = "ubuntu-autoinstall-agent"` with `path = "src/main.rs"` — the bin name is FROZEN so `target/x86_64-unknown-linux-musl/release/ubuntu-autoinstall-agent` (musl-build.yml + build-musl.sh) still resolves; `[dependencies] uaa-core = { path = "../uaa-core" }` plus `{ workspace = true }` refs for what the cli/main actually use (clap, tokio, tracing, serde, serde_yaml, serde_json, uuid, tempfile, regex, chrono — add more only if the compiler asks); same two dev-deps. Create `crates/uaa/src/lib.rs` (fresh 4-line header, new uuid) containing exactly `pub mod cli;` — this keeps the cli unit tests inside `cargo test --lib`.
6. **Mechanical import rewrite** (only in `crates/uaa/src/**` and `crates/uaa-core/tests/integration_test.rs`):
   - `crates/uaa/src/main.rs`: `ubuntu_autoinstall_agent::cli` → `uaa::cli` (all 13 match arms + the `use` block); `ubuntu_autoinstall_agent::{logging::logger, Result}`-style library imports → `uaa_core::...`.
   - `crates/uaa/src/cli/*.rs`: every `crate::<mod>` where `<mod>` is a LIBRARY module (power, error, autoinstall, network, config, image, utils, security, logging) → `uaa_core::<mod>`; `crate::cli::...` stays `crate::cli::...`. Nothing inside `crates/uaa-core/src/**` changes (its internal `crate::` paths are still correct).
   - `crates/uaa-core/tests/integration_test.rs`: `ubuntu_autoinstall_agent::` → `uaa_core::`.
7. **Pre-declare ALL stub modules** (the de-collision pre-wiring — this exact list, one header + one `//!` line each, no other content). Register each in `crates/uaa-core/src/lib.rs` (`pub mod ...;` alphabetical) or the named parent module:

   | Stub file | Registered in | Filled EXCLUSIVELY by |
   |---|---|---|
   | `crates/uaa-core/src/fleet.rs` | lib.rs | core-proto/TASK-03 (CP-03) |
   | `crates/uaa-core/src/discovery.rs` | lib.rs | core-proto/TASK-04 (CP-04) |
   | `crates/uaa-core/src/update.rs` | lib.rs | core-proto/TASK-05 (CP-05) |
   | `crates/uaa-core/src/pki.rs` | lib.rs | pki/TASK-02 (PK-02) |
   | `crates/uaa-core/src/tls.rs` | lib.rs | pki/TASK-03 (PK-03) |
   | `crates/uaa-core/src/luks_keys.rs` | lib.rs | luks-keys/TASK-01 then TASK-02 (LK-01/LK-02) |
   | `crates/uaa-core/src/luks_sync.rs` | lib.rs | luks-keys/TASK-03 (LK-03) |
   | `crates/uaa-core/src/config_place.rs` | lib.rs | tooling-port/TASK-02 (TP-02) |
   | `crates/uaa-core/src/vm_validate.rs` | lib.rs | tooling-port/TASK-04 (TP-04) |
   | `crates/uaa-core/src/iso/mod.rs` | lib.rs (`pub mod iso;`) | declares the two below only |
   | `crates/uaa-core/src/iso/remaster.rs` | iso/mod.rs | tooling-port/TASK-01 (TP-01) |
   | `crates/uaa-core/src/iso/image_build.rs` | iso/mod.rs | tooling-port/TASK-03 (TP-03) |
   | `crates/uaa-core/src/power/dash.rs` | power/mod.rs (`pub mod dash;`) | remote-power/TASK-02 (RP-02) |
   | `crates/uaa-core/src/power/amt_wol.rs` | power/mod.rs (`pub mod amt_wol;`) | remote-power/TASK-03 (RP-03) |

   Each stub: fresh 4-line `//` header (repo-relative path, version 1.0.0, `uuidgen | tr 'A-F' 'a-f'` guid, last-edited 2026-07-10) + one line like `//! AMD DASH power path — stub, filled exclusively by remote-power/TASK-02 (RP-02).`
8. **Pre-wire ALL new CLI variants** (this exact list): create six per-command module files under `crates/uaa/src/cli/` — `enroll.rs`, `luks.rs`, `iso.rs`, `config.rs`, `image.rs`, `vm_validate.rs` — each with a fresh header, an empty clap Args struct, and a `todo!()` handler, e.g.:
   ```rust
   //! `uaa enroll` — stub, filled exclusively by pki/TASK-02 (PK-02).
   #[derive(Debug, clap::Args)]
   pub struct EnrollArgs {}
   pub async fn enroll_command(_args: EnrollArgs) -> uaa_core::Result<()> {
       todo!("constellation: filled by pki/TASK-02 (PK-02)")
   }
   ```
   (Same shape for `LuksArgs`/`luks_command` → LK-01, `IsoArgs`/`iso_command` → TP-01, `ConfigArgs`/`config_command` → TP-02, `ImageArgs`/`image_command` → TP-03, `VmValidateArgs`/`vm_validate_command` → TP-04.) Declare all six in `crates/uaa/src/cli/mod.rs`. Append six tuple variants to `pub enum Commands` in `crates/uaa/src/cli/args.rs` (existing variants untouched): `Enroll(crate::cli::enroll::EnrollArgs)`, `Luks(crate::cli::luks::LuksArgs)`, `Iso(crate::cli::iso::IsoArgs)`, `Config(crate::cli::config::ConfigArgs)`, `Image(crate::cli::image::ImageArgs)`, `VmValidate(crate::cli::vm_validate::VmValidateArgs)`, each with a doc comment ending in `(constellation — not yet implemented)`. Append six match arms in `crates/uaa/src/main.rs` in the same fully-qualified style: `uaa::cli::args::Commands::Enroll(args) => uaa::cli::enroll::enroll_command(args).await,` etc. Later tasks then extend ONLY their own module file (the Args struct lives there) — args.rs/main.rs are never edited again.
9. **`.cargo/config.toml` + `scripts/build-musl.sh`:** confirm the artifact path `target/x86_64-unknown-linux-musl/release/ubuntu-autoinstall-agent` is unchanged (it is, because of the frozen `[[bin]] name`); add a one-line comment to each noting the workspace layout (`# workspace: binary built from crates/uaa`), bump both file headers.
10. **Golden byte-identity check:** `git diff origin/main --name-status | grep golden` must show pure renames (`R100`) for all three `len-serv-00*.user-data` files. If any shows `M` or a lower similarity, you regenerated a golden — STOP, restore it (`git checkout origin/main -- <old path>` then re-`git mv`), and re-check.
11. Run the gate; the summed `--lib` pass count across `uaa-core` + `uaa` must equal the Step-1 baseline (311) exactly — MORE or FEWER is a frozen-behavior violation: STOP and reconcile before committing.
12. Bump the file header (version + last-edited) on every file you touch; new files get a fresh 4-line header with a new uuid4 (`uuidgen | tr 'A-F' 'a-f'`).

## How to test

```bash
cargo test --lib --offline && cargo build --offline
# Expected: all tests pass (baseline 311, summed across uaa-core and uaa — exactly 311, no new tests in this task), build clean
cargo clippy --offline -- -D warnings
# Expected: no warnings
cargo build --offline 2>/dev/null && ./target/debug/ubuntu-autoinstall-agent --help | grep -E "enroll|luks|iso|config|image|vm-validate" | wc -l
# Expected: 6 (all pre-wired subcommands listed; none of them invoked)
git diff origin/main --name-status | grep golden
# Expected: three R100 rename lines, zero M lines (goldens byte-identical)
cargo test --offline
# Expected: integration tests also pass (integration_test.rs compiles against uaa_core::)
```

## Acceptance criteria

- [ ] Workspace transform complete: `grep -c "^\[workspace\]$" Cargo.toml` → 1; `grep -n 'members = \["crates/\*"\]' Cargo.toml` → 1 hit; `test -f crates/uaa-core/src/lib.rs && test ! -f src/lib.rs && echo OK` → OK.
- [ ] Behavior frozen: `cargo test --lib --offline` totals EXACTLY 311 passed / 0 failed across members; `git diff origin/main --name-status | grep golden | grep -c ^R100` → 3.
- [ ] Binary name frozen: `grep -n '^name = "ubuntu-autoinstall-agent"' crates/uaa/Cargo.toml` → 1 hit (in `[[bin]]`).
- [ ] All 14 stubs exist: `ls crates/uaa-core/src/{fleet,discovery,update,pki,tls,luks_keys,luks_sync,config_place,vm_validate}.rs crates/uaa-core/src/iso/{mod,remaster,image_build}.rs crates/uaa-core/src/power/{dash,amt_wol}.rs | wc -l` → 14, and each is registered (`grep -c "pub mod" crates/uaa-core/src/lib.rs` → 19).
- [ ] All 6 CLI pre-wirings exist: `ls crates/uaa/src/cli/{enroll,luks,iso,config,image,vm_validate}.rs | wc -l` → 6; `grep -c "constellation — not yet implemented" crates/uaa/src/cli/args.rs` → 6; `grep -c "uaa::cli::args::Commands::" crates/uaa/src/main.rs` → 19 (13 moved + 6 new).
- [ ] Members-glob rule holds: `grep -c "uaa-core\|uaa-proto\|uaa-control" Cargo.toml` → 0 (no crate ever named in the root manifest; deps move here only as `[workspace.dependencies]` entries in CP-02).
- [ ] Anti-over-suppression: N/A
- [ ] `cargo test --lib --offline && cargo build --offline` exits 0; clippy clean.
- [ ] File headers bumped on every changed file (`git diff origin/main --stat` files each show a version bump; guids unchanged; moved files keep their original guid with their `// file:` path updated to the new location).

## Commit message

```
refactor(workspace): convert to cargo workspace (uaa-core lib + uaa bin) with constellation stubs (ws1-core)

Root Cargo.toml becomes a virtual manifest (members = ["crates/*"] glob,
[workspace.dependencies] moved verbatim). src/** -> crates/uaa-core (library,
all 311 lib tests intact, goldens byte-identical R100 renames); main.rs + cli/
-> crates/uaa with a lib target so cli unit tests stay in --lib; [[bin]] name
frozen as ubuntu-autoinstall-agent for musl-build.yml/build-musl.sh. Pre-declares
14 uaa-core stub modules (fleet/discovery/update/pki/tls/luks_keys/luks_sync/
config_place/vm_validate/iso/{remaster,image_build}/power/{dash,amt_wol}) and 6
todo!() CLI variants (enroll/luks/iso/config/image/vm-validate) so waves 2-7
fill disjoint files without touching args.rs/main.rs again.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

Transform polarity: if `grep -n "^\[workspace\]$" Cargo.toml` hits AND `test -f crates/uaa-core/src/lib.rs` succeeds AND `test ! -f src/lib.rs` succeeds (old location empty, new location populated), the conversion is already applied — run the Acceptance criteria checks instead of re-applying. Rollback = revert the single commit: it is pure `git mv` renames plus manifest/import rewrites and empty stubs — reverting restores `src/**` and the single-crate manifest exactly; no generated artifacts, no data, no server state to unwind.
