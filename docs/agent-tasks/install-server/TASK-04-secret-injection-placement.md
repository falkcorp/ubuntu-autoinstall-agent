<!-- file: docs/agent-tasks/install-server/TASK-04-secret-injection-placement.md -->
<!-- version: 1.0.0 -->
<!-- guid: 9045f22b-4992-4951-a412-71bff3929f34 -->
<!-- last-edited: 2026-07-09 -->

# TASK-04 — deploy-usb-configs.sh --inject-from <secrets.yaml>: place-time secret injection server-locally (NO HTTP write API — locked decision) (todo:place-time-secrets)

**Priority:** P2 · **Effort:** M · **Recommended subagent:** Sonnet-class · shell-hardening subagent · **Why:** touches secret handling — must preserve the REPLACE_AT_PLACE_TIME refusal gate as the final backstop and never let a value reach logs, argv, or git · **Depends on:** none (wave 1, parallel-safe — touches only `scripts/deploy-usb-configs.sh`, disjoint from TASK-01)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/install-server-secret-injection-placement" -b agent/install-server-secret-injection-placement origin/main
cd "$REPO/.worktrees/install-server-secret-injection-placement"
git rebase origin/main
```

(Protocol is also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

## Goal

Add an optional `--inject-from <secrets.yaml>` flag to `scripts/deploy-usb-configs.sh` (design C4 / **LOCKED Decision 1** of `docs/specs/install-server-design.md`: placement stays a server-local, human-run script — `POST /api/place-config` and TLS+auth-on-:25000 were REJECTED, do not reopen). At place time, fill each host's `REPLACE_AT_PLACE_TIME` slots from a simple per-host-section YAML into a `mktemp` staging copy, then feed that copy through the EXISTING per-host placement path so every existing gate still fires — unknown-host refusal (`mac_for_host` case-lookup), missing-source refusal, and the `PLACEHOLDER="REPLACE_AT_PLACE_TIME"` grep gate, which becomes the backstop catching any slot the secrets file failed to fill. REUSE the existing loop, `mac_for_host`, `KNOWN_HOSTS`, `PLACEHOLDER`, and the `install -m 0644` placement line — do NOT write a second placement loop or a second placeholder constant. Without `--inject-from`, behavior is byte-identical to today.

## Background (verify before editing)

- The script places `<src>/<host>.yaml` → `/var/www/html/cloud-init/<hexmac>/uaa.yaml` and REFUSES (per-host, `fail=1`, final `exit 1`) any file still containing `REPLACE_AT_PLACE_TIME`, any unknown host, or a missing source. It deliberately stays bash-3.2-compatible (case-lookup, no `declare -A`) so it also runs from macOS — keep that.
- Committed per-host configs (`examples/configs/install/{len-serv-001,002,003,unimatrixone}.yaml`) each carry the placeholder on 3 secret keys (`luks_key`, `root_password`, `tpm2_pin`) **plus 1 comment line mentioning the token** (`grep -c` = 4). That comment would trip the backstop gate on an otherwise fully-injected staging copy, so injection must also drop comment lines containing the token (they document the placeholder scheme and are meaningless in a placed file).
- The Rust loader independently refuses placeholder-bearing configs (`src/cli/commands.rs`), so a leaked-through placeholder fails twice — keep both layers intact.
- **SECRETS DISCIPLINE (hard rule 4, restated):** no real `luks_key`/`root_password`/`tpm2_pin` may enter git — the committed examples stay placeholder-bearing and this task must not modify them. Secret values must never appear in: `echo`/log output, `set -x` traces (do NOT add `set -x` anywhere), argv of external commands (visible in `ps` — pass via file, never `sed "s/…/$SECRET/"`), or any file that survives exit (staging copies are `mktemp`-created 0600 and removed by an EXIT trap).
- **HARD RULES also in force:** never touch 172.16.2.30 or len-serv-003 — this task edits and tests the script locally with temp dirs only; running it against the real web root is a HUMAN action. Stay in your worktree; NEVER push/PR/merge — the coordinator owns all git.

**Re-verify these anchors** — line numbers drift, they are a starting point only. Zero hits = STOP and report:

```bash
grep -n 'PLACEHOLDER="REPLACE_AT_PLACE_TIME"' scripts/deploy-usb-configs.sh   # expect: 1 hit ~line 36
grep -c 'REPLACE_AT_PLACE_TIME' examples/configs/install/len-serv-003.yaml    # expect: count = 4
grep -n 'REPLACE_AT_PLACE_TIME placeholders' src/cli/commands.rs              # expect: 1 hit ~line 588
# reuse targets (cite-by-symbol):
grep -n 'mac_for_host()' scripts/deploy-usb-configs.sh                        # expect: 1 hit (the case-lookup fn)
grep -n 'install -m 0644' scripts/deploy-usb-configs.sh                       # expect: 1 hit (the placement line)
```

## Step-by-step

1. Run the anchor greps. The ONLY file you edit is `scripts/deploy-usb-configs.sh`. Do NOT touch `examples/configs/install/*.yaml`.
2. **Secrets-file format** (document it in the script's usage header comment):

   ```yaml
   # ~/uaa-secrets.yaml on the server — mode 0600, NEVER inside a git tree
   len-serv-003:
     luks_key: the-real-passphrase
     root_password: "the-real-password"
     tpm2_pin: "12345678"
   unimatrixone:
     luks_key: ...
   ```

   Top-level unindented `host:` section headers; indented `key: value` lines beneath. Values are copied VERBATIM after the `key: ` prefix (quotes included, so YAML typing like `"12345678"` survives into the placed file).
3. Flag parsing: in the existing `while [ $# -gt 0 ]` option loop, add a `--inject-from) SECRETS_FILE="$2"; shift 2 ;;` arm before the `-*)` catch-all, with `SECRETS_FILE=""` initialized above the loop (the script runs `set -u`). Update the usage comment block (`--inject-from <secrets.yaml>` line + format note + "keep it in ~/ on the server, mode 0600, outside any git tree").
4. Validation (only when `SECRETS_FILE` is non-empty), after option parsing, before the host loop — each failure is `echo "REFUSED: ..." >&2; exit 1` and none may print secret values:
   - Missing file: `[ -f "$SECRETS_FILE" ]` or refuse.
   - **Inside a git tree** (secrets must be un-committable): refuse when `[ "$(git -C "$(cd "$(dirname "$SECRETS_FILE")" && pwd)" rev-parse --is-inside-work-tree 2>/dev/null)" = "true" ]`. If `git` is absent the check passes (server-side use).
   - **Permissions**: group/other must have NO access (0600 or stricter). Portable (bash 3.2 / BSD+GNU): `perms="$(ls -ld "$SECRETS_FILE" | cut -c5-10)"; [ "$perms" = "------" ]` or refuse (world/group-readable secrets file is an error, not a warning).
5. Injection helper — a single `awk` program that reads the secrets file and the config as FILES (values never in argv), writing the filled copy to a path given as the 4th arg. Add above the host loop:

   ```bash
   # Fill REPLACE_AT_PLACE_TIME slots in config $2 from section $3 of secrets file $1,
   # writing to $4. Values never touch argv/logs; comment lines mentioning the token
   # are dropped (they document the placeholder scheme; the committed examples carry
   # one, which would otherwise trip the backstop gate on a fully-injected copy).
   inject_secrets() {
       awk -v host="$3" '
           NR == FNR {
               if ($0 ~ /^[A-Za-z0-9_-]+:[[:space:]]*$/) {
                   section = $0; sub(/:[[:space:]]*$/, "", section); next
               }
               if (section == host && $0 ~ /^[[:space:]]+[A-Za-z0-9_]+:[[:space:]]*[^[:space:]]/) {
                   key = $1; sub(/:$/, "", key)
                   val = $0; sub(/^[[:space:]]*[A-Za-z0-9_]+:[[:space:]]*/, "", val)
                   secret[key] = val
               }
               next
           }
           /REPLACE_AT_PLACE_TIME/ {
               if ($0 ~ /^[[:space:]]*#/) next
               line_key = $1; sub(/:$/, "", line_key)
               if (line_key in secret) {
                   indent = $0; sub(/[^[:space:]].*$/, "", indent)
                   print indent line_key ": " secret[line_key]
                   next
               }
               print; next
           }
           { print }
       ' "$1" "$2" > "$4"
   }
   ```

   Edge semantics (do not guess): a key present in the config but MISSING from the host's secrets section is left as `REPLACE_AT_PLACE_TIME` — the existing gate then refuses that host (per-host `fail=1`, others still process); a host with NO section in the secrets file likewise falls through to the gate; extra keys in the secrets file that match no placeholder line are silently unused; the replacement uses exact string concatenation (`key ": " value`), never regex substitution on the value side, so `&`, `\`, `/` in secrets are safe.
6. Staging + cleanup: initialize `TMPFILES=""` and `trap 'rm -f $TMPFILES' EXIT` near the top (before the loop). Inside the existing per-host loop, AFTER the missing-source check and BEFORE the `grep -q "$PLACEHOLDER" "$src"` gate, insert:

   ```bash
       if [ -n "$SECRETS_FILE" ]; then
           staged="$(mktemp)"           # mktemp creates 0600 — no umask games needed
           TMPFILES="$TMPFILES $staged"
           inject_secrets "$SECRETS_FILE" "$src" "$host" "$staged"
           src="$staged"
       fi
   ```

   The UNCHANGED existing lines then run against the staged copy: the placeholder grep gate (now the backstop) and `install -m 0644 "$src" "${dest_dir}/uaa.yaml"`. Do not reorder, duplicate, or weaken any existing check; do not change `mkdir -p`/`install` modes.
7. Purely additive scope check: with `SECRETS_FILE` empty, every new code path is skipped and the script's behavior is byte-identical to today (same refusals, same messages, same exit codes).
8. Bump the file header: `# version: 1.0.0` → `1.1.0` (or minor-bump from current post-rebase value), `# last-edited: 2026-07-09`, guid unchanged. Update the usage/comment header for the new flag.
9. Record (do NOT execute — HUMAN step) the deploy/use note in the header comment: copy the updated script to the server (`scp scripts/deploy-usb-configs.sh 172.16.2.30:~/`), keep `~/uaa-secrets.yaml` there at mode 0600, run it ON the server; no service restart is involved. (For the py-mirror service the standing note applies: `scp scripts/autoinstall-agent.py 172.16.2.30:/var/www/html/cloud-init/scripts/autoinstall-agent.py && ssh 172.16.2.30 'sudo systemctl restart autoinstall-agent'` — this task does not edit that file.)

