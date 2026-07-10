<!-- file: docs/agent-tasks/installer-robustness/TASK-04-netplan-renderer-dhcp.md -->
<!-- version: 1.0.0 -->
<!-- guid: c6c3b782-a858-457b-b33e-2fc57e2c3679 -->
<!-- last-edited: 2026-07-09 -->

# TASK-04 — Configurable netplan renderer (networkd | NetworkManager) + proper dhcp4 rendering for dhcp addresses (todo:renderer)

**Priority:** P2 · **Effort:** M · **Recommended subagent:** Sonnet-class · rust-backend subagent · **Why:** additive config field + template branch; invalid-netplan risk is in the fallback path only. · **Depends on:** none (wave 2 — serialized after TASK-01 merges [`system_setup.rs`] and TASK-06 merges [`config.rs`])

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/installer-robustness-netplan-renderer-dhcp" -b agent/installer-robustness-netplan-renderer-dhcp origin/main
cd "$REPO/.worktrees/installer-robustness-netplan-renderer-dhcp"
git rebase origin/main
```

(Protocol is also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

## Goal

Two additive changes to the Path B netplan pipeline:

1. **`src/network/ssh_installer/config.rs`** — add
   `pub network_renderer: String` to `InstallationConfig` with
   `#[serde(default = "default_network_renderer")]` (default `"networkd"`), so every existing
   YAML (which has no such key) keeps parsing unchanged. Copy the default-fn shape that already
   exists in this file (`default_tang_threshold` / `default_true` / `default_tpm2_pcr_ids`) —
   do NOT invent an enum or a new pattern.
2. **`src/network/ssh_installer/system_setup.rs`** — extract the inline netplan `format!` in
   `setup_network_configuration` into a pure, unit-testable builder (same associated-fn +
   unit-test shape as the existing `build_crypttab_entry`), substitute the configured renderer,
   validate it (`"networkd"` or `"NetworkManager"`, else error), and render `dhcp4: true` when
   `network_address == "dhcp"` instead of emitting `"dhcp"` as a literal address.

Reuse: `crate::error::AutoInstallError::ConfigError` for the invalid-renderer error;
`serde_yaml` (already a dependency) for the default-when-absent test. No new dependencies.

## Background (verify before editing)

