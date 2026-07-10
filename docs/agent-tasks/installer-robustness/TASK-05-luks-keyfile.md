<!-- file: docs/agent-tasks/installer-robustness/TASK-05-luks-keyfile.md -->
<!-- version: 1.0.0 -->
<!-- guid: aeca8536-09b0-4484-8edb-46950f35ef50 -->
<!-- last-edited: 2026-07-09 -->

# TASK-05 — LUKS passphrase via 0600 tempfile keyfile (kill env export + cryptsetup command-line interpolation) (todo:LUKS_KEY-env)

**Priority:** P1 · **Effort:** M · **Recommended subagent:** Opus-class · rust-security subagent · **Why:** SECURITY: secret visible in /proc/&lt;pid&gt;/environ and ps command lines; touching luksFormat is wipe-adjacent · ⚠ review-critical · **Depends on:** none

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/installer-robustness-luks-keyfile" -b agent/installer-robustness-luks-keyfile origin/main
cd "$REPO/.worktrees/installer-robustness-luks-keyfile"
git rebase origin/main
```

(Protocol is also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

## Goal

Stop the LUKS passphrase from ever appearing on a shell command line or in an env export. Today `DiskManager::setup_luks_encryption` (src/network/ssh_installer/disk_ops.rs) pipes the passphrase inline — `echo '<pass>' | cryptsetup luksFormat/open ...` — which makes it visible in `ps` output and shell history on the target, and `SshInstaller::setup_installation_variables` (src/network/ssh_installer/installer.rs) additionally runs `export LUKS_KEY='<pass>'`. Replace the echo-pipe with a root-only 0600 tempfile on the TARGET passed via `cryptsetup --key-file`, `shred -u`'d afterward, and delete the `LUKS_KEY` env export entirely.

**Reuse — do NOT invent a new secret-handling pattern.** The repo already has the exact tempfile pattern you must mirror: the Tang clevis-bind block in `SystemConfigurator::enroll_tang_clevis` (src/network/ssh_installer/system_setup.rs, tempfile `/run/.uaa-tang-enroll.key`). It does, in order: `install -m 0600 /dev/null <path>` (atomic 0600 create), `printf '%s' '<escaped-key>' > <path>` executed via `self.runner.execute(...)` directly — deliberately NOT via `log_and_execute`, so the secret is never logged — with single quotes escaped via `.replace('\'', r"'\''")`, then an always-run `shred -u <path> 2>/dev/null || rm -f <path>`. Copy that shape verbatim (re-locate it with the grep below). Keep using the existing `log_and_execute` helper for the cryptsetup commands themselves (they now carry only the keyfile PATH, not the key).

Spec: `docs/specs/installer-robustness-design.md` (decision: keyfile channel, mirror the Tang tempfile pattern) and `docs/specs/installer-robustness-plan.md` (this is its TASK-05 step).

## Background (verify before editing)

- The passphrase leaks through THREE channels today; this task closes the first two, the third is already mitigated:
  1. **cryptsetup command-line interpolation** — `echo '{}' | cryptsetup luksFormat --batch-mode ...` and `echo '{}' | cryptsetup open ... luks` in `setup_luks_encryption` (disk_ops.rs). The full passphrase is part of the command string handed to the shell: visible in `ps`, `/proc/<pid>/cmdline`, and any shell history/audit log on the target.
  2. **env export** — `("LUKS_KEY", config.luks_key.as_str())` in the `vars` array of `setup_installation_variables` (installer.rs) is run as `export LUKS_KEY='<pass>'`. Each `runner.execute` is a fresh shell, so the export is almost certainly inert (nothing downstream reads `$LUKS_KEY`; all consumers read `config.luks_key` directly, and the only `variables` map key read anywhere is `"DISK"` in zfs_ops.rs) — but it still puts the secret on a command line. Remove it anyway.
  3. **TPM2 first-boot seed heredoc** (system_setup.rs `setup_tpm2_firstboot_enrollment`) — ALREADY mitigated with the 0600+shred tempfile pattern; do not touch it.
- `src/security/luks.rs` also contains an `echo '{}' | cryptsetup luksFormat ...` — that is the SEPARATE legacy Path-A/deployer code path and is explicitly OUT OF SCOPE for this task. Do not edit it.
- **Wave/collision note:** this task runs in global wave 2. `disk_ops.rs` and `installer.rs` are also touched by installer-robustness/TASK-01 (partition-suffix helper, wave 1), which will likely have replaced the literal `{}p4` in these commands with a suffix-aware helper call by the time you run. Whatever partition-path *expression* you find in the two cryptsetup format! strings, KEEP IT AS-IS — this task changes only the passphrase channel, never the partition path. If the `'{}p4'` greps below return 0 hits but the `'cryptsetup luksFormat'` grep still hits, that is the expected post-TASK-01 state: proceed, anchoring on the `cryptsetup` greps.

- **Re-verify these anchors before editing** — line numbers drift, they are a starting point only:
  ```bash
  grep -n 'cryptsetup luksFormat' src/network/ssh_installer/disk_ops.rs
  # expect: 1 hit ~line 340 inside setup_luks_encryption (fn at ~333)
  grep -n 'cryptsetup open' src/network/ssh_installer/disk_ops.rs
  # expect: 1 hit ~line 348
  grep -n 'cryptsetup luksFormat --batch-mode {}p4\|cryptsetup open {}p4 luks' src/network/ssh_installer/disk_ops.rs
  # expect: 2 hits ~lines 340 and 348 (fn setup_luks_encryption at line 333) — 0 hits here + hits above = TASK-01 already replaced {}p4 with a helper; keep that helper expression
  grep -n '"LUKS_KEY"' src/network/ssh_installer/installer.rs
  # expect: 1 hit ~line 483; the actual `export {}='{}'` execute is found by grep -n "export {}='{}'" src/network/ssh_installer/installer.rs → 1 hit ~line 493
  grep -n 'uaa-tang-enroll.key' src/network/ssh_installer/system_setup.rs
  # expect: 1 hit ~line 660 (tmp_key_path in enroll_tang_clevis; clevis bind uses -k <path>) — this is the pattern to mirror
  grep -n 'cryptsetup luksFormat' src/security/luks.rs
  # expect: 1 hit ~line 37 — OUT OF SCOPE (legacy Path-A/deployer path); do NOT edit
  ```
  Zero hits on an anchor whose "expect" says ≥1 (other than the {}p4 case explained above) means STOP and report — do not guess.

- **HARD RULES (restated):**
  1. NEVER wipe/reimage/touch 172.16.2.30 ("the server") or len-serv-003. This task is code-only, validated by `cargo test`/`cargo build` and later in VM/QEMU — never on live servers.
  2. SECRETS: no real `luks_key`/`root_password`/`tpm2_pin` may appear anywhere in git — test fixtures use throwaway strings like `"k"` or `REPLACE_AT_PLACE_TIME`.
  3. Stay in your worktree; NEVER push/PR/merge — the coordinator owns all git.

## Step-by-step

1. Run every anchor grep above from the worktree root. Re-locate `setup_luks_encryption`, the `vars` array, and the Tang tempfile block by symbol, never by the line numbers in this brief.
2. In `src/network/ssh_installer/disk_ops.rs`, rewrite `setup_luks_encryption` to the keyfile flow. Use a fixed target-side path `const` such as `/run/.uaa-luks-setup.key` (`/run` is tmpfs — the key never touches persistent disk):
   1. Create the empty 0600 file: `install -m 0600 /dev/null /run/.uaa-luks-setup.key` (this one may go through `log_and_execute` — it contains no secret). On failure return `Err` (unlike Tang's non-fatal skip: an install cannot proceed without LUKS).
   2. Write the key with `self.runner.execute(...)` DIRECTLY — never `log_and_execute`, and never `echo` — using exactly the Tang escaping: `format!("printf '%s' '{}' > /run/.uaa-luks-setup.key", config.luks_key.replace('\'', r"'\''"))`. On failure: shred the tempfile (step 2.5 command), then return `Err`.
   3. Run luksFormat via `log_and_execute` with the passphrase replaced by the keyfile flag: `cryptsetup luksFormat --batch-mode --key-file /run/.uaa-luks-setup.key <PART>`, where `<PART>` is the exact partition expression already present in the current command (the `{}p4` format! today, or TASK-01's helper call if it landed). Do not `?`-propagate yet — capture the `Result`.
   4. Run open the same way: `cryptsetup open --key-file /run/.uaa-luks-setup.key <PART> luks` — capture the `Result`.
   5. ALWAYS shred, regardless of outcome (mirror Tang's finally-style block): `let _ = self.runner.execute("shred -u /run/.uaa-luks-setup.key 2>/dev/null || rm -f /run/.uaa-luks-setup.key").await;` — then propagate the first captured error (format error first, then open error), else `Ok(())`.
3. **Edge-case semantics (read twice, encode in tests):**
   - Key containing single quotes: handled by the `'\''` escaping in the printf write — same as Tang; a key like `a'b` must arrive in the file byte-for-byte.
   - Trailing newline: `printf '%s'` writes NO trailing newline, and it must stay that way. `cryptsetup --key-file` reads the WHOLE file verbatim (the old `echo` pipe added a `\n` that cryptsetup stripped as a line terminator). A trailing newline in the keyfile would silently enroll `<pass>\n` as the passphrase and break every other unlock channel (Tang clevis bind, TPM2 seed, interactive) which all use the trailing-newline-free convention.
   - Key containing embedded newlines: was never supported by the echo-pipe (stdin reads one line) and is not supported now; do NOT add validation for it in this task — just document the constraint in a code comment on the write step.
