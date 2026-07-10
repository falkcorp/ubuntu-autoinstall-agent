<!-- file: docs/agent-tasks/installer-robustness/TASK-06-config-schema-hardening.md -->
<!-- version: 1.0.0 -->
<!-- guid: 1bab2b0d-f9fb-4fe1-9cb4-8210b8ee53fc -->
<!-- last-edited: 2026-07-09 -->

# TASK-06 — serde deny_unknown_fields + YAML round-trip tests for InstallationConfig (todo:config-schema)

**Priority:** P3 · **Effort:** S · **Recommended subagent:** Haiku-class · rust-tests subagent · **Why:** mechanical serde attribute + tests; struct already matches YAML 1:1 (scout-verified) · **Depends on:** none

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/installer-robustness-config-schema-hardening" -b agent/installer-robustness-config-schema-hardening origin/main
cd "$REPO/.worktrees/installer-robustness-config-schema-hardening"
git rebase origin/main
```

(Protocol is also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

## Goal

Make misspelled YAML keys fail loudly instead of being silently dropped: add `#[serde(deny_unknown_fields)]` to `InstallationConfig` in `src/network/ssh_installer/config.rs`, prove the four committed example configs still parse, and add a negative test that an unknown key is rejected.

**Reuse — do NOT build a new YAML test harness.** config.rs already contains the round-trip machinery: `test_install_example_configs_round_trip` (loads all four `examples/configs/install/*.yaml` files via the existing `InstallationConfig::from_yaml_file` and asserts every field) and `test_multikey_serde_defaults_when_absent` (minimal YAML relying on serde defaults). Extend that existing `mod tests`; do not add new loader functions or new fixture files.

Spec: `docs/specs/installer-robustness-design.md` (decision: fail-loud config schema) and `docs/specs/installer-robustness-plan.md` (this is its TASK-06 step).

## Background (verify before editing)

- `InstallationConfig` (config.rs) is a plain serde struct with NO `deny_unknown_fields`, so a typo like `disk_devise:` in a placed `uaa.yaml` is silently ignored and the field silently takes... nothing — for required fields YAML parsing fails, but for optional/defaulted fields (`tang_servers`, `enroll_tpm2`, `tpm2_pin`, …) the typo silently yields the default. On an installer that LUKS-formats disks, silent config drops are unacceptable.
- The evidence scout verified the struct's fields match `examples/configs/install/unimatrixone.yaml` keys 1:1 (20 fields, no extras either way), so enabling the attribute should NOT break any committed config. The mechanical proof is simply running the existing round-trip test after adding the attribute.
- The example configs intentionally carry `REPLACE_AT_PLACE_TIME` placeholders for `luks_key`/`root_password`/`tpm2_pin` — that is fine for parsing tests and MUST stay that way.
- **Collision note:** `config.rs` is also touched by installer-robustness/TASK-04 (netplan renderer field, wave 2). This task is wave 1 and lands first; keep the diff minimal (one attribute line + tests) so TASK-04 rebases cleanly. Note for the future: once TASK-04 adds its new config field, `deny_unknown_fields` is exactly what will force its YAML key and struct field to stay in sync.

- **Re-verify these anchors before editing** — line numbers drift, they are a starting point only:
  ```bash
  grep -n 'deny_unknown_fields' src/network/ssh_installer/config.rs
  # expect: 0 hits (grep exits 1); field list via grep -n 'pub [a-z_]*:' src/network/ssh_installer/config.rs (InstallationConfig block lines 48-95)
  grep -c 'REPLACE_AT_PLACE_TIME' examples/configs/install/len-serv-003.yaml
  # expect: count = 4
  grep -n 'fn test_install_example_configs_round_trip' src/network/ssh_installer/config.rs   # expect: 1 hit — the existing round-trip test to lean on
  grep -n 'fn test_multikey_serde_defaults_when_absent' src/network/ssh_installer/config.rs   # expect: 1 hit — proves defaults still work with the attribute
  grep -n 'pub struct InstallationConfig' src/network/ssh_installer/config.rs                 # expect: 1 hit — the attribute goes on this struct only
  ```
  Zero hits on an anchor whose "expect" says ≥1 means STOP and report — do not guess. (The first anchor EXPECTS 0 hits; if it already hits, see Idempotency below.)

- **HARD RULES (restated):**
  1. NEVER wipe/reimage/touch 172.16.2.30 ("the server") or len-serv-003. This task is code/tests only.
  2. SECRETS: committed configs carry `REPLACE_AT_PLACE_TIME` — never replace them with real values, never introduce a real `luks_key`/`root_password`/`tpm2_pin` anywhere in git. Test YAML literals use throwaway strings like `"k"`/`"p"` (the existing minimal-YAML test already does).
  3. Stay in your worktree; NEVER push/PR/merge — the coordinator owns all git.