## How to test

All local, temp-dirs only — never against a real web root. Test values are obvious dummies, never committed anywhere.

```bash
bash -n scripts/deploy-usb-configs.sh
# Expected: exit 0

python3 -m py_compile scripts/autoinstall-agent.py
# Expected: exit 0 (untouched mirror still parses)

T="$(mktemp -d)"; mkdir -p "$T/src"
cp examples/configs/install/len-serv-003.yaml "$T/src/"
printf 'len-serv-003:\n  luks_key: test-luks-0000\n  root_password: "test-root-0000"\n  tpm2_pin: "12345678"\n' > "$T/secrets.yaml"
chmod 600 "$T/secrets.yaml"

# 1) happy path: fully-injected copy PASSES the gate and places (anti-over-suppression)
scripts/deploy-usb-configs.sh --src "$T/src" --dest "$T/dest" --inject-from "$T/secrets.yaml" len-serv-003
# Expected: "PLACED  len-serv-003 (6c:4b:90:bc:f7:f4) -> $T/dest/6c4b90bcf7f4/uaa.yaml"; exit 0
! grep -q 'REPLACE_AT_PLACE_TIME' "$T/dest/6c4b90bcf7f4/uaa.yaml" && echo no-placeholders-ok
# Expected: no-placeholders-ok
grep -q 'tpm2_pin: "12345678"' "$T/dest/6c4b90bcf7f4/uaa.yaml" && echo quotes-preserved-ok
# Expected: quotes-preserved-ok

# 2) half-injected -> backstop gate refuses (per-host exit 1)
printf 'len-serv-003:\n  luks_key: test-luks-0000\n  root_password: test-root-0000\n' > "$T/partial.yaml"
chmod 600 "$T/partial.yaml"
scripts/deploy-usb-configs.sh --src "$T/src" --dest "$T/dest2" --inject-from "$T/partial.yaml" len-serv-003 \
  && echo "FAIL: should have refused" || echo refused-ok
# Expected: "REFUSED len-serv-003: ... REPLACE_AT_PLACE_TIME ..." on stderr; refused-ok

# 3) world/group-readable secrets file refused
chmod 644 "$T/secrets.yaml"
scripts/deploy-usb-configs.sh --src "$T/src" --dest "$T/dest3" --inject-from "$T/secrets.yaml" len-serv-003 \
  && echo "FAIL: should have refused" || echo perms-refused-ok
chmod 600 "$T/secrets.yaml"
# Expected: perms-refused-ok

# 4) secrets file inside the repo tree refused
cp "$T/secrets.yaml" ./insecure-secrets.yaml && chmod 600 ./insecure-secrets.yaml
scripts/deploy-usb-configs.sh --src "$T/src" --dest "$T/dest4" --inject-from ./insecure-secrets.yaml len-serv-003 \
  && echo "FAIL: should have refused" || echo git-refused-ok
rm -f ./insecure-secrets.yaml
# Expected: git-refused-ok

# 5) no flag -> byte-identical legacy behavior (placeholder config refused as today)
scripts/deploy-usb-configs.sh --src examples/configs/install --dest "$T/dest5" len-serv-003 \
  && echo "FAIL: should have refused" || echo legacy-refuse-ok
# Expected: legacy-refuse-ok

# 6) no secret value leaked into any output
scripts/deploy-usb-configs.sh --src "$T/src" --dest "$T/dest6" --inject-from "$T/secrets.yaml" len-serv-003 2>&1 | grep -q 'test-luks-0000' \
  && echo "FAIL: secret leaked to output" || echo no-leak-ok
# Expected: no-leak-ok
rm -rf "$T"

cargo test --lib --offline
# Expected: 237+ passed; 0 failed (untouched Rust stays green)
cargo build --offline
# Expected: exit 0
```