4. Extract the two command strings into pure builder fns so tests cannot diverge from production (mirror the existing `build_mkfs_esp`/`build_mkfs_reset` builder style in the same file, but make these two NON-`#[cfg(test)]` private fns since production calls them): e.g. `fn build_luks_format_cmd(part: &str, key_path: &str) -> String` and `fn build_luks_open_cmd(part: &str, key_path: &str) -> String`. Route the production calls in `setup_luks_encryption` through them.
5. Add `#[cfg(test)]` unit tests in disk_ops.rs's existing `mod tests`, e.g.:
   - `test_build_luks_format_cmd_uses_key_file` — output contains `--key-file /run/.uaa-luks-setup.key` and `luksFormat --batch-mode`, and does NOT contain `echo`.
   - `test_build_luks_open_cmd_uses_key_file` — output contains `--key-file` and ends with the mapper name `luks`, and does NOT contain `echo`.
   - `test_luks_commands_never_embed_passphrase` — build both with a sentinel partition; assert neither string contains a fake passphrase sentinel (the builders don't even take the key — assert their signatures stay key-free by construction and the strings contain no `'` -wrapped secret).
6. In `src/network/ssh_installer/installer.rs`, delete the `("LUKS_KEY", config.luks_key.as_str()),` tuple from the `vars` array in `setup_installation_variables`. That single deletion removes both the `export LUKS_KEY='…'` execute and the `variables` map insert (the loop handles both). Do NOT touch the other tuples — the `ROOT_PASSWORD` export is a known sibling issue and explicitly out of scope; do not refactor the loop, do not change signatures.
7. Confirm nothing referenced the removed export: `grep -rn 'LUKS_KEY' src/ --include='*.rs'` must return 0 hits after the edit.
8. Purely additive/surgical otherwise: no changes to partition-path logic, no changes to `wipe_disk`/`prepare_disk`, no reordering of phases, no edits to system_setup.rs or src/security/luks.rs.
9. Bump the file header (`// version:` +0.1.0 or per-file convention, `// last-edited: 2026-07-09`) on every touched file; keep existing guids.

## How to test

```bash
cargo test --lib --offline
# Expected: 240+ passed (baseline 237 + the new disk_ops builder tests); 0 failed
cargo build --offline
# Expected: exit 0
cargo clippy --offline
# Expected: no new warnings
```

## Acceptance criteria

- [ ] `grep -c 'key-file' src/network/ssh_installer/disk_ops.rs` ≥ 2 (both cryptsetup commands use the keyfile).
- [ ] `grep -n "echo '{}' | cryptsetup" src/network/ssh_installer/disk_ops.rs` returns 0 hits (echo-pipe gone from the installer path).
- [ ] `grep -rn 'LUKS_KEY' src/ --include='*.rs'` returns 0 hits (env export and all references removed).
- [ ] `grep -n 'cryptsetup luksFormat' src/security/luks.rs` still returns 1 hit — proof the out-of-scope legacy path was NOT touched.
- [ ] The printf key-write call site uses `self.runner.execute` directly, not `log_and_execute` (verify: `grep -n "printf '%s'" src/network/ssh_installer/disk_ops.rs` hits, and the enclosing statement is a `runner.execute` call — no `log_and_execute` on that line or the line above).
- [ ] `grep -c 'shred -u' src/network/ssh_installer/disk_ops.rs` ≥ 1 (always-shred cleanup present).
- [ ] `grep -n 'fn test_build_luks' src/network/ssh_installer/disk_ops.rs` returns ≥ 2 hits (new builder tests exist).
- [ ] Anti-over-suppression: the happy path still works — `test_build_luks_format_cmd_uses_key_file` / `test_build_luks_open_cmd_uses_key_file` prove the commands still target the same partition expression and mapper name `luks` (a normal passphrase install still succeeds end-to-end; only the secret's channel changed), and the full suite is green: `cargo test --lib --offline` reports 240+ passed / 0 failed.
- [ ] `cargo build --offline` and `cargo clippy --offline` clean.
- [ ] File headers bumped on every changed file (`git diff origin/main -- src/network/ssh_installer/disk_ops.rs src/network/ssh_installer/installer.rs | grep -c '^+// version:'` → 2 — both version lines touched IN THIS DIFF; a bare date grep is vacuous since both files already carry today's date at HEAD).

## Commit message

```
fix(security): pass LUKS passphrase via 0600 keyfile, drop echo-pipe and LUKS_KEY export

setup_luks_encryption now writes the passphrase to a root-only tmpfs tempfile
(printf '%s', no trailing newline, single quotes escaped) and passes
--key-file to cryptsetup luksFormat/open, shredding the file afterward —
mirroring the existing Tang clevis-bind tempfile pattern. The inert
export LUKS_KEY='...' in setup_installation_variables is removed. The secret
no longer appears in any command line, ps output, or /proc/<pid>/environ.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

Already-done check (transform polarity — new channel present AND old channels absent):
```bash
grep -c 'key-file' src/network/ssh_installer/disk_ops.rs        # ≥2 → new keyfile channel present
grep -n "echo '{}' | cryptsetup" src/network/ssh_installer/disk_ops.rs   # 0 hits → echo-pipe gone
grep -rn 'LUKS_KEY' src/ --include='*.rs'                       # 0 hits → env export gone
```
If all three hold, the task is already applied — run the acceptance checks instead of re-applying. Rollback: `git revert` the single commit restores the echo-pipe cryptsetup commands and the LUKS_KEY export (a known security regression, functionally identical install); no data, config schema, or sibling task is affected.