## Step-by-step

1. Run the anchor greps above from the worktree root.
2. In `src/network/ssh_installer/config.rs`, add one line between the derive and `pub struct InstallationConfig`:
   ```rust
   #[serde(deny_unknown_fields)]
   ```
   The attribute affects Deserialize only (the struct also derives Serialize — that is unaffected). Do NOT add the attribute to `TangServer`, `InitramfsType`, or any other type — this task is InstallationConfig only.
3. Run `cargo test --lib --offline`. Expected: all pass, in particular `test_install_example_configs_round_trip` (all four example YAMLs still parse) and `test_multikey_serde_defaults_when_absent` (a YAML with OMITTED optional fields still parses — `deny_unknown_fields` rejects EXTRA keys, it does not require missing ones; this distinction is the load-bearing semantic of the task). If the round-trip test fails naming an unknown key in any example YAML, STOP and report the exact key and file — do not silently delete YAML keys or weaken the attribute.
4. Add ONE negative test to the existing `mod tests` in config.rs (copy the minimal-YAML shape from `test_multikey_serde_defaults_when_absent` and add a single typo'd key):
   ```rust
   #[test]
   fn test_unknown_yaml_key_rejected() {
       // deny_unknown_fields: a typo'd key must fail parsing loudly, not be
       // silently dropped (this installer LUKS-formats disks off this config).
       let yaml = r#"
   hostname: test
   disk_devise: /dev/sda
   disk_device: /dev/sda
   timezone: UTC
   luks_key: k
   root_password: p
   network_interface: eth0
   network_address: 10.0.0.2/24
   network_gateway: 10.0.0.1
   network_search: local
   network_nameservers: ["10.0.0.1"]
   "#;
       let err = serde_yaml::from_str::<InstallationConfig>(yaml).unwrap_err();
       assert!(err.to_string().contains("disk_devise"), "error must name the unknown key: {err}");
   }
   ```
5. Purely additive: do not modify any existing field, default fn, test, or the `for_len_serv_003()` constructor; do not touch the example YAML files.
6. Bump the file header in config.rs: `// version: 2.3.1` -> `// version: 2.4.0` (if the version
   at dispatch time is no longer 2.3.1, bump the minor of whatever you find); set
   `// last-edited:` to the dispatch date; keep the guid unchanged.

## How to test

```bash
cargo test --lib --offline
# Expected: 238+ passed (baseline 237 + test_unknown_yaml_key_rejected); 0 failed
cargo build --offline
# Expected: exit 0
cargo clippy --offline
# Expected: no new warnings
```

## Acceptance criteria

- [ ] `grep -c 'deny_unknown_fields' src/network/ssh_installer/config.rs` returns 1 (on InstallationConfig only).
- [ ] `grep -n 'fn test_unknown_yaml_key_rejected' src/network/ssh_installer/config.rs` returns 1 hit and the test passes.
- [ ] Anti-over-suppression: the guard does not reject valid input — `test_install_example_configs_round_trip` (all 4 committed example configs still parse with every field asserted) AND `test_multikey_serde_defaults_when_absent` (optional fields may still be OMITTED and default correctly) both still pass unmodified.
- [ ] `grep -rn 'REPLACE_AT_PLACE_TIME' examples/configs/install/ | wc -l` unchanged from before the task (no placeholder was touched; no real secret introduced).
- [ ] Tests green: `cargo test --lib --offline` reports 238+ passed / 0 failed; `cargo build --offline` and `cargo clippy --offline` clean.
- [ ] File header version actually bumped in config.rs: `grep -c '// version: 2.3.1' src/network/ssh_installer/config.rs` returns 0 AND `grep -n '// version: 2.4' src/network/ssh_installer/config.rs` returns 1 hit (a still-matching pre-task version means the bump was skipped).

## Commit message

```
fix(config): deny unknown YAML keys in InstallationConfig + negative parse test

Adds #[serde(deny_unknown_fields)] so typo'd keys in placed uaa.yaml configs
fail loudly at load instead of silently defaulting, with a regression test
that a misspelled key is rejected by name. All four committed example
configs still round-trip (existing tests unchanged).

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

Already-done check (additive polarity — presence of the new attribute + test):
```bash
grep -n 'deny_unknown_fields' src/network/ssh_installer/config.rs      # ≥1 hit → attribute already applied
grep -n 'fn test_unknown_yaml_key_rejected' src/network/ssh_installer/config.rs   # 1 hit → negative test already present
```
If both hit, the task is already applied — run the acceptance checks instead of re-applying. Rollback: `git revert` the single commit removes the attribute and the test; parsing returns to silently-dropping unknown keys, no data or sibling task affected.
