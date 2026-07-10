<!-- file: docs/agent-tasks/tooling-port/TASK-02-config-place-inject.md -->
<!-- version: 1.0.0 -->
<!-- guid: 8649a954-4d23-4b3e-b0bf-bacebb80072a -->
<!-- last-edited: 2026-07-10 -->

# TASK-02 — `uaa config place/inject`: port deploy-usb-configs.sh incl. --inject-from (0600 staging, git-tree refusal, perms check, placeholder hard-gate) (ws9-tooling)

**Priority:** P1 · **Effort:** M · **Recommended subagent:** Opus-class · secrets-handling rust subagent · **Why:** ⚠ real-secret handling; every guard is load-bearing (secrets never in argv/logs/git) · **Depends on:** CP-01 (wave-2 gated: `core-proto/TASK-01` workspace conversion MERGED and this worktree rebased — the stub file this task fills does not exist before then)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/tooling-port-config-place-inject" -b agent/tooling-port-config-place-inject origin/main
cd "$REPO/.worktrees/tooling-port-config-place-inject"
git rebase origin/main
```

(Protocol also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

## Goal

Fill the CP-01-created stub `crates/uaa-core/src/config_place.rs` with a faithful Rust port of `scripts/deploy-usb-configs.sh` (v1.1.0, 193 lines) — the SERVER-LOCAL placement of per-host `InstallationConfig` files into `<dest>/<hexmac>/uaa.yaml`, including `--inject-from` place-time secret injection — and replace the `todo!()` of the pre-wired `uaa config place` CLI dispatch (CP-01, `crates/uaa/src/`). EVERY guard in the script is load-bearing and MUST be ported (spec Non-goals: "No secret ever transits HTTP — `REPLACE_AT_PLACE_TIME` placeholders stay; injection is server-local only"). The shell script stays UNCHANGED — TASK-05 deletes it after the M6 gate (spec Decision 16 / Goals). Purely additive.

REUSE — do not invent parallels:

- **`InstallationConfig` deny_unknown_fields** (`src/network/ssh_installer/config.rs` — verify: `grep -n "deny_unknown_fields" src/network/ssh_installer/config.rs`) is the schema the placed YAML must parse into; parse the FULLY-INJECTED staged copy with it as a final structural gate (a typo'd key must fail loudly). Do NOT write a new config struct.
- **`AutoInstallError::ConfigError` / `AutoInstallError::SystemError`** (`src/error.rs`). No new error enum.
- `tempfile::NamedTempFile` for the staging copy — on Unix it is created 0600, exactly mirroring the script's `mktemp`; do NOT hand-roll temp files or loosen the mode.
- This module is pure `std::fs` + in-memory string work: NO `CommandExecutor`, NO network, NO ssh/scp path may exist in it (server-local only, by design).

## Background (verify before editing)

Ground truth is `scripts/deploy-usb-configs.sh`. Port ALL of these semantics:

- **Host registry** (`mac_for_host`): `len-serv-001 → 6c:4b:90:bc:39:b3`, `len-serv-002 → 6c:4b:90:bc:f8:a3`, `len-serv-003 → 6c:4b:90:bc:f7:f4`, `unimatrixone → ac:1f:6b:40:fc:e2`. `hexmac` = MAC with colons stripped. Unknown host → per-host REFUSED (not a global abort). Default host set = all four.
- **Defaults:** src dir `examples/configs/install` (repo-relative), dest base `/var/www/html/cloud-init`, placeholder literal `REPLACE_AT_PLACE_TIME`.
- **Secrets-file guards** (when `--inject-from` given; ALL fail the whole run with exit 1 BEFORE any host is processed):
  1. file must exist;
  2. REFUSE a secrets file inside ANY git work tree (`git -C <its dir> rev-parse --is-inside-work-tree` == `true`; if git itself is unavailable, PASS — server-side use);
  3. REFUSE unless mode is 0600 or stricter — group/other must have NO permission bits (read via `std::os::unix::fs::PermissionsExt`, `mode & 0o077 == 0`; do not shell out to `ls`).
- **Injection semantics** (the awk block, ported to a pure function; values NEVER touch argv, logs, error messages, or panic text):
  - secrets format: unindented top-level `host:` section headers; indented `key: value` lines beneath; the value is everything after `key: ` copied VERBATIM (quotes included);
  - in the config, only lines containing `REPLACE_AT_PLACE_TIME` are candidates: a COMMENT line (`^\s*#`) containing the token is DROPPED (the committed examples carry one that would otherwise trip the backstop gate on a fully-injected copy); a `key: REPLACE_AT_PLACE_TIME` line whose key exists in the host's secrets section is rewritten as `<original indent><key>: <value>`; a placeholder line with NO matching secret passes through unchanged — the hard gate below then refuses that host;
  - injection writes to a 0600 staging copy; the ORIGINAL src file is never modified.