- Current behavior (scout-verified 2026-07-09): `setup_network_configuration` writes
  `/etc/netplan/01-netcfg.yaml` with a hardcoded `renderer: networkd` and always renders
  `config.network_address` under `addresses:` — when the address is the literal `"dhcp"`
  (today's hardcoded autodetect value; after TASK-03, the truthful DHCP marker) the rendered
  netplan is INVALID. Only the static `--config` path is proven on hardware.
- Contract with TASK-03 (merged in the same wave, different file): `detect_network_config`
  returns the exact literal `"dhcp"` in `network_address` for DHCP-assigned interfaces. This
  task's branch keys on that literal, compared case-insensitively.
- TASK-06 (wave 1, already merged when this task starts) added `deny_unknown_fields` and YAML
  round-trip tests to `InstallationConfig` — your new field MUST carry a serde default so those
  tests and every committed example config (`examples/configs/install/*.yaml`, none of which
  have a `network_renderer` key) keep parsing. Do not edit the example configs.
- **Compiler-forced one-liners (report these to the coordinator):** adding a field to
  `InstallationConfig` breaks every exhaustive struct literal (rustc E0063). Besides
  `for_len_serv_003()` in `config.rs` (yours), the literals live in `src/cli/commands.rs`
  (`create_local_installation_config`, TASK-03's file) and in a `#[cfg(test)]`
  `sample_config()` in `src/network/ssh_installer/installer.rs` (TASK-05's file). You add
  EXACTLY ONE initializer line (plus the mandatory header bump) to each — nothing else in
  those files. The coordinator serializes the wave-2 merges because of this contact.
- HARD RULES (restate): code-only task — never run against 172.16.2.30 ("the server") or
  len-serv-003; validation is unit tests (QEMU later). No real secrets in git — committed
  configs carry `REPLACE_AT_PLACE_TIME`; this task adds no secret-bearing field. Workers never
  push/PR/merge.

**Re-verify these anchors before editing** — line numbers drift, they are a starting point only.
Zero hits = STOP and report:

```bash
grep -n 'renderer: networkd' src/network/ssh_installer/system_setup.rs   # expect: 1 hit ~line 199, inside setup_network_configuration (fn at ~193 via grep -n 'fn setup_network_configuration')
grep -n 'fn setup_network_configuration' src/network/ssh_installer/system_setup.rs   # expect: 1 hit ~line 193
grep -n 'pub disk_device' src/network/ssh_installer/config.rs   # expect: 1 hit at line 49 (the InstallationConfig field block)
grep -n 'pub network_' src/network/ssh_installer/config.rs   # expect: 5 hits ~lines 53-57 — insert the new field right after network_nameservers
grep -n 'fn default_tang_threshold' src/network/ssh_installer/config.rs   # expect: 1 hit — the serde-default fn shape to copy
grep -n 'fn build_crypttab_entry' src/network/ssh_installer/system_setup.rs   # expect: 1 hit ~line 43 — the pure-builder + unit-test shape to copy
grep -rn 'InstallationConfig {' src --include='*.rs'   # expect: 3 struct-literal sites: config.rs (for_len_serv_003), cli/commands.rs ~608, installer.rs ~621 (#[cfg(test)] sample_config) — the compiler will force one line in each
```

## Step-by-step

1. Run the anchor greps. Zero hits → STOP and report.
2. **config.rs:** after `pub network_nameservers: Vec<String>,` add:

   ```rust
   /// Netplan renderer for the installed system: "networkd" (default) or
   /// "NetworkManager". Validated at render time.
   #[serde(default = "default_network_renderer")]
   pub network_renderer: String,
   ```

   Next to the existing default fns add
   `pub(crate) fn default_network_renderer() -> String { "networkd".to_string() }`
   (`pub(crate)` so the other struct literals can reuse it — no duplicated string constants).
   Add `network_renderer: default_network_renderer(),` to the `for_len_serv_003()` literal.
3. Run `cargo build --offline`. For each E0063 missing-field error, add EXACTLY the line
   `network_renderer: crate::network::ssh_installer::config::default_network_renderer(),`
   (adjust the path to the file's existing imports — `super::config::…` inside
   `ssh_installer/installer.rs`) to the flagged literal. Expected sites:
   `src/cli/commands.rs` (`create_local_installation_config`) and
   `src/network/ssh_installer/installer.rs` (`sample_config` test helper). Make NO other edit
   in those two files beyond this line + their header bumps, and name the contact in your final
   report (they belong to wave-2 siblings TASK-03/TASK-05).
4. **system_setup.rs:** extract the netplan body into a pure associated fn (same shape as
   `build_crypttab_entry`):
   `fn build_netplan_yaml(config: &InstallationConfig) -> Result<String>`:
   - Validate first: `match config.network_renderer.as_str() { "networkd" | "NetworkManager" => {} , other => return Err(AutoInstallError::ConfigError(format!("unsupported network_renderer '{other}' (expected \"networkd\" or \"NetworkManager\")"))) }`.
     Exact-match only — do not "helpfully" normalize case or accept aliases.
   - **DHCP branch** — if `config.network_address.eq_ignore_ascii_case("dhcp")`, render:

     ```yaml
     network:
       version: 2
       renderer: {renderer}
       ethernets:
         {interface}:
           dhcp4: true
     ```

     with NO `addresses:`, `routes:`, or `nameservers:` blocks (DHCP supplies them; the
     configured gateway/nameservers are intentionally ignored on this branch).
   - **Static branch** — the existing template byte-identical except `renderer: networkd`
     becomes `renderer: {renderer}`.
   - Edge semantics (spelled here and checked in acceptance): ONLY the exact word `dhcp`
     (case-insensitive) selects the DHCP branch. An empty or otherwise odd `network_address`
     falls through to the static branch and renders as-is — identical to today's behavior; do
     not add extra guessing or validation of addresses in this task.
   - `setup_network_configuration` becomes: call `Self::build_netplan_yaml(config)?`, then the
     existing `mkdir -p` + heredoc `cat > /etc/netplan/01-netcfg.yaml` execution, unchanged.
5. Purely additive elsewhere: do not touch the other `system_setup.rs` functions (TASK-01's
   merged helper calls, boot-prod/TASK-01 lands here next wave), do not reorder fields, do not
   rename anything.
6. Add unit tests (in `system_setup.rs`'s existing `mod tests`, plus one in `config.rs`):
   - `test_build_netplan_yaml_default_renderer_static` — a config with a static address and the
     default renderer renders `renderer: networkd` AND still contains its `addresses:` block
     (anti-over-suppression: the new validation guard must not block the proven default path).
   - `test_build_netplan_yaml_networkmanager` — `network_renderer: "NetworkManager"` renders
     `renderer: NetworkManager`.
   - `test_build_netplan_yaml_rejects_unknown_renderer` — `"netword"` → `Err`.
   - `test_build_netplan_yaml_dhcp` — `network_address: "dhcp"` → output contains
     `dhcp4: true`, does NOT contain `addresses:`, and does NOT contain `- dhcp`.
   - `test_build_netplan_yaml_dhcp_uppercase` — `"DHCP"` also takes the dhcp4 branch.
   - In `config.rs`: `test_network_renderer_defaults_when_absent` — serialize
     `InstallationConfig::for_len_serv_003()` with `serde_yaml::to_string`, drop the
     `network_renderer` line (`.lines().filter(|l| !l.contains("network_renderer"))…`), parse it
     back with `serde_yaml::from_str::<InstallationConfig>` and assert
     `back.network_renderer == "networkd"` (old-YAML compatibility, the serde default).
7. Bump file headers (`version` + `last-edited`, keep `guid`) on: `config.rs`,
   `system_setup.rs`, `src/cli/commands.rs`, `src/network/ssh_installer/installer.rs`. Run the
   gate, commit.

## How to test

```bash
cargo test --lib --offline    # Expected: 237+ passed; 0 failed (baseline 237 + the new netplan/config tests)
cargo build --offline         # Expected: exit 0
cargo clippy --offline        # Expected: no new warnings
```

## Acceptance criteria

- [ ] `grep -c 'network_renderer' src/network/ssh_installer/config.rs` → ≥3 (field, default fn,
  for_len_serv_003 initializer).
- [ ] `grep -n 'dhcp4: true' src/network/ssh_installer/system_setup.rs` → ≥1 hit;
  `grep -n 'fn build_netplan_yaml' src/network/ssh_installer/system_setup.rs` → 1 hit.
- [ ] `grep -n 'renderer: networkd' src/network/ssh_installer/system_setup.rs` → hits only
  inside tests/expected strings, not as the hardcoded template literal (the template now
  substitutes the configured renderer).
- [ ] One-line-only sibling contact:
  `git diff origin/main -- src/cli/commands.rs src/network/ssh_installer/installer.rs` shows,
  per file, exactly one `network_renderer:` insertion plus the header-bump lines — nothing else.
- [ ] Renderer guard works both ways: `cargo test --lib --offline test_build_netplan_yaml_rejects_unknown_renderer`
  passes, and — Anti-over-suppression: — `... test_build_netplan_yaml_default_renderer_static`
  passes — the default networkd static config still renders.
- [ ] DHCP semantics: `cargo test --lib --offline test_build_netplan_yaml_dhcp` and
  `... test_build_netplan_yaml_dhcp_uppercase` pass (dhcp4 branch, no literal `- dhcp` address).
- [ ] Old YAML parses: `cargo test --lib --offline test_network_renderer_defaults_when_absent`
  passes (serde default keeps every committed example config valid).
- [ ] Tests green: `cargo test --lib --offline` 237+ passed, 0 failed; `cargo build --offline`
  and `cargo clippy --offline` clean.
- [ ] File headers bumped on all four touched files (`grep -n 'last-edited:' <file>` shows
  today's date; guids unchanged).

## Commit message

```
feat(installer): configurable netplan renderer + dhcp4 rendering for dhcp addresses

Adds InstallationConfig.network_renderer ("networkd" default via serde default,
"NetworkManager" allowed, anything else is a ConfigError at render time) and
extracts the netplan template into the pure builder build_netplan_yaml. When
network_address is "dhcp" (the marker detect_network_config now emits) the
template renders dhcp4: true instead of an invalid literal address; static
configs render byte-identically to before apart from the renderer substitution.
Old YAMLs without the new key parse unchanged.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

Already-done check (additive polarity — grep for the new thing's presence): if
`grep -n 'network_renderer' src/network/ssh_installer/config.rs` hits, the field is already
applied — run the acceptance checks instead of re-applying. Rollback: `git revert` the single
commit — removes the field, the builder, and the three one-line initializers; YAMLs without the
key parse identically before and after (serde default), so no config on disk needs changing and
siblings are unaffected.
