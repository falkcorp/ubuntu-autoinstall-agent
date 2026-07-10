<!-- file: docs/agent-tasks/pki/TASK-04-ca-cert-seed-embedding.md -->
<!-- version: 1.0.0 -->
<!-- guid: 1f3df54d-bafe-4f46-8ce1-000f03f3d99c -->
<!-- last-edited: 2026-07-10 -->

# TASK-04 — Bake install-ca.crt placement into the user-data template + USB bootstrap seed (ws4-pki)

**Priority:** P2 · **Effort:** S · **Recommended subagent:** Sonnet-class · rust-templates subagent · **Why:** touches the golden-tested template — regen goldens correctly or break the byte-for-byte suite. · **Depends on:** TASK-01 (PK-01) (wave-5 gated: PK-01 merged — the CA this cert placement bootstraps must exist server-side; parallel-safe with TASK-03, disjoint files)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/pki-ca-cert-seed-embedding" -b agent/pki-ca-cert-seed-embedding origin/main
cd "$REPO/.worktrees/pki-ca-cert-seed-embedding"
git rebase origin/main
```

(Protocol also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

## Goal

Make every installed host boot with `/etc/uaa/install-ca.crt` in place, so PK-02's `uaa enroll` finds its pinned CA (spec Decision 7: "install-CA public cert baked into ISO/PXE seed → agent pins it"). Three files, all additive:

1. `crates/uaa-core/src/autoinstall/templates/len-serv.user-data.tmpl` — add a late-command hunk that writes `/etc/uaa/install-ca.crt` (0644, dir 0755) into the target.
2. `crates/uaa-core/tests/fixtures/golden/len-serv-00{1,2,3}.user-data` — regenerated via the EXISTING `REGEN_GOLDEN=1` mechanism, **never hand-edited**.
3. `installer-image/nocloud/user-data` — the USB/ISO seed gets a matching `write_files` entry so SSH-ready/USB installs get the same file.

**The certificate content is a placeholder block at plan time — no real CA cert is ever committed.** Use the literal marker `UAA_INSTALL_CA_CRT_REPLACE_AT_PLACE_TIME` (same convention as the existing `TANG_INITIAL_PASSPHRASE_REPLACE_WITH_CLEVIS` placeholder already in this template — verify: `grep -n "REPLACE_WITH_CLEVIS" src/autoinstall/templates/len-serv.user-data.tmpl`); the placement pipeline (TP-02 / uaa-web PlaceSeed) substitutes the real PEM at place time. **NEVER use the CockroachDB CA** — the file this task stages is PK-01's install CA, nothing else.

REUSE — do not invent parallels: the golden regen flow is `REGEN_GOLDEN=1 cargo test --lib --offline regen_golden_fixtures` (`src/autoinstall/render.rs` — verify: `grep -n "REGEN_GOLDEN" src/autoinstall/render.rs`). Do NOT add a new regen script or edit golden files by hand. `render_user_data` (`src/autoinstall/render.rs` — verify: `grep -n "pub fn render_user_data" src/autoinstall/render.rs`) is unchanged: use a LITERAL placeholder string, NOT a new `{{...}}` template variable (the renderer errors on unfilled `{{VARS}}` — `grep -n "unfilled_placeholder_is_an_error" src/autoinstall/render.rs`).

## Background (verify before editing)

- Design spec: `docs/specs/constellation-design.md` Decision 7 + C6 (agent pins the seed-delivered CA; PK-02's default `--ca` path is `/etc/uaa/install-ca.crt`).
- The template is embedded with `include_str!` and golden-tested BYTE-FOR-BYTE against three fixtures (`renders_001_byte_for_byte` etc. in `src/autoinstall/render.rs`). Any template edit REQUIRES a golden regen or the suite fails — that is by design; the regen test writes all three fixtures deterministically.
- The template's `late-commands` already contain an inline heredoc chroot script (YAML `- |` block, heredoc terminator at column 0 after YAML strips indentation) — mirror that placement style: write the cert INSIDE the target (`/target/etc/uaa/install-ca.crt` via the late-command context, or inside the existing chroot script where other `/etc` files are staged — pick ONE location and keep the heredoc-terminator rule).
- `installer-image/nocloud/user-data` is a live-session cloud-config (NO `autoinstall:` key) with an existing `write_files:` list — append one entry (`path: /etc/uaa/install-ca.crt`, `permissions: "0644"`, `owner: root:root`, placeholder content). Note this stages the CA in the LIVE session; the USB bootstrap flow (`uaa-usb-bootstrap.sh`, same seed dir) drives `uaa install`, whose template late-command places it in the TARGET — both paths must end with the file on the installed system.
- Edge semantics: an installed host whose seed was placed BEFORE the pipeline substitutes the placeholder would carry the literal marker in `/etc/uaa/install-ca.crt` — PK-02's client treats an unparseable CA as the fail-closed missing-CA case (abort + retry), so a stale placeholder never silently trusts anything. State that in the template comment.

**Path map:** after CP-01 (wave 1) merges, `src/**` lives at `crates/uaa-core/src/**` and the CLI at `crates/uaa/src/**` — the template moves to `crates/uaa-core/src/autoinstall/templates/len-serv.user-data.tmpl` and the goldens to `crates/uaa-core/tests/fixtures/golden/`. The greps below cite pre-move paths (verifiable on today's main); at execution time run them at the old path, then the mapped path. Zero hits at BOTH = STOP and report.

**HARD RULES (non-negotiable):**
- NO hardware actions. Validate ONLY in-repo (`cargo`) and, where a brief says so, the QEMU+swtpm harness (`scripts/vm-validate.sh`). Code that COULD touch hardware is written and unit-tested against mock executors only.
- NEVER wipe, write to, or deploy on 172.16.2.30 ("the server") or len-serv-003.
- `disk_device` is read from the live target at runtime, never guessed or hardcoded.
- ipmitool runs via `ssh 172.16.2.30`, never on macOS.
- NEVER power on unimatrixone (U1).
- No real secret in any file: `REPLACE_AT_PLACE_TIME` placeholders stay placeholders.
- Stay inside your worktree; never `git push`, `gh pr`, or merge — report done and stop.

Plus, PKI-specific: **NEVER use the CockroachDB CA**; no real certificate PEM in the template, seed, goldens, or tests.

- **Re-verify these anchors before editing** — line numbers drift; zero hits at both old and mapped path = STOP and report:
  ```bash
  grep -n "include_str" src/autoinstall/render.rs                    # expect: 4 hits (template line 30 + 3 goldens lines 72-74)
  grep -n "REGEN_GOLDEN" src/autoinstall/render.rs                   # expect: 2 hits (comment line 99 + check line 100)
  grep -n "REPLACE_WITH_CLEVIS" src/autoinstall/templates/len-serv.user-data.tmpl  # expect: 1 hit (~line 73 — the placeholder precedent)
  grep -n "late-commands:" src/autoinstall/templates/len-serv.user-data.tmpl       # expect: 1 hit (~line 83)
  grep -n "write_files:" installer-image/nocloud/user-data           # expect: 1 hit (~line 42)
  ```

## Step-by-step

1. Run the ⛔ START HERE block, then the anchor greps above (old path, then mapped path). Any double-zero result → STOP and report.
2. **Template edit** (`crates/uaa-core/src/autoinstall/templates/len-serv.user-data.tmpl`): in `late-commands`, add the cert placement — `mkdir -p /target/etc/uaa` then a heredoc writing `/target/etc/uaa/install-ca.crt` whose body is exactly:
   ```text
   -----BEGIN CERTIFICATE-----
   UAA_INSTALL_CA_CRT_REPLACE_AT_PLACE_TIME
   -----END CERTIFICATE-----
   ```
   plus `chmod 0644` on the file. Add a comment: content is substituted at place time (TP-02/uaa-web); an unsubstituted marker is unparseable → PK-02 fails closed. Keep the heredoc terminator at column 0 (YAML `- |` strips indentation — see the existing `CHROOT_SETUP` comment in the file).
3. **Regen goldens**: `REGEN_GOLDEN=1 cargo test --lib --offline regen_golden_fixtures` — confirm it prints `wrote .../len-serv-001.user-data` (and 002, 003). Then `cargo test --lib --offline` — the three byte-for-byte tests must pass against the regenerated fixtures.
4. **Verify the goldens changed ONLY by regen**: `git diff --stat` must show exactly the 3 golden files + the template (+ headers). `git diff crates/uaa-core/tests/fixtures/golden/ | grep '^+' | grep -v "install-ca\|BEGIN CERT\|END CERT\|mkdir\|chmod\|^+++\|REPLACE_AT_PLACE_TIME\|#"` → no unexplained additions (nothing hand-edited).
5. **Seed edit** (`installer-image/nocloud/user-data`): append the `write_files` entry from Background with the same 3-line placeholder body; bump its `# version:` header (it is `1.1.0` today → `1.2.0`) and `# last-edited:`, keep its guid `7c1a9e40-2b56-4d81-9f3a-6e0c2d4b8a10`.
6. Bump the file header (version + last-edited) on every file you touch; new files get a fresh 4-line header with a new uuid4 (`uuidgen | tr 'A-F' 'a-f'`). (Golden fixtures carry no headers — they are generated bytes; do not add headers to them.)

## How to test

```bash
cargo test --lib --offline && cargo build --offline
# Expected: all tests pass (baseline 311 + prior waves; the 3 byte-for-byte golden tests green against REGENERATED fixtures), build clean
cargo clippy --offline -- -D warnings
# Expected: no warnings
grep -c "UAA_INSTALL_CA_CRT_REPLACE_AT_PLACE_TIME" crates/uaa-core/tests/fixtures/golden/len-serv-001.user-data crates/uaa-core/tests/fixtures/golden/len-serv-002.user-data crates/uaa-core/tests/fixtures/golden/len-serv-003.user-data
# Expected: 1 per file (placement rendered into every golden)
grep -n "install-ca.crt" installer-image/nocloud/user-data
# Expected: 1+ hits (write_files entry present)
grep -rn "BEGIN CERTIFICATE" crates/uaa-core/src/autoinstall/templates/ installer-image/nocloud/user-data | grep -v "REPLACE_AT_PLACE_TIME" | grep -v "END CERTIFICATE"
# Expected: 0 hits (no real PEM body anywhere — placeholder only)
```

## Acceptance criteria

- [ ] Template places the CA: `grep -n "/etc/uaa/install-ca.crt" crates/uaa-core/src/autoinstall/templates/len-serv.user-data.tmpl` → ≥1 hit inside `late-commands`.
- [ ] Goldens regenerated, not hand-edited: `REGEN_GOLDEN=1 cargo test --lib --offline regen_golden_fixtures` run again produces a clean `git diff` on the 3 fixtures (regen is deterministic/idempotent), and all 3 `renders_00*_byte_for_byte` tests pass.
- [ ] Seed staged: `grep -n "install-ca.crt" installer-image/nocloud/user-data` → 1+ hits with `permissions: "0644"` adjacent.
- [ ] No real cert committed: the "BEGIN CERTIFICATE" grep in How-to-test returns 0 hits; the only body line is the `UAA_INSTALL_CA_CRT_REPLACE_AT_PLACE_TIME` marker.
- [ ] Anti-over-suppression: N/A
- [ ] `cargo test --lib --offline && cargo build --offline` exits 0; clippy clean.
- [ ] File headers bumped on every changed file (`git diff origin/main --stat` files each show a version bump; guids unchanged — `installer-image/nocloud/user-data` keeps `7c1a9e40-...`).

## Commit message

```
feat(pki): bake /etc/uaa/install-ca.crt placement into template + USB seed (ws4-pki)

len-serv.user-data.tmpl late-command writes the install-CA placeholder
block (UAA_INSTALL_CA_CRT_REPLACE_AT_PLACE_TIME — substituted at place
time, unparseable marker fails closed in the enroll client) into the
target; installer-image/nocloud/user-data gets the matching write_files
entry. Goldens regenerated via REGEN_GOLDEN=1 (never hand-edited); no
real certificate committed; never the cockroach CA.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

Additive: if `grep -n "UAA_INSTALL_CA_CRT_REPLACE_AT_PLACE_TIME" crates/uaa-core/src/autoinstall/templates/len-serv.user-data.tmpl installer-image/nocloud/user-data` hits in both files, already applied — run the Acceptance criteria checks instead of re-applying. Rollback = revert the single commit (template + 3 goldens + seed revert together, keeping the byte-for-byte suite green); render.rs, the renderer contract, and all crates stay untouched.