- **Per-host placement loop** (each failure = REFUSED for that host, `continue`, final exit 1; successes still place):
  unknown host → REFUSED; `<src>/<host>.yaml` missing → REFUSED; staged (or raw `--src`) copy still containing `REPLACE_AT_PLACE_TIME` → REFUSED (**the hard gate**: a secretless config must never be servable to a booting installer); otherwise `mkdir -p <dest>/<hexmac>` and install the file as `<dest>/<hexmac>/uaa.yaml` mode 0644. Exit status: 0 iff every requested host placed.
- **Server-local only:** the script has no remote path; the port must actively REFUSE any implied remote target — a `--dest` that looks remote (`host:` scp syntax, `ssh://`, or containing `://`) is a `ConfigError` ("place-time injection is server-local only; there is NO HTTP secret-write API, by design").
- Staging copies are cleaned up on every exit path (NamedTempFile drop covers this — do not `keep()` them).

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

Additional secret rules for THIS task: test secrets are obviously fake (`"test-passphrase"`); no `Debug`/`Display` impl on the parsed-secrets type may print values (implement a manual `Debug` printing `<redacted>`); no `tracing`/`println!` of any injected line.

**Path map:** after CP-01 (wave 1) merges, `src/**` lives at
`crates/uaa-core/src/**` and the CLI at `crates/uaa/src/**`. The greps above cite
pre-move paths (verifiable on today's main); at execution time run them at the old
path, then the mapped path. Zero hits at BOTH = STOP and report.

- **Re-verify these anchors before editing** — line numbers drift; zero hits at both
  old and mapped path = STOP and report:
  ```bash
  grep -n "0600\|work tree\|git" scripts/deploy-usb-configs.sh | head -6
  # expect: hits (0600 + git-tree refusals, ~lines 40-61 and 113-127)
  grep -n "REPLACE_AT_PLACE_TIME" scripts/deploy-usb-configs.sh
  # expect: 1+ hits (placeholder hard gate, ~lines 74, 146, 181)
  grep -n "mac_for_host" scripts/deploy-usb-configs.sh
  # expect: hits (host→MAC registry, ~line 85)
  grep -n "deny_unknown_fields" src/network/ssh_installer/config.rs
  # expect: 1+ hits (then at crates/uaa-core/src/network/ssh_installer/config.rs)
  ls examples/configs/install/ | head
  # expect: per-host yaml files with placeholders (default --src contents)
  ```
- Execution-time checks (crates/ exists only after CP-01):
  ```bash
  test -f crates/uaa-core/src/config_place.rs && grep -n "todo!" crates/uaa-core/src/config_place.rs
  # expect: headered stub exists; absent = STOP, CP-01 not merged
  grep -rn "config" crates/uaa/src/ | grep -i "place\|todo" | head -5
  # expect: the pre-wired `uaa config place` variant + todo!() arm to replace
  ```

## Step-by-step

1. Run the ⛔ START HERE block, then every anchor grep above. Any zero-hit result at both paths → STOP and report.
2. In `crates/uaa-core/src/config_place.rs` add the registry + pure helpers: `pub fn mac_for_host(host: &str) -> Option<&'static str>` (the four MACs above), `pub fn hexmac(mac: &str) -> String`, `pub const PLACEHOLDER: &str = "REPLACE_AT_PLACE_TIME";`, `pub const KNOWN_HOSTS: [&str; 4]`.
3. Add `pub struct SecretsFile(...)` with `pub fn parse(text: &str) -> SecretsFile` implementing the section/key/verbatim-value format, a manual `Debug` that redacts values, and `pub fn check_secrets_file(path: &Path) -> Result<()>` running the three guards IN ORDER (exists → git-work-tree refusal → `mode & 0o077 == 0`). The git probe runs `git -C <dir> rev-parse --is-inside-work-tree` via `std::process::Command` — the ONE allowed process spawn in this module, argv contains only the directory path, never secret material; a git spawn error (git absent) PASSES the guard.
4. Add `pub fn inject_secrets(secrets: &SecretsFile, host: &str, config_text: &str) -> String` — exact awk semantics from Background (comment-drop, indent-preserving rewrite, pass-through on missing key). Pure function, no IO.
5. Add the driver `pub fn place_configs(opts: &PlaceOptions) -> Result<PlaceReport>` with `PlaceOptions { src_dir, dest_base, inject_from: Option<PathBuf>, hosts: Vec<String> }`:
   - remote-dest refusal FIRST (`ConfigError` if dest contains `://` or matches `^[^/]+:` scp syntax);
   - secrets-file guards next (whole-run abort);
   - then the per-host loop exactly as Background: stage into a `NamedTempFile` (0600) when injecting; HARD GATE `staged.contains(PLACEHOLDER)` → refused; parse the staged copy as `InstallationConfig` (deny_unknown_fields) → parse failure = refused; `fs::create_dir_all` + copy to `<dest>/<hexmac>/uaa.yaml` + `set_permissions(0o644)`;
   - `PlaceReport { placed: Vec<String>, refused: Vec<(String, String /*reason — NEVER contains a secret value*/)> }`; overall `Ok` even with refusals — the CLI maps non-empty `refused` to exit 1.
6. Replace the `todo!()` in the pre-wired `uaa config place` CLI module in `crates/uaa/src/`: flags `--src`, `--dest`, `--inject-from`, positional hosts; print `PLACED <host> (<mac>) -> <path>` / `REFUSED <host>: <reason>` lines mirroring the script; `std::process::exit(1)` on any refusal.
7. Unit tests (tempdir-based; fake secrets only):

   | Test | Asserts |
   |---|---|
   | `test_mac_registry_and_hexmac` | all four hosts map; `hexmac("6c:4b:90:bc:39:b3") == "6c4b90bc39b3"`; unknown → `None` |
   | `test_secrets_perms_guard` | tempfile chmod 0644 → `Err`; 0600 → `Ok`; 0400 → `Ok` |
   | `test_secrets_git_tree_refusal` | a secrets file created inside the repo work tree → `Err` mentioning "git work tree" |
   | `test_inject_verbatim_and_comment_drop` | `luks_key: REPLACE_AT_PLACE_TIME` (2-space indent) becomes `  luks_key: "the value"` (quotes verbatim); a `# ... REPLACE_AT_PLACE_TIME ...` comment line is dropped; an un-matched placeholder line survives unchanged |
   | `test_place_refuses_leftover_placeholder` | config with an uninjected placeholder → host in `refused`, no file written under dest |
   | `test_place_refuses_unknown_host_and_missing_src` | both produce `refused` entries; other hosts in the same run still place |
   | `test_place_refuses_remote_dest` | dest `"172.16.2.30:/var/www"` and `"ssh://x/y"` → `Err` before any host processing |
   | `test_refusal_reasons_never_leak_values` | run with a fake secret `"sekrit-123"`; assert no `refused` reason, and no `format!("{:?}", secrets)` output, contains `sekrit-123` |
   | `test_place_happy_path_end_to_end` | **anti-over-suppression:** placeholder config + valid 0600 out-of-tree secrets → file exists at `<dest>/<hexmac>/uaa.yaml`, mode 0644, contains the injected value, contains NO `REPLACE_AT_PLACE_TIME`, parses as `InstallationConfig`, exit-status view is success |

8. Bump the file header (version + last-edited) on every file you touch; new files get a fresh 4-line header with a new uuid4 (`uuidgen | tr 'A-F' 'a-f'`).

## How to test

```bash
cargo test --lib --offline && cargo build --offline
# Expected: all tests pass (baseline 311 + your new tests), build clean
cargo clippy --offline -- -D warnings
# Expected: no warnings
cargo test --lib --offline config_place
# Expected: the 9 tests above all pass
grep -rn "println!\|tracing::\|log::" crates/uaa-core/src/config_place.rs | grep -iv test
# Expected: 0 hits carrying secret-bearing variables (manual check: no injected line is ever logged)
grep -rn "CommandExecutor\|SshClient" crates/uaa-core/src/config_place.rs
# Expected: 0 hits (server-local, fs-only module)
git diff origin/main -- scripts/deploy-usb-configs.sh
# Expected: empty (script untouched — TASK-05 owns its deletion)
```

## Acceptance criteria

- [ ] All guards ported and individually tested: `grep -n "fn test_secrets_perms_guard\|fn test_secrets_git_tree_refusal\|fn test_place_refuses_leftover_placeholder\|fn test_place_refuses_remote_dest" crates/uaa-core/src/config_place.rs` → 4 hits, all passing.
- [ ] Placeholder hard gate present: `grep -n "REPLACE_AT_PLACE_TIME" crates/uaa-core/src/config_place.rs` → ≥2 hits (const + gate), and the gate fires on the STAGED copy (post-injection).
- [ ] No secret leakage path: `test_refusal_reasons_never_leak_values` passes; `SecretsFile`'s `Debug` prints `<redacted>` (`grep -n "redacted" crates/uaa-core/src/config_place.rs` → ≥1 hit).
- [ ] Comment-drop semantics ported (`test_inject_verbatim_and_comment_drop` passes — the committed example's placeholder comment cannot trip the gate on an injected copy).
- [ ] **Anti-over-suppression:** `test_place_happy_path_end_to_end` passes — a legitimately injected config passes ALL guards and is placed 0644 at `<dest>/<hexmac>/uaa.yaml`.
- [ ] `grep -n "todo!" crates/uaa-core/src/config_place.rs` → 0 hits; the `config place` CLI arm no longer contains `todo!`.
- [ ] `scripts/deploy-usb-configs.sh` unchanged: `git diff origin/main -- scripts/deploy-usb-configs.sh` empty; no real secret anywhere in the diff (`git diff origin/main` reviewed).
- [ ] `cargo test --lib --offline && cargo build --offline` exits 0; clippy clean.
- [ ] File headers bumped on every changed file (`git diff origin/main --stat` files each show a version bump; guids unchanged).

## Commit message

```
feat(config): port deploy-usb-configs.sh to uaa config place with --inject-from (ws9-tooling)

Fills crates/uaa-core/src/config_place.rs: host→MAC registry, server-local
placement to <dest>/<hexmac>/uaa.yaml (0644), and place-time secret injection
with every shell guard ported — secrets file must be 0600-or-stricter and
outside any git work tree; staging copies are 0600 NamedTempFiles; awk-style
in-memory injection (verbatim values, comment-drop) so secret values never
touch argv or logs; REPLACE_AT_PLACE_TIME hard gate on the staged copy;
remote-dest refusal (injection is server-local only, no HTTP secret-write
API); staged copy re-parsed as InstallationConfig (deny_unknown_fields).
9 tempdir unit tests incl. a no-value-leak test and an end-to-end happy path.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

If `grep -n "pub fn place_configs" crates/uaa-core/src/config_place.rs` hits (and `grep -n "todo!"` in the same file shows 0), already applied — run the acceptance checks instead of re-applying. Rollback = revert the single commit; `scripts/deploy-usb-configs.sh`, `examples/configs/install/*` (placeholders intact), and the server's `/var/www/html/cloud-init` (never touched by this task — code only) stay untouched.