## Acceptance criteria

- [ ] Flag present: `grep -n 'inject-from' scripts/deploy-usb-configs.sh` → ≥2 hits (usage comment + parsing arm); `grep -n 'inject_secrets()' scripts/deploy-usb-configs.sh` → 1 hit.
- [ ] Refusal gate preserved verbatim: `grep -n 'PLACEHOLDER="REPLACE_AT_PLACE_TIME"' scripts/deploy-usb-configs.sh` → 1 hit and `grep -n 'grep -q "\$PLACEHOLDER" "\$src"' scripts/deploy-usb-configs.sh` → 1 hit (gate runs on `$src`, i.e. the staged copy when injecting).
- [ ] Anti-over-suppression: functional test 1 places a fully-injected config (`PLACED` + `no-placeholders-ok` + `quotes-preserved-ok`) — the gates must not refuse the happy path.
- [ ] Backstop + refusals: functional tests 2–5 print `refused-ok`, `perms-refused-ok`, `git-refused-ok`, `legacy-refuse-ok` respectively.
- [ ] No leak: functional test 6 prints `no-leak-ok`; `grep -n 'set -x' scripts/deploy-usb-configs.sh` → 0 hits; `grep -rn 'test-luks-0000\|test-root-0000' --exclude-dir=.git .` → 0 hits (no dummy secret committed).
- [ ] Committed examples untouched: `grep -c 'REPLACE_AT_PLACE_TIME' examples/configs/install/len-serv-003.yaml` → 4 and `git diff --name-only origin/main` lists exactly `scripts/deploy-usb-configs.sh`.
- [ ] Temp hygiene: `grep -n "trap 'rm -f \$TMPFILES' EXIT" scripts/deploy-usb-configs.sh` → 1 hit; staging via `mktemp` (0600 by default).
- [ ] Tests green: `bash -n scripts/deploy-usb-configs.sh` exits 0; `python3 -m py_compile scripts/autoinstall-agent.py` exits 0; `cargo test --lib --offline` shows 237+ passed, 0 failed; `cargo build --offline` exits 0.
- [ ] File header actually bumped BY THIS TASK (a date grep is vacuous — the script already carries today's date at HEAD): `git diff origin/main -- scripts/deploy-usb-configs.sh | grep -c '^+# version:'` → 1 AND `git diff origin/main -- scripts/deploy-usb-configs.sh | grep -c '^[+-]# guid:'` → 0.

## Commit message

```
feat(install-server): deploy-usb-configs.sh --inject-from place-time secret injection

Optional server-local flag (LOCKED decision: NO HTTP secret-write API) that fills
REPLACE_AT_PLACE_TIME slots from a per-host secrets.yaml into a mktemp staging copy
(0600, trap-cleaned) and feeds it through the existing placement path — the
placeholder refusal gate stays as the backstop for unfilled slots. Refuses secrets
files that are group/world-readable or inside a git tree; values never reach argv,
logs, or git. Without the flag, behavior is byte-identical.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

Idempotency (additive): `grep -n 'inject_secrets()' scripts/deploy-usb-configs.sh` — if it hits, the flag is already implemented; run the acceptance checks instead of re-applying. Rollback: `git revert` the single commit removes the flag and helper; placement returns to today's manual staging-copy workflow, no placed `uaa.yaml` on any server is retroactively affected, siblings unaffected.
